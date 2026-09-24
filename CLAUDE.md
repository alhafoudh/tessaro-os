# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Yocto/OpenEmbedded build for **Tessaro**, a web kiosk, producing Linux images
for the platforms it ships on. Derivative of
[Moonforge](https://moonforgelinux.org/).

Obsidian tracking: no

**Do not count the things you list.** Anywhere in the repo - code comments,
docs, user-facing strings, test names, this file - never announce or refer
to a set of items by how many there are, spelled or numeric: not "the
following three items", "the four profiles", "these two flags", "all three
land in `/usr/bin`". The count goes stale the moment an item is added or
removed, and nothing catches it. Name what the items are instead ("the
managed profiles", "these flags", "every binary"). Numbers that are facts
rather than a count of listed things - timeouts, sizes, retries, a
16-character password, measured line counts - are fine.

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
| `mise run image-sizes` | Size of every built `.wic`, all machines at once |
| `mise run clean` | Drop build artifacts, keep sstate and downloads |
| `mise run agent-test` | `cargo test` for the whole agent workspace |
| `mise run agent-lint` | `cargo fmt --check` plus clippy for the workspace |
| `mise run agent-integration` | The agent against a real headless Chromium (`agent/compose.yaml`, needs docker compose), control plane in a sandbox |
| `mise run build-ctl` | Release `tessaro-ctl` for this host, to manage devices remotely |
| `mise run agent-e2e` | Boot qemu VMs (E2E_JOBS at a time), provoke each agent behaviour, assert on its journal |
| `mise run agent-e2e:one` | One lane or case of that suite, with plain rspec |
| `mise run agent-e2e:setup` | `bundle install` for the suite's gems |
| `mise run image:pull` | Workstation: fetch the image and bmap from the build host |
| `mise run image:update` | Workstation: pull the image and update a running device over the network (the normal path) |
| `mise run image:flash` | Workstation, manual: write the pulled image to a card or disk (first install, recovery) |
| `mise run tunnel` | Workstation: autossh VNC/SSH forwards to the build host |

Exit the QEMU serial console with `Ctrl-a x`.

**Every task acts on one machine**, `$TESSARO_MACHINE`, defaulting to
`qemux86-64` - `image-sizes` is the one exception, since comparing the targets
is the whole point of it. The `build-*` tasks are one-line wrappers that set
it; anything else takes it from the environment:

```sh
TESSARO_MACHINE=raspberrypi3-64 mise run shell
```

`mise.toml` derives `KAS_CONFIG`, `KAS_BUILD_DIR` and `WIC` from that one
variable, so each machine gets its own TOPDIR under `build/<machine>/` while
`cache/` (`DL_DIR` + `SSTATE_DIR`) stays shared. Valid values are exactly the
basenames in `kas/machine/`. `run`, `run-vnc` and `agent-e2e` refuse any
machine but `qemux86-64`, and `build` adds OVMF only there. `build` has no
prerequisites beyond kas.

The agent is an ordinary Rust workspace under `agent/`, built into the image by
the `tessaro-kiosk` recipe: `protocol/` (the wire types and the settings
registry, no Linux-only dependencies), `tessaro-agent/` (the device side),
`tessaro-ctl/` (the client, on the device and on a laptop) and `update/`
(image updates: a library the agent stages with, and `tessaro-flash`, which
applies them from the initramfs). cargo.bbclass installs every binary in the
workspace into `/usr/bin`; `tessaro-flash` is split into its own
`tessaro-kiosk-flash` package so the initramfs does not pull the agent in. The no-
blocking `clippy.toml` lives in `tessaro-agent/` only: `tessaro-ctl` is a plain
blocking client on purpose. Its tests run on the host (`mise run agent-test`);
mise pins the host toolchain to **rust 1.95.0**, the same version bitbake uses,
because that version is dictated by the Chromium pin
(`meta-lts-mixins-rust` in `kas/repo/meta-chromium.yml`) and not chosen freely.
`mise.toml` also points `CARGO_TARGET_DIR` at `build/cargo-target`: the recipe
fetches `agent/` with a `file://` SRC_URI, so a `target/` inside it would be
copied into `WORKDIR` and hashed on every build.

**Command-line output is colored, and any new CLI must be too.** `tessaro-ctl`,
and every future human-facing tool in `agent/`, prints through anstream's
`println!`/`eprintln!` (imported at the top of the file to shadow the std
macros) with the semantic palette in `tessaro-ctl/src/style.rs`: `LABEL`,
`HEADING`, `OK`, `WARN`, `BAD`, `MUTED`, `SECRET`, `CMD`, `SOURCE`. Reuse that
palette rather than picking raw colors at a call site. anstream drops the
codes by itself when the stream is not a terminal, under `NO_COLOR` or
`TERM=dumb`, and with `--color never`, so no call site ever checks. The
rules: styles decorate text and never change it, so the plain output stays
parseable; `--json` output is never styled; and aligned columns use
`style::pad`, which pads *inside* the escape codes, because `{:<N}` around a
painted string counts the escape bytes. anstream and anstyle were already in
the lock through clap, so this cost no new crates. The exception is
`tessaro-flash`, which prints plain text to the initramfs console.

**tessaro-ctl command and key structure.** The tree is always `tessaro-ctl
<group> <command>`, and a setting key starts with the group that acts on the
same thing. Keep to these rules when adding a command or a setting:

* **No flat top-level commands besides `completion`, and no hidden
  aliases.** Each action has one spelling, so docs, hints and scripts cannot
  drift apart. The one exception is the `files` group, whose commands mirror
  the shell's: `list` also answers to `ls` and `dir`, `move` to `mv`. Those
  are visible aliases (`visible_alias`, shown in `--help`), and docs and hints
  still write the full name.
* **Groups are nouns, split by what the command acts on**, not by how it is
  implemented:
  * `device`: the device itself. Its identity, current state, journal,
    `ping` to it, restart and reboot, factory reset.
  * `access`: who may manage it. Claim, login, unclaim, tokens, root password.
  * `ssh`: shell access by key. `connect` and `keys list|revoke`.
  * `config`: the settings registry. `keys`, `get`, `set`, `unset`.
  * `network`: the device's network link. Addresses, interfaces, profiles,
    WiFi, `ping` from the device, speed test.
  * `storage`: the disk the device runs from. Partitions, free space,
    growing `/data`.
  * `screen`: the physical display. Screenshot, modes, confirming a mode.
  * `browser`: what the browser shows. Navigate, maintenance, debug screen.
  * `audio`: sound. Which output plays and which input records, volume,
    mute, a test tone and a recording level.
  * `update`: putting an image on the device.
  * `files`: the file store in `/data/files`. Upload, download, sync, list,
    move, rm.
  * `nodes`: this client's own view (discovery, known devices). Needs no device.
* **Commands are verbs or short nouns**, and the same verb means the same thing
  in every group: `list`, `create`, `revoke`, `set`, `show`, `status`, `cancel`.
* **A group's bare name does nothing**; it prints its help. Overviews are an
  explicit `show` or `status` (`network show`, `network wifi status`).
* **Nest a third level only for a collection with its own verbs**
  (`access token create|list|revoke`, `ssh keys list|revoke`,
  `network profiles list|show`). Otherwise use two levels.
* **A new command goes into an existing group.** Add a group only when at least
  two commands would share it and none of the existing groups fits; a lone
  command goes to the nearest group.
* **Destructive commands take `-y/--yes`** and live in the group their effect
  belongs to (`device factory-reset` wipes the device, `access unclaim` only
  removes its owners).
* **Setting keys are prefixed by the command group that acts on the same
  thing**: `browser.*`, `screen.*`, `audio.*`, `network.*`, `device.*`, `access.*`. A key
  that no command group matches is named after the component it tunes
  (`agent.*`), and `data.*` is the user's namespace. A sub-feature with its own
  on/off gets a third level that mirrors its command (`browser maintenance on
  --url` goes with `browser.maintenance.enable` and `.url`), as do the network
  profiles' keys (`network.ethernet.*`, `network.wifi.*`).
* **Renaming a key means adding it to `RENAMED`** in `agent/protocol/src/keys.rs`,
  so devices in the field migrate at boot (see **Migration** below), and a
  `git grep` over the whole repo, placeholders in templates included. The
  `KIOSK_*` env names do not follow the keys and never need to move.
* **Every command name in a user-facing string is the full path**
  (`tessaro-ctl screen confirm`): in the ctl, the agent's hints, key docs and
  pages alike.

`tessaro-ctl completion bash|zsh|powershell` prints a completion script. The
bash one is patched after generation: clap_complete 4.6 escapes the dash in
`tessaro-ctl` two ways, and nothing below the first word completed until
`completion()` in `main.rs` unified them (a test guards it).

After changing any `Cargo.toml` in the workspace or `agent/Cargo.lock`,
regenerate the crate list the recipe requires and commit it:

```sh
mise run shell                          # then, inside:
bitbake -c update_crates tessaro-kiosk  # writes tessaro-kiosk-crates.inc
```

`do_compile` runs `cargo build --frozen` with no network, so `Cargo.lock` and
`tessaro-kiosk-crates.inc` have to agree or the build fails.

For Yocto work on a single recipe, go through the kas shell so bitbake sees the
right environment:

```sh
mise run shell                          # then, inside (cwd is /build):
bitbake -e <recipe> | grep '^VAR='      # resolved value of a variable
bitbake -c cleansstate <recipe>         # force a rebuild of one recipe
bitbake -c devshell <recipe>            # shell in the recipe's build dir
bitbake -n moonforge-image-base         # dry run: what would rebuild
```

Builds are long. Run them in a Herdr pane, not the Bash tool.

**Rules for builds and worktrees** (for Claude):

* **One build at a time, across every checkout.** Before starting any
  `mise run build*` (or `shell` with bitbake, or `agent-e2e`), check nothing
  else is building: `pgrep -af 'kas-container|bitbake'` must come back empty,
  worktrees included. If something is running, say what and wait, or ask -
  never start a second one next to it.
* **A worktree is always a Claude-managed one**, made with the `EnterWorktree`
  tool (or `claude --worktree`), which puts it under `.claude/worktrees/<name>`
  (gitignored). Never `git worktree add` by hand or anywhere else: the cache
  link below, the cleanup and the build rules all assume that location.
* **A new worktree gets the cache symlinked the moment it is created**, even
  for cargo-only work, so nothing ever re-downloads the ~40G cache:

  ```sh
  ln -s ../../../cache .claude/worktrees/<name>/cache
  mkdir -p .claude/worktrees/<name>/build   # kas-container's mkdir is not recursive
  ```

  `mise.toml` derives `DL_DIR`/`SSTATE_DIR` from `{{config_root}}`, so without
  the link a worktree starts from an empty cache.
* **Never start a build from a worktree on your own.** Prepare the changes,
  then ask; the user decides whether and where it builds. Host-side cargo
  (`agent-test`, `agent-lint`) is fine.

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
   fstypes and the `IMAGE_NAME`/`IMAGE_VERSION_SUFFIX` scheme.

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

## Kiosk browser

**Chromium** (147, `chromium-ozone-wayland` from meta-browser's `meta-chromium`
layer) fullscreen on Weston. It replaced cog/WPE: Chromium is the only browser
that will ever support the WebBluetooth/WebSerial/WebUSB APIs on the roadmap,
and CDP gives the agent a real health channel where cog's D-Bus surface was
write-only. The cost is footprint - memory is tight on the Pi 3B+'s 1GB -
and a full build takes hours.

The layers arrive through `kas/repo/meta-chromium.yml`, which pins meta-browser
(repo root is not a layer, so `layers:` is mandatory, same pattern as
`kas/repo/meta-yocto.yml`), plus its dependencies: meta-clang and the Rust
mixin, a *second* checkout of meta-lts-mixins on its `scarthgap/rust` branch
(Moonforge pins the same repo on `scarthgap/u-boot`). That mixin is also what
builds `tessaro-agent`, so the agent's rustc version is Chromium's to choose.
meta-moonforge-wpe and meta-webkit are gone entirely; `/home` on
`/data/overlay-home` is now ours
(`meta-tessaro-distro/recipes-core/volatile-binds/volatile-binds.bbappend`), and
Weston/wayland/polkit come from including `meta-moonforge-graphics` directly.

Everything else is `meta-tessaro-distro/recipes-browser/tessaro-kiosk/`:

* `tessaro-kiosk.service` runs Chromium as the `weston` user, with CDP on
  `127.0.0.1:9222` and the profile on `/data/kiosk/chromium`.
* `tessaro-agent.service` runs `/usr/bin/tessaro-agent`, the native binary
  built from `agent/` by this same recipe. `Type=simple`, `User=root`,
  `Restart=always`, and `WatchdogSec=60` - see **The agent's deadlines and
  watchdog** below. It is also the device's control plane - see **Settings,
  tessaro-ctl and the claim model** below.
* `tessaro-config.service` is a oneshot, `tessaro-agent boot`, ordered before
  Weston, the browser and the agent. It renders the device's settings, keeps
  the claim invariant and is the factory-reset escape hatch.
* `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` carries the build-time defaults
  (`TESSARO_KIOSK_URL` from `tessaro.conf`). What was set on the device lives
  in `/data/tessaro/state.json` and reaches the units as
  `/run/tessaro-kiosk/generated.env`, which the browser unit and
  `tessaro-weston-config` read *after* the defaults. The agent reads only the
  defaults and lays `state.json` over them itself. There is no
  `/etc/default/tessaro-kiosk` any more.

The defaults deliberately live under `/usr/lib`, not `/etc`: `/etc` is an
overlayfs upper on `/data`, so the first write to a file there shadows the
image's copy permanently and no later image could move the default again.

Change a setting with `tessaro-ctl config set KEY=VALUE`; it restarts exactly what
reads that key.

The agent used to be a Ruby program in a podman container. It is now a Rust
binary, and podman is gone from the image with it - see **Removing the
container runtime** below for what that cost. The behaviour did not change: it
is the same state machine, the same environment variables and the same log
lines. It watches the browser over **CDP** (`http://127.0.0.1:9222`), on one
persistent session found through `/json/list`: a `Runtime.evaluate` round
trip proves the *renderer* is alive, `Page.navigate` replaces the old D-Bus
`open` action, and the offline
page is served as `file:///run/tessaro-kiosk/index.html` (the `tessaro://`
dir-handler died with cog). The restart path is `org.freedesktop.systemd1`
`RestartUnit` over the system bus - no `systemctl` shell-outs. Same state
machine as before: probe cadence vs navigation cadence, fail threshold before
the offline page, one restart per outage, backoff, `nav_state=unknown` after a
browser restart.

Things to know:

* **The CDP port is on the loopback because the binary hardcodes it**, not
  because of a flag. `--remote-debugging-address` used to be in the unit and
  never did anything: that switch exists only in `content_shell`, and
  `chrome/browser/devtools/remote_debugging_server.cc` binds `127.0.0.1` with a
  `::1` fallback unconditionally. There is nothing to widen and nothing to get
  wrong. Recent Chromium does refuse to open the DevTools port with the
  *default* user-data-dir, so `--user-data-dir` must stay set.
* **The wrapper contributes only `--kiosk --no-first-run
  --ozone-platform=wayland`**, pinned by `tessaro.conf`. Everything else is in
  `tessaro-kiosk.service`. Keep it that way - flags in two places is how
  `--incognito` went unnoticed for as long as it did.
* **`--incognito` is gone, and how it was removed matters.** The `kiosk-mode`
  PACKAGECONFIG selects `--kiosk --no-first-run --incognito` as one bundle,
  and incognito threw away cookies, `localStorage` and service worker caches
  on every restart. The tempting fix - drop `kiosk-mode` and pass the good
  flags from the unit - costs a **full Chromium rebuild**: `PACKAGECONFIG` is a
  direct vardep of `do_configure` even for an option that expands to nothing,
  so removing it changes that basehash and everything downstream. Measured with
  `bitbake -S printdiff chromium-ozone-wayland`, which names the culprit
  outright (`Variable PACKAGECONFIG value changed: ... [-kiosk-mode-] ...`).
  So `kiosk-mode` stays and `tessaro.conf` overrides
  `CHROMIUM_EXTRA_ARGS:pn-chromium-ozone-wayland` instead - that variable is
  only read by a `sed` in `do_install`, so the cost is do_install onward. The
  recipe's own `:append` of `--ozone-platform=wayland` still lands after our
  value, which is why it ends up in the wrapper next to ours. **Run that printdiff
  before touching either line.**
* **Some flags are consequences of losing incognito, not preferences.**
  `--hide-crash-restore-bubble`, or an unclean shutdown puts a "Restore pages?"
  bubble on a public screen; and `--disk-cache-size`, because the profile now
  grows on `/data`.
* **Flags an operator may need are variables, not constants.**
  `KIOSK_CHROMIUM_ARGS_EXTRA` (unbraced `$VAR` in `ExecStart`, so systemd
  splits it at whitespace), `KIOSK_TOUCH`, `KIOSK_ENABLE_FEATURES`,
  `KIOSK_DISABLE_FEATURES` and `KIOSK_FPS_ARGS` (rendered from
  `browser.fps_counter`) all come from the same env files as everything else.
  Rules worth keeping: a flag only belongs in the unit's fixed set if
  `KIOSK_CHROMIUM_ARGS_EXTRA` can *counter* it - `--disable-pinch` is not in
  it because Chromium 147 has no `--enable-pinch` - and every `base::Feature`
  goes through `KIOSK_ENABLE_FEATURES` or `KIOSK_DISABLE_FEATURES`, because
  duplicate `--enable-features`/`--disable-features` switches do not merge
  and the last one silently wins. The IME switches are the one documented
  exception to the counter rule - see **On-screen keyboard**.
* **`--disable-crash-reporter` was a no-op too.** It is defined only in
  chromecast and headless, never in the chrome binary. The switch that works is
  `--disable-breakpad`.
* **Chromium runs under `dbus-run-session`, and that is not cosmetic.**
  `DBUS_SESSION_BUS_ADDRESS` is unset on this image, so libdbus falls back to
  *autolaunch*, which needs X11 - hence `Could not parse server address:
  Unknown address type` rather than a plain connection failure, 18 error lines
  at every start. Browser flags do not fix it: `--password-store=basic` only
  covers the keyring, and Chromium probes that bus for several other services.
  Giving it a private session bus takes those 18 lines to 0 (measured). The
  wrapper becomes the unit's `MainPID`, which is fine - the agent compares
  whether the main pid *changed*, not which binary it is - and it exits when
  the browser exits, so `Restart=` still behaves.
  `--password-store=basic` and `--disable-background-networking` stay as
  policy: no keyring, no component updater or variations fetches on a link
  that may be metered. Neither silences the single GCM `DEPRECATED_ENDPOINT`
  line at startup, which is harmless and left alone. Note this is only the
  *session* bus - Web Bluetooth talks to `org.bluez` on the **system** bus,
  which `dbus-run-session` does not touch.
* **`proprietary-codecs` is what plays H.264.** The marketing site's videos
  will not play without it; it is enabled in `tessaro.conf`.
* **Chromium has no D-Bus control interface at all.** Everything is CDP. The
  control-plane D-Bus policy that existed for cog is gone with it.
* **Supervising the browser needs only a handful of paths**: the system bus
  socket (`/run/dbus/system_bus_socket`, for RestartUnit), `/run/tessaro-kiosk`
  (it stages the offline page there), and `/data/kiosk` plus
  `/usr/share/tessaro-kiosk` as page sources. These used to be bind mounts
  into a container and are now just paths. The control plane uses many more
  (`/data/tessaro`, the policy, NetworkManager's `/run` keyfiles, `/root/.ssh`
  and so on); `paths.rs` has them all.
* **Diagnostics are journal-only** by design; nothing technical reaches the
  screen. Both halves log the same way now:
  `journalctl -fu tessaro-agent` and `journalctl -fu tessaro-kiosk`.
* **Chromium policy lives in `/etc/chromium/policies/managed/10-tessaro.json`,
  and that path is not a choice.** It is compiled into the binary
  (`components/policy/core/common/policy_paths.cc`, the non-branded branch), so
  this is the one piece of product configuration that cannot follow the
  `/usr/lib` rule above. Shipped in the image it is the `/etc` overlay's lower
  layer and works; the first on-device write to it shadows it for good. The
  loader accepts `//` comments and trailing commas
  (`JSON_PARSE_CHROMIUM_EXTENSIONS`), so the file documents itself. A syntax
  error drops the **whole file** with one `SYSLOG(WARNING)` and carries on, so
  confirm on `chrome://policy` after editing rather than assuming.
  `TranslateEnabled: false` is what stops the translate bubble - there is no
  `--disable-translate` switch in 147 any more.
* **`/data/kiosk` is root owned and only `/data/kiosk/chromium` is `weston`.**
  Both come from tmpfiles. The split matters: `/data/kiosk/offline.html` is one
  of the pages the agent stages and puts on screen, so a weston-owned parent
  would let the browser user rewrite the page it is being shown - the same rule
  that keeps `/run/tessaro-kiosk` root owned. Chromium only needs its
  `--user-data-dir` writable. `d` lines re-apply owner and mode every boot, so
  a device built before this heals itself.
* **The agent enforces the kiosk *origin*, not the kiosk URL.** Every healthy
  cycle reads the page's current URL - free, since the CDP session keeps it
  current from `Page.frameNavigated` - and navigates back if the scheme/host/port
  differs from the origin the last navigation to `KIOSK_URL` landed on,
  logging where it had gone. A redirect right after a navigation is adopted
  as the kiosk origin (logged once, `drifted_origin` in `agent.rs`), never
  treated as drift; a page with no origin, such as `about:blank`, is. Same-origin
  sub-pages, query strings and in-page routing are deliberately left alone:
  matching the whole URL would fight the site and loop on any redirect.
  `KIOSK_ENFORCE_ORIGIN=0` turns it off for a site that legitimately hands
  visitors to another host, and it is inert anyway when `KIOSK_URL` is
  `data:`/`file:` (no origin to compare). A browser that will not say where it
  is counts as "cannot tell", never as drift - otherwise a page caught
  mid-navigation would be yanked back every cycle.
* **Navigations log at info.** `navigated to <url>`, the offline page with its
  URI, the drift line, and `chromium is answering again after N failed checks`
  are all info, because "what is on screen right now" is the question anyone
  reading this journal actually has. At the default 600s refresh that is one
  line per ten minutes.
* **CDP failures are quiet until the browser has answered once.**
  `tessaro-kiosk.service` is `Type=exec`, so systemd calls it started the
  moment `dbus-run-session` (which then starts `/usr/bin/chromium`) is
  exec'd - seconds before Chromium opens its
  DevTools port. The agent's `After=` on it therefore guarantees nothing, and
  the first cycle after every boot finds port 9222 closed. Logging that at
  info put two lines that read like faults into every device's journal on
  every boot, so `report_cdp_failure` in `agent.rs` holds them at debug until
  the `seen_alive` flag flips. Nothing real is hidden: the escalation still
  logs its restart at info. Do not "fix" this by making them unconditional.
* **A slow-starting browser gets restarted once.** A failed navigation bumps
  `ping_fails` as well as a failed liveness check, so the default
  `KIOSK_PING_FAILS=3` is really reached in the second cycle, not the third -
  one `KIOSK_PROBE_INTERVAL` (30s) after the first failed one, plus the CDP
  timeouts. That is fine on x86; on the Pi, where a
  cold first start could plausibly take longer, expect one spurious restart
  and raise `KIOSK_PING_FAILS` there rather than reworking the counter.
* **The agent degrades gracefully without a system bus**: every `Units` method
  answers as if the unit were stopped and `restart` returns an error, so the
  same binary runs against `mise run agent-integration` (no bus) and on a
  device. A browser restart is still noticed there: the CDP session bumps a
  generation counter when it comes up on a new page target (or the same one
  after a crash), which the state machine treats exactly like a changed
  `MainPID`. Before the session existed, with no bus, it was invisible. A
  reconnect to the *same* live page deliberately does not bump it - that once
  turned an agent stall into a pointless reload of a public screen.
* **TLS is openssl, through native-tls used directly, and getting it wrong is
  quiet.** The HTTP client is hyper's low-level one, not reqwest (which would
  add ~48 crates, mostly the ICU tables behind `url`'s IDNA, to redo the
  redirect walk and origin parsing the agent already has). `native_tls` uses
  openssl's default verify paths - the image's `/etc/ssl/certs` from
  `ca-certificates` - so there is no compiled-in root store to pick by
  mistake. A missing provider is a link error now; the *silent* failure left
  is an https URL that never takes the TLS path and fails like a dead site.
  `agent/tessaro-agent/src/http.rs` has a test for each, and `readelf -d` on
  both built binaries should always show `libssl.so.3`. Not rustls: it would freeze the
  roots at build time, and `ring`/`aws-lc-rs` want a C toolchain and per-arch
  asm under bitbake. The one exception is the speed test - see **Speed test**.

### The agent's deadlines and watchdog

**Every wait on the outside world has a deadline, and systemd watches the
agent itself.** Before this, no D-Bus call had a timeout at all (and
`main_pid()` runs every cycle), and DNS ignored the agent's own connect
timeout, so a wedged agent could keep its unit `active` while the kiosk went
unsupervised.

* **`deadline::within` is the only timeout in the program.**
  `agent/tessaro-agent/clippy.toml` refuses `tokio::time::timeout` and
  `timeout_at` everywhere else, so every deadline names its
  call in the journal (`the system bus did not answer within 5s`). A
  source-grep test in `deadline.rs` requires every `.await` in the adapters to
  be under a `within(..)`, a method of the same adapter, or a
  `// naked: <reason>` comment - the only check that reaches the zbus calls,
  which `#[zbus::proxy]` generates and clippy cannot name. Put a `// naked:`
  comment on its own line above the statement: rustfmt moves one that trails
  a `{` into the block, where it no longer annotates anything.
* **The runtime is current-thread on purpose.** A stray blocking call stalls
  the watchdog keepalive along with everything else, systemd restarts the
  agent, and the journal says why - so the watchdog is a standing test of the
  no-blocking-calls rule. `clippy.toml` also refuses `std::thread::sleep` and
  the blocking `std::net` connect and DNS calls. zbus runs fine on it; its
  `rt-multi-thread` tokio feature is only compiled in.
* **The watchdog is fed by pledges, not by a timer.** Before every external
  call, `Heartbeat::within` publishes when that call will be over; the
  keepalive task pings only while the pledge holds, and logs
  `watchdog: <call> is Ns overdue` once before it stops. A timer would keep
  pinging through a wedge; pinging once per cycle would tie `WatchdogSec` to
  the probe timeouts, which are settings. So `WatchdogSec=60` is
  independent of them, a wedge is caught in about 75s, and waiting between
  cycles - including the `KIOSK_AGENT_ENABLE=0` park - pledges too, so idle
  is alive. A pledge is capped at 120s; a configured timeout above that is
  called out at startup.
* **`WatchdogSec=` alone is enough on `Type=simple`**: systemd 255 sets
  `NotifyAccess=main` whenever a watchdog is configured. `sd_notify` is
  hand-rolled in `notify.rs` (one datagram; no libsystemd). A false alarm
  costs one page reload - killing the agent does not touch the browser.
  `KIOSK_WATCHDOG=0` keeps pinging without judging the pledges; `WatchdogSec=0`
  in a drop-in turns it off. Check it with
  `systemctl show tessaro-agent -p NotifyAccess,WatchdogUSec,WatchdogTimestamp`.
* **DNS is bounded by glibc, not only by us.** `lookup_host` is `getaddrinfo`
  on a blocking thread; the deadline frees the agent, and the thread finishes
  within glibc's `timeout:5 attempts:2`. That holds because `nsswitch.conf`
  has no `resolve` module - adding nss-resolve would put an unbounded call
  behind it. A resolver that swallows queries is reported as
  `DNS did not answer within 5s`, not as a slow site.
* **The CDP session reconnects on its own and logs only at debug** (except a
  failed `DeviceAccess.enable` when `agent.device_access` is on). Every
  command is under `KIOSK_CDP_TIMEOUT`, and a websocket ping every
  `KIOSK_CDP_PING` seconds tears down a half-open socket after two go
  unanswered - counted, not timed, so the agent's own stalls are not blamed on
  the browser. The old
  one-connection-per-command client could not hang on one, and this must not
  either. **A crashed renderer does not end the session**, and that is
  measured, not a preference: a sad tab answers every command with `Target
  crashed` - `Page.enable` included, so priming a new session against it fails
  every time - except `Page.navigate`, which brings it back. So the session
  stays up, bumps the generation, the agent re-navigates, and the tab reloads in
  seconds instead of the browser being restarted. None of it works without
  `Inspector.enable`, which is what delivers `Inspector.targetCrashed` at all.
  A detached target does end the session. The agent waits for the
  session's first attempt before its first cycle, so an agent restart does
  not report a healthy browser as silent.

### The agent's end-to-end checks

`mise run agent-e2e` boots the qemux86-64 image and runs the RSpec suite in
`test/e2e/spec/` against it: each case provokes one thing the agent
exists to handle and asserts on the lines it writes to its journal - the
agent arming its watchdog and navigating at startup, systemd deriving
`NotifyAccess=main` and receiving the pings, the site
going down, the page wandering off the origin, a crashed renderer, a killed
browser, a wedged browser, an operator-stopped unit, a DNS server that
swallows queries, the agent itself wedging, a short agent stall, a parked
agent, SIGTERM - and the control plane: a `tessaro-ctl config set` that restarts the
agent onto the new value, the debug screen and maintenance mode each on and
off with the browser left running, a claim and unclaim round trip, an ssh key
authorized with a pinned host key, then revoked and cleared by unclaim, a resolution
change that refuses an unoffered mode and reverts unconfirmed, a file
store sync that the page then fetches from `/files/`, the
managed network profiles as the boot renders them, a static Ethernet address
that is committed and switched back to DHCP, one that cuts the VM off and is
rolled back by the device alone, one whose agent is killed half way and is
rolled back at its restart, the hotspot password following a claim and an
unclaim, the hotspot's NAT table, and `device ping` and `network ping` with and without
ping sockets. Then sound, on two emulated cards QEMU records to WAV files on
the host: the sound server running as weston, the test tone moving between
the cards with nothing restarted, volume and mute reaching PipeWire and put
back after an agent restart, a kind that is not plugged in falling back to
auto, Chromium's Web Audio playing through PipeWire, and the microphone
granted to the self-test page. On a VM whose disk is larger than the image,
`storage show` reporting the space past `/data` and `storage grow` giving it
to `/data` online. And, each on a VM of its own because each reboots it, image
updates of the image it booted from: damaged staging refused at boot with
nothing written, an update that keeps the settings, one with `--wipe-data`,
and one with `--repartition` that rewrites the whole disk from RAM; and a
grown `/data` that mounts again at the next boot. A lane gets the larger
disk with `extra_disk:` on its describe (`spec/support/vm.rb`), which boots
a grown sparse copy of the image and leaves the image itself as built. About
twelve minutes with three VMs at a time, twenty with one. Exits 1 on a
failing case, printing the journal lines it saw under the failure, and 2
when it cannot start at all (no image).

Running it: `mise run agent-e2e:setup` once (rspec and parallel_tests, in
`test/e2e/Gemfile`), then `mise run agent-e2e`. `E2E_JOBS=N` is how many VMs
run at once (3 by default, 1 for one at a time); `mise run agent-e2e:one --
spec/network_spec.rb -e ping` runs one lane or case with plain rspec, with
paths relative to `test/e2e`, and `mise run agent-e2e:one -- --only-failures`
reruns what failed last time (from `build/e2e/rspec-status.txt`). `E2E_VERBOSE=1` prints each step of a case as it
starts - guest commands, journal waits and what matched, CDP calls,
deliberate sleeps - and `2` adds every agent journal line a wait sees. Every
lane's steps go to `build/e2e/<lane>.log` whatever the verbosity, and its
console to `build/e2e/<lane>.qemu.log`. `E2E_KEEP=1` leaves a VM up after its
lane, `E2E_REUSE=1` runs against one already up on worker 0's ports, and
`-o '--tag ~reboot'` leaves out the update lanes. Output is live, each line
prefixed with its worker, and after every case one `== progress 12/30, 1
failed, 6:03 elapsed, ~9 min left` line covers the whole run: the workers
meet in `build/e2e/progress/` (`spec/support/progress.rb`), which
`mise run agent-e2e` empties before they start.

* **The suite never builds the image.** `mise run build` does, in its own
  pane; the suite refuses to start without a `.wic` and warns when the image
  is older than anything under `agent/` or `meta-tessaro-distro/`, since an
  old agent passes and proves nothing. It is a warning so an older image can
  still be tested on purpose.
* **A spec file is a lane, and a lane is a VM.** The cases of a file run in
  order (`config.order = :defined`) and leave state behind for each other,
  so the shared context in `spec/support/booted_vm.rb` boots a fresh VM
  before a file's first case and powers it off after its last. That is per
  file, not per worker: parallel_tests runs several files one after the other
  on a worker, and a lane must never inherit another's VM. A new case goes
  into the lane whose state it fits; a case that reboots goes into a file of
  its own, tagged `:reboot`.
* **Every worker has its own ports**, from `TEST_ENV_NUMBER`: SSH `2222+10n`,
  telnet `2323+10n`, API `7400+10n`, the CDP tunnel `19222+10n`
  (`spec/support/ports.rb`). Worker 0 has the ports a single VM always had.
  The forwards are fixed in the image's qemuboot.conf, so each worker writes
  its own copy, `tessaro-os-qemux86-64.e2e-worker-N.qemuboot.conf`, into the
  deploy directory and passes it to runqemu *after* the `.wic`. runqemu forces
  those choices: it derives a conf from the image argument and only a
  later conf argument replaces it, and it takes the conf's own directory as
  `DEPLOY_DIR_IMAGE`, where it looks for the kernel and OVMF. runqemu moves a
  forward whose port is taken and logs `Port forward changed`; the harness
  fails on that line rather than drive another worker's VM.
* **Cleanup is hooks and `ensure`.** `:reconfigure` on a case puts the test
  settings back and waits for a settled agent after it, whatever happened;
  cleanup particular to one case stays in its `ensure`, which runs first.
* **Steps come from the helpers, not from the cases.** `Guest#run`,
  `Journal#wait_for`/`refute` and `Cdp#command` write one line each via
  `AgentE2E.step` (`spec/support/output.rb`). Plumbing (cursors, journal
  reads, unit properties) and every polling loop run inside `quietly`, with
  one `step` naming the wait before it, or the log would get a line per poll.
  A sleep that is part of what a case proves is `pause SECONDS, "why"`. A new
  case gets its steps for free; a new polling loop has to be wrapped.

* **The VM has sound cards `mise run run` does not.** `support/vm.rb` passes
  runqemu `qemuparams=` for an Intel HDA line out and a USB audio device,
  each on a `-audiodev wav` recording to
  `build/qemux86-64/e2e-worker-N.{jack,usb}.wav`, so the audio lane reads
  which card a sound came out of from the host. They are not in the kas
  fragment on purpose, or every `mise run run` would write WAV files for as
  long as it ran. QEMU's wav backend cannot capture, so there is no
  microphone in the VM; the mic case only proves the grant.

* **It boots its own VM, not through `mise run run`.** The guest is driven over
  SSH, and runqemu's slirp forwards `127.0.0.1:2222` - but inside the kas
  container's network namespace, where `-p` publishing cannot reach it. The
  harness passes `--network=host` so that loopback is the host's. An
  unclaimed device's empty root password means no credential is involved -
  which is why the suite never leaves the device claimed: the `claim` and
  `hotspot-claim` cases claim, check and unclaim inside one SSH command, with
  a local-socket unclaim in a `trap`; `ssh-key`, which has to log in by key
  while claimed, first starts a guard on the guest that unclaims after a
  timeout if no key gets in.
* **It retunes the agent for the run** with `tessaro-ctl config set --no-apply` over
  the guest's local socket (5s probes, a 15s restart backoff, no periodic
  refresh) and unsets those keys afterwards. The VM runs with `snapshot`, so a
  power-off discards everything anyway. mDNS cannot be exercised here: slirp
  carries no multicast.
