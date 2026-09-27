//! `tessaro-ctl network ...` beyond the kernel's view: NetworkManager's
//! profiles, WiFi, a ping from the device, and `tessaro-ctl device ping` to it.
//!
//! The device manages profiles of its own and switches between them
//! through settings (`config set network.ethernet.mode=static ...`,
//! `config set network.wifi.mode=...`); `network wifi join` is the one change
//! made here, as sugar for those settings plus the password. Either way it is
//! one request: the device applies it, checks on its own that it still
//! reaches the network, and keeps it or rolls it back - so this client does
//! not have to be there for the end. When the change takes this very
//! connection away, which re-addressing the link it came in on does, the
//! answer never arrives; `network last` asks what happened.
//!
//! `network proxy set|off` are a plain `config set` of network.proxy.*:
//! not a network change, so nothing is verified or rolled back. Neither is
//! `network certs`, which is no setting at all.

use std::collections::BTreeMap;
use std::time::Duration;

use anstream::{eprintln, print, println};
use clap::{Args, Subcommand, ValueEnum};
use protocol::api::{self, Empty};
use protocol::{
    keys, speedtest_size_label, Applied, CertInfo, Net, NetChange, NetInterface, NetProfile,
    NetProfileDetail, PingEvent, ProxyStatus, Secret, SpeedtestEvent, Verify, WifiNetwork,
    WifiSecurity, WifiStatus,
};
use serde_json::json;
use tessaro_client::describe::net as describe;
use tessaro_client::network;

use crate::connect::{follow_job, Answer, Session};
use crate::style::{self, pad, paint};
use crate::{print, show_applied, show_once};

/// `tessaro-ctl network ...`.
#[derive(Subcommand)]
pub enum NetworkCmd {
    /// The network as the device sees it: address, gateway, DNS, and every
    /// interface - the same values as the network.* keys.
    Show,
    /// Every network interface: kind, state, MAC, MTU, addresses.
    Interfaces,
    /// NetworkManager's profiles. The device keeps a change to them only if
    /// it still reaches the network afterwards.
    #[command(subcommand)]
    Profiles(ProfilesCmd),
    /// What the last network change did: for when the change took the
    /// connection that asked for it.
    Last,
    /// Ping HOST from the device, not from here.
    Ping {
        host: String,
        #[arg(long, short = 'c', default_value_t = protocol::PING_DEFAULT_COUNT)]
        count: u32,
        /// Seconds between echoes.
        #[arg(long, short = 'i', default_value_t = seconds(protocol::PING_DEFAULT_INTERVAL_MS))]
        interval: f64,
        /// Seconds to wait for each reply.
        #[arg(long, short = 'W', default_value_t = seconds(protocol::PING_DEFAULT_TIMEOUT_MS))]
        timeout: f64,
        /// Send from this interface.
        #[arg(long, short = 'I')]
        interface: Option<String>,
    },
    /// WiFi: the radio, scanning, joining a network, the hotspot password.
    /// `config set network.wifi.mode=hotspot` goes back to the hotspot.
    #[command(subcommand)]
    Wifi(WifiCmd),
    /// The proxy the device reaches the internet through: the browser, the
    /// reachability probe, the public address and the speed test.
    #[command(subcommand)]
    Proxy(ProxyCmd),
    /// Extra certificate authorities the device trusts, on top of the
    /// image's: for an intranet site or a proxy that inspects TLS.
    #[command(subcommand)]
    Certs(CertsCmd),
    /// Measure the device's internet connection against speed.cloudflare.com:
    /// latency, then download and upload at growing payload sizes.
    ///
    /// Runs on the device, so it measures the kiosk's link, not this one.
    /// A full run moves a few hundred MB - mind a metered connection, and
    /// use a smaller --max-size there.
    ///
    ///   tessaro-ctl network speedtest --max-size 1m --tests 3
    Speedtest {
        /// Largest payload: 100k, 1m, 10m, 25m or 100m. Uploads stop at 25m.
        #[arg(long, default_value = "25m", value_parser = parse_payload)]
        max_size: u64,
        /// Samples per payload size.
        #[arg(long, default_value_t = protocol::SPEEDTEST_DEFAULT_TESTS)]
        tests: u32,
        /// Go around network.proxy.url, straight out: the link itself rather
        /// than the proxy. Nothing without a proxy.
        #[arg(long)]
        no_proxy: bool,
    },
}

