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

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::deadline::within;

pub const TABLE: &str = "tessaro-hotspot";

/// `nft` never takes long; past this something is wrong with it.
const NFT: Duration = Duration::from_secs(10);

const BINARY: &str = "/usr/sbin/nft";

/// The ruleset `nft -f` is fed. Declaring the table before deleting it makes
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
    let mut child = Command::new(BINARY)
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{BINARY}: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script(nat, interface).as_bytes())
            .map_err(|err| format!("{BINARY}: {err}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|err| format!("{BINARY}: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{BINARY}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// On the agent's runtime, under a deadline.
pub async fn apply(nat: bool, interface: &str) -> Result<(), String> {
    let mut child = tokio::process::Command::new(BINARY)
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("{BINARY}: {err}"))?;
    let body = script(nat, interface);
    let mut stdin = child.stdin.take();
    let run = async move {
        if let Some(stdin) = stdin.as_mut() {
            use tokio::io::AsyncWriteExt;
            // naked: bounded by the within() below
            stdin.write_all(body.as_bytes()).await?;
        }
        drop(stdin);
        // naked: bounded by the within() below
        child.wait_with_output().await
    };
    match within("nft", NFT, run).await {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(format!(
            "{BINARY}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Ok(Err(err)) => Err(format!("{BINARY}: {err}")),
        Err(expired) => Err(expired.to_string()),
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
}
