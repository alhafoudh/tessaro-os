//! Scripts: named shell bodies systemd runs as root, on demand, for a
//! schedule, or for the kiosk page.
//!
//! The `scripts` table of `tessaro.db` is the only truth. Each script is
//! rendered into a body file in `/run/tessaro-kiosk/scripts`
//! (`Paths::script_body_dir`) and systemd units in `/run/systemd/system`;
//! `control/schedules.rs` keeps both equal to the table.
//!
//! Per script `<id>`, with `<hash>` naming what a run is made of:
//!
//! * `<id>-<hash>.sh`: the body, `0600`, never escaped into a unit.
//! * `tessaro-script-<id>-<hash>@.service`: the run. It execs `/bin/sh`
//!   (`-e` when a failure stops it) on the body, and its `ExecStopPost=`
//!   writes how the run ended to `script-runs/<id>/<instance>`. The hash is
//!   in the name so an edit never changes the body under a running
//!   instance: the old template and body stay until its last run ends.
//! * `tessaro-script-<id>@.service`: what a trigger starts, with the
//!   trigger as its instance (`schedule-<schedule id>`). It starts a fresh
//!   run instance and exits at once, or, with `concurrency: skip` and a run
//!   going, says so and starts nothing. A timer never finds it running, so
//!   runs can overlap.
//!
//! A run's instance is `<trigger>-...-<unix start>-<unique>`: `manual-...`
//! and `bridge-...` started by the agent, `schedule-<schedule id>-...` by the
//! fire unit. The first word is the run's `TESSARO_TRIGGER`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use protocol::{
    Concurrency, OnError, ScriptSpec, SCRIPT_BODY_MAX, SCRIPT_DESCRIPTION_MAX, SCRIPT_RUNS_KEPT,
};
use tessaro_db::rusqlite::{self, params, types::Type, Connection};

use crate::db::{Db, Stored};
use crate::units::{exec_arg, shell, HEADER, INSTANCE};

/// Every unit and journal pattern of a script starts with this.
pub const PREFIX: &str = "tessaro-script-";

#[derive(Debug, Default)]
pub struct Scripts {
    pub scripts: Vec<Script>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub id: String,
    pub spec: ScriptSpec,
}

fn parsed<T: std::str::FromStr<Err = String>>(at: usize, text: String) -> rusqlite::Result<T> {
    text.parse().map_err(|err: String| {
        rusqlite::Error::FromSqlConversionFailure(at, Type::Text, err.into())
    })
}

impl Stored for Scripts {
    const WHAT: &'static str = "the scripts";

