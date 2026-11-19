/**
 * Plan usage of a stored subscription login, read by the vault itself (the
 * token never leaves it): the same endpoints the engine probes for a device's
 * own logins, so the engine parses the body with the code it already has.
 *
 * - Claude: `GET https://api.anthropic.com/api/oauth/usage` (5-hour session
 *   and weekly buckets).
 * - Codex: `GET https://chatgpt.com/backend-api/wham/usage` (primary and
 *   secondary windows, plus the live plan).
 * - Antigravity: Google Code Assist's `loadCodeAssist` (the account's tiers
 *   and project) then `retrieveUserQuotaSummary` for that project (each group
 *   of models' limits), answered as one body: the summary's `groups` with
 *   the tiers beside them, as the engine's own probe builds it.
 *
 * A rejection here never marks the credential: a usage endpoint answering
 * 401/403 (a setup token without the profile scope, a plan without a usage
 * view) says nothing about whether grants still work.
 */
import type { VaultProviderId } from "../api";
import { readJsonObject, upstream, type FetchFn, type GrantMaterial } from "./types";

export const CLAUDE_USAGE_URL = "https://api.anthropic.com/api/oauth/usage";
export const CODEX_USAGE_URL = "https://chatgpt.com/backend-api/wham/usage";
export const ANTIGRAVITY_TIER_URL = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
export const ANTIGRAVITY_USAGE_URL = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";

export type UsageOutcome =
  | { readonly kind: "ok"; readonly body: Record<string, unknown> }
  | { readonly kind: "unsupported" }
  | { readonly kind: "failed"; readonly reason: string };

/** Whether `provider` has a usage view at all (API keys and GitHub don't). */
export const hasUsage = (provider: VaultProviderId): boolean =>
  provider === "claude" || provider === "codex" || provider === "antigravity";

/** Why an upstream usage reply isn't one; `undefined` when it is (2xx). */
const refusal = (response: Response | undefined): UsageOutcome | undefined => {
  if (!response) return { kind: "failed", reason: "usage request timed out" };
  if (response.status === 429) return { kind: "failed", reason: "Rate limited — try again in a minute" };
  if (response.status === 401 || response.status === 403) {
    return { kind: "failed", reason: `usage isn't available for this login (HTTP ${response.status})` };
  }
  if (response.status < 200 || response.status >= 300) {
    return { kind: "failed", reason: `usage request failed (HTTP ${response.status})` };
  }
  return undefined;
};

const NOT_JSON: UsageOutcome = { kind: "failed", reason: "usage reply was not JSON" };

/** The Code Assist project `loadCodeAssist` names (a string, or `{id}`). */
const assistProject = (assist: Record<string, unknown>): string | undefined => {
  const project = assist.cloudaicompanionProject;
  if (typeof project === "string") return project || undefined;
  if (project && typeof project === "object" && "id" in project && typeof project.id === "string") return project.id || undefined;
  return undefined;
};

/** Google answers Antigravity's limits only to an Antigravity client ("You do
 * not have a valid license of this product" to any other): its server sends
 * `antigravity/acp/<version>`. */
export const ANTIGRAVITY_USER_AGENT = "antigravity/acp/zeron";

const antigravityUsage = async (material: GrantMaterial, fetchFn: FetchFn): Promise<UsageOutcome> => {
  const headers = {
    authorization: `Bearer ${material.accessToken}`,
    "content-type": "application/json",
    "user-agent": ANTIGRAVITY_USER_AGENT
  };
  const tierResponse = await upstream(fetchFn, ANTIGRAVITY_TIER_URL, {
    method: "POST",
    headers,
    body: JSON.stringify({ metadata: { ideType: "ANTIGRAVITY", pluginType: "GEMINI" } })
  });
  const tierRefused = refusal(tierResponse);
  if (tierRefused) return tierRefused;
  const assist = await readJsonObject(tierResponse!);
  if (!assist) return NOT_JSON;
  const project = material.settings?.project_id || assistProject(assist);
  if (!project) return { kind: "failed", reason: "Google named no project for this login" };
  const usageResponse = await upstream(fetchFn, ANTIGRAVITY_USAGE_URL, {
    method: "POST",
    headers,
    body: JSON.stringify({ project })
  });
  const usageRefused = refusal(usageResponse);
  if (usageRefused) return usageRefused;
  const quota = await readJsonObject(usageResponse!);
  if (!quota) return NOT_JSON;
  return {
    kind: "ok",
    body: {
      ...quota,
      ...(assist.paidTier !== undefined ? { paidTier: assist.paidTier } : {}),
      ...(assist.currentTier !== undefined ? { currentTier: assist.currentTier } : {})
    }
  };
};

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
  } else if (provider === "antigravity") {
    return antigravityUsage(material, fetchFn);
  } else {
    return { kind: "unsupported" };
  }
  const refused = refusal(response);
  if (refused) return refused;
  const body = await readJsonObject(response!);
  return body ? { kind: "ok", body } : NOT_JSON;
};
