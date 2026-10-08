-- The barcode scanners (scanner/mod.rs): which USB device each is, how it
-- is read, and how its scans end. NULL is the transport's default.
CREATE TABLE scanners (
    name TEXT PRIMARY KEY NOT NULL,
    position INTEGER NOT NULL,
    transport TEXT NOT NULL,
    vendor TEXT NOT NULL,
    product TEXT NOT NULL,
    serial TEXT,
    port TEXT,
    layout TEXT,
    terminator TEXT,
    gap_ms INTEGER,
    baud INTEGER,
    strip_prefix TEXT,
    strip_suffix TEXT,
    enabled INTEGER NOT NULL DEFAULT 1
);

-- The scanners whose scans a script runs on (control/scanners.rs), as a JSON
-- array of names, `*` for every scanner. Empty for none.
ALTER TABLE scripts ADD COLUMN scanner TEXT NOT NULL DEFAULT '[]';
