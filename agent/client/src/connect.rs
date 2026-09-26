//! Finding a device and opening a session with it.
//!
//! `--node` takes the local socket (nothing, or `local`), an IP, `ip:port`,
//! a name, `name.local`, or a DNS host name. A bare name or `.local` goes to
//! the address last seen for it first, and to an mDNS scan only when nothing
//! answers there or a different device does (`open_named`).
//!
//! Over TCP the certificate is never *verified* - every device is
//! self-signed - it is **pinned**: its SHA-256 is compared with the one stored
//! for that node id before a token is sent, and a mismatch is a hard stop.
//! (`GET /api/v1/device/id` goes first, without a token; it carries nothing
//! secret.) A node that is not known yet can only be asked who it is, or be
//! claimed or logged into, which is where its fingerprint is shown and
//! pinned. Every later connection of the session must present the same
//! certificate.
//!
//! A session speaks the HTTP API (docs/api.md) one request at a time, each
//! call typed by its endpoint (`protocol::api`), over one connection it
//! opens again when the device has closed it.

use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};
use protocol::api::{self, jobs, ApiError, Blob, Endpoint, JobQuery, JobRef, LogsQuery};
use protocol::{hex, JobStarted, NodeInfo};
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::http::{self, Broken, Reply};
use crate::nodes::{Node, Nodes};

const CONNECT: Duration = Duration::from_secs(5);
/// A cached address gets less: on a LAN a live device answers in
/// milliseconds, and a silent one should cost little before the scan.
const CACHED_CONNECT: Duration = Duration::from_secs(2);
const IO: Duration = Duration::from_secs(60);
pub const BROWSE: Duration = Duration::from_secs(3);
/// A connection idle this long, or open this long, is opened again before
/// the next request rather than trusted: a NAT may have forgotten it, and
/// the device closes every connection after ten minutes.
const IDLE: Duration = Duration::from_secs(60);
const LIFE: Duration = Duration::from_secs(8 * 60);
/// Between two looks at a job, and at a followed journal.
const JOB_POLL: Duration = Duration::from_millis(400);
const LOG_POLL: Duration = Duration::from_secs(1);

/// `Send`, so a session can live on a thread of its own (the GUI's workers).
pub trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

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
        let Some(service) = found_service(&service) else {
            continue;
        };
        if !found.iter().any(|known| known.name == service.name) {
            found.push(service);
        }
    }
    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// A resolved `_tessaro._tcp` announcement, as a device seen on the network.
/// `None` without an IPv4 address: the API listens on v4 only.
pub fn found_service(service: &ResolvedService) -> Option<Found> {
    let ip = service.get_addresses_v4().into_iter().next()?;
    let txt = |key: &str| service.get_property_val_str(key).map(str::to_string);
    Some(Found {
        name: instance_name(service.get_fullname()),
        address: SocketAddr::new(IpAddr::V4(ip), service.get_port()),
        id: txt("id"),
        fingerprint: txt("fp"),
        claimed: txt("claimed").map(|value| value == "1"),
    })
}

/// The node name in an mDNS full name: `kiosk-1._tessaro._tcp.local.` is
/// `kiosk-1`. `ServiceRemoved` carries only the full name.
pub fn instance_name(fullname: &str) -> String {
    fullname
        .strip_suffix(&format!(".{}", protocol::SERVICE_TYPE))
        .unwrap_or(fullname)
        .to_string()
}

/// A certificate seen for the first time, for the caller to accept or not.
#[derive(Debug, Clone)]
pub struct PinAsk {
    /// How the user named the node.
    pub label: String,
    pub address: SocketAddr,
    /// SHA-256 of the certificate, hex.
    pub fingerprint: String,
}

/// Whether to pin: `Ok(true)` pins and goes on, `Ok(false)` stops.
pub type Decide<'a> = &'a mut dyn FnMut(&PinAsk) -> Result<bool, String>;

