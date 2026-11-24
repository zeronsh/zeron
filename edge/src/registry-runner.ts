/**
 * What a Cloud runner may write to and read from its user's registry
 * (docs/design/cloud-device.md, "Trust boundary"), by its session's reach,
 * which only the person sets (CloudAccount, `RegistryRoom.setRunnerReach`):
 *
 * - `devices` (the default): a Cloud chat works with the person's other
 *   devices the way a chat on any of their computers does. Its runner SEES
 *   every device, chat, project and session row (so its agent can list and
 *   message chats anywhere), and writes chats as any of their devices may —
 *   start one on another device, set any field of one hosted elsewhere (its
 *   title, branch, config, …) — but it never re-homes a chat, deletes
 *   another device's (its delegates aside), writes another device's project
 *   or session rows (their host's to keep), or touches the user's own kinds
 *   (preferences, sidebar pins).
 * - Both reaches READ the account's plugin switches (`pluginSwitches`, one
 *   row per plugin: is it on?), which every engine of the account follows,
 *   Cloud ones included; like the user's other kinds, never written.
 * - `isolated`: the runner acts as its device alone. It sees every device
 *   row (names, presence), the rows it hosts, the projects of the chats it
 *   hosts, and its delegates (child chats it started on other machines) with
 *   their session rows; it writes only rows it hosts, and may rename,
 *   archive or delete its delegates — never re-home them, and never see or
 *   touch any other chat.
 *
 * Ownership is the row's `fields.deviceId` (the host). Pure (no storage),
 * separate from registry-core.ts, which mirrors the Rust merge code 1:1 —
 * this is server-only policy.
 */
import { applyOp, type Op, type Row } from "./registry-core";

/** How far a Cloud session reaches beyond its own machine. */
export type RunnerReach = "devices" | "isolated";

export const isRunnerReach = (value: unknown): value is RunnerReach => value === "devices" || value === "isolated";

/** Kinds whose rows name their host device in `fields.deviceId`
 * (crates/doc/src/registry.rs: spaces, chats, sessions). */
const HOSTED_KINDS = new Set(["chats", "spaces", "sessions"]);

/** The user's kinds a runner reads (never writes): account-wide settings
 * its engine must follow (crates/doc/src/registry/plugin_switches.rs). */
const ACCOUNT_READ_KINDS = new Set(["pluginSwitches"]);

/** What a runner may not write on a chat hosted elsewhere (`devices`): its host. */
const HOST_FIELD = "deviceId";

/** The fields an isolated runner may write on its delegates: their label and
 * whether they are archived. */
const CHILD_FIELDS = new Set(["title", "archived"]);

/** What a runner reaches beyond the rows it hosts. */
export interface RunnerScope {
  /** Projects of the chats it hosts (what an isolated runner sees of them). */
  readonly spaces: ReadonlySet<string>;
  /** Live chats whose `parentChatId` is a chat it hosts: its delegates, which
   * it may delete in either reach. */
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
 * Why runner `device`, of a session reaching `reach`, may not apply `op` to
 * the stored row `before`, or null when it may. Both reaches:
 * - `devices`: only its own device row;
 * - every new or existing hosted row it writes as its own must be hosted by
 *   the runner before and after the op (no claiming another device's row,
 *   no handing one away, no ownerless rows);
 * - deletes need a live row the runner hosts, or a delegate (in `children`)
 *   with its session row; a runner never revives a tombstone;
 * - every other kind (preferences, sidebar pins, anything new) is the
 *   user's, not a device's: refused.
 * `devices` also: a chat hosted elsewhere may have any field set but its
 * host, and a new chat may be hosted by any device. `isolated` instead: a
 * delegate may be renamed or archived, and no other chat touched.
 */
export const runnerOpRefusal = (
  device: string,
  before: Row | undefined,
  op: Op,
  reach: RunnerReach,
  children: ReadonlySet<string> = NO_SCOPE.children
): string | null => {
  if (op.kind === "devices") return op.id === device ? null : "not this runner's device row";
  if (!HOSTED_KINDS.has(op.kind)) return `kind ${op.kind} is not writable by a runner`;
  const live = before !== undefined && !before.deleted ? before : undefined;
  if (live && hostOf(live) !== device) {
    const child = children.has(op.id);
    // Deleting a chat cascades to its session row (same id).
    if (op.op === "delete") {
      return child && (op.kind === "chats" || op.kind === "sessions") ? null : "hosted by another device";
    }
    if (op.kind !== "chats") return "hosted by another device";
    const keys = [...Object.keys(op.set ?? {}), ...Object.keys(op.clocks ?? {})];
    if (reach === "isolated") {
      if (!child) return "hosted by another device";
      return keys.every((key) => CHILD_FIELDS.has(key)) ? null : "a parent may only rename, archive or delete its child chats";
    }
    return keys.includes(HOST_FIELD) ? "a chat on another device stays hosted there" : null;
  }
  if (op.op === "delete") return live ? null : "no row of this runner's to delete";
  if (before?.deleted) return "a runner cannot revive a deleted row";
  const { row } = applyOp(before, op);
  if (!row || row.deleted) return null;
  const host = hostOf(row);
  // A chat it starts on another device: any host, never none.
  if (reach === "devices" && before === undefined && op.kind === "chats" && host !== undefined) return null;
  if (host !== device) return "the row would not be hosted by this runner";
  return null;
};

/**
 * Whether runner `device`, of a session reaching `reach`, may see `row`.
 * `devices`: every device, chat, project and session row (live or
 * tombstoned), and none of the user's own kinds but the plugin switches.
 * Both reaches see the plugin switches. `isolated`: every device
 * row, the hosted rows it owns, the projects of the chats it hosts
 * (`scope.spaces`), its delegates and their session rows (`scope.children`;
 * a session row's id is its chat's), and tombstones of hosted kinds (they
 * carry no fields, and the runner must learn when its own rows die).
 */
export const runnerCanSee = (device: string, row: Row, reach: RunnerReach, scope: RunnerScope = NO_SCOPE): boolean => {
  if (row.kind === "devices") return true;
  if (ACCOUNT_READ_KINDS.has(row.kind)) return true;
  if (!HOSTED_KINDS.has(row.kind)) return false;
  if (reach === "devices" || row.deleted) return true;
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

/** The chat's host device, for the registry's `/chat-host`. */
export const chatHost = (row: Row | undefined): string | undefined =>
  row?.kind === "chats" ? hostOf(row) : undefined;
