//! Finding a device and opening a conversation with it.
//!
//! `--node` takes the local socket (nothing, or `local`), an IP, `ip:port`,
//! a name, `name.local`, or a DNS host name. A bare name or `.local` goes to
//! the address last seen for it first, and to an mDNS scan only when nothing
//! answers there or a different device does (`open_named`).
//!
//! Over TCP the certificate is never *verified* - every device is
//! self-signed - it is **pinned**: its SHA-256 is compared with the one stored
//! for that node id before a token is sent, and a mismatch is a hard stop.
//! (The hello and the welcome go first; neither carries anything secret.)
//! A node that is not known yet can only be asked `id`, or be claimed or
//! logged into, which is where its fingerprint is shown and pinned.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anstream::{eprint, eprintln};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use protocol::{from_line, to_line, Command, Frame, Hello, NodeInfo, Request, PROTOCOL_VERSION};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::nodes::{Node, Nodes};
use crate::style::{self, paint};

const CONNECT: Duration = Duration::from_secs(5);
/// A cached address gets less: on a LAN a live device answers in
/// milliseconds, and a silent one should cost little before the scan.
const CACHED_CONNECT: Duration = Duration::from_secs(2);
const IO: Duration = Duration::from_secs(60);
pub const BROWSE: Duration = Duration::from_secs(3);

pub trait Stream: Read + Write {}
impl<T: Read + Write> Stream for T {}

#[derive(Debug, Clone)]
pub enum Target {
    Local(PathBuf),
    Remote {
        address: SocketAddr,
        /// The node this address is believed to be, if known.
        expected: Option<String>,
        /// How the user named it, for messages and for a new entry.
        label: String,
    },
    /// `NAME` or `NAME.local`: the cached address if there is one, then mDNS.
    Named {
        name: String,
        port: Option<u16>,
        known: Option<Node>,
    },
}

/// A device seen on the network.
#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub address: SocketAddr,
    pub id: Option<String>,
    pub fingerprint: Option<String>,
    pub claimed: Option<bool>,
}

pub fn resolve(node: Option<&str>, nodes: &Nodes) -> Result<Target, String> {
    let socket = || {
        PathBuf::from(
            std::env::var("TESSARO_SOCKET")
                .unwrap_or_else(|_| protocol::DEFAULT_SOCKET.to_string()),
        )
    };
    let Some(node) = node.filter(|node| !node.is_empty() && *node != "local") else {
        return Ok(Target::Local(socket()));
    };

    let remote = |address: SocketAddr, label: &str| {
        let expected = nodes
            .by_address(&address.to_string())
            .map(|known| known.id.clone());
        Target::Remote {
            address,
            expected,
            label: label.to_string(),
        }
    };

    if let Ok(address) = node.parse::<SocketAddr>() {
        return Ok(remote(address, node));
    }
    if let Ok(ip) = node.parse::<IpAddr>() {
        return Ok(remote(SocketAddr::new(ip, protocol::DEFAULT_PORT), node));
    }

    let (host, port) = match node.rsplit_once(':') {
        Some((host, port)) => (
            host,
            Some(
                port.parse::<u16>()
                    .map_err(|_| format!("{node}: bad port"))?,
            ),
        ),
        None => (node, None),
    };

    let mdns_name = host
        .strip_suffix(".local")
        .or((!host.contains('.')).then_some(host));
    if let Some(name) = mdns_name {
        // Resolved when opened: the cached address first, then a scan.
        return Ok(Target::Named {
            name: name.to_string(),
            port,
            known: nodes.by_name(name).cloned(),
        });
    }

    let address = (host, port.unwrap_or(protocol::DEFAULT_PORT))
        .to_socket_addrs()
        .map_err(|err| format!("{host}: {err}"))?
        .next()
        .ok_or_else(|| format!("{host}: no address"))?;
    Ok(remote(address, node))
}

