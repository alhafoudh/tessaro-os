# Build system and platforms

## Architecture

**This repository is the kas root repo.** Everything else is a build input that
kas clones and checks out from pins under `kas/` here and in meta-moonforge's
`kas/include/repo/`, and is gitignored: `meta-moonforge/`,
`openembedded-core/`, `bitbake/`, `meta-openembedded/`, Chromium's layers
(`meta-browser/`, `meta-clang/`, `meta-lts-mixins-rust/`), the BSP layers a
target pulls in (`meta-raspberrypi/`, `meta-lts-mixins/`, `meta-yocto/`), plus
`build/<machine>/` (TOPDIR) and `cache/` (`DL_DIR` + `SSTATE_DIR`).

In kas, a `repos:` entry with **no `url:`** is the repo holding the config file,
which kas never touches. That is the `tessaro-os:` entry. Upstream layers get a
`url`/`commit`/`branch` instead. Bumping Moonforge means changing one commit
hash in `kas/common/tessaro.yml`.

**One config chain per machine.** A build is always a machine fragment plus the
shared debug fragment, `kas/machine/<machine>.yml:kas/common/debug.yml`, which
is what `mise.toml` assembles. The machine fragment includes
`kas/common/tessaro.yml` (the Moonforge pin, the kiosk layers with Chromium's
pins from `kas/repo/meta-chromium.yml`, `meta-tessaro-distro`, and the disk
layout: `IMAGE_DATA_MIN_SIZE`, `OVERLAYFS_ETC_DEVICE`, `TESSARO_ROOTFS_SIZE`,
`TESSARO_ESP_SIZE`, the bundled initramfs) and adds only what is
board-specific: the layer or repo fragment for that BSP, `WKS_FILE`, `distro`,
`machine`, and board knobs (`QB_*` on qemu, `VC4DTBO` and the boot files on
the Pi). Adding a target is one new file in `kas/machine/`; nothing else moves.

`kas/common/debug.yml` is a one-line wrapper that includes Moonforge's own
`kas/common/debug.yml`. It has to exist as a local file because kas splits a
config chain on `:` and treats each element as a plain path, so only an
`includes:` entry can be repo-prefixed, never a top-level config.

Configuration arrives through chains that each span several files:

1. **kas includes.** `kas/common/tessaro.yml` and the qemu and Pi machine
   fragments pull `meta-moonforge:kas/include/layer/meta-moonforge-*.yml`. Each *layer*
   fragment activates its layer, pulls the *repo* fragments it needs
   (`kas/include/repo/*.yml`, which carry the url/commit pins), and contributes
   `local_conf_header` defaults. So enabling a feature is one `includes:` entry
   here, never a manual `bblayers.conf` edit - kas regenerates
   `build/<machine>/conf/` on every invocation. `local_conf_header` keys merge
   by *name* across the whole chain, so a key reused by another fragment
   silently replaces the other's block; ours are `20_tessaro-common`,
   `25_tessaro-machine` and `30_tessaro-qemu-kiosk`, upstream's are
   `10_`/`20_`/`30_meta-moonforge-*` and `20_common-*`.
2. **Distro.** `meta-tessaro-distro/conf/distro/tessaro.conf` does
   `require conf/distro/moonforge.conf`, overrides the identity fields and the
   hostname, and carries the product's system-wide policy (`PACKAGECONFIG`s,
   the kiosk URLs, Chromium's arguments). Everything else (systemd, uninative, `OEEquivHash`,
   security flags, `linux-yocto 6.6`, `TARGET_VENDOR = "-moonforge"`) is
   inherited.
3. **Image.** `moonforge-image-base.bb` in meta-moonforge is just
   `inherit moonforge-image`; `moonforge-image.bbclass` inherits `core-image`
   and sets `read-only-rootfs`, `overlayfs-etc`, `splash`, the `ext4 wic.bz2`
   fstypes and the `IMAGE_NAME`/`IMAGE_VERSION_SUFFIX` scheme. Our bbappend
   swaps `wic.bz2` for `wic.zst` plus `wic.bmap`, compressed at level 19 on
   that recipe only (see **Images are zstd** in [updates.md](updates.md)).

**Where product changes go:** system-wide policy in `tessaro.conf`; packages and
image features in `meta-tessaro-distro/recipes-core/images/moonforge-image-base.bbappend`;
kiosk supervision behaviour in `agent/`, which is ordinary Rust and not a Yocto
concern at all. The image recipe itself stays upstream's - do not fork it.

