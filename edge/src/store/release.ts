/**
 * Unused ids come back (contract amendment 30). An id with no version at
 * all is released 90 days after its claim: its plugin row (and so its id
 * and lookalike skeleton) goes. Since a publish lists its version at once
 * (amendment 45) and nothing removes a version's record, that is an id
 * claimed and never published to; an admin releases one at once through
 * the reservations routes (`releaseId` with no wait). What stays: the
 * audit log (a `plugin.release` entry), and the claim in `id_claims` (which
 * counts towards the claimer's weekly limit as before). The sweep never
 * releases an id an admin reserved for an account.
 *
 * Its R2 objects, if any were ever owed, go after the rows (`object_purges`,
 * recorded in the same transaction; every sweep finishes what any release
 * still owes before it releases anything new). The id leaves its owner's
 * deploy tokens' lists (best effort: their scopes hold it no more in any case,
 * user-directory.ts `scopeHoldsPlugin`).
 */
import type { Env } from "../env";
import { TAKEDOWN_DAYS } from "./catalog";
import { FILES_KEY, PACKAGE_KEY } from "./db";
import { dropFromDeployTokens } from "./principal";

export const UNUSED_ID_DAYS = 90;
const DAY = 86_400_000;
/** Ids released, and owed objects deleted, per sweep at most. */
const SWEEP = 100;
const PURGE_BATCH = 500;

/** SQL true for plugin `?1` when it is unused and its claim is older than
 * `?2`, and (`?3` = 1) no admin reserved it for an account. */
const RELEASABLE = `NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?1)
  AND (SELECT created_at FROM plugins WHERE id = ?1) < ?2
  AND (?3 = 0 OR NOT EXISTS (SELECT 1 FROM reservations WHERE id = ?1 AND user_id IS NOT NULL))`;

/**
 * The rows of a release, in one batch: what it owes recorded, then the rows
 * gone, each statement guarded, so a publish landing meanwhile keeps the
 * id. Its owner when it was released. `honourReservations`: the sweep's (an
 * admin's own release doesn't).
 */
export const releaseRows = async (db: D1Database, id: string, before: number, actor: string, honourReservations: boolean): Promise<string | undefined> => {
  const now = Date.now();
  const cutoff = Number.isFinite(before) ? before : Number.MAX_SAFE_INTEGER;
  const reserved = honourReservations ? 1 : 0;
  const owner = await db.prepare("SELECT owner_id FROM plugins WHERE id = ?").bind(id).first<string>("owner_id");
  if (!owner) return undefined;
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
    db.prepare(`DELETE FROM version_files WHERE plugin_id = ?1 AND ${RELEASABLE}`).bind(id, cutoff, reserved),
    db.prepare(`DELETE FROM plugins WHERE id = ?1 AND ${RELEASABLE}`).bind(id, cutoff, reserved),
    db
      .prepare("INSERT INTO audit (at, actor_id, action, plugin_id, version, detail) SELECT ?, ?, 'plugin.release', ?, NULL, NULL WHERE changes() > 0")
      .bind(now, actor, id)
  ]);
  return (results[3]?.meta.changes ?? 0) > 0 ? owner : undefined;
};

/** Finish what releases still owe: delete their R2 objects, clearing each
 * record once done. Answers how many were deleted. */
export const finishReleases = async (db: D1Database, bucket: R2Bucket): Promise<number> => {
  let done = 0;
  for (;;) {
    const keys = (await db.prepare("SELECT key FROM object_purges ORDER BY at LIMIT ?").bind(PURGE_BATCH).all<{ key: string }>()).results.map((r) => r.key);
    if (keys.length === 0) break;
    await bucket.delete(keys);
    await db.prepare("DELETE FROM object_purges WHERE key IN (SELECT value FROM json_each(?))").bind(JSON.stringify(keys)).run();
    done += keys.length;
    if (keys.length < PURGE_BATCH) break;
  }
  return done;
};

/** After a release: the id leaves its former owner's deploy tokens' lists. */
const forgetInKeys = async (env: Env, owner: string, id: string): Promise<void> => {
  await dropFromDeployTokens(env, owner, id).catch((e) => console.error("store: couldn't drop a released id from deploy tokens", id, String(e)));
};

/** Release `id` now if it is unused (an admin's: reservations aside). */
export const releaseId = async (env: Env, db: D1Database, bucket: R2Bucket, id: string, before: number, actor: string): Promise<boolean> => {
  const owner = await releaseRows(db, id, before, actor, false);
  if (!owner) return false;
  await finishReleases(db, bucket);
  await forgetInKeys(env, owner, id);
  return true;
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
  await finishReleases(db, bucket);
  const orphans = await collectOrphans(db, bucket, now);
  await pruneTakedowns(db, now);
  const before = now - UNUSED_ID_DAYS * DAY;
  const stale = await db
    .prepare(
      `SELECT p.id FROM plugins p
       WHERE NOT EXISTS (SELECT 1 FROM versions v WHERE v.plugin_id = p.id)
         AND p.created_at < ?
         AND NOT EXISTS (SELECT 1 FROM reservations r WHERE r.id = p.id AND r.user_id IS NOT NULL)
       LIMIT ?`
    )
    .bind(before, SWEEP)
    .all<{ id: string }>();
  let released = 0;
  for (const { id } of stale.results) {
    const owner = await releaseRows(db, id, before, "system", true);
    if (!owner) continue;
    released++;
    await forgetInKeys(env, owner, id);
  }
  if (released) await finishReleases(db, bucket);
  // One line an hour, so an operator sees it ran (`wrangler tail`).
  const owed = await db
    .prepare("SELECT COUNT(*) AS n FROM object_purges")
    .first<number>("n");
  console.log(`store: sweep: released ${released} unused ids, deleted ${orphans} orphaned objects, ${owed ?? 0} owed left`);
  return released;
};

/** The take-down feed keeps `TAKEDOWN_DAYS` (catalog.ts): older events go,
 * oldest first, the newest always kept so its head never goes back. A
 * device behind what's left is told to start afresh (`reset`). */
export const pruneTakedowns = async (db: D1Database, now: number): Promise<number> =>
  (
    await db
      .prepare("DELETE FROM takedown_events WHERE at < ? AND seq < (SELECT MAX(seq) FROM takedown_events)")
      .bind(now - TAKEDOWN_DAYS * DAY)
      .run()
  ).meta.changes ?? 0;

/** Whether plugin `id` is unused: no version was ever published to it. */
export const isUnused = async (db: D1Database, id: string): Promise<boolean> =>
  (await db.prepare("SELECT 1 AS used FROM versions WHERE plugin_id = ? LIMIT 1").bind(id).first()) === null;
