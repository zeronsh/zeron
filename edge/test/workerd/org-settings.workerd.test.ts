import { env } from "cloudflare:test";
import { describe, expect, it, vi } from "vitest";
import { handleAuthRoute } from "../../src/auth-routes";
import type { Env } from "../../src/env";

// The account and org settings routes (/auth/me, /auth/orgs/{id}/…). WorkOS
// is the fake in workos-fake.ts (outbound host api.workos.com), shared by
// every test here: each test uses its own users and orgs.

const edgeEnv = { ...(env as unknown as Env), WORKOS_API_KEY: "sk_test" };

const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));

const newId = (prefix: string) => `${prefix}_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;

const call = async (method: string, path: string, bearer?: string, body?: unknown) => {
  const url = new URL(`https://edge.test${path}`);
  const res = (await handleAuthRoute(
    new Request(url, {
      method,
      headers: {
        ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
        ...(body === undefined ? {} : { "content-type": "application/json" })
      },
      ...(body === undefined ? {} : { body: JSON.stringify(body) })
    }),
    edgeEnv,
    url
  ))!;
  return { status: res.status, body: (await res.json()) as Record<string, unknown> };
};

const workosControl = (path: "__fail" | "__memberships" | "__users" | "__invitations", body: unknown) =>
  fetch(`https://api.workos.com/${path}`, { method: "POST", body: JSON.stringify(body) }).then(
    (r) => r.json() as Promise<Record<string, string>>
  );

interface FakeState {
  users: { id: string; first_name: string | null; last_name: string | null; updated_at: string }[];
  memberships: { id: string; user_id: string; organization_id: string; role: string }[];
  invitations: { id: string; email: string; organization_id: string; state: string; sends: number }[];
  orgs: { id: string; name: string }[];
}

const workos = async (): Promise<FakeState> => (await fetch("https://api.workos.com/__state")).json();

/** A WorkOS user with a membership in `orgId`; answers the ids. */
const member = async (orgId: string, role: "admin" | "member", profile: Record<string, string> = {}) => {
  const userId = newId("user");
  const email = `${newId("person")}@example.net`;
  await workosControl("__users", { id: userId, email, ...profile });
  const membership = await workosControl("__memberships", { user_id: userId, organization_id: orgId, role, email });
  return { userId, email, membershipId: membership.id! };
};

/** An org with an admin and a member. */
const org = async () => {
  const orgId = newId("org");
  return { orgId, admin: await member(orgId, "admin"), member: await member(orgId, "member") };
};

const inAWeek = () => new Date(Date.now() + 7 * 86_400_000).toISOString();
const daysAgo = (days: number) => new Date(Date.now() - days * 86_400_000).toISOString();

describe("the settings routes' bearer", () => {
  it("answers 401 without one and 403 for a runner token", async () => {
    const { orgId, admin, member: m } = await org();
    const routes: [string, string, unknown?][] = [
      ["GET", "/auth/me"],
      ["PATCH", "/auth/me", { name: "Ada" }],
      ["PATCH", `/auth/orgs/${orgId}`, { name: "Acme" }],
      ["GET", `/auth/orgs/${orgId}/members`],
      ["PATCH", `/auth/orgs/${orgId}/members/${m.membershipId}`, { role: "admin" }],
      ["DELETE", `/auth/orgs/${orgId}/members/${m.membershipId}`],
      ["GET", `/auth/orgs/${orgId}/invitations`],
      ["DELETE", `/auth/orgs/${orgId}/invitations/invitation_x`],
      ["POST", `/auth/orgs/${orgId}/invitations/invitation_x/resend`]
    ];
    for (const [method, path, body] of routes) {
      expect(await call(method, path, undefined, body), `${method} ${path}`).toEqual({
        status: 401,
        body: { error: "invalid or missing bearer token" }
      });
      expect(await call(method, path, `runner:${admin.userId}@${orgId}:cloud-1`, body), `${method} ${path}`).toEqual({
        status: 403,
        body: { error: "forbidden" }
      });
    }
    expect((await workos()).memberships.filter((x) => x.organization_id === orgId).map((x) => x.role)).toEqual([
      "admin",
      "member"
    ]);
  });
});

