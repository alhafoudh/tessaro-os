//! Which screen the kiosk shows on, and which screen each touchscreen
//! belongs to (screen.output).
//!
//! Weston's desktop-shell puts the fullscreen browser on the output that has
//! focus, else the first, and libinput binds every touchscreen to the first
//! output unless udev's `WL_OUTPUT` names another. With several screens that
//! is whichever connector the driver lists first. So the agent picks one:
//! the connector screen.output names when it is connected, else the biggest
//! screen with a touchscreen on it, else the biggest, ties going to the
//! first by name so a pick never flips between boots.
//!
//! The pick reaches Weston twice: `output.env` for tessaro-weston-config,
//! which writes it as `[shell] kiosk-output=` (a Tessaro Weston patch), and a
//! udev rule that gives each touchscreen its screen's `WL_OUTPUT`.
//!
//! Nothing ties a USB touch controller to an HDMI connector, so a touchscreen
//! is matched to a screen the way mutter's `meta-input-mapper.c` does it, the
//! first rule that hits winning: a touchscreen not on USB or Bluetooth is the
//! built-in panel's; its size, from its axes, is a screen's; the screen's
//! name or maker is in its name; it is the one touchscreen and that the one
//! screen. One nothing matches follows the kiosk.

use std::fs;
use std::os::fd::AsRawFd;
use std::path::Path;

use protocol::{Connector, OutputPick, OutputReason, TouchBinding, TouchMatch};

use crate::log::Log;
use crate::paths::Paths;
use crate::store;

/// `linux/input.h`: the bus types that are a cable, not the device itself.
const BUS_USB: u16 = 0x03;
const BUS_BLUETOOTH: u16 = 0x05;

/// Connector types that are a panel built into the device.
const BUILT_IN: &[&str] = &["DSI-", "eDP-", "LVDS-", "DPI-"];

/// How far a touchscreen's size may be from a screen's and still be its, in
/// each direction: a tenth, as mutter allows.
const SIZE_SLACK: f64 = 0.1;

/// A touchscreen the compositor uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touchscreen {
    /// What the udev rule matches it by: its `ID_PATH`, stable across a
    /// replug into the same port, else its `eventN` node.
    pub id: Match,
    /// The kernel's name for it: `ILITEK ILITEK-TP`.
    pub name: String,
    pub bus: u16,
    /// Its width and height in millimetres, from its axes, when it says.
    pub size_mm: Option<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Match {
    Path(String),
    Node(String),
}

impl Match {
    /// One word for output.env and weston.ini's comment: no spaces.
    fn token(&self) -> &str {
        match self {
            Match::Path(path) => path,
            Match::Node(node) => node,
        }
    }

    fn rule(&self) -> String {
        match self {
            Match::Path(path) => format!("ENV{{ID_PATH}}==\"{path}\""),
            Match::Node(node) => format!("KERNEL==\"{node}\""),
        }
    }
}

/// The touchscreens libinput opens: `ID_INPUT_TOUCHSCREEN` in udev's
/// database, and not ignored (screen.input.touch, a scanner).
pub fn touchscreens(input: &Path, udev_data: &Path, dev_input: &Path) -> Vec<Touchscreen> {
    let Ok(entries) = fs::read_dir(input) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("input"))
        })
        .map(|entry| entry.path())
        .collect();
    dirs.sort();

    let mut found = Vec::new();
    for dir in dirs {
        let Some(node) = event_node(&dir) else {
            continue;
        };
        let Ok(number) = fs::read_to_string(dir.join(&node).join("dev")) else {
            continue;
        };
        let properties =
            fs::read_to_string(udev_data.join(format!("c{}", number.trim()))).unwrap_or_default();
        let property = |key: &str| {
            properties.lines().find_map(|line| {
                let (k, v) = line.strip_prefix("E:")?.split_once('=')?;
                (k == key).then(|| v.to_string())
            })
        };
        if property("ID_INPUT_TOUCHSCREEN").as_deref() != Some("1")
            || property("LIBINPUT_IGNORE_DEVICE").as_deref() == Some("1")
        {
            continue;
        }
        let id = match property("ID_PATH").filter(|path| is_token(path)) {
            Some(path) => Match::Path(path),
            None => Match::Node(node.clone()),
        };
        let bus = u16::from_str_radix(read(&dir.join("id/bustype")).as_str(), 16).unwrap_or(0);
        found.push(Touchscreen {
            id,
            name: read(&dir.join("name")),
            bus,
            size_mm: size_mm(&dev_input.join(&node)),
        });
    }
    found
}

