import { SELF, env, runInDurableObject } from "cloudflare:test";
import type { Env } from "../../src/env";
import { signingKeyAllowed, type Publisher } from "../../src/store/principal";
import { describe, expect, it } from "vitest";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import { MAX_REQUEST_SKEW_MS, deployKeyProofMessage, publishSigningString, sha256Hex } from "../../src/store/identity/signing";
import {
  DEPLOY_KEYS_PER_PLUGIN_PER_DAY,
  DEPLOY_KEYS_PER_USER_PER_DAY,
  MAX_LIVE_DEPLOY_KEYS_PER_PLUGIN,
  type UserDirectory
} from "../../src/user-directory";
import {
  SITE,
  directory,
  errorOf,
  newId,
  newSigner,
  person,
  setOwner,
  siteChange,
  stepUp,
  webSignIn,
  type Signer
} from "./identity-helpers";

const newPlugin = () => `p${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;

const api = (bearer: string | undefined, method: string, path: string, body?: unknown) =>
  SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: {
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  });

/** An owner of a fresh plugin, as the store would record after their first publish. */
const owned = async (username?: string) => {
  const userId = await person(username);
  const pluginId = newPlugin();
  await setOwner(pluginId, userId);
  return { userId, pluginId };
};

const registration = async (pluginId: string, signer: Signer, extra: Record<string, unknown> = {}) => ({
  key: signer.key,
  name: "GitHub Actions",
  proof: await signer.sign(deployKeyProofMessage(pluginId, signer.key)),
  ...extra
});

/** Register a deploy key with a bearer and a fresh step-up. */
const addKey = async (userId: string, pluginId: string, signer?: Signer, extra: Record<string, unknown> = {}) => {
  const key = signer ?? (await newSigner());
  const res = await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, await registration(pluginId, key, { stepUp: await stepUp(userId), ...extra }));
  return { res, signer: key };
};

/** A request signed by `signer` as the contract says, its body's hash in
 * X-Zeron-Content-SHA256; the options change what's sent after signing. */
const signed = async (
  signer: Signer,
  path: string,
  body: string,
  opts: { ts?: number; method?: string; sendPath?: string; sendBody?: BodyInit; sendHash?: string | null; header?: string; headers?: Record<string, string> } = {}
) => {
  const ts = String(opts.ts ?? Date.now());
  const method = opts.method ?? "POST";
  const hash = await sha256Hex(new TextEncoder().encode(body));
  const sig = await signer.sign(publishSigningString(method, path, hash, ts));
  const sendHash = opts.sendHash === undefined ? hash : opts.sendHash;
  return SELF.fetch(`https://edge.test${opts.sendPath ?? path}`, {
    method,
    headers: {
      authorization: opts.header ?? `ZeronKey key=${signer.key}, ts=${ts}, sig=${sig}`,
      ...(sendHash === null ? {} : { "x-zeron-content-sha256": sendHash }),
      ...opts.headers
    },
    body: method === "GET" ? undefined : (opts.sendBody ?? body)
  });
};

const sql = (query: string, ...args: unknown[]) =>
  runInDurableObject(directory() as unknown as DurableObjectStub<UserDirectory>, (_i, state) => {
    state.storage.sql.exec(query, ...args);
  });

