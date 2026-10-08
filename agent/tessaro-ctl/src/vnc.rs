//! `tessaro-ctl screen vnc`: the device's screen in a VNC viewer on this
//! machine, through an SSH tunnel to the mirror on the device's
//! `127.0.0.1:5900`, which runs only while the tunnel asks for it.
//!
//! Starting the mirror, what the forward says and how it is kept are
//! `tessaro_client::vnc`, shared with the GUI.

use std::path::PathBuf;

use crate::out::println;
use serde_json::json;
use tessaro_client::report::Report;
use tessaro_client::ssh;
use tessaro_client::text::Line;
use tessaro_client::tunnel::{self, Prompts};
use tessaro_client::vnc as shared;

use crate::connect::Session;
use crate::style::{self, paint};

/// `screen vnc`, as it is typed.
#[derive(clap::Args)]
pub struct Options {
    /// The port on this machine. A viewer given a host alone tries 5900; a
    /// taken port falls back to a free one, which is printed.
    #[arg(long, default_value_t = tunnel::VNC_LOCAL)]
    pub local_port: u16,
    /// The key to send, as for `tessaro-ctl ssh connect`.
    #[arg(long, short = 'i', value_name = "PATH")]
    pub key: Option<PathBuf>,
    /// Start the mirror, send the key and print the ssh command instead of
    /// running it. Nothing keeps the mirror going: it stops a minute after
    /// this unless a viewer is connected through the tunnel by then.
    #[arg(long)]
    pub print: bool,
}

pub fn run(session: &mut Session, options: Options, json: bool) -> Result<(), String> {
    if options.print {
        let vnc = shared::start(session)?;
        let authorized = ssh::authorize(session, options.key.as_deref())?;
        let port = tunnel::free_port(options.local_port)?;
        let argv = tunnel::argv(&authorized, port, tunnel::VNC, Prompts::Terminal);
        if json {
            return crate::print_json(&json!({ "port": port, "mode": vnc.mode, "command": argv }));
        }
        println!("{}", paint(style::CMD, ssh::shell_words(&argv)));
        return Ok(());
    }

    let mut opened = shared::open(
        session,
        options.key.as_deref(),
        options.local_port,
        Prompts::Terminal,
    )?;
    let port = opened.tunnel.port;
    if json {
        crate::print_json(&json!({ "port": port, "mode": opened.vnc.mode }))?;
    } else {
        // Ctrl-C ends the process before it can say so; the device stops
        // the mirror itself once its lease runs out.
        let closing = format!(
            "Ctrl-C closes the tunnel; the device stops mirroring within {}s",
            opened.vnc.lease_s
        );
        for line in shared::explain(&session.node.name, port, &opened.vnc.mode, &closing) {
            println!("{}", style::line(&line));
        }
    }
    shared::watch(session, &mut opened.tunnel, &mut Stdout { json })
}

/// The viewers' news on stdout; nothing with `--json`.
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
