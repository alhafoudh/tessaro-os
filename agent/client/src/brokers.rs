//! The MQTT brokers this client reaches devices through: the `brokers`
//! table of this client's `tessaro.db` (`store.rs`), each by a name of the
//! user's (`tessaro-ctl nodes brokers add NAME URL`), which `--mqtt @NAME`
//! names it by. `--mqtt` also takes a URL as it is, for a broker that is
//! not kept.
//!
//! Like the nodes, every change writes its one row; the password and the
//! keys stay in the 0600 store.

use tessaro_db::rusqlite::{params, Connection};

use crate::store;

/// One broker, as kept or as `--mqtt URL` gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Broker {
    /// `@NAME` names it; empty for one given as a URL.
    pub name: String,
    /// `mqtt://`, `mqtts://`, `ws://` or `wss://`, without a user.
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    /// The devices' mqtt.prefix: where their topics are.
    pub prefix: String,
    /// A certificate authority to trust for the broker, PEM, beside the
    /// system's.
    pub ca: Option<String>,
    /// A client certificate and its key, PEM.
    pub client_cert: Option<String>,
    pub client_key: Option<String>,
}

impl Broker {
    /// How the user calls it: `@NAME`, or its URL.
    pub fn label(&self) -> String {
        match self.name.is_empty() {
            true => self.url.clone(),
            false => format!("@{}", self.name),
        }
    }
}

#[derive(Debug, Default)]
pub struct Brokers {
    pub brokers: Vec<Broker>,
}

/// A broker's name: what `@NAME` can say.
pub fn check_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    match valid {
        true => Ok(()),
        false => Err(format!(
            "{name:?} is not a broker name: letters, digits, - _ and ., at most 40"
        )),
    }
}

/// What `--mqtt` stands for: `@NAME` for a kept broker, or a URL, which may
/// carry `user:password@` for a broker that is not kept.
pub fn resolve(spec: &str, brokers: &Brokers) -> Result<Broker, String> {
    let spec = spec.trim();
    if let Some(name) = spec.strip_prefix('@') {
        return brokers.by_name(name).cloned().ok_or_else(|| {
            format!("no broker named {name}; `tessaro-ctl nodes brokers list` lists them, `nodes brokers add` keeps one")
        });
    }
    let (url, username, password) = split_credentials(spec)?;
    protocol::mqtt::parse_url(&url)?;
    Ok(Broker {
        name: String::new(),
        url,
        username,
        password,
        prefix: std::env::var("TESSARO_MQTT_PREFIX")
            .ok()
            .filter(|prefix| !prefix.is_empty())
            .unwrap_or_else(|| protocol::mqtt::topic::DEFAULT_PREFIX.to_string()),
        ca: None,
        client_cert: None,
        client_key: None,
    })
}

/// A URL's `user[:password]@`, percent-decoded, taken out of it.
pub fn split_credentials(url: &str) -> Result<(String, Option<String>, Option<String>), String> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Ok((url.to_string(), None, None));
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return Ok((url.to_string(), None, None));
    };
    let (user, password) = match userinfo.split_once(':') {
        Some((user, password)) => (decode(user)?, Some(decode(password)?)),
        None => (decode(userinfo)?, None),
    };
    Ok((format!("{scheme}://{host}{path}"), Some(user), password))
}

fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = text
                .get(at + 1..at + 3)
                .ok_or("a % in the URL is not followed by two hex digits")?;
            out.push(
                u8::from_str_radix(hex, 16)
                    .map_err(|_| format!("%{hex} in the URL is not a byte"))?,
            );
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "the URL's user or password is not UTF-8".to_string())
}

fn store_row(db: &Connection, broker: &Broker) -> Result<(), String> {
    db.execute(
        "INSERT INTO brokers (name, url, username, password, prefix, ca, client_cert, client_key) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT (name) DO UPDATE SET url = excluded.url, username = excluded.username, \
         password = excluded.password, prefix = excluded.prefix, ca = excluded.ca, \
         client_cert = excluded.client_cert, client_key = excluded.client_key",
        params![
            broker.name,
            broker.url,
            broker.username,
            broker.password,
            broker.prefix,
            broker.ca,
            broker.client_cert,
            broker.client_key
        ],
    )
    .map(drop)
    .map_err(|err| format!("{}: {err}", store::path().display()))
}

