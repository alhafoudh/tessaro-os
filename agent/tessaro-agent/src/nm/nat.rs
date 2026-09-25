//! Whether hotspot clients reach past the device: `network.wifi.nat`.
//!
//! The hotspot is `ipv4.method=shared`, and with `firewall-backend=nftables`
//! NetworkManager masquerades it and turns on `ip_forward` by itself (table
//! `ip nm-shared-<iface>`). There is no per-connection switch for that in
//! 1.46, so `network.wifi.nat=0` is a table of our own, `inet tessaro-hotspot`,
//! whose forward-hook chain drops anything coming in on the WiFi interface.
//! A drop in any base chain ends the packet, whatever NetworkManager's
//! chain on the same hook accepts, so clients still get DHCP, DNS and the
//! device itself - tessaro-ctl, SSH - and nothing beyond it.
//!
//! Applied by the boot oneshot (blocking, before NetworkManager is up) and
//! by the agent when the setting changes, through `nft -f -` either way.

use std::process::Command;
use std::time::Duration;

use crate::proc;

pub const TABLE: &str = "tessaro-hotspot";

/// `nft` never takes long; past this something is wrong with it.
const NFT: Duration = Duration::from_secs(10);

const BINARY: &str = "/usr/sbin/nft";

/// The ruleset `nft -f` is fed. `interface` is an `iifname` match, so the
/// `wl*` of `network.wifi.interface=auto` works as a wildcard. Declaring the table before deleting it makes
/// the delete succeed whether it existed or not, in one atomic transaction.
pub fn script(nat: bool, interface: &str) -> String {
    let mut text = format!("table inet {TABLE}\ndelete table inet {TABLE}\n");
    if !nat {
        text.push_str(&format!(
            "table inet {TABLE} {{\n  chain forward {{\n    type filter hook forward priority 0; policy accept;\n    iifname \"{interface}\" drop\n  }}\n}}\n"
        ));
    }
    text
}

/// Blocking, for the boot oneshot.
pub fn apply_blocking(nat: bool, interface: &str) -> Result<(), String> {
    let script = script(nat, interface);
    let output = proc::run(
        Command::new(BINARY).args(["-f", "-"]),
        Some(script.as_bytes()),
    )?;
    succeeded(&output)
}

/// On the agent's runtime, under a deadline.
pub async fn apply(nat: bool, interface: &str) -> Result<(), String> {
    let script = script(nat, interface);
    let output = proc::run_async(
        tokio::process::Command::new(BINARY).args(["-f", "-"]),
        Some(script.as_bytes()),
        "nft",
        NFT,
    )
    .await?;
    succeeded(&output)
}

fn succeeded(output: &std::process::Output) -> Result<(), String> {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{BINARY}: {}", proc::said(output)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nat_on_only_removes_the_table() {
        assert_eq!(
            script(true, "wlan0"),
            "table inet tessaro-hotspot\ndelete table inet tessaro-hotspot\n"
        );
    }

    #[test]
    fn nat_off_drops_forwarding_from_the_hotspot() {
        let text = script(false, "wlan0");
        assert!(text.starts_with("table inet tessaro-hotspot\ndelete table inet tessaro-hotspot\n"));
        assert!(text.contains("type filter hook forward priority 0; policy accept;"));
        assert!(text.contains("iifname \"wlan0\" drop"));
    }

    #[test]
    fn nat_off_on_auto_drops_every_wl_interface() {
        assert!(script(false, "wl*").contains("iifname \"wl*\" drop"));
    }
}
