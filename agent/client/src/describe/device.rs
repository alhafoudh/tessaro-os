//! The device itself: `tessaro-ctl device status` and `id`, `config set`
//! and `config keys`, `browser eval`, and the GUI's Overview, settings and
//! Browser pages.

use protocol::keys::Consumer;
use protocol::{Applied, EvalResult, Hardware, KeyInfo, NodeInfo, Pending, Status};
use serde_json::Value;

use crate::describe::{audio, playlist, screen, time};
use crate::storage::{free_line, usage_line};
use crate::text::{unit_state, usage_level, yes_no, Fact, Line, Tone};

/// What keeps a change that is on probation.
pub const CONFIRM_COMMAND: &str = "tessaro-ctl screen confirm";
/// What applies a change the firmware reads at power-on.
pub const REBOOT_COMMAND: &str = "tessaro-ctl device reboot";

/// Who the device is.
pub fn node(node: &NodeInfo) -> Vec<Fact> {
    vec![
        Fact::new("name", Line::of(Tone::Heading, &node.name)),
        Fact::new("node id", node.id.as_str()),
        Fact::new("machine", node.machine.as_str()),
        Fact::new("agent", node.version.as_str()),
        Fact::new("fingerprint", Line::of(Tone::Muted, &node.fingerprint)),
        Fact::new("claimed", yes_no(node.claimed)),
        Fact::new(
            "tags",
            tags(&crate::tags::effective(&node.tags, Some(node.claimed))),
        ),
    ]
}

/// A device's tags, comma separated: `unclaimed` as a warning, `(none)`
/// for no tag at all.
pub fn tags(tags: &[String]) -> Line {
    if tags.is_empty() {
        return Line::of(Tone::Muted, "(none)");
    }
    let mut line = Line::new();
    for (at, tag) in tags.iter().enumerate() {
        if at > 0 {
            line = line.add(Tone::Plain, ", ");
        }
        let tone = if crate::tags::is_reserved(tag) {
            Tone::Warn
        } else {
            Tone::Plain
        };
        line = line.add(tone, tag);
    }
    line
}

/// What the device is, one fact per thing the firmware says; a field it
/// does not say is left out.
pub fn hardware(hardware: &Hardware) -> Vec<Fact> {
    let mut facts = Vec::new();
    if let Some(machine) = hardware.machine() {
        facts.push(Fact::new("hardware", machine));
    }
    match (&hardware.board, &hardware.firmware) {
        (Some(board), Some(firmware)) => facts.push(Fact::new(
            "board",
            Line::plain(format!("{board} ")).add(Tone::Muted, format!("firmware {firmware}")),
        )),
        (Some(board), None) => facts.push(Fact::new("board", board.as_str())),
        (None, Some(firmware)) => facts.push(Fact::new("firmware", firmware.as_str())),
        (None, None) => {}
    }
    let arch = Line::of(Tone::Muted, &hardware.arch);
    facts.push(Fact::new(
        "cpu",
        match hardware.cpu_line() {
            Some(cpu) => Line::plain(format!("{cpu} ")).join(arch),
            None => arch,
        },
    ));
    if let Some(serial) = &hardware.serial {
        facts.push(Fact::new("serial", Line::of(Tone::Muted, serial)));
    }
    facts
}

/// `on - COMMAND WHAT`: a mode that is on, and how to end it.
fn on_until(command: &str, what: &str) -> Line {
    Line::of(Tone::Warn, "on")
        .text(" ")
        .add(Tone::Muted, "-")
        .text(" ")
        .add(Tone::Cmd, command)
        .text(" ")
        .add(Tone::Muted, what)
}

/// `device status` in its parts: the facts before the units, the units,
/// and the facts after them.
pub struct StatusText {
    pub facts: Vec<Fact>,
    pub units: Vec<Fact>,
    pub more: Vec<Fact>,
    /// A change on probation, and how to keep it.
    pub pending: Option<Line>,
}

