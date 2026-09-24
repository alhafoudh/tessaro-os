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

## Updating and flashing from a workstation

The build host is a remote x86 machine; the device is on your network, or its
card or disk is plugged into your workstation (macOS or Linux). These tasks run
on the workstation, from its own checkout of this repo. Put the settings in
`mise.local.toml` (gitignored):

```toml
# Optional: default machine on this workstation (a [vars] override, because
# WIC and KAS_CONFIG are derived from it). TESSARO_MACHINE=... still wins.
[vars]
machine = "{{ env.TESSARO_MACHINE | default(value='raspberrypi3-64') }}"

[env]
TESSARO_BUILD_HOST = "build-host.example.com"          # SSH host that builds
TESSARO_BUILD_HOST_DIR = "Projects/tessaro/tessaro-os" # this repo there, relative to your home
TESSARO_NODE = "brave-otter-3fa2"                      # default device for update (tessaro-ctl --node)
TESSARO_FLASH_DEVICE = "/dev/disk8"                    # default target for flash
TESSARO_BUILD_HOST_CONTAINER_IP = "172.17.0.9"         # tunnel: container with VNC on 5901
TESSARO_DEVICE_IP = "192.168.69.123"                   # tunnel: device on your network
```

Then `mise trust && mise install` (that installs `bmaptool`).

**Updating a running device** is the normal way, over the network, keeping its
settings and claim:

```sh
mise run image:update                    # the device in $TESSARO_NODE
mise run image:update brave-otter-3fa2   # or name it
mise run image:update --wipe-data NAME   # also start /data over; it comes back unclaimed
mise run image:update --repartition NAME # the whole disk, for a device on an older layout
```

`--repartition` is `image:flash` over the network: partition table, boot,
root and an empty `/data`, written from a copy of the upload in the device's
RAM. It comes back unclaimed, and a power cut while it writes needs a
physical reflash. It needs the device to run an image that knows the flag.

It pulls the image first, builds `tessaro-ctl` from this checkout, and runs
`tessaro-ctl --node NAME update send` with it, which shows the upload, the
device preparing it and the reboot. A dropped connection is resumed by running
it again. The device has to be claimed from this workstation
(`tessaro-ctl --node NAME access claim`), and running an image with the update layout -
see [docs/updates.md](docs/updates.md).

**Flashing** is the manual path: a first install, a device that no longer
boots, or one flashed before the update layout:

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
`localhost:7400` and `localhost:2222` are that VM's tessaro-ctl port and SSH
(`tessaro-ctl -n 127.0.0.1 device status`,
`tessaro-ctl -n 127.0.0.1 ssh connect --port 2222`), and on the build host `localhost:5022` reaches SSH on `$TESSARO_DEVICE_IP`.

## Status

| Platform | Machine | State |
| --- | --- | --- |
| QEMU x86_64 | `qemux86-64` | builds and boots, used for development |
| x86_64 hardware | `genericx86-64` | configured, first build still to be run |
| Raspberry Pi 3B+ | `raspberrypi3-64` | builds, boots and runs the kiosk |

Every machine builds the same image: read-only rootfs, overlayfs `/etc`,
persistent `/data`, Weston and the Chromium kiosk.

## Kiosk browser

Images boot straight into a fullscreen Chromium on Weston. A factory image
shows the self-test page described below, served locally on
`http://127.0.0.1/`; a deployment shows the site it is there to show. The
build-time default is `TESSARO_KIOSK_URL` in
`meta-tessaro-distro/conf/distro/tessaro.conf`.

On a running device, change the URL without rebuilding with `tessaro-ctl`,
which restarts whatever reads the setting:

```sh
tessaro-ctl config set browser.url=https://example.com/
```

Settings live in `/data/tessaro/state.json` on the persistent `/data`
partition, so the change survives a reboot and image updates.

A site can keep its media on the device, so it plays with the network down.
Files go into `/data/files`, which the device serves at
`http://127.0.0.1/files/` to any origin:

```sh
tessaro-ctl files sync ./site-assets    # mirror a directory: send what changed, remove the rest
tessaro-ctl files upload promo.mp4 /media/
tessaro-ctl files list /media           # like ls -l (alias ls, dir); -R for the whole tree
tessaro-ctl files move /media/promo.mp4 /archive/   # like mv (alias mv)
```

`sync` compares size and modification time only, like rsync without
`--checksum`, and asks before it removes anything. A factory reset empties
the store.

## Checking a device

A factory image boots into a static self-test page, served by nginx on the
loopback from `/usr/share/tessaro-selftest/`. It checks rendering, fonts,
emoji, every HTML input type, touch and mouse scrolling, WebSerial and WebHID,
audio and video playback, and WebAudio synthesis - all from local files, so it
works with the network down.

