//! Putting an image on a device, for `tessaro-ctl update send` and the
//! GUI's Update page alike.
//!
//! The image goes up in acknowledged chunks (`transfer::upload_image`), so
//! a dropped link resumes from the last one the device has - sending it
//! again is the resume. The device checks it while the kiosk keeps running,
//! and writes it at the next boot, from its initramfs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use protocol::api::{self, CommitBody};
use protocol::{ImageUpload, UpdatePhase, UpdateStatus};

use crate::connect::Session;
use crate::report::{clock, percent, step_line, Rate, Report};
use crate::text::{Line, Tone};
use crate::transfer::{self, mb};

/// How long to wait for a device to come back from applying an update:
/// writing a root filesystem to a slow SD card, then a second boot.
pub const COME_BACK: Duration = Duration::from_secs(20 * 60);

/// What to send and what the device does with it.
#[derive(Debug, Clone)]
pub struct Plan {
    pub image: PathBuf,
    /// The block map, usually `transfer::bmap_for(image)`.
    pub bmap: PathBuf,
    /// Re-create /data: every setting, the claim, the browser profile and
    /// the identity go.
    pub wipe_data: bool,
    /// Write the whole disk: partition table, boot, root and /data.
    pub repartition: bool,
    /// Have the device check the whole upload against its SHA-256.
    pub verify: bool,
    /// Reboot into it once it is committed.
    pub reboot: bool,
}

impl Plan {
    /// `/data` goes: asked for, or part of rewriting the whole disk.
    pub fn wipes_data(&self) -> bool {
        self.wipe_data || self.repartition
    }

    /// The image's file name.
    pub fn name(&self) -> Result<String, String> {
        self.image
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
            .ok_or_else(|| format!("{} is not a file", self.image.display()))
    }

    /// What is lost for good, as the end of "This will ...", when it is
    /// more than the root partition: the caller has the user confirm it
    /// the way it confirms anything destructive.
    pub fn warning(&self, name: &str) -> Option<String> {
        if self.repartition {
            Some(format!(
                "rewrite the whole disk with {name} (partition table, boot, root and /data: \
                 every setting, the claim and the identity go, and a power cut while it \
                 writes needs a physical reflash)"
            ))
        } else if self.wipes_data() {
            Some(format!(
                "write {name} and erase /data - every setting, the claim, the browser \
                 profile and the device's identity"
            ))
        } else {
            None
        }
    }
}

/// Where `send` left the device, and what that means for the known nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// Committed, applied at the next reboot, which is left for later.
    Staged,
    /// Rebooting into it: `wait_back` follows it to the end.
    Rebooting,
    /// `/data` is being wiped: the device comes back with a new identity,
    /// so its pin and token on this machine are worthless and go.
    Wiped,
}

/// Hash, upload, have the device check and stage it, commit it.
pub fn send(session: &mut Session, plan: &Plan, report: &mut dyn Report) -> Result<Sent, String> {
    let bmap = std::fs::read_to_string(&plan.bmap)
        .map_err(|err| format!("{}: {err}", plan.bmap.display()))?;
    let name = plan.name()?;
    let size = std::fs::metadata(&plan.image)
        .map_err(|err| format!("{}: {err}", plan.image.display()))?
        .len();
    let node = session.node.name.clone();

    let sha256 = hash(&plan.image, size, report)?;
    stop(report)?;
    let begun = session.send::<api::update::Begin>(ImageUpload {
        name: name.clone(),
        size,
        sha256,
        bmap,
        verify: plan.verify,
        repartition: plan.repartition,
    })?;
    if begun.phase == UpdatePhase::Receiving {
        upload(session, &plan.image, size, begun.offset, report)?;
    } else {
        report.line(Line::of(Tone::Ok, format!("{node} already has {name}")));
    }

    prepare(session, report)?;
    stop(report)?;
    let done = session.send::<api::update::Commit>(CommitBody {
        wipe_data: plan.wipes_data(),
        reboot: plan.reboot,
    })?;
    report.line(Line::of(Tone::Ok, &done.message));

    if !plan.reboot {
        report.line(
            Line::of(Tone::Cmd, "`tessaro-ctl device reboot`")
                .text(" applies it; ")
                .add(Tone::Cmd, "`tessaro-ctl update cancel`")
                .text(" drops it"),
        );
        return Ok(if plan.wipes_data() {
            Sent::Wiped
        } else {
            Sent::Staged
        });
    }
    report.line(Line::of(
        Tone::Warn,
        format!("{node} is rebooting to apply it - this takes a few minutes; do not power it off"),
    ));
    if plan.wipes_data() {
        report.line(
            Line::of(
                Tone::Warn,
                format!("{node} comes back unclaimed, with a new name and certificate:"),
            )
            .text(" find it with ")
            .add(Tone::Cmd, "`tessaro-ctl nodes list`")
            .text(" and claim it again"),
        );
        return Ok(Sent::Wiped);
    }
    Ok(Sent::Rebooting)
}

