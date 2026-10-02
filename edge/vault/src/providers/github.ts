/**
 * GitHub — GitHub App user-to-server tokens obtained by the OAuth device flow.
 * The vault itself polls GitHub, so the token never transits a device: the
 * laptop only relays the user code. Material is therefore never uploadable.
 *
 * Expiring tokens (the GitHub App default: 8 h access, 6 month refresh) are
 * refreshed with the App's client secret when under 10 minutes remain. A
 * token issued without `expires_in` (expiration opted out) never refreshes;
 * its grants still expire hourly so revocation reaches consumers.
 *
 * GitHub's OAuth endpoints answer most errors with HTTP 200 and an `error`
 * field, so classification looks at the body first.
 */
import {
  HOUR_MS,
  nonEmptyString,
  readJsonObject,
  upstream,
  type FetchFn,
  type ProviderAdapter,
  type RefreshContext,
  type RefreshOutcome
} from "./types";

export const GITHUB_DEVICE_CODE_URL = "https://github.com/login/device/code";
export const GITHUB_TOKEN_URL = "https://github.com/login/oauth/access_token";
export const GITHUB_USER_URL = "https://api.github.com/user";
export const DEVICE_GRANT_TYPE = "urn:ietf:params:oauth:grant-type:device_code";
const REFRESH_BEFORE_EXPIRY_MS = 10 * 60_000;
/** GitHub's documented default when a device-code response omits it. */
const DEFAULT_INTERVAL_SECS = 5;
/** `slow_down` adds this to the interval (RFC 8628 §3.5). */
const SLOW_DOWN_STEP_SECS = 5;

export interface GithubSecret {
  readonly accessToken: string;
  /** Unix ms; absent = non-expiring token. */
  readonly expiresAt?: number;
  readonly refreshToken?: string;
  readonly refreshTokenExpiresAt?: number;
  readonly login?: string;
  readonly lastRefresh: number;
}

/** Errors that mean the refresh token (or the user's authorization) is gone. */
const PERMANENT_ERRORS = new Set(["bad_refresh_token", "invalid_grant"]);

const form = (fields: Record<string, string>): string => new URLSearchParams(fields).toString();
const FORM_HEADERS = { accept: "application/json", "content-type": "application/x-www-form-urlencoded" };

const seconds = (value: unknown): number | undefined =>
  typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;

/** A successful token response → the secret to store (login carried over). */
export const parseGithubToken = (
  body: Record<string, unknown>,
  now: number,
  login: string | undefined
): GithubSecret | undefined => {
  if (!nonEmptyString(body.access_token)) return undefined;
  const expiresIn = seconds(body.expires_in);
  const refreshExpiresIn = seconds(body.refresh_token_expires_in);
  return {
    accessToken: body.access_token,
    expiresAt: expiresIn === undefined ? undefined : now + expiresIn * 1000,
    refreshToken: nonEmptyString(body.refresh_token) ? body.refresh_token : undefined,
    refreshTokenExpiresAt: refreshExpiresIn === undefined ? undefined : now + refreshExpiresIn * 1000,
    login,
    lastRefresh: now
  };
};

export const parseGithubRefresh = (
  status: number,
  body: Record<string, unknown> | undefined,
  previous: GithubSecret,
  now: number
): RefreshOutcome<GithubSecret> => {
  const error = typeof body?.error === "string" ? body.error : "";
  if (error) {
    if (PERMANENT_ERRORS.has(error)) return { kind: "reconnect", reason: error };
    // incorrect_client_credentials & co. are OUR misconfiguration: never
    // punish the user's credential for them.
    return { kind: "transient", reason: error };
  }
  if (status === 401) return { kind: "reconnect", reason: "HTTP 401" };
  if (status < 200 || status >= 300 || !body) return { kind: "transient", reason: `HTTP ${status}` };
  const secret = parseGithubToken(body, now, previous.login);
  if (!secret) return { kind: "transient", reason: "refresh response carried no access_token" };
  // A refresh that omits a new refresh token keeps the old one (and its expiry).
  return {
    kind: "ok",
    secret: secret.refreshToken
      ? secret
      : { ...secret, refreshToken: previous.refreshToken, refreshTokenExpiresAt: previous.refreshTokenExpiresAt }
  };
};

export const githubRefreshDue = (secret: GithubSecret, now: number): boolean =>
  secret.expiresAt !== undefined && !!secret.refreshToken && secret.expiresAt - now < REFRESH_BEFORE_EXPIRY_MS;

export const refreshGithub = async (
  secret: GithubSecret,
  context: RefreshContext
): Promise<RefreshOutcome<GithubSecret>> => {
  if (!secret.refreshToken) return { kind: "reconnect", reason: "no refresh token" };
  if (!context.githubClientId || !context.githubClientSecret) {
    return { kind: "transient", reason: "GitHub App secrets are not configured" };
  }
  const response = await upstream(context.fetch, GITHUB_TOKEN_URL, {
    method: "POST",
    headers: FORM_HEADERS,
    body: form({
      client_id: context.githubClientId,
      client_secret: context.githubClientSecret,
      grant_type: "refresh_token",
      refresh_token: secret.refreshToken
    })
  });
  if (!response) return { kind: "transient", reason: "timeout or network error" };
  return parseGithubRefresh(response.status, await readJsonObject(response), secret, context.now);
};

