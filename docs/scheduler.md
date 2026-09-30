# Scheduler

**A schedule is systemd `OnCalendar` expressions and the script they run
([scripts.md](scripts.md)), kept in the `schedules` table of
`/data/tessaro/tessaro.db` and fired by a systemd timer the agent renders
from it.** systemd does all the timing - the calendar, the timezone, DST,
clock jumps - and the agent only keeps the units matching the table, so a
schedule keeps firing while the agent is down or restarting.
`tessaro-ctl schedule create|set|enable|disable|remove` change the table,
`list` and `show` add what systemd reports (next run, last trigger, runs
going) and how the last run it started ended, `check` asks systemd about
expressions without saving, and `logs` reads the journal of the runs it
started. A run now is `tessaro-ctl script run`. The logic is
`agent/tessaro-agent/src/schedules.rs` (the store, the timer, parsing) and
`control/schedules.rs` (the commands and the reconcile).

* **A schedule names its script by id.** It is given by name or id and
  saved as the id, so renaming the script keeps it. What runs, how failures
  and timeouts are handled and whether runs overlap are the script's.
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
* **Per schedule there is one timer, `tessaro-schedule-<id>.timer`, in
  `/run/systemd/system`.** It has one `OnCalendar=` per expression,
  `AccuracySec=1s` (systemd's default minute would move every run by up to
  that) and no `Persistent=`, so a time missed while the device was off is
  skipped. It starts `tessaro-script-<script id>@schedule-<id>.service`, the
  script's fire unit with the schedule as its trigger, which starts the run.
* **A schedule's runs are its script's runs whose instance starts with
  `schedule-<id>-`**: `running`, `last_run` and the `units` pattern `logs`
  reads are cut from the script's by that prefix.
* **Reconciling compares, then changes only what differs.** It lists the
  `tessaro-script-*` and `tessaro-schedule-*` files in the unit directory,
  the bodies in `/run/tessaro-kiosk/scripts` and the units systemd has
  loaded, writes the files that differ (`store::replace_if_changed`),
  removes the ones nothing wants unless a run of them is active, calls
  `Reload` once if a unit changed (on a connection of its own, with a longer
  deadline than a plain call), then starts, restarts (its file changed) or
  stops each timer, and prunes the run records. It runs at the agent's
  start, which is what puts the units back after a boot, once a minute
  (`watch_units`), which brings back a timer stopped by hand and removes a
  template whose last run ended, and after every change to a script or a
  schedule. Unit file writes are escaped for systemd's `ExecStart=` quoting
  (`units::exec_arg`: `\`, `"`, `%`, `$`), which the unit tests in
  `units.rs` and `scripts.rs` pin.
* **Schedules stay through an unclaim and go with a factory reset,** with
  the scripts ([scripts.md](scripts.md)); the control-plane reset stops the
  timers at once.
* **A development host's systemd is never touched.** With
  `KIOSK_MANAGE_SCHEDULES=0` (set by `agent:integration` and the unit tests,
  with `KIOSK_SYSTEMD_UNIT_DIR` in the sandbox) the units are only rendered.
  `KIOSK_SYSTEMD_ANALYZE` points the check at a stand-in in the unit tests.
