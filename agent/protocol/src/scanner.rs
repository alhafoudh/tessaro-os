//! Barcode scanners as clients and the agent name them: how a scanner is
//! read, which USB device it is, how a scan ends, what a scan is, and the
//! scanners a script runs on (docs/scanners.md).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the agent reports to the page (`tessaro:scanner`), in `event`.
pub const EVENTS: &[&str] = &["begin", "end", "connected", "disconnected"];

/// Names a scanner cannot have: the API's own path segments.
pub const RESERVED: &[&str] = &["discover", "identify", "logs"];

/// A script's trigger that runs it on every scanner's scans.
pub const ANY: &str = "*";

/// The quiet between two pieces of a scan that ends it, by default. A
/// scanner in keyboard mode types a character every few milliseconds; a
/// serial one sends a scan in one burst.
pub const GAP_DEFAULT_MS: u32 = 100;
/// The same for HID POS, whose scans come in whole reports: the quiet
/// between a report and the one continuing it.
pub const GAP_HIDPOS_MS: u32 = 500;
pub const GAP_MIN_MS: u32 = 10;
pub const GAP_MAX_MS: u32 = 5000;
/// The longest scan kept: a large QR code holds about 3 kB, a PDF417 1 kB.
pub const SCAN_MAX: usize = 8192;
/// The longest prefix or suffix stripped from a scan.
pub const STRIP_MAX: usize = 16;
pub const BAUD_DEFAULT: u32 = 9600;
/// The baud rates a serial scanner may be read at. A USB CDC scanner
/// ignores it.
pub const BAUDS: &[u32] = &[1200, 2400, 4800, 9600, 19200, 38400, 57600, 115_200];
/// The keyboard layout a scanner in keyboard mode types in, by default.
pub const LAYOUT_DEFAULT: &str = "us";

/// How the device reads a scanner.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// A USB keyboard: the agent takes its keys from everyone else and
    /// reads them in the scanner's layout.
    #[default]
    Keyboard,
    /// A USB serial port (CDC ACM, or an FTDI, CP210x or CH341 bridge).
    Serial,
    /// A HID POS barcode scanner (USB HID usage page 0x8C).
    Hidpos,
}

impl Transport {
    pub const NAMES: &'static [&'static str] = &["keyboard", "serial", "hidpos"];

    pub fn name(self) -> &'static str {
        match self {
            Transport::Keyboard => "keyboard",
            Transport::Serial => "serial",
            Transport::Hidpos => "hidpos",
        }
    }

    /// The terminators a scanner read this way may end its scans with.
    pub fn terminators(self) -> &'static [&'static str] {
        match self {
            Transport::Keyboard => &["auto", "enter", "tab", "none"],
            Transport::Serial => &["auto", "cr", "lf", "crlf", "none", "0xNN"],
            Transport::Hidpos => &["auto"],
        }
    }

    pub fn gap_default(self) -> u32 {
        match self {
            Transport::Hidpos => GAP_HIDPOS_MS,
            _ => GAP_DEFAULT_MS,
        }
    }
}

impl std::str::FromStr for Transport {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        match name.trim().to_ascii_lowercase().as_str() {
            "keyboard" => Ok(Transport::Keyboard),
            "serial" => Ok(Transport::Serial),
            "hidpos" | "hid-pos" => Ok(Transport::Hidpos),
            _ => Err(format!(
                "{name:?} is not a scanner transport; one of {}",
                Transport::NAMES.join(", ")
            )),
        }
    }
}

/// What ends a scan besides the quiet of `gap_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminator {
    /// Keyboard: Enter. Serial: CR or LF, and the LF of a CR LF is not a
    /// scan of its own.
    Auto,
    Enter,
    Tab,
    Cr,
    Lf,
    CrLf,
    /// Only the quiet ends a scan.
    None,
    /// Serial: one byte.
    Byte(u8),
}

