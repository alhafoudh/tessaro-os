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
mise run image:build        # the image for $TESSARO_MACHINE (plus OVMF on x86)
mise run image:build:x86    # genericx86-64   - x86_64 PCs and mini PCs, UEFI; boots under QEMU, e2e runs on it
mise run image:build:arm64  # genericarm64    - Arm64 UEFI, boots under QEMU here or on a Mac
mise run image:build:rpi3   # raspberrypi3-64 - Pi 3B / 3B+, SD or USB storage
mise run image:build:rpi4   # raspberrypi4-64 - Pi 4B / 400 / CM4, SD or USB storage
mise run image:build:rpi5   # raspberrypi5    - Pi 5, SD / USB / NVMe storage

TESSARO_MACHINE=raspberrypi3-64 mise run image:shell   # any task, any target
mise run image:sizes                                   # every built image, side by side
mise run image:name                                    # what the next build will be called
mise run image:clean                                   # drop build output, keep the caches
```

Every task acts on one machine, `$TESSARO_MACHINE`, which defaults to
`genericx86-64`: the image x86 PCs run, and the one `qemu:*` and the e2e
suite boot in QEMU. Each target gets its own build directory, `build/<machine>/`,
and images land in `build/<machine>/tmp/deploy/images/<machine>/`. The
download and sstate caches in `cache/` are shared, so the second target reuses
most of the first one's work. On the build host `cache/` is a symlink to
`/srv/tessaro/cache`, which the CI runner uses too ([docs/ci.md](docs/ci.md)).

**Pi images carry the board version in their filenames**, so each board's
artifacts stay distinct:

| Build task | Versioned image | Stable image symlink |
| --- | --- | --- |
| `image:build:rpi3` | `tessaro-os-raspberrypi3-64-<version>-<sha>[-dirty].wic.zst` | `tessaro-os-raspberrypi3-64.rootfs.wic.zst` |
| `image:build:rpi4` | `tessaro-os-raspberrypi4-64-<version>-<sha>[-dirty].wic.zst` | `tessaro-os-raspberrypi4-64.rootfs.wic.zst` |
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

A barcode scanner the same way, in the mode to try:

```sh
mise run usbscanner:run -- --mode keyboard --attach          # hidpos, serial
echo 'HELLO-123' | nc 127.0.0.1 3242                         # a scan
mise run usbscanner:run -- --mode serial --scan 5901234123457 --every 5 --attach
```

How it works is in [docs/scanners.md](docs/scanners.md), "Testing in qemu".

### genericarm64, on the build host or a Mac

```sh
mise run qemu:run:arm64              # the build host: TCG, slow; a Mac: HVF, a window
mise run qemu:vnc:arm64              # VNC on localhost:5901, as qemu:vnc
mise run qemu:run:arm64 IMAGE.wic.zst   # a Mac: a downloaded release image
mise run qemu:run:arm64 --no-vmnet   # a Mac: slirp and the 127.0.0.1 forwards, no sudo
```

On a Mac it boots the image `TESSARO_MACHINE=genericarm64 mise run
image:pull` fetched, with Homebrew's `qemu` and its edk2 firmware, under
Hypervisor.framework. The guest is on macOS's shared vmnet network, so
`tessaro-ctl nodes list` finds it by mDNS; vmnet needs root, so QEMU runs
under sudo. Everything is in `scripts/qemu-arm64.sh`.

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

## Try Tessaro

Try Tessaro, in the same workspace (`gui/try-tessaro`), is the Mac app that
runs a device in a VM for someone trying Tessaro. It bundles a QEMU built
with VirGL from pinned sources (`gui/try-tessaro/build-qemu-gpu.sh`, once,
into `build/qemu-gpu/`), so a Mac building it needs `brew install glib
pixman libslirp dtc pkgconf`, and a genericarm64 image, the one
`image:pull` fetched unless another is named. `qemu:run:arm64` uses that
QEMU too once it is built.

```sh
mise run try:run                      # package and open it, from this checkout
mise run try:run IMAGE.wic.zst        # with another genericarm64 image
mise run try:build                    # release app and build/Try-Tessaro-macos-arm64.dmg
```

Its device lives in `~/Library/Application Support/Try Tessaro/`; delete
that folder for a first launch from scratch. How it works is in
[docs/try-tessaro.md](docs/try-tessaro.md).

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

## The demo

The demo shows every feature of the device on its own screen, a React app
in `demo/` that reaches the device only through the page bridge.

```sh
mise run demo:setup     # npm ci
mise run demo:run       # dev server on http://localhost:5174, with a pretend device
mise run demo:test      # unit tests, bitbake-lock.json current
mise run demo:lint      # tsc, ESLint, Prettier
mise run demo:build     # into build/demo
mise run demo:lock      # after changing a dependency
```

To work on it against a real device, build it into the device's file store
and put it on the screen:

```sh
mise run ctl:run -- -n ADDRESS config set agent.refresh_interval=0 browser.bridge.mode=actions
TESSARO_NODE=ADDRESS mise run demo:sync
```

`demo:sync` opens `http://127.0.0.1/files/demo/`. Put
`agent.refresh_interval` back with `config unset` when done. How it works
is in [docs/demo.md](docs/demo.md).

## The player page

The player page plays playlists on the device: static files in
`meta-tessaro-distro/recipes-browser/tessaro-selftest/files/player/`, plain
ES modules with no build step, served by nginx at
`http://127.0.0.1/player.html`. Its decisions are in `player-core.js`, which
has no DOM and is unit-tested.

