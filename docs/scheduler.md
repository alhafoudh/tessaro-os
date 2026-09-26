# Scheduler

**A schedule is systemd `OnCalendar` expressions and shell command lines,
kept in `/data/tessaro/schedules.json` and run by systemd timers the agent
renders from it.** systemd does all the timing - the calendar, the
timezone, DST, clock jumps - and the agent only keeps the units matching
the file, so a schedule keeps firing while the agent is down or restarting.
`tessaro-ctl schedule create|set|enable|disable|remove` change the file,
`list` and `show` add what systemd reports (next run, last trigger, runs
going) and how the last run ended, `run` starts one now, `check` asks
systemd about expressions without saving, and `logs` reads the runs'
journal. The logic is `agent/tessaro-agent/src/schedules.rs` (the file,
the units, parsing) and `control/schedules.rs` (the commands and the
reconcile).

* **Each line runs as `/bin/sh -c '<line>'`, as root, in order.** Nothing
  restricts what a line does: a `tessaro-ctl` command is written out in full
  and reaches the agent over the local socket like any other. Creating a
  schedule is running code as root, which anyone who may manage the device
  can already do (`ssh`, `browser eval`), so it needs no rule of its own.
  `on_error: stop` ends a run at the first failing line; `continue` runs the
  rest anyway (`ExecStart=-...`). Only the run's overall result is kept; which
  line failed and what each printed is in the journal
  (`tessaro-ctl schedule logs NAME`, the unit pattern
  `tessaro-schedule-<id>-*@*.service`).
* **Expressions go to systemd unchanged, and are checked by systemd before
  they are saved.** `create`, `set` with `--on` and `check` run
  `systemd-analyze calendar --iterations=N` on each one (the image ships the
  `systemd-analyze` package for this, pulled in by `tessaro-kiosk`), and a
  refusal is systemd's own message. Its times are read from the `(in UTC):`
  lines, or from the times themselves when the device's zone is UTC and
  systemd prints none, so no zone abbreviation is ever parsed; they go on the
  wire as `Moment`s with the device's wall clock beside them
  (`time::local_clock`). An expression may name its own zone at the end;
  without one it is the device's `time.timezone`. `%` is refused: nothing in
  the syntax uses it, and in a unit file it starts a specifier.
* **Per schedule there are a timer, a fire service and a run template, all
  in `/run/systemd/system`.** In `/run`, never `/etc`, so they are rendered
  from the file at every start and never land on the `/etc` overlay. The
  timer has one `OnCalendar=` per expression, `AccuracySec=1s` (systemd's
  default minute would move every run by up to that) and no `Persistent=`,
  so a time missed while the device was off is skipped. It starts
  `tessaro-schedule-<id>.service`, which only starts a fresh instance of
  `tessaro-schedule-<id>-<hash>@.service` with `systemctl start --no-block`
  and exits.
* **Runs overlap instead of being skipped.** A timer does not start its
  service while it is still active, so the lines cannot live in the unit the
  timer starts. The fire service is done in milliseconds, and each run is an
  instance of its own, named `<start unix seconds>-<pid>`. Nothing limits how
  many pile up but `--timeout`, which is the run template's
  `TimeoutStartSec=` (`infinity` without it; a oneshot's start timeout covers
  the whole run).
* **The content hash is in the run template's name.** Any change to the name,
  calendar, lines, `on_error` or timeout is a new template, so an edit never
  changes the lines under a run already going. The old template is removed
  once no instance of it is active; enabling or disabling changes only
  whether the timer runs.
* **How the last run ended is written by the run itself.** The template's
  `ExecStopPost=` writes `<instance> <finished> $SERVICE_RESULT $EXIT_STATUS`
  to `/data/tessaro/schedule-runs/<id>`, through a temporary renamed over it,
  so it is recorded while the agent is down too, and survives a reboot.
  `CollectMode=inactive-or-failed` lets systemd forget a finished instance at
  once; the file is the record. With `continue`, a failing line does not
  fail the run, so its result is `success`.
* **Reconciling compares, then changes only what differs.** It lists the
  `tessaro-schedule-*` files in the unit directory and the units systemd has
  loaded, writes the files that differ (`store::replace_if_changed`), removes
  the ones no schedule wants unless a run of them is active, calls `Reload`
  once if anything changed (on a connection of its own, with a longer
  deadline than a plain call), then starts, restarts (its file changed) or
  stops each timer. It runs at the agent's start, which is what puts the
  units back after a boot, once a minute (`watch_schedules`), which brings
  back a timer stopped by hand and removes a template whose last run ended,
  and after every change. Unit file writes are escaped for systemd's
  `ExecStart=` quoting (`exec_arg`: `\`, `"`, `%`, `$`), which the unit tests
  in `schedules.rs` pin.
* **Schedules stay through an unclaim and go with a factory reset,** like the
  settings: both reset paths remove `schedules.json` and `schedule-runs/`,
  and the control-plane reset stops the timers at once.
* **A development host's systemd is never touched.** With
  `KIOSK_MANAGE_SCHEDULES=0` (set by `agent:integration` and the unit tests,
  with `KIOSK_SYSTEMD_UNIT_DIR` in the sandbox) the units are only rendered,
  and `run` says the host is not managed. `KIOSK_SYSTEMD_ANALYZE` points the
  check at a stand-in in the unit tests.
