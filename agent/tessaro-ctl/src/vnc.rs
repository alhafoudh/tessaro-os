//! `tessaro-ctl screen vnc`: the device's screen in a VNC viewer on this
//! machine, through an SSH tunnel to the mirror on the device's
//! `127.0.0.1:5900`, which runs only while the tunnel asks for it.
//!
//! Starting the mirror, what the forward says and how it is kept are
//! `tessaro_client::vnc`, shared with the GUI. Ctrl-C is caught, so the
//! mirror is stopped as the tunnel closes rather than when the device's lease
//! runs out.

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
    // After the ssh prompts, which Ctrl-C should still end at once.
    interrupt::catch();
    let port = opened.tunnel.port;
    if json {
        crate::print_json(&json!({ "port": port, "mode": opened.vnc.mode }))?;
    } else {
        let closing = "Ctrl-C closes the tunnel and stops the mirror";
        for line in shared::explain(&session.node.name, port, &opened.vnc.mode, closing) {
            println!("{}", style::line(&line));
        }
    }
    shared::watch(session, &mut opened.tunnel, &mut Stdout { json })?;
    if !json {
        println!(
            "{}",
            paint(style::MUTED, "closed the tunnel and stopped the mirror")
        );
    }
    Ok(())
}

/// The viewers' news on stdout; nothing with `--json`. Stopped by Ctrl-C.
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

    fn stopped(&self) -> bool {
        interrupt::caught()
    }
}

/// Ctrl-C (and a plain `kill`) noted instead of ending the process, so the
/// watch can stop the mirror on its way out. The terminal sends Ctrl-C to
/// ssh too, which closes the tunnel by itself.
#[cfg(unix)]
mod interrupt {
    use std::sync::atomic::{AtomicBool, Ordering};

    static CAUGHT: AtomicBool = AtomicBool::new(false);

    extern "C" fn note(_: libc::c_int) {
        CAUGHT.store(true, Ordering::SeqCst);
    }

    pub fn catch() {
        let handler = note as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SAFETY: the handler only stores to an atomic, which is
        // async-signal-safe. signal() keeps SA_RESTART on Linux and macOS,
        // so a request in flight carries on.
        unsafe {
            libc::signal(libc::SIGINT, handler);
            libc::signal(libc::SIGTERM, handler);
        }
    }

    pub fn caught() -> bool {
        CAUGHT.load(Ordering::SeqCst)
    }
}

/// Elsewhere Ctrl-C ends the process, and the device's lease stops the
/// mirror.
#[cfg(not(unix))]
mod interrupt {
    pub fn catch() {}

    pub fn caught() -> bool {
        false
    }
}
