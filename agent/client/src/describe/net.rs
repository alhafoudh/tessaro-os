//! The network: profiles, WiFi, the proxy, extra certificate authorities
//! and what a change did, for `tessaro-ctl network` and the Network page.

use protocol::{
    CertInfo, ChangeOutcome, NetAddress, NetChange, NetProfile, NetProfileDetail, ProxyStatus,
    ProxyTested, WifiNetwork, WifiStatus,
};

use crate::text::{row, unit_state, yes_no, Line, Tone};

/// The saved profiles, one a line.
pub fn profiles(profiles: &[NetProfile]) -> Vec<Line> {
    if profiles.is_empty() {
        return vec![Line::of(Tone::Muted, "no profiles")];
    }
    profiles
        .iter()
        .map(|profile| {
            let line = Line::new()
                .pad(Tone::Heading, &profile.name, 24)
                .text("  ")
                .pad(Tone::Muted, &profile.kind, 9)
                .text(" ");
            let line = match (&profile.device, profile.active) {
                (Some(device), true) => line.pad(Tone::Ok, device, 10),
                (Some(device), false) => line.pad(Tone::Muted, device, 10),
                (None, _) => line.pad(Tone::Muted, "-", 10),
            }
            .text(" ");
            let line = if profile.active {
                line.pad(Tone::Ok, "active", 7)
            } else {
                line.pad(Tone::Muted, "", 7)
            }
            .text(" ")
            .add(Tone::Label, "autoconnect")
            .text(" ")
            .join(yes_no(profile.autoconnect))
            .text("  ")
            .add(Tone::Muted, &profile.uuid);
            if profile.managed {
                line.text("  ").add(Tone::Muted, "(managed)")
            } else if profile.saved {
                line
            } else {
                line.text("  ").add(Tone::Muted, "(not saved)")
            }
        })
        .collect()
}

/// One profile, in full.
pub fn profile(detail: &NetProfileDetail) -> Vec<Line> {
    let profile = &detail.profile;
    let none = || Line::of(Tone::Muted, "(none)");
    let sub = |label: &str, value: Line| {
        Line::plain("    ")
            .pad(Tone::Label, label, 15)
            .text(" ")
            .join(value)
    };

    let mut lines = vec![
        Line::of(Tone::Heading, &profile.name)
            .text(" ")
            .add(Tone::Muted, format!("({})", profile.uuid)),
        row("kind", profile.kind.as_str()),
        row(
            "device",
            match (&profile.device, profile.active) {
                (Some(device), true) => Line::plain(format!("{device}  ")).add(Tone::Ok, "active"),
                (Some(device), false) => {
                    Line::plain(format!("{device}  ")).add(Tone::Muted, "(not active)")
                }
                (None, _) => Line::of(Tone::Muted, "(any, not active)"),
            },
        ),
        row(
            "autoconnect",
            yes_no(profile.autoconnect)
                .text("  ")
                .add(Tone::Muted, format!("(priority {})", profile.priority)),
        ),
        row(
            "saved",
            if profile.managed {
                Line::of(Tone::Ok, "managed")
                    .text("  ")
                    .add(Tone::Muted, "(rendered from settings every boot)")
            } else if profile.saved {
                Line::of(Tone::Ok, "yes")
            } else {
                Line::of(Tone::Warn, "no")
                    .text("  ")
                    .add(Tone::Muted, "(lost at reboot)")
            },
        ),
    ];
    if let Some(wifi) = &detail.wifi {
        lines.push(row("ssid", Line::of(Tone::Heading, &wifi.ssid)));
        lines.push(row("security", wifi.security.as_str()));
        lines.push(row("hidden", yes_no(wifi.hidden)));
    }
    for (family, settings) in [("ipv4", &detail.ipv4), ("ipv6", &detail.ipv6)] {
        lines.push(row(family, settings.method.as_str()));
        if !settings.addresses.is_empty() || settings.method == "manual" {
            lines.push(sub(
                "addresses",
                if settings.addresses.is_empty() {
                    none()
                } else {
                    Line::plain(settings.addresses.join(", "))
                },
            ));
        }
        if let Some(gateway) = &settings.gateway {
            lines.push(sub("gateway", Line::plain(gateway)));
        }
        if !settings.dns.is_empty() {
            lines.push(sub("dns", Line::plain(settings.dns.join(", "))));
        }
        if settings.ignore_auto_dns {
            lines.push(sub("ignore-auto-dns", yes_no(true)));
        }
    }
    if profile.active {
        let now = if detail.addresses.is_empty() {
            Line::of(Tone::Warn, "(no address)")
        } else {
            let mut now = Line::new();
            for (at, one) in detail.addresses.iter().enumerate() {
                if at > 0 {
                    now = now.text(", ");
                }
                now = now.join(address(one));
            }
            now
        };
        lines.push(row("now", now));
    }
    lines
}

