//! `tessaro-ctl script ...`: shell scripts the device keeps and runs as
//! root, now (`run`), for a schedule (`tessaro-ctl schedule`), for the
//! kiosk page (`--bridge`), on the TV's HDMI-CEC events (`--cec`) or on
//! presence events (`--presence`).
//!
//! A body runs with `/bin/sh` from `/`, so a `tessaro-ctl` command in it is
//! written out in full. `run` follows the run to its end and prints what it
//! wrote, stdout and stderr together as the journal keeps them.

use std::cell::Cell;
use std::io::Read;
use std::path::PathBuf;

use crate::out::println;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, Subcommand};
use protocol::api::{self, ScriptChange, ScriptRef};
use protocol::{Concurrency, OnError, ScriptEvent, ScriptInfo, ScriptSpec};
use tessaro_client::schedule::{duration, now, parse_timeout};
use tessaro_client::script::{self as shared, behaviour, event_line, last_run, run_line};

use crate::connect::Session;
use crate::schedule::journal;
use crate::style::{self, pad, paint};
use crate::{done, print, print_json, prompt};

#[derive(Subcommand)]
pub enum ScriptCmd {
    /// Every script: what it is for, how its last run ended.
    List,
    /// One script in full: its body, how it runs, its recent runs.
    Show { script: String },
    /// Add a script.
    ///
    ///   tessaro-ctl script create dim --body 'tessaro-ctl screen power off'
    ///   tessaro-ctl script create cleanup --file cleanup.sh --on-error continue --timeout 10m
    ///   tessaro-ctl script create restock --file restock.sh --bridge --concurrency skip
    ///   tessaro-ctl script create back-off --body 'tessaro-ctl screen power off' --cec key:red
    ///   tessaro-ctl script create greet --body 'tessaro-ctl screen power on' --presence arrived
    Create {
        /// Lower-case letters, digits and -.
        name: String,
        #[command(flatten)]
        fields: Fields,
    },
    /// Change a script, by name or id; runs already going keep the old one.
    ///
    ///   tessaro-ctl script set cleanup --file cleanup.sh
    ///   tessaro-ctl script set restock --no-bridge --timeout none
    Set {
        script: String,
        /// Rename it.
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        fields: Fields,
        /// The page may not run it any more.
        #[arg(long, conflicts_with = "bridge")]
        no_bridge: bool,
    },
    /// Run it now and follow it: its output, then how it ended. Exits
    /// non-zero when the run failed.
    ///
    ///   tessaro-ctl script run cleanup
    Run {
        script: String,
        /// Only start it, and say which run it is.
        #[arg(long)]
        no_wait: bool,
    },
    /// Remove a script no schedule runs; runs already going finish.
    Remove {
        script: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// What its runs wrote, from the device's journal.
    Logs {
        script: String,
        /// Only this run, as `show` lists them.
        #[arg(long, value_name = "RUN")]
        run: Option<String>,
        #[arg(long, short)]
        follow: bool,
        /// How many lines back to start (`-n` is --node).
        #[arg(long, default_value_t = 100)]
        lines: u32,
    },
}

/// What a script runs and how.
#[derive(Args)]
pub struct Fields {
    /// The shell body, run with /bin/sh as root.
    #[arg(long, value_name = "TEXT", conflicts_with = "file")]
    body: Option<String>,
    /// The body from FILE; `-` reads stdin.
    #[arg(long, value_name = "FILE")]
    file: Option<PathBuf>,
    /// One line saying what it does.
    #[arg(long, value_name = "TEXT")]
    description: Option<String>,
    /// When a command fails: stop the run there (`sh -e`), or continue.
    #[arg(long = "on-error", value_name = "WHAT",
        value_parser = PossibleValuesParser::new(OnError::NAMES)
            .map(|name| name.parse::<OnError>().expect("one of the names")))]
    on_error: Option<OnError>,
    /// The longest a run may take before it is killed: 90s, 10m, 2h;
    /// `none` for no limit.
    #[arg(long, value_name = "DURATION", value_parser = parse_timeout)]
    timeout: Option<u64>,
    /// While a run is going: start another anyway, or skip it.
    #[arg(long, value_name = "WHAT",
        value_parser = PossibleValuesParser::new(Concurrency::NAMES)
            .map(|name| name.parse::<Concurrency>().expect("one of the names")))]
    concurrency: Option<Concurrency>,
    /// The kiosk page may list it and run it (tessaro.scripts).
    #[arg(long)]
    bridge: bool,
    /// Run it on HDMI-CEC events, comma separated: tv-on, tv-standby,
    /// source-gained, source-lost, key (any remote key) or key:NAME (one,
    /// e.g. key:red); empty for none. Needs screen.cec.enable.
    #[arg(long, value_name = "EVENTS", value_parser = cec_events)]
    cec: Option<String>,
    /// Run it on presence events, comma separated: arrived, left, near,
    /// far; empty for none. Needs camera.presence.enable.
    #[arg(long, value_name = "EVENTS", value_parser = presence_events)]
    presence: Option<String>,
}

