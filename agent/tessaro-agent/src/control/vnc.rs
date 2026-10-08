//! The VNC mirror on demand: `tessaro-ctl screen vnc`, the GUI's VNC tunnel
//! and its built-in viewer.
//!
//! A shared output costs Weston a readback of every damaged region and its
//! hardware planes, so nothing is mirrored until a tunnel asks. A start
//! shares the screen through screen-share's socket (the weston bbappend's
//! fifth patch), waits for the mirror's port and renews a lease; the client
//! holding the tunnel starts it again as its keepalive. `watch_vnc` stops the
//! mirror once no viewer is connected and no start came within `LEASE`, so a
//! client that vanished without a stop costs a minute at most. A Weston
//! restart loses the share, and the next keepalive shares again. See
//! docs/remote-access.md.

use std::sync::Arc;
use std::time::{Duration, Instant};

use protocol::{keys, Done, VncSession, VncStatus};

use super::{Caller, Control};
use crate::deadline::blocking;
use crate::loopback;
use crate::power;
use crate::state;
use crate::sync::lock;

/// Where the mirror listens, on the loopback: tessaro-weston-config's
/// VNC_PORT.
pub const PORT: u16 = 5900;

/// How long the mirror outlives the last start with no viewer connected.
pub const LEASE: Duration = Duration::from_secs(60);

/// How long a start waits for the mirror's port to open: a second Weston
/// starting, with its TLS key.
const OPENING: Duration = Duration::from_secs(10);

/// How often `watch_vnc` looks for viewers.
const EVERY: Duration = Duration::from_secs(5);

const SOCKET: &str = "the screen share socket";

/// Whether the mirror stays up: a viewer is connected, or a client started
/// or kept it within `LEASE`.
fn keep(viewers: usize, started: Option<Instant>, now: Instant) -> bool {
    viewers > 0 || started.is_some_and(|at| now.saturating_duration_since(at) < LEASE)
}

impl Control {
    /// Mirror the screen for a tunnel, or keep mirroring it: idempotent, and
    /// what a client holding the tunnel calls as its keepalive.
    pub(super) async fn vnc_start(&self, caller: &Caller) -> Result<VncSession, String> {
        let state = self.read_state().await?;
        let mode = state::setting(&state.settings, &self.defaults, keys::VNC)
            .unwrap_or_else(|| "on".to_string());
        if mode == "off" {
            return Err(format!(
                "VNC is off on this device; `tessaro-ctl config set {}=on` lets a tunnel mirror \
                 the screen",
                keys::VNC
            ));
        }

        // Renewed before sharing, so watch_vnc cannot stop it in between.
        let fresh = {
            let mut started = lock(&self.vnc_started);
            let now = Instant::now();
            let fresh = !keep(0, *started, now);
            *started = Some(now);
            fresh
        };
        // naked: power::exchange bounds itself with within()
        power::exchange(SOCKET, &self.paths.screen_share_socket, "share")
            .await
            .map_err(|err| format!("the compositor cannot start the VNC mirror: {err}"))?;
        self.vnc_opened().await?;
        if fresh {
            self.log
                .info(format!("VNC mirror started by {}", caller.describe()));
        }
        Ok(VncSession {
            mode,
            port: PORT,
            lease_s: LEASE.as_secs(),
        })
    }

    /// Wait until the mirror listens: at once when it already did.
    async fn vnc_opened(&self) -> Result<(), String> {
        let until = Instant::now() + OPENING;
        loop {
            if blocking("looking for the VNC mirror's port", || {
                Ok(loopback::listening(PORT))
            })
            .await?
            {
                return Ok(());
            }
            if Instant::now() >= until {
                return Err(format!(
                    "the VNC mirror did not open 127.0.0.1:{PORT} within {}s",
                    OPENING.as_secs()
                ));
            }
            // naked: a fixed pause between two reads, and OPENING bounds the loop
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// Stop mirroring: the tunnel is closing.
    pub(super) async fn vnc_stop(&self, caller: &Caller) -> Result<Done, String> {
        *lock(&self.vnc_started) = None;
        // naked: power::exchange bounds itself with within()
        power::exchange(SOCKET, &self.paths.screen_share_socket, "unshare")
            .await
            .map_err(|err| format!("the compositor cannot stop the VNC mirror: {err}"))?;
        self.log
            .info(format!("VNC mirror stopped by {}", caller.describe()));
        Ok(Done::new("the VNC mirror stopped"))
    }

    /// The mirror and its viewers, for `device status`; `None` when the
    /// compositor does not answer.
    pub(super) async fn vnc_status(&self) -> Option<VncStatus> {
        let socket = &self.paths.screen_share_socket;
        // naked: power::exchange bounds itself with within()
        let sharing = power::exchange(SOCKET, socket, "status").await.ok()?;
        let viewers = blocking("looking for VNC viewers", || Ok(loopback::foreign(PORT)))
            .await
            .unwrap_or(0);
        Some(VncStatus {
            sharing,
            viewers: u32::try_from(viewers).unwrap_or(u32::MAX),
        })
    }

    /// Every 5s: stop the mirror once no viewer is connected and no start
    /// came within `LEASE`. One `/proc` read, and a local socket round trip
    /// only when the lease has run out.
    pub fn watch_vnc(self: &Arc<Self>) {
        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(EVERY) => {}
                    _ = shutdown.changed() => return,
                }
                let viewers = blocking("looking for VNC viewers", || Ok(loopback::foreign(PORT)))
                    .await
                    .unwrap_or(0);
                let started = *lock(&control.vnc_started);
                if keep(viewers, started, Instant::now()) {
                    continue;
                }
                let socket = control.paths.screen_share_socket.clone();
                // naked: power::exchange bounds itself with within()
                if let Ok(true) = power::exchange(SOCKET, &socket, "status").await {
                    // naked: power::exchange bounds itself with within()
                    match power::exchange(SOCKET, &socket, "unshare").await {
                        Ok(_) => control.log.info("no VNC viewer; the mirror stops"),
                        Err(err) => control.log.debug(format!("vnc: {err}")),
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewer_keeps_the_mirror_whatever_the_lease() {
        let started = Instant::now();
        assert!(keep(1, None, started));
        assert!(keep(2, Some(started), started + LEASE * 3));
    }

    #[test]
    fn a_start_keeps_it_for_the_lease_and_no_longer() {
        let started = Instant::now();
        assert!(keep(0, Some(started), started));
        assert!(keep(
            0,
            Some(started),
            started + LEASE - Duration::from_secs(1)
        ));
        assert!(!keep(0, Some(started), started + LEASE));
        assert!(!keep(0, None, started));
    }

    #[test]
    fn a_start_after_now_does_not_panic() {
        let now = Instant::now();
        assert!(keep(0, Some(now + Duration::from_secs(5)), now));
    }
}
