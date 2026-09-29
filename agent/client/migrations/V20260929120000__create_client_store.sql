-- The devices this client knows (nodes.rs), keyed by node id. In rowid
-- order, the order they were first met. `token` is NULL for a device only
-- pinned.
CREATE TABLE nodes (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    address TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    token TEXT
);

-- tessaro-gui's window and table preferences, each a JSON value by name.
CREATE TABLE gui_prefs (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