/// `--cec` checked, as the comma-separated list it is stored as.
fn cec_events(typed: &str) -> Result<String, String> {
    protocol::cec::triggers(typed).map(|events| events.join(","))
}

/// `--presence` checked, the same way.
fn presence_events(typed: &str) -> Result<String, String> {
    protocol::presence::triggers(typed).map(|events| events.join(","))
}

/// The events `--cec` or `--presence` gave, one by one.
fn cec_list(checked: String) -> Vec<String> {
    checked
        .split(',')
        .filter(|event| !event.is_empty())
        .map(str::to_string)
        .collect()
}

impl Fields {
    /// The body, or `None` when neither --body nor --file was given.
    fn body(&self) -> Result<Option<String>, String> {
        match (&self.body, &self.file) {
            (Some(body), _) => Ok(Some(body.clone())),
            (None, Some(path)) if path.as_os_str() == "-" => {
                let mut text = String::new();
                std::io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|err| format!("stdin: {err}"))?;
                Ok(Some(text))
            }
            (None, Some(path)) => std::fs::read_to_string(path)
                .map(Some)
                .map_err(|err| format!("{}: {err}", path.display())),
            (None, None) => Ok(None),
        }
    }
}

pub fn run(session: &mut Session, command: ScriptCmd, json: bool) -> Result<(), String> {
    match command {
        ScriptCmd::List => {
            let scripts = session.fetch::<api::script::List>()?;
            print(json, &scripts, || list(&scripts))
        }
        ScriptCmd::Show { script } => {
            let info = find(session, &script)?;
            print(json, &info, || show(&info))
        }
        ScriptCmd::Create { name, fields } => {
            let Some(body) = fields.body()? else {
                return Err("a script needs a body: --body TEXT or --file FILE".to_string());
            };
            let spec = ScriptSpec {
                name,
                description: fields.description.unwrap_or_default(),
                body,
                on_error: fields.on_error.unwrap_or_default(),
                timeout_s: fields.timeout.filter(|seconds| *seconds > 0),
                concurrency: fields.concurrency.unwrap_or_default(),
                bridge: fields.bridge,
                cec: fields.cec.map(cec_list).unwrap_or_default(),
                presence: fields.presence.map(cec_list).unwrap_or_default(),
            };
            let info = session.send::<api::script::Create>(spec)?;
            print(json, &info, || {
                println!(
                    "{}",
                    paint(style::OK, format!("created {}", info.spec.name))
                );
                show(&info);
            })
        }
        ScriptCmd::Set {
            script,
            name,
            fields,
            no_bridge,
        } => {
            let bridge = if fields.bridge {
                Some(true)
            } else if no_bridge {
                Some(false)
            } else {
                None
            };
            let change = ScriptChange {
                name,
                body: fields.body()?,
                description: fields.description,
                on_error: fields.on_error,
                timeout_s: fields.timeout,
                concurrency: fields.concurrency,
                bridge,
                cec: fields.cec.map(cec_list),
                presence: fields.presence.map(cec_list),
            };
            let info = session.call::<api::script::Change>(ScriptRef { script }, change)?;
            print(json, &info, || {
                println!(
                    "{}",
                    paint(style::OK, format!("changed {}", info.spec.name))
                );
                show(&info);
            })
        }
        ScriptCmd::Run { script, no_wait } => follow(session, &script, no_wait, json),
        ScriptCmd::Remove { script, yes } => {
            prompt::confirm(yes, &format!("Remove script {script}?"))?;
            done::<api::script::Remove>(session, ScriptRef { script }, (), json)
        }
        ScriptCmd::Logs {
            script,
            run,
            follow,
            lines,
        } => {
            let info = find(session, &script)?;
            let units = match run {
                Some(run) => format!("tessaro-script-{}-*@{run}.service", info.id),
                None => info.units,
            };
            journal(session, units, lines, follow, json)
        }
    }
}

