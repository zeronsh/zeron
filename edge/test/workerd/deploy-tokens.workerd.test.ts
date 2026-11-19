/**
 * Deploy tokens (contract a315eef5a): making one (`/store/v1/me/tokens`, a
 * fresh sign-in from a device or the web), its limits and revocation, who
 * a request carrying one is, what its scope holds, a plugin leaving the
 * account, the token never reaching an answer, a log or the audit, and the
 * operator's revocations. Publishing with one, end to end, is in
 * store-identity.workerd.test.ts.
 */
import { SELF, env, runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Env } from "../../src/env";
import { PLUGIN_KEY_PROOF_PREFIX } from "../../src/plugin-keys";
import { deployTokenHash, tokenFromBody } from "../../src/store/identity/deploy-token";
import { signingKeyAllowed, tokenInScope, type Publisher, type ScopedPlugin } from "../../src/store/principal";
import { DEPLOY_TOKENS_PER_DAY, MAX_DEPLOY_TOKEN_PLUGINS, MAX_LIVE_DEPLOY_TOKENS, type UserDirectory } from "../../src/user-directory";
import { SITE, directory, errorOf, newSigner, person, setOwner, siteChange, stepUp, webSignIn } from "./identity-helpers";
import { author, makePackage, newSigner as newPackageSigner, publish, tokenPublish, uniqueId } from "./store-helpers";

type Scope = { all: true } | { plugins: string[] };
interface Made {
  token: string;
  id: string;
  short: string;
  name: string;
  scope: Scope;
  created_at: number;
  expires_at: number | null;
}

const newPlugin = () => `p${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;
const TOKENS = "/store/v1/me/tokens";

const api = (bearer: string | undefined, method: string, path: string, body?: unknown, headers: Record<string, string> = {}) =>
  SELF.fetch(`https://edge.test${path}`, {
    method,
    headers: {
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body === undefined ? {} : { "content-type": "application/json" }),
      ...headers
    },
    body: body === undefined ? undefined : typeof body === "string" || body instanceof ReadableStream ? body : JSON.stringify(body)
  });

/** An account owning `n` fresh plugins, claimed a minute ago (before any
 * token a test then makes), as the store records a first publish. */
const owned = async (opts: { n?: number; username?: string } = {}) => {
  const userId = await person(opts.username);
  const plugins = Array.from({ length: opts.n ?? 1 }, newPlugin);
  for (const plugin of plugins) await setOwner(plugin, userId, Date.now() - 60_000);
  return { userId, pluginId: plugins[0]!, plugins };
};

/** Make a token with a bearer and a fresh step-up. */
const addToken = async (userId: string, scope: unknown, extra: Record<string, unknown> = {}) =>
  api(userId, "POST", TOKENS, { name: "GitHub Actions", scope, stepUp: await stepUp(userId), ...extra });

const made = async (userId: string, scope: Scope, extra: Record<string, unknown> = {}): Promise<Made> => {
  const res = await addToken(userId, scope, extra);
  expect(res.status, await res.clone().text()).toBe(201);
  return (await res.json()) as Made;
};

const tokensOf = async (userId: string) => ((await (await api(userId, "GET", TOKENS)).json()) as { tokens: Record<string, unknown>[] }).tokens;

/** Who the principal probes say a request carrying `token` is (fixture.ts). */
const probe = (token: string, path: string, opts: { method?: string; body?: BodyInit } = {}) =>
  SELF.fetch(`https://edge.test${path}`, {
    method: opts.method ?? "POST",
    headers: { authorization: `Bearer ${token}`, "x-test-principal": "1" },
    body: (opts.method ?? "POST") === "GET" ? undefined : (opts.body ?? "pkg")
  });
const versions = (pluginId: string) => `/store/v1/plugins/${pluginId}/versions`;

const sql = (query: string, ...args: unknown[]) =>
  runInDurableObject(directory() as unknown as DurableObjectStub<UserDirectory>, (_i, state) => {
    state.storage.sql.exec(query, ...args);
  });

afterEach(() => {
  vi.restoreAllMocks();
});

