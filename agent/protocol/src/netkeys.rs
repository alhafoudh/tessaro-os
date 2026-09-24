//! The NetworkManager profile properties `tessaro-ctl net set` may change.
//!
//! A deliberately short list, in nmcli's own names so a technician who knows
//! `nmcli connection modify` reads them without a table: addressing, DNS and
//! autoconnect. Anything else stays with `nmtui` over SSH. Values are checked
//! here, on both ends, so the client refuses a typo before it is sent and the
//! agent refuses whatever an older client lets through.
//!
//! These are not settings in the `keys` sense: they live in NetworkManager's
//! own profiles, not in `state.json`, and a change is applied as one
//! transaction that the device rolls back on its own if it cuts the device
//! off (see `tessaro-agent/src/nm/txn.rs`).

use std::net::{Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetKind {
    /// Free text: the profile's name.
    Text,
    /// yes/no, true/false, on/off, 1/0.
    Flag,
    /// A signed priority; higher wins.
    Priority,
    /// `ipv4.method`.
    Method4,
    /// `ipv6.method`.
    Method6,
    /// Comma separated `ADDRESS/PREFIX`, IPv4.
    Addresses4,
    Addresses6,
    /// One address, or empty for none.
    Gateway4,
    Gateway6,
    /// Comma separated servers, or empty for none.
    Dns4,
    Dns6,
}

pub const METHODS4: &[&str] = &["auto", "manual", "link-local", "disabled"];
pub const METHODS6: &[&str] = &["auto", "dhcp", "manual", "link-local", "ignore", "disabled"];

impl NetKind {
    /// What a value may be, in words, for `tessaro-ctl net keys`.
    pub fn describe(self) -> String {
        match self {
            NetKind::Text => "any text".to_string(),
            NetKind::Flag => "yes or no".to_string(),
            NetKind::Priority => "an integer, -999 to 999".to_string(),
            NetKind::Method4 => METHODS4.join(", "),
            NetKind::Method6 => METHODS6.join(", "),
            NetKind::Addresses4 => "ADDRESS/PREFIX, comma separated; empty for none".to_string(),
            NetKind::Addresses6 => {
                "IPv6 ADDRESS/PREFIX, comma separated; empty for none".to_string()
            }
            NetKind::Gateway4 => "an IPv4 address; empty for none".to_string(),
            NetKind::Gateway6 => "an IPv6 address; empty for none".to_string(),
            NetKind::Dns4 => "IPv4 addresses, comma separated; empty for none".to_string(),
            NetKind::Dns6 => "IPv6 addresses, comma separated; empty for none".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetKey {
    pub name: &'static str,
    pub kind: NetKind,
    /// Only on a WiFi profile.
    pub wifi: bool,
    pub doc: &'static str,
}

const fn key(name: &'static str, kind: NetKind, doc: &'static str) -> NetKey {
    NetKey {
        name,
        kind,
        wifi: false,
        doc,
    }
}

pub static NET_KEYS: &[NetKey] = &[
    key(
        "connection.id",
        NetKind::Text,
        "The profile's name, as `net profiles` lists it.",
    ),
    key(
        "connection.autoconnect",
        NetKind::Flag,
        "Bring the profile up on its own when its device is available. Refused as `no` on \
         the profile carrying the default route, unless another device has one: the device \
         would come back from its next boot with no network.",
    ),
    key(
        "connection.autoconnect-priority",
        NetKind::Priority,
        "Which profile autoconnect tries first when several fit the same device.",
    ),
    key(
        "ipv4.method",
        NetKind::Method4,
        "auto is DHCP. manual needs ipv4.addresses; the gateway and DNS usually go with it.",
    ),
    key(
        "ipv4.addresses",
        NetKind::Addresses4,
        "Static addresses, for ipv4.method=manual: 192.168.1.50/24.",
    ),
    key(
        "ipv4.gateway",
        NetKind::Gateway4,
        "The default gateway with static addresses. Must be inside one of them.",
    ),
    key(
        "ipv4.dns",
        NetKind::Dns4,
        "DNS servers, used alongside what DHCP hands out unless ipv4.ignore-auto-dns=yes.",
    ),
    key(
        "ipv4.ignore-auto-dns",
        NetKind::Flag,
        "Use only ipv4.dns, not the servers DHCP hands out.",
    ),
    key(
        "ipv6.method",
        NetKind::Method6,
        "auto is SLAAC plus DHCPv6 as the router says; ignore leaves the kernel to it.",
    ),
    key(
        "ipv6.addresses",
        NetKind::Addresses6,
        "Static addresses, for ipv6.method=manual: 2001:db8::50/64.",
    ),
    key(
        "ipv6.gateway",
        NetKind::Gateway6,
        "The default gateway with static IPv6 addresses.",
    ),
    key("ipv6.dns", NetKind::Dns6, "IPv6 DNS servers."),
    key(
        "ipv6.ignore-auto-dns",
        NetKind::Flag,
        "Use only ipv6.dns, not the servers the router hands out.",
    ),
    NetKey {
        wifi: true,
        ..key(
            "802-11-wireless.hidden",
            NetKind::Flag,
            "The network does not broadcast its name, so the device has to probe for it.",
        )
    },
];

pub fn find(name: &str) -> Option<&'static NetKey> {
    NET_KEYS.iter().find(|key| key.name == name)
}

/// Every value, whatever its kind: it ends up in a D-Bus string, and a
/// control character in a profile name is nothing but trouble in `nmcli`.
fn plain(value: &str) -> Result<(), String> {
    if value.chars().any(char::is_control) {
        return Err("must not contain control characters".to_string());
    }
    Ok(())
}

pub fn validate(key: &NetKey, value: &str) -> Result<(), String> {
    let checked = plain(value).and_then(|()| match key.kind {
        NetKind::Text if value.trim().is_empty() => Err("must not be empty".to_string()),
        NetKind::Text if value.len() > 255 => Err("is too long".to_string()),
        NetKind::Text => Ok(()),
        NetKind::Flag => parse_flag(value).map(drop),
        NetKind::Priority => match value.parse::<i32>() {
            Ok(priority) if (-999..=999).contains(&priority) => Ok(()),
            _ => Err("must be an integer from -999 to 999".to_string()),
        },
        NetKind::Method4 => one_of(value, METHODS4),
        NetKind::Method6 => one_of(value, METHODS6),
        NetKind::Addresses4 => parse_addresses4(value).map(drop),
        NetKind::Addresses6 => parse_addresses6(value).map(drop),
        NetKind::Gateway4 => parse_optional::<Ipv4Addr>(value).map(drop),
        NetKind::Gateway6 => parse_optional::<Ipv6Addr>(value).map(drop),
        NetKind::Dns4 => parse_list::<Ipv4Addr>(value).map(drop),
        NetKind::Dns6 => parse_list::<Ipv6Addr>(value).map(drop),
    });
    checked.map_err(|why| format!("{}: {why}", key.name))
}

fn one_of(value: &str, allowed: &[&str]) -> Result<(), String> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!("must be one of {}", allowed.join(", ")))
    }
}

