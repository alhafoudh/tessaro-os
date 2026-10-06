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

## Where the details are

This file holds the rules. README.md is for people using Tessaro;
DEVELOPMENT.md covers building, the workstation flow and running tests. How
each subsystem works, and why, is in `docs/`.
Read the relevant file before changing that subsystem, and update it in the
same change as the behaviour it describes.

| File | Covers |
| --- | --- |
| [docs/build.md](docs/build.md) | kas layout and config chains, platform gotchas (wks, fstab, QEMU, GPU, Pi), target status |
| [docs/kiosk-browser.md](docs/kiosk-browser.md) | Chromium units and flags, CDP supervision, origin enforcement, TLS, remote DevTools, page zoom, self-test page, WebSerial/HID/USB/Bluetooth |
| [docs/agent.md](docs/agent.md) | deadlines, the pledge-fed watchdog, the CDP session, settings on a running agent and which keys still restart it |
| [docs/settings.md](docs/settings.md) | the saved settings, templates and placeholders, read-only keys, the claim model, names, completion, maintenance mode, debug screen |
| [docs/storage.md](docs/storage.md) | the SQLite stores: the device's `tessaro.db` and `sessions.db`, a client's `tessaro.db`, their tables, the pragmas, migrations, a broken store set aside, the `sqlite3` shell |
| [docs/api.md](docs/api.md) | the HTTP API: endpoint types, the OpenAPI document and Swagger UI, the socket and TLS, pinning and Bearer tokens, errors, jobs and log pages, connections |
| [docs/display.md](docs/display.md) | Weston scaling and resolution, hotplug, on-screen keyboard, screen power, boot splash and wallpaper |
| [docs/bridge.md](docs/bridge.md) | the injected script, `window.tessaro` and its modes, who may call, `browser eval` |
| [docs/remote-access.md](docs/remote-access.md) | VNC mirror and its PAM auth, SSH and `ssh connect` keys |
| [docs/audio.md](docs/audio.md) | PipeWire units, how `audio.*` is applied, `auto` |
| [docs/camera.md](docs/camera.md) | the camera mirrors and v4l2loopback, one reader per `Mirror N`, why not PipeWire, hiding the real cameras, how a format is picked, `camera.*`, snapshots and previews, the USB/IP test camera in qemu, what does not work |
| [docs/time.md](docs/time.md) | timedated and timesyncd, how `time.*` is applied, DHCP's NTP servers, the persistent clock, where `time show`'s numbers come from |
| [docs/scripts.md](docs/scripts.md) | scripts: the body file and the fire and run units, triggers and `TESSARO_TRIGGER`, concurrency, how each run is recorded, `script run` as a job, the page's scripts |
| [docs/scheduler.md](docs/scheduler.md) | schedules: the systemd timer each is rendered into and the script it starts, the reconcile of every script and schedule unit, checking `OnCalendar` expressions |
| [docs/hardware.md](docs/hardware.md) | vendor, model, board, CPU, serial and RAM in `device status`: DMI, the device tree, placeholders, where the serial goes |
| [docs/files.md](docs/files.md) | the `/data/files` store served at `/files/` |
| [docs/networking.md](docs/networking.md) | NetworkManager, the managed profiles and their transactions, hotspot, ping, speed test, the proxy (local tinyproxy, what goes through it), extra certificate authorities |
| [docs/updates.md](docs/updates.md) | in-place updates, `--repartition`, growing `/data` |
| [docs/e2e.md](docs/e2e.md) | the qemu RSpec suite: running it, lanes, ports, harness quirks |
| [docs/ci.md](docs/ci.md) | GitHub Actions: the per-push checks, the image jobs shared by the manual e2e run and the manual image build with release, the Try Tessaro DMG built around the run's genericarm64 image, the self-hosted runner and its cache used in place |
| [docs/sbom.md](docs/sbom.md) | the SBOM: what each component is read from (bitbake's SPDX, cargo, npm, vendored files), the bundle and license list a release carries, the license policy and its exceptions, what is not covered |
| [docs/clients.md](docs/clients.md) | what `tessaro-ctl` and `tessaro-gui` share in `agent/client` and what each keeps, reporting without printing, tones, lines and facts, adding a command, the differences on purpose |
| [docs/gui.md](docs/gui.md) | `tessaro-gui`: inner windows, keyboard, the node list, device pages per command group, workers and jobs, the VNC viewer |
| [docs/try-tessaro.md](docs/try-tessaro.md) | Try Tessaro, the Mac app with a device in a VM: what the bundle carries, the relinked QEMU runtime and its licenses, the command line, the disk overlay and reset, ports, the client store entry, the activities |
| [docs/webconfig.md](docs/webconfig.md) | Webconfig: browser sessions, tickets and the activity rule, the handover across restarts, opening it from ctl and the GUI, the pages, the describe port and its golden fixtures, serving and caching, the frontend and its codegen, the dev proxy, the bitbake build and `bitbake-lock.json` |
| [docs/printing.md](docs/printing.md) | the device's CUPS and why it is set up that way, the `printers` table and the reconcile, driverless and raw printers, `printer.enable` and `window.print()`, the default printer, discovery, supplies, sizes, which printers work |
| [docs/playlists.md](docs/playlists.md) | playlists and the player page: when the player is on screen instead of browser.url, the timetable and what plays, preloading and transitions, interactive items and input from frames, the frame-unlock extension, first-party frames and their grants, the media cache |
| [docs/quick-setup.md](docs/quick-setup.md) | the welcome page's QR code, captive portal detection, nginx's redirect to Quick Setup on the API's port, what the page uses, the online indicator |

**Writing docs** (in `docs/` and in this file):

* **Put things where they belong.** A rule every change must follow (a build
  trap, a code convention, a "do not fix this") goes in this file, in a line
  or two. How a subsystem works and why goes in its `docs/` file. A new
  subsystem gets a new file and a row in the table above. Never `@import` a
  doc from here: imported files load every session too.
* **Describe the system as it is now.** No history: not "used to be", "was
  replaced", "no longer", "now", "since", or what a previous version did.
  Git keeps the history. The one kind of past worth writing down is a dead
  end someone would otherwise try again - write it as a present-tense rule
  ("`TAG+="uaccess"` does not work here: ..."), not as a story.
* **Lead with the behaviour and the reason**, in a bold first sentence,
  then the detail. Keep the why: it is what stops the next person undoing
  it.
* **Say each thing once.** A rule that applies across subsystems (the `/etc`
  overlay, the Chromium rebuild traps) lives in this file; a doc refers to
  another doc's section by file and section name instead of repeating it.
* **Leave out what goes stale or what the code already says**: measured
  sizes and durations of the current build, crate counts, lists of keys,
  cases or commands that `--help`, `config keys` or the spec files already
  enumerate. Name the file or command that has the answer instead. Numbers
  that are part of the design (timeouts, limits, reserved sizes) stay.
* **Name paths, functions and upstream sources** so a claim can be checked
  (`render.rs`, `remote_debugging_server.cc`), and prefer what was measured
  over what was assumed.

## GitHub issues

**Every fix or feature asked for in a session is checked against the open
GitHub issues first**, so the work and its issue stay linked instead of the
issue going stale next to a finished change.

* **Before starting**, search the issues (`gh issue list --search
  '<keywords>'`, `gh issue view <n>`). If one matches, name it and ask the
  user whether this work is that issue. Never assume the match.
* **Once confirmed, track it to the end**: read the issue and its comments
  for the requirements, keep the work to what the issue asks, and say which
  parts are done or left when reporting back.
* **When the work is committed**, offer to close the issue with a comment
  that links the commits (`gh issue close <n> --comment 'Fixed in <sha>'`).
  Closing it, and commenting on it, waits for the user's yes, like any
  outward-facing action.

## Planning a feature

**A feature plan covers every surface of the tooling, so Tessaro stays one
coherent product instead of a device whose clients drift apart.** For each
surface below, the plan says what changes and in which files, or why nothing
does there. Nothing is left for a follow-up. The rules further down give the
details for each surface; this is the checklist.

* **The API**: the endpoint types in `agent/protocol/src/api.rs`, the
  regenerated `openapi.json`, and any new setting in `keys.rs`.
* **`tessaro-ctl`**: the command, its group and name (**tessaro-ctl command
  and key structure** below), its `--json` output, and its logic and words
  in `agent/client`.
* **`tessaro-gui`**: the action on its group's page.
* **The page bridge**: the `window.tessaro` call that mirrors it, or the
  reason the page must not have it ([docs/bridge.md](docs/bridge.md)).
* **Webconfig**: the action on its group's page and the ported describe
  functions with their golden fixtures, or the reason it is native-only.
* **Tests**: unit tests next to the code, `agent:integration` when Chromium
  is involved, and an e2e case in the lane whose state it fits, or the
  reason qemu cannot test it.
* **Docs**: the subsystem's `docs/` file (a new file and a row in the table
  above for a new subsystem), README for what users see, and
  `docs:screenshots` when a page it shows changes.

