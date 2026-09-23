//! The network as the device sees it, read-only.
//!
//! Straight from the kernel: interfaces from `/sys/class/net`, addresses from
//! `getifaddrs` (the `if-addrs` crate, already here through mdns-sd), the
//! IPv4 default route from `/proc/net/route`, and DNS from systemd-resolved's
//! own resolv.conf - `/etc/resolv.conf` is its 127.0.0.53 stub. Not through
//! NetworkManager: this has to answer on a device whose NetworkManager is the
//! broken thing, and nothing here changes anything.
//!
//! Everything is blocking file I/O and one syscall; call it through
//! `control::blocking`. The one exception is the public address, which only
//! the outside world knows: `public_ip` asks Cloudflare over the network, and
//! the agent keeps the answer in `/run/tessaro-kiosk/public-ip`, which is all
//! `snapshot` reads - so the boot render and every `get` stay local.

use std::collections::BTreeMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use protocol::{Net, NetAddress, NetInterface};

use crate::http::HyperHttp;
use crate::paths::Paths;

/// Cloudflare's trace endpoint, by address: no DNS in the way, and the
/// certificate carries 1.1.1.1 as an IP SAN, so TLS verifies as usual.
pub const TRACE_URL: &str = "https://1.1.1.1/cdn-cgi/trace";

/// The address the internet sees this device at, from the `ip=` line of
/// Cloudflare's trace. Every phase of the request has its own deadline.
pub async fn public_ip(http: &HyperHttp) -> Result<IpAddr, String> {
    // naked: every phase inside fetch() has its own within()
    let response = http.fetch(TRACE_URL).await.map_err(|err| err.to_string())?;
    if response.status != 200 {
        return Err(format!("answered HTTP {}", response.status));
    }
    parse_trace(&response.body).ok_or_else(|| "no ip= line in the answer".to_string())
}

/// The `ip=` line of a `/cdn-cgi/trace` body.
pub fn parse_trace(body: &str) -> Option<IpAddr> {
    body.lines()
        .find_map(|line| line.strip_prefix("ip="))
        .and_then(|ip| ip.trim().parse().ok())
}

/// The last public address the agent found, if it found one this boot.
pub fn cached_public_ip(paths: &Paths) -> Option<String> {
    read(&paths.public_ip_file())
}

pub fn snapshot(paths: &Paths) -> Net {
    let addresses = if_addrs::get_if_addrs().unwrap_or_default();
    let (interface, gateway) = fs::read_to_string(&paths.proc_route)
        .ok()
        .and_then(|table| default_route(&table))
        .map(|(name, gateway)| (Some(name), Some(gateway.to_string())))
        .unwrap_or((None, None));

    let mut by_name: BTreeMap<String, Vec<NetAddress>> = BTreeMap::new();
    for entry in &addresses {
        let (family, prefix) = match &entry.addr {
            if_addrs::IfAddr::V4(v4) => ("ipv4", v4.prefixlen),
            if_addrs::IfAddr::V6(v6) => ("ipv6", v6.prefixlen),
        };
        let scope = if entry.addr.is_loopback() {
            "loopback"
        } else if entry.addr.is_link_local() {
            "link-local"
        } else {
            "global"
        };
        by_name
            .entry(entry.name.clone())
            .or_default()
            .push(NetAddress {
                address: entry.addr.ip().to_string(),
                prefix,
                family: family.to_string(),
                scope: scope.to_string(),
            });
    }

    // Every interface the kernel has, addressed or not - a wifi card that
    // is down is exactly the thing worth seeing.
    let mut names: Vec<String> = fs::read_dir(&paths.sys_net)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    for name in by_name.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    names.sort();

    let interfaces = names
        .into_iter()
        .map(|name| {
            let dir = paths.sys_net.join(&name);
            NetInterface {
                kind: kind(&dir, &name),
                mac: read(&dir.join("address")).filter(|mac| mac != "00:00:00:00:00:00"),
                state: read(&dir.join("operstate")).unwrap_or_else(|| "unknown".to_string()),
                carrier: read(&dir.join("carrier")).map(|value| value == "1"),
                mtu: read(&dir.join("mtu")).and_then(|value| value.parse().ok()),
                // Reads -1 or fails with EINVAL on a link that is down.
                speed_mbps: read(&dir.join("speed")).and_then(|value| value.parse().ok()),
                default_route: interface.as_deref() == Some(name.as_str()),
                addresses: by_name.remove(&name).unwrap_or_default(),
                name,
            }
        })
        .collect();

    Net {
        hostname: read(&paths.hostname).unwrap_or_default(),
        interface,
        gateway,
        dns: fs::read_to_string(&paths.resolv)
            .map(|text| nameservers(&text))
            .unwrap_or_default(),
        interfaces,
        public_ip: cached_public_ip(paths),
    }
}

