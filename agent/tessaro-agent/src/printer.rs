//! Printing: the printers in the `printers` table of `tessaro.db` and the
//! CUPS queues they are set up as.
//!
//! CUPS runs as `tessaro-cups.service`, listening only on its unix socket,
//! with its own state under `/data/cups` (docs/printing.md). The
//! agent is its only administrator: the table is the truth, and every
//! queue CUPS has is one of its printers. CUPS is driven through its own
//! small clients - `lpadmin`, `lpstat`, `lp`, `cancel`, `lpinfo` - each of
//! them run under a deadline, the way `audio.rs` drives PipeWire.
//!
//! A driverless printer (`ipp`) is set up with `-m everywhere`: CUPS asks
//! the printer what it takes, so the printer has to answer then. One that
//! does not while the device starts is set up again once a minute, and
//! `printer-list` says `missing` until it is. A raw one is a queue without a
//! driver: the bytes go to the printer as they are.
//!
//! Parsing what the clients print, and deciding what to change, is pure and
//! tested here; their output is read with `LC_ALL=C`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use protocol::{
    PrintJob, PrinterFound, PrinterInfo, PrinterKind, PrinterMarker, PrinterSpec, PRINT_COPIES_MAX,
};
use tessaro_db::rusqlite::{self, params, types::Type, Connection};

use crate::db::Stored;
use crate::log::Log;
use crate::paths::Paths;
use crate::proc;

/// One `lpstat`, `lpadmin -x`, `cancel`: a request to the local CUPS.
const CALL: Duration = Duration::from_secs(10);
/// `lpadmin -m everywhere` asks the printer over the network what it takes.
const SETUP: Duration = Duration::from_secs(60);
/// `lp` hands the whole document to CUPS over its socket.
const SEND: Duration = Duration::from_secs(60);
/// `lpinfo -v` waits for the network backends to answer.
pub const DISCOVER: Duration = Duration::from_secs(40);
/// What `lpinfo` gives each network backend.
const DISCOVER_TIMEOUT_S: u32 = 15;
/// `ipptool` asking a printer for its supplies.
const QUERY: Duration = Duration::from_secs(10);

/// Names a printer cannot have: the API's own path segments.
const RESERVED: &[&str] = &["jobs", "discover"];
/// URI schemes a printer may be reached by: CUPS's backends in the image.
const SCHEMES: &[&str] = &[
    "ipp", "ipps", "http", "https", "socket", "lpd", "usb", "dnssd",
];

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Printers {
    pub printers: Vec<PrinterSpec>,
    /// The one `window.print()` uses. Always one of `printers`, or none.
    pub default: Option<String>,
}

impl Stored for Printers {
    const WHAT: &'static str = "the printers";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db
            .prepare("SELECT name, uri, kind, media, is_default FROM printers ORDER BY position")?;
        let mut default = None;
        let printers = rows
            .query_map([], |row| {
                let kind: String = row.get(2)?;
                let is_default: bool = row.get(4)?;
                let spec = PrinterSpec {
                    name: row.get(0)?,
                    uri: row.get(1)?,
                    kind: kind.parse().map_err(|err: String| {
                        rusqlite::Error::FromSqlConversionFailure(2, Type::Text, err.into())
                    })?,
                    media: row
                        .get::<_, Option<String>>(3)?
                        .filter(|media| !media.is_empty()),
                };
                Ok((spec, is_default))
            })?
            .map(|row| {
                row.map(|(spec, is_default)| {
                    if is_default {
                        default = Some(spec.name.clone());
                    }
                    spec
                })
            })
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { printers, default })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM printers", [])?;
        let mut insert = db.prepare(
            "INSERT INTO printers (name, position, uri, kind, media, is_default) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (position, printer) in self.printers.iter().enumerate() {
            insert.execute(params![
                printer.name,
                position as i64,
                printer.uri,
                printer.kind.name(),
                printer.media,
                self.default.as_deref() == Some(printer.name.as_str()),
            ])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM printers", []).map(drop)
    }
}

impl Printers {
    pub fn find(&self, name: &str) -> Result<&PrinterSpec, String> {
        self.printers
            .iter()
            .find(|printer| printer.name == name)
            .ok_or_else(|| missing(name))
    }

    /// `name`, or the default one without a name.
    pub fn pick(&self, name: Option<&str>) -> Result<&PrinterSpec, String> {
        match name.filter(|name| !name.is_empty()) {
            Some(name) => self.find(name),
            None => match &self.default {
                Some(name) => self.find(name),
                None => {
                    Err("no default printer; `tessaro-ctl printer default NAME` sets one".into())
                }
            },
        }
    }
}

pub fn missing(name: &str) -> String {
    format!("no printer {name:?}; `tessaro-ctl printer list` shows them")
}

