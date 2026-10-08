//! HDMI-CEC as clients and the agent name it: the events the agent turns the
//! bus into, the TV remote's keys, the CEC events a script runs on, and the
//! names of addresses, opcodes and makers that `screen cec messages` reads a
//! message with (docs/cec.md).

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

/// The TV's logical address, always.
pub const TV: u8 = 0;
/// The audio system's: a sound bar or a receiver.
pub const AUDIO: u8 = 5;
/// The destination of a broadcast, and the sender of a message from a
/// device that has no logical address.
pub const BROADCAST: u8 = 15;

/// The most a message carries after its header: an opcode and operands.
pub const DATA_MAX: usize = 15;

/// `1.0.0.0`.
pub fn physical(address: u16) -> String {
    format!(
        "{}.{}.{}.{}",
        address >> 12,
        (address >> 8) & 0xf,
        (address >> 4) & 0xf,
        address & 0xf
    )
}

/// What a logical address makes a device.
pub fn kind(address: u8) -> &'static str {
    match address {
        0 => "tv",
        1 | 2 | 9 => "recorder",
        3 | 6 | 7 | 10 => "tuner",
        4 | 8 | 11 => "playback",
        5 => "audio",
        12 | 13 => "backup",
        14 => "specific",
        _ => "unregistered",
    }
}

/// The makers of TVs and what plugs into them, by the IEEE OUI their
/// `<Device Vendor ID>` names.
const VENDORS: &[(u32, &str)] = &[
    (0x000039, "Toshiba"),
    (0x0000f0, "Samsung"),
    (0x0005cd, "Denon"),
    (0x000678, "Marantz"),
    (0x000982, "Loewe"),
    (0x0009b0, "Onkyo"),
    (0x000ce7, "MediaTek"),
    (0x0010fa, "Apple"),
    (0x001582, "Pulse-Eight"),
    (0x001a11, "Google"),
    (0x008045, "Panasonic"),
    (0x00903e, "Philips"),
    (0x00a0de, "Yamaha"),
    (0x00d0d5, "Grundig"),
    (0x00e036, "Pioneer"),
    (0x00e091, "LG"),
    (0x08001f, "Sharp"),
    (0x080046, "Sony"),
    (0x18c086, "Broadcom"),
    (0x6b746d, "Vizio"),
    (0x9c645e, "Harman Kardon"),
];

/// A maker by its OUI, else the OUI in hex.
pub fn vendor(oui: u32) -> String {
    VENDORS
        .iter()
        .find(|(known, _)| *known == oui)
        .map(|(_, name)| name.to_string())
        .unwrap_or_else(|| format!("{oui:06x}"))
}

/// Every opcode of CEC 1.4 and 2.0, by the name `screen cec messages` gives
/// it.
const OPCODES: &[(u8, &str)] = &[
    (0x00, "feature-abort"),
    (0x04, "image-view-on"),
    (0x05, "tuner-step-increment"),
    (0x06, "tuner-step-decrement"),
    (0x07, "tuner-device-status"),
    (0x08, "give-tuner-device-status"),
    (0x09, "record-on"),
    (0x0a, "record-status"),
    (0x0b, "record-off"),
    (0x0d, "text-view-on"),
    (0x0f, "record-tv-screen"),
    (0x1a, "give-deck-status"),
    (0x1b, "deck-status"),
    (0x32, "set-menu-language"),
    (0x33, "clear-analogue-timer"),
    (0x34, "set-analogue-timer"),
    (0x35, "timer-status"),
    (0x36, "standby"),
    (0x41, "play"),
    (0x42, "deck-control"),
    (0x43, "timer-cleared-status"),
    (0x44, "user-control-pressed"),
    (0x45, "user-control-released"),
    (0x46, "give-osd-name"),
    (0x47, "set-osd-name"),
    (0x64, "set-osd-string"),
    (0x67, "set-timer-program-title"),
    (0x70, "system-audio-mode-request"),
    (0x71, "give-audio-status"),
    (0x72, "set-system-audio-mode"),
    (0x7a, "report-audio-status"),
    (0x7d, "give-system-audio-mode-status"),
    (0x7e, "system-audio-mode-status"),
    (0x80, "routing-change"),
    (0x81, "routing-information"),
    (0x82, "active-source"),
    (0x83, "give-physical-address"),
    (0x84, "report-physical-address"),
    (0x85, "request-active-source"),
    (0x86, "set-stream-path"),
    (0x87, "device-vendor-id"),
    (0x89, "vendor-command"),
    (0x8a, "vendor-remote-button-down"),
    (0x8b, "vendor-remote-button-up"),
    (0x8c, "give-device-vendor-id"),
    (0x8d, "menu-request"),
    (0x8e, "menu-status"),
    (0x8f, "give-device-power-status"),
    (0x90, "report-power-status"),
    (0x91, "get-menu-language"),
    (0x92, "select-analogue-service"),
    (0x93, "select-digital-service"),
    (0x97, "set-digital-timer"),
    (0x99, "clear-digital-timer"),
    (0x9a, "set-audio-rate"),
    (0x9d, "inactive-source"),
    (0x9e, "cec-version"),
    (0x9f, "get-cec-version"),
    (0xa0, "vendor-command-with-id"),
    (0xa1, "clear-external-timer"),
    (0xa2, "set-external-timer"),
    (0xa5, "give-features"),
    (0xa6, "report-features"),
    (0xa7, "request-current-latency"),
    (0xa8, "report-current-latency"),
    (0xc0, "initiate-arc"),
    (0xc1, "report-arc-initiated"),
    (0xc2, "report-arc-terminated"),
    (0xc3, "request-arc-initiation"),
    (0xc4, "request-arc-termination"),
    (0xc5, "terminate-arc"),
    (0xf8, "cdc-message"),
    (0xff, "abort"),
];

