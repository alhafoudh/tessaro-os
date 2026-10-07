//! tessaro-vision: finds the faces in front of the screen.
//!
//! ```text
//! tessaro-vision
//! tessaro-vision --bench [--model NAME] [--models DIR] FRAME.jpg...
//! ```
//!
//! The agent starts `tessaro-vision.service` while camera.presence.enable is
//! on, and the camera mirrors add a hidden virtual camera, `<camera> Vision`,
//! that only root may open. This reads the one of camera.presence.camera
//! (`KIOSK_PRESENCE_CAMERA`: `auto` for the first, or a camera's name), at
//! most `KIOSK_PRESENCE_FPS` frames a second, newest first: each is decoded
//! no larger than the model needs, run through MediaPipe's BlazeFace
//! (`KIOSK_PRESENCE_MODEL`), and the faces, followed from frame to frame,
//! go to the agent as one `protocol::presence::VisionFrame` datagram on
//! `/run/tessaro-kiosk/vision.sock`. Everything that decides what a face
//! means - confidence, distance, arriving and leaving - is the agent's, so
//! its settings apply live without restarting this.
//!
//! How it is doing goes to `/run/tessaro-vision/status.json`. No frame is
//! ever written anywhere.
//!
//! `--bench` runs the model on JPEG files instead and prints the faces and
//! how long decoding and inference took: what to run on a new board.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use protocol::presence::{VisionFrame, VisionStatus};
use protocol::CameraInfo;
use tessaro_camera::choice::MJPEG;
use tessaro_camera::v4l2::Capture;
use tessaro_camera::write_whole;
use tessaro_vision::blazeface::{self, Model};
use tessaro_vision::detector::Detector;
use tessaro_vision::picture::{self, Rgb};
use tessaro_vision::track::Tracker;

const CAMERA_DIR: &str = "/run/tessaro-camera";
const STATE_DIR: &str = "/run/tessaro-vision";
const SOCKET: &str = "/run/tessaro-kiosk/vision.sock";
const MODELS: &str = "/usr/share/tessaro-vision";

/// The lowest score a face is passed on with. The agent applies
/// camera.presence.confidence on top, so changing that needs no restart.
const FLOOR: f32 = 0.3;

const DEFAULT_FPS: u32 = 5;
const MAX_FPS: u32 = 15;

const BUFFERS: u32 = 2;
const TICK_MS: i32 = 1000;

/// How often the status file is written and the camera's report read again.
const STATUS_EVERY: Duration = Duration::from_secs(2);

/// Without a frame for this long the mirror is gone or stuck: start over.
const STALL: Duration = Duration::from_secs(10);

/// How long to wait before looking for the camera again.
const RETRY: Duration = Duration::from_secs(2);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

fn path_from(var: &str, default: &str) -> PathBuf {
    env::var_os(var)
        .filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from(default), PathBuf::from)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--bench") {
        return bench(&args[1..]);
    }
    if !args.is_empty() {
        eprintln!("usage: tessaro-vision [--bench [--model NAME] [--models DIR] FRAME.jpg...]");
        return ExitCode::from(64);
    }
    stop_on_signals();

    let settings = Settings::from_env();
    let state = path_from("KIOSK_VISION_DIR", STATE_DIR).join("status.json");
    let mut status = VisionStatus {
        model: settings.model.name.to_string(),
        ..VisionStatus::default()
    };
    let detector = match Detector::load(&settings.models, settings.model) {
        Ok(detector) => detector,
        Err(why) => {
            // Said in the status and waited out: restarting into the same
            // missing file helps nobody.
            eprintln!("{why}");
            status.error = Some(why);
            write_status(&state, &mut status);
            while !stopping() {
                sleep(Duration::from_millis(500));
            }
            return ExitCode::SUCCESS;
        }
    };
    eprintln!(
        "looking for faces with {} at up to {} fps",
        settings.model.name, settings.fps
    );

    let mut sender = Sender::new(path_from("KIOSK_VISION_SOCKET", SOCKET));
    while !stopping() {
        let camera = match pick(&settings.camera_dir, &settings.camera) {
            Ok(camera) => camera,
            Err(why) => {
                if status.error.as_deref() != Some(why.as_str()) {
                    eprintln!("{why}");
                }
                status.camera = None;
                status.mirror = None;
                status.fps = None;
                status.error = Some(why);
                write_status(&state, &mut status);
                pause(RETRY);
                continue;
            }
        };
        status.camera = Some(camera.info.name.clone());
        status.mirror = Some(camera.mirror.clone());
        status.error = None;
        eprintln!("reading {} from {}", camera.info.name, camera.mirror);
        let outcome = watch(
            &camera,
            &settings,
            &detector,
            &mut sender,
            &state,
            &mut status,
        );
        if let Err(why) = outcome {
            eprintln!("{why}");
            status.error = Some(why);
            write_status(&state, &mut status);
            pause(RETRY);
        }
    }
    let _ = fs::remove_file(&state);
    ExitCode::SUCCESS
}