describe("deploy keys: registering", () => {
  it("binds a key to its plugin for the owner, listed without any secret", async () => {
    const { userId, pluginId } = await owned();
    const expires = Date.now() + 30 * 86_400_000;
    const { res, signer } = await addKey(userId, pluginId, undefined, { expires });
    expect(res.status).toBe(201);
    expect(res.headers.get("cache-control")).toBe("no-store");
    expect(await res.json()).toEqual({
      key: signer.key,
      plugin: pluginId,
      name: "GitHub Actions",
      created_at: expect.any(Number),
      expires_at: expires,
      fingerprint: expect.stringMatching(/^[0-9a-f]{4}( [0-9a-f]{4}){7}$/)
    });
    const list = (await (await api(userId, "GET", `/store/v1/plugins/${pluginId}/keys`)).json()) as { keys: Record<string, unknown>[] };
    expect(list.keys).toEqual([
      { key: signer.key, fingerprint: expect.any(String), name: "GitHub Actions", created_at: expect.any(Number), expires_at: expires }
    ]);
    // The same key again: a no-op that succeeds, no new ticket needed.
    const again = await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, await registration(pluginId, signer));
    expect(again.status).toBe(201);
  });

  it("is for the plugin's owner alone", async () => {
    const { pluginId } = await owned();
    const stranger = await person();
    const { res } = await addKey(stranger, pluginId);
    expect(res.status).toBe(404);
    expect((await api(stranger, "GET", `/store/v1/plugins/${pluginId}/keys`)).status).toBe(404);
    // A plugin no one owns yet has no keys to manage.
    const unclaimed = newPlugin();
    expect((await addKey(stranger, unclaimed)).res.status).toBe(404);
    // Nor does an id that can't be one.
    expect((await api(stranger, "GET", "/store/v1/plugins/Not_An_Id/keys")).status).toBe(404);
    expect((await api(undefined, "GET", `/store/v1/plugins/${pluginId}/keys`)).status).toBe(401);
  });

  it("needs a fresh sign-in: a bearer alone can't add a key", async () => {
    const { userId, pluginId } = await owned();
    const signer = await newSigner();
    const res = await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, await registration(pluginId, signer));
    expect(res.status).toBe(403);
    expect(await errorOf(res)).toBe("step_up_required");
    const used = await stepUp(userId);
    expect((await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, await registration(pluginId, signer, { stepUp: used }))).status).toBe(201);
    // The ticket is used up.
    const second = await newSigner();
    expect((await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, await registration(pluginId, second, { stepUp: used }))).status).toBe(403);
    // Someone else's ticket is no good either.
    const other = await stepUp(await person());
    expect((await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, await registration(pluginId, second, { stepUp: other }))).status).toBe(403);
  });

  it("takes the web's sign-in as the fresh sign-in, once", async () => {
    const { userId, pluginId } = await owned();
    const browser = await webSignIn(userId);
    const add = async () =>
      siteChange(`/store/v1/plugins/${pluginId}/keys`, browser, {
        headers: { "content-type": "application/json" },
        body: JSON.stringify(await registration(pluginId, await newSigner()))
      });
    expect((await add()).status).toBe(201);
    const second = await add();
    expect(second.status).toBe(403);
    expect(await errorOf(second)).toBe("step_up_required");
    const session = (await (await SELF.fetch(`${SITE}/auth/web/session`, { headers: { cookie: browser.cookie } })).json()) as { fresh_sign_in: boolean };
    expect(session.fresh_sign_in).toBe(false);
    // And it's a change: the site's origin and token, as always.
    const forged = await SELF.fetch(`${SITE}/store/v1/plugins/${pluginId}/keys`, {
      method: "POST",
      headers: { cookie: browser.cookie, origin: "https://evil.example", "x-csrf-token": browser.csrf },
      body: JSON.stringify(await registration(pluginId, await newSigner()))
    });
    expect(forged.status).toBe(403);
  });

  it("needs the key's own proof, naming this plugin", async () => {
    const { userId, pluginId } = await owned();
    const signer = await newSigner();
    const other = await newSigner();
    const ticket = await stepUp(userId);
    const proofs = [
      await other.sign(deployKeyProofMessage(pluginId, signer.key)),
      await signer.sign(deployKeyProofMessage("another-plugin", signer.key)),
      // A device key's proof is not a deploy key's.
      await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key),
      "AAAA"
    ];
    for (const proof of proofs) {
      const res = await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, { key: signer.key, name: "ci", proof, stepUp: ticket });
      expect(res.status).toBe(400);
      expect(await errorOf(res)).toBe("invalid_proof");
    }
    const bad: [Record<string, unknown>, string][] = [
      [{ key: `${signer.key}=` }, "invalid_key"],
      [{ name: "" }, "invalid_name"],
      [{ name: "x".repeat(65) }, "invalid_name"],
      [{ name: "a\nb" }, "invalid_name"],
      [{ expires: Date.now() - 1 }, "invalid_expiry"],
      [{ expires: Date.now() + 6 * 365 * 86_400_000 }, "invalid_expiry"],
      [{ expires: "soon" }, "invalid_expiry"]
    ];
    for (const [change, error] of bad) {
      const res = await api(userId, "POST", `/store/v1/plugins/${pluginId}/keys`, { ...(await registration(pluginId, signer, { stepUp: ticket })), ...change });
      expect(res.status, JSON.stringify(change)).toBe(400);
      expect(await errorOf(res)).toBe(error);
    }
  });

  it("never shares a key with a device or another plugin", async () => {
    const { userId, pluginId } = await owned();
    const device = await newSigner();
    const deviceRes = await api(userId, "POST", "/plugin-keys", { key: device.key, proof: await device.sign(PLUGIN_KEY_PROOF_PREFIX + device.key), stepUp: await stepUp(userId) });
    expect(deviceRes.status).toBe(201);
    const asDeploy = await addKey(userId, pluginId, device);
    expect(asDeploy.res.status).toBe(409);
    expect(await errorOf(asDeploy.res)).toBe("key_taken");
    const { signer } = await addKey(userId, pluginId);
    const asDevice = await api(userId, "POST", "/plugin-keys", { key: signer.key, proof: await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key), stepUp: await stepUp(userId) });
    expect(asDevice.status).toBe(409);
    const otherPlugin = newPlugin();
    await setOwner(otherPlugin, userId);
    expect((await addKey(userId, otherPlugin, signer)).res.status).toBe(409);
    // A device's list never shows deploy keys, nor revokes them.
    const keys = ((await (await api(userId, "GET", "/plugin-keys")).json()) as { keys: { key: string }[] }).keys.map((k) => k.key);
    expect(keys).toEqual([device.key]);
    expect((await api(userId, "DELETE", `/plugin-keys/${signer.key}`)).status).toBe(404);
  });

  it(`keeps to ${MAX_LIVE_DEPLOY_KEYS_PER_PLUGIN} live keys and ${DEPLOY_KEYS_PER_PLUGIN_PER_DAY} new ones a plugin a day, ${DEPLOY_KEYS_PER_USER_PER_DAY} a user`, async () => {
    const { userId, pluginId } = await owned();
    for (let i = 0; i < DEPLOY_KEYS_PER_PLUGIN_PER_DAY; i++) expect((await addKey(userId, pluginId)).res.status).toBe(201);
    const today = await addKey(userId, pluginId);
    expect(today.res.status).toBe(429);
    // A day later the day's count is clear, but the plugin is full.
    await sql("UPDATE plugin_keys SET created_at = created_at - 86400001 WHERE plugin_id = ?", pluginId);
    const full = await addKey(userId, pluginId);
    expect(full.res.status).toBe(409);
    expect(await errorOf(full.res)).toBe("too_many_keys");
    // Revoking one makes room.
    const { keys } = (await (await api(userId, "GET", `/store/v1/plugins/${pluginId}/keys`)).json()) as { keys: { key: string }[] };
    expect((await api(userId, "DELETE", `/store/v1/plugins/${pluginId}/keys/${keys[0]!.key}`)).status).toBe(200);
    expect((await addKey(userId, pluginId)).res.status).toBe(201);
    // The user's own daily count, across plugins.
    const busy = await person();
    let made = 0;
    while (made < DEPLOY_KEYS_PER_USER_PER_DAY) {
      const plugin = newPlugin();
      await setOwner(plugin, busy);
      for (let i = 0; i < 5 && made < DEPLOY_KEYS_PER_USER_PER_DAY; i++, made++) expect((await addKey(busy, plugin)).res.status).toBe(201);
    }
    const plugin = newPlugin();
    await setOwner(plugin, busy);
    expect((await addKey(busy, plugin)).res.status).toBe(429);
  });

  it("is revoked by its owner, once, and only theirs", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    const stranger = await person();
    expect((await api(stranger, "DELETE", `/store/v1/plugins/${pluginId}/keys/${signer.key}`)).status).toBe(404);
    expect((await api(userId, "DELETE", `/store/v1/plugins/${pluginId}/keys/${signer.key}`)).status).toBe(200);
    const listed = async () => ((await (await api(userId, "GET", `/store/v1/plugins/${pluginId}/keys`)).json()) as { keys: { revoked_at: number }[] }).keys[0]!.revoked_at;
    const first = await listed();
    expect(first).toEqual(expect.any(Number));
    expect((await api(userId, "DELETE", `/store/v1/plugins/${pluginId}/keys/${signer.key}`)).status).toBe(200);
    expect(await listed()).toBe(first);
    expect((await api(userId, "DELETE", `/store/v1/plugins/${pluginId}/keys/AAAA`)).status).toBe(404);
    // Revoked for good: it can't be registered again.
    expect((await addKey(userId, pluginId, signer)).res.status).toBe(409);
    expect((await api(userId, "PUT", `/store/v1/plugins/${pluginId}/keys`)).status).toBe(405);
  });

  it("is never managed by a deploy key or a runner token", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    const path = `/store/v1/plugins/${pluginId}/keys`;
    const res = await signed(signer, path, "", { method: "GET" });
    expect(res.status).toBe(403);
    expect(await errorOf(res)).toBe("key_not_allowed");
    expect((await signed(signer, path, JSON.stringify(await registration(pluginId, await newSigner())))).status).toBe(403);
    expect((await api(`runner:${userId}@org1:cloud-1`, "GET", path)).status).toBe(403);
    expect((await api(`runner:${userId}@org1:cloud-1`, "DELETE", `${path}/${signer.key}`)).status).toBe(403);
  });
});

