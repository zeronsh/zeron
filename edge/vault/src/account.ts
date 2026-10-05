/**
 * VaultAccount — one Durable Object per (user, provider), `acct1/{userId}/{provider}`:
 * the provider's stored logins ("accounts", any number), which one is ACTIVE
 * (grants come from it), the devices allowed to draw grants, generations,
 * status and refresh bookkeeping per account. The github object also holds
 * pending device-flow attempts and the App installation tokens it minted
 * (sealed, reused until 15 minutes before they expire).
 *
 * The point of one object per provider is refresh-token safety. Providers
 * rotate refresh tokens and treat reuse as theft (OpenAI signs the user out),
 * so two concurrent refreshes from one chain must never happen. A DO is
 * single-threaded, but it still interleaves events across `await`s — the
 * upstream fetch especially — so refreshes go through `refreshing`, an
 * in-memory promise gate per account: every caller that finds a refresh due
 * while one is in flight awaits the same promise, then re-reads the account.
 * Commits run under `blockConcurrencyWhile` and re-check the generation they
 * started from, so an upload that lands mid-refresh wins and the refreshed
 * tokens are dropped. A new generation is always persisted BEFORE any grant
 * built from it is returned (and the output gate holds the reply until the
 * write is durable). Generations count across the object's accounts, so an
 * envelope's AAD never repeats.
 *
 * Accounts: a sign-in to a login already stored (same email + organization,
 * same ChatGPT account, same API key, same GitHub login) replaces it; any
 * other adds one; either way it becomes active. Forgetting the active account
 * leaves none active until the user picks one.
 *
 * Failure semantics (design "Grants"): `reconnect` from the adapter marks the
 * account `needs_reconnect` and stops grants from it; `transient` keeps the
 * current generation, serves the still-valid token if there is one, and
 * retries on a later grant (no sooner than `REFRESH_BACKOFF_MS`).
 *
 * KEK rotation is lazy: every decrypt of an account sealed under the previous
 * key re-seals it under the current one (see `open`).
 */
import { DurableObject } from "cloudflare:workers";
import type {
  Grant,
  GithubDevicePoll,
  GithubRepoView,
  GithubDeviceStart,
  VaultAccountProfile,
  VaultAccountView,
  VaultConnectionView,
  VaultMaterial,
  VaultProviderId,
  VaultResult,
  VaultUsageView
} from "./api";
import { keyId, loadKeyring, openJson, recordAad, seal, sealJson, VaultUnavailable, type Keyring } from "./crypto";
import type { AuditEntry } from "./devices";
import { randomId, utf8 } from "./encoding";
import type { Env } from "./env";
import { devicesStub } from "./names";
import { adapterFor } from "./providers";
import { codexEmail } from "./providers/codex";
import { fetchGithubLogin, parseGithubToken, pollDeviceFlow, startDeviceFlow } from "./providers/github";
import { appJwt, findInstallation, installUrl, mintInstallationToken } from "./providers/github-app";
import { filterRepos, listGithubBranches, listGithubRepos } from "./providers/github-repos";
import type { FetchFn, GrantMaterial, ProviderAdapter } from "./providers/types";
import { fetchUsage, hasUsage } from "./providers/usage";
import { fail, ok, type VaultFailure } from "./result";

/** A refresh attempted this recently is not retried while the current token
 * is still good: bounds upstream traffic during a provider outage, and for
 * providers whose tokens are always "due" (short-lived) it caps refreshes. */
export const REFRESH_BACKOFF_MS = 60_000;
/** Concurrent GitHub device flows kept per user; older ones are dropped. */
const MAX_FLOWS = 5;
const CANARY_KEEP = 60;
/** Canary warns when a non-refreshable credential has less life than this. */
const CANARY_EXPIRY_WARNING_MS = 30 * 24 * 60 * 60_000;
/** A cached installation token is handed out again only with more life than
 * this — above the engine's 10-minute re-grant lead, so a re-grant never
 * gets back the token it is replacing. */
const INSTALLATION_TOKEN_REUSE_MS = 15 * 60_000;
/** More logins per provider than anyone plausibly keeps; bounds the object. */
const MAX_ACCOUNTS = 20;
const PROFILE_FIELD_MAX = 200;

type AccountKind = VaultAccountView["kind"];

type AccountRow = {
  slot: string;
  generation: number;
  status: string;
  kind: string;
  /** De-duplication key (lower-cased email|org, key fingerprint, …); null =
   * unknown, never matched. */
  identity: string | null;
  account: string | null;
  /** Non-secret `VaultAccountProfile` JSON. */
  profile: string | null;
  authorized_devices: string;
  envelope: string;
  created_at: number;
  updated_at: number;
  /** Non-secret end of life for credentials that cannot refresh. */
  hard_expires_at: number | null;
  last_refresh_at: number | null;
  last_attempt_at: number | null;
  last_error: string | null;
  failures: number;
};

type FlowRow = {
  flow_id: string;
  envelope: string;
  interval_secs: number;
  expires_at: number;
  next_poll_at: number;
  authorized_devices: string;
};

/** `ok` = a new generation was committed, or another write superseded the
 * refresh; either way, re-read the account. */
type RefreshDone =
  | { readonly kind: "ok" }
  | { readonly kind: "reconnect"; readonly reason: string }
  | { readonly kind: "transient"; readonly reason: string };

export interface CanaryResult {
  readonly provider: VaultProviderId;
  readonly ok: boolean;
  /** `refreshed` · `valid` · `expiring` · `missing` · `needs_reconnect` ·
   * `not_refreshable` · `upstream` · `error` — the worst over the accounts. */
  readonly status: string;
  readonly at: number;
}

/** Account stored but unreadable (corruption, AAD mismatch, rotated KEK). */
class RecordUnreadable extends Error {}

