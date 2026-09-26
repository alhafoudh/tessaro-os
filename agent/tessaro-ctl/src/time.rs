//! `tessaro-ctl time ...`: the timezone, NTP servers and sync, and setting
//! the clock by hand.
//!
//! `timezone` and `ntp` are a `config set` of the time.* keys: the device
//! applies them to the running clock at once, and restarts only
//! systemd-timesyncd, only when its servers change. `show` prints what
//! timedated and timesyncd report - offset, delay, jitter, drift - and
//! nothing the device measured on its own.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anstream::println;
use clap::Subcommand;
use protocol::api;
use protocol::{keys, TimeStatus, TimeSummary};
use tessaro_client::clock::{drift, leap, offset, offset_between, precision, span, utc_offset};

use crate::connect::Session;
use crate::style::{self, pad, paint};
use crate::{print, show_applied, Toggle};

#[derive(Subcommand)]
pub enum TimeCmd {
    /// The clock as systemd reports it: timezone, local time, whether it is
    /// in sync, the NTP server, offset, delay, jitter and drift, and where
    /// the servers come from.
    Show,
    /// Every timezone the device knows; with FILTER, only the ones that
    /// contain it.
    ///
    ///   tessaro-ctl time zones europe
    Zones { filter: Option<String> },
    /// The device's timezone, from `tessaro-ctl time zones`. The browser
    /// follows without a restart. The same as
    /// `tessaro-ctl config set time.timezone=...`.
    ///
    ///   tessaro-ctl time timezone Europe/Bratislava
    Timezone { zone: String },
    /// Keep the clock in sync over NTP, or stop. The same as
    /// `tessaro-ctl config set time.ntp.enable=1|0`.
    ///
    ///   tessaro-ctl time ntp on --server ntp1.corp.test --server ntp2.corp.test
    ///
    /// Without --server the device uses the servers the network's DHCP
    /// offers, else the image's fallback; `tessaro-ctl config unset
    /// time.ntp.servers` goes back to that.
    Ntp {
        state: Toggle,
        /// With `on`: set time.ntp.servers in the same change. Repeat it for
        /// more than one server.
        #[arg(long = "server", value_name = "HOST")]
        servers: Vec<String>,
    },
    /// Ask the NTP servers again now, instead of at the next poll.
    Sync,
    /// Set the clock by hand, with NTP off: to TIME, as YYYY-MM-DD
    /// HH:MM[:SS] in the device's timezone, or without TIME to this
    /// computer's clock.
    ///
    ///   tessaro-ctl time ntp off && tessaro-ctl time set
    ///   tessaro-ctl time set '2026-09-25 14:30'
    Set { time: Option<String> },
}

pub fn run(session: &mut Session, command: TimeCmd, json: bool) -> Result<(), String> {
    match command {
        TimeCmd::Show => {
            let status = session.fetch::<api::time::Show>()?;
            print(json, &status, || show(&status))
        }
        TimeCmd::Zones { filter } => {
            let mut zones = session.fetch::<api::time::Zones>()?;
            if let Some(filter) = filter {
                let filter = filter.to_ascii_lowercase();
                zones.retain(|zone| zone.to_ascii_lowercase().contains(&filter));
            }
            print(json, &zones, || {
                for zone in &zones {
                    println!("{zone}");
                }
                if zones.is_empty() {
                    println!("{}", paint(style::MUTED, "(no timezone matches)"));
                }
            })
        }
        TimeCmd::Timezone { zone } => set(
            session,
            json,
            BTreeMap::from([(keys::TIMEZONE.to_string(), zone)]),
        ),
        TimeCmd::Ntp { state, servers } => {
            let mut values =
                BTreeMap::from([(keys::NTP_ENABLE.to_string(), state.flag().to_string())]);
            if !servers.is_empty() {
                if state == Toggle::Off {
                    return Err("--server goes with `tessaro-ctl time ntp on`".to_string());
                }
                values.insert(keys::NTP_SERVERS.to_string(), servers.join(","));
            }
            set(session, json, values)
        }
        TimeCmd::Sync => {
            let done = session.send::<api::time::Sync>(())?;
            print(json, &done, || {
                println!("{}", paint(style::OK, &done.message))
            })
        }
        TimeCmd::Set { time } => {
            let body = match time {
                Some(local) => {
                    protocol::parse_local_time(&local)?;
                    api::TimeSetBody {
                        usec: None,
                        local: Some(local),
                    }
                }
                None => api::TimeSetBody {
                    usec: Some(now_usec()?),
                    local: None,
                },
            };
            let done = session.send::<api::time::Set>(body)?;
            print(json, &done, || {
                println!("{}", paint(style::OK, &done.message))
            })
        }
    }
}

