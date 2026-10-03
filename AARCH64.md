# Tessaro on Apple Silicon - handoff

Working notes for continuing on a Mac. Not a doc of the system yet: once
this lands, the build parts move to `docs/build.md` and the rest to a new
`docs/` file, and this file goes.

## Intent

Let someone try Tessaro on an Apple Silicon Mac with as little effort as
possible:

* boot a prebuilt Tessaro image in a VM, natively fast;
* give it the Mac's display, audio (output and microphone) and webcam;
* ship `tessaro-ctl` and `tessaro-gui` for macOS alongside it;
* forward the API port so Webconfig opens in the Mac's browser;
* wrap it in a simple launcher (start/stop, open Webconfig, open the GUI,
  a terminal with `tessaro-ctl` ready).

## Inspiration: try-omarchy

<https://github.com/omacom/try-omarchy> runs Omarchy (Arch + Hyprland) as a
macOS app. How it works:

* **Guest:** their own ARM64 Arch Linux image, built in a privileged ARM64
  Docker container (`guest/Containerfile`, `guest/build.sh`) from a pinned
  package lock and a pinned Omarchy commit plus patches. The output is a raw
  disk plus a separate kernel `Image` and initramfs. QEMU boots them directly
  (`-kernel`/`-initrd`, `root=/dev/vda`), with no bootloader. The factory disk
  is copied to `~/Library/Application Support/Try Omarchy/VM/v1` on the first
  launch.
* **QEMU:** built from source (`macos/build-qemu-gpu-runtime.sh`), QEMU 11.1.1
  with `--target-list=aarch64-softmmu --enable-hvf --enable-cocoa
  --enable-opengl --enable-virglrenderer`, patches in `macos/patches/`.
  virglrenderer is also built from source with patches. The other libraries
  are checksum-pinned Homebrew `arm64_sequoia` bottles
  (`pinned-runtime-bottles.sh`), relinked into the bundle with
  `install_name_tool` (`bundle-macho-dependencies.sh`). What ships is listed
  in `macos/runtime-files.txt`. QEMU is signed with the
  `com.apple.security.hypervisor` entitlement (`qemu-hvf.entitlements`), and
  the app is notarized into a DMG.
* **Launch:** `-machine virt,accel=hvf,gic-version=3 -cpu host`,
  `virtio-gpu-gl-pci` with VirGL in QEMU's Cocoa window (`gl=on`), see
  `macos/run-qemu-gpu.sh`.
* **Launcher:** Swift/AppKit (`macos/Sources/OmarchyVMHelper`): start menu,
  permissions, disk management.
* **Mac integrations:** a virtio-serial port each (clipboard, camera, battery,
  time zone, Touch ID), carrying JSON between a Swift bridge on the Mac and an
  agent in the guest. The camera sends raw 720p NV12 frames into a
  v4l2loopback device in the guest.

## Decisions so far

* **The image must be aarch64.** An x86-64 guest on Apple Silicon runs on
  QEMU's TCG emulator, which is far too slow for Chromium. An aarch64 guest
  runs on the CPU through Hypervisor.framework (HVF).
* **The image builds on the Linux build host, no Mac needed.** Yocto
  cross-compiles everything; the host architecture does not matter. aarch64
  Chromium already builds for the Pi targets.
* **The Mac is needed for:** building `tessaro-ctl`/`tessaro-gui` for macOS
  (or a GitHub macOS runner), building and signing the launcher and any
  bundled QEMU, and testing HVF, the display, audio and the camera.

## Machine choice

A new `kas/machine/<name>.yml` plus its own wks. The frozen partition layout
binds per machine, so a new machine is free to choose.

* `qemuarm64` (oe-core): the reference ARM QEMU machine, the counterpart of
  our `qemux86-64` target.
* `genericarm64` (meta-yocto-bsp): a generic machine that boots through UEFI.
  It would keep the ESP and systemd-boot layout of x86, so `image:update` and
  `--repartition` could work the same way. On the Mac it needs AAVMF/edk2
  (`edk2-aarch64-code.fd`, which QEMU ships).

**Chosen: `genericarm64`** (`kas/machine/genericarm64.yml`,
`meta-tessaro-distro/wic/tessaro-image-base-genericarm64.wks.in`): UEFI boot,
which UTM and QEMU with edk2 do by default, and the genericx86-64 disk layout
with its own identifiers.

**Its tune is the Pi 3's `cortexa53`**, plain ARMv8.0 plus CRC that any
Apple Silicon runs under HVF, so most target packages come from the Pi 3's
sstate (the dry run matched 86%). Chromium still builds once for this
machine: mesa, systemd, wayland and others below it differ by machine
(MACHINE_FEATURES, the rpi overrides). Requiring `tune-cortexa53.inc` from
local.conf does not work: genericarm64.conf requires `arch-armv8a.inc` too,
and the second inclusion doubles variables in every signature.

**The kernel needs `pci-host-generic` built in.** genericarm64 builds QEMU
virt's PCIe host bridge as a module that its own initramfs carries; the image
bundles `tessaro-initramfs`, which has no modules, so without it the guest
sees no disk, network or GPU and stops after `Run /init`. That, V4L2, UVC,
the VM sound cards and USB/IP are in
`meta-tessaro-distro/recipes-kernel/linux/files/tessaro-genericarm64.cfg`.
A real SystemReady board may need its own storage controller built in the
same way.

