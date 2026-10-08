//! Schedules: `OnCalendar` times at which systemd runs a script.
//!
//! The `schedules` table of `tessaro.db` is the only truth. Each schedule is
//! rendered into one timer in `/run/systemd/system`
//! (`Paths::systemd_unit_dir`), and systemd does all the timing: the
//! calendar, the timezone, clock jumps. The agent only keeps the units
//! matching the table (`control/schedules.rs` reconciles), so a schedule
//! keeps firing while the agent is down.
//!
//! `tessaro-schedule-<id>.timer` has one `OnCalendar=` per expression,
//! `AccuracySec=1s` (the default minute would move every run) and no
//! `Persistent=`, so a time missed while the device was off is skipped. It
//! starts its script's fire unit with `schedule-<id>` as the trigger
//! (`scripts.rs`), which starts the run.
//!
//! Everything here is pure or blocking file work; the bus calls are in
//! `control/schedules.rs`.

use std::collections::BTreeMap;

use protocol::{Moment, ScheduleSpec, SCHEDULE_CALENDAR_MAX, SCHEDULE_EXPRESSION_MAX};
use tessaro_db::rusqlite::{self, params, types::Type, Connection};

use crate::db::{Db, Stored};
use crate::scripts::{self, check_name, Script};
use crate::units::HEADER;

/// Every unit of a schedule starts with this.
pub const PREFIX: &str = "tessaro-schedule-";

#[derive(Debug, Default)]
pub struct Schedules {
    pub schedules: Vec<Schedule>,
}

/// A schedule as stored: `spec.script` is the script's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub id: String,
    pub spec: ScheduleSpec,
}