/// After `Sent::Rebooting`: reconnect until the device answers again, then
/// report what the apply did. `reopen` opens a session to it, pinned.
pub fn wait_back(
    node: &str,
    mut reopen: impl FnMut() -> Result<Session, String>,
    report: &mut dyn Report,
) -> Result<(), String> {
    let started = Instant::now();
    // It takes a moment to go down; answering straight away is the old boot.
    pause(report, Duration::from_secs(10))?;
    let (last, status) = loop {
        report.progress(
            step_line(
                Tone::Label,
                "waiting",
                format!("for {node} to come back ({})", clock(started.elapsed())),
            ),
            0,
            1,
        );
        if let Ok(mut session) = reopen() {
            let update = session.fetch::<api::update::Status>()?;
            let status = session.fetch::<api::device::Status>()?;
            report.line(Line::of(
                Tone::Ok,
                format!("{node} is back after {}", clock(started.elapsed())),
            ));
            break (update.last, status);
        }
        if started.elapsed() > COME_BACK {
            return Err(format!(
                "{node} has not answered for {}; it may still be writing - watch its console, \
                 or try `tessaro-ctl update status` later",
                clock(started.elapsed())
            ));
        }
        pause(report, Duration::from_secs(5))?;
    };
    match last {
        Some(result) if result.applied => {
            report.line(Line::of(Tone::Ok, &result.message));
            if let Some(os) = status.os {
                report.line(Line::of(
                    Tone::Ok,
                    format!(
                        "{node} now runs {os}{}",
                        status
                            .image_version
                            .map(|version| format!(", image {version}"))
                            .unwrap_or_default()
                    ),
                ));
            }
            Ok(())
        }
        Some(result) => Err(format!("the update was not applied: {}", result.message)),
        None => Err(format!(
            "{node} is back but reports no update result; `tessaro-ctl update status` for more"
        )),
    }
}

/// `update status` as lines: where an update is, and how the last one went.
pub fn status_lines(status: &UpdateStatus) -> Vec<Line> {
    let name = || Line::of(Tone::Heading, status.name.as_deref().unwrap_or(""));
    let mut lines = vec![match status.phase {
        UpdatePhase::Idle => Line::of(Tone::Muted, "no update under way"),
        UpdatePhase::Receiving => Line::of(Tone::Warn, "receiving")
            .text(" ")
            .join(name())
            .text(format!(
                ": {} of {} ({}%) - run ",
                mb(status.received),
                mb(status.size),
                percent(status.received, status.size),
            ))
            .add(Tone::Cmd, "`update send`")
            .text(" again to resume"),
        UpdatePhase::Verifying => Line::of(Tone::Warn, "verifying")
            .text(" ")
            .join(name())
            .text(format!(
                ": {} of {} ({}%)",
                mb(status.verified),
                mb(status.size),
                percent(status.verified, status.size)
            )),
        UpdatePhase::Preparing => Line::of(Tone::Warn, "preparing")
            .text(" ")
            .join(name())
            .text(format!(
                ": {} of {} checked ({}%)",
                mb(status.prepared),
                mb(status.to_prepare),
                percent(status.prepared, status.to_prepare)
            )),
        UpdatePhase::Ready => name().text(" ").add(Tone::Ok, "is staged, not committed"),
        UpdatePhase::Pending => {
            let line = name()
                .text(" ")
                .add(Tone::Ok, "is applied at the next boot");
            if status.repartition {
                line.add(Tone::Warn, ", rewriting the whole disk")
            } else if status.wipe_data {
                line.add(Tone::Warn, ", and /data is wiped")
            } else {
                line
            }
        }
        UpdatePhase::Failed => Line::of(Tone::Bad, "failed:").text(format!(
            " {}",
            status.error.as_deref().unwrap_or("no reason given")
        )),
    }];
    if let Some(last) = &status.last {
        let mut line = Line::new()
            .pad(Tone::Label, "last update", 12)
            .text(format!(" {} (", last.message));
        line = if last.applied {
            line.add(Tone::Ok, "applied")
        } else {
            line.add(Tone::Bad, "not applied")
        };
        if last.wiped_data {
            line = line.add(Tone::Warn, ", /data wiped");
        }
        lines.push(line.text(")"));
    }
    lines
}

