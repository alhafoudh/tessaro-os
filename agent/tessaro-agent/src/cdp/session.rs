//! One persistent DevTools session, owned by a driver task.
//!
//! This replaced one-websocket-per-command. The reasons are what the old model
//! could not do: listen. Knowing the page's URL now costs no round trip at all
//! (`Page.frameNavigated` keeps it current), a new session is a free and
//! bus-independent signal that the browser restarted, and TODO item 5's
//! `DeviceAccess.deviceRequestPrompted` needs a listener in place *before*
//! the page asks, which is why the session is established at browser start
//! rather than lazily. Screenshots (item 2, item 8) become one command on it
//! instead of a connect and handshake per frame.
//!
//! What it had to not lose: the old `alive()` opened a fresh connection every
//! time, so a half-open socket could never make it hang. Here every command
//! is under a deadline, so a hang still costs one failed check in the same
//! time it always did, and a websocket ping every `KIOSK_CDP_PING` seconds
//! tears down a socket that has gone quiet - two pings unanswered and it is
//! gone - without waiting for a cycle to notice. Counted, not timed, so the
//! agent's own stalls are never blamed on the browser.
//!
//! The driver never touches the state machine's state. It reconnects on its
//! own, and bumps `generation` only when the page it lands on is new - a
//! different target, or the same one after it crashed; `Agent::cycle` decides
//! what that means. A reconnect to the same page is not news. And
//! everything it logs is debug: a browser restart is already reported, once,
//! by the agent, and the boot race - Chromium opening its DevTools port a few
//! seconds after systemd calls it started - must not put a reconnect line in
//! every device's journal.
//!
//! Every deadline here is plain `deadline::within`, never the heartbeat's:
//! this task reconnects in a loop of its own, and if it could renew the
//! watchdog pledge it would keep systemd happy while the state machine was
//! stuck.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use super::protocol::{self, Incoming};
use super::targets;
use crate::deadline;
use crate::http::HyperHttp;
use crate::log::Log;
use crate::watchdog::Heartbeat;

/// The reason a session is down before the driver has tried at all.
const NOT_CONNECTED_YET: &str = "not connected yet";

/// A session that lasted at least this long earns an immediate reconnect; a
/// shorter one backs off, so a browser that accepts, primes and drops cannot
/// become a hot loop.
const STABLE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub base_url: String,
    /// One command, and each step of connecting.
    pub timeout: Duration,
    pub ping: Duration,
    pub reconnect_max: Duration,
    pub device_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub up: bool,
    /// Bumped when a session comes up on a page not seen before: a new
    /// target, or the same one after a crash. Still reported while down; it
    /// only ever grows, and a change is what the agent watches for.
    pub generation: u64,
    /// Why the session is down, for the error a caller gets meanwhile.
    pub reason: String,
}

/// The name `Runtime.addBinding` gives the page's one way to the agent. The
/// bridge preamble takes it into a closure and deletes it before any page
/// script runs.
pub const BINDING: &str = "__tessaroBridge";

/// The player page's way to the agent (docs/playlists.md): what it plays,
/// its heartbeat, and input in the frames it shows. The page keeps it; a
/// frame's copy is taken by `PageScripts::frame_sources` before the frame's
/// own scripts run.
pub const PLAYER_BINDING: &str = "__tessaroPlayerEvent";

/// What the page gets besides itself: the bridge binding and the scripts run
/// in every new document. Registered on every session, since Chromium drops
/// them with the connection that made them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageScripts {
    /// `Runtime.enable` and the `BINDING`.
    pub binding: bool,
    /// Sources for `Page.addScriptToEvaluateOnNewDocument`, in order.
    pub sources: Vec<String>,
    /// The player is on screen: `Runtime.enable`, the `PLAYER_BINDING`, and
    /// every iframe in a process of its own attached as a child session
    /// (`Target.setAutoAttach` with `flatten`), which gets the binding and
    /// `frame_sources` before it runs.
    pub player: bool,
    /// What each child session runs in every new document.
    pub frame_sources: Vec<String>,
    /// Evaluated in the page after the binding is added on a session: hands a
    /// page that already ran the preamble the new binding, so a reconnect to
    /// the same page needs no reload.
    pub rebind: Option<String>,
    /// Bumped to reload the page, so a changed script runs now.
    pub reload: u64,
}

/// A call from the page through the `BINDING`, with where it came from as
/// the browser reports it - never as the page says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingCall {
    /// Which binding was called: `BINDING` or `PLAYER_BINDING`.
    pub binding: &'static str,
    /// The child session of the iframe it came from, which `context` belongs
    /// to; `None` for the page's own.
    pub session: Option<String>,
    pub context: i64,
    /// The calling context's origin; empty when the browser has not said.
    pub origin: String,
    /// The main frame's own page world: not an iframe, not an isolated world.
    pub top: bool,
    /// A document's own page world, the main frame's or an iframe's: not an
    /// isolated world.
    pub frame: bool,
    pub payload: String,
}

/// The session's side of the page bridge.
pub struct PageHooks {
    pub scripts: watch::Receiver<PageScripts>,
    pub calls: mpsc::Sender<BindingCall>,
}

#[cfg(test)]
impl PageHooks {
    /// Nothing on the page, and nobody listening.
    pub fn none() -> Self {
        let (_, scripts) = watch::channel(PageScripts::default());
        let (calls, _) = mpsc::channel(1);
        Self { scripts, calls }
    }
}

type Reply = Result<Value, String>;

struct Envelope {
    /// A child session to send it on; the page's own without one.
    session: Option<String>,
    method: &'static str,
    params: Value,
    reply: oneshot::Sender<Reply>,
}

#[derive(Clone)]
pub struct SessionHandle {
    commands: mpsc::Sender<Envelope>,
    state: watch::Receiver<State>,
    url: watch::Receiver<Option<String>>,
    events: broadcast::Sender<Value>,
    /// The DevTools port, for `others`; `None` if the base URL names none.
    port: Option<u16>,
}

impl SessionHandle {
    /// One command, answered or failed within `limit`. A session that is down
    /// is answered immediately - the driver already knows, so there is
    /// nothing to wait out.
    pub async fn call(
        &self,
        heartbeat: &Heartbeat,
        method: &'static str,
        params: Value,
        limit: Duration,
    ) -> Reply {
        // naked: call_on bounds itself with within()
        self.call_on(heartbeat, None, method, params, limit).await
    }

    /// `call`, on a child session (`BindingCall::session`) when one is given.
    pub async fn call_on(
        &self,
        heartbeat: &Heartbeat,
        session: Option<&str>,
        method: &'static str,
        params: Value,
        limit: Duration,
    ) -> Reply {
        let down = {
            let state = self.state.borrow();
            (!state.up).then(|| state.reason.clone())
        };
        if let Some(reason) = down {
            return Err(format!("no session: {reason}"));
        }

        let (reply, answer) = oneshot::channel();
        self.commands
            .try_send(Envelope {
                session: session.map(str::to_string),
                method,
                params,
                reply,
            })
            .map_err(|_| "the session is not taking commands".to_string())?;

        match heartbeat.within(method, limit, answer).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("the session was lost before it answered".to_string()),
            Err(expired) => Err(expired.to_string()),
        }
    }

    pub fn generation(&self) -> u64 {
        self.state.borrow().generation
    }

    /// Is there a live session to the page right now?
    pub fn is_up(&self) -> bool {
        self.state.borrow().up
    }

    /// Resolves once the driver's first connection attempt has concluded,
    /// whichever way it went. The one-socket-per-command client connected
    /// inside the first cycle; without this wait the first cycle would run
    /// before the session had even tried, and report a healthy browser as
    /// silent. Callers bound it - a slow browser must not hold up startup.
    pub async fn first_attempt(&self) {
        let mut state = self.state.clone();
        let _ = state
            .wait_for(|state| state.up || state.reason != NOT_CONNECTED_YET)
            .await; // naked: the caller bounds it
    }

    /// What the page is showing, kept current by the browser's own events.
    /// `None` while the session is down or not yet primed - "cannot tell",
    /// which the agent never mistakes for drift.
    pub fn current_url(&self) -> Option<String> {
        self.url.borrow().clone()
    }

    /// How many DevTools clients besides this agent are connected to the
    /// browser right now (`clients.rs`). 0 when that cannot be told.
    pub async fn others(&self) -> usize {
        let Some(port) = self.port else {
            return 0;
        };
        deadline::blocking("looking for other DevTools clients", move || {
            Ok(super::clients::foreign(port))
        })
        .await
        .unwrap_or(0)
    }

    /// Every event the browser sends. Nothing listens yet; this is where TODO
    /// item 5's `DeviceAccess.deviceRequestPrompted` handler attaches.
    #[allow(dead_code)]
    pub fn events(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    #[cfg(test)]
    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }
}

