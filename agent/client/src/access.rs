//! Becoming a device's owner, or one of them, for `tessaro-ctl access
//! claim|login`, the GUI's node list and its Access page alike. What this
//! machine keeps about it afterwards (`Nodes::remember`) is the caller's.

use protocol::api::{self, Empty, NameBody};
use protocol::Claimed;

use crate::connect::{Answer, Session};
use crate::text::{Line, Tone};

/// What `access unclaim` loses, as the end of "This will ...". The
/// settings stay.
pub const UNCLAIM_LOSES: &str = "remove every token and ssh key and empty the root password";

/// What `device factory-reset` loses, as the end of "This will ...". The
/// device comes back unclaimed.
pub const FACTORY_RESET_LOSES: &str =
    "erase every setting, remove every token and ssh key and empty the root password";

/// Who is claiming, as typed; `user@host` when nothing was.
pub fn claimer(typed: &str) -> String {
    match typed.trim() {
        "" => crate::client_name(),
        name => name.to_string(),
    }
}

/// Claim the device on `session` as `name`. A token left over from before
/// an unclaim means nothing now, so none is sent; the session carries the
/// new one afterwards.
pub fn claim(session: &mut Session, name: &str) -> Answer<Claimed> {
    session.clear_token();
    let answer = session.request::<api::access::Claim>(
        Empty {},
        NameBody {
            name: claimer(name),
        },
    );
    if let Answer::Ok(claimed) = &answer {
        session.set_token(claimed.token.clone());
    }
    answer
}

/// Use `token` on `session`, proven by asking the device for its tokens
/// before anything keeps it.
pub fn login(session: &mut Session, token: &str) -> Result<(), String> {
    session.set_token(token.to_string());
    session.fetch::<api::access::Tokens>().map(drop)
}

/// `claimed NAME (id)`, or `logged in to NAME (id)`: the verb, the device,
/// the detail.
pub fn done(verb: &str, session: &Session) -> Line {
    Line::of(Tone::Ok, verb)
        .text(" ")
        .add(Tone::Heading, &session.node.name)
        .text(" ")
        .add(Tone::Muted, format!("({})", session.node.id))
}

/// What to show once after a claim: the new root password, and the
/// hotspot's when the device has one.
pub fn secrets(claimed: &Claimed) -> Vec<(String, String)> {
    let mut secrets = vec![(
        "root password - shown this once, store it now:".to_string(),
        claimed.root_password.clone(),
    )];
    if let Some(hotspot) = &claimed.hotspot {
        secrets.push((
            format!(
                "hotspot {} password - shown this once; anyone on the hotspot now is dropped:",
                hotspot.ssid
            ),
            hotspot.password.clone(),
        ));
    }
    secrets
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_claimer_is_this_machine() {
        assert_eq!(claimer("  "), crate::client_name());
        assert_eq!(claimer(" kiosk-admin "), "kiosk-admin");
    }
}
