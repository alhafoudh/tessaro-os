//! Chromium's DevTools protocol, as the `Cdp` port.
//!
//! This replaced a D-Bus `Ping` against cog. It is a strictly better health
//! signal: `Runtime.evaluate` proves the *renderer* is turning, not just that
//! the UI process is alive.
//!
//! The transport is one persistent session (`session.rs`), found through
//! `/json/list` (`targets.rs`), speaking `protocol.rs`. This file is only the
//! translation into what the state machine asks.

pub mod protocol;
pub mod session;
pub mod targets;

use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::error::{Error, Result};
use crate::log::Log;
use crate::ports::Cdp;
use crate::watchdog::Heartbeat;
use session::SessionHandle;

pub struct CdpClient<'a> {
    log: &'a Log,
    session: SessionHandle,
    heartbeat: Heartbeat,
    timeout: Duration,
}

impl<'a> CdpClient<'a> {
    /// `timeout` is seconds, and is the whole budget for one command.
    pub fn new(log: &'a Log, session: SessionHandle, heartbeat: Heartbeat, timeout: i64) -> Self {
        Self {
            log,
            session,
            heartbeat,
            timeout: Duration::from_secs(timeout.max(1) as u64),
        }
    }
}

#[async_trait(?Send)]
impl Cdp for CdpClient<'_> {
    async fn alive(&self) -> bool {
        let evaluate = json!({ "expression": "1 + 1", "returnByValue": true });
        let outcome = self
            .session
            .call(&self.heartbeat, "Runtime.evaluate", evaluate, self.timeout)
            .await; // naked: SessionHandle::call is under the heartbeat's within()

        match outcome {
            Ok(_) => true,
            Err(err) => {
                self.log.debug(format!("cdp alive? failed: {err}"));
                false
            }
        }
    }

    /// No round trip at all: the session keeps it current from the browser's
    /// own navigation events.
    async fn current_url(&self) -> Option<String> {
        self.session.current_url()
    }

    async fn navigate(&self, url: &str) -> Result<()> {
        let result = self
            .session
            .call(
                &self.heartbeat,
                "Page.navigate",
                json!({ "url": url }),
                self.timeout,
            )
            .await // naked: SessionHandle::call is under the heartbeat's within()
            .map_err(Error::Cdp)?;

        match result["errorText"].as_str() {
            Some(text) if !text.is_empty() => Err(Error::Cdp(format!("Page.navigate: {text}"))),
            _ => Ok(()),
        }
    }

    fn generation(&self) -> u64 {
        self.session.generation()
    }
}
