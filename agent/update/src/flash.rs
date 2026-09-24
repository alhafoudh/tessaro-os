//! What tessaro-flash does with a pending update: the marker, the attempt
//! count, the result, the cleanup, and which of those the initramfs hook
//! should act on. `apply` does the writing; this decides what it means.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::apply::{self, Failure};
use crate::manifest::{Manifest, Mode, Outcome, Pending};
use crate::wipe::{self, Run};
use crate::{fsutil, megabytes, PENDING, RESULT, STAGING, UPLOAD};

/// Boots that may try before the updater stops rebooting in a loop.
pub const MAX_ATTEMPTS: u32 = 5;

/// RAM a disk update leaves free beyond its copy of the upload: the
/// decompressor, the write buffers, the rest of the initramfs.
pub const RAM_MARGIN: u64 = 64 << 20;

/// The process exit code, which is what the initramfs hook branches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Written. Reboot into the new kernel.
    Applied = 0,
    /// Failed after writing began. Reboot and try again.
    Retry = 1,
    /// Refused before writing anything. Carry on booting the old system.
    Refused = 2,
    /// No update pending. Carry on booting.
    Nothing = 3,
    /// Out of attempts with the root partly written, or a disk update that
    /// failed half way. Stop and say so.
    GaveUp = 4,
}

pub struct Targets {
    pub dir: PathBuf,
    pub root: PathBuf,
    pub esp: PathBuf,
    /// The `/data` device and where it is mounted. Needed when the update
    /// asks for `/data` to be wiped, and by every disk update.
    pub data: Option<(PathBuf, PathBuf)>,
    /// Only needed by a disk update.
    pub disk: Option<Disk>,
}

pub struct Disk {
    /// The whole disk, `/dev/sda`.
    pub device: PathBuf,
    /// Where to mount the tmpfs the upload is copied into.
    pub ram: PathBuf,
    /// `/proc/meminfo`, for how much RAM there is to copy it into.
    pub meminfo: PathBuf,
}

pub fn apply_pending(targets: &Targets, console: &mut dyn Write, run: &mut dyn Run) -> Exit {
    let pending_path = targets.dir.join(PENDING);
    if !pending_path.exists() {
        return Exit::Nothing;
    }
    let mut pending: Pending = match fsutil::read_json(&pending_path) {
        Ok(pending) => pending,
        Err(err) => {
            say(
                console,
                &format!("the update marker is unreadable ({err}); ignoring it"),
            );
            Pending::default()
        }
    };
    let manifest = apply::read_manifest(&targets.dir);
    let source = manifest
        .as_ref()
        .map(|manifest| manifest.source.name.clone())
        .unwrap_or_else(|_| "an unknown image".to_string());

    pending.attempts += 1;
    if pending.attempts > MAX_ATTEMPTS {
        say(
            console,
            &format!(
                "{} attempts to write {source} failed and the root filesystem is incomplete; \
                 this device needs a full reflash (mise run image:flash)",
                MAX_ATTEMPTS
            ),
        );
        return Exit::GaveUp;
    }
    if let Err(err) = fsutil::write_json(&pending_path, &pending) {
        say(console, &format!("cannot update the marker: {err}"));
        // Without a durable count this could loop forever; do not start.
        return finish_refused(targets, console, &pending, &source, err.to_string());
    }
    say(
        console,
        &format!("applying {source}, attempt {}", pending.attempts),
    );

    if let Ok(manifest) = &manifest {
        if manifest.mode == Mode::Disk {
            return apply_disk(targets, console, run, &pending, manifest);
        }
    }

    let mut marked = pending.clone();
    let result = apply::apply(
        &targets.dir,
        &targets.root,
        &targets.esp,
        console,
        &mut || {
            marked.started = true;
            fsutil::write_json(&pending_path, &marked).map_err(|err| format!("the marker: {err}"))
        },
    );

    match result {
        Ok(_) => {
            let outcome = Outcome {
                applied: true,
                message: format!("update applied: {source}"),
                source,
                wiped_data: false,
                attempts: pending.attempts,
                reported: false,
            };
            if pending.wipe_data {
                let Some((device, mount)) = &targets.data else {
                    say(
                        console,
                        "the update asks to wipe /data, but no data device was given",
                    );
                    return Exit::Retry;
                };
                say(console, "re-creating /data");
                if let Err(err) = wipe::begin(&targets.esp, device, mount, &outcome, run) {
                    say(console, &format!("re-creating /data failed: {err}"));
                    return Exit::Retry;
                }
            } else {
                if let Err(err) = fsutil::write_json(&targets.dir.join(RESULT), &outcome) {
                    say(console, &format!("cannot record the result: {err}"));
                }
                clean(&targets.dir, console);
            }
            say(console, &outcome.message);
            Exit::Applied
        }
        Err(Failure::Untouched(message)) if !pending.started => {
            finish_refused(targets, console, &pending, &source, message)
        }
        // Refused this time, but an earlier attempt already began writing:
        // the old root is not there to fall back to.
        Err(Failure::Untouched(message)) | Err(Failure::Partial(message)) => {
            say(console, &format!("the update failed: {message}"));
            if pending.attempts >= MAX_ATTEMPTS {
                say(
                    console,
                    "out of attempts; this device needs a full reflash (mise run image:flash)",
                );
                Exit::GaveUp
            } else {
                Exit::Retry
            }
        }
    }
}

