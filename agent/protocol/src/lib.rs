//! The Tessaro API: the types on the wire, the endpoints that carry them and
//! the configuration key registry.
//!
//! Every client - tessaro-ctl, tessaro-gui, Webconfig, anyone's own
//! program - speaks the HTTP API that `api` defines, over TLS on TCP or
//! plain over the local unix socket (docs/api.md). `Command` is the agent's
//! own model of what it can be asked: each endpoint maps its request onto
//! one, and so does the page bridge. It never crosses the network.

pub mod api;
pub mod files;
pub mod keys;
pub mod openapi;
pub mod playlist;
pub mod policy;
pub mod sshkey;

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const DEFAULT_PORT: u16 = 7400;
pub const DEFAULT_SOCKET: &str = "/run/tessaro-agent.sock";

/// mDNS service type, in the fully qualified form mdns-sd expects.
pub const SERVICE_TYPE: &str = "_tessaro._tcp.local.";

/// How long a guarded change (`screen.resolution`, `screen.rotation`) waits
/// for `confirm` before it reverts itself.
pub const CONFIRM_SECONDS: u64 = 60;

/// Largest piece of an upload, an image or a file, in one request. Small
/// enough that one request answers well inside the client's timeout on a
/// slow link, large enough that the round trips do not dominate on a fast
/// one.
pub const UPDATE_CHUNK: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NodeInfo {
    /// The app-specific id derived from /etc/machine-id. Never the machine id.
    pub id: String,
    pub name: String,
    pub version: String,
    pub machine: String,
    /// SHA-256 of the device's TLS certificate, lower-case hex.
    pub fingerprint: String,
    pub claimed: bool,
    /// device.tags, sorted. Never `unclaimed`: a client adds that from
    /// `claimed`.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// What `restart` restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RestartTarget {
    Browser,
    Weston,
    Agent,
}

impl RestartTarget {
    /// The wire names, which are also what the command line takes.
    pub const NAMES: [&'static str; 3] = ["browser", "weston", "agent"];
}

impl std::str::FromStr for RestartTarget {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        from_name(name)
    }
}

/// A unit-only enum from its wire name, the way serde spells it.
fn from_name<T: DeserializeOwned>(name: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(name.to_string()))
        .map_err(|_| format!("{name:?} is not one of the accepted values"))
}

fn yes() -> bool {
    true
}

/// A secret on its way to the device: a WiFi password. It serializes as the
/// plain string, but prints as `***`, so a `{:?}` of a command - in a log
/// line, a panic, a test failure - never shows it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct Secret(pub String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

/// What the device checks, on its own, before it keeps a network change.
/// Whatever it is, the change must also leave the device with a default
/// route if it had one, and a connection it activated must come up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "check", rename_all = "kebab-case")]
pub enum Verify {
    /// The default gateway answers a ping.
    #[default]
    Gateway,
    /// This host answers a ping.
    Host { host: String },
    /// A TCP connection to this host and port opens.
    Tcp { host: String, port: u16 },
    /// Only the route and the activation.
    None,
}

