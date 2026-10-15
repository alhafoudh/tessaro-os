//! Keeping Weston's generated config true to what is plugged in.
//!
//! `tessaro-weston-config` decides a device's `[output]` and `[input-method]`
//! sections once, as `ExecStartPre=` of `weston.service`, from the screens
//! and keyboards attached at that moment. A screen plugged in after boot gets
//! no scale and no `screen.resolution`, a 4K panel swapped for a 1080p one
//! keeps a scale of 2, and `screen.osk=auto` never notices a keyboard coming
//! or going. So the agent watches the same sysfs the generator reads and,
//! once a change has settled, runs the generator again into a scratch file
//! and compares. Weston is restarted only when the answer differs from the
//! config it is running - the generator stays the one place these decisions
//! are made, and a monitor that merely went to sleep and came back restarts
//! nothing.
//!
//! What counts as different:
//!
//! * a connector connected now that was not when Weston started - the
//!   generator records that set in a comment line. This is also what brings a
//!   device booted with no screen onto a screen plugged in later, whatever
//!   the scale and resolution settings are.
//! * an `[output]` section the generator would now write differently: a
//!   different panel on the same connector.
//! * the `[input-method]` section: the on-screen keyboard's verdict changed.
//! * `[shell] kiosk-output`: the kiosk's screen (`crate::output`), which the
//!   agent picks again before the generator runs.
//! * a touchscreen whose screen is not the one Weston bound it to, or that
//!   it has not seen: Weston reads `WL_OUTPUT` only when it opens a device.
//!   The generator records the bindings in a comment line.
//!
//! A connector going away is no reason by itself. Weston copes with a head
//! disappearing on its own, and the section it had stays in the running
//! config for when the screen comes back. Only when the kiosk's screen goes
//! and another is left does the pick move, and with it the kiosk.

use std::fs;
use std::path::Path;
use std::process::Command;

/// The comment line `tessaro-weston-config` writes: every connector that was
/// connected when it ran, space separated, or `(none)`.
const MARKER: &str = "# tessaro-weston-config: connected";

/// The comment line with each touchscreen's screen, `id=connector` words
/// from output.env's `KIOSK_OUTPUT_TOUCH`.
const TOUCH: &str = "# tessaro-weston-config: touch";

/// Where the connector and input device state is read from. Cheap - every
/// file is a cached value the kernel keeps, and reading one probes nothing.
pub struct Sources<'a> {
    pub drm: &'a Path,
    pub input: &'a Path,
}

/// Everything the generator's verdict depends on, as one comparable string:
/// each connector's status and modes, and each input device's bus and name.
/// Only ever compared with itself, never parsed.
pub fn snapshot(sources: &Sources) -> String {
    let mut out = String::new();
    for dir in sorted(sources.drm, |name| {
        name.split_once('-')
            .is_some_and(|(card, _)| card.starts_with("card"))
    }) {
        out.push_str(&format!(
            "{} {} [{}]\n",
            dir.file_name().unwrap_or_default().to_string_lossy(),
            read(&dir.join("status")),
            read(&dir.join("modes")).replace('\n', " ")
        ));
    }
    for dir in sorted(sources.input, |name| name.starts_with("input")) {
        out.push_str(&format!(
            "{} {} {}\n",
            dir.file_name().unwrap_or_default().to_string_lossy(),
            read(&dir.join("id/bustype")),
            read(&dir.join("name"))
        ));
    }
    out
}

fn sorted(root: &Path, keep: impl Fn(&str) -> bool) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().is_some_and(&keep))
        .map(|entry| entry.path())
        .collect();
    found.sort();
    found
}

fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// Run the generator the way `weston.service` does, but into `out`, and
/// return what it wrote. Blocking: call it from `spawn_blocking`.
///
/// The environment overrides are the generator's own test hooks, pointed at
/// the paths this agent uses, so a host run reads the same fixtures.
pub fn generate(
    generator: &Path,
    out: &Path,
    sources: &Sources,
    override_env: &Path,
    output_env: &Path,
) -> Result<String, String> {
    let output = crate::proc::run(
        Command::new(generator)
            .env("TESSARO_WESTON_CONFIG", out)
            .env("TESSARO_DRM_ROOT", sources.drm)
            .env("TESSARO_INPUT_ROOT", sources.input)
            .env("TESSARO_OVERRIDE", override_env)
            .env("TESSARO_OUTPUT", output_env),
        None,
    )?;
    if !output.status.success() {
        return Err(format!(
            "{} failed ({}): {}",
            generator.display(),
            output.status,
            crate::proc::said(&output)
        ));
    }
    fs::read_to_string(out).map_err(|err| format!("{}: {err}", out.display()))
}

