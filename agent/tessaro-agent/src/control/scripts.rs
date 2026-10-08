//! `tessaro-ctl script`: the scripts in the `scripts` table of `tessaro.db`,
//! and running one now.
//!
//! Their units and bodies are rendered and reconciled with the schedules'
//! timers (`schedules.rs`, `apply_units`). A run started here is started by
//! the agent itself, not through the fire unit, so it knows the instance it
//! follows: it reads that instance's journal and ends with the record its
//! `ExecStopPost=` writes. Like the settings, scripts stay through an unclaim
//! and go with a factory reset.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use protocol::{
    Concurrency, Done, OnError, ScriptEvent, ScriptInfo, ScriptRun, ScriptSpec, SCRIPT_OUTPUT_MAX,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::{journal, log_page, Caller, Control, Stream, LOGS, LOG_PAGE};
use crate::deadline::{blocking, within};
use crate::log::Log;
use crate::schedules::{self, Schedules};
use crate::scripts::{self, Ended, Script, Scripts};
use crate::systemd::Bus;

/// How often a followed run's journal and record are read.
const POLL: Duration = Duration::from_millis(500);
/// Following a run lasts its timeout and this, for its `ExecStopPost=`.
const FOLLOW_MARGIN: Duration = Duration::from_secs(60);
/// Following a run with no timeout ends after this; the run goes on.
const FOLLOW_MAX: Duration = Duration::from_secs(24 * 3600);
/// Polls in a row with the run gone from systemd and no record before the
/// run is taken as ended without one.
const GONE_POLLS: u32 = 3;
/// The most runs one script starts on one kind of event, CEC, presence or
/// scans, in `EVENT_WINDOW`.
const EVENT_BURST: usize = 10;
const EVENT_WINDOW: Duration = Duration::from_secs(60);
/// How long a scan waits for the run it started to take it.
const SCAN_KEEP: Duration = Duration::from_secs(600);

/// A scan for one run, root's alone: `0700` directory, `0600` file, written
/// whole before the run can start. NUL bytes cannot reach a shell variable
/// and are left out.
fn keep_scan(file: &std::path::Path, scanned: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let fail = |err: std::io::Error| format!("{}: {err}", file.display());
    if let Some(dir) = file.parent() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(fail)?;
    }
    let bytes: Vec<u8> = scanned.iter().copied().filter(|byte| *byte != 0).collect();
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(file)
        .and_then(|mut out| out.write_all(&bytes))
        .map_err(fail)
}

/// What `script-set` changes; `None` keeps what is there.
pub(super) struct Change {
    pub name: Option<String>,
    pub description: Option<String>,
    pub body: Option<String>,
    pub on_error: Option<OnError>,
    pub timeout_s: Option<u64>,
    pub concurrency: Option<Concurrency>,
    pub bridge: Option<bool>,
    pub cec: Option<Vec<String>>,
    pub presence: Option<Vec<String>>,
    pub scanner: Option<Vec<String>>,
}

/// Who asked for a run, the first word of its instance.
#[derive(Debug, Clone, Copy)]
pub enum Trigger {
    Manual,
    /// The kiosk page, through `tessaro.scripts.run`.
    Bridge,
}

impl Trigger {
    fn word(self) -> &'static str {
        match self {
            Trigger::Manual => "manual",
            Trigger::Bridge => "bridge",
        }
    }
}

impl Control {
    pub(super) async fn script_list(&self) -> Result<Vec<ScriptInfo>, String> {
        let (all, schedules) = self.read_scripts_and_schedules().await?;
        let running = self.running_runs().await;
        let runs_dir = self.paths.script_runs_dir();
        blocking("reading the scripts' runs", move || {
            let names = schedule_names(&schedules);
            Ok(all
                .into_iter()
                .map(|script| {
                    let runs = scripts::read_runs(&runs_dir, &script.id)
                        .into_iter()
                        .take(protocol::SCRIPT_RUNS_KEPT)
                        .map(|ended| run_of(ended, &names))
                        .collect();
                    ScriptInfo {
                        units: scripts::journal_pattern(&script.id),
                        running: running.get(&script.id).map_or(0, |runs| runs.len() as u32),
                        schedules: schedules
                            .iter()
                            .filter(|schedule| schedule.spec.script == script.id)
                            .map(|schedule| schedule.spec.name.clone())
                            .collect(),
                        runs,
                        spec: script.spec,
                        id: script.id,
                    }
                })
                .collect())
        })
        .await
    }

