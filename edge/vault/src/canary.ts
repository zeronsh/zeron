/**
 * Daily canary (cron `0 6 * * *`): force one refresh of `CANARY_USER_ID`'s
 * credential for every refreshable provider (codex, claude, github), through
 * the same per-account gate real grants use. A provider that changes its
 * refresh contract (new client id, new error shape, endpoint move) fails here
 * a day before users' grants start failing. A non-refreshable credential with
 * an end date (Claude setup token) is instead checked for >30 days of life.
 * The GitHub App's own credentials are checked too (`GET /app` with an App
 * JWT): every Cloud session's GitHub access is minted with them.
 * Each result is recorded in the account DO's `canary` table and logged as one
 * `vault.canary` line — alert on `ok=false` in Workers observability.
 */
import type { CanaryResult } from "./account";
import type { Env } from "./env";
import { accountStub } from "./names";
import { CANARY_PROVIDERS } from "./providers";
import { checkApp } from "./providers/github-app";

export const runCanary = async (env: Env): Promise<CanaryResult[]> => {
  if (env.VAULT_DISABLED === "1") {
    console.log(JSON.stringify({ event: "vault.canary", skipped: "vault disabled" }));
    return [];
  }
  const results: CanaryResult[] = [];
  const userId = env.CANARY_USER_ID;
  if (userId) {
    for (const provider of CANARY_PROVIDERS) {
      let result: CanaryResult;
      try {
        result = await accountStub(env, userId, provider).canary(userId, provider);
      } catch {
        result = { provider, ok: false, status: "error", at: Date.now() };
      }
      console.log(JSON.stringify({ event: "vault.canary", provider, ok: result.ok, status: result.status }));
      results.push(result);
    }
  } else {
    console.log(JSON.stringify({ event: "vault.canary", skipped: "CANARY_USER_ID not set" }));
  }
  if (env.GITHUB_APP_CLIENT_ID && env.GITHUB_APP_PRIVATE_KEY) {
    const ok = await checkApp(env.GITHUB_APP_CLIENT_ID, env.GITHUB_APP_PRIVATE_KEY, (input, init) => fetch(input, init));
    const status = ok ? "app_valid" : "app_rejected";
    console.log(JSON.stringify({ event: "vault.canary", provider: "github", ok, status }));
    results.push({ provider: "github", ok, status, at: Date.now() });
  }
  return results;
};