    fn load(db: &Connection) -> rusqlite::Result<Self> {
        let mut rows = db.prepare(
            "SELECT id, name, description, body, on_error, timeout_s, concurrency, bridge, cec, \
             presence FROM scripts ORDER BY position",
        )?;
        let scripts = rows
            .query_map([], |row| {
                let timeout_s: Option<i64> = row.get(5)?;
                let list = |at: usize| -> rusqlite::Result<Vec<String>> {
                    let text: String = row.get(at)?;
                    serde_json::from_str(&text).map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(at, Type::Text, err.into())
                    })
                };
                let cec = list(8)?;
                let presence = list(9)?;
                Ok(Script {
                    id: row.get(0)?,
                    spec: ScriptSpec {
                        name: row.get(1)?,
                        description: row.get(2)?,
                        body: row.get(3)?,
                        on_error: parsed(4, row.get(4)?)?,
                        timeout_s: timeout_s.map(|seconds| seconds as u64),
                        concurrency: parsed(6, row.get(6)?)?,
                        bridge: row.get(7)?,
                        cec,
                        presence,
                    },
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Self { scripts })
    }

    fn save(&self, db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM scripts", [])?;
        let mut insert = db.prepare(
            "INSERT INTO scripts \
             (id, position, name, description, body, on_error, timeout_s, concurrency, bridge, \
              cec, presence) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )?;
        for (position, script) in self.scripts.iter().enumerate() {
            let spec = &script.spec;
            insert.execute(params![
                script.id,
                position as i64,
                spec.name,
                spec.description,
                spec.body,
                spec.on_error.name(),
                spec.timeout_s.map(|seconds| seconds as i64),
                spec.concurrency.name(),
                spec.bridge,
                serde_json::to_string(&spec.cec).unwrap_or_else(|_| "[]".to_string()),
                serde_json::to_string(&spec.presence).unwrap_or_else(|_| "[]".to_string()),
            ])?;
        }
        Ok(())
    }

    fn clear(db: &Connection) -> rusqlite::Result<()> {
        db.execute("DELETE FROM scripts", []).map(drop)
    }
}

/// `spec` made ready to save: trimmed, checked, line ends made `\n`,
/// `timeout_s: Some(0)` as none. `others` are the scripts it must not share
/// a name with.
pub fn validate(mut spec: ScriptSpec, others: &[&Script]) -> Result<ScriptSpec, String> {
    spec.name = spec.name.trim().to_string();
    check_name("script", &spec.name)?;
    if others.iter().any(|other| other.spec.name == spec.name) {
        return Err(format!("a script named {} exists already", spec.name));
    }

    spec.description = spec.description.trim().to_string();
    if spec.description.len() > SCRIPT_DESCRIPTION_MAX {
        return Err(format!(
            "a description is at most {SCRIPT_DESCRIPTION_MAX} bytes long"
        ));
    }
    if spec.description.chars().any(char::is_control) {
        return Err("a description is one line of text".to_string());
    }

    spec.body = spec.body.replace("\r\n", "\n");
    if spec.body.trim().is_empty() {
        return Err("a script needs a body".to_string());
    }
    if spec.body.len() > SCRIPT_BODY_MAX {
        return Err(format!("a body is at most {SCRIPT_BODY_MAX} bytes long"));
    }
    if spec
        .body
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
    {
        return Err("a body has no control characters but newlines and tabs".to_string());
    }
    if !spec.body.ends_with('\n') {
        spec.body.push('\n');
    }

    if spec.timeout_s == Some(0) {
        spec.timeout_s = None;
    }
    spec.cec = protocol::cec::triggers(&spec.cec.join(","))?;
    spec.presence = protocol::presence::triggers(&spec.presence.join(","))?;
    Ok(spec)
}

/// A script or schedule name: it goes into unit descriptions, the
/// journal's identifier and `TESSARO_SCRIPT`, so it stays plain.
pub fn check_name(what: &str, name: &str) -> Result<(), String> {
    let valid = name.len() <= 40
        && name.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        && name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "{name:?} is not a {what} name: lower-case letters, digits and -, \
             starting with a letter or digit, at most 40"
        ))
    }
}

/// The script `query` names: its id or its exact name.
pub fn find(scripts: &[Script], query: &str) -> Result<usize, String> {
    scripts
        .iter()
        .position(|script| script.id == query || script.spec.name == query)
        .ok_or_else(|| format!("no script {query:?}; `tessaro-ctl script list` shows them"))
}

/// A fresh id: 8 hex characters `taken` says no to.
pub fn new_id(taken: impl Fn(&str) -> bool) -> Result<String, String> {
    loop {
        let id = protocol::hex(&crate::auth::random(4)?);
        if !taken(&id) {
            return Ok(id);
        }
    }
}

/// 8 hex characters of SHA-256 over everything a run is made of: a change to
/// any of it is a new run template and body. The description, the bridge
/// flag and the CEC events change no run.
pub fn content_hash(spec: &ScriptSpec) -> String {
    let content = serde_json::json!([
        spec.name,
        spec.body,
        spec.on_error,
        spec.timeout_s,
        spec.concurrency
    ]);
    protocol::hex(&openssl::sha::sha256(content.to_string().as_bytes()))[..8].to_string()
}

/// What a trigger starts, with the trigger as the instance.
pub fn fire_unit(id: &str, trigger: &str) -> String {
    format!("{PREFIX}{id}@{trigger}.service")
}

fn fire_template(id: &str) -> String {
    format!("{PREFIX}{id}@.service")
}

/// The run template of `script` as it is now.
pub fn run_template(script: &Script) -> String {
    format!(
        "{PREFIX}{}-{}@.service",
        script.id,
        content_hash(&script.spec)
    )
}

/// One run of `script` as it is now.
pub fn run_unit(script: &Script, instance: &str) -> String {
    format!(
        "{PREFIX}{}-{}@{instance}.service",
        script.id,
        content_hash(&script.spec)
    )
}

