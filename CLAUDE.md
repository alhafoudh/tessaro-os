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
| `mise run watchdog-image` | Build the watchdog container image and export it into the recipe |
| `mise run watchdog-test` | Watchdog unit tests in the Ruby 4 container |
| `mise run watchdog-integration` | Real Chromium + the watchdog together in compose |

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
to run on anything else. `build` depends on `watchdog-image`, which regenerates
the container archive the recipe packages (gitignored) on every build; with the
docker layer cache that costs seconds.

The watchdog is a Ruby project under `watchdog/` with its own test suite
(minitest, `mise run watchdog-test`) and an integration compose
(`mise run watchdog-integration`). Everything runs in Docker, no Ruby on the
host. For Yocto work on a single recipe, go through the kas shell so bitbake
sees the right environment:

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
`meta-moonforge/`, `openembedded-core/`, `bitbake/`, `meta-openembedded/`, the
BSP layers a target pulls in (`meta-raspberrypi/`, `meta-lts-mixins/`,
`meta-yocto/`), plus `build/<machine>/` (TOPDIR) and `cache/` (`DL_DIR` +
`SSTATE_DIR`).

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
name: meta-chromium registers itself as `chromium-browser-layer`. Prefer a
`:pn-<recipe>` override in `tessaro.conf` when all you need is a variable; that
is how Chromium's `PACKAGECONFIG` is set without a bbappend at all.

## Kiosk browser

**Chromium** (147, `chromium-ozone-wayland` from meta-browser's `meta-chromium`
layer) fullscreen on Weston. It replaced cog/WPE: Chromium is the only browser
that will ever support the WebBluetooth/WebSerial/WebUSB APIs on the roadmap,
and CDP gives the watchdog a real health channel where cog's D-Bus surface was
write-only. The cost is footprint - the Pi 3B+ with its 1GB is likely to OOM,
and a full build takes hours.

The layers arrive through `kas/repo/meta-chromium.yml`, which pins meta-browser
(repo root is not a layer, so `layers:` is mandatory, same pattern as
`kas/repo/meta-yocto.yml`), plus its two dependencies: meta-clang and the Rust
mixin, a *second* checkout of meta-lts-mixins on its `scarthgap/rust` branch
(Moonforge pins the same repo on `scarthgap/u-boot`). meta-moonforge-wpe and
meta-webkit are gone entirely; `/home` on `/data/overlay-home` is now ours
(`meta-tessaro-distro/recipes-core/volatile-binds/volatile-binds.bbappend`), and
Weston/wayland/polkit come from including `meta-moonforge-graphics` directly.

Everything else is `meta-tessaro-distro/recipes-browser/tessaro-kiosk/`:

* `tessaro-kiosk.service` runs Chromium as the `weston` user, with CDP on
  `127.0.0.1:9222` and the profile on `/data/kiosk/chromium`.
* `tessaro-kiosk-watchdog.service` runs the **Ruby watchdog in a podman
  container** (`--network=host`, system bus socket bind-mounted in). The image
  is built by `mise run watchdog-image` from `watchdog/`, shipped as
  `/usr/share/tessaro-kiosk/tessaro-kiosk-watchdog-image.tar.gz` (gitignored,
  regenerated by every `build`), and loaded into podman's `/data` storage by
  `tessaro-kiosk-watchdog-image.service` on first boot.
* `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` carries the build-time defaults
  (`TESSARO_KIOSK_URL` from `tessaro.conf`), `/etc/default/tessaro-kiosk`
  overrides them and ships entirely **commented out**. **Both units read them
  as systemd `EnvironmentFile=`**; the watchdog's container inherits the result
  through podman `--env-host`, so systemd is the only parser involved.
  That is deliberate and worth keeping: podman's own `--env-file` is not
  systemd's parser. It does not strip quotes, so `KIOSK_URL="https://..."`
  reached the browser clean and the watchdog with the quotes still attached,
  which silently failed the watchdog's `\Ahttps?://` check and dropped it to
  refresh-only; and it has no `-` prefix, so a deleted override file killed the
  unit outright. `--env-host` also overwrites `PATH` inside the container,
  which is why `watchdog/Dockerfile`'s `ENTRYPOINT` is an absolute
  `/usr/local/bin/ruby` and `LANG` is pinned in the unit.

The defaults deliberately live under `/usr/lib`, not `/etc`: `/etc` is an
overlayfs upper on `/data`, so the first write to a file there shadows the
image's copy permanently and no later image could move the default again.

Apply a change with `systemctl restart tessaro-kiosk tessaro-kiosk-watchdog`.

