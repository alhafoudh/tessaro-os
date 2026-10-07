//! Presence detection on the running agent: the datagrams tessaro-vision
//! sends, turned into events for the journal, the page and the scripts
//! (`crate::presence` decides what they mean), and what `tessaro-ctl camera
//! presence` and `camera calibrate` answer. See docs/presence.md.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use protocol::keys;
use protocol::presence::{
    Calibrated, PresenceEvent, PresenceStatus, PresenceSummary, VisionFrame, VisionStatus,
};
use serde_json::json;
use tokio::net::UnixDatagram;

use super::bridge::{page_face, page_faces};
use super::{Caller, Control, Reply};
use crate::config;
use crate::deadline::blocking;
use crate::presence::{self, Settings};
use crate::sync::lock;

/// How often time is looked at with no frame coming: someone gone for
/// camera.presence.linger has left even if the vision service went quiet.
const TICK: Duration = Duration::from_secs(1);

/// A frame older than this is not who is in front of the screen now.
const FRESH: Duration = Duration::from_secs(3);

/// The vision service writes its status every 2s; one older than this is a
/// service that is not running.
const RUNNING: i64 = 10;

/// The largest datagram taken: a frame of faces is a few hundred bytes each.
const DATAGRAM_MAX: usize = 64 * 1024;

/// The distances `camera calibrate` takes, in centimeters.
const CALIBRATE_CM: std::ops::RangeInclusive<u32> = 30..=1000;

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

impl Control {
    pub fn watch_presence(self: &Arc<Self>) {
        let control = Arc::clone(self);
        // naked: the listener waits only for a datagram, a tick or the stop signal; the rest is bounded inside it
        tokio::spawn(async move { control.presence_listen().await });
    }

    /// Listen for the vision service's frames until the agent stops, and
    /// start the service when camera.presence.enable is on: it runs on no
    /// boot target of its own, so a reboot leaves it to the agent.
    async fn presence_listen(self: Arc<Self>) {
        let socket = match self.presence_socket().await {
            Ok(socket) => socket,
            Err(err) => {
                self.log.info(format!("presence: {err}"));
                return;
            }
        };
        if self.current.borrow().config.presence.enable {
            if let Err(err) = self.bus.start(&self.paths.vision_unit).await {
                self.log.info(format!(
                    "presence: starting {}: {err}",
                    self.paths.vision_unit
                ));
            }
        }
        let mut shutdown = self.shutdown.clone();
        let mut buffer = vec![0u8; DATAGRAM_MAX];
        let mut tick = tokio::time::interval(TICK);
        loop {
            // naked: the next datagram, a tick or the stop signal, whichever comes first
            tokio::select! {
                received = socket.recv(&mut buffer) => {
                    let mut frames = Vec::new();
                    if let Ok(len) = received {
                        frames.extend(parse(&buffer[..len]));
                    }
                    // What queued while the last frame was handled: every one
                    // counts for the timing, only the newest goes to the page.
                    while let Ok(len) = socket.try_recv(&mut buffer) {
                        frames.extend(parse(&buffer[..len]));
                    }
                    self.presence_frames(frames).await;
                }
                _ = tick.tick() => self.presence_tick().await,
                _ = shutdown.changed() => break,
            }
        }
    }

