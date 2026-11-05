import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { MAX_ONBOARDING_PAGES } from "../../src/user-directory";

const newUserId = () => `user_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;

type Done = { done: Record<string, string> };

const onboarding = (bearer: string | undefined, body?: unknown) =>
  SELF.fetch("https://edge.test/onboarding", {
    method: body === undefined ? "GET" : "POST",
    headers: {
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  });

const done = async (bearer: string, body?: unknown): Promise<Done> => {
  const res = await onboarding(bearer, body);
  expect(res.status).toBe(200);
  return res.json();
};

const pages = (n: number, prefix: string) => Array.from({ length: n }, (_, i) => `${prefix}-${i}`);

describe("/onboarding", () => {
  it("answers {} until pages are recorded, then each page with when it was first done", async () => {
    const userId = newUserId();
    expect(await done(userId)).toEqual({ done: {} });

    const first = await done(userId, { done: ["username", "org"] });
    expect(Object.keys(first.done).sort()).toEqual(["org", "username"]);
    for (const at of Object.values(first.done)) expect(new Date(at).toISOString()).toBe(at);
    expect(await done(userId)).toEqual(first);

    // Another user's pages are their own.
    expect(await done(newUserId())).toEqual({ done: {} });
  });

  it("is idempotent and keeps the first time", async () => {
    const userId = newUserId();
    const first = await done(userId, { done: ["welcome"] });
    await new Promise((r) => setTimeout(r, 5));
    const again = await done(userId, { done: ["welcome", "welcome", "whats-new-0-3"] });
    expect(again.done.welcome).toBe(first.done.welcome);
    expect(Object.keys(again.done).sort()).toEqual(["welcome", "whats-new-0-3"]);
    expect(await done(userId, { done: ["welcome"] })).toEqual(again);
    // Nothing to record still answers the shape.
    expect(await done(userId, { done: [] })).toEqual(again);
  });

  it("answers 400 invalid for bad ids, bodies and limits, recording nothing", async () => {
    const userId = newUserId();
    for (const body of [
      {},
      { done: "welcome" },
      { done: [3] },
      { done: ["Welcome"] },
      { done: ["-welcome"] },
      { done: ["wel come"] },
      { done: ["a".repeat(65)] },
      { done: ["ok", "bad_id"] },
      { done: pages(51, "p") }
    ]) {
      const res = await onboarding(userId, body);
      expect(res.status).toBe(400);
      expect(await res.json()).toEqual({ error: "invalid" });
    }
    const bad = await SELF.fetch("https://edge.test/onboarding", {
      method: "POST",
      headers: { authorization: `Bearer ${userId}`, "content-type": "application/json" },
      body: "{not json"
    });
    expect(bad.status).toBe(400);
    expect(await done(userId)).toEqual({ done: {} });
    // The longest id and the most a request may carry are fine.
    expect(Object.keys((await done(userId, { done: ["a".repeat(64), ...pages(49, "q")] })).done)).toHaveLength(50);
  });

  it(`stores at most ${MAX_ONBOARDING_PAGES} pages a user`, async () => {
    const userId = newUserId();
    for (let i = 0; i < MAX_ONBOARDING_PAGES / 50; i++) await done(userId, { done: pages(50, `batch${i}`) });
    expect(Object.keys((await done(userId)).done)).toHaveLength(MAX_ONBOARDING_PAGES);
    // Pages already done still go through; one more doesn't, and nothing in its batch lands.
    expect(Object.keys((await done(userId, { done: ["batch0-0"] })).done)).toHaveLength(MAX_ONBOARDING_PAGES);
    const res = await onboarding(userId, { done: ["batch0-1", "one-more"] });
    expect(res.status).toBe(400);
    expect(await res.json()).toEqual({ error: "invalid" });
    expect((await done(userId)).done["one-more"]).toBeUndefined();
  });

  it("forbids runner tokens and needs a bearer", async () => {
    const userId = newUserId();
    expect((await onboarding(`runner:${userId}@org1:cloud-1`)).status).toBe(403);
    expect((await onboarding(`runner:${userId}@org1:cloud-1`, { done: ["welcome"] })).status).toBe(403);
    expect((await onboarding(undefined)).status).toBe(401);
    expect((await onboarding(undefined, { done: ["welcome"] })).status).toBe(401);
    expect(await done(userId)).toEqual({ done: {} });
  });

  it("shows each user's finished pages in the admin list", async () => {
    const userId = newUserId();
    const stamp = "2026-10-31T10:00:00.000Z";
    const payload = { id: userId, email: `${userId}@example.net`, first_name: "Ada", last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp };
    await env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1")).upsert(payload);
    await done(userId, { done: ["username"] });
    await new Promise((r) => setTimeout(r, 5));
    await done(userId, { done: ["org"] });
    const admin = (path: string) => SELF.fetch(`https://edge.test${path}`, { headers: { authorization: "Bearer test-admin-token" } });
    const listed = (await (await admin("/admin/users")).json()) as { users: { userId: string; onboardingDone: string[] }[] };
    expect(listed.users.find((u) => u.userId === userId)?.onboardingDone).toEqual(["username", "org"]);
    const csv = await (await admin("/admin/users?format=csv")).text();
    expect(csv.split("\n")[0]!.endsWith(",created_org_id,onboarding_done")).toBe(true);
    expect(csv).toMatch(new RegExp(`\\n\\d+,${userId},.*,username;org\\n`));
  });
});
