-- The presence events a script runs on (control/presence.rs), as a JSON
-- array of protocol::presence triggers: `arrived`, `left`, `near`, `far`.
-- Empty for none.
ALTER TABLE scripts ADD COLUMN presence TEXT NOT NULL DEFAULT '[]';
