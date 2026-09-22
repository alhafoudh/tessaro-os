//! The seams the state machine is written against.
//!
//! `Agent` never touches Chromium, the bus or the filesystem directly - it
//! only knows these four traits. That is what lets the whole state machine be
//! tested against fakes with a fixed clock, and what makes a new health
//! signal a matter of one more trait plus one branch in `Agent::cycle`,
//! instead of a rewrite.

use crate::error::Result;

/// Chromium, over the DevTools protocol.
pub trait Cdp {
    /// Is the browser answering *and* is its renderer actually executing?
    /// Never fails: an unreachable browser is an answer, not an error.
    fn alive(&self) -> bool;

    /// What the page target is currently showing, or `None` when the browser
    /// cannot say. Never fails, for the same reason `alive` does not: this is
    /// used to notice drift, and "cannot tell" must not be mistaken for "has
    /// drifted" and trigger a navigation.
    fn current_url(&self) -> Option<String>;

    /// Point the page target at a URL.
    fn navigate(&self, url: &str) -> Result<()>;
}

/// The systemd unit the browser runs as.
pub trait Units {
    /// `active`, `activating`, `failed`, or `inactive` - the last one also
    /// when the unit or the bus is missing.
    fn active_state(&self) -> String;

    /// The unit's MainPID, or 0 when there is no unit to have one.
    fn main_pid(&self) -> u32;

    fn restart(&self) -> Result<()>;
}

/// One reachability check of the kiosk site.
pub trait Prober {
    fn call(&self, url: &str) -> ProbeResult;
}

/// The local offline page.
pub trait OfflinePage {
    /// Put the page where the browser can load it and return the URL to
    /// navigate to, or `None` when there is nothing readable to stage.
    fn stage(&self) -> Option<String>;
}

/// The outcome of a probe. Always a value, never an error - "the site is
/// down" is the normal case here, not an exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub ok: bool,
    pub status: Option<u16>,
    pub reason: String,
}

impl ProbeResult {
    pub fn ok(status: u16) -> Self {
        Self {
            ok: true,
            status: Some(status),
            reason: String::new(),
        }
    }

    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            ok: false,
            status: None,
            reason: reason.into(),
        }
    }
}
