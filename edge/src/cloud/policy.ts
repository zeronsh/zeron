/**
 * Pure decisions behind the Cloud device lifecycle (docs/design/cloud-device.md),
 * kept free of Workers APIs so the Node unit tier covers them directly: when
 * an idle device sleeps, when a runner token timestamp is acceptable, and
 * the deterministic workflow ids.
 * Provider-neutral: sandbox states come normalized from sandbox-provider.ts.
 */

import type { SandboxMachine } from "./sandbox-provider";

export type CloudState =
  | "off"
  | "provisioning"
  | "starting"
  | "ready"
  | "sleeping"
  | "stopping"
  | "error"
  | "deleting"
  /** The provider deleted the stopped machine's disk. The transcript is safe
   * in its chat; a fresh machine (`retry`) starts on the same repo and branch. */
  | "data_expired";

/** Per-session lifecycle workflows. */
export type WorkflowKind = "provision" | "wake" | "sleep" | "delete";

/** The CloudAccount DO's name. Keyed by the caller's own (org, user): a user
 * can only ever address their own Cloud account and sessions. */
export const cloudAccountName = (orgId: string, userId: string): string => `cloud1/${orgId}/${userId}`;

/** Cloud device ids (logical and session) are minted by the DO: `cloud-{uuid}`. */
export const CLOUD_DEVICE_PREFIX = "cloud-";

export const newCloudDeviceId = (): string => `${CLOUD_DEVICE_PREFIX}${crypto.randomUUID()}`;

/** Deterministic per (device, generation): a double click or a retried
 * `create` lands on the same instance instead of running two lifecycles. */
export const workflowInstanceId = (kind: WorkflowKind, deviceId: string, generation: number): string =>
  `${kind === "provision" ? "prov" : kind}-${deviceId}-${generation}`;

/** Runner token timestamps: within ±120 s of now and strictly increasing. */
export const TOKEN_WINDOW_MS = 120_000;

export const tokenTimestampCheck = (
  ts: number,
  lastAccepted: number | undefined,
  now: number
): "ok" | "stale" | "replay" => {
  if (!Number.isSafeInteger(ts) || Math.abs(now - ts) > TOKEN_WINDOW_MS) return "stale";
  if (lastAccepted !== undefined && ts <= lastAccepted) return "replay";
  return "ok";
};

/** Runners heartbeat every 60 s; three missed beats = the engine is gone and
 * whatever it last reported (active runs, clients) no longer holds. */
export const HEARTBEAT_STALE_MS = 3 * 60_000;

/** The slowest cadence counted as a fast one. Newer engines beat every 15 s
 * while a turn runs, and say so (`everySecs`). */
export const FAST_BEAT_MAX_MS = 20_000;
/** However fast it beats, at least this long before it counts as gone: a
 * machine busy enough to miss a beat or two isn't dead. */
export const FAST_STALE_MIN_MS = 45_000;

/**
 * How long a machine may go without a heartbeat before it counts as gone:
 * three beats at the cadence it last said it keeps, never under
 * {@link FAST_STALE_MIN_MS}. An engine that never said (an older one), or one
 * beating at the idle pace, gets {@link HEARTBEAT_STALE_MS}. Read with a turn
 * in flight, this is how long a killed turn waits before its machine is
 * brought back to resume it.
 */
export const heartbeatStaleMs = (beatEveryMs: number | undefined): number =>
  beatEveryMs !== undefined && beatEveryMs > 0 && beatEveryMs <= FAST_BEAT_MAX_MS
    ? Math.max(FAST_STALE_MIN_MS, 3 * beatEveryMs)
    : HEARTBEAT_STALE_MS;

export interface IdleInput {
  readonly now: number;
  readonly idleMs: number;
  /** Last time anything kept the device busy (or it became ready). */
  readonly lastActiveAt: number | undefined;
  readonly lastHeartbeatAt: number | undefined;
  readonly activeRuns: number;
  readonly clients: number;
}

export type IdleDecision = { readonly sleep: true } | { readonly sleep: false; readonly checkAt: number };

/**
 * Sleep after `idleMs` with no active runs, no connected clients and no
 * heartbeat change. A stale heartbeat's "busy" report is ignored — a dead
 * engine must not hold a billed sandbox awake forever.
 */
export const idleDecision = (input: IdleInput): IdleDecision => {
  const { now, idleMs } = input;
  const fresh =
    input.lastHeartbeatAt !== undefined && now - input.lastHeartbeatAt < HEARTBEAT_STALE_MS;
  if (fresh && (input.activeRuns > 0 || input.clients > 0)) {
    return { sleep: false, checkAt: now + idleMs };
  }
  const since = input.lastActiveAt ?? 0;
  if (now - since >= idleMs) return { sleep: true };
  return { sleep: false, checkAt: since + idleMs };
};

/** While a TTL'd (trial) sandbox is busy, push its auto-stop out at most
 * this often. */
export const TTL_EXTEND_EVERY_MS = 10 * 60_000;

export const shouldExtendTtl = (input: {
  readonly ttlSeconds: number | null;
  readonly busy: boolean;
  readonly lastExtendedAt: number | undefined;
  readonly now: number;
}): boolean =>
  input.ttlSeconds !== null &&
  input.busy &&
  input.now - (input.lastExtendedAt ?? 0) >= TTL_EXTEND_EVERY_MS;

/** `CLOUD_TTL_SECONDS`: unset/empty/"null" = no auto-stop. */
export const parseTtlSeconds = (raw: string | undefined): number | null => {
  if (raw === undefined || raw.trim() === "" || raw.trim() === "null") return null;
  const n = Number(raw);
  return Number.isInteger(n) && n > 0 ? n : null;
};

