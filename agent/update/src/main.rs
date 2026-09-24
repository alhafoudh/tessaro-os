//! tessaro-flash: applies a prepared update from the initramfs.
//!
//! Called by the initramfs hook (`/init.d/80-tessaro_update`) with the
//! staging directory on the mounted `/data`, the root partition's device
//! node and the mounted ESP. Everything it prints goes to the console, and
//! to the screen as well where the console is only a serial port (the Pi's
//! `console=serial0`): a technician looking at a kiosk that is rewriting its
//! own disk has to see that it is, or the power gets pulled mid-write.
//!
//! ```text
//! tessaro-flash apply --dir DIR --root DEV --esp DIR [--data-device DEV --data-mount DIR]
//!                     [--disk DEV --ram DIR [--meminfo FILE]]
//! tessaro-flash finish-wipe --esp DIR --data-device DEV --data-mount DIR
//! ```
//!
//! `--disk` and `--ram` are what a disk update (`--repartition`) needs: the
//! whole disk to write, and where to mount the tmpfs the upload is copied
//! into first. Without them one is refused, and a root update ignores them.
//!
//! `apply` exits 0 (written, reboot), 1 (retry: reboot), 2 (refused, nothing
//! written: boot on), 3 (nothing pending: boot on) or 4 (gave up).
//! `finish-wipe` exits 0 (done, reboot), 1 (retry: reboot) or 2 (out of
//! attempts, marker removed: boot on).
//!
//! Commands it runs (`umount`, `mke2fs`, `mount`) are looked up on PATH,
//! which the kernel does not set for /init - the initramfs hook exports it.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use update::flash::{self, Disk, Exit, Targets};
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
    let mut console = Console {
        out: io::stdout(),
        screen: screen(),
    };

    match command.as_str() {
        "apply" => {
            let (Some(dir), Some(root), Some(esp)) = (path("dir"), path("root"), path("esp"))
            else {
                return usage();
            };
            let data = path("data-device").zip(path("data-mount"));
            let disk = path("disk").zip(path("ram")).map(|(device, ram)| Disk {
                device,
                ram,
                meminfo: path("meminfo").unwrap_or_else(|| "/proc/meminfo".into()),
            });
            let targets = Targets {
                dir,
                root,
                esp,
                data,
                disk,
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

/// stdout, which is `/dev/console`, plus the screen when that is not
/// already one of the consoles.
struct Console {
    out: Stdout,
    screen: Option<File>,
}

impl Write for Console {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(screen) = self.screen.as_mut() {
            // Best effort: the screen is a courtesy, the console the record.
            let _ = screen.write_all(buf);
        }
        self.out.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(screen) = self.screen.as_mut() {
            let _ = screen.flush();
        }
        self.out.flush()
    }
}

/// The first virtual terminal, unless a virtual terminal is already a
/// console (`console=tty0` on x86), where writing it again would print
/// every line twice. `/sys/class/tty/console/active` names them: `ttyS0
/// tty0` on qemu, `ttyS0` alone on the Pi.
fn screen() -> Option<File> {
    let active = fs::read_to_string("/sys/class/tty/console/active").unwrap_or_default();
    let on_screen = active.split_whitespace().any(|console| {
        console
            .strip_prefix("tty")
            .is_some_and(|number| number.parse::<u32>().is_ok())
    });
    if on_screen {
        return None;
    }
    OpenOptions::new().write(true).open("/dev/tty1").ok()
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
         \x20                   [--disk DEV --ram DIR [--meminfo FILE]]\n       \
         tessaro-flash finish-wipe --esp DIR --data-device DEV --data-mount DIR"
    );
    ExitCode::from(64)
}
