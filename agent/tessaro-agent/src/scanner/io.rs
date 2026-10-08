//! A scanner's device node, read without waiting: an evdev keyboard taken
//! from everyone else (`EVIOCGRAB`), a tty in raw mode that nobody else may
//! open (`TIOCEXCL`), or a hidraw node.
//!
//! Opening and setting a device up are syscalls that answer at once; the
//! caller still runs them on `blocking`. `Reader::read` waits only in
//! `readable()`, which the worker's `select!` ends.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use protocol::scanner::Transport;
use tokio::io::unix::AsyncFd;

/// `_IOW('E', 0x90, int)`.
const EVIOCGRAB: u32 = (1 << 30) | (4 << 16) | ((b'E' as u32) << 8) | 0x90;
/// `linux/input-event-codes.h`: the event types an evdev node gives.
const EV_KEY: u16 = 0x01;
#[cfg(test)]
const EV_SYN: u16 = 0x00;
#[cfg(test)]
const EV_MSC: u16 = 0x04;

/// One open device node.
pub struct Reader {
    fd: AsyncFd<File>,
}

/// Open `node` for `transport`. `grab` takes a keyboard from Weston and
/// everyone else, and a tty from any other opener; without it the device
/// is only listened to (`scanner identify`).
pub fn open(node: &Path, transport: Transport, grab: bool, baud: u32) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    let flags = match transport {
        Transport::Serial => libc::O_NONBLOCK | libc::O_NOCTTY,
        _ => libc::O_NONBLOCK,
    };
    if transport == Transport::Serial && grab {
        options.write(true);
    }
    let file = options.custom_flags(flags).open(node)?;
    match transport {
        Transport::Keyboard if grab => {
            // SAFETY: EVIOCGRAB takes an int by value.
            let result = unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGRAB as _, 1 as libc::c_int) };
            if result < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Transport::Serial if grab => {
            // SAFETY: TIOCEXCL takes no argument.
            if unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCEXCL as _) } < 0 {
                return Err(io::Error::last_os_error());
            }
            raw(&file, baud)?;
        }
        _ => {}
    }
    Ok(file)
}

/// The tty in raw mode at `baud`: every byte as it comes, nothing echoed or
/// translated, 8 data bits, no parity, one stop bit, the modem lines
/// ignored.
fn raw(file: &File, baud: u32) -> io::Result<()> {
    let fd = file.as_raw_fd();
    // SAFETY: an all-zero termios is a valid value for tcgetattr to fill.
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: fd is an open tty, termios a valid pointer.
    if unsafe { libc::tcgetattr(fd, &mut termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: termios was filled by tcgetattr.
    unsafe { libc::cfmakeraw(&mut termios) };
    termios.c_cflag |= libc::CLOCAL | libc::CREAD;
    termios.c_cc[libc::VMIN] = 1;
    termios.c_cc[libc::VTIME] = 0;
    // SAFETY: as above; speed is one of the libc constants.
    if unsafe { libc::cfsetspeed(&mut termios, speed(baud)) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above.
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: as above. What was queued before the scanner was ours is no
    // scan of its.
    unsafe { libc::tcflush(fd, libc::TCIFLUSH) };
    Ok(())
}

fn speed(baud: u32) -> libc::speed_t {
    match baud {
        1200 => libc::B1200,
        2400 => libc::B2400,
        4800 => libc::B4800,
        19200 => libc::B19200,
        38400 => libc::B38400,
        57600 => libc::B57600,
        115_200 => libc::B115200,
        _ => libc::B9600,
    }
}

impl Reader {
    /// Register an open node with the runtime: on its thread.
    pub fn new(file: File) -> io::Result<Self> {
        Ok(Self {
            fd: AsyncFd::new(file)?,
        })
    }

    /// Wait for the node and read what is there. `Ok(0)` or an error is the
    /// device gone.
    pub async fn read(&self, buffer: &mut [u8]) -> io::Result<usize> {
        loop {
            // naked: the worker's select! ends the wait
            let mut ready = self.fd.readable().await?;
            match ready.try_io(|fd| fd.get_ref().read(buffer)) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }
}

/// The key events among what an evdev node gave: `(code, value)` of each
/// `EV_KEY`. Whatever does not fill a whole event is dropped; evdev never
/// splits one.
pub fn key_events(bytes: &[u8]) -> Vec<(u16, i32)> {
    const SIZE: usize = std::mem::size_of::<libc::input_event>();
    bytes
        .chunks_exact(SIZE)
        .filter_map(|chunk| {
            // SAFETY: a whole input_event's bytes, read unaligned.
            let event: libc::input_event =
                unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast()) };
            (event.type_ == EV_KEY).then_some((event.code, event.value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grab_request_is_the_kernel_headers() {
        assert_eq!(EVIOCGRAB, 0x4004_4590);
    }

    #[test]
    fn key_events_are_picked_out_of_what_evdev_gives() {
        let event = |type_: u16, code: u16, value: i32| {
            let event = libc::input_event {
                time: libc::timeval {
                    tv_sec: 0,
                    tv_usec: 0,
                },
                type_,
                code,
                value,
            };
            // SAFETY: a plain C struct as its bytes.
            unsafe {
                std::slice::from_raw_parts(
                    (&event as *const libc::input_event).cast::<u8>(),
                    std::mem::size_of::<libc::input_event>(),
                )
            }
            .to_vec()
        };
        let mut bytes = event(EV_MSC, 4, 0x70004);
        bytes.extend(event(EV_KEY, 30, 1));
        bytes.extend(event(EV_SYN, 0, 0));
        bytes.extend(event(EV_KEY, 30, 0));
        bytes.extend([0u8; 3]);
        assert_eq!(key_events(&bytes), vec![(30, 1), (30, 0)]);
    }

    #[test]
    fn a_device_that_is_not_there_fails_to_open() {
        assert!(open(
            Path::new("/nonexistent/event0"),
            Transport::Keyboard,
            true,
            9600
        )
        .is_err());
    }
}
