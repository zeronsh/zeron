import { SELF, env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { handleAuthRoute } from "../../src/auth-routes";
import { b64urlEncode } from "../../src/cloud/crypto";
import type { Env } from "../../src/env";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import type { UserDirectory } from "../../src/user-directory";

// The PR #1137 review's edge findings (R4/R13 keys, R16 deletion, R17
// identity, R24 abuse), each pinned here. WorkOS is workos-fake.ts.

const edgeEnv = { ...(env as unknown as Env), WORKOS_API_KEY: "sk_test" };
const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));
const newId = (prefix: string) => `${prefix}_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;

/** An /auth/* route, as the Worker's own handler answers it. */
const auth = async (method: string, path: string, bearer?: string, init: { json?: unknown; bytes?: Uint8Array; type?: string } = {}) => {
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
  const res = (await handleAuthRoute(new Request(url, { method, headers, body }), edgeEnv, url))!;
  const text = await res.text();
  let parsed: Record<string, unknown> = {};
  try {
    parsed = JSON.parse(text) as Record<string, unknown>;
  } catch {
    /* not JSON */
  }
  return { status: res.status, body: parsed, headers: res.headers };
};

/** Through the whole Worker (the bearer gate, then /plugin-keys). */
const edge = (bearer: string | undefined, method: string, path: string, body?: unknown) =>
  SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: {
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  });

const workosControl = (path: string, body: unknown) =>
  fetch(`https://api.workos.com/${path}`, { method: "POST", body: JSON.stringify(body) }).then(
    (r) => r.json() as Promise<Record<string, string>>
  );
const workosState = async () =>
  (await (await fetch("https://api.workos.com/__state")).json()) as {
    users: { id: string }[];
    memberships: { id: string; user_id: string; organization_id: string; role: string }[];
    invitations: { organization_id: string }[];
  };

/** A WorkOS user (in the directory too), optionally in an org. */
const person = async (orgId?: string, role: "admin" | "member" = "member") => {
  const userId = newId("user");
  const email = `${newId("p")}@example.net`;
  const seeded = await workosControl("__users", { id: userId, email, first_name: "Ada" });
  await directory().upsert({ ...(seeded as never), profile_picture_url: null, created_at: seeded.updated_at });
  const membership = orgId ? await workosControl("__memberships", { user_id: userId, organization_id: orgId, role, email }) : undefined;
  return { userId, email, membershipId: membership?.id };
};

/** Sign in interactively (a code exchange) and keep the step-up it earns. */
const signIn = async (userId: string): Promise<string> => {
  const res = await auth("POST", "/auth/exchange", undefined, { json: { code: `code:${userId}` } });
  expect(res.status).toBe(200);
  const stepUp = res.body.stepUp as { token: string; expiresAt: number } | undefined;
  expect(stepUp?.token).toMatch(/^[A-Za-z0-9_-]{43}$/);
  return stepUp!.token;
};

interface Signer {
  readonly key: string;
  sign(message: string): Promise<string>;
}
const newSigner = async (): Promise<Signer> => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  return {
    key,
    sign: async (m) => b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, new TextEncoder().encode(m))))
  };
};
const register = async (userId: string, signer: Signer, stepUp?: string, deviceId = "dev-a") =>
  edge(userId, "POST", "/plugin-keys", {
    key: signer.key,
    deviceId,
    proof: await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key),
    ...(stepUp ? { stepUp } : {})
  });

