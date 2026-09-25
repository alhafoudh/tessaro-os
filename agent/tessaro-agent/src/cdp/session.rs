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
    /// Page zoom in percent (`browser.zoom`); 100 sends nothing.
    pub zoom: u16,
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

type Reply = Result<Value, String>;

struct Envelope {
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
pub fn spawn(
    config: SessionConfig,
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
    let driver = Driver {
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
        zoom_told: false,
    };
    tokio::spawn(driver.run());

    SessionHandle {
        commands,
        state: state_rx,
        url: url_rx,
        events,
    }
}

struct Link {
    ws: WebSocketStream<TcpStream>,
    next_id: u64,
    main_frame: Option<String>,
    /// The page's own window, before the zoom: the override's base. `None`
    /// until asked - a session that came up on a sad tab asks once the tab
    /// is back.
    zoom_base: Option<protocol::Window>,
    /// The id of that question, sent from the serve loop, whose answer
    /// applies the zoom.
    zoom_probe: Option<u64>,
}

struct Driver {
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
    /// The zoom has been reported once: every later session re-applies it
    /// quietly.
    zoom_told: bool,
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

        let mut link = Link {
            ws,
            next_id: 0,
            main_frame: None,
            zoom_base: None,
            zoom_probe: None,
        };
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

        if self.config.zoom != 100 {
            self.apply_zoom(link).await;
        }

        // Runtime.enable is deliberately not sent: Runtime.evaluate works
        // without it, and enabling it only puts console noise on the wire.
        Ok(())
    }

    /// `browser.zoom`, as an emulation override on this session. The override
    /// belongs to the DevTools session - Chromium drops it when the client
    /// goes - so every session applies it again, and the window the page
    /// reports here is always its own, never a previous zoom. A page that
    /// will not be zoomed stays at 100% rather than costing the session.
    async fn apply_zoom(&mut self, link: &mut Link) {
        let asked = self
            .rpc(link, "Runtime.evaluate", protocol::window_query())
            .await;
        match self.zoom_from(link, asked) {
            Ok(params) => {
                let applied = self
                    .rpc(link, "Emulation.setDeviceMetricsOverride", params.clone())
                    .await;
                match applied {
                    Ok(_) => self.tell_zoom(link, &params),
                    Err(err) => self.log.info(format!("page zoom not applied: {err}")),
                }
            }
            Err(err) => self.log.info(format!("page zoom not applied: {err}")),
        }
    }

    /// The override for the page's answer to `window_query`, remembering the
    /// window for a later crash.
    fn zoom_from(&self, link: &mut Link, answer: Reply) -> Result<Value, String> {
        let result = answer.map_err(|err| format!("the page's window: {err}"))?;
        let window = protocol::window(&result)
            .ok_or_else(|| format!("the page reported no usable window ({result})"))?;
        link.zoom_base = Some(window);
        Ok(protocol::zoom_override(&window, self.config.zoom))
    }

    fn tell_zoom(&mut self, link: &Link, params: &Value) {
        let Some(window) = link.zoom_base else {
            return;
        };
        let line = format!(
            "page zoom {}% (viewport {}x{} at scale {} -> {}x{} at scale {:.3})",
            self.config.zoom,
            window.width,
            window.height,
            window.ratio,
            params["width"],
            params["height"],
            params["deviceScaleFactor"].as_f64().unwrap_or_default(),
        );
        if self.zoom_told {
            self.log.debug(line);
        } else {
            self.zoom_told = true;
            self.log.info(line);
        }
    }

    /// The tab is back after a crash: put the zoom back on it, or - when the
    /// session came up on the sad tab and never learnt the window - ask for
    /// the window first; the answer comes through `serve`.
    async fn rezoom(&self, link: &mut Link) -> Result<(), String> {
        if self.config.zoom == 100 {
            return Ok(());
        }
        match link.zoom_base {
            Some(window) => {
                let params = protocol::zoom_override(&window, self.config.zoom);
                self.fire(link, "Emulation.setDeviceMetricsOverride", params)
                    .await?;
            }
            None => {
                let query = protocol::window_query();
                let id = self.fire(link, "Runtime.evaluate", query).await?;
                link.zoom_probe = Some(id);
            }
        }
        Ok(())
    }

