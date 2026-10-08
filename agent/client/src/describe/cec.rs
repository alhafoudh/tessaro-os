//! HDMI-CEC actions and the message log: what `tessaro-ctl screen cec`, the
//! Screen pages and their CEC consoles say about what was sent, what came
//! back and what goes over the bus.

use protocol::cec::{key_name, kind, opcode_name, BROADCAST, TV};
use protocol::{CecActed, CecAdapterActed, CecDirection, CecMessage, CecMessages, CecSent};

use crate::describe::screen::power;
use crate::text::{Line, Tone};

/// A logical address in words: `TV`, `playback 4`, `everyone` as a
/// destination, `unregistered` as a sender.
pub fn address(address: u8, to: bool) -> String {
    match address {
        TV => "TV".to_string(),
        BROADCAST if to => "everyone".to_string(),
        BROADCAST => "unregistered".to_string(),
        other => format!("{} {other}", kind(other)),
    }
}

/// The opcode and its operands in words: `standby (36)`,
/// `user-control-pressed (44) 41 volume-up`, `poll`.
pub fn data(data: &str) -> String {
    let bytes: Vec<u8> = protocol::cec::parse_data(data).unwrap_or_default();
    let Some((opcode, operands)) = bytes.split_first() else {
        return "poll".to_string();
    };
    let mut words = match opcode_name(*opcode) {
        Some(name) => format!("{name} ({opcode:02x})"),
        None => format!("opcode {opcode:02x}"),
    };
    if !operands.is_empty() {
        words.push(' ');
        words.push_str(&protocol::cec::hex(operands));
    }
    if let ([0x44], [code, ..]) = (&[*opcode][..], operands) {
        words.push(' ');
        words.push_str(&key_name(*code));
    }
    words
}

/// Whether a sent message was taken: a broadcast has nobody to say so.
fn taken(to: u8, acked: bool) -> Line {
    match (to, acked) {
        (BROADCAST, _) => Line::of(Tone::Muted, "sent"),
        (_, true) => Line::of(Tone::Ok, "acknowledged"),
        (_, false) => Line::of(Tone::Warn, "not acknowledged"),
    }
}

/// One message of the log, in columns: when, which way, who to whom, what.
pub fn message(message: &CecMessage) -> Line {
    let (way, tone) = match message.direction {
        CecDirection::In => ("in", Tone::Plain),
        CecDirection::Out => ("out", Tone::Label),
    };
    let route = format!(
        "{} -> {}",
        address(message.from, false),
        address(message.to, true)
    );
    let mut line = Line::new()
        .pad(Tone::Muted, &message.time, 13)
        .pad(tone, way, 4)
        .pad(Tone::Plain, route, 26)
        .text(data(&message.data));
    if let Some(acked) = message.acked {
        line = line.text(", ").join(taken(message.to, acked));
    }
    line
}

/// A page of the log, oldest first.
pub fn messages(page: &CecMessages) -> Vec<Line> {
    if page.messages.is_empty() {
        return vec![Line::of(Tone::Muted, "no messages yet")];
    }
    page.messages.iter().map(message).collect()
}

fn sent(sent: &CecSent) -> Line {
    Line::plain(format!(
        "    {} to {}, ",
        data(&sent.data),
        address(sent.to, true)
    ))
    .join(taken(sent.to, sent.acked))
}

fn adapter(action: &str, acted: &CecAdapterActed) -> Vec<Line> {
    let on = acted.connector.as_deref().unwrap_or(&acted.device);
    let mut lines = vec![Line::of(Tone::Heading, format!("{action} on {on}"))];
    if let Some(error) = &acted.error {
        lines.push(Line::plain("    ").add(Tone::Bad, error));
    }
    lines.extend(acted.sent.iter().map(sent));
    if action == "scan" && acted.error.is_none() {
        lines.push(if acted.answered.is_empty() {
            Line::plain("    ").add(Tone::Warn, "nobody answered")
        } else {
            Line::plain(format!(
                "    answered: {}",
                acted
                    .answered
                    .iter()
                    .map(|answered| address(*answered, true))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        });
    }
    if let Some(reply) = &acted.reply {
        lines.push(Line::plain("    reply: ").text(format!(
            "{} from {}",
            data(&reply.data),
            address(reply.from, false)
        )));
    }
    if let Some(tv) = acted.tv {
        lines.push(Line::plain("    the TV is ").join(power(tv)));
    }
    lines
}

/// What an action did on every adapter it went out on.
pub fn acted(acted: &CecActed) -> Vec<Line> {
    if acted.adapters.is_empty() {
        return vec![Line::of(
            Tone::Warn,
            format!("{}: no adapter answered in time", acted.action),
        )];
    }
    acted
        .adapters
        .iter()
        .flat_map(|one| adapter(&acted.action, one))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_opcodes_read_as_words() {
        assert_eq!(address(0, true), "TV");
        assert_eq!(address(4, false), "playback 4");
        assert_eq!(address(15, true), "everyone");
        assert_eq!(address(15, false), "unregistered");
        assert_eq!(data("36"), "standby (36)");
        assert_eq!(data("44 41"), "user-control-pressed (44) 41 volume-up");
        assert_eq!(data(""), "poll");
        assert_eq!(data("30"), "opcode 30");
    }
}
