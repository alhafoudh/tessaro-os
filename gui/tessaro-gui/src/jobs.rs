//! Long work on a device, each on a connection of its own so the window's
//! worker keeps polling: the streams (`network ping`, the speed test,
//! growing `/data`), files going up or down, and an image update.
//!
//! A job is a subscription keyed by its id. Cancelling it drops the
//! subscription; a watcher thread then shuts the connection down, which
//! ends whatever call the job was in. The transfers themselves are
//! `tessaro_client::transfer`, the same as `tessaro-ctl files` and
//! `tessaro-ctl update send`.

use std::hash::{Hash, Hasher};
use std::net::Shutdown;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iced::futures::channel::mpsc as ui;
use iced::Subscription;
use protocol::files::{self as store, FileEntry, FileKind, FilesListing};
use protocol::{Command, Done, ImageUpload, UpdateBegun, UpdatePhase, UpdateStatus};
use tessaro_client::connect::Session;
use tessaro_client::nodes::Node;
use tessaro_client::transfer::{self, mb, mtime_of};

use crate::worker;

#[derive(Debug, Clone)]
pub enum Kind {
    /// A command answered with events.
    Stream(Command),
    /// Local files and directories into `into`, a directory of the store.
    Upload { local: Vec<PathBuf>, into: String },
    /// A stored file or directory into the local directory `into`.
    Download { entry: FileEntry, into: PathBuf },
    /// An image, staged, checked and committed.
    Update(Update),
    /// `tessaro-ctl device ping`: round trips over the control connection.
    ControlPing { count: u32 },
}

#[derive(Debug, Clone)]
pub struct Update {
    pub image: PathBuf,
    pub bmap: PathBuf,
    pub wipe_data: bool,
    pub repartition: bool,
    pub verify: bool,
    pub reboot: bool,
}

#[derive(Debug, Clone)]
pub enum Event {
    Progress {
        label: String,
        done: u64,
        total: u64,
    },
    /// One event of a stream.
    Value(serde_json::Value),
    /// A step done, for the log.
    Line(String),
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

struct Report<'a> {
    out: &'a ui::UnboundedSender<Event>,
}

impl Report<'_> {
    fn progress(&self, label: impl Into<String>, done: u64, total: u64) {
        let _ = self.out.unbounded_send(Event::Progress {
            label: label.into(),
            done,
            total,
        });
    }

    fn line(&self, line: impl Into<String>) {
        let _ = self.out.unbounded_send(Event::Line(line.into()));
    }
}

fn run(spec: &Spec, out: &ui::UnboundedSender<Event>) -> Result<String, String> {
    let (mut session, _) = worker::connect(&spec.node)?;
    let done = Arc::new(AtomicBool::new(false));
    if let Some(tcp) = session.shutdown_handle() {
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
    let report = Report { out };
    let result = match &spec.kind {
        Kind::Stream(command) => stream(&mut session, command.clone(), out),
        Kind::Upload { local, into } => upload(&mut session, local, into, &report),
        Kind::Download { entry, into } => download(&mut session, entry, into, &report),
        Kind::Update(update) => update_send(&mut session, update, &report),
        Kind::ControlPing { count } => control_ping(&mut session, *count, &report),
    };
    done.store(true, Ordering::Relaxed);
    if out.is_closed() {
        return Err("cancelled".to_string());
    }
    result
}

fn stream(
    session: &mut Session,
    command: Command,
    out: &ui::UnboundedSender<Event>,
) -> Result<String, String> {
    session.stream(command, |event| {
        let _ = out.unbounded_send(Event::Value(event));
    })?;
    Ok("done".to_string())
}

fn upload(
    session: &mut Session,
    local: &[PathBuf],
    into: &str,
    report: &Report,
) -> Result<String, String> {
    let mut sent = 0usize;
    let mut unchanged = 0usize;
    for path in local {
        let name = file_name(path)?;
        let target = store::join(into, &name);
        let meta = std::fs::metadata(path).map_err(|err| format!("{}: {err}", path.display()))?;
        if meta.is_dir() {
            session.call::<Done>(Command::FilesMkdir {
                path: target.clone(),
            })?;
            let mut children: Vec<PathBuf> = std::fs::read_dir(path)
                .map_err(|err| format!("{}: {err}", path.display()))?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|child| {
                    child
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| !name.starts_with('.'))
                })
                .collect();
            children.sort();
            let done = upload(session, &children, &target, report)?;
            report.line(format!("{target}/: {done}"));
            continue;
        }
        let size = meta.len();
        let label = format!("sending {target}");
        let result =
            transfer::send_file(session, path, &target, size, mtime_of(&meta), |offset| {
                report.progress(&label, offset, size)
            })?;
        match result {
            Some(_) => {
                sent += 1;
                report.line(format!("sent {target} ({})", protocol::size_label(size)));
            }
            None => unchanged += 1,
        }
    }
    Ok(format!("{sent} sent, {unchanged} already there"))
}