/// The read-only `net.*` keys, as placeholders and in `keys`/`get`.
pub fn values(net: &Net) -> BTreeMap<String, String> {
    let primary = net
        .interface
        .as_deref()
        .and_then(|name| net.interfaces.iter().find(|iface| iface.name == name));
    let primary_v4 = primary.and_then(|iface| iface.addresses.iter().find(|a| a.family == "ipv4"));

    let every = |family: &str| {
        net.interfaces
            .iter()
            .flat_map(|iface| iface.addresses.iter())
            .filter(|address| address.family == family && address.scope != "loopback")
            .map(|address| address.address.clone())
            .collect::<Vec<_>>()
            .join(",")
    };

    let mut out = BTreeMap::new();
    let mut put = |key: &str, value: String| {
        out.insert(key.to_string(), value);
    };
    put("net.hostname", net.hostname.clone());
    put("net.interface", net.interface.clone().unwrap_or_default());
    put(
        "net.mac",
        primary
            .and_then(|iface| iface.mac.clone())
            .unwrap_or_default(),
    );
    put(
        "net.ip",
        primary_v4.map(|a| a.address.clone()).unwrap_or_default(),
    );
    put(
        "net.netmask",
        primary_v4
            .map(|a| netmask(a.prefix).to_string())
            .unwrap_or_default(),
    );
    put(
        "net.cidr",
        primary_v4
            .map(|a| format!("{}/{}", a.address, a.prefix))
            .unwrap_or_default(),
    );
    put("net.gateway", net.gateway.clone().unwrap_or_default());
    put("net.dns", net.dns.join(","));
    put("net.ipv4", every("ipv4"));
    put("net.ipv6", every("ipv6"));
    put("net.public_ip", net.public_ip.clone().unwrap_or_default());
    put("net.interfaces", interfaces(net));
    out
}

/// `eth0 up 10.0.0.20/24 fe80::1/64; wlan0 down` - every interface but
/// loopback, addressed or not, for a screen a technician reads at a glance.
fn interfaces(net: &Net) -> String {
    net.interfaces
        .iter()
        .filter(|iface| iface.kind != "loopback")
        .map(|iface| {
            let mut words = vec![iface.name.clone(), iface.state.clone()];
            words.extend(
                iface
                    .addresses
                    .iter()
                    .map(|a| format!("{}/{}", a.address, a.prefix)),
            );
            words.join(" ")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The interface and gateway of the IPv4 default route, from
/// `/proc/net/route`: destination and mask 0, the gateway flag set, the
/// lowest metric winning. Addresses there are little-endian hex.
pub fn default_route(table: &str) -> Option<(String, Ipv4Addr)> {
    const RTF_UP: u32 = 0x1;
    const RTF_GATEWAY: u32 = 0x2;

    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (iface, destination, gateway, flags, metric, mask) = (
                *fields.first()?,
                fields.get(1)?,
                fields.get(2)?,
                u32::from_str_radix(fields.get(3)?, 16).ok()?,
                fields.get(6)?.parse::<u32>().ok()?,
                fields.get(7)?,
            );
            let wanted = *destination == "00000000"
                && *mask == "00000000"
                && flags & RTF_UP != 0
                && flags & RTF_GATEWAY != 0;
            wanted.then(|| {
                let gateway = u32::from_str_radix(gateway, 16).ok()?;
                Some((
                    metric,
                    iface.to_string(),
                    Ipv4Addr::from(gateway.swap_bytes()),
                ))
            })?
        })
        .min_by_key(|(metric, _, _)| *metric)
        .map(|(_, iface, gateway)| (iface, gateway))
}

/// `nameserver` lines, in order, without the resolved stub.
pub fn nameservers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        if words.next() == Some("nameserver") {
            if let Some(server) = words.next() {
                if server != "127.0.0.53" && !out.iter().any(|known| known == server) {
                    out.push(server.to_string());
                }
            }
        }
    }
    out
}

pub fn netmask(prefix: u8) -> Ipv4Addr {
    let bits = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix.min(32)))
    };
    Ipv4Addr::from(bits)
}

