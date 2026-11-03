import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it, vi } from "vitest";
import { MAX_INVITES_PER_REQUEST, handleAuthRoute } from "../../src/auth-routes";
import type { Env } from "../../src/env";
import { INVITES_PER_HOUR, type UserDirectory } from "../../src/user-directory";

// WorkOS is the fake in workos-fake.ts (outbound host api.workos.com), shared
// by every test here: each test uses its own user and org.

const edgeEnv = { ...(env as unknown as Env), WORKOS_API_KEY: "sk_test" };

const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));

const newId = (prefix: string) => `${prefix}_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;

const invite = async (bearer: string, orgId: string, body: unknown) => {
  const url = new URL(`https://edge.test/auth/orgs/${orgId}/invitations`);
  const res = (await handleAuthRoute(
    new Request(url, {
      method: "POST",
      headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json" },
      body: JSON.stringify(body)
    }),
    edgeEnv,
    url
  ))!;
  return { status: res.status, body: (await res.json()) as Record<string, unknown> };
};

const workosControl = (path: "__fail" | "__memberships", body: unknown) =>
  fetch(`https://api.workos.com/${path}`, { method: "POST", body: JSON.stringify(body) });

interface Invitation {
  email: string;
  organization_id: string;
  inviter_user_id: string | null;
  role_slug: string | null;
}

const invitations = async (orgId: string): Promise<Invitation[]> =>
  ((await (await fetch("https://api.workos.com/__state")).json()) as { invitations: Invitation[] }).invitations.filter(
    (i) => i.organization_id === orgId
  );

/** A user who administers a fresh org. */
const admin = async () => {
  const userId = newId("user");
  const orgId = newId("org");
  await workosControl("__memberships", { user_id: userId, organization_id: orgId, role: "admin", email: `${userId}@example.net` });
  return { userId, orgId };
};

/** `n` distinct addresses. */
const addresses = (n: number) => Array.from({ length: n }, () => `${newId("invitee")}@example.net`);

describe("POST /auth/orgs/{id}/invitations", () => {
  it("invites each address once as a member, and says how each one went", async () => {
    const { userId, orgId } = await admin();
    const member = `${newId("member")}@example.net`;
    await workosControl("__memberships", { user_id: newId("user"), organization_id: orgId, role: "member", email: member });
    const res = await invite(userId, orgId, {
      emails: ["  Ada@Example.net ", "ada@example.net", "grace@example.net", "not-an-email", "x@y..net", member.toUpperCase()]
    });
    expect(res).toEqual({
      status: 200,
      body: {
        sent: ["Ada@Example.net", "grace@example.net"],
        failed: [
          { email: "not-an-email", error: "invalid" },
          { email: "x@y..net", error: "invalid" },
          { email: member.toUpperCase(), error: "exists" }
        ]
      }
    });
    expect(await invitations(orgId)).toMatchObject([
      { id: expect.any(String), email: "Ada@Example.net", organization_id: orgId, inviter_user_id: userId, role_slug: "member" },
      { id: expect.any(String), email: "grace@example.net", organization_id: orgId, inviter_user_id: userId, role_slug: "member" }
    ]);

    // Already invited.
    expect((await invite(userId, orgId, { emails: ["GRACE@example.net"] })).body).toEqual({
      sent: [],
      failed: [{ email: "GRACE@example.net", error: "exists" }]
    });
  });

  it("answers failed for an address WorkOS fails on, and sends the rest", async () => {
    const { userId, orgId } = await admin();
    await workosControl("__fail", { path: "/user_management/invitations", times: 1 });
    const [a, b] = addresses(2);
    const res = await invite(userId, orgId, { emails: [a, b] });
    expect(res.status).toBe(200);
    expect((res.body.sent as string[]).length).toBe(1);
    expect(res.body.failed).toEqual([{ email: expect.any(String), error: "failed" }]);
  });

  it("is for the org's admins only", async () => {
    const { orgId } = await admin();
    const member = newId("user");
    await workosControl("__memberships", { user_id: member, organization_id: orgId, role: "member" });
    expect(await invite(member, orgId, { emails: addresses(1) })).toEqual({ status: 403, body: { error: "forbidden" } });
    expect(await invite(newId("user"), orgId, { emails: addresses(1) })).toEqual({ status: 403, body: { error: "forbidden" } });
    const other = await admin();
    expect(await invite(other.userId, orgId, { emails: addresses(1) })).toEqual({ status: 403, body: { error: "forbidden" } });
    expect(await invitations(orgId)).toEqual([]);
  });

  it("forbids a runner token", async () => {
    const { userId, orgId } = await admin();
    expect(await invite(`runner:${userId}@${orgId}:cloud-1`, orgId, { emails: addresses(1) })).toEqual({
      status: 403,
      body: { error: "forbidden" }
    });
    expect(await invitations(orgId)).toEqual([]);
  });

  it("answers 400 invalid for a bad body", async () => {
    const { userId, orgId } = await admin();
    for (const body of [
      {},
      { emails: "a@example.net" },
      { emails: [] },
      { emails: ["  ", ""] },
      { emails: ["a@example.net", 3] },
      { emails: addresses(MAX_INVITES_PER_REQUEST + 1) }
    ]) {
      expect(await invite(userId, orgId, body)).toEqual({ status: 400, body: { error: "invalid" } });
    }
    // One too many that dedupe to the most are fine.
    const most = addresses(MAX_INVITES_PER_REQUEST);
    expect((await invite(userId, orgId, { emails: [...most, most[0]!.toUpperCase()] })).status).toBe(200);
  });

  it(`limits a user to ${INVITES_PER_HOUR} addresses an hour, across their orgs`, async () => {
    const { userId, orgId } = await admin();
    const second = newId("org");
    await workosControl("__memberships", { user_id: userId, organization_id: second, role: "admin" });
    // 20 = 10 + 5 + 5.
    expect((await invite(userId, orgId, { emails: addresses(10) })).status).toBe(200);
    expect((await invite(userId, second, { emails: addresses(5) })).status).toBe(200);
    expect((await invite(userId, orgId, { emails: addresses(6) })).status).toBe(429);
    expect((await invite(userId, orgId, { emails: addresses(5) })).status).toBe(200);
    expect(await invite(userId, orgId, { emails: addresses(1) })).toEqual({ status: 429, body: { error: "rate_limited" } });
    expect((await invitations(orgId)).length + (await invitations(second)).length).toBe(INVITES_PER_HOUR);

    // An hour on, it starts over.
    await runInDurableObject(directory(), (_instance: UserDirectory, state) => {
      state.storage.sql.exec("UPDATE invite_quota SET window_start = window_start - 3600000 WHERE user_id = ?", userId);
    });
    expect((await invite(userId, orgId, { emails: addresses(1) })).status).toBe(200);
  });

  it("never logs the addresses", async () => {
    const { userId, orgId } = await admin();
    const logged: unknown[] = [];
    const spies = (["log", "info", "warn", "error", "debug"] as const).map((level) =>
      vi.spyOn(console, level).mockImplementation((...args: unknown[]) => void logged.push(...args))
    );
    try {
      await workosControl("__fail", { path: "/user_management/invitations", times: 1 });
      const emails = [...addresses(2), "secret-invalid", "x@secret..net"];
      const res = await invite(userId, orgId, { emails });
      expect((res.body.failed as unknown[]).length).toBe(3);
      const text = logged.map((l) => (typeof l === "string" ? l : JSON.stringify(l) ?? String(l))).join("\n");
      for (const email of emails) expect(text).not.toContain(email);
    } finally {
      for (const spy of spies) spy.mockRestore();
    }
  });
});
