/**
 * VaultConnections — one Durable Object per user (`conn1/{userId}`): encrypted
 * remote MCP and plugin connection configs and credentials (custom headers or
 * OAuth tokens), device authorization, and single-flight OAuth refresh per
 * connection id.
 *
 * Security invariants:
 * - Secret material (`headers` values, OAuth `accessToken` / `refreshToken` /
 *   `clientSecret`) is AES-256-GCM sealed under `VAULT_KEK` with AAD
 *   `conn|${userId}|${id}|${generation}`. Generations are strictly monotonic
 *   per DO, so an AAD never repeats even after delete + recreate.
 * - `VAULT_KEK_PREVIOUS` is lazily re-wrapped under `VAULT_KEK` on decrypt.
 * - Views (`VaultConnectionView`) never contain secrets (only `headerKeys`,
 *   `scopes`, `accountLabel`, `status`, timestamps, non-secret `lastError`).
 * - Grants (`ConnectionGrant`) never contain `refreshToken` or `clientSecret`.
 * - OAuth refresh validates `tokenEndpoint` with `assertSafeHttpsUrl`, uses
 *   `redirect: "manual"`, deduplicates concurrent refreshes per connection id
 *   via `refreshing`, and commits the new generation + sealed envelope under
 *   `blockConcurrencyWhile` before returning any grant built from it.
 */
import { DurableObject } from "cloudflare:workers";
import {
  CONNECTION_ID_RE,
  type ConnectionGrant,
  type ConnectionGrantManyReply,
  type PutConnectionRequest,
  type UpdateConnectionRequest,
  type VaultConnectionAuthKind,
  type VaultConnectionKind,
  type VaultConnectionMaterial,
  type VaultConnectionStatus,
  type VaultConnectionTransport,
  type VaultConnectionView,
  type VaultResult
} from "./api";
import { loadKeyring, openJson, seal, sealJson, VaultUnavailable, type Keyring } from "./crypto";
import type { AuditEntry } from "./devices";
import type { Env } from "./env";
import { devicesStub } from "./names";
import { fail, ok, type VaultFailure } from "./result";
import { assertSafeHttpsUrl, UnsafeUrlError } from "./ssrf";
import { isId } from "./validate";

export const MAX_CONNECTIONS = 100;
export const OAUTH_REFRESH_LEAD_MS = 5 * 60_000;
export const CONNECTION_REFRESH_BACKOFF_MS = 60_000;

const MAX_NAME_LENGTH = 128;
const MAX_PRESET_LENGTH = 64;
const MAX_LABEL_LENGTH = 200;
const MAX_AUTHORIZED_DEVICES = 32;
const MAX_HEADERS = 32;
const MAX_HEADER_VALUE_LENGTH = 8192;
const MAX_SCOPES = 64;
const MAX_SCOPE_LENGTH = 128;
const MAX_TOKEN_LENGTH = 16384;
const MAX_CLIENT_ID_LENGTH = 512;
const MAX_CLIENT_SECRET_LENGTH = 4096;
const MAX_RESOURCE_LENGTH = 2048;

const HEADER_NAME_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]{1,128}$/;
const TOKEN_TYPE_RE = /^[A-Za-z0-9_-]{1,32}$/;
const CRLF_RE = /[\r\n]/;

export const connectionAad = (userId: string, id: string, generation: number): string =>
  `conn|${userId}|${id}|${generation}`;

type ConnectionRow = {
  id: string;
  kind: string;
  name: string;
  preset: string | null;
  url: string;
  transport: string;
  enabled: number;
  auth_kind: string;
  status: string;
  header_keys: string;
  scopes: string;
  account_label: string | null;
  authorized_devices: string;
  generation: number;
  envelope: string | null;
  expires_at: number | null;
  last_refresh_at: number | null;
  last_attempt_at: number | null;
  last_error: string | null;
  failures: number;
  created_at: number;
  updated_at: number;
};

type OAuthMaterial = Extract<VaultConnectionMaterial, { kind: "oauth" }>;

type RefreshOutcome =
  | { readonly kind: "ok" }
  | { readonly kind: "reauth"; readonly reason: string }
  | { readonly kind: "ssrf"; readonly reason: string }
  | { readonly kind: "transient"; readonly reason: string };

export interface ListConnectionsFilter {
  readonly kind?: VaultConnectionKind;
  readonly deviceId?: string;
  readonly parentId?: string;
  readonly enabledOnly?: boolean;
}

class RecordUnreadable extends Error {}

const isPlainObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const errorMessage = (error: unknown): string => (error instanceof Error ? error.message : String(error));

const kindFromId = (id: string): VaultConnectionKind => (id.startsWith("mcp:") ? "mcp" : "plugin");

const parseStringArray = (raw: string): string[] => {
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((item): item is string => typeof item === "string") : [];
  } catch {
    return [];
  }
};

export const isDeviceAuthorizedForConnection = (
  authorizedDevices: readonly string[],
  deviceId: string,
  parentId?: string
): boolean =>
  authorizedDevices.includes("*") ||
  authorizedDevices.includes(deviceId) ||
  (parentId !== undefined && authorizedDevices.includes(parentId));

