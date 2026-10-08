//! Barcode scanners, the flows both clients run: finding the devices that
//! may be scanners, naming the one a scan comes from, a scanner built from
//! what was typed, its scans watched for a while, and the scanners' log
//! followed as it grows. The words are in `describe::scanner`.

use std::time::{Duration, Instant};

use protocol::api::{self, ScanLogQuery, ScannerRef};
use protocol::scanner::{
    parse_device, Scan, ScanLog, ScanLogEntry, ScannerCandidate, ScannerChange, ScannerSpec,
};

use crate::connect::Session;

/// How often a followed log is asked for what is new.
pub const LOGS_POLL: Duration = Duration::from_secs(1);

/// A scanner's settings as a form or a command line has them, each as
/// typed; empty for the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typed {
    pub layout: String,
    pub terminator: String,
    /// Milliseconds.
    pub gap_ms: String,
    pub baud: String,
    pub strip_prefix: String,
    pub strip_suffix: String,
}

impl Typed {
    /// What a form shows for `spec`: saving it unchanged changes nothing.
    pub fn of(spec: &ScannerSpec) -> Self {
        Self {
            layout: spec.layout.clone().unwrap_or_default(),
            terminator: spec.terminator.clone().unwrap_or_default(),
            gap_ms: spec.gap_ms.map(|gap| gap.to_string()).unwrap_or_default(),
            baud: spec.baud.map(|baud| baud.to_string()).unwrap_or_default(),
            strip_prefix: spec.strip_prefix.clone().unwrap_or_default(),
            strip_suffix: spec.strip_suffix.clone().unwrap_or_default(),
        }
    }

    /// The scanner `name` on the device `device` (an id from `scanner
    /// discover` or `scanner identify`), with these settings.
    pub fn spec(&self, name: &str, device: &str, enabled: bool) -> Result<ScannerSpec, String> {
        let id = parse_device(device)?;
        let spec = ScannerSpec {
            name: name.trim().to_string(),
            transport: id.transport,
            vendor: id.vendor,
            product: id.product,
            serial: id.serial,
            port: id.port,
            layout: text(&self.layout),
            terminator: text(&self.terminator),
            gap_ms: number(&self.gap_ms, "gap")?,
            baud: number(&self.baud, "baud rate")?,
            strip_prefix: Some(self.strip_prefix.clone()).filter(|text| !text.is_empty()),
            strip_suffix: Some(self.strip_suffix.clone()).filter(|text| !text.is_empty()),
            enabled,
        };
        protocol::scanner::validate(spec, &[])
    }

    /// Every setting of an existing scanner replaced with what was typed;
    /// an empty one goes back to its default.
    pub fn change(&self) -> Result<ScannerChange, String> {
        Ok(ScannerChange {
            layout: Some(self.layout.trim().to_string()),
            terminator: Some(self.terminator.trim().to_string()),
            gap_ms: Some(number(&self.gap_ms, "gap")?.unwrap_or(0)),
            baud: Some(number(&self.baud, "baud rate")?.unwrap_or(0)),
            strip_prefix: Some(self.strip_prefix.clone()),
            strip_suffix: Some(self.strip_suffix.clone()),
            enabled: None,
        })
    }

    /// Only what was typed, the rest kept: what `scanner set` sends.
    pub fn given(&self) -> Result<ScannerChange, String> {
        let some = |typed: &str| Some(typed.to_string()).filter(|typed| !typed.is_empty());
        Ok(ScannerChange {
            layout: some(self.layout.trim()),
            terminator: some(self.terminator.trim()),
            gap_ms: number(&self.gap_ms, "gap")?,
            baud: number(&self.baud, "baud rate")?,
            strip_prefix: some(&self.strip_prefix),
            strip_suffix: some(&self.strip_suffix),
            enabled: None,
        })
    }
}

fn text(typed: &str) -> Option<String> {
    Some(typed.trim().to_string()).filter(|text| !text.is_empty())
}

fn number(typed: &str, what: &str) -> Result<Option<u32>, String> {
    match typed.trim() {
        "" => Ok(None),
        typed => typed
            .parse()
            .map(Some)
            .map_err(|_| format!("{typed:?} is not a {what}; a whole number")),
    }
}

