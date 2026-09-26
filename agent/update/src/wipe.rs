//! `--wipe-data`: re-create `/data` once the new image is on disk.
//!
//! `/data` is where the marker lives, so while it is being re-created the
//! only thing that persists is the ESP. A marker there, holding the result
//! to write into the new filesystem, goes down first and comes off last: a
//! power cut in between leaves it, and the next boot runs this again instead
//! of booting onto a half-made `/data`.
//!
//! The retries are counted in the marker. When they run out the marker comes
//! off and the device boots on: the new root is already written, and a
//! `/data` that `mke2fs` could not re-create is most likely the old one,
//! untouched. A kiosk that boots with its old settings beats one that
//! reboots in a loop.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::manifest::Outcome;
use crate::{fsutil, RESULT, WIPE_MARKER};

/// Boots that may try to re-create `/data` before the device boots on.
pub const MAX_ATTEMPTS: u32 = 3;

/// The ESP marker: the result to write into the new `/data`, and how many
/// boots have tried.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Marker {
    outcome: Outcome,
    #[serde(default)]
    attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wiped {
    Done,
    /// Out of attempts; the marker is gone and the device should boot on.
    GaveUp(String),
}

/// Runs a program to completion. Real commands on a device, a recording in
/// the tests.
pub trait Run {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String>;

    /// Make the kernel read `disk`'s partition table again, after a disk
    /// update rewrote it. An ioctl rather than a program: the initramfs's
    /// BusyBox may not have `blockdev`.
    fn reread_partitions(&mut self, disk: &Path) -> Result<(), String> {
        crate::fsutil::reread_partitions(disk).map_err(|err| {
            format!(
                "re-reading the partition table of {}: {err}",
                disk.display()
            )
        })
    }
}

pub struct System;

impl Run for System {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String> {
        let status = Command::new(program)
            .args(args)
            .status()
            .map_err(|err| format!("{program}: {err}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{program} {} failed: {status}", args.join(" ")))
        }
    }
}

/// Put the marker on the ESP, then re-create `/data`.
pub fn begin(
    esp: &Path,
    device: &Path,
    mount: &Path,
    outcome: &Outcome,
    run: &mut dyn Run,
) -> Result<Wiped, String> {
    let marker = esp.join(WIPE_MARKER);
    let body = Marker {
        outcome: outcome.clone(),
        attempts: 0,
    };
    fsutil::write_json(&marker, &body).map_err(|err| format!("{}: {err}", marker.display()))?;
    finish(esp, device, mount, run)
}

