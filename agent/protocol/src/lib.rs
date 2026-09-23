//! The Tessaro control protocol.
//!
//! One command model, every transport an encoding of it (TODO.md item 8).
//! Today there are two transports and one encoding: newline-delimited JSON
//! over the local unix socket, and the same over TLS on TCP.
//!
//! A conversation is:
//!
//! ```text
//! client: {"protocol":1,"client":"tessaro-ctl 1.0.0"}              Hello
//! server: {"type":"welcome","protocol":1,"node":{...}}             Frame::Welcome
//! client: {"id":1,"token":"tsr_...","command":{"cmd":"status"}}   Request
//! server: {"type":"ok","id":1,"result":{...}}                      Frame::Ok
//! ```
//!
//! Requests on one connection are answered in order. A streaming command
//! (`logs`, `speedtest`) answers with `event` frames and ends with `end`.
//! `token` is only looked at over TCP; the local socket is root-only and
//! needs none.

pub mod keys;

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped on any change a client of the previous version would misread.
pub const PROTOCOL_VERSION: u32 = 1;

pub const DEFAULT_PORT: u16 = 7400;
pub const DEFAULT_SOCKET: &str = "/run/tessaro-agent.sock";

/// mDNS service type, in the fully qualified form mdns-sd expects.
pub const SERVICE_TYPE: &str = "_tessaro._tcp.local.";

/// How long a guarded change (`display.resolution`) waits for `confirm`
/// before it reverts itself.
pub const CONFIRM_SECONDS: u64 = 60;

/// Longest line either side accepts. A screenshot is the largest thing on
/// the wire; a 4K JPEG in base64 fits with room to spare.
pub const MAX_LINE: usize = 32 * 1024 * 1024;

/// Largest piece of an image upload, before base64. Small enough that one
/// request answers well inside the client's timeout on a slow link, large
/// enough that the round trips do not dominate on a fast one.
pub const UPDATE_CHUNK: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    pub client: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    /// The app-specific id derived from /etc/machine-id. Never the machine id.
    pub id: String,
    pub name: String,
    pub version: String,
    pub machine: String,
    /// SHA-256 of the device's TLS certificate, lower-case hex.
    pub fingerprint: String,
    pub claimed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    Browser,
    Weston,
    Agent,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Command {
    Status,
    Id,
    Keys,
    /// Output modes every connected connector advertises.
    Modes,
    /// The network as the device sees it: addresses, route, DNS,
    /// interfaces. Read-only.
    Net,
    Get {
        #[serde(default)]
        key: Option<String>,
    },
    Set {
        values: BTreeMap<String, String>,
        #[serde(default)]
        if_revision: Option<u64>,
        #[serde(default = "yes")]
        apply: bool,
    },
    Unset {
        keys: Vec<String>,
        #[serde(default)]
        if_revision: Option<u64>,
        #[serde(default = "yes")]
        apply: bool,
    },
    /// Keep a guarded change that is on probation.
    Confirm,
    Navigate {
        url: String,
    },
    Restart {
        what: Target,
    },
    Reboot,
    Screenshot,
    Logs {
        #[serde(default)]
        follow: bool,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        lines: Option<u32>,
    },
    /// Take an unclaimed device. TCP only.
    Claim {
        name: String,
    },
    TokenCreate {
        name: String,
    },
    TokenList,
    TokenRevoke {
        id: String,
    },
    /// `None` generates one and returns it.
    PasswordSet {
        #[serde(default)]
        password: Option<String>,
    },
    Unclaim,
    FactoryReset,
    /// Start, or resume, uploading an image: the `.wic.bz2` is described
    /// here and sent in `UpdateChunk`s. The answer says where to resume.
    UpdateBegin {
        name: String,
        size: u64,
        /// SHA-256 of the whole file, lower-case hex.
        sha256: String,
        /// The `.wic.bmap`, verbatim.
        bmap: String,
        /// Check the whole upload against `sha256` before preparing it.
        /// The bmap's per-range checksums are checked either way.
        #[serde(default = "yes")]
        verify: bool,
    },
    /// The next piece of the upload, starting at `offset`, base64. At most
    /// `UPDATE_CHUNK` bytes before encoding.
    UpdateChunk {
        offset: u64,
        data: String,
    },
    UpdateStatus,
    /// Apply the prepared update at the next boot.
    UpdateCommit {
        /// Re-create `/data` too: settings, the claim, the browser profile.
        #[serde(default)]
        wipe_data: bool,
        #[serde(default = "yes")]
        reboot: bool,
    },
    /// Drop the upload or the prepared update, and the pending marker.
    UpdateCancel,
    /// Measure the device's own internet connection against
    /// speed.cloudflare.com. A stream of `SpeedtestEvent`s.
    Speedtest {
        /// Largest payload, one of `SPEEDTEST_SIZES`.
        #[serde(default)]
        max_size: Option<u64>,
        /// Samples per payload size.
        #[serde(default)]
        tests: Option<u32>,
    },
}

