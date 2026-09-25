//! What every `tessaro-ctl` command does, whichever transport it came in on.
//!
//! The state machine in `agent.rs` never sees any of this. A change to the
//! configuration is: validate, commit to `state.json` under the store's lock,
//! re-render `generated.env` and the policy, then restart exactly what reads
//! the keys that changed - the browser, Weston, or the agent itself. The
//! agent restarting itself is how an agent setting takes effect: it costs
//! nothing on screen, and it keeps the state machine a single sequential task
//! with one `Config` for its whole life.
//!
//! Anything that would take this process down with it - restarting the agent
//! or Weston (the agent is `PartOf=` it), a reboot - is returned as an
//! `After` and run by the server once the reply is on the wire.
//!
//! All file I/O is blocking and goes through `blocking()`: `spawn_blocking`
//! under a deadline, so the one runtime thread never waits on a disk.
//!
//! This module has the types, `handle` and the commands that only read;
//! the rest is split by what it acts on, each an `impl Control` of its own:
//! `settings` (changing them, and probation), `access` (the claim, tokens,
//! passwords, SSH keys), `network` (WiFi and the profiles' inputs),
//! `watchers` (what is kept true with nobody asking), `page` (the page on
//! screen: reload, eval, the keyboard), `screen` (its power) and `bridge`
//! (`window.tessaro` and the injected script).

mod access;
mod bridge;
mod network;
mod page;
mod screen;
mod settings;
mod watchers;

pub use bridge::BridgeSetup;

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::keys::{self, Key};
use protocol::{
    Command, Done, KeyInfo, NodeInfo, Pending, RestartTarget, Screenshot, Setting, Settings,
    Source, Status,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::audio::Audio;
use crate::auth::{self, Auth};
use crate::cdp::session::SessionHandle;
use crate::deadline::blocking;
use crate::display;
use crate::files::Files;
use crate::log::Log;
use crate::mdns::Mdns;
use crate::nm::profiles;
use crate::nm::Network;
use crate::paths::Paths;
use crate::render;
use crate::secrets;
use crate::speedtest;
use crate::state::{self, State};
use crate::storage;
use crate::store::Store;
use crate::sync::lock;
use crate::systemd::Bus;
use crate::time::Time;
use crate::updates::Updates;
use crate::watchdog::Heartbeat;

/// One DevTools command from the control plane.
const CDP_LIMIT: Duration = Duration::from_secs(10);

/// A screenshot of a 4K page can take a while to encode.
const SCREENSHOT_LIMIT: Duration = Duration::from_secs(30);

/// Who is asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// The unix socket: root on the device.
    Local,
    /// TCP, with a valid token.
    Token { id: String, peer: SocketAddr },
    /// TCP, no valid token: the `Command::is_public` commands, and every
    /// command while the device is unclaimed.
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
            Caller::Anonymous { peer } => peer.to_string(),
            Caller::Page => "the page".to_string(),
        }
    }
}

/// Work that must wait until the reply has been sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum After {
    Restart(String),
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
    state: Store,
    auth_store: Store,
    /// The hotspot's and the WiFi client's passwords, never in `state.json`.
    secrets: Store,
    /// What `auth.json` holds, kept in memory so verifying a token is not a
    /// disk read. Only ever replaced after a successful write.
    auth: Mutex<Auth>,
    session: SessionHandle,
    bus: Bus,
    identity: Identity,
    mdns: Mutex<Option<Mdns>>,
    /// Serialises every change, so two clients cannot interleave a commit
    /// with a render. The store's flock does the same across processes.
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
    /// Set once, by `start_bridge`.
    bridge: std::sync::OnceLock<Arc<bridge::Bridge>>,
}

