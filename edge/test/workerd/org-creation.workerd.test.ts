import { env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { handleAuthRoute } from "../../src/auth-routes";
import type { Env } from "../../src/env";
import { ORG_CLAIM_STALE_MS, type UserDirectory } from "../../src/user-directory";

// WorkOS is the fake in workos-fake.ts (outbound host api.workos.com), shared
// by every test here: each test uses its own user.

const edgeEnv = { ...(env as unknown as Env), WORKOS_API_KEY: "sk_test" };

const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));

const newUserId = () => `user_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;

const createOrg = async (bearer: string, name = "Acme") => {
  const url = new URL("https://edge.test/auth/orgs");
  const res = (await handleAuthRoute(
    new Request(url, {
      method: "POST",
      headers: { authorization: `Bearer ${bearer}`, "content-type": "application/json" },
      body: JSON.stringify({ name })
    }),
    edgeEnv,
    url
  ))!;
  return { status: res.status, body: (await res.json()) as { organizationId?: string | null; error?: string } };
};

interface WorkOsState {
  orgs: { id: string; name: string }[];
  memberships: { user_id: string; organization_id: string; role: string }[];
}

const workos = async (): Promise<WorkOsState> => (await fetch("https://api.workos.com/__state")).json();

/** Orgs the fake WorkOS made `userId` admin of. */
const adminOrgs = async (userId: string) =>
  (await workos()).memberships.filter((m) => m.user_id === userId && m.role === "admin").map((m) => m.organization_id);

const workosControl = (path: "__fail" | "__memberships", body: unknown) =>
  fetch(`https://api.workos.com/${path}`, { method: "POST", body: JSON.stringify(body) });

describe("POST /auth/orgs: one created org per user", () => {
  it("creates the first org and answers 409 org_exists with it after that", async () => {
    const userId = newUserId();
    const first = await createOrg(userId);
    expect(first.status).toBe(200);
    expect(first.body.organizationId).toMatch(/^org_/);

    const second = await createOrg(userId, "Another");
    expect(second).toEqual({ status: 409, body: { error: "org_exists", organizationId: first.body.organizationId } });
    expect(await adminOrgs(userId)).toEqual([first.body.organizationId]);
  });

  it("lets concurrent requests from one user create only one org", async () => {
    const userId = newUserId();
    const results = await Promise.all(Array.from({ length: 5 }, () => createOrg(userId)));
    const created = results.filter((r) => r.status === 200);
    expect(created).toHaveLength(1);
    for (const r of results.filter((r) => r.status !== 200)) {
      expect(r.status).toBe(409);
      expect(r.body.error).toBe("org_exists");
    }
    expect(await adminOrgs(userId)).toEqual([created[0]!.body.organizationId]);
    expect(await createOrg(userId)).toEqual({
      status: 409,
      body: { error: "org_exists", organizationId: created[0]!.body.organizationId }
    });
  });

  it("gives the claim back when WorkOS fails, so the user can try again", async () => {
    const userId = newUserId();
    await workosControl("__fail", { path: "/organizations", times: 1 });
    const failed = await createOrg(userId);
    expect(failed.status).toBe(401);
    expect(await adminOrgs(userId)).toEqual([]);

    const retried = await createOrg(userId);
    expect(retried.status).toBe(200);
    expect(await adminOrgs(userId)).toEqual([retried.body.organizationId]);
  });

  it("counts an org the user already administers, from before creations were recorded", async () => {
    const userId = newUserId();
    await workosControl("__memberships", { user_id: userId, organization_id: "org_legacy1", role: "admin" });
    expect(await createOrg(userId)).toEqual({ status: 409, body: { error: "org_exists", organizationId: "org_legacy1" } });
    expect(await adminOrgs(userId)).toEqual(["org_legacy1"]);
    // Nothing is held: the answer comes from WorkOS each time.
    expect(await createOrg(userId)).toEqual({ status: 409, body: { error: "org_exists", organizationId: "org_legacy1" } });
  });

  it("lets a member invited to someone else's org create their first", async () => {
    const userId = newUserId();
    await workosControl("__memberships", { user_id: userId, organization_id: "org_invited1", role: "member" });
    const created = await createOrg(userId);
    expect(created.status).toBe(200);
    expect(await adminOrgs(userId)).toEqual([created.body.organizationId]);
  });

  it("still forbids a runner token", async () => {
    const userId = newUserId();
    expect((await createOrg(`runner:${userId}@org1:cloud-1`)).status).toBe(403);
    expect(await adminOrgs(userId)).toEqual([]);
  });

  it("holds a claim in flight, and takes over one a crashed create left behind", async () => {
    const userId = newUserId();
    const claim = await directory().claimOrgCreation(userId);
    expect(claim).toEqual({ claimId: expect.any(String) });
    // Another request is creating right now.
    expect(await createOrg(userId)).toEqual({ status: 409, body: { error: "org_exists", organizationId: null } });

    // That request died: once its claim is stale, the next one creates.
    await runInDurableObject(directory(), (_instance: UserDirectory, state) => {
      state.storage.sql.exec(
        "UPDATE org_creations SET claimed_at = ? WHERE user_id = ?",
        Date.now() - ORG_CLAIM_STALE_MS - 1,
        userId
      );
    });
    const created = await createOrg(userId);
    expect(created.status).toBe(200);
    // The dead request can't record over the new claim.
    await directory().finishOrgCreation(userId, (claim as { claimId: string }).claimId, "org_dead");
    expect(await createOrg(userId)).toEqual({
      status: 409,
      body: { error: "org_exists", organizationId: created.body.organizationId }
    });
  });

  it("answers from WorkOS when a crashed create got as far as the org", async () => {
    const userId = newUserId();
    await directory().claimOrgCreation(userId);
    await workosControl("__memberships", { user_id: userId, organization_id: "org_halfmade1", role: "admin" });
    await runInDurableObject(directory(), (_instance: UserDirectory, state) => {
      state.storage.sql.exec("UPDATE org_creations SET claimed_at = 0 WHERE user_id = ?", userId);
    });
    expect(await createOrg(userId)).toEqual({ status: 409, body: { error: "org_exists", organizationId: "org_halfmade1" } });
    expect(await adminOrgs(userId)).toEqual(["org_halfmade1"]);
  });

  it("shows who created which org in the admin lists", async () => {
    const userId = newUserId();
    const created = await createOrg(userId);
    const orgId = created.body.organizationId!;
    const stamp = "2026-10-30T10:00:00.000Z";
    await directory().upsert({
      id: userId,
      email: `${userId}@example.com`,
      first_name: "Ada",
      last_name: null,
      profile_picture_url: null,
      created_at: stamp,
      updated_at: stamp
    });
    await directory().upsertOrganization({ id: orgId, name: "Acme", external_id: null, domains: [], created_at: stamp, updated_at: stamp });

    const users = await directory().list();
    expect(users.find((u) => u.userId === userId)?.createdOrgId).toBe(orgId);
    const orgs = await directory().listOrganizations();
    expect(orgs.find((o) => o.orgId === orgId)?.createdBy).toBe(userId);
  });
});