/** `CLOUD_MAX_AWAKE`: most sessions awake at once per user (default 5). */
export const parseMaxAwake = (raw: string | undefined): number => {
  const n = Number(raw);
  return Number.isInteger(n) && n > 0 ? n : 5;
};

/** One machine size, as clients show it. */
export interface MachineSpec {
  readonly id: SandboxMachine;
  readonly name: string;
  readonly vcpu: number;
  readonly memoryGB: number;
  /** What a machine of this size "holds": the limit a resume is refused
   * against, in what the machine added. `df` also counts the base image
   * (`SandboxCapabilities.diskOverheadGiB`). */
  readonly diskGiB: number;
  /** Credits it spends relative to `default` (metering's live estimate). */
  readonly creditMultiplier: number;
}

/**
 * Every size, smallest first: the one place this is written down. Boat's table
 * (docs.boat.dev/machines) gives CPU, memory, the multipliers (xlarge is 50/9)
 * and disk rounded down to 51, 51, 111 and 251. `default`'s 51.2 and `large`'s
 * 111.8 are what Boat's own refusals said; `small` is refused at the same
 * figure as `default` on a shrink. It has said no exact figure for `xlarge`.
 */
export const MACHINES: readonly MachineSpec[] = [
  { id: "small", name: "Small", vcpu: 2, memoryGB: 4, diskGiB: 51.2, creditMultiplier: 0.5 },
  { id: "default", name: "Default", vcpu: 4, memoryGB: 8, diskGiB: 51.2, creditMultiplier: 1 },
  { id: "large", name: "Large", vcpu: 8, memoryGB: 16, diskGiB: 111.8, creditMultiplier: 2 },
  { id: "xlarge", name: "Extra large", vcpu: 16, memoryGB: 32, diskGiB: 251, creditMultiplier: 5.56 }
];

export const machineSpec = (id: string | undefined): MachineSpec | undefined => MACHINES.find((m) => m.id === id);

/** A sandbox size by name, or undefined. */
export const machineName = (raw: unknown): SandboxMachine | undefined =>
  typeof raw === "string" ? machineSpec(raw)?.id : undefined;

export type DiskLevel = "ok" | "warn" | "critical";

/** A machine warns before it can no longer fit its size. */
export const DISK_WARN = 0.8;
export const DISK_CRITICAL = 0.9;

export const diskLevel = (usedGiB: number, capacityGiB: number): DiskLevel => {
  const share = usedGiB / capacityGiB;
  return share >= DISK_CRITICAL ? "critical" : share >= DISK_WARN ? "warn" : "ok";
};

/**
 * A sleeping machine is resumed and stopped again this long before its
 * provider would delete its disk: long enough for a busy account, a provider
 * outage or a failed attempt to get another try.
 */
export const REFRESH_MARGIN_MS = 2 * 24 * 60 * 60_000;

/** When a machine stopped at `stoppedAt` is next due for a refresh. */
export const refreshDueAt = (stoppedAt: number, retentionMs: number): number =>
  stoppedAt + retentionMs - REFRESH_MARGIN_MS;

/** `CLOUD_MACHINE`: the session sandbox size (default "default"). */
export const parseMachine = (raw: string | undefined): SandboxMachine => machineName(raw) ?? "default";

/** `CLOUD_TEMPLATE`: a provider snapshot name, or undefined. */
export const parseTemplate = (raw: string | undefined): string | undefined => {
  const name = raw?.trim();
  return name && /^[A-Za-z0-9._-]{1,128}$/.test(name) ? name : undefined;
};

const FIRST_REPORT_GRACE_MS = 60_000;

/** Session states that hold a running (billed) sandbox slot. */
export const AWAKE_STATES: ReadonlySet<CloudState> = new Set(["provisioning", "starting", "ready"]);

export interface AwakeSession {
  readonly chatId: string;
  readonly state: CloudState;
  readonly lastActiveAt?: number;
  readonly lastHeartbeatAt?: number;
  readonly activeRuns: number;
  readonly clients: number;
  /** Background work outliving a turn (a parked chat's subagents). */
  readonly backgroundRuns?: number;
}

/**
 * Room for one more awake session: which sessions to put to sleep first, or
 * `undefined` when every awake session is busy. Evictable = ready, no active
 * run, and no client requests in its last heartbeat (or an engine that has
 * stopped heartbeating, whose report no longer holds); least recently
 * active first. Pure: `now` is a parameter.
 */
export const planEviction = (
  sessions: readonly AwakeSession[],
  waking: string,
  maxAwake: number,
  now: number
): string[] | undefined => {
  const awake = sessions.filter((s) => s.chatId !== waking && AWAKE_STATES.has(s.state));
  const need = awake.length - maxAwake + 1;
  if (need <= 0) return [];
  const idle = awake
    .filter((s) => {
      if (s.state !== "ready") return false;
      if (s.lastHeartbeatAt === undefined) {
        // Just came up and hasn't reported yet: its first command may be
        // about to run, so give it a minute before it counts as idle.
        return now - (s.lastActiveAt ?? 0) >= FIRST_REPORT_GRACE_MS;
      }
      if (now - s.lastHeartbeatAt >= HEARTBEAT_STALE_MS) return true; // dead engine
      return s.activeRuns === 0 && s.clients === 0 && (s.backgroundRuns ?? 0) === 0;
    })
    .sort((a, b) => (a.lastActiveAt ?? 0) - (b.lastActiveAt ?? 0));
  return idle.length >= need ? idle.slice(0, need).map((s) => s.chatId) : undefined;
};

export const parseIdleMs = (raw: string | undefined): number => {
  const minutes = Number(raw);
  return (Number.isFinite(minutes) && minutes > 0 ? minutes : 10) * 60_000;
};
