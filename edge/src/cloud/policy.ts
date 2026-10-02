/**
 * Pure decisions behind the Cloud device lifecycle (docs/design/cloud-device.md),
 * kept free of Workers APIs so the Node unit tier covers them directly: when
 * an idle device sleeps, when a runner token timestamp is acceptable, and
 * the deterministic workflow ids.
 * Provider-neutral: sandbox states come normalized from sandbox-provider.ts.
 */

export type CloudState =
  | "off"
  | "provisioning"
  | "starting"
  | "ready"
  | "sleeping"
  | "stopping"
  | "error"
  | "deleting";

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

/** `CLOUD_MACHINE`: the session sandbox size (default "default"). */
export const parseMachine = (raw: string | undefined): "small" | "default" | "large" =>
  raw === "small" || raw === "large" ? raw : "default";

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
      return s.activeRuns === 0 && s.clients === 0;
    })
    .sort((a, b) => (a.lastActiveAt ?? 0) - (b.lastActiveAt ?? 0));
  return idle.length >= need ? idle.slice(0, need).map((s) => s.chatId) : undefined;
};

export const parseIdleMs = (raw: string | undefined): number => {
  const minutes = Number(raw);
  return (Number.isFinite(minutes) && minutes > 0 ? minutes : 20) * 60_000;
};
