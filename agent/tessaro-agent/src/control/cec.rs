//! HDMI-CEC on the running agent (docs/cec.md): a worker per adapter that
//! claims the device's address, answers the TV, follows the bus
//! (`cec::bus`) and hands what happens on it to the page, the remote's keys
//! to the page as key presses, and both to the scripts that run on them.
//!
//! The workers follow `screen.cec.*` as the agent applies it: switching CEC
//! off or renaming the device ends them and starts them again; the other
//! keys they read at every event. `screen power` asks them to wake the TV
//! or put it in standby (`cec::Request`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use protocol::{ScreenShow, TvStatus};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, watch};

use super::Control;
use crate::cec::bus::{self, Bus, Context, Out};
use crate::cec::io::{self, Receiver};
use crate::cec::Request;
use crate::config::{Cec, CecSource};
use crate::deadline::{blocking, within};
use crate::sync::lock;

/// How often a worker looks at the adapter's addresses and at a key held
/// with no release.
const TICK: Duration = Duration::from_secs(1);
/// How often the TV is asked for its power: many TVs do not say when they
/// are switched on.
const ASK_POWER_EVERY: Duration = Duration::from_secs(10);
/// How often the bus is polled for what is on it.
const SCAN_EVERY: Duration = Duration::from_secs(300);
/// How long before adapters are looked for again, when there were none or
/// one went away.
const RETRY: Duration = Duration::from_secs(30);
/// The longest a worker is waited for to give up its address.
const STOP_LIMIT: Duration = Duration::from_secs(25);

impl Control {
    /// `screen show`: the connected displays and the CEC bus.
    pub(super) async fn screen_show(&self) -> Result<ScreenShow, String> {
        let connectors = self.modes().await?;
        let cec = self.current.borrow().config.cec.enable;
        let adapters = lock(&self.cec_adapters).values().cloned().collect();
        Ok(ScreenShow {
            connectors,
            cec,
            adapters,
        })
    }

    /// The TV as the first adapter that has heard from it reports it.
    pub(super) fn tv_status(&self) -> Option<TvStatus> {
        let adapters = lock(&self.cec_adapters);
        let adapter = adapters
            .values()
            .find(|adapter| adapter.tv.is_some())
            .or_else(|| adapters.values().next())?;
        let tv = adapter
            .devices
            .iter()
            .find(|device| device.address == bus::TV);
        if adapter.tv.is_none() && tv.is_none() {
            return None;
        }
        Some(TvStatus {
            power: adapter.tv,
            showing: adapter.active,
            name: tv.and_then(|tv| tv.name.clone()),
        })
    }

    /// Ask every worker; with none listening CEC is off and nothing is sent.
    pub(super) fn cec_request(&self, request: Request) {
        let _ = self.cec_requests.send(request);
    }

    pub fn watch_cec(self: &Arc<Self>) {
        let control = Arc::clone(self);
        // naked: the supervisor's every wait is bounded inside it
        tokio::spawn(async move { control.cec_supervise().await });
    }

    /// Start the workers while `screen.cec.enable` is on, and again with a
    /// new name, after an adapter went away, or when none was there.
    async fn cec_supervise(self: Arc<Self>) {
        let mut follow = self.current.subscribe();
        let mut shutdown = self.shutdown.clone();
        let mut told_none = false;
        loop {
            let settings = follow.borrow_and_update().config.cec.clone();
            let mut retry = false;

            if settings.enable {
                let dev = self.paths.dev.clone();
                let only = self.paths.cec_devices.clone();
                let found = blocking("looking for HDMI-CEC adapters", move || {
                    Ok(io::find(&dev, &only))
                })
                .await
                .unwrap_or_default();

                if found.is_empty() {
                    if !told_none {
                        self.log.info(
                            "cec: screen.cec.enable is on, but there is no HDMI-CEC adapter on a display connector",
                        );
                        told_none = true;
                    }
                    retry = true;
                } else {
                    told_none = false;
                    // naked: the workers' every wait is bounded inside them
                    match self.cec_run(found, &settings, &mut follow).await {
                        Ending::Shutdown => return,
                        Ending::Changed => {}
                        Ending::Gone => retry = true,
                    }
                }
            }

            // naked: the settings, the shutdown signal and a timer
            tokio::select! {
                changed = follow.changed() => if changed.is_err() { return },
                _ = shutdown.changed() => return,
                _ = tokio::time::sleep(RETRY), if retry => {}
            }
        }
    }

