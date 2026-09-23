//! The two ways in: the local unix socket and TLS on TCP.
//!
//! Both carry the same newline-delimited JSON (see the protocol crate) and
//! end up in `Control::handle`. What differs is who is asking:
//!
//! * **`/run/tessaro-agent.sock`** is mode 0600, root. No token, no TLS -
//!   whoever can open it is already root on the device. Deliberately not
//!   group accessible: Chromium runs as `weston`, and a compromised browser
//!   must not be one `connect()` away from the control plane.
//! * **TCP** (`api.listen`, default `0.0.0.0:7400`) is TLS with the device's
//!   own self-signed identity, which clients pin. Every request needs a
//!   valid token except `id` and `claim`. Failed tokens are counted per
//!   address, and an address that keeps failing is refused for a minute.
//!
//! Every wait on a client is bounded - the hello, each request, each write -
//! so a client that connects and says nothing costs one task until its
//! deadline, never a stuck server. The accept loops and a followed log
//! stream are the waits that are meant to be open-ended.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::{from_line, to_line, Command, Frame, Hello, Request, MAX_LINE, PROTOCOL_VERSION};
use serde_json::Value;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::control::{Caller, Control};
use crate::deadline::within;
use crate::identity::Tls;
use crate::log::Log;

const HELLO: Duration = Duration::from_secs(10);
const HANDSHAKE: Duration = Duration::from_secs(10);
const WRITE: Duration = Duration::from_secs(30);
/// A remote client that goes quiet this long is dropped.
const REMOTE_IDLE: Duration = Duration::from_secs(10 * 60);
const LOCAL_IDLE: Duration = Duration::from_secs(60 * 60);
/// A `logs` without `--follow` that has not finished by now is cut off.
const LOGS: Duration = Duration::from_secs(30);

/// Failed tokens from one address within `WINDOW` before it is refused.
const STRIKES: u32 = 5;
const WINDOW: Duration = Duration::from_secs(60);
const PENALTY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
enum Origin {
    Local,
    Remote(SocketAddr),
}

pub fn spawn_unix(
    control: Arc<Control>,
    path: PathBuf,
    log: Arc<Log>,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    // A socket left behind by the previous run would make bind() fail.
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if metadata.file_type().is_socket() {
            std::fs::remove_file(&path)?;
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let listener = UnixListener::bind(&path)?;
    // /run is root-only-writable, and nobody but root can connect() to a
    // socket they cannot write to, so the umask window before this is closed.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    log.info(format!("control: listening on {}", path.display()));

    let limiter = Arc::new(Limiter::default());
    tokio::spawn(async move {
        let mut stop = shutdown.clone();
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted, // naked: an accept loop waits for as long as the agent runs
                _ = stop.changed() => break,
            };
            match accepted {
                Ok((stream, _)) => {
                    tokio::spawn(serve(
                        Arc::clone(&control),
                        stream,
                        Origin::Local,
                        Arc::clone(&log),
                        Arc::clone(&limiter),
                        shutdown.clone(),
                    ));
                }
                Err(err) => log.info(format!("control: accept on the socket failed: {err}")),
            }
        }
        let _ = std::fs::remove_file(&path);
    });
    Ok(())
}

pub async fn spawn_tls(
    control: Arc<Control>,
    addr: SocketAddr,
    tls: &Tls,
    log: Arc<Log>,
    shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let identity =
        native_tls::Identity::from_pkcs8(&tls.cert_pem, &tls.key_pem).map_err(io::Error::other)?;
    let acceptor = native_tls::TlsAcceptor::builder(identity)
        .min_protocol_version(Some(native_tls::Protocol::Tlsv12))
        .build()
        .map_err(io::Error::other)?;
    let acceptor = tokio_native_tls::TlsAcceptor::from(acceptor);

    let listener = TcpListener::bind(addr).await?; // naked: bind() is a local syscall, not a wait
    log.info(format!(
        "control: listening on {addr} (TLS, fingerprint {})",
        tls.fingerprint
    ));

    let limiter = Arc::new(Limiter::default());
    tokio::spawn(async move {
        let mut stop = shutdown.clone();
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted, // naked: an accept loop waits for as long as the agent runs
                _ = stop.changed() => break,
            };
            let (tcp, peer) = match accepted {
                Ok(accepted) => accepted,
                Err(err) => {
                    log.info(format!("control: accept failed: {err}"));
                    continue;
                }
            };

            let acceptor = acceptor.clone();
            let control = Arc::clone(&control);
            let log = Arc::clone(&log);
            let limiter = Arc::clone(&limiter);
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                match within("a TLS handshake", HANDSHAKE, acceptor.accept(tcp)).await {
                    Ok(Ok(stream)) => {
                        serve(
                            control,
                            stream,
                            Origin::Remote(peer),
                            log,
                            limiter,
                            shutdown,
                        )
                        .await // naked: serve bounds every wait itself
                    }
                    Ok(Err(err)) => {
                        log.debug(format!("control: TLS handshake with {peer} failed: {err}"))
                    }
                    Err(expired) => log.debug(format!("control: {peer}: {expired}")),
                }
            });
        }
    });
    Ok(())
}

