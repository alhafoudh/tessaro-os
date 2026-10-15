//! Reaching devices through their MQTT broker instead of over IP: the link
//! to a broker (`Link`), asking one device something on its command topic
//! and waiting for the answer on this client's own reply topic
//! (`protocol::mqtt::envelope` says how both travel), and listing the
//! devices whose status the broker keeps (`Link::scan`). A `Session` over
//! MQTT asks through here (`connect.rs`), so every command works the same
//! either way.
//!
//! Blocking, like the rest of this crate: the socket reads with a short
//! timeout, and between reads the session's timers - the keep-alive - get
//! their turn. The broker is trusted: there is no certificate of the
//! device's to pin, and no token to send - whoever may publish to a
//! device's topics has it (docs/mqtt.md).

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use protocol::mqtt::topic;
use protocol::mqtt::{BrokerUrl, Presence};
use tessaro_mqtt::{Event, Message, Options, Qos, Session};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::Message as Frame;

use crate::brokers::Broker;
use crate::connect::Stream;

/// The TCP connect, the TLS and WebSocket handshakes, and the broker's
/// answer to the CONNECT and the SUBSCRIBE: each.
const CONNECT: Duration = Duration::from_secs(10);
/// One read waits this long at most, so the timers get their turn.
const POLL: Duration = Duration::from_millis(200);
/// Seconds between pings while the link is quiet.
const KEEP_ALIVE: u16 = 30;
/// How long the broker keeps a request for a device that is away.
const REQUEST_EXPIRY: u32 = 60;
/// How long a scan listens for the statuses the broker keeps: it sends them
/// the moment the subscription is taken.
pub const SCAN: Duration = Duration::from_millis(1500);