/// `inputN/eventM`: the node libinput opens.
fn event_node(dir: &Path) -> Option<String> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.starts_with("event").then_some(name)
        })
}

/// What a udev rule and weston.ini's comment may carry unquoted.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '.' | '-' | '_' | '+'))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// `struct input_absinfo`.
#[repr(C)]
#[derive(Default)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// `EVIOCGABS(abs)`: `_IOR('E', 0x40 + abs, struct input_absinfo)`.
const fn eviocgabs(abs: u32) -> u32 {
    (2 << 30)
        | ((std::mem::size_of::<AbsInfo>() as u32) << 16)
        | ((b'E' as u32) << 8)
        | (0x40 + abs)
}

const ABS_X: u32 = 0x00;
const ABS_Y: u32 = 0x01;

/// The touch area in millimetres: each axis's range over its resolution in
/// units per millimetre. Many panels report no resolution; then `None`.
fn size_mm(node: &Path) -> Option<(u32, u32)> {
    let file = fs::File::open(node).ok()?;
    let axis = |abs: u32| -> Option<u32> {
        let mut info = AbsInfo::default();
        // SAFETY: EVIOCGABS fills one input_absinfo, which `info` is.
        let result = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                eviocgabs(abs) as _,
                &mut info as *mut AbsInfo,
            )
        };
        if result < 0 || info.resolution <= 0 || info.maximum <= info.minimum {
            return None;
        }
        Some(((info.maximum - info.minimum) / info.resolution) as u32)
    };
    Some((axis(ABS_X)?, axis(ABS_Y)?)).filter(|(w, h)| *w > 0 && *h > 0)
}

/// The kiosk's screen and every touchscreen's, for screen.output = `setting`
/// (`auto`, or a connector). `None` with no screen connected.
pub fn pick(
    connectors: &[Connector],
    touches: &[Touchscreen],
    setting: &str,
) -> Option<OutputPick> {
    if connectors.is_empty() {
        return None;
    }
    let bound: Vec<(String, Option<(String, TouchMatch)>)> = touches
        .iter()
        .map(|touch| {
            (
                touch.name.clone(),
                match_touch(touch, connectors, touches.len()),
            )
        })
        .collect();

    let set = (setting != "auto" && !setting.is_empty()).then_some(setting);
    let (name, reason) =
        if let Some(name) = set.filter(|name| connectors.iter().any(|c| c.name == *name)) {
            (name.to_string(), OutputReason::Set)
        } else if connectors.len() == 1 {
            (connectors[0].name.clone(), OutputReason::Only)
        } else {
            let touched: Vec<&Connector> = connectors
                .iter()
                .filter(|c| {
                    bound
                        .iter()
                        .any(|(_, b)| b.as_ref().is_some_and(|(name, _)| *name == c.name))
                })
                .collect();
            if touched.is_empty() {
                let all: Vec<&Connector> = connectors.iter().collect();
                (biggest(&all).name.clone(), OutputReason::Biggest)
            } else {
                (biggest(&touched).name.clone(), OutputReason::Touch)
            }
        };

    let touch = bound
        .into_iter()
        .map(|(touch_name, matched)| {
            let (connector, how) = matched.unwrap_or_else(|| (name.clone(), TouchMatch::Kiosk));
            TouchBinding {
                name: touch_name,
                connector,
                how,
            }
        })
        .collect();
    Some(OutputPick {
        missing: set
            .filter(|_| reason != OutputReason::Set)
            .map(str::to_string),
        name,
        reason,
        touch,
    })
}

