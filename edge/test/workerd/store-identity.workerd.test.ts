/**
 * The store and the edge's identity, end to end: what only exists once the
 * two meet. A real web sign-in made staff by an operator verifies a
 * version; a real deploy token (contract a315eef5a), made with a fresh
 * sign-in, publishes from CI as a plain bearer and is held to its scope; a
 * plugin that leaves the account leaves every token's scope.
 */
import { SELF, env } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { SITE, grantAdmin, person, sessionHeader, setCookies, stepUp, webSignIn } from "./identity-helpers";
import { adminCall, author, call, deployToken, makePackage, newSigner, publish, registerKey, tokenPublish, uniqueId, uniqueName } from "./store-helpers";

const json = async <T = Record<string, unknown>>(res: Response) => (await res.json()) as T;

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

  it("publishes with a real deploy token, holds it to its scope, never a first version, and stops at a transfer", async () => {
    const a = await author();
    const b = await author();
    const id = uniqueId("ci");
    const notInScope = uniqueId("ci");
    // A plugin's first version comes from the author's device.
    for (const p of [id, notInScope]) expect((await publish(a.userId, p, await makePackage({ id: p, version: "1.0.0", signer: a.signer }))).status).toBe(201);

    // Made by the signed-in account with a fresh sign-in; the secret, this once.
    const noStepUp = await call(a.userId, "POST", "/store/v1/me/tokens", { name: "Nightly build", scope: { plugins: [id] } });
    expect([noStepUp.status, (await json<{ error: string }>(noStepUp)).error]).toEqual([403, "step_up_required"]);
    const made = await call(a.userId, "POST", "/store/v1/me/tokens", { name: "Nightly build", scope: { plugins: [id] }, stepUp: await stepUp(a.userId) });
    expect(made.status, await made.clone().text()).toBe(201);
    const created = await json<{ token: string; id: string; short: string; name: string; scope: unknown; created_at: number; expires_at: number | null }>(made);
    expect(created).toEqual({ token: expect.stringMatching(/^zdt_[0-9A-Za-z]{49}$/), id: expect.stringMatching(/^tok_[0-9A-Za-z]{16}$/), short: created.token.slice(4, 12), name: "Nightly build", scope: { plugins: [id] }, created_at: expect.any(Number), expires_at: null });
    const { token } = created;
    // Nobody can scope a token to a plugin that isn't theirs.
    const stranger = await call(b.userId, "POST", "/store/v1/me/tokens", { name: "CI", scope: { plugins: [id] }, stepUp: await stepUp(b.userId) });
    expect([stranger.status, await stranger.json()]).toEqual([403, expect.objectContaining({ error: "not_owner", ids: [id] })]);

    // A version from CI, published with the token as a bearer: unsigned, the
    // token its publisher's word. A signed one is refused (a token's package
    // never is), and a device's unsigned one too.
    const signedByToken = await tokenPublish(token, id, await makePackage({ id, version: "1.1.0", signer: await newSigner() }));
    expect([signedByToken.status, await signedByToken.json()]).toEqual([400, expect.objectContaining({ error: "bad_signature", reason: "bad_signature" })]);
    const unsignedFromDevice = await publish(a.userId, id, await makePackage({ id, version: "1.1.0", signer: null }));
    expect([unsignedFromDevice.status, await unsignedFromDevice.json()]).toEqual([400, expect.objectContaining({ error: "bad_signature", reason: "unsigned" })]);
    const published = await tokenPublish(token, id, await makePackage({ id, version: "1.1.0", signer: null }));
    expect(published.status, await published.clone().text()).toBe(201);
    expect(await published.json()).toMatchObject({ status: "listed", verified: false });
    const row = await env.STORE_DB.prepare(
      "SELECT via, publisher_id, deploy_token_name, deploy_token_short, deploy_token_id, signer_key, content_hash, checks FROM versions WHERE plugin_id = ? AND version = '1.1.0'"
    )
      .bind(id)
      .first<Record<string, unknown>>();
    expect(row).toMatchObject({ via: "deploy_token", publisher_id: a.userId, deploy_token_name: "Nightly build", deploy_token_short: created.short, deploy_token_id: created.id, signer_key: null });
    expect(JSON.parse(row!.checks as string)).toContainEqual({ id: "signature", level: "ok", message: "Unsigned: uploaded with a deploy token, which vouches for its publisher." });
    // Every device reads who published it from the store's record: no key,
    // the publisher, and the content hash it checks the package against.
    const card = await json<{ versions: Record<string, unknown>[]; package: Record<string, unknown> }>(await call(undefined, "GET", `/store/v1/plugins/${id}`));
    expect(card.versions[0]).toMatchObject({ version: "1.1.0", via: "deploy_token", key: null, hash: row!.content_hash });
    expect(card.package).toMatchObject({ key: null, hash: row!.content_hash });
    const firstHash = (await env.STORE_DB.prepare("SELECT content_hash FROM versions WHERE plugin_id = ? AND version = '1.0.0'").bind(id).first<{ content_hash: string }>())!.content_hash;
    const offers = await json<{ updates: Record<string, unknown>[] }>(
      await call(undefined, "POST", "/store/v1/updates", { zeron: "0.5.0", plugins: [{ id, version: "1.0.0", hash: firstHash }] })
    );
    expect(offers.updates).toEqual([expect.objectContaining({ id, version: "1.1.0", key: null, publisher: a.username, hash: row!.content_hash })]);
    // The author's devices see which token published which version (the notice).
    const own = (await json<{ plugins: { id: string; versions: Record<string, unknown>[] }[] }>(await call(a.userId, "GET", "/store/v1/me/plugins"))).plugins.find((p) => p.id === id)!;
    expect(own.versions[0]).toMatchObject({ version: "1.1.0", via: "deploy_token", key: null, deploy_token: { id: created.id, short: created.short, name: "Nightly build" } });
    expect(own.versions[1]).toMatchObject({ version: "1.0.0", via: "device", key: a.signer.key, deploy_token: null });
    const listed = await call(a.userId, "GET", "/store/v1/me/tokens");
    const listText = await listed.clone().text();
    expect(listText).not.toContain(token);
    expect((await json<{ tokens: Record<string, unknown>[] }>(listed)).tokens.find((t) => t.id === created.id)).toMatchObject({ short: created.short, last_used_at: expect.any(Number), last_used_plugin: id });
    // The audit names it by its short id, never more.
    const audit = await adminCall("GET", `/store/v1/admin/audit?plugin=${id}`);
    const auditText = await audit.clone().text();
    expect(auditText).not.toContain(token);
    expect((await json<{ entries: { action: string; detail: Record<string, unknown> }[] }>(audit)).entries.find((e) => e.action === "version.publish" && e.detail.via === "deploy_token")!.detail).toMatchObject({ token: created.short });

    // It reads nothing, not even its own plugins (the owner's view is a person's).
    const mine = await call(undefined, "GET", "/store/v1/me/plugins", undefined, { authorization: `Bearer ${token}` });
    expect([mine.status, (await json<{ error: string }>(mine)).error]).toEqual([401, "session_required"]);
    // A live token asking for what it may not: one answer, 403, whether a
    // plugin outside its scope or a first version (CI says which: not this).
    const fresh = uniqueId();
    const notAllowed = [
      await tokenPublish(token, notInScope, await makePackage({ id: notInScope, version: "2.0.0", signer: null })),
      await tokenPublish(token, fresh, await makePackage({ id: fresh, version: "1.0.0", signer: null }))
    ];
    // A token that isn't one: another, 401, whether unknown or mistyped.
    const unknown = `zdt_${"A".repeat(43)}${created.token.slice(-6)}`;
    const badChecksum = `${token.slice(0, -1)}${token.endsWith("A") ? "B" : "A"}`;
    const invalid = [
      await tokenPublish(unknown, id, await makePackage({ id, version: "2.0.0", signer: null })),
      await tokenPublish(badChecksum, id, await makePackage({ id, version: "2.0.0", signer: null }))
    ];
    for (const [responses, status, error] of [[notAllowed, 403, "token_not_allowed"], [invalid, 401, "invalid_token"]] as const) {
      const bodies = await Promise.all(responses.map(async (r) => [r.status, await r.text()] as const));
      for (const [got, text] of bodies) {
        expect(got).toBe(status);
        expect(text).toBe(bodies[0]![1]);
        expect(text).not.toContain(token);
      }
      expect(JSON.parse(bodies[0]![1])).toMatchObject({ error });
    }
    expect(await env.STORE_DB.prepare("SELECT 1 FROM plugins WHERE id = ?").bind(fresh).first()).toBeNull();
    // Anywhere else it isn't a session: refused before it's looked up.
    for (const [method, path] of [["POST", `/store/v1/plugins/${id}/versions/1.0.0/yank`], ["GET", "/store/v1/admin/queue"], ["GET", "/store/v1/me/tokens"]] as const) {
      const res = await call(undefined, method, path, undefined, { authorization: `Bearer ${token}` });
      expect([res.status, (await json<{ error: string }>(res)).error], path).toEqual([401, "session_required"]);
    }

    // A transfer: the plugin leaves the account and every token's scope, for good.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: b.username })).status).toBe(200);
    const after = await tokenPublish(token, id, await makePackage({ id, version: "1.3.0", signer: null }));
    expect([after.status, (await json<{ error: string }>(after)).error]).toEqual([403, "token_not_allowed"]);
    expect((await json<{ tokens: { id: string; scope: unknown }[] }>(await call(a.userId, "GET", "/store/v1/me/tokens"))).tokens.find((t) => t.id === created.id)!.scope).toEqual({ plugins: [] });
    // Coming back doesn't restore it.
    expect((await adminCall("POST", `/store/v1/admin/plugins/${id}/transfer`, { to_username: a.username })).status).toBe(200);
    expect((await tokenPublish(token, id, await makePackage({ id, version: "1.3.0", signer: null }))).status).toBe(403);
    // Revoked: not a token any more.
    expect(await json(await call(a.userId, "DELETE", `/store/v1/me/tokens/${created.id}`))).toEqual({ revoked: true });
    expect((await json<{ tokens: Record<string, unknown>[] }>(await call(a.userId, "GET", "/store/v1/me/tokens"))).tokens.find((t) => t.id === created.id)).toMatchObject({ revoked_at: expect.any(Number) });
    const revoked = await tokenPublish(token, id, await makePackage({ id, version: "1.3.0", signer: null }));
    expect([revoked.status, (await json<{ error: string }>(revoked)).error]).toEqual([401, "invalid_token"]);
  });

  it("an `all` token publishes what its account owns and claims later, never a plugin transferred in", async () => {
    const a = await author();
    const b = await author();
    const before = uniqueId("ci");
    expect((await publish(a.userId, before, await makePackage({ id: before, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const { token } = await deployToken(a.userId, { all: true });
    expect((await tokenPublish(token, before, await makePackage({ id: before, version: "1.1.0", signer: null }))).status).toBe(201);
    // Claimed by the account later (from its device), then published with the token.
    const later = uniqueId("ci");
    expect((await publish(a.userId, later, await makePackage({ id: later, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    expect((await tokenPublish(token, later, await makePackage({ id: later, version: "1.1.0", signer: null }))).status).toBe(201);
    // Transferred in from someone else: never in an `all` scope.
    const theirs = uniqueId("ci");
    expect((await publish(b.userId, theirs, await makePackage({ id: theirs, version: "1.0.0", signer: b.signer }))).status).toBe(201);
    expect((await adminCall("POST", `/store/v1/admin/plugins/${theirs}/transfer`, { to_username: a.username })).status).toBe(200);
    const moved = await tokenPublish(token, theirs, await makePackage({ id: theirs, version: "1.1.0", signer: null }));
    expect([moved.status, (await json<{ error: string }>(moved)).error]).toEqual([403, "token_not_allowed"]);
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
