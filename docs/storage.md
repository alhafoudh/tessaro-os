# Storage: the SQLite stores

**What the device keeps about itself, and what a client keeps about the
devices it manages, are SQLite databases.** A kiosk loses power as a matter
of routine, so every write has to be all or nothing and survive the cut, and
several processes (the boot oneshot, the agent, a factory reset, the
`sqlite3` shell, tessaro-ctl next to tessaro-gui) must never interleave. A
SQLite transaction gives both, and a table per kind of record lets a change
touch one row instead of rewriting a whole document.

| Store | Opened by | Tables | Why there |
| --- | --- | --- | --- |
| `/data/tessaro/tessaro.db` | `tessaro-agent` and `tessaro-agent boot` (`db.rs`) | `settings`, `state`, `tokens`, `secrets`, `schedules`, `net_txn`, `net_last` | on `/data`, so it survives reboots and updates |
| `/run/tessaro-kiosk/sessions.db` | `tessaro-agent` (`api/sessions.rs`) | `sessions` | on tmpfs, so a reboot ends every browser session |
| `<config dir>/tessaro.db` | `tessaro-ctl`, `tessaro-gui` (`agent/client/src/store.rs`) | `nodes`, `gui_prefs` | the config dir is `TESSARO_CONFIG_DIR`, else `$XDG_CONFIG_HOME/tessaro`, else `~/.config/tessaro` |

The migrations next to each store's code are the schema:
`agent/tessaro-agent/migrations/device/`, `agent/tessaro-agent/migrations/sessions/`
and `agent/client/migrations/`. Each table is commented there.

## Opening a store

**Every store is opened through `tessaro-db` (`agent/db/src/lib.rs`)**, so the
device and the clients share one set of rules:

* **`journal_mode=WAL` with `synchronous=FULL`.** WAL lets a reader run beside
  a writer; `FULL` syncs the log at every commit, so a commit that returned is
  there after a power cut. `NORMAL` would lose the last commits, which on a
  kiosk means a claim or a setting the operator was told had been saved.
* **`busy_timeout` is 5 s.** A writer in another process delays a call instead
  of failing it, and 5 s is well inside the agent's 20 s `deadline::blocking`,
  so a stuck writer shows up as a failed call, not a stalled runtime.
* **The file is created 0600** before SQLite opens it; SQLite gives its `-wal`
  and `-shm` the database's mode. The device's store holds token hashes and
  the network passwords, a client's holds tokens.
* **A connection per operation.** Opening one costs a file open and the
  pragmas, so nothing shares a `Connection` across threads or holds one across
  an `.await`, and the agent's blocking work stays inside `spawn_blocking`.
* **`open` runs the pending migrations** (see **Migrations**); `connect` only
  sets the pragmas, for a store this process has already opened.

**A broken store is set aside, never fatal.** A file that is not a database,
or fails `PRAGMA quick_check`, is renamed to `<name>.corrupt-<unix time>` with
its `-wal` and `-shm`, an empty store takes its place, and the agent logs
where the old one went. The kiosk comes up on the image's defaults, unclaimed,
which is what a reflash would give too, and the broken file is still there to
look at.

## The device's store

**`db.rs` is the only way the agent reaches `tessaro.db`.** A type kept there
implements `Stored`: it loads itself from its tables and saves itself back,
whole, inside the transaction it is given (`State` in `state.rs`, `Auth` in
`auth.rs`, `Secrets` in `secrets.rs`, `Schedules` in `schedules.rs`).

* **`read` never fails.** Whatever goes wrong is logged and the default comes
  back, so the boot oneshot always renders something and the agent always
  starts.
* **`update` is read, change, write in one `BEGIN IMMEDIATE`.** The write lock
  is taken before the read, so two writers in different processes cannot both
  read the old value. A change that returns an error rolls back and writes
  nothing.
* **`transaction` is the same for a change that spans types.** The network
  commit (`control/settings.rs`) writes a staged WiFi password to `secrets`
  and the settings to `settings` together, so a power cut cannot leave a
  password for a network the settings do not name.
* **A factory reset clears rows, not the file**: `tokens`, `settings`,
  `state`, `secrets`, `schedules`. The store and its migration history stay.
* **The settings are sparse**: a key that was never set has no row, so it
  follows the image's default (see [settings.md](settings.md)).
* **`net_txn` is the network transaction's record** while a change runs, and
  `net_last` what the last one did (see **One change is one transaction** in
  [networking.md](networking.md)).

**Not everything the agent keeps is in the store.** Files stay files where a
program other than the agent reads or writes them:

* `schedule-runs/<id>`, written by systemd's `ExecStopPost` shell line when a
  run ends ([scheduler.md](scheduler.md));