fn set(session: &mut Session, json: bool, values: BTreeMap<String, String>) -> Result<(), String> {
    let applied = crate::set(session, values)?;
    print(json, &applied, || show_applied(&applied, false))
}

/// This computer's clock, for `time set` without a time.
fn now_usec() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_micros() as u64)
        .map_err(|_| "this computer's clock is before 1970".to_string())
}

/// What a change of the time.* keys did to the clock.
pub fn show_outcome(time: &str) {
    if time.starts_with("saved,") {
        println!("{}", paint(style::WARN, time));
    } else {
        println!("{}", paint(style::OK, time));
    }
}

/// The one line `device status` shows: `Europe/Bratislava, in sync`.
pub fn summary(time: &TimeSummary) -> String {
    let zone = time.timezone.as_deref().unwrap_or("(unknown timezone)");
    let sync = match (time.ntp, time.synchronized) {
        (Some(false), _) => paint(style::WARN, "NTP off"),
        (_, Some(true)) => paint(style::OK, "in sync"),
        (_, Some(false)) => paint(style::WARN, "not in sync"),
        (_, None) => paint(style::MUTED, "sync unknown"),
    };
    format!("{zone}, {sync}")
}

fn show(status: &TimeStatus) {
    let none = || paint(style::MUTED, "(not reported)");
    let row = style::row;
    if let Some(err) = &status.error {
        println!(
            "{} {err}",
            paint(style::BAD, "not everything could be read:")
        );
        println!();
    }

    let zone = match &status.timezone {
        Some(zone) => {
            let detail = match (&status.zone_abbreviation, status.utc_offset_seconds) {
                (Some(abbreviation), Some(offset)) => {
                    format!(
                        " {}",
                        paint(
                            style::MUTED,
                            format!("({abbreviation}, {})", utc_offset(offset))
                        )
                    )
                }
                _ => String::new(),
            };
            format!("{zone}{detail}")
        }
        None => none(),
    };
    row("timezone", &zone);
    if status
        .timezone
        .as_deref()
        .is_some_and(|zone| zone != status.setting_timezone)
    {
        row(
            "",
            &paint(
                style::WARN,
                format!(
                    "time.timezone is {}, not applied yet",
                    status.setting_timezone
                ),
            ),
        );
    }
    row(
        "local time",
        &status.local_time.clone().unwrap_or_else(none),
    );
    row(
        "in sync",
        &match status.synchronized {
            Some(true) => paint(style::OK, "yes"),
            Some(false) => paint(style::WARN, "no"),
            None => none(),
        },
    );
    row(
        "ntp",
        &match (status.ntp, status.can_ntp) {
            (_, Some(false)) => paint(style::BAD, "not available on this image"),
            (Some(true), _) => paint(style::OK, "on"),
            (Some(false), _) => paint(style::WARN, "off"),
            (None, _) => none(),
        },
    );
    if let Some(state) = &status.timesyncd {
        row("timesyncd", &paint(style::unit_state(state), state));
    }

    if status.timesyncd.as_deref() == Some("active") {
        let server = match (&status.server_name, &status.server_address) {
            (Some(name), Some(address)) if name != address => {
                format!("{name} {}", paint(style::MUTED, format!("({address})")))
            }
            (Some(name), _) => name.clone(),
            (None, Some(address)) => address.clone(),
            (None, None) => paint(style::WARN, "none yet"),
        };
        row("server", &server);
        if let Some(poll) = status.poll_interval_usec {
            let bounds = match (status.poll_interval_min_usec, status.poll_interval_max_usec) {
                (Some(min), Some(max)) => format!(
                    " {}",
                    paint(style::MUTED, format!("({} to {})", span(min), span(max)))
                ),
                _ => String::new(),
            };
            row("poll", &format!("every {}{bounds}", span(poll)));
        }
        match &status.last {
            Some(last) => {
                row("offset", &offset(last.offset_usec));
                row("delay", &span(last.delay_usec.unsigned_abs()));
                row("jitter", &span(last.jitter_usec));
                if let Some(ppm) = status.frequency_ppm() {
                    row(
                        "drift",
                        &format!(
                            "{} {}",
                            drift(ppm),
                            paint(style::MUTED, "(the kernel's frequency correction)")
                        ),
                    );
                }
                let max = status
                    .root_distance_max_usec
                    .map(|max| format!(" {}", paint(style::MUTED, format!("(max {})", span(max)))))
                    .unwrap_or_default();
                row(
                    "root dist.",
                    &format!(
                        "{}{max}",
                        span(last.root_delay_usec / 2 + last.root_dispersion_usec)
                    ),
                );
                row(
                    "stratum",
                    &format!(
                        "{} {}",
                        last.stratum,
                        paint(
                            style::MUTED,
                            format!(
                                "(reference {}, precision {})",
                                last.reference,
                                precision(last.precision)
                            )
                        )
                    ),
                );
                if last.leap != 0 {
                    row("leap", &paint(style::WARN, leap(last.leap)));
                }
                let age = status
                    .now_usec
                    .and_then(|now| now.checked_sub(last.received_usec))
                    .map(|age| format!("{} ago, ", span(age)))
                    .unwrap_or_default();
                let spike = if last.spike {
                    format!(", {}", paint(style::WARN, "the last one was an outlier"))
                } else {
                    String::new()
                };
                row(
                    "answers",
                    &format!("{age}{} so far{spike}", last.packet_count),
                );
            }
            None => row("answers", &paint(style::WARN, "none yet")),
        }
    }

    if let (Some(rtc), Some(now)) = (status.rtc_usec, status.now_usec) {
        let local = if status.local_rtc == Some(true) {
            paint(style::WARN, " (keeps local time)")
        } else {
            String::new()
        };
        row(
            "hw clock",
            &format!("{} from the system clock{local}", offset_between(rtc, now)),
        );
    } else if status.now_usec.is_some() {
        row("hw clock", &paint(style::MUTED, "none"));
    }

    println!("{}", paint(style::HEADING, "servers:"));
    let servers = &status.servers;
    let list = |names: &[String]| {
        if names.is_empty() {
            paint(style::MUTED, "(none)")
        } else {
            names.join(" ")
        }
    };
    for (label, names, from) in [
        (
            "runtime",
            &servers.runtime,
            "DHCP's, while time.ntp.servers is empty",
        ),
        ("system", &servers.system, "time.ntp.servers"),
        ("fallback", &servers.fallback, "the image's"),
        ("dhcp", &servers.dhcp, "what the network offers"),
    ] {
        println!(
            "  {} {} {}",
            pad(style::LABEL, label, 10),
            list(names),
            paint(style::MUTED, format!("({from})"))
        );
    }

    if status.ntp == Some(true) && status.synchronized == Some(false) {
        println!(
            "\n{} {}",
            paint(style::MUTED, "not in sync? name a reachable server:"),
            paint(style::CMD, "tessaro-ctl time ntp on --server HOST")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(text: &str) -> String {
        anstream::adapter::strip_str(text).to_string()
    }

    #[test]
    fn the_status_line_says_zone_and_sync() {
        let summary_of = |ntp, synchronized| {
            plain(&summary(&TimeSummary {
                timezone: Some("Europe/Bratislava".to_string()),
                synchronized,
                ntp,
            }))
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
