//! The one HTTP client, behind a trait.
//!
//! Its call sites are the reachability probe (which may be https, against
//! the real internet) and the CDP session's `/json/list` (plaintext,
//! loopback). They get separate instances because their timeouts differ.
//!
//! The trait exists so `Probe`'s redirect walk and its error-to-message
//! mapping can be tested without a network or a test server. Those messages
//! are what a technician reads on the serial console, so they are worth
//! testing.
//!
//! It is hyper's low-level client, not a batteries-included one, and these
//! things follow from that which are easy to get wrong:
//!
//! 1. **hyper adds no `Host` header.** Without one most servers answer 400,
//!    which the probe would faithfully report as "server answered HTTP 400" -
//!    a plausible-looking wrong answer. `exchange` sets `Host` and
//!    `User-Agent` itself.
//! 2. **The connection future has to be driven and then reaped.** `handshake`
//!    returns a request handle and a connection future; the future must be
//!    spawned or the request never completes, and aborted afterwards or every
//!    probe leaks a task. The no-naked-awaits rule does not catch this class,
//!    so there is a test for it.
//! 3. **No redirects, no pooling.** The probe walks redirects by hand so a 3xx
//!    without a `Location` and a loop stay distinguishable in the journal, and
//!    one connection per request means every probe proves a *fresh*
//!    connection works.
//! 4. **The body is capped.** hyper has no bound of its own. The probe only
//!    needs the status, so the body is truncated at the cap, never an error.
//!
//! Every phase - DNS, connect, TLS, handshake, response, body - is its own
//! `within()`, so each reaches the journal under its own name and each pledges
//! its own budget to the watchdog.
//!
//! **With network.proxy.url set, the probe and the public address go
//! through the device's local proxy** (`with_proxy`): a `CONNECT` tunnel for
//! http and https alike, and no DNS on the device - the proxy resolves the
//! name. The local proxy is tinyproxy, which carries the upstream's scheme
//! and credentials (docs/networking.md, "Proxy").
//!
//! **TLS is openssl, through native-tls, and there are ways to get it wrong
//! quietly.** A missing provider used to be the silent one (ureq linked
//! cleanly without libssl and panicked on the first https request); with
//! native-tls used directly that is now a link error. The silent one now is
//! an https URL that never takes the TLS path: a plaintext GET to port 443
//! fails like a dead site, and the device sits on the offline page forever.
//! Both have a test. `readelf -d` on the built binary should always show
//! `libssl.so.3`.

use std::io::ErrorKind;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::header::{HOST, LOCATION, USER_AGENT as USER_AGENT_HEADER};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

use crate::watchdog::Heartbeat;

pub const USER_AGENT: &str = "tessaro-agent";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub location: Option<String>,
    pub body: String,
}

/// Classified transport failures. The probe turns these into the sentences
/// that reach the journal; keeping the classification separate from the
/// wording means the wording can be asserted on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HttpError {
    #[error("the URL is not valid")]
    InvalidUrl,
    #[error("DNS did not resolve the host")]
    Dns,
    /// The resolver took longer than the connect budget. Its own variant
    /// because "timed out" would point at the web server, and a nameserver
    /// that swallows queries is the realistic way to stall this probe.
    #[error("DNS did not answer")]
    DnsTimeout,
    #[error("connection refused")]
    ConnectionRefused,
    #[error("network unreachable")]
    NetworkUnreachable,
    #[error("connection timed out")]
    ConnectTimeout,
    #[error("timed out")]
    ReadTimeout,
    #[error("certificate verification failed")]
    CertificateVerification,
    #[error("TLS handshake failed")]
    Tls,
    /// The device's own proxy, on loopback, refused or did not answer.
    #[error("the local proxy is not answering")]
    ProxyUnreachable,
    /// The proxy chain refused the tunnel: 407 for credentials the
    /// upstream turned down, 5xx for a target it could not reach.
    #[error("the proxy answered HTTP {0}")]
    Proxy(u16),
    #[error("{0}")]
    Other(String),
}

#[async_trait(?Send)]
pub trait HttpGet {
    async fn get(&self, url: &str) -> Result<HttpResponse, HttpError>;
}