/// Every `_tessaro._tcp` device that answers within `wait`.
pub fn browse(wait: Duration) -> Vec<Found> {
    let Ok(daemon) = ServiceDaemon::new() else {
        return Vec::new();
    };
    let Ok(events) = daemon.browse(protocol::SERVICE_TYPE) else {
        return Vec::new();
    };

    let deadline = Instant::now() + wait;
    let mut found: Vec<Found> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        let ServiceEvent::ServiceResolved(service) = event else {
            continue;
        };
        let Some(ip) = service.get_addresses_v4().into_iter().next() else {
            continue;
        };
        let name = service
            .get_fullname()
            .strip_suffix(&format!(".{}", protocol::SERVICE_TYPE))
            .unwrap_or(service.get_fullname())
            .to_string();
        let txt = |key: &str| service.get_property_val_str(key).map(str::to_string);

        if !found.iter().any(|known| known.name == name) {
            found.push(Found {
                name,
                address: SocketAddr::new(IpAddr::V4(ip), service.get_port()),
                id: txt("id"),
                fingerprint: txt("fp"),
                claimed: txt("claimed").map(|value| value == "1"),
            });
        }
    }
    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// What the caller wants to happen if the node has no pin yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// Refuse: only known nodes.
    KnownOnly,
    /// Allowed without pinning (`id`): nothing secret is sent.
    Peek,
    /// Pin it, after the user accepted the fingerprint (`claim`, `login`).
    Pin { assume_yes: bool },
}

pub struct Session {
    stream: BufReader<Box<dyn Stream>>,
    next_id: u64,
    token: Option<String>,
    pub node: NodeInfo,
    /// Set when this session should be (re)stored in nodes.json.
    pub remote: Option<(SocketAddr, String)>,
    /// The TCP socket under the TLS, kept to change its read timeout.
    tcp: Option<TcpStream>,
    /// How long the TCP connect and the TLS handshake plus the welcome took.
    pub timing: Option<Timing>,
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub connect: Duration,
    pub handshake: Duration,
}

/// How one command went.
pub enum Answer {
    Ok(Value),
    /// The device answered with an error.
    Refused(String),
    /// No answer came: the connection broke or timed out.
    Lost(String),
}

pub fn open(target: &Target, nodes: &Nodes, trust: Trust, follow: bool) -> Result<Session, String> {
    match target {
        Target::Local(path) => open_local(path),
        Target::Remote {
            address,
            expected,
            label,
        } => open_remote(
            *address,
            expected.as_deref(),
            label,
            nodes,
            trust,
            follow,
            CONNECT,
        )
        .map_err(Failure::message),
        Target::Named { name, port, known } => {
            open_named(name, *port, known.as_ref(), nodes, trust, follow)
        }
    }
}

#[cfg(unix)]
fn open_local(path: &std::path::Path) -> Result<Session, String> {
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|err| {
        format!(
            "{}: {err} (on the device, as root; elsewhere pass --node)",
            path.display()
        )
    })?;
    handshake(Box::new(stream), None, None)
}

#[cfg(not(unix))]
fn open_local(_: &std::path::Path) -> Result<Session, String> {
    Err("no local socket on this platform; pass --node".to_string())
}