describe("deploy tokens: making one", () => {
  it("answers the secret once, keeps its hash and short id, and lists it without either secret or hash", async () => {
    const { userId, plugins } = await owned({ n: 2 });
    const expires = Date.now() + 30 * 86_400_000;
    // Sent in any order; kept sorted.
    const res = await addToken(userId, { plugins: [...plugins].sort().reverse() }, { expires });
    expect(res.status, await res.clone().text()).toBe(201);
    expect(res.headers.get("cache-control")).toBe("no-store");
    const token = (await res.json()) as Made;
    expect(token).toEqual({
      token: expect.stringMatching(/^zdt_[0-9A-Za-z]{49}$/),
      id: expect.stringMatching(/^tok_[0-9A-Za-z]{16}$/),
      short: token.token.slice(4, 12),
      name: "GitHub Actions",
      scope: { plugins: [...plugins].sort() },
      created_at: expect.any(Number),
      expires_at: expires
    });
    // Its checksum is the body's.
    expect(tokenFromBody(token.token.slice(4, 47))).toBe(token.token);
    const listed = await api(userId, "GET", TOKENS);
    expect(listed.headers.get("cache-control")).toBe("no-store");
    const text = await listed.clone().text();
    expect(text).not.toContain(token.token);
    expect(text).not.toContain(await deployTokenHash(token.token));
    expect(((await listed.json()) as { tokens: unknown[] }).tokens).toEqual([
      { id: token.id, short: token.short, name: "GitHub Actions", scope: token.scope, created_at: token.created_at, last_used_at: null, last_used_plugin: null, expires_at: expires, revoked_at: null }
    ]);
    // An `all` token that never expires; each a new secret.
    const all = await made(userId, { all: true });
    expect(all).toMatchObject({ scope: { all: true }, expires_at: null });
    expect(all.token).not.toBe(token.token);
    expect((await tokensOf(userId)).map((t) => t.id)).toEqual([token.id, all.id]);
    // Only the directory's hash, never the secret.
    const stored = await runInDurableObject(directory() as unknown as DurableObjectStub<UserDirectory>, (_i, state) =>
      state.storage.sql.exec("SELECT token_hash, short FROM deploy_tokens WHERE id = ?", token.id).one()
    );
    expect(stored).toEqual({ token_hash: await deployTokenHash(token.token), short: token.short });
  });

  it("holds a token's scope to plugins its account owns now, each named once, and checks its name and expiry", async () => {
    const { userId, pluginId } = await owned();
    const theirs = await owned();
    const unclaimed = newPlugin();
    const notMine = await addToken(userId, { plugins: [pluginId, theirs.pluginId, unclaimed] });
    expect(notMine.status).toBe(403);
    expect(await notMine.json()).toMatchObject({ error: "not_owner", ids: [theirs.pluginId, unclaimed].sort() });
    const ticket = await stepUp(userId);
    const bad: [unknown, string[] | undefined][] = [
      [undefined, undefined],
      [{}, undefined],
      [{ all: false }, undefined],
      [{ all: true, plugins: [pluginId] }, undefined],
      [[pluginId], undefined],
      [{ plugins: [] }, undefined],
      [{ plugins: Array.from({ length: MAX_DEPLOY_TOKEN_PLUGINS + 1 }, newPlugin) }, undefined],
      [{ plugins: [pluginId, "Not_An_Id", 7] }, ["Not_An_Id", "7"]],
      [{ plugins: [pluginId, unclaimed, pluginId] }, [pluginId]]
    ];
    for (const [scope, ids] of bad) {
      const res = await api(userId, "POST", TOKENS, { name: "ci", scope, stepUp: ticket });
      const body = (await res.json()) as { error: string; ids?: string[] };
      expect([res.status, body.error], JSON.stringify(scope)).toEqual([400, "invalid_scope"]);
      expect(body.ids, JSON.stringify(scope)).toEqual(ids);
    }
    const wrong: [Record<string, unknown>, string][] = [
      [{ name: "" }, "invalid_name"],
      [{ name: "   " }, "invalid_name"],
      [{ name: "x".repeat(65) }, "invalid_name"],
      [{ name: "a\nb" }, "invalid_name"],
      [{ name: 7 }, "invalid_name"],
      [{ expires: Date.now() - 1 }, "invalid_expiry"],
      [{ expires: Date.now() + 6 * 365 * 86_400_000 }, "invalid_expiry"],
      [{ expires: "soon" }, "invalid_expiry"]
    ];
    for (const [change, error] of wrong) {
      const res = await api(userId, "POST", TOKENS, { name: "ci", scope: { plugins: [pluginId] }, stepUp: ticket, ...change });
      expect([res.status, await errorOf(res)], JSON.stringify(change)).toEqual([400, error]);
    }
    expect((await api(userId, "POST", TOKENS, "not json", { "content-type": "application/json" })).status).toBe(400);
    // (The ticket was never used: a good request takes it.)
    expect((await api(userId, "POST", TOKENS, { name: " ci ", scope: { plugins: [pluginId] }, stepUp: ticket })).status).toBe(201);
    expect((await tokensOf(userId))[0]!.name).toBe("ci");
    expect((await api(undefined, "GET", TOKENS)).status).toBe(401);
    expect((await api(undefined, "POST", TOKENS, {})).status).toBe(401);
  });

  it("needs a fresh sign-in from a device: a bearer alone can't make one", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const res = await api(userId, "POST", TOKENS, { name: "ci", scope });
    expect([res.status, await errorOf(res)]).toEqual([403, "step_up_required"]);
    const used = await stepUp(userId);
    expect((await api(userId, "POST", TOKENS, { name: "ci", scope, stepUp: used })).status).toBe(201);
    // The ticket is used up.
    expect((await api(userId, "POST", TOKENS, { name: "ci", scope, stepUp: used })).status).toBe(403);
    // Someone else's ticket is no good either, nor a made-up one.
    expect((await api(userId, "POST", TOKENS, { name: "ci", scope, stepUp: await stepUp(await person()) })).status).toBe(403);
    expect((await api(userId, "POST", TOKENS, { name: "ci", scope, stepUp: "x".repeat(43) })).status).toBe(403);
    expect(await tokensOf(userId)).toHaveLength(1);
  });

  it("takes a web sign-in as the fresh sign-in, once; signing in again makes the next", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const add = (browser: { cookie: string; csrf: string }) =>
      siteChange(TOKENS, browser, { headers: { "content-type": "application/json" }, body: JSON.stringify({ name: "From the site", scope }) });
    const browser = await webSignIn(userId);
    const first = await add(browser);
    expect(first.status).toBe(201);
    expect(((await first.json()) as Made).token).toMatch(/^zdt_/);
    const second = await add(browser);
    expect([second.status, await errorOf(second)]).toEqual([403, "step_up_required"]);
    const session = (await (await SELF.fetch(`${SITE}/auth/web/session`, { headers: { cookie: browser.cookie } })).json()) as { fresh_sign_in: boolean };
    expect(session.fresh_sign_in).toBe(false);
    const again = await webSignIn(userId, { cookie: browser.cookie });
    expect((await add(again)).status).toBe(201);
    // The list and a revocation, with the cookie.
    const listed = (await (await SELF.fetch(`${SITE}${TOKENS}`, { headers: { cookie: again.cookie } })).json()) as { tokens: { id: string }[] };
    expect(listed.tokens).toHaveLength(2);
    expect((await siteChange(`${TOKENS}/${listed.tokens[0]!.id}`, again, {}, "DELETE")).status).toBe(200);
    // And it's a change: the site's origin and token, as always.
    const forged = await SELF.fetch(`${SITE}${TOKENS}`, {
      method: "POST",
      headers: { cookie: again.cookie, origin: "https://evil.example", "x-csrf-token": again.csrf, "content-type": "application/json" },
      body: JSON.stringify({ name: "x", scope })
    });
    expect(forged.status).toBe(403);
    const noCsrf = await SELF.fetch(`${SITE}${TOKENS}/${listed.tokens[1]!.id}`, { method: "DELETE", headers: { cookie: again.cookie, origin: SITE } });
    expect(noCsrf.status).toBe(403);
  });

  it(`keeps an account to ${MAX_LIVE_DEPLOY_TOKENS} live tokens and ${DEPLOY_TOKENS_PER_DAY} new ones a day`, async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const aDayPasses = () => sql("UPDATE deploy_tokens SET created_at = created_at - 86400001 WHERE user_id = ?", userId);
    for (let i = 0; i < DEPLOY_TOKENS_PER_DAY; i++) expect((await addToken(userId, scope)).status).toBe(201);
    const today = await addToken(userId, scope);
    expect(today.headers.get("retry-after")).not.toBeNull();
    expect([today.status, await errorOf(today)]).toEqual([429, "rate_limited"]);
    await aDayPasses();
    for (let i = 0; i < MAX_LIVE_DEPLOY_TOKENS - DEPLOY_TOKENS_PER_DAY; i++) expect((await addToken(userId, scope)).status).toBe(201);
    await aDayPasses();
    // A new day, but the account is full.
    const full = await addToken(userId, scope);
    expect([full.status, await errorOf(full)]).toEqual([409, "too_many_tokens"]);
    // An expired token isn't live, nor a revoked one: each makes room.
    const tokens = await tokensOf(userId);
    await sql("UPDATE deploy_tokens SET expires_at = ? WHERE id = ?", Date.now() - 1, tokens[0]!.id);
    expect((await addToken(userId, scope)).status).toBe(201);
    expect((await addToken(userId, scope)).status).toBe(409);
    expect((await api(userId, "DELETE", `${TOKENS}/${tokens[1]!.id}`)).status).toBe(200);
    expect((await addToken(userId, scope)).status).toBe(201);
    // A refusal doesn't use up the ticket.
    const ticket = await stepUp(userId);
    expect((await api(userId, "POST", TOKENS, { name: "ci", scope, stepUp: ticket })).status).toBe(409);
    expect((await api(userId, "DELETE", `${TOKENS}/${tokens[2]!.id}`)).status).toBe(200);
    expect((await api(userId, "POST", TOKENS, { name: "ci", scope, stepUp: ticket })).status).toBe(201);
    // Another account counts its own.
    const other = await owned();
    expect((await addToken(other.userId, { plugins: [other.pluginId] })).status).toBe(201);
  });

  it("is revoked by its owner, once, and only theirs", async () => {
    const { userId, pluginId } = await owned();
    const token = await made(userId, { plugins: [pluginId] });
    const stranger = await person();
    expect([(await api(stranger, "DELETE", `${TOKENS}/${token.id}`)).status, (await tokensOf(userId))[0]!.revoked_at]).toEqual([404, null]);
    const revoked = await api(userId, "DELETE", `${TOKENS}/${token.id}`);
    expect([revoked.status, await revoked.json()]).toEqual([200, { revoked: true }]);
    const revokedAt = async () => (await tokensOf(userId))[0]!.revoked_at;
    const first = await revokedAt();
    expect(first).toEqual(expect.any(Number));
    expect((await api(userId, "DELETE", `${TOKENS}/${token.id}`)).status).toBe(200);
    expect(await revokedAt()).toBe(first);
    // By its id only: never its secret or its short id.
    for (const id of ["tok_unknown", token.short, token.token, "tok_0000000000000000"]) {
      expect((await api(userId, "DELETE", `${TOKENS}/${id}`)).status, id).toBe(404);
    }
    for (const [method, path] of [["PUT", TOKENS], ["DELETE", TOKENS], ["GET", `${TOKENS}/${token.id}`], ["POST", `${TOKENS}/${token.id}`]] as const) {
      expect((await api(userId, method, path)).status, `${method} ${path}`).toBe(405);
    }
    expect((await probe(token.token, versions(pluginId))).status).toBe(401);
  });

  it("is never managed by a deploy token or a runner token", async () => {
    const { userId, pluginId } = await owned();
    const token = await made(userId, { plugins: [pluginId] });
    for (const [method, path, body] of [
      ["GET", TOKENS, undefined],
      ["POST", TOKENS, { name: "x", scope: { all: true }, stepUp: await stepUp(userId) }],
      ["DELETE", `${TOKENS}/${token.id}`, undefined]
    ] as const) {
      const res = await api(token.token, method, path, body);
      expect([res.status, await errorOf(res)], `${method} ${path}`).toEqual([401, "session_required"]);
    }
    expect((await tokensOf(userId)).map((t) => t.revoked_at)).toEqual([null]);
    expect((await api(`runner:${userId}@org1:cloud-1`, "GET", TOKENS)).status).toBe(403);
    expect((await api(`runner:${userId}@org1:cloud-1`, "DELETE", `${TOKENS}/${token.id}`)).status).toBe(403);
  });
});

