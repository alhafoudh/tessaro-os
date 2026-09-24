//! NetworkManager's settings dicts (`a{sa{sv}}`), read and patched.
//!
//! All pure: a profile's `GetSettings` answer goes in, what `net show` prints
//! or what `Update2` gets comes out. The encodings are NetworkManager 1.46's
//! and easy to get quietly wrong, so each has a test:
//!
//! * addresses go in `address-data` (`aa{sv}`: `address`, `prefix`), and the
//!   deprecated `addresses` is dropped, so the two can never disagree;
//! * IPv4 `dns` is `au`, each address a `u32` whose bytes in memory are the
//!   address in network order - `from_ne_bytes` of its octets;
//! * IPv6 `dns` is `aay`, 16 bytes each; a `dns-data` (`as`), which later
//!   releases add, is dropped when `dns` is written.
//!
//! The patch starts from `GetSettings`, which never includes secrets, and
//! only carries a `psk` when one is being changed. That matters: Update2
//! keeps the stored secrets only when the new settings have **none** at all
//! (`nm-settings-connection.c`), so one secret too many would erase the rest.

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr};

use protocol::netkeys::{self, NetKind};
use protocol::{NetIpSettings, NetWifiSettings};
use zbus::zvariant::{self, OwnedValue, Value};

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

pub fn text(dict: &Dict, section: &str, key: &str) -> Option<String> {
    match get(dict, section, key).map(inner)? {
        Value::Str(text) => Some(text.to_string()),
        _ => None,
    }
}