/// `script run`: every step as it comes, then how the run ended; a failed
/// run is this command failing. With `no_wait`, only the start.
fn follow(session: &mut Session, script: &str, no_wait: bool, json: bool) -> Result<(), String> {
    let started = Cell::new(false);
    let stop = || no_wait && started.get();
    let ended = shared::run(session, script, &stop, |event| {
        if matches!(event, ScriptEvent::Started { .. }) {
            started.set(true);
        }
        if json {
            let _ = print_json(event);
            return;
        }
        match event {
            // A failed run is said once, as the command's error.
            ScriptEvent::Ended { record } if !record.succeeded() => {}
            _ => println!("{}", style::line(&event_line(event, script))),
        }
    })?;
    match ended {
        Some(run) if !run.succeeded() => Err(shared::ended(&run).to_string()),
        Some(_) => Ok(()),
        None if no_wait => {
            if !json {
                println!(
                    "{} {}",
                    paint(style::MUTED, "its output:"),
                    paint(style::CMD, format!("tessaro-ctl script logs {script}"))
                );
            }
            Ok(())
        }
        None => Err("the run's end was not seen".to_string()),
    }
}

/// The script `query` names, by name or id.
fn find(session: &mut Session, query: &str) -> Result<ScriptInfo, String> {
    let scripts = session.fetch::<api::script::List>()?;
    scripts
        .into_iter()
        .find(|info| info.id == query || info.spec.name == query)
        .ok_or_else(|| format!("no script {query:?}; `tessaro-ctl script list` shows them"))
}

fn list(scripts: &[ScriptInfo]) {
    if scripts.is_empty() {
        println!(
            "{} {}",
            paint(style::MUTED, "no scripts; add one with"),
            paint(style::CMD, "tessaro-ctl script create")
        );
        return;
    }
    let now = now();
    println!(
        "{} {} {}",
        pad(style::HEADING, "name", 20),
        pad(style::HEADING, "description", 36),
        paint(style::HEADING, "last run")
    );
    for info in scripts {
        let description: String = info.spec.description.chars().take(36).collect();
        println!(
            "{} {} {}",
            pad(style::HEADING, &info.spec.name, 20),
            pad(anstyle::Style::new(), description, 36),
            style::line(&last_run(info, now))
        );
    }
}

fn show(info: &ScriptInfo) {
    let now = now();
    let spec = &info.spec;
    style::row("name", &paint(style::HEADING, &spec.name));
    style::row("id", &paint(style::MUTED, &info.id));
    if !spec.description.is_empty() {
        style::row("about", &spec.description);
    }
    style::row("runs", &behaviour(spec));
    if !info.schedules.is_empty() {
        style::row("schedules", &info.schedules.join(", "));
    }
    if let Some(seconds) = spec.timeout_s {
        style::row("timeout", &duration(seconds));
    }
    if info.running > 0 {
        style::row("running", &paint(style::WARN, info.running));
    }
    match info.runs.split_first() {
        None => style::row("last runs", &paint(style::MUTED, "never")),
        Some((first, rest)) => {
            let label = |at: usize| if at == 0 { "last runs" } else { "" };
            for (at, run) in std::iter::once(first).chain(rest).take(5).enumerate() {
                style::row(
                    label(at),
                    &format!(
                        "{} {}",
                        style::line(&run_line(run, now)),
                        paint(style::MUTED, &run.run)
                    ),
                );
            }
        }
    }
    println!("\n{}", paint(style::HEADING, "body"));
    for line in spec.body.lines() {
        println!("  {line}");
    }
    println!(
        "\n{} {}   {} {}",
        paint(style::MUTED, "run it:"),
        paint(style::CMD, format!("tessaro-ctl script run {}", spec.name)),
        paint(style::MUTED, "output:"),
        paint(style::CMD, format!("tessaro-ctl script logs {}", spec.name))
    );
}
