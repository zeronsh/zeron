/**
 * Unused ids come back (contract amendment 30). An id that has never had an
 * approved version and has no pending one is released 90 days after its
 * last submission: its plugin row (and so its id and lookalike skeleton),
 * its versions' records and file rows, and its packages and files in R2 go;
 * its deploy keys are revoked. What stays: the audit log (a
 * `plugin.release` entry naming every version released, so the history of
 * what was submitted and decided survives), and the claim in `id_claims`
 * (which counts towards the claimer's weekly limit as before). The sweep
 * never releases an id an admin reserved for an account.
 *
 * "Never approved" is "no version approved, yanked or removed": only an
 * approved version becomes either. The hourly cron sweeps
 * (`releaseUnusedIds`); an admin releases one at once through the
 * reservations routes (`releaseId` with no wait).
 *
 * No release is ever left half done. Its rows go in one D1 batch, which
 * also records, in the same transaction, every R2 object it owes
 * (`object_purges`) and the deploy-key revocation (`key_revocations`); the
 * objects and keys go after (`finishReleases`). If that's interrupted, the
 * records stay, and every sweep finishes what any release still owes
 * before it releases anything new.
 */
import type { Env } from "../env";
import { FILES_KEY, PACKAGE_KEY } from "./db";
import { revokeDeployKeys } from "./principal";

export const UNUSED_ID_DAYS = 90;
const DAY = 86_400_000;
/** Ids released, and owed objects deleted, per sweep at most. */
const SWEEP = 100;
const PURGE_BATCH = 500;

/** SQL true for plugin `?1` when it is unused and its last submission (or
 * its claim, if it has none) is older than `?2`, and (`?3` = 1) no admin
 * reserved it for an account. */
const RELEASABLE = `NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?1 AND status IN ('pending', 'approved', 'yanked', 'removed'))
  AND COALESCE((SELECT MAX(submitted_at) FROM versions WHERE plugin_id = ?1), (SELECT created_at FROM plugins WHERE id = ?1)) < ?2
  AND (?3 = 0 OR NOT EXISTS (SELECT 1 FROM reservations WHERE id = ?1 AND user_id IS NOT NULL))`;

/**
 * The rows of a release, in one batch: what it owes recorded, then the rows
 * gone, each statement guarded, so a submission landing meanwhile keeps the
 * id. True when it was released. `honourReservations`: the sweep's (an
 * admin's own release doesn't).
 */
export const releaseRows = async (db: D1Database, id: string, before: number, actor: string, honourReservations: boolean): Promise<boolean> => {
  const now = Date.now();
  const cutoff = Number.isFinite(before) ? before : Number.MAX_SAFE_INTEGER;
  const reserved = honourReservations ? 1 : 0;
  const versions = (await db.prepare("SELECT version, status FROM versions WHERE plugin_id = ?").bind(id).all<{ version: string; status: string }>()).results;
  const owed = (key: { readonly prefix: string; readonly suffix: string }) =>
    db
      .prepare(
        `INSERT INTO object_purges (key, at) SELECT ?4 || plugin_id || '/' || version || '/' || package_sha256 || ?5, ?6
         FROM versions WHERE plugin_id = ?1 AND ${RELEASABLE} ON CONFLICT DO NOTHING`
      )
      .bind(id, cutoff, reserved, key.prefix, key.suffix, now);
  const results = await db.batch([
    owed(PACKAGE_KEY),
    owed(FILES_KEY),
    db.prepare(`INSERT INTO key_revocations (plugin_id, at) SELECT ?1, ?4 WHERE ${RELEASABLE} AND EXISTS (SELECT 1 FROM plugins WHERE id = ?1) ON CONFLICT DO NOTHING`).bind(id, cutoff, reserved, now),
    db.prepare(`DELETE FROM version_files WHERE plugin_id = ?1 AND ${RELEASABLE}`).bind(id, cutoff, reserved),
    db.prepare(`DELETE FROM versions WHERE plugin_id = ?1 AND ${RELEASABLE}`).bind(id, cutoff, reserved),
    db
      .prepare(
        `DELETE FROM plugins WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?1) AND created_at < ?2
           AND (?3 = 0 OR NOT EXISTS (SELECT 1 FROM reservations WHERE id = ?1 AND user_id IS NOT NULL))`
      )
      .bind(id, cutoff, reserved),
    db
      .prepare("INSERT INTO audit (at, actor_id, action, plugin_id, version, detail) SELECT ?, ?, 'plugin.release', ?, NULL, ? WHERE changes() > 0")
      .bind(now, actor, id, JSON.stringify({ versions }))
  ]);
  return (results[5]?.meta.changes ?? 0) > 0;
};

/** Finish what releases still owe: delete their R2 objects and revoke their
 * deploy keys, clearing each record once done. Answers how much was done. */
