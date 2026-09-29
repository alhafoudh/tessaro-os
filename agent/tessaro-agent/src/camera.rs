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

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::CameraMode;

    fn camera(device: &str) -> CameraInfo {
        CameraInfo {
            name: "HD Webcam".to_string(),
            device: device.to_string(),
            bus: "usb-0000:00:14.0-2".to_string(),
            virtual_device: Some("/dev/video50".to_string()),
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
}