fn pause(how_long: Duration) {
    let until = Instant::now() + how_long;
    while !stopping() && Instant::now() < until {
        sleep(Duration::from_millis(100));
    }
}

struct Settings {
    camera: String,
    model: Model,
    fps: u32,
    models: PathBuf,
    camera_dir: PathBuf,
}

impl Settings {
    fn from_env() -> Settings {
        let camera = env::var("KIOSK_PRESENCE_CAMERA")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "auto".to_string());
        let model = env::var("KIOSK_PRESENCE_MODEL")
            .ok()
            .and_then(|name| blazeface::model(name.trim()))
            .unwrap_or(blazeface::FULL);
        let fps = env::var("KIOSK_PRESENCE_FPS")
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|fps| (1..=MAX_FPS).contains(fps))
            .unwrap_or(DEFAULT_FPS);
        Settings {
            camera,
            model,
            fps,
            models: path_from("KIOSK_VISION_MODELS", MODELS),
            camera_dir: path_from("KIOSK_CAMERA_DIR", CAMERA_DIR),
        }
    }
}

/// The camera presence detection reads, and its hidden mirror.
#[derive(Debug, Clone, PartialEq)]
struct Picked {
    info: CameraInfo,
    mirror: String,
}

/// The node number of `video12`, for the order cameras are taken in.
fn node_number(device: &str) -> u32 {
    device
        .trim_start_matches("video")
        .parse()
        .unwrap_or(u32::MAX)
}

/// camera.presence.camera among the mirrors' reports: `auto` is the camera
/// with the lowest node, a name is matched without case.
fn pick(dir: &Path, wanted: &str) -> Result<Picked, String> {
    let mut cameras: Vec<CameraInfo> = fs::read_dir(dir)
        .map_err(|err| format!("{}: {err}", dir.display()))?
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| fs::read(entry.path()).ok())
        .filter_map(|body| serde_json::from_slice::<CameraInfo>(&body).ok())
        .collect();
    cameras.sort_by_key(|camera| node_number(&camera.device));
    choose(cameras, wanted)
}

fn choose(cameras: Vec<CameraInfo>, wanted: &str) -> Result<Picked, String> {
    if cameras.is_empty() {
        return Err("no camera is plugged in".to_string());
    }
    let auto = wanted.eq_ignore_ascii_case("auto");
    let Some(camera) = cameras
        .into_iter()
        .find(|camera| auto || camera.name.eq_ignore_ascii_case(wanted))
    else {
        return Err(format!("no camera called {wanted:?} is plugged in"));
    };
    match (&camera.vision, &camera.mode) {
        (Some(vision), Some(_)) => Ok(Picked {
            mirror: vision.device.clone(),
            info: camera,
        }),
        _ => Err(format!(
            "{} has no vision mirror yet; its mirror adds one while camera.presence.enable is on",
            camera.name
        )),
    }
}

/// Datagrams to the agent. One that is not listening is said once, not at
/// every frame.
struct Sender {
    socket: Option<UnixDatagram>,
    path: PathBuf,
    failing: bool,
}

impl Sender {
    fn new(path: PathBuf) -> Sender {
        Sender {
            socket: UnixDatagram::unbound().ok(),
            path,
            failing: false,
        }
    }

