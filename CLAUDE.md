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
| `mise run image-sizes` | Size of every built `.wic`, all machines at once |
| `mise run clean` | Drop build artifacts, keep sstate and downloads |
| `mise run agent-test` | `cargo test` for the agent workspace (protocol, agent, ctl) |
| `mise run agent-lint` | `cargo fmt --check` plus clippy for the workspace |
| `mise run agent-integration` | The agent against a real headless Chromium, control plane in a sandbox |
| `mise run build-ctl` | Release `tessaro-ctl` for this host, to manage devices remotely |
| `mise run agent-e2e` | Boot the qemu image, provoke each agent behaviour, assert on its journal |
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
basenames in `kas/machine/`. `run`, `run-vnc` and OVMF are qemu-only and refuse
to run on anything else. `build` has no prerequisites beyond kas.

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
`TERM=dumb`, and with `--color never`, so no call site ever checks. Three
rules: styles decorate text and never change it, so the plain output stays
parseable; `--json` output is never styled; and aligned columns use
`style::pad`, which pads *inside* the escape codes, because `{:<N}` around a
painted string counts the escape bytes. Both crates were already in the lock
through clap, so this cost no new crates.

After changing any `Cargo.toml` in the workspace or `agent/Cargo.lock`,
regenerate the crate list the recipe requires and commit it:

```sh
mise run shell                          # then, inside:
bitbake -c update_crates tessaro-kiosk  # writes tessaro-kiosk-crates.inc
```

`do_compile` runs `cargo build --frozen` with no network, so `Cargo.lock` and
`tessaro-kiosk-crates.inc` have to agree or the build fails at fetch time.

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
image features in `meta-tessaro-distro/recipes-core/images/moonforge-image-base.bbappend`;
kiosk supervision behaviour in `agent/`, which is ordinary Rust and not a Yocto
concern at all. The image recipe itself stays upstream's - do not fork it.

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
and CDP gives the agent a real health channel where cog's D-Bus surface was
write-only. The cost is footprint - the Pi 3B+ with its 1GB is likely to OOM,
and a full build takes hours.

The layers arrive through `kas/repo/meta-chromium.yml`, which pins meta-browser
(repo root is not a layer, so `layers:` is mandatory, same pattern as
`kas/repo/meta-yocto.yml`), plus its two dependencies: meta-clang and the Rust
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

Change a setting with `tessaro-ctl set KEY=VALUE`; it restarts exactly what
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
* **The wrapper contributes exactly three flags**, pinned by `tessaro.conf`:
  `--kiosk --no-first-run --ozone-platform=wayland`. Everything else is in
  `tessaro-kiosk.service`. Keep it that way - flags in two places is how
  `--incognito` went unnoticed for as long as it did.
* **`--incognito` is gone, and how it was removed matters.** The `kiosk-mode`
  PACKAGECONFIG selects `--kiosk --no-first-run --incognito` as one bundle,
  and incognito threw away cookies, `localStorage` and service worker caches
  on every restart. The tempting fix - drop `kiosk-mode` and pass the two good
  flags from the unit - costs a **full Chromium rebuild**: `PACKAGECONFIG` is a
  direct vardep of `do_configure` even for an option that expands to nothing,
  so removing it changes that basehash and everything downstream. Measured with
  `bitbake -S printdiff chromium-ozone-wayland`, which names the culprit
  outright (`Variable PACKAGECONFIG value changed: ... [-kiosk-mode-] ...`).
  So `kiosk-mode` stays and `tessaro.conf` overrides
  `CHROMIUM_EXTRA_ARGS:pn-chromium-ozone-wayland` instead - that variable is
  only read by a `sed` in `do_install`, so the cost is do_install onward. The
  recipe's own `:append` of `--ozone-platform=wayland` still lands after our
  value, which is why all three end up in the wrapper. **Run that printdiff
  before touching either line.**
* **Two flags are consequences of losing incognito, not preferences.**
  `--hide-crash-restore-bubble`, or an unclean shutdown puts a "Restore pages?"
  bubble on a public screen; and `--disk-cache-size`, because the profile now
  grows on `/data`.