impl Verify {
    /// `gateway`, `none`, `HOST` or `HOST:PORT`, the way `--verify` takes it.
    pub fn parse(text: &str) -> Result<Verify, String> {
        match text {
            "" => Err("--verify needs gateway, none, HOST or HOST:PORT".to_string()),
            "gateway" => Ok(Verify::Gateway),
            "none" => Ok(Verify::None),
            _ => {
                // A bare IPv6 address has colons of its own; `[ADDR]:PORT`
                // is how it takes a port.
                if let Ok(address) = text.parse::<std::net::SocketAddr>() {
                    return Ok(Verify::Tcp {
                        host: address.ip().to_string(),
                        port: address.port(),
                    });
                }
                if text.parse::<std::net::IpAddr>().is_ok() {
                    return Ok(Verify::Host {
                        host: text.to_string(),
                    });
                }
                match text.rsplit_once(':') {
                    Some((host, port)) if !host.is_empty() => {
                        let port = port
                            .parse::<u16>()
                            .map_err(|_| format!("{text}: bad port"))?;
                        Ok(Verify::Tcp {
                            host: host.to_string(),
                            port,
                        })
                    }
                    _ => Ok(Verify::Host {
                        host: text.to_string(),
                    }),
                }
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Verify::Gateway => "the gateway answers".to_string(),
            Verify::Host { host } => format!("{host} answers"),
            Verify::Tcp { host, port } => format!("{host}:{port} accepts a connection"),
            Verify::None => "no reachability check".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum WifiSecurity {
    Open,
    /// WPA2 personal.
    Psk,
    /// WPA3 personal.
    Sae,
}

impl WifiSecurity {
    /// The wire names, which are also what the command line takes.
    pub const NAMES: [&'static str; 3] = ["open", "psk", "sae"];
}

impl std::str::FromStr for WifiSecurity {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        from_name(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
        /// What the device checks before it keeps a change to network keys.
        #[serde(default)]
        verify: Verify,
    },
    Unset {
        keys: Vec<String>,
        #[serde(default)]
        if_revision: Option<u64>,
        #[serde(default = "yes")]
        apply: bool,
        #[serde(default)]
        verify: Verify,
    },
    /// Keep the guarded changes that are on probation.
    Confirm,
    Navigate {
        url: String,
    },
    Restart {
        what: RestartTarget,
    },
    Reboot,
    Screenshot,
    /// Reload the page on screen, past the cache.
    Reload,
    /// Empty the browser's HTTP cache.
    ClearCache,
    /// Run JavaScript in the page on screen, now. At most `EVAL_MAX` bytes;
    /// `timeout_ms` defaults to `EVAL_TIMEOUT_MS` and is held to
    /// `EVAL_TIMEOUT_MAX_MS`.
    Eval {
        code: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// Wait for a returned Promise and answer with what it settles to.
        #[serde(default = "yes")]
        await_promise: bool,
        /// Run as if the user had just touched the page: what audio,
        /// fullscreen and a focused field's keyboard need.
        #[serde(default)]
        user_gesture: bool,
    },
    /// The stored browser policies (`policy`), without their text.
    BrowserPolicyList,
    /// One stored browser policy, its text as typed.
    BrowserPolicyGet {
        name: String,
    },
    /// Store a browser policy under `name`, replacing one of that name, and
    /// merge it into the policy Chromium reads. With `if_revision`, only
    /// while the stored one is still at that revision; `""` only while
    /// there is none. A new one goes to the bottom, the lowest priority,
    /// unless `position` places it; a stored one keeps its place unless
    /// `position` moves it.
    BrowserPolicySet {
        name: String,
        text: String,
        #[serde(default)]
        if_revision: Option<String>,
        #[serde(default)]
        position: Option<u32>,
    },
    /// Move a browser policy to `position` (from 1, the highest priority),
    /// the others shifting to make room.
    BrowserPolicyMove {
        name: String,
        position: u32,
    },
    BrowserPolicyRemove {
        name: String,
    },
    /// The merged policy Chromium reads, each entry with where it comes from.
    BrowserPolicyEffective,
    /// Show or hide the on-screen keyboard. Showing focuses `selector`, or
    /// the field that already has the focus.
    Keyboard {
        show: bool,
        #[serde(default)]
        selector: Option<String>,
    },
    /// Switch the display off or on; `None` only reports which it is.
    ScreenPower {
        #[serde(default)]
        on: Option<bool>,
    },
    /// A page of the journal: the last `lines` entries, or with `cursor`
    /// everything after it. A `LogPage`; following is asking again with its
    /// cursor.
    Logs {
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        lines: Option<u32>,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// What the welcome page shows: the setup QR code and address, whether
    /// the device is online. Asking keeps the online check running.
    Welcome,
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
    /// Add a public key to root's `authorized_keys`, one `.pub` line. The
    /// answer carries the device's SSH host keys, so the client can check
    /// them without trusting on first use.
    SshAuthorize {
        key: String,
    },
    SshKeyList,
    /// Remove one key: its `SHA256:` fingerprint, a unique prefix of it,
    /// or its exact comment.
    SshKeyRevoke {
        key: String,
    },
    /// Start, or resume, uploading an image: the `.wic.zst` is described
    /// here and sent in `UpdateChunk`s. The answer says where to resume.
    UpdateBegin(ImageUpload),
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
        /// Go around network.proxy.url, straight to Cloudflare: the link
        /// itself rather than the proxy. Nothing without a proxy.
        #[serde(default)]
        direct: bool,
    },
    /// One round trip and nothing else, for `tessaro-ctl device ping`. Public, like
    /// `id`: it says no more than that the agent is answering.
    Ping,
    /// NetworkManager's profiles: the ones the device manages, and any made
    /// by hand.
    NetProfiles,
    /// One profile's addressing, DNS and WiFi settings, never its secrets.
    NetShow {
        profile: String,
    },
    /// What the last network change did, for a client whose connection went
    /// with the change.
    NetLast,
    /// The WiFi radio and what each WiFi device is connected to.
    Wifi,
    WifiScan {
        #[serde(default)]
        interface: Option<String>,
        /// Ask the device to scan first. Without it, what it last saw.
        #[serde(default = "yes")]
        rescan: bool,
    },
    /// Join a network in client mode: `network.wifi.mode=client`, `network.wifi.ssid`,
    /// `network.wifi.security` and `network.wifi.hidden` in one change, with the password
    /// stored where `get` never shows it. The hotspot goes down.
    WifiJoin {
        ssid: String,
        /// `None` keeps the stored one: rejoining the same network.
        #[serde(default)]
        psk: Option<Secret>,
        /// Found by scanning when not given; a hidden network needs it.
        #[serde(default)]
        security: Option<WifiSecurity>,
        #[serde(default)]
        hidden: bool,
        #[serde(default)]
        verify: Verify,
    },
    /// A new random hotspot password, shown once. Claimed devices only.
    HotspotPassword,
    /// Ping a host from the device. A stream of `PingEvent`s.
    NetPing {
        host: String,
        #[serde(default)]
        count: Option<u32>,
        #[serde(default)]
        interval_ms: Option<u64>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        interface: Option<String>,
    },
    /// What is stored in `path` in `/data/files`, like `ls`: a directory's
    /// own entries, or with `recursive` everything under it. A
    /// `FilesListing`; a file lists itself.
    FilesList {
        #[serde(default)]
        path: String,
        #[serde(default)]
        recursive: bool,
    },
    /// Start, or resume, storing one file: described here, sent in
    /// `FilesChunk`s, put in place with `mtime` once the last byte is in.
    /// The answer says where to resume.
    FilesBegin {
        path: String,
        size: u64,
        mtime: i64,
    },
    /// The next piece of the file begun for `path`, starting at `offset`,
    /// base64. At most `UPDATE_CHUNK` bytes before encoding.
    FilesChunk {
        path: String,
        offset: u64,
        data: String,
    },
    /// Up to `len` bytes of a stored file from `offset`: a `FileData`.
    FilesRead {
        path: String,
        #[serde(default)]
        offset: u64,
        len: u64,
    },
    /// Make a directory, and any missing above it.
    FilesMkdir {
        path: String,
    },
    /// Move or rename a file or a directory, like `mv`: into `to` if that is
    /// a directory already, otherwise to `to` itself, replacing a file there
    /// and making any missing directory above it.
    FilesMove {
        from: String,
        to: String,
    },
    /// Remove files, or directories with everything in them if `recursive`.
    FilesDelete {
        paths: Vec<String>,
        #[serde(default)]
        recursive: bool,
    },
    /// What PipeWire has - every output and input - and which of them the
    /// audio.* settings put in use.
    AudioStatus,
    /// A short tone on the output in use, or with `input`, a few seconds
    /// recorded from the input in use and its level.
    AudioTest {
        #[serde(default)]
        input: bool,
    },
    /// Every USB camera, what its mirror captures, and the virtual camera
    /// pages and everything else read it through. Read-only.
    CameraList,
    /// The newest frame of the camera whose node is `device` (`video0`), as
    /// a JPEG. Read-only.
    CameraSnapshot {
        device: String,
    },
    /// The disk the device runs from: its partitions and how full each
    /// filesystem is. Read-only.
    Storage,
    /// Grow `/data` over the unallocated space at the end of the disk, while
    /// it stays mounted. A stream of `StorageGrowEvent`s: the plan first,
    /// then, unless `check`, one per step and the result.
    StorageGrow {
        #[serde(default)]
        check: bool,
    },
    /// The clock as systemd-timedated and systemd-timesyncd report it: the
    /// timezone, whether it is in sync, and the last NTP exchange. Read-only.
    TimeStatus,
    /// Every timezone the device's tz database has, for time.timezone.
    TimeZones,
    /// Restart systemd-timesyncd, so it asks its servers again now.
    TimeSync,
    /// Set the clock by hand, with time.ntp.enable off: to `usec`
    /// (microseconds since the epoch, UTC), or to `local`, a
    /// `YYYY-MM-DD HH:MM[:SS]` read in the device's timezone.
    TimeSet {
        #[serde(default)]
        usec: Option<u64>,
        #[serde(default)]
        local: Option<String>,
    },
    /// The proxy: network.proxy.url masked, the bypass list and the local
    /// proxy's unit. Read-only.
    ProxyStatus,
    /// Cloudflare's trace fetched through the proxy: the address the
    /// internet sees through it, or why it could not be reached.
    ProxyTest,
    /// Trust the certificate authorities in `pem`, one or more PEM
    /// certificates, in Chromium and the agent's own TLS client.
    NetCertAdd {
        pem: String,
    },
    /// The extra certificate authorities the device trusts.
    NetCertList,
    /// Stop trusting one of them: its SHA-256 fingerprint, a unique prefix
    /// of it, or its exact subject.
    NetCertRevoke {
        cert: String,
    },
    /// Every script, with its recent runs.
    ScriptList,
    ScriptCreate {
        spec: ScriptSpec,
    },
    /// Change what is given of one script, by id or name.
    ScriptSet {
        script: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        on_error: Option<OnError>,
        /// `Some(0)` removes the timeout.
        #[serde(default)]
        timeout_s: Option<u64>,
        #[serde(default)]
        concurrency: Option<Concurrency>,
        #[serde(default)]
        bridge: Option<bool>,
    },
    /// Refused while a schedule runs it.
    ScriptRemove {
        script: String,
    },
    /// Run a script now and follow it: a stream of `ScriptEvent`s, ending
    /// with how the run ended.
    ScriptRun {
        script: String,
    },
    /// Every schedule, with when it runs next and how its last run ended.
    ScheduleList,
    /// Add a schedule. Its calendar is checked with systemd before
    /// anything is saved.
    ScheduleCreate {
        spec: ScheduleSpec,
    },
    /// Change what is given of one schedule, by id or name. A given
    /// `calendar` replaces the whole list.
    ScheduleSet {
        schedule: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        calendar: Option<Vec<String>>,
        /// The script it runs, by id or name.
        #[serde(default)]
        script: Option<String>,
        #[serde(default)]
        enabled: Option<bool>,
    },
    ScheduleRemove {
        schedule: String,
    },
    /// Check `OnCalendar` expressions with systemd, saving nothing: the
    /// next `count` times they fire, merged, or why one is refused.
    ScheduleCheck {
        calendar: Vec<String>,
        #[serde(default)]
        count: Option<u32>,
    },
    /// Every playlist and where it is used.
    PlaylistList,
    /// One playlist, its items in full.
    PlaylistShow {
        playlist: String,
    },
    PlaylistCreate {
        spec: playlist::PlaylistSpec,
    },
    /// Change what is given of one playlist, by id or name. Given `items`
    /// replace them all.
    PlaylistSet {
        playlist: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        transition: Option<playlist::Transition>,
        #[serde(default)]
        transition_ms: Option<u32>,
        #[serde(default)]
        items: Option<Vec<playlist::PlaylistItem>>,
    },
    /// Refused while it is `playlist.default` or in the timetable.
    PlaylistRemove {
        playlist: String,
    },
    /// Add an item at `at` (from 1), at the end without one.
    PlaylistItemAdd {
        playlist: String,
        item: playlist::PlaylistItem,
        #[serde(default)]
        at: Option<u32>,
    },
    /// Replace the item at `position` (from 1).
    PlaylistItemSet {
        playlist: String,
        position: u32,
        item: playlist::PlaylistItem,
    },
    PlaylistItemRemove {
        playlist: String,
        position: u32,
    },
    /// Move the item at `position` to `to`, both from 1.
    PlaylistItemMove {
        playlist: String,
        position: u32,
        to: u32,
    },
    /// What the player is doing.
    PlaylistStatus,
    /// Every timetable entry, in order.
    TimetableList,
    TimetableCreate {
        spec: playlist::TimetableSpec,
    },
    /// Change what is given of one entry, by id.
    TimetableSet {
        entry: String,
        #[serde(default)]
        playlist: Option<String>,
        #[serde(default)]
        days: Option<Vec<playlist::Day>>,
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        to: Option<String>,
        #[serde(default)]
        priority: Option<i32>,
        #[serde(default)]
        enabled: Option<bool>,
    },
    TimetableRemove {
        entry: String,
    },
    /// Every printer, the default one, and whether pages may print.
    PrinterList,
    /// One printer in full: its state, what CUPS says about it, its
    /// supplies when it reports them.
    PrinterShow {
        printer: String,
    },
    /// Printers CUPS finds on USB and the network, each with the URI
    /// `printer-create` takes. A stream of `PrinterFound`s.
    PrinterDiscover,
    /// Add a printer. A driverless one is asked what it supports now, so it
    /// has to answer.
    PrinterCreate {
        spec: PrinterSpec,
    },
    PrinterRemove {
        printer: String,
    },
    /// The printer `window.print()` and a bridge call without a printer
    /// print to.
    PrinterDefault {
        printer: String,
    },
    /// A test page: a PDF for a driverless printer, a few lines of text for
    /// a raw one.
    PrinterTest {
        printer: String,
    },
    /// Print a document: `data`, base64, or `path`, a file in the store.
    PrinterPrint {
        printer: String,
        #[serde(default)]
        data: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        copies: Option<u32>,
        #[serde(default)]
        media: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    /// The jobs not yet printed, of one printer or all of them.
    PrinterJobs {
        #[serde(default)]
        printer: Option<String>,
    },
    PrinterCancel {
        job: String,
    },
}

/// The image `update-begin` describes. Its fields sit in the command itself
/// on the wire, next to `cmd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImageUpload {
    pub name: String,
    pub size: u64,
    /// SHA-256 of the whole file, lower-case hex.
    pub sha256: String,
    /// The `.wic.bmap`, verbatim.
    pub bmap: String,
    /// Check the whole upload against `sha256` before preparing it.
    /// The bmap's per-range checksums are checked either way.
    #[serde(default = "yes")]
    pub verify: bool,
    /// Write the whole disk (partition table, every partition, `/data`)
    /// instead of the root partition: for a device on another disk
    /// layout. Implies wiping `/data`; a power cut while it writes needs
    /// a physical reflash. An older device ignores it and refuses the
    /// layout as before.
    #[serde(default)]
    pub repartition: bool,
}

impl Command {
    /// Run as a job (`api::Action::Start`): its steps are kept on the
    /// device and polled, not answered at once. The device bounds every
    /// one of them in time.
    pub fn is_job(&self) -> bool {
        matches!(
            self,
            Command::Speedtest { .. }
                | Command::NetPing { .. }
                | Command::StorageGrow { .. }
                | Command::PrinterDiscover
                | Command::ScriptRun { .. }
        )
    }
}

/// A page of the journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LogPage {
    /// journalctl's JSON entries, oldest first: `MESSAGE`,
    /// `SYSLOG_IDENTIFIER`, `PRIORITY`, `__REALTIME_TIMESTAMP` and the rest.
    #[schemars(with = "Vec<JournalEntry>")]
    pub entries: Vec<Value>,
    /// Where the next page starts: send it back as `cursor` to get only
    /// what came after. The one sent when nothing new came, `None` for an
    /// empty journal.
    pub cursor: Option<String>,
}

/// One journal entry as journalctl writes it: the fields a client reads,
/// and whatever else the entry has. Only described, for the API's
/// document: the entries pass through as they are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JournalEntry {
    #[serde(rename = "MESSAGE", default)]
    pub message: Option<JournalText>,
    #[serde(rename = "SYSLOG_IDENTIFIER", default)]
    pub syslog_identifier: Option<String>,
    /// `0` (emergency) to `7` (debug), as a string.
    #[serde(rename = "PRIORITY", default)]
    pub priority: Option<String>,
    /// Microseconds since the epoch, as a string.
    #[serde(rename = "__REALTIME_TIMESTAMP", default)]
    pub realtime_timestamp: Option<String>,
    #[serde(rename = "_SYSTEMD_UNIT", default)]
    pub systemd_unit: Option<String>,
    #[serde(flatten)]
    pub rest: BTreeMap<String, Value>,
}

/// A journal field: text, or its bytes when it is not UTF-8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum JournalText {
    Text(String),
    Bytes(Vec<u8>),
}

/// One step of a job, of the kind the job is. Only described, for the
/// API's document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum JobEvent {
    Ping(PingEvent),
    Speedtest(SpeedtestEvent),
    StorageGrow(StorageGrowEvent),
    PrinterFound(PrinterFound),
    Script(ScriptEvent),
}

/// A job the device started: poll it at `/api/v1/jobs/{job}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobStarted {
    pub job: String,
}