export const finishReleases = async (env: Env, db: D1Database, bucket: R2Bucket): Promise<number> => {
  let done = 0;
  for (;;) {
    const keys = (await db.prepare("SELECT key FROM object_purges ORDER BY at LIMIT ?").bind(PURGE_BATCH).all<{ key: string }>()).results.map((r) => r.key);
    if (keys.length === 0) break;
    await bucket.delete(keys);
    await db.prepare("DELETE FROM object_purges WHERE key IN (SELECT value FROM json_each(?))").bind(JSON.stringify(keys)).run();
    done += keys.length;
    if (keys.length < PURGE_BATCH) break;
  }
  const revocations = (await db.prepare("SELECT plugin_id, at FROM key_revocations ORDER BY at LIMIT ?").bind(SWEEP).all<{ plugin_id: string; at: number }>()).results;
  for (const r of revocations) {
    // Claimed again since: its new owner's keys are theirs (and the old
    // owner's no longer verify, amendment 28).
    const again = await db.prepare("SELECT 1 FROM plugins WHERE id = ? AND created_at >= ?").bind(r.plugin_id, r.at).first();
    if (!again) await revokeDeployKeys(env, r.plugin_id);
    await db.prepare("DELETE FROM key_revocations WHERE plugin_id = ? AND at = ?").bind(r.plugin_id, r.at).run();
    done++;
  }
  return done;
};

/** Release `id` now if it is unused (an admin's: reservations aside). */
export const releaseId = async (env: Env, db: D1Database, bucket: R2Bucket, id: string, before: number, actor: string): Promise<boolean> => {
  const released = await releaseRows(db, id, before, actor, false);
  if (released) await finishReleases(env, db, bucket);
  return released;
};

/** How long a publish's possibly orphaned object waits before the sweep
 * looks at it: far past any publish still in flight. */
export const ORPHAN_GRACE_MS = 3_600_000;

/** `<prefix><id>/<version>/<sha256><suffix>` read back, or undefined. */
const keyNames = (key: string): { id: string; version: string; sha256: string } | undefined => {
  for (const k of [PACKAGE_KEY, FILES_KEY]) {
    if (!key.startsWith(k.prefix) || !key.endsWith(k.suffix)) continue;
    const parts = key.slice(k.prefix.length, key.length - k.suffix.length).split("/");
    if (parts.length === 3) return { id: parts[0]!, version: parts[1]!, sha256: parts[2]! };
  }
  return undefined;
};

/** Delete the objects publishes recorded as possibly orphaned, once their
 * grace is over and only where no version row names them; a named one is
 * kept and its record cleared. Answers how many were deleted. */
export const collectOrphans = async (db: D1Database, bucket: R2Bucket, now: number): Promise<number> => {
  const rows = (await db.prepare("SELECT key FROM object_orphans WHERE at < ? ORDER BY at LIMIT ?").bind(now - ORPHAN_GRACE_MS, PURGE_BATCH).all<{ key: string }>()).results;
  let deleted = 0;
  for (const { key } of rows) {
    // Claimed first, while it's still old: a publish of these bytes that
    // started since refreshed the record (publish.ts `recordObjects`, before
    // its R2 writes), and then the object is its, not the sweep's.
    const claimed = await db.prepare("DELETE FROM object_orphans WHERE key = ? AND at < ?").bind(key, now - ORPHAN_GRACE_MS).run();
    if ((claimed.meta.changes ?? 0) === 0) continue;
    const names = keyNames(key);
    const named =
      names !== undefined &&
      (await db.prepare("SELECT 1 FROM versions WHERE plugin_id = ? AND version = ? AND package_sha256 = ?").bind(names.id, names.version, names.sha256).first()) !== null;
    // (A fresh record now means a publish began after the claim: keep it.)
    const fresh = (await db.prepare("SELECT 1 FROM object_orphans WHERE key = ?").bind(key).first()) !== null;
    if (!named && !fresh) {
      await bucket.delete(key);
      deleted++;
    }
  }
  return deleted;
};

/** The hourly sweep: finish what earlier releases owe, collect publishes'
 * orphaned objects, then release ids unused for `UNUSED_ID_DAYS` (never one
 * reserved for an account). */
export const releaseUnusedIds = async (env: Env, now: number): Promise<number> => {
  const db = env.STORE_DB;
  const bucket = env.PLUGIN_STORE;
  if (!db || !bucket) return 0;
  await finishReleases(env, db, bucket);
  const orphans = await collectOrphans(db, bucket, now);
  const before = now - UNUSED_ID_DAYS * DAY;
  const stale = await db
    .prepare(
      `SELECT p.id FROM plugins p
       WHERE NOT EXISTS (SELECT 1 FROM versions v WHERE v.plugin_id = p.id AND v.status IN ('pending', 'approved', 'yanked', 'removed'))
         AND COALESCE((SELECT MAX(submitted_at) FROM versions v WHERE v.plugin_id = p.id), p.created_at) < ?
         AND NOT EXISTS (SELECT 1 FROM reservations r WHERE r.id = p.id AND r.user_id IS NOT NULL)
       LIMIT ?`
    )
    .bind(before, SWEEP)
    .all<{ id: string }>();
  let released = 0;
  for (const { id } of stale.results) if (await releaseRows(db, id, before, "system", true)) released++;
  if (released) await finishReleases(env, db, bucket);
  // One line an hour, so an operator sees it ran (`wrangler tail`).
  const owed = await db
    .prepare("SELECT (SELECT COUNT(*) FROM object_purges) + (SELECT COUNT(*) FROM key_revocations) AS n")
    .first<number>("n");
  console.log(`store: sweep: released ${released} unused ids, deleted ${orphans} orphaned objects, ${owed ?? 0} owed left`);
  return released;
};

/** Whether plugin `id` is unused (never approved, nothing pending). */
export const isUnused = async (db: D1Database, id: string): Promise<boolean> =>
  (await db
    .prepare("SELECT 1 AS used FROM versions WHERE plugin_id = ? AND status IN ('pending', 'approved', 'yanked', 'removed') LIMIT 1")
    .bind(id)
    .first()) === null;
