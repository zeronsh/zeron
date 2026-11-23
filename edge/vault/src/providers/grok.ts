/**
 * Grok sign-ins (`grok`): one entry of the `auth.json` that `grok login
 * --device-auth` writes, keyed `{issuer}::{client_id}`. It is an xAI OIDC
 * token set — `key` (the access token), `refresh_token`, `expires_at`
 * (RFC 3339), `oidc_issuer`, `oidc_client_id` — beside the account's
 * identity (`user_id`, `email`, names, `subscription_tier`, …).
 *
 * From upload on the vault is the only refresher: `POST
 * {oidc_issuer}/oauth2/token` (form: `grant_type=refresh_token`, the entry's
 * own client id), the issuer pinned to an `https` `x.ai` host so the refresh
 * token never goes anywhere else. Refresh tokens may rotate; the account DO's
 * single-flight gate keeps one refresh in flight. Machines get the access
 * token plus the entry's non-secret fields (`settings`) and write the entry
 * back with an empty refresh token, ahead of its expiry.
 */
import {
  isObject,
  nonEmptyString,
  readJsonObject,
  upstream,
  type ParseResult,
  type ProviderAdapter,
  type RefreshContext,
  type RefreshOutcome
} from "./types";

export interface GrokSecret {
  /** The entry's map key: `{issuer}::{client_id}`. */
  readonly scope: string;
  readonly issuer: string;
  readonly clientId: string;
  readonly accessToken: string;
  readonly refreshToken: string;
  /** Unix ms; 0 when the upload named none (refreshed at the first grant). */
  readonly expiresAt: number;
  /** Unix ms the access token was issued (a refresh, or the entry's
   * `create_time`): its lifetime is `expiresAt - issuedAt`. 0 = unknown (an
   * upload's token may be nearly spent, whatever time it arrives). */
  readonly issuedAt: number;
  /** The entry's other string fields (identity, plan), as `grok` wrote them. */
  readonly fields: Readonly<Record<string, string>>;
}

const SECRET_FIELDS = ["key", "refresh_token", "expires_at"] as const;
const FIELD_RE = /^[a-z][a-z0-9_]{0,63}$/;
const MAX_FIELDS = 32;
const MAX_VALUE = 1024;
const MAX_SCOPE = 512;
const DEFAULT_LIFETIME_SECS = 3600;
/** Refresh once less than this (or half the token's life) remains. */
const REFRESH_BEFORE_EXPIRY_MS = 30 * 60_000;

/** `https://x.ai` or a subdomain, no port, credentials, query or path: the
 * only place a Grok refresh token is ever sent. */
export const isXaiIssuer = (value: string): boolean => {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" || url.username || url.password || url.port || url.search || url.hash) return false;
  if (url.pathname !== "/" && url.pathname !== "") return false;
  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  return host === "x.ai" || host.endsWith(".x.ai");
};

const issuerBase = (issuer: string): string => issuer.replace(/\/+$/, "");

export const parseGrokUpload = (material: unknown, now: number): ParseResult<GrokSecret> => {
  if (!isObject(material) || !isObject(material.grokAuth) || typeof material.scope !== "string") {
    return { ok: false, message: "grok material must be {grokAuth, scope}" };
  }
  const entry = material.grokAuth;
  const scope = material.scope;
  const split = scope.lastIndexOf("::");
  if (scope.length > MAX_SCOPE || split <= 0 || split + 2 >= scope.length) {
    return { ok: false, message: "grok scope must be {issuer}::{client_id}" };
  }
  if (!nonEmptyString(entry.key) || !nonEmptyString(entry.refresh_token)) {
    return { ok: false, message: "grok entry needs key and refresh_token" };
  }
  const issuer = issuerBase(nonEmptyString(entry.oidc_issuer) ? entry.oidc_issuer : scope.slice(0, split));
  if (!isXaiIssuer(issuer)) return { ok: false, message: "grok issuer must be an https x.ai host" };
  const clientId = nonEmptyString(entry.oidc_client_id) ? entry.oidc_client_id : scope.slice(split + 2);
  if (!/^[A-Za-z0-9._:-]{1,128}$/.test(clientId)) return { ok: false, message: "grok client id is malformed" };
  const fields: Record<string, string> = {};
  let count = 0;
  for (const [name, value] of Object.entries(entry)) {
    if (typeof value !== "string" || (SECRET_FIELDS as readonly string[]).includes(name)) continue;
    if (!FIELD_RE.test(name) || value.length > MAX_VALUE) continue;
    if (++count > MAX_FIELDS) break;
    fields[name] = value;
  }
  fields.oidc_issuer = issuer;
  fields.oidc_client_id = clientId;
  const parsed = typeof entry.expires_at === "string" ? Date.parse(entry.expires_at) : Number.NaN;
  const expiresAt = Number.isFinite(parsed) && parsed > 0 ? parsed : 0;
  const created = typeof entry.create_time === "string" ? Date.parse(entry.create_time) : Number.NaN;
  const issuedAt = Number.isFinite(created) && created > 0 && created <= now && created < expiresAt ? created : 0;
  return {
    ok: true,
    secret: {
      scope,
      issuer,
      clientId,
      accessToken: entry.key,
      refreshToken: entry.refresh_token,
      expiresAt,
      issuedAt,
      fields
    }
  };
};

