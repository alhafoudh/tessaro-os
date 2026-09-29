-- The browser policies (policies.rs): each document by name, its text as
-- typed, comments included, so an editor gets it back as it was written.
CREATE TABLE browser_policies (
    name TEXT PRIMARY KEY,
    text TEXT NOT NULL
);
