-- What a root post's reply footer shows under Collapsed Reply Threads
-- (#123): when the last reply came, who replied (a JSON array of user ids,
-- newest first) and whether the user follows the thread (NULL: unknown).
ALTER TABLE posts ADD COLUMN last_reply_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE posts ADD COLUMN participants TEXT NOT NULL DEFAULT '[]';
ALTER TABLE posts ADD COLUMN is_following INTEGER;
