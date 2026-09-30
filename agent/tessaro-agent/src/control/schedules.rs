//! `tessaro-ctl schedule`: the schedules in the `schedules` table of
//! `tessaro.db`, and the reconcile of every unit the scripts and schedules
//! are rendered into (`crate::scripts`, `crate::schedules`).
//!
//! Reconciling compares the files on disk (the unit files, the script
//! bodies) and the units systemd has loaded with what the store wants, and
//! changes only what differs: the files first, one `Reload` if a unit
//! changed, then each timer started, restarted or stopped. It runs when the
//! agent starts, which is what puts the units back in `/run` after a boot,
//! once a minute from `watch_units` (a timer stopped by hand comes back, a
//! run template whose last run ended goes, the run records are pruned), and
//! after every change. Like the settings, schedules stay through an unclaim
//! and go with a factory reset.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use protocol::{CalendarCheck, Done, ScheduleInfo, ScheduleSpec, SCHEDULE_CHECK_MAX};

use super::scripts::{run_of, schedule_names};
use super::{Caller, Control};
use crate::deadline::blocking;
use crate::proc;
use crate::schedules::{self, Schedule, Schedules};
use crate::scripts::{self, Scripts};
use crate::units;

/// One `systemd-analyze calendar` run: it parses and computes, no I/O.
const ANALYZE: Duration = Duration::from_secs(10);

/// How many run times `schedule-check` answers with when not asked.
const CHECK_DEFAULT: u32 = 5;

/// What `schedule-set` changes; `None` keeps what is there.
pub(super) struct Change {
    pub name: Option<String>,
    pub calendar: Option<Vec<String>>,
    pub script: Option<String>,
    pub enabled: Option<bool>,
}

/// What systemd reports about one schedule, before the times are put on
/// the device's clock.
struct Reported {
    schedule: Schedule,
    next_usec: Option<u64>,
    last_usec: Option<u64>,
    running: u32,
}

impl Control {
    pub(super) async fn schedule_list(&self) -> Result<Vec<ScheduleInfo>, String> {
        let (all_scripts, all) = self.read_scripts_and_schedules().await?;
        let running = self.running_runs().await;
        let mut reported = Vec::with_capacity(all.len());
        for schedule in &all {
            let timer = schedules::timer_unit(&schedule.id);
            let next_usec = if schedule.spec.enabled {
                self.bus.timer_usec(&timer, "NextElapseUSecRealtime").await
            } else {
                None
            };
            let last_usec = self.bus.timer_usec(&timer, "LastTriggerUSec").await;
            let started = format!("{}-", schedules::trigger(&schedule.id));
            let running = running.get(&schedule.spec.script).map_or(0, |runs| {
                runs.iter().filter(|run| run.starts_with(&started)).count() as u32
            });
            reported.push(Reported {
                schedule: schedule.clone(),
                next_usec,
                last_usec,
                running,
            });
        }
        let runs_dir = self.paths.script_runs_dir();
        blocking("reading the schedules' last runs", move || {
            let names = schedule_names(&all);
            let script_names: BTreeMap<&str, &str> = all_scripts
                .iter()
                .map(|script| (script.id.as_str(), script.spec.name.as_str()))
                .collect();
            Ok(reported
                .into_iter()
                .map(|reported| {
                    let schedule = &reported.schedule;
                    let script = schedule.spec.script.clone();
                    let last_run = scripts::read_runs(&runs_dir, &script)
                        .into_iter()
                        .find(|run| run.schedule.as_deref() == Some(schedule.id.as_str()))
                        .map(|ended| run_of(ended, &names));
                    let moment_of = |usec: Option<u64>| {
                        usec.filter(|usec| *usec > 0)
                            .and_then(|usec| i64::try_from(usec / 1_000_000).ok())
                            .map(schedules::moment)
                    };
                    ScheduleInfo {
                        units: scripts::schedule_journal_pattern(&script, &schedule.id),
                        script_name: script_names
                            .get(script.as_str())
                            .map_or_else(|| script.clone(), |name| name.to_string()),
                        next: moment_of(reported.next_usec),
                        last_trigger: moment_of(reported.last_usec),
                        last_run,
                        running: reported.running,
                        id: schedule.id.clone(),
                        spec: reported.schedule.spec,
                    }
                })
                .collect())
        })
        .await
    }

    pub(super) async fn schedule_create(
        &self,
        caller: &Caller,
        spec: ScheduleSpec,
    ) -> Result<ScheduleInfo, String> {
        let _writes = self.writes.lock().await;
        let (all_scripts, existing) = self.read_scripts_and_schedules().await?;
        let spec = schedules::validate(spec, &existing.iter().collect::<Vec<_>>(), &all_scripts)?;
        self.analyze(&spec.calendar, 1).await?;

        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let saved = spec.clone();
        let id = blocking("updating the schedules", move || {
            let all_scripts = db.read::<Scripts>(&log).scripts;
            db.update(|all: &mut Schedules| {
                let others: Vec<&Schedule> = all.schedules.iter().collect();
                let spec = schedules::validate(saved, &others, &all_scripts)?;
                let id =
                    scripts::new_id(|id| all.schedules.iter().any(|schedule| schedule.id == id))?;
                all.schedules.push(Schedule {
                    id: id.clone(),
                    spec,
                });
                Ok(id)
            })
        })
        .await?;
        self.log.info(format!(
            "schedule {} ({id}) created by {}",
            spec.name,
            caller.describe()
        ));
        self.apply_units().await?;
        self.schedule_info(&id).await
    }

