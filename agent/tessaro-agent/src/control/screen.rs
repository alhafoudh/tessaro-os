//! The screen's power: `tessaro-ctl screen power`, through Weston's
//! `tessaro-power.so` (`crate::power`), and the watcher that puts a screen
//! that was switched off back off after Weston restarts. Switching it also
//! puts the TV in standby or wakes it over HDMI-CEC (`control/cec.rs`); the
//! watcher's switching back does not.

use std::sync::Arc;
use std::time::Duration;

use protocol::ScreenPower;

use super::{Caller, Control};
use crate::deadline::blocking;
use crate::power;

impl Control {
    /// Switch the screen, or with `None` only say whether it is on. What was
    /// asked for is kept in `/run/tessaro-kiosk`, so it outlives a Weston
    /// restart but not a reboot.
    pub(super) async fn screen_power(
        &self,
        caller: &Caller,
        on: Option<bool>,
    ) -> Result<ScreenPower, String> {
        let socket = self.paths.power_socket.clone();
        let Some(on) = on else {
            return power::send(&socket, "status")
                .await // naked: power::send bounds itself with within()
                .map(|on| ScreenPower { on });
        };

        let now = power::send(&socket, if on { "on" } else { "off" })
            .await // naked: power::send bounds itself with within()
            .map_err(|err| format!("the compositor cannot switch the screen: {err}"))?;
        let marker = self.paths.screen_power_file();
        blocking("keeping the screen's power", move || {
            let result = if on {
                std::fs::remove_file(&marker).or_else(|err| match err.kind() {
                    std::io::ErrorKind::NotFound => Ok(()),
                    _ => Err(err),
                })
            } else {
                std::fs::write(&marker, b"off\n")
            };
            result.map_err(|err| format!("{}: {err}", marker.display()))
        })
        .await?;
        self.log.info(format!(
            "screen switched {} by {}",
            if now { "on" } else { "off" },
            caller.describe()
        ));
        // The TV follows over HDMI-CEC, when screen.cec.enable is on.
        self.cec_power(on);
        Ok(ScreenPower { on: now })
    }

    /// Whether the screen was switched off on purpose, by what is kept.
    pub(super) async fn screen_kept_off(&self) -> bool {
        let marker = self.paths.screen_power_file();
        blocking("reading the screen's power", move || Ok(marker.exists()))
            .await
            .unwrap_or(false)
    }

    /// Weston forgets the screen was off when it restarts - a hotplug, a
    /// `screen.*` setting - and comes back lit. Every 10s, while the screen
    /// is meant to be off, this asks the compositor and switches it off
    /// again. One local socket round trip, and only while off.
    pub fn watch_screen_power(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(10);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(EVERY) => {}
                    _ = shutdown.changed() => return,
                }
                // naked: one blocking() read
                if !control.screen_kept_off().await {
                    continue;
                }
                let socket = control.paths.power_socket.clone();
                // naked: power::send bounds itself with within()
                if let Ok(true) = power::send(&socket, "status").await {
                    // naked: power::send bounds itself with within()
                    match power::send(&socket, "off").await {
                        Ok(_) => control
                            .log
                            .info("screen switched off again after the compositor restarted"),
                        Err(err) => control.log.debug(format!("screen power: {err}")),
                    }
                }
            }
        });
    }
}