/// What a job did since `after`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobPage {
    /// Its steps from number `after` on, in order: `PingEvent`s,
    /// `SpeedtestEvent`s, `StorageGrowEvent`s, `PrinterFound`s or
    /// `ScriptEvent`s, by the job.
    #[schemars(with = "Vec<JobEvent>")]
    pub events: Vec<Value>,
    /// What to send as `after` next.
    pub next: u64,
    /// It has ended. With `error`, it failed.
    pub done: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// The guarded changes on probation: made together, kept by one `confirm`
/// or reverted together when `seconds_left` runs out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Pending {
    /// By key, never empty.
    pub changes: Vec<PendingChange>,
    pub seconds_left: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PendingChange {
    pub key: String,
    pub value: String,
    /// What a revert goes back to; `None` when the key was not set.
    pub previous: Option<String>,
}

impl PendingChange {
    /// What a revert goes back to, in words.
    pub fn previous_or_default(&self) -> &str {
        previous_or_default(self.previous.as_deref())
    }
}

/// `previous`, or `the default` when the key was not set before.
pub fn previous_or_default(previous: Option<&str>) -> &str {
    previous.unwrap_or("the default")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// A DevTools client other than the agent is connected to the browser,
    /// so the agent leaves the tab alone (docs/kiosk-browser.md). Defaulted
    /// the same way.
    #[serde(default)]
    pub devtools: bool,
    /// Where sound plays and at what volume. `None` from a device that
    /// predates audio.
    #[serde(default)]
    pub audio: Option<AudioStatus>,
    /// How full `/data` is. Defaulted the same way.
    #[serde(default)]
    pub data: Option<FsUsage>,
    /// Whether the display is on; `None` when the compositor cannot say.
    #[serde(default)]
    pub screen_on: Option<bool>,
    /// The page bridge: `browser.bridge.mode`, and the injected script with
    /// its state. Defaulted the same way.
    #[serde(default)]
    pub bridge: Option<BridgeStatus>,
    /// The timezone and whether the clock is in sync. Defaulted the same way.
    #[serde(default)]
    pub time: Option<TimeSummary>,
    /// Vendor, model, board and CPU. Defaulted the same way.
    #[serde(default)]
    pub hardware: Option<Hardware>,
    /// How much RAM there is and how much is in use. Defaulted the same way.
    #[serde(default)]
    pub memory: Option<MemUsage>,
    /// How busy the CPU was over the agent's last 2s sample, every core
    /// counted: 100 is all of them. `None` until it has two samples.
    /// Defaulted the same way.
    #[serde(default)]
    pub cpu_percent: Option<u8>,
    /// The player: whether it is on screen, the playlist and the item.
    /// Defaulted the same way.
    #[serde(default)]
    pub playlist: Option<playlist::PlaylistStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BridgeStatus {
    pub mode: String,
    /// `browser.inject.script`, empty for none.
    pub script: String,
    /// Why the script is not in the page, when it is not.
    #[serde(default)]
    pub script_problem: Option<String>,
}

/// The clock in one line, for `device status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimeSummary {
    pub timezone: Option<String>,
    /// timedated's `NTPSynchronized`: the kernel clock is disciplined.
    pub synchronized: Option<bool>,
    /// time.ntp.enable, as systemd reports it.
    pub ntp: Option<bool>,
}

/// Everything systemd says about the clock. Every field is what timedated
/// or timesyncd reported, never measured by the agent; `None` is "not
/// reported", which a service that is down or has not synced yet does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TimeStatus {
    /// Why timedated or timesyncd could not be read, if one of them could not.
    pub error: Option<String>,
    /// time.timezone, as set or defaulted.
    pub setting_timezone: String,
    /// time.ntp.servers as set: empty uses DHCP's, then the fallback.
    pub setting_servers: Vec<String>,
    /// The zone systemd has, `Europe/Bratislava`.
    pub timezone: Option<String>,
    /// The system clock, microseconds since the epoch.
    pub now_usec: Option<u64>,
    /// The same, as a wall clock in `timezone`: `2026-09-25 14:03:12`.
    pub local_time: Option<String>,
    /// `CEST`, and its distance from UTC in seconds east.
    pub zone_abbreviation: Option<String>,
    pub utc_offset_seconds: Option<i32>,
    /// The hardware clock, where there is one; a Raspberry Pi has none.
    pub rtc_usec: Option<u64>,
    /// The hardware clock keeps local time instead of UTC.
    pub local_rtc: Option<bool>,
    /// An NTP service is installed.
    pub can_ntp: Option<bool>,
    /// NTP is switched on (time.ntp.enable).
    pub ntp: Option<bool>,
    /// The kernel clock is disciplined by NTP.
    pub synchronized: Option<bool>,
    /// systemd-timesyncd's unit state: `active`, `inactive`, ...
    pub timesyncd: Option<String>,
    /// The server timesyncd uses now, by name and address.
    pub server_name: Option<String>,
    pub server_address: Option<String>,
    /// How often timesyncd asks, and between what bounds it moves.
    pub poll_interval_usec: Option<u64>,
    pub poll_interval_min_usec: Option<u64>,
    pub poll_interval_max_usec: Option<u64>,
    /// A server farther than this from a reference clock is not trusted.
    pub root_distance_max_usec: Option<u64>,
    /// The kernel's frequency correction - how fast this clock drifts - in
    /// units of 2^-16 ppm, as adjtimex keeps it. `frequency_ppm` reads it.
    pub frequency: Option<i64>,
    /// The last NTP answer, if there was one.
    pub last: Option<NtpSample>,
    pub servers: NtpServers,
}

