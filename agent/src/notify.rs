//! The sd_notify side of systemd's watchdog protocol, by hand.
//!
//! No libsystemd and no `libc` crate: the protocol is one `AF_UNIX` datagram
//! carrying one line, and std has covered the only fiddly part - Linux
//! abstract socket names - since 1.70. Pulling in a C library and a -sys crate
//! to send `WATCHDOG=1` would cost more than it is worth, the same argument
//! systemd.rs makes for talking to the bus directly.
//!
//! Everything here is optional. `NOTIFY_SOCKET` unset means systemd did not
//! start us - `cargo test`, `mise run agent-integration`, someone running the
//! binary by hand - and there is simply no notifier.
//!
//! `WatchdogSec=` alone is enough on a `Type=simple` unit: systemd 255
//! (`src/core/service.c`, `service_add_extras`) sets `NotifyAccess=main` when a
//! watchdog is configured, and exports `WATCHDOG_USEC` and `WATCHDOG_PID`
//! (`src/core/exec-invoke.c`). So nothing here sends `READY=1`; `Type=simple`
//! does not wait for it.

use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::time::Duration;

use crate::config::Env;
use crate::log::Log;

pub struct Notifier {
    socket: UnixDatagram,
    /// What `WATCHDOG_USEC` said, or `None` when the unit has no
    /// `WatchdogSec=` - a real configuration, e.g. a `WatchdogSec=0` drop-in.
    watchdog: Option<Duration>,
}

impl Notifier {
    pub fn from_env(env: &dyn Env, log: &Log) -> Option<Self> {
        let value = env.get("NOTIFY_SOCKET").filter(|value| !value.is_empty())?;

        let socket = match open(&value) {
            Ok(socket) => socket,
            Err(err) => {
                // Includes systemd's newer "vsock:cid:port" form, which only
                // appears inside a VM and which we have no reason to speak.
                log.info(format!("ignoring NOTIFY_SOCKET={value}: {err}"));
                return None;
            }
        };

        Some(Self {
            socket,
            watchdog: watchdog_interval(env, log),
        })
    }

    pub fn watchdog(&self) -> Option<Duration> {
        self.watchdog
    }

    /// Fire and forget. The socket is non-blocking, so a full buffer on
    /// systemd's side is `WouldBlock` rather than a stall - the next tick
    /// tries again - and any other failure ends with systemd killing us on the
    /// timeout, which is the intended outcome by the intended route.
    pub fn send(&self, message: &str, log: &Log) {
        if let Err(err) = self.socket.send(message.as_bytes()) {
            log.debug(format!("sd_notify {} failed: {err}", message.trim_end()));
        }
    }
}

/// systemd spells an abstract socket with a leading `@`, standing for the NUL
/// byte that actually starts the name. Anything else must be an absolute path.
fn open(value: &str) -> std::io::Result<UnixDatagram> {
    let address = match value.strip_prefix('@') {
        Some(name) => SocketAddr::from_abstract_name(name)?,
        None if value.starts_with('/') => SocketAddr::from_pathname(value)?,
        None => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "neither an absolute path nor an abstract name",
            ))
        }
    };

    let socket = UnixDatagram::unbound()?;
    // connect(), not send_to(): the address is checked once here rather than
    // on every ping. The socket stays unbound on purpose - systemd identifies
    // the sender from SO_PASSCRED, which is what NotifyAccess=main matches
    // against MainPID; libsystemd does the same for a plain notification.
    socket.connect_addr(&address)?;
    // A watchdog that can block on its own keepalive would be a joke.
    socket.set_nonblocking(true)?;
    Ok(socket)
}

fn watchdog_interval(env: &dyn Env, log: &Log) -> Option<Duration> {
    let usec: u64 = env.get("WATCHDOG_USEC")?.trim().parse().ok()?;
    if usec == 0 {
        return None;
    }

    // A process that merely inherited the environment must not answer
    // someone else's watchdog. Under Type=simple we are always the main
    // process, so this only ever matters if the binary is exec'd by something.
    if let Some(pid) = env.get("WATCHDOG_PID") {
        if pid.trim().parse::<u32>().ok() != Some(std::process::id()) {
            log.info(format!(
                "WATCHDOG_PID is {pid}, not this process; not answering the watchdog"
            ));
            return None;
        }
    }

    Some(Duration::from_micros(usec))
}

#[cfg(test)]
pub mod test_support {
    use super::*;

