//! HDMI-CEC on the running agent (docs/cec.md): a worker per adapter that
//! claims the device's address, answers the TV, follows the bus
//! (`cec::bus`) and hands what happens on it to the page, the remote's keys
//! to the page as key presses, and both to the scripts that run on them.
//!
//! The workers follow `screen.cec.*` as the agent applies it: switching CEC
//! off or renaming the device ends them and starts them again; the other
//! keys they read at every event. Each takes jobs (`cec::Job`): `screen
//! power` asks them to wake the TV or put it in standby, `screen cec` for
//! any action, and waits for what each did. Every message either way goes
//! into the message log (`cec::log`), and to a page in bridge `actions`
//! mode as a `message` event.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use protocol::{
    CecActed, CecAction, CecAdapterActed, CecDirection, CecMessages, CecSent, ScreenShow, TvStatus,
};
use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};

use super::Control;
use crate::cec::bus::{self, Bus, Context, Msg, Out};
use crate::cec::io::{self, Receiver};
use crate::cec::{log, Act, Job};
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
/// The longest `screen cec` waits for every adapter to do what it asked:
/// a scan polls the whole bus, a message at a time.
const ACT_LIMIT: Duration = Duration::from_secs(15);
/// How long `send --reply` waits for the answer.
const REPLY_WAIT: Duration = Duration::from_secs(2);
/// The most messages waiting for the page; more are dropped.
const PAGE_QUEUE: usize = 64;

/// A running worker, as `screen power` and `screen cec` reach it.
pub(super) struct Running {
    connector: Option<String>,
    jobs: mpsc::Sender<Job>,
}

