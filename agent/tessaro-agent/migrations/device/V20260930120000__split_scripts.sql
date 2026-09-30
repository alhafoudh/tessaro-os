-- The scripts (scripts.rs), in `position` order: a shell body and how its
-- runs behave. Schedules no longer carry command lines, they name a script;
-- the schedules stored before this are dropped, not converted.
CREATE TABLE scripts (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    name TEXT NOT NULL UNIQUE,
    description TEXT NOT NULL,
    body TEXT NOT NULL,
    on_error TEXT NOT NULL,
    timeout_s INTEGER,
    concurrency TEXT NOT NULL,
    bridge INTEGER NOT NULL
);

DROP TABLE schedules;

-- The schedules (schedules.rs), in `position` order. `calendar` is a JSON
-- array of strings; `script_id` is a row of `scripts`, checked by the agent
-- (a save rewrites every row, which a foreign key would refuse).
CREATE TABLE schedules (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    name TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL,
    calendar TEXT NOT NULL,
    script_id TEXT NOT NULL
);