/// How the kernel describes the device: `wireless/` for wifi, `type` 772
/// for loopback, and anything without a `device/` link to hardware is
/// virtual - a bridge, a tunnel, a veth.
fn kind(dir: &Path, name: &str) -> String {
    if read(&dir.join("type")).as_deref() == Some("772") || name == "lo" {
        "loopback"
    } else if dir.join("wireless").exists() || dir.join("phy80211").exists() {
        "wireless"
    } else if dir.join("device").exists() {
        "ethernet"
    } else {
        "virtual"
    }
    .to_string()
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTE: &str = "\
Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT
wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0
eth0\t00000000\t0100000A\t0003\t0\t0\t100\t00000000\t0\t0\t0
eth0\t0000000A\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0
";

    #[test]
    fn the_default_route_is_the_lowest_metric_one() {
        assert_eq!(
            default_route(ROUTE),
            Some(("eth0".to_string(), Ipv4Addr::new(10, 0, 0, 1)))
        );
        assert_eq!(default_route("Iface\tDestination\n"), None);
    }

    #[test]
    fn the_resolved_stub_is_not_a_dns_server() {
        let text = "# generated\nnameserver 127.0.0.53\nnameserver 192.168.1.1\nnameserver 1.1.1.1\nnameserver 192.168.1.1\nsearch lan\n";
        assert_eq!(nameservers(text), ["192.168.1.1", "1.1.1.1"]);
    }

    #[test]
    fn netmasks() {
        assert_eq!(netmask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(netmask(0), Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(netmask(32), Ipv4Addr::new(255, 255, 255, 255));
        assert_eq!(netmask(20), Ipv4Addr::new(255, 255, 240, 0));
    }

    #[test]
    fn values_come_from_the_default_route_interface() {
        let address = |address: &str, prefix, family: &str, scope: &str| NetAddress {
            address: address.to_string(),
            prefix,
            family: family.to_string(),
            scope: scope.to_string(),
        };
        let interface = |name: &str, default_route, addresses| NetInterface {
            name: name.to_string(),
            kind: if name == "lo" { "loopback" } else { "ethernet" }.to_string(),
            mac: Some(format!("02:00:00:00:00:{}", name.len())),
            state: "up".to_string(),
            carrier: Some(true),
            mtu: Some(1500),
            speed_mbps: None,
            default_route,
            addresses,
        };
        let net = Net {
            hostname: "tessaro".to_string(),
            interface: Some("eth0".to_string()),
            gateway: Some("10.0.0.1".to_string()),
            dns: vec!["10.0.0.1".to_string(), "1.1.1.1".to_string()],
            interfaces: vec![
                interface(
                    "lo",
                    false,
                    vec![address("127.0.0.1", 8, "ipv4", "loopback")],
                ),
                interface(
                    "eth0",
                    true,
                    vec![
                        address("10.0.0.20", 24, "ipv4", "global"),
                        address("fe80::1", 64, "ipv6", "link-local"),
                    ],
                ),
                interface(
                    "wlan0",
                    false,
                    vec![address("192.168.1.7", 24, "ipv4", "global")],
                ),
            ],
            public_ip: Some("203.0.113.9".to_string()),
        };

        let values = values(&net);

        assert_eq!(values["net.interface"], "eth0");
        assert_eq!(values["net.ip"], "10.0.0.20");
        assert_eq!(values["net.netmask"], "255.255.255.0");
        assert_eq!(values["net.cidr"], "10.0.0.20/24");
        assert_eq!(values["net.gateway"], "10.0.0.1");
        assert_eq!(values["net.dns"], "10.0.0.1,1.1.1.1");
        assert_eq!(values["net.mac"], "02:00:00:00:00:4");
        assert_eq!(values["net.ipv4"], "10.0.0.20,192.168.1.7");
        assert_eq!(values["net.ipv6"], "fe80::1");
        assert_eq!(values["net.public_ip"], "203.0.113.9");
        assert_eq!(
            values["net.interfaces"],
            "eth0 up 10.0.0.20/24 fe80::1/64; wlan0 up 192.168.1.7/24"
        );
        // Every read-only net.* key in the registry has a value here.
        for key in protocol::keys::KEYS {
            if key.name.starts_with("net.") {
                assert!(values.contains_key(key.name), "{}", key.name);
            }
        }
    }

    #[test]
    fn a_device_with_no_route_still_answers() {
        let net = Net {
            hostname: "tessaro".to_string(),
            interface: None,
            gateway: None,
            dns: Vec::new(),
            interfaces: Vec::new(),
            public_ip: None,
        };
        let values = values(&net);
        assert_eq!(values["net.ip"], "");
        assert_eq!(values["net.public_ip"], "");
        assert_eq!(values["net.hostname"], "tessaro");
    }

    #[test]
    fn the_public_address_is_the_ip_line_of_the_trace() {
        let body = "fl=12f1\nh=1.1.1.1\nip=203.0.113.9\nts=1758000000.1\nvisit_scheme=https\n";
        assert_eq!(parse_trace(body), "203.0.113.9".parse().ok());
        assert_eq!(parse_trace("ip=2001:db8::7\n"), "2001:db8::7".parse().ok());
        assert_eq!(parse_trace("h=1.1.1.1\n"), None);
        assert_eq!(parse_trace("ip=not-an-address\n"), None);
    }

    /// Against the real endpoint: the IP SAN has to verify through openssl.
    #[tokio::test]
    #[ignore = "needs the internet"]
    async fn cloudflare_answers_with_an_address() {
        let http = HyperHttp::new(5, 5, 4096, crate::watchdog::Heartbeat::detached());
        let ip = public_ip(&http).await.expect("the trace answers");
        assert!(!ip.is_loopback());
    }

    #[test]
    fn a_snapshot_of_this_host_does_not_fail() {
        let paths = Paths::load(&std::collections::HashMap::<String, String>::new());
        let net = snapshot(&paths);
        assert!(net.interfaces.iter().any(|iface| iface.kind == "loopback"));
    }
}
