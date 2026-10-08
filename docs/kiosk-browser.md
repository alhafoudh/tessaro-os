# Kiosk browser

**Chromium** (147, `chromium-ozone-wayland` from meta-browser's `meta-chromium`
layer) fullscreen on Weston. It is the only browser that will support the
WebBluetooth/WebSerial/WebUSB APIs on the roadmap, and CDP gives the agent a
real health channel. The cost is footprint - memory is tight on the Pi 3B+'s
1GB - and a full build takes hours.

The layers arrive through `kas/repo/meta-chromium.yml`, which pins meta-browser
(repo root is not a layer, so `layers:` is mandatory) plus its dependencies:
meta-clang and the Rust mixin, a *second* checkout of meta-lts-mixins on its
`scarthgap/rust` branch (Moonforge pins the same repo on `scarthgap/u-boot`).
That mixin also builds `tessaro-agent`, so the agent's rustc version is
Chromium's to choose. Weston/wayland/polkit come from `meta-moonforge-graphics`,
and `/home` on `/data/overlay-home` from our volatile-binds bbappend.

Everything else is `meta-tessaro-distro/recipes-browser/tessaro-kiosk/`:

* `tessaro-kiosk.service` runs Chromium as the `weston` user, with CDP on
  `127.0.0.1:9222` and the profile on `/data/kiosk/chromium`.
* `tessaro-agent.service` runs `/usr/bin/tessaro-agent`, built from `agent/`
  by this same recipe. `Type=simple`, `User=root`, `Restart=always`,
  `WatchdogSec=60` - see [agent.md](agent.md). It is also the device's control
  plane - see [settings.md](settings.md).
* `tessaro-config.service` is a oneshot, `tessaro-agent boot`, ordered before
  Weston, the browser and the agent. It renders the device's settings, keeps
  the claim invariant and is the factory-reset escape hatch.
* `/usr/lib/tessaro-kiosk/tessaro-kiosk.env` carries the build-time defaults
  (`TESSARO_KIOSK_URL` from `tessaro.conf`). What was set on the device lives
  in `/data/tessaro/tessaro.db` and reaches the units as
  `/run/tessaro-kiosk/generated.env`, which the browser unit and
  `tessaro-weston-config` read *after* the defaults. The agent reads only the
  defaults and lays the stored settings over them itself.

The agent watches the browser over **CDP** (`http://127.0.0.1:9222`), on one
persistent session found through `/json/list`: a `Runtime.evaluate` round
trip proves the *renderer* is alive, `Page.navigate` navigates, and the
offline page is served as `file:///run/tessaro-kiosk/index.html`. Restarts go
through `org.freedesktop.systemd1` `RestartUnit` over the system bus - no
`systemctl` shell-outs. The state machine: probe cadence vs navigation
cadence, fail threshold before the offline page, one restart per outage,
backoff, `nav_state=unknown` after a browser restart.

Things to know:

* **The CDP port is on the loopback because the binary hardcodes it.**
  `--remote-debugging-address` exists only in `content_shell`;
  `chrome/browser/devtools/remote_debugging_server.cc` binds `127.0.0.1` with
  a `::1` fallback unconditionally. Chromium refuses to open the DevTools port
  with the *default* user-data-dir, so `--user-data-dir` must stay set.
* **The wrapper contributes only `--kiosk --no-first-run
  --ozone-platform=wayland`**, pinned by `tessaro.conf`. Everything else is in
  `tessaro-kiosk.service`, so flags never live in two places.
* **`kiosk-mode` stays in `PACKAGECONFIG`, and `CHROMIUM_EXTRA_ARGS` drops its
  `--incognito`.** The `kiosk-mode` PACKAGECONFIG selects `--kiosk
  --no-first-run --incognito` as one bundle, and incognito throws away
  cookies, `localStorage` and service worker caches on every restart.
  Removing `kiosk-mode` costs a **full Chromium rebuild**: `PACKAGECONFIG` is a
  direct vardep of `do_configure` even for an option that expands to nothing.
  `CHROMIUM_EXTRA_ARGS:pn-chromium-ozone-wayland` in `tessaro.conf` is only
  read by a `sed` in `do_install`, so overriding it costs do_install onward.
  The recipe's own `:append` of `--ozone-platform=wayland` lands after our
  value. **Run `bitbake -S printdiff chromium-ozone-wayland` before touching
  either line.**
