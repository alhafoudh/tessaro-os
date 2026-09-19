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
| `mise run build` | Build the image and OVMF firmware |
| `mise run shell` | Interactive kas shell (cwd is the build dir) |
| `mise run unpack` | Decompress the `.wic` for runqemu |
| `mise run run` | Boot in QEMU, serial console on the terminal |
| `mise run run-vnc` | Boot in QEMU with VNC on localhost:5900 |
| `mise run clean` | Drop build artifacts, keep sstate and downloads |

Exit the QEMU serial console with `Ctrl-a x`.

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
kas clones and checks out from pins in `kas/*.yml`, and is gitignored:
`meta-moonforge/`, `openembedded-core/`, `bitbake/`, plus `build/` (TOPDIR) and
`cache/` (`DL_DIR` + `SSTATE_DIR`).

In kas, a `repos:` entry with **no `url:`** is the repo holding the config file,
which kas never touches. That is the `tessaro-os:` entry. Upstream layers get a
`url`/`commit`/`branch` instead. Bumping Moonforge means changing one commit
hash in `kas/tessaro-image-base-qemux86-64.yml`.

Configuration arrives through three chains that each span several files:

1. **kas includes.** `kas/tessaro-image-base-qemux86-64.yml` pulls
   `meta-moonforge:kas/include/layer/meta-moonforge-*.yml`. Each *layer*
   fragment activates its layer, pulls the *repo* fragments it needs
   (`kas/include/repo/*.yml`, which carry the url/commit pins), and contributes
   `local_conf_header` defaults. So enabling a feature is one `includes:` entry
   here, never a manual `bblayers.conf` edit - kas regenerates `build/conf/`
   on every invocation.
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
that layer does not trip over a dangling bbappend.

## Kiosk browser

`meta-moonforge-wpe` gives WPEWebKit fullscreen on Weston via
`wpe-simple-launcher`. It pulls in `meta-moonforge-graphics`, `meta-webkit` and
`meta-openembedded` on its own, so it costs exactly one `includes:` entry.

Upstream bakes `WPE_SIMPLE_LAUNCHER_URL` into `wpe-simple-launcher.service` with
a `sed` at `do_compile`, so the URL is a literal in the unit and can only be
changed by rebuilding. Tessaro layers a runtime knob on top, in
`dynamic-layers/meta-moonforge-wpe/recipes-browser/wpe-simple-launcher/`:

* a drop-in at
  `/lib/systemd/system/wpe-simple-launcher.service.d/10-tessaro-kiosk.conf`
  resets `ExecStart`, re-points it at `${KIOSK_URL}`, and carries the
  build-time default in `Environment=`.
* `/etc/default/tessaro-kiosk` is an `EnvironmentFile=` that overrides it,
  shipped with the assignment **commented out**.

The default deliberately lives in the drop-in under `/lib`, not in `/etc`:
`/etc` is an overlayfs upper on `/data`, so the first write to a file there
shadows the image's copy permanently and no later image could move the default
again. Keeping `/etc` empty until someone opts in preserves that.

Apply a change with `systemctl restart wpe-simple-launcher`.

Two things to know about the unit:

* **The URL is not shell-safe.** `/usr/bin/wpe-exported-wayland` ends in
  `su weston -c "... $*"` with `$*` unquoted, so the URL is re-parsed by a
  second shell. A `&` in a query string would background the launcher, and
  `;`/backticks/`$()` are live. Fine for the plain
  `https://www.freevision.sk` we ship; fixing it properly means overriding
  that script in our layer.
* **The start limit had to be disabled.** `wpe-exported-wayland` exits in
  milliseconds when `weston-keyboard` is not up, which burns systemd's default
  5-starts-in-10s limit before Weston finishes and fails the unit for good.
  The drop-in sets `StartLimitIntervalSec=0`, `Restart=always`, `RestartSec=2`
  and an `ExecStartPre` that waits for `weston-keyboard`.

## Gotchas

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
  kiosk browser as Cog with `WAYLAND_COG_LAUNCH_URL`, but the layer now ships
  `wpe-simple-launcher` with `WPE_SIMPLE_LAUNCHER_URL`. Trust the layer sources
  over upstream docs.

## Status

`qemux86-64` works and is the development target, and carries the kiosk browser.
Raspberry Pi 4/5 is not wired up yet; it is available as a Moonforge layer
(`meta-moonforge-raspberrypi`) and costs one `includes:` entry.
