//! `tessaro-ctl net ...` beyond the kernel's view: NetworkManager's profiles,
//! WiFi, a ping from the device, and `tessaro-ctl ping` to it.
//!
//! The device manages four profiles of its own and switches between them
//! through settings (`set ethernet.mode=static ...`, `set wifi.mode=...`);
//! `net wifi join` is the one change made here, as sugar for those settings
//! plus the password. Either way it is one request: the device applies it,
//! checks on its own that it still reaches the network, and keeps it or
//! rolls it back - so this client does not have to be there for the end.
//! When the change takes this very connection away, which re-addressing the
//! link it came in on does, the answer never arrives; `net last` asks what
//! happened.

use std::io::Read;
use std::time::{Duration, Instant};

use anstream::{eprintln, println};
use clap::{Args, Subcommand, ValueEnum};
use protocol::{
    Applied, ChangeOutcome, Command, Done, HotspotCredentials, NetAddress, NetChange, NetProfile,
    NetProfileDetail, PingEvent, Secret, Verify, WifiNetwork, WifiSecurity, WifiStatus,
};
use serde_json::json;

use crate::connect::{Answer, Session};
use crate::style::{self, pad, paint, yes_no};
use crate::{call, print, show_applied, show_once};

/// Longer than the device takes to apply, check and roll back a change
/// (about 90s at most), so its answer is waited for - and short enough that
/// a connection the change silently broke does not hang the terminal.
const CHANGE: Duration = Duration::from_secs(180);

#[derive(Subcommand)]
pub enum NetCmd {
    /// Every network interface: kind, state, MAC, MTU, addresses.
    Interfaces,
    /// NetworkManager's profiles: the device's own tessaro-* four, and any
    /// made by hand - which is active where, and which come up on their own.
    Profiles,
    /// One profile, by name or uuid: addressing, DNS, WiFi - never its
    /// password - and what its device has right now.
    Show { profile: String },
    /// What the last network change did: for when the change took the
    /// connection that asked for it.
    Last,
    /// Ping HOST from the device, not from here.
    Ping {
        host: String,
        #[arg(long, short = 'c', default_value_t = protocol::PING_DEFAULT_COUNT)]
        count: u32,
        /// Seconds between echoes.
        #[arg(long, short = 'i', default_value_t = 1.0)]
        interval: f64,
        /// Seconds to wait for each reply.
        #[arg(long, short = 'W', default_value_t = 2.0)]
        timeout: f64,
        /// Send from this interface.
        #[arg(long, short = 'I')]
        interface: Option<String>,
    },
    /// The WiFi radio and what each WiFi device is connected to. Scan, join
    /// with the subcommands; `set wifi.mode=hotspot` goes back to the hotspot.
    Wifi {
        #[command(subcommand)]
        what: Option<WifiCmd>,
    },
}