* **Flags an operator may need are variables, not constants.**
  `KIOSK_CHROMIUM_ARGS_EXTRA` (unbraced `$VAR` in `ExecStart`, so systemd
  splits it at whitespace), `KIOSK_TOUCH` and `KIOSK_ENABLE_FEATURES` all come
  from the same two env files as everything else. Two rules worth keeping:
  a flag only belongs in the unit's fixed set if `KIOSK_CHROMIUM_ARGS_EXTRA`
  can *counter* it - `--disable-pinch` is not in it because Chromium 147 has no
  `--enable-pinch` - and every `base::Feature` goes through
  `KIOSK_ENABLE_FEATURES`, because duplicate `--enable-features` switches do
  not merge and the last one silently wins. The two IME switches are the one
  documented exception to the first rule - see **On-screen keyboard**.
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
* **The agent touches four paths**: the system bus socket
  (`/run/dbus/system_bus_socket`, for RestartUnit), `/run/tessaro-kiosk`
  (it stages the offline page there), and `/data/kiosk` plus
  `/usr/share/tessaro-kiosk` as page sources. These used to be bind mounts
  into a container and are now just paths.
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
  differs from `KIOSK_URL`'s, logging where it had gone. Same-origin
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
  moment `/usr/bin/chromium` is exec'd - seconds before Chromium opens its
  DevTools port. The agent's `After=` on it therefore guarantees nothing, and
  the first cycle after every boot finds port 9222 closed. Logging that at
  info put two lines that read like faults into every device's journal on
  every boot, so `report_cdp_failure` in `agent.rs` holds them at debug until
  the `seen_alive` flag flips. Nothing real is hidden: the escalation still
  logs its restart at info. Do not "fix" this by making them unconditional.
* **A slow-starting browser gets restarted once.** A failed navigation bumps
  `ping_fails` as well as a failed liveness check, so the default
  `KIOSK_PING_FAILS=3` is really reached after two cycles, not three - about
  60s at the default probe interval. That is fine on x86; on the Pi, where a
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

* **`deadline::within` is the only timeout in the program.** `agent/clippy.toml`
  refuses `tokio::time::timeout` everywhere else, so every deadline names its
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
* **The CDP session reconnects on its own and logs only at debug.** Every
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

`mise run agent-e2e` boots the qemux86-64 image and runs
`test/e2e/agent_e2e.rb` against it: each case provokes one thing the agent
exists to handle and asserts on the lines it writes to its journal - the site
going down, the page wandering off the origin, a crashed renderer, a killed
browser, a wedged browser, an operator-stopped unit, a DNS server that
swallows queries, the agent itself wedging, a short agent stall, a parked
agent, SIGTERM - and the control plane: a `tessaro-ctl set` that restarts the
agent onto the new value, the debug screen and maintenance mode each on and
off with the browser left running, a claim and unclaim round trip, an ssh key
authorized with a pinned host key, then revoked and cleared by unclaim, and a resolution
change that refuses an unoffered mode and reverts unconfirmed. Last, because
each reboots the VM, three image updates of the image it booted from:
damaged staging refused at boot with nothing written, an update that keeps
the settings, and one with `--wipe-data`. About twenty minutes; exits non-zero on any failure and prints
the journal lines the failing case saw. `ruby test/e2e/agent_e2e.rb --list`
names the cases, `--only a,b` runs some, `--boot --keep` leaves the VM up, and
without `--boot` it reuses a VM left up that way.

* **It boots its own VM, not through `mise run run`.** The guest is driven over
  SSH, and runqemu's slirp forwards `127.0.0.1:2222` - but inside the kas
  container's network namespace, where `-p` publishing cannot reach it. The
  harness passes `--network=host` so that loopback is the host's. An
  unclaimed device's empty root password means no credential is involved -
  which is why the suite never leaves the device claimed: its claim case
  claims, checks and unclaims inside one SSH command, with a local-socket
  unclaim in a `trap`.
* **It retunes the agent for the run** with `tessaro-ctl set --no-apply` over
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
with `tessaro-ctl set kiosk.url=...`.

`meta-tessaro-distro/recipes-browser/tessaro-selftest/` ships one static page at
`/usr/share/tessaro-selftest/index.html`, with its media beside it. It exercises
rendering, fonts, emoji, every `<input>` type, touch and mouse scrolling plus
multi-touch, WebSerial and WebHID, audio and video playback, and WebAudio
synthesis - from local files, with the network down. Passive checks grade
themselves in a strip at the top; interactive ones stay `pending` until someone
actually does something. To get back to it on a deployed device,
`tessaro-ctl set kiosk.url=http://127.0.0.1/`, and `unset` it afterwards.

* **It is served by nginx, and that is not a preference.** A `file://` page has
  a null origin, and `SerialAllowAllPortsForUrls` /
  `WebHidAllowAllDevicesForUrls` match on origin and nothing else - so the
  self-test could never be pre-granted a device, and the chooser dialog would
  be the only route. `http://127.0.0.1` is a real origin *and* a potentially
  trustworthy one, so the grants apply and the page is a secure context, which
  is itself a precondition for `navigator.serial` existing at all. The page
  still handles the `file://` case and says what is missing.
* **The policy lists two origins, not one.** `TESSARO_DEVICE_ORIGINS` in
  `tessaro-kiosk_1.0.bb` is `TESSARO_KIOSK_ORIGIN` plus
  `TESSARO_SELFTEST_ORIGIN`, deduplicated - one entry on a factory image where
  they are the same string, two on a customer image. That second entry is what
  keeps the diagnostic page able to open a serial port on a *deployed* device.
  `TESSARO_SELFTEST_ORIGIN` must match the `listen` line in
  `tessaro-selftest`'s nginx conf.