## Commands

Use the mise tasks rather than calling `kas-container` directly:

| Task | Purpose |
| --- | --- |
| `mise run image:build` | Build the image for `$TESSARO_MACHINE` (plus OVMF on qemu) |
| `mise run image:build:qemu` | Same, forced to `qemux86-64` |
| `mise run image:build:x86` | Same, forced to `genericx86-64` |
| `mise run image:build:arm64` | Same, forced to `genericarm64` (plus U-Boot for QEMU) |
| `mise run image:build:rpi3` | Same, forced to `raspberrypi3-64` (Pi 3B / 3B+) |
| `mise run image:build:rpi4` | Same, forced to `raspberrypi4-64` (Pi 4B / 400 / CM4) |
| `mise run image:build:rpi5` | Same, forced to `raspberrypi5` (Pi 5) |
| `mise run image:shell` | Interactive kas shell (cwd is the build dir) |
| `mise run image:clean` | Drop build artifacts, keep sstate and downloads |
| `mise run image:sizes` | Size of every built `.wic`, all machines at once |
| `mise run image:name` | The file name the next build will give the image, `-dirty` included |
| `mise run image:list` | Workstation: every `.wic.zst` on the build host, all machines at once |
| `mise run image:pull` | Workstation: fetch the image and bmap from the build host |
| `mise run image:update` | Workstation: pull the image and update a running device over the network (the normal path) |
| `mise run image:flash` | Workstation, manual: write the pulled image to a card or disk (first install, recovery) |
| `mise run qemu:unpack` | Decompress the `.wic` for runqemu |
| `mise run qemu:run` | Boot in QEMU, serial console on the terminal; `--count N` boots N devices on one network the host sees over mDNS (Linux: a bridge under sudo for the run) |
| `mise run qemu:vnc` | Boot in QEMU with VNC on localhost:5901; `--count N` as above, device n on 590n |
| `mise run qemu:run:arm64` / `qemu:vnc:arm64` | Same, forced to `genericarm64`; on a Mac on vmnet for mDNS, `--no-vmnet` for the 127.0.0.1 forwards, `[IMAGE]` for a release image |
| `mise run usbcam:run -- [CLIP] --attach` | A fake USB webcam looping CLIP (or a test pattern) over USB/IP, attached to that VM |
| `mise run usbcam:test` | The fake webcam's unit tests |
| `mise run agent:test` | `cargo test` for the whole agent workspace |
| `mise run agent:lint` | `cargo fmt --check` plus clippy for the workspace |
| `mise run agent:integration` | The agent against a real headless Chromium (`agent/compose.yaml`, needs docker compose), control plane in a sandbox |
| `mise run e2e:run` | Boot qemu VMs (E2E_JOBS at a time), provoke each agent behaviour, assert on its journal |
| `mise run e2e:one` | One lane or case of that suite, with plain rspec |
| `mise run e2e:setup` | `bundle install` for the suite's gems |
| `mise run ctl:build` | Release `tessaro-ctl` for this host, to manage devices remotely |
| `mise run ctl:run -- ARGS` | Run that `tessaro-ctl`, rebuilt first if its source changed |
| `mise run gui:run` | Run `tessaro-gui`, the desktop client, from this checkout |
| `mise run gui:build` | Release `tessaro-gui` for this host |
| `mise run gui:test` / `gui:lint` | The `gui/` workspace's unit tests (tessaro-gui, Try Tessaro); `cargo fmt --check` plus clippy |
| `mise run try:run -- [IMAGE]` | macOS: package and open Try Tessaro with a genericarm64 image (default: the pulled one) |
| `mise run try:build -- [IMAGE]` | macOS: `Try Tessaro.app` and its DMG, release, into `build/` |
| `mise run webconfig:setup` | `npm ci` for Webconfig |
| `mise run webconfig:gen` | Webconfig's API types from `agent/protocol/openapi.json` |
| `mise run webconfig:run` | Webconfig's dev server, the API proxied to `TESSARO_WEBCONFIG_TARGET` (default the qemu forward) |
| `mise run webconfig:build` | Webconfig into `build/webconfig`, which `agent:integration` serves |
| `mise run webconfig:test` / `webconfig:lint` | Its unit tests, the golden fixtures and `bitbake-lock.json`; tsc, ESLint and Prettier |
| `mise run player:test` | The player page's unit tests (`player-core.js`), with `node --test` |
| `mise run player:run` | The player page with a sample playlist on http://127.0.0.1:8090, for desktop development |
| `mise run webconfig:lock` | `bitbake-lock.json` from `package-lock.json`, after any change to the dependencies |
| `mise run sbom:build` | The `$TESSARO_MACHINE` image's SBOM bundle and license list into `build/sbom/`, from its built image |
| `mise run sbom:check` | Every crate, npm package and vendored file against the license policy in `sbom/licenses.yml` |
| `mise run sbom:test` | The SBOM tool's unit tests |
| `mise run sources:collect` | The `$TESSARO_MACHINE` image's GPL, LGPL and AGPL sources into the store on the build host (`$TESSARO_SOURCES_DIR`), listed in `build/sbom/` |
| `mise run docs:screenshots` | The README's screenshots: the kiosk's pages and Webconfig (Playwright in docker) and `tessaro-gui` (headless), from the agent's own fixtures with fixed data; rerun after changing any page they show, `qr.rs` or the default debug template |
| `mise run dev:tunnel` | Workstation: autossh VNC/SSH forwards to the build host |

