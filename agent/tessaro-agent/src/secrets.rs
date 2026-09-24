//! The network's passwords: the hotspot's and the WiFi client's.
//!
//! In `/data/tessaro/secrets.json`, through the same store as `state.json`
//! and `auth.json` (0600, locked, fsynced), and never in `state.json`: `get`
//! and `keys` print every setting, and these must never be printed. They
//! reach one other place, the 0600 keyfiles under
//! `/run/NetworkManager/system-connections` that `nm::profiles` renders.
//!
//! The hotspot password follows the claim, like the root password: none
//! while the device is unclaimed (an open hotspot), a random one from the
//! claim on, none again after unclaim or a factory reset.

use protocol::Secret;
use serde::{Deserialize, Serialize};

pub const FILE: &str = "secrets.json";

/// Each password is a `Secret` from the command that brought it to the
/// keyfile that uses it, so no `{:?}` on the way prints one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotspot_psk: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wifi_psk: Option<Secret>,
}

/// A hotspot password: 16 characters without look-alikes, from the same
/// generator as the root password - long enough for WPA2, short enough to
/// type on a phone.
pub fn random_hotspot_psk() -> Result<String, String> {
    let password = crate::auth::random_password()?;
    Ok(password.chars().take(16).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_password_is_a_valid_passphrase() {
        let psk = random_hotspot_psk().unwrap();
        assert_eq!(psk.len(), 16);
        protocol::keys::check_psk(&psk).unwrap();
    }

    #[test]
    fn an_empty_store_has_no_passwords() {
        let secrets: Secrets = serde_json::from_str("{}").unwrap();
        assert_eq!(secrets, Secrets::default());
        assert_eq!(serde_json::to_string(&Secrets::default()).unwrap(), "{}");
    }

    #[test]
    fn the_file_holds_the_passwords_and_debug_never_shows_them() {
        let text = r#"{"hotspot_psk":"abcdefgh23456789","wifi_psk":"hunter2hunter2"}"#;
        let secrets: Secrets = serde_json::from_str(text).unwrap();
        assert_eq!(serde_json::to_string(&secrets).unwrap(), text);
        let shown = format!("{secrets:?}");
        assert!(
            !shown.contains("abcdefgh") && !shown.contains("hunter2"),
            "{shown}"
        );
    }
}
