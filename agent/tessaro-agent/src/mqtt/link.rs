//! The bytes under the MQTT session: a TCP connection, straight or through
//! the local proxy's tunnel; TLS for `mqtts://` and `wss://`, with the
//! device's trust (the system's authorities and `network certs`'), a client
//! certificate when one is kept, and a pinned fingerprint instead of the
//! chain when mqtt.tls.fingerprint is set; and a WebSocket for `ws://` and
//! `wss://`, its handshake over that stream. Every step has a deadline and
//! names itself when it fails, which `tessaro-ctl mqtt test` reports.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use protocol::mqtt::BrokerUrl;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::WebSocketStream;

use crate::deadline::within;

/// DNS, the TCP connect, the proxy's tunnel, the TLS and WebSocket
/// handshakes: each.
const STEP: Duration = Duration::from_secs(15);

/// Whatever carries the session: a plain socket or a TLS stream over one.
pub trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Where to connect and how.
#[derive(Clone)]
pub struct Target {
    pub url: BrokerUrl,
    /// mqtt.tls.fingerprint: the broker certificate's SHA-256, which
    /// replaces the chain check.
    pub fingerprint: Option<String>,
    /// A client certificate and its key, both PEM.
    pub identity: Option<(String, String)>,
    /// The device's local proxy, when network.proxy.url is set.
    pub proxy: Option<SocketAddr>,
}

/// Where opening the link stopped, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub stage: &'static str,
    pub error: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.error)
    }
}

fn fail(stage: &'static str, error: impl Into<String>) -> Failure {
    Failure {
        stage,
        error: error.into(),
    }
}

pub struct Opened {
    pub transport: Transport,
    /// The broker certificate's SHA-256, over TLS.
    pub fingerprint: Option<String>,
}

pub enum Transport {
    Stream(Box<dyn Io>, Vec<u8>),
    Socket(Box<WebSocketStream<Box<dyn Io>>>),
}

impl Transport {
    /// The next bytes from the broker; `None` when it closed. Cancel-safe:
    /// whatever was read is never lost to a `select!` that went elsewhere.
    pub async fn recv(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        match self {
            Transport::Stream(stream, buffer) => {
                // naked: the session's keep-alive bounds a quiet link
                let read = stream.read(buffer).await?;
                Ok((read > 0).then(|| buffer[..read].to_vec()))
            }
            Transport::Socket(socket) => loop {
                // naked: the session's keep-alive bounds a quiet link
                match socket.next().await {
                    None | Some(Ok(Frame::Close(_))) => return Ok(None),
                    Some(Ok(Frame::Binary(bytes))) => return Ok(Some(bytes.to_vec())),
                    // Pings are answered by the socket itself, on the next
                    // write or flush.
                    Some(Ok(_)) => {}
                    Some(Err(err)) => return Err(std::io::Error::other(err.to_string())),
                }
            },
        }
    }

    /// Bytes to the broker. The caller bounds it.
    pub async fn send(&mut self, bytes: Vec<u8>) -> std::io::Result<()> {
        match self {
            Transport::Stream(stream, _) => {
                // naked: the caller bounds the write
                stream.write_all(&bytes).await?;
                // naked: the caller bounds the write
                stream.flush().await
            }
            Transport::Socket(socket) => socket
                .send(Frame::Binary(bytes.into()))
                // naked: the caller bounds the write
                .await
                .map_err(|err| std::io::Error::other(err.to_string())),
        }
    }

    /// Close it politely, briefly.
    pub async fn close(&mut self) {
        let closing = async {
            match self {
                Transport::Stream(stream, _) => {
                    // naked: bounded by the within() below
                    let _ = stream.shutdown().await;
                }
                Transport::Socket(socket) => {
                    // naked: bounded by the within() below
                    let _ = SinkExt::close(socket).await;
                }
            }
        };
        let _ = within("closing the MQTT link", Duration::from_secs(2), closing).await;
    }
}

/// Open the link `target` describes, up to where the MQTT session starts.
pub async fn open(target: &Target) -> Result<Opened, Failure> {
    let url = &target.url;
    // Each step below bounds itself with within().
    let stream = match target.proxy {
        // naked: tunnel bounds itself with within()
        Some(proxy) => tunnel(proxy, url).await?,
        // naked: connect bounds itself with within()
        None => connect(url).await?,
    };
    let (stream, fingerprint): (Box<dyn Io>, Option<String>) = if url.tls() {
        // naked: tls bounds itself with within()
        let (stream, fingerprint) = tls(target, stream).await?;
        (stream, Some(fingerprint))
    } else {
        (Box::new(stream), None)
    };
    let transport = if url.websocket() {
        // naked: websocket bounds itself with within()
        Transport::Socket(Box::new(websocket(url, stream).await?))
    } else {
        Transport::Stream(stream, vec![0u8; 64 * 1024])
    };
    Ok(Opened {
        transport,
        fingerprint,
    })
}

async fn connect(url: &BrokerUrl) -> Result<TcpStream, Failure> {
    let addresses: Vec<SocketAddr> = match url.host.parse::<std::net::IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, url.port)],
        Err(_) => {
            // getaddrinfo on a blocking thread, bounded by glibc's resolver
            // as well (docs/agent.md).
            let lookup = tokio::net::lookup_host((url.host.as_str(), url.port));
            match within("the broker's DNS lookup", STEP, lookup).await {
                Err(expired) => return Err(fail("dns", expired.to_string())),
                Ok(Err(err)) => return Err(fail("dns", format!("{}: {err}", url.host))),
                Ok(Ok(found)) => found.collect(),
            }
        }
    };
    if addresses.is_empty() {
        return Err(fail("dns", format!("{} has no address", url.host)));
    }
    let attempt = async {
        let mut last =
            std::io::Error::new(std::io::ErrorKind::NotFound, "no address to connect to");
        for address in &addresses {
            // naked: bounded by the connect within() below
            match TcpStream::connect(address).await {
                Ok(stream) => return Ok(stream),
                Err(err) => last = err,
            }
        }
        Err(last)
    };
    match within("connecting to the broker", STEP, attempt).await {
        Err(expired) => Err(fail("connect", expired.to_string())),
        Ok(Err(err)) => Err(fail("connect", format!("{}:{}: {err}", url.host, url.port))),
        Ok(Ok(stream)) => {
            let _ = stream.set_nodelay(true);
            Ok(stream)
        }
    }
}