Exit the QEMU serial console with `Ctrl-a x`.

**Real test devices are listed in `DEV_MACHINES.txt`** at the repo root, if
it exists: their names, IPs and how to reach them from this host. It is
per-user and gitignored, so read it before asking which device to use, and
reach a device with `mise run ctl:run -- -n <IP> ...`.

**Task names are `<artifact>:<action>[:<variant>]`**, and an action means the
same thing in every group (`build`, `run`, `test`, `lint`). A new task goes
into the group of the artifact it acts on; `dev:*` holds workstation plumbing
that belongs to no artifact. Renaming a task means a `git grep` over the whole
repo, docs, comments and error messages included.

**Every task acts on one machine**, `$TESSARO_MACHINE`, defaulting to
`qemux86-64` (`image:sizes` and `image:list` excepted). The `image:build:*` tasks set it;
anything else takes it from the environment (`TESSARO_MACHINE=raspberrypi3-64
mise run image:shell`). Valid values are the basenames in `kas/machine/`. Each
machine gets its own TOPDIR under `build/<machine>/`; `cache/` (`DL_DIR` +
`SSTATE_DIR`) is shared. `qemu:*` boots qemux86-64 and genericarm64, the
latter on a Mac too (`scripts/qemu-arm64.sh`); `e2e:*` is qemux86-64 only.

