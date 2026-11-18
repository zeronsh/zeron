-- The plugin store's catalog (edge/src/store/, docs/plans/2026-11-17-plugin-store.md).
-- Times are ms since the epoch. User ids are WorkOS ids; usernames live in
-- the users1 Durable Object and are copied here only for search.

-- One row per store id, from its first submission on.
CREATE TABLE plugins (
  seq INTEGER PRIMARY KEY,                       -- stable sort tiebreak and FTS rowid
  id TEXT NOT NULL UNIQUE,                       -- the manifest id
  kind TEXT NOT NULL DEFAULT 'plugin' CHECK (kind IN ('plugin')),
  owner_id TEXT NOT NULL,
  -- username.ts `skeleton` of the id: a new id that looks like an existing one is refused.
  skeleton TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'suspended')),
  suspend_reason TEXT,
  featured_at INTEGER,
  installs INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  -- The listed version (see store/db.ts `relist`) and what the card shows of it;
  -- null while nothing is listed.
  listed_version TEXT,
  name TEXT,
  summary TEXT,
  icon TEXT,
  categories TEXT,                               -- JSON array
  keywords TEXT,                                 -- JSON array
  author_username TEXT,                          -- for search only
  listed_at INTEGER,                             -- first listed ("new")
  updated_at INTEGER                             -- listed version last changed ("updated")
);
CREATE INDEX plugins_owner ON plugins (owner_id);
CREATE INDEX plugins_popular ON plugins (installs DESC, seq DESC) WHERE listed_version IS NOT NULL AND status = 'active';
CREATE INDEX plugins_updated ON plugins (updated_at DESC, seq DESC) WHERE listed_version IS NOT NULL AND status = 'active';
CREATE INDEX plugins_new ON plugins (listed_at DESC, seq DESC) WHERE listed_version IS NOT NULL AND status = 'active';

-- Every version ever submitted. A version belongs to its content for good.
-- Its R2 objects are written before its row, so a row means they exist.
CREATE TABLE versions (
  plugin_id TEXT NOT NULL REFERENCES plugins (id),
  version TEXT NOT NULL,                         -- as written (build metadata kept)
  precedence TEXT NOT NULL,                      -- without build metadata: the identity
  major INTEGER NOT NULL,
  minor INTEGER NOT NULL,
  patch INTEGER NOT NULL,
  is_release INTEGER NOT NULL CHECK (is_release IN (0, 1)),
  pre_key TEXT NOT NULL,                         -- semver.ts `preKey`
  status TEXT NOT NULL CHECK (status IN ('pending', 'approved', 'rejected', 'withdrawn', 'yanked', 'removed')),
  blocked INTEGER NOT NULL DEFAULT 0,
  block_reason TEXT,
  publisher_id TEXT NOT NULL,
  via TEXT NOT NULL CHECK (via IN ('session', 'deploy_key')),
  signer_key TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  package_sha256 TEXT NOT NULL,
  package_bytes INTEGER NOT NULL,
  files_count INTEGER NOT NULL,
  files_bytes INTEGER NOT NULL,
  manifest TEXT NOT NULL,                        -- canonical JSON, without its signature (≤ 256 KB)
  name TEXT NOT NULL,
  summary TEXT NOT NULL,
  icon TEXT NOT NULL,
  categories TEXT NOT NULL,
  keywords TEXT NOT NULL,
  homepage TEXT,
  repository TEXT,
  license TEXT,
  screenshots TEXT NOT NULL,
  engines_zeron TEXT,
  permissions TEXT NOT NULL,                     -- consent.ts item, JSON
  readme TEXT NOT NULL,
  notes TEXT,
  checks TEXT NOT NULL,                          -- checks.ts list, JSON
  submitted_at INTEGER NOT NULL,
  decided_at INTEGER,
  decided_by TEXT,
  reason TEXT,                                   -- a rejection's or removal's reason word
  message TEXT,                                  -- what the author reads
  PRIMARY KEY (plugin_id, version),
  UNIQUE (plugin_id, precedence)
);
CREATE INDEX versions_order ON versions (plugin_id, major, minor, patch, is_release, pre_key);
CREATE INDEX versions_queue ON versions (status, submitted_at);
CREATE INDEX versions_publisher ON versions (publisher_id, submitted_at);