describe("deploy tokens: who's calling", () => {
  it("publishes as its account, with its ids, name and scope, and records its last use", async () => {
    const { userId, pluginId } = await owned({ username: `dt${crypto.randomUUID().slice(0, 8)}` });
    const token = await made(userId, { plugins: [pluginId] });
    const res = await probe(token.token, versions(pluginId));
    expect(res.status).toBe(200);
    expect(((await res.json()) as { publisher: unknown }).publisher).toEqual({
      userId,
      username: expect.stringMatching(/^dt/),
      via: "deploy_token",
      token: { id: token.id, short: token.short, name: "GitHub Actions", scope: { plugins: [pluginId] }, createdAt: token.created_at }
    });
    const listed = (await tokensOf(userId))[0]!;
    expect(Math.abs(Date.now() - (listed.last_used_at as number))).toBeLessThan(60_000);
    expect(listed.last_used_plugin).toBe(pluginId);
  });

  it("refuses anything but publishing, reads included, before the token is looked up", async () => {
    const { userId, pluginId } = await owned();
    const token = await made(userId, { plugins: [pluginId] });
    for (const [method, path] of [
      // The owner's view (decline notes, unlisted plugins, token names), even
      // for the plugins in its scope (contract "A token publishes and reads
      // nothing").
      ["GET", "/store/v1/me/plugins"],
      ["POST", `/store/v1/plugins/${pluginId}/installed`],
      ["POST", `${versions(pluginId)}/1.0.0/yank`],
      ["POST", `${versions(pluginId)}/1.0.0/unyank`],
      ["GET", versions(pluginId)],
      ["POST", `/store/v1/admin/versions/${pluginId}/1.0.0/verify`],
      ["GET", "/store/v1/admin/queue"],
      ["POST", "/store/v1/reports"]
    ] as const) {
      const res = await api(token.token, method, path);
      expect([res.status, await errorOf(res)], `${method} ${path}`).toEqual([401, "session_required"]);
    }
    // Not a use.
    expect((await tokensOf(userId))[0]).toMatchObject({ last_used_at: null });
  });

  it("is refused before a byte of the body is read", async () => {
    const { userId, pluginId } = await owned();
    const token = await made(userId, { plugins: [pluginId] });
    // A body that never ends: reading it would never answer.
    const endless = () =>
      new ReadableStream<Uint8Array>({
        pull: (controller) =>
          new Promise((resolve) =>
            setTimeout(() => {
              controller.enqueue(new Uint8Array(1024));
              resolve();
            }, 50)
          )
      });
    const revoked = await made(userId, { all: true });
    expect((await api(userId, "DELETE", `${TOKENS}/${revoked.id}`)).status).toBe(200);
    expect((await api(revoked.token, "POST", versions(pluginId), endless())).status).toBe(401);
    const unknown = tokenFromBody("1".repeat(43));
    expect((await api(unknown, "POST", versions(pluginId), endless())).status).toBe(401);
    // Out of its scope, or a plugin no one has.
    const other = newPlugin();
    await setOwner(other, userId, Date.now() - 60_000);
    expect((await api(token.token, "POST", versions(other), endless())).status).toBe(403);
    expect((await api(token.token, "POST", versions(newPlugin()), endless())).status).toBe(403);
  });

  it("refuses every bad token with one answer: malformed, a bad checksum, unknown, revoked, expired, a deleted owner's", async () => {
    const { userId, pluginId } = await owned();
    const scope = { plugins: [pluginId] };
    const good = await made(userId, scope);
    const revoked = await made(userId, scope);
    expect((await api(userId, "DELETE", `${TOKENS}/${revoked.id}`)).status).toBe(200);
    const expiring = await made(userId, scope, { expires: Date.now() + 60_000 });
    expect((await probe(expiring.token, versions(pluginId))).status).toBe(200);
    await sql("UPDATE deploy_tokens SET expires_at = ? WHERE id = ?", Date.now() - 1, expiring.id);
    const gone = await owned();
    const theirs = await made(gone.userId, { all: true });
    const stamp = new Date().toISOString();
    await directory().remove({ id: gone.userId, email: "", first_name: null, last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp }, stamp);
    const last = good.token.at(-1)!;
    const bad = [
      "zdt_",
      "zdt_short",
      `${good.token}x`,
      good.token.slice(0, -1),
      `${good.token.slice(0, -1)}${last === "A" ? "B" : "A"}`,
      `zdt_${good.token.slice(4, 46)}!${good.token.slice(47)}`,
      // Its short id and checksum right, its secret not.
      tokenFromBody(`${good.short}${"0".repeat(35)}`),
      tokenFromBody("0".repeat(43)),
      revoked.token,
      expiring.token,
      theirs.token
    ];
    const answers = new Set<string>();
    for (const token of bad) {
      for (const path of [versions(pluginId), `${versions(pluginId)}/1.0.0/yank`]) {
        const res = await probe(token, path);
        if (path !== versions(pluginId)) {
          // Anywhere else, refused before it's looked up: never a hint.
          expect([res.status, await errorOf(res)], path).toEqual([401, "session_required"]);
          continue;
        }
        expect(res.status, `${token} ${path}`).toBe(401);
        const text = await res.text();
        expect(text).not.toContain(token);
        answers.add(text);
      }
    }
    expect([...answers]).toEqual([JSON.stringify({ error: "invalid_token", message: "This deploy token isn't valid: it may be mistyped, revoked or expired." })]);
    // The good one still is.
    expect((await probe(good.token, versions(pluginId))).status).toBe(200);
    // Any other bearer is a session's, refused as one.
    expect(await errorOf(await probe("not-a-token", versions(pluginId)))).not.toBe("invalid_token");
  });
});

