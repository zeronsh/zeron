/**
 * Unused ids come back (contract amendment 30). An id that has never had an
 * approved version and has no pending one is released 90 days after its
 * last submission: its plugin row (and so its id and lookalike skeleton),
 * its versions' records and file rows, and its packages and files in R2 go;
 * its deploy keys are revoked. What stays: the audit log (a
 * `plugin.release` entry naming every version released, so the history of
 * what was submitted and decided survives), and the claim in `id_claims`
 * (which counts towards the claimer's weekly limit as before).
 *
 * "Never approved" is "no version approved, yanked or removed": only an
 * approved version becomes either. The hourly cron sweeps
 * (`releaseUnusedIds`); an admin releases one at once through the
 * reservations routes (`releaseId` with no wait).
 */
import type { Env } from "../env";
import { revokeDeployKeys } from "./principal";

export const UNUSED_ID_DAYS = 90;
const DAY = 86_400_000;
/** Ids released per sweep, at most. */
const SWEEP = 100;

/** SQL true for plugin `?1` when it is unused and its last submission (or
 * its claim, if it has none) is older than `?2`. */
const UNUSED = `NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?1 AND status IN ('pending', 'approved', 'yanked', 'removed'))
  AND COALESCE((SELECT MAX(submitted_at) FROM versions WHERE plugin_id = ?1), (SELECT created_at FROM plugins WHERE id = ?1)) < ?2`;

/**
 * Release `id` if it is unused and quiet since `before` (ms; `Infinity`:
 * any unused id, for an admin). True when it was released. One D1 batch,
 * each statement guarded, so a submission landing meanwhile keeps it.
 */
export const releaseId = async (env: Env, db: D1Database, bucket: R2Bucket, id: string, before: number, actor: string): Promise<boolean> => {
  const now = Date.now();
  const cutoff = Number.isFinite(before) ? before : Number.MAX_SAFE_INTEGER;
  const versions = (await db.prepare("SELECT version, status FROM versions WHERE plugin_id = ?").bind(id).all<{ version: string; status: string }>()).results;
  const results = await db.batch([
    db.prepare(`DELETE FROM version_files WHERE plugin_id = ?1 AND ${UNUSED}`).bind(id, cutoff),
    db.prepare(`DELETE FROM versions WHERE plugin_id = ?1 AND ${UNUSED}`).bind(id, cutoff),
    db.prepare(`DELETE FROM plugins WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?1) AND created_at < ?2`).bind(id, cutoff),
    db
      .prepare("INSERT INTO audit (at, actor_id, action, plugin_id, version, detail) SELECT ?, ?, 'plugin.release', ?, NULL, ? WHERE changes() > 0")
      .bind(now, actor, id, JSON.stringify({ versions }))
  ]);
  if ((results[2]?.meta.changes ?? 0) === 0) return false;
  for (const prefix of [`pkg/${id}/`, `files/${id}/`]) {
    let cursor: string | undefined;
    do {
      const page = await bucket.list({ prefix, cursor });
      if (page.objects.length) await bucket.delete(page.objects.map((o) => o.key));
      cursor = page.truncated ? page.cursor : undefined;
    } while (cursor);
  }
  await revokeDeployKeys(env, id);
  return true;
};

/** The hourly sweep: release ids unused for `UNUSED_ID_DAYS`. */
export const releaseUnusedIds = async (env: Env, now: number): Promise<number> => {
  const db = env.STORE_DB;
  const bucket = env.PLUGIN_STORE;
  if (!db || !bucket) return 0;
  const before = now - UNUSED_ID_DAYS * DAY;
  const stale = await db
    .prepare(
      `SELECT p.id FROM plugins p
       WHERE NOT EXISTS (SELECT 1 FROM versions v WHERE v.plugin_id = p.id AND v.status IN ('pending', 'approved', 'yanked', 'removed'))
         AND COALESCE((SELECT MAX(submitted_at) FROM versions v WHERE v.plugin_id = p.id), p.created_at) < ?
       LIMIT ?`
    )
    .bind(before, SWEEP)
    .all<{ id: string }>();
  let released = 0;
  for (const { id } of stale.results) if (await releaseId(env, db, bucket, id, before, "system")) released++;
  return released;
};

/** Whether plugin `id` is unused (never approved, nothing pending). */
export const isUnused = async (db: D1Database, id: string): Promise<boolean> =>
  (await db
    .prepare("SELECT 1 AS used FROM versions WHERE plugin_id = ? AND status IN ('pending', 'approved', 'yanked', 'removed') LIMIT 1")
    .bind(id)
    .first()) === null;
