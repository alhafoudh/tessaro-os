//! The NetworkManager profiles the device manages, rendered as
//! keyfiles from its settings.
//!
//! * `tessaro-ethernet-dhcp` and `tessaro-ethernet-static`: the managed
//!   Ethernet port (`network.ethernet.interface`, or the first one up for `auto`).
//! * `tessaro-wifi-hotspot` (`tessaro-NAME`, open until claimed) and
//!   `tessaro-wifi-client` (`network.wifi.ssid`): the managed WiFi device
//!   (`network.wifi.interface`, or whichever there is for `auto`).
//!
//! Only the selected mode of each pair autoconnects, at priority 100, so it
//! wins over anything made by hand. The files go to
//! `/run/NetworkManager/system-connections`, which NetworkManager reads with
//! the highest precedence and which is gone at every boot - so they are
//! never saved anywhere, and the boot oneshot renders them afresh from
//! the saved settings and passwords before NetworkManager starts. A change
//! that did not commit can therefore never outlive a reboot.
//!
//! Everything here is pure: settings in, file names and text out.

use protocol::{keys, Secret};

/// Fixed, so the agent finds them by uuid on every device and every boot.
pub const ETHERNET_DHCP: Profile = Profile {
    id: "tessaro-ethernet-dhcp",
    uuid: "3c9a1e52-7b1d-4f6e-9a2e-5d0c4b8f1a01",
};
pub const ETHERNET_STATIC: Profile = Profile {
    id: "tessaro-ethernet-static",
    uuid: "3c9a1e52-7b1d-4f6e-9a2e-5d0c4b8f1a02",
};
pub const WIFI_HOTSPOT: Profile = Profile {
    id: "tessaro-wifi-hotspot",
    uuid: "3c9a1e52-7b1d-4f6e-9a2e-5d0c4b8f1a03",
};
pub const WIFI_CLIENT: Profile = Profile {
    id: "tessaro-wifi-client",
    uuid: "3c9a1e52-7b1d-4f6e-9a2e-5d0c4b8f1a04",
};
pub const ALL: [Profile; 4] = [ETHERNET_DHCP, ETHERNET_STATIC, WIFI_HOTSPOT, WIFI_CLIENT];

/// Whether a NetworkManager profile is one of ours.
pub fn is_managed(uuid: &str) -> bool {
    ALL.iter().any(|profile| profile.uuid == uuid)
}

/// Wins over a hand-made profile on the same device.
const PRIORITY: i32 = 100;

/// What the hotspot's name starts with.
pub const HOTSPOT_PREFIX: &str = "tessaro-";

/// The device's own address on the hotspot, where Quick Setup answers.
/// `tessaro-captive.conf` (tessaro-network) and `20-tessaro-portal.conf`
/// (tessaro-selftest) name it too.
pub const HOTSPOT_ADDRESS: &str = "10.42.0.1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub id: &'static str,
    pub uuid: &'static str,
}