/// Whatever carries the request: a plain socket or a TLS stream over one.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

pub struct HyperHttp {
    connect_timeout: Duration,
    read_timeout: Duration,
    max_body: usize,
    heartbeat: Heartbeat,
    tls: Result<tokio_native_tls::TlsConnector, String>,
    /// The device's local proxy, when requests go through it (`with_proxy`).
    proxy: Option<SocketAddr>,
}

impl HyperHttp {
    /// `connect_timeout` and `read_timeout` are seconds. The first covers
    /// DNS, the TCP connect, the TLS handshake and the HTTP handshake; the
    /// second the response headers and, separately, the body.
    pub fn new(
        connect_timeout: i64,
        read_timeout: i64,
        max_body: usize,
        heartbeat: Heartbeat,
    ) -> Self {
        // native-tls means openssl here, with openssl's default verify paths:
        // the device's /etc/ssl/certs from ca-certificates. That is the
        // platform verifier by construction - there is no compiled-in root
        // store to pick by mistake.
        let tls = native_tls::TlsConnector::new()
            .map(tokio_native_tls::TlsConnector::from)
            .map_err(|err| err.to_string());

        Self {
            connect_timeout: seconds(connect_timeout),
            read_timeout: seconds(read_timeout),
            max_body,
            heartbeat,
            tls,
            proxy: None,
        }
    }

    /// Every request through the device's local proxy at `proxy` (tinyproxy,
    /// `tessaro-proxy.service`), which carries the upstream's scheme and
    /// credentials. Only for clients that reach the internet - the probe
    /// and the public address - never CDP on loopback.
    pub fn with_proxy(mut self, proxy: Option<SocketAddr>) -> Self {
        self.proxy = proxy;
        self
    }

    /// The whole request. An inherent method rather than only the trait's,
    /// because the trait's futures are `!Send` and the CDP session driver
    /// calls this from a spawned task.
    pub async fn fetch(&self, url: &str) -> Result<HttpResponse, HttpError> {
        let target = Target::parse(url).ok_or(HttpError::InvalidUrl)?;
        let stream = match self.proxy {
            Some(proxy) => self.tunnel(&target, proxy).await?,
            None => self.connect(&target).await?,
        };

        let io: Box<dyn Io> = if target.https {
            self.tls_handshake(&target, stream).await?
        } else {
            Box::new(stream)
        };

        self.exchange(&target, io).await
    }

    async fn connect(&self, target: &Target) -> Result<TcpStream, HttpError> {
        let addresses: Vec<SocketAddr> = match target.host.parse::<IpAddr>() {
            Ok(ip) => vec![SocketAddr::new(ip, target.port)],
            Err(_) => {
                // tokio's lookup_host is getaddrinfo on a blocking thread. The
                // deadline frees *us*; the thread finishes on its own, bounded
                // by glibc's resolver (timeout:5 attempts:2) - this image has
                // no nss-resolve in nsswitch.conf, so there is no unbounded
                // call behind it.
                let lookup = tokio::net::lookup_host((target.host.as_str(), target.port));
                match self
                    .heartbeat
                    .within("DNS lookup", self.connect_timeout, lookup)
                    .await
                {
                    Err(_) => return Err(HttpError::DnsTimeout),
                    Ok(Err(_)) => return Err(HttpError::Dns),
                    Ok(Ok(found)) => found.collect(),
                }
            }
        };
        if addresses.is_empty() {
            return Err(HttpError::Dns);
        }

        // Every address in turn, all inside one connect budget: "localhost"
        // is ::1 first, and a refusal there is instant.
        let attempt = async {
            let mut last = std::io::Error::new(ErrorKind::NotFound, "no address to connect to");
            for address in &addresses {
                // naked: bounded by the connect within() below
                match TcpStream::connect(address).await {
                    Ok(stream) => return Ok(stream),
                    Err(err) => last = err,
                }
            }
            Err(last)
        };

        match self
            .heartbeat
            .within("TCP connect", self.connect_timeout, attempt)
            .await
        {
            Err(_) => Err(HttpError::ConnectTimeout),
            Ok(Err(err)) => Err(classify_io(&err)),
            Ok(Ok(stream)) => Ok(stream),
        }
    }