* **nginx serves from `/usr/lib/nginx/conf.d/`, not `/etc/nginx/conf.d/`**, for
  the same reason everything else does: `/etc` is an overlay upper on `/data`.
  `recipes-httpd/nginx/nginx_%.bbappend` adds that include to `nginx.conf` -
  which has to stay in `/etc`, its path being compiled in by `--conf-path` -
  and deletes the stock `default_server` symlink, which would otherwise answer
  on `0.0.0.0:80` with the nginx welcome page. Ours binds `127.0.0.1` only.
* **`tessaro-ctl set agent.refresh_interval=0` before a manual pass.** The agent
  re-navigates on that timer, 600s by default, and a reload closes any serial
  port the page has open and wipes every form value. Put it back afterwards:
  on a real site the periodic reload is what recovers a stale page.
* **The agent probes the local server**, because `KIOSK_URL` is now http - so
  nginx dying puts the offline page on screen like any other outage, rather
  than going unnoticed.
* **The on-screen keyboard only appears on a device with no keyboard**, so the
  text inputs need a USB keyboard or `display.osk=always` - see **On-screen
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
resolved to the same three faces and every emoji anywhere on the kiosk was a
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
  It writes the `[input-method]` section too - see **On-screen keyboard**.
* **Its log is `journalctl -t tessaro-weston-config`, not `-u weston`.** It runs
  as `ExecStartPre=`, and those lines do not come back under the unit even
  though the compositor's own do. Every decision it makes - connector, scale
  and why, keyboard and why - is one line there.
* Scale is `display.scale` (`KIOSK_SCALE`) if set, otherwise 2 above 3400px
  wide and 1 below - measured on the mode being set, if one is. `none` writes
  no `scale=`.
* **Resolution is `display.resolution` (`KIOSK_RESOLUTION`)**: `preferred`, or
  a `WIDTHxHEIGHT` written as `mode=` into each connector's `[output]`. It is
  the one setting that can leave nobody able to see the screen, so it has
  three guards. The agent only accepts a mode some connected connector lists
  in `/sys/class/drm/*/modes` (`tessaro-ctl modes` prints them); the
  generator writes it only for connectors that list it and leaves the rest on
  their preferred mode; and the change is on **probation** - it reverts on its
  own unless `tessaro-ctl confirm` arrives within 60s. The pending change is in
  `state.json`, so it survives the agent restarting with Weston, and the boot
  oneshot reverts a change still pending at boot: a reboot is not a confirm.
  The timer is monotonic, never the wall clock.
* **The technician-facing file is still `/etc/xdg/weston/weston.ini`**, which is
  on the `/etc` overlay and persists. It is the base the generator copies, and
  any connector already named in an `[output]` section there is left alone - so
  a hand-written scale always wins. `/run/weston/weston.ini` is generated and
  must never be edited.
* The empty `ExecStart=` in the drop-in is required to clear oe-core's line
  before replacing it, and `--modules=systemd-notify.so` has to be carried over
  verbatim - `weston.service` is `Type=notify` and hangs without it.
* The `display.*` keys are the ones read by the compositor, so
  `tessaro-ctl set` restarts Weston for them - and with it the browser and the
  agent - rather than just the browser.

### On-screen keyboard

**It was always in the image; nothing was speaking to it.**
`/usr/libexec/weston-keyboard` ships in the `weston` package (oe-core's
`FILES:${PN}` covers `${libexecdir}`, and the `clients` PACKAGECONFIG is on by
default), and Weston launches it unprompted - `text_backend_configuration()`
defaults `[input-method] path=` to `wet_get_libexec_path("weston-keyboard")`.
It never drew anything because Chromium was not asking for it.

Two halves, deliberately split:

* **The browser is put in IME mode unconditionally**, by
  `--enable-wayland-ime --wayland-text-input-version=1` in
  `tessaro-kiosk.service`. Chromium 147 speaks text-input v1 and v3, and
  `kWaylandTextInputV3` is `FEATURE_ENABLED_BY_DEFAULT`, so left alone it binds
  v3, finds no `zwp_text_input_manager_v3` on Weston 13 (v1 and
  input-method-v1, nothing newer) and logs `text-input-v3 not available`. The
  version switch is only read when `--enable-wayland-ime` is also present, and
  v3 would not help anyway: `wayland_input_method_context.cc` says outright
  that it "does not support input panel show/hide yet". **These two are the
  exception to the counterable-from-`KIOSK_CHROMIUM_ARGS_EXTRA` rule** -
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
  `tessaro-ctl set display.osk=always`; note `run`/`run-vnc` pass
  `-snapshot`, so that does not survive a reboot of the VM.
* **Keyboard-shaped peripherals will fool it.** A barcode scanner, an RFID
  reader or a KVM dongle enumerates as a USB HID keyboard. `display.osk=always`
  is the answer, which is why that value exists.