describe("R4/R13: binding a plugin key needs a fresh sign-in", () => {
  it("refuses a new key on a bearer alone with 403 step_up_required (never a 409)", async () => {
    const { userId } = await person();
    const res = await register(userId, await newSigner());
    expect(res.status).toBe(403);
    expect(((await res.json()) as { error: string }).error).toBe("step_up_required");
    expect(((await (await edge(userId, "GET", "/plugin-keys")).json()) as { keys: unknown[] }).keys).toEqual([]);
  });

  it("binds it with the sign-in's ticket, once; the same key again needs none", async () => {
    const { userId } = await person();
    const ticket = await signIn(userId);
    const signer = await newSigner();
    expect((await register(userId, signer, ticket)).status).toBe(201);
    // Retried (an answer lost): no ticket needed, nothing new bound.
    expect((await register(userId, signer)).status).toBe(201);
    // The ticket is used up.
    const second = await register(userId, await newSigner(), ticket);
    expect(second.status).toBe(403);
  });

  it("refuses someone else's ticket, a refresh's, and an expired one", async () => {
    const { userId } = await person();
    const { userId: other } = await person();
    const theirs = await signIn(other);
    expect((await register(userId, await newSigner(), theirs)).status).toBe(403);
    // /auth/refresh never mints one.
    const refreshed = await auth("POST", "/auth/refresh", undefined, { json: { refreshToken: `rt:${userId}` } });
    expect(refreshed.status).toBe(200);
    expect(refreshed.body.stepUp).toBeUndefined();
    const ticket = await signIn(userId);
    await runInDurableObject(directory(), (_i: UserDirectory, state) => {
      state.storage.sql.exec("UPDATE step_ups SET expires_at = ? WHERE user_id = ?", Date.now() - 1, userId);
    });
    expect((await register(userId, await newSigner(), ticket)).status).toBe(403);
  });
});

describe("R13: keys are listed, revocable, and revocation is immediate", () => {
  it("lists keys with fingerprints, since a time, and answers no-store", async () => {
    const { userId } = await person();
    const first = await newSigner();
    expect((await register(userId, first, await signIn(userId), "laptop")).status).toBe(201);
    const listed = await edge(userId, "GET", "/plugin-keys");
    expect(listed.headers.get("cache-control")).toBe("no-store");
    const { keys, now } = (await listed.json()) as { keys: { key: string; fingerprint: string; deviceId: string }[]; now: number };
    expect(keys).toEqual([expect.objectContaining({ key: first.key, deviceId: "laptop", fingerprint: expect.stringMatching(/^([0-9a-f]{4} ){7}[0-9a-f]{4}$/) })]);
    expect(typeof now).toBe("number");

    await new Promise((r) => setTimeout(r, 5));
    const second = await newSigner();
    expect((await register(userId, second, await signIn(userId), "desktop")).status).toBe(201);
    const since = (await (await edge(userId, "GET", `/plugin-keys?since=${now}`)).json()) as { keys: { key: string }[] };
    expect(since.keys.map((k) => k.key)).toEqual([second.key]);
    expect((await edge(userId, "GET", "/plugin-keys?since=soon")).status).toBe(400);
  });

  it("reads revoked on the very next lookup, uncached", async () => {
    const owner = await person();
    await directory().claimUsername(owner.userId, `k${crypto.randomUUID().slice(0, 8)}`, 1);
    const signer = await newSigner();
    expect((await register(owner.userId, signer, await signIn(owner.userId))).status).toBe(201);
    const { userId: viewer } = await person();
    const before = await edge(viewer, "GET", `/plugin-keys/${signer.key}`);
    expect(before.headers.get("cache-control")).toBe("no-store");
    expect(((await before.json()) as { revoked: boolean }).revoked).toBe(false);
    expect((await edge(owner.userId, "DELETE", `/plugin-keys/${signer.key}`)).status).toBe(200);
    expect(await (await edge(viewer, "GET", `/plugin-keys/${signer.key}`)).json()).toMatchObject({ revoked: true, verified: false });
  });
});

