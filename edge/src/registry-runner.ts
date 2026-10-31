/**
 * What a Cloud runner may write to and read from its user's registry
 * (docs/design/cloud-device.md, "Trust boundary").
 *
 * The Cloud sandbox runs full-access agents on arbitrary repository code, so
 * its runner key must be treated as potentially stolen. A runner therefore
 * acts as ITS DEVICE, not as the whole user: it sees every `devices` row
 * (names, presence), the chats/spaces/sessions it hosts, and the projects
 * (spaces) those chats belong to — a Cloud chat's project lives on the
 * user's computer; it can write only rows it hosts. Ownership is the row's `fields.deviceId` (the
 * host). One exception, delegation: the chat a runner hosts may have child
 * chats on other machines (Cloud sessions it started). The runner sees those
 * children's chat and session rows, and may rename, archive or delete the
 * children — never re-home them, and never see any other main chat. Pure (no storage), separate from registry-core.ts, which mirrors
 * the Rust merge code 1:1 — this is server-only policy.
 */
import { applyOp, type Op, type Row } from "./registry-core";

/** Kinds whose rows name their host device in `fields.deviceId`
 * (crates/doc/src/registry.rs: spaces, chats, sessions). */
const HOSTED_KINDS = new Set(["chats", "spaces", "sessions"]);

/** The fields a runner may write on a child chat (hosted elsewhere): its
 * label and whether it is archived. */
const CHILD_FIELDS = new Set(["title", "archived"]);

/** What a runner reaches beyond the rows it hosts. */
export interface RunnerScope {
  /** Projects of the chats it hosts. */
  readonly spaces: ReadonlySet<string>;
  /** Live chats whose `parentChatId` is a chat it hosts (its delegates). */
  readonly children: ReadonlySet<string>;
}

/** No reach beyond hosted rows. */
export const NO_SCOPE: RunnerScope = { spaces: new Set(), children: new Set() };

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
 * - hosted kinds: a live row must already be hosted by the runner, and the
 *   row as it would be after the op must still be (no claiming another
 *   device's chat, no handing one away, no ownerless rows);
 * - deletes need a live row the runner hosts; a runner never revives a
 *   tombstone (its fields — and so its former host — are gone);
 * - a child chat (in `children`) may be renamed, archived or deleted, and
 *   nothing else — its host and parent stay as the edge wrote them;
 * - every other kind (preferences, sidebar pins, anything new) is the
 *   user's, not a device's: refused.
 */
export const runnerOpRefusal = (
  device: string,
  before: Row | undefined,
  op: Op,
  children: ReadonlySet<string> = NO_SCOPE.children
): string | null => {
  if (op.kind === "devices") return op.id === device ? null : "not this runner's device row";
  if (!HOSTED_KINDS.has(op.kind)) return `kind ${op.kind} is not writable by a runner`;
  const live = before !== undefined && !before.deleted ? before : undefined;
  if (live && hostOf(live) !== device) {
    if (children.has(op.id) && (op.kind === "chats" || (op.kind === "sessions" && op.op === "delete"))) {
      // Deleting a chat cascades to its session row (same id).
      if (op.op === "delete") return null;
      const keys = [...Object.keys(op.set ?? {}), ...Object.keys(op.clocks ?? {})];
      return keys.every((key) => CHILD_FIELDS.has(key)) ? null : "a parent may only rename, archive or delete its child chats";
    }
    return "hosted by another device";
  }
  if (op.op === "delete") return live ? null : "no row of this runner's to delete";
  if (before?.deleted) return "a runner cannot revive a deleted row";
  const { row } = applyOp(before, op);
  if (row && !row.deleted && hostOf(row) !== device) return "the row would not be hosted by this runner";
  return null;
};

/**
 * Whether a runner may see `row`: all device rows, the hosted rows it owns,
 * the projects of the chats it hosts (`scope.spaces`), its child chats and
 * their session rows (`scope.children`; a session row's id is its chat's),
 * and tombstones of hosted kinds (they carry no fields — only the id and
 * delete clock — and the runner must learn when its own rows die).
 */
export const runnerCanSee = (device: string, row: Row, scope: RunnerScope = NO_SCOPE): boolean => {
  if (row.kind === "devices") return true;
  if (!HOSTED_KINDS.has(row.kind)) return false;
  if (row.deleted) return true;
  if (hostOf(row) === device) return true;
  if (row.kind === "spaces") return scope.spaces.has(row.id);
  return scope.children.has(row.id);
};

/** The chat's parent chat id, if it records one. */
export const parentOf = (row: Row | undefined): string | undefined => {
  if (!row || row.deleted || row.kind !== "chats") return undefined;
  const parent = row.fields.parentChatId;
  return typeof parent === "string" && parent !== "" ? parent : undefined;
};

/** The chat's host device, for the Worker's chat2/blob gate. */
export const chatHost = (row: Row | undefined): string | undefined =>
  row?.kind === "chats" ? hostOf(row) : undefined;
