//! MQTT: the device keeps a connection to a broker, publishes what it is
//! doing there and takes commands from it, and a client reaches it through
//! the broker instead of over IP. Where everything lives is `topic`, how a
//! request and its answer travel is `envelope`; docs/mqtt.md says how it
//! all behaves.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub mod envelope;
pub mod topic;

/// What `mqtt.commands` may be: none, the endpoints that only read (`GET`),
/// or every one.
pub const COMMANDS: &[&str] = &["off", "read", "full"];

/// What `mqtt.logs` may be: off, or the lowest journal priority published.
pub const LOG_LEVELS: &[&str] = &["off", "err", "warning", "notice", "info", "debug"];

/// The schemes `mqtt.url` may have, and the port each defaults to.
pub const SCHEMES: &[(&str, u16)] = &[("mqtt", 1883), ("mqtts", 8883), ("ws", 80), ("wss", 443)];

/// The most topic filters one script runs on.
pub const TRIGGERS_MAX: usize = 16;

/// A broker URL taken apart: `mqtts://broker.example:8883`,
/// `wss://broker.example/mqtt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokerUrl {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// The WebSocket path, `/mqtt`; `/` for a plain or TLS one.
    pub path: String,
}

impl BrokerUrl {
    pub fn tls(&self) -> bool {
        self.scheme == "mqtts" || self.scheme == "wss"
    }

    pub fn websocket(&self) -> bool {
        self.scheme == "ws" || self.scheme == "wss"
    }
}

impl std::fmt::Display for BrokerUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        write!(f, "{}://{host}:{}", self.scheme, self.port)?;
        if self.websocket() {
            write!(f, "{}", self.path)?;
        }
        Ok(())
    }
}

/// A broker URL, or why it is not one. No user or password in it: those
/// are `mqtt.username` and the secret `tessaro-ctl mqtt password set`
/// keeps, so the password never sits in the settings.
pub fn parse_url(url: &str) -> Result<BrokerUrl, String> {
    let url = url.trim();
    let (scheme, rest) = url
        .split_once("://")
        .ok_or("a broker URL is mqtt://, mqtts://, ws:// or wss://host[:port]")?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port = SCHEMES
        .iter()
        .find(|(name, _)| *name == scheme)
        .map(|(_, port)| *port)
        .ok_or_else(|| format!("{scheme}:// is not a broker URL; mqtt, mqtts, ws or wss"))?;
    let (authority, path) = match rest.find(['/', '?', '#']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if authority.contains('@') {
        return Err("no user or password in the URL: set mqtt.username, and the password with `tessaro-ctl mqtt password set`".into());
    }
    if path.contains(['?', '#']) {
        return Err("a broker URL has no query or fragment".into());
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest
            .split_once(']')
            .ok_or("an IPv6 address is closed with ]")?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err("the broker URL has no host".into());
    }
    if !host
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | ':'))
    {
        return Err(format!("{host:?} is not a host name or address"));
    }
    let port = match port {
        None => default_port,
        Some(port) => match port.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => return Err(format!("{port:?} is not a port")),
        },
    };
    let ws = scheme == "ws" || scheme == "wss";
    if !ws && !path.is_empty() && path != "/" {
        return Err(format!(
            "{scheme}:// has no path; a WebSocket broker is ws:// or wss://"
        ));
    }
    if path.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return Err("the path has a space or a control character".into());
    }
    let path = if !ws || path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };
    Ok(BrokerUrl {
        scheme,
        host: host.to_ascii_lowercase(),
        port,
        path,
    })
}

/// The topic filters of a script's MQTT trigger, as they are stored: each
/// checked, none twice. An empty list runs on nothing.
pub fn triggers(filters: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for filter in filters {
        let filter = filter.trim();
        topic::check_filter(filter)?;
        if !out.iter().any(|known| known == filter) {
            out.push(filter.to_string());
        }
    }
    if out.len() > TRIGGERS_MAX {
        return Err(format!(
            "a script runs on at most {TRIGGERS_MAX} topic filters"
        ));
    }
    Ok(out)
}

/// Whether a script with these filters runs on a message to `topic`.
pub fn runs_on(filters: &[String], topic: &str) -> bool {
    filters.iter().any(|filter| topic::matches(filter, topic))
}

/// How the device's link to its broker stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum MqttState {
    /// mqtt.url is empty.
    Off,
    Connecting,
    Connected,
    /// The last attempt failed; another follows. `last_error` says why.
    Failed,
}

