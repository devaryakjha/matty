-- Channel bookmarks (#226), kept so the bar shows offline. `data` is the
-- server's bookmark JSON with its file; the other columns order and find it.
CREATE TABLE channel_bookmarks (
    id          TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    create_at   INTEGER NOT NULL DEFAULT 0,
    data        TEXT NOT NULL DEFAULT '{}'
) STRICT;

CREATE INDEX channel_bookmarks_channel ON channel_bookmarks (channel_id, sort_order);