const parseAuthorizedDevices = (value: unknown): string[] | undefined => {
  if (!Array.isArray(value) || value.length > MAX_AUTHORIZED_DEVICES) return undefined;
  const out: string[] = [];
  const seen = new Set<string>();
  for (const item of value) {
    if (item !== "*" && !isId(item)) return undefined;
    if (!seen.has(item)) {
      seen.add(item);
      out.push(item);
    }
  }
  return out;
};

const parseScopes = (value: unknown): string[] | undefined => {
  if (!Array.isArray(value) || value.length > MAX_SCOPES) return undefined;
  const out: string[] = [];
  const seen = new Set<string>();
  for (const item of value) {
    if (typeof item !== "string") return undefined;
    const trimmed = item.trim();
    if (!trimmed || trimmed.length > MAX_SCOPE_LENGTH || CRLF_RE.test(trimmed)) return undefined;
    if (!seen.has(trimmed)) {
      seen.add(trimmed);
      out.push(trimmed);
    }
  }
  return out;
};

const validateMaterial = (
  authKind: VaultConnectionAuthKind,
  raw: unknown
): VaultResult<{ readonly material: VaultConnectionMaterial | null; readonly headerKeys: string[]; readonly scopes?: string[]; readonly expiresAt: number | null }> => {
  if (authKind === "none") {
    if (raw !== undefined) return fail("bad_request", "material is not allowed when authKind is none");
    return ok({ material: null, headerKeys: [], expiresAt: null });
  }
  if (raw === undefined) {
    return ok({ material: null, headerKeys: [], expiresAt: null });
  }
  if (!isPlainObject(raw)) return fail("bad_request", "material must be an object");

  if (authKind === "headers") {
    if (raw.kind !== "headers" || !isPlainObject(raw.headers)) {
      return fail("bad_request", "headers material must be { kind: 'headers', headers: Record<string, string> }");
    }
    const entries = Object.entries(raw.headers);
    if (entries.length === 0 || entries.length > MAX_HEADERS) {
      return fail("bad_request", `headers must contain between 1 and ${MAX_HEADERS} entries`);
    }
    const headers: Record<string, string> = {};
    for (const [key, val] of entries) {
      if (!HEADER_NAME_RE.test(key)) return fail("bad_request", `invalid header name "${key}"`);
      if (typeof val !== "string" || val.length === 0 || val.length > MAX_HEADER_VALUE_LENGTH || CRLF_RE.test(val)) {
        return fail("bad_request", `invalid value for header "${key}"`);
      }
      headers[key] = val;
    }
    return ok({
      material: { kind: "headers", headers },
      headerKeys: Object.keys(headers).sort(),
      expiresAt: null
    });
  }

  // authKind === "oauth"
  if (raw.kind !== "oauth") {
    return fail("bad_request", "oauth material must have kind 'oauth'");
  }
  if (
    typeof raw.accessToken !== "string" ||
    raw.accessToken.length === 0 ||
    raw.accessToken.length > MAX_TOKEN_LENGTH ||
    CRLF_RE.test(raw.accessToken)
  ) {
    return fail("bad_request", "oauth material requires a non-empty accessToken");
  }
  if (typeof raw.tokenEndpoint !== "string") {
    return fail("bad_request", "oauth material requires tokenEndpoint");
  }
  let tokenUrl: URL;
  try {
    tokenUrl = assertSafeHttpsUrl(raw.tokenEndpoint, "tokenEndpoint");
  } catch (error) {
    return fail("bad_request", errorMessage(error));
  }
  if (
    typeof raw.clientId !== "string" ||
    raw.clientId.trim().length === 0 ||
    raw.clientId.length > MAX_CLIENT_ID_LENGTH ||
    CRLF_RE.test(raw.clientId)
  ) {
    return fail("bad_request", "oauth material requires a non-empty clientId");
  }
  let refreshToken: string | undefined;
  if (raw.refreshToken !== undefined) {
    if (
      typeof raw.refreshToken !== "string" ||
      raw.refreshToken.length === 0 ||
      raw.refreshToken.length > MAX_TOKEN_LENGTH ||
      CRLF_RE.test(raw.refreshToken)
    ) {
      return fail("bad_request", "refreshToken must be a non-empty string");
    }
    refreshToken = raw.refreshToken;
  }
  let tokenType: string | undefined;
  if (raw.tokenType !== undefined) {
    if (typeof raw.tokenType !== "string" || !TOKEN_TYPE_RE.test(raw.tokenType)) {
      return fail("bad_request", "tokenType is invalid");
    }
    tokenType = raw.tokenType;
  }
  let expiresAt: number | undefined;
  if (raw.expiresAt !== undefined) {
    if (typeof raw.expiresAt !== "number" || !Number.isSafeInteger(raw.expiresAt) || raw.expiresAt <= 0) {
      return fail("bad_request", "expiresAt must be a positive Unix ms timestamp");
    }
    expiresAt = raw.expiresAt;
  }
  let clientSecret: string | undefined;
  if (raw.clientSecret !== undefined) {
    if (
      typeof raw.clientSecret !== "string" ||
      raw.clientSecret.length > MAX_CLIENT_SECRET_LENGTH ||
      CRLF_RE.test(raw.clientSecret)
    ) {
      return fail("bad_request", "clientSecret is invalid");
    }
    if (raw.clientSecret.length > 0) clientSecret = raw.clientSecret;
  }
  let scopes: string[] | undefined;
  if (raw.scopes !== undefined) {
    scopes = parseScopes(raw.scopes);
    if (!scopes) return fail("bad_request", "oauth scopes must be a list of scope strings");
  }
  let resource: string | undefined;
  if (raw.resource !== undefined) {
    if (
      typeof raw.resource !== "string" ||
      raw.resource.length === 0 ||
      raw.resource.length > MAX_RESOURCE_LENGTH ||
      CRLF_RE.test(raw.resource)
    ) {
      return fail("bad_request", "oauth resource is invalid");
    }
    resource = raw.resource;
  }

  const oauth: OAuthMaterial = {
    kind: "oauth",
    accessToken: raw.accessToken,
    tokenEndpoint: tokenUrl.href,
    clientId: raw.clientId,
    ...(refreshToken !== undefined ? { refreshToken } : {}),
    ...(tokenType !== undefined ? { tokenType } : {}),
    ...(expiresAt !== undefined ? { expiresAt } : {}),
    ...(clientSecret !== undefined ? { clientSecret } : {}),
    ...(scopes !== undefined ? { scopes } : {}),
    ...(resource !== undefined ? { resource } : {})
  };
  return ok({ material: oauth, headerKeys: [], scopes, expiresAt: expiresAt ?? null });
};

