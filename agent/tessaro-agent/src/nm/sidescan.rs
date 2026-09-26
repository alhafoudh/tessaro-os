//! Scanning while the radio is the hotspot.
//!
//! A WiFi interface in AP mode cannot scan on most drivers: mac80211 refuses
//! it unless the driver sets `NL80211_FEATURE_AP_SCAN` (`ieee80211_scan` in
//! `net/mac80211/cfg.c`), and iwlwifi does not, so wpa_supplicant logs
//! `CTRL-EVENT-SCAN-FAILED ret=-95` and NetworkManager's list holds only the
//! hotspot itself. What those radios do allow is a station interface beside
//! the AP (`iw phy <phy> info`, "valid interface combinations"), and that one
//! may scan. So a scan of a hotspot adds [`INTERFACE`] on the same phy, scans
//! from it with `iw`, and deletes it again; the hotspot stays up.
//!
//! NetworkManager leaves the interface alone (`unmanaged-devices` in
//! `10-tessaro.conf`), and `net.rs` hides it, so `auto` never picks it as the
//! WiFi device. Its name must not start with `wl`: the profiles' and the NAT
//! table's `wl*` would match it.

use std::process::Output;
use std::time::Duration;

use nmrs::models::SecurityFeatures;

use crate::deadline::blocking;
use crate::paths::Paths;
use crate::proc;

pub const INTERFACE: &str = "tessaro-scan";

const IW: &str = "/usr/sbin/iw";
const IP: &str = "/usr/sbin/ip";

/// Adding, raising and deleting an interface answer at once.
const STEP: Duration = Duration::from_secs(5);
/// One active scan of every channel the radio has, 2.4 and 5 GHz.
const SCAN: Duration = Duration::from_secs(20);

/// One network a side scan saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Empty for a hidden network.
    pub ssid: String,
    /// Upper case, as NetworkManager has it.
    pub bssid: String,
    /// Percent, as NetworkManager computes it from dBm.
    pub signal: u8,
    pub frequency_mhz: u32,
    pub security: SecurityFeatures,
}

/// Whether `interface` is an access point right now. False when it cannot
/// be told: the caller then does what it does for a client.
pub async fn is_ap(interface: &str) -> bool {
    match run(IW, &["dev", interface, "info"], STEP).await {
        Ok(text) => text.lines().any(|line| line.trim() == "type AP"),
        Err(_) => false,
    }
}

/// Scan from a station interface beside the access point `beside`.
pub async fn scan(paths: &Paths, beside: &str) -> Result<Vec<Found>, String> {
    let dir = paths.sys_net.join(beside);
    let (phy, mac) = blocking("reading the WiFi device", move || {
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .map(|text| text.trim().to_string())
                .map_err(|err| format!("{}: {err}", dir.join(name).display()))
        };
        Ok((read("phy80211/name")?, read("address")?))
    })
    .await?;
    let mac = local_mac(&mac).ok_or_else(|| format!("{beside} has no usable MAC address"))?;

    // A leftover from an agent that stopped halfway through a scan.
    let _ = run(IW, &["dev", INTERFACE, "del"], STEP).await;
    let add = [
        "phy",
        &phy,
        "interface",
        "add",
        INTERFACE,
        "type",
        "managed",
        "addr",
        &mac,
    ];
    run(IW, &add, STEP)
        .await
        .map_err(|err| format!("{beside} cannot scan while it is the hotspot: {err}"))?;
    let found = raise_and_scan().await;
    // Left behind if this fails, the next scan deletes it first.
    let _ = run(IW, &["dev", INTERFACE, "del"], STEP).await;
    found
}

async fn raise_and_scan() -> Result<Vec<Found>, String> {
    run(IP, &["link", "set", INTERFACE, "up"], STEP).await?;
    let text = run(IW, &["dev", INTERFACE, "scan"], SCAN).await?;
    Ok(parse(&text))
}

async fn run(program: &str, args: &[&str], limit: Duration) -> Result<String, String> {
    let output = proc::run_async(
        tokio::process::Command::new(program).args(args),
        None,
        "iw",
        limit,
    )
    .await?;
    succeeded(program, &output)
}

fn succeeded(program: &str, output: &Output) -> Result<String, String> {
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!("{program}: {}", proc::said(output)))
    }
}

