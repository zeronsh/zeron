/**
 * A listing that says the plugin is Zeron's own (`claims_zeron`, contract
 * 44–45, "Claims of being Zeron's, after the identity lane's attack") is
 * only ever its publisher's to say while that account carries Zeron's own
 * badge (`maintainer`). Publishing checks it (publish.ts); so do the two ways
 * a claim could outlive the badge:
 *
 * - a staff transfer to an account without the badge is refused 409
 *   `claims_zeron` until the plugin is renamed (admin.ts);
 * - an operator revoking the badge suspends each of that account's plugins
 *   that would now be refused, names them in the answer and the audit, and
 *   leaves their installed copies running (user-directory.ts). Granting the
 *   badge back doesn't relist them: a staff restore does, once the plugin
 *   is moved to an account with the badge or the versions that say so are
 *   taken down (a suspended plugin takes no new version, so its owner
 *   can't rename it); until then a restore is refused 409 `claims_zeron`.
 *
 * A plugin says what its listed versions, and the versions its author may
 * unyank, say. One a newer version superseded comes back only by a staff
 * decision, so it doesn't count.
 */
import { listingClaimsZeron } from "./listing";
import { purgeCatalog } from "./purge";

/** The badge Zeron's own publishers carry: granted by an operator at the
 * edge only; nothing a publisher sends sets it. */
export const ZERON_BADGE = "maintainer";

/** Which of the plugins `ownerIds` own (or just `pluginId`) would be
 * refused `claims_zeron` for a publisher without the badge. */
export const pluginsClaimingZeron = async (db: D1Database, ownerIds: readonly string[], pluginId?: string): Promise<string[]> => {
  if (ownerIds.length === 0) return [];
  const rows = (
    await db
      .prepare(
        `SELECT DISTINCT v.plugin_id, v.name, v.summary FROM versions v JOIN plugins p ON p.id = v.plugin_id
         WHERE p.owner_id IN (SELECT value FROM json_each(?1)) AND (?2 IS NULL OR p.id = ?2)
           AND (v.status = 'listed' OR (v.status = 'yanked' AND v.yanked_by = 'author'))`
      )
      .bind(JSON.stringify(ownerIds), pluginId ?? null)
      .all<{ plugin_id: string; name: string; summary: string }>()
  ).results;
  return [...new Set(rows.filter((r) => listingClaimsZeron(r.name, r.summary)).map((r) => r.plugin_id))].sort();
};

const SUSPEND_REASON = "Its listing says it's Zeron's own, and its account no longer carries Zeron's badge.";

/** Suspend each active plugin of `ownerIds` that would now be refused
 * `claims_zeron` (their badge was revoked), with an audit row each by
 * `actor`: the ids suspended. */
export const suspendZeronClaims = async (db: D1Database, ownerIds: readonly string[], actor: string): Promise<string[]> => {
  const ids = await pluginsClaimingZeron(db, ownerIds);
  if (ids.length === 0) return [];
  const now = Date.now();
  const suspended: string[] = [];
  for (const id of ids) {
    const [changed] = await db.batch([
      db.prepare("UPDATE plugins SET status = 'suspended', suspend_reason = ? WHERE id = ? AND status = 'active'").bind(SUSPEND_REASON, id),
      db
        .prepare("INSERT INTO audit (at, actor_id, action, plugin_id, version, detail) SELECT ?, ?, 'plugin.suspend', ?, NULL, ? WHERE changes() > 0")
        .bind(now, actor, id, JSON.stringify({ reason: SUSPEND_REASON, claims_zeron: true }))
    ]);
    if ((changed?.meta.changes ?? 0) > 0) suspended.push(id);
  }
  // Out of the lists here at once (a suspended plugin is in none).
  await Promise.all(suspended.map((id) => purgeCatalog(db, id)));
  return suspended;
};
