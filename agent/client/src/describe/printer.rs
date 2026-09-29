//! Printing: `tessaro-ctl printer` and the Printer page.

use protocol::{PrintJob, PrintQueued, PrinterFound, PrinterInfo, PrinterList};

use crate::text::{Fact, Line, Tone};

/// A printer's state: printing along, stopped, or not set up yet.
pub fn state_tone(state: &str) -> Tone {
    match state {
        "idle" | "printing" => Tone::Ok,
        "stopped" => Tone::Bad,
        _ => Tone::Warn,
    }
}

/// Whether the page may print, in a line.
pub fn enabled(list: &PrinterList) -> Line {
    if list.enabled {
        Line::of(Tone::Ok, "the page prints")
            .text(" ")
            .add(Tone::Muted, "(printer.enable is on)")
    } else {
        Line::of(Tone::Warn, "the page does not print;")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl config set printer.enable=1")
    }
}

/// `printer list`: a line per printer, the default marked, its URI under
/// it, and whether the page may print.
pub fn list(list: &PrinterList) -> Vec<Line> {
    let mut lines = Vec::new();
    if list.printers.is_empty() {
        lines.push(
            Line::of(Tone::Warn, "no printers;")
                .text(" ")
                .add(Tone::Cmd, "tessaro-ctl printer discover")
                .text(" ")
                .add(Tone::Muted, "finds them"),
        );
    }
    for printer in &list.printers {
        let marker = if printer.default {
            Line::of(Tone::Ok, "*")
        } else {
            Line::plain(" ")
        };
        let mut line = marker
            .text(" ")
            .pad(Tone::Heading, &printer.spec.name, 16)
            .text(" ")
            .pad(Tone::Label, printer.spec.kind.name(), 4)
            .text(" ")
            .add(state_tone(&printer.state), &printer.state);
        if printer.queued > 0 {
            line = line
                .text("  ")
                .add(Tone::Plain, format!("{} queued", printer.queued));
        }
        lines.push(line);
        lines.push(Line::plain("    ").add(Tone::Muted, &printer.spec.uri));
        if let Some(message) = &printer.message {
            lines.push(Line::plain("    ").add(state_tone(&printer.state), message));
        }
    }
    lines.push(Line::new());
    lines.push(enabled(list));
    if !list.printers.is_empty() {
        lines.push(
            Line::of(Tone::Muted, "* window.print() prints here; another with")
                .text(" ")
                .add(Tone::Cmd, "tessaro-ctl printer default NAME"),
        );
    }
    lines
}

/// `printer show`: one printer in full.
pub fn show(printer: &PrinterInfo) -> Vec<Fact> {
    let mut facts = vec![
        Fact::new("name", Line::of(Tone::Heading, &printer.spec.name)),
        Fact::new(
            "state",
            Line::of(state_tone(&printer.state), &printer.state),
        ),
    ];
    if let Some(message) = &printer.message {
        facts.push(Fact::new(
            "says",
            Line::of(state_tone(&printer.state), message),
        ));
    }
    facts.push(Fact::new("uri", printer.spec.uri.as_str()));
    let kind = match printer.spec.kind {
        protocol::PrinterKind::Ipp => "ipp (driverless)",
        protocol::PrinterKind::Raw => "raw (bytes as they are)",
    };
    facts.push(Fact::new("kind", kind));
    if let Some(model) = &printer.model {
        facts.push(Fact::new("model", model.as_str()));
    }
    facts.push(Fact::new(
        "paper",
        match &printer.spec.media {
            Some(media) => Line::plain(media),
            None => Line::of(Tone::Muted, "the printer's own"),
        },
    ));
    facts.push(Fact::new(
        "default",
        if printer.default {
            Line::of(Tone::Ok, "yes")
        } else {
            Line::of(Tone::Muted, "no")
        },
    ));
    facts.push(Fact::new("queued", Line::plain(printer.queued.to_string())));
    for marker in &printer.markers {
        let level = match marker.level {
            Some(level) => Line::of(
                if level < 10 { Tone::Warn } else { Tone::Ok },
                format!("{level}%"),
            ),
            None => Line::of(Tone::Muted, "unknown"),
        };
        facts.push(Fact::new(marker.name.as_str(), level));
    }
    facts
}

/// `printer jobs`: what is waiting, oldest first.
pub fn jobs(jobs: &[PrintJob]) -> Vec<Line> {
    if jobs.is_empty() {
        return vec![Line::of(Tone::Muted, "no jobs waiting")];
    }
    let mut lines: Vec<Line> = jobs
        .iter()
        .map(|job| {
            Line::new()
                .pad(Tone::Heading, &job.job, 20)
                .text(" ")
                .pad(Tone::Plain, protocol::size_label(job.size), 10)
                .text(" ")
                .add(Tone::Muted, &job.submitted)
        })
        .collect();
    lines.push(Line::new());
    lines.push(
        Line::of(Tone::Muted, "cancel one with")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl printer cancel JOB"),
    );
    lines
}

/// One printer `printer discover` found, and the URI to add it by.
pub fn found(found: &PrinterFound) -> Vec<Line> {
    let mut first = Line::new()
        .pad(Tone::Label, found.kind.name(), 4)
        .text(" ")
        .add(Tone::Heading, &found.description);
    if let Some(name) = &found.known {
        first = first
            .text(" ")
            .add(Tone::Muted, format!("(printer {name})"));
    }
    vec![first, Line::plain("     ").add(Tone::Muted, &found.uri)]
}

/// What to do with what `printer discover` found.
pub fn found_hint(found: &[PrinterFound]) -> Line {
    if found.is_empty() {
        return Line::of(
            Tone::Warn,
            "no printers found; a network printer that is not announced is added by its URI",
        );
    }
    Line::of(Tone::Muted, "add one with")
        .text(" ")
        .add(Tone::Cmd, "tessaro-ctl printer create NAME --uri URI")
}

/// A document handed to CUPS.
pub fn queued(queued: &PrintQueued) -> Line {
    Line::of(Tone::Ok, &queued.message)
}