describe("R17: a lookup says who, how sure, and what to compare", () => {
  it("answers fingerprint, first-seen and verified for a named owner", async () => {
    const owner = await person();
    const username = `v${crypto.randomUUID().replaceAll("-", "").slice(0, 8)}`;
    expect((await directory().claimUsername(owner.userId, username, 1)).error).toBeUndefined();
    const signer = await newSigner();
    expect((await register(owner.userId, signer, await signIn(owner.userId))).status).toBe(201);
    const { userId: viewer } = await person();
    const found = (await (await edge(viewer, "GET", `/plugin-keys/${signer.key}`)).json()) as Record<string, unknown>;
    expect(found).toMatchObject({ key: signer.key, username, mine: false, revoked: false, verified: true, revokedAt: null });
    expect(found.fingerprint).toMatch(/^([0-9a-f]{4} ){7}[0-9a-f]{4}$/);
    expect(Math.abs(Date.now() - (found.firstSeen as number))).toBeLessThan(60_000);
  });

  it("reports a key whose owner has no username as unknown to others, unverified to its owner", async () => {
    const owner = await person();
    const signer = await newSigner();
    expect((await register(owner.userId, signer, await signIn(owner.userId))).status).toBe(201);
    const { userId: viewer } = await person();
    const res = await edge(viewer, "GET", `/plugin-keys/${signer.key}`);
    expect(res.status).toBe(404);
    expect(((await res.json()) as { error: string }).error).toBe("unknown_key");
    expect(await (await edge(owner.userId, "GET", `/plugin-keys/${signer.key}`)).json()).toMatchObject({
      mine: true,
      username: null,
      verified: false
    });
  });

  it("keeps brand, staff and lookalike names from being claimed", async () => {
    const { userId } = await person();
    for (const name of ["github", "anthropic", "openai", "google", "apple", "microsoft", "wing", "admin", "support", "security", "zer0n", "the-zeron", "openai-dev", "github_bot", "g00gle", "adm1n", "zeron-support", "wingleeio"]) {
      const res = await edge(userId, "POST", "/username", { username: name });
      expect(res.status, name).toBe(403);
    }
    // Ordinary names still go.
    const ok = `pine${crypto.randomUUID().replaceAll("-", "").slice(0, 8)}`;
    expect((await edge(userId, "POST", "/username", { username: ok })).status).toBe(200);
  });
});

describe("R16: deleting the account needs a fresh sign-in", () => {
  it("refuses the address typed back on a bearer alone, and deletes with the ticket", async () => {
    const { userId, email } = await person();
    const bare = await auth("DELETE", "/auth/me", userId, { json: { confirm: email } });
    expect(bare).toMatchObject({ status: 403, body: { error: "step_up_required" } });
    expect(await auth("DELETE", "/auth/me", userId, { json: { confirm: email, stepUp: "x".repeat(43) } })).toMatchObject({ status: 403 });
    expect((await workosState()).users.some((u) => u.id === userId)).toBe(true);
    const ticket = await signIn(userId);
    expect(await auth("DELETE", "/auth/me", userId, { json: { confirm: email, stepUp: ticket } })).toMatchObject({ status: 200, body: { ok: true } });
    expect((await workosState()).users.some((u) => u.id === userId)).toBe(false);
  });

  it("refuses another user's ticket", async () => {
    const { userId, email } = await person();
    const { userId: other } = await person();
    expect((await auth("DELETE", "/auth/me", userId, { json: { confirm: email, stepUp: await signIn(other) } })).status).toBe(403);
  });
});

// A 1×1 PNG: IHDR, IDAT, IEND, CRCs valid.
const PNG = Uint8Array.from(atob("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg=="), (c) => c.charCodeAt(0));

/** A PNG chunk with a valid CRC. */
const chunk = async (type: string, data: Uint8Array): Promise<Uint8Array> => {
  const { crc32 } = await import("../../src/image-check");
  const out = new Uint8Array(12 + data.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, data.length);
  out.set(new TextEncoder().encode(type), 4);
  out.set(data, 8);
  view.setUint32(8 + data.length, crc32(out.subarray(4, 8 + data.length)));
  return out;
};
const join = (...parts: Uint8Array[]) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) (out.set(p, o), (o += p.length));
  return out;
};