/// The interface's own MAC with the locally administered bit set, so the
/// station interface does not share the hotspot's address. When that bit is
/// already set, the last octet differs instead.
fn local_mac(mac: &str) -> Option<String> {
    let mut octets = mac
        .split(':')
        .map(|part| u8::from_str_radix(part, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    if octets.len() != 6 {
        return None;
    }
    if octets[0] & 0x02 == 0 {
        octets[0] |= 0x02;
    } else {
        octets[5] ^= 0x01;
    }
    Some(
        octets
            .iter()
            .map(|octet| format!("{octet:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
    )
}

/// `iw dev <interface> scan` as text: one `BSS` line per network, its
/// details indented under it.
fn parse(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut current: Option<Found> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("BSS ") {
            found.extend(current.take());
            let bssid = rest.get(..17).unwrap_or_default();
            if bssid.split(':').count() == 6 {
                current = Some(Found {
                    ssid: String::new(),
                    bssid: bssid.to_uppercase(),
                    signal: 0,
                    frequency_mhz: 0,
                    security: SecurityFeatures::default(),
                });
            }
            continue;
        }
        let Some(bss) = current.as_mut() else {
            continue;
        };
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("freq:") {
            bss.frequency_mhz = value.trim().parse::<f64>().map_or(0, |mhz| mhz as u32);
        } else if let Some(value) = trimmed.strip_prefix("signal:") {
            if let Some(dbm) = value.split_whitespace().next().and_then(|v| v.parse().ok()) {
                bss.signal = percent(dbm);
            }
        } else if let Some(value) = trimmed.strip_prefix("SSID:") {
            // Only the network's own: later lines may name others.
            if bss.ssid.is_empty() {
                bss.ssid = unescape(value.trim());
            }
        } else if let Some(value) = trimmed.strip_prefix("capability:") {
            if value.split_whitespace().any(|word| word == "Privacy") {
                bss.security.privacy = true;
            }
        } else if let Some(value) = trimmed.strip_prefix("* Authentication suites:") {
            for suite in value.split_whitespace() {
                if suite.contains("802.1X") {
                    bss.security.eap = true;
                }
                if suite.contains("SUITE-B-192") {
                    bss.security.eap_suite_b_192 = true;
                }
                if suite.contains("PSK") {
                    bss.security.psk = true;
                }
                if suite == "SAE" || suite.ends_with("/SAE") {
                    bss.security.sae = true;
                }
                if suite == "OWE" {
                    bss.security.owe = true;
                }
            }
        }
    }
    found.extend(current);
    found
}

/// dBm to percent the way NetworkManager does it
/// (`nm_wifi_utils_level_to_quality`): -100 dBm and below is 0, -40 and
/// above is 100.
fn percent(dbm: f64) -> u8 {
    let level = dbm.clamp(-100.0, -40.0);
    (100.0 - (level + 40.0).abs() * 100.0 / 60.0) as u8
}

/// iw prints an SSID's unprintable bytes, and its outer spaces, as `\xNN`.
fn unescape(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let raw = text.as_bytes();
    let mut at = 0;
    while at < raw.len() {
        if raw[at] == b'\\' && raw.get(at + 1) == Some(&b'x') {
            if let Some(byte) = text
                .get(at + 2..at + 4)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                bytes.push(byte);
                at += 4;
                continue;
            }
        }
        bytes.push(raw[at]);
        at += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCAN_TEXT: &str = "\
BSS b8:27:eb:ff:7a:12(on tessaro-scan)
\tlast seen: 2804.056s [boottime]
\tTSF: 119952901774 usec (1d, 09:19:12)
\tfreq: 2437.0
\tbeacon interval: 100 TUs
\tcapability: ESS Privacy ShortSlotTime (0x0411)
\tsignal: -52.00 dBm
\tSSID: AlHafoudh
\tRSN:\t * Version: 1
\t\t * Group cipher: CCMP
\t\t * Pairwise ciphers: CCMP
\t\t * Authentication suites: PSK SAE
BSS 11:22:33:44:55:66(on tessaro-scan)
\tfreq: 5180
\tcapability: ESS (0x0001)
\tsignal: -95.00 dBm
\tSSID: caf\\xc3\\xa9\\x20
BSS aa:bb:cc:dd:ee:ff(on tessaro-scan)
\tfreq: 2412.0
\tcapability: ESS Privacy (0x0011)
\tsignal: -30.00 dBm
\tSSID: office
\tRSN:\t * Version: 1
\t\t * Authentication suites: IEEE 802.1X FT/IEEE 802.1X
";

    #[test]
    fn parses_what_iw_prints() {
        let found = parse(SCAN_TEXT);
        assert_eq!(found.len(), 3);

        assert_eq!(found[0].bssid, "B8:27:EB:FF:7A:12");
        assert_eq!(found[0].ssid, "AlHafoudh");
        assert_eq!(found[0].frequency_mhz, 2437);
        assert_eq!(found[0].signal, 80);
        assert!(found[0].security.privacy && found[0].security.psk && found[0].security.sae);
        assert!(!found[0].security.eap);

        assert_eq!(found[1].ssid, "café ");
        assert_eq!(found[1].frequency_mhz, 5180);
        assert_eq!(found[1].signal, 8);
        assert_eq!(found[1].security, SecurityFeatures::default());

        assert_eq!(found[2].signal, 100);
        assert!(found[2].security.eap && !found[2].security.psk);
    }

    #[test]
    fn nothing_before_the_first_bss_counts() {
        assert!(parse("command failed: Device or resource busy (-16)\n").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn the_station_gets_a_local_address_of_its_own() {
        assert_eq!(
            local_mac("9c:da:3e:9c:19:fe").as_deref(),
            Some("9e:da:3e:9c:19:fe")
        );
        assert_eq!(
            local_mac("02:00:00:00:00:10").as_deref(),
            Some("02:00:00:00:00:11")
        );
        assert_eq!(local_mac("not a mac"), None);
        assert_eq!(local_mac("00:11:22:33:44"), None);
    }

    #[test]
    fn the_name_is_never_one_of_the_wifi_wildcards() {
        assert!(!INTERFACE.starts_with("wl"));
        assert!(INTERFACE.len() <= 15, "IFNAMSIZ");
    }
}