async fn serve<S>(
    control: Arc<Control>,
    stream: S,
    origin: Origin,
    log: Arc<Log>,
    limiter: Arc<Limiter>,
    mut shutdown: watch::Receiver<bool>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);

    let hello = match within("a client's hello", HELLO, read_line(&mut reader, 64 * 1024)).await {
        Ok(Ok(Some(line))) => line,
        _ => return,
    };
    match from_line::<Hello>(&hello) {
        Ok(hello) if hello.protocol == PROTOCOL_VERSION => {}
        Ok(hello) => {
            let error = format!(
                "this agent speaks protocol {PROTOCOL_VERSION}, the client {}; update {}",
                hello.protocol, hello.client
            );
            let _ = send(&mut write, &Frame::Error { id: 0, error }).await;
            return;
        }
        Err(error) => {
            let _ = send(&mut write, &Frame::Error { id: 0, error }).await;
            return;
        }
    }

    let welcome = Frame::Welcome {
        protocol: PROTOCOL_VERSION,
        node: control.node(),
    };
    if send(&mut write, &welcome).await.is_err() {
        return;
    }

    let idle = match origin {
        Origin::Local => LOCAL_IDLE,
        Origin::Remote(_) => REMOTE_IDLE,
    };

    loop {
        let line = tokio::select! {
            line = within("a client request", idle, read_line(&mut reader, MAX_LINE)) => line,
            _ = shutdown.changed() => return,
        };
        let line = match line {
            Ok(Ok(Some(line))) => line,
            _ => return,
        };

        let request = match from_line::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                if send(&mut write, &Frame::Error { id: 0, error })
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let id = request.id;

        let caller = match authenticate(&control, &limiter, &log, origin, &request) {
            Ok(caller) => caller,
            Err(error) => {
                if send(&mut write, &Frame::Error { id, error }).await.is_err() {
                    return;
                }
                continue;
            }
        };

        if let Command::Logs {
            follow,
            unit,
            lines,
        } = &request.command
        {
            if stream_logs(
                &control,
                &mut write,
                id,
                *follow,
                unit.as_deref(),
                *lines,
                shutdown.clone(),
            )
            .await // naked: stream_logs bounds its own writes, and a follow is open-ended by design
            .is_err()
            {
                return;
            }
            continue;
        }

        let reply = control.handle(&caller, request.command).await; // naked: Control bounds every call it makes
        let frame = match reply.result {
            Ok(result) => Frame::Ok { id, result },
            Err(error) => Frame::Error { id, error },
        };
        let sent = send(&mut write, &frame).await;

        // Only now: these take this process down, and the client must have
        // its answer first.
        if let Some(after) = reply.after {
            control.run_after(after).await; // naked: run_after is under within() inside Bus
        }
        if sent.is_err() {
            return;
        }
    }
}

fn authenticate(
    control: &Control,
    limiter: &Limiter,
    log: &Log,
    origin: Origin,
    request: &Request,
) -> Result<Caller, String> {
    let peer = match origin {
        Origin::Local => return Ok(Caller::Local),
        Origin::Remote(peer) => peer,
    };

    match &request.token {
        // A valid token always gets in. Tokens are 256 bits, so the limiter
        // is not what stops guessing - it keeps a scan out of the journal and
        // off the CPU - and it must not lock out someone who shares the
        // scanner's NAT.
        Some(token) => match control.verify(token) {
            Some(id) => Ok(Caller::Token { id, peer }),
            // A stale token on `id` or `claim` - a client that still holds one
            // from before an unclaim - is no reason to refuse what needs none.
            None if request.command.is_public() => Ok(Caller::Anonymous { peer }),
            None if limiter.refused(peer.ip()) => {
                Err("too many failed attempts from this address; try again in a minute".to_string())
            }
            None => {
                limiter.strike(peer.ip());
                log.info(format!("control: {peer} presented an invalid token"));
                Err("invalid token".to_string())
            }
        },
        None if request.command.is_public() => Ok(Caller::Anonymous { peer }),
        None if !control.claimed() => {
            Err("this device is unclaimed; `tessaro-ctl claim` it first".to_string())
        }
        None => Err("a token is required; `tessaro-ctl login` with one".to_string()),
    }
}