Deploying a device means pointing `browser.url` at the site it is there to
show. To get back to the self-test page afterwards:

```sh
tessaro-ctl config set browser.url=http://127.0.0.1/ agent.refresh_interval=0
```

`agent.refresh_interval=0` matters when working through it by hand - otherwise
the agent reloads the page every ten minutes, closing any serial port it has
open and wiping every field you have typed into. `tessaro-ctl config unset`
both afterwards.

It is served over http rather than opened as a file on purpose: a `file://`
page has no origin, and Chromium's serial and HID policy grants match on
origin, so the page could only reach a device through a chooser dialog.
`http://127.0.0.1` is a real origin and a secure context, so the pre-grants
apply. The on-screen keyboard only appears on a device with no hardware
keyboard attached; `tessaro-ctl config set screen.osk=always` forces it.
Everything else works with a finger.

An agent keeps the page honest: it watches the browser over CDP (the
DevTools protocol, on `127.0.0.1:9222`), probes the URL, re-opens it every ten
minutes, shows a local offline page while the site is unreachable, and restarts
the browser over systemd's D-Bus API if it stops responding. It is a native
Rust binary, `tessaro-agent`, and also the device's control plane for
`tessaro-ctl`. Replace the offline page by dropping a file at
`/data/kiosk/offline.html`. Why it failed is in the journal, not on the screen:

```sh
journalctl -fu tessaro-agent    # the agent
journalctl -fu tessaro-kiosk    # the browser
```

## Developing the agent

The agent is a Rust workspace in `agent/`, tested on the host:

```sh
mise run agent-test          # cargo test for the workspace
mise run agent-lint          # cargo fmt --check plus clippy
mise run agent-integration   # the agent against a real headless Chromium in compose
mise run agent-e2e           # boot the qemu image and exercise the agent on it
```

### End-to-end tests

`mise run agent-e2e` boots the qemux86-64 image and provokes what the agent
exists to handle - the site going down, the browser crashing or wedging, the
agent itself wedging, settings, claiming, network changes, image updates -
asserting on what the agent writes to its journal. It is an RSpec suite in
`test/e2e/spec/`: each spec file is a lane, whose cases run in order on a VM
of its own, and the lanes run in parallel.

It tests the image as built and never builds one, so build first. It needs
Ruby with Bundler on the host, and KVM to be quick; each VM takes 4 GB of RAM.

```sh
mise run build              # the image under test (qemux86-64)
mise run agent-e2e:setup    # once: installs rspec and parallel_tests
mise run agent-e2e          # every lane, three VMs at a time
```

The suite warns when the image is older than the agent's sources, since an
old agent passes and proves nothing.

Running part of it, and watching it:

```sh
E2E_JOBS=1 mise run agent-e2e                                 # one VM at a time
mise run agent-e2e -- -o '--tag ~reboot'                      # leave out the image updates
mise run agent-e2e:one -- spec/network_spec.rb                # one lane, plain rspec
mise run agent-e2e:one -- spec/agent_spec.rb -e 'dns:'        # one case, by its name
mise run agent-e2e:one -- --only-failures                     # what failed last time
E2E_VERBOSE=1 mise run agent-e2e:one -- spec/agent_spec.rb    # each step as it happens
```

`agent-e2e:one` paths are relative to `test/e2e`. `E2E_VERBOSE=2` adds every
agent journal line a case sees, `E2E_KEEP=1` leaves the VM up after its lane,
and `E2E_REUSE=1` runs against a VM left up that way. Output is live, with an
overall `== progress 12/30, 6:03 elapsed, ~9 min left` line after each case.
A failure prints the agent's journal lines since the case began.

Everything a run leaves is in `build/e2e/`: `<lane>.log` holds every step of a
lane whatever the verbosity, `<lane>.qemu.log` its VM's console. Exit status 1
means a case failed, 2 that the suite could not start (no image). How the
lanes, ports and VMs fit together is in [docs/e2e.md](docs/e2e.md).

## Structure

`kas/common/tessaro.yml` pins every upstream repo and selects the layers shared
by all targets, `kas/machine/<machine>.yml` adds what is board-specific, and
`meta-tessaro-distro` holds everything specific to this product. Adding a target
is one new file in `kas/machine/`. See [docs/build.md](docs/build.md) for the
full layout and the gotchas worth knowing before your first build,
[CLAUDE.md](CLAUDE.md) for the task list and the project's rules, and `docs/`
for how each subsystem works.
