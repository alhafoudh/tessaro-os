# The agent's deadlines and watchdog

**Every wait on the outside world has a deadline, and systemd watches the
agent itself**, so a wedged agent cannot keep its unit `active` while the
kiosk goes unsupervised.

* **`deadline::within` is the only timeout in the program.**
  `agent/tessaro-agent/clippy.toml` refuses `tokio::time::timeout` and
  `timeout_at` everywhere else, so every deadline names its call in the
  journal (`the system bus did not answer within 5s`). A source-grep test in
  `deadline.rs` requires every `.await` in the adapters to be under a
  `within(..)`, a method of the same adapter, or a `// naked: <reason>`
  comment - the only check that reaches the zbus calls, which
  `#[zbus::proxy]` generates and clippy cannot name. Put a `// naked:` comment
  on its own line above the statement: rustfmt moves one that trails a `{`
  into the block, where it no longer annotates anything.
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
  the probe timeouts, which are settings. So `WatchdogSec=60` is independent
  of them, a wedge is caught in about 75s, and waiting between cycles -
  including the `KIOSK_AGENT_ENABLE=0` park - pledges too. A pledge is capped
  at 120s; a configured timeout above that is called out at startup.
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
* **A stopping agent does not wait for its blocking threads.** A deadline
  frees the agent from a `spawn_blocking` call, but the thread runs on, and
  dropping the runtime would wait for it without limit. `main` ends with
  `shutdown_timeout(SHUTDOWN)` (2s) instead, and the unit's
  `TimeoutStopSec=10` caps whatever else could hold a stop. The journal has
  `signal received, stopping` when the signal lands and `stopping` when the
  loop has left.
* **The CDP session reconnects on its own and logs only at debug** (except a
  failed `DeviceAccess.enable` when `agent.device_access` is on). Every
  command is under `KIOSK_CDP_TIMEOUT`, and a websocket ping every
  `KIOSK_CDP_PING` seconds tears down a half-open socket after two go
  unanswered - counted, not timed, so the agent's own stalls are not blamed on
  the browser. **A crashed renderer does not end the session**: a sad tab
  answers every command with `Target crashed` - `Page.enable` included, so
  priming a new session against it fails every time - except `Page.navigate`,
  which brings it back. So the session stays up, bumps the generation, the
  agent re-navigates, and the tab reloads in seconds instead of the browser
  being restarted. None of it works without `Inspector.enable`, which delivers
  `Inspector.targetCrashed`. A detached target does end the session. The agent
  waits for the session's first attempt before its first cycle, so an agent
  restart does not report a healthy browser as silent.

## Settings on a running agent

**A setting the agent reads applies to the running process, with nothing
restarted** (`Consumer::Agent`), so a change never drops the API, a
Webconfig session or the loop's bookkeeping. `converge` in
`control/settings.rs` builds a new `Config` from the defaults
(`state::defaults`, which also captures the image-only variables in
`config::IMAGE_ONLY`) and the saved settings, and `publish` hands it, with
the settings, to a tokio watch channel as a `config::Current` - only when
it differs.

* **The state machine takes it at the top of each cycle** (`follow_config`
  in `agent.rs`), and its nap between cycles wakes on a change. A different
  kiosk URL (maintenance mode moves it too) or offline page makes the screen
  unknown, so the cycle navigates with `Page.navigate`, and the accepted
  origin is reset. Everything else applies from that cycle on, with the
  failures and the backoff kept. `agent.enable` parks and resumes the loop,
  and the `watching ...` line is logged again.
* **The other readers follow the same channel.** The debug screen
  (`debug.rs`) takes its template and the settings from it, `agent.debug`
  switches the log's verbosity (`Log::set_debug`), the watchdog reads
  `agent.watchdog` on every tick, and the page bridge recomputes its offer on
  every refresh (**Reconnects, crashes and restarts** in
  [bridge.md](bridge.md)).
* **What the agent sets up once per process restarts it**
  (`Consumer::AgentRestart`): the listener and mDNS (`access.listen`,
  `access.mdns`, `device.name`), and the HTTP and DevTools clients with their
  budgets (the `agent.probe_*` timeouts, the `agent.cdp_*` keys,
  `agent.device_access`). Its first cycle loads the page again. The proxy
  switched on or off and the extra certificate authorities restart it for
  the same reason (**Proxy** in [networking.md](networking.md)). The browser
  keeps running: the change wakes the loop just before the stop, and a
  stopping agent neither counts a failed CDP check (the session answers
  "shutting down") nor restarts the browser (`restart` in `agent.rs`).