    fn send(&mut self, frame: &VisionFrame) {
        let (Some(socket), Ok(body)) = (&self.socket, serde_json::to_vec(frame)) else {
            return;
        };
        match socket.send_to(&body, &self.path) {
            Ok(_) => self.failing = false,
            Err(err) => {
                if !self.failing {
                    eprintln!("telling the agent at {}: {err}", self.path.display());
                }
                self.failing = true;
            }
        }
    }
}

fn write_status(path: &Path, status: &mut VisionStatus) {
    status.updated = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    if let Ok(mut body) = serde_json::to_vec_pretty(status) {
        body.push(b'\n');
        write_whole(path, &body);
    }
}

/// An average that follows the last few values.
fn average(old: Option<f64>, new: f64) -> Option<f64> {
    Some(old.map_or(new, |old| old + (new - old) * 0.2))
}

/// Read `camera`'s hidden mirror until asked to stop, the mirror goes or the
/// report names another one.
fn watch(
    camera: &Picked,
    settings: &Settings,
    detector: &Detector,
    sender: &mut Sender,
    state: &Path,
    status: &mut VisionStatus,
) -> Result<(), String> {
    let Some(mode) = camera.info.mode.clone() else {
        return Err(format!("{} captures nothing", camera.info.name));
    };
    let file = open(&camera.mirror)?;
    let capture =
        Capture::start(&file, BUFFERS).map_err(|err| format!("{}: {err}", camera.mirror))?;
    let every = Duration::from_secs(1) / settings.fps;
    let side = detector.model.size;
    let mut tracker = Tracker::default();
    let mut last_frame = Instant::now();
    let mut looked: Option<Instant> = None;
    let mut window = (Instant::now(), 0u32);
    write_status(state, status);

    while !stopping() {
        if window.0.elapsed() >= STATUS_EVERY {
            let seconds = window.0.elapsed().as_secs_f64();
            status.fps = Some((f64::from(window.1) / seconds * 10.0).round() / 10.0);
            write_status(state, status);
            window = (Instant::now(), 0);
            match pick(&settings.camera_dir, &settings.camera) {
                Ok(now) if now == *camera => {}
                _ => return Ok(()),
            }
        }
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
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
            return Err(format!("poll: {err}"));
        }
        if ready == 0 {
            if last_frame.elapsed() >= STALL {
                return Err(format!(
                    "no frame from {} for {} s",
                    camera.mirror,
                    STALL.as_secs()
                ));
            }
            continue;
        }
        let frame = match capture.next() {
            Ok(frame) => frame,
            Err(err) if err.kind() == ErrorKind::WouldBlock => continue,
            Err(err) => return Err(format!("{}: {err}", camera.mirror)),
        };
        last_frame = Instant::now();
        // The newest frame when one is due; every other one goes straight
        // back to the driver.
        let due =
            !frame.corrupt && frame.bytes > 0 && looked.is_none_or(|at| at.elapsed() >= every);
        let bytes = due.then(|| capture.bytes(&frame).to_vec());
        capture
            .give_back(frame)
            .map_err(|err| format!("{}: {err}", camera.mirror))?;
        let Some(bytes) = bytes else {
            continue;
        };
        looked = Some(Instant::now());
        window.1 += 1;
        let t = now_ms();

        let started = Instant::now();
        let picture = if mode.format == MJPEG {
            picture::jpeg(&bytes, side)
        } else {
            picture::yuyv(&bytes, mode.width as usize, mode.height as usize, side)
        };
        let picture = match picture {
            Ok(picture) => picture,
            Err(why) => {
                status.error = Some(format!("a frame that does not decode: {why}"));
                continue;
            }
        };
        status.decode_ms = average(status.decode_ms, ms(started.elapsed()));
        let started = Instant::now();
        let faces = detector.detect(&picture, FLOOR)?;
        status.inference_ms = average(status.inference_ms, ms(started.elapsed()));
        status.error = None;
        sender.send(&VisionFrame {
            t,
            width: mode.width,
            height: mode.height,
            faces: tracker.update(faces),
        });
    }
    Ok(())
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