    pub(super) async fn script_create(
        &self,
        caller: &Caller,
        spec: ScriptSpec,
    ) -> Result<ScriptInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let (id, spec) = blocking("updating the scripts", move || {
            db.update(|all: &mut Scripts| {
                let others: Vec<&Script> = all.scripts.iter().collect();
                let spec = scripts::validate(spec, &others)?;
                let id = scripts::new_id(|id| all.scripts.iter().any(|script| script.id == id))?;
                all.scripts.push(Script {
                    id: id.clone(),
                    spec: spec.clone(),
                });
                Ok((id, spec))
            })
        })
        .await?;
        self.log.info(format!(
            "script {} ({id}) created by {}",
            spec.name,
            caller.describe()
        ));
        self.apply_units().await?;
        self.script_info(&id).await
    }

    pub(super) async fn script_set(
        &self,
        caller: &Caller,
        query: String,
        change: Change,
    ) -> Result<ScriptInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let (id, spec) = blocking("updating the scripts", move || {
            db.update(|all: &mut Scripts| {
                let at = scripts::find(&all.scripts, &query)?;
                let mut spec = all.scripts[at].spec.clone();
                if let Some(name) = change.name {
                    spec.name = name;
                }
                if let Some(description) = change.description {
                    spec.description = description;
                }
                if let Some(body) = change.body {
                    spec.body = body;
                }
                if let Some(on_error) = change.on_error {
                    spec.on_error = on_error;
                }
                if let Some(timeout_s) = change.timeout_s {
                    spec.timeout_s = Some(timeout_s);
                }
                if let Some(concurrency) = change.concurrency {
                    spec.concurrency = concurrency;
                }
                if let Some(bridge) = change.bridge {
                    spec.bridge = bridge;
                }
                if let Some(cec) = change.cec {
                    spec.cec = cec;
                }
                if let Some(presence) = change.presence {
                    spec.presence = presence;
                }
                if let Some(scanner) = change.scanner {
                    spec.scanner = scanner;
                }
                let id = all.scripts[at].id.clone();
                let others: Vec<&Script> =
                    all.scripts.iter().filter(|other| other.id != id).collect();
                let spec = scripts::validate(spec, &others)?;
                all.scripts[at].spec = spec.clone();
                Ok((id, spec))
            })
        })
        .await?;
        self.log.info(format!(
            "script {} ({id}) changed by {}",
            spec.name,
            caller.describe()
        ));
        self.apply_units().await?;
        self.script_info(&id).await
    }

    pub(super) async fn script_remove(
        &self,
        caller: &Caller,
        query: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let removed = blocking("updating the scripts", move || {
            let schedules: Schedules = db.read(&log);
            db.update(|all: &mut Scripts| {
                let at = scripts::find(&all.scripts, &query)?;
                let script = &all.scripts[at];
                let users: Vec<&str> = schedules
                    .schedules
                    .iter()
                    .filter(|schedule| schedule.spec.script == script.id)
                    .map(|schedule| schedule.spec.name.as_str())
                    .collect();
                if !users.is_empty() {
                    return Err(format!(
                        "schedule {} runs {}; remove it or `tessaro-ctl schedule set` it \
                         to another script first",
                        users.join(", "),
                        script.spec.name
                    ));
                }
                Ok(all.scripts.remove(at))
            })
        })
        .await?;
        self.log.info(format!(
            "script {} ({}) removed by {}",
            removed.spec.name,
            removed.id,
            caller.describe()
        ));
        self.apply_units().await?;
        Ok(Done::new(format!(
            "removed script {}; runs already going finish",
            removed.spec.name
        )))
    }

    /// Start a run of script `query` now and follow it. The page may run
    /// only a script with `bridge` on; with `concurrency: skip` a run while
    /// one is going is refused.
    pub(super) async fn script_run(
        &self,
        caller: &Caller,
        query: String,
        trigger: Trigger,
    ) -> Result<Stream, String> {
        let all = self.read_scripts().await?;
        let script = all[scripts::find(&all, &query)?].clone();
        let name = &script.spec.name;
        if matches!(trigger, Trigger::Bridge) && !script.spec.bridge {
            return Err(format!(
                "{name} is not runnable from the page; `tessaro-ctl script set {name} --bridge` \
                 allows it"
            ));
        }
        if !self.paths.manage_schedules {
            return Err("this host's scripts are not run (KIOSK_MANAGE_SCHEDULES=0)".to_string());
        }
        if script.spec.concurrency == Concurrency::Skip {
            if let Some(going) = self
                .running_runs()
                .await
                .get(&script.id)
                .and_then(|runs| runs.first())
            {
                return Err(format!("skipped: run {going} of {name} is going"));
            }
        }

        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_secs())
            .unwrap_or_default();
        let instance = format!(
            "{}-{started}-{}",
            trigger.word(),
            protocol::hex(&crate::auth::random(2)?)
        );
        let unit = scripts::run_unit(&script, &instance);
        self.bus.start(&unit).await.map_err(|err| err.to_string())?;
        self.log.info(format!(
            "script {name} ({}) run {instance} started by {}",
            script.id,
            caller.describe()
        ));

        let total = script.spec.timeout_s.map_or(FOLLOW_MAX, |seconds| {
            Duration::from_secs(seconds) + FOLLOW_MARGIN
        });
        let (send, steps) = mpsc::channel(64);
        let follow = Follow {
            bus: self.bus.clone(),
            log: Arc::clone(&self.log),
            db: self.db.clone(),
            runs_dir: self.paths.script_runs_dir(),
            script,
            instance,
            unit,
        };
        tokio::spawn(follow.run(send));
        Ok(Stream::Script { steps, total })
    }

    /// Start a run of every script that runs on this CEC event, `key` for a
    /// remote key, and leave it: how it ended is in its record, as for a
    /// schedule's run. Each script starts at most `EVENT_BURST` runs in
    /// `EVENT_WINDOW`, so a remote key held down cannot pile up root shells.
    pub(super) async fn cec_scripts(&self, event: &str, key: Option<&str>) {
        let word = match key {
            Some(key) => format!("key:{key}"),
            None => event.to_string(),
        };
        let wanted = |script: &Script| protocol::cec::runs_on(&script.spec.cec, event, key);
        self.event_scripts("cec", "CEC", &word, wanted, None).await;
    }

    /// The same for a presence event (`protocol::presence::EVENTS`). With
    /// camera.presence.demographics on, `env` is the run's variables for the
    /// faces' age and gender, left like a scan (`scripts::run_shell`).
    pub(super) async fn presence_scripts(&self, event: &str, env: Option<&[u8]>) {
        let wanted = |script: &Script| protocol::presence::runs_on(&script.spec.presence, event);
        self.event_scripts("presence", "presence", event, wanted, env)
            .await;
    }

    /// The same for a scan of `scanner`. What it said goes to each run in a
    /// file of its own (`scripts::run_shell`).
    pub(super) async fn scanner_scripts(&self, scanner: &str, scanned: &[u8]) {
        let wanted = |script: &Script| protocol::scanner::runs_on(&script.spec.scanner, scanner);
        self.event_scripts("scanner", "scanner", scanner, wanted, Some(scanned))
            .await;
    }

    /// Start a run of every script `wanted` picks, as `<trigger>-<word>-...`,
    /// with each script's runs on `trigger` events held to `EVENT_BURST` in
    /// `EVENT_WINDOW`. `what` names the events in the journal. `payload` is
    /// left in `scans/<run>` for the run to read, root's alone: what a scan
    /// said, or a presence event's variables.
    async fn event_scripts(
        &self,
        trigger: &str,
        what: &str,
        word: &str,
        wanted: impl Fn(&Script) -> bool,
        payload: Option<&[u8]>,
    ) {
        if !self.paths.manage_schedules {
            return;
        }
        let all = match self.read_scripts().await {
            Ok(all) => all,
            Err(err) => {
                self.log.info(format!("{trigger}: the scripts: {err}"));
                return;
            }
        };
        let wanted: Vec<&Script> = all.iter().filter(|script| wanted(script)).collect();
        if wanted.is_empty() {
            return;
        }
        let running = self.running_runs().await;

        for script in wanted {
            let name = &script.spec.name;
            if !self.event_run_allowed(trigger, &script.id) {
                self.log.info(format!(
                    "script {name}: more than {EVENT_BURST} runs on {what} events in {}s; {word} skipped",
                    EVENT_WINDOW.as_secs()
                ));
                continue;
            }
            if script.spec.concurrency == Concurrency::Skip
                && running.get(&script.id).is_some_and(|runs| !runs.is_empty())
            {
                self.log
                    .debug(format!("script {name}: {word} skipped, a run is going"));
                continue;
            }
            let started = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_secs())
                .unwrap_or_default();
            let Ok(random) = crate::auth::random(2) else {
                continue;
            };
            let instance = format!("{trigger}-{word}-{started}-{}", protocol::hex(&random));
            if let Some(payload) = payload {
                let file = self.paths.scans_dir().join(&instance);
                let bytes = payload.to_vec();
                if let Err(err) =
                    blocking("keeping the event's payload for its script", move || {
                        keep_scan(&file, &bytes)
                    })
                    .await
                {
                    self.log.info(format!(
                        "script {name}: keeping the {what} event for it: {err}"
                    ));
                    continue;
                }
            }
            match self.bus.start(&scripts::run_unit(script, &instance)).await {
                Ok(_) => self.log.info(format!(
                    "script {name} ({}) run {instance} started by the {what} event {word}",
                    script.id
                )),
                Err(err) => self
                    .log
                    .info(format!("script {name}: starting a run for {word}: {err}")),
            }
        }
    }

    /// Scans no run took: a run that never started, or failed before its
    /// shell did, leaves its file behind. Older than `SCAN_KEEP`, removed.
    pub(super) async fn prune_scans(&self) {
        let dir = self.paths.scans_dir();
        let _ = blocking("pruning old scans", move || {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return Ok(());
            };
            for entry in entries.filter_map(Result::ok) {
                let old = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > SCAN_KEEP);
                if old {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            Ok(())
        })
        .await;
    }

    /// Room for one more run of script `id` on `trigger` events in
    /// `EVENT_WINDOW`, taken. Each kind of event has its own window.
    fn event_run_allowed(&self, trigger: &str, id: &str) -> bool {
        let now = std::time::Instant::now();
        let mut runs = crate::sync::lock(&self.event_runs);
        let recent = runs.entry(format!("{trigger}:{id}")).or_default();
        recent.retain(|at| now.duration_since(*at) < EVENT_WINDOW);
        if recent.len() >= EVENT_BURST {
            return false;
        }
        recent.push(now);
        true
    }

    /// A factory reset: every script and schedule gone, and the timers
    /// stopped now. Runs already going finish. The caller holds `writes`.
    pub(super) async fn clear_scripts(&self) -> Result<(), String> {
        let db = self.db.clone();
        let runs_dir = self.paths.script_runs_dir();
        blocking("removing the scripts and schedules", move || {
            schedules::clear(&db)?;
            scripts::clear(&db, &runs_dir)
        })
        .await?;
        if let Err(err) = self.apply_units().await {
            // /run goes with the reboot a reset ends in; until then the
            // watcher tries again.
            self.log.info(format!(
                "factory reset: the scripts' and schedules' units: {err}"
            ));
        }
        Ok(())
    }

    async fn script_info(&self, id: &str) -> Result<ScriptInfo, String> {
        self.script_list()
            .await?
            .into_iter()
            .find(|info| info.id == id)
            .ok_or_else(|| format!("script {id} is gone"))
    }

    pub(super) async fn read_scripts(&self) -> Result<Vec<Script>, String> {
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        blocking("reading the scripts", move || {
            Ok(db.read::<Scripts>(&log).scripts)
        })
        .await
    }

    pub(super) async fn read_scripts_and_schedules(
        &self,
    ) -> Result<(Vec<Script>, Vec<schedules::Schedule>), String> {
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        blocking("reading the scripts and schedules", move || {
            Ok((
                db.read::<Scripts>(&log).scripts,
                db.read::<Schedules>(&log).schedules,
            ))
        })
        .await
    }

    /// The instances of the runs going now, by script id; none when the
    /// bus cannot say.
    pub(super) async fn running_runs(&self) -> BTreeMap<String, Vec<String>> {
        let pattern = format!("{}*-*@*.service", scripts::PREFIX);
        let mut running: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let Ok(units) = self.bus.list_units(&[&pattern]).await else {
            return running;
        };
        for (name, state) in units {
            if state == "inactive" || state == "failed" {
                continue;
            }
            if let Some((id, instance)) = scripts::parse_run_unit(&name) {
                running
                    .entry(id.to_string())
                    .or_default()
                    .push(instance.to_string());
            }
        }
        running
    }
}