    /// A listener standing in for systemd's notify socket, bound to an
    /// abstract name unique to this process and call, so parallel test runs
    /// cannot collide and nothing is left on disk.
    pub struct FakeSystemd {
        pub listener: UnixDatagram,
        pub notify_socket: String,
    }

    impl FakeSystemd {
        pub fn abstract_socket() -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);

            let name = format!(
                "tessaro-agent-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let address = SocketAddr::from_abstract_name(&name).expect("abstract name");
            let listener = UnixDatagram::bind_addr(&address).expect("bind");
            listener.set_nonblocking(true).expect("nonblocking");

            Self {
                listener,
                notify_socket: format!("@{name}"),
            }
        }

        /// Everything received so far, oldest first.
        pub fn received(&self) -> Vec<String> {
            let mut messages = Vec::new();
            let mut buffer = [0u8; 256];
            while let Ok(length) = self.listener.recv(&mut buffer) {
                messages.push(String::from_utf8_lossy(&buffer[..length]).into_owned());
            }
            messages
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::FakeSystemd;
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn no_notify_socket_means_no_notifier() {
        let log = Log::buffered(true);

        assert!(Notifier::from_env(&env(&[]), &log).is_none());
        assert!(Notifier::from_env(&env(&[("NOTIFY_SOCKET", "")]), &log).is_none());
        // Nothing to say about it: this is `cargo test` and agent-integration.
        assert!(log.lines().is_empty());
    }

    #[test]
    fn an_abstract_socket_receives_the_exact_bytes() {
        // The one that proves the "@" -> NUL translation, which is the part
        // most likely to be silently wrong.
        let systemd = FakeSystemd::abstract_socket();
        let log = Log::buffered(true);
        let notifier = Notifier::from_env(&env(&[("NOTIFY_SOCKET", &systemd.notify_socket)]), &log)
            .expect("notifier");

        notifier.send("WATCHDOG=1", &log);

        assert_eq!(systemd.received(), vec!["WATCHDOG=1"]);
    }

    #[test]
    fn a_filesystem_socket_receives_the_exact_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notify");
        let listener = UnixDatagram::bind(&path).expect("bind");
        let log = Log::buffered(true);
        let notifier = Notifier::from_env(&env(&[("NOTIFY_SOCKET", path.to_str().unwrap())]), &log)
            .unwrap_or_else(|| panic!("no notifier for {} - sun_path limit?", path.display()));

        notifier.send("STOPPING=1", &log);

        let mut buffer = [0u8; 64];
        let length = listener.recv(&mut buffer).expect("recv");
        assert_eq!(&buffer[..length], b"STOPPING=1");
    }

    #[test]
    fn watchdog_usec_is_read_in_microseconds() {
        let systemd = FakeSystemd::abstract_socket();
        let log = Log::buffered(true);
        let pid = std::process::id().to_string();
        let notifier = Notifier::from_env(
            &env(&[
                ("NOTIFY_SOCKET", &systemd.notify_socket),
                ("WATCHDOG_USEC", "60000000"),
                ("WATCHDOG_PID", &pid),
            ]),
            &log,
        )
        .expect("notifier");

        assert_eq!(notifier.watchdog(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn no_watchdog_usec_means_no_watchdog_but_still_a_notifier() {
        let systemd = FakeSystemd::abstract_socket();
        let log = Log::buffered(true);
        let notifier = Notifier::from_env(&env(&[("NOTIFY_SOCKET", &systemd.notify_socket)]), &log)
            .expect("notifier");

        assert_eq!(notifier.watchdog(), None);
    }

    #[test]
    fn someone_elses_watchdog_is_not_answered() {
        let systemd = FakeSystemd::abstract_socket();
        let log = Log::buffered(true);
        let notifier = Notifier::from_env(
            &env(&[
                ("NOTIFY_SOCKET", &systemd.notify_socket),
                ("WATCHDOG_USEC", "60000000"),
                ("WATCHDOG_PID", "1"),
            ]),
            &log,
        )
        .expect("notifier");

        assert_eq!(notifier.watchdog(), None);
        assert!(log.lines()[0].contains("not this process"));
    }

    #[test]
    fn a_socket_we_do_not_speak_is_ignored_once() {
        for value in ["vsock:2:1234", "relative/path"] {
            let log = Log::buffered(true);

            assert!(Notifier::from_env(&env(&[("NOTIFY_SOCKET", value)]), &log).is_none());
            assert_eq!(log.lines().len(), 1, "{value}");
            assert!(log.lines()[0].contains("ignoring NOTIFY_SOCKET"));
        }
    }
}