impl Profile {
    pub fn file_name(&self) -> String {
        format!("{}.nmconnection", self.id)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Keyfile {
    pub name: String,
    /// Carries the WiFi passwords in the clear, as NetworkManager wants them.
    pub body: String,
}

/// The name and the size, never the body: it holds a password.
impl std::fmt::Debug for Keyfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyfile")
            .field("name", &self.name)
            .field("body", &format_args!("<{} bytes>", self.body.len()))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticIp {
    /// `ADDRESS/PREFIX`.
    pub address: String,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiMode {
    Hotspot,
    Client,
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ethernet {
    /// `None` for `auto`: the first Ethernet device NetworkManager brings up.
    pub interface: Option<String>,
    /// `None` is DHCP.
    pub fixed: Option<StaticIp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub ssid: String,
    /// `wpa-psk`, `sae`, or `open`.
    pub key_mgmt: String,
    pub hidden: bool,
    pub psk: Option<Secret>,
    pub fixed: Option<StaticIp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wifi {
    /// `None` for `auto`: whichever WiFi device the device has. The kernel
    /// names it `wlan0`, udev's predictable names `wlp1s0` or `wlx...`, so
    /// the profiles bind no name and live operations ask [`wifi_device`].
    pub interface: Option<String>,
    pub mode: WifiMode,
    pub hotspot_ssid: String,
    pub hotspot_psk: Option<Secret>,
    /// Hotspot clients may reach the internet and the LAN through us.
    pub nat: bool,
    /// Only once there is a network to join.
    pub client: Option<Client>,
}

/// Everything the managed profiles are rendered from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetConfig {
    pub ethernet: Ethernet,
    pub wifi: Wifi,
}

/// A key's effective value when neither the settings nor the image's env
/// file says: what an image without these keys in its defaults behaves as.
fn fallback(name: &str) -> &'static str {
    match name {
        "network.ethernet.interface" | "network.wifi.interface" => "auto",
        "network.ethernet.mode" | "network.wifi.ipv4" => "dhcp",
        "network.wifi.mode" => "hotspot",
        "network.wifi.nat" => "1",
        "network.wifi.security" => "psk",
        "network.wifi.hidden" => "0",
        keys::WIFI_FALLBACK_AFTER => "120",
        _ => "",
    }
}

/// The value of a network key for `value` (set or image default), or its
/// fallback when that is empty.
pub fn effective(value: &dyn Fn(&str) -> String, name: &str) -> String {
    let set = value(name);
    if set.is_empty() {
        fallback(name).to_string()
    } else {
        set
    }
}

/// The hotspot's SSID for a node name, within 802.11's 32 bytes.
pub fn hotspot_ssid(node_name: &str) -> String {
    let mut ssid = format!("{HOTSPOT_PREFIX}{node_name}");
    while ssid.len() > 32 {
        ssid.pop();
    }
    ssid
}

impl NetConfig {
    /// `value` returns a key's effective value (set, or the image default)
    /// and may return empty; the fallbacks above fill that in.
    pub fn from_settings(
        value: &dyn Fn(&str) -> String,
        hotspot_psk: Option<Secret>,
        wifi_psk: Option<Secret>,
        node_name: &str,
    ) -> Self {
        let get = |name: &str| effective(value, name);
        let fixed = |prefix: &str| StaticIp {
            address: get(&format!("{prefix}.address")),
            gateway: Some(get(&format!("{prefix}.gateway"))).filter(|g| !g.is_empty()),
            dns: keys::parse_addresses(&get(&format!("{prefix}.dns")))
                .unwrap_or_default()
                .iter()
                .map(|address| address.to_string())
                .collect(),
        };

        let interface = get("network.ethernet.interface");
        let ethernet = Ethernet {
            interface: (interface != "auto").then_some(interface),
            fixed: (get("network.ethernet.mode") == "static").then(|| fixed("network.ethernet")),
        };

        let ssid = get("network.wifi.ssid");
        let client = (!ssid.is_empty()).then(|| Client {
            key_mgmt: match get("network.wifi.security").as_str() {
                "sae" => "sae",
                "open" => "open",
                _ => "wpa-psk",
            }
            .to_string(),
            hidden: get("network.wifi.hidden") == "1",
            psk: wifi_psk.filter(|psk| !psk.expose().is_empty()),
            fixed: (get("network.wifi.ipv4") == "static").then(|| fixed("network.wifi")),
            ssid,
        });

        let interface = get("network.wifi.interface");
        let wifi = Wifi {
            interface: (interface != "auto").then_some(interface),
            mode: match get("network.wifi.mode").as_str() {
                "client" => WifiMode::Client,
                "off" => WifiMode::Off,
                _ => WifiMode::Hotspot,
            },
            hotspot_ssid: hotspot_ssid(node_name),
            hotspot_psk: hotspot_psk.filter(|psk| !psk.expose().is_empty()),
            nat: get("network.wifi.nat") != "0",
            client,
        };
        NetConfig { ethernet, wifi }
    }

    /// The Ethernet profile that should be up.
    pub fn ethernet_profile(&self) -> Profile {
        match self.ethernet.fixed {
            Some(_) => ETHERNET_STATIC,
            None => ETHERNET_DHCP,
        }
    }

    /// This config as it runs while the client has given way to the hotspot
    /// until the next boot: the hotspot autoconnects in its place. Anything
    /// but a client is left as it is.
    pub fn fallen_back(mut self) -> Self {
        if self.wifi.mode == WifiMode::Client {
            self.wifi.mode = WifiMode::Hotspot;
        }
        self
    }

    /// Whether the WiFi client is the same in both: mode, interface and
    /// network. The hotspot's name and password, the NAT and Ethernet may
    /// differ - a change to them keeps a fallback.
    pub fn same_client(&self, other: &NetConfig) -> bool {
        self.wifi.mode == other.wifi.mode
            && self.wifi.interface == other.wifi.interface
            && self.wifi.client == other.wifi.client
    }

    /// The WiFi profile that should be up, if any.
    pub fn wifi_profile(&self) -> Option<Profile> {
        match self.wifi.mode {
            WifiMode::Hotspot => Some(WIFI_HOTSPOT),
            WifiMode::Client if self.wifi.client.is_some() => Some(WIFI_CLIENT),
            _ => None,
        }
    }
}

impl Wifi {
    /// What the hotspot NAT's `iifname` matches: the named interface, or for
    /// `auto` every `wl` name - `wlan0` from the kernel and `wlp1s0`/`wlx...`
    /// from udev alike. The boot oneshot sets it before any WiFi driver may
    /// have loaded, so it cannot wait for the real name.
    pub fn nat_match(&self) -> &str {
        self.interface.as_deref().unwrap_or("wl*")
    }
}

/// The managed WiFi device among `present`, the kernel's interfaces as
/// (name, kind): the named one if it is there, else for `auto` the first
/// `wireless` one by name. `None` when there is none.
pub fn wifi_device(wanted: Option<&str>, present: &[(String, String)]) -> Option<String> {
    let mut wireless: Vec<&String> = present
        .iter()
        .filter(|(_, kind)| kind == "wireless")
        .map(|(name, _)| name)
        .collect();
    wireless.sort();
    match wanted {
        Some(wanted) => wireless.into_iter().find(|name| *name == wanted).cloned(),
        None => wireless.first().map(|name| name.to_string()),
    }
}

/// The client the WiFi fallback waits on after boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackWatch {
    /// As `Wifi::interface`: `None` for `auto`.
    pub interface: Option<String>,
    pub ssid: String,
    /// How long the client may go without connecting.
    pub after: std::time::Duration,
}

/// What the fallback watches for these settings: `None` unless WiFi is a
/// client with a network to join and `network.wifi.fallback_after` is not 0.
pub fn fallback_watch(value: &dyn Fn(&str) -> String) -> Option<FallbackWatch> {
    let config = NetConfig::from_settings(value, None, None, "");
    let seconds: u64 = effective(value, keys::WIFI_FALLBACK_AFTER)
        .parse()
        .unwrap_or(0);
    if config.wifi_profile() != Some(WIFI_CLIENT) || seconds == 0 {
        return None;
    }
    Some(FallbackWatch {
        ssid: config.wifi.client?.ssid,
        interface: config.wifi.interface,
        after: std::time::Duration::from_secs(seconds),
    })
}

/// A key's value as `settings` has it, else the image default for its env
/// name, else empty - what `NetConfig::from_settings` and
/// `keys::check_network` take.
pub fn value_of<'a>(
    settings: &'a std::collections::BTreeMap<String, String>,
    defaults: &'a std::collections::HashMap<String, String>,
) -> impl Fn(&str) -> String + 'a {
    move |name: &str| {
        settings
            .get(name)
            .cloned()
            .or_else(|| {
                let key = keys::find(name)?;
                defaults.get(key.env).cloned()
            })
            .unwrap_or_default()
    }
}