* **DevTools is driven from the host** through an SSH tunnel to the guest's
  `127.0.0.1:9222`, with a small websocket client in the script - the image has
  no curl or Python. Chromium's DevTools HTTP server answers HTTP/1.0 with
  nothing at all and holds 1.1 connections open, so the client reads by
  `Content-Length`.
* **The guest's BusyBox has `pgrep` but no `pkill`**, and `pgrep -f` also
  matches the remote shell running the kill, whose command line contains the
  pattern. The harness brackets one character (`--type=rendere[r]`); unbracketed,
  `SIGKILL` ended its own SSH session and `SIGSTOP` froze it.

### Self-test page

**This is what a factory image opens.** `TESSARO_KIOSK_URL` in `tessaro.conf`
defaults to `http://127.0.0.1/`; a deployment repoints it, at build time or
with `tessaro-ctl config set browser.url=...`.

`meta-tessaro-distro/recipes-browser/tessaro-selftest/` ships one static page at
`/usr/share/tessaro-selftest/index.html`, with its media beside it. It exercises
rendering, fonts, emoji, every `<input>` type, touch and mouse scrolling plus
multi-touch, WebSerial and WebHID, audio and video playback, and WebAudio
synthesis - from local files, with the network down. Passive checks grade
themselves in a strip at the top; interactive ones stay `pending` until someone
actually does something. To get back to it on a deployed device,
`tessaro-ctl config set browser.url=http://127.0.0.1/`, and `config unset` it afterwards.