describe("/auth/me", () => {
  it("answers the caller's name and email", async () => {
    const orgId = newId("org");
    const { userId, email } = await member(orgId, "member", { first_name: "Ada", last_name: "Lovelace" });
    expect(await call("GET", "/auth/me", userId)).toEqual({
      status: 200,
      body: { firstName: "Ada", lastName: "Lovelace", email, name: "Ada Lovelace", title: null, avatar: null }
    });
  });

  it("splits a new name at the first space, in WorkOS and the directory", async () => {
    const { userId, email } = await member(newId("org"), "member", { first_name: "Old" });
    // The directory knows the user (the webhook landed) and they hold a username.
    const seeded = (await workos()).users.find((u) => u.id === userId)!;
    await directory().upsert({ ...seeded, email, profile_picture_url: null, created_at: seeded.updated_at } as never);
    const username = `n${crypto.randomUUID().replaceAll("-", "").slice(0, 10)}`;
    expect((await directory().claimUsername(userId, username, 1)).error).toBeUndefined();

    expect(await call("PATCH", "/auth/me", userId, { name: "  Ada \t Lovelace  King  " })).toEqual({
      status: 200,
      body: { firstName: "Ada", lastName: "Lovelace  King", email, name: "Ada Lovelace  King", title: null, avatar: null }
    });
    const stored = (await workos()).users.find((u) => u.id === userId)!;
    expect([stored.first_name, stored.last_name]).toEqual(["Ada", "Lovelace  King"]);
    expect((await directory().profiles([userId]))[userId]).toMatchObject({ firstName: "Ada", lastName: "Lovelace  King" });
    expect((await directory().card(username))?.firstName).toBe("Ada");

    expect((await call("PATCH", "/auth/me", userId, { name: "Cher" })).body).toMatchObject({
      firstName: "Cher",
      lastName: "",
      name: "Cher"
    });
  });

  it("answers 400 invalid for a name that isn't 1-80 characters", async () => {
    const { userId } = await member(newId("org"), "member", { first_name: "Kept" });
    for (const body of [{}, { name: 3 }, { name: "" }, { name: "   " }, { name: "x".repeat(81) }]) {
      expect(await call("PATCH", "/auth/me", userId, body)).toEqual({ status: 400, body: { error: "invalid" } });
    }
    expect((await call("PATCH", "/auth/me", userId, { name: ` ${"x".repeat(80)} ` })).status).toBe(200);
  });

  it("never logs the name", async () => {
    const { userId } = await member(newId("org"), "member");
    const logged: unknown[] = [];
    const spies = (["log", "info", "warn", "error", "debug"] as const).map((level) =>
      vi.spyOn(console, level).mockImplementation((...args: unknown[]) => void logged.push(...args))
    );
    try {
      await workosControl("__fail", { path: `/user_management/users/${userId}`, times: 1 });
      expect(await call("PATCH", "/auth/me", userId, { name: "Secret Person" })).toEqual({
        status: 502,
        body: { error: "upstream" }
      });
      const text = logged.map((l) => String(l)).join("\n");
      expect(text).not.toContain("Secret");
      expect(text).not.toContain(userId);
    } finally {
      for (const spy of spies) spy.mockRestore();
    }
  });
});