describe("R24: images are checked, stripped, counted and go with the account", () => {
  it("strips text chunks and anything after IEND; refuses broken CRCs and unknown critical chunks", async () => {
    const { userId } = await person();
    const iend = PNG.length - 12;
    const withText = join(PNG.subarray(0, 33), await chunk("tEXt", new TextEncoder().encode("Comment\0<script>alert(1)</script>")), PNG.subarray(33), new TextEncoder().encode("<html>trailer"));
    const put = await auth("PUT", "/auth/me/images", userId, { bytes: withText });
    expect(put.status).toBe(200);
    const url = new URL(`https://edge.test/auth/avatars/${put.body.key}`);
    const served = (await handleAuthRoute(new Request(url), edgeEnv, url))!;
    expect(served.headers.get("content-security-policy")).toContain("sandbox");
    expect(new Uint8Array(await served.arrayBuffer())).toEqual(PNG);

    const badCrc = PNG.slice();
    badCrc[iend - 1] ^= 0xff;
    expect((await auth("PUT", "/auth/me/images", userId, { bytes: badCrc })).status).toBe(400);
    const unknownCritical = join(PNG.subarray(0, 33), await chunk("ZZZZ", new Uint8Array([1])), PNG.subarray(33));
    expect((await auth("PUT", "/auth/me/images", userId, { bytes: unknownCritical })).status).toBe(400);
    // A header that only starts like a PNG.
    expect((await auth("PUT", "/auth/me/images", userId, { bytes: PNG.subarray(0, 20) })).status).toBe(400);
  });

  it("caps how many images a user holds", async () => {
    const { userId } = await person();
    const { MAX_IMAGES_PER_USER } = await import("../../src/user-directory");
    await runInDurableObject(directory(), (_i: UserDirectory, state) => {
      for (let n = 0; n < MAX_IMAGES_PER_USER; n++) {
        state.storage.sql.exec(
          "INSERT INTO user_images (image_key, user_id, bytes, created_at) VALUES (?, ?, 10, ?)",
          `${userId}-${n}`,
          userId,
          Date.now() - 2 * 86_400_000
        );
      }
    });
    expect(await auth("PUT", "/auth/me/images", userId, { bytes: PNG })).toMatchObject({ status: 409, body: { error: "too_many_images" } });
  });

  it("deletes a user's images with their account", async () => {
    const { userId, email } = await person();
    const put = await auth("PUT", "/auth/me/images", userId, { bytes: PNG });
    expect(put.status).toBe(200);
    // One put before images were tracked: found by its owner metadata.
    await env.BLOBS.put("avatars/0123456789abcdef0123456789abcdef", PNG, { customMetadata: { owner: userId, kind: "image" } });
    const ticket = await signIn(userId);
    expect((await auth("DELETE", "/auth/me", userId, { json: { confirm: email, stepUp: ticket } })).status).toBe(200);
    expect(await env.BLOBS.head(`avatars/${put.body.key}`)).toBeNull();
    expect(await env.BLOBS.head("avatars/0123456789abcdef0123456789abcdef")).toBeNull();
  });
});

describe("R24: an org always keeps an admin", () => {
  it("two admins demoting each other at once leave one admin", async () => {
    const orgId = newId("org");
    const a = await person(orgId, "admin");
    const b = await person(orgId, "admin");
    await person(orgId, "member");
    const [first, second] = await Promise.all([
      auth("PATCH", `/auth/orgs/${orgId}/members/${b.membershipId}`, a.userId, { json: { role: "member" } }),
      auth("PATCH", `/auth/orgs/${orgId}/members/${a.membershipId}`, b.userId, { json: { role: "member" } })
    ]);
    expect([first.status, second.status].filter((s) => s === 200)).toHaveLength(1);
    const admins = (await workosState()).memberships.filter((m) => m.organization_id === orgId && m.role === "admin");
    expect(admins).toHaveLength(1);
  });

  it("two admins removing each other at once leave one", async () => {
    const orgId = newId("org");
    const a = await person(orgId, "admin");
    const b = await person(orgId, "admin");
    await Promise.all([
      auth("DELETE", `/auth/orgs/${orgId}/members/${b.membershipId}`, a.userId),
      auth("DELETE", `/auth/orgs/${orgId}/members/${a.membershipId}`, b.userId)
    ]);
    const admins = (await workosState()).memberships.filter((m) => m.organization_id === orgId && m.role === "admin");
    expect(admins).toHaveLength(1);
  });

  it("answers 409 busy while another change holds the org", async () => {
    const orgId = newId("org");
    const a = await person(orgId, "admin");
    const b = await person(orgId, "member");
    expect(await directory().lockOrg(orgId, "someone-else")).toBe(true);
    expect(await auth("PATCH", `/auth/orgs/${orgId}/members/${b.membershipId}`, a.userId, { json: { role: "admin" } })).toMatchObject({
      status: 409,
      body: { error: "busy" }
    });
    await directory().unlockOrg(orgId, "someone-else");
    expect((await auth("PATCH", `/auth/orgs/${orgId}/members/${b.membershipId}`, a.userId, { json: { role: "admin" } })).status).toBe(200);
  });
});

