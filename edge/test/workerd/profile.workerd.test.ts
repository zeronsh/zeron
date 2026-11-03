import { SELF, env } from "cloudflare:test";
import { describe, expect, it, vi } from "vitest";
import { MAX_PHOTO_BYTES, handleAuthRoute } from "../../src/auth-routes";
import type { Env } from "../../src/env";
import { signWorkOsPayload } from "../../src/user-directory";

// Settings → Profile's own routes: a title (PATCH /auth/me {title}), a photo
// of one's own (PUT|DELETE /auth/me/photo, served at GET /auth/avatars/{key})
// and deleting the account (DELETE /auth/me). WorkOS is workos-fake.ts.

const edgeEnv = { ...(env as unknown as Env), WORKOS_API_KEY: "sk_test" };

const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));

const newId = (prefix: string) => `${prefix}_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;

const send = async (method: string, path: string, bearer?: string, init: { json?: unknown; bytes?: Uint8Array; type?: string } = {}) => {
  const url = new URL(`https://edge.test${path}`);
  const headers: Record<string, string> = bearer ? { authorization: `Bearer ${bearer}` } : {};
  let body: BodyInit | undefined;
  if (init.json !== undefined) {
    headers["content-type"] = "application/json";
    body = JSON.stringify(init.json);
  } else if (init.bytes) {
    headers["content-type"] = init.type ?? "image/png";
    body = init.bytes;
  }
  return (await handleAuthRoute(new Request(url, { method, headers, body }), edgeEnv, url))!;
};

const call = async (method: string, path: string, bearer?: string, json?: unknown) => {
  const res = await send(method, path, bearer, { json });
  return { status: res.status, body: (await res.json()) as Record<string, unknown> };
};

const workosControl = (path: "__memberships" | "__users", body: unknown) =>
  fetch(`https://api.workos.com/${path}`, { method: "POST", body: JSON.stringify(body) }).then(
    (r) => r.json() as Promise<Record<string, string>>
  );

interface FakeState {
  users: { id: string; updated_at: string }[];
  memberships: { id: string; user_id: string; organization_id: string; role: string }[];
  invitations: { organization_id: string; state: string }[];
  deletedOrgs: string[];
}
const workos = async (): Promise<FakeState> => (await fetch("https://api.workos.com/__state")).json();

const member = async (orgId: string, role: "admin" | "member", profile: Record<string, string> = {}) => {
  const userId = newId("user");
  const email = `${newId("person")}@example.net`;
  await workosControl("__users", { id: userId, email, ...profile });
  const membership = await workosControl("__memberships", { user_id: userId, organization_id: orgId, role, email });
  return { userId, email, membershipId: membership.id! };
};

/** The directory has them (their webhook landed). */
const recorded = async (userId: string, email: string) => {
  const seeded = (await workos()).users.find((u) => u.id === userId)!;
  await directory().upsert({ ...seeded, email, profile_picture_url: null, created_at: seeded.updated_at } as never);
};

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 73, 72, 68, 82]);
const JPEG = new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 0, 16, 74, 70, 73, 70]);

const keyOf = (avatar: unknown): string => {
  const m = /\/auth\/avatars\/([0-9a-f]{32})$/.exec(String(avatar));
  expect(m, String(avatar)).not.toBeNull();
  return m![1]!;
};

describe("Profile's routes' bearer", () => {
  it("answers 401 without one and 403 for a runner token", async () => {
    const orgId = newId("org");
    const { userId } = await member(orgId, "member");
    for (const [method, path] of [
      ["DELETE", "/auth/me"],
      ["PUT", "/auth/me/photo"],
      ["DELETE", "/auth/me/photo"]
    ] as const) {
      expect((await send(method, path)).status, `${method} ${path}`).toBe(401);
      expect((await send(method, path, `runner:${userId}@${orgId}:cloud-1`)).status, `${method} ${path}`).toBe(403);
    }
    expect((await workos()).users.some((u) => u.id === userId)).toBe(true);
  });
});

