//! Image updates: the half that runs while the kiosk does.
//!
//! `update-begin` describes a `.wic.zst` and its bmap, `update-chunk`s
//! append it to `/data/tessaro/update/upload.part`, and once the last one is
//! in, a background thread checks the whole file against its SHA-256 and
//! decompresses it once as a dry run (`update::prepare`). The upload stays
//! as it is: the initramfs decompresses it again to write it.
//! `update-commit` writes the marker the initramfs acts on at the next boot.
//! Everything the agent knows about an update is on disk, so a restart - of
//! the agent, or of the device before the commit - resumes where it was: an
//! upload from its last byte, a preparation from the start.
//!
//! The partition table is checked as soon as the first chunk is in, so an
//! image for the wrong machine or the old disk layout fails in seconds
//! rather than after the whole upload. With `repartition` the layout may
//! differ - the whole disk is rewritten - but the image has to fit on it,
//! and the upload has to fit in RAM, since the initramfs copies it there
//! before it overwrites the `/data` it is on.
//!
//! Preparing reads several hundred megabytes, so it runs on a thread of its
//! own at idle CPU and I/O priority, not on the blocking pool: the pool's
//! threads are shared with every deadline-bound disk call, and this one has
//! no deadline - it is watched through `update-status` instead.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use protocol::{Done, ImageUpload, Received, UpdateBegun, UpdatePhase, UpdateResult, UpdateStatus};
use update::image::Image;
use update::manifest::{Manifest, Mode, Outcome, Pending, Source, Upload};
use update::{bmap, flash, fsutil, layout, megabytes, prepare, ptable};

use crate::deadline::blocking;
use crate::log::Log;
use crate::paths::Paths;
use crate::sync::lock;

/// Beyond the upload: the manifest, and room for the kiosk to keep writing
/// its profile meanwhile.
const MARGIN: u64 = 128 << 20;

/// A root update also stages the image's boot partition, sparse, to copy
/// the kernel out of: at most its mapped bytes, and the kernel once more.
/// This is the room for a device whose boot partition cannot be read: the
/// largest boot partition, the Pi's 512M. Otherwise `boot_room` measures it.
const BOOT_ROOM: u64 = 512 << 20;

/// On top of twice the boot partition's mapped bytes: the kernel copy's own
/// filesystem blocks and rounding.
const BOOT_SLACK: u64 = 8 << 20;

/// What a root update's boot staging takes on /data: the image's boot
/// partition as the bmap maps it - a few tens of MB, not the partition's
/// size, since the staging is sparse - and the kernel copied out of it,
/// which is part of those mapped bytes, so twice them covers both. The
/// image's boot partition is where this device's is (`layout::check`
/// refuses anything else), so the device's own offsets say where to count.
/// Measured rather than assumed because the image's /data is 1 GB until
/// `storage grow`: the fixed 512M left an update no room on it.
fn boot_room(probe: &layout::Probe, bmap: &bmap::Bmap) -> u64 {
    match layout::probe(probe) {
        Ok(device) => {
            let end = device.boot.start + device.boot.size;
            2 * bmap.mapped_within(device.boot.start, end) + BOOT_SLACK
        }
        Err(_) => BOOT_ROOM,
    }
}

/// RAM the running system needs beyond what the initramfs of a disk update
/// takes: the kernel, the initramfs itself, and whatever the firmware keeps.
/// MemTotal is what the initramfs will see, less about this.
const RAM_RESERVED: u64 = 128 << 20;

/// How much must have arrived before the partition table is first looked
/// for in the compressed upload. Enough for bz2, whose 900 kB streams come out
/// whole; zstd often needs more, because ruzstd holds back a whole window
/// (8 MiB at level 19) before it hands out the first byte, and the boot
/// partition that follows the table barely compresses. So the check is
/// repeated at every chunk until the table comes out.
const HEAD_CHECK: u64 = 4 << 20;

/// Past this the head check stops being retried and is skipped: each try
/// decompresses the upload from its start, and the dry run checks the
/// partition table anyway, only later.
const HEAD_CHECK_LIMIT: u64 = 64 << 20;

/// Copies the kernel named `name` out of the staged boot partition image.
type Extract = fn(&Path, &Path, &str) -> Result<(), String>;

pub struct Updates {
    paths: Paths,
    log: Arc<Log>,
    job: Arc<Mutex<Job>>,
    loaded: AtomicBool,
    extract: Extract,
}

#[derive(Debug, Clone)]
struct Job {
    phase: UpdatePhase,
    upload: Option<Upload>,
    received: u64,
    verified: u64,
    prepared: u64,
    to_prepare: u64,
    error: Option<String>,
    wipe_data: bool,
    /// The staged image, once prepared: its name and SHA-256.
    staged: Option<(String, String)>,
    /// What the upload under way, or the staged one, writes.
    mode: Mode,
    head_checked: bool,
    cancel: Arc<AtomicBool>,
}