impl TimeStatus {
    /// The drift correction in parts per million.
    pub fn frequency_ppm(&self) -> Option<f64> {
        self.frequency.map(|raw| raw as f64 / 65536.0)
    }
}

/// Where timesyncd's servers come from, in the order it tries them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NtpServers {
    /// Set at runtime by the agent: the DHCP servers, while time.ntp.servers
    /// is empty.
    pub runtime: Vec<String>,
    /// time.ntp.servers, from the agent's timesyncd drop-in.
    pub system: Vec<String>,
    /// Per-link servers, which only systemd-networkd hands out; empty here.
    pub link: Vec<String>,
    /// The image's fallback, used when none of the above has any.
    pub fallback: Vec<String>,
    /// What the network's DHCP offered, whether in use or not.
    pub dhcp: Vec<String>,
}

/// timesyncd's `NTPMessage`: the last answer from the server, and what
/// follows from its timestamps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NtpSample {
    /// 0 no warning, 1 or 2 a leap second at the end of the day, 3 the
    /// server is not synchronized.
    pub leap: u32,
    pub version: u32,
    /// The server's distance from a reference clock: 1 is attached to one.
    pub stratum: u32,
    /// The server clock's precision, log2 seconds (-20 is about 1 µs).
    pub precision: i32,
    pub root_delay_usec: u64,
    pub root_dispersion_usec: u64,
    /// The server's reference: a clock name like `GPS` at stratum 1, else
    /// the address of its own server.
    pub reference: String,
    /// How far this clock was from the server's: positive is behind.
    pub offset_usec: i64,
    /// The round trip, less the time the server took to answer.
    pub delay_usec: i64,
    /// timesyncd's running estimate of how much the offset varies.
    pub jitter_usec: u64,
    /// Answers counted since timesyncd started.
    pub packet_count: u64,
    /// The last answer was thrown away as an outlier.
    pub spike: bool,
    /// When the answer arrived, microseconds since the epoch.
    pub received_usec: u64,
}

impl NtpSample {
    /// The offset and delay of one exchange, from its origin, receive,
    /// transmit and destination timestamps (microseconds of one clock):
    /// what `timedatectl timesync-status`
    /// shows, computed the same way.
    pub fn offset_and_delay(
        origin: u64,
        receive: u64,
        transmit: u64,
        destination: u64,
    ) -> (i64, i64) {
        let (origin, receive, transmit, destination) = (
            origin as i128,
            receive as i128,
            transmit as i128,
            destination as i128,
        );
        let offset = ((receive - origin) + (transmit - destination)) / 2;
        let delay = (destination - origin) - (transmit - receive);
        (offset as i64, delay as i64)
    }
}

/// `YYYY-MM-DD HH:MM[:SS]` as fields, for `time set`: year, month, day,
/// hour, minute, second. A `T` between the date and the time is accepted.
pub fn parse_local_time(value: &str) -> Result<[i32; 6], String> {
    let bad = || format!("{value} is not a time; write it as YYYY-MM-DD HH:MM[:SS]");
    let value = value.trim();
    let (date, time) = value.split_once([' ', 'T']).ok_or_else(bad)?;
    let date: Vec<&str> = date.split('-').collect();
    let time: Vec<&str> = time.trim().split(':').collect();
    if date.len() != 3 || !(2..=3).contains(&time.len()) {
        return Err(bad());
    }
    let number = |part: &str| part.parse::<i32>().map_err(|_| bad());
    let fields = [
        number(date[0])?,
        number(date[1])?,
        number(date[2])?,
        number(time[0])?,
        number(time[1])?,
        if time.len() == 3 { number(time[2])? } else { 0 },
    ];
    let [year, month, day, hour, minute, second] = fields;
    let ok = (2000..=2200).contains(&year)
        && (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (0..=23).contains(&hour)
        && (0..=59).contains(&minute)
        && (0..=60).contains(&second);
    if ok {
        Ok(fields)
    } else {
        Err(bad())
    }
}

/// One output or input as PipeWire has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioDevice {
    /// What audio.output or audio.input takes to pick exactly this one.
    pub name: String,
    pub description: String,
    /// `hdmi`, `jack`, `usb`, `bluetooth`, or `other`.
    pub kind: String,
    /// Something is plugged in, as far as the hardware can tell; `None` when
    /// it cannot.
    pub available: Option<bool>,
    /// The one the settings put in use.
    pub in_use: bool,
    /// Not there until its sound card is switched to another profile, which
    /// choosing it does.
    #[serde(default)]
    pub needs_profile: bool,
}

/// The output side or the input side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioSide {
    /// audio.output or audio.input, as set or defaulted.
    pub setting: String,
    /// The device in use, if any.
    pub using: Option<AudioDevice>,
    /// Why that is not what the setting names: `usb is not connected`.
    pub fallback: Option<String>,
    /// audio.volume or audio.input_volume, percent.
    pub volume: u8,
    /// Muted: audio.mute, or the setting is `off`.
    pub muted: bool,
    pub devices: Vec<AudioDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AudioStatus {
    /// PipeWire answered. When it did not, `error` says why and the sides
    /// carry only the settings.
    pub running: bool,
    pub error: Option<String>,
    pub output: AudioSide,
    pub input: AudioSide,
}

/// What `audio test` played or heard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AudioTested {
    pub message: String,
    /// The recording's loudest sample and its average, dBFS: 0 is full
    /// scale, silence is far below -60.
    #[serde(default)]
    pub peak_dbfs: Option<f64>,
    #[serde(default)]
    pub rms_dbfs: Option<f64>,
    /// Where the recording was left in the file store, for `tessaro-ctl
    /// files download` or `http://127.0.0.1/files/`.
    #[serde(default)]
    pub saved: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePhase {
    /// Nothing under way.
    Idle,
    /// Part of the image has arrived.
    Receiving,
    /// All of it has; the device is checking the whole file against its
    /// SHA-256 (`verified` of `size`). Skipped with `verify: false`.
    Verifying,
    /// Checked; the device is decompressing it once as a dry run, checking
    /// each bmap range it will write (`prepared` of `to_prepare`).
    Preparing,
    /// Staged and verified; waiting for `update-commit`.
    Ready,
    /// Committed; applied at the next boot.
    Pending,
    /// Preparing failed; `error` says why. Begin again to retry.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateBegun {
    /// Bytes the device already has. Send from here.
    pub offset: u64,
    pub phase: UpdatePhase,
}