/// The mirror, non-blocking so poll() is what waits and a SIGTERM is seen.
fn open(device: &str) -> Result<File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(device)
        .map_err(|err| format!("{device}: {err}"))
}

/// `--bench`: every file through the model, a few times, with the faces and
/// the timings.
fn bench(args: &[String]) -> ExitCode {
    let mut model = blazeface::FULL;
    let mut models = path_from("KIOSK_VISION_MODELS", MODELS);
    let mut files = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--model" => match args.next().and_then(|name| blazeface::model(name)) {
                Some(found) => model = found,
                None => {
                    eprintln!(
                        "--model takes one of {}",
                        protocol::presence::MODELS.join(", ")
                    );
                    return ExitCode::from(64);
                }
            },
            "--models" => match args.next() {
                Some(dir) => models = PathBuf::from(dir),
                None => return ExitCode::from(64),
            },
            file => files.push(file.to_string()),
        }
    }
    let started = Instant::now();
    let detector = match Detector::load(&models, model) {
        Ok(detector) => detector,
        Err(why) => {
            eprintln!("{why}");
            return ExitCode::FAILURE;
        }
    };
    println!("{} loaded in {:.0} ms", model.name, ms(started.elapsed()));
    const RUNS: u32 = 10;
    for file in files {
        let bytes = match fs::read(&file) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("{file}: {err}");
                return ExitCode::FAILURE;
            }
        };
        let (mut decode, mut infer) = (Duration::ZERO, Duration::ZERO);
        let mut faces = Vec::new();
        for _ in 0..RUNS {
            let started = Instant::now();
            let picture: Rgb = match picture::jpeg(&bytes, model.size) {
                Ok(decoded) => decoded,
                Err(why) => {
                    eprintln!("{file}: {why}");
                    return ExitCode::FAILURE;
                }
            };
            decode += started.elapsed();
            let started = Instant::now();
            faces = detector.detect(&picture, 0.5).unwrap_or_default();
            infer += started.elapsed();
        }
        println!(
            "{file}: decode {:.1} ms, inference {:.1} ms, {} face(s)",
            ms(decode / RUNS),
            ms(infer / RUNS),
            faces.len()
        );
        for (area, score, _) in faces {
            println!(
                "  {:.2} at x {:.3} y {:.3} w {:.3} h {:.3}",
                score, area.x, area.y, area.w, area.h
            );
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{CameraMirror, CameraMode};

    fn camera(device: &str, name: &str, vision: bool) -> CameraInfo {
        CameraInfo {
            name: name.into(),
            device: device.into(),
            bus: "usb-1".into(),
            mirrors: Vec::new(),
            vision: vision.then(|| CameraMirror {
                name: format!("{name} Vision"),
                device: format!("/dev/{device}9"),
            }),
            mode: Some(CameraMode {
                format: "mjpeg".into(),
                width: 640,
                height: 480,
                fps: 30,
            }),
            fallback: None,
            error: None,
            modes: Vec::new(),
        }
    }

    #[test]
    fn auto_is_the_first_camera_and_a_name_is_matched_without_case() {
        let cameras = vec![
            camera("video0", "Front", true),
            camera("video2", "Side", true),
        ];
        assert_eq!(choose(cameras.clone(), "auto").unwrap().info.name, "Front");
        assert_eq!(choose(cameras, "side").unwrap().mirror, "/dev/video29");
    }

    #[test]
    fn a_camera_without_its_vision_mirror_says_why() {
        let why = choose(vec![camera("video0", "Front", false)], "auto").unwrap_err();
        assert!(why.contains("no vision mirror"), "{why}");
        assert!(choose(Vec::new(), "auto")
            .unwrap_err()
            .contains("no camera"));
        let why = choose(vec![camera("video0", "Front", true)], "Back").unwrap_err();
        assert!(why.contains("Back"), "{why}");
    }

    #[test]
    fn nodes_sort_by_number() {
        assert!(node_number("video2") < node_number("video10"));
        assert_eq!(node_number("odd"), u32::MAX);
    }
}