/// The biggest of `candidates`, the first by name on a tie. By the EDID's
/// size when every one of them says it, else by the pixels of the preferred
/// mode: a projector reports no size, and centimetres cannot be weighed
/// against pixels.
fn biggest<'a>(candidates: &[&'a Connector]) -> &'a Connector {
    let physical = candidates.iter().all(|c| physical_cm(c).is_some());
    let size = |c: &Connector| -> u64 {
        if physical {
            let (w, h) = physical_cm(c).unwrap_or_default();
            (w as u64).pow(2) + (h as u64).pow(2)
        } else {
            c.modes
                .first()
                .and_then(|mode| protocol::keys::parse_mode(mode))
                .map_or(0, |(w, h)| w as u64 * h as u64)
        }
    };
    let mut best = candidates[0];
    for candidate in &candidates[1..] {
        let (a, b) = (size(candidate), size(best));
        if a > b || (a == b && candidate.name < best.name) {
            best = candidate;
        }
    }
    best
}

fn physical_cm(connector: &Connector) -> Option<(u8, u8)> {
    let display = connector.display.as_ref()?;
    Some((display.width_cm?, display.height_cm?))
}

/// The screen `touch` belongs to, and how that was found; `None` when no
/// rule matches.
fn match_touch(
    touch: &Touchscreen,
    connectors: &[Connector],
    touches: usize,
) -> Option<(String, TouchMatch)> {
    if touch.bus != BUS_USB && touch.bus != BUS_BLUETOOTH {
        if let Some(panel) = connectors
            .iter()
            .find(|c| BUILT_IN.iter().any(|kind| c.name.starts_with(kind)))
        {
            return Some((panel.name.clone(), TouchMatch::BuiltIn));
        }
    }

    if let Some((touch_w, touch_h)) = touch.size_mm {
        let near = |a: u32, b: u32| (a as f64 - b as f64).abs() <= b as f64 * SIZE_SLACK;
        if let Some(screen) = connectors.iter().find(|c| {
            physical_cm(c).is_some_and(|(w, h)| {
                let (w, h) = (w as u32 * 10, h as u32 * 10);
                (near(touch_w, w) && near(touch_h, h)) || (near(touch_w, h) && near(touch_h, w))
            })
        }) {
            return Some((screen.name.clone(), TouchMatch::Size));
        }
    }

    let touch_name = touch.name.to_lowercase();
    if let Some(screen) = connectors.iter().find(|c| {
        c.display.as_ref().is_some_and(|display| {
            [display.model.as_deref(), display.vendor.as_deref()]
                .into_iter()
                .flatten()
                .map(|name| name.trim().to_lowercase())
                .any(|name| name.len() >= 3 && touch_name.contains(&name))
                || touch_name
                    .split(|ch: char| !ch.is_ascii_alphanumeric())
                    .any(|word| word.eq_ignore_ascii_case(&display.vendor_id))
        })
    }) {
        return Some((screen.name.clone(), TouchMatch::Name));
    }

    (touches == 1 && connectors.len() == 1).then(|| (connectors[0].name.clone(), TouchMatch::Only))
}

/// output.env, for tessaro-weston-config: the kiosk's connector, and each
/// touchscreen's as `id=connector` words. Empty values with no screen.
pub fn env_file(pick: Option<&OutputPick>, touches: &[Touchscreen]) -> String {
    let active = pick.map_or("", |pick| pick.name.as_str());
    let touch: Vec<String> = pick
        .map(|pick| {
            touches
                .iter()
                .zip(&pick.touch)
                .map(|(touch, binding)| format!("{}={}", touch.id.token(), binding.connector))
                .collect()
        })
        .unwrap_or_default();
    format!(
        "# Rendered by tessaro-agent: the screen the kiosk shows on (screen.output).\n\
         KIOSK_OUTPUT_ACTIVE={active}\n\
         KIOSK_OUTPUT_TOUCH={}\n",
        touch.join(" ")
    )
}