* **It fails towards showing the keyboard.** No `udevadm`, an unpopulated udev
  database, anything unexpected: the verdict is "no keyboard" and the panel is
  offered. A superfluous keyboard on screen is a nuisance; a touch-only device
  with no way to type is a brick.
* **The decision is made once, at compositor start.** Hotplug does nothing
  until `tessaro-ctl restart weston`, which takes the browser and the agent with
  it. A udev rule that recomputes and restarts on change is the obvious
  follow-up and is deliberately not built yet - the detection wants proving
  against real peripherals first.
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
  under `KIOSK_VNC` (`on` by default, `off` to disable) - the same file, the
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
  which *is* the TLS path. Hence two things that would otherwise look like
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
  `KIOSK_VNC_PASSWORD` from the same two environment files as every other
  kiosk setting. No account, no `/etc/shadow`, no privilege - and the
  credential can be changed on a running device with no restart, since PAM runs
  the checker on every attempt. It needs `pam-plugin-exec`, which is not in the
  image by default and is an `RDEPENDS` of weston for that reason; without it
  every login fails with a bare `PAM: authentication failed`.
* **Client compatibility is narrow.** VeNCrypt with plain auth means TigerVNC
  or Remmina. macOS Screen Sharing and RealVNC fail in the handshake. The cert
  is self-signed and identical across an image, so the fingerprint warning
  means nothing.
* **Sharing is not free while it is on.** `weston_output_disable_planes_incr()`
  takes the output off hardware overlay and cursor planes for as long as it is
  shared, and every damage rectangle goes through `read_pixels()`. A static
  page is nearly free; full-screen video is a readback per frame. `KIOSK_VNC=off`
  is the first thing to try on a Pi that feels slow.
* **One client at a time** - a second connection disconnects the first - and
  **only outputs present when Weston starts are shared**, so a monitor plugged
  in later needs `tessaro-ctl restart weston`, the same limitation
  `display.osk` has.
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