* **Some flags exist because the profile persists.**
  `--hide-crash-restore-bubble`, or an unclean shutdown puts a "Restore pages?"
  bubble on a public screen; and `--disk-cache-size`, because the profile
  grows on `/data`. For the same reason the default `KIOSK_DISABLE_FEATURES`
  carries `PersistentHistograms`: with it on, every browser start maps a 4MB
  `BrowserMetrics-*.pma` file into the profile, and a browser killed before
  it has started leaves its file there (the reasoning is in
  `tessaro-kiosk.env.in`).
* **Flags an operator may need are variables, not constants.**
  `KIOSK_CHROMIUM_ARGS_EXTRA` (unbraced `$VAR` in `ExecStart`, so systemd
  splits it at whitespace), `KIOSK_TOUCH`, `KIOSK_ENABLE_FEATURES`,
  `KIOSK_DISABLE_FEATURES`, `KIOSK_FPS_ARGS` (rendered from
  `browser.fps_counter`) and `KIOSK_PRINT_ARGS` (`--kiosk-printing`, rendered
  from `printer.enable`) come from the same env files as everything else. A
  flag only belongs in the unit's fixed set if `KIOSK_CHROMIUM_ARGS_EXTRA` can
  *counter* it - `--disable-pinch` is not there because Chromium 147 has no
  `--enable-pinch`. The IME switches are the documented exception - see
  **On-screen keyboard** in [display.md](display.md).
* **Switches that look right and do nothing in 147:** `--disable-crash-reporter`
  (chromecast and headless only; use `--disable-breakpad`) and
  `--disable-translate` (does not exist; the policy's `TranslateEnabled: false`
  does it).
* **Chromium runs under `dbus-run-session`.** `DBUS_SESSION_BUS_ADDRESS` is
  unset on this image, so libdbus falls back to *autolaunch*, which needs X11,
  and Chromium logs a burst of `Could not parse server address` errors at every
  start. A private session bus silences them; no browser flag does. The wrapper
  becomes the unit's `MainPID`, which is fine - the agent compares whether the
  main pid *changed* - and it exits when the browser exits, so `Restart=`
  still behaves. `--password-store=basic` and `--disable-background-networking`
  are policy: no keyring, no component updater or variations fetches on a
  link that may be metered. The single GCM `DEPRECATED_ENDPOINT` line at
  startup is harmless. This is only the *session* bus - Web Bluetooth talks to
  `org.bluez` on the **system** bus.
* **`proprietary-codecs` is what plays H.264**; it is enabled in `tessaro.conf`.
* **Chromium has no D-Bus control interface.** Everything is CDP.
* **Supervising the browser needs only a handful of paths**: the system bus
  socket (for RestartUnit), `/run/tessaro-kiosk` (the staged offline page),
  and `/data/kiosk` plus `/usr/share/tessaro-kiosk` as page sources. The
  control plane uses many more; `paths.rs` has them all.
* **Diagnostics are journal-only** by design; nothing technical reaches the
  screen. `journalctl -fu tessaro-agent` and `journalctl -fu tessaro-kiosk`.
* **Chromium policy lives in `/etc/chromium/policies/managed/10-tessaro.json`**,
  a path compiled into the binary (`components/policy/core/common/policy_paths.cc`).
  The loader accepts `//` comments and trailing commas, so the file documents
  itself. A syntax error drops the **whole file** with one `SYSLOG(WARNING)`,
  so confirm on `chrome://policy` after editing. The agent renders it
  (`render.rs`), `CACertificates` in it is `tessaro-ctl network certs`
  (see **Certificates** in [networking.md](networking.md)), the URL filter
  lists take `browser.block` and `browser.allow` (see **URL filters**
  below), and anything else is added with `tessaro-ctl browser policies`
  (see **Policies** below).
* **Printing is off in the policy until `printer.enable`**, which sets
  `PrintingEnabled` and `PrintPreviewUseSystemDefaultPrinter`, and adds
  `--kiosk-printing`. Chromium's own printing still does not print: it
  needs GTK, which this build lacks, so the agent takes `window.print()`
  over (see **Pages** in [printing.md](printing.md)); `cups` in its
  `PACKAGECONFIG` and the flag wait for a build with GTK. A browser policy
  cannot set these entries: they are the device's own.
* **`/data/kiosk` is root owned and only `/data/kiosk/chromium` is `weston`.**
  Both come from tmpfiles `d` lines, which re-apply owner and mode every boot.
  `/data/kiosk/offline.html` is a page the agent puts on screen, so a
  weston-owned parent would let the browser user rewrite the page it is shown -
  the same reason `/run/tessaro-kiosk` is root owned.