#[derive(Subcommand)]
pub enum ProxyCmd {
    /// The proxy (its password masked), what bypasses it, and the device's
    /// local proxy that carries it.
    Show,
    /// Send everything through URL: http://host:port or socks5://host:port,
    /// with user:password@ in it if the proxy wants a login. A password
    /// with $ " ' \ ` or @ in it is written percent-encoded (%24 for $).
    /// The same as `tessaro-ctl config set network.proxy.url=URL`; the
    /// browser restarts when the proxy is switched on.
    ///
    ///   tessaro-ctl network proxy set http://10.0.0.5:3128
    ///   tessaro-ctl network proxy set 'http://jan:s3cret@proxy.corp.test:8080' --bypass .corp.test,10.0.0.0/8
    ///   tessaro-ctl network proxy set socks5://10.0.0.5:1080
    Set {
        url: String,
        /// What goes around the proxy, comma separated: host names,
        /// .domain suffixes, IP addresses and networks. Sets
        /// network.proxy.bypass in the same change.
        #[arg(long)]
        bypass: Option<String>,
    },
    /// Stop using a proxy: everything goes straight out again. The browser
    /// restarts.
    Off,
    /// Fetch Cloudflare's trace through the proxy, from the device: the
    /// address the internet sees it at, or why the proxy did not get there.
    Test,
}

#[derive(Subcommand)]
pub enum CertsCmd {
    /// The extra certificate authorities: fingerprint, subject, expiry.
    List,
    /// Trust the certificates in FILE, PEM or DER, one or a chain. The
    /// browser takes them without a restart; the agent restarts to take
    /// them for its reachability probe. Never send a private key.
    ///
    ///   tessaro-ctl network certs add corp-root-ca.pem
    Add { file: std::path::PathBuf },
    /// Stop trusting one: its SHA-256 fingerprint, a unique prefix of it,
    /// or its exact subject.
    Revoke { cert: String },
}

#[derive(Subcommand)]
pub enum ProfilesCmd {
    /// The device's own tessaro-* profiles, and any made by hand - which is
    /// active where, and which come up on their own.
    List,
    /// One profile, by name or uuid: addressing, DNS, WiFi - never its
    /// password - and what its device has right now.
    Show { profile: String },
}

