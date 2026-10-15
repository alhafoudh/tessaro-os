//! The device's link to its MQTT broker: `tessaro-ctl mqtt status` and
//! `mqtt test`, the MQTT page, and the line of `device status`. Ported to
//! Webconfig (`webconfig/src/describe/mqtt.ts`).

use protocol::mqtt::{MqttState, MqttStatus, MqttTested};

use crate::schedule::duration;
use crate::text::{Fact, Line, Tone};

/// The state in a word, toned.
fn state(status: &MqttStatus) -> Line {
    match status.state {
        MqttState::Off => Line::of(Tone::Muted, "off"),
        MqttState::Connecting => Line::of(Tone::Warn, "connecting"),
        MqttState::Connected => Line::of(Tone::Ok, "connected"),
        MqttState::Failed => Line::of(Tone::Bad, "not connected"),
    }
}

/// The one line `device status` shows: `connected to mqtts://b:8883`.
pub fn summary(status: &MqttStatus) -> Line {
    let line = state(status);
    match (&status.broker, status.state) {
        (_, MqttState::Off) => line.add(Tone::Muted, ", mqtt.url is not set"),
        (Some(broker), MqttState::Connected) => line.text(" to ").add(Tone::Heading, broker),
        (Some(broker), _) => {
            let line = line.text(" to ").add(Tone::Heading, broker);
            match &status.last_error {
                Some(error) => line.text(format!(": {error}")),
                None => line,
            }
        }
        (None, _) => line,
    }
}

/// The link, a fact a row.
pub fn facts(status: &MqttStatus) -> Vec<Fact> {
    let mut facts = Vec::new();
    let mut now = state(status);
    if let Some(seconds) = status.seconds {
        now = now.add(Tone::Muted, format!(" for {}", duration(seconds)));
    }
    facts.push(Fact::new("state", now));
    if status.state == MqttState::Off {
        facts.push(Fact::new(
            "",
            Line::of(Tone::Muted, "set mqtt.url to connect: ")
                .add(Tone::Cmd, "tessaro-ctl config set mqtt.url=mqtts://..."),
        ));
    }
    if let Some(broker) = &status.broker {
        facts.push(Fact::new("broker", Line::of(Tone::Heading, broker)));
    }
    if let Some(error) = &status.last_error {
        facts.push(Fact::new("last error", Line::of(Tone::Bad, error)));
    }
    facts.push(Fact::new("client id", Line::plain(&status.client_id)));
    facts.push(Fact::new(
        "user",
        match &status.username {
            Some(user) => Line::plain(user),
            None => Line::of(Tone::Muted, "none"),
        },
    ));
    facts.push(Fact::new(
        "password",
        match status.password {
            true => Line::of(Tone::Secret, "kept"),
            false => Line::of(Tone::Muted, "none"),
        },
    ));
    facts.push(Fact::new(
        "client cert",
        match status.client_cert {
            true => Line::plain("kept"),
            false => Line::of(Tone::Muted, "none"),
        },
    ));
    facts.push(Fact::new(
        "topics",
        Line::plain(format!("{}/...", status.topic)),
    ));
    facts.push(Fact::new(
        "commands",
        match status.commands.as_str() {
            "full" => Line::plain("full").add(Tone::Muted, ", every command"),
            "read" => Line::plain("read").add(Tone::Muted, ", only what reads"),
            other => Line::of(Tone::Muted, other),
        },
    ));
    for warning in &status.warnings {
        facts.push(Fact::new("", Line::of(Tone::Warn, warning)));
    }
    if status.state != MqttState::Off {
        facts.push(Fact::new(
            "messages",
            Line::plain(format!(
                "{} published, {} received",
                status.published, status.received
            ))
            .add(
                if status.queued > 0 {
                    Tone::Warn
                } else {
                    Tone::Muted
                },
                format!(", {} waiting", status.queued),
            ),
        ));
    }
    if !status.subscriptions.is_empty() {
        facts.push(Fact::new(
            "subscribed",
            Line::plain(status.subscriptions.join(" ")),
        ));
    }
    facts
}

/// What `mqtt test` found, a line a finding.
pub fn tested(tested: &MqttTested) -> Vec<Line> {
    let mut lines = vec![match (tested.ok, &tested.stage, &tested.error) {
        (true, _, _) => Line::of(Tone::Ok, "the broker took the connection")
            .text(" at ")
            .add(Tone::Heading, &tested.broker)
            .add(Tone::Muted, format!(" ({} ms)", tested.millis)),
        (false, stage, error) => Line::of(
            Tone::Bad,
            format!("stopped at {}:", stage.as_deref().unwrap_or("the link")),
        )
        .text(format!(
            " {}",
            error.as_deref().unwrap_or("no reason given")
        ))
        .add(Tone::Muted, format!(" ({})", tested.broker)),
    }];
    if let Some(fingerprint) = &tested.fingerprint {
        lines.push(
            Line::of(
                Tone::Muted,
                "its certificate's SHA-256, for mqtt.tls.fingerprint: ",
            )
            .add(Tone::Plain, fingerprint),
        );
    }
    lines
}
