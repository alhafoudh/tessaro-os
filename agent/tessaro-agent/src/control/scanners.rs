//! `tessaro-ctl scanner` and the scanners on the running agent
//! (docs/scanners.md): the `scanners` table, a supervisor that matches it to
//! the devices plugged in, and a worker per scanner that reads it and turns
//! what it sends into scans (`crate::scanner`).
//!
//! The supervisor follows scanner.enable and the table, and looks at sysfs
//! every `POLL`: the agent has no udev monitor. While scanner.enable is on
//! it keeps the udev rule that takes the scanners from libinput and the
//! browser (`scanner::rules`), and runs a worker for each enabled scanner
//! that is plugged in. A worker takes its device for itself - `EVIOCGRAB`
//! on a keyboard, `TIOCEXCL` on a tty - and hands each scan's beginning and
//! end to the page (`tessaro:scanner`, also in the player's frames that ask)
//! and its end to the scripts that run on it, through a queue, so a slow
//! page never holds the scanner up. What was scanned goes nowhere else: the
//! journal and the log (`scanner::log`) get its length and duration only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use protocol::scanner::{
    self as wire, Scan, ScanLog, ScannerChange, ScannerInfo, ScannerList, ScannerSpec, Transport,
};
use protocol::Done;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::{Caller, Control, Stream};
use crate::cec::log::{now_ms, time};
use crate::deadline::{blocking, within};
use crate::scanner::devices::{self, Found, Sysfs};
use crate::scanner::frame::{self, Assembler, Event};
use crate::scanner::keys::{Keyboard, Keymap};
use crate::scanner::log::Entry;
use crate::scanner::{hidpos, io, Scanners};
use crate::sync::lock;

/// How often the devices plugged in are looked at.
const POLL: Duration = Duration::from_secs(2);
/// How long `scanner identify` listens.
const IDENTIFY: Duration = Duration::from_secs(30);
/// How long `scanner test` shows a scanner's scans.
const TEST: Duration = Duration::from_secs(60);
/// `scanner discover` reads sysfs; a margin for its job.
const DISCOVER: Duration = Duration::from_secs(10);
/// The most scans and changes waiting for the page and the scripts; more
/// are dropped, never waited for.
const QUEUE: usize = 64;
/// The longest a worker is waited for to let its device go.
const STOP_LIMIT: Duration = Duration::from_secs(5);
/// One read's worth: a few dozen key events, a HID report, a serial burst.
const READ: usize = 4096;

/// How one scanner is doing, as `scanner list` reports it.
#[derive(Debug, Clone, Default)]
pub(super) struct State {
    state: String,
    node: Option<String>,
    message: Option<String>,
    scans: u64,
    last_scan_ms: Option<i64>,
}

/// One thing for the page and the scripts, in the order it happened.
pub(super) struct Dispatch {
    scanner: String,
    detail: Value,
    /// A scan's bytes, for the scripts that run on it.
    scanned: Option<Vec<u8>>,
}

/// A worker reading one scanner on one device node.
struct Running {
    spec: ScannerSpec,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Control {
    pub(super) async fn scanner_list(&self) -> Result<ScannerList, String> {
        let all = self.read_scanners().await?;
        let enabled = self.current.borrow().config.scanner.enable;
        let states = lock(&self.scanner_states).clone();
        let infos: Vec<ScannerInfo> = all
            .scanners
            .iter()
            .map(|spec| info(spec, states.get(&spec.name), enabled))
            .collect();
        let scanners = blocking("reading the clock", move || {
            Ok(infos
                .into_iter()
                .map(|mut info| {
                    info.last_scan = info.last_scan.take().and_then(|at| {
                        at.parse::<i64>()
                            .ok()
                            .map(time)
                            .filter(|text| !text.is_empty())
                    });
                    info
                })
                .collect())
        })
        .await?;
        Ok(ScannerList { enabled, scanners })
    }

    pub(super) async fn scanner_show(&self, name: &str) -> Result<ScannerInfo, String> {
        let list = self.scanner_list().await?;
        list.scanners
            .into_iter()
            .find(|info| info.spec.name == name)
            .ok_or_else(|| crate::scanner::missing(name))
    }

    pub(super) async fn scanner_create(
        &self,
        caller: &Caller,
        spec: ScannerSpec,
    ) -> Result<ScannerInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let saved = blocking("saving the scanner", move || {
            db.update(|all: &mut Scanners| {
                let others: Vec<&ScannerSpec> = all.scanners.iter().collect();
                let spec = wire::validate(spec, &others)?;
                all.scanners.push(spec.clone());
                Ok(spec)
            })
        })
        .await?;
        self.log.info(format!(
            "scanner {} ({} {}) created by {}",
            saved.name,
            saved.transport.name(),
            saved.device(),
            caller.describe()
        ));
        self.scanner_wake.notify_one();
        self.scanner_settled(&saved.name).await
    }

