/**
 * The store and the edge's identity, end to end: what only exists once the
 * two meet. A real web sign-in made staff by an operator verifies a
 * version; a real account deploy key (amendment 44), registered with a
 * step-up and its proof, publishes with a signed request and is held to its
 * scope; a plugin that leaves the account leaves every key's scope.
 */
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import vectors from "../../../docs/plugin-package-vectors.json";
import { b64urlEncode } from "../../src/cloud/crypto";
import { deployKeyProofMessage, publishSigningString, scopeText, sha256Hex } from "../../src/store/identity/signing";
import { publisherId } from "../../src/store/cards";
import { SITE, grantAdmin, person, sessionHeader, setCookies, stepUp, webSignIn } from "./identity-helpers";
import { type Signer, adminCall, author, call, makePackage, newSigner, publish, registerKey, uniqueId, uniqueName } from "./store-helpers";

const json = async <T = Record<string, unknown>>(res: Response) => (await res.json()) as T;

/** An ed25519 key pair that signs bytes (packages) and text (requests, proofs). */
const deployKey = async () => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  const sign = async (message: Uint8Array) => b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, message)));
  const signer: Signer = { key, sign };
  return { key, signer, signText: (text: string) => sign(new TextEncoder().encode(text)) };
};

/** A request signed as the contract says (the identity lane's
 * publishSigningString), its body's hash in X-Zeron-Content-SHA256. */
const signed = async (k: Awaited<ReturnType<typeof deployKey>>, method: string, path: string, body: Uint8Array = new Uint8Array(0)) => {
  const ts = String(Date.now());
  const hash = await sha256Hex(body);
  const sig = await k.signText(publishSigningString(method, path, hash, ts));
  return SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: { authorization: `ZeronKey key=${k.key}, ts=${ts}, sig=${sig}`, "x-zeron-content-sha256": hash },
    body: method === "GET" ? undefined : body
  });
};

