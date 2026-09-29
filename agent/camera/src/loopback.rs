//! v4l2loopback's control node, `/dev/v4l2loopback`: adding the virtual
//! camera a mirror writes into, and removing it again. The struct and the
//! ioctl numbers are `v4l2loopback.h` at the tag the recipe pins
//! (`meta-tessaro-distro/recipes-kernel/v4l2loopback/`).
//!
//! Neither call checks a capability: the driver trusts whoever can open the
//! control node, which is root's 0600. That is why the unit can run with an
//! empty capability set.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::v4l2::{ioc, ioctl};

pub const CONTROL: &str = "/dev/v4l2loopback";

/// `struct v4l2_loopback_config`.
#[repr(C)]
struct Config {
    /// The node number, -1 for the first free one.
    output_nr: i32,
    /// `capture_nr` in the driver's own comments; split devices are not
    /// implemented, so it is ignored.
    unused: i32,
    card_label: [u8; 32],
    /// 0 is the driver's default for each of these.
    min_width: u32,
    max_width: u32,
    min_height: u32,
    max_height: u32,
    max_buffers: i32,
    max_openers: i32,
    debug: i32,
    /// 0 is exclusive caps: the node says it is a capture device once a
    /// writer is streaming into it, and an output device before that.
    /// Chromium skips a node that says it is both capture and output, so
    /// this is what makes the virtual camera a camera to a page.
    announce_all_caps: i32,
}

const _: () = assert!(size_of::<Config>() == 72);

// _IOW('~', 1, struct v4l2_loopback_config) and _IOW('~', 2, __u32).
const CTL_ADD: u64 = ioc(1, b'~', 1, size_of::<Config>());
const CTL_REMOVE: u64 = ioc(1, b'~', 2, size_of::<u32>());

const _: () = assert!(CTL_ADD == 0x4048_7e01);
const _: () = assert!(CTL_REMOVE == 0x4004_7e02);

/// How many may have the virtual camera open at once, the mirror and udev's
/// probe included. The driver's default is 10; every tab of a page and the
/// tracker count.
const MAX_OPENERS: i32 = 32;

/// How long removal waits for the last reader to close the node.
const REMOVE_WAIT: Duration = Duration::from_secs(3);

pub struct Control {
    file: File,
}

impl Control {
    pub fn open() -> io::Result<Control> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(CONTROL)
            .map_err(|err| io::Error::new(err.kind(), format!("{CONTROL}: {err}")))?;
        Ok(Control { file })
    }

    /// A new virtual camera named `label`. Returns its node number,
    /// `/dev/video<nr>`.
    pub fn add(&self, label: &str) -> io::Result<u32> {
        let mut config = Config {
            output_nr: -1,
            unused: -1,
            card_label: [0; 32],
            min_width: 0,
            max_width: 0,
            min_height: 0,
            max_height: 0,
            max_buffers: 0,
            max_openers: MAX_OPENERS,
            debug: 0,
            announce_all_caps: 0,
        };
        // At most 31 bytes, so it stays NUL-terminated, cut on a character
        // boundary.
        let mut end = label.len().min(config.card_label.len() - 1);
        while !label.is_char_boundary(end) {
            end -= 1;
        }
        config.card_label[..end].copy_from_slice(&label.as_bytes()[..end]);
        let nr = ioctl(self.file.as_raw_fd(), CTL_ADD, &mut config)?;
        Ok(nr as u32)
    }

    /// Remove `/dev/video<nr>`. The driver refuses while anything still has
    /// it open, so a reader closing just now gets a moment; `Busy` when one
    /// is still there after it.
    pub fn remove(&self, nr: u32, wait: bool) -> Removed {
        let started = Instant::now();
        loop {
            // CTL_REMOVE takes the number itself as its argument, not a
            // pointer to it.
            // SAFETY: a plain integer argument.
            let ret =
                unsafe { libc::ioctl(self.file.as_raw_fd(), CTL_REMOVE as _, nr as libc::c_ulong) };
            if ret >= 0 {
                return Removed::Done;
            }
            let err = io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::EBUSY) if wait && started.elapsed() < REMOVE_WAIT => {
                    sleep(Duration::from_millis(100));
                }
                Some(libc::EBUSY) => return Removed::Busy,
                _ => return Removed::Gone(err),
            }
        }
    }
}

pub enum Removed {
    Done,
    /// A reader still has it open.
    Busy,
    /// It is not there, or not a loopback: nothing left to remove.
    Gone(io::Error),
}

/// Virtual cameras a mirror could not remove because a reader still had
/// them open, in `/run/tessaro-camera/<device>.leftover`. The next mirror of
/// the same camera keeps trying, so a page that holds on to the old one
/// through a restart does not leave it behind for good. A number is only
/// ever in the file of the camera whose mirror made it, so nothing else's
/// node is removed by mistake.
pub struct Leftovers {
    path: PathBuf,
    numbers: Vec<u32>,
}

impl Leftovers {
    pub fn load(path: &Path) -> Leftovers {
        let numbers = fs::read_to_string(path)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|word| word.parse().ok())
            .collect();
        Leftovers {
            path: path.to_path_buf(),
            numbers,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.numbers.is_empty()
    }

    pub fn add(&mut self, nr: u32) {
        if !self.numbers.contains(&nr) {
            self.numbers.push(nr);
        }
        self.save();
    }

    /// Try each once, without waiting; keep the ones still in use.
    pub fn sweep(&mut self, control: &Control) {
        let before = self.numbers.len();
        self.numbers.retain(|&nr| match control.remove(nr, false) {
            Removed::Done => {
                eprintln!("removed /dev/video{nr}, left over from an earlier start");
                false
            }
            Removed::Busy => true,
            Removed::Gone(_) => false,
        });
        if self.numbers.len() != before {
            self.save();
        }
    }

    fn save(&self) {
        let result = if self.numbers.is_empty() {
            match fs::remove_file(&self.path) {
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        } else {
            let body: Vec<String> = self.numbers.iter().map(u32::to_string).collect();
            fs::write(&self.path, body.join("\n") + "\n")
        };
        if let Err(err) = result {
            eprintln!("{}: {err}", self.path.display());
        }
    }
}
