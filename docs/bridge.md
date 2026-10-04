# The page bridge

**The agent can put two things into every page the kiosk shows, before the
page's own scripts run: a script of your own from the file store, and
`window.tessaro`, which hands the page the device's settings and, if asked
for, device actions.** Both go over the agent's existing DevTools session
(`agent/tessaro-agent/src/cdp/session.rs`), so the browser gets no new path to
the control plane: `/run/tessaro-agent.sock` stays root-only, and the agent
decides every call. The code is `control/bridge.rs` and its preamble
`control/bridge.js`; `tessaro-ctl browser eval`, which runs code in the page on
an operator's behalf, is in `control/page.rs`.

| Setting | Command | What it does |
| --- | --- | --- |
| `browser.inject.script` | `tessaro-ctl browser inject on --script FILE` / `off` | Runs FILE from `/data/files` in every page |
| `browser.bridge.mode` | `tessaro-ctl browser bridge off\|config\|actions` | What the page gets as `window.tessaro` |

Both are agent keys: a change restarts nothing. The bridge's next refresh
takes the new config and reloads the page, which comes up with the new
scripts. The browser keeps running.

## The injected script

**`browser.inject.script` names a file in the store from its root**: `inject.js`
and `/inject.js` are the same file, `/data/files/inject.js`, stored without the
slash. It is validated with the store's own path rules (`protocol::files`), so
it cannot leave `/data/files`, and read with `Files::read_whole`, which follows
no symlink.

* **It runs in every document, before any of the page's scripts**, through
  `Page.addScriptToEvaluateOnNewDocument`. Iframes run it too; a script that
  must act only once checks `window === window.top` itself.
* **The page's Content Security Policy does not apply to it.** A script added
  over DevTools is not subject to `script-src`.
* **Uploading a new copy reloads the page with it.** `Files` bumps a counter
  whenever a file lands, moves or goes (`Files::changes`); the bridge reads
  the script again and, when its text changed, swaps the registration and
  bumps `PageScripts::reload`, which the session turns into
  `Page.reload{ignoreCache}`.
* **A missing, oversized or non-UTF-8 file is skipped, not fatal.** The limit
  is `protocol::EVAL_MAX` (1 MiB). `device status` shows an `inject` row with
  the file and why it is not in the page, and the journal says so once.

## `window.tessaro`

**`browser.bridge.mode` is `off`, `config` or `actions`, and each includes the
one before.** `off` puts nothing on the page at all.

```js
tessaro.mode                 // "config" or "actions"
tessaro.config["network.ip"] // any placeholder key, as {network.ip} in browser.url
window.addEventListener("tessaro:config", (e) => console.log(e.detail.changed));
await tessaro.device.status();
```

* **`tessaro.config` is every key a template may use**, with the value
  `{key}` would expand to (`state::resolve`): settings, `data.*`, and the
  read-only `network.*`, `storage.*` and `device.id`. It is a frozen object of
  plain strings, so `JSON.stringify(tessaro.config)` always works.
* **Left out on purpose** (`HIDDEN` in `bridge.rs`): `access.*` (who may manage
  the device and where it listens), and `network.public_ip`, which costs a
  request to Cloudflare and is
  `tessaro.network.publicIp()` instead. `browser.block` and `browser.allow`
  too, for the same reason as the browser policies below: a page must not
  read its own restrictions. `network.proxy.url` holds the proxy's password
  as typed (see **Proxy** in [networking.md](networking.md)). `agent.*`, the
  Chromium switches (`browser.args_extra`, `browser.enable_features`,
  `browser.disable_features`), `browser.device_origins`,
  `browser.enforce_origin` and `screen.vnc` say how the page is locked down
  and how the device can be reached, and every script on the kiosk origin
  reads `tessaro.config`.
* **It follows the device without a reload.** The snapshot is built again
  after every settings change (`converge` pokes the bridge) and every 15s for
  what moves on its own, an address from DHCP or free space. When it changed,
  the agent replaces `tessaro.config` with a new frozen object and fires a
  `tessaro:config` event on `window` whose `detail.changed` lists the keys. The
  preamble registered for the next document carries the new snapshot too.