#[derive(Subcommand)]
pub enum WifiCmd {
    /// The WiFi radio and what each WiFi device is connected to.
    Status,
    /// The networks in range, strongest first.
    Scan {
        #[arg(long, short = 'I')]
        interface: Option<String>,
        /// What the device saw last, without scanning again.
        #[arg(long)]
        cached: bool,
    },
    /// Join SSID as a client: network.wifi.mode=client, and the hotspot goes
    /// down. The password is prompted, or read from stdin with
    /// --password-stdin - never taken from the command line, never shown by
    /// `config get`. Rejoining the same network keeps its saved password if
    /// you give none. `config set network.wifi.mode=hotspot` brings the
    /// hotspot back.
    ///
    ///   tessaro-ctl network wifi join Office
    ///   tessaro-ctl network wifi join Backroom --hidden --security psk
    Join {
        ssid: String,
        /// Read the password from stdin instead of prompting.
        #[arg(long)]
        password_stdin: bool,
        /// A network that does not broadcast its name. Needs --security.
        #[arg(long, requires = "security")]
        hidden: bool,
        /// The network's security, which a scan finds by itself for a
        /// network that is not hidden.
        #[arg(long, value_enum)]
        security: Option<Security>,
        #[command(flatten)]
        verify: VerifyArg,
    },
    /// A new random password for the hotspot, shown once. Claimed devices
    /// only: an unclaimed device's hotspot is open.
    HotspotPassword,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Security {
    /// WPA2 personal.
    Psk,
    /// WPA3 personal.
    Sae,
    Open,
}

impl From<Security> for WifiSecurity {
    fn from(security: Security) -> Self {
        match security {
            Security::Psk => WifiSecurity::Psk,
            Security::Sae => WifiSecurity::Sae,
            Security::Open => WifiSecurity::Open,
        }
    }
}

#[derive(Args)]
pub struct VerifyArg {
    /// What the device checks before it keeps the change: `gateway` (it
    /// answers a ping), `HOST` (a ping), `HOST:PORT` (a TCP connection), or
    /// `none`. A change must also leave a default route if there was one.
    #[arg(long, value_name = "CHECK", default_value = "gateway", value_parser = Verify::parse)]
    pub verify: Verify,
}

fn speedtest(
    session: &mut Session,
    json: bool,
    max_size: u64,
    tests: u32,
    direct: bool,
) -> Result<(), String> {
    if !json {
        eprintln!(
            "{}",
            paint(
                style::MUTED,
                format!(
                    "{}: measuring against speed.cloudflare.com, up to {} per sample{}...",
                    session.node.name,
                    speedtest_size_label(max_size),
                    if direct { ", around any proxy" } else { "" }
                )
            )
        );
    }
    follow_job::<api::network::Speedtest, _>(
        session,
        api::SpeedtestBody {
            max_size: Some(max_size),
            tests: Some(tests),
            direct,
        },
        json,
        |step: SpeedtestEvent| {
            println!(
                "{}",
                style::line(&tessaro_client::speedtest::event_line(&step))
            )
        },
    )
}

/// `--max-size`: one of the sizes the device offers, as `100k`, `1m`, ...
fn parse_payload(text: &str) -> Result<u64, String> {
    tessaro_client::speedtest::parse_size(text)
}

/// One certificate authority on a line: fingerprint, subject, expiry, and
/// who issued it when that is not itself.
fn show_cert(cert: &CertInfo) {
    println!("{}", style::line(&describe::cert(cert)));
}

fn show_proxy(status: &ProxyStatus) {
    lines(describe::proxy(status));
}

/// Shared lines, each on its own.
fn lines(lines: Vec<tessaro_client::text::Line>) {
    for line in lines {
        println!("{}", style::line(&line));
    }
}

fn show_net(net: &Net) {
    let primary = net
        .interface
        .as_deref()
        .and_then(|name| net.interfaces.iter().find(|iface| iface.name == name));
    let address = primary.and_then(|iface| iface.addresses.iter().find(|a| a.family == "ipv4"));
    let none = paint(style::MUTED, "(none)");
    let row = style::row;

    row("hostname", &paint(style::HEADING, &net.hostname));
    row(
        "interface",
        &net.interface
            .clone()
            .unwrap_or_else(|| paint(style::WARN, "(no default route)")),
    );
    row(
        "address",
        &address
            .map(|a| format!("{}/{}", a.address, a.prefix))
            .unwrap_or_else(|| none.clone()),
    );
    row("gateway", net.gateway.as_ref().unwrap_or(&none));
    row("public ip", net.public_ip.as_ref().unwrap_or(&none));
    if let Some(proxy) = &net.proxy {
        row("proxy", proxy);
    }
    row(
        "dns",
        &if net.dns.is_empty() {
            none.clone()
        } else {
            net.dns.join(", ")
        },
    );
    if let Some(mac) = primary.and_then(|iface| iface.mac.as_ref()) {
        row("mac", mac);
    }
    println!();
    println!("{}", paint(style::HEADING, "interfaces:"));
    for iface in &net.interfaces {
        let addresses: Vec<String> = iface
            .addresses
            .iter()
            .map(|a| format!("{}/{}", a.address, a.prefix))
            .collect();
        let marker = if iface.default_route {
            paint(style::OK, " *")
        } else {
            String::new()
        };
        println!(
            "  {} {} {} {}{marker}",
            pad(style::HEADING, &iface.name, 12),
            pad(style::MUTED, &iface.kind, 9),
            pad(style::link_state(&iface.state), &iface.state, 8),
            if addresses.is_empty() {
                paint(style::MUTED, "-")
            } else {
                addresses.join(" ")
            }
        );
    }
    println!(
        "\n  {}",
        paint(
            style::MUTED,
            "* carries the default route. `tessaro-ctl network interfaces` for details."
        )
    );
}

fn show_interface(iface: &NetInterface) {
    let marker = if iface.default_route {
        paint(style::OK, "  (default route)")
    } else {
        String::new()
    };
    let row = style::sub_row;
    println!("{}{marker}", paint(style::HEADING, &iface.name));
    row("kind", &iface.kind);
    row(
        "state",
        &paint(style::link_state(&iface.state), &iface.state),
    );
    if let Some(carrier) = iface.carrier {
        row("carrier", &style::yes_no(carrier));
    }
    if let Some(mac) = &iface.mac {
        row("mac", mac);
    }
    if let Some(mtu) = iface.mtu {
        row("mtu", &mtu.to_string());
    }
    if let Some(speed) = iface.speed_mbps {
        row("speed", &format!("{speed} Mb/s"));
    }
    for address in &iface.addresses {
        row(
            &address.family,
            &format!(
                "{}/{}  {}",
                address.address,
                address.prefix,
                paint(style::MUTED, format!("({})", address.scope))
            ),
        );
    }
}

/// A protocol default in milliseconds, as the seconds the command line takes.
pub(crate) fn seconds(millis: u64) -> f64 {
    millis as f64 / 1000.0
}

pub fn run(session: &mut Session, command: NetworkCmd, json: bool) -> Result<(), String> {
    match command {
        NetworkCmd::Show => {
            let net = session.fetch::<api::network::Show>()?;
            print(json, &net, || show_net(&net))
        }
        NetworkCmd::Interfaces => {
            let net = session.fetch::<api::network::Show>()?;
            print(json, &net.interfaces, || {
                for (at, interface) in net.interfaces.iter().enumerate() {
                    if at > 0 {
                        println!();
                    }
                    show_interface(interface);
                }
            })
        }
        NetworkCmd::Speedtest {
            max_size,
            tests,
            no_proxy,
        } => speedtest(session, json, max_size, tests, no_proxy),
        NetworkCmd::Proxy(ProxyCmd::Show) => {
            let status = session.fetch::<api::network::Proxy>()?;
            print(json, &status, || show_proxy(&status))
        }
        NetworkCmd::Proxy(ProxyCmd::Set { url, bypass }) => {
            let url = network::proxy_url(&url, "", "")?;
            let mut values = BTreeMap::from([(keys::PROXY_URL.to_string(), url)]);
            if let Some(bypass) = bypass {
                values.insert(keys::PROXY_BYPASS.to_string(), bypass);
            }
            let applied = crate::set(session, values)?;
            print(json, &applied, || show_applied(&applied, false))
        }
        NetworkCmd::Proxy(ProxyCmd::Off) => {
            let values = BTreeMap::from([(keys::PROXY_URL.to_string(), String::new())]);
            let applied = crate::set(session, values)?;
            print(json, &applied, || show_applied(&applied, false))
        }
        NetworkCmd::Proxy(ProxyCmd::Test) => {
            let tested = session.send::<api::network::ProxyTest>(())?;
            print(json, &tested, || {
                println!("{}", style::line(&describe::proxy_test(&tested)))
            })
        }
        NetworkCmd::Certs(CertsCmd::List) => {
            let certs = session.fetch::<api::network::Certs>()?;
            print(json, &certs, || {
                if certs.is_empty() {
                    println!(
                        "{}",
                        paint(style::MUTED, "no extra certificate authorities")
                    );
                }
                for cert in &certs {
                    show_cert(cert);
                }
            })
        }
        NetworkCmd::Certs(CertsCmd::Add { file }) => {
            let pem = tessaro_client::certs::read_pem(&file)?;
            let added = session.send::<api::network::CertAdd>(api::CertBody { pem })?;
            print(json, &added, || {
                for cert in &added.added {
                    print!("{} ", paint(style::OK, "trusted"));
                    show_cert(cert);
                }
                for cert in &added.present {
                    print!("{} ", paint(style::MUTED, "already trusted"));
                    show_cert(cert);
                }
            })
        }
        NetworkCmd::Certs(CertsCmd::Revoke { cert }) => {
            let revoked = session.call::<api::network::CertRevoke>(api::CertQuery { cert }, ())?;
            print(json, &revoked, || {
                print!("{} ", paint(style::OK, "revoked"));
                show_cert(&revoked);
            })
        }
        NetworkCmd::Profiles(ProfilesCmd::List) => {
            let profiles = session.fetch::<api::network::Profiles>()?;
            print(json, &profiles, || show_profiles(&profiles))
        }
        NetworkCmd::Profiles(ProfilesCmd::Show { profile }) => {
            let detail =
                session.call::<api::network::Profile>(api::ProfileQuery { profile }, ())?;
            print(json, &detail, || show_detail(&detail))
        }
        NetworkCmd::Last => {
            let last = session.fetch::<api::network::Last>()?;
            print(json, &last, || match &last {
                Some(change) => show_change(change),
                None => println!("{}", paint(style::MUTED, "no network change yet")),
            })
        }
        NetworkCmd::Ping {
            host,
            count,
            interval,
            timeout,
            interface,
        } => net_ping(session, json, host, count, interval, timeout, interface),
        NetworkCmd::Wifi(what) => wifi(session, json, what),
    }
}

fn wifi(session: &mut Session, json: bool, what: WifiCmd) -> Result<(), String> {
    match what {
        WifiCmd::Status => {
            let status = session.fetch::<api::network::Wifi>()?;
            print(json, &status, || show_wifi(&status))
        }
        WifiCmd::Scan { interface, cached } => {
            if !json && !cached {
                eprintln!(
                    "{}",
                    paint(style::MUTED, format!("{}: scanning...", session.node.name))
                );
            }
            let query = api::WifiScanQuery {
                interface,
                rescan: !cached,
            };
            let networks = session.call::<api::network::WifiScan>(query, ())?;
            print(json, &networks, || show_networks(&networks))
        }
        WifiCmd::Join {
            ssid,
            password_stdin,
            hidden,
            security,
            verify,
        } => {
            let psk = join_password(session, &ssid, security, password_stdin)?;
            let doing = format!("joining {ssid}");
            let body = api::WifiJoinBody {
                ssid,
                psk,
                security: security.map(WifiSecurity::from),
                hidden,
                verify: verify.verify.clone(),
            };
            apply(session, json, &doing, &verify.verify, |session| {
                session.request::<api::network::WifiJoin>(Empty {}, body)
            })
        }
        WifiCmd::HotspotPassword => {
            let hotspot = session.send::<api::network::HotspotPassword>(())?;
            print(json, &hotspot, || {
                show_once(
                    &format!("hotspot {} password - shown this once:", hotspot.ssid),
                    &hotspot.password,
                )
            })
        }
    }
}

/// The password to join with, if the network wants one: what the last scan
/// says about it decides whether to ask, and a known network may keep the
/// one it has.
fn join_password(
    session: &mut Session,
    ssid: &str,
    security: Option<Security>,
    from_stdin: bool,
) -> Result<Option<Secret>, String> {
    let seen = match security {
        Some(_) => None,
        None => {
            let query = api::WifiScanQuery {
                interface: None,
                rescan: false,
            };
            let networks = session
                .call::<api::network::WifiScan>(query, ())
                .unwrap_or_default();
            networks.into_iter().find(|network| network.ssid == ssid)
        }
    };
    let security = security.map(WifiSecurity::from);
    if network::wifi_open(security, seen.as_ref()) {
        return Ok(None);
    }
    let prompt = if network::wifi_known(seen.as_ref()) {
        format!("WiFi password for {ssid} (empty keeps the saved one): ")
    } else {
        format!("WiFi password for {ssid}: ")
    };
    let psk = crate::prompt::password(from_stdin, &prompt)?;
    network::wifi_psk(&psk, security, seen.as_ref())
}

pub use network::is_network_key;

/// Send one change of network settings - a `config set`, a `config unset`,
/// a join - with `request`, and wait for the device's verdict, or explain
/// why it never came.
pub fn apply(
    session: &mut Session,
    json: bool,
    doing: &str,
    verify: &Verify,
    request: impl FnOnce(&mut Session) -> Answer<Applied>,
) -> Result<(), String> {
    if !json {
        eprintln!(
            "{}",
            style::line(&network::notice(&session.node.name, doing, verify))
        );
    }
    let applied = match network::apply(session, request) {
        Answer::Ok(applied) => applied,
        // A rolled-back change is the device refusing it, with the reason.
        Answer::Refused(error) => return Err(error),
        Answer::Lost(why) => {
            eprintln!("{}", paint(style::WARN, network::lost(&why)));
            eprintln!("{}", style::line(&network::lost_hint()));
            return Err("no answer from the device".to_string());
        }
    };
    print(json, &applied, || {
        if let Some(change) = &applied.network {
            show_change(change);
        }
        show_applied(&applied, false);
    })
}

#[allow(clippy::too_many_arguments)]
fn net_ping(
    session: &mut Session,
    json: bool,
    host: String,
    count: u32,
    interval: f64,
    timeout: f64,
    interface: Option<String>,
) -> Result<(), String> {
    let millis = |seconds: f64| (seconds * 1000.0).round().max(0.0) as u64;
    let mut failed = false;
    follow_job::<api::network::Ping, _>(
        session,
        api::PingBody {
            host,
            count: Some(count),
            interval_ms: Some(millis(interval)),
            timeout_ms: Some(millis(timeout)),
            interface,
        },
        json,
        |step: PingEvent| {
            if let PingEvent::Summary { received: 0, .. } = step {
                failed = true;
            }
            println!("{}", style::line(&tessaro_client::ping::event_line(&step)));
        },
    )?;
    if failed {
        return Err("no replies".to_string());
    }
    Ok(())
}

/// `tessaro-ctl device ping`: the path this client really uses - the TCP connect,
/// the TLS handshake with asking the device who it is, then round trips on
/// the session.
pub fn ping(session: &mut Session, json: bool, count: u32, interval: f64) -> Result<(), String> {
    tessaro_client::ping::check_count(count).map_err(|err| format!("--count: {err}"))?;
    let timing = session.timing;
    if !json {
        let mut intro = tessaro_client::ping::intro(session).into_iter();
        if let Some(first) = intro.next() {
            eprintln!("{}", style::line(&first));
        }
        for line in intro {
            println!("{}", style::line(&line));
        }
    }

    let interval = Duration::from_secs_f64(interval.max(0.0));
    let summary = tessaro_client::ping::device(session, count, interval, &mut Stdout { json })?;
    if json {
        let seconds = |d: &Duration| d.as_secs_f64() * 1000.0;
        return crate::print_json(&json!({
            "node": session.node.name,
            "connect_ms": timing.map(|t| seconds(&t.connect)),
            "handshake_ms": timing.map(|t| seconds(&t.handshake)),
            "sent": summary.sent,
            "received": summary.received(),
            "rtt_ms": summary.rtt_ms(),
            "min_ms": summary.min_ms(),
            "avg_ms": summary.avg_ms(),
            "max_ms": summary.max_ms(),
        }));
    }
    println!("{}", style::line(&summary.line()));
    if summary.received() < summary.sent {
        return Err("the device did not answer every ping".to_string());
    }
    Ok(())
}

/// A ping's lines on stdout, as `ping` prints them; nothing with `--json`.
struct Stdout {
    json: bool,
}

impl tessaro_client::report::Report for Stdout {
    fn progress(&mut self, _: tessaro_client::text::Line, _: u64, _: u64) {}

    fn line(&mut self, line: tessaro_client::text::Line) {
        if !self.json {
            println!("{}", style::line(&line));
        }
    }
}

fn show_profiles(profiles: &[NetProfile]) {
    lines(describe::profiles(profiles));
}

fn show_detail(detail: &NetProfileDetail) {
    lines(describe::profile(detail));
}

fn show_wifi(status: &WifiStatus) {
    lines(describe::wifi(status));
}

fn show_networks(networks: &[WifiNetwork]) {
    lines(describe::networks(networks));
}

pub fn show_change(change: &NetChange) {
    lines(describe::change(change));
}