    /// A `CONNECT` tunnel to the target through the local proxy, for http
    /// and https alike, so `exchange` is the same either way. No DNS here:
    /// the proxy resolves the name, which on a network that only allows
    /// the proxy is the only place that can.
    async fn tunnel(&self, target: &Target, proxy: SocketAddr) -> Result<TcpStream, HttpError> {
        let mut stream = match self
            .heartbeat
            .within(
                "proxy connect",
                self.connect_timeout,
                TcpStream::connect(proxy),
            )
            .await
        {
            Err(_) | Ok(Err(_)) => return Err(HttpError::ProxyUnreachable),
            Ok(Ok(stream)) => stream,
        };

        let authority = target.connect_authority();
        let request = format!(
            "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: {USER_AGENT}\r\n\r\n"
        );
        let status = async {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            // naked: bounded by the "proxy CONNECT" within() below
            stream.write_all(request.as_bytes()).await?;
            // Byte by byte up to the blank line, so nothing of what the
            // target sends after the tunnel opens is taken here.
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                // naked: bounded by the "proxy CONNECT" within() below
                if head.len() > 8192 || stream.read(&mut byte).await? == 0 {
                    return Err(std::io::Error::new(
                        ErrorKind::InvalidData,
                        "the proxy's answer ended early",
                    ));
                }
                head.push(byte[0]);
            }
            Ok(connect_status(&head))
        };
        match self
            .heartbeat
            .within("proxy CONNECT", self.connect_timeout, status)
            .await
        {
            Err(_) => Err(HttpError::ConnectTimeout),
            Ok(Err(_)) => Err(HttpError::ProxyUnreachable),
            Ok(Ok(Some(200))) => Ok(stream),
            Ok(Ok(Some(status))) => Err(HttpError::Proxy(status)),
            Ok(Ok(None)) => Err(HttpError::ProxyUnreachable),
        }
    }

    async fn tls_handshake(
        &self,
        target: &Target,
        stream: TcpStream,
    ) -> Result<Box<dyn Io>, HttpError> {
        let connector = self.tls.as_ref().map_err(|_| HttpError::Tls)?;
        let handshake = connector.connect(&target.host, stream);

        match self
            .heartbeat
            .within("TLS handshake", self.connect_timeout, handshake)
            .await
        {
            Err(_) => Err(HttpError::ConnectTimeout),
            // Anything that fails here is TLS, whatever the text says - a
            // plaintext server answering on 443 included.
            Ok(Err(err)) => Err(match classify_text(&err.to_string()) {
                HttpError::CertificateVerification => HttpError::CertificateVerification,
                _ => HttpError::Tls,
            }),
            Ok(Ok(stream)) => Ok(Box::new(stream)),
        }
    }

    async fn exchange(&self, target: &Target, io: Box<dyn Io>) -> Result<HttpResponse, HttpError> {
        let handshake = hyper::client::conn::http1::handshake(TokioIo::new(io));
        let (mut sender, connection) = match self
            .heartbeat
            .within("HTTP handshake", self.connect_timeout, handshake)
            .await
        {
            Err(_) => return Err(HttpError::ConnectTimeout),
            Ok(Err(err)) => return Err(HttpError::Other(err.to_string())),
            Ok(Ok(pair)) => pair,
        };

        // Driven by a task of its own - the request never completes without
        // it - and aborted on the way out, whichever way that is, so no probe
        // leaves a task behind.
        let _driver = AbortOnDrop(tokio::spawn(connection));

        let request = Request::get(target.path.as_str())
            .header(HOST, target.authority.as_str())
            .header(USER_AGENT_HEADER, USER_AGENT)
            .body(Empty::<Bytes>::new())
            .map_err(|_| HttpError::InvalidUrl)?;

        let response = match self
            .heartbeat
            .within(
                "HTTP response",
                self.read_timeout,
                sender.send_request(request),
            )
            .await
        {
            Err(_) => return Err(HttpError::ReadTimeout),
            Ok(Err(err)) => return Err(classify_text(&err.to_string())),
            Ok(Ok(response)) => response,
        };

        let status = response.status().as_u16();
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);

        let body = read_body(response.into_body(), self.max_body);
        let body = match self
            .heartbeat
            .within("HTTP body", self.read_timeout, body)
            .await
        {
            Err(_) => return Err(HttpError::ReadTimeout),
            Ok(Err(err)) => return Err(HttpError::Other(err)),
            Ok(Ok(body)) => body,
        };

        Ok(HttpResponse {
            status,
            location,
            body,
        })
    }
}

