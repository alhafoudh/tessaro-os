//! `tessaro-ctl camera ...`: the USB cameras, and how their mirrors capture.
//!
//! Only a camera's mirror, `tessaro-camera@<device>.service`, opens the
//! camera itself; the page and anything else read the virtual cameras it
//! republishes, one reader each. `format`, `size` and `mirrors` are each a
//! `config set` of one camera.* key: every running mirror restarts to
//! capture that way. `snapshot` asks a mirror for its newest frame, without
//! taking one of the virtual cameras.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::out::{eprintln, println};
use clap::builder::PossibleValuesParser;
use clap::Subcommand;
use protocol::api;
use protocol::keys;
use serde_json::json;
use tessaro_client::camera;
use tessaro_client::describe::camera as describe;
use tessaro_client::report::Report;
use tessaro_client::text::Line;

use crate::connect::Session;
use crate::style::{self, paint};
use crate::{print, show_applied};

#[derive(Subcommand)]
pub enum CameraCmd {
    /// Every USB camera: its node and the virtual cameras pages read, what
    /// its mirror captures and why, every format and size it has, and the
    /// saved camera.* settings.
    List,
    /// How cameras capture: auto (MJPEG where the camera has it, else
    /// YUYV), mjpeg or yuyv. A camera without the format uses auto. The
    /// same as `tessaro-ctl config set camera.format=...`.
    Format {
        #[arg(value_parser = PossibleValuesParser::new(keys::CAMERA_FORMATS))]
        format: String,
    },
    /// The frame size cameras capture at: auto, or WIDTHxHEIGHT from
    /// `tessaro-ctl camera list`. A camera without the size uses auto. The
    /// same as `tessaro-ctl config set camera.size=...`.
    ///
    ///   tessaro-ctl camera size 1280x720
    ///   tessaro-ctl camera size auto
    Size { size: String },
    /// Virtual cameras each camera gets, `<camera> Mirror 1` and up, all
    /// with the same picture. Each has one reader at a time - the page, or
    /// a service on the device - so this is how many may watch a camera at
    /// once. The same as `tessaro-ctl config set camera.mirrors=...`.
    ///
    ///   tessaro-ctl camera mirrors 2
    Mirrors {
        #[arg(value_parser = clap::value_parser!(u32).range(1..=keys::CAMERA_MIRRORS_MAX))]
        mirrors: u32,
    },
    /// Save the newest frame of a camera as a JPEG, taken by its mirror
    /// without a virtual camera of its own. DEVICE is its node from
    /// `tessaro-ctl camera list` (`video0`), needed only with several
    /// cameras. The first snapshot after a pause may take a few seconds.
    ///
    ///   tessaro-ctl camera snapshot
    ///   tessaro-ctl camera snapshot video2 -o door.jpg --watch 5
    Snapshot {
        /// The camera's node, `video0`; the only camera unless given.
        device: Option<String>,
        /// Where the JPEG goes; `<node>-<device>.jpg` unless given.
        #[arg(long, short)]
        output: Option<String>,
        /// Rewrite the file with a new snapshot every SECONDS, until Ctrl-C.
        #[arg(long, value_name = "SECONDS")]
        watch: Option<f64>,
    },
    /// Presence detection: whether anyone is in front of the screen, the
    /// faces in view and how far away each is, and how it runs. `on` and
    /// `off` switch it, the same as `tessaro-ctl config set
    /// camera.presence.enable=1`; each camera's mirrors restart once.
    ///
    ///   tessaro-ctl camera presence
    ///   tessaro-ctl camera presence on --near 1.2
    ///   tessaro-ctl camera presence on --demographics on
    ///   tessaro-ctl camera presence --watch 1
    Presence {
        state: Option<crate::Toggle>,
        /// With `on`: the camera to watch, by its name from `tessaro-ctl
        /// camera list`; the first one unless given.
        #[arg(long, value_name = "NAME")]
        camera: Option<String>,
        /// With `on`: meters within which someone counts as near, or off.
        #[arg(long, value_name = "METERS")]
        near: Option<String>,
        /// With `on`: estimate the age and gender of every face
        /// (camera.presence.demographics). Whether that may be used where
        /// the device stands is the owner's decision.
        #[arg(long, value_name = "on|off")]
        demographics: Option<crate::Toggle>,
        /// Show it again every SECONDS, until Ctrl-C.
        #[arg(long, value_name = "SECONDS", conflicts_with = "state")]
        watch: Option<f64>,
    },
    /// Measure the camera's field of view with one person standing
    /// METERS from it, facing it, and nobody else in view, and save it as
    /// camera.presence.fov: distances are right from then on.
    ///
    ///   tessaro-ctl camera calibrate --distance 1
    Calibrate {
        #[arg(long, value_name = "METERS")]
        distance: f64,
    },
}

