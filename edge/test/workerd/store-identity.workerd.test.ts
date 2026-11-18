/**
 * The store and the edge's identity, end to end: what only exists once the
 * two meet. A real web sign-in made staff by an operator reviews a
 * submission; a real deploy key, registered with a step-up and its proof,
 * publishes with a signed request and is held to what a key may do; a
 * transfer revokes it.
 */
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import vectors from "../../../docs/plugin-package-vectors.json";
import { b64urlEncode } from "../../src/cloud/crypto";
import { deployKeyProofMessage, publishSigningString, sha256Hex } from "../../src/store/identity/signing";
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
  it("lets a web session, made staff by an operator, approve a submission with its Origin and CSRF token; then it lists", async () => {
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
    const approve = `${SITE}/store/v1/admin/submissions/${id}/1.0.0/approve`;
    // A change without the site's Origin, or without the session's token: refused.
    expect(keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, "x-csrf-token": browser.csrf } })).status).toBe(403);
    expect(keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, origin: SITE } })).status).toBe(403);
    expect(keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, origin: "https://evil.example", "x-csrf-token": browser.csrf } })).status).toBe(403);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(404);
    const ok = keep(await SELF.fetch(approve, { method: "POST", headers: { cookie, origin: SITE, "x-csrf-token": browser.csrf } }));
    expect(ok.status).toBe(200);
    expect((await json<{ version: { status: string } }>(ok)).version.status).toBe("approved");
    const listed = await json<{ id: string; version: string }>(await call(undefined, "GET", `/store/v1/plugins/${id}`));
    expect(listed).toMatchObject({ id, version: "1.0.0" });
    // The audit names the staff account.
    const audit = await json<{ entries: { action: string; actor: { user_id: string } }[] }>(await adminCall("GET", `/store/v1/admin/audit?plugin=${id}`));
    expect(audit.entries[0]).toMatchObject({ action: "version.approve", actor: { user_id: staffId } });
  });

  it("signs a request exactly as the engine's deploy-key vector does", () => {
    const r = vectors.deploy_key.request as { method: string; path: string; body_sha256: string; ts: number | string; message: string };
    expect(publishSigningString(r.method, r.path, r.body_sha256, String(r.ts))).toBe(r.message);
  });

  it("publishes with a real deploy key, holds it to its plugin, and stops it at a transfer", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId("ci");
    // The id is claimed from the author's device first (amendment 17).
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/submissions/${id}/1.0.0/approve`)).status).toBe(200);

    // Registered through the identity routes, with a fresh step-up and its proof.
    const k = await deployKey();
    const proof = await k.signText(deployKeyProofMessage(id, k.key));
    const noStepUp = await call(a.userId, "POST", `/store/v1/plugins/${id}/keys`, { key: k.key, name: "CI", proof });
    expect(noStepUp.status).toBe(403);
    const registered = await call(a.userId, "POST", `/store/v1/plugins/${id}/keys`, { key: k.key, name: "CI", proof, stepUp: await stepUp(a.userId) });
    expect(registered.status, await registered.clone().text()).toBe(201);
    // Someone else can't register a key for it.
    const theirs = await deployKey();
    const stranger = await call(b.userId, "POST", `/store/v1/plugins/${id}/keys`, {
      key: theirs.key,
      name: "CI",
      proof: await theirs.signText(deployKeyProofMessage(id, theirs.key)),
      stepUp: await stepUp(b.userId)
    });
    expect([403, 404]).toContain(stranger.status);

    // A version the key signed, uploaded with a request the key signed.
    const pkg = await makePackage({ id, version: "1.1.0", signer: k.signer });
    const published = await signed(k, "POST", `/store/v1/plugins/${id}/versions`, pkg);
    expect(published.status, await published.clone().text()).toBe(201);
    const row = await env.STORE_DB.prepare("SELECT via, signer_key, publisher_id FROM versions WHERE plugin_id = ? AND version = '1.1.0'").bind(id).first();
    expect(row).toEqual({ via: "deploy_key", signer_key: k.key, publisher_id: a.userId });
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

    // Its plugin's state, for that plugin only.
    const mine = await signed(k, "GET", "/store/v1/me/plugins");
    expect(mine.status).toBe(200);
    expect((await json<{ plugins: { id: string }[] }>(mine)).plugins.map((p) => p.id)).toEqual([id]);
    // Nothing else: a yank, another plugin, an admin route.
    for (const [method, path] of [
      ["POST", `/store/v1/plugins/${id}/versions/1.0.0/yank`],
      ["POST", `/store/v1/plugins/${uniqueId()}/versions`],
      ["GET", "/store/v1/admin/queue"]
    ] as const) {
      const res = await signed(k, method, path);
      expect(res.status, `${method} ${path}`).toBe(403);
    }

    // As devices see it: verified for its plugin only.
    const lookup = async (plugin?: string) =>
      json<{ verified: boolean; kind?: string }>(await call(b.userId, "GET", `/plugin-keys/${k.key}${plugin ? `?plugin=${plugin}` : ""}`));
    expect(await lookup(id)).toMatchObject({ verified: true, kind: "deploy" });
    expect((await lookup()).verified).toBe(false);
    expect((await lookup("another-plugin")).verified).toBe(false);

    // A transfer: the key stops, for publishing and for lookups.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username })).status).toBe(200);
    const after = await signed(k, "POST", `/store/v1/plugins/${id}/versions`, await makePackage({ id, version: "1.3.0", signer: k.signer }));
    expect(after.status).toBe(401);
    expect((await lookup(id)).verified).toBe(false);
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

  it("lets an owner read a pending version's package and files with a web session, privately, leaving nothing cached", async () => {
    const userId = await person(uniqueName());
    const signer = await newSigner();
    await registerKey(userId, signer);
    const id = uniqueId();
    expect((await publish(userId, id, await makePackage({ id, version: "1.0.0", signer }))).status).toBe(201);
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

  it("takes an admin's approval through the site's service binding", async () => {
    const a = await author();
    const id = uniqueId();
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const staffId = await person();
    await grantAdmin(staffId);
    const browser = await webSignIn(staffId);
    const approve = await env.SITE.fetch(`${SITE}/store/v1/admin/submissions/${id}/1.0.0/approve`, {
      method: "POST",
      redirect: "manual",
      headers: { cookie: browser.cookie, origin: SITE, "x-csrf-token": browser.csrf }
    });
    expect(approve.status, await approve.clone().text()).toBe(200);
    expect((await json<{ version: { status: string } }>(approve)).version.status).toBe("approved");
    // The rotated cookie comes back through the binding for the browser.
    expect(setCookies(approve).get("__Host-zeron_session")).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect((await call(undefined, "GET", `/store/v1/plugins/${id}`)).status).toBe(200);
  });
});
