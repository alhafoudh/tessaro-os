# Scripts

**A script is a named shell body the device keeps in the `scripts` table
of `/data/tessaro/tessaro.db` and runs as root through systemd: now, for a
schedule ([scheduler.md](scheduler.md)), for the kiosk page, on an
HDMI-CEC event ([cec.md](cec.md)), on a presence event
([presence.md](presence.md)) or on a barcode scan
([scanners.md](scanners.md)).** Every
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
  `TESSARO_TRIGGER` (`manual`, `bridge`, `schedule`, `cec`, `presence` or
  `scanner`); a CEC run also gets `TESSARO_CEC_EVENT`, and `TESSARO_CEC_KEY`
  for a remote key, a presence run `TESSARO_PRESENCE_EVENT`, a scan's run
  `TESSARO_SCANNER` and `TESSARO_SCAN_TEXT`.
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
  `bridge-...`, `schedule-<schedule id>-<unix>-<pid>`,
  `cec-<event>-<unix>-<random>` (`cec-tv-standby-...`, `cec-key:red-...`),
  `presence-<event>-<unix>-<random>`, `scanner-<scanner>-<unix>-<random>`.
  `TESSARO_TRIGGER` is that first word, and a CEC or presence run's event or
  a scan's scanner what lies between it and the time, both cut out by the
  wrapper line (`run_shell`) because systemd has no specifier for them.
  `script list` names the schedule behind a `schedule-...` run while that
  schedule exists, and the event or scanner behind a `cec-...`,
  `presence-...` or `scanner-...` run.
* **A script runs on the HDMI-CEC events in its `cec` list**
  (`script create|set --cec`): `tv-on`, `tv-standby`, `source-gained`,
  `source-lost`, `key` for every remote key, `key:<name>` for one
  (`protocol::cec`). The agent starts the run itself, as for `script run`,
  but does not follow it: how it ended is in its record. A key starts a run
  when it goes down, not again while held, and each script starts at most
  10 runs on CEC events a minute (`EVENT_BURST`), so a remote key held down
  cannot pile up root shells. `screen.cec.scripts=0` starts none, and the
  rest of the CEC events are [cec.md](cec.md)'s.
* **A script runs on the presence events in its `presence` list**
  (`script create|set --presence`): `arrived`, `left`, `near`, `far`
  (`protocol::presence`). Started the same way, with a burst window of its
  own (`EVENT_BURST` again, kept apart from the CEC one), so someone
  stepping in and out of view cannot pile up runs either.
  `camera.presence.scripts=0` starts none; what the events mean is
  [presence.md](presence.md)'s.
* **A script runs on the scans of the scanners in its `scanner` list**
  (`script create|set --scanner`): scanner names, or `*` for every scanner
  (`protocol::scanner`). Started the same way, with a burst window of its
  own. What was scanned cannot go into a unit's name: the agent writes it to
  `/run/tessaro-kiosk/scans/<run>` (root's, `0600`) before it starts the
  run, and the wrapper line reads it into `TESSARO_SCAN_TEXT`, trailing
  newlines kept, and removes it; a file no run took is gone after 10
  minutes. `scanner.scripts=0` starts none; the scans are
  [scanners.md](scanners.md)'s.
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