impl Control {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        log: Arc<Log>,
        paths: Paths,
        defaults: HashMap<String, String>,
        auth: Auth,
        session: SessionHandle,
        bus: Bus,
        identity: Identity,
        shutdown: watch::Receiver<bool>,
        agent_url: String,
        proxy: Option<SocketAddr>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_url,
            proxy,
            updates: Updates::new(Arc::clone(&log), paths.clone()),
            files: Files::new(Arc::clone(&log), paths.clone()),
            network: Network::new(Arc::clone(&log), paths.clone()),
            audio: Audio::new(Arc::clone(&log), &paths),
            time: Time::new(Arc::clone(&log), &paths),
            state: Store::new(&paths.state_dir, state::FILE),
            auth_store: Store::new(&paths.state_dir, auth::FILE),
            secrets: Store::new(&paths.state_dir, secrets::FILE),
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
            shutdown,
            speedtest: Arc::new(tokio::sync::Mutex::new(())),
            storage_grow: Arc::new(tokio::sync::Mutex::new(())),
            bridge: std::sync::OnceLock::new(),
        })
    }

    /// A command answered with events (`Command::is_stream`), validated and
    /// started; the server sends what it produces.
    pub fn stream(&self, caller: &Caller, command: Command) -> Result<Stream, String> {
        match command {
            Command::Logs {
                follow,
                unit,
                lines,
            } => Ok(Stream::Journal {
                command: journal(follow, unit.as_deref(), lines)?,
                follow,
            }),
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
                let changes = values.into_iter().map(|(k, v)| (k, Some(v))).collect();
                self.change(caller, changes, if_revision, apply, verify, None)
                    .await
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
            // `Command::is_stream`: the server starts them through `stream`.
            Command::Logs { .. }
            | Command::Speedtest { .. }
            | Command::NetPing { .. }
            | Command::StorageGrow { .. } => Reply::err("a stream is not answered with one reply"),
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
        }
    }

    /// Resume whatever update the staging directory holds.
    pub async fn load_update(&self) {
        self.updates.load().await;
    }

    // --- reading -----------------------------------------------------------

    async fn read_state(&self) -> Result<State, String> {
        let store = self.state.clone();
        let log = Arc::clone(&self.log);
        blocking("reading state.json", move || Ok(store.read::<State>(&log))).await
    }

    /// Change `state.json` under the store's lock, off the runtime thread.
    async fn update_state<R: Send + 'static>(
        &self,
        what: &'static str,
        change: impl FnOnce(&mut State) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        let store = self.state.clone();
        let log = Arc::clone(&self.log);
        blocking(what, move || store.update(&log, change)).await
    }

    /// Change `auth.json` the same way. The copy in memory that tokens are
    /// verified against follows, and only once the write has succeeded.
    async fn update_auth<R: Send + 'static>(
        &self,
        what: &'static str,
        change: impl FnOnce(&mut Auth) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        let store = self.auth_store.clone();
        let log = Arc::clone(&self.log);
        let (auth, out) = blocking(what, move || {
            store.update(&log, |auth: &mut Auth| {
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
            After::Restart(unit) => self.bus.restart(unit).await,
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

/// A command that answers with events, started. Each kind of step stream
/// keeps its own type; the server turns every step into one `event` frame.
pub enum Stream {
    /// `journalctl`, with the end of a non-following one bounded by the
    /// server.
    Journal {
        command: tokio::process::Command,
        follow: bool,
    },
    Speedtest(tokio::sync::mpsc::Receiver<speedtest::Step>),
    Grow(tokio::sync::mpsc::Receiver<storage::Step>),
    Ping {
        steps: tokio::sync::mpsc::Receiver<crate::ping::Step>,
        /// The longest the whole run can take, from the plan.
        total: Duration,
    },
}

/// `journalctl` for the `logs` stream.
fn journal(
    follow: bool,
    unit: Option<&str>,
    lines: Option<u32>,
) -> Result<tokio::process::Command, String> {
    let mut command = tokio::process::Command::new("journalctl");
    command
        .args(["--output=json", "--no-pager", "--quiet"])
        .arg(format!("--lines={}", lines.unwrap_or(100).min(100_000)));
    if follow {
        command.arg("--follow");
    }
    if let Some(unit) = unit {
        let valid = !unit.is_empty()
            && unit
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "@._:-".contains(ch));
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

/// The refusal of anything but `id` and `claim` on an unclaimed device.
pub const UNCLAIMED: &str = "this device is unclaimed; `tessaro-ctl access claim` it first";

/// `restarting UNIT`, with the restart itself left until the answer is out:
/// the agent goes down with either unit it is asked to restart.
fn restart_after(unit: &str) -> Reply {
    Reply::ok(Done::new(format!("restarting {unit}"))).then(Some(After::Restart(unit.to_string())))
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
        paths: Paths,
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

        let control = Control::new(
            log,
            paths.clone(),
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
        let path = fx.paths.state_dir.join(secrets::FILE);
        fs::read_to_string(path)
            .map(|text| serde_json::from_str(&text).unwrap())
            .unwrap_or_default()
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
        fs::create_dir_all(&fx.paths.state_dir).unwrap();
        fs::write(
            fx.paths.state_dir.join(secrets::FILE),
            r#"{"hotspot_psk":"hotspotsecret1","wifi_psk":"clientsecret22"}"#,
        )
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
        let auth = fs::read_to_string(fx.paths.state_dir.join(auth::FILE)).unwrap();
        assert!(!auth.contains(&claimed.token_id));

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
        // An old name is refused too, saying what it is called now.
        let old = err(
            &fx.control,
            &Caller::Local,
            set(&[("display.osk", "never")]),
        )
        .await;
        assert_eq!(old, "display.osk is now screen.osk");
        let old_get = err(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("kiosk.url".into()),
            },
        )
        .await;
        assert_eq!(old_get, "kiosk.url is now browser.url");

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
