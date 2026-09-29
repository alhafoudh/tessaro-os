//! tessaro-ctl: the one way to manage a Tessaro kiosk.
//!
//! On the device, as root, it talks to the agent over the local socket and
//! needs nothing else. From anywhere else, `--node` (or `TESSARO_NODE`) names
//! the device by IP, `name.local` or plain name, and the conversation is TLS
//! with a pinned certificate and a token.
//!
//! A fresh device is unclaimed and answers every command without a token or
//! a pin, credentials aside. `tessaro-ctl --node NAME access claim` takes it,
//! stores the token in ~/.config/tessaro/tessaro.db, and prints the device's
//! new root password - once.

mod audio;
mod connect;
mod devtools;
mod files;
mod net;
mod policies;
mod progress;
mod prompt;
mod schedule;
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
use protocol::api::{self, Empty, Endpoint};
use protocol::keys;
use protocol::{Applied, Done, EvalResult, KeyInfo, NodeInfo, RestartTarget, Source, Status};
use serde_json::Value;
use tessaro_client::access;
use tessaro_client::describe::device as describe;
use tessaro_client::nodes::Nodes;
use tessaro_client::webconfig;

use connect::{Answer, Session, Target, Trust};
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
        ~/.config/tessaro/tessaro.db.\n\n\
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
        \x20 tessaro-ctl network proxy set 'http://jan:s3cret@proxy.corp.test:8080' && tessaro-ctl network proxy test\n\
        \x20 tessaro-ctl network speedtest --no-proxy       the link itself, around the proxy\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 device ping    from here to the device\n\
        \x20 tessaro-ctl storage show                       disk size, unallocated space, how full /data is\n\
        \x20 tessaro-ctl storage grow                       give /data the rest of the disk, no reboot\n\
        \x20 tessaro-ctl config get network.ip              one read-only value\n\
        \x20 tessaro-ctl config set 'browser.url=https://menu.test/?ip={network.ip}'  read-only keys are placeholders too\n\
        \x20 tessaro-ctl config set browser.fps_counter=on\n\
        \x20 tessaro-ctl browser maintenance on             show the maintenance page; `off` goes back\n\
        \x20 tessaro-ctl browser debug on                   name and addresses full screen; `off` goes back\n\
        \x20 tessaro-ctl browser zoom 125                   page zoom, like Ctrl+/- in Chrome\n\
        \x20 tessaro-ctl -n brave-otter-3fa2 browser devtools   the kiosk tab in chrome://inspect, over ssh\n\
        \x20 tessaro-ctl audio show                         where sound plays, how loud, what is plugged in\n\
        \x20 tessaro-ctl audio output hdmi && tessaro-ctl audio volume 60 && tessaro-ctl audio test\n\
        \x20 tessaro-ctl time show                          timezone, NTP sync, offset and drift\n\
        \x20 tessaro-ctl time timezone Europe/Bratislava && tessaro-ctl time ntp on --server ntp.corp.test\n\
        \x20 tessaro-ctl schedule create night --on '*-*-* 22:00' --run 'tessaro-ctl screen power off'\n\
        \x20 tessaro-ctl schedule list                      when each runs next, how the last run ended\n\
        \x20 tessaro-ctl config unset browser.url           back to the image default\n\
        \x20 tessaro-ctl device logs -f -u tessaro-agent.service\n\
        \x20 tessaro-ctl update send tessaro-os-qemux86-64.rootfs.wic.zst   a new image; settings are kept\n\
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
        \x20 tessaro-ctl network certs add corp-root-ca.pem trust an intranet or TLS-inspecting CA\n\
        \x20 source <(tessaro-ctl completion bash)          tab completion; also zsh, powershell\n\n\
        ENVIRONMENT:\n\
        \x20 TESSARO_NODE        default for --node\n\
        \x20 TESSARO_TOKEN       use this token instead of the stored one\n\
        \x20 TESSARO_CONFIG_DIR  where tessaro.db lives (default ~/.config/tessaro)\n\
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
    /// The physical display: what is on it, its modes, its power and the
    /// on-screen keyboard.
    #[command(subcommand)]
    Screen(ScreenCmd),
    /// What the browser shows: a URL, maintenance mode, the debug screen, and
    /// what the page runs: the injected script, the page bridge, eval.
    #[command(subcommand)]
    Browser(BrowserCmd),
    /// Sound: which output plays and which input records, volume, a test.
    #[command(subcommand)]
    Audio(audio::AudioCmd),
    /// The clock: timezone, NTP servers and sync, setting it by hand.
    #[command(subcommand)]
    Time(time::TimeCmd),
    /// Command lines the device runs on calendar times: create, change,
    /// switch on and off, run now, their output.
    #[command(subcommand)]
    Schedule(schedule::ScheduleCmd),
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
    /// handshake, then round trips over the API connection. Needs no
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
    /// Open Webconfig, the device's management pages, in the browser. On a
    /// claimed device it arrives signed in, through a one-time ticket in the
    /// address, never the token.
    Webconfig {
        /// Print the address, ticket included, instead of opening a browser:
        /// for a machine without one. The ticket is good for a minute, once.
        #[arg(long)]
        print: bool,
    },
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
    /// Switch the display off or on, or with neither say which it is. Off
    /// stays off through touches and a restart of the compositor, until
    /// `on` or a reboot.
    ///
    ///   tessaro-ctl screen power off
    Power { state: Option<Toggle> },
    /// Show or hide the on-screen keyboard. It follows a focused field, so
    /// `show` focuses --selector, or the field that has the focus. Needs
    /// screen.osk=always, or auto with no hardware keyboard.
    ///
    ///   tessaro-ctl screen keyboard show --selector '#search'
    Keyboard {
        action: KeyboardAction,
        /// With `show`: a CSS selector for the field to type into.
        #[arg(long)]
        selector: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum KeyboardAction {
    Show,
    Hide,
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
    /// The kiosk tab in Chrome DevTools on this machine: forwards
    /// localhost:9222 over ssh to the device's DevTools port, until Ctrl-C.
    /// Open chrome://inspect and the tab is under Remote Target. While a
    /// DevTools window is connected the agent leaves the tab alone - no
    /// restart, reload or navigation - so a breakpoint can sit. The key is
    /// sent as for `tessaro-ctl ssh connect`.
    ///
    ///   tessaro-ctl -n brave-otter-3fa2 browser devtools
    Devtools(devtools::Options),
    /// Reload the page on screen, past the cache.
    Reload,
    /// Empty the browser's HTTP cache.
    ClearCache,
    /// Run a script from the file store in every page, before the page's
    /// own scripts: browser.inject.script. Uploading a new copy of the file
    /// reloads the page with it.
    ///
    ///   tessaro-ctl files upload inject.js
    ///   tessaro-ctl browser inject on --script inject.js
    Inject {
        state: Toggle,
        /// With `on`: the file, from the store's root.
        #[arg(long)]
        script: Option<String>,
    },
    /// What the page gets as window.tessaro: off, config (the settings,
    /// read-only) or actions (the settings and device actions such as
    /// reload, volume and screen power). browser.bridge.mode.
    ///
    ///   tessaro-ctl browser bridge config
    Bridge {
        #[arg(value_parser = PossibleValuesParser::new(keys::BRIDGE_MODES))]
        mode: String,
    },
    /// Run JavaScript in the page on screen now and print what it returns.
    /// A returned Promise is waited for.
    ///
    ///   tessaro-ctl browser eval 'document.title'
    ///   tessaro-ctl browser eval --file fix.js
    ///   echo 'location.reload()' | tessaro-ctl browser eval -
    Eval {
        /// The code, or - to read it from stdin.
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        code: Option<String>,
        /// Read the code from a file.
        #[arg(long)]
        file: Option<String>,
        /// Seconds to wait, at most 60.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u64).range(1..=60))]
        timeout: u64,
        /// Do not wait for a returned Promise.
        #[arg(long)]
        no_await: bool,
        /// Run as if the screen had just been touched: for audio,
        /// fullscreen and the keyboard.
        #[arg(long)]
        gesture: bool,
    },
    /// Extra Chromium policies, merged over the image's: named documents,
    /// set from a file or edited in $EDITOR. The device sets the device-API
    /// origins, the proxy and CACertificates itself.
    ///
    ///   tessaro-ctl browser policies set lockdown lockdown.json
    ///   tessaro-ctl browser policies show
    #[command(subcommand)]
    Policies(policies::PoliciesCmd),
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
    /// Upload IMAGE, a .wic.zst (or an older build's .wic.bz2) with its
    /// .wic.bmap next to it, and reboot the device into it. Only the blocks
    /// the bmap lists are written, and only to the root partition, plus the
    /// kernel file; /data is kept. Run it again after a dropped connection
    /// and it resumes.
    ///
    ///   tessaro-ctl -n brave-otter-3fa2 update send tessaro-os-qemux86-64.rootfs.wic.zst
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
        Cmd::Browser(BrowserCmd::Policies(policies::PoliciesCmd::Check { file })) => {
            return policies::check(file, cli.json)
        }
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
            let status = session.fetch::<api::device::Status>()?;
            print(json, &status, || show_status(&status))
        }
        Cmd::Device(DeviceCmd::Id) => {
            let node = session.fetch::<api::device::Id>()?;
            print(json, &node, || show_node(&node))
        }
        Cmd::Config(ConfigCmd::Keys { key }) => {
            let mut keys = session.fetch::<api::config::Keys>()?;
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
        Cmd::Browser(BrowserCmd::Policies(what)) => policies::run(&mut session, what, json),
        Cmd::Storage(what) => storage::run(&mut session, what, json),
        Cmd::Device(DeviceCmd::Ping { count, interval }) => {
            net::ping(&mut session, json, count, interval)
        }
        Cmd::Screen(ScreenCmd::Modes) => {
            let connectors = session.fetch::<api::screen::Modes>()?;
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
            let settings = session.call::<api::config::Get>(api::ConfigQuery { key }, ())?;
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
            let change = Change::Set(api::SetConfig {
                values,
                if_revision: how.if_revision,
                apply: !how.no_apply,
                verify: how.verify.verify.clone(),
            });
            change.send(&mut session, json, network, &how)
        }
        Cmd::Config(ConfigCmd::Unset { keys, how }) => {
            let network = keys.iter().any(|key| net::is_network_key(key));
            let change = Change::Unset(api::UnsetConfig {
                keys,
                if_revision: how.if_revision,
                apply: !how.no_apply,
                verify: how.verify.verify.clone(),
            });
            change.send(&mut session, json, network, &how)
        }
        Cmd::Screen(ScreenCmd::Confirm) => {
            done::<api::screen::Confirm>(&mut session, Empty {}, (), json)
        }
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
            done::<api::browser::Navigate>(&mut session, Empty {}, api::NavigateBody { url }, json)
        }
        Cmd::Browser(BrowserCmd::Zoom { percent }) => {
            let values = BTreeMap::from([(keys::ZOOM.to_string(), percent.to_string())]);
            let applied = set(&mut session, values)?;
            print(json, &applied, || show_applied(&applied, false))
        }
        Cmd::Browser(BrowserCmd::Devtools(options)) => devtools::run(&mut session, options, json),
        Cmd::Browser(BrowserCmd::Reload) => {
            done::<api::browser::Reload>(&mut session, Empty {}, (), json)
        }
        Cmd::Browser(BrowserCmd::ClearCache) => {
            done::<api::browser::ClearCache>(&mut session, Empty {}, (), json)
        }
        Cmd::Browser(BrowserCmd::Inject { state, script }) => {
            let value = match (state, script) {
                (Toggle::On, Some(script)) => script,
                (Toggle::On, None) => {
                    return Err("`tessaro-ctl browser inject on` needs --script FILE".to_string())
                }
                (Toggle::Off, None) => String::new(),
                (Toggle::Off, Some(_)) => {
                    return Err("--script goes with `tessaro-ctl browser inject on`".to_string())
                }
            };
            let values = BTreeMap::from([(keys::INJECT_SCRIPT.to_string(), value.clone())]);
            let applied = set(&mut session, values)?;
            print(json, &applied, || {
                let line = match (value.is_empty(), applied.changed.is_empty()) {
                    (false, false) => {
                        paint(style::OK, format!("injecting {value} into every page"))
                    }
                    (false, true) => paint(style::OK, format!("{value} was already injected")),
                    (true, false) => paint(style::OK, "no script injected any more"),
                    (true, true) => paint(style::OK, "no script was injected"),
                };
                println!("{line}");
                show_applied(&applied, false)
            })
        }
        Cmd::Browser(BrowserCmd::Bridge { mode }) => {
            let values = BTreeMap::from([(keys::BRIDGE_MODE.to_string(), mode.clone())]);
            let applied = set(&mut session, values)?;
            print(json, &applied, || {
                let what = match mode.as_str() {
                    "config" => "window.tessaro has the settings, read-only",
                    "actions" => "window.tessaro has the settings and the device actions",
                    _ => "the page gets no window.tessaro",
                };
                println!(
                    "{} {}",
                    paint(style::OK, format!("bridge {mode}")),
                    paint(style::MUTED, format!("- {what}"))
                );
                show_applied(&applied, false)
            })
        }
        Cmd::Browser(BrowserCmd::Eval {
            code,
            file,
            timeout,
            no_await,
            gesture,
        }) => {
            let code = match (code.as_deref(), file) {
                (_, Some(path)) => {
                    std::fs::read_to_string(&path).map_err(|err| format!("{path}: {err}"))?
                }
                (Some("-"), None) => {
                    let mut code = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut code)
                        .map_err(|err| format!("stdin: {err}"))?;
                    code
                }
                (Some(code), None) => code.to_string(),
                (None, None) => unreachable!("clap requires the code or --file"),
            };
            let result = session.send::<api::browser::Eval>(api::EvalBody {
                code,
                timeout_ms: Some(timeout * 1000),
                await_promise: !no_await,
                user_gesture: gesture,
            })?;
            print(json, &result, || show_eval(&result))?;
            match &result.exception {
                Some(_) => Err("the script threw".to_string()),
                None => Ok(()),
            }
        }
        Cmd::Screen(ScreenCmd::Power { state }) => {
            let on = state.map(|state| state == Toggle::On);
            let power = match on {
                Some(on) => session.send::<api::screen::PowerSet>(api::ScreenPowerBody { on })?,
                None => session.fetch::<api::screen::Power>()?,
            };
            print(json, &power, || {
                let (label, note) = match (power.on, on.is_some()) {
                    (true, true) => (paint(style::OK, "screen on"), String::new()),
                    (false, true) => (
                        paint(style::WARN, "screen off"),
                        format!(
                            "until {} or a reboot",
                            paint(style::CMD, "tessaro-ctl screen power on")
                        ),
                    ),
                    (true, false) => (paint(style::OK, "the screen is on"), String::new()),
                    (false, false) => (paint(style::WARN, "the screen is off"), String::new()),
                };
                println!("{label} {}", paint(style::MUTED, note));
            })
        }
        Cmd::Screen(ScreenCmd::Keyboard { action, selector }) => {
            if selector.is_some() && action == KeyboardAction::Hide {
                return Err("--selector goes with `tessaro-ctl screen keyboard show`".to_string());
            }
            let body = api::KeyboardBody {
                show: action == KeyboardAction::Show,
                selector,
            };
            done::<api::screen::Keyboard>(&mut session, Empty {}, body, json)
        }
        Cmd::Audio(command) => audio::run(&mut session, command, json),
        Cmd::Time(command) => time::run(&mut session, command, json),
        Cmd::Schedule(command) => schedule::run(&mut session, command, json),
        Cmd::Device(DeviceCmd::Restart { what }) => {
            done::<api::device::Restart>(&mut session, Empty {}, api::RestartBody { what }, json)
        }
        Cmd::Device(DeviceCmd::Reboot) => {
            done::<api::device::Reboot>(&mut session, Empty {}, (), json)
        }
        Cmd::Screen(ScreenCmd::Screenshot { output }) => {
            let bytes = session
                .download::<api::screen::Screenshot>(Empty {})
                .into_result()?
                .body;
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
        }) => session.logs(
            api::LogsQuery {
                unit,
                lines: Some(lines),
                cursor: None,
            },
            follow,
            // Following ends with Ctrl-C, which ends the process.
            &|| false,
            |event| {
                if json {
                    println!("{event}");
                } else {
                    println!("{}", journal_line(&event));
                }
            },
        ),
        Cmd::Access(AccessCmd::Claim { name, .. }) => {
            let claimed =
                access::claim(&mut session, name.as_deref().unwrap_or("")).into_result()?;
            remember(&mut nodes, &session, Some(claimed.token.clone()), local)?;
            if json {
                return print_json(&claimed);
            }
            println!("{}", style::line(&access::done("claimed", &session)));
            println!(
                "token {} saved in {}",
                claimed.token_id,
                tessaro_client::store::path().display()
            );
            println!();
            for (intro, secret) in access::secrets(&claimed) {
                show_once(&intro, &secret);
            }
            Ok(())
        }
        Cmd::Access(AccessCmd::Login { token, .. }) => {
            access::login(&mut session, &token)?;
            remember(&mut nodes, &session, Some(token), local)?;
            println!("{}", style::line(&access::done("logged in to", &session)));
            Ok(())
        }
        Cmd::Access(AccessCmd::Webconfig { print }) => {
            let address = webconfig::address(&mut session)?;
            if print {
                println!("{}", address.url);
                return Ok(());
            }
            webconfig::open(&address.url)?;
            println!(
                "{} {}",
                paint(style::OK, "opened"),
                paint(style::CMD, &address.shown)
            );
            Ok(())
        }
        Cmd::Nodes(_) | Cmd::Completion { .. } => unreachable!("handled above"),
        Cmd::Access(AccessCmd::Token(command)) => match command {
            TokenCmd::Create { name } => {
                let created = session.send::<api::access::TokenCreate>(api::NameBody { name })?;
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
                let tokens = session.fetch::<api::access::Tokens>()?;
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
            TokenCmd::Revoke { id } => {
                done::<api::access::TokenRevoke>(&mut session, api::TokenRef { id }, (), json)
            }
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
                let set = session.send::<api::access::Password>(api::PasswordBody { password })?;
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
                let keys = session.fetch::<api::ssh::Keys>()?;
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
                let revoked = session.call::<api::ssh::Revoke>(api::SshKeyQuery { key }, ())?;
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
            prompt::confirm_destructive(&session, yes, access::UNCLAIM_LOSES)?;
            done::<api::access::Unclaim>(&mut session, Empty {}, (), json)?;
            forget_session(&mut nodes, &session)
        }
        Cmd::Device(DeviceCmd::FactoryReset { yes }) => {
            prompt::confirm_destructive(&session, yes, access::FACTORY_RESET_LOSES)?;
            done::<api::device::FactoryReset>(&mut session, Empty {}, (), json)?;
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

/// `config set` or `config unset`.
enum Change {
    Set(api::SetConfig),
    Unset(api::UnsetConfig),
}

impl Change {
    fn request(self, session: &mut Session) -> Answer<Applied> {
        match self {
            Change::Set(body) => session.request::<api::config::Set>(Empty {}, body),
            Change::Unset(body) => session.request::<api::config::Unset>(Empty {}, body),
        }
    }

    /// Sent. A change to network keys waits for the device's own verdict on
    /// it; anything else is answered at once.
    fn send(
        self,
        session: &mut Session,
        json: bool,
        network: bool,
        how: &ChangeArgs,
    ) -> Result<(), String> {
        if network && !how.no_apply {
            return net::apply(
                session,
                json,
                "changing the network",
                &how.verify.verify,
                |session| self.request(session),
            );
        }
        let applied = self.request(session).into_result()?;
        print(json, &applied, || show_applied(&applied, how.no_apply))
    }
}

/// `KEY=VALUE ...` set and applied, with no revision check: what the
/// shorthand commands (`browser maintenance on`, `audio volume 40`) are.
pub(crate) fn set(
    session: &mut Session,
    values: BTreeMap<String, String>,
) -> Result<Applied, String> {
    session.send::<api::config::Set>(api::SetConfig {
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

/// An endpoint that answers with a `Done`, its message printed.
pub(crate) fn done<E: Endpoint<Response = Done>>(
    session: &mut Session,
    params: E::Params,
    body: E::Body,
    json: bool,
) -> Result<(), String> {
    let done = session.call::<E>(params, body)?;
    print(json, &done, || println!("{}", done.message))
}

/// A secret the device will never show again: the intro, then the secret
/// set off by blank lines so it is easy to select.
fn show_once(intro: &str, secret: &str) {
    lines(describe::once(intro, secret));
}

/// Shared lines, each on its own.
fn lines(lines: Vec<tessaro_client::text::Line>) {
    for line in lines {
        println!("{}", style::line(&line));
    }
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
    lines(describe::key(key));
}

fn show_node(node: &NodeInfo) {
    style::facts(&describe::node(node));
}

fn show_status(status: &Status) {
    let text = describe::status(status);
    style::facts(&text.facts);
    for unit in &text.units {
        println!(
            "  {} {}",
            pad(style::LABEL, &unit.label, 24),
            style::line(&unit.value)
        );
    }
    style::facts(&text.more);
    if let Some(pending) = &text.pending {
        println!("{}", style::line(pending));
    }
}

/// What `browser eval` came to: the value, or the exception on stderr.
fn show_eval(result: &EvalResult) {
    match describe::eval(result) {
        Ok(line) => println!("{}", style::line(&line)),
        Err(line) => eprintln!("{}", style::line(&line)),
    }
}

fn show_applied(applied: &Applied, no_apply: bool) {
    lines(describe::applied(applied, no_apply));
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
    nodes.forget(&id)?;
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