/// The whole disk, `/data` and the staging on it included. The upload goes
/// into RAM first and is checked there; until the first write, a refusal
/// still boots the old system. From the first write on there is nothing to
/// fall back to or to write again from.
fn apply_disk(
    targets: &Targets,
    console: &mut dyn Write,
    run: &mut dyn Run,
    pending: &Pending,
    manifest: &Manifest,
) -> Exit {
    let source = manifest.source.name.clone();
    let (Some(disk), Some((data_device, data_mount))) = (&targets.disk, &targets.data) else {
        let why = "the update rewrites the whole disk, but no disk and data device were given";
        return finish_refused(targets, console, pending, &source, why.to_string());
    };

    let upload = match into_ram(&targets.dir.join(UPLOAD), manifest, disk, console, run) {
        Ok(upload) => upload,
        Err(message) => return finish_refused(targets, console, pending, &source, message),
    };
    if let Err(message) = apply::check(manifest, &upload, &targets.dir, &disk.device, None, console)
    {
        let _ = run.run("umount", &[&disk.ram.to_string_lossy()]);
        return finish_refused(targets, console, pending, &source, message);
    }

    // Nothing may have the disk's filesystems mounted while it is rewritten.
    // The ESP first: while /data is still mounted, a refusal can be recorded.
    for mount in [&targets.esp, data_mount] {
        if let Err(err) = run.run("umount", &[&mount.to_string_lossy()]) {
            let _ = run.run("umount", &[&disk.ram.to_string_lossy()]);
            let message = format!("cannot let go of the disk: {err}");
            return finish_refused(targets, console, pending, &source, message);
        }
    }

    match apply::write(manifest, &upload, &disk.device, console, &mut || Ok(())) {
        Ok(()) => {}
        Err(Failure::Untouched(message)) => {
            // Nothing written: /data is the old one, and takes the refusal.
            let remounted = run.run(
                "mount",
                &[
                    "-t",
                    "ext4",
                    &data_device.to_string_lossy(),
                    &data_mount.to_string_lossy(),
                ],
            );
            if let Err(err) = remounted {
                say(console, &format!("cannot mount /data again: {err}"));
            }
            return finish_refused(targets, console, pending, &source, message);
        }
        Err(Failure::Partial(message)) => {
            say(console, &format!("the update failed: {message}"));
            say(
                console,
                "the disk is partly written and nothing is left to write it again from; \
                 this device needs a full reflash (mise run image:flash)",
            );
            return Exit::GaveUp;
        }
    }

    let outcome = Outcome {
        applied: true,
        message: format!("disk rewritten: {source}"),
        source,
        wiped_data: true,
        attempts: pending.attempts,
        reported: false,
    };
    // The image's /data is a new, empty filesystem. The result goes into it
    // if it can; the device boots the new system either way.
    if let Err(err) = record_on_new_disk(&disk.device, data_device, data_mount, &outcome, run) {
        say(
            console,
            &format!("the disk is written, but the result could not be recorded: {err}"),
        );
    }
    say(console, &outcome.message);
    Exit::Applied
}