/// Start the driver. It connects in the background and keeps reconnecting
/// until `shutdown` changes.
#[cfg(test)]
pub fn spawn(
    config: SessionConfig,
    log: Arc<Log>,
    shutdown: watch::Receiver<bool>,
) -> SessionHandle {
    spawn_with(config, PageHooks::none(), log, shutdown)
}

/// `spawn`, with the page bridge's scripts and binding.
pub fn spawn_with(
    config: SessionConfig,
    hooks: PageHooks,
    log: Arc<Log>,
    shutdown: watch::Receiver<bool>,
) -> SessionHandle {
    let (commands, commands_rx) = mpsc::channel(16);
    let (state, state_rx) = watch::channel(State {
        up: false,
        generation: 0,
        reason: NOT_CONNECTED_YET.to_string(),
    });
    let (url, url_rx) = watch::channel(None);
    let (events, _) = broadcast::channel(64);

    let seconds = config.timeout.as_secs() as i64;
    let port = super::clients::port_of(&config.base_url);
    let reloaded = hooks.scripts.borrow().reload;
    let driver = Driver {
        scripts: hooks.scripts,
        scripts_open: true,
        reloaded,
        calls: hooks.calls,
        // /json/list can list many targets; a megabyte is plenty.
        http: HyperHttp::new(seconds, seconds, 1 << 20, Heartbeat::detached()),
        config,
        log,
        commands: commands_rx,
        state,
        url,
        events: events.clone(),
        shutdown,
        generation: 0,
        last_target: None,
        page_gone: AtomicBool::new(false),
    };
    tokio::spawn(driver.run());

    SessionHandle {
        commands,
        state: state_rx,
        url: url_rx,
        events,
        port,
    }
}

struct Link {
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
    main_frame: Option<String>,
    /// What this connection registered, and the identifiers Chromium gave
    /// the scripts, to remove them when they change.
    applied: PageScripts,
    script_ids: Vec<String>,
    /// Requests the driver sent itself whose replies carry a script's
    /// identifier.
    script_requests: Vec<u64>,
    /// The same, for scripts replaced before their reply came: removed as
    /// soon as it does.
    stale_requests: Vec<u64>,
    /// The page's execution contexts, from `Runtime.executionContextCreated`:
    /// id to (origin, frame id, default world). Only kept with the binding.
    contexts: Contexts,
    /// The player's iframes in processes of their own, by child session.
    children: HashMap<String, Child>,
}

/// An iframe attached as a child session.
struct Child {
    /// Its target id, which is its main frame's id.
    target: String,
    contexts: Contexts,
}

impl Link {
    fn new(ws: WebSocketStream<TcpStream>) -> Self {
        Link {
            ws,
            next_id: 0,
            main_frame: None,
            applied: PageScripts::default(),
            script_ids: Vec::new(),
            script_requests: Vec::new(),
            stale_requests: Vec::new(),
            contexts: HashMap::new(),
            children: HashMap::new(),
        }
    }
}

struct Driver {
    scripts: watch::Receiver<PageScripts>,
    /// False once the scripts' sender is gone: nothing more will change.
    scripts_open: bool,
    /// The last `PageScripts::reload` acted on.
    reloaded: u64,
    calls: mpsc::Sender<BindingCall>,
    http: HyperHttp,
    config: SessionConfig,
    log: Arc<Log>,
    commands: mpsc::Receiver<Envelope>,
    state: watch::Sender<State>,
    url: watch::Sender<Option<String>>,
    events: broadcast::Sender<Value>,
    shutdown: watch::Receiver<bool>,
    generation: u64,
    /// The page target the last session was on.
    last_target: Option<String>,
    /// The page crashed or detached: the next session, even on the same
    /// target, is on a page that is no longer what it was. Atomic only so the
    /// `&self` priming path can set it.
    page_gone: AtomicBool,
}

impl Driver {
    async fn run(mut self) {
        let mut backoff = Duration::ZERO;

        loop {
            if !self.wait(backoff).await {
                return;
            }

            let reason = match self.establish().await {
                Ok(mut link) => {
                    let started = Instant::now();
                    let reason = self.serve(&mut link).await;
                    backoff = if started.elapsed() >= STABLE {
                        Duration::ZERO
                    } else {
                        next_backoff(backoff, self.config.reconnect_max)
                    };
                    reason
                }
                Err(reason) => {
                    backoff = next_backoff(backoff, self.config.reconnect_max);
                    reason
                }
            };

            self.go_down(reason);
            if *self.shutdown.borrow() {
                return;
            }
        }
    }

    /// Sit out the backoff, turning away any command that slips in while the
    /// session is down. `false` means shut down.
    async fn wait(&mut self, backoff: Duration) -> bool {
        let until = Instant::now() + backoff;

        loop {
            tokio::select! {
                biased;
                _ = self.shutdown.changed() => return false,
                _ = tokio::time::sleep_until(until) => return true,
                command = self.commands.recv() => match command {
                    None => return false,
                    Some(envelope) => {
                        let reason = self.state.borrow().reason.clone();
                        let _ = envelope.reply.send(Err(format!("no session: {reason}")));
                    }
                },
            }
        }
    }

    fn go_down(&mut self, reason: String) {
        self.url.send_replace(None);
        self.state.send_replace(State {
            up: false,
            generation: self.generation,
            reason: reason.clone(),
        });
        self.log.debug(format!("cdp session down: {reason}"));
    }

    async fn establish(&mut self) -> Result<Link, String> {
        let ws_url = targets::page_ws_url(&self.http, &self.config.base_url).await?; // naked: every phase inside page_ws_url() has its own within()
        let address = self.resolve(&ws_url).await?;
        let limit = self.config.timeout;

        let stream = match deadline::within("cdp connect", limit, TcpStream::connect(address)).await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => return Err(format!("connect to {address} failed: {err}")),
            Err(expired) => return Err(expired.to_string()),
        };

        let handshake = tokio_tungstenite::client_async(ws_url.as_str(), stream);
        let ws = match deadline::within("cdp websocket handshake", limit, handshake).await {
            Ok(Ok((ws, _response))) => ws,
            Ok(Err(err)) => return Err(format!("websocket handshake failed: {err}")),
            Err(expired) => return Err(expired.to_string()),
        };

        let mut link = Link::new(ws);
        self.prime(&mut link).await?;

        // Only a new page is news to the agent. Reconnecting to the same
        // target after a transport hiccup - or after the agent itself stalled
        // and missed a few pings - is the same page, still showing what it
        // was; bumping the generation there reloaded a public screen for
        // nothing. Both halves are evaluated, so a crash flag never outlives
        // the session it was meant for.
        let new_target = self.last_target.as_deref() != Some(ws_url.as_str());
        let page_gone = self.page_gone.swap(false, Ordering::Relaxed);
        if new_target || page_gone {
            self.generation += 1;
        }
        self.last_target = Some(ws_url.clone());

        self.state.send_replace(State {
            up: true,
            generation: self.generation,
            reason: String::new(),
        });
        self.log.debug(format!(
            "cdp session up (generation {}) on {ws_url}",
            self.generation
        ));