pub fn status(status: &Status) -> StatusText {
    let mut facts = node(&status.node);
    if let Some(os) = &status.os {
        facts.push(Fact::new(
            "os",
            match &status.image_version {
                Some(version) => Line::plain(format!("{os}, "))
                    .add(Tone::Label, "image")
                    .text(format!(" {version}")),
                None => Line::plain(os),
            },
        ));
    }
    if let Some(hw) = &status.hardware {
        facts.extend(hardware(hw));
    }
    facts.push(Fact::new("revision", status.revision.to_string()));
    if let Some(percent) = status.cpu_percent {
        facts.push(Fact::new(
            "cpu use",
            Line::of(usage_level(percent.into()), format!("{percent}%")),
        ));
    }
    if let Some(memory) = &status.memory {
        facts.push(Fact::new(
            "memory",
            free_line(memory.available, memory.total, memory.used_percent()),
        ));
    }
    if let Some(data) = &status.data {
        facts.push(Fact::new("data", usage_line(data)));
    }
    if status.maintenance {
        facts.push(Fact::new(
            "maintenance",
            on_until(
                "tessaro-ctl browser maintenance off",
                "returns to browser.url",
            ),
        ));
    }
    if status.debug_screen {
        facts.push(Fact::new(
            "debug screen",
            on_until("tessaro-ctl browser debug off", "returns to the page below"),
        ));
    }
    facts.push(Fact::new("browser url", status.kiosk_url.as_str()));
    facts.push(Fact::new(
        "showing",
        match &status.current_url {
            Some(url) => Line::plain(url),
            None => Line::of(Tone::Warn, "(cannot tell)"),
        },
    ));
    facts.push(Fact::new(
        "browser",
        if status.browser_answering {
            Line::of(Tone::Ok, "answering")
        } else {
            Line::of(Tone::Bad, "not answering")
        },
    ));
    if status.devtools {
        facts.push(Fact::new(
            "devtools",
            Line::of(Tone::Warn, "connected").text(" ").add(
                Tone::Muted,
                "- the agent leaves the tab alone until it disconnects",
            ),
        ));
    }

    let units = status
        .units
        .iter()
        .map(|(unit, state)| Fact::new(unit.as_str(), Line::of(unit_state(state), state)))
        .collect();

    let mut more = Vec::new();
    if let Some(player) = status.playlist.as_ref().filter(|player| player.player) {
        more.push(Fact::new("playlist", playlist::summary(player)));
    }
    if let Some(summary) = &status.audio {
        more.push(Fact::new("audio", audio::summary(summary)));
    }
    if let Some(summary) = &status.time {
        more.push(Fact::new("time", time::summary(summary)));
    }
    if status.screen_on == Some(false) {
        more.push(Fact::new(
            "screen",
            Line::of(Tone::Warn, "off")
                .text(" ")
                .add(Tone::Muted, "-")
                .text(" ")
                .add(Tone::Cmd, "tessaro-ctl screen power on"),
        ));
    }
    if let Some(tv) = &status.tv {
        more.push(Fact::new("tv", screen::tv(tv)));
    }
    if let Some(vnc) = status.vnc.filter(|vnc| vnc.sharing) {
        more.push(Fact::new("vnc", screen::vnc(&vnc)));
    }
    if let Some(bridge) = &status.bridge {
        if bridge.mode != "off" {
            more.push(Fact::new("page bridge", bridge.mode.as_str()));
        }
        if !bridge.script.is_empty() {
            let state = match &bridge.script_problem {
                Some(problem) => Line::of(Tone::Bad, format!("not injected: {problem}")),
                None => Line::of(Tone::Ok, "injected"),
            };
            more.push(Fact::new(
                "inject",
                Line::plain(format!("{} ", bridge.script)).join(state),
            ));
        }
    }

    let pending = status.pending.as_ref().map(|pending| {
        Line::of(Tone::Warn, "on probation")
            .text(format!(" {} - ", pending_values(pending)))
            .add(Tone::Cmd, format!("`{CONFIRM_COMMAND}`"))
            .text(format!(
                " within {}s or {}",
                pending.seconds_left,
                pending_back(pending, false)
            ))
    });
    StatusText {
        facts,
        units,
        more,
        pending,
    }
}

