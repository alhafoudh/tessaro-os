//! The page bridge: `window.tessaro` and the injected script.
//!
//! The agent registers two sources with the DevTools session
//! (`cdp::session::PageScripts`), run in every document before the page's
//! own scripts: the preamble (`bridge.js`, with the settings baked in) and
//! `browser.inject.script` from the file store. In `config` and `actions`
//! mode the page also gets a binding; each call through it arrives here as a
//! `BindingCall`, is checked against where the browser says it came from,
//! runs as the matching control-plane command with `Caller::Page`, and is
//! answered by settling the page's Promise with `Runtime.evaluate` in the
//! calling context.
//!
//! The browser gets no path to the control plane: the agent decides every
//! call, and only the kiosk's own origin, in the top frame, is answered.
//! See docs/bridge.md.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::keys::{self, Placeholder};
use protocol::{BridgeStatus, Command, RestartTarget};
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch, Notify};
use tokio::time::Instant;

use super::{After, Caller, Control, Stream, CDP_LIMIT};
use crate::cdp::session::{BindingCall, PageScripts, BINDING};
use crate::config::BridgeMode;
use crate::deadline::{blocking, within};
use crate::state;
use crate::sync::lock;
use crate::watchdog::Heartbeat;

const PREAMBLE: &str = include_str!("bridge.js");

/// How often the settings are looked at for a change nobody announced: an
/// address from DHCP, free space.
const REFRESH_EVERY: Duration = Duration::from_secs(15);

/// A reload, a restart, a reboot or anything else that starts the page over
/// is refused within this long of the agent's start and of the last one, so
/// a page that does it on load cannot loop.
const DISRUPT_GAP: Duration = Duration::from_secs(60);

/// A speed test moves real data over a link that may be metered.
const SPEEDTEST_GAP: Duration = Duration::from_secs(600);

/// A public address this fresh is answered without asking again.
const PUBLIC_IP_FRESH: Duration = Duration::from_secs(30);

/// The longest a streamed action - a ping, a speed test - is waited for.
const STREAM_LIMIT: Duration = Duration::from_secs(300);

/// The longest message `log` writes to the journal.
const LOG_MAX: usize = 2000;

/// Left out of `tessaro.config`: who may manage the device and how to reach
/// it, the device's name (and the hotspot named after it), and the public
/// address, which costs a request and is `network.publicIp()` instead.
const HIDDEN: &[&str] = &[
    "access.",
    keys::NAME,
    "network.wifi.hotspot_ssid",
    keys::PUBLIC_IP,
];

/// The calls `config` mode answers; `actions` answers every call.
const READS: &[&str] = &["log", "device.status", "network.status", "audio.status"];

/// What `main` hands over: the mode and script this agent started with, the
/// session's end of the page scripts, and the page's calls.
pub struct BridgeSetup {
    pub mode: BridgeMode,
    pub script: String,
    pub scripts: watch::Sender<PageScripts>,
    pub calls: mpsc::Receiver<BindingCall>,
}

pub(super) struct Bridge {
    mode: BridgeMode,
    script: String,
    /// The page's hidden handle, new with every agent start.
    settle: String,
    /// The origins answered: the page the agent drives, and browser.url's.
    origins: Vec<String>,
    scripts: watch::Sender<PageScripts>,
    /// A setting changed: look at the snapshot now.
    poke: Notify,
    problem: Mutex<Option<String>>,
    last_disrupt: Mutex<Instant>,
    last_speedtest: Mutex<Option<Instant>>,
    /// Held across a lookup, so calls at the same time share one request.
    public_ip: tokio::sync::Mutex<Option<(Instant, String)>>,
}

/// A call's answer: the value, or what the page's Promise rejects with.
type Answer = Result<Value, Value>;

fn fail(message: impl Into<String>) -> Value {
    json!({ "message": message.into() })
}

