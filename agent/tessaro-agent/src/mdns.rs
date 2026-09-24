//! `NAME.local` and `_tessaro._tcp`, so `tessaro-ctl` can find a device by
//! name instead of by whatever address DHCP handed it today.
//!
//! mdns-sd runs its own thread and its own sockets; every call here is a
//! channel send to it and returns at once, so nothing it does can stall the
//! runtime. `enable_addr_auto` lets it follow the interface list itself,
//! which is what survives NetworkManager bringing links up and down.
//!
//! TXT carries `id` (node id), `fp` (TLS fingerprint), `ver`, `machine` and
//! `claimed`. A client uses `fp` only as a hint: the pin is checked against
//! the certificate on the connection, never against what a broadcast says.
//! `claimed=0` tells the whole segment which devices can be taken - accepted
//! and documented, with `access.mdns=off` as the answer where that matters.

use mdns_sd::{ServiceDaemon, ServiceInfo};

use crate::log::Log;

pub struct Mdns {
    daemon: ServiceDaemon,
    name: String,
    port: u16,
    txt: Vec<(String, String)>,
}

impl Mdns {
    pub fn start(
        log: &Log,
        name: &str,
        port: u16,
        id: &str,
        fingerprint: &str,
        machine: &str,
        claimed: bool,
    ) -> Option<Self> {
        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(err) => {
                log.info(format!("mdns: cannot start: {err}"));
                return None;
            }
        };

        let mdns = Self {
            daemon,
            name: name.to_string(),
            port,
            txt: vec![
                ("id".to_string(), id.to_string()),
                ("fp".to_string(), fingerprint.to_string()),
                ("ver".to_string(), env!("CARGO_PKG_VERSION").to_string()),
                ("machine".to_string(), machine.to_string()),
            ],
        };
        if let Err(err) = mdns.announce(claimed) {
            log.info(format!("mdns: cannot announce {name}.local: {err}"));
            return None;
        }
        log.info(format!("mdns: announcing {name}.local, port {port}"));
        Some(mdns)
    }

    /// Re-register with the new `claimed` value; mdns-sd replaces the record.
    pub fn set_claimed(&self, claimed: bool, log: &Log) {
        if let Err(err) = self.announce(claimed) {
            log.info(format!("mdns: cannot update the claim state: {err}"));
        }
    }

    fn announce(&self, claimed: bool) -> Result<(), String> {
        let mut txt = self.txt.clone();
        txt.push((
            "claimed".to_string(),
            if claimed { "1" } else { "0" }.to_string(),
        ));

        let info = ServiceInfo::new(
            protocol::SERVICE_TYPE,
            &self.name,
            &format!("{}.local.", self.name),
            "",
            self.port,
            &txt[..],
        )
        .map_err(|err| err.to_string())?
        .enable_addr_auto();

        self.daemon.register(info).map_err(|err| err.to_string())
    }
}

impl Drop for Mdns {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}
