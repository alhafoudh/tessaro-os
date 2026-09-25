//! tessaro-ctl: the one way to manage a Tessaro kiosk.
//!
//! On the device, as root, it talks to the agent over the local socket and
//! needs nothing else. From anywhere else, `--node` (or `TESSARO_NODE`) names
//! the device by IP, `name.local` or plain name, and the conversation is TLS
//! with a pinned certificate and a token.
//!
//! A fresh device is unclaimed and answers every command without a token or
//! a pin, credentials aside. `tessaro-ctl --node NAME access claim` takes it,
//! stores the token in ~/.config/tessaro/nodes.json, and prints the device's
//! new root password - once.

mod audio;
mod connect;
mod files;
mod net;
mod progress;
mod prompt;
mod ssh;
mod storage;
mod style;
mod time;
mod update;

use std::collections::BTreeMap;
use std::process::ExitCode;

// Shadow the std macros: these strip colors when stdout is not a terminal.
use anstream::{eprintln, println};
use clap::builder::styling::Styles;
use clap::builder::{PossibleValuesParser, TypedValueParser};
use clap::{Args, ColorChoice, CommandFactory, Parser, Subcommand, ValueEnum};
use protocol::keys;
use protocol::{
    Applied, Claimed, Command, Connector, Done, KeyInfo, NodeInfo, Password, RestartTarget,
    Screenshot, Settings, Source, SshKeyInfo, SshKeyRevoked, Status, TokenCreated, TokenInfo,
};
use serde_json::Value;
use tessaro_client::nodes::{self, Nodes};

use connect::{Session, Target, Trust};
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
        certificate and a token, which `access claim` or `access login` stores in \
        ~/.config/tessaro/nodes.json.\n\n\
        `tessaro-ctl config keys` documents every setting: what it accepts, its default, what is \
        set, and what a change restarts.",
    after_long_help = "EXAMPLES:\n\
        \x20 tessaro-ctl nodes list                         devices answering on this network\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 access claim   take a fresh device; prints its root password once\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 device status\n\
        \x20 tessaro-ctl config keys                        every setting, documented\n\
        \x20 tessaro-ctl config keys screen.resolution      one setting in full\n\
        \x20 tessaro-ctl config set browser.url=https://shop.test/\n\
        \x20 tessaro-ctl config set 'browser.url=https://menu.test/?table={data.table}' data.table=12\n\
        \x20 tessaro-ctl config set 'browser.url=https://{device.name}.menu.test/'  any setting is a placeholder too\n\
        \x20 tessaro-ctl screen modes && tessaro-ctl config set screen.resolution=1920x1080 && tessaro-ctl screen confirm\n\
        \x20 tessaro-ctl network show                       address, gateway, DNS, interfaces\n\
        \x20 tessaro-ctl network interfaces                 every interface in detail\n\
        \x20 tessaro-ctl network profiles list              NetworkManager's profiles\n\
        \x20 tessaro-ctl config set network.ethernet.mode=static network.ethernet.address=192.168.1.50/24 network.ethernet.gateway=192.168.1.1\n\
        \x20                                                kept only if the gateway still answers\n\
        \x20 tessaro-ctl config set network.ethernet.mode=dhcp\n\
        \x20 tessaro-ctl network wifi scan && tessaro-ctl network wifi join Office   the hotspot goes down\n\
        \x20 tessaro-ctl config set network.wifi.mode=hotspot   back to the hotspot, tessaro-NAME\n\
        \x20 tessaro-ctl config set network.wifi.nat=0      hotspot clients reach the device only\n\
        \x20 tessaro-ctl network last                       what the last change did, if the answer never came\n\
        \x20 tessaro-ctl network ping 192.168.1.1           from the device\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 device ping    from here to the device\n\
        \x20 tessaro-ctl storage show                       disk size, unallocated space, how full /data is\n\
        \x20 tessaro-ctl storage grow                       give /data the rest of the disk, no reboot\n\
        \x20 tessaro-ctl config get network.ip              one read-only value\n\
        \x20 tessaro-ctl config set 'browser.url=https://menu.test/?ip={network.ip}'  read-only keys are placeholders too\n\
        \x20 tessaro-ctl config set browser.fps_counter=on\n\
        \x20 tessaro-ctl browser maintenance on             show the maintenance page; `off` goes back\n\
        \x20 tessaro-ctl browser debug on                   name and addresses full screen; `off` goes back\n\
        \x20 tessaro-ctl browser zoom 125                   page zoom, like Ctrl+/- in Chrome\n\
        \x20 tessaro-ctl audio show                         where sound plays, how loud, what is plugged in\n\
        \x20 tessaro-ctl audio output hdmi && tessaro-ctl audio volume 60 && tessaro-ctl audio test\n\
        \x20 tessaro-ctl time show                          timezone, NTP sync, offset and drift\n\
        \x20 tessaro-ctl time timezone Europe/Bratislava && tessaro-ctl time ntp on --server ntp.corp.test\n\
        \x20 tessaro-ctl config unset browser.url           back to the image default\n\
        \x20 tessaro-ctl device logs -f -u tessaro-agent.service\n\
        \x20 tessaro-ctl update send tessaro-os-qemux86-64.rootfs.wic.bz2   a new image; settings are kept\n\
        \x20 tessaro-ctl files sync ./site-assets           the store now holds exactly that directory\n\
        \x20                                                the page reads it at http://127.0.0.1/files/...\n\
        \x20 tessaro-ctl files upload promo.mp4 media/      one file, keeping its name\n\
        \x20 tessaro-ctl files list /media                  like ls -l; -R for the whole tree\n\
        \x20 tessaro-ctl files move /promo.mp4 /media/      like mv\n\
        \x20 tessaro-ctl access token create phone          a token for a second client\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 ssh connect    a root shell, by your ~/.ssh key\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 ssh connect -- journalctl -fu tessaro-agent\n\
        \x20 tessaro-ctl ssh keys list                      keys that can log in as root\n\
        \x20 tessaro-ctl ssh keys revoke user@laptop        by comment or fingerprint\n\
        \x20 source <(tessaro-ctl completion bash)          tab completion; also zsh, powershell\n\n\
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