/// Why a remote conversation did not open. The difference matters for a
/// named node's cached address: nothing there means "it moved, look for it",
/// something else there means the same - but worth a warning.
#[derive(Debug)]
enum Failure {
    /// Nothing answered, or not with TLS.
    Unreachable(String),
    /// Something answered, but not the device that is pinned: another
    /// certificate, or another node id.
    Mismatch(String),
    /// The right device, or an unknown one, and it went wrong anyway.
    Refused(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Failure::Unreachable(why) | Failure::Mismatch(why) | Failure::Refused(why) => why,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn open_remote(
    address: SocketAddr,
    expected: Option<&str>,
    label: &str,
    nodes: &Nodes,
    trust: Trust,
    follow: bool,
    connect: Duration,
) -> Result<Session, Failure> {
    let started = Instant::now();
    let tcp = TcpStream::connect_timeout(&address, connect)
        .map_err(|err| Failure::Unreachable(format!("{address}: {err}")))?;
    let connected = started.elapsed();
    tcp.set_read_timeout(if follow { None } else { Some(IO) })
        .and_then(|()| tcp.set_write_timeout(Some(IO)))
        .map_err(|err| Failure::Refused(err.to_string()))?;
    let control = tcp.try_clone().ok();

    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|err| Failure::Refused(err.to_string()))?;
    let tls = connector
        .connect("tessaro", tcp)
        .map_err(|err| Failure::Unreachable(format!("{address}: TLS: {err}")))?;

    let der = tls
        .peer_certificate()
        .map_err(|err| Failure::Refused(err.to_string()))?
        .ok_or_else(|| Failure::Unreachable(format!("{address}: presented no certificate")))?
        .to_der()
        .map_err(|err| Failure::Refused(err.to_string()))?;
    let fingerprint = hex(&Sha256::digest(&der));

    // The hello carries nothing secret, and the welcome says which node this
    // claims to be. The pin is checked before any token is sent.
    let mut session = handshake(Box::new(tls), None, None)
        .map_err(|why| Failure::Unreachable(format!("{address}: {why}")))?;
    session.tcp = control;
    session.timing = Some(Timing {
        connect: connected,
        handshake: started.elapsed().saturating_sub(connected),
    });

    // The node we meant, if we meant one. Only without an expectation - an
    // IP typed by hand - is the answering device looked up by its own id,
    // so a known device that DHCP moved is still recognised. With one, the
    // expected pin comes first: another known device answering at that
    // address must be a mismatch, not a silent switch to the wrong kiosk.
    let known: Option<&Node> = match expected.and_then(|id| nodes.by_id(id)) {
        Some(node) => Some(node),
        None => nodes.by_id(&session.node.id),
    };
    match known {
        Some(node) if node.id != session.node.id => {
            return Err(Failure::Mismatch(format!(
                "{label}: a different device answers at {address} (node {} {}, not {} {})",
                session.node.name, session.node.id, node.name, node.id
            )));
        }
        Some(node) if node.fingerprint != fingerprint => {
            return Err(Failure::Mismatch(format!(
                "{label} ({address}) presented certificate {fingerprint},\n\
                 but {} is pinned to {}.\n\
                 This is either a different device or someone in the middle. If the device\n\
                 was reinstalled or its /data wiped, `tessaro-ctl nodes forget {}` and pin it again.",
                node.name, node.fingerprint, node.name
            )));
        }
        Some(_) => {}
        None => match trust {
            Trust::KnownOnly => {
                return Err(Failure::Refused(format!(
                    "{label} ({address}) is not a known node; `tessaro-ctl access claim` or `tessaro-ctl access login` it first"
                )))
            }
            Trust::Peek => {}
            Trust::Pin { assume_yes } => {
                eprintln!(
                    "{label} ({address}) presents certificate\n  {}",
                    paint(style::HEADING, &fingerprint)
                );
                if !assume_yes && !ask("Pin it and continue?").map_err(Failure::Refused)? {
                    return Err(Failure::Refused("not pinned".to_string()));
                }
            }
        },
    }

    if session.node.fingerprint != fingerprint {
        return Err(Failure::Mismatch(format!(
            "{label}: says its fingerprint is {} but presented {fingerprint}",
            session.node.fingerprint
        )));
    }
    session.token = std::env::var("TESSARO_TOKEN")
        .ok()
        .or_else(|| known.and_then(|node| node.token.clone()));
    session.remote = Some((address, fingerprint));
    Ok(session)
}

/// A node asked for by name: its cached address first, then mDNS.
///
/// * The cached address answers as the pinned device: done, no scan at all.
/// * Something else answers there - another certificate, another node id -
///   warn, then scan: the device most likely moved and its old address went
///   to someone else. The scan result is held to the pin as strictly.
/// * Nothing answers there: scan, quietly.
fn open_named(
    name: &str,
    port: Option<u16>,
    known: Option<&Node>,
    nodes: &Nodes,
    trust: Trust,
    follow: bool,
) -> Result<Session, String> {
    let with_port =
        |address: SocketAddr| SocketAddr::new(address.ip(), port.unwrap_or(address.port()));

    let cached = known.and_then(|node| {
        node.address
            .parse::<SocketAddr>()
            .ok()
            .map(|a| (node, with_port(a)))
    });
    // What happened at the cached address, for the final error.
    let mut at_cached = "not answering at";
    if let Some((node, address)) = cached {
        match open_remote(
            address,
            Some(&node.id),
            name,
            nodes,
            trust,
            follow,
            CACHED_CONNECT,
        ) {
            Ok(session) => return Ok(session),
            Err(Failure::Mismatch(why)) => {
                let warning = paint(style::WARN, "warning:");
                eprintln!("{warning} {why}\n{warning} looking for {name} on the network instead");
                at_cached = "another device now at";
            }
            Err(Failure::Unreachable(_)) => {}
            Err(Failure::Refused(why)) => return Err(why),
        }
    }

    let found = browse(BROWSE).into_iter().find(|found| {
        found.name == name
            || known.is_some_and(|node| found.id.as_deref() == Some(node.id.as_str()))
    });
    match found {
        Some(found) => {
            let expected = known.map(|node| node.id.clone()).or(found.id);
            open_remote(
                with_port(found.address),
                expected.as_deref(),
                name,
                nodes,
                trust,
                follow,
                CONNECT,
            )
            .map_err(Failure::message)
        }
        None => Err(match cached {
            Some((_, address)) => format!(
                "{name}: {at_cached} its last address {address}, and not found on the network (mDNS)"
            ),
            None => format!("{name}: not found on the network (mDNS) and not a known node"),
        }),
    }
}

fn handshake(
    stream: Box<dyn Stream>,
    token: Option<String>,
    remote: Option<(SocketAddr, String)>,
) -> Result<Session, String> {
    let mut stream = BufReader::new(stream);
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        client: format!("tessaro-ctl {}", env!("CARGO_PKG_VERSION")),
    };
    write_line(&mut stream, &to_line(&hello))?;