describe("R24: invitations", () => {
  it("caps a request at 10 addresses and a user at 20 an hour", async () => {
    const orgId = newId("org");
    const a = await person(orgId, "admin");
    const addresses = (n: number) => ({ emails: Array.from({ length: n }, () => `${newId("i")}@example.net`) });
    expect((await auth("POST", `/auth/orgs/${orgId}/invitations`, a.userId, { json: addresses(11) })).status).toBe(400);
    expect((await auth("POST", `/auth/orgs/${orgId}/invitations`, a.userId, { json: addresses(10) })).status).toBe(200);
    expect((await auth("POST", `/auth/orgs/${orgId}/invitations`, a.userId, { json: addresses(10) })).status).toBe(200);
    expect((await auth("POST", `/auth/orgs/${orgId}/invitations`, a.userId, { json: addresses(1) })).status).toBe(429);
  });

  it("caps a user at 50 a day", async () => {
    const orgId = newId("org");
    const a = await person(orgId, "admin");
    await runInDurableObject(directory(), (_i: UserDirectory, state) => {
      state.storage.sql.exec("INSERT INTO invite_quota_day (user_id, window_start, sent) VALUES (?, ?, 50)", a.userId, Date.now());
    });
    expect((await auth("POST", `/auth/orgs/${orgId}/invitations`, a.userId, { json: { emails: [`${newId("i")}@example.net`] } })).status).toBe(429);
  });

  it("refuses org names with links, addresses, lookalike letters or our name", async () => {
    for (const name of ["Verify at evil.com", "https://x.test", "Zeron Security", "Zer0n Team", "Acme‮gnp", "Pаypal Support", "Login www.example", "mail me@x.io", "GitHub", "Open AI"]) {
      const { userId } = await person();
      const created = await auth("POST", "/auth/orgs", userId, { json: { name } });
      expect(created.status, name).toBe(400);
      expect(created.body.error, name).toBe("invalid_name");
    }
    const { userId } = await person();
    expect((await auth("POST", "/auth/orgs", userId, { json: { name: "Pineapple Studio" } })).status).toBe(200);
  });

  it("checks a name when it's set: an org named before the rules keeps inviting, and can't be renamed to one", async () => {
    for (const old of ["hata.dev", "Zeron", "Acme‮gnp", "Pаypal"]) {
      const orgId = newId("org");
      const a = await person(orgId, "admin");
      // Named in WorkOS before the edge checked names.
      await fetch(`https://api.workos.com/organizations/${orgId}`, { method: "PUT", body: JSON.stringify({ name: old }) });
      const res = await auth("POST", `/auth/orgs/${orgId}/invitations`, a.userId, { json: { emails: [`${newId("i")}@example.net`] } });
      expect(res.status, old).toBe(200);
      expect((await workosState()).invitations.some((i) => i.organization_id === orgId), old).toBe(true);
      // A rename to such a name is refused.
      expect(await auth("PATCH", `/auth/orgs/${orgId}`, a.userId, { json: { name: old } }), old).toMatchObject({
        status: 400,
        body: { error: "invalid_name" }
      });
      expect((await auth("PATCH", `/auth/orgs/${orgId}`, a.userId, { json: { name: "Acme Studio" } })).status, old).toBe(200);
    }
  });

  it("refuses a person's name with a link (it's in the invite email)", async () => {
    const { userId } = await person();
    expect(await auth("PATCH", "/auth/me", userId, { json: { name: "Ada see evil.com" } })).toMatchObject({ status: 400, body: { error: "invalid_name" } });
    expect((await auth("PATCH", "/auth/me", userId, { json: { name: "Ada​Lovelace" } })).status).toBe(400);
    expect((await auth("PATCH", "/auth/me", userId, { json: { name: "J. R. Tolkien" } })).status).toBe(200);
  });
});