-- Each file of a version: where its bytes start in the version's files
-- object in R2 (files/<id>/<version>/<package sha256>.bin), and how many.
CREATE TABLE version_files (
  plugin_id TEXT NOT NULL,
  version TEXT NOT NULL,
  path TEXT NOT NULL,
  at INTEGER NOT NULL,
  bytes INTEGER NOT NULL,
  sha256 TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('png', 'jpeg', 'svg', 'text', 'binary')),
  PRIMARY KEY (plugin_id, version, path)
);

-- The listed version's categories and keywords, for the filters.
CREATE TABLE plugin_categories (
  plugin_id TEXT NOT NULL,
  category TEXT NOT NULL,
  PRIMARY KEY (category, plugin_id)
);
CREATE TABLE plugin_keywords (
  plugin_id TEXT NOT NULL,
  keyword TEXT NOT NULL,
  PRIMARY KEY (keyword, plugin_id)
);

-- Listed plugins only; rowid = plugins.seq.
CREATE VIRTUAL TABLE plugin_search USING fts5 (
  name, summary, keywords, author, readme,
  tokenize = 'unicode61 remove_diacritics 2',
  prefix = '2 3 4'
);

CREATE TABLE installs (
  plugin_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY (plugin_id, user_id)
);

CREATE TABLE reports (
  id INTEGER PRIMARY KEY,
  plugin_id TEXT NOT NULL,
  version TEXT,
  reporter_id TEXT NOT NULL,
  reason TEXT NOT NULL CHECK (reason IN ('malware', 'misleading', 'broken', 'copyright', 'other')),
  text TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'dismissed', 'actioned')),
  resolved_by TEXT,
  resolved_at INTEGER,
  note TEXT
);
CREATE INDEX reports_open ON reports (status, id);
-- One open report per person per plugin, however many arrive at once.
CREATE UNIQUE INDEX reports_one_open ON reports (plugin_id, reporter_id) WHERE status = 'open';
CREATE INDEX reports_plugin ON reports (plugin_id, id);
CREATE INDEX reports_reporter ON reports (reporter_id, created_at);

-- Every admin action, written in the same transaction as the change.
CREATE TABLE audit (
  seq INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  actor_id TEXT NOT NULL,
  action TEXT NOT NULL,
  plugin_id TEXT,
  version TEXT,
  detail TEXT                                    -- JSON
);

-- Every new id an account claimed, kept when the id is released again: an
-- account claims at most 5 in any 7 days (amendment 30).
CREATE TABLE id_claims (
  user_id TEXT NOT NULL,
  plugin_id TEXT NOT NULL,
  at INTEGER NOT NULL
);
CREATE INDEX id_claims_user ON id_claims (user_id, at);

-- What a release (store/release.ts) still owes after its rows went, in the
-- same transaction: R2 objects to delete, and a plugin's deploy keys to
-- revoke. Each sweep finishes these first, so a release interrupted midway
-- is completed by the next one.
CREATE TABLE object_purges (
  key TEXT PRIMARY KEY,
  at INTEGER NOT NULL
);
CREATE TABLE key_revocations (
  plugin_id TEXT NOT NULL,
  at INTEGER NOT NULL,
  PRIMARY KEY (plugin_id, at)
);

-- R2 objects a publish wrote and may not need: one that threw before or
-- during its D1 batch, or lost the version to other bytes. Never deleted on
-- the spot (a retry of the same bytes writes the same keys, and its row may
-- yet land): the sweep deletes one after a grace period, and only if no
-- version row names it.
CREATE TABLE object_orphans (
  key TEXT PRIMARY KEY,
  at INTEGER NOT NULL
);

-- Ids kept back from first come, first served: for nobody (user_id null), or
-- for the one account that may take one the reserved-name rules refuse.
CREATE TABLE reservations (
  id TEXT PRIMARY KEY,
  user_id TEXT,
  note TEXT,
  created_by TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