    match read_frame(&mut stream)? {
        Frame::Welcome { node, .. } => Ok(Session {
            stream,
            next_id: 1,
            token,
            node,
            remote,
            tcp: None,
            timing: None,
        }),
        Frame::Error { error, .. } => Err(error),
        other => Err(format!("unexpected greeting: {other:?}")),
    }
}

impl Session {
    pub fn set_token(&mut self, token: String) {
        self.token = Some(token);
    }

    pub fn clear_token(&mut self) {
        self.token = None;
    }

    /// One command, one answer.
    pub fn call(&mut self, command: Command) -> Result<Value, String> {
        match self.request(command) {
            Answer::Ok(value) => Ok(value),
            Answer::Refused(error) | Answer::Lost(error) => Err(error),
        }
    }

    /// One command, one answer - and whether a missing answer was the
    /// device's doing or the connection's.
    pub fn request(&mut self, command: Command) -> Answer {
        let id = match self.send(command) {
            Ok(id) => id,
            Err(error) => return Answer::Lost(error),
        };
        loop {
            match read_frame(&mut self.stream) {
                Ok(Frame::Ok { id: got, result }) if got == id => return Answer::Ok(result),
                Ok(Frame::Error { id: got, error }) if got == id || got == 0 => {
                    return Answer::Refused(error)
                }
                Ok(_) => continue,
                Err(error) => return Answer::Lost(error),
            }
        }
    }

    /// How long a TCP session waits for an answer from now on. `None` waits
    /// for good; the local socket never times out.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) {
        if let Some(tcp) = &self.tcp {
            let _ = tcp.set_read_timeout(timeout);
        }
    }

    /// A streaming command: `each` gets every event until the end.
    pub fn stream(&mut self, command: Command, mut each: impl FnMut(Value)) -> Result<(), String> {
        let id = self.send(command)?;
        loop {
            match read_frame(&mut self.stream)? {
                Frame::Event { id: got, event } if got == id => each(event),
                Frame::End { id: got } if got == id => return Ok(()),
                Frame::Error { id: got, error } if got == id || got == 0 => return Err(error),
                _ => continue,
            }
        }
    }

    fn send(&mut self, command: Command) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request {
            id,
            token: self.token.clone(),
            command,
        };
        write_line(&mut self.stream, &to_line(&request))?;
        Ok(id)
    }
}

fn write_line(stream: &mut BufReader<Box<dyn Stream>>, line: &str) -> Result<(), String> {
    let inner = stream.get_mut();
    inner
        .write_all(line.as_bytes())
        .and_then(|()| inner.flush())
        .map_err(|err| format!("sending: {err}"))
}

fn read_frame(stream: &mut BufReader<Box<dyn Stream>>) -> Result<Frame, String> {
    let mut line = String::new();
    let read = stream
        .by_ref()
        .take(protocol::MAX_LINE as u64 + 1)
        .read_line(&mut line)
        .map_err(|err| format!("receiving: {err}"))?;
    if read == 0 {
        return Err("the device closed the connection".to_string());
    }
    from_line(&line)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn ask(question: &str) -> Result<bool, String> {
    eprint!("{question} {} ", paint(style::LABEL, "[y/N]"));
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|err| err.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}
