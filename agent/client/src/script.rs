//! Scripts: a script built from what was typed, a run started and followed
//! to its end, and the words for scripts (`protocol::ScriptInfo`) and their
//! runs, shared by `tessaro-ctl script` and the GUI's Scripts page, and
//! ported to Webconfig (`webconfig/src/describe/script.ts`).

use protocol::api::{self, ScriptChange, ScriptRef};
use protocol::{Concurrency, OnError, ScriptEvent, ScriptInfo, ScriptRun, ScriptSpec};
use serde_json::Value;

use crate::connect::Session;
use crate::schedule::{duration, outcome, parse_timeout, relative, runs};
use crate::text::{Line, Tone};

/// A script's fields as a form or a command line has them, each as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typed {
    pub name: String,
    pub description: String,
    pub body: String,
    /// `stop` or `continue`.
    pub on_error: String,
    /// As `parse_timeout` reads it; empty or `none` for none.
    pub timeout: String,
    /// `overlap` or `skip`.
    pub concurrency: String,
    pub bridge: bool,
    /// The CEC events it runs on, comma separated, as `protocol::cec`
    /// reads them; empty for none.
    pub cec: String,
    /// The presence events it runs on, comma separated, as
    /// `protocol::presence` reads them; empty for none.
    pub presence: String,
}

impl Typed {
    /// What a form shows for `spec`: saving it unchanged changes nothing.
    pub fn of(spec: &ScriptSpec) -> Self {
        Self {
            name: spec.name.clone(),
            description: spec.description.clone(),
            body: spec.body.clone(),
            on_error: spec.on_error.name().to_string(),
            timeout: crate::schedule::format_timeout(spec.timeout_s.unwrap_or(0)),
            concurrency: spec.concurrency.name().to_string(),
            bridge: spec.bridge,
            cec: spec.cec.join(", "),
            presence: spec.presence.join(", "),
        }
    }

    /// The script to create.
    pub fn spec(&self) -> Result<ScriptSpec, String> {
        let timeout = self.timeout_s()?;
        Ok(ScriptSpec {
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            body: self.body.clone(),
            on_error: self.on_error()?,
            timeout_s: (timeout > 0).then_some(timeout),
            concurrency: self.concurrency()?,
            bridge: self.bridge,
            cec: protocol::cec::triggers(&self.cec)?,
            presence: protocol::presence::triggers(&self.presence)?,
        })
    }

    /// Every field of an existing script replaced with what was typed.
    pub fn change(&self) -> Result<ScriptChange, String> {
        Ok(ScriptChange {
            name: Some(self.name.trim().to_string()),
            description: Some(self.description.trim().to_string()),
            body: Some(self.body.clone()),
            on_error: Some(self.on_error()?),
            timeout_s: Some(self.timeout_s()?),
            concurrency: Some(self.concurrency()?),
            bridge: Some(self.bridge),
            cec: Some(protocol::cec::triggers(&self.cec)?),
            presence: Some(protocol::presence::triggers(&self.presence)?),
        })
    }

    fn on_error(&self) -> Result<OnError, String> {
        match self.on_error.trim() {
            "" => Ok(OnError::default()),
            name => name.parse(),
        }
    }

    fn concurrency(&self) -> Result<Concurrency, String> {
        match self.concurrency.trim() {
            "" => Ok(Concurrency::default()),
            name => name.parse(),
        }
    }

    fn timeout_s(&self) -> Result<u64, String> {
        match self.timeout.trim() {
            "" => Ok(0),
            text => parse_timeout(text),
        }
    }
}

/// Run script `script` now and follow it to its end: `each` gets every step
/// as it comes (a step this client does not know from a newer device is
/// left out). How it ended, or `None` when `stop` said to stop following
/// first; the run itself goes on then.
pub fn run(
    session: &mut Session,
    script: &str,
    stop: &dyn Fn() -> bool,
    mut each: impl FnMut(&ScriptEvent),
) -> Result<Option<ScriptRun>, String> {
    let mut ended = None;
    session.job_at::<api::script::Run, ScriptEvent>(
        ScriptRef {
            script: script.to_string(),
        },
        (),
        stop,
        |step: Result<ScriptEvent, Value>| {
            if let Ok(event) = step {
                if let ScriptEvent::Ended { record } = &event {
                    ended = Some(record.clone());
                }
                each(&event);
            }
        },
    )?;
    Ok(ended)
}

/// How the script's last run went and when, and how many run now.
pub fn last_run(info: &ScriptInfo, now: i64) -> Line {
    runs(info.runs.first(), info.running, now)
}

/// Who started a run: `by hand`, `from the page`, `by schedule night`,
/// `by the TV going to standby`, `by the remote's red key`, `by someone
/// arriving`.
pub fn started_by(run: &ScriptRun) -> String {
    match (run.trigger.as_str(), &run.schedule) {
        ("manual", _) => "by hand".to_string(),
        ("bridge", _) => "from the page".to_string(),
        ("schedule", Some(name)) => format!("by schedule {name}"),
        ("schedule", None) => "by a removed schedule".to_string(),
        ("cec", _) => match run.event.as_deref() {
            Some(event) => format!("by {}", cec_event(event)),
            None => "by HDMI-CEC".to_string(),
        },
        ("presence", _) => match run.event.as_deref() {
            Some(event) => format!("by {}", presence_event(event)),
            None => "by presence detection".to_string(),
        },
        (other, _) => format!("by {other}"),
    }
}

