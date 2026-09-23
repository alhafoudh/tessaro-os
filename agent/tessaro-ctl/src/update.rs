//! `tessaro-ctl update`: put a new image on a device.
//!
//! The `.wic.bz2` goes up in `UPDATE_CHUNK` pieces, each acknowledged
//! before the next, so a dropped link resumes from the last one the device
//! has - running the same command again is the resume. The device then
//! checks and stages it while the kiosk keeps running, and applies it at the
//! next boot, from its initramfs.
//!
//! Every phase reports progress on stderr: one line that redraws itself on a
//! terminal, one line per step otherwise.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{IsTerminal, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use protocol::{
    Command, Done, Status, UpdateBegun, UpdatePhase, UpdateReceived, UpdateResult, UpdateStatus,
};
use sha2::{Digest, Sha256};

use crate::connect::{self, Session, Target, Trust};
use crate::nodes::Nodes;

/// How long to wait for a device to come back from applying an update:
/// writing a root filesystem to a slow SD card, then a second boot.
const COME_BACK: Duration = Duration::from_secs(20 * 60);

pub struct Send {
    pub image: PathBuf,
    pub bmap: Option<PathBuf>,
    pub wipe_data: bool,
    pub no_reboot: bool,
    pub no_wait: bool,
    pub no_verify: bool,
    pub yes: bool,
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
        .unwrap_or_else(|| bmap_for(&options.image));
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
    if options.wipe_data {
        crate::confirm_destructive(
            session,
            options.yes,
            &format!(
                "write {name} and erase /data - every setting, the claim, the browser \
                 profile and the device's identity"
            ),
        )?;
    } else if !options.yes
        && !connect::ask(&format!(
            "Write {name} to {node}{}?",
            if options.no_reboot {
                ""
            } else {
                " and reboot it"
            }
        ))?
    {
        return Err("not confirmed".to_string());
    }

    let mut progress = Progress::new(json);
    let sha256 = hash(&options.image, size, &mut progress)?;

    let begun: UpdateBegun = call(
        session,
        Command::UpdateBegin {
            name: name.clone(),
            size,
            sha256,
            bmap,
            verify: !options.no_verify,
        },
    )?;
    if begun.phase == UpdatePhase::Receiving {
        upload(session, &options.image, size, begun.offset, &mut progress)?;
    } else {
        progress.done(&format!("{node} already has {name}"));
    }

    prepare(session, &mut progress)?;
    let reboot = !options.no_reboot;
    let done: Done = call(
        session,
        Command::UpdateCommit {
            wipe_data: options.wipe_data,
            reboot,
        },
    )?;
    progress.done(&done.message);

    if !reboot {
        progress.done("`tessaro-ctl reboot` applies it; `tessaro-ctl update cancel` drops it");
        return Ok(if options.wipe_data {
            Sent::Wiped
        } else {
            Sent::Kept
        });
    }
    progress.done(&format!(
        "{node} is rebooting to apply it - this takes a few minutes; do not power it off"
    ));

    if options.wipe_data {
        progress.done(&format!(
            "{node} comes back unclaimed, with a new name and certificate: \
             find it with `tessaro-ctl nodes` and claim it again"
        ));
        return Ok(Sent::Wiped);
    }
    if options.no_wait || matches!(target, Target::Local(_)) {
        return Ok(Sent::Kept);
    }

