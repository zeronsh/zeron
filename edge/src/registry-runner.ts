/**
 * What a Cloud runner may write to and read from its user's registry
 * (docs/design/cloud-device.md, "Trust boundary").
 *
 * A Cloud chat works with the person's other devices the way a chat on any
 * of their computers does: its runner SEES every device, chat, project and
 * session row (so its agent can list and message chats anywhere), but it
 * still WRITES as its device. It may write the rows it hosts, start a chat
 * on another device (a new row hosted there), and rename or archive any
 * chat — never re-home a chat, delete another device's (its delegates
 * aside), or touch the user's own kinds (preferences, sidebar pins). Its
 * delegates (child chats it started on other machines) may also be deleted.
 * Ownership is the row's `fields.deviceId` (the host). Pure (no storage),
 * separate from registry-core.ts, which mirrors the Rust merge code 1:1 —
 * this is server-only policy.
 */
import { applyOp, type Op, type Row } from "./registry-core";

/** Kinds whose rows name their host device in `fields.deviceId`
 * (crates/doc/src/registry.rs: spaces, chats, sessions). */
const HOSTED_KINDS = new Set(["chats", "spaces", "sessions"]);

/** The fields a runner may write on a chat hosted elsewhere: its label and
 * whether it is archived (what an agent's tools do to any chat). */
const ANY_CHAT_FIELDS = new Set(["title", "archived"]);

/** No delegates. */
const NO_CHILDREN: ReadonlySet<string> = new Set();

/** The host device of a live row, if it names one. */
const hostOf = (row: Row | undefined): string | undefined => {
  if (!row || row.deleted) return undefined;
  const host = row.fields.deviceId;
  return typeof host === "string" ? host : undefined;
};

/**
 * Why runner `device` may not apply `op` to the stored row `before`, or null
 * when it may. Rules:
 * - `devices`: only its own device row;
 * - a chat hosted elsewhere may be renamed or archived; a delegate (in
 *   `children`) may also be deleted, with its session row — its host and
 *   parent stay as written;
 * - a new chat may be hosted by any device (a chat it starts on another of
 *   the person's devices); every other new or existing hosted row must be
 *   hosted by the runner before and after the op (no claiming another
 *   device's row, no handing one away, no ownerless rows);
 * - deletes need a live row the runner hosts; a runner never revives a
 *   tombstone (its fields — and so its former host — are gone);
 * - every other kind (preferences, sidebar pins, anything new) is the
 *   user's, not a device's: refused.
 */
export const runnerOpRefusal = (
  device: string,
  before: Row | undefined,
  op: Op,
  children: ReadonlySet<string> = NO_CHILDREN
): string | null => {
  if (op.kind === "devices") return op.id === device ? null : "not this runner's device row";
  if (!HOSTED_KINDS.has(op.kind)) return `kind ${op.kind} is not writable by a runner`;
  const live = before !== undefined && !before.deleted ? before : undefined;
  if (live && hostOf(live) !== device) {
    // Deleting a chat cascades to its session row (same id).
    if (op.op === "delete") {
      return children.has(op.id) && (op.kind === "chats" || op.kind === "sessions") ? null : "hosted by another device";
    }
    if (op.kind !== "chats") return "hosted by another device";
    const keys = [...Object.keys(op.set ?? {}), ...Object.keys(op.clocks ?? {})];
    return keys.every((key) => ANY_CHAT_FIELDS.has(key)) ? null : "a chat on another device may only be renamed or archived";
  }
  if (op.op === "delete") return live ? null : "no row of this runner's to delete";
  if (before?.deleted) return "a runner cannot revive a deleted row";
  const { row } = applyOp(before, op);
  if (!row || row.deleted) return null;
  const host = hostOf(row);
  // A chat it starts on another device: any host, never none.
  if (before === undefined && op.kind === "chats" && host !== undefined) return null;
  if (host !== device) return "the row would not be hosted by this runner";
  return null;
};

/**
 * Whether a runner may see `row`: every device, chat, project and session
 * row (live or tombstoned), and none of the user's own kinds.
 */
export const runnerCanSee = (_device: string, row: Row): boolean => row.kind === "devices" || HOSTED_KINDS.has(row.kind);

/** The chat's host device, for the registry's `/chat-host`. */
export const chatHost = (row: Row | undefined): string | undefined =>
  row?.kind === "chats" ? hostOf(row) : undefined;