/// `192.168.1.5/24 global`.
pub fn address(address: &NetAddress) -> Line {
    Line::plain(format!("{}/{} ", address.address, address.prefix)).add(Tone::Muted, &address.scope)
}

/// The radio, each WiFi device and what it is connected to, and the
/// fallback hotspot when it is up.
pub fn wifi(status: &WifiStatus) -> Vec<Line> {
    let radio = match (status.enabled, status.hardware_enabled) {
        (true, true) => Line::of(Tone::Ok, "on"),
        (false, true) => Line::of(Tone::Warn, "off"),
        (_, false) => Line::of(Tone::Bad, "off")
            .text("  ")
            .add(Tone::Muted, "(hardware switch)"),
    };
    let mut lines = vec![row("radio", radio)];
    if status.devices.is_empty() {
        lines.push(Line::of(Tone::Muted, "no WiFi device"));
    }
    for device in &status.devices {
        let network = match &device.ssid {
            Some(ssid) => {
                let mut network = Line::of(Tone::Heading, ssid);
                if let Some(signal) = device.signal {
                    network = network
                        .text("  ")
                        .add(signal_tone(signal), format!("{signal}%"));
                }
                if let Some(mhz) = device.frequency_mhz {
                    network = network.text("  ").add(Tone::Muted, band(mhz));
                }
                network
            }
            None => Line::of(Tone::Muted, "(not connected)"),
        };
        let state = if device.state == "activated" {
            Tone::Ok
        } else {
            Tone::Muted
        };
        lines.push(
            Line::new()
                .pad(Tone::Heading, &device.interface, 12)
                .text(" ")
                .pad(state, &device.state, 13)
                .text(" ")
                .join(network),
        );
    }
    if let Some(ssid) = &status.fallback {
        lines.push(row(
            "fallback",
            Line::of(Tone::Warn, "on")
                .text(" ")
                .add(Tone::Heading, ssid)
                .text(" did not connect after boot, so the hotspot is up until the next boot; ")
                .add(Tone::Cmd, "tessaro-ctl network wifi join")
                .text(" tries it again now"),
        ));
    }
    lines
}

/// The networks a scan found, one a line.
pub fn networks(networks: &[WifiNetwork]) -> Vec<Line> {
    if networks.is_empty() {
        return vec![Line::of(Tone::Muted, "no networks found")];
    }
    networks
        .iter()
        .map(|network| {
            let line = if network.ssid.is_empty() {
                Line::new().pad(Tone::Muted, "(hidden)", 24)
            } else {
                Line::new().pad(Tone::Heading, &network.ssid, 24)
            }
            .text("  ")
            .pad(
                signal_tone(network.signal),
                format!("{}%", network.signal),
                4,
            )
            .text(" ")
            .pad(Tone::Muted, band(network.frequency_mhz), 5)
            .text(" ")
            .pad(Tone::Label, &network.security, 12)
            .text(" ");
            if network.active {
                line.pad(Tone::Ok, "connected", 9)
            } else if network.known {
                line.pad(Tone::Muted, "known", 9)
            } else {
                line.pad(Tone::Muted, "", 9)
            }
            .text(" ")
            .add(Tone::Muted, &network.bssid)
        })
        .collect()
}

/// How good a signal is, in percent.
pub fn signal_tone(signal: u8) -> Tone {
    match signal {
        60.. => Tone::Ok,
        30..=59 => Tone::Warn,
        _ => Tone::Bad,
    }
}

/// The band of a frequency: `2.4G`, `5G`, `6G`.
pub fn band(mhz: u32) -> &'static str {
    match mhz {
        0..=3000 => "2.4G",
        3001..=5925 => "5G",
        _ => "6G",
    }
}

