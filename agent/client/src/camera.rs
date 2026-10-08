//! Camera snapshots and presence detection, for `tessaro-ctl camera` and
//! the GUI's Camera page: which camera a snapshot is of, one snapshot, and a
//! new one every so often until the caller stops; who is in front of the
//! screen, the same way; calibrating the distances.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use protocol::api::{self, CalibrateBody, CameraRef, HEADER_FRAME_AGE};
use protocol::keys;
use protocol::presence::{Calibrated, PresenceStatus};
use protocol::CameraList;

use crate::connect::{Answer, Session};
use crate::report::Report;
use crate::text::{Line, Tone};

/// The newest frame of a camera, as its mirror took it.
pub struct Snapshot {
    pub jpeg: Vec<u8>,
    /// How old the frame was when the device answered, where it said.
    pub age: Option<Duration>,
}

/// One snapshot of `device` (`video0`). The first after a pause may take a
/// few seconds: the mirror hands frames out only while they are asked for.
pub fn snapshot(session: &mut Session, device: &str) -> Answer<Snapshot> {
    let download = match session.download::<api::camera::Snapshot>(CameraRef {
        device: device.to_string(),
    }) {
        Answer::Ok(download) => download,
        Answer::Refused(error) => return Answer::Refused(error),
        Answer::Lost(why) => return Answer::Lost(why),
    };
    let age = download
        .header(HEADER_FRAME_AGE)
        .and_then(|ms| ms.trim().parse::<u64>().ok())
        .map(Duration::from_millis);
    Answer::Ok(Snapshot {
        jpeg: download.body,
        age,
    })
}

