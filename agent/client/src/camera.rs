//! Camera snapshots, for `tessaro-ctl camera snapshot` and the GUI's Camera
//! page: which camera a snapshot is of, one snapshot, and a new one every
//! so often until the caller stops.

use std::time::{Duration, Instant};

use protocol::api::{self, CameraRef, HEADER_FRAME_AGE};
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
