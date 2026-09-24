//! tessaro-ctl: the one way to manage a Tessaro kiosk.
//!
//! On the device, as root, it talks to the agent over the local socket and
//! needs nothing else. From anywhere else, `--node` (or `TESSARO_NODE`) names
//! the device by IP, `name.local` or plain name, and the conversation is TLS
//! with a pinned certificate and a token.
//!
//! A fresh device is unclaimed: `tessaro-ctl --node NAME claim` takes it,
//! stores the token in ~/.config/tessaro/nodes.json, and prints the device's
//! new root password - once.

mod connect;
mod net;
mod nodes;
mod ssh;
mod style;
mod update;

use std::collections::BTreeMap;
use std::process::ExitCode;

// Shadow the std macros: these strip colors when stdout is not a terminal.
use anstream::{eprint, eprintln, println};
use clap::builder::styling::Styles;
use clap::{ColorChoice, Parser, Subcommand, ValueEnum};
use protocol::speedtest_size_label as size_label;
use protocol::{
    Applied, Claimed, Command, Connector, Direction, Done, KeyInfo, Net, NetInterface, NodeInfo,
    Password, Screenshot, Settings, Source, SpeedtestEvent, SshKeyInfo, Status,
    Target as RestartTarget, TokenCreated, TokenInfo,
};
use serde_json::Value;

use connect::{Session, Target, Trust};
use nodes::{Node, Nodes};
use style::{pad, paint};

/// `--help` in the same palette as everything else.
const HELP_STYLES: Styles = Styles::styled()
    .header(style::HEADING.underline())
    .usage(style::HEADING.underline())
    .literal(style::SOURCE)
    .error(style::BAD)
    .valid(style::OK)
    .invalid(style::WARN);

#[derive(Parser)]
#[command(
    name = "tessaro-ctl",
    version,
    styles = HELP_STYLES,
    about = "Manage Tessaro kiosks",
    long_about = "Manage Tessaro kiosks: settings, the browser, the display, tokens and the root password.\n\n\
        On the device, as root, it talks to the agent over the local socket and needs nothing else. \
        From anywhere else, --node names the device and the conversation is TLS with a pinned \
        certificate and a token, which `claim` or `login` stores in ~/.config/tessaro/nodes.json.\n\n\
        `tessaro-ctl keys` documents every setting: what it accepts, its default, what is set, and \
        what a change restarts.",
    after_long_help = "EXAMPLES:\n\
        \x20 tessaro-ctl nodes                              devices answering on this network\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 claim          take a fresh device; prints its root password once\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 status\n\
        \x20 tessaro-ctl keys                               every setting, documented\n\
        \x20 tessaro-ctl keys display.resolution            one setting in full\n\
        \x20 tessaro-ctl set kiosk.url=https://shop.test/\n\
        \x20 tessaro-ctl set 'kiosk.url=https://menu.test/?table={data.table}' data.table=12\n\
        \x20 tessaro-ctl set 'kiosk.url=https://{node.name}.menu.test/'  any setting is a placeholder too\n\
        \x20 tessaro-ctl modes && tessaro-ctl set display.resolution=1920x1080 && tessaro-ctl confirm\n\
        \x20 tessaro-ctl net                                address, gateway, DNS, interfaces\n\
        \x20 tessaro-ctl net interfaces                     every interface in detail\n\
        \x20 tessaro-ctl net profiles                       NetworkManager's profiles\n\
        \x20 tessaro-ctl set ethernet.mode=static ethernet.address=192.168.1.50/24 ethernet.gateway=192.168.1.1\n\
        \x20                                                kept only if the gateway still answers\n\
        \x20 tessaro-ctl set ethernet.mode=dhcp\n\
        \x20 tessaro-ctl net wifi scan && tessaro-ctl net wifi join Office   the hotspot goes down\n\
        \x20 tessaro-ctl set wifi.mode=hotspot              back to the hotspot, tessaro-NAME\n\
        \x20 tessaro-ctl set wifi.nat=0                     hotspot clients reach the device only\n\
        \x20 tessaro-ctl net last                           what the last change did, if the answer never came\n\
        \x20 tessaro-ctl net ping 192.168.1.1               from the device\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 ping           from here to the device\n\
        \x20 tessaro-ctl get net.ip                         one read-only value\n\
        \x20 tessaro-ctl set 'kiosk.url=https://menu.test/?ip={net.ip}'  read-only keys are placeholders too\n\
        \x20 tessaro-ctl set browser.fps_counter=on\n\
        \x20 tessaro-ctl maintenance on                     show the maintenance page; `off` goes back\n\
        \x20 tessaro-ctl debug on                           name and addresses full screen; `off` goes back\n\
        \x20 tessaro-ctl unset kiosk.url                    back to the image default\n\
        \x20 tessaro-ctl logs -f -u tessaro-agent.service\n\
        \x20 tessaro-ctl update send tessaro-os-qemux86-64.rootfs.wic.bz2   a new image; settings are kept\n\
        \x20 tessaro-ctl token create phone                 a token for a second client\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 ssh            a root shell, by your ~/.ssh key\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 ssh -- journalctl -fu tessaro-agent\n\
        \x20 tessaro-ctl ssh-key list                       keys that can log in as root\n\
        \x20 tessaro-ctl ssh-key revoke user@laptop         by comment or fingerprint\n\n\
        ENVIRONMENT:\n\
        \x20 TESSARO_NODE        default for --node\n\
        \x20 TESSARO_TOKEN       use this token instead of the stored one\n\
        \x20 TESSARO_CONFIG_DIR  where nodes.json lives (default ~/.config/tessaro)\n\
        \x20 TESSARO_SOCKET      the local socket (default /run/tessaro-agent.sock)"
)]
struct Cli {
    /// The device: IP, ip:port, NAME, NAME.local, a host name, or `local`.
    /// Without it, the local socket on the device itself.
    #[arg(long, short = 'n', global = true, env = "TESSARO_NODE")]
    node: Option<String>,

    /// Print the raw JSON the device answered.
    #[arg(long, global = true)]
    json: bool,

