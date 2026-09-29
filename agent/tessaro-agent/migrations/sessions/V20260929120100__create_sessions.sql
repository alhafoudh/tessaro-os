-- Browser sessions handed from one agent process to the next across a
-- restart it makes of itself (api/sessions.rs). Rows live between `save`
-- and the next `load`, which empties the table.
CREATE TABLE sessions (
    id_sha TEXT PRIMARY KEY,
    token_id TEXT NOT NULL,
    token_sha TEXT NOT NULL,
    idle_s INTEGER NOT NULL
);
