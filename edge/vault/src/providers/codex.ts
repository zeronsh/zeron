/**
 * Codex (ChatGPT OAuth) — material is the `auth.json` that `codex login`
 * writes into a throwaway CODEX_HOME on the laptop. From upload on, the vault
 * is the ONLY refresher of that token set: OpenAI rotates refresh tokens and
 * rejects reuse, so a second refresher would sign the user out.
 *
 * Refresh policy: when the access token's JWT `exp` is under an hour away, or
 * — for a token without `exp` — when the last refresh is over 7 days old (the
 * CLI's own staleness rule).
 */
import type { VaultMaterial } from "../api";
import { decodeJwtPayload, jwtExpMs } from "../jwt";
import {
  DAY_MS,
  HOUR_MS,
  isObject,
  nonEmptyString,
  readJsonObject,
  upstream,
  type ParseResult,
  type ProviderAdapter,
  type RefreshContext,
  type RefreshOutcome
} from "./types";

export const CODEX_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann";
export const CODEX_TOKEN_URL = "https://auth.openai.com/oauth/token";
const REFRESH_BEFORE_EXPIRY_MS = HOUR_MS;
const STALE_WITHOUT_EXP_MS = 7 * DAY_MS;

export interface CodexSecret {
  readonly idToken: string;
  readonly accessToken: string;
  readonly refreshToken: string;
  readonly accountId?: string;
  /** Unix ms of the last successful refresh (auth.json `last_refresh` at upload). */
  readonly lastRefresh: number;
}

/** OpenAI error codes that mean the refresh token itself is dead. */
const PERMANENT_CODES = new Set([
  "invalid_grant",
  "refresh_token_expired",
  "refresh_token_reused",
  "refresh_token_invalidated"
]);

/** ChatGPT account id from the id_token's namespaced auth claim. */
const accountIdFromIdToken = (idToken: string): string | undefined => {
  const auth = decodeJwtPayload(idToken)?.["https://api.openai.com/auth"];
  return isObject(auth) && nonEmptyString(auth.chatgpt_account_id) ? auth.chatgpt_account_id : undefined;
};

export const codexEmail = (idToken: string): string | undefined => {
  const claims = decodeJwtPayload(idToken);
  if (!claims) return undefined;
  if (nonEmptyString(claims.email)) return claims.email;
  const profile = claims["https://api.openai.com/profile"];
  return isObject(profile) && nonEmptyString(profile.email) ? profile.email : undefined;
};

export const parseCodexUpload = (material: VaultMaterial, now: number): ParseResult<CodexSecret> => {
  if (!("authJson" in material) || !isObject(material.authJson)) {
    return { ok: false, message: "codex material must be {authJson}" };
  }
  const tokens = material.authJson.tokens;
  if (!isObject(tokens)) return { ok: false, message: "auth.json has no tokens (API-key logins are not uploadable)" };
  const { id_token, access_token, refresh_token, account_id } = tokens;
  if (!nonEmptyString(id_token) || !nonEmptyString(access_token) || !nonEmptyString(refresh_token)) {
    return { ok: false, message: "auth.json tokens need id_token, access_token and refresh_token" };
  }
  const lastRefreshRaw = material.authJson.last_refresh;
  const lastRefresh = typeof lastRefreshRaw === "string" ? Date.parse(lastRefreshRaw) : Number.NaN;
  return {
    ok: true,
    secret: {
      idToken: id_token,
      accessToken: access_token,
      refreshToken: refresh_token,
      accountId: nonEmptyString(account_id) ? account_id : accountIdFromIdToken(id_token),
      lastRefresh: Number.isFinite(lastRefresh) ? Math.min(lastRefresh, now) : now
    }
  };
};

export const codexRefreshDue = (secret: CodexSecret, now: number): boolean => {
  const exp = jwtExpMs(secret.accessToken);
  if (exp !== undefined) return exp - now < REFRESH_BEFORE_EXPIRY_MS;
  return now - secret.lastRefresh > STALE_WITHOUT_EXP_MS;
};

/**
 * Classify the token endpoint's answer. Absent fields keep their old values
 * (the endpoint may omit an unrotated id/refresh token). A 2xx that carries no
 * token at all is a contract change, not a success: transient, so the chain we
 * hold is not overwritten with nothing.
 */
export const parseCodexRefresh = (
  status: number,
  body: Record<string, unknown> | undefined,
  previous: CodexSecret,
  now: number
): RefreshOutcome<CodexSecret> => {
  if (status >= 200 && status < 300) {
    if (!body) return { kind: "transient", reason: "unparseable refresh response" };
    const idToken = nonEmptyString(body.id_token) ? body.id_token : undefined;
    const accessToken = nonEmptyString(body.access_token) ? body.access_token : undefined;
    const refreshToken = nonEmptyString(body.refresh_token) ? body.refresh_token : undefined;
    if (!idToken && !accessToken && !refreshToken) {
      return { kind: "transient", reason: "refresh response carried no tokens" };
    }
    const nextIdToken = idToken ?? previous.idToken;
    return {
      kind: "ok",
      secret: {
        idToken: nextIdToken,
        accessToken: accessToken ?? previous.accessToken,
        refreshToken: refreshToken ?? previous.refreshToken,
        accountId: (idToken && accountIdFromIdToken(idToken)) || previous.accountId,
        lastRefresh: now
      }
    };
  }
  // `{error: "invalid_grant"}` (OAuth) or `{error: {code: "refresh_token_expired"}}` (OpenAI).
  const error = body?.error;
  const code = typeof error === "string" ? error : isObject(error) && typeof error.code === "string" ? error.code : "";
  if (status === 401 || PERMANENT_CODES.has(code)) {
    return { kind: "reconnect", reason: code || `HTTP ${status}` };
  }
  return { kind: "transient", reason: `HTTP ${status}${code ? ` ${code}` : ""}` };
};

export const refreshCodex = async (secret: CodexSecret, context: RefreshContext): Promise<RefreshOutcome<CodexSecret>> => {
  const response = await upstream(context.fetch, CODEX_TOKEN_URL, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json" },
    body: JSON.stringify({
      client_id: CODEX_CLIENT_ID,
      grant_type: "refresh_token",
      refresh_token: secret.refreshToken,
      scope: "openid profile email"
    })
  });
  if (!response) return { kind: "transient", reason: "timeout or network error" };
  return parseCodexRefresh(response.status, await readJsonObject(response), secret, context.now);
};

export const codexAdapter: ProviderAdapter<CodexSecret> = {
  id: "codex",
  parseUpload: parseCodexUpload,
  account: (secret) => codexEmail(secret.idToken),
  refreshable: (secret) => nonEmptyString(secret.refreshToken),
  refreshDue: codexRefreshDue,
  refresh: refreshCodex,
  grant: (secret, now) => ({
    accessToken: secret.accessToken,
    idToken: secret.idToken,
    accountId: secret.accountId,
    expiresAt: jwtExpMs(secret.accessToken) ?? now + HOUR_MS,
    account: codexEmail(secret.idToken)
  })
};
