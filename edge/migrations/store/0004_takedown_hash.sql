-- A take-down event names the version's content hash (contract
-- "Templates: after the trust review"): a device keeps it for as long as the
-- feed keeps the event, and refuses a file or link import of that package.
-- Null only for an event whose version is gone (none is, today).
ALTER TABLE takedown_events ADD COLUMN hash TEXT;

UPDATE takedown_events SET hash = (
  SELECT v.content_hash FROM versions v WHERE v.plugin_id = takedown_events.plugin_id AND v.version = takedown_events.version
);
