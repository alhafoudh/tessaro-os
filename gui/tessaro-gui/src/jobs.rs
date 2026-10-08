//! Long work on a device, each on a connection of its own so the window's
//! worker keeps polling: the device's own jobs (`network ping`, the speed
//! test, growing `/data`, looking for printers and scanners, a scanner's
//! test), files going up or down, an
//! image update, the DevTools and VNC tunnels, and a script's run followed
//! to its end.
//!
//! A job is a subscription keyed by its id. Cancelling it drops the
//! subscription. A device job sees that between two polls and cancels it on
//! the device; the shared flows see it as `Report::stopped` at their next
//! step, and for a call in flight a watcher thread shuts the connection
//! down. The flows themselves are `tessaro_client`'s (`files`, `update`,
//! `ping`, `storage`, `devtools`, `vnc`, `printer`, `scanner`, `script`),
//! the same as
//! `tessaro-ctl`'s.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::net::Shutdown;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iced::futures::channel::mpsc as ui;
use iced::Subscription;
use protocol::api::{
    self, Empty, Endpoint, GrowBody, PingBody, RestartBody, SetConfig, SpeedtestBody,
};
use protocol::files::{self as store, FileEntry};
use protocol::{JobStarted, PingEvent, RestartTarget, ScriptEvent, Verify};
use serde_json::Value;
use tessaro_client::connect::{Answer, Session};
use tessaro_client::nodes::Node;
use tessaro_client::report::{self, Report as _};
use tessaro_client::text::{Line, Tone};
use tessaro_client::tunnel::{self, Prompts, Tunnel};
use tessaro_client::update::{self, Plan, Sent};
use tessaro_client::{
    describe, devtools, files, network, ping, printer, scanner, script, ssh, storage, vnc,
};

use crate::worker;

#[derive(Debug, Clone)]
pub enum Kind {
    /// A job the device runs, answered with events.
    Stream(Stream),
    /// Local files and directories into `into`, a directory of the store.
    Upload { local: Vec<PathBuf>, into: String },
    /// A stored file or directory into the local directory `into`.
    Download { entry: FileEntry, into: PathBuf },
    /// An image, staged, checked and committed.
    Update(Plan),
    /// `tessaro-ctl device ping`: round trips over the control connection.
    ControlPing { count: u32 },
    /// `tessaro-ctl browser devtools`: the device's DevTools port forwarded
    /// to this machine until the job is cancelled.
    DevTools,
    /// `tessaro-ctl screen vnc`: the device's VNC mirror started and
    /// forwarded to this machine, for a viewer of the user's own, until the
    /// job is cancelled.
    Vnc,
    /// `tessaro-ctl printer discover`: every printer the device finds, as
    /// the job's values.
    Discover,
    /// `tessaro-ctl scanner discover`: every device that may be a scanner,
    /// as the job's values.
    ScannerDiscover,
    /// `tessaro-ctl scanner identify`: the device the next scan comes from,
    /// as the job's value.
    ScannerIdentify,
    /// `tessaro-ctl scanner test`: a scanner's scans as the job's lines.
    ScannerTest(String),
    /// `tessaro-ctl script run`: the script's output as the job's lines,
    /// then how the run ended.
    Script(String),
    /// `tessaro-ctl browser reload`, for the bulk window: the device
    /// windows send it through their worker.
    Reload,
    /// `tessaro-ctl device restart`, for the bulk window.
    Restart(RestartTarget),
    /// `tessaro-ctl device reboot`, for the bulk window.
    Reboot,
    /// `tessaro-ctl config set`, for the bulk window: no revision to hold
    /// it to, each device has its own.
    Set(BTreeMap<String, String>),
}

/// The device's jobs, each with what starts it.
#[derive(Debug, Clone)]
pub enum Stream {
    Ping(PingBody),
    Speedtest(SpeedtestBody),
    Grow(GrowBody),
}

#[derive(Debug, Clone)]
pub enum Event {
    Progress {
        label: Line,
        done: u64,
        total: u64,
    },
    /// One event of a stream.
    Value(serde_json::Value),
    /// A step done, for the log.
    Line(Line),
    /// The update erases /data: the device comes back as a new node, and
    /// what this machine knows about it is worthless.
    Wiped,
    Finished(Result<String, String>),
}

#[derive(Debug, Clone)]
struct Spec {
    id: u64,
    node: Node,
    kind: Kind,
}

impl Hash for Spec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.node.id.hash(state);
        self.id.hash(state);
    }
}

