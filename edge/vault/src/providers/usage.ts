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
import { readJsonObject, upstream, type FetchFn, type GrantMaterial } from "./types";

export const CLAUDE_USAGE_URL = "https://api.anthropic.com/api/oauth/usage";
export const CODEX_USAGE_URL = "https://chatgpt.com/backend-api/wham/usage";

export type UsageOutcome =
  | { readonly kind: "ok"; readonly body: Record<string, unknown> }
  | { readonly kind: "unsupported" }
  | { readonly kind: "failed"; readonly reason: string };

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
  if (response.status === 429) return { kind: "failed", reason: "Rate limited — try again in a minute" };
  if (response.status === 401 || response.status === 403) {
    return { kind: "failed", reason: `usage isn't available for this login (HTTP ${response.status})` };
  }
  if (response.status < 200 || response.status >= 300) {
    return { kind: "failed", reason: `usage request failed (HTTP ${response.status})` };
  }
  const body = await readJsonObject(response);
  return body ? { kind: "ok", body } : { kind: "failed", reason: "usage reply was not JSON" };
};
