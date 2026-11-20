/**
 * Pieces every vault-refreshed OAuth login shares: where its refresh token
 * may be sent (an `https` host of the provider's own domain, nothing else),
 * how a token endpoint's answer classifies, and when to refresh ahead of
 * expiry so a consumer that refreshes on its own schedule never needs to.
 */
import { jwtExpMs } from "../jwt";
import { nonEmptyString, readJsonObject, upstream, type FetchFn, type RefreshOutcome } from "./types";

/** `value` as an `https` url on `domains` (or a subdomain), with no
 * credentials, port, query or fragment; `undefined` otherwise. */
export const httpsUrlOn = (value: string, domains: readonly string[]): URL | undefined => {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  if (url.protocol !== "https:" || url.username || url.password || url.port || url.search || url.hash) return undefined;
  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  return domains.some((domain) => host === domain || host.endsWith(`.${domain}`)) ? url : undefined;
};

/** Unix ms from an ISO time, a JWT's `exp`, or `expires_in` seconds after
 * `now`, in that order; 0 when none (refreshed at the first grant). */
export const expiryOf = (now: number, iso: unknown, accessToken: string, expiresIn?: unknown): number => {
  const parsed = typeof iso === "string" ? Date.parse(iso) : Number.NaN;
  if (Number.isFinite(parsed) && parsed > 0) return parsed;
  const exp = jwtExpMs(accessToken);
  if (exp !== undefined) return exp;
  if (typeof expiresIn === "number" && Number.isFinite(expiresIn) && expiresIn > 0) return now + expiresIn * 1000;
  return 0;
};

/** Due once less than `cap` — or half the token's known life, if shorter —
 * remains. `issuedAt` 0 = life unknown: `cap` alone. */
export const refreshDueAhead = (expiresAt: number, issuedAt: number, now: number, cap: number): boolean => {
  const life = issuedAt > 0 ? expiresAt - issuedAt : 0;
  const ahead = life > 0 ? Math.min(cap, life / 2) : cap;
  return expiresAt - now < ahead;
};

export interface TokenReply {
  readonly accessToken: string;
  /** Absent: the old refresh token stays. */
  readonly refreshToken?: string;
  readonly expiresIn?: number;
  readonly body: Record<string, unknown>;
}

/** Classify a token endpoint's answer: a dead grant needs a reconnect, a
 * 5xx / timeout / junk is retried later with the current generation. */
export const classifyTokenReply = (
  status: number,
  body: Record<string, unknown> | undefined
): { readonly kind: "ok"; readonly reply: TokenReply } | Exclude<RefreshOutcome<never>, { kind: "ok" }> => {
  if (status >= 200 && status < 300) {
    if (!body || !nonEmptyString(body.access_token)) return { kind: "transient", reason: "refresh response carried no access_token" };
    const expiresIn =
      typeof body.expires_in === "number" && Number.isFinite(body.expires_in) && body.expires_in > 0 ? body.expires_in : undefined;
    return {
      kind: "ok",
      reply: {
        accessToken: body.access_token,
        ...(nonEmptyString(body.refresh_token) ? { refreshToken: body.refresh_token } : {}),
        ...(expiresIn !== undefined ? { expiresIn } : {}),
        body
      }
    };
  }
  const error = body?.error;
  const code = typeof error === "string" ? error : "";
  if (
    status === 401 ||
    ["invalid_grant", "invalid_client", "unauthorized_client", "refresh_token_reused", "invalid_token"].includes(code)
  ) {
    return { kind: "reconnect", reason: code || `HTTP ${status}` };
  }
  return { kind: "transient", reason: `HTTP ${status}${code ? ` ${code}` : ""}` };
};

/** POST a form to a token endpoint and classify the answer. */
export const postTokenForm = async (
  fetchFn: FetchFn,
  url: string,
  form: Record<string, string>,
  headers: Record<string, string> = {}
) => {
  const response = await upstream(fetchFn, url, {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded", accept: "application/json", ...headers },
    body: new URLSearchParams(form).toString()
  });
  if (!response) return { kind: "transient" as const, reason: "timeout or network error" };
  return classifyTokenReply(response.status, await readJsonObject(response));
};
