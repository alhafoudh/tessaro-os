//! What every `tessaro-ctl` command does, whichever transport it came in on.
//!
//! The state machine in `agent.rs` never sees any of this. A change to the
//! configuration is: validate, commit to `tessaro.db` in one transaction,
//! re-render `generated.env` and the policy, then restart exactly what reads
//! the keys that changed - the browser, Weston, or the agent itself. The
//! agent restarting itself is how an agent setting takes effect: it costs
//! nothing on screen, and it keeps the state machine a single sequential task
//! with one `Config` for its whole life.
//!
//! Anything that would take this process down with it - restarting the agent
//! or Weston (the agent is `PartOf=` it), a reboot - is returned as an
//! `After` and run by the API server once the reply is on the wire.
//!
//! All file I/O is blocking and goes through `blocking()`: `spawn_blocking`
//! under a deadline, so the one runtime thread never waits on a disk.
//!
//! This module has the types, `handle` and the commands that only read;
//! the rest is split by what it acts on, each an `impl Control` of its own:
//! `settings` (changing them, and probation), `access` (the claim, tokens,
//! passwords, SSH keys), `network` (WiFi and the profiles' inputs),
//! `watchers` (what is kept true with nobody asking), `page` (the page on
//! screen: reload, eval, the keyboard), `screen` (its power), `bridge`
//! (`window.tessaro` and the injected script), `schedules` (the
//! schedules and their systemd units), `certs` (the extra certificate
//! authorities), `policies` (the browser policies) and `printers` (the
//! printers and their CUPS queues).

mod access;
mod bridge;
mod certs;
mod network;
mod page;
mod policies;
mod printers;
mod schedules;
mod screen;
mod settings;
mod watchers;

pub use bridge::BridgeSetup;

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::keys::{self, Key};
use protocol::{
    Command, Done, KeyInfo, LogPage, NodeInfo, Pending, RestartTarget, Screenshot, Setting,
    Settings, Source, Status, TokenInfo,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::api::sessions::{self, Sessions};
use crate::audio::Audio;
use crate::auth::Auth;
use crate::cdp::session::SessionHandle;
use crate::db::Db;
use crate::deadline::{blocking, within};
use crate::display;
use crate::files::Files;
use crate::log::Log;
use crate::mdns::Mdns;
use crate::nm::profiles;
use crate::nm::Network;
use crate::paths::Paths;
use crate::render;
use crate::speedtest;
use crate::state::{self, State};
use crate::storage;
use crate::sync::lock;
use crate::systemd::Bus;
use crate::time::Time;
use crate::updates::Updates;
use crate::watchdog::Heartbeat;

/// One DevTools command from the control plane.
const CDP_LIMIT: Duration = Duration::from_secs(10);

/// A screenshot of a 4K page can take a while to encode.
const SCREENSHOT_LIMIT: Duration = Duration::from_secs(30);

/// access.session_timeout when neither the image nor the device sets it: a
/// week.
const DEFAULT_SESSION_TIMEOUT: u64 = 7 * 24 * 3600;

/// Who is asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The unix socket: root on the device.
    Local,
    /// TCP, with a valid token.
    Token { id: String, peer: SocketAddr },
    /// TCP, no valid token: the public endpoints (`api::Endpoint::PUBLIC`),
    /// and every endpoint while the device is unclaimed. Webconfig on an
    /// unclaimed device is one of these; once it is claimed a browser signs
    /// in and comes as `Token`, through its session.
    Anonymous { peer: SocketAddr },
    /// The kiosk page itself, through the page bridge: only the commands
    /// `bridge.rs` maps its calls onto.
    Page,
}

impl Caller {
    fn describe(&self) -> String {
        match self {
            Caller::Local => "the local socket".to_string(),
            Caller::Token { id, peer } => format!("{peer} (token {id})"),
            Caller::Anonymous { peer } => peer.ip().to_string(),
            Caller::Page => "the page".to_string(),
        }
    }
}

/// Work that must wait until the reply has been sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After {
    /// Restart a unit to apply a change. When this process goes down with
    /// it, the browser sessions are handed over to the next one.
    Restart(String),
    /// Restart a unit because someone asked to (`device restart`): the
    /// browser sessions end with this process.
    RestartAsked(String),
    Reboot,
    /// Re-render the network profiles from the saved settings: the hotspot
    /// after a claim, an unclaim or a new password.
    Network,
}

pub struct Reply {
    pub result: Result<Value, String>,
    pub after: Option<After>,
}

impl Reply {
    fn ok<T: Serialize>(value: T) -> Self {
        Self {
            result: serde_json::to_value(value).map_err(|err| err.to_string()),
            after: None,
        }
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            result: Err(message.into()),
            after: None,
        }
    }

    fn then(mut self, after: Option<After>) -> Self {
        if self.result.is_ok() {
            self.after = after;
        }
        self
    }
}

impl<T: Serialize> From<Result<T, String>> for Reply {
    fn from(outcome: Result<T, String>) -> Self {
        match outcome {
            Ok(value) => Reply::ok(value),
            Err(err) => Reply::err(err),
        }
    }
}

pub struct Identity {
    pub id: String,
    pub name: String,
    pub machine: String,
    pub fingerprint: String,
}

pub struct Control {
    log: Arc<Log>,
    paths: Paths,
    /// The image defaults for every registry key, from the process
    /// environment systemd filled from the `/usr/lib` env file.
    defaults: HashMap<String, String>,
    /// `tessaro.db`: the settings, the tokens, the network passwords (never
    /// in the settings), the schedules and the printers.
    db: Db,
    /// One reconcile of the schedules' units at a time (`schedules`).
    reconciling: tokio::sync::Mutex<()>,
    cups: Arc<crate::printer::Cups>,
    /// One change to the CUPS queues at a time (`printers`).
    printing: tokio::sync::Mutex<()>,
    /// Why the last reconcile left a printer out of CUPS, if it did.
    printer_problem: Mutex<Option<String>>,
    /// Held by a printer discovery for as long as it runs.
    printer_discovering: Arc<tokio::sync::Mutex<()>>,
    /// What the `tokens` table holds, kept in memory so verifying a token is
    /// not a disk read. Only ever replaced after a successful write.
    auth: Mutex<Auth>,
    session: SessionHandle,
    bus: Bus,
    identity: Identity,
    mdns: Mutex<Option<Mdns>>,
    /// Serialises every change, so two clients cannot interleave a commit
    /// with a render. The store's write transactions do the same across
    /// processes.
    writes: tokio::sync::Mutex<()>,
    /// When the guarded change on probation reverts, if one is.
    probation: Mutex<Option<Instant>>,
    shutdown: watch::Receiver<bool>,
    /// The kiosk URL, expanded, that this agent process started with and is
    /// driving the browser to. It never changes: a new one needs a restart.
    agent_url: String,
    /// The local proxy while network.proxy.url is set, as this agent process
    /// started with it; a change restarts the agent (`Consumer::Agent`).
    proxy: Option<SocketAddr>,
    updates: Arc<Updates>,
    files: Arc<Files>,
    /// Held by the thread running a speed test, for as long as it runs.
    speedtest: Arc<tokio::sync::Mutex<()>>,
    /// Held by the thread growing /data, the same way.
    storage_grow: Arc<tokio::sync::Mutex<()>>,
    network: Arc<Network>,
    audio: Arc<Audio>,
    time: Arc<Time>,
    /// Wakes `watch_welcome` early, when the claim changes.
    welcome: tokio::sync::Notify,
    /// Whether the last public address lookup answered: the device is
    /// online. `None` until one has been tried. Not the cached address, which
    /// a failure leaves in place.
    online: Mutex<Option<bool>>,
    /// When a client last asked for the welcome values (Quick Setup does,
    /// every few seconds), which keeps the online check running while
    /// someone is looking at it.
    portal_seen: Mutex<Option<Instant>>,
    /// Set once, by `start_bridge`.
    bridge: std::sync::OnceLock<Arc<bridge::Bridge>>,
    /// How busy the CPU was over `watch_cpu`'s last interval, in percent.
    cpu: Mutex<Option<u8>>,
    /// Webconfig's browser sessions (`api::sessions`). Here, not in the API
    /// server, because a restart this process makes of itself hands them
    /// over (`run_after`).
    sessions: Arc<Sessions>,
    /// access.session_timeout in seconds, as last read from the settings.
    session_timeout: AtomicU64,
}