const parseDevices = (json: string): string[] => {
  try {
    const value: unknown = JSON.parse(json);
    return Array.isArray(value) ? value.filter((d): d is string => typeof d === "string") : [];
  } catch {
    return [];
  }
};

const parseProfile = (json: string | null): VaultAccountProfile => {
  if (!json) return {};
  try {
    const value: unknown = JSON.parse(json);
    return typeof value === "object" && value !== null ? cleanProfile(value as VaultAccountProfile) : {};
  } catch {
    return {};
  }
};

/** Only short plain strings survive: the profile is display data. */
export const cleanProfile = (profile: VaultAccountProfile | undefined): VaultAccountProfile => {
  const out: Record<string, string> = {};
  for (const key of ["email", "displayName", "organization", "plan"] as const) {
    const value = profile?.[key];
    if (typeof value === "string" && value.trim() && value.length <= PROFILE_FIELD_MAX) out[key] = value.trim();
  }
  return out;
};

const flowAad = (userId: string, flowId: string): string => `${userId}|github|flow:${flowId}`;
const installationAad = (userId: string, installationId: number): string =>
  `${userId}|github|installation:${installationId}`;

/** Why a session can't reach `owner`, in words the engine shows the user. */
const notInstalled = (owner: string, slug: string | undefined): string =>
  `github_app_not_installed: The Zeron GitHub App isn't installed on ${owner}` +
  (slug ? ` — install it at ${installUrl(slug)}` : "");

const hardExpired = (row: AccountRow, now: number): boolean =>
  row.hard_expires_at !== null && row.hard_expires_at <= now;

const statusOf = (row: AccountRow, now: number): "connected" | "needsReconnect" =>
  row.status === "connected" && !hardExpired(row, now) ? "connected" : "needsReconnect";

const kindOf = (provider: VaultProviderId, secret: unknown): AccountKind => {
  if (provider === "github") return "github";
  if (provider === "anthropic-key" || provider === "openai-key") return "api-key";
  if (provider === "claude" && (secret as { kind?: string } | null)?.kind === "setup-token") return "setup-token";
  return "oauth";
};

/** Canary statuses, worst first: the provider reports its worst account. */
const CANARY_SEVERITY = ["error", "needs_reconnect", "upstream", "expiring", "not_refreshable", "missing", "valid", "refreshed"];

