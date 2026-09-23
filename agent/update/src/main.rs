//! tessaro-flash: applies a prepared update from the initramfs.
//!
//! Called by the initramfs hook (`/init.d/80-tessaro_update`) with the
//! staging directory on the mounted `/data`, the root partition's device
//! node and the mounted ESP. Everything it prints goes to the console.
//!
//! ```text
//! tessaro-flash apply --dir DIR --root DEV --esp DIR [--data-device DEV --data-mount DIR]
//! tessaro-flash finish-wipe --esp DIR --data-device DEV --data-mount DIR
//! ```
//!
//! `apply` exits 0 (written, reboot), 1 (retry: reboot), 2 (refused, nothing
//! written: boot on), 3 (nothing pending: boot on) or 4 (gave up).
//! `finish-wipe` exits 0 (done, reboot), 1 (retry: reboot) or 2 (out of
//! attempts, marker removed: boot on).
//!
//! Commands it runs (`umount`, `mke2fs`, `mount`) are looked up on PATH,
//! which the kernel does not set for /init - the initramfs hook exports it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

use update::flash::{self, Exit, Targets};
use update::wipe::{self, System};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        return usage();
    };
    let Some(options) = options(rest) else {
        return usage();
    };
    let path = |name: &str| options.get(name).map(PathBuf::from);
    let mut console = std::io::stdout();

    match command.as_str() {
        "apply" => {
            let (Some(dir), Some(root), Some(esp)) = (path("dir"), path("root"), path("esp"))
            else {
                return usage();
            };
            let data = path("data-device").zip(path("data-mount"));
            let targets = Targets {
                dir,
                root,
                esp,
                data,
            };
            let exit = flash::apply_pending(&targets, &mut console, &mut System);
            ExitCode::from(exit as u8)
        }
        "finish-wipe" => {
            let (Some(esp), Some(device), Some(mount)) =
                (path("esp"), path("data-device"), path("data-mount"))
            else {
                return usage();
            };
            flash::say(&mut console, "finishing an interrupted wipe of /data");
            match wipe::finish(&esp, &device, &mount, &mut System) {
                Ok(wipe::Wiped::Done) => ExitCode::from(Exit::Applied as u8),
                // The marker is gone; boot on with /data as it is.
                Ok(wipe::Wiped::GaveUp(why)) => {
                    flash::say(&mut console, &why);
                    ExitCode::from(Exit::Refused as u8)
                }
                Err(err) => {
                    flash::say(&mut console, &format!("re-creating /data failed: {err}"));
                    ExitCode::from(Exit::Retry as u8)
                }
            }
        }
        _ => usage(),
    }
}

/// `--name value` pairs, nothing else.
fn options(args: &[String]) -> Option<HashMap<String, String>> {
    let mut options = HashMap::new();
    let mut args = args.iter();
    while let Some(name) = args.next() {
        let name = name.strip_prefix("--")?;
        options.insert(name.to_string(), args.next()?.clone());
    }
    Some(options)
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: tessaro-flash apply --dir DIR --root DEV --esp DIR [--data-device DEV --data-mount DIR]\n       \
         tessaro-flash finish-wipe --esp DIR --data-device DEV --data-mount DIR"
    );
    ExitCode::from(64)
}