## Status

**Booted on the x86 build host** (TCG, so slow): U-Boot, systemd-boot, the
kernel, then the agent, Weston, Chromium (rendering through VirGL on the
host GPU), PipeWire with the Intel HDA card, and the API on
`127.0.0.1:7401`. `tessaro-ctl device status` reports `machine
genericarm64`, and `tessaro-ctl screen screenshot` shows the welcome page.

**Not yet run on a Mac.**

## Running it

```sh
# build host
mise run image:build:arm64                       # the image plus U-Boot for QEMU
mise run qemu:vnc:arm64                          # VNC 127.0.0.1:5901, password "tessaro"
mise run qemu:run:arm64                          # serial console only

# Mac (Apple Silicon)
brew install qemu
TESSARO_MACHINE=genericarm64 mise run image:pull # into the repo root
mise run qemu:run:arm64                          # HVF, a Cocoa window, CoreAudio, vmnet (sudo)
mise run ctl:run -- nodes list                   # finds it by mDNS, as NAME or NAME.local
mise run qemu:run:arm64 --no-vmnet               # slirp instead, no sudo
mise run ctl:run -- -n 127.0.0.1:7401 device status
open https://127.0.0.1:7401                      # Webconfig
```

Both go through `scripts/qemu-arm64.sh`: the serial console on the terminal
(Ctrl-a x quits) and `-snapshot` so the image stays as pulled. The build
host and `--no-vmnet` use slirp, with SSH on `127.0.0.1:2222` and the API
and Webconfig on `127.0.0.1:7401`; the Mac's default is vmnet (Peripherals,
the network row). On the Mac
the firmware is Homebrew's `edk2-aarch64-code.fd`; on Linux it is the U-Boot
`image:build:arm64` builds, because Yocto has no aarch64 edk2 without
meta-arm.

## Peripherals

| Piece | Plan |
| --- | --- |
| Display | `virtio-gpu-gl` with VirGL, on QEMU's Cocoa display with `gl=on`. Homebrew's QEMU is built without VirGL, which is why try-omarchy builds its own. **Biggest risk:** `kas/machine/qemux86-64.yml` notes the kiosk renders nothing without VirGL (Chromium's unprivileged renderer cannot allocate dumb buffers on `kms_swrast`). |
| Audio | QEMU's `coreaudio` audiodev plus `intel-hda` or `virtio-sound`, with the matching kernel modules in the image (see the `MACHINE_EXTRA_RRECOMMENDS` for sound in `kas/machine/qemux86-64.yml`). |
| Webcam | QEMU emulates no camera (see **Testing in qemu** in `docs/camera.md`). Reuse `test/usbcam/usbcam.rb`, the USB/IP UVC camera, with ffmpeg's `-f avfoundation` as its source instead of a clip: the guest gets a real USB camera and the image needs no change beyond the usbip bits already on qemux86-64. Passing through the Mac's built-in camera does not work. |
| Network, Webconfig, ctl | slirp `hostfwd` in `scripts/qemu-arm64.sh`, the same ports as qemu:run on qemux86-64: `127.0.0.1:7401` to the guest's 7400 (API and Webconfig), `127.0.0.1:2222` to 22. 7401 because `dev:tunnel` holds 7400 for the device on the desk. **On a Mac the default is `TESSARO_QEMU_NET=vmnet-shared`, for mDNS** (`--no-vmnet` for slirp): macOS's shared NAT network in place of slirp (`bridge100`, an address from the Mac's DHCP), where `nodes list` and `NAME.local` find the guest; slirp carries no multicast. It needs root, so QEMU runs under sudo, and the forwards go with slirp. One NIC, not slirp plus vmnet: the managed ethernet profile binds no interface, so it would come up on only one of them. |
| ctl and gui | `cargo build --release` for `aarch64-apple-darwin` (`mise run ctl:build`, `mise run gui:build` on the Mac). |

## Plan

1. **Linux build host:** done, see Status.
2. **Mac:** `mise run qemu:run` as above. The first question is whether the
   kiosk renders: Homebrew's QEMU has no VirGL, so the guest gets a plain
   virtio-gpu (see the Display row).
3. **Mac, if Homebrew's QEMU does not render it, UTM** (<https://mac.getutm.app>, a free QEMU-based VM
   app that may already handle GPU acceleration): import the
   image, use the HVF accelerator, VirGL display, CoreAudio and port forwards
   for 7400 and 22. This answers whether Chromium renders, how fast it is and
   whether audio works, before any launcher is written.
4. **Mac:** build `tessaro-ctl` and `tessaro-gui`, reach the VM with
   `tessaro-ctl -n 127.0.0.1:7401 ...`, open Webconfig in the browser.
5. **Mac:** the webcam over usbcam.rb with an AVFoundation source
   (`usbcam:run` is still qemux86-64 only).
6. If all of that holds up, decide the package: a `.utm` bundle plus the
   macOS binaries may already be enough. A custom Swift launcher that bundles
   its own QEMU (try-omarchy's route) is a large job and only worth it after
   the UTM test.

## Open questions

* Does `image:update` work on genericarm64? The layout is ready for it, the
  updater's kernel file name on this machine is untested.
* Does VirGL on macOS (UTM's or a self-built QEMU) render our Chromium?
* Ship Ruby for usbcam.rb on the Mac, or rewrite the camera source in Swift?