/// How much of an upload - an image, or one file - the device has, after a
/// chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Received {
    pub received: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// Mapped bytes of the image checked, of `to_prepare`.
    pub prepared: u64,
    pub to_prepare: u64,
    pub error: Option<String>,
    pub wipe_data: bool,
    /// The whole disk is rewritten, not the root partition. Defaulted for
    /// older devices, which cannot.
    #[serde(default)]
    pub repartition: bool,
    /// What the last boot that applied an update did.
    pub last: Option<UpdateResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateResult {
    pub applied: bool,
    pub message: String,
    pub source: String,
    pub wiped_data: bool,
    pub attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// The image's default, from /usr/lib/tessaro-kiosk/tessaro-kiosk.env.
    Default,
    /// Set on this device.
    Set,
    /// Read-only: reported by the device as it is right now.
    Live,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Setting {
    pub key: String,
    pub env: String,
    pub value: Option<String>,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    pub revision: u64,
    pub settings: Vec<Setting>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Applied {
    pub revision: u64,
    pub changed: Vec<String>,
    /// Units restarted, or to be restarted once this reply is out.
    pub restarted: Vec<String>,
    pub pending: Option<Pending>,
    /// What the network change did, when network keys changed: its checks.
    /// A rolled-back change is an error, not an `Applied`. Defaulted for
    /// older devices.
    #[serde(default)]
    pub network: Option<NetChange>,
    /// What the audio.* keys did on the sound server, in words, when they
    /// changed: where sound now plays and how loud, or why it could not be
    /// applied (the setting is saved either way).
    #[serde(default)]
    pub audio: Option<String>,
    /// What the time.* keys did to the clock, in words, when they changed
    /// (the setting is saved either way). Defaulted for older devices.
    #[serde(default)]
    pub time: Option<String>,
    /// A setting the firmware reads at power-on changed on disk, so it takes
    /// effect at the next reboot, which the device leaves to the operator.
    /// Defaulted for older devices.
    #[serde(default)]
    pub reboot: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// The control an editor offers for it. Defaulted for older devices.
    #[serde(default)]
    pub input: KeyInput,
}

/// How a value is entered. `config set` checks what is entered either way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum KeyInput {
    /// A checkbox: `1` or `0`.
    Flag,
    /// One of these.
    Choice { choices: Vec<String> },
    /// A whole number in a range.
    Int { min: i64, max: i64 },
    /// Text.
    #[default]
    Text,
    /// Nothing to enter: the device reports it.
    ReadOnly,
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
            input: key.kind.input(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Connector {
    pub name: String,
    pub modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetAddress {
    pub address: String,
    pub prefix: u8,
    /// `ipv4` or `ipv6`.
    pub family: String,
    /// `global`, `link-local` or `loopback`.
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    /// network.proxy.url with its password masked; `None` without a proxy.
    /// Defaulted for older devices.
    #[serde(default)]
    pub proxy: Option<String>,
}

/// What `network proxy show` reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProxyStatus {
    /// network.proxy.url with its password masked; `None` without a proxy.
    pub url: Option<String>,
    /// network.proxy.bypass, besides loopback.
    pub bypass: Vec<String>,
    /// Where the device's local proxy listens, `127.0.0.1:3128`.
    pub listen: String,
    /// Its unit's state: `active`, `inactive`, `failed`, ...
    pub unit: String,
}

/// What `network proxy test` got through the proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProxyTested {
    /// The address the internet sees through the proxy.
    pub ip: Option<String>,
    /// Why it could not be reached, when it could not.
    pub error: Option<String>,
}

/// One partition of the disk the device runs from. Sizes and offsets in
/// bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Partition {
    pub number: u32,
    /// The kernel's name, `sda3` or `mmcblk0p3`.
    pub name: String,
    pub start: u64,
    pub size: u64,
    /// The filesystem label, `data` for /data.
    pub label: Option<String>,
    /// Only for filesystems that are mounted.
    pub fstype: Option<String>,
    pub mountpoint: Option<String>,
}

/// How full one mounted filesystem is, in bytes. `available` is what a
/// process that is not root can still write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FsUsage {
    pub mountpoint: String,
    pub source: String,
    pub fstype: String,
    pub size: u64,
    pub used: u64,
    pub available: u64,
}

impl FsUsage {
    /// Percent of the space a writer can see that is used, as `df` counts it.
    pub fn used_percent(&self) -> u64 {
        let seen = self.used + self.available;
        if seen == 0 {
            0
        } else {
            (self.used * 100).div_ceil(seen)
        }
    }
}

/// What the hardware says it is, from DMI on x86 or the device tree on the
/// Pi (docs/hardware.md). `None` is "the firmware does not say".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Hardware {
    /// `LENOVO`, `QEMU`, `Raspberry Pi`.
    pub vendor: Option<String>,
    /// `ThinkCentre M720q`, `Raspberry Pi 3 Model B Plus Rev 1.3`.
    pub model: Option<String>,
    /// The mainboard, or on the Pi the manufacturer that built it.
    pub board: Option<String>,
    /// The BIOS version and date. The Pi does not say.
    pub firmware: Option<String>,
    pub serial: Option<String>,
    /// `Intel(R) Core(TM) i5-8500T CPU @ 2.10GHz`, or the Pi's SoC, `BCM2837`.
    pub cpu: Option<String>,
    pub cores: Option<u32>,
    /// `x86_64`, `aarch64`.
    pub arch: String,
}