* **The agent enforces the kiosk *origin*, not the kiosk URL.** Every healthy
  cycle reads the page's current URL (kept current from `Page.frameNavigated`)
  and navigates back if the scheme/host/port differs from the origin the last
  navigation to `KIOSK_URL` landed on, logging where it had gone. A redirect
  right after a navigation is adopted as the kiosk origin (logged once,
  `drifted_origin` in `agent.rs`); a page with no origin, such as
  `about:blank`, counts as drift. Same-origin sub-pages, query strings and
  in-page routing are left alone: matching the whole URL would fight the site
  and loop on any redirect. `KIOSK_ENFORCE_ORIGIN=0` turns it off, and it is
  inert when `KIOSK_URL` is `data:`/`file:`. A browser that will not say where
  it is counts as "cannot tell", never as drift.
* **Navigations log at info** (`navigated to <url>`, the offline page, the
  drift line, `chromium is answering again after N failed checks`), because
  "what is on screen right now" is the question anyone reading this journal
  has.
* **CDP failures are quiet until the browser has answered once.**
  `tessaro-kiosk.service` is `Type=exec`, so systemd calls it started seconds
  before Chromium opens its DevTools port, and the first cycle after every boot
  finds port 9222 closed. `report_cdp_failure` in `agent.rs` holds those lines
  at debug until `seen_alive` flips; the escalation still logs its restart at
  info. Do not "fix" this by making them unconditional.
* **A slow-starting browser gets restarted once.** A failed navigation bumps
  `ping_fails` as well as a failed liveness check, so the default
  `KIOSK_PING_FAILS=3` is reached in the second cycle. On the Pi, where a cold
  first start can take longer, expect one spurious restart and raise
  `KIOSK_PING_FAILS` rather than reworking the counter.
* **The agent degrades gracefully without a system bus**: every `Units` method
  answers as if the unit were stopped and `restart` returns an error, so the
  same binary runs under `mise run agent:integration`. A browser restart is
  still noticed: the CDP session bumps a generation counter when it comes up on
  a new page target (or the same one after a crash), treated exactly like a
  changed `MainPID`. A reconnect to the *same* live page deliberately does not
  bump it - that would turn an agent stall into a pointless reload.
* **TLS is openssl, through native-tls used directly, and getting it wrong is
  quiet.** The HTTP client is hyper's low-level one, not reqwest, whose
  dependency tree would redo the redirect walk and origin parsing the agent
  already has. `native_tls` uses openssl's default verify paths - the image's
  `/etc/ssl/certs` from `ca-certificates` - so there is no compiled-in root
  store. The extra certificate authorities are added to those roots, never
  written into `/etc/ssl` (see **Certificates** in
  [networking.md](networking.md)). The silent failure left is an https URL that never takes the TLS path
  and fails like a dead site; `agent/tessaro-agent/src/http.rs` tests for it,
  and `readelf -d` on the built binaries should show `libssl.so.3`. Not
  rustls: it freezes the roots at build time, and `ring`/`aws-lc-rs` want a C
  toolchain and per-arch asm under bitbake. The one exception is the speed
  test - see [networking.md](networking.md).
* **`ca-certificates` and `meta-python` are explicit on purpose**: no upstream
  fragment we include enables them. `ca-certificates` is an
  `RDEPENDS` of `tessaro-kiosk`; without it every https probe fails and the
  device sits on the offline page. `meta-python` is listed next to
  `meta-networking` in `kas/common/tessaro.yml`, which needs it to parse. Do
  not add `seccomp` to `DISTRO_FEATURES`: oe-core's default already has it and
  removes it per architecture, which an unconditional append would override.

## Policies

**Extra Chromium policies are named documents the agent merges into the one
file it renders**, never files of their own in `managed/`. Chromium merges
the files of that directory key by key, the file that sorts last winning the
whole key (`config_dir_policy_loader.cc`), so a dropped-in file could
override the device-API origins, the proxy or `CACertificates`, or lose
every key to the image's, and a syntax error in it would drop it without a
word.

* **A document is stored as typed**, comments and trailing commas included,
  in `browser_policies` of `tessaro.db` (`policies.rs`, see
  [storage.md](storage.md)), so editing one gives back its own comments. It
  is not a setting: the registry refuses quotes. The names, the size limit and the most documents the device holds
  are in `agent/protocol/src/policy.rs`.
* **Every save is checked the way Chromium would read it, and more**
  (`protocol::policy::check`): one JSON object, keys that are policy names,
  and none of the keys the device sets itself (`managed`), each refused with
  the command that sets it. The clients run the same check before they send,
  and Webconfig a port of it (`webconfig/src/flows/policies.ts`); the
  device's answer is the one that counts. The check does not know which
  names Chromium has, so a misspelt policy is stored and ignored: confirm on
  `chrome://policy`.