impl Control {
    /// Start the bridge, with the scripts in place before this returns so
    /// the agent's first navigation already runs them.
    pub async fn start_bridge(self: &Arc<Self>, setup: BridgeSetup) {
        let BridgeSetup {
            mode,
            script,
            scripts,
            mut calls,
        } = setup;

        let mut origins = Vec::new();
        if let Ok(state) = self.read_state().await {
            let url = self.template(&state.settings, keys::URL);
            let live = self.live().await;
            let (expanded, _) = state::expand_url(&url, &state.settings, &self.defaults, &live);
            origins.extend(origin_of(&expanded));
        }
        origins.extend(origin_of(&self.agent_url));
        origins.dedup();

        let bridge = Arc::new(Bridge {
            mode,
            script,
            settle: settle_name(),
            origins,
            scripts,
            poke: Notify::new(),
            problem: Mutex::new(None),
            last_disrupt: Mutex::new(Instant::now()),
            last_speedtest: Mutex::new(None),
            public_ip: tokio::sync::Mutex::new(None),
        });
        let _ = self.bridge.set(Arc::clone(&bridge));
        if mode == BridgeMode::Off && bridge.script.is_empty() {
            return;
        }

        let mut shown = Shown::default();
        // naked: blocking() reads and the session's own within()
        self.refresh_bridge(&bridge, &mut shown).await;
        self.log.info(format!(
            "page bridge: {}{}",
            mode.name(),
            if bridge.script.is_empty() {
                String::new()
            } else {
                format!(", injecting {}", bridge.script)
            }
        ));

        let control = Arc::clone(self);
        let mut shutdown = self.shutdown.clone();
        let mut files = self.files.changes();
        tokio::spawn(async move {
            loop {
                // naked: channels, a timer and the shutdown signal, not the outside world
                tokio::select! {
                    _ = shutdown.changed() => return,
                    call = calls.recv() => {
                        let Some(call) = call else { return };
                        let control = Arc::clone(&control);
                        let bridge = Arc::clone(&bridge);
                        // naked: page_call's every wait is bounded; see there
                        tokio::spawn(async move { control.page_call(&bridge, call).await });
                        continue;
                    }
                    _ = bridge.poke.notified() => {}
                    changed = files.changed() => {
                        if changed.is_err() || bridge.script.is_empty() {
                            continue;
                        }
                    }
                    _ = tokio::time::sleep(REFRESH_EVERY) => {}
                }
                // naked: blocking() reads and the session's own within()
                control.refresh_bridge(&bridge, &mut shown).await;
            }
        });
    }

    /// A setting was changed: the snapshot is worth a look now rather than
    /// at the next tick.
    pub(super) fn poke_bridge(&self) {
        if let Some(bridge) = self.bridge.get() {
            bridge.poke.notify_one();
        }
    }

    pub(super) fn bridge_status(&self) -> Option<BridgeStatus> {
        let bridge = self.bridge.get()?;
        Some(BridgeStatus {
            mode: bridge.mode.name().to_string(),
            script: bridge.script.clone(),
            script_problem: lock(&bridge.problem).clone(),
        })
    }

    /// Build the snapshot and the script again and hand the session what
    /// changed. A changed script reloads the page so it runs now; changed
    /// settings are pushed into the page as they are, with no reload.
    async fn refresh_bridge(&self, bridge: &Bridge, shown: &mut Shown) {
        let snapshot = if bridge.mode >= BridgeMode::Config {
            // naked: blocking() reads
            self.bridge_snapshot().await
        } else {
            BTreeMap::new()
        };

        let source = if bridge.script.is_empty() {
            None
        } else {
            // naked: Files waits only through blocking()
            match self
                .files
                .read_whole(&bridge.script, protocol::EVAL_MAX as u64)
                .await
            {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(text) => Some(text),
                    Err(_) => {
                        self.bridge_problem(
                            bridge,
                            Some(format!("{} is not UTF-8 text", bridge.script)),
                        );
                        None
                    }
                },
                Err(err) => {
                    self.bridge_problem(bridge, Some(err));
                    None
                }
            }
        };
        if source.is_some() {
            self.bridge_problem(bridge, None);
        }

        let first = !shown.started;
        let script_changed = !first && source != shown.source;
        let changed_keys: Vec<&String> = snapshot
            .iter()
            .filter(|(name, value)| shown.snapshot.get(*name) != Some(*value))
            .map(|(name, _)| name)
            .chain(
                shown
                    .snapshot
                    .keys()
                    .filter(|name| !snapshot.contains_key(*name)),
            )
            .collect();

