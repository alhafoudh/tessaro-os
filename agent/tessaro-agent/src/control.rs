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

use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::keys::{self, Consumer, Key};
use protocol::sshkey::PublicKey;
use protocol::{
    Applied, Claimed, Command, Done, HotspotCredentials, KeyInfo, NodeInfo, Password, Pending,
    Screenshot, Setting, Settings, Source, SshAccess, SshKeyInfo, Status, Target, TokenCreated,
    TokenInfo,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::auth::{self, Auth};
use crate::cdp::session::SessionHandle;
use crate::display;
use crate::log::Log;
use crate::mdns::Mdns;
use crate::nm::profiles::{self, NetConfig};
use crate::nm::Network;
use crate::paths::Paths;
use crate::render;
use crate::secrets::{self, Secrets};
use crate::shadow;
use crate::speedtest;
use crate::ssh;
use crate::state::{self, PendingChange, State};
use crate::store::Store;
use crate::systemd::Bus;
use crate::updates::Updates;
use crate::watchdog::Heartbeat;
use update::manifest::Upload;

/// Any one piece of file work: a store update, a render, a shadow rewrite.
/// Milliseconds normally; past this the disk is the problem.
const BLOCKING: Duration = Duration::from_secs(20);

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
    /// TCP, no token. Only ever reaches `Command::is_public` commands.
    Anonymous { peer: SocketAddr },
}

