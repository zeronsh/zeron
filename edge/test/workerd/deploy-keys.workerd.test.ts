/**
 * Deploy keys belong to an account and carry a scope (amendment 44):
 * registering one (`/store/v1/me/keys`, a step-up, the proof v2), what it
 * vouches for, a plugin leaving the account, the signed requests it makes,
 * `signingKeyAllowed`, and the operator's revocations.
 */
import { SELF, env, runInDurableObject } from "cloudflare:test";
import type { Env } from "../../src/env";
import { signingKeyAllowed, type Publisher, type ScopedPlugin } from "../../src/store/principal";
import { describe, expect, it } from "vitest";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import { publisherId } from "../../src/store/cards";
import { MAX_REQUEST_SKEW_MS, deployKeyProofMessage, publishSigningString, scopeText, sha256Hex } from "../../src/store/identity/signing";
import { DEPLOY_KEYS_PER_DAY, MAX_DEPLOY_KEY_PLUGINS, MAX_LIVE_DEPLOY_KEYS, type UserDirectory } from "../../src/user-directory";
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

type Scope = { all: true } | { plugins: string[] };

const newPlugin = () => `p${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;
const KEYS = "/store/v1/me/keys";

const api = (bearer: string | undefined, method: string, path: string, body?: unknown) =>
  SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: {
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" })
    },
    body: body === undefined ? undefined : JSON.stringify(body)
  });

/** An account owning `n` fresh plugins, claimed a minute ago (before any
 * key a test then makes), as the store records a first publish. */
const owned = async (opts: { n?: number; username?: string } = {}) => {
  const userId = await person(opts.username);
  const plugins = Array.from({ length: opts.n ?? 1 }, newPlugin);
  for (const plugin of plugins) await setOwner(plugin, userId, Date.now() - 60_000);
  return { userId, pluginId: plugins[0]!, plugins };
};

/** A registration body for `scope`, the key's proof v2 over `userId`'s
 * publisher id, the expiry and the scope. */
const registration = async (userId: string, signer: Signer, scope: unknown, extra: Record<string, unknown> = {}) => {
  const expires = typeof extra.expires === "number" ? extra.expires : null;
  const text = scopeText(scope as Scope) ?? "";
  return { key: signer.key, name: "GitHub Actions", scope, proof: await signer.sign(deployKeyProofMessage(await publisherId(userId), expires, text, signer.key)), ...extra };
};

/** Register a deploy key with a bearer and a fresh step-up. */
const addKey = async (userId: string, scope: Scope, signer?: Signer, extra: Record<string, unknown> = {}) => {
  const key = signer ?? (await newSigner());
  const res = await api(userId, "POST", KEYS, await registration(userId, key, scope, { stepUp: await stepUp(userId), ...extra }));
  return { res, signer: key };
};

const keysOf = async (userId: string) => ((await (await api(userId, "GET", KEYS)).json()) as { keys: Record<string, unknown>[] }).keys;

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
      // To the fixture's principal probes: these tests are about who the
      // signed request is (the store's real routes are tested on their own).
      "x-test-principal": "1",
      ...opts.headers
    },
    body: method === "GET" ? undefined : (opts.sendBody ?? body)
  });
};

const sql = (query: string, ...args: unknown[]) =>
  runInDurableObject(directory() as unknown as DurableObjectStub<UserDirectory>, (_i, state) => {
    state.storage.sql.exec(query, ...args);
  });

/** How `viewer` sees deploy key `key` vouching for `plugin`. */
const lookup = async (viewer: string, key: string, plugin?: string) =>
  (await api(viewer, "GET", `/plugin-keys/${key}${plugin === undefined ? "" : `?plugin=${plugin}`}`)).json() as Promise<Record<string, unknown>>;

describe("deploy keys: registering", () => {
  it("binds a key to the account with its scope, listed without any secret", async () => {
    const { userId, plugins } = await owned({ n: 2 });
    const expires = Date.now() + 30 * 86_400_000;
    // Sent in any order; kept, and proved, sorted.
    const sent = { plugins: [...plugins].sort().reverse() };
    const scope = { plugins: [...plugins].sort() };
    const { res, signer } = await addKey(userId, sent, undefined, { expires });
    expect(res.status, await res.clone().text()).toBe(201);
    expect(res.headers.get("cache-control")).toBe("no-store");
    const made = {
      key: signer.key,
      fingerprint: expect.stringMatching(/^[0-9a-f]{4}( [0-9a-f]{4}){7}$/),
      name: "GitHub Actions",
      scope,
      created_at: expect.any(Number),
      last_used_at: null,
      last_used_plugin: null,
      expires_at: expires
    };
    expect(await res.json()).toEqual(made);
    expect(await keysOf(userId)).toEqual([made]);
    // The same key again: a no-op that succeeds, no new ticket needed.
    const again = await api(userId, "POST", KEYS, await registration(userId, signer, sent, { expires }));
    expect(again.status).toBe(201);
    // The same key with another scope is another key: refused.
    const wider = await api(userId, "POST", KEYS, await registration(userId, signer, { all: true }, { expires, stepUp: await stepUp(userId) }));
    expect([wider.status, await errorOf(wider)]).toEqual([409, "key_taken"]);
    // A key for all the account's plugins, that never expires.
    const all = await addKey(userId, { all: true });
    expect(all.res.status).toBe(201);
    expect(await all.res.json()).toMatchObject({ scope: { all: true }, expires_at: null });
    expect((await keysOf(userId)).map((k) => k.key)).toEqual([signer.key, all.signer.key]);
  });

  it("holds a key's scope to plugins its account owns now, each named once", async () => {
    const { userId, pluginId } = await owned();
    const theirs = await owned();
    const unclaimed = newPlugin();
    const notMine = await addKey(userId, { plugins: [pluginId, theirs.pluginId, unclaimed] });
    expect(notMine.res.status).toBe(403);
    expect(await notMine.res.json()).toMatchObject({ error: "not_owner", ids: [theirs.pluginId, unclaimed].sort() });
    const signer = await newSigner();
    const ticket = await stepUp(userId);
    const bad: [unknown, string[] | undefined][] = [
      [undefined, undefined],
      [{}, undefined],
      [{ all: false }, undefined],
      [{ all: true, plugins: [pluginId] }, undefined],
      [[pluginId], undefined],
      [{ plugins: [] }, undefined],
      [{ plugins: Array.from({ length: MAX_DEPLOY_KEY_PLUGINS + 1 }, newPlugin) }, undefined],
      [{ plugins: [pluginId, "Not_An_Id", 7] }, ["Not_An_Id", "7"]],
      // A repeat is refused, never dropped: the proof names the scope as sent.
      [{ plugins: [pluginId, unclaimed, pluginId] }, [pluginId]]
    ];
    for (const [scope, ids] of bad) {
      const res = await api(userId, "POST", KEYS, { key: signer.key, name: "ci", scope, proof: "AAAA", stepUp: ticket });
      const body = (await res.json()) as { error: string; ids?: string[] };
      expect([res.status, body.error], JSON.stringify(scope)).toEqual([400, "invalid_scope"]);
      expect(body.ids, JSON.stringify(scope)).toEqual(ids);
    }
    // (The ticket was never used: a good registration takes it.)
    expect((await api(userId, "POST", KEYS, await registration(userId, signer, { plugins: [pluginId] }, { stepUp: ticket }))).status).toBe(201);
    expect((await api(undefined, "GET", KEYS)).status).toBe(401);
    expect((await api(undefined, "POST", KEYS, {})).status).toBe(401);
  });

  it("needs a fresh sign-in: a bearer alone can't add a key", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const signer = await newSigner();
    const res = await api(userId, "POST", KEYS, await registration(userId, signer, scope));
    expect(res.status).toBe(403);
    expect(await errorOf(res)).toBe("step_up_required");
    const used = await stepUp(userId);
    expect((await api(userId, "POST", KEYS, await registration(userId, signer, scope, { stepUp: used }))).status).toBe(201);
    // The ticket is used up.
    const second = await newSigner();
    expect((await api(userId, "POST", KEYS, await registration(userId, second, scope, { stepUp: used }))).status).toBe(403);
    // Someone else's ticket is no good either.
    const other = await stepUp(await person());
    expect((await api(userId, "POST", KEYS, await registration(userId, second, scope, { stepUp: other }))).status).toBe(403);
  });

  it("takes the web's sign-in as the fresh sign-in, once", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const browser = await webSignIn(userId);
    const add = async () =>
      siteChange(KEYS, browser, { headers: { "content-type": "application/json" }, body: JSON.stringify(await registration(userId, await newSigner(), scope)) });
    expect((await add()).status).toBe(201);
    const second = await add();
    expect(second.status).toBe(403);
    expect(await errorOf(second)).toBe("step_up_required");
    const session = (await (await SELF.fetch(`${SITE}/auth/web/session`, { headers: { cookie: browser.cookie } })).json()) as { fresh_sign_in: boolean };
    expect(session.fresh_sign_in).toBe(false);
    // The list, read with the cookie.
    expect((await (await SELF.fetch(`${SITE}${KEYS}`, { headers: { cookie: browser.cookie } })).json()) as { keys: unknown[] }).toMatchObject({ keys: [expect.any(Object)] });
    // And it's a change: the site's origin and token, as always.
    const forged = await SELF.fetch(`${SITE}${KEYS}`, {
      method: "POST",
      headers: { cookie: browser.cookie, origin: "https://evil.example", "x-csrf-token": browser.csrf },
      body: JSON.stringify(await registration(userId, await newSigner(), scope))
    });
    expect(forged.status).toBe(403);
  });

  it("needs the key's own proof v2, naming this account, expiry and scope", async () => {
    const { userId, plugins } = await owned({ n: 2 });
    const scope = { plugins: [plugins[0]!] };
    const signer = await newSigner();
    const other = await newSigner();
    const ticket = await stepUp(userId);
    const pub = await publisherId(userId);
    const expires = Date.now() + 86_400_000;
    const text = scopeText(scope)!;
    const proofs = [
      await other.sign(deployKeyProofMessage(pub, expires, text, signer.key)),
      await signer.sign(deployKeyProofMessage(await publisherId(await person()), expires, text, signer.key)),
      await signer.sign(deployKeyProofMessage(pub, expires + 1, text, signer.key)),
      await signer.sign(deployKeyProofMessage(pub, null, text, signer.key)),
      await signer.sign(deployKeyProofMessage(pub, expires, scopeText({ plugins })!, signer.key)),
      await signer.sign(deployKeyProofMessage(pub, expires, "*", signer.key)),
      // The first deploy keys' proof, for one plugin.
      await signer.sign(`zeron-deploy-key:v1:${plugins[0]}:${signer.key}`),
      // A device key's proof is not a deploy key's.
      await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key),
      "AAAA"
    ];
    for (const proof of proofs) {
      const res = await api(userId, "POST", KEYS, { key: signer.key, name: "ci", scope, expires, proof, stepUp: ticket });
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
      const res = await api(userId, "POST", KEYS, { ...(await registration(userId, signer, scope, { stepUp: ticket })), ...change });
      expect(res.status, JSON.stringify(change)).toBe(400);
      expect(await errorOf(res)).toBe(error);
    }
    expect((await api(userId, "POST", KEYS, await registration(userId, signer, scope, { expires, stepUp: ticket }))).status).toBe(201);
  });

  it("never shares a key with a device or another account", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const device = await newSigner();
    const deviceRes = await api(userId, "POST", "/plugin-keys", { key: device.key, proof: await device.sign(PLUGIN_KEY_PROOF_PREFIX + device.key), stepUp: await stepUp(userId) });
    expect(deviceRes.status).toBe(201);
    const asDeploy = await addKey(userId, scope, device);
    expect(asDeploy.res.status).toBe(409);
    expect(await errorOf(asDeploy.res)).toBe("key_taken");
    const { signer } = await addKey(userId, scope);
    const asDevice = await api(userId, "POST", "/plugin-keys", { key: signer.key, proof: await signer.sign(PLUGIN_KEY_PROOF_PREFIX + signer.key), stepUp: await stepUp(userId) });
    expect(asDevice.status).toBe(409);
    const stranger = await owned();
    expect((await addKey(stranger.userId, { plugins: [stranger.pluginId] }, signer)).res.status).toBe(409);
    // A device's list never shows deploy keys, nor revokes them; nor do the
    // key routes revoke a device's.
    const keys = ((await (await api(userId, "GET", "/plugin-keys")).json()) as { keys: { key: string }[] }).keys.map((k) => k.key);
    expect(keys).toEqual([device.key]);
    expect((await api(userId, "DELETE", `/plugin-keys/${signer.key}`)).status).toBe(404);
    expect((await api(userId, "DELETE", `${KEYS}/${device.key}`)).status).toBe(404);
  });

  it(`keeps an account to ${MAX_LIVE_DEPLOY_KEYS} live keys and ${DEPLOY_KEYS_PER_DAY} new ones a day`, async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const aDayPasses = () => sql("UPDATE plugin_keys SET created_at = created_at - 86400001 WHERE user_id = ? AND kind = 'deploy'", userId);
    for (let i = 0; i < DEPLOY_KEYS_PER_DAY; i++) expect((await addKey(userId, scope)).res.status).toBe(201);
    const today = await addKey(userId, scope);
    expect([today.res.status, await errorOf(today.res)]).toEqual([429, "rate_limited"]);
    expect(today.res.headers.get("retry-after")).not.toBeNull();
    await aDayPasses();
    for (let i = 0; i < MAX_LIVE_DEPLOY_KEYS - DEPLOY_KEYS_PER_DAY; i++) expect((await addKey(userId, scope)).res.status).toBe(201);
    await aDayPasses();
    // A new day, but the account is full.
    const full = await addKey(userId, scope);
    expect([full.res.status, await errorOf(full.res)]).toEqual([409, "too_many_keys"]);
    // An expired key isn't live, nor a revoked one: each makes room.
    const keys = await keysOf(userId);
    await sql("UPDATE plugin_keys SET expires_at = ? WHERE key = ?", Date.now() - 1, keys[0]!.key);
    expect((await addKey(userId, scope)).res.status).toBe(201);
    expect((await addKey(userId, scope)).res.status).toBe(409);
    expect((await api(userId, "DELETE", `${KEYS}/${keys[1]!.key}`)).status).toBe(200);
    expect((await addKey(userId, scope)).res.status).toBe(201);
    // Another account counts its own.
    const other = await owned();
    expect((await addKey(other.userId, { plugins: [other.pluginId] })).res.status).toBe(201);
  });

  it("is revoked by its owner, once, and only theirs", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const { signer } = await addKey(userId, scope);
    const stranger = await person();
    expect((await api(stranger, "DELETE", `${KEYS}/${signer.key}`)).status).toBe(404);
    const revoked = await api(userId, "DELETE", `${KEYS}/${signer.key}`);
    expect([revoked.status, await revoked.json()]).toEqual([200, { revoked: true }]);
    const revokedAt = async () => (await keysOf(userId))[0]!.revoked_at;
    const first = await revokedAt();
    expect(first).toEqual(expect.any(Number));
    expect((await api(userId, "DELETE", `${KEYS}/${signer.key}`)).status).toBe(200);
    expect(await revokedAt()).toBe(first);
    expect((await api(userId, "DELETE", `${KEYS}/AAAA`)).status).toBe(404);
    // Revoked for good: it can't be registered again.
    const again = await addKey(userId, scope, signer);
    expect([again.res.status, await errorOf(again.res)]).toEqual([409, "key_revoked"]);
    for (const [method, path] of [["PUT", KEYS], ["DELETE", KEYS], ["GET", `${KEYS}/${signer.key}`], ["POST", `${KEYS}/${signer.key}`]] as const) {
      expect((await api(userId, method, path)).status, `${method} ${path}`).toBe(405);
    }
  });

  it("is never managed by a deploy key or a runner token", async () => {
    const { userId, pluginId } = await owned();
    const { signer } = await addKey(userId, { plugins: [pluginId] });
    const res = await signed(signer, KEYS, "", { method: "GET" });
    expect(res.status).toBe(403);
    expect(await errorOf(res)).toBe("key_not_allowed");
    expect((await signed(signer, KEYS, JSON.stringify(await registration(userId, await newSigner(), { all: true })))).status).toBe(403);
    expect((await signed(signer, `${KEYS}/${signer.key}`, "", { method: "DELETE" })).status).toBe(403);
    expect((await api(`runner:${userId}@org1:cloud-1`, "GET", KEYS)).status).toBe(403);
    expect((await api(`runner:${userId}@org1:cloud-1`, "DELETE", `${KEYS}/${signer.key}`)).status).toBe(403);
  });
});

describe("deploy keys: who signed it", () => {
  it("vouches only for a plugin its scope holds that its account owns, and shows its scope to its owner only", async () => {
    const { userId, plugins } = await owned({ n: 2, username: `dk${newId("u").slice(5, 12).toLowerCase()}` });
    const [inScope, outOfScope] = plugins as [string, string];
    const { signer } = await addKey(userId, { plugins: [inScope] });
    const viewer = await person();
    const seen = await lookup(viewer, signer.key);
    expect(seen).toMatchObject({ key: signer.key, kind: "deploy", revoked: false, expired: false, verified: false, mine: false });
    expect(seen).not.toHaveProperty("scope");
    expect(await lookup(viewer, signer.key, inScope)).toMatchObject({ verified: true, username: expect.stringMatching(/^dk/) });
    // Its account's, but not in its scope.
    expect(await lookup(viewer, signer.key, outOfScope)).toMatchObject({ verified: false });
    expect(await lookup(viewer, signer.key, "another")).toMatchObject({ verified: false });
    expect(await lookup(userId, signer.key, inScope)).toMatchObject({ mine: true, verified: true, scope: { plugins: [inScope] } });
    // Expired, then revoked: still whose it is, never verified.
    await sql("UPDATE plugin_keys SET expires_at = ? WHERE key = ?", Date.now() - 1, signer.key);
    expect(await lookup(viewer, signer.key, inScope)).toMatchObject({ verified: false, expired: true, revoked: false });
    await sql("UPDATE plugin_keys SET expires_at = NULL WHERE key = ?", signer.key);
    expect((await api(userId, "DELETE", `${KEYS}/${signer.key}`)).status).toBe(200);
    expect(await lookup(viewer, signer.key, inScope)).toMatchObject({ verified: false, revoked: true, revokedAt: expect.any(Number) });
    // A runner may look it up, as any key.
    expect((await api(`runner:${viewer}@org1:cloud-1`, "GET", `/plugin-keys/${signer.key}`)).status).toBe(200);
  });

  it("an `all` key vouches for what its account owned when it was made or claims later, never a plugin moved in", async () => {
    const { userId, pluginId: before } = await owned({ username: `dka${newId("u").slice(5, 11).toLowerCase()}` });
    const { signer } = await addKey(userId, { all: true });
    const later = newPlugin();
    await setOwner(later, userId, Date.now() + 1000);
    const theirs = await owned();
    await setOwner(theirs.pluginId, userId, Date.now() + 1000);
    const viewer = await person();
    expect((await lookup(viewer, signer.key, before)).verified).toBe(true);
    expect((await lookup(viewer, signer.key, later)).verified).toBe(true);
    expect((await lookup(viewer, signer.key, theirs.pluginId)).verified).toBe(false);
    expect((await lookup(viewer, signer.key, newPlugin())).verified).toBe(false);
  });
});

describe("deploy keys: a plugin leaving the account", () => {
  it("leaves every key's scope for good: moved away, moved back, or released and claimed afresh", async () => {
    const { userId: former, plugins } = await owned({ n: 2, username: `dkf${newId("u").slice(5, 11).toLowerCase()}` });
    const [pluginId, kept] = plugins as [string, string];
    const { signer: listed } = await addKey(former, { plugins: [pluginId, kept] });
    const { signer: all } = await addKey(former, { all: true });
    const viewer = await person();
    const vouches = async (key: string, plugin = pluginId) => (await lookup(viewer, key, plugin)).verified;
    expect([await vouches(listed.key), await vouches(all.key)]).toEqual([true, true]);
    // Transferred, before (or without) the store dropping it from the keys.
    const next = await person();
    await setOwner(pluginId, next, Date.now() + 1000);
    expect([await vouches(listed.key), await vouches(all.key), await vouches(listed.key, kept)]).toEqual([false, false, true]);
    // The keys stay their account's: its to see and revoke, never the new owner's.
    expect((await keysOf(former)).map((k) => k.key)).toEqual([listed.key, all.key]);
    expect(await keysOf(next)).toEqual([]);
    expect((await api(next, "DELETE", `${KEYS}/${listed.key}`)).status).toBe(404);
    // Back again: still not.
    await setOwner(pluginId, former, Date.now() + 2000);
    expect([await vouches(listed.key), await vouches(all.key)]).toEqual([false, false]);
    // Released and claimed afresh by the account itself: never in a list made
    // before; in `all`, as anything the account claims later.
    await setOwner(pluginId, null);
    await setOwner(pluginId, former, Date.now() + 3000);
    expect([await vouches(listed.key), await vouches(all.key)]).toEqual([false, true]);
    // The store's transfer drops it from the lists too, for the owner to see.
    expect(await directory().dropFromDeployKeys(former, pluginId)).toBe(1);
    expect((await keysOf(former)).find((k) => k.key === listed.key)!.scope).toEqual({ plugins: [kept] });
  });
});

describe("deploy keys: signed requests", () => {
  const publish = (pluginId: string) => `/store/v1/plugins/${pluginId}/versions`;
  const keyed = async (opts: { username?: string } = {}) => {
    const { userId, pluginId } = await owned(opts);
    const { signer } = await addKey(userId, { plugins: [pluginId] });
    return { userId, pluginId, signer };
  };

  it("publish as the key's account, with its name and scope, the body intact", async () => {
    const { userId, pluginId, signer } = await keyed({ username: "dkpublisher" });
    const body = "the .zeron-plugin bytes";
    const res = await signed(signer, publish(pluginId), body);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      publisher: { userId, username: "dkpublisher", via: "deploy_key", key: signer.key, keyName: "GitHub Actions", scope: { plugins: [pluginId] }, keyCreatedAt: expect.any(Number) },
      body: await sha256Hex(new TextEncoder().encode(body))
    });
    const listed = (await keysOf(userId))[0]!;
    expect(Math.abs(Date.now() - (listed.last_used_at as number))).toBeLessThan(60_000);
    expect(listed.last_used_plugin).toBe(pluginId);
  });

  it("read their account's plugins, signing an empty body", async () => {
    const { userId, pluginId, signer } = await keyed({ username: "dkreader" });
    const res = await signed(signer, "/store/v1/me/plugins", "", { method: "GET" });
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({
      publisher: { userId, username: "dkreader", via: "deploy_key", key: signer.key, scope: { plugins: [pluginId] } },
      body: await sha256Hex(new Uint8Array(0))
    });
    // A read is a use, of no plugin.
    expect(await keysOf(userId)).toMatchObject([{ last_used_at: expect.any(Number), last_used_plugin: null }]);
  });

  it("are accepted once (replay)", async () => {
    const { pluginId, signer } = await keyed();
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

  it("refuse a plugin outside a listed scope, even the same account's", async () => {
    const { userId, pluginId, signer } = await keyed();
    const other = newPlugin();
    await setOwner(other, userId, Date.now() - 60_000);
    for (const path of [publish(other), publish(newPlugin())]) {
      const res = await signed(signer, path, "pkg");
      expect(res.status, path).toBe(403);
      expect(await errorOf(res)).toBe("key_not_allowed");
    }
    expect((await signed(signer, publish(pluginId), "pkg")).status).toBe(200);
  });

  it("refuse anything but publishing and reading: no yank, unyank, review or keys", async () => {
    const { pluginId, signer } = await keyed();
    for (const [method, path] of [
      ["POST", `${publish(pluginId)}/1.0.0/yank`],
      ["POST", `${publish(pluginId)}/1.0.0/unyank`],
      ["POST", `/store/v1/admin/versions/${pluginId}/1.0.0/verify`],
      ["GET", publish(pluginId)],
      ["GET", KEYS]
    ]) {
      const res = await signed(signer, path!, "", { method });
      expect(res.status, path).toBe(403);
    }
    // The device and session API never takes one.
    expect((await signed(signer, "/plugin-keys", "", { method: "GET" })).status).toBe(401);
  });

  it("are refused before a byte of the body is read", async () => {
    const { pluginId, signer } = await keyed();
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
    const outOfScope = await signed(signer, publish(newPlugin()), "pkg", { sendBody: endless() });
    expect(outOfScope.status).toBe(403);
  });

  it("refuse a stale or future time", async () => {
    const { pluginId, signer } = await keyed();
    for (const ts of [Date.now() - MAX_REQUEST_SKEW_MS - 5000, Date.now() + MAX_REQUEST_SKEW_MS + 5000]) {
      const res = await signed(signer, publish(pluginId), "pkg", { ts });
      expect(res.status).toBe(401);
      expect(await errorOf(res)).toBe("stale_request");
    }
    expect((await signed(signer, publish(pluginId), "pkg", { ts: Date.now() - MAX_REQUEST_SKEW_MS + 5000 })).status).toBe(200);
  });

  it("refuse a changed body, path or signer, and a query string", async () => {
    const { userId, pluginId } = await owned({ n: 1 });
    const otherPlugin = newPlugin();
    await setOwner(otherPlugin, userId, Date.now() - 60_000);
    const { signer } = await addKey(userId, { plugins: [pluginId, otherPlugin] });
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
    // Signed for one plugin in its scope, sent to another.
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

  it("refuse an unknown, revoked or expired key, a plugin dropped from its scope, and a deleted owner's", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const unknown = await signed(await newSigner(), publish(pluginId), "pkg");
    expect(unknown.status).toBe(401);
    expect(await errorOf(unknown)).toBe("unauthenticated");

    const revoked = (await addKey(userId, scope)).signer;
    expect((await api(userId, "DELETE", `${KEYS}/${revoked.key}`)).status).toBe(200);
    expect(await errorOf(await signed(revoked, publish(pluginId), "pkg"))).toBe("key_revoked");

    const expiring = (await addKey(userId, scope, undefined, { expires: Date.now() + 60_000 })).signer;
    expect((await signed(expiring, publish(pluginId), "pkg")).status).toBe(200);
    await sql("UPDATE plugin_keys SET expires_at = ? WHERE key = ?", Date.now() - 1, expiring.key);
    expect(await errorOf(await signed(expiring, publish(pluginId), "pkg 2"))).toBe("key_expired");

    // The plugin left the account: the store drops it from its keys' lists.
    const dropped = (await addKey(userId, scope)).signer;
    expect(await directory().dropFromDeployKeys(userId, pluginId)).toBeGreaterThanOrEqual(1);
    expect(await errorOf(await signed(dropped, publish(pluginId), "pkg"))).toBe("key_not_allowed");

    const gone = await owned();
    const theirs = (await addKey(gone.userId, { all: true })).signer;
    const stamp = new Date().toISOString();
    await directory().remove({ id: gone.userId, email: "", first_name: null, last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp }, stamp);
    expect((await signed(theirs, publish(gone.pluginId), "pkg")).status).toBe(401);
  });
});

describe("signingKeyAllowed", () => {
  const row = (id: string) =>
    env.STORE_DB!.prepare("SELECT id, owner_id, owner_since, created_at FROM plugins WHERE id = ?").bind(id).first<ScopedPlugin>();
  const allowed = async (publisher: Publisher, id: string, key: string) => signingKeyAllowed(env as unknown as Env, publisher, id, key, await row(id));

  it("with a session: the caller's live device key, or their live deploy key for a plugin its scope holds", async () => {
    const { userId, plugins } = await owned({ n: 2 });
    const [pluginId, other] = plugins as [string, string];
    const session: Publisher = { userId, username: null, via: "session" };
    const device = await newSigner();
    expect((await api(userId, "POST", "/plugin-keys", { key: device.key, proof: await device.sign(PLUGIN_KEY_PROOF_PREFIX + device.key), stepUp: await stepUp(userId) })).status).toBe(201);
    const { signer: deploy } = await addKey(userId, { plugins: [pluginId] });
    const { signer: all } = await addKey(userId, { all: true });
    expect(await allowed(session, pluginId, device.key)).toBe(true);
    // A device key signs a new id; a deploy key never claims one.
    expect(await allowed(session, newPlugin(), device.key)).toBe(true);
    expect(await allowed(session, pluginId, deploy.key)).toBe(true);
    expect(await allowed(session, other, deploy.key)).toBe(false);
    expect(await allowed(session, other, all.key)).toBe(true);
    expect(await allowed(session, newPlugin(), deploy.key)).toBe(false);
    expect(await allowed(session, newPlugin(), all.key)).toBe(false);
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

  it("with a deploy key: that very key, for a plugin its scope holds", async () => {
    const { userId, plugins } = await owned({ n: 2 });
    const [pluginId, other] = plugins as [string, string];
    const scope = { plugins: [pluginId] };
    const { signer } = await addKey(userId, scope);
    const { signer: second } = await addKey(userId, scope);
    const made = (await keysOf(userId))[0]!.created_at as number;
    const publisher: Publisher = { userId, username: null, via: "deploy_key", key: signer.key, keyName: "GitHub Actions", scope, keyCreatedAt: made };
    expect(await allowed(publisher, pluginId, signer.key)).toBe(true);
    // Not the account's other deploy key, nor its device key, nor another plugin.
    expect(await allowed(publisher, pluginId, second.key)).toBe(false);
    const device = await newSigner();
    expect((await api(userId, "POST", "/plugin-keys", { key: device.key, proof: await device.sign(PLUGIN_KEY_PROOF_PREFIX + device.key), stepUp: await stepUp(userId) })).status).toBe(201);
    expect(await allowed(publisher, pluginId, device.key)).toBe(false);
    expect(await allowed(publisher, other, signer.key)).toBe(false);
    // Nor once the plugin has moved away.
    await setOwner(pluginId, (await person()), Date.now() + 1000);
    expect(await allowed(publisher, pluginId, signer.key)).toBe(false);
  });
});

// Last in this file: it revokes every deploy key there is.
describe("deploy keys: the operator's routes", () => {
  const operator = (method: string, path: string, body?: unknown, token = "test-admin-token") =>
    SELF.fetch(`https://edge.test${path}`, {
      method,
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body)
    });

  it("revoke the keys that may publish one plugin, or every key (before a rollback), with the operator token only", async () => {
    const a = await owned({ n: 2 });
    const b = await owned();
    const [leaked, other] = a.plugins as [string, string];
    const { signer: aKey } = await addKey(a.userId, { plugins: [leaked] });
    const { signer: aAll } = await addKey(a.userId, { all: true });
    const { signer: aOther } = await addKey(a.userId, { plugins: [other] });
    const { signer: bKey } = await addKey(b.userId, { plugins: [b.pluginId] });
    expect((await operator("GET", "/admin/deploy-keys", undefined, "wrong")).status).toBe(401);
    expect((await operator("POST", "/admin/deploy-keys/revoke", { all: true }, a.userId)).status).toBe(401);
    for (const body of [{}, { pluginId: "Bad Id" }, { all: "yes" }, { all: true, pluginId: leaked }]) {
      expect((await operator("POST", "/admin/deploy-keys/revoke", body)).status, JSON.stringify(body)).toBe(400);
    }
    expect(((await (await operator("GET", "/admin/deploy-keys")).json()) as { live: number }).live).toBeGreaterThanOrEqual(4);
    // An id no one owns has no keys.
    expect(await (await operator("POST", "/admin/deploy-keys/revoke", { pluginId: newPlugin() })).json()).toEqual({ revoked: 0 });
    // Its owner's keys whose scope may hold it: the one listing it, and `all`.
    expect(await (await operator("POST", "/admin/deploy-keys/revoke", { pluginId: leaked })).json()).toEqual({ revoked: 2 });
    expect(await errorOf(await signed(aKey, `/store/v1/plugins/${leaked}/versions`, "pkg"))).toBe("key_revoked");
    expect(await errorOf(await signed(aAll, `/store/v1/plugins/${other}/versions`, "pkg"))).toBe("key_revoked");
    expect((await signed(aOther, `/store/v1/plugins/${other}/versions`, "pkg")).status).toBe(200);
    expect((await signed(bKey, `/store/v1/plugins/${b.pluginId}/versions`, "pkg")).status).toBe(200);
    const all = (await (await operator("POST", "/admin/deploy-keys/revoke", { all: true })).json()) as { revoked: number };
    expect(all.revoked).toBeGreaterThanOrEqual(2);
    expect(await errorOf(await signed(bKey, `/store/v1/plugins/${b.pluginId}/versions`, "pkg 2"))).toBe("key_revoked");
    expect(await (await operator("GET", "/admin/deploy-keys")).json()).toEqual({ live: 0 });
    expect((await operator("DELETE", "/admin/deploy-keys")).status).toBe(405);
  });
});