export const grokRefreshDue = (secret: GrokSecret, now: number): boolean => {
  const lifetime = secret.issuedAt > 0 ? secret.expiresAt - secret.issuedAt : 0;
  const ahead = lifetime > 0 ? Math.min(REFRESH_BEFORE_EXPIRY_MS, lifetime / 2) : REFRESH_BEFORE_EXPIRY_MS;
  return secret.expiresAt - now < ahead;
};

/** Classify the token endpoint's answer (standard OAuth errors). */
export const parseGrokRefresh = (
  status: number,
  body: Record<string, unknown> | undefined,
  previous: GrokSecret,
  now: number
): RefreshOutcome<GrokSecret> => {
  if (status >= 200 && status < 300) {
    if (!body || !nonEmptyString(body.access_token)) {
      return { kind: "transient", reason: "refresh response carried no access_token" };
    }
    const expiresIn =
      typeof body.expires_in === "number" && Number.isFinite(body.expires_in) && body.expires_in > 0
        ? body.expires_in
        : DEFAULT_LIFETIME_SECS;
    return {
      kind: "ok",
      secret: {
        ...previous,
        accessToken: body.access_token,
        refreshToken: nonEmptyString(body.refresh_token) ? body.refresh_token : previous.refreshToken,
        expiresAt: now + expiresIn * 1000,
        issuedAt: now
      }
    };
  }
  const code = typeof body?.error === "string" ? body.error : "";
  if (status === 401 || code === "invalid_grant" || code === "invalid_client" || code === "unauthorized_client") {
    return { kind: "reconnect", reason: code || `HTTP ${status}` };
  }
  return { kind: "transient", reason: `HTTP ${status}${code ? ` ${code}` : ""}` };
};

export const refreshGrok = async (secret: GrokSecret, context: RefreshContext): Promise<RefreshOutcome<GrokSecret>> => {
  // Checked again here: a record from before a policy change never sends its
  // refresh token somewhere the upload check would now refuse.
  if (!isXaiIssuer(secret.issuer)) return { kind: "reconnect", reason: "issuer is not an xAI host" };
  const response = await upstream(context.fetch, `${issuerBase(secret.issuer)}/oauth2/token`, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json" },
    body: new URLSearchParams({
      grant_type: "refresh_token",
      refresh_token: secret.refreshToken,
      client_id: secret.clientId
    }).toString()
  });
  if (!response) return { kind: "transient", reason: "timeout or network error" };
  return parseGrokRefresh(response.status, await readJsonObject(response), secret, context.now);
};

const label = (secret: GrokSecret): string =>
  secret.fields.email || secret.fields.user_id || secret.fields.principal_id || "Grok account";

export const grokAdapter: ProviderAdapter<GrokSecret> = {
  id: "grok",
  parseUpload: (material, now) => parseGrokUpload(material, now),
  account: label,
  identity: async (secret) => {
    const who = secret.fields.user_id || secret.fields.principal_id || secret.fields.email;
    return `grok:${(who ?? `oidc:${secret.scope}`).toLowerCase()}`;
  },
  refreshable: () => true,
  refreshDue: grokRefreshDue,
  refresh: refreshGrok,
  grant: (secret) => ({
    accessToken: secret.accessToken,
    expiresAt: secret.expiresAt,
    account: label(secret),
    settings: { ...secret.fields, scope: secret.scope }
  })
};
