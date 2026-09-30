//! tessaro-camera: mirrors one USB camera into v4l2loopback devices.
//!
//! ```text
//! tessaro-camera /dev/videoN
//! ```
//!
//! A V4L2 camera streams to one reader, so whoever opens it first holds it.
//! This is that one reader: udev starts `tessaro-camera@videoN.service` for
//! every USB camera (and hides the camera's own node from everyone else),
//! and the mirror copies each frame, as captured, into every one of the
//! camera's virtual cameras, `<camera> Mirror <k>`. A loopback streams to one
//! reader at a time too, so each reader takes a mirror of its own; see
//! `mirrors.rs`.
//!
//! The mode comes from `KIOSK_CAMERA_FORMAT` and `KIOSK_CAMERA_SIZE`
//! (camera.format and camera.size, rendered by the agent into
//! `/run/tessaro-camera/camera.env`); see `choice.rs`. How many mirrors it
//! makes comes from `KIOSK_CAMERA_MIRRORS` (camera.mirrors) in the same file,
//! read at start only: the agent restarts the mirror when it changes. What
//! it captures goes to `/run/tessaro-camera/videoN.json` as a
//! `protocol::CameraInfo`, which `tessaro-ctl camera list` reads back, and is
//! removed on exit along with the virtual cameras.
//!
//! Exits 0 on SIGTERM and when the camera is unplugged, non-zero on any other
//! failure, which the unit restarts. A camera it cannot mirror at all (no
//! MJPEG or YUYV) is reported in the state file and then waited out, so the
//! unit does not restart for nothing.

mod choice;
mod loopback;
mod mirrors;
mod v4l2;

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use protocol::{CameraInfo, CameraMirror, CameraMode};

use choice::Wanted;
use loopback::{Control, Leftovers, Removed};
use v4l2::{Capture, Interval, BUF_TYPE_VIDEO_CAPTURE, BUF_TYPE_VIDEO_OUTPUT};

const STATE_DIR: &str = "/run/tessaro-camera";

/// Capture buffers. Enough for the driver to fill one while another is
/// being copied out, without holding more than a few frames of latency.
const BUFFERS: u32 = 4;

/// How often the loop looks up from poll() to see whether it was asked to
/// stop, and to retry removing a left-over virtual camera.
const TICK_MS: i32 = 1000;

/// A camera that sends nothing for this long is stuck; exiting non-zero
/// has the unit start it afresh.
const STALL: Duration = Duration::from_secs(10);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// SIGTERM (systemd stopping the unit, or the camera's device going away)
/// and SIGINT set STOP. Without SA_RESTART, so a poll() in progress returns
/// EINTR and the loop sees it at once.
fn stop_on_signals() {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: a zeroed sigaction with an empty mask and a handler that
        // only stores to an atomic, which is async-signal-safe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
}

fn stopping() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Why the mirror ended, other than being asked to.
enum Stop {
    /// The camera is gone: a clean exit, udev starts a new mirror on replug.
    Unplugged,
    Failed(String),
}

type Result<T> = std::result::Result<T, Stop>;

/// ENODEV is how every call on a node answers once the camera is unplugged.
fn stop(err: io::Error, what: &str) -> Stop {
    match err.raw_os_error() {
        Some(libc::ENODEV) | Some(libc::ENXIO) => Stop::Unplugged,
        _ => Stop::Failed(format!("{what}: {err}")),
    }
}

