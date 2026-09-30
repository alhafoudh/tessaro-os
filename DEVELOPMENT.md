# Developing Tessaro

How to build the images, run them in QEMU, get them onto devices from a
workstation, and test the agent. How each subsystem works is in
[docs/](docs/); the project's rules for changes are in [CLAUDE.md](CLAUDE.md).

## Prerequisites

- Linux x86_64 with Docker, for the builds. [kas](https://kas.readthedocs.io/)
  provides `kas-container`, which runs every build in its own container.
- [mise](https://mise.jdx.dev/): `mise trust && mise install` pins Rust (the
  version Chromium's layers dictate) and Node (the one bitbake builds
  Webconfig with), and installs `bmaptool`.
- `zstd` on the build host and the workstation: images ship as `.wic.zst`,
  and `bmaptool` and `qemu:unpack` run it to decompress them. macOS does not
  have it (`brew install zstd`).
- KVM on the build host for QEMU at a usable speed, and a GPU with a render
  node at `/dev/dri` for the kiosk to paint under QEMU - see the gotchas in
  [docs/build.md](docs/build.md).
- A lot of disk: the shared download and sstate cache is around 40 GB.

## Building

```sh
mise run image:build        # the image for $TESSARO_MACHINE (plus OVMF on qemu)
mise run image:build:qemu   # qemux86-64      - development, boots under QEMU
mise run image:build:x86    # genericx86-64   - x86_64 PCs and mini PCs, UEFI
mise run image:build:rpi3   # raspberrypi3-64 - Pi 3B / 3B+, SD or USB storage
mise run image:build:rpi5   # raspberrypi5    - Pi 5, SD / USB / NVMe storage

TESSARO_MACHINE=raspberrypi3-64 mise run image:shell   # any task, any target
mise run image:sizes                                   # every built image, side by side
mise run image:name                                    # what the next build will be called
mise run image:clean                                   # drop build output, keep the caches
```

Every task acts on one machine, `$TESSARO_MACHINE`, which defaults to
`qemux86-64`. Each target gets its own build directory, `build/<machine>/`,
and images land in `build/<machine>/tmp/deploy/images/<machine>/`. The
download and sstate caches in `cache/` are shared, so the second target reuses
most of the first one's work.

**Pi images carry the board version in their filenames**, so Pi 3 and Pi 5
artifacts stay distinct:

| Build task | Versioned image | Stable image symlink |
| --- | --- | --- |
| `image:build:rpi3` | `tessaro-os-raspberrypi3-64-<version>-<sha>[-dirty].wic.zst` | `tessaro-os-raspberrypi3-64.rootfs.wic.zst` |
| `image:build:rpi5` | `tessaro-os-raspberrypi5-<version>-<sha>[-dirty].wic.zst` | `tessaro-os-raspberrypi5.rootfs.wic.zst` |

The `.wic.bmap` accompanies each image. A build wrapper selects the machine
for that invocation only; select it again for naming, pulling or flashing:

```sh
TESSARO_MACHINE=raspberrypi5 mise run image:name
TESSARO_MACHINE=raspberrypi5 mise run image:pull
TESSARO_MACHINE=raspberrypi5 mise run image:flash /dev/sdX
```

Each Pi image supports its board's boot media without rebuilding for the
storage device. Pi 3 NVMe drives need a USB enclosure. Firmware prerequisites
and migration from U-Boot are in [Pi storage boot](docs/build.md#pi-storage-boot).
Hardware validation is tracked in [platform status](docs/build.md#status).

For work on one recipe, `mise run image:shell` opens a kas shell where `bitbake`
sees the right environment (`bitbake -e <recipe>`, `bitbake -c devshell
<recipe>`, `bitbake -n moonforge-image-base`).

## Running in QEMU

```sh
mise run qemu:vnc   # boot it, framebuffer on localhost:5901, VNC password "tessaro"
mise run qemu:run   # serial console only (Ctrl-a x to exit)
```

Use `qemu:vnc` to see the kiosk: `qemu:run` boots with `nographic`, so there is no
display for the browser. Both boot with `-snapshot`, so nothing a session
changes survives it. The VNC password needs QEMU built with nettle; see
[docs/build.md](docs/build.md), "Gotchas".

The VM has no camera of its own. To give it one, with the VM up:

```sh
mise run usbcam:run -- --attach            # a moving test pattern
mise run usbcam:run -- clip.mp4 --attach   # a clip, looped
```

It needs ffmpeg on this host, and runs until Ctrl-C, which unplugs the
camera again. How it works is in [docs/camera.md](docs/camera.md), "Testing
in qemu".

## From a workstation

The build host is usually a remote x86 machine; the device is on your
network, or its card or disk is plugged into your workstation (macOS or
Linux). These tasks run on the workstation, from its own checkout. Put the
settings in `mise.local.toml` (gitignored):

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

Then `mise trust && mise install`.

**Updating a running device** is the normal way, over the network, keeping
its settings and claim:

```sh
mise run image:update                    # the device in $TESSARO_NODE
mise run image:update brave-otter-3fa2   # or name it
mise run image:update --wipe-data NAME   # also start /data over; it comes back unclaimed
mise run image:update --repartition NAME # the whole disk, for a device on an older layout
```

It pulls the image, builds `tessaro-ctl` from this checkout, and runs
`tessaro-ctl --node NAME update send` with it, which shows the upload, the
device preparing it and the reboot. A dropped connection is resumed by running
it again. The device has to be claimed from this workstation and run an image
with the update layout - see [docs/updates.md](docs/updates.md), which also
covers what `--repartition` risks.

**Flashing** is the manual path: a first install, a device that no longer
boots, or one on a layout updates cannot handle:

```sh
TESSARO_MACHINE=raspberrypi3-64 mise run image:pull    # .wic.zst + .wic.bmap into the repo root
diskutil list                                          # or lsblk - check the device twice
TESSARO_MACHINE=raspberrypi3-64 mise run image:flash   # or: mise run image:flash /dev/disk4
```

`image:pull` rsyncs the machine's `.rootfs.wic.zst` and `.wic.bmap` from the
build host and skips what is already up to date; pass a remote path in single
quotes to pull something else. `mise run image:list` shows every `.wic.zst`
on the build host, all machines at once, and marks the one `image:pull`
fetches by default. `image:flash` unmounts the device, writes it
with bmaptool (through `/dev/rdiskN` on macOS), syncs and ejects it.

`mise run dev:tunnel` keeps an autossh tunnel to the build host up (needs
`autossh`): `localhost:5901` is the build host's QEMU VNC from `qemu:vnc`,
`localhost:5902` is VNC on port 5901 of `$TESSARO_BUILD_HOST_CONTAINER_IP`,
`localhost:7401` and `localhost:2222` are that VM's API port and SSH
(`tessaro-ctl -n 127.0.0.1:7401 device status`,
`tessaro-ctl -n 127.0.0.1:7401 ssh connect --port 2222`), and on the build
host `localhost:7400` and `localhost:5022` reach the API and SSH on
`$TESSARO_DEVICE_IP`. `https://localhost:7401/api/docs/` is the VM's
Swagger UI ([docs/api.md](docs/api.md)).

## The agent

The agent, the client and the updater are a Rust workspace in `agent/`,
tested on the host:

```sh
mise run agent:test          # cargo test for the workspace
mise run agent:lint          # cargo fmt --check plus clippy
mise run agent:integration   # the agent against a real headless Chromium in docker compose
mise run ctl:build           # a release tessaro-ctl for this machine
mise run ctl:run -- nodes list   # that build, rebuilt first if stale, arguments after --
```

After changing any `Cargo.toml` or `agent/Cargo.lock`, regenerate the crate
list the recipe builds from, or the image build fails:

```sh
mise run image:shell                    # then, inside:
bitbake -c update_crates tessaro-kiosk  # writes tessaro-kiosk-crates.inc
```

## The desktop client

`tessaro-gui` is the desktop client for technicians: devices found on the
network and the known ones from `~/.config/tessaro/tessaro.db`, an inner window per device
with its settings, a page for every `tessaro-ctl` command group, the live
journal and a live VNC view. It is its own workspace in `gui/`, built on the
workstation and never part of the image. It uses `agent/protocol` and
`agent/client` by path, so it speaks what `tessaro-ctl` speaks.

```sh
mise run gui:run             # run it from this checkout
mise run gui:test            # its unit tests
mise run gui:lint            # cargo fmt --check plus clippy
mise run gui:build           # a release build for this machine
```

On macOS, the release build also creates
`build/gui-target/release/Tessaro.app`, with its Dock and Finder icon.

How it works is in [docs/gui.md](docs/gui.md).

## Webconfig

Webconfig is the device's management pages in a browser, a React app in
`webconfig/` that the image builds (`tessaro-webconfig`) and the agent
serves at `/` on port 7400. Its API types are generated from
`agent/protocol/openapi.json`.

```sh
mise run webconfig:setup     # npm ci
mise run webconfig:run       # dev server on http://localhost:5173, API proxied to the qemu forward
mise run webconfig:test      # unit tests, the golden fixtures, bitbake-lock.json current
mise run webconfig:lint      # tsc, ESLint, Prettier
mise run webconfig:build     # into build/webconfig, which agent:integration serves
mise run webconfig:lock      # after changing a dependency
```

`TESSARO_WEBCONFIG_TARGET=https://ADDRESS:7400 mise run webconfig:run`
points the dev server at a real device. How it works is in
[docs/webconfig.md](docs/webconfig.md).

## End-to-end tests

`mise run e2e:run` boots the qemux86-64 image and provokes what the agent
exists to handle - the site going down, the browser crashing or wedging, the
agent itself wedging, settings, claiming, network changes, image updates -
asserting on what the agent writes to its journal. It is an RSpec suite in
`test/e2e/spec/`: each spec file is a lane, whose cases run in order on a VM
of its own, and the lanes run in parallel.

It tests the image as built and never builds one, so build first. It needs
Ruby with Bundler on the host, and KVM to be quick; each VM takes 4 GB of RAM.

```sh
mise run image:build        # the image under test (qemux86-64)
mise run e2e:setup          # once: installs rspec and parallel_tests
mise run e2e:run            # every lane, three VMs at a time
```

Running part of it, and watching it:

```sh
E2E_JOBS=1 mise run e2e:run                             # one VM at a time
mise run e2e:run -- -o '--tag ~reboot'                  # leave out the image updates
mise run e2e:one -- spec/network_spec.rb                # one lane, plain rspec
mise run e2e:one -- spec/agent_spec.rb -e 'dns:'        # one case, by its name
mise run e2e:one -- --only-failures                     # what failed last time
E2E_VERBOSE=1 mise run e2e:one -- spec/agent_spec.rb    # each step as it happens
```

`e2e:one` paths are relative to `test/e2e`. `E2E_VERBOSE=2` adds every
agent journal line a case sees, `E2E_KEEP=1` leaves the VM up after its lane,
and `E2E_REUSE=1` runs against a VM left up that way. Everything a run leaves
is in `build/e2e/`. Lanes, ports and the harness are explained in
[docs/e2e.md](docs/e2e.md).

## Repository layout

| Path | What it is |
| --- | --- |
| `kas/common/tessaro.yml` | pins every upstream layer and selects the ones every target shares |
| `kas/machine/<machine>.yml` | what is board-specific; a new target is one new file here |
| `meta-tessaro-distro/` | the product layer: distro config, image additions, recipes, wks files |
| `agent/` | `tessaro-agent`, `tessaro-ctl`, the API's types, the client library and the updater |
| `gui/` | `tessaro-gui`, the desktop client (a workspace of its own) |
| `webconfig/` | Webconfig, the management pages the device serves to a browser |
| `test/e2e/` | the end-to-end suite |
| `docs/` | how each subsystem works |
| `mise.toml` | every task |

This repository is the kas root; every other layer is a pinned checkout kas
makes on the first build (gitignored). See [docs/build.md](docs/build.md).
