-- Files attached while offline (#194): a JSON array of local paths, read and
-- uploaded on reconnect before the post goes. Once they are up their server
-- ids join `file_ids` and this goes back to '[]'.
ALTER TABLE outbox ADD COLUMN local_files TEXT NOT NULL DEFAULT '[]';
