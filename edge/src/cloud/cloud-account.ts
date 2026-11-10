/**
 * CloudAccount — one Durable Object per (org, user): `cloud1/{orgId}/{userId}`
 * (docs/design/cloud-device.md, "Edge: Cloud control plane"). The single
 * source of truth for one user's Cloud:
 *
 * - the **account**: on/off and the logical Cloud device id (the vault parent
 *   every session device's provider connections are authorized through; it
 *   has no engine and no registry row);
 * - **sessions**: one per top-level chat a user runs on Cloud (the checkout
 *   picker's Cloud, on any project whose GitHub repository Cloud reaches),
 *   each with its OWN sandbox cloning that repository, session device id,
 *   enrolled runner key, idle tracking and lifecycle — sessions never share
 *   a machine;
 * - the usage meter (metering.ts) and lifecycle ledger for all of them.
 *
 * The DO decides; Workflows (workflows.ts) do the slow, retried provider work
 * per session and report back through the `wf*` methods. Every report carries
 * the chat id and the session `generation` the workflow was started for, so a
 * stale workflow (superseded by a delete or a newer lifecycle) can never
 * clobber current state. Provider-neutral: it speaks sandbox-provider.ts only,
 * and each sandbox keeps using the provider that created it.
 *
 * Concurrency discipline: records live in memory (loaded once under
 * blockConcurrencyWhile) and are checked-then-mutated synchronously,
 * re-checked after any await that lets other requests interleave (WebCrypto,
 * vault RPC, Workflows, the provider). That is what makes single-use
 * enrollment codes and strictly increasing token timestamps hold under
 * concurrent calls. Each session is stored under its own key so the account
 * scales with its session count.
 */
import { DurableObject } from "cloudflare:workers";
import { AUTH_USER_HEADER, type Env } from "../env";
import { encodeHlc } from "../registry-core";
import { mintRunnerToken, type RunnerToken } from "../runner-token";
import { cloudIndexStub } from "./cloud-index";
import {
  constantTimeEqual,
  parseEd25519PublicKey,
  randomCode,
  runnerTokenMessage,
  sha256B64url,
  verifyEd25519
} from "./crypto";
import { isBranch, isCheckoutPath, isRepoUrl } from "./install-script";
import {
  RECONCILE_EVERY_MS,
  STARTING_CREDITS,
  addCredits,
  appendLedger,
  creditBreakdown,
  creditRate,
  creditsGranted,
  creditsPerDay,
  ensureBillingTables,
  firstSeen,
  hasGrant,
  listPeriods,
  readBillingState,
  upsertPeriod,
  writeBillingState,
  closeUsageRow,
  ensureMeteringTables,
  listLedger,
  listSandboxes,
  listUsage,
  markSandboxDeleted,
  markSandboxFinal,
  meteringOpen,
  planReconcile,
  recordSandbox,
  recordUsageFailure,
  sandboxType,
  setSandboxType,
  sizeMultiplier,
  summarizeMonth,
  unmeteredCredits,
  upsertUsage,
  type CloudUsage,
  type CreditRate,
  type LedgerEntry,
  type MonthUsage,
  type LedgerEvent,
  type ReconcileTask,
  type SandboxRef
} from "./metering";
import {
  AWAKE_STATES,
  HEARTBEAT_STALE_MS,
  heartbeatStaleMs,
  MACHINES,
  diskLevel,
  idleDecision,
  machineName,
  machineSpec,
  newCloudDeviceId,
  parseMachine,
  refreshDueAt,
  parseIdleMs,
  parseMaxAwake,
  parseTtlSeconds,
  planEviction,
  shouldExtendTtl,
  tokenTimestampCheck,
  workflowInstanceId,
  type CloudState,
  type DiskLevel,
  type MachineSpec,
  type WorkflowKind
} from "./policy";
import { activeProviderName, sandboxProvider } from "./providers";
import { WAKE_STAGE_TTL_MS, wakeTiming, type EngineTiming, type WakeStage, type WakeStages, type WakeTrace } from "./wake";
import {
  CREDIT_PACK,
  PLANS,
  StripeError,
  createCheckout,
  createPortal,
  planById,
  type BillingEvent,
  type CheckoutRequest,
  type CreditPack,
  type Plan,
  type StripeEnv,
  type SubscriptionState
} from "./stripe";
import { SandboxError, effectiveTtl, type SandboxFailure, type SandboxMachine, type SandboxUsage } from "./sandbox-provider";

export interface CloudCaller {
  readonly orgId: string;
  readonly userId: string;
  /** A Cloud machine acting for the chat it hosts (runner bearer): it may
   * only start and manage that chat's child sessions. Absent for the user. */
  readonly deviceId?: string;
}

/** The user action whose lifecycle failed (the UI's Retry). */
export type CloudAction = "enable" | "wake" | "sleep" | "delete";

const ACTION_OF: Record<WorkflowKind, CloudAction> = {
  provision: "enable",
  wake: "wake",
  sleep: "sleep",
  delete: "delete"
};

/** Wire shape of `CloudStatus` (crates/proto/src/cloud.rs): the account. */
export interface CloudStatusView {
  readonly state: CloudState;
  readonly deviceId?: string;
  readonly error?: string;
  readonly failedAction?: CloudAction;
  readonly lastActiveAt?: number;
  readonly awakeSessions: number;
  readonly maxAwakeSessions: number;
  readonly available: boolean;
  /** Credits left (a minute of a small machine each); sessions don't start
   * or wake at zero or below. */
  readonly credits: number;
  /** Every machine size a user can pick (policy.ts). */
  readonly machines: readonly MachineSpec[];
  /** The account's default: what new sessions start on. */
  readonly machine: SandboxMachine;
}

/** Subscription statuses that still grant the plan (a failed renewal stays
 * `past_due` while Stripe retries). */
const SUBSCRIBED: ReadonlySet<string> = new Set(["active", "trialing", "past_due"]);

const stripeFailure = (e: unknown): CloudResult<never> => {
  if (e instanceof StripeError) return fail(e.status, e.status === 503 ? "unavailable" : "billing_failed", e.message);
  console.error("billing: Stripe call failed", String(e));
  return fail(502, "billing_failed", "Couldn't reach the payment service; try again.");
};

/** Wire shape of `BillingStatus` (crates/proto/src/cloud.rs). */
export interface BillingView {
  /** Billing is configured on this server. */
  readonly available: boolean;
  readonly plan?: Plan & {
    readonly status: string;
    readonly periodEnd: number;
    readonly cancelAtPeriodEnd: boolean;
  };
  readonly credits: {
    readonly monthly: number;
    readonly monthlyTotal: number;
    readonly monthlyEndsAt?: number;
    readonly extra: number;
    /** What sessions can spend now (both, less unmetered awake time). */
    readonly total: number;
  };
  readonly plans: readonly Plan[];
  readonly pack: CreditPack;
  readonly hasCustomer: boolean;
}

/** Wire shape of `CloudSession`. */
export interface CloudSessionView {
  readonly chatId: string;
  readonly deviceId: string;
  readonly spaceId: string;
  /** The repository the machine clones (`owner/name`) and where. */
  readonly repo: string;
  readonly path: string;
  readonly state: CloudState;
  readonly error?: string;
  readonly failedAction?: CloudAction;
  readonly lastActiveAt?: number;
  readonly createdAt: number;
  readonly sandbox?: { readonly provider: string; readonly id: string };
  /** The size it is on, or will come back on when it was picked asleep. */
  readonly machine: SandboxMachine;
  /** Omitted until its engine first reports. `usedGiB` holds its high-water
   * mark while a resumed disk is still copying in (design doc, "Disk"). */
  readonly disk?: { readonly usedGiB: number; readonly capacityGiB: number; readonly level: DiskLevel };
  /** Its disk doesn't fit: it waits for the user to pick a bigger size. */
  readonly wakeBlocked?: { readonly reason: "disk_too_small"; readonly neededGiB: number };
  /** Unix ms its engine last found its chat store damaged on a start and
   * rebuilt it (from the rooms, and what could still be read). */
  readonly storeRecoveredAt?: number;
  /** Unix ms the provider deletes its disk, while it sleeps (a refresh pushes
   * this out; past it with `state: "data_expired"` it is gone). */
  readonly expiresAt?: number;
  /** A longer wake's reason, while it lasts: its engine says what it's doing. */
  readonly stage?: WakeStage;
}

export type CloudResult<T> =
  | { readonly ok: true; readonly value: T }
  | {
      readonly ok: false;
      readonly status: number;
      readonly error: string;
      readonly message: string;
      /** Typed fields beside `{error, message}` on the wire. */
      readonly details?: Readonly<Record<string, string | number>>;
    };

/** The repository a session sandbox clones. */
export interface SessionProject {
  readonly cloneUrl: string;
  readonly path: string;
  readonly defaultBranch: string;
}

export interface ProvisionParams {
  readonly orgId: string;
  readonly userId: string;
  readonly chatId: string;
  /** The session device. */
  readonly deviceId: string;
  /** The account's logical Cloud device. */
  readonly accountDeviceId: string;
  readonly generation: number;
  /** Provider the sandbox lives (or will live) on. */
  readonly provider: string;
  /** One-time code; the DO keeps only its hash. Lives in the instance's
   * params until the workflow ends (never in a step output). */
  readonly enrollCode: string;
  readonly edgeUrl: string;
  readonly project: SessionProject;
  /** Set when retrying a failed provision on the sandbox it already made. */
  readonly sandboxId?: string;
  /** The account's machine size, when an operator set one. */
  readonly machine?: SandboxMachine;
}

export interface LifecycleParams {
  readonly orgId: string;
  readonly userId: string;
  readonly chatId: string;
  readonly deviceId: string;
  readonly generation: number;
  readonly provider: string;
  readonly sandboxId: string;
  /** Sleep only: stop even a busy machine (out of credits). An idle sleep
   * leaves a busy one running. */
  readonly force?: boolean;
  /** Wake only: come back on this size. */
  readonly machine?: SandboxMachine;
}

export interface EnrollRequest extends CloudCaller {
  readonly deviceId: string;
  readonly code: string;
  readonly publicKey: string;
}

export interface TokenRequest extends CloudCaller {
  readonly deviceId: string;
  readonly ts: number;
  readonly sig: string;
}

export interface Heartbeat {
  readonly activeRuns: number;
  readonly clients: number;
  /** Background work outliving a turn (a parked chat's subagents). */
  readonly backgroundRuns?: number;
  /** Of `activeRuns`, those waiting on an answer rather than working. */
  readonly waitingRuns?: number;
  /** Its disk's used bytes, as the machine reads them. */
  readonly diskUsedBytes?: number;
  /** How often it beats from here on, in seconds. Older engines don't say. */
  readonly everySecs?: number;
  /** When this run of the engine found its chat store damaged and rebuilt it. */
  readonly storeRecoveredAt?: number;
}

/** The GitHub repository a new session runs on (its project's origin). */
export interface RepoInput {
  readonly fullName: string;
  readonly cloneUrl: string;
  readonly defaultBranch: string;
}

/** A session's repository and where its machine checks it out. */
interface SessionRepo extends RepoInput {
  readonly path: string;
}

/** Per-user billing view for the operator export. */
export interface CloudBillingRow extends MonthUsage {
  readonly orgId?: string;
  readonly userId?: string;
  readonly deviceId?: string;
  readonly errors: readonly string[];
}

interface AccountRecord {
  state: "off" | "ready" | "deleting" | "error";
  /** Bumped on account delete; never reset. */
  generation: number;
  orgId?: string;
  userId?: string;
  /** The logical Cloud device. */
  deviceId?: string;
  deviceCreatedAt?: number;
  deletingSince?: number;
  error?: string;
  failedAction?: CloudAction;
  /** Alarm multiplexing: the metering reconcile shares the DO's one alarm
   * with every session's idle check. */
  nextReconcileAt?: number;
  /** A failed sweep of deleted sessions' devices retries then
   * (`sweepStaleDevices`). */
  sweepDevicesAt?: number;
  /** The next check for machines whose chat is gone (`sweepOrphans`). */
  nextOrphanSweepAt?: number;
  /** Logical device ids of earlier times Cloud was on (each turn-off
   * revokes one). Connections still authorized for them move to the new
   * one on enable. Survives turning Cloud off. */
  retiredDeviceIds?: string[];
  /** The last reconcile that read awake machines' meters (any kind). */
  lastReconcileAt?: number;
  /** The size this account's new sessions start on, over `CLOUD_MACHINE`:
   * the user's pick (`PUT /cloud/{org}/machine`) or an operator's
   * (`POST /admin/cloud/machine`), whichever came last. */
  machine?: SandboxMachine;
  /** Operator-set caps for this account (`POST /admin/cloud/limits`), over
   * `CLOUD_MAX_AWAKE` / `CLOUD_MAX_SESSIONS`. */
  maxAwake?: number;
  maxSessions?: number;
  /** The last reconcile of every open row (the others read awake machines'). */
  lastFullReconcileAt?: number;
}

