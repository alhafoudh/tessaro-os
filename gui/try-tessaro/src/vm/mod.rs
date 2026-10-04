//! The VM: its disk, its command line, the running QEMU and how it stops.

pub mod disk;
pub mod ports;
pub mod qemu;
pub mod qmp;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::paths::Bundle;
use crate::settings::{Resolution, Settings};
use ports::Ports;

/// How long the device gets to shut down after the power button before
/// QEMU is told to quit, and how long after that before it is killed.
const POWERDOWN: Duration = Duration::from_secs(30);
const QUIT: Duration = Duration::from_secs(10);

pub const QEMU_LOG: &str = "qemu.log";
pub const SERIAL_LOG: &str = "serial.log";
const QMP: &str = "qmp.sock";

/// A QEMU this launcher started.
pub struct Running {
    child: Child,
    pub ports: Ports,
    qmp: PathBuf,
    /// When the power button was pressed, and whether `quit` went after it.
    stopping: Option<(Instant, bool)>,
}

impl Running {
    pub fn spawn(
        bundle: &Bundle,
        dir: &Path,
        settings: Settings,
        resolution: Resolution,
        overlay: &Path,
    ) -> Result<Self, String> {
        let qemu = bundle.qemu_system();
        let ports = Ports::pick()?;
        let qmp = dir.join(QMP);
        let _ = std::fs::remove_file(&qmp);
        let (cpus, memory_mb) = qemu::size(cores(), host_memory_mb());
        let launch = qemu::Launch {
            qemu_data: bundle.qemu_data(),
            firmware: bundle.firmware(),
            disk: overlay.to_path_buf(),
            qmp: qmp.clone(),
            serial: dir.join(SERIAL_LOG),
            settings,
            resolution,
            ports,
            cpus,
            memory_mb,
            gl: settings.accelerated && has_gl(&qemu),
        };
        let log_path = dir.join(QEMU_LOG);
        let log =
            File::create(&log_path).map_err(|err| format!("{}: {err}", log_path.display()))?;
        let child = Command::new(&qemu)
            .args(qemu::args(&launch))
            // The virglrenderer patches log at debug by default, into
            // qemu.log for as long as the device runs.
            .env("VIRGL_LOG_LEVEL", "warning")
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|err| err.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|err| format!("{}: {err}", qemu.display()))?;
        Ok(Self {
            child,
            ports,
            qmp,
            stopping: None,
        })
    }

    /// How QEMU ended, once it has.
    pub fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) if status.success() => Some("the device stopped".to_string()),
            Ok(Some(status)) => Some(format!("QEMU ended: {status}")),
            Ok(None) => None,
            Err(err) => Some(format!("QEMU: {err}")),
        }
    }

    /// Press the power button: the device shuts down and QEMU exits.
    pub fn stop(&mut self) -> Result<(), String> {
        if self.stopping.is_none() {
            self.stopping = Some((Instant::now(), false));
        }
        qmp::execute(&self.qmp, "system_powerdown")
    }

    /// Past the power button's grace: `quit`, then kill.
    pub fn escalate(&mut self) {
        let Some((since, quit)) = self.stopping else {
            return;
        };
        let waited = since.elapsed();
        if !quit && waited >= POWERDOWN {
            self.stopping = Some((since, true));
            if qmp::execute(&self.qmp, "quit").is_ok() {
                return;
            }
        }
        if waited >= POWERDOWN + QUIT {
            let _ = self.child.kill();
        }
    }
}

impl Drop for Running {
    /// Never leave a VM behind the launcher.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn cores() -> usize {
    std::thread::available_parallelism().map_or(4, usize::from)
}

fn host_memory_mb() -> u64 {
    #[cfg(target_os = "macos")]
    {
        let bytes = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|text| text.trim().parse::<u64>().ok());
        if let Some(bytes) = bytes {
            return bytes / (1024 * 1024);
        }
    }
    16384
}

/// Whether this QEMU was built with VirGL, as `scripts/qemu-arm64.sh` asks.
fn has_gl(qemu: &Path) -> bool {
    Command::new(qemu)
        .args(["-device", "help"])
        .stderr(Stdio::null())
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).contains("\"virtio-gpu-gl-pci\""))
        .unwrap_or(false)
}
