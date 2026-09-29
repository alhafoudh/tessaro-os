//! Which of the camera's modes the mirror captures, from `camera.format`
//! and `camera.size`. Pure, so the rules are tested over tables of modes.
//!
//! `auto` is MJPEG when the camera has it, because a USB 2 camera sends
//! 1080p at full rate only compressed; YUYV otherwise. In either format the
//! largest size up to 1920x1080 that reaches 25 fps, or, when none does,
//! the fastest and then the largest. The frames are passed through as
//! captured, never decoded, so the cap is what keeps a 4K camera's frames
//! from filling the loopback's buffers and every reader's pipeline.

use protocol::CameraMode;

pub const MJPEG: &str = "mjpeg";
pub const YUYV: &str = "yuyv";

/// Below this a moving picture visibly stutters.
const SMOOTH_FPS: u32 = 25;
const MAX_WIDTH: u32 = 1920;
const MAX_HEIGHT: u32 = 1080;

/// What the settings ask for. None is `auto`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wanted {
    pub format: Option<String>,
    pub size: Option<(u32, u32)>,
}

impl Wanted {
    /// `KIOSK_CAMERA_FORMAT` and `KIOSK_CAMERA_SIZE` as the agent renders
    /// them. Empty or missing is `auto`; anything else it would not have
    /// saved is `auto` too, with the reason.
    pub fn parse(format: Option<&str>, size: Option<&str>) -> (Wanted, Vec<String>) {
        let mut wanted = Wanted::default();
        let mut problems = Vec::new();
        match format.map(str::trim).unwrap_or("") {
            "" | "auto" => {}
            value @ (MJPEG | YUYV) => wanted.format = Some(value.to_string()),
            other => problems.push(format!(
                "camera.format is '{other}', not auto, mjpeg or yuyv; capturing auto"
            )),
        }
        match size.map(str::trim).unwrap_or("") {
            "" | "auto" => {}
            value => match parse_size(value) {
                Some(size) => wanted.size = Some(size),
                None => problems.push(format!(
                    "camera.size is '{value}', not auto or WIDTHxHEIGHT; capturing auto"
                )),
            },
        }
        (wanted, problems)
    }
}

fn parse_size(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.split_once('x')?;
    let width: u32 = width.parse().ok()?;
    let height: u32 = height.parse().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

/// The mode to capture, as an index into `modes`, and why it is not what
/// the settings asked for, when it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub index: usize,
    pub fallback: Option<String>,
}

/// None when the camera has neither MJPEG nor YUYV.
pub fn choose(modes: &[CameraMode], wanted: &Wanted) -> Option<Choice> {
    let chosen = |index: usize| Choice {
        index,
        fallback: None,
    };
    let fallback = |why: String| {
        auto(modes).map(|index| Choice {
            index,
            fallback: Some(format!("{why}; capturing auto")),
        })
    };

    match (&wanted.format, wanted.size) {
        (None, None) => auto(modes).map(chosen),
        (Some(format), None) => match best(modes, format) {
            Some(index) => Some(chosen(index)),
            None => fallback(format!("the camera has no {format}")),
        },
        (None, Some(size)) => match [MJPEG, YUYV]
            .iter()
            .find_map(|format| sized(modes, format, size))
        {
            Some(index) => Some(chosen(index)),
            None => fallback(format!("the camera has no {}x{}", size.0, size.1)),
        },
        (Some(format), Some(size)) => match sized(modes, format, size) {
            Some(index) => Some(chosen(index)),
            None => fallback(format!(
                "the camera has no {}x{} in {format}",
                size.0, size.1
            )),
        },
    }
}

fn auto(modes: &[CameraMode]) -> Option<usize> {
    best(modes, MJPEG).or_else(|| best(modes, YUYV))
}