/// A tunnel to the broker through the local proxy, which resolves its name.
async fn tunnel(proxy: SocketAddr, url: &BrokerUrl) -> Result<TcpStream, Failure> {
    let mut stream = match within("connecting to the proxy", STEP, TcpStream::connect(proxy)).await
    {
        Err(expired) => return Err(fail("proxy", expired.to_string())),
        Ok(Err(err)) => return Err(fail("proxy", format!("the local proxy at {proxy}: {err}"))),
        Ok(Ok(stream)) => stream,
    };
    let authority = match url.host.contains(':') {
        true => format!("[{}]:{}", url.host, url.port),
        false => format!("{}:{}", url.host, url.port),
    };
    match within(
        "the proxy's tunnel",
        STEP,
        crate::http::proxy_connect(&mut stream, &authority),
    )
    .await
    {
        Err(expired) => Err(fail("proxy", expired.to_string())),
        Ok(Err(err)) => Err(fail("proxy", format!("the local proxy: {err}"))),
        Ok(Ok(Some(200))) => {
            let _ = stream.set_nodelay(true);
            Ok(stream)
        }
        Ok(Ok(Some(status))) => Err(fail(
            "proxy",
            format!(
                "the proxy refused a tunnel to {authority} ({status}); check network.proxy.url"
            ),
        )),
        Ok(Ok(None)) => Err(fail("proxy", "the proxy's answer is not HTTP")),
    }
}

async fn tls(target: &Target, stream: TcpStream) -> Result<(Box<dyn Io>, String), Failure> {
    let identity = match &target.identity {
        Some((cert, key)) => Some(
            native_tls::Identity::from_pkcs8(cert.as_bytes(), key.as_bytes())
                .map_err(|err| fail("tls", format!("the client certificate: {err}")))?,
        ),
        None => None,
    };
    let connector = crate::http::tls_client(identity, target.fingerprint.is_some())
        .map_err(|err| fail("tls", err))?;
    let handshake = connector.connect(&target.url.host, stream);
    let stream = match within("the broker's TLS handshake", STEP, handshake).await {
        Err(expired) => return Err(fail("tls", expired.to_string())),
        Ok(Err(err)) => return Err(fail("tls", tls_error(&err.to_string()))),
        Ok(Ok(stream)) => stream,
    };
    let fingerprint = stream
        .get_ref()
        .peer_certificate()
        .ok()
        .flatten()
        .and_then(|cert| cert.to_der().ok())
        .map(|der| protocol::hex(&openssl::sha::sha256(&der)))
        .ok_or_else(|| fail("tls", "the broker sent no certificate"))?;
    if let Some(pinned) = &target.fingerprint {
        if *pinned != fingerprint {
            return Err(fail(
                "tls",
                format!("the broker's certificate is {fingerprint}, not mqtt.tls.fingerprint's {pinned}"),
            ));
        }
    }
    Ok((Box::new(stream), fingerprint))
}

/// openssl's words for a chain it would not take, made shorter.
fn tls_error(error: &str) -> String {
    if error.contains("certificate verify failed") {
        format!(
            "the broker's certificate is not trusted ({error}); add its authority with \
             `tessaro-ctl network certs add`, or pin it with mqtt.tls.fingerprint"
        )
    } else {
        error.to_string()
    }
}

async fn websocket(
    url: &BrokerUrl,
    stream: Box<dyn Io>,
) -> Result<WebSocketStream<Box<dyn Io>>, Failure> {
    // The scheme only picks the Host and the path here: the TLS, if any,
    // is already under `stream`.
    let host = if url.host.contains(':') {
        format!("[{}]", url.host)
    } else {
        url.host.clone()
    };
    let mut request = format!("ws://{host}:{}{}", url.port, url.path)
        .into_client_request()
        .map_err(|err| fail("websocket", err.to_string()))?;
    request
        .headers_mut()
        .insert("Sec-WebSocket-Protocol", HeaderValue::from_static("mqtt"));
    let handshake = tokio_tungstenite::client_async(request, stream);
    match within("the broker's WebSocket handshake", STEP, handshake).await {
        Err(expired) => Err(fail("websocket", expired.to_string())),
        Ok(Err(err)) => Err(fail("websocket", err.to_string())),
        Ok(Ok((socket, _))) => Ok(socket),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untrusted_chain_says_what_to_do() {
        let words = tls_error("error:0A000086:SSL routines:tls_post_process_server_certificate:certificate verify failed");
        assert!(
            words.contains("network certs add") && words.contains("mqtt.tls.fingerprint"),
            "{words}"
        );
        assert_eq!(tls_error("handshake failure"), "handshake failure");
    }

    #[tokio::test]
    async fn a_refused_connect_names_its_stage() {
        // A port nothing listens on, on loopback: refused at once.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let target = Target {
            url: protocol::mqtt::parse_url(&format!("mqtt://127.0.0.1:{port}")).unwrap(),
            fingerprint: None,
            identity: None,
            proxy: None,
        };
        let failure = open(&target).await.err().expect("nothing listens");
        assert_eq!(failure.stage, "connect");
    }
}