    /// Run a worker for each adapter until CEC is switched off or renamed,
    /// the agent stops, or an adapter goes away.
    async fn cec_run(
        self: &Arc<Self>,
        found: Vec<(PathBuf, Option<(u32, u32)>)>,
        settings: &Cec,
        follow: &mut crate::config::Follow,
    ) -> Ending {
        let (stop, stopped) = watch::channel(false);
        let (gone, mut gone_rx) = mpsc::channel::<()>(found.len());
        let mut workers = Vec::new();
        for (path, connector) in found {
            let drm = self.paths.drm.clone();
            let connector = match connector {
                Some((card, id)) => {
                    blocking("naming the HDMI-CEC adapter's connector", move || {
                        Ok(crate::display::connector_name(&drm, card, id))
                    })
                    .await
                    .ok()
                    .flatten()
                }
                None => None,
            };
            let worker = Worker {
                path,
                connector,
                name: settings.name.clone(),
            };
            let control = Arc::clone(self);
            let stop = stopped.clone();
            let requests = self.cec_requests.subscribe();
            let gone = gone.clone();
            workers.push(tokio::spawn(async move {
                // naked: the worker's every wait is bounded inside it
                if let Err(err) = control.cec_worker(&worker, stop, requests).await {
                    control
                        .log
                        .info(format!("cec: {}: {err}", worker.path.display()));
                    lock(&control.cec_adapters).remove(&worker.device());
                    let _ = gone.try_send(());
                }
            }));
        }

        let mut shutdown = self.shutdown.clone();
        let ending = loop {
            // naked: the settings, the shutdown signal and the workers' end
            tokio::select! {
                changed = follow.changed() => {
                    if changed.is_err() {
                        break Ending::Shutdown;
                    }
                    let now = follow.borrow().config.cec.clone();
                    if !now.enable || now.name != settings.name {
                        break Ending::Changed;
                    }
                }
                _ = shutdown.changed() => break Ending::Shutdown,
                _ = gone_rx.recv() => break Ending::Gone,
            }
        };

        let _ = stop.send(true);
        for worker in workers {
            if within("an HDMI-CEC worker stopping", STOP_LIMIT, worker)
                .await
                .is_err()
            {
                self.log.info("cec: a worker did not stop in time");
            }
        }
        lock(&self.cec_adapters).clear();
        if matches!(ending, Ending::Changed) {
            // follow.changed() consumed the change: look at it again.
            follow.mark_changed();
        }
        ending
    }