describe("deploy tokens: what a scope holds", () => {
  const row = async (id: string) => (await env.STORE_DB!.prepare("SELECT id, owner_id, owner_since, created_at FROM plugins WHERE id = ?").bind(id).first<ScopedPlugin>())!;
  const holder = async (token: Made, userId: string): Promise<Publisher> => ({
    userId,
    username: null,
    via: "deploy_token",
    token: { id: token.id, short: token.short, name: token.name, scope: (await tokensOf(userId)).find((t) => t.id === token.id)!.scope as Scope, createdAt: token.created_at }
  });

  it("a list holds the plugins named; `all` what the account owned when it was made or claims later, never a plugin moved in", async () => {
    const { userId, plugins } = await owned({ n: 2 });
    const [listed, other] = plugins as [string, string];
    const list = await made(userId, { plugins: [listed] });
    const all = await made(userId, { all: true });
    const later = newPlugin();
    await setOwner(later, userId, Date.now() + 1000);
    const theirs = await owned();
    await setOwner(theirs.pluginId, userId, Date.now() + 1000);
    const asList = await holder(list, userId);
    const asAll = await holder(all, userId);
    expect([tokenInScope(asList, await row(listed)), tokenInScope(asList, await row(other)), tokenInScope(asList, await row(later))]).toEqual([true, false, false]);
    expect([tokenInScope(asAll, await row(listed)), tokenInScope(asAll, await row(other)), tokenInScope(asAll, await row(later))]).toEqual([true, true, true]);
    expect(tokenInScope(asAll, await row(theirs.pluginId))).toBe(false);
    // Another account's plugin, whatever the scope says.
    expect(tokenInScope({ ...asAll, userId: theirs.userId }, await row(listed))).toBe(false);
  });

  it("a plugin leaving the account leaves every token's scope for good: moved away, moved back, or released and claimed afresh", async () => {
    const { userId: former, plugins } = await owned({ n: 2 });
    const [pluginId, kept] = plugins as [string, string];
    const list = await made(former, { plugins: [pluginId, kept] });
    const all = await made(former, { all: true });
    const inScope = async (token: Made, plugin = pluginId) => tokenInScope(await holder(token, former), await row(plugin));
    expect([await inScope(list), await inScope(all)]).toEqual([true, true]);
    // Transferred, before (or without) the store dropping it from the lists.
    const next = await person();
    await setOwner(pluginId, next, Date.now() + 1000);
    expect([await inScope(list), await inScope(all), await inScope(list, kept)]).toEqual([false, false, true]);
    // The tokens stay their account's: its to see and revoke, never the new owner's.
    expect((await tokensOf(former)).map((t) => t.id)).toEqual([list.id, all.id]);
    expect(await tokensOf(next)).toEqual([]);
    expect((await api(next, "DELETE", `${TOKENS}/${list.id}`)).status).toBe(404);
    // Back again: still not.
    await setOwner(pluginId, former, Date.now() + 2000);
    expect([await inScope(list), await inScope(all)]).toEqual([false, false]);
    // Released and claimed afresh by the account itself: never in a list made
    // before; in `all`, as anything the account claims later.
    await setOwner(pluginId, null);
    await setOwner(pluginId, former, Date.now() + 3000);
    expect([await inScope(list), await inScope(all)]).toEqual([false, true]);
    // The store's transfer drops it from the lists too, for the owner to see.
    expect(await directory().dropFromDeployTokens(former, pluginId)).toBe(1);
    expect((await tokensOf(former)).find((t) => t.id === list.id)!.scope).toEqual({ plugins: [kept] });
  });
});