interface SessionRecord {
  chatId: string;
  /** The project (on whichever device) the chat belongs to. */
  spaceId: string;
  repo: SessionRepo;
  /** The branch the session starts from (picked in the composer); absent =
   * the repository's default branch. */
  baseBranch?: string;
  /** The session device (`cloud-{uuid}`). */
  deviceId: string;
  createdAt: number;
  state: CloudState;
  /** Bumped on every lifecycle start (and on delete); workflow reports for an
   * older generation are ignored. */
  generation: number;
  provider?: string;
  sandboxId?: string;
  runnerKey?: string;
  enrollHash?: string;
  enrolledAt?: number;
  lastTokenTs?: number;
  /** Set by a wake; the runner's first token or heartbeat clears it and fires
   * the WakeWorkflow's `online` event. */
  awaitingOnline?: boolean;
  /** The wake in progress, until its engine reports its side of it. */
  wakeTrace?: WakeTrace;
  /** What its engine says it's doing before it answers, and since when. */
  wakeStage?: { stage: WakeStage; at: number };
  lastActiveAt?: number;
  lastHeartbeatAt?: number;
  activeRuns: number;
  clients: number;
  /** Last heartbeat's background work (see {@link Heartbeat}). */
  backgroundRuns?: number;
  /** Last heartbeat's runs waiting on an answer. */
  waitingRuns?: number;
  /** How often its engine said, in its last heartbeat, it beats from then on
   * (newer engines: every 15 s while a turn runs). */
  beatEveryMs?: number;
  lastTtlExtendAt?: number;
  /** Throttles the "ready but silent — did the provider auto-stop it?" probe. */
  lastProbeAt?: number;
  /** The last send that reached this machine while it was up. */
  lastNudgeAt?: number;
  workflow?: { kind: WorkflowKind; id: string };
  /** A wake requested while stopping: start it as soon as the stop lands. */
  pendingWake?: boolean;
  /** A delete failed half-way; only a delete may proceed from here. */
  pendingDelete?: boolean;
  deletingSince?: number;
  error?: string;
  failedAction?: CloudAction;
  idleCheckAt?: number;
  /** When the machine last started running (provision or wake): unmetered
   * time since the last reconcile counts against the balance. */
  awakeSince?: number;
  /** A delegate: the chat whose machine started this session. One level
   * only — a child never starts children of its own. */
  parentChatId?: string;
  /** Failed deletes so far, and when the next one runs on its own: a
   * machine nobody can reach must not stay billed until someone retries. */
  deleteAttempts?: number;
  deleteRetryAt?: number;
  /** The size its sandbox is on. Absent on sessions that predate choosing:
   * read `sandboxes`, which recorded it at create. */
  machine?: SandboxMachine;
  /** Picked while it sleeps: the next resume moves it there. */
  pendingMachine?: SandboxMachine;
  /** A resume was refused for want of disk. Never retried on its own and
   * never upsized on its own: credits change, so the user picks. */
  wakeBlocked?: { reason: "disk_too_small"; neededGiB: number };
  /** See {@link CloudSessionView.storeRecoveredAt}. */
  storeRecoveredAt?: number;
  /** The disk's high-water mark and when it was last raised or settled. */
  disk?: { usedGiB: number; at: number };
  /** Resumed, and its disk may still be copying in: a lower reading means
   * nothing yet. Cleared once the provider says it is all there. */
  diskHydrating?: boolean;
  lastHydrationProbeAt?: number;
  /** Unix ms of its last stop: the provider's retention clock. */
  stoppedAt?: number;
  /** The next look at its retention clock (`checkExpiry`). */
  expiryCheckAt?: number;
  /** This wake only keeps the disk from expiring: it goes back to sleep as
   * soon as it is up, unless someone turns up meanwhile. */
  refreshing?: boolean;
}

const ACCOUNT_KEY = "account";
/** Earlier logical device ids remembered for moving connections over. */
const RETIRED_DEVICES_KEPT = 8;
/** While a machine is awake, credits are re-checked this often. */
const CREDIT_CHECK_MS = 5 * 60_000;
const SESSION_PREFIX = "session:";
/** A delete that crashed mid-way may be retried after this long. */
const DELETE_STUCK_MS = 5 * 60_000;
/** Waits before retrying a failed delete on its own; the last repeats. */
const DELETE_RETRY_MS = [60_000, 5 * 60_000, 15 * 60_000, 60 * 60_000];
/** How often machines are checked against their chats, and how old a
 * session must be before a missing chat row counts as deleted (the laptop
 * writes the row after the session starts). */
const ORPHAN_SWEEP_MS = 10 * 60_000;
const ORPHAN_GRACE_MS = 15 * 60_000;
/** The writer the edge's own registry ops are attributed to (and clocked by). */
const REGISTRY_WRITER = "cloud-edge";
/** Mirrors `zeron_proto::CLOUD_SESSION_CAPABILITY` (a session machine's device). */
const CLOUD_SESSION_CAPABILITY = "cloud-session";
const SWEEP_RETRY_MS = 5 * 60_000;
const GIB = 1024 ** 3;
/** How often a lower disk reading asks whether the disk is all there yet. */
const HYDRATION_PROBE_MS = 5 * 60_000;
/** Back-off of a retention check that couldn't refresh a machine. */
const EXPIRY_RETRY_MS = 60 * 60_000;
/** No room among the awake machines: soon, since it only needs a minute. */
const EXPIRY_NO_ROOM_MS = 15 * 60_000;
/** Refreshes started by one alarm pass, so a large account spreads out. */
const REFRESHES_PER_PASS = 2;
const DATA_EXPIRED_MESSAGE =
  "This machine's files were deleted after it slept for too long. The chat is safe; start a fresh machine to carry on.";
const FULL_NAME_RE = /^[A-Za-z0-9_.-]{1,100}\/[A-Za-z0-9_.-]{1,100}$/;
/** Where a session machine checks its repository out (one per machine;
 * `CLOUD_PROJECTS_ROOT` overrides it for a local stack). */
const CLOUD_PROJECTS_ROOT = "/home/user";
const ID_RE = /^[A-Za-z0-9_-]{1,128}$/;

const freshAccount = (generation: number, caller?: CloudCaller): AccountRecord => ({
  state: "off",
  generation,
  ...(caller ? { orgId: caller.orgId, userId: caller.userId } : {})
});

const fail = <T>(
  status: number,
  error: string,
  message: string,
  details?: Readonly<Record<string, string | number>>
): CloudResult<T> => ({
  ok: false,
  status,
  error,
  message,
  ...(details ? { details } : {})
});

const roundGiB = (value: number): number => Math.round(value * 10) / 10;

const explain = (e: unknown): string =>
  e instanceof SandboxError ? `${e.provider} ${e.providerCode}: ${e.message}` : String(e);

