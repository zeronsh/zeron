/**
 * The store's D1 catalog (migrations/store): reads and the statements that
 * keep a plugin's listing in step with its versions.
 *
 * A version is listed the moment it is published (amendment 45); a review
 * only marks it verified. A plugin is listed while it is `active` and has a
 * listed version. Its card shows its newest listed version, verified or
 * not: its highest listed release, or its highest listed pre-release when it
 * has no release; and beside it the newest verified one. `relist`
 * recomputes those, the card fields copied from it, its search row and its
 * filter rows; it is idempotent, so every change that can move the listing
 * ends its batch with it.
 */
import type { Env } from "../env";
import type { PluginOwnership } from "../user-directory";

export interface PluginRow {
  readonly seq: number;
  readonly id: string;
  readonly kind: string;
  readonly owner_id: string;
  readonly owner_since: number;
  readonly skeleton: string;
  readonly status: "active" | "suspended";
  readonly suspend_reason: string | null;
  readonly featured_at: number | null;
  readonly installs: number;
  readonly created_at: number;
  readonly listed_version: string | null;
  readonly verified: number;
  readonly verified_version: string | null;
  /** When `verified_version` was verified. */
  readonly verified_at: number | null;
  readonly name: string | null;
  readonly summary: string | null;
  readonly icon: string | null;
  readonly categories: string | null;
  readonly keywords: string | null;
  readonly author_username: string | null;
  readonly listed_at: number | null;
  readonly updated_at: number | null;
}

export type VersionStatus = "listed" | "yanked" | "taken_down" | "removed";

export interface VersionRow {
  readonly plugin_id: string;
  readonly version: string;
  readonly precedence: string;
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
  readonly is_release: number;
  readonly pre_key: string;
  readonly status: VersionStatus;
  readonly yanked_by: "author" | "superseded" | null;
  readonly removed_at: number | null;
  readonly removed_by: string | null;
  readonly removal_reason: string | null;
  readonly verification: "unverified" | "verified";
  readonly verified_at: number | null;
  readonly verified_by: string | null;
  readonly declined_at: number | null;
  readonly declined_by: string | null;
  readonly decline_reason: string | null;
  readonly decline_message: string | null;
  readonly superseded_at: number | null;
  readonly superseded_by: string | null;
  readonly publisher_id: string;
  readonly via: "device" | "deploy_key";
  readonly deploy_key_name: string | null;
  readonly signer_key: string;
  readonly content_hash: string;
  readonly package_sha256: string;
  readonly package_bytes: number;
  readonly files_count: number;
  readonly files_bytes: number;
  readonly manifest: string;
  readonly name: string;
  readonly summary: string;
  readonly icon: string;
  readonly categories: string;
  readonly keywords: string;
  readonly homepage: string | null;
  readonly repository: string | null;
  readonly license: string | null;
  readonly screenshots: string;
  readonly engines_zeron: string | null;
  readonly permissions: string;
  readonly readme: string;
  readonly notes: string | null;
  readonly checks: string;
  readonly published_at: number;
}

/** Highest precedence first. */
export const VERSION_ORDER = "major DESC, minor DESC, patch DESC, is_release DESC, pre_key DESC";
/**
 * The superseding rule (amendment 45) for version `version` of plugin `id`
 * once it is listed, however that happened (a publish, an author's unyank, a
 * restore): every older unverified version on its line that isn't already
 * superseded, listed or yanked, stops taking new installs for good. A
 * release's line is every older version; a pre-release's, older
 * pre-releases. Decided in the statement, from the row as it is then: it
 * does nothing unless `version` is listed when it runs, so it belongs in the
 * same batch as the change that lists it.
 */
export const supersedeOlder = (db: D1Database, id: string, version: string, at: number): D1PreparedStatement =>
  db
    .prepare(
      `UPDATE versions SET status = 'yanked', yanked_by = 'superseded', superseded_at = ?3, superseded_by = ?2
       WHERE plugin_id = ?1 AND version != ?2 AND verification = 'unverified' AND status IN ('listed', 'yanked') AND superseded_at IS NULL
         AND EXISTS (SELECT 1 FROM versions x WHERE x.plugin_id = ?1 AND x.version = ?2 AND x.status = 'listed'
           AND (x.is_release = 1 OR versions.is_release = 0)
           AND (versions.major, versions.minor, versions.patch, versions.is_release, versions.pre_key) < (x.major, x.minor, x.patch, x.is_release, x.pre_key))`
    )
    .bind(id, version, at);

/** The versions `version` superseded at `at` (to purge them). */
export const supersededAt = async (db: D1Database, id: string, version: string, at: number): Promise<string[]> =>
  (
    await db
      .prepare(`SELECT version FROM versions WHERE plugin_id = ? AND superseded_by = ? AND superseded_at = ? AND yanked_by = 'superseded' ORDER BY ${VERSION_ORDER}`)
      .bind(id, version, at)
      .all<{ version: string }>()
  ).results.map((r) => r.version);

/** The listed version first: releases before pre-releases, then precedence. */
const LISTING_ORDER = "is_release DESC, major DESC, minor DESC, patch DESC, pre_key DESC";
/** Where a plugin shows to the public. */
export const LISTED = "p.status = 'active' AND p.listed_version IS NOT NULL";
/** Versions anyone may see and download. */
export const PUBLIC_STATUSES = ["listed", "yanked"] as const;
/** The same, as SQL for a `versions` row (unqualified, or with `alias.`). */
export const publicVersion = (alias = ""): string => `${alias}status IN ('listed', 'yanked')`;
/** A version no reviewer has decided yet (verified or declined). */
export const undecided = (alias = ""): string => `${alias}verification = 'unverified' AND ${alias}declined_at IS NULL`;