/// `spec` made ready to save: trimmed and checked. `others` are the
/// printers it must not share a name with.
pub fn validate(mut spec: PrinterSpec, others: &[PrinterSpec]) -> Result<PrinterSpec, String> {
    spec.name = spec.name.trim().to_string();
    check_name(&spec.name)?;
    if others.iter().any(|other| other.name == spec.name) {
        return Err(format!("a printer named {} exists already", spec.name));
    }
    spec.uri = spec.uri.trim().to_string();
    check_uri(&spec.uri)?;
    spec.media = spec
        .media
        .map(|media| media.trim().to_string())
        .filter(|media| !media.is_empty());
    if let Some(media) = &spec.media {
        check_media(media)?;
    }
    Ok(spec)
}

fn check_name(name: &str) -> Result<(), String> {
    let valid = name.len() <= 40
        && name.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_');
    if !valid {
        return Err(format!(
            "{name:?} is not a printer name: lower-case letters, digits, - and _, \
             starting with a letter or digit, at most 40"
        ));
    }
    if RESERVED.contains(&name) {
        return Err(format!("{name:?} cannot name a printer"));
    }
    Ok(())
}

fn check_uri(uri: &str) -> Result<(), String> {
    let Some((scheme, rest)) = uri.split_once("://") else {
        return Err(format!(
            "{uri:?} is not a printer URI: ipp://host/ipp/print, socket://host:9100, \
             usb://..., or one from `tessaro-ctl printer discover`"
        ));
    };
    if !SCHEMES.contains(&scheme) {
        return Err(format!(
            "{uri:?}: the device reaches printers by {}",
            SCHEMES.join(", ")
        ));
    }
    if rest.is_empty() || uri.len() > 1024 {
        return Err(format!("{uri:?} is not a printer URI"));
    }
    // It goes to lpadmin as one argument, but CUPS writes it into
    // printers.conf a line at a time.
    if uri
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '"' | '\'' | '\\'))
    {
        return Err(format!(
            "{uri:?}: a printer URI has no spaces, quotes or backslashes"
        ));
    }
    Ok(())
}

fn check_media(media: &str) -> Result<(), String> {
    let valid = media.len() <= 64
        && media
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{media:?} is not a paper size: the printer's name for it, like iso_a4_210x297mm or A4"
        ))
    }
}

/// A job's title as CUPS gets it: printable, short.
pub fn title(title: Option<&str>) -> String {
    let text: String = title
        .unwrap_or("")
        .chars()
        .filter(|ch| !ch.is_control())
        .take(80)
        .collect();
    let text = text.trim();
    if text.is_empty() {
        "tessaro".to_string()
    } else {
        text.to_string()
    }
}

pub fn copies(copies: Option<u32>) -> Result<u32, String> {
    match copies.unwrap_or(1) {
        0 => Err("copies is at least 1".to_string()),
        n if n > PRINT_COPIES_MAX => Err(format!("copies is at most {PRINT_COPIES_MAX}")),
        n => Ok(n),
    }
}

// --- what CUPS has ---------------------------------------------------------

/// One queue as `lpstat` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    pub uri: String,
    /// `idle`, `printing` or `stopped`.
    pub state: String,
    pub message: Option<String>,
}

/// Every queue CUPS has, and its default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Present {
    pub queues: BTreeMap<String, Queue>,
    pub default: Option<String>,
}

/// `lpstat -v`: `device for NAME: URI` per queue.
pub fn parse_devices(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("device for "))
        .filter_map(|rest| rest.split_once(": "))
        .map(|(name, uri)| (name.trim().to_string(), uri.trim().to_string()))
        .collect()
}

/// `lpstat -p`: `printer NAME is idle.  enabled since ...`, `printer NAME
/// now printing NAME-3.  enabled since ...` or `printer NAME disabled since
/// ... -`, each maybe followed by indented lines of what CUPS says about it.
pub fn parse_states(text: &str) -> BTreeMap<String, (String, Option<String>)> {
    let mut states: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    let mut last: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("printer ") {
            let Some((name, said)) = rest.split_once(' ') else {
                continue;
            };
            let state = if said.starts_with("disabled") {
                "stopped"
            } else if said.starts_with("now printing") {
                "printing"
            } else {
                "idle"
            };
            states.insert(name.to_string(), (state.to_string(), None));
            last = Some(name.to_string());
        } else if line.starts_with(char::is_whitespace) {
            let said = line.trim();
            if said.is_empty() {
                continue;
            }
            if let Some(entry) = last.as_ref().and_then(|name| states.get_mut(name)) {
                entry.1 = Some(match entry.1.take() {
                    Some(before) => format!("{before}; {said}"),
                    None => said.to_string(),
                });
            }
        }
    }
    states
}

/// `lpstat -d`: `system default destination: NAME`.
pub fn parse_default(text: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix("system default destination: "))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