describe("deploy tokens: never told", () => {
  it("no answer, log line or audit row carries a token, from its making to its last refusal", async () => {
    const logged: unknown[] = [];
    for (const level of ["log", "info", "warn", "error", "debug"] as const) {
      vi.spyOn(console, level).mockImplementation((...args: unknown[]) => void logged.push(...args));
    }
    const a = await author();
    const id = uniqueId("ci");
    expect((await publish(a.userId, id, await makePackage({ id, version: "1.0.0", signer: a.signer }))).status).toBe(201);
    const created = await api(a.userId, "POST", TOKENS, { name: "Leak check", scope: { plugins: [id] }, stepUp: await stepUp(a.userId) });
    const { token, id: tokenId, short } = (await created.json()) as Made;
    const hash = await deployTokenHash(token);
    const answers: string[] = [];
    const keep = async (res: Response) => {
      answers.push(JSON.stringify([...res.headers]), await res.text());
      return res.status;
    };
    // Published, refused for every reason, listed, refused elsewhere, revoked.
    expect(await keep(await tokenPublish(token, id, await makePackage({ id, version: "1.1.0", signer: null })))).toBe(201);
    // (The same package again is a retry, answered alike; another is refused.)
    expect(await keep(await tokenPublish(token, id, await makePackage({ id, version: "1.1.0", signer: null })))).toBe(201);
    expect(await keep(await tokenPublish(token, id, await makePackage({ id, version: "1.1.0", signer: null, files: { "extra.txt": "x" } })))).toBe(409);
    expect(await keep(await tokenPublish(token, id, new TextEncoder().encode("not a package")))).toBe(400);
    expect(await keep(await tokenPublish(token, uniqueId("ci"), await makePackage({ id, version: "1.2.0", signer: null })))).toBe(403);
    expect(await keep(await api(token, "GET", "/store/v1/me/plugins"))).toBe(401);
    expect(await keep(await api(token, "GET", TOKENS))).toBe(401);
    expect(await keep(await api(token, "POST", `${versions(id)}/1.1.0/yank`))).toBe(401);
    expect(await keep(await api(a.userId, "GET", TOKENS))).toBe(200);
    expect(await keep(await api(a.userId, "GET", `/store/v1/plugins/${id}`))).toBe(200);
    expect(await keep(await api(undefined, "GET", `/store/v1/plugins/${id}/versions`))).toBe(200);
    expect(await keep(await api(a.userId, "DELETE", `${TOKENS}/${tokenId}`))).toBe(200);
    expect(await keep(await tokenPublish(token, id, await makePackage({ id, version: "1.3.0", signer: null })))).toBe(401);
    expect(await keep(await api(token, "GET", "/store/v1/me/plugins"))).toBe(401);
    const audit = await env.STORE_DB!.prepare("SELECT action, detail FROM audit WHERE plugin_id = ?").bind(id).all<{ action: string; detail: string | null }>();
    const version = await env.STORE_DB!.prepare("SELECT * FROM versions WHERE plugin_id = ? AND version = '1.1.0'").bind(id).first();
    const everything = JSON.stringify({ answers, logged: logged.map(String), audit: audit.results, version });
    expect(everything).not.toContain(token);
    expect(everything).not.toContain(token.slice(4, 47));
    expect(everything).not.toContain(hash);
    // Where it's named, it's by its short id and name.
    expect(audit.results.find((e) => e.action === "version.publish" && e.detail?.includes("deploy_token"))!.detail).toContain(short);
    expect(version).toMatchObject({ via: "deploy_token", deploy_token_name: "Leak check", deploy_token_short: short, deploy_token_id: tokenId, signer_key: null });
  });
});