describe("deploy keys: who signed it", () => {
  it("answers the key's plugin, and vouches only for that plugin", async () => {
    const { userId, pluginId } = await owned(`dk${newId("u").slice(5, 12).toLowerCase()}`);
    const { signer } = await addKey(userId, pluginId);
    const viewer = await person();
    const lookup = async (query = "") => (await api(viewer, "GET", `/plugin-keys/${signer.key}${query}`)).json() as Promise<Record<string, unknown>>;
    expect(await lookup()).toMatchObject({ key: signer.key, kind: "deploy", plugin: pluginId, revoked: false, expired: false, verified: false, mine: false });
    expect(await lookup(`?plugin=${pluginId}`)).toMatchObject({ verified: true, username: expect.stringMatching(/^dk/) });
    expect(await lookup("?plugin=another")).toMatchObject({ verified: false });
    // Expired, then revoked: still whose it is, never verified.
    await sql("UPDATE plugin_keys SET expires_at = ? WHERE key = ?", Date.now() - 1, signer.key);
    expect(await lookup(`?plugin=${pluginId}`)).toMatchObject({ verified: false, expired: true, revoked: false });
    await sql("UPDATE plugin_keys SET expires_at = NULL WHERE key = ?", signer.key);
    expect((await api(userId, "DELETE", `/store/v1/plugins/${pluginId}/keys/${signer.key}`)).status).toBe(200);
    expect(await lookup(`?plugin=${pluginId}`)).toMatchObject({ verified: false, revoked: true, revokedAt: expect.any(Number) });
    // A runner may look it up, as any key.
    expect((await api(`runner:${viewer}@org1:cloud-1`, "GET", `/plugin-keys/${signer.key}`)).status).toBe(200);
  });
});

