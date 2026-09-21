# tessaro-os

Yocto/OpenEmbedded build for **Tessaro**, a web kiosk. It produces bootable
Linux images for the hardware platforms Tessaro ships on.

Built as a derivative of [Moonforge](https://moonforgelinux.org/), so images
come with an immutable read-only rootfs, systemd, an overlayfs `/etc` and a
persistent `/data` partition out of the box.

## Quick start

```sh
mise run build     # build for QEMU (the default target) plus OVMF firmware
mise run run-vnc   # boot it in QEMU, framebuffer on localhost:5900
mise run run       # same, but serial console only (Ctrl-a x to exit)
```

Use `run-vnc` to see the kiosk: `run` boots with `nographic`, so there is no
display to render the browser on.

## Targets

Every task acts on one machine, `$TESSARO_MACHINE`, which defaults to
`qemux86-64`:

```sh
mise run build-qemu   # qemux86-64      - development, boots under QEMU
mise run build-x86    # genericx86-64   - x86_64 PCs and mini PCs, UEFI
mise run build-rpi    # raspberrypi3-64 - Raspberry Pi 3 Model B+

TESSARO_MACHINE=raspberrypi3-64 mise run shell   # any task, any target
```

Each target gets its own build directory, `build/<machine>/`, and images land
in `build/<machine>/tmp/deploy/images/<machine>/`. The download and sstate
caches in `cache/` are shared, so the second target reuses most of the first
one's work.

Writing an image to a card or a disk is left to you on purpose:

```sh
TESSARO_MACHINE=raspberrypi3-64 mise run unpack
sudo dd if=build/raspberrypi3-64/tmp/deploy/images/raspberrypi3-64/moonforge-image-base-raspberrypi3-64.rootfs.wic \
    of=/dev/sdX bs=4M status=progress conv=fsync
```

## Status

| Platform | Machine | State |
| --- | --- | --- |
| QEMU x86_64 | `qemux86-64` | builds and boots, used for development |
| x86_64 hardware | `genericx86-64` | configured, first build still to be run |
| Raspberry Pi 3B+ | `raspberrypi3-64` | configured, first build still to be run |

All three build the same image: read-only rootfs, overlayfs `/etc`, persistent
`/data`, Weston and the Chromium kiosk.

## Kiosk browser

Images boot straight into a fullscreen Chromium on Weston, showing
`https://www.freevision.sk`. The build-time default is `TESSARO_KIOSK_URL` in
`meta-tessaro-distro/conf/distro/tessaro.conf`.

On a running device, change the URL without rebuilding by uncommenting and
editing `KIOSK_URL` in `/etc/default/tessaro-kiosk`:

```sh
vi /etc/default/tessaro-kiosk
systemctl restart tessaro-kiosk tessaro-kiosk-watchdog
```

`/etc` is an overlayfs whose upper layer is the persistent `/data` partition,
so the change survives a reboot.

A watchdog keeps the page honest: it watches the browser over CDP (the
DevTools protocol, on `127.0.0.1:9222`), probes the URL, re-opens it every ten
minutes, shows a local offline page while the site is unreachable, and restarts
the browser over systemd's D-Bus API if it stops responding. The watchdog is a
Ruby 4 container, built by `mise run watchdog-image` and run by podman with
storage on `/data`. Replace the offline page by dropping a file at
`/data/kiosk/offline.html`. Why it failed is in the journal, not on the screen:

```sh
journalctl -fu tessaro-kiosk-watchdog        # the browser and the watchdog unit
journalctl CONTAINER_NAME=tessaro-kiosk-watchdog   # the container's own output
```

## Developing the watchdog

The watchdog lives in `watchdog/`, as a plain Ruby project with minitest tests,
and runs in Docker locally - no Ruby on the host needed:

```sh
mise run watchdog-test         # unit tests in the Ruby 4 container
mise run watchdog-integration  # real Chromium + the watchdog in compose
```

## Structure

`kas/common/tessaro.yml` pins every upstream repo and selects the layers shared
by all targets, `kas/machine/<machine>.yml` adds what is board-specific, and
`meta-tessaro-distro` holds everything specific to this product. Adding a target
is one new file in `kas/machine/`. See [CLAUDE.md](CLAUDE.md) for the
full layout, the task list and the gotchas worth knowing before your first build.