pub fn body_file(id: &str, hash: &str) -> String {
    format!("{id}-{hash}.sh")
}

/// The body file a run template runs.
pub fn body_of_template(template: &str) -> Option<String> {
    let rest = template.strip_prefix(PREFIX)?.strip_suffix("@.service")?;
    let (id, hash) = rest.split_once('-')?;
    Some(body_file(id, hash))
}

/// Every run of script `id`, whichever template it came from: a pattern
/// for `ListUnitsByPatterns`.
pub fn runs_pattern(id: &str) -> String {
    format!("{PREFIX}{id}-*@*.service")
}

/// Every run of script `id` and what its fire unit said: a pattern for
/// `journalctl --unit`. Ids are all the same length, so no other script's
/// units match.
pub fn journal_pattern(id: &str) -> String {
    format!("{PREFIX}{id}*@*.service")
}

/// The runs schedule `schedule` started, and its fire unit's notices.
pub fn schedule_journal_pattern(id: &str, schedule: &str) -> String {
    format!("{PREFIX}{id}*@schedule-{schedule}*.service")
}

/// A running unit's script id and run instance: `None` for anything but a
/// run.
pub fn parse_run_unit(unit: &str) -> Option<(&str, &str)> {
    let rest = unit.strip_prefix(PREFIX)?.strip_suffix(".service")?;
    let (template, instance) = rest.split_once('@')?;
    let (id, _hash) = template.split_once('-')?;
    (!instance.is_empty()).then_some((id, instance))
}

/// The unit files and the body file `script` is made of, by name.
pub fn render(
    script: &Script,
    body_dir: &Path,
    runs_dir: &Path,
) -> (Vec<(String, String)>, (String, String)) {
    let id = &script.id;
    let spec = &script.spec;
    let hash = content_hash(spec);
    let body = body_file(id, &hash);
    let template = run_template(script);
    let instance_prefix = template.trim_end_matches("@.service");

    let start = format!(
        "exec systemctl start --no-block {instance_prefix}@{INSTANCE}-$(date +%s)-$$.service"
    );
    let start = match spec.concurrency {
        Concurrency::Overlap => start,
        Concurrency::Skip => format!(
            "if [ -n \"$(systemctl list-units --no-legend --plain \
             --state=activating,active,deactivating '{}')\" ]; then \
             echo 'skipped: a run of {} is going'; exit 0; fi; {start}",
            runs_pattern(id),
            spec.name
        ),
    };
    let fire = format!(
        "{HEADER}[Unit]\nDescription=tessaro script {name} (start a run for %i)\n\n\
         [Service]\nType=oneshot\nSyslogIdentifier={PREFIX}{name}\n\
         ExecStart={}\n",
        shell(&start),
        name = spec.name
    );

    let timeout = spec
        .timeout_s
        .map_or_else(|| "infinity".to_string(), |seconds| format!("{seconds}s"));
    let strict = match spec.on_error {
        OnError::Stop => " -e",
        OnError::Continue => "",
    };
    let exec = run_shell(strict);
    let run = format!(
        "{HEADER}[Unit]\nDescription=tessaro script {name} (run)\n\
         CollectMode=inactive-or-failed\n\n\
         [Service]\nType=oneshot\nTimeoutStartSec={timeout}\n\
         SyslogIdentifier={PREFIX}{name}\nWorkingDirectory=/\n\
         Environment=TESSARO_SCRIPT={name} TESSARO_RUN=%i\n\
         ExecStart=/bin/sh -c {} {}\n\
         ExecStopPost={}\n",
        exec_arg(&exec),
        exec_arg(&body_dir.join(&body).display().to_string()),
        shell(&record_script(runs_dir, id)),
        name = spec.name
    );

    (
        vec![(fire_template(id), fire), (template, run)],
        (body, spec.body.clone()),
    )
}

