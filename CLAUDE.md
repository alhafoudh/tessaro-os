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
| `mise run agent-test` | `cargo test` for the kiosk agent |
| `mise run agent-lint` | `cargo fmt --check` plus clippy for the agent |
| `mise run agent-integration` | The agent against a real headless Chromium |

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

The agent is an ordinary Rust project under `agent/`, built into the image by
the `tessaro-kiosk` recipe. Its tests run on the host (`mise run agent-test`);
mise pins the host toolchain to **rust 1.95.0**, the same version bitbake uses,
because that version is dictated by the Chromium pin
(`meta-lts-mixins-rust` in `kas/repo/meta-chromium.yml`) and not chosen freely.
`mise.toml` also points `CARGO_TARGET_DIR` at `build/cargo-target`: the recipe
fetches `agent/` with a `file://` SRC_URI, so a `target/` inside it would be
copied into `WORKDIR` and hashed on every build.

After changing `agent/Cargo.toml` or `Cargo.lock`, regenerate the crate list
the recipe requires and commit it:

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
  built from `agent/` by this same recipe. Plain `Type=simple`, `User=root`,
  `Restart=always`. Nothing between it and the system.
* `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` carries the build-time defaults
  (`TESSARO_KIOSK_URL` from `tessaro.conf`), `/etc/default/tessaro-kiosk`
  overrides them and ships entirely **commented out**. **Both units read them
  as systemd `EnvironmentFile=`**, in that order, so the browser and its
  supervisor can never disagree about what the kiosk URL is. The `-` on the
  second makes a missing override file a non-event, which is what `/etc` being
  an overlay demands.

The defaults deliberately live under `/usr/lib`, not `/etc`: `/etc` is an
overlayfs upper on `/data`, so the first write to a file there shadows the
image's copy permanently and no later image could move the default again.

Apply a change with `systemctl restart tessaro-kiosk tessaro-agent`.

The agent used to be a Ruby program in a podman container. It is now a Rust
binary, and podman is gone from the image with it - see **Removing the
container runtime** below for what that cost. The behaviour did not change: it
is the same state machine, the same environment variables and the same log
lines. It watches the browser over **CDP** (`http://127.0.0.1:9222`):
`/json/list` plus a `Runtime.evaluate` round trip proves the *renderer* is
alive, `Page.navigate` replaces the old D-Bus `open` action, and the offline
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
  cycle reads the page's current URL out of `/json/list` - one cheap HTTP
  round trip, no websocket - and navigates back if the scheme/host/port
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
  device.
* **TLS is openssl, and getting that wrong is quiet.** `ureq` gates its
  native-tls connector on `cfg(feature = "native-tls")`; the `native-tls-no-default`
  variant compiles the crate in, never references it, links without `libssl`
  and then **panics on the first https request**. The build stays green
  throughout. `agent/src/http.rs` has a test that makes an https request to
  `127.0.0.1:1` purely to prove the provider is wired, and
  `readelf -d` on the built binary should always show `libssl.so.3`.
* **`RootCerts::PlatformVerifier`, not `WebPki`.** The former uses openssl's
  default store, which is the image's `/etc/ssl/certs` from `ca-certificates`.
  The latter would use the Mozilla roots that ureq's `native-tls` feature
  compiles in, freezing the trust store at build time.

### Self-test page

**This is what a factory image opens.** `TESSARO_KIOSK_URL` in `tessaro.conf`
defaults to `http://127.0.0.1/`; a deployment repoints it, at build time or in
`/etc/default/tessaro-kiosk`.

`meta-tessaro-distro/recipes-browser/tessaro-selftest/` ships one static page at
`/usr/share/tessaro-selftest/index.html`, with its media beside it. It exercises
rendering, fonts, emoji, every `<input>` type, touch and mouse scrolling plus
multi-touch, WebSerial and WebHID, audio and video playback, and WebAudio
synthesis - from local files, with the network down. Passive checks grade
themselves in a strip at the top; interactive ones stay `pending` until someone
actually does something. To get back to it on a deployed device, set
`KIOSK_URL=http://127.0.0.1/` and restart both units.

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
* **Set `KIOSK_REFRESH_INTERVAL=0` before a manual pass.** The agent
  re-navigates on that timer, 600s by default, and a reload closes any serial
  port the page has open and wipes every form value. Put it back afterwards:
  on a real site the periodic reload is what recovers a stale page.
* **The agent probes the local server**, because `KIOSK_URL` is now http - so
  nginx dying puts the offline page on screen like any other outage, rather
  than going unnoticed.
* **The on-screen keyboard only appears on a device with no keyboard**, so the
  text inputs need a USB keyboard or `KIOSK_OSK=always` - see **On-screen
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
* Scale is `KIOSK_SCALE` if set, otherwise 2 above 3400px wide and 1 below.
  `KIOSK_SCALE=none` disables the mechanism entirely.
* **The technician-facing file is still `/etc/xdg/weston/weston.ini`**, which is
  on the `/etc` overlay and persists. It is the base the generator copies, and
  any connector already named in an `[output]` section there is left alone - so
  a hand-written scale always wins. `/run/weston/weston.ini` is generated and
  must never be edited.
* The empty `ExecStart=` in the drop-in is required to clear oe-core's line
  before replacing it, and `--modules=systemd-notify.so` has to be carried over
  verbatim - `weston.service` is `Type=notify` and hangs without it.
* `KIOSK_SCALE` is one of the two keys in `/etc/default/tessaro-kiosk` that need
  `systemctl restart weston` rather than `systemctl restart tessaro-kiosk`.
  `KIOSK_OSK` is the other.

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
  `KIOSK_OSK=always` in `/etc/default/tessaro-kiosk` plus
  `systemctl restart weston`; note `run`/`run-vnc` pass `-snapshot`, so that
  edit does not survive a reboot of the VM.
* **Keyboard-shaped peripherals will fool it.** A barcode scanner, an RFID
  reader or a KVM dongle enumerates as a USB HID keyboard. `KIOSK_OSK=always`
  is the answer, which is why that value exists.
* **It fails towards showing the keyboard.** No `udevadm`, an unpopulated udev
  database, anything unexpected: the verdict is "no keyboard" and the panel is
  offered. A superfluous keyboard on screen is a nuisance; a touch-only device
  with no way to type is a brick.
* **The decision is made once, at compositor start.** Hotplug does nothing
  until `systemctl restart weston`, which takes the browser and the agent with
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
  file: the kiosk's own, derived from `TESSARO_KIOSK_URL`, and the self-test's
  `http://127.0.0.1`. They collapse to one entry on a factory image, where
  those are the same string. These match on *origin* only - scheme, host, port,
  no `[*.]host` wildcards - and both are substituted at build time
  (`TESSARO_DEVICE_ORIGINS` in the recipe). **Pointing `KIOSK_URL` at a third
  origin in `/etc/default/tessaro-kiosk` silently voids the grant for it**,
  which is the one sharp edge here; building the image with the right
  `TESSARO_KIOSK_URL` avoids it.
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
| `raspberrypi3-64` | Raspberry Pi 3 Model B+ | configured, never attempted - nothing blocks the build now, but Chromium will likely OOM on 1GB |

"Configured" means the kas chain resolves and bitbake parses it with the right
`DISTRO`/`MACHINE`/`WKS_FILE`; neither image has been built or booted on real
hardware yet. Expect the first build of each to surface fetch or packaging
issues that parsing cannot.

Writing an image to a card or disk is deliberately not a mise task - decompress
with `mise run unpack` and `dd` the `.wic` yourself.