pub fn subscription(node: Node, id: u64, kind: Kind) -> Subscription<Event> {
    Subscription::run_with(Spec { id, node, kind }, start)
}

fn start(spec: &Spec) -> ui::UnboundedReceiver<Event> {
    let (out, receive) = ui::unbounded();
    let spec = spec.clone();
    std::thread::spawn(move || {
        let result = run(&spec, &out);
        let _ = out.unbounded_send(Event::Finished(result));
    });
    receive
}

/// A shared flow's progress, as the job's events. Cancel closes `out`,
/// which the flow sees as `stopped` at its next step.
struct Report<'a> {
    out: &'a ui::UnboundedSender<Event>,
}

impl report::Report for Report<'_> {
    fn progress(&mut self, label: Line, done: u64, total: u64) {
        let _ = self
            .out
            .unbounded_send(Event::Progress { label, done, total });
    }

    fn line(&mut self, line: Line) {
        let _ = self.out.unbounded_send(Event::Line(line));
    }

    fn stopped(&self) -> bool {
        self.out.is_closed()
    }
}

fn run(spec: &Spec, out: &ui::UnboundedSender<Event>) -> Result<String, String> {
    let (mut session, _) = worker::connect(&spec.node)?;
    let done = Arc::new(AtomicBool::new(false));
    // A device job stops by itself, and cancels on the device on the way:
    // shutting its connection down would only lose that cancel. The VNC
    // tunnel the same: it stops the mirror as it closes.
    let watched = !matches!(
        spec.kind,
        Kind::Stream(_)
            | Kind::Discover
            | Kind::ScannerDiscover
            | Kind::ScannerIdentify
            | Kind::ScannerTest(_)
            | Kind::Script(_)
            | Kind::Vnc
    );
    if let Some(tcp) = session.shutdown_handle().filter(|_| watched) {
        let (out, done) = (out.clone(), done.clone());
        std::thread::spawn(move || {
            while !out.is_closed() && !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
            }
            if !done.load(Ordering::Relaxed) {
                let _ = tcp.shutdown(Shutdown::Both);
            }
        });
    }
    let mut report = Report { out };
    let result = match &spec.kind {
        Kind::Stream(Stream::Ping(body)) => net_ping(&mut session, body.clone(), out),
        Kind::Stream(Stream::Speedtest(body)) => {
            stream::<api::network::Speedtest>(&mut session, body.clone(), out, |_| {})
        }
        Kind::Stream(Stream::Grow(body)) => grow(&mut session, *body, out),
        Kind::Upload { local, into } => upload(&mut session, local, into, &mut report),
        Kind::Download { entry, into } => {
            files::download(&mut session, &entry.path, Some(into.clone()), &mut report)
                .map(|summary| summary.line("received").to_string())
        }
        Kind::Update(update) => update_send(&mut session, &spec.node, update, &mut report),
        Kind::ControlPing { count } => control_ping(&mut session, *count, &mut report),
        Kind::DevTools => open_devtools(&mut session, &mut report),
        Kind::Vnc => open_vnc(&mut session, &mut report),
        Kind::Discover => discover(&mut session, out),
        Kind::ScannerDiscover => scanner_discover(&mut session, out),
        Kind::ScannerIdentify => scanner_identify(&mut session, out),
        Kind::ScannerTest(name) => scanner_test(&mut session, name, out),
        Kind::Script(name) => script_run(&mut session, name, out),
        Kind::Reload => session
            .call::<api::browser::Reload>(Empty {}, ())
            .map(|done| done.message),
        Kind::Restart(what) => session
            .call::<api::device::Restart>(Empty {}, RestartBody { what: *what })
            .map(|done| done.message),
        Kind::Reboot => session
            .call::<api::device::Reboot>(Empty {}, ())
            .map(|done| done.message),
        Kind::Set(values) => set(&mut session, values, &mut report),
    };
    done.store(true, Ordering::Relaxed);
    if out.is_closed() {
        return Err("cancelled".to_string());
    }
    result
}

/// Every event of the job as it came: the page reads them as the job's
/// event type. `each` sees them too, for a verdict at the end.
fn stream<S>(
    session: &mut Session,
    body: S::Body,
    out: &ui::UnboundedSender<Event>,
    mut each: impl FnMut(&Value),
) -> Result<String, String>
where
    S: Endpoint<Response = JobStarted>,
    S::Params: Default,
{
    session.job::<S, Value>(body, &|| out.is_closed(), |event| {
        let event = event.unwrap_or_else(|unknown| unknown);
        each(&event);
        let _ = out.unbounded_send(Event::Value(event));
    })?;
    Ok("done".to_string())
}

