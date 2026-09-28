//! Opening Webconfig, the device's own management pages, in a browser, for
//! `tessaro-ctl access webconfig` and the GUI's Access page alike
//! (docs/webconfig.md).
//!
//! The browser goes to the address this session reached the device at. A
//! claimed device wants a credential, and the token never goes into a URL:
//! the device issues a one-time ticket for it instead, which the page trades
//! for a session cookie and drops from the address. The ticket rides in the
//! fragment, which a browser never sends to any server.

use std::net::{IpAddr, SocketAddr};

use protocol::api;

use crate::connect::Session;

/// Where the browser goes, with a ticket when the device is claimed.
pub struct Address {
    /// The address with the ticket, for the browser.
    pub url: String,
    /// The same without it, to show.
    pub shown: String,
}

/// Webconfig's address for `session`'s device.
pub fn address(session: &mut Session) -> Result<Address, String> {
    let at = session.address().ok_or(
        "over the local socket there is no address a browser can open; \
         open Webconfig from another machine",
    )?;
    let shown = base(at)?;
    if !session.node.claimed {
        return Ok(Address {
            url: shown.clone(),
            shown,
        });
    }
    if !session.has_token() {
        return Err(format!(
            "{} is claimed and this machine has no token for it; \
             `tessaro-ctl access login` with one first",
            session.node.name
        ));
    }
    let ticket = session.call::<api::access::TicketCreate>(api::Empty {}, ())?;
    Ok(Address {
        url: with_ticket(&shown, &ticket.ticket),
        shown,
    })
}

/// `https://ADDRESS:PORT/`. A link-local IPv6 address needs its interface,
/// which no browser takes in a URL.
fn base(at: SocketAddr) -> Result<String, String> {
    if let IpAddr::V6(ip) = at.ip() {
        if (ip.segments()[0] & 0xffc0) == 0xfe80 {
            return Err(format!(
                "{} is a link-local address, which a browser cannot open; \
                 reach the device at another address",
                at.ip()
            ));
        }
    }
    // SocketAddr's own form puts an IPv6 address in brackets.
    Ok(format!("https://{at}/"))
}

fn with_ticket(base: &str, ticket: &str) -> String {
    format!("{base}#ticket={ticket}")
}

/// Open `url` in the default browser, without waiting for it. `$BROWSER`
/// wins when it is set.
pub fn open(url: &str) -> Result<(), String> {
    let mut command = match std::env::var("BROWSER") {
        Ok(browser) if !browser.trim().is_empty() => std::process::Command::new(browser.trim()),
        _ if cfg!(target_os = "macos") => std::process::Command::new("open"),
        _ if cfg!(windows) => {
            // `start` needs a shell and mangles `&`; explorer answers 1 even
            // when it worked. This opens the URL and exits 0.
            let mut command = std::process::Command::new("rundll32");
            command.arg("url.dll,FileProtocolHandler");
            command
        }
        _ => std::process::Command::new("xdg-open"),
    };
    command
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|err| format!("could not start a browser: {err}"))?;
    // Reaped on a thread of its own: a browser started in the foreground
    // does not return until it is closed.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_the_one_the_session_used() {
        assert_eq!(
            base("192.0.2.5:7400".parse().unwrap()).unwrap(),
            "https://192.0.2.5:7400/"
        );
        assert_eq!(
            base("[2001:db8::5]:17400".parse().unwrap()).unwrap(),
            "https://[2001:db8::5]:17400/"
        );
        assert!(base("[fe80::1]:7400".parse().unwrap()).is_err());
    }

    #[test]
    fn the_ticket_rides_in_the_fragment() {
        let url = with_ticket("https://192.0.2.5:7400/", "abc");
        assert_eq!(url, "https://192.0.2.5:7400/#ticket=abc");
    }
}
