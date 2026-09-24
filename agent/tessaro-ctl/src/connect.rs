//! tessaro-client's connection, the way a terminal wants it: a certificate
//! seen for the first time is shown and asked about at the keyboard, and the
//! client's notes are printed as warnings.
//!
//! How a device is found and pinned is in `tessaro_client::connect`.

use anstream::{eprintln, println};
use protocol::Command;
use serde::de::DeserializeOwned;
use tessaro_client::connect::{self as client, PinAsk};
pub use tessaro_client::connect::{browse, resolve, Answer, Session, Target};
use tessaro_client::nodes::Nodes;

use crate::style::{self, paint};

/// How this client names itself in the hello.
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

/// Streams printed the way every stream is.
pub trait StreamEvents {
    /// With `--json` each event as the device sent it, otherwise through
    /// `each` - and an event this client does not know, from a newer
    /// device, as it came.
    fn stream_events<E: DeserializeOwned>(
        &mut self,
        command: Command,
        json: bool,
        each: impl FnMut(E),
    ) -> Result<(), String>;
}

impl StreamEvents for Session {
    fn stream_events<E: DeserializeOwned>(
        &mut self,
        command: Command,
        json: bool,
        mut each: impl FnMut(E),
    ) -> Result<(), String> {
        if json {
            return self.stream(command, |event| println!("{event}"));
        }
        self.stream_typed(command, |event| match event {
            Ok(step) => each(step),
            Err(raw) => println!("{raw}"),
        })
    }
}
