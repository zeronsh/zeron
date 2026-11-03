/**
 * Claude (subscription) — the credential the unmodified Claude Code CLI uses,
 * for running Claude Code on the user's own Cloud device (permitted for Zeron
 * under its arrangement with Anthropic; grants go only to the user's own
 * devices). Two upload shapes:
 *
 * - **OAuth login** `{claudeAiOauth: {accessToken, refreshToken, expiresAt,
 *   scopes, subscriptionType?}}` — the CLI's credential blob. Access tokens are
 *   short-lived and refresh tokens ROTATE on every refresh, so this is exactly
 *   the race VaultAccount's single-flight gate exists for: from upload on, the
 *   vault must be the only refresher. Refreshed when under 10 minutes remain.
 * - **Setup token** `{oauthToken: "sk-ant-oat…", expiresAt?}` from
 *   `claude setup-token` — long-lived (default one year from upload), never
 *   refreshed; past its expiry the account needs a reconnect.
 */
import type { VaultMaterial } from "../api";
import {
  DAY_MS,
  isObject,
  nonEmptyString,
  readJsonObject,
  upstream,
  type ParseResult,
  type ProviderAdapter,
  type RefreshContext,
  type RefreshOutcome
} from "./types";

export const CLAUDE_CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
export const CLAUDE_TOKEN_URL = "https://platform.claude.com/v1/oauth/token";
const REFRESH_BEFORE_EXPIRY_MS = 10 * 60_000;
const DEFAULT_ACCESS_LIFETIME_SECS = 3600;
const SETUP_TOKEN_LIFETIME_MS = 365 * DAY_MS;
const SETUP_TOKEN_RE = /^sk-ant-oat[\x21-\x7e]{8,500}$/;
/** `subscriptionType` doubles as the account label: keep it a plain word. */
const SUBSCRIPTION_RE = /^[A-Za-z0-9_-]{1,32}$/;

export type ClaudeSecret =
  | {
      readonly kind: "oauth";
      readonly accessToken: string;
      readonly refreshToken: string;
      /** Unix ms. */
      readonly expiresAt: number;
      readonly scopes: readonly string[];
      readonly subscriptionType?: string;
      readonly lastRefresh: number;
    }
  | {
      readonly kind: "setup-token";
      readonly token: string;
      /** Unix ms. */
      readonly expiresAt: number;
    };

const positive = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;

const scopeList = (value: unknown): string[] | undefined =>
  Array.isArray(value) && value.every((scope) => typeof scope === "string") ? (value as string[]) : undefined;

export const parseClaudeUpload = (material: VaultMaterial, now: number): ParseResult<ClaudeSecret> => {
  if ("claudeAiOauth" in material) {
    const blob = material.claudeAiOauth;
    if (!isObject(blob)) return { ok: false, message: "claudeAiOauth must be an object" };
    const expiresAt = positive(blob.expiresAt);
    if (!nonEmptyString(blob.accessToken) || !nonEmptyString(blob.refreshToken) || expiresAt === undefined) {
      return { ok: false, message: "claudeAiOauth needs accessToken, refreshToken and expiresAt (Unix ms)" };
    }
    const scopes = blob.scopes === undefined ? [] : scopeList(blob.scopes);
    if (!scopes) return { ok: false, message: "claudeAiOauth.scopes must be a list of strings" };
    const subscriptionType =
      typeof blob.subscriptionType === "string" && SUBSCRIPTION_RE.test(blob.subscriptionType)
        ? blob.subscriptionType
        : undefined;
    return {
      ok: true,
      secret: {
        kind: "oauth",
        accessToken: blob.accessToken,
        refreshToken: blob.refreshToken,
        expiresAt,
        scopes,
        ...(subscriptionType ? { subscriptionType } : {}),
        lastRefresh: now
      }
    };
  }
  if ("oauthToken" in material) {
    if (typeof material.oauthToken !== "string" || !SETUP_TOKEN_RE.test(material.oauthToken.trim())) {
      return { ok: false, message: "oauthToken must be a `claude setup-token` token (sk-ant-oat…)" };
    }
    if (material.expiresAt !== undefined && positive(material.expiresAt) === undefined) {
      return { ok: false, message: "expiresAt must be Unix ms" };
    }
    const expiresAt = positive(material.expiresAt) ?? now + SETUP_TOKEN_LIFETIME_MS;
    if (expiresAt <= now) return { ok: false, message: "oauthToken has already expired" };
    return { ok: true, secret: { kind: "setup-token", token: material.oauthToken.trim(), expiresAt } };
  }
  return { ok: false, message: "claude material must be {claudeAiOauth} or {oauthToken, expiresAt?}" };
};