* **`strict_json` blanks comments and trailing commas instead of dropping
  them**, keeping newlines, so a parse error's line and column point into
  the text as typed.
* **The documents are in an explicit priority order, and position 1 wins**
  a key others set too. Merge order is the image's base, then the documents
  from the lowest priority up, then the device's own keys
  (`render::policy`). The order is never inferred from names: nobody sees a
  name order, and renaming would silently change which policy wins. A save
  says which keys it overrides of the image's and which it shares with
  another document. A stored document that no longer passes the check
  (edited by hand in the store, or a key the device took over) is left out
  with a journal line, never failing the render.
* **Positions run 1 to the number of documents, with no gaps**
  (`policies.rs`). A new document goes to the bottom, so adding one never
  overrides what is there; `set --position` and `move` place one, the others
  shifting, a position past the last meaning the last; removing one closes
  the gap. The GUI and Webconfig move the selected row up or down one.
* **A change restarts the browser only when the merged file changed**
  (`control/policies.rs`), since most policies are read at start. Saving the
  same text writes nothing and restarts nothing; neither does swapping two
  documents that share no key.
* **Keys are listed sorted everywhere** (`protocol::policy::keys`), never in
  the map's order: serde_json keeps the typed order where a crate in the
  build turns on `preserve_order` (cfspeedtest does, in the agent workspace)
  and sorts where none does (the GUI's), so relying on it made each client
  say something else.
* **A save can name the revision it started from** (the SHA-256 of the text,
  `if_revision`), and is refused once the device's copy has moved on; `""`
  means "only while there is none". The GUI, Webconfig and `tessaro-ctl
  browser policies edit` always send it, so two editors cannot overwrite each
  other; `set FILE` does not, for scripts.
* **`tessaro-ctl browser policies show` without a name is the merged file**,
  each entry with where it comes from: the image, a document, or the device
  (`render::effective`, read back from the rendered file).
* **Stored through an unclaim, gone with a factory reset**, like the
  certificates.

## URL filters

**`browser.block` and `browser.allow` are Chromium's `URLBlocklist` and
`URLAllowlist`, as settings**, so restricting where the kiosk goes needs no
policy document. Chromium does the matching; the device only renders the
lists (`render::policy`). Patterns are Chromium's URL filter format
(`example.com`, `[*.]example.com/menu`, `https://*`, `*`), up to its limit
of `MAX_URL_PATTERNS` each (`keys.rs`).

* **Chromium decides which one is in effect, not the device**: the most
  specific pattern wins (the longest host, then the longest path), and allow
  wins a tie. An allow list on its own does nothing, because everything not
  blocked is already allowed: "only these sites" is `browser.block=*` plus
  `browser.allow`. A blocked page is Chromium's own "blocked by your
  administrator" page (`ERR_BLOCKED_BY_ADMINISTRATOR`), not a navigation home.
* **The settings add to the lists, they do not own them.** A browser policy
  document may still set `URLBlocklist` or `URLAllowlist`: the settings'
  patterns are appended to what the image or the winning document sets,
  each pattern once. Taking the keys over (`managed`) would refuse every
  document that already uses them and drop the stored ones from the render.
* **While anything is blocked, the device's own pages are allowed**
  (`render::own_pages`): the kiosk origin, the local pages' origin (the welcome
  page, the demo and the file store), the maintenance and offline pages, and the
  directory of the shipped offline page and the debug screen
  (`file:///run/tessaro-kiosk/`). This holds for a document's block too, so no
  filter can shut the kiosk out of the pages the agent puts on screen. They
  are allowed in and out of maintenance mode alike, so toggling it never
  changes the policy, which would restart the browser. A `data:` page has no
  pattern to allow.
* **Origin enforcement still runs on top** (`browser.enforce_origin`, see
  above): it already brings the top page back to the kiosk origin, so the
  filters matter for what it lets through - a new window, iframes and
  subresources, or a site that hands visitors to another host with
  enforcement off.
* **A change restarts the browser**, as for any key Chromium reads at start.
  `tessaro-ctl browser policies show` lists a list no document or image sets
  as the device's.
* **The page cannot read them** (see [bridge.md](bridge.md)).

## Remote DevTools

**`tessaro-ctl -n NAME browser devtools` puts the kiosk tab in the
technician's own Chrome DevTools**: it sends the SSH key as `ssh connect`
does and runs `ssh -N -L 127.0.0.1:9222:127.0.0.1:9222` until Ctrl-C
(`tessaro_client::devtools` and `tunnel`). In
`chrome://inspect` the tab is under Remote Target, because 9222 is one of the
ports Chrome discovers without being configured; `--local-port` takes
another, and a port taken here falls back to a free one, which then has to
be added under Configure. tessaro-gui runs the same tunnel as a job from the
Browser page (see [gui.md](gui.md)).