impl Control {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        log: Arc<Log>,
        paths: Paths,
        db: Db,
        defaults: HashMap<String, String>,
        auth: Auth,
        session: SessionHandle,
        bus: Bus,
        identity: Identity,
        shutdown: watch::Receiver<bool>,
        agent_url: String,
        proxy: Option<SocketAddr>,
    ) -> Arc<Self> {
        let session_timeout = defaults
            .get(keys::find(keys::SESSION_TIMEOUT).map_or("", |key| key.env))
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_SESSION_TIMEOUT);
        Arc::new(Self {
            sessions: Arc::new(Sessions::new(&identity.id)),
            session_timeout: AtomicU64::new(session_timeout),
            agent_url,
            proxy,
            updates: Updates::new(Arc::clone(&log), paths.clone()),
            files: Files::new(Arc::clone(&log), paths.clone()),
            network: Network::new(Arc::clone(&log), paths.clone(), db.clone()),
            audio: Audio::new(Arc::clone(&log), &paths),
            time: Time::new(Arc::clone(&log), &paths),
            db,
            reconciling: tokio::sync::Mutex::new(()),
            cups: crate::printer::Cups::new(Arc::clone(&log), &paths),
            printing: tokio::sync::Mutex::new(()),
            printer_problem: Mutex::new(None),
            printer_discovering: Arc::new(tokio::sync::Mutex::new(())),
            log,
            paths,
            defaults,
            auth: Mutex::new(auth),
            session,
            bus,
            identity,
            mdns: Mutex::new(None),
            writes: tokio::sync::Mutex::new(()),
            probation: Mutex::new(None),
            welcome: tokio::sync::Notify::new(),
            online: Mutex::new(None),
            portal_seen: Mutex::new(None),
            shutdown,
            speedtest: Arc::new(tokio::sync::Mutex::new(())),
            storage_grow: Arc::new(tokio::sync::Mutex::new(())),
            bridge: std::sync::OnceLock::new(),
            cpu: Mutex::new(None),
        })
    }

    /// A command run in steps (`Command::is_job`), validated and started:
    /// the API keeps what it produces as a job, the page bridge hands it to
    /// the page.
    pub fn stream(&self, caller: &Caller, command: Command) -> Result<Stream, String> {
        match command {
            Command::Speedtest {
                max_size,
                tests,
                direct,
            } => self
                .speedtest(caller, max_size, tests, direct)
                .map(Stream::Speedtest),
            Command::StorageGrow { check } => self.storage_grow(caller, check).map(Stream::Grow),
            Command::NetPing {
                host,
                count,
                interval_ms,
                timeout_ms,
                interface,
            } => {
                let plan = crate::ping::Plan::new(host, count, interval_ms, timeout_ms, interface)?;
                Ok(Stream::Ping {
                    total: plan.total(),
                    steps: self.net_ping(caller, plan),
                })
            }
            Command::PrinterDiscover => self.printer_discover(caller).map(Stream::Printers),
            _ => Err("that command is not a stream".to_string()),
        }
    }

    /// Grow /data, or with `check` only say how; the server streams what it
    /// sends. Refused while another grow is still running.
    fn storage_grow(
        &self,
        caller: &Caller,
        check: bool,
    ) -> Result<tokio::sync::mpsc::Receiver<storage::Step>, String> {
        // The update was checked against the partitions as they are, and
        // growing /data moves the last one.
        if !check && self.updates.is_staged() {
            return Err(
                "an update is staged against this disk's partitions; grow /data after it is \
                 applied, or `tessaro-ctl update cancel` it"
                    .to_string(),
            );
        }
        let lock = Arc::clone(&self.storage_grow)
            .try_lock_owned()
            .map_err(|_| "/data is already being grown on this device".to_string())?;
        if !check {
            self.log
                .info(format!("storage grow requested by {}", caller.describe()));
        }
        Ok(storage::start(
            storage::Sources::new(&self.paths),
            check,
            lock,
            Arc::clone(&self.log),
        ))
    }

    /// Start a speed test; the server streams what it sends. Refused while
    /// another one - even an abandoned one - is still running.
    fn speedtest(
        &self,
        caller: &Caller,
        max_size: Option<u64>,
        tests: Option<u32>,
        direct: bool,
    ) -> Result<tokio::sync::mpsc::Receiver<speedtest::Step>, String> {
        let mut plan = speedtest::Plan::new(max_size, tests)?;
        // Through the local proxy while there is one, unless asked not to.
        plan.proxy = self.proxy.filter(|_| !direct);
        let lock = Arc::clone(&self.speedtest)
            .try_lock_owned()
            .map_err(|_| "a speed test is already running on this device".to_string())?;
        self.log.info(format!(
            "speed test requested by {}{}",
            caller.describe(),
            match plan.proxy {
                Some(_) => ", through the proxy",
                None if direct => ", around the proxy",
                None => "",
            }
        ));
        Ok(speedtest::start(plan, lock, Arc::clone(&self.log)))
    }

    /// Ping a host from the device; the server streams what comes back.
    fn net_ping(
        &self,
        caller: &Caller,
        plan: crate::ping::Plan,
    ) -> tokio::sync::mpsc::Receiver<crate::ping::Step> {
        self.log.debug(format!(
            "net ping {} requested by {}",
            plan.host,
            caller.describe()
        ));
        crate::ping::start(plan, Arc::clone(&self.log))
    }

    pub fn set_mdns(&self, mdns: Option<Mdns>) {
        *lock(&self.mdns) = mdns;
    }

    pub fn claimed(&self) -> bool {
        lock(&self.auth).claimed()
    }

    /// The id of the token `presented` belongs to.
    pub fn verify(&self, presented: &str) -> Option<String> {
        lock(&self.auth)
            .verify(presented)
            .map(|entry| entry.id.clone())
    }

    /// The stored hash of token `id`, while the device has it: what keeps a
    /// browser session tied to its token.
    pub fn token_sha(&self, id: &str) -> Option<String> {
        lock(&self.auth)
            .tokens
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.sha256.clone())
    }

    pub fn token_info(&self, id: &str) -> Option<TokenInfo> {
        self.token_list().into_iter().find(|token| token.id == id)
    }

    pub fn sessions(&self) -> Arc<Sessions> {
        Arc::clone(&self.sessions)
    }

    /// How long a browser session lasts unused (access.session_timeout).
    pub fn session_timeout(&self) -> Duration {
        Duration::from_secs(self.session_timeout.load(Ordering::Relaxed))
    }

    /// Unclaimed, and nothing set on it yet: Webconfig opens on Quick Setup.
    pub async fn fresh(&self) -> bool {
        if self.claimed() {
            return false;
        }
        self.read_state()
            .await
            .is_ok_and(|state| state.settings.is_empty())
    }

    /// Take over the sessions the previous agent process handed over.
    pub async fn load_sessions(&self) {
        let sessions = Arc::clone(&self.sessions);
        let file = self.paths.run_dir.join(sessions::HANDOVER);
        match blocking("taking over the browser sessions", move || {
            sessions.load(&file)
        })
        .await
        {
            Ok(0) => {}
            Ok(count) => self
                .log
                .info(format!("api: took over {count} browser session(s)")),
            Err(err) => self.log.info(format!("api: browser sessions: {err}")),
        }
    }

    /// Hand the browser sessions to the next agent process, before a
    /// restart this one makes to apply a change.
    async fn save_sessions(&self) {
        let sessions = Arc::clone(&self.sessions);
        let file = self.paths.run_dir.join(sessions::HANDOVER);
        let timeout = self.session_timeout();
        match blocking("handing over the browser sessions", move || {
            sessions.save(&file, timeout)
        })
        .await
        {
            Ok(0) => {}
            Ok(count) => self
                .log
                .info(format!("api: handing over {count} browser session(s)")),
            Err(err) => self.log.info(format!("api: browser sessions: {err}")),
        }
    }

    pub fn node(&self) -> NodeInfo {
        NodeInfo {
            id: self.identity.id.clone(),
            name: self.identity.name.clone(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            machine: self.identity.machine.clone(),
            fingerprint: self.identity.fingerprint.clone(),
            claimed: self.claimed(),
        }
    }

    pub async fn handle(self: &Arc<Self>, caller: &Caller, command: Command) -> Reply {
        match command {
            Command::Id => Reply::ok(self.node()),
            Command::Status => self.status().await.into(),
            Command::Keys => self.keys().await.into(),
            Command::Modes => self.modes().await.into(),
            Command::Net => self.net().await.into(),
            Command::Get { key } => self.get(key).await.into(),
            Command::Set {
                values,
                if_revision,
                apply,
                verify,
            } => {
                // The captive flag follows network.wifi.captive at once.
                let captive = values.contains_key(protocol::keys::WIFI_CAPTIVE);
                let changes = values.into_iter().map(|(k, v)| (k, Some(v))).collect();
                let reply = self
                    .change(caller, changes, if_revision, apply, verify, None)
                    .await;
                if captive {
                    self.nudge_welcome();
                }
                reply
            }
            Command::Unset {
                keys,
                if_revision,
                apply,
                verify,
            } => {
                let changes = keys.into_iter().map(|k| (k, None)).collect();
                self.change(caller, changes, if_revision, apply, verify, None)
                    .await
            }
            Command::Confirm => self.confirm().await.into(),
            Command::Navigate { url } => self.navigate(&url).await.into(),
            Command::Restart { what } => self.restart(what).await,
            Command::Reboot => {
                self.log
                    .info(format!("reboot requested by {}", caller.describe()));
                Reply::ok(Done::new("rebooting")).then(Some(After::Reboot))
            }
            Command::Screenshot => self.screenshot().await.into(),
            Command::Reload => self.reload().await.into(),
            Command::ClearCache => self.clear_cache().await.into(),
            Command::Eval {
                code,
                timeout_ms,
                await_promise,
                user_gesture,
            } => self
                .eval(caller, &code, timeout_ms, await_promise, user_gesture)
                .await
                .into(),
            Command::Keyboard { show, selector } => {
                self.keyboard(show, selector.as_deref()).await.into()
            }
            Command::ScreenPower { on } => self.screen_power(caller, on).await.into(),
            Command::Logs {
                unit,
                lines,
                cursor,
            } => self.logs(unit, lines, cursor).await.into(),
            Command::Welcome => {
                self.portal_seen();
                self.welcome().await.into()
            }
            // `Command::is_job`: started through `stream`.
            Command::Speedtest { .. }
            | Command::NetPing { .. }
            | Command::StorageGrow { .. }
            | Command::PrinterDiscover => Reply::err("that command runs as a job"),
            // The hotspot's security follows the claim, re-applied once the
            // answer is out: whoever claims through the hotspot gets its new
            // password before the hotspot drops them.
            Command::Claim { name } => {
                let reply: Reply = self.claim(caller, &name).await.into();
                reply.then(Some(After::Network))
            }
            Command::TokenCreate { name } => self.token_create(caller, &name).await.into(),
            Command::TokenList => Reply::ok(self.token_list()),
            Command::TokenRevoke { id } => {
                let reply: Reply = self.token_revoke(caller, &id).await.into();
                reply.then(Some(After::Network))
            }
            Command::PasswordSet { password } => self.password_set(caller, password).await.into(),
            Command::Unclaim => {
                let reply: Reply = self.unclaim(caller).await.into();
                reply.then(Some(After::Network))
            }
            Command::FactoryReset => self.factory_reset(caller).await,
            Command::SshAuthorize { key } => self.ssh_authorize(caller, &key).await.into(),
            Command::SshKeyList => self.ssh_key_list().await.into(),
            Command::SshKeyRevoke { key } => self.ssh_key_revoke(caller, &key).await.into(),
            Command::UpdateBegin(upload) => {
                self.updates.begin(&caller.describe(), upload).await.into()
            }
            Command::UpdateChunk { offset, data } => self.updates.chunk(offset, data).await.into(),
            Command::UpdateStatus => self.updates.status().await.into(),
            Command::UpdateCommit { .. } if self.network.busy() => {
                Reply::err("a network change is in progress; commit the update once it is done")
            }
            Command::UpdateCommit { wipe_data, reboot } => {
                let who = caller.describe();
                let reply: Reply = self.updates.commit(&who, wipe_data).await.into();
                reply.then(reboot.then_some(After::Reboot))
            }
            Command::UpdateCancel => self.updates.cancel(&caller.describe()).await.into(),
            Command::Ping => Reply::ok(Done::new("pong")),
            Command::Storage => self.storage().await.into(),
            Command::NetProfiles => self.network.profiles().await.into(),
            Command::NetShow { profile } => self.network.show(&profile).await.into(),
            Command::NetLast => self.network.last().await.into(),
            Command::Wifi => self.wifi_status().await.into(),
            Command::WifiScan { interface, rescan } => {
                self.network.scan(interface, rescan).await.into()
            }
            Command::WifiJoin {
                ssid,
                psk,
                security,
                hidden,
                verify,
            } => self.join(caller, ssid, psk, security, hidden, verify).await,
            Command::HotspotPassword => {
                let reply: Reply = self.hotspot_password(caller).await.into();
                reply.then(Some(After::Network))
            }
            Command::FilesList { path, recursive } => {
                self.files.list(&path, recursive).await.into()
            }
            Command::FilesBegin { path, size, mtime } => {
                let who = caller.describe();
                self.files.begin(&who, &path, size, mtime).await.into()
            }
            Command::FilesChunk { path, offset, data } => {
                let who = caller.describe();
                self.files.chunk(&who, &path, offset, data).await.into()
            }
            Command::FilesRead { path, offset, len } => {
                self.files.read(&path, offset, len).await.into()
            }
            Command::FilesMkdir { path } => {
                self.files.mkdir(&caller.describe(), &path).await.into()
            }
            Command::FilesMove { from, to } => {
                let who = caller.describe();
                self.files.rename(&who, &from, &to).await.into()
            }
            Command::FilesDelete { paths, recursive } => {
                let who = caller.describe();
                self.files.delete(&who, paths, recursive).await.into()
            }
            Command::AudioStatus => self.audio_status().await.into(),
            Command::AudioTest { input } => self.audio_test(&caller.describe(), input).await.into(),
            Command::TimeStatus => self.time_status().await.into(),
            Command::TimeZones => self.time.zones(&self.bus).await.into(),
            Command::TimeSync => self.time.sync(&self.bus).await.map(Done::new).into(),
            Command::TimeSet { usec, local } => self
                .time
                .set_clock(&self.bus, &caller.describe(), usec, local)
                .await
                .map(Done::new)
                .into(),
            Command::ProxyStatus => self.proxy_status().await.into(),
            Command::ProxyTest => Reply::ok(self.proxy_test().await),
            Command::NetCertAdd { pem } => self.cert_add(caller, pem).await,
            Command::NetCertList => self.cert_list().await.into(),
            Command::NetCertRevoke { cert } => self.cert_revoke(caller, cert).await,
            Command::BrowserPolicyList => self.policy_list().await.into(),
            Command::BrowserPolicyGet { name } => self.policy_get(name).await.into(),
            Command::BrowserPolicySet {
                name,
                text,
                if_revision,
                position,
            } => {
                self.policy_set(caller, name, text, if_revision, position)
                    .await
            }
            Command::BrowserPolicyMove { name, position } => {
                self.policy_move(caller, name, position).await
            }
            Command::BrowserPolicyRemove { name } => self.policy_remove(caller, name).await,
            Command::BrowserPolicyEffective => self.policy_effective().await.into(),
            Command::ScheduleList => self.schedule_list().await.into(),
            Command::ScheduleCreate { spec } => self.schedule_create(caller, spec).await.into(),
            Command::ScheduleSet {
                schedule,
                name,
                calendar,
                lines,
                on_error,
                timeout_s,
                enabled,
            } => {
                let change = schedules::Change {
                    name,
                    calendar,
                    lines,
                    on_error,
                    timeout_s,
                    enabled,
                };
                self.schedule_set(caller, schedule, change).await.into()
            }
            Command::ScheduleRemove { schedule } => {
                self.schedule_remove(caller, schedule).await.into()
            }
            Command::ScheduleRun { schedule } => self.schedule_run(caller, schedule).await.into(),
            Command::ScheduleCheck { calendar, count } => {
                self.schedule_check(calendar, count).await.into()
            }
            Command::PrinterList => self.printer_list().await.into(),
            Command::PrinterShow { printer } => self.printer_show(printer).await.into(),
            Command::PrinterCreate { spec } => self.printer_create(caller, spec).await.into(),
            Command::PrinterRemove { printer } => self.printer_remove(caller, printer).await.into(),
            Command::PrinterDefault { printer } => {
                self.printer_default(caller, printer).await.into()
            }
            Command::PrinterTest { printer } => self.printer_test(caller, printer).await.into(),
            Command::PrinterPrint {
                printer,
                data,
                path,
                copies,
                media,
                title,
            } => self
                .printer_print(caller, Some(printer), data, path, copies, media, title)
                .await
                .into(),
            Command::PrinterJobs { printer } => self.printer_jobs(printer).await.into(),
            Command::PrinterCancel { job } => self.printer_cancel(caller, job).await.into(),
        }
    }

    /// A page of the journal: the last `lines` entries, or everything after
    /// `cursor`.
    async fn logs(
        &self,
        unit: Option<String>,
        lines: Option<u32>,
        cursor: Option<String>,
    ) -> Result<LogPage, String> {
        let mut command = journal(unit.as_deref(), lines, cursor.as_deref())?;
        let output = within("journalctl", LOGS, command.output())
            .await
            .map_err(|expired| expired.to_string())?
            .map_err(|err| format!("journalctl: {err}"))?;
        Ok(log_page(&String::from_utf8_lossy(&output.stdout), cursor))
    }

    /// Resume whatever update the staging directory holds.
    pub async fn load_update(&self) {
        self.updates.load().await;
    }

    // --- reading -----------------------------------------------------------

    async fn read_state(&self) -> Result<State, String> {
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        let state = blocking("reading the settings", move || Ok(db.read::<State>(&log))).await?;
        // The API checks every session cookie against this, and reading the
        // store for each would be a disk read per request.
        let seconds = state::setting(&state.settings, &self.defaults, keys::SESSION_TIMEOUT)
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_SESSION_TIMEOUT);
        self.session_timeout.store(seconds, Ordering::Relaxed);
        Ok(state)
    }

    /// Change the settings in one write transaction, off the runtime thread.
    async fn update_state<R: Send + 'static>(
        &self,
        what: &'static str,
        change: impl FnOnce(&mut State) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        let db = self.db.clone();
        blocking(what, move || db.update(change)).await
    }

    /// Change the tokens the same way. The copy in memory that tokens are
    /// verified against follows, and only once the write has succeeded.
    async fn update_auth<R: Send + 'static>(
        &self,
        what: &'static str,
        change: impl FnOnce(&mut Auth) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        let db = self.db.clone();
        let (auth, out) = blocking(what, move || {
            db.update(|auth: &mut Auth| {
                let out = change(auth)?;
                Ok((auth.clone(), out))
            })
        })
        .await?;
        *lock(&self.auth) = auth;
        Ok(out)
    }

    /// What the device reports now: derived name, node id, addresses, and
    /// the hotspot's name, which follows `device.name`.
    async fn live(&self) -> state::Live {
        let paths = self.paths.clone();
        let mut live = blocking("reading the network", move || Ok(render::live(&paths)))
            .await
            .unwrap_or_default();
        if let Ok(state) = self.read_state().await {
            live.values.insert(
                "network.wifi.hotspot_ssid".to_string(),
                profiles::hotspot_ssid(&self.node_name_for(&state.settings)),
            );
        }
        live
    }

    async fn net(&self) -> Result<protocol::Net, String> {
        // naked: the lookup's every phase is under its own within()
        self.refresh_public_ip_now().await;
        let paths = self.paths.clone();
        let mut net = blocking("reading the network", move || {
            Ok(crate::net::snapshot(&paths))
        })
        .await?;
        net.proxy = self.proxy_setting().await?.map(|(url, _)| url);
        Ok(net)
    }

    /// network.proxy.url masked and network.proxy.bypass, as set now;
    /// `None` without a proxy.
    async fn proxy_setting(&self) -> Result<Option<(String, Vec<String>)>, String> {
        let state = self.read_state().await?;
        let value =
            |name: &str| state::setting(&state.settings, &self.defaults, name).unwrap_or_default();
        let url = value(keys::PROXY_URL);
        if url.trim().is_empty() {
            return Ok(None);
        }
        let bypass = keys::parse_bypass(&value(keys::PROXY_BYPASS)).unwrap_or_default();
        Ok(Some((keys::masked_proxy(&url), bypass)))
    }

    async fn proxy_status(&self) -> Result<protocol::ProxyStatus, String> {
        let setting = self.proxy_setting().await?;
        Ok(protocol::ProxyStatus {
            bypass: setting
                .as_ref()
                .map(|(_, bypass)| bypass.clone())
                .unwrap_or_default(),
            url: setting.map(|(url, _)| url),
            listen: self.paths.proxy_listen.to_string(),
            unit: self.bus.active_state(&self.paths.proxy_unit).await,
        })
    }

    /// Cloudflare's trace through the local proxy, whether or not this agent
    /// started with one - a proxy set a moment ago is tested before the
    /// restarted agent is up. Answered either way, with what went wrong.
    async fn proxy_test(&self) -> protocol::ProxyTested {
        let failed = |error: String| protocol::ProxyTested {
            ip: None,
            error: Some(error),
        };
        match self.proxy_setting().await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return failed("no proxy is set; `tessaro-ctl network proxy set URL`".to_string())
            }
            Err(err) => return failed(err),
        }
        let http = crate::net::public_ip_client(Some(self.paths.proxy_listen));
        // naked: public_ip's every phase is under its own within()
        match crate::net::public_ip(&http).await {
            Ok(ip) => {
                self.log.info(format!("proxy test: the internet sees {ip}"));
                protocol::ProxyTested {
                    ip: Some(ip.to_string()),
                    error: None,
                }
            }
            Err(err) => {
                self.log.info(format!("proxy test failed: {err}"));
                failed(err)
            }
        }
    }

    async fn storage(&self) -> Result<protocol::Storage, String> {
        let paths = self.paths.clone();
        blocking("reading the storage", move || storage::snapshot(&paths)).await
    }

    async fn status(&self) -> Result<Status, String> {
        let state = self.read_state().await?;
        let kiosk_url = self.expanded_url(&state.settings).await;

        let mut units = BTreeMap::new();
        for unit in [
            &self.paths.weston_unit,
            &self.paths.kiosk_unit,
            &self.paths.agent_unit,
        ] {
            units.insert(unit.clone(), self.bus.active_state(unit).await);
        }

        let os_release = self.paths.os_release.clone();
        let (os, image_version) = blocking("reading os-release", move || {
            Ok(os_release_fields(&os_release))
        })
        .await
        .unwrap_or_default();
        let paths = self.paths.clone();
        let (hardware, memory) = blocking("reading the hardware", move || {
            Ok((
                Some(crate::hardware::hardware(&paths)),
                crate::hardware::memory(&paths.meminfo),
            ))
        })
        .await
        .unwrap_or_default();
        let data = self
            .storage()
            .await
            .ok()
            .and_then(|storage| storage::data(&storage));

        let wanted = self.audio_wanted_from(&state.settings);
        let audio = self.audio.status(&wanted).await;
        let time = self.time.summary(&self.bus).await;
        // naked: a /proc read under blocking()'s within()
        let devtools = self.session.others().await > 0;
        let screen_on = crate::power::send(&self.paths.power_socket, "status")
            .await // naked: power::send bounds itself with within()
            .ok();

        Ok(Status {
            screen_on,
            bridge: self.bridge_status(),
            os,
            image_version,
            data,
            node: self.node(),
            revision: state.revision,
            kiosk_url,
            current_url: self.session.current_url(),
            browser_answering: self.session.is_up(),
            units,
            pending: self.pending(&state),
            maintenance: state::maintenance(&state.settings, &self.defaults),
            debug_screen: state::debug_screen(&state.settings, &self.defaults),
            devtools,
            audio: Some(audio),
            time,
            hardware,
            memory,
            cpu_percent: *lock(&self.cpu),
        })
    }

    /// One of `keys::TEMPLATES` as set, else the image default.
    fn template(&self, settings: &BTreeMap<String, String>, name: &str) -> String {
        state::setting(settings, &self.defaults, name).unwrap_or_default()
    }

    /// The registry, documented: what each key accepts, the image default,
    /// and what this device has set. `data.<name>` appears once as the
    /// template entry, saying which custom values exist, then once per
    /// custom value, saying whether browser.url uses it.
    async fn keys(&self) -> Result<Vec<KeyInfo>, String> {
        let state = self.read_state().await?;
        let live = self.live().await;
        let mut out: Vec<KeyInfo> = keys::KEYS
            .iter()
            .filter(|key| self.paths.offers(key))
            .map(|key| KeyInfo {
                default: self.defaults.get(key.env).cloned(),
                // A read-only key's value is what the device reports now.
                value: if key.kind == keys::Kind::ReadOnly {
                    Some(live.values.get(key.name).cloned().unwrap_or_default())
                } else {
                    state.settings.get(key.name).cloned()
                },
                ..KeyInfo::from(key)
            })
            .collect();

        let templates: Vec<(&str, String)> = keys::TEMPLATES
            .iter()
            .map(|(name, _)| (*name, self.template(&state.settings, name)))
            .collect();
        let custom: Vec<(&String, &String)> = state
            .settings
            .iter()
            .filter(|(name, _)| keys::param_name(name).is_some())
            .collect();

        let defined = if custom.is_empty() {
            "No custom values are defined on this device yet.".to_string()
        } else {
            format!(
                "Defined on this device: {}.",
                custom
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        out.push(KeyInfo {
            doc: format!("{} {defined}", keys::DATA.doc),
            ..KeyInfo::from(&keys::DATA)
        });

        for (name, value) in custom {
            // The placeholder is the key itself.
            let users: Vec<&str> = templates
                .iter()
                .filter(|(_, template)| keys::placeholders(template).contains(&name.as_str()))
                .map(|(key, _)| *key)
                .collect();
            let usage = if users.is_empty() {
                format!("No template uses it; add {{{name}}} to browser.url or another template to use it.")
            } else {
                format!("{} uses it as {{{name}}}.", users.join(" and "))
            };
            out.push(KeyInfo {
                name: name.clone(),
                value: Some(value.clone()),
                doc: format!("Custom value. {usage}"),
                ..KeyInfo::from(&keys::DATA)
            });
        }
        Ok(out)
    }

    fn pending(&self, state: &State) -> Option<Pending> {
        let change = state.pending.as_ref()?;
        let seconds_left = lock(&self.probation)
            .map(|deadline| deadline.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or(protocol::CONFIRM_SECONDS);
        Some(Pending {
            key: change.key.clone(),
            value: change.value.clone(),
            previous: change.previous.clone(),
            seconds_left,
        })
    }

    async fn modes(&self) -> Result<Vec<protocol::Connector>, String> {
        let drm = self.paths.drm.clone();
        blocking("reading DRM connectors", move || {
            Ok(display::connectors(&drm))
        })
        .await
    }

    async fn get(&self, key: Option<String>) -> Result<Settings, String> {
        if key.as_deref() == Some("network.public_ip") {
            // naked: the lookup's every phase is under its own within()
            self.refresh_public_ip_now().await;
        }
        let state = self.read_state().await?;
        // The registry, then every custom data.* that is set, by its name.
        let wanted: Vec<(String, &Key)> = match &key {
            Some(name) => {
                let key = keys::find(name).ok_or_else(|| unknown(name))?;
                if !self.paths.offers(key) {
                    return Err(not_offered(key));
                }
                vec![(name.clone(), key)]
            }
            None => keys::KEYS
                .iter()
                .filter(|key| self.paths.offers(key))
                .map(|key| (key.name.to_string(), key))
                .chain(
                    state
                        .settings
                        .keys()
                        .filter(|name| keys::param_name(name).is_some())
                        .map(|name| (name.clone(), &keys::DATA)),
                )
                .collect(),
        };

        let live = self.live().await;
        let settings = wanted
            .into_iter()
            .map(|(name, key)| match state.settings.get(&name) {
                _ if key.kind == keys::Kind::ReadOnly => Setting {
                    value: Some(live.values.get(key.name).cloned().unwrap_or_default()),
                    key: name,
                    env: String::new(),
                    source: Source::Live,
                },
                Some(value) => Setting {
                    key: name,
                    env: key.env.to_string(),
                    value: Some(value.clone()),
                    source: Source::Set,
                },
                None => Setting {
                    key: name,
                    env: key.env.to_string(),
                    value: self.defaults.get(key.env).cloned(),
                    source: Source::Default,
                },
            })
            .collect();

        Ok(Settings {
            revision: state.revision,
            settings,
        })
    }

    // --- the browser -------------------------------------------------------

    async fn navigate(&self, url: &str) -> Result<Done, String> {
        let url = keys::validate(
            keys::find("browser.url").expect("browser.url is a key"),
            url,
        )?;
        let result = self
            .session
            .call(
                &Heartbeat::detached(),
                "Page.navigate",
                json!({ "url": url }),
                CDP_LIMIT,
            )
            .await?; // naked: SessionHandle::call bounds itself with within()

        match result["errorText"].as_str() {
            Some(text) if !text.is_empty() => Err(format!("Page.navigate: {text}")),
            _ => Ok(Done::new(format!("navigated to {url}"))),
        }
    }

    async fn screenshot(&self) -> Result<Screenshot, String> {
        // A powered-off output paints nothing, and the capture waits for a
        // frame that never comes.
        if self.screen_kept_off().await {
            return Err(
                "the screen is switched off; `tessaro-ctl screen power on` first".to_string(),
            );
        }
        // fromSurface:false - the surface path can hang on this stack.
        let result = self
            .session
            .call(
                &Heartbeat::detached(),
                "Page.captureScreenshot",
                json!({ "format": "jpeg", "quality": 85, "fromSurface": false }),
                SCREENSHOT_LIMIT,
            )
            .await?; // naked: SessionHandle::call bounds itself with within()

        let data = result["data"]
            .as_str()
            .ok_or_else(|| "the browser returned no image".to_string())?;
        Ok(Screenshot {
            format: "jpeg".to_string(),
            data: data.to_string(),
        })
    }

    async fn restart(&self, what: RestartTarget) -> Reply {
        match what {
            RestartTarget::Browser => match self.bus.restart(&self.paths.kiosk_unit).await {
                Ok(()) => Reply::ok(Done::new(format!("restarted {}", self.paths.kiosk_unit))),
                Err(err) => Reply::err(err.to_string()),
            },
            RestartTarget::Weston => restart_after(&self.paths.weston_unit),
            RestartTarget::Agent => restart_after(&self.paths.agent_unit),
        }
    }

    pub async fn run_after(&self, after: After) {
        let outcome = match &after {
            After::Restart(unit) => {
                if *unit == self.paths.agent_unit || *unit == self.paths.weston_unit {
                    self.save_sessions().await;
                }
                self.bus.restart(unit).await
            }
            After::RestartAsked(unit) => self.bus.restart(unit).await,
            After::Reboot => self.bus.reboot().await,
            After::Network => {
                // naked: refresh_network waits only through Network, whose calls are within()
                self.refresh_network().await;
                Ok(())
            }
        };
        if let Err(err) = outcome {
            self.log.info(format!("{after:?}: {err}"));
        }
    }
}

/// A command run in steps, started. Each kind keeps its own step type; the
/// API turns every step into one event of a job.
pub enum Stream {
    Speedtest(tokio::sync::mpsc::Receiver<speedtest::Step>),
    Grow(tokio::sync::mpsc::Receiver<storage::Step>),
    Ping {
        steps: tokio::sync::mpsc::Receiver<crate::ping::Step>,
        /// The longest the whole run can take, from the plan.
        total: Duration,
    },
    Printers(tokio::sync::mpsc::Receiver<Result<protocol::PrinterFound, String>>),
}

/// The most entries one page of the journal carries. A page after a cursor
/// that has more is its last `LOG_PAGE` entries.
const LOG_PAGE: u32 = 10_000;
/// One `journalctl` for a page.
const LOGS: Duration = Duration::from_secs(30);

/// `journalctl` for one page of `logs`.
fn journal(
    unit: Option<&str>,
    lines: Option<u32>,
    cursor: Option<&str>,
) -> Result<tokio::process::Command, String> {
    let mut command = tokio::process::Command::new("journalctl");
    command.args(["--output=json", "--no-pager", "--quiet"]);
    match cursor {
        Some(cursor) => {
            if cursor.is_empty() || cursor.chars().any(char::is_control) {
                return Err("that is not a journal cursor".to_string());
            }
            command
                .arg(format!("--after-cursor={cursor}"))
                .arg(format!("--lines={LOG_PAGE}"));
        }
        None => {
            command.arg(format!("--lines={}", lines.unwrap_or(100).min(LOG_PAGE)));
        }
    }
    if let Some(unit) = unit {
        let valid = !unit.is_empty()
            && unit
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "@._:-*".contains(ch));
        if !valid {
            return Err(format!("{unit:?} is not a unit name"));
        }
        command.arg(format!("--unit={unit}"));
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

/// journalctl's output as a page: one entry per line, the last one's
/// `__CURSOR` where the next page starts, `cursor` when nothing came.
fn log_page(output: &str, cursor: Option<String>) -> LogPage {
    let entries: Vec<Value> = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.to_string())))
        .collect();
    let last = entries
        .iter()
        .rev()
        .find_map(|entry| entry["__CURSOR"].as_str().map(str::to_string));
    LogPage {
        cursor: last.or(cursor),
        entries,
    }
}