/** R2 keys: `<prefix><id>/<version>/<sha256><suffix>`, a package's and a
 * version's files object's. */
export const PACKAGE_KEY = { prefix: "pkg/", suffix: ".zeron-plugin" } as const;
export const FILES_KEY = { prefix: "files/", suffix: ".bin" } as const;
export const packageKey = (id: string, version: string, sha256: string): string => `${PACKAGE_KEY.prefix}${id}/${version}/${sha256}${PACKAGE_KEY.suffix}`;
export const filesKey = (id: string, version: string, sha256: string): string => `${FILES_KEY.prefix}${id}/${version}/${sha256}${FILES_KEY.suffix}`;

export const pluginRow = (db: D1Database, id: string): Promise<PluginRow | null> =>
  db.prepare("SELECT * FROM plugins WHERE id = ?").bind(id).first<PluginRow>();

export const versionRow = (db: D1Database, id: string, version: string): Promise<VersionRow | null> =>
  db.prepare("SELECT * FROM versions WHERE plugin_id = ? AND version = ?").bind(id, version).first<VersionRow>();

/** Who owns store id `id`, since when, and when it was first claimed (for
 * the identity lane: a deploy key's scope is judged by it, amendment 44),
 * or null when no one has claimed it. */
export const pluginOwner = async (env: Pick<Env, "STORE_DB">, id: string): Promise<PluginOwnership | null> => {
  if (!env.STORE_DB) return null;
  const row = await env.STORE_DB.prepare("SELECT owner_id, owner_since, created_at FROM plugins WHERE id = ?").bind(id).first<{ owner_id: string; owner_since: number; created_at: number }>();
  return row ? { owner: row.owner_id, ownerSince: row.owner_since, createdAt: row.created_at } : null;
};

/**
 * The statements that bring plugin `id`'s listing in step with its
 * versions. `author` refreshes the username its search row carries (null:
 * keep the one it has).
 */
export const relist = (db: D1Database, id: string, author: string | null): D1PreparedStatement[] => [
  db
    .prepare(
      `WITH best AS (
         SELECT * FROM versions WHERE plugin_id = ?1 AND status = 'listed' ORDER BY ${LISTING_ORDER} LIMIT 1
       ), vetted AS (
         SELECT version, verified_at FROM versions WHERE plugin_id = ?1 AND status = 'listed' AND verification = 'verified' ORDER BY ${LISTING_ORDER} LIMIT 1
       )
       UPDATE plugins SET
         listed_version = (SELECT version FROM best),
         verified = COALESCE((SELECT verification = 'verified' FROM best), 0),
         verified_version = (SELECT version FROM vetted),
         verified_at = (SELECT verified_at FROM vetted),
         name = (SELECT name FROM best),
         summary = (SELECT summary FROM best),
         icon = (SELECT icon FROM best),
         categories = (SELECT categories FROM best),
         keywords = (SELECT keywords FROM best),
         author_username = COALESCE(?2, author_username),
         listed_at = COALESCE(listed_at, (SELECT published_at FROM best)),
         updated_at = (SELECT published_at FROM best)
       WHERE id = ?1`
    )
    .bind(id, author),
  db.prepare("DELETE FROM plugin_search WHERE rowid = (SELECT seq FROM plugins WHERE id = ?)").bind(id),
  db
    .prepare(
      `INSERT INTO plugin_search (rowid, name, summary, keywords, author, readme)
       SELECT p.seq, p.name, p.summary, COALESCE((SELECT group_concat(value, ' ') FROM json_each(p.keywords)), ''),
              COALESCE(p.author_username, ''), substr(v.readme, 1, 65536)
       FROM plugins p JOIN versions v ON v.plugin_id = p.id AND v.version = p.listed_version
       WHERE p.id = ? AND ${LISTED}`
    )
    .bind(id),
  db.prepare("DELETE FROM plugin_categories WHERE plugin_id = ?").bind(id),
  db
    .prepare(
      `INSERT INTO plugin_categories (plugin_id, category)
       SELECT p.id, j.value FROM plugins p, json_each(p.categories) j WHERE p.id = ? AND ${LISTED}`
    )
    .bind(id),
  db.prepare("DELETE FROM plugin_keywords WHERE plugin_id = ?").bind(id),
  db
    .prepare(
      `INSERT INTO plugin_keywords (plugin_id, keyword)
       SELECT p.id, j.value FROM plugins p, json_each(p.keywords) j WHERE p.id = ? AND ${LISTED}`
    )
    .bind(id)
];

/** An audit row for the change the statement before it made, written only
 * if that statement changed a row: the same transaction as the change. */
export const auditIfChanged = (
  db: D1Database,
  at: number,
  actor: string,
  action: string,
  pluginId: string | null,
  version: string | null,
  detail: unknown
): D1PreparedStatement =>
  db
    .prepare("INSERT INTO audit (at, actor_id, action, plugin_id, version, detail) SELECT ?, ?, ?, ?, ?, ? WHERE changes() > 0")
    .bind(at, actor, action, pluginId, version, detail === undefined ? null : JSON.stringify(detail));

export const parseList = (json: string | null): string[] => {
  if (!json) return [];
  try {
    const value: unknown = JSON.parse(json);
    return Array.isArray(value) ? value.filter((v): v is string => typeof v === "string") : [];
  } catch {
    return [];
  }
};
