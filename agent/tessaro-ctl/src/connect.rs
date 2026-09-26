//! tessaro-client's connection, the way a terminal wants it: a certificate
//! seen for the first time is shown and asked about at the keyboard, and the
//! client's notes are printed as warnings.
//!
//! How a device is found and pinned is in `tessaro_client::connect`.

use anstream::{eprintln, println};
use protocol::api::Endpoint;
use protocol::JobStarted;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tessaro_client::connect::{self as client, PinAsk};
pub use tessaro_client::connect::{browse, resolve, Answer, Session, Target};
use tessaro_client::nodes::Nodes;

use crate::style::{self, paint};

/// How this client names itself to the device (its User-Agent).
const CLIENT: &str = concat!("tessaro-ctl ", env!("CARGO_PKG_VERSION"));

/// What the command wants to happen if the node has no pin yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Refuse: only known nodes.
    KnownOnly,
    /// Allowed without pinning (`id`): nothing secret is sent.
    Peek,
    /// Pin it, after the user accepted the fingerprint (`claim`, `login`).
    Pin { assume_yes: bool },
}

pub fn open(target: &Target, nodes: &Nodes, trust: Trust) -> Result<Session, String> {
    let mut decide = |ask: &PinAsk| {
        eprintln!(
            "{} ({}) presents certificate\n  {}",
            ask.label,
            ask.address,
            paint(style::HEADING, &ask.fingerprint)
        );
        let assume_yes = matches!(trust, Trust::Pin { assume_yes: true });
        Ok(assume_yes || crate::prompt::ask("Pin it and continue?")?)
    };
    let mut trust = match trust {
        Trust::KnownOnly => client::Trust::KnownOnly,
        Trust::Peek => client::Trust::Peek,
        Trust::Pin { .. } => client::Trust::Pin(&mut decide),
    };
    let session = client::open(target, nodes, &mut trust, CLIENT)?;
    let warning = paint(style::WARN, "warning:");
    for note in &session.notes {
        eprintln!("{warning} {note}");
    }
    Ok(session)
}

/// Job `S` started and followed to its end, its steps printed the way every
/// job's are: with `--json` each step as the device sent it, otherwise
/// through `each` - and a step this client does not know, from a newer
/// device, as it came. Only Ctrl-C stops it early, which ends the process.
pub fn follow_job<S, T>(
    session: &mut Session,
    body: S::Body,
    json: bool,
    mut each: impl FnMut(T),
) -> Result<(), String>
where
    S: Endpoint<Response = JobStarted>,
    S::Params: Default,
    T: DeserializeOwned,
{
    if json {
        return session.job::<S, Value>(body, &|| false, |step| match step {
            Ok(raw) | Err(raw) => println!("{raw}"),
        });
    }
    session.job::<S, T>(body, &|| false, |step| match step {
        Ok(step) => each(step),
        Err(raw) => println!("{raw}"),
    })
}
