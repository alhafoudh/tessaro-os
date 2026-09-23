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
mod nodes;

use std::collections::BTreeMap;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use protocol::{
    Applied, Claimed, Command, Connector, Done, KeyInfo, NodeInfo, Password, Screenshot, Settings,
    Source, Status, Target as RestartTarget, TokenCreated, TokenInfo,
};
use serde_json::Value;

use connect::{Session, Target, Trust};
use nodes::{Node, Nodes};

#[derive(Parser)]
#[command(
    name = "tessaro-ctl",
    version,
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
        \x20 tessaro-ctl set 'kiosk.url=https://{store}.shop.test/?lang={lang}' url.store=north url.lang=sk\n\
        \x20 tessaro-ctl modes && tessaro-ctl set display.resolution=1920x1080 && tessaro-ctl confirm\n\
        \x20 tessaro-ctl set browser.fps_counter=on\n\
        \x20 tessaro-ctl unset kiosk.url                    back to the image default\n\
        \x20 tessaro-ctl logs -f -u tessaro-agent.service\n\
        \x20 tessaro-ctl token create phone                 a token for a second client\n\n\
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
    /// Current settings, or one of them.
    Get {
        key: Option<String>,
    },
    /// Change settings: KEY=VALUE ... Restarts only what reads them.
    ///
    /// `tessaro-ctl keys` lists every KEY with what it accepts. Several pairs
    /// are applied as one change. Any `url.NAME=VALUE` fills the `{NAME}`
    /// placeholder in kiosk.url, percent-encoded, e.g.
    ///
    ///   tessaro-ctl set 'kiosk.url=https://{store}.shop.test/?lang={lang}' url.store=north url.lang=sk
    Set {
        #[arg(required = true, value_name = "KEY=VALUE")]
        pairs: Vec<String>,
        /// Refuse unless the settings are still at this revision.
        #[arg(long)]
        if_revision: Option<u64>,
        /// Save and render, but restart nothing yet.
        #[arg(long)]
        no_apply: bool,
    },
    /// Go back to the image default for KEY ...
    Unset {
        #[arg(required = true)]
        keys: Vec<String>,
        #[arg(long)]
        if_revision: Option<u64>,
        #[arg(long)]
        no_apply: bool,
    },
    /// Keep a change that is on probation (display.resolution).
    Confirm,
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
    /// Release the device: every token removed, root password emptied.
    Unclaim {
        #[arg(long, short)]
        yes: bool,
    },
    /// Defaults, unclaimed, empty root password - the fresh-install state.
    FactoryReset {
        #[arg(long, short)]
        yes: bool,
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
enum PasswordCmd {
    /// Set it: prompted, or generated with --random and shown once.
    Set {
        #[arg(long)]
        random: bool,
    },
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
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("tessaro-ctl: {err}");
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
        Cmd::Id => Trust::Peek,
        Cmd::Claim { yes, .. } | Cmd::Login { yes, .. } => Trust::Pin { assume_yes: *yes },
        _ => Trust::KnownOnly,
    };
    let follow = matches!(cli.command, Cmd::Logs { follow: true, .. });
    let mut session = connect::open(&target, &nodes, trust, follow)?;
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
                let template = wanted.starts_with("url.");
                keys.retain(|k| &k.name == wanted || (template && k.name == "url.<name>"));
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
        Cmd::Modes => {
            let connectors: Vec<Connector> = call(&mut session, Command::Modes)?;
            print(json, &connectors, || {
                if connectors.is_empty() {
                    println!("no connected display reports its modes");
                }
                for connector in &connectors {
                    println!("{}:", connector.name);
                    for (at, mode) in connector.modes.iter().enumerate() {
                        let note = if at == 0 { "  (preferred)" } else { "" };
                        println!("  {mode}{note}");
                    }
                }
                println!("\nset one with: tessaro-ctl set display.resolution=WIDTHxHEIGHT");
            })
        }
        Cmd::Get { key } => {
            let settings: Settings = call(&mut session, Command::Get { key })?;
            print(json, &settings, || {
                for setting in &settings.settings {
                    let value = setting.value.as_deref().unwrap_or("");
                    let source = match setting.source {
                        Source::Set => "",
                        Source::Default => "  (default)",
                    };
                    println!("{} = {value}{source}", setting.key);
                }
                println!("# revision {}", settings.revision);
            })
        }
        Cmd::Set {
            pairs,
            if_revision,
            no_apply,
        } => {
            let mut values = BTreeMap::new();
            for pair in pairs {
                let (key, value) = pair
                    .split_once('=')
                    .ok_or_else(|| format!("{pair}: expected KEY=VALUE"))?;
                values.insert(key.to_string(), value.to_string());
            }
            let applied: Applied = call(
                &mut session,
                Command::Set {
                    values,
                    if_revision,
                    apply: !no_apply,
                },
            )?;
            print(json, &applied, || show_applied(&applied, no_apply))
        }
        Cmd::Unset {
            keys,
            if_revision,
            no_apply,
        } => {
            let applied: Applied = call(
                &mut session,
                Command::Unset {
                    keys,
                    if_revision,
                    apply: !no_apply,
                },
            )?;
            print(json, &applied, || show_applied(&applied, no_apply))
        }
        Cmd::Confirm => done(&mut session, Command::Confirm, json),
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
            println!("{path} ({} bytes)", bytes.len());
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
        Cmd::Claim { name, .. } => {
            let name = name.unwrap_or_else(default_client_name);
            // A token left over from before an unclaim means nothing now.
            session.clear_token();
            let claimed: Claimed = call(&mut session, Command::Claim { name })?;
            remember(&mut nodes, &session, Some(claimed.token.clone()), local)?;
            if json {
                return print(true, &claimed, || {});
            }
            println!("claimed {} ({})", session.node.name, session.node.id);
            println!(
                "token {} saved in {}",
                claimed.token_id,
                nodes::dir().join("nodes.json").display()
            );
            println!();
            println!("root password - shown this once, store it now:");
            println!();
            println!("    {}", claimed.root_password);
            println!();
            Ok(())
        }
        Cmd::Login { token, .. } => {
            session.set_token(token.clone());
            // Prove the token before storing it.
            let _: Vec<TokenInfo> = call(&mut session, Command::TokenList)?;
            remember(&mut nodes, &session, Some(token), local)?;
            println!("logged in to {} ({})", session.node.name, session.node.id);
            Ok(())
        }
        Cmd::Forget { .. } | Cmd::Nodes { .. } => unreachable!("handled above"),
        Cmd::Token { command } => match command {
            TokenCmd::Create { name } => {
                let created: TokenCreated = call(&mut session, Command::TokenCreate { name })?;
                print(json, &created, || {
                    println!("token {} - shown this once:", created.id);
                    println!();
                    println!("    {}", created.token);
                    println!();
                    println!(
                        "use it with: tessaro-ctl --node {} login --token <token>",
                        session.node.name
                    );
                })
            }
            TokenCmd::List => {
                let tokens: Vec<TokenInfo> = call(&mut session, Command::TokenList)?;
                print(json, &tokens, || {
                    for token in &tokens {
                        println!(
                            "{}  {:<24} issued by {}",
                            token.id, token.name, token.issued_by
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
                        println!("root password - shown this once, store it now:");
                        println!();
                        println!("    {password}");
                        println!();
                    }
                    None => println!("root password changed"),
                })
            }
        },
        Cmd::Unclaim { yes } => {
            confirm_destructive(
                &session,
                yes,
                "remove every token and empty the root password",
            )?;
            done(&mut session, Command::Unclaim, json)?;
            forget_session(&mut nodes, &session)
        }
        Cmd::FactoryReset { yes } => {
            confirm_destructive(
                &session,
                yes,
                "erase every setting, remove every token and empty the root password",
            )?;
            done(&mut session, Command::FactoryReset, json)?;
            forget_session(&mut nodes, &session)
        }
    }
}