    /// The window arrived for a session that came up on a sad tab.
    async fn zoom_probe_answered(&mut self, link: &mut Link, answer: Reply) -> Result<(), String> {
        match self.zoom_from(link, answer) {
            Ok(params) => {
                self.fire(link, "Emulation.setDeviceMetricsOverride", params.clone())
                    .await?;
                self.tell_zoom(link, &params);
            }
            Err(err) => self.log.info(format!("page zoom not applied: {err}")),
        }
        Ok(())
    }

    /// A command from the serve loop that no caller waits for; its reply is
    /// dropped unless the loop knows the id. `Err` ends the session.
    async fn fire(&self, link: &mut Link, method: &str, params: Value) -> Result<u64, String> {
        link.next_id += 1;
        let text = protocol::request(link.next_id, method, &params);
        let sent = link.ws.send(Message::Text(text.into()));
        match deadline::within("cdp send", self.config.timeout, sent).await {
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
                        Incoming::Event { method, params } => {
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
                    let text = protocol::request(id, envelope.method, &envelope.params);
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
                                if link.zoom_probe == Some(id) {
                                    link.zoom_probe = None;
                                    if let Err(reason) = self.zoom_probe_answered(link, result).await {
                                        return reason;
                                    }
                                } else if let Some((method, reply)) = pending.remove(&id) {
                                    let _ = reply.send(result.map_err(|err| format!("{method}: {err}")));
                                }
                            }
                            Incoming::Event { method, params } => {
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
                                    // resumes, and put the zoom back. Fire
                                    // and forget: no caller.
                                    PageEvent::Reloaded => {
                                        if let Err(reason) = self.fire(link, "Page.enable", json!({})).await {
                                            return reason;
                                        }
                                        if let Err(reason) = self.rezoom(link).await {
                                            return reason;
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

    #[derive(Clone, Copy, Debug)]
    enum Control {
        /// Drop the connection; the page itself is untouched.
        Drop,
        /// Report the page crashed, as Chromium does before a sad tab.
        Crash,
    }

    /// Just enough Chromium: `/json/list` on one port, a page target's
    /// websocket on another, and ways to lose the connection, restart the
    /// browser (a new target id) or crash the page.
    struct FakeChromium {
        base_url: String,
        control: broadcast::Sender<Control>,
        page: Arc<std::sync::atomic::AtomicU32>,
        /// Every command received, sad tab or not, in order.
        received: Arc<std::sync::Mutex<Vec<(String, Value)>>>,
    }

    impl FakeChromium {
        async fn start() -> Self {
            let ws = TcpListener::bind("127.0.0.1:0").await.expect("bind ws");
            let ws_port = ws.local_addr().unwrap().port();
            let http = TcpListener::bind("127.0.0.1:0").await.expect("bind http");
            let http_port = http.local_addr().unwrap().port();
            let (control, _) = broadcast::channel(4);
            let page = Arc::new(std::sync::atomic::AtomicU32::new(1));
            let received = Arc::new(std::sync::Mutex::new(Vec::new()));
            let log = Arc::clone(&received);

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
            tokio::spawn(async move {
                while let Ok((socket, _)) = ws.accept().await {
                    let mut orders = controls.subscribe();
                    let sad = Arc::clone(&crashed);
                    let log = Arc::clone(&log);
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
                                    log.lock().unwrap().push((method.to_string(), request["params"].clone()));

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
                                        "Runtime.evaluate" if request["params"] == protocol::window_query() => {
                                            json!({ "result": { "type": "object", "value": [2, 800, 600] } })
                                        }
                                        "Runtime.evaluate" => json!({ "result": { "type": "number", "value": 2 } }),
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

        /// The methods received so far, in order.
        fn methods(&self) -> Vec<String> {
            let received = self.received.lock().unwrap();
            received.iter().map(|(method, _)| method.clone()).collect()
        }

        /// The viewport width of every zoom override received.
        fn zooms(&self) -> Vec<Value> {
            let received = self.received.lock().unwrap();
            received
                .iter()
                .filter(|(method, _)| method == "Emulation.setDeviceMetricsOverride")
                .map(|(_, params)| params["width"].clone())
                .collect()
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
            zoom: 100,
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
        session_zoomed(chromium, 100).await
    }

    async fn session_zoomed(chromium: &FakeChromium, zoom: u16) -> SessionHandle {
        let (stop, shutdown) = watch::channel(false);
        // Leaked on purpose: a dropped sender reads as shutdown.
        std::mem::forget(stop);
        let session = spawn(
            SessionConfig {
                zoom,
                ..config(&chromium.base_url)
            },
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
    async fn every_session_puts_the_zoom_on_the_page_it_primed() {
        // The fake page is 800x600, so 150% is a 533-pixel-wide viewport.
        let chromium = FakeChromium::start().await;
        let session = session_zoomed(&chromium, 150).await;

        assert_eq!(chromium.zooms(), [json!(533)]);
        let methods = chromium.methods();
        let tree = methods.iter().position(|m| m == "Page.getFrameTree");
        let zoom = methods
            .iter()
            .position(|m| m == "Emulation.setDeviceMetricsOverride");
        assert!(tree < zoom, "the zoom comes after priming: {methods:?}");

        chromium.drop_the_connection();
        reconnected(&session).await;
        assert_eq!(chromium.zooms().len(), 2, "a reconnect is a new session");

        chromium.restart_the_browser();
        reconnected(&session).await;
        assert_eq!(chromium.zooms().len(), 3, "so is a new browser");
    }

    #[tokio::test]
    async fn no_zoom_sends_nothing() {
        let chromium = FakeChromium::start().await;
        let session = session_on(&chromium).await;
        chromium.drop_the_connection();
        reconnected(&session).await;

        let methods = chromium.methods();
        let zoomed = |m: &String| m.starts_with("Emulation.") || m == "Runtime.evaluate";
        assert!(!methods.iter().any(zoomed), "{methods:?}");
    }

    #[tokio::test]
    async fn a_tab_back_from_a_crash_gets_its_zoom_again() {
        let chromium = FakeChromium::start().await;
        let session = session_zoomed(&chromium, 150).await;

        chromium.crash_the_page();
        until("the crash", || session.generation() == 2).await;
        session
            .call(
                &Heartbeat::detached(),
                "Page.navigate",
                json!({ "url": "http://kiosk.test/" }),
                Duration::from_secs(2),
            )
            .await
            .expect("navigate reloads the tab");

        until("the zoom after the reload", || chromium.zooms().len() == 2).await;
        assert_eq!(chromium.zooms()[1], json!(533));
    }

    #[tokio::test]
    async fn a_session_that_came_up_on_a_sad_tab_zooms_once_it_is_back() {
        // Priming a sad tab stops before the zoom, so the base is unknown
        // until the tab reloads; the serve loop asks for it then.
        let chromium = FakeChromium::start().await;
        let session = session_zoomed(&chromium, 150).await;

        chromium.crash_the_page();
        until("the crash", || session.generation() == 2).await;
        chromium.drop_the_connection();
        reconnected(&session).await;
        assert_eq!(chromium.zooms().len(), 1, "nothing to zoom on a sad tab");

        session
            .call(
                &Heartbeat::detached(),
                "Page.navigate",
                json!({ "url": "http://kiosk.test/" }),
                Duration::from_secs(2),
            )
            .await
            .expect("navigate reloads the tab");

        until("the zoom after the reload", || chromium.zooms().len() == 2).await;
        assert_eq!(chromium.zooms()[1], json!(533));
        assert!(session.state().up);
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
}
