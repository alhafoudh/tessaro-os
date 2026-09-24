//! `tessaro-ctl update`: put a new image on a device.
//!
//! The `.wic.bz2` goes up in `UPDATE_CHUNK` pieces, each acknowledged
//! before the next, so a dropped link resumes from the last one the device
//! has - running the same command again is the resume. The device then
//! checks it while the kiosk keeps running, and writes it at the next boot,
//! from its initramfs: to the root partition, or with `--repartition` to the
//! whole disk.
//!
//! Every phase reports progress on stderr: one line that redraws itself on a
//! terminal, one line per step otherwise.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anstream::println;
use protocol::{
    Command, Done, ImageUpload, Status, UpdateBegun, UpdatePhase, UpdateResult, UpdateStatus,
};
use tessaro_client::nodes::Nodes;
use tessaro_client::transfer;

use crate::connect::{self, Session, Target, Trust};
use crate::progress::{clock, mb, percent, step_line, Progress, Rate};
use crate::prompt;
use crate::style::{self, pad, paint};

/// How long to wait for a device to come back from applying an update:
/// writing a root filesystem to a slow SD card, then a second boot.
const COME_BACK: Duration = Duration::from_secs(20 * 60);

/// `update send`, as it is typed.
#[derive(clap::Args)]
pub struct Send {
    pub image: PathBuf,
    /// The block map, if it is not IMAGE without .bz2 plus .bmap.
    #[arg(long)]
    pub bmap: Option<PathBuf>,
    /// Also re-create /data: every setting, the claim, the browser
    /// profile and the device's identity go. It comes back unclaimed.
    #[arg(long)]
    wipe_data: bool,
    /// Write the whole disk - partition table, boot, root and /data - as
    /// `mise run image:flash` would, for a device on another disk layout.
    /// Implies --wipe-data. The device holds the upload in RAM while it
    /// writes, and a power cut before it is done needs a physical
    /// reflash.
    #[arg(long)]
    pub repartition: bool,
    /// Stage and commit it, but leave the reboot for later.
    #[arg(long)]
    pub no_reboot: bool,
    /// Do not wait for the device to come back.
    #[arg(long)]
    pub no_wait: bool,
    /// Skip the device's check of the whole upload against its SHA-256
    /// before preparing it. The bmap's checksums still cover every block
    /// that is written. Needs a device on an image that knows the flag;
    /// an older one checks anyway.
    #[arg(long)]
    pub no_verify: bool,
    #[arg(long, short)]
    pub yes: bool,
}

impl Send {
    /// `/data` goes: asked for, or part of rewriting the whole disk.
    fn wipes_data(&self) -> bool {
        self.wipe_data || self.repartition
    }
}

/// What `send` left for the caller to do with nodes.json.
pub enum Sent {
    /// Nothing changes on this machine.
    Kept,
    /// `/data` is being wiped: the device comes back with a new identity,
    /// so its pin and token here are worthless.
    Wiped,
}

