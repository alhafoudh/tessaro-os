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
use protocol::{
    Applied, Claimed, Command, Done, KeyInfo, NodeInfo, Password, Pending, Screenshot, Setting,
    Settings, Source, Status, Target, TokenCreated, TokenInfo,
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
use crate::paths::Paths;
use crate::render;
use crate::shadow;
use crate::state::{self, PendingChange, State};
use crate::store::Store;
use crate::systemd::Bus;
use crate::watchdog::Heartbeat;

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
    /// For `{node.name}` when no name is set - the same derivation the
    /// renderer and `main` use, so all three always agree.
    derived_name: Option<String>,
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
            derived_name: render::derived_name(&paths),
            agent_url,
            state: Store::new(&paths.state_dir, state::FILE),
            auth_store: Store::new(&paths.state_dir, auth::FILE),
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
        })
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
            Command::Get { key } => self.get(key).await.into(),
            Command::Set {
                values,
                if_revision,
                apply,
            } => {
                let changes = values.into_iter().map(|(k, v)| (k, Some(v))).collect();
                self.change(caller, changes, if_revision, apply).await
            }
            Command::Unset {
                keys,
                if_revision,
                apply,
            } => {
                let changes = keys.into_iter().map(|k| (k, None)).collect();
                self.change(caller, changes, if_revision, apply).await
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
            Command::Claim { name } => self.claim(caller, &name).await.into(),
            Command::TokenCreate { name } => self.token_create(caller, &name).await.into(),
            Command::TokenList => Reply::ok(self.token_list()),
            Command::TokenRevoke { id } => self.token_revoke(caller, &id).await.into(),
            Command::PasswordSet { password } => self.password_set(caller, password).await.into(),
            Command::Unclaim => self.unclaim(caller).await.into(),
            Command::FactoryReset => self.factory_reset(caller).await,
        }
    }

    // --- reading -----------------------------------------------------------

    async fn read_state(&self) -> Result<State, String> {
        let store = self.state.clone();
        let log = Arc::clone(&self.log);
        blocking("reading state.json", move || Ok(store.read::<State>(&log))).await
    }

    async fn status(&self) -> Result<Status, String> {
        let state = self.read_state().await?;
        let kiosk_url = self.expanded_url(&state.settings);

        let mut units = BTreeMap::new();
        for unit in [
            &self.paths.weston_unit,
            &self.paths.kiosk_unit,
            &self.paths.agent_unit,
        ] {
            units.insert(unit.clone(), self.bus.active_state(unit).await);
        }

        Ok(Status {
            node: self.node(),
            revision: state.revision,
            kiosk_url,
            current_url: self.session.current_url(),
            browser_answering: self.session.is_up(),
            units,
            pending: self.pending(&state),
        })
    }

    /// The registry, documented: what each key accepts, the image default,
    /// and what this device has set. `url.<name>` appears once as the
    /// template entry, then once per parameter that is set.
    async fn keys(&self) -> Result<Vec<KeyInfo>, String> {
        let state = self.read_state().await?;
        let mut out: Vec<KeyInfo> = keys::KEYS
            .iter()
            .map(|key| KeyInfo {
                default: self.defaults.get(key.env).cloned(),
                value: state.settings.get(key.name).cloned(),
                ..KeyInfo::from(key)
            })
            .collect();

        out.push(KeyInfo::from(&keys::URL_PARAM));
        for (name, value) in &state.settings {
            if keys::param_name(name).is_some() {
                out.push(KeyInfo {
                    name: name.clone(),
                    value: Some(value.clone()),
                    ..KeyInfo::from(&keys::URL_PARAM)
                });
            }
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
        let state = self.read_state().await?;
        // The registry, then every url.* that is set, under its own name.
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
                        .map(|name| (name.clone(), &keys::URL_PARAM)),
                )
                .collect(),
        };

        let settings = wanted
            .into_iter()
            .map(|(name, key)| match state.settings.get(&name) {
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
    ) -> Reply {
        if changes.is_empty() {
            return Reply::err("nothing to change");
        }

        // Validate everything before touching anything. Keyed by the name as
        // given, not the registry entry's: every url.* shares one entry.
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

        let _writes = self.writes.lock().await;

        let store = self.state.clone();
        let log = Arc::clone(&self.log);
        let guarded_names: Vec<&'static str> = guarded.iter().map(|key| key.name).collect();
        let default_url = self.defaults.get("KIOSK_URL").cloned().unwrap_or_default();
        let defaults = self.defaults.clone();
        let committed = blocking("updating state.json", move || {
            store.update(&log, |state: &mut State| {
                if let Some(expected) = if_revision {
                    if state.revision != expected {
                        return Err(format!(
                            "the settings are at revision {}, not {expected}; someone else changed them",
                            state.revision
                        ));
                    }
                }
                if !guarded_names.is_empty() {
                    if let Some(pending) = &state.pending {
                        return Err(format!(
                            "{}={} is waiting for `tessaro-ctl confirm`; confirm it or let it revert first",
                            pending.key, pending.value
                        ));
                    }
                }

                let before = state.settings.clone();
                for (name, value) in &normalized {
                    match value {
                        Some(value) => state.settings.insert(name.clone(), value.clone()),
                        None => state.settings.remove(name),
                    };
                }
                if state.settings == before {
                    return Ok((before, state.clone()));
                }

                // Every {placeholder} the kiosk URL uses must have a value -
                // checked on the result, so setting a template and its values
                // in one command works, and unsetting a value still in use
                // does not.
                let template = state.settings.get("kiosk.url").unwrap_or(&default_url);
                let (_, missing) = state::expand_url(template, &state.settings, &defaults, None);
                // A dotted name that is not a setting is a typo; a plain one
                // is a url.* nobody set yet.
                let (typos, unset): (Vec<&String>, Vec<&String>) =
                    missing.iter().partition(|name| name.contains('.'));
                if let Some(typo) = typos.first() {
                    return Err(format!(
                        "kiosk.url {template} uses {{{typo}}}, which is not a setting \
                         (and kiosk.url cannot contain itself); `tessaro-ctl keys` lists them"
                    ));
                }
                if !unset.is_empty() {
                    let needed: Vec<String> =
                        unset.iter().map(|name| format!("url.{name}")).collect();
                    return Err(format!(
                        "kiosk.url {template} uses {}; set {} (it can go in the same command)",
                        unset
                            .iter()
                            .map(|name| format!("{{{name}}}"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        needed.join(", ")
                    ));
                }

                state.revision += 1;
                for name in &guarded_names {
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
            })
        })
        .await;

        let (before, after) = match committed {
            Ok(outcome) => outcome,
            Err(err) => return Reply::err(err),
        };

        let changed = changed_keys(&before, &after.settings);
        if changed.is_empty() {
            return Reply::ok(Applied {
                revision: after.revision,
                changed: Vec::new(),
                restarted: Vec::new(),
                pending: self.pending(&after),
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

        self.converge(&changed, &after, apply).await
    }

    /// kiosk.url as these settings expand it.
    fn expanded_url(&self, settings: &BTreeMap<String, String>) -> String {
        let effective = state::Effective::new(&self.defaults, settings, &self.log)
            .with_derived_name(self.derived_name.clone());
        crate::config::Env::get(&effective, "KIOSK_URL").unwrap_or_default()
    }

    /// Render, then restart what reads the changed keys. The reply is built
    /// here so every path that changes settings reports it the same way.
    async fn converge(&self, changed: &[Changed], state: &State, apply: bool) -> Reply {
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
        let url_moved = self.expanded_url(&state.settings) != self.agent_url;

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
        let reply = self.converge(&changed, &state, true).await;
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

        Ok(Claimed {
            token_id: entry.id,
            token: secret,
            root_password: password,
        })
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
            self.set_root(None).await?;
            self.announce_claimed(false);
            self.log
                .info("the last token was revoked: unclaimed, root password emptied");
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
            "unclaimed by {}: every token removed, root password emptied",
            caller.describe()
        ));
        Ok(Done {
            message: "unclaimed: every token is gone and the root password is empty".to_string(),
        })
    }

    /// Tokens first, then the password - the order that a power cut in
    /// between leaves healable (see `claim`).
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
        self.set_root(None).await
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

        if let Err(err) = self.render(&BTreeMap::new()).await {
            return Reply::err(format!("reset, but rendering failed: {err}"));
        }

        self.log.info(format!(
            "factory reset by {}: settings, tokens and root password cleared",
            caller.describe()
        ));
        // Weston takes the browser and the agent with it (PartOf=), so every
        // consumer comes back up on the defaults.
        Reply::ok(Done {
            message: "factory reset: defaults restored, unclaimed, restarting the display"
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
            ("KIOSK_DRM", at("drm")),
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
        let defaults: HashMap<String, String> =
            [("KIOSK_URL".to_string(), "http://127.0.0.1/".to_string())].into();
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

        let _: Done = ok(&fx.control, &holder, Command::Unclaim).await;

        assert!(!fx.control.claimed());
        assert!(fx.control.verify(&claimed.token).is_none());
        assert!(!shadow::root_has_password(&fx.paths.shadow).unwrap());
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
    async fn url_parameters_fill_the_kiosk_url_template() {
        let fx = fixture();

        let missing = err(
            &fx.control,
            &Caller::Local,
            // Same origin as the default, so the policy - and with it a
            // browser restart, which needs a bus - stays out of this test.
            set(&[("kiosk.url", "http://127.0.0.1/?store={store}&lang={lang}")]),
        )
        .await;
        assert!(missing.contains("url.store, url.lang"), "{missing}");

        let applied: Applied = ok(
            &fx.control,
            &Caller::Local,
            set(&[
                ("kiosk.url", "http://127.0.0.1/?store={store}&lang={lang}"),
                ("url.store", "42"),
                ("url.lang", "sk"),
            ]),
        )
        .await;
        assert_eq!(applied.changed, ["kiosk.url", "url.lang", "url.store"]);
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
                keys: vec!["url.store".into()],
                if_revision: None,
                apply: true,
            },
        )
        .await;
        assert!(in_use.contains("{store}"), "{in_use}");

        let _: Applied = ok(&fx.control, &Caller::Local, set(&[("url.lang", "en")])).await;
        let env = fs::read_to_string(fx.paths.generated_env()).unwrap();
        assert!(env.contains("lang=en\n"), "{env}");

        let settings: Settings = ok(&fx.control, &Caller::Local, Command::Get { key: None }).await;
        let names: Vec<&str> = settings.settings.iter().map(|s| s.key.as_str()).collect();
        assert!(names.contains(&"url.store") && names.contains(&"url.lang"));
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
        assert!(keys.iter().any(|k| k.name == "url.<name>"));
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
