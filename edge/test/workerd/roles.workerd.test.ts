import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { ADMIN, directory, errorOf, grantAdmin, newId, person, webSignIn, workos } from "./identity-helpers";

const roles = (path: string, body?: unknown, headers: Record<string, string> = ADMIN) =>
  SELF.fetch(`https://edge.test/admin/roles${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers,
    body: body === undefined ? undefined : JSON.stringify(body)
  });

const admin = (bearer: string) => SELF.fetch("https://edge.test/store/v1/admin/queue", { headers: { authorization: `Bearer ${bearer}` } });

describe("the admin role", () => {
  it("is granted, listed and revoked by an operator, with who did it", async () => {
    const userId = await person("rolesowner");
    const granted = await roles("/grant", { userId, role: "admin", by: "wing", note: "first reviewer" });
    expect(await granted.json()).toEqual({ granted: true });
    // Granting again changes nothing.
    expect(await (await roles("/grant", { userId, role: "admin", by: "someone" })).json()).toEqual({ granted: false });
    const listed = ((await (await roles("")).json()) as { roles: Record<string, unknown>[] }).roles.find((r) => r.userId === userId);
    expect(listed).toMatchObject({ role: "admin", username: "rolesowner", grantedBy: "wing", note: "first reviewer", revokedAt: null });
    expect(await (await roles("/revoke", { userId, role: "admin", by: "wing" })).json()).toEqual({ revoked: true });
    expect(await (await roles("/revoke", { userId, role: "admin", by: "wing" })).json()).toEqual({ revoked: false });
    const live = ((await (await roles("")).json()) as { roles: { userId: string }[] }).roles;
    expect(live.some((r) => r.userId === userId)).toBe(false);
    const all = ((await (await roles("?all=1")).json()) as { roles: Record<string, unknown>[] }).roles.find((r) => r.userId === userId);
    expect(all).toMatchObject({ revokedBy: "wing", revokedAt: expect.any(Number) });
    // Granted afresh after a revoke.
    expect(await (await roles("/grant", { userId, role: "admin", by: "wing" })).json()).toEqual({ granted: true });
    expect(await directory().roles(userId)).toEqual(["admin"]);
  });

  it("needs the operator token, a known role, a live user and who's granting", async () => {
    const userId = await person();
    expect((await roles("", undefined, {})).status).toBe(401);
    expect((await roles("/grant", { userId, role: "admin", by: "x" }, { authorization: "Bearer wrong", "content-type": "application/json" })).status).toBe(401);
    expect((await roles("/grant", { userId, role: "admin", by: "x" }, { authorization: `Bearer ${userId}`, "content-type": "application/json" })).status).toBe(401);
    for (const body of [
      { userId, role: "owner", by: "x" },
      { userId, role: "admin" },
      { userId, role: "admin", by: "" },
      { userId: "nobody", role: "admin", by: "x" },
      { userId, role: "admin", by: "x", note: 5 }
    ]) {
      expect((await roles("/grant", body)).status, JSON.stringify(body)).toBe(400);
    }
    const unknown = await roles("/grant", { userId: newId("user"), role: "admin", by: "x" });
    expect(unknown.status).toBe(404);
    expect(await errorOf(unknown)).toBe("unknown_user");
    expect((await roles("/grant", { userId, role: "admin", by: "x" }, { ...ADMIN })).status).toBe(200);
    for (const [method, path] of [["DELETE", "/admin/roles"], ["GET", "/admin/roles/grant"]] as const) {
      const res = await SELF.fetch(`https://edge.test${path}`, { method, headers: ADMIN });
      expect(res.status).toBe(405);
      expect(await errorOf(res)).toBe("method_not_allowed");
    }
  });

  it("is the edge's own record: a WorkOS admin role counts for nothing", async () => {
    const userId = await person();
    // Every user is admin of their own personal organization in WorkOS…
    const orgId = newId("org");
    const membership = (await workos("__memberships", { user_id: userId, organization_id: orgId, role: "admin" })) as Record<string, string>;
    await directory().upsertMembership({
      id: membership.id!,
      organization_id: orgId,
      user_id: userId,
      role: "admin",
      status: "active",
      created_at: membership.created_at!,
      updated_at: membership.created_at!
    });
    // …and their token can carry that org.
    for (const bearer of [userId, `${userId}@${orgId}`]) {
      const res = await admin(bearer);
      expect(res.status, bearer).toBe(403);
      expect(await errorOf(res)).toBe("forbidden");
    }
    const browser = await webSignIn(userId);
    expect((await SELF.fetch("https://site.test/store/v1/admin/queue", { headers: { cookie: browser.cookie } })).status).toBe(403);
    expect((await (await SELF.fetch("https://site.test/auth/web/session", { headers: { cookie: browser.cookie } })).json()) as { roles: string[] }).toMatchObject({
      roles: []
    });
    // The edge's grant is what counts, on the web.
    await grantAdmin(userId);
    const staff = await webSignIn(userId);
    const res = await SELF.fetch("https://site.test/store/v1/admin/queue", { headers: { cookie: staff.cookie } });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ admin: { userId } });
  });

  it("refuses an admin's valid bearer on every admin route of the contract, and takes their session", async () => {
    const userId = await person();
    await grantAdmin(userId);
    const staff = await webSignIn(userId);
    const routes: [string, string][] = [
      ["GET", "/store/v1/admin/queue?status=pending"],
      ["GET", "/store/v1/admin/submissions/demo/1.0.0"],
      ["POST", "/store/v1/admin/submissions/demo/1.0.0/approve"],
      ["POST", "/store/v1/admin/submissions/demo/1.0.0/reject"],
      ["GET", "/store/v1/admin/plugins/demo"],
      ...["suspend", "restore", "feature", "transfer"].map((a): [string, string] => ["POST", `/store/v1/admin/plugins/demo/${a}`]),
      ["POST", "/store/v1/admin/plugins/demo/versions/1.0.0/remove"],
      ["GET", "/store/v1/admin/reports"],
      ["POST", "/store/v1/admin/reports/r1/resolve"],
      ["GET", "/store/v1/admin/audit"],
      ["GET", "/store/v1/admin/reservations"],
      ["POST", "/store/v1/admin/reservations"]
    ];
    for (const [method, path] of routes) {
      const bearer = await SELF.fetch(`https://site.test${path}`, { method, headers: { authorization: `Bearer ${userId}`, origin: "https://site.test" } });
      expect(bearer.status, `${method} ${path}`).toBe(403);
      expect(await errorOf(bearer)).toBe("forbidden");
      const session = await SELF.fetch(`https://site.test${path}`, {
        method,
        headers: { cookie: staff.cookie, origin: "https://site.test", "x-csrf-token": staff.csrf }
      });
      expect(session.status, `${method} ${path} (session)`).toBe(200);
      await session.arrayBuffer();
    }
  });

  it("is by web session only: never a bearer, a runner token or a deploy key", async () => {
    const userId = await person();
    await grantAdmin(userId);
    // An admin's own bearer, org-scoped or not, even beside their live web session.
    const staff = await webSignIn(userId);
    for (const bearer of [userId, `${userId}@org1`]) {
      const res = await admin(bearer);
      expect(res.status, bearer).toBe(403);
      expect(await errorOf(res)).toBe("forbidden");
      const both = await SELF.fetch("https://site.test/store/v1/admin/queue", { headers: { authorization: `Bearer ${bearer}`, cookie: staff.cookie } });
      expect(both.status).toBe(403);
    }
    const runner = await admin(`runner:${userId}@org1:cloud-1`);
    expect(runner.status).toBe(403);
    const key = await SELF.fetch("https://edge.test/store/v1/admin/queue", { headers: { authorization: `ZeronKey key=${"A".repeat(43)}, ts=1, sig=${"A".repeat(86)}` } });
    expect(key.status).toBe(403);
    expect((await SELF.fetch("https://edge.test/store/v1/admin/queue")).status).toBe(401);
    expect((await SELF.fetch("https://edge.test/store/v1/admin/queue", { headers: { authorization: "Bearer " } })).status).toBe(403);
    // A bearer in the query string is never read here.
    expect((await SELF.fetch(`https://edge.test/store/v1/admin/queue?token=${userId}`)).status).toBe(401);
  });
});