/// Why Weston should be restarted onto `candidate`, if it should. `running`
/// is the config it started with.
pub fn verdict(running: &str, candidate: &str) -> Option<String> {
    let before = Ini::parse(running);
    let after = Ini::parse(candidate);

    // No marker means a config this generator did not write; the section
    // checks below still apply.
    if let (Some(was), Some(now)) = (&before.connected, &after.connected) {
        let new: Vec<&str> = now
            .iter()
            .filter(|name| !was.contains(name))
            .map(String::as_str)
            .collect();
        if !new.is_empty() {
            return Some(format!("{} connected since Weston started", new.join(", ")));
        }
    }

    for section in after.sections.iter().filter(|s| s.header == "output") {
        let Some(name) = section.value("name") else {
            continue;
        };
        if before.output(name) != Some(section) {
            return Some(format!(
                "{name} now wants [output] {}",
                section.lines.join(" ")
            ));
        }
    }

    if before.section("input-method") != after.section("input-method") {
        return Some(match after.section("input-method") {
            Some(_) => "a keyboard is attached, so the on-screen keyboard goes".to_string(),
            None => {
                "no keyboard is attached any more, so the on-screen keyboard comes back".to_string()
            }
        });
    }

    // With no screen at all there is no pick, and nothing to move to.
    let kiosk = |ini: &Ini| {
        ini.section("shell")
            .and_then(|shell| shell.value("kiosk-output"))
            .map(str::to_string)
    };
    if let Some(now) = kiosk(&after) {
        if kiosk(&before).as_deref() != Some(now.as_str()) {
            return Some(format!("the kiosk's screen is now {now}"));
        }
    }

    for (id, connector) in &after.touch {
        let was = before.touch.iter().find(|(old, _)| old == id);
        if was.map(|(_, old)| old) != Some(connector) {
            return Some(format!("touchscreen {id} belongs to {connector}"));
        }
    }

    None
}

#[derive(Debug, PartialEq, Eq)]
struct Section {
    header: String,
    /// `key=value` lines, trimmed, comments and blanks dropped.
    lines: Vec<String>,
}

impl Section {
    fn value(&self, key: &str) -> Option<&str> {
        self.lines.iter().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key).then(|| v.trim())
        })
    }
}

/// Just enough of weston.ini: `[section]` headers, `key=value` lines, and
/// `#` comments (Weston's parser only honours them at the start of a line).
struct Ini {
    sections: Vec<Section>,
    connected: Option<Vec<String>>,
    /// Each touchscreen's id and connector, from the `TOUCH` line.
    touch: Vec<(String, String)>,
}

