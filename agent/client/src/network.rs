//! Changing a device's network, for `tessaro-ctl network` and `config set`
//! of a network key, and the GUI's Network and WiFi pages alike.
//!
//! A network change is a transaction on the device: applied, checked, and
//! kept or rolled back on its own (docs/networking.md). The answer can take
//! the device a minute and a half, and never come when the change moved the
//! link the request came in on; both are said the same way here.

use std::time::Duration;

use protocol::keys::{self, Consumer};
use protocol::{Applied, Secret, Verify, WifiNetwork, WifiSecurity};

use crate::connect::{Answer, Session};
use crate::text::{Line, Tone};

/// Longer than the device takes to apply, check and roll back a change
/// (about 90s at most), so its answer is waited for - and short enough that
/// a connection the change silently broke does not hang the client.
pub const CHANGE: Duration = Duration::from_secs(180);

/// Whether `key` is one of the device's network settings, whose change runs
/// as a verified network transaction.
pub fn is_network_key(key: &str) -> bool {
    keys::find(key).is_some_and(|key| key.consumers.contains(&Consumer::Network))
}

/// Send one network change with `request`, waiting as long as the device
/// may take for its verdict.
pub fn apply(
    session: &mut Session,
    request: impl FnOnce(&mut Session) -> Answer<Applied>,
) -> Answer<Applied> {
    session.set_read_timeout(Some(CHANGE));
    let answer = request(session);
    session.restore_read_timeout();
    answer
}

/// Said before the change is sent: what it does and what it has to pass.
pub fn notice(node: &str, doing: &str, verify: &Verify) -> Line {
    Line::of(
        Tone::Muted,
        format!(
            "{node}: {doing}; kept only if {} - this can take a minute...",
            verify.describe()
        ),
    )
}

/// Why no verdict came, when the connection went during the change.
pub fn lost(why: &str) -> String {
    format!(
        "lost the connection while the device applied the change ({why}). \
         That is expected when it moved the link this connection came in on: \
         the device keeps the change or rolls it back on its own."
    )
}

/// Where to see what the device did after `lost`.
pub fn lost_hint() -> Line {
    Line::plain("see what it did with: ").add(
        Tone::Cmd,
        "tessaro-ctl network last   (at the new address, if it changed)",
    )
}

/// Whether joining needs no password: said so, or the last scan saw the
/// network open. `seen` is the network in the last scan, if it was there.
pub fn wifi_open(security: Option<WifiSecurity>, seen: Option<&WifiNetwork>) -> bool {
    security == Some(WifiSecurity::Open) || seen.is_some_and(|network| network.security == "open")
}

/// Whether the device already has a password for the network, so an empty
/// one keeps it.
pub fn wifi_known(seen: Option<&WifiNetwork>) -> bool {
    seen.is_some_and(|network| network.known)
}

/// The password to send for a join: none for an open network, none when
/// it is left empty for a known one (the device keeps the saved one), and
/// otherwise one WPA takes.
pub fn wifi_psk(
    password: &str,
    security: Option<WifiSecurity>,
    seen: Option<&WifiNetwork>,
) -> Result<Option<Secret>, String> {
    if wifi_open(security, seen) || (password.is_empty() && wifi_known(seen)) {
        return Ok(None);
    }
    keys::check_psk(password)?;
    Ok(Some(Secret(password.to_string())))
}

/// `network.proxy.url` from what was typed: the URL, with a user and a
/// password put into it when they are given apart, checked as the device
/// checks it.
pub fn proxy_url(url: &str, user: &str, password: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("a URL, please; switching the proxy off stops using one".to_string());
    }
    let mut proxy = keys::parse_proxy(url).map_err(|err| format!("{}: {err}", keys::PROXY_URL))?;
    let user = user.trim();
    if user.is_empty() && password.is_empty() {
        return Ok(url.to_string());
    }
    if !user.is_empty() {
        proxy.user = Some(user.to_string());
    }
    if !password.is_empty() {
        if proxy.user.is_none() {
            return Err("a password needs a user".to_string());
        }
        proxy.password = Some(password.to_string());
    }
    let url = proxy.to_string();
    match keys::find(keys::PROXY_URL) {
        Some(key) => keys::validate(key, &url),
        None => Ok(url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(security: &str, known: bool) -> WifiNetwork {
        WifiNetwork {
            ssid: "cafe".into(),
            bssid: "00:11:22:33:44:55".into(),
            signal: 70,
            frequency_mhz: 2437,
            security: security.into(),
            known,
            active: false,
            interface: "wlan0".into(),
        }
    }

    #[test]
    fn a_known_network_keeps_its_saved_password() {
        let seen = network("wpa2", true);
        assert_eq!(wifi_psk("", None, Some(&seen)), Ok(None));
        assert!(wifi_psk("", None, Some(&network("wpa2", false))).is_err());
        assert!(wifi_psk("hunter2hunter2", None, Some(&seen))
            .unwrap()
            .is_some());
    }

    #[test]
    fn an_open_network_takes_no_password() {
        assert_eq!(wifi_psk("", None, Some(&network("open", false))), Ok(None));
        assert_eq!(wifi_psk("", Some(WifiSecurity::Open), None), Ok(None));
    }

    #[test]
    fn a_proxy_url_takes_its_user_apart() {
        assert_eq!(
            proxy_url("http://proxy.corp:3128", "", ""),
            Ok("http://proxy.corp:3128".to_string())
        );
        assert!(proxy_url("http://proxy.corp:3128", "", "secret").is_err());
        let with = proxy_url("http://proxy.corp:3128", "me", "secret").unwrap();
        assert!(with.contains("me:secret@"), "{with}");
    }
}