#[async_trait(?Send)]
impl HttpGet for HyperHttp {
    async fn get(&self, url: &str) -> Result<HttpResponse, HttpError> {
        self.fetch(url).await // naked: every phase inside fetch() has its own within()
    }
}

struct AbortOnDrop(JoinHandle<Result<(), hyper::Error>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Up to `limit` bytes, then stop: the probe wants the status, and a large
/// home page must not become a failure.
async fn read_body(mut body: Incoming, limit: usize) -> Result<String, String> {
    let mut bytes = Vec::new();

    while bytes.len() < limit {
        // naked: bounded by the body within() in exchange()
        match body.frame().await {
            None => break,
            Some(Err(err)) => return Err(err.to_string()),
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    bytes.extend_from_slice(&data);
                }
            }
        }
    }

    bytes.truncate(limit);
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Just enough URL for a GET, by hand like the rest of `url.rs`.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    https: bool,
    /// Without brackets, for DNS, the socket address and TLS SNI.
    host: String,
    port: u16,
    /// As written in the URL, for the `Host` header.
    authority: String,
    /// Origin-form: path and query, never empty.
    path: String,
}

impl Target {
    fn parse(url: &str) -> Option<Self> {
        let (https, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            return None;
        };

        let rest = rest.split('#').next().unwrap_or("");
        let split = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(split);
        let authority = authority.rsplit('@').next().unwrap_or("");

        let path = if tail.is_empty() {
            "/".to_string()
        } else if tail.starts_with('?') {
            format!("/{tail}")
        } else {
            tail.to_string()
        };

        let default_port = if https { 443 } else { 80 };
        let (host, port) = match authority.strip_prefix('[') {
            Some(bracketed) => {
                let (host, after) = bracketed.split_once(']')?;
                let port = match after.strip_prefix(':') {
                    Some(port) => port.parse().ok()?,
                    None if after.is_empty() => default_port,
                    None => return None,
                };
                (host.to_string(), port)
            }
            None => match authority.rsplit_once(':') {
                Some((host, port)) => (host.to_string(), port.parse().ok()?),
                None => (authority.to_string(), default_port),
            },
        };

        if host.is_empty() {
            return None;
        }

        Some(Self {
            https,
            host,
            port,
            authority: authority.to_string(),
            path,
        })
    }
}

impl Target {
    /// `host:port` for a `CONNECT`, an IPv6 host in brackets.
    fn connect_authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// The status of a proxy's answer to `CONNECT`: `HTTP/1.x NNN ...`.
fn connect_status(head: &[u8]) -> Option<u16> {
    let line = head.split(|byte| *byte == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut words = line.split_whitespace();
    words
        .next()
        .filter(|version| version.starts_with("HTTP/"))?;
    words.next()?.parse().ok()
}

fn seconds(value: i64) -> Duration {
    Duration::from_secs(value.max(0) as u64)
}

/// The io::ErrorKind cases are matched structurally because they are the ones
/// that matter operationally - refused, unreachable.
fn classify_io(err: &std::io::Error) -> HttpError {
    match err.kind() {
        ErrorKind::ConnectionRefused => HttpError::ConnectionRefused,
        ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => HttpError::NetworkUnreachable,
        ErrorKind::TimedOut => HttpError::ConnectTimeout,
        _ => classify_text(&err.to_string()),
    }
}

/// TLS is matched on the message: OpenSSL's verification failures are only
/// distinguishable by their text, and telling "the clock is wrong / the CA
/// store is empty" apart from "the handshake broke" is worth the string match.
fn classify_text(text: &str) -> HttpError {
    let lowered = text.to_ascii_lowercase();

    if lowered.contains("certificate verify failed")
        || lowered.contains("unable to get local issuer")
        || lowered.contains("self-signed")
        || lowered.contains("self signed")
    {
        HttpError::CertificateVerification
    } else if lowered.contains("tls") || lowered.contains("ssl") || lowered.contains("handshake") {
        HttpError::Tls
    } else if lowered.contains("connection refused") {
        HttpError::ConnectionRefused
    } else if lowered.contains("unreachable") {
        HttpError::NetworkUnreachable
    } else if lowered.contains("failed to lookup") || lowered.contains("name or service not known")
    {
        HttpError::Dns
    } else {
        HttpError::Other(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn client() -> HyperHttp {
        HyperHttp::new(2, 2, 1024, Heartbeat::detached())
    }

    /// A server that answers every connection with `response`, hands back the
    /// requests it saw, and keeps each socket open afterwards - so a client
    /// that forgot to reap its connection task would visibly leak it.
    async fn serve(
        response: impl Into<String>,
    ) -> (u16, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let response: String = response.into();
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let (seen, requests) = tokio::sync::mpsc::unbounded_channel();

        tokio::spawn(async move {
            let mut open = Vec::new();
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buffer = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match socket.read(&mut buffer).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buffer[..n]),
                    }
                }
                let _ = seen.send(String::from_utf8_lossy(&request).into_owned());
                let _ = socket.write_all(response.as_bytes()).await;
                open.push(socket);
            }
        });

        (port, requests)
    }