* **It is served by nginx, and that is not a preference.** A `file://` page has
  a null origin, and `SerialAllowAllPortsForUrls` /
  `WebHidAllowAllDevicesForUrls` match on origin and nothing else - so the
  self-test could never be pre-granted a device, and the chooser dialog would
  be the only route. `http://127.0.0.1` is a real origin *and* a potentially
  trustworthy one, so the grants apply and the page is a secure context, which
  is itself a precondition for `navigator.serial` existing at all. The page
  still handles the `file://` case and says what is missing.
* **The policy lists the self-test's origin next to the kiosk's.**
  `TESSARO_DEVICE_ORIGINS` in `tessaro-kiosk_1.0.bb` is `TESSARO_KIOSK_ORIGIN`
  plus `TESSARO_SELFTEST_ORIGIN`, deduplicated - a single entry on a factory
  image where they are the same string. On a customer image the self-test
  entry is what keeps the diagnostic page able to open a serial port on a
  *deployed* device.
  `TESSARO_SELFTEST_ORIGIN` must match the `listen` line in
  `tessaro-selftest`'s nginx conf.
* **nginx serves from `/usr/lib/nginx/conf.d/`, not `/etc/nginx/conf.d/`**, for
  the same reason everything else does: `/etc` is an overlay upper on `/data`.
  `recipes-httpd/nginx/nginx_%.bbappend` adds that include to `nginx.conf` -
  which has to stay in `/etc`, its path being compiled in by `--conf-path` -
  and deletes the stock `default_server` symlink, which would otherwise answer
  on `0.0.0.0:80` with the nginx welcome page. Ours binds `127.0.0.1` only.
* **`tessaro-ctl config set agent.refresh_interval=0` before a manual pass.** The agent
  re-navigates on that timer, 600s by default, and a reload closes any serial
  port the page has open and wipes every form value. Put it back afterwards:
  on a real site the periodic reload is what recovers a stale page.
* **The agent probes the local server**, because `KIOSK_URL` is now http - so
  nginx dying puts the offline page on screen like any other outage, rather
  than going unnoticed.
* **The on-screen keyboard only appears on a device with no keyboard**, so the
  text inputs need a USB keyboard or `screen.osk=always` - see **On-screen
  keyboard** below. Everything that is a tap, a slider or a picker works with a
  finger. The page carries this as a note.
* **It is a separate recipe from `tessaro-kiosk` on purpose.** That recipe
  `inherit`s cargo and builds `tessaro-agent`, so anything added to its
  `SRC_URI` re-hashes `do_fetch` and drags the whole Rust build behind every
  edit to a `<div>`.
* **The video clips are stand-ins**, generated with ffmpeg (H.264 high, 30 fps,
  3 s, 1080p and 4K, AAC audio). Replace them with real footage by dropping
  files of the same names into `files/media/`.

Fonts are part of this story. Until now nothing in the tree named a font
package at all: `liberation-fonts` was the only TTF family in the image and it
arrived as an `RRECOMMENDS` of `weston`, which meant every generic family
resolved to the same Liberation faces and every emoji anywhere on the kiosk was a
tofu box. `moonforge-image-base.bbappend` now installs `ttf-noto-emoji-color`
(NotoColorEmoji, ~10 MB) and DejaVu sans/serif/mono (~2 MB) from meta-oe -
which is why `layer.conf` names `openembedded-layer` in `LAYERDEPENDS`.

### Display scaling

**Chromium cannot scale itself on this stack, so Weston does it.** The obvious
knob, `--force-device-scale-factor`, is inert here: Chromium only honours it
when the compositor advertises `wp_fractional_scale_manager_v1`
(`WaylandWindowManager::DetermineUiScale`, gated on `IsUiScaleEnabled()`), and
Weston 13.0.1 does not implement that protocol server side - the XML in its
tree is used by its own demo clients. Chromium's own ozone-wayland startup even
logs it as "TEST ONLY". Set it and nothing happens.

What is left is `weston.ini`'s `[output] scale=`, which on the DRM backend is
the *only* path: integers only, no fractional scaling, matched against an exact
connector name, with no `--scale` command-line option and no wildcard matching
(`drm_config_find_controlling_output_section`). Since a shipped image cannot
know whether it will be plugged into a 1080p or a 4K panel, or what the
connector will be called, the config is generated per boot:

* `/usr/libexec/tessaro-weston-config` runs as `ExecStartPre=` of
  `weston.service` (drop-in `10-tessaro-scale.conf`, from the `weston-init`
  bbappend), reads every connected connector out of `/sys/class/drm`, and
  writes `/etc/xdg/weston/weston.ini` plus an `[output]` section per connector
  to `/run/weston/weston.ini`. The drop-in then points `weston --config=` at it.
  It also writes `require-outputs=none` into `[core]` and a
  connected-connectors comment (see **Display hotplug**), `[input-method]`
  (see **On-screen keyboard**) and `[screen-share]` (see **Remote access**).
* **Its log is `journalctl -t tessaro-weston-config`, not `-u weston`.** It runs
  as `ExecStartPre=`, and those lines do not come back under the unit even
  though the compositor's own do. Every decision it makes - connector, scale
  and why, keyboard and why - is one line there.
* Scale is `screen.scale` (`KIOSK_SCALE`) if set, otherwise 2 at 3400px
  wide or more and 1 below - measured on the mode being set, if one is. `none` writes
  no `scale=`.
* **Resolution is `screen.resolution` (`KIOSK_RESOLUTION`)**: `preferred`, or
  a `WIDTHxHEIGHT` written as `mode=` into each connector's `[output]`. It is
  the one setting that can leave nobody able to see the screen, so it is
  guarded on every layer. The agent only accepts a mode some connected connector lists
  in `/sys/class/drm/*/modes` (`tessaro-ctl screen modes` prints them); the
  generator writes it only for connectors that list it and leaves the rest on
  their preferred mode; and the change is on **probation** - it reverts on its
  own unless `tessaro-ctl screen confirm` arrives within 60s. The pending change is in
  `state.json`, so it survives the agent restarting with Weston, and the boot
  oneshot reverts a change still pending at boot: a reboot is not a confirm.
  The timer is monotonic, never the wall clock.
* **The technician-facing file is still `/etc/xdg/weston/weston.ini`**, which is
  on the `/etc` overlay and persists. It is the base the generator copies, and
  any connector already named there (any `name=` line) is left alone - so
  a hand-written scale always wins. `/run/weston/weston.ini` is generated and
  must never be edited.
* The empty `ExecStart=` in the drop-in is required to clear oe-core's line
  before replacing it, and `--modules=systemd-notify.so` has to be carried over
  verbatim - `weston.service` is `Type=notify` and hangs without it.
* The `screen.*` keys are the ones read by the compositor, so
  `tessaro-ctl config set` restarts Weston for them - and with it the browser and the
  agent - rather than just the browser.

### Display hotplug

**The generated config is kept true to what is plugged in, by the agent.**
`tessaro-weston-config` only sees the screens and keyboards attached when
Weston starts. So `watch_display` in `control.rs` reads `/sys/class/drm/*/`
`status`/`modes` and `/sys/class/input/input*` every 2s. Once a change has
held still for 5s, it runs the generator again, as root, into
`/run/tessaro-kiosk/weston-candidate.ini`, and compares that with
`/run/weston/weston.ini` (`agent/tessaro-agent/src/hotplug.rs`). Weston is
restarted, taking the browser and the agent with it, only if one of these
holds:

* a connector is connected now that was not when Weston started. The
  generator records that set in a `# tessaro-weston-config: connected ...`
  comment line, and the agent matches that exact prefix. This is what brings a
  device booted with no screen onto a screen plugged in later, whatever the
  scale and resolution settings are;
* an `[output]` section would now be written differently: a different panel
  on the same connector;
* the `[input-method]` section changed: `screen.osk=auto` saw a keyboard come
  or go.

A connector going away is never a reason. Weston copes with a head
disappearing, and the running config keeps its section for when the screen
comes back. So a monitor switched off and on restarts nothing. Every restart,
and every refusal to restart for the same hardware, is one `display: ...` line
in `journalctl -u tessaro-agent`; "the config still fits" and an
operator-stopped Weston are logged only with `agent.debug=1`.

* **`screen.resolution` and `screen.scale` stay in charge.** The restart
  only re-runs the same generator with the same settings, so a late screen
  gets the configured mode (if it offers it) and the right scale. The check is
  paused while a change is on probation: that change restarted Weston itself,
  and a monitor re-syncing to the new mode must not be taken for a new one.
* **It cannot loop.** The hardware snapshot a restart was made for is written
  to `/run/tessaro-kiosk/display-reconciled`. If Weston comes back from that
  restart and the config still differs for the same hardware, the agent logs
  it and leaves it alone. It also leaves an operator-stopped `weston.service`
  alone.