pub fn run(session: &mut Session, command: CameraCmd, json: bool) -> Result<(), String> {
    match command {
        CameraCmd::List => {
            let list = session.fetch::<api::camera::List>()?;
            print(json, &list, || {
                for line in describe::list(&list) {
                    println!("{}", style::line(&line));
                }
            })
        }
        CameraCmd::Format { format } => set(session, json, keys::CAMERA_FORMAT, format),
        CameraCmd::Size { size } => set(session, json, keys::CAMERA_SIZE, size),
        CameraCmd::Mirrors { mirrors } => {
            set(session, json, keys::CAMERA_MIRRORS, mirrors.to_string())
        }
        CameraCmd::Snapshot {
            device,
            output,
            watch,
        } => snapshot(session, json, device, output, watch),
        CameraCmd::Presence {
            state: Some(state),
            camera,
            near,
            demographics,
            ..
        } => {
            let values = camera::presence_change(
                state == crate::Toggle::On,
                camera.as_deref(),
                near.as_deref(),
                demographics.map(|toggle| toggle == crate::Toggle::On),
            )?;
            let applied = crate::set(session, values)?;
            print(json, &applied, || show_applied(&applied, false))
        }
        CameraCmd::Presence {
            state: None,
            watch: None,
            ..
        } => {
            let status = camera::presence(session)?;
            print(json, &status, || show_presence(&status))
        }
        CameraCmd::Presence {
            state: None,
            watch: Some(seconds),
            ..
        } => {
            if seconds.is_nan() || seconds <= 0.0 || seconds.is_infinite() {
                return Err("--watch: the interval must be more than 0 seconds".to_string());
            }
            // Watching ends with Ctrl-C, which ends the process.
            camera::watch_presence(
                session,
                Duration::from_secs_f64(seconds),
                &mut Stderr,
                |status| {
                    if json {
                        return crate::print_json(&status);
                    }
                    show_presence(&status);
                    println!();
                    Ok(())
                },
            )
        }
        CameraCmd::Calibrate { distance } => {
            let calibrated = camera::calibrate(session, distance)?;
            print(json, &calibrated, || {
                println!("{}", style::line(&describe::calibrated(&calibrated)))
            })
        }
    }
}

fn show_presence(status: &protocol::presence::PresenceStatus) {
    for line in describe::presence(status) {
        println!("{}", style::line(&line));
    }
}

/// `tessaro-ctl camera snapshot`: one JPEG, or with `--watch` a new one
/// into the same file every so often, one line per write.
fn snapshot(
    session: &mut Session,
    json: bool,
    device: Option<String>,
    output: Option<String>,
    watch: Option<f64>,
) -> Result<(), String> {
    let list = session.fetch::<api::camera::List>()?;
    let device = camera::pick(&list, device.as_deref())?;
    let path = output.unwrap_or_else(|| camera::file_name(&session.node.name, &device));
    let save = |shot: camera::Snapshot| -> Result<(), String> {
        std::fs::write(&path, &shot.jpeg).map_err(|err| format!("{path}: {err}"))?;
        if json {
            return crate::print_json(&json!({
                "path": path,
                "device": device,
                "bytes": shot.jpeg.len(),
                "age_ms": shot.age.map(|age| age.as_millis() as u64),
            }));
        }
        let details = match shot.age {
            Some(age) => format!("({} bytes, {})", shot.jpeg.len(), camera::age_text(age)),
            None => format!("({} bytes)", shot.jpeg.len()),
        };
        println!(
            "{} {}",
            paint(style::OK, &path),
            paint(style::MUTED, details)
        );
        Ok(())
    };
    match watch {
        None => save(camera::snapshot(session, &device).into_result()?),
        Some(seconds) if seconds.is_nan() || seconds <= 0.0 || seconds.is_infinite() => {
            Err("--watch: the interval must be more than 0 seconds".to_string())
        }
        // Watching ends with Ctrl-C, which ends the process.
        Some(seconds) => camera::watch(
            session,
            &device,
            Duration::from_secs_f64(seconds),
            &mut Stderr,
            save,
        ),
    }
}

/// A failed snapshot while watching, on stderr, so stdout keeps one line
/// per file written.
struct Stderr;

impl Report for Stderr {
    fn progress(&mut self, _: Line, _: u64, _: u64) {}

    fn line(&mut self, line: Line) {
        eprintln!("{}", style::line(&line));
    }
}

fn set(session: &mut Session, json: bool, key: &str, value: String) -> Result<(), String> {
    let applied = crate::set(session, BTreeMap::from([(key.to_string(), value)]))?;
    print(json, &applied, || show_applied(&applied, false))
}
