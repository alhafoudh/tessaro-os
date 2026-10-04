//! QEMU's command line, from the settings and the bundle. Pure, so the
//! tests can read it; `scripts/qemu-arm64.sh` is the same machine for the
//! developer's `qemu:run:arm64`, and the two should not drift apart
//! (docs/try-tessaro.md, "The VM").

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::ports::Ports;
use crate::settings::{Resolution, Settings};

/// Everything the command line is made of.
#[derive(Debug, Clone)]
pub struct Launch {
    pub qemu_data: PathBuf,
    pub firmware: PathBuf,
    pub disk: PathBuf,
    pub qmp: PathBuf,
    pub serial: PathBuf,
    pub settings: Settings,
    /// The screen size, the settings' choice resolved for this computer's
    /// screen (`Settings::resolution_on`).
    pub resolution: Resolution,
    pub ports: Ports,
    pub cpus: usize,
    pub memory_mb: u64,
    /// The device draws on the GPU: the bundled QEMU has VirGL
    /// (`virtio-gpu-gl-pci`) and the settings did not turn it off. Without
    /// it the kiosk renders in software.
    pub gl: bool,
}

/// What the device is called in QEMU's window title.
pub const NAME: &str = "Tessaro";

pub fn args(launch: &Launch) -> Vec<OsString> {
    let (width, height) = launch.resolution.size();
    let gpu = if launch.gl {
        "virtio-gpu-gl-pci"
    } else {
        "virtio-gpu-pci"
    };
    // Never zoom-to-fit: it makes QEMU's window resizable, and a resizable
    // window hands its own size to the guest as the preferred mode
    // (`virtio_gpu_ui_info` in hw/display/virtio-gpu-base.c). The window
    // keeps the size the firmware's screen gave it, so the kiosk came up at
    // 640 x 360 whatever `xres`/`yres` said. A fixed window is the guest's
    // size, so the two agree.
    //
    // GL is macOS's own OpenGL (`gl=on`), never ANGLE's GLES on Metal
    // (`gl=es`): through ANGLE VirGL offers the guest desktop GL 2.1 only,
    // Chromium's GLES 3 context fails and the kiosk falls back to software
    // (docs/try-tessaro.md, "The QEMU runtime").
    let display = if launch.gl { "cocoa,gl=on" } else { "cocoa" };
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |items: &[&str]| args.extend(items.iter().map(OsString::from));
    push(&["-name", NAME]);
    push(&["-machine", "virt", "-accel", "hvf", "-cpu", "host"]);
    push(&["-smp", &launch.cpus.to_string()]);
    push(&["-m", &launch.memory_mb.to_string()]);
    push(&["-L", &path(&launch.qemu_data)]);
    push(&["-bios", &path(&launch.firmware)]);
    push(&[
        "-drive",
        &format!("file={},format=qcow2,if=virtio", option(&launch.disk)),
    ]);
    push(&[
        "-device",
        "qemu-xhci",
        "-device",
        "usb-kbd",
        "-device",
        "usb-tablet",
    ]);
    push(&["-device", &format!("{gpu},xres={width},yres={height}")]);
    push(&["-display", display]);
    push(&[
        "-netdev",
        &format!(
            "user,id=net0,hostfwd=tcp:127.0.0.1:{}-:7400,hostfwd=tcp:127.0.0.1:{}-:22",
            launch.ports.api, launch.ports.ssh
        ),
    ]);
    // No option ROM: UEFI does not boot from the network here, and the
    // bundle carries no ROMs.
    push(&["-device", "virtio-net-pci,netdev=net0,romfile="]);
    if launch.settings.sound {
        let codec = if launch.settings.microphone {
            "hda-duplex"
        } else {
            "hda-output"
        };
        push(&["-audiodev", "coreaudio,id=snd0"]);
        push(&[
            "-device",
            "intel-hda",
            "-device",
            &format!("{codec},audiodev=snd0"),
        ]);
    }
    push(&[
        "-qmp",
        &format!("unix:{},server=on,wait=off", option(&launch.qmp)),
    ]);
    push(&["-serial", &format!("file:{}", option(&launch.serial))]);
    push(&["-monitor", "none"]);
    args
}