/// A `send --reply` waiting for its answer.
struct Waiting {
    from: u8,
    opcode: u8,
    until: Instant,
    acted: CecAdapterActed,
    reply: Option<oneshot::Sender<CecAdapterActed>>,
}

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

    /// The screen was switched: every worker wakes the TV or puts it in
    /// standby, and nobody waits. With none running CEC is off and nothing
    /// is sent.
    pub(super) fn cec_power(&self, on: bool) {
        let act = if on {
            Act::Wake {
                source: self.current.borrow().config.cec.source != CecSource::Off,
            }
        } else {
            Act::Standby { all: false }
        };
        for running in lock(&self.cec_workers).values() {
            let _ = running.jobs.try_send(Job {
                act: act.clone(),
                reply: None,
            });
        }
    }

    /// `screen cec wake|standby|source|key|scan|send`: checked, then done
    /// by every adapter's worker, or the one on `connector`, and answered
    /// with what each did.
    pub(super) async fn cec_act(
        &self,
        action: CecAction,
        connector: Option<String>,
    ) -> Result<CecActed, String> {
        if !self.current.borrow().config.cec.enable {
            return Err(
                "HDMI-CEC is off; `tessaro-ctl config set screen.cec.enable=1` turns it on"
                    .to_string(),
            );
        }
        let name = action.name().to_string();
        let act = match action {
            CecAction::Wake { source } => {
                if self.screen_kept_off().await {
                    return Err(
                        "the screen is off; `tessaro-ctl screen power on` wakes it and the TV"
                            .to_string(),
                    );
                }
                Act::Wake { source }
            }
            CecAction::Standby { all } => Act::Standby { all },
            CecAction::Source => Act::Source,
            CecAction::Key { key, to } => Act::Key {
                code: protocol::cec::key_code(&key)?,
                to: address(to.unwrap_or(protocol::cec::TV))?,
            },
            CecAction::Scan => Act::Scan,
            CecAction::Send { to, data, reply } => Act::Send {
                to: address(to)?,
                data: protocol::cec::parse_data(&data)?,
                reply,
            },
        };

        let mut waiting = Vec::new();
        {
            let workers = lock(&self.cec_workers);
            for running in workers.values().filter(|running| {
                connector.is_none() || running.connector.as_deref() == connector.as_deref()
            }) {
                let (reply, answer) = oneshot::channel();
                running
                    .jobs
                    .try_send(Job {
                        act: act.clone(),
                        reply: Some(reply),
                    })
                    .map_err(|_| "the HDMI-CEC adapter is busy; try again".to_string())?;
                waiting.push(answer);
            }
        }
        if waiting.is_empty() {
            return Err(match connector {
                Some(connector) => format!(
                    "no HDMI-CEC adapter on {connector}; `tessaro-ctl screen show` lists them"
                ),
                None => {
                    "no HDMI-CEC adapter is in use; `tessaro-ctl screen show` says why".to_string()
                }
            });
        }

        let every = async {
            let mut adapters = Vec::new();
            for answer in waiting {
                // naked: bounded by the within() below
                if let Ok(acted) = answer.await {
                    adapters.push(acted);
                }
            }
            adapters
        };
        let answers = within("an HDMI-CEC action", ACT_LIMIT, every)
            .await
            .map_err(|expired| expired.to_string())?;
        Ok(CecActed {
            action: name,
            adapters: answers,
        })
    }

    /// `screen cec messages`: the message log after `after`, each with the
    /// time on the device's clock.
    pub(super) async fn cec_messages(&self, after: u64) -> Result<CecMessages, String> {
        let page = lock(&self.cec_log).page(after);
        blocking("reading the clock", move || {
            let mut page = page;
            for message in &mut page.messages {
                message.time = log::time(message.at_ms);
            }
            Ok(page)
        })
        .await
    }

    pub fn watch_cec(self: &Arc<Self>) {
        let (queue, queued) = mpsc::channel(PAGE_QUEUE);
        if self.cec_page.set(queue).is_ok() {
            let control = Arc::clone(self);
            // naked: bridge_event's every wait is SessionHandle::call's within()
            tokio::spawn(async move { control.bridge_cec_messages(queued).await });
        }
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
            let (jobs, taken) = mpsc::channel(16);
            lock(&self.cec_workers).insert(
                worker.device(),
                Running {
                    connector: worker.connector.clone(),
                    jobs,
                },
            );
            let control = Arc::clone(self);
            let stop = stopped.clone();
            let gone = gone.clone();
            workers.push(tokio::spawn(async move {
                // naked: the worker's every wait is bounded inside it
                let ended = control.cec_worker(&worker, stop, taken).await;
                lock(&control.cec_workers).remove(&worker.device());
                if let Err(err) = ended {
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
        lock(&self.cec_workers).clear();
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
        mut jobs: mpsc::Receiver<Job>,
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
        let mut waiting: Option<Waiting> = None;
        loop {
            // naked: the bus, the jobs, a timer and the stop signal
            tokio::select! {
                received = receiver.receive() => {
                    let messages = received.map_err(|err| format!("the adapter went away: {err}"))?;
                    for msg in messages {
                        let logged = self.cec_logged(worker, CecDirection::In, &msg, None);
                        let answers = waiting
                            .as_ref()
                            .is_some_and(|wait| wait.from == msg.from && msg.opcode == Some(wait.opcode));
                        if let (true, Some(mut wait)) = (answers, waiting.take()) {
                            wait.acted.reply = Some(logged);
                            wait.acted.tv = bus.tv;
                            if let Some(reply) = wait.reply {
                                let _ = reply.send(wait.acted);
                            }
                        }
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
                job = jobs.recv() => {
                    let Some(job) = job else {
                        return Ok(());
                    };
                    self.cec_job(worker, &tx, &mut bus, job, &mut waiting).await;
                }
                _ = tick.tick() => {
                    if waiting.as_ref().is_some_and(|wait| Instant::now() >= wait.until) {
                        if let Some(wait) = waiting.take() {
                            if let Some(reply) = wait.reply {
                                let _ = reply.send(wait.acted);
                            }
                        }
                    }
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

    /// Poll the bus and ask whoever answers what they are: the addresses
    /// that answered.
    async fn cec_scan(&self, worker: &Worker, tx: &Arc<std::fs::File>, bus: &mut Bus) -> Vec<u8> {
        let Some(from) = bus.logical else {
            return Vec::new();
        };
        let polling = Arc::clone(tx);
        let Ok(answered) = blocking("polling the HDMI-CEC bus", move || {
            Ok(io::poll_all(&polling, from))
        })
        .await
        else {
            return Vec::new();
        };
        for address in &answered {
            self.cec_logged(
                worker,
                CecDirection::Out,
                &Msg::poll(from, *address),
                Some(true),
            );
        }
        bus.present(&answered);
        let out: Vec<Out> = answered
            .iter()
            .flat_map(|address| bus.ask_about(*address))
            .collect();
        self.cec_handle(worker, tx, bus, out).await;
        answered
    }

    /// Send what the bus asks, and hand its events on: what was sent, and
    /// why something could not be.
    async fn cec_handle(
        &self,
        worker: &Worker,
        tx: &Arc<std::fs::File>,
        bus: &mut Bus,
        out: Vec<Out>,
    ) -> (Vec<CecSent>, Option<String>) {
        let mut sent = Vec::new();
        let mut failed = None;
        for out in out {
            match out {
                Out::Send(msg) => {
                    let sending = Arc::clone(tx);
                    let message = msg.clone();
                    let result = blocking("sending on the HDMI-CEC bus", move || {
                        io::transmit(&sending, &message).map_err(|err| err.to_string())
                    })
                    .await;
                    let acked = match result {
                        Ok(acked) => acked,
                        Err(err) => {
                            self.log
                                .debug(format!("cec: {}: sending {msg:?}: {err}", worker.device()));
                            failed.get_or_insert(err);
                            false
                        }
                    };
                    let logged = self.cec_logged(worker, CecDirection::Out, &msg, Some(acked));
                    sent.push(CecSent {
                        to: msg.to,
                        data: logged.data,
                        acked,
                    });
                }
                Out::Event(event) => self.cec_event(worker, bus, &event).await,
            }
        }
        (sent, failed)
    }

    /// One job: the act on the bus, and what it did for whoever waits. A
    /// `send` with a reply to wait for is answered later, by the worker's
    /// loop, when the reply comes or `REPLY_WAIT` is up.
    async fn cec_job(
        &self,
        worker: &Worker,
        tx: &Arc<std::fs::File>,
        bus: &mut Bus,
        job: Job,
        waiting: &mut Option<Waiting>,
    ) {
        let Job { act, reply } = job;
        let mut acted = CecAdapterActed {
            device: worker.device(),
            connector: worker.connector.clone(),
            sent: Vec::new(),
            reply: None,
            answered: Vec::new(),
            tv: None,
            error: None,
        };
        let no_address = || {
            "no logical address yet: the TV gives none (switched off, or no hot-plug in standby)"
                .to_string()
        };
        let mut wait_for = None;
        let out = match act {
            Act::Wake { source } => bus.wake(source),
            Act::Standby { all: true } => bus.standby_all(),
            Act::Standby { all: false } => bus.standby(),
            Act::Source if bus.logical.is_none() => {
                acted.error = Some(no_address());
                Vec::new()
            }
            Act::Source => bus.take_source(),
            Act::Key { code, to } => bus.key(code, to),
            Act::Scan if bus.logical.is_none() => {
                acted.error = Some(no_address());
                Vec::new()
            }
            Act::Scan => {
                acted.answered = self.cec_scan(worker, tx, bus).await;
                Vec::new()
            }
            Act::Send { to, data, reply } => {
                wait_for = reply.map(|opcode| (to, opcode));
                bus.raw(to, &data)
            }
        };
        let (sent, failed) = self.cec_handle(worker, tx, bus, out).await;
        acted.sent = sent;
        acted.error = acted.error.or(failed);
        acted.tv = bus.tv;

        match wait_for {
            Some((from, opcode)) if acted.error.is_none() => {
                // One waits at a time: an earlier one is answered as it is.
                if let Some(earlier) = waiting.take() {
                    if let Some(reply) = earlier.reply {
                        let _ = reply.send(earlier.acted);
                    }
                }
                *waiting = Some(Waiting {
                    from,
                    opcode,
                    until: Instant::now() + REPLY_WAIT,
                    acted,
                    reply,
                });
            }
            _ => {
                if let Some(reply) = reply {
                    let _ = reply.send(acted);
                }
            }
        }
    }

    /// Keep a message in the log, and hand it to a page in bridge
    /// `actions` mode as a `message` event, without waiting for the page.
    fn cec_logged(
        &self,
        worker: &Worker,
        direction: CecDirection,
        msg: &Msg,
        acked: Option<bool>,
    ) -> protocol::CecMessage {
        let logged =
            lock(&self.cec_log).push(&worker.device(), direction, msg, acked, log::now_ms());
        if self.current.borrow().config.cec.page {
            let opcode = msg.opcode;
            let detail = json!({
                "event": "message",
                "connector": worker.connector,
                "seq": logged.seq,
                "direction": direction,
                "from": msg.from,
                "to": msg.to,
                "data": logged.data,
                "opcode": opcode,
                "name": opcode.and_then(protocol::cec::opcode_name),
                "acked": acked,
            });
            self.bridge_cec_message(detail);
        }
        logged
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

/// A logical address as `screen cec` is asked for it.
fn address(address: u8) -> Result<u8, String> {
    if address <= protocol::cec::BROADCAST {
        Ok(address)
    } else {
        Err(format!("{address} is not a CEC address: 0 to 15"))
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