/// What a network change did: kept or rolled back, its checks, why.
pub fn change(change: &NetChange) -> Vec<Line> {
    let mut first = match change.outcome {
        ChangeOutcome::Committed => Line::of(Tone::Ok, "committed"),
        ChangeOutcome::RolledBack => Line::of(Tone::Bad, "rolled back"),
    }
    .text(" ")
    .add(Tone::Muted, &change.action);
    if let Some(name) = &change.profile {
        first = first.text(" ").add(Tone::Heading, name);
    }
    if let Some(uuid) = &change.uuid {
        first = first.text(" ").add(Tone::Muted, format!("({uuid})"));
    }
    let mut lines = vec![first];
    for check in &change.checks {
        let result = if check.passed {
            Line::of(Tone::Ok, "passed")
        } else {
            Line::of(Tone::Bad, "failed")
        };
        lines.push(
            Line::new()
                .pad(Tone::Label, &check.name, 12)
                .text(" ")
                .join(result)
                .text(format!(" {}", check.detail)),
        );
    }
    if let Some(reason) = &change.reason {
        lines.push(
            Line::new()
                .pad(Tone::Label, "why", 12)
                .text(format!(" {reason}")),
        );
    }
    if let Some(note) = &change.note {
        lines.push(Line::of(Tone::Warn, note));
    }
    lines
}

/// One certificate authority on a line: fingerprint, subject, expiry, and
/// who issued it when that is not itself.
pub fn cert(cert: &CertInfo) -> Line {
    let date = crate::certs::date(cert.not_after);
    let line = Line::of(Tone::Muted, &cert.fingerprint)
        .text("  ")
        .add(Tone::Heading, &cert.subject)
        .text("  ");
    let line = if crate::certs::expired(cert.not_after) {
        line.add(Tone::Bad, format!("expired {date}"))
    } else {
        line.add(Tone::Label, format!("until {date}"))
    };
    if cert.self_signed {
        line
    } else {
        line.text(" ")
            .add(Tone::Muted, format!("issued by {}", cert.issuer))
    }
}

/// The proxy the device uses, what bypasses it, the local one, and what to
/// run next.
pub fn proxy(status: &ProxyStatus) -> Vec<Line> {
    let mut lines = vec![row(
        "proxy",
        match &status.url {
            Some(url) => Line::of(Tone::Heading, url),
            None => Line::of(Tone::Muted, "(none: everything goes straight out)"),
        },
    )];
    if status.url.is_some() {
        lines.push(row(
            "bypass",
            if status.bypass.is_empty() {
                Line::of(Tone::Muted, "(loopback only)")
            } else {
                Line::plain(status.bypass.join(", "))
            },
        ));
    }
    lines.push(row(
        "local proxy",
        Line::plain(format!("{} ", status.listen)).add(unit_state(&status.unit), &status.unit),
    ));
    lines.push(Line::new());
    lines.push(Line::of(
        Tone::Cmd,
        if status.url.is_some() {
            "tessaro-ctl network proxy test"
        } else {
            "tessaro-ctl network proxy set http://HOST:PORT"
        },
    ));
    lines
}

/// What `network proxy test` found.
pub fn proxy_test(tested: &ProxyTested) -> Line {
    match (&tested.ip, &tested.error) {
        (Some(ip), _) => Line::of(Tone::Ok, "through the proxy the internet sees")
            .text(" ")
            .add(Tone::Heading, ip),
        (None, error) => Line::of(Tone::Bad, "the proxy did not get through:")
            .text(format!(" {}", error.as_deref().unwrap_or("no answer"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rolled_back_change_says_why() {
        let change = NetChange {
            outcome: ChangeOutcome::RolledBack,
            action: "set".into(),
            profile: Some("Wired connection 1".into()),
            uuid: Some("abc".into()),
            reason: Some("the device lost its default route".into()),
            checks: vec![protocol::NetCheck {
                name: "route".into(),
                passed: false,
                detail: "no default route any more".into(),
            }],
            note: None,
        };
        let lines: Vec<String> = super::change(&change)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(
            lines,
            vec![
                "rolled back set Wired connection 1 (abc)",
                "route        failed no default route any more",
                "why          the device lost its default route",
            ]
        );
    }

    #[test]
    fn bands() {
        assert_eq!(band(2437), "2.4G");
        assert_eq!(band(5180), "5G");
        assert_eq!(band(5955), "6G");
    }
}