describe("GET /auth/orgs/{id}/members", () => {
  it("lists members by membership id, never by user id", async () => {
    const { orgId, admin, member: m } = await org();
    // The admin is in the directory, with a username; the member only in WorkOS.
    const seeded = (await workos()).users.find((u) => u.id === admin.userId)!;
    await directory().upsert({
      ...seeded,
      email: admin.email,
      first_name: "Grace",
      last_name: "Hopper",
      profile_picture_url: "https://img.example.net/g.png",
      created_at: seeded.updated_at
    } as never);
    const username = `g${crypto.randomUUID().replaceAll("-", "").slice(0, 10)}`;
    await directory().claimUsername(admin.userId, username, 1);
    await call("PATCH", "/auth/me", m.userId, { name: "Ada Lovelace" });

    const res = await call("GET", `/auth/orgs/${orgId}/members`, m.userId);
    expect(res).toEqual({
      status: 200,
      body: {
        members: [
          {
            id: admin.membershipId,
            name: "Grace Hopper",
            email: admin.email,
            role: "admin",
            you: false,
            avatar: "https://img.example.net/g.png",
            title: null,
            username,
            joinedAt: expect.any(String)
          },
          {
            id: m.membershipId,
            name: "Ada Lovelace",
            email: m.email,
            role: "member",
            you: true,
            avatar: null,
            title: null,
            username: null,
            joinedAt: expect.any(String)
          }
        ]
      }
    });
    const text = JSON.stringify(res.body);
    expect(text).not.toContain(admin.userId);
    expect(text).not.toContain(m.userId);
  });

  it("is for the org's members only", async () => {
    const { orgId } = await org();
    const outsider = await org();
    expect(await call("GET", `/auth/orgs/${orgId}/members`, outsider.admin.userId)).toEqual({
      status: 403,
      body: { error: "forbidden" }
    });
  });

  it("reads every page, up to 200 members", async () => {
    const orgId = newId("org");
    const admin = await member(orgId, "admin");
    for (let i = 0; i < 204; i++) {
      await workosControl("__memberships", { user_id: newId("user"), organization_id: orgId, role: "member" });
    }
    const res = await call("GET", `/auth/orgs/${orgId}/members`, admin.userId);
    const members = res.body.members as { id: string; you: boolean; name: string | null }[];
    expect(members).toHaveLength(200);
    expect(new Set(members.map((x) => x.id)).size).toBe(200);
    expect(members[0]).toMatchObject({ id: admin.membershipId, you: true });
  });
});