    pub(super) async fn schedule_set(
        &self,
        caller: &Caller,
        query: String,
        change: Change,
    ) -> Result<ScheduleInfo, String> {
        let _writes = self.writes.lock().await;
        let (all_scripts, existing) = self.read_scripts_and_schedules().await?;
        let at = schedules::find(&existing, &query)?;
        let id = existing[at].id.clone();
        let calendar_changed = change.calendar.is_some();
        let mut spec = existing[at].spec.clone();
        if let Some(name) = change.name {
            spec.name = name;
        }
        if let Some(calendar) = change.calendar {
            spec.calendar = calendar;
        }
        if let Some(script) = change.script {
            spec.script = script;
        }
        if let Some(enabled) = change.enabled {
            spec.enabled = enabled;
        }
        let others: Vec<&Schedule> = existing.iter().filter(|other| other.id != id).collect();
        let spec = schedules::validate(spec, &others, &all_scripts)?;
        if calendar_changed {
            self.analyze(&spec.calendar, 1).await?;
        }

        let db = self.db.clone();
        let saved = spec.clone();
        let target = id.clone();
        blocking("updating the schedules", move || {
            db.update(|all: &mut Schedules| {
                let at = schedules::find(&all.schedules, &target)?;
                all.schedules[at].spec = saved;
                Ok(())
            })
        })
        .await?;
        self.log.info(format!(
            "schedule {} ({id}) changed by {}",
            spec.name,
            caller.describe()
        ));
        self.apply_units().await?;
        self.schedule_info(&id).await
    }

    pub(super) async fn schedule_remove(
        &self,
        caller: &Caller,
        query: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let removed = blocking("updating the schedules", move || {
            db.update(|all: &mut Schedules| {
                let at = schedules::find(&all.schedules, &query)?;
                Ok(all.schedules.remove(at))
            })
        })
        .await?;
        self.log.info(format!(
            "schedule {} ({}) removed by {}",
            removed.spec.name,
            removed.id,
            caller.describe()
        ));
        self.apply_units().await?;
        Ok(Done::new(format!(
            "removed schedule {}; runs already going finish",
            removed.spec.name
        )))
    }

    pub(super) async fn schedule_check(
        &self,
        calendar: Vec<String>,
        count: Option<u32>,
    ) -> Result<CalendarCheck, String> {
        let count = count.unwrap_or(CHECK_DEFAULT).clamp(1, SCHEDULE_CHECK_MAX);
        let (normalized, times) = self.analyze(&calendar, count).await?;
        blocking("reading the local time", move || {
            Ok(CalendarCheck {
                normalized,
                next: times.into_iter().map(schedules::moment).collect(),
            })
        })
        .await
    }