/// An opcode's name, `None` for one the standard does not have.
pub fn opcode_name(opcode: u8) -> Option<&'static str> {
    OPCODES
        .iter()
        .find(|(known, _)| *known == opcode)
        .map(|(_, name)| *name)
}

/// Bytes as `screen cec` shows and takes them: `44 41`.
pub fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The bytes typed as hex: `44 41`, `44:41`, `0x44,0x41`, `4441`. At most
/// `DATA_MAX`, never none.
pub fn parse_data(typed: &str) -> Result<Vec<u8>, String> {
    let digits: String = typed
        .split(|ch: char| ch.is_whitespace() || ch == ':' || ch == ',')
        .map(|part| part.trim_start_matches("0x").trim_start_matches("0X"))
        .map(|part| {
            if part.len() == 1 {
                format!("0{part}")
            } else {
                part.to_string()
            }
        })
        .collect();
    if digits.is_empty() {
        return Err("no bytes to send; an opcode in hex, e.g. 8f".to_string());
    }
    if !digits.len().is_multiple_of(2) || !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(format!("{:?} is not hex bytes, e.g. 44 41", typed.trim()));
    }
    let data: Vec<u8> = (0..digits.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or_default())
        .collect();
    if data.len() > DATA_MAX {
        return Err(format!(
            "a message carries at most {DATA_MAX} bytes after its header, this has {}",
            data.len()
        ));
    }
    Ok(data)
}

/// A logical address as typed: `tv`, `audio`, `all`, or 0 to 15.
pub fn parse_address(typed: &str) -> Result<u8, String> {
    match typed.trim().to_ascii_lowercase().as_str() {
        "tv" => Ok(TV),
        "audio" => Ok(AUDIO),
        "all" | "broadcast" => Ok(BROADCAST),
        number => match number.parse::<u8>() {
            Ok(address) if address <= 15 => Ok(address),
            _ => Err(format!(
                "{:?} is not a CEC address: tv, audio, all, or 0 to 15",
                typed.trim()
            )),
        },
    }
}

/// A remote key by its name, as `screen cec key` takes it.
pub fn key_code(name: &str) -> Result<u8, String> {
    let lower = name.trim().to_ascii_lowercase();
    KEYS.iter()
        .find(|key| key.name == lower)
        .map(|key| key.code)
        .ok_or_else(|| {
            format!(
                "{lower:?} is not a remote key; one of {}",
                KEYS.iter()
                    .map(|key| key.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_is_read_in_every_hex_spelling() {
        assert_eq!(parse_data("44 41").unwrap(), vec![0x44, 0x41]);
        assert_eq!(parse_data("0x44,0x41").unwrap(), vec![0x44, 0x41]);
        assert_eq!(parse_data("44:41").unwrap(), vec![0x44, 0x41]);
        assert_eq!(parse_data("8F").unwrap(), vec![0x8f]);
        assert_eq!(parse_data("4 1").unwrap(), vec![0x04, 0x01]);
        assert!(parse_data("").is_err());
        assert!(parse_data("4g").is_err());
        assert!(parse_data(&"00".repeat(16)).is_err());
        assert_eq!(hex(&[0x44, 0x01]), "44 01");
    }

    #[test]
    fn addresses_and_keys_are_read_by_name_or_number() {
        assert_eq!(parse_address("TV").unwrap(), 0);
        assert_eq!(parse_address("audio").unwrap(), 5);
        assert_eq!(parse_address("all").unwrap(), 15);
        assert_eq!(parse_address("11").unwrap(), 11);
        assert!(parse_address("16").is_err());
        assert_eq!(key_code("Volume-Up").unwrap(), 0x41);
        assert!(key_code("nope").is_err());
        assert_eq!(opcode_name(0x36), Some("standby"));
        assert_eq!(opcode_name(0x30), None);
        assert_eq!(vendor(0x0000f0), "Samsung");
        assert_eq!(physical(0x1200), "1.2.0.0");
    }

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