For Yocto work on a single recipe, go through the kas shell:

```sh
mise run image:shell                    # then, inside (cwd is /build):
bitbake -e <recipe> | grep '^VAR='      # resolved value of a variable
bitbake -c cleansstate <recipe>         # force a rebuild of one recipe
bitbake -c devshell <recipe>            # shell in the recipe's build dir
bitbake -n moonforge-image-base         # dry run: what would rebuild
```

**After changing any `Cargo.toml` in the workspace or `agent/Cargo.lock`**,
regenerate the crate list and commit it with the change - `do_compile` runs
`cargo build --frozen` with no network, so the two must agree:

```sh
mise run image:shell                    # then, inside:
bitbake -c update_crates tessaro-kiosk  # writes tessaro-kiosk-crates.inc
```

## Rules for builds and worktrees

Builds are long. Run them in a Herdr pane, not the Bash tool.

* **One build at a time, across every checkout.** Before starting any
  `mise run image:build*` (or `image:shell` with bitbake, or `e2e:*`), check nothing
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
  (`agent:*`, `ctl:build`, `gui:*`) is fine.

## The Yocto side

* **This repo is the kas root repo.** Every other layer is a pinned, gitignored
  checkout. Bumping Moonforge is one commit hash in `kas/common/tessaro.yml`.
  A build is `kas/machine/<machine>.yml:kas/common/debug.yml`; the machine
  fragment includes `kas/common/tessaro.yml` and adds only board specifics.
  Adding a target is one new file in `kas/machine/`.