impl Terminator {
    /// The terminator as stored and shown: `auto`, `cr`, `0x03`.
    pub fn name(self) -> String {
        match self {
            Terminator::Auto => "auto".into(),
            Terminator::Enter => "enter".into(),
            Terminator::Tab => "tab".into(),
            Terminator::Cr => "cr".into(),
            Terminator::Lf => "lf".into(),
            Terminator::CrLf => "crlf".into(),
            Terminator::None => "none".into(),
            Terminator::Byte(byte) => format!("0x{byte:02x}"),
        }
    }

    /// A terminator as typed, for a scanner read with `transport`.
    pub fn parse(typed: &str, transport: Transport) -> Result<Terminator, String> {
        let lower = typed.trim().to_ascii_lowercase();
        let terminator = match lower.as_str() {
            "" | "auto" => Terminator::Auto,
            "enter" => Terminator::Enter,
            "tab" => Terminator::Tab,
            "cr" => Terminator::Cr,
            "lf" => Terminator::Lf,
            "crlf" => Terminator::CrLf,
            "none" => Terminator::None,
            hex if hex.starts_with("0x") && hex.len() == 4 => u8::from_str_radix(&hex[2..], 16)
                .map(Terminator::Byte)
                .map_err(|_| format!("{:?} is not a byte; 0x00 to 0xff", typed.trim()))?,
            _ => return Err(not_a_terminator(typed, transport)),
        };
        let allowed = matches!(
            (transport, terminator),
            (_, Terminator::Auto)
                | (
                    Transport::Keyboard,
                    Terminator::Enter | Terminator::Tab | Terminator::None
                )
                | (
                    Transport::Serial,
                    Terminator::Cr
                        | Terminator::Lf
                        | Terminator::CrLf
                        | Terminator::None
                        | Terminator::Byte(_)
                )
        );
        if allowed {
            Ok(terminator)
        } else {
            Err(not_a_terminator(typed, transport))
        }
    }
}

fn not_a_terminator(typed: &str, transport: Transport) -> String {
    match transport {
        Transport::Hidpos => {
            "a HID POS scanner ends its scans itself; its terminator is auto".into()
        }
        _ => format!(
            "{:?} does not end a {} scanner's scans; one of {}",
            typed.trim(),
            transport.name(),
            transport.terminators().join(", ")
        ),
    }
}

/// What a scanner is: which USB device, read how, its scans ended how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScannerSpec {
    /// `[a-z0-9][a-z0-9_-]*`, unique on the device: what events, scripts
    /// and the page name it by.
    pub name: String,
    pub transport: Transport,
    /// The USB vendor id, four hex digits: `0c2e`.
    pub vendor: String,
    /// The USB product id, four hex digits. Many scanners have one per
    /// mode, so a scanner switched to another mode is another scanner.
    pub product: String,
    /// The USB serial number, when the scanner has one: then it is found on
    /// any port.
    #[serde(default)]
    pub serial: Option<String>,
    /// The USB port it is plugged into, `1-1.2`: how a scanner without a
    /// serial number is told from another of the same model.
    #[serde(default)]
    pub port: Option<String>,
    /// Keyboard only: the keyboard layout the scanner is set to type in, as
    /// xkb names it: `us`, `de`, `sk(qwerty)`. `us` by default.
    #[serde(default)]
    pub layout: Option<String>,
    /// What ends a scan besides the quiet: `auto`, `enter`, `tab`, `cr`,
    /// `lf`, `crlf`, `none` or a byte as `0x03`, by the transport.
    #[serde(default)]
    pub terminator: Option<String>,
    /// Milliseconds of quiet that end a scan.
    #[serde(default)]
    pub gap_ms: Option<u32>,
    /// Serial only: the baud rate, 9600 by default.
    #[serde(default)]
    pub baud: Option<u32>,
    /// Taken off the start of a scan when it is there: a prefix the scanner
    /// is set to send.
    #[serde(default)]
    pub strip_prefix: Option<String>,
    /// Taken off the end of a scan, after the terminator.
    #[serde(default)]
    pub strip_suffix: Option<String>,
    /// Read it. A disabled scanner is left to whoever else reads it: a
    /// keyboard one types into the page again.
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl ScannerSpec {
    /// The id `scanner-create --device` takes and `scanner-discover`
    /// gives: `keyboard:0c2e:0b61:S12345` with a serial number,
    /// `serial:1a86:7523:@1-1.2` with a port.
    pub fn device(&self) -> String {
        device_id(
            self.transport,
            &self.vendor,
            &self.product,
            self.serial.as_deref(),
            self.port.as_deref(),
        )
    }

    pub fn terminator(&self) -> Terminator {
        Terminator::parse(self.terminator.as_deref().unwrap_or("auto"), self.transport)
            .unwrap_or(Terminator::Auto)
    }

    pub fn gap_ms(&self) -> u32 {
        self.gap_ms.unwrap_or(self.transport.gap_default())
    }

    pub fn layout(&self) -> &str {
        self.layout.as_deref().unwrap_or(LAYOUT_DEFAULT)
    }

    pub fn baud(&self) -> u32 {
        self.baud.unwrap_or(BAUD_DEFAULT)
    }

    /// Is this the USB device with these ids, serial number and port?
    pub fn matches(&self, vendor: &str, product: &str, serial: Option<&str>, port: &str) -> bool {
        if !self.vendor.eq_ignore_ascii_case(vendor) || !self.product.eq_ignore_ascii_case(product)
        {
            return false;
        }
        match (&self.serial, &self.port) {
            (Some(wanted), _) => serial == Some(wanted.as_str()),
            (None, Some(wanted)) => port == wanted,
            (None, None) => true,
        }
    }
}