The watchdog is a port of the old POSIX-sh one with the health checks
upgraded. It watches the browser over **CDP** (`http://127.0.0.1:9222`):
`/json/list` plus a `Runtime.evaluate` round trip proves the *renderer* is
alive, `Page.navigate` replaces the old D-Bus `open` action, and the offline
page is served as `file:///run/tessaro-kiosk/index.html` (the `tessaro://`
dir-handler died with cog). The restart path is `org.freedesktop.systemd1`
`RestartUnit` over the system bus - no `systemctl` shell-outs. Same state
machine as before: probe cadence vs navigation cadence, fail threshold before
the offline page, one restart per outage, backoff, `nav_state=unknown` after a
browser restart.

Things to know:

* **The CDP port is on the loopback and stays there.** `--remote-debugging-address=127.0.0.1`
  in the unit; the watchdog container reaches it through `--network=host`.
  Recent Chromium also refuses to open the DevTools port with the *default*
  user-data-dir, so `--user-data-dir` must stay set.
* **Do not duplicate the wrapper's flags.** `/usr/bin/chromium` is the
  recipe's wrapper that prepends `CHROMIUM_EXTRA_ARGS` -
  `--ozone-platform=wayland`, plus `--kiosk --no-first-run --incognito` from
  the `kiosk-mode` PACKAGECONFIG in `tessaro.conf`. The unit only adds CDP,
  profile and autoplay flags.
* **`proprietary-codecs` is what plays H.264.** The marketing site's videos
  will not play without it; it is enabled in `tessaro.conf`.
* **Chromium has no D-Bus control interface at all.** Everything is CDP. The
  control-plane D-Bus policy that existed for cog is gone with it.
* **The watchdog container mounts four things**: the system bus socket
  (`/run/dbus/system_bus_socket`, for RestartUnit), `/run/tessaro-kiosk`
  (writable, it stages the offline page there), `/data/kiosk` and
  `/usr/share/tessaro-kiosk` (read-only page sources).
* **Podman storage lives on `/data/containers`** via the Moonforge podman
  layer's bbappend to `container-host-config` (graphroot in storage.conf);
  that is why the recipe RDEPENDS on the package explicitly.
* **Diagnostics are journal-only** by design; nothing technical reaches the
  screen. The watchdog container logs with `--log-driver=journald`:
  `journalctl CONTAINER_NAME=tessaro-kiosk-watchdog`; the browser and the
  units under `journalctl -fu tessaro-kiosk`.
* **Kiosk modes and first run are suppressed at the wrapper level**
  (`--kiosk --no-first-run --incognito`), not by the unit, so profile writes
  stay minimal.
* **`/data/kiosk` is root owned and only `/data/kiosk/chromium` is `weston`.**
  Both come from tmpfiles. The split matters: `/data/kiosk/offline.html` is one
  of the pages the watchdog stages and puts on screen, so a weston-owned parent
  would let the browser user rewrite the page it is being shown - the same rule
  that keeps `/run/tessaro-kiosk` root owned. Chromium only needs its
  `--user-data-dir` writable. `d` lines re-apply owner and mode every boot, so
  a device built before this heals itself.
* **The watchdog degrades gracefully without a system bus**: every Systemd
  method answers as if the unit were stopped and `restart!` raises
  `Tessaro::KioskWatchdog::Error`, so the same image runs in the integration
  compose (no bus) and on a device.
* **The watchdog container is built for the build host's architecture.**
  `mise run watchdog-image` is a plain `docker build`, so the archive is
  amd64; the task refuses any `TESSARO_MACHINE` that is not x86_64 rather than
  ship a container podman cannot start. See the Pi gotcha below.
* **`Requires=`, not just `After=`, ties the watchdog to the image-load unit**,
  and the image-load unit uses `RequiresMountsFor=/data/containers`. With bare
  ordering a failed load left the watchdog crash-looping on `image not known`
  under `Restart=always`, which hides its own cause.

## Networking

**NetworkManager**, from `meta-networking`, which `kas/common/tessaro.yml`
enables on the meta-openembedded pin Moonforge already carries. It replaces
systemd-networkd outright: `PACKAGECONFIG:remove:pn-systemd = "networkd"` in
`tessaro.conf` stops networkd being built at all, and
`PACKAGECONFIG:remove:pn-systemd-conf = "dhcp-ethernet"` drops the
`80-wired.network` that used to provide ethernet DHCP.

The reason is WiFi. Under systemd-networkd, changing a network in the field
means hand-writing a `.network` file and a `wpa_supplicant.conf` in two
syntaxes with no feedback; `nmtui` makes it one screen. The reconfiguration
story is a technician on `getty@tty1` (Ctrl-Alt-F1 - Weston is on tty7), on the
serial console, or over SSH.

