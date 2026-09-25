//! `tessaro-ctl browser devtools`: the kiosk tab in this machine's Chrome
//! DevTools, through an SSH tunnel to the device's `127.0.0.1:9222`.
//!
//! The key is sent and the host key pinned as for `ssh connect`
//! (`tessaro_client::ssh`), the forward is `tessaro_client::tunnel`, and
//! while it is up the device's `Status.devtools` says whether a DevTools
//! window is connected through it - which is when the agent leaves the tab
//! alone (docs/kiosk-browser.md).

use std::path::PathBuf;
use std::time::Duration;

use anstream::println;
use protocol::{Command, Status};
use serde_json::json;
use tessaro_client::ssh;
use tessaro_client::tunnel::{self, Prompts, Tunnel};

use crate::connect::Session;
use crate::style::{self, paint};

/// How often the device is asked whether DevTools is connected.
const POLL: Duration = Duration::from_secs(3);

/// `browser devtools`, as it is typed.
#[derive(clap::Args)]
pub struct Options {
    /// The port on this machine. chrome://inspect looks at 9222 without
    /// being configured; any other needs adding under its Configure.
    #[arg(long, default_value_t = tunnel::DEVTOOLS_LOCAL)]
    pub local_port: u16,
    /// The key to send, as for `tessaro-ctl ssh connect`.
    #[arg(long, short = 'i', value_name = "PATH")]
    pub key: Option<PathBuf>,
    /// Send the key and print the ssh command instead of running it.
    #[arg(long)]
    pub print: bool,
}

pub fn run(session: &mut Session, options: Options, json: bool) -> Result<(), String> {
    let authorized = ssh::authorize(session, options.key.as_deref())?;
    let port = options.local_port;
    let argv = tunnel::argv(&authorized, port, tunnel::DEVTOOLS, Prompts::Terminal);
    if options.print {
        if json {
            return crate::print_json(&json!({ "port": port, "command": argv }));
        }
        println!("{}", paint(style::CMD, crate::ssh::shell_words(&argv)));
        return Ok(());
    }

    let mut tunnel = Tunnel::open(&authorized, port, tunnel::DEVTOOLS, Prompts::Terminal)?;
    if json {
        crate::print_json(&json!({ "port": port, "command": argv }))?;
    } else {
        explain(&session.node.name, port);
    }

    let mut connected = None;
    loop {
        if let Some(why) = tunnel.ended() {
            return Err(why);
        }
        // A missed answer is not news; the next one will do.
        if let Ok(status) = session.call::<Status>(Command::Status) {
            let news = match connected {
                Some(was) => was != status.devtools,
                None => status.devtools,
            };
            if news && !json {
                announce(status.devtools);
            }
            connected = Some(status.devtools);
        }
        std::thread::sleep(POLL);
    }
}

fn explain(name: &str, port: u16) {
    println!(
        "{} {} {}",
        paint(style::OK, "forwarding"),
        paint(style::HEADING, format!("localhost:{port}")),
        paint(style::MUTED, format!("to the DevTools of {name}"))
    );
    println!(
        "  open {} in Chrome: the kiosk tab is under Remote Target",
        paint(style::CMD, "chrome://inspect")
    );
    if port != tunnel::DEVTOOLS_LOCAL {
        println!(
            "  {}",
            paint(
                style::MUTED,
                format!("add localhost:{port} under Discover network targets, Configure")
            )
        );
    }
    println!(
        "  {}",
        paint(
            style::MUTED,
            "while DevTools is connected the agent leaves the tab alone: no restart, reload or navigation"
        )
    );
    println!("{}", paint(style::MUTED, "Ctrl-C closes the tunnel"));
}

fn announce(connected: bool) {
    if connected {
        println!(
            "{} {}",
            paint(style::WARN, "DevTools connected:"),
            "the agent is leaving the browser alone"
        );
    } else {
        println!(
            "{} {}",
            paint(style::OK, "DevTools disconnected:"),
            "the agent is watching the browser again"
        );
    }
}