/// The device `start` stands for among `seen`: the one with that exact
/// name or id, else the one whose name or id starts with it. The start of
/// several, or of none, is an error naming the broker.
pub fn pick<'a>(start: &str, seen: &'a [Seen], broker: &str) -> Result<&'a Seen, String> {
    let name = |seen: &Seen| seen.presence.name.clone().unwrap_or_default();
    if let Some(exact) = seen
        .iter()
        .find(|seen| seen.id == start || name(seen) == start)
    {
        return Ok(exact);
    }
    let starts: Vec<&Seen> = seen
        .iter()
        .filter(|seen| seen.id.starts_with(start) || name(seen).starts_with(start))
        .collect();
    match &starts[..] {
        [only] => Ok(only),
        [] => Err(format!(
            "{start}: no device by that name or id at {broker}; `tessaro-ctl nodes list --mqtt {broker}` lists them"
        )),
        several => Err(format!(
            "{start}: more than one device at {broker} starts with that: {}",
            several.iter().map(|seen| name(seen)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The open link to a broker.
pub struct Link {
    transport: Transport,
    session: Session,
    prefix: String,
    /// This client's reply topic, `<prefix>/client/<random>/reply`.
    reply: String,
    /// Correlation data of the next request.
    next: u64,
    /// How the user named the broker, for messages.
    label: String,
}

/// A device whose status the broker keeps.
#[derive(Debug, Clone)]
pub struct Seen {
    pub id: String,
    pub presence: Presence,
}

enum Transport {
    Stream(Box<dyn Stream>),
    Socket(Box<tungstenite::WebSocket<Box<dyn Stream>>>),
}

impl Transport {
    /// What arrived within `POLL`; empty when nothing did.
    fn recv(&mut self) -> Result<Vec<u8>, String> {
        match self {
            Transport::Stream(stream) => {
                let mut buffer = vec![0u8; 64 * 1024];
                match stream.read(&mut buffer) {
                    Ok(0) => Err("the broker closed the connection".to_string()),
                    Ok(read) => {
                        buffer.truncate(read);
                        Ok(buffer)
                    }
                    Err(err) if quiet(&err) => Ok(Vec::new()),
                    Err(err) => Err(format!("reading from the broker: {err}")),
                }
            }
            Transport::Socket(socket) => match socket.read() {
                Ok(Frame::Binary(bytes)) => Ok(bytes.to_vec()),
                Ok(Frame::Close(_)) => Err("the broker closed the connection".to_string()),
                Ok(_) => Ok(Vec::new()),
                Err(tungstenite::Error::Io(err)) if quiet(&err) => Ok(Vec::new()),
                Err(err) => Err(format!("reading from the broker: {err}")),
            },
        }
    }

    fn send(&mut self, bytes: Vec<u8>) -> Result<(), String> {
        let sent = match self {
            Transport::Stream(stream) => stream
                .write_all(&bytes)
                .and_then(|()| stream.flush())
                .map_err(|err| err.to_string()),
            Transport::Socket(socket) => socket
                .send(Frame::Binary(bytes.into()))
                .map_err(|err| err.to_string()),
        };
        sent.map_err(|err| format!("writing to the broker: {err}"))
    }
}

/// A read that only timed out.
fn quiet(err: &std::io::Error) -> bool {
    matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

impl Link {
    /// Connect to `broker`; `client` is how this program names itself, in
    /// its client id.
    pub fn connect(broker: &Broker, client: &str) -> Result<Link, String> {
        let url = protocol::mqtt::parse_url(&broker.url)?;
        let label = broker.label();
        let transport = open(&url, broker).map_err(|err| format!("{label}: {err}"))?;
        let random = random_hex();
        let mut link = Link {
            transport,
            session: Session::new(),
            prefix: broker.prefix.clone(),
            reply: topic::reply(&broker.prefix, &random),
            next: 1,
            label: label.clone(),
        };
        let options = Options {
            client_id: format!("{client}-{random}"),
            username: broker.username.clone(),
            password: broker.password.clone().map(String::into_bytes),
            keep_alive: KEEP_ALIVE,
            will: None,
        };
        link.session.connect(&options, Instant::now())?;
        let deadline = Instant::now() + CONNECT;
        link.pump(deadline, |event| match event {
            Event::Connected => Some(Ok(())),
            Event::Refused(why) => Some(Err(format!("the broker refused the connection: {why}"))),
            _ => None,
        })
        .and_then(|outcome| {
            outcome.unwrap_or_else(|| Err("the broker did not answer the CONNECT".to_string()))
        })
        .map_err(|err| format!("{label}: {err}"))?;
        let reply = link.reply.clone();
        link.subscribe(&reply)?;
        Ok(link)
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    fn subscribe(&mut self, filter: &str) -> Result<(), String> {
        let id = self
            .session
            .subscribe(&[(filter.to_string(), Qos::AtLeastOnce)], Instant::now())?;
        let deadline = Instant::now() + CONNECT;
        let label = self.label.clone();
        self.pump(deadline, |event| match event {
            Event::Subscribed {
                id: answered,
                refused,
            } if *answered == id => Some(match refused.first() {
                Some(refusal) => Err(format!("the broker refused a subscription to {refusal}")),
                None => Ok(()),
            }),
            _ => None,
        })?
        .unwrap_or_else(|| Err("the broker did not answer the SUBSCRIBE".to_string()))
        .map_err(|err| format!("{label}: {err}"))
    }

    /// Ask device `id` for endpoint `group`/`name` (`protocol::api::by_name`)
    /// and wait up to `timeout` for its answer.
    pub fn request(
        &mut self,
        id: &str,
        group: &str,
        name: &str,
        payload: Vec<u8>,
        user: Vec<(String, String)>,
        timeout: Duration,
    ) -> Result<Message, String> {
        let correlation = self.next.to_be_bytes().to_vec();
        self.next += 1;
        let mut message = Message::new(
            topic::device_command(&self.prefix, id, group, name),
            payload,
        );
        message.response_topic = Some(self.reply.clone());
        message.correlation = Some(correlation.clone());
        message.expiry = Some(REQUEST_EXPIRY);
        message.user = user;
        self.session.publish(&message, Instant::now())?;
        let reply = self.reply.clone();
        let answer = self.pump(Instant::now() + timeout, |event| match event {
            Event::Message(message)
                if message.topic == reply
                    && message.correlation.as_deref() == Some(&correlation) =>
            {
                Some(message.clone())
            }
            _ => None,
        })?;
        answer.ok_or_else(|| {
            format!(
                "no answer through {} within {}s; is the device connected to it? `tessaro-ctl nodes list --mqtt {}` shows",
                self.label,
                timeout.as_secs(),
                self.label
            )
        })
    }

    /// Every device the broker keeps a status for under this prefix, as it
    /// says within `wait`: a broker sends what it retains at once.
    pub fn scan(&mut self, wait: Duration) -> Result<Vec<Seen>, String> {
        let filter = topic::statuses(&self.prefix);
        self.subscribe(&filter)?;
        let mut seen: Vec<Seen> = Vec::new();
        let prefix = self.prefix.clone();
        self.pump(Instant::now() + wait, |event| {
            if let Event::Message(message) = event {
                if let Some(id) = topic::status_device(&prefix, &message.topic) {
                    if let Ok(presence) = serde_json::from_slice::<Presence>(&message.payload) {
                        seen.retain(|known| known.id != id);
                        seen.push(Seen {
                            id: id.to_string(),
                            presence,
                        });
                    }
                }
            }
            None::<()>
        })?;
        self.session.unsubscribe(&[filter], Instant::now())?;
        self.flush()?;
        seen.sort_by(|a, b| a.presence.name.cmp(&b.presence.name).then(a.id.cmp(&b.id)));
        Ok(seen)
    }

    /// A polite goodbye.
    pub fn close(mut self) {
        self.session.disconnect(Instant::now());
        let _ = self.flush();
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.session.has_output() {
            let bytes = self.session.output();
            self.transport.send(bytes)?;
        }
        Ok(())
    }

    /// Move bytes and fire timers until `take` makes something of an event,
    /// or `deadline`: what it made, `None` at the deadline.
    fn pump<T>(
        &mut self,
        deadline: Instant,
        mut take: impl FnMut(&Event) -> Option<T>,
    ) -> Result<Option<T>, String> {
        loop {
            self.flush()?;
            while let Some(event) = self.session.event() {
                if let Event::Closed(why) = &event {
                    return Err(format!("{}: {why}", self.label));
                }
                if let Some(made) = take(&event) {
                    return Ok(Some(made));
                }
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            if self.session.next_timer().is_some_and(|at| at <= now) {
                self.session.fire_timers(now);
                continue;
            }
            let bytes = self
                .transport
                .recv()
                .map_err(|err| format!("{}: {err}", self.label))?;
            if !bytes.is_empty() {
                self.session.received(&bytes, Instant::now());
            }
        }
    }
}

/// The bytes under the session: TCP, TLS over it, a WebSocket over either.
fn open(url: &BrokerUrl, broker: &Broker) -> Result<Transport, String> {
    let address = (url.host.as_str(), url.port)
        .to_socket_addrs()
        .map_err(|err| format!("{}: {err}", url.host))?
        .next()
        .ok_or_else(|| format!("{} has no address", url.host))?;
    let tcp =
        TcpStream::connect_timeout(&address, CONNECT).map_err(|err| format!("{address}: {err}"))?;
    let _ = tcp.set_nodelay(true);
    tcp.set_read_timeout(Some(CONNECT))
        .and_then(|()| tcp.set_write_timeout(Some(CONNECT)))
        .map_err(|err| err.to_string())?;
    let control = tcp.try_clone().map_err(|err| err.to_string())?;

    let stream: Box<dyn Stream> = if url.tls() {
        let mut builder = native_tls::TlsConnector::builder();
        if let Some(ca) = &broker.ca {
            for pem in split_pem(ca) {
                let cert = native_tls::Certificate::from_pem(pem.as_bytes())
                    .map_err(|err| format!("the broker's CA: {err}"))?;
                builder.add_root_certificate(cert);
            }
        }
        if let (Some(cert), Some(key)) = (&broker.client_cert, &broker.client_key) {
            let identity = native_tls::Identity::from_pkcs8(cert.as_bytes(), key.as_bytes())
                .map_err(|err| format!("the client certificate: {err}"))?;
            builder.identity(identity);
        }
        let connector = builder.build().map_err(|err| err.to_string())?;
        Box::new(
            connector
                .connect(&url.host, tcp)
                .map_err(|err| tls_error(&err.to_string()))?,
        )
    } else {
        Box::new(tcp)
    };

    let transport = if url.websocket() {
        let host = if url.host.contains(':') {
            format!("[{}]", url.host)
        } else {
            url.host.clone()
        };
        let mut request = format!("ws://{host}:{}{}", url.port, url.path)
            .into_client_request()
            .map_err(|err| err.to_string())?;
        request
            .headers_mut()
            .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("mqtt"));
        let (socket, _) = tungstenite::client::client(request, stream)
            .map_err(|err| format!("WebSocket: {err}"))?;
        Transport::Socket(Box::new(socket))
    } else {
        Transport::Stream(stream)
    };
    control
        .set_read_timeout(Some(POLL))
        .map_err(|err| err.to_string())?;
    Ok(transport)
}

/// The certificates in a PEM bundle, one each.
fn split_pem(bundle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in bundle.lines() {
        current.push_str(line);
        current.push('\n');
        if line.starts_with("-----END") {
            out.push(std::mem::take(&mut current));
        }
    }
    out
}

fn tls_error(error: &str) -> String {
    if error.contains("certificate verify failed") || error.contains("not trusted") {
        format!("TLS: the broker's certificate is not trusted ({error}); keep its CA with `tessaro-ctl nodes brokers add ... --ca FILE`")
    } else {
        format!("TLS: {error}")
    }
}

fn random_hex() -> String {
    // Not secret, only unlikely to be taken: the time and this process.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();
    let mixed = nanos ^ (u128::from(std::process::id()) << 64);
    protocol::hex(&mixed.to_be_bytes()[4..])
}

/// A device's answer as the HTTP API would have given it.
pub fn reply(message: Message) -> crate::http::Reply {
    let status =
        match protocol::mqtt::envelope::property(&message.user, protocol::mqtt::envelope::STATUS) {
            Some(protocol::mqtt::envelope::OK) => 200,
            Some(code) => serde_json::from_value::<protocol::api::ErrorCode>(
                serde_json::Value::String(code.to_string()),
            )
            .map(|code| code.status())
            .unwrap_or(422),
            None => 502,
        };
    let mut headers: Vec<(String, String)> = message
        .user
        .iter()
        .filter(|(name, _)| name.starts_with("x-tessaro-"))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect();
    if let Some(content_type) = &message.content_type {
        headers.push(("content-type".to_string(), content_type.clone()));
    }
    crate::http::Reply {
        status,
        headers,
        body: message.payload,
        close: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: &str) -> Message {
        Message::new(
            "tessaro/client/x/reply",
            br#"{"error":"no","code":"not-found"}"#.to_vec(),
        )
        .user("status", status)
        .user("device", "ab")
        .user("x-tessaro-size", "3")
        .content_type("application/json")
    }

    #[test]
    fn an_answer_reads_like_the_http_one() {
        let ok = reply(answer("ok"));
        assert_eq!(ok.status, 200);
        assert!(ok.headers.contains(&("x-tessaro-size".into(), "3".into())));
        assert!(ok
            .headers
            .contains(&("content-type".into(), "application/json".into())));
        assert!(!ok.headers.iter().any(|(name, _)| name == "device"));
        assert_eq!(reply(answer("not-found")).status, 404);
        assert_eq!(reply(answer("refused")).status, 422);
        assert_eq!(reply(Message::new("t", b"".to_vec())).status, 502);
    }

    fn seen(id: &str, name: &str) -> Seen {
        Seen {
            id: id.to_string(),
            presence: Presence {
                online: true,
                id: Some(id.to_string()),
                name: Some(name.to_string()),
                tags: Vec::new(),
                version: None,
                machine: None,
                fingerprint: None,
                claimed: None,
                commands: None,
            },
        }
    }

    #[test]
    fn a_device_is_picked_by_its_name_its_id_or_a_start() {
        let all = [
            seen("1a2b", "lobby"),
            seen("3c4d", "lobby-2"),
            seen("5e6f", "cafe"),
        ];
        assert_eq!(pick("lobby", &all, "@b").unwrap().id, "1a2b");
        assert_eq!(pick("3c4d", &all, "@b").unwrap().id, "3c4d");
        assert_eq!(pick("caf", &all, "@b").unwrap().id, "5e6f");
        assert_eq!(pick("5e", &all, "@b").unwrap().id, "5e6f");
        assert!(pick("lob", &all, "@b")
            .unwrap_err()
            .contains("lobby, lobby-2"));
        assert!(pick("zz", &all, "@b")
            .unwrap_err()
            .contains("nodes list --mqtt @b"));
    }

    #[test]
    fn a_bundle_splits_into_its_certificates() {
        let bundle = "-----BEGIN CERTIFICATE-----\nAA\n-----END CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\nBB\n-----END CERTIFICATE-----\n";
        let parts = split_pem(bundle);
        assert_eq!(parts.len(), 2);
        assert!(parts[1].contains("BB"));
    }
}