impl Ini {
    fn parse(text: &str) -> Self {
        let mut sections: Vec<Section> = Vec::new();
        let mut connected = None;
        let mut touch = Vec::new();
        for line in text.lines() {
            if let Some(words) = line.strip_prefix(TOUCH) {
                // The id is an ID_PATH, which has no `=`; the connector none.
                touch = words
                    .split_whitespace()
                    .filter_map(|word| word.rsplit_once('='))
                    .map(|(id, connector)| (id.to_string(), connector.to_string()))
                    .collect();
                continue;
            }
            if let Some(names) = line.strip_prefix(MARKER) {
                connected = Some(
                    names
                        .split_whitespace()
                        .filter(|name| *name != "(none)")
                        .map(str::to_string)
                        .collect(),
                );
                continue;
            }
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                sections.push(Section {
                    header: header.trim().to_string(),
                    lines: Vec::new(),
                });
            } else if let Some(section) = sections.last_mut() {
                section.lines.push(line.to_string());
            }
        }
        Self {
            sections,
            connected,
            touch,
        }
    }

    /// The first match, as Weston takes it.
    fn section(&self, header: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.header == header)
    }

    fn output(&self, name: &str) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| s.header == "output" && s.value("name") == Some(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "[core]\nidle-time=0\n\n[shell]\npanel-position=none\n";

    fn config(connected: &str, extra: &str) -> String {
        format!("{BASE}\n{MARKER} {connected}\n{extra}")
    }

    #[test]
    fn the_same_config_is_no_reason() {
        let text = config("HDMI-A-1", "\n[output]\nname=HDMI-A-1\nscale=1\n");
        assert_eq!(verdict(&text, &text), None);
    }

    #[test]
    fn a_screen_that_was_not_there_at_start_is() {
        let running = config("(none)", "");
        let candidate = config("HDMI-A-1", "\n[output]\nname=HDMI-A-1\nscale=1\n");
        assert_eq!(
            verdict(&running, &candidate).as_deref(),
            Some("HDMI-A-1 connected since Weston started")
        );
    }

    #[test]
    fn a_new_screen_counts_even_with_no_output_sections() {
        // A base weston.ini that already names the connector: the generator
        // writes no [output] of its own, but a device booted with no screen
        // still needs a restart.
        assert!(verdict(&config("(none)", ""), &config("HDMI-A-1", "")).is_some());
    }

    #[test]
    fn a_screen_going_away_is_not() {
        let running = config("HDMI-A-1", "\n[output]\nname=HDMI-A-1\nscale=2\n");
        assert_eq!(verdict(&running, &config("(none)", "")), None);
    }

    #[test]
    fn the_same_screen_coming_back_is_not() {
        // Unplugged since start, then back: the running config still has its
        // section, and the generator writes the same one again.
        let text = config(
            "HDMI-A-1",
            "\n[output]\nname=HDMI-A-1\nscale=1\nmode=1280x720\n",
        );
        assert_eq!(verdict(&text, &text), None);
    }

    #[test]
    fn a_different_panel_on_the_same_connector_is() {
        let running = config("HDMI-A-1", "\n[output]\nname=HDMI-A-1\nscale=1\n");
        let candidate = config("HDMI-A-1", "\n[output]\nname=HDMI-A-1\nscale=2\n");
        assert_eq!(
            verdict(&running, &candidate).as_deref(),
            Some("HDMI-A-1 now wants [output] name=HDMI-A-1 scale=2")
        );
    }

    #[test]
    fn the_keyboard_verdict_changing_is() {
        let with = config("HDMI-A-1", "\n[input-method]\npath=\n");
        let without = config("HDMI-A-1", "");
        assert!(verdict(&without, &with)
            .unwrap()
            .contains("on-screen keyboard goes"));
        assert!(verdict(&with, &without)
            .unwrap()
            .contains("on-screen keyboard comes back"));
    }

    fn kiosk(connected: &str, output: &str, touch: &str) -> String {
        format!(
            "[core]\nidle-time=0\n\n[shell]\npanel-position=none\nkiosk-output={output}\n\
             \n{MARKER} {connected}\n{TOUCH} {touch}\n"
        )
    }

    #[test]
    fn the_kiosks_screen_moving_is() {
        let running = kiosk("HDMI-A-1 HDMI-A-2", "HDMI-A-1", "");
        let candidate = kiosk("HDMI-A-2", "HDMI-A-2", "");
        assert_eq!(
            verdict(&running, &candidate).as_deref(),
            Some("the kiosk's screen is now HDMI-A-2")
        );
        assert_eq!(verdict(&running, &running), None);
    }

    #[test]
    fn no_screen_left_is_not() {
        let running = kiosk("HDMI-A-1", "HDMI-A-1", "");
        let candidate = format!("{BASE}\n{MARKER} (none)\n{TOUCH} \n");
        assert_eq!(verdict(&running, &candidate), None);
    }

    #[test]
    fn a_touchscreen_new_or_moved_is_and_one_gone_is_not() {
        let running = kiosk("DSI-1", "DSI-1", "platform-ts=DSI-1");
        let added = kiosk("DSI-1", "DSI-1", "platform-ts=DSI-1 pci-usb-0:1=DSI-1");
        assert_eq!(
            verdict(&running, &added).as_deref(),
            Some("touchscreen pci-usb-0:1 belongs to DSI-1")
        );
        let moved = kiosk("DSI-1", "DSI-1", "platform-ts=HDMI-A-1");
        assert!(verdict(&running, &moved).is_some());
        assert_eq!(verdict(&added, &running), None);
    }

    #[test]
    fn comments_blanks_and_spacing_do_not_count() {
        let running = config("HDMI-A-1", "\n[output]\nname=HDMI-A-1\nscale=1\n");
        let candidate = config(
            "HDMI-A-1",
            "\n# a comment\n[output]\n  name=HDMI-A-1\n\nscale=1  \n",
        );
        assert_eq!(verdict(&running, &candidate), None);
    }

    #[test]
    fn a_config_without_the_marker_is_compared_by_sections_only() {
        let running = format!("{BASE}\n[output]\nname=HDMI-A-1\nscale=1\n");
        let candidate = config("HDMI-A-1 DP-1", "\n[output]\nname=HDMI-A-1\nscale=1\n");
        assert_eq!(verdict(&running, &candidate), None);
    }

    #[test]
    fn the_snapshot_sees_status_modes_and_input_devices() {
        let dir = tempfile::tempdir().unwrap();
        let drm = dir.path().join("drm");
        let input = dir.path().join("input");
        let hdmi = drm.join("card0-HDMI-A-1");
        fs::create_dir_all(&hdmi).unwrap();
        fs::create_dir_all(drm.join("card0")).unwrap();
        fs::write(hdmi.join("status"), "disconnected\n").unwrap();
        fs::write(hdmi.join("modes"), "").unwrap();
        fs::create_dir_all(input.join("input3/id")).unwrap();
        fs::write(input.join("input3/id/bustype"), "0003\n").unwrap();
        fs::write(input.join("input3/name"), "USB Keyboard\n").unwrap();
        fs::create_dir_all(input.join("event3")).unwrap();

        let sources = Sources {
            drm: &drm,
            input: &input,
        };
        let before = snapshot(&sources);
        assert_eq!(
            before,
            "card0-HDMI-A-1 disconnected []\ninput3 0003 USB Keyboard\n"
        );

        fs::write(hdmi.join("status"), "connected\n").unwrap();
        fs::write(hdmi.join("modes"), "1920x1080\n1280x720\n").unwrap();
        assert_eq!(
            snapshot(&sources),
            "card0-HDMI-A-1 connected [1920x1080 1280x720]\ninput3 0003 USB Keyboard\n"
        );
    }

    #[test]
    fn missing_roots_are_an_empty_snapshot() {
        let sources = Sources {
            drm: Path::new("/nonexistent/drm"),
            input: Path::new("/nonexistent/input"),
        };
        assert_eq!(snapshot(&sources), "");
    }

    /// The image's own generator, on two screens: the kiosk's gets
    /// `kiosk-output=` and the rotation, the other neither.
    #[test]
    fn the_real_generator_writes_the_kiosks_screen() {
        let generator = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../meta-tessaro-distro/recipes-graphics/wayland/files/tessaro-weston-config"
        ));
        let dir = tempfile::tempdir().unwrap();
        let (drm, input) = (dir.path().join("drm"), dir.path().join("input"));
        for name in ["HDMI-A-1", "HDMI-A-2"] {
            let connector = drm.join(format!("card0-{name}"));
            fs::create_dir_all(&connector).unwrap();
            fs::write(connector.join("status"), "connected\n").unwrap();
            fs::write(connector.join("modes"), "1920x1080\n1280x720\n").unwrap();
        }
        fs::create_dir_all(&input).unwrap();
        let base = dir.path().join("base.ini");
        fs::write(&base, BASE).unwrap();
        let settings = dir.path().join("generated.env");
        fs::write(
            &settings,
            "KIOSK_ROTATION=90\nKIOSK_RESOLUTION=1280x720\nKIOSK_VNC=off\n",
        )
        .unwrap();
        let output = dir.path().join("output.env");
        fs::write(
            &output,
            "KIOSK_OUTPUT_ACTIVE=HDMI-A-2\nKIOSK_OUTPUT_TOUCH=pci-usb-0:1=HDMI-A-2\n",
        )
        .unwrap();
        let sources = Sources {
            drm: &drm,
            input: &input,
        };

        let out = dir.path().join("weston.ini");
        let generated = Command::new("sh")
            .arg(generator)
            .env("TESSARO_WESTON_BASE", &base)
            .env("TESSARO_WESTON_CONFIG", &out)
            .env("TESSARO_DRM_ROOT", sources.drm)
            .env("TESSARO_INPUT_ROOT", sources.input)
            .env("TESSARO_OVERRIDE", &settings)
            .env("TESSARO_OUTPUT", &output)
            .status()
            .unwrap();
        assert!(generated.success());
        let text = fs::read_to_string(&out).unwrap();
        let ini = Ini::parse(&text);

        assert_eq!(
            ini.section("shell").unwrap().value("kiosk-output"),
            Some("HDMI-A-2")
        );
        assert_eq!(
            ini.touch,
            vec![("pci-usb-0:1".to_string(), "HDMI-A-2".to_string())]
        );
        let kiosk = ini.output("HDMI-A-2").unwrap();
        assert_eq!(kiosk.value("transform"), Some("rotate-90"));
        assert_eq!(kiosk.value("mode"), Some("1280x720"));
        let other = ini.output("HDMI-A-1").unwrap();
        assert_eq!(
            (other.value("transform"), other.value("mode")),
            (None, None)
        );
    }

    #[test]
    fn generate_runs_the_generator_into_the_scratch_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("gen");
        fs::write(
            &script,
            "#!/bin/sh\nprintf '[core]\\ndrm=%s\\n' \"$TESSARO_DRM_ROOT\" > \"$TESSARO_WESTON_CONFIG\"\necho chatter\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let out = dir.path().join("candidate.ini");
        let sources = Sources {
            drm: Path::new("/x/drm"),
            input: Path::new("/x/input"),
        };

        let (generated, output) = (Path::new("/x/generated.env"), Path::new("/x/output.env"));
        let text = generate(&script, &out, &sources, generated, output).unwrap();
        assert_eq!(text, "[core]\ndrm=/x/drm\n");

        fs::write(&script, "#!/bin/sh\necho broken >&2\nexit 3\n").unwrap();
        let err = generate(&script, &out, &sources, generated, output).unwrap_err();
        assert!(err.contains("broken"), "{err}");
    }
}