Appends to recipes from an optional upstream layer go under
`meta-tessaro-distro/dynamic-layers/<collection>/`, wired up by a
`BBFILES_DYNAMIC` in `meta-tessaro-distro/conf/layer.conf` (only a comment
there today). That way a target that does not enable that layer does not trip
over a dangling bbappend. There are none right now: meta-chromium is on in
every target, so `recipes-browser/chromium/chromium-ozone-wayland_%.bbappend`
(a source patch) lives in the plain tree; move it under
`dynamic-layers/chromium-browser-layer/` if a target ever drops Chromium. Note
the key is the layer's `BBFILE_COLLECTIONS` name, not its directory name:
meta-chromium registers itself as `chromium-browser-layer`. Prefer a
`:pn-<recipe>` override in `tessaro.conf` when all you need is a variable; that
is how Chromium's `PACKAGECONFIG` and `CHROMIUM_EXTRA_ARGS` are set.

## Gotchas

* **`distro:` has to be set by the entry point of the kas chain.** kas resolves
  a plain scalar by include order, and a file's own value beats the ones its
  includes set. Every machine's chain includes a `meta-moonforge-*` layer
  fragment, directly or through `kas/common/tessaro.yml`, which pulls `meta-moonforge-distro.yml`, which says
  `distro: moonforge` - so `distro: tessaro` sitting in `kas/common/tessaro.yml`
  gets silently undone and the whole image builds as Moonforge (no `tessaro`
  hostname, no Chromium `PACKAGECONFIG`, `DISTROOVERRIDES` flipped). It lives in each
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
  genericx86-64 wks passes it. Our qemux86-64 and raspberrypi wks files
  (copies of Moonforge's) do not, which is harmless there (`sda` under QEMU, `mmcblk0` on SD) right up until someone
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
  board-specific (psplash framebuffer config, a udev rule), and the disk
  layout is our own `tessaro-image-base-raspberrypi.wks.in`. The 3B+ has 1GB
  of RAM shared with the GPU and Chromium is heavy, so memory is the
  constraint to plan the Pi target around.
  `VC4DTBO` is set to `vc4-kms-v3d` (full KMS, see **Display hotplug** in
  [display.md](display.md));
  `GPU_MEM` stays unset. Under full KMS the GPU draws from the CMA pool, and
  what `gpu_mem` still sizes - the firmware's own share, which its hardware
  video decoder uses - is a device setting, `device.gpu_mem`, written into
  `tessaro.txt` on the boot partition. `RPI_EXTRA_CONFIG` makes `config.txt`
  include that file last, and the agent adds the include at boot to a
  `config.txt` that lacks it (`render_firmware` in `render.rs`), since an
  update never rewrites `config.txt`.
* **An AMD board has no GPU driver unless mesa has `gallium-llvm`.** radeonsi
  is only in mesa's gallium drivers with `gallium-llvm` and `r600`
  (`GALLIUMDRIVERS_RADEONSI` in `mesa.inc`); without them the x86 set is
  Intel's, virgl and swrast, and Weston and Chromium render in software on an
  AMD GPU. `tessaro.conf` enables them for x86-64, with the VA driver. Check
  the built drivers in `build/<machine>/tmp/work/*/mesa/*/image/usr/lib/dri`.
* **runqemu needs a file path, not an image name.** `runqemu ... qemux86-64
  moonforge-image-base wic` fails with `IMAGE_LINK_NAME wasn't set`: the image
  name is treated as a lazy rootfs, and the machine argument makes runqemu run
  `bitbake -e` with no recipe target, where `IMAGE_LINK_NAME` (set by the
  recipe-scope `image-artifact-names.bbclass`) does not exist. Pass the
  `.rootfs.wic` path instead - that is what the `qemu:*` tasks do.
* **KVM inside the kas container** needs `-e GROUP_ID=$(stat -c %g /dev/kvm)`,
  not `--group-add`: the entrypoint's `gosu builder` rebuilds supplementary
  groups from the container's `/etc/group`, so only the primary gid survives.
* **The build host is headless.** `nographic` is the default for `qemu:run`;
  use `qemu:vnc` plus an SSH tunnel if you need the framebuffer. With the
  kiosk enabled, `qemu:run` shows nothing but the serial console - the browser
  needs `qemu:vnc`.
* **`qemu:vnc`'s display asks for the password `tessaro`, which needs QEMU
  built with nettle.** VNC password auth is DES, and oe-core's
  `qemu-system-native` has no crypto library that provides it: QEMU refuses
  to start with `Cipher backend does not support DES algorithm`. The password
  goes in as `-object secret` with `-vnc ...,password-secret=` (`mise.toml`),
  so no monitor command is needed, and VNC reads at most 8 characters of it.
  `PACKAGECONFIG:append:pn-qemu-system-native = " nettle"` in
  `kas/machine/qemux86-64.yml` is a build-time dependency of the host's QEMU
  only, which is why it is in the machine fragment and not `tessaro.conf`:
  the image gets no new package and no target recipe rebuilds.
* **The kiosk needs a real GPU on the build host to render under QEMU.** Only
  the DRM master may allocate KMS dumb buffers, which is how Mesa's
  `kms_swrast` backs GBM when there is no GPU. Weston holds master so Weston
  draws; Chromium's unprivileged GPU process is refused with
  `DRM_IOCTL_MODE_CREATE_DUMB failed: Permission denied`, so the browser
  loads the page and silently paints nothing. An ordinary SHM client such as
  `weston-simple-shm` still renders, so a blank screen with a healthy Weston
  is this bug, not a broken compositor. The fix is host-side: a render node
  at `/dev/dri`, passed into the kas container, with `runqemu ... egl-headless`
  selecting `virtio-vga-gl`/virgl. If the host kernel boots with `nomodeset`,
  no GPU driver loads at all and `modprobe amdgpu` fails with `Invalid
  argument`; that has to come off the kernel command line first.
* **`runqemu`'s `QB_MEM` default is 256M**, which Chromium plus Weston will
  not survive. Fixed at 4G in the `30_tessaro-qemu-kiosk` block of the kas
  fragment.
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
  in an upstream layer silently stops applying, with no warning. Re-check
  (`git grep ':moonforge'` over the layers) after a Moonforge bump or when
  enabling a new layer. Fix if needed: `DISTROOVERRIDES =. "moonforge:"` in `tessaro.conf`.
* **Artifacts are named `tessaro-os-qemux86-64-<version>.*`**, e.g.
  `tessaro-os-qemux86-64-0.1.0-1a2b3c4.wic.zst`: the `tessaro-os` prefix
  is `IMAGE_BASENAME` in `moonforge-image-base.bbappend` (it defaults to `${PN}`,
  which would name the product after the upstream recipe), and the version is
  `IMAGE_VERSION` in `tessaro.conf`: `DISTRO_VERSION` (the semver marketing
  version, bumped by hand) plus the short sha of this repo's `HEAD`, computed
  at parse, with `-dirty` appended when `git status` reports any change. It overrides the `IMAGE_VERSION: "0"` that Moonforge's
  `meta-moonforge-distro.yml` kas fragment sets under `env:`, and it is also
  `IMAGE_VERSION` in os-release. The separator is `-`, not semver's `+`,
  because os-release allows only `[0-9a-z._-]` there. The stable symlink is
  `tessaro-os-qemux86-64.rootfs.*`, and the mise tasks depend on that `.rootfs`
  spelling. The bitbake target is still `moonforge-image-base` - only the
  output is renamed.
* **Moonforge's `STRUCTURE.md` is stale in places.** Trust the layer sources
  over upstream docs.

## Status

One fragment per target in `kas/machine/`. Every target carries the same
image: read-only rootfs, overlayfs `/etc` on `/data`, Weston and the Chromium
kiosk.

| Machine | Purpose | State |
| --- | --- | --- |
| `qemux86-64` | development, boots through `mise run qemu:vnc` | builds and boots |
| `genericx86-64` | shipping x86_64 hardware (UEFI), Intel or AMD GPU | configured, never built end to end |
| `raspberrypi3-64` | Raspberry Pi 3 Model B+ | builds, boots and runs the kiosk on a 3B+, rendering on the GPU (ES 2.0) |

"Configured" means the kas chain resolves and bitbake parses it with the right
`DISTRO`/`MACHINE`/`WKS_FILE`; `genericx86-64` has not been built or booted on
real hardware yet. Expect its first build to surface fetch or packaging issues
that parsing cannot.

Images are written from a workstation, not from the build host: `mise run
image:pull` rsyncs the `$TESSARO_MACHINE` image and bmap from
`$TESSARO_BUILD_HOST` into the repo root (gitignored), `mise run image:flash`
writes it with bmaptool, and
`mise run dev:tunnel` holds the VNC/SSH port forwards. Their settings live in the
gitignored `mise.local.toml`; see README.md. Flashing is the manual path: a
device on the update layout is updated over the network with `mise run
image:update NAME` - see [updates.md](updates.md).