/// What a `config set` or `config unset` did: what changed, what it
/// restarted, and what is left to do. `no_apply` when it was saved only.
pub fn applied(applied: &Applied, no_apply: bool) -> Vec<Line> {
    if applied.changed.is_empty() {
        return vec![Line::of(
            Tone::Muted,
            format!("nothing changed (revision {})", applied.revision),
        )];
    }
    let mut lines = vec![
        Line::of(Tone::Muted, format!("revision {}:", applied.revision))
            .text(" ")
            .add(Tone::Ok, applied.changed.join(", ")),
    ];
    if let Some(sound) = &applied.audio {
        lines.extend(audio::outcome(sound));
    }
    if let Some(clock) = &applied.time {
        lines.push(time::outcome(clock));
    }
    if no_apply {
        lines.push(Line::of(Tone::Muted, "saved; nothing restarted"));
    } else if applied.restarted.is_empty() {
        if applied.audio.is_none() && applied.time.is_none() && !applied.reboot {
            lines.push(Line::of(Tone::Muted, "nothing to restart"));
        }
    } else {
        lines.push(Line::of(
            Tone::Warn,
            format!("restarting {}", applied.restarted.join(", ")),
        ));
    }
    if applied.reboot {
        lines.push(
            Line::of(Tone::Warn, "takes effect at the next reboot:")
                .text(" ")
                .add(Tone::Cmd, REBOOT_COMMAND),
        );
    }
    if let Some(pending) = &applied.pending {
        let is = if pending.changes.len() == 1 {
            "is"
        } else {
            "are"
        };
        lines.push(Line::new());
        lines.push(
            Line::of(
                Tone::Warn,
                format!("{} {is} on probation.", pending_values(pending)),
            )
            .text(" Check the screen, then run"),
        );
        lines.push(Line::new());
        lines.push(Line::plain("    ").add(Tone::Cmd, CONFIRM_COMMAND));
        lines.push(Line::new());
        lines.push(Line::plain(format!(
            "within {}s, or {}.",
            pending.seconds_left,
            pending_back(pending, true)
        )));
    }
    lines
}

/// The changes on probation in a status bar, `left` seconds before they
/// revert: `a=1 reverts to 0 in 42s`, or `a=1, b=2 revert in 42s`.
pub fn reverting(pending: &Pending, left: u64) -> String {
    match pending.changes.as_slice() {
        [change] => format!(
            "{}={} reverts to {} in {left}s",
            change.key,
            change.value,
            change.previous_or_default()
        ),
        _ => format!("{} revert in {left}s", pending_values(pending)),
    }
}

