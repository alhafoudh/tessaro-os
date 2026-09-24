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

use std::time::{Duration, Instant};

use anstream::{eprintln, println};
use clap::{Args, Subcommand, ValueEnum};
use protocol::{
    speedtest_size_label, Applied, ChangeOutcome, Command, Direction, Done, HotspotCredentials,
    Net, NetAddress, NetChange, NetInterface, NetProfile, NetProfileDetail, PingEvent, Secret,
    SpeedtestEvent, Verify, WifiNetwork, WifiSecurity, WifiStatus,
};
use serde_json::json;

use crate::connect::{Answer, Session, StreamEvents};
use crate::style::{self, pad, paint, yes_no};
use crate::{print, show_applied, show_once};

/// Longer than the device takes to apply, check and roll back a change
/// (about 90s at most), so its answer is waited for - and short enough that
/// a connection the change silently broke does not hang the terminal.
const CHANGE: Duration = Duration::from_secs(180);

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
    },
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

fn speedtest(session: &mut Session, json: bool, max_size: u64, tests: u32) -> Result<(), String> {
    if !json {
        eprintln!(
            "{}",
            paint(
                style::MUTED,
                format!(
                    "{}: measuring against speed.cloudflare.com, up to {} per sample...",
                    session.node.name,
                    speedtest_size_label(max_size)
                )
            )
        );
    }
    session.stream_events(
        Command::Speedtest {
            max_size: Some(max_size),
            tests: Some(tests),
        },
        json,
        |step: SpeedtestEvent| println!("{}", speedtest_line(&step)),
    )
}

/// `--max-size`: one of the sizes the device offers, as `100k`, `1m`, ...
fn parse_payload(text: &str) -> Result<u64, String> {
    protocol::SPEEDTEST_SIZES
        .into_iter()
        .find(|size| speedtest_size_label(*size) == text.to_ascii_lowercase())
        .ok_or_else(|| {
            let offered: Vec<String> = protocol::SPEEDTEST_SIZES.map(speedtest_size_label).into();
            format!("one of {}", offered.join(", "))
        })
}

