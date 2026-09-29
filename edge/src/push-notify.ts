/**
 * When a session status change deserves a phone notification — a port of the
 * desktop's rule (crates/ui/src/sound.rs `SessionNotificationState::sound_since`,
 * with `effective_indicator` from crates/proto/src/view.rs), applied to the
 * registry `sessions` rows RegistryRoom sees before and after each push. Pure;
 * unit tested in push-notify.test.ts against the desktop's own cases.
 */
import type { FieldValue, Row } from "./registry-core";

/** A Working/AwaitingInput row older than this is dead (view.rs SESSION_STALE_MS). */
export const SESSION_STALE_MS = 45_000;

export type Indicator = "none" | "working" | "awaiting" | "errored";

/** Which notification — also the per-device preference key. */
export type Category = "done" | "input" | "failed";

const num = (v: FieldValue | undefined): number | undefined => (typeof v === "number" ? v : undefined);
const str = (v: FieldValue | undefined): string | undefined => (typeof v === "string" ? v : undefined);

/** Staleness-checked indicator of a `sessions` row. */
export const indicatorOf = (fields: Row["fields"], now: number): Indicator => {
  switch (fields.status) {
    case "errored":
      return "errored";
    case "working":
    case "awaitingInput": {
      const updatedAt = num(fields.updatedAt) ?? 0;
      if (now - updatedAt > SESSION_STALE_MS) return "none";
      return fields.status === "working" ? "working" : "awaiting";
    }
    default:
      return "none";
  }
};

/**
 * The notification a `sessions` row change calls for, if any:
 * - failed: the indicator becomes Errored;
 * - input: it becomes AwaitingInput (a question / permission prompt);
 * - done: a new completed turn, reported while fresh.
 * A row appearing for the first time (or revived) only sets the baseline, so
 * re-seeds and first syncs never replay old events. Working → Idle without a
 * completion (interrupt, expiry) is silent.
 */
export const notificationFor = (before: Row | undefined, after: Row, now: number): Category | null => {
  if (before === undefined || before.deleted || after.deleted) return null;
  const prev = indicatorOf(before.fields, now);
  const next = indicatorOf(after.fields, now);
  if (next === "errored" && prev !== "errored") return "failed";
  if (next === "awaiting" && prev !== "awaiting") return "input";
  const turn = str(after.fields.lastCompletedTurn);
  const fresh = now - (num(after.fields.updatedAt) ?? 0) <= SESSION_STALE_MS;
  if (fresh && turn !== undefined && turn !== str(before.fields.lastCompletedTurn)) return "done";
  return null;
};

/** The desktop banner's words. */
export const bodyFor = (category: Category): string =>
  category === "done" ? "Run finished" : category === "input" ? "Waiting on your input" : "Run failed";

/** Title from the chat row; side chats and archived chats never notify. */
export const chatForNotification = (chat: Row | undefined): { title: string } | null => {
  if (chat?.deleted) return null;
  if (str(chat?.fields.parentChatId) !== undefined) return null;
  if (chat?.fields.archived === true) return null;
  const title = str(chat?.fields.title)?.trim();
  return { title: title ? title : "New session" };
};

/** Per-device choices (all on by default, like the desktop). */
export interface PushPrefs {
  done: boolean;
  input: boolean;
  failed: boolean;
}

export const DEFAULT_PREFS: PushPrefs = { done: true, input: true, failed: true };

export const parsePrefs = (raw: unknown): PushPrefs => {
  const p = (raw ?? {}) as Partial<Record<keyof PushPrefs, unknown>>;
  return {
    done: p.done !== false,
    input: p.input !== false,
    failed: p.failed !== false
  };
};

/** The APNs body: alert + the chat to open on tap. One thread per chat, and a
 * newer notification for the same chat replaces the older one. */
export const apnsPayload = (chatId: string, title: string, category: Category) => ({
  aps: {
    alert: { title, body: bodyFor(category) },
    sound: "default",
    "thread-id": chatId
  },
  chatId,
  category
});