    pub(super) async fn scanner_set(
        &self,
        caller: &Caller,
        name: String,
        change: ScannerChange,
    ) -> Result<ScannerInfo, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let target = name.clone();
        blocking("saving the scanner", move || {
            db.update(|all: &mut Scanners| {
                let at = all
                    .scanners
                    .iter()
                    .position(|scanner| scanner.name == target)
                    .ok_or_else(|| crate::scanner::missing(&target))?;
                let changed = change.apply(all.scanners[at].clone());
                let others: Vec<&ScannerSpec> = all
                    .scanners
                    .iter()
                    .filter(|other| other.name != target)
                    .collect();
                all.scanners[at] = wire::validate(changed, &others)?;
                Ok(())
            })
        })
        .await?;
        self.log
            .info(format!("scanner {name} changed by {}", caller.describe()));
        self.scanner_wake.notify_one();
        self.scanner_settled(&name).await
    }

    pub(super) async fn scanner_remove(
        &self,
        caller: &Caller,
        name: String,
    ) -> Result<Done, String> {
        let _writes = self.writes.lock().await;
        let db = self.db.clone();
        let target = name.clone();
        blocking("removing the scanner", move || {
            db.update(|all: &mut Scanners| {
                let at = all
                    .scanners
                    .iter()
                    .position(|scanner| scanner.name == target)
                    .ok_or_else(|| crate::scanner::missing(&target))?;
                all.scanners.remove(at);
                Ok(())
            })
        })
        .await?;
        lock(&self.scanner_states).remove(&name);
        self.log
            .info(format!("scanner {name} removed by {}", caller.describe()));
        self.scanner_wake.notify_one();
        Ok(Done::new(format!(
            "removed scanner {name}; its device is a plain keyboard or port again"
        )))
    }

    /// A changed scanner as the supervisor has it a moment later: reading,
    /// missing or failed, rather than how it was before the change.
    async fn scanner_settled(&self, name: &str) -> Result<ScannerInfo, String> {
        let settle = async {
            for _ in 0..10 {
                // naked: a timer, bounded by the within() below
                tokio::time::sleep(Duration::from_millis(200)).await;
                let pending = lock(&self.scanner_states)
                    .get(name)
                    .is_none_or(|state| state.state.is_empty());
                if !pending {
                    break;
                }
            }
        };
        let _ = within("a scanner settling", Duration::from_secs(3), settle).await;
        self.scanner_show(name).await
    }

    pub(super) async fn scanner_logs(&self, after: u64) -> Result<ScanLog, String> {
        let page = lock(&self.scan_log).page(after);
        blocking("reading the clock", move || {
            let mut page = page;
            for entry in &mut page.entries {
                entry.time = time(entry.at_ms);
            }
            Ok(page)
        })
        .await
    }

    /// Every device that may be a scanner, as a job.
    pub(super) fn scanner_discover(&self) -> Stream {
        let (send, steps) = mpsc::channel(64);
        let sysfs = Sysfs::new(&self.paths);
        let db = self.db.clone();
        let log = Arc::clone(&self.log);
        tokio::spawn(async move {
            let found = blocking("looking for scanners", move || {
                let scanners = db.read::<Scanners>(&log);
                Ok(devices::find(&sysfs)
                    .into_iter()
                    .map(|one| {
                        let known = scanners.of(&one).map(|spec| spec.name.clone());
                        one.candidate(known)
                    })
                    .collect::<Vec<_>>())
            })
            .await;
            match found {
                Ok(found) => {
                    for candidate in found {
                        let step = serde_json::to_value(candidate).map_err(|err| err.to_string());
                        // naked: the receiver is the job's drain, itself bounded
                        if send.send(step).await.is_err() {
                            return;
                        }
                    }
                }
                Err(err) => {
                    // naked: the receiver is the job's drain, itself bounded
                    let _ = send.send(Err(err)).await;
                }
            }
        });
        Stream::Scanner {
            steps,
            what: "looking for scanners",
            total: DISCOVER,
        }
    }

    /// Listen to every device that may be a scanner and name the first one
    /// a scan comes from, as a job. A scanner the agent reads already is
    /// heard through its worker; any other device is only listened to, so
    /// a keyboard one types into the page meanwhile, as it always has.
    pub(super) fn scanner_identify(&self, caller: &Caller) -> Result<Stream, String> {
        self.log.info(format!(
            "scanner identify requested by {}",
            caller.describe()
        ));
        let (send, steps) = mpsc::channel(4);
        let (tap, mut tapped) = mpsc::channel(4);
        lock(&self.scan_taps).push((wire::ANY.to_string(), tap));
        let reading: BTreeSet<String> = lock(&self.scanner_states)
            .values()
            .filter(|state| state.state == "reading")
            .filter_map(|state| state.node.clone())
            .collect();
        let sysfs = Sysfs::new(&self.paths);
        let xkb = self.paths.xkb.clone();
        let db = self.db.clone();
        let log = Arc::clone(&self.log);

        tokio::spawn(async move {
            let looked = blocking("looking for scanners", move || {
                Ok((devices::find(&sysfs), db.read::<Scanners>(&log)))
            })
            .await;
            let (found, scanners) = match looked {
                Ok(looked) => looked,
                Err(err) => {
                    // naked: the receiver is the job's drain, itself bounded
                    let _ = send.send(Err(err)).await;
                    return;
                }
            };

            let (stop, stopped) = watch::channel(false);
            let (heard_tx, mut heard) = mpsc::channel(4);
            for one in found
                .iter()
                .filter(|one| !reading.contains(&one.node.display().to_string()))
            {
                let one = one.clone();
                let xkb = xkb.clone();
                let stop = stopped.clone();
                let heard = heard_tx.clone();
                // naked: listen ends with the stop signal sent below
                tokio::spawn(async move { listen(one, xkb, stop, heard).await });
            }
            drop(heard_tx);

            let first = async {
                // naked: the listeners and the workers' taps, bounded by the within() below
                tokio::select! {
                    Some((one, scan)) = heard.recv() => {
                        let known = scanners.of(&one).map(|spec| spec.name.clone());
                        let mut candidate = one.candidate(known);
                        candidate.scan = Some(scan);
                        Some(candidate)
                    }
                    Some(scan) = tapped.recv() => {
                        found.iter().find(|one| {
                            scanners.of(one).is_some_and(|spec| spec.name == scan.scanner)
                        }).map(|one| {
                            let mut candidate = one.candidate(Some(scan.scanner.clone()));
                            candidate.scan = Some(scan);
                            candidate
                        })
                    }
                    else => None,
                }
            };
            let heard = within("listening for a scan", IDENTIFY, first).await;
            let _ = stop.send(true);
            let step = match heard {
                Ok(Some(candidate)) => {
                    serde_json::to_value(candidate).map_err(|err| err.to_string())
                }
                _ => Err(format!(
                    "no scan came in {}s; scan a barcode while `tessaro-ctl scanner identify` \
                     listens, with the scanner plugged in",
                    IDENTIFY.as_secs()
                )),
            };
            // naked: the receiver is the job's drain, itself bounded
            let _ = send.send(step).await;
        });

        Ok(Stream::Scanner {
            steps,
            what: "listening for a scan",
            total: IDENTIFY + Duration::from_secs(10),
        })
    }

    /// The scans of one scanner, with what they say, as a job. Only what
    /// comes while it runs, and nothing of it kept.
    pub(super) async fn scanner_test(
        &self,
        caller: &Caller,
        name: String,
    ) -> Result<Stream, String> {
        let all = self.read_scanners().await?;
        let spec = all.find(&name)?;
        if !self.current.borrow().config.scanner.enable {
            return Err(
                "scanner.enable is off; `tessaro-ctl config set scanner.enable 1` reads the scanners"
                    .to_string(),
            );
        }
        if !spec.enabled {
            return Err(format!(
                "scanner {name} is disabled; `tessaro-ctl scanner enable {name}` reads it"
            ));
        }
        self.log.info(format!(
            "scanner {name}: test requested by {}",
            caller.describe()
        ));
        let (tap, mut tapped) = mpsc::channel::<Scan>(16);
        lock(&self.scan_taps).push((name, tap));
        let (send, steps) = mpsc::channel(16);
        tokio::spawn(async move {
            let follow = async {
                // naked: the worker's scans, bounded by the within() below
                while let Some(scan) = tapped.recv().await {
                    let step = serde_json::to_value(scan).map_err(|err| err.to_string());
                    // naked: the receiver is the job's drain, itself bounded
                    if send.send(step).await.is_err() {
                        return;
                    }
                }
            };
            // The test ends when its time is up: that is not a failure.
            let _ = within("a scanner's test", TEST, follow).await;
        });
        Ok(Stream::Scanner {
            steps,
            what: "testing the scanner",
            total: TEST + Duration::from_secs(5),
        })
    }

    /// A factory reset: every scanner gone, its device given back.
    pub(super) async fn clear_scanners(&self) -> Result<(), String> {
        let db = self.db.clone();
        blocking("removing the scanners", move || db.clear::<Scanners>()).await?;
        lock(&self.scanner_states).clear();
        self.scanner_wake.notify_one();
        Ok(())
    }

    /// The scanners, or why they cannot be read.
    pub(super) async fn read_scanners(&self) -> Result<Scanners, String> {
        let db = self.db.clone();
        blocking("reading the scanners", move || {
            db.transaction(crate::db::load::<Scanners>)
        })
        .await
    }

    pub fn watch_scanners(self: &Arc<Self>) {
        let (queue, mut queued) = mpsc::channel::<Dispatch>(QUEUE);
        if self.scanner_queue.set(queue).is_ok() {
            let control = Arc::clone(self);
            tokio::spawn(async move {
                // naked: the queue only waits for the workers' own scans
                while let Some(dispatch) = queued.recv().await {
                    // naked: the page's and the scripts' every wait is bounded inside
                    control.scanner_dispatch(dispatch).await;
                }
            });
        }
        let control = Arc::clone(self);
        // naked: the supervisor's every wait is bounded inside it
        tokio::spawn(async move { control.scanner_supervise().await });
    }

    /// One scan or change to the page, and a scan to its scripts.
    async fn scanner_dispatch(&self, dispatch: Dispatch) {
        let settings = self.current.borrow().config.scanner;
        if settings.page {
            // naked: bridge_scanner's every wait is SessionHandle's within()
            self.bridge_scanner(dispatch.detail).await;
        }
        if let (true, Some(scanned)) = (settings.scripts, dispatch.scanned) {
            // naked: event_scripts waits only on blocking() and the bus, each bounded
            self.scanner_scripts(&dispatch.scanner, &scanned).await;
        }
    }

    /// Keep a worker on every enabled scanner that is plugged in, while
    /// scanner.enable is on, and the udev rule on the scanners.
    async fn scanner_supervise(self: Arc<Self>) {
        let mut follow = self.current.subscribe();
        let mut shutdown = self.shutdown.clone();
        let mut running: BTreeMap<PathBuf, Running> = BTreeMap::new();
        let mut told: Option<String> = None;
        let sysfs = Sysfs::new(&self.paths);
        self.prune_scans().await;

        loop {
            let enable = follow.borrow_and_update().config.scanner.enable;
            let scanners = match self.read_scanners().await {
                Ok(scanners) => scanners,
                Err(err) => {
                    if told.as_deref() != Some(err.as_str()) {
                        self.log.info(format!("scanners: {err}"));
                        told = Some(err);
                    }
                    Scanners::default()
                }
            };
            let read: Vec<&ScannerSpec> = scanners
                .scanners
                .iter()
                .filter(|spec| enable && spec.enabled)
                .collect();

            let rules = crate::scanner::rules(&read);
            let (path, udevadm) = (self.paths.scanner_rules.clone(), self.paths.udevadm.clone());
            let written = rules.clone();
            let sysfs_rules = sysfs.clone();
            match blocking("writing the scanners' udev rule", move || {
                let ttys = devices::usb_ttys(&sysfs_rules);
                crate::scanner::apply_rules(&path, written.as_deref(), &udevadm, &ttys)
            })
            .await
            {
                Ok(true) => self.scanner_keyboards(rules.as_deref()).await,
                Ok(false) => {}
                Err(err) => self.log.info(format!("scanners: the udev rule: {err}")),
            }

            let sysfs_now = sysfs.clone();
            let found = blocking("looking for scanners", move || {
                Ok(devices::find(&sysfs_now))
            })
            .await
            .unwrap_or_default();
            let wanted: BTreeMap<PathBuf, (&ScannerSpec, &Found)> = found
                .iter()
                .filter_map(|one| {
                    let spec = read.iter().copied().find(|spec| {
                        spec.transport == one.transport
                            && spec.matches(
                                &one.vendor,
                                &one.product,
                                one.serial.as_deref(),
                                &one.port,
                            )
                    })?;
                    Some((one.node.clone(), (spec, one)))
                })
                .collect();

            // Stop what is no longer wanted as it is: unplugged, removed,
            // changed, disabled.
            let stopping: Vec<PathBuf> = running
                .iter()
                .filter(|(node, worker)| {
                    wanted
                        .get(*node)
                        .is_none_or(|(spec, _)| **spec != worker.spec)
                })
                .map(|(node, _)| node.clone())
                .collect();
            for node in stopping {
                if let Some(worker) = running.remove(&node) {
                    self.scanner_stop(worker).await;
                }
            }
            for (node, (spec, one)) in &wanted {
                if running.contains_key(node) {
                    continue;
                }
                let (stop, stopped) = watch::channel(false);
                let control = Arc::clone(&self);
                let (spec, one) = ((*spec).clone(), (*one).clone());
                let task = tokio::spawn({
                    let spec = spec.clone();
                    // naked: the worker's every wait is bounded inside it
                    async move { control.scanner_worker(spec, one, stopped).await }
                });
                running.insert(node.clone(), Running { spec, stop, task });
            }

            self.scanner_states_now(&scanners, enable, &wanted);

            // naked: the settings, the table, a timer and the shutdown signal
            tokio::select! {
                changed = follow.changed() => if changed.is_err() { break },
                _ = self.scanner_wake.notified() => {}
                _ = tokio::time::sleep(POLL) => {}
                _ = shutdown.changed() => break,
            }
        }

        for (_, worker) in std::mem::take(&mut running) {
            self.scanner_stop(worker).await;
        }
        let path = self.paths.scanner_rules.clone();
        let udevadm = self.paths.udevadm.clone();
        let _ = blocking("removing the scanners' udev rule", move || {
            let ttys = devices::usb_ttys(&sysfs);
            crate::scanner::apply_rules(&path, None, &udevadm, &ttys)
        })
        .await;
    }

    /// The rule changed which keyboards are scanners, which screen.osk=auto
    /// counts: Weston's config is checked again now, as after a hotplug,
    /// rather than at the next device plugged in. The rule is part of what
    /// it is checked for, so a restart for the same hardware with other
    /// scanners is not taken for one already made.
    async fn scanner_keyboards(&self, rules: Option<&str>) {
        let paths = self.paths.clone();
        let Ok(hardware) = blocking("reading the displays and input devices", move || {
            Ok(crate::hotplug::snapshot(&crate::hotplug::Sources {
                drm: &paths.drm,
                input: &paths.input,
            }))
        })
        .await
        else {
            return;
        };
        let snapshot = format!("{hardware}\n{}", rules.unwrap_or("no scanners"));
        // naked: every wait in it is blocking()/Bus, under within()
        self.reconcile_display(&snapshot).await;
    }

    async fn scanner_stop(&self, worker: Running) {
        let _ = worker.stop.send(true);
        if within("a scanner's worker stopping", STOP_LIMIT, worker.task)
            .await
            .is_err()
        {
            self.log.info(format!(
                "scanner {}: the worker did not stop in time",
                worker.spec.name
            ));
        }
    }

    /// Each scanner's state where the supervisor decides it: disabled, or
    /// missing while no device is it. A worker says reading or failed.
    fn scanner_states_now(
        &self,
        scanners: &Scanners,
        enable: bool,
        wanted: &BTreeMap<PathBuf, (&ScannerSpec, &Found)>,
    ) {
        let mut gone = Vec::new();
        {
            let mut states = lock(&self.scanner_states);
            states.retain(|name, _| scanners.scanners.iter().any(|spec| spec.name == *name));
            for spec in &scanners.scanners {
                let state = states.entry(spec.name.clone()).or_default();
                let plugged = wanted.values().any(|(wanted, _)| wanted.name == spec.name);
                let now = if !enable || !spec.enabled {
                    Some("disabled")
                } else if !plugged {
                    Some("missing")
                } else {
                    None
                };
                if let Some(now) = now {
                    if state.state == "reading" {
                        gone.push((spec.clone(), state.node.clone()));
                    }
                    state.state = now.to_string();
                    state.node = None;
                    state.message = None;
                }
            }
        }
        for (spec, node) in gone {
            self.scanner_change(&spec, "disconnected", node, None);
        }
    }

    /// One scanner on one device node until it is unplugged or stopped.
    async fn scanner_worker(
        self: Arc<Self>,
        spec: ScannerSpec,
        found: Found,
        mut stop: watch::Receiver<bool>,
    ) {
        let node = found.node.display().to_string();
        let opened = self.scanner_open(&spec, &found).await;
        let (reader, mut decoder) = match opened {
            Ok(opened) => opened,
            Err(err) => {
                self.scanner_failed(&spec, &node, err);
                return;
            }
        };
        self.scanner_connected(&spec, &node);

        let mut assembler = Assembler::new(&spec);
        let mut buffer = vec![0u8; READ];
        let mut symbology: Option<String> = None;
        loop {
            let deadline = assembler.deadline();
            let quiet = tokio::time::Instant::from_std(
                deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600)),
            );
            // naked: the device, the quiet that ends a scan and the stop signal
            let events = tokio::select! {
                read = reader.read(&mut buffer) => match read {
                    Ok(n) if n > 0 => decoder.decode(&buffer[..n], &mut assembler, &mut symbology, Instant::now()),
                    Ok(_) => {
                        self.scanner_unplugged(&spec, &node, None);
                        return;
                    }
                    Err(err) => {
                        self.scanner_unplugged(&spec, &node, Some(err.to_string()));
                        return;
                    }
                },
                _ = tokio::time::sleep_until(quiet), if deadline.is_some() => {
                    assembler.expire(Instant::now()).map(Event::End).into_iter().collect()
                }
                _ = stop.changed() => return,
            };
            for event in events {
                self.scanner_event(&spec, event, &mut symbology).await;
            }
        }
    }

    /// The device opened and taken, and what turns its bytes into input.
    async fn scanner_open(
        &self,
        spec: &ScannerSpec,
        found: &Found,
    ) -> Result<(io::Reader, Decoder), String> {
        let decoder = match spec.transport {
            Transport::Keyboard => {
                let xkb = self.paths.xkb.clone();
                let layout = spec.layout().to_string();
                let keymap = blocking("reading the keyboard layout", move || {
                    Keymap::load(&xkb, &layout)
                })
                .await?;
                Decoder::Keys(Keyboard::new(keymap))
            }
            Transport::Serial => Decoder::Bytes,
            Transport::Hidpos => Decoder::Reports(hidpos::layout(&found.descriptor).ok_or(
                "its report descriptor has no barcode data (HID usage page 0x8C, Decoded Data)",
            )?),
        };
        let node = found.node.clone();
        let (transport, baud) = (spec.transport, spec.baud());
        let file = blocking("opening the scanner", move || {
            io::open(&node, transport, true, baud).map_err(|err| match err.kind() {
                std::io::ErrorKind::ResourceBusy => {
                    "another program has it; a keyboard scanner's input device or a port open \
                     elsewhere"
                        .to_string()
                }
                _ => err.to_string(),
            })
        })
        .await?;
        // Registering with the runtime needs the runtime's thread.
        let reader = io::Reader::new(file).map_err(|err| err.to_string())?;
        Ok((reader, decoder))
    }

    async fn scanner_event(
        &self,
        spec: &ScannerSpec,
        event: Event,
        symbology: &mut Option<String>,
    ) {
        match event {
            Event::Begin => {
                let detail = json!({
                    "event": "begin",
                    "scanner": spec.name,
                    "transport": spec.transport,
                    "at_ms": now_ms(),
                });
                self.scanner_send(Dispatch {
                    scanner: spec.name.clone(),
                    detail,
                    scanned: None,
                });
            }
            Event::End(done) => {
                let symbology = symbology.take();
                self.scanner_scanned(spec, done, symbology).await;
            }
        }
    }

    /// A scan ended: counted, logged without its content, and handed on.
    async fn scanner_scanned(
        &self,
        spec: &ScannerSpec,
        done: frame::Done,
        symbology: Option<String>,
    ) {
        let at_ms = now_ms();
        let length = done.bytes.len() as u32;
        if let Some(state) = lock(&self.scanner_states).get_mut(&spec.name) {
            state.scans += 1;
            state.last_scan_ms = Some(at_ms);
        }
        lock(&self.scan_log).push(
            Entry {
                scanner: &spec.name,
                event: "scan",
                length: Some(length),
                ms: Some(done.ms),
                symbology: symbology.clone(),
                message: None,
            },
            at_ms,
        );
        self.log.debug(format!(
            "scanner {}: a scan of {length} bytes in {} ms",
            spec.name, done.ms
        ));

        let scan = Scan {
            scanner: spec.name.clone(),
            transport: spec.transport,
            text: String::from_utf8(done.bytes.clone()).ok(),
            bytes: openssl::base64::encode_block(&done.bytes),
            length,
            ms: done.ms,
            symbology,
            at_ms,
            time: String::new(),
        };
        self.scanner_tap(&scan).await;
        let detail = json!({
            "event": "end",
            "scanner": scan.scanner,
            "transport": scan.transport,
            "at_ms": at_ms,
            "text": scan.text,
            "bytes": scan.bytes,
            "length": length,
            "ms": scan.ms,
            "symbology": scan.symbology,
        });
        self.scanner_send(Dispatch {
            scanner: spec.name.clone(),
            detail,
            scanned: Some(done.bytes),
        });
    }

    /// The scan to every `scanner test` and `scanner identify` listening
    /// for it; one that went away is forgotten.
    async fn scanner_tap(&self, scan: &Scan) {
        let wanted = lock(&self.scan_taps)
            .iter()
            .any(|(name, _)| name == &scan.scanner || name == wire::ANY);
        if !wanted {
            return;
        }
        let at_ms = scan.at_ms;
        let mut scan = scan.clone();
        scan.time = blocking("reading the clock", move || Ok(time(at_ms)))
            .await
            .unwrap_or_default();
        lock(&self.scan_taps).retain(|(name, tap)| {
            if name != &scan.scanner && name != wire::ANY {
                return !tap.is_closed();
            }
            !matches!(
                tap.try_send(scan.clone()),
                Err(mpsc::error::TrySendError::Closed(_))
            )
        });
    }

    fn scanner_send(&self, dispatch: Dispatch) {
        if let Some(queue) = self.scanner_queue.get() {
            if queue.try_send(dispatch).is_err() {
                self.log
                    .debug("scanners: the page and the scripts are behind; a scan event dropped");
            }
        }
    }

    fn scanner_connected(&self, spec: &ScannerSpec, node: &str) {
        if let Some(state) = lock(&self.scanner_states).get_mut(&spec.name) {
            state.state = "reading".to_string();
            state.node = Some(node.to_string());
            state.message = None;
        }
        self.log.info(format!(
            "scanner {}: reading {} on {node}",
            spec.name,
            spec.transport.name()
        ));
        self.scanner_change(spec, "connected", Some(node.to_string()), None);
    }

    fn scanner_failed(&self, spec: &ScannerSpec, node: &str, err: String) {
        if let Some(state) = lock(&self.scanner_states).get_mut(&spec.name) {
            state.state = "failed".to_string();
            state.node = Some(node.to_string());
            state.message = Some(err.clone());
        }
        self.log
            .info(format!("scanner {}: {node}: {err}", spec.name));
        lock(&self.scan_log).push(
            Entry {
                scanner: &spec.name,
                event: "failed",
                message: Some(err),
                ..Default::default()
            },
            now_ms(),
        );
    }

    /// The device went away under its worker.
    fn scanner_unplugged(&self, spec: &ScannerSpec, node: &str, err: Option<String>) {
        if let Some(state) = lock(&self.scanner_states).get_mut(&spec.name) {
            state.state = "missing".to_string();
            state.node = None;
            state.message = None;
        }
        self.log.info(format!(
            "scanner {}: {node} went away{}",
            spec.name,
            err.map(|err| format!(": {err}")).unwrap_or_default()
        ));
        self.scanner_change(spec, "disconnected", Some(node.to_string()), None);
    }

    /// `connected` or `disconnected`: to the log and the page.
    fn scanner_change(
        &self,
        spec: &ScannerSpec,
        event: &str,
        node: Option<String>,
        message: Option<String>,
    ) {
        let at_ms = now_ms();
        lock(&self.scan_log).push(
            Entry {
                scanner: &spec.name,
                event,
                message: message.or_else(|| node.clone()),
                ..Default::default()
            },
            at_ms,
        );
        let mut detail = json!({
            "event": event,
            "scanner": spec.name,
            "transport": spec.transport,
            "at_ms": at_ms,
        });
        if let (Some(node), "connected") = (node, event) {
            detail["node"] = json!(node);
        }
        self.scanner_send(Dispatch {
            scanner: spec.name.clone(),
            detail,
            scanned: None,
        });
    }
}

