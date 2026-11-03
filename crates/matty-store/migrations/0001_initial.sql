-- Matty's cache of one account on one server (PLAN §6.5). Ids are the
-- server's 26-character ids; times are server milliseconds. Columns hold what
-- the sync engine queries; `data` holds the rest of the server object as JSON
-- so new fields can be read without a migration.

CREATE TABLE teams (
    id            TEXT PRIMARY KEY NOT NULL,
    name          TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    kind          TEXT NOT NULL,
    delete_at     INTEGER NOT NULL DEFAULT 0,
    sort_order    INTEGER,
    data          TEXT NOT NULL DEFAULT '{}'
) STRICT;

CREATE TABLE channels (
    id                    TEXT PRIMARY KEY NOT NULL,
    -- Empty for direct and group messages.
    team_id               TEXT NOT NULL DEFAULT '',
    kind                  TEXT NOT NULL,
    name                  TEXT NOT NULL,
    display_name          TEXT NOT NULL,
    last_post_at          INTEGER NOT NULL DEFAULT 0,
    last_root_post_at     INTEGER NOT NULL DEFAULT 0,
    total_msg_count       INTEGER NOT NULL DEFAULT 0,
    total_msg_count_root  INTEGER NOT NULL DEFAULT 0,
    update_at             INTEGER NOT NULL DEFAULT 0,
    delete_at             INTEGER NOT NULL DEFAULT 0,
    data                  TEXT NOT NULL DEFAULT '{}'
) STRICT;
CREATE INDEX channels_team ON channels (team_id);

CREATE TABLE channel_members (
    channel_id            TEXT NOT NULL,
    user_id               TEXT NOT NULL,
    roles                 TEXT NOT NULL DEFAULT '',
    last_viewed_at        INTEGER NOT NULL DEFAULT 0,
    msg_count             INTEGER NOT NULL DEFAULT 0,
    mention_count         INTEGER NOT NULL DEFAULT 0,
    msg_count_root        INTEGER NOT NULL DEFAULT 0,
    mention_count_root    INTEGER NOT NULL DEFAULT 0,
    urgent_mention_count  INTEGER NOT NULL DEFAULT 0,
    notify_props          TEXT NOT NULL DEFAULT '{}',
    last_update_at        INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, user_id)
) STRICT;
CREATE INDEX channel_members_user ON channel_members (user_id);

