//! HDMI-CEC: talking to the TV over the HDMI cable (docs/cec.md).
//!
//! `uapi` is the kernel interface, `io` an adapter, `bus` what the agent
//! knows of the bus and decides from it, `log` the messages that went over
//! it. The worker that runs one adapter and hands its events to the page and
//! the scripts is the control plane's, in `control/cec.rs`.

pub mod bus;
pub mod io;
pub mod log;
pub mod uapi;

use tokio::sync::oneshot;

/// What the control plane asks an adapter's worker to do: `screen power`'s
/// wake and standby, and every `screen cec` action, checked already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Wake the TV, and with `source` switch it to the device's input.
    Wake { source: bool },
    /// The TV goes to standby, or with `all` everything on the bus.
    Standby { all: bool },
    /// Switch the TV to the device's input.
    Source,
    /// A remote key pressed and let go, sent to `to`.
    Key { code: u8, to: u8 },
    /// Poll the bus and ask whoever answers what they are.
    Scan,
    /// Any message to `to`, and with `reply` the answer it waits for.
    Send {
        to: u8,
        data: Vec<u8>,
        reply: Option<u8>,
    },
}

/// One act for one worker, and where its answer goes, if anyone waits.
#[derive(Debug)]
pub struct Job {
    pub act: Act,
    pub reply: Option<oneshot::Sender<protocol::CecAdapterActed>>,
}
