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
