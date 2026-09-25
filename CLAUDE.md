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
| [docs/kiosk-browser.md](docs/kiosk-browser.md) | Chromium units and flags, CDP supervision, origin enforcement, TLS, page zoom, self-test page, WebSerial/HID/USB/Bluetooth |
| [docs/agent.md](docs/agent.md) | deadlines, the pledge-fed watchdog, the CDP session |
| [docs/settings.md](docs/settings.md) | `state.json`, templates and placeholders, read-only keys, the protocol, the claim model, names, completion, maintenance mode, debug screen |
| [docs/display.md](docs/display.md) | Weston scaling and resolution, hotplug, on-screen keyboard |
| [docs/remote-access.md](docs/remote-access.md) | VNC mirror and its PAM auth, SSH and `ssh connect` keys |
| [docs/audio.md](docs/audio.md) | PipeWire units, how `audio.*` is applied, `auto` |
| [docs/files.md](docs/files.md) | the `/data/files` store served at `/files/` |
| [docs/networking.md](docs/networking.md) | NetworkManager, the managed profiles and their transactions, hotspot, ping, speed test |
| [docs/updates.md](docs/updates.md) | in-place updates, `--repartition`, growing `/data` |
| [docs/e2e.md](docs/e2e.md) | the qemu RSpec suite: running it, lanes, ports, harness quirks |
| [docs/gui.md](docs/gui.md) | `tessaro-gui`: inner windows, keyboard, the node list, device pages per command group, workers and jobs, the VNC viewer |

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

## Commands

Use the mise tasks rather than calling `kas-container` directly:

| Task | Purpose |
| --- | --- |
| `mise run image:build` | Build the image for `$TESSARO_MACHINE` (plus OVMF on qemu) |
| `mise run image:build:qemu` | Same, forced to `qemux86-64` |
| `mise run image:build:x86` | Same, forced to `genericx86-64` |
| `mise run image:build:rpi` | Same, forced to `raspberrypi3-64` |
| `mise run image:shell` | Interactive kas shell (cwd is the build dir) |
| `mise run image:clean` | Drop build artifacts, keep sstate and downloads |
| `mise run image:sizes` | Size of every built `.wic`, all machines at once |
| `mise run image:name` | The file name the next build will give the image, `-dirty` included |
| `mise run image:pull` | Workstation: fetch the image and bmap from the build host |
| `mise run image:update` | Workstation: pull the image and update a running device over the network (the normal path) |
| `mise run image:flash` | Workstation, manual: write the pulled image to a card or disk (first install, recovery) |
| `mise run qemu:unpack` | Decompress the `.wic` for runqemu |
| `mise run qemu:run` | Boot in QEMU, serial console on the terminal |
| `mise run qemu:vnc` | Boot in QEMU with VNC on localhost:5900 |
| `mise run agent:test` | `cargo test` for the whole agent workspace |
| `mise run agent:lint` | `cargo fmt --check` plus clippy for the workspace |
| `mise run agent:integration` | The agent against a real headless Chromium (`agent/compose.yaml`, needs docker compose), control plane in a sandbox |
| `mise run e2e:run` | Boot qemu VMs (E2E_JOBS at a time), provoke each agent behaviour, assert on its journal |
| `mise run e2e:one` | One lane or case of that suite, with plain rspec |
| `mise run e2e:setup` | `bundle install` for the suite's gems |
| `mise run ctl:build` | Release `tessaro-ctl` for this host, to manage devices remotely |
| `mise run ctl:run -- ARGS` | Run that built `tessaro-ctl` (never builds) |
| `mise run gui:run` | Run `tessaro-gui`, the desktop client, from this checkout |
| `mise run gui:build` | Release `tessaro-gui` for this host |
| `mise run gui:test` / `gui:lint` | Its unit tests; `cargo fmt --check` plus clippy |
| `mise run dev:tunnel` | Workstation: autossh VNC/SSH forwards to the build host |

Exit the QEMU serial console with `Ctrl-a x`.

**Task names are `<artifact>:<action>[:<variant>]`**, and an action means the
same thing in every group (`build`, `run`, `test`, `lint`). A new task goes
into the group of the artifact it acts on; `dev:*` holds workstation plumbing
that belongs to no artifact. Renaming a task means a `git grep` over the whole
repo, docs, comments and error messages included.

**Every task acts on one machine**, `$TESSARO_MACHINE`, defaulting to
`qemux86-64` (`image:sizes` excepted). The `image:build:*` tasks set it;
anything else takes it from the environment (`TESSARO_MACHINE=raspberrypi3-64
mise run image:shell`). Valid values are the basenames in `kas/machine/`. Each
machine gets its own TOPDIR under `build/<machine>/`; `cache/` (`DL_DIR` +
`SSTATE_DIR`) is shared. `qemu:*` and `e2e:*` are qemux86-64 only.

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
  `25_tessaro-machine`, `30_tessaro-qemu-kiosk`.
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
* **`/etc` is an overlayfs upper on `/data`.** The first write to a file there
  shadows the image's copy forever, so shipped defaults and drop-ins go under
  `/usr/lib` (kiosk env, nginx `conf.d`, NetworkManager `conf.d`). The
  exceptions are paths compiled into a binary: the Chromium policy in
  `/etc/chromium/policies/managed/`, `nginx.conf`, the base `weston.ini`.
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
`protocol/` (wire types and the settings registry, `keys.rs`),
`client/` (discovery, the pinned session, `nodes.json`, SSH key setup and
the chunked transfers, shared by both clients), `tessaro-agent/` (device side), `tessaro-ctl/` (client), `update/`
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
  `/etc/ssl/certs`. reqwest and rustls are allowed only inside
  `speedtest.rs`, for cfspeedtest.
* **Do not "fix" the quiet CDP failures before the browser first answers**
  (`seen_alive` in `agent.rs`). Every boot has them.
* **Settings go through the registry** (`agent/protocol/src/keys.rs`) and are
  validated once at `config set`: no control characters, quotes, backslashes
  or `$`, because values end up in env files. Secrets live in
  `secrets.json`, never `state.json`. The VNC credential is an image
  property and must not become a setting.
* **`agent/client` never prints or prompts.** Both clients link it; a
  decision (pinning) is passed in, and warnings come back as
  `Session::notes`. Terminal output stays in `tessaro-ctl`.
* **`gui/` is its own workspace and never joins `agent/`**: the recipe builds
  every member of that one into the image. It reaches `protocol` and `client`
  by path, and its tasks build into `build/gui-target`.
* **A new `tessaro-ctl` command gets its place in `tessaro-gui` in the same
  change**: an action on the page of its group (`gui/tessaro-gui/src/device/pages.rs`,
  the table in [docs/gui.md](docs/gui.md)). Logic both need goes into
  `agent/client`, not into either binary.

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
  so devices in the field migrate at boot (see **Migration** in
  [docs/settings.md](docs/settings.md)), and a `git grep` over the whole
  repo, placeholders in templates included. The `KIOSK_*` env names do not
  follow the keys and never need to move.
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