fn check<T>(result: io::Result<T>, what: &str) -> Result<T> {
    result.map_err(|err| stop(err, what))
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let [device] = args.as_slice() else {
        return usage();
    };
    let device = PathBuf::from(device);
    let Some(name) = device.file_name().and_then(|name| name.to_str()) else {
        return usage();
    };
    let name = name.to_string();
    stop_on_signals();

    let dir = env::var_os("KIOSK_CAMERA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(STATE_DIR));
    let state = State {
        path: dir.join(format!("{name}.json")),
    };
    let result = run(&device, &name, &dir, &state);
    state.remove();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Stop::Unplugged) => {
            eprintln!("{}: unplugged", device.display());
            ExitCode::SUCCESS
        }
        Err(Stop::Failed(why)) => {
            eprintln!("{}: {why}", device.display());
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: tessaro-camera /dev/videoN");
    ExitCode::from(64)
}

fn run(device: &Path, name: &str, dir: &Path, state: &State) -> Result<()> {
    // Non-blocking, so a dequeue with nothing ready returns instead of
    // hanging past a SIGTERM; poll() is what waits.
    let camera = check(
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(device),
        "opening it",
    )
    .map_err(|stop| match stop {
        // Gone between udev's event and the unit starting.
        Stop::Failed(_) if !device.exists() => Stop::Unplugged,
        stop => stop,
    })?;
    let fd = camera.as_raw_fd();
    let identity = check(v4l2::identity(fd), "VIDIOC_QUERYCAP")?;

    let mut found = v4l2::modes(fd);
    // MJPEG first, then the largest and fastest, which is how a list reads.
    found.sort_by_key(|(mode, _)| {
        (
            mode.format != choice::MJPEG,
            std::cmp::Reverse(u64::from(mode.width) * u64::from(mode.height)),
            std::cmp::Reverse(mode.fps),
        )
    });
    let modes: Vec<CameraMode> = found.iter().map(|(mode, _)| mode.clone()).collect();

    // The camera's own name, uvcvideo's repeated product dropped and without
    // a mirror's " Mirror <k>": each mirror carries its own label.
    let camera_name = mirrors::camera_name(&identity.card);
    let mut info = CameraInfo {
        name: camera_name.clone(),
        device: name.to_string(),
        bus: identity.bus.clone(),
        mirrors: Vec::new(),
        mode: None,
        fallback: None,
        error: None,
        modes: modes.clone(),
    };

    if !identity.captures {
        return state.idle(
            info,
            "the node does not stream video, so it is not a camera",
        );
    }

    let (wanted, mut reasons) = Wanted::parse(
        env::var("KIOSK_CAMERA_FORMAT").ok().as_deref(),
        env::var("KIOSK_CAMERA_SIZE").ok().as_deref(),
    );
    let Some(choice) = choice::choose(&modes, &wanted) else {
        return state.idle(
            info,
            "the camera has neither MJPEG nor YUYV, the formats the mirror passes through",
        );
    };
    reasons.extend(choice.fallback);
    let (chosen, interval) = &found[choice.index];
    let mode = configure(&camera, chosen, *interval)?;
    info.fallback = (!reasons.is_empty()).then(|| reasons.join("; "));
    if let Some(why) = &info.fallback {
        eprintln!("{why}");
    }

    let control = check(Control::open(), "the v4l2loopback control node")?;
    let mut leftovers = Leftovers::load(&dir.join(format!("{name}.leftover")));
    leftovers.sweep(&control);

    let (count, why) = mirrors::count(env::var("KIOSK_CAMERA_MIRRORS").ok().as_deref());
    if let Some(why) = why {
        eprintln!("{why}");
    }
    // Every one or none: a loopback that cannot be added removes the ones
    // made before it, on the same path as a mirror that ends.
    let mut made: Vec<(u32, CameraMirror)> = Vec::new();
    let mut result = Ok(());
    for label in mirrors::labels(&camera_name, count) {
        match control.add(&label) {
            Ok(nr) => made.push((
                nr,
                CameraMirror {
                    name: label,
                    device: format!("/dev/video{nr}"),
                },
            )),
            Err(err) => {
                result = Err(stop(err, "adding a v4l2loopback device"));
                break;
            }
        }
    }
    if result.is_ok() {
        let loopbacks: Vec<CameraMirror> = made.iter().map(|(_, m)| m.clone()).collect();
        result = mirror(
            &camera,
            &loopbacks,
            &mode,
            &mut info,
            state,
            &control,
            &mut leftovers,
        );
    }
    // The state first, so nothing reads a virtual device that is going.
    state.remove();
    for (nr, made) in &made {
        match control.remove(*nr, true) {
            Removed::Done => {}
            Removed::Busy => {
                eprintln!(
                    "{} is still open, so it stays until its reader lets go",
                    made.device
                );
                leftovers.add(*nr);
            }
            Removed::Gone(err) => eprintln!("removing {}: {err}", made.device),
        }
    }
    result
}

/// Set the camera to `chosen`. Returns the mode it really took: a driver
/// may round a size or a rate to one of its own.
fn configure(camera: &File, chosen: &CameraMode, interval: Option<Interval>) -> Result<CameraMode> {
    let fd = camera.as_raw_fd();
    let pix = v4l2::pix_format(
        v4l2::pixelformat(&chosen.format),
        chosen.width,
        chosen.height,
        false,
    );
    let pix = check(
        v4l2::set_format(fd, BUF_TYPE_VIDEO_CAPTURE, pix),
        "setting the camera's format",
    )?;
    let Some(format) = v4l2::format_name(pix.pixelformat) else {
        return Err(Stop::Failed(format!(
            "the camera took {:#010x} instead of {}",
            pix.pixelformat, chosen.format
        )));
    };
    let mut fps = chosen.fps;
    if let Some(interval) = interval {
        match v4l2::set_interval(fd, BUF_TYPE_VIDEO_CAPTURE, interval) {
            Ok(set) if set.fps() > 0 => fps = set.fps(),
            Ok(_) => {}
            Err(err) if err.raw_os_error() == Some(libc::ENODEV) => return Err(Stop::Unplugged),
            // Some cameras have a fixed rate and no VIDIOC_S_PARM.
            Err(err) => eprintln!("setting the frame rate: {err}"),
        }
    }
    Ok(CameraMode {
        format: format.to_string(),
        width: pix.width,
        height: pix.height,
        fps,
    })
}

/// One virtual camera the mirror writes into.
struct Output {
    file: File,
    device: String,
    /// Its last write failed, so the next failure is not said again.
    failing: bool,
}

/// Open `device` and tell it the format and rate the camera captures.
fn open_output(device: &str, mode: &CameraMode) -> Result<Output> {
    let file = open_virtual(device)?;
    let fd = file.as_raw_fd();
    let pixelformat = v4l2::pixelformat(&mode.format);
    check(
        v4l2::set_format(
            fd,
            BUF_TYPE_VIDEO_OUTPUT,
            v4l2::pix_format(pixelformat, mode.width, mode.height, true),
        ),
        &format!("setting {device}'s format"),
    )?;
    if mode.fps > 0 {
        // What readers are told the rate is; frames go out as they come.
        let rate = Interval {
            numerator: 1,
            denominator: mode.fps,
        };
        if let Err(err) = v4l2::set_interval(fd, BUF_TYPE_VIDEO_OUTPUT, rate) {
            eprintln!("setting {device}'s frame rate: {err}");
        }
    }
    Ok(Output {
        file,
        device: device.to_string(),
        failing: false,
    })
}

/// Stream the camera into every one of `loopbacks` until asked to stop.
fn mirror(
    camera: &File,
    loopbacks: &[CameraMirror],
    mode: &CameraMode,
    info: &mut CameraInfo,
    state: &State,
    control: &Control,
    leftovers: &mut Leftovers,
) -> Result<()> {
    let mut outputs = loopbacks
        .iter()
        .map(|loopback| open_output(&loopback.device, mode))
        .collect::<Result<Vec<Output>>>()?;

    let capture = check(Capture::start(camera, BUFFERS), "starting the capture")?;
    info.mirrors = loopbacks.to_vec();
    info.mode = Some(mode.clone());
    state.write(info);
    let devices: Vec<&str> = loopbacks.iter().map(|m| m.device.as_str()).collect();
    eprintln!(
        "capturing {} {}x{} at {} fps from {:?} into {}",
        mode.format,
        mode.width,
        mode.height,
        mode.fps,
        info.name,
        devices.join(", ")
    );

    let mut last_frame = Instant::now();
    while !stopping() {
        if !leftovers.is_empty() {
            leftovers.sweep(control);
        }
        let mut poll = libc::pollfd {
            fd: camera.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, TICK_MS) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(Stop::Failed(format!("poll: {err}")));
        }
        if ready == 0 {
            if last_frame.elapsed() >= STALL {
                return Err(Stop::Failed(format!(
                    "no frame from the camera for {} s",
                    STALL.as_secs()
                )));
            }
            continue;
        }

        let frame = match capture.next() {
            Ok(frame) => frame,
            Err(err) if err.kind() == ErrorKind::WouldBlock => continue,
            Err(err) => return Err(stop(err, "VIDIOC_DQBUF")),
        };
        last_frame = Instant::now();
        if !frame.corrupt && frame.bytes > 0 {
            let bytes = capture.bytes(&frame);
            // Each loopback on its own: one that refuses a frame does not
            // keep it from the others.
            for output in &mut outputs {
                // One write is one frame to v4l2loopback, so never
                // write_all: a short write is the driver cutting the frame
                // to its buffer.
                match (&output.file).write(bytes) {
                    Ok(_) => output.failing = false,
                    Err(err) if err.kind() == ErrorKind::Interrupted => {}
                    Err(err) => {
                        // Said once, not at every frame; a reader changing
                        // the format under the mirror is the likely cause.
                        if !output.failing {
                            eprintln!("writing to {}: {err}", output.device);
                        }
                        output.failing = true;
                    }
                }
            }
        }
        check(capture.give_back(frame), "VIDIOC_QBUF")?;
    }
    Ok(())
}