    /// Color the output: auto (only on a terminal, and not under NO_COLOR),
    /// always, or never.
    #[arg(long, global = true, value_name = "WHEN", default_value_t = ColorChoice::Auto)]
    color: ColorChoice,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// What the device is doing right now.
    Status,
    /// Who the device is: node id, name, TLS fingerprint, claim state.
    Id,
    /// Every setting, documented: accepted values, default, current value,
    /// and what a change restarts. With KEY, just that one.
    Keys {
        key: Option<String>,
    },
    /// The resolutions the connected displays offer (for display.resolution).
    Modes,
    /// The network as the device sees it: address, gateway, DNS, and every
    /// interface - the same values as the net.* keys. The subcommands read
    /// and change NetworkManager's profiles and WiFi; the device keeps a
    /// change only if it still reaches the network afterwards.
    Net {
        #[command(subcommand)]
        what: Option<net::NetCmd>,
    },
    /// How fast the device answers this client: the TCP connect, the TLS
    /// handshake, then round trips over the control connection. Needs no
    /// token, like `id`.
    Ping {
        #[arg(long, short = 'c', default_value_t = 4)]
        count: u32,
        /// Seconds between round trips.
        #[arg(long, short = 'i', default_value_t = 1.0)]
        interval: f64,
    },
    /// Current settings, or one of them.
    Get {
        key: Option<String>,
    },
    /// Change settings: KEY=VALUE ... Restarts only what reads them.
    ///
    /// `tessaro-ctl keys` lists every KEY with what it accepts. Several pairs
    /// are applied as one change. `data.NAME=VALUE` defines a custom value,
    /// with a NAME you choose. Any setting's full key in braces is a
    /// placeholder in kiosk.url, filled percent-encoded: `{data.NAME}`,
    /// `{node.name}`, `{display.osk}`, ...
    ///
    ///   tessaro-ctl set 'kiosk.url=https://menu.test/?table={data.table}&screen={data.screen}' data.table=12 data.screen=entrance
    ///
    /// The ethernet.* and wifi.* keys switch the device's own network
    /// profiles. That change is applied and checked by the device before it
    /// is saved at all - see --verify - and rolled back by the device alone
    /// if it cuts it off:
    ///
    ///   tessaro-ctl set ethernet.mode=static ethernet.address=192.168.1.50/24 ethernet.gateway=192.168.1.1 ethernet.dns=192.168.1.1
    ///   tessaro-ctl set ethernet.mode=dhcp
    ///   tessaro-ctl set wifi.mode=hotspot
    Set {
        #[arg(required = true, value_name = "KEY=VALUE")]
        pairs: Vec<String>,
        /// Refuse unless the settings are still at this revision.
        #[arg(long)]
        if_revision: Option<u64>,
        /// Save and render, but restart nothing yet. Not for network keys,
        /// which are always applied and checked at once.
        #[arg(long)]
        no_apply: bool,
        #[command(flatten)]
        verify: net::VerifyArg,
    },
    /// Go back to the image default for KEY ...
    Unset {
        #[arg(required = true)]
        keys: Vec<String>,
        #[arg(long)]
        if_revision: Option<u64>,
        #[arg(long)]
        no_apply: bool,
        #[command(flatten)]
        verify: net::VerifyArg,
    },
    /// Keep a change that is on probation (display.resolution).
    Confirm,
    /// Maintenance mode: show maintenance.url instead of kiosk.url, which is
    /// left as it is. The same as `set maintenance.enable=1|0`.
    ///
    ///   tessaro-ctl maintenance on --url 'http://127.0.0.1/maintenance.html?message=Back%20at%2014:00'
    Maintenance {
        state: Toggle,
        /// With `on`: set maintenance.url in the same change.
        #[arg(long)]
        url: Option<String>,
    },
    /// The debug screen: debug.template in large text over the whole screen,
    /// instead of the kiosk or maintenance page. The same as
    /// `set debug.enable=1|0`. `\n` breaks a line.
    ///
    ///   tessaro-ctl debug on --template 'IP {net.ip}\nGW {net.gateway}'
    Debug {
        state: Toggle,
        /// With `on`: set debug.template in the same change.
        #[arg(long)]
        template: Option<String>,
    },
    /// Point the browser at a URL until the next refresh.
    Navigate {
        url: String,
    },
    /// Restart the browser, the display (Weston, with the browser and agent)
    /// or the agent.
    Restart {
        what: What,
    },
    Reboot,
    /// Save what the browser is rendering as a JPEG.
    Screenshot {
        #[arg(long, short)]
        output: Option<String>,
    },
    /// The device's journal.
    Logs {
        #[arg(long, short)]
        follow: bool,
        #[arg(long, short)]
        unit: Option<String>,
        /// How many lines back to start (`-n` is --node).
        #[arg(long, default_value_t = 100)]
        lines: u32,
    },
    /// Measure the device's internet connection against speed.cloudflare.com:
    /// latency, then download and upload at growing payload sizes.
    ///
    /// Runs on the device, so it measures the kiosk's link, not this one.
    /// A full run moves a few hundred MB - mind a metered connection, and
    /// use a smaller --max-size there.
    ///
    ///   tessaro-ctl speedtest --max-size 1m --tests 3
    Speedtest {
        /// Largest payload: 100k, 1m, 10m, 25m or 100m. Uploads stop at 25m.
        #[arg(long, default_value = "25m", value_parser = parse_payload)]
        max_size: u64,
        /// Samples per payload size.
        #[arg(long, default_value_t = protocol::SPEEDTEST_DEFAULT_TESTS)]
        tests: u32,
    },
    /// Take an unclaimed device: get a token and its new root password.
    Claim {
        /// What to call this client in `token list`.
        #[arg(long)]
        name: Option<String>,
        /// Pin the certificate without asking.
        #[arg(long, short)]
        yes: bool,
    },
    /// Remember a device with a token someone issued for you.
    Login {
        #[arg(long)]
        token: String,
        #[arg(long, short)]
        yes: bool,
    },
    /// Forget a device: its pin and token on this machine.
    Forget {
        node: String,
    },
    /// Tokens on the device.
    Token {
        #[command(subcommand)]
        command: TokenCmd,
    },
    /// The device's root password.
    Password {
        #[command(subcommand)]
        command: PasswordCmd,
    },
    /// A root shell on the device. Sends your SSH public key over this
    /// pinned connection, adds it to root's authorized_keys, then runs ssh
    /// with the host key the device reported - no password, no first-use
    /// prompt. Anything after `--` goes to ssh: options or a command.
    Ssh {
        /// The key to send: a .pub file, or a private key with its .pub
        /// next to it. Default: the first of ~/.ssh/id_ed25519.pub,
        /// id_ecdsa.pub, id_ecdsa_sk.pub, id_ed25519_sk.pub, id_rsa.pub.
        #[arg(long, short = 'i', value_name = "PATH")]
        key: Option<std::path::PathBuf>,
        /// The device's SSH port.
        #[arg(long, default_value_t = 22)]
        port: u16,
        /// Send the key and print the ssh command instead of running it.
        #[arg(long)]
        print: bool,
        #[arg(last = true, value_name = "SSH_ARGS")]
        args: Vec<String>,
    },
    /// The SSH keys that can log in as root. Unclaiming or a factory reset
    /// removes them all.
    SshKey {
        #[command(subcommand)]
        command: SshKeyCmd,
    },
    /// Release the device: every token and ssh key removed, root password
    /// emptied.
    Unclaim {
        #[arg(long, short)]
        yes: bool,
    },
    /// Defaults, unclaimed, empty root password - the fresh-install state.
    FactoryReset {
        #[arg(long, short)]
        yes: bool,
    },
    /// Put a new image on the device, keeping its settings and claim.
    Update {
        #[command(subcommand)]
        command: UpdateCmd,
    },
    /// Devices answering on the local network.
    Nodes {
        /// Seconds to listen.
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
}

#[derive(Subcommand)]
enum TokenCmd {
    /// Issue a token for another client. Shown once.
    Create {
        name: String,
    },
    List,
    /// Revoke a token. Revoking the last one unclaims the device.
    Revoke {
        id: String,
    },
}

#[derive(Subcommand)]
enum SshKeyCmd {
    /// Every key in root's authorized_keys: fingerprint, type, comment.
    List,
    /// Remove one: its SHA256 fingerprint, a unique prefix of it, or its
    /// exact comment.
    Revoke { key: String },
}

#[derive(Subcommand)]
enum UpdateCmd {
    /// Upload IMAGE, a .wic.bz2 with its .wic.bmap next to it, and reboot
    /// the device into it. Only the blocks the bmap lists are written, and
    /// only to the root partition, plus the kernel file; /data is kept.
    /// Run it again after a dropped connection and it resumes.
    ///
    ///   tessaro-ctl -n brave-otter-3fa2 update send tessaro-os-qemux86-64.rootfs.wic.bz2
    Send {
        image: std::path::PathBuf,
        /// The block map, if it is not IMAGE without .bz2 plus .bmap.
        #[arg(long)]
        bmap: Option<std::path::PathBuf>,
        /// Also re-create /data: every setting, the claim, the browser
        /// profile and the device's identity go. It comes back unclaimed.
        #[arg(long)]
        wipe_data: bool,
        /// Write the whole disk - partition table, boot, root and /data - as
        /// `mise run image:flash` would, for a device on another disk layout.
        /// Implies --wipe-data. The device holds the upload in RAM while it
        /// writes, and a power cut before it is done needs a physical
        /// reflash.
        #[arg(long)]
        repartition: bool,
        /// Stage and commit it, but leave the reboot for later.
        #[arg(long)]
        no_reboot: bool,
        /// Do not wait for the device to come back.
        #[arg(long)]
        no_wait: bool,
        /// Skip the device's check of the whole upload against its SHA-256
        /// before preparing it. The bmap's checksums still cover every block
        /// that is written. Needs a device on an image that knows the flag;
        /// an older one checks anyway.
        #[arg(long)]
        no_verify: bool,
        #[arg(long, short)]
        yes: bool,
    },
    /// What is under way, and what the last update did.
    Status,
    /// Drop the upload, or the staged update before it is applied.
    Cancel,
}

#[derive(Subcommand)]
enum PasswordCmd {
    /// Set it: prompted, or generated with --random and shown once.
    Set {
        #[arg(long)]
        random: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Toggle {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum What {
    Browser,
    Weston,
    Agent,
}

/// Rust starts with SIGPIPE ignored, so `tessaro-ctl keys | head` panics on
/// the first write after `head` exits. A command-line tool should just stop,
/// as every other one in the pipe does. SIGPIPE is 13 and SIG_DFL is 0 on
/// both Linux and macOS.
#[cfg(unix)]
fn default_sigpipe() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    // SAFETY: restoring the default disposition of one signal, before any
    // other thread exists.
    unsafe {
        signal(13, 0);
    }
}

fn main() -> ExitCode {
    #[cfg(unix)]
    default_sigpipe();

    let cli = Cli::parse();
    match cli.color {
        ColorChoice::Auto => {}
        ColorChoice::Always => anstream::ColorChoice::Always.write_global(),
        ColorChoice::Never => anstream::ColorChoice::Never.write_global(),
    }
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{} {err}", paint(style::BAD, "tessaro-ctl:"));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let mut nodes = Nodes::load()?;

    // The two that do not need a device conversation at all.
    match &cli.command {
        Cmd::Nodes { wait } => return list_nodes(&nodes, *wait, cli.json),
        Cmd::Forget { node } => return forget(&mut nodes, node),
        _ => {}
    }

    let target = connect::resolve(cli.node.as_deref(), &nodes)?;
    let local = matches!(target, Target::Local(_));
    let trust = match &cli.command {
        Cmd::Id | Cmd::Ping { .. } => Trust::Peek,
        Cmd::Claim { yes, .. } | Cmd::Login { yes, .. } => Trust::Pin { assume_yes: *yes },
        _ => Trust::KnownOnly,
    };
    // No read timeout: a followed log is open-ended, and a speed test on a
    // slow link can go quiet for longer than one. The device bounds that.
    let follow = match &cli.command {
        Cmd::Logs { follow: true, .. } | Cmd::Speedtest { .. } => true,
        Cmd::Net { what: Some(what) } => net::streams(what),
        _ => false,
    };
    let mut session = connect::open(&target, &nodes, trust, follow)?;
    refresh_address(&mut nodes, &session)?;
    let json = cli.json;

    match cli.command {
        Cmd::Status => {
            let status: Status = call(&mut session, Command::Status)?;
            print(json, &status, || show_status(&status))
        }
        Cmd::Id => {
            let node: NodeInfo = call(&mut session, Command::Id)?;
            print(json, &node, || show_node(&node))
        }
        Cmd::Keys { key } => {
            let mut keys: Vec<KeyInfo> = call(&mut session, Command::Keys)?;
            if let Some(wanted) = &key {
                let template = wanted.starts_with(protocol::keys::DATA_PREFIX);
                keys.retain(|k| &k.name == wanted || (template && k.name == "data.<name>"));
                if keys.len() > 1 {
                    keys.retain(|k| &k.name == wanted);
                }
                if keys.is_empty() {
                    return Err(format!(
                        "{wanted} is not a setting; `tessaro-ctl keys` lists them"
                    ));
                }
            }
            print(json, &keys, || {
                for (at, info) in keys.iter().enumerate() {
                    if at > 0 {
                        println!();
                    }
                    show_key(info);
                }
            })
        }
        Cmd::Net {
            what: what @ (None | Some(net::NetCmd::Interfaces)),
        } => {
            let net: Net = call(&mut session, Command::Net)?;
            match what {
                None => print(json, &net, || show_net(&net)),
                _ => print(json, &net.interfaces, || {
                    for (at, interface) in net.interfaces.iter().enumerate() {
                        if at > 0 {
                            println!();
                        }
                        show_interface(interface);
                    }
                }),
            }
        }
        Cmd::Net { what: Some(what) } => net::run(&mut session, what, json),
        Cmd::Ping { count, interval } => net::ping(&mut session, json, count, interval),
        Cmd::Modes => {
            let connectors: Vec<Connector> = call(&mut session, Command::Modes)?;
            print(json, &connectors, || {
                if connectors.is_empty() {
                    println!(
                        "{}",
                        paint(style::WARN, "no connected display reports its modes")
                    );
                }
                for connector in &connectors {
                    println!("{}", paint(style::HEADING, format!("{}:", connector.name)));
                    for (at, mode) in connector.modes.iter().enumerate() {
                        let note = if at == 0 {
                            paint(style::MUTED, "  (preferred)")
                        } else {
                            String::new()
                        };
                        println!("  {mode}{note}");
                    }
                }
                println!(
                    "\nset one with: {}",
                    paint(
                        style::CMD,
                        "tessaro-ctl set display.resolution=WIDTHxHEIGHT"
                    )
                );
            })
        }
        Cmd::Get { key } => {
            let settings: Settings = call(&mut session, Command::Get { key })?;
            print(json, &settings, || {
                for setting in &settings.settings {
                    let value = setting.value.as_deref().unwrap_or("");
                    let source = match setting.source {
                        Source::Set => String::new(),
                        Source::Default => paint(style::MUTED, "  (default)"),
                        Source::Live => paint(style::MUTED, "  (read-only)"),
                    };
                    println!(
                        "{} {} {value}{source}",
                        paint(style::HEADING, &setting.key),
                        paint(style::LABEL, "=")
                    );
                }
                println!(
                    "{}",
                    paint(style::MUTED, format!("# revision {}", settings.revision))
                );
            })
        }
        Cmd::Set {
            pairs,
            if_revision,
            no_apply,
            verify,
        } => {
            let mut values = BTreeMap::new();
            for pair in pairs {
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("{pair}: expected KEY=VALUE"))?;
                values.insert(key.to_string(), value.to_string());
            }
            let network = values.keys().any(|key| net::is_network_key(key));
            let command = Command::Set {
                values,
                if_revision,
                apply: !no_apply,
                verify: verify.verify.clone(),
            };
            if network && !no_apply {
                return net::apply(
                    &mut session,
                    json,
                    "changing the network",
                    &verify.verify,
                    command,
                );
            }
            let applied: Applied = call(&mut session, command)?;
            print(json, &applied, || show_applied(&applied, no_apply))
        }
        Cmd::Unset {
            keys,
            if_revision,
            no_apply,
            verify,
        } => {
            let network = keys.iter().any(|key| net::is_network_key(key));
            let command = Command::Unset {
                keys,
                if_revision,
                apply: !no_apply,
                verify: verify.verify.clone(),
            };
            if network && !no_apply {
                return net::apply(
                    &mut session,
                    json,
                    "changing the network",
                    &verify.verify,
                    command,
                );
            }
            let applied: Applied = call(&mut session, command)?;
            print(json, &applied, || show_applied(&applied, no_apply))
        }
        Cmd::Confirm => done(&mut session, Command::Confirm, json),
        Cmd::Maintenance { state, url } => toggle(
            &mut session,
            json,
            &Mode {
                name: "maintenance",
                flag: "maintenance.enable",
                with: ("--url", "maintenance.url"),
                shows: "maintenance.url",
                back: "kiosk.url",
                command: "maintenance",
            },
            state,
            url,
        ),
        Cmd::Debug { state, template } => toggle(
            &mut session,
            json,
            &Mode {
                name: "debug screen",
                flag: "debug.enable",
                with: ("--template", "debug.template"),
                shows: "debug.template",
                back: "the kiosk page (or the maintenance page)",
                command: "debug",
            },
            state,
            template,
        ),
        Cmd::Navigate { url } => done(&mut session, Command::Navigate { url }, json),
        Cmd::Restart { what } => {
            let what = match what {
                What::Browser => RestartTarget::Browser,
                What::Weston => RestartTarget::Weston,
                What::Agent => RestartTarget::Agent,
            };
            done(&mut session, Command::Restart { what }, json)
        }
        Cmd::Reboot => done(&mut session, Command::Reboot, json),
        Cmd::Screenshot { output } => {
            let shot: Screenshot = call(&mut session, Command::Screenshot)?;
            let bytes = data_encoding::BASE64
                .decode(shot.data.as_bytes())
                .map_err(|err| format!("the image is not base64: {err}"))?;
            let path = output.unwrap_or_else(|| format!("{}.jpg", session.node.name));
            std::fs::write(&path, &bytes).map_err(|err| format!("{path}: {err}"))?;
            println!(
                "{} {}",
                paint(style::OK, &path),
                paint(style::MUTED, format!("({} bytes)", bytes.len()))
            );
            Ok(())
        }
        Cmd::Logs {
            follow,
            unit,
            lines,
        } => session.stream(
            Command::Logs {
                follow,
                unit,
                lines: Some(lines),
            },
            |event| {
                if json {
                    println!("{event}");
                } else {
                    println!("{}", journal_line(&event));
                }
            },
        ),
        Cmd::Speedtest { max_size, tests } => {
            if !json {
                eprintln!(
                    "{}",
                    paint(
                        style::MUTED,
                        format!(
                            "{}: measuring against speed.cloudflare.com, up to {} per sample...",
                            session.node.name,
                            size_label(max_size)
                        )
                    )
                );
            }
            session.stream(
                Command::Speedtest {
                    max_size: Some(max_size),
                    tests: Some(tests),
                },
                |event| {
                    if json {
                        println!("{event}");
                    } else {
                        match serde_json::from_value::<SpeedtestEvent>(event.clone()) {
                            Ok(step) => println!("{}", speedtest_line(&step)),
                            Err(_) => println!("{event}"),
                        }
                    }
                },
            )
        }
        Cmd::Claim { name, .. } => {
            let name = name.unwrap_or_else(default_client_name);
            // A token left over from before an unclaim means nothing now.
            session.clear_token();
            let claimed: Claimed = call(&mut session, Command::Claim { name })?;
            remember(&mut nodes, &session, Some(claimed.token.clone()), local)?;
            if json {
                return print(true, &claimed, || {});
            }
            println!(
                "{} {} {}",
                paint(style::OK, "claimed"),
                paint(style::HEADING, &session.node.name),
                paint(style::MUTED, format!("({})", session.node.id))
            );
            println!(
                "token {} saved in {}",
                claimed.token_id,
                nodes::dir().join("nodes.json").display()
            );
            println!();
            show_once(
                "root password - shown this once, store it now:",
                &claimed.root_password,
            );
            if let Some(hotspot) = &claimed.hotspot {
                show_once(
                    &format!(
                        "hotspot {} password - shown this once; anyone on the hotspot now is dropped:",
                        hotspot.ssid
                    ),
                    &hotspot.password,
                );
            }
            Ok(())
        }
        Cmd::Login { token, .. } => {
            session.set_token(token.clone());
            // Prove the token before storing it.
            let _: Vec<TokenInfo> = call(&mut session, Command::TokenList)?;
            remember(&mut nodes, &session, Some(token), local)?;
            println!(
                "{} {} {}",
                paint(style::OK, "logged in to"),
                paint(style::HEADING, &session.node.name),
                paint(style::MUTED, format!("({})", session.node.id))
            );
            Ok(())
        }
        Cmd::Forget { .. } | Cmd::Nodes { .. } => unreachable!("handled above"),
        Cmd::Token { command } => match command {
            TokenCmd::Create { name } => {
                let created: TokenCreated = call(&mut session, Command::TokenCreate { name })?;
                print(json, &created, || {
                    show_once(
                        &format!("token {} - shown this once:", created.id),
                        &created.token,
                    );
                    println!(
                        "use it with: {}",
                        paint(
                            style::CMD,
                            format!(
                                "tessaro-ctl --node {} login --token <token>",
                                session.node.name
                            )
                        )
                    );
                })
            }
            TokenCmd::List => {
                let tokens: Vec<TokenInfo> = call(&mut session, Command::TokenList)?;
                print(json, &tokens, || {
                    for token in &tokens {
                        println!(
                            "{}  {} {} {}",
                            paint(style::MUTED, &token.id),
                            pad(style::HEADING, &token.name, 24),
                            paint(style::LABEL, "issued by"),
                            token.issued_by
                        );
                    }
                })
            }
            TokenCmd::Revoke { id } => done(&mut session, Command::TokenRevoke { id }, json),
        },
        Cmd::Password { command } => match command {
            PasswordCmd::Set { random } => {
                let password = if random {
                    None
                } else {
                    let first = rpassword::prompt_password("new root password: ")
                        .map_err(|err| err.to_string())?;
                    let again =
                        rpassword::prompt_password("again: ").map_err(|err| err.to_string())?;
                    if first != again {
                        return Err("the two do not match".to_string());
                    }
                    protocol::check_password(&first)?;
                    Some(first)
                };
                let set: Password = call(&mut session, Command::PasswordSet { password })?;
                print(json, &set, || match &set.password {
                    Some(password) => {
                        show_once("root password - shown this once, store it now:", password)
                    }
                    None => println!("{}", paint(style::OK, "root password changed")),
                })
            }
        },
        Cmd::Ssh {
            key,
            port,
            print,
            args,
        } => ssh::run(
            &mut session,
            ssh::Options {
                key,
                port,
                print,
                args,
            },
            json,
        ),
        Cmd::SshKey { command } => match command {
            SshKeyCmd::List => {
                let keys: Vec<SshKeyInfo> = call(&mut session, Command::SshKeyList)?;
                print(json, &keys, || {
                    if keys.is_empty() {
                        println!("{}", paint(style::MUTED, "no ssh keys"));
                    }
                    for key in &keys {
                        let comment = if key.comment.is_empty() {
                            paint(style::MUTED, "(no comment)")
                        } else {
                            paint(style::HEADING, &key.comment)
                        };
                        println!(
                            "{}  {} {comment}",
                            paint(style::MUTED, &key.fingerprint),
                            pad(style::LABEL, &key.kind, 12)
                        );
                    }
                })
            }
            SshKeyCmd::Revoke { key } => {
                let done: Done = call(&mut session, Command::SshKeyRevoke { key })?;
                print(json, &done, || {
                    match done.message.strip_prefix("revoked ") {
                        Some(fingerprint) => println!(
                            "{} {}",
                            paint(style::OK, "revoked"),
                            paint(style::MUTED, fingerprint)
                        ),
                        None => println!("{}", done.message),
                    }
                })
            }
        },
        Cmd::Unclaim { yes } => {
            confirm_destructive(
                &session,
                yes,
                "remove every token and ssh key and empty the root password",
            )?;
            done(&mut session, Command::Unclaim, json)?;
            forget_session(&mut nodes, &session)
        }
        Cmd::FactoryReset { yes } => {
            confirm_destructive(
                &session,
                yes,
                "erase every setting, remove every token and ssh key and empty the root password",
            )?;
            done(&mut session, Command::FactoryReset, json)?;
            forget_session(&mut nodes, &session)
        }
        Cmd::Update { command } => match command {
            UpdateCmd::Send {
                image,
                bmap,
                wipe_data,
                repartition,
                no_reboot,
                no_wait,
                no_verify,
                yes,
            } => {
                let options = update::Send {
                    image,
                    bmap,
                    wipe_data: wipe_data || repartition,
                    repartition,
                    no_reboot,
                    no_wait,
                    no_verify,
                    yes,
                };
                match update::send(&mut session, &target, &nodes, options, json)? {
                    update::Sent::Kept => Ok(()),
                    update::Sent::Wiped => forget_session(&mut nodes, &session),
                }
            }
            UpdateCmd::Status => update::status(&mut session, json),
            UpdateCmd::Cancel => update::cancel(&mut session, json),
        },
    }
}

fn call<T: serde::de::DeserializeOwned>(
    session: &mut Session,
    command: Command,
) -> Result<T, String> {
    let value = session.call(command)?;
    serde_json::from_value(value).map_err(|err| format!("unexpected answer: {err}"))
}

/// A screen mode `on|off` switches: its flag, and the one setting `on` may
/// change in the same step.
struct Mode {
    name: &'static str,
    flag: &'static str,
    /// The option and the key it sets.
    with: (&'static str, &'static str),
    /// What the screen shows while the mode is on, and after `off`.
    shows: &'static str,
    back: &'static str,
    /// The subcommand, for the one usage error.
    command: &'static str,
}

/// `maintenance on|off` and `debug on|off`: one change, applied, said in one
/// line.
fn toggle(
    session: &mut Session,
    json: bool,
    mode: &Mode,
    state: Toggle,
    value: Option<String>,
) -> Result<(), String> {
    let (option, key) = mode.with;
    if value.is_some() && state == Toggle::Off {
        return Err(format!("{option} goes with `{} on`", mode.command));
    }
    let mut values = BTreeMap::new();
    let flag = if state == Toggle::On { "1" } else { "0" };
    values.insert(mode.flag.to_string(), flag.to_string());
    if let Some(value) = value {
        values.insert(key.to_string(), value);
    }
    let applied: Applied = call(
        session,
        Command::Set {
            values,
            if_revision: None,
            apply: true,
            verify: Default::default(),
        },
    )?;
    print(json, &applied, || {
        let name = mode.name;
        let (label, rest) = match (state, applied.changed.is_empty()) {
            (Toggle::On, false) => (
                paint(style::WARN, format!("{name} on")),
                format!("- the screen shows {}", mode.shows),
            ),
            (Toggle::On, true) => (
                paint(style::WARN, format!("{name} was already on")),
                String::new(),
            ),
            (Toggle::Off, false) => (
                paint(style::OK, format!("{name} off")),
                format!("- back on {}", mode.back),
            ),
            (Toggle::Off, true) => (
                paint(style::OK, format!("{name} was already off")),
                String::new(),
            ),
        };
        println!("{label} {}", paint(style::MUTED, &rest));
        show_applied(&applied, false)
    })
}

fn done(session: &mut Session, command: Command, json: bool) -> Result<(), String> {
    let done: Done = call(session, command)?;
    print(json, &done, || println!("{}", done.message))
}

/// A secret the device will never show again: the intro, then the secret
/// set off by blank lines so it is easy to select.
fn show_once(intro: &str, secret: &str) {
    println!("{}", paint(style::WARN, intro));
    println!();
    println!("    {}", paint(style::SECRET, secret));
    println!();
}

fn print<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce()) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).map_err(|err| err.to_string())?
        );
    } else {
        human();
    }
    Ok(())
}