impl Caller {
    fn describe(&self) -> String {
        match self {
            Caller::Local => "the local socket".to_string(),
            Caller::Token { id, peer } => format!("{peer} (token {id})"),
            Caller::Anonymous { peer } => peer.to_string(),
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
    updates: Arc<Updates>,
    /// Held by the thread running a speed test, for as long as it runs.
    speedtest: Arc<tokio::sync::Mutex<()>>,
    network: Arc<Network>,
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
    ) -> Arc<Self> {
        Arc::new(Self {
            agent_url,
            updates: Updates::new(Arc::clone(&log), paths.clone()),
            network: Network::new(Arc::clone(&log), paths.clone()),
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
            shutdown,
            speedtest: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// Start a speed test; the server streams what it sends. Refused while
    /// another one - even an abandoned one - is still running.
    pub fn speedtest(
        &self,
        caller: &Caller,
        max_size: Option<u64>,
        tests: Option<u32>,
    ) -> Result<tokio::sync::mpsc::Receiver<speedtest::Step>, String> {
        let plan = speedtest::Plan::new(max_size, tests)?;
        let lock = Arc::clone(&self.speedtest)
            .try_lock_owned()
            .map_err(|_| "a speed test is already running on this device".to_string())?;
        self.log
            .info(format!("speed test requested by {}", caller.describe()));
        Ok(speedtest::start(plan, lock, Arc::clone(&self.log)))
    }

    /// Ping a host from the device; the server streams what comes back.
    pub fn net_ping(
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
                Reply::ok(Done {
                    message: "rebooting".to_string(),
                })
                .then(Some(After::Reboot))
            }
            Command::Screenshot => self.screenshot().await.into(),
            Command::Logs { .. } => Reply::err("logs is a stream; the server handles it"),
            Command::Speedtest { .. } => Reply::err("speedtest is a stream; the server handles it"),
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
            Command::UpdateBegin {
                name,
                size,
                sha256,
                bmap,
                verify,
                repartition,
            } => {
                let upload = Upload {
                    name,
                    size,
                    sha256,
                    bmap,
                    verify,
                    repartition,
                };
                let who = caller.describe();
                self.updates.begin(&who, upload).await.into()
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
            Command::Ping => Reply::ok(Done {
                message: "pong".to_string(),
            }),
            Command::NetPing { .. } => Reply::err("net-ping is a stream; the server handles it"),
            Command::NetProfiles => self.network.profiles().await.into(),
            Command::NetShow { profile } => self.network.show(&profile).await.into(),
            Command::NetLast => self.network.last().await.into(),
            Command::Wifi => self.network.wifi().await.into(),
            Command::WifiScan { interface, rescan } => {
                self.network.scan(interface, rescan).await.into()
            }
            Command::WifiJoin {
                ssid,
                psk,
                security,
                hidden,
                verify,
            } => {
                let psk = psk.map(|secret| secret.expose().to_string());
                self.join(caller, ssid, psk, security, hidden, verify).await
            }
            Command::HotspotPassword => {
                let reply: Reply = self.hotspot_password(caller).await.into();
                reply.then(Some(After::Network))
            }
        }
    }

    /// `net wifi join`: client mode on `ssid`, as one change of the network
    /// keys, with the password staged so it is saved only if the join holds.
    async fn join(
        self: &Arc<Self>,
        caller: &Caller,
        ssid: String,
        psk: Option<String>,
        security: Option<protocol::WifiSecurity>,
        hidden: bool,
        verify: protocol::Verify,
    ) -> Reply {
        let key = keys::find("wifi.ssid").expect("wifi.ssid is a key");
        if let Err(err) = keys::validate(key, &ssid) {
            return Reply::err(err);
        }
        if let Some(psk) = &psk {
            if let Err(err) = keys::check_psk(psk) {
                return Reply::err(err);
            }
        }
        let state = match self.read_state().await {
            Ok(state) => state,
            Err(err) => return Reply::err(err),
        };
        let value = profiles::value_of(&state.settings, &self.defaults);
        let interface = match profiles::effective(&value, "wifi.interface").as_str() {
            "auto" => "wlan0".to_string(),
            name => name.to_string(),
        };
        let security = match security {
            Some(security) => security,
            None if hidden => {
                return Reply::err("a hidden network needs --security psk, sae or open")
            }
            None => match self.network.security_of_ssid(&interface, &ssid).await {
                Ok(security) => security,
                Err(err) => return Reply::err(err),
            },
        };
        let word = match security {
            protocol::WifiSecurity::Psk => "psk",
            protocol::WifiSecurity::Sae => "sae",
            protocol::WifiSecurity::Open => "open",
        };
        // Without a new password, only the network whose password is stored
        // can be joined: another one would try it with the wrong one.
        let same_network = profiles::effective(&value, "wifi.ssid") == ssid;
        if psk.is_none() && word != "open" && !same_network {
            return Reply::err(format!("{ssid} needs a password"));
        }

        let changes = BTreeMap::from([
            ("wifi.mode".to_string(), Some("client".to_string())),
            ("wifi.ssid".to_string(), Some(ssid)),
            ("wifi.security".to_string(), Some(word.to_string())),
            (
                "wifi.hidden".to_string(),
                Some(if hidden { "1" } else { "0" }.to_string()),
            ),
        ]);
        self.change(caller, changes, None, true, verify, psk).await
    }

    /// A new random hotspot password, shown once. Unclaimed devices keep an
    /// open hotspot, like an empty root password.
    async fn hotspot_password(&self, caller: &Caller) -> Result<HotspotCredentials, String> {
        let _writes = self.writes.lock().await;
        if !self.claimed() {
            return Err(
                "an unclaimed device's hotspot is open; claiming it sets a password".to_string(),
            );
        }
        let password = secrets::random_hotspot_psk()?;
        let stored = password.clone();
        self.update_secrets(move |secrets| secrets.hotspot_psk = Some(stored))
            .await?;
        self.log.info(format!(
            "network: a new hotspot password was set by {}",
            caller.describe()
        ));
        let config = self.net_config().await?;
        Ok(HotspotCredentials {
            ssid: config.wifi.hotspot_ssid,
            password,
        })
    }

    async fn read_secrets(&self) -> Secrets {
        let store = self.secrets.clone();
        let log = Arc::clone(&self.log);
        blocking("reading secrets.json", move || {
            Ok(store.read::<Secrets>(&log))
        })
        .await
        .unwrap_or_default()
    }

    async fn update_secrets(
        &self,
        change: impl FnOnce(&mut Secrets) + Send + 'static,
    ) -> Result<(), String> {
        let store = self.secrets.clone();
        let log = Arc::clone(&self.log);
        blocking("updating secrets.json", move || {
            store.update(&log, |secrets: &mut Secrets| {
                change(secrets);
                Ok(())
            })
        })
        .await
    }

    /// The node name these settings give, as the hotspot is named after it.
    fn node_name_for(&self, settings: &BTreeMap<String, String>) -> String {
        let value = profiles::value_of(settings, &self.defaults);
        profiles::node_name(&value, &crate::identity::friendly_name(&self.identity.id))
    }

    fn net_config_for(&self, settings: &BTreeMap<String, String>, secrets: &Secrets) -> NetConfig {
        let value = profiles::value_of(settings, &self.defaults);
        NetConfig::from_settings(
            &value,
            secrets.hotspot_psk.clone(),
            secrets.wifi_psk.clone(),
            &self.node_name_for(settings),
        )
    }

    /// The network as `state.json` and `secrets.json` say it is.
    async fn net_config(&self) -> Result<NetConfig, String> {
        let state = self.read_state().await?;
        let secrets = self.read_secrets().await;
        Ok(self.net_config_for(&state.settings, &secrets))
    }

    /// Roll back a network change the previous agent never finished, onto
    /// what the saved settings say.
    pub async fn recover_network(&self) {
        match self.net_config().await {
            Ok(config) => self.network.recover(config),
            Err(err) => self.log.info(format!("network: {err}")),
        }
    }

    /// Re-render the profiles from the saved settings, outside a change:
    /// after a claim, an unclaim or a new hotspot password.
    async fn refresh_network(&self) {
        let config = match self.net_config().await {
            Ok(config) => config,
            Err(err) => {
                self.log.info(format!("network: {err}"));
                return;
            }
        };
        if let Err(err) = self.network.refresh(&config).await {
            self.log
                .info(format!("network: re-rendering the profiles: {err}"));
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

    /// What the device reports now: derived name, node id, addresses, and
    /// the hotspot's name, which follows `node.name`.
    async fn live(&self) -> state::Live {
        let paths = self.paths.clone();
        let mut live = blocking("reading the network", move || Ok(render::live(&paths)))
            .await
            .unwrap_or_default();
        if let Ok(state) = self.read_state().await {
            live.values.insert(
                "wifi.hotspot_ssid".to_string(),
                profiles::hotspot_ssid(&self.node_name_for(&state.settings)),
            );
        }
        live
    }

    async fn net(&self) -> Result<protocol::Net, String> {
        // naked: the lookup's every phase is under its own within()
        self.refresh_public_ip_now().await;
        let paths = self.paths.clone();
        blocking("reading the network", move || {
            Ok(crate::net::snapshot(&paths))
        })
        .await
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

        Ok(Status {
            os,
            image_version,
            node: self.node(),
            revision: state.revision,
            kiosk_url,
            current_url: self.session.current_url(),
            browser_answering: self.session.is_up(),
            units,
            pending: self.pending(&state),
            maintenance: state::maintenance(&state.settings, &self.defaults),
            debug_screen: state::debug_screen(&state.settings, &self.defaults),
        })
    }

    /// A template (`keys::TEMPLATES`, or debug.template) as set, else the
    /// image default.
    fn template(&self, settings: &BTreeMap<String, String>, name: &str) -> String {
        settings
            .get(name)
            .cloned()
            .or_else(|| {
                let key = keys::find(name)?;
                self.defaults.get(key.env).cloned()
            })
            .unwrap_or_default()
    }

    /// The registry, documented: what each key accepts, the image default,
    /// and what this device has set. `data.<name>` appears once as the
    /// template entry, saying which custom values exist, then once per
    /// custom value, saying whether kiosk.url uses it.
    async fn keys(&self) -> Result<Vec<KeyInfo>, String> {
        let state = self.read_state().await?;
        let live = self.live().await;
        let mut out: Vec<KeyInfo> = keys::KEYS
            .iter()
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
            .chain([&"debug.template"])
            .map(|name| (*name, self.template(&state.settings, name)))
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
                format!("No template uses it; add {{{name}}} to kiosk.url or another template to use it.")
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
        if key.as_deref() == Some("net.public_ip") {
            // naked: the lookup's every phase is under its own within()
            self.refresh_public_ip_now().await;
        }
        let state = self.read_state().await?;
        // The registry, then every custom data.* that is set, by its name.
        let wanted: Vec<(String, &Key)> = match &key {
            Some(name) => vec![(name.clone(), keys::find(name).ok_or_else(|| unknown(name))?)],
            None => keys::KEYS
                .iter()
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

    // --- changing ----------------------------------------------------------

    async fn change(
        self: &Arc<Self>,
        caller: &Caller,
        changes: BTreeMap<String, Option<String>>,
        if_revision: Option<u64>,
        apply: bool,
        verify: protocol::Verify,
        wifi_psk: Option<String>,
    ) -> Reply {
        if changes.is_empty() {
            return Reply::err("nothing to change");
        }

        // Validate everything before touching anything. Keyed by the name as
        // given, not the registry entry's: every data.* shares one entry.
        let mut normalized: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut guarded: Vec<&'static Key> = Vec::new();
        for (name, value) in changes {
            let Some(key) = keys::find(&name) else {
                return Reply::err(unknown(&name));
            };
            let value = match value {
                Some(value) => match keys::validate(key, &value) {
                    Ok(value) => Some(value),
                    Err(err) => return Reply::err(err),
                },
                None => None,
            };
            if let (keys::Kind::Resolution, Some(mode)) = (key.kind, &value) {
                if let Err(err) = self.check_mode(mode).await {
                    return Reply::err(err);
                }
            }
            if key.guarded {
                if !apply {
                    return Reply::err(format!(
                        "{} is applied on probation and cannot be set without applying",
                        key.name
                    ));
                }
                guarded.push(key);
            }
            normalized.insert(name, value);
        }

        let network = wifi_psk.is_some()
            || normalized.keys().any(|name| {
                keys::find(name).is_some_and(|key| key.consumers.contains(&Consumer::Network))
            });
        if network && !apply {
            return Reply::err(
                "network settings are applied and checked at once; drop --no-apply".to_string(),
            );
        }

        let _writes = self.writes.lock().await;

        let edit = Edit {
            normalized,
            if_revision,
            guarded: guarded.iter().map(|key| key.name).collect(),
            default_templates: keys::TEMPLATES
                .iter()
                .map(|name| (*name, self.template(&BTreeMap::new(), name)))
                .collect(),
            default_debug: self.template(&BTreeMap::new(), "debug.template"),
            defaults: self.defaults.clone(),
        };

        let (committed, network_change) = if network {
            match self.change_network(caller, &edit, verify, wifi_psk).await {
                Ok(outcome) => outcome,
                Err(err) => return Reply::err(err),
            }
        } else {
            let store = self.state.clone();
            let log = Arc::clone(&self.log);
            let edit = edit.clone();
            let committed = blocking("updating state.json", move || {
                store.update(&log, |state: &mut State| edit.apply(state))
            })
            .await;
            match committed {
                Ok(outcome) => (outcome, None),
                Err(err) => return Reply::err(err),
            }
        };
        let (before, after) = committed;

        let changed = changed_keys(&before, &after.settings);
        if changed.is_empty() {
            return Reply::ok(Applied {
                revision: after.revision,
                changed: Vec::new(),
                restarted: Vec::new(),
                pending: self.pending(&after),
                network: network_change,
            });
        }

        self.log.info(format!(
            "settings revision {}: {} changed by {}",
            after.revision,
            changed
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            caller.describe()
        ));

        if after.pending.is_some() && !guarded.is_empty() {
            self.arm_probation();
        }

        self.converge(&changed, &after, apply, network_change).await
    }

    /// A change that touches the network: tried as one transaction the
    /// device verifies, and saved - `state.json`, and a staged WiFi password
    /// to `secrets.json` - only once it has held. A change that did not hold
    /// is an error, and nothing is saved. The saving is done by the
    /// transaction itself, so it happens even if this caller is gone.
    async fn change_network(
        &self,
        caller: &Caller,
        edit: &Edit,
        verify: protocol::Verify,
        wifi_psk: Option<String>,
    ) -> Result<
        (
            (BTreeMap<String, String>, State),
            Option<protocol::NetChange>,
        ),
        String,
    > {
        if self.updates.is_pending() {
            return Err(
                "an update is committed and waiting for its reboot; change the network after it"
                    .to_string(),
            );
        }
        let current = self.read_state().await?;
        let mut next = current.clone();
        let (before, _) = edit.apply(&mut next)?;
        if next.settings == before && wifi_psk.is_none() {
            return Ok(((before, next), None));
        }

        let secrets = self.read_secrets().await;
        let staged = Secrets {
            wifi_psk: wifi_psk.clone().or_else(|| secrets.wifi_psk.clone()),
            ..secrets.clone()
        };
        let value = profiles::value_of(&next.settings, &self.defaults);
        keys::check_network(
            |name| profiles::effective(&value, name),
            staged.wifi_psk.is_some(),
        )?;
        let old = self.net_config_for(&current.settings, &secrets);
        let new = self.net_config_for(&next.settings, &staged);

        let action = match &wifi_psk {
            Some(_) => format!(
                "join {}",
                new.wifi
                    .client
                    .as_ref()
                    .map(|c| c.ssid.as_str())
                    .unwrap_or("")
            ),
            None => format!(
                "set {}",
                changed_keys(&before, &next.settings)
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        };

        // What the transaction runs once the change has held.
        let slot: Arc<Mutex<Option<Committed>>> = Arc::default();
        let commit: crate::nm::txn::Commit = {
            let store = self.state.clone();
            let secrets_store = self.secrets.clone();
            let log = Arc::clone(&self.log);
            let edit = edit.clone();
            let slot = Arc::clone(&slot);
            Box::pin(async move {
                let committed = blocking("saving the network settings", move || {
                    if let Some(psk) = wifi_psk {
                        secrets_store.update(&log, |secrets: &mut Secrets| {
                            secrets.wifi_psk = Some(psk);
                            Ok(())
                        })?;
                    }
                    store.update(&log, |state: &mut State| edit.apply(state))
                })
                .await?;
                *lock(&slot) = Some(committed);
                Ok(())
            })
        };

        // naked: Network bounds every NetworkManager call with within()
        let change = self
            .network
            .apply(caller.describe(), action, old, new, verify, commit)
            .await?;
        if change.outcome == protocol::ChangeOutcome::RolledBack {
            return Err(format!(
                "the network change was rolled back: {}",
                change.reason.as_deref().unwrap_or("it did not hold")
            ));
        }
        let committed = lock(&slot).take().ok_or_else(|| {
            "the network change held, but its settings were not saved".to_string()
        })?;
        Ok((committed, Some(change)))
    }

    /// kiosk.url as these settings, and the device as it is now, expand it.
    async fn expanded_url(&self, settings: &BTreeMap<String, String>) -> String {
        let live = self.live().await;
        let effective = state::Effective::new(&self.defaults, settings, &self.log).with_live(live);
        crate::config::Env::get(&effective, "KIOSK_URL").unwrap_or_default()
    }

    /// A kiosk.url that uses a read-only key - `{net.ip}` - can move with no
    /// `set` at all: DHCP renews, the link changes, and at boot the render
    /// ran before there was any address. So while the template uses one,
    /// this checks every 15s and, when the URL no longer matches the one the
    /// agent is driving, re-renders and restarts the agent onto it (and the
    /// browser, if the origin - and with it the policy - moved).
    pub fn watch_url(self: &Arc<Self>) {
        const EVERY: Duration = Duration::from_secs(15);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            loop {
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(EVERY) => {}
                    _ = shutdown.changed() => return,
                }
                // naked: check_url's every wait is blocking()/Bus, under within()
                control.check_url().await;
            }
        });
    }

    /// Keeps `net.public_ip` current while kiosk.url uses it - or the debug
    /// screen is on and its template does - and only then: a link may be
    /// metered, so a device that shows no `{net.public_ip}` never asks.
    /// While one does, Cloudflare's trace is
    /// asked every 5 minutes (every 30s until the first answer, and after a
    /// failure), and the answer goes to `/run/tessaro-kiosk/public-ip`, which
    /// is all the read-only key ever reads. A failure keeps the last address
    /// rather than emptying it, so one lost request never moves the URL;
    /// `watch_url` notices when it does change. The template is checked every
    /// 15s, so a `set` that starts using the key is answered within that.
    pub fn watch_public_ip(self: &Arc<Self>) {
        const TICK: Duration = Duration::from_secs(15);
        const EVERY: Duration = Duration::from_secs(300);
        const RETRY: Duration = Duration::from_secs(30);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            let http = crate::http::HyperHttp::new(5, 5, 4096, Heartbeat::detached());
            // When the next request is due; `None` asks at once.
            let mut due: Option<Instant> = None;
            loop {
                // naked: a disk read under blocking()'s within()
                let wanted = control.url_uses("net.public_ip").await;
                if !wanted {
                    // Asked afresh the moment the URL uses it again.
                    due = None;
                } else if due.is_none_or(|at| Instant::now() >= at) {
                    // naked: public_ip's every phase is under its own within()
                    let wait = match control.refresh_public_ip(&http).await {
                        Ok(()) => EVERY,
                        Err(()) => RETRY,
                    };
                    due = Some(Instant::now() + wait);
                }
                // naked: a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = tokio::time::sleep(TICK) => {}
                    _ = shutdown.changed() => return,
                }
            }
        });
    }

    /// Does the template on screen - kiosk.url, or maintenance.url in
    /// maintenance mode, as set, else the image default - name this key as a
    /// placeholder? While the debug screen is up, its template counts too.
    async fn url_uses(&self, key: &str) -> bool {
        let Ok(state) = self.read_state().await else {
            return false;
        };
        let (_, template) = state::shown_template(&state.settings, &self.defaults);
        keys::placeholders(&template).contains(&key)
            || (state::debug_screen(&state.settings, &self.defaults)
                && keys::placeholders(&self.template(&state.settings, "debug.template"))
                    .contains(&key))
    }

    /// One lookup, saved on success. A failure is logged at debug and leaves
    /// the last address in place.
    async fn refresh_public_ip(&self, http: &crate::http::HyperHttp) -> Result<(), ()> {
        // naked: public_ip's every phase is under its own within()
        match crate::net::public_ip(http).await {
            Ok(ip) => {
                // naked: a file write under blocking()'s within()
                self.store_public_ip(ip.to_string()).await;
                Ok(())
            }
            Err(err) => {
                self.log
                    .debug(format!("public address: {} {err}", crate::net::TRACE_URL));
                Err(())
            }
        }
    }

    /// Someone asked for the public address outright - `net`, or
    /// `get net.public_ip` - so look it up now, whatever kiosk.url uses.
    /// One request per ask; at worst the command waits out the 5s budgets.
    async fn refresh_public_ip_now(&self) {
        let http = crate::http::HyperHttp::new(5, 5, 4096, Heartbeat::detached());
        // naked: public_ip's every phase is under its own within()
        let _ = self.refresh_public_ip(&http).await;
    }

    async fn store_public_ip(&self, ip: String) {
        let paths = self.paths.clone();
        let body = format!("{ip}\n");
        let written = blocking("writing the public address", move || {
            let file = paths.public_ip_file();
            crate::store::replace_if_changed(&file, body.as_bytes(), 0o644)
                .map_err(|err| format!("{}: {err}", file.display()))
        })
        .await;
        match written {
            Ok(true) => self.log.info(format!("public address is {ip}")),
            Ok(false) => {}
            Err(err) => self.log.info(format!("public address: {err}")),
        }
    }

    async fn check_url(&self) {
        let Ok(state) = self.read_state().await else {
            return;
        };
        let (name, template) = state::shown_template(&state.settings, &self.defaults);
        if !state::Live::moves(&template) {
            return;
        }

        let url = self.expanded_url(&state.settings).await;
        if url == self.agent_url {
            return;
        }

        let _writes = self.writes.lock().await;
        self.log.info(format!(
            "{name} now expands to {url} (the agent is on {}); applying",
            self.agent_url
        ));
        let reply = self.converge(&[], &state, true, None).await;
        if let Err(err) = &reply.result {
            self.log.info(format!("applying the new {name}: {err}"));
        }
        if let Some(after) = reply.after {
            self.run_after(after).await;
        }
    }

    /// Render, then restart what reads the changed keys. The reply is built
    /// here so every path that changes settings reports it the same way.
    async fn converge(
        &self,
        changed: &[Changed],
        state: &State,
        apply: bool,
        network: Option<protocol::NetChange>,
    ) -> Reply {
        let rendered = match self.render(&state.settings).await {
            Ok(rendered) => rendered,
            Err(err) => {
                return Reply::err(format!(
                    "saved as revision {}, but rendering failed: {err}",
                    state.revision
                ))
            }
        };

        let reads = |consumer: Consumer| {
            changed
                .iter()
                .any(|(_, key)| key.consumers.contains(&consumer))
        };
        // kiosk.url can be built from any setting, so a change to one of
        // them can move the URL without touching a key the agent reads. The
        // test is whether the URL this agent started with is still the one.
        let url_moved = self.expanded_url(&state.settings).await != self.agent_url;

        let weston = reads(Consumer::Weston);
        let browser = !weston && (reads(Consumer::Browser) || rendered.policy_changed);
        let agent = !weston && (reads(Consumer::Agent) || url_moved);

        let mut restarted = Vec::new();
        let mut after = None;
        if apply {
            if browser {
                if let Err(err) = self.bus.restart(&self.paths.kiosk_unit).await {
                    return Reply::err(format!(
                        "saved as revision {}, but restarting the browser failed: {err}",
                        state.revision
                    ));
                }
                restarted.push(self.paths.kiosk_unit.clone());
            }
            if weston {
                restarted.push(self.paths.weston_unit.clone());
                after = Some(After::Restart(self.paths.weston_unit.clone()));
            } else if agent {
                restarted.push(self.paths.agent_unit.clone());
                after = Some(After::Restart(self.paths.agent_unit.clone()));
            }
        }

        Reply::ok(Applied {
            revision: state.revision,
            changed: changed.iter().map(|(name, _)| name.clone()).collect(),
            restarted,
            pending: self.pending(state),
            network,
        })
        .then(after)
    }

    async fn render(
        &self,
        settings: &BTreeMap<String, String>,
    ) -> Result<render::Rendered, String> {
        let paths = self.paths.clone();
        let defaults = self.defaults.clone();
        let settings = settings.clone();
        let log = Arc::clone(&self.log);
        blocking("rendering", move || {
            render::all(&paths, &defaults, &settings, &log)
        })
        .await
    }

    async fn check_mode(&self, mode: &str) -> Result<(), String> {
        if mode == "preferred" {
            return Ok(());
        }
        let connectors = self.modes().await?;
        if connectors.is_empty() {
            return Err(format!(
                "no connected display reports its modes, so {mode} cannot be checked; refusing"
            ));
        }
        if display::offered(&connectors, mode) {
            return Ok(());
        }
        let offered: Vec<String> = connectors
            .iter()
            .map(|c| format!("{}: {}", c.name, c.modes.join(" ")))
            .collect();
        Err(format!(
            "no connected display offers {mode}; see `tessaro-ctl modes` ({})",
            offered.join("; ")
        ))
    }

    // --- probation ---------------------------------------------------------

    /// Start the confirm window. Called when a guarded change is made, and at
    /// startup when one is pending - which is the usual case, because the
    /// change restarted Weston and this agent with it.
    pub fn arm_probation(self: &Arc<Self>) {
        let deadline = Instant::now() + Duration::from_secs(protocol::CONFIRM_SECONDS);
        *lock(&self.probation) = Some(deadline);

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => {}
                _ = shutdown.changed() => return,
            }
            control.expire_probation(deadline).await;
        });
    }

    pub async fn arm_if_pending(self: &Arc<Self>) {
        if let Ok(state) = self.read_state().await {
            if let Some(pending) = &state.pending {
                self.log.info(format!(
                    "{}={} is on probation: `tessaro-ctl confirm` within {}s or it reverts",
                    pending.key,
                    pending.value,
                    protocol::CONFIRM_SECONDS
                ));
                self.arm_probation();
            }
        }
    }

    async fn expire_probation(&self, deadline: Instant) {
        if *lock(&self.probation) != Some(deadline) {
            return; // confirmed, or re-armed since
        }

        let _writes = self.writes.lock().await;
        let store = self.state.clone();
        let log = Arc::clone(&self.log);
        let reverted = blocking("reverting a change", move || {
            store.update(&log, |state: &mut State| {
                Ok((state.revert_pending(), state.clone()))
            })
        })
        .await;

        let (pending, state) = match reverted {
            Ok((Some(pending), state)) => (pending, state),
            Ok((None, _)) => return,
            Err(err) => {
                self.log
                    .info(format!("could not revert an unconfirmed change: {err}"));
                return;
            }
        };
        *lock(&self.probation) = None;

        self.log.info(format!(
            "{}={} was not confirmed within {}s; back to {}",
            pending.key,
            pending.value,
            protocol::CONFIRM_SECONDS,
            pending.previous.as_deref().unwrap_or("the default")
        ));

        let changed: Vec<Changed> = keys::find(&pending.key)
            .map(|key| (pending.key.clone(), key))
            .into_iter()
            .collect();
        let reply = self.converge(&changed, &state, true, None).await;
        if let Err(err) = &reply.result {
            self.log.info(format!("reverting: {err}"));
        }
        if let Some(after) = reply.after {
            self.run_after(after).await;
        }
    }

    async fn confirm(&self) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let store = self.state.clone();
        let log = Arc::clone(&self.log);
        let kept = blocking("confirming", move || {
            store.update(&log, |state: &mut State| {
                state
                    .pending
                    .take()
                    .ok_or_else(|| "nothing is waiting to be confirmed".to_string())
            })
        })
        .await?;

        *lock(&self.probation) = None;
        self.log
            .info(format!("{}={} confirmed", kept.key, kept.value));
        Ok(Done {
            message: format!("kept {}={}", kept.key, kept.value),
        })
    }

    // --- the browser -------------------------------------------------------

    async fn navigate(&self, url: &str) -> Result<Done, String> {
        let url = keys::validate(keys::find("kiosk.url").expect("kiosk.url is a key"), url)?;
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
            _ => Ok(Done {
                message: format!("navigated to {url}"),
            }),
        }
    }

    async fn screenshot(&self) -> Result<Screenshot, String> {
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

    async fn restart(&self, what: Target) -> Reply {
        match what {
            Target::Browser => match self.bus.restart(&self.paths.kiosk_unit).await {
                Ok(()) => Reply::ok(Done {
                    message: format!("restarted {}", self.paths.kiosk_unit),
                }),
                Err(err) => Reply::err(err.to_string()),
            },
            Target::Weston => Reply::ok(Done {
                message: format!("restarting {}", self.paths.weston_unit),
            })
            .then(Some(After::Restart(self.paths.weston_unit.clone()))),
            Target::Agent => Reply::ok(Done {
                message: format!("restarting {}", self.paths.agent_unit),
            })
            .then(Some(After::Restart(self.paths.agent_unit.clone()))),
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

    // --- claim, tokens, password ------------------------------------------

    async fn claim(&self, caller: &Caller, name: &str) -> Result<Claimed, String> {
        if matches!(caller, Caller::Token { .. }) {
            return Err("this device is already claimed".to_string());
        }
        let name = auth::check_name(name)?;

        let _writes = self.writes.lock().await;
        if self.claimed() {
            return Err("this device is already claimed".to_string());
        }

        // The password first, then the token: a power cut in between leaves
        // no token and a password, which the boot oneshot puts back to empty.
        // The other order would leave a claimed device with an empty root.
        let password = auth::random_password()?;
        self.set_root(Some(password.clone())).await?;

        let store = self.auth_store.clone();
        let log = Arc::clone(&self.log);
        let issued = blocking("updating auth.json", move || {
            store.update(&log, |auth: &mut Auth| {
                if auth.claimed() {
                    return Err("this device is already claimed".to_string());
                }
                let (entry, secret) = auth.issue(&name, "claim")?;
                Ok((auth.clone(), entry, secret))
            })
        })
        .await;

        let (auth, entry, secret) = match issued {
            Ok(issued) => issued,
            Err(err) => {
                // Put the invariant back: unclaimed means an empty password.
                let _ = self.set_root(None).await;
                return Err(err);
            }
        };

        *lock(&self.auth) = auth;
        self.announce_claimed(true);
        self.log.info(format!(
            "claimed by {} as {:?} (token {}); root password set",
            caller.describe(),
            entry.name,
            entry.id
        ));

        // The hotspot's password last, after the claim that decides it: a
        // power cut before it leaves a claimed device with an open hotspot
        // until `net wifi hotspot-password`, never an unclaimed one with a
        // password nobody was shown. Applied once the answer is out.
        let hotspot = match self.set_hotspot_psk().await {
            Ok(hotspot) => hotspot,
            Err(err) => {
                self.log
                    .info(format!("claim: the hotspot keeps no password: {err}"));
                None
            }
        };

        Ok(Claimed {
            token_id: entry.id,
            token: secret,
            root_password: password,
            hotspot,
        })
    }

    /// A new hotspot password, stored; the credentials to show, when the
    /// device has its WiFi interface at all. The profiles are re-rendered
    /// afterwards, by `After::Network`.
    async fn set_hotspot_psk(&self) -> Result<Option<HotspotCredentials>, String> {
        let password = secrets::random_hotspot_psk()?;
        let stored = password.clone();
        self.update_secrets(move |secrets| secrets.hotspot_psk = Some(stored))
            .await?;
        let config = self.net_config().await?;
        let here = self.network.has_wifi(&config.wifi.interface).await;
        Ok(here.then_some(HotspotCredentials {
            ssid: config.wifi.hotspot_ssid,
            password,
        }))
    }

    async fn token_create(&self, caller: &Caller, name: &str) -> Result<TokenCreated, String> {
        let name = auth::check_name(name)?;
        let issuer = match caller {
            Caller::Local => "local".to_string(),
            Caller::Token { id, .. } => id.clone(),
            Caller::Anonymous { .. } => {
                return Err("a token is needed to issue a token".to_string())
            }
        };

        let _writes = self.writes.lock().await;
        if !self.claimed() {
            return Err("this device is unclaimed; `tessaro-ctl claim` it first".to_string());
        }

        let store = self.auth_store.clone();
        let log = Arc::clone(&self.log);
        let issuer_for_store = issuer.clone();
        let (auth, entry, secret) = blocking("updating auth.json", move || {
            store.update(&log, |auth: &mut Auth| {
                let (entry, secret) = auth.issue(&name, &issuer_for_store)?;
                Ok((auth.clone(), entry, secret))
            })
        })
        .await?;

        *lock(&self.auth) = auth;
        self.log.info(format!(
            "token {} ({:?}) issued by {}",
            entry.id,
            entry.name,
            caller.describe()
        ));
        Ok(TokenCreated {
            id: entry.id,
            token: secret,
        })
    }

    fn token_list(&self) -> Vec<TokenInfo> {
        lock(&self.auth)
            .tokens
            .iter()
            .map(|entry| TokenInfo {
                id: entry.id.clone(),
                name: entry.name.clone(),
                issued_by: entry.issued_by.clone(),
            })
            .collect()
    }

    async fn token_revoke(&self, caller: &Caller, id: &str) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let store = self.auth_store.clone();
        let log = Arc::clone(&self.log);
        let wanted = id.to_string();
        let (auth, entry) = blocking("updating auth.json", move || {
            store.update(&log, |auth: &mut Auth| {
                let entry = auth
                    .revoke(&wanted)
                    .ok_or_else(|| format!("no token {wanted}"))?;
                Ok((auth.clone(), entry))
            })
        })
        .await?;

        let now_unclaimed = !auth.claimed();
        *lock(&self.auth) = auth;
        self.log.info(format!(
            "token {} ({:?}) revoked by {}",
            entry.id,
            entry.name,
            caller.describe()
        ));

        if now_unclaimed {
            let path = self.paths.authorized_keys.clone();
            blocking("emptying authorized_keys", move || {
                ssh::clear(&path).map_err(|err| format!("{}: {err}", path.display()))
            })
            .await?;
            self.set_root(None).await?;
            self.update_secrets(|secrets| secrets.hotspot_psk = None)
                .await?;
            self.announce_claimed(false);
            self.log.info(
                "the last token was revoked: unclaimed, ssh keys removed, root password emptied, hotspot open",
            );
            return Ok(Done {
                message: format!(
                    "revoked {}; that was the last token, the device is unclaimed",
                    entry.id
                ),
            });
        }
        Ok(Done {
            message: format!("revoked {}", entry.id),
        })
    }

    async fn password_set(
        &self,
        caller: &Caller,
        password: Option<String>,
    ) -> Result<Password, String> {
        let _writes = self.writes.lock().await;
        if !self.claimed() {
            return Err(
                "this device is unclaimed, and an unclaimed device keeps an empty root password; claim it first"
                    .to_string(),
            );
        }

        let (password, generated) = match password {
            Some(password) => {
                protocol::check_password(&password)?;
                (password, false)
            }
            None => (auth::random_password()?, true),
        };
        self.set_root(Some(password.clone())).await?;
        self.log
            .info(format!("root password changed by {}", caller.describe()));

        Ok(Password {
            password: generated.then_some(password),
        })
    }

    async fn unclaim(&self, caller: &Caller) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        self.drop_claim().await?;
        self.log.info(format!(
            "unclaimed by {}: every token and ssh key removed, root password emptied",
            caller.describe()
        ));
        Ok(Done {
            message: "unclaimed: every token and ssh key is gone and the root password is empty"
                .to_string(),
        })
    }

    /// Tokens first, then SSH keys, then the password - the order that a
    /// power cut in between leaves healable: the boot oneshot empties both
    /// credentials on a device with no tokens (see `claim`).
    async fn drop_claim(&self) -> Result<(), String> {
        let store = self.auth_store.clone();
        let log = Arc::clone(&self.log);
        let auth = blocking("updating auth.json", move || {
            store.update(&log, |auth: &mut Auth| {
                auth.tokens.clear();
                Ok(auth.clone())
            })
        })
        .await?;
        *lock(&self.auth) = auth;
        self.announce_claimed(false);
        let path = self.paths.authorized_keys.clone();
        blocking("emptying authorized_keys", move || {
            ssh::clear(&path).map_err(|err| format!("{}: {err}", path.display()))
        })
        .await?;
        self.set_root(None).await?;
        // Unclaimed means an open hotspot, like an empty root password. The
        // profiles follow once the answer is out (`After::Network`).
        self.update_secrets(|secrets| secrets.hotspot_psk = None)
            .await
    }

    async fn ssh_authorize(&self, caller: &Caller, key: &str) -> Result<SshAccess, String> {
        let _writes = self.writes.lock().await;
        if !self.claimed() {
            return Err(
                "this device is unclaimed, and an unclaimed device has no credentials; claim it first"
                    .to_string(),
            );
        }
        let key = PublicKey::parse(key)?;
        let fingerprint = key.fingerprint();

        let path = self.paths.authorized_keys.clone();
        let dirs = self.paths.ssh_host_key_dirs.clone();
        let (added, generated, host_keys) = blocking("updating authorized_keys", move || {
            let added =
                ssh::add(&path, &key).map_err(|err| format!("{}: {err}", path.display()))?;
            let generated = ssh::ensure_host_key(&dirs);
            Ok((added, generated, ssh::host_keys(&dirs)))
        })
        .await?;
        if let Err(err) = generated {
            self.log
                .info(format!("no ssh host key, and making one failed: {err}"));
        }

        if added {
            self.log.info(format!(
                "ssh key {fingerprint} authorized by {}",
                caller.describe()
            ));
        }
        if host_keys.is_empty() {
            self.log
                .info("no ssh host key could be read; the client will ask about it");
        }
        Ok(SshAccess {
            fingerprint,
            added,
            host_keys,
        })
    }

    async fn ssh_key_list(&self) -> Result<Vec<SshKeyInfo>, String> {
        let path = self.paths.authorized_keys.clone();
        let keys = blocking("reading authorized_keys", move || {
            ssh::list(&path).map_err(|err| format!("{}: {err}", path.display()))
        })
        .await?;
        Ok(keys
            .into_iter()
            .map(|key| SshKeyInfo {
                fingerprint: key.fingerprint(),
                kind: key.kind,
                comment: key.comment,
            })
            .collect())
    }

    async fn ssh_key_revoke(&self, caller: &Caller, query: &str) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let path = self.paths.authorized_keys.clone();
        let query = query.to_string();
        let key = blocking("updating authorized_keys", move || {
            ssh::remove(&path, &query)
        })
        .await?;
        let fingerprint = key.fingerprint();
        self.log.info(format!(
            "ssh key {fingerprint} ({:?}) revoked by {}",
            key.comment,
            caller.describe()
        ));
        Ok(Done {
            message: format!("revoked {fingerprint}"),
        })
    }

    async fn factory_reset(&self, caller: &Caller) -> Reply {
        let _writes = self.writes.lock().await;
        if let Err(err) = self.drop_claim().await {
            return Reply::err(err);
        }

        let store = self.state.clone();
        if let Err(err) = blocking("removing state.json", move || {
            store.remove().map_err(|err| err.to_string())
        })
        .await
        {
            return Reply::err(err);
        }
        *lock(&self.probation) = None;
        let secrets = self.secrets.clone();
        if let Err(err) = blocking("removing secrets.json", move || {
            secrets.remove().map_err(|err| err.to_string())
        })
        .await
        {
            return Reply::err(err);
        }

        if let Err(err) = self.render(&BTreeMap::new()).await {
            return Reply::err(format!("reset, but rendering failed: {err}"));
        }
        // The profiles now say the defaults: DHCP, an open hotspot. What is
        // up keeps running until the next boot brings them up afresh.
        self.refresh_network().await;

        self.log.info(format!(
            "factory reset by {}: settings, tokens, ssh keys, passwords and the network cleared",
            caller.describe()
        ));
        // Weston takes the browser and the agent with it (PartOf=), so every
        // consumer comes back up on the defaults.
        Reply::ok(Done {
            message: "factory reset: defaults restored, unclaimed, restarting the display; \
                      the network is DHCP and an open hotspot from the next boot"
                .to_string(),
        })
        .then(Some(After::Restart(self.paths.weston_unit.clone())))
    }

    async fn set_root(&self, password: Option<String>) -> Result<(), String> {
        let path = self.paths.shadow.clone();
        blocking("updating /etc/shadow", move || {
            let hashed = match password {
                Some(password) => Some(shadow::hash(&password).map_err(|err| err.to_string())?),
                None => None,
            };
            shadow::set_root(&path, hashed.as_deref())
                .map_err(|err| format!("{}: {err}", path.display()))
        })
        .await
    }

    fn announce_claimed(&self, claimed: bool) {
        if let Some(mdns) = lock(&self.mdns).as_ref() {
            mdns.set_claimed(claimed, &self.log);
        }
    }

    /// `journalctl` for the `logs` stream.
    pub fn journal(
        &self,
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
}

