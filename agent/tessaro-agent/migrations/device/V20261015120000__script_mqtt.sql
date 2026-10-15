-- The MQTT topic filters a script runs on (crate::mqtt), as a JSON array of
-- filters, `+` and `#` allowed (protocol::mqtt::triggers). Empty for none.
ALTER TABLE scripts ADD COLUMN mqtt TEXT NOT NULL DEFAULT '[]';
