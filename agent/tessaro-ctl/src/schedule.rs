//! `tessaro-ctl schedule ...`: shell command lines the device runs on
//! systemd `OnCalendar` times.
//!
//! The device keeps the schedules and renders each into a systemd timer;
//! systemd does the timing, so a schedule keeps firing while the agent is
//! down. Every line runs with `/bin/sh -c` as root, so a `tessaro-ctl`
//! command is written out in full. Times are the device's wall clock.

use std::io::Read;
use std::path::PathBuf;

use anstream::println;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Subcommand};
use protocol::{CalendarCheck, Command, Done, Moment, OnError, ScheduleInfo, ScheduleSpec};
use tessaro_client::schedule::{duration, now, outcome, parse_timeout, relative};

use crate::connect::Session;
use crate::style::{self, pad, paint};
use crate::{journal_line, print, prompt};

#[derive(Subcommand)]
pub enum ScheduleCmd {
    /// Every schedule: on or off, when it runs next, how its last run ended.
    List,
    /// One schedule in full, and the next times it fires.
    Show { schedule: String },
    /// Add a schedule. Its calendar is checked by the device's systemd first.
    ///
    ///   tessaro-ctl schedule create screen-off --on '*-*-* 22:00' \
    ///       --run 'tessaro-ctl screen power off'
    ///   tessaro-ctl schedule create weekend --on 'Sat,Sun 08:00' \
    ///       --run 'tessaro-ctl config set browser.url=https://example.com/weekend'
    ///   tessaro-ctl schedule create cleanup --on daily --run-file cleanup.txt \
    ///       --on-error continue --timeout 10m
    Create {
        /// Lower-case letters, digits and -.
        name: String,
        #[command(flatten)]
        body: Body,
        /// Save it switched off.
        #[arg(long)]
        disabled: bool,
    },
    /// Change a schedule, by name or id. A given --on or --run replaces
    /// the whole list.
    ///
    ///   tessaro-ctl schedule set screen-off --on 'Mon..Fri 20:00' --on 'Sat,Sun 23:00'
    ///   tessaro-ctl schedule set cleanup --timeout none
    Set {
        schedule: String,
        /// Rename it.
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        body: Body,
    },
    /// Switch a schedule's timer on.
    Enable { schedule: String },
    /// Switch a schedule's timer off; runs already going finish.
    Disable { schedule: String },
    /// Start one run now, enabled or not.
    Run { schedule: String },
    /// Remove a schedule; runs already going finish.
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
    /// What a schedule's runs wrote, from the device's journal.
    Logs {
        schedule: String,
        #[arg(long, short)]
        follow: bool,
        /// How many lines back to start (`-n` is --node).
        #[arg(long, default_value_t = 100)]
        lines: u32,
    },
}

/// When a schedule fires and what a run does.
#[derive(Args)]
pub struct Body {
    /// A systemd OnCalendar expression (`man systemd.time`): `daily`,
    /// `Mon..Fri 07:00`, `*:0/15`. Repeat it; any of them fires.
    #[arg(long = "on", value_name = "CALENDAR")]
    calendar: Vec<String>,
    /// A shell command line, run as root; repeat it for more, run in order.
    #[arg(long = "run", value_name = "LINE")]
    lines: Vec<String>,
    /// Command lines from FILE, one per line, after any --run; `-` reads
    /// stdin. Blank lines and lines starting with # are skipped.
    #[arg(long = "run-file", value_name = "FILE")]
    run_file: Option<PathBuf>,
    /// When a line fails: stop the run there, or continue with the next.
    #[arg(long = "on-error", value_name = "WHAT",
        value_parser = PossibleValuesParser::new(OnError::NAMES)
            .map(|name| name.parse::<OnError>().expect("one of the names")))]
    on_error: Option<OnError>,
    /// The longest a whole run may take before it is killed: 90s, 10m, 2h;
    /// `none` for no limit.
    #[arg(long, value_name = "DURATION", value_parser = parse_timeout)]
    timeout: Option<u64>,
}

impl Body {
    /// The command lines, or `None` when neither --run nor --run-file was
    /// given.
    fn lines(&self) -> Result<Option<Vec<String>>, String> {
        let mut lines = self.lines.clone();
        if let Some(path) = &self.run_file {
            let text = if path.as_os_str() == "-" {
                let mut text = String::new();
                std::io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|err| format!("stdin: {err}"))?;
                text
            } else {
                std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?
            };
            lines.extend(
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .map(str::to_string),
            );
        } else if lines.is_empty() {
            return Ok(None);
        }
        Ok(Some(lines))
    }
}