**Calls return Promises.** A refusal rejects with an `Error` whose message is
the control plane's own. `config` mode answers the reads; `actions` mode
answers everything:

| Call | Mode | Does what `tessaro-ctl` does with |
| --- | --- | --- |
| `log(level, message)` | config | the journal, as `page (level): message`; `debug` only with `agent.debug` |
| `device.status()` | config | `device status`, without the node's fingerprint and claim, whether remote DevTools is open (it tells a page it can be driven) and the settings revision (`tessaro:config` says when they change): its `name`, the hardware and its serial, memory, `cpuPercent`, the clock (`time`), a mode on probation (`pending`) and why the injected script is not in the page (`bridge.scriptProblem`) included |
| `network.status()` | config | `network show`, without the public address |
| `audio.status()` | config | `audio show` |
| `printer.list()` | config | `printer list`, without each printer's URI |
| `printer.jobs(printer)` | config | `printer jobs`: every printer's, or the one named |
| `scripts.list()` | config | `script list`, only the scripts with `--bridge`, without their bodies: name, description, concurrency, runs going, the last run |
| `network.publicIp()` | actions | `config get network.public_ip`: asked now |
| `network.online()` | actions | the same lookup, resolved as `true` or `false` |
| `browser.reload()` | actions | `browser reload` |
| `browser.restart()` | actions | `device restart browser` |
| `browser.home()` | actions | `browser navigate` to the page the agent drives |
| `browser.clearCache()` | actions | `browser clear-cache` |
| `browser.maintenance(on, url)` | actions | `browser maintenance on --url` / `off` |
| `device.reboot()` | actions | `device reboot` |
| `audio.volume(percent)`, `audio.mute(on)` | actions | `audio volume`, `audio mute` |
| `audio.inputVolume(percent)` | actions | `audio input-volume` |
| `keyboard.show(selector)`, `keyboard.hide()` | actions | `screen keyboard show --selector` / `hide` |
| `screen.off()`, `screen.on()` | actions | `screen power off` / `on` |
| `network.ping(host)` | actions | `network ping`: every event, in order |
| `network.speedTest()` | actions | `network speedtest`: every event, in order |
| `files.list(path)` | actions | `files list` |
| `printer.print({ data, path, printer, copies, media, title })` | actions | `printer print`: `data` a string, `Blob`, `ArrayBuffer` or bytes; `path` a file in the store; no `printer` is the default one |
| `printer.cancel(job)` | actions | `printer cancel`, of a job `printer.jobs()` names |
| `data.set(name, value)`, `data.unset(name)` | actions | `config set data.NAME=...` / `unset` |
| `scripts.run(name)` | actions | `script run`, of a script with `--bridge` only: resolves when the run ends with `{run, trigger, started, finished, result, status, succeeded, output, truncated}` (times in seconds since the epoch), whether it succeeded or not |

Nothing under `access`, `ssh`, `update` or `device factory-reset`, and no
`config set` of anything but `data.*`, is reachable from a page. That
includes Webconfig's browser sessions and tickets (`access/session`,
`access/ticket`): they are credentials, and the kiosk page is not a
manager of the device. Nor are the browser policies (`browser policies`),
read or written: they are what the page may do, and a page must not read
or loosen its own restrictions. The same goes for the printers: a page
prints on the ones the operator set up, and `printer create`, `remove` and
`default` are not reachable from it. Nor is `camera list`: the page has the
cameras' mirrors themselves in `navigator.mediaDevices.enumerateDevices()`,
with the names `camera list` shows, and `camera.*` is the operator's, like
any setting outside `data.*`. Nor is `camera snapshot`: a page that wants a
camera's picture opens a mirror with `getUserMedia` and draws a frame
itself, and a snapshot is how the operator sees a camera without taking a
mirror from the page. Nor is `screen.input.*`, which the page reads in
`tessaro.config` but cannot set: ignoring the touchscreen or the keyboard
is the operator's lockout of the people in front of the screen, which the
page must not lift, and each change restarts Weston and the browser
(**Input devices** in docs/display.md).