/// The camera a snapshot is of: `asked` (`video0` or `/dev/video0`) if the
/// device has it, else the only camera. With several and none asked, the
/// refusal names them.
pub fn pick(list: &CameraList, asked: Option<&str>) -> Result<String, String> {
    let devices = || {
        list.cameras
            .iter()
            .map(|camera| camera.device.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    if list.cameras.is_empty() {
        return Err("the device has no cameras; plug in a USB camera".to_string());
    }
    match asked {
        Some(asked) => {
            let device = asked.strip_prefix("/dev/").unwrap_or(asked);
            if list.cameras.iter().any(|camera| camera.device == device) {
                Ok(device.to_string())
            } else {
                Err(format!("no camera {device}; the device has {}", devices()))
            }
        }
        None if list.cameras.len() == 1 => Ok(list.cameras[0].device.clone()),
        None => Err(format!(
            "the device has several cameras, name one: {}",
            devices()
        )),
    }
}

/// Where a snapshot is saved unless told: `<node>-<device>.jpg`.
pub fn file_name(node: &str, device: &str) -> String {
    format!("{node}-{device}.jpg")
}

/// `frame 120 ms old`, what a snapshot's age reads as.
pub fn age_text(age: Duration) -> String {
    format!("frame {} ms old", age.as_millis())
}

/// Whether anyone is in front of the screen, and how presence detection
/// runs: `tessaro-ctl camera presence` and the Camera page.
pub fn presence(session: &mut Session) -> Result<PresenceStatus, String> {
    session.fetch::<api::camera::Presence>()
}

/// The presence status every `interval`, each handed to `each`, until the
/// report says stop. A failed read is reported and the watching goes on.
pub fn watch_presence(
    session: &mut Session,
    interval: Duration,
    report: &mut dyn Report,
    mut each: impl FnMut(PresenceStatus) -> Result<(), String>,
) -> Result<(), String> {
    let interval = interval.max(Duration::from_millis(100));
    loop {
        if report.stopped() {
            return Ok(());
        }
        let started = Instant::now();
        match presence(session) {
            Ok(status) => each(status)?,
            Err(error) => report.line(Line::of(Tone::Bad, error)),
        }
        let until = started + interval;
        while Instant::now() < until && !report.stopped() {
            std::thread::sleep(Duration::from_millis(50).min(until - Instant::now()));
        }
    }
}

/// The settings `camera presence on|off` saves: camera.presence.enable, and
/// with `on` the camera, the near distance and whether age and gender are
/// estimated (camera.presence.demographics), where given (empty is not
/// given). Off with any of them is refused, as the command line refuses it.
pub fn presence_change(
    on: bool,
    camera: Option<&str>,
    near: Option<&str>,
    demographics: Option<bool>,
) -> Result<BTreeMap<String, String>, String> {
    let camera = camera.map(str::trim).filter(|value| !value.is_empty());
    let near = near.map(str::trim).filter(|value| !value.is_empty());
    if !on && (camera.is_some() || near.is_some() || demographics.is_some()) {
        return Err(
            "--camera, --near and --demographics go with `tessaro-ctl camera presence on`"
                .to_string(),
        );
    }
    let flag = |on: bool| if on { "1" } else { "0" }.to_string();
    let mut values = BTreeMap::from([(keys::PRESENCE_ENABLE.to_string(), flag(on))]);
    if let Some(demographics) = demographics {
        values.insert(keys::PRESENCE_DEMOGRAPHICS.to_string(), flag(demographics));
    }
    for (key, value) in [(keys::PRESENCE_CAMERA, camera), (keys::PRESENCE_NEAR, near)] {
        if let Some(value) = value {
            let key_info = keys::find(key).ok_or_else(|| format!("no key {key}"))?;
            values.insert(key.to_string(), keys::validate(key_info, value)?);
        }
    }
    Ok(values)
}

/// Measure camera.presence.fov from the one face in view, of someone
/// standing `distance` meters away, and save it.
pub fn calibrate(session: &mut Session, distance: f64) -> Result<Calibrated, String> {
    if !(0.3..=10.0).contains(&distance) {
        return Err("stand 0.3 to 10 m from the camera to calibrate".to_string());
    }
    session.send::<api::camera::Calibrate>(CalibrateBody { distance })
}

/// A snapshot of `device` every `interval`, each handed to `each`, until
/// the report says stop. A failed one is reported and the watching goes
/// on, as `ping::device` does with a lost round trip.
pub fn watch(
    session: &mut Session,
    device: &str,
    interval: Duration,
    report: &mut dyn Report,
    mut each: impl FnMut(Snapshot) -> Result<(), String>,
) -> Result<(), String> {
    let interval = interval.max(Duration::from_millis(100));
    loop {
        if report.stopped() {
            return Ok(());
        }
        let started = Instant::now();
        match snapshot(session, device) {
            Answer::Ok(shot) => each(shot)?,
            Answer::Refused(error) | Answer::Lost(error) => {
                report.line(Line::of(Tone::Bad, format!("{device}: {error}")))
            }
        }
        let until = started + interval;
        while Instant::now() < until && !report.stopped() {
            std::thread::sleep(Duration::from_millis(50).min(until - Instant::now()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::CameraInfo;

    #[test]
    fn presence_on_takes_a_camera_a_near_distance_and_demographics_and_off_takes_none() {
        let on = presence_change(true, Some("HD Webcam"), Some("1.50"), Some(true)).unwrap();
        assert_eq!(on[keys::PRESENCE_ENABLE], "1");
        assert_eq!(on[keys::PRESENCE_CAMERA], "HD Webcam");
        assert_eq!(on[keys::PRESENCE_NEAR], "1.5");
        assert_eq!(on[keys::PRESENCE_DEMOGRAPHICS], "1");
        let plain = presence_change(true, None, None, None).unwrap();
        assert!(!plain.contains_key(keys::PRESENCE_DEMOGRAPHICS));
        let off = presence_change(false, None, Some(" "), None).unwrap();
        assert_eq!(off.len(), 1);
        assert_eq!(off[keys::PRESENCE_ENABLE], "0");
        assert!(presence_change(false, None, Some("2"), None).is_err());
        assert!(presence_change(false, None, None, Some(false)).is_err());
        assert!(presence_change(true, None, Some("far"), None).is_err());
    }

    fn list(devices: &[&str]) -> CameraList {
        CameraList {
            format: "auto".to_string(),
            size: "auto".to_string(),
            mirrors: 1,
            cameras: devices
                .iter()
                .map(|device| CameraInfo {
                    name: format!("Cam {device}"),
                    device: device.to_string(),
                    bus: "usb-1".to_string(),
                    mirrors: Vec::new(),
                    vision: None,
                    mode: None,
                    fallback: None,
                    error: None,
                    modes: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_only_camera_is_picked_without_asking() {
        assert_eq!(pick(&list(&["video0"]), None).unwrap(), "video0");
    }

    #[test]
    fn several_cameras_want_one_named() {
        let err = pick(&list(&["video0", "video2"]), None).unwrap_err();
        assert!(err.ends_with("video0, video2"), "{err}");
        assert_eq!(
            pick(&list(&["video0", "video2"]), Some("/dev/video2")).unwrap(),
            "video2"
        );
    }

    #[test]
    fn an_unknown_camera_is_refused() {
        let err = pick(&list(&["video0"]), Some("video4")).unwrap_err();
        assert!(err.starts_with("no camera video4"), "{err}");
        assert!(pick(&list(&[]), None).is_err());
    }
}