describe("the org admin routes", () => {
  it("are for the org's admins only", async () => {
    const { orgId, admin, member: m } = await org();
    const invitation = await workosControl("__invitations", {
      email: "a@example.net",
      organization_id: orgId,
      state: "pending",
      expires_at: inAWeek()
    });
    const outsider = (await org()).admin.userId;
    for (const caller of [m.userId, outsider]) {
      for (const [method, path, body] of [
        ["PATCH", `/auth/orgs/${orgId}`, { name: "Taken" }],
        ["PATCH", `/auth/orgs/${orgId}/members/${admin.membershipId}`, { role: "member" }],
        ["DELETE", `/auth/orgs/${orgId}/members/${admin.membershipId}`],
        ["GET", `/auth/orgs/${orgId}/invitations`],
        ["DELETE", `/auth/orgs/${orgId}/invitations/${invitation.id}`],
        ["POST", `/auth/orgs/${orgId}/invitations/${invitation.id}/resend`]
      ] as [string, string, unknown?][]) {
        expect(await call(method, path, caller, body), `${method} ${path}`).toEqual({ status: 403, body: { error: "forbidden" } });
      }
    }
    const state = await workos();
    expect(state.memberships.find((x) => x.id === admin.membershipId)?.role).toBe("admin");
    expect(state.invitations.find((i) => i.id === invitation.id)).toMatchObject({ state: "pending", sends: 1 });
    expect(state.orgs.find((o) => o.id === orgId)).toBeUndefined();
  });

  it("rename the org", async () => {
    const { orgId, admin } = await org();
    expect(await call("PATCH", `/auth/orgs/${orgId}`, admin.userId, { name: "  Acme Labs " })).toEqual({
      status: 200,
      body: { organizationId: orgId, name: "Acme Labs" }
    });
    expect(await call("PATCH", `/auth/orgs/${orgId}`, admin.userId, { name: " " })).toEqual({
      status: 400,
      body: { error: "invalid" }
    });
  });

  it("change another member's role and remove them", async () => {
    const { orgId, admin, member: m } = await org();
    const path = `/auth/orgs/${orgId}/members/${m.membershipId}`;
    expect(await call("PATCH", path, admin.userId, { role: "owner" })).toEqual({ status: 400, body: { error: "invalid" } });
    expect(await call("PATCH", path, admin.userId, { role: "admin" })).toEqual({ status: 200, body: { ok: true } });
    expect((await workos()).memberships.find((x) => x.id === m.membershipId)?.role).toBe("admin");

    expect(await call("DELETE", path, admin.userId)).toEqual({ status: 200, body: { ok: true } });
    expect((await workos()).memberships.find((x) => x.id === m.membershipId)).toBeUndefined();
    expect(await call("DELETE", path, admin.userId)).toEqual({ status: 404, body: { error: "not_found" } });
  });

  it("answer 400 self for the caller's own membership", async () => {
    const { orgId, admin } = await org();
    const path = `/auth/orgs/${orgId}/members/${admin.membershipId}`;
    expect(await call("DELETE", path, admin.userId)).toEqual({ status: 400, body: { error: "self" } });
    expect(await call("PATCH", path, admin.userId, { role: "member" })).toEqual({ status: 400, body: { error: "self" } });
    expect((await workos()).memberships.find((x) => x.id === admin.membershipId)?.role).toBe("admin");
  });

  it("answer 404 for another org's membership or invitation", async () => {
    const { orgId, admin } = await org();
    const other = await org();
    const theirs = await workosControl("__invitations", {
      email: "b@example.net",
      organization_id: other.orgId,
      state: "pending",
      expires_at: inAWeek()
    });
    const notFound = { status: 404, body: { error: "not_found" } };
    expect(await call("DELETE", `/auth/orgs/${orgId}/members/${other.member.membershipId}`, admin.userId)).toEqual(notFound);
    expect(await call("PATCH", `/auth/orgs/${orgId}/members/${other.member.membershipId}`, admin.userId, { role: "admin" })).toEqual(
      notFound
    );
    expect(await call("DELETE", `/auth/orgs/${orgId}/invitations/${theirs.id}`, admin.userId)).toEqual(notFound);
    expect(await call("POST", `/auth/orgs/${orgId}/invitations/${theirs.id}/resend`, admin.userId)).toEqual(notFound);
    expect(await call("DELETE", `/auth/orgs/${orgId}/invitations/invitation_missing`, admin.userId)).toEqual(notFound);
    const state = await workos();
    expect(state.memberships.find((x) => x.id === other.member.membershipId)?.role).toBe("member");
    expect(state.invitations.find((i) => i.id === theirs.id)).toMatchObject({ state: "pending", sends: 1 });
  });

  it("map WorkOS's invalid_role to 400 invalid_role", async () => {
    const { orgId, admin, member: m } = await org();
    // As where the environment has no `admin` role.
    await workosControl("__fail", {
      path: `/user_management/organization_memberships/${m.membershipId}`,
      method: "PUT",
      times: 1,
      status: 422,
      code: "invalid_role"
    });
    expect(await call("PATCH", `/auth/orgs/${orgId}/members/${m.membershipId}`, admin.userId, { role: "admin" })).toEqual({
      status: 400,
      body: { error: "invalid_role" }
    });
    expect((await workos()).memberships.find((x) => x.id === m.membershipId)?.role).toBe("member");
  });
});