/// `lpstat -o`: `NAME-12   root   1024   Mon 29 Sep 2026 10:00:00 AM CEST`.
pub fn parse_jobs(text: &str, known: &[String]) -> Vec<PrintJob> {
    let mut jobs = Vec::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(job), Some(_user), Some(size)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(size) = size.parse::<u64>() else {
            continue;
        };
        // The id is the queue's name, a dash and a number; a name may
        // itself hold dashes.
        let Some((printer, number)) = job.rsplit_once('-') else {
            continue;
        };
        if number.parse::<u64>().is_err() {
            continue;
        }
        if !known.is_empty() && !known.iter().any(|name| name == printer) {
            continue;
        }
        jobs.push(PrintJob {
            job: job.to_string(),
            printer: printer.to_string(),
            size,
            submitted: fields.collect::<Vec<_>>().join(" "),
        });
    }
    jobs
}

/// `lp`'s `request id is NAME-12 (1 file(s))`.
pub fn parse_request(text: &str) -> Option<String> {
    let rest = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("request id is "))?;
    rest.split_whitespace().next().map(str::to_string)
}

/// `lpinfo -l -v`: a block per device, `Device: uri = ...` and then its
/// `class`, `info`, `make-and-model`. The bare backends it also lists
/// (`uri = socket`) are no printer.
pub fn parse_found(text: &str, have: &[PrinterSpec]) -> Vec<PrinterFound> {
    let mut found = Vec::new();
    let mut current: Option<BTreeMap<String, String>> = None;
    let finish = |fields: BTreeMap<String, String>, found: &mut Vec<PrinterFound>| {
        let uri = fields.get("uri").cloned().unwrap_or_default();
        if !uri.contains("://") {
            return;
        }
        let pick = |key: &str| {
            fields
                .get(key)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty() && *value != "Unknown")
                .map(str::to_string)
        };
        let description = pick("info")
            .or_else(|| pick("make-and-model"))
            .unwrap_or_else(|| uri.clone());
        found.push(PrinterFound {
            kind: kind_for(&uri),
            class: fields.get("class").cloned().unwrap_or_default(),
            known: have
                .iter()
                .find(|printer| printer.uri == uri)
                .map(|printer| printer.name.clone()),
            description,
            uri,
        });
    };
    for line in text.lines() {
        let line = line.trim();
        let (key, value) = match line.strip_prefix("Device:") {
            Some(rest) => {
                if let Some(fields) = current.take() {
                    finish(fields, &mut found);
                }
                current = Some(BTreeMap::new());
                match rest.trim().split_once(" = ") {
                    Some(pair) => pair,
                    None => continue,
                }
            }
            None => match line.split_once(" = ") {
                Some(pair) => pair,
                None => match line.strip_suffix(" =") {
                    Some(key) => (key, ""),
                    None => continue,
                },
            },
        };
        if let Some(fields) = current.as_mut() {
            fields.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    if let Some(fields) = current.take() {
        finish(fields, &mut found);
    }
    // The same printer over IPP and over DNS-SD: both are shown, driverless
    // first.
    found.sort_by(|a, b| {
        (a.kind != PrinterKind::Ipp, &a.description)
            .cmp(&(b.kind != PrinterKind::Ipp, &b.description))
    });
    found
}

/// How a printer at `uri` is best driven: over IPP if it speaks it, else
/// raw. A USB printer is raw: IPP over USB needs a daemon the image lacks.
pub fn kind_for(uri: &str) -> PrinterKind {
    let scheme = uri.split("://").next().unwrap_or("");
    match scheme {
        "ipp" | "ipps" => PrinterKind::Ipp,
        "dnssd" if uri.contains("._ipp") => PrinterKind::Ipp,
        _ => PrinterKind::Raw,
    }
}

/// What `ipptool` displayed of a printer's supplies and model:
/// `marker-names (1setOf nameWithoutLanguage) = Black,Cyan` and the like.
pub fn parse_markers(text: &str) -> (Vec<PrinterMarker>, Option<String>) {
    let value = |name: &str| {
        text.lines().find_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix(name)?;
            if !rest.starts_with(' ') {
                return None;
            }
            rest.split_once(" = ")
                .map(|(_, value)| value.trim().to_string())
        })
    };
    let names: Vec<String> = value("marker-names")
        .map(|names| {
            names
                .split(',')
                .map(|name| name.trim().to_string())
                .collect()
        })
        .unwrap_or_default();
    let levels: Vec<Option<u8>> = value("marker-levels")
        .map(|levels| {
            levels
                .split(',')
                .map(|level| {
                    level
                        .trim()
                        .parse::<i32>()
                        .ok()
                        .filter(|level| (0..=100).contains(level))
                        .map(|level| level as u8)
                })
                .collect()
        })
        .unwrap_or_default();
    let markers = names
        .into_iter()
        .filter(|name| !name.is_empty())
        .enumerate()
        .map(|(at, name)| PrinterMarker {
            name,
            level: levels.get(at).copied().flatten(),
        })
        .collect();
    (
        markers,
        value("printer-make-and-model").filter(|model| !model.is_empty()),
    )
}

// --- what to change --------------------------------------------------------

