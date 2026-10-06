-- The playlists (playlists.rs), in `position` order. `items` is a JSON
-- array of protocol::playlist::PlaylistItem, kept whole: an item has no
-- life of its own outside its playlist.
CREATE TABLE playlists (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    name TEXT NOT NULL UNIQUE,
    transition TEXT NOT NULL,
    transition_ms INTEGER NOT NULL,
    items TEXT NOT NULL
);

-- The timetable (playlists.rs), in `position` order, which breaks a tie of
-- priorities. `days` is a JSON array of day names, empty for every day;
-- `playlist_id` is a row of `playlists`, checked by the agent (a save
-- rewrites every row, which a foreign key would refuse).
CREATE TABLE timetable (
    id TEXT PRIMARY KEY,
    position INTEGER NOT NULL,
    playlist_id TEXT NOT NULL,
    days TEXT NOT NULL,
    time_from TEXT NOT NULL,
    time_to TEXT NOT NULL,
    priority INTEGER NOT NULL,
    enabled INTEGER NOT NULL
);

-- The copies of the playlists' remote media (media.rs): where each source
-- is kept under the media cache, and what revalidating it needs.
CREATE TABLE media_cache (
    url TEXT PRIMARY KEY,
    file TEXT NOT NULL,
    etag TEXT,
    last_modified TEXT,
    size INTEGER,
    fetched_at INTEGER,
    error TEXT
);
