//! The host ports the device is forwarded on. The same ones every time when
//! they are free, so the client store's entry for the device stays valid:
//! 7401 for the API and Webconfig, as `qemu:run` uses (7400 is the device on
//! the desk, through `dev:tunnel`), and 2222 for SSH.

use std::net::{Ipv4Addr, TcpListener};

pub const API: u16 = 7401;
pub const SSH: u16 = 2222;

/// How far past the preferred port to look.
const RANGE: u16 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ports {
    pub api: u16,
    pub ssh: u16,
}

impl Ports {
    pub fn pick() -> Result<Self, String> {
        let api = first_free(API, &[])?;
        let ssh = first_free(SSH, &[api])?;
        Ok(Self { api, ssh })
    }
}

fn first_free(from: u16, taken: &[u16]) -> Result<u16, String> {
    (from..from.saturating_add(RANGE))
        .find(|port| !taken.contains(port) && free(*port))
        .ok_or_else(|| format!("no free port on 127.0.0.1 from {from} on"))
}

fn free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_taken_port_is_skipped() {
        let held = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        let picked = first_free(port, &[]).unwrap();
        assert_ne!(picked, port);
        assert!(picked > port);
    }

    #[test]
    fn a_port_already_given_out_is_skipped() {
        let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        assert_ne!(first_free(port, &[port]).unwrap(), port);
    }
}