/// `tessaro-ctl <group> <command>`, always. The groups are nouns for what a
/// command acts on, and a setting key starts with the group that acts on the
/// same thing (`screen confirm`, `screen.resolution`). The rules for adding
/// to the tree are "tessaro-ctl command and key structure" in CLAUDE.md.
#[derive(Subcommand)]
enum Cmd {
    /// The device itself: who it is, what it is doing, its journal, restarts.
    #[command(subcommand)]
    Device(DeviceCmd),
    /// Who may manage the device: claiming it, tokens, the root password.
    #[command(subcommand)]
    Access(AccessCmd),
    /// A root shell on the device by key, and the keys that may log in.
    #[command(subcommand)]
    Ssh(SshCmd),
    /// The device's settings: documented, read and changed.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// The device's network: addresses, profiles, WiFi, ping, speed test.
    #[command(subcommand)]
    Network(net::NetworkCmd),
    /// The device's disk: partitions, free space, growing /data.
    #[command(subcommand)]
    Storage(storage::StorageCmd),
    /// The physical display: what is on it, and its modes.
    #[command(subcommand)]
    Screen(ScreenCmd),
    /// What the browser shows: a URL, maintenance mode, the debug screen.
    #[command(subcommand)]
    Browser(BrowserCmd),
    /// Sound: which output plays and which input records, volume, a test.
    #[command(subcommand)]
    Audio(audio::AudioCmd),
    /// The clock: timezone, NTP servers and sync, setting it by hand.
    #[command(subcommand)]
    Time(time::TimeCmd),
    /// Put a new image on the device, keeping its settings and claim.
    #[command(subcommand)]
    Update(UpdateCmd),
    /// The device's file store, served to the kiosk at
    /// http://127.0.0.1/files/: upload, download, sync, list, remove.
    #[command(subcommand)]
    Files(files::FilesCmd),
    /// The devices this client knows or finds. Needs no device.
    #[command(subcommand)]
    Nodes(NodesCmd),
    /// Print the completion script for a shell.
    ///
    ///   source <(tessaro-ctl completion bash)
    ///   tessaro-ctl completion zsh > ~/.zfunc/_tessaro-ctl
    Completion { shell: CompletionShell },
}