**`tessaro-ctl --node NAME ssh` is a root shell by key, with no password and
no first-use prompt.** It sends your public key (`~/.ssh/id_ed25519.pub` and
the other ssh-keygen defaults, or `--key PATH`) over the pinned, token-
authenticated control connection; the agent adds it to root's
`authorized_keys` and answers with the device's host key; the client writes
that to `~/.config/tessaro/known_hosts` under `tessaro-<node id>` and execs
`ssh -o HostKeyAlias=... -o StrictHostKeyChecking=yes root@<address>`.
Anything after `--` goes to ssh. `--print` pushes the key and prints the
command instead. `tessaro-ctl ssh-key list` and `ssh-key revoke
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
  answering, and the first `tessaro-ctl ssh` already gets a pin. When no host
  key can be read, the client drops the stale pin and ssh asks as usual.
* **The address is the one the control connection used**, from `nodes.json`
  or mDNS, so a device that moved is found the same way `status` finds it -
  and a different device at the old address fails the TLS pin before any
  key is sent.

### Settings, tessaro-ctl and the claim model

**One management surface.** A device's settings live in
`/data/tessaro/state.json`, the only way to change them is `tessaro-ctl`, and
`tessaro-agent` is the only thing that writes the file. `tessaro-ctl keys`
lists every setting (the registry is `agent/protocol/src/keys.rs`), `get`,
`set KEY=VALUE ...` and `unset KEY ...` do what they say, and each change
restarts exactly what reads the key: the agent restarts itself for an agent
key (invisible on screen), the browser restarts for a browser key or a new
kiosk origin, Weston restarts - taking the browser and agent with it - for a
`display.*` key.

* **`state.json` is sparse**: only what was set, as the registry's dotted
  names. Everything else follows `/usr/lib/tessaro-kiosk/tessaro-kiosk.env`,
  so a later image still moves a default.
* **Writes are locked and survive a power cut.** `flock` on a separate
  `.lock` file (a rename swaps the data file's inode, so the lock cannot live
  on it), the new content to `.tmp` and `fsync`, the current file hard-linked
  to `.prev`, `rename`, `fsync` of the directory. A read falls back from the
  file to `.prev` to the defaults and logs it; a torn file never stops the
  kiosk. `auth.json` uses the same store. Nothing in either file is a
  timestamp: device clocks drift, and `revision` is a counter
  (`set --if-revision N` is compare-and-set).
* **The CLI documents itself.** `tessaro-ctl keys` prints every setting with
  its description, current value or default, what it accepts and what a
  change restarts; `keys KEY` prints one. The text comes from the registry on
  the device (`Kind::describe` plus each key's `doc`), so a client never
  documents settings a device does not have. `tessaro-ctl --help` carries
  worked examples.
* **Custom values and URL placeholders.** `data.NAME=VALUE` defines a custom
  value - the NAME is whatever the site needs, the kiosk gives it no meaning.
  **A placeholder is always a setting's full key in braces**, custom or
  built-in, anywhere in `kiosk.url` - host, path or query - percent-encoded so
  a value cannot change the URL's structure:
  `set 'kiosk.url=https://menu.test/?table={data.table}' data.table=12`.
  There is no short form: `{table}` is refused, with a hint to write
  `{data.table}`. `tessaro-ctl keys` lists every custom value defined, and
  says whether the URL uses it. Expansion happens once, in `state::Effective`:
  the browser unit gets the expanded URL in `generated.env`, and the agent's
  origin checks and the device-API policy see the same one, so a placeholder
  in the host moves the grants too. Built-in settings expand to their
  effective value, set or image default - `{display.osk}`,
  `{browser.fps_counter}` - and `{node.name}` is the name the device actually
  answers to even when none was set. Only `{kiosk.url}`,
  `{maintenance.url}` and `{debug.template}` are refused, as no template may
  contain a template (the debug template alone takes `{kiosk.url}`).
  `maintenance.url` and `debug.template` are templates by the same rules, and
  `set` checks all three whichever one is on screen. Because any setting can move the URL, whether
  the agent restarts is decided by comparing the expanded URL with the one the
  running agent started with, not by which key changed. `set` refuses a
  template with an unset `data.*` or a name that is no setting, and an `unset`
  of a `data.*` still in use; custom values and template can go in one command.
  Nothing is added implicitly - only what the template names.
* **Read-only keys report the device.** `node.id` and `net.*` - `net.ip`,
  `net.netmask`, `net.cidr`, `net.gateway`, `net.dns`, `net.interface`,
  `net.mac`, `net.hostname`, and every address as `net.ipv4`/`net.ipv6`
  (comma separated) - are listed by `keys`, read by `get`, usable as
  placeholders, and refused by `set`. They come straight from the kernel
  (`agent/tessaro-agent/src/net.rs`: `/sys/class/net`, `getifaddrs`,
  `/proc/net/route`, and resolved's own `/run/systemd/resolve/resolv.conf`,
  since `/etc/resolv.conf` is its 127.0.0.53 stub), not from NetworkManager,
  so they answer even when NM is the broken thing. The exception is
  `net.public_ip`, which only the outside world knows. **It is looked up only
  while `kiosk.url` uses `{net.public_ip}`** - a link may be metered - and
  then the agent asks `https://1.1.1.1/cdn-cgi/trace` every 5 minutes (30s
  until it has an answer, and after a failure), keeps it in
  `/run/tessaro-kiosk/public-ip`, and keeps the last address when a request
  fails. So it is empty at the boot render and fills in shortly after.
  `tessaro-ctl net` and `get net.public_ip` look it up on the spot whatever
  the URL uses - one request per ask, up to ~5s when offline - while `keys`
  and a plain `get` only show the last address found this boot. "Primary"
  means the
  interface carrying the IPv4 default route. `tessaro-ctl net` shows the same
  as an overview, `net interfaces` every interface with kind, state, carrier,
  MAC, MTU, speed and addresses. Nothing here edits the network yet.
  **A URL using one moves on its own**: the boot render runs before DHCP, and
  leases change, so while `kiosk.url` uses a read-only key the agent checks
  every 15s and, when the expanded URL is no longer the one it drives,
  re-renders and restarts itself onto it (and the browser, if the origin moved).
* **Values are validated once, at `set`**: enums, ranges, URLs, modes - and no
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
  on by the boot oneshot before the agent starts. `tessaro-ctl factory-reset`
  does the same while the agent runs.
* **Migration**: a leftover `/etc/default/tessaro-kiosk` is imported into
  `state.json` once at boot - valid keys only, the rest named in
  `journalctl -t tessaro-config` - and renamed `.migrated`.

**Two transports, one protocol** (newline-delimited JSON,
`agent/protocol/src/lib.rs`):

* **`/run/tessaro-agent.sock`**, mode 0600 root: no auth, no TLS, full power.
  Not group accessible on purpose - Chromium runs as `weston`, and a
  compromised browser must not be one `connect()` from the control plane.
* **TLS on `api.listen`** (default `0.0.0.0:7400`, `off` disables it). The
  device makes an EC P-256 key and a self-signed certificate in
  `/data/tessaro/tls/` on first boot, valid from 1970 to 9999 so a wrong clock
  cannot break it. Clients **pin** its SHA-256 on first use, keyed by node id,
  and check the pin before any token is sent.

**The claim model:**

* A fresh device is **unclaimed**: no tokens, empty root password. Over TCP it
  answers only `id` and `claim`.
* **The first `claim` wins.** It gets a token and the root password becomes a
  random 20-character one, which `tessaro-ctl` shows exactly once. Order
  matters for power loss: the password is set first, then the token
  committed, so a cut in between leaves "no tokens, a password", which the
  boot oneshot resets to empty. The reverse would leave a claimed device with
  an empty root.