        Ok(link)
    }

    /// Chromium reports `127.0.0.1:9222`, which parses without a resolver.
    /// Only an operator pointing `KIOSK_CDP_URL` at a hostname reaches DNS,
    /// and then it is bounded like everything else.
    async fn resolve(&self, ws_url: &str) -> Result<SocketAddr, String> {
        let authority = targets::authority(ws_url)?;
        if let Ok(address) = authority.parse::<SocketAddr>() {
            return Ok(address);
        }

        let lookup = tokio::net::lookup_host(authority.clone());
        match deadline::within("cdp DNS lookup", self.config.timeout, lookup).await {
            Ok(Ok(mut found)) => found
                .next()
                .ok_or_else(|| format!("{ws_url} does not resolve")),
            Ok(Err(err)) => Err(format!("{ws_url} does not resolve: {err}")),
            Err(expired) => Err(expired.to_string()),
        }
    }

    /// Everything a session needs before it counts as up.
    async fn prime(&mut self, link: &mut Link) -> Result<(), String> {
        // First, and required: without it Inspector.targetCrashed and
        // Inspector.detached are never delivered, and a crashed renderer is
        // noticed only the slow way - failed checks, then a full browser
        // restart. On a tab that is already a sad tab, Chromium replays
        // targetCrashed in answer to this, which `rpc` records.
        self.rpc(link, "Inspector.enable", json!({})).await?;

        // A crashed tab refuses Page.enable ("Target crashed") and everything
        // else - except Page.navigate, which brings it back. So a sad tab is
        // not a failed connection but a session on a new page: bring it up,
        // and the agent's re-navigation reloads it. Priming against it would
        // fail on every attempt until the browser was restarted, which is
        // exactly what used to happen.
        match self.rpc(link, "Page.enable", json!({})).await {
            Ok(_) => {}
            Err(err)
                if self.page_gone.load(Ordering::Relaxed) || err.contains("Target crashed") =>
            {
                self.page_gone.store(true, Ordering::Relaxed);
                self.url.send_replace(None);
                return Ok(());
            }
            Err(err) => return Err(err),
        }

        // Page.enable does not replay the current URL, so ask for it once;
        // frameNavigated keeps it fresh from here on.
        let tree = self.rpc(link, "Page.getFrameTree", json!({})).await?;
        let frame = &tree["frameTree"]["frame"];
        link.main_frame = frame["id"].as_str().map(str::to_string);
        self.url.send_replace(protocol::frame_url(frame));

        // Experimental domain: a Chromium that does not know it must not cost
        // us the session.
        if self.config.device_access {
            if let Err(err) = self.rpc(link, "DeviceAccess.enable", json!({})).await {
                self.log.info(format!(
                    "DeviceAccess.enable failed ({err}); carrying on without it"
                ));
            }
        }

        // Runtime.enable is sent only for the page bridge, which needs
        // Runtime.bindingCalled and the contexts' origins: Runtime.evaluate
        // works without it, and enabling it puts console noise on the wire,
        // which `runtime_event` drops.
        let wanted = self.scripts.borrow_and_update().clone();
        self.apply(link, &wanted).await?;
        if wanted.reload != self.reloaded {
            self.reloaded = wanted.reload;
            self.fire(link, "Page.reload", json!({ "ignoreCache": true }))
                .await?;
        }
        Ok(())
    }

    /// One command whose reply nobody waits for. The reply still arrives and
    /// is dropped by the serve loop, unless its id was kept to match it.
    async fn fire(&self, link: &mut Link, method: &str, params: Value) -> Result<u64, String> {
        link.next_id += 1;
        let text = protocol::request(link.next_id, method, &params);
        match deadline::within(
            "cdp send",
            self.config.timeout,
            link.ws.send(Message::Text(text.into())),
        )
        .await
        {
            Ok(Ok(())) => Ok(link.next_id),
            Ok(Err(err)) => Err(format!("send failed: {err}")),
            Err(expired) => Err(expired.to_string()),
        }
    }

    /// Bring what this connection registered in line with `wanted`. Only the
    /// difference is sent, so a new snapshot in the preamble swaps the
    /// scripts without touching the binding. Replies come back to the serve
    /// loop, which keeps the scripts' identifiers; a failure there is logged
    /// by nobody on purpose - the page simply goes without.
    async fn apply(&self, link: &mut Link, wanted: &PageScripts) -> Result<(), String> {
        let runtime = |scripts: &PageScripts| scripts.binding || scripts.player;
        if runtime(wanted) && !runtime(&link.applied) {
            self.fire(link, "Runtime.enable", json!({})).await?;
        }
        if wanted.binding && !link.applied.binding {
            self.fire(link, "Runtime.addBinding", json!({ "name": BINDING }))
                .await?;
            if let Some(rebind) = &wanted.rebind {
                self.fire(link, "Runtime.evaluate", json!({ "expression": rebind }))
                    .await?;
            }
        } else if !wanted.binding && link.applied.binding {
            self.fire(link, "Runtime.removeBinding", json!({ "name": BINDING }))
                .await?;
        }
        if wanted.player && !link.applied.player {
            self.fire(
                link,
                "Runtime.addBinding",
                json!({ "name": PLAYER_BINDING }),
            )
            .await?;
            self.fire(link, "Target.setAutoAttach", auto_attach(true))
                .await?;
        } else if !wanted.player && link.applied.player {
            self.fire(
                link,
                "Runtime.removeBinding",
                json!({ "name": PLAYER_BINDING }),
            )
            .await?;
            self.fire(link, "Target.setAutoAttach", auto_attach(false))
                .await?;
        }
        if !runtime(wanted) && runtime(&link.applied) {
            self.fire(link, "Runtime.disable", json!({})).await?;
            link.contexts.clear();
        }

        if wanted.sources != link.applied.sources {
            for identifier in std::mem::take(&mut link.script_ids) {
                self.fire(
                    link,
                    "Page.removeScriptToEvaluateOnNewDocument",
                    json!({ "identifier": identifier }),
                )
                .await?;
            }
            let unanswered = std::mem::take(&mut link.script_requests);
            link.stale_requests.extend(unanswered);
            for source in &wanted.sources {
                let id = self
                    .fire(
                        link,
                        "Page.addScriptToEvaluateOnNewDocument",
                        json!({ "source": source }),
                    )
                    .await?;
                link.script_requests.push(id);
            }
        }

        link.applied = wanted.clone();
        Ok(())
    }

    /// The Runtime domain's events, which only the bridge wants: the
    /// contexts' origins are kept, a binding call is passed on, and the rest,
    /// console messages above all, goes nowhere. `true` when the event was
    /// one of them.
    fn runtime_event(&self, link: &mut Link, method: &str, params: &Value) -> bool {
        if !method.starts_with("Runtime.") {
            return false;
        }
        if let Some(call) = track(
            &mut link.contexts,
            method,
            params,
            None,
            link.main_frame.as_deref(),
        ) {
            self.pass_on(call);
        }
        true
    }

    fn pass_on(&self, call: BindingCall) {
        if self.calls.try_send(call).is_err() {
            self.log
                .debug("cdp: a page bridge call was dropped; the bridge is busy");
        }
    }

    /// An event of a child session: an iframe in a process of its own. Its
    /// contexts are kept apart from the page's - context ids are per target,
    /// so they would collide - and its binding calls are passed on with the
    /// session they came from, which is where they are answered.
    async fn child_event(
        &self,
        link: &mut Link,
        session: &str,
        method: &str,
        params: &Value,
    ) -> Result<(), String> {
        if method == "Target.attachedToTarget" {
            return self.attached(link, params).await;
        }
        if method == "Target.detachedFromTarget" {
            if let Some(child) = params["sessionId"].as_str() {
                link.children.remove(child);
            }
            return Ok(());
        }
        if !method.starts_with("Runtime.") {
            return Ok(());
        }
        let Some(child) = link.children.get_mut(session) else {
            return Ok(());
        };
        let target = child.target.clone();
        if let Some(call) = track(
            &mut child.contexts,
            method,
            params,
            Some(session),
            Some(&target),
        ) {
            self.pass_on(call);
        }
        Ok(())
    }

    /// A target auto-attached while the player is on screen: an iframe gets
    /// the bindings and the frame scripts, then runs. Anything else attached
    /// (a worker) is only let go on. Paused until then by
    /// `waitForDebuggerOnStart`, so the frame's own scripts never run first.
    async fn attached(&self, link: &mut Link, params: &Value) -> Result<(), String> {
        let Some(session) = params["sessionId"].as_str() else {
            return Ok(());
        };
        let session = session.to_string();
        if params["targetInfo"]["type"].as_str() == Some("iframe") && link.applied.player {
            link.children.insert(
                session.clone(),
                Child {
                    // An iframe target's id is its main frame's.
                    target: params["targetInfo"]["targetId"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                    contexts: HashMap::new(),
                },
            );
            self.fire_on(link, &session, "Runtime.enable", json!({}))
                .await?;
            // Without it a child session's scripts are accepted and never
            // run: measured on Chromium 147, the frame's own scripts then
            // keep the binding and the frame says nothing about input.
            self.fire_on(link, &session, "Page.enable", json!({}))
                .await?;
            self.fire_on(
                link,
                &session,
                "Runtime.addBinding",
                json!({ "name": PLAYER_BINDING }),
            )
            .await?;
            if link.applied.binding {
                self.fire_on(
                    link,
                    &session,
                    "Runtime.addBinding",
                    json!({ "name": BINDING }),
                )
                .await?;
            }
            for source in link.applied.frame_sources.clone() {
                self.fire_on(
                    link,
                    &session,
                    "Page.addScriptToEvaluateOnNewDocument",
                    json!({ "source": source }),
                )
                .await?;
            }
            self.fire_on(link, &session, "Target.setAutoAttach", auto_attach(true))
                .await?;
        }
        self.fire_on(link, &session, "Runtime.runIfWaitingForDebugger", json!({}))
            .await
            .map(drop)
    }

    /// `fire`, on a child session.
    async fn fire_on(
        &self,
        link: &mut Link,
        session: &str,
        method: &str,
        params: Value,
    ) -> Result<u64, String> {
        link.next_id += 1;
        let text = protocol::request_on(link.next_id, session, method, &params);
        match deadline::within(
            "cdp send",
            self.config.timeout,
            link.ws.send(Message::Text(text.into())),
        )
        .await
        {
            Ok(Ok(())) => Ok(link.next_id),
            Ok(Err(err)) => Err(format!("send failed: {err}")),
            Err(expired) => Err(expired.to_string()),
        }
    }

    /// A command during priming, before the serve loop exists to route replies.
    async fn rpc(&self, link: &mut Link, method: &'static str, params: Value) -> Reply {
        link.next_id += 1;
        let id = link.next_id;
        let limit = self.config.timeout;

        let text = protocol::request(id, method, &params);
        match deadline::within("cdp send", limit, link.ws.send(Message::Text(text.into()))).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => return Err(format!("{method}: send failed: {err}")),
            Err(expired) => return Err(expired.to_string()),
        }

        let answer = async {
            loop {
                // naked: bounded by the within() below
                match link.ws.next().await {
                    None => return Err(format!("{method}: the browser closed the connection")),
                    Some(Err(err)) => return Err(format!("{method}: read failed: {err}")),
                    Some(Ok(Message::Close(frame))) => {
                        return Err(format!("{method}: websocket closed: {frame:?}"))
                    }
                    Some(Ok(Message::Text(text))) => match protocol::parse(&text) {
                        Incoming::Reply { id: got, result } if got == id => {
                            return result.map_err(|err| format!("{method}: {err}"))
                        }
                        Incoming::Event {
                            method,
                            params,
                            session,
                        } => {
                            // Child sessions are only asked for once the
                            // page is primed; one cannot be talking yet.
                            if session.is_some() || method.starts_with("Target.") {
                                continue;
                            }
                            if self.runtime_event(link, &method, &params) {
                                continue;
                            }
                            match on_event(
                                &self.url,
                                &self.events,
                                &mut link.main_frame,
                                &method,
                                &params,
                            ) {
                                // Keep waiting: the reply still comes, and
                                // `prime` decides what a sad tab means.
                                PageEvent::Crashed => self.page_gone.store(true, Ordering::Relaxed),
                                PageEvent::Detached(reason) => {
                                    self.page_gone.store(true, Ordering::Relaxed);
                                    return Err(reason);
                                }
                                PageEvent::Ordinary | PageEvent::Reloaded => {}
                            }
                        }
                        _ => {}
                    },
                    Some(Ok(_)) => {}
                }
            }
        };

        match deadline::within(method, limit, answer).await {
            Ok(result) => result,
            Err(expired) => Err(expired.to_string()),
        }
    }

    /// Route commands out and replies back until the session ends; returns why.
    async fn serve(&mut self, link: &mut Link) -> String {
        let limit = self.config.timeout;
        let mut pending: HashMap<u64, (&'static str, oneshot::Sender<Reply>)> = HashMap::new();

        let mut ping =
            tokio::time::interval_at(Instant::now() + self.config.ping, self.config.ping);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        // Pings sent since anything was last heard. Counted, not timed: a
        // clock would also run while the agent itself was stalled, and call a
        // healthy browser silent for the agent's own pause.
        let mut unanswered: u32 = 0;

        loop {
            tokio::select! {
                biased;

                _ = self.shutdown.changed() => return "shutting down".to_string(),

                command = self.commands.recv() => {
                    let Some(envelope) = command else {
                        return "the agent went away".to_string();
                    };
                    // A caller that already gave up must not have its command
                    // run late - least of all on a session that reconnected.
                    if envelope.reply.is_closed() {
                        continue;
                    }
                    // A renderer that is wedged still answers pings, so replies
                    // that never come would otherwise pile up here forever.
                    pending.retain(|_, (_, reply)| !reply.is_closed());

                    link.next_id += 1;
                    let id = link.next_id;
                    let text = match &envelope.session {
                        Some(session) => protocol::request_on(id, session, envelope.method, &envelope.params),
                        None => protocol::request(id, envelope.method, &envelope.params),
                    };
                    match deadline::within("cdp send", limit, link.ws.send(Message::Text(text.into()))).await {
                        Ok(Ok(())) => {
                            pending.insert(id, (envelope.method, envelope.reply));
                        }
                        Ok(Err(err)) => return format!("send failed: {err}"),
                        Err(expired) => return expired.to_string(),
                    }
                }

                frame = link.ws.next() => {
                    unanswered = 0;
                    match frame {
                        None => return "the browser closed the connection".to_string(),
                        Some(Err(err)) => return format!("read failed: {err}"),
                        Some(Ok(Message::Close(frame))) => return format!("websocket closed: {frame:?}"),
                        Some(Ok(Message::Text(text))) => match protocol::parse(&text) {
                            Incoming::Reply { id, result } => {
                                if let Some((method, reply)) = pending.remove(&id) {
                                    let _ = reply.send(result.map_err(|err| format!("{method}: {err}")));
                                } else if let Some(at) = link.script_requests.iter().position(|&request| request == id) {
                                    link.script_requests.remove(at);
                                    match result {
                                        Ok(reply) => {
                                            if let Some(identifier) = reply["identifier"].as_str() {
                                                link.script_ids.push(identifier.to_string());
                                            }
                                        }
                                        Err(err) => self.log.info(format!(
                                            "cdp: the browser refused a page script ({err})"
                                        )),
                                    }
                                } else if let Some(at) = link.stale_requests.iter().position(|&request| request == id) {
                                    link.stale_requests.remove(at);
                                    if let Some(identifier) = result.ok().and_then(|reply| reply["identifier"].as_str().map(str::to_string)) {
                                        if let Err(err) = self.fire(link, "Page.removeScriptToEvaluateOnNewDocument", json!({ "identifier": identifier })).await {
                                            return err;
                                        }
                                    }
                                }
                            }
                            Incoming::Event { method, params, session } => {
                                if let Some(session) = session {
                                    if let Err(err) = self.child_event(link, &session, &method, &params).await {
                                        return err;
                                    }
                                    continue;
                                }
                                if method == "Target.attachedToTarget" {
                                    if let Err(err) = self.attached(link, &params).await {
                                        return err;
                                    }
                                    continue;
                                }
                                if method.starts_with("Target.") {
                                    continue;
                                }
                                if self.runtime_event(link, &method, &params) {
                                    continue;
                                }
                                match on_event(&self.url, &self.events, &mut link.main_frame, &method, &params) {
                                    // The session survives a crash - it is
                                    // what will reload the tab. Only the page
                                    // is new, so only the generation moves.
                                    PageEvent::Crashed => {
                                        self.generation += 1;
                                        self.state.send_replace(State {
                                            up: true,
                                            generation: self.generation,
                                            reason: String::new(),
                                        });
                                        self.log.debug(format!(
                                            "cdp: the page crashed (generation {})",
                                            self.generation
                                        ));
                                    }
                                    // The Page domain does not survive the
                                    // crash; enable it again so URL tracking
                                    // resumes. Fire and forget: no caller.
                                    //
                                    // The page scripts and the binding went
                                    // with the renderer: register them again,
                                    // and reload so the page that navigation
                                    // brought back runs them.
                                    PageEvent::Reloaded => {
                                        if let Err(err) = self.fire(link, "Page.enable", json!({})).await {
                                            return err;
                                        }
                                        link.applied = PageScripts::default();
                                        link.script_ids.clear();
                                        link.script_requests.clear();
                                        link.stale_requests.clear();
                                        link.contexts.clear();
                                        link.children.clear();
                                        let wanted = self.scripts.borrow().clone();
                                        if let Err(err) = self.apply(link, &wanted).await {
                                            return err;
                                        }
                                        if wanted.binding || !wanted.sources.is_empty() {
                                            if let Err(err) = self.fire(link, "Page.reload", json!({ "ignoreCache": true })).await {
                                                return err;
                                            }
                                        }
                                    }
                                    PageEvent::Detached(reason) => {
                                        self.page_gone.store(true, Ordering::Relaxed);
                                        return reason;
                                    }
                                    PageEvent::Ordinary => {}
                                }
                            }
                            Incoming::Other => {}
                        },
                        Some(Ok(_)) => {}
                    }
                }

                changed = self.scripts.changed(), if self.scripts_open => {
                    if changed.is_err() {
                        self.scripts_open = false;
                        continue;
                    }
                    let wanted = self.scripts.borrow_and_update().clone();
                    if let Err(err) = self.apply(link, &wanted).await {
                        return err;
                    }
                    if wanted.reload != self.reloaded {
                        self.reloaded = wanted.reload;
                        if let Err(err) = self.fire(link, "Page.reload", json!({ "ignoreCache": true })).await {
                            return err;
                        }
                    }
                }

                _ = ping.tick() => {
                    if unanswered >= 2 {
                        return format!("{unanswered} pings in a row went unanswered");
                    }
                    match deadline::within("cdp ping", limit, link.ws.send(Message::Ping(Vec::new().into()))).await {
                        Ok(Ok(())) => unanswered += 1,
                        Ok(Err(err)) => return format!("ping failed: {err}"),
                        Err(expired) => return expired.to_string(),
                    }
                }
            }
        }
    }
}