/// The virtual camera's node. devtmpfs has it by the time CTL_ADD returns;
/// the retry is for a slow udev renaming or re-permissioning it.
fn open_virtual(path: &str) -> Result<File> {
    let started = Instant::now();
    loop {
        match OpenOptions::new().read(true).write(true).open(path) {
            Ok(file) => return Ok(file),
            Err(err) if err.kind() == ErrorKind::NotFound && started.elapsed().as_secs() < 2 => {
                sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(Stop::Failed(format!("{path}: {err}"))),
        }
    }
}

/// `/run/tessaro-camera/<device>.json`.
struct State {
    path: PathBuf,
}

impl State {
    /// Whole or not at all: a temporary file in the same directory, then a
    /// rename over the real one. A failure is logged, not fatal: the mirror
    /// is what matters, the file only reports on it.
    fn write(&self, info: &CameraInfo) {
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("camera.json");
        let temporary = self.path.with_file_name(format!(".{name}.tmp"));
        let result = serde_json::to_vec_pretty(info)
            .map_err(io::Error::other)
            .and_then(|mut body| {
                body.push(b'\n');
                fs::write(&temporary, &body)?;
                fs::rename(&temporary, &self.path)
            });
        if let Err(err) = result {
            eprintln!("{}: {err}", self.path.display());
            let _ = fs::remove_file(&temporary);
        }
    }

    fn remove(&self) {
        match fs::remove_file(&self.path) {
            Err(err) if err.kind() != ErrorKind::NotFound => {
                eprintln!("{}: {err}", self.path.display());
            }
            _ => {}
        }
    }

    /// A camera the mirror cannot serve: say why in the state file and wait
    /// to be stopped, rather than exit and be restarted into the same answer.
    fn idle(&self, mut info: CameraInfo, why: &str) -> Result<()> {
        eprintln!("{why}");
        info.error = Some(why.to_string());
        self.write(&info);
        while !stopping() {
            sleep(Duration::from_millis(500));
        }
        Ok(())
    }
}
