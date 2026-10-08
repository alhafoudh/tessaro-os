//! Barcode scanners: `tessaro-ctl scanner` and the Scanner page.

use protocol::scanner::{
    Scan, ScanLog, ScanLogEntry, ScannerCandidate, ScannerInfo, ScannerList, Transport,
};

use crate::text::{Fact, Line, Tone};

/// A scanner's state: being read, unplugged, switched off, or failing.
pub fn state_tone(state: &str) -> Tone {
    match state {
        "reading" => Tone::Ok,
        "disabled" => Tone::Muted,
        "failed" => Tone::Bad,
        _ => Tone::Warn,
    }
}

/// How a scanner is read, in words.
pub fn transport(transport: Transport) -> &'static str {
    match transport {
        Transport::Keyboard => "keyboard (its keys never reach the page)",
        Transport::Serial => "serial port",
        Transport::Hidpos => "HID POS",
    }
}

/// A scan's text with what cannot be shown named: `<CR>`, `<GS>`,
/// `<0x07>`. A GS1 code's group separators stay visible.
pub fn visible(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '\r' => out.push_str("<CR>"),
            '\n' => out.push_str("<LF>"),
            '\t' => out.push_str("<TAB>"),
            '\u{1d}' => out.push_str("<GS>"),
            '\u{1e}' => out.push_str("<RS>"),
            '\u{04}' => out.push_str("<EOT>"),
            ch if (ch as u32) < 0x20 || ch as u32 == 0x7f => {
                out.push_str(&format!("<0x{:02x}>", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out
}

/// Whether scanners are read at all, in a line.
pub fn enabled(list: &ScannerList) -> Line {
    if list.enabled {
        Line::of(Tone::Ok, "scanners are read")
            .text(" ")
            .add(Tone::Muted, "(scanner.enable)")
    } else {
        Line::of(Tone::Warn, "scanners are not read; turn them on with")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl config set scanner.enable=1")
    }
}

/// `scanner list`: a line per scanner with its state, its device under it,
/// and whether scanners are read.
pub fn list(list: &ScannerList) -> Vec<Line> {
    let mut lines = Vec::new();
    if list.scanners.is_empty() {
        lines.push(
            Line::of(Tone::Warn, "no scanners;")
                .text(" ")
                .add(Tone::Cmd, "tessaro-ctl scanner identify")
                .text(" ")
                .add(Tone::Muted, "names the device you scan with"),
        );
    }
    for scanner in &list.scanners {
        let mut line = Line::new()
            .pad(Tone::Heading, &scanner.spec.name, 16)
            .text(" ")
            .pad(Tone::Label, scanner.spec.transport.name(), 8)
            .text(" ")
            .add(state_tone(&scanner.state), &scanner.state);
        if let Some(last) = &scanner.last_scan {
            line = line
                .text("  ")
                .add(Tone::Muted, format!("last scan {last}"));
        }
        lines.push(line);
        let mut device = Line::plain("    ").add(Tone::Muted, scanner.spec.device());
        if let Some(node) = &scanner.node {
            device = device.text(" ").add(Tone::Muted, format!("on {node}"));
        }
        lines.push(device);
        if let Some(message) = &scanner.message {
            lines.push(Line::plain("    ").add(state_tone(&scanner.state), message));
        }
    }
    lines.push(Line::new());
    lines.push(enabled(list));
    lines
}

/// `scanner show`: one scanner in full.
pub fn show(scanner: &ScannerInfo) -> Vec<Fact> {
    let spec = &scanner.spec;
    let mut facts = vec![
        Fact::new("name", Line::of(Tone::Heading, &spec.name)),
        Fact::new(
            "state",
            Line::of(state_tone(&scanner.state), &scanner.state),
        ),
    ];
    if let Some(message) = &scanner.message {
        facts.push(Fact::new(
            "says",
            Line::of(state_tone(&scanner.state), message),
        ));
    }
    facts.push(Fact::new("read as", transport(spec.transport)));
    facts.push(Fact::new("device", spec.device()));
    facts.push(Fact::new(
        "node",
        match &scanner.node {
            Some(node) => Line::plain(node),
            None => Line::of(Tone::Muted, "not plugged in"),
        },
    ));
    if spec.transport == Transport::Keyboard {
        facts.push(Fact::new("layout", spec.layout()));
    }
    if spec.transport != Transport::Hidpos {
        facts.push(Fact::new("ends on", spec.terminator().name()));
    }
    facts.push(Fact::new(
        "gap",
        Line::plain(format!("{} ms of quiet ends a scan", spec.gap_ms())),
    ));
    if spec.transport == Transport::Serial {
        facts.push(Fact::new("baud", Line::plain(spec.baud().to_string())));
    }
    if let Some(prefix) = &spec.strip_prefix {
        facts.push(Fact::new(
            "strips",
            Line::plain(format!("{} first", visible(prefix))),
        ));
    }
    if let Some(suffix) = &spec.strip_suffix {
        facts.push(Fact::new(
            "strips",
            Line::plain(format!("{} last", visible(suffix))),
        ));
    }
    facts.push(Fact::new(
        "enabled",
        if spec.enabled {
            Line::of(Tone::Ok, "yes")
        } else {
            Line::of(Tone::Muted, "no")
        },
    ));
    facts.push(Fact::new("scans", Line::plain(scanner.scans.to_string())));
    facts.push(Fact::new(
        "last scan",
        match &scanner.last_scan {
            Some(last) => Line::plain(last),
            None => Line::of(Tone::Muted, "none yet"),
        },
    ));
    facts
}

/// One device `scanner discover` found, and the id to add it by.
pub fn candidate(candidate: &ScannerCandidate) -> Vec<Line> {
    let mut first = Line::new()
        .pad(Tone::Label, candidate.transport.name(), 8)
        .text(" ")
        .add(Tone::Heading, &candidate.description);
    if let Some(name) = &candidate.known {
        first = first
            .text(" ")
            .add(Tone::Muted, format!("(scanner {name})"));
    }
    vec![
        first,
        Line::plain("         ")
            .add(Tone::Plain, &candidate.device)
            .text(" ")
            .add(Tone::Muted, format!("on {}", candidate.node)),
    ]
}

/// What to do with what `scanner discover` found.
pub fn candidates_hint(found: &[ScannerCandidate]) -> Line {
    if found.is_empty() {
        return Line::of(
            Tone::Warn,
            "nothing that may be a scanner is plugged in: no USB keyboard, serial port or HID POS device",
        );
    }
    Line::of(Tone::Muted, "add one with")
        .text(" ")
        .add(Tone::Cmd, "tessaro-ctl scanner create NAME --device DEVICE")
        .text(", ")
        .add(Tone::Muted, "or let a scan pick it with")
        .text(" ")
        .add(Tone::Cmd, "tessaro-ctl scanner identify")
}

/// What `scanner identify` heard: the device and its scan, and how to add
/// it, or that nothing was scanned.
pub fn identified(heard: Option<&ScannerCandidate>) -> Vec<Line> {
    let Some(heard) = heard else {
        return vec![Line::of(
            Tone::Warn,
            "nothing was scanned while the device listened; scan a code while it does",
        )];
    };
    let mut lines = candidate(heard);
    if let Some(scan) = &heard.scan {
        lines.push(Line::plain("         ").join(scan_text(scan)));
    }
    lines.push(match &heard.known {
        Some(name) => Line::of(Tone::Muted, format!("it is scanner {name} already; see"))
            .text(" ")
            .add(Tone::Cmd, format!("tessaro-ctl scanner show {name}")),
        None => Line::of(Tone::Muted, "add it with").text(" ").add(
            Tone::Cmd,
            format!("tessaro-ctl scanner create NAME --device {}", heard.device),
        ),
    });
    lines
}

/// `1 byte`, `33 bytes`.
pub fn bytes(length: u32) -> String {
    if length == 1 {
        "1 byte".to_string()
    } else {
        format!("{length} bytes")
    }
}

/// What a scan says, and how long it is.
fn scan_text(scan: &Scan) -> Line {
    let text = match &scan.text {
        Some(text) => Line::of(Tone::Heading, visible(text)),
        None => Line::of(Tone::Warn, "not text"),
    };
    let mut about = format!("{} in {} ms", bytes(scan.length), scan.ms);
    if let Some(symbology) = &scan.symbology {
        about.push_str(&format!(", {symbology}"));
    }
    text.text(" ").add(Tone::Muted, format!("({about})"))
}

/// One scan of `scanner test`, with when it ended.
pub fn scan(scan: &Scan) -> Line {
    Line::new()
        .pad(Tone::Muted, &scan.time, 13)
        .join(scan_text(scan))
}

/// One entry of the scanners' log: when, which scanner, what happened.
pub fn log_entry(entry: &ScanLogEntry) -> Line {
    let line = Line::new()
        .pad(Tone::Muted, &entry.time, 13)
        .pad(Tone::Heading, &entry.scanner, 17);
    match entry.event.as_str() {
        "scan" => {
            let mut what = format!(
                "scan, {} in {} ms",
                bytes(entry.length.unwrap_or(0)),
                entry.ms.unwrap_or(0)
            );
            if let Some(symbology) = &entry.symbology {
                what.push_str(&format!(", {symbology}"));
            }
            line.add(Tone::Plain, what)
        }
        "connected" => {
            let line = line.add(Tone::Ok, "connected");
            match &entry.message {
                Some(node) => line.text(" ").add(Tone::Muted, format!("on {node}")),
                None => line,
            }
        }
        "disconnected" => line.add(Tone::Warn, "disconnected"),
        "failed" => line.add(
            Tone::Bad,
            format!(
                "failed: {}",
                entry.message.as_deref().unwrap_or("no reason given")
            ),
        ),
        other => line.add(Tone::Plain, other),
    }
}

/// A page of the log, oldest first.
pub fn logs(page: &ScanLog) -> Vec<Line> {
    if page.entries.is_empty() {
        return vec![Line::of(
            Tone::Muted,
            "nothing has happened to a scanner yet",
        )];
    }
    page.entries.iter().map(log_entry).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_cannot_be_shown_is_named() {
        assert_eq!(visible("01\u{1d}10ab\r\n"), "01<GS>10ab<CR><LF>");
        assert_eq!(visible("\u{7}é"), "<0x07>é");
    }
}