* **Open it from `chrome://inspect`, not from `/json`'s
  `devtoolsFrontendUrl` in a tab.** Chromium refuses a DevTools websocket
  whose `Origin` it has not been told to allow (`--remote-allow-origins`,
  unset here), and `chrome://inspect` connects from the browser process with
  no `Origin` at all. The tunnel keeps the `Host` header at `localhost`,
  which is the other check the DevTools HTTP server makes. The frontend
  itself comes from `chrome-devtools-frontend.appspot.com` for the device's
  Chromium revision, so the technician's machine needs the internet; the
  "inspect (fallback)" link uses their local one.
* **While another DevTools client is connected, the agent leaves the tab
  alone**: no liveness check, probe, navigation, offline page or restart.
  A breakpoint stops the renderer answering `Runtime.evaluate`, and without
  this `KIOSK_PING_FAILS` cycles later the agent restarts the browser under
  the debugger; a page the technician opens is not drift either. The cycle
  logs `a DevTools client is connected; ...` once, and on disconnect `the
  DevTools client disconnected; watching the browser again`, then treats the
  screen as unknown and navigates back to the kiosk URL. `device status`
  and the GUI show it from `Status.devtools`.
* **"Connected" is read from the kernel, because Chromium does not say.**
  `/json/list` has no client count and `Target.getTargets` reports
  `attached`, which the agent's own session always makes true. So
  `cdp/clients.rs` counts the established loopback connections whose remote
  port is 9222 in `/proc/net/tcp` and `tcp6`, minus the agent's own sockets
  from `/proc/self/fd`. dropbear holds the client end of a forward, so an
  idle tunnel is not a client, and an open DevTools window is (its websocket
  stays up). Whether `chrome://inspect` merely listing the target counts
  depends on whether its discovery polling holds a connection open at the
  moment a cycle looks; that is not measured yet. Anything else on the
  device that talks to 9222 counts too, the e2e suite's one-command
  websockets included - for the moment each one lasts.
* **The hold has no time limit.** It lasts as long as the connection, and
  the connection lasts as long as the technician's ssh; closing the tunnel
  or the DevTools window ends it.

## Page zoom

**`browser.zoom` (`tessaro-ctl browser zoom PERCENT`) is Chrome's own
Ctrl+/- zoom, set for every site, and a change restarts the browser.** The
range is Chrome's, 25 to 500; 100 is no zoom. It stacks on `screen.scale`: at
Weston scale 1 and 150% the page sees `devicePixelRatio` 1.5 and a viewport
1.5 times smaller in CSS pixels, drawn over the whole window.

* **It lives in the profile, not in the DevTools session.** Ctrl+/- is
  Chromium's `HostZoomMap`, which the profile keeps in
  `/data/kiosk/chromium/Default/Preferences` as
  `partition.default_zoom_level` (every site) and
  `partition.per_host_zoom_levels` (one site each), keyed by storage
  partition, `x` for the default one. The level is log base 1.2 of the
  factor, so 120% is 1.
* **`tessaro-agent zoom` writes it, as `ExecStartPre` of
  `tessaro-kiosk.service`,** before every start, as the `weston` user
  (`zoom.rs`). Chromium reads the file only at start and writes its own copy
  back, so the key is a `BROWSER` key, and the step runs every time rather
  than once. The per-site levels are dropped at the same time, so a Ctrl+/-
  typed on the device lasts until the next browser start. A file that is not
  JSON is left alone, and every failure logs and lets the browser start.
* **The DevTools protocol cannot set this zoom, and emulation is a different
  thing.** `Emulation.setDeviceMetricsOverride` with only a
  `deviceScaleFactor` keeps the CSS viewport and draws it at a higher
  resolution. With a width and height as well, the fixed viewport outlives
  the session that set it: on Chromium 147 the next agent found the page
  still 1707x960 after the scale reverted, leaving white space on a
  2560x1440 screen until the browser restarted. `Emulation.setPageScaleFactor`
  is pinch zoom, and `--force-device-scale-factor` has no effect on this
  stack (see **Display scaling** in [display.md](display.md)).

## Hardware video decode

**Chromium decodes video on the GPU's decoder where the hardware has one: V4L2
on the Raspberry Pi, VA-API on x86.** It only does so with the
`AcceleratedVideoDecoder`, `AcceleratedVideoDecodeLinuxGL` and
`AcceleratedVideoDecodeLinuxZeroCopyGL` features, which are the default of
`KIOSK_ENABLE_FEATURES`. Without them every frame is decoded by ffmpeg on the
CPU.