describe("signingKeyAllowed", () => {
  it("a session's: a live device key of the caller's account, and nothing else", async () => {
    const userId = await person();
    const session: Publisher = { userId, username: null, via: "session" };
    const device = await newSigner();
    expect((await api(userId, "POST", "/plugin-keys", { key: device.key, proof: await device.sign(PLUGIN_KEY_PROOF_PREFIX + device.key), stepUp: await stepUp(userId) })).status).toBe(201);
    const allowed = (publisher: Publisher, key: string) => signingKeyAllowed(env as unknown as Env, publisher, key);
    expect(await allowed(session, device.key)).toBe(true);
    // Someone else's key, or none at all.
    expect(await allowed({ userId: await person(), username: null, via: "session" }, device.key)).toBe(false);
    expect(await allowed(session, (await newSigner()).key)).toBe(false);
    // Revoked keys sign nothing.
    expect((await api(userId, "DELETE", `/plugin-keys/${device.key}`)).status).toBe(200);
    expect(await allowed(session, device.key)).toBe(false);
  });
});

// Last in this file: it revokes every deploy token there is.
describe("deploy tokens: the operator's routes", () => {
  const operator = (method: string, path: string, body?: unknown, token = "test-admin-token") =>
    SELF.fetch(`https://edge.test${path}`, {
      method,
      headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body)
    });

  it("revoke the tokens that may publish one plugin, or every token (on a leak), with the operator token only", async () => {
    const a = await owned({ n: 2 });
    const b = await owned();
    const [leaked, other] = a.plugins as [string, string];
    const aToken = await made(a.userId, { plugins: [leaked] });
    const aAll = await made(a.userId, { all: true });
    const aOther = await made(a.userId, { plugins: [other] });
    const bToken = await made(b.userId, { plugins: [b.pluginId] });
    expect((await operator("GET", "/admin/deploy-tokens", undefined, "wrong")).status).toBe(401);
    expect((await operator("POST", "/admin/deploy-tokens/revoke", { all: true }, a.userId)).status).toBe(401);
    expect((await operator("POST", "/admin/deploy-tokens/revoke", { all: true }, aAll.token)).status).toBe(401);
    for (const body of [{}, { pluginId: "Bad Id" }, { all: "yes" }, { all: true, pluginId: leaked }]) {
      expect((await operator("POST", "/admin/deploy-tokens/revoke", body)).status, JSON.stringify(body)).toBe(400);
    }
    expect(((await (await operator("GET", "/admin/deploy-tokens")).json()) as { live: number }).live).toBeGreaterThanOrEqual(4);
    // An id no one owns has no tokens.
    expect(await (await operator("POST", "/admin/deploy-tokens/revoke", { pluginId: newPlugin() })).json()).toEqual({ revoked: 0 });
    // Its owner's tokens whose scope may hold it: the one listing it, and `all`.
    expect(await (await operator("POST", "/admin/deploy-tokens/revoke", { pluginId: leaked })).json()).toEqual({ revoked: 2 });
    expect((await probe(aToken.token, versions(leaked))).status).toBe(401);
    expect((await probe(aAll.token, versions(other))).status).toBe(401);
    expect((await probe(aOther.token, versions(other))).status).toBe(200);
    expect((await probe(bToken.token, versions(b.pluginId))).status).toBe(200);
    const all = (await (await operator("POST", "/admin/deploy-tokens/revoke", { all: true })).json()) as { revoked: number };
    expect(all.revoked).toBeGreaterThanOrEqual(2);
    expect((await probe(bToken.token, versions(b.pluginId))).status).toBe(401);
    expect(await (await operator("GET", "/admin/deploy-tokens")).json()).toEqual({ live: 0 });
    expect((await operator("DELETE", "/admin/deploy-tokens")).status).toBe(405);
  });
});
