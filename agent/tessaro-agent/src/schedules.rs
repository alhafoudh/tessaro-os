//! Schedules: shell command lines systemd runs on `OnCalendar` times.
//!
//! `schedules.json` is the only truth. Each schedule is rendered into
//! systemd units in `/run/systemd/system` (`Paths::systemd_unit_dir`), and
//! systemd does all the timing: the calendar, the timezone, clock jumps.
//! The agent only keeps the units matching the file (`control/schedules.rs`
//! reconciles), so a schedule keeps firing while the agent is down.
//!
//! Per schedule `<id>`, with `<hash>` naming its content:
//!
//! * `tessaro-schedule-<id>.timer`: one `OnCalendar=` per expression,
//!   `AccuracySec=1s` (the default minute would move every run), no
//!   `Persistent=`, so a time missed while the device was off is skipped.
//! * `tessaro-schedule-<id>.service`: what the timer starts. It only starts
//!   a fresh instance of the run template and exits at once, so the timer
//!   never finds it still running: that is what lets runs overlap instead of
//!   systemd skipping a trigger.
//! * `tessaro-schedule-<id>-<hash>@.service`: the run. One `ExecStart=` per
//!   line (with `-` when failures continue), and an `ExecStopPost=` that
//!   writes how the run ended to `schedule-runs/<id>`. The hash is in the
//!   name so an edit never changes the lines under a running instance: the
//!   old template stays until its last run ends.
//!
//! Everything here is pure or blocking file work; the bus calls are in
//! `control/schedules.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use protocol::{
    Moment, OnError, ScheduleSpec, SCHEDULE_CALENDAR_MAX, SCHEDULE_LINES_MAX, SCHEDULE_LINE_MAX,
};
use serde::{Deserialize, Serialize};

use crate::store;

pub const FILE: &str = "schedules.json";

/// Every unit and journal pattern of a schedule starts with this.
pub const PREFIX: &str = "tessaro-schedule-";

/// Stands in for systemd's `%i` while a command is escaped, which would
/// double its `%`.
const INSTANCE: &str = "@@INSTANCE@@";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Schedules {
    #[serde(default)]
    pub schedules: Vec<Schedule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    #[serde(flatten)]
    pub spec: ScheduleSpec,
}

/// `spec` made ready to save: trimmed, checked, `timeout_s: Some(0)` as
/// none. `others` are the schedules it must not share a name with.
pub fn validate(mut spec: ScheduleSpec, others: &[&Schedule]) -> Result<ScheduleSpec, String> {
    spec.name = spec.name.trim().to_string();
    check_name(&spec.name)?;
    if others.iter().any(|other| other.spec.name == spec.name) {
        return Err(format!("a schedule named {} exists already", spec.name));
    }

    spec.calendar = trimmed(spec.calendar);
    if spec.calendar.is_empty() {
        return Err("a schedule needs at least one calendar expression".to_string());
    }
    if spec.calendar.len() > SCHEDULE_CALENDAR_MAX {
        return Err(format!(
            "a schedule takes at most {SCHEDULE_CALENDAR_MAX} calendar expressions"
        ));
    }
    for expression in &spec.calendar {
        check_text("calendar expression", expression)?;
        // Nothing in the OnCalendar syntax uses it, and in a unit file it
        // starts a specifier.
        if expression.contains('%') {
            return Err(format!("{expression:?}: a calendar expression has no %"));
        }
    }

    spec.lines = trimmed(spec.lines);
    if spec.lines.is_empty() {
        return Err("a schedule needs at least one command line".to_string());
    }
    if spec.lines.len() > SCHEDULE_LINES_MAX {
        return Err(format!(
            "a schedule takes at most {SCHEDULE_LINES_MAX} command lines"
        ));
    }
    for line in &spec.lines {
        check_text("command line", line)?;
    }

    if spec.timeout_s == Some(0) {
        spec.timeout_s = None;
    }
    Ok(spec)
}

