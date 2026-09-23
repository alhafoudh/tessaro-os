//! What tessaro-flash does with a pending update: the marker, the attempt
//! count, the result, the cleanup, and which of those the initramfs hook
//! should act on. `apply` does the writing; this decides what it means.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::apply::{self, Failure};
use crate::manifest::{Manifest, Outcome, Pending};
use crate::wipe::{self, Run};
use crate::{fsutil, MANIFEST, PENDING, RESULT, STAGING};

/// Boots that may try before the updater stops rebooting in a loop.
pub const MAX_ATTEMPTS: u32 = 5;

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
    /// Out of attempts with the root partly written. Stop and say so.
    GaveUp = 4,
}

pub struct Targets {
    pub dir: PathBuf,
    pub root: PathBuf,
    pub esp: PathBuf,
    /// Only needed when the update asks for `/data` to be wiped.
    pub data: Option<(PathBuf, PathBuf)>,
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
    let source = fsutil::read_json::<Manifest>(&targets.dir.join(MANIFEST))
        .map(|manifest| manifest.source.name)
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
    use crate::testing::{staged, MIB};
    use crate::{KERNEL, ROOT_IMAGE};
    use std::fs;

    struct NoCommands;
    impl Run for NoCommands {
        fn run(&mut self, program: &str, _args: &[&str]) -> Result<(), String> {
            panic!("{program} should not run");
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
        };
        (dir, targets)
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
        assert!(targets.dir.join(ROOT_IMAGE).exists());
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
}