/// What a run's `/bin/sh -c` runs before the body, which is its `$0`. The
/// trigger is the instance's first word, which systemd has no specifier
/// for. A CEC or presence run's event is what lies between it and the start
/// time: `cec-tv-on-<unix>-<hex>`, `cec-key:red-<unix>-<hex>`,
/// `presence-arrived-<unix>-<hex>`.
fn run_shell(strict: &str) -> String {
    format!(
        "export TESSARO_TRIGGER=\"${{TESSARO_RUN%%-*}}\"; \
         if [ \"$TESSARO_TRIGGER\" = cec ]; then \
         e=\"${{TESSARO_RUN#cec-}}\"; e=\"${{e%-*}}\"; export TESSARO_CEC_EVENT=\"${{e%-*}}\"; \
         case \"$TESSARO_CEC_EVENT\" in key:*) \
         export TESSARO_CEC_KEY=\"${{TESSARO_CEC_EVENT#key:}}\" TESSARO_CEC_EVENT=key;; esac; fi; \
         if [ \"$TESSARO_TRIGGER\" = presence ]; then \
         e=\"${{TESSARO_RUN#presence-}}\"; e=\"${{e%-*}}\"; export TESSARO_PRESENCE_EVENT=\"${{e%-*}}\"; fi; \
         exec /bin/sh{strict} \"$0\""
    )
}

/// What `ExecStopPost=` runs: `<finished> <result> <status>` into
/// `script-runs/<id>/<instance>`, through a temporary so a reader never
/// sees half a line.
fn record_script(runs_dir: &Path, id: &str) -> String {
    let dir = runs_dir.join(id);
    let dir = dir.display();
    format!(
        "mkdir -p '{dir}' && \
         printf '%s %s %s\\n' \"$(date +%s)\" \"$SERVICE_RESULT\" \"$EXIT_STATUS\" \
         > '{dir}/.{INSTANCE}' && mv -f '{dir}/.{INSTANCE}' '{dir}/{INSTANCE}'"
    )
}

/// Every unit file and every body file the scripts are made of.
pub fn wanted(
    scripts: &[Script],
    body_dir: &Path,
    runs_dir: &Path,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut units = BTreeMap::new();
    let mut bodies = BTreeMap::new();
    for script in scripts {
        let (files, body) = render(script, body_dir, runs_dir);
        units.extend(files);
        bodies.insert(body.0, body.1);
    }
    (units, bodies)
}

/// How a run ended, as its `ExecStopPost=` wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub run: String,
    pub trigger: String,
    /// The schedule that started it, by id.
    pub schedule: Option<String>,
    /// The CEC or presence event that started it: `tv-on`, `key:red`,
    /// `arrived`.
    pub event: Option<String>,
    pub started: i64,
    pub finished: i64,
    pub result: String,
    pub status: String,
}

/// Run `run`'s record, `1700000012 exit-code 1`; `None` for anything else.
pub fn parse_ended(run: &str, text: &str) -> Option<Ended> {
    let mut fields = text.trim_end_matches('\n').splitn(3, ' ');
    let finished = fields.next()?.parse().ok()?;
    let result = fields.next()?.to_string();
    let status = fields.next().unwrap_or("").to_string();
    let words: Vec<&str> = run.split('-').collect();
    let trigger = words.first()?.to_string();
    let started = words.iter().rev().nth(1)?.parse().ok()?;
    let schedule = (trigger == "schedule")
        .then(|| words.get(1).map(|id| id.to_string()))
        .flatten();
    let event = (matches!(trigger.as_str(), "cec" | "presence") && words.len() > 3)
        .then(|| words[1..words.len() - 2].join("-"));
    Some(Ended {
        run: run.to_string(),
        trigger,
        schedule,
        event,
        started,
        finished,
        result,
        status,
    })
}

/// Script `id`'s run `run`, once it has ended.
pub fn read_ended(runs_dir: &Path, id: &str, run: &str) -> Option<Ended> {
    parse_ended(run, &fs::read_to_string(runs_dir.join(id).join(run)).ok()?)
}

/// Script `id`'s finished runs, newest first.
pub fn read_runs(runs_dir: &Path, id: &str) -> Vec<Ended> {
    let Ok(entries) = fs::read_dir(runs_dir.join(id)) else {
        return Vec::new();
    };
    let mut runs: Vec<Ended> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let run = entry.file_name().to_str()?.to_string();
            if run.starts_with('.') {
                return None;
            }
            parse_ended(&run, &fs::read_to_string(entry.path()).ok()?)
        })
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse((run.started, run.finished)));
    runs
}

