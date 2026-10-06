//! HDMI-CEC as clients and the agent name it: the events the agent turns the
//! bus into, the TV remote's keys, and the CEC events a script runs on
//! (docs/cec.md).

/// What the agent reports from the bus, to the page (`tessaro:cec`) and to
/// the scripts that run on it.
pub const EVENTS: &[&str] = &["tv-on", "tv-standby", "source-gained", "source-lost", "key"];

/// A key of the TV remote: its CEC UI command, the name events and scripts
/// use, and what reaches the page as a key press with `screen.cec.keys`, if
/// anything does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteKey {
    pub code: u8,
    pub name: &'static str,
    pub press: Option<Press>,
}

/// A key press as CDP's `Input.dispatchKeyEvent` takes it: the DOM `key`
/// and `code`, the Windows virtual key code, and the text a printable key
/// types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    pub key: &'static str,
    pub code: &'static str,
    pub virtual_key: u16,
    pub text: Option<&'static str>,
}

const fn press(key: &'static str, code: &'static str, virtual_key: u16) -> Option<Press> {
    Some(Press {
        key,
        code,
        virtual_key,
        text: None,
    })
}

const fn typed(key: &'static str, code: &'static str, virtual_key: u16) -> Option<Press> {
    Some(Press {
        key,
        code,
        virtual_key,
        text: Some(key),
    })
}

/// Enter types a carriage return, which is what submits a form or presses
/// a focused button.
const ENTER: Option<Press> = Some(Press {
    key: "Enter",
    code: "Enter",
    virtual_key: 13,
    text: Some("\r"),
});

const fn remote(code: u8, name: &'static str, press: Option<Press>) -> RemoteKey {
    RemoteKey { code, name, press }
}

/// The UI commands of CEC 1.4's table that remotes send. The rest arrive as
/// `key` events named `0xNN` and never reach the page as a press.
pub const KEYS: &[RemoteKey] = &[
    remote(0x00, "select", ENTER),
    remote(0x01, "up", press("ArrowUp", "ArrowUp", 38)),
    remote(0x02, "down", press("ArrowDown", "ArrowDown", 40)),
    remote(0x03, "left", press("ArrowLeft", "ArrowLeft", 37)),
    remote(0x04, "right", press("ArrowRight", "ArrowRight", 39)),
    remote(0x09, "root-menu", press("ContextMenu", "ContextMenu", 93)),
    remote(0x0A, "setup-menu", None),
    remote(0x0B, "contents-menu", None),
    remote(0x0C, "favorite-menu", None),
    remote(0x0D, "exit", press("Escape", "Escape", 27)),
    remote(0x20, "0", typed("0", "Digit0", 48)),
    remote(0x21, "1", typed("1", "Digit1", 49)),
    remote(0x22, "2", typed("2", "Digit2", 50)),
    remote(0x23, "3", typed("3", "Digit3", 51)),
    remote(0x24, "4", typed("4", "Digit4", 52)),
    remote(0x25, "5", typed("5", "Digit5", 53)),
    remote(0x26, "6", typed("6", "Digit6", 54)),
    remote(0x27, "7", typed("7", "Digit7", 55)),
    remote(0x28, "8", typed("8", "Digit8", 56)),
    remote(0x29, "9", typed("9", "Digit9", 57)),
    remote(0x2A, "dot", typed(".", "Period", 190)),
    remote(0x2B, "enter", ENTER),
    remote(0x2C, "clear", press("Backspace", "Backspace", 8)),
    remote(0x30, "channel-up", press("PageUp", "PageUp", 33)),
    remote(0x31, "channel-down", press("PageDown", "PageDown", 34)),
    remote(0x32, "previous-channel", None),
    remote(0x35, "info", None),
    remote(0x36, "help", None),
    remote(0x37, "page-up", press("PageUp", "PageUp", 33)),
    remote(0x38, "page-down", press("PageDown", "PageDown", 34)),
    remote(0x40, "power", None),
    remote(0x41, "volume-up", None),
    remote(0x42, "volume-down", None),
    remote(0x43, "mute", None),
    remote(0x44, "play", press("MediaPlayPause", "MediaPlayPause", 179)),
    remote(0x45, "stop", press("MediaStop", "MediaStop", 178)),
    remote(
        0x46,
        "pause",
        press("MediaPlayPause", "MediaPlayPause", 179),
    ),
    remote(0x47, "record", None),
    remote(0x48, "rewind", press("MediaRewind", "", 0)),
    remote(0x49, "fast-forward", press("MediaFastForward", "", 0)),
    remote(0x4A, "eject", None),
    remote(
        0x4B,
        "forward",
        press("MediaTrackNext", "MediaTrackNext", 176),
    ),
    remote(
        0x4C,
        "backward",
        press("MediaTrackPrevious", "MediaTrackPrevious", 177),
    ),
    remote(0x53, "guide", None),
    remote(0x71, "blue", press("ColorF3Blue", "", 0)),
    remote(0x72, "red", press("ColorF0Red", "", 0)),
    remote(0x73, "green", press("ColorF1Green", "", 0)),
    remote(0x74, "yellow", press("ColorF2Yellow", "", 0)),
    remote(0x76, "data", None),
];