/// What the caller wants to happen if the node has no pin yet.
pub enum Trust<'a> {
    /// Only known nodes, and unclaimed ones, which are not pinned.
    KnownOnly,
    /// Allowed without pinning (`id`): nothing secret is sent.
    Peek,
    /// Pin it, if `decide` accepts the fingerprint (`claim`, `login`).
    Pin(Decide<'a>),
}

/// Where a session's connections go.
enum Dial {
    Local(PathBuf),
    /// The address, and the certificate every connection must present.
    Remote {
        address: SocketAddr,
        fingerprint: String,
    },
}

/// One open connection.
struct Conn {
    stream: BufReader<Box<dyn Stream>>,
    /// The TCP socket under the TLS, kept to change its read timeout and to
    /// shut it down from another thread.
    tcp: Option<TcpStream>,
    opened: Instant,
    used: Instant,
}

pub struct Session {
    conn: Option<Conn>,
    dial: Dial,
    /// How this program names itself to the device, for its journal.
    agent: String,
    token: Option<String>,
    read_timeout: Option<Duration>,
    pub node: NodeInfo,
    /// Set when this session should be (re)stored in nodes.json.
    pub remote: Option<(SocketAddr, String)>,
    /// How long the TCP connect took, and the TLS handshake plus asking the
    /// device who it is.
    pub timing: Option<Timing>,
    /// Worth telling the user, though the session opened: a pinned device
    /// that was not at its last address, an unclaimed one left unpinned.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub connect: Duration,
    pub handshake: Duration,
}

/// How one request went.
pub enum Answer<T> {
    Ok(T),
    /// The device answered with an error.
    Refused(String),
    /// No answer came: the connection broke or timed out.
    Lost(String),
}

impl<T> Answer<T> {
    pub fn into_result(self) -> Result<T, String> {
        match self {
            Answer::Ok(value) => Ok(value),
            Answer::Refused(error) | Answer::Lost(error) => Err(error),
        }
    }
}

/// A raw answer: a download or a screenshot.
pub struct Download {
    /// Names in lower case.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Download {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Open a session with `target`. `client` is how this program names itself
/// to the device (`tessaro-ctl 1.0.0`), for its journal.
pub fn open(
    target: &Target,
    nodes: &Nodes,
    trust: &mut Trust,
    client: &str,
) -> Result<Session, String> {
    match target {
        Target::Local(path) => open_local(path, client),
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
            client,
            CONNECT,
        )
        .map_err(Failure::message),
        Target::Named { name, port, known } => {
            open_named(name, *port, known.as_ref(), nodes, trust, client)
        }
    }
}

fn open_local(path: &std::path::Path, client: &str) -> Result<Session, String> {
    let conn = dial_local(path)?;
    let mut session = Session::new(Dial::Local(path.to_path_buf()), conn, client);
    session.node = session.identify()?;
    Ok(session)
}

#[cfg(unix)]
fn dial_local(path: &std::path::Path) -> Result<Conn, String> {
    let stream = std::os::unix::net::UnixStream::connect(path).map_err(|err| {
        format!(
            "{}: {err} (on the device, as root; elsewhere pass --node)",
            path.display()
        )
    })?;
    Ok(Conn::new(Box::new(stream), None))
}

#[cfg(not(unix))]
fn dial_local(_: &std::path::Path) -> Result<Conn, String> {
    Err("no local socket on this platform; pass --node".to_string())
}

/// Why a remote session did not open. The difference matters for a named
/// node's cached address: nothing there means "it moved, look for it",
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

/// A TLS connection to `address`, and the SHA-256 of the certificate it
/// presented. Nothing is sent over it yet.
fn dial_remote(address: SocketAddr, connect: Duration) -> Result<(Conn, String), Failure> {
    let tcp = TcpStream::connect_timeout(&address, connect)
        .map_err(|err| Failure::Unreachable(format!("{address}: {err}")))?;
    tcp.set_read_timeout(Some(IO))
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
    Ok((Conn::new(Box::new(tls), control), fingerprint))
}