impl Hardware {
    /// Vendor and model in one line, `LENOVO 10T7002VMC (ThinkCentre M720q)`.
    pub fn machine(&self) -> Option<String> {
        let parts: Vec<&str> = [self.vendor.as_deref(), self.model.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    /// The CPU and its cores, `BCM2837, 4 cores`, without the architecture.
    pub fn cpu_line(&self) -> Option<String> {
        let cores = self
            .cores
            .map(|n| format!("{n} {}", if n == 1 { "core" } else { "cores" }));
        let parts: Vec<String> = self.cpu.iter().cloned().chain(cores).collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// RAM, in bytes, from `/proc/meminfo`. `available` is `MemAvailable`: what
/// can be handed out without swapping, page cache included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MemUsage {
    pub total: u64,
    pub available: u64,
}

impl MemUsage {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    pub fn used_percent(&self) -> u64 {
        if self.total == 0 {
            0
        } else {
            (self.used() * 100).div_ceil(self.total)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Storage {
    /// The disk holding root, `sda` or `mmcblk0`.
    pub device: String,
    pub size: u64,
    /// `gpt` or `dos`.
    pub table: String,
    pub model: Option<String>,
    /// Space after the last partition that `storage-grow` would give to
    /// `/data`; 0 when there is too little to bother (`GROW_MIN`).
    pub unallocated: u64,
    pub partitions: Vec<Partition>,
    pub filesystems: Vec<FsUsage>,
}

/// Less unallocated space than this is not worth growing into.
pub const GROW_MIN: u64 = 64 << 20;

/// Every camera, and the camera.* settings their mirrors capture with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CameraList {
    /// camera.format, as saved: `auto`, `mjpeg` or `yuyv`.
    pub format: String,
    /// camera.size, as saved: `auto` or `WIDTHxHEIGHT`.
    pub size: String,
    /// camera.mirrors, as saved: how many virtual cameras each camera gets.
    pub mirrors: u32,
    pub cameras: Vec<CameraInfo>,
}

/// One USB camera. Only its mirror, `tessaro-camera@<device>.service`, opens
/// the camera itself; everything else reads one of its `mirrors`. The mirror
/// writes this to `/run/tessaro-camera/<device>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CameraInfo {
    /// What the camera calls itself, `HD Pro Webcam C920`.
    pub name: String,
    /// The camera's own node, `video0`.
    pub device: String,
    /// Where it is plugged in, as the kernel says: `usb-0000:00:14.0-2`.
    pub bus: String,
    /// The virtual cameras the mirror writes every frame into, one reader
    /// each: v4l2loopback streams a device to one reader at a time. Empty
    /// when the mirror could not start.
    #[serde(default)]
    pub mirrors: Vec<CameraMirror>,
    /// What the mirror captures. None when it could not start.
    #[serde(default)]
    pub mode: Option<CameraMode>,
    /// Why the camera.* settings are not what it captures: the camera does
    /// not have that format or size.
    #[serde(default)]
    pub fallback: Option<String>,
    /// Why the mirror is not running.
    #[serde(default)]
    pub error: Option<String>,
    /// Every format and size the camera has, and the most frames a second
    /// each gives.
    #[serde(default)]
    pub modes: Vec<CameraMode>,
}

/// One virtual camera of a camera.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CameraMirror {
    /// What readers see, and a page's `enumerateDevices()` shows: `HD Pro
    /// Webcam C920 Mirror 1`. The camera's name is cut short to fit V4L2's
    /// 31 characters.
    pub name: String,
    /// Its node, `/dev/video50`.
    pub device: String,
}

/// A format, frame size and rate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CameraMode {
    /// `mjpeg` or `yuyv`. A camera's other formats are not listed: the
    /// mirror captures only these.
    pub format: String,
    pub width: u32,
    pub height: u32,
    /// Frames a second.
    pub fps: u32,
}

/// A NetworkManager profile, as `net profiles` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetProfile {
    /// `connection.id`.
    pub name: String,
    pub uuid: String,
    /// `ethernet`, `wifi`, or NetworkManager's own type name for anything
    /// else.
    pub kind: String,
    /// The device it is active on, or bound to.
    pub device: Option<String>,
    pub active: bool,
    pub autoconnect: bool,
    pub priority: i32,
    /// Written to disk. A profile under `/run` counts as unsaved, the
    /// managed ones included.
    pub saved: bool,
    /// One of the fixed profiles the agent renders from settings on
    /// every boot, so unsaved but never lost.
    #[serde(default)]
    pub managed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetIpSettings {
    pub method: String,
    /// `ADDRESS/PREFIX`.
    pub addresses: Vec<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    pub ignore_auto_dns: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetWifiSettings {
    pub ssid: String,
    /// `open`, `wpa-psk`, `sae`, `wpa-eap`, or NetworkManager's key-mgmt.
    pub security: String,
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetProfileDetail {
    pub profile: NetProfile,
    pub ipv4: NetIpSettings,
    pub ipv6: NetIpSettings,
    pub wifi: Option<NetWifiSettings>,
    /// What its device has right now, from the kernel. Empty when inactive.
    pub addresses: Vec<NetAddress>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WifiDeviceInfo {
    pub interface: String,
    /// NetworkManager's device state: `activated`, `disconnected`, ...
    pub state: String,
    pub ssid: Option<String>,
    /// Percent.
    pub signal: Option<u8>,
    pub frequency_mhz: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WifiStatus {
    /// The radio, as software (`net wifi on|off`) left it.
    pub enabled: bool,
    /// A hardware kill switch, if the device has one.
    pub hardware_enabled: bool,
    pub devices: Vec<WifiDeviceInfo>,
    /// The client network the hotspot stands in for, when the client did not
    /// connect after boot (`network.wifi.fallback_after`). Until the next boot.
    #[serde(default)]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WifiNetwork {
    /// Empty for a hidden network.
    pub ssid: String,
    pub bssid: String,
    /// Percent.
    pub signal: u8,
    pub frequency_mhz: u32,
    /// `open`, `wpa-psk`, `sae`, `wpa-eap`, `wep`, or several joined by `/`.
    pub security: String,
    pub interface: String,
    /// A saved profile has this SSID.
    pub known: bool,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeOutcome {
    Committed,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

/// What a network change did. Kept on the device as the last one, so a
/// client whose connection went with the change can still ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetChange {
    pub outcome: ChangeOutcome,
    /// What was asked: `set`, `up`, `down`, `forget`, `join`, `wifi on` ...
    pub action: String,
    pub profile: Option<String>,
    pub uuid: Option<String>,
    /// Why it was rolled back.
    pub reason: Option<String>,
    pub checks: Vec<NetCheck>,
    /// Anything the operator should know about a kept change.
    pub note: Option<String>,
}

/// One step of `net-ping`, in the order they arrive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum PingEvent {
    /// The address the host resolved to.
    Start {
        host: String,
        address: String,
    },
    Reply {
        seq: u16,
        bytes: usize,
        rtt_ms: f64,
    },
    Timeout {
        seq: u16,
    },
    Summary {
        sent: u32,
        received: u32,
        min_ms: Option<f64>,
        avg_ms: Option<f64>,
        max_ms: Option<f64>,
    },
}

pub const PING_DEFAULT_COUNT: u32 = 4;
pub const PING_MAX_COUNT: u32 = 100;
pub const PING_DEFAULT_INTERVAL_MS: u64 = 1_000;
pub const PING_MIN_INTERVAL_MS: u64 = 200;
pub const PING_DEFAULT_TIMEOUT_MS: u64 = 2_000;
pub const PING_MAX_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Claimed {
    pub token_id: String,
    pub token: String,
    pub root_password: String,
    /// The hotspot's new password, shown once like the root password. `None`
    /// on a device without its WiFi interface, or an older one.
    #[serde(default)]
    pub hotspot: Option<HotspotCredentials>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HotspotCredentials {
    pub ssid: String,
    pub password: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    /// The id of the token that issued it, `claim`, or `local`.
    pub issued_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenCreated {
    pub id: String,
    pub token: String,
}

/// What the welcome page shows, the body of `welcome.json` too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WelcomeInfo {
    pub node: String,
    /// Global IPv4 addresses, with the interface each is on.
    pub addresses: Vec<WelcomeAddress>,
    /// While the hotspot is up.
    pub hotspot: Option<WelcomeHotspot>,
    pub claimed: bool,
    /// Whether the last public address lookup answered; `None` until one
    /// has been tried.
    pub online: Option<bool>,
    /// While the hotspot is up and the device unclaimed.
    pub setup: Option<WelcomeSetup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WelcomeAddress {
    pub address: String,
    pub interface: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WelcomeHotspot {
    pub ssid: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WelcomeSetup {
    /// An SVG QR code that joins a phone to the hotspot.
    pub qr: String,
    /// Where Quick Setup is on the hotspot.
    pub url: String,
}

/// Who a browser is to the device: what Webconfig asks first
/// (docs/webconfig.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WebSession {
    pub claimed: bool,
    /// Unclaimed and nothing set yet: Webconfig opens on Quick Setup.
    pub fresh: bool,
    pub via: Via,
    /// The token the request came in with, directly or through a session.
    #[serde(default)]
    pub token: Option<TokenInfo>,
    /// How long a browser session lasts without use, in seconds
    /// (`access.session_timeout`).
    pub timeout: u64,
}

/// How a request was let in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    /// No credential: the device is unclaimed, or the endpoint is public.
    Anonymous,
    /// An `Authorization: Bearer` token.
    Token,
    /// A browser session's cookie.
    Session,
    /// The local socket.
    Local,
}

/// A one-time ticket that signs a browser in: `tessaro-ctl access
/// webconfig` puts it in the address it opens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ticket {
    pub ticket: String,
    /// Seconds it can still be redeemed in.
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Password {
    /// The generated password, when the server chose it. Shown once.
    pub password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SshAccess {
    /// SHA256 fingerprint of the key sent.
    pub fingerprint: String,
    /// False when the key was already there.
    pub added: bool,
    /// The device's host keys, as OpenSSH `TYPE BASE64` lines. Empty when
    /// the device could not read them; the client then falls back to
    /// ssh's own first-use prompt.
    pub host_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SshKeyInfo {
    pub fingerprint: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub comment: String,
}

/// What `ssh-key-revoke` removed. `message` is what an older client prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SshKeyRevoked {
    pub message: String,
    /// The removed key's SHA256 fingerprint. `None` from an older device,
    /// which only says it in `message`.
    #[serde(default)]
    pub fingerprint: Option<String>,
}

/// The most PEM text one `net-cert-add` may carry: a chain of a few
/// certificates, with room to spare.
pub const CERT_PEM_MAX: usize = 64 * 1024;

/// One extra certificate authority the device trusts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CertInfo {
    /// SHA-256 of the DER, lower-case hex.
    pub fingerprint: String,
    /// `CN=Corp Root CA, O=Corp`.
    pub subject: String,
    pub issuer: String,
    /// When it expires, seconds since the epoch, UTC.
    pub not_after: i64,
    /// Signed by its own key: a root rather than an intermediate.
    pub self_signed: bool,
}

/// What `net-cert-add` did with each certificate it was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CertsAdded {
    pub added: Vec<CertInfo>,
    /// Already trusted; nothing changed for these.
    pub present: Vec<CertInfo>,
}

/// Most `OnCalendar` expressions one schedule may carry.
pub const SCHEDULE_CALENDAR_MAX: usize = 16;
/// Longest calendar expression, in bytes.
pub const SCHEDULE_EXPRESSION_MAX: usize = 4096;
/// Most run times `schedule-check` answers with.
pub const SCHEDULE_CHECK_MAX: u32 = 50;

/// Longest script body, in bytes.
pub const SCRIPT_BODY_MAX: usize = 64 * 1024;
/// Longest script description, in bytes.
pub const SCRIPT_DESCRIPTION_MAX: usize = 200;
/// How many finished runs of a script the device remembers.
pub const SCRIPT_RUNS_KEPT: usize = 20;
/// Most output lines `script-run` answers with; the journal keeps the rest.
pub const SCRIPT_OUTPUT_MAX: usize = 1000;

/// What a run does when a command of the script fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OnError {
    /// The run ends there, failed: the body runs with `/bin/sh -e`.
    #[default]
    Stop,
    /// The rest runs anyway; the run ends as its last command does.
    Continue,
}

impl OnError {
    pub const NAMES: &'static [&'static str] = &["stop", "continue"];

    pub fn name(self) -> &'static str {
        match self {
            OnError::Stop => "stop",
            OnError::Continue => "continue",
        }
    }
}

impl std::str::FromStr for OnError {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "stop" => Ok(OnError::Stop),
            "continue" => Ok(OnError::Continue),
            _ => Err(format!("{name:?} is not stop or continue")),
        }
    }
}

/// What a run does when one of the same script is still going.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Concurrency {
    /// It starts anyway; runs overlap.
    #[default]
    Overlap,
    /// It does not start.
    Skip,
}

impl Concurrency {
    pub const NAMES: &'static [&'static str] = &["overlap", "skip"];

    pub fn name(self) -> &'static str {
        match self {
            Concurrency::Overlap => "overlap",
            Concurrency::Skip => "skip",
        }
    }
}

impl std::str::FromStr for Concurrency {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "overlap" => Ok(Concurrency::Overlap),
            "skip" => Ok(Concurrency::Skip),
            _ => Err(format!("{name:?} is not overlap or skip")),
        }
    }
}

/// What a script is: a shell body and how its runs behave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptSpec {
    /// `[a-z0-9][a-z0-9-]*`, unique on the device.
    pub name: String,
    /// One line saying what it does.
    #[serde(default)]
    pub description: String,
    /// Run by `/bin/sh` as root, from `/`.
    pub body: String,
    #[serde(default)]
    pub on_error: OnError,
    /// The longest a run may take before systemd kills it.
    #[serde(default)]
    pub timeout_s: Option<u64>,
    #[serde(default)]
    pub concurrency: Concurrency,
    /// The kiosk page may list it and run it (`tessaro.scripts`).
    #[serde(default)]
    pub bridge: bool,
}