/// `tessaro-ctl network ping`: no replies at all is a failure, as there.
fn net_ping(
    session: &mut Session,
    body: PingBody,
    out: &ui::UnboundedSender<Event>,
) -> Result<String, String> {
    let mut failed = false;
    stream::<api::network::Ping>(session, body, out, |event| {
        if let Ok(PingEvent::Summary { received: 0, .. }) = serde_json::from_value(event.clone()) {
            failed = true;
        }
    })?;
    if failed {
        return Err("no replies".to_string());
    }
    Ok("done".to_string())
}

/// `tessaro-ctl storage grow`: the device's plan first, and the grow only
/// when it would change something.
fn grow(
    session: &mut Session,
    body: GrowBody,
    out: &ui::UnboundedSender<Event>,
) -> Result<String, String> {
    let plan = storage::check(session)?;
    let (facts, nothing) = plan.facts();
    for fact in facts {
        let line = Line::new()
            .pad(Tone::Label, fact.label, 12)
            .text(" ")
            .join(fact.value);
        let _ = out.unbounded_send(Event::Line(line));
    }
    if let Some(nothing) = nothing {
        return Ok(nothing.to_string());
    }
    if body.check {
        return Ok("checked".to_string());
    }
    stream::<api::storage::Grow>(session, body, out, |_| {})
        .map_err(|error| format!("{error}; {}", storage::STOPPED_HINT))
}

/// `tessaro-ctl printer discover`: each printer as it comes, for the page to
/// list, and what to do with them at the end.
fn discover(session: &mut Session, out: &ui::UnboundedSender<Event>) -> Result<String, String> {
    let found = printer::discover(session, &|| out.is_closed(), |found| {
        if let Ok(value) = serde_json::to_value(found) {
            let _ = out.unbounded_send(Event::Value(value));
        }
    })?;
    Ok(describe::printer::found_hint(&found).to_string())
}

/// `tessaro-ctl scanner discover`: each device as it comes, for the page to
/// list, and what to do with them at the end.
fn scanner_discover(
    session: &mut Session,
    out: &ui::UnboundedSender<Event>,
) -> Result<String, String> {
    let found = scanner::discover(session, &|| out.is_closed(), |candidate| {
        if let Ok(value) = serde_json::to_value(candidate) {
            let _ = out.unbounded_send(Event::Value(value));
        }
    })?;
    Ok(describe::scanner::candidates_hint(&found).to_string())
}

/// `tessaro-ctl scanner identify`: the device a scan came from, for the
/// page to offer, or that nothing was scanned.
fn scanner_identify(
    session: &mut Session,
    out: &ui::UnboundedSender<Event>,
) -> Result<String, String> {
    match scanner::identify(session, &|| out.is_closed())? {
        Some(heard) => {
            let said = format!("heard {}", heard.description);
            if let Ok(value) = serde_json::to_value(&heard) {
                let _ = out.unbounded_send(Event::Value(value));
            }
            Ok(said)
        }
        None => Err(describe::scanner::identified(None)
            .first()
            .map(ToString::to_string)
            .unwrap_or_default()),
    }
}

/// `tessaro-ctl scanner test`: each scan as a line as it comes.
fn scanner_test(
    session: &mut Session,
    name: &str,
    out: &ui::UnboundedSender<Event>,
) -> Result<String, String> {
    let scans = scanner::test(session, name, &|| out.is_closed(), |scan| {
        let _ = out.unbounded_send(Event::Line(describe::scanner::scan(scan)));
    })?;
    if scans.is_empty() {
        Err("nothing was scanned".to_string())
    } else {
        Ok("done".to_string())
    }
}

/// `tessaro-ctl script run`: each line as it comes, and a failed run is a
/// failed job. Cancelling stops following it; the run goes on.
fn script_run(
    session: &mut Session,
    name: &str,
    out: &ui::UnboundedSender<Event>,
) -> Result<String, String> {
    let ended = script::run(session, name, &|| out.is_closed(), |event| {
        if !matches!(event, ScriptEvent::Ended { .. }) {
            let _ = out.unbounded_send(Event::Line(script::event_line(event, name)));
        }
    })?;
    match ended {
        Some(run) if run.succeeded() => Ok(script::ended(&run).to_string()),
        Some(run) => Err(script::ended(&run).to_string()),
        None => Err("the run's end was not seen".to_string()),
    }
}