impl Default for Job {
    fn default() -> Self {
        Self {
            phase: UpdatePhase::Idle,
            upload: None,
            received: 0,
            verified: 0,
            prepared: 0,
            to_prepare: 0,
            error: None,
            wipe_data: false,
            staged: None,
            mode: Mode::Root,
            head_checked: false,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Updates {
    pub fn new(log: Arc<Log>, paths: Paths) -> Arc<Self> {
        Self::with_extract(log, paths, loop_mount_kernel)
    }

    fn with_extract(log: Arc<Log>, paths: Paths, extract: Extract) -> Arc<Self> {
        Arc::new(Self {
            paths,
            log,
            job: Arc::new(Mutex::new(Job::default())),
            loaded: AtomicBool::new(false),
            extract,
        })
    }

    /// Pick up whatever the staging directory holds. Called at startup, so
    /// a preparation the agent was restarted in the middle of carries on.
    pub async fn load(self: &Arc<Self>) {
        if self.loaded.load(Ordering::SeqCst) {
            return;
        }
        let dir = self.paths.update_dir();
        let found = blocking("reading the update staging", move || Ok(scan(&dir)))
            .await
            .unwrap_or_default();
        if self.loaded.swap(true, Ordering::SeqCst) {
            return;
        }
        let resume = working(found.phase);
        let name = found.upload.as_ref().map(|upload| upload.name.clone());
        *lock(&self.job) = found;
        if resume {
            self.log.info(format!(
                "update: resuming the preparation of {}",
                name.unwrap_or_default()
            ));
            self.spawn_prepare();
        }
    }

    pub async fn begin(
        self: &Arc<Self>,
        caller: &str,
        upload: ImageUpload,
    ) -> Result<UpdateBegun, String> {
        self.load().await;
        check_name(&upload.name)?;
        let upload = Upload {
            name: upload.name,
            size: upload.size,
            sha256: upload.sha256.to_ascii_lowercase(),
            bmap: upload.bmap,
            verify: upload.verify,
            repartition: upload.repartition,
        };
        let (name, size, sha256) = (upload.name.clone(), upload.size, upload.sha256.clone());
        if sha256.len() != 64 || !sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err("sha256 must be 64 hex digits".to_string());
        }
        if size == 0 {
            return Err("the file is empty".to_string());
        }
        let parsed = bmap::parse(&upload.bmap)?;

        {
            let mut job = lock(&self.job);
            let same_upload = job.upload.as_ref().is_some_and(|known| {
                known.sha256 == sha256
                    && known.size == size
                    && known.bmap == upload.bmap
                    && known.repartition == upload.repartition
            });
            let same_staged = job.staged.as_ref().is_some_and(|(_, sha)| *sha == sha256)
                && job.mode == upload.mode();
            match job.phase {
                UpdatePhase::Pending => {
                    return Err(format!(
                        "{} is already committed for the next boot; \
                         `tessaro-ctl update cancel` drops it",
                        job.staged
                            .as_ref()
                            .map(|(name, _)| name.as_str())
                            .unwrap_or("an update")
                    ))
                }
                phase if working(phase) && same_upload => {
                    return Ok(UpdateBegun {
                        offset: size,
                        phase,
                    })
                }
                phase if working(phase) => {
                    return Err(format!(
                        "the device is checking {}; wait for it, or `tessaro-ctl update cancel`",
                        job.upload
                            .as_ref()
                            .map(|upload| upload.name.as_str())
                            .unwrap_or("an image")
                    ))
                }
                UpdatePhase::Ready if same_staged => {
                    return Ok(UpdateBegun {
                        offset: size,
                        phase: UpdatePhase::Ready,
                    })
                }
                UpdatePhase::Receiving if same_upload => {
                    self.log.info(format!(
                        "update: {caller} resumes {name} at {}",
                        megabytes(job.received)
                    ));
                    // The resuming client decides about the whole-file check.
                    // Not persisted: an agent restart before the last chunk
                    // falls back to what the upload began with.
                    if let Some(known) = job.upload.as_mut() {
                        known.verify = upload.verify;
                    }
                    return Ok(UpdateBegun {
                        offset: job.received,
                        phase: UpdatePhase::Receiving,
                    });
                }
                _ => {}
            }
        }

        let dir = self.paths.update_dir();
        let meminfo = self.paths.meminfo.clone();
        let meta = upload.clone();
        let mode = upload.mode();
        let probe = self.probe();
        blocking("starting the upload", move || {
            if mode == Mode::Disk {
                fits_in_ram(&meminfo, size)?;
            }
            let needed = match mode {
                Mode::Root => size + boot_room(&probe, &parsed) + MARGIN,
                Mode::Disk => size + MARGIN,
            };
            fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
            flash::clean(&dir, &mut io::sink());
            let free =
                fsutil::available(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
            if free < needed {
                return Err(format!(
                    "/data has {} free; this update needs {}",
                    megabytes(free),
                    megabytes(needed)
                ));
            }
            fsutil::write_json(&dir.join(update::UPLOAD_META), &meta)
                .map_err(|err| format!("writing the upload's description: {err}"))?;
            File::create(dir.join(update::UPLOAD))
                .map_err(|err| format!("creating the upload: {err}"))?;
            Ok(())
        })
        .await?;

        *lock(&self.job) = Job {
            phase: UpdatePhase::Receiving,
            upload: Some(upload),
            mode,
            ..Job::default()
        };
        self.log.info(format!(
            "update: receiving {name} ({}) from {caller}{}",
            megabytes(size),
            if mode == Mode::Disk {
                ", to rewrite the whole disk"
            } else {
                ""
            }
        ));
        Ok(UpdateBegun {
            offset: 0,
            phase: UpdatePhase::Receiving,
        })
    }

    pub async fn chunk(self: &Arc<Self>, offset: u64, data: String) -> Result<Received, String> {
        self.load().await;
        let (upload, received, head_checked) = {
            let job = lock(&self.job);
            match (job.phase, &job.upload) {
                (UpdatePhase::Receiving, Some(upload)) => {
                    (upload.clone(), job.received, job.head_checked)
                }
                (UpdatePhase::Failed, _) => {
                    return Err(job
                        .error
                        .clone()
                        .unwrap_or_else(|| "the update failed".to_string()))
                }
                (UpdatePhase::Idle, _) | (UpdatePhase::Receiving, None) => {
                    return Err("no upload is under way; `update-begin` starts one".to_string())
                }
                _ => return Err("the upload is already complete".to_string()),
            }
        };
        if offset != received {
            return Err(format!(
                "the device has {received} bytes of {}; send from there",
                upload.name
            ));
        }
        let bytes = protocol::decode_chunk(&data)?;
        if received + bytes.len() as u64 > upload.size {
            return Err("the chunk runs past the end of the file".to_string());
        }

        let path = self.paths.update_dir().join(update::UPLOAD);
        let this = Arc::clone(self);
        let now = blocking("writing the upload", move || {
            let mut file = OpenOptions::new()
                .append(true)
                .open(&path)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            let length = file.metadata().map_err(|err| err.to_string())?.len();
            if length != offset {
                return Err(format!(
                    "the upload on disk is {length} bytes, not {offset}; begin again to resume"
                ));
            }
            // Synced per chunk, so a power cut costs at most the one in flight.
            file.write_all(&bytes)
                .and_then(|()| file.sync_data())
                .map_err(|err| format!("writing the upload: {err}"))?;
            let now = length + bytes.len() as u64;
            // Here and not after the await: a client that drops the
            // connection cancels this request, but not the write, and a
            // count left behind the file would refuse every resume.
            let mut job = lock(&this.job);
            if job.phase == UpdatePhase::Receiving {
                job.received = now;
            }
            Ok(now)
        })
        .await?;
        let tenth = |bytes: u64| bytes * 10 / upload.size;
        if tenth(now) > tenth(received) && now < upload.size {
            self.log.info(format!(
                "update: received {} of {} ({}%)",
                megabytes(now),
                megabytes(upload.size),
                tenth(now) * 10
            ));
        }

        if !head_checked && now >= HEAD_CHECK.min(upload.size) {
            let dir = self.paths.update_dir();
            let probe = self.probe();
            let meta = upload.clone();
            let check = blocking("reading the image's partition table", move || {
                Ok(check_head(&dir.join(update::UPLOAD), &probe, &meta))
            })
            .await?;
            match check {
                Ok(true) => lock(&self.job).head_checked = true,
                Ok(false) if now >= HEAD_CHECK_LIMIT.min(upload.size) => {
                    lock(&self.job).head_checked = true
                }
                Ok(false) => {}
                Err(err) => {
                    self.fail(&err).await;
                    return Err(err);
                }
            }
        }

        if now == upload.size {
            self.log.info(format!(
                "update: received all of {}; preparing it",
                upload.name
            ));
            self.spawn_prepare();
        }
        Ok(Received {
            received: now,
            size: upload.size,
        })
    }

    pub async fn status(self: &Arc<Self>) -> Result<UpdateStatus, String> {
        self.load().await;
        let path = self.paths.update_dir().join(update::RESULT);
        let last = blocking("reading the last update's result", move || {
            Ok(fsutil::read_json::<Outcome>(&path).ok())
        })
        .await?;
        let job = lock(&self.job);
        Ok(UpdateStatus {
            phase: job.phase,
            name: job
                .upload
                .as_ref()
                .map(|upload| upload.name.clone())
                .or_else(|| job.staged.as_ref().map(|(name, _)| name.clone())),
            size: job.upload.as_ref().map(|upload| upload.size).unwrap_or(0),
            received: job.received,
            verified: job.verified,
            prepared: job.prepared,
            to_prepare: job.to_prepare,
            error: job.error.clone(),
            wipe_data: job.wipe_data,
            repartition: job.mode == Mode::Disk && job.phase != UpdatePhase::Idle,
            last: last.map(|outcome| UpdateResult {
                applied: outcome.applied,
                message: outcome.message,
                source: outcome.source,
                wiped_data: outcome.wiped_data,
                attempts: outcome.attempts,
            }),
        })
    }

    /// Committed, and applied at the next boot.
    pub fn is_pending(&self) -> bool {
        lock(&self.job).phase == UpdatePhase::Pending
    }

    /// An upload has been checked against this disk's partition table, or
    /// is being: it would be applied to a layout that must not move first.
    pub fn is_staged(&self) -> bool {
        matches!(
            lock(&self.job).phase,
            UpdatePhase::Verifying
                | UpdatePhase::Preparing
                | UpdatePhase::Ready
                | UpdatePhase::Pending
        )
    }

    pub async fn commit(self: &Arc<Self>, caller: &str, wipe_data: bool) -> Result<Done, String> {
        self.load().await;
        let (name, mode) = {
            let job = lock(&self.job);
            match (job.phase, &job.staged) {
                (UpdatePhase::Ready | UpdatePhase::Pending, Some((name, _))) => {
                    (name.clone(), job.mode)
                }
                (UpdatePhase::Verifying, _) => {
                    return Err("the upload is still being verified".to_string())
                }
                (UpdatePhase::Preparing, _) => {
                    return Err("the update is still being prepared".to_string())
                }
                (UpdatePhase::Receiving, _) => {
                    return Err("the upload is not complete yet".to_string())
                }
                (UpdatePhase::Failed, _) => {
                    return Err(job
                        .error
                        .clone()
                        .unwrap_or_else(|| "the update failed".to_string()))
                }
                _ => return Err("no update is staged".to_string()),
            }
        };
        // A disk update writes the image's own, empty /data whatever is
        // asked; saying so keeps the marker and the status honest.
        let wipe_data = wipe_data || mode == Mode::Disk;

        let path = self.paths.update_dir().join(update::PENDING);
        blocking("marking the update", move || {
            fsutil::write_json(
                &path,
                &Pending {
                    wipe_data,
                    ..Pending::default()
                },
            )
            .map_err(|err| format!("writing the update marker: {err}"))
        })
        .await?;
        {
            let mut job = lock(&self.job);
            job.phase = UpdatePhase::Pending;
            job.wipe_data = wipe_data;
        }
        let what = match (mode, wipe_data) {
            (Mode::Disk, _) => ", rewriting the whole disk: /data is wiped",
            (Mode::Root, true) => ", and /data is wiped",
            (Mode::Root, false) => "",
        };
        self.log.info(format!(
            "update: {name} committed by {caller}; it is applied at the next boot{what}"
        ));
        Ok(Done::new(format!(
            "{name} is applied at the next boot{what}"
        )))
    }

    pub async fn cancel(self: &Arc<Self>, caller: &str) -> Result<Done, String> {
        self.load().await;
        let preparing = {
            let job = lock(&self.job);
            if working(job.phase) {
                job.cancel.store(true, Ordering::SeqCst);
                true
            } else {
                false
            }
        };
        if preparing {
            self.log
                .info(format!("update: preparation cancelled by {caller}"));
            return Ok(Done::new("cancelling the preparation"));
        }

        let dir = self.paths.update_dir();
        blocking("removing the update staging", move || {
            if dir.exists() {
                flash::clean(&dir, &mut io::sink());
            }
            Ok(())
        })
        .await?;
        *lock(&self.job) = Job::default();
        self.log.info(format!("update: cancelled by {caller}"));
        Ok(Done::new("the update is cancelled and its staging removed"))
    }

    /// Drop the staging, and remember why for `update-status`.
    async fn fail(self: &Arc<Self>, error: &str) {
        let dir = self.paths.update_dir();
        let _ = blocking("removing the update staging", move || {
            flash::clean(&dir, &mut io::sink());
            Ok(())
        })
        .await;
        *lock(&self.job) = Job {
            phase: UpdatePhase::Failed,
            error: Some(error.to_string()),
            ..Job::default()
        };
        self.log.info(format!("update: failed: {error}"));
    }

    fn probe(&self) -> layout::Probe {
        layout::Probe {
            cmdline: self.paths.cmdline.clone(),
            sys_block: self.paths.sys_block.clone(),
            by_partuuid: self.paths.by_partuuid.clone(),
            by_label: self.paths.by_label.clone(),
        }
    }

    fn spawn_prepare(self: &Arc<Self>) {
        let (upload, cancel) = {
            let mut job = lock(&self.job);
            let Some(upload) = job.upload.clone() else {
                return;
            };
            job.phase = if upload.verify {
                UpdatePhase::Verifying
            } else {
                UpdatePhase::Preparing
            };
            job.verified = 0;
            job.prepared = 0;
            job.to_prepare = 0;
            job.cancel = Arc::new(AtomicBool::new(false));
            (upload, Arc::clone(&job.cancel))
        };
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("update-prepare".to_string())
            .spawn(move || {
                lower_priority();
                let outcome = this.prepare(&upload, &cancel);
                this.finish(&upload, outcome, &cancel);
            });
        if let Err(err) = spawned {
            let mut job = lock(&self.job);
            job.phase = UpdatePhase::Failed;
            job.error = Some(format!("cannot start preparing: {err}"));
        }
    }

    /// The SHA-256 of the whole upload, reporting how far it got as it goes
    /// so `update-status` - and with it tessaro-ctl's ETA - can follow.
    fn verify(
        &self,
        path: &Path,
        upload: &Upload,
        cancel: &AtomicBool,
    ) -> Result<(u64, String), String> {
        let mut file = File::open(path).map_err(|err| format!("reading the upload: {err}"))?;
        let mut hasher = openssl::sha::Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        let (mut done, mut shown) = (0u64, 0u64);
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".to_string());
            }
            let read = file
                .read(&mut buffer)
                .map_err(|err| format!("reading the upload: {err}"))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            done += read as u64;
            lock(&self.job).verified = done;
            let percent = done * 100 / upload.size.max(1);
            if percent >= shown + 10 && done < upload.size {
                shown = percent - percent % 10;
                self.log.info(format!(
                    "update: verifying {} {shown}% ({} of {})",
                    upload.name,
                    megabytes(done),
                    megabytes(upload.size)
                ));
            }
        }
        Ok((done, update::hex(&hasher.finish())))
    }

    /// Runs on the preparation thread; blocking throughout.
    fn prepare(&self, upload: &Upload, cancel: &Arc<AtomicBool>) -> Result<Manifest, String> {
        let dir = self.paths.update_dir();
        let path = dir.join(update::UPLOAD);

        // The whole file, before a byte of it is trusted - unless the client
        // asked to skip it (`update send --no-verify`). Then the bmap's
        // per-range checksums below are the guard, and they still cover
        // every block that will be written; what goes unchecked is the rest
        // of the file, which is never used.
        if upload.verify {
            let (size, sha256) = self.verify(&path, upload, cancel)?;
            if size != upload.size || sha256 != upload.sha256 {
                return Err(format!(
                    "{} arrived damaged: its SHA-256 does not match; upload it again",
                    upload.name
                ));
            }
            lock(&self.job).phase = UpdatePhase::Preparing;
        } else {
            self.log.info(format!(
                "update: not checking {} as a whole, as asked; the bmap's checksums still apply",
                upload.name
            ));
        }
        let bmap = bmap::parse(&upload.bmap)?;
        let device = layout::probe(&self.probe())
            .map_err(|err| format!("cannot tell which disk this device booted from: {err}"))?;
        let image = Image::open(&path).map_err(|err| format!("reading the upload: {err}"))?;
        let mode = upload.mode();
        let image_size = bmap.image_size;

        let mut observer = Progress {
            job: Arc::clone(&self.job),
            cancel: Arc::clone(cancel),
            log: Arc::clone(&self.log),
            name: upload.name.clone(),
            shown: 0,
        };
        let extract = self.extract;
        let kernel = self.paths.kernel_file.clone();
        let manifest = prepare::prepare(
            image,
            &bmap,
            mode,
            Source {
                name: upload.name.clone(),
                sha256: upload.sha256.clone(),
            },
            &dir,
            |partitions| check_partitions(&device, partitions, mode, image_size),
            |boot, dest| extract(boot, dest, &kernel).map(|()| kernel.clone()),
            &mut observer,
        )?;

        // The manifest describes the upload now; it stays, to be written.
        fsutil::remove_if_exists(&dir.join(update::UPLOAD_META))
            .and_then(|()| fsutil::sync_dir(&dir))
            .map_err(|err| format!("{}: {err}", update::UPLOAD_META))?;
        Ok(manifest)
    }

    fn finish(&self, upload: &Upload, outcome: Result<Manifest, String>, cancel: &AtomicBool) {
        match outcome {
            Ok(manifest) => {
                let to_write = megabytes(manifest.target.mapped());
                self.log.info(match &manifest.kernel {
                    Some(kernel) => format!(
                        "update: {} is checked: {to_write} of the root filesystem to write, \
                         kernel {}",
                        upload.name, kernel.name
                    ),
                    None => format!(
                        "update: {} is checked: {to_write} of the whole disk to write",
                        upload.name
                    ),
                });
                let mut job = lock(&self.job);
                job.phase = UpdatePhase::Ready;
                job.mode = manifest.mode;
                job.staged = Some((upload.name.clone(), upload.sha256.clone()));
                job.upload = None;
                job.prepared = job.to_prepare;
            }
            Err(err) => {
                flash::clean(&self.paths.update_dir(), &mut io::sink());
                let cancelled = cancel.load(Ordering::SeqCst);
                let mut job = lock(&self.job);
                *job = Job::default();
                if cancelled {
                    self.log
                        .info(format!("update: preparing {} cancelled", upload.name));
                } else {
                    self.log
                        .info(format!("update: preparing {} failed: {err}", upload.name));
                    job.phase = UpdatePhase::Failed;
                    job.error = Some(err);
                }
            }
        }
    }
}

struct Progress {
    job: Arc<Mutex<Job>>,
    cancel: Arc<AtomicBool>,
    log: Arc<Log>,
    name: String,
    shown: u64,
}

impl prepare::Observer for Progress {
    fn progress(&mut self, done: u64, total: u64) {
        {
            let mut job = lock(&self.job);
            job.prepared = done;
            job.to_prepare = total;
        }
        let percent = done * 100 / total.max(1);
        if percent >= self.shown + 10 && done < total {
            self.shown = percent - percent % 10;
            self.log.info(format!(
                "update: preparing {} {}% ({} of {} checked)",
                self.name,
                self.shown,
                megabytes(done),
                megabytes(total)
            ));
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

/// For the boot oneshot: put what the initramfs did into the journal, once.
pub fn report(paths: &Paths, log: &Log) {
    let path = paths.update_dir().join(update::RESULT);
    let Ok(mut outcome) = fsutil::read_json::<Outcome>(&path) else {
        return;
    };
    if outcome.reported {
        return;
    }
    let attempts = if outcome.attempts > 1 {
        format!(" after {} attempts", outcome.attempts)
    } else {
        String::new()
    };
    let wiped = if outcome.wiped_data {
        ", /data re-created"
    } else {
        ""
    };
    if outcome.applied {
        log.info(format!("{}{attempts}{wiped}", outcome.message));
    } else {
        log.info(format!(
            "update {} was not applied{attempts}: {}",
            outcome.source, outcome.message
        ));
    }
    outcome.reported = true;
    if let Err(err) = fsutil::write_json(&path, &outcome) {
        log.info(format!("cannot mark the update result as reported: {err}"));
    }
}

/// What the staging directory says about the update under way.
fn scan(dir: &Path) -> Job {
    let mut job = Job::default();
    if let Ok(manifest) = fsutil::read_json::<Manifest>(&dir.join(update::MANIFEST)) {
        job.phase = UpdatePhase::Ready;
        job.prepared = manifest.target.mapped();
        job.to_prepare = job.prepared;
        job.mode = manifest.mode;
        job.staged = Some((manifest.source.name, manifest.source.sha256));
        if let Ok(pending) = fsutil::read_json::<Pending>(&dir.join(update::PENDING)) {
            job.phase = UpdatePhase::Pending;
            job.wipe_data = pending.wipe_data;
        }
        return job;
    }
    if let Ok(upload) = fsutil::read_json::<Upload>(&dir.join(update::UPLOAD_META)) {
        let received = fs::metadata(dir.join(update::UPLOAD))
            .map(|meta| meta.len())
            .unwrap_or(0)
            .min(upload.size);
        job.received = received;
        // Short of the limit it is simply checked again at the next chunk.
        job.head_checked = received >= HEAD_CHECK_LIMIT.min(upload.size);
        job.phase = if received == upload.size {
            UpdatePhase::Preparing
        } else {
            UpdatePhase::Receiving
        };
        job.mode = upload.mode();
        job.upload = Some(upload);
    }
    job
}

/// A root update needs this disk's own layout; a disk update brings its
/// own and only has to fit.
fn check_partitions(
    device: &layout::Layout,
    partitions: &[ptable::Partition],
    mode: Mode,
    image_size: u64,
) -> Result<(), String> {
    match mode {
        Mode::Root => layout::check(device, partitions),
        Mode::Disk => layout::check_disk(device, partitions, image_size),
    }
}

/// Whether the initramfs can hold a disk update's upload in RAM. MemTotal,
/// not MemAvailable: the initramfs runs before anything else is started.
fn fits_in_ram(meminfo: &Path, size: u64) -> Result<(), String> {
    let text =
        fs::read_to_string(meminfo).map_err(|err| format!("{}: {err}", meminfo.display()))?;
    let total = crate::hardware::meminfo_bytes(&text, "MemTotal")
        .ok_or_else(|| format!("{} has no MemTotal", meminfo.display()))?;
    let needed = size + flash::RAM_MARGIN + RAM_RESERVED;
    if total < needed {
        return Err(format!(
            "this device has {} of RAM; rewriting its disk from a {} upload needs {}, \
             since the upload is held in RAM while /data is overwritten",
            megabytes(total),
            megabytes(size),
            megabytes(needed)
        ));
    }
    Ok(())
}

/// The partition table from the start of a partial upload, against this
/// disk. An upload too short to decompress that far is not an error yet:
/// `Ok(false)`, try again with more of it.
fn check_head(upload: &Path, probe: &layout::Probe, meta: &Upload) -> Result<bool, String> {
    let mut image = Image::open(upload).map_err(|err| format!("reading the upload: {err}"))?;
    let mut head = vec![0u8; 1 << 20];
    let mut filled = 0;
    while filled < head.len() {
        match image.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            // A compressed upload cut off mid-block, or before the decoder
            // had given out anything.
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(_) if filled > 0 => break,
            Err(err) => return Err(format!("the upload is not a disk image: {err}")),
        }
    }
    if filled < 64 * 1024 {
        return Ok(false);
    }
    let partitions = ptable::parse(&head[..filled])?;
    let device = layout::probe(probe)
        .map_err(|err| format!("cannot tell which disk this device booted from: {err}"))?;
    let image_size = bmap::parse(&meta.bmap)?.image_size;
    check_partitions(&device, &partitions, meta.mode(), image_size).map(|()| true)
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name.contains('/')
        || name.chars().any(|ch| ch.is_control())
    {
        return Err(format!("{name:?} is not a file name"));
    }
    Ok(())
}

/// Loop-mount the staged boot partition read-only and copy the kernel out.
fn loop_mount_kernel(boot: &Path, dest: &Path, name: &str) -> Result<(), String> {
    let mount = boot.with_extension("mnt");
    fs::create_dir_all(&mount).map_err(|err| format!("{}: {err}", mount.display()))?;
    // Left over if the agent died with it mounted; failing is the normal case.
    let _ = Command::new("umount").arg(&mount).status();
    run(
        "mount",
        &[
            "-t".as_ref(),
            "vfat".as_ref(),
            "-o".as_ref(),
            "loop,ro,nodev,nosuid,noexec".as_ref(),
            boot.as_os_str(),
            mount.as_os_str(),
        ],
    )?;
    let copied = fs::copy(mount.join(name), dest);
    let unmounted = run("umount", &[mount.as_os_str()]);
    let _ = fs::remove_dir(&mount);
    copied.map_err(|err| format!("the image's boot partition has no {name}: {err}"))?;
    unmounted
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let output = crate::proc::run(Command::new(program).args(args), None)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed: {}", crate::proc::said(&output)))
    }
}

/// Idle CPU and I/O priority for the calling thread only: on Linux both
/// calls take a thread id, and 0 is the caller.
fn lower_priority() {
    // SAFETY: plain syscalls with integer arguments.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
        // IOPRIO_WHO_PROCESS, the caller, IOPRIO_CLASS_IDLE.
        libc::syscall(libc::SYS_ioprio_set, 1, 0, 3 << 13);
    }
}

/// The preparation thread is running: verifying the upload, then staging it.
fn working(phase: UpdatePhase) -> bool {
    matches!(phase, UpdatePhase::Verifying | UpdatePhase::Preparing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;

    use update::testing;

    struct Device {
        _dir: tempfile::TempDir,
        paths: Paths,
        image: Vec<u8>,
        bmap: String,
    }

    /// The boot staging is sized by what the bmap maps inside this device's
    /// boot partition, sda1 at 1-2 MiB in the fixture, not by the fixed 512M;
    /// that is what lets an update fit an ungrown 1 GB /data.
    #[test]
    fn boot_room_is_what_the_boot_partition_maps() {
        let dir = tempfile::tempdir().unwrap();
        let probe = testing::device(dir.path());
        let (_, text) = testing::disk();
        let parsed = bmap::parse(&text).unwrap();
        let mapped = parsed.mapped_within(1 << 20, 2 << 20);
        assert!(mapped > 0);
        assert_eq!(boot_room(&probe, &parsed), 2 * mapped + BOOT_SLACK);
        assert!(boot_room(&probe, &parsed) < BOOT_ROOM);

        // A device whose layout cannot be read keeps the fixed worst case.
        let unknown = layout::Probe {
            cmdline: dir.path().join("missing"),
            ..probe
        };
        assert_eq!(boot_room(&unknown, &parsed), BOOT_ROOM);
    }

    fn device() -> Device {
        let dir = tempfile::tempdir().unwrap();
        let probe = testing::device(dir.path());
        let at = |name: &str| dir.path().join(name).display().to_string();
        fs::write(dir.path().join("meminfo"), "MemTotal:  4000000 kB\n").unwrap();
        let env: HashMap<String, String> = [
            ("KIOSK_STATE_DIR", at("data")),
            ("KIOSK_AUTHORIZED_KEYS", at("root/.ssh/authorized_keys")),
            ("KIOSK_CMDLINE", probe.cmdline.display().to_string()),
            ("KIOSK_SYS_BLOCK", probe.sys_block.display().to_string()),
            ("KIOSK_BY_PARTUUID", probe.by_partuuid.display().to_string()),
            ("KIOSK_BY_LABEL", probe.by_label.display().to_string()),
            ("KIOSK_MEMINFO", at("meminfo")),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        let (image, bmap) = testing::disk();
        Device {
            _dir: dir,
            paths: Paths::load(&env),
            image,
            bmap,
        }
    }

    fn fake_extract(_boot: &Path, dest: &Path, _name: &str) -> Result<(), String> {
        fs::write(dest, b"a kernel").map_err(|err| err.to_string())
    }

    fn updates(device: &Device) -> Arc<Updates> {
        Updates::with_extract(
            Arc::new(Log::buffered(true)),
            device.paths.clone(),
            fake_extract,
        )
    }

    fn upload(device: &Device, sha256: String, verify: bool) -> ImageUpload {
        ImageUpload {
            name: "tessaro.wic".to_string(),
            size: device.image.len() as u64,
            sha256,
            bmap: device.bmap.clone(),
            verify,
            repartition: false,
        }
    }

    /// The same disk, booted from a root with another PARTUUID: as far as
    /// the layout check can tell, a disk laid out for another build.
    fn relayout(device: &Device) {
        let other = "09b3d676-0000-0000-0000-000000000000";
        let by = &device.paths.by_partuuid;
        fs::rename(by.join(testing::ROOT_PARTUUID), by.join(other)).unwrap();
        fs::write(&device.paths.cmdline, format!("root=PARTUUID={other}\n")).unwrap();
    }

    async fn begin(updates: &Arc<Updates>, device: &Device) -> Result<UpdateBegun, String> {
        let upload = upload(device, update::sha256(&device.image), true);
        updates.begin("a test", upload).await
    }

    async fn send(updates: &Arc<Updates>, device: &Device, from: u64) -> Result<(), String> {
        let mut offset = from as usize;
        while offset < device.image.len() {
            let end = (offset + protocol::UPDATE_CHUNK).min(device.image.len());
            let data = openssl::base64::encode_block(&device.image[offset..end]);
            updates.chunk(offset as u64, data).await?;
            offset = end;
        }
        Ok(())
    }

    async fn settle(updates: &Arc<Updates>) -> UpdateStatus {
        for _ in 0..500 {
            let status = updates.status().await.unwrap();
            if !working(status.phase) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("preparing never finished");
    }

    #[tokio::test]
    async fn an_upload_is_staged_and_committed() {
        let device = device();
        let updates = updates(&device);

        assert_eq!(begin(&updates, &device).await.unwrap().offset, 0);
        send(&updates, &device, 0).await.unwrap();
        let status = settle(&updates).await;
        assert_eq!(status.phase, UpdatePhase::Ready, "{status:?}");
        assert_eq!(status.prepared, status.to_prepare);

        let dir = device.paths.update_dir();
        let manifest: Manifest = fsutil::read_json(&dir.join(update::MANIFEST)).unwrap();
        assert_eq!(manifest.mode, Mode::Root);
        assert_eq!(manifest.upload.sha256, update::sha256(&device.image));
        // The upload is what the initramfs writes from; nothing else is kept.
        assert!(dir.join(update::UPLOAD).exists());
        assert!(dir.join(update::KERNEL).exists());
        assert!(!dir.join(update::BOOT_IMAGE).exists());
        assert!(!dir.join(update::UPLOAD_META).exists());

        updates.commit("a test", true).await.unwrap();
        let pending: Pending = fsutil::read_json(&dir.join(update::PENDING)).unwrap();
        assert!(pending.wipe_data);
        let status = updates.status().await.unwrap();
        assert_eq!(status.phase, UpdatePhase::Pending);
        assert_eq!(status.name.as_deref(), Some("tessaro.wic"));

        // A second begin of anything is refused until it is cancelled.
        assert!(begin(&updates, &device)
            .await
            .unwrap_err()
            .contains("committed"));
        updates.cancel("a test").await.unwrap();
        assert!(!dir.join(update::PENDING).exists());
        assert_eq!(updates.status().await.unwrap().phase, UpdatePhase::Idle);
    }

    #[tokio::test]
    async fn an_interrupted_upload_resumes_after_a_restart() {
        let device = device();
        let first = updates(&device);
        begin(&first, &device).await.unwrap();
        let chunk = openssl::base64::encode_block(&device.image[..protocol::UPDATE_CHUNK]);
        first.chunk(0, chunk).await.unwrap();
        drop(first);

        let second = updates(&device);
        let begun = begin(&second, &device).await.unwrap();
        assert_eq!(begun.offset, protocol::UPDATE_CHUNK as u64);
        assert_eq!(begun.phase, UpdatePhase::Receiving);
        send(&second, &device, begun.offset).await.unwrap();
        assert_eq!(settle(&second).await.phase, UpdatePhase::Ready);
    }

    #[tokio::test]
    async fn a_chunk_cut_off_mid_write_is_counted_on_resume() {
        let device = device();
        let updates = updates(&device);
        begin(&updates, &device).await.unwrap();
        let chunk = openssl::base64::encode_block(&device.image[..protocol::UPDATE_CHUNK]);
        // The client goes away while the chunk is being written: the request
        // is dropped after its first poll, the write carries on regardless.
        let cut = crate::deadline::within("a chunk", Duration::ZERO, updates.chunk(0, chunk)).await;
        assert!(
            cut.is_err(),
            "the chunk finished before it could be cut off"
        );

        let mut begun = begin(&updates, &device).await.unwrap();
        for _ in 0..500 {
            if begun.offset != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            begun = begin(&updates, &device).await.unwrap();
        }
        assert_eq!(begun.offset, protocol::UPDATE_CHUNK as u64);
        send(&updates, &device, begun.offset).await.unwrap();
        assert_eq!(settle(&updates).await.phase, UpdatePhase::Ready);
    }

    #[tokio::test]
    async fn a_chunk_out_of_place_is_refused() {
        let device = device();
        let updates = updates(&device);
        begin(&updates, &device).await.unwrap();
        let err = updates
            .chunk(10, openssl::base64::encode_block(b"xx"))
            .await
            .unwrap_err();
        assert!(err.contains("send from there"), "{err}");
    }

    #[tokio::test]
    async fn an_image_for_another_disk_fails_on_the_first_chunk() {
        let device = device();
        relayout(&device);

        let updates = updates(&device);
        begin(&updates, &device).await.unwrap();
        let chunk = openssl::base64::encode_block(&device.image[..protocol::UPDATE_CHUNK]);
        let err = updates.chunk(0, chunk).await.unwrap_err();
        assert!(err.contains("--repartition"), "{err}");
        let status = updates.status().await.unwrap();
        assert_eq!(status.phase, UpdatePhase::Failed);
        assert!(!device.paths.update_dir().join(update::UPLOAD).exists());
    }

    #[tokio::test]
    async fn a_repartition_takes_another_layout_and_always_wipes() {
        let device = device();
        relayout(&device);
        let first = updates(&device);
        let upload = ImageUpload {
            repartition: true,
            ..upload(&device, update::sha256(&device.image), true)
        };
        first.begin("a test", upload).await.unwrap();
        send(&first, &device, 0).await.unwrap();
        let status = settle(&first).await;
        assert_eq!(status.phase, UpdatePhase::Ready, "{status:?}");
        assert!(status.repartition);

        let dir = device.paths.update_dir();
        let manifest: Manifest = fsutil::read_json(&dir.join(update::MANIFEST)).unwrap();
        assert_eq!(manifest.mode, Mode::Disk);
        assert!(manifest.kernel.is_none());
        assert!(!dir.join(update::KERNEL).exists());

        // Not asked to wipe, and wiped anyway: the image's /data is empty.
        let done = first.commit("a test", false).await.unwrap();
        assert!(done.message.contains("whole disk"), "{}", done.message);
        let pending: Pending = fsutil::read_json(&dir.join(update::PENDING)).unwrap();
        assert!(pending.wipe_data);

        // A restart finds it as it was.
        let restarted = updates(&device);
        let status = restarted.status().await.unwrap();
        assert_eq!(status.phase, UpdatePhase::Pending);
        assert!(status.repartition && status.wipe_data);
    }

    #[tokio::test]
    async fn a_repartition_needs_the_ram_to_hold_the_upload() {
        let device = device();
        fs::write(&device.paths.meminfo, "MemTotal:  100000 kB\n").unwrap();
        let updates = updates(&device);
        let upload = ImageUpload {
            repartition: true,
            ..upload(&device, update::sha256(&device.image), true)
        };
        let err = updates.begin("a test", upload).await.unwrap_err();
        assert!(err.contains("RAM"), "{err}");
    }

    #[tokio::test]
    async fn a_repartition_needs_a_disk_the_image_fits_on() {
        let device = device();
        // The disk shrinks to 8 MiB; the image is 12.
        let disk = device.paths.sys_block.join("sda/size");
        fs::write(disk, "16384\n").unwrap();
        let updates = updates(&device);
        let upload = ImageUpload {
            repartition: true,
            ..upload(&device, update::sha256(&device.image), true)
        };
        updates.begin("a test", upload).await.unwrap();
        let chunk = openssl::base64::encode_block(&device.image[..protocol::UPDATE_CHUNK]);
        let err = updates.chunk(0, chunk).await.unwrap_err();
        assert!(err.contains("holds"), "{err}");
    }

    #[tokio::test]
    async fn a_damaged_upload_is_caught_by_its_checksum() {
        let device = device();
        let updates = updates(&device);
        let upload = upload(&device, "0".repeat(64), true);
        updates.begin("a test", upload).await.unwrap();
        send(&updates, &device, 0).await.unwrap();
        let status = settle(&updates).await;
        assert_eq!(status.phase, UpdatePhase::Failed);
        assert!(status.error.unwrap().contains("damaged"));
    }

    #[tokio::test]
    async fn no_verify_skips_the_whole_file_check_but_not_the_bmap() {
        let device = device();
        let updates = updates(&device);
        // A wrong file checksum is not looked at...
        let unchecked = upload(&device, "0".repeat(64), false);
        updates.begin("a test", unchecked).await.unwrap();
        send(&updates, &device, 0).await.unwrap();
        assert_eq!(settle(&updates).await.phase, UpdatePhase::Ready);

        // ...but a damaged block still fails against the bmap.
        updates.cancel("a test").await.unwrap();
        let mut damaged = device.image.clone();
        damaged[(3 * testing::MIB) as usize] ^= 0xff;
        let damaged_device = Device {
            image: damaged,
            ..device
        };
        let damaged_upload = upload(&damaged_device, "0".repeat(64), false);
        updates.begin("a test", damaged_upload).await.unwrap();
        send(&updates, &damaged_device, 0).await.unwrap();
        let status = settle(&updates).await;
        assert_eq!(status.phase, UpdatePhase::Failed);
        assert!(status.error.unwrap().contains("bmap"), "bmap check");
    }

    #[tokio::test]
    async fn commit_needs_a_staged_update() {
        let device = device();
        let updates = updates(&device);
        assert!(updates.commit("a test", false).await.is_err());
        begin(&updates, &device).await.unwrap();
        assert!(updates
            .commit("a test", false)
            .await
            .unwrap_err()
            .contains("not complete"));
    }

    #[tokio::test]
    async fn the_last_result_is_reported() {
        let device = device();
        let dir = device.paths.update_dir();
        fs::create_dir_all(&dir).unwrap();
        fsutil::write_json(
            &dir.join(update::RESULT),
            &Outcome {
                applied: true,
                message: "update applied: tessaro.wic".to_string(),
                source: "tessaro.wic".to_string(),
                wiped_data: false,
                attempts: 1,
                reported: false,
            },
        )
        .unwrap();
        let status = updates(&device).status().await.unwrap();
        assert!(status.last.unwrap().applied);
    }

    #[test]
    fn the_boot_after_an_update_reports_it_once() {
        let device = device();
        let dir = device.paths.update_dir();
        fs::create_dir_all(&dir).unwrap();
        let outcome = Outcome {
            applied: false,
            message: "the staged kernel is damaged; nothing was written".to_string(),
            source: "tessaro.wic".to_string(),
            wiped_data: false,
            attempts: 1,
            reported: false,
        };
        fsutil::write_json(&dir.join(update::RESULT), &outcome).unwrap();

        let log = Log::buffered(true);
        report(&device.paths, &log);
        report(&device.paths, &log);
        let lines = log.lines();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("not applied"), "{}", lines[0]);
    }
}