```sh
mise run player:test   # player-core.js, with node --test
mise run player:run    # the page and dev/sample.json on http://127.0.0.1:8090
```

`player:run` prints the URL to open. The page reads the playlist from
`?src=` and looks for changes every 5 seconds, so editing
`dev/sample.json` shows on the open page. The events the agent would get
go to the browser console.

## End-to-end tests

`mise run e2e:run` boots the genericx86-64 image in QEMU and provokes what the agent
exists to handle - the site going down, the browser crashing or wedging, the
agent itself wedging, settings, claiming, network changes, image updates -
asserting on what the agent writes to its journal. It is an RSpec suite in
`test/e2e/spec/`: each spec file is a lane, whose cases run in order on a VM
of its own, and the lanes run in parallel.

It tests the image as built and never builds one, so build first. It needs
Ruby with Bundler on the host, and KVM to be quick; each VM takes 4 GB of RAM.

```sh
mise run image:build        # the image under test (genericx86-64)
mise run e2e:setup          # once: installs rspec and parallel_tests
mise run e2e:run            # every lane, three VMs at a time
```

Running part of it, and watching it:

```sh
E2E_JOBS=1 mise run e2e:run                             # one VM at a time
mise run e2e:run -- -o '--tag ~reboot'                  # leave out the image updates
mise run e2e:one -- spec/network_spec.rb                # one lane, plain rspec
mise run e2e:one -- spec/agent_browser_spec.rb -e 'dns:'  # one case, by its name
mise run e2e:one -- --only-failures                       # what failed last time
E2E_VERBOSE=1 mise run e2e:one -- spec/agent_browser_spec.rb  # each step as it happens
```

`e2e:one` paths are relative to `test/e2e`. `E2E_VERBOSE=2` adds every
agent journal line a case sees, `E2E_KEEP=1` leaves the VM up after its lane,
and `E2E_REUSE=1` runs against a VM left up that way. Everything a run leaves
is in `build/e2e/`. Lanes, ports and the harness are explained in
[docs/e2e.md](docs/e2e.md).

## CI

GitHub Actions lints and tests the agent, the desktop client and Webconfig,
and checks their dependencies' licenses, on every push (`.github/workflows/ci.yml`). Releases are started by hand from
the Actions tab (`release.yml`): the images on a self-hosted runner on the
build host, with e2e on the genericx86-64 image, and `tessaro-ctl` and `tessaro-gui`
on GitHub's runners. To test a branch before merging it, a pull request's
included, start `e2e.yml` from the Actions tab on that branch: it builds
the genericx86-64 image and runs e2e on it, nothing else. Pull requests do not run
it on their own. The runner's setup is in [docs/ci.md](docs/ci.md).

## SBOM and licenses

Every release carries each image's bill of materials and a flat license
list. To write them for a built image, or to check the licenses of the
crates, npm packages and vendored files without one:

```sh
mise run sbom:build           # build/sbom/<machine>/<image>.sbom.tar.zst and .licenses.csv
mise run sbom:check           # the license policy in sbom/licenses.yml, also on every push
mise run sources:collect      # the image's GPL/LGPL/AGPL sources into /srv/tessaro/sources
mise run sbom:test            # the tool's unit tests
```

What each part is read from and how the policy works is in
[docs/sbom.md](docs/sbom.md).

## README screenshots

**The README's screenshots are rendered, never taken from a device**, so they
always show the same made-up device and follow the pages as they ship:

```sh
mise run docs:screenshots     # writes docs/images/*.jpg
```

The task first writes the fixtures with the agent's own code, through the
`*_screenshot_fixture*` tests (which also fail `agent:test` once a fixture is
out of date): `docs/screenshots/welcome.json` from `welcome_value` and
`qr.rs`, `docs/screenshots/debug.html` from `debug::page` and the image's
default `KIOSK_DEBUG_TEMPLATE`, and `docs/screenshots/api/`, the API's
answers by path. Those come from a control plane in a sandbox on the image's
defaults (`screenshots.rs`), with what a sandbox cannot know - hardware, the
browser, units, the read-only keys - filled in for one made-up device, which
the debug screen shows too.

It then builds Webconfig and draws `tessaro-gui` headless: the ignored
`screenshot::the_readme_screenshots` test boots the app on an empty config
directory, opens a device window and feeds it what a worker would send, from
those fixtures, and renders it with iced's tiny-skia renderer (no GPU, the
same pixels everywhere) into `build/screenshots/`. A page call without a
fixture fails the test by its tag; answer it in `call_answers`.

Last, `docs/screenshots/shoot.mjs` loads the shipped pages in Playwright's
Docker image and answers their requests with the fixtures: the kiosk's pages
at 1920x1080, Webconfig at 1280x800 from `build/webconfig` with `/api`
answered from `api/` (a request without a fixture is named and fails the
run), and the GUI's PNGs turned into JPEGs. Every image is saved 1600 wide.
Rerun it after changing any page it shows, the QR code or the default
template, and commit the fixtures and the images together. It needs Docker.

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
| `sbom/` | the SBOM tool, its license policy and the list of vendored files |
| `docs/` | how each subsystem works; `docs/screenshots/` renders the README's screenshot |
| `mise.toml` | every task |

This repository is the kas root; every other layer is a pinned checkout kas
makes on the first build (gitignored). See [docs/build.md](docs/build.md).
