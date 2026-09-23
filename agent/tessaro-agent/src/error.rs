//! The one error type the agent raises on purpose.
//!
//! Everything a dependency can legitimately fail at ends up here, so a caller
//! that wants "our failure, not a bug" has exactly one type to match on. The
//! Display text is what reaches the journal, so each variant carries its own
//! prefix rather than relying on the call site to add one.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Chromium's DevTools endpoint did not answer, or answered with an error.
    #[error("cdp: {0}")]
    Cdp(String),

    /// The system bus is missing, or systemd refused a call.
    #[error("systemd: {0}")]
    Systemd(String),
}

pub type Result<T> = std::result::Result<T, Error>;