export class CloudAccount extends DurableObject<Env> {
  private acct: AccountRecord = freshAccount(0);
  private readonly sessions = new Map<string, SessionRecord>();
  /** session device id → chat id */
  private readonly byDevice = new Map<string, string>();
  private tablesReady = false;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    void ctx.blockConcurrencyWhile(async () => {
      this.acct = (await ctx.storage.get<AccountRecord>(ACCOUNT_KEY)) ?? freshAccount(0);
      for (const [, session] of await ctx.storage.list<SessionRecord>({ prefix: SESSION_PREFIX })) {
        this.sessions.set(session.chatId, session);
        this.byDevice.set(session.deviceId, session.chatId);
      }
    });
  }

  // ── plumbing ──────────────────────────────────────────────────────────────

  /** Metering tables, created lazily so an unauthenticated /runner/* probe
   * against a never-enabled (org, user) leaves no storage behind. */
  private sql(): SqlStorage {
    if (!this.tablesReady) {
      ensureMeteringTables(this.ctx.storage.sql);
      ensureBillingTables(this.ctx.storage.sql);
      this.tablesReady = true;
    }
    return this.ctx.storage.sql;
  }

  private async saveAccount(): Promise<void> {
    await this.ctx.storage.put(ACCOUNT_KEY, this.acct);
    await this.armAlarm();
  }

  private async saveSession(s: SessionRecord): Promise<void> {
    await this.ctx.storage.put(`${SESSION_PREFIX}${s.chatId}`, s);
    await this.armAlarm();
  }

  private async dropSession(s: SessionRecord): Promise<void> {
    this.sessions.delete(s.chatId);
    this.byDevice.delete(s.deviceId);
    await this.ctx.storage.delete(`${SESSION_PREFIX}${s.chatId}`);
    await this.armAlarm();
  }

  private async armAlarm(): Promise<void> {
    const due: number[] = [];
    for (const s of this.sessions.values()) {
      if (s.state === "ready" && s.idleCheckAt !== undefined) due.push(s.idleCheckAt);
      if (s.deleteRetryAt !== undefined) due.push(s.deleteRetryAt);
      if (s.state === "deleting") due.push((s.deletingSince ?? 0) + DELETE_STUCK_MS);
      if (this.awaitingExpiry(s) && s.expiryCheckAt !== undefined) due.push(s.expiryCheckAt);
    }
    // While any machine exists, it is checked against its chat.
    if (this.sessions.size > 0) {
      this.acct.nextOrphanSweepAt ??= Date.now() + ORPHAN_SWEEP_MS;
      due.push(this.acct.nextOrphanSweepAt);
    }
    if (this.acct.nextReconcileAt !== undefined) due.push(this.acct.nextReconcileAt);
    if (this.acct.sweepDevicesAt !== undefined) due.push(this.acct.sweepDevicesAt);
    if (due.length === 0) await this.ctx.storage.deleteAlarm();
    else await this.ctx.storage.setAlarm(Math.min(...due));
  }

  private matches(caller: CloudCaller): boolean {
    return this.acct.orgId === caller.orgId && this.acct.userId === caller.userId;
  }

  /** First authenticated management call stamps the DO's identity (a DO
   * cannot read its own name); later calls must match it. */
  private bind(caller: CloudCaller): boolean {
    if (this.acct.orgId === undefined) {
      this.acct.orgId = caller.orgId;
      this.acct.userId = caller.userId;
      return true;
    }
    return this.matches(caller);
  }

  private workflowsBound(): boolean {
    const e = this.env;
    return Boolean(e.CLOUD_PROVISION && e.CLOUD_WAKE && e.CLOUD_SLEEP && e.CLOUD_DELETE);
  }

  private providerOf(s: SessionRecord) {
    return sandboxProvider(this.env, s.provider ?? activeProviderName(this.env));
  }

  private idleMs(): number {
    return parseIdleMs(this.env.CLOUD_IDLE_MINUTES);
  }

  private maxAwake(): number {
    return this.acct.maxAwake ?? parseMaxAwake(this.env.CLOUD_MAX_AWAKE);
  }

  private ledger(event: LedgerEvent, s?: SessionRecord, extra: Omit<LedgerEntry, "at" | "event"> = {}): void {
    appendLedger(this.sql(), {
      at: Date.now(),
      event,
      ...(s
        ? { chatId: s.chatId, provider: s.provider, sandboxId: s.sandboxId, workflowId: s.workflow?.id }
        : {}),
      ...extra
    });
  }

  private awakeCount(): number {
    let n = 0;
    for (const s of this.sessions.values()) if (AWAKE_STATES.has(s.state)) n++;
    return n;
  }

  private accountView(): CloudStatusView {
    const a = this.acct;
    let lastActiveAt: number | undefined;
    for (const s of this.sessions.values()) {
      if (s.lastActiveAt !== undefined && (lastActiveAt === undefined || s.lastActiveAt > lastActiveAt)) {
        lastActiveAt = s.lastActiveAt;
      }
    }
    return {
      state: a.state,
      ...(a.deviceId ? { deviceId: a.deviceId } : {}),
      ...(a.error ? { error: a.error } : {}),
      ...(a.state === "error" && a.failedAction ? { failedAction: a.failedAction } : {}),
      ...(lastActiveAt ? { lastActiveAt } : {}),
      awakeSessions: this.awakeCount(),
      maxAwakeSessions: this.maxAwake(),
      available: true,
      credits: Math.floor(this.balance() * 100) / 100,
      machines: MACHINES,
      machine: this.defaultMachine()
    };
  }

  private defaultMachine(): SandboxMachine {
    return this.acct.machine ?? parseMachine(this.env.CLOUD_MACHINE);
  }

  /** The size `s`'s sandbox is on. */
  private currentMachine(s: SessionRecord): SandboxMachine {
    if (s.machine) return s.machine;
    const recorded =
      s.provider && s.sandboxId && this.acct.orgId
        ? machineName(sandboxType(this.sql(), { provider: s.provider, sandboxId: s.sandboxId }))
        : undefined;
    return recorded ?? this.defaultMachine();
  }

  /** What clients see: the size it will come back on. */
  private shownMachine(s: SessionRecord): SandboxMachine {
    return s.pendingMachine ?? this.currentMachine(s);
  }

  /** The disk doesn't fit the size it would come back on. */
  private blockedOnDisk(s: SessionRecord, machine = this.shownMachine(s)): boolean {
    return s.wakeBlocked !== undefined && (machineSpec(machine)?.diskGiB ?? 0) < s.wakeBlocked.neededGiB;
  }

  private retentionMs(s: SessionRecord): number | undefined {
    return this.providerOf(s)?.capabilities.stoppedRetentionMs;
  }

  /** Stopped, so its retention clock is running. */
  private awaitingExpiry(s: SessionRecord): boolean {
    return (s.state === "sleeping" || s.state === "error") && Boolean(s.sandboxId) && !s.pendingDelete;
  }

  private expiresAt(s: SessionRecord): number | undefined {
    if (s.stoppedAt === undefined || !(this.awaitingExpiry(s) || s.state === "data_expired")) return undefined;
    const retention = this.retentionMs(s);
    return retention === undefined ? undefined : s.stoppedAt + retention;
  }

  private diskView(s: SessionRecord): CloudSessionView["disk"] {
    const capacityGiB = machineSpec(this.shownMachine(s))?.diskGiB;
    if (!s.disk || capacityGiB === undefined) return undefined;
    return { usedGiB: roundGiB(s.disk.usedGiB), capacityGiB, level: diskLevel(s.disk.usedGiB, capacityGiB) };
  }

  /** Credits left: granted minus used, less awake machines' time since their
   * meters were last read (a running machine is never free). */
  private balance(now = Date.now()): number {
    if (!this.acct.orgId) return 0;
    const credits = this.breakdown(now);
    let unmetered = 0;
    const rate = this.rate();
    let sizes: Map<string, string> | undefined;
    for (const s of this.sessions.values()) {
      if (!AWAKE_STATES.has(s.state)) continue;
      const since = Math.max(s.awakeSince ?? now, this.acct.lastReconcileAt ?? 0);
      // A bigger machine spends credits faster: estimate with its size.
      sizes ??= new Map(listSandboxes(this.sql()).map((row) => [row.sandboxId, row.type]));
      // Its live size: a resize moved it off what it was created on.
      const type = s.machine ?? (s.sandboxId ? sizes.get(s.sandboxId) : this.acct.machine);
      unmetered += unmeteredCredits(since, now, sizeMultiplier(type), rate);
    }
    return credits.monthly + credits.extra - unmetered;
  }

  /** Plan credits this period and credits that never expire. */
  private breakdown(now = Date.now()) {
    const sql = this.sql();
    return creditBreakdown(creditsGranted(sql), listPeriods(sql), listUsage(sql), now, this.rate());
  }

  /** What a credit costs, by day (`CLOUD_CREDIT_SECONDS`, `CLOUD_CREDIT_RATE_FROM`). */
  private rate(): CreditRate {
    return creditRate(this.env.CLOUD_CREDIT_SECONDS, this.env.CLOUD_CREDIT_RATE_FROM);
  }

  private outOfCredits(): CloudResult<never> {
    return fail(402, "no_credits", "You're out of Cloud credits. Add more in Settings → Billing.");
  }

  private sessionView(s: SessionRecord): CloudSessionView {
    const disk = this.diskView(s);
    const expiresAt = this.expiresAt(s);
    return {
      chatId: s.chatId,
      deviceId: s.deviceId,
      spaceId: s.spaceId,
      repo: s.repo.fullName,
      path: s.repo.path,
      state: s.state,
      ...(s.error ? { error: s.error } : {}),
      ...(s.state === "error" && s.failedAction ? { failedAction: s.failedAction } : {}),
      ...(s.lastActiveAt ? { lastActiveAt: s.lastActiveAt } : {}),
      createdAt: s.createdAt,
      ...(s.provider && s.sandboxId ? { sandbox: { provider: s.provider, id: s.sandboxId } } : {}),
      machine: this.shownMachine(s),
      ...(disk ? { disk } : {}),
      ...(s.wakeStage && Date.now() - s.wakeStage.at < WAKE_STAGE_TTL_MS ? { stage: s.wakeStage.stage } : {}),
      ...(s.wakeBlocked ? { wakeBlocked: { reason: s.wakeBlocked.reason, neededGiB: s.wakeBlocked.neededGiB } } : {}),
      ...(s.storeRecoveredAt === undefined ? {} : { storeRecoveredAt: s.storeRecoveredAt }),
      ...(expiresAt === undefined ? {} : { expiresAt })
    };
  }

  private okAccount(): CloudResult<CloudStatusView> {
    return { ok: true, value: this.accountView() };
  }

  private okSession(s: SessionRecord): CloudResult<CloudSessionView> {
    return { ok: true, value: this.sessionView(s) };
  }

  private workflowBinding(kind: WorkflowKind): Workflow | undefined {
    switch (kind) {
      case "provision":
        return this.env.CLOUD_PROVISION;
      case "wake":
        return this.env.CLOUD_WAKE;
      case "sleep":
        return this.env.CLOUD_SLEEP;
      case "delete":
        return this.env.CLOUD_DELETE;
    }
  }

  /** Tell the index (billing's walk list) about this account. Best effort:
   * every lifecycle event and reconcile registers again. */
  private register(): void {
    const index = cloudIndexStub(this.env);
    const { orgId, userId, deviceId } = this.acct;
    if (!index || !orgId || !userId) return;
    const sandboxes = listSandboxes(this.sql()).map(({ provider, sandboxId }) => ({ provider, sandboxId }));
    this.ctx.waitUntil(
      index
        .register({ orgId, userId, deviceId, sandboxes })
        .catch((e: unknown) => console.warn("cloud.index register failed", String(e)))
    );
  }

  /** When metering runs next: soon while a machine is awake (its credits
   * are being spent), hourly while any row is still open, else never. */
  private nextReconcile(sql: SqlStorage, now: number): number | undefined {
    if (this.awakeCount() > 0) return now + CREDIT_CHECK_MS;
    return meteringOpen(listSandboxes(sql), listUsage(sql), now) ? now + RECONCILE_EVERY_MS : undefined;
  }

  private scheduleReconcile(at = Date.now() + 1_000): void {
    this.acct.nextReconcileAt = Math.min(this.acct.nextReconcileAt ?? Infinity, at);
  }

  private sendEvent(s: SessionRecord, kind: WorkflowKind, type: "enrolled" | "online"): void {
    const workflow = s.workflow;
    const binding = this.workflowBinding(kind);
    if (!workflow || workflow.kind !== kind || !binding) return;
    this.ctx.waitUntil(
      (async () => {
        try {
          const instance = await binding.get(workflow.id);
          await instance.sendEvent({ type, payload: {} });
        } catch (e) {
          // The workflow re-checks the DO before and after waiting, so a lost
          // event costs at most its wait timeout, never correctness.
          console.warn("cloud workflow event failed", type, workflow.id, String(e));
        }
      })()
    );
  }

  /** Start a session lifecycle workflow under a fresh generation. The caller
   * has already moved `state` synchronously, so concurrent calls see it. */
  private async startWorkflow<P>(
    s: SessionRecord,
    kind: WorkflowKind,
    params: (generation: number) => P
  ): Promise<CloudResult<CloudSessionView>> {
    const binding = this.workflowBinding(kind);
    const generation = ++s.generation;
    const id = workflowInstanceId(kind, s.deviceId, generation);
    s.workflow = { kind, id };
    s.failedAction = undefined;
    await this.saveSession(s);
    if (!binding) return this.failNow(s, ACTION_OF[kind], 503, "unavailable", "Cloud is not configured on this server.");
    try {
      await binding.create({ id, params: params(generation) });
    } catch (e) {
      // Deterministic ids: "already exists" means an earlier attempt got
      // through, which is exactly the instance we wanted.
      if (!/exist/i.test(String(e))) {
        console.error("cloud workflow create failed", kind, id, String(e));
        return this.failNow(s, ACTION_OF[kind], 503, "workflow_failed", "Couldn't start the Cloud workflow; try again.");
      }
    }
    return this.okSession(s);
  }

  private async failNow(
    s: SessionRecord,
    action: CloudAction,
    status: number,
    error: string,
    message: string
  ): Promise<CloudResult<CloudSessionView>> {
    if (s.state === "deleting") this.noteDeleteFailure(s);
    s.state = "error";
    s.error = message;
    s.failedAction = action;
    this.ledger("error", s, { detail: message });
    await this.saveSession(s);
    return fail(status, error, message);
  }

  private markReady(s: SessionRecord, now: number): void {
    s.state = "ready";
    s.error = undefined;
    s.failedAction = undefined;
    s.wakeBlocked = undefined;
    s.stoppedAt = undefined;
    s.expiryCheckAt = undefined;
    s.lastActiveAt = now;
    s.idleCheckAt = now + this.idleMs();
    this.ledger("ready", s);
    this.scheduleReconcile();
  }

  /** The session's runner proved it is alive (a token or a heartbeat). */
  private markAlive(s: SessionRecord, now: number): void {
    if (s.awaitingOnline) {
      s.awaitingOnline = false;
      if (s.wakeTrace) s.wakeTrace.online = now;
      this.sendEvent(s, "wake", "online");
    }
    // Its engine is talking: whatever was slow about this wake is over.
    if (s.wakeStage?.stage === "slow") s.wakeStage = undefined;
    // Sleeping/error but the engine is talking: the sandbox came back without
    // us (resumed from the provider's side, or a wake that timed out finished
    // late). Believe the engine.
    if (s.state === "sleeping" || (s.state === "error" && !s.pendingDelete)) this.markReady(s, now);
  }

  /**
   * Room for one more awake session: put the least recently active idle
   * sessions to sleep first. `false` when every awake session is busy.
   */
  private async makeRoomFor(chatId: string): Promise<boolean> {
    const now = Date.now();
    const plan = planEviction([...this.sessions.values()], chatId, this.maxAwake(), now);
    if (plan === undefined) return false;
    for (const evict of plan) {
      const s = this.sessions.get(evict);
      if (s) await this.beginSleep(s, "evicted to make room for another session");
    }
    return true;
  }

  private tooManyAwake(): CloudResult<never> {
    return fail(
      409,
      "too_many_awake",
      `All ${this.maxAwake()} of your Cloud sessions that may run at once are busy. Wait for one to finish, or put one to sleep.`
    );
  }

  private accountOf(caller: CloudCaller): CloudResult<never> | undefined {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    if (this.acct.state !== "ready" || !this.acct.deviceId) {
      return fail(409, "cloud_off", "Cloud isn't enabled. Turn it on in Settings → Cloud.");
    }
    return undefined;
  }

  // ── account (Worker routes, WorkOS bearer) ────────────────────────────────

  status(caller: CloudCaller): CloudResult<CloudStatusView> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    return this.okAccount();
  }

  /** `PUT /cloud/{org}/machine`: the size this user's new sessions start on.
   * Sessions that already have a machine keep theirs. */
  async setDefaultMachine(caller: CloudCaller, machine: SandboxMachine): Promise<CloudResult<CloudStatusView>> {
    if (!this.bind(caller)) return fail(403, "forbidden", "Not your Cloud.");
    this.acct.machine = machine;
    await this.saveAccount();
    return this.okAccount();
  }

  /** Turn Cloud on: mint and register the logical device. Instant — machines
   * are per session. */
  async enable(caller: CloudCaller): Promise<CloudResult<CloudStatusView>> {
    if (!this.bind(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const a = this.acct;
    if (a.state === "ready" || a.state === "deleting") return this.okAccount();
    if (!this.workflowsBound() || !sandboxProvider(this.env, activeProviderName(this.env))) {
      return fail(503, "unavailable", "Cloud is not available on this server.");
    }
    if (!a.deviceId) {
      a.deviceId = newCloudDeviceId();
      a.deviceCreatedAt = Date.now();
    }
    a.state = "ready";
    a.error = undefined;
    a.failedAction = undefined;
    // Once per user, ever: the grant row outlives turning Cloud off.
    const starting = Number(this.env.CLOUD_STARTING_CREDITS ?? "0");
    if (Number.isFinite(starting) && starting > 0 && !hasGrant(this.sql(), STARTING_CREDITS)) {
      addCredits(this.sql(), { at: Date.now(), credits: starting, reason: STARTING_CREDITS });
    }
    await this.saveAccount();
    this.ledger("ready", undefined, { detail: `account ${a.deviceId}` });
    this.register();
    await this.moveConnections(caller, a.deviceId);
    return this.okAccount();
  }

  /**
   * Turning Cloud off revoked its logical device and turning it on minted a
   * new one: every stored connection (GitHub, the Claude Code and Codex
   * accounts) still authorized for an earlier Cloud is authorized for this
   * one instead, so nothing has to be connected again. Best effort: a
   * failure leaves the connection for the user to re-connect.
   */
  private async moveConnections(caller: CloudCaller, deviceId: string): Promise<void> {
    const retired = this.acct.retiredDeviceIds ?? [];
    const vault = this.env.VAULT;
    if (retired.length === 0 || !vault) return;
    const user = { userId: caller.userId, orgId: caller.orgId, kind: "user" as const };
    try {
      const status = await vault.status(user);
      if (!status.ok) throw new Error(`${status.error}: ${status.message}`);
      for (const connection of status.value.connections) {
        if (!connection.authorizedDevices.some((id) => retired.includes(id))) continue;
        const devices = [
          ...new Set(connection.authorizedDevices.map((id) => (retired.includes(id) ? deviceId : id)))
        ];
        const moved = await vault.authorize(user, connection.provider, devices);
        if (!moved.ok) throw new Error(`${connection.provider}: ${moved.error}: ${moved.message}`);
      }
      this.acct.retiredDeviceIds = undefined;
      await this.saveAccount();
    } catch (e) {
      console.warn("cloud enable: moving connections to the new Cloud failed", explain(e));
    }
  }

  /**
   * Turn Cloud off: delete every session's sandbox (each through the exact
   * stop → final usage → delete path), revoke the logical device in the vault
   * (which revokes every session device under it, including ones enrolled
   * later), tombstone its registry row. Answers `deleting`; reads `off` once
   * the last session is gone.
   */
  async destroy(caller: CloudCaller): Promise<CloudResult<CloudStatusView>> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const a = this.acct;
    if (a.state === "off") return this.okAccount();
    if (a.state === "deleting" && Date.now() - (a.deletingSince ?? 0) < DELETE_STUCK_MS) return this.okAccount();
    a.state = "deleting";
    a.deletingSince = Date.now();
    a.generation++;
    await this.saveAccount();
    if (a.deviceId && this.env.VAULT) {
      try {
        const res = await this.env.VAULT.revokeDevice({ userId: caller.userId, orgId: caller.orgId, kind: "user" }, a.deviceId);
        if (!res.ok && res.error !== "not_found" && res.error !== "device_unknown") {
          console.warn("cloud delete: vault revoke failed", a.deviceId, res.error, res.message);
        }
      } catch (e) {
        console.warn("cloud delete: vault revoke threw", a.deviceId, String(e));
      }
    }
    for (const s of [...this.sessions.values()]) await this.beginDelete(caller, s);
    await this.maybeFinishAccountDelete();
    return this.okAccount();
  }

  /** The account delete completes once its last session is gone. */
  private async maybeFinishAccountDelete(): Promise<void> {
    if (this.acct.state !== "deleting" || this.sessions.size > 0) return;
    const { generation, orgId, userId, deviceId, retiredDeviceIds = [], machine, maxAwake, maxSessions } = this.acct;
    this.acct = freshAccount(generation, orgId && userId ? { orgId, userId } : undefined);
    // Operator settings outlive turning Cloud off.
    if (machine) this.acct.machine = machine;
    if (maxAwake !== undefined) this.acct.maxAwake = maxAwake;
    if (maxSessions !== undefined) this.acct.maxSessions = maxSessions;
    if (deviceId) this.acct.retiredDeviceIds = [...retiredDeviceIds, deviceId].slice(-RETIRED_DEVICES_KEPT);
    const sql = this.sql();
    const now = Date.now();
    this.acct.nextReconcileAt = this.nextReconcile(sql, now);
    this.register();
    await this.saveAccount();
  }

  // ── sessions ──────────────────────────────────────────────────────────────

  listSessions(caller: CloudCaller): CloudResult<{ sessions: CloudSessionView[] }> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const sessions = [...this.sessions.values()]
      .sort((a, b) => b.createdAt - a.createdAt)
      .map((s) => this.sessionView(s));
    return { ok: true, value: { sessions } };
  }

  /**
   * A chat runs on Cloud: give it its own machine, cloning `repo` (its
   * project's GitHub repository) into `/home/user/{name}`. Idempotent per
   * chat — a retried create returns the same session (and so the same host
   * device id for the chat row).
   */
  async createSession(
    caller: CloudCaller,
    chatId: string,
    spaceId: string,
    repo: RepoInput,
    edgeUrl: string,
    baseBranch?: string,
    parentChatId?: string
  ): Promise<CloudResult<CloudSessionView>> {
    const off = this.accountOf(caller);
    if (off) return off;
    if (caller.deviceId !== undefined && parentChatId === undefined) {
      return fail(403, "forbidden", "A Cloud machine starts only sessions for the chat it hosts.");
    }
    if (!ID_RE.test(chatId)) return fail(400, "bad_request", "Malformed chatId.");
    if (!ID_RE.test(spaceId)) return fail(400, "bad_request", "Malformed spaceId.");
    if (baseBranch !== undefined && !isBranch(baseBranch)) return fail(400, "bad_request", "Malformed branch.");
    const existing = this.sessions.get(chatId);
    if (existing) return this.okSession(existing);
    if (this.balance() <= 0) return this.outOfCredits();
    // Sleeping sessions keep their sandbox (storage on the provider) and a
    // record here: bounded, whoever is creating them.
    const live = [...this.sessions.values()].filter((x) => x.state !== "deleting").length;
    if (live >= this.maxSessions()) {
      return fail(429, "too_many_sessions", `You have ${live} Cloud chats. Delete some to start another.`);
    }
    const name = typeof repo?.fullName === "string" ? repo.fullName.split("/")[1] : undefined;
    const root = (this.env.CLOUD_PROJECTS_ROOT ?? CLOUD_PROJECTS_ROOT).replace(/\/+$/, "");
    const path = name ? `${root}/${name}` : "";
    if (
      !name ||
      !FULL_NAME_RE.test(repo.fullName) ||
      repo.fullName.split("/").some((part) => part === "." || part === "..") ||
      typeof repo.cloneUrl !== "string" ||
      !isRepoUrl(repo.cloneUrl) ||
      typeof repo.defaultBranch !== "string" ||
      !isBranch(repo.defaultBranch) ||
      !isCheckoutPath(path)
    ) {
      return fail(400, "bad_request", "Expected repo {fullName, cloneUrl (https), defaultBranch}.");
    }
    const providerName = activeProviderName(this.env);
    if (!this.workflowsBound() || !sandboxProvider(this.env, providerName)) {
      return fail(503, "unavailable", "Cloud is not available on this server.");
    }
    if (!(await this.makeRoomFor(chatId))) return this.tooManyAwake();
    if (this.sessions.has(chatId)) return this.okSession(this.sessions.get(chatId)!); // raced
    const now = Date.now();
    const s: SessionRecord = {
      chatId,
      spaceId,
      repo: { fullName: repo.fullName, cloneUrl: repo.cloneUrl, defaultBranch: repo.defaultBranch, path },
      ...(baseBranch !== undefined ? { baseBranch } : {}),
      deviceId: newCloudDeviceId(),
      createdAt: now,
      state: "provisioning",
      // Not 0: a session re-created for a chat id whose last one was just
      // deleted must not share generations with that one's stray workflows.
      generation: now,
      provider: providerName,
      activeRuns: 0,
      clients: 0,
      lastTtlExtendAt: now,
      ...(parentChatId !== undefined ? { parentChatId } : {})
    };
    this.sessions.set(chatId, s);
    this.byDevice.set(s.deviceId, chatId);
    // Before the laptop learns the device: its first send nudges this room
    // while the machine is still booting, and must queue, not 404.
    await this.claimDeviceRoom(s.deviceId, caller.userId);
    // Credits can run out across those awaits: a refused start leaves no
    // half-made session behind (unsaved, unworked, yet counted as awake).
    if (this.balance() <= 0) {
      // Only this record: a delete and re-create during the awaits put
      // another one there.
      if (this.sessions.get(chatId) === s) this.sessions.delete(chatId);
      this.byDevice.delete(s.deviceId);
      return this.outOfCredits();
    }
    return this.beginProvision(caller, s, edgeUrl);
  }

  /** A delete failed: it stays pending, and retries on its own with backoff. */
  private noteDeleteFailure(s: SessionRecord): void {
    s.pendingDelete = true;
    const attempts = (s.deleteAttempts ?? 0) + 1;
    s.deleteAttempts = attempts;
    s.deleteRetryAt = Date.now() + DELETE_RETRY_MS[Math.min(attempts, DELETE_RETRY_MS.length) - 1]!;
  }

  /** The account as caller, for deletes nobody is waiting on. */
  private systemCaller(): CloudCaller | undefined {
    const { orgId, userId } = this.acct;
    return orgId && userId ? { orgId, userId } : undefined;
  }

  /** Failed deletes whose retry is due, and deletes stuck mid-way (a lost
   * workflow): run them again. */
  private async retryDeletes(now: number): Promise<void> {
    const caller = this.systemCaller();
    if (!caller) return;
    for (const s of [...this.sessions.values()]) {
      const due = s.pendingDelete && s.deleteRetryAt !== undefined && now >= s.deleteRetryAt;
      const stuck = s.state === "deleting" && now - (s.deletingSince ?? 0) >= DELETE_STUCK_MS;
      if (due || stuck) {
        this.ledger("delete", s, { detail: stuck ? "resuming a stuck delete" : `retry ${s.deleteAttempts ?? 1}` });
        await this.beginDelete(caller, s);
      }
    }
  }

  /**
   * Machines whose chat is gone — deleted on any device, from any client,
   * even if the delete call that should have followed never arrived — are
   * deleted here. A session younger than the grace period may still be
   * waiting for its row. Registry trouble skips a pass, never deletes.
   */
  private async sweepOrphans(now: number): Promise<void> {
    const caller = this.systemCaller();
    const { orgId, userId } = this.acct;
    if (!caller || !orgId || !userId) return;
    const room = this.env.REGISTRY_ROOMS.get(this.env.REGISTRY_ROOMS.idFromName(`reg1/${orgId}/${userId}`));
    for (const s of [...this.sessions.values()]) {
      if (s.state === "deleting" || s.pendingDelete || now - s.createdAt < ORPHAN_GRACE_MS) continue;
      let host: unknown;
      try {
        const reply = await room.fetch(
          new Request(`https://registry/chat-host?chat=${encodeURIComponent(s.chatId)}`, {
            headers: { [AUTH_USER_HEADER]: userId }
          })
        );
        if (!reply.ok) throw new Error(`HTTP ${reply.status}`);
        host = ((await reply.json()) as { deviceId?: unknown }).deviceId;
      } catch (e) {
        console.warn("cloud: orphan sweep skipped; registry unavailable", String(e));
        return;
      }
      if (host === null || host === undefined) {
        this.ledger("delete", s, { detail: "its chat is gone" });
        await this.beginDelete(caller, s);
      }
    }
  }

  /**
   * A Cloud machine starts a delegate for the chat it hosts: a session of its
   * own on a machine of its own, on the parent's project and repository (a
   * machine never names another repository), from `branch` or the parent's
   * base. One level only — a delegate never starts delegates. The child's
   * chat row is written here, as the user: a machine may only write rows it
   * hosts, and the edge alone knows which machine it just made.
   */
  async createChildSession(
    caller: CloudCaller,
    chatId: string,
    parentChatId: string,
    edgeUrl: string,
    branch?: string,
    config?: Record<string, unknown>
  ): Promise<CloudResult<CloudSessionView>> {
    const off = this.accountOf(caller);
    if (off) return off;
    const parentOfCaller = caller.deviceId === undefined ? undefined : this.byDevice.get(caller.deviceId);
    if (parentOfCaller === undefined || parentOfCaller !== parentChatId) {
      return fail(403, "forbidden", "A Cloud machine starts sessions only for the chat it hosts.");
    }
    const parent = this.sessions.get(parentChatId);
    if (!parent) return fail(404, "unknown_session", "No Cloud session for the parent chat.");
    if (parent.parentChatId !== undefined) {
      return fail(403, "too_deep", "A delegated chat can't start chats of its own.");
    }
    const existing = this.sessions.get(chatId);
    if (existing && existing.parentChatId !== parentChatId) {
      return fail(409, "conflict", "That chat already has a Cloud session.");
    }
    // A new delegate takes a fresh chat id. Its row is written as the user,
    // so an existing (or deleted) chat's id would re-home that chat under
    // this machine and hand a stolen runner key its transcript.
    if (!existing) {
      const taken = await this.chatIdTaken(chatId);
      if (taken === undefined) return fail(503, "unavailable", "Couldn't check the chat id; try again.");
      if (taken) return fail(409, "conflict", "That chat id is already in use.");
    }
    const started = existing
      ? this.okSession(existing)
      : await this.createSession(
          caller,
          chatId,
          parent.spaceId,
          { fullName: parent.repo.fullName, cloneUrl: parent.repo.cloneUrl, defaultBranch: parent.repo.defaultBranch },
          edgeUrl,
          branch ?? parent.baseBranch,
          parentChatId
        );
    if (!started.ok) return started;
    const s = this.sessions.get(chatId);
    if (s) await this.writeChildChatRow(s, config, branch ?? parent.baseBranch);
    return started;
  }

  /** Whether `chatId` names a chat row in the user's registry, live or
   * deleted; `undefined` when the registry couldn't be read. */
  private async chatIdTaken(chatId: string): Promise<boolean | undefined> {
    const { orgId, userId } = this.acct;
    if (!orgId || !userId) return undefined;
    const room = this.env.REGISTRY_ROOMS.get(this.env.REGISTRY_ROOMS.idFromName(`reg1/${orgId}/${userId}`));
    try {
      const res = await room.fetch(
        new Request(`https://registry/chat-host?chat=${encodeURIComponent(chatId)}`, { headers: { [AUTH_USER_HEADER]: userId } })
      );
      if (!res.ok) return undefined;
      return ((await res.json()) as { known?: unknown }).known === true;
    } catch {
      return undefined;
    }
  }

  /** The delegate's chat row: hosted by its new machine, under its parent,
   * in the parent's project — only the fields known now (no nulls), so the
   * host's own later writes win field by field. Idempotent: an existing row
   * on this machine is left alone. */
  private async writeChildChatRow(s: SessionRecord, config: Record<string, unknown> | undefined, branch: string | undefined): Promise<void> {
    const { orgId, userId } = this.acct;
    if (!orgId || !userId || !s.parentChatId) return;
    const room = this.env.REGISTRY_ROOMS.get(this.env.REGISTRY_ROOMS.idFromName(`reg1/${orgId}/${userId}`));
    const headers = { [AUTH_USER_HEADER]: userId };
    const host = await room.fetch(new Request(`https://registry/chat-host?chat=${encodeURIComponent(s.chatId)}`, { headers }));
    if (!host.ok) {
      console.warn("cloud: reading a delegate's chat row failed", s.chatId, host.status);
      return;
    }
    const row = (await host.json()) as { deviceId?: unknown; known?: unknown };
    if (row.deviceId === s.deviceId) return;
    // Only ever the first write of a fresh id: never over another device's
    // chat, or a deleted one.
    if (row.known === true) {
      console.warn("cloud: not writing a delegate's row over an existing chat", s.chatId);
      return;
    }
    const set: Record<string, unknown> = {
      id: s.chatId,
      deviceId: s.deviceId,
      archived: false,
      cwd: s.repo.path,
      createdAt: Date.now(),
      spaceId: s.spaceId,
      parentChatId: s.parentChatId,
      roomGen: 2,
      ...(config ? { config } : {}),
      ...(branch ? { branch } : {})
    };
    const push = await room.fetch(
      new Request(`https://registry/push?device=${REGISTRY_WRITER}`, {
        method: "POST",
        headers: { ...headers, "content-type": "application/json" },
        body: JSON.stringify({
          batch: `cloud-child-${s.chatId}-${crypto.randomUUID()}`,
          ops: [{ kind: "chats", id: s.chatId, op: "upsert", set, hlc: encodeHlc(Date.now(), 0, REGISTRY_WRITER) }]
        })
      })
    );
    if (!push.ok) console.warn("cloud: writing a delegate's chat row failed", s.chatId, push.status);
  }

  /**
   * Tombstone every Cloud session device in the user's registry that no
   * session of this account owns any more — a deleted machine's engine wrote
   * its row and is gone for good (its chat keeps its row). Nothing lists,
   * polls or dials it again: the dial gate reads a deleted device as dark.
   * Runs after each delete; a failure retries on the alarm.
   */
  private async sweepStaleDevices(): Promise<void> {
    const { orgId, userId } = this.acct;
    if (!orgId || !userId) return;
    const room = this.env.REGISTRY_ROOMS.get(this.env.REGISTRY_ROOMS.idFromName(`reg1/${orgId}/${userId}`));
    const headers = { [AUTH_USER_HEADER]: userId };
    try {
      const res = await room.fetch(new Request("https://registry/rows?cursor=0", { headers }));
      if (!res.ok) throw new Error(`reading the registry: HTTP ${res.status}`);
      const { rows } = (await res.json()) as { rows: { kind: string; id: string; deleted: boolean; fields: Record<string, unknown> }[] };
      const stale = rows
        .filter((r) => r.kind === "devices" && !r.deleted && !this.byDevice.has(r.id))
        .filter((r) => Array.isArray(r.fields.capabilities) && r.fields.capabilities.includes(CLOUD_SESSION_CAPABILITY))
        .map((r) => r.id)
        .slice(0, 500); // one push batch; the next delete sweeps the rest
      if (stale.length > 0) {
        const hlc = encodeHlc(Date.now(), 0, REGISTRY_WRITER);
        const push = await room.fetch(
          new Request(`https://registry/push?device=${REGISTRY_WRITER}`, {
            method: "POST",
            headers: { ...headers, "content-type": "application/json" },
            body: JSON.stringify({
              batch: `cloud-sweep-${crypto.randomUUID()}`,
              ops: stale.map((id) => ({ kind: "devices", id, op: "delete", hlc }))
            })
          })
        );
        if (!push.ok) throw new Error(`removing ${stale.length} device rows: HTTP ${push.status}`);
      }
      this.acct.sweepDevicesAt = undefined;
    } catch (e) {
      console.warn("cloud: sweeping deleted sessions' devices failed; retrying", String(e));
      this.acct.sweepDevicesAt = Date.now() + SWEEP_RETRY_MS;
    }
  }

  /** Claim the session device's room for its user (see DeviceRoom `/claim`). */
  private async claimDeviceRoom(deviceId: string, userId: string): Promise<void> {
    const ns = this.env.DEVICE_ROOMS;
    try {
      const res = await ns.get(ns.idFromName(`d2/${deviceId}`)).fetch(
        new Request("https://device/claim", { method: "POST", headers: { [AUTH_USER_HEADER]: userId } })
      );
      if (!res.ok) console.warn("cloud: device room claim failed", deviceId, res.status);
    } catch (e) {
      console.warn("cloud: device room claim threw", deviceId, String(e));
    }
  }

  private async beginProvision(caller: CloudCaller, s: SessionRecord, edgeUrl: string): Promise<CloudResult<CloudSessionView>> {
    // Every way a machine boots (create, retry, a wake that re-provisions)
    // needs credits; a failed provision retried out of credits would
    // otherwise boot unpaid, and land in `error`, which no sleep reaches.
    if (this.balance() <= 0) return this.outOfCredits();
    const project = s.repo;
    const accountDeviceId = this.acct.deviceId;
    if (!project || !accountDeviceId) {
      return this.failNow(s, "enable", 409, "unknown_project", "This session has no repository to clone.");
    }
    const code = randomCode();
    const hash = await sha256B64url(code);
    const cur = this.sessions.get(s.chatId);
    if (cur !== s || (s.state !== "provisioning" && s.state !== "error")) return this.okSession(cur ?? s);
    s.state = "provisioning";
    s.awakeSince = Date.now();
    s.enrollHash = hash;
    s.runnerKey = undefined;
    s.lastTokenTs = undefined;
    s.awaitingOnline = false;
    s.error = undefined;
    s.lastTtlExtendAt = Date.now();
    this.register();
    const { deviceId, sandboxId } = s;
    const provider = s.provider ?? activeProviderName(this.env);
    const machine = s.pendingMachine ?? s.machine ?? this.acct.machine;
    return this.startWorkflow<ProvisionParams>(s, "provision", (generation) => ({
      orgId: caller.orgId,
      userId: caller.userId,
      chatId: s.chatId,
      deviceId,
      accountDeviceId,
      generation,
      provider,
      enrollCode: code,
      edgeUrl,
      project: { cloneUrl: project.cloneUrl, path: project.path, defaultBranch: s.baseBranch ?? project.defaultBranch },
      ...(sandboxId ? { sandboxId } : {}),
      // A retried session keeps its own size; a new one takes the account's.
      ...(machine ? { machine } : {})
    }));
  }

  private lifecycleParams(s: SessionRecord): Omit<LifecycleParams, "generation"> | undefined {
    const { orgId, userId } = this.acct;
    const { chatId, deviceId, provider, sandboxId } = s;
    return orgId && userId && provider && sandboxId ? { orgId, userId, chatId, deviceId, provider, sandboxId } : undefined;
  }

  private async beginWake(s: SessionRecord): Promise<CloudResult<CloudSessionView>> {
    const params = this.lifecycleParams(s);
    if (!params) {
      return this.failNow(s, "wake", 409, "no_sandbox", "This session has no machine to wake. Delete its machine and send again.");
    }
    if (this.balance() <= 0) return this.outOfCredits();
    if (!(await this.makeRoomFor(s.chatId))) return this.tooManyAwake();
    s.state = "starting";
    s.awakeSince = Date.now();
    s.awaitingOnline = true;
    // Not a refresh's: nobody is waiting on it, and it would read as a wake.
    s.wakeTrace = s.refreshing
      ? undefined
      : { requested: Date.now(), ...(s.stoppedAt === undefined ? {} : { asleepMs: Date.now() - s.stoppedAt }) };
    s.pendingWake = false;
    s.error = undefined;
    // It fits the size it's coming back on, or it wouldn't have got here.
    s.wakeBlocked = undefined;
    s.diskHydrating = true;
    s.lastTtlExtendAt = Date.now(); // resume sets the TTL afresh
    this.ledger("wake", s);
    this.scheduleReconcile();
    const machine = s.pendingMachine && s.pendingMachine !== this.currentMachine(s) ? s.pendingMachine : undefined;
    s.pendingMachine = machine;
    return this.startWorkflow<LifecycleParams>(s, "wake", (generation) => ({
      ...params,
      generation,
      ...(machine ? { machine } : {})
    }));
  }

  private async beginSleep(s: SessionRecord, reason: string, force = false): Promise<CloudResult<CloudSessionView>> {
    const params = this.lifecycleParams(s);
    if (!params) return this.okSession(s);
    s.state = "stopping";
    s.pendingWake = false;
    s.idleCheckAt = undefined;
    this.ledger("sleep", s, { detail: reason });
    return this.startWorkflow<LifecycleParams>(s, "sleep", (generation) => ({ ...params, generation, ...(force ? { force } : {}) }));
  }

  private session(caller: CloudCaller, chatId: string): SessionRecord | CloudResult<never> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const s = this.sessions.get(chatId);
    if (!s) return fail(404, "unknown_session", "No Cloud session for that chat.");
    // A machine manages only the sessions it started (its delegates).
    if (caller.deviceId !== undefined) {
      const parent = this.byDevice.get(caller.deviceId);
      if (parent === undefined || s.parentChatId !== parent) {
        return fail(403, "forbidden", "A Cloud machine manages only the sessions it started.");
      }
    }
    return s;
  }

  /**
   * `POST …/wake {machine?, wake?}`: bring it up, on `machine` when one is
   * given; `wake: false` only records the size for the next wake. A size its
   * disk doesn't fit is refused here with a typed 409 before anything starts,
   * as is any wake on the size that was just refused.
   */
  async wakeSession(
    caller: CloudCaller,
    chatId: string,
    options: { readonly machine?: SandboxMachine; readonly wake?: boolean } = {}
  ): Promise<CloudResult<CloudSessionView>> {
    const s = this.session(caller, chatId);
    if ("ok" in s) return s;
    const { machine } = options;
    // A size changes what the user pays: never a machine's call.
    if (machine && caller.deviceId !== undefined) {
      return fail(403, "forbidden", "A Cloud machine can't choose a machine size.");
    }
    if (s.pendingDelete || s.state === "deleting") return this.okSession(s);
    if (s.state === "data_expired") return fail(409, "data_expired", DATA_EXPIRED_MESSAGE);
    const target = machine ?? this.shownMachine(s);
    if (this.blockedOnDisk(s, target)) {
      const capacityGiB = machineSpec(target)?.diskGiB ?? 0;
      const neededGiB = s.wakeBlocked!.neededGiB;
      return fail(409, "machine_too_small", `This machine needs ${neededGiB} GiB of disk; a ${target} machine holds ${capacityGiB} GiB.`, {
        machine: target,
        neededGiB,
        capacityGiB
      });
    }
    if (machine && machine !== this.currentMachine(s) && AWAKE_STATES.has(s.state)) {
      return fail(409, "machine_awake", "A machine changes size while it sleeps. Put it to sleep first.");
    }
    if (machine) {
      s.pendingMachine = machine === this.currentMachine(s) ? undefined : machine;
      // No longer blocked on the old size: it clears on the wake, or here if
      // this only records the size.
      if (options.wake === false && s.wakeBlocked) {
        // A refused resume left it stopped, so it simply sleeps again.
        s.wakeBlocked = undefined;
        if (s.state === "error") {
          s.state = "sleeping";
          s.error = undefined;
          s.failedAction = undefined;
        }
      }
      await this.saveSession(s);
    }
    if (options.wake === false) return this.okSession(s);
    // Someone wants it: a refresh in flight doesn't put it back to sleep.
    s.refreshing = false;
    if (s.state === "sleeping" || (s.state === "error" && s.runnerKey)) return this.beginWake(s);
    if (s.state === "error" && !s.runnerKey && this.acct.state === "ready") {
      // Provisioning never finished: retry it on the sandbox it already made.
      return this.beginProvision(caller, s, this.env.CLOUD_EDGE_URL ?? "");
    }
    if (s.state === "stopping" && !s.pendingWake) {
      s.pendingWake = true;
      await this.saveSession(s);
    }
    return this.okSession(s);
  }

  /** Retry a failed provision with the edge origin of the request. */
  async retrySession(caller: CloudCaller, chatId: string, edgeUrl: string, machine?: SandboxMachine): Promise<CloudResult<CloudSessionView>> {
    const s = this.session(caller, chatId);
    if ("ok" in s) return s;
    if (s.state === "data_expired") {
      if (caller.deviceId !== undefined) return fail(403, "forbidden", "A Cloud machine can't start a fresh machine.");
      return this.beginFresh(caller, s, edgeUrl, machine);
    }
    if (s.state === "error" && !s.runnerKey && !s.pendingDelete) return this.beginProvision(caller, s, edgeUrl);
    return this.wakeSession(caller, chatId, machine ? { machine } : {});
  }

  /**
   * The provider deleted this machine's disk: a new machine on the same chat,
   * repository and branch. Same session device, so the chat keeps its host and
   * its queued messages; the engine enrolls a new key (a fresh disk has none)
   * and clones, and work it pushed to the session's branch comes back with it.
   * The dead sandbox is dropped, best effort: its meter stopped long ago.
   */
  private async beginFresh(
    caller: CloudCaller,
    s: SessionRecord,
    edgeUrl: string,
    machine?: SandboxMachine
  ): Promise<CloudResult<CloudSessionView>> {
    if (this.balance() <= 0) return this.outOfCredits();
    if (!(await this.makeRoomFor(s.chatId))) return this.tooManyAwake();
    if (s.state !== "data_expired") return this.okSession(s); // raced
    const provider = this.providerOf(s);
    const { sandboxId } = s;
    if (sandboxId && s.provider) {
      markSandboxDeleted(this.sql(), { provider: s.provider, sandboxId }, Date.now());
      this.ledger("delete", s, { detail: "its data had expired; starting a fresh machine" });
      if (provider) {
        this.ctx.waitUntil(
          provider.delete(sandboxId).catch((e: unknown) => console.warn("cloud: dropping an expired sandbox failed", sandboxId, explain(e)))
        );
      }
    }
    s.pendingMachine = machine ?? this.currentMachine(s);
    s.sandboxId = undefined;
    s.provider = activeProviderName(this.env);
    s.machine = undefined;
    s.disk = undefined;
    s.diskHydrating = false;
    s.stoppedAt = undefined;
    s.expiryCheckAt = undefined;
    s.state = "provisioning";
    s.error = undefined;
    return this.beginProvision(caller, s, edgeUrl);
  }

  async sleepSession(caller: CloudCaller, chatId: string): Promise<CloudResult<CloudSessionView>> {
    const s = this.session(caller, chatId);
    if ("ok" in s) return s;
    if (s.pendingDelete) return this.okSession(s);
    if (s.state === "ready" || (s.state === "error" && s.runnerKey)) return this.beginSleep(s, "user");
    if (s.pendingWake) {
      s.pendingWake = false;
      await this.saveSession(s);
    }
    return this.okSession(s);
  }

  async deleteSession(caller: CloudCaller, chatId: string): Promise<CloudResult<CloudSessionView>> {
    const s = this.session(caller, chatId);
    if ("ok" in s) return s;
    return this.beginDelete(caller, s);
  }

  /**
   * Explicit delete of one session's machine — the ONLY path that deletes a
   * sandbox. Synchronously: cut its runner off (key cleared, so its next
   * token refresh fails and the engine goes dark within one token lifetime),
   * stop whatever lifecycle workflow runs, revoke its vault device. Then the
   * DeleteWorkflow makes billing exact before anything irreversible:
   * stop → stopped → final usage read (`running: false`) → rows closed →
   * delete. A refused stop leaves `pendingDelete` plus an error, and a
   * retried delete resumes.
   */
  private async beginDelete(caller: CloudCaller, s: SessionRecord): Promise<CloudResult<CloudSessionView>> {
    if (s.state === "deleting" && Date.now() - (s.deletingSince ?? 0) < DELETE_STUCK_MS) return this.okSession(s);
    const { workflow } = s;
    const params = this.lifecycleParams(s);
    s.state = "deleting";
    s.deletingSince = Date.now();
    s.deleteRetryAt = undefined;
    s.generation++; // in-flight workflows' reports are void from here on
    s.runnerKey = undefined;
    s.enrollHash = undefined;
    s.awaitingOnline = false;
    s.pendingWake = false;
    s.pendingDelete = false;
    s.idleCheckAt = undefined;
    s.error = undefined;
    s.failedAction = undefined;
    await this.saveSession(s);

    // A provision still creating its sandbox is left to finish: told it was
    // superseded, it deletes the sandbox it made. Terminated mid-create, the
    // provider may already have made one nobody would ever record.
    const creating = workflow?.kind === "provision" && !params;
    if (workflow && !creating) {
      try {
        const instance = await this.workflowBinding(workflow.kind)?.get(workflow.id);
        const { status } = (await instance?.status()) ?? { status: "complete" };
        if (!["complete", "errored", "terminated"].includes(status)) await instance?.terminate();
      } catch (e) {
        // Its reports are void anyway (generation bumped above).
        console.warn("cloud delete: terminate failed", workflow.id, String(e));
      }
    }
    if (this.env.VAULT) {
      try {
        const res = await this.env.VAULT.revokeDevice({ userId: caller.userId, orgId: caller.orgId, kind: "user" }, s.deviceId);
        if (!res.ok && res.error !== "not_found" && res.error !== "device_unknown") {
          console.warn("cloud delete: vault revoke failed", s.deviceId, res.error, res.message);
        }
      } catch (e) {
        console.warn("cloud delete: vault revoke threw", s.deviceId, String(e));
      }
    }
    if (!params) {
      await this.finishSessionDelete(s);
      return { ok: true, value: { ...this.sessionView(s), state: "off" } };
    }
    return this.startWorkflow<LifecycleParams>(s, "delete", (generation) => ({ ...params, generation }));
  }

  /** The session is gone; its metering rows stay (its month is still billed). */
  private async finishSessionDelete(s: SessionRecord): Promise<void> {
    await this.dropSession(s);
    await this.sweepStaleDevices();
    const sql = this.sql();
    const now = Date.now();
    this.acct.nextReconcileAt = meteringOpen(listSandboxes(sql), listUsage(sql), now)
      ? Math.min(this.acct.nextReconcileAt ?? Infinity, now + RECONCILE_EVERY_MS)
      : this.acct.nextReconcileAt;
    this.register();
    await this.saveAccount();
    await this.maybeFinishAccountDelete();
  }

  /**
   * `/device/{deviceId}/nudge` from the user (a send to a sleeping session):
   * wake that session's machine. Viewing never wakes — only sends do. Also
   * catches a sandbox the provider auto-stopped behind our back (trial TTL):
   * ready-but-silent gets one cheap state probe per minute. A session in
   * `error` is recovered too: a send is the user asking for it, and a machine
   * that merely failed to sleep or wake once mustn't need a trip to Settings.
   */
  async autoWake(caller: CloudCaller, deviceId: string, edgeUrl?: string): Promise<boolean> {
    const chatId = this.byDevice.get(deviceId);
    const s = chatId ? this.sessions.get(chatId) : undefined;
    if (!s || !this.matches(caller) || s.pendingDelete || s.state === "deleting") return false;
    // A machine wakes only the sessions it started (its delegates).
    if (caller.deviceId !== undefined && s.parentChatId !== this.byDevice.get(caller.deviceId)) return false;
    if (s.state === "ready") {
      // A send to a machine that's up. If it goes dark before its next
      // heartbeat, that heartbeat never reports the turn this started.
      const now = Date.now();
      s.lastNudgeAt = now;
      s.idleCheckAt = Math.min(s.idleCheckAt ?? Infinity, now + HEARTBEAT_STALE_MS);
      await this.saveSession(s);
    }
    return this.wake(s, edgeUrl);
  }

  /** Bring `s`'s machine back: from sleep, after a stop in progress, or when
   * the provider stopped it behind our back (auto-stop). */
  private async wake(s: SessionRecord, edgeUrl = this.env.CLOUD_EDGE_URL): Promise<boolean> {
    s.refreshing = false; // someone wants it: it stays up
    if (this.blockedOnDisk(s)) return false; // the user picks a size; a retry gets the same answer
    if (s.state === "sleeping") return (await this.beginWake(s)).ok;
    if (s.state === "error") {
      if (s.runnerKey) return (await this.beginWake(s)).ok;
      // It never finished provisioning: on the sandbox it made, if any.
      const caller = this.systemCaller();
      return caller && edgeUrl ? (await this.beginProvision(caller, s, edgeUrl)).ok : false;
    }
    if (s.state === "stopping") {
      if (!s.pendingWake) {
        s.pendingWake = true;
        await this.saveSession(s);
      }
      return true;
    }
    const now = Date.now();
    const silentSince = s.lastHeartbeatAt ?? s.lastActiveAt ?? now;
    if (s.state !== "ready" || !s.sandboxId || now - silentSince < HEARTBEAT_STALE_MS) return false;
    if (now - (s.lastProbeAt ?? 0) < 60_000) return false;
    s.lastProbeAt = now;
    await this.saveSession(s);
    const provider = this.providerOf(s);
    if (!provider) return false;
    try {
      const sandbox = await provider.get(s.sandboxId);
      if (sandbox.state !== "stopped" || s.state !== "ready") return false;
      s.state = "sleeping";
      s.awakeSince = undefined;
      this.noteStopped(s, sandbox.stoppedAt ?? now);
      this.ledger("stop", s, { detail: "stopped by the provider (auto-stop)" });
      this.scheduleReconcile();
      return (await this.beginWake(s)).ok;
    } catch (e) {
      console.warn("cloud auto-wake probe failed", explain(e));
      return false;
    }
  }

  // ── runner identity (pre-bearer /runner/enroll, /runner/token) ───────────

  private sessionByDevice(deviceId: string): SessionRecord | undefined {
    const chatId = this.byDevice.get(deviceId);
    return chatId ? this.sessions.get(chatId) : undefined;
  }

  async enroll(req: EnrollRequest): Promise<CloudResult<{ ok: true }>> {
    const s = this.sessionByDevice(req.deviceId);
    const live = s && s.state !== "off" && s.state !== "deleting" && !s.pendingDelete;
    if (!s || !live || !this.matches(req) || !this.acct.deviceId) {
      return fail(404, "unknown_device", "Unknown Cloud device.");
    }
    if (!parseEd25519PublicKey(req.publicKey)) {
      return fail(400, "bad_request", "publicKey must be a raw 32-byte Ed25519 key, base64url.");
    }
    const okReply = { ok: true as const, value: { ok: true as const } };
    // Same key again = the engine retrying after a lost reply.
    if (!s.enrollHash) {
      return s.runnerKey === req.publicKey ? okReply : fail(409, "code_used", "Enrollment code already used.");
    }
    const hash = await sha256B64url(req.code);
    if (!s.enrollHash || !constantTimeEqual(hash, s.enrollHash)) {
      return s.runnerKey === req.publicKey ? okReply : fail(403, "bad_code", "Invalid enrollment code.");
    }
    // Vault first: on failure the code stays valid so the engine can retry.
    // The session device enrolls under the account's logical device, so the
    // providers the user connected "for Cloud" cover it.
    if (this.env.VAULT) {
      try {
        const res = await this.env.VAULT.enrollDevice(
          { userId: req.userId, orgId: req.orgId, kind: "user" },
          { deviceId: req.deviceId, kind: "cloud", publicKey: req.publicKey, parentId: this.acct.deviceId }
        );
        if (!res.ok) {
          return fail(res.status >= 500 ? 503 : 502, `vault_${res.error}`, `Vault enrollment failed: ${res.message}`);
        }
      } catch (e) {
        console.warn("cloud enroll: vault unreachable", String(e));
        return fail(503, "vault_unavailable", "The credential vault is unavailable; retry.");
      }
    }
    const cur = this.sessionByDevice(req.deviceId);
    if (cur !== s || s.enrollHash !== hash) {
      return s.runnerKey === req.publicKey ? okReply : fail(409, "code_used", "Enrollment code already used.");
    }
    const now = Date.now();
    s.runnerKey = req.publicKey;
    s.enrollHash = undefined;
    s.enrolledAt = now;
    s.lastTokenTs = undefined;
    // The provision workflow gave up waiting but the engine made it after
    // all: it is up, so it is ready.
    if (s.state === "error") this.markReady(s, now);
    else this.sendEvent(s, "provision", "enrolled");
    await this.saveSession(s);
    await this.saveAccount();
    return okReply;
  }

  async token(req: TokenRequest): Promise<CloudResult<RunnerToken>> {
    const s = this.sessionByDevice(req.deviceId);
    const key = s?.runnerKey;
    if (!s || !key || !this.matches(req) || s.state === "deleting" || !this.acct.deviceId) {
      return fail(403, "revoked", "This Cloud device is not enrolled.");
    }
    const now = Date.now();
    const pre = tokenTimestampCheck(req.ts, s.lastTokenTs, now);
    if (pre !== "ok") return fail(401, pre, pre === "stale" ? "Timestamp outside ±120 s." : "Timestamp replayed.");
    if (!(await verifyEd25519(key, runnerTokenMessage(req.orgId, req.userId, req.deviceId, req.ts), req.sig))) {
      return fail(401, "bad_signature", "Bad signature.");
    }
    if (this.sessionByDevice(req.deviceId) !== s || s.runnerKey !== key) {
      return fail(403, "revoked", "This Cloud device is not enrolled.");
    }
    // Re-check after the await: a concurrent request with the same ts may
    // have been accepted while we verified.
    if (tokenTimestampCheck(req.ts, s.lastTokenTs, now) !== "ok") return fail(401, "replay", "Timestamp replayed.");
    s.lastTokenTs = req.ts;
    this.markAlive(s, now);
    await this.saveSession(s);
    await this.saveAccount();
    const token = await mintRunnerToken(
      this.env,
      { userId: req.userId, orgId: req.orgId, deviceId: req.deviceId, accountDeviceId: this.acct.deviceId },
      now
    );
    return token ? { ok: true, value: token } : fail(503, "unavailable", "Runner tokens are not configured.");
  }

  /** The repository a Cloud machine's chat works in: a runner's GitHub
   * grants are bound to it (a stolen runner key gets nothing else). */
  async repoOfDevice(caller: CloudCaller, deviceId: string): Promise<string | undefined> {
    if (!this.matches(caller)) return undefined;
    return this.sessionByDevice(deviceId)?.repo.fullName;
  }

  /** `POST /runner/wake-timing` (runner bearer, already verified): the engine's
   * side of the wake that started it. Counts once per wake, whatever a runner
   * sends: the first report closes the trace. */
  async wakeTimingReport(runner: CloudCaller & { deviceId: string }, engine: EngineTiming): Promise<CloudResult<{ ok: true }>> {
    const s = this.sessionByDevice(runner.deviceId);
    if (!s || !s.runnerKey || !this.matches(runner)) {
      return fail(403, "revoked", "This Cloud device is not enrolled.");
    }
    // It's answering: whatever it was doing first is over.
    s.wakeStage = undefined;
    const trace = s.wakeTrace;
    if (trace && !trace.engine) {
      if (trace.stages) {
        s.wakeTrace = undefined;
        this.recordWakeTiming(s, trace, engine);
      } else {
        // Ahead of the workflow's report: written with it, in one entry.
        trace.engine = engine;
      }
      await this.saveSession(s);
    }
    return { ok: true, value: { ok: true } };
  }

  /** `POST /runner/wake-stage` (runner bearer, already verified). */
  async wakeStage(runner: CloudCaller & { deviceId: string }, stage: WakeStage | null): Promise<CloudResult<{ ok: true }>> {
    const s = this.sessionByDevice(runner.deviceId);
    if (!s || !s.runnerKey || !this.matches(runner)) {
      return fail(403, "revoked", "This Cloud device is not enrolled.");
    }
    s.wakeStage = stage ? { stage, at: Date.now() } : undefined;
    await this.saveSession(s);
    return { ok: true, value: { ok: true } };
  }

  private recordWakeTiming(s: SessionRecord, trace: WakeTrace, engine?: EngineTiming): void {
    const timing = wakeTiming(trace, engine);
    this.ledger("wake_timing", s, { detail: JSON.stringify(timing) });
    console.info(JSON.stringify({ event: "cloud.wake_timing", chatId: s.chatId, complete: engine !== undefined, ...timing }));
  }

  /** The machine of the session that started this one, for a delegate: it
   * works as the same GitHub login. */
  async parentDeviceOf(caller: CloudCaller, deviceId: string): Promise<string | undefined> {
    if (!this.matches(caller)) return undefined;
    const parent = this.sessionByDevice(deviceId)?.parentChatId;
    return parent === undefined ? undefined : this.sessions.get(parent)?.deviceId;
  }

  /** `POST /runner/heartbeat` (runner bearer, already verified). */
  async heartbeat(
    runner: CloudCaller & { deviceId: string },
    beat: Heartbeat
  ): Promise<CloudResult<{ ok: true; diskCapacityGiB?: number }>> {
    const s = this.sessionByDevice(runner.deviceId);
    if (!s || !s.runnerKey || !this.matches(runner)) {
      return fail(403, "revoked", "This Cloud device is not enrolled.");
    }
    const now = Date.now();
    const backgroundRuns = beat.backgroundRuns ?? 0;
    const waitingRuns = beat.waitingRuns ?? 0;
    const wasWorking = s.activeRuns - (s.waitingRuns ?? 0) > 0 || (s.backgroundRuns ?? 0) > 0;
    const working = beat.activeRuns - waitingRuns > 0 || backgroundRuns > 0;
    const changed =
      s.activeRuns !== beat.activeRuns ||
      s.clients !== beat.clients ||
      (s.backgroundRuns ?? 0) !== backgroundRuns ||
      (s.waitingRuns ?? 0) !== waitingRuns;
    const busy = beat.activeRuns > 0 || beat.clients > 0 || backgroundRuns > 0;
    s.activeRuns = beat.activeRuns;
    s.clients = beat.clients;
    s.backgroundRuns = backgroundRuns;
    s.waitingRuns = waitingRuns;
    s.beatEveryMs = beat.everySecs === undefined ? undefined : beat.everySecs * 1000;
    if (beat.storeRecoveredAt !== undefined && beat.storeRecoveredAt !== s.storeRecoveredAt) {
      s.storeRecoveredAt = beat.storeRecoveredAt;
      this.ledger("store_recovered", s, { detail: new Date(beat.storeRecoveredAt).toISOString() });
    }
    s.lastHeartbeatAt = now;
    if (busy || changed) s.lastActiveAt = now;
    this.markAlive(s, now);
    const provider = this.providerOf(s);
    // What's judged is what the size's limit counts, never the base image.
    const diskOverheadGiB = provider?.capabilities.diskOverheadGiB ?? 0;
    if (beat.diskUsedBytes !== undefined) this.noteDisk(s, Math.max(0, beat.diskUsedBytes / GIB - diskOverheadGiB), now);
    if (s.state === "ready") {
      s.idleCheckAt = (s.lastActiveAt ?? now) + this.idleMs();
      // A turn in flight is looked at as soon as its heartbeats would be late,
      // not a whole idle window on: a chat is waiting on that machine.
      if (working) s.idleCheckAt = Math.min(s.idleCheckAt, now + heartbeatStaleMs(s.beatEveryMs));
    }
    const ttlSeconds = provider ? effectiveTtl(provider.capabilities, parseTtlSeconds(this.env.CLOUD_TTL_SECONDS)) : null;
    // Out of credits, a busy machine is not kept alive past its TTL.
    if (provider && s.sandboxId && this.balance() > 0 && shouldExtendTtl({ ttlSeconds, busy, lastExtendedAt: s.lastTtlExtendAt, now })) {
      // A provider account that forces auto-stop (Boat trial) would stop a
      // busy session mid-run: keep pushing it out. Fire-and-forget; the next
      // beat retries.
      s.lastTtlExtendAt = now;
      this.ledger("ttl_extend", s, { detail: String(ttlSeconds) });
      const sandboxId = s.sandboxId;
      this.ctx.waitUntil(
        provider.extendTtl(sandboxId, ttlSeconds).catch((e: unknown) => console.warn("cloud ttl extend failed", explain(e)))
      );
    }
    await this.saveSession(s);
    await this.saveAccount();
    // A delegate that just stopped working (finished, or waiting on an
    // answer) has something for the chat that started it, which reads it
    // from its own machine: wake that machine if it went to sleep meanwhile.
    if (wasWorking && !working && s.parentChatId !== undefined) {
      const parent = this.sessions.get(s.parentChatId);
      if (parent && !parent.pendingDelete && parent.state !== "deleting") {
        await this.wake(parent).catch((e: unknown) => console.warn("cloud parent wake failed", explain(e)));
      }
    }
    // What its pre-sleep cleanup measures its own `df` against: what this size
    // holds, as `df` would read it with the base image in.
    const holds = machineSpec(this.currentMachine(s))?.diskGiB;
    const diskCapacityGiB = holds === undefined ? undefined : holds + diskOverheadGiB;
    return { ok: true, value: { ok: true, ...(diskCapacityGiB === undefined ? {} : { diskCapacityGiB }) } };
  }

  /**
   * A machine's disk reading, kept as a high-water mark. A resumed disk copies
   * its files in lazily, so `df` under-reads until that's done (seen: 26 GiB
   * right after a resume Boat said needed 118.9): a higher reading always
   * counts, a lower one only once the provider says every file is there.
   */
  private noteDisk(s: SessionRecord, usedGiB: number, now: number): void {
    if (!s.disk || usedGiB >= s.disk.usedGiB || !s.diskHydrating) {
      s.disk = { usedGiB, at: now };
      return;
    }
    if (now - (s.lastHydrationProbeAt ?? 0) < HYDRATION_PROBE_MS) return;
    s.lastHydrationProbeAt = now;
    const provider = this.providerOf(s);
    const sandboxId = s.sandboxId;
    if (!provider || !sandboxId) return;
    this.ctx.waitUntil(
      provider
        .get(sandboxId)
        .then(async (info) => {
          // A provider that doesn't say restores nothing lazily.
          if (info.hydrated === false || this.sessions.get(s.chatId) !== s || s.sandboxId !== sandboxId) return;
          s.diskHydrating = false;
          await this.saveSession(s);
        })
        .catch((e: unknown) => console.warn("cloud: reading a machine's hydration failed", explain(e)))
    );
  }

  /** The retention clock starts: `at` is when the sandbox stopped. */
  private noteStopped(s: SessionRecord, at: number): void {
    s.stoppedAt = at;
    const retention = this.retentionMs(s);
    s.expiryCheckAt = retention === undefined ? undefined : refreshDueAt(at, retention);
  }

  private markDataExpired(s: SessionRecord): void {
    s.state = "data_expired";
    s.error = DATA_EXPIRED_MESSAGE;
    s.failedAction = undefined;
    s.refreshing = false;
    s.pendingWake = false;
    s.awaitingOnline = false;
    s.awakeSince = undefined;
    s.wakeBlocked = undefined;
    s.expiryCheckAt = undefined;
    this.ledger("error", s, { detail: "its data expired" });
  }

  /**
   * A sleeping machine's retention clock. A resume restarts it, for the machine
   * seconds that takes, so a machine due for one comes up and goes straight back
   * to sleep. Skipped while refreshing can't work (no credits, or a disk that
   * doesn't fit its size: the user's to fix, and `expiresAt` says by when).
   * Returns whether it started a refresh.
   */
  private async checkExpiry(s: SessionRecord, now: number): Promise<boolean> {
    const provider = this.providerOf(s);
    const retention = this.retentionMs(s);
    if (!provider || retention === undefined || !s.sandboxId) {
      s.expiryCheckAt = undefined;
      return false;
    }
    let info;
    try {
      info = await provider.get(s.sandboxId);
    } catch (e) {
      console.warn("cloud: reading a sleeping machine failed", explain(e));
      s.expiryCheckAt = now + EXPIRY_RETRY_MS;
      return false;
    }
    if (!this.awaitingExpiry(s)) return false; // it woke meanwhile
    if (info.failure?.kind === "data_expired") {
      this.markDataExpired(s);
      return false;
    }
    // Running, or in an error of its own: no clock to refresh yet.
    if (info.state !== "stopped") {
      s.expiryCheckAt = now + EXPIRY_RETRY_MS;
      return false;
    }
    // The provider's clock wins; a session that predates ours has only this.
    s.stoppedAt = info.stoppedAt ?? s.stoppedAt ?? s.lastActiveAt ?? now;
    const due = refreshDueAt(s.stoppedAt, retention);
    if (now < due) {
      s.expiryCheckAt = due;
      return false;
    }
    // No engine ever enrolled (a failed provision) has nothing worth keeping,
    // and a wake would wait out its full timeout for an engine that isn't there.
    if (!s.runnerKey || this.blockedOnDisk(s) || this.balance(now) <= 0) {
      s.expiryCheckAt = now + EXPIRY_RETRY_MS;
      return false;
    }
    // Never evicts anyone: a refresh waits for a free slot.
    if (this.awakeCount() >= this.maxAwake()) {
      s.expiryCheckAt = now + EXPIRY_NO_ROOM_MS;
      return false;
    }
    s.refreshing = true;
    s.expiryCheckAt = now + EXPIRY_RETRY_MS;
    this.ledger("refresh", s, { detail: `stopped ${new Date(s.stoppedAt).toISOString()}` });
    const started = await this.beginWake(s);
    if (!started.ok) s.refreshing = false;
    return started.ok;
  }

  /** A refresh failed: back to sleep if the machine is up, and try again. */
  private async settleFailedRefresh(s: SessionRecord, message: string): Promise<void> {
    const now = Date.now();
    s.refreshing = false;
    s.awaitingOnline = false;
    s.expiryCheckAt = now + EXPIRY_RETRY_MS;
    this.ledger("error", s, { detail: `refresh failed: ${message}` });
    const provider = this.providerOf(s);
    if (provider && s.sandboxId) {
      try {
        if ((await provider.get(s.sandboxId)).state === "stopped") {
          s.state = "sleeping";
          s.awakeSince = undefined;
          s.error = undefined;
          s.failedAction = undefined;
          return;
        }
      } catch (e) {
        console.warn("cloud: reading a machine after a failed refresh failed", explain(e));
      }
    }
    // Possibly up and billing, with nobody who asked for it.
    await this.beginSleep(s, "a refresh failed", true);
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    await this.retryDeletes(now);
    if (this.acct.nextOrphanSweepAt !== undefined && now >= this.acct.nextOrphanSweepAt) {
      this.acct.nextOrphanSweepAt = now + ORPHAN_SWEEP_MS;
      await this.sweepOrphans(now);
    }
    for (const s of [...this.sessions.values()]) {
      if (s.state !== "ready" || s.idleCheckAt === undefined || now < s.idleCheckAt) continue;
      // It went dark with a turn in flight (the machine died, or its engine
      // hung): nobody may send again for days, and the turn is stuck until
      // something boots it. Sleep it and bring it straight back, so its
      // engine's boot recovery resumes the turn or says why it can't.
      const dark = s.lastHeartbeatAt !== undefined && now - s.lastHeartbeatAt >= heartbeatStaleMs(s.beatEveryMs);
      const working =
        s.activeRuns - (s.waitingRuns ?? 0) > 0 ||
        (s.backgroundRuns ?? 0) > 0 ||
        (s.lastNudgeAt ?? 0) > (s.lastHeartbeatAt ?? 0);
      const decision =
        dark && working
          ? { sleep: true as const }
          : idleDecision({
              now,
              idleMs: this.idleMs(),
              lastActiveAt: s.lastActiveAt,
              lastHeartbeatAt: s.lastHeartbeatAt,
              activeRuns: s.activeRuns + (s.backgroundRuns ?? 0),
              clients: s.clients
            });
      if (decision.sleep) {
        const sleeping = await this.beginSleep(s, dark && working ? "went dark during a turn; recovering it" : "idle");
        if (dark && working && sleeping.ok) {
          s.pendingWake = true;
          await this.saveSession(s);
        }
      } else {
        s.idleCheckAt = decision.checkAt;
        await this.saveSession(s);
      }
    }
    let refreshes = 0;
    for (const s of [...this.sessions.values()]) {
      if (!this.awaitingExpiry(s) || now < (s.expiryCheckAt ?? 0)) continue;
      if (refreshes >= REFRESHES_PER_PASS) {
        s.expiryCheckAt = now + EXPIRY_NO_ROOM_MS;
        await this.saveSession(s);
        continue;
      }
      if (await this.checkExpiry(s, now)) refreshes++;
      await this.saveSession(s);
    }
    if (this.acct.sweepDevicesAt !== undefined && now >= this.acct.sweepDevicesAt) {
      await this.sweepStaleDevices();
    }
    if (this.acct.nextReconcileAt !== undefined && now >= this.acct.nextReconcileAt) {
      try {
        await this.reconcile(now);
      } catch (e) {
        console.error("cloud reconcile failed", String(e));
        this.acct.nextReconcileAt = now + RECONCILE_EVERY_MS;
      }
    }
    await this.saveAccount();
  }

  // ── metering ──────────────────────────────────────────────────────────────

  private async readUsage(task: ReconcileTask): Promise<SandboxUsage> {
    const provider = sandboxProvider(this.env, task.provider);
    if (!provider) {
      throw new SandboxError("config", `provider ${task.provider} is not configured`, "unconfigured", task.provider);
    }
    return provider.usage(task.sandboxId, { since: task.since, until: task.until });
  }

  /**
   * Pull each provider's figure for open (day, sandbox) windows: every one
   * hourly, and in between only awake machines' (their credits are being
   * spent). Out of credits, every awake machine is put to sleep. Returns
   * whether every read succeeded. Never throws for a provider failure: the
   * row keeps its last figure plus `reconcileError`, and the next pass
   * retries. `now` is a parameter for the day-boundary tests.
   */
  async reconcile(
    now = Date.now(),
    full = now - (this.acct.lastFullReconcileAt ?? 0) >= RECONCILE_EVERY_MS
  ): Promise<boolean> {
    if (!this.acct.orgId) return true; // never enabled: nothing to meter
    const sql = this.sql();
    const awake = new Set(
      [...this.sessions.values()]
        .filter((s) => AWAKE_STATES.has(s.state) || s.state === "stopping")
        .flatMap((s) => (s.sandboxId ? [`${s.provider}/${s.sandboxId}`] : []))
    );
    let allOk = true;
    for (const task of planReconcile(listSandboxes(sql), listUsage(sql), now)) {
      if (!full && !awake.has(`${task.provider}/${task.sandboxId}`)) continue;
      if (!task.fetch) {
        closeUsageRow(sql, task.day, task);
        continue;
      }
      try {
        const usage = await this.readUsage(task);
        upsertUsage(sql, {
          day: task.day,
          provider: task.provider,
          sandboxId: task.sandboxId,
          sandboxType: usage.machine,
          seconds: usage.seconds,
          dollars: usage.dollars,
          running: usage.running,
          reconciledAt: now,
          closed: task.close
        });
      } catch (e) {
        allOk = false;
        // A sandbox the provider no longer knows can never be re-read: once
        // its day is over, close the row on its last figure (flagged).
        const gone = e instanceof SandboxError && e.kind === "notFound";
        recordUsageFailure(sql, task, explain(e), gone && task.close);
      }
    }
    if (allOk) {
      this.acct.lastReconcileAt = now;
      if (full) this.acct.lastFullReconcileAt = now;
    }
    if (this.balance(now) <= 0) {
      for (const s of [...this.sessions.values()]) {
        // Forced: a busy machine can't decline this one and run on unpaid.
        if (s.state === "ready" && !s.pendingDelete) await this.beginSleep(s, "out of credits", true);
      }
    }
    this.acct.nextReconcileAt = this.nextReconcile(sql, now);
    this.register();
    await this.saveAccount();
    return allOk;
  }

  /** `GET /cloud/{orgId}/usage?from=&to=` — the caller's credits: balance,
   * and use per UTC day over `[from, to]`. */
  usage(caller: CloudCaller, from: string, to: string, now = Date.now()): CloudResult<CloudUsage> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const rows = this.acct.orgId ? listUsage(this.sql(), { from, to }) : [];
    const days = creditsPerDay(from, to, rows, this.rate());
    return {
      ok: true,
      value: {
        from,
        to,
        days,
        credits: Math.round(days.reduce((sum, d) => sum + d.credits, 0) * 100) / 100,
        balance: Math.floor(this.balance(now) * 100) / 100,
        available: true
      }
    };
  }

  /** Operator: the machine size this account's new sessions start on
   * (`POST /admin/cloud/machine`; undefined = the server default). Sessions
   * that already have a machine keep it. */
  async setMachine(machine: SandboxMachine | undefined): Promise<{ readonly machine: SandboxMachine | null }> {
    if (machine) this.acct.machine = machine;
    else delete this.acct.machine;
    await this.saveAccount();
    return { machine: this.acct.machine ?? null };
  }

  /** Operator: this account's caps — machines awake at once and Cloud chats
   * kept (`POST /admin/cloud/limits`; undefined = the server default). */
  async setLimits(limits: {
    readonly maxAwake?: number | null;
    readonly maxSessions?: number | null;
  }): Promise<{ readonly maxAwake: number; readonly maxSessions: number }> {
    if (limits.maxAwake === null) delete this.acct.maxAwake;
    else if (limits.maxAwake !== undefined) this.acct.maxAwake = limits.maxAwake;
    if (limits.maxSessions === null) delete this.acct.maxSessions;
    else if (limits.maxSessions !== undefined) this.acct.maxSessions = limits.maxSessions;
    await this.saveAccount();
    return { maxAwake: this.maxAwake(), maxSessions: this.maxSessions() };
  }

  /** Operator grant (or correction, when negative): `POST /admin/cloud/credits`. */
  async grantCredits(credits: number, reason: string): Promise<{ readonly balance: number }> {
    addCredits(this.sql(), { at: Date.now(), credits, reason });
    return { balance: Math.floor(this.balance() * 100) / 100 };
  }

  /** Operator export row (billing.ts). */
  billingUsage(month: string, now = Date.now()): CloudBillingRow {
    return this.billing(month, now);
  }

  private billing(month: string, now: number): CloudBillingRow {
    const summary = this.acct.orgId
      ? summarizeMonth(month, listSandboxes(this.sql()), listUsage(this.sql()), now)
      : summarizeMonth(month, [], [], now);
    return {
      ...summary,
      ...(this.acct.orgId ? { orgId: this.acct.orgId } : {}),
      ...(this.acct.userId ? { userId: this.acct.userId } : {}),
      ...(this.acct.deviceId ? { deviceId: this.acct.deviceId } : {})
    };
  }

  /** Lifecycle ledger, newest last (operator/debug). */
  ledgerEntries(limit = 500): LedgerEntry[] {
    return this.acct.orgId ? listLedger(this.sql(), limit) : [];
  }

  /** Recent wakes' timelines, newest last (`GET /admin/cloud/wakes`). */
  wakeTimings(limit = 200): LedgerEntry[] {
    return this.ledgerEntries(limit * 10)
      .filter((entry) => entry.event === "wake_timing")
      .slice(-limit);
  }

  // ── billing (cloud/stripe.ts) ─────────────────────────────────────────────

  private subscription(): SubscriptionState | undefined {
    return this.acct.orgId ? readBillingState<SubscriptionState>(this.sql(), "subscription") : undefined;
  }

  private customer(): string | undefined {
    return this.acct.orgId ? readBillingState<string>(this.sql(), "customer") : undefined;
  }

  /** `GET /cloud/{org}/billing` — plan, credits and what can be bought. */
  billingStatus(caller: CloudCaller, available: boolean, now = Date.now()): CloudResult<BillingView> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const subscription = this.subscription();
    const plan = subscription && SUBSCRIBED.has(subscription.status) ? planById(subscription.plan) : undefined;
    const credits = this.acct.orgId ? this.breakdown(now) : { monthly: 0, monthlyTotal: 0, extra: 0 };
    const total = Math.floor(this.balance(now) * 100) / 100;
    return {
      ok: true,
      value: {
        available,
        ...(plan && subscription
          ? {
              plan: {
                ...plan,
                status: subscription.status,
                periodEnd: subscription.periodEnd,
                cancelAtPeriodEnd: subscription.cancelAtPeriodEnd
              }
            }
          : {}),
        credits: { ...credits, total },
        plans: PLANS,
        pack: CREDIT_PACK,
        hasCustomer: this.customer() !== undefined
      }
    };
  }

  /** A Stripe Checkout page for a plan or credit packs. */
  async checkout(caller: CloudCaller, request: CheckoutRequest, returnUrl: string): Promise<CloudResult<{ url: string }>> {
    if (!this.bind(caller)) return fail(403, "forbidden", "Not your Cloud.");
    await this.saveAccount();
    const subscription = this.subscription();
    if (request.kind === "subscription" && subscription && SUBSCRIBED.has(subscription.status)) {
      return fail(409, "already_subscribed", "You already have a plan. Change it from Manage subscription.");
    }
    try {
      return { ok: true, value: { url: await createCheckout(this.env as StripeEnv, caller, request, returnUrl, this.customer()) } };
    } catch (e) {
      return stripeFailure(e);
    }
  }

  /** The Stripe Billing Portal for the user's customer. */
  async portal(caller: CloudCaller, returnUrl: string): Promise<CloudResult<{ url: string }>> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const customer = this.customer();
    if (!customer) return fail(409, "no_customer", "Subscribe or buy credits first.");
    try {
      return { ok: true, value: { url: await createPortal(this.env as StripeEnv, customer, returnUrl) } };
    } catch (e) {
      return stripeFailure(e);
    }
  }

  /** Apply one verified webhook event (each event once). */
  async applyBillingEvent(event: BillingEvent, now = Date.now()): Promise<{ applied: boolean }> {
    // A purchase may come before Cloud was ever turned on: it names the account.
    if (!this.bind(event.account)) {
      console.warn("billing: event for another account", event.eventId);
      return { applied: false };
    }
    const sql = this.sql();
    if (!firstSeen(sql, event.eventId, now)) return { applied: false };
    if (event.customer) writeBillingState(sql, "customer", event.customer);
    switch (event.type) {
      case "credits_purchased": {
        const reason = `purchase ${event.reference}`;
        if (!hasGrant(sql, reason)) addCredits(sql, { at: now, credits: event.credits, reason });
        break;
      }
      case "subscription": {
        // Stripe delivers out of order: a late `updated` must not undo a
        // newer state, and a canceled subscription never comes back.
        const stored = readBillingState<SubscriptionState & { eventAt?: number }>(sql, "subscription");
        const incoming = event.subscription;
        // Every subscription ever seen canceled: Stripe's cancel is terminal,
        // and it may arrive before that subscription's own earlier events.
        const canceled = readBillingState<string[]>(sql, "canceledSubscriptions") ?? [];
        if (incoming.status === "canceled" && !canceled.includes(incoming.id)) {
          writeBillingState(sql, "canceledSubscriptions", [...canceled, incoming.id].slice(-50));
        }
        const same = stored?.id === incoming.id;
        // Older than what's on record, whichever subscription it's for.
        const older = stored?.eventAt !== undefined && event.createdAt !== undefined && event.createdAt < stored.eventAt;
        const revived = incoming.status !== "canceled" && canceled.includes(incoming.id);
        // A canceled other subscription never erases the live one on record
        // (and so never lets a second plan be bought).
        const displaced = stored !== undefined && !same && stored.status !== "canceled" && incoming.status === "canceled";
        // Same subscription, same second: Checkout's `incomplete` never undoes
        // the `active` it became.
        const regressed =
          same &&
          stored?.eventAt === event.createdAt &&
          incoming.status === "incomplete" &&
          (stored?.status === "active" || stored?.status === "trialing");
        if (!older && !revived && !displaced && !regressed) {
          writeBillingState(sql, "subscription", { ...event.subscription, ...(event.createdAt !== undefined ? { eventAt: event.createdAt } : {}) });
        }
        break;
      }
      case "period_paid": {
        const plan = planById(event.plan);
        if (plan) {
          upsertPeriod(sql, { invoice: event.invoice, plan: plan.id, start: event.start, end: event.end, credits: plan.monthlyCredits });
        }
        break;
      }
      case "customer":
        break;
    }
    await this.saveAccount();
    return { applied: true };
  }

  // ── workflow reports (workflows.ts) ───────────────────────────────────────

  private current(chatId: string, generation: number): SessionRecord | undefined {
    const s = this.sessions.get(chatId);
    return s && s.generation === generation ? s : undefined;
  }

  private maxSessions(): number {
    if (this.acct.maxSessions !== undefined) return this.acct.maxSessions;
    const n = Number(this.env.CLOUD_MAX_SESSIONS);
    return Number.isInteger(n) && n > 0 ? n : 50;
  }

  /** Whether any session holds `sandboxId` (a superseded provision keeps
   * its sandbox only if one does). */
  async wfSandboxAdopted(sandboxId: string): Promise<boolean> {
    return [...this.sessions.values()].some((s) => s.sandboxId === sandboxId);
  }

  async wfSandboxCreated(
    chatId: string,
    generation: number,
    provider: string,
    sandboxId: string,
    type: string,
    createdAt: number
  ): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    s.provider = provider;
    s.sandboxId = sandboxId;
    s.machine = machineName(type);
    s.pendingMachine = undefined;
    recordSandbox(this.sql(), { provider, sandboxId, type, createdAt, chatId });
    this.ledger("create", s, { type });
    this.scheduleReconcile();
    this.register();
    await this.saveSession(s);
    await this.saveAccount();
    return true;
  }

  wfEnrolled(chatId: string, generation: number): boolean {
    return Boolean(this.current(chatId, generation)?.runnerKey);
  }

  wfOnline(chatId: string, generation: number): boolean {
    const s = this.current(chatId, generation);
    return Boolean(s) && !s!.awaitingOnline;
  }

  /**
   * WakeWorkflow: this wake is taking far longer than normal (a resume stuck,
   * or a machine up with no engine). The session says so (`stage: "slow"`)
   * until its engine is heard from, and the ledger keeps why. Returns when.
   */
  async wfSlowWake(chatId: string, generation: number, reason: string): Promise<number> {
    const now = Date.now();
    const s = this.current(chatId, generation);
    if (!s) return now;
    s.wakeStage = { stage: "slow", at: now };
    this.ledger("wake_slow", s, { detail: reason });
    console.info(JSON.stringify({ event: "cloud.wake_slow", chatId, reason }));
    await this.saveSession(s);
    return now;
  }

  async wfReady(chatId: string, generation: number, stages?: WakeStages): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    // Written now as well: an engine too old to report (or one no run ever
    // starts on) must not leave the wake unmeasured. Its report supersedes it.
    if (stages && s.wakeTrace) {
      s.wakeTrace.stages = stages;
      this.recordWakeTiming(s, s.wakeTrace, s.wakeTrace.engine);
      if (s.wakeTrace.engine) s.wakeTrace = undefined;
      await this.saveSession(s);
    }
    if (s.state === "provisioning" || s.state === "starting") {
      const before = s.lastActiveAt;
      this.markReady(s, Date.now());
      if (s.refreshing) {
        // The clock restarts at this stop. It wasn't used: its idle age holds.
        s.refreshing = false;
        if (before !== undefined) s.lastActiveAt = before;
        await this.beginSleep(s, "refreshed its stored data");
      }
      await this.saveSession(s);
      await this.saveAccount();
    }
    return true;
  }

  /** WakeWorkflow: the sandbox came back on `machine`, its new size. */
  async wfResumedOn(chatId: string, generation: number, machine: SandboxMachine): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    s.machine = machine;
    if (s.pendingMachine === machine) s.pendingMachine = undefined;
    if (s.provider && s.sandboxId) setSandboxType(this.sql(), { provider: s.provider, sandboxId: s.sandboxId }, machine);
    this.ledger("resize", s, { type: machine });
    // Read the meter across the change, so the estimate starts from it.
    this.scheduleReconcile();
    await this.saveSession(s);
    await this.saveAccount();
    return true;
  }

  /**
   * WakeWorkflow: the provider refused the resume in a way a retry can't fix.
   * The sandbox stays exactly as it was, stopped, on its old size. A refused
   * shrink leaves it wakeable as it is; a disk too big for the size it is on
   * waits for the user to pick a bigger one (credits change, so never on its
   * own).
   */
  async wfWakeRefused(chatId: string, generation: number, failure: SandboxFailure): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    if (failure.kind === "data_expired") {
      this.markDataExpired(s);
    } else {
      const asked = s.pendingMachine ?? this.currentMachine(s);
      const askedHolds = machineSpec(asked)?.diskGiB ?? 0;
      // A refusal without a figure still says it doesn't fit what was asked, and
      // so does one whose figure reads a little under it (a shrink is judged on
      // a slightly tighter limit than a resume on the same size).
      const neededGiB = roundGiB(Math.max(failure.neededGiB ?? s.disk?.usedGiB ?? 0, askedHolds + 0.1));
      s.wakeBlocked = { reason: "disk_too_small", neededGiB };
      s.pendingMachine = undefined;
      s.refreshing = false;
      s.awaitingOnline = false;
      s.awakeSince = undefined;
      s.disk = { usedGiB: Math.max(s.disk?.usedGiB ?? 0, neededGiB), at: Date.now() };
      this.ledger("error", s, { detail: `refused: its disk needs ${neededGiB} GiB; ${asked} holds ${askedHolds}` });
      if (this.blockedOnDisk(s)) {
        s.state = "error";
        s.failedAction = "wake";
        s.error = `This machine needs ${neededGiB} GiB of disk, more than a ${asked} machine holds. Pick a bigger size to wake it.`;
      } else {
        s.state = "sleeping";
        s.error = undefined;
        s.failedAction = undefined;
      }
      s.expiryCheckAt = Date.now() + EXPIRY_RETRY_MS;
    }
    this.scheduleReconcile();
    await this.saveSession(s);
    await this.saveAccount();
    return true;
  }

  /**
   * SleepWorkflow: the engine declined the sleep (a turn in flight or a
   * message waiting) before anything stopped. The machine stays up; idle
   * sleep tries again a full idle window later.
   */
  async wfSleepDeclined(chatId: string, generation: number, why: string): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s || s.state !== "stopping") return false;
    const now = Date.now();
    s.state = "ready";
    s.pendingWake = false;
    s.lastActiveAt = now;
    s.idleCheckAt = now + this.idleMs();
    console.info("cloud sleep declined by the engine", chatId, why);
    this.scheduleReconcile();
    await this.saveSession(s);
    await this.saveAccount();
    return true;
  }

  async wfStopped(chatId: string, generation: number): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s || s.state !== "stopping") return false;
    s.state = "sleeping";
    s.awakeSince = undefined;
    s.wakeTrace = undefined;
    s.wakeStage = undefined;
    s.activeRuns = 0;
    s.clients = 0;
    s.beatEveryMs = undefined;
    s.idleCheckAt = undefined;
    // A machine that never came up (a sleep from `error`) didn't stop just now:
    // its clock stands, and the provider's reading settles it.
    if (s.stoppedAt === undefined) this.noteStopped(s, Date.now());
    else s.expiryCheckAt = Date.now();
    this.ledger("stop", s);
    this.scheduleReconcile();
    // Asleep first, persisted: a wake that's refused (out of credits, too
    // many awake) must leave a sleeping session, never one stuck stopping.
    const wake = s.pendingWake;
    s.pendingWake = false;
    await this.saveSession(s);
    if (wake) {
      await this.beginWake(s);
      return true;
    }
    await this.saveAccount();
    return true;
  }

  /**
   * DeleteWorkflow, after the sandbox stopped: read the provider's final
   * figure for every open month of the sandbox and close those rows — it is
   * stopped, so the figure can no longer move, and after the delete the
   * provider no longer serves it. A read that still says `running` (meter lag
   * right after the stop) answers "running" so the workflow re-reads shortly;
   * `lastAttempt` persists whatever it has (left open and flagged) so a stuck
   * meter never blocks a user's delete.
   */
  async wfCaptureFinalUsage(
    chatId: string,
    generation: number,
    ref: SandboxRef,
    lastAttempt: boolean
  ): Promise<"final" | "running" | "failed" | "stale"> {
    if (!this.current(chatId, generation)) return "stale";
    const sql = this.sql();
    const now = Date.now();
    const tasks = planReconcile(listSandboxes(sql), listUsage(sql), now).filter(
      (task) => task.provider === ref.provider && task.sandboxId === ref.sandboxId && task.fetch
    );
    const reads: { task: ReconcileTask; usage: SandboxUsage }[] = [];
    let failed = false;
    for (const task of tasks) {
      try {
        reads.push({ task, usage: await this.readUsage(task) });
      } catch (e) {
        failed = true;
        if (lastAttempt) recordUsageFailure(sql, task, `final read failed: ${explain(e)}`, false);
      }
    }
    const running = reads.some((read) => read.usage.running);
    if (!lastAttempt && (failed || running)) return failed ? "failed" : "running";
    for (const { task, usage } of reads) {
      upsertUsage(sql, {
        day: task.day,
        provider: task.provider,
        sandboxId: task.sandboxId,
        sandboxType: usage.machine,
        seconds: usage.seconds,
        dollars: usage.dollars,
        running: usage.running,
        reconciledAt: now,
        closed: !usage.running
      });
      if (usage.running) recordUsageFailure(sql, task, "meter still running at delete", false);
    }
    if (!failed && !running) markSandboxFinal(sql, ref, now);
    await this.saveAccount();
    return failed ? "failed" : running ? "running" : "final";
  }

  async wfDeleted(chatId: string, generation: number, ref: SandboxRef, operationId?: string): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    markSandboxDeleted(this.sql(), ref, Date.now());
    this.ledger("delete", s, { ...ref, ...(operationId ? { detail: `operation ${operationId}` } : {}) });
    await this.finishSessionDelete(s);
    return true;
  }

  async wfDeleteFailed(chatId: string, generation: number, message: string): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    s.state = "error";
    this.noteDeleteFailure(s);
    s.error = message;
    s.failedAction = "delete";
    this.ledger("error", s, { detail: message });
    await this.saveSession(s);
    return true;
  }

  async wfFailed(chatId: string, generation: number, message: string): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    const kind = s.workflow?.kind;
    if (s.refreshing && kind === "wake") {
      // Nobody asked for this wake, so nobody should see it fail.
      await this.settleFailedRefresh(s, message);
      await this.saveSession(s);
      await this.saveAccount();
      return true;
    }
    if (kind === "delete") this.noteDeleteFailure(s);
    s.state = "error";
    s.error = message;
    s.failedAction = kind ? ACTION_OF[kind] : undefined;
    s.awaitingOnline = false;
    this.ledger("error", s, { detail: message });
    this.scheduleReconcile();
    await this.saveSession(s);
    await this.saveAccount();
    return true;
  }
}
