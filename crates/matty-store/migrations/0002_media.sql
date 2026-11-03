-- Index of the on-disk media cache (#30). The bytes live in files next to
-- the database; this table says which keys exist, how big they are and when
-- they were last used, for LRU eviction.
CREATE TABLE media (
    key           TEXT PRIMARY KEY NOT NULL,
    -- Relative to the media folder.
    path          TEXT NOT NULL,
    size          INTEGER NOT NULL,
    content_type  TEXT NOT NULL DEFAULT '',
    -- Local clock in ms, nudged so every access sorts after the previous one.
    accessed_at   INTEGER NOT NULL
) STRICT;
CREATE INDEX media_accessed ON media (accessed_at);
