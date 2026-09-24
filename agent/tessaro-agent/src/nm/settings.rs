//! NetworkManager's settings dicts (`a{sa{sv}}`), read for `net show`.
//!
//! All pure: a profile's `GetSettings` answer goes in, what `net show`
//! prints comes out - for the managed profiles and hand-made ones alike.
//! The encodings are NetworkManager 1.46's:
//!
//! * addresses are `address-data` (`aa{sv}`: `address`, `prefix`);
//! * IPv4 `dns` is `au`, each address a `u32` whose bytes in memory are the
//!   address in network order - `from_ne_bytes` of its octets;
//! * IPv6 `dns` is `aay`, 16 bytes each; a `dns-data` (`as`), which later
//!   releases add, wins when present.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};

use protocol::{NetIpSettings, NetWifiSettings};
use zbus::zvariant::{OwnedValue, Value};

pub type Dict = HashMap<String, HashMap<String, OwnedValue>>;

pub const WIFI: &str = "802-11-wireless";
pub const WIFI_SECURITY: &str = "802-11-wireless-security";
pub const ETHERNET: &str = "802-3-ethernet";

/// The value inside a `v`, however many layers deep.
fn inner<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(boxed) => inner(boxed),
        other => other,
    }
}

fn get<'a>(dict: &'a Dict, section: &str, key: &str) -> Option<&'a Value<'static>> {
    dict.get(section)?.get(key).map(|owned| &**owned)
}

fn text(dict: &Dict, section: &str, key: &str) -> Option<String> {
    match get(dict, section, key).map(inner)? {
        Value::Str(text) => Some(text.to_string()),
        _ => None,
    }
}

fn flag(dict: &Dict, section: &str, key: &str) -> Option<bool> {
    match get(dict, section, key).map(inner)? {
        Value::Bool(flag) => Some(*flag),
        _ => None,
    }
}

fn bytes(value: &Value) -> Option<Vec<u8>> {
    match inner(value) {
        Value::Array(array) => array
            .inner()
            .iter()
            .map(|byte| match inner(byte) {
                Value::U8(byte) => Some(*byte),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn is_wifi(dict: &Dict) -> bool {
    text(dict, "connection", "type").as_deref() == Some(WIFI)
}

/// The SSID, decoded as UTF-8 where it is (it is only bytes to 802.11).
fn ssid(dict: &Dict) -> Option<String> {
    let raw = bytes(get(dict, WIFI, "ssid")?)?;
    Some(String::from_utf8_lossy(&raw).into_owned())
}

/// What `net show` prints for one address family.
pub fn ip(dict: &Dict, family: &str) -> NetIpSettings {
    let v6 = family == "ipv6";
    let mut settings = NetIpSettings {
        method: text(dict, family, "method").unwrap_or_else(|| "auto".to_string()),
        gateway: text(dict, family, "gateway").filter(|gateway| !gateway.is_empty()),
        ignore_auto_dns: flag(dict, family, "ignore-auto-dns").unwrap_or(false),
        ..NetIpSettings::default()
    };

    if let Some(Value::Array(entries)) = get(dict, family, "address-data").map(inner) {
        for entry in entries.inner() {
            let Value::Dict(fields) = inner(entry) else {
                continue;
            };
            let mut address = None;
            let mut prefix = None;
            for (key, value) in fields.iter() {
                match (inner(key), inner(value)) {
                    (Value::Str(key), Value::Str(text)) if key.as_str() == "address" => {
                        address = Some(text.to_string())
                    }
                    (Value::Str(key), Value::U32(n)) if key.as_str() == "prefix" => {
                        prefix = Some(*n)
                    }
                    _ => {}
                }
            }
            if let (Some(address), Some(prefix)) = (address, prefix) {
                settings.addresses.push(format!("{address}/{prefix}"));
            }
        }
    }

    match get(dict, family, "dns-data").map(inner) {
        Some(Value::Array(servers)) if !servers.is_empty() => {
            for server in servers.inner() {
                if let Value::Str(server) = inner(server) {
                    settings.dns.push(server.to_string());
                }
            }
        }
        _ => {
            if let Some(Value::Array(servers)) = get(dict, family, "dns").map(inner) {
                for server in servers.inner() {
                    match inner(server) {
                        Value::U32(n) if !v6 => settings
                            .dns
                            .push(Ipv4Addr::from(n.to_ne_bytes()).to_string()),
                        other if v6 => {
                            if let Some(Ok(octets)) = bytes(other).map(<[u8; 16]>::try_from) {
                                settings.dns.push(Ipv6Addr::from(octets).to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    settings
}

/// `open`, `wpa-psk`, `sae`, `wpa-eap`... as `net show` and a scan say it.
pub fn wifi(dict: &Dict) -> Option<NetWifiSettings> {
    if !is_wifi(dict) {
        return None;
    }
    let security = match text(dict, WIFI_SECURITY, "key-mgmt").as_deref() {
        None => "open".to_string(),
        Some("none") => "wep".to_string(),
        Some(other) => other.to_string(),
    };
    Some(NetWifiSettings {
        ssid: ssid(dict).unwrap_or_default(),
        security,
        hidden: flag(dict, WIFI, "hidden").unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).unwrap()
    }

    #[test]
    fn a_static_profile_reads_back() {
        let address: HashMap<String, Value> = HashMap::from([
            ("address".to_string(), Value::from("192.168.1.50")),
            ("prefix".to_string(), Value::from(24u32)),
        ]);
        let mut dict = Dict::new();
        dict.insert(
            "ipv4".into(),
            HashMap::from([
                ("method".into(), value(Value::from("manual"))),
                ("address-data".into(), value(Value::from(vec![address]))),
                ("gateway".into(), value(Value::from("192.168.1.1"))),
                (
                    "dns".into(),
                    value(Value::from(vec![u32::from_ne_bytes([1, 1, 1, 1])])),
                ),
            ]),
        );
        let read = ip(&dict, "ipv4");
        assert_eq!(read.method, "manual");
        assert_eq!(read.addresses, vec!["192.168.1.50/24"]);
        assert_eq!(read.gateway.as_deref(), Some("192.168.1.1"));
        assert_eq!(read.dns, vec!["1.1.1.1"]);
        assert_eq!(ip(&dict, "ipv6").method, "auto");
    }

    #[test]
    fn a_wifi_profile_says_its_network() {
        let mut dict = Dict::new();
        dict.insert(
            "connection".into(),
            HashMap::from([("type".into(), value(Value::from(WIFI)))]),
        );
        dict.insert(
            WIFI.into(),
            HashMap::from([("ssid".into(), value(Value::from(b"Office".to_vec())))]),
        );
        let settings = wifi(&dict).unwrap();
        assert_eq!(settings.ssid, "Office");
        assert_eq!(settings.security, "open");
        dict.insert(
            WIFI_SECURITY.into(),
            HashMap::from([("key-mgmt".into(), value(Value::from("sae")))]),
        );
        assert_eq!(wifi(&dict).unwrap().security, "sae");
        assert!(wifi(&Dict::new()).is_none());
    }
}
