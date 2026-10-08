//! HDMI-CEC actions, for `tessaro-ctl screen cec` and the Screen pages: a
//! request built from what was typed - a key's name, an address, bytes in
//! hex - and the device's message log followed as it grows.

use std::time::{Duration, Instant};

use protocol::api::{self, CecKeyBody, CecMessagesQuery, CecSendBody};
use protocol::{CecMessage, CecMessages};

use crate::connect::Session;

/// How often a followed message log is asked for what is new.
pub const MESSAGES_POLL: Duration = Duration::from_secs(1);

/// What was typed for an address, `tv`, `audio`, `all` or 0 to 15, or
/// nothing for `default`.
fn address(typed: &str, default: Option<u8>) -> Result<Option<u8>, String> {
    match typed.trim() {
        "" => Ok(default),
        typed => protocol::cec::parse_address(typed).map(Some),
    }
}

/// What the connector was typed as; nothing for every adapter.
fn connector(typed: &str) -> Option<String> {
    Some(typed.trim().to_string()).filter(|connector| !connector.is_empty())
}

/// `screen cec key NAME [--to ADDRESS]`: the key checked by name, to the TV
/// unless an address was typed.
pub fn key_body(key: &str, to: &str, on: &str) -> Result<CecKeyBody, String> {
    let code = protocol::cec::key_code(key)?;
    let name = protocol::cec::key_name(code);
    Ok(CecKeyBody {
        key: name,
        to: address(to, None)?,
        connector: connector(on),
    })
}

/// `screen cec send HEX --to ADDRESS [--reply OPCODE]`.
pub fn send_body(data: &str, to: &str, reply: &str, on: &str) -> Result<CecSendBody, String> {
    let bytes = protocol::cec::parse_data(data)?;
    let to = address(to, None)?.ok_or_else(|| {
        "a message needs an address to go to: tv, audio, all, or 0 to 15".to_string()
    })?;
    let reply = match reply.trim() {
        "" => None,
        typed => match protocol::cec::parse_data(typed)?.as_slice() {
            [opcode] => Some(*opcode),
            _ => return Err("a reply is one opcode in hex, e.g. 90".to_string()),
        },
    };
    Ok(CecSendBody {
        to,
        data: protocol::cec::hex(&bytes),
        reply,
        connector: connector(on),
    })
}

/// `screen cec wake [--no-source]`.
pub fn wake_body(source: bool, on: &str) -> api::CecWakeBody {
    api::CecWakeBody {
        source,
        connector: connector(on),
    }
}

/// `screen cec standby [--all]`.
pub fn standby_body(all: bool, on: &str) -> api::CecStandbyBody {
    api::CecStandbyBody {
        all,
        connector: connector(on),
    }
}

/// The bare connector body, for `source` and `scan`.
pub fn on(typed: &str) -> api::CecBody {
    api::CecBody {
        connector: connector(typed),
    }
}

/// The message log after `after`, and with `follow` every message after
/// it as it comes, until `stop` says so.
pub fn messages(
    session: &mut Session,
    mut after: u64,
    follow: bool,
    stop: &dyn Fn() -> bool,
    mut each: impl FnMut(&CecMessage),
) -> Result<(), String> {
    loop {
        let page: CecMessages =
            session.call::<api::screen::CecMessages>(CecMessagesQuery { after }, ())?;
        page.messages.iter().for_each(&mut each);
        if !follow {
            return Ok(());
        }
        after = page.next;
        let waited = Instant::now();
        while waited.elapsed() < MESSAGES_POLL {
            if stop() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_goes_to_the_tv_unless_told_otherwise() {
        let body = key_body("Volume-Up", "", "").unwrap();
        assert_eq!(
            (body.key.as_str(), body.to, body.connector),
            ("volume-up", None, None)
        );
        let body = key_body("mute", "audio", "HDMI-A-2").unwrap();
        assert_eq!(
            (body.to, body.connector.as_deref()),
            (Some(5), Some("HDMI-A-2"))
        );
        assert!(key_body("nope", "", "").is_err());
        assert!(key_body("mute", "17", "").is_err());
    }

    #[test]
    fn a_message_needs_bytes_an_address_and_at_most_one_reply_opcode() {
        let body = send_body("0x8F", "tv", "90", "").unwrap();
        assert_eq!(
            (body.data.as_str(), body.to, body.reply),
            ("8f", 0, Some(0x90))
        );
        assert!(send_body("8f", "", "", "").is_err());
        assert!(send_body("8f", "tv", "90 91", "").is_err());
        assert!(send_body("zz", "tv", "", "").is_err());
    }
}