fn call<T: serde::de::DeserializeOwned>(
    session: &mut Session,
    command: Command,
) -> Result<T, String> {
    let value = session.call(command)?;
    serde_json::from_value(value).map_err(|err| format!("unexpected answer: {err}"))
}

fn done(session: &mut Session, command: Command, json: bool) -> Result<(), String> {
    let done: Done = call(session, command)?;
    print(json, &done, || println!("{}", done.message))
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

fn show_key(key: &KeyInfo) {
    let current = match (&key.value, &key.default) {
        (Some(value), _) => format!("{value}  (set)"),
        (None, Some(default)) => format!("{default}  (default)"),
        (None, None) => "(not set)".to_string(),
    };
    let restarts = key
        .applies
        .iter()
        .map(|consumer| match consumer {
            protocol::keys::Consumer::Agent => "the agent (invisible on screen)",
            protocol::keys::Consumer::Browser => "the browser",
            protocol::keys::Consumer::Weston => "the display (Weston, browser and agent)",
        })
        .collect::<Vec<_>>()
        .join(", ");

    println!("{}", key.name);
    println!("    {}", key.doc);
    println!("    value     {current}");
    if key.value.is_some() {
        if let Some(default) = &key.default {
            println!("    default   {default}");
        }
    }
    println!("    accepts   {}", key.values);
    println!("    restarts  {restarts}");
    if key.guarded {
        println!(
            "    note      applied on probation: `tessaro-ctl confirm` within 60s or it reverts"
        );
    }
    if !key.env.is_empty() {
        println!("    env       {}", key.env);
    }
}

fn show_node(node: &NodeInfo) {
    println!("name         {}", node.name);
    println!("node id      {}", node.id);
    println!("machine      {}", node.machine);
    println!("agent        {}", node.version);
    println!("fingerprint  {}", node.fingerprint);
    println!("claimed      {}", if node.claimed { "yes" } else { "no" });
}

fn show_status(status: &Status) {
    show_node(&status.node);
    println!("revision     {}", status.revision);
    println!("kiosk url    {}", status.kiosk_url);
    println!(
        "showing      {}",
        status.current_url.as_deref().unwrap_or("(cannot tell)")
    );
    println!(
        "browser      {}",
        if status.browser_answering {
            "answering"
        } else {
            "not answering"
        }
    );
    for (unit, state) in &status.units {
        println!("  {unit:<24} {state}");
    }
    if let Some(pending) = &status.pending {
        println!(
            "on probation {}={} - `tessaro-ctl confirm` within {}s or it goes back to {}",
            pending.key,
            pending.value,
            pending.seconds_left,
            pending.previous.as_deref().unwrap_or("the default")
        );
    }
}

fn show_applied(applied: &Applied, no_apply: bool) {
    if applied.changed.is_empty() {
        println!("nothing changed (revision {})", applied.revision);
        return;
    }
    println!(
        "revision {}: {}",
        applied.revision,
        applied.changed.join(", ")
    );
    if no_apply {
        println!("saved; nothing restarted");
    } else if applied.restarted.is_empty() {
        println!("nothing to restart");
    } else {
        println!("restarting {}", applied.restarted.join(", "));
    }
    if let Some(pending) = &applied.pending {
        println!();
        println!(
            "{}={} is on probation. Check the screen, then run\n\n    tessaro-ctl confirm\n\n\
             within {}s, or it goes back to {} on its own.",
            pending.key,
            pending.value,
            pending.seconds_left,
            pending.previous.as_deref().unwrap_or("the default")
        );
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
    format!("{source}: {message}")
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
    eprintln!("This will {what} on {}.", session.node.name);
    eprint!("Type the device name to go ahead: ");
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
        println!("no Tessaro devices answered within {wait}s");
    }
    for found in &found {
        let known = found.id.as_deref().and_then(|id| nodes.by_id(id));
        let claimed = match found.claimed {
            Some(true) => "claimed",
            Some(false) => "UNCLAIMED",
            None => "?",
        };
        let pin = match (known, &found.fingerprint) {
            (Some(node), Some(fp)) if &node.fingerprint != fp => "PIN MISMATCH",
            (Some(_), _) => "known",
            (None, _) => "",
        };
        println!(
            "{:<28} {:<22} {:<10} {pin}",
            found.name, found.address, claimed
        );
    }
    Ok(())
}
