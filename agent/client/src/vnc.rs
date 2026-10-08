//! The device's screen over VNC through an SSH tunnel, for `tessaro-ctl
//! screen vnc`, the GUI's VNC tunnel job and its built-in viewer alike.
//!
//! Nothing is mirrored until a tunnel asks (docs/remote-access.md): `open`
//! starts the mirror (`api::screen::VncStart`), then sends the key and pins
//! the host key as `ssh connect` does (`ssh`) and opens the forward
//! (`tunnel`). The device stops the mirror once no viewer is connected and
//! no start came within its lease, so whoever holds the tunnel starts it
//! again every `KEEP` (`watch`, `keep`), and says so when it closes (`stop`).

use std::path::Path;
use std::time::{Duration, Instant};

use protocol::{api, VncSession};

use crate::connect::Session;
use crate::report::Report;
use crate::ssh;
use crate::text::{Line, Tone};
use crate::tunnel::{self, Prompts, Tunnel};

/// How often the mirror is started again while the tunnel is up: well within
/// the device's lease, which is a minute.
pub const KEEP: Duration = Duration::from_secs(20);

/// The image's VNC login (`KIOSK_VNC_USER`/`KIOSK_VNC_PASSWORD`), an image
/// property with no setting, per docs/remote-access.md.
pub const USER: &str = "tessaro";
pub const PASSWORD: &str = "tessaro";

/// The mirror started and the forward to it.
pub struct Opened {
    pub tunnel: Tunnel,
    pub vnc: VncSession,
}

/// Start the mirror, or keep it going.
pub fn start(session: &mut Session) -> Result<VncSession, String> {
    session.send::<api::screen::VncStart>(())
}

/// Stop the mirror, as the tunnel closes. Best effort: a device that does
/// not answer stops it itself when its lease runs out.
pub fn stop(session: &mut Session) {
    let _ = session.send::<api::screen::VncStop>(());
}

/// Start the mirror and forward `local` on this machine (0 for any free
/// port, a taken one falls back to a free one) to it, with `key` sent as for
/// `ssh connect`.
pub fn open(
    session: &mut Session,
    key: Option<&Path>,
    local: u16,
    prompts: Prompts,
) -> Result<Opened, String> {
    let vnc = start(session)?;
    let authorized = ssh::authorize(session, key)?;
    let port = tunnel::free_port(local)?;
    let tunnel = Tunnel::open(&authorized, port, tunnel::VNC, prompts)?;
    Ok(Opened { tunnel, vnc })
}

/// What the forward is and how to use it. `closing` says how this client
/// closes it: Ctrl-C, a Cancel button.
pub fn explain(name: &str, port: u16, mode: &str, closing: &str) -> Vec<Line> {
    let mut lines = vec![
        Line::of(Tone::Ok, "forwarding")
            .text(" ")
            .add(Tone::Heading, format!("localhost:{port}"))
            .text(" ")
            .add(Tone::Muted, format!("to the screen of {name}")),
        Line::plain("  open ")
            .add(Tone::Cmd, format!("vnc://localhost:{port}"))
            .text(format!(" in a VNC viewer, and log in as {USER} / {PASSWORD}")),
        Line::plain("  ").add(
            Tone::Muted,
            "a viewer asking for the password alone (macOS Screen Sharing, RealVNC) takes the same password",
        ),
    ];
    if mode == "view-only" {
        lines.push(Line::plain("  ").add(
            Tone::Warn,
            "view only: screen.vnc=view-only drops what the viewer clicks and types",
        ));
    }
    lines.push(Line::plain("  ").add(
        Tone::Muted,
        "the screen is mirrored only while this tunnel is open or a viewer is connected",
    ));
    lines.push(Line::of(Tone::Muted, closing));
    lines
}

/// Viewers came or went.
pub fn announce(viewers: u32) -> Line {
    match viewers {
        0 => Line::of(Tone::Muted, "no viewer connected"),
        1 => Line::of(Tone::Ok, "a viewer connected"),
        n => Line::of(Tone::Ok, format!("{n} viewers connected")),
    }
}

/// Keep the mirror going and say when viewers come and go, until ssh ends
/// (an error) or the user stops it; then stop the mirror.
pub fn watch(
    session: &mut Session,
    tunnel: &mut Tunnel,
    report: &mut dyn Report,
) -> Result<(), String> {
    let watched = keep(session, tunnel, report);
    stop(session);
    watched
}

fn keep(session: &mut Session, tunnel: &mut Tunnel, report: &mut dyn Report) -> Result<(), String> {
    const POLL: Duration = Duration::from_secs(3);

    let mut viewers = None;
    let mut started = Instant::now();
    while !report.stopped() {
        if let Some(why) = tunnel.ended() {
            return Err(why);
        }
        if started.elapsed() >= KEEP {
            // A refusal is news (screen.vnc turned off); a missed answer is
            // not, the next one will do.
            match start(session) {
                Ok(_) => started = Instant::now(),
                Err(err) if err.contains("screen.vnc") => return Err(err),
                Err(_) => {}
            }
        }
        if let Ok(status) = session.fetch::<api::device::Status>() {
            let now = status.vnc.map_or(0, |vnc| vnc.viewers);
            if viewers.is_some_and(|was| was != now) {
                report.line(announce(now));
            }
            viewers = Some(now);
        }
        let until = Instant::now() + POLL;
        while Instant::now() < until && !report.stopped() {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|line| {
                line.0
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_viewer_is_pointed_at_the_local_port_with_the_login() {
        let shown = text(&explain("lobby", 5901, "on", "Ctrl-C closes the tunnel"));
        assert!(shown.contains("localhost:5901"), "{shown}");
        assert!(shown.contains("tessaro / tessaro"), "{shown}");
        assert!(shown.contains("lobby"), "{shown}");
        assert!(shown.ends_with("Ctrl-C closes the tunnel"), "{shown}");
        assert!(!shown.contains("view only"), "{shown}");
    }

    #[test]
    fn view_only_is_said() {
        let shown = text(&explain(
            "lobby",
            5900,
            "view-only",
            "Cancel closes the tunnel",
        ));
        assert!(shown.contains("view only"), "{shown}");
    }

    #[test]
    fn viewers_are_counted_in_words() {
        assert_eq!(text(&[announce(0)]), "no viewer connected");
        assert_eq!(text(&[announce(1)]), "a viewer connected");
        assert_eq!(text(&[announce(3)]), "3 viewers connected");
    }
}