/// Keep the URL current, pass the event on, and say what the event means for
/// the session. A crash does *not* end it: a sad tab refuses everything except
/// `Page.navigate`, which reloads it, so the session stays up, the generation
/// moves, and the agent's re-navigation is what brings the page back. Measured
/// on Chromium 147 - reconnecting instead meant priming a sad tab, which fails
/// every time, until the browser was restarted. A detached target does end it.
/// What an event means for the session, beyond keeping the URL current.
#[derive(Debug, PartialEq, Eq)]
enum PageEvent {
    Ordinary,
    /// The renderer is gone; the tab is a sad tab on the same target.
    Crashed,
    /// A navigation brought a crashed tab back.
    Reloaded,
    /// The target itself is gone: the session is over.
    Detached(String),
}

fn on_event(
    url: &watch::Sender<Option<String>>,
    events: &broadcast::Sender<Value>,
    main_frame: &mut Option<String>,
    method: &str,
    params: &Value,
) -> PageEvent {
    let meaning = match method {
        "Page.frameNavigated" => {
            let frame = &params["frame"];
            if protocol::is_main_frame(frame) {
                *main_frame = frame["id"].as_str().map(str::to_string);
                url.send_replace(protocol::frame_url(frame));
            }
            PageEvent::Ordinary
        }
        // In-page routing: the site's own pushState and hash changes.
        "Page.navigatedWithinDocument"
            if main_frame.is_some() && params["frameId"].as_str() == main_frame.as_deref() =>
        {
            if let Some(within_document) = params["url"].as_str() {
                url.send_replace(Some(within_document.to_string()));
            }
            PageEvent::Ordinary
        }
        "Inspector.detached" => PageEvent::Detached(format!(
            "the page detached ({})",
            params["reason"].as_str().unwrap_or("no reason given")
        )),
        "Inspector.targetCrashed" => {
            // A sad tab shows no page of ours: "cannot tell", never drift.
            url.send_replace(None);
            PageEvent::Crashed
        }
        "Inspector.targetReloadedAfterCrash" => PageEvent::Reloaded,
        _ => PageEvent::Ordinary,
    };

    // Nobody may be listening, which is not an error.
    let _ = events.send(json!({ "method": method, "params": params }));
    meaning
}