/// Every device that may be a scanner. `each` sees them as they come;
/// `stop` ends the wait early.
pub fn discover(
    session: &mut Session,
    stop: &dyn Fn() -> bool,
    mut each: impl FnMut(&ScannerCandidate),
) -> Result<Vec<ScannerCandidate>, String> {
    let mut found = Vec::new();
    let mut failed = None;
    session.job::<api::scanner::Discover, ScannerCandidate>((), stop, |event| match event {
        Ok(candidate) => {
            each(&candidate);
            found.push(candidate);
        }
        Err(event) => failed = Some(format!("the device sent {event}")),
    })?;
    match failed {
        Some(err) => Err(err),
        None => Ok(found),
    }
}

/// The device the next scan comes from, with that scan; `None` when
/// nothing was scanned before the device stopped listening, or `stop`
/// said to stop first.
pub fn identify(
    session: &mut Session,
    stop: &dyn Fn() -> bool,
) -> Result<Option<ScannerCandidate>, String> {
    let mut heard = None;
    let mut failed = None;
    session.job::<api::scanner::Identify, ScannerCandidate>((), stop, |event| match event {
        Ok(candidate) => heard = Some(candidate),
        Err(event) => failed = Some(format!("the device sent {event}")),
    })?;
    match failed {
        Some(err) => Err(err),
        None => Ok(heard),
    }
}

/// The scans of `scanner` while the device watches them for this client,
/// or until `stop` says so. `each` sees every one as it comes.
pub fn test(
    session: &mut Session,
    scanner: &str,
    stop: &dyn Fn() -> bool,
    mut each: impl FnMut(&Scan),
) -> Result<Vec<Scan>, String> {
    let mut scans = Vec::new();
    session.job_at::<api::scanner::Test, Scan>(
        ScannerRef {
            scanner: scanner.to_string(),
        },
        (),
        stop,
        |event| {
            if let Ok(scan) = event {
                each(&scan);
                scans.push(scan);
            }
        },
    )?;
    Ok(scans)
}

/// The scanners' log after `after`, and with `follow` every entry after it
/// as it comes, until `stop` says so.
pub fn logs(
    session: &mut Session,
    mut after: u64,
    follow: bool,
    stop: &dyn Fn() -> bool,
    mut each: impl FnMut(&ScanLogEntry),
) -> Result<(), String> {
    loop {
        let page: ScanLog = session.call::<api::scanner::Logs>(ScanLogQuery { after }, ())?;
        page.entries.iter().for_each(&mut each);
        if !follow {
            return Ok(());
        }
        after = page.next;
        let waited = Instant::now();
        while waited.elapsed() < LOGS_POLL {
            if stop() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::scanner::Transport;

    #[test]
    fn a_scanner_is_built_from_a_device_id_and_its_settings() {
        let typed = Typed {
            terminator: "LF".into(),
            baud: "115200".into(),
            ..Typed::default()
        };
        let spec = typed
            .spec(" front ", "serial:1A86:7523:@1-1.2", true)
            .unwrap();
        assert_eq!(spec.name, "front");
        assert_eq!(spec.transport, Transport::Serial);
        assert_eq!(spec.vendor, "1a86");
        assert_eq!(spec.port.as_deref(), Some("1-1.2"));
        assert_eq!(spec.terminator.as_deref(), Some("lf"));
        assert_eq!(spec.baud, Some(115_200));
        assert_eq!(
            Typed::of(&spec)
                .spec("front", &spec.device(), true)
                .unwrap(),
            spec
        );

        assert!(Typed {
            layout: "de".into(),
            ..Typed::default()
        }
        .spec("x", "serial:1a86:7523", true)
        .is_err());
        assert!(Typed {
            gap_ms: "soon".into(),
            ..Typed::default()
        }
        .spec("x", "keyboard:1a86:7523", true)
        .is_err());
        assert!(Typed::default().spec("x", "nothing", true).is_err());
    }

    #[test]
    fn a_change_resets_what_is_empty_and_set_keeps_it() {
        let change = Typed::default().change().unwrap();
        assert_eq!(change.gap_ms, Some(0));
        assert_eq!(change.layout.as_deref(), Some(""));
        let given = Typed {
            gap_ms: "50".into(),
            ..Typed::default()
        }
        .given()
        .unwrap();
        assert_eq!(given.gap_ms, Some(50));
        assert_eq!(given.layout, None);
        assert_eq!(given.terminator, None);
    }
}