pub fn parse_flag(value: &str) -> Result<bool, String> {
    match value {
        "yes" | "true" | "on" | "1" => Ok(true),
        "no" | "false" | "off" | "0" => Ok(false),
        _ => Err("must be yes or no".to_string()),
    }
}

/// Comma separated, spaces allowed, empty for none.
fn items(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
}

pub fn parse_list<T: std::str::FromStr>(value: &str) -> Result<Vec<T>, String> {
    items(value)
        .map(|item| {
            item.parse::<T>()
                .map_err(|_| format!("{item} is not an address"))
        })
        .collect()
}

pub fn parse_optional<T: std::str::FromStr>(value: &str) -> Result<Option<T>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<T>()
        .map(Some)
        .map_err(|_| format!("{value} is not an address"))
}

fn parse_cidr<T: std::str::FromStr>(item: &str, max: u8) -> Result<(T, u8), String> {
    let (address, prefix) = item
        .split_once('/')
        .ok_or_else(|| format!("{item}: needs a prefix, as in {item}/{max}"))?;
    let address = address
        .parse::<T>()
        .map_err(|_| format!("{address} is not an address"))?;
    match prefix.parse::<u8>() {
        Ok(prefix) if (1..=max).contains(&prefix) => Ok((address, prefix)),
        _ => Err(format!("{item}: the prefix must be 1 to {max}")),
    }
}