/// The node name these settings give: `device.name` as set or defaulted, else
/// the name derived from the node id.
pub fn node_name(value: &dyn Fn(&str) -> String, derived: &str) -> String {
    let name = value("device.name");
    if name.is_empty() {
        derived.to_string()
    } else {
        name
    }
}

/// Make `dir` hold exactly the managed profiles in `files`: each written if
/// it differs, 0600 root as NetworkManager requires, and any managed file
/// not in the list removed. Hand-made profiles in the same directory are
/// never touched. Blocking; whether anything changed.
pub fn write(dir: &std::path::Path, files: &[Keyfile]) -> std::io::Result<bool> {
    std::fs::create_dir_all(dir)?;
    let mut changed = false;
    for profile in ALL {
        let path = dir.join(profile.file_name());
        match files.iter().find(|file| file.name == profile.file_name()) {
            Some(file) => {
                changed |= crate::store::replace_if_changed(&path, file.body.as_bytes(), 0o600)?;
            }
            None => match std::fs::remove_file(&path) {
                Ok(()) => changed = true,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            },
        }
    }
    Ok(changed)
}

/// Every managed profile's keyfile. The client is left out until there is a
/// network to join.
pub fn render(config: &NetConfig) -> Vec<Keyfile> {
    let up = config.ethernet_profile();
    let mut files = vec![
        ethernet(
            ETHERNET_DHCP,
            up == ETHERNET_DHCP,
            config.ethernet.interface.as_deref(),
            None,
        ),
        ethernet(
            ETHERNET_STATIC,
            up == ETHERNET_STATIC,
            config.ethernet.interface.as_deref(),
            // A static profile with no address yet is still written, with
            // DHCP, so it exists to be switched to and shows in `profiles`.
            config.ethernet.fixed.as_ref(),
        ),
        hotspot(&config.wifi),
    ];
    if let Some(client) = &config.wifi.client {
        files.push(wifi_client(&config.wifi, client));
    }
    files
}