/// What makes CUPS match the stored printers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Set up, or set up again with a changed URI.
    pub setup: Vec<PrinterSpec>,
    pub remove: Vec<String>,
    /// The default to set, when it differs.
    pub default: Option<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.setup.is_empty() && self.remove.is_empty() && self.default.is_none()
    }
}

/// Whether `queue` is `printer` set up. CUPS resolves a `dnssd://` URI when
/// the queue is set up and reports what it found (`ipp://host.local:631/...`),
/// so such a printer is its queue by name alone; any other by its URI too.
pub fn is_set_up(printer: &PrinterSpec, queue: &Queue) -> bool {
    printer.uri.starts_with("dnssd://") || queue.uri == printer.uri
}

pub fn plan(wanted: &Printers, present: &Present) -> Plan {
    let mut plan = Plan::default();
    for printer in &wanted.printers {
        match present.queues.get(&printer.name) {
            Some(queue) if is_set_up(printer, queue) => {}
            _ => plan.setup.push(printer.clone()),
        }
    }
    for name in present.queues.keys() {
        if !wanted.printers.iter().any(|printer| &printer.name == name) {
            plan.remove.push(name.clone());
        }
    }
    if let Some(default) = &wanted.default {
        if present.default.as_ref() != Some(default) {
            plan.default = Some(default.clone());
        }
    }
    plan
}

/// `lpadmin`'s arguments for setting `printer` up.
pub fn setup_args(printer: &PrinterSpec) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        printer.name.clone(),
        "-E".to_string(),
        "-v".to_string(),
        printer.uri.clone(),
    ];
    if printer.kind == PrinterKind::Ipp {
        args.extend(["-m".to_string(), "everywhere".to_string()]);
        if let Some(media) = &printer.media {
            args.extend(["-o".to_string(), format!("media-default={media}")]);
        }
    }
    args
}

/// `lp`'s arguments for a document on stdin.
pub fn print_args(
    printer: &PrinterSpec,
    copies: u32,
    media: Option<&str>,
    title: &str,
) -> Vec<String> {
    let mut args = vec![
        "-d".to_string(),
        printer.name.clone(),
        "-n".to_string(),
        copies.to_string(),
        "-t".to_string(),
        title.to_string(),
    ];
    match printer.kind {
        PrinterKind::Raw => args.extend(["-o".to_string(), "raw".to_string()]),
        PrinterKind::Ipp => {
            if let Some(media) = media.or(printer.media.as_deref()) {
                args.extend(["-o".to_string(), format!("media={media}")]);
            }
        }
    }
    args.push("-".to_string());
    args
}

// --- test pages ------------------------------------------------------------

/// The test page for `printer`: a one-page PDF for a driverless printer, a
/// few lines of plain text for a raw one, which is what a receipt printer
/// prints as it is.
pub fn test_page(printer: &PrinterSpec, device: &str, when: &str) -> Vec<u8> {
    let lines = [
        "Tessaro test page".to_string(),
        String::new(),
        format!("Device:  {device}"),
        format!("Printer: {}", printer.name),
        format!("URI:     {}", printer.uri),
        format!("Printed: {when}"),
    ];
    match printer.kind {
        PrinterKind::Raw => {
            let mut text = lines.join("\n");
            text.push_str("\n\n\n\n\n");
            text.into_bytes()
        }
        PrinterKind::Ipp => pdf(&lines),
    }
}