/// The best mode of `format` by the rule in the module doc.
fn best(modes: &[CameraMode], format: &str) -> Option<usize> {
    let of_format = || {
        modes
            .iter()
            .enumerate()
            .filter(move |(_, mode)| mode.format == format)
    };
    let capped = of_format().any(|(_, mode)| fits(mode));
    let pool = || of_format().filter(move |(_, mode)| !capped || fits(mode));

    let smooth = pool()
        .filter(|(_, mode)| mode.fps >= SMOOTH_FPS)
        .max_by_key(|(_, mode)| (pixels(mode), mode.fps));
    smooth
        .or_else(|| pool().max_by_key(|(_, mode)| (mode.fps, pixels(mode))))
        .map(|(index, _)| index)
}

/// `format` at exactly `size`, at the most frames a second it has.
fn sized(modes: &[CameraMode], format: &str, size: (u32, u32)) -> Option<usize> {
    modes
        .iter()
        .enumerate()
        .filter(|(_, mode)| mode.format == format && (mode.width, mode.height) == size)
        .max_by_key(|(_, mode)| mode.fps)
        .map(|(index, _)| index)
}

fn fits(mode: &CameraMode) -> bool {
    mode.width <= MAX_WIDTH && mode.height <= MAX_HEIGHT
}

fn pixels(mode: &CameraMode) -> u64 {
    u64::from(mode.width) * u64::from(mode.height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(format: &str, width: u32, height: u32, fps: u32) -> CameraMode {
        CameraMode {
            format: format.to_string(),
            width,
            height,
            fps,
        }
    }

    /// A C920-like camera: MJPEG up to 1080p30, YUYV that only reaches
    /// 30 fps at small sizes.
    fn webcam() -> Vec<CameraMode> {
        vec![
            mode(YUYV, 640, 480, 30),
            mode(YUYV, 1280, 720, 10),
            mode(YUYV, 1920, 1080, 5),
            mode(MJPEG, 640, 480, 30),
            mode(MJPEG, 1280, 720, 30),
            mode(MJPEG, 1920, 1080, 30),
        ]
    }

    fn pick(
        modes: &[CameraMode],
        format: Option<&str>,
        size: Option<&str>,
    ) -> (String, Option<String>) {
        let (wanted, problems) = Wanted::parse(format, size);
        assert!(problems.is_empty(), "{problems:?}");
        let choice = choose(modes, &wanted).expect("a mode");
        let mode = &modes[choice.index];
        (
            format!(
                "{} {}x{}@{}",
                mode.format, mode.width, mode.height, mode.fps
            ),
            choice.fallback,
        )
    }

    #[test]
    fn auto_picks_by_the_rules() {
        let cases: &[(&str, Vec<CameraMode>, &str)] = &[
            (
                "mjpeg preferred, largest smooth",
                webcam(),
                "mjpeg 1920x1080@30",
            ),
            (
                "the cap leaves out a 4K mode even when it is smooth",
                vec![mode(MJPEG, 3840, 2160, 30), mode(MJPEG, 1920, 1080, 30)],
                "mjpeg 1920x1080@30",
            ),
            (
                "a larger size below 25 fps loses to a smaller smooth one",
                vec![mode(MJPEG, 1920, 1080, 15), mode(MJPEG, 1280, 720, 25)],
                "mjpeg 1280x720@25",
            ),
            (
                "nothing smooth: the fastest, then the largest",
                vec![
                    mode(MJPEG, 1920, 1080, 10),
                    mode(MJPEG, 1280, 720, 20),
                    mode(MJPEG, 640, 480, 20),
                ],
                "mjpeg 1280x720@20",
            ),
            (
                "the same size at a higher rate wins",
                vec![mode(MJPEG, 1280, 720, 30), mode(MJPEG, 1280, 720, 60)],
                "mjpeg 1280x720@60",
            ),
            (
                "no mjpeg: yuyv by the same rule",
                vec![
                    mode(YUYV, 640, 480, 30),
                    mode(YUYV, 1280, 720, 10),
                    mode(YUYV, 800, 600, 25),
                ],
                "yuyv 800x600@25",
            ),
            (
                "only sizes over the cap: the cap is dropped",
                vec![mode(YUYV, 2592, 1944, 15), mode(YUYV, 3840, 2160, 5)],
                "yuyv 2592x1944@15",
            ),
        ];
        for (name, modes, expected) in cases {
            assert_eq!(
                pick(modes, None, None),
                (expected.to_string(), None),
                "{name}"
            );
        }
    }

    #[test]
    fn forced_format_and_size() {
        let cases: &[(Option<&str>, Option<&str>, &str)] = &[
            (Some("yuyv"), None, "yuyv 640x480@30"),
            (Some("mjpeg"), Some("1280x720"), "mjpeg 1280x720@30"),
            (Some("yuyv"), Some("1920x1080"), "yuyv 1920x1080@5"),
            // A size alone is taken in MJPEG when MJPEG has it, as auto would.
            (None, Some("640x480"), "mjpeg 640x480@30"),
            (Some("auto"), Some("auto"), "mjpeg 1920x1080@30"),
            (Some(""), Some(""), "mjpeg 1920x1080@30"),
        ];
        for (format, size, expected) in cases {
            assert_eq!(
                pick(&webcam(), *format, *size),
                (expected.to_string(), None),
                "{format:?} {size:?}"
            );
        }
    }

    #[test]
    fn a_size_only_yuyv_has_is_taken_in_yuyv() {
        let modes = vec![mode(MJPEG, 1280, 720, 30), mode(YUYV, 320, 240, 30)];
        assert_eq!(
            pick(&modes, None, Some("320x240")),
            ("yuyv 320x240@30".into(), None)
        );
    }

    #[test]
    fn what_the_camera_lacks_falls_back_to_auto_and_says_why() {
        // Modes, camera.format, camera.size, the fallback.
        type Case<'a> = (&'a [CameraMode], Option<&'a str>, Option<&'a str>, &'a str);
        let yuyv_only = vec![mode(YUYV, 640, 480, 30)];
        let cases: &[Case] = &[
            (
                &webcam(),
                Some("mjpeg"),
                Some("1600x1200"),
                "the camera has no 1600x1200 in mjpeg; capturing auto",
            ),
            (
                &webcam(),
                None,
                Some("1600x1200"),
                "the camera has no 1600x1200; capturing auto",
            ),
            (
                &yuyv_only,
                Some("mjpeg"),
                None,
                "the camera has no mjpeg; capturing auto",
            ),
        ];
        for (modes, format, size, why) in cases {
            let (_, fallback) = pick(modes, *format, *size);
            assert_eq!(fallback.as_deref(), Some(*why));
        }
        assert_eq!(
            pick(&webcam(), Some("mjpeg"), Some("1600x1200")).0,
            "mjpeg 1920x1080@30"
        );
    }

    #[test]
    fn neither_format_is_no_choice() {
        assert_eq!(choose(&[], &Wanted::default()), None);
        let wanted = Wanted {
            format: Some(MJPEG.into()),
            size: Some((640, 480)),
        };
        assert_eq!(choose(&[], &wanted), None);
    }

    #[test]
    fn unsaved_values_are_auto_with_the_reason() {
        let (wanted, problems) = Wanted::parse(Some("h264"), Some("big"));
        assert_eq!(wanted, Wanted::default());
        assert_eq!(
            problems,
            [
                "camera.format is 'h264', not auto, mjpeg or yuyv; capturing auto",
                "camera.size is 'big', not auto or WIDTHxHEIGHT; capturing auto",
            ]
        );
        assert_eq!(Wanted::parse(None, Some("0x480")).1.len(), 1);
        assert_eq!(
            Wanted::parse(Some(" yuyv "), Some("1280x720")).0,
            Wanted {
                format: Some(YUYV.into()),
                size: Some((1280, 720)),
            }
        );
    }
}