        let mut sources = Vec::new();
        if bridge.mode >= BridgeMode::Config {
            sources.push(preamble(bridge, &snapshot));
        }
        sources.extend(source.clone());
        let binding = bridge.mode >= BridgeMode::Config;
        let rebind =
            binding.then(|| format!("window[{0}] && window[{0}].rebind()", json!(bridge.settle)));
        bridge.scripts.send_if_modified(|scripts| {
            let before = scripts.clone();
            scripts.binding = binding;
            scripts.sources = sources;
            scripts.rebind = rebind;
            if script_changed {
                scripts.reload += 1;
            }
            *scripts != before
        });
        if script_changed {
            self.log.info(format!(
                "page bridge: {} changed; reloading the page",
                bridge.script
            ));
        }

        if !first && !script_changed && !changed_keys.is_empty() && binding {
            let expression = format!(
                "window[{0}] && window[{0}].config({1}, {2})",
                json!(bridge.settle),
                json!(snapshot),
                json!(changed_keys)
            );
            // Nobody to tell if it fails: the page reloads with the new
            // snapshot in the preamble anyway.
            let _ = self
                .session
                .call(
                    &Heartbeat::detached(),
                    "Runtime.evaluate",
                    json!({ "expression": expression }),
                    CDP_LIMIT,
                )
                .await; // naked: SessionHandle::call bounds itself with within()
        }