/// The key a UI command is, if the table has it.
pub fn key(code: u8) -> Option<&'static RemoteKey> {
    KEYS.iter().find(|key| key.code == code)
}

/// The name events give a UI command: the table's, else `0xNN`.
pub fn key_name(code: u8) -> String {
    match key(code) {
        Some(key) => key.name.to_string(),
        None => format!("0x{code:02x}"),
    }
}

/// One entry of what a script runs on, as typed: an event, `key` for every
/// key, or `key:<name>` for one. Stored lower-case.
pub fn trigger(typed: &str) -> Result<String, String> {
    let lower = typed.trim().to_ascii_lowercase();
    if let Some(name) = lower.strip_prefix("key:") {
        if KEYS.iter().any(|key| key.name == name) {
            return Ok(lower);
        }
        return Err(format!(
            "{name:?} is not a remote key; one of {}",
            KEYS.iter()
                .map(|key| key.name)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if EVENTS.contains(&lower.as_str()) {
        return Ok(lower);
    }
    Err(format!(
        "{:?} is not a CEC event; one of {}, or key:<name>",
        typed.trim(),
        EVENTS.join(", ")
    ))
}

/// Every entry of a comma-separated list, checked, without repeats.
pub fn triggers(typed: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for entry in typed.split(',').filter(|entry| !entry.trim().is_empty()) {
        let entry = trigger(entry)?;
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Does a script that runs on `triggers` run on this event? A key press is
/// `key` with the key's name.
pub fn runs_on(triggers: &[String], event: &str, key: Option<&str>) -> bool {
    triggers
        .iter()
        .any(|trigger| match (trigger.strip_prefix("key:"), key) {
            (Some(wanted), Some(key)) => event == "key" && wanted == key,
            (Some(_), None) => false,
            (None, _) => trigger == event,
        })
}

/// The longest name the TV shows for a device: CEC's `<Set OSD Name>`.
pub const NAME_MAX: usize = 14;

/// A name as the TV can show it: printable ASCII only, at most `NAME_MAX`.
pub fn osd_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_graphic() || *ch == ' ')
        .take(NAME_MAX)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_has_its_own_code_and_name() {
        for (i, a) in KEYS.iter().enumerate() {
            for b in &KEYS[i + 1..] {
                assert_ne!(a.code, b.code, "{} and {}", a.name, b.name);
                assert_ne!(a.name, b.name);
            }
        }
    }

    #[test]
    fn an_unknown_key_is_named_by_its_code() {
        assert_eq!(key_name(0x00), "select");
        assert_eq!(key_name(0x7f), "0x7f");
    }

    #[test]
    fn triggers_are_events_or_keys() {
        assert_eq!(
            triggers(" TV-On, key:select ,tv-on,").unwrap(),
            vec!["tv-on", "key:select"]
        );
        assert_eq!(triggers("key").unwrap(), vec!["key"]);
        assert!(triggers("tv-off").is_err());
        assert!(triggers("key:nope").is_err());
        assert!(triggers("").unwrap().is_empty());
    }

    #[test]
    fn a_script_runs_on_its_events_and_keys() {
        let on = vec!["tv-standby".to_string(), "key:red".to_string()];
        assert!(runs_on(&on, "tv-standby", None));
        assert!(!runs_on(&on, "tv-on", None));
        assert!(runs_on(&on, "key", Some("red")));
        assert!(!runs_on(&on, "key", Some("blue")));
        assert!(runs_on(&["key".to_string()], "key", Some("blue")));
    }

    #[test]
    fn an_osd_name_is_short_printable_ascii() {
        assert_eq!(osd_name("lobby-screen-number-two"), "lobby-screen-n");
        assert_eq!(osd_name("Kávovar 1"), "Kvovar 1");
        assert_eq!(osd_name(" x "), "x");
    }
}
