//! The kiosk tab in this machine's Chrome DevTools, through an SSH tunnel to
//! the device's `127.0.0.1:9222`, for `tessaro-ctl browser devtools` and
//! the GUI's Browser page alike.
//!
//! The key is sent and the host key pinned as for `ssh connect` (`ssh`), the
//! forward is `tunnel`, and while it is up the device's `Status.devtools`
//! says whether a DevTools window is connected through it - which is when
//! the agent leaves the tab alone (docs/kiosk-browser.md).

use std::time::{Duration, Instant};

use protocol::api;

use crate::connect::Session;
use crate::report::Report;
use crate::text::{Line, Tone};
use crate::tunnel::{self, Tunnel};

/// How often the device is asked whether DevTools is connected.
const POLL: Duration = Duration::from_secs(3);

/// What the forward is and how to use it. `closing` says how this client
/// closes it: Ctrl-C, a Cancel button.
pub fn explain(name: &str, port: u16, closing: &str) -> Vec<Line> {
    let mut lines = vec![
        Line::of(Tone::Ok, "forwarding")
            .text(" ")
            .add(Tone::Heading, format!("localhost:{port}"))
            .text(" ")
            .add(Tone::Muted, format!("to the DevTools of {name}")),
        Line::plain("  open ")
            .add(Tone::Cmd, "chrome://inspect")
            .text(" in Chrome: the kiosk tab is under Remote Target"),
    ];
    if port != tunnel::DEVTOOLS_LOCAL {
        lines.push(Line::plain("  ").add(
            Tone::Muted,
            format!("add localhost:{port} under Discover network targets, Configure"),
        ));
    }
    lines.push(Line::plain("  ").add(
        Tone::Muted,
        "while DevTools is connected the agent leaves the tab alone: no restart, reload or navigation",
    ));
    lines.push(Line::of(Tone::Muted, closing));
    lines
}

/// DevTools came or went.
pub fn announce(connected: bool) -> Line {
    if connected {
        Line::of(Tone::Warn, "DevTools connected:").text(" the agent is leaving the browser alone")
    } else {
        Line::of(Tone::Ok, "DevTools disconnected:")
            .text(" the agent is watching the browser again")
    }
}

/// Keep the forward up and say when DevTools connects and disconnects,
/// until ssh ends (an error) or the user stops it.
pub fn watch(
    session: &mut Session,
    tunnel: &mut Tunnel,
    report: &mut dyn Report,
) -> Result<(), String> {
    let mut connected = None;
    while !report.stopped() {
        if let Some(why) = tunnel.ended() {
            return Err(why);
        }
        // A missed answer is not news; the next one will do.
        if let Ok(status) = session.fetch::<api::device::Status>() {
            let news = match connected {
                Some(was) => was != status.devtools,
                None => status.devtools,
            };
            if news {
                report.line(announce(status.devtools));
            }
            connected = Some(status.devtools);
        }
        let until = Instant::now() + POLL;
        while Instant::now() < until && !report.stopped() {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    Ok(())
}
