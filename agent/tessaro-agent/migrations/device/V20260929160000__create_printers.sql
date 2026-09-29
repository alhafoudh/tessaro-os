-- The printers (printer.rs), in `position` order, each set up as the CUPS
-- queue of the same name. `kind` is `ipp` or `raw`; `media` is empty for the
-- printer's own paper. `is_default` marks the one window.print() uses: at
-- most one row has it.
CREATE TABLE printers (
    name TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    uri TEXT NOT NULL,
    kind TEXT NOT NULL,
    media TEXT,
    is_default INTEGER NOT NULL DEFAULT 0
);

CREATE UNIQUE INDEX printers_one_default ON printers (is_default) WHERE is_default = 1;