pub fn flag(dict: &Dict, section: &str, key: &str) -> Option<bool> {
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

pub fn uuid(dict: &Dict) -> Option<String> {
    text(dict, "connection", "uuid")
}

pub fn is_wifi(dict: &Dict) -> bool {
    text(dict, "connection", "type").as_deref() == Some(WIFI)
}

/// The SSID, decoded as UTF-8 where it is (it is only bytes to 802.11).
pub fn ssid(dict: &Dict) -> Option<String> {
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

pub fn autoconnect(dict: &Dict) -> bool {
    flag(dict, "connection", "autoconnect").unwrap_or(true)
}

fn owned(value: Value<'_>) -> Result<OwnedValue, String> {
    OwnedValue::try_from(value).map_err(|err| format!("encoding a setting: {err}"))
}

fn address_data<T: ToString>(addresses: &[(T, u8)]) -> Result<OwnedValue, String> {
    let entries: Vec<HashMap<String, Value>> = addresses
        .iter()
        .map(|(address, prefix)| {
            HashMap::from([
                ("address".to_string(), Value::from(address.to_string())),
                ("prefix".to_string(), Value::from(u32::from(*prefix))),
            ])
        })
        .collect();
    owned(Value::from(entries))
}

/// `current` with `values` (netkeys names) and a new WiFi password applied.
/// Every value is checked first; nothing is half-applied.
pub fn patch(
    current: &Dict,
    values: &BTreeMap<String, String>,
    psk: Option<&str>,
) -> Result<Dict, String> {
    let mut next = current.clone();
    let wifi = is_wifi(current);

    for (name, value) in values {
        let key = netkeys::find(name).ok_or_else(|| {
            format!("{name} cannot be changed here; `tessaro-ctl net keys` lists what can")
        })?;
        netkeys::validate(key, value)?;
        if key.wifi && !wifi {
            return Err(format!("{name} is only for a WiFi profile"));
        }
        let (section, property) = key
            .name
            .split_once('.')
            .expect("every net key is SECTION.PROPERTY");
        let entry = next.entry(section.to_string()).or_default();

        let encoded = match key.kind {
            NetKind::Text | NetKind::Method4 | NetKind::Method6 => {
                owned(Value::from(value.clone()))?
            }
            NetKind::Flag => owned(Value::from(netkeys::parse_flag(value)?))?,
            NetKind::Priority => owned(Value::from(
                value
                    .parse::<i32>()
                    .map_err(|err| format!("{name}: {err}"))?,
            ))?,
            NetKind::Addresses4 => {
                entry.remove("addresses");
                address_data(&netkeys::parse_addresses4(value)?)?
            }
            NetKind::Addresses6 => {
                entry.remove("addresses");
                address_data(&netkeys::parse_addresses6(value)?)?
            }
            NetKind::Gateway4 | NetKind::Gateway6 => {
                if value.trim().is_empty() {
                    entry.remove(property);
                    continue;
                }
                owned(Value::from(value.trim().to_string()))?
            }
            NetKind::Dns4 => {
                entry.remove("dns-data");
                let servers: Vec<u32> = netkeys::parse_list::<Ipv4Addr>(value)?
                    .into_iter()
                    .map(|server| u32::from_ne_bytes(server.octets()))
                    .collect();
                owned(Value::from(servers))?
            }
            NetKind::Dns6 => {
                entry.remove("dns-data");
                let servers: Vec<Vec<u8>> = netkeys::parse_list::<Ipv6Addr>(value)?
                    .into_iter()
                    .map(|server| server.octets().to_vec())
                    .collect();
                owned(Value::from(servers))?
            }
        };
        let property = match key.kind {
            NetKind::Addresses4 | NetKind::Addresses6 => "address-data",
            _ => property,
        };
        entry.insert(property.to_string(), encoded);
    }

    if let Some(psk) = psk {
        netkeys::check_psk(psk)?;
        let security = next
            .get_mut(WIFI_SECURITY)
            .filter(|_| wifi)
            .ok_or_else(|| {
                "this profile has no WiFi password to change; join the network again with \
             `tessaro-ctl net wifi join`"
                    .to_string()
            })?;
        security.insert("psk".to_string(), owned(Value::from(psk.to_string()))?);
    }

    consistent(&next)?;
    Ok(next)
}

/// What NetworkManager would refuse anyway, said in our words and before
/// anything is touched.
pub fn consistent(dict: &Dict) -> Result<(), String> {
    for family in ["ipv4", "ipv6"] {
        let settings = ip(dict, family);
        if settings.method == "manual" && settings.addresses.is_empty() {
            return Err(format!("{family}.method=manual needs {family}.addresses"));
        }
    }

    let v4 = ip(dict, "ipv4");
    if let (Some(gateway), false) = (&v4.gateway, v4.addresses.is_empty()) {
        let gateway: Ipv4Addr = gateway
            .parse()
            .map_err(|_| format!("ipv4.gateway: {gateway} is not an address"))?;
        let inside = v4.addresses.iter().any(|address| {
            netkeys::parse_addresses4(address)
                .ok()
                .and_then(|parsed| parsed.first().copied())
                .is_some_and(|(network, prefix)| netkeys::contains4(network, prefix, gateway))
        });
        if !inside {
            return Err(format!(
                "ipv4.gateway {gateway} is outside every one of ipv4.addresses ({})",
                v4.addresses.join(", ")
            ));
        }
    }
    Ok(())
}

/// The settings of a new WiFi profile, or the secret a saved one gets.
pub fn wifi_security(dict: &mut Dict, key_mgmt: &str, psk: Option<&str>) -> Result<(), String> {
    if key_mgmt == "open" {
        dict.remove(WIFI_SECURITY);
        if let Some(wifi) = dict.get_mut(WIFI) {
            wifi.remove("security");
        }
        return Ok(());
    }
    let psk = psk.ok_or_else(|| "this network needs a password".to_string())?;
    netkeys::check_psk(psk)?;
    let security = dict.entry(WIFI_SECURITY.to_string()).or_default();
    security.insert(
        "key-mgmt".to_string(),
        owned(Value::from(key_mgmt.to_string()))?,
    );
    security.insert("psk".to_string(), owned(Value::from(psk.to_string()))?);
    // Stored by NetworkManager itself: there is no secret agent here.
    security.insert("psk-flags".to_string(), owned(Value::from(0u32))?);
    Ok(())
}

/// `extra`'s sections laid over `dict`, key by key: secrets onto settings.
pub fn merge(dict: &mut Dict, extra: Dict) {
    for (section, values) in extra {
        dict.entry(section).or_default().extend(values);
    }
}

fn context() -> zvariant::serialized::Context {
    zvariant::serialized::Context::new_dbus(zvariant::LE, 0)
}

/// The dict in D-Bus's own encoding, so a snapshot on disk comes back with
/// every variant's type exactly as NetworkManager gave it.
pub fn encode(dict: &Dict) -> Result<Vec<u8>, String> {
    zvariant::to_bytes(context(), dict)
        .map(|data| data.bytes().to_vec())
        .map_err(|err| format!("encoding a profile: {err}"))
}

pub fn decode(bytes: &[u8]) -> Result<Dict, String> {
    let data = zvariant::serialized::Data::new(bytes, context());
    data.deserialize::<Dict>()
        .map(|(dict, _)| dict)
        .map_err(|err| format!("decoding a profile: {err}"))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn value(value: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(value).unwrap()
    }

    /// What NetworkManager's own `Wired connection 1` looks like over D-Bus.
    pub fn wired() -> Dict {
        let mut dict = Dict::new();
        dict.insert(
            "connection".into(),
            HashMap::from([
                ("id".into(), value(Value::from("Wired connection 1"))),
                (
                    "uuid".into(),
                    value(Value::from("8a3c0e5c-5f2d-4c6e-9c0f-8e9a1c2b3d4e")),
                ),
                ("type".into(), value(Value::from(ETHERNET))),
            ]),
        );
        dict.insert(
            "ipv4".into(),
            HashMap::from([
                ("method".into(), value(Value::from("auto"))),
                (
                    "addresses".into(),
                    value(Value::from(Vec::<Vec<u32>>::new())),
                ),
                (
                    "address-data".into(),
                    value(Value::from(Vec::<HashMap<String, Value>>::new())),
                ),
                ("dns".into(), value(Value::from(Vec::<u32>::new()))),
            ]),
        );
        dict.insert(
            "ipv6".into(),
            HashMap::from([("method".into(), value(Value::from("auto")))]),
        );
        dict
    }

    pub fn wifi_profile() -> Dict {
        let mut dict = wired();
        dict.get_mut("connection")
            .unwrap()
            .insert("type".into(), value(Value::from(WIFI)));
        dict.insert(
            WIFI.into(),
            HashMap::from([("ssid".into(), value(Value::from(b"Office".to_vec())))]),
        );
        dict.insert(
            WIFI_SECURITY.into(),
            HashMap::from([("key-mgmt".into(), value(Value::from("wpa-psk")))]),
        );
        dict
    }

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn static_addressing_is_encoded_as_1_46_expects() {
        let patched = patch(
            &wired(),
            &values(&[
                ("ipv4.method", "manual"),
                ("ipv4.addresses", "192.168.1.50/24"),
                ("ipv4.gateway", "192.168.1.1"),
                ("ipv4.dns", "1.1.1.1, 9.9.9.9"),
            ]),
            None,
        )
        .unwrap();

        let ipv4 = &patched["ipv4"];
        assert!(!ipv4.contains_key("addresses"), "the deprecated key goes");
        assert_eq!(ipv4["address-data"].value_signature().to_string(), "aa{sv}");
        assert_eq!(ipv4["dns"].value_signature().to_string(), "au");
        let Value::Array(servers) = &*ipv4["dns"] else {
            panic!("dns is not an array")
        };
        assert_eq!(
            servers.inner()[0],
            Value::U32(u32::from_ne_bytes([1, 1, 1, 1]))
        );

        let read = ip(&patched, "ipv4");
        assert_eq!(read.method, "manual");
        assert_eq!(read.addresses, vec!["192.168.1.50/24"]);
        assert_eq!(read.gateway.as_deref(), Some("192.168.1.1"));
        assert_eq!(read.dns, vec!["1.1.1.1", "9.9.9.9"]);
    }

    #[test]
    fn ipv6_dns_is_bytes() {
        let patched = patch(&wired(), &values(&[("ipv6.dns", "2001:db8::53")]), None).unwrap();
        assert_eq!(patched["ipv6"]["dns"].value_signature().to_string(), "aay");
        assert_eq!(ip(&patched, "ipv6").dns, vec!["2001:db8::53"]);
    }

    #[test]
    fn an_empty_gateway_is_removed() {
        let set = patch(&wired(), &values(&[("ipv4.gateway", "")]), None).unwrap();
        assert!(!set["ipv4"].contains_key("gateway"));
    }

    #[test]
    fn inconsistent_changes_are_refused_before_anything_moves() {
        let manual_alone = patch(&wired(), &values(&[("ipv4.method", "manual")]), None);
        assert!(manual_alone.unwrap_err().contains("needs ipv4.addresses"));

        let outside = patch(
            &wired(),
            &values(&[
                ("ipv4.method", "manual"),
                ("ipv4.addresses", "10.99.0.5/24"),
                ("ipv4.gateway", "192.168.1.1"),
            ]),
            None,
        );
        assert!(outside.unwrap_err().contains("outside"));

        assert!(patch(&wired(), &values(&[("ipv4.route-metric", "5")]), None).is_err());
        assert!(patch(
            &wired(),
            &values(&[("802-11-wireless.hidden", "yes")]),
            None
        )
        .is_err());
    }

    #[test]
    fn a_password_only_goes_where_there_is_one() {
        assert!(patch(&wired(), &BTreeMap::new(), Some("hunter2hunter2")).is_err());
        let wifi = patch(&wifi_profile(), &BTreeMap::new(), Some("hunter2hunter2")).unwrap();
        assert_eq!(
            text(&wifi, WIFI_SECURITY, "psk").as_deref(),
            Some("hunter2hunter2")
        );
        // No secret is added unless asked for: Update2 keeps the stored ones
        // only when the patch has none.
        let untouched = patch(&wifi_profile(), &values(&[("ipv4.dns", "1.1.1.1")]), None).unwrap();
        assert!(text(&untouched, WIFI_SECURITY, "psk").is_none());
    }

    #[test]
    fn wifi_reads() {
        let settings = wifi(&wifi_profile()).unwrap();
        assert_eq!(settings.ssid, "Office");
        assert_eq!(settings.security, "wpa-psk");
        assert!(wifi(&wired()).is_none());
        assert!(is_wifi(&wifi_profile()));
        assert!(!is_wifi(&wired()));
    }

    #[test]
    fn a_snapshot_round_trips_with_its_types() {
        let dict = patch(
            &wifi_profile(),
            &values(&[
                ("ipv4.method", "manual"),
                ("ipv4.addresses", "192.168.1.50/24"),
                ("connection.autoconnect-priority", "5"),
            ]),
            Some("hunter2hunter2"),
        )
        .unwrap();
        let back = decode(&encode(&dict).unwrap()).unwrap();
        assert_eq!(back, dict);
        assert_eq!(
            *back["connection"]["autoconnect-priority"],
            Value::I32(5),
            "an integer stays an i32"
        );
    }
}
