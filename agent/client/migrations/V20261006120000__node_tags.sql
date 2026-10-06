-- A known device's tags as it last announced or reported them (device.tags,
-- comma separated), so one that is offline still shows them.
ALTER TABLE nodes ADD COLUMN tags TEXT NOT NULL DEFAULT '';