* **Where product changes go:** system-wide policy in
  `meta-tessaro-distro/conf/distro/tessaro.conf`; packages and image features
  in `meta-tessaro-distro/recipes-core/images/moonforge-image-base.bbappend`;
  kiosk behaviour in `agent/`. The image recipe stays upstream's - do not fork
  it. Prefer a `:pn-<recipe>` override in `tessaro.conf` when a variable is
  all you need.
* **Enable features with kas `includes:`**, never by editing `bblayers.conf`
  (kas regenerates `build/<machine>/conf/`). `local_conf_header` keys merge by
  name across the chain, so never reuse one: ours are `20_tessaro-common`,
  `25_tessaro-machine`, `26_tessaro-pi5`, `30_tessaro-qemu-kiosk`.
* **`distro:` lives in each `kas/machine/*.yml`**, never in
  `kas/common/tessaro.yml`, where an include silently resets it to
  `moonforge`. Check with `kas dump <chain>` after touching includes.
* **Layer dependencies use the `BBFILE_COLLECTIONS` name**, not the directory:
  `networking-layer`, `chromium-browser-layer`. Appends for an optional layer
  go under `meta-tessaro-distro/dynamic-layers/<collection>/`.
* **`DISTROOVERRIDES` is `tessaro`**, so any upstream `VAR:moonforge = ...`
  silently stops applying. Re-check after a Moonforge bump.