fn hash(path: &Path, size: u64, report: &mut dyn Report) -> Result<String, String> {
    let sha256 = transfer::hash(path, |done| {
        report.progress(
            step_line(Tone::Label, "hashing", format!("{}/{}", mb(done), mb(size))),
            done,
            size,
        );
    })?;
    report.line(step_line(Tone::Ok, "hashed", mb(size)));
    Ok(sha256)
}

fn upload(
    session: &mut Session,
    path: &Path,
    size: u64,
    from: u64,
    report: &mut dyn Report,
) -> Result<(), String> {
    let resumed = if from > 0 {
        Line::of(Tone::Muted, format!("  (resumed at {})", mb(from)))
    } else {
        Line::new()
    };
    let mut rate = Rate::new(from);
    transfer::upload_image(session, path, size, from, |offset| {
        report.progress(
            step_line(
                Tone::Label,
                "uploading",
                rate.line(offset, size).join(resumed.clone()),
            ),
            offset,
            size,
        );
    })?;
    report.line(step_line(
        Tone::Ok,
        "uploaded",
        format!("{} in {}", mb(size - from), clock(rate.started.elapsed())),
    ));
    Ok(())
}

/// Poll until the device has verified and staged the image, with a rate and
/// an ETA for each step worked out from the device's own progress.
fn prepare(session: &mut Session, report: &mut dyn Report) -> Result<(), String> {
    // The step being shown, and its rate since it started.
    let mut step: Option<(UpdatePhase, Rate)> = None;
    loop {
        stop(report)?;
        let status = session.fetch::<api::update::Status>()?;
        let (label, done, total) = match status.phase {
            UpdatePhase::Verifying => ("verifying", status.verified, status.size),
            UpdatePhase::Preparing => ("preparing", status.prepared, status.to_prepare),
            _ => ("", 0, 0),
        };
        match status.phase {
            UpdatePhase::Verifying | UpdatePhase::Preparing => {
                if step.as_ref().map(|(phase, _)| *phase) != Some(status.phase) {
                    if let Some((UpdatePhase::Verifying, rate)) = &step {
                        report.line(step_line(
                            Tone::Ok,
                            "verified",
                            format!("{} in {}", mb(status.size), clock(rate.started.elapsed())),
                        ));
                    }
                    step = Some((status.phase, Rate::new(done)));
                }
                let rate = &mut step.as_mut().expect("set just above").1;
                if total > 0 {
                    report.progress(
                        step_line(Tone::Label, label, rate.line(done, total)),
                        done,
                        total,
                    );
                } else {
                    report.progress(step_line(Tone::Label, label, "starting"), 0, 1);
                }
                pause(report, Duration::from_secs(1))?;
            }
            UpdatePhase::Ready | UpdatePhase::Pending => {
                report.line(step_line(
                    Tone::Ok,
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
                return Err("the device lost the upload; send it again".to_string())
            }
        }
    }
}

fn stop(report: &dyn Report) -> Result<(), String> {
    if report.stopped() {
        Err("stopped".to_string())
    } else {
        Ok(())
    }
}

/// Sleep, but end early when the user stops the flow.
fn pause(report: &dyn Report, time: Duration) -> Result<(), String> {
    let until = Instant::now() + time;
    while Instant::now() < until {
        stop(report)?;
        std::thread::sleep(Duration::from_millis(200).min(until - Instant::now()));
    }
    stop(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(wipe_data: bool, repartition: bool) -> Plan {
        Plan {
            image: PathBuf::from("/x/tessaro.wic.zst"),
            bmap: PathBuf::from("/x/tessaro.wic.bmap"),
            wipe_data,
            repartition,
            verify: true,
            reboot: true,
        }
    }

    #[test]
    fn only_losing_data_needs_a_warning() {
        assert_eq!(plan(false, false).warning("a"), None);
        assert!(plan(true, false)
            .warning("a")
            .unwrap()
            .contains("erase /data"));
        assert!(plan(false, true)
            .warning("a")
            .unwrap()
            .contains("whole disk"));
        assert!(plan(false, true).wipes_data());
    }

    #[test]
    fn a_pending_wipe_says_so() {
        let status = UpdateStatus {
            phase: UpdatePhase::Pending,
            name: Some("tessaro.wic.zst".to_string()),
            size: 0,
            received: 0,
            verified: 0,
            prepared: 0,
            to_prepare: 0,
            error: None,
            wipe_data: true,
            repartition: false,
            last: None,
        };
        assert_eq!(
            status_lines(&status)[0].to_string(),
            "tessaro.wic.zst is applied at the next boot, and /data is wiped"
        );
    }
}
