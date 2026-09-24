//! ICMP echo from the device: `tessaro-ctl net ping`, and the gateway check
//! a network change must pass before it is kept.
//!
//! Over the kernel's "ping" datagram sockets (`SOCK_DGRAM`,
//! `IPPROTO_ICMP`/`IPPROTO_ICMPV6`) where it can: the kernel picks the
//! identifier, fills in the checksum and hands a connected socket only the
//! replies to its own echoes, so there is nothing to filter and no IP header
//! to parse. Those are governed by `net.ipv4.ping_group_range`, which does
//! **not** exempt root - systemd's default opens it to every group, but a
//! host with the kernel's own `1 0` refuses even uid 0 - so a refused one
//! falls back to a raw socket, which the agent may open as root
//! (`CAP_NET_RAW`). There the identifier is ours, the IPv4 checksum is ours
//! (the kernel does ICMPv6's), and an IPv4 reply comes with its IP header.
//!
//! Every wait is under `within()`: resolving the name, each reply, and the
//! pause between echoes is a plain timer. Nothing here blocks the runtime.

use std::io;
use std::mem::MaybeUninit;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use protocol::{
    PingEvent, PING_DEFAULT_COUNT, PING_DEFAULT_INTERVAL_MS, PING_DEFAULT_TIMEOUT_MS,
    PING_MAX_COUNT, PING_MAX_TIMEOUT_MS, PING_MIN_INTERVAL_MS,
};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::deadline::within;
use crate::log::Log;

/// getaddrinfo, on its blocking thread. glibc gives up within about 10s
/// itself (`timeout:5 attempts:2`); the agent stops waiting before that.
const RESOLVE: Duration = Duration::from_secs(5);

/// The echo payload, iputils' default size.
const PAYLOAD: usize = 56;

/// What to ping, validated before anything is sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub host: String,
    pub count: u32,
    pub interval: Duration,
    pub timeout: Duration,
    pub interface: Option<String>,
}

impl Plan {
    pub fn new(
        host: String,
        count: Option<u32>,
        interval_ms: Option<u64>,
        timeout_ms: Option<u64>,
        interface: Option<String>,
    ) -> Result<Self, String> {
        let host = host.trim().to_string();
        if host.is_empty() || host.len() > 253 || host.chars().any(|ch| ch.is_control()) {
            return Err("ping needs a host name or an address".to_string());
        }
        let count = count.unwrap_or(PING_DEFAULT_COUNT);
        if !(1..=PING_MAX_COUNT).contains(&count) {
            return Err(format!("count must be between 1 and {PING_MAX_COUNT}"));
        }
        let interval = interval_ms.unwrap_or(PING_DEFAULT_INTERVAL_MS);
        if interval < PING_MIN_INTERVAL_MS {
            return Err(format!(
                "the interval must be at least {PING_MIN_INTERVAL_MS}ms"
            ));
        }
        let timeout = timeout_ms.unwrap_or(PING_DEFAULT_TIMEOUT_MS);
        if !(1..=PING_MAX_TIMEOUT_MS).contains(&timeout) {
            return Err(format!(
                "the timeout must be between 1 and {PING_MAX_TIMEOUT_MS}ms"
            ));
        }
        if let Some(name) = &interface {
            check_interface(name)?;
        }
        Ok(Self {
            host,
            count,
            interval: Duration::from_millis(interval),
            timeout: Duration::from_millis(timeout),
            interface,
        })
    }

    /// The longest the whole run can take, for the server's deadline on it.
    pub fn total(&self) -> Duration {
        RESOLVE + (self.interval + self.timeout) * self.count + Duration::from_secs(5)
    }
}

/// An interface name the kernel could have: `SO_BINDTODEVICE` takes it raw.
pub fn check_interface(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 15
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "-_.:".contains(ch))
    {
        return Err(format!("{name} is not an interface name"));
    }
    Ok(())
}

pub type Step = Result<PingEvent, String>;

/// Run `plan` on a task of its own and stream what happens. The task stops
/// at the next echo once nobody is listening.
pub fn start(plan: Plan, log: Arc<Log>) -> mpsc::Receiver<Step> {
    let (tx, rx) = mpsc::channel(8);
    tokio::spawn(async move {
        run(plan, &tx, &log).await; // naked: every wait in run() is under within()
    });
    rx
}

