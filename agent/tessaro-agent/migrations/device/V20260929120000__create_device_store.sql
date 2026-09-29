-- What has been set on the device (state.rs). Sparse: a key that was never
-- set has no row and follows the image's default.
CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- The rest of State, one row: the compare-and-set counter and the guarded
-- change on probation, if one is.
CREATE TABLE state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    revision INTEGER NOT NULL DEFAULT 0,
    pending_key TEXT,
    pending_value TEXT,
    pending_previous TEXT,
    CHECK ((pending_key IS NULL) = (pending_value IS NULL))
);

-- Who may use the TCP API (auth.rs): only a SHA-256 of each token. In
-- rowid order, the order they were issued.
CREATE TABLE tokens (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    sha256 TEXT NOT NULL UNIQUE,
    issued_by TEXT NOT NULL
);

-- The network passwords (secrets.rs), never in settings: `config get` and
-- `config keys` print every setting.
CREATE TABLE secrets (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- The schedules (schedules.rs), in `position` order. `calendar` and `lines`
-- are JSON arrays of strings.
CREATE TABLE schedules (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    name TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL,
    calendar TEXT NOT NULL,
    lines TEXT NOT NULL,
    on_error TEXT NOT NULL,
    timeout_s INTEGER
);

-- A network change under way (nm/txn.rs), one row while it runs.
CREATE TABLE net_txn (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    action TEXT NOT NULL,
    checkpoint TEXT
);

-- What the last network change did, one row: a JSON NetChange.
CREATE TABLE net_last (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    change TEXT NOT NULL
);