impl Stored for Schedules {
    const WHAT: &'static str = "the schedules";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db.prepare(
            "SELECT id, name, enabled, calendar, script_id \
             FROM schedules ORDER BY position",
        )?;
        let schedules = rows
            .query_map([], |row| {
                let calendar: String = row.get(3)?;
                let calendar = serde_json::from_str(&calendar).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(3, Type::Text, Box::new(err))
                })?;
                Ok(Schedule {
                    id: row.get(0)?,
                    spec: ScheduleSpec {
                        name: row.get(1)?,
                        enabled: row.get(2)?,
                        calendar,
                        script: row.get(4)?,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { schedules })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM schedules", [])?;
        let mut insert = db.prepare(
            "INSERT INTO schedules (id, position, name, enabled, calendar, script_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for (position, schedule) in self.schedules.iter().enumerate() {
            let spec = &schedule.spec;
            insert.execute(params![
                schedule.id,
                position as i64,
                spec.name,
                spec.enabled,
                serde_json::to_string(&spec.calendar).unwrap_or_default(),
                spec.script,
            ])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM schedules", []).map(drop)
    }
}

/// `spec` made ready to save: trimmed, checked, `script` turned into the
/// id of one of `scripts`. `others` are the schedules it must not share a
/// name with.
pub fn validate(
    mut spec: ScheduleSpec,
    others: &[&Schedule],
    scripts: &[Script],
) -> Result<ScheduleSpec, String> {
    spec.name = spec.name.trim().to_string();
    check_name("schedule", &spec.name)?;
    if others.iter().any(|other| other.spec.name == spec.name) {
        return Err(format!("a schedule named {} exists already", spec.name));
    }
    spec.calendar = check_calendar(spec.calendar)?;
    let script = spec.script.trim();
    if script.is_empty() {
        return Err("a schedule needs a script to run".to_string());
    }
    spec.script = scripts[scripts::find(scripts, script)?].id.clone();
    Ok(spec)
}

/// Calendar expressions trimmed, blanks dropped, and checked for what
/// systemd-analyze is not asked about.
pub fn check_calendar(calendar: Vec<String>) -> Result<Vec<String>, String> {
    let calendar: Vec<String> = calendar
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if calendar.is_empty() {
        return Err("a schedule needs at least one calendar expression".to_string());
    }
    if calendar.len() > SCHEDULE_CALENDAR_MAX {
        return Err(format!(
            "a schedule takes at most {SCHEDULE_CALENDAR_MAX} calendar expressions"
        ));
    }
    for expression in &calendar {
        if expression.len() > SCHEDULE_EXPRESSION_MAX {
            return Err(format!(
                "a calendar expression is at most {SCHEDULE_EXPRESSION_MAX} bytes long"
            ));
        }
        if expression.chars().any(|ch| ch.is_control() && ch != '\t') {
            return Err(format!(
                "{expression:?}: a calendar expression has no newlines or other control characters"
            ));
        }
        // Nothing in the OnCalendar syntax uses it, and in a unit file it
        // starts a specifier.
        if expression.contains('%') {
            return Err(format!("{expression:?}: a calendar expression has no %"));
        }
    }
    Ok(calendar)
}

/// The schedule `query` names: its id or its exact name.
pub fn find(schedules: &[Schedule], query: &str) -> Result<usize, String> {
    schedules
        .iter()
        .position(|schedule| schedule.id == query || schedule.spec.name == query)
        .ok_or_else(|| format!("no schedule {query:?}; `tessaro-ctl schedule list` shows them"))
}

pub fn timer_unit(id: &str) -> String {
    format!("{PREFIX}{id}.timer")
}

/// What the timer passes its script's fire unit as the trigger.
pub fn trigger(id: &str) -> String {
    format!("schedule-{id}")
}

/// The timer schedule `schedule` is, by name.
pub fn render(schedule: &Schedule) -> (String, String) {
    let id = &schedule.id;
    let spec = &schedule.spec;
    let mut timer = format!(
        "{HEADER}[Unit]\nDescription=tessaro schedule {}\n\n[Timer]\n",
        spec.name
    );
    for expression in &spec.calendar {
        timer.push_str(&format!("OnCalendar={expression}\n"));
    }
    timer.push_str(&format!(
        "AccuracySec=1s\nUnit={}\n",
        scripts::fire_unit(&spec.script, &trigger(id))
    ));
    (timer_unit(id), timer)
}

/// Every timer the schedules are made of.
pub fn wanted(schedules: &[Schedule]) -> BTreeMap<String, String> {
    schedules.iter().map(render).collect()
}

/// Every schedule gone: a factory reset. The timers follow at the next
/// reconcile, or with `/run` at the next boot.
pub fn clear(db: &Db) -> Result<(), String> {
    db.clear::<Schedules>()
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
    use protocol::{Concurrency, OnError, ScriptSpec};

    fn spec(name: &str) -> ScheduleSpec {
        ScheduleSpec {
            name: name.to_string(),
            enabled: true,
            calendar: vec!["Mon..Fri 07:00".to_string()],
            script: "wake".to_string(),
        }
    }

    fn scripts() -> Vec<Script> {
        vec![Script {
            id: "5c41a7e0".to_string(),
            spec: ScriptSpec {
                name: "wake".to_string(),
                description: String::new(),
                body: "true\n".to_string(),
                on_error: OnError::Stop,
                timeout_s: None,
                concurrency: Concurrency::Overlap,
                bridge: false,
                cec: Vec::new(),
                presence: Vec::new(),
                scanner: Vec::new(),
            },
        }]
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
        let valid = validate(input, &[], &scripts()).unwrap();
        assert_eq!(valid.name, "morning");
        assert_eq!(valid.calendar, ["Mon..Fri 07:00"]);
        assert_eq!(valid.script, "5c41a7e0");
        let mut by_id = spec("morning");
        by_id.script = "5c41a7e0".into();
        assert_eq!(validate(by_id, &[], &scripts()).unwrap().script, "5c41a7e0");

        assert!(validate(spec("Morning"), &[], &scripts()).is_err());
        let taken = schedule("aa", "morning");
        assert!(validate(spec("morning"), &[&taken], &scripts()).is_err());

        let mut no_script = spec("x");
        no_script.script = "sleep".into();
        assert!(validate(no_script, &[], &scripts()).is_err());
        let mut no_calendar = spec("x");
        no_calendar.calendar = vec![" ".into()];
        assert!(validate(no_calendar, &[], &scripts()).is_err());
        let mut percent = spec("x");
        percent.calendar = vec!["%H".into()];
        assert!(validate(percent, &[], &scripts()).is_err());
        let mut newline = spec("x");
        newline.calendar = vec!["Mon\n07:00".into()];
        assert!(validate(newline, &[], &scripts()).is_err());
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
    fn a_schedule_renders_a_timer_starting_its_script() {
        let mut schedule = schedule("ab12cd34", "morning");
        schedule.spec.script = "5c41a7e0".into();
        schedule
            .spec
            .calendar
            .push("Sat,Sun 09:00 Europe/Bratislava".into());
        let (name, timer) = render(&schedule);
        assert_eq!(name, "tessaro-schedule-ab12cd34.timer");
        assert!(timer
            .contains("OnCalendar=Mon..Fri 07:00\nOnCalendar=Sat,Sun 09:00 Europe/Bratislava\n"));
        assert!(timer.contains("AccuracySec=1s\n"));
        assert!(timer.contains("Unit=tessaro-script-5c41a7e0@schedule-ab12cd34.service\n"));
        assert!(!timer.contains("Persistent"));
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