fn path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A path inside a `key=value,...` option, where a comma is written twice.
fn option(path: &Path) -> String {
    path.to_string_lossy().replace(',', ",,")
}

/// vCPUs and memory from what this computer has: half its cores, 2 to 4,
/// and 4 GB, or 3 GB on a computer with 8 GB or less.
pub fn size(cores: usize, host_memory_mb: u64) -> (usize, u64) {
    let cpus = (cores / 2).clamp(2, 4);
    let memory = if host_memory_mb <= 8192 { 3072 } else { 4096 };
    (cpus, memory)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(settings: Settings) -> Launch {
        Launch {
            qemu_data: PathBuf::from("/App/qemu/share/qemu"),
            firmware: PathBuf::from("/App/qemu/share/qemu/edk2-aarch64-code.fd"),
            disk: PathBuf::from("/Users/me/Library/Application Support/Try Tessaro/disk.qcow2"),
            qmp: PathBuf::from("/tmp/a,b/qmp.sock"),
            serial: PathBuf::from("/data/serial.log"),
            settings,
            resolution: Resolution::FullHd,
            ports: Ports {
                api: 7401,
                ssh: 2222,
            },
            cpus: 4,
            memory_mb: 4096,
            gl: false,
        }
    }

    fn line(launch: &Launch) -> String {
        args(launch)
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn the_resolution_is_the_gpus_preferred_mode() {
        let mut launch = launch(Settings::default());
        assert!(line(&launch).contains("-device virtio-gpu-pci,xres=1920,yres=1080"));
        launch.resolution = Resolution::Portrait;
        assert!(line(&launch).contains("-device virtio-gpu-pci,xres=1080,yres=1920"));
    }

    #[test]
    fn the_window_never_resizes_the_guest() {
        let mut launch = launch(Settings::default());
        assert!(!line(&launch).contains("zoom-to-fit"));
        launch.gl = true;
        assert!(!line(&launch).contains("zoom-to-fit"));
    }

    #[test]
    fn the_api_and_ssh_are_forwarded_on_localhost_only() {
        assert!(line(&launch(Settings::default())).contains(
            "user,id=net0,hostfwd=tcp:127.0.0.1:7401-:7400,hostfwd=tcp:127.0.0.1:2222-:22"
        ));
    }

    #[test]
    fn sound_off_leaves_out_the_sound_card() {
        let settings = Settings {
            sound: false,
            microphone: true,
            ..Settings::default()
        };
        let line = line(&launch(settings));
        assert!(!line.contains("audiodev"));
        assert!(!line.contains("intel-hda"));
    }

    #[test]
    fn the_microphone_takes_the_duplex_codec() {
        let mut settings = Settings {
            sound: true,
            microphone: false,
            ..Settings::default()
        };
        assert!(line(&launch(settings)).contains("hda-output,audiodev=snd0"));
        settings.microphone = true;
        assert!(line(&launch(settings)).contains("hda-duplex,audiodev=snd0"));
    }

    #[test]
    fn paths_stay_whole_and_commas_are_escaped() {
        let args = args(&launch(Settings::default()));
        let drive = args.iter().position(|arg| arg == "-drive").unwrap();
        assert_eq!(
            args[drive + 1],
            "file=/Users/me/Library/Application Support/Try Tessaro/disk.qcow2,format=qcow2,if=virtio"
        );
        assert!(line(&launch(Settings::default()))
            .contains("unix:/tmp/a,,b/qmp.sock,server=on,wait=off"));
    }

    #[test]
    fn gl_switches_the_gpu_and_the_display() {
        let mut launch = launch(Settings::default());
        launch.gl = true;
        let line = line(&launch);
        assert!(line.contains("-device virtio-gpu-gl-pci,"));
        assert!(line.contains("-display cocoa,gl=on"));
    }

    #[test]
    fn the_vm_is_sized_from_the_host() {
        assert_eq!(size(2, 8192), (2, 3072));
        assert_eq!(size(8, 16384), (4, 4096));
        assert_eq!(size(16, 65536), (4, 4096));
        assert_eq!(size(6, 16384), (3, 4096));
    }
}