/// The udev rule binding each touchscreen to its screen, or `None` without
/// one. Weston reads `WL_OUTPUT` only when it opens a device.
pub fn touch_rules(pick: Option<&OutputPick>, touches: &[Touchscreen]) -> Option<String> {
    let pick = pick?;
    let lines: Vec<String> = touches
        .iter()
        .zip(&pick.touch)
        .map(|(touch, binding)| {
            format!(
                "SUBSYSTEM==\"input\", KERNEL==\"event*\", ENV{{ID_INPUT_TOUCHSCREEN}}==\"1\", {}, \
                 ENV{{WL_OUTPUT}}=\"{}\"\n",
                touch.id.rule(),
                binding.connector
            )
        })
        .collect();
    (!lines.is_empty()).then(|| {
        format!(
            "# Rendered by tessaro-agent from screen.output: the screen each touchscreen's touches land on.\n{}",
            lines.concat()
        )
    })
}

/// What screen.output and screen.input.touch say now, read the agent's way.
pub struct Setting {
    /// screen.output: `auto` or a connector.
    pub output: String,
    /// screen.input.touch is on: the touchscreens count.
    pub touch: bool,
}

/// The pick for what is plugged in now. Blocking: sysfs and an ioctl.
pub fn current(paths: &Paths, setting: &Setting) -> (Option<OutputPick>, Vec<Touchscreen>) {
    let connectors = crate::display::connectors(&paths.drm);
    let touches = if setting.touch {
        touchscreens(&paths.input, &paths.udev_data, &paths.dev_input)
    } else {
        Vec::new()
    };
    (pick(&connectors, &touches, &setting.output), touches)
}