/// What a schedule is: when it fires and which script it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleSpec {
    /// `[a-z0-9][a-z0-9-]*`, unique on the device.
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// systemd `OnCalendar` expressions; the schedule fires when any of
    /// them does.
    pub calendar: Vec<String>,
    /// The script it runs: its id or its name when sent, its id when
    /// answered.
    pub script: String,
}

/// A moment as the device reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Moment {
    /// Seconds since the epoch, UTC.
    pub unix: i64,
    /// The device's wall clock: `2026-09-28 07:00:00 CEST`.
    pub local: String,
}

/// How a finished run of a script ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptRun {
    /// The run's systemd instance, `manual-1700000000-4f2a`: what
    /// `logs --unit` takes after the script's template.
    pub run: String,
    /// What started it: `manual`, `bridge` (the kiosk page) or
    /// `schedule`, the first word of `run`.
    pub trigger: String,
    /// The schedule that started it, by name, while that schedule exists.
    #[serde(default)]
    pub schedule: Option<String>,
    pub started: Moment,
    pub finished: Moment,
    /// systemd's `$SERVICE_RESULT`: `success`, `exit-code`, `timeout`,
    /// `signal`...
    pub result: String,
    /// systemd's `$EXIT_STATUS`: an exit code or a signal name.
    pub status: String,
}

impl ScriptRun {
    pub fn succeeded(&self) -> bool {
        self.result == "success"
    }
}

/// One script, its recent runs and who runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptInfo {
    /// Stable across renames; names the script's systemd units.
    pub id: String,
    #[serde(flatten)]
    pub spec: ScriptSpec,
    /// Its finished runs, newest first, at most `SCRIPT_RUNS_KEPT`.
    #[serde(default)]
    pub runs: Vec<ScriptRun>,
    /// Runs going right now.
    #[serde(default)]
    pub running: u32,
    /// The schedules that run it, by name.
    #[serde(default)]
    pub schedules: Vec<String>,
    /// The journal pattern of its runs, for `logs --unit`.
    pub units: String,
}

/// One step of `script-run`, in the order they arrive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum ScriptEvent {
    /// The run started, as this systemd instance.
    Started { run: String },
    /// One line the run wrote, to stdout or stderr: the journal keeps
    /// them in one stream, in order.
    Line { text: String },
    /// Past `SCRIPT_OUTPUT_MAX` lines; the rest is only in the journal.
    Cut,
    /// How it ended, as the run recorded it; the last step.
    Ended { record: ScriptRun },
}

/// One schedule and what systemd reports about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleInfo {
    /// Stable across renames; names the schedule's systemd timer.
    pub id: String,
    #[serde(flatten)]
    pub spec: ScheduleSpec,
    /// The name of the script it runs.
    pub script_name: String,
    /// When the timer fires next; `None` while disabled.
    #[serde(default)]
    pub next: Option<Moment>,
    /// When the timer last fired.
    #[serde(default)]
    pub last_trigger: Option<Moment>,
    /// The last run this schedule started that has finished.
    #[serde(default)]
    pub last_run: Option<ScriptRun>,
    /// Runs this schedule started that are going right now.
    #[serde(default)]
    pub running: u32,
    /// The journal pattern of the runs it started, for `logs --unit`.
    pub units: String,
}

/// What `schedule-check` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CalendarCheck {
    /// Each expression as systemd normalizes it, in the order sent.
    pub normalized: Vec<String>,
    /// The next times any of them fires, earliest first.
    pub next: Vec<Moment>,
}

/// The largest document `printer-print` takes as `data`, decoded: one
/// request's worth. A larger one goes into the file store first and is
/// printed by its `path`.
pub const PRINT_DATA_MAX: usize = UPDATE_CHUNK;
/// Most copies of one document.
pub const PRINT_COPIES_MAX: u32 = 99;

/// How CUPS drives a printer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PrinterKind {
    /// Driverless: IPP Everywhere or AirPrint. CUPS asks the printer what
    /// it takes and turns a PDF into it.
    #[default]
    Ipp,
    /// The bytes go to the printer as they are: a receipt printer's ESC/POS,
    /// a label printer's ZPL, or a document already in the printer's
    /// language.
    Raw,
}

impl PrinterKind {
    pub const NAMES: &'static [&'static str] = &["ipp", "raw"];

    pub fn name(self) -> &'static str {
        match self {
            PrinterKind::Ipp => "ipp",
            PrinterKind::Raw => "raw",
        }
    }
}

impl std::str::FromStr for PrinterKind {
    type Err = String;
    fn from_str(name: &str) -> Result<Self, String> {
        match name {
            "ipp" => Ok(PrinterKind::Ipp),
            "raw" => Ok(PrinterKind::Raw),
            _ => Err(format!("{name:?} is not ipp or raw")),
        }
    }
}

/// What a printer is: how it is reached and how it is driven.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrinterSpec {
    /// `[a-z0-9][a-z0-9_-]*`, unique on the device: what a page and
    /// `tessaro-ctl printer print` name it by.
    pub name: String,
    /// Where it is, as CUPS spells it: `ipp://host/ipp/print`,
    /// `ipps://...`, `socket://host:9100`, `usb://...`, or a `dnssd://` URI
    /// from `printer-discover`.
    pub uri: String,
    #[serde(default)]
    pub kind: PrinterKind,
    /// The paper a job gets when it names none, as the printer names it:
    /// `iso_a4_210x297mm`, `na_letter_8.5x11in`, `A4`. Empty for the
    /// printer's own default.
    #[serde(default)]
    pub media: Option<String>,
}

/// One supply a printer reports: an ink, a toner, a waste box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrinterMarker {
    pub name: String,
    /// Percent left, when the printer says.
    #[serde(default)]
    pub level: Option<u8>,
}

/// One printer and what CUPS reports about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrinterInfo {
    #[serde(flatten)]
    pub spec: PrinterSpec,
    /// `window.print()` prints here.
    pub default: bool,
    /// `idle`, `printing`, `stopped`, or `missing` while CUPS does not have
    /// it yet: it did not answer when it was set up, and the device keeps
    /// trying.
    pub state: String,
    /// Why, as CUPS says: `offline-report`, `media-empty`, or the error that
    /// kept it from being set up.
    #[serde(default)]
    pub message: Option<String>,
    /// Jobs waiting or printing.
    #[serde(default)]
    pub queued: u32,
    /// Only from `printer-show`, and only from a printer that reports them.
    #[serde(default)]
    pub markers: Vec<PrinterMarker>,
    /// The make and model the printer gave, for a driverless one.
    #[serde(default)]
    pub model: Option<String>,
}

/// Every printer, and whether pages may print.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrinterList {
    /// printer.enable: `window.print()` and the page bridge print. The
    /// device's own clients print either way.
    pub enabled: bool,
    pub printers: Vec<PrinterInfo>,
}

/// One step of `printer-discover`: a printer CUPS found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrinterFound {
    /// What `printer-create` takes as `uri`.
    pub uri: String,
    /// What the printer calls itself, or its make and model.
    pub description: String,
    /// How it is best driven: `ipp` for a printer that answers IPP, `raw`
    /// for a socket or USB one.
    pub kind: PrinterKind,
    /// `network` or `direct` (USB), as CUPS classes it.
    pub class: String,
    /// A printer the device has already, by this URI.
    #[serde(default)]
    pub known: Option<String>,
}

/// A job CUPS holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrintJob {
    /// `office-12`: what `printer-cancel` takes.
    pub job: String,
    pub printer: String,
    /// Bytes, as sent.
    pub size: u64,
    /// When it was sent, as CUPS writes it.
    pub submitted: String,
}

/// A document handed to CUPS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrintQueued {
    pub job: String,
    pub printer: String,
    pub message: String,
}

/// A camera's newest frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CameraSnapshot {
    /// `jpeg`.
    pub format: String,
    /// Base64.
    pub data: String,
    /// How old the frame is, milliseconds.
    pub age_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

/// `7.8 GB`, `512.0 MB`, `4.0 kB`: decimal units, one decimal, like `df -H`.
pub fn size_label(bytes: u64) -> String {
    let value = bytes as f64;
    if bytes >= 1_000_000_000_000 {
        format!("{:.1} TB", value / 1e12)
    } else if bytes >= 1_000_000_000 {
        format!("{:.1} GB", value / 1e9)
    } else if bytes >= 1_000_000 {
        format!("{:.1} MB", value / 1e6)
    } else if bytes >= 1_000 {
        format!("{:.1} kB", value / 1e3)
    } else {
        format!("{bytes} B")
    }
}

