# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Yocto/OpenEmbedded build for **Tessaro**, a web kiosk, producing Linux images
for the platforms it ships on. Derivative of
[Moonforge](https://moonforgelinux.org/).

Obsidian tracking: no

## Commands

Use the mise tasks rather than calling `kas-container` directly:

| Task | Purpose |
| --- | --- |
| `mise run build` | Build the image for `$TESSARO_MACHINE` (plus OVMF on qemu) |
| `mise run build-qemu` | Same, forced to `qemux86-64` |
| `mise run build-x86` | Same, forced to `genericx86-64` |
| `mise run build-rpi` | Same, forced to `raspberrypi3-64` |
| `mise run shell` | Interactive kas shell (cwd is the build dir) |
| `mise run unpack` | Decompress the `.wic` for runqemu |
| `mise run run` | Boot in QEMU, serial console on the terminal |
| `mise run run-vnc` | Boot in QEMU with VNC on localhost:5900 |
| `mise run clean` | Drop build artifacts, keep sstate and downloads |

Exit the QEMU serial console with `Ctrl-a x`.

**Every task acts on one machine**, `$TESSARO_MACHINE`, defaulting to
`qemux86-64`. The `build-*` tasks are one-line wrappers that set it; anything
else takes it from the environment:

```sh
TESSARO_MACHINE=raspberrypi3-64 mise run shell
```

`mise.toml` derives `KAS_CONFIG`, `KAS_BUILD_DIR` and `WIC` from that one
variable, so each machine gets its own TOPDIR under `build/<machine>/` while
`cache/` (`DL_DIR` + `SSTATE_DIR`) stays shared. Valid values are exactly the
basenames in `kas/machine/`. `run`, `run-vnc` and OVMF are qemu-only and refuse
to run on anything else.

There is no test suite or linter; correctness is "the image builds and boots".
For work on a single recipe, go through the kas shell so bitbake sees the right
environment:

```sh
mise run shell                          # then, inside (cwd is /build):
bitbake -e <recipe> | grep '^VAR='      # resolved value of a variable
bitbake -c cleansstate <recipe>         # force a rebuild of one recipe
bitbake -c devshell <recipe>            # shell in the recipe's build dir
bitbake -n moonforge-image-base         # dry run: what would rebuild
```

Builds are long. Run them in a Herdr pane, not the Bash tool.

## Architecture

**This repository is the kas root repo.** Everything else is a build input that
kas clones and checks out from pins under `kas/`, and is gitignored:
`meta-moonforge/`, `openembedded-core/`, `bitbake/`, the BSP layers a target
pulls in (`meta-raspberrypi/`, `meta-lts-mixins/`, `meta-yocto/`), plus
`build/<machine>/` (TOPDIR) and `cache/` (`DL_DIR` + `SSTATE_DIR`).

In kas, a `repos:` entry with **no `url:`** is the repo holding the config file,
which kas never touches. That is the `tessaro-os:` entry. Upstream layers get a
`url`/`commit`/`branch` instead. Bumping Moonforge means changing one commit
hash in `kas/common/tessaro.yml`.

**One config chain per machine.** A build is always a machine fragment plus the
shared debug fragment, `kas/machine/<machine>.yml:kas/common/debug.yml`, which
is what `mise.toml` assembles. The machine fragment includes
`kas/common/tessaro.yml` (the pins, `meta-tessaro-distro`, the kiosk layers,
`IMAGE_DATA_MIN_SIZE`) and adds only what is board-specific: the layer fragment
for that BSP, `WKS_FILE`, `OVERLAYFS_ETC_DEVICE`, `distro`, `machine`. Adding a
target is one new file in `kas/machine/`; nothing else moves.

`kas/common/debug.yml` is a one-line wrapper that includes Moonforge's own
`kas/common/debug.yml`. It has to exist as a local file because kas splits a
config chain on `:` and treats each element as a plain path, so only an
`includes:` entry can be repo-prefixed, never a top-level config.

Configuration arrives through three chains that each span several files:

1. **kas includes.** `kas/machine/<machine>.yml` and `kas/common/tessaro.yml`
   pull `meta-moonforge:kas/include/layer/meta-moonforge-*.yml`. Each *layer*
   fragment activates its layer, pulls the *repo* fragments it needs
   (`kas/include/repo/*.yml`, which carry the url/commit pins), and contributes
   `local_conf_header` defaults. So enabling a feature is one `includes:` entry
   here, never a manual `bblayers.conf` edit - kas regenerates
   `build/<machine>/conf/` on every invocation. `local_conf_header` keys merge
   by *name* across the whole chain, so a key reused by two fragments silently
   replaces the other's block; ours are `20_tessaro-common` and
   `25_tessaro-machine`, upstream's are `10_`/`20_meta-moonforge-*`.
2. **Distro.** `meta-tessaro-distro/conf/distro/tessaro.conf` does
   `require conf/distro/moonforge.conf` and overrides only identity fields plus
   the hostname. Everything else (systemd, uninative, `OEEquivHash`,
   security flags, `linux-yocto 6.6`, `TARGET_VENDOR = "-moonforge"`) is
   inherited.
3. **Image.** `moonforge-image-base.bb` in meta-moonforge is just
   `inherit moonforge-image`; `moonforge-image.bbclass` inherits `core-image`
   and sets `read-only-rootfs`, `overlayfs-etc`, `splash`, the `ext4 wic.bz2`
   fstypes and the `IMAGE_NAME`/`IMAGE_VERSION_SUFFIX` scheme.

**Where product changes go:** system-wide policy in `tessaro.conf`; packages and
image features in `meta-tessaro-distro/recipes-core/images/moonforge-image-base.bbappend`.
The image recipe itself stays upstream's - do not fork it.

Appends to recipes from an optional upstream layer go under
`meta-tessaro-distro/dynamic-layers/<collection>/`, wired up by `BBFILES_DYNAMIC`
in `meta-tessaro-distro/conf/layer.conf`. That way a target that does not enable
that layer does not trip over a dangling bbappend. There are none right now -
and note the key is the layer's `BBFILE_COLLECTIONS` name, not its directory
name: meta-webkit registers itself as `webkit`. Prefer a `:pn-<recipe>` override
in `tessaro.conf` when all you need is a variable; that is how cog's
`PACKAGECONFIG` is set without a bbappend at all.

## Kiosk browser

Igalia's **cog** (0.18.5, from `meta-webkit`) fullscreen on Weston. Moonforge's
own launcher, `wpe-simple-launcher`, is deliberately *not* installed: it
connects no error signals at all, so a network blip, a bad certificate or a
renderer crash left WebKit's error page on screen forever with nothing in the
journal and no process exit for systemd to act on. Cog connects `load-failed`,
`load-failed-with-tls-errors` and `web-process-terminated` by default and has a
D-Bus control interface.

`meta-moonforge-wpe` stays in the `includes:` - it sets
`PREFERRED_PROVIDER_virtual/wpebackend` (cog's `wl` plugin needs wpebackend-fdo),
pulls in `meta-moonforge-graphics`, `meta-webkit` and `meta-openembedded`, and
maps `/home` onto `/data/overlay-home`. Only its *package* is dropped, with a
`CORE_IMAGE_EXTRA_INSTALL:remove` in `moonforge-image-base.bbappend`, because
the `+=` that installs it lives in the pinned upstream kas fragment.

Everything else is `meta-tessaro-distro/recipes-browser/tessaro-kiosk/`:

* `tessaro-kiosk.service` runs `cog --platform=wl` as the `weston` user.
* `tessaro-kiosk-watchdog.service` probes the URL with curl, re-navigates cog
  through D-Bus, shows a local offline page while the site is down, and restarts
  the browser when it stops answering.
* `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` carries the build-time defaults
  (`TESSARO_KIOSK_URL` from `tessaro.conf`), `/etc/default/tessaro-kiosk`
  overrides them and ships entirely **commented out**.

The defaults deliberately live under `/usr/lib`, not `/etc`: `/etc` is an
overlayfs upper on `/data`, so the first write to a file there shadows the
image's copy permanently and no later image could move the default again.

Apply a change with `systemctl restart tessaro-kiosk tessaro-kiosk-watchdog`.

Things to know:

* **`cogctl` needs `--system`.** `tessaro.conf` sets
  `PACKAGECONFIG:append:pn-cog = " dbus"`, which moves the control interface
  off the session bus so a root watchdog can reach a browser running as
  `weston`. `COG_DBUS_OWN_USER` must match `User=` in the unit or cog cannot
  own its name at all.
* **`cogctl ping` is broken in 0.18.5** - it tests the `GError**` instead of the
  connection and always fails without contacting the bus. The watchdog uses
  `busctl ... org.freedesktop.DBus.Peer Ping`.
* **The control surface is write-only** - five stateless actions (`quit`,
  `previous`, `next`, `reload`, `open`). Nothing can be read back, so the
  watchdog cannot tell a live page from cog's error page and simply
  re-navigates on every successful probe.
* **Cog's error page is a hardcoded C string** with a "Try again" *button*,
  useless without a pointer. Ours is a separate page the watchdog navigates to:
  `$KIOSK_OFFLINE_URL`, else `/data/kiosk/offline.html`, else
  `/usr/share/tessaro-kiosk/offline.html`, staged into `/run/tessaro-kiosk` and
  served through cog's `--dir-handler=tessaro:` scheme.
* **`--webprocess-failure=exit`** is what makes renderer death visible to
  systemd. Cog's own `restart` mode reloads in-process at most 5 times in a
  hardcoded 1s window and then sits on an error page, which looks healthy to
  everything.
* **`XDG_RUNTIME_DIR` is mandatory** even though `WAYLAND_DISPLAY` is an
  absolute path: wpebackend-fdo puts its nested Wayland display there. Unset,
  cog runs, answers D-Bus and paints nothing.
* **Ctrl-W quits the kiosk.** Cog's wl plugin hardwires it (and F11, F5/Ctrl-R,
  Alt-arrows, Ctrl-+/-/0) with no way to disable them. `Restart=always` - not
  `on-failure`, because that path exits 0 - is the only mitigation short of
  patching cog.
* **Diagnostics are journal-only** by design; nothing technical reaches the
  screen. `journalctl -fu tessaro-kiosk-watchdog`.
* **Our D-Bus policy lives in `/etc/dbus-1/system.d/`, and its comments may not
  contain `--`.** cog's own `com.igalia.Cog.conf` lets `context="default"` - any
  local user - send to the kiosk, `quit` included. Overriding it means being
  parsed later, and `dbus-1`'s `system.conf` lists
  `<includedir>system.d</includedir>` before
  `<includedir>/etc/dbus-1/system.d</includedir>`, so the directory is the only
  ordering guarantee; filename sorting within one directory is not promised.
  Separately, a `--` anywhere in an XML comment ends it and makes the file
  malformed, at which point dbus discards the whole policy **silently** and you
  get no hardening at all - which is what pasting a `busctl --system ...`
  example into the comment did. `journalctl -b -u dbus` shows the parse error.

## Gotchas

* **`distro:` has to be set by the entry point of the kas chain.** kas resolves
  a plain scalar by include order, and a file's own value beats the ones its
  includes set. Every machine fragment includes a `meta-moonforge-*` layer
  fragment, which pulls `meta-moonforge-distro.yml`, which says
  `distro: moonforge` - so `distro: tessaro` sitting in `kas/common/tessaro.yml`
  gets silently undone and the whole image builds as Moonforge (no `tessaro`
  hostname, no cog `PACKAGECONFIG`, `DISTROOVERRIDES` flipped). It lives in each
  `kas/machine/*.yml` instead. `kas dump <chain>` prints the resolved value and
  is the cheap way to check after touching includes.
* **`genericx86-64` comes from `meta-yocto-bsp`, not meta-intel.** The generic
  x86 machines moved out of meta-intel years ago; meta-intel's own machines
  would switch `virtual/kernel` to `linux-intel` and pull in the Intel media
  stack, while meta-yocto-bsp keeps `linux-yocto 6.6` and works on AMD boards.
  It is pinned as the split-out `meta-yocto` repo (`kas/repo/meta-yocto.yml`),
  not all of poky, and only the `meta-yocto-bsp` layer is enabled - the repo
  root is not a layer, so the `layers:` key there is mandatory.
* **`/data` is mounted by label, not by device node.**
  `OVERLAYFS_ETC_DEVICE = "LABEL=data"` in `kas/common/tessaro.yml`, so one
  image boots off SATA, USB, NVMe or SD unchanged. It works because the preinit
  generated by `overlayfs-etc.bbclass` runs `/bin/mount`, which is
  `util-linux-mount` with `libblkid1` behind it (not busybox), on a kernel with
  `CONFIG_DEVTMPFS_MOUNT=y` - `/dev` is populated by the kernel before
  `/sbin/init` runs, so blkid can resolve the label. Every wks in use passes
  `--label data`, which wic turns into `mkfs.ext4 -L`. If a wks ever drops that
  label the symptom is `PREINIT: Mounting </data> failed!` on the console
  followed by a booting but non-persistent system. Two disks carrying a `data`
  label (usually the flashing USB stick left plugged in) is the one case this
  gets wrong, and device nodes are no safer there - that stick is often
  `/dev/sda`.
* **wic rewrites `/etc/fstab` inside the image, and that is a second place a
  disk gets named.** `update_fstab()` in `scripts/lib/wic/plugins/imager/direct.py`
  adds a line for every partition with a mountpoint: `PARTUUID=`/`UUID=` with
  `--use-uuid`, `LABEL=` with `--use-label`, and a bare `/dev/sdaN` otherwise.
  So the preinit mounting `/data` by label is only half the job - without
  `--use-label` on that partition, fstab still says `/dev/sda3`, and on an NVMe
  board systemd fails `data.mount` after a perfectly good preinit. Our
  genericx86-64 wks passes it. The upstream qemu and Pi wks files do not, which
  is harmless there (`sda` under QEMU, `mmcblk0` on SD) right up until someone
  boots the Pi image off USB.
* **The x86 hardware image is UEFI-only.**
  `meta-tessaro-distro/wic/tessaro-image-base-genericx86-64.wks.in` is GPT plus
  an ESP with grub-efi. `genericx86-64` does declare the `pcbios`
  `MACHINE_FEATURE`, and oe-core's `bootimg-biosplusefi` wic plugin can put
  syslinux and an EFI loader in the same `/boot` partition (it picks
  syslinux's `gptmbr.bin` on GPT, so the partition table can stay), but nothing
  here is set up or tested for legacy boot today.
* **The Pi target is `raspberrypi3-64` and covers the 3B and 3B+** - the B+
  device tree is in `RPI_KERNEL_DEVICETREE` and the firmware picks it at boot.
  `meta-moonforge-raspberrypi` advertises Pi 4/5 only, but contains nothing
  board-specific (psplash framebuffer config, a udev rule, the mmcblk wic
  layout). The 3B+ has 1GB of RAM shared with the GPU and WPEWebKit is the
  heaviest thing in the image, so `GPU_MEM` and `VC4DTBO` (fake KMS by default
  on this machine) are the first knobs if the kiosk gets OOM-killed or crawls;
  both are noted in the fragment and left at meta-raspberrypi's defaults.
* **runqemu needs a file path, not an image name.** `runqemu ... qemux86-64
  moonforge-image-base wic` fails with `IMAGE_LINK_NAME wasn't set`: the image
  name is treated as a lazy rootfs, and the machine argument makes runqemu run
  `bitbake -e` with no recipe target, where `IMAGE_LINK_NAME` (set by the
  recipe-scope `image-artifact-names.bbclass`) does not exist. Pass the
  `.rootfs.wic` path instead - that is what the `run` tasks do.
* **KVM inside the kas container** needs `-e GROUP_ID=$(stat -c %g /dev/kvm)`,
  not `--group-add`: the entrypoint's `gosu builder` rebuilds supplementary
  groups from the container's `/etc/group`, so only the primary gid survives.
* **The build host is headless.** `nographic` is the default for `run`; use
  `run-vnc` plus an SSH tunnel if you need the framebuffer. With the kiosk
  enabled, `run` shows nothing but the serial console - the browser needs
  `run-vnc`.
* **The kiosk needs a real GPU on the build host to render under QEMU.** Only
  the DRM master may allocate KMS dumb buffers, which is how Mesa's
  `kms_swrast` backs GBM when there is no GPU. Weston holds master so Weston
  draws; `WPEWebProcess` runs as the unprivileged `weston` user and is refused
  with `DRM_IOCTL_MODE_CREATE_DUMB failed: Permission denied`, so the browser
  loads the page and silently paints nothing. WPE 2.52 has no `wl_shm`
  fallback - `WEBKIT_DISABLE_DMABUF_RENDERER` was removed in that release, and
  forcing Weston to `use-pixman` only changes the failure to `no valid format
  found`. An ordinary SHM client such as `weston-simple-shm` still renders, so
  a blank screen with a healthy Weston is this bug, not a broken compositor.
  The fix is host-side: a render node at `/dev/dri`, passed into the kas
  container, with `runqemu ... egl-headless` selecting `virtio-vga-gl`/virgl.
  If the host kernel boots with `nomodeset`, no GPU driver loads at all and
  `modprobe amdgpu` fails with `Invalid argument`; that has to come off the
  kernel command line first.
* **`runqemu`'s `QB_MEM` default is 256M**, which WPEWebKit plus Weston will not
  survive. Fixed in the `30_tessaro-qemu-kiosk` block of the kas fragment.
* **`QB_GRAPHICS` is the knob for the QEMU display, not `QB_OPT_APPEND`** -
  `runqemu` appends `QB_GRAPHICS` unconditionally, while
  `x86/qemuboot-x86.inc` already owns `QB_OPT_APPEND`. `runqemu`'s
  `setup_vga()` only adds `-device virtio-vga` on its `sdl`/`gtk`/
  `egl-headless` paths, never on `publicvnc`. This is a performance choice, not
  a prerequisite: QEMU's default std VGA plus `CONFIG_DRM_BOCHS=y` in
  `linux-yocto` already gives Weston a `/dev/dri` node. Note `-device
  virtio-vga` does not replace the default std VGA, so `-vga none` goes with it
  or Weston sees two cards. Falling back to the default std VGA is a working
  configuration if virtio ever misbehaves under OVMF.
* **Weston is `WantedBy=graphical.target`, not `multi-user.target`.** That
  works because `rootfs-postcommands.bbclass` sets
  `SYSTEMD_DEFAULT_TARGET = "graphical.target"` whenever `IMAGE_FEATURES` has
  `weston`. If the compositor never starts, check `systemctl get-default` first.
* **`DISTROOVERRIDES` is `tessaro`, not `moonforge`.** Any `VAR:moonforge = ...`
  in an upstream layer silently stops applying, with no warning. There are
  currently zero such lines; re-check after a Moonforge bump or when enabling a
  new layer. Fix if needed: `DISTROOVERRIDES =. "moonforge:"` in `tessaro.conf`.
* **Artifacts are named `...-qemux86-64-0.*`** because the kas fragment sets
  `IMAGE_VERSION: "0"`. The stable symlink is `...-qemux86-64.rootfs.*`, and
  the mise tasks depend on that `.rootfs` spelling.
* **Moonforge's `STRUCTURE.md` is stale in places** - e.g. it documents the
  kiosk browser as Cog with `WAYLAND_COG_LAUNCH_URL`, while the layer actually
  ships `wpe-simple-launcher` with `WPE_SIMPLE_LAUNCHER_URL`. Tessaro is back on
  Cog, but through its own units, and no `WAYLAND_COG_LAUNCH_URL` is involved.
  Trust the layer sources over upstream docs.

## Status

Three targets, one fragment each in `kas/machine/`. All three carry the same
image: read-only rootfs, overlayfs `/etc` on `/data`, Weston and the cog kiosk.

| Machine | Purpose | State |
| --- | --- | --- |
| `qemux86-64` | development, boots through `mise run run-vnc` | builds and boots |
| `genericx86-64` | shipping x86_64 hardware (UEFI) | configured, never built end to end |
| `raspberrypi3-64` | Raspberry Pi 3 Model B+ | configured, never built end to end |

"Configured" means the kas chain resolves and bitbake parses it with the right
`DISTRO`/`MACHINE`/`WKS_FILE`; neither image has been built or booted on real
hardware yet. Expect the first build of each to surface fetch or packaging
issues that parsing cannot.

Writing an image to a card or disk is deliberately not a mise task - decompress
with `mise run unpack` and `dd` the `.wic` yourself.