describe("deploy keys: a plugin changing hands", () => {
  it("vouches for nothing of its former owner's, and is its new owner's to see and revoke", async () => {
    const { userId: former, pluginId } = await owned(`dkf${newId("u").slice(5, 11).toLowerCase()}`);
    const { signer } = await addKey(former, pluginId);
    const viewer = await person();
    const lookup = async () => (await api(viewer, "GET", `/plugin-keys/${signer.key}?plugin=${pluginId}`)).json() as Promise<{ verified: boolean }>;
    expect((await lookup()).verified).toBe(true);
    // Transferred, before (or without) the store revoking its keys.
    const next = await person();
    await setOwner(pluginId, next);
    expect((await lookup()).verified).toBe(false);
    expect((await api(former, "GET", `/store/v1/plugins/${pluginId}/keys`)).status).toBe(404);
    const listed = ((await (await api(next, "GET", `/store/v1/plugins/${pluginId}/keys`)).json()) as { keys: { key: string }[] }).keys;
    expect(listed.map((k) => k.key)).toEqual([signer.key]);
    expect((await api(next, "DELETE", `/store/v1/plugins/${pluginId}/keys/${signer.key}`)).status).toBe(200);
    expect(await errorOf(await signed(signer, `/store/v1/plugins/${pluginId}/versions`, "pkg"))).toBe("key_revoked");
  });
});

