//! `NAME.local` and `_tessaro._tcp`, so `tessaro-ctl` can find a device by
//! name instead of by whatever address DHCP handed it today.
//!
//! mdns-sd runs its own thread and its own sockets; every call here is a
//! channel send to it and returns at once, so nothing it does can stall the
//! runtime. `enable_addr_auto` lets it follow the interface list itself,
//! which is what survives NetworkManager bringing links up and down.
//!
//! TXT carries `id` (node id), `fp` (TLS fingerprint), `ver`, `machine`,
//! `claimed` and `tags` (device.tags, comma separated). A client uses `fp`
//! only as a hint: the pin is checked against the certificate on the
//! connection, never against what a broadcast says. `claimed=0` tells the
//! whole segment which devices can be taken - accepted and documented, with
//! `access.mdns=off` as the answer where that matters. The tags are as public
//! as the name.

use std::sync::Mutex;

use mdns_sd::{ServiceDaemon, ServiceInfo};
use protocol::NodeInfo;

use crate::log::Log;
use crate::sync::lock;

pub struct Mdns {
    daemon: ServiceDaemon,
    name: String,
    port: u16,
    txt: Vec<(String, String)>,
    /// What changes while the agent runs; every change announces all of it.
    live: Mutex<Live>,
}

#[derive(Clone)]
struct Live {
    claimed: bool,
    tags: Vec<String>,
}

impl Mdns {
    /// Announce `node` as it is now on `port`.
    pub fn start(log: &Log, port: u16, node: NodeInfo) -> Option<Self> {
        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(err) => {
                log.info(format!("mdns: cannot start: {err}"));
                return None;
            }
        };

        let name = node.name;
        let mdns = Self {
            daemon,
            name: name.clone(),
            port,
            txt: vec![
                ("id".to_string(), node.id),
                ("fp".to_string(), node.fingerprint),
                ("ver".to_string(), env!("CARGO_PKG_VERSION").to_string()),
                ("machine".to_string(), node.machine),
            ],
            live: Mutex::new(Live {
                claimed: node.claimed,
                tags: node.tags,
            }),
        };
        if let Err(err) = mdns.announce() {
            log.info(format!("mdns: cannot announce {name}.local: {err}"));
            return None;
        }
        log.info(format!("mdns: announcing {name}.local, port {port}"));
        Some(mdns)
    }

    /// Re-register with the new `claimed` value; mdns-sd replaces the record.
    pub fn set_claimed(&self, claimed: bool, log: &Log) {
        lock(&self.live).claimed = claimed;
        if let Err(err) = self.announce() {
            log.info(format!("mdns: cannot update the claim state: {err}"));
        }
    }

    /// Re-register with new tags, when device.tags changed.
    pub fn set_tags(&self, tags: Vec<String>, log: &Log) {
        {
            let mut live = lock(&self.live);
            if live.tags == tags {
                return;
            }
            live.tags = tags;
        }
        if let Err(err) = self.announce() {
            log.info(format!("mdns: cannot update the tags: {err}"));
        }
    }

    fn announce(&self) -> Result<(), String> {
        let live = lock(&self.live).clone();
        let txt = txt(&self.txt, &live);

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

/// The fixed entries, then the claim and the tags as they are now.
fn txt(fixed: &[(String, String)], live: &Live) -> Vec<(String, String)> {
    let mut txt = fixed.to_vec();
    txt.push((
        "claimed".to_string(),
        if live.claimed { "1" } else { "0" }.to_string(),
    ));
    txt.push(("tags".to_string(), live.tags.join(",")));
    txt
}

impl Drop for Mdns {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_claim_and_the_tags_follow_the_fixed_entries() {
        let fixed = vec![("id".to_string(), "abc".to_string())];
        let live = Live {
            claimed: false,
            tags: vec!["floor-2".to_string(), "lobby".to_string()],
        };
        assert_eq!(
            txt(&fixed, &live),
            vec![
                ("id".to_string(), "abc".to_string()),
                ("claimed".to_string(), "0".to_string()),
                ("tags".to_string(), "floor-2,lobby".to_string()),
            ]
        );
    }
}