    /// Keeps the scripts' and schedules' units on the store, from the
    /// agent's start and then once a minute.
    pub fn watch_units(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(60);
        /// systemd or the bus not answering yet.
        const RETRY: Duration = Duration::from_secs(10);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut reported: Option<String> = None;
            loop {
                // naked: apply_units waits only on blocking() and the bus, each bounded
                let outcome = control.apply_units().await;
                let wait = if outcome.is_ok() { EVERY } else { RETRY };
                let problem = outcome.err();
                if problem != reported {
                    if let Some(err) = &problem {
                        control.log.info(format!("scripts and schedules: {err}"));
                    }
                    reported = problem;
                }
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// Reconcile: the files, then systemd. One at a time, whoever asks.
    pub(super) async fn apply_units(&self) -> Result<(), String> {
        // naked: held only by another apply_units, itself bounded
        let _reconciling = self.reconciling.lock().await;
        let manage = self.paths.manage_schedules;
        let patterns = [
            format!("{}*", scripts::PREFIX),
            format!("{}*", schedules::PREFIX),
        ];
        let loaded = if manage {
            let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
            let listed = self.bus.list_units(&patterns).await;
            listed.map_err(|err| err.to_string())?
        } else {
            Vec::new()
        };
        let busy: BTreeSet<String> = loaded
            .iter()
            .filter(|(_, state)| state != "inactive" && state != "failed")
            .filter_map(|(name, _)| units::template_of(name))
            .collect();
        let busy_bodies: BTreeSet<String> = busy
            .iter()
            .filter_map(|template| scripts::body_of_template(template))
            .collect();

        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let unit_dir = self.paths.systemd_unit_dir.clone();
        let body_dir = self.paths.script_body_dir();
        let runs_dir = self.paths.script_runs_dir();
        let (plan, all) = blocking("rendering the scripts' and schedules' units", move || {
            let all_scripts: Scripts = db.read(&log);
            let all: Schedules = db.read(&log);
            let (mut wanted, bodies) = scripts::wanted(&all_scripts.scripts, &body_dir, &runs_dir);
            let with_script: Vec<Schedule> = all
                .schedules
                .into_iter()
                .filter(|schedule| {
                    all_scripts
                        .scripts
                        .iter()
                        .any(|script| script.id == schedule.spec.script)
                })
                .collect();
            wanted.extend(schedules::wanted(&with_script));

            let at =
                |dir: &std::path::Path, err: std::io::Error| format!("{}: {err}", dir.display());
            let present = units::present(&unit_dir, &[scripts::PREFIX, schedules::PREFIX])
                .map_err(|err| at(&unit_dir, err))?;
            let plan = units::plan(&wanted, &present, &busy);
            units::apply(&unit_dir, &plan, 0o644).map_err(|err| at(&unit_dir, err))?;

            let present = units::present(&body_dir, &[""]).map_err(|err| at(&body_dir, err))?;
            let body_plan = units::plan(&bodies, &present, &busy_bodies);
            units::apply(&body_dir, &body_plan, 0o600).map_err(|err| at(&body_dir, err))?;

            let ids: Vec<&str> = all_scripts
                .scripts
                .iter()
                .map(|script| script.id.as_str())
                .collect();
            scripts::forget_others(&runs_dir, &ids).map_err(|err| at(&runs_dir, err))?;
            for id in &ids {
                scripts::prune_runs(&runs_dir, id).map_err(|err| at(&runs_dir, err))?;
            }
            Ok((plan, with_script))
        })
        .await?;
        if !plan.is_empty() {
            let written: Vec<&str> = plan.write.iter().map(|(name, _)| name.as_str()).collect();
            self.log.info(format!(
                "scripts and schedules: wrote [{}], removed [{}]",
                written.join(", "),
                plan.remove.join(", ")
            ));
        }
        if !manage {
            return Ok(());
        }
        if !plan.is_empty() {
            self.bus.reload().await.map_err(|err| err.to_string())?;
        }

        let timers: BTreeMap<&str, &str> = loaded
            .iter()
            .filter(|(name, _)| name.ends_with(".timer"))
            .map(|(name, state)| (name.as_str(), state.as_str()))
            .collect();
        let rewritten: BTreeSet<&str> = plan.write.iter().map(|(name, _)| name.as_str()).collect();
        let mut problems = Vec::new();
        let mut wanted_timers = BTreeSet::new();
        for schedule in &all {
            let timer = schedules::timer_unit(&schedule.id);
            let state = timers.get(timer.as_str()).copied().unwrap_or("inactive");
            let action = if !schedule.spec.enabled {
                (state == "active").then_some("stopped")
            } else if rewritten.contains(timer.as_str()) {
                Some("restarted")
            } else {
                (state != "active").then_some("started")
            };
            if let Some(action) = action {
                let outcome = match action {
                    "stopped" => self.bus.stop(&timer).await,
                    "restarted" => self.bus.restart(&timer).await,
                    _ => self.bus.start(&timer).await,
                };
                match outcome {
                    Ok(()) => self
                        .log
                        .info(format!("schedule {}: timer {action}", schedule.spec.name)),
                    Err(err) => problems.push(err.to_string()),
                }
            }
            wanted_timers.insert(timer);
        }
        for (timer, state) in &timers {
            if *state == "active" && !wanted_timers.contains(*timer) {
                match self.bus.stop(timer).await {
                    Ok(()) => self.log.info(format!("schedules: {timer} stopped")),
                    Err(err) => problems.push(err.to_string()),
                }
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    async fn schedule_info(&self, id: &str) -> Result<ScheduleInfo, String> {
        self.schedule_list()
            .await?
            .into_iter()
            .find(|info| info.id == id)
            .ok_or_else(|| format!("schedule {id} is gone"))
    }

    /// Each expression through `systemd-analyze calendar`: how systemd
    /// normalizes it, and the next `count` times any of them fires.
    async fn analyze(
        &self,
        calendar: &[String],
        count: u32,
    ) -> Result<(Vec<String>, Vec<i64>), String> {
        let calendar = schedules::check_calendar(calendar.to_vec())?;
        let mut normalized = Vec::with_capacity(calendar.len());
        let mut times = Vec::new();
        for expression in &calendar {
            let mut command = tokio::process::Command::new(&self.paths.systemd_analyze);
            command
                .env("LC_ALL", "C")
                .arg("calendar")
                .arg(format!("--iterations={count}"))
                .arg("--")
                .arg(expression);
            let output = proc::run_async(&mut command, None, "systemd-analyze", ANALYZE).await?;
            if !output.status.success() {
                let said = proc::said(&output);
                return Err(if said.is_empty() {
                    format!("{expression:?} is not a calendar expression")
                } else {
                    said
                });
            }
            let (form, fires) =
                schedules::parse_calendar(&String::from_utf8_lossy(&output.stdout))?;
            normalized.push(form);
            times.extend(fires);
        }
        times.sort_unstable();
        times.dedup();
        times.truncate(count as usize);
        Ok((normalized, times))
    }
}