        shown.started = true;
        shown.snapshot = snapshot;
        shown.source = source;
    }

    fn bridge_problem(&self, bridge: &Bridge, problem: Option<String>) {
        let mut current = lock(&bridge.problem);
        if *current != problem {
            if let Some(problem) = &problem {
                self.log
                    .info(format!("page bridge: not injecting: {problem}"));
            }
            *current = problem;
        }
    }

    /// Every key a template may use, with the value it would expand to,
    /// less `HIDDEN`: what `tessaro.config` holds.
    async fn bridge_snapshot(&self) -> BTreeMap<String, String> {
        let Ok(state) = self.read_state().await else {
            return BTreeMap::new();
        };
        let live = self.live().await;
        let hidden = |name: &str| {
            HIDDEN.iter().any(|hidden| {
                name == *hidden || (hidden.ends_with('.') && name.starts_with(hidden))
            })
        };
        keys::KEYS
            .iter()
            .filter(|key| self.paths.offers(key))
            .map(|key| key.name.to_string())
            .chain(
                state
                    .settings
                    .keys()
                    .filter(|name| keys::param_name(name).is_some())
                    .cloned(),
            )
            .filter(|name| {
                matches!(
                    keys::placeholder(name),
                    Placeholder::Key(_) | Placeholder::Param(_)
                )
            })
            .filter(|name| !hidden(name))
            .filter_map(|name| {
                let value = state::resolve(&name, &state.settings, &self.defaults, &live)?;
                Some((name, value))
            })
            .collect()
    }

    /// One call from the page, answered in the context it came from.
    async fn page_call(self: &Arc<Self>, bridge: &Bridge, call: BindingCall) {
        if !call.top || !bridge.origins.contains(&call.origin) {
            self.log.debug(format!(
                "page bridge: ignored a call from {:?} (top frame: {})",
                call.origin, call.top
            ));
            return;
        }
        let Ok(request) = serde_json::from_str::<Value>(&call.payload) else {
            return;
        };
        let Some(id) = request["id"].as_u64() else {
            return;
        };
        let name = request["name"].as_str().unwrap_or("");
        let args = request["args"].as_array().cloned().unwrap_or_default();

        let (answer, after) = if bridge.mode < BridgeMode::Actions && !READS.contains(&name) {
            (
                Err(fail(format!("{name} needs browser.bridge.mode actions"))),
                None,
            )
        } else {
            // naked: every action is a bounded control-plane call; see page_action
            self.page_action(bridge, name, &args).await
        };
        if let Err(refused) = &answer {
            self.log.debug(format!("page bridge: {name}: {refused}"));
        }

        let (ok, value) = match answer {
            Ok(value) => (true, value),
            Err(value) => (false, value),
        };
        let expression = format!(
            "window[{0}] && window[{0}].settle({id}, {ok}, {1})",
            json!(bridge.settle),
            value
        );
        let settled = self
            .session
            .call(
                &Heartbeat::detached(),
                "Runtime.evaluate",
                json!({ "expression": expression, "contextId": call.context }),
                CDP_LIMIT,
            )
            .await; // naked: SessionHandle::call bounds itself with within()
        if let Err(err) = settled {
            self.log
                .debug(format!("page bridge: could not answer {name}: {err}"));
        }

        // A restart or a reboot waits for the answer, like the server's.
        if let Some(after) = after {
            // naked: run_after waits only through Bus and Network, whose calls are within()
            self.run_after(after).await;
        }
    }

    /// What one call does. Each maps onto the command `tessaro-ctl` sends
    /// for the same thing, so the page can do nothing an operator's command
    /// could not, and says so in the journal as `page`.
    async fn page_action(
        self: &Arc<Self>,
        bridge: &Bridge,
        name: &str,
        args: &[Value],
    ) -> (Answer, Option<After>) {
        let caller = Caller::Page;
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Value::Null);
        let plain = |outcome: Result<Value, String>| (outcome.map_err(fail), None);

        match name {
            "log" => {
                let level = arg(0);
                let message = match arg(1) {
                    Value::String(text) => text,
                    other => other.to_string(),
                };
                let message: String = message
                    .chars()
                    .filter(|ch| !ch.is_control())
                    .take(LOG_MAX)
                    .collect();
                match level.as_str() {
                    Some("debug") => self.log.debug(format!("page: {message}")),
                    Some("warn") | Some("error") | Some("info") => self.log.info(format!(
                        "page ({}): {message}",
                        level.as_str().unwrap_or("info")
                    )),
                    _ => {
                        return plain(Err(
                            "log takes a level (debug, info, warn, error) and a message"
                                .to_string(),
                        ))
                    }
                }
                (Ok(Value::Null), None)
            }
            "device.status" => plain(self.status().await.map(|status| page_status(&status))),
            "network.status" => {
                let paths = self.paths.clone();
                plain(
                    blocking("reading the network", move || {
                        let mut net = crate::net::snapshot(&paths);
                        net.public_ip = None;
                        serde_json::to_value(net).map_err(|err| err.to_string())
                    })
                    .await,
                )
            }
            "audio.status" => plain(self.audio_status().await.and_then(to_value)),
            "network.publicIp" => (self.page_public_ip(bridge).await, None),
            // The same lookup and cache as publicIp, as a plain yes or no: a
            // device that cannot reach the internet resolves `false`, it does
            // not reject.
            "network.online" => (Ok(json!(self.page_public_ip(bridge).await.is_ok())), None),
            "browser.reload" => match disrupt(bridge) {
                Err(refused) => (Err(refused), None),
                Ok(()) => plain(self.reload().await.and_then(to_value)),
            },
            "browser.restart" => match disrupt(bridge) {
                Err(refused) => (Err(refused), None),
                Ok(()) => self.reply(self.restart(RestartTarget::Browser).await),
            },
            "browser.home" => match disrupt(bridge) {
                Err(refused) => (Err(refused), None),
                Ok(()) => plain(self.navigate(&self.agent_url).await.and_then(to_value)),
            },
            "browser.clearCache" => plain(self.clear_cache().await.and_then(to_value)),
            "browser.maintenance" => {
                let Some(on) = arg(0).as_bool() else {
                    return plain(Err("maintenance takes true or false, and a URL".to_string()));
                };
                let mut values = BTreeMap::new();
                values.insert(
                    keys::MAINTENANCE_ENABLE.to_string(),
                    Some(if on { "1" } else { "0" }.to_string()),
                );
                if let Some(url) = arg(1).as_str() {
                    values.insert(keys::MAINTENANCE_URL.to_string(), Some(url.to_string()));
                }
                match disrupt(bridge) {
                    Err(refused) => (Err(refused), None),
                    Ok(()) => self.reply(
                        self.change(&caller, values, None, true, Default::default(), None)
                            .await,
                    ),
                }
            }
            "device.reboot" => match disrupt(bridge) {
                Err(refused) => (Err(refused), None),
                Ok(()) => self.reply(self.handle(&caller, Command::Reboot).await),
            },
            "audio.volume" => {
                let Some(percent) = arg(0).as_u64() else {
                    return plain(Err("volume takes a whole number, 0 to 100".to_string()));
                };
                self.set_one(&caller, keys::AUDIO_VOLUME, percent.to_string(), true)
                    .await
            }
            "audio.mute" => {
                let Some(on) = arg(0).as_bool() else {
                    return plain(Err("mute takes true or false".to_string()));
                };
                self.set_one(
                    &caller,
                    keys::AUDIO_MUTE,
                    if on { "1" } else { "0" }.to_string(),
                    true,
                )
                .await
            }
            "keyboard.show" => {
                let selector = arg(0);
                plain(
                    self.keyboard(true, selector.as_str())
                        .await
                        .and_then(to_value),
                )
            }
            "keyboard.hide" => plain(self.keyboard(false, None).await.and_then(to_value)),
            "screen.on" | "screen.off" => plain(
                self.screen_power(&caller, Some(name == "screen.on"))
                    .await
                    .and_then(to_value),
            ),
            "network.ping" => {
                let Some(host) = arg(0).as_str().map(str::to_string) else {
                    return plain(Err("ping takes a host".to_string()));
                };
                let command = Command::NetPing {
                    host,
                    count: None,
                    interval_ms: None,
                    timeout_ms: None,
                    interface: None,
                };
                // naked: collect() is under within()
                (self.collect(command).await.map_err(fail), None)
            }
            "network.speedTest" => {
                {
                    let mut last = lock(&bridge.last_speedtest);
                    if let Some(when) = *last {
                        let waited = when.elapsed();
                        if waited < SPEEDTEST_GAP {
                            return plain(Err(format!(
                                "a speed test ran {}s ago; the next one in {}s",
                                waited.as_secs(),
                                (SPEEDTEST_GAP - waited).as_secs()
                            )));
                        }
                    }
                    *last = Some(Instant::now());
                }
                let command = Command::Speedtest {
                    max_size: None,
                    tests: None,
                    direct: false,
                };
                // naked: collect() is under within()
                (self.collect(command).await.map_err(fail), None)
            }
            "files.list" => {
                let path = arg(0).as_str().unwrap_or("").to_string();
                plain(self.files.list(&path, false).await.and_then(to_value))
            }
            "data.set" | "data.unset" => {
                let Some(given) = arg(0).as_str().map(str::to_string) else {
                    return plain(Err(format!(
                        "{name} takes a name, like table or data.table"
                    )));
                };
                let key = if given.starts_with(keys::DATA_PREFIX) {
                    given.to_string()
                } else {
                    format!("{}{given}", keys::DATA_PREFIX)
                };
                if keys::param_name(&key).is_none() {
                    return plain(Err(format!(
                        "{key}: a name is lower-case letters, digits and _, up to 32"
                    )));
                }
                let value = match (name, arg(1)) {
                    ("data.unset", _) => None,
                    (_, Value::String(text)) => Some(text),
                    (_, Value::Number(number)) => Some(number.to_string()),
                    (_, Value::Bool(flag)) => Some(if flag { "1" } else { "0" }.to_string()),
                    _ => {
                        return plain(Err(
                            "data.set takes a text, a number or true/false".to_string()
                        ))
                    }
                };
                // Nothing reads a data.* but the templates and this bridge:
                // one no template uses is saved without restarting anything,
                // so a page can keep its own values without reloading itself.
                let used = self.template_uses(&key).await;
                if used {
                    if let Err(refused) = disrupt(bridge) {
                        return (Err(refused), None);
                    }
                }
                let mut values = BTreeMap::new();
                values.insert(key, value);
                let outcome = self.reply(
                    self.change(&caller, values, None, used, Default::default(), None)
                        .await,
                );
                self.poke_bridge();
                outcome
            }
            _ => plain(Err(format!("tessaro has no {name}"))),
        }
    }

    async fn set_one(
        self: &Arc<Self>,
        caller: &Caller,
        key: &str,
        value: String,
        apply: bool,
    ) -> (Answer, Option<After>) {
        let mut values = BTreeMap::new();
        values.insert(key.to_string(), Some(value));
        self.reply(
            self.change(caller, values, None, apply, Default::default(), None)
                .await,
        )
    }

    fn reply(&self, reply: super::Reply) -> (Answer, Option<After>) {
        (reply.result.map_err(fail), reply.after)
    }

    /// Does any template - browser.url, the maintenance URL, the debug
    /// screen - use `{key}`?
    async fn template_uses(&self, key: &str) -> bool {
        let Ok(state) = self.read_state().await else {
            return true;
        };
        keys::TEMPLATES.iter().any(|(template, _)| {
            keys::placeholders(&self.template(&state.settings, template)).contains(&key)
        })
    }

    /// A streamed command run to its end, every event in order.
    async fn collect(&self, command: Command) -> Result<Value, String> {
        let events = async {
            let mut events = Vec::new();
            match self.stream(&Caller::Page, command)? {
                Stream::Ping { mut steps, .. } => {
                    // naked: bounded by the within() below
                    while let Some(step) = steps.recv().await {
                        events.push(to_value(step?)?);
                    }
                }
                Stream::Speedtest(mut steps) => {
                    // naked: bounded by the within() below
                    while let Some(step) = steps.recv().await {
                        events.push(to_value(step?)?);
                    }
                }
                _ => return Err("not a stream the page may run".to_string()),
            }
            Ok(Value::Array(events))
        };
        match within("the streamed action", STREAM_LIMIT, events).await {
            Ok(result) => result,
            Err(expired) => Err(expired.to_string()),
        }
    }

    /// The public address, asked now - unless it was, moments ago. Calls at
    /// the same time share the one request. A failure says the last address
    /// found, if there is one.
    async fn page_public_ip(&self, bridge: &Bridge) -> Answer {
        // naked: the in-process lock, held only across the bounded lookup below
        let mut cached = bridge.public_ip.lock().await;
        if let Some((when, ip)) = cached.as_ref() {
            if when.elapsed() < PUBLIC_IP_FRESH {
                return Ok(json!(ip));
            }
        }
        // naked: public_ip's every phase is under its own within()
        match crate::net::public_ip(&crate::net::public_ip_client(self.proxy)).await {
            Ok(ip) => {
                let ip = ip.to_string();
                // naked: a file write under blocking()'s within()
                self.store_public_ip(ip.clone()).await;
                *cached = Some((Instant::now(), ip.clone()));
                Ok(json!(ip))
            }
            Err(err) => {
                self.set_online(false);
                let paths = self.paths.clone();
                let last = blocking("reading the public address", move || {
                    Ok(crate::net::cached_public_ip(&paths))
                })
                .await
                .ok()
                .flatten();
                Err(json!({
                    "message": format!("{}: {err}", crate::net::TRACE_URL),
                    "lastKnown": last,
                }))
            }
        }
    }
}