describe("the store with the edge's identity", () => {
  it("lets a web session, made staff by an operator, verify a version with its Origin and CSRF token", async () => {
    const a = await author();
    const id = uniqueId();
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const staffId = await person();
    // Signed in, but not staff: refused.
    let browser = await webSignIn(staffId);
    expect((await SELF.fetch(`${SITE}/store/v1/admin/queue`, { headers: { cookie: browser.cookie } })).status).toBe(403);
    await grantAdmin(staffId);
    browser = await webSignIn(staffId);
    let cookie = browser.cookie;
    const keep = (res: Response) => {
      const rotated = setCookies(res).get("__Host-zeron_session");
      if (rotated) cookie = sessionHeader(rotated);
      return res;
    };
    const queue = keep(await SELF.fetch(`${SITE}/store/v1/admin/queue`, { headers: { cookie } }));
    expect(queue.status).toBe(200);
    expect((await json<{ items: { plugin: { id: string } }[] }>(queue)).items.map((i) => i.plugin.id)).toContain(id);
    const approve = `${SITE}/store/v1/admin/versions/${id}/1.0.0/verify`;
    // A change without the site's Origin, or without the session's token: refused.
    expect(keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, "x-csrf-token": browser.csrf } })).status).toBe(403);
    expect(keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, origin: SITE } })).status).toBe(403);
    expect(keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, origin: "https://evil.example", "x-csrf-token": browser.csrf } })).status).toBe(403);
    expect(await json(await call(undefined, "GET", `/store/v1/plugins/${id}`))).toMatchObject({ verified: false });
    const ok = keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, origin: SITE, "x-csrf-token": browser.csrf } }));
    expect(ok.status).toBe(200);
    expect((await json<{ version: { status: string; verified: boolean } }>(ok)).version).toMatchObject({ status: "listed", verified: true });
    expect(await json(await call(undefined, "GET", `/store/v1/plugins/${id}`))).toMatchObject({ id, version: "1.0.0", verified: true, verified_version: "1.0.0" });
    // The audit names the staff account.
    const audit = await json<{ entries: { action: string; actor: { user_id: string } }[] }>(await adminCall("GET", `/store/v1/admin/audit?plugin=${id}`));
    expect(audit.entries[0]).toMatchObject({ action: "version.verify", actor: { user_id: staffId } });
  });

  it("signs a request exactly as the engine's deploy-key vector does", () => {
    const r = vectors.deploy_key.request as { method: string; path: string; body_sha256: string; ts: number | string; message: string };
    expect(publishSigningString(r.method, r.path, r.body_sha256, String(r.ts))).toBe(r.message);
  });

  it("publishes with a real deploy key, holds it to its scope, never claims an id, and stops at a transfer", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId("ci");
    const notInScope = uniqueId("ci");
    // A plugin's first version comes from the author's device (amendment 44).
    for (const p of [id, notInScope]) expect((await publish(a.userId, p, await makePackage({ id: p, version: "1.0.0", signer: a.signer }))).status).toBe(201);

    // Made through the account's key routes, with a fresh step-up and a proof
    // binding the account, the expiry and the scope.
    const k = await deployKey();
    const scope = { plugins: [id] };
    const expires = Date.now() + 86_400_000;
    const proof = await k.signText(deployKeyProofMessage(await publisherId(a.userId), expires, scopeText(scope)!, k.key));
    const body = { key: k.key, name: "Nightly build", scope, expires, proof };
    const noStepUp = await call(a.userId, "POST", "/store/v1/me/keys", body);
    expect(noStepUp.status).toBe(403);
    // A proof for another expiry, scope or account isn't this key's.
    for (const wrong of [{ ...body, expires: expires + 1 }, { ...body, scope: { all: true } }]) {
      const res = await call(a.userId, "POST", "/store/v1/me/keys", { ...wrong, stepUp: await stepUp(a.userId) });
      expect([res.status, ((await res.json()) as { error: string }).error]).toEqual([400, "invalid_proof"]);
    }
    const registered = await call(a.userId, "POST", "/store/v1/me/keys", { ...body, stepUp: await stepUp(a.userId) });
    expect(registered.status, await registered.clone().text()).toBe(201);
    expect(await registered.json()).toMatchObject({ key: k.key, name: "Nightly build", scope, expires_at: expires, last_used_at: null, last_used_plugin: null });
    // Nobody can scope a key to a plugin that isn't theirs.
    const theirs = await deployKey();
    const strangerScope = { plugins: [id] };
    const stranger = await call(b.userId, "POST", "/store/v1/me/keys", {
      key: theirs.key, name: "CI", scope: strangerScope,
      proof: await theirs.signText(deployKeyProofMessage(await publisherId(b.userId), null, scopeText(strangerScope)!, theirs.key)),
      stepUp: await stepUp(b.userId)
    });
    expect([stranger.status, await stranger.json()]).toEqual([403, expect.objectContaining({ error: "not_owner", ids: [id] })]);

    // A version the key signed, uploaded with a request the key signed.
    const pkg = await makePackage({ id, version: "1.1.0", signer: k.signer });
    const published = await signed(k, "POST", `/store/v1/plugins/${id}/versions`, pkg);
    expect(published.status, await published.clone().text()).toBe(201);
    expect(await published.json()).toMatchObject({ status: "listed", verified: false });
    const row = await env.STORE_DB.prepare("SELECT via, signer_key, publisher_id, deploy_key_name FROM versions WHERE plugin_id = ? AND version = '1.1.0'").bind(id).first();
    expect(row).toEqual({ via: "deploy_key", signer_key: k.key, publisher_id: a.userId, deploy_key_name: "Nightly build" });
    // The author's devices see which key published which version (the notice).
    const own = (await json<{ plugins: { id: string; versions: Record<string, unknown>[] }[] }>(await call(a.userId, "GET", "/store/v1/me/plugins"))).plugins.find((p) => p.id === id)!;
    expect(own.versions[0]).toMatchObject({ version: "1.1.0", via: "deploy_key", key: k.key, deploy_key: { key: k.key, name: "Nightly build" } });
    expect(own.versions[1]).toMatchObject({ version: "1.0.0", via: "device", deploy_key: null });
    const keys = (await json<{ keys: Record<string, unknown>[] }>(await call(a.userId, "GET", "/store/v1/me/keys"))).keys;
    expect(keys.find((x) => x.key === k.key)).toMatchObject({ last_used_at: expect.any(Number), last_used_plugin: id });
    // The same bytes again, signed anew: the same answer, as any retry.
    expect((await signed(k, "POST", `/store/v1/plugins/${id}/versions`, pkg)).status).toBe(201);
    // A body other than the one signed: refused before it's read, as a body
    // changed on the way (amendment 36), not as a bad signature.
    const ts = String(Date.now());
    const other = await makePackage({ id, version: "1.2.0", signer: k.signer });
    const sig = await k.signText(publishSigningString("POST", `/store/v1/plugins/${id}/versions`, await sha256Hex(pkg), ts));
    const mismatch = await SELF.fetch(`https://edge.test/store/v1/plugins/${id}/versions`, {
      method: "POST",
      headers: { authorization: `ZeronKey key=${k.key}, ts=${ts}, sig=${sig}`, "x-zeron-content-sha256": await sha256Hex(pkg) },
      body: other
    });
    expect([mismatch.status, ((await mismatch.json()) as { error: string }).error]).toEqual([400, "body_mismatch"]);
    // A signature that doesn't verify stays 401 invalid_signature.
    const forged = await SELF.fetch(`https://edge.test/store/v1/plugins/${id}/versions`, {
      method: "POST",
      headers: { authorization: `ZeronKey key=${k.key}, ts=${ts}, sig=${"A".repeat(86)}`, "x-zeron-content-sha256": await sha256Hex(pkg) },
      body: pkg
    });
    expect([forged.status, ((await forged.json()) as { error: string }).error]).toEqual([401, "invalid_signature"]);

    // Its account's plugins in its scope, and nothing else.
    const mine = await signed(k, "GET", "/store/v1/me/plugins");
    expect(mine.status).toBe(200);
    const read = await json<{ publisher_id: string; plugins: { id: string }[] }>(mine);
    expect(read.publisher_id).toBe(await publisherId(a.userId));
    expect(read.plugins.map((p) => p.id)).toEqual([id]);
    const fresh = uniqueId();
    for (const [method, path, what] of [
      ["POST", `/store/v1/plugins/${id}/versions/1.0.0/yank`, "a yank"],
      ["POST", `/store/v1/plugins/${notInScope}/versions`, "a plugin of theirs outside its scope"],
      ["POST", `/store/v1/plugins/${fresh}/versions`, "a new id"],
      ["GET", "/store/v1/admin/queue", "an admin route"],
      ["GET", "/store/v1/me/keys", "the keys"]
    ] as const) {
      const res = await signed(k, method, path, method === "POST" && path.endsWith("/versions") ? await makePackage({ id: path.split("/")[4]!, version: "2.0.0", signer: k.signer }) : undefined);
      expect(res.status, what).toBe(403);
    }
    expect(await env.STORE_DB.prepare("SELECT 1 FROM plugins WHERE id = ?").bind(fresh).first()).toBeNull();

    // As devices see it: verified for a plugin in its scope only.
    const lookup = async (plugin?: string) =>
      json<{ verified: boolean; kind?: string; scope?: unknown }>(await call(b.userId, "GET", `/plugin-keys/${k.key}${plugin ? `?plugin=${plugin}` : ""}`));
    expect(await lookup(id)).toMatchObject({ verified: true, kind: "deploy" });
    // (Its scope is its owner's to read.)
    expect(await lookup(id)).not.toHaveProperty("scope");
    expect((await json(await call(a.userId, "GET", `/plugin-keys/${k.key}?plugin=${id}`))).scope).toEqual(scope);
    expect((await lookup()).verified).toBe(false);
    expect((await lookup(notInScope)).verified).toBe(false);

    // A transfer: the plugin leaves the account and every key's scope, for good.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username })).status).toBe(200);
    const after = await signed(k, "POST", `/store/v1/plugins/${id}/versions`, await makePackage({ id, version: "1.3.0", signer: k.signer }));
    expect([after.status, ((await after.json()) as { error: string }).error]).toEqual([403, "key_not_allowed"]);
    expect((await lookup(id)).verified).toBe(false);
    expect((await json<{ keys: { key: string; scope: unknown }[] }>(await call(a.userId, "GET", "/store/v1/me/keys"))).keys.find((x) => x.key === k.key)!.scope).toEqual({ plugins: [] });
    // Coming back doesn't restore it.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: a.username })).status).toBe(200);
    const back = await signed(k, "POST", `/store/v1/plugins/${id}/versions`, await makePackage({ id, version: "1.3.0", signer: k.signer }));
    expect(back.status).toBe(403);
    // Revoked: nothing.
    expect(await json(await call(a.userId, "DELETE", `/store/v1/me/keys/${k.key}`))).toEqual({ revoked: true });
    expect((await json<{ keys: Record<string, unknown>[] }>(await call(a.userId, "GET", "/store/v1/me/keys"))).keys.find((x) => x.key === k.key)).toMatchObject({ revoked_at: expect.any(Number) });
    expect((await signed(k, "GET", "/store/v1/me/plugins")).status).toBe(401);
  });

  it("an `all` key publishes what its account owns and claims later, never a plugin transferred in", async () => {
    const a = await author();
    const b = await author();
    const before = uniqueId("ci");
    expect((await publish(a.userId, before, await makePackage({ id: before, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const k = await deployKey();
    const proof = await k.signText(deployKeyProofMessage(await publisherId(a.userId), null, "*", k.key));
    const made = await call(a.userId, "POST", "/store/v1/me/keys", { key: k.key, name: "All", scope: { all: true }, proof, stepUp: await stepUp(a.userId) });
    expect(made.status, await made.clone().text()).toBe(201);
    expect(await made.json()).toMatchObject({ scope: { all: true }, expires_at: null });
    // Owned when the key was made.
    expect((await signed(k, "POST", `/store/v1/plugins/${before}/versions`, await makePackage({ id: before, version: "1.1.0", signer: k.signer }))).status).toBe(201);
    // Claimed by the account later (from its device), then published by the key.
    const later = uniqueId("ci");
    expect((await publish(a.userId, later, await makePackage({ id: later, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await signed(k, "POST", `/store/v1/plugins/${later}/versions`, await makePackage({ id: later, version: "1.1.0", signer: k.signer }))).status).toBe(201);
    // Transferred in from someone else: never in an `all` scope.
    const theirs = uniqueId("ci");
    expect((await publish(b.userId, theirs, await makePackage({ id: theirs, version: "1.0.0", signer: b.signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${theirs}/transfer`, { to_username: a.username })).status).toBe(200);
    const moved = await signed(k, "POST", `/store/v1/plugins/${theirs}/versions`, await makePackage({ id: theirs, version: "1.1.0", signer: k.signer }));
    expect([moved.status, ((await moved.json()) as { error: string }).error]).toEqual([403, "key_not_allowed"]);
    // The key's /me/plugins: what it may publish.
    const mine = (await json<{ plugins: { id: string }[] }>(await signed(k, "GET", "/store/v1/me/plugins"))).plugins.map((p) => p.id).sort();
    expect(mine).toEqual([before, later].sort());
  });

  it("refuses an admin's own bearer on every admin route, and takes their web session", async () => {
    const staffId = await person();
    await grantAdmin(staffId);
    const routes: [string, string][] = [
      ["GET", "/store/v1/admin/queue"],
      ["GET", "/store/v1/admin/plugins/nope-not-here"],
      ["POST", "/store/v1/admin/plugins/nope-not-here/feature"],
      ["GET", "/store/v1/admin/reports"],
      ["GET", "/store/v1/admin/audit"],
      ["GET", "/store/v1/admin/reservations"]
    ];
    for (const [method, path] of routes) {
      expect((await call(staffId, method, path, method === "POST" ? { featured: true } : undefined)).status, `${method} ${path} by bearer`).toBe(403);
    }
    const browser = await webSignIn(staffId);
    expect((await SELF.fetch(`${SITE}/store/v1/admin/queue`, { headers: { cookie: browser.cookie } })).status).toBe(200);
  });

  it("rotates a fresh staff session's token on its first use of a real store route, and sends the cookie on a 404 too", async () => {
    const staffId = await person();
    await grantAdmin(staffId);
    for (const [path, status] of [
      ["/store/v1/admin/queue", 200],
      ["/store/v1/admin/plugins/nope-not-here", 404]
    ] as const) {
      const browser = await webSignIn(staffId);
      const res = await SELF.fetch(`${SITE}${path}`, { headers: { cookie: browser.cookie } });
      expect(res.status, path).toBe(status);
      const rotated = setCookies(res).get("__Host-zeron_session");
      expect(rotated, path).toMatch(/^[A-Za-z0-9_-]{43}$/);
      expect(rotated).not.toBe(browser.token);
      // The new token is the session from now on.
      expect((await SELF.fetch(`${SITE}/store/v1/admin/queue`, { headers: { cookie: sessionHeader(rotated!) } })).status).toBe(200);
    }
  });

  it("lets an owner read a taken-down version's package and files with a web session, privately, leaving nothing cached", async () => {
    const userId = await person(uniqueName());
    const signer = await newSigner();
    await registerKey(userId, signer);
    const id = uniqueId();
    expect((await publish(userId, id, await makePackage({ id, version: "1.0.0", signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/versions/1.0.0/take-down`, { reason: "Harmful." })).status).toBe(200);
    const browser = await webSignIn(userId);
    for (const path of [`/store/v1/plugins/${id}/versions/1.0.0/package`, `/store/v1/plugins/${id}/files/1.0.0/init.luau`, `/store/v1/plugins/${id}/files/1.0.0/icon.svg`]) {
      const own = await SELF.fetch(`${SITE}${path}`, { headers: { cookie: browser.cookie } });
      expect(own.status, path).toBe(200);
      expect(own.headers.get("cache-control"), path).toBe("private, no-store");
      await own.arrayBuffer();
      // Nobody else gets it, from the cache or otherwise, twice over.
      for (let i = 0; i < 2; i++) expect((await call(undefined, "GET", path)).status, `${path} anonymous`).toBe(404);
    }
  });

  it("takes an admin's verify through the site's service binding", async () => {
    const a = await author();
    const id = uniqueId();
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const staffId = await person();
    await grantAdmin(staffId);
    const browser = await webSignIn(staffId);
    const approve = await env.SITE.fetch(`${SITE}/store/v1/admin/versions/${id}/1.0.0/verify`, {
      method: "POST",
      redirect: "manual",
      headers: { cookie: browser.cookie, origin: SITE, "x-csrf-token": browser.csrf }
    });
    expect(approve.status, await approve.clone().text()).toBe(200);
    expect((await json<{ version: { verified: boolean } }>(approve)).version.verified).toBe(true);
    // The rotated cookie comes back through the binding for the browser.
    expect(setCookies(approve).get("__Host-zeron_session")).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(200);
  });
});
