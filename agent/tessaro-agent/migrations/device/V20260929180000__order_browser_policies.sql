-- The browser policies in priority order (policies.rs): position 1 wins a
-- policy others set too, positions run 1..N without gaps.
ALTER TABLE browser_policies ADD COLUMN position INTEGER NOT NULL DEFAULT 0;

-- Until now the last name won, so the stored ones are numbered by name from
-- the last: every device keeps the policy it had.
UPDATE browser_policies
SET position = (
    SELECT count(*) FROM browser_policies AS later
    WHERE later.name >= browser_policies.name
);