* **The features are meta-chromium's own, moved.** The recipe selects them
  and `--use-angle=gles-egl` into `CHROMIUM_EXTRA_ARGS` for its default
  `use-v4l2` and `use-egl` PACKAGECONFIG options. `tessaro.conf` replaces that
  variable to keep `--incognito` out, which drops them too, so the features
  live in the env file and `--use-angle` in `tessaro-kiosk.service`. Setting
  `browser.enable_features` replaces the whole list: keep them in it.
* **On the Pi the decoder is `bcm2835-codec`**, a V4L2 memory-to-memory device
  (`/dev/video10` and up, group `video`, which the unit is in). Chromium's V4L2
  support is compiled in by meta-chromium's default `use-v4l2`. The decoder
  runs on the VideoCore firmware and takes its memory from `gpu_mem`, not from
  the CMA pool the rest of the GPU uses, so `device.gpu_mem` (Pi only, applied
  at the next reboot, see [settings.md](settings.md)) is the knob when decoding
  fails for memory. Every megabyte of it is taken from the browser.
* **On x86 it is VA-API**, which needs Chromium built with `use-vaapi` and
  without `use-v4l2` (`tessaro.conf`, x86-64 only, so the Pi's Chromium keeps
  its hash; Chromium's `media/gpu/BUILD.gn` refuses both at once), libva, and
  a driver for the GPU, which libva picks at runtime.
  Intel's come from meta-intel (`kas/repo/meta-intel.yml`, genericx86-64
  only): `intel-media-driver` (iHD) for Broadwell and newer, and
  `intel-vaapi-driver` (i965) for older GPUs and for the Braswell and Cherry
  Trail Atoms iHD does not support, such as the Dell Wyse 3040's. AMD's is Mesa's radeonsi VA
  driver, from mesa's `va` PACKAGECONFIG; H.264 in it needs `VIDEO_CODECS =
  "all"`, because oe-core's `all_free` leaves out every patented codec.
* **Check it on the device** with `chrome://media-internals` on the page's
  player: the decoder is `V4L2VideoDecoder` or `VaapiVideoDecoder`, not
  `FFmpegVideoDecoder`. `chrome://gpu` lists "Video Decode" as hardware
  accelerated, or says what disabled it; a GPU blocklist entry can be tested
  with `browser.args_extra=--ignore-gpu-blocklist` before changing the image.

## The welcome page

**This is what a factory image opens.** `TESSARO_KIOSK_URL` in `tessaro.conf`
defaults to `http://127.0.0.1/`, which is the welcome page; a deployment
repoints it, at build time or with `tessaro-ctl config set browser.url=...`.

It is `index.html` in `meta-tessaro-distro/recipes-browser/tessaro-selftest/`,
styled like the maintenance page, and shows what someone standing at a fresh
device needs to reach it: the node name, its IPv4 addresses, the hotspot's
SSID while the hotspot is up, whether the device is online, whether it is
claimed and, until it is, the `tessaro-ctl access claim --node NAME` that
claims it, next to a QR code that opens Quick Setup on a phone (see
[quick-setup.md](quick-setup.md)).

The details and setup panels have equal dimensions, with the hotspot name
above the QR code and the setup address below it. Narrow screens stack the
panels and may scroll; wider ones never show a scrollbar. Without a setup QR (including devices with no Wi-Fi interface), the
details panel stands centered on its own; an active protected hotspot's SSID
stays in those details. The claim instruction remains until the device is
claimed.

* **The values come from `/welcome.json`, which the agent keeps current.**
  `watch_welcome` in `control/watchers.rs` writes
  `/run/tessaro-kiosk/welcome.json` every 5s, and at once after a claim, an
  unclaim or a change of online, only when its content changes. nginx serves
  that one file of the run directory; the page asks for it every 5s and dims
  the last values while it gets no answer.
* **Nothing secret goes in it.** Any page on the loopback can read it, so the
  hotspot's password stays out. An unclaimed device's hotspot is open anyway,
  and the QR code (`setup`) is there only then; a claimed one's password comes
  from `tessaro-ctl network wifi hotspot-password`.
* **Everything is inline**, for the reason the maintenance page's is: this is
  shown before anyone set the device up, often with no network.

## The player page