const toView = (row: ConnectionRow): VaultConnectionView => ({
  id: row.id,
  kind: row.kind === "plugin" ? "plugin" : "mcp",
  name: row.name,
  ...(row.preset ? { preset: row.preset } : {}),
  url: row.url,
  transport: row.transport === "sse" ? "sse" : "http",
  enabled: row.enabled === 1,
  authKind: row.auth_kind as VaultConnectionAuthKind,
  status: row.status as VaultConnectionStatus,
  headerKeys: parseStringArray(row.header_keys),
  scopes: parseStringArray(row.scopes),
  ...(row.account_label ? { accountLabel: row.account_label } : {}),
  authorizedDevices: parseStringArray(row.authorized_devices),
  updatedAt: row.updated_at,
  ...(row.expires_at !== null ? { expiresAt: row.expires_at } : {}),
  ...(row.last_error ? { lastError: row.last_error } : {})
});

export class VaultConnections extends DurableObject<Env> {
  private readonly sql: SqlStorage;
  /** Single-flight OAuth refresh gate keyed by connection id. */
  private readonly refreshing = new Map<string, Promise<RefreshOutcome>>();
  private ownerId: string | undefined;

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);
    this.sql = ctx.storage.sql;
    this.sql.exec("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)");
    this.sql.exec(
      `CREATE TABLE IF NOT EXISTS connections (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        name TEXT NOT NULL,
        preset TEXT,
        url TEXT NOT NULL,
        transport TEXT NOT NULL,
        enabled INTEGER NOT NULL DEFAULT 1,
        auth_kind TEXT NOT NULL,
        status TEXT NOT NULL,
        header_keys TEXT NOT NULL DEFAULT '[]',
        scopes TEXT NOT NULL DEFAULT '[]',
        account_label TEXT,
        authorized_devices TEXT NOT NULL DEFAULT '["*"]',
        generation INTEGER NOT NULL DEFAULT 0,
        envelope TEXT,
        expires_at INTEGER,
        last_refresh_at INTEGER,
        last_attempt_at INTEGER,
        last_error TEXT,
        failures INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      )`
    );
  }

  // ── RPC surface (called only by Vault / VaultApi) ────────────────────────

  list(userId: string, filter?: ListConnectionsFilter): VaultResult<VaultConnectionView[]> {
    const owner = this.claim(userId);
    if (owner) return owner;
    const rows = filter?.kind
      ? this.sql
          .exec<ConnectionRow>("SELECT * FROM connections WHERE kind = ? ORDER BY updated_at DESC, id ASC", filter.kind)
          .toArray()
      : this.sql.exec<ConnectionRow>("SELECT * FROM connections ORDER BY updated_at DESC, id ASC").toArray();
    const out: VaultConnectionView[] = [];
    for (const row of rows) {
      if (filter?.enabledOnly && row.enabled !== 1) continue;
      if (filter?.deviceId !== undefined) {
        const authorized = parseStringArray(row.authorized_devices);
        if (!isDeviceAuthorizedForConnection(authorized, filter.deviceId, filter.parentId)) continue;
      }
      out.push(toView(row));
    }
    return ok(out);
  }

  put(userId: string, id: string, request: PutConnectionRequest): Promise<VaultResult<VaultConnectionView>> {
    const owner = this.claim(userId);
    if (owner) return Promise.resolve(owner);
    return this.guard(async () => {
      if (!CONNECTION_ID_RE.test(id)) {
        return fail("bad_request", "connection id must match ^(mcp|plugin):[a-z0-9][a-z0-9._-]{0,63}$");
      }
      if (!isPlainObject(request)) return fail("bad_request", "request must be an object");

      const name = typeof request.name === "string" ? request.name.trim() : "";
      if (!name || name.length > MAX_NAME_LENGTH || CRLF_RE.test(name)) {
        return fail("bad_request", "name must be a non-empty string");
      }

      let preset: string | null = null;
      if (request.preset !== undefined) {
        if (
          typeof request.preset !== "string" ||
          request.preset.trim().length === 0 ||
          request.preset.length > MAX_PRESET_LENGTH ||
          CRLF_RE.test(request.preset)
        ) {
          return fail("bad_request", "preset must be a non-empty string");
        }
        preset = request.preset.trim();
      }

      let url: URL;
      try {
        url = assertSafeHttpsUrl(request.url, "url", { allowCustomPort: true });
      } catch (error) {
        return fail("bad_request", errorMessage(error));
      }

      const transport: VaultConnectionTransport = request.transport ?? "http";
      if (transport !== "http" && transport !== "sse") {
        return fail("bad_request", "transport must be 'http' or 'sse'");
      }

      const enabled = request.enabled ?? true;
      if (typeof enabled !== "boolean") return fail("bad_request", "enabled must be a boolean");

      const authKind = request.authKind;
      if (authKind !== "none" && authKind !== "headers" && authKind !== "oauth") {
        return fail("bad_request", "authKind must be 'none', 'headers', or 'oauth'");
      }

      const authorizedDevices =
        request.authorizedDevices === undefined ? ["*"] : parseAuthorizedDevices(request.authorizedDevices);
      if (!authorizedDevices) {
        return fail("bad_request", "authorizedDevices must be a list of device ids or '*'");
      }

      let requestScopes: string[] = [];
      if (request.scopes !== undefined) {
        const parsedScopes = parseScopes(request.scopes);
        if (!parsedScopes) return fail("bad_request", "scopes must be a list of scope strings");
        requestScopes = parsedScopes;
      }

      let accountLabel: string | null = null;
      if (request.accountLabel !== undefined) {
        if (typeof request.accountLabel !== "string" || request.accountLabel.length > MAX_LABEL_LENGTH || CRLF_RE.test(request.accountLabel)) {
          return fail("bad_request", "accountLabel is invalid");
        }
        const trimmed = request.accountLabel.trim();
        accountLabel = trimmed || null;
      }

      const validated = validateMaterial(authKind, request.material);
      if (!validated.ok) return validated;
      const { material, headerKeys, scopes: materialScopes, expiresAt } = validated.value;
      const scopes = materialScopes ?? requestScopes;
      const status: VaultConnectionStatus =
        authKind === "none" ? "connected" : material !== null ? "connected" : "needs_auth";

      const keyring = material !== null ? await this.keyring() : undefined;
      const kind = kindFromId(id);

      return this.ctx.blockConcurrencyWhile(async (): Promise<VaultResult<VaultConnectionView>> => {
        const existing = this.row(id);
        if (!existing && this.count() >= MAX_CONNECTIONS) {
          return fail("bad_request", `at most ${MAX_CONNECTIONS} connections per user; delete one first`);
        }
        const generation = this.lastGeneration() + 1;
        const envelope =
          material !== null && keyring ? await sealJson(keyring, material, connectionAad(userId, id, generation)) : null;
        const now = Date.now();
        this.sql.exec(
          `INSERT INTO connections (
             id, kind, name, preset, url, transport, enabled, auth_kind, status,
             header_keys, scopes, account_label, authorized_devices, generation,
             envelope, expires_at, last_refresh_at, last_attempt_at, last_error,
             failures, created_at, updated_at
           )
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, 0, ?, ?)
           ON CONFLICT(id) DO UPDATE SET
             kind = excluded.kind,
             name = excluded.name,
             preset = excluded.preset,
             url = excluded.url,
             transport = excluded.transport,
             enabled = excluded.enabled,
             auth_kind = excluded.auth_kind,
             status = excluded.status,
             header_keys = excluded.header_keys,
             scopes = excluded.scopes,
             account_label = excluded.account_label,
             authorized_devices = excluded.authorized_devices,
             generation = excluded.generation,
             envelope = excluded.envelope,
             expires_at = excluded.expires_at,
             last_refresh_at = NULL,
             last_attempt_at = NULL,
             last_error = NULL,
             failures = 0,
             updated_at = excluded.updated_at`,
          id,
          kind,
          name,
          preset,
          url.href,
          transport,
          enabled ? 1 : 0,
          authKind,
          status,
          JSON.stringify(headerKeys),
          JSON.stringify(scopes),
          accountLabel,
          JSON.stringify(authorizedDevices),
          generation,
          envelope,
          expiresAt,
          existing?.created_at ?? now,
          now
        );
        this.setMeta("last_generation", String(generation));
        return ok(toView(this.row(id)!));
      });
    });
  }

  update(userId: string, id: string, patch: UpdateConnectionRequest): VaultResult<VaultConnectionView> {
    const owner = this.claim(userId);
    if (owner) return owner;
    if (!CONNECTION_ID_RE.test(id)) return fail("bad_request", "malformed connection id");
    if (!isPlainObject(patch)) return fail("bad_request", "patch must be an object");
    const existing = this.row(id);
    if (!existing) return fail("not_found", "no such connection");

    let name = existing.name;
    if (patch.name !== undefined) {
      const trimmed = typeof patch.name === "string" ? patch.name.trim() : "";
      if (!trimmed || trimmed.length > MAX_NAME_LENGTH || CRLF_RE.test(trimmed)) {
        return fail("bad_request", "name must be a non-empty string");
      }
      name = trimmed;
    }

    let urlHref = existing.url;
    if (patch.url !== undefined) {
      if (typeof patch.url !== "string") return fail("bad_request", "url must be a string");
      try {
        urlHref = assertSafeHttpsUrl(patch.url, "url", { allowCustomPort: true }).href;
      } catch (error) {
        return fail("bad_request", errorMessage(error));
      }
    }

    let transport = existing.transport;
    if (patch.transport !== undefined) {
      if (patch.transport !== "http" && patch.transport !== "sse") {
        return fail("bad_request", "transport must be 'http' or 'sse'");
      }
      transport = patch.transport;
    }

    let enabled = existing.enabled;
    if (patch.enabled !== undefined) {
      if (typeof patch.enabled !== "boolean") return fail("bad_request", "enabled must be a boolean");
      enabled = patch.enabled ? 1 : 0;
    }

    let authorizedDevicesJson = existing.authorized_devices;
    if (patch.authorizedDevices !== undefined) {
      const devices = parseAuthorizedDevices(patch.authorizedDevices);
      if (!devices) return fail("bad_request", "authorizedDevices must be a list of device ids or '*'");
      authorizedDevicesJson = JSON.stringify(devices);
    }

    let accountLabel = existing.account_label;
    if (patch.accountLabel !== undefined) {
      if (patch.accountLabel === null) {
        accountLabel = null;
      } else if (
        typeof patch.accountLabel !== "string" ||
        patch.accountLabel.length > MAX_LABEL_LENGTH ||
        CRLF_RE.test(patch.accountLabel)
      ) {
        return fail("bad_request", "accountLabel must be a string or null");
      } else {
        const trimmed = patch.accountLabel.trim();
        accountLabel = trimmed || null;
      }
    }

    const now = Date.now();
    this.sql.exec(
      `UPDATE connections
       SET name = ?, url = ?, transport = ?, enabled = ?, authorized_devices = ?, account_label = ?, updated_at = ?
       WHERE id = ?`,
      name,
      urlHref,
      transport,
      enabled,
      authorizedDevicesJson,
      accountLabel,
      now,
      id
    );
    return ok(toView(this.row(id)!));
  }

  delete(userId: string, id: string): VaultResult<{ readonly deleted: boolean }> {
    const owner = this.claim(userId);
    if (owner) return owner;
    if (!CONNECTION_ID_RE.test(id)) return fail("bad_request", "malformed connection id");
    const removed = this.sql.exec("DELETE FROM connections WHERE id = ?", id).rowsWritten > 0;
    return ok({ deleted: removed });
  }

  disconnectAuth(userId: string, id: string): Promise<VaultResult<VaultConnectionView>> {
    const owner = this.claim(userId);
    if (owner) return Promise.resolve(owner);
    if (!CONNECTION_ID_RE.test(id)) return Promise.resolve(fail("bad_request", "malformed connection id"));
    return this.ctx.blockConcurrencyWhile(async (): Promise<VaultResult<VaultConnectionView>> => {
      const existing = this.row(id);
      if (!existing) return fail("not_found", "no such connection");
      const generation = this.lastGeneration() + 1;
      const now = Date.now();
      this.sql.exec(
        `UPDATE connections
         SET generation = ?, status = 'needs_auth', envelope = NULL, header_keys = '[]',
             expires_at = NULL, last_refresh_at = NULL, last_attempt_at = NULL,
             last_error = NULL, failures = 0, updated_at = ?
         WHERE id = ?`,
        generation,
        now,
        id
      );
      this.setMeta("last_generation", String(generation));
      return ok(toView(this.row(id)!));
    });
  }

  grant(
    userId: string,
    id: string,
    deviceId: string,
    parentId?: string,
    forceRefresh?: boolean
  ): Promise<VaultResult<ConnectionGrant>> {
    const owner = this.claim(userId);
    if (owner) return Promise.resolve(owner);
    return this.guard(() => this.grantOne(id, deviceId, parentId, Boolean(forceRefresh)));
  }

  grantMany(
    userId: string,
    ids: readonly string[],
    deviceId: string,
    parentId?: string
  ): Promise<VaultResult<ConnectionGrantManyReply>> {
    const owner = this.claim(userId);
    if (owner) return Promise.resolve(owner);
    return this.guard(async () => {
      const results = await Promise.all(
        ids.map(async (id) => ({
          id,
          result: await this.guard(() => this.grantOne(id, deviceId, parentId, false))
        }))
      );
      const grants: ConnectionGrant[] = [];
      const missing: { id: string; error: VaultFailure["error"]; message: string }[] = [];
      for (const { id, result } of results) {
        if (result.ok) grants.push(result.value);
        else missing.push({ id, error: result.error, message: result.message });
      }
      return ok({ grants, missing });
    });
  }

  // ── grant & refresh internals ────────────────────────────────────────────

  private async grantOne(
    id: string,
    deviceId: string,
    parentId: string | undefined,
    forceRefresh: boolean
  ): Promise<VaultResult<ConnectionGrant>> {
    let refreshed = false;
    for (let pass = 0; pass < 4; pass++) {
      const row = this.row(id);
      if (!row) return fail("not_found", "no such connection");
      if (row.enabled !== 1) return fail("not_authorized", "connection is disabled");
      const authorized = parseStringArray(row.authorized_devices);
      if (!isDeviceAuthorizedForConnection(authorized, deviceId, parentId)) {
        return fail("not_authorized", "device is not authorized for this connection");
      }

      const transport: VaultConnectionTransport = row.transport === "sse" ? "sse" : "http";
      if (row.auth_kind === "none") {
        if (row.status !== "connected") return fail("needs_reconnect", "connection needs authentication");
        return ok({ id: row.id, url: row.url, transport, headers: {}, generation: row.generation });
      }

      if (row.status === "needs_auth") return fail("needs_reconnect", "connection requires authentication");
      if (row.status === "reauth_required") {
        return fail(
          "needs_reconnect",
          row.last_error ? `connection requires re-authentication: ${row.last_error}` : "connection requires re-authentication"
        );
      }
      if (row.status === "error") return fail("upstream", row.last_error ?? "connection is in an error state");
      if (!row.envelope) return fail("needs_reconnect", "connection has no stored credentials");

      const material = await this.open(row);
      if (this.row(id)?.generation !== row.generation) continue;

      if (row.auth_kind === "headers") {
        if (material.kind !== "headers") {
          throw new RecordUnreadable(`connection ${id} material kind mismatch`);
        }
        return ok({
          id: row.id,
          url: row.url,
          transport,
          headers: { ...material.headers },
          generation: row.generation
        });
      }

      if (material.kind !== "oauth") {
        throw new RecordUnreadable(`connection ${id} material kind mismatch`);
      }

      const now = Date.now();
      const hasRefresh = Boolean(material.refreshToken);
      const expiringSoon = material.expiresAt !== undefined && material.expiresAt - now <= OAUTH_REFRESH_LEAD_MS;
      const shouldRefresh = !refreshed && hasRefresh && (forceRefresh || expiringSoon);

      const stillValid = material.expiresAt === undefined || material.expiresAt > now;
      if (shouldRefresh && !forceRefresh && stillValid && now - (row.last_attempt_at ?? 0) < CONNECTION_REFRESH_BACKOFF_MS) {
        return ok(this.oauthGrant(row, transport, material));
      }

      if (!shouldRefresh) {
        if (material.expiresAt !== undefined && material.expiresAt <= now && !hasRefresh) {
          this.markReauth(row.id, row.generation, "access token expired");
          return fail("needs_reconnect", "access token expired and no refresh token is stored");
        }
        return ok(this.oauthGrant(row, transport, material));
      }

      const done = await this.refreshOnce(row, material);
      if (done.kind === "ok") {
        refreshed = true;
        continue;
      }
      if (done.kind === "reauth") return fail("needs_reconnect", `oauth refresh rejected: ${done.reason}`);
      if (done.kind === "ssrf") return fail("bad_request", done.reason);
      if (this.row(id)?.generation !== row.generation) continue;
      if (!forceRefresh && (material.expiresAt === undefined || material.expiresAt > Date.now())) {
        return ok(this.oauthGrant(row, transport, material));
      }
      return fail("upstream", `oauth refresh failed: ${done.reason}`);
    }
    return fail("unavailable", "connection changed concurrently; retry");
  }

  private oauthGrant(
    row: ConnectionRow,
    transport: VaultConnectionTransport,
    material: OAuthMaterial
  ): ConnectionGrant {
    const tokenType = material.tokenType || "Bearer";
    return {
      id: row.id,
      url: row.url,
      transport,
      headers: { Authorization: `${tokenType} ${material.accessToken}` },
      ...(material.expiresAt !== undefined ? { expiresAt: material.expiresAt } : {}),
      generation: row.generation
    };
  }

  /** At most one OAuth refresh in flight per connection id; concurrent callers share it. */
  private refreshOnce(row: ConnectionRow, material: OAuthMaterial): Promise<RefreshOutcome> {
    const inflight = this.refreshing.get(row.id);
    if (inflight) return inflight;
    const run = this.runRefresh(row, material).finally(() => {
      if (this.refreshing.get(row.id) === run) this.refreshing.delete(row.id);
    });
    this.refreshing.set(row.id, run);
    return run;
  }

  private async runRefresh(row: ConnectionRow, material: OAuthMaterial): Promise<RefreshOutcome> {
    if (!material.refreshToken) return { kind: "transient", reason: "no refresh token stored" };
    let tokenUrl: URL;
    try {
      tokenUrl = assertSafeHttpsUrl(material.tokenEndpoint, "tokenEndpoint");
    } catch (error) {
      const reason = error instanceof UnsafeUrlError ? error.message : errorMessage(error);
      await this.audit({ event: "conn_refresh_failed", detail: `${row.id} ssrf: ${reason}` });
      return { kind: "ssrf", reason };
    }

    const keyring = await this.keyring();
    const params = new URLSearchParams({
      grant_type: "refresh_token",
      refresh_token: material.refreshToken,
      client_id: material.clientId
    });
    if (material.clientSecret) params.set("client_secret", material.clientSecret);
    if (material.scopes && material.scopes.length > 0) params.set("scope", material.scopes.join(" "));
    if (material.resource) params.set("resource", material.resource);

    let outcome:
      | { readonly kind: "ok"; readonly next: OAuthMaterial }
      | { readonly kind: "reauth"; readonly reason: string }
      | { readonly kind: "transient"; readonly reason: string };

    try {
      const response = await fetch(tokenUrl.href, {
        method: "POST",
        redirect: "manual",
        headers: {
          "content-type": "application/x-www-form-urlencoded",
          accept: "application/json"
        },
        body: params.toString()
      });

      if (response.status >= 300 && response.status < 400) {
        outcome = { kind: "transient", reason: `token endpoint redirect refused (${response.status})` };
      } else {
        let body: Record<string, unknown> | undefined;
        try {
          const parsed: unknown = await response.json();
          if (isPlainObject(parsed)) body = parsed;
        } catch {
          body = undefined;
        }

        if (response.ok) {
          const accessToken = typeof body?.access_token === "string" ? body.access_token : "";
          if (!accessToken || accessToken.length > MAX_TOKEN_LENGTH || CRLF_RE.test(accessToken)) {
            outcome = { kind: "transient", reason: "token response missing valid access_token" };
          } else {
            const nextRefresh =
              typeof body?.refresh_token === "string" && body.refresh_token.length > 0 && body.refresh_token.length <= MAX_TOKEN_LENGTH
                ? body.refresh_token
                : material.refreshToken;
            const nextTokenType =
              typeof body?.token_type === "string" && TOKEN_TYPE_RE.test(body.token_type)
                ? body.token_type
                : material.tokenType;
            const rawExpiresIn = body?.expires_in;
            const expiresInSecs =
              typeof rawExpiresIn === "number"
                ? rawExpiresIn
                : typeof rawExpiresIn === "string" && /^\d+(?:\.\d+)?$/.test(rawExpiresIn)
                  ? Number(rawExpiresIn)
                  : undefined;
            const nextExpiresAt =
              expiresInSecs !== undefined && Number.isFinite(expiresInSecs) && expiresInSecs > 0
                ? Date.now() + Math.floor(expiresInSecs * 1000)
                : undefined;
            const nextScopes =
              typeof body?.scope === "string" && body.scope.trim().length > 0
                ? parseScopes(body.scope.trim().split(/\s+/)) ?? material.scopes
                : material.scopes;

            outcome = {
              kind: "ok",
              next: {
                kind: "oauth",
                accessToken,
                refreshToken: nextRefresh,
                tokenEndpoint: tokenUrl.href,
                clientId: material.clientId,
                ...(nextTokenType !== undefined ? { tokenType: nextTokenType } : {}),
                ...(nextExpiresAt !== undefined ? { expiresAt: nextExpiresAt } : {}),
                ...(material.clientSecret !== undefined ? { clientSecret: material.clientSecret } : {}),
                ...(nextScopes !== undefined ? { scopes: nextScopes } : {}),
                ...(material.resource !== undefined ? { resource: material.resource } : {})
              }
            };
          }
        } else {
          const oauthError = typeof body?.error === "string" ? body.error : undefined;
          const oauthDesc = typeof body?.error_description === "string" ? body.error_description : undefined;
          const reason = (oauthDesc ? `${oauthError ?? `HTTP ${response.status}`}: ${oauthDesc}` : oauthError ?? `HTTP ${response.status}`).slice(0, 200);
          const isReauth =
            response.status === 401 ||
            (response.status === 400 &&
              (oauthError === "invalid_grant" || oauthError === "unauthorized_client" || oauthError === "access_denied"));
          outcome = isReauth ? { kind: "reauth", reason } : { kind: "transient", reason };
        }
      }
    } catch (error) {
      outcome = { kind: "transient", reason: errorMessage(error).slice(0, 200) };
    }

    let committedGeneration = row.generation;
    const done = await this.ctx.blockConcurrencyWhile(async (): Promise<RefreshOutcome> => {
      const current = this.row(row.id);
      // Superseded by a concurrent put, disconnect, or delete: drop refreshed tokens.
      if (!current || current.generation !== row.generation) return { kind: "ok" };
      const now = Date.now();
      switch (outcome.kind) {
        case "ok": {
          committedGeneration = this.lastGeneration() + 1;
          const envelope = await sealJson(keyring, outcome.next, connectionAad(this.userId, row.id, committedGeneration));
          const scopes = outcome.next.scopes ?? parseStringArray(current.scopes);
          this.sql.exec(
            `UPDATE connections
             SET generation = ?, envelope = ?, status = 'connected', scopes = ?,
                 expires_at = ?, last_refresh_at = ?, last_attempt_at = ?,
                 last_error = NULL, failures = 0, updated_at = ?
             WHERE id = ?`,
            committedGeneration,
            envelope,
            JSON.stringify(scopes),
            outcome.next.expiresAt ?? null,
            now,
            now,
            now,
            row.id
          );
          this.setMeta("last_generation", String(committedGeneration));
          return { kind: "ok" };
        }
        case "reauth":
          this.sql.exec(
            `UPDATE connections
             SET status = 'reauth_required', last_attempt_at = ?, last_error = ?,
                 failures = failures + 1, updated_at = ?
             WHERE id = ?`,
            now,
            outcome.reason,
            now,
            row.id
          );
          return outcome;
        case "transient":
          this.sql.exec(
            `UPDATE connections
             SET last_attempt_at = ?, last_error = ?, failures = failures + 1
             WHERE id = ?`,
            now,
            outcome.reason,
            row.id
          );
          return outcome;
      }
    });

    const detail =
      done.kind === "ok" ? `${row.id} generation ${committedGeneration}` : `${row.id} ${done.kind}: ${done.reason}`;
    console.log(JSON.stringify({ event: "vault.connection.refresh", id: row.id, ok: done.kind === "ok", detail }));
    await this.audit({ event: done.kind === "ok" ? "conn_refresh" : "conn_refresh_failed", detail });
    return done;
  }

  // ── crypto & storage helpers ─────────────────────────────────────────────

  private async open(row: ConnectionRow): Promise<VaultConnectionMaterial> {
    if (!row.envelope) throw new RecordUnreadable(`connection ${row.id} has no envelope`);
    const keyring = await this.keyring();
    const aad = connectionAad(this.userId, row.id, row.generation);
    let opened: Awaited<ReturnType<typeof openJson<VaultConnectionMaterial>>>;
    try {
      opened = await openJson<VaultConnectionMaterial>(keyring, row.envelope, aad);
    } catch (error) {
      if (error instanceof VaultUnavailable) throw error;
      throw new RecordUnreadable(`connection ${row.id} (generation ${row.generation}) could not be decrypted`);
    }
    if (opened.opened.rewrap) {
      const envelope = await seal(keyring, opened.opened.plaintext, aad);
      this.sql.exec(
        "UPDATE connections SET envelope = ? WHERE id = ? AND generation = ? AND envelope = ?",
        envelope,
        row.id,
        row.generation,
        row.envelope
      );
      console.log(
        JSON.stringify({
          event: "vault.rewrap",
          connectionId: row.id,
          from: opened.opened.kid,
          to: keyring.current.kid
        })
      );
    }
    return opened.value;
  }

  private markReauth(id: string, generation: number, reason: string): void {
    this.sql.exec(
      "UPDATE connections SET status = 'reauth_required', last_error = ?, updated_at = ? WHERE id = ? AND generation = ?",
      reason,
      Date.now(),
      id,
      generation
    );
  }

  private keyring(): Promise<Keyring> {
    return loadKeyring(this.env);
  }

  private async guard<T>(run: () => Promise<VaultResult<T>>): Promise<VaultResult<T>> {
    try {
      return await run();
    } catch (error) {
      if (error instanceof VaultUnavailable) return fail("unavailable", error.message, 503);
      if (error instanceof RecordUnreadable) {
        console.error(JSON.stringify({ event: "vault.connection.unreadable", message: error.message }));
        return fail("unavailable", error.message, 500);
      }
      console.error(JSON.stringify({ event: "vault.connection.error", message: errorMessage(error) }));
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

  private claim(userId: string): VaultFailure | undefined {
    const owner = this.getMeta("owner");
    if (owner === undefined) {
      this.setMeta("owner", userId);
    } else if (owner !== userId) {
      return fail("forbidden", "vault object belongs to another user");
    }
    this.ownerId = userId;
    return undefined;
  }

  private get userId(): string {
    return this.ownerId ?? this.getMeta("owner") ?? "";
  }

  private row(id: string): ConnectionRow | undefined {
    return this.sql.exec<ConnectionRow>("SELECT * FROM connections WHERE id = ?", id).toArray()[0];
  }

  private count(): number {
    return this.sql.exec<{ count: number }>("SELECT COUNT(*) AS count FROM connections").toArray()[0]?.count ?? 0;
  }

  private lastGeneration(): number {
    const raw = this.getMeta("last_generation");
    const parsed = raw ? Number.parseInt(raw, 10) : 0;
    return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : 0;
  }

  private getMeta(key: string): string | undefined {
    return this.sql.exec<{ value: string }>("SELECT value FROM meta WHERE key = ?", key).toArray()[0]?.value;
  }

  private setMeta(key: string, value: string): void {
    this.sql.exec(
      "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
      key,
      value
    );
  }
}