describe("a title", () => {
  it("is set, read back, shown to the org, and cleared; the name is left alone", async () => {
    const orgId = newId("org");
    const { userId, email } = await member(orgId, "member", { first_name: "Ada", last_name: "Lovelace" });
    await recorded(userId, email);
    expect(await call("PATCH", "/auth/me", userId, { title: "  Analyst  " })).toEqual({
      status: 200,
      body: { firstName: "Ada", lastName: "Lovelace", email, name: "Ada Lovelace", title: "Analyst", avatar: null }
    });
    expect((await call("GET", "/auth/me", userId)).body.title).toBe("Analyst");
    const members = (await call("GET", `/auth/orgs/${orgId}/members`, userId)).body.members as { title: string | null }[];
    expect(members[0]!.title).toBe("Analyst");
    // Name and title together; then cleared with "" (or null).
    expect((await call("PATCH", "/auth/me", userId, { name: "Ada King", title: "Countess" })).body).toMatchObject({
      name: "Ada King",
      title: "Countess"
    });
    expect((await call("PATCH", "/auth/me", userId, { title: "" })).body.title).toBeNull();
    expect((await call("PATCH", "/auth/me", userId, { title: "Again" })).body.title).toBe("Again");
    expect((await call("PATCH", "/auth/me", userId, { title: null })).body.title).toBeNull();
  });

  it("records someone whose webhook hasn't landed yet", async () => {
    const { userId } = await member(newId("org"), "member", { first_name: "New" });
    expect((await call("PATCH", "/auth/me", userId, { title: "Early" })).body.title).toBe("Early");
    expect((await directory().profiles([userId]))[userId]?.title).toBe("Early");
  });

  it("answers 400 invalid for one that isn't a string of at most 80 characters", async () => {
    const { userId } = await member(newId("org"), "member");
    for (const body of [{}, { title: 3 }, { title: "x".repeat(81) }, { title: ["a"] }]) {
      expect(await call("PATCH", "/auth/me", userId, body), JSON.stringify(body)).toEqual({ status: 400, body: { error: "invalid" } });
    }
    expect((await call("PATCH", "/auth/me", userId, { title: ` ${"x".repeat(80)} ` })).status).toBe(200);
  });
});

describe("an image of one's own (a project's icon)", () => {
  it("is stored under a key of its own, served to anyone, and deleted only by its owner", async () => {
    const { userId } = await member(newId("org"), "member");
    const { userId: other } = await member(newId("org"), "member");
    const put = await send("PUT", "/auth/me/images", userId, { bytes: PNG });
    expect(put.status).toBe(200);
    const { key, url } = (await put.json()) as { key: string; url: string };
    expect(keyOf(url)).toBe(key);
    expect(url).not.toContain(userId);
    // Not their photo: the profile is untouched.
    expect((await call("GET", "/auth/me", userId)).body.avatar).toBeNull();

    const served = await send("GET", `/auth/avatars/${key}`);
    expect(served.status).toBe(200);
    expect(served.headers.get("cache-control")).toContain("immutable");
    expect(new Uint8Array(await served.arrayBuffer())).toEqual(PNG);

    // Someone else's delete is a missing one's; a photo isn't deletable here.
    expect((await send("DELETE", `/auth/me/images/${key}`, other)).status).toBe(404);
    const photo = keyOf(((await (await send("PUT", "/auth/me/photo", userId, { bytes: PNG })).json()) as { avatar: string }).avatar);
    expect((await send("DELETE", `/auth/me/images/${photo}`, userId)).status).toBe(404);
    expect((await send("GET", `/auth/avatars/${photo}`)).status).toBe(200);

    expect((await call("DELETE", `/auth/me/images/${key}`, userId)).body).toEqual({ ok: true });
    expect((await send("GET", `/auth/avatars/${key}`)).status).toBe(404);
    expect((await send("DELETE", `/auth/me/images/${key}`, userId)).status).toBe(404);
  });

  it("refuses what isn't a PNG or JPEG, is too large, or comes without a person's bearer", async () => {
    const orgId = newId("org");
    const { userId } = await member(orgId, "member");
    expect((await send("PUT", "/auth/me/images", userId, { bytes: PNG, type: "image/gif" })).status).toBe(400);
    expect((await send("PUT", "/auth/me/images", userId, { bytes: new Uint8Array([1, 2, 3]), type: "image/jpeg" })).status).toBe(400);
    const big = new Uint8Array(MAX_PHOTO_BYTES + 1);
    big.set(PNG);
    expect((await send("PUT", "/auth/me/images", userId, { bytes: big })).status).toBe(413);
    expect((await send("PUT", "/auth/me/images")).status).toBe(401);
    expect((await send("PUT", "/auth/me/images", `runner:${userId}@${orgId}:cloud-1`, { bytes: PNG })).status).toBe(403);
    expect((await send("DELETE", `/auth/me/images/${"0".repeat(32)}`)).status).toBe(401);
  });
});