CREATE TABLE categories (
    id            TEXT PRIMARY KEY NOT NULL,
    team_id       TEXT NOT NULL,
    kind          TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    sorting       TEXT NOT NULL DEFAULT '',
    collapsed     INTEGER NOT NULL DEFAULT 0,
    muted         INTEGER NOT NULL DEFAULT 0,
    -- Position in the team's category order.
    sort_order    INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX categories_team ON categories (team_id, sort_order);

CREATE TABLE category_channels (
    category_id  TEXT NOT NULL REFERENCES categories (id) ON DELETE CASCADE,
    channel_id   TEXT NOT NULL,
    sort_order   INTEGER NOT NULL,
    PRIMARY KEY (category_id, channel_id)
) STRICT;
CREATE INDEX category_channels_channel ON category_channels (channel_id);

CREATE TABLE users (
    id                   TEXT PRIMARY KEY NOT NULL,
    username             TEXT NOT NULL,
    first_name           TEXT NOT NULL DEFAULT '',
    last_name            TEXT NOT NULL DEFAULT '',
    nickname             TEXT NOT NULL DEFAULT '',
    is_bot               INTEGER NOT NULL DEFAULT 0,
    last_picture_update  INTEGER NOT NULL DEFAULT 0,
    delete_at            INTEGER NOT NULL DEFAULT 0,
    data                 TEXT NOT NULL DEFAULT '{}'
) STRICT;
CREATE INDEX users_username ON users (username);

CREATE TABLE posts (
    id               TEXT PRIMARY KEY NOT NULL,
    channel_id       TEXT NOT NULL,
    user_id          TEXT NOT NULL,
    -- Empty for a root post.
    root_id          TEXT NOT NULL DEFAULT '',
    create_at        INTEGER NOT NULL,
    update_at        INTEGER NOT NULL DEFAULT 0,
    edit_at          INTEGER NOT NULL DEFAULT 0,
    delete_at        INTEGER NOT NULL DEFAULT 0,
    kind             TEXT NOT NULL DEFAULT '',
    message          TEXT NOT NULL DEFAULT '',
    reply_count      INTEGER NOT NULL DEFAULT 0,
    is_pinned        INTEGER NOT NULL DEFAULT 0,
    pending_post_id  TEXT NOT NULL DEFAULT '',
    props            TEXT NOT NULL DEFAULT '{}',
    metadata         TEXT NOT NULL DEFAULT '{}',
    -- When Matty last showed or fetched it, for LRU eviction (local clock).
    touched_at       INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX posts_channel_create ON posts (channel_id, create_at);
CREATE INDEX posts_root ON posts (root_id);
CREATE INDEX posts_pending ON posts (pending_post_id) WHERE pending_post_id != '';

CREATE TABLE reactions (
    post_id     TEXT NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    user_id     TEXT NOT NULL,
    emoji_name  TEXT NOT NULL,
    create_at   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (post_id, user_id, emoji_name)
) STRICT;

CREATE TABLE files (
    id          TEXT PRIMARY KEY NOT NULL,
    post_id     TEXT NOT NULL DEFAULT '',
    channel_id  TEXT NOT NULL DEFAULT '',
    name        TEXT NOT NULL DEFAULT '',
    extension   TEXT NOT NULL DEFAULT '',
    size        INTEGER NOT NULL DEFAULT 0,
    mime_type   TEXT NOT NULL DEFAULT '',
    width       INTEGER NOT NULL DEFAULT 0,
    height      INTEGER NOT NULL DEFAULT 0,
    has_preview INTEGER NOT NULL DEFAULT 0,
    create_at   INTEGER NOT NULL DEFAULT 0,
    delete_at   INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX files_post ON files (post_id);

CREATE TABLE threads (
    -- The root post's id.
    id               TEXT PRIMARY KEY NOT NULL,
    team_id          TEXT NOT NULL DEFAULT '',
    channel_id       TEXT NOT NULL DEFAULT '',
    reply_count      INTEGER NOT NULL DEFAULT 0,
    last_reply_at    INTEGER NOT NULL DEFAULT 0,
    last_viewed_at   INTEGER NOT NULL DEFAULT 0,
    unread_replies   INTEGER NOT NULL DEFAULT 0,
    unread_mentions  INTEGER NOT NULL DEFAULT 0,
    is_following     INTEGER NOT NULL DEFAULT 0,
    participants     TEXT NOT NULL DEFAULT '[]',
    delete_at        INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX threads_team_reply ON threads (team_id, last_reply_at);

CREATE TABLE drafts (
    channel_id  TEXT NOT NULL,
    -- Empty for a draft in the channel itself.
    root_id     TEXT NOT NULL DEFAULT '',
    message     TEXT NOT NULL DEFAULT '',
    file_ids    TEXT NOT NULL DEFAULT '[]',
    props       TEXT NOT NULL DEFAULT '{}',
    update_at   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (channel_id, root_id)
) STRICT;

-- Posts written but not yet acknowledged by the server, sent in order.
CREATE TABLE outbox (
    seq              INTEGER PRIMARY KEY AUTOINCREMENT,
    pending_post_id  TEXT NOT NULL UNIQUE,
    channel_id       TEXT NOT NULL,
    root_id          TEXT NOT NULL DEFAULT '',
    message          TEXT NOT NULL DEFAULT '',
    file_ids         TEXT NOT NULL DEFAULT '[]',
    props            TEXT NOT NULL DEFAULT '{}',
    created_at       INTEGER NOT NULL,
    attempts         INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT
) STRICT;

CREATE TABLE preferences (
    category  TEXT NOT NULL,
    name      TEXT NOT NULL,
    value     TEXT NOT NULL,
    PRIMARY KEY (category, name)
) STRICT;

CREATE TABLE emoji (
    id          TEXT PRIMARY KEY NOT NULL,
    name        TEXT NOT NULL,
    creator_id  TEXT NOT NULL DEFAULT '',
    update_at   INTEGER NOT NULL DEFAULT 0,
    delete_at   INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX emoji_name ON emoji (name);

-- Where each kind of sync left off: `since` timestamps from the server, the
-- WebSocket connection id and sequence, and similar.
CREATE TABLE sync_cursors (
    key    TEXT PRIMARY KEY NOT NULL,
    value  TEXT NOT NULL
) STRICT;