/// What the page shows now, to tell what changed.
#[derive(Default)]
struct Shown {
    started: bool,
    snapshot: BTreeMap<String, String>,
    source: Option<String>,
}

/// The preamble with this agent's values filled in.
fn preamble(bridge: &Bridge, snapshot: &BTreeMap<String, String>) -> String {
    PREAMBLE
        .replace("__MODE__", &json!(bridge.mode.name()).to_string())
        .replace("__SETTLE__", &json!(bridge.settle).to_string())
        .replace("__ORIGINS__", &json!(bridge.origins).to_string())
        .replace("__BINDING__", &json!(BINDING).to_string())
        .replace("__CONFIG__", &json!(snapshot).to_string())
}

/// Refuse what starts the page over within `DISRUPT_GAP` of the agent's
/// start and of the last one; otherwise note that one happens now.
fn disrupt(bridge: &Bridge) -> Result<(), Value> {
    let mut last = lock(&bridge.last_disrupt);
    let waited = last.elapsed();
    if waited < DISRUPT_GAP {
        return Err(fail(format!(
            "refused: the page was started over {}s ago; try again in {}s",
            waited.as_secs(),
            (DISRUPT_GAP - waited).as_secs()
        )));
    }
    *last = Instant::now();
    Ok(())
}

/// `tessaro.device.status()`: what `device status` shows, without the
/// node's name, fingerprint and claim.
fn page_status(status: &protocol::Status) -> Value {
    json!({
        "os": status.os,
        "imageVersion": status.image_version,
        "version": status.node.version,
        "machine": status.node.machine,
        "kioskUrl": status.kiosk_url,
        "currentUrl": status.current_url,
        "maintenance": status.maintenance,
        "debugScreen": status.debug_screen,
        "screenOn": status.screen_on,
        "units": status.units,
        "data": status.data,
        "hardware": status.hardware,
        "memory": status.memory,
        "cpuPercent": status.cpu_percent,
    })
}