/// A session's execution contexts: id to (origin, frame id, default world).
type Contexts = HashMap<i64, (String, String, bool)>;

/// Keep `contexts` on one Runtime event, and make a binding call of it when
/// it is one. `main_frame` is the frame whose default world counts as `top`
/// for the page's own session; a child session passes its iframe's frame,
/// whose calls are never `top`.
fn track(
    contexts: &mut Contexts,
    method: &str,
    params: &Value,
    session: Option<&str>,
    main_frame: Option<&str>,
) -> Option<BindingCall> {
    match method {
        "Runtime.executionContextCreated" => {
            let context = &params["context"];
            if let Some(id) = context["id"].as_i64() {
                let aux = &context["auxData"];
                contexts.insert(
                    id,
                    (
                        context["origin"].as_str().unwrap_or("").to_string(),
                        aux["frameId"].as_str().unwrap_or("").to_string(),
                        aux["isDefault"].as_bool().unwrap_or(false),
                    ),
                );
            }
            None
        }
        "Runtime.executionContextDestroyed" => {
            if let Some(id) = params["executionContextId"].as_i64() {
                contexts.remove(&id);
            }
            None
        }
        "Runtime.executionContextsCleared" => {
            contexts.clear();
            None
        }
        "Runtime.bindingCalled" => {
            let binding = match params["name"].as_str() {
                Some(BINDING) => BINDING,
                Some(PLAYER_BINDING) => PLAYER_BINDING,
                _ => return None,
            };
            let context = params["executionContextId"].as_i64().unwrap_or(0);
            let (origin, frame, top) = match contexts.get(&context) {
                Some((origin, frame, default)) => (
                    origin.clone(),
                    *default,
                    *default && session.is_none() && main_frame == Some(frame.as_str()),
                ),
                None => (String::new(), false, false),
            };
            Some(BindingCall {
                binding,
                session: session.map(str::to_string),
                context,
                origin,
                top,
                frame,
                payload: params["payload"].as_str().unwrap_or("").to_string(),
            })
        }
        _ => None,
    }
}

/// `Target.setAutoAttach` for the player's frames: every iframe in a process
/// of its own as a child session on this connection (`flatten`), paused until
/// it has what it needs (`waitForDebuggerOnStart`).
fn auto_attach(on: bool) -> Value {
    json!({ "autoAttach": on, "waitForDebuggerOnStart": on, "flatten": true })
}