/// A GKeyFile value: backslash, a leading space and control characters are
/// escaped; everything else is taken literally.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (at, ch) in value.chars().enumerate() {
        match ch {
            '\\' => out.push_str("\\\\"),
            ' ' if at == 0 => out.push_str("\\s"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out
}

/// An SSID as text where that reads back unchanged, and as NetworkManager's
/// byte list otherwise (`;` would split it, and edges would be trimmed).
fn ssid_value(ssid: &str) -> String {
    let plain = ssid
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' '))
        && !ssid.starts_with(' ')
        && !ssid.ends_with(' ');
    if plain {
        ssid.to_string()
    } else {
        ssid.bytes().map(|byte| format!("{byte};")).collect()
    }
}

fn connection(profile: Profile, kind: &str, autoconnect: bool, interface: Option<&str>) -> String {
    let mut text = format!(
        "[connection]\nid={}\nuuid={}\ntype={kind}\nautoconnect={autoconnect}\nautoconnect-priority={PRIORITY}\n",
        profile.id, profile.uuid
    );
    if let Some(interface) = interface {
        text.push_str(&format!("interface-name={}\n", escape(interface)));
    }
    text
}

fn ipv4(fixed: Option<&StaticIp>) -> String {
    match fixed {
        None => "\n[ipv4]\nmethod=auto\n".to_string(),
        Some(ip) => {
            let mut text = format!("\n[ipv4]\nmethod=manual\naddress1={}\n", ip.address);
            if let Some(gateway) = &ip.gateway {
                text.push_str(&format!("gateway={gateway}\n"));
            }
            if !ip.dns.is_empty() {
                text.push_str(&format!(
                    "dns={};\nignore-auto-dns=true\n",
                    ip.dns.join(";")
                ));
            }
            text
        }
    }
}

fn ethernet(
    profile: Profile,
    autoconnect: bool,
    interface: Option<&str>,
    fixed: Option<&StaticIp>,
) -> Keyfile {
    let mut body = connection(profile, "ethernet", autoconnect, interface);
    body.push_str("\n[ethernet]\n");
    body.push_str(&ipv4(fixed.filter(|ip| !ip.address.is_empty())));
    body.push_str("\n[ipv6]\nmethod=auto\n");
    Keyfile {
        name: profile.file_name(),
        body,
    }
}

fn hotspot(wifi: &Wifi) -> Keyfile {
    let autoconnect = wifi.mode == WifiMode::Hotspot;
    let mut body = connection(WIFI_HOTSPOT, "wifi", autoconnect, wifi.interface.as_deref());
    body.push_str(&format!(
        "\n[wifi]\nmode=ap\nband=bg\nssid={}\n",
        ssid_value(&wifi.hotspot_ssid)
    ));
    // WPA2 with CCMP and no PMF: brcmfmac, the Pi's own WiFi, refuses a
    // client while PMF is on in AP mode, and its WPA3 AP support is broken.
    if let Some(psk) = &wifi.hotspot_psk {
        body.push_str(&format!(
            "\n[wifi-security]\nkey-mgmt=wpa-psk\nproto=rsn\npairwise=ccmp\ngroup=ccmp\npmf=1\npsk={}\n",
            escape(psk.expose())
        ));
    }
    // `shared`: NetworkManager's dnsmasq hands out 10.42.0.x and forwards
    // DNS; the NAT, when network.wifi.nat allows it, is its nftables table.
    // The address is NM's own default, stated so it cannot move: the setup
    // portal's captive DNS drop-in and its nginx server name it.
    body.push_str(&format!(
        "\n[ipv4]\nmethod=shared\naddress1={HOTSPOT_ADDRESS}/24\n\n[ipv6]\nmethod=disabled\n"
    ));
    Keyfile {
        name: WIFI_HOTSPOT.file_name(),
        body,
    }
}