/// What turns a device's bytes into a scanner's input.
enum Decoder {
    Keys(Keyboard),
    Bytes,
    Reports(hidpos::Layout),
}

impl Decoder {
    fn decode(
        &mut self,
        bytes: &[u8],
        assembler: &mut Assembler,
        symbology: &mut Option<String>,
        now: Instant,
    ) -> Vec<Event> {
        match self {
            Decoder::Keys(keyboard) => io::key_events(bytes)
                .into_iter()
                .flat_map(|(code, value)| keyboard.key(code, value))
                .flat_map(|input| assembler.push(input, now))
                .collect(),
            Decoder::Bytes => assembler.push_all(bytes, now),
            Decoder::Reports(layout) => {
                let Some(piece) = layout.read(bytes) else {
                    return Vec::new();
                };
                if piece.symbology.is_some() {
                    *symbology = piece.symbology;
                }
                let mut events = assembler.push_all(&piece.data, now);
                if !piece.continued {
                    events.extend(assembler.finish(now));
                }
                events
            }
        }
    }
}

/// `scanner identify` on one device that is no scanner yet: listened to,
/// never taken, read as a scanner of its kind with the defaults would be,
/// until a scan ends or `stop`.
async fn listen(
    found: Found,
    xkb: PathBuf,
    mut stop: watch::Receiver<bool>,
    heard: mpsc::Sender<(Found, Scan)>,
) {
    let spec = ScannerSpec {
        name: String::new(),
        transport: found.transport,
        vendor: found.vendor.clone(),
        product: found.product.clone(),
        serial: found.serial.clone(),
        port: Some(found.port.clone()),
        layout: None,
        terminator: None,
        gap_ms: None,
        baud: None,
        strip_prefix: None,
        strip_suffix: None,
        enabled: true,
    };
    let mut decoder = match found.transport {
        Transport::Keyboard => {
            let layout = spec.layout().to_string();
            match blocking("reading the keyboard layout", move || {
                Keymap::load(&xkb, &layout)
            })
            .await
            {
                Ok(keymap) => Decoder::Keys(Keyboard::new(keymap)),
                Err(_) => return,
            }
        }
        Transport::Serial => Decoder::Bytes,
        Transport::Hidpos => match hidpos::layout(&found.descriptor) {
            Some(layout) => Decoder::Reports(layout),
            None => return,
        },
    };
    let node = found.node.clone();
    let transport = found.transport;
    let Ok(file) = blocking("opening a device to listen to", move || {
        io::open(&node, transport, false, wire::BAUD_DEFAULT).map_err(|err| err.to_string())
    })
    .await
    else {
        return;
    };
    let Ok(reader) = io::Reader::new(file) else {
        return;
    };
    let mut assembler = Assembler::new(&spec);
    let mut buffer = vec![0u8; READ];
    let mut symbology = None;
    loop {
        let deadline = assembler.deadline();
        let quiet = tokio::time::Instant::from_std(
            deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(3600)),
        );
        // naked: the device, the quiet that ends a scan and the stop signal
        let events = tokio::select! {
            read = reader.read(&mut buffer) => match read {
                Ok(n) if n > 0 => decoder.decode(&buffer[..n], &mut assembler, &mut symbology, Instant::now()),
                _ => return,
            },
            _ = tokio::time::sleep_until(quiet), if deadline.is_some() => {
                assembler.expire(Instant::now()).map(Event::End).into_iter().collect()
            }
            _ = stop.changed() => return,
        };
        for event in events {
            if let Event::End(done) = event {
                if done.bytes.is_empty() {
                    continue;
                }
                let at_ms = now_ms();
                let scan = Scan {
                    scanner: String::new(),
                    transport: found.transport,
                    text: String::from_utf8(done.bytes.clone()).ok(),
                    bytes: openssl::base64::encode_block(&done.bytes),
                    length: done.bytes.len() as u32,
                    ms: done.ms,
                    symbology: symbology.take(),
                    at_ms,
                    time: blocking("reading the clock", move || Ok(time(at_ms)))
                        .await
                        .unwrap_or_default(),
                };
                let _ = heard.try_send((found, scan));
                return;
            }
        }
    }
}

