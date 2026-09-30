# Scripts

**A script is a named shell body the device keeps in the `scripts` table
of `/data/tessaro/tessaro.db` and runs as root through systemd: now, for a
schedule ([scheduler.md](scheduler.md)), or for the kiosk page.** Every
trigger starts the same run unit, so the timeout, the concurrency rule, the
journal and the run history belong to the script, whoever started it.
`tessaro-ctl script create|set|remove` change the table, `list` and `show`
add the recent runs and the schedules that use it, `run` starts one and
follows it to its end, and `logs` reads the runs' journal. The logic is
`agent/tessaro-agent/src/scripts.rs` (the store, the units, the run
records), `control/scripts.rs` (the commands and following a run) and
`control/schedules.rs` (the reconcile, shared with the schedules).

* **The body runs with `/bin/sh` as root, from `/`.** `on_error: stop` is
  `/bin/sh -e`, so the first failing command ends the run; `continue` runs
  the rest and the run ends as the last command does. Nothing restricts what
  a body does: a `tessaro-ctl` command is written out in full and reaches the
  agent over the local socket. Creating a script is running code as root,
  which anyone who may manage the device can already do (`ssh`,
  `browser eval`), so it needs no rule of its own. A run gets
  `TESSARO_SCRIPT` (its name), `TESSARO_RUN` (its instance) and
  `TESSARO_TRIGGER` (`manual`, `bridge` or `schedule`).
* **The body is a file, never escaped into a unit.** It is written to
  `/run/tessaro-kiosk/scripts/<id>-<hash>.sh`, `0600`, and the run unit
  execs `/bin/sh` on it. Only the fixed wrapper line goes through `exec_arg`
  (`units.rs`), so a body needs no quoting rules beyond its size and no
  control characters but newlines and tabs; `\r\n` is saved as `\n`.
* **Per script there are a fire template and a run template, in
  `/run/systemd/system`.** In `/run`, never `/etc`, like the body, so all of
  them are rendered from the table at every start and never land on the
  `/etc` overlay.
  * `tessaro-script-<id>-<hash>@.service` is a run. `CollectMode=
    inactive-or-failed` lets systemd forget a finished instance at once;
    `TimeoutStartSec=` is the script's timeout (`infinity` without one; a
    oneshot's start timeout covers the whole run).
  * `tessaro-script-<id>@.service` is what a trigger starts, with the trigger
    as its instance (a schedule's timer starts `@schedule-<schedule id>`). It
    starts a fresh run instance with `systemctl start --no-block` and exits,
    so a timer never finds it running.
* **A run's instance names its trigger first:** `manual-<unix>-<random>`,
  `bridge-...`, `schedule-<schedule id>-<unix>-<pid>`. `TESSARO_TRIGGER` is
  that first word, cut out by the wrapper line because systemd has no
  specifier for it. `script list` names the schedule behind a
  `schedule-...` run while that schedule exists.
* **The content hash is in the run template's and the body's names.** Any
  change to the name, body, `on_error`, timeout or concurrency is a new
  template and body, so an edit never changes a run already going; the old
  ones are removed once no instance of them is active. The description and
  the bridge flag change no run and are not hashed.
* **`concurrency: skip` is checked where a run starts.** The fire unit asks
  `systemctl list-units` for an active run of the script and, finding one,
  says `skipped: a run of <name> is going` in the journal and starts
  nothing. `script run` and the page check the same through the bus and
  refuse. The two checks are not atomic with the start: two triggers in the
  same instant can still both run. `overlap` starts every run; nothing but
  the timeout limits how many pile up.
* **How each run ended is written by the run itself.** `ExecStopPost=`
  writes `<finished> $SERVICE_RESULT $EXIT_STATUS` to
  `/data/tessaro/script-runs/<id>/<instance>`, through a temporary renamed
  over it, so it is recorded while the agent is down too, and survives a
  reboot. The reconcile keeps the newest `SCRIPT_RUNS_KEPT` per script and
  removes the directory of a script that is gone.
* **`script run` is a job ([api.md](api.md), jobs): the agent starts the run
  instance itself, so it knows which one to follow.** It reads that unit's
  journal every 500 ms with a cursor and sends each line as a
  `ScriptEvent::Line`, then `Ended` with the run's record. stdout and stderr
  are both in the journal as one stream, in order, and are not told apart.
  systemd's own lines about the unit are left out (`_SYSTEMD_UNIT` is the
  run's). Past `SCRIPT_OUTPUT_MAX` lines the job sends `Cut` and only the
  journal has the rest. The journal is read once more a poll after the
  record appears, since journald can still be writing the last lines. A run
  gone from systemd with no record for several polls ends the job with an
  error. Following lasts the timeout plus a minute (a day without one);
  cancelling the job stops following, not the run.
* **The page may list and run only a script with `bridge` on.** See
  `tessaro.scripts` in [bridge.md](bridge.md).
* **A script a schedule runs cannot be removed**; the refusal names the
  schedules. Scripts stay through an unclaim and go with a factory reset,
  like the settings: both reset paths clear the `scripts` and `schedules`
  rows and remove `script-runs/`.
* **The reconcile is the schedules' one** (**Reconciling** in
  [scheduler.md](scheduler.md)): it writes the unit files and the bodies
  that differ, removes what no script wants unless a run of it is active,
  and reloads systemd once if a unit changed.
* **A development host's systemd is never touched.** With
  `KIOSK_MANAGE_SCHEDULES=0` the units and bodies are only rendered, and
  `script run` says the host's scripts are not run.