describe("a photo", () => {
  it("is stored under a key of its own, served to anyone, replaced and removed", async () => {
    const orgId = newId("org");
    const { userId, email } = await member(orgId, "member", { profile_picture_url: "https://img.example.net/signin.png" });
    await recorded(userId, email);
    expect((await call("GET", "/auth/me", userId)).body.avatar).toBe("https://img.example.net/signin.png");

    const put = await send("PUT", "/auth/me/photo", userId, { bytes: PNG });
    expect(put.status).toBe(200);
    const first = keyOf(((await put.json()) as { avatar: string }).avatar);
    expect(first).not.toContain(userId);
    expect((await call("GET", "/auth/me", userId)).body.avatar).toBe(`https://edge.test/auth/avatars/${first}`);
    const members = (await call("GET", `/auth/orgs/${orgId}/members`, userId)).body.members as { avatar: string }[];
    expect(members[0]!.avatar).toBe(`https://edge.test/auth/avatars/${first}`);

    // Anyone may see it, without a bearer; it never changes, so it caches.
    const served = await send("GET", `/auth/avatars/${first}`);
    expect(served.status).toBe(200);
    expect(served.headers.get("content-type")).toBe("image/png");
    expect(served.headers.get("cache-control")).toContain("immutable");
    expect(new Uint8Array(await served.arrayBuffer())).toEqual(PNG);

    // Replaced: a new key, the old one gone.
    const again = await send("PUT", "/auth/me/photo", userId, { bytes: JPEG, type: "image/jpeg" });
    const second = keyOf(((await again.json()) as { avatar: string }).avatar);
    expect(second).not.toBe(first);
    expect((await send("GET", `/auth/avatars/${first}`)).status).toBe(404);
    expect((await send("GET", `/auth/avatars/${second}`)).headers.get("content-type")).toBe("image/jpeg");

    // Removed: back to the sign-in's.
    const removed = await send("DELETE", "/auth/me/photo", userId);
    expect(((await removed.json()) as { avatar: string }).avatar).toBe("https://img.example.net/signin.png");
    expect((await send("GET", `/auth/avatars/${second}`)).status).toBe(404);
  });

  it("refuses what isn't a PNG or JPEG, or is too large", async () => {
    const { userId } = await member(newId("org"), "member");
    expect((await send("PUT", "/auth/me/photo", userId, { bytes: PNG, type: "image/gif" })).status).toBe(400);
    expect((await send("PUT", "/auth/me/photo", userId, { bytes: JPEG, type: "image/png" })).status).toBe(400);
    expect((await send("PUT", "/auth/me/photo", userId, { bytes: new Uint8Array([1, 2, 3]), type: "image/jpeg" })).status).toBe(400);
    const big = new Uint8Array(MAX_PHOTO_BYTES + 1);
    big.set(PNG);
    expect((await send("PUT", "/auth/me/photo", userId, { bytes: big })).status).toBe(413);
    expect((await call("GET", "/auth/me", userId)).body.avatar).toBeNull();
  });

  it("answers 404 for a key that isn't one", async () => {
    for (const key of ["nope", "../secret", "A".repeat(32), "0".repeat(32)]) {
      expect((await send("GET", `/auth/avatars/${encodeURIComponent(key)}`)).status, key).toBe(404);
    }
  });
});

