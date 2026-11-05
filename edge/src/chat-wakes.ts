/** Opaque chat rows and their host-discovery obligation commit together.
 * One coalesced receipt per chat; newer rows fence an in-flight completion. */
export interface ChatWake extends Record<string, SqlStorageValue> {
  chat_id: string;
  host_device: string;
  user_id: string;
  token: string;
  attempts: number;
  next_at: number;
}

/** Chat ids the device room's nudge accepts (1-64): longer ones still push
 * their rows, they just carry no wake. */
export const validWakeRoute = (chat: string, host: string): boolean =>
  /^[A-Za-z0-9_-]{1,64}$/.test(chat) && /^[A-Za-z0-9_-]{1,128}$/.test(host);

/** Device-room answers no retry can change (see device-room.ts /nudge): a
 * chat id it refuses (400) or another owner's device (403). An unclaimed
 * room (404) is a host that has not connected yet — that one waits. */
export const PERMANENT_WAKE_REJECTIONS = new Set([400, 403]);

/** About a day of capped (60s) retries; past it the host's own sync owns it. */
export const MAX_WAKE_ATTEMPTS = 1440;

export function ensureChatWakes(sql: SqlStorage): void {
  sql.exec("CREATE TABLE IF NOT EXISTS chat_wake (id INTEGER PRIMARY KEY CHECK(id=1), chat_id TEXT NOT NULL, host_device TEXT NOT NULL, user_id TEXT NOT NULL, token TEXT NOT NULL, attempts INTEGER NOT NULL, next_at INTEGER NOT NULL)");
}

export function queueChatWake(sql: SqlStorage, chat: string, host: string, user: string): void {
  sql.exec("INSERT INTO chat_wake VALUES (1,?,?,?,?,0,?) ON CONFLICT(id) DO UPDATE SET chat_id=excluded.chat_id,host_device=excluded.host_device,user_id=excluded.user_id,token=excluded.token,attempts=0,next_at=excluded.next_at",
    chat, host, user, crypto.randomUUID(), Date.now() + 1000);
}

export function pendingChatWake(sql: SqlStorage): ChatWake | undefined {
  return [...sql.exec("SELECT * FROM chat_wake WHERE id=1")][0] as ChatWake | undefined;
}

export function finishChatWake(sql: SqlStorage, token: string): void {
  sql.exec("DELETE FROM chat_wake WHERE id=1 AND token=?", token);
}

export function retryChatWake(sql: SqlStorage, wake: ChatWake): void {
  const delay = Math.min(60_000, 1000 * 2 ** Math.min(wake.attempts, 6));
  sql.exec("UPDATE chat_wake SET attempts=attempts+1,next_at=? WHERE id=1 AND token=?",
    Date.now() + delay, wake.token);
}