export const claudeRefreshDue = (secret: ClaudeSecret, now: number): boolean =>
  secret.kind === "oauth" && secret.expiresAt - now < REFRESH_BEFORE_EXPIRY_MS;

/**
 * Classify the token endpoint's answer. A refresh that returns no new refresh
 * token keeps the old one; no `scope` keeps the old scopes. Errors arrive as
 * OAuth `{error: "invalid_grant"}` or Anthropic's `{error: {type: …}}`.
 */
export const parseClaudeRefresh = (
  status: number,
  body: Record<string, unknown> | undefined,
  previous: Extract<ClaudeSecret, { kind: "oauth" }>,
  now: number
): RefreshOutcome<ClaudeSecret> => {
  if (status >= 200 && status < 300) {
    if (!body || !nonEmptyString(body.access_token)) {
      return { kind: "transient", reason: "refresh response carried no access_token" };
    }
    const expiresIn = positive(body.expires_in) ?? DEFAULT_ACCESS_LIFETIME_SECS;
    return {
      kind: "ok",
      secret: {
        ...previous,
        accessToken: body.access_token,
        refreshToken: nonEmptyString(body.refresh_token) ? body.refresh_token : previous.refreshToken,
        expiresAt: now + expiresIn * 1000,
        scopes: nonEmptyString(body.scope) ? body.scope.split(" ").filter(Boolean) : previous.scopes,
        lastRefresh: now
      }
    };
  }
  const error = body?.error;
  const code = typeof error === "string" ? error : isObject(error) && typeof error.type === "string" ? error.type : "";
  if (status === 401 || code === "invalid_grant") return { kind: "reconnect", reason: code || `HTTP ${status}` };
  return { kind: "transient", reason: `HTTP ${status}${code ? ` ${code}` : ""}` };
};

export const refreshClaude = async (secret: ClaudeSecret, context: RefreshContext): Promise<RefreshOutcome<ClaudeSecret>> => {
  if (secret.kind !== "oauth") return { kind: "reconnect", reason: "setup tokens do not refresh" };
  const response = await upstream(context.fetch, CLAUDE_TOKEN_URL, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json" },
    body: JSON.stringify({
      grant_type: "refresh_token",
      refresh_token: secret.refreshToken,
      client_id: CLAUDE_CLIENT_ID
    })
  });
  if (!response) return { kind: "transient", reason: "timeout or network error" };
  return parseClaudeRefresh(response.status, await readJsonObject(response), secret, context.now);
};

export const claudeAdapter: ProviderAdapter<ClaudeSecret> = {
  id: "claude",
  parseUpload: parseClaudeUpload,
  account: (secret) => (secret.kind === "oauth" && secret.subscriptionType) || "Claude subscription",
  refreshable: (secret) => secret.kind === "oauth",
  refreshDue: claudeRefreshDue,
  refresh: refreshClaude,
  hardExpiresAt: (secret) => (secret.kind === "setup-token" ? secret.expiresAt : undefined),
  grant: (secret) =>
    secret.kind === "oauth"
      ? {
          accessToken: secret.accessToken,
          expiresAt: secret.expiresAt,
          scopes: secret.scopes,
          ...(secret.subscriptionType ? { subscriptionType: secret.subscriptionType } : {})
        }
      : { accessToken: secret.token, expiresAt: secret.expiresAt }
};