* **Weston has to be allowed to start with no screen.** Weston 13's `[core]
  require-outputs` defaults to `any`: with no output to enable, it exits with
  status 1, and its log just stops at `Color manager: no-op`. On the Pi under
  full KMS, booting without HDMI therefore meant no Weston and no browser at
  all. The generator adds `require-outputs=none` to `[core]` unless the base
  sets it. Measured on the Pi: Weston then starts with no screen, and lights
  the screen by itself when HDMI is plugged in. Chromium started with no
  output does not answer DevTools, so the agent restarts it once meanwhile.
  That is harmless; the hotplug restart gives it a fresh start anyway.
* **Writeback connectors are not screens.** vc4 under full KMS exposes
  `Writeback-1`, always `connected` with no modes. The generator and
  `tessaro-ctl screen modes` skip it.
* **The generator's output must depend on the settings and the hardware
  only.** A timestamp or anything random in it would make every hotplug
  restart the compositor.
* **On the Pi this only works on full KMS** (`VC4DTBO = "vc4-kms-v3d"` in its
  kas fragment). meta-raspberrypi defaults `raspberrypi3-64` to fake KMS, where
  the firmware owns HDMI. A screen missing at boot then never came up, and a
  monitor switched off and on came back with the firmware scaling Weston's old
  framebuffer into a new mode, which looked like squashed text. Under full KMS
  the kernel owns HDMI and sends real hotplug uevents. `config.txt`'s `hdmi_*`
  options are ignored there; `video=` on the kernel command line replaces them.

### On-screen keyboard

**It was always in the image; nothing was speaking to it.**
`/usr/libexec/weston-keyboard` ships in the `weston` package (oe-core's
`FILES:${PN}` covers `${libexecdir}`, and the `clients` PACKAGECONFIG is on by
default), and Weston launches it unprompted - `text_backend_configuration()`
defaults `[input-method] path=` to `wet_get_libexec_path("weston-keyboard")`.
It never drew anything because Chromium was not asking for it.

The browser and the compositor each own half of it, deliberately split:

* **The browser is put in IME mode unconditionally**, by
  `--enable-wayland-ime --wayland-text-input-version=1` in
  `tessaro-kiosk.service`. Chromium 147 speaks text-input v1 and v3, and
  `kWaylandTextInputV3` is `FEATURE_ENABLED_BY_DEFAULT`, so left alone it binds
  v3, finds no `zwp_text_input_manager_v3` on Weston 13 (v1 and
  input-method-v1, nothing newer) and logs `text-input-v3 not available`. The
  version switch is only read when `--enable-wayland-ime` is also present, and
  v3 would not help anyway: `wayland_input_method_context.cc` says outright
  that it "does not support input panel show/hide yet". **These switches are
  the exception to the counterable-from-`KIOSK_CHROMIUM_ARGS_EXTRA` rule** -
  `--disable-wayland-ime` cannot undo them, because `IsImeEnabled()` tests for
  `--enable-wayland-ime` first.
* **Whether a keyboard exists is a compositor decision**, made by
  `tessaro-weston-config` writing `[input-method] path=` (empty) into the
  generated `weston.ini`, or leaving the section out so Weston's default
  applies. With no input method client bound,
  `input_method_context_create()` returns early, no panel surface is ever
  created and `show_input_panel` reaches nothing.

Keeping the flags unconditional is the point: if the IME path itself switched
with the panel, the device would take text input differently - composition,
dead keys - depending on what was plugged in. This way only the panel changes.

`KIOSK_OSK` is `auto` (default), `always` or `never`. `auto` means "no hardware
keyboard attached", and how that is decided matters:

* **udev's `ID_INPUT_KEYBOARD`, and the bus.** systemd's `input_id` builtin
  (`60-input-id.rules`) is the only thing here that tells a full keyboard from
  a device that merely has keys - a power button, a lid switch and a mouse's
  consumer-control endpoint all carry `EV_KEY`. But that tag alone is a trap:
  nearly every x86 board exposes an "AT Translated Set 2 keyboard" through the
  i8042 or the EC with nothing plugged in, so counting those would mean the
  keyboard never appeared on x86 at all. Hence USB (`0003`) and Bluetooth
  (`0005`) only, from `/sys/class/input/input*/id/bustype`.
* **Under QEMU the answer is "keyboard attached", and that is right.** runqemu
  boots x86 with `-machine q35,i8042=off -usb -device usb-kbd`, so the guest
  has a real USB keyboard (`QEMU QEMU USB Keyboard`) and `auto` hides the
  panel. Exercising the keyboard under `mise run run-vnc` therefore needs
  `tessaro-ctl config set screen.osk=always`; note `run`/`run-vnc` pass
  `-snapshot`, so that does not survive a reboot of the VM.
* **Keyboard-shaped peripherals will fool it.** A barcode scanner, an RFID
  reader or a KVM dongle enumerates as a USB HID keyboard. `screen.osk=always`
  is the answer, which is why that value exists.
* **It fails towards showing the keyboard.** No `udevadm`, an unpopulated udev
  database, anything unexpected: the verdict is "no keyboard" and the panel is
  offered. A superfluous keyboard on screen is a nuisance; a touch-only device
  with no way to type is a brick.
* **It follows hotplug, at the cost of a Weston restart.** The agent re-runs
  the decision when an input device comes or goes, and restarts Weston, taking
  the browser with it, if the verdict changed - see **Display hotplug**. So on
  a device with `auto`, plugging in a keyboard, or a scanner that looks like
  one, costs a page reload. `screen.osk=always`/`never` never restart for it.
* An `[input-method]` section written by hand in `/etc/xdg/weston/weston.ini`
  wins over all of it, the same courtesy `[output]` sections get.

What weston-keyboard actually is: a demo client, cairo-drawn, fixed 60x50 px
keys scaled by the output scale, QWERTY with shift and symbols, a numeric
layout selected from the field's content purpose, and a real touch handler
(`clients/keyboard.c`), so a finger works. It takes no keyboard grab, so a USB
keyboard keeps working normally with the panel up.

**The page cannot see it.** Chromium's v1 client keeps only a visible/not
visible bool out of `input_panel_state` and never learns the panel geometry,
the surface is not resized, and `visualViewport` does not change - so a field
near the bottom of the page can sit behind the keyboard with nothing the site
can do about it. A touch-first site should keep its inputs out of the bottom of
the viewport, or bring its own keyboard in the page, where it can reserve the
space. That in-page route is also the only one that can react to a keyboard
being plugged in without restarting anything.

### Remote access

**A technician sees the real panel over VNC on `127.0.0.1:5900`**, reached
through an SSH tunnel. It is the live screen with the live Chromium on it, not
a second session.

**It is view only, and that is a Chromium limitation rather than a setting.**
Remote input does arrive: `screen-share` injects it with `notify_motion_absolute`
and friends through a synthetic seat. But that seat is a *second* `wl_seat`
(`weston_seat_init(&seat->base, compositor, "screen-share")`,
`screen-share.c:374`), created when the first viewer connects, and Chromium
binds exactly one seat - `wayland_seat.cc:34` returns early once
`connection->seat_` is set, which the libinput seat has done at startup. So
clicks and keys are delivered to a seat the browser never bound. The fix is
about 40 lines in a patch against `screen-share.c`, written up as item 11 in
`TODO.md`; do not go looking for a flag.

**It cannot be done by adding a VNC backend to the running compositor.** In
Weston a backend is what drives the display, and this one is on
`drm-backend.so`. So the mirror is `screen-share.so` (loaded from the
`weston.service` drop-in): it forks a *second* Weston on `vnc-backend.so` plus
`fullscreen-shell.so`, presents this compositor's output surface into it over
`zwp_fullscreen_shell_v1`, and injects the remote pointer and key events back
through a synthetic seat (`ss_seat_handle_motion` → `notify_motion_absolute`) -
which is the half that does not reach the browser, see above.

* **The `[screen-share]` section is generated**, by `tessaro-weston-config`,
  under `screen.vnc` (`KIOSK_VNC`, `on` by default, `off` to disable) - the same file, the
  same log (`journalctl -t tessaro-weston-config`) and the same "a section
  written by hand in `/etc/xdg/weston/weston.ini` wins" rule as `[output]` and
  `[input-method]`. The `weston-init` bbappend deletes the `[screen-share]`
  block the shipped `weston.ini` inherits from oe-core through Moonforge; that
  block names `rdp-backend.so`, which is not built, and Weston honours the
  *first* matching section, so a leftover would silently shadow ours. A
  `bbfatal` in `do_install` guards that.
* **`--address` and `--port` are command-line only.** weston.ini's `[vnc]`
  section takes `refresh-rate`, `tls-cert` and `tls-key` and nothing that binds
  a socket, so loopback is enforced from the generated `command=`. The child
  also gets `--no-config`, without which it loads `weston.ini`, finds a
  `[screen-share]` section and shares itself recursively.
* **Do not copy oe-core's stock command verbatim.** It carries
  `--no-clients-resize`, an RDP-backend option; an unrecognised option is fatal
  to the child, and the failure reads as a screen-share problem rather than a
  typo.
* **TLS and a login are not optional and not configurable.**
  `libweston/backend-vnc/vnc.c` calls `nvnc_enable_auth(NVNC_AUTH_REQUIRE_AUTH |
  NVNC_AUTH_REQUIRE_ENCRYPTION, ...)` and refuses to start without a cert and
  key. There is no unauthenticated mode and no VNC-standard password auth -
  neatvnc's only password mechanism is the "plain" sub-type inside VeNCrypt,
  which *is* the TLS path. Hence what would otherwise look like
  over-engineering: `PACKAGECONFIG:append:pn-neatvnc = " tls"` in
  `tessaro.conf` (its own default is `""`, and without it Weston logs `Neat VNC
  built without TLS support` and dies), and a self-signed certificate generated
  at build time by the `weston-init` bbappend into `/usr/lib/tessaro-vnc/`.
* **The credential is `tessaro` / `tessaro`, and it is deliberately not a
  system account.** `weston_authenticate_user()` is
  `pam_start("weston-remote-access", <username the client sent>, ...)`, and the
  stack Weston ships is `auth include login` - which can only ever accept one
  username, `weston`. The compositor is unprivileged, and `pam_unix`'s helper
  refuses to check any account but the caller's own: `unix_chkpwd.c:133-146`
  drops its setuid when the requested user differs, and then cannot read
  `/etc/shadow` (0400 root). Weston's own man page says as much - "the VNC
  client has to authenticate as the user running weston". Creating a `tessaro`
  account does *not* work around it; that was tried and every login was
  refused.
  So `weston_%.bbappend` replaces that PAM stack with `pam_exec` running
  `/usr/libexec/tessaro-vnc-auth`, which compares against `KIOSK_VNC_USER` and
  `KIOSK_VNC_PASSWORD` from the image defaults
  (`/usr/lib/tessaro-kiosk/tessaro-kiosk.env`) only. No account, no
  `/etc/shadow`, no privilege. The credential is an image property on
  purpose: there is no setting for it (a test in `keys.rs` keeps it that way),
  `generated.env` is not read, and changing it takes a new image. The stack
  alone was not enough: `vnc_handle_auth` in `vnc.c` also refused any username
  that did not resolve to the compositor's own uid before PAM was ever called,
  and `0001-vnc-let-the-PAM-stack-decide-which-user-may-log-in.patch`, applied
  by the same bbappend, removes that check. It needs `pam-plugin-exec`, which is not in the
  image by default and is an `RDEPENDS` of weston for that reason; without it
  every login fails with a bare `PAM: authentication failed`.
* **Client compatibility is narrow.** VeNCrypt with plain auth means TigerVNC
  or Remmina. macOS Screen Sharing and RealVNC fail in the handshake. The cert
  is self-signed and identical across an image, so the fingerprint warning
  means nothing.
* **Sharing is not free while it is on.** `weston_output_disable_planes_incr()`
  takes the output off hardware overlay and cursor planes for as long as it is
  shared, and every damage rectangle goes through `read_pixels()`. A static
  page is nearly free; full-screen video is a readback per frame. `screen.vnc=off`
  is the first thing to try on a Pi that feels slow.
* **One client at a time** - a second connection disconnects the first - and
  **only outputs present when Weston starts are shared**. A monitor plugged
  in later is picked up by the agent's hotplug restart (see **Display
  hotplug**), and the share with it.
* **SSH now ships in every image**, not only debug ones:
  `ssh-server-dropbear empty-root-password allow-empty-password` in
  `moonforge-image-base.bbappend`. `allow-empty-password` is the one that
  matters for dropbear - it adds `-B`, without which a blank password is
  refused whatever the hash says. **The empty root password now only lasts
  while the device is unclaimed**: claiming it (below) sets a random one, and
  unclaiming or a factory reset empties it again. So a fresh or reset device
  is a root shell with no credential on any network it joins - accepted, and
  the reason to claim a device before it leaves the bench. On a claimed
  device the way in is a key: see **SSH keys** below.

### SSH keys

**`tessaro-ctl --node NAME ssh connect` is a root shell by key, with no password and
no first-use prompt.** It sends your public key (`~/.ssh/id_ed25519.pub` and
the other ssh-keygen defaults, or `--key PATH`) over the pinned, token-
authenticated control connection; the agent adds it to root's
`authorized_keys` and answers with the device's host key; the client writes
that to `~/.config/tessaro/known_hosts` under `tessaro-<node id>` and execs
`ssh -o HostKeyAlias=... -o StrictHostKeyChecking=yes root@<address>`.
Anything after `--` goes to ssh. `--print` pushes the key and prints the
command instead. `tessaro-ctl ssh keys list` and `ssh keys revoke
<fingerprint|prefix|comment>` manage what is there. The logic is
`agent/tessaro-agent/src/ssh.rs`; key parsing is `agent/protocol/src/sshkey.rs`,
shared so both ends refuse the same keys.

* **The claim model owns `authorized_keys`, as it owns the root password.**
  Unclaim, factory reset and revoking the last token empty it, and the boot
  oneshot empties it on any device with no tokens, which heals a power cut
  in the middle of an unclaim. An unclaimed device refuses a key outright.
  Revoking a key never unclaims: only tokens decide that.
* **Options are refused.** A line with `command=`, `from=`, `no-pty` and the
  like is rejected on both ends. Whoever holds a token could otherwise plant a
  forced command for root. Lines already in the file that the agent does not
  understand are kept by add and revoke, and go with unclaim.
* **The file lives in `/root/.ssh`, and `/root` had to be made writable.**
  `ROOT_HOME` is `/root` on a systemd distro (oe-core's
  `init-manager-systemd.inc`, not `/home/root`), it is on the read-only
  rootfs, and dropbear 2022.83 only ever reads `$HOME/.ssh/authorized_keys` -
  there is no option to point it elsewhere. So the volatile-binds bbappend
  binds `/data/overlay-root` over `/root`, like `/home`. The first e2e run,
  with the file under `/home/root`, got `Permission denied (publickey)`. It
  persists and survives updates. Every write replaces it whole, `.ssh`
  at 0700 and the file at 0600, set explicitly: dropbear silently ignores a
  key file that is group or world writable.
* **The host key already persists, and that was checked, not assumed.**
  oe-core's `read_only_rootfs_hook` moves dropbear's key to tmpfs
  `/var/lib/dropbear` - a new key every boot - but only on images without
  `overlayfs-etc`. Ours has it, so the key stays in `/etc/dropbear` on the
  overlay (the built rootfs's `/etc/default/dropbear` has no
  `DROPBEAR_RSAKEY_DIR`). Losing `overlayfs-etc` would bring the per-boot key
  back, and the pin with it would refuse every login after a reboot.
* **Dropbear is socket-activated**, and `dropbearkey.service` only runs on the
  first connection. So a device nobody has logged in to has no host key yet;
  the agent makes it (`dropbearkey -t rsa`, the unit's own command) before
  answering, and the first `tessaro-ctl ssh connect` already gets a pin. When no host
  key can be read, the client drops the stale pin and ssh asks as usual.
* **The address is the one the control connection used**, from `nodes.json`
  or mDNS, so a device that moved is found the same way `device status` finds it -
  and a different device at the old address fails the TLS pin before any
  key is sent.

### Settings, tessaro-ctl and the claim model

**One management surface.** A device's settings live in
`/data/tessaro/state.json`, the only way to change them is `tessaro-ctl`, and
`tessaro-agent` is the only thing that writes the file. `tessaro-ctl config keys`
lists every setting (the registry is `agent/protocol/src/keys.rs`), `config get`,
`config set KEY=VALUE ...` and `config unset KEY ...` do what they say, and each change
restarts exactly what reads the key: the agent restarts itself for an agent
key (invisible on screen), the browser restarts for a browser key or a new
kiosk origin, Weston restarts - taking the browser and agent with it - for a
`screen.*` key.

* **`state.json` is sparse**: only what was set, as the registry's dotted
  names. Everything else follows `/usr/lib/tessaro-kiosk/tessaro-kiosk.env`,
  so a later image still moves a default.
* **Writes are locked and survive a power cut.** `flock` on a separate
  `.lock` file (a rename swaps the data file's inode, so the lock cannot live
  on it), the new content to `.tmp` and `fsync`, the current file hard-linked
  to `.prev`, `rename`, `fsync` of the directory. A read falls back from the
  file to `.prev` to the defaults and logs it; a torn file never stops the
  kiosk. `auth.json` and `secrets.json` use the same store. Nothing in these
  files is a timestamp: device clocks drift, and `revision` is a counter
  (`config set --if-revision N` is compare-and-set).
* **The CLI documents itself.** `tessaro-ctl config keys` prints every setting with
  its description, current value or default, what it accepts and what a
  change restarts; `config keys KEY` prints one. The text comes from the registry on
  the device (`Kind::describe` plus each key's `doc`), so a client never
  documents settings a device does not have. `tessaro-ctl --help` carries
  worked examples.
* **Custom values and URL placeholders.** `data.NAME=VALUE` defines a custom
  value - the NAME is whatever the site needs, the kiosk gives it no meaning.
  **A placeholder is always a setting's full key in braces**, custom or
  built-in, anywhere in `browser.url` - host, path or query - percent-encoded so
  a value cannot change the URL's structure:
  `config set 'browser.url=https://menu.test/?table={data.table}' data.table=12`.
  There is no short form: `{table}` is refused, with a hint to write
  `{data.table}`. `tessaro-ctl config keys` lists every custom value defined, and
  says whether the URL uses it. Expansion happens once, in `state::Effective`:
  the browser unit gets the expanded URL in `generated.env`, and the agent's
  origin checks and the device-API policy see the same one, so a placeholder
  in the host moves the grants too. Built-in settings expand to their
  effective value, set or image default - `{screen.osk}`,
  `{browser.fps_counter}` - and `{device.name}` is the name the device actually
  answers to even when none was set. Only `{browser.url}`,
  `{browser.maintenance.url}` and `{browser.debug.template}` are refused, as no template may
  contain a template (the debug template alone takes `{browser.url}`).
  `browser.maintenance.url` and `browser.debug.template` are templates by the same rules, and
  `config set` checks every template whichever one is on screen. Because any setting can move the URL, whether
  the agent restarts is decided by comparing the expanded URL with the one the
  running agent started with, not by which key changed. `config set` refuses a
  template with an unset `data.*` or a name that is no setting, and a `config unset`
  of a `data.*` still in use; custom values and template can go in one command.
  Nothing is added implicitly - only what the template names.
* **Read-only keys report the device.** `device.id` and the read-only
  `network.*` keys - `network.ip`, `network.netmask`, `network.cidr`,
  `network.gateway`, `network.dns`, `network.interface`, `network.interfaces`,
  `network.mac`, `network.hostname`, `network.wifi.hotspot_ssid`, and every
  address as `network.ipv4`/`network.ipv6` (comma separated) - and the
  `storage.*` keys (see **Storage**) are listed by `config keys`, read by `config get`, usable as
  placeholders, and refused by `config set`. The network ones come straight from the kernel
  (`agent/tessaro-agent/src/net.rs`: `/sys/class/net`, `getifaddrs`,
  `/proc/net/route`, and resolved's own `/run/systemd/resolve/resolv.conf`,
  since `/etc/resolv.conf` is its 127.0.0.53 stub), not from NetworkManager,
  so they answer even when NM is the broken thing. The exception is
  `network.public_ip`, which only the outside world knows. **It is looked up only
  while the template on screen uses `{network.public_ip}`** (`browser.url`,
  `browser.maintenance.url` in maintenance mode, or the debug template while
  the debug screen is up) - a link may be metered - and
  then the agent asks `https://1.1.1.1/cdn-cgi/trace` every 5 minutes (30s
  until it has an answer, and after a failure), keeps it in
  `/run/tessaro-kiosk/public-ip`, and keeps the last address when a request
  fails. So it is empty at the boot render and fills in shortly after.
  `tessaro-ctl network show` and `config get network.public_ip` look it up on the spot whatever
  the URL uses - one request per ask, up to ~10s when offline - while `config keys`
  and a plain `config get` only show the last address found this boot. "Primary"
  means the
  interface carrying the IPv4 default route. `tessaro-ctl network show` shows the same
  as an overview, `network interfaces` every interface with kind, state, carrier,
  MAC, MTU, speed and addresses. Changing the network is the `network.ethernet.*` and
  `network.wifi.*` settings - see **Network control**.
  **A URL using one moves on its own**: the boot render runs before DHCP, and
  leases change, so while the template on screen uses a read-only key the agent checks
  every 15s and, when the expanded URL is no longer the one it drives,
  re-renders and restarts itself onto it (and the browser, if the origin moved).