    /// The datagram socket in the agent's run directory, root's alone: only
    /// the vision service, which runs as root, may tell the agent who is
    /// there.
    async fn presence_socket(&self) -> Result<UnixDatagram, String> {
        let path = self.paths.vision_socket.clone();
        let socket = blocking("binding the presence socket", move || {
            match std::fs::remove_file(&path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                    return Err(format!("{}: {err}", path.display()));
                }
                _ => {}
            }
            let socket = std::os::unix::net::UnixDatagram::bind(&path)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|err| format!("{}: {err}", path.display()))?;
            socket
                .set_nonblocking(true)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            Ok(socket)
        })
        .await?;
        UnixDatagram::from_std(socket).map_err(|err| err.to_string())
    }

    async fn presence_frames(&self, frames: Vec<VisionFrame>) {
        let settings = self.current.borrow().config.presence.clone();
        if !settings.enable || frames.is_empty() {
            return;
        }
        let thresholds = Settings::from(&settings);
        let now = Instant::now();
        let (events, newest) = {
            let mut state = lock(&self.presence);
            let mut events = Vec::new();
            for frame in &frames {
                events.extend(state.frame(now, frame, &thresholds));
            }
            if let Some(event) = events.last() {
                state.last = Some((event, unix_now()));
            }
            (events, state.frame.as_ref().map(|(_, frame)| frame.clone()))
        };
        for event in events {
            self.presence_event(event, &settings).await;
        }
        if settings.page {
            if let Some(frame) = newest {
                self.bridge_faces(page_faces(&frame)).await;
            }
        }
    }

    async fn presence_tick(&self) {
        let settings = self.current.borrow().config.presence.clone();
        if !settings.enable {
            // Nothing is decided while it is off, and nothing is left over
            // for when it is switched on again.
            *lock(&self.presence) = presence::Presence::default();
            return;
        }
        let events = {
            let mut state = lock(&self.presence);
            let events = state.tick(Instant::now(), &Settings::from(&settings));
            if let Some(event) = events.last() {
                state.last = Some((event, unix_now()));
            }
            events
        };
        for event in events {
            self.presence_event(event, &settings).await;
        }
    }

    /// One event to the journal, the page and the scripts that run on it.
    async fn presence_event(&self, event: &'static str, settings: &config::Presence) {
        let (detail, count) = {
            let state = lock(&self.presence);
            let faces: Vec<serde_json::Value> =
                state.frame.as_ref().map_or_else(Vec::new, |(_, frame)| {
                    frame.faces.iter().map(page_face).collect()
                });
            let count = faces.len();
            (
                json!({
                    "event": event,
                    "present": state.present,
                    "near": state.near,
                    "count": count,
                    "faces": faces,
                }),
                count,
            )
        };
        self.log
            .info(format!("presence: {event}, with {count} face(s) in view"));
        if settings.page {
            self.bridge_presence(detail).await;
        }
        if settings.scripts {
            self.presence_scripts(event).await;
        }
    }

    /// Presence in `device status`, while camera.presence.enable is on.
    pub(super) fn presence_summary(&self) -> Option<PresenceSummary> {
        if !self.current.borrow().config.presence.enable {
            return None;
        }
        let state = lock(&self.presence);
        Some(PresenceSummary {
            present: state.present,
            near: state.near,
            count: state.count(Instant::now(), FRESH),
        })
    }

    /// `tessaro-ctl camera presence`.
    pub(super) async fn camera_presence(&self) -> Result<PresenceStatus, String> {
        let settings = self.current.borrow().config.presence.clone();
        let (present, near, last, frame) = {
            let state = lock(&self.presence);
            let frame = state
                .frame
                .as_ref()
                .filter(|(at, _)| at.elapsed() <= FRESH)
                .map(|(_, frame)| frame.clone());
            (state.present, state.near, state.last, frame)
        };
        let path = self.paths.vision_dir.join("status.json");
        let (vision, last) = blocking("reading presence detection's status", move || {
            let vision: Option<VisionStatus> = std::fs::read(&path)
                .ok()
                .and_then(|body| serde_json::from_slice(&body).ok());
            let last = last.map(|(event, unix)| PresenceEvent {
                event: event.to_string(),
                at: crate::schedules::moment(unix),
            });
            Ok((vision, last))
        })
        .await?;
        let running = settings.enable
            && vision
                .as_ref()
                .is_some_and(|vision| (unix_now() - vision.updated).abs() <= RUNNING);
        let vision = vision.filter(|_| running).unwrap_or_default();
        Ok(PresenceStatus {
            enabled: settings.enable,
            running,
            camera: vision.camera,
            model: settings.model,
            fps: vision.fps,
            inference_ms: vision.inference_ms,
            error: vision.error,
            present,
            near,
            near_m: settings.near.map(|cm| cm as f64 / 100.0),
            fov: settings.fov as f64,
            last,
            frame,
        })
    }

    /// `tessaro-ctl camera calibrate`: the field of view that puts the one
    /// face in view at `distance_cm`, saved as camera.presence.fov.
    pub(super) async fn camera_calibrate(
        self: &Arc<Self>,
        caller: &Caller,
        distance_cm: u32,
    ) -> Reply {
        if !CALIBRATE_CM.contains(&distance_cm) {
            return Reply::err("stand 0.3 to 10 m from the camera to calibrate");
        }
        if !self.current.borrow().config.presence.enable {
            return Reply::err(
                "presence detection is off; switch it on with `tessaro-ctl camera presence on` first",
            );
        }
        let faces = {
            let state = lock(&self.presence);
            state
                .frame
                .as_ref()
                .filter(|(at, _)| at.elapsed() <= FRESH)
                .map(|(_, frame)| frame.faces.clone())
        };
        let Some(faces) = faces else {
            return Reply::err(
                "presence detection has not looked at a frame in the last few seconds; see `tessaro-ctl camera presence`",
            );
        };
        let [face] = faces.as_slice() else {
            return Reply::err(format!(
                "calibrating needs exactly one face in view, and it sees {}",
                faces.len()
            ));
        };
        let distance = f64::from(distance_cm) / 100.0;
        let fov = presence::fov(face.area.w, distance);
        let changes = BTreeMap::from([(keys::PRESENCE_FOV.to_string(), Some(format!("{fov}")))]);
        let reply = self
            .change(caller, changes, None, true, Default::default(), None)
            .await;
        if reply.result.is_err() {
            return reply;
        }
        self.log.info(format!(
            "presence: calibrated by {} at {distance} m: camera.presence.fov={fov}",
            caller.describe()
        ));
        Reply::ok(Calibrated {
            distance,
            width: face.area.w,
            fov,
        })
    }
}

/// A datagram as a frame; anything else is dropped.
fn parse(body: &[u8]) -> Option<VisionFrame> {
    serde_json::from_slice(body).ok()
}
