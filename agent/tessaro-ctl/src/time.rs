//! `tessaro-ctl time ...`: the timezone, NTP servers and sync, and setting
//! the clock by hand.
//!
//! `timezone` and `ntp` are a `config set` of the time.* keys: the device
//! applies them to the running clock at once, and restarts only
//! systemd-timesyncd, only when its servers change. `show` prints what
//! timedated and timesyncd report - offset, delay, jitter, drift - and
//! nothing the device measured on its own. What it says is
//! `tessaro_client::describe::time`, shared with the GUI's Time page.

use std::collections::BTreeMap;

use anstream::println;
use clap::Subcommand;
use protocol::api;
use protocol::{keys, TimeStatus};
use tessaro_client::describe::time as describe;

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
            let values = tessaro_client::actions::ntp_change(state == Toggle::On, &servers)
                .map_err(|_| "--server goes with `tessaro-ctl time ntp on`".to_string())?;
            set(session, json, values)
        }
        TimeCmd::Sync => {
            let done = session.send::<api::time::Sync>(())?;
            print(json, &done, || {
                println!("{}", paint(style::OK, &done.message))
            })
        }
        TimeCmd::Set { time } => {
            let body = tessaro_client::actions::set_clock(time.as_deref())?;
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

fn show(status: &TimeStatus) {
    if let Some(error) = describe::error(status) {
        println!("{}", style::line(&error));
        println!();
    }
    style::facts(&describe::facts(status));
    println!("{}", paint(style::HEADING, "servers:"));
    for fact in describe::servers(status) {
        println!(
            "  {} {}",
            pad(style::LABEL, &fact.label, 10),
            style::line(&fact.value)
        );
    }
    if let Some(hint) = describe::hint(status) {
        println!("\n{}", style::line(&hint));
    }
}