/// The schedules' names by id.
pub(super) fn schedule_names(schedules: &[schedules::Schedule]) -> BTreeMap<String, String> {
    schedules
        .iter()
        .map(|schedule| (schedule.id.clone(), schedule.spec.name.clone()))
        .collect()
}

/// A run record as the API answers it. Reads `/etc/localtime`: call it from
/// `blocking`.
pub(super) fn run_of(ended: Ended, schedules: &BTreeMap<String, String>) -> ScriptRun {
    ScriptRun {
        schedule: ended
            .schedule
            .as_ref()
            .and_then(|id| schedules.get(id).cloned()),
        started: schedules::moment(ended.started),
        finished: schedules::moment(ended.finished),
        run: ended.run,
        trigger: ended.trigger,
        event: ended.event,
        result: ended.result,
        status: ended.status,
    }
}

/// One run followed from its start to its record.
struct Follow {
    bus: Bus,
    log: Arc<Log>,
    db: crate::db::Db,
    runs_dir: std::path::PathBuf,
    script: Script,
    instance: String,
    unit: String,
}

impl Follow {
    /// Every line the run writes, then how it ended. Stops when nobody
    /// listens any more: a cancelled job leaves the run going.
    async fn run(self, send: mpsc::Sender<Result<ScriptEvent, String>>) {
        let started = ScriptEvent::Started {
            run: self.instance.clone(),
        };
        // naked: the receiver is the job's drain, itself bounded
        if send.send(Ok(started)).await.is_err() {
            return;
        }
        let mut cursor: Option<String> = None;
        let mut lines = 0usize;
        let mut gone = 0u32;
        // Seen ended: the journal is read once more a poll later, since
        // journald may still be writing the last lines when the record is
        // there.
        let mut finished: Option<ScriptRun> = None;
        loop {
            let ended = match finished {
                Some(_) => Ok(None),
                None => self.ended().await,
            };
            let page = match self.journal(cursor.clone()).await {
                Ok(page) => page,
                Err(err) => {
                    // naked: the receiver is the job's drain, itself bounded
                    let _ = send.send(Err(err)).await;
                    return;
                }
            };
            cursor = page.1.or(cursor);
            for text in page.0 {
                let event = if lines < SCRIPT_OUTPUT_MAX {
                    ScriptEvent::Line { text }
                } else if lines == SCRIPT_OUTPUT_MAX {
                    ScriptEvent::Cut
                } else {
                    continue;
                };
                lines += 1;
                // naked: the receiver is the job's drain, itself bounded
                if send.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            if let Some(run) = finished.take() {
                // naked: the receiver is the job's drain, itself bounded
                let _ = send.send(Ok(ScriptEvent::Ended { record: run })).await;
                return;
            }
            match ended {
                Ok(Some(run)) => finished = Some(run),
                Ok(None) => {
                    let state = self.bus.active_state(&self.unit).await;
                    gone = if matches!(state.as_str(), "inactive" | "failed" | "unknown") {
                        gone + 1
                    } else {
                        0
                    };
                }
                Err(err) => {
                    // naked: the receiver is the job's drain, itself bounded
                    let _ = send.send(Err(err)).await;
                    return;
                }
            }
            if gone >= GONE_POLLS {
                let name = &self.script.spec.name;
                let err = format!(
                    "run {} of {name} ended without saying how; \
                     `tessaro-ctl script logs {name}` shows its output",
                    self.instance
                );
                self.log.info(format!("script {name}: {err}"));
                // naked: the receiver is the job's drain, itself bounded
                let _ = send.send(Err(err)).await;
                return;
            }
            // naked: a timer, not the outside world
            tokio::time::sleep(POLL).await;
        }
    }

    /// The run's record, once its `ExecStopPost=` has written it. Read
    /// before the journal, so the last lines are in the page read after it.
    async fn ended(&self) -> Result<Option<ScriptRun>, String> {
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let runs_dir = self.runs_dir.clone();
        let id = self.script.id.clone();
        let instance = self.instance.clone();
        blocking("reading the run's record", move || {
            let Some(ended) = scripts::read_ended(&runs_dir, &id, &instance) else {
                return Ok(None);
            };
            let schedules: Schedules = db.read(&log);
            Ok(Some(run_of(ended, &schedule_names(&schedules.schedules))))
        })
        .await
    }

    /// What the run wrote since `cursor`, and where the next page starts.
    /// systemd's own lines about the unit are left out.
    async fn journal(
        &self,
        cursor: Option<String>,
    ) -> Result<(Vec<String>, Option<String>), String> {
        let lines = cursor.is_none().then_some(LOG_PAGE);
        let mut command = journal(Some(&self.unit), lines, cursor.as_deref())?;
        let output = within("journalctl", LOGS, command.output())
            .await
            .map_err(|expired| expired.to_string())?
            .map_err(|err| format!("journalctl: {err}"))?;
        let page = log_page(&String::from_utf8_lossy(&output.stdout), cursor);
        let texts = page
            .entries
            .iter()
            .filter(|entry| entry["_SYSTEMD_UNIT"].as_str() == Some(self.unit.as_str()))
            .map(message)
            .collect();
        Ok((texts, page.cursor))
    }
}

/// An entry's `MESSAGE`, which journalctl writes as bytes when it is not
/// UTF-8.
fn message(entry: &Value) -> String {
    match &entry["MESSAGE"] {
        Value::String(text) => text.clone(),
        Value::Array(bytes) => {
            let bytes: Vec<u8> = bytes
                .iter()
                .filter_map(|byte| byte.as_u64().and_then(|byte| u8::try_from(byte).ok()))
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => String::new(),
    }
}
