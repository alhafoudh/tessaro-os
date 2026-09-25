# Time

**The clock is systemd-timedated and systemd-timesyncd, and the time.*
settings are the only truth about it.** `tessaro-ctl time timezone ZONE`
sets `time.timezone`, and `time ntp on [--server HOST]...` / `time ntp off`
set `time.ntp.enable` and `time.ntp.servers`. The agent applies them to the
running system at once: only systemd-timesyncd restarts, and only when its
servers change. `time show` prints what the services report, `time zones`
lists what `time.timezone` takes, `time sync` asks the servers again now,
and `time set` sets the clock by hand while NTP is off. The logic is
`agent/tessaro-agent/src/time.rs`; the image side is
`meta-tessaro-distro/recipes-core/tessaro-time/`.

* **Applying compares against what the services report, never against
  files.** `Time::apply` reads timedated's `Timezone` and `NTP` and
  timesyncd's `SystemNTPServers` and `RuntimeNTPServers`, and calls only
  what differs. So it is idempotent and cheap, and it runs from the watcher
  at the agent's start and once a minute (`watch_time` in
  `control/watchers.rs`), as well as from `config set` (`converge`, which
  reports the result in `Applied.time`). The watcher is what puts
  everything back after a boot, and after anyone restarts timesyncd.
* **time.ntp.servers is a drop-in in `/run`,
  `/run/systemd/timesyncd.conf.d/50-tessaro.conf`**, rendered from the
  settings on every start and removed when the key is empty. Never
  `/etc/systemd/timesyncd.conf`: that is on the `/etc` overlay, where a
  write would shadow the image's file for good. timesyncd reads
  configuration only at start, so a changed drop-in is followed by
  `TryRestartUnit` - restarting it only if NTP is on and it is running.
* **DHCP's NTP servers reach timesyncd through the agent.** timesyncd takes
  per-link servers only from systemd-networkd, which the image does not
  build, so on its own it would ignore the servers the network offers.
  While `time.ntp.servers` is empty the agent reads the `ntp_servers`
  option of every NetworkManager device's DHCPv4 lease (`DHCP4Config`) and
  hands them to timesyncd with `SetRuntimeNTPServers`. Runtime servers
  outrank every other kind (systemd's `timesyncd-manager.c` tries runtime,
  then system, then link, then fallback servers), so
  when `time.ntp.servers` is set the agent clears them, and the configured
  servers are what timesyncd uses. With neither, the image's fallback
  servers (systemd's compiled-in `time1-4.google.com`) apply.
* **timesync1 is only asked while timesyncd runs.** Its D-Bus name is
  activatable, so any call would start timesyncd again and undo
  `time.ntp.enable=0`. `apply` and `status` check the unit's `ActiveState`
  first.
* **The timezone is timedated's `SetTimezone`**, which relinks
  `/etc/localtime`. The link lands on the `/etc` overlay; that is
  acceptable because `state.json` stays the source of truth and the agent
  reapplies it at every start, and the image's own link (to `UTC`, from
  `tzdata-core` with `DEFAULT_TIMEZONE:pn-tzdata`) only ever sits below it.
  A zone is checked against `ListTimezones` at `config set`, next to the
  audio device check in `control/settings.rs`; the registry's
  `keys::is_timezone` only keeps the name from being a path.
* **Nothing restarts for a timezone.** glibc re-reads `/etc/localtime` on
  `tzset` when the file changed, and Chromium watches the file
  (`time_zone_monitor_linux.cc`), so a new zone shows in pages and the
  journal without a browser restart; the e2e case in `control_spec.rb`
  checks the page's `Intl` zone. The zone is never passed as `TZ`: the
  time.* keys are not in `generated.env` at all.
* **No `/etc/timezone`.** timedated never updates it, so it would go on
  naming the image's zone; `INSTALL_TIMEZONE_FILE:pn-tzdata = "0"` in
  `tessaro.conf` leaves it out.
* **The saved clock survives a reboot.** timesyncd writes the time of every
  sync to `/var/lib/systemd/timesync/clock` and at start moves a clock that
  is behind it forward. `/var/lib` is tmpfs (volatile-binds), and a
  Raspberry Pi has no hardware clock, so without it every boot would start
  at the build date until the first answer, and TLS would fail meanwhile.
  `tessaro-time-state.service` binds `/data/overlay-timesync` there, shaped
  like `tessaro-network-state.service` for the reason given in
  [networking.md](networking.md), and ordered before
  `systemd-timesyncd.service`, which is `DefaultDependencies=no` and would
  otherwise start first.
* **Every number `time show` prints is systemd's.** The agent measures
  nothing. From timedated: the time, the hardware clock, the zone, whether
  NTP is on and whether the kernel clock is synchronized. From timesyncd:
  the server, the poll interval and its bounds, `Frequency` - the kernel's
  frequency correction in 2^-16 ppm, shown as drift - and `NTPMessage`, the
  last answer. Offset and delay come from that answer's origin, receive,
  transmit and destination timestamps
  exactly as `timedatectl timesync-status` computes them
  (`NtpSample::offset_and_delay` in `protocol`), and an answer that fails
  timedatectl's own sanity check is treated as none. Formatting is
  `agent/client/src/clock.rs`, shared by `tessaro-ctl` and the GUI.
* **`time set` is refused while NTP is on**, as timedated refuses it; it is
  for a network without any time server. With no time given, `tessaro-ctl`
  sends the workstation's clock; a `YYYY-MM-DD HH:MM[:SS]` is read in the
  device's timezone by `mktime` on the device.
* **A development host's clock is never touched.** The agent's system bus
  is the host's own there, so `KIOSK_MANAGE_CLOCK=0` (set by
  `agent:integration` and the unit tests' fixture) makes `apply`, `sync`
  and `set` do nothing; `status` still reads.
