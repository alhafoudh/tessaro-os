-- The HDMI-CEC events a script runs on (control/cec.rs), as a JSON array
-- of protocol::cec triggers: `tv-on`, `key`, `key:red`. Empty for none.
ALTER TABLE scripts ADD COLUMN cec TEXT NOT NULL DEFAULT '[]';