Things to know:

* **Ethernet DHCP is still zero-configuration.** NM's auto-default gives any
  managed ethernet device with no stored profile an in-memory
  `Wired connection 1` with `ipv4.method=auto`. Nothing sets `no-auto-default`.
  A factory device boots and takes a lease exactly as before; the profile is
  just ephemeral until someone saves a real one.
* **Profiles persist for free, state needs a unit.**
  `/etc/NetworkManager/system-connections` is on the `/etc` overlay, so saved
  connections land on `/data` with no work. `/var/lib/NetworkManager` is tmpfs,
  because oe-core's `VOLATILE_BINDS` maps `/var/volatile/lib` over `/var/lib`,
  and `tessaro-network-state.service` binds it to `/data/overlay-nm`.
* **That state unit is deliberately *not* a `VOLATILE_BINDS` entry**, even
  though `/home` is one. Adding `/data/overlay-nm /var/lib/NetworkManager` to
  `VOLATILE_BINDS` is silently broken: every unit volatile-binds generates is
  `DefaultDependencies=no` and `Before=local-fs.target` with **no ordering
  between them**, and the template carries `ConditionPathIsReadWrite=!<where>`.
  Race `var-volatile-lib.service` and you lose both ways - if the tmpfs lands
  first the condition skips your unit without a word, and if yours lands first
  the tmpfs mounts over it. `/home` escapes only because it is not under a
  volatile path. Anything nested under `/var/lib`, `/var/cache`, `/var/spool`
  or `/srv` needs its own unit with `After=var-volatile-<x>.service`.
* **systemd-resolved stays and keeps `/etc/resolv.conf`.** That path is a
  symlink into `/run` recreated by a tmpfiles `L!` line each boot, which is why
  it survives the `/etc` overlay. `10-tessaro.conf` sets
  `dns=systemd-resolved` and `rc-manager=unmanaged` so NM never writes a real
  file there - one that would land in the overlay upper and outlive every
  future image.
* **The NM drop-in lives in `/usr/lib/NetworkManager/conf.d/`, not `/etc`**, for
  the same reason the kiosk's defaults do. It ships in
  `meta-tessaro-distro/recipes-connectivity/tessaro-network/`.
* **`auth-polkit=root-only` is load-bearing for SSH.** `polkit` is in
  `DISTRO_FEATURES` and in NM's `PACKAGECONFIG`, but the image ships no polkit
  *agent*. Upstream's policy grants `settings.modify.system` and
  `network-control` to `allow_active` and demands `auth_admin_keep` otherwise,
  so without this line `nmtui` saves a profile fine from a getty on tty1
  (logind gives it an active seat) and fails over dropbear with "Not authorized
  to modify the system settings". Same command, two answers, depending on how
  the technician got in. `nmcli general permissions` should read `yes`
  throughout.
* **Split packages only.** The plain `networkmanager` package is `ALLOW_EMPTY`
  and `RRECOMMENDS` every plugin built - ppp, wwan, adsl, ovs, bluetooth,
  cloud-setup. The image names `networkmanager-daemon`, `-nmcli`, `-nmtui`,
  `-wifi`. `nmtui` also needs `PACKAGECONFIG:append:pn-networkmanager = " nmtui"`;
  it is not in the recipe's default and pulls `libnewt` from oe-core.
* **`networking-layer`, not `meta-networking`,** is what
  `LAYERDEPENDS_meta-tessaro-distro` names - the layer's `BBFILE_COLLECTIONS`
  value, same trap as meta-webkit registering itself as `webkit`.
* **WiFi drivers and firmware are both per machine, and both already handled on
  the two real targets.** They are separate things: drivers are
  `kernel-module-*` packages, firmware is `linux-firmware*`. `linux-yocto`
  builds the wifi drivers as modules on every machine here - the qemu package
  feed has `kernel-module-brcmfmac`, `-ath9k` and the rest - but a module is
  only *installed* if something recommends it.
  - `raspberrypi3-64`: `rpi-base.inc` adds `kernel-modules` (every built
    module), and `raspberrypi3-64.conf` adds the bcm43430/43455 rpidistro
    firmware. Nothing to do.
  - `genericx86-64`: meta-yocto-bsp's `genericx86-common.inc` adds
    `kernel-modules linux-firmware`. Drivers are complete. Firmware is **not**
    "all firmware": oe-core splits that recipe into 138 packages and
    `FILES:${PN}` is only the catch-all `${nonarch_base_libdir}/firmware/*`,
    so anything a split package claims is absent and nothing pulls it back.
    The line runs through Intel - `-iwlwifi-8265`, `-9260`, `-7260` and the
    other legacy generations are split out, the AX200/AX210/BE200 blobs are
    not and so land in the catch-all. Same for `-ath10k`/`-ath11k` (split,
    missing) vs ath12k (an explicit `RDEPENDS` of the base). So a modern card
    works out of the box and an 8265 or ath10k - common in exactly this class
    of mini PC - binds its driver and finds no firmware. See its kas fragment.
  - `qemux86-64`: neither, and the image ships 15 modules total. Correct -
    QEMU emulates no wireless NIC, so wifi cannot be exercised here at all.
    The first real wifi test has to be on the Pi.