export const githubAdapter: ProviderAdapter<GithubSecret> = {
  id: "github",
  account: (secret) => secret.login,
  refreshable: (secret) => !!secret.refreshToken && secret.expiresAt !== undefined,
  refreshDue: githubRefreshDue,
  refresh: refreshGithub,
  hardExpiresAt: (secret) => (secret.refreshToken ? undefined : secret.expiresAt),
  grant: (secret, now) => ({
    accessToken: secret.accessToken,
    expiresAt: secret.expiresAt ?? now + HOUR_MS,
    account: secret.login
  })
};

// ── Device flow ────────────────────────────────────────────────────────────

export interface DeviceCodeStart {
  readonly deviceCode: string;
  readonly userCode: string;
  readonly verificationUri: string;
  readonly expiresInSecs: number;
  readonly intervalSecs: number;
}

export type DeviceCodeResult =
  | { readonly ok: true; readonly start: DeviceCodeStart }
  | { readonly ok: false; readonly reason: string };

export const parseDeviceCode = (status: number, body: Record<string, unknown> | undefined): DeviceCodeResult => {
  if (!body) return { ok: false, reason: `HTTP ${status}` };
  if (typeof body.error === "string") return { ok: false, reason: body.error };
  const { device_code, user_code, verification_uri } = body;
  if (status < 200 || status >= 300 || !nonEmptyString(device_code) || !nonEmptyString(user_code) || !nonEmptyString(verification_uri)) {
    return { ok: false, reason: `unexpected device code response (HTTP ${status})` };
  }
  return {
    ok: true,
    start: {
      deviceCode: device_code,
      userCode: user_code,
      verificationUri: verification_uri,
      expiresInSecs: seconds(body.expires_in) ?? 900,
      intervalSecs: seconds(body.interval) ?? DEFAULT_INTERVAL_SECS
    }
  };
};

export const startDeviceFlow = async (clientId: string, fetchFn: FetchFn): Promise<DeviceCodeResult> => {
  const response = await upstream(fetchFn, GITHUB_DEVICE_CODE_URL, {
    method: "POST",
    headers: FORM_HEADERS,
    body: form({ client_id: clientId })
  });
  if (!response) return { ok: false, reason: "timeout or network error" };
  return parseDeviceCode(response.status, await readJsonObject(response));
};

export type DevicePollResult =
  | { readonly kind: "pending" }
  | { readonly kind: "slow_down"; readonly intervalSecs: number }
  | { readonly kind: "expired" }
  | { readonly kind: "denied" }
  | { readonly kind: "failed"; readonly error: string }
  | { readonly kind: "transient"; readonly reason: string }
  | { readonly kind: "token"; readonly body: Record<string, unknown> };

export const parseDevicePoll = (
  status: number,
  body: Record<string, unknown> | undefined,
  currentIntervalSecs: number
): DevicePollResult => {
  const error = typeof body?.error === "string" ? body.error : undefined;
  switch (error) {
    case undefined:
      break;
    case "authorization_pending":
      return { kind: "pending" };
    case "slow_down":
      return {
        kind: "slow_down",
        intervalSecs: seconds(body?.interval) ?? currentIntervalSecs + SLOW_DOWN_STEP_SECS
      };
    case "expired_token":
      return { kind: "expired" };
    case "access_denied":
      return { kind: "denied" };
    default:
      return { kind: "failed", error };
  }
  if (status >= 500 || status === 429 || !body) return { kind: "transient", reason: `HTTP ${status}` };
  if (status < 200 || status >= 300 || !nonEmptyString(body.access_token)) {
    return { kind: "failed", error: `unexpected token response (HTTP ${status})` };
  }
  return { kind: "token", body };
};

export const pollDeviceFlow = async (
  clientId: string,
  deviceCode: string,
  currentIntervalSecs: number,
  fetchFn: FetchFn
): Promise<DevicePollResult> => {
  const response = await upstream(fetchFn, GITHUB_TOKEN_URL, {
    method: "POST",
    headers: FORM_HEADERS,
    body: form({ client_id: clientId, device_code: deviceCode, grant_type: DEVICE_GRANT_TYPE })
  });
  if (!response) return { kind: "transient", reason: "timeout or network error" };
  return parseDevicePoll(response.status, await readJsonObject(response), currentIntervalSecs);
};

/** The account label. Best effort: the token is already ours (the device code
 * is single-use), so a failed lookup stores the token without a login rather
 * than throwing it away. */
export const fetchGithubLogin = async (accessToken: string, fetchFn: FetchFn): Promise<string | undefined> => {
  const response = await upstream(fetchFn, GITHUB_USER_URL, {
    headers: {
      accept: "application/vnd.github+json",
      authorization: `Bearer ${accessToken}`,
      "user-agent": "zeron-vault",
      "x-github-api-version": "2022-11-28"
    }
  });
  if (!response?.ok) return undefined;
  const body = await readJsonObject(response);
  return nonEmptyString(body?.login) ? body.login : undefined;
};