describe("deploy keys: signed requests", () => {
  const publish = (pluginId: string) => `/store/v1/plugins/${pluginId}/versions`;

  it("publish as the key's owner, scoped to its plugin, with the body intact", async () => {
    const { userId, pluginId } = await owned("dkpublisher");
    const { signer } = await addKey(userId, pluginId);
    const body = "the .zeron-plugin bytes";
    const res = await signed(signer, publish(pluginId), body);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      publisher: { userId, username: "dkpublisher", via: "deploy_key", key: signer.key, pluginScope: pluginId },
      body: await sha256Hex(new TextEncoder().encode(body))
    });
    const listed = ((await (await api(userId, "GET", `/store/v1/plugins/${pluginId}/keys`)).json()) as { keys: { last_used_at: number }[] }).keys[0]!;
    expect(Math.abs(Date.now() - listed.last_used_at)).toBeLessThan(60_000);
  });

  it("read their plugin's state, signing an empty body", async () => {
    const { userId, pluginId } = await owned("dkreader");
    const { signer } = await addKey(userId, pluginId);
    const res = await signed(signer, "/store/v1/me/plugins", "", { method: "GET" });
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({
      publisher: { userId, username: "dkreader", via: "deploy_key", key: signer.key, pluginScope: pluginId },
      body: await sha256Hex(new Uint8Array(0))
    });
  });

  it("are accepted once (replay)", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    const ts = Date.now();
    expect((await signed(signer, publish(pluginId), "pkg", { ts })).status).toBe(200);
    const replay = await signed(signer, publish(pluginId), "pkg", { ts });
    expect(replay.status).toBe(401);
    expect(await errorOf(replay)).toBe("replayed_request");
    // A read replays no better.
    expect((await signed(signer, "/store/v1/me/plugins", "", { ts, method: "GET" })).status).toBe(200);
    expect(await errorOf(await signed(signer, "/store/v1/me/plugins", "", { ts, method: "GET" }))).toBe("replayed_request");
    // The same upload signed afresh is a new request.
    expect((await signed(signer, publish(pluginId), "pkg", { ts: ts + 1 })).status).toBe(200);
  });

  it("refuse a key used on another plugin, even the same owner's", async () => {
    const { userId, pluginId } = await owned();
    const other = newPlugin();
    await setOwner(other, userId);
    const { signer } = await addKey(userId, pluginId);
    for (const path of [publish(other)]) {
      const res = await signed(signer, path, "pkg");
      expect(res.status, path).toBe(403);
      expect(await errorOf(res)).toBe("key_not_allowed");
    }
  });

  it("refuse anything but publishing and reading: no withdraw, yank or unyank", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    for (const [method, path] of [
      ["POST", `${publish(pluginId)}/1.0.0/withdraw`],
      ["POST", `${publish(pluginId)}/1.0.0/yank`],
      ["POST", `${publish(pluginId)}/1.0.0/unyank`],
      ["POST", `/store/v1/admin/submissions/${pluginId}/1.0.0/approve`],
      ["GET", publish(pluginId)]
    ]) {
      const res = await signed(signer, path!, "", { method });
      expect(res.status, path).toBe(403);
    }
    // The device and session API never takes one.
    expect((await signed(signer, "/plugin-keys", "", { method: "GET" })).status).toBe(401);
  });

  it("are refused before a byte of the body is read", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    // A body that never ends: reading it would never answer.
    const endless = () =>
      new ReadableStream<Uint8Array>({
        pull: (controller) => new Promise((resolve) => setTimeout(() => {
          controller.enqueue(new Uint8Array(1024));
          resolve();
        }, 50))
      });
    const forged = await signed(signer, publish(pluginId), "pkg", {
      header: `ZeronKey key=${signer.key}, ts=${Date.now()}, sig=${"A".repeat(86)}`,
      sendBody: endless()
    });
    expect(forged.status).toBe(401);
    expect(await errorOf(forged)).toBe("invalid_signature");
    const unknown = await signed(await newSigner(), publish(pluginId), "pkg", { sendBody: endless() });
    expect(unknown.status).toBe(401);
    const stale = await signed(signer, publish(pluginId), "pkg", { ts: Date.now() - MAX_REQUEST_SKEW_MS - 5000, sendBody: endless() });
    expect(stale.status).toBe(401);
  });

  it("refuse a stale or future time", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    for (const ts of [Date.now() - MAX_REQUEST_SKEW_MS - 5000, Date.now() + MAX_REQUEST_SKEW_MS + 5000]) {
      const res = await signed(signer, publish(pluginId), "pkg", { ts });
      expect(res.status).toBe(401);
      expect(await errorOf(res)).toBe("stale_request");
    }
    expect((await signed(signer, publish(pluginId), "pkg", { ts: Date.now() - MAX_REQUEST_SKEW_MS + 5000 })).status).toBe(200);
  });

  it("refuse a changed body, path or signer, and a query string", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    // The body swapped under a good signature: the route never gets it,
    // buffered or streamed.
    for (const stream of ["0", "1"]) {
      const swapped = await signed(signer, publish(pluginId), "pkg", { ts: Date.now() + Number(stream), sendBody: "evil pkg", headers: { "x-test-stream": stream } });
      expect(swapped.status).toBe(200);
      expect(((await swapped.json()) as { body: string }).body, `stream=${stream}`).toBe("body_mismatch");
    }
    const streamedOk = await signed(signer, publish(pluginId), "pkg", { headers: { "x-test-stream": "1" } });
    expect(((await streamedOk.json()) as { body: string }).body).toBe(await sha256Hex(new TextEncoder().encode("pkg")));
    // The hash sent isn't the one signed, or isn't sent at all.
    const rehashed = await signed(signer, publish(pluginId), "pkg", { sendBody: "evil pkg", sendHash: await sha256Hex(new TextEncoder().encode("evil pkg")) });
    expect(rehashed.status).toBe(401);
    expect(await errorOf(rehashed)).toBe("invalid_signature");
    for (const sendHash of [null, "ABC", "0".repeat(63)]) {
      const res = await signed(signer, publish(pluginId), "pkg", { sendHash });
      expect(res.status, String(sendHash)).toBe(401);
      expect(await errorOf(res)).toBe("unauthenticated");
    }
    const otherPlugin = newPlugin();
    await setOwner(otherPlugin, userId);
    const moved = await signed(signer, publish(otherPlugin), "pkg", { sendPath: publish(pluginId) });
    expect(await errorOf(moved)).toBe("invalid_signature");
    const impostor = await newSigner();
    const ts = String(Date.now());
    const sig = await impostor.sign(publishSigningString("POST", publish(pluginId), await sha256Hex(new TextEncoder().encode("pkg")), ts));
    const forged = await signed(signer, publish(pluginId), "pkg", { header: `ZeronKey key=${signer.key}, ts=${ts}, sig=${sig}` });
    expect(await errorOf(forged)).toBe("invalid_signature");
    const query = await signed(signer, publish(pluginId), "pkg", { sendPath: `${publish(pluginId)}?force=1` });
    expect(query.status).toBe(401);
    const malformed = await signed(signer, publish(pluginId), "pkg", { header: `ZeronKey key=${signer.key}` });
    expect(malformed.status).toBe(401);
  });

  it("refuse an unknown, revoked or expired key, a key whose plugin changed hands, and a deleted owner's", async () => {
    const { userId, pluginId } = await owned();
    const unknown = await signed(await newSigner(), publish(pluginId), "pkg");
    expect(unknown.status).toBe(401);
    expect(await errorOf(unknown)).toBe("unauthenticated");

    const revoked = (await addKey(userId, pluginId)).signer;
    expect((await api(userId, "DELETE", `/store/v1/plugins/${pluginId}/keys/${revoked.key}`)).status).toBe(200);
    expect(await errorOf(await signed(revoked, publish(pluginId), "pkg"))).toBe("key_revoked");

    const expiring = (await addKey(userId, pluginId, undefined, { expires: Date.now() + 60_000 })).signer;
    expect((await signed(expiring, publish(pluginId), "pkg")).status).toBe(200);
    await sql("UPDATE plugin_keys SET expires_at = ? WHERE key = ?", Date.now() - 1, expiring.key);
    expect(await errorOf(await signed(expiring, publish(pluginId), "pkg 2"))).toBe("key_expired");

    // The plugin changed hands: the store revokes its keys.
    const handed = (await addKey(userId, pluginId)).signer;
    expect(await directory().revokeDeployKeys(pluginId)).toBeGreaterThanOrEqual(1);
    expect(await errorOf(await signed(handed, publish(pluginId), "pkg"))).toBe("key_revoked");

    const gone = await owned();
    const theirs = (await addKey(gone.userId, gone.pluginId)).signer;
    const stamp = new Date().toISOString();
    await directory().remove({ id: gone.userId, email: "", first_name: null, last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp }, stamp);
    expect((await signed(theirs, publish(gone.pluginId), "pkg")).status).toBe(401);
  });
});

