import { SELF, env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { UserDirectory, signWorkOsPayload, verifyWorkOsSignature } from "../../src/user-directory";

const SECRET = "workos_whsec_test";

const user = (id: string, overrides: Record<string, unknown> = {}) => ({
  object: "user",
  id,
  email: `${id}@example.com`,
  first_name: "Ada",
  last_name: "Lovelace",
  profile_picture_url: "https://workoscdn.com/images/v1/abc",
  email_verified: true,
  created_at: "2026-10-01T10:00:00.000Z",
  updated_at: "2026-10-01T10:00:00.000Z",
  ...overrides
});

/** POST a WorkOS-signed webhook (or a badly signed one). */
const webhook = async (event: string, data: Record<string, unknown>, opts: { secret?: string; t?: number } = {}) => {
  const payload = JSON.stringify({ id: `event_${crypto.randomUUID()}`, event, data, created_at: new Date().toISOString() });
  const t = opts.t ?? Date.now();
  const v1 = await signWorkOsPayload(opts.secret ?? SECRET, t, payload);
  return SELF.fetch("https://edge.test/workos/webhook", {
    method: "POST",
    headers: { "content-type": "application/json", "workos-signature": `t=${t}, v1=${v1}` },
    body: payload
  });
};

type Listed = {
  count: number;
  users: {
    userId: string;
    userNumber: number;
    email: string | null;
    name: string | null;
    profilePictureUrl: string | null;
    joinedAt: string;
    deletedAt: string | null;
    badges: { id: string; grantedAt: number; note: string | null }[];
  }[];
};

const organization = (id: string, overrides: Record<string, unknown> = {}) => ({
  object: "organization",
  id,
  name: "Foo Corp",
  external_id: null,
  domains: [
    { object: "organization_domain", id: "org_domain_1", organization_id: id, domain: "foo-corp.com", state: "verified" }
  ],
  metadata: {},
  created_at: "2026-10-01T10:00:00.000Z",
  updated_at: "2026-10-01T10:00:00.000Z",
  ...overrides
});

const membership = (id: string, orgId: string, userId: string, overrides: Record<string, unknown> = {}) => ({
  object: "organization_membership",
  id,
  user_id: userId,
  organization_id: orgId,
  status: "active",
  role: { slug: "member" },
  roles: [{ slug: "member" }],
  custom_attributes: {},
  directory_managed: false,
  created_at: "2026-10-01T10:00:00.000Z",
  updated_at: "2026-10-01T10:00:00.000Z",
  ...overrides
});

type ListedOrgs = {
  count: number;
  organizations: {
    orgId: string;
    name: string | null;
    externalId: string | null;
    domains: string[];
    createdAt: string;
    deletedAt: string | null;
    memberCount: number;
    members: {
      membershipId: string;
      userId: string;
      userNumber: number | null;
      email: string | null;
      name: string | null;
      role: string | null;
      status: string;
      joinedAt: string;
    }[];
  }[];
};

const listOrgs = async (token = "test-admin-token") =>
  SELF.fetch("https://edge.test/admin/organizations", { headers: { authorization: `Bearer ${token}` } });

const findOrg = async (id: string) => ((await (await listOrgs()).json()) as ListedOrgs).organizations.find((o) => o.orgId === id);

const listUsers = async (token = "test-admin-token") =>
  SELF.fetch("https://edge.test/admin/users", { headers: { authorization: `Bearer ${token}` } });

const find = async (id: string) => ((await (await listUsers()).json()) as Listed).users.find((u) => u.userId === id);

describe("user directory", () => {
  it("verifies WorkOS signatures within tolerance only", async () => {
    const payload = '{"id":"event_1"}';
    const t = 1_800_000_000_000;
    const v1 = await signWorkOsPayload("s", t, payload);
    expect(await verifyWorkOsSignature("s", `t=${t}, v1=${v1}`, payload, t + 1000)).toBe(true);
    expect(await verifyWorkOsSignature("s", `t=${t}, v1=${v1}`, `${payload} `, t)).toBe(false);
    expect(await verifyWorkOsSignature("other", `t=${t}, v1=${v1}`, payload, t)).toBe(false);
    expect(await verifyWorkOsSignature("s", `t=${t}, v1=${v1}`, payload, t + 181_000)).toBe(false);
    expect(await verifyWorkOsSignature("s", null, payload, t)).toBe(false);
  });

  it("records a new user's name, email, picture and join date", async () => {
    const res = await webhook("user.created", user("user_created1"));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ received: true, applied: true });
    expect(await find("user_created1")).toMatchObject({
      email: "user_created1@example.com",
      name: "Ada Lovelace",
      profilePictureUrl: "https://workoscdn.com/images/v1/abc",
      joinedAt: "2026-10-01T10:00:00.000Z",
      deletedAt: null
    });
  });

  it("rejects bad signatures and stale timestamps", async () => {
    expect((await webhook("user.created", user("user_forged"), { secret: "wrong" })).status).toBe(400);
    expect((await webhook("user.created", user("user_forged"), { t: Date.now() - 10 * 60_000 })).status).toBe(400);
    expect(await find("user_forged")).toBeUndefined();
  });

  it("keeps the newest profile when deliveries arrive out of order", async () => {
    await webhook("user.updated", user("user_order", { first_name: "New", updated_at: "2026-10-02T00:00:00.000Z" }));
    const stale = await webhook("user.created", user("user_order", { first_name: "Old" }));
    expect(await stale.json()).toEqual({ received: true, applied: false });
    expect((await find("user_order"))?.name).toBe("New Lovelace");
  });

  it("clears a deleted user's profile and never revives it", async () => {
    await webhook("user.created", user("user_gone"));
    await webhook("user.deleted", user("user_gone"));
    await webhook("user.updated", user("user_gone", { updated_at: "2026-10-09T00:00:00.000Z" }));
    const gone = await find("user_gone");
    expect(gone).toMatchObject({ email: null, name: null, profilePictureUrl: null, joinedAt: "2026-10-01T10:00:00.000Z" });
    expect(gone?.deletedAt).toBeTruthy();
  });

  it("acknowledges events it doesn't track", async () => {
    const res = await webhook("session.created", { id: "session_1" });
    expect(await res.json()).toEqual({ received: true });
  });

  it("lists users only with the admin token, as JSON or CSV", async () => {
    expect((await listUsers("nope")).status).toBe(401);
    await webhook("user.created", user("user_csv", { first_name: "Grace, Jr.", last_name: null }));
    const csv = await (
      await SELF.fetch("https://edge.test/admin/users?format=csv", { headers: { authorization: "Bearer test-admin-token" } })
    ).text();
    expect(csv.split("\n")[0]).toBe(
      "user_number,user_id,email,name,first_name,last_name,profile_picture_url,joined_at,updated_at,deleted_at,badges,username"
    );
    const number = (await find("user_csv"))?.userNumber;
    expect(csv).toContain(`\n${number},user_csv,user_csv@example.com,"Grace, Jr.","Grace, Jr.",,`);
  });

  it("gives each new user the next number, kept through updates and deletion", async () => {
    await webhook("user.created", user("user_num1"));
    await webhook("user.created", user("user_num2", { created_at: "2026-09-01T00:00:00.000Z" }));
    const first = (await find("user_num1"))!.userNumber;
    // Numbers follow when a user is recorded, not when they joined.
    expect((await find("user_num2"))!.userNumber).toBe(first + 1);

    await webhook("user.updated", user("user_num1", { first_name: "Renamed", updated_at: "2026-10-05T00:00:00.000Z" }));
    await webhook("user.created", user("user_num1"));
    await webhook("user.deleted", user("user_num1"));
    expect(await find("user_num1")).toMatchObject({ userNumber: first, name: null });

    // A user first seen through their deletion still takes a number.
    await webhook("user.deleted", user("user_num3"));
    expect((await find("user_num3"))!.userNumber).toBe(first + 2);

    const numbers = ((await (await listUsers()).json()) as Listed).users.map((u) => u.userNumber);
    expect(new Set(numbers).size).toBe(numbers.length);
  });

  it("numbers users recorded before numbers existed once, in join order", async () => {
    const stub = env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("legacy-directory"));
    await runInDurableObject(stub, (_instance: UserDirectory, state) => {
      const sql = state.storage.sql;
      // The table as it was before numbers.
      sql.exec("DROP INDEX users_by_number");
      sql.exec("ALTER TABLE users DROP COLUMN user_number");
      const insert = (id: string, joined: string) =>
        sql.exec(
          "INSERT INTO users (user_id, created_at, updated_at, recorded_at) VALUES (?, ?, ?, ?)",
          id,
          joined,
          joined,
          Date.now()
        );
      insert("user_late", "2026-10-03T00:00:00.000Z");
      insert("user_early", "2026-10-01T00:00:00.000Z");
      insert("user_tie_b", "2026-10-02T00:00:00.000Z");
      insert("user_tie_a", "2026-10-02T00:00:00.000Z");

      const directory = new UserDirectory(state, env as never);
      const numbered = () => Object.fromEntries(directory.list().map((u) => [u.userId, u.userNumber]));
      expect(numbered()).toEqual({ user_early: 1, user_tie_a: 2, user_tie_b: 3, user_late: 4 });

      // The next sign-up continues the sequence; a restart renumbers nothing.
      directory.upsert(user("user_after") as never);
      new UserDirectory(state, env as never);
      expect(numbered()).toEqual({ user_early: 1, user_tie_a: 2, user_tie_b: 3, user_late: 4, user_after: 5 });
    });
  });

  const admin = (path: string, body?: unknown, token = "test-admin-token") =>
    SELF.fetch(`https://edge.test${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body)
    });

  it("defines badges and grants them to users, once each", async () => {
    expect((await admin("/admin/badges", undefined, "nope")).status).toBe(401);
    expect((await admin("/admin/badges", { id: "Cloud Tester", name: "x" })).status).toBe(400);
    expect((await admin("/admin/badges/grant", { badgeId: "nobody-defined", userIds: ["user_a"] })).status).toBe(404);

    const defined = await admin("/admin/badges", { id: "early-bird", name: "Early Bird", description: "Here first" });
    expect(await defined.json()).toMatchObject({ id: "early-bird", name: "Early Bird", description: "Here first", icon: null, holders: 0 });

    await webhook("user.created", user("user_badge1"));
    const first = await admin("/admin/badges/grant", { badgeId: "early-bird", userIds: ["user_badge1", "user_badge1"], note: "beta" });
    expect(await first.json()).toEqual({ granted: 1 });
    // Re-granting keeps the first grant.
    expect(await (await admin("/admin/badges/grant", { badgeId: "early-bird", userIds: ["user_badge1"] })).json()).toEqual({ granted: 0 });
    const held = (await find("user_badge1"))?.badges as { id: string; note: string | null }[];
    expect(held).toMatchObject([{ id: "early-bird", note: "beta" }]);

    // Renaming keeps holders.
    await admin("/admin/badges", { id: "early-bird", name: "Early Bird 2", icon: "sparkle" });
    const listed = (await (await admin("/admin/badges")).json()) as { badges: { id: string; name: string; icon: string | null; holders: number }[] };
    expect(listed.badges.find((b) => b.id === "early-bird")).toMatchObject({ name: "Early Bird 2", icon: "sparkle", holders: 1 });

    expect(await (await admin("/admin/badges/revoke", { badgeId: "early-bird", userIds: ["user_badge1"] })).json()).toEqual({ revoked: 1 });
    expect((await find("user_badge1"))?.badges).toEqual([]);
  });

  it("grants a badge to every live user at once, and a delete drops it", async () => {
    await webhook("user.created", user("user_all1"));
    await webhook("user.created", user("user_all2"));
    await admin("/admin/badges", { id: "cloud-tester", name: "Cloud Tester" });
    const res = (await (await admin("/admin/badges/grant", { badgeId: "cloud-tester", userIds: "all" })).json()) as { granted: number };
    expect(res.granted).toBeGreaterThanOrEqual(2);
    expect((await find("user_all1"))?.badges).toMatchObject([{ id: "cloud-tester" }]);
    // "all" is grant-only.
    expect((await admin("/admin/badges/revoke", { badgeId: "cloud-tester", userIds: "all" })).status).toBe(400);

    await webhook("user.deleted", user("user_all2"));
    expect((await find("user_all2"))?.badges).toEqual([]);
  });

  it("records an organization with its members' profiles and roles", async () => {
    const res = await webhook("organization.created", organization("org_team", { external_id: "acme" }));
    expect(await res.json()).toEqual({ received: true, applied: true });
    await webhook("user.created", user("user_team1"));
    await webhook("user.created", user("user_team2", { first_name: "Grace", last_name: "Hopper" }));
    await webhook("organization_membership.created", membership("om_team1", "org_team", "user_team1", { role: { slug: "admin" } }));
    await webhook(
      "organization_membership.created",
      membership("om_team2", "org_team", "user_team2", { created_at: "2026-10-02T10:00:00.000Z", updated_at: "2026-10-02T10:00:00.000Z" })
    );
    await webhook(
      "organization_membership.created",
      membership("om_team3", "org_team", "user_team3", { status: "pending", created_at: "2026-10-03T10:00:00.000Z", updated_at: "2026-10-03T10:00:00.000Z" })
    );

    const org = await findOrg("org_team");
    expect(org).toMatchObject({ name: "Foo Corp", externalId: "acme", domains: ["foo-corp.com"], createdAt: "2026-10-01T10:00:00.000Z", deletedAt: null });
    // Pending members are listed but not counted.
    expect(org?.memberCount).toBe(2);
    expect(org?.members).toMatchObject([
      { userId: "user_team1", email: "user_team1@example.com", name: "Ada Lovelace", role: "admin", status: "active", joinedAt: "2026-10-01T10:00:00.000Z" },
      { userId: "user_team2", email: "user_team2@example.com", name: "Grace Hopper", role: "member", status: "active" },
      { userId: "user_team3", email: null, name: null, userNumber: null, status: "pending" }
    ]);
    expect(org?.members[0]?.userNumber).toEqual((await find("user_team1"))?.userNumber);
  });

  it("shows a member's profile whichever delivery lands first", async () => {
    await webhook("organization_membership.created", membership("om_early", "org_early", "user_early_member"));
    // The membership lands ahead of its org: nothing to list yet.
    expect(await findOrg("org_early")).toBeUndefined();
    await webhook("organization.created", organization("org_early"));
    expect((await findOrg("org_early"))?.members).toMatchObject([{ userId: "user_early_member", email: null }]);
    await webhook("user.created", user("user_early_member"));
    expect((await findOrg("org_early"))?.members).toMatchObject([{ email: "user_early_member@example.com", name: "Ada Lovelace" }]);
  });

  it("keeps the newest organization and membership when deliveries arrive out of order", async () => {
    await webhook("organization.updated", organization("org_order", { name: "New", updated_at: "2026-10-02T00:00:00.000Z" }));
    const staleOrg = await webhook("organization.created", organization("org_order", { name: "Old" }));
    expect(await staleOrg.json()).toEqual({ received: true, applied: false });
    expect((await findOrg("org_order"))?.name).toBe("New");

    await webhook(
      "organization_membership.updated",
      membership("om_order", "org_order", "user_order_member", { role: { slug: "admin" }, updated_at: "2026-10-02T00:00:00.000Z" })
    );
    const staleMember = await webhook("organization_membership.created", membership("om_order", "org_order", "user_order_member"));
    expect(await staleMember.json()).toEqual({ received: true, applied: false });
    expect((await findOrg("org_order"))?.members).toMatchObject([{ role: "admin" }]);
  });

  it("clears a deleted organization, drops its members and never revives it", async () => {
    await webhook("organization.created", organization("org_gone", { external_id: "gone" }));
    await webhook("organization_membership.created", membership("om_gone", "org_gone", "user_gone_member"));
    await webhook("organization.deleted", organization("org_gone"));
    await webhook("organization.updated", organization("org_gone", { updated_at: "2026-10-09T00:00:00.000Z" }));
    // A late membership for the deleted org stays out of the listing.
    await webhook("organization_membership.created", membership("om_gone2", "org_gone", "user_gone_member2"));
    const gone = await findOrg("org_gone");
    expect(gone).toMatchObject({ name: null, externalId: null, domains: [], createdAt: "2026-10-01T10:00:00.000Z", memberCount: 0, members: [] });
    expect(gone?.deletedAt).toBeTruthy();

    // An org first seen through its deletion is still recorded.
    await webhook("organization.deleted", organization("org_never_seen"));
    expect((await findOrg("org_never_seen"))?.deletedAt).toBeTruthy();
  });

  it("drops a removed membership and never revives it", async () => {
    await webhook("organization.created", organization("org_left"));
    await webhook("organization_membership.created", membership("om_left", "org_left", "user_left"));
    await webhook("organization_membership.created", membership("om_stays", "org_left", "user_stays"));
    await webhook("organization_membership.deleted", membership("om_left", "org_left", "user_left"));
    await webhook("organization_membership.updated", membership("om_left", "org_left", "user_left", { updated_at: "2026-10-09T00:00:00.000Z" }));
    // A delete that outruns its create leaves nothing to revive.
    await webhook("organization_membership.deleted", membership("om_late", "org_left", "user_late"));
    await webhook("organization_membership.created", membership("om_late", "org_left", "user_late"));
    const org = await findOrg("org_left");
    expect(org?.members.map((m) => m.membershipId)).toEqual(["om_stays"]);
    expect(org?.memberCount).toBe(1);
  });

  it("counts only active members, and drops deleted users", async () => {
    await webhook("organization.created", organization("org_status"));
    await webhook("user.created", user("user_status1"));
    await webhook("user.created", user("user_status2"));
    await webhook("organization_membership.created", membership("om_status1", "org_status", "user_status1"));
    await webhook("organization_membership.created", membership("om_status2", "org_status", "user_status2"));
    await webhook("organization_membership.updated", membership("om_status1", "org_status", "user_status1", { status: "inactive", updated_at: "2026-10-02T00:00:00.000Z" }));
    const org = await findOrg("org_status");
    expect(org?.memberCount).toBe(1);
    expect(org?.members.map((m) => m.status)).toEqual(["inactive", "active"]);

    await webhook("user.deleted", user("user_status2"));
    const after = await findOrg("org_status");
    expect(after?.memberCount).toBe(0);
    expect(after?.members.map((m) => m.userId)).toEqual(["user_status1"]);
  });

  it("rejects events whose data isn't what they say", async () => {
    expect((await webhook("organization.created", { id: "org_bad" })).status).toBe(400);
    expect((await webhook("organization_membership.created", membership("nope", "org_x", "user_x"))).status).toBe(400);
    expect((await webhook("organization_membership.created", membership("om_x", "nope", "user_x"))).status).toBe(400);
    expect((await webhook("user.created", user("user_x", { email: undefined }))).status).toBe(400);
  });

  it("lists organizations only with the admin token, as JSON or CSV", async () => {
    expect((await listOrgs("nope")).status).toBe(401);
    await webhook("organization.created", organization("org_csv", { name: 'Foo, "Corp"' }));
    await webhook("user.created", user("user_csv_member"));
    await webhook("organization_membership.created", membership("om_csv", "org_csv", "user_csv_member"));
    await webhook("organization_membership.created", membership("om_csv2", "org_csv", "user_csv_no_profile"));
    await webhook("organization_membership.created", membership("om_csv3", "org_csv", "user_csv_pending", { status: "pending" }));
    const res = await SELF.fetch("https://edge.test/admin/organizations?format=csv", { headers: { authorization: "Bearer test-admin-token" } });
    expect(res.headers.get("content-disposition")).toContain("zeron-organizations.csv");
    const csv = await res.text();
    expect(csv.split("\n")[0]).toBe("org_id,name,external_id,domains,member_count,members,created_at,updated_at,deleted_at");
    expect(csv).toContain(
      '\norg_csv,"Foo, ""Corp""",,foo-corp.com,2,user_csv_member@example.com;user_csv_no_profile,2026-10-01T10:00:00.000Z,'
    );
  });
});
