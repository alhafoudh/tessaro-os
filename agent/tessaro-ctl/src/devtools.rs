//! `tessaro-ctl browser devtools`: the kiosk tab in this machine's Chrome
//! DevTools, through an SSH tunnel to the device's `127.0.0.1:9222`.
//!
//! What the forward says and how it is watched is
//! `tessaro_client::devtools`, shared with the GUI.

use std::path::PathBuf;

use anstream::println;
use serde_json::json;
use tessaro_client::devtools as shared;
use tessaro_client::report::Report;
use tessaro_client::ssh;
use tessaro_client::text::Line;
use tessaro_client::tunnel::{self, Prompts, Tunnel};

use crate::connect::Session;
use crate::style::{self, paint};

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
    // A port taken here falls back to a free one, which `explain` says how
    // to add to chrome://inspect.
    let port = tunnel::free_port(options.local_port)?;
    let argv = tunnel::argv(&authorized, port, tunnel::DEVTOOLS, Prompts::Terminal);
    if options.print {
        if json {
            return crate::print_json(&json!({ "port": port, "command": argv }));
        }
        println!("{}", paint(style::CMD, ssh::shell_words(&argv)));
        return Ok(());
    }

    let mut tunnel = Tunnel::open(&authorized, port, tunnel::DEVTOOLS, Prompts::Terminal)?;
    if json {
        crate::print_json(&json!({ "port": port, "command": argv }))?;
    } else {
        for line in shared::explain(&session.node.name, port, "Ctrl-C closes the tunnel") {
            println!("{}", style::line(&line));
        }
    }
    shared::watch(session, &mut tunnel, &mut Stdout { json })
}

/// The connect and disconnect news on stdout; nothing with `--json`.
struct Stdout {
    json: bool,
}

impl Report for Stdout {
    fn progress(&mut self, _: Line, _: u64, _: u64) {}

    fn line(&mut self, line: Line) {
        if !self.json {
            println!("{}", style::line(&line));
        }
    }
}