fn show_net(net: &Net) {
    let primary = net
        .interface
        .as_deref()
        .and_then(|name| net.interfaces.iter().find(|iface| iface.name == name));
    let address = primary.and_then(|iface| iface.addresses.iter().find(|a| a.family == "ipv4"));
    let none = paint(style::MUTED, "(none)");
    let row = |label: &str, value: &str| println!("{} {value}", pad(style::LABEL, label, 12));

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
            "* carries the default route. `tessaro-ctl net interfaces` for details."
        )
    );
}

fn show_interface(iface: &NetInterface) {
    let marker = if iface.default_route {
        paint(style::OK, "  (default route)")
    } else {
        String::new()
    };
    let row = |label: &str, value: &str| println!("    {} {value}", pad(style::LABEL, label, 9));
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

fn show_key(key: &KeyInfo) {
    // Nothing reads a read-only key, so it is the one kind that restarts
    // nothing - and its value is reported, never set.
    let read_only = key.applies.is_empty();
    let current = match (&key.value, &key.default) {
        (Some(value), _) if read_only => {
            let shown = if value.is_empty() {
                paint(style::MUTED, "(none)")
            } else {
                value.clone()
            };
            format!(
                "{shown}  {}",
                paint(style::MUTED, "(read-only, reported by the device)")
            )
        }
        (Some(value), _) => format!("{}  {}", paint(style::OK, value), paint(style::OK, "(set)")),
        (None, Some(default)) => format!("{default}  {}", paint(style::MUTED, "(default)")),
        (None, None) => paint(style::MUTED, "(not set)"),
    };
    let restarts = key
        .applies
        .iter()
        .map(|consumer| match consumer {
            protocol::keys::Consumer::Agent => "the agent (invisible on screen)",
            protocol::keys::Consumer::Browser => "the browser",
            protocol::keys::Consumer::Weston => "the display (Weston, browser and agent)",
            protocol::keys::Consumer::Network => {
                "nothing: the network profiles are switched, and checked before it is saved"
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let row = |label: &str, value: &str| println!("    {} {value}", pad(style::LABEL, label, 9));
    println!("{}", paint(style::HEADING, &key.name));
    println!("    {}", key.doc);
    row("value", &current);
    if key.value.is_some() {
        if let Some(default) = &key.default {
            row("default", default);
        }
    }
    row("accepts", &key.values);
    if !read_only {
        row("restarts", &restarts);
    }
    if key.guarded {
        row(
            "note",
            &paint(
                style::WARN,
                "applied on probation: `tessaro-ctl confirm` within 60s or it reverts",
            ),
        );
    }
    if !key.env.is_empty() {
        row("env", &paint(style::MUTED, &key.env));
    }
}

/// `label` padded to the column `show_node` and `show_status` share.
fn node_row(label: &str, value: &str) {
    println!("{} {value}", pad(style::LABEL, label, 12));
}

fn show_node(node: &NodeInfo) {
    node_row("name", &paint(style::HEADING, &node.name));
    node_row("node id", &node.id);
    node_row("machine", &node.machine);
    node_row("agent", &node.version);
    node_row("fingerprint", &paint(style::MUTED, &node.fingerprint));
    node_row("claimed", &style::yes_no(node.claimed));
}

fn show_status(status: &Status) {
    show_node(&status.node);
    if let Some(os) = &status.os {
        match &status.image_version {
            Some(version) => node_row(
                "os",
                &format!("{os}, {} {version}", paint(style::LABEL, "image")),
            ),
            None => node_row("os", os),
        }
    }
    node_row("revision", &status.revision.to_string());
    if status.maintenance {
        node_row(
            "maintenance",
            &format!(
                "{} {} {} {}",
                paint(style::WARN, "on"),
                paint(style::MUTED, "-"),
                paint(style::CMD, "tessaro-ctl maintenance off"),
                paint(style::MUTED, "returns to kiosk.url")
            ),
        );
    }
    if status.debug_screen {
        node_row(
            "debug screen",
            &format!(
                "{} {} {} {}",
                paint(style::WARN, "on"),
                paint(style::MUTED, "-"),
                paint(style::CMD, "tessaro-ctl debug off"),
                paint(style::MUTED, "returns to the page below")
            ),
        );
    }
    node_row("kiosk url", &status.kiosk_url);
    node_row(
        "showing",
        &status
            .current_url
            .clone()
            .unwrap_or_else(|| paint(style::WARN, "(cannot tell)")),
    );
    node_row(
        "browser",
        &if status.browser_answering {
            paint(style::OK, "answering")
        } else {
            paint(style::BAD, "not answering")
        },
    );
    for (unit, state) in &status.units {
        println!(
            "  {} {}",
            pad(style::LABEL, unit, 24),
            paint(style::unit_state(state), state)
        );
    }
    if let Some(pending) = &status.pending {
        println!(
            "{} {}={} - {} within {}s or it goes back to {}",
            paint(style::WARN, "on probation"),
            pending.key,
            pending.value,
            paint(style::CMD, "`tessaro-ctl confirm`"),
            pending.seconds_left,
            pending.previous.as_deref().unwrap_or("the default")
        );
    }
}

fn show_applied(applied: &Applied, no_apply: bool) {
    if applied.changed.is_empty() {
        println!(
            "{}",
            paint(
                style::MUTED,
                format!("nothing changed (revision {})", applied.revision)
            )
        );
        return;
    }
    println!(
        "{} {}",
        paint(style::MUTED, format!("revision {}:", applied.revision)),
        paint(style::OK, applied.changed.join(", "))
    );
    if no_apply {
        println!("{}", paint(style::MUTED, "saved; nothing restarted"));
    } else if applied.restarted.is_empty() {
        println!("{}", paint(style::MUTED, "nothing to restart"));
    } else {
        println!(
            "{}",
            paint(
                style::WARN,
                format!("restarting {}", applied.restarted.join(", "))
            )
        );
    }
    if let Some(pending) = &applied.pending {
        println!();
        println!(
            "{} Check the screen, then run\n\n    {}\n\n\
             within {}s, or it goes back to {} on its own.",
            paint(
                style::WARN,
                format!("{}={} is on probation.", pending.key, pending.value)
            ),
            paint(style::CMD, "tessaro-ctl confirm"),
            pending.seconds_left,
            pending.previous.as_deref().unwrap_or("the default")
        );
    }
}

/// `--max-size`: one of the sizes the device offers, as `100k`, `1m`, ...
fn parse_payload(text: &str) -> Result<u64, String> {
    protocol::SPEEDTEST_SIZES
        .into_iter()
        .find(|size| size_label(*size) == text.to_ascii_lowercase())
        .ok_or_else(|| {
            let offered: Vec<String> = protocol::SPEEDTEST_SIZES.map(size_label).into();
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
    let label = |text: &str| pad(style::LABEL, text, 9);
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
                pad(style::HEADING, size_label(*size), 5),
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

/// `unit: message`, from one journal JSON object.
fn journal_line(event: &Value) -> String {
    let field = |name: &str| event.get(name).and_then(Value::as_str);
    let source = field("SYSLOG_IDENTIFIER")
        .or_else(|| field("_SYSTEMD_UNIT"))
        .unwrap_or("?");
    let message = match event.get("MESSAGE") {
        Some(Value::String(text)) => text.clone(),
        // Non-UTF-8 messages come as a byte array.
        Some(Value::Array(bytes)) => {
            let bytes: Vec<u8> = bytes
                .iter()
                .filter_map(|b| b.as_u64().map(|b| b as u8))
                .collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Some(other) => other.to_string(),
        None => event.to_string(),
    };
    format!("{} {message}", paint(style::SOURCE, format!("{source}:")))
}

fn default_client_name() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "someone".to_string());
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|host| host.trim().to_string())
        .filter(|host| !host.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "a laptop".to_string());
    format!("{user}@{host}")
}

/// A known device that answered, pin and id checked, somewhere other than its
/// cached address has moved: remember where, so the next command by name
/// goes straight there instead of scanning.
fn refresh_address(nodes: &mut Nodes, session: &Session) -> Result<(), String> {
    let Some((address, _)) = &session.remote else {
        return Ok(());
    };
    let address = address.to_string();
    let Some(known) = nodes.by_id(&session.node.id) else {
        return Ok(());
    };
    if known.address == address {
        return Ok(());
    }

    eprintln!(
        "{}",
        paint(
            style::WARN,
            format!(
                "{}: now at {address} (was {}), remembered",
                known.name, known.address
            )
        )
    );
    let moved = Node {
        address,
        ..known.clone()
    };
    nodes.put(moved);
    nodes.save()
}

fn remember(
    nodes: &mut Nodes,
    session: &Session,
    token: Option<String>,
    local: bool,
) -> Result<(), String> {
    if local {
        return Ok(()); // the local socket needs neither a pin nor a token
    }
    let (address, fingerprint) = session
        .remote
        .clone()
        .ok_or_else(|| "no remote session to remember".to_string())?;
    nodes.put(Node {
        id: session.node.id.clone(),
        name: session.node.name.clone(),
        address: address.to_string(),
        fingerprint,
        token,
    });
    nodes.save()
}

fn forget_session(nodes: &mut Nodes, session: &Session) -> Result<(), String> {
    if session.remote.is_some() && nodes.remove(&session.node.id) {
        nodes.save()?;
        println!("forgot {} on this machine", session.node.name);
    }
    Ok(())
}

fn forget(nodes: &mut Nodes, node: &str) -> Result<(), String> {
    let id = nodes
        .by_name(node.trim_end_matches(".local"))
        .or_else(|| nodes.by_id(node))
        .or_else(|| nodes.by_address(node))
        .map(|known| known.id.clone())
        .ok_or_else(|| format!("{node} is not a known node"))?;
    nodes.remove(&id);
    nodes.save()?;
    println!("forgot {node}");
    Ok(())
}

fn confirm_destructive(session: &Session, yes: bool, what: &str) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    eprintln!(
        "{} {}.",
        paint(style::WARN, format!("This will {what} on")),
        paint(style::HEADING, &session.node.name)
    );
    eprint!(
        "{}",
        paint(style::LABEL, "Type the device name to go ahead: ")
    );
    let mut typed = String::new();
    std::io::stdin()
        .read_line(&mut typed)
        .map_err(|err| err.to_string())?;
    if typed.trim() == session.node.name {
        Ok(())
    } else {
        Err("not confirmed".to_string())
    }
}

fn list_nodes(nodes: &Nodes, wait: u64, json: bool) -> Result<(), String> {
    let found = connect::browse(std::time::Duration::from_secs(wait));
    if json {
        let list: Vec<Value> = found
            .iter()
            .map(|found| {
                serde_json::json!({
                    "name": found.name,
                    "address": found.address.to_string(),
                    "id": found.id,
                    "claimed": found.claimed,
                    "known": found.id.as_deref().is_some_and(|id| nodes.by_id(id).is_some()),
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&list).map_err(|err| err.to_string())?
        );
        return Ok(());
    }

    if found.is_empty() {
        println!(
            "{}",
            paint(
                style::MUTED,
                format!("no Tessaro devices answered within {wait}s")
            )
        );
    }
    for found in &found {
        let known = found.id.as_deref().and_then(|id| nodes.by_id(id));
        let claimed = match found.claimed {
            Some(true) => pad(style::OK, "claimed", 10),
            Some(false) => pad(style::WARN, "UNCLAIMED", 10),
            None => pad(style::MUTED, "?", 10),
        };
        let pin = match (known, &found.fingerprint) {
            (Some(node), Some(fp)) if &node.fingerprint != fp => paint(style::BAD, "PIN MISMATCH"),
            (Some(_), _) => paint(style::OK, "known"),
            (None, _) => String::new(),
        };
        println!(
            "{} {} {claimed} {pin}",
            pad(style::HEADING, &found.name, 28),
            pad(style::MUTED, found.address, 22)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anstream::adapter::strip_str;

    fn plain(step: SpeedtestEvent) -> String {
        strip_str(&speedtest_line(&step)).to_string()
    }

    #[test]
    fn speedtest_lines_strip_to_aligned_plain_text() {
        assert_eq!(
            plain(SpeedtestEvent::Transfer {
                direction: Direction::Upload,
                size: 1_000_000,
                samples: 3,
                attempts: 4,
                median_mbit: Some(42.5),
                min_mbit: Some(40.0),
                max_mbit: None,
            }),
            "upload    1m    42.5 Mbit/s (min 40.0 Mbit/s, max n/a, 3/4 samples)"
        );
        assert_eq!(
            plain(SpeedtestEvent::Result {
                download_mbit: Some(93.14),
                upload_mbit: None,
                latency_ms: Some(12.0),
            }),
            "result    download 93.1 Mbit/s, upload n/a, latency 12.0 ms"
        );
    }

    #[test]
    fn max_size_takes_the_offered_sizes_only() {
        assert_eq!(parse_payload("25M"), Ok(25_000_000));
        assert_eq!(parse_payload("100k"), Ok(100_000));
        assert!(parse_payload("5m").is_err());
    }
}
