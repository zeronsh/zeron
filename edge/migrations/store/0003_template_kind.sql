-- Templates in the store (contract a33b7c5b6, "Templates in the store"):
-- one `plugins` table holds both kinds, so ids, skeletons, claims and
-- reservations stay one namespace with no check across tables. SQLite can't
-- alter a CHECK, so `plugins` is rebuilt with `kind` widened to
-- ('plugin', 'template'); every row stays a plugin.
--
-- The rebuild keeps every row as it is, `seq` included: `plugin_search`
-- (FTS5) is keyed by `rowid = plugins.seq`. `versions.plugin_id`
-- references `plugins (id)`, so the foreign keys are checked at the end of
-- the migration (one transaction), not as the old table is dropped; and the
-- rows go back in *after* the drop, so that each one settles the versions
-- left without a parent by it (a deferred check counts those, and only an
-- insert into `plugins` itself pays them back). The copy isn't called
-- `plugins_new`: an index of that name exists (0001's "new" sort).
-- The list indexes now lead with `kind`, which every list filters on.
PRAGMA defer_foreign_keys = on;

CREATE TABLE plugins_0003_rows AS SELECT * FROM plugins;
DROP TABLE plugins;

CREATE TABLE plugins (
  seq INTEGER PRIMARY KEY,                       -- stable sort tiebreak and FTS rowid
  id TEXT NOT NULL UNIQUE,                       -- the manifest id, one namespace for both kinds
  kind TEXT NOT NULL DEFAULT 'plugin' CHECK (kind IN ('plugin', 'template')),  -- fixed when the id is claimed
  owner_id TEXT NOT NULL,
  -- When the owner came to own it: its claim (= created_at), or its last
  -- transfer. A deploy token scoped to `all` holds an item owned when the
  -- token was made, or claimed by the account and never moved (a315eef5a).
  owner_since INTEGER NOT NULL,
  -- username.ts `skeleton` of the id: a new id that looks like an existing one is refused.
  skeleton TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
  suspend_reason TEXT,
  featured_at INTEGER,
  installs INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  -- The card (store/db.ts `relist`): its newest listed version (a release
  -- before a pre-release), verified or not, and what the card shows of it;
  -- null while nothing is listed. `verified`: that version is verified.
  -- `verified_version`: the newest verified listed version, verified at
  -- `verified_at` (the catalog's `sort=verified`).
  listed_version TEXT,
  verified INTEGER NOT NULL DEFAULT 0 CHECK (verified IN (0, 1)),
  verified_version TEXT,
  verified_at INTEGER,
  name TEXT,
  summary TEXT,
  icon TEXT,                                     -- a plugin's; a template has none
  categories TEXT,                               -- JSON array (a template's is [])
  keywords TEXT,                                 -- JSON array
  author_username TEXT,                          -- for search only
  listed_at INTEGER,                             -- first listed ("new")
  updated_at INTEGER,                            -- the card's version published ("updated")
  cover TEXT                                     -- a plugin's first screenshot, a template's preview.png (0002)
);

INSERT INTO plugins (
  seq, id, kind, owner_id, owner_since, skeleton, status, suspend_reason, featured_at, installs, created_at,
  listed_version, verified, verified_version, verified_at, name, summary, icon, categories, keywords,
  author_username, listed_at, updated_at, cover
)
SELECT
  seq, id, kind, owner_id, owner_since, skeleton, status, suspend_reason, featured_at, installs, created_at,
  listed_version, verified, verified_version, verified_at, name, summary, icon, categories, keywords,
  author_username, listed_at, updated_at, cover
FROM plugins_0003_rows;

DROP TABLE plugins_0003_rows;

CREATE INDEX plugins_owner ON plugins (owner_id);
CREATE INDEX plugins_popular ON plugins (kind, installs DESC, seq DESC) WHERE listed_version IS NOT NULL AND status = 'active';
CREATE INDEX plugins_updated ON plugins (kind, updated_at DESC, seq DESC) WHERE listed_version IS NOT NULL AND status = 'active';
CREATE INDEX plugins_new ON plugins (kind, listed_at DESC, seq DESC) WHERE listed_version IS NOT NULL AND status = 'active';

-- The take-down feed names which kind an event is about (its `kind` is
-- already take_down | restore): kept with the event, since a released id's
-- row is gone by the time an old event is read.
ALTER TABLE takedown_events ADD COLUMN item_kind TEXT NOT NULL DEFAULT 'plugin' CHECK (item_kind IN ('plugin', 'template'));
