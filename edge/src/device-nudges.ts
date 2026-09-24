/** Durable wake receipts. The opaque token fences ACKs against newer work. */
export const NUDGE_PAGE = 64;
export const NUDGE_CAP = 4096;
export type PendingNudge = { chat_id: string; token: string };

export function ensureNudges(sql: SqlStorage): void {
  sql.exec("CREATE TABLE IF NOT EXISTS pending_nudges (chat_id TEXT PRIMARY KEY, queued_at INTEGER NOT NULL)");
  const columns = [...sql.exec("PRAGMA table_info(pending_nudges)")];
  if (!columns.some((c) => c.name === "token")) {
    sql.exec("ALTER TABLE pending_nudges ADD COLUMN token TEXT NOT NULL DEFAULT ''");
  }
  sql.exec("UPDATE pending_nudges SET token=lower(hex(randomblob(16))) WHERE token=''");
  sql.exec("CREATE TABLE IF NOT EXISTS nudge_reconcile (id INTEGER PRIMARY KEY CHECK(id=1), token TEXT NOT NULL)");
}

export function enqueueNudge(sql: SqlStorage, chatId: string): boolean {
  const exists = [...sql.exec("SELECT 1 FROM pending_nudges WHERE chat_id=?", chatId)].length > 0;
  const count = [...sql.exec("SELECT count(*) AS n FROM pending_nudges")][0].n as number;
  if (!exists && count >= NUDGE_CAP) {
    // Never silently discard an accepted wake. New senders retain their command
    // and retry a 503; a durable reconciliation covers older best-effort senders.
    sql.exec("INSERT INTO nudge_reconcile VALUES (1,?) ON CONFLICT(id) DO UPDATE SET token=excluded.token", crypto.randomUUID());
    return false;
  }
  sql.exec("INSERT INTO pending_nudges(chat_id,queued_at,token) VALUES (?,?,?) ON CONFLICT(chat_id) DO UPDATE SET queued_at=excluded.queued_at,token=excluded.token",
    chatId, Date.now(), crypto.randomUUID());
  return true;
}

export function pendingNudges(sql: SqlStorage): PendingNudge[] {
  const reconcile = [...sql.exec("SELECT '*' AS chat_id,token FROM nudge_reconcile")];
  const rows = [...sql.exec("SELECT chat_id,token FROM pending_nudges ORDER BY queued_at,chat_id LIMIT ?", NUDGE_PAGE * 2)];
  return [...reconcile, ...rows] as PendingNudge[];
}

export function acknowledgeNudge(sql: SqlStorage, chatId: string, token: string): void {
  if (chatId === "*") sql.exec("DELETE FROM nudge_reconcile WHERE id=1 AND token=?", token);
  else sql.exec("DELETE FROM pending_nudges WHERE chat_id=? AND token=?", chatId, token);
}