async fn stream_logs<W: AsyncWrite + Unpin>(
    control: &Control,
    write: &mut W,
    id: u64,
    follow: bool,
    unit: Option<&str>,
    lines: Option<u32>,
    mut shutdown: watch::Receiver<bool>,
) -> io::Result<()> {
    let mut command = match control.journal(follow, unit, lines) {
        Ok(command) => command,
        Err(error) => return send(write, &Frame::Error { id, error }).await,
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let error = format!("journalctl: {err}");
            return send(write, &Frame::Error { id, error }).await;
        }
    };
    let Some(stdout) = child.stdout.take() else {
        return send(write, &Frame::End { id }).await;
    };
    let mut output = BufReader::new(stdout).lines();
    let started = Instant::now();

    loop {
        let next = if follow {
            tokio::select! {
                next = output.next_line() => next, // naked: a followed stream runs until the client leaves
                _ = shutdown.changed() => break,
            }
        } else {
            let left = LOGS.saturating_sub(started.elapsed());
            match within("journalctl", left, output.next_line()).await {
                Ok(next) => next,
                Err(_) => break,
            }
        };

        match next {
            Ok(Some(line)) => {
                let event = serde_json::from_str::<Value>(&line).unwrap_or(Value::String(line));
                send(write, &Frame::Event { id, event }).await?;
            }
            Ok(None) | Err(_) => break,
        }
    }

    let _ = child.start_kill();
    send(write, &Frame::End { id }).await
}

/// One line, at most `limit` bytes. `None` at end of stream.
async fn read_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let read = (&mut *reader)
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut buf)
        .await?; // naked: every caller bounds read_line with within()
    if read == 0 {
        return Ok(None);
    }
    if buf.len() > limit {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

async fn send<W: AsyncWrite + Unpin>(write: &mut W, frame: &Frame) -> io::Result<()> {
    let line = to_line(frame);
    let work = async {
        write.write_all(line.as_bytes()).await?; // naked: bounded by the within() below
        write.flush().await // naked: bounded by the within() below
    };
    match within("writing to a client", WRITE, work).await {
        Ok(outcome) => outcome,
        Err(expired) => Err(io::Error::new(io::ErrorKind::TimedOut, expired.to_string())),
    }
}

#[derive(Default)]
struct Limiter(Mutex<HashMap<IpAddr, Strikes>>);

struct Strikes {
    count: u32,
    since: Instant,
    refused_until: Option<Instant>,
}

impl Limiter {
    fn refused(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let map = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        map.get(&ip)
            .and_then(|strikes| strikes.refused_until)
            .is_some_and(|until| now < until)
    }

    fn strike(&self, ip: IpAddr) {
        let now = Instant::now();
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Forget addresses that have been quiet, so the map cannot grow
        // without bound under a scan.
        map.retain(|_, strikes| {
            now.duration_since(strikes.since) < WINDOW
                || strikes.refused_until.is_some_and(|until| now < until)
        });

        let strikes = map.entry(ip).or_insert(Strikes {
            count: 0,
            since: now,
            refused_until: None,
        });
        if now.duration_since(strikes.since) >= WINDOW {
            strikes.count = 0;
            strikes.since = now;
        }
        strikes.count += 1;
        if strikes.count >= STRIKES {
            strikes.refused_until = Some(now + PENALTY);
            strikes.count = 0;
            strikes.since = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn an_address_that_keeps_failing_is_refused_for_a_while() {
        let limiter = Limiter::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "192.0.2.2".parse().unwrap();

        for _ in 0..STRIKES - 1 {
            limiter.strike(ip);
        }
        assert!(!limiter.refused(ip));
        limiter.strike(ip);
        assert!(limiter.refused(ip));
        assert!(!limiter.refused(other));

        tokio::time::advance(PENALTY + Duration::from_secs(1)).await;
        assert!(!limiter.refused(ip));
    }

    #[tokio::test(start_paused = true)]
    async fn strikes_spread_out_over_time_are_forgiven() {
        let limiter = Limiter::default();
        let ip: IpAddr = "192.0.2.1".parse().unwrap();
        for _ in 0..STRIKES * 3 {
            limiter.strike(ip);
            tokio::time::advance(WINDOW).await;
        }
        assert!(!limiter.refused(ip));
    }

    #[tokio::test]
    async fn read_line_refuses_an_endless_line() {
        let data = [b'a'; 100];
        let mut reader = BufReader::new(&data[..]);
        assert!(read_line(&mut reader, 10).await.is_err());

        let mut reader = BufReader::new(&b"one\ntwo"[..]);
        assert_eq!(read_line(&mut reader, 10).await.unwrap().unwrap(), "one\n");
        assert_eq!(read_line(&mut reader, 10).await.unwrap().unwrap(), "two");
        assert_eq!(read_line(&mut reader, 10).await.unwrap(), None);
    }
}