describe("deleting an account", () => {
  it("needs the account's address typed back", async () => {
    const { userId, email } = await member(newId("org"), "member");
    for (const body of [undefined, {}, { confirm: "" }, { confirm: "   " }, { confirm: "someone@example.net" }, { confirm: 1 }]) {
      expect((await call("DELETE", "/auth/me", userId, body)).status, JSON.stringify(body)).toBe(400);
    }
    expect((await workos()).users.some((u) => u.id === userId)).toBe(true);
    // (Case and spaces aside.)
    expect((await call("DELETE", "/auth/me", userId, { confirm: ` ${email.toUpperCase()} ` })).body).toEqual({ ok: true });
  });

  it("is refused for the only admin of an org others are in, until there's another", async () => {
    const orgId = newId("org");
    const admin = await member(orgId, "admin");
    const other = await member(orgId, "member");
    expect(await call("DELETE", "/auth/me", admin.userId, { confirm: admin.email })).toEqual({
      status: 409,
      body: { error: "last_admin", organization: orgId }
    });
    expect((await workos()).users.some((u) => u.id === admin.userId)).toBe(true);
    // Another admin: fine.
    await call("PATCH", `/auth/orgs/${orgId}/members/${other.membershipId}`, admin.userId, { role: "admin" });
    expect((await call("DELETE", "/auth/me", admin.userId, { confirm: admin.email })).status).toBe(200);
    // Alone in an org: fine too.
    const solo = await member(newId("org"), "admin");
    expect((await call("DELETE", "/auth/me", solo.userId, { confirm: solo.email })).status).toBe(200);
  });

  it("deletes them in WorkOS and tombstones them here at once: memberships, username and photo go", async () => {
    const orgId = newId("org");
    const { userId, email } = await member(orgId, "member", { first_name: "Gone" });
    await recorded(userId, email);
    const username = `d${crypto.randomUUID().replaceAll("-", "").slice(0, 10)}`;
    expect((await directory().claimUsername(userId, username, 1)).error).toBeUndefined();
    await call("PATCH", "/auth/me", userId, { title: "Leaving" });
    const key = keyOf(((await (await send("PUT", "/auth/me/photo", userId, { bytes: PNG })).json()) as { avatar: string }).avatar);

    const logged: unknown[] = [];
    const spies = (["log", "warn", "error"] as const).map((level) =>
      vi.spyOn(console, level).mockImplementation((...args: unknown[]) => void logged.push(...args))
    );
    try {
      expect(await call("DELETE", "/auth/me", userId, { confirm: email })).toEqual({ status: 200, body: { ok: true } });
    } finally {
      for (const spy of spies) spy.mockRestore();
    }
    const state = await workos();
    expect(state.users.some((u) => u.id === userId)).toBe(false);
    expect(state.memberships.some((m) => m.user_id === userId)).toBe(false);
    expect(await directory().profiles([userId])).toEqual({});
    expect(await directory().username(userId)).toBeNull();
    expect(await directory().card(username)).toBeUndefined();
    expect((await send("GET", `/auth/avatars/${key}`)).status).toBe(404);
    // Nothing of theirs in the logs.
    const text = JSON.stringify(logged);
    expect(text).not.toContain(userId);
    expect(text).not.toContain(email);
  });

  it("is safe to retry: asked again once it's done, it answers the same and changes nothing", async () => {
    const orgId = newId("org");
    const { userId, email } = await member(orgId, "member");
    await recorded(userId, email);
    expect(await call("DELETE", "/auth/me", userId, { confirm: email })).toEqual({ status: 200, body: { ok: true } });
    const after = await workos();
    expect(await call("DELETE", "/auth/me", userId, { confirm: email })).toEqual({ status: 200, body: { ok: true } });
    expect(await workos()).toEqual(after);
    // Still never without the address typed.
    expect((await call("DELETE", "/auth/me", userId, {})).status).toBe(400);
  });

  it("finishes the edge's part on a retry when the first answer was lost after WorkOS deleted them", async () => {
    const orgId = newId("org");
    const { userId, email } = await member(orgId, "member");
    await recorded(userId, email);
    const username = `r${crypto.randomUUID().replaceAll("-", "").slice(0, 10)}`;
    expect((await directory().claimUsername(userId, username, 1)).error).toBeUndefined();
    const key = keyOf(((await (await send("PUT", "/auth/me/photo", userId, { bytes: PNG })).json()) as { avatar: string }).avatar);
    // WorkOS deleted them, but the edge never got to its part.
    await fetch(`https://api.workos.com/user_management/users/${userId}`, { method: "DELETE" });
    expect(await directory().username(userId)).toBe(username);
    // A wrong address is still refused while their address is on record.
    expect((await call("DELETE", "/auth/me", userId, { confirm: "someone@example.net" })).status).toBe(400);
    expect(await call("DELETE", "/auth/me", userId, { confirm: email })).toEqual({ status: 200, body: { ok: true } });
    expect(await directory().profiles([userId])).toEqual({});
    expect(await directory().username(userId)).toBeNull();
    expect((await send("GET", `/auth/avatars/${key}`)).status).toBe(404);
    // Retired: no one else can take the name.
    const other = await member(newId("org"), "member");
    await recorded(other.userId, other.email);
    expect((await directory().claimUsername(other.userId, username, 1)).error).toBeDefined();
  });

  it("leaves nothing changed when WorkOS fails, and a retry then finishes", async () => {
    const orgId = newId("org");
    const { userId, email } = await member(orgId, "member");
    await member(orgId, "admin");
    await recorded(userId, email);
    await fetch("https://api.workos.com/__fail", {
      method: "POST",
      body: JSON.stringify({ path: `/user_management/users/${userId}`, times: 1, method: "DELETE" })
    });
    expect(await call("DELETE", "/auth/me", userId, { confirm: email })).toEqual({ status: 502, body: { error: "upstream" } });
    expect((await workos()).users.some((u) => u.id === userId)).toBe(true);
    expect(Object.keys(await directory().profiles([userId]))).toEqual([userId]);
    expect((await call("DELETE", "/auth/me", userId, { confirm: email })).status).toBe(200);
    expect((await workos()).users.some((u) => u.id === userId)).toBe(false);
  });

  it("takes an org they're the only member of with them, its invitations too; one with others stays", async () => {
    const solo = newId("org");
    const shared = newId("org");
    const { userId, email } = await member(solo, "admin");
    await workosControl("__memberships", { user_id: userId, organization_id: shared, role: "member", email });
    const stays = await member(shared, "admin");
    await fetch("https://api.workos.com/__invitations", {
      method: "POST",
      body: JSON.stringify({ email: "invitee@example.net", organization_id: solo, state: "pending", expires_at: new Date(Date.now() + 86_400_000).toISOString() })
    });
    expect((await call("DELETE", "/auth/me", userId, { confirm: email })).status).toBe(200);
    const state = await workos();
    expect(state.deletedOrgs).toContain(solo);
    expect(state.deletedOrgs).not.toContain(shared);
    expect(state.invitations.some((i) => i.organization_id === solo)).toBe(false);
    expect(state.memberships.filter((m) => m.organization_id === shared).map((m) => m.user_id)).toEqual([stays.userId]);
  });
});

