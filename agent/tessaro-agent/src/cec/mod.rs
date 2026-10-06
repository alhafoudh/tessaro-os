//! HDMI-CEC: talking to the TV over the HDMI cable (docs/cec.md).
//!
//! `uapi` is the kernel interface, `io` an adapter, `bus` what the agent
//! knows of the bus and decides from it. The worker that runs one adapter
//! and hands its events to the page and the scripts is the control plane's,
//! in `control/cec.rs`.

pub mod bus;
pub mod io;
pub mod uapi;

/// What the control plane asks every adapter to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// The screen was switched on: wake the TV, and switch its input with
    /// `screen.cec.source` not `off`.
    Wake,
    /// The screen was switched off: the TV goes to standby.
    Standby,
}