export class VaultAccount extends DurableObject<Env> {
  private readonly sql: SqlStorage;
  /** Resolved at call time so tests can stub the global `fetch`. */
  private readonly fetchFn: FetchFn = (input, init) => fetch(input, init);
  private userId = "";
  private provider: VaultProviderId = "codex";
  /** One refresh in flight per account (by slot). */
  private readonly refreshing = new Map<string, Promise<RefreshDone>>();
  private readonly polls = new Map<string, Promise<VaultResult<GithubDevicePoll>>>();

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
    this.sql.exec(
      `CREATE TABLE IF NOT EXISTS accounts (
        slot TEXT PRIMARY KEY,
        generation INTEGER NOT NULL,
        status TEXT NOT NULL,
        kind TEXT NOT NULL,
        identity TEXT,
        account TEXT,
        profile TEXT,
        authorized_devices TEXT NOT NULL,
        envelope TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        hard_expires_at INTEGER,
        last_refresh_at INTEGER,
        last_attempt_at INTEGER,
        last_error TEXT,
        failures INTEGER NOT NULL DEFAULT 0
      )`
    );
    this.sql.exec(
      `CREATE TABLE IF NOT EXISTS github_flows (
        flow_id TEXT PRIMARY KEY,
        envelope TEXT NOT NULL,
        interval_secs INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        next_poll_at INTEGER NOT NULL,
        authorized_devices TEXT NOT NULL,
        created_at INTEGER NOT NULL
      )`
    );
    this.sql.exec(
      "CREATE TABLE IF NOT EXISTS canary (seq INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL, ok INTEGER NOT NULL, status TEXT NOT NULL)"
    );
    this.sql.exec(
      `CREATE TABLE IF NOT EXISTS installation_tokens (
        installation_id INTEGER PRIMARY KEY,
        envelope TEXT NOT NULL,
        expires_at INTEGER NOT NULL
      )`
    );
    this.migrateSingleRecord();
  }

  /** The pre-accounts layout held one `record` row: it becomes the provider's
   * one (active) account, same generation and envelope (so the same AAD). */
  private migrateSingleRecord(): void {
    const legacy = this.sql.exec("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'record'").toArray();
    if (legacy.length === 0) return;
    const row = this.sql.exec<Record<string, SqlStorageValue>>("SELECT * FROM record WHERE id = 1").toArray()[0];
    if (row) {
      const slot = newSlot();
      const provider = this.getMeta("provider");
      const kind: AccountKind =
        provider === "github" ? "github" : provider === "anthropic-key" || provider === "openai-key" ? "api-key" : "oauth";
      this.sql.exec(
        `INSERT INTO accounts (slot, generation, status, kind, identity, account, profile, authorized_devices, envelope,
           created_at, updated_at, hard_expires_at, last_refresh_at, last_attempt_at, last_error, failures)
         VALUES (?, ?, ?, ?, NULL, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
        slot,
        row.generation ?? 0,
        row.status ?? "connected",
        kind,
        row.account ?? null,
        row.authorized_devices ?? "[]",
        row.envelope ?? "",
        row.updated_at ?? 0,
        row.updated_at ?? 0,
        row.hard_expires_at ?? null,
        row.last_refresh_at ?? null,
        row.last_attempt_at ?? null,
        row.last_error ?? null,
        row.failures ?? 0
      );
      this.setMeta("active", slot);
    }
    this.sql.exec("DROP TABLE record");
  }

  // ── RPC surface ──────────────────────────────────────────────────────────

  /** The provider's connection, summarized from its active account (else
   * its most recent one); null when it holds none. */
  view(userId: string, provider: VaultProviderId): VaultResult<VaultConnectionView | null> {
    const owner = this.claim(userId, provider);
    if (owner) return owner;
    const rows = this.rows();
    if (rows.length === 0) return ok(null);
    const active = this.activeRow();
    const shown = active ?? rows.reduce((latest, row) => (row.updated_at > latest.updated_at ? row : latest));
    return ok({
      provider,
      status: statusOf(shown, Date.now()),
      authorizedDevices: parseDevices(shown.authorized_devices),
      ...(shown.account ? { account: shown.account } : {}),
      updatedAt: Math.max(...rows.map((row) => row.updated_at)),
      accounts: rows.length,
      hasActive: active !== undefined
    });
  }

  /** Every stored login, most recently updated first. */
  accounts(userId: string, provider: VaultProviderId): VaultResult<VaultAccountView[]> {
    const owner = this.claim(userId, provider);
    if (owner) return owner;
    return ok(this.accountViews());
  }

  put(
    userId: string,
    provider: VaultProviderId,
    material: VaultMaterial,
    authorizedDevices: string[],
    profile?: VaultAccountProfile
  ): Promise<VaultResult<null>> {
    const owner = this.claim(userId, provider);
    if (owner) return Promise.resolve(owner);
    return this.guard(async () => {
      const adapter = this.adapter();
      if (!adapter.parseUpload) return fail("bad_request", `${provider} cannot be uploaded; use the device flow`);
      const parsed = adapter.parseUpload(material, Date.now());
      if (!parsed.ok) return fail("bad_request", parsed.message);
      const stored = await this.store(parsed.secret, authorizedDevices, cleanProfile(profile));
      return stored.ok ? ok(null) : stored;
    });
  }

  /** Grant access to `authorizedDevices` on every account of the provider. */
  authorize(userId: string, provider: VaultProviderId, authorizedDevices: string[]): VaultResult<null> {
    const owner = this.claim(userId, provider);
    if (owner) return owner;
    if (this.rows().length === 0) return fail("not_found", `no ${provider} credential`);
    this.sql.exec("UPDATE accounts SET authorized_devices = ?, updated_at = ?", JSON.stringify(authorizedDevices), Date.now());
    return ok(null);
  }

  /** `slot` becomes the account grants come from; `null` = none. */
  activate(userId: string, provider: VaultProviderId, slot: string | null): VaultResult<VaultAccountView[]> {
    const owner = this.claim(userId, provider);
    if (owner) return owner;
    if (slot === null) {
      this.deleteMeta("active");
    } else {
      if (!this.row(slot)) return fail("not_found", `no such ${provider} account`);
      this.setMeta("active", slot);
    }
    return ok(this.accountViews());
  }

  /** Delete one account. The active one leaves none active. Idempotent. */
  forget(
    userId: string,
    provider: VaultProviderId,
    slot: string
  ): VaultResult<{ readonly removed: boolean; readonly accounts: VaultAccountView[] }> {
    const owner = this.claim(userId, provider);
    if (owner) return owner;
    const removed = this.row(slot) !== undefined;
    this.sql.exec("DELETE FROM accounts WHERE slot = ?", slot);
    if (this.getMeta("active") === slot) this.deleteMeta("active");
    if (this.rows().length === 0) this.sql.exec("DELETE FROM installation_tokens");
    return ok({ removed, accounts: this.accountViews() });
  }

  /** Wipe every account (and pending flows). Idempotent. The generation
   * counter survives, so a later upload never reuses an old generation's AAD. */
  disconnect(userId: string, provider: VaultProviderId): VaultResult<{ readonly removed: boolean }> {
    const owner = this.claim(userId, provider);
    if (owner) return owner;
    const removed = this.rows().length > 0;
    this.sql.exec("DELETE FROM accounts");
    this.sql.exec("DELETE FROM github_flows");
    this.sql.exec("DELETE FROM installation_tokens");
    this.deleteMeta("active");
    return ok({ removed });
  }

  /** Issue a grant from the ACTIVE account to an already-authenticated
   * device (VaultDevices verified the signature, the fence, and that neither
   * the device nor its parent is revoked). Authorized when the account's
   * `authorizedDevices` lists the device OR its parent (a Cloud session
   * sandbox under the logical Cloud device). A `github` grant is an App
   * installation token for `repo`'s owner; the user token itself is never
   * granted. */
  grant(
    userId: string,
    provider: VaultProviderId,
    deviceId: string,
    parentId?: string,
    repo?: string
  ): Promise<VaultResult<Grant>> {
    const owner = this.claim(userId, provider);
    if (owner) return Promise.resolve(owner);
    return this.guard(async () => {
      const active = this.activeRow();
      if (!active) {
        return fail("not_found", this.rows().length > 0 ? `no ${provider} account is active` : `no ${provider} credential`);
      }
      const usable = await this.usableMaterial(active.slot, (row) => {
        const allowed = parseDevices(row.authorized_devices);
        return allowed.includes(deviceId) || (parentId !== undefined && allowed.includes(parentId))
          ? undefined
          : fail("not_authorized", `device is not authorized for ${provider}`);
      });
      if (!usable.ok) return usable;
      if (provider !== "github") return ok(this.toGrant(usable.value.generation, usable.value.material));
      if (repo === undefined) return fail("bad_request", "github grants need the repository they are for");
      return this.installationGrant(active.slot, usable.value.generation, usable.value.material, repo);
    });
  }

  /** Plan usage of one Claude or Codex account, read here; the token never
   * leaves. Refreshes the token first when due (same path as grants). */
  usage(userId: string, provider: VaultProviderId, slot: string): Promise<VaultResult<VaultUsageView>> {
    const owner = this.claim(userId, provider);
    if (owner) return Promise.resolve(owner);
    if (!hasUsage(provider)) return Promise.resolve(fail("not_found", `${provider} has no usage view`));
    return this.guard(async () => {
      const usable = await this.usableMaterial(slot, () => undefined);
      if (!usable.ok) return usable;
      const outcome = await fetchUsage(provider, usable.value.material, this.fetchFn);
      switch (outcome.kind) {
        case "ok":
          return ok({ body: outcome.body, fetchedAt: Date.now() });
        case "unsupported":
          return fail("not_found", `${provider} has no usage view`);
        case "failed":
          return fail("upstream", outcome.reason, 502);
      }
    });
  }

  /** Repositories the user's GitHub connection can reach (users only — the
   * caller checks). Same refresh path as grants; the token never leaves. */
  githubRepos(userId: string, query?: string): Promise<VaultResult<GithubRepoView[]>> {
    const owner = this.claim(userId, "github");
    if (owner) return Promise.resolve(owner);
    return this.guard(async () => {
      const slot = this.activeRow()?.slot;
      if (!slot) return fail("not_found", "no github credential");
      const usable = await this.usableMaterial(slot, () => undefined);
      if (!usable.ok) return usable;
      const listing = await listGithubRepos(usable.value.material.accessToken, this.fetchFn);
      if (listing.kind === "unauthorized") {
        this.markReconnect(slot, usable.value.generation, "GitHub answered 401");
        return fail("needs_reconnect", "GitHub rejected the stored token; reconnect GitHub");
      }
      if (listing.kind === "failed") return fail("upstream", `GitHub repository listing failed: ${listing.reason}`);
      return ok(filterRepos(listing.repos, query));
    });
  }

  /** Branch names of `repo` through the user's connection (users only — the
   * caller checks). Same refresh path as grants; the token never leaves. */
  githubBranches(userId: string, repo: string): Promise<VaultResult<string[]>> {
    const owner = this.claim(userId, "github");
    if (owner) return Promise.resolve(owner);
    return this.guard(async () => {
      const slot = this.activeRow()?.slot;
      if (!slot) return fail("not_found", "no github credential");
      const usable = await this.usableMaterial(slot, () => undefined);
      if (!usable.ok) return usable;
      const listing = await listGithubBranches(usable.value.material.accessToken, repo, this.fetchFn);
      switch (listing.kind) {
        case "unauthorized":
          this.markReconnect(slot, usable.value.generation, "GitHub answered 401");
          return fail("needs_reconnect", "GitHub rejected the stored token; reconnect GitHub");
        case "not_found":
          return fail("not_found", `GitHub can't see ${repo} through this connection`);
        case "failed":
          return fail("upstream", `GitHub branch listing failed: ${listing.reason}`);
        case "ok":
          return ok(listing.branches);
      }
    });
  }

  /** Daily canary: force one refresh of every account through the same gate
   * grants use (so a login kept but not in use stays alive too); the
   * provider reports its worst account. */
  async canary(userId: string, provider: VaultProviderId): Promise<CanaryResult> {
    const at = Date.now();
    const finish = (okFlag: boolean, status: string): CanaryResult => {
      this.sql.exec("INSERT INTO canary (at, ok, status) VALUES (?, ?, ?)", at, okFlag ? 1 : 0, status);
      this.sql.exec("DELETE FROM canary WHERE seq <= (SELECT MAX(seq) FROM canary) - ?", CANARY_KEEP);
      return { provider, ok: okFlag, status, at };
    };
    const owner = this.claim(userId, provider);
    if (owner) return { provider, ok: false, status: "error", at };
    const rows = this.rows();
    if (rows.length === 0) return finish(false, "missing");
    const results: { ok: boolean; status: string }[] = [];
    for (const row of rows) results.push(await this.canaryOne(row, at));
    const worst = results.reduce((a, b) => (CANARY_SEVERITY.indexOf(b.status) < CANARY_SEVERITY.indexOf(a.status) ? b : a));
    return finish(results.every((r) => r.ok), worst.status);
  }

  canaryLog(userId: string, provider: VaultProviderId): VaultResult<CanaryResult[]> {
    const owner = this.claim(userId, provider);
    if (owner) return owner;
    const rows = this.sql
      .exec<{ at: number; ok: number; status: string }>("SELECT at, ok, status FROM canary ORDER BY seq DESC")
      .toArray();
    return ok(rows.map((row) => ({ provider, ok: row.ok === 1, status: row.status, at: row.at })));
  }

  githubStart(userId: string, authorizedDevices: string[]): Promise<VaultResult<GithubDeviceStart>> {
    const owner = this.claim(userId, "github");
    if (owner) return Promise.resolve(owner);
    return this.guard(async () => {
      const clientId = this.env.GITHUB_APP_CLIENT_ID;
      if (!clientId) return fail("unavailable", "GitHub App is not configured");
      const started = await startDeviceFlow(clientId, this.fetchFn);
      if (!started.ok) return fail("upstream", `GitHub device flow failed: ${started.reason}`);
      const { start } = started;
      const flowId = randomId(16);
      // The device code is a bearer for the pending authorization: sealed
      // like any credential.
      const envelope = await sealJson(await this.keyring(), start.deviceCode, flowAad(userId, flowId));
      const now = Date.now();
      const expiresAt = now + start.expiresInSecs * 1000;
      this.sql.exec("DELETE FROM github_flows WHERE expires_at < ?", now);
      this.sql.exec(
        `INSERT INTO github_flows (flow_id, envelope, interval_secs, expires_at, next_poll_at, authorized_devices, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)`,
        flowId,
        envelope,
        start.intervalSecs,
        expiresAt,
        now,
        JSON.stringify(authorizedDevices),
        now
      );
      this.sql.exec(
        "DELETE FROM github_flows WHERE flow_id NOT IN (SELECT flow_id FROM github_flows ORDER BY created_at DESC LIMIT ?)",
        MAX_FLOWS
      );
      return ok({
        flowId,
        userCode: start.userCode,
        verificationUri: start.verificationUri,
        intervalSecs: start.intervalSecs,
        expiresAt
      });
    });
  }

  /** Concurrent polls of one flow share a single upstream exchange: the
   * device code is single-use, so a second exchange could only fail. */
  githubPoll(userId: string, flowId: string): Promise<VaultResult<GithubDevicePoll>> {
    const owner = this.claim(userId, "github");
    if (owner) return Promise.resolve(owner);
    const inflight = this.polls.get(flowId);
    if (inflight) return inflight;
    const run = this.guard(() => this.pollOnce(userId, flowId)).finally(() => {
      if (this.polls.get(flowId) === run) this.polls.delete(flowId);
    });
    this.polls.set(flowId, run);
    return run;
  }

  // ── internals ────────────────────────────────────────────────────────────

  private async canaryOne(row: AccountRow, at: number): Promise<{ ok: boolean; status: string }> {
    try {
      if (row.status !== "connected") return { ok: false, status: "needs_reconnect" };
      const secret = await this.open(row);
      const adapter = this.adapter();
      if (!adapter.refreshable(secret)) {
        const end = adapter.hardExpiresAt?.(secret);
        if (end === undefined) return { ok: false, status: "not_refreshable" };
        if (end <= at) {
          this.markReconnect(row.slot, row.generation, "expired");
          return { ok: false, status: "needs_reconnect" };
        }
        return end - at > CANARY_EXPIRY_WARNING_MS ? { ok: true, status: "valid" } : { ok: false, status: "expiring" };
      }
      const done = await this.refreshOnce(row, secret);
      if (done.kind === "ok") return { ok: true, status: "refreshed" };
      return { ok: false, status: done.kind === "reconnect" ? "needs_reconnect" : "upstream" };
    } catch (error) {
      console.error(JSON.stringify({ event: "vault.canary.error", provider: this.provider, message: errorMessage(error) }));
      return { ok: false, status: "error" };
    }
  }

  /**
   * An App installation token for `repo`'s owner. The user token (already
   * refreshed by `usableMaterial`) lists the installations the user can
   * reach — the ownership check — and the App JWT mints for the one on the
   * owner. A minted token is cached sealed and reused while it has more than
   * `INSTALLATION_TOKEN_REUSE_MS` left; the installation lookup still runs on
   * every grant, so losing access to an account stops grants at once.
   */
  private async installationGrant(
    slot: string,
    generation: number,
    user: GrantMaterial,
    repo: string
  ): Promise<VaultResult<Grant>> {
    const clientId = this.env.GITHUB_APP_CLIENT_ID;
    const pem = this.env.GITHUB_APP_PRIVATE_KEY;
    if (!clientId || !pem) return fail("unavailable", "GitHub App installation tokens are not configured");
    const owner = repo.slice(0, repo.indexOf("/"));
    const found = await findInstallation(user.accessToken, owner, this.fetchFn);
    switch (found.kind) {
      case "unauthorized":
        this.markReconnect(slot, generation, "GitHub answered 401");
        return fail("needs_reconnect", "GitHub rejected the stored token; reconnect GitHub");
      case "failed":
        return fail("upstream", `GitHub installation lookup failed: ${found.reason}`);
      case "none":
        return fail("not_found", notInstalled(owner, this.env.GITHUB_APP_SLUG));
      case "found":
        break;
    }
    const grant = (token: string, expiresAt: number): VaultResult<Grant> =>
      ok({ provider: "github", accessToken: token, expiresAt, generation, ...(user.account ? { account: user.account } : {}) });

    const cached = await this.cachedInstallationToken(found.id);
    if (cached) return grant(cached.token, cached.expiresAt);

    let jwt: string;
    try {
      jwt = await appJwt(clientId, pem, Date.now());
    } catch (error) {
      console.error(JSON.stringify({ event: "vault.github.app_key", message: errorMessage(error) }));
      return fail("unavailable", "the GitHub App private key is unusable");
    }
    const minted = await mintInstallationToken(jwt, found.id, this.fetchFn);
    switch (minted.kind) {
      case "gone":
        this.sql.exec("DELETE FROM installation_tokens WHERE installation_id = ?", found.id);
        return fail("not_found", notInstalled(owner, this.env.GITHUB_APP_SLUG));
      case "rejected":
        // Our App credentials, never the user's: page someone, don't
        // mark the user's connection.
        console.error(JSON.stringify({ event: "vault.github.app_rejected", reason: minted.reason }));
        return fail("unavailable", `GitHub refused the App's credentials (${minted.reason})`);
      case "failed":
        return fail("upstream", `GitHub installation token failed: ${minted.reason}`);
      case "ok":
        break;
    }
    const envelope = await sealJson(await this.keyring(), minted.token, installationAad(this.userId, found.id));
    this.sql.exec(
      `INSERT INTO installation_tokens (installation_id, envelope, expires_at) VALUES (?, ?, ?)
       ON CONFLICT(installation_id) DO UPDATE SET envelope = excluded.envelope, expires_at = excluded.expires_at`,
      found.id,
      envelope,
      minted.expiresAt
    );
    await this.audit({ event: "github_installation_token", provider: "github", detail: `installation ${found.id}` });
    return grant(minted.token, minted.expiresAt);
  }

  private async cachedInstallationToken(
    installationId: number
  ): Promise<{ readonly token: string; readonly expiresAt: number } | undefined> {
    const row = this.sql
      .exec<{ envelope: string; expires_at: number }>(
        "SELECT envelope, expires_at FROM installation_tokens WHERE installation_id = ?",
        installationId
      )
      .toArray()[0];
    if (!row || row.expires_at - Date.now() <= INSTALLATION_TOKEN_REUSE_MS) return undefined;
    try {
      const opened = await openJson<string>(await this.keyring(), row.envelope, installationAad(this.userId, installationId));
      return { token: opened.value, expiresAt: row.expires_at };
    } catch (error) {
      if (error instanceof VaultUnavailable) throw error;
      // Unreadable (rotated past, corrupt): mint a fresh one instead.
      return undefined;
    }
  }

  /**
   * An account's current credential, refreshed first when due — the one
   * path grants and vault-side API calls share. Each pass re-reads the
   * account; passes repeat only when another write (refresh commit, upload,
   * forget) landed during an await.
   */
  private async usableMaterial(
    slot: string,
    authorize: (row: AccountRow) => VaultFailure | undefined
  ): Promise<VaultResult<{ readonly generation: number; readonly material: GrantMaterial }>> {
    const adapter = this.adapter();
    const { provider } = this;
    let refreshed = false;
    for (let pass = 0; pass < 4; pass++) {
      const row = this.row(slot);
      if (!row) return fail("not_found", `no such ${provider} account`);
      if (row.status !== "connected") return fail("needs_reconnect", `${provider} must be reconnected`);
      const refused = authorize(row);
      if (refused) return refused;
      if (hardExpired(row, Date.now())) {
        this.markReconnect(slot, row.generation, "expired");
        return fail("needs_reconnect", `${provider} credential expired; reconnect it`);
      }
      const secret = await this.open(row);
      if (this.row(slot)?.generation !== row.generation) continue;

      const now = Date.now();
      const current = { generation: row.generation, material: adapter.grant(secret, now) };
      if (refreshed || !adapter.refreshDue(secret, now)) return ok(current);
      const stillValid = current.material.expiresAt > now;
      if (stillValid && now - (row.last_attempt_at ?? 0) < REFRESH_BACKOFF_MS) return ok(current);

      const done = await this.refreshOnce(row, secret);
      if (done.kind === "ok") {
        refreshed = true;
        continue;
      }
      if (done.kind === "reconnect") return fail("needs_reconnect", `${provider} refresh was rejected: ${done.reason}`);
      if (this.row(slot)?.generation !== row.generation) continue;
      if (current.material.expiresAt > Date.now()) return ok(current);
      return fail("upstream", `${provider} refresh failed: ${done.reason}`);
    }
    return fail("unavailable", "credential changed concurrently; retry");
  }

  private async pollOnce(userId: string, flowId: string): Promise<VaultResult<GithubDevicePoll>> {
    const clientId = this.env.GITHUB_APP_CLIENT_ID;
    if (!clientId) return fail("unavailable", "GitHub App is not configured");
    const flow = this.flow(flowId);
    if (!flow) return fail("not_found", "no such GitHub device flow");
    const now = Date.now();
    const finished = (state: "connected" | "failed", extra: { account?: string; error?: string }) => {
      this.sql.exec("DELETE FROM github_flows WHERE flow_id = ?", flowId);
      return ok<GithubDevicePoll>({ state, ...extra });
    };
    if (now >= flow.expires_at) return finished("failed", { error: "expired" });
    // Polling faster than GitHub's interval earns `slow_down` penalties; an
    // early poll is answered locally.
    if (now < flow.next_poll_at) return ok<GithubDevicePoll>({ state: "pending" });

    const deviceCode = await this.openFlow(userId, flow);
    const result = await pollDeviceFlow(clientId, deviceCode, flow.interval_secs, this.fetchFn);
    const later = (intervalSecs: number) => {
      this.sql.exec(
        "UPDATE github_flows SET interval_secs = ?, next_poll_at = ? WHERE flow_id = ?",
        intervalSecs,
        Date.now() + intervalSecs * 1000,
        flowId
      );
      return ok<GithubDevicePoll>({ state: "pending" });
    };
    switch (result.kind) {
      case "pending":
        return later(flow.interval_secs);
      case "slow_down":
        return later(result.intervalSecs);
      case "transient":
        console.warn(JSON.stringify({ event: "vault.github.poll", transient: result.reason }));
        return later(flow.interval_secs);
      case "expired":
        return finished("failed", { error: "expired" });
      case "denied":
        return finished("failed", { error: "access_denied" });
      case "failed":
        return finished("failed", { error: result.error });
      case "token": {
        const accessToken = typeof result.body.access_token === "string" ? result.body.access_token : "";
        const login = await fetchGithubLogin(accessToken, this.fetchFn);
        const secret = parseGithubToken(result.body, Date.now(), login);
        if (!secret) return finished("failed", { error: "unexpected token response" });
        const stored = await this.store(secret, parseDevices(flow.authorized_devices), {});
        if (!stored.ok) return finished("failed", { error: stored.message });
        return finished("connected", login ? { account: login } : {});
      }
    }
  }

  /** At most one refresh in flight per account; late arrivals share it. */
  private refreshOnce(row: AccountRow, secret: unknown): Promise<RefreshDone> {
    const inflight = this.refreshing.get(row.slot);
    if (inflight) return inflight;
    const run = this.runRefresh(row, secret).finally(() => {
      if (this.refreshing.get(row.slot) === run) this.refreshing.delete(row.slot);
    });
    this.refreshing.set(row.slot, run);
    return run;
  }

  private async runRefresh(row: AccountRow, secret: unknown): Promise<RefreshDone> {
    const adapter = this.adapter();
    const { userId, provider } = this;
    if (!adapter.refresh) return { kind: "transient", reason: "provider does not refresh" };
    const keyring = await this.keyring();
    const outcome = await adapter.refresh(secret, {
      fetch: this.fetchFn,
      now: Date.now(),
      githubClientId: this.env.GITHUB_APP_CLIENT_ID,
      githubClientSecret: this.env.GITHUB_APP_CLIENT_SECRET
    });
    let generation = row.generation;
    const done = await this.ctx.blockConcurrencyWhile(async (): Promise<RefreshDone> => {
      const current = this.row(row.slot);
      // Superseded by an upload or forget while upstream was answering:
      // that write is the user's newer intent; drop the refreshed tokens.
      if (!current || current.generation !== row.generation) return { kind: "ok" };
      const now = Date.now();
      switch (outcome.kind) {
        case "ok": {
          generation = this.lastGeneration() + 1;
          const envelope = await sealJson(keyring, outcome.secret, recordAad(userId, provider, generation));
          this.sql.exec(
            `UPDATE accounts SET generation = ?, envelope = ?, account = ?, hard_expires_at = ?,
               last_refresh_at = ?, last_attempt_at = ?, last_error = NULL, failures = 0 WHERE slot = ?`,
            generation,
            envelope,
            parseProfile(current.profile).email ?? adapter.account(outcome.secret) ?? current.account,
            adapter.hardExpiresAt?.(outcome.secret) ?? null,
            now,
            now,
            row.slot
          );
          this.setMeta("last_generation", String(generation));
          return { kind: "ok" };
        }
        case "reconnect":
          this.sql.exec(
            `UPDATE accounts SET status = 'needs_reconnect', last_attempt_at = ?, last_error = ?,
               failures = failures + 1 WHERE slot = ?`,
            now,
            outcome.reason,
            row.slot
          );
          return outcome;
        case "transient":
          this.sql.exec(
            "UPDATE accounts SET last_attempt_at = ?, last_error = ?, failures = failures + 1 WHERE slot = ?",
            now,
            outcome.reason,
            row.slot
          );
          return outcome;
      }
    });
    const detail = done.kind === "ok" ? `generation ${generation}` : `${done.kind}: ${done.reason}`;
    console.log(JSON.stringify({ event: "vault.refresh", provider, ok: done.kind === "ok", detail }));
    await this.audit({ event: done.kind === "ok" ? "refresh" : "refresh_failed", provider, detail });
    return done;
  }

  /**
   * Seal `secret` as the next generation of the account it identifies (the
   * same login signed in again) or of a new account, and make that account
   * active — a sign-in is a request to use it.
   */
  private async store(secret: unknown, authorizedDevices: string[], profile: VaultAccountProfile): Promise<VaultResult<string>> {
    const keyring = await this.keyring();
    const identity = await this.identityOf(secret, profile);
    const adapter = this.adapter();
    return this.ctx.blockConcurrencyWhile(async (): Promise<VaultResult<string>> => {
      const existing = identity === undefined ? undefined : this.rows().find((row) => row.identity === identity);
      if (!existing && this.rows().length >= MAX_ACCOUNTS) {
        return fail("bad_request", `at most ${MAX_ACCOUNTS} ${this.provider} accounts; remove one first`);
      }
      const slot = existing?.slot ?? newSlot();
      const generation = this.lastGeneration() + 1;
      const envelope = await sealJson(keyring, secret, recordAad(this.userId, this.provider, generation));
      const now = Date.now();
      this.sql.exec(
        `INSERT INTO accounts (slot, generation, status, kind, identity, account, profile, authorized_devices, envelope,
           created_at, updated_at, hard_expires_at, last_refresh_at, last_attempt_at, last_error, failures)
         VALUES (?, ?, 'connected', ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, 0)
         ON CONFLICT(slot) DO UPDATE SET generation = excluded.generation, status = excluded.status,
           kind = excluded.kind, identity = excluded.identity, account = excluded.account,
           profile = excluded.profile, authorized_devices = excluded.authorized_devices,
           envelope = excluded.envelope, updated_at = excluded.updated_at,
           hard_expires_at = excluded.hard_expires_at, last_refresh_at = NULL,
           last_attempt_at = NULL, last_error = NULL, failures = 0`,
        slot,
        generation,
        kindOf(this.provider, secret),
        identity ?? null,
        profile.email ?? adapter.account(secret) ?? null,
        JSON.stringify(profile),
        JSON.stringify(authorizedDevices),
        envelope,
        existing?.created_at ?? now,
        now,
        adapter.hardExpiresAt?.(secret) ?? null
      );
      this.setMeta("last_generation", String(generation));
      this.setMeta("active", slot);
      return ok(slot);
    });
  }

  /** What makes two uploads the same login: never secret material itself. */
  private async identityOf(secret: unknown, profile: VaultAccountProfile): Promise<string | undefined> {
    const value = (secret ?? {}) as Record<string, unknown>;
    switch (this.provider) {
      case "anthropic-key":
      case "openai-key":
        return typeof value.key === "string" ? `key:${await keyId(utf8(value.key))}` : undefined;
      case "github":
        return typeof value.login === "string" ? `login:${value.login.toLowerCase()}` : undefined;
      case "codex": {
        const email = typeof value.idToken === "string" ? codexEmail(value.idToken) : undefined;
        return email ? `chatgpt:${email.toLowerCase()}|${String(value.accountId ?? "")}` : undefined;
      }
      case "claude":
        return profile.email && value.kind === "oauth"
          ? `claude:${profile.email.toLowerCase()}|${(profile.organization ?? "").toLowerCase()}`
          : undefined;
    }
  }

  private toGrant(generation: number, material: GrantMaterial): Grant {
    return {
      provider: this.provider,
      accessToken: material.accessToken,
      expiresAt: material.expiresAt,
      generation,
      ...(material.accountId ? { accountId: material.accountId } : {}),
      ...(material.idToken ? { idToken: material.idToken } : {}),
      ...(material.account ? { account: material.account } : {}),
      ...(material.scopes ? { scopes: [...material.scopes] } : {}),
      ...(material.subscriptionType ? { subscriptionType: material.subscriptionType } : {})
    };
  }

  /** Stop issuing from an account until the user reconnects it (expired
   * non-refreshable credential, or the provider rejected the token
   * outright); persisted so `status` says so too. Scoped to the generation
   * that failed. */
  private markReconnect(slot: string, generation: number, reason: string): void {
    this.sql.exec(
      "UPDATE accounts SET status = 'needs_reconnect', last_error = ? WHERE slot = ? AND generation = ?",
      reason,
      slot,
      generation
    );
  }

  /**
   * Decrypt an account. One sealed under `VAULT_KEK_PREVIOUS` is re-sealed
   * under the current key (same generation, same AAD) and persisted before
   * the plaintext is used, so normal traffic migrates accounts after a
   * rotation. The write is conditional on the row still holding the envelope
   * we opened: a concurrent upload or refresh commit wins.
   */
  private async open(row: AccountRow): Promise<unknown> {
    const keyring = await this.keyring();
    const aad = recordAad(this.userId, this.provider, row.generation);
    let opened: Awaited<ReturnType<typeof openJson<unknown>>>;
    try {
      opened = await openJson<unknown>(keyring, row.envelope, aad);
    } catch (error) {
      // An unknown kid stays `unavailable` (503) and names the key.
      if (error instanceof VaultUnavailable) throw error;
      throw new RecordUnreadable(`${this.provider} record (generation ${row.generation}) could not be decrypted`);
    }
    if (opened.opened.rewrap) {
      const envelope = await seal(keyring, opened.opened.plaintext, aad);
      this.sql.exec(
        "UPDATE accounts SET envelope = ? WHERE slot = ? AND generation = ? AND envelope = ?",
        envelope,
        row.slot,
        row.generation,
        row.envelope
      );
      console.log(JSON.stringify({ event: "vault.rewrap", provider: this.provider, from: opened.opened.kid, to: keyring.current.kid }));
    }
    return opened.value;
  }

  private async openFlow(userId: string, flow: FlowRow): Promise<string> {
    try {
      return (await openJson<string>(await this.keyring(), flow.envelope, flowAad(userId, flow.flow_id))).value;
    } catch (error) {
      if (error instanceof VaultUnavailable) throw error;
      throw new RecordUnreadable("GitHub device flow could not be decrypted");
    }
  }

  /** Loaded per use: imported keys are cached per isolate by kid, and a
   * missing/invalid secret must fail now rather than stay cached. */
  private keyring(): Promise<Keyring> {
    return loadKeyring(this.env);
  }

  private adapter(): ProviderAdapter<unknown> {
    return adapterFor(this.provider);
  }

  /** Refusals are values; only bugs throw. Messages never carry secrets. */
  private async guard<T>(run: () => Promise<VaultResult<T>>): Promise<VaultResult<T>> {
    try {
      return await run();
    } catch (error) {
      if (error instanceof VaultUnavailable) return fail("unavailable", error.message, 503);
      if (error instanceof RecordUnreadable) {
        console.error(JSON.stringify({ event: "vault.unreadable", provider: this.provider, message: error.message }));
        return fail("unavailable", error.message, 500);
      }
      console.error(JSON.stringify({ event: "vault.error", provider: this.provider, message: errorMessage(error) }));
      return fail("unavailable", "internal vault error", 500);
    }
  }

  private async audit(entry: AuditEntry): Promise<void> {
    try {
      await devicesStub(this.env, this.userId).audit(this.userId, entry);
    } catch (error) {
      console.error(JSON.stringify({ event: "vault.audit.error", message: errorMessage(error) }));
    }
  }

  /** Pin the object to its (user, provider) on first use; a mismatch is a
   * routing bug and is refused rather than mixed. */
  private claim(userId: string, provider: VaultProviderId): VaultFailure | undefined {
    const owner = this.getMeta("owner");
    const slot = this.getMeta("provider");
    if (owner === undefined || slot === undefined) {
      this.setMeta("owner", userId);
      this.setMeta("provider", provider);
    } else if (owner !== userId || slot !== provider) {
      return fail("forbidden", "vault object belongs to another slot");
    }
    this.userId = userId;
    this.provider = provider;
    return undefined;
  }

  /** Most recently updated first. */
  private rows(): AccountRow[] {
    // Creation order: a re-sign-in or a switch never moves an account.
    return this.sql.exec<AccountRow>("SELECT * FROM accounts ORDER BY created_at, slot").toArray();
  }

  private row(slot: string): AccountRow | undefined {
    return this.sql.exec<AccountRow>("SELECT * FROM accounts WHERE slot = ?", slot).toArray()[0];
  }

  private activeRow(): AccountRow | undefined {
    const slot = this.getMeta("active");
    return slot === undefined ? undefined : this.row(slot);
  }

  private accountViews(): VaultAccountView[] {
    const active = this.getMeta("active");
    const now = Date.now();
    return this.rows().map((row) => ({
      slot: row.slot,
      status: statusOf(row, now),
      active: row.slot === active,
      kind: (ACCOUNT_KINDS.includes(row.kind as AccountKind) ? row.kind : "oauth") as AccountKind,
      ...(row.account ? { account: row.account } : {}),
      ...parseProfile(row.profile),
      authorizedDevices: parseDevices(row.authorized_devices),
      createdAt: row.created_at,
      updatedAt: row.updated_at
    }));
  }

  private flow(flowId: string): FlowRow | undefined {
    return this.sql.exec<FlowRow>("SELECT * FROM github_flows WHERE flow_id = ?", flowId).toArray()[0];
  }

  private lastGeneration(): number {
    const stored = this.sql.exec<{ g: number | null }>("SELECT MAX(generation) AS g FROM accounts").toArray()[0]?.g ?? 0;
    return Math.max(Number(this.getMeta("last_generation") ?? 0), stored);
  }

  private getMeta(key: string): string | undefined {
    return this.sql.exec<{ value: string }>("SELECT value FROM meta WHERE key = ?", key).toArray()[0]?.value;
  }

  private setMeta(key: string, value: string): void {
    this.sql.exec("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value", key, value);
  }

  private deleteMeta(key: string): void {
    this.sql.exec("DELETE FROM meta WHERE key = ?", key);
  }
}

const ACCOUNT_KINDS: readonly AccountKind[] = ["oauth", "setup-token", "api-key", "github"];

/** Account ids: short, random, URL- and route-safe. */
const newSlot = (): string => `a${randomId(9)}`;

const errorMessage = (error: unknown): string => (error instanceof Error ? error.message : String(error));