fn speedtest_line(step: &SpeedtestEvent) -> String {
    // The headline number in `style`, a missing one muted; the spread and the
    // sample counts are background.
    let value = |style: anstyle::Style, v: Option<f64>, unit: &str| match v {
        Some(v) => paint(style, format!("{v:.1} {unit}")),
        None => paint(style::MUTED, "n/a"),
    };
    let mbit = |style, v| value(style, v, "Mbit/s");
    let ms = |style, v| value(style, v, "ms");
    let label = style::label;
    match step {
        SpeedtestEvent::Server { ip, colo, country } => format!(
            "{} Cloudflare {}, seen from {ip} ({country})",
            label("server"),
            paint(style::HEADING, colo)
        ),
        SpeedtestEvent::Latency {
            samples,
            avg_ms,
            min_ms,
            max_ms,
        } => format!(
            "{} {} {}",
            label("latency"),
            ms(style::HEADING, *avg_ms),
            paint(
                style::MUTED,
                format!(
                    "(min {}, max {}, {samples} samples)",
                    ms(anstyle::Style::new(), *min_ms),
                    ms(anstyle::Style::new(), *max_ms)
                )
            )
        ),
        SpeedtestEvent::Transfer {
            direction,
            size,
            samples,
            attempts,
            median_mbit,
            min_mbit,
            max_mbit,
        } => {
            let direction = match direction {
                Direction::Download => "download",
                Direction::Upload => "upload",
            };
            // Samples short of the attempts means retries: worth noticing.
            let counted = if samples < attempts {
                style::WARN
            } else {
                style::MUTED
            };
            format!(
                "{} {} {} {} {}",
                label(direction),
                pad(style::HEADING, speedtest_size_label(*size), 5),
                mbit(style::HEADING, *median_mbit),
                paint(
                    style::MUTED,
                    format!(
                        "(min {}, max {},",
                        mbit(anstyle::Style::new(), *min_mbit),
                        mbit(anstyle::Style::new(), *max_mbit)
                    )
                ),
                paint(counted, format!("{samples}/{attempts} samples)"))
            )
        }
        SpeedtestEvent::Result {
            download_mbit,
            upload_mbit,
            latency_ms,
        } => format!(
            "{} download {}, upload {}, latency {}",
            pad(style::HEADING, "result", 9),
            mbit(style::OK, *download_mbit),
            mbit(style::OK, *upload_mbit),
            ms(style::OK, *latency_ms)
        ),
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
            let net: Net = session.call(Command::Net)?;
            print(json, &net, || show_net(&net))
        }
        NetworkCmd::Interfaces => {
            let net: Net = session.call(Command::Net)?;
            print(json, &net.interfaces, || {
                for (at, interface) in net.interfaces.iter().enumerate() {
                    if at > 0 {
                        println!();
                    }
                    show_interface(interface);
                }
            })
        }
        NetworkCmd::Speedtest { max_size, tests } => speedtest(session, json, max_size, tests),
        NetworkCmd::Profiles(ProfilesCmd::List) => {
            let profiles: Vec<NetProfile> = session.call(Command::NetProfiles)?;
            print(json, &profiles, || show_profiles(&profiles))
        }
        NetworkCmd::Profiles(ProfilesCmd::Show { profile }) => {
            let detail: NetProfileDetail = session.call(Command::NetShow { profile })?;
            print(json, &detail, || show_detail(&detail))
        }
        NetworkCmd::Last => {
            let last: Option<NetChange> = session.call(Command::NetLast)?;
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
            let status: WifiStatus = session.call(Command::Wifi)?;
            print(json, &status, || show_wifi(&status))
        }
        WifiCmd::Scan { interface, cached } => {
            if !json && !cached {
                eprintln!(
                    "{}",
                    paint(style::MUTED, format!("{}: scanning...", session.node.name))
                );
            }
            let networks: Vec<WifiNetwork> = session.call(Command::WifiScan {
                interface,
                rescan: !cached,
            })?;
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
            apply(
                session,
                json,
                &format!("joining {ssid}"),
                &verify.verify,
                Command::WifiJoin {
                    ssid,
                    psk: psk.map(Secret),
                    security: security.map(WifiSecurity::from),
                    hidden,
                    verify: verify.verify.clone(),
                },
            )
        }
        WifiCmd::HotspotPassword => {
            let hotspot: HotspotCredentials = session.call(Command::HotspotPassword)?;
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
) -> Result<Option<String>, String> {
    let seen = match security {
        Some(_) => None,
        None => {
            let networks: Vec<WifiNetwork> = session
                .call(Command::WifiScan {
                    interface: None,
                    rescan: false,
                })
                .unwrap_or_default();
            networks.into_iter().find(|network| network.ssid == ssid)
        }
    };
    let open = matches!(security, Some(Security::Open))
        || seen
            .as_ref()
            .is_some_and(|network| network.security == "open");
    if open {
        return Ok(None);
    }
    let known = seen.as_ref().is_some_and(|network| network.known);
    let prompt = if known {
        format!("WiFi password for {ssid} (empty keeps the saved one): ")
    } else {
        format!("WiFi password for {ssid}: ")
    };
    let psk = crate::prompt::password(from_stdin, &prompt)?;
    if psk.is_empty() && known {
        return Ok(None);
    }
    protocol::keys::check_psk(&psk)?;
    Ok(Some(psk))
}

/// Whether `key` is one of the device's network settings, whose change runs
/// as a verified network transaction.
pub fn is_network_key(key: &str) -> bool {
    protocol::keys::find(key)
        .is_some_and(|key| key.consumers.contains(&protocol::keys::Consumer::Network))
}

/// Send one change of network settings - a `config set`, a `config unset`,
/// a join - and
/// wait for the device's verdict, or explain why it never came.
pub fn apply(
    session: &mut Session,
    json: bool,
    doing: &str,
    verify: &Verify,
    command: Command,
) -> Result<(), String> {
    if !json {
        eprintln!(
            "{}",
            paint(
                style::MUTED,
                format!(
                    "{}: {doing}; kept only if {} - this can take a minute...",
                    session.node.name,
                    verify.describe()
                )
            )
        );
    }
    session.set_read_timeout(Some(CHANGE));
    let answer = session.request::<Applied>(command);
    session.restore_read_timeout();

    let applied = match answer {
        Answer::Ok(applied) => applied,
        // A rolled-back change is the device refusing it, with the reason.
        Answer::Refused(error) => return Err(error),
        Answer::Lost(why) => {
            eprintln!(
                "{}",
                paint(
                    style::WARN,
                    format!(
                        "lost the connection while the device applied the change ({why}).\n\
                         That is expected when it moved the link this connection came in on: \
                         the device keeps the change or rolls it back on its own."
                    )
                )
            );
            eprintln!(
                "see what it did with: {}",
                paint(
                    style::CMD,
                    "tessaro-ctl network last   (at the new address, if it changed)"
                )
            );
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
    session.stream_events(
        Command::NetPing {
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
            println!("{}", ping_line(&step));
        },
    )?;
    if failed {
        return Err("no replies".to_string());
    }
    Ok(())
}

/// `tessaro-ctl device ping`: the path this client really uses - the TCP connect,
/// the TLS handshake with the hello, then round trips on the session.
pub fn ping(session: &mut Session, json: bool, count: u32, interval: f64) -> Result<(), String> {
    if count == 0 {
        return Err("--count must be at least 1".to_string());
    }
    let timing = session.timing;
    let label = style::label;
    if !json {
        match session.remote.as_ref() {
            Some((address, _)) => eprintln!(
                "{}",
                paint(
                    style::MUTED,
                    format!("{} ({address}): the control connection", session.node.name)
                )
            ),
            None => eprintln!(
                "{}",
                paint(
                    style::MUTED,
                    format!("{}: the local socket", session.node.name)
                )
            ),
        }
        if let Some(timing) = timing {
            println!(
                "{} {}",
                label("connect"),
                paint(style::HEADING, ms(timing.connect))
            );
            println!(
                "{} {} {}",
                label("tls"),
                paint(style::HEADING, ms(timing.handshake)),
                paint(style::MUTED, "(handshake and hello)")
            );
        }
    }

    let mut rtts = Vec::new();
    let mut lost = 0u32;
    for seq in 1..=count {
        if seq > 1 {
            std::thread::sleep(Duration::from_secs_f64(interval.max(0.05)));
        }
        let started = Instant::now();
        match session.call::<Done>(Command::Ping) {
            Ok(_) => {
                let rtt = started.elapsed();
                rtts.push(rtt);
                if !json {
                    println!(
                        "{} {} {}",
                        label("reply"),
                        paint(style::HEADING, ms(rtt)),
                        paint(style::MUTED, format!("seq={seq}"))
                    );
                }
            }
            Err(error) => {
                lost = count - seq + 1;
                if !json {
                    println!(
                        "{} {} {}",
                        label("lost"),
                        paint(style::BAD, &error),
                        paint(style::MUTED, format!("seq={seq}"))
                    );
                }
                break;
            }
        }
    }

    let seconds = |d: &Duration| d.as_secs_f64() * 1000.0;
    let min = rtts.iter().map(seconds).reduce(f64::min);
    let max = rtts.iter().map(seconds).reduce(f64::max);
    let avg = (!rtts.is_empty()).then(|| rtts.iter().map(seconds).sum::<f64>() / rtts.len() as f64);
    if json {
        return crate::print_json(&json!({
            "node": session.node.name,
            "connect_ms": timing.map(|t| seconds(&t.connect)),
            "handshake_ms": timing.map(|t| seconds(&t.handshake)),
            "sent": count,
            "received": rtts.len(),
            "rtt_ms": rtts.iter().map(seconds).collect::<Vec<_>>(),
            "min_ms": min,
            "avg_ms": avg,
            "max_ms": max,
        }));
    }
    println!(
        "{}",
        summary_line(count, rtts.len() as u32, min, avg, max, "answered")
    );
    if lost > 0 {
        return Err("the device stopped answering".to_string());
    }
    Ok(())
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

fn summary_line(
    sent: u32,
    received: u32,
    min: Option<f64>,
    avg: Option<f64>,
    max: Option<f64>,
    verb: &str,
) -> String {
    let loss = ((sent - received.min(sent)) * 100)
        .checked_div(sent)
        .unwrap_or(0);
    let loss_style = match loss {
        0 => style::OK,
        100 => style::BAD,
        _ => style::WARN,
    };
    let number = |v: Option<f64>| match v {
        Some(v) => format!("{v:.1}"),
        None => "n/a".to_string(),
    };
    let spread = match (min, avg, max) {
        (None, None, None) => String::new(),
        _ => format!(
            " {}",
            paint(
                style::MUTED,
                format!(
                    "(min {}, avg {}, max {} ms)",
                    paint(anstyle::Style::new(), number(min)),
                    paint(anstyle::Style::new(), number(avg)),
                    paint(anstyle::Style::new(), number(max))
                )
            )
        ),
    };
    format!(
        "{} {received}/{sent} {verb}, {}{spread}",
        pad(style::HEADING, "result", 9),
        paint(loss_style, format!("{loss}% lost"))
    )
}

fn ping_line(step: &PingEvent) -> String {
    let label = style::label;
    match step {
        PingEvent::Start { host, address } => {
            let target = if host == address {
                paint(style::HEADING, host)
            } else {
                format!("{} ({address})", paint(style::HEADING, host))
            };
            format!("{} {target}", label("ping"))
        }
        PingEvent::Reply { seq, bytes, rtt_ms } => format!(
            "{} {} {}",
            label("reply"),
            paint(style::HEADING, format!("{rtt_ms:.1} ms")),
            paint(style::MUTED, format!("seq={seq} {bytes} bytes"))
        ),
        PingEvent::Timeout { seq } => format!(
            "{} {} {}",
            label("timeout"),
            paint(style::WARN, "no reply"),
            paint(style::MUTED, format!("seq={seq}"))
        ),
        PingEvent::Summary {
            sent,
            received,
            min_ms,
            avg_ms,
            max_ms,
        } => summary_line(*sent, *received, *min_ms, *avg_ms, *max_ms, "received"),
    }
}

fn show_profiles(profiles: &[NetProfile]) {
    if profiles.is_empty() {
        println!("{}", paint(style::MUTED, "no profiles"));
    }
    for profile in profiles {
        let device = match (&profile.device, profile.active) {
            (Some(device), true) => pad(style::OK, device, 10),
            (Some(device), false) => pad(style::MUTED, device, 10),
            (None, _) => pad(style::MUTED, "-", 10),
        };
        let state = if profile.active {
            pad(style::OK, "active", 7)
        } else {
            pad(style::MUTED, "", 7)
        };
        let saved = if profile.managed {
            format!("  {}", paint(style::MUTED, "(managed)"))
        } else if profile.saved {
            String::new()
        } else {
            format!("  {}", paint(style::MUTED, "(not saved)"))
        };
        println!(
            "{}  {} {device} {state} {} {}  {}{saved}",
            pad(style::HEADING, &profile.name, 24),
            pad(style::MUTED, &profile.kind, 9),
            paint(style::LABEL, "autoconnect"),
            yes_no(profile.autoconnect),
            paint(style::MUTED, &profile.uuid),
        );
    }
}

fn show_detail(detail: &NetProfileDetail) {
    let profile = &detail.profile;
    let none = || paint(style::MUTED, "(none)");
    let row = style::row;
    let sub = |label: &str, value: &str| println!("    {} {value}", pad(style::LABEL, label, 15));

    println!(
        "{} {}",
        paint(style::HEADING, &profile.name),
        paint(style::MUTED, format!("({})", profile.uuid))
    );
    row("kind", &profile.kind);
    row(
        "device",
        &match (&profile.device, profile.active) {
            (Some(device), true) => format!("{device}  {}", paint(style::OK, "active")),
            (Some(device), false) => format!("{device}  {}", paint(style::MUTED, "(not active)")),
            (None, _) => paint(style::MUTED, "(any, not active)"),
        },
    );
    row(
        "autoconnect",
        &format!(
            "{}  {}",
            yes_no(profile.autoconnect),
            paint(style::MUTED, format!("(priority {})", profile.priority))
        ),
    );
    row(
        "saved",
        &if profile.managed {
            format!(
                "{}  {}",
                paint(style::OK, "managed"),
                paint(style::MUTED, "(rendered from settings every boot)")
            )
        } else if profile.saved {
            paint(style::OK, "yes")
        } else {
            format!(
                "{}  {}",
                paint(style::WARN, "no"),
                paint(style::MUTED, "(lost at reboot)")
            )
        },
    );
    if let Some(wifi) = &detail.wifi {
        row("ssid", &paint(style::HEADING, &wifi.ssid));
        row("security", &wifi.security);
        row("hidden", &yes_no(wifi.hidden));
    }
    for (family, settings) in [("ipv4", &detail.ipv4), ("ipv6", &detail.ipv6)] {
        row(family, &settings.method);
        if !settings.addresses.is_empty() || settings.method == "manual" {
            sub(
                "addresses",
                &if settings.addresses.is_empty() {
                    none()
                } else {
                    settings.addresses.join(", ")
                },
            );
        }
        if let Some(gateway) = &settings.gateway {
            sub("gateway", gateway);
        }
        if !settings.dns.is_empty() {
            sub("dns", &settings.dns.join(", "));
        }
        if settings.ignore_auto_dns {
            sub("ignore-auto-dns", &yes_no(true));
        }
    }
    if profile.active {
        row(
            "now",
            &if detail.addresses.is_empty() {
                paint(style::WARN, "(no address)")
            } else {
                detail
                    .addresses
                    .iter()
                    .map(address)
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
    }
}

fn address(address: &NetAddress) -> String {
    format!(
        "{}/{} {}",
        address.address,
        address.prefix,
        paint(style::MUTED, &address.scope)
    )
}

fn show_wifi(status: &WifiStatus) {
    let row = style::row;
    let radio = match (status.enabled, status.hardware_enabled) {
        (true, true) => paint(style::OK, "on"),
        (false, true) => paint(style::WARN, "off"),
        (_, false) => format!(
            "{}  {}",
            paint(style::BAD, "off"),
            paint(style::MUTED, "(hardware switch)")
        ),
    };
    row("radio", &radio);
    if status.devices.is_empty() {
        println!("{}", paint(style::MUTED, "no WiFi device"));
    }
    for device in &status.devices {
        let network = match &device.ssid {
            Some(ssid) => {
                let signal = device
                    .signal
                    .map(|signal| {
                        format!("  {}", paint(signal_style(signal), format!("{signal}%")))
                    })
                    .unwrap_or_default();
                let band = device
                    .frequency_mhz
                    .map(|mhz| format!("  {}", paint(style::MUTED, band(mhz))))
                    .unwrap_or_default();
                format!("{}{signal}{band}", paint(style::HEADING, ssid))
            }
            None => paint(style::MUTED, "(not connected)"),
        };
        let state_style = if device.state == "activated" {
            style::OK
        } else {
            style::MUTED
        };
        println!(
            "{} {} {network}",
            pad(style::HEADING, &device.interface, 12),
            pad(state_style, &device.state, 13)
        );
    }
    if let Some(ssid) = &status.fallback {
        row(
            "fallback",
            &format!(
                "{} {} did not connect after boot, so the hotspot is up until the next \
                 boot; {} tries it again now",
                paint(style::WARN, "on"),
                paint(style::HEADING, ssid),
                paint(style::CMD, "tessaro-ctl network wifi join"),
            ),
        );
    }
}

fn show_networks(networks: &[WifiNetwork]) {
    if networks.is_empty() {
        println!("{}", paint(style::MUTED, "no networks found"));
    }
    for network in networks {
        let name = if network.ssid.is_empty() {
            pad(style::MUTED, "(hidden)", 24)
        } else {
            pad(style::HEADING, &network.ssid, 24)
        };
        let state = if network.active {
            pad(style::OK, "connected", 9)
        } else if network.known {
            pad(style::MUTED, "known", 9)
        } else {
            pad(style::MUTED, "", 9)
        };
        println!(
            "{name}  {} {} {} {state} {}",
            pad(
                signal_style(network.signal),
                format!("{}%", network.signal),
                4
            ),
            pad(style::MUTED, band(network.frequency_mhz), 5),
            pad(style::LABEL, &network.security, 12),
            paint(style::MUTED, &network.bssid)
        );
    }
}

fn signal_style(signal: u8) -> anstyle::Style {
    match signal {
        60.. => style::OK,
        30..=59 => style::WARN,
        _ => style::BAD,
    }
}

fn band(mhz: u32) -> &'static str {
    match mhz {
        0..=3000 => "2.4G",
        3001..=5925 => "5G",
        _ => "6G",
    }
}

fn show_change(change: &NetChange) {
    let subject = change
        .profile
        .as_deref()
        .map(|name| format!(" {}", paint(style::HEADING, name)))
        .unwrap_or_default();
    let uuid = change
        .uuid
        .as_deref()
        .map(|uuid| format!(" {}", paint(style::MUTED, format!("({uuid})"))))
        .unwrap_or_default();
    let verdict = match change.outcome {
        ChangeOutcome::Committed => paint(style::OK, "committed"),
        ChangeOutcome::RolledBack => paint(style::BAD, "rolled back"),
    };
    println!(
        "{verdict} {}{subject}{uuid}",
        paint(style::MUTED, &change.action)
    );
    for check in &change.checks {
        let result = if check.passed {
            paint(style::OK, "passed")
        } else {
            paint(style::BAD, "failed")
        };
        println!(
            "{} {result} {}",
            pad(style::LABEL, &check.name, 12),
            check.detail
        );
    }
    if let Some(reason) = &change.reason {
        println!("{} {reason}", pad(style::LABEL, "why", 12));
    }
    if let Some(note) = &change.note {
        println!("{}", paint(style::WARN, note));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anstream::adapter::strip_str;
    use protocol::NetCheck;

    #[test]
    fn speedtest_lines_strip_to_aligned_plain_text() {
        assert_eq!(
            plain(speedtest_line(&SpeedtestEvent::Transfer {
                direction: Direction::Upload,
                size: 1_000_000,
                samples: 3,
                attempts: 4,
                median_mbit: Some(42.5),
                min_mbit: Some(40.0),
                max_mbit: None,
            })),
            "upload    1m    42.5 Mbit/s (min 40.0 Mbit/s, max n/a, 3/4 samples)"
        );
        assert_eq!(
            plain(speedtest_line(&SpeedtestEvent::Result {
                download_mbit: Some(93.14),
                upload_mbit: None,
                latency_ms: Some(12.0),
            })),
            "result    download 93.1 Mbit/s, upload n/a, latency 12.0 ms"
        );
    }

    #[test]
    fn max_size_takes_the_offered_sizes_only() {
        assert_eq!(parse_payload("25M"), Ok(25_000_000));
        assert_eq!(parse_payload("100k"), Ok(100_000));
        assert!(parse_payload("5m").is_err());
    }

    fn plain(text: String) -> String {
        strip_str(&text).to_string()
    }

    #[test]
    fn ping_lines_strip_to_plain_text() {
        assert_eq!(
            plain(ping_line(&PingEvent::Reply {
                seq: 2,
                bytes: 64,
                rtt_ms: 0.84
            })),
            "reply     0.8 ms seq=2 64 bytes"
        );
        assert_eq!(
            plain(ping_line(&PingEvent::Summary {
                sent: 4,
                received: 3,
                min_ms: Some(0.5),
                avg_ms: Some(1.0),
                max_ms: Some(2.0),
            })),
            "result    3/4 received, 25% lost (min 0.5, avg 1.0, max 2.0 ms)"
        );
        assert_eq!(
            plain(ping_line(&PingEvent::Summary {
                sent: 2,
                received: 0,
                min_ms: None,
                avg_ms: None,
                max_ms: None,
            })),
            "result    0/2 received, 100% lost"
        );
    }

    #[test]
    fn bands() {
        assert_eq!(band(2437), "2.4G");
        assert_eq!(band(5180), "5G");
        assert_eq!(band(5955), "6G");
    }

    #[test]
    fn a_verdict_reads_the_same_without_color() {
        // Exercised for the panic-free path; the text is what a script greps.
        let change = NetChange {
            outcome: ChangeOutcome::RolledBack,
            action: "set".into(),
            profile: Some("Wired connection 1".into()),
            uuid: Some("abc".into()),
            reason: Some("the device lost its default route".into()),
            checks: vec![NetCheck {
                name: "route".into(),
                passed: false,
                detail: "no default route any more".into(),
            }],
            note: None,
        };
        show_change(&change);
    }
}
