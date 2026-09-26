//! `tessaro-ctl schedule`: the schedules in `/data/tessaro/schedules.json`
//! and the systemd units they are rendered into (`crate::schedules`).
//!
//! Reconciling compares the unit files on disk and the units systemd has
//! loaded with what the schedules want, and changes only what differs: the
//! files first, one `Reload` if any changed, then each timer started,
//! restarted or stopped. It runs when the agent starts, which is what puts
//! the units back in `/run` after a boot, once a minute from
//! `watch_schedules` (a timer stopped by hand comes back, a run template
//! whose last run ended goes), and after every change here. Like the
//! settings, schedules stay through an unclaim and go with a factory reset.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use protocol::{
    CalendarCheck, Done, OnError, ScheduleInfo, ScheduleRun, ScheduleSpec, SCHEDULE_CHECK_MAX,
};

use super::{Caller, Control};
use crate::deadline::blocking;
use crate::proc;
use crate::schedules::{self, Schedule, Schedules};

/// One `systemd-analyze calendar` run: it parses and computes, no I/O.
const ANALYZE: Duration = Duration::from_secs(10);

/// How many run times `schedule-check` answers with when not asked.
const CHECK_DEFAULT: u32 = 5;

/// What `schedule-set` changes; `None` keeps what is there.
pub(super) struct Change {
    pub name: Option<String>,
    pub calendar: Option<Vec<String>>,
    pub lines: Option<Vec<String>>,
    pub on_error: Option<OnError>,
    pub timeout_s: Option<u64>,
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
        let all = self.read_schedules().await?;
        let running = self.running_runs().await;
        let mut reported = Vec::with_capacity(all.len());
        for schedule in all {
            let timer = schedules::timer_unit(&schedule.id);
            let next_usec = if schedule.spec.enabled {
                self.bus.timer_usec(&timer, "NextElapseUSecRealtime").await
            } else {
                None
            };
            let last_usec = self.bus.timer_usec(&timer, "LastTriggerUSec").await;
            let running = running.get(&schedule.id).copied().unwrap_or(0);
            reported.push(Reported {
                schedule,
                next_usec,
                last_usec,
                running,
            });
        }
        let runs_dir = self.paths.schedule_runs_dir();
        blocking("reading the schedules' last runs", move || {
            Ok(reported
                .into_iter()
                .map(|reported| info(reported, &runs_dir))
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
        let existing = self.read_schedules().await?;
        let spec = schedules::validate(spec, &existing.iter().collect::<Vec<_>>())?;
        self.analyze(&spec.calendar, 1).await?;

        let store = self.schedules.clone();
        let log = Arc::clone(&self.log);
        let saved = spec.clone();
        let id = blocking("updating schedules.json", move || {
            store.update(&log, |all: &mut Schedules| {
                let others: Vec<&Schedule> = all.schedules.iter().collect();
                let spec = schedules::validate(saved, &others)?;
                let id = schedules::new_id(&all.schedules)?;
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
        self.apply_schedules().await?;
        self.schedule_info(&id).await
    }

    pub(super) async fn schedule_set(
        &self,
        caller: &Caller,
        query: String,
        change: Change,
    ) -> Result<ScheduleInfo, String> {
        let _writes = self.writes.lock().await;
        let existing = self.read_schedules().await?;
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
        if let Some(lines) = change.lines {
            spec.lines = lines;
        }
        if let Some(on_error) = change.on_error {
            spec.on_error = on_error;
        }
        if let Some(timeout_s) = change.timeout_s {
            spec.timeout_s = Some(timeout_s);
        }
        if let Some(enabled) = change.enabled {
            spec.enabled = enabled;
        }
        let others: Vec<&Schedule> = existing.iter().filter(|other| other.id != id).collect();
        let spec = schedules::validate(spec, &others)?;
        if calendar_changed {
            self.analyze(&spec.calendar, 1).await?;
        }

        let store = self.schedules.clone();
        let log = Arc::clone(&self.log);
        let saved = spec.clone();
        let target = id.clone();
        blocking("updating schedules.json", move || {
            store.update(&log, |all: &mut Schedules| {
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
        self.apply_schedules().await?;
        self.schedule_info(&id).await
    }

    pub(super) async fn schedule_remove(
        &self,
        caller: &Caller,
        query: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let store = self.schedules.clone();
        let log = Arc::clone(&self.log);
        let runs_dir = self.paths.schedule_runs_dir();
        let removed = blocking("updating schedules.json", move || {
            let removed = store.update(&log, |all: &mut Schedules| {
                let at = schedules::find(&all.schedules, &query)?;
                Ok(all.schedules.remove(at))
            })?;
            schedules::forget_ended(&runs_dir, &removed.id)
                .map_err(|err| format!("{}: {err}", runs_dir.display()))?;
            Ok(removed)
        })
        .await?;
        self.log.info(format!(
            "schedule {} ({}) removed by {}",
            removed.spec.name,
            removed.id,
            caller.describe()
        ));
        self.apply_schedules().await?;
        Ok(Done::new(format!(
            "removed schedule {}; runs already going finish",
            removed.spec.name
        )))
    }

    pub(super) async fn schedule_run(
        &self,
        caller: &Caller,
        query: String,
    ) -> Result<Done, String> {
        let all = self.read_schedules().await?;
        let schedule = &all[schedules::find(&all, &query)?];
        if !self.paths.manage_schedules {
            return Err(
                "this host's schedules are not managed (KIOSK_MANAGE_SCHEDULES=0)".to_string(),
            );
        }
        let unit = schedules::fire_unit(&schedule.id);
        self.bus.start(&unit).await.map_err(|err| err.to_string())?;
        self.log.info(format!(
            "schedule {} ({}) run by {}",
            schedule.spec.name,
            schedule.id,
            caller.describe()
        ));
        Ok(Done::new(format!(
            "started a run of {}; `tessaro-ctl schedule logs {}` shows its output",
            schedule.spec.name, schedule.spec.name
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

    /// A factory reset: every schedule gone, and its timer stopped now.
    /// Runs already going finish. The caller holds `writes`.
    pub(super) async fn clear_schedules(&self) -> Result<(), String> {
        let store = self.schedules.clone();
        let runs_dir = self.paths.schedule_runs_dir();
        blocking("removing the schedules", move || {
            schedules::clear(&store, &runs_dir).map_err(|err| err.to_string())
        })
        .await?;
        if let Err(err) = self.apply_schedules().await {
            // /run goes with the reboot a reset ends in; until then the
            // watcher tries again.
            self.log
                .info(format!("factory reset: the schedules' units: {err}"));
        }
        Ok(())
    }

    /// Keeps the schedules' units on `schedules.json`, from the agent's
    /// start and then once a minute.
    pub fn watch_schedules(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(60);
        /// systemd or the bus not answering yet.
        const RETRY: Duration = Duration::from_secs(10);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let mut reported: Option<String> = None;
            loop {
                // naked: apply_schedules waits only on blocking() and the bus, each bounded
                let outcome = control.apply_schedules().await;
                let wait = if outcome.is_ok() { EVERY } else { RETRY };
                let problem = outcome.err();
                if problem != reported {
                    if let Some(err) = &problem {
                        control.log.info(format!("schedules: {err}"));
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

    /// Reconcile: the unit files, then systemd. One at a time, whoever asks.
    async fn apply_schedules(&self) -> Result<(), String> {
        // naked: held only by another apply_schedules, itself bounded
        let _reconciling = self.reconciling.lock().await;
        let manage = self.paths.manage_schedules;
        let pattern = format!("{}*", schedules::PREFIX);
        let loaded = if manage {
            let listed = self.bus.list_units(&[&pattern]).await;
            listed.map_err(|err| err.to_string())?
        } else {
            Vec::new()
        };
        let busy: BTreeSet<String> = loaded
            .iter()
            .filter(|(_, state)| state != "inactive" && state != "failed")
            .filter_map(|(name, _)| schedules::template_of(name))
            .collect();

        let store = self.schedules.clone();
        let log = Arc::clone(&self.log);
        let unit_dir = self.paths.systemd_unit_dir.clone();
        let runs_dir = self.paths.schedule_runs_dir();
        let (plan, all) = blocking("rendering the schedules' units", move || {
            let all: Schedules = store.read(&log);
            let wanted = schedules::wanted(&all.schedules, &runs_dir);
            let present = schedules::present(&unit_dir)
                .map_err(|err| format!("{}: {err}", unit_dir.display()))?;
            let plan = schedules::plan(&wanted, &present, &busy);
            schedules::apply(&unit_dir, &plan)
                .map_err(|err| format!("{}: {err}", unit_dir.display()))?;
            Ok((plan, all.schedules))
        })
        .await?;
        if !plan.is_empty() {
            let written: Vec<&str> = plan.write.iter().map(|(name, _)| name.as_str()).collect();
            self.log.info(format!(
                "schedules: wrote [{}], removed [{}]",
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

    async fn read_schedules(&self) -> Result<Vec<Schedule>, String> {
        let store = self.schedules.clone();
        let log = Arc::clone(&self.log);
        blocking("reading schedules.json", move || {
            Ok(store.read::<Schedules>(&log).schedules)
        })
        .await
    }

    /// Runs going now, by schedule id; none when the bus cannot say.
    async fn running_runs(&self) -> BTreeMap<String, u32> {
        let pattern = format!("{}*@*.service", schedules::PREFIX);
        let mut running = BTreeMap::new();
        let Ok(units) = self.bus.list_units(&[&pattern]).await else {
            return running;
        };
        for (name, state) in units {
            if state == "inactive" || state == "failed" {
                continue;
            }
            let id = name
                .trim_start_matches(schedules::PREFIX)
                .split(['-', '@'])
                .next()
                .unwrap_or_default()
                .to_string();
            *running.entry(id).or_insert(0) += 1;
        }
        running
    }

    /// Each expression through `systemd-analyze calendar`: how systemd
    /// normalizes it, and the next `count` times any of them fires.
    async fn analyze(
        &self,
        calendar: &[String],
        count: u32,
    ) -> Result<(Vec<String>, Vec<i64>), String> {
        let probe = ScheduleSpec {
            name: "check".to_string(),
            enabled: true,
            calendar: calendar.to_vec(),
            lines: vec!["true".to_string()],
            on_error: OnError::Stop,
            timeout_s: None,
        };
        let calendar = schedules::validate(probe, &[])?.calendar;
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

/// One schedule as `schedule-list` answers it. Reads the runs file and
/// `/etc/localtime`: call it from `blocking`.
fn info(reported: Reported, runs_dir: &std::path::Path) -> ScheduleInfo {
    let moment_of = |usec: Option<u64>| {
        usec.filter(|usec| *usec > 0)
            .and_then(|usec| i64::try_from(usec / 1_000_000).ok())
            .map(schedules::moment)
    };
    let id = reported.schedule.id;
    let last_run = schedules::read_ended(runs_dir, &id).map(|ended| ScheduleRun {
        started: schedules::moment(ended.started),
        finished: schedules::moment(ended.finished),
        result: ended.result,
        status: ended.status,
    });
    ScheduleInfo {
        units: schedules::runs_pattern(&id),
        next: moment_of(reported.next_usec),
        last_trigger: moment_of(reported.last_usec),
        last_run,
        running: reported.running,
        spec: reported.schedule.spec,
        id,
    }
}
