//! Cameras: `tessaro-ctl camera` and the Camera page.

use protocol::{CameraInfo, CameraList, CameraMode};

use crate::text::{Line, Tone};

/// `mjpeg 1280x720 @ 30 fps`.
pub fn mode(mode: &CameraMode) -> String {
    format!(
        "{} {}x{} @ {} fps",
        mode.format, mode.width, mode.height, mode.fps
    )
}

/// What a device without a camera shows, and how one appears.
pub fn none() -> Line {
    Line::of(Tone::Warn, "no cameras;")
        .text(" ")
        .add(Tone::Muted, "plug in a USB camera and it shows here")
}

/// `camera list`: a block per camera - what it is called, its node and the
/// virtual camera readers open, what its mirror captures and why that is
/// not what the settings say, every mode it has - then the saved settings.
pub fn list(list: &CameraList) -> Vec<Line> {
    let mut lines = Vec::new();
    if list.cameras.is_empty() {
        lines.push(none());
    }
    for camera in &list.cameras {
        lines.extend(one(camera));
    }
    lines.push(Line::new());
    lines.push(
        Line::new()
            .pad(Tone::Label, "saved", 9)
            .text(" ")
            .add(Tone::Label, "format")
            .text(format!(" {}  ", list.format))
            .add(Tone::Label, "size")
            .text(format!(" {}", list.size)),
    );
    lines.push(
        Line::of(Tone::Muted, "change them with")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl camera format auto|mjpeg|yuyv")
            .text(" ")
            .add(Tone::Muted, "and")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl camera size auto|WIDTHxHEIGHT"),
    );
    lines
}

fn one(camera: &CameraInfo) -> Vec<Line> {
    let indent = || Line::plain("    ");
    let mut lines = vec![Line::of(Tone::Heading, &camera.name)
        .text(" ")
        .add(Tone::Muted, format!("({})", camera.bus))];
    let virtual_device = match &camera.virtual_device {
        Some(node) => Line::of(Tone::Ok, node),
        None => Line::of(Tone::Muted, "(no virtual camera)"),
    };
    lines.push(
        indent()
            .pad(Tone::Label, "device", 9)
            .text(format!(" /dev/{} ", camera.device))
            .add(Tone::Muted, "->")
            .text(" ")
            .join(virtual_device),
    );
    if let Some(captures) = &camera.mode {
        lines.push(
            indent()
                .pad(Tone::Label, "captures", 9)
                .text(format!(" {}", mode(captures))),
        );
    }
    if let Some(why) = &camera.fallback {
        lines.push(indent().add(Tone::Warn, why));
    }
    if let Some(err) = &camera.error {
        lines.push(indent().add(Tone::Bad, err));
    }
    let mut formats: Vec<&str> = Vec::new();
    for mode in &camera.modes {
        if !formats.contains(&mode.format.as_str()) {
            formats.push(&mode.format);
        }
    }
    for format in formats {
        let sizes: Vec<String> = camera
            .modes
            .iter()
            .filter(|mode| mode.format == format)
            .map(|mode| format!("{}x{}@{}", mode.width, mode.height, mode.fps))
            .collect();
        lines.push(
            indent()
                .pad(Tone::Label, format, 9)
                .text(" ")
                .add(Tone::Muted, sizes.join(" ")),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(format: &str, width: u32, height: u32, fps: u32) -> CameraMode {
        CameraMode {
            format: format.into(),
            width,
            height,
            fps,
        }
    }

    #[test]
    fn a_mode_reads_as_format_size_and_rate() {
        assert_eq!(mode(&at("mjpeg", 1280, 720, 30)), "mjpeg 1280x720 @ 30 fps");
    }

    #[test]
    fn modes_are_grouped_by_format_in_the_order_the_camera_gives() {
        let camera = CameraInfo {
            name: "Cam".into(),
            device: "video0".into(),
            bus: "usb-1".into(),
            virtual_device: Some("/dev/video50".into()),
            mode: Some(at("mjpeg", 1280, 720, 30)),
            fallback: None,
            error: None,
            modes: vec![
                at("mjpeg", 1920, 1080, 30),
                at("yuyv", 640, 480, 30),
                at("mjpeg", 1280, 720, 30),
            ],
        };
        let text: Vec<String> = one(&camera).iter().map(Line::to_string).collect();
        assert!(text.iter().any(|l| l.ends_with("1920x1080@30 1280x720@30")));
        assert!(text.last().unwrap().ends_with("640x480@30"));
    }
}