/// `tessaro-ctl mqtt status`: the link, where the device publishes, and
/// what it has done since the agent started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MqttStatus {
    pub state: MqttState,
    /// mqtt.url as the device uses it, port filled in; `None` when off.
    #[serde(default)]
    pub broker: Option<String>,
    /// Always the device id.
    pub client_id: String,
    /// mqtt.username filled in; `None` for none.
    #[serde(default)]
    pub username: Option<String>,
    /// Whether a password is kept for the broker.
    pub password: bool,
    /// Whether a client certificate is kept for the broker.
    pub client_cert: bool,
    /// Where the device's topics are: `<prefix>/<device.id>`.
    pub topic: String,
    /// mqtt.commands.
    pub commands: String,
    /// Seconds in the present state.
    #[serde(default)]
    pub seconds: Option<u64>,
    /// Why the last attempt or the connection failed.
    #[serde(default)]
    pub last_error: Option<String>,
    /// The filters the device is subscribed to.
    #[serde(default)]
    pub subscriptions: Vec<String>,
    /// Events waiting for the broker while the link is down.
    #[serde(default)]
    pub queued: u64,
    #[serde(default)]
    pub published: u64,
    #[serde(default)]
    pub received: u64,
    /// What the operator should know: a plain link taking every command.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// What `tessaro-ctl mqtt test` found: whether the broker took a
/// connection with the settings, and where it stopped if not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MqttTested {
    pub broker: String,
    pub ok: bool,
    /// The step that failed, and why: `dns`, `connect`, `proxy`, `tls`,
    /// `websocket` or `mqtt`.
    #[serde(default)]
    pub stage: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// The broker certificate's SHA-256, for mqtt.tls.fingerprint.
    #[serde(default)]
    pub fingerprint: Option<String>,
    pub millis: u64,
}

/// `tessaro-ctl mqtt test`: the saved settings, or another URL with them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MqttTestBody {
    #[serde(default)]
    pub url: Option<String>,
}

/// The broker password, or `None` to forget it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MqttPasswordBody {
    #[serde(default)]
    pub password: Option<crate::Secret>,
}

/// A client certificate and its private key, both PEM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MqttCertBody {
    pub cert: String,
    pub key: crate::Secret,
}

/// Something the device did or saw, as the broker gets it on
/// `<prefix>/<id>/event/<source>/<name>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    /// Counts up from 1 since the agent started, so a gap shows something
    /// was lost; the device clock may be wrong, so this is the order.
    pub seq: u64,
    pub source: String,
    pub name: String,
    /// What happened, by source: the same as the page's event for the same
    /// thing where there is one.
    #[serde(default)]
    pub detail: serde_json::Value,
}

/// `<prefix>/<id>/status`, retained. The will is `{"online": false}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Presence {
    pub online: bool,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub machine: Option<String>,
    /// The device's TLS certificate, for a client that goes on to reach it
    /// over IP.
    #[serde(default)]
    pub fingerprint: Option<String>,
    #[serde(default)]
    pub claimed: Option<bool>,
    #[serde(default)]
    pub commands: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broker_urls_fill_in_the_port() {
        let url = parse_url("mqtts://Broker.Example").unwrap();
        assert_eq!(
            (url.scheme.as_str(), url.host.as_str(), url.port),
            ("mqtts", "broker.example", 8883)
        );
        assert!(url.tls() && !url.websocket());
        assert_eq!(url.to_string(), "mqtts://broker.example:8883");
        assert_eq!(
            parse_url("mqtt://10.0.2.2").unwrap().to_string(),
            "mqtt://10.0.2.2:1883"
        );
        assert_eq!(parse_url("mqtt://10.0.2.2:1884/").unwrap().port, 1884);
        let ws = parse_url("wss://b.example/mqtt").unwrap();
        assert_eq!((ws.port, ws.path.as_str()), (443, "/mqtt"));
        assert_eq!(ws.to_string(), "wss://b.example:443/mqtt");
        assert_eq!(parse_url("ws://b.example:9001").unwrap().path, "/");
        assert_eq!(parse_url("mqtt://[fd00::1]:1883").unwrap().host, "fd00::1");
    }

    #[test]
    fn broker_urls_refuse_credentials_and_nonsense() {
        assert!(parse_url("mqtt://user:pw@b")
            .unwrap_err()
            .contains("mqtt.username"));
        assert!(parse_url("http://b").is_err());
        assert!(parse_url("b:1883").is_err());
        assert!(parse_url("mqtt://").is_err());
        assert!(parse_url("mqtt://b:0").is_err());
        assert!(parse_url("mqtt://b:99999").is_err());
        assert!(parse_url("mqtt://b/path").is_err());
        assert!(parse_url("wss://b/mqtt?x=1").is_err());
        assert!(parse_url("mqtt://b c").is_err());
    }

    #[test]
    fn script_triggers_are_filters() {
        let stored = triggers(&[
            "kiosk/+/open".into(),
            " kiosk/+/open".into(),
            "alarm/#".into(),
        ])
        .unwrap();
        assert_eq!(
            stored,
            vec!["kiosk/+/open".to_string(), "alarm/#".to_string()]
        );
        assert!(triggers(&["a/#/b".into()]).is_err());
        assert!(runs_on(&stored, "kiosk/lobby/open"));
        assert!(runs_on(&stored, "alarm/fire/2"));
        assert!(!runs_on(&stored, "kiosk/lobby/close"));
        let many: Vec<String> = (0..=TRIGGERS_MAX).map(|i| format!("t/{i}")).collect();
        assert!(triggers(&many).is_err());
    }
}