fn wifi_client(wifi: &Wifi, client: &Client) -> Keyfile {
    let autoconnect = wifi.mode == WifiMode::Client;
    let mut body = connection(WIFI_CLIENT, "wifi", autoconnect, wifi.interface.as_deref());
    body.push_str(&format!(
        "\n[wifi]\nmode=infrastructure\nssid={}\n",
        ssid_value(&client.ssid)
    ));
    if client.hidden {
        body.push_str("hidden=true\n");
    }
    if client.key_mgmt != "open" {
        body.push_str(&format!(
            "\n[wifi-security]\nkey-mgmt={}\n",
            client.key_mgmt
        ));
        if let Some(psk) = &client.psk {
            body.push_str(&format!("psk={}\n", escape(psk.expose())));
        }
    }
    body.push_str(&ipv4(
        client.fixed.as_ref().filter(|ip| !ip.address.is_empty()),
    ));
    body.push_str("\n[ipv6]\nmethod=auto\n");
    Keyfile {
        name: WIFI_CLIENT.file_name(),
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn the_four_profiles_are_managed_and_nothing_else() {
        for profile in ALL {
            assert!(is_managed(profile.uuid), "{}", profile.id);
        }
        assert!(!is_managed("0b7f6d3e-1c2a-4e5f-8a9b-0c1d2e3f4a5b"));
        assert!(!is_managed(""));
    }

    fn config(pairs: &[(&str, &str)], hotspot: Option<&str>, wifi: Option<&str>) -> NetConfig {
        let pairs: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let value = move |name: &str| pairs.get(name).cloned().unwrap_or_default();
        NetConfig::from_settings(
            &value,
            hotspot.map(|psk| Secret(psk.to_string())),
            wifi.map(|psk| Secret(psk.to_string())),
            "brave-otter-3fa2",
        )
    }

    fn file(files: &[Keyfile], profile: Profile) -> &str {
        &files
            .iter()
            .find(|file| file.name == profile.file_name())
            .unwrap_or_else(|| panic!("no {}", profile.id))
            .body
    }

    #[test]
    fn the_defaults_are_dhcp_and_an_open_hotspot() {
        let config = config(&[], None, None);
        let files = render(&config);
        assert_eq!(files.len(), 3, "no client until there is a network to join");

        let dhcp = file(&files, ETHERNET_DHCP);
        assert!(dhcp.contains("autoconnect=true\n"));
        assert!(dhcp.contains("autoconnect-priority=100\n"));
        assert!(!dhcp.contains("interface-name"), "auto binds no interface");
        assert!(dhcp.contains("[ipv4]\nmethod=auto\n"));
        assert!(file(&files, ETHERNET_STATIC).contains("autoconnect=false\n"));

        let hotspot = file(&files, WIFI_HOTSPOT);
        assert!(hotspot.contains("autoconnect=true\n"));
        assert!(!hotspot.contains("interface-name"), "auto binds none");
        assert!(hotspot.contains("mode=ap\n"));
        assert!(hotspot.contains("ssid=tessaro-brave-otter-3fa2\n"));
        assert!(!hotspot.contains("[wifi-security]"), "open while unclaimed");
        assert!(hotspot.contains("[ipv4]\nmethod=shared\naddress1=10.42.0.1/24\n"));
        assert_eq!(config.wifi_profile(), Some(WIFI_HOTSPOT));
        assert_eq!(config.ethernet_profile(), ETHERNET_DHCP);
    }

    #[test]
    fn a_named_wifi_interface_is_bound_and_auto_matches_every_wl_name() {
        let auto = config(&[], None, None);
        assert_eq!(auto.wifi.interface, None);
        assert_eq!(auto.wifi.nat_match(), "wl*");

        let named = config(&[("network.wifi.interface", "wlp1s0")], None, None);
        let hotspot = file(&render(&named), WIFI_HOTSPOT).to_string();
        assert!(hotspot.contains("interface-name=wlp1s0\n"));
        assert_eq!(named.wifi.nat_match(), "wlp1s0");
    }

    #[test]
    fn the_wifi_device_is_the_named_one_or_the_first_wireless_one() {
        let present: Vec<(String, String)> = [
            ("enp0s31f6", "ethernet"),
            ("wlx00c0ca", "wireless"),
            ("wlp1s0", "wireless"),
        ]
        .iter()
        .map(|(name, kind)| (name.to_string(), kind.to_string()))
        .collect();
        assert_eq!(wifi_device(None, &present).as_deref(), Some("wlp1s0"));
        assert_eq!(
            wifi_device(Some("wlx00c0ca"), &present).as_deref(),
            Some("wlx00c0ca")
        );
        assert_eq!(wifi_device(Some("wlan0"), &present), None);
        assert_eq!(wifi_device(Some("enp0s31f6"), &present), None);
        assert_eq!(wifi_device(None, &present[..1]), None);
    }

    #[test]
    fn a_claimed_hotspot_is_wpa2_without_pmf() {
        let files = render(&config(&[], Some("abcdefgh23456789"), None));
        let hotspot = file(&files, WIFI_HOTSPOT);
        assert!(hotspot.contains("key-mgmt=wpa-psk\n"));
        assert!(hotspot.contains("pmf=1\n"));
        assert!(hotspot.contains("psk=abcdefgh23456789\n"));
    }

    #[test]
    fn debug_output_never_shows_a_password() {
        let config = config(
            &[("network.wifi.ssid", "Office")],
            Some("abcdefgh23456789"),
            Some("hunter2hunter2"),
        );
        let files = render(&config);
        for shown in [format!("{config:?}"), format!("{files:?}")] {
            assert!(
                !shown.contains("abcdefgh") && !shown.contains("hunter2"),
                "{shown}"
            );
        }
    }

    #[test]
    fn static_ethernet_on_a_named_port() {
        let config = config(
            &[
                ("network.ethernet.interface", "enp2s0"),
                ("network.ethernet.mode", "static"),
                ("network.ethernet.address", "192.168.1.50/24"),
                ("network.ethernet.gateway", "192.168.1.1"),
                ("network.ethernet.dns", "192.168.1.1,1.1.1.1"),
            ],
            None,
            None,
        );
        let files = render(&config);
        let fixed = file(&files, ETHERNET_STATIC);
        assert!(fixed.contains("autoconnect=true\n"));
        assert!(fixed.contains("interface-name=enp2s0\n"));
        assert!(fixed.contains("method=manual\naddress1=192.168.1.50/24\ngateway=192.168.1.1\n"));
        assert!(fixed.contains("dns=192.168.1.1;1.1.1.1;\nignore-auto-dns=true\n"));
        assert!(file(&files, ETHERNET_DHCP).contains("autoconnect=false\n"));
        assert_eq!(config.ethernet_profile(), ETHERNET_STATIC);
    }

    #[test]
    fn a_fallen_back_client_autoconnects_the_hotspot_and_keeps_its_profile() {
        let client = config(
            &[
                ("network.wifi.mode", "client"),
                ("network.wifi.ssid", "Office"),
            ],
            Some("abcdefgh23456789"),
            Some("password1"),
        );
        let fallen = client.clone().fallen_back();
        assert_eq!(fallen.wifi_profile(), Some(WIFI_HOTSPOT));
        let files = render(&fallen);
        assert!(file(&files, WIFI_HOTSPOT).contains("autoconnect=true\n"));
        assert!(file(&files, WIFI_CLIENT).contains("autoconnect=false\n"));
        assert!(file(&files, WIFI_HOTSPOT).contains("psk=abcdefgh23456789\n"));

        let off = config(&[("network.wifi.mode", "off")], None, None);
        assert_eq!(off.clone().fallen_back(), off, "only a client falls back");
    }

    #[test]
    fn only_a_change_to_the_client_ends_a_fallback() {
        let client = [
            ("network.wifi.mode", "client"),
            ("network.wifi.ssid", "Office"),
        ];
        let base = config(&client, None, Some("password1"));
        let with = |extra: (&str, &str)| {
            let mut pairs = client.to_vec();
            pairs.push(extra);
            config(&pairs, None, Some("password1"))
        };
        assert!(base.same_client(&with(("network.ethernet.mode", "static"))));
        assert!(base.same_client(&with(("network.wifi.nat", "0"))));
        assert!(!base.same_client(&with(("network.wifi.ssid", "Home"))));
        assert!(!base.same_client(&with(("network.wifi.interface", "wlan1"))));
        assert!(!base.same_client(&with(("network.wifi.mode", "hotspot"))));
        assert!(!base.same_client(&config(&client, None, Some("password2"))));
    }

    fn watch(pairs: &[(&str, &str)]) -> Option<FallbackWatch> {
        let pairs: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        fallback_watch(&move |name: &str| pairs.get(name).cloned().unwrap_or_default())
    }

    #[test]
    fn only_a_client_with_a_network_is_watched_for_the_fallback() {
        let client = [
            ("network.wifi.mode", "client"),
            ("network.wifi.ssid", "Office"),
        ];
        assert_eq!(
            watch(&client),
            Some(FallbackWatch {
                interface: None,
                ssid: "Office".to_string(),
                after: std::time::Duration::from_secs(120),
            }),
            "on by default, on the auto interface"
        );
        let mut later = client.to_vec();
        later.push((keys::WIFI_FALLBACK_AFTER, "600"));
        assert_eq!(watch(&later).unwrap().after.as_secs(), 600);
        let mut never = client.to_vec();
        never.push((keys::WIFI_FALLBACK_AFTER, "0"));
        assert_eq!(watch(&never), None);
        assert_eq!(
            watch(&[("network.wifi.mode", "client")]),
            None,
            "no network yet"
        );
        assert_eq!(watch(&[]), None, "the hotspot");
        assert_eq!(
            watch(&[
                ("network.wifi.mode", "off"),
                ("network.wifi.ssid", "Office")
            ]),
            None
        );
    }

    #[test]
    fn a_client_takes_over_from_the_hotspot() {
        let config = config(
            &[
                ("network.wifi.mode", "client"),
                ("network.wifi.ssid", "Office; 2"),
                ("network.wifi.security", "sae"),
                ("network.wifi.hidden", "1"),
            ],
            Some("abcdefgh23456789"),
            Some("pa\\ss word!"),
        );
        let files = render(&config);
        assert!(file(&files, WIFI_HOTSPOT).contains("autoconnect=false\n"));
        let client = file(&files, WIFI_CLIENT);
        assert!(client.contains("autoconnect=true\n"));
        assert!(client.contains("mode=infrastructure\n"));
        assert!(
            client.contains("ssid=79;102;102;105;99;101;59;32;50;\n"),
            "{client}"
        );
        assert!(client.contains("hidden=true\n"));
        assert!(client.contains("key-mgmt=sae\n"));
        assert!(client.contains("psk=pa\\\\ss word!\n"), "{client}");
        assert!(client.contains("[ipv4]\nmethod=auto\n"));
        assert_eq!(config.wifi_profile(), Some(WIFI_CLIENT));
    }

    #[test]
    fn an_open_client_and_a_static_client() {
        let files = render(&config(
            &[
                ("network.wifi.mode", "client"),
                ("network.wifi.ssid", "Cafe"),
                ("network.wifi.security", "open"),
                ("network.wifi.ipv4", "static"),
                ("network.wifi.address", "10.1.0.9/24"),
            ],
            None,
            None,
        ));
        let client = file(&files, WIFI_CLIENT);
        assert!(!client.contains("[wifi-security]"));
        assert!(client.contains("method=manual\naddress1=10.1.0.9/24\n"));
    }

    #[test]
    fn off_brings_up_neither() {
        let config = config(&[("network.wifi.mode", "off")], None, None);
        assert_eq!(config.wifi_profile(), None);
        assert!(file(&render(&config), WIFI_HOTSPOT).contains("autoconnect=false\n"));
    }

    #[test]
    fn a_long_name_is_cut_to_32_bytes() {
        let ssid = hotspot_ssid(&"x".repeat(40));
        assert_eq!(ssid.len(), 32);
        assert!(ssid.starts_with("tessaro-"));
    }

    #[test]
    fn escaping() {
        assert_eq!(escape(" lead"), "\\slead");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("plain; text"), "plain; text");
        assert_eq!(ssid_value("Office-2"), "Office-2");
        assert_eq!(ssid_value("a;b"), "97;59;98;");
    }
}