fn check_name(name: &str) -> Result<(), String> {
    let valid = name.len() <= 40
        && name.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{name:?} is not a schedule name: lower-case letters, digits and -, \
             starting with a letter or digit, at most 40"
        ))
    }
}

fn check_text(what: &str, text: &str) -> Result<(), String> {
    if text.len() > SCHEDULE_LINE_MAX {
        return Err(format!(
            "a {what} is at most {SCHEDULE_LINE_MAX} bytes long"
        ));
    }
    // One line is one command; a tab is still text.
    if text.chars().any(|ch| ch.is_control() && ch != '\t') {
        return Err(format!(
            "{text:?}: a {what} has no newlines or other control characters"
        ));
    }
    Ok(())
}

fn trimmed(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// The schedule `query` names: its id or its exact name.
pub fn find(schedules: &[Schedule], query: &str) -> Result<usize, String> {
    schedules
        .iter()
        .position(|schedule| schedule.id == query || schedule.spec.name == query)
        .ok_or_else(|| format!("no schedule {query:?}; `tessaro-ctl schedule list` shows them"))
}

/// A fresh id: 8 hex characters no other schedule has.
pub fn new_id(schedules: &[Schedule]) -> Result<String, String> {
    loop {
        let id = protocol::hex(&crate::auth::random(4)?);
        if !schedules.iter().any(|schedule| schedule.id == id) {
            return Ok(id);
        }
    }
}

/// 8 hex characters of SHA-256 over everything a run is made of: a change to
/// any of it is a new run template.
pub fn content_hash(spec: &ScheduleSpec) -> String {
    let content = serde_json::json!([
        spec.name,
        spec.calendar,
        spec.lines,
        spec.on_error,
        spec.timeout_s
    ]);
    protocol::hex(&openssl::sha::sha256(content.to_string().as_bytes()))[..8].to_string()
}

pub fn timer_unit(id: &str) -> String {
    format!("{PREFIX}{id}.timer")
}

/// What the timer starts, and `schedule run` too.
pub fn fire_unit(id: &str) -> String {
    format!("{PREFIX}{id}.service")
}

fn template_unit(id: &str, hash: &str) -> String {
    format!("{PREFIX}{id}-{hash}@.service")
}

/// Every run of schedule `id`, whichever template it came from: a pattern
/// for `ListUnitsByPatterns` and `journalctl --unit`.
pub fn runs_pattern(id: &str) -> String {
    format!("{PREFIX}{id}-*@*.service")
}

/// The template an instance was started from:
/// `tessaro-schedule-ab12-cd34@1700000000-42.service` is
/// `tessaro-schedule-ab12-cd34@.service`.
pub fn template_of(instance: &str) -> Option<String> {
    let (template, rest) = instance.split_once('@')?;
    (template.starts_with(PREFIX) && rest.ends_with(".service") && !rest.is_empty())
        .then(|| format!("{template}@.service"))
}

/// `text` as one argument of an `ExecStart=` line: in double quotes, with
/// what systemd would otherwise interpret escaped. Backslashes are C
/// escapes inside the quotes, `%` starts a specifier and `$` an environment
/// variable.
pub fn exec_arg(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '$' => quoted.push_str("$$"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

const HEADER: &str = "# Written by tessaro-agent from schedules.json at every start and change;\n\
                      # edits here do not last. See docs/scheduler.md.\n";

/// The unit files schedule `schedule` is made of, by name.
pub fn render(schedule: &Schedule, runs_dir: &Path) -> Vec<(String, String)> {
    let id = &schedule.id;
    let spec = &schedule.spec;
    let hash = content_hash(spec);
    let template = template_unit(id, &hash);
    let instance_prefix = template.trim_end_matches("@.service");

    let mut timer = format!(
        "{HEADER}[Unit]\nDescription=tessaro schedule {}\n\n[Timer]\n",
        spec.name
    );
    for expression in &spec.calendar {
        timer.push_str(&format!("OnCalendar={expression}\n"));
    }
    timer.push_str(&format!("AccuracySec=1s\nUnit={}\n", fire_unit(id)));

    // The instance is the start time and the shell's pid: unique, and it
    // says when the run started. `$$`/`%%` are systemd's escapes.
    let fire = format!(
        "{HEADER}[Unit]\nDescription=tessaro schedule {} (start a run)\n\n\
         [Service]\nType=oneshot\n\
         ExecStart=/bin/sh -c \"exec systemctl start --no-block {instance_prefix}@$$(date +%%s)-$$$$.service\"\n",
        spec.name
    );

    let timeout = spec
        .timeout_s
        .map_or_else(|| "infinity".to_string(), |seconds| format!("{seconds}s"));
    let mut run = format!(
        "{HEADER}[Unit]\nDescription=tessaro schedule {} (run)\n\
         CollectMode=inactive-or-failed\n\n\
         [Service]\nType=oneshot\nTimeoutStartSec={timeout}\n\
         SyslogIdentifier={PREFIX}{}\n",
        spec.name, spec.name
    );
    let ignore = match spec.on_error {
        OnError::Stop => "",
        OnError::Continue => "-",
    };
    for line in &spec.lines {
        run.push_str(&format!(
            "ExecStart={ignore}/bin/sh -c {}\n",
            exec_arg(line)
        ));
    }
    run.push_str(&format!(
        "ExecStopPost=/bin/sh -c {}\n",
        exec_arg(&record_script(runs_dir, id)).replace(INSTANCE, "%i")
    ));

    vec![
        (timer_unit(id), timer),
        (fire_unit(id), fire),
        (template, run),
    ]
}

/// What `ExecStopPost=` runs: `<instance> <finished> <result> <status>`
/// into `schedule-runs/<id>`, through a temporary so a reader never sees
/// half a line, and one per run, since runs overlap.
fn record_script(runs_dir: &Path, id: &str) -> String {
    let dir = runs_dir.display();
    format!(
        "mkdir -p '{dir}' && \
         printf '%s %s %s %s\\n' \"{INSTANCE}\" \"$(date +%s)\" \"$SERVICE_RESULT\" \"$EXIT_STATUS\" \
         > '{dir}/.{id}.'$$ && mv -f '{dir}/.{id}.'$$ '{dir}/{id}'"
    )
}

/// Every unit file the schedules are made of.
pub fn wanted(schedules: &[Schedule], runs_dir: &Path) -> BTreeMap<String, String> {
    schedules
        .iter()
        .flat_map(|schedule| render(schedule, runs_dir))
        .collect()
}

/// What to do to the unit directory.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub write: Vec<(String, String)>,
    pub remove: Vec<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.write.is_empty() && self.remove.is_empty()
    }
}

/// From the unit files `present` to `wanted`: what differs is written, what
/// is not wanted goes, except a run template in `busy` (one with runs going),
/// which goes once they end.
pub fn plan(
    wanted: &BTreeMap<String, String>,
    present: &BTreeMap<String, Vec<u8>>,
    busy: &BTreeSet<String>,
) -> Plan {
    let write = wanted
        .iter()
        .filter(|(name, body)| present.get(*name).map(Vec::as_slice) != Some(body.as_bytes()))
        .map(|(name, body)| (name.clone(), body.clone()))
        .collect();
    let remove = present
        .keys()
        .filter(|name| !wanted.contains_key(*name) && !busy.contains(*name))
        .cloned()
        .collect();
    Plan { write, remove }
}

/// The schedule unit files in `dir`, with their content.
pub fn present(dir: &Path) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => return Err(err),
    };
    let mut units = BTreeMap::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with(PREFIX) {
            units.insert(name, fs::read(entry.path())?);
        }
    }
    Ok(units)
}

