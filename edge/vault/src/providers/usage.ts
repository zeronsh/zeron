/**
 * Plan usage of a stored subscription login, read by the vault itself (the
 * token never leaves it): the same endpoints the engine probes for a device's
 * own logins, so the engine parses the body with the code it already has.
 *
 * - Claude: `GET https://api.anthropic.com/api/oauth/usage` (5-hour session
 *   and weekly buckets).
 * - Codex: `GET https://chatgpt.com/backend-api/wham/usage` (primary and
 *   secondary windows, plus the live plan).
 *
 * A rejection here never marks the credential: a usage endpoint answering
 * 401/403 (a setup token without the profile scope, a plan without a usage
 * view) says nothing about whether grants still work.
 */
import type { VaultProviderId } from "../api";
import { isObject, readJsonObject, upstream, type FetchFn, type GrantMaterial } from "./types";

export const CLAUDE_USAGE_URL = "https://api.anthropic.com/api/oauth/usage";
export const CODEX_USAGE_URL = "https://chatgpt.com/backend-api/wham/usage";

export type UsageOutcome =
  | { readonly kind: "ok"; readonly body: Record<string, unknown> }
  | { readonly kind: "unsupported" }
  | { readonly kind: "failed"; readonly reason: string; readonly retryAfterMs?: number };

/** Whether `provider` has a usage view at all (API keys and GitHub don't). */
export const hasUsage = (provider: VaultProviderId): boolean => provider === "claude" || provider === "codex";

export const fetchUsage = async (
  provider: VaultProviderId,
  material: GrantMaterial,
  fetchFn: FetchFn
): Promise<UsageOutcome> => {
  let response: Response | undefined;
  if (provider === "claude") {
    response = await upstream(fetchFn, CLAUDE_USAGE_URL, {
      headers: {
        authorization: `Bearer ${material.accessToken}`,
        "anthropic-beta": "oauth-2025-04-20",
        "content-type": "application/json"
      }
    });
  } else if (provider === "codex") {
    response = await upstream(fetchFn, CODEX_USAGE_URL, {
      headers: {
        authorization: `Bearer ${material.accessToken}`,
        "chatgpt-account-id": material.accountId ?? ""
      }
    });
  } else {
    return { kind: "unsupported" };
  }
  if (!response) return { kind: "failed", reason: "usage request timed out" };
  if (response.status === 429) {
    // Seconds form only; an HTTP date leaves the caller's own backoff.
    const seconds = Number(response.headers.get("retry-after") ?? NaN);
    return {
      kind: "failed",
      reason: "Rate limited — try again in a minute",
      ...(seconds >= 0 ? { retryAfterMs: seconds * 1000 } : {})
    };
  }
  if (response.status === 401 || response.status === 403) {
    return { kind: "failed", reason: `usage isn't available for this login (HTTP ${response.status})` };
  }
  if (response.status < 200 || response.status >= 300) {
    return { kind: "failed", reason: `usage request failed (HTTP ${response.status})` };
  }
  const body = await readJsonObject(response);
  return body ? { kind: "ok", body } : { kind: "failed", reason: "usage reply was not JSON" };
};

/** A usage reply's fullest window, in percent, and the first reset among
 * its windows (Unix ms). */
export interface PlanUsed {
  readonly percent: number;
  readonly resetsAt?: number;
}

/** The windows the app's meters show, read as the app reads them
 * (crates/proto/src/plan_usage.rs): Claude's `five_hour` and `seven_day`,
 * Codex's primary and secondary. Undefined when the reply carries none. */
export const planUsed = (body: Record<string, unknown>): PlanUsed | undefined => {
  const limits = isObject(body.rate_limit) ? body.rate_limit : {};
  let percent: number | undefined;
  let resetsAt: number | undefined;
  for (const [bucket, usedKey, resetKey] of [
    [body.five_hour, "utilization", "resets_at"],
    [body.seven_day, "utilization", "resets_at"],
    [limits.primary_window, "used_percent", "reset_at"],
    [limits.secondary_window, "used_percent", "reset_at"]
  ] as const) {
    if (!isObject(bucket)) continue;
    const used = bucket[usedKey];
    if (typeof used !== "number") continue;
    percent = Math.max(percent ?? 0, used);
    const reset = resetMs(bucket[resetKey]);
    if (reset !== undefined) resetsAt = Math.min(resetsAt ?? reset, reset);
  }
  if (percent === undefined) return undefined;
  return resetsAt === undefined ? { percent } : { percent, resetsAt };
};

/** A reset as Unix seconds or RFC 3339, in ms. */
const resetMs = (value: unknown): number | undefined => {
  if (typeof value === "number") return value * 1000;
  const ms = typeof value === "string" ? Date.parse(value) : NaN;
  return Number.isNaN(ms) ? undefined : ms;
};
