-- The guarded changes on probation (state.rs), one row per key. One
-- `config set` can change more than one guarded key (screen.resolution and
-- screen.rotation), and they are confirmed or reverted together, so the
-- single pending row in `state` cannot hold them. A change on probation
-- when this runs moves over as it is.
CREATE TABLE pending (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    -- What a revert goes back to; NULL means the key was not set.
    previous TEXT
);

INSERT INTO pending (key, value, previous)
SELECT pending_key, pending_value, pending_previous
FROM state
WHERE pending_key IS NOT NULL;

-- The rest of State, one row: the compare-and-set counter.
CREATE TABLE state_revision (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    revision INTEGER NOT NULL DEFAULT 0
);

INSERT INTO state_revision (id, revision) SELECT id, revision FROM state;

DROP TABLE state;
ALTER TABLE state_revision RENAME TO state;