async fn run(plan: Plan, tx: &mpsc::Sender<Step>, log: &Log) {
    // naked: bounded channel the server drains; a receiver that is gone fails at once
    let send = |step: Step| async move { tx.send(step).await.is_ok() };

    // naked: resolve() is under within()
    let address = match resolve(&plan.host).await {
        Ok(address) => address,
        Err(err) => {
            // naked: see send above
            send(Err(err)).await;
            return;
        }
    };
    log.debug(format!("net ping: {} ({address})", plan.host));
    let start = PingEvent::Start {
        host: plan.host.clone(),
        address: address.to_string(),
    };
    // naked: see send above
    if !send(Ok(start)).await {
        return;
    }

    let pinger = match Pinger::open(address, plan.interface.as_deref()) {
        Ok(pinger) => pinger,
        Err(err) => {
            // naked: see send above
            send(Err(err)).await;
            return;
        }
    };

    let mut rtts = Vec::new();
    for seq in 1..=plan.count {
        if seq > 1 {
            // naked: the pause between echoes is a plain timer
            tokio::time::sleep(plan.interval).await;
        }
        let seq = seq as u16;
        // naked: echo() waits under within(timeout)
        let event = match pinger.echo(seq, plan.timeout).await {
            Ok(Some(rtt)) => {
                rtts.push(rtt);
                PingEvent::Reply {
                    seq,
                    bytes: PAYLOAD + 8,
                    rtt_ms: millis(rtt),
                }
            }
            Ok(None) => PingEvent::Timeout { seq },
            Err(err) => {
                // naked: see send above
                send(Err(err)).await;
                return;
            }
        };
        // naked: see send above
        if !send(Ok(event)).await {
            return;
        }
    }

    let summary = summarize(plan.count, &rtts);
    // naked: see send above
    send(Ok(summary)).await;
}

pub fn summarize(sent: u32, rtts: &[Duration]) -> PingEvent {
    let ms: Vec<f64> = rtts.iter().copied().map(millis).collect();
    let min = ms.iter().copied().reduce(f64::min);
    let max = ms.iter().copied().reduce(f64::max);
    let avg = (!ms.is_empty()).then(|| ms.iter().sum::<f64>() / ms.len() as f64);
    PingEvent::Summary {
        sent,
        received: rtts.len() as u32,
        min_ms: min,
        avg_ms: avg,
        max_ms: max,
    }
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1000.0 * 1000.0).round() / 1000.0
}

/// An address, or the first one a name resolves to.
pub async fn resolve(host: &str) -> Result<IpAddr, String> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(address) = bare.parse::<IpAddr>() {
        return Ok(address);
    }
    let lookup = tokio::net::lookup_host((host, 0));
    match within("DNS", RESOLVE, lookup).await {
        Ok(Ok(mut addresses)) => addresses
            .next()
            .map(|address| address.ip())
            .ok_or_else(|| format!("{host}: no address")),
        Ok(Err(err)) => Err(format!("{host}: {err}")),
        Err(expired) => Err(format!("{host}: {expired}")),
    }
}

/// Whether `address` answers one echo within `timeout`: the gateway check.
pub async fn answers(address: IpAddr, interface: Option<&str>, timeout: Duration) -> bool {
    let Ok(pinger) = Pinger::open(address, interface) else {
        return false;
    };
    // naked: echo() waits under within(timeout)
    let reply = pinger.echo(1, timeout).await;
    matches!(reply, Ok(Some(_)))
}

/// One connected ping socket.
struct Pinger {
    socket: AsyncFd<Socket>,
    v6: bool,
    /// Set on a raw socket, where the identifier is ours to pick and check.
    raw: Option<u16>,
}