pub fn parse_addresses4(value: &str) -> Result<Vec<(Ipv4Addr, u8)>, String> {
    items(value).map(|item| parse_cidr(item, 32)).collect()
}

pub fn parse_addresses6(value: &str) -> Result<Vec<(Ipv6Addr, u8)>, String> {
    items(value).map(|item| parse_cidr(item, 128)).collect()
}

/// Whether `ip` is inside `network/prefix`.
pub fn contains4(network: Ipv4Addr, prefix: u8, ip: Ipv4Addr) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    u32::from(network) & mask == u32::from(ip) & mask
}

/// A WPA passphrase: 8 to 63 printable ASCII characters, or the 64 hex
/// digits of a raw key. What wpa_supplicant itself accepts.
pub fn check_psk(psk: &str) -> Result<(), String> {
    if psk.len() == 64 && psk.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Ok(());
    }
    if !(8..=63).contains(&psk.len()) {
        return Err("a WiFi password is 8 to 63 characters, or 64 hex digits".to_string());
    }
    if !psk
        .chars()
        .all(|ch| ch.is_ascii() && !ch.is_ascii_control())
    {
        return Err("a WiFi password is printable ASCII only".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, value: &str) -> Result<(), String> {
        validate(find(name).expect(name), value)
    }

    #[test]
    fn every_key_is_found_by_its_name() {
        for key in NET_KEYS {
            assert_eq!(find(key.name), Some(key));
        }
        assert!(find("ipv4.route-metric").is_none());
    }

    #[test]
    fn values_are_checked_by_kind() {
        assert!(check("ipv4.method", "manual").is_ok());
        assert!(check("ipv4.method", "static").is_err());
        assert!(check("ipv6.method", "ignore").is_ok());
        assert!(check("ipv4.addresses", "192.168.1.50/24, 10.0.0.2/8").is_ok());
        assert!(check("ipv4.addresses", "").is_ok());
        assert!(check("ipv4.addresses", "192.168.1.50").is_err());
        assert!(check("ipv4.addresses", "192.168.1.50/33").is_err());
        assert!(check("ipv6.addresses", "2001:db8::50/64").is_ok());
        assert!(check("ipv4.gateway", "192.168.1.1").is_ok());
        assert!(check("ipv4.gateway", "").is_ok());
        assert!(check("ipv4.gateway", "2001:db8::1").is_err());
        assert!(check("ipv4.dns", "1.1.1.1,9.9.9.9").is_ok());
        assert!(check("ipv4.dns", "one.one").is_err());
        assert!(check("connection.autoconnect", "no").is_ok());
        assert!(check("connection.autoconnect", "maybe").is_err());
        assert!(check("connection.autoconnect-priority", "-5").is_ok());
        assert!(check("connection.autoconnect-priority", "1000").is_err());
        assert!(check("connection.id", "Office").is_ok());
        assert!(check("connection.id", " ").is_err());
        assert!(check("connection.id", "a\nb").is_err());
    }

    #[test]
    fn the_error_names_the_key() {
        let error = check("ipv4.gateway", "nope").unwrap_err();
        assert!(error.starts_with("ipv4.gateway: "), "{error}");
    }

    #[test]
    fn subnets() {
        let net = Ipv4Addr::new(192, 168, 1, 50);
        assert!(contains4(net, 24, Ipv4Addr::new(192, 168, 1, 1)));
        assert!(!contains4(net, 24, Ipv4Addr::new(192, 168, 2, 1)));
        assert!(contains4(net, 16, Ipv4Addr::new(192, 168, 2, 1)));
    }

    #[test]
    fn passphrases() {
        assert!(check_psk("correct horse").is_ok());
        assert!(check_psk("short").is_err());
        assert!(check_psk(&"a".repeat(64)).is_ok());
        assert!(check_psk(&"g".repeat(64)).is_err());
        assert!(check_psk("pässwort123").is_err());
        assert!(check_psk("tab\there!").is_err());
    }
}
