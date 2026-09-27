//! The clock: `tessaro-ctl time show`, the Time page, and the line of
//! `device status`.

use protocol::{TimeStatus, TimeSummary};

use crate::clock::{drift, leap, offset, offset_between, precision, span, utc_offset};
use crate::text::{unit_state, Fact, Line, Tone};

/// The one line `device status` shows: `Europe/Bratislava, in sync`.
pub fn summary(time: &TimeSummary) -> Line {
    let zone = time.timezone.as_deref().unwrap_or("(unknown timezone)");
    let sync = match (time.ntp, time.synchronized) {
        (Some(false), _) => Line::of(Tone::Warn, "NTP off"),
        (_, Some(true)) => Line::of(Tone::Ok, "in sync"),
        (_, Some(false)) => Line::of(Tone::Warn, "not in sync"),
        (_, None) => Line::of(Tone::Muted, "sync unknown"),
    };
    Line::plain(format!("{zone}, ")).join(sync)
}

/// What a change of the time.* keys did to the clock.
pub fn outcome(time: &str) -> Line {
    if time.starts_with("saved,") {
        Line::of(Tone::Warn, time)
    } else {
        Line::of(Tone::Ok, time)
    }
}

/// What could not be read, when something could not.
pub fn error(status: &TimeStatus) -> Option<Line> {
    status
        .error
        .as_ref()
        .map(|err| Line::of(Tone::Bad, "not everything could be read:").text(format!(" {err}")))
}

/// The clock and how it is kept, a fact a row.
pub fn facts(status: &TimeStatus) -> Vec<Fact> {
    let none = || Line::of(Tone::Muted, "(not reported)");
    let mut facts = Vec::new();

    let zone = match &status.timezone {
        Some(zone) => {
            let line = Line::plain(zone);
            match (&status.zone_abbreviation, status.utc_offset_seconds) {
                (Some(abbreviation), Some(at)) => line
                    .text(" ")
                    .add(Tone::Muted, format!("({abbreviation}, {})", utc_offset(at))),
                _ => line,
            }
        }
        None => none(),
    };
    facts.push(Fact::new("timezone", zone));
    if status
        .timezone
        .as_deref()
        .is_some_and(|zone| zone != status.setting_timezone)
    {
        facts.push(Fact::new(
            "",
            Line::of(
                Tone::Warn,
                format!(
                    "time.timezone is {}, not applied yet",
                    status.setting_timezone
                ),
            ),
        ));
    }
    facts.push(Fact::new(
        "local time",
        status
            .local_time
            .clone()
            .map(Line::plain)
            .unwrap_or_else(none),
    ));
    facts.push(Fact::new(
        "in sync",
        match status.synchronized {
            Some(true) => Line::of(Tone::Ok, "yes"),
            Some(false) => Line::of(Tone::Warn, "no"),
            None => none(),
        },
    ));
    facts.push(Fact::new(
        "ntp",
        match (status.ntp, status.can_ntp) {
            (_, Some(false)) => Line::of(Tone::Bad, "not available on this image"),
            (Some(true), _) => Line::of(Tone::Ok, "on"),
            (Some(false), _) => Line::of(Tone::Warn, "off"),
            (None, _) => none(),
        },
    ));
    if let Some(state) = &status.timesyncd {
        facts.push(Fact::new("timesyncd", Line::of(unit_state(state), state)));
    }

    if status.timesyncd.as_deref() == Some("active") {
        let server = match (&status.server_name, &status.server_address) {
            (Some(name), Some(address)) if name != address => {
                Line::plain(format!("{name} ")).add(Tone::Muted, format!("({address})"))
            }
            (Some(name), _) => Line::plain(name),
            (None, Some(address)) => Line::plain(address),
            (None, None) => Line::of(Tone::Warn, "none yet"),
        };
        facts.push(Fact::new("server", server));
        if let Some(poll) = status.poll_interval_usec {
            let line = Line::plain(format!("every {}", span(poll)));
            facts.push(Fact::new(
                "poll",
                match (status.poll_interval_min_usec, status.poll_interval_max_usec) {
                    (Some(min), Some(max)) => line
                        .text(" ")
                        .add(Tone::Muted, format!("({} to {})", span(min), span(max))),
                    _ => line,
                },
            ));
        }
        match &status.last {
            Some(last) => {
                facts.push(Fact::new("offset", offset(last.offset_usec)));
                facts.push(Fact::new("delay", span(last.delay_usec.unsigned_abs())));
                facts.push(Fact::new("jitter", span(last.jitter_usec)));
                if let Some(ppm) = status.frequency_ppm() {
                    facts.push(Fact::new(
                        "drift",
                        Line::plain(format!("{} ", drift(ppm)))
                            .add(Tone::Muted, "(the kernel's frequency correction)"),
                    ));
                }
                let distance =
                    Line::plain(span(last.root_delay_usec / 2 + last.root_dispersion_usec));
                facts.push(Fact::new(
                    "root dist.",
                    match status.root_distance_max_usec {
                        Some(max) => distance
                            .text(" ")
                            .add(Tone::Muted, format!("(max {})", span(max))),
                        None => distance,
                    },
                ));
                facts.push(Fact::new(
                    "stratum",
                    Line::plain(format!("{} ", last.stratum)).add(
                        Tone::Muted,
                        format!(
                            "(reference {}, precision {})",
                            last.reference,
                            precision(last.precision)
                        ),
                    ),
                ));
                if last.leap != 0 {
                    facts.push(Fact::new("leap", Line::of(Tone::Warn, leap(last.leap))));
                }
                let age = status
                    .now_usec
                    .and_then(|now| now.checked_sub(last.received_usec))
                    .map(|age| format!("{} ago, ", span(age)))
                    .unwrap_or_default();
                let answers = Line::plain(format!("{age}{} so far", last.packet_count));
                facts.push(Fact::new(
                    "answers",
                    if last.spike {
                        answers
                            .text(", ")
                            .add(Tone::Warn, "the last one was an outlier")
                    } else {
                        answers
                    },
                ));
            }
            None => facts.push(Fact::new("answers", Line::of(Tone::Warn, "none yet"))),
        }
    }

    if let (Some(rtc), Some(now)) = (status.rtc_usec, status.now_usec) {
        let line = Line::plain(format!(
            "{} from the system clock",
            offset_between(rtc, now)
        ));
        facts.push(Fact::new(
            "hw clock",
            if status.local_rtc == Some(true) {
                line.add(Tone::Warn, " (keeps local time)")
            } else {
                line
            },
        ));
    } else if status.now_usec.is_some() {
        facts.push(Fact::new("hw clock", Line::of(Tone::Muted, "none")));
    }
    facts
}