impl Brokers {
    /// Every kept broker; none, and no store made, when there is no store.
    pub fn load() -> Result<Self, String> {
        match store::open_existing()? {
            Some(db) => Self::load_from(&db),
            None => Ok(Self::default()),
        }
    }

    fn load_from(db: &Connection) -> Result<Self, String> {
        let fail = |err: tessaro_db::rusqlite::Error| format!("{}: {err}", store::path().display());
        let mut rows = db
            .prepare(
                "SELECT name, url, username, password, prefix, ca, client_cert, client_key \
                 FROM brokers ORDER BY name",
            )
            .map_err(fail)?;
        let brokers = rows
            .query_map([], |row| {
                Ok(Broker {
                    name: row.get(0)?,
                    url: row.get(1)?,
                    username: row.get(2)?,
                    password: row.get(3)?,
                    prefix: row.get(4)?,
                    ca: row.get(5)?,
                    client_cert: row.get(6)?,
                    client_key: row.get(7)?,
                })
            })
            .map_err(fail)?
            .collect::<Result<_, _>>()
            .map_err(fail)?;
        Ok(Self { brokers })
    }

    pub fn by_name(&self, name: &str) -> Option<&Broker> {
        self.brokers.iter().find(|broker| broker.name == name)
    }

    /// Keep `broker`, replacing one of its name.
    pub fn put(&mut self, broker: Broker) -> Result<(), String> {
        check_name(&broker.name)?;
        protocol::mqtt::parse_url(&broker.url)?;
        protocol::mqtt::topic::check_prefix(&broker.prefix)?;
        store_row(&store::open()?, &broker)?;
        match self
            .brokers
            .iter_mut()
            .find(|known| known.name == broker.name)
        {
            Some(known) => *known = broker,
            None => self.brokers.push(broker),
        }
        Ok(())
    }

    /// Forget the broker named `name`; false when there was none.
    pub fn remove(&mut self, name: &str) -> Result<bool, String> {
        let Some(at) = self.brokers.iter().position(|broker| broker.name == name) else {
            return Ok(false);
        };
        store::open()?
            .execute("DELETE FROM brokers WHERE name = ?1", params![name])
            .map_err(|err| format!("{}: {err}", store::path().display()))?;
        self.brokers.remove(at);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_may_carry_its_user_and_password() {
        assert_eq!(
            split_credentials("mqtts://kiosk:p%24ss@b.example:8883/x").unwrap(),
            (
                "mqtts://b.example:8883/x".to_string(),
                Some("kiosk".to_string()),
                Some("p$ss".to_string())
            )
        );
        assert_eq!(
            split_credentials("mqtt://b.example").unwrap(),
            ("mqtt://b.example".to_string(), None, None)
        );
        assert_eq!(
            split_credentials("ws://only@b").unwrap(),
            ("ws://b".to_string(), Some("only".to_string()), None)
        );
        assert!(split_credentials("mqtt://a:%zz@b").is_err());
    }

    #[test]
    fn mqtt_names_a_kept_broker_or_is_a_url() {
        let brokers = Brokers {
            brokers: vec![Broker {
                name: "venue1".into(),
                url: "mqtts://b.example".into(),
                username: None,
                password: None,
                prefix: "acme".into(),
                ca: None,
                client_cert: None,
                client_key: None,
            }],
        };
        assert_eq!(resolve("@venue1", &brokers).unwrap().prefix, "acme");
        assert!(resolve("@nope", &brokers)
            .unwrap_err()
            .contains("nodes brokers list"));
        let given = resolve("mqtt://u:p@10.0.2.2:1883", &brokers).unwrap();
        assert_eq!(
            (given.url.as_str(), given.username.as_deref()),
            ("mqtt://10.0.2.2:1883", Some("u"))
        );
        assert_eq!(given.label(), "mqtt://10.0.2.2:1883");
        assert!(resolve("http://b", &brokers).is_err());
    }

    #[test]
    fn broker_names_are_plain() {
        assert!(check_name("venue-1.eu").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("a b").is_err());
    }
}
