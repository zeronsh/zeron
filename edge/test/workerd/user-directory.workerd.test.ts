import { SELF } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { signWorkOsPayload, verifyWorkOsSignature } from "../../src/user-directory";

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
    email: string | null;
    name: string | null;
    profilePictureUrl: string | null;
    joinedAt: string;
    deletedAt: string | null;
    badges: { id: string; grantedAt: number; note: string | null }[];
  }[];
};

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
    const res = await webhook("organization.created", { id: "org_1" });
    expect(await res.json()).toEqual({ received: true });
  });

  it("lists users only with the admin token, as JSON or CSV", async () => {
    expect((await listUsers("nope")).status).toBe(401);
    await webhook("user.created", user("user_csv", { first_name: "Grace, Jr.", last_name: null }));
    const csv = await (
      await SELF.fetch("https://edge.test/admin/users?format=csv", { headers: { authorization: "Bearer test-admin-token" } })
    ).text();
    expect(csv.split("\n")[0]).toBe("user_id,email,name,first_name,last_name,profile_picture_url,joined_at,updated_at,deleted_at,badges");
    expect(csv).toContain('user_csv,user_csv@example.com,"Grace, Jr.","Grace, Jr.",,');
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
});
