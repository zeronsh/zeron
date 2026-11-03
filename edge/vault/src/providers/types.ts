/**
 * Provider adapter contract. An adapter is pure policy over one provider's
 * secret shape: how an upload parses, when a refresh is due, how the refresh
 * response classifies, and what a grant exposes. It never touches storage —
 * VaultAccount owns persistence, generations and the one-refresh-in-flight
 * gate — so every adapter is unit-testable in Node with an injected `fetch`.
 */
import type { VaultMaterial, VaultProviderId } from "../api";

export type FetchFn = (input: string, init?: RequestInit) => Promise<Response>;

/** Upstream calls never hang a Durable Object: past this they count as an
 * ambiguous (transient) failure and the current generation is kept. */
export const UPSTREAM_TIMEOUT_MS = 10_000;

export const HOUR_MS = 60 * 60_000;
export const DAY_MS = 24 * HOUR_MS;

export interface RefreshContext {
  readonly fetch: FetchFn;
  readonly now: number;
  readonly githubClientId?: string;
  readonly githubClientSecret?: string;
}

/**
 * - `ok`: persist `secret` as the next generation BEFORE answering anyone.
 * - `reconnect`: the provider says the refresh token is dead (`invalid_grant`,
 *   401, …) — mark `needs_reconnect` and stop issuing grants.
 * - `transient`: timeout / 5xx / malformed — keep the current generation and
 *   retry on a later grant.
 */
export type RefreshOutcome<S> =
  | { readonly kind: "ok"; readonly secret: S }
  | { readonly kind: "reconnect"; readonly reason: string }
  | { readonly kind: "transient"; readonly reason: string };

export type ParseResult<S> = { readonly ok: true; readonly secret: S } | { readonly ok: false; readonly message: string };

/** What a grant exposes. Never includes a refresh token. */
export interface GrantMaterial {
  readonly accessToken: string;
  readonly expiresAt: number;
  readonly accountId?: string;
  readonly idToken?: string;
  readonly account?: string;
  readonly scopes?: readonly string[];
  readonly subscriptionType?: string;
}

export interface ProviderAdapter<S> {
  readonly id: VaultProviderId;
  /** Absent = not uploadable (GitHub: device flow only). */
  parseUpload?(material: VaultMaterial, now: number): ParseResult<S>;
  /** Display label: email / login / `…abcd`. Never secret material. */
  account(secret: S): string | undefined;
  /** Whether a refresh token exists at all (canary: "not_refreshable"). */
  refreshable(secret: S): boolean;
  refreshDue(secret: S, now: number): boolean;
  refresh?(secret: S, context: RefreshContext): Promise<RefreshOutcome<S>>;
  /** When a credential that cannot be refreshed dies for good (Unix ms), so
   * the account flips to `needs_reconnect` — visible in `status` without a
   * decrypt. Undefined = refreshable, or never expires. */
  hardExpiresAt?(secret: S): number | undefined;
  grant(secret: S, now: number): GrantMaterial;
}

export const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

export const nonEmptyString = (value: unknown): value is string => typeof value === "string" && value.length > 0;

/** Body as a JSON object, or undefined (HTML error pages, empty bodies, …). */
export const readJsonObject = async (response: Response): Promise<Record<string, unknown> | undefined> => {
  try {
    const value: unknown = JSON.parse(await response.text());
    return isObject(value) ? value : undefined;
  } catch {
    return undefined;
  }
};

/** Run an upstream call with the shared timeout; network errors and timeouts
 * become `undefined` (= transient) instead of throwing. */
export const upstream = async (fetchFn: FetchFn, url: string, init: RequestInit): Promise<Response | undefined> => {
  try {
    return await fetchFn(url, { ...init, signal: AbortSignal.timeout(UPSTREAM_TIMEOUT_MS) });
  } catch {
    return undefined;
  }
};
