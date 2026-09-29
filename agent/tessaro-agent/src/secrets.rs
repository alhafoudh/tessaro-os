//! The network's passwords: the hotspot's and the WiFi client's.
//!
//! In the `secrets` table of `tessaro.db` (0600, like the rest of it), and
//! never in `settings`: `get` and `keys` print every setting, and these must
//! never be printed. They reach one other place, the 0600 keyfiles under
//! `/run/NetworkManager/system-connections` that `nm::profiles` renders.
//!
//! The hotspot password follows the claim, like the root password: none
//! while the device is unclaimed (an open hotspot), a random one from the
//! claim on, none again after unclaim or a factory reset.

use protocol::Secret;
use tessaro_db::rusqlite::{params, Connection};

use crate::db::Stored;

const HOTSPOT_PSK: &str = "hotspot_psk";
const WIFI_PSK: &str = "wifi_psk";

/// Each password is a `Secret` from the command that brought it to the
/// keyfile that uses it, so no `{:?}` on the way prints one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Secrets {
    pub hotspot_psk: Option<Secret>,
    pub wifi_psk: Option<Secret>,
}

impl Stored for Secrets {
    const WHAT: &'static str = "the network passwords";

    fn load(db: &Connection) -> tessaro_db::rusqlite::Result<Self> {
        let mut secrets = Self::default();
        let mut rows = db.prepare("SELECT name, value FROM secrets")?;
        for row in rows.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (name, value): (String, String) = row?;
            match name.as_str() {
                HOTSPOT_PSK => secrets.hotspot_psk = Some(Secret(value)),
                WIFI_PSK => secrets.wifi_psk = Some(Secret(value)),
                _ => {}
            }
        }
        Ok(secrets)
    }

    fn save(&self, db: &Connection) -> tessaro_db::rusqlite::Result<()> {
        db.execute("DELETE FROM secrets", [])?;
        let mut insert = db.prepare("INSERT INTO secrets (name, value) VALUES (?1, ?2)")?;
        for (name, value) in [(HOTSPOT_PSK, &self.hotspot_psk), (WIFI_PSK, &self.wifi_psk)] {
            if let Some(value) = value {
                insert.execute(params![name, value.expose()])?;
            }
        }
        Ok(())
    }

    fn clear(db: &Connection) -> tessaro_db::rusqlite::Result<()> {
        db.execute("DELETE FROM secrets", []).map(drop)
    }
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
    fn the_store_holds_the_passwords_and_debug_never_shows_them() {
        let dir = tempfile::tempdir().unwrap();
        let log = crate::log::Log::buffered(true);
        let db = crate::db::Db::open(dir.path(), &log);
        assert_eq!(db.read::<Secrets>(&log), Secrets::default());

        let saved = Secrets {
            hotspot_psk: Some(Secret("abcdefgh23456789".into())),
            wifi_psk: Some(Secret("hunter2hunter2".into())),
        };
        db.update(|secrets: &mut Secrets| {
            *secrets = saved.clone();
            Ok(())
        })
        .unwrap();

        let secrets: Secrets = db.read(&log);
        assert_eq!(secrets, saved);
        let shown = format!("{secrets:?}");
        assert!(
            !shown.contains("abcdefgh") && !shown.contains("hunter2"),
            "{shown}"
        );
    }
}