    /// One adapter: claim, then answer and follow the bus until `stop`.
    async fn cec_worker(
        self: &Arc<Self>,
        worker: &Worker,
        mut stop: watch::Receiver<bool>,
        mut requests: broadcast::Receiver<Request>,
    ) -> Result<(), String> {
        let path = worker.path.clone();
        let tx = blocking("opening the HDMI-CEC adapter", move || {
            io::open(&path).map_err(|err| err.to_string())
        })
        .await?;
        let tx = Arc::new(tx);
        // A device node and a mode: syscalls that answer at once.
        let receiver = Receiver::open(&worker.path).map_err(|err| err.to_string())?;

        let (claiming, name) = (Arc::clone(&tx), worker.name.clone());
        if let Err(err) = blocking("claiming an HDMI-CEC address", move || {
            io::claim(&claiming, &name).map_err(|err| err.to_string())
        })
        .await
        {
            // An adapter whose driver sets the addresses itself keeps them.
            self.log.info(format!(
                "cec: {}: claiming an address: {err}",
                worker.device()
            ));
        }

        let mut bus = Bus::default();
        self.cec_addresses(&tx, &mut bus).await;
        self.log.info(format!(
            "cec: {}{} as {:?}, {}",
            worker.device(),
            worker
                .connector
                .as_ref()
                .map(|connector| format!(" on {connector}"))
                .unwrap_or_default(),
            worker.name,
            describe(&bus)
        ));
        self.cec_publish(worker, &bus);

        // Once a boot: the screen comes on with the device, so the TV does.
        if !self.screen_kept_off().await && self.cec_first_wake().await {
            let source = self.current.borrow().config.cec.source != CecSource::Off;
            let out = bus.wake(source);
            self.cec_handle(worker, &tx, &mut bus, out).await;
            self.log.info("cec: woke the TV for this boot");
        }
        self.cec_scan(worker, &tx, &mut bus).await;

        let mut tick = tokio::time::interval(TICK);
        let mut asked = Instant::now();
        let mut scanned = Instant::now();
        loop {
            // naked: the bus, the requests, a timer and the stop signal
            tokio::select! {
                received = receiver.receive() => {
                    let messages = received.map_err(|err| format!("the adapter went away: {err}"))?;
                    for msg in messages {
                        let context = self.cec_context().await;
                        let news = msg.opcode == Some(bus::op::REPORT_PHYSICAL_ADDR)
                            && !bus.devices.contains_key(&msg.from);
                        let mut out = bus.received(&msg, context);
                        if news {
                            out.extend(bus.ask_about(msg.from));
                        }
                        self.cec_handle(worker, &tx, &mut bus, out).await;
                    }
                }
                request = requests.recv() => {
                    let out = match request {
                        Ok(Request::Wake) => {
                            let source = self.current.borrow().config.cec.source != CecSource::Off;
                            bus.wake(source)
                        }
                        Ok(Request::Standby) => bus.standby(),
                        Err(broadcast::error::RecvError::Lagged(_)) => Vec::new(),
                        Err(broadcast::error::RecvError::Closed) => return Ok(()),
                    };
                    self.cec_handle(worker, &tx, &mut bus, out).await;
                }
                _ = tick.tick() => {
                    let changed = receiver.changes().map_err(|err| format!("the adapter went away: {err}"))?;
                    if changed.is_some() {
                        let had = bus.logical;
                        self.cec_addresses(&tx, &mut bus).await;
                        self.log.info(format!("cec: {} now {}", worker.device(), describe(&bus)));
                        if had.is_none() && bus.logical.is_some() {
                            self.cec_scan(worker, &tx, &mut bus).await;
                            scanned = Instant::now();
                        }
                    }
                    let out = bus.release_late(Instant::now());
                    self.cec_handle(worker, &tx, &mut bus, out).await;
                    if asked.elapsed() >= ASK_POWER_EVERY {
                        asked = Instant::now();
                        let out = bus.ask_power();
                        self.cec_handle(worker, &tx, &mut bus, out).await;
                    }
                    if scanned.elapsed() >= SCAN_EVERY {
                        scanned = Instant::now();
                        self.cec_scan(worker, &tx, &mut bus).await;
                    }
                }
                _ = stop.changed() => {
                    let releasing = Arc::clone(&tx);
                    let _ = blocking("giving up the HDMI-CEC address", move || {
                        io::release(&releasing).map_err(|err| err.to_string())
                    })
                    .await;
                    lock(&self.cec_adapters).remove(&worker.device());
                    return Ok(());
                }
            }
            self.cec_publish(worker, &bus);
        }
    }

    /// The adapter's addresses as the kernel has them now.
    async fn cec_addresses(&self, tx: &Arc<std::fs::File>, bus: &mut Bus) {
        let reading = Arc::clone(tx);
        if let Ok((physical, logical)) = blocking("reading the HDMI-CEC addresses", move || {
            io::addresses(&reading).map_err(|err| err.to_string())
        })
        .await
        {
            bus.addresses(physical, logical);
        }
    }

    /// Poll the bus and ask whoever answers what they are.
    async fn cec_scan(&self, worker: &Worker, tx: &Arc<std::fs::File>, bus: &mut Bus) {
        let Some(from) = bus.logical else {
            return;
        };
        let polling = Arc::clone(tx);
        let Ok(answered) = blocking("polling the HDMI-CEC bus", move || {
            Ok(io::poll_all(&polling, from))
        })
        .await
        else {
            return;
        };
        bus.present(&answered);
        let out: Vec<Out> = answered
            .iter()
            .flat_map(|address| bus.ask_about(*address))
            .collect();
        self.cec_handle(worker, tx, bus, out).await;
    }

