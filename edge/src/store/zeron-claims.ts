/**
 * A listing that says the plugin is Zeron's own (`claims_zeron`, contract
 * 44–45) is only ever its publisher's to say while that account carries
 * Zeron's own badge (`maintainer`). Publishing checks it (publish.ts); so do
 * the two ways a claim could outlive the badge: a staff transfer to another
 * account (admin.ts), and an operator revoking the badge (user-directory.ts).
 */
import { claimsZeron } from "./listing";

/** The badge Zeron's own publishers carry: granted by an operator at the
 * edge only; nothing a publisher sends sets it. */
export const ZERON_BADGE = "maintainer";

/** Which of the plugins `ownerIds` own have a listed or yanked version
 * whose name or summary says it is Zeron's own. */
export const pluginsClaimingZeron = async (db: D1Database, ownerIds: readonly string[], pluginId?: string): Promise<string[]> => {
  if (ownerIds.length === 0) return [];
  const rows = (
    await db
      .prepare(
        `SELECT DISTINCT v.plugin_id, v.name, v.summary FROM versions v JOIN plugins p ON p.id = v.plugin_id
         WHERE p.owner_id IN (SELECT value FROM json_each(?1)) AND (?2 IS NULL OR p.id = ?2) AND v.status IN ('listed', 'yanked')`
      )
      .bind(JSON.stringify(ownerIds), pluginId ?? null)
      .all<{ plugin_id: string; name: string; summary: string }>()
  ).results;
  return [...new Set(rows.filter((r) => claimsZeron(r.name) !== undefined || claimsZeron(r.summary) !== undefined).map((r) => r.plugin_id))].sort();
};