/// The refusal of anything but `id` and `claim` on an unclaimed device.
pub const UNCLAIMED: &str = "this device is unclaimed; `tessaro-ctl access claim` it first";

/// `restarting UNIT`, with the restart itself left until the answer is out:
/// the agent goes down with either unit it is asked to restart, and with it
/// every browser session.
fn restart_after(unit: &str) -> Reply {
    Reply::ok(Done::new(format!("restarting {unit}")))
        .then(Some(After::RestartAsked(unit.to_string())))
}

/// `PRETTY_NAME` and `IMAGE_VERSION` from an os-release file.
fn os_release_fields(path: &std::path::Path) -> (Option<String>, Option<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (None, None);
    };
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .map(|value| value.trim().trim_matches('"').to_string())
    };
    (field("PRETTY_NAME"), field("IMAGE_VERSION"))
}

fn unknown(name: &str) -> String {
    keys::unknown(name)
}

/// For a key this device lacks the hardware for (`Paths::offers`).
fn not_offered(key: &Key) -> String {
    match key.only {
        Some(keys::Hardware::PiFirmware) => {
            format!("{} is only available on a Raspberry Pi", key.name)
        }
        None => unknown(key.name),
    }
}

/// A `Control` in a sandbox, for the server's tests.
#[cfg(test)]
pub(crate) use tests::fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use protocol::{
        Applied, AudioStatus, Claimed, HotspotCredentials, Password, Secret, SshAccess, SshKeyInfo,
        TokenCreated, TokenInfo,
    };

    use crate::cdp::session::{self, SessionConfig};
    use crate::secrets::Secrets;
    use crate::{shadow, ssh};

    pub(crate) struct Fixture {
        _dir: tempfile::TempDir,
        pub(crate) control: Arc<Control>,
        pub(crate) paths: Paths,
        _stop: watch::Sender<bool>,
    }

    fn peer() -> SocketAddr {
        "192.0.2.10:50000".parse().unwrap()
    }

    fn anonymous() -> Caller {
        Caller::Anonymous { peer: peer() }
    }

    pub(crate) fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let at = |name: &str| dir.path().join(name).display().to_string();
        let env: HashMap<String, String> = [
            ("KIOSK_STATE_DIR", at("data")),
            ("KIOSK_FILES_DIR", at("files")),
            ("KIOSK_RUN_DIR", at("run")),
            ("KIOSK_POLICY", at("policy.json")),
            ("KIOSK_POLICY_BASE", at("policy-base.json")),
            ("KIOSK_SHADOW", at("etc/shadow")),
            ("KIOSK_AUTHORIZED_KEYS", at("root/.ssh/authorized_keys")),
            // None: nothing runs dropbearkey on the host.
            ("KIOSK_SSH_HOST_KEY_DIRS", String::new()),
            ("KIOSK_DRM", at("drm")),
            // No interfaces, and profiles rendered into the sandbox: nothing
            // here may ever reach this host's own NetworkManager.
            ("KIOSK_SYS_NET", at("sys-net")),
            ("KIOSK_NM_RUN_DIR", at("nm")),
            // No PipeWire: nothing here may reach this host's sound server.
            ("KIOSK_AUDIO_RUNTIME_DIR", at("audio")),
            ("KIOSK_ASOUND_CARDS", at("asound-cards")),
            // Never this host's clock.
            ("KIOSK_MANAGE_CLOCK", "0".to_string()),
            ("KIOSK_PROXY_CONFIG", at("tinyproxy.conf")),
            ("KIOSK_TIMESYNCD_DROPIN", at("timesyncd.conf")),
            // Never this host's systemd: units are only rendered, and the
            // calendar is checked by a stand-in (`fake_analyze`).
            ("KIOSK_MANAGE_SCHEDULES", "0".to_string()),
            ("KIOSK_SYSTEMD_UNIT_DIR", at("units")),
            ("KIOSK_SYSTEMD_ANALYZE", at("systemd-analyze")),
            // Never this host's CUPS: the printers are only stored.
            ("KIOSK_MANAGE_PRINTERS", "0".to_string()),
            // A qemu VM's hardware, never this host's.
            ("KIOSK_DMI", at("dmi")),
            ("KIOSK_DEVICE_TREE", at("device-tree")),
            ("KIOSK_CPUINFO", at("cpuinfo")),
            ("KIOSK_MEMINFO", at("meminfo")),
            // Never made: no captive flag is written.
            ("KIOSK_PORTAL_DIR", at("portal")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        fs::create_dir_all(dir.path().join("etc")).unwrap();
        fs::write(dir.path().join("etc/shadow"), "root::1:0:99999:7:::\n").unwrap();
        fs::write(dir.path().join("policy-base.json"), "{}").unwrap();
        let connector = dir.path().join("drm/card0-HDMI-A-1");
        fs::create_dir_all(&connector).unwrap();
        fs::write(connector.join("status"), "connected\n").unwrap();
        fs::write(connector.join("modes"), "1920x1080\n1280x720\n").unwrap();
        fake_analyze(&dir.path().join("systemd-analyze"));
        let dmi = dir.path().join("dmi");
        fs::create_dir_all(&dmi).unwrap();
        fs::write(dmi.join("sys_vendor"), "QEMU\n").unwrap();
        fs::write(dmi.join("product_name"), "Standard PC (Q35 + ICH9, 2009)\n").unwrap();
        fs::write(
            dir.path().join("cpuinfo"),
            "processor\t: 0\nmodel name\t: QEMU Virtual CPU version 2.5+\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("meminfo"),
            "MemTotal:        4000000 kB\nMemAvailable:    3000000 kB\n",
        )
        .unwrap();

        let paths = Paths::load(&env);
        let defaults: HashMap<String, String> = [
            ("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string()),
            (
                "KIOSK_MAINTENANCE_URL".to_string(),
                "http://127.0.0.1/maintenance.html".to_string(),
            ),
        ]
        .into();
        let (stop, shutdown) = watch::channel(false);
        let log = Arc::new(Log::buffered(true));
        // What the boot oneshot has always done by the time the agent runs.
        render::all(&paths, &defaults, &BTreeMap::new(), &log).unwrap();
        // Nothing listens there; the session just stays down.
        let session = session::spawn(
            SessionConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                timeout: Duration::from_secs(1),
                ping: Duration::from_secs(10),
                reconnect_max: Duration::from_secs(10),
                device_access: false,
            },
            Arc::clone(&log),
            shutdown.clone(),
        );

        let db = Db::open(&paths.state_dir, &log);
        let control = Control::new(
            log,
            paths.clone(),
            db,
            defaults,
            Auth::default(),
            session,
            Bus::none(),
            Identity {
                id: "0".repeat(32),
                name: "test-node".to_string(),
                machine: "qemux86-64".to_string(),
                fingerprint: "f".repeat(64),
            },
            shutdown,
            "http://127.0.0.1/".to_string(),
            None,
        );

        Fixture {
            _dir: dir,
            control,
            paths,
            _stop: stop,
        }
    }

    async fn ok<T: serde::de::DeserializeOwned>(
        control: &Arc<Control>,
        caller: &Caller,
        command: Command,
    ) -> T {
        let reply = control.handle(caller, command).await;
        serde_json::from_value(reply.result.expect("the command should succeed")).unwrap()
    }

    async fn err(control: &Arc<Control>, caller: &Caller, command: Command) -> String {
        control
            .handle(caller, command)
            .await
            .result
            .expect_err("the command should fail")
    }

    fn set(pairs: &[(&str, &str)]) -> Command {
        Command::Set {
            values: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            if_revision: None,
            apply: true,
            verify: Default::default(),
        }
    }

    #[tokio::test]
    async fn status_says_what_the_hardware_is_and_how_much_ram_is_used() {
        let fx = fixture();
        let status: Status = ok(&fx.control, &Caller::Local, Command::Status).await;
        let hardware = status.hardware.unwrap();
        assert_eq!(hardware.vendor.as_deref(), Some("QEMU"));
        assert_eq!(
            hardware.model.as_deref(),
            Some("Standard PC (Q35 + ICH9, 2009)")
        );
        assert_eq!(
            hardware.cpu.as_deref(),
            Some("QEMU Virtual CPU version 2.5+")
        );
        assert_eq!(hardware.cores, Some(1));
        let memory = status.memory.unwrap();
        assert_eq!(memory.total, 4_000_000 * 1024);
        assert_eq!(memory.used_percent(), 25);
    }

    #[tokio::test]
    async fn claim_sets_a_password_issues_a_token_and_only_works_once() {
        let fx = fixture();
        assert!(!fx.control.claimed());

        let claimed: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;

        assert!(fx.control.claimed());
        assert_eq!(
            fx.control.verify(&claimed.token).as_deref(),
            Some(claimed.token_id.as_str())
        );
        let shadow = fs::read_to_string(&fx.paths.shadow).unwrap();
        let hashed = shadow.split(':').nth(1).unwrap();
        assert!(shadow::verify(&claimed.root_password, hashed));

        let again = err(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "thief".into(),
            },
        )
        .await;
        assert!(again.contains("already claimed"), "{again}");
    }

    fn secrets_of(fx: &Fixture) -> Secrets {
        Db::at(&fx.paths.state_dir).read(&Log::buffered(true))
    }

    #[tokio::test]
    async fn the_hotspot_password_follows_the_claim() {
        let fx = fixture();
        let claimed: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;
        let psk = secrets_of(&fx)
            .hotspot_psk
            .expect("claim sets a hotspot password");
        protocol::keys::check_psk(psk.expose()).unwrap();
        // No WiFi device in this sandbox: nothing to show, but it is stored for a
        // WiFi dongle plugged in later.
        assert_eq!(claimed.hotspot, None);

        let rotated: HotspotCredentials =
            ok(&fx.control, &Caller::Local, Command::HotspotPassword).await;
        assert_ne!(rotated.password, psk.expose());
        assert!(rotated.ssid.starts_with("tessaro-"));
        assert_eq!(secrets_of(&fx).hotspot_psk, Some(Secret(rotated.password)));

        let _: Done = ok(&fx.control, &Caller::Local, Command::Unclaim).await;
        assert_eq!(secrets_of(&fx).hotspot_psk, None, "unclaimed means open");
        let refused = err(&fx.control, &Caller::Local, Command::HotspotPassword).await;
        assert!(refused.contains("open"), "{refused}");
    }

    #[tokio::test]
    async fn network_settings_are_checked_before_anything_moves() {
        let fx = fixture();
        let no_apply = err(
            &fx.control,
            &Caller::Local,
            Command::Set {
                values: [("network.ethernet.mode".to_string(), "static".to_string())].into(),
                if_revision: None,
                apply: false,
                verify: Default::default(),
            },
        )
        .await;
        assert!(no_apply.contains("--no-apply"), "{no_apply}");

        let incomplete = err(
            &fx.control,
            &Caller::Local,
            set(&[("network.ethernet.mode", "static")]),
        )
        .await;
        assert!(
            incomplete.contains("needs network.ethernet.address"),
            "{incomplete}"
        );
        let client = err(
            &fx.control,
            &Caller::Local,
            set(&[("network.wifi.mode", "client")]),
        )
        .await;
        assert!(client.contains("network.wifi.ssid"), "{client}");

        let settings: Settings = ok(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("network.ethernet.mode".into()),
            },
        )
        .await;
        assert_eq!(settings.revision, 0, "nothing was saved");
    }

    #[tokio::test]
    async fn network_passwords_are_never_shown() {
        let fx = fixture();
        Db::at(&fx.paths.state_dir)
            .update(|secrets: &mut Secrets| {
                secrets.hotspot_psk = Some(Secret("hotspotsecret1".into()));
                secrets.wifi_psk = Some(Secret("clientsecret22".into()));
                Ok(())
            })
            .unwrap();
        let everything = serde_json::to_string(
            &ok::<Settings>(&fx.control, &Caller::Local, Command::Get { key: None }).await,
        )
        .unwrap()
            + &serde_json::to_string(
                &ok::<Vec<KeyInfo>>(&fx.control, &Caller::Local, Command::Keys).await,
            )
            .unwrap();
        assert!(!everything.contains("hotspotsecret1"));
        assert!(!everything.contains("clientsecret22"));
        assert!(
            everything.contains("tessaro-"),
            "network.wifi.hotspot_ssid is reported"
        );
    }

    #[tokio::test]
    async fn tokens_are_issued_only_against_a_token() {
        let fx = fixture();
        let claimed: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;
        let holder = Caller::Token {
            id: claimed.token_id.clone(),
            peer: peer(),
        };

        let refused = err(
            &fx.control,
            &anonymous(),
            Command::TokenCreate { name: "x".into() },
        )
        .await;
        assert!(refused.contains("token"), "{refused}");

        let created: TokenCreated = ok(
            &fx.control,
            &holder,
            Command::TokenCreate {
                name: "phone".into(),
            },
        )
        .await;
        let list: Vec<TokenInfo> = ok(&fx.control, &holder, Command::TokenList).await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].issued_by, claimed.token_id);
        assert!(fx.control.verify(&created.token).is_some());
    }

    #[tokio::test]
    async fn revoking_the_last_token_unclaims_and_empties_the_password() {
        let fx = fixture();
        let claimed: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;
        assert!(shadow::root_has_password(&fx.paths.shadow).unwrap());

        let _: Done = ok(
            &fx.control,
            &Caller::Local,
            Command::TokenRevoke {
                id: claimed.token_id.clone(),
            },
        )
        .await;

        assert!(!fx.control.claimed());
        assert!(fx.control.verify(&claimed.token).is_none());
        assert!(!shadow::root_has_password(&fx.paths.shadow).unwrap());
        // Revoking removes: nothing of the token is left on disk.
        let auth: Auth = Db::at(&fx.paths.state_dir).read(&Log::buffered(true));
        assert!(auth.tokens.iter().all(|entry| entry.id != claimed.token_id));

        // And the next claim wins.
        let _: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "next".into(),
            },
        )
        .await;
    }

    #[tokio::test]
    async fn unclaim_is_the_fresh_install_state() {
        let fx = fixture();
        let claimed: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;
        let holder = Caller::Token {
            id: claimed.token_id,
            peer: peer(),
        };

        let _: SshAccess = ok(&fx.control, &holder, authorize(SSH_KEY)).await;

        let _: Done = ok(&fx.control, &holder, Command::Unclaim).await;

        assert!(!fx.control.claimed());
        assert!(fx.control.verify(&claimed.token).is_none());
        assert!(!shadow::root_has_password(&fx.paths.shadow).unwrap());
        assert!(ssh::list(&fx.paths.authorized_keys).unwrap().is_empty());
    }

    const SSH_KEY: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIO7gEEtj0g4zaawVIwrP4wxLZQ2TqgASR86NTHDJ66jj a@laptop";
    const SSH_FINGERPRINT: &str = "SHA256:YY7C2uXwz+G0YAPdSZsL/SANSo5RStBfYENHBRMPE7A";

    fn authorize(key: &str) -> Command {
        Command::SshAuthorize { key: key.into() }
    }

    async fn claimed(fx: &Fixture) -> Caller {
        let claimed: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;
        Caller::Token {
            id: claimed.token_id,
            peer: peer(),
        }
    }

    /// `systemd-analyze calendar` for the tests: refuses `bad`, and fires
    /// every expression at the same two UTC times.
    fn fake_analyze(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::write(
            path,
            "#!/bin/sh\n\
             for expression; do :; done\n\
             case \"$expression\" in *bad*)\n\
               echo \"Failed to parse calendar specification '$expression': Invalid argument\" >&2\n\
               exit 1;;\n\
             esac\n\
             echo \"Normalized form: $expression\"\n\
             echo \"    Next elapse: Mon 2026-09-28 07:00:00 UTC\"\n\
             echo \"   Iteration #2: Tue 2026-09-29 07:00:00 UTC\"\n",
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[tokio::test]
    async fn a_schedule_is_created_rendered_changed_and_removed() {
        let fx = fixture();
        let units = || crate::schedules::present(&fx.paths.systemd_unit_dir).unwrap();
        let spec = protocol::ScheduleSpec {
            name: "night".into(),
            enabled: true,
            calendar: vec!["22:00".into()],
            lines: vec!["tessaro-ctl screen power off".into()],
            on_error: protocol::OnError::Stop,
            timeout_s: None,
        };

        let created: protocol::ScheduleInfo = ok(
            &fx.control,
            &Caller::Local,
            Command::ScheduleCreate { spec: spec.clone() },
        )
        .await;
        assert_eq!(created.spec, spec);
        assert_eq!(created.running, 0);
        assert_eq!(
            created.units,
            format!("tessaro-schedule-{}-*@*.service", created.id)
        );
        let rendered = units();
        assert_eq!(rendered.len(), 3, "{rendered:?}");
        let timer =
            String::from_utf8(rendered[&format!("tessaro-schedule-{}.timer", created.id)].clone())
                .unwrap();
        assert!(timer.contains("OnCalendar=22:00\n"));

        // The same name twice, a bad calendar: refused, nothing saved.
        let reply = fx
            .control
            .handle(
                &Caller::Local,
                Command::ScheduleCreate { spec: spec.clone() },
            )
            .await;
        assert!(reply.result.is_err());
        let mut bad = spec.clone();
        bad.name = "other".into();
        bad.calendar = vec!["bad".into()];
        let reply = fx
            .control
            .handle(&Caller::Local, Command::ScheduleCreate { spec: bad })
            .await;
        assert!(reply.result.unwrap_err().contains("Failed to parse"));

        let changed: protocol::ScheduleInfo = ok(
            &fx.control,
            &Caller::Local,
            Command::ScheduleSet {
                schedule: "night".into(),
                name: None,
                calendar: None,
                lines: Some(vec!["true".into(), "false".into()]),
                on_error: Some(protocol::OnError::Continue),
                timeout_s: Some(60),
                enabled: Some(false),
            },
        )
        .await;
        assert_eq!(changed.id, created.id);
        assert!(!changed.spec.enabled);
        assert_eq!(changed.spec.timeout_s, Some(60));
        // A new run template replaces the old one; nothing runs to keep it.
        let rendered = units();
        assert_eq!(rendered.len(), 3, "{rendered:?}");
        let template = rendered
            .iter()
            .find(|(name, _)| name.ends_with("@.service"))
            .map(|(_, body)| String::from_utf8(body.clone()).unwrap())
            .unwrap();
        assert!(template.contains("ExecStart=-/bin/sh -c \"false\"\n"));
        assert!(template.contains("TimeoutStartSec=60s\n"));

        let listed: Vec<protocol::ScheduleInfo> =
            ok(&fx.control, &Caller::Local, Command::ScheduleList).await;
        assert_eq!(listed.len(), 1);

        // How the run unit records a run, read back.
        fs::create_dir_all(fx.paths.schedule_runs_dir()).unwrap();
        fs::write(
            fx.paths.schedule_runs_dir().join(&created.id),
            "1790409110-42 1790409135 exit-code 1\n",
        )
        .unwrap();
        let listed: Vec<protocol::ScheduleInfo> =
            ok(&fx.control, &Caller::Local, Command::ScheduleList).await;
        let run = listed[0].last_run.clone().unwrap();
        assert_eq!(
            (run.started.unix, run.finished.unix),
            (1_790_409_110, 1_790_409_135)
        );
        assert!(!run.succeeded());

        let check: protocol::CalendarCheck = ok(
            &fx.control,
            &Caller::Local,
            Command::ScheduleCheck {
                calendar: vec!["Mon 07:00".into(), "Tue 07:00".into()],
                count: Some(3),
            },
        )
        .await;
        assert_eq!(check.normalized, ["Mon 07:00", "Tue 07:00"]);
        // Both fire at the same two times: merged, not repeated.
        assert_eq!(check.next.len(), 2);

        // Not managed here: nothing to start.
        let reply = fx
            .control
            .handle(
                &Caller::Local,
                Command::ScheduleRun {
                    schedule: "night".into(),
                },
            )
            .await;
        assert!(reply.result.unwrap_err().contains("KIOSK_MANAGE_SCHEDULES"));

        let _: Done = ok(
            &fx.control,
            &Caller::Local,
            Command::ScheduleRemove {
                schedule: created.id.clone(),
            },
        )
        .await;
        assert!(units().is_empty());
        assert!(!fx.paths.schedule_runs_dir().join(&created.id).exists());
    }

    #[tokio::test]
    async fn printers_are_kept_with_a_default_and_the_page_needs_printer_enable() {
        let fx = fixture();
        let spec = |name: &str, uri: &str| protocol::PrinterSpec {
            name: name.into(),
            uri: uri.into(),
            kind: protocol::PrinterKind::Raw,
            media: None,
        };

        let created: protocol::PrinterInfo = ok(
            &fx.control,
            &Caller::Local,
            Command::PrinterCreate {
                spec: spec("front", "socket://10.0.0.9:9100"),
            },
        )
        .await;
        // The first printer is the default; not managed here, so CUPS has
        // none of them.
        assert!(created.default);
        assert_eq!(created.state, "missing");
        let _: protocol::PrinterInfo = ok(
            &fx.control,
            &Caller::Local,
            Command::PrinterCreate {
                spec: spec("office", "ipp://10.0.0.5/ipp/print"),
            },
        )
        .await;
        let twice = fx
            .control
            .handle(
                &Caller::Local,
                Command::PrinterCreate {
                    spec: spec("office", "ipp://10.0.0.6/ipp/print"),
                },
            )
            .await;
        assert!(twice.result.unwrap_err().contains("exists already"));

        let _: Done = ok(
            &fx.control,
            &Caller::Local,
            Command::PrinterDefault {
                printer: "office".into(),
            },
        )
        .await;
        let list: protocol::PrinterList =
            ok(&fx.control, &Caller::Local, Command::PrinterList).await;
        assert!(!list.enabled);
        let defaults: Vec<(&str, bool)> = list
            .printers
            .iter()
            .map(|printer| (printer.spec.name.as_str(), printer.default))
            .collect();
        assert_eq!(defaults, [("front", false), ("office", true)]);

        // Removing the default makes the next one it.
        let _: Done = ok(
            &fx.control,
            &Caller::Local,
            Command::PrinterRemove {
                printer: "office".into(),
            },
        )
        .await;
        let list: protocol::PrinterList =
            ok(&fx.control, &Caller::Local, Command::PrinterList).await;
        assert_eq!(list.printers.len(), 1);
        assert!(list.printers[0].default);

        // Printing asks CUPS, which this host does not let it reach.
        let reply = fx
            .control
            .handle(
                &Caller::Local,
                Command::PrinterTest {
                    printer: "front".into(),
                },
            )
            .await;
        assert!(reply.result.unwrap_err().contains("KIOSK_MANAGE_PRINTERS"));
        let reply = fx
            .control
            .handle(
                &Caller::Local,
                Command::PrinterPrint {
                    printer: "front".into(),
                    data: None,
                    path: None,
                    copies: None,
                    media: None,
                    title: None,
                },
            )
            .await;
        assert_eq!(reply.result.unwrap_err(), "nothing to print");

        let policy = || fs::read_to_string(&fx.paths.policy).unwrap();
        // Saved and rendered; restarting the browser needs this host's
        // systemd, which the fixture never reaches.
        let reply = fx
            .control
            .handle(
                &Caller::Local,
                Command::Set {
                    values: [("printer.enable".to_string(), "1".to_string())].into(),
                    if_revision: None,
                    apply: true,
                    verify: Default::default(),
                },
            )
            .await;
        assert!(
            reply.result.unwrap_err().contains("restarting the browser"),
            "printer.enable is the browser's"
        );
        assert!(
            policy().contains("\"PrintingEnabled\": true"),
            "{}",
            policy()
        );
        let list: protocol::PrinterList =
            ok(&fx.control, &Caller::Local, Command::PrinterList).await;
        assert!(list.enabled);
    }

    #[tokio::test]
    async fn a_ca_is_trusted_listed_kept_by_unclaim_and_reset_away() {
        let fx = fixture();
        let holder = claimed(&fx).await;
        let (ca, _) = crate::certs::tests::ca("Corp Root", 30);
        let pem = String::from_utf8(ca.to_pem().unwrap()).unwrap();
        let policy = || fs::read_to_string(&fx.paths.policy).unwrap();

        let reply = fx
            .control
            .handle(&holder, Command::NetCertAdd { pem: pem.clone() })
            .await;
        // The agent's own client picks the roots up when it starts again.
        assert_eq!(
            reply.after,
            Some(After::Restart(fx.paths.agent_unit.clone()))
        );
        let added: protocol::CertsAdded = serde_json::from_value(reply.result.unwrap()).unwrap();
        assert_eq!(added.added.len(), 1);
        assert!(policy().contains("\"CACertificates\""));

        // The same again changes nothing, and restarts nothing.
        let reply = fx
            .control
            .handle(&holder, Command::NetCertAdd { pem: pem.clone() })
            .await;
        assert_eq!(reply.after, None);

        let listed: Vec<protocol::CertInfo> = ok(&fx.control, &holder, Command::NetCertList).await;
        assert_eq!(listed, added.added);

        let _: Done = ok(&fx.control, &holder, Command::Unclaim).await;
        let listed: Vec<protocol::CertInfo> =
            ok(&fx.control, &Caller::Local, Command::NetCertList).await;
        assert_eq!(listed.len(), 1, "an unclaim keeps the configuration");

        let revoked: protocol::CertInfo = ok(
            &fx.control,
            &Caller::Local,
            Command::NetCertRevoke {
                cert: listed[0].fingerprint[..8].to_string(),
            },
        )
        .await;
        assert_eq!(revoked, listed[0]);
        assert!(!policy().contains("\"CACertificates\""));
        let missing = err(
            &fx.control,
            &Caller::Local,
            Command::NetCertRevoke {
                cert: revoked.fingerprint.clone(),
            },
        )
        .await;
        assert!(missing.contains("no certificate matches"), "{missing}");

        let _: protocol::CertsAdded =
            ok(&fx.control, &Caller::Local, Command::NetCertAdd { pem }).await;
        let reply = fx
            .control
            .handle(&Caller::Local, Command::FactoryReset)
            .await;
        assert!(reply.result.is_ok(), "{:?}", reply.result);
        assert!(!fx.paths.ca_certs_dir().exists());
        assert!(!policy().contains("\"CACertificates\""));
    }

    #[tokio::test]
    async fn a_browser_policy_is_merged_guarded_by_revision_and_reset_away() {
        let fx = fixture();
        let holder = claimed(&fx).await;
        let policy = || fs::read_to_string(&fx.paths.policy).unwrap_or_default();
        let set = |text: &str, if_revision: Option<&str>| Command::BrowserPolicySet {
            name: "lockdown".to_string(),
            text: text.to_string(),
            if_revision: if_revision.map(str::to_string),
            position: None,
        };
        let text = "// kiosk\n{\"SpellcheckEnabled\": false,}\n";

        // Stored and rendered; the fixture has no systemd to restart the
        // browser with, which is the only thing that fails.
        let reply = fx.control.handle(&holder, set(text, Some(""))).await;
        let failed = reply.result.unwrap_err();
        assert!(failed.contains("restarting the browser failed"), "{failed}");
        assert!(policy().contains("\"SpellcheckEnabled\": false"));

        // The same text again changes nothing, so restarts nothing.
        let saved: protocol::policy::PolicySaved = ok(&fx.control, &holder, set(text, None)).await;
        assert!(saved.unchanged && !saved.restarted);
        assert_eq!(saved.keys, ["SpellcheckEnabled"]);

        let doc: protocol::policy::PolicyDoc = ok(
            &fx.control,
            &holder,
            Command::BrowserPolicyGet {
                name: "lockdown".to_string(),
            },
        )
        .await;
        assert_eq!(doc.text, text);
        let stale = err(&fx.control, &holder, set("{}", Some("0000"))).await;
        assert!(stale.contains("changed on the device"), "{stale}");
        let again = err(&fx.control, &holder, set("{}", Some(""))).await;
        assert!(again.contains("already exists"), "{again}");
        let managed = err(
            &fx.control,
            &holder,
            set("{\"ProxyMode\": \"direct\"}", None),
        )
        .await;
        assert!(managed.contains("set by the device"), "{managed}");

        let listed: Vec<protocol::policy::PolicyInfo> =
            ok(&fx.control, &holder, Command::BrowserPolicyList).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].revision, doc.revision);
        assert_eq!(listed[0].position, 1);

        // A move that changes nothing is answered without a render; one to
        // a policy that does not exist is refused.
        let moved: protocol::policy::PolicyMoved = ok(
            &fx.control,
            &holder,
            Command::BrowserPolicyMove {
                name: "lockdown".to_string(),
                position: 5,
            },
        )
        .await;
        assert_eq!((moved.position, moved.restarted), (1, false));
        let missing = err(
            &fx.control,
            &holder,
            Command::BrowserPolicyMove {
                name: "nope".to_string(),
                position: 1,
            },
        )
        .await;
        assert!(missing.contains("no browser policy"), "{missing}");

        let effective: Vec<protocol::policy::EffectiveEntry> =
            ok(&fx.control, &holder, Command::BrowserPolicyEffective).await;
        assert!(effective
            .iter()
            .any(|entry| entry.key == "SpellcheckEnabled"
                && entry.source
                    == protocol::policy::PolicySource::Policy {
                        name: "lockdown".to_string()
                    }));

        let _: Done = ok(&fx.control, &holder, Command::Unclaim).await;
        let listed: Vec<protocol::policy::PolicyInfo> =
            ok(&fx.control, &Caller::Local, Command::BrowserPolicyList).await;
        assert_eq!(listed.len(), 1, "an unclaim keeps the configuration");

        let reply = fx
            .control
            .handle(&Caller::Local, Command::FactoryReset)
            .await;
        assert!(reply.result.is_ok(), "{:?}", reply.result);
        let listed: Vec<protocol::policy::PolicyInfo> =
            ok(&fx.control, &Caller::Local, Command::BrowserPolicyList).await;
        assert!(listed.is_empty());
        assert!(!policy().contains("SpellcheckEnabled"));
    }

    #[tokio::test]
    async fn an_ssh_key_is_authorized_once_listed_and_revoked() {
        let fx = fixture();
        let holder = claimed(&fx).await;

        let access: SshAccess = ok(&fx.control, &holder, authorize(SSH_KEY)).await;
        assert_eq!(access.fingerprint, SSH_FINGERPRINT);
        assert!(access.added);
        // No dropbear in the fixture: no host keys, and that is not an error.
        assert!(access.host_keys.is_empty());

        let again: SshAccess = ok(&fx.control, &holder, authorize(SSH_KEY)).await;
        assert!(!again.added);

        let keys: Vec<SshKeyInfo> = ok(&fx.control, &holder, Command::SshKeyList).await;
        assert_eq!(
            keys,
            vec![SshKeyInfo {
                fingerprint: SSH_FINGERPRINT.to_string(),
                kind: "ssh-ed25519".to_string(),
                comment: "a@laptop".to_string(),
            }]
        );

        let done: Done = ok(
            &fx.control,
            &holder,
            Command::SshKeyRevoke {
                key: "a@laptop".into(),
            },
        )
        .await;
        assert!(done.message.contains(SSH_FINGERPRINT));
        let keys: Vec<SshKeyInfo> = ok(&fx.control, &holder, Command::SshKeyList).await;
        assert!(keys.is_empty());
        // Keys are not what makes a device claimed.
        assert!(fx.control.claimed());

        let missing = err(
            &fx.control,
            &holder,
            Command::SshKeyRevoke {
                key: "a@laptop".into(),
            },
        )
        .await;
        assert!(missing.contains("no key matches"), "{missing}");
    }

    #[tokio::test]
    async fn an_ssh_key_needs_a_claimed_device_and_a_bare_key() {
        let fx = fixture();
        let refused = err(&fx.control, &Caller::Local, authorize(SSH_KEY)).await;
        assert!(refused.contains("unclaimed"), "{refused}");
        assert!(!fx.paths.authorized_keys.exists());

        let holder = claimed(&fx).await;
        let refused = err(
            &fx.control,
            &holder,
            authorize(&format!("command=\"/bin/sh\" {SSH_KEY}")),
        )
        .await;
        assert!(refused.contains("options"), "{refused}");
        assert!(ssh::list(&fx.paths.authorized_keys).unwrap().is_empty());
    }

    #[tokio::test]
    async fn revoking_the_last_token_removes_the_ssh_keys() {
        let fx = fixture();
        let holder = claimed(&fx).await;
        let _: SshAccess = ok(&fx.control, &holder, authorize(SSH_KEY)).await;
        let Caller::Token { id, .. } = &holder else {
            unreachable!()
        };

        let _: Done = ok(
            &fx.control,
            &holder,
            Command::TokenRevoke { id: id.clone() },
        )
        .await;

        assert!(!fx.control.claimed());
        assert!(ssh::list(&fx.paths.authorized_keys).unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_password_can_be_changed_but_only_on_a_claimed_device() {
        let fx = fixture();
        let refused = err(
            &fx.control,
            &Caller::Local,
            Command::PasswordSet { password: None },
        )
        .await;
        assert!(refused.contains("unclaimed"), "{refused}");

        let _: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;

        let generated: Password = ok(
            &fx.control,
            &Caller::Local,
            Command::PasswordSet { password: None },
        )
        .await;
        let generated = generated
            .password
            .expect("a generated password is returned");
        let hashed = fs::read_to_string(&fx.paths.shadow)
            .unwrap()
            .split(':')
            .nth(1)
            .unwrap()
            .to_string();
        assert!(shadow::verify(&generated, &hashed));

        let chosen: Password = ok(
            &fx.control,
            &Caller::Local,
            Command::PasswordSet {
                password: Some("my own one".into()),
            },
        )
        .await;
        assert_eq!(chosen.password, None);

        let bad = err(
            &fx.control,
            &Caller::Local,
            Command::PasswordSet {
                password: Some("a:b".into()),
            },
        )
        .await;
        assert!(bad.contains("':'"), "{bad}");
    }

    #[tokio::test]
    async fn factory_reset_clears_settings_and_the_claim() {
        let fx = fixture();
        let _: Claimed = ok(
            &fx.control,
            &anonymous(),
            Command::Claim {
                name: "laptop".into(),
            },
        )
        .await;
        let _: Applied = ok(&fx.control, &Caller::Local, set(&[("agent.debug", "1")])).await;
        let _: SshAccess = ok(&fx.control, &Caller::Local, authorize(SSH_KEY)).await;

        let reply = fx
            .control
            .handle(&Caller::Local, Command::FactoryReset)
            .await;
        assert!(reply.result.is_ok());
        assert_eq!(
            reply.after,
            Some(After::Restart("weston.service".to_string()))
        );

        assert!(!fx.control.claimed());
        assert!(!shadow::root_has_password(&fx.paths.shadow).unwrap());
        assert!(ssh::list(&fx.paths.authorized_keys).unwrap().is_empty());
        let settings: Settings = ok(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("agent.debug".into()),
            },
        )
        .await;
        assert_eq!(settings.settings[0].source, Source::Default);
    }

    #[tokio::test]
    async fn maintenance_mode_restarts_only_the_agent_onto_the_maintenance_page() {
        let fx = fixture();
        // A deployed device: the site's origin in the policy.
        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            Command::Set {
                values: [("browser.url".to_string(), "https://shop.test/".to_string())].into(),
                if_revision: None,
                apply: false,
                verify: Default::default(),
            },
        )
        .await;

        let reply = fx
            .control
            .handle(&Caller::Local, set(&[("browser.maintenance.enable", "on")]))
            .await;
        let applied: Applied = serde_json::from_value(reply.result.unwrap()).unwrap();

        // The agent, not the browser: the grants did not move.
        assert_eq!(applied.restarted, ["tessaro-agent.service"]);
        let env = fs::read_to_string(fx.paths.generated_env()).unwrap();
        assert!(
            env.contains("KIOSK_URL=http://127.0.0.1/maintenance.html\n"),
            "{env}"
        );
        let status: Status = ok(&fx.control, &Caller::Local, Command::Status).await;
        assert!(status.maintenance);
        assert_eq!(status.kiosk_url, "http://127.0.0.1/maintenance.html");

        // A maintenance page that cannot expand is refused at `set`, even
        // while it is not the one on screen.
        let refused = err(
            &fx.control,
            &Caller::Local,
            set(&[
                ("browser.maintenance.enable", "0"),
                (
                    "browser.maintenance.url",
                    "http://127.0.0.1/maintenance.html?m={data.msg}",
                ),
            ]),
        )
        .await;
        assert!(refused.contains("browser.maintenance.url"), "{refused}");
        assert!(refused.contains("data.msg"), "{refused}");
    }

    #[tokio::test]
    async fn an_agent_setting_restarts_the_agent_after_the_reply() {
        let fx = fixture();
        let reply = fx
            .control
            .handle(&Caller::Local, set(&[("agent.probe_interval", "7")]))
            .await;
        let applied: Applied = serde_json::from_value(reply.result.unwrap()).unwrap();

        assert_eq!(applied.changed, ["agent.probe_interval"]);
        assert_eq!(applied.restarted, ["tessaro-agent.service"]);
        assert_eq!(
            reply.after,
            Some(After::Restart("tessaro-agent.service".to_string()))
        );
        let env = fs::read_to_string(fx.paths.generated_env()).unwrap();
        assert!(env.contains("KIOSK_PROBE_INTERVAL=7\n"));

        // The same value again changes nothing and restarts nothing.
        let reply = fx
            .control
            .handle(&Caller::Local, set(&[("agent.probe_interval", "7")]))
            .await;
        let applied: Applied = serde_json::from_value(reply.result.unwrap()).unwrap();
        assert!(applied.changed.is_empty());
        assert_eq!(reply.after, None);
    }

    #[tokio::test]
    async fn a_display_setting_restarts_weston() {
        let fx = fixture();
        let reply = fx
            .control
            .handle(&Caller::Local, set(&[("screen.osk", "never")]))
            .await;
        assert_eq!(
            reply.after,
            Some(After::Restart("weston.service".to_string()))
        );
    }

    #[tokio::test]
    async fn bad_values_and_unknown_keys_change_nothing() {
        let fx = fixture();
        let bad = err(
            &fx.control,
            &Caller::Local,
            set(&[("agent.debug", "1"), ("screen.osk", "maybe")]),
        )
        .await;
        assert!(bad.contains("screen.osk"), "{bad}");
        let unknown = err(&fx.control, &Caller::Local, set(&[("no.such", "1")])).await;
        assert!(unknown.contains("not a setting"), "{unknown}");
        let unknown_get = err(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("no.such".into()),
            },
        )
        .await;
        assert!(unknown_get.contains("not a setting"), "{unknown_get}");

        let settings: Settings = ok(&fx.control, &Caller::Local, Command::Get { key: None }).await;
        assert_eq!(settings.revision, 0);
    }

    #[tokio::test]
    async fn if_revision_refuses_a_stale_write() {
        let fx = fixture();
        let _: Applied = ok(&fx.control, &Caller::Local, set(&[("agent.debug", "1")])).await;

        let stale = err(
            &fx.control,
            &Caller::Local,
            Command::Set {
                values: [("agent.debug".to_string(), "0".to_string())].into(),
                if_revision: Some(0),
                apply: true,
                verify: Default::default(),
            },
        )
        .await;
        assert!(stale.contains("revision 1"), "{stale}");
    }

    #[tokio::test]
    async fn a_pi_firmware_key_does_not_exist_without_the_pi_firmware() {
        let fx = fixture();
        assert!(fx.paths.boot_config_dir.is_none());

        let listed: Vec<KeyInfo> = ok(&fx.control, &Caller::Local, Command::Keys).await;
        assert!(listed.iter().all(|key| key.name != keys::GPU_MEM));
        let all: Settings = ok(&fx.control, &Caller::Local, Command::Get { key: None }).await;
        assert!(all
            .settings
            .iter()
            .all(|setting| setting.key != keys::GPU_MEM));

        for command in [
            set(&[(keys::GPU_MEM, "128")]),
            Command::Get {
                key: Some(keys::GPU_MEM.into()),
            },
        ] {
            let refused = err(&fx.control, &Caller::Local, command).await;
            assert!(
                refused.contains("only available on a Raspberry Pi"),
                "{refused}"
            );
        }

        // Taking a stray value out is always allowed.
        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            Command::Unset {
                keys: vec![keys::GPU_MEM.into()],
                if_revision: None,
                apply: true,
                verify: Default::default(),
            },
        )
        .await;
    }

    #[tokio::test]
    async fn only_an_offered_resolution_is_accepted_and_it_waits_for_confirm() {
        let fx = fixture();

        let refused = err(
            &fx.control,
            &Caller::Local,
            set(&[("screen.resolution", "3840x2160")]),
        )
        .await;
        assert!(refused.contains("1280x720"), "{refused}");

        let applied: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[("screen.resolution", "1280x720")]),
        )
        .await;
        let pending = applied.pending.expect("a guarded change is on probation");
        assert_eq!(pending.value, "1280x720");
        assert_eq!(pending.previous, None);

        let busy = err(
            &fx.control,
            &Caller::Local,
            set(&[("screen.resolution", "1920x1080")]),
        )
        .await;
        assert!(busy.contains("confirm"), "{busy}");

        let _: Done = ok(&fx.control, &Caller::Local, Command::Confirm).await;
        let status: Settings = ok(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("screen.resolution".into()),
            },
        )
        .await;
        assert_eq!(status.settings[0].value.as_deref(), Some("1280x720"));
        let nothing = err(&fx.control, &Caller::Local, Command::Confirm).await;
        assert!(nothing.contains("nothing"), "{nothing}");
    }

    #[tokio::test(start_paused = true)]
    async fn an_unconfirmed_resolution_reverts_on_its_own() {
        let fx = fixture();
        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[("screen.resolution", "1280x720")]),
        )
        .await;

        tokio::time::sleep(Duration::from_secs(protocol::CONFIRM_SECONDS + 1)).await;
        // The revert does its file work on the blocking pool; let it land.
        for _ in 0..50 {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let settings: Settings = ok(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("screen.resolution".into()),
            },
        )
        .await;
        assert_eq!(settings.settings[0].source, Source::Default);
    }

    #[tokio::test]
    async fn custom_values_fill_the_kiosk_url_template() {
        let fx = fixture();

        let missing = err(
            &fx.control,
            &Caller::Local,
            // Same origin as the default, so the policy - and with it a
            // browser restart, which needs a bus - stays out of this test.
            set(&[(
                "browser.url",
                "http://127.0.0.1/?store={data.store}&lang={data.lang}",
            )]),
        )
        .await;
        assert!(missing.contains("data.store, data.lang"), "{missing}");

        let applied: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[
                (
                    "browser.url",
                    "http://127.0.0.1/?store={data.store}&lang={data.lang}",
                ),
                ("data.store", "42"),
                ("data.lang", "sk"),
            ]),
        )
        .await;
        assert_eq!(applied.changed, ["browser.url", "data.lang", "data.store"]);
        let env = fs::read_to_string(fx.paths.generated_env()).unwrap();
        assert!(
            env.contains("KIOSK_URL=http://127.0.0.1/?store=42&lang=sk\n"),
            "{env}"
        );

        // A value still in use cannot be unset; one no longer in use can.
        let in_use = err(
            &fx.control,
            &Caller::Local,
            Command::Unset {
                keys: vec!["data.store".into()],
                if_revision: None,
                apply: true,
                verify: Default::default(),
            },
        )
        .await;
        assert!(in_use.contains("{data.store}"), "{in_use}");

        // The short form is not a placeholder, and the error says so.
        let short = err(
            &fx.control,
            &Caller::Local,
            set(&[("browser.url", "http://127.0.0.1/?store={store}")]),
        )
        .await;
        assert!(short.contains("{data.store}"), "{short}");

        let _: Applied = ok(&fx.control, &Caller::Local, set(&[("data.lang", "en")])).await;
        let env = fs::read_to_string(fx.paths.generated_env()).unwrap();
        assert!(env.contains("lang=en\n"), "{env}");

        let settings: Settings = ok(&fx.control, &Caller::Local, Command::Get { key: None }).await;
        let names: Vec<&str> = settings.settings.iter().map(|s| s.key.as_str()).collect();
        assert!(names.contains(&"data.store") && names.contains(&"data.lang"));

        // The old prefix is not a setting.
        let old = err(&fx.control, &Caller::Local, set(&[("url.lang", "en")])).await;
        assert!(old.contains("not a setting"), "{old}");
    }

    #[tokio::test]
    async fn keys_list_the_custom_values_that_exist_and_whether_they_are_used() {
        let fx = fixture();

        let keys: Vec<KeyInfo> = ok(&fx.control, &Caller::Local, Command::Keys).await;
        let template = keys.iter().find(|k| k.name == "data.<name>").unwrap();
        assert!(
            template.doc.contains("No custom values"),
            "{}",
            template.doc
        );

        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[
                ("browser.url", "http://127.0.0.1/?t={data.table}"),
                ("data.table", "12"),
                ("data.spare", "x"),
            ]),
        )
        .await;

        let keys: Vec<KeyInfo> = ok(&fx.control, &Caller::Local, Command::Keys).await;
        let template = keys.iter().find(|k| k.name == "data.<name>").unwrap();
        assert!(
            template
                .doc
                .contains("Defined on this device: data.spare, data.table"),
            "{}",
            template.doc
        );
        let table = keys.iter().find(|k| k.name == "data.table").unwrap();
        assert_eq!(table.value.as_deref(), Some("12"));
        assert!(
            table.doc.contains("uses it as {data.table}"),
            "{}",
            table.doc
        );
        let spare = keys.iter().find(|k| k.name == "data.spare").unwrap();
        assert!(spare.doc.contains("No template uses it"), "{}", spare.doc);

        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[("browser.debug.template", "spare {data.spare}")]),
        )
        .await;
        let keys: Vec<KeyInfo> = ok(&fx.control, &Caller::Local, Command::Keys).await;
        let spare = keys.iter().find(|k| k.name == "data.spare").unwrap();
        assert!(
            spare
                .doc
                .contains("browser.debug.template uses it as {data.spare}"),
            "{}",
            spare.doc
        );
    }

    #[tokio::test]
    async fn the_debug_template_is_held_to_the_same_placeholder_rules() {
        let fx = fixture();

        let typo = err(
            &fx.control,
            &Caller::Local,
            set(&[("browser.debug.template", "ip {network.ip}\\nt {table}")]),
        )
        .await;
        assert!(
            typo.contains("browser.debug.template") && typo.contains("{data.table}"),
            "{typo}"
        );

        let unset = err(
            &fx.control,
            &Caller::Local,
            set(&[("browser.debug.template", "t {data.table}")]),
        )
        .await;
        assert!(unset.contains("set data.table"), "{unset}");

        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[
                (
                    "browser.debug.template",
                    "{device.name}\\nurl {browser.url}\\nt {data.table}",
                ),
                ("data.table", "12"),
            ]),
        )
        .await;
    }

    #[tokio::test]
    async fn keys_document_themselves() {
        let fx = fixture();
        let _: Applied = ok(&fx.control, &Caller::Local, set(&[("screen.osk", "never")])).await;

        let keys: Vec<KeyInfo> = ok(&fx.control, &Caller::Local, Command::Keys).await;

        let osk = keys.iter().find(|k| k.name == "screen.osk").unwrap();
        assert_eq!(osk.values, "one of: auto, always, never");
        assert_eq!(osk.value.as_deref(), Some("never"));
        let url = keys.iter().find(|k| k.name == "browser.url").unwrap();
        assert_eq!(url.default.as_deref(), Some("http://127.0.0.1/"));
        assert!(keys.iter().any(|k| k.name == "data.<name>"));
    }

    #[tokio::test]
    async fn the_fps_counter_becomes_a_chromium_switch() {
        let fx = fixture();
        let reply = fx
            .control
            .handle(&Caller::Local, set(&[("browser.fps_counter", "on")]))
            .await;
        // No bus in the test, so the browser restart itself fails - after the
        // setting was saved and rendered.
        assert!(reply.result.unwrap_err().contains("restarting the browser"));
        let env = fs::read_to_string(fx.paths.generated_env()).unwrap();
        assert!(
            env.contains("KIOSK_FPS_COUNTER=1\nKIOSK_FPS_ARGS=--show-fps-counter\n"),
            "{env}"
        );
    }

    #[tokio::test]
    async fn an_audio_setting_restarts_nothing_and_is_saved_without_pipewire() {
        let fx = fixture();
        let reply = fx
            .control
            .handle(
                &Caller::Local,
                set(&[("audio.volume", "40"), ("audio.output", "usb")]),
            )
            .await;
        assert_eq!(reply.after, None);
        let applied: Applied = serde_json::from_value(reply.result.unwrap()).unwrap();
        assert_eq!(applied.changed, ["audio.output", "audio.volume"]);
        assert!(applied.restarted.is_empty());
        let audio = applied.audio.unwrap();
        assert!(audio.contains("not applied yet"), "{audio}");
        assert!(audio.contains("PipeWire is not running"), "{audio}");

        let status: AudioStatus = ok(&fx.control, &Caller::Local, Command::AudioStatus).await;
        assert!(!status.running);
        assert_eq!(status.output.setting, "usb");
        assert_eq!(status.output.volume, 40);
        assert_eq!(status.input.setting, "auto");
        assert_eq!(status.input.volume, 100);
    }

    #[tokio::test]
    async fn one_audio_output_by_name_must_exist() {
        let fx = fixture();
        let refused = err(
            &fx.control,
            &Caller::Local,
            set(&[("audio.output", "alsa_output.usb-Speaker-00.analog-stereo")]),
        )
        .await;
        assert!(refused.contains("cannot be checked"), "{refused}");
        let settings: Settings = ok(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("audio.output".to_string()),
            },
        )
        .await;
        assert_eq!(settings.settings[0].source, Source::Default);
    }
}