* `update/`, which `tessaro-flash` reads in the initramfs ([updates.md](updates.md));
* the TLS identity, ssh's `authorized_keys`, `/etc/shadow` and the extra
  certificate authorities, which their own programs read;
* everything the agent renders (`generated.env`, the policy, the keyfiles,
  the schedules' units), which is written with `store.rs` and only when its
  content changes.

## The session handover

**`sessions.db` carries Webconfig's browser sessions across a restart the
agent makes of itself** ([webconfig.md](webconfig.md)). `Sessions::save`
writes the sessions still good just before the restart; `Sessions::load` reads
them at the next start and empties the table in the same transaction, so a
later start never takes them again. It lives in `/run`, so a reboot ends every
session, as it should: a reboot is not a restart the operator asked to
survive.

## A client's store

**tessaro-ctl and tessaro-gui write single rows, never the whole list.**
`Nodes::load` is a copy read at one moment; `keep`, `forget`, `remember`,
`refresh` and `forget_session` write the one row they change straight to the
store as well (`nodes.rs`). The GUI keeps its copy for as long as it runs, and
a device window's worker writes from its own thread, so a whole-list save
from a stale copy would undo a token tessaro-ctl stored meanwhile.

* **Reading never makes the store.** Only a write creates the file and its
  directory, so a command that only reads works where the config dir cannot
  be written: tessaro-ctl on the device, run with no `HOME`, has `/`.
* **`gui_prefs`** holds the GUI's zoom, window placements and table widths and
  sorting, a JSON value per name (`Prefs` in `gui/tessaro-gui/src/main.rs`). A
  row that does not parse is ignored, as if it was never saved.
* **`known_hosts` stays a file** next to the store: it is handed to ssh as
  `UserKnownHostsFile` ([remote-access.md](remote-access.md)).

## Migrations

**The schema is changed only by migrations, Rails-style.** A migration is a
file `V<YYYYMMDDHHMMSS>__<name>.sql` in the store's `migrations/` directory,
compiled into the binary with refinery's `embed_migrations!`. `open` applies
the ones a store has not had yet, each in its own transaction, and records
them in `schema_migrations` (version, name, when, checksum).

* **An applied migration is never edited.** refinery checksums every one and
  refuses to open a store whose history does not match the binary's. A fix is
  a new migration.
* **The version is the time it was written**, which needs refinery's
  `int8-versions`: a timestamp does not fit its default `i32`.
* **The boot oneshot migrates the device's store before anything else runs**
  (`tessaro-config.service` is ordered before the agent), so the agent starts
  on the current schema. An image update is a new binary with new migrations,
  applied at its first boot.

**Renaming a setting is a data migration**, so devices in the field follow:
move the row, and the probation's key, and rewrite the placeholder in the
values that are templates. For `screen.osk` becoming `screen.keyboard`:

```sql
UPDATE OR IGNORE settings SET key = 'screen.keyboard' WHERE key = 'screen.osk';
DELETE FROM settings WHERE key = 'screen.osk';
UPDATE state SET pending_key = 'screen.keyboard' WHERE pending_key = 'screen.osk';
UPDATE settings SET value = replace(value, '{screen.osk}', '{screen.keyboard}')
    WHERE key IN ('browser.url', 'browser.debug.template');
```

`OR IGNORE` keeps a value already set under the new name; the `DELETE` then
drops the old one. The template keys are the registry's `Url` and `Template`
kinds (`keys.rs`).

## The sqlite3 shell

**The image carries `sqlite3`, to read the device's store and edit it by
hand** (`moonforge-image-base.bbappend`). It is safe to use while the agent
runs: WAL lets it read beside the agent, and a write waits for the agent's
transaction like any other writer.

```sh
sqlite3 /data/tessaro/tessaro.db '.tables'
sqlite3 /data/tessaro/tessaro.db 'SELECT key, value FROM settings'
sqlite3 /data/tessaro/tessaro.db 'SELECT id, name, issued_by FROM tokens'
```

* **A hand edit skips `config set`'s validation.** A value with a quote, a
  backslash, a `$` or a control character ends up in an env file; the agent
  skips unknown keys and falls back on values it cannot parse, but only `config
  set` guarantees a value is sound. Use `tessaro-ctl config set` for settings.
* **Nothing is told about a hand edit.** The rendered files follow at the next
  boot or the next change made with tessaro-ctl (`render::all`), the tokens
  and the settings the agent acts on at its next start, and the schedules at
  the next reconcile, within a minute (`watch_schedules`).
* **`secrets` holds the network passwords in clear**, like the keyfiles they
  are rendered into, which is why the store is 0600 and root's.