/// A scanner as `scanner list` reports it. `last_scan` carries the moment
/// in milliseconds until the caller turns it into the device's clock.
fn info(spec: &ScannerSpec, state: Option<&State>, enabled: bool) -> ScannerInfo {
    let state = state.cloned().unwrap_or_default();
    let name = if !enabled || !spec.enabled {
        "disabled".to_string()
    } else if state.state.is_empty() {
        "missing".to_string()
    } else {
        state.state
    };
    ScannerInfo {
        spec: spec.clone(),
        state: name,
        node: state.node,
        message: state.message,
        scans: state.scans,
        last_scan: state.last_scan_ms.map(|at| at.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(enabled: bool) -> ScannerSpec {
        ScannerSpec {
            name: "front".into(),
            transport: Transport::Serial,
            vendor: "0c2e".into(),
            product: "0b63".into(),
            serial: None,
            port: Some("1-1".into()),
            layout: None,
            terminator: None,
            gap_ms: None,
            baud: None,
            strip_prefix: None,
            strip_suffix: None,
            enabled,
        }
    }

    #[test]
    fn a_scanner_is_disabled_missing_or_as_its_worker_says() {
        assert_eq!(info(&spec(true), None, false).state, "disabled");
        assert_eq!(info(&spec(false), None, true).state, "disabled");
        assert_eq!(info(&spec(true), None, true).state, "missing");
        let reading = State {
            state: "reading".into(),
            node: Some("/dev/ttyACM0".into()),
            scans: 3,
            last_scan_ms: Some(1_700_000_000_000),
            ..Default::default()
        };
        let shown = info(&spec(true), Some(&reading), true);
        assert_eq!(
            (shown.state.as_str(), shown.node.as_deref(), shown.scans),
            ("reading", Some("/dev/ttyACM0"), 3)
        );
        assert_eq!(shown.last_scan.as_deref(), Some("1700000000000"));
    }

    #[test]
    fn a_hid_pos_report_ends_a_scan_unless_more_follows() {
        let layout = hidpos::layout(crate::scanner::hidpos::tests::DESCRIPTOR).unwrap();
        let mut decoder = Decoder::Reports(layout);
        let mut hidpos = spec(true);
        hidpos.transport = Transport::Hidpos;
        let mut assembler = Assembler::new(&hidpos);
        let mut symbology = None;
        let report = |data: &[u8], continued: bool| {
            let mut report = vec![0x02, b']', b'Q', b'1'];
            let mut padded = data.to_vec();
            padded.resize(56, 0);
            report.extend(padded);
            report.push(u8::from(continued));
            report
        };
        let now = Instant::now();
        let first = decoder.decode(
            &report(b"part one ", true),
            &mut assembler,
            &mut symbology,
            now,
        );
        assert_eq!(first, vec![Event::Begin]);
        assert_eq!(symbology.as_deref(), Some("]Q1"));
        let second = decoder.decode(
            &report(b"and two", false),
            &mut assembler,
            &mut symbology,
            now,
        );
        match second.as_slice() {
            [Event::End(done)] => assert_eq!(done.bytes, b"part one and two"),
            other => panic!("{other:?}"),
        }
    }
}