pub fn run(session: &mut Session, command: ScheduleCmd, json: bool) -> Result<(), String> {
    match command {
        ScheduleCmd::List => {
            let schedules: Vec<ScheduleInfo> = session.call(Command::ScheduleList)?;
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
            body,
            disabled,
        } => {
            let spec = ScheduleSpec {
                name,
                enabled: !disabled,
                lines: body.lines()?.unwrap_or_default(),
                calendar: body.calendar,
                on_error: body.on_error.unwrap_or_default(),
                timeout_s: body.timeout.filter(|seconds| *seconds > 0),
            };
            let info: ScheduleInfo = session.call(Command::ScheduleCreate { spec })?;
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
            body,
        } => {
            let lines = body.lines()?;
            let calendar = (!body.calendar.is_empty()).then_some(body.calendar);
            let command = Command::ScheduleSet {
                schedule,
                name,
                calendar,
                lines,
                on_error: body.on_error,
                timeout_s: body.timeout,
                enabled: None,
            };
            changed(session, command, json)
        }
        ScheduleCmd::Enable { schedule } => changed(session, enabled(schedule, true), json),
        ScheduleCmd::Disable { schedule } => changed(session, enabled(schedule, false), json),
        ScheduleCmd::Run { schedule } => {
            let done: Done = session.call(Command::ScheduleRun { schedule })?;
            print(json, &done, || println!("{}", done.message))
        }
        ScheduleCmd::Remove { schedule, yes } => {
            prompt::confirm(yes, &format!("Remove schedule {schedule}?"))?;
            let done: Done = session.call(Command::ScheduleRemove { schedule })?;
            print(json, &done, || println!("{}", done.message))
        }
        ScheduleCmd::Check { calendar, count } => {
            let check: CalendarCheck = session.call(Command::ScheduleCheck {
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
            session.stream(
                Command::Logs {
                    follow,
                    unit: Some(info.units),
                    lines: Some(lines),
                },
                |event| {
                    if json {
                        println!("{event}");
                    } else {
                        println!("{}", journal_line(&event));
                    }
                },
            )
        }
    }
}

/// `enable` and `disable` are one `schedule-set` of `enabled`.
fn enabled(schedule: String, enabled: bool) -> Command {
    Command::ScheduleSet {
        schedule,
        name: None,
        calendar: None,
        lines: None,
        on_error: None,
        timeout_s: None,
        enabled: Some(enabled),
    }
}

fn changed(session: &mut Session, command: Command, json: bool) -> Result<(), String> {
    let info: ScheduleInfo = session.call(command)?;
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
    let schedules: Vec<ScheduleInfo> = session.call(Command::ScheduleList)?;
    schedules
        .into_iter()
        .find(|info| info.id == query || info.spec.name == query)
        .ok_or_else(|| format!("no schedule {query:?}; `tessaro-ctl schedule list` shows them"))
}

/// The next times `info` fires; `None` if the device cannot say.
fn upcoming(session: &mut Session, info: &ScheduleInfo) -> Option<CalendarCheck> {
    session
        .call(Command::ScheduleCheck {
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
        "{} {} {} {}",
        pad(style::HEADING, "name", 20),
        pad(style::HEADING, "state", 8),
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
        let mut last = match &info.last_run {
            Some(run) if run.succeeded() => paint(
                style::OK,
                format!("{}, {}", outcome(run), relative(run.finished.unix, now)),
            ),
            Some(run) => paint(
                style::BAD,
                format!("{}, {}", outcome(run), relative(run.finished.unix, now)),
            ),
            None => paint(style::MUTED, "never"),
        };
        if info.running > 0 {
            last.push_str(&paint(style::WARN, format!(" ({} running)", info.running)));
        }
        println!(
            "{} {state} {} {last}",
            pad(style::HEADING, &info.spec.name, 20),
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
    for (at, line) in spec.lines.iter().enumerate() {
        style::row(
            if at == 0 { "runs" } else { "" },
            &format!("{} {line}", paint(style::MUTED, format!("{}.", at + 1))),
        );
    }
    style::row("on error", spec.on_error.name());
    style::row(
        "timeout",
        &spec
            .timeout_s
            .map_or_else(|| paint(style::MUTED, "none"), duration),
    );
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
        "\n{} {}",
        paint(style::MUTED, "output:"),
        paint(
            style::CMD,
            format!("tessaro-ctl schedule logs {}", spec.name)
        )
    );
}

fn show_upcoming(check: &CalendarCheck, label: &str) {
    let now = now();
    if check.next.is_empty() {
        style::row(label, &paint(style::WARN, "never again"));
    }
    for (at, next) in check.next.iter().enumerate() {
        style::row(if at == 0 { label } else { "" }, &moment(next, now));
    }
}

/// `2026-09-28 07:00:00 CEST (in 1 day 15h)`.
fn moment(moment: &Moment, now: i64) -> String {
    let local = if moment.local.is_empty() {
        moment.unix.to_string()
    } else {
        moment.local.clone()
    };
    format!(
        "{local} {}",
        paint(style::MUTED, format!("({})", relative(moment.unix, now)))
    )
}