/// Copy the upload into a tmpfs of its own size, if the RAM is there.
fn into_ram(
    upload: &Path,
    manifest: &Manifest,
    disk: &Disk,
    console: &mut dyn Write,
    run: &mut dyn Run,
) -> Result<PathBuf, String> {
    let size = manifest.upload.size;
    let available = mem_available(&disk.meminfo)?;
    if available < size + RAM_MARGIN {
        return Err(format!(
            "{} of RAM is free; rewriting the disk needs {} to hold the upload",
            megabytes(available),
            megabytes(size + RAM_MARGIN)
        ));
    }
    fs::create_dir_all(&disk.ram).map_err(|err| format!("{}: {err}", disk.ram.display()))?;
    let options = format!("size={},mode=0700", size + (16 << 20));
    run.run(
        "mount",
        &[
            "-t",
            "tmpfs",
            "-o",
            &options,
            "tessaro-update",
            &disk.ram.to_string_lossy(),
        ],
    )?;
    say(
        console,
        &format!("copying the upload into RAM, {}", megabytes(size)),
    );
    let copy = disk.ram.join(UPLOAD);
    if let Err(err) = fs::copy(upload, &copy) {
        let _ = run.run("umount", &[&disk.ram.to_string_lossy()]);
        return Err(format!("copying the upload into RAM: {err}"));
    }
    Ok(copy)
}

/// `MemAvailable` from `/proc/meminfo`, in bytes.
fn mem_available(meminfo: &Path) -> Result<u64, String> {
    let text =
        fs::read_to_string(meminfo).map_err(|err| format!("{}: {err}", meminfo.display()))?;
    text.lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))
        .and_then(|rest| {
            rest.trim()
                .trim_end_matches("kB")
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|kib| kib * 1024)
        .ok_or_else(|| format!("{} has no MemAvailable", meminfo.display()))
}

