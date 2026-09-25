# Settings, tessaro-ctl and the claim model

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
* **A key can need hardware** (`Key::only`). One whose hardware a device
  lacks is left out of `config keys` and `config get` there, and `config
  set` refuses it by name ("only available on a Raspberry Pi") rather than as
  unknown; `config unset` always works, so a value copied from another device
  can be taken out. What the device has comes from the build
  (`KIOSK_BOOT_CONFIG_DIR` for the Pi firmware, `Paths::offers`), never from
  probing.
* **Firmware keys apply at the next reboot, and the device never reboots for
  them.** `device.gpu_mem` is read by the Raspberry Pi firmware at power-on,
  from `tessaro.txt` on the boot partition, which `config.txt` includes last
  and the renderer writes like `generated.env`. `config set` saves it, writes
  the file and answers `reboot` (`Applied::reboot`), which the ctl prints as a
  hint to run `tessaro-ctl device reboot`: a reboot blanks a public screen, so
  when is the operator's call. A factory reset empties the file at the boot
  that performs it, which the firmware has already read, so it reaches the
  firmware one boot later.
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
* **Read-only keys report the device.** `device.id`, the read-only
  `network.*` keys and the `storage.*` keys (see **Storage** in
  [updates.md](updates.md)) are listed by `config keys`, read by
  `config get`, usable as placeholders, and refused by `config set`. The network ones come straight from the kernel
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
  the URL uses, while `config keys` and a plain `config get` only show the
  last address found this boot. "Primary" means the interface carrying the
  IPv4 default route. Changing the network is the `network.ethernet.*` and
  `network.wifi.*` settings - see **Network control** in
  [networking.md](networking.md).
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
* **Migration**: at boot, settings saved under a key's old name move to the
  new one (`keys::RENAMED`), placeholders in the templates included, and each
  move is logged to `journalctl -t tessaro-config`. A leftover
  `/etc/default/tessaro-kiosk` is imported the same way and renamed
  `.migrated`. A `config set` or `config get` of an old name is refused with
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
  answers every command without a token (a stale one is ignored), because
  whoever can reach it could claim it and do the same anyway. What makes a
  credential still needs the claim first (`require_claimed` in
  `control/access.rs`): a token, the root password, an ssh key, the hotspot
  password; `ssh connect` and the GUI's VNC use the empty root password
  instead (**SSH keys** in [remote-access.md](remote-access.md)). Both clients talk to an unclaimed device they have no pin for
  without pinning it (`Trust::KnownOnly` in `agent/client/src/connect.rs`),
  with a note saying so; once it is claimed, they refuse it again until
  `access login`.
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
* **Accepted exposure**: until it is claimed, anyone who reaches a fresh or
  reset device manages it, the first to claim owns it, and the mDNS record
  says which devices are unclaimed.

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

`tessaro-ctl` on a laptop: `mise run ctl:build`, then
`tessaro-ctl --node NAME access claim` (or `access login --token` with a token someone
issued). Pins and tokens are kept in `~/.config/tessaro/nodes.json`, 0600.

## Shell completion

`tessaro-ctl completion bash|zsh|powershell` prints a completion script. The
bash one is patched after generation: clap_complete 4.6 escapes the dash in
`tessaro-ctl` two ways, which breaks everything below the first word, so
`completion()` in `main.rs` unifies them (a test guards it). On the device it
is on by itself: the `bash-completion-pkgs` image feature installs
`tessaro-kiosk-bash-completion`, whose
`/usr/share/bash-completion/completions/tessaro-ctl` just evals
`tessaro-ctl completion bash` on the first Tab, so it can never fall behind the
binary. Root's `/bin/sh` is bash, and completion works in its POSIX mode. On a
workstation, `source <(tessaro-ctl completion bash)` in `~/.bashrc`.

## Maintenance mode

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

## Debug screen

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
  The default shows the device's identity, its addresses and routes, and the
  kiosk URL.
* **`\n` - a backslash and an n, as typed - is the line break**, and it is
  the one backslash any value may carry (`Kind::Template` in `keys.rs`). The
  generic no-backslash rule exists because values end up in env files, and
  this is how that stays true here. `render::env_file` never writes the template
  into `generated.env`, since only the agent reads it and it reads state.json.
  And the image default in `tessaro-kiosk.env.in` is **single-quoted**,
  because systemd keeps a backslash only inside single quotes (unquoted
  `a\nb` reaches the process as `anb`).
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

## The page bridge

**`browser.inject.script` and `browser.bridge.mode` put a script from the file
store and `window.tessaro` into every page**, the latter with the same keys a
template can use as `tessaro.config`. Both are agent keys, and
`tessaro-ctl browser inject` and `browser bridge` are their shorthands. How it
works, what the page may call and who may call it is
[bridge.md](bridge.md).
