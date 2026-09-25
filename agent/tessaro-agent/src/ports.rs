//! The seams the state machine is written against.
//!
//! `Agent` never touches Chromium, the bus or the filesystem directly - it
//! only knows these traits. That is what lets the whole state machine be
//! tested against fakes with a fixed clock, and what makes a new health
//! signal a matter of one more trait plus one branch in `Agent::cycle`,
//! instead of a rewrite.
//!
//! They are async traits held as `&dyn`, which is why `async-trait` rather
//! than native `async fn` (not dyn-compatible). `?Send` because the runtime is
//! current-thread and the adapters and fakes legitimately hold `Cell`s.

use async_trait::async_trait;

use crate::error::Result;

/// Chromium, over the DevTools protocol.
#[async_trait(?Send)]
pub trait Cdp {
    /// Is the browser answering *and* is its renderer actually executing?
    /// Never fails: an unreachable browser is an answer, not an error.
    async fn alive(&self) -> bool;

    /// What the page target is currently showing, or `None` when the browser
    /// cannot say. Never fails, for the same reason `alive` does not: this is
    /// used to notice drift, and "cannot tell" must not be mistaken for "has
    /// drifted" and trigger a navigation.
    async fn current_url(&self) -> Option<String>;

    /// Point the page target at a URL.
    async fn navigate(&self, url: &str) -> Result<()>;

    /// Is a DevTools client other than the agent connected - a technician
    /// with the tab open in DevTools? Never fails: "cannot tell" is `false`,
    /// so the browser is never left unwatched on a guess.
    async fn inspected(&self) -> bool;

    /// Bumped when the page itself is new - a new browser or page target, or
    /// the same target after it crashed - so nothing about what is on screen
    /// can be assumed. A reconnect to the same live page does not bump it.
    /// Synchronous on purpose: it is a value the session already holds, never
    /// I/O.
    fn generation(&self) -> u64;
}

/// The systemd unit the browser runs as.
#[async_trait(?Send)]
pub trait Units {
    /// `active`, `activating`, `failed`, or `inactive` - the last one also
    /// when the unit or the bus is missing.
    async fn active_state(&self) -> String;

    /// The unit's MainPID, or 0 when there is no unit to have one.
    async fn main_pid(&self) -> u32;

    async fn restart(&self) -> Result<()>;
}

/// One reachability check of the kiosk site.
#[async_trait(?Send)]
pub trait Prober {
    async fn call(&self, url: &str) -> ProbeResult;
}

/// The local offline page.
#[async_trait(?Send)]
pub trait OfflinePage {
    /// Put the page where the browser can load it and return the URL to
    /// navigate to, or `None` when there is nothing readable to stage.
    async fn stage(&self) -> Option<String>;
}

/// The debug screen: `browser.debug.template`, filled in with the device as it is now.
#[async_trait(?Send)]
pub trait DebugScreen {
    /// Render the page where the browser can load it, or `None` when it
    /// could not be written.
    async fn stage(&self) -> Option<Staged>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub uri: String,
    /// The page differs from the one staged before, so what is on screen is
    /// out of date.
    pub changed: bool,
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