/// Make the kernel read the new partition table, then mount the new `/data`
/// and put the result in it.
fn record_on_new_disk(
    disk: &Path,
    data_device: &Path,
    data_mount: &Path,
    outcome: &Outcome,
    run: &mut dyn Run,
) -> Result<(), String> {
    // EBUSY while anything still has a partition open - udev probing the
    // old ones, most likely - so a few tries before giving up on it.
    let mut tries = 0;
    while let Err(err) = run.reread_partitions(disk) {
        tries += 1;
        if tries == 10 {
            return Err(err);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // The kernel removes and re-adds the partitions, and devtmpfs follows.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !data_device.exists() {
        if Instant::now() > deadline {
            return Err(format!("{} did not come back", data_device.display()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    run.run(
        "mount",
        &[
            "-t",
            "ext4",
            &data_device.to_string_lossy(),
            &data_mount.to_string_lossy(),
        ],
    )?;
    let dir = data_mount.join("tessaro/update");
    fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    fsutil::write_json(&dir.join(RESULT), outcome).map_err(|err| format!("the result: {err}"))
}

fn finish_refused(
    targets: &Targets,
    console: &mut dyn Write,
    pending: &Pending,
    source: &str,
    message: String,
) -> Exit {
    say(console, &format!("the update was refused: {message}"));
    say(console, "nothing was written; booting the current system");
    let outcome = Outcome {
        applied: false,
        message,
        source: source.to_string(),
        wiped_data: false,
        attempts: pending.attempts,
        reported: false,
    };
    if let Err(err) = fsutil::write_json(&targets.dir.join(RESULT), &outcome) {
        say(console, &format!("cannot record the result: {err}"));
    }
    clean(&targets.dir, console);
    Exit::Refused
}

/// Remove everything staged, marker included. The result stays.
pub fn clean(dir: &Path, console: &mut dyn Write) {
    for name in STAGING {
        if let Err(err) = fsutil::remove_if_exists(&dir.join(name)) {
            say(console, &format!("cannot remove {name}: {err}"));
        }
    }
    let _ = fsutil::sync_dir(dir);
}

pub fn say(console: &mut dyn Write, line: &str) {
    let _ = writeln!(console, "tessaro-update: {line}");
    let _ = console.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{staged, staged_as, DISK_SECTORS, MIB};
    use crate::KERNEL;

    struct NoCommands;
    impl Run for NoCommands {
        fn run(&mut self, program: &str, _args: &[&str]) -> Result<(), String> {
            panic!("{program} should not run");
        }
    }

    /// Records the commands and runs none of them.
    #[derive(Default)]
    struct Recorder {
        calls: Vec<String>,
    }
    impl Run for Recorder {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String> {
            self.calls.push(format!("{program} {}", args.join(" ")));
            Ok(())
        }
        fn reread_partitions(&mut self, disk: &Path) -> Result<(), String> {
            self.calls.push(format!("reread {}", disk.display()));
            Ok(())
        }
    }

    fn device(wipe_data: bool) -> (tempfile::TempDir, Targets) {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("update");
        let esp = dir.path().join("esp");
        fs::create_dir(&staging).unwrap();
        fs::create_dir(&esp).unwrap();
        staged(&staging);
        fsutil::write_json(
            &staging.join(PENDING),
            &Pending {
                wipe_data,
                ..Pending::default()
            },
        )
        .unwrap();
        let root = dir.path().join("sda2");
        fs::write(&root, vec![0u8; (6 * MIB) as usize]).unwrap();
        let targets = Targets {
            dir: staging,
            root,
            esp,
            data: None,
            disk: None,
        };
        (dir, targets)
    }

    /// A device with a disk update staged: its /data is `data/`, holding
    /// the staging, and it has `free` bytes of RAM.
    fn disk_device(free: u64) -> (tempfile::TempDir, Targets, Manifest) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let staging = data.join("tessaro/update");
        let esp = dir.path().join("esp");
        fs::create_dir_all(&staging).unwrap();
        fs::create_dir(&esp).unwrap();
        let (_, manifest) = staged_as(&staging, Mode::Disk);
        fsutil::write_json(&staging.join(PENDING), &Pending::default()).unwrap();
        let sda = dir.path().join("sda");
        fs::write(&sda, vec![0x5au8; (DISK_SECTORS * 512) as usize]).unwrap();
        let sda3 = dir.path().join("sda3");
        fs::write(&sda3, b"").unwrap();
        let meminfo = dir.path().join("meminfo");
        fs::write(
            &meminfo,
            format!("MemTotal: 4000000 kB\nMemAvailable:   {} kB\n", free / 1024),
        )
        .unwrap();
        let targets = Targets {
            dir: staging,
            root: dir.path().join("sda2"),
            esp,
            data: Some((sda3, data)),
            disk: Some(Disk {
                device: sda,
                ram: dir.path().join("ram"),
                meminfo,
            }),
        };
        (dir, targets, manifest)
    }

    fn result(targets: &Targets) -> Outcome {
        fsutil::read_json(&targets.dir.join(RESULT)).unwrap()
    }

    #[test]
    fn an_applied_update_leaves_only_its_result() {
        let (_dir, targets) = device(false);
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut NoCommands);
        assert_eq!(exit, Exit::Applied);
        let outcome = result(&targets);
        assert!(outcome.applied);
        assert_eq!(outcome.attempts, 1);
        for name in STAGING {
            assert!(!targets.dir.join(name).exists(), "{name} is still there");
        }
    }

    #[test]
    fn nothing_pending_does_nothing() {
        let (_dir, targets) = device(false);
        fs::remove_file(targets.dir.join(PENDING)).unwrap();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut NoCommands);
        assert_eq!(exit, Exit::Nothing);
        assert!(targets.dir.join(UPLOAD).exists());
    }

    #[test]
    fn damaged_staging_boots_the_old_system_and_says_why() {
        let (_dir, targets) = device(false);
        fs::write(targets.dir.join(KERNEL), b"damaged").unwrap();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut NoCommands);
        assert_eq!(exit, Exit::Refused);
        let outcome = result(&targets);
        assert!(!outcome.applied);
        assert!(outcome.message.contains("damaged"), "{}", outcome.message);
        assert!(!targets.dir.join(PENDING).exists());
    }

    #[test]
    fn a_refusal_after_writing_began_retries_instead_of_booting_the_old_root() {
        let (_dir, targets) = device(false);
        fsutil::write_json(
            &targets.dir.join(PENDING),
            &Pending {
                attempts: 1,
                started: true,
                ..Pending::default()
            },
        )
        .unwrap();
        fs::write(targets.dir.join(KERNEL), b"damaged").unwrap();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut NoCommands);
        assert_eq!(exit, Exit::Retry);
        assert!(targets.dir.join(PENDING).exists());
    }

    #[test]
    fn attempts_run_out() {
        let (_dir, targets) = device(false);
        fsutil::write_json(
            &targets.dir.join(PENDING),
            &Pending {
                attempts: MAX_ATTEMPTS,
                started: true,
                ..Pending::default()
            },
        )
        .unwrap();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut NoCommands);
        assert_eq!(exit, Exit::GaveUp);
    }

    #[test]
    fn a_root_that_cannot_be_opened_is_refused() {
        let (_dir, targets) = device(false);
        // Opening a directory for writing fails before anything is written.
        fs::remove_file(&targets.root).unwrap();
        fs::create_dir(&targets.root).unwrap();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut NoCommands);
        assert_eq!(exit, Exit::Refused);
    }

    #[test]
    fn a_wipe_without_a_data_device_retries() {
        let (_dir, targets) = device(true);
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut NoCommands);
        assert_eq!(exit, Exit::Retry);
    }

    #[test]
    fn a_disk_update_goes_through_ram_and_leaves_its_result_on_the_new_data() {
        let (_dir, targets, manifest) = disk_device(1 << 30);
        let mut recorder = Recorder::default();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut recorder);
        assert_eq!(exit, Exit::Applied);

        let disk = targets.disk.as_ref().unwrap();
        let (sda3, data) = targets.data.as_ref().unwrap();
        let written = fs::read(&disk.device).unwrap();
        let (image, _) = crate::testing::disk();
        for chunk in &manifest.target.chunks {
            let range = chunk.offset as usize..(chunk.offset + chunk.len) as usize;
            assert_eq!(written[range.clone()], image[range]);
        }
        assert!(disk.ram.join(UPLOAD).exists(), "the copy in RAM was used");

        let calls = recorder.calls;
        let esp = targets.esp.display().to_string();
        let expected = [
            format!("umount {esp}"),
            format!("umount {}", data.display()),
            format!("reread {}", disk.device.display()),
            format!("mount -t ext4 {} {}", sda3.display(), data.display()),
        ];
        assert!(calls[0].starts_with("mount -t tmpfs"), "{calls:?}");
        assert_eq!(calls[1..], expected, "{calls:?}");

        // Where the new /data is mounted - here the same directory.
        let outcome = result(&targets);
        assert!(outcome.applied && outcome.wiped_data);
        assert!(
            outcome.message.contains("disk rewritten"),
            "{}",
            outcome.message
        );
    }

    #[test]
    fn a_disk_update_without_the_ram_for_it_is_refused() {
        // Less than the margin alone.
        let (_dir, targets, _) = disk_device(10 << 20);
        let before = fs::read(&targets.disk.as_ref().unwrap().device).unwrap();
        let mut recorder = Recorder::default();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut recorder);
        assert_eq!(exit, Exit::Refused);
        assert!(recorder.calls.is_empty(), "{:?}", recorder.calls);
        assert!(result(&targets).message.contains("RAM"));
        assert_eq!(
            fs::read(&targets.disk.as_ref().unwrap().device).unwrap(),
            before
        );
        assert!(!targets.dir.join(UPLOAD).exists());
    }

    #[test]
    fn a_damaged_disk_update_is_refused_before_the_disk_is_let_go() {
        let (_dir, targets, _) = disk_device(1 << 30);
        let path = targets.dir.join(UPLOAD);
        let mut upload = fs::read(&path).unwrap();
        let middle = upload.len() / 2;
        upload[middle] ^= 0xff;
        fs::write(&path, upload).unwrap();

        let mut recorder = Recorder::default();
        let exit = apply_pending(&targets, &mut std::io::sink(), &mut recorder);
        assert_eq!(exit, Exit::Refused);
        let esp = format!("umount {}", targets.esp.display());
        assert!(!recorder.calls.contains(&esp), "{:?}", recorder.calls);
        assert!(result(&targets).message.contains("damaged"));
    }

    #[test]
    fn mem_available_is_read_in_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("meminfo");
        fs::write(
            &path,
            "MemTotal:  8 kB\nMemFree: 1 kB\nMemAvailable:    2048 kB\n",
        )
        .unwrap();
        assert_eq!(mem_available(&path).unwrap(), 2048 * 1024);
        fs::write(&path, "MemTotal: 8 kB\n").unwrap();
        assert!(mem_available(&path).is_err());
    }
}