* **Values are validated once, at `config set`**: enums, ranges, URLs, modes - and no
  control characters, quotes, backslashes or `$` anywhere, because the value
  ends up in an env file systemd parses. A newline would write a second
  variable.
* **Rendering.** `generated.env` in `/run/tessaro-kiosk` and the Chromium
  policy are re-rendered after every change and by `tessaro-config.service`
  at boot, and written only when the content differs - a no-op must never
  restart anything on a public screen.
* **The agent never spawns Chromium.** It only ever drives the units over
  `org.freedesktop.systemd1`, and the browser unit keeps `WantedBy=`, so a
  crashlooping agent still leaves a browser on the defaults.
* **Escape hatch**: `/data/tessaro/factory-reset`, or `tessaro.factory_reset`
  typed on the kernel command line at the boot loader for one boot, is acted
  on by the boot oneshot before the agent starts. `tessaro-ctl device factory-reset`
  does the same while the agent runs.
* **Migration**: a leftover `/etc/default/tessaro-kiosk` is imported into
  `state.json` once at boot - valid keys only, the rest named in
  `journalctl -t tessaro-config` - and renamed `.migrated`. The same boot step
  moves settings saved under a key's old name to the new one
  (`keys::RENAMED`: `kiosk.url` became `browser.url`, `display.*` became
  `screen.*`, and so on), placeholders in the templates included, and logs
  each move. A `config set` or `config get` of an old name is refused with
  the new one - there are no aliases.

**One protocol over a local socket and TLS** (newline-delimited JSON,
`agent/protocol/src/lib.rs`):

* **`/run/tessaro-agent.sock`**, mode 0600 root: no auth, no TLS, full power.
  Not group accessible on purpose - Chromium runs as `weston`, and a
  compromised browser must not be one `connect()` from the control plane.
* **TLS on `access.listen`** (default `0.0.0.0:7400`, `off` disables it). The
  device makes an EC P-256 key and a self-signed certificate in
  `/data/tessaro/tls/` on first boot, valid from 1970 to 9999 so a wrong clock
  cannot break it. Clients **pin** its SHA-256 on first use, keyed by node id,
  and check the pin before any token is sent.

**The claim model:**

* A fresh device is **unclaimed**: no tokens, empty root password. Over TCP it
  answers only `id`, `claim` and `ping`.
* **The first `claim` wins.** It gets a token and the root password becomes a
  random 20-character one, which `tessaro-ctl` shows exactly once. Order
  matters for power loss: the password is set first, then the token
  committed, so a cut in between leaves "no tokens, a password", which the
  boot oneshot resets to empty. The reverse would leave a claimed device with
  an empty root.
* **Further tokens are issued only against a valid token** (`access token create`),
  or over the local socket. Tokens never expire; revoking one deletes it.
  Only SHA-256s are stored, compared in constant time. The device is claimed
  exactly when a token exists, so revoking the last one unclaims it.
* **`access unclaim`** removes every token and ssh key and empties the root password;
  **`device factory-reset`** also wipes the settings. After either, the first client
  to claim wins again. The TLS key survives both, so pins stay valid.
* `access password set` (prompted, a `PASSWORD` argument, `--password-stdin`,
  or `--random`) changes the root password on a
  claimed device; an unclaimed one keeps it empty.
* Failed tokens are rate-limited per address, but a valid token always gets
  in - the tokens are 256 bits, the limiter only keeps scans quiet.
* **Accepted exposure**: whoever reaches a fresh or reset device first owns it,
  and the mDNS record says which devices are unclaimed.

**Names.** The node id is systemd's app-specific machine id (HMAC-SHA256 of
`/etc/machine-id` over a fixed Tessaro app id, stamped v4) - it matches
`systemd-id128 -a 8a6c7b172d5443cd9033a24d0df85022 machine-id`, and the
machine id itself never leaves the device. **Never change that app id**: it
would rename every device. The name is `adjective-noun-xxxx` from the id, or
`device.name`. The agent announces `NAME.local` and `_tessaro._tcp` over mDNS
(`mdns-sd`, TXT `id`, `fp`, `ver`, `machine`, `claimed`; `access.mdns=off`
stops it). `tessaro-ctl --node NAME` goes to the address it last saw that
device at first - instant, no scan - and scans mDNS only when nothing answers
there, or when a different certificate or node id does (then with a warning:
the device most likely moved and its old address went to someone else). A
device found at a new address has it updated in `nodes.json`. With an
expected node, its own pin is always checked first, so another known kiosk
answering at that address is a mismatch, never a silent switch.
`tessaro-ctl nodes list` lists what answers.
Wiping `/data` or the `/etc` overlay re-identifies a device.

`tessaro-ctl` on a laptop: `mise run build-ctl`, then
`tessaro-ctl --node NAME access claim` (or `access login --token` with a token someone
issued). Pins and tokens are kept in `~/.config/tessaro/nodes.json`, 0600.

### Maintenance mode

**`tessaro-ctl browser maintenance on|off`** - the same as `config set browser.maintenance.enable=1|0`
- puts `browser.maintenance.url` on screen and leaves `browser.url` as it is, so `off`
goes straight back to the site. The default page is
`http://127.0.0.1/maintenance.html` (`TESSARO_MAINTENANCE_URL` in
`tessaro.conf`), shipped by `tessaro-selftest` next to the self-test page,
self-contained so it renders with the network down. It takes `?title=` and
`?message=` as plain text, which is how a device customises it without an
image: `browser maintenance on --url 'http://127.0.0.1/maintenance.html?message={data.msg}'`
plus `data.msg=...`.

* **The swap is one place, `state::Effective`.** With `KIOSK_MAINTENANCE=1`,
  `KIOSK_URL` *is* the expanded maintenance URL, so every consumer follows it
  without knowing the mode exists: `generated.env` (a reboot in maintenance
  never flashes the site), the agent's navigation and origin enforcement, the
  periodic refresh, `device status`, the `url_moved` restart check and the read-only
  key watcher.
* **`KIOSK_PROBE_URL` reads as empty meanwhile**, so the agent probes the
  maintenance page. Probing the site's health endpoint instead would put the
  offline page over the maintenance page the moment the site went down - and
  maintenance is often exactly when it is down.
* **The device-API grants do not move.** `render::device_origins` uses
  `Effective::kiosk_url()`, browser.url's origin whatever the mode. Following the
  maintenance page would rewrite the policy on every toggle, restart the
  browser on a public screen and take the site's grants away. So a toggle
  restarts the agent only, which re-navigates; the browser keeps running.

### Debug screen

**`tessaro-ctl browser debug on|off`** - the same as `config set browser.debug.enable=1|0` - swaps the
page for a full-screen text screen: `browser.debug.template` filled in, in large
DejaVu Sans Mono, white on black, shrunk until the longest line fits.
`browser debug on --template '...'` sets the template in the same change, and
`device status` shows a `debug screen` row while it is up. Both keys are agent keys,
so a toggle restarts only the agent, like maintenance mode; the browser keeps
running. `browser.debug.enable` is not `agent.debug`, which is journal verbosity; its
env name is `KIOSK_DEBUG_SCREEN` because `KIOSK_DEBUG` was taken.

* **It wins over maintenance mode, and it is the agent's, not
  `Effective`'s.** Maintenance swaps `KIOSK_URL`, a URL every consumer can
  follow. The debug screen is a page the agent generates, so the agent shows it
  instead of whatever `KIOSK_URL` is (`state::debug_screen`), and nothing else
  changes: not `generated.env`, not the policy. After a reboot the browser comes
  up on `KIOSK_URL` for the few seconds until the agent's first cycle.

* **The template is browser.url's templating with raw values.** It accepts the
  same `{key}` placeholders (any setting, read-only or `data.*`), plus
  `{browser.url}` itself, expanded. `config set` holds it to the same rules: an unset
  `data.*` or a name that is no setting is refused. Values go in as they are,
  not percent-encoded, and `debug.rs` escapes them for HTML
  (`state::expand_text` straight on `keys::expand_with`, next to `expand_url`,
  which goes through `keys::expand`, its percent-encoding wrapper).
  The default shows the name, node id, hostname, the default route, IP, MAC,
  DNS, the public address, every interface (`network.interfaces`, read-only,
  added for this), IPv6 and the kiosk URL (`{browser.url}`).
* **`\n` - a backslash and an n, as typed - is the line break**, and it is
  the one backslash any value may carry (`Kind::Template` in `keys.rs`). The
  generic no-backslash rule exists because values end up in env files, and
  this is how that stays true here. `render::env_file` never writes the template
  into `generated.env`, since only the agent reads it and it reads state.json.
  And the image default in `tessaro-kiosk.env.in` is **single-quoted**,
  because systemd keeps a backslash only inside single quotes. Measured:
  unquoted `a\nb` reaches the process as `anb`.
* **It replaces the page. It is not an overlay.** The agent stages
  `/run/tessaro-kiosk/debug.html` next to the offline page (same
  temp-and-rename, `offline::replace`) and navigates to it. While it is up
  there is no probe, no offline page and no origin enforcement: a technician
  wants the addresses most exactly when the site is down. The CDP liveness
  check and the browser restart still run. An overlay injected into the site
  would be lost on every navigation, and the page could hide it.
* **It re-renders every 5s and navigates only when the text changed**, so a
  DHCP renewal shows up within seconds without a reload loop. The template is
  filled in from the settings the agent started with and the device as it is
  at that moment (`render::live`).