/// Pick, and write output.env and the touch rule when they change; a
/// changed rule is applied to the devices already present. Whether anything
/// changed. A rule that cannot be written is only logged: the kiosk still
/// goes to its screen, and only touch is left on the first. Blocking.
pub fn apply(paths: &Paths, setting: &Setting, log: &Log) -> Result<bool, String> {
    let (pick, touches) = current(paths, setting);
    let env_path = paths.output_env();
    let env_changed = store::replace_if_changed(
        &env_path,
        env_file(pick.as_ref(), &touches).as_bytes(),
        0o644,
    )
    .map_err(|err| format!("{}: {err}", env_path.display()))?;

    let rules = &paths.touch_rules;
    let written = match touch_rules(pick.as_ref(), &touches) {
        Some(body) => store::replace_if_changed(rules, body.as_bytes(), 0o644),
        None => match fs::remove_file(rules) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err),
        },
    };
    let rules_changed = written.unwrap_or_else(|err| {
        log.info(format!("{}: {err}", rules.display()));
        false
    });
    if rules_changed && !paths.udevadm.as_os_str().is_empty() {
        if let Err(err) = crate::render::apply_input_rules(&paths.udevadm) {
            log.info(format!("the touchscreen rule: {err}"));
        }
    }
    Ok(env_changed || rules_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::DisplayIdentity;

    fn screen(name: &str, mode: &str, cm: Option<(u8, u8)>) -> Connector {
        screen_named(name, mode, cm, None, "XXX")
    }

    fn screen_named(
        name: &str,
        mode: &str,
        cm: Option<(u8, u8)>,
        model: Option<&str>,
        vendor_id: &str,
    ) -> Connector {
        Connector {
            name: name.to_string(),
            modes: vec![mode.to_string()],
            display: Some(DisplayIdentity {
                vendor_id: vendor_id.to_string(),
                vendor: None,
                product_code: 1,
                model: model.map(str::to_string),
                serial: None,
                year: None,
                week: None,
                width_cm: cm.map(|(w, _)| w),
                height_cm: cm.map(|(_, h)| h),
                hdmi_address: None,
            }),
        }
    }

    fn touch(name: &str, bus: u16, size_mm: Option<(u32, u32)>) -> Touchscreen {
        Touchscreen {
            id: Match::Path(format!("path-{}", name.replace(' ', "_"))),
            name: name.to_string(),
            bus,
            size_mm,
        }
    }

    #[test]
    fn nothing_connected_is_no_pick() {
        assert_eq!(pick(&[], &[], "auto"), None);
    }

    #[test]
    fn one_screen_is_the_only_one() {
        let pick = pick(&[screen("HDMI-A-1", "1920x1080", None)], &[], "auto").unwrap();
        assert_eq!(
            (pick.name.as_str(), pick.reason),
            ("HDMI-A-1", OutputReason::Only)
        );
    }

    #[test]
    fn the_biggest_by_edid_size() {
        let screens = [
            screen("HDMI-A-1", "3840x2160", Some((60, 34))),
            screen("HDMI-A-2", "1920x1080", Some((120, 68))),
        ];
        let pick = pick(&screens, &[], "auto").unwrap();
        assert_eq!(
            (pick.name.as_str(), pick.reason),
            ("HDMI-A-2", OutputReason::Biggest)
        );
    }

    #[test]
    fn by_pixels_when_one_has_no_size() {
        let screens = [
            screen("HDMI-A-1", "3840x2160", None),
            screen("HDMI-A-2", "1920x1080", Some((120, 68))),
        ];
        assert_eq!(pick(&screens, &[], "auto").unwrap().name, "HDMI-A-1");
    }

    #[test]
    fn a_tie_goes_to_the_first_by_name() {
        let screens = [
            screen("HDMI-A-2", "1920x1080", Some((60, 34))),
            screen("HDMI-A-1", "1920x1080", Some((60, 34))),
        ];
        assert_eq!(pick(&screens, &[], "auto").unwrap().name, "HDMI-A-1");
    }

    #[test]
    fn a_screen_with_a_touchscreen_beats_a_bigger_one() {
        let screens = [
            screen("DSI-1", "800x480", Some((15, 9))),
            screen("HDMI-A-1", "3840x2160", Some((120, 68))),
        ];
        // I2C, the Pi's ribbon touch screen.
        let touches = [touch("raspberrypi-ts", 0x18, None)];
        let pick = pick(&screens, &touches, "auto").unwrap();
        assert_eq!(
            (pick.name.as_str(), pick.reason),
            ("DSI-1", OutputReason::Touch)
        );
        assert_eq!(pick.touch[0].connector, "DSI-1");
        assert_eq!(pick.touch[0].how, TouchMatch::BuiltIn);
    }

    #[test]
    fn a_usb_touchscreen_by_its_size() {
        let screens = [
            screen("HDMI-A-1", "1920x1080", Some((52, 29))),
            screen("HDMI-A-2", "3840x2160", Some((120, 68))),
        ];
        let touches = [touch("ILITEK ILITEK-TP", BUS_USB, Some((515, 295)))];
        let pick = pick(&screens, &touches, "auto").unwrap();
        assert_eq!(
            (pick.name.as_str(), pick.reason),
            ("HDMI-A-1", OutputReason::Touch)
        );
        assert_eq!(pick.touch[0].how, TouchMatch::Size);
    }

    #[test]
    fn a_usb_touchscreen_by_the_screens_name() {
        let screens = [
            screen_named("HDMI-A-1", "1920x1080", None, Some("ET2202L"), "ELO"),
            screen("HDMI-A-2", "3840x2160", None),
        ];
        let touches = [touch("Elo TouchSystems 2700 IntelliTouch", BUS_USB, None)];
        let pick = pick(&screens, &touches, "auto").unwrap();
        assert_eq!(pick.name, "HDMI-A-1");
        assert_eq!(pick.touch[0].how, TouchMatch::Name);
    }

    #[test]
    fn an_unmatched_touchscreen_follows_the_kiosk() {
        let screens = [
            screen("HDMI-A-1", "1920x1080", None),
            screen("HDMI-A-2", "3840x2160", None),
        ];
        let touches = [touch("Generic Touch", BUS_USB, None)];
        let pick = pick(&screens, &touches, "auto").unwrap();
        assert_eq!(
            (pick.name.as_str(), pick.reason),
            ("HDMI-A-2", OutputReason::Biggest)
        );
        assert_eq!(pick.touch[0].connector, "HDMI-A-2");
        assert_eq!(pick.touch[0].how, TouchMatch::Kiosk);
    }

    #[test]
    fn one_touchscreen_and_one_screen() {
        let screens = [screen("HDMI-A-1", "1920x1080", None)];
        let touches = [touch("Generic Touch", BUS_USB, None)];
        let pick = pick(&screens, &touches, "auto").unwrap();
        assert_eq!(pick.touch[0].how, TouchMatch::Only);
    }

    #[test]
    fn the_setting_wins_and_falls_back_when_unplugged() {
        let screens = [
            screen("HDMI-A-1", "1920x1080", None),
            screen("HDMI-A-2", "3840x2160", None),
        ];
        let set = pick(&screens, &[], "HDMI-A-1").unwrap();
        assert_eq!(
            (set.name.as_str(), set.reason, set.missing),
            ("HDMI-A-1", OutputReason::Set, None)
        );

        let gone = pick(&screens, &[], "DP-1").unwrap();
        assert_eq!(gone.name, "HDMI-A-2");
        assert_eq!(gone.missing.as_deref(), Some("DP-1"));
    }

    #[test]
    fn env_file_and_rule() {
        let screens = [
            screen("DSI-1", "800x480", None),
            screen("HDMI-A-1", "1920x1080", None),
        ];
        let touches = [touch("raspberrypi-ts", 0x18, None)];
        let picked = pick(&screens, &touches, "auto");
        assert_eq!(
            env_file(picked.as_ref(), &touches),
            "# Rendered by tessaro-agent: the screen the kiosk shows on (screen.output).\n\
             KIOSK_OUTPUT_ACTIVE=DSI-1\n\
             KIOSK_OUTPUT_TOUCH=path-raspberrypi-ts=DSI-1\n"
        );
        let rules = touch_rules(picked.as_ref(), &touches).unwrap();
        assert!(rules.contains(
            "ENV{ID_INPUT_TOUCHSCREEN}==\"1\", ENV{ID_PATH}==\"path-raspberrypi-ts\", ENV{WL_OUTPUT}=\"DSI-1\""
        ));
        assert_eq!(touch_rules(picked.as_ref(), &[]), None);
        assert_eq!(
            env_file(None, &[]),
            "# Rendered by tessaro-agent: the screen the kiosk shows on (screen.output).\n\
             KIOSK_OUTPUT_ACTIVE=\nKIOSK_OUTPUT_TOUCH=\n"
        );
    }

    #[test]
    fn touchscreens_from_sysfs_and_the_udev_database() {
        let dir = tempfile::tempdir().unwrap();
        let (input, data, dev) = (
            dir.path().join("input"),
            dir.path().join("data"),
            dir.path().join("dev"),
        );
        let device = |n: u32, name: &str, bus: &str, props: &str| {
            let base = input.join(format!("input{n}"));
            fs::create_dir_all(base.join(format!("event{n}"))).unwrap();
            fs::create_dir_all(base.join("id")).unwrap();
            fs::write(base.join("name"), format!("{name}\n")).unwrap();
            fs::write(base.join("id/bustype"), format!("{bus}\n")).unwrap();
            fs::write(
                base.join(format!("event{n}/dev")),
                format!("13:{}\n", 64 + n),
            )
            .unwrap();
            fs::create_dir_all(&data).unwrap();
            fs::write(data.join(format!("c13:{}", 64 + n)), props).unwrap();
        };
        device(
            1,
            "Touch One",
            "0003",
            "E:ID_INPUT=1\nE:ID_INPUT_TOUCHSCREEN=1\nE:ID_PATH=pci-0000:00:14.0-usb-0:1:1.0\n",
        );
        device(2, "A Keyboard", "0003", "E:ID_INPUT_KEYBOARD=1\n");
        device(
            3,
            "Ignored Touch",
            "0003",
            "E:ID_INPUT_TOUCHSCREEN=1\nE:LIBINPUT_IGNORE_DEVICE=1\n",
        );
        device(4, "No Path", "0018", "E:ID_INPUT_TOUCHSCREEN=1\n");

        let found = touchscreens(&input, &data, &dev);

        assert_eq!(
            found,
            vec![
                Touchscreen {
                    id: Match::Path("pci-0000:00:14.0-usb-0:1:1.0".to_string()),
                    name: "Touch One".to_string(),
                    bus: BUS_USB,
                    size_mm: None,
                },
                Touchscreen {
                    id: Match::Node("event4".to_string()),
                    name: "No Path".to_string(),
                    bus: 0x18,
                    size_mm: None,
                },
            ]
        );
    }
}
