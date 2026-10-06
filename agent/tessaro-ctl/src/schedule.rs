//! `tessaro-ctl schedule ...`: systemd `OnCalendar` times at which the
//! device runs one of its scripts (`tessaro-ctl script`).
//!
//! The device keeps the schedules and renders each into a systemd timer;
//! systemd does the timing, so a schedule keeps firing while the agent is
//! down. Times are the device's wall clock.

use crate::out::println;
use clap::{Args, Subcommand};
use protocol::api::{self, CalendarBody, LogsQuery, ScheduleChange, ScheduleRef};
use protocol::{CalendarCheck, Moment, ScheduleInfo, ScheduleSpec};
use tessaro_client::schedule::{self as shared, duration, last_run, now, outcome, relative};

use crate::connect::Session;
use crate::style::{self, pad, paint};
use crate::{done, journal_line, print, prompt};

#[derive(Subcommand)]
pub enum ScheduleCmd {
    /// Every schedule: on or off, the script it runs, when it runs next,
    /// how its last run ended.
    List,
    /// One schedule in full, and the next times it fires.
    Show { schedule: String },
    /// Add a schedule. Its calendar is checked by the device's systemd first.
    ///
    ///   tessaro-ctl schedule create screen-off --on '*-*-* 22:00' --script dim
    ///   tessaro-ctl schedule create weekend --on 'Sat,Sun 08:00' --script weekend-page
    Create {
        /// Lower-case letters, digits and -.
        name: String,
        #[command(flatten)]
        when: When,
        /// Save it switched off.
        #[arg(long)]
        disabled: bool,
    },
    /// Change a schedule, by name or id. A given --on replaces the whole
    /// list.
    ///
    ///   tessaro-ctl schedule set screen-off --on 'Mon..Fri 20:00' --on 'Sat,Sun 23:00'
    ///   tessaro-ctl schedule set screen-off --script dim-slowly
    Set {
        schedule: String,
        /// Rename it.
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        when: When,
    },
    /// Switch a schedule's timer on.
    Enable { schedule: String },
    /// Switch a schedule's timer off; runs already going finish.
    Disable { schedule: String },
    /// Remove a schedule; runs already going finish, its script stays.
    Remove {
        schedule: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Check OnCalendar expressions with the device's systemd, saving
    /// nothing: how it reads them and the next times they fire.
    ///
    ///   tessaro-ctl schedule check 'Mon..Fri 07:00' 'Sat,Sun *:0/15'
    Check {
        #[arg(required = true, value_name = "CALENDAR")]
        calendar: Vec<String>,
        /// How many run times to show.
        #[arg(long, short, default_value_t = 5)]
        count: u32,
    },
    /// What the runs this schedule started wrote, from the device's journal.
    Logs {
        schedule: String,
        #[arg(long, short)]
        follow: bool,
        /// How many lines back to start (`-n` is --node).
        #[arg(long, default_value_t = 100)]
        lines: u32,
    },
}

/// When a schedule fires and what it runs.
#[derive(Args)]
pub struct When {
    /// A systemd OnCalendar expression (`man systemd.time`): `daily`,
    /// `Mon..Fri 07:00`, `*:0/15`. Repeat it; any of them fires.
    #[arg(long = "on", value_name = "CALENDAR")]
    calendar: Vec<String>,
    /// The script it runs, by name or id (`tessaro-ctl script list`).
    #[arg(long, value_name = "SCRIPT")]
    script: Option<String>,
}

pub fn run(session: &mut Session, command: ScheduleCmd, json: bool) -> Result<(), String> {
    match command {
        ScheduleCmd::List => {
            let schedules = session.fetch::<api::schedule::List>()?;
            print(json, &schedules, || list(&schedules))
        }
        ScheduleCmd::Show { schedule } => {
            let info = find(session, &schedule)?;
            let check = upcoming(session, &info);
            if json {
                return print(json, &info, || {});
            }
            show(&info, check.as_ref());
            Ok(())
        }
        ScheduleCmd::Create {
            name,
            when,
            disabled,
        } => {
            let Some(script) = when.script else {
                return Err(
                    "a schedule runs a script: --script NAME (`tessaro-ctl script list`)"
                        .to_string(),
                );
            };
            let spec = ScheduleSpec {
                name,
                enabled: !disabled,
                calendar: when.calendar,
                script,
            };
            let info = session.send::<api::schedule::Create>(spec)?;
            print(json, &info, || {
                println!(
                    "{}",
                    paint(style::OK, format!("created {}", info.spec.name))
                );
                show(&info, None);
            })
        }
        ScheduleCmd::Set {
            schedule,
            name,
            when,
        } => {
            let calendar = (!when.calendar.is_empty()).then_some(when.calendar);
            let change = ScheduleChange {
                name,
                calendar,
                script: when.script,
                enabled: None,
            };
            changed(session, schedule, change, json)
        }
        ScheduleCmd::Enable { schedule } => changed(session, schedule, enabled(true), json),
        ScheduleCmd::Disable { schedule } => changed(session, schedule, enabled(false), json),
        ScheduleCmd::Remove { schedule, yes } => {
            prompt::confirm(yes, &format!("Remove schedule {schedule}?"))?;
            done::<api::schedule::Remove>(session, ScheduleRef { schedule }, (), json)
        }
        ScheduleCmd::Check { calendar, count } => {
            let check = session.send::<api::schedule::Check>(CalendarBody {
                calendar,
                count: Some(count),
            })?;
            print(json, &check, || {
                for form in &check.normalized {
                    style::row("reads as", form);
                }
                show_upcoming(&check, "fires");
            })
        }
        ScheduleCmd::Logs {
            schedule,
            follow,
            lines,
        } => {
            let info = find(session, &schedule)?;
            journal(session, info.units, lines, follow, json)
        }
    }
}

/// `enable` and `disable` are one change of `enabled`.
fn enabled(enabled: bool) -> ScheduleChange {
    ScheduleChange {
        enabled: Some(enabled),
        ..ScheduleChange::default()
    }
}

fn changed(
    session: &mut Session,
    schedule: String,
    change: ScheduleChange,
    json: bool,
) -> Result<(), String> {
    let info = session.call::<api::schedule::Change>(ScheduleRef { schedule }, change)?;
    print(json, &info, || {
        println!(
            "{}",
            paint(style::OK, format!("changed {}", info.spec.name))
        );
        show(&info, None);
    })
}

/// The schedule `query` names, by name or id.
fn find(session: &mut Session, query: &str) -> Result<ScheduleInfo, String> {
    let schedules = session.fetch::<api::schedule::List>()?;
    schedules
        .into_iter()
        .find(|info| info.id == query || info.spec.name == query)
        .ok_or_else(|| format!("no schedule {query:?}; `tessaro-ctl schedule list` shows them"))
}

/// The next times `info` fires; `None` if the device cannot say.
fn upcoming(session: &mut Session, info: &ScheduleInfo) -> Option<CalendarCheck> {
    session
        .send::<api::schedule::Check>(CalendarBody {
            calendar: info.spec.calendar.clone(),
            count: Some(5),
        })
        .ok()
}

fn list(schedules: &[ScheduleInfo]) {
    if schedules.is_empty() {
        println!(
            "{} {}",
            paint(style::MUTED, "no schedules; add one with"),
            paint(style::CMD, "tessaro-ctl schedule create")
        );
        return;
    }
    let now = now();
    println!(
        "{} {} {} {} {}",
        pad(style::HEADING, "name", 20),
        pad(style::HEADING, "state", 8),
        pad(style::HEADING, "script", 20),
        pad(style::HEADING, "next", 18),
        paint(style::HEADING, "last run")
    );
    for info in schedules {
        let state = if info.spec.enabled {
            pad(style::OK, "on", 8)
        } else {
            pad(style::MUTED, "off", 8)
        };
        let next = info
            .next
            .as_ref()
            .map_or_else(|| "-".to_string(), |next| relative(next.unix, now));
        let last = style::line(&last_run(info, now));
        println!(
            "{} {state} {} {} {last}",
            pad(style::HEADING, &info.spec.name, 20),
            pad(anstyle::Style::new(), &info.script_name, 20),
            pad(anstyle::Style::new(), next, 18),
        );
    }
}

fn show(info: &ScheduleInfo, check: Option<&CalendarCheck>) {
    let now = now();
    let spec = &info.spec;
    style::row("name", &paint(style::HEADING, &spec.name));
    style::row("id", &paint(style::MUTED, &info.id));
    style::row(
        "state",
        &if spec.enabled {
            paint(style::OK, "on")
        } else {
            paint(style::MUTED, "off")
        },
    );
    for (at, expression) in spec.calendar.iter().enumerate() {
        style::row(if at == 0 { "on" } else { "" }, expression);
    }
    style::row("runs", &info.script_name);
    match &info.next {
        Some(next) => style::row("next", &moment(next, now)),
        None if spec.enabled => style::row("next", &paint(style::MUTED, "not scheduled")),
        None => style::row("next", &paint(style::MUTED, "off")),
    }
    if let Some(last) = &info.last_trigger {
        style::row("fired", &moment(last, now));
    }
    match &info.last_run {
        Some(run) => {
            let result = if run.succeeded() {
                paint(style::OK, outcome(run))
            } else {
                paint(style::BAD, outcome(run))
            };
            style::row(
                "last run",
                &format!(
                    "{result} {}",
                    paint(
                        style::MUTED,
                        format!(
                            "started {}, took {}",
                            moment(&run.started, now),
                            duration((run.finished.unix - run.started.unix).unsigned_abs())
                        )
                    )
                ),
            );
        }
        None => style::row("last run", &paint(style::MUTED, "never")),
    }
    if info.running > 0 {
        style::row("running", &paint(style::WARN, info.running));
    }
    if let Some(check) = check {
        show_upcoming(check, if spec.enabled { "fires" } else { "when on" });
    }
    println!(
        "\n{} {}   {} {}",
        paint(style::MUTED, "output:"),
        paint(
            style::CMD,
            format!("tessaro-ctl schedule logs {}", spec.name)
        ),
        paint(style::MUTED, "run it now:"),
        paint(
            style::CMD,
            format!("tessaro-ctl script run {}", info.script_name)
        )
    );
}

fn show_upcoming(check: &CalendarCheck, label: &str) {
    style::facts(&shared::upcoming(check, label));
}

/// `2026-09-28 07:00:00 CEST (in 1 day 15h)`.
fn moment(at: &Moment, now: i64) -> String {
    style::line(&shared::moment(at, now))
}

/// The journal of `units` for `logs`, printed as it comes.
pub fn journal(
    session: &mut Session,
    units: String,
    lines: u32,
    follow: bool,
    json: bool,
) -> Result<(), String> {
    session.logs(
        LogsQuery {
            unit: Some(units),
            lines: Some(lines),
            cursor: None,
        },
        follow,
        // Following ends with Ctrl-C, which ends the process.
        &|| false,
        |event| {
            if json {
                println!("{event}");
            } else {
                println!("{}", journal_line(&event));
            }
        },
    )
}