* **`{network.public_ip}` asks Cloudflare only while the screen shows it.**
  `watch_public_ip` treats the debug template as in use only while
  `browser.debug.enable` is on, so the default template costs no request on a
  device that is not in debug mode.

### File storage

**`tessaro-ctl files upload|download|sync|list|move|rm` manages `/data/files`,
which nginx serves read-only at `http://127.0.0.1/files/`**, so a site can
keep its videos, images and JSON on the device and have them with the
network down. Remote paths are from the store's root, a leading `/`
optional. `files list [REMOTE]` is `ls -l`: one level, directories with a
trailing `/`, `-R` for the whole tree (`files-list` has `recursive`, which
sync and download set). `files move SOURCE... DEST` is `mv`: into DEST when
it is a directory or ends in `/`, a rename otherwise, within the store so
atomic. `files sync LOCAL_DIR [REMOTE_DIR]` is rsync `-r --delete`
compared on size and mtime (whole seconds) alone: new and changed files are
sent, whatever the local directory lacks is removed, after a prompt unless
`-y`, and `--dry-run` only prints the plan. Symlinks and special files on the
client are skipped with a warning. The logic is `agent/tessaro-agent/src/files.rs`
and `agent/tessaro-ctl/src/files.rs`; path rules are `agent/protocol/src/files.rs`,
shared so both ends refuse the same paths.

* **A file arrives like an image upload.** `files-begin` describes it (path,
  size, mtime), `files-chunk`s of up to `UPDATE_CHUNK` append it to
  `/data/tessaro/files-upload/upload.part`, each synced before it is
  acknowledged, and the last one sets the mtime, mode 0644, and renames it
  into place. So nginx never serves half a file, and the staging being on the
  same filesystem is what makes the rename atomic. One upload slot: the same
  path, size and mtime resumes, across an agent restart too; anything else
  drops the unfinished one. A file the store already has with the same size
  and mtime is answered as complete and never sent.
* **Downloads are request/response, not a stream**: `files-read` of up to
  `UPDATE_CHUNK` from an offset, written to a `.part` beside the target and
  renamed, with the device's mtime.
* **256 MiB of `/data` is always left free** (`RESERVE`): the Chromium
  profile, the settings and an image update's staging live there too.
* **Nothing follows a symlink**, on either side of nginx: the agent checks
  every component with `symlink_metadata` and opens with `O_NOFOLLOW`, and the
  location has `disable_symlinks on`. The agent never makes one; one made by
  hand could otherwise reach `/data/tessaro` and its tokens.
* **The nginx location is in the self-test's server block**
  (`10-tessaro-selftest.conf`), because that is the one server on
  `127.0.0.1:80`. `autoindex off`, `Access-Control-Allow-Origin: *` and
  `Cache-Control: no-cache` - restated there, since a location's `add_header`
  replaces the server's.
* **An https kiosk site may use it without a prompt.** `http://127.0.0.1` is
  potentially trustworthy, so it is not mixed content, but Chromium's Local
  Network Access (139 on) puts a public site's requests to the loopback behind
  a permission prompt. `LocalNetworkAccessAllowedForUrls` lists the device
  origins, rendered by the agent with the serial and HID grants
  (`ORIGIN_POLICIES` in `render.rs`), so it follows `browser.url`.
* **A factory reset empties it; unclaim keeps it** - it is the site's
  content, not access to the device. The reset renames the store to
  `/data/tessaro/files-trash`, makes it again empty and then deletes the trash,
  so what nginx serves is gone at once, and the boot oneshot finishes a
  deletion that was cut short. `--wipe-data` and `--repartition` re-create
  `/data` anyway. `/data/files` itself comes from tmpfiles (0755 root) and the
  agent makes it too.

### Speed test