The other commands no call mirrors are the operator's too:

* **`time`**: the page has `Date` and `device.status().time`; the clock and
  the timezone are set for the device, not for one page.
* **`schedule`, `script logs` and the rest of `script`**: when root scripts
  run and what they printed before is the operator's; the page gets the
  output of the runs it starts.
* **`storage`**: the page reads `storage.*` in `tessaro.config`, and
  `storage grow` repartitions the disk.
* **`screen modes`, `confirm` and `screenshot`**: a wrong mode leaves the
  screen black, and a screenshot would show the page what else is on it.
* **`browser navigate`, `zoom`, `debug`, `devtools` and `eval`**: the page
  moves itself with `location`; the rest are tools to look at the page from
  outside.
* **`network wifi`, `profiles`, `interfaces`, `proxy` and `certs`**: the
  link is the operator's, and a page that changed it could cut itself off.
* **`files upload`, `download`, `sync`, `move` and `rm`**: the page reads
  the store over `/files/` and does not change the operator's files.
* **`audio output`, `input` and `test`**: which device plays and records is
  the operator's; the page sets the volume it plays and records at.
* **`device logs`, `ping` and `restart`** of anything but the browser: the
  journal carries every caller's actions, and the agent's restart is the
  operator's.

* **`data.set` leaves the page alone when no template uses the key.** A `data.*` is
  read by the templates and by this bridge only, so a value no template names
  is saved with `apply` off, and the page keeps its own state across reboots
  without reloading itself. One that `browser.url`, the maintenance URL or the
  debug template uses moves the page like `config set`: the agent loads the
  new URL, or shows the debug screen with the new value.
* **Starting the page over is refused within 60s** (`DISRUPT_GAP`) of the
  agent's start and of the last time: `browser.reload`, `restart`, `home`,
  `maintenance`, `device.reboot`, and a `data.set` a template uses. A
  page that calls one on load would otherwise loop.
* **`network.speedTest()` runs at most once in 10 minutes**: it moves real
  data over a link that may be metered.
* **`printer.print()` needs `printer.enable`, and prints at most
  `PRINT_BURST` documents per `PRINT_WINDOW`** (`control/bridge.rs`): the
  switch that lets `window.print()` print is the one that lets the page print
  at all, and a page that prints in a loop empties the paper tray, not the
  device. The preamble sends `data` as base64; the whole document goes in the
  one call, at most `PRINT_DATA_MAX` (see **Sizes** in
  [printing.md](printing.md)).
* **`window.print()` is the preamble's while `printer.enable` is on**, in
  every mode, `off` included: it calls `page.print`, the one call answered
  with the bridge off, and nothing else of `window.tessaro` is there then.
  It counts against the same `PRINT_BURST` (see **Pages** in
  [printing.md](printing.md)).
* **`printer.list()` leaves out where each printer is**: a URI can carry a
  print server's user and password, and the page needs only the names.
* **`printer.cancel()` needs `printer.enable` too**, and cancels any job
  waiting, the operator's included: a page that may print may also take
  back what is stuck in front of its own document. `printer.jobs()` is a
  read and answers with printing off, like `printer.list()`.