    /// Reaching a transport error at all proves the TLS provider is compiled
    /// in and wired. Port 1 on the loopback refuses instantly, so this costs
    /// nothing and needs no network.
    #[tokio::test]
    async fn https_reaches_the_transport() {
        let result = client().fetch("https://127.0.0.1:1/").await;

        assert!(
            matches!(
                result,
                Err(HttpError::ConnectionRefused | HttpError::ConnectTimeout)
            ),
            "expected a transport failure, got {result:?}"
        );
    }

    /// The silent failure this client can have: an https URL that is sent in
    /// the clear. A server that answers plaintext must fail the handshake, not
    /// come back as a 200 - and not as a vague transport error either.
    #[tokio::test]
    async fn an_https_url_is_handshaken_not_sent_in_the_clear() {
        // A plaintext web server on the https port: it reads whatever arrives
        // - a ClientHello, which is no HTTP request - and answers it in the
        // clear at once, as a real one does.
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer).await;
            let _ = socket
                .write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n")
                .await;
            let _ = socket.read(&mut buffer).await;
        });

        let result = client().fetch(&format!("https://127.0.0.1:{port}/")).await;

        assert_eq!(result, Err(HttpError::Tls));
    }

    #[tokio::test]
    async fn a_request_carries_host_and_user_agent() {
        let (port, mut requests) =
            serve("HTTP/1.1 302 Found\r\nlocation: /next\r\ncontent-length: 2\r\n\r\nhi").await;

        let response = client()
            .fetch(&format!("http://127.0.0.1:{port}/health?x=1"))
            .await
            .expect("response");

        assert_eq!(
            response,
            HttpResponse {
                status: 302,
                location: Some("/next".to_string()),
                body: "hi".to_string(),
            }
        );

        let request = requests.recv().await.expect("request").to_ascii_lowercase();
        assert!(
            request.starts_with("get /health?x=1 http/1.1\r\n"),
            "{request}"
        );
        assert!(
            request.contains(&format!("host: 127.0.0.1:{port}\r\n")),
            "{request}"
        );
        assert!(
            request.contains("user-agent: tessaro-agent\r\n"),
            "{request}"
        );
    }

    /// A proxy that answers the `CONNECT` with `answer` and, when that is a
    /// 200, serves one plain HTTP response inside the tunnel. Hands back the
    /// `CONNECT` it saw.
    async fn proxy(
        answer: &'static str,
    ) -> (SocketAddr, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("addr");
        let (seen, requests) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                    return;
                }
                head.push(byte[0]);
            }
            let _ = seen.send(String::from_utf8_lossy(&head).into_owned());
            let _ = socket.write_all(answer.as_bytes()).await;
            if answer.starts_with("HTTP/1.1 200") {
                let mut buffer = [0u8; 1024];
                let _ = socket.read(&mut buffer).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n")
                    .await;
                let _ = socket.read(&mut buffer).await;
            }
        });
        (address, requests)
    }

    /// Through a proxy nothing is resolved here: the name goes to the proxy
    /// in the `CONNECT`, and the request runs inside the tunnel.
    #[tokio::test]
    async fn a_proxy_gets_a_connect_for_the_name_and_the_request_goes_through_it() {
        let (address, mut requests) = proxy("HTTP/1.1 200 Connection established\r\n\r\n").await;

        let response = client()
            .with_proxy(Some(address))
            .fetch("http://kiosk.invalid:8080/health")
            .await
            .expect("response");

        assert_eq!(response.status, 204);
        let connect = requests.recv().await.expect("connect");
        assert!(
            connect.starts_with("CONNECT kiosk.invalid:8080 HTTP/1.1\r\n"),
            "{connect}"
        );
    }

    #[tokio::test]
    async fn a_refused_tunnel_carries_the_proxys_status() {
        let (address, _requests) =
            proxy("HTTP/1.1 407 Proxy Authentication Required\r\ncontent-length: 0\r\n\r\n").await;

        let result = client()
            .with_proxy(Some(address))
            .fetch("https://kiosk.invalid/")
            .await;

        assert_eq!(result, Err(HttpError::Proxy(407)));
    }

    #[tokio::test]
    async fn a_proxy_that_is_not_there_says_so() {
        let result = client()
            .with_proxy(Some("127.0.0.1:1".parse().unwrap()))
            .fetch("https://kiosk.invalid/")
            .await;

        assert_eq!(result, Err(HttpError::ProxyUnreachable));
    }

    #[test]
    fn connect_answers_and_authorities() {
        assert_eq!(connect_status(b"HTTP/1.0 200 OK\r\n\r\n"), Some(200));
        assert_eq!(connect_status(b"garbage\r\n\r\n"), None);
        let target = Target::parse("https://[fd00::1]/x").unwrap();
        assert_eq!(target.connect_authority(), "[fd00::1]:443");
    }

    #[tokio::test]
    async fn a_large_body_is_truncated_not_failed() {
        let (port, _requests) = serve(format!(
            "HTTP/1.1 200 OK\r\ncontent-length: 2000\r\n\r\n{}",
            "x".repeat(2000)
        ))
        .await;

        let response = client()
            .fetch(&format!("http://127.0.0.1:{port}/"))
            .await
            .expect("response");

        assert_eq!(response.status, 200);
        assert_eq!(response.body.len(), 1024);
    }

    #[tokio::test]
    async fn no_probe_leaves_a_task_behind() {
        let (port, _requests) = serve("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n").await;
        let http = client();
        let url = format!("http://127.0.0.1:{port}/");

        http.fetch(&url).await.expect("warm up");
        let settle = || async {
            for _ in 0..20 {
                tokio::task::yield_now().await;
            }
        };
        settle().await;
        let before = tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks();

        for _ in 0..100 {
            http.fetch(&url).await.expect("response");
        }
        settle().await;

        assert_eq!(
            tokio::runtime::Handle::current()
                .metrics()
                .num_alive_tasks(),
            before
        );
    }

    #[test]
    fn targets_parse_the_way_a_get_needs_them() {
        let parsed = |url: &str| Target::parse(url);

        assert_eq!(
            parsed("https://kiosk.test"),
            Some(Target {
                https: true,
                host: "kiosk.test".into(),
                port: 443,
                authority: "kiosk.test".into(),
                path: "/".into(),
            })
        );
        assert_eq!(
            parsed("http://127.0.0.1:9222/json/list#x"),
            Some(Target {
                https: false,
                host: "127.0.0.1".into(),
                port: 9222,
                authority: "127.0.0.1:9222".into(),
                path: "/json/list".into(),
            })
        );
        assert_eq!(
            parsed("http://[::1]:8080?q").map(|t| (t.host, t.port, t.path)),
            Some(("::1".into(), 8080, "/?q".into()))
        );
        assert_eq!(
            parsed("http://user@host/p").map(|t| t.authority),
            Some("host".into())
        );
        assert_eq!(parsed("ftp://kiosk.test/"), None);
        assert_eq!(parsed("http://"), None);
        assert_eq!(parsed("http://host:notaport/"), None);
    }
}
