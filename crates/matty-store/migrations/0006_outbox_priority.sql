-- Message priority for a queued send (#220): the post's `metadata.priority`
-- as JSON, or '' for none, so a send queued offline keeps its label,
-- acknowledgement request and persistent notifications.
ALTER TABLE outbox ADD COLUMN priority TEXT NOT NULL DEFAULT '';