    let (result, status) = wait_for(target, nodes, &node, &mut progress)?;
    match result {
        Some(result) if result.applied => {
            progress.done(&result.message);
            if let Some(os) = status.os {
                progress.done(&format!(
                    "{node} now runs {os}{}",
                    status
                        .image_version
                        .map(|version| format!(", image {version}"))
                        .unwrap_or_default()
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
    let status: UpdateStatus = call(session, Command::UpdateStatus)?;
    crate::print(json, &status, || show(&status))
}

pub fn cancel(session: &mut Session, json: bool) -> Result<(), String> {
    let done: Done = call(session, Command::UpdateCancel)?;
    crate::print(json, &done, || println!("{}", done.message))
}

fn show(status: &UpdateStatus) {
    let name = status.name.as_deref().unwrap_or("");
    match status.phase {
        UpdatePhase::Idle => println!("no update under way"),
        UpdatePhase::Receiving => println!(
            "receiving {name}: {} of {} ({}%) - run `update send` again to resume",
            mb(status.received),
            mb(status.size),
            percent(status.received, status.size)
        ),
        UpdatePhase::Verifying => println!(
            "verifying {name}: {} of {} ({}%)",
            mb(status.verified),
            mb(status.size),
            percent(status.verified, status.size)
        ),
        UpdatePhase::Preparing => println!(
            "preparing {name}: {} of {} checked ({}%)",
            mb(status.prepared),
            mb(status.to_prepare),
            percent(status.prepared, status.to_prepare)
        ),
        UpdatePhase::Ready => println!("{name} is staged, not committed"),
        UpdatePhase::Pending => println!(
            "{name} is applied at the next boot{}",
            if status.wipe_data {
                ", and /data is wiped"
            } else {
                ""
            }
        ),
        UpdatePhase::Failed => println!(
            "failed: {}",
            status.error.as_deref().unwrap_or("no reason given")
        ),
    }
    if let Some(last) = &status.last {
        println!(
            "last update  {} ({}{})",
            last.message,
            if last.applied {
                "applied"
            } else {
                "not applied"
            },
            if last.wiped_data { ", /data wiped" } else { "" }
        );
    }
}

/// `x.rootfs.wic.bz2` -> `x.rootfs.wic.bmap`, the same rule as `image:flash`.
fn bmap_for(image: &Path) -> PathBuf {
    let text = image.to_string_lossy();
    let base = text.strip_suffix(".bz2").unwrap_or(&text);
    PathBuf::from(format!("{base}.bmap"))
}

fn hash(path: &Path, size: u64, progress: &mut Progress) -> Result<String, String> {
    let mut file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| format!("{}: {err}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        done += read as u64;
        progress.show(&format!("hashing    {}/{}", mb(done), mb(size)), done, size);
    }
    progress.done(&format!("hashed     {}", mb(size)));
    Ok(connect::hex(&hasher.finalize()))
}

fn upload(
    session: &mut Session,
    path: &Path,
    size: u64,
    from: u64,
    progress: &mut Progress,
) -> Result<(), String> {
    let mut file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    file.seek(SeekFrom::Start(from))
        .map_err(|err| format!("{}: {err}", path.display()))?;
    let resumed = if from > 0 {
        format!("  (resumed at {})", mb(from))
    } else {
        String::new()
    };
    let mut rate = Rate::new(from);
    let mut buffer = vec![0u8; protocol::UPDATE_CHUNK];
    let mut offset = from;

    while offset < size {
        let want = ((size - offset) as usize).min(buffer.len());
        file.read_exact(&mut buffer[..want])
            .map_err(|err| format!("{}: {err}", path.display()))?;
        let data = data_encoding::BASE64.encode(&buffer[..want]);
        let received: UpdateReceived = call(session, Command::UpdateChunk { offset, data })
            .map_err(|err| {
                format!(
                    "the upload stopped at {}: {err}; run the same command again to resume",
                    mb(offset)
                )
            })?;
        offset = received.received;
        let (speed, eta) = rate.update(offset, size);
        progress.show(
            &format!(
                "uploading  {}/{}  {:>3}%  {}/s  ETA {}{resumed}",
                mb(offset),
                mb(size),
                percent(offset, size),
                mb(speed as u64),
                eta
            ),
            offset,
            size,
        );
    }
    progress.done(&format!(
        "uploaded   {} in {}",
        mb(size - from),
        clock(rate.started.elapsed())
    ));
    Ok(())
}

/// Poll until the device has verified and staged the image, with a rate and
/// an ETA for each step worked out from the device's own progress.
fn prepare(session: &mut Session, progress: &mut Progress) -> Result<(), String> {
    // The step being shown, and its rate since it started.
    let mut step: Option<(UpdatePhase, Rate)> = None;
    loop {
        let status: UpdateStatus = call(session, Command::UpdateStatus)?;
        let (label, done, total) = match status.phase {
            UpdatePhase::Verifying => ("verifying", status.verified, status.size),
            UpdatePhase::Preparing => ("preparing", status.prepared, status.to_prepare),
            _ => ("", 0, 0),
        };
        match status.phase {
            UpdatePhase::Verifying | UpdatePhase::Preparing => {
                if step.as_ref().map(|(phase, _)| *phase) != Some(status.phase) {
                    if let Some((UpdatePhase::Verifying, rate)) = &step {
                        progress.done(&format!(
                            "verified   {} in {}",
                            mb(status.size),
                            clock(rate.started.elapsed())
                        ));
                    }
                    step = Some((status.phase, Rate::new(done)));
                }
                let rate = &mut step.as_mut().expect("set just above").1;
                if total > 0 {
                    let (speed, eta) = rate.update(done, total);
                    progress.show(
                        &format!(
                            "{label:<10} {}/{}  {:>3}%  {}/s  ETA {eta}",
                            mb(done),
                            mb(total),
                            percent(done, total),
                            mb(speed as u64),
                        ),
                        done,
                        total,
                    );
                } else {
                    progress.show(&format!("{label:<10} starting"), 0, 1);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            UpdatePhase::Ready | UpdatePhase::Pending => {
                progress.done(&format!(
                    "prepared   {} of the image checked and staged",
                    mb(status.to_prepare)
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
            &format!(
                "waiting    for {node} to come back ({})",
                clock(started.elapsed())
            ),
            0,
            1,
        );
        if let Ok(mut session) = connect::open(target, nodes, Trust::KnownOnly, false) {
            let update: UpdateStatus = call(&mut session, Command::UpdateStatus)?;
            let status: Status = call(&mut session, Command::Status)?;
            progress.done(&format!(
                "{node} is back after {}",
                clock(started.elapsed())
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

fn call<T: serde::de::DeserializeOwned>(
    session: &mut Session,
    command: Command,
) -> Result<T, String> {
    let value = session.call(command)?;
    serde_json::from_value(value).map_err(|err| format!("unexpected answer: {err}"))
}

/// Progress on stderr. On a terminal one line redraws itself; otherwise,
/// and with `--json`, a line per tenth, so a log stays readable.
struct Progress {
    redraw: bool,
    shown: Option<u64>,
}

impl Progress {
    fn new(json: bool) -> Self {
        Self {
            redraw: !json && std::io::stderr().is_terminal(),
            shown: None,
        }
    }

    fn show(&mut self, line: &str, done: u64, total: u64) {
        let mut stderr = std::io::stderr();
        if self.redraw {
            let _ = write!(stderr, "\r{line}\x1b[K");
            let _ = stderr.flush();
            return;
        }
        let tenth = done * 10 / total.max(1);
        if self.shown != Some(tenth) {
            self.shown = Some(tenth);
            let _ = writeln!(stderr, "{line}");
        }
    }

    /// A step is over: its last line stays.
    fn done(&mut self, line: &str) {
        let mut stderr = std::io::stderr();
        if self.redraw {
            let _ = writeln!(stderr, "\r{line}\x1b[K");
        } else {
            let _ = writeln!(stderr, "{line}");
        }
        self.shown = None;
    }
}

/// Upload speed over the last few seconds, and what it means for the rest.
struct Rate {
    started: Instant,
    samples: VecDeque<(Instant, u64)>,
}

impl Rate {
    const WINDOW: Duration = Duration::from_secs(5);

    fn new(from: u64) -> Self {
        let now = Instant::now();
        Self {
            started: now,
            samples: VecDeque::from([(now, from)]),
        }
    }

    /// Bytes per second, and the time left as text.
    fn update(&mut self, done: u64, total: u64) -> (f64, String) {
        let now = Instant::now();
        self.samples.push_back((now, done));
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > Self::WINDOW {
            self.samples.pop_front();
        }
        let (then, before) = self.samples[0];
        let seconds = now.duration_since(then).as_secs_f64();
        if seconds <= 0.0 || done <= before {
            return (0.0, "--:--".to_string());
        }
        let speed = (done - before) as f64 / seconds;
        let left = Duration::from_secs_f64(total.saturating_sub(done) as f64 / speed);
        (speed, clock(left))
    }
}

fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

fn percent(done: u64, total: u64) -> u64 {
    done * 100 / total.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bmap_is_found_next_to_the_image() {
        assert_eq!(
            bmap_for(Path::new("out/tessaro-os-qemux86-64.rootfs.wic.bz2")),
            PathBuf::from("out/tessaro-os-qemux86-64.rootfs.wic.bmap")
        );
        assert_eq!(bmap_for(Path::new("x.wic")), PathBuf::from("x.wic.bmap"));
    }

    #[test]
    fn clocks() {
        assert_eq!(clock(Duration::from_secs(75)), "1:15");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn the_rate_needs_two_samples() {
        let mut rate = Rate::new(0);
        rate.samples[0].0 -= Duration::from_secs(2);
        let (speed, eta) = rate.update(2_000_000, 10_000_000);
        assert!((900_000.0..1_100_000.0).contains(&speed), "{speed}");
        assert_eq!(eta, "0:08");
    }
}