/// Keep script `id`'s newest `SCRIPT_RUNS_KEPT` records.
pub fn prune_runs(runs_dir: &Path, id: &str) -> io::Result<()> {
    for old in read_runs(runs_dir, id).into_iter().skip(SCRIPT_RUNS_KEPT) {
        remove(&runs_dir.join(id).join(&old.run))?;
    }
    Ok(())
}

/// Forget every run of the scripts not in `ids`.
pub fn forget_others(runs_dir: &Path, ids: &[&str]) -> io::Result<()> {
    let entries = match fs::read_dir(runs_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    for entry in entries {
        let entry = entry?;
        let known = entry
            .file_name()
            .to_str()
            .is_some_and(|name| ids.contains(&name));
        if !known {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Every script and every run record gone: a factory reset. The units
/// follow at the next reconcile, or with `/run` at the next boot.
pub fn clear(db: &Db, runs_dir: &Path) -> Result<(), String> {
    db.clear::<Scripts>()?;
    match fs::remove_dir_all(runs_dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            Err(format!("{}: {err}", runs_dir.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> ScriptSpec {
        ScriptSpec {
            name: name.to_string(),
            description: String::new(),
            body: "tessaro-ctl screen power on\n".to_string(),
            on_error: OnError::Stop,
            timeout_s: None,
            concurrency: Concurrency::Overlap,
            bridge: false,
            cec: Vec::new(),
            presence: Vec::new(),
        }
    }

    fn script(id: &str, name: &str) -> Script {
        Script {
            id: id.to_string(),
            spec: spec(name),
        }
    }

    #[test]
    fn a_spec_is_trimmed_and_checked() {
        let mut input = spec(" morning ");
        input.description = "  wakes the screen ".into();
        input.body = "echo a\r\necho b".into();
        input.timeout_s = Some(0);
        let valid = validate(input, &[]).unwrap();
        assert_eq!(valid.name, "morning");
        assert_eq!(valid.description, "wakes the screen");
        assert_eq!(valid.body, "echo a\necho b\n");
        assert_eq!(valid.timeout_s, None);

        assert!(validate(spec("Morning"), &[]).is_err());
        assert!(validate(spec("-morning"), &[]).is_err());
        let taken = script("aa", "morning");
        assert!(validate(spec("morning"), &[&taken]).is_err());

        let mut empty = spec("x");
        empty.body = " \n\t\n".into();
        assert!(validate(empty, &[]).is_err());
        let mut long = spec("x");
        long.body = "x".repeat(SCRIPT_BODY_MAX + 1);
        assert!(validate(long, &[]).is_err());
        let mut nul = spec("x");
        nul.body = "echo \0".into();
        assert!(validate(nul, &[]).is_err());
        let mut two_lines = spec("x");
        two_lines.description = "a\nb".into();
        assert!(validate(two_lines, &[]).is_err());

        let mut shell = spec("x");
        shell.body = "for f in \"$HOME\"/*; do echo \"100% $f\\n\"; done\n".into();
        assert!(validate(shell, &[]).is_ok());
    }

    #[test]
    fn a_script_is_found_by_id_or_name() {
        let all = [script("ab12cd34", "morning"), script("ef56ab78", "night")];
        assert_eq!(find(&all, "night"), Ok(1));
        assert_eq!(find(&all, "ab12cd34"), Ok(0));
        assert!(find(&all, "ab12").is_err());
    }

    #[test]
    fn a_script_renders_its_fire_and_run_units_and_its_body() {
        let mut spec = spec("morning");
        spec.on_error = OnError::Continue;
        spec.timeout_s = Some(600);
        let script = Script {
            id: "ab12cd34".into(),
            spec,
        };
        let hash = content_hash(&script.spec);
        let (units, body) = render(
            &script,
            Path::new("/run/tessaro-kiosk/scripts"),
            Path::new("/data/tessaro/script-runs"),
        );
        let names: Vec<&str> = units.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "tessaro-script-ab12cd34@.service".to_string(),
                format!("tessaro-script-ab12cd34-{hash}@.service"),
            ]
        );
        assert_eq!(
            body,
            (
                format!("ab12cd34-{hash}.sh"),
                "tessaro-ctl screen power on\n".to_string()
            )
        );

        let fire = &units[0].1;
        assert!(fire.contains(&format!(
            "ExecStart=/bin/sh -c \"exec systemctl start --no-block \
             tessaro-script-ab12cd34-{hash}@%i-$$(date +%%s)-$$$$.service\"\n"
        )));
        assert!(!fire.contains("list-units"));

        let run = &units[1].1;
        assert!(run.contains("TimeoutStartSec=600s\n"));
        assert!(run.contains("CollectMode=inactive-or-failed\n"));
        assert!(run.contains("Environment=TESSARO_SCRIPT=morning TESSARO_RUN=%i\n"));
        assert!(run.contains(
            "ExecStart=/bin/sh -c \"export TESSARO_TRIGGER=\\\"$${TESSARO_RUN%%%%-*}\\\"; "
        ));
        assert!(run.contains(&format!(
            "exec /bin/sh \\\"$$0\\\"\" \"/run/tessaro-kiosk/scripts/ab12cd34-{hash}.sh\"\n"
        )));
        assert!(run.contains(
            "ExecStopPost=/bin/sh -c \"mkdir -p '/data/tessaro/script-runs/ab12cd34' && \
             printf '%%s %%s %%s\\\\n' \\\"$$(date +%%s)\\\" \
             \\\"$$SERVICE_RESULT\\\" \\\"$$EXIT_STATUS\\\" > \
             '/data/tessaro/script-runs/ab12cd34/.%i' && \
             mv -f '/data/tessaro/script-runs/ab12cd34/.%i' \
             '/data/tessaro/script-runs/ab12cd34/%i'\"\n"
        ));
    }

    #[test]
    fn stop_runs_the_body_with_e_and_skip_checks_for_a_run() {
        let mut skipping = script("ab12cd34", "morning");
        skipping.spec.concurrency = Concurrency::Skip;
        let (units, _) = render(&skipping, Path::new("/b"), Path::new("/r"));
        assert!(units[1].1.contains("exec /bin/sh -e \\\"$$0\\\""));
        assert!(units[1].1.contains("TimeoutStartSec=infinity\n"));
        assert!(units[0].1.contains(
            "if [ -n \\\"$$(systemctl list-units --no-legend --plain \
             --state=activating,active,deactivating 'tessaro-script-ab12cd34-*@*.service')\\\" ]; \
             then echo 'skipped: a run of morning is going'; exit 0; fi; exec systemctl start"
        ));
    }

    #[test]
    fn the_hash_follows_the_run_but_not_the_description_or_the_bridge() {
        let base = spec("morning");
        let mut described = base.clone();
        described.description = "x".into();
        described.bridge = true;
        assert_eq!(content_hash(&base), content_hash(&described));
        let mut other = base.clone();
        other.body.push_str("true\n");
        assert_ne!(content_hash(&base), content_hash(&other));
        let mut skipping = base.clone();
        skipping.concurrency = Concurrency::Skip;
        assert_ne!(content_hash(&base), content_hash(&skipping));
        assert_eq!(content_hash(&base).len(), 8);
    }

    #[test]
    fn units_and_bodies_name_each_other() {
        let script = script("ab12cd34", "morning");
        let hash = content_hash(&script.spec);
        assert_eq!(
            body_of_template(&run_template(&script)),
            Some(format!("ab12cd34-{hash}.sh"))
        );
        assert_eq!(body_of_template("tessaro-script-ab12cd34@.service"), None);
        assert_eq!(
            parse_run_unit(&run_unit(&script, "manual-1700000000-4f2a")),
            Some(("ab12cd34", "manual-1700000000-4f2a"))
        );
        assert_eq!(
            parse_run_unit("tessaro-script-ab12cd34@schedule-ef56ab78.service"),
            None
        );
        assert_eq!(
            fire_unit("ab12cd34", "schedule-ef56ab78"),
            "tessaro-script-ab12cd34@schedule-ef56ab78.service"
        );
    }

    #[test]
    fn a_run_learns_its_trigger_and_cec_event_from_its_instance() {
        let dir = tempfile::tempdir().unwrap();
        let body = dir.path().join("body");
        fs::write(
            &body,
            "echo \"$TESSARO_TRIGGER|${TESSARO_CEC_EVENT-}|${TESSARO_CEC_KEY-}\"\n",
        )
        .unwrap();
        let run = |instance: &str| {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(run_shell(""))
                .arg(&body)
                .env("TESSARO_RUN", instance)
                .env_remove("TESSARO_CEC_EVENT")
                .env_remove("TESSARO_CEC_KEY")
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        assert_eq!(run("manual-1700000000-4f2a"), "manual||");
        assert_eq!(run("cec-tv-standby-1700000000-4f2a"), "cec|tv-standby|");
        assert_eq!(run("cec-key:red-1700000000-4f2a"), "cec|key|red");
    }

    #[test]
    fn a_run_learns_its_presence_event_from_its_instance() {
        let dir = tempfile::tempdir().unwrap();
        let body = dir.path().join("body");
        fs::write(
            &body,
            "echo \"$TESSARO_TRIGGER|${TESSARO_PRESENCE_EVENT-}|${TESSARO_CEC_EVENT-}\"\n",
        )
        .unwrap();
        let run = |instance: &str| {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(run_shell(""))
                .arg(&body)
                .env("TESSARO_RUN", instance)
                .env_remove("TESSARO_PRESENCE_EVENT")
                .env_remove("TESSARO_CEC_EVENT")
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        assert_eq!(run("presence-arrived-1700000000-4f2a"), "presence|arrived|");
        assert_eq!(run("manual-1700000000-4f2a"), "manual||");
        let ended = parse_ended("presence-left-1700000000-4f2a", "1700000001 success 0").unwrap();
        assert_eq!(
            (ended.trigger.as_str(), ended.event.as_deref()),
            ("presence", Some("left"))
        );
    }

    #[test]
    fn runs_are_read_back_newest_first_and_pruned() {
        assert_eq!(
            parse_ended(
                "schedule-ef56ab78-1700000000-42",
                "1700000012 exit-code 1\n"
            ),
            Some(Ended {
                run: "schedule-ef56ab78-1700000000-42".into(),
                trigger: "schedule".into(),
                schedule: Some("ef56ab78".into()),
                event: None,
                started: 1_700_000_000,
                finished: 1_700_000_012,
                result: "exit-code".into(),
                status: "1".into(),
            })
        );
        let manual = parse_ended("manual-1700000000-4f2a", "1700000001 timeout \n").unwrap();
        assert_eq!((manual.trigger.as_str(), manual.schedule), ("manual", None));
        assert_eq!(manual.status, "");
        assert_eq!(parse_ended("manual-1700000000-4f2a", "garbage"), None);
        assert_eq!(parse_ended("x", "1700000001 success 0"), None);
        let cec = parse_ended("cec-tv-standby-1700000000-4f2a", "1700000001 success 0").unwrap();
        assert_eq!(
            (cec.trigger.as_str(), cec.event.as_deref(), cec.started),
            ("cec", Some("tv-standby"), 1_700_000_000)
        );

        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("ab12cd34");
        fs::create_dir_all(&runs).unwrap();
        for at in 0..SCRIPT_RUNS_KEPT + 2 {
            let started = 1_700_000_000 + at as i64;
            fs::write(
                runs.join(format!("manual-{started}-1")),
                format!("{} success 0\n", started + 1),
            )
            .unwrap();
        }
        fs::write(runs.join(".manual-1-1"), "half").unwrap();
        let all = read_runs(dir.path(), "ab12cd34");
        assert_eq!(all.len(), SCRIPT_RUNS_KEPT + 2);
        assert_eq!(all[0].started, 1_700_000_000 + SCRIPT_RUNS_KEPT as i64 + 1);
        prune_runs(dir.path(), "ab12cd34").unwrap();
        let kept = read_runs(dir.path(), "ab12cd34");
        assert_eq!(kept.len(), SCRIPT_RUNS_KEPT);
        assert_eq!(kept.last().unwrap().started, 1_700_000_002);
        assert!(read_ended(dir.path(), "ab12cd34", &kept[0].run).is_some());

        fs::create_dir_all(dir.path().join("gone0000")).unwrap();
        forget_others(dir.path(), &["ab12cd34"]).unwrap();
        assert!(runs.exists());
        assert!(!dir.path().join("gone0000").exists());
    }
}