/// Re-create `/data` from the ESP marker: the second half of `begin`, and
/// what a boot that finds the marker still there runs. An error means try
/// again at the next boot.
pub fn finish(esp: &Path, device: &Path, mount: &Path, run: &mut dyn Run) -> Result<Wiped, String> {
    let marker_path = esp.join(WIPE_MARKER);
    let mut marker: Marker = fsutil::read_json(&marker_path)?;
    let device = device.to_string_lossy();
    let mount_point = mount.to_string_lossy();

    // Mounted unless a previous attempt got past this. `umount` of a
    // directory that is not a mount point fails, and that is fine.
    let _ = run.run("umount", &[&mount_point]);

    marker.attempts += 1;
    if marker.attempts > MAX_ATTEMPTS {
        fs::remove_file(&marker_path)
            .and_then(|()| fsutil::sync_dir(esp))
            .map_err(|err| format!("{}: {err}", marker_path.display()))?;
        return Ok(Wiped::GaveUp(format!(
            "re-creating /data failed {MAX_ATTEMPTS} times; booting with /data as it is"
        )));
    }
    fsutil::write_json(&marker_path, &marker)
        .map_err(|err| format!("{}: {err}", marker_path.display()))?;

    // The label is how the overlayfs-etc preinit and fstab find /data.
    run.run("mke2fs", &["-F", "-q", "-t", "ext4", "-L", "data", &device])?;
    fs::create_dir_all(mount).map_err(|err| format!("{}: {err}", mount.display()))?;
    run.run("mount", &["-t", "ext4", &device, &mount_point])?;

    let dir = mount.join("tessaro/update");
    fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    fsutil::write_json(
        &dir.join(RESULT),
        &Outcome {
            wiped_data: true,
            ..marker.outcome
        },
    )
    .map_err(|err| format!("writing the result: {err}"))?;

    fs::remove_file(&marker_path).map_err(|err| format!("{}: {err}", marker_path.display()))?;
    fsutil::sync_dir(esp).map_err(|err| format!("syncing the boot partition: {err}"))?;
    Ok(Wiped::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records the commands, and makes `mke2fs` empty the mount point the
    /// way a new filesystem would look once mounted.
    struct Fake {
        calls: Vec<String>,
        mount: std::path::PathBuf,
        fail_mkfs: bool,
    }

    impl Run for Fake {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<(), String> {
            self.calls.push(format!("{program} {}", args.join(" ")));
            if program == "mke2fs" {
                if self.fail_mkfs {
                    return Err("mke2fs failed".to_string());
                }
                let _ = fs::remove_dir_all(&self.mount);
            }
            Ok(())
        }
    }

    fn outcome() -> Outcome {
        Outcome {
            applied: true,
            message: "update applied".to_string(),
            source: "tessaro.wic.zst".to_string(),
            wiped_data: false,
            attempts: 1,
            reported: false,
        }
    }

    #[test]
    fn data_is_recreated_and_the_result_lands_in_it() {
        let dir = tempfile::tempdir().unwrap();
        let (esp, mount) = (dir.path().join("esp"), dir.path().join("data"));
        fs::create_dir_all(&esp).unwrap();
        fs::create_dir_all(mount.join("tessaro")).unwrap();
        fs::write(mount.join("tessaro/state.json"), "{}").unwrap();
        let mut fake = Fake {
            calls: Vec::new(),
            mount: mount.clone(),
            fail_mkfs: false,
        };

        begin(&esp, Path::new("/dev/sda3"), &mount, &outcome(), &mut fake).unwrap();

        assert_eq!(fake.calls[1], "mke2fs -F -q -t ext4 -L data /dev/sda3");
        assert!(!mount.join("tessaro/state.json").exists());
        let result: Outcome =
            fsutil::read_json(&mount.join("tessaro/update").join(RESULT)).unwrap();
        assert!(result.wiped_data);
        assert!(!esp.join(WIPE_MARKER).exists());
    }

    #[test]
    fn an_interrupted_wipe_leaves_the_marker_for_the_next_boot() {
        let dir = tempfile::tempdir().unwrap();
        let (esp, mount) = (dir.path().join("esp"), dir.path().join("data"));
        fs::create_dir_all(&esp).unwrap();
        let mut fake = Fake {
            calls: Vec::new(),
            mount: mount.clone(),
            fail_mkfs: true,
        };
        assert!(begin(&esp, Path::new("/dev/sda3"), &mount, &outcome(), &mut fake).is_err());
        assert!(esp.join(WIPE_MARKER).exists());

        fake.fail_mkfs = false;
        let wiped = finish(&esp, Path::new("/dev/sda3"), &mount, &mut fake).unwrap();
        assert_eq!(wiped, Wiped::Done);
        assert!(!esp.join(WIPE_MARKER).exists());
        assert!(mount.join("tessaro/update").join(RESULT).exists());
    }

    #[test]
    fn a_wipe_that_keeps_failing_gives_up_and_boots_on() {
        let dir = tempfile::tempdir().unwrap();
        let (esp, mount) = (dir.path().join("esp"), dir.path().join("data"));
        fs::create_dir_all(&esp).unwrap();
        let mut fake = Fake {
            calls: Vec::new(),
            mount: mount.clone(),
            fail_mkfs: true,
        };
        // The boot that applied the image, then the boots that retried.
        assert!(begin(&esp, Path::new("/dev/sda3"), &mount, &outcome(), &mut fake).is_err());
        for _ in 1..MAX_ATTEMPTS {
            assert!(finish(&esp, Path::new("/dev/sda3"), &mount, &mut fake).is_err());
        }
        let wiped = finish(&esp, Path::new("/dev/sda3"), &mount, &mut fake).unwrap();
        assert!(matches!(wiped, Wiped::GaveUp(_)), "{wiped:?}");
        assert!(!esp.join(WIPE_MARKER).exists());
        let tries = fake
            .calls
            .iter()
            .filter(|call| call.starts_with("mke2fs"))
            .count();
        assert_eq!(tries as u32, MAX_ATTEMPTS);
    }
}