fn to_value<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| err.to_string())
}

/// A name the page cannot guess, for the handle only the agent calls.
fn settle_name() -> String {
    let mut bytes = [0u8; 8];
    if openssl::rand::rand_bytes(&mut bytes).is_err() {
        // Not a secret, only unguessable in practice: the time will do.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos() as u64)
            .unwrap_or_default();
        bytes = nanos.to_le_bytes();
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("__tessaro_{hex}")
}

/// `scheme://host[:port]` as the browser writes a context's origin: a
/// default port is left out.
fn origin_of(url: &str) -> Option<String> {
    let origin = crate::url::origin(url)?.to_ascii_lowercase();
    Some(
        origin
            .strip_suffix(":443")
            .filter(|_| origin.starts_with("https://"))
            .or_else(|| {
                origin
                    .strip_suffix(":80")
                    .filter(|_| origin.starts_with("http://"))
            })
            .map(str::to_string)
            .unwrap_or(origin),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_page_gets_the_hardware_but_not_the_node() {
        let fx = crate::control::fixture();
        let status = page_status(&fx.control.status().await.unwrap());
        assert_eq!(status["hardware"]["vendor"], "QEMU");
        assert_eq!(status["memory"]["total"], 4_000_000u64 * 1024);
        assert!(status.get("cpuPercent").is_some());
        for hidden in ["name", "fingerprint", "claimed", "node"] {
            assert!(status.get(hidden).is_none(), "{hidden} reached the page");
        }
    }

    fn bridge(mode: BridgeMode) -> Bridge {
        Bridge {
            mode,
            script: String::new(),
            settle: "__tessaro_test".to_string(),
            origins: vec!["http://kiosk.test".to_string()],
            scripts: watch::channel(PageScripts::default()).0,
            poke: Notify::new(),
            problem: Mutex::new(None),
            last_disrupt: Mutex::new(Instant::now()),
            last_speedtest: Mutex::new(None),
            public_ip: tokio::sync::Mutex::new(None),
        }
    }

    #[test]
    fn origins_are_written_the_way_the_browser_writes_them() {
        assert_eq!(
            origin_of("https://Menu.Test:443/a?b").as_deref(),
            Some("https://menu.test")
        );
        assert_eq!(
            origin_of("http://127.0.0.1/").as_deref(),
            Some("http://127.0.0.1")
        );
        assert_eq!(
            origin_of("http://h:8080/x").as_deref(),
            Some("http://h:8080")
        );
        assert_eq!(origin_of("http://h:443/").as_deref(), Some("http://h:443"));
        assert_eq!(origin_of("data:text/html,x"), None);
    }

    #[test]
    fn the_preamble_gets_every_token_filled() {
        let mut snapshot = BTreeMap::new();
        snapshot.insert("data.table".to_string(), "12".to_string());
        let source = preamble(&bridge(BridgeMode::Actions), &snapshot);
        assert!(!source.contains("__MODE__") && !source.contains("__CONFIG__"));
        assert!(!source.contains("__SETTLE__") && !source.contains("__ORIGINS__"));
        assert!(!source.contains("__BINDING__"));
        assert!(source.contains(r#"{"data.table":"12"}"#));
        assert!(source.contains(r#"const MODE = "actions";"#));
        assert!(source.contains(r#"["http://kiosk.test"]"#));
    }

    #[test]
    fn starting_the_page_over_is_refused_right_after_the_start_and_the_last_time() {
        let fresh = bridge(BridgeMode::Actions);
        assert!(disrupt(&fresh).is_err(), "the agent just started");

        let settled = bridge(BridgeMode::Actions);
        *lock(&settled.last_disrupt) = Instant::now() - DISRUPT_GAP - Duration::from_secs(1);
        assert!(disrupt(&settled).is_ok());
        assert!(disrupt(&settled).is_err(), "twice in a row");
    }

    #[test]
    fn settle_names_differ() {
        assert_ne!(settle_name(), settle_name());
        assert!(settle_name().starts_with("__tessaro_"));
    }

    #[test]
    fn reads_are_what_config_mode_answers() {
        assert!(READS.contains(&"device.status"));
        assert!(!READS.contains(&"network.publicIp"));
        assert!(
            !READS.contains(&"network.online"),
            "a request, like publicIp"
        );
        assert!(!READS.contains(&"browser.reload"));
    }
}
