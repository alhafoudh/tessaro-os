//! The cameras, as their mirrors report them.
//!
//! Every USB camera is opened by one `tessaro-camera@<device>.service` and
//! nothing else; the mirror republishes it as a v4l2loopback device that the
//! browser and any other reader open (docs/camera.md). Each mirror writes what
//! it captures to `<camera_dir>/<device>.json` and removes it when it stops, so
//! the files in that directory are the cameras there are.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use protocol::CameraInfo;

/// Every camera a mirror has reported, ordered by device. A file that does
/// not parse is skipped: a mirror writes it whole or not at all, so that is a
/// mirror from another image, not a camera.
pub fn snapshot(dir: &Path) -> Result<Vec<CameraInfo>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("{}: {err}", dir.display())),
    };
    let mut cameras: Vec<CameraInfo> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| fs::read(&path).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect();
    cameras.sort_by(|a, b| natural(&a.device).cmp(&natural(&b.device)));
    Ok(cameras)
}

/// `video2` before `video10`.
fn natural(device: &str) -> (usize, &str) {
    (device.len(), device)
}

/// A frame no older than this is served as it is; an older one is waited
/// past for the next.
pub const SNAPSHOT_FRESH: Duration = Duration::from_secs(2);

/// Ask `device`'s mirror for frames: it writes `<device>.jpg` while
/// `<device>.want` was touched in the last few seconds (docs/camera.md,
/// Snapshots). Refuses a device no mirror reported.
pub fn want_snapshot(dir: &Path, device: &str) -> Result<(), String> {
    let known = is_node_name(device) && dir.join(format!("{device}.json")).is_file();
    if !known {
        return Err(format!(
            "no camera {device}; tessaro-ctl camera list names them"
        ));
    }
    let want = dir.join(format!("{device}.want"));
    // Created, or its mtime moved to now: both are what the mirror looks at.
    fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&want)
        .map_err(|err| format!("{}: {err}", want.display()))?;
    Ok(())
}

/// `device`'s newest frame and its age, when it is at most `fresh` old.
pub fn snapshot_frame(
    dir: &Path,
    device: &str,
    fresh: Duration,
) -> Result<Option<(Vec<u8>, Duration)>, String> {
    let path = dir.join(format!("{device}.jpg"));
    let modified = match fs::metadata(&path).and_then(|meta| meta.modified()) {
        Ok(modified) => modified,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("{}: {err}", path.display())),
    };
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    if age > fresh {
        return Ok(None);
    }
    match fs::read(&path) {
        Ok(bytes) => Ok(Some((bytes, age))),
        // Gone between the stat and the read: the mirror stopped.
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// A kernel node name, `video0`: the only thing a request may put into a
/// file name under the camera directory.
fn is_node_name(device: &str) -> bool {
    device
        .strip_prefix("video")
        .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::CameraMode;

    fn camera(device: &str) -> CameraInfo {
        CameraInfo {
            name: "HD Webcam".to_string(),
            device: device.to_string(),
            bus: "usb-0000:00:14.0-2".to_string(),
            mirrors: vec![protocol::CameraMirror {
                name: "HD Webcam Mirror 1".to_string(),
                device: "/dev/video50".to_string(),
            }],
            mode: Some(CameraMode {
                format: "mjpeg".to_string(),
                width: 1280,
                height: 720,
                fps: 30,
            }),
            fallback: None,
            error: None,
            modes: Vec::new(),
        }
    }

    #[test]
    fn lists_what_the_mirrors_wrote_in_device_order_and_skips_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        for device in ["video10", "video2"] {
            let json = serde_json::to_vec(&camera(device)).unwrap();
            fs::write(dir.path().join(format!("{device}.json")), json).unwrap();
        }
        fs::write(dir.path().join("video4.json"), "{half").unwrap();
        fs::write(dir.path().join("camera.env"), "KIOSK_CAMERA_FORMAT=auto\n").unwrap();

        let devices: Vec<String> = snapshot(dir.path())
            .unwrap()
            .into_iter()
            .map(|camera| camera.device)
            .collect();
        assert_eq!(devices, ["video2", "video10"]);
    }

    #[test]
    fn no_directory_is_no_cameras() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(snapshot(&dir.path().join("missing")).unwrap(), []);
    }

    #[test]
    fn a_snapshot_is_asked_of_a_known_camera_only_and_by_node_name() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("video0.json"),
            serde_json::to_vec(&camera("video0")).unwrap(),
        )
        .unwrap();

        want_snapshot(dir.path(), "video0").unwrap();
        assert!(dir.path().join("video0.want").is_file());
        assert!(want_snapshot(dir.path(), "video1").is_err());
        for bad in ["../video0", "video", "video0.json", "Video0", "camera"] {
            assert!(want_snapshot(dir.path(), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_frame_is_served_while_fresh() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            snapshot_frame(dir.path(), "video0", SNAPSHOT_FRESH).unwrap(),
            None
        );

        fs::write(dir.path().join("video0.jpg"), b"\xff\xd8jpeg").unwrap();
        let (bytes, age) = snapshot_frame(dir.path(), "video0", SNAPSHOT_FRESH)
            .unwrap()
            .unwrap();
        assert_eq!(bytes, b"\xff\xd8jpeg");
        assert!(age < SNAPSHOT_FRESH);
        assert_eq!(
            snapshot_frame(dir.path(), "video0", Duration::ZERO).unwrap(),
            None
        );
    }
}