#[derive(Subcommand)]
enum DeviceCmd {
    /// What the device is doing right now.
    Status,
    /// Who the device is: node id, name, TLS fingerprint, claim state.
    Id,
    /// How fast the device answers this client: the TCP connect, the TLS
    /// handshake, then round trips over the control connection. Needs no
    /// token, like `id`.
    Ping {
        #[arg(long, short = 'c', default_value_t = protocol::PING_DEFAULT_COUNT)]
        count: u32,
        /// Seconds between round trips.
        #[arg(long, short = 'i', default_value_t = net::seconds(protocol::PING_DEFAULT_INTERVAL_MS))]
        interval: f64,
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
    /// Restart the browser, the display (Weston, with the browser and agent)
    /// or the agent.
    Restart {
        #[arg(value_parser = PossibleValuesParser::new(RestartTarget::NAMES)
            .map(|name| name.parse::<RestartTarget>().expect("one of the names")))]
        what: RestartTarget,
    },
    Reboot,
    /// Defaults, unclaimed, empty root password - the fresh-install state.
    FactoryReset {
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum AccessCmd {
    /// Take an unclaimed device: get a token and its new root password.
    Claim {
        /// What to call this client in `access token list`.
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
    /// Release the device: every token and ssh key removed, root password
    /// emptied.
    Unclaim {
        #[arg(long, short)]
        yes: bool,
    },
    /// Tokens on the device.
    #[command(subcommand)]
    Token(TokenCmd),
    /// The device's root password.
    #[command(subcommand)]
    Password(PasswordCmd),
}

#[derive(Subcommand)]
enum SshCmd {
    /// A root shell on the device. Sends your SSH public key over this
    /// pinned connection, adds it to root's authorized_keys, then runs ssh
    /// with the host key the device reported - no password, no first-use
    /// prompt. An unclaimed device takes no key: ssh logs in with its empty
    /// root password and checks no host key. Anything after `--` goes to
    /// ssh: options or a command.
    Connect(ssh::Options),
    /// The SSH keys that can log in as root. Unclaiming or a factory reset
    /// removes them all.
    #[command(subcommand)]
    Keys(SshKeysCmd),
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Every setting, documented: accepted values, default, current value,
    /// and what a change restarts. With KEY, just that one.
    Keys { key: Option<String> },
    /// Current settings, or one of them.
    Get { key: Option<String> },
    /// Change settings: KEY=VALUE ... Restarts only what reads them.
    ///
    /// `tessaro-ctl config keys` lists every KEY with what it accepts.
    /// Several pairs are applied as one change. `data.NAME=VALUE` defines a
    /// custom value, with a NAME you choose. Any setting's full key in braces
    /// is a placeholder in browser.url, filled percent-encoded:
    /// `{data.NAME}`, `{device.name}`, `{screen.osk}`, ...
    ///
    ///   tessaro-ctl config set 'browser.url=https://menu.test/?table={data.table}&screen={data.screen}' data.table=12 data.screen=entrance
    ///
    /// The network.ethernet.* and network.wifi.* keys switch the device's own
    /// network profiles. That change is applied and checked by the device
    /// before it is saved at all - see --verify - and rolled back by the
    /// device alone if it cuts it off:
    ///
    ///   tessaro-ctl config set network.ethernet.mode=static network.ethernet.address=192.168.1.50/24 network.ethernet.gateway=192.168.1.1 network.ethernet.dns=192.168.1.1
    ///   tessaro-ctl config set network.ethernet.mode=dhcp
    ///   tessaro-ctl config set network.wifi.mode=hotspot
    Set {
        #[arg(required = true, value_name = "KEY=VALUE")]
        pairs: Vec<String>,
        #[command(flatten)]
        how: ChangeArgs,
    },
    /// Go back to the image default for KEY ...
    Unset {
        #[arg(required = true)]
        keys: Vec<String>,
        #[command(flatten)]
        how: ChangeArgs,
    },
}

/// How `config set` and `config unset` apply a change.
#[derive(Args)]
struct ChangeArgs {
    /// Refuse unless the settings are still at this revision.
    #[arg(long)]
    if_revision: Option<u64>,
    /// Save and render, but restart nothing yet. Not for network keys,
    /// which are always applied and checked at once.
    #[arg(long)]
    no_apply: bool,
    #[command(flatten)]
    verify: net::VerifyArg,
}

#[derive(Subcommand)]
enum ScreenCmd {
    /// Save what the browser is rendering as a JPEG.
    Screenshot {
        #[arg(long, short)]
        output: Option<String>,
    },
    /// The resolutions the connected displays offer (for screen.resolution).
    Modes,
    /// Keep a change that is on probation (screen.resolution).
    Confirm,
}

#[derive(Subcommand)]
enum BrowserCmd {
    /// Point the browser at a URL until the next refresh.
    Navigate { url: String },
    /// Maintenance mode: show browser.maintenance.url instead of
    /// browser.url, which is left as it is. The same as
    /// `config set browser.maintenance.enable=1|0`.
    ///
    ///   tessaro-ctl browser maintenance on --url 'http://127.0.0.1/maintenance.html?message=Back%20at%2014:00'
    Maintenance {
        state: Toggle,
        /// With `on`: set browser.maintenance.url in the same change.
        #[arg(long)]
        url: Option<String>,
    },
    /// The debug screen: browser.debug.template in large text over the whole
    /// screen, instead of the kiosk or maintenance page. The same as
    /// `config set browser.debug.enable=1|0`. `\n` breaks a line.
    ///
    ///   tessaro-ctl browser debug on --template 'IP {network.ip}\nGW {network.gateway}'
    Debug {
        state: Toggle,
        /// With `on`: set browser.debug.template in the same change.
        #[arg(long)]
        template: Option<String>,
    },
    /// Page zoom in percent, 25 to 500: Chrome's Ctrl+/- zoom for every
    /// site, on top of screen.scale; 100 is no zoom. Restarts the browser.
    /// The same as `tessaro-ctl config set browser.zoom=...`.
    ///
    ///   tessaro-ctl browser zoom 125
    Zoom {
        #[arg(value_parser = clap::value_parser!(u16).range(25..=500))]
        percent: u16,
    },
}

#[derive(Subcommand)]
enum NodesCmd {
    /// Devices answering on the local network.
    List {
        /// Seconds to listen.
        #[arg(long, default_value_t = 3)]
        wait: u64,
    },
    /// Forget a device: its pin and token on this machine.
    Forget { node: String },
}

/// The shells `completion` writes for. clap_complete's own enum offers fish
/// and elvish too, which nobody here uses.
#[derive(Clone, Copy, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Powershell,
}

impl From<CompletionShell> for clap_complete::Shell {
    fn from(shell: CompletionShell) -> Self {
        match shell {
            CompletionShell::Bash => clap_complete::Shell::Bash,
            CompletionShell::Zsh => clap_complete::Shell::Zsh,
            CompletionShell::Powershell => clap_complete::Shell::PowerShell,
        }
    }
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
enum SshKeysCmd {
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
    Send(update::Send),
    /// What is under way, and what the last update did.
    Status,
    /// Drop the upload, or the staged update before it is applied.
    Cancel,
}

#[derive(Subcommand)]
enum PasswordCmd {
    /// Set it: prompted, given as PASSWORD or on stdin, or generated with
    /// --random and shown once. PASSWORD on the command line lands in shell
    /// history and the process list; scripts should prefer --password-stdin.
    ///
    ///   tessaro-ctl access password set
    ///   tessaro-ctl access password set 'correct horse battery'
    ///   printf %s "$PW" | tessaro-ctl access password set --password-stdin
    Set {
        /// The new password, instead of a prompt.
        #[arg(conflicts_with_all = ["random", "password_stdin"])]
        password: Option<String>,
        /// Read the password from stdin instead of prompting.
        #[arg(long, conflicts_with = "random")]
        password_stdin: bool,
        #[arg(long)]
        random: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Toggle {
    On,
    Off,
}

impl Toggle {
    /// The value a `*.enable` or `*.mute` setting takes.
    fn flag(self) -> &'static str {
        match self {
            Toggle::On => "1",
            Toggle::Off => "0",
        }
    }
}

/// Rust starts with SIGPIPE ignored, so `tessaro-ctl config keys | head` panics on
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
    if let Cmd::Completion { shell } = cli.command {
        print!("{}", completion(shell));
        return Ok(());
    }

    let mut nodes = Nodes::load()?;

    // The commands that do not need a device conversation at all.
    match &cli.command {
        Cmd::Nodes(NodesCmd::List { wait }) => return list_nodes(&nodes, *wait, cli.json),
        Cmd::Nodes(NodesCmd::Forget { node }) => return forget(&mut nodes, node),
        _ => {}
    }

    let target = connect::resolve(cli.node.as_deref(), &nodes)?;
    let local = matches!(target, Target::Local(_));
    let trust = match &cli.command {
        Cmd::Device(DeviceCmd::Id | DeviceCmd::Ping { .. }) => Trust::Peek,
        Cmd::Access(AccessCmd::Claim { yes, .. } | AccessCmd::Login { yes, .. }) => {
            Trust::Pin { assume_yes: *yes }
        }
        _ => Trust::KnownOnly,
    };
    let mut session = connect::open(&target, &nodes, trust)?;
    refresh_address(&mut nodes, &session)?;
    let json = cli.json;

    match cli.command {
        Cmd::Device(DeviceCmd::Status) => {
            let status: Status = session.call(Command::Status)?;
            print(json, &status, || show_status(&status))
        }
        Cmd::Device(DeviceCmd::Id) => {
            let node: NodeInfo = session.call(Command::Id)?;
            print(json, &node, || show_node(&node))
        }
        Cmd::Config(ConfigCmd::Keys { key }) => {
            let mut keys: Vec<KeyInfo> = session.call(Command::Keys)?;
            if let Some(wanted) = &key {
                let template = wanted.starts_with(protocol::keys::DATA_PREFIX);
                keys.retain(|k| &k.name == wanted || (template && k.name == "data.<name>"));
                if keys.len() > 1 {
                    keys.retain(|k| &k.name == wanted);
                }
                if keys.is_empty() {
                    return Err(format!(
                        "{wanted} is not a setting; `tessaro-ctl config keys` lists them"
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
        Cmd::Network(what) => net::run(&mut session, what, json),
        Cmd::Storage(what) => storage::run(&mut session, what, json),
        Cmd::Device(DeviceCmd::Ping { count, interval }) => {
            net::ping(&mut session, json, count, interval)
        }
        Cmd::Screen(ScreenCmd::Modes) => {
            let connectors: Vec<Connector> = session.call(Command::Modes)?;
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
                        "tessaro-ctl config set screen.resolution=WIDTHxHEIGHT"
                    )
                );
            })
        }
        Cmd::Config(ConfigCmd::Get { key }) => {
            let settings: Settings = session.call(Command::Get { key })?;
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
        Cmd::Config(ConfigCmd::Set { pairs, how }) => {
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
                if_revision: how.if_revision,
                apply: !how.no_apply,
                verify: how.verify.verify.clone(),
            };
            change(&mut session, json, command, network, &how)
        }
        Cmd::Config(ConfigCmd::Unset { keys, how }) => {
            let network = keys.iter().any(|key| net::is_network_key(key));
            let command = Command::Unset {
                keys,
                if_revision: how.if_revision,
                apply: !how.no_apply,
                verify: how.verify.verify.clone(),
            };
            change(&mut session, json, command, network, &how)
        }
        Cmd::Screen(ScreenCmd::Confirm) => done(&mut session, Command::Confirm, json),
        Cmd::Browser(BrowserCmd::Maintenance { state, url }) => toggle(
            &mut session,
            json,
            &Mode {
                name: "maintenance",
                flag: keys::MAINTENANCE_ENABLE,
                with: ("--url", keys::MAINTENANCE_URL),
                shows: keys::MAINTENANCE_URL,
                back: keys::URL,
                command: "browser maintenance",
            },
            state,
            url,
        ),
        Cmd::Browser(BrowserCmd::Debug { state, template }) => toggle(
            &mut session,
            json,
            &Mode {
                name: "debug screen",
                flag: keys::DEBUG_ENABLE,
                with: ("--template", keys::DEBUG_TEMPLATE),
                shows: keys::DEBUG_TEMPLATE,
                back: "the kiosk page (or the maintenance page)",
                command: "browser debug",
            },
            state,
            template,
        ),
        Cmd::Browser(BrowserCmd::Navigate { url }) => {
            done(&mut session, Command::Navigate { url }, json)
        }
        Cmd::Browser(BrowserCmd::Zoom { percent }) => {
            let values = BTreeMap::from([(keys::ZOOM.to_string(), percent.to_string())]);
            let applied = set(&mut session, values)?;
            print(json, &applied, || show_applied(&applied, false))
        }
        Cmd::Audio(command) => audio::run(&mut session, command, json),
        Cmd::Time(command) => time::run(&mut session, command, json),
        Cmd::Device(DeviceCmd::Restart { what }) => {
            done(&mut session, Command::Restart { what }, json)
        }
        Cmd::Device(DeviceCmd::Reboot) => done(&mut session, Command::Reboot, json),
        Cmd::Screen(ScreenCmd::Screenshot { output }) => {
            let shot: Screenshot = session.call(Command::Screenshot)?;
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
        Cmd::Device(DeviceCmd::Logs {
            follow,
            unit,
            lines,
        }) => session.stream(
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
        Cmd::Access(AccessCmd::Claim { name, .. }) => {
            let name = name.unwrap_or_else(tessaro_client::client_name);
            // A token left over from before an unclaim means nothing now.
            session.clear_token();
            let claimed: Claimed = session.call(Command::Claim { name })?;
            remember(&mut nodes, &session, Some(claimed.token.clone()), local)?;
            if json {
                return print_json(&claimed);
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
        Cmd::Access(AccessCmd::Login { token, .. }) => {
            session.set_token(token.clone());
            // Prove the token before storing it.
            let _: Vec<TokenInfo> = session.call(Command::TokenList)?;
            remember(&mut nodes, &session, Some(token), local)?;
            println!(
                "{} {} {}",
                paint(style::OK, "logged in to"),
                paint(style::HEADING, &session.node.name),
                paint(style::MUTED, format!("({})", session.node.id))
            );
            Ok(())
        }
        Cmd::Nodes(_) | Cmd::Completion { .. } => unreachable!("handled above"),
        Cmd::Access(AccessCmd::Token(command)) => match command {
            TokenCmd::Create { name } => {
                let created: TokenCreated = session.call(Command::TokenCreate { name })?;
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
                                "tessaro-ctl --node {} access login --token <token>",
                                session.node.name
                            )
                        )
                    );
                })
            }
            TokenCmd::List => {
                let tokens: Vec<TokenInfo> = session.call(Command::TokenList)?;
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
        Cmd::Access(AccessCmd::Password(command)) => match command {
            PasswordCmd::Set {
                password,
                password_stdin,
                random,
            } => {
                let password = if random {
                    None
                } else {
                    let password = match password {
                        Some(password) => password,
                        None if password_stdin => prompt::password(true, "")?,
                        None => prompt::new_password("new root password: ")?,
                    };
                    protocol::check_password(&password)?;
                    Some(password)
                };
                let set: Password = session.call(Command::PasswordSet { password })?;
                print(json, &set, || match &set.password {
                    Some(password) => {
                        show_once("root password - shown this once, store it now:", password)
                    }
                    None => println!("{}", paint(style::OK, "root password changed")),
                })
            }
        },
        Cmd::Ssh(SshCmd::Connect(options)) => ssh::run(&mut session, options, json),
        Cmd::Ssh(SshCmd::Keys(command)) => match command {
            SshKeysCmd::List => {
                let keys: Vec<SshKeyInfo> = session.call(Command::SshKeyList)?;
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
            SshKeysCmd::Revoke { key } => {
                let revoked: SshKeyRevoked = session.call(Command::SshKeyRevoke { key })?;
                print(json, &revoked, || match &revoked.fingerprint {
                    Some(fingerprint) => println!(
                        "{} {}",
                        paint(style::OK, "revoked"),
                        paint(style::MUTED, fingerprint)
                    ),
                    None => println!("{}", revoked.message),
                })
            }
        },
        Cmd::Access(AccessCmd::Unclaim { yes }) => {
            prompt::confirm_destructive(
                &session,
                yes,
                "remove every token and ssh key and empty the root password",
            )?;
            done(&mut session, Command::Unclaim, json)?;
            forget_session(&mut nodes, &session)
        }
        Cmd::Device(DeviceCmd::FactoryReset { yes }) => {
            prompt::confirm_destructive(
                &session,
                yes,
                "erase every setting, remove every token and ssh key and empty the root password",
            )?;
            done(&mut session, Command::FactoryReset, json)?;
            forget_session(&mut nodes, &session)
        }
        Cmd::Update(command) => match command {
            UpdateCmd::Send(options) => {
                match update::send(&mut session, &target, &nodes, options, json)? {
                    update::Sent::Kept => Ok(()),
                    update::Sent::Wiped => forget_session(&mut nodes, &session),
                }
            }
            UpdateCmd::Status => update::status(&mut session, json),
            UpdateCmd::Cancel => update::cancel(&mut session, json),
        },
        Cmd::Files(command) => files::run(&mut session, command, json),
    }
}

/// The completion script for `shell`.
///
/// clap_complete 4.6's bash script names the root `tessaro__ctl` when it
/// walks the words typed, but `tessaro__subcmd__ctl` in the cases that list
/// what comes next - it escapes the dash in the binary's name inconsistently -
/// so nothing below the first word would ever complete. One spelling fixes it.
fn completion(shell: CompletionShell) -> String {
    let mut script = Vec::new();
    clap_complete::generate(
        clap_complete::Shell::from(shell),
        &mut Cli::command(),
        "tessaro-ctl",
        &mut script,
    );
    let script = String::from_utf8_lossy(&script).into_owned();
    match shell {
        CompletionShell::Bash => script.replace("tessaro__subcmd__ctl", "tessaro__ctl"),
        _ => script,
    }
}

/// `config set` or `config unset`, sent. A change to network keys waits for
/// the device's own verdict on it; anything else is answered at once.
fn change(
    session: &mut Session,
    json: bool,
    command: Command,
    network: bool,
    how: &ChangeArgs,
) -> Result<(), String> {
    if network && !how.no_apply {
        return net::apply(
            session,
            json,
            "changing the network",
            &how.verify.verify,
            command,
        );
    }
    let applied: Applied = session.call(command)?;
    print(json, &applied, || show_applied(&applied, how.no_apply))
}

/// `KEY=VALUE ...` set and applied, with no revision check: what the
/// shorthand commands (`browser maintenance on`, `audio volume 40`) are.
pub(crate) fn set(
    session: &mut Session,
    values: BTreeMap<String, String>,
) -> Result<Applied, String> {
    session.call(Command::Set {
        values,
        if_revision: None,
        apply: true,
        verify: Default::default(),
    })
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
    values.insert(mode.flag.to_string(), state.flag().to_string());
    if let Some(value) = value {
        values.insert(key.to_string(), value);
    }
    let applied = set(session, values)?;
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
    let done: Done = session.call(command)?;
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

/// `value` as pretty JSON with `--json`, otherwise what `human` prints.
fn print<T: serde::Serialize>(json: bool, value: &T, human: impl FnOnce()) -> Result<(), String> {
    if json {
        print_json(value)
    } else {
        human();
        Ok(())
    }
}

/// `value` as pretty JSON, the way every `--json` answer is printed.
pub(crate) fn print_json<T: serde::Serialize>(value: &T) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|err| err.to_string())?
    );
    Ok(())
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
            protocol::keys::Consumer::Audio => "nothing: applied to the sound server at once",
            protocol::keys::Consumer::Firmware => {
                "nothing: the Pi firmware reads it at the next reboot"
            }
            protocol::keys::Consumer::Time => {
                "nothing on screen: applied to the clock at once; systemd-timesyncd when its servers change"
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let row = style::sub_row;
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
                format!(
                    "applied on probation: `{CONFIRM_COMMAND}` within {}s or it reverts",
                    protocol::CONFIRM_SECONDS
                ),
            ),
        );
    }
    if !key.env.is_empty() {
        row("env", &paint(style::MUTED, &key.env));
    }
}

fn show_node(node: &NodeInfo) {
    style::row("name", &paint(style::HEADING, &node.name));
    style::row("node id", &node.id);
    style::row("machine", &node.machine);
    style::row("agent", &node.version);
    style::row("fingerprint", &paint(style::MUTED, &node.fingerprint));
    style::row("claimed", &style::yes_no(node.claimed));
}

fn show_status(status: &Status) {
    show_node(&status.node);
    if let Some(os) = &status.os {
        match &status.image_version {
            Some(version) => style::row(
                "os",
                &format!("{os}, {} {version}", paint(style::LABEL, "image")),
            ),
            None => style::row("os", os),
        }
    }
    style::row("revision", &status.revision.to_string());
    if let Some(data) = &status.data {
        style::row("data", &storage::usage_line(data));
    }
    if status.maintenance {
        style::row(
            "maintenance",
            &format!(
                "{} {} {} {}",
                paint(style::WARN, "on"),
                paint(style::MUTED, "-"),
                paint(style::CMD, "tessaro-ctl browser maintenance off"),
                paint(style::MUTED, "returns to browser.url")
            ),
        );
    }
    if status.debug_screen {
        style::row(
            "debug screen",
            &format!(
                "{} {} {} {}",
                paint(style::WARN, "on"),
                paint(style::MUTED, "-"),
                paint(style::CMD, "tessaro-ctl browser debug off"),
                paint(style::MUTED, "returns to the page below")
            ),
        );
    }
    style::row("browser url", &status.kiosk_url);
    style::row(
        "showing",
        &status
            .current_url
            .clone()
            .unwrap_or_else(|| paint(style::WARN, "(cannot tell)")),
    );
    style::row(
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
    if let Some(audio) = &status.audio {
        style::row("audio", &audio::summary(audio));
    }
    if let Some(summary) = &status.time {
        style::row("time", &time::summary(summary));
    }
    if let Some(pending) = &status.pending {
        println!(
            "{} {}={} - {} within {}s or it goes back to {}",
            paint(style::WARN, "on probation"),
            pending.key,
            pending.value,
            paint(style::CMD, format!("`{CONFIRM_COMMAND}`")),
            pending.seconds_left,
            pending.previous_or_default()
        );
    }
}

/// What keeps a change that is on probation.
const CONFIRM_COMMAND: &str = "tessaro-ctl screen confirm";
/// What applies a change the firmware reads at power-on.
const REBOOT_COMMAND: &str = "tessaro-ctl device reboot";

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
    if let Some(audio) = &applied.audio {
        audio::show_outcome(audio);
    }
    if let Some(time) = &applied.time {
        time::show_outcome(time);
    }
    if no_apply {
        println!("{}", paint(style::MUTED, "saved; nothing restarted"));
    } else if applied.restarted.is_empty() {
        if applied.audio.is_none() && applied.time.is_none() && !applied.reboot {
            println!("{}", paint(style::MUTED, "nothing to restart"));
        }
    } else {
        println!(
            "{}",
            paint(
                style::WARN,
                format!("restarting {}", applied.restarted.join(", "))
            )
        );
    }
    if applied.reboot {
        println!(
            "{} {}",
            paint(style::WARN, "takes effect at the next reboot:"),
            paint(style::CMD, REBOOT_COMMAND)
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
            paint(style::CMD, CONFIRM_COMMAND),
            pending.seconds_left,
            pending.previous_or_default()
        );
    }
}

/// `unit: message`, from one journal JSON object.
fn journal_line(event: &Value) -> String {
    let entry = tessaro_client::journal::Entry::parse(event);
    format!(
        "{} {}",
        paint(style::SOURCE, format!("{}:", entry.source)),
        entry.message
    )
}

/// A known device that answered, pin and id checked, somewhere other than its
/// cached address has moved: remember where, so the next command by name
/// goes straight there instead of scanning.
fn refresh_address(nodes: &mut Nodes, session: &Session) -> Result<(), String> {
    if let (Some(was), Some((address, _))) = (nodes.refresh(session)?, &session.remote) {
        eprintln!(
            "{}",
            paint(
                style::WARN,
                format!(
                    "{}: now at {address} (was {was}), remembered",
                    session.node.name
                )
            )
        );
    }
    Ok(())
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
    nodes.remember(session, token)
}

fn forget_session(nodes: &mut Nodes, session: &Session) -> Result<(), String> {
    if nodes.forget_session(session)? {
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
        return print_json(&list);
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

    #[test]
    fn bash_completes_below_the_first_word() {
        let script = completion(CompletionShell::Bash);
        // Every command the word walk can arrive at has a case of its own.
        for line in script.lines() {
            let Some(name) = line.trim().strip_prefix("cmd=\"") else {
                continue;
            };
            let name = name.trim_end_matches('"');
            if name.is_empty() {
                continue;
            }
            assert!(
                script.contains(&format!("        {name})")),
                "no case for {name}"
            );
        }
        assert!(script.contains("tessaro__ctl__subcmd__ssh__subcmd__keys)"));
    }

    #[test]
    fn password_set_takes_one_source() {
        let parse = |args: &[&str]| {
            Cli::try_parse_from(
                ["tessaro-ctl", "access", "password", "set"]
                    .iter()
                    .chain(args),
            )
        };
        assert!(parse(&[]).is_ok());
        assert!(parse(&["secret"]).is_ok());
        assert!(parse(&["--password-stdin"]).is_ok());
        assert!(parse(&["--random"]).is_ok());
        assert!(parse(&["secret", "--password-stdin"]).is_err());
        assert!(parse(&["secret", "--random"]).is_err());
        assert!(parse(&["--password-stdin", "--random"]).is_err());
    }
}
