//! Cameras: `tessaro-ctl camera` and the Camera page, presence detection
//! included.

use protocol::keys;
use protocol::presence::{Calibrated, Demographics, Face, Gender, Genders, PresenceStatus};
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

/// `camera list`: a block per camera - what it is called, its node and each
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
            .text(format!(" {}  ", list.size))
            .add(Tone::Label, "mirrors")
            .text(format!(" {}", list.mirrors)),
    );
    lines.push(
        Line::of(Tone::Muted, "change them with")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl camera format auto|mjpeg|yuyv")
            .add(Tone::Muted, ",")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl camera size auto|WIDTHxHEIGHT")
            .text(" ")
            .add(Tone::Muted, "and")
            .text(" ")
            .add(
                Tone::Cmd,
                format!("tessaro-ctl camera mirrors 1-{}", keys::CAMERA_MIRRORS_MAX),
            ),
    );
    lines
}

fn one(camera: &CameraInfo) -> Vec<Line> {
    let indent = || Line::plain("    ");
    let mut lines = vec![Line::of(Tone::Heading, &camera.name)
        .text(" ")
        .add(Tone::Muted, format!("({})", camera.bus))];
    lines.push(
        indent()
            .pad(Tone::Label, "device", 9)
            .text(format!(" /dev/{}", camera.device)),
    );
    if camera.mirrors.is_empty() {
        lines.push(
            indent()
                .pad(Tone::Label, "mirror", 9)
                .text(" ")
                .add(Tone::Muted, "(no virtual camera)"),
        );
    }
    let width = camera
        .mirrors
        .iter()
        .map(|mirror| mirror.device.chars().count())
        .max()
        .unwrap_or(0);
    for mirror in &camera.mirrors {
        lines.push(
            indent()
                .pad(Tone::Label, "mirror", 9)
                .text(" ")
                .pad(Tone::Ok, &mirror.device, width)
                .text(format!("  {}", mirror.name)),
        );
    }
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

/// `camera presence`: whether anyone is there and who, how the detection
/// runs, the last event, then the saved distances and how to change them.
pub fn presence(status: &PresenceStatus) -> Vec<Line> {
    if !status.enabled {
        return vec![Line::of(Tone::Muted, "presence detection is off;")
            .text(" ")
            .add(Tone::Muted, "switch it on with")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl camera presence on")];
    }
    let mut state = Line::new().pad(Tone::Label, "presence", 9).text(" ");
    state = if !status.running {
        state.add(Tone::Warn, "not running")
    } else if status.present && status.near {
        state.add(Tone::Ok, "someone is there, near")
    } else if status.present {
        state.add(Tone::Ok, "someone is there")
    } else {
        state.add(Tone::Muted, "nobody is there")
    };
    let mut lines = vec![state];
    let faces = status
        .frame
        .as_ref()
        .map_or(&[][..], |frame| &frame.faces[..]);
    for face in faces {
        lines.push(face_line(face));
    }
    let mut camera = Line::new()
        .pad(Tone::Label, "camera", 9)
        .text(format!(
            " {}  ",
            status.camera.as_deref().unwrap_or("(none)")
        ))
        .add(Tone::Label, "model")
        .text(format!(" {}", status.model));
    if let Some(fps) = status.fps {
        camera = camera.add(Tone::Muted, format!("  {fps:.1} fps"));
    }
    if let Some(ms) = status.inference_ms {
        camera = camera.add(Tone::Muted, format!("  {ms:.0} ms a frame"));
    }
    lines.push(camera);
    if status.demographics {
        let genders = Genders::of(faces);
        let mut estimate = Line::new()
            .pad(Tone::Label, "estimate", 9)
            .text(" age and gender  ")
            .add(
                Tone::Plain,
                format!(
                    "{} male, {} female, {} unknown",
                    genders.male, genders.female, genders.unknown
                ),
            );
        if let Some(ms) = status.classify_ms {
            estimate = estimate.add(Tone::Muted, format!("  {ms:.0} ms a look"));
        }
        lines.push(estimate);
    }
    if let Some(err) = &status.error {
        lines.push(Line::plain("          ").add(Tone::Bad, err));
    }
    if let Some(last) = &status.last {
        lines.push(
            Line::new()
                .pad(Tone::Label, "last", 9)
                .text(format!(" {} at {}", last.event, last.at.local)),
        );
    }
    let near = match status.near_m {
        Some(meters) => format!(" {meters:.1} m  "),
        None => " off  ".to_string(),
    };
    lines.push(Line::new());
    lines.push(
        Line::new()
            .pad(Tone::Label, "saved", 9)
            .text(" ")
            .add(Tone::Label, "near")
            .text(near)
            .add(Tone::Label, "fov")
            .text(format!(" {:.0}°", status.fov)),
    );
    lines.push(
        Line::of(Tone::Muted, "change them with")
            .text(" ")
            .add(
                Tone::Cmd,
                "tessaro-ctl config set camera.presence.near=METERS",
            )
            .add(Tone::Muted, ",")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl camera calibrate --distance 1"),
    );
    lines
}

/// One face: `face     #3 1.2 m near facing (0.93)`, then `female, about
/// 34` once its age and gender settled.
fn face_line(face: &Face) -> Line {
    let mut line = Line::plain("    ")
        .pad(Tone::Label, "face", 5)
        .text(format!(" #{} {:.1} m", face.id, face.distance));
    if face.near {
        line = line.text(" ").add(Tone::Ok, "near");
    }
    line = line
        .text(" ")
        .add(
            Tone::Plain,
            if face.facing { "facing" } else { "turned away" },
        )
        .add(Tone::Muted, format!(" ({:.2})", face.score));
    if let Some(estimate) = &face.demographics {
        line = line.text("  ").add(Tone::Plain, self::estimate(estimate));
    }
    line
}

/// A face's settled age and gender: `female, about 34`, `gender unknown,
/// about 52`. The camera panels label their face boxes with it too.
pub fn estimate(estimate: &Demographics) -> String {
    let gender = match estimate.gender {
        Gender::Unknown => "gender unknown",
        known => known.as_str(),
    };
    format!("{gender}, about {}", estimate.age)
}

/// What `camera calibrate` saved.
pub fn calibrated(calibrated: &Calibrated) -> Line {
    Line::of(
        Tone::Ok,
        format!("camera.presence.fov is {:.0}°", calibrated.fov),
    )
    .add(
        Tone::Muted,
        format!(
            " (a face {:.3} of the frame wide at {:.1} m)",
            calibrated.width, calibrated.distance
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::CameraMirror;

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
            mirrors: Vec::new(),
            vision: None,
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

    #[test]
    fn every_mirror_gets_a_line_with_its_node_aligned() {
        let mirror = |device: &str, n: u32| CameraMirror {
            name: format!("Cam Mirror {n}"),
            device: device.into(),
        };
        let camera = CameraInfo {
            name: "Cam".into(),
            device: "video0".into(),
            bus: "usb-1".into(),
            mirrors: vec![mirror("/dev/video9", 1), mirror("/dev/video10", 2)],
            vision: None,
            mode: None,
            fallback: None,
            error: None,
            modes: Vec::new(),
        };
        let text: Vec<String> = one(&camera).iter().map(Line::to_string).collect();
        assert!(text.contains(&"    mirror    /dev/video9   Cam Mirror 1".to_string()));
        assert!(text.contains(&"    mirror    /dev/video10  Cam Mirror 2".to_string()));
    }

    #[test]
    fn a_camera_without_mirrors_says_so() {
        let camera = CameraInfo {
            name: "Cam".into(),
            device: "video0".into(),
            bus: "usb-1".into(),
            mirrors: Vec::new(),
            vision: None,
            mode: None,
            fallback: None,
            error: Some("broken".into()),
            modes: Vec::new(),
        };
        let text: Vec<String> = one(&camera).iter().map(Line::to_string).collect();
        assert!(text.iter().any(|l| l.ends_with("(no virtual camera)")));
    }
}