**While a playlist is in use the browser shows `http://127.0.0.1/player.html`
instead of browser.url, and the page plays the playlist in frames, images and
videos of its own** ([playlists.md](playlists.md)). What it changes for the
browser is there: the `frame-unlock` extension loaded with
`KIOSK_EXTENSION_ARGS`, the cookie and storage partitioning policies that keep
its frames first-party, the interactive items' origins in the device grants,
and the probe and periodic refresh in player mode.

## The demo and the local pages

**The demo is at `http://127.0.0.1/demo/`, beside the welcome page, which
opens it.** It shows every feature through the page bridge and carries the
device's diagnostic checks (fonts, emoji, inputs, multi-touch, codecs,
WebSerial and WebHID); how it works is [demo.md](demo.md). The welcome
page, the maintenance page, the player and the nginx server for all of them
are `meta-tessaro-distro/recipes-browser/tessaro-selftest/`, in
`/usr/share/tessaro-selftest`; the demo is `tessaro-demo`, in
`/usr/share/tessaro-demo`, served at `/demo/` by the same server.

* **They are served by nginx because the device grants need a real origin.** A
  `file://` page has a null origin, and `SerialAllowAllPortsForUrls` /
  `WebHidAllowAllDevicesForUrls` match on origin only. `http://127.0.0.1` is a
  real, potentially trustworthy origin, so the grants apply and the page is a
  secure context, which `navigator.serial` requires.
* **The policy lists the local pages' origin next to the kiosk's.**
  `TESSARO_DEVICE_ORIGINS` in `tessaro-kiosk_1.0.bb` is `TESSARO_KIOSK_ORIGIN`
  plus `TESSARO_SELFTEST_ORIGIN`, deduplicated. On a customer image that
  entry keeps the demo able to open a serial port.
  `TESSARO_SELFTEST_ORIGIN` must match the `listen` line in
  `tessaro-selftest`'s nginx conf.
* **nginx serves from `/usr/lib/nginx/conf.d/`.**
  `recipes-httpd/nginx/nginx_%.bbappend` adds that include to `nginx.conf`
  (whose path is compiled in by `--conf-path`) and deletes the stock
  `default_server` symlink, which would answer on `0.0.0.0:80` with the nginx
  welcome page. Ours binds `127.0.0.1` only; the captive portal's server is
  the one other, and answers only the hotspot's subnet.
* **The refresh timer reloads the demo where it is**, rather than going
  back to `browser.url` (**Where it runs** in [demo.md](demo.md)). A reload
  still closes any serial port the page has open and wipes every form value;
  `tessaro-ctl config set agent.refresh_interval=0` before a long manual
  pass, and put it back afterwards.
* **The agent probes the local server**, so nginx dying puts the offline page
  on screen like any other outage.
* **The text inputs need a USB keyboard or `screen.osk=always`** - see
  **On-screen keyboard** in [display.md](display.md).
* **They are separate recipes from `tessaro-kiosk` on purpose.** That recipe
  `inherit`s cargo, so anything added to its `SRC_URI` drags the whole Rust
  build behind every edit to a `<div>`.