* **Further tokens are issued only against a valid token** (`token create`),
  or over the local socket. Tokens never expire; revoking one deletes it.
  Only SHA-256s are stored, compared in constant time. The device is claimed
  exactly when a token exists, so revoking the last one unclaims it.
* **`unclaim`** removes every token and ssh key and empties the root password;
  **`factory-reset`** also wipes the settings. After either, the first client
  to claim wins again. The TLS key survives both, so pins stay valid.
* `password set` (prompted, or `--random`) changes the root password on a
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
`node.name`. The agent announces `NAME.local` and `_tessaro._tcp` over mDNS
(`mdns-sd`, TXT `id`, `fp`, `ver`, `machine`, `claimed`; `api.mdns=off`
stops it). `tessaro-ctl --node NAME` goes to the address it last saw that
device at first - instant, no scan - and scans mDNS only when nothing answers
there, or when a different certificate or node id does (then with a warning:
the device most likely moved and its old address went to someone else). A
device found at a new address has it updated in `nodes.json`. With an
expected node, its own pin is always checked first, so another known kiosk
answering at that address is a mismatch, never a silent switch.
`tessaro-ctl nodes` lists what answers.
Wiping `/data` or the `/etc` overlay re-identifies a device.

`tessaro-ctl` on a laptop: `mise run build-ctl`, then
`tessaro-ctl --node NAME claim` (or `login --token` with a token someone
issued). Pins and tokens are kept in `~/.config/tessaro/nodes.json`, 0600.

### Maintenance mode

**`tessaro-ctl maintenance on|off`** - the same as `set maintenance.enable=1|0`
- puts `maintenance.url` on screen and leaves `kiosk.url` as it is, so `off`
goes straight back to the site. The default page is
`http://127.0.0.1/maintenance.html` (`TESSARO_MAINTENANCE_URL` in
`tessaro.conf`), shipped by `tessaro-selftest` next to the self-test page,
self-contained so it renders with the network down. It takes `?title=` and
`?message=` as plain text, which is how a device customises it without an
image: `maintenance on --url 'http://127.0.0.1/maintenance.html?message={data.msg}'`
plus `data.msg=...`.

* **The swap is one place, `state::Effective`.** With `KIOSK_MAINTENANCE=1`,
  `KIOSK_URL` *is* the expanded maintenance URL, so every consumer follows it
  without knowing the mode exists: `generated.env` (a reboot in maintenance
  never flashes the site), the agent's navigation and origin enforcement, the
  periodic refresh, `status`, the `url_moved` restart check and the read-only
  key watcher.
* **`KIOSK_PROBE_URL` reads as empty meanwhile**, so the agent probes the
  maintenance page. Probing the site's health endpoint instead would put the
  offline page over the maintenance page the moment the site went down - and
  maintenance is often exactly when it is down.
* **The device-API grants do not move.** `render::device_origins` uses
  `Effective::kiosk_url()`, kiosk.url's origin whatever the mode. Following the
  maintenance page would rewrite the policy on every toggle, restart the
  browser on a public screen and take the site's grants away. So a toggle
  restarts the agent only, which re-navigates; the browser keeps running.

### Debug screen

**`tessaro-ctl debug on|off`** - the same as `set debug.enable=1|0` - swaps the
page for a full-screen text screen: `debug.template` filled in, in large
DejaVu Sans Mono, white on black, shrunk until the longest line fits.
`debug on --template '...'` sets the template in the same change, and
`status` shows a `debug screen` row while it is up. Both keys are agent keys,
so a toggle restarts only the agent, like maintenance mode; the browser keeps
running. `debug.enable` is not `agent.debug`, which is journal verbosity; its
env name is `KIOSK_DEBUG_SCREEN` because `KIOSK_DEBUG` was taken.

* **It wins over maintenance mode, and it is the agent's, not
  `Effective`'s.** Maintenance swaps `KIOSK_URL`, a URL every consumer can
  follow. The debug screen is a page the agent generates, so the agent shows it
  instead of whatever `KIOSK_URL` is (`state::debug_screen`), and nothing else
  changes: not `generated.env`, not the policy. After a reboot the browser comes
  up on `KIOSK_URL` for the few seconds until the agent's first cycle.

* **The template is kiosk.url's templating with raw values.** It accepts the
  same `{key}` placeholders (any setting, read-only or `data.*`), plus
  `{kiosk.url}` itself, expanded. `set` holds it to the same rules: an unset
  `data.*` or a name that is no setting is refused. Values go in as they are,
  not percent-encoded, and `debug.rs` escapes them for HTML
  (`state::expand_text` next to `expand_url`, both on `keys::expand_with`).
  The default shows the name, node id, hostname, the default route, IP, MAC,
  DNS, the public address, every interface (`net.interfaces`, read-only,
  added for this) and IPv6.