describe("signingKeyAllowed", () => {
  const allowed = (publisher: Publisher, id: string, key: string) => signingKeyAllowed(env as unknown as Env, publisher, id, key);

  it("with a session: the caller's live device key, or their live deploy key for that plugin", async () => {
    const { userId, pluginId } = await owned();
    const session: Publisher = { userId, username: null, via: "session" };
    const device = await newSigner();
    expect((await api(userId, "POST", "/plugin-keys", { key: device.key, proof: await device.sign(PLUGIN_KEY_PROOF_PREFIX + device.key), stepUp: await stepUp(userId) })).status).toBe(201);
    const { signer: deploy } = await addKey(userId, pluginId);
    expect(await allowed(session, pluginId, device.key)).toBe(true);
    expect(await allowed(session, "any-plugin", device.key)).toBe(true);
    expect(await allowed(session, pluginId, deploy.key)).toBe(true);
    expect(await allowed(session, "another-plugin", deploy.key)).toBe(false);
    // Someone else's key, or none at all.
    const stranger = await person();
    expect(await allowed({ userId: stranger, username: null, via: "session" }, pluginId, device.key)).toBe(false);
    expect(await allowed(session, pluginId, (await newSigner()).key)).toBe(false);
    // Revoked or expired keys sign nothing.
    expect((await api(userId, "DELETE", `/plugin-keys/${device.key}`)).status).toBe(200);
    expect(await allowed(session, pluginId, device.key)).toBe(false);
    await sql("UPDATE plugin_keys SET expires_at = ? WHERE key = ?", Date.now() - 1, deploy.key);
    expect(await allowed(session, pluginId, deploy.key)).toBe(false);
  });

  it("with a deploy key: that very key, for its plugin", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, pluginId);
    const { signer: second } = await addKey(userId, pluginId);
    const publisher: Publisher = { userId, username: null, via: "deploy_key", key: signer.key, pluginScope: pluginId };
    expect(await allowed(publisher, pluginId, signer.key)).toBe(true);
    // Not the plugin's other deploy key, nor the owner's device key, nor another plugin.
    expect(await allowed(publisher, pluginId, second.key)).toBe(false);
    const device = await newSigner();
    expect((await api(userId, "POST", "/plugin-keys", { key: device.key, proof: await device.sign(PLUGIN_KEY_PROOF_PREFIX + device.key), stepUp: await stepUp(userId) })).status).toBe(201);
    expect(await allowed(publisher, pluginId, device.key)).toBe(false);
    expect(await allowed(publisher, "another-plugin", signer.key)).toBe(false);
  });
});