impl Pinger {
    fn open(address: IpAddr, interface: Option<&str>) -> Result<Self, String> {
        let (domain, protocol) = match address {
            IpAddr::V4(_) => (Domain::IPV4, Protocol::ICMPV4),
            IpAddr::V6(_) => (Domain::IPV6, Protocol::ICMPV6),
        };
        let fail = |err: io::Error| format!("ping {address}: {err}");
        let (socket, raw) = match Socket::new(domain, Type::DGRAM, Some(protocol)) {
            Ok(socket) => (socket, None),
            Err(err) if matches!(err.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM)) => {
                let socket = Socket::new(domain, Type::RAW, Some(protocol)).map_err(fail)?;
                (socket, Some(identifier()))
            }
            Err(err) => return Err(fail(err)),
        };
        socket.set_nonblocking(true).map_err(fail)?;
        if let Some(name) = interface {
            socket
                .bind_device(Some(name.as_bytes()))
                .map_err(|err| format!("ping on {name}: {err}"))?;
        }
        // Connecting a datagram socket only sets the peer - no packet goes
        // out - so it cannot wait on anything.
        socket
            .connect(&SockAddr::from(SocketAddr::new(address, 0)))
            .map_err(fail)?;
        let socket = AsyncFd::new(socket).map_err(fail)?;
        Ok(Self {
            socket,
            v6: address.is_ipv6(),
            raw,
        })
    }

    /// Send echo `seq` and wait for its reply. `None` when none came in time.
    async fn echo(&self, seq: u16, timeout: Duration) -> Result<Option<Duration>, String> {
        let mut packet = echo_request(self.v6, seq);
        if let Some(id) = self.raw {
            packet[4..6].copy_from_slice(&id.to_be_bytes());
            if !self.v6 {
                let sum = checksum(&packet);
                packet[2..4].copy_from_slice(&sum.to_be_bytes());
            }
        }
        let sent = Instant::now();
        self.socket
            .get_ref()
            .send(&packet)
            .map_err(|err| format!("ping: sending: {err}"))?;

        let reply = async {
            loop {
                // naked: bounded by the within() below
                let mut ready = self.socket.readable().await?;
                let mut buf = [MaybeUninit::<u8>::uninit(); 1500];
                match ready.try_io(|socket| socket.get_ref().recv(&mut buf)) {
                    Ok(Ok(len)) => {
                        // SAFETY: recv initialised the first `len` bytes.
                        let bytes: Vec<u8> = buf[..len]
                            .iter()
                            .map(|b| unsafe { b.assume_init() })
                            .collect();
                        let icmp = match self.raw {
                            // A raw IPv4 socket hands over the IP header too.
                            Some(_) if !self.v6 => strip_ipv4(&bytes),
                            _ => Some(bytes.as_slice()),
                        };
                        let ours = match self.raw {
                            Some(id) => icmp.is_some_and(|icmp| {
                                icmp.len() >= 8 && icmp[4..6] == id.to_be_bytes()
                            }),
                            None => true,
                        };
                        if ours && icmp.is_some_and(|icmp| is_reply(self.v6, icmp, seq)) {
                            return Ok::<_, io::Error>(sent.elapsed());
                        }
                        // A late reply to an earlier echo: keep waiting.
                    }
                    Ok(Err(err)) => return Err(err),
                    Err(_would_block) => continue,
                }
            }
        };
        match within("the echo reply", timeout, reply).await {
            Ok(Ok(rtt)) => Ok(Some(rtt)),
            // An unreachable host is reported as an ICMP error on the socket,
            // which is an answer of sorts: no reply is coming.
            Ok(Err(err)) if is_unreachable(&err) => Ok(None),
            Ok(Err(err)) => Err(format!("ping: {err}")),
            Err(_) => Ok(None),
        }
    }
}

fn is_unreachable(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EHOSTUNREACH) | Some(libc::ENETUNREACH) | Some(libc::ECONNREFUSED)
    )
}

/// An echo request: type, code, checksum (the kernel's), identifier (also
/// the kernel's), sequence, then the payload.
fn echo_request(v6: bool, seq: u16) -> Vec<u8> {
    let mut packet = vec![0u8; 8 + PAYLOAD];
    packet[0] = if v6 { 128 } else { 8 };
    packet[6..8].copy_from_slice(&seq.to_be_bytes());
    for (at, byte) in packet[8..].iter_mut().enumerate() {
        *byte = at as u8;
    }
    packet
}