/// A one-page A4 PDF of `lines` in Helvetica, built by hand: the offsets
/// in the cross-reference table are counted as it is written.
pub fn pdf(lines: &[String]) -> Vec<u8> {
    let escape = |line: &str| {
        line.chars()
            .filter(|ch| ch.is_ascii() && !ch.is_ascii_control())
            .flat_map(|ch| match ch {
                '(' | ')' | '\\' => vec!['\\', ch],
                _ => vec![ch],
            })
            .collect::<String>()
    };
    let mut content = String::from("BT /F1 14 Tf 72 770 Td 20 TL\n");
    for (at, line) in lines.iter().enumerate() {
        if at == 0 {
            content.push_str(&format!("/F1 22 Tf ({}) Tj /F1 14 Tf T*\n", escape(line)));
        } else {
            content.push_str(&format!("({}) Tj T*\n", escape(line)));
        }
    }
    content.push_str("ET\n");

    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}endstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (at, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", at + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

// --- running CUPS's clients ------------------------------------------------

/// CUPS's clients, pointed at the device's CUPS.
pub struct Cups {
    log: Arc<Log>,
    /// `CUPS_SERVER`: the socket the device's CUPS listens on.
    server: PathBuf,
    /// Where the clients are; empty for `PATH`.
    bin: PathBuf,
    /// The `ipptool` test that asks a printer for its supplies.
    markers_test: PathBuf,
    /// Whether this host's CUPS is the device's (`KIOSK_MANAGE_PRINTERS`).
    pub managed: bool,
}

impl Cups {
    pub fn new(log: Arc<Log>, paths: &Paths) -> Arc<Self> {
        Arc::new(Self {
            log,
            server: paths.cups_server.clone(),
            bin: paths.cups_bin.clone(),
            markers_test: paths.cups_markers_test.clone(),
            managed: paths.manage_printers,
        })
    }

    fn command(&self, program: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(self.bin.join(program));
        command.env("LC_ALL", "C").env("CUPS_SERVER", &self.server);
        command
    }

    /// Run `program`; its stdout, or what it said on failure.
    async fn run(
        &self,
        program: &'static str,
        args: &[String],
        input: Option<&[u8]>,
        limit: Duration,
    ) -> Result<String, String> {
        if !self.managed {
            return Err(
                "this host's printers are not managed (KIOSK_MANAGE_PRINTERS=0)".to_string(),
            );
        }
        // Without the socket every client fails with a bare "Bad file
        // descriptor", which says nothing about why.
        if !self.server.exists() {
            return Err(format!(
                "the device's CUPS is not running ({} is missing); `tessaro-ctl device logs -u tessaro-cups.service` says why",
                self.server.display()
            ));
        }
        let mut command = self.command(program);
        command.args(args);
        let output = proc::run_async(&mut command, input, program, limit).await?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
        let said = proc::said(&output);
        let said = said
            .strip_prefix(&format!("{program}: "))
            .unwrap_or(&said)
            .to_string();
        Err(if said.is_empty() {
            format!("{program} failed ({})", output.status)
        } else {
            said
        })
    }

    /// Every queue and the default, from three `lpstat` runs. `lpstat`
    /// fails when there is nothing to list, which is no queue.
    pub async fn present(&self) -> Result<Present, String> {
        let devices = match self.run("lpstat", &["-v".to_string()], None, CALL).await {
            Ok(text) => parse_devices(&text),
            Err(err) if nothing_listed(&err) => BTreeMap::new(),
            Err(err) => return Err(err),
        };
        let states = match self.run("lpstat", &["-p".to_string()], None, CALL).await {
            Ok(text) => parse_states(&text),
            Err(_) => BTreeMap::new(),
        };
        let default = self
            .run("lpstat", &["-d".to_string()], None, CALL)
            .await
            .ok()
            .and_then(|text| parse_default(&text));
        let queues = devices
            .into_iter()
            .map(|(name, uri)| {
                let (state, message) = states.get(&name).cloned().unwrap_or_default();
                (
                    name,
                    Queue {
                        uri,
                        state,
                        message,
                    },
                )
            })
            .collect();
        Ok(Present { queues, default })
    }

    /// Set one printer up in CUPS, replacing a queue of that name.
    pub async fn setup(&self, printer: &PrinterSpec) -> Result<(), String> {
        let limit = match printer.kind {
            PrinterKind::Ipp => SETUP,
            PrinterKind::Raw => CALL,
        };
        self.run("lpadmin", &setup_args(printer), None, limit)
            .await
            .map_err(|err| format!("setting up {}: {err}", printer.name))?;
        Ok(())
    }

    pub async fn remove(&self, name: &str) -> Result<(), String> {
        self.run("lpadmin", &["-x".to_string(), name.to_string()], None, CALL)
            .await
            .map(|_| ())
    }

    pub async fn set_default(&self, name: &str) -> Result<(), String> {
        self.run("lpadmin", &["-d".to_string(), name.to_string()], None, CALL)
            .await
            .map(|_| ())
    }

    /// Make CUPS match `wanted`. Each printer on its own: one that does not
    /// answer leaves the others set up. What failed, in words.
    pub async fn reconcile(&self, wanted: &Printers) -> Result<(), String> {
        let present = self.present().await?;
        let plan = plan(wanted, &present);
        if plan.is_empty() {
            return Ok(());
        }
        let mut problems = Vec::new();
        for name in &plan.remove {
            match self.remove(name).await {
                Ok(()) => self.log.info(format!("printers: removed queue {name}")),
                Err(err) => problems.push(format!("removing {name}: {err}")),
            }
        }
        let mut failed = Vec::new();
        for printer in &plan.setup {
            match self.setup(printer).await {
                Ok(()) => self.log.info(format!(
                    "printers: set up {} at {}",
                    printer.name, printer.uri
                )),
                Err(err) => {
                    problems.push(err);
                    failed.push(printer.name.as_str());
                }
            }
        }
        // CUPS refuses a default it has no queue for; the next reconcile
        // sets it once the printer is set up.
        if let Some(default) = plan
            .default
            .as_deref()
            .filter(|name| !failed.contains(name))
        {
            if let Err(err) = self.set_default(default).await {
                problems.push(format!("making {default} the default: {err}"));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// Hand a document to CUPS; the job id.
    pub async fn print(
        &self,
        printer: &PrinterSpec,
        document: &[u8],
        copies: u32,
        media: Option<&str>,
        title: &str,
    ) -> Result<String, String> {
        let args = print_args(printer, copies, media, title);
        let said = self.run("lp", &args, Some(document), SEND).await?;
        parse_request(&said).ok_or_else(|| format!("lp said nothing of a job: {}", said.trim()))
    }

    pub async fn jobs(
        &self,
        printer: Option<&str>,
        known: &[String],
    ) -> Result<Vec<PrintJob>, String> {
        let mut args = vec!["-o".to_string()];
        args.extend(printer.map(str::to_string));
        match self.run("lpstat", &args, None, CALL).await {
            Ok(text) => Ok(parse_jobs(&text, known)),
            Err(err) if nothing_listed(&err) => Ok(Vec::new()),
            Err(err) => Err(err),
        }
    }

    pub async fn cancel(&self, job: &str) -> Result<(), String> {
        self.run("cancel", &[job.to_string()], None, CALL)
            .await
            .map(|_| ())
    }

    /// Everything `lpinfo` finds on USB and the network.
    pub async fn discover(&self, have: &[PrinterSpec]) -> Result<Vec<PrinterFound>, String> {
        let args = [
            "-l".to_string(),
            "-v".to_string(),
            "--timeout".to_string(),
            DISCOVER_TIMEOUT_S.to_string(),
        ];
        let text = self.run("lpinfo", &args, None, DISCOVER).await?;
        Ok(parse_found(&text, have))
    }

    /// A printer's supplies and model, asked of the printer itself: only one
    /// reached over IPP can say. Nothing when it does not answer.
    pub async fn markers(&self, printer: &PrinterSpec) -> (Vec<PrinterMarker>, Option<String>) {
        let scheme = printer.uri.split("://").next().unwrap_or("");
        if !matches!(scheme, "ipp" | "ipps") || !self.managed {
            return (Vec::new(), None);
        }
        let mut command = self.command("ipptool");
        command
            .arg("-t")
            .arg("-T")
            .arg("5")
            .arg(&printer.uri)
            .arg(&self.markers_test);
        match proc::run_async(&mut command, None, "ipptool", QUERY).await {
            Ok(output) => parse_markers(&String::from_utf8_lossy(&output.stdout)),
            Err(err) => {
                self.log
                    .debug(format!("printer {}: supplies: {err}", printer.name));
                (Vec::new(), None)
            }
        }
    }
}

/// `lpstat` with nothing to list says so on stderr and fails.
fn nothing_listed(err: &str) -> bool {
    err.contains("No destinations added") || err.contains("no destinations")
}

/// One printer as `printer-list` answers it.
pub fn info(
    spec: &PrinterSpec,
    wanted: &Printers,
    present: &Present,
    problem: Option<&str>,
) -> PrinterInfo {
    let queue = present.queues.get(&spec.name);
    let (state, message) = match queue {
        Some(queue) if is_set_up(spec, queue) => (
            if queue.state.is_empty() {
                "idle".to_string()
            } else {
                queue.state.clone()
            },
            queue.message.clone(),
        ),
        _ => ("missing".to_string(), problem.map(str::to_string)),
    };
    PrinterInfo {
        spec: spec.clone(),
        default: wanted.default.as_deref() == Some(spec.name.as_str()),
        state,
        message,
        queued: 0,
        markers: Vec::new(),
        model: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, uri: &str, kind: PrinterKind) -> PrinterSpec {
        PrinterSpec {
            name: name.to_string(),
            uri: uri.to_string(),
            kind,
            media: None,
        }
    }

    #[test]
    fn a_printer_is_checked_before_it_is_saved() {
        let office = spec(" office ", " ipp://10.0.0.5/ipp/print ", PrinterKind::Ipp);
        let saved = validate(office, &[]).unwrap();
        assert_eq!(saved.name, "office");
        assert_eq!(saved.uri, "ipp://10.0.0.5/ipp/print");

        let twice = validate(saved.clone(), std::slice::from_ref(&saved)).unwrap_err();
        assert!(twice.contains("exists already"), "{twice}");
        for name in ["Office", "-a", "jobs", "a b", ""] {
            let bad = spec(name, "socket://10.0.0.9:9100", PrinterKind::Raw);
            assert!(validate(bad, &[]).is_err(), "{name:?}");
        }
        for uri in ["10.0.0.5", "ftp://x", "ipp://", "ipp://a b", "ipp://a\"b"] {
            let bad = spec("a", uri, PrinterKind::Ipp);
            assert!(validate(bad, &[]).is_err(), "{uri:?}");
        }
        let mut paper = spec("a", "ipp://x/ipp/print", PrinterKind::Ipp);
        paper.media = Some(" ".to_string());
        assert_eq!(validate(paper.clone(), &[]).unwrap().media, None);
        paper.media = Some("A4;rm".to_string());
        assert!(validate(paper, &[]).is_err());
    }

    #[test]
    fn the_default_is_the_printer_a_call_without_one_gets() {
        let mut printers = Printers {
            printers: vec![spec("office", "ipp://x/ipp/print", PrinterKind::Ipp)],
            default: None,
        };
        assert!(printers
            .pick(None)
            .unwrap_err()
            .contains("no default printer"));
        assert_eq!(printers.pick(Some("office")).unwrap().name, "office");
        printers.default = Some("office".to_string());
        assert_eq!(printers.pick(None).unwrap().name, "office");
        assert_eq!(printers.pick(Some("")).unwrap().name, "office");
        assert!(printers.pick(Some("front")).is_err());
    }

    #[test]
    fn lpstat_is_read_queue_by_queue() {
        let devices = parse_devices(
            "device for office: ipp://10.0.0.5/ipp/print\n\
             device for front-desk: socket://10.0.0.9:9100\n",
        );
        assert_eq!(devices["office"], "ipp://10.0.0.5/ipp/print");
        assert_eq!(devices["front-desk"], "socket://10.0.0.9:9100");

        let states = parse_states(
            "printer front-desk is idle.  enabled since Mon Sep 29 10:00:00 2026\n\
             printer office disabled since Mon Sep 29 10:00:00 2026 -\n\
             \tThe printer is not responding.\n\
             printer label now printing label-7.  enabled since Mon Sep 29 10:00:00 2026\n",
        );
        assert_eq!(states["front-desk"], ("idle".to_string(), None));
        assert_eq!(
            states["office"],
            (
                "stopped".to_string(),
                Some("The printer is not responding.".to_string())
            )
        );
        assert_eq!(states["label"].0, "printing");

        assert_eq!(
            parse_default("system default destination: office\n").as_deref(),
            Some("office")
        );
        assert_eq!(parse_default("no system default destination\n"), None);
    }

    #[test]
    fn jobs_are_read_with_the_queue_their_id_names() {
        let text = "front-desk-12           root              1024   Mon 29 Sep 2026 10:00:00 AM CEST\n\
                    office-3                root            250000   Mon 29 Sep 2026 10:01:00 AM CEST\n\
                    garbage\n";
        let jobs = parse_jobs(text, &[]);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].job, "front-desk-12");
        assert_eq!(jobs[0].printer, "front-desk");
        assert_eq!(jobs[0].size, 1024);
        assert_eq!(jobs[0].submitted, "Mon 29 Sep 2026 10:00:00 AM CEST");
        let only = parse_jobs(text, &["office".to_string()]);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].job, "office-3");

        assert_eq!(
            parse_request("request id is office-4 (1 file(s))\n").as_deref(),
            Some("office-4")
        );
        assert_eq!(parse_request("nothing"), None);
    }

    #[test]
    fn discovery_keeps_printers_not_bare_backends() {
        let text = "Device: uri = socket\n\
                    \tclass = network\n\
                    \tinfo = AppSocket/HP JetDirect\n\
                    \tmake-and-model = Unknown\n\
                    Device: uri = usb://EPSON/TM-T20III?serial=X4ZE\n\
                    \tclass = direct\n\
                    \tinfo = EPSON TM-T20III\n\
                    \tmake-and-model = EPSON TM-T20III\n\
                    \tdevice-id = MFG:EPSON;CMD:ESCPOS;\n\
                    \tlocation =\n\
                    Device: uri = ipp://BRW1234.local:631/ipp/print\n\
                    \tclass = network\n\
                    \tinfo = Brother HL-L2350DW\n\
                    \tmake-and-model = Brother HL-L2350DW series\n";
        let have = [spec(
            "office",
            "ipp://BRW1234.local:631/ipp/print",
            PrinterKind::Ipp,
        )];
        let found = parse_found(text, &have);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].uri, "ipp://BRW1234.local:631/ipp/print");
        assert_eq!(found[0].kind, PrinterKind::Ipp);
        assert_eq!(found[0].known.as_deref(), Some("office"));
        assert_eq!(found[1].description, "EPSON TM-T20III");
        assert_eq!(found[1].kind, PrinterKind::Raw);
        assert_eq!(found[1].class, "direct");

        assert_eq!(
            kind_for("dnssd://Office._ipp._tcp.local/?uuid=1"),
            PrinterKind::Ipp
        );
        assert_eq!(
            kind_for("dnssd://Label._pdl-datastream._tcp.local/"),
            PrinterKind::Raw
        );
    }

    #[test]
    fn supplies_are_read_from_what_ipptool_displays() {
        let text = "\"markers.test\":\n\
                    \x20   Get supplies                                             [PASS]\n\
                    \x20       marker-names (1setOf nameWithoutLanguage) = Black Toner,Drum\n\
                    \x20       marker-levels (1setOf integer) = 40,-3\n\
                    \x20       printer-make-and-model (textWithoutLanguage) = Brother HL-L2350DW series\n";
        let (markers, model) = parse_markers(text);
        assert_eq!(
            markers,
            [
                PrinterMarker {
                    name: "Black Toner".to_string(),
                    level: Some(40)
                },
                PrinterMarker {
                    name: "Drum".to_string(),
                    level: None
                },
            ]
        );
        assert_eq!(model.as_deref(), Some("Brother HL-L2350DW series"));
        assert_eq!(parse_markers("nothing"), (Vec::new(), None));
    }

    #[test]
    fn the_plan_changes_only_what_differs() {
        let office = spec("office", "ipp://10.0.0.5/ipp/print", PrinterKind::Ipp);
        let front = spec("front", "socket://10.0.0.9:9100", PrinterKind::Raw);
        let wanted = Printers {
            printers: vec![office.clone(), front.clone()],
            default: Some("office".to_string()),
        };
        let queue = |uri: &str| Queue {
            uri: uri.to_string(),
            state: "idle".to_string(),
            message: None,
        };

        let same = Present {
            queues: [
                ("office".to_string(), queue(&office.uri)),
                ("front".to_string(), queue(&front.uri)),
            ]
            .into(),
            default: Some("office".to_string()),
        };
        assert!(plan(&wanted, &same).is_empty());

        let drifted = Present {
            queues: [
                ("office".to_string(), queue("ipp://10.0.0.6/ipp/print")),
                ("old".to_string(), queue("socket://x:9100")),
            ]
            .into(),
            default: None,
        };
        let plan = plan(&wanted, &drifted);
        assert_eq!(plan.setup, [office.clone(), front]);
        assert_eq!(plan.remove, ["old"]);
        assert_eq!(plan.default.as_deref(), Some("office"));
    }

    #[test]
    fn a_dnssd_printer_is_its_queue_whatever_cups_resolved_it_to() {
        let mac = spec(
            "mac",
            "dnssd://Test%20Printer._ipp._tcp.local/?uuid=192deb0e",
            PrinterKind::Ipp,
        );
        let wanted = Printers {
            printers: vec![mac.clone()],
            default: Some("mac".to_string()),
        };
        let resolved = Queue {
            uri: "ipp://gray.local:8631/ipp/print".to_string(),
            state: "idle".to_string(),
            message: None,
        };
        let present = Present {
            queues: [("mac".to_string(), resolved)].into(),
            default: Some("mac".to_string()),
        };
        assert!(plan(&wanted, &present).is_empty());
        assert_eq!(info(&mac, &wanted, &present, None).state, "idle");
    }

    #[test]
    fn a_raw_printer_gets_no_driver_and_its_bytes_as_they_are() {
        let mut office = spec("office", "ipp://10.0.0.5/ipp/print", PrinterKind::Ipp);
        office.media = Some("iso_a4_210x297mm".to_string());
        assert_eq!(
            setup_args(&office),
            [
                "-p",
                "office",
                "-E",
                "-v",
                "ipp://10.0.0.5/ipp/print",
                "-m",
                "everywhere",
                "-o",
                "media-default=iso_a4_210x297mm"
            ]
        );
        assert_eq!(
            print_args(&office, 2, None, "ticket"),
            [
                "-d",
                "office",
                "-n",
                "2",
                "-t",
                "ticket",
                "-o",
                "media=iso_a4_210x297mm",
                "-"
            ]
        );

        let front = spec("front", "socket://10.0.0.9:9100", PrinterKind::Raw);
        assert_eq!(
            setup_args(&front),
            ["-p", "front", "-E", "-v", "socket://10.0.0.9:9100"]
        );
        assert_eq!(
            print_args(&front, 1, Some("A4"), "receipt"),
            ["-d", "front", "-n", "1", "-t", "receipt", "-o", "raw", "-"]
        );
    }

    #[test]
    fn copies_and_titles_stay_in_bounds() {
        assert_eq!(copies(None), Ok(1));
        assert!(copies(Some(0)).is_err());
        assert!(copies(Some(PRINT_COPIES_MAX + 1)).is_err());
        assert_eq!(title(Some(" a\nb ")), "ab");
        assert_eq!(title(None), "tessaro");
    }

    #[test]
    fn the_test_page_pdf_has_its_offsets_right() {
        let office = spec("office", "ipp://10.0.0.5/ipp/print", PrinterKind::Ipp);
        let page = test_page(&office, "tessaro-ab12 (a (b) \\ c)", "2026-09-29 10:00");
        let text = String::from_utf8(page).unwrap();
        assert!(text.starts_with("%PDF-1.4\n"));
        assert!(text.ends_with("%%EOF\n"));
        assert!(text.contains("\\(b\\)"), "{text}");

        let xref: usize = text
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(text[xref..].starts_with("xref\n"));
        let entries: Vec<usize> = text[xref..]
            .lines()
            .skip(3)
            .take(5)
            .map(|line| line[..10].parse().unwrap())
            .collect();
        for (at, offset) in entries.iter().enumerate() {
            assert!(
                text[*offset..].starts_with(&format!("{} 0 obj", at + 1)),
                "object {}",
                at + 1
            );
        }

        let front = spec("front", "socket://10.0.0.9:9100", PrinterKind::Raw);
        let receipt = String::from_utf8(test_page(&front, "tessaro-ab12", "now")).unwrap();
        assert!(receipt.starts_with("Tessaro test page\n"));
    }
}