/// A device id from its parts: the serial number when there is one, else
/// the port after `@`, else neither.
pub fn device_id(
    transport: Transport,
    vendor: &str,
    product: &str,
    serial: Option<&str>,
    port: Option<&str>,
) -> String {
    let mut id = format!(
        "{}:{}:{}",
        transport.name(),
        vendor.to_ascii_lowercase(),
        product.to_ascii_lowercase()
    );
    match (serial, port) {
        (Some(serial), _) => {
            id.push(':');
            id.push_str(serial);
        }
        (None, Some(port)) => {
            id.push_str(":@");
            id.push_str(port);
        }
        (None, None) => {}
    }
    id
}

/// A device id as `scanner-discover` gave it, apart: the transport, the
/// vendor and product ids, the serial number, the port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceId {
    pub transport: Transport,
    pub vendor: String,
    pub product: String,
    pub serial: Option<String>,
    pub port: Option<String>,
}

pub fn parse_device(typed: &str) -> Result<DeviceId, String> {
    let wrong = || {
        format!(
            "{:?} is not a scanner device; one from `tessaro-ctl scanner discover`, \
             e.g. keyboard:0c2e:0b61:@1-1.2",
            typed.trim()
        )
    };
    let mut parts = typed.trim().splitn(4, ':');
    let transport: Transport = parts.next().ok_or_else(wrong)?.parse()?;
    let vendor = parts.next().ok_or_else(wrong)?.to_ascii_lowercase();
    let product = parts.next().ok_or_else(wrong)?.to_ascii_lowercase();
    if !is_usb_id(&vendor) || !is_usb_id(&product) {
        return Err(wrong());
    }
    let (serial, port) = match parts.next().filter(|rest| !rest.is_empty()) {
        Some(port) if port.starts_with('@') => (None, Some(port[1..].to_string())),
        Some(serial) => (Some(serial.to_string()), None),
        None => (None, None),
    };
    Ok(DeviceId {
        transport,
        vendor,
        product,
        serial,
        port,
    })
}