pub fn apply(dir: &Path, plan: &Plan) -> io::Result<()> {
    for (name, body) in &plan.write {
        store::replace_if_changed(&dir.join(name), body.as_bytes(), 0o644)?;
    }
    for name in &plan.remove {
        match fs::remove_file(dir.join(name)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(())
}

/// How a run ended, as its `ExecStopPost=` wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub started: i64,
    pub finished: i64,
    pub result: String,
    pub status: String,
}

/// `1700000000-42 1700000012 exit-code 1`; `None` for anything else.
pub fn parse_ended(text: &str) -> Option<Ended> {
    let mut fields = text.trim_end_matches('\n').splitn(4, ' ');
    let instance = fields.next()?;
    let finished = fields.next()?.parse().ok()?;
    let result = fields.next()?.to_string();
    let status = fields.next().unwrap_or("").to_string();
    let started = instance.split('-').next()?.parse().ok()?;
    Some(Ended {
        started,
        finished,
        result,
        status,
    })
}

pub fn read_ended(runs_dir: &Path, id: &str) -> Option<Ended> {
    parse_ended(&fs::read_to_string(runs_dir.join(id)).ok()?)
}

/// Forget schedule `id`'s last run.
pub fn forget_ended(runs_dir: &Path, id: &str) -> io::Result<()> {
    match fs::remove_file(runs_dir.join(id)) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Every schedule and every last run gone: a factory reset. The units
/// follow at the next reconcile, or with `/run` at the next boot.
pub fn clear(store: &store::Store, runs_dir: &Path) -> io::Result<()> {
    store.remove()?;
    match fs::remove_dir_all(runs_dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// `unix` on the device's wall clock. Reads `/etc/localtime`: call it from
/// `blocking`.
pub fn moment(unix: i64) -> Moment {
    let local = u64::try_from(unix)
        .ok()
        .and_then(|seconds| crate::time::local_clock(seconds * 1_000_000))
        .map(|local| {
            format!("{} {}", local.text, local.abbreviation)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    Moment { unix, local }
}

/// What `systemd-analyze calendar --iterations=N EXPR` printed: the
/// normalized form and the times it fires, as seconds since the epoch.
///
/// systemd prints each time on the device's clock, followed by an
/// `(in UTC):` line unless the device's zone is UTC, in which case the
/// times themselves are UTC. Only UTC is parsed, so no zone abbreviation
/// ever has to be understood. A calendar that never fires again prints
/// `Next elapse: never`.
pub fn parse_calendar(output: &str) -> Result<(String, Vec<i64>), String> {
    let mut normalized = None;
    let mut local = Vec::new();
    let mut utc = Vec::new();
    for line in output.lines() {
        let Some((label, value)) = line.split_once(": ") else {
            continue;
        };
        let label = label.trim();
        let value = value.trim();
        if label == "Normalized form" {
            normalized = Some(value.to_string());
        } else if label == "Next elapse" || label.starts_with("Iter") {
            local.push(value);
        } else if label == "(in UTC)" {
            utc.push(value);
        }
    }
    let normalized =
        normalized.ok_or_else(|| "systemd-analyze printed no normalized form".to_string())?;
    let times = if utc.is_empty() { local } else { utc };
    let times = times
        .into_iter()
        .filter(|value| *value != "never")
        .map(|value| {
            timestamp(value).ok_or_else(|| format!("systemd-analyze printed a time {value:?}"))
        })
        .collect::<Result<_, _>>()?;
    Ok((normalized, times))
}

/// `Mon 2026-09-28 05:00:00 UTC` as seconds since the epoch.
fn timestamp(value: &str) -> Option<i64> {
    let mut words = value.split_whitespace();
    let _weekday = words.next()?;
    let date: Vec<i64> = words
        .next()?
        .split('-')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let time: Vec<i64> = words
        .next()?
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    let ([year, month, day], [hour, minute, second]) = (date.as_slice(), time.as_slice()) else {
        return None;
    };
    Some(days_from_civil(*year, *month, *day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> ScheduleSpec {
        ScheduleSpec {
            name: name.to_string(),
            enabled: true,
            calendar: vec!["Mon..Fri 07:00".to_string()],
            lines: vec!["tessaro-ctl screen power on".to_string()],
            on_error: OnError::Stop,
            timeout_s: None,
        }
    }

    fn schedule(id: &str, name: &str) -> Schedule {
        Schedule {
            id: id.to_string(),
            spec: spec(name),
        }
    }

    #[test]
    fn a_spec_is_trimmed_and_checked() {
        let mut input = spec(" morning ");
        input.calendar = vec![" Mon..Fri 07:00 ".into(), "  ".into()];
        input.lines.push(String::new());
        input.timeout_s = Some(0);
        let valid = validate(input, &[]).unwrap();
        assert_eq!(valid.name, "morning");
        assert_eq!(valid.calendar, ["Mon..Fri 07:00"]);
        assert_eq!(valid.lines, ["tessaro-ctl screen power on"]);
        assert_eq!(valid.timeout_s, None);

        assert!(validate(spec("Morning"), &[]).is_err());
        assert!(validate(spec("-morning"), &[]).is_err());
        assert!(validate(spec("mor ning"), &[]).is_err());
        let taken = schedule("aa", "morning");
        assert!(validate(spec("morning"), &[&taken]).is_err());

        let mut empty = spec("x");
        empty.lines.clear();
        assert!(validate(empty, &[]).is_err());
        let mut no_calendar = spec("x");
        no_calendar.calendar = vec![" ".into()];
        assert!(validate(no_calendar, &[]).is_err());
        let mut newline = spec("x");
        newline.lines = vec!["echo a\necho b".into()];
        assert!(validate(newline, &[]).is_err());
        let mut percent = spec("x");
        percent.calendar = vec!["%H".into()];
        assert!(validate(percent, &[]).is_err());
        let mut long = spec("x");
        long.lines = vec!["x".repeat(SCHEDULE_LINE_MAX + 1)];
        assert!(validate(long, &[]).is_err());

        let mut shell = spec("x");
        shell.lines = vec!["echo \"$HOME\" 100% \\ 'q' | tee /tmp/a\t; true".into()];
        assert!(validate(shell, &[]).is_ok());
    }

    #[test]
    fn a_schedule_is_found_by_id_or_name() {
        let all = [
            schedule("ab12cd34", "morning"),
            schedule("ef56ab78", "night"),
        ];
        assert_eq!(find(&all, "night"), Ok(1));
        assert_eq!(find(&all, "ab12cd34"), Ok(0));
        assert!(find(&all, "ab12").is_err());
    }

    #[test]
    fn exec_arg_escapes_what_systemd_would_interpret() {
        assert_eq!(exec_arg("echo hi"), "\"echo hi\"");
        assert_eq!(
            exec_arg(r#"echo "$HOME" 100% \n"#),
            r#""echo \"$$HOME\" 100%% \\n""#
        );
    }

    #[test]
    fn a_schedule_renders_its_timer_fire_and_run_units() {
        let mut spec = spec("morning");
        spec.calendar.push("Sat,Sun 09:00 Europe/Bratislava".into());
        spec.lines.push("echo \"it's $HOME\"".into());
        spec.on_error = OnError::Continue;
        spec.timeout_s = Some(600);
        let schedule = Schedule {
            id: "ab12cd34".into(),
            spec,
        };
        let units = render(&schedule, Path::new("/data/tessaro/schedule-runs"));
        let hash = content_hash(&schedule.spec);
        let names: Vec<&str> = units.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "tessaro-schedule-ab12cd34.timer".to_string(),
                "tessaro-schedule-ab12cd34.service".to_string(),
                format!("tessaro-schedule-ab12cd34-{hash}@.service"),
            ]
        );

        let timer = &units[0].1;
        assert!(timer
            .contains("OnCalendar=Mon..Fri 07:00\nOnCalendar=Sat,Sun 09:00 Europe/Bratislava\n"));
        assert!(timer.contains("AccuracySec=1s\n"));
        assert!(timer.contains("Unit=tessaro-schedule-ab12cd34.service\n"));
        assert!(!timer.contains("Persistent"));

        let fire = &units[1].1;
        assert!(fire.contains(&format!(
            "ExecStart=/bin/sh -c \"exec systemctl start --no-block \
             tessaro-schedule-ab12cd34-{hash}@$$(date +%%s)-$$$$.service\"\n"
        )));

        let run = &units[2].1;
        assert!(run.contains("TimeoutStartSec=600s\n"));
        assert!(run.contains("CollectMode=inactive-or-failed\n"));
        assert!(run.contains("ExecStart=-/bin/sh -c \"tessaro-ctl screen power on\"\n"));
        assert!(run.contains("ExecStart=-/bin/sh -c \"echo \\\"it's $$HOME\\\"\"\n"));
        assert!(run.contains(
            "ExecStopPost=/bin/sh -c \"mkdir -p '/data/tessaro/schedule-runs' && \
             printf '%%s %%s %%s %%s\\\\n' \\\"%i\\\" \\\"$$(date +%%s)\\\" \
             \\\"$$SERVICE_RESULT\\\" \\\"$$EXIT_STATUS\\\" > \
             '/data/tessaro/schedule-runs/.ab12cd34.'$$$$ && \
             mv -f '/data/tessaro/schedule-runs/.ab12cd34.'$$$$ \
             '/data/tessaro/schedule-runs/ab12cd34'\"\n"
        ));
    }

    #[test]
    fn stop_runs_lines_without_the_ignore_prefix_and_no_timeout() {
        let units = render(&schedule("ab12cd34", "morning"), Path::new("/r"));
        let run = &units[2].1;
        assert!(run.contains("ExecStart=/bin/sh -c \"tessaro-ctl screen power on\"\n"));
        assert!(run.contains("TimeoutStartSec=infinity\n"));
    }

    #[test]
    fn the_hash_follows_the_content_but_not_enabled() {
        let base = spec("morning");
        let mut disabled = base.clone();
        disabled.enabled = false;
        assert_eq!(content_hash(&base), content_hash(&disabled));
        let mut other = base.clone();
        other.lines.push("true".into());
        assert_ne!(content_hash(&base), content_hash(&other));
        assert_eq!(content_hash(&base).len(), 8);
    }

    #[test]
    fn a_plan_writes_what_differs_and_keeps_busy_templates() {
        let wanted: BTreeMap<String, String> = [
            ("tessaro-schedule-a.timer".to_string(), "t2".to_string()),
            ("tessaro-schedule-a.service".to_string(), "f".to_string()),
        ]
        .into();
        let present: BTreeMap<String, Vec<u8>> = [
            ("tessaro-schedule-a.timer".to_string(), b"t1".to_vec()),
            ("tessaro-schedule-a.service".to_string(), b"f".to_vec()),
            (
                "tessaro-schedule-a-old1@.service".to_string(),
                b"r".to_vec(),
            ),
            (
                "tessaro-schedule-a-old2@.service".to_string(),
                b"r".to_vec(),
            ),
        ]
        .into();
        let busy: BTreeSet<String> = ["tessaro-schedule-a-old1@.service".to_string()].into();
        assert_eq!(
            plan(&wanted, &present, &busy),
            Plan {
                write: vec![("tessaro-schedule-a.timer".into(), "t2".into())],
                remove: vec!["tessaro-schedule-a-old2@.service".into()],
            }
        );
        let same: BTreeMap<String, Vec<u8>> = wanted
            .iter()
            .map(|(name, body)| (name.clone(), body.clone().into_bytes()))
            .collect();
        assert!(plan(&wanted, &same, &BTreeSet::new()).is_empty());
    }

    #[test]
    fn units_are_written_and_removed_in_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("other.service"), "x").unwrap();
        let wanted = wanted(&[schedule("ab12cd34", "morning")], Path::new("/r"));
        let first = plan(&wanted, &present(dir.path()).unwrap(), &BTreeSet::new());
        assert_eq!(first.write.len(), 3);
        apply(dir.path(), &first).unwrap();
        assert!(plan(&wanted, &present(dir.path()).unwrap(), &BTreeSet::new()).is_empty());

        let gone = plan(
            &BTreeMap::new(),
            &present(dir.path()).unwrap(),
            &BTreeSet::new(),
        );
        assert_eq!(gone.remove.len(), 3);
        apply(dir.path(), &gone).unwrap();
        assert!(present(dir.path()).unwrap().is_empty());
        assert!(dir.path().join("other.service").exists());
        assert!(present(&dir.path().join("missing")).unwrap().is_empty());
    }

    #[test]
    fn an_instance_names_its_template() {
        assert_eq!(
            template_of("tessaro-schedule-ab12-cd34@1700000000-42.service").as_deref(),
            Some("tessaro-schedule-ab12-cd34@.service")
        );
        assert_eq!(template_of("tessaro-schedule-ab12.service"), None);
        assert_eq!(template_of("other@1.service"), None);
    }

    #[test]
    fn the_last_run_is_read_back() {
        assert_eq!(
            parse_ended("1700000000-42 1700000012 exit-code 1\n"),
            Some(Ended {
                started: 1_700_000_000,
                finished: 1_700_000_012,
                result: "exit-code".into(),
                status: "1".into(),
            })
        );
        assert_eq!(
            parse_ended("1700000000-42 1700000012 timeout \n").map(|ended| ended.status),
            Some(String::new())
        );
        assert_eq!(parse_ended("garbage"), None);
        assert_eq!(parse_ended(""), None);
    }

    /// `systemd-analyze calendar --iterations=3 'Mon..Fri 07:00'` in
    /// Europe/Bratislava, as systemd 255 prints it.
    const IN_A_ZONE: &str = "  Original form: Mon..Fri 07:00
Normalized form: Mon..Fri *-*-* 07:00:00
    Next elapse: Mon 2026-09-28 07:00:00 CEST
       (in UTC): Mon 2026-09-28 05:00:00 UTC
       From now: 1 day 21h left
   Iteration #2: Tue 2026-09-29 07:00:00 CEST
       (in UTC): Tue 2026-09-29 05:00:00 UTC
       From now: 2 days left
   Iteration #3: Wed 2026-09-30 07:00:00 CEST
       (in UTC): Wed 2026-09-30 05:00:00 UTC
       From now: 3 days left
";

    /// The same with `TZ=UTC`, where systemd prints no `(in UTC):` lines.
    const IN_UTC: &str = "  Original form: Mon..Fri 07:00
Normalized form: Mon..Fri *-*-* 07:00:00
    Next elapse: Mon 2026-09-28 07:00:00 UTC
       From now: 1 day 23h left
   Iteration #2: Tue 2026-09-29 07:00:00 UTC
       From now: 2 days left
";

    #[test]
    fn systemd_analyze_output_is_read_in_utc() {
        let (normalized, times) = parse_calendar(IN_A_ZONE).unwrap();
        assert_eq!(normalized, "Mon..Fri *-*-* 07:00:00");
        // 2026-09-28 05:00:00 UTC.
        assert_eq!(times, [1_790_571_600, 1_790_658_000, 1_790_744_400]);

        let (_, times) = parse_calendar(IN_UTC).unwrap();
        assert_eq!(times, [1_790_578_800, 1_790_665_200]);

        let never = "  Original form: 2020-01-01\nNormalized form: 2020-01-01 00:00:00\n    Next elapse: never\n";
        assert_eq!(parse_calendar(never).unwrap().1, Vec::<i64>::new());
        assert!(parse_calendar("").is_err());
    }

    #[test]
    fn civil_days_match_the_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(timestamp("Thu 1970-01-01 00:01:05 UTC"), Some(65));
    }
}