    /// Send what the bus asks, and hand its events on.
    async fn cec_handle(
        &self,
        worker: &Worker,
        tx: &Arc<std::fs::File>,
        bus: &mut Bus,
        out: Vec<Out>,
    ) {
        for out in out {
            match out {
                Out::Send(msg) => {
                    let sending = Arc::clone(tx);
                    let what = format!("{msg:?}");
                    if let Err(err) = blocking("sending on the HDMI-CEC bus", move || {
                        io::transmit(&sending, &msg).map_err(|err| err.to_string())
                    })
                    .await
                    {
                        self.log
                            .debug(format!("cec: {}: sending {what}: {err}", worker.device()));
                    }
                }
                Out::Event(event) => self.cec_event(worker, bus, &event).await,
            }
        }
    }

    /// One event: the journal, the page, the remote's key, the scripts.
    async fn cec_event(&self, worker: &Worker, bus: &Bus, event: &bus::Event) {
        let settings = self.current.borrow().config.cec.clone();
        let key = event.key.as_ref().map(|(name, _)| name.as_str());
        match key {
            Some(key) => self.log.debug(format!(
                "cec: key {key} {}{}",
                if event.pressed { "pressed" } else { "released" },
                if event.repeat { " (held)" } else { "" }
            )),
            None => self.log.info(format!(
                "cec: {}",
                match event.name {
                    "tv-on" => "the TV switched on",
                    "tv-standby" => "the TV went to standby",
                    "source-gained" => "the TV shows this device",
                    _ => "the TV switched to another input",
                }
            )),
        }

        if settings.page {
            let mut detail = json!({
                "event": event.name,
                "connector": worker.connector,
                "tv": bus.tv.map(|power| power.name()),
                "showing": bus.active,
            });
            if let Some(key) = key {
                detail["key"] = json!(key);
                detail["pressed"] = json!(event.pressed);
                detail["repeat"] = json!(event.repeat);
            }
            self.bridge_cec(detail).await;
        }

        if let (true, Some((_, code))) = (settings.keys, &event.key) {
            if let Some(press) = protocol::cec::key(*code).and_then(|key| key.press.as_ref()) {
                if let Err(err) = self.dispatch_key(press, event.pressed, event.repeat).await {
                    self.log.debug(format!("cec: key {}: {err}", press.key));
                }
            }
        }

        // A key runs a script when it goes down, not again while held.
        let fresh = key.is_none() || (event.pressed && !event.repeat);
        if settings.scripts && fresh {
            self.cec_scripts(event.name, key).await;
        }
    }

    /// The screen is meant to be on, `screen.cec.source` is `always`.
    async fn cec_context(&self) -> Context {
        let keep_source = self.current.borrow().config.cec.source == CecSource::Always;
        Context {
            screen_on: !self.screen_kept_off().await,
            keep_source,
            now: Instant::now(),
        }
    }

    /// Whether this is the first wake of the boot, which is then marked.
    async fn cec_first_wake(&self) -> bool {
        let marker = self.paths.cec_woke_file();
        blocking("keeping the boot's TV wake", move || {
            if marker.exists() {
                return Ok(false);
            }
            std::fs::write(&marker, b"woke\n")
                .map_err(|err| format!("{}: {err}", marker.display()))?;
            Ok(true)
        })
        .await
        .unwrap_or(false)
    }

    fn cec_publish(&self, worker: &Worker, bus: &Bus) {
        let snapshot = bus.snapshot(&worker.device(), worker.connector.clone(), &worker.name);
        lock(&self.cec_adapters).insert(worker.device(), snapshot);
    }
}

/// Why the workers ended.
enum Ending {
    Shutdown,
    /// CEC was switched off, or the device renamed.
    Changed,
    /// An adapter went away.
    Gone,
}

/// One adapter and what it is called.
struct Worker {
    path: PathBuf,
    connector: Option<String>,
    name: String,
}

impl Worker {
    fn device(&self) -> String {
        self.path.display().to_string()
    }
}

/// `logical 4, physical 1.0.0.0`, or why there is no address.
fn describe(bus: &Bus) -> String {
    match (bus.logical, bus.physical) {
        (Some(logical), Some(physical)) => {
            format!(
                "logical address {logical}, physical {}",
                bus::physical(physical)
            )
        }
        (None, None) => {
            "no address: the TV gives none (switched off, or no hot-plug in standby)".to_string()
        }
        (logical, physical) => format!(
            "logical address {}, physical {}",
            logical.map_or("none".to_string(), |logical| logical.to_string()),
            physical.map_or("none".to_string(), bus::physical)
        ),
    }
}