fn is_usb_id(id: &str) -> bool {
    id.len() == 4 && id.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// `spec` made ready to save: trimmed and checked, its defaults left out.
/// `others` are the scanners it must not share a name with.
pub fn validate(mut spec: ScannerSpec, others: &[&ScannerSpec]) -> Result<ScannerSpec, String> {
    spec.name = spec.name.trim().to_string();
    check_name(&spec.name)?;
    if others.iter().any(|other| other.name == spec.name) {
        return Err(format!("a scanner named {} exists already", spec.name));
    }
    spec.vendor = spec.vendor.trim().to_ascii_lowercase();
    spec.product = spec.product.trim().to_ascii_lowercase();
    if !is_usb_id(&spec.vendor) || !is_usb_id(&spec.product) {
        return Err(format!(
            "{}:{} is not a USB vendor and product id; four hex digits each, e.g. 0c2e:0b61",
            spec.vendor, spec.product
        ));
    }
    spec.serial = trimmed(spec.serial);
    spec.port = trimmed(spec.port);
    if let Some(serial) = &spec.serial {
        if serial.len() > 64 || serial.chars().any(|ch| ch.is_control()) {
            return Err(format!("{serial:?} is not a USB serial number"));
        }
    }
    if let Some(port) = &spec.port {
        let valid = port.len() <= 32
            && port
                .chars()
                .all(|ch| ch.is_ascii_digit() || ch == '-' || ch == '.');
        if !valid {
            return Err(format!(
                "{port:?} is not a USB port; as sysfs names it, e.g. 1-1.2"
            ));
        }
    }
    if let Some(other) = others.iter().find(|other| {
        other.transport == spec.transport
            && other.vendor == spec.vendor
            && other.product == spec.product
            && other.serial == spec.serial
            && other.port == spec.port
    }) {
        return Err(format!("{} is that device already", other.name));
    }

    spec.layout = trimmed(spec.layout).map(|layout| layout.to_ascii_lowercase());
    match (&spec.layout, spec.transport) {
        (Some(_), transport) if transport != Transport::Keyboard => {
            return Err("only a keyboard scanner has a layout".into());
        }
        (Some(layout), _) => check_layout(layout)?,
        _ => {}
    }
    if spec.layout.as_deref() == Some(LAYOUT_DEFAULT) {
        spec.layout = None;
    }

    spec.terminator = match trimmed(spec.terminator) {
        Some(typed) => match Terminator::parse(&typed, spec.transport)? {
            Terminator::Auto => None,
            terminator => Some(terminator.name()),
        },
        None => None,
    };

    if let Some(gap) = spec.gap_ms {
        if !(GAP_MIN_MS..=GAP_MAX_MS).contains(&gap) {
            return Err(format!(
                "a gap of {gap} ms: from {GAP_MIN_MS} to {GAP_MAX_MS}"
            ));
        }
        if gap == spec.transport.gap_default() {
            spec.gap_ms = None;
        }
    }

    match (spec.baud, spec.transport) {
        (Some(_), transport) if transport != Transport::Serial => {
            return Err("only a serial scanner has a baud rate".into());
        }
        (Some(baud), _) if !BAUDS.contains(&baud) => {
            return Err(format!(
                "{baud} is not a baud rate; one of {}",
                BAUDS
                    .iter()
                    .map(|baud| baud.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        (Some(BAUD_DEFAULT), _) => spec.baud = None,
        _ => {}
    }

    for strip in [&mut spec.strip_prefix, &mut spec.strip_suffix] {
        *strip = strip.take().filter(|text| !text.is_empty());
        if let Some(text) = strip {
            if text.len() > STRIP_MAX {
                return Err(format!(
                    "{text:?} is longer than the {STRIP_MAX} bytes a prefix or suffix may be"
                ));
            }
        }
    }
    Ok(spec)
}

fn trimmed(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn check_name(name: &str) -> Result<(), String> {
    let valid = name.len() <= 40
        && name.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    if !valid {
        return Err(format!(
            "{name:?} is not a scanner name: lower-case letters, digits, - and _, \
             starting with a letter or digit, at most 40"
        ));
    }
    if RESERVED.contains(&name) {
        return Err(format!("{name:?} cannot name a scanner"));
    }
    Ok(())
}

/// An xkb layout, optionally with its variant: `de`, `sk(qwerty)`.
pub fn check_layout(layout: &str) -> Result<(), String> {
    let (name, variant) = match layout.split_once('(') {
        Some((name, rest)) => match rest.strip_suffix(')') {
            Some(variant) => (name, Some(variant)),
            None => ("", None),
        },
        None => (layout, None),
    };
    let valid = (2..=10).contains(&name.len())
        && name.chars().all(|ch| ch.is_ascii_lowercase())
        && variant.is_none_or(|variant| {
            (1..=24).contains(&variant.len())
                && variant
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{layout:?} is not a keyboard layout; as xkb names it, e.g. us, de or sk(qwerty)"
        ))
    }
}

/// One entry of what a script runs on, as typed: a scanner's name, or `*`
/// for every scanner. Whether the scanner exists is not checked: a script
/// may be set up before its scanner.
pub fn trigger(typed: &str) -> Result<String, String> {
    let name = typed.trim().to_ascii_lowercase();
    if name == ANY {
        return Ok(name);
    }
    check_name(&name)?;
    Ok(name)
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

/// Does a script that runs on `triggers` run on a scan of `scanner`?
pub fn runs_on(triggers: &[String], scanner: &str) -> bool {
    triggers
        .iter()
        .any(|trigger| trigger == ANY || trigger == scanner)
}

/// One scanner and how it is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScannerInfo {
    #[serde(flatten)]
    pub spec: ScannerSpec,
    /// `reading`, `missing` (not plugged in), `disabled` (the scanner or
    /// scanner.enable is off) or `failed` (plugged in, and could not be
    /// read: `message` says why).
    pub state: String,
    /// The device node it is read from while it is plugged in:
    /// `/dev/input/event3`, `/dev/ttyACM0`, `/dev/hidraw1`.
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    /// Scans since the agent started.
    #[serde(default)]
    pub scans: u64,
    /// When the last of them ended, the device's wall clock.
    #[serde(default)]
    pub last_scan: Option<String>,
}

/// Every scanner, and whether any is read (scanner.enable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScannerList {
    pub enabled: bool,
    pub scanners: Vec<ScannerInfo>,
}

/// A USB device that may be a scanner: a keyboard, a serial port or a HID
/// POS device. One step of `scanner-discover`, and what `scanner-identify`
/// answers with the scan it heard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScannerCandidate {
    /// What `scanner-create` takes as `--device`.
    pub device: String,
    pub transport: Transport,
    pub vendor: String,
    pub product: String,
    #[serde(default)]
    pub serial: Option<String>,
    pub port: String,
    /// `/dev/input/event3`, `/dev/ttyACM0`, `/dev/hidraw1`.
    pub node: String,
    /// What the device calls itself: its maker and product.
    pub description: String,
    /// The scanner this device is already, by name.
    #[serde(default)]
    pub known: Option<String>,
    /// From `scanner-identify`: the scan that came from it.
    #[serde(default)]
    pub scan: Option<Scan>,
}

/// One scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Scan {
    /// The scanner's name; empty from a device that is no scanner yet.
    pub scanner: String,
    pub transport: Transport,
    /// The scan as text, when it is UTF-8.
    #[serde(default)]
    pub text: Option<String>,
    /// The scan's bytes, base64, always.
    pub bytes: String,
    pub length: u32,
    /// From its first byte to its last.
    pub ms: u64,
    /// The AIM symbology identifier a HID POS scanner reports: `]Q1` for a
    /// QR code, `]E0` for an EAN-13.
    #[serde(default)]
    pub symbology: Option<String>,
    /// Milliseconds since the epoch.
    pub at_ms: i64,
    /// The device's wall clock: `21:04:05.123`.
    pub time: String,
}

/// What happened to the scanners, never what was scanned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScanLogEntry {
    /// Grows by one per entry.
    pub seq: u64,
    /// Milliseconds since the epoch.
    pub at_ms: i64,
    /// The device's wall clock: `21:04:05.123`.
    pub time: String,
    pub scanner: String,
    /// `scan`, `connected`, `disconnected` or `failed`.
    pub event: String,
    /// A scan's length in bytes.
    #[serde(default)]
    pub length: Option<u32>,
    /// How long a scan took.
    #[serde(default)]
    pub ms: Option<u64>,
    #[serde(default)]
    pub symbology: Option<String>,
    /// Why it failed; the device node it connected on.
    #[serde(default)]
    pub message: Option<String>,
}

/// A page of the scanners' log: every entry after a `seq`, and where to
/// ask from next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScanLog {
    pub entries: Vec<ScanLogEntry>,
    pub next: u64,
}