* **Dropping an upstream kas fragment can drop unrelated things it carried**
  (podman's took `ca-certificates` and `meta-python` with it). Diff
  `bitbake -e` before and after.
* **Some changes silently cost a full or near-full rebuild.** Check with
  `bitbake -S printdiff <recipe>` or `bitbake -n moonforge-image-base | grep -i
  chromium` before touching:
  * Chromium's `PACKAGECONFIG` (even removing `kiosk-mode`). Browser flags
    go through `CHROMIUM_EXTRA_ARGS` or the unit instead.
  * `DISTRO_FEATURES` or `DISTRO_FEATURES_BACKFILL_CONSIDERED` (flips
    `use_pulseaudio`).
  * systemd's `PACKAGECONFIG`.
* **The partition layout is frozen.** The wks pins (PARTUUIDs, the ESP's
  `--fsuuid`, `--label data`) and `TESSARO_ROOTFS_SIZE`/`TESSARO_ESP_SIZE` are
  what in-place updates rely on. Changing any of them means every device
  needs a reflash or `--repartition`.
* **Pi images boot Linux directly through firmware**, configured in
  `kas/common/raspberrypi.yml`. Keep `root=LABEL=root`, the wks filesystem
  labels and the updater's kernel filename in sync; see **Pi storage boot**
  in `docs/build.md` for media support and migration from U-Boot.
* **`/etc` is an overlayfs upper on `/data`.** The first write to a file there
  shadows the image's copy forever, so shipped defaults and drop-ins go under
  `/usr/lib` (kiosk env, nginx `conf.d`, NetworkManager `conf.d`). The
  exceptions are paths compiled into a binary: the Chromium policy in
  `/etc/chromium/policies/managed/`, `nginx.conf`, the base `weston.ini`,
  NetworkManager's `dnsmasq-shared.d/`.
* **Chromium flags have one home each.** The wrapper carries only
  `--kiosk --no-first-run --ozone-platform=wayland`; everything else is in
  `tessaro-kiosk.service`. A fixed flag must be counterable from
  `KIOSK_CHROMIUM_ARGS_EXTRA`, and every `base::Feature` goes through
  `KIOSK_ENABLE_FEATURES`/`KIOSK_DISABLE_FEATURES` (duplicate
  `--enable-features` switches do not merge). The IME switches are the
  documented exception.
* **Generated config depends on settings and hardware only**, and is written
  only when its content changes. A timestamp in `generated.env`, the policy or
  the generated `weston.ini` restarts things on a public screen.

## The agent workspace

`agent/` is a plain Rust workspace built by the `tessaro-kiosk` recipe:
`protocol/` (the API's endpoint types, `api.rs`, the OpenAPI document and
the settings registry, `keys.rs`),
`client/` (discovery, the pinned session, the known nodes, SSH key setup and
the chunked transfers, shared by both clients), `db/` (opening a SQLite
store, shared by the agent and `client/`), `tessaro-agent/` (device side),
`tessaro-ctl/` (client), `update/`
(the staging library and `tessaro-flash`, packaged separately for the
initramfs).
The host toolchain is pinned to **rust 1.95.0** because the Chromium pin
(`meta-lts-mixins-rust`) dictates bitbake's; do not bump it on its own.
`CARGO_TARGET_DIR` is `build/cargo-target` so no `target/` is hashed into the
recipe.

* **In `tessaro-agent`, nothing blocks and every wait has a deadline.** The
  runtime is current-thread and the watchdog stops being fed if it stalls.
  `deadline::within` is the only timeout (`clippy.toml` refuses the others,
  and blocking sleep, connect and DNS). A source-grep test wants every
  `.await` in the adapters under `within(..)` or a `// naked: <reason>`
  comment, on its own line above the statement. Blocking work goes on
  `spawn_blocking`. `tessaro-ctl` is a plain blocking client on purpose.
* **TLS is openssl via native-tls on hyper**, using the device's
  `/etc/ssl/certs` plus the extra CAs of `network certs` (`http::trust`).
  Never run `update-ca-certificates` or write `/etc/ssl`: the overlay would
  shadow the image's bundle forever. reqwest and rustls are allowed only
  inside `speedtest.rs`, for cfspeedtest.
* **Do not "fix" the quiet CDP failures before the browser first answers**
  (`seen_alive` in `agent.rs`). Every boot has them.
* **Settings go through the registry** (`agent/protocol/src/keys.rs`) and are
  validated once at `config set`: no control characters, quotes, backslashes
  or `$`, because values end up in env files. Secrets live in the
  `secrets` table, never `settings` - with the one deliberate exception of
  `network.proxy.url`, whose password is stored as typed (see **Proxy** in
  [docs/networking.md](docs/networking.md)); do not add a second. The VNC
  credential is an image property and must not become a setting.
* **Every change to a store's schema is a new migration**, a
  `V<YYYYMMDDHHMMSS>__<name>.sql` in the `migrations/` of the crate that
  owns the store, and an applied migration is never edited: refinery
  checksums it and refuses to open a store whose history differs (see
  [docs/storage.md](docs/storage.md)). What the device keeps goes in
  `tessaro.db` through `db.rs`, not in a new JSON file.
* **Everything both clients do lives in `agent/client`, once**: flows of
  several requests, how a request is built from what was typed, and what an
  answer says in words. A binary only parses input, draws and decides; a
  second copy in `tessaro-ctl` or `tessaro-gui` is a bug waiting to drift
  (see [docs/clients.md](docs/clients.md)).
* **`agent/client` never prints or prompts.** Both clients link it. A
  decision is passed in (`Trust::Pin`) or handed back (`update::Sent`),
  progress goes to a `report::Report`, and text comes back as
  `text::Line`/`Fact` spans with a `Tone`, never as painted strings: the
  ctl paints tones with `style.rs`, the GUI with `theme.rs`
  (`gui/tessaro-style`, shared with Try Tessaro). Terminal output
  stays in `tessaro-ctl`.
* **`gui/` is its own workspace and never joins `agent/`**: the recipe builds
  every member of that one into the image. It reaches `protocol` and `client`
  by path, and its tasks build into `build/gui-target`.
* **A new `tessaro-ctl` command gets its place in `tessaro-gui` in the same
  change**: an action on the page of its group (`gui/tessaro-gui/src/device/pages.rs`,
  the table in [docs/gui.md](docs/gui.md)). Its logic and its words go into
  `agent/client` first and both call them (**Adding a command** in
  [docs/clients.md](docs/clients.md)).
* **A new `tessaro-ctl` command gets its Webconfig action in the same
  change too**, on the page of its group in `webconfig/src/pages/`, unless
  it is native-only (the terminal, DevTools over SSH, discovery), which
  [docs/webconfig.md](docs/webconfig.md) says so of.
* **Webconfig's words are a port of `agent/client`'s**
  (`webconfig/src/describe/`), and a golden fixture pins every ported
  function: a new or changed describe function gets a fixture in
  `agent/client/tests/describe/` and its renderer in both `tests/describe.rs`
  and `renderers.ts` (**The pages** in [docs/webconfig.md](docs/webconfig.md)).
  Floats go through `fixed()`, never `toFixed`.
* **A change to `webconfig/package-lock.json` rewrites
  `webconfig/bitbake-lock.json` in the same change** (`mise run
  webconfig:lock`): the recipe fetches only what that file lists. Keep
  Webconfig's tools on Node 22.11 (Vite 6, not 7): that is bitbake's
  `nodejs-native`.
* **A new dependency whose license `sbom/licenses.yml` does not allow gets
  an `exceptions` entry with its reason in the same change**, never a wider
  allowlist; a third-party file copied into the repo gets its
  `sbom/vendored.yml` entry ([docs/sbom.md](docs/sbom.md)).
* **What a `tessaro-ctl` read returns reaches the page bridge in the same
  change**, when a bridge call mirrors that read (the call table in
  [docs/bridge.md](docs/bridge.md), `page_action` in `control/bridge.rs`):
  a new field on `Status` goes into `device.status()`, and so on. A field
  or command the page must not have is added to what that doc says is left
  out on purpose, with the reason, instead.
* **Everything a client can ask is an endpoint type in
  `agent/protocol/src/api.rs`**, mapped onto a `Command`; clients call it by
  type and never build a `Command`. A new or changed endpoint regenerates
  `agent/protocol/openapi.json` in the same change
  (`UPDATE_OPENAPI=1 cargo test -p tessaro-agent openapi`). Nothing streams:
  a long command is a job, polled ([docs/api.md](docs/api.md)). Port 7400
  serves the API, Webconfig and Swagger UI, and nothing else.
* **A browser session is a token's, never a credential of its own**
  (`api/sessions.rs`): it must end with its token, and must never decide
  whether the device is claimed. A browser write from another origin stays
  refused (`guard` in `api/mod.rs`); do not add CORS.

**Command-line output is colored, and any new CLI must be too.** Print through
anstream's `println!`/`eprintln!` (imported to shadow the std macros) with the
palette in `tessaro-ctl/src/style.rs` (`LABEL`, `HEADING`, `OK`, `WARN`,
`BAD`, `MUTED`, `SECRET`, `CMD`, `SOURCE`), never raw colors. Styles never
change the text, `--json` is never styled, and aligned columns use
`style::pad` (`{:<N}` counts escape bytes). `tessaro-flash` is the exception:
plain text to the initramfs console.

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
    WiFi, the proxy, extra certificate authorities, `ping` from the device,
    speed test.
  * `storage`: the disk the device runs from. Partitions, free space,
    growing `/data`.
  * `screen`: the physical display. Screenshot, modes, confirming a mode,
    power, the on-screen keyboard.
  * `browser`: what the browser shows. Navigate, reload, maintenance, debug
    screen, zoom, remote DevTools, the injected script, the page bridge,
    `eval`, the extra Chromium policies.
  * `audio`: sound. Which output plays and which input records, volume,
    mute, a test tone and a recording level.
  * `camera`: the USB cameras. Which there are and what each captures, the
    format and size they capture in, how many mirrors each has, a snapshot.
  * `time`: the clock. Timezone, NTP servers and sync, its status, setting
    it by hand.
  * `script`: shell scripts the device keeps and runs as root. The scripts,
    a run now, their output, which ones the page may run.
  * `schedule`: calendar times the device runs a script at. The schedules,
    switching them on and off, their output, checking a calendar.
  * `printer`: the printers the device prints on. Discover, create, remove,
    the default, a test page, print, jobs.
  * `playlist`: the screens the player shows instead of browser.url. The
    playlists, their items, the timetable that picks one, what plays now.
  * `update`: putting an image on the device.
  * `files`: the file store in `/data/files`. Upload, download, sync, list,
    move, rm.
  * `nodes`: this client's own view (discovery, known devices). Needs no device.
* **Commands are verbs or short nouns**, and the same verb means the same thing
  in every group: `list`, `create`, `revoke`, `set`, `show`, `status`, `cancel`,
  `remove` (delete a thing the user made), `enable`/`disable`, `run` (start
  it now), `check` (validate without saving), `logs`, `test` (play or print
  something to see it works: `audio test`, `printer test`), `default` (make
  it the one used when none is named: `printer default`).
* **A group's bare name does nothing**; it prints its help. Overviews are an
  explicit `show` or `status` (`network show`, `network wifi status`).
* **Nest a third level only for a collection with its own verbs**
  (`access token create|list|revoke`, `ssh keys list|revoke`,
  `network profiles list|show`). Otherwise use two levels.
* **A new command goes into an existing group.** Add a group only when at least
  two commands would share it and none of the existing groups fits; a lone
  command goes to the nearest group.
* **A new group or page goes into a named section**:
  `agent/client/src/sections.rs` orders the ctl's `--help`, and its titles
  head the GUI's nav (`Page::TOOLS`) and Webconfig's menu (`registry.tsx`).
* **Destructive commands take `-y/--yes`** and live in the group their effect
  belongs to (`device factory-reset` wipes the device, `access unclaim` only
  removes its owners).
* **Setting keys are prefixed by the command group that acts on the same
  thing**: `browser.*`, `screen.*`, `audio.*`, `time.*`, `network.*`, `device.*`, `access.*`, `printer.*`, `camera.*`, `playlist.*`. A key
  that no command group matches is named after the component it tunes
  (`agent.*`), and `data.*` is the user's namespace. A sub-feature with its own
  on/off gets a third level that mirrors its command (`browser maintenance on
  --url` goes with `browser.maintenance.enable` and `.url`), as do the network
  profiles' keys (`network.ethernet.*`, `network.wifi.*`).
* **Renaming a key means a migration** in
  `agent/tessaro-agent/migrations/device/` that moves its row in `settings`
  (and in `pending`) and rewrites its placeholders in the URL and
  template values, so devices in the field follow at boot (see
  **Migrations** in [docs/storage.md](docs/storage.md)), and a `git grep`
  over the whole repo. The `KIOSK_*` env names do not follow the keys and
  never need to move.
* **Every command name in a user-facing string is the full path**
  (`tessaro-ctl screen confirm`): in the ctl, the agent's hints, key docs and
  pages alike.

## Writing e2e cases

The suite is in `test/e2e/spec/`; see [docs/e2e.md](docs/e2e.md) for running it.

* **It never builds the image.** Build first, in its own pane; an image older
  than the agent source proves nothing.
* **A spec file is a lane, and a lane is a fresh VM.** Cases in a file run in
  order and share state. Put a new case in the lane whose state it fits; a
  case that reboots gets a file of its own, tagged `:reboot`.
* **Steps come from the helpers** (`Guest#run`, `Journal#wait_for`/`refute`,
  `Cdp#command`). Wrap every new polling loop in `quietly` with one `step`
  naming the wait before it, and write a deliberate sleep as
  `pause SECONDS, "why"`.
* **Never leave the device claimed** past one SSH command; the harness logs in
  with the unclaimed device's empty root password.
* **qemu cannot test WiFi or mDNS.** Those are checked on the Pi by hand.