/// `100k`, `1m`, ... the way `tessaro-ctl network speedtest --max-size` spells them.
pub fn speedtest_size_label(size: u64) -> String {
    if size >= 1_000_000 {
        format!("{}m", size / 1_000_000)
    } else {
        format!("{}k", size / 1_000)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Download,
    Upload,
}

/// One step of `speedtest`, in the order they arrive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// One step of `storage-grow`, in the order they arrive. Sizes in bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "phase", rename_all = "kebab-case")]
pub enum StorageGrowEvent {
    /// What would change. Equal sizes mean that part is already done; both
    /// equal, that there is nothing to grow.
    Plan {
        /// The partition, `/dev/sda3`.
        partition: String,
        partition_from: u64,
        partition_to: u64,
        filesystem_from: u64,
        filesystem_to: u64,
    },
    /// About to run `command`.
    Step { what: String, command: String },
    /// The sizes afterwards, read back from the kernel and the filesystem.
    Grown { partition: u64, filesystem: u64 },
}

/// The largest script `eval` takes, and the largest the page bridge injects.
pub const EVAL_MAX: usize = 1 << 20;
/// How long `eval` waits by default, and at most.
pub const EVAL_TIMEOUT_MS: u64 = 10_000;
pub const EVAL_TIMEOUT_MAX_MS: u64 = 60_000;

/// What `eval` came to: the value, as JSON, or what it threw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EvalResult {
    /// `None` for `undefined`, and for a value JSON cannot hold (a
    /// function, a DOM node): `description` says what it was.
    #[serde(default)]
    pub value: Option<Value>,
    /// The value's type as JavaScript names it, `undefined` included.
    pub kind: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub exception: Option<EvalException>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EvalException {
    pub text: String,
    /// 1-based, in the code as sent.
    pub line: u64,
    pub column: u64,
}

/// Whether the display is on, as the compositor has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScreenPower {
    pub on: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Done {
    pub message: String,
}

impl Done {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Lower-case hex, the way every fingerprint, id and checksum here is
/// written.
pub fn hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

/// Hex of either case back to bytes; `None` if it is not hex.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    data_encoding::HEXLOWER_PERMISSIVE
        .decode(text.as_bytes())
        .ok()
}

/// One piece of an upload (`update-chunk`, `files-chunk`): base64 of 1 to
/// `UPDATE_CHUNK` bytes.
pub fn decode_chunk(data: &str) -> Result<Vec<u8>, String> {
    let bytes = data_encoding::BASE64
        .decode(data.as_bytes())
        .map_err(|_| "the chunk is not base64".to_string())?;
    if bytes.is_empty() || bytes.len() > UPDATE_CHUNK {
        return Err(format!(
            "a chunk is 1 to {UPDATE_CHUNK} bytes, not {}",
            bytes.len()
        ));
    }
    Ok(bytes)
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
    fn offset_and_delay_follow_timedatectl() {
        // Sent at 1000, the server read it at 1600 and answered at 1700,
        // back at 1300: the server is 500 ahead, 200 of travel.
        assert_eq!(
            NtpSample::offset_and_delay(1000, 1600, 1700, 1300),
            (500, 200)
        );
        // A clock ahead of the server gives a negative offset.
        assert_eq!(
            NtpSample::offset_and_delay(2000, 1510, 1520, 2030),
            (-500, 20)
        );
    }

    #[test]
    fn frequency_reads_as_ppm() {
        let status = TimeStatus {
            error: None,
            setting_timezone: "UTC".to_string(),
            setting_servers: Vec::new(),
            timezone: None,
            now_usec: None,
            local_time: None,
            zone_abbreviation: None,
            utc_offset_seconds: None,
            rtc_usec: None,
            local_rtc: None,
            can_ntp: None,
            ntp: None,
            synchronized: None,
            timesyncd: None,
            server_name: None,
            server_address: None,
            poll_interval_usec: None,
            poll_interval_min_usec: None,
            poll_interval_max_usec: None,
            root_distance_max_usec: None,
            frequency: Some(-65536 * 12 - 32768),
            last: None,
            servers: NtpServers::default(),
        };
        assert_eq!(status.frequency_ppm(), Some(-12.5));
    }

    #[test]
    fn local_times_for_time_set() {
        assert_eq!(
            parse_local_time("2026-09-25 14:03").unwrap(),
            [2026, 9, 25, 14, 3, 0]
        );
        assert_eq!(
            parse_local_time(" 2026-09-25T14:03:09 ").unwrap(),
            [2026, 9, 25, 14, 3, 9]
        );
        for bad in [
            "",
            "2026-09-25",
            "14:03",
            "2026-13-01 10:00",
            "2026-09-25 24:00",
            "1970-01-01 00:00",
            "2026/09/25 10:00",
        ] {
            assert!(parse_local_time(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_secret_travels_but_never_prints() {
        let command = Command::WifiJoin {
            ssid: "Office".into(),
            psk: Some(Secret("hunter2hunter2".into())),
            security: None,
            hidden: false,
            verify: Verify::Gateway,
        };
        assert!(serde_json::to_string(&command)
            .unwrap()
            .contains("hunter2hunter2"));
        assert!(!format!("{command:?}").contains("hunter2"));
    }

    #[test]
    fn verify_is_tagged_by_check() {
        let tcp = serde_json::to_value(Verify::Tcp {
            host: "a.test".into(),
            port: 443,
        })
        .unwrap();
        assert_eq!(tcp["check"], "tcp");
    }

    #[test]
    fn verify_parses_like_the_flag() {
        assert_eq!(Verify::parse("gateway").unwrap(), Verify::Gateway);
        assert_eq!(Verify::parse("none").unwrap(), Verify::None);
        assert_eq!(
            Verify::parse("10.0.0.1").unwrap(),
            Verify::Host {
                host: "10.0.0.1".into()
            }
        );
        assert_eq!(
            Verify::parse("2001:db8::1").unwrap(),
            Verify::Host {
                host: "2001:db8::1".into()
            }
        );
        assert_eq!(
            Verify::parse("[2001:db8::1]:443").unwrap(),
            Verify::Tcp {
                host: "2001:db8::1".into(),
                port: 443
            }
        );
        assert_eq!(
            Verify::parse("api.test:7400").unwrap(),
            Verify::Tcp {
                host: "api.test".into(),
                port: 7400
            }
        );
        assert!(Verify::parse("api.test:http").is_err());
    }

    #[test]
    fn ping_events_are_tagged() {
        let event = PingEvent::Reply {
            seq: 3,
            bytes: 64,
            rtt_ms: 1.25,
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["event"], "reply");
        assert_eq!(serde_json::from_value::<PingEvent>(value).unwrap(), event);
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
    }

    #[test]
    fn sizes_are_labelled_in_decimal_units() {
        assert_eq!(size_label(512), "512 B");
        assert_eq!(size_label(4_096), "4.1 kB");
        assert_eq!(size_label(4_294_967_296), "4.3 GB");
        assert_eq!(size_label(2_000_398_934_016), "2.0 TB");
        assert_eq!(size_label(0), "0 B");
    }

    #[test]
    fn used_percent_rounds_up_like_df() {
        let fs = |used, available| FsUsage {
            mountpoint: "/data".into(),
            source: "/dev/sda3".into(),
            fstype: "ext4".into(),
            size: used + available,
            used,
            available,
        };
        assert_eq!(fs(1, 99).used_percent(), 1);
        assert_eq!(fs(1, 998).used_percent(), 1);
        assert_eq!(fs(0, 0).used_percent(), 0);
    }

    #[test]
    fn the_command_line_names_are_the_wire_names() {
        for name in RestartTarget::NAMES {
            let target: RestartTarget = name.parse().unwrap();
            assert_eq!(serde_json::to_value(target).unwrap(), name);
        }
        for name in WifiSecurity::NAMES {
            let security: WifiSecurity = name.parse().unwrap();
            assert_eq!(serde_json::to_value(security).unwrap(), name);
        }
        assert!("reboot".parse::<RestartTarget>().is_err());
    }

    #[test]
    fn only_the_stepped_commands_are_jobs() {
        assert!(Command::Speedtest {
            max_size: None,
            tests: None,
            direct: false
        }
        .is_job());
        assert!(Command::StorageGrow { check: true }.is_job());
        assert!(!Command::Status.is_job());
        assert!(!Command::Storage.is_job());
    }

    #[test]
    fn hex_and_chunks() {
        assert_eq!(hex(&[0x0a, 0xff]), "0aff");
        assert_eq!(unhex("0aFF"), Some(vec![0x0a, 0xff]));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("zz"), None);

        assert_eq!(decode_chunk("YWJj").unwrap(), b"abc");
        assert_eq!(decode_chunk("!").unwrap_err(), "the chunk is not base64");
        assert!(decode_chunk("").unwrap_err().starts_with("a chunk is 1 to"));
    }

    #[test]
    fn passwords() {
        assert!(check_password("abc").is_ok());
        assert!(check_password("").is_err());
        assert!(check_password("a:b").is_err());
        assert!(check_password("a\nb").is_err());
    }
}