* **`\n` - a backslash and an n, as typed - is the line break**, and it is
  the one backslash any value may carry (`Kind::Template` in `keys.rs`). The
  generic no-backslash rule exists because values end up in env files, so two
  things keep that true here. `render::env_file` never writes the template
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
* **`{net.public_ip}` asks Cloudflare only while the screen shows it.**
  `watch_public_ip` treats the debug template as in use only while
  `debug.enable` is on, so the default template costs no request on a
  device that is not in debug mode.

### Speed test

**`tessaro-ctl speedtest` measures the device's link, not the client's.** The
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

### Device APIs: WebSerial, WebHID, WebUSB, Web Bluetooth

**All four are already compiled in; nothing about the browser build needs to
change.** `use_dbus`, `use_udev` (`build/config/features.gni`) and `use_bluez`
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
* **Serial and HID are pre-granted to two origins** by
  `SerialAllowAllPortsForUrls` and `WebHidAllowAllDevicesForUrls` in the policy
  file: the kiosk's own and the self-test's `http://127.0.0.1`, plus any in
  `browser.device_origins`. They collapse to one entry on a factory image,
  where the first two are the same string. These match on *origin* only -
  scheme, host, port, no `[*.]host` wildcards. The image ships them
  substituted at build time (`TESSARO_DEVICE_ORIGINS` in the recipe), and on
  the device **tessaro-agent re-renders the policy from `kiosk.url` as set**,
  from the `/usr/lib/tessaro-kiosk/policy.json` copy, so pointing a device at
  a new origin moves the grants with it. A change of origin therefore restarts
  the browser, not just the agent: Chromium reads the policy at start. The
  render goes through serde, because a syntax error would drop the whole file.
* **WebUSB ships granted to nothing.** It is the only one of the three with no
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
  so that is where it would go. It is on TODO.md, not built.
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
* **The kernel needed four things it did not have**, all in
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
`meta-moonforge-podman.yml` - but that fragment was carrying three things that
had nothing to do with containers, and each of them had to be put back by hand.
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
  and looked like a third thing to rescue - systemd's `PACKAGECONFIG` keys off
  it, and losing it would silently turn every `SystemCallFilter=` into a no-op
  that still parses. It turned out to be redundant: oe-core's
  `DISTRO_FEATURES_DEFAULT` has carried `seccomp` since scarthgap, so the
  fragment was only ever re-stating it. Re-adding it in `tessaro.conf` would
  have been worse than nothing, because oe-core removes it again per
  architecture (`:remove:riscv32` and friends) and an unconditional append
  overrides that. Check `bitbake -e <recipe> | grep '^DISTRO_FEATURES='` rather
  than assuming either way. Only `virtualization` actually went.

What left for free, with nothing to unwind: `meta-moonforge-podman` and
`meta-virtualization`, `podman` and `podman-compose`, and `container-host-config`
with its `storage.conf` (`graphroot = /data/containers/storage`) and its
tmpfiles line. There was never an fstab entry, mount unit or wic partition for
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
the boot and root partitions.

1. **Upload.** 4 MiB base64 chunks over the control protocol (`update-begin`,
   `update-chunk`), each fsynced to `/data/tessaro/update/upload.part` before
   it is acknowledged. The same command run again resumes from the last byte
   the device has. The partition table is checked against the device's own
   after the first chunk, so a wrong image fails in seconds, not after 270 MB.