/// What is given replaces what the scanner has. An empty string unsets a
/// text field, back to its default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScannerChange {
    #[serde(default)]
    pub layout: Option<String>,
    #[serde(default)]
    pub terminator: Option<String>,
    /// `0` goes back to the default.
    #[serde(default)]
    pub gap_ms: Option<u32>,
    /// `0` goes back to the default.
    #[serde(default)]
    pub baud: Option<u32>,
    #[serde(default)]
    pub strip_prefix: Option<String>,
    #[serde(default)]
    pub strip_suffix: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

impl ScannerChange {
    /// `spec` with the change made, not yet validated.
    pub fn apply(&self, mut spec: ScannerSpec) -> ScannerSpec {
        let text = |change: &Option<String>, current: Option<String>| match change {
            Some(value) if value.trim().is_empty() => None,
            Some(value) => Some(value.clone()),
            None => current,
        };
        spec.layout = text(&self.layout, spec.layout);
        spec.terminator = text(&self.terminator, spec.terminator);
        spec.strip_prefix = match &self.strip_prefix {
            Some(value) if value.is_empty() => None,
            Some(value) => Some(value.clone()),
            None => spec.strip_prefix,
        };
        spec.strip_suffix = match &self.strip_suffix {
            Some(value) if value.is_empty() => None,
            Some(value) => Some(value.clone()),
            None => spec.strip_suffix,
        };
        match self.gap_ms {
            Some(0) => spec.gap_ms = None,
            Some(gap) => spec.gap_ms = Some(gap),
            None => {}
        }
        match self.baud {
            Some(0) => spec.baud = None,
            Some(baud) => spec.baud = Some(baud),
            None => {}
        }
        if let Some(enabled) = self.enabled {
            spec.enabled = enabled;
        }
        spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(transport: Transport) -> ScannerSpec {
        ScannerSpec {
            name: "front".into(),
            transport,
            vendor: "0C2E".into(),
            product: "0b61".into(),
            serial: None,
            port: Some("1-1.2".into()),
            layout: None,
            terminator: None,
            gap_ms: None,
            baud: None,
            strip_prefix: None,
            strip_suffix: None,
            enabled: true,
        }
    }

    #[test]
    fn a_device_id_goes_there_and_back() {
        let id = device_id(Transport::Serial, "1A86", "7523", None, Some("1-1.2"));
        assert_eq!(id, "serial:1a86:7523:@1-1.2");
        let parsed = parse_device(&id).unwrap();
        assert_eq!(parsed.transport, Transport::Serial);
        assert_eq!(parsed.port.as_deref(), Some("1-1.2"));
        assert_eq!(parsed.serial, None);
        let parsed = parse_device("hidpos:0c2e:0b61:AB:12").unwrap();
        assert_eq!(parsed.serial.as_deref(), Some("AB:12"));
        assert!(parse_device("keyboard:0c2e").is_err());
        assert!(parse_device("mouse:0c2e:0b61").is_err());
        assert!(parse_device("keyboard:zz2e:0b61").is_err());
    }

    #[test]
    fn terminators_depend_on_the_transport() {
        assert_eq!(
            Terminator::parse("CRLF", Transport::Serial).unwrap(),
            Terminator::CrLf
        );
        assert_eq!(
            Terminator::parse("0x03", Transport::Serial).unwrap(),
            Terminator::Byte(3)
        );
        assert!(Terminator::parse("cr", Transport::Keyboard).is_err());
        assert!(Terminator::parse("enter", Transport::Serial).is_err());
        assert!(Terminator::parse("tab", Transport::Hidpos).is_err());
        assert_eq!(
            Terminator::parse("", Transport::Hidpos).unwrap(),
            Terminator::Auto
        );
        assert!(Terminator::parse("0xzz", Transport::Serial).is_err());
        assert_eq!(Terminator::Byte(3).name(), "0x03");
    }

    #[test]
    fn validation_trims_and_drops_the_defaults() {
        let mut typed = spec(Transport::Keyboard);
        typed.layout = Some(" US ".into());
        typed.terminator = Some("auto".into());
        typed.gap_ms = Some(GAP_DEFAULT_MS);
        typed.strip_prefix = Some(String::new());
        let saved = validate(typed, &[]).unwrap();
        assert_eq!(saved.vendor, "0c2e");
        assert_eq!(saved.layout, None);
        assert_eq!(saved.terminator, None);
        assert_eq!(saved.gap_ms, None);
        assert_eq!(saved.strip_prefix, None);

        let mut serial = spec(Transport::Serial);
        serial.terminator = Some("LF".into());
        serial.baud = Some(115_200);
        let saved = validate(serial, &[]).unwrap();
        assert_eq!(saved.terminator.as_deref(), Some("lf"));
        assert_eq!(saved.baud, Some(115_200));
    }

    #[test]
    fn validation_refuses_what_does_not_fit_the_transport() {
        let mut serial = spec(Transport::Serial);
        serial.layout = Some("de".into());
        assert!(validate(serial, &[]).is_err());
        let mut keyboard = spec(Transport::Keyboard);
        keyboard.baud = Some(9600);
        assert!(validate(keyboard, &[]).is_err());
        let mut serial = spec(Transport::Serial);
        serial.baud = Some(1234);
        assert!(validate(serial, &[]).is_err());
        let mut keyboard = spec(Transport::Keyboard);
        keyboard.gap_ms = Some(1);
        assert!(validate(keyboard, &[]).is_err());
        let mut keyboard = spec(Transport::Keyboard);
        keyboard.name = "logs".into();
        assert!(validate(keyboard, &[]).is_err());
        let mut keyboard = spec(Transport::Keyboard);
        keyboard.layout = Some("sk(qwerty)".into());
        assert!(validate(keyboard, &[]).is_ok());
        let mut keyboard = spec(Transport::Keyboard);
        keyboard.layout = Some("sk(qwerty".into());
        assert!(validate(keyboard, &[]).is_err());
    }

    #[test]
    fn a_device_or_a_name_is_one_scanner_only() {
        let first = validate(spec(Transport::Keyboard), &[]).unwrap();
        let mut renamed = spec(Transport::Keyboard);
        renamed.name = "back".into();
        assert!(validate(renamed, &[&first])
            .unwrap_err()
            .contains("front is that device"));
        assert!(validate(spec(Transport::Serial), &[&first])
            .unwrap_err()
            .contains("exists already"));
    }

    #[test]
    fn a_scanner_matches_by_serial_number_else_by_port() {
        let by_port = validate(spec(Transport::Keyboard), &[]).unwrap();
        assert!(by_port.matches("0c2e", "0B61", None, "1-1.2"));
        assert!(!by_port.matches("0c2e", "0b61", None, "1-1.3"));
        let mut by_serial = by_port.clone();
        by_serial.serial = Some("S1".into());
        assert!(by_serial.matches("0c2e", "0b61", Some("S1"), "1-1.3"));
        assert!(!by_serial.matches("0c2e", "0b61", Some("S2"), "1-1.2"));
        assert!(!by_serial.matches("0c2f", "0b61", Some("S1"), "1-1.2"));
    }

    #[test]
    fn a_script_runs_on_its_scanners_or_on_every_one() {
        assert_eq!(triggers(" Front,*,front,").unwrap(), vec!["front", "*"]);
        assert!(triggers("no way").is_err());
        assert!(runs_on(&["front".to_string()], "front"));
        assert!(!runs_on(&["front".to_string()], "back"));
        assert!(runs_on(&[ANY.to_string()], "back"));
        assert!(!runs_on(&[], "back"));
    }

    #[test]
    fn a_change_unsets_with_an_empty_value() {
        let mut current = spec(Transport::Serial);
        current.terminator = Some("lf".into());
        current.baud = Some(19200);
        let change = ScannerChange {
            terminator: Some(String::new()),
            baud: Some(0),
            enabled: Some(false),
            strip_suffix: Some("!".into()),
            ..Default::default()
        };
        let changed = change.apply(current);
        assert_eq!(changed.terminator, None);
        assert_eq!(changed.baud, None);
        assert!(!changed.enabled);
        assert_eq!(changed.strip_suffix.as_deref(), Some("!"));
    }
}