/// Each source of NTP servers, with where it comes from.
pub fn servers(status: &TimeStatus) -> Vec<Fact> {
    let servers = &status.servers;
    [
        (
            "runtime",
            &servers.runtime,
            "DHCP's, while time.ntp.servers is empty",
        ),
        ("system", &servers.system, "time.ntp.servers"),
        ("fallback", &servers.fallback, "the image's"),
        ("dhcp", &servers.dhcp, "what the network offers"),
    ]
    .into_iter()
    .map(|(label, names, from)| {
        let list = if names.is_empty() {
            Line::of(Tone::Muted, "(none)")
        } else {
            Line::plain(names.join(" "))
        };
        Fact::new(label, list.text(" ").add(Tone::Muted, format!("({from})")))
    })
    .collect()
}

/// What to try when NTP is on but the clock is not in sync.
pub fn hint(status: &TimeStatus) -> Option<Line> {
    (status.ntp == Some(true) && status.synchronized == Some(false)).then(|| {
        Line::of(Tone::Muted, "not in sync? name a reachable server:")
            .text(" ")
            .add(Tone::Cmd, "tessaro-ctl time ntp on --server HOST")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_says_zone_and_sync() {
        let summary_of = |ntp, synchronized| {
            summary(&TimeSummary {
                timezone: Some("Europe/Bratislava".to_string()),
                synchronized,
                ntp,
            })
            .to_string()
        };
        assert_eq!(
            summary_of(Some(true), Some(true)),
            "Europe/Bratislava, in sync"
        );
        assert_eq!(
            summary_of(Some(true), Some(false)),
            "Europe/Bratislava, not in sync"
        );
        assert_eq!(
            summary_of(Some(false), Some(false)),
            "Europe/Bratislava, NTP off"
        );
        assert_eq!(summary_of(None, None), "Europe/Bratislava, sync unknown");
    }
}