* **`media/sample-video-3s-fullhd.mp4` is a stand-in**, generated with ffmpeg
  (H.264 high, 30 fps, 3 s, 1080p, AAC audio), for a playlist that plays with
  the network down (the e2e playlist lane, the player's dev sample).

**Fonts are named explicitly.** `moonforge-image-base.bbappend` installs
`ttf-noto-emoji-color` and DejaVu sans/serif/mono from meta-oe (hence
`openembedded-layer` in `LAYERDEPENDS`). Without them the only TTF family is
`liberation-fonts`, pulled in by `weston`, and every emoji is a tofu box.

## Device APIs: WebSerial, WebHID, WebUSB, Web Bluetooth

The camera is not one of them: `getUserMedia({video})` reads the virtual
cameras of the camera mirrors, granted by `VideoCaptureAllowedUrls`; see
[camera.md](camera.md).

**Every one of them is compiled in.** `use_dbus`, `use_udev` and `use_bluez`
default to true on Linux and the recipe overrides none of them. `bluez5` and
`bluetoothd` come from oe-core's default `bluetooth` `DISTRO_FEATURE`. What
they need from us is kernel drivers and file permissions.

* **The page has to enumerate, not request.** A policy-granted permission is
  only visible to `navigator.serial.getPorts()`, `navigator.hid.getDevices()`
  and `navigator.usb.getDevices()`. `requestPort()`/`requestDevice()` open a
  chooser dialog, and there is nobody in front of a kiosk to click it. This is
  the constraint that makes or breaks unattended device access.
* **Serial and HID are pre-granted to the device origins** by
  `SerialAllowAllPortsForUrls` and `WebHidAllowAllDevicesForUrls`: the kiosk's
  own and the self-test's `http://127.0.0.1`, plus any in
  `browser.device_origins`. They match on *origin* only - scheme, host, port,
  no wildcards. The image ships them substituted at build time, and on the
  device **tessaro-agent re-renders the policy from `browser.url` as set**,
  from the `/usr/lib/tessaro-kiosk/policy.json` copy, so a new origin moves
  the grants with it. A change of origin therefore restarts the browser, which
  reads the policy at start. The render goes through serde, because a syntax
  error would drop the whole file.
* **WebUSB ships granted to nothing.** It has no "allow all" policy, and
  blanket raw USB is a bigger grant, so `WebUsbAllowDevicesForUrls` is an
  empty list with a worked example in the comment. `"devices": [{}]` is a true
  wildcard if that is ever wanted.
* **Web Bluetooth cannot be pre-granted.** No allowlist policy exists in
  Chromium 147, and `DefaultWebBluetoothGuardSetting` only chooses between
  "block" and "ask". A peripheral needs one real chooser interaction by a
  technician, once per device; with `WebBluetoothNewPermissionsBackend` that
  grant persists and `getDevices()` returns it on later boots. The only
  non-interactive route is CDP's experimental `DeviceAccess.selectPrompt`.
  `agent.device_access` already sends `DeviceAccess.enable`, but nothing
  answers a prompt yet.
* **Web Bluetooth is experimental on Linux.** `runtime_enabled_features.json5`
  leaves the Linux bucket at `experimental`, so `navigator.bluetooth` does not
  exist until `KIOSK_ENABLE_FEATURES` names `WebBluetooth`. It ships empty.
* **Blocklists beat policy.** Serial and HID consult their blocklist *before*
  the policy grant. `--disable-features=WebSerialBlocklist` is the escape hatch.
* **The serial grant includes the console UART, so pick ports by USB id.**
  On the Pi, `getPorts()` returns `/dev/ttyS0` first, with an empty
  `getInfo()`. That is the serial console, so `open()` on it fails with
  `NetworkError: Failed to open serial port.` - by design. A page that takes
  `ports[0]` never reaches the USB adapter. Select on
  `getInfo().usbVendorId`; the demo's serial section does that and offers a picker.
* **WebHID lists keyboard-mode devices but never delivers their input.** A
  barcode scanner in keyboard emulation shows up with a single `1:6`
  (Generic Desktop / Keyboard) collection, and Chromium blocks reports from
  protected keyboard collections. Only a scanner exposing a HID POS collection
  (usage page `0x8C`) can be read, and that is a setting on the scanner. Check
  with `hexdump -C /sys/class/hidraw/hidrawN/device/report_descriptor`: HID POS
  contains `05 8c`. A TMS/TEEMI-type scanner (`f126:0288`) never changes its
  descriptor, whatever "HID POS" codes it is given; its CDC serial port is the
  non-keyboard channel it actually has.
* **Device nodes are group-owned, not `uaccess`.** Chromium's device service
  is in-process and opens the node itself, so the `weston` user needs the
  permission directly. `dialout` covers `/dev/tty*`; `70-tessaro-devices.rules`
  puts `hidraw` and `usb_device` in `plugdev` at 0660. The unit names both
  groups in `SupplementaryGroups=`. `TAG+="uaccess"` would *not* work: logind
  grants those ACLs to the active seat session's user, and
  `tessaro-kiosk.service` has no PAM session.
* **Kernel drivers are in `recipes-kernel/linux/files/tessaro-devices.cfg`**,
  built in rather than `=m` so no `kernel-module-*` package has to be chased
  into the image: `HIDRAW` (no `/dev/hidraw*` without it), `HID_MULTITOUCH`
  (hid-generic does not decode multi-finger reports), `USB_ACM` plus the CP210x
  and CH341 serial bridges, and an HCI transport for Bluetooth (`CONFIG_BT`
  alone reaches no controller). btusb is the exception on genericx86-64,
  where `tessaro-x86-wireless.cfg` makes it a module: Intel controllers load
  `intel/ibt-*.sfi` at probe, and built in it probes before the rootfs with
  the firmware is mounted (see the WiFi drivers bullet in
  [networking.md](networking.md)). The bbappend is `linux-yocto_%` only;
  `raspberrypi3-64` builds `linux-raspberrypi` and has not been checked.
* **`--touch-events` defaults to `disabled` on Linux.** Finger input still
  arrives as synthesized mouse events, but `ontouchstart` and
  `navigator.maxTouchPoints` are absent, so a site's feature detection sees no
  touch. `KIOSK_TOUCH=auto` ties it to Weston's `wl_seat` capability.