fn open_remote(
    address: SocketAddr,
    expected: Option<&str>,
    label: &str,
    nodes: &Nodes,
    trust: &mut Trust,
    client: &str,
    connect: Duration,
) -> Result<Session, Failure> {
    let started = Instant::now();
    let (conn, fingerprint) = dial_remote(address, connect)?;
    let connected = started.elapsed();

    // Asking who it is carries nothing secret, and the answer says which
    // node this claims to be. The pin is checked before any token is sent.
    let dial = Dial::Remote {
        address,
        fingerprint: fingerprint.clone(),
    };
    let mut session = Session::new(dial, conn, client);
    session.node = session
        .identify()
        .map_err(|why| Failure::Unreachable(format!("{address}: {why}")))?;
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
            // An unclaimed device answers everything without a token, and no
            // token is sent to it: talk to it without pinning.
            Trust::KnownOnly if !session.node.claimed => {
                session.notes.push(format!(
                    "{} is unclaimed and not pinned; `tessaro-ctl access claim` it to keep it",
                    session.node.name
                ));
            }
            Trust::KnownOnly => {
                return Err(Failure::Refused(format!(
                    "{label} ({address}) is not a known node; `tessaro-ctl access claim` or `tessaro-ctl access login` it first"
                )))
            }
            Trust::Peek => {}
            Trust::Pin(decide) => {
                let ask = PinAsk {
                    label: label.to_string(),
                    address,
                    fingerprint: fingerprint.clone(),
                };
                if !decide(&ask).map_err(Failure::Refused)? {
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
    trust: &mut Trust,
    client: &str,
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
    let mut notes = Vec::new();
    if let Some((node, address)) = cached {
        match open_remote(
            address,
            Some(&node.id),
            name,
            nodes,
            trust,
            client,
            CACHED_CONNECT,
        ) {
            Ok(session) => return Ok(session),
            Err(Failure::Mismatch(why)) => {
                notes.push(why);
                notes.push(format!("looking for {name} on the network instead"));
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
            let mut session = open_remote(
                with_port(found.address),
                expected.as_deref(),
                name,
                nodes,
                trust,
                client,
                CONNECT,
            )
            .map_err(Failure::message)?;
            notes.append(&mut session.notes);
            session.notes = notes;
            Ok(session)
        }
        None => Err(match cached {
            Some((_, address)) => format!(
                "{name}: {at_cached} its last address {address}, and not found on the network (mDNS)"
            ),
            None => format!("{name}: not found on the network (mDNS) and not a known node"),
        }),
    }
}

impl Conn {
    fn new(stream: Box<dyn Stream>, tcp: Option<TcpStream>) -> Self {
        let now = Instant::now();
        Self {
            stream: BufReader::new(stream),
            tcp,
            opened: now,
            used: now,
        }
    }

    fn stale(&self) -> bool {
        self.used.elapsed() > IDLE || self.opened.elapsed() > LIFE
    }
}

impl Session {
    fn new(dial: Dial, conn: Conn, client: &str) -> Self {
        Self {
            conn: Some(conn),
            dial,
            agent: client.to_string(),
            token: None,
            read_timeout: Some(IO),
            node: NodeInfo {
                id: String::new(),
                name: String::new(),
                version: String::new(),
                machine: String::new(),
                fingerprint: String::new(),
                claimed: false,
            },
            remote: None,
            timing: None,
            notes: Vec::new(),
        }
    }

    fn identify(&mut self) -> Result<NodeInfo, String> {
        self.fetch::<api::device::Id>()
    }

    pub fn set_token(&mut self, token: String) {
        self.token = Some(token);
    }

    pub fn clear_token(&mut self) {
        self.token = None;
    }

    /// Whether a token goes with every request.
    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    /// One request, its answer as the endpoint's type.
    pub fn call<E: Endpoint>(
        &mut self,
        params: E::Params,
        body: E::Body,
    ) -> Result<E::Response, String> {
        self.request::<E>(params, body).into_result()
    }

    /// `call` for an endpoint that takes neither a query nor a body.
    pub fn fetch<E: Endpoint<Body = ()>>(&mut self) -> Result<E::Response, String>
    where
        E::Params: Default,
    {
        self.call::<E>(E::Params::default(), ())
    }

    /// `call` for an endpoint that takes only a body.
    pub fn send<E: Endpoint>(&mut self, body: E::Body) -> Result<E::Response, String>
    where
        E::Params: Default,
    {
        self.call::<E>(E::Params::default(), body)
    }

    /// One request - and whether a missing answer was the device's doing or
    /// the connection's. An answer of the wrong shape counts as refused: the
    /// device did answer.
    pub fn request<E: Endpoint>(
        &mut self,
        params: E::Params,
        body: E::Body,
    ) -> Answer<E::Response> {
        let target = match api::target::<E>(&params) {
            Ok(target) => target,
            Err(error) => return Answer::Refused(error),
        };
        let body = match serde_json::to_value(&body) {
            Ok(Value::Null) => Vec::new(),
            Ok(value) => value.to_string().into_bytes(),
            Err(err) => return Answer::Refused(err.to_string()),
        };
        let content = (!body.is_empty()).then_some("application/json");
        match self.exchange(E::METHOD.as_str(), &target, content, &body) {
            Ok(reply) => json_answer(reply),
            Err(error) => Answer::Lost(error),
        }
    }

    /// A piece of an upload, as the body of a `RAW_BODY` endpoint.
    pub fn upload<E: Endpoint<Body = Blob>>(
        &mut self,
        params: E::Params,
        data: &[u8],
    ) -> Answer<E::Response> {
        let target = match api::target::<E>(&params) {
            Ok(target) => target,
            Err(error) => return Answer::Refused(error),
        };
        match self.exchange(
            E::METHOD.as_str(),
            &target,
            Some("application/octet-stream"),
            data,
        ) {
            Ok(reply) => json_answer(reply),
            Err(error) => Answer::Lost(error),
        }
    }

    /// The bytes a `RAW_RESPONSE` endpoint answers with.
    pub fn download<E: Endpoint<Body = ()>>(&mut self, params: E::Params) -> Answer<Download> {
        let target = match api::target::<E>(&params) {
            Ok(target) => target,
            Err(error) => return Answer::Refused(error),
        };
        match self.exchange(E::METHOD.as_str(), &target, None, &[]) {
            Ok(reply) if (200..300).contains(&reply.status) => Answer::Ok(Download {
                headers: reply.headers,
                body: reply.body,
            }),
            Ok(reply) => Answer::Refused(refusal(&reply)),
            Err(error) => Answer::Lost(error),
        }
    }

    /// Start a job and follow it to its end: `each` gets every step as `T`,
    /// or one this client does not know from a newer device as it came.
    /// `stop` is asked between looks; when it says so the job is cancelled.
    pub fn job<S, T>(
        &mut self,
        body: S::Body,
        stop: &dyn Fn() -> bool,
        mut each: impl FnMut(Result<T, Value>),
    ) -> Result<(), String>
    where
        S: Endpoint<Response = JobStarted>,
        S::Params: Default,
        T: DeserializeOwned,
    {
        let JobStarted { job } = self.send::<S>(body)?;
        let mut after = 0;
        loop {
            if stop() {
                let _ = self.call::<jobs::Cancel>(JobRef { job }, ());
                return Ok(());
            }
            let page = self.call::<jobs::Poll>(
                JobQuery {
                    job: job.clone(),
                    after,
                },
                (),
            )?;
            for event in page.events {
                match serde_json::from_value::<T>(event.clone()) {
                    Ok(step) => each(Ok(step)),
                    Err(_) => each(Err(event)),
                }
            }
            after = page.next;
            if page.done {
                return page.error.map_or(Ok(()), Err);
            }
            std::thread::sleep(JOB_POLL);
        }
    }

    /// The journal: a page of it, and with `follow` every entry after it as
    /// it comes, until `stop` says so.
    pub fn logs(
        &mut self,
        mut query: LogsQuery,
        follow: bool,
        stop: &dyn Fn() -> bool,
        mut each: impl FnMut(Value),
    ) -> Result<(), String> {
        loop {
            let page = self.call::<api::device::Logs>(query.clone(), ())?;
            page.entries.into_iter().for_each(&mut each);
            if !follow {
                return Ok(());
            }
            query.cursor = page.cursor.or(query.cursor);
            let waited = Instant::now();
            while waited.elapsed() < LOG_POLL {
                if stop() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    /// How long a TCP session waits for an answer from now on. `None` waits
    /// for good; the local socket never times out.
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) {
        self.read_timeout = timeout;
        if let Some(tcp) = self.conn.as_ref().and_then(|conn| conn.tcp.as_ref()) {
            let _ = tcp.set_read_timeout(timeout);
        }
    }

    /// Back to the usual wait for an answer, after a longer one.
    pub fn restore_read_timeout(&mut self) {
        self.set_read_timeout(Some(IO));
    }

    /// The TCP socket under the session, for another thread to `shutdown`:
    /// the one way to end a request blocked on a device that has gone
    /// quiet. `None` on the local socket.
    pub fn shutdown_handle(&self) -> Option<TcpStream> {
        self.conn
            .as_ref()
            .and_then(|conn| conn.tcp.as_ref())
            .and_then(|tcp| tcp.try_clone().ok())
    }

    /// One exchange on the connection, opened again first if it is gone or
    /// stale. A request that never left on a connection the device had
    /// already closed is sent once more on a new one.
    fn exchange(
        &mut self,
        method: &str,
        target: &str,
        content: Option<&str>,
        body: &[u8],
    ) -> Result<Reply, String> {
        let authorization = self.token.as_ref().map(|token| format!("Bearer {token}"));
        let mut headers: Vec<(&str, &str)> =
            vec![("User-Agent", &self.agent), ("Accept", "application/json")];
        if let Some(value) = &authorization {
            headers.push(("Authorization", value));
        }
        if let Some(content) = content {
            headers.push(("Content-Type", content));
        }
        let headers: Vec<(String, String)> = headers
            .into_iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();

        for attempt in 0..2 {
            if self.conn.as_ref().is_none_or(Conn::stale) {
                self.conn = Some(self.redial()?);
            }
            let conn = self.conn.as_mut().expect("a connection was just opened");
            let pairs: Vec<(&str, &str)> = headers
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();
            match http::exchange(&mut conn.stream, method, target, &pairs, body) {
                Ok(reply) => {
                    conn.used = Instant::now();
                    if reply.close {
                        self.conn = None;
                    }
                    return Ok(reply);
                }
                Err(Broken::Unsent(error)) => {
                    self.conn = None;
                    if attempt > 0 {
                        return Err(error);
                    }
                }
                Err(Broken::Sent(error)) => {
                    self.conn = None;
                    return Err(error);
                }
            }
        }
        Err("the device keeps closing the connection".to_string())
    }

    /// A new connection to the same device, held to the same certificate.
    fn redial(&self) -> Result<Conn, String> {
        match &self.dial {
            Dial::Local(path) => dial_local(path),
            Dial::Remote {
                address,
                fingerprint,
            } => {
                let (conn, presented) = dial_remote(*address, CONNECT).map_err(Failure::message)?;
                if &presented != fingerprint {
                    return Err(format!(
                        "{address} now presents certificate {presented}, not {fingerprint}; \
                         this is either a different device or someone in the middle"
                    ));
                }
                if let Some(tcp) = &conn.tcp {
                    let _ = tcp.set_read_timeout(self.read_timeout);
                }
                Ok(conn)
            }
        }
    }
}

/// A JSON answer as `T`, or the device's refusal.
fn json_answer<T: DeserializeOwned>(reply: Reply) -> Answer<T> {
    if !(200..300).contains(&reply.status) {
        return Answer::Refused(refusal(&reply));
    }
    match serde_json::from_slice(&reply.body) {
        Ok(value) => Answer::Ok(value),
        Err(err) => Answer::Refused(format!("unexpected answer: {err}")),
    }
}

/// What the device said when it refused.
fn refusal(reply: &Reply) -> String {
    match serde_json::from_slice::<ApiError>(&reply.body) {
        Ok(refused) => refused.error,
        Err(_) => format!("the device answered HTTP {}", reply.status),
    }
}
