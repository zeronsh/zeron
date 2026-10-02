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
  appendLedger,
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
  summarizeMonth,
  upsertUsage,
  type CloudUsage,
  type LedgerEntry,
  type LedgerEvent,
  type ReconcileTask,
  type SandboxRef
} from "./metering";
import {
  AWAKE_STATES,
  HEARTBEAT_STALE_MS,
  idleDecision,
  newCloudDeviceId,
  parseIdleMs,
  parseMaxAwake,
  parseTtlSeconds,
  planEviction,
  shouldExtendTtl,
  tokenTimestampCheck,
  workflowInstanceId,
  type CloudState,
  type WorkflowKind
} from "./policy";
import { activeProviderName, sandboxProvider } from "./providers";
import { SandboxError, effectiveTtl, type SandboxUsage } from "./sandbox-provider";

export interface CloudCaller {
  readonly orgId: string;
  readonly userId: string;
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
}

export type CloudResult<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly status: number; readonly error: string; readonly message: string };

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
}

export interface LifecycleParams {
  readonly orgId: string;
  readonly userId: string;
  readonly chatId: string;
  readonly deviceId: string;
  readonly generation: number;
  readonly provider: string;
  readonly sandboxId: string;
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
export interface CloudBillingRow extends CloudUsage {
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
  lastActiveAt?: number;
  lastHeartbeatAt?: number;
  activeRuns: number;
  clients: number;
  lastTtlExtendAt?: number;
  /** Throttles the "ready but silent — did the provider auto-stop it?" probe. */
  lastProbeAt?: number;
  workflow?: { kind: WorkflowKind; id: string };
  /** A wake requested while stopping: start it as soon as the stop lands. */
  pendingWake?: boolean;
  /** A delete failed half-way; only a delete may proceed from here. */
  pendingDelete?: boolean;
  deletingSince?: number;
  error?: string;
  failedAction?: CloudAction;
  idleCheckAt?: number;
}

const ACCOUNT_KEY = "account";
const SESSION_PREFIX = "session:";
/** A delete that crashed mid-way may be retried after this long. */
const DELETE_STUCK_MS = 5 * 60_000;
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

const fail = <T>(status: number, error: string, message: string): CloudResult<T> => ({
  ok: false,
  status,
  error,
  message
});

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
    }
    if (this.acct.nextReconcileAt !== undefined) due.push(this.acct.nextReconcileAt);
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
    return parseMaxAwake(this.env.CLOUD_MAX_AWAKE);
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
      available: true
    };
  }

  private sessionView(s: SessionRecord): CloudSessionView {
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
      ...(s.provider && s.sandboxId ? { sandbox: { provider: s.provider, id: s.sandboxId } } : {})
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
    if (s.state === "deleting") s.pendingDelete = true;
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
    s.lastActiveAt = now;
    s.idleCheckAt = now + this.idleMs();
    this.ledger("ready", s);
    this.scheduleReconcile();
  }

  /** The session's runner proved it is alive (a token or a heartbeat). */
  private markAlive(s: SessionRecord, now: number): void {
    if (s.awaitingOnline) {
      s.awaitingOnline = false;
      this.sendEvent(s, "wake", "online");
    }
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
    await this.saveAccount();
    this.ledger("ready", undefined, { detail: `account ${a.deviceId}` });
    this.register();
    return this.okAccount();
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
    const { generation, orgId, userId } = this.acct;
    this.acct = freshAccount(generation, orgId && userId ? { orgId, userId } : undefined);
    const sql = this.sql();
    const now = Date.now();
    this.acct.nextReconcileAt = meteringOpen(listSandboxes(sql), listUsage(sql), now)
      ? now + RECONCILE_EVERY_MS
      : undefined;
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
    baseBranch?: string
  ): Promise<CloudResult<CloudSessionView>> {
    const off = this.accountOf(caller);
    if (off) return off;
    if (!ID_RE.test(chatId)) return fail(400, "bad_request", "Malformed chatId.");
    if (!ID_RE.test(spaceId)) return fail(400, "bad_request", "Malformed spaceId.");
    if (baseBranch !== undefined && !isBranch(baseBranch)) return fail(400, "bad_request", "Malformed branch.");
    const existing = this.sessions.get(chatId);
    if (existing) return this.okSession(existing);
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
      generation: 0,
      provider: providerName,
      activeRuns: 0,
      clients: 0,
      lastTtlExtendAt: now
    };
    this.sessions.set(chatId, s);
    this.byDevice.set(s.deviceId, chatId);
    // Before the laptop learns the device: its first send nudges this room
    // while the machine is still booting, and must queue, not 404.
    await this.claimDeviceRoom(s.deviceId, caller.userId);
    return this.beginProvision(caller, s, edgeUrl);
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
    s.enrollHash = hash;
    s.runnerKey = undefined;
    s.lastTokenTs = undefined;
    s.awaitingOnline = false;
    s.error = undefined;
    s.lastTtlExtendAt = Date.now();
    this.register();
    const { deviceId, sandboxId } = s;
    const provider = s.provider ?? activeProviderName(this.env);
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
      ...(sandboxId ? { sandboxId } : {})
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
    if (!(await this.makeRoomFor(s.chatId))) return this.tooManyAwake();
    s.state = "starting";
    s.awaitingOnline = true;
    s.pendingWake = false;
    s.error = undefined;
    s.lastTtlExtendAt = Date.now(); // resume sets the TTL afresh
    this.ledger("wake", s);
    this.scheduleReconcile();
    return this.startWorkflow<LifecycleParams>(s, "wake", (generation) => ({ ...params, generation }));
  }

  private async beginSleep(s: SessionRecord, reason: string): Promise<CloudResult<CloudSessionView>> {
    const params = this.lifecycleParams(s);
    if (!params) return this.okSession(s);
    s.state = "stopping";
    s.pendingWake = false;
    s.idleCheckAt = undefined;
    this.ledger("sleep", s, { detail: reason });
    return this.startWorkflow<LifecycleParams>(s, "sleep", (generation) => ({ ...params, generation }));
  }

  private session(caller: CloudCaller, chatId: string): SessionRecord | CloudResult<never> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    return this.sessions.get(chatId) ?? fail(404, "unknown_session", "No Cloud session for that chat.");
  }

  async wakeSession(caller: CloudCaller, chatId: string): Promise<CloudResult<CloudSessionView>> {
    const s = this.session(caller, chatId);
    if ("ok" in s) return s;
    if (s.pendingDelete || s.state === "deleting") return this.okSession(s);
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
  async retrySession(caller: CloudCaller, chatId: string, edgeUrl: string): Promise<CloudResult<CloudSessionView>> {
    const s = this.session(caller, chatId);
    if ("ok" in s) return s;
    if (s.state === "error" && !s.runnerKey && !s.pendingDelete) return this.beginProvision(caller, s, edgeUrl);
    return this.wakeSession(caller, chatId);
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

    if (workflow) {
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
   * ready-but-silent gets one cheap state probe per minute.
   */
  async autoWake(caller: CloudCaller, deviceId: string): Promise<boolean> {
    const chatId = this.byDevice.get(deviceId);
    const s = chatId ? this.sessions.get(chatId) : undefined;
    if (!s || !this.matches(caller) || s.pendingDelete || s.state === "deleting") return false;
    if (s.state === "sleeping") return (await this.beginWake(s)).ok;
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

  /** `POST /runner/heartbeat` (runner bearer, already verified). */
  async heartbeat(runner: CloudCaller & { deviceId: string }, beat: Heartbeat): Promise<CloudResult<{ ok: true }>> {
    const s = this.sessionByDevice(runner.deviceId);
    if (!s || !s.runnerKey || !this.matches(runner)) {
      return fail(403, "revoked", "This Cloud device is not enrolled.");
    }
    const now = Date.now();
    const changed = s.activeRuns !== beat.activeRuns || s.clients !== beat.clients;
    const busy = beat.activeRuns > 0 || beat.clients > 0;
    s.activeRuns = beat.activeRuns;
    s.clients = beat.clients;
    s.lastHeartbeatAt = now;
    if (busy || changed) s.lastActiveAt = now;
    this.markAlive(s, now);
    if (s.state === "ready") s.idleCheckAt = (s.lastActiveAt ?? now) + this.idleMs();
    const provider = this.providerOf(s);
    const ttlSeconds = provider ? effectiveTtl(provider.capabilities, parseTtlSeconds(this.env.CLOUD_TTL_SECONDS)) : null;
    if (provider && s.sandboxId && shouldExtendTtl({ ttlSeconds, busy, lastExtendedAt: s.lastTtlExtendAt, now })) {
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
    return { ok: true, value: { ok: true } };
  }

  async alarm(): Promise<void> {
    const now = Date.now();
    for (const s of [...this.sessions.values()]) {
      if (s.state !== "ready" || s.idleCheckAt === undefined || now < s.idleCheckAt) continue;
      const decision = idleDecision({
        now,
        idleMs: this.idleMs(),
        lastActiveAt: s.lastActiveAt,
        lastHeartbeatAt: s.lastHeartbeatAt,
        activeRuns: s.activeRuns,
        clients: s.clients
      });
      if (decision.sleep) await this.beginSleep(s, "idle");
      else {
        s.idleCheckAt = decision.checkAt;
        await this.saveSession(s);
      }
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
   * Pull each provider's figure for every open (month, sandbox) window.
   * Returns whether every read succeeded. Never throws for a provider
   * failure: the row keeps its last figure plus `reconcileError`, and the
   * next pass retries. `now` is a parameter for the month-boundary tests.
   */
  async reconcile(now = Date.now()): Promise<boolean> {
    if (!this.acct.orgId) return true; // never enabled: nothing to meter
    const sql = this.sql();
    let allOk = true;
    for (const task of planReconcile(listSandboxes(sql), listUsage(sql), now)) {
      if (!task.fetch) {
        closeUsageRow(sql, task.month, task);
        continue;
      }
      try {
        const usage = await this.readUsage(task);
        upsertUsage(sql, {
          month: task.month,
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
        // its month is over, close the row on its last figure (flagged).
        const gone = e instanceof SandboxError && e.kind === "notFound";
        recordUsageFailure(sql, task, explain(e), gone && task.close);
      }
    }
    this.acct.nextReconcileAt = meteringOpen(listSandboxes(sql), listUsage(sql), now)
      ? now + RECONCILE_EVERY_MS
      : undefined;
    this.register();
    await this.saveAccount();
    return allOk;
  }

  /** `GET /cloud/{orgId}/usage?month=` — the caller's own month. */
  usage(caller: CloudCaller, month: string, now = Date.now()): CloudResult<CloudUsage> {
    if (this.acct.orgId !== undefined && !this.matches(caller)) return fail(403, "forbidden", "Not your Cloud.");
    const { errors: _errors, orgId: _o, userId: _u, deviceId: _d, ...view } = this.billing(month, now);
    void [_errors, _o, _u, _d];
    return { ok: true, value: view };
  }

  /** Operator export row (billing.ts). */
  billingUsage(month: string, now = Date.now()): CloudBillingRow {
    return this.billing(month, now);
  }

  private billing(month: string, now: number): CloudBillingRow {
    const summary = this.acct.orgId
      ? summarizeMonth(month, listSandboxes(this.sql()), listUsage(this.sql(), month), now)
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

  // ── workflow reports (workflows.ts) ───────────────────────────────────────

  private current(chatId: string, generation: number): SessionRecord | undefined {
    const s = this.sessions.get(chatId);
    return s && s.generation === generation ? s : undefined;
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

  async wfReady(chatId: string, generation: number): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s) return false;
    if (s.state === "provisioning" || s.state === "starting") {
      this.markReady(s, Date.now());
      await this.saveSession(s);
      await this.saveAccount();
    }
    return true;
  }

  async wfStopped(chatId: string, generation: number): Promise<boolean> {
    const s = this.current(chatId, generation);
    if (!s || s.state !== "stopping") return false;
    s.state = "sleeping";
    s.activeRuns = 0;
    s.clients = 0;
    s.idleCheckAt = undefined;
    this.ledger("stop", s);
    this.scheduleReconcile();
    if (s.pendingWake) {
      await this.beginWake(s);
      return true;
    }
    await this.saveSession(s);
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
        month: task.month,
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
    s.pendingDelete = true;
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
