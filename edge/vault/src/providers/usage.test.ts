import { describe, expect, it } from "vitest";
import { CLAUDE_USAGE_URL, fetchUsage, planUsed } from "./usage";

const material = { accessToken: "sk-ant-oat01-test", expiresAt: Date.now() + 60 * 60_000 };

describe("planUsed", () => {
  it("reads Claude's fullest bucket and its first reset", () => {
    // The reply as the engine's tests hold it (crates/engine/src/agent_accounts.rs).
    expect(
      planUsed({
        five_hour: { utilization: 6.0, resets_at: "2026-04-08T18:59:59Z" },
        seven_day: { utilization: 35.0, resets_at: "2026-04-14T16:59:59Z" }
      })
    ).toEqual({ percent: 35, resetsAt: Date.parse("2026-04-08T18:59:59Z") });
  });

  it("reads Codex's windows, their resets in Unix seconds", () => {
    expect(
      planUsed({
        plan_type: "plus",
        rate_limit: {
          primary_window: { used_percent: 92, limit_window_seconds: 18000, reset_at: 1_800_000_000 },
          secondary_window: { used_percent: 40, limit_window_seconds: 604800 }
        }
      })
    ).toEqual({ percent: 92, resetsAt: 1_800_000_000_000 });
  });

  it("skips what the app's meters skip: no number, no window", () => {
    expect(planUsed({})).toBeUndefined();
    expect(planUsed({ five_hour: {}, seven_day: null, rate_limit: [] })).toBeUndefined();
    expect(planUsed({ five_hour: { utilization: null }, seven_day: { utilization: 12, resets_at: "soon" } })).toEqual({
      percent: 12
    });
  });
});

describe("fetchUsage", () => {
  const answer429 = (headers: Record<string, string>) => () =>
    Promise.resolve(new Response("", { status: 429, headers }));

  it("hands a 429's Retry-After on, in ms", async () => {
    expect(await fetchUsage("claude", material, answer429({ "retry-after": "120" }))).toMatchObject({
      kind: "failed",
      retryAfterMs: 120_000
    });
  });

  it("leaves it out when absent or not in seconds", async () => {
    for (const headers of [{}, { "retry-after": "Wed, 21 Oct 2026 07:28:00 GMT" }] as Record<string, string>[]) {
      const outcome = await fetchUsage("claude", material, answer429(headers));
      expect(outcome.kind).toBe("failed");
      expect(outcome).not.toHaveProperty("retryAfterMs");
    }
  });

  it("asks Claude's endpoint with the beta header", async () => {
    let seen: { url: string; headers: Headers } | undefined;
    await fetchUsage("claude", material, (input, init) => {
      seen = { url: String(input), headers: new Headers(init?.headers) };
      return Promise.resolve(Response.json({ five_hour: { utilization: 1 } }));
    });
    expect(seen?.url).toBe(CLAUDE_USAGE_URL);
    expect(seen?.headers.get("anthropic-beta")).toBe("oauth-2025-04-20");
  });
});
