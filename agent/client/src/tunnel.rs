//! A local port forwarded over SSH to one on the device's own loopback,
//! where the services a technician reaches from outside listen: VNC on
//! `127.0.0.1:5900` (docs/remote-access.md) and Chromium's DevTools on
//! `127.0.0.1:9222` (docs/kiosk-browser.md). The system's `ssh -N -L`, with
//! the key sent and the host key pinned by `ssh::authorize` first.
//! tessaro-gui runs one for its VNC panel and its DevTools job;
//! `tessaro-ctl browser devtools` runs one in the foreground.

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::ssh::Authorized;

/// Where Chromium's DevTools listens on the device.
pub const DEVTOOLS: &str = "127.0.0.1:9222";
/// The port `chrome://inspect` looks at without being configured.
pub const DEVTOOLS_LOCAL: u16 = 9222;
/// How long the forward may take to come up.
const UP: Duration = Duration::from_secs(15);

/// How ssh may talk to the person running it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prompts {
    /// Never ask (`BatchMode`), and keep what ssh says for the error: a
    /// GUI has no terminal for it to ask on.
    Never,
    /// The terminal's: ssh may ask for a key's passphrase, and says what
    /// went wrong itself.
    Terminal,
}

/// `ssh -N -L`, alive for as long as this is.
pub struct Tunnel {
    child: Child,
    /// The local end, on 127.0.0.1.
    pub port: u16,
}

/// The ssh command that forwards `local` on this machine's loopback to
/// `remote` on the device's. Options go before the destination: after it,
/// ssh reads a command.
pub fn argv(authorized: &Authorized, local: u16, remote: &str, prompts: Prompts) -> Vec<String> {
    let mut argv = authorized.argv(22, &[]);
    let destination = argv.pop().unwrap_or_default();
    let mut options = vec!["ExitOnForwardFailure=yes", "ServerAliveInterval=15"];
    if prompts == Prompts::Never {
        options.insert(0, "BatchMode=yes");
    }
    for option in options {
        argv.push("-o".into());
        argv.push(option.into());
    }
    argv.extend([
        "-N".into(),
        "-L".into(),
        format!("127.0.0.1:{local}:{remote}"),
    ]);
    argv.push(destination);
    argv
}

/// A local port nobody listens on: `preferred` when it is free, else any
/// (0 prefers none).
pub fn free_port(preferred: u16) -> Result<u16, String> {
    if preferred != 0 && TcpListener::bind(("127.0.0.1", preferred)).is_ok() {
        return Ok(preferred);
    }
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|err| format!("no local port: {err}"))
}

impl Tunnel {
    /// Forward `local` (0 for any free port) to `remote` on the device, and
    /// return once the local end answers.
    pub fn open(
        authorized: &Authorized,
        local: u16,
        remote: &str,
        prompts: Prompts,
    ) -> Result<Self, String> {
        let port = match local {
            0 => free_port(0)?,
            port => port,
        };
        let argv = argv(authorized, port, remote, prompts);
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).stdout(Stdio::null());
        if prompts == Prompts::Never {
            command.stdin(Stdio::null()).stderr(Stdio::piped());
        }
        let child = command
            .spawn()
            .map_err(|err| format!("{}: {err}", argv[0]))?;
        let mut tunnel = Self { child, port };

        let started = Instant::now();
        loop {
            if let Some(why) = tunnel.ended() {
                return Err(why);
            }
            if TcpStream::connect_timeout(
                &([127, 0, 0, 1], port).into(),
                Duration::from_millis(200),
            )
            .is_ok()
            {
                return Ok(tunnel);
            }
            if started.elapsed() > UP {
                return Err("the SSH tunnel did not come up".to_string());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    /// Why ssh is gone, once it is; `None` while it forwards.
    pub fn ended(&mut self) -> Option<String> {
        let status = self.child.try_wait().ok()??;
        let mut error = String::new();
        if let Some(mut stderr) = self.child.stderr.take() {
            let _ = stderr.read_to_string(&mut error);
        }
        let error = error.trim();
        Some(if error.is_empty() {
            format!("ssh ended ({status})")
        } else {
            format!("ssh: {error}")
        })
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn unclaimed() -> Authorized {
        Authorized {
            access: None,
            address: "192.0.2.7:7443".parse().unwrap(),
            alias: "tessaro-abc".to_string(),
            known_hosts: PathBuf::from("/cfg/known_hosts"),
            identity: None,
        }
    }

    #[test]
    fn the_forward_comes_before_the_destination() {
        let argv = argv(&unclaimed(), 9222, DEVTOOLS, Prompts::Never);
        let tail = argv[argv.len() - 4..].join(" ");
        assert_eq!(tail, "-N -L 127.0.0.1:9222:127.0.0.1:9222 root@192.0.2.7");
        assert!(argv.contains(&"BatchMode=yes".to_string()));
        assert!(argv.contains(&"ExitOnForwardFailure=yes".to_string()));
    }

    #[test]
    fn a_terminal_lets_ssh_ask() {
        let argv = argv(&unclaimed(), 9333, DEVTOOLS, Prompts::Terminal);
        assert!(!argv.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn a_taken_port_is_swapped_for_a_free_one() {
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let chosen = free_port(port).unwrap();
        assert_ne!(chosen, port);
        assert_ne!(chosen, 0);
    }
}
