-- The card's cover (contract "Cards and detail", Nov 24): the first
-- screenshot of the card's version, as a path in that version's package
-- (amendment 35, as `icon` is), or null when that version has none. Kept by
-- store/db.ts `relist` with the card's other fields; existing cards are
-- filled from their listed version here.
ALTER TABLE plugins ADD COLUMN cover TEXT;

UPDATE plugins SET cover = (
  SELECT json_extract(v.screenshots, '$[0]') FROM versions v
  WHERE v.plugin_id = plugins.id AND v.version = plugins.listed_version
)
WHERE listed_version IS NOT NULL;
