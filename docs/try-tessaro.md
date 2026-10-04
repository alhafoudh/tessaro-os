# Try Tessaro

**Try Tessaro is a Mac app that runs a whole Tessaro device in a VM, for
someone who wants to see Tessaro before they have a screen to put it on.**
One window starts and stops the device, opens `tessaro-gui`, a terminal
with `tessaro-ctl` ready and Webconfig, and offers things to try. The kiosk
itself is QEMU's own window. The app is `gui/try-tessaro`; `mise run
try:run` runs it from a checkout and `mise run try:build` makes the DMG,
which the release workflow's `try` job builds around that run's
genericarm64 image when try is ticked (**By hand: `release.yml`** in
[ci.md](ci.md)).

It is macOS on Apple silicon only. The code keeps the platform's parts
(the QEMU command line, the control socket, the terminal) in a few places so
a Windows build with WHPX and the x86-64 image can follow.

## What the app carries

**Everything it needs is inside `Try Tessaro.app`**, so trying Tessaro needs
no Homebrew, no mise and no checkout:

| Path in `Contents/` | What |
| --- | --- |
| `MacOS/try-tessaro` | The launcher (iced, `gui/try-tessaro/src/`) |
| `Resources/qemu/` | The QEMU runtime, with VirGL (below) |
| `Resources/bin/tessaro-ctl` | For the terminal |
| `Helpers/Tessaro.app` | `tessaro-gui`, as a release ships it |
| `Resources/image/*.wic.zst` | The genericarm64 image the device is made from |

`gui/try-tessaro/package-macos.sh` assembles it and signs it ad hoc, like
`Tessaro.app`. It is not notarized: a downloaded copy needs its quarantine
flag cleared once (README, **Try it on a Mac**).

**The DMG opens on the welcome page's look**: the app on the left, a link
to Applications on the right, big icons and an arrow between them over
`dmg/background.tiff` (`dmg/background.svg` at 1x and 2x, made by
`dmg/generate.sh`). **dmgbuild lays it out** (`dmg/settings.py`), pinned and
installed by `try:build` into `build/dmgbuild-venv`: it writes Finder's
`.DS_Store` itself, so the layout needs no Finder and no AppleScript and
comes out the same on a headless CI runner. It also sizes the image from
the files' full size; `hdiutil create -srcfolder` alone counts blocks on
disk, and the sparse UEFI firmware then runs it out of room. The icon labels
are Finder's own: dark text in light mode, light in dark mode, so on this
dark background they read best in dark mode.

## The QEMU runtime

**The device draws on the Mac's GPU: the app bundles a QEMU built with
VirGL**, because Homebrew's has no OpenGL and no virglrenderer and leaves
the kiosk in software. `build-qemu-gpu.sh` is Try Omarchy's GPU recipe
(github.com/omacom/try-omarchy, `macos/build-qemu-gpu-runtime.sh`) and
nothing else of theirs: QEMU 11.1.1 with their Cocoa OpenGL patch (upstream
`ui/cocoa.m` has no GL at all), their fence-polling and refresh fixes,
virglrenderer 1.3.0 with the `startergo/homebrew-virglrenderer` patch set
and their native OpenGL patch, and startergo's libepoxy and ANGLE bottles.
Every download is pinned by sha256 in the script, the patches are fetched
at a pinned commit rather than kept here, and glib, pixman, libslirp and dtc
are Homebrew's. It builds into `build/qemu-gpu/` and does nothing while
`.stamp` holds the script's hash, so a pin change is what rebuilds it.

**GL is macOS's own OpenGL (`-display cocoa,gl=on`), never ANGLE's GLES on
Metal (`gl=es`).** Through ANGLE, VirGL offers the guest desktop GL 2.1
only; Chromium's GLES 3 context (`--use-angle=gles-egl`) then fails with
`EGL_BAD_ATTRIBUTE` and the kiosk quietly renders in software. On native GL,
with the virglrenderer patches, the guest's Mesa virgl driver offers GLES
3.0 and Chromium composites, rasterizes and runs WebGL on the GPU. Video
decoding stays in software. Try Omarchy measured this
(`docs/performance.md` there). The launcher picks `virtio-gpu-gl-pci` when
the bundled QEMU lists it and **Graphics on this computer's GPU** is on in
the settings (`Settings::accelerated`, on by default); off is the plain
`virtio-gpu-pci` and software, for a Mac whose GL path misbehaves.

**`bundle-qemu-macos.sh` makes that build self-contained.** It copies
`qemu-system-aarch64` and `qemu-img`, walks `otool -L` to every non-system
dylib they load, copies each under the name its load command uses and
rewrites every load command to `@executable_path/../lib/`. ANGLE's
`libEGL.dylib` and `libGLESv2.dylib` are copied by hand: libepoxy opens them
by name, so nothing links them. It then fails if anything still points
outside the bundle or the system. Relinking breaks the signatures, so the
dylibs are signed ad hoc again and QEMU with `qemu-hvf.entitlements`
(`com.apple.security.hypervisor`), which HVF needs and an ad hoc signature
can carry. `share/qemu/` holds only the UEFI firmware: the command line
names it, and `romfile=` keeps QEMU from looking for a network ROM.
`package-macos.sh` runs both and remakes the runtime only when the build's
stamp changed.

**Its licenses are in the bundle and in the SBOM.** The bundle script copies
each Homebrew formula's license files into `qemu/LICENSES/<formula>/`, a
`NOTICE` from `brew info` for a bottle that has none, and the license texts
`build-qemu-gpu.sh` gathered for QEMU, virglrenderer, libepoxy and ANGLE.
`runtime.json` is `brew info --json=v2` of every formula shipped plus the
build's `components.json`, the same shape for those four. `ruby
sbom/sbom.rb check --runtime <runtime.json>` judges them under the
`runtime` policy in `sbom/licenses.yml`; the release workflow's `try` job
runs it where the file exists (see [sbom.md](sbom.md)).

**A release carries the QEMU's source beside the DMG**, because QEMU is GPL
and the bundled one is patched. `qemu-source.sh` packs
`try-tessaro-<version>-<platform>-qemu-source.tar` from what
`build-qemu-gpu.sh` downloaded: the QEMU and virglrenderer release tarballs,
the virglrenderer patch set, Try Omarchy's patches and `build-qemu-gpu.sh`
itself, which says how they go together. The libepoxy and ANGLE bottles are
binaries under MIT and BSD and stay out of it.

## The VM

**The command line is `vm/qemu.rs`, a pure function the tests read**, and
`scripts/qemu-arm64.sh` is the same machine for `qemu:run:arm64`; change
both together. What differs from the script:

* **slirp only, no vmnet.** The QEMU is built with vmnet all the same
  (`--enable-vmnet` in `build-qemu-gpu.sh`), because the script prefers it
  and defaults to vmnet. The API and Webconfig are forwarded on
  `127.0.0.1:7401`, SSH on `127.0.0.1:2222`, so nothing needs root and
  nothing asks for a password. The cost: the device is reachable from this
  computer only, mDNS does not find it, and the welcome page shows the
  guest's own `10.0.2.15`. The launcher shows the address that works.
* **Ports are picked, not fixed** (`vm/ports.rs`): 7401 and 2222 when free,
  else the next free one. The same ones every time they are free, so the
  client store's entry for the device stays valid.
* **The resolution is the GPU's preferred mode**, `virtio-gpu-pci`'s
  `xres`/`yres`, which Weston takes as the display's mode. It is picked
  before Start, with Full HD pre-selected (`settings::default_for`), or HD
  when the screen the launcher's window opened on is measured smaller
  than 1920 x 1080 pixels (iced's monitor size in points times the scale
  factor): QEMU's window shows the device pixel for pixel, so a bigger one
  would not fit. `settings.json` keeps a size only once the user picked
  one. The script always asks for Full HD.
* **QEMU's window is never `zoom-to-fit`.** That option makes the Cocoa
  window resizable, and a resizable window hands its own size to the guest
  as the preferred mode (`virtio_gpu_ui_info` in
  `hw/display/virtio-gpu-base.c`; `xres`/`yres` only set where it starts).
  The window keeps the size the firmware's screen gave it, so the kiosk
  came up at 640 x 360 with Full HD picked. A fixed window is the guest's
  size in the screen's pixels: on a Retina screen, Full HD is a 960 x 540
  point window, sharp but small.
* **Sound is CoreAudio through `intel-hda`**: `hda-output` with the
  microphone off, `hda-duplex` with it on, and no sound card at all with
  sound off. The microphone is off by default, so macOS asks for it only
  when someone turns it on; `NSMicrophoneUsageDescription` in the app's
  `Info.plist` is what macOS shows, since QEMU's input is attributed to the
  app.
* **vCPUs and memory come from the host** (`qemu::size`): half the cores,
  2 to 4, and 4 GB, 3 GB on a computer with 8 GB or less.
* **QMP on a Unix socket** (`vm/qmp.rs`) is how Stop works: the power
  button (`system_powerdown`), so the device shuts down cleanly, then
  `quit` after 30 s, then a kill 10 s later. Quitting the app while the
  device runs asks first and shuts it down the same way.

## The disk

**The image is unpacked once into a base that is never written, and the
device's disk is a qcow2 overlay on it** (`vm/disk.rs`). Every write goes to
the overlay, so Reset to factory is deleting it: instant, and nothing is
unpacked again. The unpack reads every zstd frame (`zstd -T` writes one,
pzstd one per chunk) with `ruzstd`, the decoder the device's updater uses,
and seeks over all-zero blocks so the base stays sparse. It is written
beside its name first, so a cut-off unpack is never taken for a whole one.
`agent/update` itself is Linux-only (`BLKRRPART`, `posix_fadvise`), which is
why its reader is not reused.

**An app update with a newer image leaves the device on its old base.** The
overlay belongs to the base it was made on, so the launcher keeps it, says a
newer Tessaro is bundled, and Reset to factory moves to it. Sending the new
image with `tessaro-ctl update send` instead is untested on genericarm64.

## Where it keeps things

`~/Library/Application Support/Try Tessaro/` (`$TRY_TESSARO_DATA` for
another): the base, `disk.qcow2`, `state.json` (the base the overlay was
made on and the device's node id), `settings.json`, `qemu.log`,
`serial.log` (the device's console), `qmp.sock`, `terminal.command`, and
`lock`. The lock is held while the app runs, so two copies cannot boot the
same disk. Show logs opens this folder.

## The device and the client store

**The launcher talks to the device through `agent/client`, like both other
clients** (`device.rs`). Readiness is a session opened with
`Trust::KnownOnly`: the device answering `device/id` is up. The first time,
the device goes into the client store, pinned and without a token, the way
`tessaro-gui` keeps an unclaimed device it opened. So `tessaro-gui`'s node
list and `tessaro-ctl` find it with no mDNS. Reset forgets it there, because
the new device has a new certificate and the old pin would refuse it.

**The launcher never claims the device.** An unclaimed device answers every
endpoint without a token (**The claim model** in
[settings.md](settings.md)), so every activity works on a fresh one. Claimed
from the terminal or the GUI, the token lands in the same store and the
launcher keeps working; claimed from Webconfig, the launcher has no token
and says so, pointing at `tessaro-ctl access login`.

**The terminal** is Terminal.app running `terminal.command`, which puts the
bundled `tessaro-ctl` on `PATH` and sets `TESSARO_NODE` to the device's
address, so every command reaches it with no `-n`. **Webconfig** opens
through `webconfig::address`, signed in on a claimed device the store holds
a token for. **Tessaro GUI** is the nested `Tessaro.app`.

## The activities

**Each card does one thing on the device and shows the `tessaro-ctl`
commands that do the same** (`activities.rs`), so trying Tessaro teaches
the CLI. The work is `agent/client`'s (`config::set`, the endpoint types,
`describe`), never a third copy. A new card is a variant of `Activity` and
`Action`, its title, words and commands, a branch in `run`, and its buttons
in `main.rs`; the test that every card names its commands holds it to that.

## The look

**`gui/tessaro-style` is the look Try Tessaro and `tessaro-gui` share**: the
palette, the widget styles, Manrope and the drawn icons, in the welcome
page's colours. Try Tessaro's text is a step bigger than the GUI's
(`TEXT` in `main.rs`): it is read by someone new to Tessaro, not scanned by
a technician. The window is the welcome page's dark desk with the logo, a
status pill and one large Start/Stop button over two tabs: **Basic**, the
device, the Open buttons and the settings, and **Try it**, the activities
as task cards.

**Its icon is `tessaro-gui`'s with a DEMO ribbon** (`icons/try-tessaro.svg`),
so the two apps tell apart in the Dock. `icons/generate-macos.sh` makes the
PNG the header shows and `TryTessaro.icns` from it, with `rsvg-convert`;
rerun it after changing the SVG.