**`tessaro-ctl network speedtest` measures the device's link, not the client's.** The
agent runs it against speed.cloudflare.com and streams one line per step:
where Cloudflare sees the device from (`/cdn-cgi/trace`), latency (25 empty
requests, less the server's own `Server-Timing`), then download and upload at
100k, 1m, 10m, 25m, 100m up to `--max-size` (default 25m), `--tests` samples
each (default 10), and a result - the median at the largest size that
produced samples, because small payloads never leave slow start. No Ookla,
on purpose: its only client is a closed binary.

* **It is the `cfspeedtest` crate, and that is the one place reqwest and
  rustls are allowed.** cfspeedtest is blocking reqwest on rustls/ring with
  webpki-roots - about 100 more crates in the lock, and a root store compiled
  in, so this test (only this test) ignores the device's `/etc/ssl/certs`.
  Everything else stays on hyper + native-tls, as the TLS bullet above says.
  Fenced into `agent/tessaro-agent/src/speedtest.rs`; do not reach for
  reqwest elsewhere just because it is in the lock now.
* **It runs on one `spawn_blocking` thread**, which sends a step per payload
  size down a channel that `server.rs` forwards as events. The reqwest client
  is built and dropped on that thread - a blocking client dropped on the
  runtime thread panics. Every request has a 30s timeout, the thread stops at
  the next step once nobody is listening, and the server gives the whole test
  5 minutes, so the agent never waits on Cloudflare and the watchdog never
  notices. A size that took over 5s is the last one tried (cfspeedtest's own
  rule), which keeps a slow link from spending minutes on 25 MB samples.
* **One at a time.** The lock is held by the thread, not the request, so a
  second test is refused even while an abandoned one is still finishing.
* **It moves a few hundred MB.** Uploads stop at 25m whatever `--max-size`
  says, because cfspeedtest builds the body in memory. On a metered link use
  `--max-size 1m`. The start and the result go to the journal at info.

### Audio

**Sound is PipeWire and WirePlumber, and the audio.* settings are the only
truth about it.** `tessaro-ctl audio output hdmi|jack|usb|bluetooth|auto|off`
(or one output's name from `audio outputs`), `audio volume N`, `audio mute
on|off`, `audio input ...` and `audio input-volume N` are each a `config set`
of one `audio.*` key. The agent applies it to the running sound server at
once: nothing restarts, and a sound already playing moves over. `audio show`
says what the settings resolved to and why, `audio test` plays a 1s tone on
the output in use, `audio test --input` records 3s and prints the level. The
logic is `agent/tessaro-agent/src/audio.rs`; the image side is
`meta-tessaro-distro/recipes-multimedia/tessaro-audio/`.

* **The sound server runs as weston, from units of our own.**
  `tessaro-pipewire`, `tessaro-wireplumber` and `tessaro-pipewire-pulse` are
  system units with `User=weston`, sharing `/run/tessaro-audio` (tmpfiles,
  0700) as their runtime directory. PipeWire's own user units need a logind
  session the image never has, and the recipe's system-wide unit runs as a
  `pipewire` user whose Pulse socket Chromium could not reach. The recipe's
  unit is kept from being enabled by leaving `systemd-system-service` out of
  PipeWire's `PACKAGECONFIG` - in a bbappend, because the recipe sets
  `PACKAGECONFIG:class-target`, and a `:pn-pipewire` value in `tessaro.conf`
  would lose to it silently (`CLASSOVERRIDE` comes after `pn-${PN}` in
  `OVERRIDES`).
* **Chromium plays through the Pulse socket and nothing else.**
  `PULSE_SERVER=unix:/run/tessaro-audio/pulse/native` in
  `tessaro-kiosk.service`, and `libpulse` in the image, which is what makes
  Chromium's Pulse backend load at all (it is dlopened, and nothing pulled it
  in before). Chromium is deliberately not in the `audio` group: it cannot
  open a card itself and fight PipeWire for it, and with no sound server it
  plays nothing rather than something unpredictable.
* **Chromium does not rebuild for any of this, and must not.** It was always
  built with `use_pulseaudio=true`: `pulseaudio` is a *backfilled*
  `DISTRO_FEATURE`, and the `pulseaudio` recipe was already a build
  dependency, so `libpulse` costs nothing new. Do not touch
  `DISTRO_FEATURES` or `DISTRO_FEATURES_BACKFILL_CONSIDERED` for sound: that
  flips `use_pulseaudio` and rebuilds Chromium. `bitbake -n
  moonforge-image-base | grep -i chromium` is the check.
* **WirePlumber remembers nothing.** Its drop-in,
  `/usr/share/wireplumber/wireplumber.conf.d/50-tessaro.conf`, turns off
  every restore setting, and the units point `XDG_STATE_HOME` and
  `XDG_CONFIG_HOME` into `/run`. So a volume or default output never
  outlives a factory reset or fights the settings after a reboot. The same
  drop-in turns off what needs a session bus (device reservation), Bluetooth
  (for now) and MIDI.
* **When the agent applies.** On a `set` of an `audio.*` key (the answer
  says where sound now plays: `Applied.audio`); at startup once PipeWire
  answers; when the sound hardware changes and has held still for 2s; and
  once a minute anyway, which puts back anything else that moved it. The
  hardware check reads `/proc/asound/cards`, the DRM connectors' status and
  the PipeWire socket's inode every 2s, which is cheap; PipeWire itself
  (`pw-dump`) is only asked when there is something to apply. Every
  `pw-dump`, `wpctl` and `pw-play` is under `deadline::within`. Applying is
  idempotent and logs `audio: ...` lines only for real changes. A server
  that is not up yet only means the setting is saved and applied later.
* **What `auto` picks.** The USB or Bluetooth output plugged in last
  (PipeWire's object serial, which is never reused), else HDMI with a screen
  connected, else the jack. HDMI counts as connected when its port says so,
  or - on the Pi's vc4-hdmi, which cannot tell - when a DRM connector named
  HDMI or DP is connected. A kind that is not there (`usb` with nothing
  plugged in) is kept and plays on `auto` meanwhile, and `audio show` says
  why; one output *by name* must exist at `set` time, like
  `screen.resolution`. `off` mutes what `auto` would pick.
* **HDMI and analog are often one card.** An Intel HDA card offers them as
  profiles, and only the active profile's outputs exist. Outputs a card has
  only in another profile are listed too (`audio outputs` says it switches
  the card over), and choosing one switches the profile first, preferring a
  profile that keeps the analog input. Checked against a fixture, not yet on
  real x86 hardware.
* **Volumes are wpctl's cubic scale**, the one every desktop slider uses:
  50 sounds about half as loud as 100. PipeWire stores the linear value, the
  cube of it.
* **The microphone is granted by policy.** `AudioCaptureAllowedUrls` lists
  the same origins as the serial and HID grants and moves with them
  (`render::ORIGIN_POLICIES`), so `getUserMedia({audio: true})` is answered
  with no prompt. `audio.input=off` mutes the input at PipeWire and leaves
  the grant alone, so it never restarts the browser. The Pi has no audio
  input of its own; a microphone there is a USB one.
* **On the Pi, HDMI sound comes from vc4, not bcm2835.** Under full KMS the
  `vc4-kms-v3d` overlay boots `snd_bcm2835.enable_hdmi=0`, so bcm2835 is the
  headphone jack only, and HDMI is vc4-hdmi's own card, which takes IEC958
  frames only - PipeWire handles that through alsa-lib's `vc4-hdmi.conf`.
  `dtparam=audio=on` is already in meta-raspberrypi's `config.txt`.

### Device APIs: WebSerial, WebHID, WebUSB, Web Bluetooth

**Every one of them is already compiled in; nothing about the browser build
needs to change.** `use_dbus`, `use_udev` (`build/config/features.gni`) and `use_bluez`
(`device/bluetooth/cast_bluetooth.gni`) all default to true on Linux and the
recipe overrides none of them, so the BlueZ backend and the udev enumeration
paths are there. `bluez5` and `bluetoothd` are in the image too, inherited from
oe-core's default `bluetooth` `DISTRO_FEATURE` rather than anything we set.

What was actually missing was kernel drivers and file permissions.

* **The page has to enumerate, not request.** A policy-granted permission is
  only visible to `navigator.serial.getPorts()`, `navigator.hid.getDevices()`
  and `navigator.usb.getDevices()`. `requestPort()`/`requestDevice()` still
  open a chooser dialog, and there is nobody in front of a kiosk to click it.
  This is a constraint on the web app and it is the thing that makes or breaks
  unattended device access.
* **Serial and HID are pre-granted to the device origins** by
  `SerialAllowAllPortsForUrls` and `WebHidAllowAllDevicesForUrls` in the policy
  file: the kiosk's own and the self-test's `http://127.0.0.1`, plus any in
  `browser.device_origins`. The kiosk's and the self-test's collapse to a
  single entry on a factory image, where they are the same string. These match on *origin* only -
  scheme, host, port, no `[*.]host` wildcards. The image ships them
  substituted at build time (`TESSARO_DEVICE_ORIGINS` in the recipe), and on
  the device **tessaro-agent re-renders the policy from `browser.url` as set**,
  from the `/usr/lib/tessaro-kiosk/policy.json` copy, so pointing a device at
  a new origin moves the grants with it. A change of origin therefore restarts
  the browser, not just the agent: Chromium reads the policy at start. The
  render goes through serde, because a syntax error would drop the whole file.
* **WebUSB ships granted to nothing.** Unlike serial and HID it has no
  "allow all" policy, and blanket raw USB is a bigger grant than blanket serial
  or HID, so `WebUsbAllowDevicesForUrls` is an empty list with a worked example
  in the comment. Its schema does not mark `vendor_id` required, so
  `"devices": [{}]` is a true wildcard if that is ever wanted.
* **Web Bluetooth cannot be pre-granted at all.** No allowlist policy exists in
  Chromium 147 - the Bluetooth chooser context has zero policy references, and
  `DefaultWebBluetoothGuardSetting` only chooses between "block" and "ask". A
  peripheral therefore needs one real chooser interaction by a technician, once
  per device; with `WebBluetoothNewPermissionsBackend` that grant persists and
  `getDevices()` returns it on later boots. The only non-interactive route is
  CDP's experimental `DeviceAccess.selectPrompt`, which happens to support
  Bluetooth and nothing else - `tessaro-agent` already holds a CDP connection,
  so that is where it would go. `agent.device_access` already sends
  `DeviceAccess.enable`, but nothing answers a prompt yet; the rest is on
  TODO.md, not built.
* **Web Bluetooth is also experimental on Linux specifically.**
  `runtime_enabled_features.json5` marks it `stable` on Android, ChromeOS, iOS,
  macOS and Windows and leaves the `default` bucket - which is us - at
  `experimental`, and `about_flags.cc` registers its flag `kOsLinux` only. So
  `navigator.bluetooth` does not exist until `KIOSK_ENABLE_FEATURES` names
  `WebBluetooth`. It ships empty.
* **Blocklists beat policy.** Serial and HID consult their blocklist *before*
  the policy grant, so a blocklisted device stays blocked no matter what is
  listed. `--disable-features=WebSerialBlocklist` is the escape hatch.
* **The serial grant includes the console UART, so pick ports by USB id.**
  `SerialAllowAllPortsForUrls` means *all*: on the Pi, `getPorts()` returns
  `/dev/ttyS0` first, with an empty `getInfo()`. That is the serial console
  (`console=ttyS0`, a getty on it, `root:tty 0620`), so `open()` on it fails
  with `NetworkError: Failed to open serial port.` - by design, and not
  something to grant. A page that takes `ports[0]` never reaches the USB
  adapter listed after it. Select on `getInfo().usbVendorId` instead; the
  self-test page does that and offers a picker.
* **WebHID lists keyboard-mode devices but never delivers their input.** A
  barcode scanner in keyboard emulation shows up in `getDevices()` with a
  single `1:6` (Generic Desktop / Keyboard) collection, and Chromium blocks
  reports from protected keyboard collections. Only a scanner that exposes a
  HID POS collection (usage page `0x8C`) can be read, and that is a setting on
  the scanner, not here. Check it with `hexdump -C
  /sys/class/hidraw/hidrawN/device/report_descriptor`: HID POS contains `05 8c`.
  A TMS/TEEMI-type scanner (`f126:0288`) never changed its descriptor through
  four "HID POS" configuration codes - it stays keyboard plus CDC serial, and
  its serial port is the non-keyboard channel it actually has.
* **Device nodes are group-owned, not `uaccess`.** Chromium's device service is
  in-process in the browser and opens the node itself - no privileged helper
  outside ChromeOS - so the `weston` user needs the permission directly.
  `dialout` covers `/dev/tty*` from systemd's own rules; `70-tessaro-devices.rules`
  puts `hidraw` and `usb_device` in `plugdev` at 0660, since hidraw has no group
  at all by default and usbfs is 0664 root:root. The unit names both groups in
  `SupplementaryGroups=`. `TAG+="uaccess"` would *not* work here: logind grants
  those ACLs to the active seat session's user, and `tessaro-kiosk.service` is a
  plain system unit with no PAM session.
* **The kernel was missing drivers**, now all in
  `recipes-kernel/linux/files/tessaro-devices.cfg` and all built in rather than
  `=m` so no `kernel-module-*` package has to be chased into the image:
  `HIDRAW` (there is no `/dev/hidraw*` without it), `HID_MULTITOUCH` (most touch
  panels are HID multitouch and fall back to hid-generic, which does not decode
  multi-finger reports), `USB_ACM` plus the CP210x and CH341 bridges for serial
  (`ftdi_sio` and `pl2303` were already on), and an HCI transport for Bluetooth -
  `CONFIG_BT` and `CONFIG_BT_LE` were on but *every* transport driver was off,
  so the stack had no way to reach a controller and `bluetoothd` found no
  adapter. The bbappend is `linux-yocto_%` only; `raspberrypi3-64` builds
  `linux-raspberrypi` and has not been checked.
* **`--touch-events` defaults to `disabled` on Linux**, not `auto`. Finger input
  still arrives as synthesized mouse events, but `ontouchstart` and
  `navigator.maxTouchPoints` are absent, so a site's own feature detection sees
  no touch at all. `KIOSK_TOUCH=auto` ties it to Weston's `wl_seat` capability.

### Removing the container runtime

Dropping podman was one deleted `includes:` entry in `kas/common/tessaro.yml` -
`meta-moonforge-podman.yml` - but that fragment was carrying things that had
nothing to do with containers, and some of them had to be put back by hand.
This is worth knowing before including or dropping any other upstream fragment.

* **`ca-certificates`** was enabled in exactly one place in the tree: that
  fragment. It is now an explicit `RDEPENDS` of `tessaro-kiosk`. Losing it is
  invisible at build time and total at runtime - every https probe fails
  verification and the device sits on the offline page forever.
* **`meta-python`** was likewise enabled only there, and
  `LAYERDEPENDS_networking-layer` names it, so NetworkManager's layer stops
  parsing without it. It is now listed next to `meta-networking` in
  `kas/common/tessaro.yml`. This one at least fails loudly.
* **`seccomp`** rode along in `DISTRO_FEATURES:append = " virtualization seccomp"`
  and looked like another thing to rescue - systemd's `PACKAGECONFIG` keys off
  it, and losing it would silently turn every `SystemCallFilter=` into a no-op
  that still parses. It turned out to be redundant: oe-core's
  `DISTRO_FEATURES_DEFAULT` has carried `seccomp` since scarthgap, so the
  fragment was only ever re-stating it. Re-adding it in `tessaro.conf` would
  have been worse than nothing, because oe-core removes it again per
  architecture (`:remove:riscv32` and friends) and an unconditional append
  overrides that. Check `bitbake -e <recipe> | grep '^DISTRO_FEATURES='` rather
  than assuming either way. Only `virtualization` actually went.

What left for free, with nothing to unwind: `meta-moonforge-podman` and
`meta-virtualization`, `podman` and `podman-compose`, `container-host-config`
with its `storage.conf` (`graphroot = /data/containers/storage`) and its
tmpfiles line, and the `meta-filesystems` layer, which nothing else enables.
The fragment's `meta-oe` and `meta-networking` were already enabled elsewhere. There was never an fstab entry, mount unit or wic partition for
`/data/containers` - it was a plain directory inside the `/data` filesystem.
`IMAGE_DATA_MIN_SIZE` stays at 4096M: the Chromium profile is what dominates
it, not the container storage.

## Updating a device

**In place, without A/B partitions and without signing, from the same
`.wic.bz2` and `.wic.bmap` that `image:flash` writes.** `tessaro-ctl --node
NAME update send IMAGE.wic.bz2` (or `mise run image:update NAME`) and the
device reboots into it; settings, the claim and the browser profile stay.
`--wipe-data` re-creates `/data` as well, and the device comes back unclaimed
with a new identity. Only the blocks the bmap lists are written, and only to
the root partition, plus the kernel file on the boot partition.
`--repartition` writes the whole disk instead - see **Rewriting the whole
disk** below.

1. **Upload.** 4 MiB base64 chunks over the control protocol (`update-begin`,
   `update-chunk`), each fsynced to `/data/tessaro/update/upload.part` before
   it is acknowledged. The same command run again resumes from the last byte
   the device has. The partition table is checked against the device's own
   after the first chunk, so a wrong image fails in seconds, not after 270 MB.
2. **Verify, then prepare**, in the agent, on a thread of its own at idle
   CPU and I/O priority while the kiosk keeps running. `verifying` checks the
   whole file against its SHA-256 (`update send --no-verify` skips it - the
   bmap's checksums below still cover every block that gets written);
   `preparing` is a dry run: one pass over the decompressed image, each bmap
   range up to the end of root checked against the bmap's own SHA-256, and
   the mapped parts of root re-cut into 4 MiB chunks with checksums of their
   own (`manifest.json`, with the upload's own SHA-256). Nothing of the image
   is kept but the kernel, copied out of a loop mount of the image's boot
   partition, staged sparse in `boot.img` for as long as that takes. The
   upload itself stays: it is what gets written. `/data` in the image is
   never decompressed. ~734 MB of the 4096M root partition was
   mapped on qemu when last measured.
3. **Commit** writes `pending` and reboots.
4. **Apply**, in the initramfs (`/init.d/80-tessaro_update`, before
   `90-rootfs`, so nothing has the root partition mounted): `tessaro-flash`
   checks the upload against the manifest's SHA-256 and the staged kernel
   *before the first write*, then decompresses the upload again, checks each
   chunk against its hash as it writes it, drops the device's page cache and
   reads everything back, then installs the kernel as `<name>.new` and
   renames it over the old one, and reboots into it. Decompressing is the
   slow part: seconds on x86, likely a few minutes on the Pi, with the screen
   showing the console's progress. The logic is
   `agent/update/src/{image,apply,flash,wipe}.rs`.
5. **Report**: the boot oneshot puts the result in `journalctl -t
   tessaro-config` once; `tessaro-ctl update status` shows it until the next
   update. `tessaro-ctl device status` shows `PRETTY_NAME`/`IMAGE_VERSION`.

Things to know:

* **A power cut is survivable at every step but one.** Before the first
  write the old system is intact. From the first write on, the marker and
  the upload are still on `/data`, so the next boot writes everything again
  - the half-written root is never mounted. `pending.started` is set just
  before the first write, so staging that stops verifying after that is not
  taken as a reason to boot the (gone) old root. Five attempts, then it stops
  on the console asking for a reflash. The one unprotected path is the ESP -
  a kernel that does not boot - and the bootloader is never touched.
* **The upload is checked whole before the initramfs writes a byte**, and
  that is what makes decompressing it twice safe. The dry run proved the
  file decompresses to what the bmap says; the SHA-256 check in the
  initramfs proves it is still that file. Without it, a file damaged on
  `/data` between the dry run and the apply would only show up at the chunk it hits, after
  root was half written, and every retry would hit it again.
* **Every identifier the rootfs names is pinned in the x86 wks files, and
  that is load-bearing.** wic makes new ones on every build otherwise: the
  PARTUUIDs (`--uuid`) go into grub.cfg's `root=`, and the `/boot` vfat
  serial (`--fsuuid`) into the rootfs's `/etc/fstab` as a `UUID=` line. The
  partition table and the ESP's filesystem are never
  rewritten, so an unpinned new rootfs would name another build's `/boot` and
  fail `local-fs.target`. The pins differ per machine, which makes the layout
  check a machine check too. qemux86-64 now uses our own copy of Moonforge's
  wks for this.
* **The Pi cannot pin its disk signature** - this wic has no `--diskid`, and
  derives it from `SOURCE_DATE_EPOCH` - but nothing there names a PARTUUID
  (`root=/dev/mmcblk0p2`, device nodes in fstab), so for MBR images the
  layout check compares partition geometry instead.
* **The root partition is a fixed `TESSARO_ROOTFS_SIZE` (4096M), the ESP a
  fixed `TESSARO_ESP_SIZE` (256M), the Pi's boot partition 512M.** A later
  image has to fit the partition already on the disk, and the ESP holds two
  kernels during the swap. Each is sized well past today's ~900M rootfs and
  21M/51M kernels on purpose, since growing one costs a reflash. wic fails the
  build if the rootfs outgrows it.
  Changing any partition is a new disk layout: every device needs one full
  reflash or one `--repartition`, which the updater says in so many words
  when it refuses.
* **The initramfs is bundled into the kernel** (`INITRAMFS_IMAGE_BUNDLE`), so
  the boot partition still has one kernel file to swap and bootimg-efi picks
  it up by itself - as `bzImage-initramfs-<machine>.bin`, which is the
  `KIOSK_KERNEL_FILE` the agent extracts. On the Pi the bundle is installed
  as `Image`, the name `boot.scr` loads. The price: any change to
  `tessaro-flash`, and so to the agent workspace, re-bundles the kernel, and
  every update then swaps it.
* **The initramfs is modelled on `core-image-initramfs-boot`, plus our
  update module** (`recipes-core/images/tessaro-initramfs.bb`, its own
  `inherit image` recipe): udev for `/dev/disk/by-*`,
  90-rootfs, finish. finish `switch_root`s to `/sbin/init`, which is still
  the overlayfs-etc preinit. It finds the ESP and `/data` as partitions 1 and
  3 of root's disk, which every Tessaro wks has. On an ordinary boot the cost
  is a read-only mount of the ESP and of `/data`, and an `ls`.
* **The `/etc` overlay keeps shadowing the image.** A file edited on the
  device stays edited across updates, exactly as it does today - an update
  replaces the lower layer only. `--wipe-data` is the way out.
* **Devices flashed before this cannot take updates** - their PARTUUIDs are
  random and their root partition is sized to its old build. One
  `image:flash` gets them onto the layout.

### Rewriting the whole disk

**`update send --repartition` (`mise run image:update --repartition NAME`) is
`image:flash` over the network**, for a device whose disk layout is not the
image's - the partition sizes changed, or the pins did. The whole image is
written from the partition table on: boot, root and an empty `/data`,
the bmap's mapped blocks only, like bmaptool. It implies `--wipe-data`, so the
device comes back unclaimed with a new identity, and `tessaro-ctl` forgets
it. The upload, the dry run (over the whole image this time, `/data`
included, and with no kernel to copy out) and the commit are the same as a
root update's; what differs is the initramfs (`apply_disk` in
`agent/update/src/flash.rs`).

* **The upload goes into RAM, because the disk update overwrites the `/data`
  it sits on.** tessaro-flash mounts a tmpfs of the upload's size plus 16 MiB on
  `/run/tessaro-update/ram`, copies it in and checks the copy's SHA-256 while
  `/data` is still there, so a refusal up to that point boots the old system
  like any other. The agent refuses at `update-begin` if MemTotal is short of
  the upload plus 192 MiB, and tessaro-flash again if MemAvailable is short
  of it plus 64 MiB. About 270 MB of bz2 on qemu today: the Pi's 1 GB should
  hold it, which is why the compressed file is what is kept, not the
  decompressed image (~750 MB would not fit).
* **A power cut from the first write on needs a physical reflash.** There is
  nothing left to write again from, and the partition table may already
  describe partitions that hold nothing yet. That is the price, and the
  confirmation says so; a write failing half way stops on the console (exit
  4), for the same reason.
* **Nothing may have the disk's filesystems mounted.** tessaro-flash unmounts
  the ESP and `/data` itself after the RAM copy is checked, ESP first so that
  a failure there can still be recorded on `/data`. Then it writes the whole
  disk device (`--disk`, `/dev/sda` or `/dev/mmcblk0`, from the hook).
* **The result lands on the new `/data`.** After the write, `BLKRRPART` makes
  the kernel read the new table (retried: udev in the initramfs can hold a
  partition open for a moment), the new partition 3 is mounted and
  `result.json` goes into it with `wiped_data`, so the boot oneshot reports
  `disk rewritten: NAME ..., /data re-created`. If that part fails, the disk
  is still complete and boots; only the report is lost.
* **The check is that it fits, not that it matches.** `layout::check_disk`
  wants partitions 1 to 3 in the image (the hook finds the ESP and `/data`
  by number), the same kind of table as the device's (GPT or MBR, which is
  also the only machine check left), and an image no larger than the disk.
  Writing the wrong machine's image is possible here and bricks the device
  until a reflash, like `image:flash` of the wrong file.
* **The backup GPT header ends up where the image ended, not at the end of
  the disk**, and `/data` stays at `IMAGE_DATA_MIN_SIZE` - both exactly as
  after `image:flash` with bmaptool, which does not relocate or grow anything
  either. The kernel logs a GPT warning about the backup header and boots.
  `tessaro-ctl storage grow` fixes both afterwards - see **Storage**. A
  device whose `/data` was grown gets the image's size back, empty.
* **It needs the new code on the device first.** The initramfs doing the
  work is the running image's. A device on an image older than this refuses
  `--repartition` as a layout mismatch, and needs one physical reflash.

## Storage

**`/data` is the last partition on every machine, and it gets the rest of
the disk only on command.** Every image carries it at `IMAGE_DATA_MIN_SIZE`,
and `image:flash` and `update send --repartition` leave it at that size
whatever the card or disk holds. `tessaro-ctl storage show` reports the
space past it as unallocated (in `WARN`, with the command to run), and
`tessaro-ctl storage grow` gives it to `/data` on the running device: no
reboot, `/data` stays mounted, the kiosk keeps running. `storage grow
--check` prints the plan and changes nothing; `--yes` skips the question.
`storage partitions` and `storage usage` list the partitions and every
mounted filesystem. The logic is `agent/tessaro-agent/src/storage.rs`; the
client is `agent/tessaro-ctl/src/storage.rs`.

* **The grow is online, and each step is safe to cut.** GPT only, `sfdisk
  --relocate gpt-bak-std` moves the backup header to the real end of the disk.
  `sfdisk -N 3` with `,+` moves the end of partition 3 there (clamped at 2 TiB
  on MBR). `partx -u -n 3` hands the kernel the new size through BLKPG, and
  `resize2fs` grows ext4 online. `BLKRRPART` (`fsutil::reread_partitions`,
  what the initramfs uses) is refused on a disk with a mounted partition, which
  is why the kernel is told through `partx` instead. The partition write is
  one table write, and the kernel journals an online resize, so a power cut
  or an agent restart in between leaves a consistent disk.
* **The plan is made from the disk every time**, from sysfs and the ext4
  superblock, never remembered. The partition step runs when the space after
  `/data` is at least `GROW_MIN` (64 MiB), and the filesystem step when the
  filesystem is that much smaller than its partition. So a grow cut short
  after the partition step is finished by running it again, and a second grow
  on a grown disk says there is nothing to do.
* **Swap is gone from the x86 layouts** so that `/data` is last. It was a
  44M partition after `/data`, useless next to Chromium, and it is what made
  a grow impossible. A device still on that layout is refused with the
  reason; one reflash or `--repartition` gets it onto the new one.
* **A grown `/data` changes nothing for updates.** The root update's layout
  check compares partitions 1 and 2 only, and `--wipe-data` re-creates the
  filesystem on the partition that is there, so it keeps the grown size.
* **It runs like the speed test**: on a `spawn_blocking` thread that streams
  a `StorageGrowEvent` per step, under a 10 minute deadline, one at a time.
  The start, each step and `storage: grew /data from A to B` go to the
  journal at info.
* **`sfdisk`, `partx` and `resize2fs` are `RDEPENDS` of `tessaro-kiosk`.**
  Nothing else in the image carried them.
* **The `storage.*` read-only keys never move a template.** `storage.size`,
  `.unallocated`, `.data_size`, `.data_free`, `.data_used` and `.root_free`
  work as placeholders, and the default debug template shows `/data`'s free
  space. `state::Live::moves` skips them: free space changes with every write
  to `/data`, and following it would re-render and restart onto each change.
  The debug screen re-renders every 5s anyway. `device status` has a `data`
  row with the same numbers.

## Networking

**NetworkManager**, from `meta-networking`, which `kas/common/tessaro.yml`
enables on the meta-openembedded pin Moonforge already carries. It replaces
systemd-networkd outright: `PACKAGECONFIG:remove:pn-systemd = "networkd"` in
`tessaro.conf` stops networkd being built at all, and
`PACKAGECONFIG:remove:pn-systemd-conf = "dhcp-ethernet"` drops the
`80-wired.network` that used to provide ethernet DHCP.

The reason is WiFi. Under systemd-networkd, changing a network in the field
means hand-writing a `.network` file and a `wpa_supplicant.conf`, each in its
own syntax, with no feedback; `nmtui` makes it one screen. The reconfiguration
story is a technician on `getty@tty1` (Ctrl-Alt-F1 - Weston is on tty7), on the
serial console, or over SSH.

Things to know:

* **Ethernet DHCP is still zero-configuration**, but no longer NM's
  `Wired connection 1`: `10-tessaro.conf` sets `no-auto-default=*`, and the
  device's own `tessaro-ethernet-dhcp` takes the first port that comes up -
  see **Network control**. A saved `Wired connection 1` left on an upgraded
  device stays, and loses to priority 100.
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
  to modify the system settings". Same command, different answers, depending on how
  the technician got in. `nmcli general permissions` should read `yes`
  throughout.
* **Split packages only.** The plain `networkmanager` package is `ALLOW_EMPTY`
  and `RRECOMMENDS` every plugin built - ppp, wwan, adsl, ovs, bluetooth,
  cloud-setup. The image names `networkmanager-daemon`, `-nmcli`, `-nmtui`,
  `-wifi`. `nmtui` also needs `PACKAGECONFIG:append:pn-networkmanager = " nmtui"`;
  it is not in the recipe's default and pulls `libnewt` from oe-core.
* **`networking-layer`, not `meta-networking`,** is what
  `LAYERDEPENDS_meta-tessaro-distro` names - the layer's `BBFILE_COLLECTIONS`
  value, same trap as meta-chromium registering itself as
  `chromium-browser-layer`.
* **WiFi drivers and firmware are both per machine, and both already handled on
  the hardware targets.** They are separate things: drivers are
  `kernel-module-*` packages, firmware is `linux-firmware*`. `linux-yocto`
  builds the wifi drivers as modules on every machine here - the qemu package
  feed has `kernel-module-brcmfmac`, `-ath9k` and the rest - but a module is
  only *installed* if something recommends it.
  - `raspberrypi3-64`: `rpi-base.inc` adds `kernel-modules` (every built
    module), and `raspberrypi3-64.conf` adds the bcm43430/43455 rpidistro
    firmware. Nothing to do.
  - `genericx86-64`: meta-yocto-bsp's `genericx86-common.inc` adds
    `kernel-modules linux-firmware`. Drivers are complete, and so is firmware:
    oe-core splits that recipe into a few hundred packages, but
    `populate_packages:prepend` in `linux-firmware_*.bb` makes the base
    package `RRECOMMENDS` every split one, and nothing here sets
    `BAD_RECOMMENDATIONS` for them. So every blob lands in the image (a few
    hundred MB); the newer Intel ones come through `-iwlwifi-misc`. To trim
    it, `BAD_RECOMMENDATIONS` the split packages the board does not need, or
    list only the ones it does. Not yet confirmed on a built genericx86-64
    rootfs.
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

### Network control

**The device manages NetworkManager profiles of its own and switches
between them through ordinary settings**, over the pinned, token-checked
control connection:

| profile | up when |
| --- | --- |
| `tessaro-ethernet-dhcp` | `network.ethernet.mode=dhcp` (the default) |
| `tessaro-ethernet-static` | `network.ethernet.mode=static`, with `network.ethernet.address`, `.gateway`, `.dns` |
| `tessaro-wifi-hotspot` | `network.wifi.mode=hotspot` (the default) and `network.wifi.interface` (`auto` = `wlan0`) exists |
| `tessaro-wifi-client` | `network.wifi.mode=client`, joining `network.wifi.ssid` (`network.wifi.ipv4=dhcp\|static` like Ethernet) |

`tessaro-ctl config set network.ethernet.mode=static network.ethernet.address=192.168.1.50/24
network.ethernet.gateway=192.168.1.1` and `config set network.ethernet.mode=dhcp` switch Ethernet,
`network wifi join SSID` makes WiFi a client (the hotspot goes down), `config set
network.wifi.mode=hotspot` brings the hotspot back, `network.wifi.mode=off` frees the radio.
Profiles made by hand - other ports, anything nmtui saved - are listed by `network
profiles list` and never touched. `network.ethernet.interface` names the managed port;
`auto` leaves the profile unbound, so NetworkManager puts it on the first
Ethernet device that comes up.

**A network change is kept only if the device still reaches the network
afterwards, and it is saved only then.** There is no confirm step, on purpose:
a change is one request, and whether it sticks is the device's decision alone,
so a change that takes the operator's own connection away - re-addressing the
link it came in on - is safe by construction. The client explains a lost
connection and `network last` reads the verdict afterwards. `network profiles
list|show`, `network wifi status` and `network wifi scan` read; `network ping HOST`
pings from the device (streamed, like `network speedtest`); `tessaro-ctl device ping`
times the client's own path to the agent - TCP connect, TLS handshake, round
trips - and, like `device id`, needs no token. The logic is `agent/tessaro-agent/src/nm/` (`profiles.rs`
renders, `txn.rs` switches) and `ping.rs`.

* **The profiles are generated, never saved.** The agent renders them as
  keyfiles into `/run/NetworkManager/system-connections` (0600, the in-memory
  directory NetworkManager reads with the highest precedence) from
  `state.json`, `secrets.json` and the node name, and the boot oneshot renders
  them again before NetworkManager starts (`tessaro-config.service` is
  `Before=NetworkManager.service`). So `state.json` is the truth: a reboot at
  any point comes back on the committed configuration, and nothing managed is
  ever written to `/etc`. Only the selected profile of each pair has
  `autoconnect=true`, at priority 100, so it wins over a hand-made profile on
  the same device. The uuids are fixed, the same on every device.
  NetworkManager flags everything under `/run` as unsaved, so `network profiles list`
  lists these as `(managed)` instead; `(not saved)` on any other
  profile means it really is lost at reboot.
* **One change is one transaction** (`nm/txn.rs`): write `txn.json`, take a
  NetworkManager **checkpoint** on the devices involved (with a 150s rollback
  timer of NetworkManager's own, the backstop if the agent dies), write the
  new keyfiles and reload them, bring profiles down and up, set the NAT,
  verify - and only then run the caller's commit, which writes `state.json`
  (and a staged WiFi password to `secrets.json`), and drop the checkpoint. Any
  failure puts the old keyfiles back, then rolls the checkpoint back; nothing
  is saved. The outcome goes to `last.json`. `config set` answers with the checks
  (`Applied.network`); a rolled-back change is an error with the reason.
  Network keys are always applied: `config set --no-apply` refuses them.
* **Verify means, on the device:** what was brought up reaches ACTIVATED
  (failing fast with NetworkManager's reason - `no secrets (wrong
  password?)`), its device gets a global address, a default route is still
  there if there was one, and `--verify` holds: `gateway` (the default, one
  ping from the interface), `HOST` (a ping), `HOST:PORT` (a TCP connect) or
  `none`. About 90s at most; the client waits 180s. Leaving client mode for
  the hotspot or `off` gives up WiFi's route on purpose, so that one change
  does not require the route.
* **An agent that dies half way is rolled back at its next start** (`recover`,
  from `start_control`, retrying for a minute while NetworkManager comes up),
  onto what `state.json` renders. That covers SIGTERM too: the transaction is
  not waited for at shutdown.
* **The transaction runs on a task of its own**, holding the one-at-a-time
  lock the way a speed test holds its own, so a client that is cut off does
  not stop it - the commit happens anyway. A network change is refused while
  an update waits for its reboot, and the `update-commit` step of `update
  send` is refused during one.
  `device.name` is a network key too: it renames the hotspot.
* **The hotspot is `tessaro-<node name>` (read-only
  `network.wifi.hotspot_ssid`), open while the device is unclaimed.**
  `access claim` gives it a random 16-character WPA2 password, stored in
  `/data/tessaro/secrets.json` (0600, never in `state.json`, never shown by
  `config get` or `config keys`) and shown once with the root password; the profiles are
  re-rendered only after the answer is out (`After::Network`), so a claimer on
  the hotspot gets the password before it drops them. `network wifi
  hotspot-password` makes a new one. Unclaim, revoking the last token, a
  factory reset and the boot oneshot's claim invariant open it again. It is
  WPA2 with CCMP and `pmf=1` (disabled): the Pi's brcmfmac refuses clients
  with PMF on in AP mode, and its WPA3 AP support is broken.
* **Hotspot clients get DHCP and DNS from NetworkManager's own dnsmasq**
  (`ipv4.method=shared`, 10.42.0.x) and, with `network.wifi.nat=1`, NAT through
  NetworkManager's nftables table (`firewall-backend=nftables` in
  `10-tessaro.conf`). `network.wifi.nat=0` is a table of the agent's, `inet
  tessaro-hotspot`, dropping forwarded traffic from the WiFi interface - NM
  1.46 has no per-connection switch - so clients reach the device itself and
  nothing past it. dnsmasq and nftables are `RDEPENDS` of `tessaro-network`;
  the dnsmasq bbappend removes its resolved drop-in (`DNSStubListener=no`,
  which would break every lookup on the device) and never enables its own
  unit (which would hold port 53). The NAT modules are recommended, since
  linux-yocto builds them as modules.
* **WiFi joins are Open, WPA2-PSK and WPA3-SAE.** The security comes from a
  scan, or `--hidden --security`; enterprise (802.1X) and WEP are refused.
  Rejoining the same network keeps its saved password if none is given. The
  password is prompted or read from stdin, never argv, travels as
  `protocol::Secret` (whose `Debug` prints `***`), and is saved only if the
  join holds.
* **nmrs is for reading only.** Saved profiles, access points and WiFi
  devices come from it; checkpoints, activation and reloading are our own
  proxies (`nm/proxy.rs`) on nmrs's connection. It costs 19 crates - it asks
  for zbus's default features, which bring async-io, async-executor, blocking
  and polling back - compiled but idle: zbus still runs on tokio, and the nmrs
  calls that start futures-timer's thread are never made.
* **`network ping` falls back to a raw socket.** It prefers the kernel's ICMP
  datagram sockets, but `net.ipv4.ping_group_range` does not exempt root: the
  kernel's own `1 0` refuses even uid 0 (systemd's default opens it). Refused,
  the agent opens a raw socket, which root may, sets the identifier and the
  IPv4 checksum itself and strips the IP header from replies. The e2e runs
  both.
* **qemu cannot exercise WiFi** - no emulated wireless NIC - so the e2e checks
  the hotspot's keyfile and NAT table, and joins, scans and the hotspot
  itself are tested on the Pi by hand.

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
  of RAM shared with the GPU and Chromium is far heavier than the WPE browser
  it replaced, so memory is the constraint to plan the Pi target around.
  `VC4DTBO` is set to `vc4-kms-v3d` (full KMS, see **Display hotplug**);
  `GPU_MEM` is noted in the fragment and left at `rpi-base.inc`'s 64, since
  under full KMS the GPU draws from the CMA pool instead.
* **The Pi is no longer blocked by the agent.** It used to be: the agent
  shipped as an amd64 container archive built by `docker build` on the build
  host, and the task that produced it refused any non-x86_64 machine rather
  than ship something podman on the Pi could not start. `tessaro-agent` is
  cross-compiled by bitbake like everything else, and `mise run build-rpi`
  builds the full image.
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
* **Artifacts are named `tessaro-os-qemux86-64-0.*`**: the `tessaro-os` prefix
  is `IMAGE_BASENAME` in `moonforge-image-base.bbappend` (it defaults to `${PN}`,
  which would name the product after the upstream recipe), and the `-0` is
  `IMAGE_VERSION: "0"` under `env:` in Moonforge's `meta-moonforge-distro.yml`
  kas fragment. The stable symlink is
  `tessaro-os-qemux86-64.rootfs.*`, and the mise tasks depend on that `.rootfs`
  spelling. The bitbake target is still `moonforge-image-base` - only the
  output is renamed.
* **Moonforge's `STRUCTURE.md` is stale in places** - e.g. it documents the
  kiosk browser as Cog with `WAYLAND_COG_LAUNCH_URL`, while the layer actually
  ships `wpe-simple-launcher` with `WPE_SIMPLE_LAUNCHER_URL`. Tessaro runs
  Chromium through its own units, and neither variable is involved. Trust the
  layer sources over upstream docs.

## Status

One fragment per target in `kas/machine/`. Every target carries the same
image: read-only rootfs, overlayfs `/etc` on `/data`, Weston and the Chromium
kiosk.

| Machine | Purpose | State |
| --- | --- | --- |
| `qemux86-64` | development, boots through `mise run run-vnc` | builds and boots |
| `genericx86-64` | shipping x86_64 hardware (UEFI) | configured, never built end to end |
| `raspberrypi3-64` | Raspberry Pi 3 Model B+ | builds, boots and runs the kiosk on a 3B+, rendering on the GPU (ES 2.0) |

"Configured" means the kas chain resolves and bitbake parses it with the right
`DISTRO`/`MACHINE`/`WKS_FILE`; `genericx86-64` has not been built or booted on
real hardware yet. Expect its first build to surface fetch or packaging issues
that parsing cannot.

Images are written from a workstation, not from the build host: `mise run
image:pull` rsyncs the `$TESSARO_MACHINE` image and bmap from
`$TESSARO_BUILD_HOST` into the repo root (gitignored), `mise run image:flash`
writes it with bmaptool, and
`mise run tunnel` holds the VNC/SSH port forwards. Their settings live in the
gitignored `mise.local.toml`; see README.md. That is the manual path now: a
device already running an image with the update layout is updated over the
network with `mise run image:update NAME` (it pulls first, and uses a
`tessaro-ctl` built from the checkout) - see **Updating a device**.
