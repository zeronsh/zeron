/**
 * Draft-room protocol constants and pure helpers (docs/draft-sync.md §1).
 * Kept free of Durable Object imports so the Worker route, the DO and the
 * plain-Node unit tier all share one definition.
 */

/** Per-row cap. A draft row is one composer edit (or one small snapshot);
 * chat2's 1 MiB cap would let an abandoned draft grow without bound. */
export const MAX_ROW_BYTES = 64 * 1024;
/** Checkpoint upload cap (413 above). */
export const MAX_CHECKPOINT_BYTES = 256 * 1024;
/** A room untouched by any accepted write (push, checkpoint, discard) for
 * this long is deleted outright by its alarm. */
export const DRAFT_IDLE_TTL_MS = 30 * 24 * 60 * 60 * 1000;
/** WebSocket close code: the draft was discarded (or the socket's epoch is
 * stale). Clients re-read `/epoch` before redialing. */
export const CLOSE_DRAFT_DISCARDED = 4411;
export const CLOSE_DRAFT_DISCARDED_REASON = "draft discarded";
/** The first epoch of a room. */
export const INITIAL_EPOCH = 1;

/** `?epoch=` value: a positive base-10 integer, no sign, no leading zeros.
 * Anything else is `undefined` (callers treat that as "no valid epoch"). */
export const parseEpoch = (raw: string | null | undefined): number | undefined => {
  if (raw === null || raw === undefined || !/^[1-9][0-9]{0,15}$/.test(raw)) return undefined;
  const n = Number(raw);
  return Number.isSafeInteger(n) ? n : undefined;
};

/** True once `now` is at least a full TTL past the last accepted write. A room
 * with no recorded write (`lastWriteAt <= 0`) counts as idle. */
export const isIdleExpired = (
  lastWriteAt: number,
  now: number,
  ttlMs: number = DRAFT_IDLE_TTL_MS
): boolean => !(lastWriteAt > 0) || now - lastWriteAt >= ttlMs;

/** Durable Object name: the user id comes from the verified JWT, never the
 * URL, so a user can only ever address their own rooms. */
export const draftRoomName = (orgId: string, userId: string, chatId: string): string =>
  `draft1/${orgId}/${userId}/${chatId}`;

export const DRAFT_ID_RE = /^[A-Za-z0-9_-]{1,128}$/;

/** Sub-routes and their allowed methods (chat2-style allow-list). */
export const DRAFT_ROUTES: Record<string, readonly string[]> = {
  checkpoint: ["GET", "POST"],
  rows: ["GET", "POST"],
  epoch: ["GET"],
  discard: ["POST"],
  stats: ["GET"]
};

export type DraftPath =
  | { kind: "ok"; orgId: string; chatId: string; action: string }
  | { kind: "notDraft" }
  | { kind: "notFound" };

/** Split `/draft/:orgId/:chatId/:action`. Does not validate ids (the route
 * orders the org check before the chat-id check); `notDraft` = not ours. */
export const parseDraftPath = (pathname: string): DraftPath => {
  const parts = pathname.split("/").filter(Boolean);
  if (parts[0] !== "draft") return { kind: "notDraft" };
  if (parts.length !== 4 || !parts[1] || !parts[2] || !parts[3]) return { kind: "notFound" };
  return { kind: "ok", orgId: parts[1], chatId: parts[2], action: parts[3] };
};