2. **Verify, then prepare**, in the agent, on a thread of its own at idle
   CPU and I/O priority while the kiosk keeps running. `verifying` checks the
   whole file against its SHA-256 (`update send --no-verify` skips it - the
   bmap's checksums below still cover every block that gets written);
   `preparing` is one pass over the decompressed image that keeps the
   bmap-mapped parts of p1 and p2 - each range checked against the bmap's
   own SHA-256 - in sparse `root.img`/`boot.img`, re-cut into 4 MiB chunks
   with checksums of their own (`manifest.json`). The kernel is copied out
   of a loop mount of `boot.img`. `/data` and swap in the image are never
   read past. ~734 MB of a 1.19 GB root is mapped on qemu today.
3. **Commit** writes `pending` and reboots.
4. **Apply**, in the initramfs (`/init.d/80-tessaro_update`, before
   `90-rootfs`, so nothing has the root partition mounted): `tessaro-flash`
   verifies every staged chunk and the kernel *before the first write*,
   writes, drops the device's page cache and reads everything back, then
   installs the kernel as `<name>.new` and renames it over the old one, and
   reboots into it. The logic is `agent/update/src/{apply,flash,wipe}.rs`.
5. **Report**: the boot oneshot puts the result in `journalctl -t
   tessaro-config` once; `tessaro-ctl update status` shows it until the next
   update. `tessaro-ctl status` shows `PRETTY_NAME`/`IMAGE_VERSION`.

Things to know:

* **A power cut is survivable at every step but one.** Before the first
  write the old system is intact. From the first write on, the marker and
  the staging are still on `/data`, so the next boot writes everything again
  - the half-written root is never mounted. `pending.started` is set just
  before the first write, so staging that stops verifying after that is not
  taken as a reason to boot the (gone) old root. Five attempts, then it stops
  on the console asking for a reflash. The one unprotected path is the ESP -
  a kernel that does not boot - and the bootloader is never touched.
* **Every identifier the rootfs names is pinned in the x86 wks files, and
  that is load-bearing.** wic makes new ones on every build otherwise: the
  PARTUUIDs (`--uuid`) go into grub.cfg's `root=`, and the `/boot` vfat
  serial and the swap UUID (`--fsuuid`) into the rootfs's `/etc/fstab` as
  `UUID=` lines. The partition table and the ESP's filesystem are never
  rewritten, so an unpinned new rootfs would name another build's `/boot` and
  fail `local-fs.target`. The pins differ per machine, which makes the layout
  check a machine check too. qemux86-64 now uses our own copy of Moonforge's
  wks for this.
* **The Pi cannot pin its disk signature** - this wic has no `--diskid`, and
  derives it from `SOURCE_DATE_EPOCH` - but nothing there names a PARTUUID
  (`root=/dev/mmcblk0p2`, device nodes in fstab), so for MBR images the
  layout check compares partition geometry instead.
* **The root partition is a fixed `TESSARO_ROOTFS_SIZE` (2048M), the ESP a
  fixed `TESSARO_ESP_SIZE` (128M), the Pi's boot partition 256M.** A later
  image has to fit the partition already on the disk, and the ESP holds two
  kernels during the swap. wic fails the build if the rootfs outgrows it.
  Changing any partition is a new disk layout: every device needs one full
  reflash, which the updater says in so many words when it refuses.
* **The initramfs is bundled into the kernel** (`INITRAMFS_IMAGE_BUNDLE`), so
  the boot partition still has one kernel file to swap and bootimg-efi picks
  it up by itself - as `bzImage-initramfs-<machine>.bin`, which is the
  `KIOSK_KERNEL_FILE` the agent extracts. On the Pi the bundle is installed
  as `Image`, the name `boot.scr` loads. The price: any change to
  `tessaro-flash`, and so to the agent workspace, re-bundles the kernel, and
  every update then swaps it.
* **The initramfs is `core-image-initramfs-boot` plus one module**
  (`recipes-core/images/tessaro-initramfs.bb`): udev for `/dev/disk/by-*`,
  90-rootfs, finish. finish `switch_root`s to `/sbin/init`, which is still
  the overlayfs-etc preinit. It finds the ESP and `/data` as partitions 1 and
  3 of root's disk, which every Tessaro wks has. On an ordinary boot the cost
  is two read-only mounts and an `ls`.
* **The `/etc` overlay keeps shadowing the image.** A file edited on the
  device stays edited across updates, exactly as it does today - an update
  replaces the lower layer only. `--wipe-data` is the way out.
* **Devices flashed before this cannot take updates** - their PARTUUIDs are
  random and their root partition is sized to its old build. One
  `image:flash` gets them onto the layout.

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
* **The Pi is no longer blocked by the agent.** It used to be: the agent
  shipped as an amd64 container archive built by `docker build` on the build
  host, and the task that produced it refused any non-x86_64 machine rather
  than ship something podman on the Pi could not start. `tessaro-agent` is
  cross-compiled by bitbake like everything else, so `mise run build-rpi` now
  gets as far as Chromium, which is where the real problem was all along.
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
  `IMAGE_VERSION: "0"` in the kas fragment. The stable symlink is
  `tessaro-os-qemux86-64.rootfs.*`, and the mise tasks depend on that `.rootfs`
  spelling. The bitbake target is still `moonforge-image-base` - only the
  output is renamed.
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
| `raspberrypi3-64` | Raspberry Pi 3 Model B+ | configured, never attempted - nothing blocks the build now, but Chromium will likely OOM on 1GB |

"Configured" means the kas chain resolves and bitbake parses it with the right
`DISTRO`/`MACHINE`/`WKS_FILE`; neither image has been built or booted on real
hardware yet. Expect the first build of each to surface fetch or packaging
issues that parsing cannot.

Images are written from a workstation, not from the build host: `mise run
image:pull` rsyncs the `$TESSARO_MACHINE` image and bmap from
`$TESSARO_BUILD_HOST` into the repo root (gitignored), `mise run image:flash`
writes it with bmaptool, and
`mise run tunnel` holds the VNC/SSH port forwards. Their settings live in the
gitignored `mise.local.toml`; see README.md. That is the manual path now: a
device already running an image with the update layout is updated over the
network with `mise run image:update NAME` (it pulls first, and uses a
`tessaro-ctl` built from the checkout) - see **Updating a device**.
