//! Finding a device and opening a conversation with it.
//!
//! `--node` takes the local socket (nothing, or `local`), an IP, `ip:port`,
//! a name, `name.local`, or a DNS host name. A bare name or `.local` is
//! looked up with mDNS, falling back to the address last seen for it.
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

use mdns_sd::{ServiceDaemon, ServiceEvent};
use protocol::{from_line, to_line, Command, Frame, Hello, NodeInfo, Request, PROTOCOL_VERSION};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::nodes::{Node, Nodes};

const CONNECT: Duration = Duration::from_secs(5);
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
        let known = nodes.by_name(name);
        if let Some(found) = browse(BROWSE).into_iter().find(|found| {
            found.name == name
                || (known.is_some() && found.id.as_deref() == known.map(|k| k.id.as_str()))
        }) {
            let address = SocketAddr::new(found.address.ip(), port.unwrap_or(found.address.port()));
            return Ok(Target::Remote {
                address,
                expected: found.id.or_else(|| known.map(|k| k.id.clone())),
                label: name.to_string(),
            });
        }
        if let Some(known) = known {
            let address: SocketAddr = known
                .address
                .parse()
                .map_err(|_| format!("{}: bad stored address {}", known.name, known.address))?;
            eprintln!("{name}: not answering on mDNS, trying its last address {address}");
            return Ok(Target::Remote {
                address,
                expected: Some(known.id.clone()),
                label: name.to_string(),
            });
        }
        return Err(format!(
            "{name}: not found on the network (mDNS) and not a known node"
        ));
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
}

pub fn open(target: &Target, nodes: &Nodes, trust: Trust, follow: bool) -> Result<Session, String> {
    match target {
        Target::Local(path) => open_local(path),
        Target::Remote {
            address,
            expected,
            label,
        } => open_remote(*address, expected.as_deref(), label, nodes, trust, follow),
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

fn open_remote(
    address: SocketAddr,
    expected: Option<&str>,
    label: &str,
    nodes: &Nodes,
    trust: Trust,
    follow: bool,
) -> Result<Session, String> {
    let tcp =
        TcpStream::connect_timeout(&address, CONNECT).map_err(|err| format!("{address}: {err}"))?;
    tcp.set_read_timeout(if follow { None } else { Some(IO) })
        .and_then(|()| tcp.set_write_timeout(Some(IO)))
        .map_err(|err| err.to_string())?;

    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|err| err.to_string())?;
    let tls = connector
        .connect("tessaro", tcp)
        .map_err(|err| format!("{address}: TLS: {err}"))?;

    let der = tls
        .peer_certificate()
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("{address}: presented no certificate"))?
        .to_der()
        .map_err(|err| err.to_string())?;
    let fingerprint = hex(&Sha256::digest(&der));

    // The hello carries nothing secret, and the welcome says which node this
    // claims to be - so a known device that DHCP moved is still recognised.
    // The pin is checked against *that* node before any token is sent.
    let mut session = handshake(Box::new(tls), None, None)?;
    let known: Option<&Node> = nodes
        .by_id(&session.node.id)
        .or_else(|| expected.and_then(|id| nodes.by_id(id)));
    match known {
        Some(node) if node.fingerprint != fingerprint => {
            return Err(format!(
                "{label} ({address}) presented certificate {fingerprint},\n\
                 but {} is pinned to {}.\n\
                 This is either a different device or someone in the middle. If the device\n\
                 was reinstalled or its /data wiped, `tessaro-ctl forget {}` and pin it again.",
                node.name, node.fingerprint, node.name
            ));
        }
        Some(_) => {}
        None => match trust {
            Trust::KnownOnly => {
                return Err(format!(
                    "{label} ({address}) is not a known node; `tessaro-ctl claim` or `tessaro-ctl login` it first"
                ))
            }
            Trust::Peek => {}
            Trust::Pin { assume_yes } => {
                eprintln!("{label} ({address}) presents certificate\n  {fingerprint}");
                if !assume_yes && !ask("Pin it and continue?")? {
                    return Err("not pinned".to_string());
                }
            }
        },
    }

    if session.node.fingerprint != fingerprint {
        return Err(format!(
            "{label}: says its fingerprint is {} but presented {fingerprint}",
            session.node.fingerprint
        ));
    }
    if let Some(node) = known {
        if node.id != session.node.id {
            return Err(format!(
                "{label}: pinned as node {} but answers as {}",
                node.id, session.node.id
            ));
        }
    }
    session.token = std::env::var("TESSARO_TOKEN")
        .ok()
        .or_else(|| known.and_then(|node| node.token.clone()));
    session.remote = Some((address, fingerprint));
    Ok(session)
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
        let id = self.send(command)?;
        loop {
            match read_frame(&mut self.stream)? {
                Frame::Ok { id: got, result } if got == id => return Ok(result),
                Frame::Error { id: got, error } if got == id || got == 0 => return Err(error),
                _ => continue,
            }
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
    eprint!("{question} [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|err| err.to_string())?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}