/// A setting that changed: its name as stored, and the registry entry that
/// says what reads it.
type Changed = (String, &'static Key);

/// What a committed edit leaves: the settings before, and the state after.
type Committed = (BTreeMap<String, String>, State);

/// Every stored name whose value differs, in name order.
fn changed_keys(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<Changed> {
    let names: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    names
        .into_iter()
        .filter(|name| before.get(*name) != after.get(*name))
        .filter_map(|name| keys::find(name).map(|key| (name.clone(), key)))
        .collect()
}

/// One `set`/`unset`, validated, as it is applied to `state.json`: once as a
/// dry run to know what a network change would become, then for real - by
/// the store, or by the network transaction once the change has held.
#[derive(Debug, Clone)]
struct Edit {
    normalized: BTreeMap<String, Option<String>>,
    if_revision: Option<u64>,
    /// The guarded keys among them (`display.resolution`).
    guarded: Vec<&'static str>,
    default_templates: Vec<(&'static str, String)>,
    default_debug: String,
    defaults: HashMap<String, String>,
}

impl Edit {
    /// Apply to `state`, returning the settings before and the state after.
    /// An edit that changes nothing leaves the revision where it was.
    fn apply(&self, state: &mut State) -> Result<(BTreeMap<String, String>, State), String> {
        if let Some(expected) = self.if_revision {
            if state.revision != expected {
                return Err(format!(
                    "the settings are at revision {}, not {expected}; someone else changed them",
                    state.revision
                ));
            }
        }
        if !self.guarded.is_empty() {
            if let Some(pending) = &state.pending {
                return Err(format!(
                    "{}={} is waiting for `tessaro-ctl confirm`; confirm it or let it revert first",
                    pending.key, pending.value
                ));
            }
        }

        let before = state.settings.clone();
        for (name, value) in &self.normalized {
            match value {
                Some(value) => state.settings.insert(name.clone(), value.clone()),
                None => state.settings.remove(name),
            };
        }
        if state.settings == before {
            return Ok((before, state.clone()));
        }

        // Every {placeholder} a template uses must have a value - checked on
        // the result, so setting a template and its values in one command
        // works, and unsetting a value still in use does not. Every template,
        // whichever mode the device is in: a maintenance page or a debug
        // screen that cannot expand is found at `set`, not when someone
        // needs it. Read-only keys always have a value, so no live values
        // are needed to know what is missing.
        for (key, default) in &self.default_templates {
            let template = state.settings.get(*key).unwrap_or(default);
            let (_, missing) = state::expand_url(
                template,
                &state.settings,
                &self.defaults,
                &state::Live::default(),
            );
            check_template(key, template, &missing)?;
        }

        let template = state
            .settings
            .get("debug.template")
            .unwrap_or(&self.default_debug);
        let (_, missing) = state::expand_text(
            template,
            &state.settings,
            &self.defaults,
            &state::Live::default(),
        );
        check_template("debug.template", template, &missing)?;

        state.revision += 1;
        for name in &self.guarded {
            if before.get(*name) != state.settings.get(*name) {
                state.pending = Some(PendingChange {
                    key: name.to_string(),
                    value: state
                        .settings
                        .get(*name)
                        .cloned()
                        .unwrap_or_else(|| "preferred".to_string()),
                    previous: before.get(*name).cloned(),
                });
            }
        }
        Ok((before, state.clone()))
    }
}

/// Why a template (`keys::TEMPLATES`, debug.template) cannot be saved with these
/// placeholders missing, if it cannot.
fn check_template(key: &str, template: &str, missing: &[String]) -> Result<(), String> {
    // A custom data.* nobody set yet just needs a value; anything else is not
    // a setting at all.
    let (unset, typos): (Vec<&String>, Vec<&String>) = missing
        .iter()
        .partition(|name| keys::param_name(name).is_some());
    if let Some(typo) = typos.first() {
        // The likeliest slip: {table} for {data.table}.
        let hint = if keys::is_param(typo) {
            format!(
                "; a custom value is written in full: {{{}{typo}}}",
                keys::DATA_PREFIX
            )
        } else {
            String::new()
        };
        return Err(format!(
            "{key} {template} uses {{{typo}}}, which is not a setting \
             (and no template can contain a template){hint}; `tessaro-ctl keys` lists them"
        ));
    }
    if !unset.is_empty() {
        return Err(format!(
            "{key} {template} uses {}; set {} (it can go in the same command)",
            unset
                .iter()
                .map(|name| format!("{{{name}}}"))
                .collect::<Vec<_>>()
                .join(", "),
            unset
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(())
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
    format!("{name} is not a setting; `tessaro-ctl keys` lists them")
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Blocking file work, off the runtime thread and under a deadline.
pub async fn blocking<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    match crate::deadline::within(what, BLOCKING, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(join)) => Err(format!("{what}: {join}")),
        Err(expired) => Err(expired.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::cdp::session::{self, SessionConfig};

    struct Fixture {
        _dir: tempfile::TempDir,
        control: Arc<Control>,
        paths: Paths,
        _stop: watch::Sender<bool>,
    }

    fn peer() -> SocketAddr {
        "192.0.2.10:50000".parse().unwrap()
    }

    fn anonymous() -> Caller {
        Caller::Anonymous { peer: peer() }
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let at = |name: &str| dir.path().join(name).display().to_string();
        let env: HashMap<String, String> = [
            ("KIOSK_STATE_DIR", at("data")),
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
        protocol::keys::check_psk(&psk).unwrap();
        // No wlan0 in this sandbox: nothing to show, but it is stored for a
        // WiFi dongle plugged in later.
        assert_eq!(claimed.hotspot, None);

        let rotated: HotspotCredentials =
            ok(&fx.control, &Caller::Local, Command::HotspotPassword).await;
        assert_ne!(rotated.password, psk);
        assert!(rotated.ssid.starts_with("tessaro-"));
        assert_eq!(secrets_of(&fx).hotspot_psk, Some(rotated.password));

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
                values: [("ethernet.mode".to_string(), "static".to_string())].into(),
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
            set(&[("ethernet.mode", "static")]),
        )
        .await;
        assert!(
            incomplete.contains("needs ethernet.address"),
            "{incomplete}"
        );
        let client = err(&fx.control, &Caller::Local, set(&[("wifi.mode", "client")])).await;
        assert!(client.contains("wifi.ssid"), "{client}");

        let settings: Settings = ok(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("ethernet.mode".into()),
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
            "wifi.hotspot_ssid is reported"
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
                values: [("kiosk.url".to_string(), "https://shop.test/".to_string())].into(),
                if_revision: None,
                apply: false,
                verify: Default::default(),
            },
        )
        .await;

        let reply = fx
            .control
            .handle(&Caller::Local, set(&[("maintenance.enable", "on")]))
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
                ("maintenance.enable", "0"),
                (
                    "maintenance.url",
                    "http://127.0.0.1/maintenance.html?m={data.msg}",
                ),
            ]),
        )
        .await;
        assert!(refused.contains("maintenance.url"), "{refused}");
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
            .handle(&Caller::Local, set(&[("display.osk", "never")]))
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
            set(&[("agent.debug", "1"), ("display.osk", "maybe")]),
        )
        .await;
        assert!(bad.contains("display.osk"), "{bad}");
        let unknown = err(&fx.control, &Caller::Local, set(&[("no.such", "1")])).await;
        assert!(unknown.contains("not a setting"), "{unknown}");

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
    async fn only_an_offered_resolution_is_accepted_and_it_waits_for_confirm() {
        let fx = fixture();

        let refused = err(
            &fx.control,
            &Caller::Local,
            set(&[("display.resolution", "3840x2160")]),
        )
        .await;
        assert!(refused.contains("1280x720"), "{refused}");

        let applied: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[("display.resolution", "1280x720")]),
        )
        .await;
        let pending = applied.pending.expect("a guarded change is on probation");
        assert_eq!(pending.value, "1280x720");
        assert_eq!(pending.previous, None);

        let busy = err(
            &fx.control,
            &Caller::Local,
            set(&[("display.resolution", "1920x1080")]),
        )
        .await;
        assert!(busy.contains("confirm"), "{busy}");

        let _: Done = ok(&fx.control, &Caller::Local, Command::Confirm).await;
        let status: Settings = ok(
            &fx.control,
            &Caller::Local,
            Command::Get {
                key: Some("display.resolution".into()),
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
            set(&[("display.resolution", "1280x720")]),
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
                key: Some("display.resolution".into()),
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
                "kiosk.url",
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
                    "kiosk.url",
                    "http://127.0.0.1/?store={data.store}&lang={data.lang}",
                ),
                ("data.store", "42"),
                ("data.lang", "sk"),
            ]),
        )
        .await;
        assert_eq!(applied.changed, ["data.lang", "data.store", "kiosk.url"]);
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
            set(&[("kiosk.url", "http://127.0.0.1/?store={store}")]),
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
                ("kiosk.url", "http://127.0.0.1/?t={data.table}"),
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
            set(&[("debug.template", "spare {data.spare}")]),
        )
        .await;
        let keys: Vec<KeyInfo> = ok(&fx.control, &Caller::Local, Command::Keys).await;
        let spare = keys.iter().find(|k| k.name == "data.spare").unwrap();
        assert!(
            spare.doc.contains("debug.template uses it as {data.spare}"),
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
            set(&[("debug.template", "ip {net.ip}\\nt {table}")]),
        )
        .await;
        assert!(
            typo.contains("debug.template") && typo.contains("{data.table}"),
            "{typo}"
        );

        let unset = err(
            &fx.control,
            &Caller::Local,
            set(&[("debug.template", "t {data.table}")]),
        )
        .await;
        assert!(unset.contains("set data.table"), "{unset}");

        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[
                (
                    "debug.template",
                    "{node.name}\\nurl {kiosk.url}\\nt {data.table}",
                ),
                ("data.table", "12"),
            ]),
        )
        .await;
    }

    #[tokio::test]
    async fn keys_document_themselves() {
        let fx = fixture();
        let _: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[("display.osk", "never")]),
        )
        .await;

        let keys: Vec<KeyInfo> = ok(&fx.control, &Caller::Local, Command::Keys).await;

        let osk = keys.iter().find(|k| k.name == "display.osk").unwrap();
        assert_eq!(osk.values, "one of: auto, always, never");
        assert_eq!(osk.value.as_deref(), Some("never"));
        let url = keys.iter().find(|k| k.name == "kiosk.url").unwrap();
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
}