pub fn send(
    session: &mut Session,
    target: &Target,
    nodes: &Nodes,
    options: Send,
    json: bool,
) -> Result<Sent, String> {
    let bmap_path = options
        .bmap
        .clone()
        .unwrap_or_else(|| transfer::bmap_for(&options.image));
    let bmap = std::fs::read_to_string(&bmap_path).map_err(|err| {
        format!(
            "{}: {err} (pass --bmap if it is somewhere else)",
            bmap_path.display()
        )
    })?;
    let name = options
        .image
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| format!("{} is not a file", options.image.display()))?;
    let size = std::fs::metadata(&options.image)
        .map_err(|err| format!("{}: {err}", options.image.display()))?
        .len();

    let node = session.node.name.clone();
    if options.repartition {
        prompt::confirm_destructive(
            session,
            options.yes,
            &format!(
                "rewrite the whole disk with {name} (partition table, boot, root and /data: \
                 every setting, the claim and the identity go, and a power cut while it \
                 writes needs a physical reflash)"
            ),
        )?;
    } else if options.wipes_data() {
        prompt::confirm_destructive(
            session,
            options.yes,
            &format!(
                "write {name} and erase /data - every setting, the claim, the browser \
                 profile and the device's identity"
            ),
        )?;
    } else {
        prompt::confirm(
            options.yes,
            &format!(
                "Write {name} to {node}{}?",
                if options.no_reboot {
                    ""
                } else {
                    " and reboot it"
                }
            ),
        )?;
    }

    let mut progress = Progress::new(json);
    let sha256 = hash(&options.image, size, &mut progress)?;

    let begun: UpdateBegun = session.call(Command::UpdateBegin(ImageUpload {
        name: name.clone(),
        size,
        sha256,
        bmap,
        verify: !options.no_verify,
        repartition: options.repartition,
    }))?;
    if begun.phase == UpdatePhase::Receiving {
        upload(session, &options.image, size, begun.offset, &mut progress)?;
    } else {
        progress.done(&paint(style::OK, format!("{node} already has {name}")));
    }

    prepare(session, &mut progress)?;
    let reboot = !options.no_reboot;
    let done: Done = session.call(Command::UpdateCommit {
        wipe_data: options.wipes_data(),
        reboot,
    })?;
    progress.done(&paint(style::OK, &done.message));

    if !reboot {
        progress.done(&format!(
            "{} applies it; {} drops it",
            paint(style::CMD, "`tessaro-ctl device reboot`"),
            paint(style::CMD, "`tessaro-ctl update cancel`")
        ));
        return Ok(if options.wipes_data() {
            Sent::Wiped
        } else {
            Sent::Kept
        });
    }
    progress.done(&paint(
        style::WARN,
        format!("{node} is rebooting to apply it - this takes a few minutes; do not power it off"),
    ));

    if options.wipes_data() {
        progress.done(&format!(
            "{} find it with {} and claim it again",
            paint(
                style::WARN,
                format!("{node} comes back unclaimed, with a new name and certificate:")
            ),
            paint(style::CMD, "`tessaro-ctl nodes list`")
        ));
        return Ok(Sent::Wiped);
    }
    if options.no_wait || matches!(target, Target::Local(_)) {
        return Ok(Sent::Kept);
    }

    let (result, status) = wait_for(target, nodes, &node, &mut progress)?;
    match result {
        Some(result) if result.applied => {
            progress.done(&paint(style::OK, &result.message));
            if let Some(os) = status.os {
                progress.done(&paint(
                    style::OK,
                    format!(
                        "{node} now runs {os}{}",
                        status
                            .image_version
                            .map(|version| format!(", image {version}"))
                            .unwrap_or_default()
                    ),
                ));
            }
            Ok(Sent::Kept)
        }
        Some(result) => Err(format!("the update was not applied: {}", result.message)),
        None => Err(format!(
            "{node} is back but reports no update result; `tessaro-ctl update status` for more"
        )),
    }
}

pub fn status(session: &mut Session, json: bool) -> Result<(), String> {
    let status: UpdateStatus = session.call(Command::UpdateStatus)?;
    crate::print(json, &status, || show(&status))
}

pub fn cancel(session: &mut Session, json: bool) -> Result<(), String> {
    let done: Done = session.call(Command::UpdateCancel)?;
    crate::print(json, &done, || println!("{}", done.message))
}

fn show(status: &UpdateStatus) {
    let name = paint(style::HEADING, status.name.as_deref().unwrap_or(""));
    match status.phase {
        UpdatePhase::Idle => println!("{}", paint(style::MUTED, "no update under way")),
        UpdatePhase::Receiving => println!(
            "{} {name}: {} of {} ({}%) - run {} again to resume",
            paint(style::WARN, "receiving"),
            mb(status.received),
            mb(status.size),
            percent(status.received, status.size),
            paint(style::CMD, "`update send`")
        ),
        UpdatePhase::Verifying => println!(
            "{} {name}: {} of {} ({}%)",
            paint(style::WARN, "verifying"),
            mb(status.verified),
            mb(status.size),
            percent(status.verified, status.size)
        ),
        UpdatePhase::Preparing => println!(
            "{} {name}: {} of {} checked ({}%)",
            paint(style::WARN, "preparing"),
            mb(status.prepared),
            mb(status.to_prepare),
            percent(status.prepared, status.to_prepare)
        ),
        UpdatePhase::Ready => println!("{name} {}", paint(style::OK, "is staged, not committed")),
        UpdatePhase::Pending => println!(
            "{name} {}{}",
            paint(style::OK, "is applied at the next boot"),
            if status.repartition {
                paint(style::WARN, ", rewriting the whole disk")
            } else if status.wipe_data {
                paint(style::WARN, ", and /data is wiped")
            } else {
                String::new()
            }
        ),
        UpdatePhase::Failed => println!(
            "{} {}",
            paint(style::BAD, "failed:"),
            status.error.as_deref().unwrap_or("no reason given")
        ),
    }
    if let Some(last) = &status.last {
        println!(
            "{} {} ({}{})",
            pad(style::LABEL, "last update", 12),
            last.message,
            if last.applied {
                paint(style::OK, "applied")
            } else {
                paint(style::BAD, "not applied")
            },
            if last.wiped_data {
                paint(style::WARN, ", /data wiped")
            } else {
                String::new()
            }
        );
    }
}

