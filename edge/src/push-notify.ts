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

/** Staleness-checked running-subagent count of a `sessions` row (view.rs
 * `running_subagents`): a dead engine's row counts nothing. */
export const runningSubagentsOf = (fields: Row["fields"], now: number): number =>
  now - (num(fields.updatedAt) ?? 0) > SESSION_STALE_MS ? 0 : (num(fields.runningSubagents) ?? 0);

/** A row change's notification, and the completed turn whose "done" now
 * waits on its subagents (the caller stores it for the chat's next change). */
export interface Decision {
  category: Category | null;
  held: string | undefined;
}

/**
 * The notification a `sessions` row change calls for, if any:
 * - failed: the indicator becomes Errored;
 * - input: it becomes AwaitingInput (a question / permission prompt);
 * - done: a new completed turn, reported while fresh.
 * A row appearing for the first time (or revived) only sets the baseline, so
 * re-seeds and first syncs never replay old events. Working → Idle without a
 * completion (interrupt, expiry) is silent.
 *
 * A finished turn whose subagents still run is held (`held`, the previous
 * change's hold, comes back in): "done" rings once they drain on that same
 * turn, and never if a newer turn or a stale row took over first. Port of
 * the engine's `SessionNotificationState::decide`
 * (crates/engine/src/desktop_notify.rs) as its notifier calls it, with no
 * pending send: session rows don't carry one.
 */
export const decideNotification = (
  before: Row | undefined,
  after: Row,
  now: number,
  held: string | undefined
): Decision => {
  const quiet: Decision = { category: null, held: undefined };
  if (before === undefined || before.deleted || after.deleted) return quiet;
  const prev = indicatorOf(before.fields, now);
  const next = indicatorOf(after.fields, now);
  if (next === "errored" && prev !== "errored") return { category: "failed", held: undefined };
  if (next === "awaiting" && prev !== "awaiting") return { category: "input", held: undefined };
  const fresh = now - (num(after.fields.updatedAt) ?? 0) <= SESSION_STALE_MS;
  if (!fresh) return quiet;
  // Idle: no newer turn is running, asking, or failing.
  const idle = next === "none";
  const running = runningSubagentsOf(after.fields, now);
  const turn = str(after.fields.lastCompletedTurn);
  if (turn !== undefined && turn !== str(before.fields.lastCompletedTurn)) {
    return idle && running > 0 ? { category: null, held: turn } : { category: "done", held: undefined };
  }
  if (idle && held !== undefined && held === turn) {
    return running > 0 ? { category: null, held } : { category: "done", held: undefined };
  }
  return quiet;
};

/** [`decideNotification`] for a chat with no finish held. */
export const notificationFor = (before: Row | undefined, after: Row, now: number): Category | null =>
  decideNotification(before, after, now, undefined).category;

/** The desktop banner's words. */
export const bodyFor = (category: Category): string =>
  category === "done" ? "Run finished" : category === "input" ? "Waiting on your input" : "Run failed";

/** Title from the chat row; side chats and archived chats never notify, and a
 * snoozed chat (`snoozedUntil` epoch ms still ahead) skips "done" only, since
 * input and failure wake it. */
export const chatForNotification = (
  chat: Row | undefined,
  category?: Category,
  now: number = Date.now()
): { title: string } | null => {
  if (chat?.deleted) return null;
  if (str(chat?.fields.parentChatId) !== undefined) return null;
  if (chat?.fields.archived === true) return null;
  if (category === "done" && (num(chat?.fields.snoozedUntil) ?? 0) > now) return null;
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