#[derive(Subcommand)]
pub enum WifiCmd {
    /// The networks in range, strongest first.
    Scan {
        #[arg(long, short = 'I')]
        interface: Option<String>,
        /// What the device saw last, without scanning again.
        #[arg(long)]
        cached: bool,
    },
    /// Join SSID as a client: wifi.mode=client, and the hotspot goes down.
    /// The password is prompted, or read from stdin with --password-stdin -
    /// never taken from the command line, never shown by `get`. Rejoining
    /// the same network keeps its saved password if you give none.
    /// `set wifi.mode=hotspot` brings the hotspot back.
    ///
    ///   tessaro-ctl net wifi join Office
    ///   tessaro-ctl net wifi join Backroom --hidden --security psk
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

/// Whether `net ping` streams, which drops the read timeout.
pub fn streams(command: &NetCmd) -> bool {
    matches!(command, NetCmd::Ping { .. })
}

pub fn run(session: &mut Session, command: NetCmd, json: bool) -> Result<(), String> {
    match command {
        NetCmd::Interfaces => unreachable!("main shows the kernel's interfaces"),
        NetCmd::Profiles => {
            let profiles: Vec<NetProfile> = call(session, Command::NetProfiles)?;
            print(json, &profiles, || show_profiles(&profiles))
        }
        NetCmd::Show { profile } => {
            let detail: NetProfileDetail = call(session, Command::NetShow { profile })?;
            print(json, &detail, || show_detail(&detail))
        }
        NetCmd::Last => {
            let last: Option<NetChange> = call(session, Command::NetLast)?;
            print(json, &last, || match &last {
                Some(change) => show_change(change),
                None => println!("{}", paint(style::MUTED, "no network change yet")),
            })
        }
        NetCmd::Ping {
            host,
            count,
            interval,
            timeout,
            interface,
        } => net_ping(session, json, host, count, interval, timeout, interface),
        NetCmd::Wifi { what } => wifi(session, json, what),
    }
}

fn wifi(session: &mut Session, json: bool, what: Option<WifiCmd>) -> Result<(), String> {
    match what {
        None => {
            let status: WifiStatus = call(session, Command::Wifi)?;
            print(json, &status, || show_wifi(&status))
        }
        Some(WifiCmd::Scan { interface, cached }) => {
            if !json && !cached {
                eprintln!(
                    "{}",
                    paint(style::MUTED, format!("{}: scanning...", session.node.name))
                );
            }
            let networks: Vec<WifiNetwork> = call(
                session,
                Command::WifiScan {
                    interface,
                    rescan: !cached,
                },
            )?;
            print(json, &networks, || show_networks(&networks))
        }
        Some(WifiCmd::Join {
            ssid,
            password_stdin,
            hidden,
            security,
            verify,
        }) => {
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
        Some(WifiCmd::HotspotPassword) => {
            let hotspot: HotspotCredentials = call(session, Command::HotspotPassword)?;
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
            let networks: Vec<WifiNetwork> = call(
                session,
                Command::WifiScan {
                    interface: None,
                    rescan: false,
                },
            )
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
    let psk = password(from_stdin, &prompt)?;
    if psk.is_empty() && known {
        return Ok(None);
    }
    protocol::keys::check_psk(&psk)?;
    Ok(Some(psk))
}

fn password(from_stdin: bool, prompt: &str) -> Result<String, String> {
    if from_stdin {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|err| format!("reading the password: {err}"))?;
        return Ok(text.trim_end_matches(['\n', '\r']).to_string());
    }
    rpassword::prompt_password(prompt).map_err(|err| err.to_string())
}

/// Whether `key` is one of the device's network settings, whose change runs
/// as a verified network transaction.
pub fn is_network_key(key: &str) -> bool {
    protocol::keys::find(key)
        .is_some_and(|key| key.consumers.contains(&protocol::keys::Consumer::Network))
}

/// Send one change of network settings - a `set`, an `unset`, a join - and
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
    let answer = session.request(command);
    session.set_read_timeout(Some(Duration::from_secs(60)));

    let value = match answer {
        Answer::Ok(value) => value,
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
                    "tessaro-ctl net last   (at the new address, if it changed)"
                )
            );
            return Err("no answer from the device".to_string());
        }
    };
    let applied: Applied =
        serde_json::from_value(value).map_err(|err| format!("unexpected answer: {err}"))?;
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
    session.stream(
        Command::NetPing {
            host,
            count: Some(count),
            interval_ms: Some(millis(interval)),
            timeout_ms: Some(millis(timeout)),
            interface,
        },
        |event| {
            if json {
                println!("{event}");
                return;
            }
            match serde_json::from_value::<PingEvent>(event.clone()) {
                Ok(step) => {
                    if let PingEvent::Summary { received: 0, .. } = step {
                        failed = true;
                    }
                    println!("{}", ping_line(&step));
                }
                Err(_) => println!("{event}"),
            }
        },
    )?;
    if failed {
        return Err("no replies".to_string());
    }
    Ok(())
}

/// `tessaro-ctl ping`: the path this client really uses - the TCP connect,
/// the TLS handshake with the hello, then round trips on the session.
pub fn ping(session: &mut Session, json: bool, count: u32, interval: f64) -> Result<(), String> {
    if count == 0 {
        return Err("--count must be at least 1".to_string());
    }
    let timing = session.timing;
    let label = |text: &str| pad(style::LABEL, text, 9);
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
        match session.call(Command::Ping) {
            Ok(value) => {
                let rtt = started.elapsed();
                let _: Done = serde_json::from_value(value)
                    .map_err(|err| format!("unexpected answer: {err}"))?;
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
        return print(
            true,
            &json!({
                "node": session.node.name,
                "connect_ms": timing.map(|t| seconds(&t.connect)),
                "handshake_ms": timing.map(|t| seconds(&t.handshake)),
                "sent": count,
                "received": rtts.len(),
                "rtt_ms": rtts.iter().map(seconds).collect::<Vec<_>>(),
                "min_ms": min,
                "avg_ms": avg,
                "max_ms": max,
            }),
            || {},
        );
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
    let label = |text: &str| pad(style::LABEL, text, 9);
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
        println!("{}", paint(style::MUTED, "no saved profiles"));
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
        let saved = if profile.saved {
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
    let row = |label: &str, value: &str| println!("{} {value}", pad(style::LABEL, label, 12));
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
        &if profile.saved {
            paint(style::OK, "yes")
        } else {
            format!(
                "{}  {}",
                paint(style::WARN, "no"),
                paint(style::MUTED, "(in memory until something changes it)")
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
    let row = |label: &str, value: &str| println!("{} {value}", pad(style::LABEL, label, 12));
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