fn next_backoff(current: Duration, ceiling: Duration) -> Duration {
    if current.is_zero() {
        Duration::from_secs(1).min(ceiling)
    } else {
        (current * 2).min(ceiling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[derive(Clone, Debug)]
    enum Control {
        /// Drop the connection; the page itself is untouched.
        Drop,
        /// Report the page crashed, as Chromium does before a sad tab.
        Crash,
        /// Send this event to the agent, as the browser would.
        Emit(Value),
    }

    /// Just enough Chromium: `/json/list` on one port, a page target's
    /// websocket on another, and ways to lose the connection, restart the
    /// browser (a new target id) or crash the page.
    struct FakeChromium {
        base_url: String,
        control: broadcast::Sender<Control>,
        page: Arc<std::sync::atomic::AtomicU32>,
        /// Every command received, in order, across connections.
        received: Arc<std::sync::Mutex<Vec<(String, Value)>>>,
    }

    impl FakeChromium {
        async fn start() -> Self {
            let ws = TcpListener::bind("127.0.0.1:0").await.expect("bind ws");
            let ws_port = ws.local_addr().unwrap().port();
            let http = TcpListener::bind("127.0.0.1:0").await.expect("bind http");
            let http_port = http.local_addr().unwrap().port();
            let (control, _) = broadcast::channel(32);
            let page = Arc::new(std::sync::atomic::AtomicU32::new(1));

            let listed = Arc::clone(&page);
            tokio::spawn(async move {
                while let Ok((mut socket, _)) = http.accept().await {
                    let body = format!(
                        r#"[{{"type":"page","webSocketDebuggerUrl":"ws://127.0.0.1:{ws_port}/devtools/page/P{}"}}]"#,
                        listed.load(Ordering::Relaxed)
                    );
                    let mut buffer = [0u8; 1024];
                    let _ = socket.read(&mut buffer).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            });

            let controls = control.clone();
            // The page's own state, so a crash outlives the connection.
            let crashed = Arc::new(AtomicBool::new(false));
            let received = Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = Arc::clone(&received);
            let scripts = Arc::new(std::sync::atomic::AtomicU32::new(0));
            tokio::spawn(async move {
                while let Ok((socket, _)) = ws.accept().await {
                    let mut orders = controls.subscribe();
                    let sad = Arc::clone(&crashed);
                    let log = Arc::clone(&log);
                    let scripts = Arc::clone(&scripts);
                    tokio::spawn(async move {
                        let Ok(mut ws) = tokio_tungstenite::accept_async(socket).await else {
                            return;
                        };
                        // As in Chromium: Inspector events reach only a
                        // session that enabled the domain. The fake used to
                        // send them regardless, and so hid that the agent
                        // never asked.
                        let mut inspector = false;
                        let crash_event =
                            json!({ "method": "Inspector.targetCrashed", "params": {} })
                                .to_string();
                        loop {
                            tokio::select! {
                                order = orders.recv() => match order {
                                    Ok(Control::Crash) => {
                                        sad.store(true, Ordering::Relaxed);
                                        if inspector {
                                            let _ = ws.send(Message::Text(crash_event.clone().into())).await;
                                        }
                                    }
                                    Ok(Control::Emit(event)) => {
                                        let _ = ws.send(Message::Text(event.to_string().into())).await;
                                    }
                                    _ => return,
                                },
                                message = ws.next() => {
                                    let Some(Ok(Message::Text(text))) = message else {
                                        if message.is_none() { return; }
                                        continue;
                                    };
                                    let request: Value = serde_json::from_str(&text).unwrap();
                                    let id = request["id"].clone();
                                    let method = request["method"].as_str().unwrap();
                                    // A child session's command is logged as
                                    // `<session>:<method>`.
                                    let logged = match request["sessionId"].as_str() {
                                        Some(session) => format!("{session}:{method}"),
                                        None => method.to_string(),
                                    };
                                    log.lock().unwrap().push((logged, request["params"].clone()));

                                    // A sad tab, as Chromium 147 behaves: the
                                    // crash is replayed to a session that
                                    // enables the Inspector, every command but
                                    // Page.navigate is refused, and navigating
                                    // brings the tab back.
                                    if sad.load(Ordering::Relaxed) {
                                        match method {
                                            "Inspector.enable" => {
                                                inspector = true;
                                                let _ = ws.send(Message::Text(crash_event.clone().into())).await;
                                                let _ = ws.send(Message::Text(json!({ "id": id, "result": {} }).to_string().into())).await;
                                                continue;
                                            }
                                            "Page.navigate" => {
                                                sad.store(false, Ordering::Relaxed);
                                                let reloaded = json!({ "method": "Inspector.targetReloadedAfterCrash", "params": {} });
                                                let _ = ws.send(Message::Text(reloaded.to_string().into())).await;
                                            }
                                            _ => {
                                                let refused = json!({ "id": id, "error": { "code": -32000, "message": "Target crashed" } });
                                                let _ = ws.send(Message::Text(refused.to_string().into())).await;
                                                continue;
                                            }
                                        }
                                    }

                                    if method == "Inspector.enable" {
                                        inspector = true;
                                    }
                                    let result = match method {
                                        "Page.getFrameTree" => {
                                            json!({ "frameTree": { "frame": { "id": "F", "url": "http://kiosk.test/" } } })
                                        }
                                        "Runtime.evaluate" => json!({ "result": { "type": "number", "value": 2 } }),
                                        "Page.addScriptToEvaluateOnNewDocument" => {
                                            let n = scripts.fetch_add(1, Ordering::Relaxed) + 1;
                                            json!({ "identifier": format!("S{n}") })
                                        }
                                        "Page.navigate" => {
                                            let url = request["params"]["url"].clone();
                                            let _ = ws.send(Message::Text(json!({ "id": id, "result": { "frameId": "F" } }).to_string().into())).await;
                                            let event = json!({ "method": "Page.frameNavigated", "params": { "frame": { "id": "F", "url": url } } });
                                            let _ = ws.send(Message::Text(event.to_string().into())).await;
                                            continue;
                                        }
                                        _ => json!({}),
                                    };
                                    let _ = ws.send(Message::Text(json!({ "id": id, "result": result }).to_string().into())).await;
                                }
                            }
                        }
                    });
                }
            });

            Self {
                base_url: format!("http://127.0.0.1:{http_port}"),
                control,
                page,
                received,
            }
        }

        fn emit(&self, event: Value) {
            let _ = self.control.send(Control::Emit(event));
        }

        /// How many times `method` came, with params matching `params`
        /// where `params` is not null.
        fn count(&self, method: &str, params: &Value) -> usize {
            self.received
                .lock()
                .unwrap()
                .iter()
                .filter(|(got, with)| got == method && (params.is_null() || with == params))
                .count()
        }

        fn drop_the_connection(&self) {
            let _ = self.control.send(Control::Drop);
        }

        fn restart_the_browser(&self) {
            self.page.fetch_add(1, Ordering::Relaxed);
            let _ = self.control.send(Control::Drop);
        }

        fn crash_the_page(&self) {
            let _ = self.control.send(Control::Crash);
        }
    }

    fn config(base_url: &str) -> SessionConfig {
        SessionConfig {
            base_url: base_url.to_string(),
            timeout: Duration::from_secs(2),
            ping: Duration::from_secs(10),
            reconnect_max: Duration::from_millis(200),
            device_access: false,
        }
    }

    async fn until(what: &'static str, condition: impl Fn() -> bool) {
        let wait = async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        deadline::within(what, Duration::from_secs(5), wait)
            .await
            .unwrap_or_else(|expired| panic!("{expired}"));
    }

    #[tokio::test]
    async fn a_session_comes_up_primed_and_answers() {
        let chromium = FakeChromium::start().await;
        let (_stop, shutdown) = watch::channel(false);
        let session = spawn(
            config(&chromium.base_url),
            Arc::new(Log::buffered(true)),
            shutdown,
        );
        let heartbeat = Heartbeat::detached();

        until("the first session", || session.state().up).await;
        assert_eq!(session.generation(), 1);
        assert_eq!(session.current_url().as_deref(), Some("http://kiosk.test/"));

        let reply = session
            .call(
                &heartbeat,
                "Runtime.evaluate",
                json!({ "expression": "1 + 1" }),
                Duration::from_secs(2),
            )
            .await
            .expect("evaluate");
        assert_eq!(reply["result"]["value"], 2);

        session
            .call(
                &heartbeat,
                "Page.navigate",
                json!({ "url": "http://kiosk.test/next" }),
                Duration::from_secs(2),
            )
            .await
            .expect("navigate");
        until("the frameNavigated event", || {
            session.current_url().as_deref() == Some("http://kiosk.test/next")
        })
        .await;
    }

    async fn session_on(chromium: &FakeChromium) -> SessionHandle {
        let (stop, shutdown) = watch::channel(false);
        // Leaked on purpose: a dropped sender reads as shutdown.
        std::mem::forget(stop);
        let session = spawn(
            config(&chromium.base_url),
            Arc::new(Log::buffered(true)),
            shutdown,
        );
        until("the first session", || session.state().up).await;
        assert_eq!(session.generation(), 1);
        session
    }

    async fn reconnected(session: &SessionHandle) {
        until("the session to drop", || !session.state().up).await;
        until("the session to come back", || session.state().up).await;
    }

    #[tokio::test]
    async fn a_dropped_connection_to_the_same_page_is_not_news() {
        // The regression this guards against: an agent stall, or any
        // transport hiccup, used to read as a browser restart and reload a
        // perfectly good page on a public screen.
        let chromium = FakeChromium::start().await;
        let session = session_on(&chromium).await;

        chromium.drop_the_connection();
        reconnected(&session).await;

        assert_eq!(session.generation(), 1);
        assert_eq!(session.current_url().as_deref(), Some("http://kiosk.test/"));
    }

    #[tokio::test]
    async fn a_restarted_browser_is_a_new_generation() {
        let chromium = FakeChromium::start().await;
        let session = session_on(&chromium).await;

        chromium.restart_the_browser();
        reconnected(&session).await;

        assert_eq!(session.generation(), 2);
    }

    #[tokio::test]
    async fn a_crash_keeps_the_session_and_moves_the_generation() {
        // The session survives, because it is what reloads the tab: the
        // agent sees the new generation and re-navigates on it.
        let chromium = FakeChromium::start().await;
        let session = session_on(&chromium).await;

        chromium.crash_the_page();
        until("the new generation", || session.generation() == 2).await;
        assert!(session.state().up, "a crash must not end the session");
        assert_eq!(session.current_url(), None, "a sad tab is 'cannot tell'");

        let heartbeat = Heartbeat::detached();
        let refused = session
            .call(
                &heartbeat,
                "Runtime.evaluate",
                json!({}),
                Duration::from_secs(2),
            )
            .await;
        assert!(refused.unwrap_err().contains("Target crashed"));

        session
            .call(
                &heartbeat,
                "Page.navigate",
                json!({ "url": "http://kiosk.test/" }),
                Duration::from_secs(2),
            )
            .await
            .expect("navigate reloads the tab");
        until("the reloaded page's URL", || {
            session.current_url().as_deref() == Some("http://kiosk.test/")
        })
        .await;
        assert_eq!(session.generation(), 2);
        assert!(session.state().up);
    }

    #[tokio::test]
    async fn a_session_that_finds_a_sad_tab_comes_up_on_a_new_page() {
        // Reconnecting to a tab that crashed meanwhile. Priming against it
        // used to fail on every attempt, until the browser was restarted.
        let chromium = FakeChromium::start().await;
        let session = session_on(&chromium).await;

        chromium.crash_the_page();
        until("the crash", || session.generation() == 2).await;
        chromium.drop_the_connection();
        reconnected(&session).await;

        assert!(session.state().up);
        assert_eq!(
            session.generation(),
            3,
            "a sad tab found while priming is a new page"
        );
        assert_eq!(session.current_url(), None);
    }

    #[tokio::test]
    async fn the_first_attempt_is_awaitable_either_way() {
        let chromium = FakeChromium::start().await;
        let (_stop, shutdown) = watch::channel(false);
        let up = spawn(
            config(&chromium.base_url),
            Arc::new(Log::buffered(true)),
            shutdown.clone(),
        );
        let refused = spawn(
            config("http://127.0.0.1:1"),
            Arc::new(Log::buffered(true)),
            shutdown,
        );

        let settle = async {
            up.first_attempt().await;
            refused.first_attempt().await;
        };
        deadline::within("both first attempts", Duration::from_secs(5), settle)
            .await
            .expect("settled");

        assert!(up.state().up);
        assert!(!refused.state().up);
        assert_ne!(refused.state().reason, NOT_CONNECTED_YET);
    }

    #[tokio::test]
    async fn a_call_with_no_browser_fails_at_once() {
        // Port 1 on the loopback refuses instantly.
        let (_stop, shutdown) = watch::channel(false);
        let session = spawn(
            config("http://127.0.0.1:1"),
            Arc::new(Log::buffered(true)),
            shutdown,
        );
        let started = Instant::now();

        let outcome = session
            .call(
                &Heartbeat::detached(),
                "Runtime.evaluate",
                json!({}),
                Duration::from_secs(2),
            )
            .await;

        assert!(outcome.unwrap_err().starts_with("no session"));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(session.generation(), 0);
        assert_eq!(session.current_url(), None);
    }

    #[test]
    fn backoff_doubles_up_to_the_ceiling() {
        let ceiling = Duration::from_secs(15);
        let mut backoff = Duration::ZERO;
        let mut seen = Vec::new();
        for _ in 0..6 {
            backoff = next_backoff(backoff, ceiling);
            seen.push(backoff.as_secs());
        }

        assert_eq!(seen, vec![1, 2, 4, 8, 15, 15]);
    }

    /// A session with the page bridge's scripts, and where the page's calls
    /// arrive.
    async fn bridged_session(
        chromium: &FakeChromium,
        scripts: PageScripts,
    ) -> (
        SessionHandle,
        watch::Sender<PageScripts>,
        mpsc::Receiver<BindingCall>,
    ) {
        let (stop, shutdown) = watch::channel(false);
        std::mem::forget(stop);
        let (scripts, scripts_rx) = watch::channel(scripts);
        let (calls_tx, calls) = mpsc::channel(8);
        let session = spawn_with(
            config(&chromium.base_url),
            PageHooks {
                scripts: scripts_rx,
                calls: calls_tx,
            },
            Arc::new(Log::buffered(true)),
            shutdown,
        );
        until("the first session", || session.state().up).await;
        (session, scripts, calls)
    }

    fn bridge_scripts(sources: &[&str]) -> PageScripts {
        PageScripts {
            binding: true,
            sources: sources.iter().map(|source| source.to_string()).collect(),
            rebind: Some("rebind()".to_string()),
            reload: 0,
            ..PageScripts::default()
        }
    }

    #[tokio::test]
    async fn every_session_registers_the_page_scripts_and_the_binding() {
        let chromium = FakeChromium::start().await;
        let (session, _scripts, _calls) =
            bridged_session(&chromium, bridge_scripts(&["preamble", "inject"])).await;
        let add = "Page.addScriptToEvaluateOnNewDocument";

        until("the scripts", || {
            chromium.count(add, &json!({ "source": "inject" })) == 1
        })
        .await;
        assert_eq!(chromium.count(add, &json!({ "source": "preamble" })), 1);
        assert_eq!(chromium.count("Runtime.enable", &Value::Null), 1);
        assert_eq!(
            chromium.count("Runtime.addBinding", &json!({ "name": BINDING })),
            1
        );
        assert_eq!(
            chromium.count("Page.reload", &Value::Null),
            0,
            "no reload for a new session"
        );

        // Chromium drops them with the connection, so a reconnect to the
        // same page registers them again - and hands the page the new
        // binding instead of reloading it.
        chromium.drop_the_connection();
        reconnected(&session).await;
        until("the scripts again", || {
            chromium.count(add, &json!({ "source": "inject" })) == 2
        })
        .await;
        assert_eq!(chromium.count("Runtime.addBinding", &Value::Null), 2);
        assert_eq!(
            chromium.count("Runtime.evaluate", &json!({ "expression": "rebind()" })),
            2
        );
        assert_eq!(chromium.count("Page.reload", &Value::Null), 0);
    }

    #[tokio::test]
    async fn changed_scripts_replace_the_old_ones_and_a_reload_reloads() {
        let chromium = FakeChromium::start().await;
        let (_session, scripts, _calls) =
            bridged_session(&chromium, bridge_scripts(&["preamble", "inject"])).await;
        let add = "Page.addScriptToEvaluateOnNewDocument";
        let remove = "Page.removeScriptToEvaluateOnNewDocument";
        until("the scripts", || chromium.count(add, &Value::Null) == 2).await;

        // A new snapshot in the preamble: swapped, no reload.
        // Sent at once, so the old scripts' identifiers may still be on the
        // way: those are removed when they arrive.
        scripts.send_replace(bridge_scripts(&["preamble 2", "inject"]));
        until("the swap", || chromium.count(remove, &Value::Null) == 2).await;
        until("the new scripts", || chromium.count(add, &Value::Null) == 4).await;
        assert_eq!(chromium.count(remove, &json!({ "identifier": "S1" })), 1);
        assert_eq!(chromium.count("Page.reload", &Value::Null), 0);
        assert_eq!(
            chromium.count("Runtime.addBinding", &Value::Null),
            1,
            "the binding stays"
        );

        // A changed injected script: swapped, and the page reloaded.
        scripts.send_replace(PageScripts {
            reload: 1,
            ..bridge_scripts(&["preamble 2", "inject 2"])
        });
        until("the reload", || {
            chromium.count("Page.reload", &json!({ "ignoreCache": true })) == 1
        })
        .await;
        assert_eq!(chromium.count(add, &json!({ "source": "inject 2" })), 1);
    }

    #[tokio::test]
    async fn a_binding_call_arrives_with_where_the_browser_says_it_came_from() {
        let chromium = FakeChromium::start().await;
        let (_session, _scripts, mut calls) =
            bridged_session(&chromium, bridge_scripts(&["preamble"])).await;
        until("the binding", || {
            chromium.count("Runtime.addBinding", &Value::Null) == 1
        })
        .await;

        let context = |id: i64, frame: &str, default: bool| {
            json!({ "method": "Runtime.executionContextCreated", "params": { "context": {
                "id": id, "origin": "http://kiosk.test",
                "auxData": { "frameId": frame, "isDefault": default }
            } } })
        };
        let called = |id: i64, payload: &str| {
            json!({ "method": "Runtime.bindingCalled", "params": {
                "name": BINDING, "executionContextId": id, "payload": payload
            } })
        };
        chromium.emit(context(1, "F", true));
        chromium.emit(context(2, "iframe", true));
        chromium.emit(context(3, "F", false));
        chromium.emit(called(1, "from the page"));
        chromium.emit(called(2, "from an iframe"));
        chromium.emit(called(3, "from an isolated world"));
        chromium.emit(called(9, "from nowhere known"));

        let mut got = Vec::new();
        for _ in 0..4 {
            let call = deadline::within("a binding call", Duration::from_secs(5), calls.recv())
                .await
                .expect("a call in time")
                .expect("the channel is open");
            got.push((call.payload, call.top, call.origin));
        }
        assert_eq!(
            got,
            vec![
                (
                    "from the page".to_string(),
                    true,
                    "http://kiosk.test".to_string()
                ),
                (
                    "from an iframe".to_string(),
                    false,
                    "http://kiosk.test".to_string()
                ),
                (
                    "from an isolated world".to_string(),
                    false,
                    "http://kiosk.test".to_string()
                ),
                ("from nowhere known".to_string(), false, String::new()),
            ]
        );
    }

    #[tokio::test]
    async fn the_player_attaches_its_frames_and_hears_from_them() {
        let chromium = FakeChromium::start().await;
        let scripts = PageScripts {
            binding: true,
            sources: vec!["input".to_string(), "preamble".to_string()],
            player: true,
            frame_sources: vec!["input".to_string(), "preamble".to_string()],
            ..PageScripts::default()
        };
        let (_session, _scripts, mut calls) = bridged_session(&chromium, scripts).await;
        until("the frames attached", || {
            chromium.count("Target.setAutoAttach", &auto_attach(true)) == 1
                && chromium.count("Runtime.addBinding", &json!({ "name": PLAYER_BINDING })) == 1
        })
        .await;

        // An iframe in a process of its own, paused until it is ready.
        chromium.emit(json!({ "method": "Target.attachedToTarget", "params": {
            "sessionId": "C1",
            "targetInfo": { "targetId": "FRAME1", "type": "iframe" },
            "waitingForDebugger": true
        } }));
        until("the frame ready to run", || {
            chromium.count("C1:Runtime.runIfWaitingForDebugger", &Value::Null) == 1
        })
        .await;
        assert_eq!(
            chromium.count("C1:Runtime.addBinding", &json!({ "name": PLAYER_BINDING })),
            1
        );
        assert_eq!(
            chromium.count("C1:Runtime.addBinding", &json!({ "name": BINDING })),
            1
        );
        assert_eq!(
            chromium.count("C1:Page.addScriptToEvaluateOnNewDocument", &Value::Null),
            2
        );
        // A child session runs its scripts only with the Page domain on.
        assert_eq!(chromium.count("C1:Page.enable", &Value::Null), 1);

        // The frame's contexts are its own: the same id as the page's means
        // nothing to the page's.
        chromium.emit(
            json!({ "method": "Runtime.executionContextCreated", "sessionId": "C1",
            "params": { "context": { "id": 1, "origin": "https://menu.test",
                "auxData": { "frameId": "FRAME1", "isDefault": true } } } }),
        );
        chromium.emit(json!({ "method": "Runtime.executionContextCreated",
            "params": { "context": { "id": 1, "origin": "http://127.0.0.1",
                "auxData": { "frameId": "F", "isDefault": true } } } }));
        chromium.emit(
            json!({ "method": "Runtime.bindingCalled", "sessionId": "C1", "params": {
            "name": PLAYER_BINDING, "executionContextId": 1, "payload": "{\"event\":\"input\"}"
        } }),
        );
        chromium.emit(
            json!({ "method": "Runtime.bindingCalled", "sessionId": "C1", "params": {
            "name": BINDING, "executionContextId": 1, "payload": "bridge"
        } }),
        );
        chromium.emit(json!({ "method": "Runtime.bindingCalled", "params": {
            "name": PLAYER_BINDING, "executionContextId": 1, "payload": "report"
        } }));

        let mut got = Vec::new();
        for _ in 0..3 {
            let call = deadline::within("a binding call", Duration::from_secs(5), calls.recv())
                .await
                .expect("a call in time")
                .expect("the channel is open");
            got.push((
                call.binding,
                call.session,
                call.origin,
                call.top,
                call.frame,
            ));
        }
        assert_eq!(
            got,
            vec![
                (
                    PLAYER_BINDING,
                    Some("C1".to_string()),
                    "https://menu.test".to_string(),
                    false,
                    true
                ),
                (
                    BINDING,
                    Some("C1".to_string()),
                    "https://menu.test".to_string(),
                    false,
                    true
                ),
                (
                    PLAYER_BINDING,
                    None,
                    "http://127.0.0.1".to_string(),
                    true,
                    true
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_worker_attached_is_only_let_go_on() {
        let chromium = FakeChromium::start().await;
        let scripts = PageScripts {
            player: true,
            ..PageScripts::default()
        };
        let (_session, _scripts, _calls) = bridged_session(&chromium, scripts).await;
        until("auto-attach", || {
            chromium.count("Target.setAutoAttach", &auto_attach(true)) == 1
        })
        .await;
        chromium.emit(json!({ "method": "Target.attachedToTarget", "params": {
            "sessionId": "W1",
            "targetInfo": { "targetId": "W", "type": "worker" },
            "waitingForDebugger": true
        } }));
        until("the worker running", || {
            chromium.count("W1:Runtime.runIfWaitingForDebugger", &Value::Null) == 1
        })
        .await;
        assert_eq!(chromium.count("W1:Runtime.addBinding", &Value::Null), 0);
    }
}
