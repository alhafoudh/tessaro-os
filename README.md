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

## Flashing from a workstation

The build host is a remote x86 machine; the card or disk is plugged into your
workstation (macOS or Linux). Three tasks run on the workstation, from its own
checkout of this repo. Put the settings in `mise.local.toml` (gitignored):

```toml
# Optional: default machine on this workstation (a [vars] override, because
# WIC and KAS_CONFIG are derived from it). TESSARO_MACHINE=... still wins.
[vars]
machine = "{{ env.TESSARO_MACHINE | default(value='raspberrypi3-64') }}"

[env]
TESSARO_BUILD_HOST = "build-host.example.com"          # SSH host that builds
TESSARO_BUILD_HOST_DIR = "Projects/tessaro/tessaro-os" # this repo there, relative to your home
TESSARO_FLASH_DEVICE = "/dev/disk8"                    # default target for flash
TESSARO_BUILD_HOST_CONTAINER_IP = "172.17.0.9"         # tunnel: container with VNC on 5901
TESSARO_DEVICE_IP = "192.168.69.123"                   # tunnel: device on your network
```

Then `mise trust && mise install` (that installs `bmaptool`) and:

```sh
TESSARO_MACHINE=raspberrypi3-64 mise run image:pull    # .wic.bz2 + .wic.bmap into the repo root
diskutil list                                          # or lsblk - check the device twice
TESSARO_MACHINE=raspberrypi3-64 mise run image:flash   # or: mise run image:flash /dev/disk4
```

`image:pull` rsyncs `build/<machine>/tmp/deploy/images/<machine>/tessaro-os-<machine>.rootfs.wic.*`
from the build host and skips what is already up to date. Pass a remote path
in single quotes to pull something else. `image:flash` unmounts the device, writes
it with bmaptool (through `/dev/rdiskN` on macOS), syncs and ejects it. Every
build produces a `.bmap`; for an older image without one the whole image is
written.

`mise run tunnel` keeps an autossh tunnel to the build host up (needs
`autossh`): `localhost:5901` is the build host's QEMU VNC from `run-vnc`,
`localhost:5902` is VNC on port 5901 of `$TESSARO_BUILD_HOST_CONTAINER_IP`,
and on the build host `localhost:5022` reaches SSH on `$TESSARO_DEVICE_IP`.

## Status

| Platform | Machine | State |
| --- | --- | --- |
| QEMU x86_64 | `qemux86-64` | builds and boots, used for development |
| x86_64 hardware | `genericx86-64` | configured, first build still to be run |
| Raspberry Pi 3B+ | `raspberrypi3-64` | configured, first build still to be run |

All three build the same image: read-only rootfs, overlayfs `/etc`, persistent
`/data`, Weston and the Chromium kiosk.

## Kiosk browser

Images boot straight into a fullscreen Chromium on Weston. A factory image
shows the self-test page described below, served locally on
`http://127.0.0.1/`; a deployment shows the site it is there to show. The
build-time default is `TESSARO_KIOSK_URL` in
`meta-tessaro-distro/conf/distro/tessaro.conf`.

On a running device, change the URL without rebuilding by uncommenting and
editing `KIOSK_URL` in `/etc/default/tessaro-kiosk`:

```sh
vi /etc/default/tessaro-kiosk
systemctl restart tessaro-kiosk tessaro-kiosk-watchdog
```

`/etc` is an overlayfs whose upper layer is the persistent `/data` partition,
so the change survives a reboot.

## Checking a device

A factory image boots into a static self-test page, served by nginx on the
loopback from `/usr/share/tessaro-selftest/`. It checks rendering, fonts,
emoji, every HTML input type, touch and mouse scrolling, WebSerial and WebHID,
audio and video playback, and WebAudio synthesis - all from local files, so it
works with the network down.

Deploying a device means pointing `KIOSK_URL` at the site it is there to show.
To get back to the self-test page afterwards:

```sh
vi /etc/default/tessaro-kiosk
#   KIOSK_URL=http://127.0.0.1/
systemctl restart tessaro-kiosk tessaro-agent
```

Set `KIOSK_REFRESH_INTERVAL=0` as well before working through it by hand -
otherwise the supervisor reloads the page every ten minutes, closing any serial
port it has open and wiping every field you have typed into.

It is served over http rather than opened as a file on purpose: a `file://`
page has no origin, and Chromium's serial and HID policy grants match on
origin, so the page could only reach a device through a chooser dialog.
`http://127.0.0.1` is a real origin and a secure context, so the pre-grants
apply. One thing that does still need hardware: this image has no on-screen
keyboard, so the text fields need a USB keyboard. Everything else works with a
finger.

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