describe("a user deleted in WorkOS (the webhook)", () => {
  it("takes their own photo with them", async () => {
    const { userId, email } = await member(newId("org"), "member");
    await recorded(userId, email);
    const key = keyOf(((await (await send("PUT", "/auth/me/photo", userId, { bytes: PNG })).json()) as { avatar: string }).avatar);
    const secret = "workos_whsec_test";
    const payload = JSON.stringify({
      id: `event_${crypto.randomUUID()}`,
      event: "user.deleted",
      created_at: new Date().toISOString(),
      data: {
        object: "user",
        id: userId,
        email,
        first_name: null,
        last_name: null,
        profile_picture_url: null,
        email_verified: true,
        created_at: "2026-10-01T10:00:00.000Z",
        updated_at: "2026-10-01T10:00:00.000Z"
      }
    });
    const t = Date.now();
    const res = await SELF.fetch("https://edge.test/workos/webhook", {
      method: "POST",
      headers: { "content-type": "application/json", "workos-signature": `t=${t}, v1=${await signWorkOsPayload(secret, t, payload)}` },
      body: payload
    });
    expect(res.status).toBe(200);
    expect((await send("GET", `/auth/avatars/${key}`)).status).toBe(404);
    expect(await directory().profiles([userId])).toEqual({});
  });
});