impl Command {
    /// Allowed over TCP without a token.
    pub fn is_public(&self) -> bool {
        matches!(self, Command::Id | Command::Claim { .. })
    }
}

/// Everything the server sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Frame {
    Welcome { protocol: u32, node: NodeInfo },
    Ok { id: u64, result: Value },
    Error { id: u64, error: String },
    Event { id: u64, event: Value },
    End { id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub key: String,
    pub value: String,
    pub previous: Option<String>,
    pub seconds_left: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub node: NodeInfo,
    pub revision: u64,
    pub kiosk_url: String,
    pub current_url: Option<String>,
    pub browser_answering: bool,
    pub units: BTreeMap<String, String>,
    pub pending: Option<Pending>,
    /// `PRETTY_NAME` and `IMAGE_VERSION` from the image's os-release, so an
    /// update can be seen to have landed. Defaulted for older devices.
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub image_version: Option<String>,
    /// In maintenance mode `kiosk_url` is the maintenance page's. Defaulted,
    /// so a client still reads an agent that predates it.
    #[serde(default)]
    pub maintenance: bool,
    /// The debug screen is up, whatever `kiosk_url` says. Defaulted the same
    /// way.
    #[serde(default)]
    pub debug_screen: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePhase {
    /// Nothing under way.
    Idle,
    /// Part of the image has arrived.
    Receiving,
    /// All of it has; the device is checking the whole file against its
    /// SHA-256 (`verified` of `size`). Skipped with `verify: false`.
    Verifying,
    /// Checked; the device is staging the boot and root partitions,
    /// checking each bmap range as it goes (`prepared` of `to_prepare`).
    Preparing,
    /// Staged and verified; waiting for `update-commit`.
    Ready,
    /// Committed; applied at the next boot.
    Pending,
    /// Preparing failed; `error` says why. Begin again to retry.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateBegun {
    /// Bytes the device already has. Send from here.
    pub offset: u64,
    pub phase: UpdatePhase,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateReceived {
    pub received: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub phase: UpdatePhase,
    /// The file being uploaded or staged.
    pub name: Option<String>,
    pub size: u64,
    pub received: u64,
    /// Bytes of the upload checked against its SHA-256, of `size`: the first
    /// step of preparing, skipped with `verify: false`. Defaulted for older
    /// devices, which do not report it.
    #[serde(default)]
    pub verified: u64,
    /// Mapped bytes of the boot and root partitions checked and staged, of
    /// `to_prepare`.
    pub prepared: u64,
    pub to_prepare: u64,
    pub error: Option<String>,
    pub wipe_data: bool,
    /// What the last boot that applied an update did.
    pub last: Option<UpdateResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateResult {
    pub applied: bool,
    pub message: String,
    pub source: String,
    pub wiped_data: bool,
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The image's default, from /usr/lib/tessaro-kiosk/tessaro-kiosk.env.
    Default,
    /// Set on this device.
    Set,
    /// Read-only: reported by the device as it is right now.
    Live,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub env: String,
    pub value: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub revision: u64,
    pub settings: Vec<Setting>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    pub revision: u64,
    pub changed: Vec<String>,
    /// Units restarted, or to be restarted once this reply is out.
    pub restarted: Vec<String>,
    pub pending: Option<Pending>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyInfo {
    pub name: String,
    pub env: String,
    pub applies: Vec<keys::Consumer>,
    pub guarded: bool,
    pub doc: String,
    /// What a value may be, in words.
    #[serde(default)]
    pub values: String,
    /// The image's default, if it has one.
    #[serde(default)]
    pub default: Option<String>,
    /// What this device has set, if anything.
    #[serde(default)]
    pub value: Option<String>,
}

impl From<&keys::Key> for KeyInfo {
    fn from(key: &keys::Key) -> Self {
        Self {
            name: key.name.to_string(),
            env: key.env.to_string(),
            applies: key.consumers.to_vec(),
            guarded: key.guarded,
            doc: key.doc.to_string(),
            values: key.kind.describe(),
            default: None,
            value: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connector {
    pub name: String,
    pub modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetAddress {
    pub address: String,
    pub prefix: u8,
    /// `ipv4` or `ipv6`.
    pub family: String,
    /// `global`, `link-local` or `loopback`.
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetInterface {
    pub name: String,
    /// `ethernet`, `wireless`, `loopback`, or `virtual` (bridges, tunnels...).
    pub kind: String,
    pub mac: Option<String>,
    /// The kernel's operstate: `up`, `down`, `dormant`, `unknown`, ...
    pub state: String,
    /// A cable or an association, if the driver says.
    pub carrier: Option<bool>,
    pub mtu: Option<u32>,
    pub speed_mbps: Option<u32>,
    /// Carries the IPv4 default route.
    pub default_route: bool,
    pub addresses: Vec<NetAddress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Net {
    pub hostname: String,
    /// The interface with the IPv4 default route.
    pub interface: Option<String>,
    pub gateway: Option<String>,
    /// The upstream servers, not systemd-resolved's 127.0.0.53 stub.
    pub dns: Vec<String>,
    pub interfaces: Vec<NetInterface>,
    /// The address the internet sees, as the agent last found it through
    /// Cloudflare's trace; `None` until it has. Defaulted for older devices.
    #[serde(default)]
    pub public_ip: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claimed {
    pub token_id: String,
    pub token: String,
    pub root_password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    /// The id of the token that issued it, `claim`, or `local`.
    pub issued_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCreated {
    pub id: String,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Password {
    /// The generated password, when the server chose it. Shown once.
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Screenshot {
    pub format: String,
    /// Base64, as CDP returns it.
    pub data: String,
}

/// The payload sizes a speed test steps through, cfspeedtest's own.
pub const SPEEDTEST_SIZES: [u64; 5] = [100_000, 1_000_000, 10_000_000, 25_000_000, 100_000_000];
pub const SPEEDTEST_DEFAULT_SIZE: u64 = 25_000_000;
pub const SPEEDTEST_DEFAULT_TESTS: u32 = 10;
/// cfspeedtest builds an upload body in memory, so a larger one would be
/// 100 MB of RAM on a device that may have 1 GB for Chromium as well.
pub const SPEEDTEST_UPLOAD_MAX: u64 = 25_000_000;

/// `100k`, `1m`, ... the way `tessaro-ctl speedtest --max-size` spells them.
pub fn speedtest_size_label(size: u64) -> String {
    if size >= 1_000_000 {
        format!("{}m", size / 1_000_000)
    } else {
        format!("{}k", size / 1_000)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Download,
    Upload,
}

/// One step of `speedtest`, in the order they arrive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum SpeedtestEvent {
    /// Where Cloudflare sees the device from.
    Server {
        ip: String,
        /// The Cloudflare data centre answering, as an airport code.
        colo: String,
        country: String,
    },
    /// Round trips of an empty request, less the server's own time.
    Latency {
        samples: u32,
        avg_ms: Option<f64>,
        min_ms: Option<f64>,
        max_ms: Option<f64>,
    },
    /// Every sample of one payload size in one direction.
    Transfer {
        direction: Direction,
        size: u64,
        samples: u32,
        attempts: u32,
        median_mbit: Option<f64>,
        min_mbit: Option<f64>,
        max_mbit: Option<f64>,
    },
    /// The median at the largest size that produced samples: small payloads
    /// never leave slow start and under-report a fast link.
    Result {
        download_mbit: Option<f64>,
        upload_mbit: Option<f64>,
        latency_ms: Option<f64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Done {
    pub message: String,
}

/// One frame, newline terminated.
pub fn to_line<T: Serialize>(value: &T) -> String {
    let mut line = serde_json::to_string(value).expect("protocol types always serialize");
    line.push('\n');
    line
}

pub fn from_line<T: DeserializeOwned>(line: &str) -> Result<T, String> {
    serde_json::from_str(line.trim_end()).map_err(|err| format!("malformed frame: {err}"))
}

/// A root password or a token value a human may have to type. Kept here so
/// the agent's generator and the client's prompt agree on what is valid.
pub fn check_password(password: &str) -> Result<(), String> {
    if password.is_empty() {
        return Err("the password must not be empty".to_string());
    }
    if password.len() > 256 {
        return Err("the password is too long".to_string());
    }
    if password.chars().any(|ch| ch.is_control() || ch == ':') {
        return Err("the password must not contain ':' or control characters".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        let request = Request {
            id: 7,
            token: Some("tsr_x".to_string()),
            command: Command::Set {
                values: [("kiosk.url".to_string(), "https://a.test/".to_string())].into(),
                if_revision: Some(3),
                apply: false,
            },
        };

        let line = to_line(&request);
        assert!(line.ends_with('\n'));
        assert_eq!(from_line::<Request>(&line).unwrap(), request);
    }

    #[test]
    fn the_wire_form_is_readable() {
        let line = to_line(&Request {
            id: 1,
            token: None,
            command: Command::Restart {
                what: Target::Browser,
            },
        });
        assert_eq!(
            line,
            "{\"id\":1,\"command\":{\"cmd\":\"restart\",\"what\":\"browser\"}}\n"
        );
    }

    #[test]
    fn set_applies_unless_told_otherwise() {
        let request: Request =
            from_line(r#"{"id":1,"command":{"cmd":"set","values":{"a":"b"}}}"#).unwrap();
        assert!(matches!(request.command, Command::Set { apply: true, .. }));
    }

    #[test]
    fn frames_round_trip() {
        let frame = Frame::Error {
            id: 2,
            error: "nope".to_string(),
        };
        assert_eq!(from_line::<Frame>(&to_line(&frame)).unwrap(), frame);
    }

    #[test]
    fn only_id_and_claim_are_public() {
        assert!(Command::Id.is_public());
        assert!(Command::Claim { name: "x".into() }.is_public());
        assert!(!Command::Status.is_public());
        assert!(!Command::TokenCreate { name: "x".into() }.is_public());
    }

    #[test]
    fn speedtest_events_are_tagged_by_phase() {
        let event = SpeedtestEvent::Transfer {
            direction: Direction::Upload,
            size: 1_000_000,
            samples: 3,
            attempts: 4,
            median_mbit: Some(42.5),
            min_mbit: Some(40.0),
            max_mbit: Some(44.0),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["phase"], "transfer");
        assert_eq!(value["direction"], "upload");
        assert_eq!(
            serde_json::from_value::<SpeedtestEvent>(value).unwrap(),
            event
        );

        let request: Request = from_line(r#"{"id":1,"command":{"cmd":"speedtest"}}"#).unwrap();
        assert_eq!(
            request.command,
            Command::Speedtest {
                max_size: None,
                tests: None
            }
        );
    }

    #[test]
    fn passwords() {
        assert!(check_password("abc").is_ok());
        assert!(check_password("").is_err());
        assert!(check_password("a:b").is_err());
        assert!(check_password("a\nb").is_err());
    }
}