fn hash(path: &Path, size: u64, progress: &mut Progress) -> Result<String, String> {
    let sha256 = transfer::hash(path, |done| {
        progress.show(
            &step_line(
                style::LABEL,
                "hashing",
                format!("{}/{}", mb(done), mb(size)),
            ),
            done,
            size,
        );
    })?;
    progress.done(&step_line(style::OK, "hashed", mb(size)));
    Ok(sha256)
}

fn upload(
    session: &mut Session,
    path: &Path,
    size: u64,
    from: u64,
    progress: &mut Progress,
) -> Result<(), String> {
    let resumed = if from > 0 {
        paint(style::MUTED, format!("  (resumed at {})", mb(from)))
    } else {
        String::new()
    };
    let mut rate = Rate::new(from);
    transfer::upload_image(session, path, size, from, |offset| {
        progress.show(
            &step_line(
                style::LABEL,
                "uploading",
                format!("{}{resumed}", rate.line(offset, size)),
            ),
            offset,
            size,
        );
    })?;
    progress.done(&step_line(
        style::OK,
        "uploaded",
        format!("{} in {}", mb(size - from), clock(rate.started.elapsed())),
    ));
    Ok(())
}

/// Poll until the device has verified and staged the image, with a rate and
/// an ETA for each step worked out from the device's own progress.
fn prepare(session: &mut Session, progress: &mut Progress) -> Result<(), String> {
    // The step being shown, and its rate since it started.
    let mut step: Option<(UpdatePhase, Rate)> = None;
    loop {
        let status: UpdateStatus = session.call(Command::UpdateStatus)?;
        let (label, done, total) = match status.phase {
            UpdatePhase::Verifying => ("verifying", status.verified, status.size),
            UpdatePhase::Preparing => ("preparing", status.prepared, status.to_prepare),
            _ => ("", 0, 0),
        };
        match status.phase {
            UpdatePhase::Verifying | UpdatePhase::Preparing => {
                if step.as_ref().map(|(phase, _)| *phase) != Some(status.phase) {
                    if let Some((UpdatePhase::Verifying, rate)) = &step {
                        progress.done(&step_line(
                            style::OK,
                            "verified",
                            format!("{} in {}", mb(status.size), clock(rate.started.elapsed())),
                        ));
                    }
                    step = Some((status.phase, Rate::new(done)));
                }
                let rate = &mut step.as_mut().expect("set just above").1;
                if total > 0 {
                    progress.show(
                        &step_line(style::LABEL, label, rate.line(done, total)),
                        done,
                        total,
                    );
                } else {
                    progress.show(&step_line(style::LABEL, label, "starting"), 0, 1);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            UpdatePhase::Ready | UpdatePhase::Pending => {
                progress.done(&step_line(
                    style::OK,
                    "prepared",
                    format!("{} of the image checked", mb(status.to_prepare)),
                ));
                return Ok(());
            }
            UpdatePhase::Failed => {
                return Err(format!(
                    "the device refused the image: {}",
                    status.error.as_deref().unwrap_or("no reason given")
                ))
            }
            UpdatePhase::Idle | UpdatePhase::Receiving => {
                return Err("the device lost the upload; run the same command again".to_string())
            }
        }
    }
}

/// Reconnect until the device answers again, then read what the apply did.
fn wait_for(
    target: &Target,
    nodes: &Nodes,
    node: &str,
    progress: &mut Progress,
) -> Result<(Option<UpdateResult>, Status), String> {
    let started = Instant::now();
    // It takes a moment to go down; answering straight away is the old boot.
    std::thread::sleep(Duration::from_secs(10));
    loop {
        progress.show(
            &step_line(
                style::LABEL,
                "waiting",
                format!("for {node} to come back ({})", clock(started.elapsed())),
            ),
            0,
            1,
        );
        if let Ok(mut session) = connect::open(target, nodes, Trust::KnownOnly) {
            let update: UpdateStatus = session.call(Command::UpdateStatus)?;
            let status: Status = session.call(Command::Status)?;
            progress.done(&paint(
                style::OK,
                format!("{node} is back after {}", clock(started.elapsed())),
            ));
            return Ok((update.last, status));
        }
        if started.elapsed() > COME_BACK {
            return Err(format!(
                "{node} has not answered for {}; it may still be writing - watch its console, \
                 or try `tessaro-ctl update status` later",
                clock(started.elapsed())
            ));
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}