/// `tessaro-ctl config set`, applied: what it did as the job's lines. A
/// network key gets the time a network change takes, as in the worker.
fn set(
    session: &mut Session,
    values: &BTreeMap<String, String>,
    report: &mut Report,
) -> Result<String, String> {
    let network = values.keys().any(|key| network::is_network_key(key));
    let body = SetConfig {
        values: values.clone(),
        if_revision: None,
        apply: true,
        verify: Verify::default(),
    };
    let request = |session: &mut Session| session.request::<api::config::Set>(Empty {}, body);
    let answer = if network {
        network::apply(session, request)
    } else {
        request(session)
    };
    let applied = match answer {
        Answer::Ok(applied) => applied,
        Answer::Refused(error) => return Err(error),
        Answer::Lost(why) if network => return Err(network::lost(&why)),
        Answer::Lost(why) => return Err(format!("lost the connection: {why}")),
    };
    for line in describe::device::applied(&applied, false) {
        report.line(line);
    }
    Ok("set".to_string())
}

/// Each local path into `into`: a file as `into/NAME`, a directory as
/// `into/NAME` with everything in it.
fn upload(
    session: &mut Session,
    local: &[PathBuf],
    into: &str,
    report: &mut Report,
) -> Result<String, String> {
    let mut lines = Vec::new();
    for path in local {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{}: no usable file name", path.display()))?;
        let summary = files::upload(session, path, Some(&store::join(into, name)), report)?;
        lines.push(summary.line("sent").to_string());
    }
    Ok(lines.join("; "))
}

/// `tessaro-ctl update send`, reporting here, and waiting for the device to
/// come back when it reboots into the image.
fn update_send(
    session: &mut Session,
    node: &Node,
    plan: &Plan,
    report: &mut Report,
) -> Result<String, String> {
    match update::send(session, plan, report)? {
        Sent::Staged => Ok("staged; it is applied at the next reboot".to_string()),
        Sent::Wiped => {
            let _ = report.out.unbounded_send(Event::Wiped);
            Ok("sent; the device comes back as a new node".to_string())
        }
        Sent::Rebooting => {
            update::wait_back(
                &session.node.name,
                || worker::connect(node).map(|(session, _)| session),
                report,
            )?;
            Ok("applied".to_string())
        }
    }
}

/// `tessaro-ctl device ping`, reporting here.
fn control_ping(session: &mut Session, count: u32, report: &mut Report) -> Result<String, String> {
    for line in ping::intro(session) {
        report.line(line);
    }
    let summary = ping::device(session, count, Duration::from_secs(1), report)?;
    if summary.received() == 0 {
        return Err("no replies".to_string());
    }
    Ok(summary.line().to_string())
}

/// `tessaro-ctl browser devtools`: the tunnel, open until the job is
/// cancelled or ssh ends. The device reports a connected DevTools window
/// in its `Status`, which the Browser page shows.
fn open_devtools(session: &mut Session, report: &mut Report) -> Result<String, String> {
    let authorized = ssh::authorize(session, None)?;
    let port = tunnel::free_port(tunnel::DEVTOOLS_LOCAL)?;
    let mut forward = Tunnel::open(&authorized, port, tunnel::DEVTOOLS, Prompts::Never)?;
    report.progress(
        Line::of(Tone::Ok, format!("forwarding localhost:{port}")),
        1,
        1,
    );
    for line in devtools::explain(&session.node.name, port, "Cancel closes the tunnel") {
        report.line(line);
    }
    devtools::watch(session, &mut forward, report)?;
    Ok("closed".to_string())
}

/// `tessaro-ctl screen vnc`: the mirror started and the tunnel to it, open
/// until the job is cancelled or ssh ends, then the mirror stopped.
fn open_vnc(session: &mut Session, report: &mut Report) -> Result<String, String> {
    let mut opened = vnc::open(session, None, tunnel::VNC_LOCAL, Prompts::Never)?;
    let port = opened.tunnel.port;
    report.progress(
        Line::of(Tone::Ok, format!("forwarding localhost:{port}")),
        1,
        1,
    );
    let mode = opened.vnc.mode.clone();
    for line in vnc::explain(&session.node.name, port, &mode, "Cancel closes the tunnel") {
        report.line(line);
    }
    vnc::watch(session, &mut opened.tunnel, report)?;
    Ok("closed".to_string())
}