/// `a=1, b=2`: the changes on probation.
fn pending_values(pending: &Pending) -> String {
    pending
        .changes
        .iter()
        .map(|change| format!("{}={}", change.key, change.value))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a revert of them does: `it goes back to 1`, or for more than one
/// `they go back: a to 1, b to the default`, with `on its own` or `on their
/// own` when `alone`.
fn pending_back(pending: &Pending, alone: bool) -> String {
    match pending.changes.as_slice() {
        [change] => format!(
            "it goes back to {}{}",
            change.previous_or_default(),
            if alone { " on its own" } else { "" }
        ),
        changes => format!(
            "they go back{}: {}",
            if alone { " on their own" } else { "" },
            changes
                .iter()
                .map(|change| format!("{} to {}", change.key, change.previous_or_default()))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// What a change of a key read by `consumer` restarts, in words.
pub fn restarts(consumer: Consumer) -> &'static str {
    match consumer {
        Consumer::Agent => "nothing: the agent applies it at once",
        Consumer::AgentRestart => "the agent (it loads the page again)",
        Consumer::Browser => "the browser",
        Consumer::Weston => "the display (Weston and the browser)",
        Consumer::Network => {
            "nothing: the network profiles are switched, and checked before it is saved"
        }
        Consumer::Audio => "nothing: applied to the sound server at once",
        Consumer::Firmware => "nothing: the Pi firmware reads it at the next reboot",
        Consumer::Time => {
            "nothing on screen: applied to the clock at once; systemd-timesyncd when its servers change"
        }
        Consumer::Proxy => {
            "the local proxy (tessaro-proxy.service); the browser and the agent when the proxy is switched on or off"
        }
        Consumer::Camera => {
            "the camera mirrors (tessaro-camera@*.service); a page showing a camera asks for it again"
        }
        Consumer::Vision => "presence detection (tessaro-vision.service)",
    }
}

/// The same in a word or two, for a table column.
pub fn restarts_short(consumer: Consumer) -> &'static str {
    match consumer {
        Consumer::Agent => "agent (live)",
        Consumer::AgentRestart => "agent",
        Consumer::Browser => "browser",
        Consumer::Weston => "weston",
        Consumer::Network => "network",
        Consumer::Audio => "audio",
        Consumer::Firmware => "firmware (next reboot)",
        Consumer::Time => "clock",
        Consumer::Proxy => "local proxy (browser, agent on switching)",
        Consumer::Camera => "camera mirrors",
        Consumer::Vision => "presence detection",
    }
}

/// One key of `config keys`: its doc, value, what it accepts and restarts.
pub fn key(key: &KeyInfo) -> Vec<Line> {
    // Nothing reads a read-only key, so it is the one kind that restarts
    // nothing - and its value is reported, never set.
    let read_only = key.applies.is_empty();
    let current = match (&key.value, &key.default) {
        (Some(value), _) if read_only => {
            let shown = if value.is_empty() {
                Line::of(Tone::Muted, "(none)")
            } else {
                Line::plain(value)
            };
            shown
                .text("  ")
                .add(Tone::Muted, "(read-only, reported by the device)")
        }
        (Some(value), _) => Line::of(Tone::Ok, value).text("  ").add(Tone::Ok, "(set)"),
        (None, Some(default)) => Line::plain(format!("{default}  ")).add(Tone::Muted, "(default)"),
        (None, None) => Line::of(Tone::Muted, "(not set)"),
    };
    let row = |label: &str, value: Line| {
        Line::plain("    ")
            .pad(Tone::Label, label, 9)
            .text(" ")
            .join(value)
    };
    let mut lines = vec![
        Line::of(Tone::Heading, &key.name),
        Line::plain(format!("    {}", key.doc)),
        row("value", current),
    ];
    if key.value.is_some() {
        if let Some(default) = &key.default {
            lines.push(row("default", Line::plain(default)));
        }
    }
    lines.push(row("accepts", Line::plain(&key.values)));
    if !read_only {
        let restarts = key
            .applies
            .iter()
            .map(|consumer| restarts(*consumer))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(row("restarts", Line::plain(restarts)));
    }
    if key.guarded {
        lines.push(row(
            "note",
            Line::of(
                Tone::Warn,
                format!(
                    "applied on probation: `{CONFIRM_COMMAND}` within {}s or it reverts",
                    protocol::CONFIRM_SECONDS
                ),
            ),
        ));
    }
    if !key.env.is_empty() {
        lines.push(row("env", Line::of(Tone::Muted, &key.env)));
    }
    lines
}

/// What `browser eval` came to: the value as JSON, what JavaScript calls it
/// when JSON cannot hold it, or the exception with where it was thrown
/// (`Err`, for the error stream).
pub fn eval(result: &EvalResult) -> Result<Line, Line> {
    if let Some(exception) = &result.exception {
        return Err(Line::of(Tone::Bad, &exception.text).text(" ").add(
            Tone::Muted,
            format!("(line {}, column {})", exception.line, exception.column),
        ));
    }
    Ok(match (&result.value, &result.description) {
        (Some(Value::String(text)), _) => Line::plain(text),
        // Chromium hands a function or a DOM node over as `{}`: its type
        // says more.
        (Some(Value::Object(map)), _) if map.is_empty() && result.kind != "object" => {
            Line::of(Tone::Muted, format!("({})", result.kind))
        }
        (Some(value), _) => {
            Line::plain(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
        }
        (None, Some(description)) => {
            Line::plain(format!("{description} ")).add(Tone::Muted, format!("({})", result.kind))
        }
        (None, None) => Line::of(Tone::Muted, &result.kind),
    })
}

/// A secret shown once, to be written down: a root password, a token.
pub fn once(intro: &str, secret: &str) -> Vec<Line> {
    vec![
        Line::of(Tone::Warn, intro),
        Line::new(),
        Line::plain("    ").add(Tone::Secret, secret),
        Line::new(),
    ]
}