fn download(
    session: &mut Session,
    entry: &FileEntry,
    into: &Path,
    report: &Report,
) -> Result<String, String> {
    let name = entry
        .path
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("files")
        .to_string();
    if entry.kind == FileKind::File {
        let target = into.join(&name);
        let label = format!("receiving {}", entry.path);
        transfer::fetch_file(session, entry, &target, |done, total| {
            report.progress(&label, done, total)
        })?;
        return Ok(format!("saved {}", target.display()));
    }

    let listing: FilesListing = session.call(Command::FilesList {
        path: entry.path.clone(),
        recursive: true,
    })?;
    let base = into.join(&name);
    std::fs::create_dir_all(&base).map_err(|err| format!("{}: {err}", base.display()))?;
    let prefix = format!("{}/", entry.path);
    let mut received = 0usize;
    for item in &listing.entries {
        let Some(relative) = item.path.strip_prefix(&prefix) else {
            continue;
        };
        let target = base.join(relative.split('/').collect::<PathBuf>());
        match item.kind {
            FileKind::Dir => std::fs::create_dir_all(&target)
                .map_err(|err| format!("{}: {err}", target.display()))?,
            FileKind::File => {
                let label = format!("receiving {}", item.path);
                if transfer::fetch_file(session, item, &target, |done, total| {
                    report.progress(&label, done, total)
                })? {
                    received += 1;
                }
            }
        }
    }
    Ok(format!("{received} received into {}", base.display()))
}

/// What `tessaro-ctl update send` does, reporting here instead.
fn update_send(session: &mut Session, update: &Update, report: &Report) -> Result<String, String> {
    let bmap = std::fs::read_to_string(&update.bmap)
        .map_err(|err| format!("{}: {err}", update.bmap.display()))?;
    let name = file_name(&update.image)?;
    let size = std::fs::metadata(&update.image)
        .map_err(|err| format!("{}: {err}", update.image.display()))?
        .len();

    let sha256 = transfer::hash(&update.image, |done| report.progress("hashing", done, size))?;
    report.line(format!("hashed {}", mb(size)));
    let begun: UpdateBegun = session.call(Command::UpdateBegin(ImageUpload {
        name: name.clone(),
        size,
        sha256,
        bmap,
        verify: update.verify,
        repartition: update.repartition,
    }))?;
    if begun.phase == UpdatePhase::Receiving {
        if begun.offset > 0 {
            report.line(format!("resuming at {}", mb(begun.offset)));
        }
        transfer::upload_image(session, &update.image, size, begun.offset, |offset| {
            report.progress("uploading", offset, size)
        })?;
        report.line(format!("uploaded {}", mb(size)));
    } else {
        report.line(format!("the device already has {name}"));
    }

    loop {
        let status: UpdateStatus = session.call(Command::UpdateStatus)?;
        match status.phase {
            UpdatePhase::Verifying => report.progress("verifying", status.verified, status.size),
            UpdatePhase::Preparing => {
                report.progress("preparing", status.prepared, status.to_prepare.max(1))
            }
            UpdatePhase::Ready | UpdatePhase::Pending => break,
            UpdatePhase::Failed => {
                return Err(format!(
                    "the device refused the image: {}",
                    status.error.as_deref().unwrap_or("no reason given")
                ))
            }
            UpdatePhase::Idle | UpdatePhase::Receiving => {
                return Err("the device lost the upload; send it again".to_string())
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    report.line("prepared");

    let done: Done = session.call(Command::UpdateCommit {
        wipe_data: update.wipe_data || update.repartition,
        reboot: update.reboot,
    })?;
    Ok(done.message)
}

fn control_ping(session: &mut Session, count: u32, report: &Report) -> Result<String, String> {
    if let Some(timing) = session.timing {
        report.line(format!(
            "connect {:.1} ms, tls {:.1} ms",
            timing.connect.as_secs_f64() * 1e3,
            timing.handshake.as_secs_f64() * 1e3
        ));
    }
    let mut rtts = Vec::new();
    for seq in 1..=count {
        let started = std::time::Instant::now();
        match session.call::<Done>(Command::Ping) {
            Ok(_) => {
                let rtt = started.elapsed().as_secs_f64() * 1e3;
                rtts.push(rtt);
                report.line(format!("reply {rtt:.1} ms seq={seq}"));
            }
            Err(error) => report.line(format!("no reply seq={seq}: {error}")),
        }
        report.progress("pinging", u64::from(seq), u64::from(count));
        if seq < count {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if rtts.is_empty() {
        return Err("no replies".to_string());
    }
    let average = rtts.iter().sum::<f64>() / rtts.len() as f64;
    let (min, max) = rtts.iter().fold((f64::MAX, 0f64), |(min, max), rtt| {
        (min.min(*rtt), max.max(*rtt))
    });
    Ok(format!(
        "{}/{count} replies, min {min:.1} / avg {average:.1} / max {max:.1} ms",
        rtts.len()
    ))
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{}: no usable file name", path.display()))
}