/// A presence event a script runs on, in words: `someone arriving`.
pub fn presence_event(event: &str) -> String {
    match event {
        "arrived" => "someone arriving".to_string(),
        "left" => "everyone leaving".to_string(),
        "near" => "someone coming near".to_string(),
        "far" => "everyone near stepping back".to_string(),
        other => other.to_string(),
    }
}

/// A CEC event a script runs on, in words: `the TV switching on`, `the
/// remote's red key`, `any remote key`.
pub fn cec_event(event: &str) -> String {
    match event {
        "tv-on" => "the TV switching on".to_string(),
        "tv-standby" => "the TV going to standby".to_string(),
        "source-gained" => "the TV switching to this device".to_string(),
        "source-lost" => "the TV switching away".to_string(),
        "key" => "any remote key".to_string(),
        other => match other.strip_prefix("key:") {
            Some(key) => format!("the remote's {key} key"),
            None => other.to_string(),
        },
    }
}

/// One finished run in a list: `exit-code 1, 3min ago, by hand (2s)`.
pub fn run_line(run: &ScriptRun, now: i64) -> Line {
    let took = run.finished.unix.saturating_sub(run.started.unix).max(0) as u64;
    Line::of(
        if run.succeeded() { Tone::Ok } else { Tone::Bad },
        outcome(run),
    )
    .add(
        Tone::Plain,
        format!(
            ", {}, {}",
            relative(run.finished.unix, now),
            started_by(run)
        ),
    )
    .add(Tone::Muted, format!(" ({})", duration(took)))
}

/// What a followed run's step says, as `tessaro-ctl script run` prints it:
/// an output line as it is, the start and the end around it. `name` is the
/// script's, for where the rest of a cut output is.
pub fn event_line(event: &ScriptEvent, name: &str) -> Line {
    match event {
        ScriptEvent::Started { run } => Line::of(Tone::Muted, format!("run {run} started")),
        ScriptEvent::Line { text } => Line::plain(text.clone()),
        ScriptEvent::Cut => Line::of(
            Tone::Warn,
            format!(
                "output cut at {} lines; `tessaro-ctl script logs {name}` has the rest",
                protocol::SCRIPT_OUTPUT_MAX
            ),
        ),
        ScriptEvent::Ended { record } => ended(record),
    }
}

/// How a followed run ended: `run manual-... succeeded after 2s`.
pub fn ended(run: &ScriptRun) -> Line {
    let took = duration(run.finished.unix.saturating_sub(run.started.unix).max(0) as u64);
    if run.succeeded() {
        Line::of(Tone::Ok, format!("run {} succeeded after {took}", run.run))
    } else {
        Line::of(
            Tone::Bad,
            format!("run {} failed: {}, after {took}", run.run, outcome(run)),
        )
    }
}

/// How a script's runs behave, in a few words: `stop on error, skip if
/// running, 10min timeout, page`.
pub fn behaviour(spec: &ScriptSpec) -> String {
    let mut words = vec![
        match spec.on_error {
            OnError::Stop => "stop on error",
            OnError::Continue => "continue on error",
        }
        .to_string(),
        match spec.concurrency {
            Concurrency::Overlap => "overlap",
            Concurrency::Skip => "skip if running",
        }
        .to_string(),
    ];
    if let Some(seconds) = spec.timeout_s {
        words.push(format!("{} timeout", duration(seconds)));
    }
    if spec.bridge {
        words.push("page may run it".to_string());
    }
    if !spec.cec.is_empty() {
        words.push(format!("runs on {}", spec.cec.join(" ")));
    }
    if !spec.presence.is_empty() {
        words.push(format!("runs on presence {}", spec.presence.join(" ")));
    }
    words.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_fields_make_a_spec_and_read_back_the_same() {
        let typed = Typed {
            name: " dim ".into(),
            description: "screen off ".into(),
            body: "true\n".into(),
            on_error: "continue".into(),
            timeout: "10m".into(),
            concurrency: "skip".into(),
            bridge: true,
            cec: "tv-standby, Key:Red".into(),
            presence: "Arrived".into(),
        };
        let spec = typed.spec().unwrap();
        assert_eq!(spec.name, "dim");
        assert_eq!(spec.cec, vec!["tv-standby", "key:red"]);
        assert_eq!(spec.presence, vec!["arrived"]);
        assert!(behaviour(&spec).ends_with("runs on presence arrived"));
        assert_eq!(spec.timeout_s, Some(600));
        assert_eq!(spec.concurrency, Concurrency::Skip);
        assert_eq!(Typed::of(&spec).spec().unwrap(), spec);
        assert_eq!(typed.change().unwrap().timeout_s, Some(600));

        let plain = Typed {
            name: "x".into(),
            body: "true".into(),
            ..Typed::default()
        };
        let spec = plain.spec().unwrap();
        assert_eq!((spec.on_error, spec.timeout_s), (OnError::Stop, None));
        assert_eq!(plain.change().unwrap().timeout_s, Some(0));
        let bad = Typed {
            concurrency: "sometimes".into(),
            ..plain
        };
        assert!(bad.spec().is_err());
    }
}