* **The page runs only the scripts the operator marked for it, and at most
  `SCRIPT_BURST` runs per `SCRIPT_WINDOW`** (`control/bridge.rs`). Each run
  is a root shell, so which ones is the operator's `script create|set
  --bridge`, never the page's, and `scripts.list()` gives no body: a page
  learns what a script is for, not what it does. The run's trigger is
  `bridge` and it honours the script's concurrency like any other
  ([scripts.md](scripts.md)); a run that skipped rejects. `scripts.run` waits
  for the end at most `STREAM_LIMIT`, then rejects with `run` naming the
  instance, which goes on. A failed run resolves, with `succeeded: false`:
  the page gets the output either way. The output is the run's stdout and
  stderr in one list, at most `SCRIPT_OUTPUT_MAX` lines, `truncated` past
  that.
* **`network.publicIp()` is shared and cached for 30s.** Calls at the same time
  wait for one request. A failure rejects with `lastKnown`, the last address
  found this boot, if any. A success also updates what
  `config get network.public_ip` reports, but not `tessaro.config`.
* **`network.online()` is `publicIp()` as a yes or no, and never rejects for
  being offline.** It shares that call's request and cache: an answer from
  Cloudflare's trace at 1.1.1.1 resolves `true`, anything else `false`. The
  result also feeds the welcome page's online indicator (see **Online** in
  [quick-setup.md](quick-setup.md)).

## Who may call

**Only the kiosk's own origin, in the top frame's page world, is answered.**
The allowed origins are those of the expanded `browser.url` and of the page
the agent drives (the maintenance page, in maintenance mode), with a default
port left out as the browser writes it.

* **Where a call came from is the browser's word, not the page's.** With the
  binding the session sends `Runtime.enable` and keeps each execution
  context's origin, frame and whether it is the default world
  (`Runtime.executionContextCreated`). A `Runtime.bindingCalled` is passed on
  as a `BindingCall` with that origin and `top` set only for the main frame's
  default world. An iframe, an isolated world or an unknown context is ignored
  without an answer. The console noise `Runtime.enable` brings is dropped in
  the session, never broadcast.
* **The raw binding never reaches page code.** Chromium installs it as
  `window.__tessaroBridge` in every frame; the preamble runs first, takes it
  into a closure and deletes it, and outside the allowed origin or the top
  frame exposes nothing at all.
* **The agent answers through a handle the page cannot guess**: a property
  named `__tessaro_` plus random hex, new with every agent start, reached with
  `Runtime.evaluate` in the calling context.
* **Every script on the kiosk origin can call what the mode allows**, a
  third-party analytics script included. That is the trust model to accept
  before choosing `actions`.
* **The journal names the page as the caller** (`Caller::Page`, "the page"), so
  a reboot or a setting it changed is as traceable as an operator's.

## Reconnects, crashes and restarts

**Chromium forgets the scripts and the binding with the DevTools connection
that registered them**, so the session registers `PageScripts` again on every
connection (`prime`). What happens to the page on screen depends on why:

* **The mode, the injected script or `printer.enable` changed**: the bridge
  keeps them as an `Offer` computed from the current config on every
  refresh, and a change bumps the session's reload counter, so the page
  reloads with the new `window.tessaro`. New answered origins alone reload
  nothing.
* **The agent restarted**: its first cycle navigates, so the page loads with
  the scripts.
* **The session reconnected to the same page**: the page keeps running and is
  not reloaded. The session evaluates `PageScripts::rebind`, which gives the
  preamble the new binding in place of the dead one.
* **The page crashed and came back**: its scripts died with the renderer. The
  session registers them again on `Inspector.targetReloadedAfterCrash` and
  reloads the page so they run.
* **A script replaced before Chromium answered for it** is removed when its
  identifier arrives (`Link::stale_requests`), so a quick succession of
  changes cannot leave an old copy registered.

## `tessaro-ctl browser eval`

**`browser eval CODE`, `--file FILE` or `-` for stdin runs JavaScript in the page
now** (`Command::Eval`) and prints the value, or the exception with its line
and column; the exit status is non-zero when it threw. It is an operator
command with the normal token auth, available whatever the bridge mode, and
it sees `window.tessaro` like any page script.

* **A returned Promise is waited for**; `--no-await` does not. `--gesture` runs
  it as if the screen had just been touched, which audio, fullscreen and the
  keyboard need.
* **It cannot hold the page.** `--timeout` (default 10s, at most 60s) goes to
  Chromium as `Runtime.evaluate`'s own `timeout`, and when the agent's deadline
  passes it sends `Runtime.terminateExecution`, so neither a `while (true)` nor
  a Promise that never settles outlives it.
* **The journal gets the code's size and the start of its SHA-256, never the
  code**, which may carry a secret.
* **A value JSON cannot hold comes back as Chromium serializes it**: a
  function or a DOM node as `{}` with its type, which is what `browser eval`
  prints; `NaN`, `Infinity` and a `BigInt` as their description.