describe("org invitations", () => {
  it("lists pending ones and those expired in the last 30 days", async () => {
    const { orgId, admin } = await org();
    const seed = (email: string, state: string, expires_at: string) =>
      workosControl("__invitations", { email, organization_id: orgId, state, expires_at });
    const pending = await seed("pending@example.net", "pending", inAWeek());
    const lapsed = await seed("lapsed@example.net", "pending", daysAgo(1));
    const expired = await seed("expired@example.net", "expired", daysAgo(20));
    await seed("old@example.net", "expired", daysAgo(40));
    await seed("revoked@example.net", "revoked", inAWeek());
    await seed("accepted@example.net", "accepted", inAWeek());
    const newest = await seed("newest@example.net", "pending", inAWeek());

    const res = await call("GET", `/auth/orgs/${orgId}/invitations`, admin.userId);
    expect(res.status).toBe(200);
    expect(res.body.invitations).toEqual([
      { id: newest.id, email: "newest@example.net", state: "pending", expiresAt: newest.expires_at, createdAt: newest.created_at },
      { id: expired.id, email: "expired@example.net", state: "expired", expiresAt: expired.expires_at, createdAt: expired.created_at },
      { id: lapsed.id, email: "lapsed@example.net", state: "expired", expiresAt: lapsed.expires_at, createdAt: lapsed.created_at },
      { id: pending.id, email: "pending@example.net", state: "pending", expiresAt: pending.expires_at, createdAt: pending.created_at }
    ]);
  });

  it("revokes one", async () => {
    const { orgId, admin } = await org();
    const sent = await call("POST", `/auth/orgs/${orgId}/invitations`, admin.userId, { emails: ["r@example.net"] });
    expect(sent.body.sent).toEqual(["r@example.net"]);
    const [invitation] = (await workos()).invitations.filter((i) => i.organization_id === orgId);
    expect(await call("DELETE", `/auth/orgs/${orgId}/invitations/${invitation!.id}`, admin.userId)).toEqual({
      status: 200,
      body: { ok: true }
    });
    expect((await workos()).invitations.find((i) => i.id === invitation!.id)?.state).toBe("revoked");
    expect((await call("GET", `/auth/orgs/${orgId}/invitations`, admin.userId)).body.invitations).toEqual([]);
  });

  it("resends one, or revokes it and invites again where WorkOS can't resend", async () => {
    const { orgId, admin } = await org();
    const invitation = await workosControl("__invitations", {
      email: "again@example.net",
      organization_id: orgId,
      state: "expired",
      expires_at: daysAgo(2)
    });
    const path = `/auth/orgs/${orgId}/invitations/${invitation.id}/resend`;
    expect(await call("POST", path, admin.userId)).toEqual({ status: 200, body: { ok: true } });
    expect((await workos()).invitations.find((i) => i.id === invitation.id)).toMatchObject({ state: "pending", sends: 2 });

    await workosControl("__fail", { path: `/user_management/invitations/${invitation.id}/resend`, times: 1, status: 404 });
    expect(await call("POST", path, admin.userId)).toEqual({ status: 200, body: { ok: true } });
    const mine = (await workos()).invitations.filter((i) => i.organization_id === orgId);
    expect(mine.map((i) => [i.email, i.state])).toEqual([
      ["again@example.net", "revoked"],
      ["again@example.net", "pending"]
    ]);

    // A revoked one isn't resent.
    expect(await call("POST", path, admin.userId)).toEqual({ status: 409, body: { error: "not_pending" } });
  });

  it("counts a resend against the hourly invitation quota", async () => {
    const { orgId, admin } = await org();
    // The hour's 20, ten at a time.
    const emails = Array.from({ length: 10 }, (_, i) => `q${i}-${newId("x")}@example.net`);
    for (let i = 0; i < 2; i++) {
      await call("POST", `/auth/orgs/${orgId}/invitations`, admin.userId, { emails: emails.map((e) => `${i}${e}`) });
    }
    const [invitation] = (await workos()).invitations.filter((i) => i.organization_id === orgId);
    expect(await call("POST", `/auth/orgs/${orgId}/invitations/${invitation!.id}/resend`, admin.userId)).toEqual({
      status: 429,
      body: { error: "rate_limited" }
    });
  });
});