* **`NetworkManager-wait-online.service` *is* enabled** - `preset-all` at rootfs
  time creates `/etc/systemd/system/network-online.target.wants/NetworkManager-wait-online.service`,
  even though `SYSTEMD_SERVICE:networkmanager-daemon` never names it. It is
  inert only because nothing in the image `Wants=` or `Requires=`
  `network-online.target`, so the target is never pulled into a transaction.
  The moment something does - an update agent, a VPN, an MQTT client - that
  unit starts gating boot with `nm-online`'s 30-second default on a link-less
  device. Ship a drop-in from `tessaro-network` capping the timeout at that
  point, and do not add one before, since an override with no consumer just
  rots.
* **Changing systemd's `PACKAGECONFIG` rebuilds most of the image.** Removing
  `networkd` is not an incremental change. Budget a near-full build.

## Gotchas

* **`distro:` has to be set by the entry point of the kas chain.** kas resolves
  a plain scalar by include order, and a file's own value beats the ones its
  includes set. Every machine fragment includes a `meta-moonforge-*` layer
  fragment, which pulls `meta-moonforge-distro.yml`, which says
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
  layout). The 3B+ has 1GB of RAM shared with the GPU and Chromium is far
  heavier than the WPE browser it replaced - expect OOM kills and plan the Pi
  target around that. `GPU_MEM` and `VC4DTBO` (fake KMS by default on this
  machine) are the first knobs; both are noted in the fragment and left at
  meta-raspberrypi's defaults.
* **The Pi needs a cross-built watchdog container before it can be built at
  all.** `mise run watchdog-image` is a plain `docker build` on an x86_64 host,
  so the archive it exports into the recipe is amd64 and podman on the Pi
  cannot start it - a failure that would only show up as a crash-looping unit
  on the device. The task therefore refuses any non-x86_64 `TESSARO_MACHINE`,
  and `build` depends on it, so `mise run build-rpi` stops with an explanation.
  Lifting it means `docker buildx build --platform linux/arm64` plus qemu-user
  binfmt on the build host, and an emulated `bundle install` is slow. Do it as
  part of the first real Pi attempt, not before.
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
  in an upstream layer silently stops applying, with no warning. There are
  currently zero such lines; re-check after a Moonforge bump or when enabling a
  new layer. Fix if needed: `DISTROOVERRIDES =. "moonforge:"` in `tessaro.conf`.
* **Artifacts are named `...-qemux86-64-0.*`** because the kas fragment sets
  `IMAGE_VERSION: "0"`. The stable symlink is `...-qemux86-64.rootfs.*`, and
  the mise tasks depend on that `.rootfs` spelling.
* **Moonforge's `STRUCTURE.md` is stale in places** - e.g. it documents the
  kiosk browser as Cog with `WAYLAND_COG_LAUNCH_URL`, while the layer actually
  ships `wpe-simple-launcher` with `WPE_SIMPLE_LAUNCHER_URL`. Tessaro runs
  Chromium through its own units, and neither variable is involved. Trust the
  layer sources over upstream docs.

## Status

Three targets, one fragment each in `kas/machine/`. All three carry the same
image: read-only rootfs, overlayfs `/etc` on `/data`, Weston and the Chromium
kiosk.

| Machine | Purpose | State |
| --- | --- | --- |
| `qemux86-64` | development, boots through `mise run run-vnc` | builds and boots |
| `genericx86-64` | shipping x86_64 hardware (UEFI) | configured, never built end to end |
| `raspberrypi3-64` | Raspberry Pi 3 Model B+ | configured, cannot be built yet - `watchdog-image` refuses non-x86_64 until the buildx cross build exists; Chromium will likely OOM on 1GB anyway |

"Configured" means the kas chain resolves and bitbake parses it with the right
`DISTRO`/`MACHINE`/`WKS_FILE`; neither image has been built or booted on real
hardware yet. Expect the first build of each to surface fetch or packaging
issues that parsing cannot.

Writing an image to a card or disk is deliberately not a mise task - decompress
with `mise run unpack` and `dd` the `.wic` yourself.