/// An identifier for a raw socket's echoes: the process id is what ping(8)
/// uses, mixed with the clock so two probes in one agent differ.
fn identifier() -> u16 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.subsec_nanos())
        .unwrap_or(0);
    (std::process::id() ^ nanos) as u16
}

/// The internet checksum (RFC 1071) over an ICMP message.
fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = bytes
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// The ICMP message inside an IPv4 packet, past a header of any length.
fn strip_ipv4(packet: &[u8]) -> Option<&[u8]> {
    let length = usize::from(packet.first()? & 0x0f) * 4;
    packet.get(length..).filter(|_| length >= 20)
}

/// An echo reply to `seq`: the ICMP message alone, with no IP header in
/// front of it.
fn is_reply(v6: bool, bytes: &[u8], seq: u16) -> bool {
    let reply = if v6 { 129 } else { 0 };
    bytes.len() >= 8 && bytes[0] == reply && bytes[6..8] == seq.to_be_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_are_checked() {
        let plan = Plan::new("10.0.2.2".into(), None, None, None, None).unwrap();
        assert_eq!(plan.count, PING_DEFAULT_COUNT);
        assert!(Plan::new(" ".into(), None, None, None, None).is_err());
        assert!(Plan::new("h".into(), Some(0), None, None, None).is_err());
        assert!(Plan::new("h".into(), Some(101), None, None, None).is_err());
        assert!(Plan::new("h".into(), None, Some(50), None, None).is_err());
        assert!(Plan::new("h".into(), None, None, Some(60_000), None).is_err());
        assert!(Plan::new("h".into(), None, None, None, Some("eth0".into())).is_ok());
        assert!(Plan::new("h".into(), None, None, None, Some("a b".into())).is_err());
        assert!(plan.total() > Duration::from_secs(4 * 3));
    }

    #[test]
    fn requests_and_replies() {
        let request = echo_request(false, 0x0102);
        assert_eq!(request[0], 8);
        assert_eq!(&request[6..8], &[1, 2]);
        assert_eq!(request.len(), 64);
        assert_eq!(echo_request(true, 1)[0], 128);

        let mut reply = request.clone();
        reply[0] = 0;
        assert!(is_reply(false, &reply, 0x0102));
        assert!(!is_reply(false, &reply, 0x0103));
        assert!(!is_reply(false, &request, 0x0102));
        assert!(!is_reply(false, &reply[..4], 0x0102));
        let mut reply6 = echo_request(true, 7);
        reply6[0] = 129;
        assert!(is_reply(true, &reply6, 7));
    }

    #[test]
    fn raw_socket_helpers() {
        // RFC 1071's own example.
        let example = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&example), !0xddf2);
        // A message with its checksum filled in sums to zero.
        let mut packet = echo_request(false, 9);
        let sum = checksum(&packet);
        packet[2..4].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(checksum(&packet), 0);

        let mut ip = vec![0x45u8; 20];
        ip.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 9]);
        assert_eq!(strip_ipv4(&ip).map(|icmp| icmp[7]), Some(9));
        assert_eq!(
            strip_ipv4(&[0x41; 30]),
            None,
            "a header under 20 bytes is bogus"
        );
        assert_eq!(strip_ipv4(&[]), None);
    }

    #[test]
    fn a_summary_of_nothing_has_no_times() {
        assert_eq!(
            summarize(3, &[]),
            PingEvent::Summary {
                sent: 3,
                received: 0,
                min_ms: None,
                avg_ms: None,
                max_ms: None
            }
        );
        let PingEvent::Summary {
            received,
            min_ms,
            avg_ms,
            max_ms,
            ..
        } = summarize(3, &[Duration::from_millis(1), Duration::from_millis(3)])
        else {
            panic!("not a summary")
        };
        assert_eq!(received, 2);
        assert_eq!(min_ms, Some(1.0));
        assert_eq!(avg_ms, Some(2.0));
        assert_eq!(max_ms, Some(3.0));
    }

    /// The loopback always answers; needs ping sockets allowed for this
    /// user, which systemd's default `ping_group_range` does.
    #[tokio::test]
    #[ignore]
    async fn the_loopback_answers() {
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(answers(loopback, None, Duration::from_secs(1)).await);
    }
}
