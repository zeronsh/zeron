import { SELF, env, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import type { Env } from "../../src/env";
import type { UserDirectory } from "../../src/user-directory";
import { handleWebAuthRoute, usesPkce } from "../../src/web-auth-routes";
import {
  ROTATED_TOKEN_GRACE_MS,
  STAFF_SESSION_ABSOLUTE_MS,
  STAFF_SESSION_IDLE_MS,
  STAFF_TOKEN_ROTATION_MS,
  WEB_SESSION_ABSOLUTE_MS,
  WEB_SESSION_IDLE_MS
} from "../../src/user-directory";
import { sessionTokenHash } from "../../src/web-cookies";
import { sha256Hex } from "../../src/store/identity/signing";
import {
  SITE,
  directory,
  errorOf,
  fetchSite,
  grantAdmin,
  newId,
  person,
  revokeAdmin,
  sessionHeader,
  setCookies,
  siteChange,
  startLogin,
  webSignIn,
  workos
} from "./identity-helpers";

/** Move a session's clock: `columns` set to now minus each value (ms). */
const age = async (token: string, columns: Record<string, number>) => {
  const hash = (await sessionTokenHash(token))!;
  await runInDurableObject(directory() as unknown as DurableObjectStub<UserDirectory>, (_i, state) => {
    for (const [column, ago] of Object.entries(columns)) {
      state.storage.sql.exec(`UPDATE web_sessions SET ${column} = ? WHERE token_hash = ? OR prev_token_hash = ?`, Date.now() - ago, hash, hash);
    }
  });
};

const sessionOf = (cookie: string) => fetchSite("/auth/web/session", { cookie });

describe("web sign-in", () => {
  it("sends the browser to AuthKit with state, PKCE and a forced re-authentication, bound by a __Host- cookie", async () => {
    const { res, authorize } = await startLogin("/plugins/git");
    expect(`${authorize.origin}${authorize.pathname}`).toBe("https://api.workos.com/user_management/authorize");
    const q = authorize.searchParams;
    expect(q.get("client_id")).toBe("test-only");
    expect(q.get("redirect_uri")).toBe(`${SITE}/auth/web/callback`);
    expect(q.get("response_type")).toBe("code");
    expect(q.get("provider")).toBe("authkit");
    expect(q.get("state")).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(q.get("code_challenge")).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(q.get("code_challenge_method")).toBe("S256");
    expect(q.get("max_age")).toBe("0");
    const cookie = res.headers.getSetCookie().find((c) => c.startsWith("__Host-zeron_web_login="))!;
    expect(cookie).toMatch(/; Path=\/; Secure; HttpOnly; SameSite=Lax; Max-Age=600$/);
    expect(cookie).not.toMatch(/domain=/i);
    // Two sign-ins never share a state.
    expect((await startLogin()).authorize.searchParams.get("state")).not.toBe(q.get("state"));
  });

  it("signs the browser in: a revocable __Host- session, back where it started, with a step-up", async () => {
    const userId = await person(`web${newId("u").slice(5, 13).toLowerCase()}`);
    const { authorize, loginCookie } = await startLogin("/plugins/git?tab=versions");
    const code = newId("code");
    await workos("__codes", { code, user_id: userId, code_challenge: authorize.searchParams.get("code_challenge") });
    const res = await fetchSite(`/auth/web/callback?code=${code}&state=${authorize.searchParams.get("state")}`, { cookie: loginCookie });
    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe(`${SITE}/plugins/git?tab=versions`);
    expect(res.headers.get("cache-control")).toBe("no-store");
    const cookies = res.headers.getSetCookie();
    const session = cookies.find((c) => c.startsWith("__Host-zeron_session="))!;
    expect(session).toMatch(/^__Host-zeron_session=[A-Za-z0-9_-]{43}; Path=\/; Secure; HttpOnly; SameSite=Lax; Max-Age=\d+$/);
    expect(cookies.some((c) => c.startsWith("__Host-zeron_web_login=;") && c.includes("Max-Age=0"))).toBe(true);
    // The code went with the client secret and this browser's verifier.
    const { authenticated } = (await workos("__state")) as { authenticated: { code: string; code_verifier: string | null; client_secret: string | null }[] };
    const sent = authenticated.find((a) => a.code === code)!;
    expect(sent.client_secret).toBe("sk_test");
    expect(sent.code_verifier).toMatch(/^[A-Za-z0-9_-]{43}$/);

    const token = setCookies(res).get("__Host-zeron_session")!;
    const answer = await sessionOf(sessionHeader(token));
    expect(answer.status).toBe(200);
    expect(answer.headers.get("cache-control")).toBe("no-store");
    const body = (await answer.json()) as Record<string, unknown>;
    expect(body).toMatchObject({ user: { name: "Ada Lovelace" }, roles: [], fresh_sign_in: true });
    expect(body.user).not.toHaveProperty("avatar");
    expect(body.username).toMatch(/^web/);
    expect(body.csrf).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(body.expires_at).toEqual(expect.any(Number));
    // No user id goes out.
    expect(JSON.stringify(body)).not.toContain(userId);
    // The directory keeps only the token's hash.
    const stored = await runInDurableObject(directory() as unknown as DurableObjectStub<UserDirectory>, (_i, state) =>
      state.storage.sql.exec("SELECT token_hash FROM web_sessions WHERE user_id = ?", userId).toArray()
    );
    expect(stored).toEqual([{ token_hash: await sessionTokenHash(token) }]);
  });

  it("refuses a callback whose state isn't this browser's (login CSRF)", async () => {
    const victim = await startLogin();
    const attacker = await startLogin();
    const userId = await person();
    const code = newId("code");
    await workos("__codes", { code, user_id: userId, code_challenge: attacker.authorize.searchParams.get("code_challenge") });
    // The attacker's own callback link, opened in the victim's browser.
    const forged = await fetchSite(`/auth/web/callback?code=${code}&state=${attacker.authorize.searchParams.get("state")}`, {
      cookie: victim.loginCookie
    });
    expect(forged.status).toBe(400);
    expect(setCookies(forged).has("__Host-zeron_session")).toBe(false);
    // No sign-in attempt in this browser at all.
    const bare = await fetchSite(`/auth/web/callback?code=${code}&state=${attacker.authorize.searchParams.get("state")}`);
    expect(bare.status).toBe(400);
    expect(setCookies(bare).has("__Host-zeron_session")).toBe(false);
    // A state that isn't one, or none.
    for (const query of [`code=${code}&state=x`, `code=${code}`]) {
      const res = await fetchSite(`/auth/web/callback?${query}`, { cookie: victim.loginCookie });
      expect(res.status, query).toBe(400);
    }
  });

  it("refuses a code issued to another browser's sign-in, even with a matching state (PKCE)", async () => {
    const victim = await startLogin();
    const attacker = await startLogin();
    const userId = await person();
    const code = newId("code");
    // The attacker's code (bound to the attacker's challenge), sent back with
    // the victim's own state.
    await workos("__codes", { code, user_id: userId, code_challenge: attacker.authorize.searchParams.get("code_challenge") });
    const res = await fetchSite(`/auth/web/callback?code=${code}&state=${victim.authorize.searchParams.get("state")}`, {
      cookie: victim.loginCookie
    });
    expect(res.status).toBe(400);
    expect(setCookies(res).has("__Host-zeron_session")).toBe(false);
  });

  it("refuses a used code, and a sign-in AuthKit refused", async () => {
    const userId = await person();
    const { authorize, loginCookie } = await startLogin();
    const code = newId("code");
    await workos("__codes", { code, user_id: userId, code_challenge: authorize.searchParams.get("code_challenge") });
    const path = `/auth/web/callback?code=${code}&state=${authorize.searchParams.get("state")}`;
    expect((await fetchSite(path, { cookie: loginCookie })).status).toBe(303);
    expect((await fetchSite(path, { cookie: loginCookie })).status).toBe(400);
    const refused = await startLogin();
    const res = await fetchSite(`/auth/web/callback?error=access_denied&state=${refused.authorize.searchParams.get("state")}`, {
      cookie: refused.loginCookie
    });
    expect(res.status).toBe(400);
    expect(await res.text()).toContain("Sign-in didn&#39;t finish");
  });

  it("never redirects off the site (open redirect)", async () => {
    const userId = await person();
    for (const returnTo of ["//evil.example", "https://evil.example/x", "/\\evil.example", "/auth/web/logout", "javascript:alert(1)"]) {
      const browser = await webSignIn(userId, { returnTo });
      expect(browser.location, returnTo).toBe(`${SITE}/`);
    }
    // A login cookie edited by hand to point away is checked again.
    const { authorize } = await startLogin();
    const state = authorize.searchParams.get("state")!;
    const verifier = "A".repeat(43);
    const challenge = await crypto.subtle
      .digest("SHA-256", new TextEncoder().encode(verifier))
      .then((d) => btoa(String.fromCharCode(...new Uint8Array(d))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""));
    const evil = btoa("https://evil.example/").replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    const code = newId("code");
    await workos("__codes", { code, user_id: userId, code_challenge: challenge });
    const res = await fetchSite(`/auth/web/callback?code=${code}&state=${state}`, { cookie: `__Host-zeron_web_login=${state}.${verifier}.${evil}` });
    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe(`${SITE}/`);
  });

  it("sets its cookies only on the web origin", async () => {
    for (const path of ["/auth/web/login", "/auth/web/callback?code=x&state=y"]) {
      const res = await SELF.fetch(`https://edge.test${path}`, { redirect: "manual" });
      expect(res.status, path).toBe(404);
    }
    expect((await SELF.fetch("https://edge.test/auth/web/logout", { method: "POST", headers: { origin: SITE } })).status).toBe(404);
    expect((await fetchSite("/auth/web/login", { method: "POST" })).status).toBe(405);
    expect((await fetchSite("/auth/web/logout")).status).toBe(405);
    expect((await fetchSite("/auth/web/nope")).status).toBe(404);
  });

  it("starts a new session at each sign-in, ending the one the browser held", async () => {
    const userId = await person();
    const first = await webSignIn(userId);
    const second = await webSignIn(userId, { cookie: first.cookie });
    expect(second.token).not.toBe(first.token);
    expect((await sessionOf(first.cookie)).status).toBe(401);
    expect((await sessionOf(second.cookie)).status).toBe(200);
  });
});

describe("web sessions", () => {
  it("refuse a forged or malformed cookie, and clear it", async () => {
    for (const value of ["A".repeat(43), "not-a-token", "", `${"A".repeat(43)}; __Host-zeron_session=${"B".repeat(43)}`]) {
      const res = await sessionOf(sessionHeader(value));
      expect(res.status, value).toBe(401);
      expect(await errorOf(res)).toBe("signed_out");
      expect(res.headers.getSetCookie().some((c) => c.startsWith("__Host-zeron_session=;"))).toBe(true);
    }
    expect((await sessionOf("")).status).toBe(401);
  });

  it("refuse a cookie whose name only matches once Unicode spaces are trimmed (__Host- bypass)", async () => {
    const victim = await webSignIn(await person());
    for (const space of ["\u2000", "\u00a0", "\u3000", "\ufeff", "\u2028", "\u205f"]) {
      // A planted look-alike is never read as the session…
      expect((await sessionOf(`${space}${sessionHeader(victim.token)}`)).status, escape(space)).toBe(401);
      // …and doesn't shadow the real one beside it.
      expect((await sessionOf(`${space}${sessionHeader("A".repeat(43))}; ${victim.cookie}`)).status, escape(space)).toBe(200);
    }
    // Nor a planted sign-in attempt.
    const { authorize, loginCookie } = await startLogin();
    const res = await fetchSite(`/auth/web/callback?code=x&state=${authorize.searchParams.get("state")}`, { cookie: `\u2000${loginCookie}` });
    expect(res.status).toBe(400);
  });

  it("refuse a session cookie sent twice, even when one is real", async () => {
    const browser = await webSignIn(await person());
    expect((await sessionOf(`${browser.cookie}; ${sessionHeader("A".repeat(43))}`)).status).toBe(401);
  });

  it("are never read by the device and session API", async () => {
    const browser = await webSignIn(await person());
    for (const [method, path] of [
      ["GET", "/plugin-keys"],
      ["GET", "/username"],
      ["GET", "/chat2/chat1/stats"],
      ["GET", "/device/dev1/status"],
      ["GET", "/registry/org1/rows"],
      ["POST", "/plugin-keys"]
    ] as const) {
      const res = await SELF.fetch(`https://edge.test${path}`, { method, headers: { cookie: browser.cookie, origin: SITE, "x-csrf-token": browser.csrf } });
      expect(res.status, path).toBe(401);
      const onSite = await fetchSite(path, { method, cookie: browser.cookie, headers: { origin: SITE, "x-csrf-token": browser.csrf } });
      expect(onSite.status, `site ${path}`).toBe(401);
    }
  });

  it("go idle and end, on the normal lifetimes", async () => {
    const userId = await person();
    const idle = await webSignIn(userId);
    await age(idle.token, { last_seen_at: WEB_SESSION_IDLE_MS + 1000 });
    expect((await sessionOf(idle.cookie)).status).toBe(401);
    const old = await webSignIn(userId);
    await age(old.token, { created_at: WEB_SESSION_ABSOLUTE_MS + 1000 });
    expect((await sessionOf(old.cookie)).status).toBe(401);
    // A day old and used an hour ago is fine.
    const live = await webSignIn(userId);
    await age(live.token, { created_at: 86_400_000, last_seen_at: 3_600_000 });
    expect((await sessionOf(live.cookie)).status).toBe(200);
  });

  it("are short for an account holding a role, from the moment it's granted", async () => {
    const userId = await person();
    const browser = await webSignIn(userId);
    await age(browser.token, { created_at: STAFF_SESSION_ABSOLUTE_MS + 1000, last_seen_at: 60_000 });
    expect((await sessionOf(browser.cookie)).status).toBe(200);
    await grantAdmin(userId);
    expect((await sessionOf(browser.cookie)).status).toBe(401);
    const idle = await webSignIn(userId);
    await age(idle.token, { last_seen_at: STAFF_SESSION_IDLE_MS + 1000 });
    expect((await sessionOf(idle.cookie)).status).toBe(401);
    const live = await webSignIn(userId);
    const answer = (await (await sessionOf(live.cookie)).json()) as { roles: string[]; expires_at: number };
    expect(answer.roles).toEqual(["admin"]);
    expect(answer.expires_at - Date.now()).toBeLessThanOrEqual(STAFF_SESSION_IDLE_MS);
  });

  it("rotate a staff session's token on privileged use; the old one lapses after its grace", async () => {
    const userId = await person();
    await grantAdmin(userId);
    const browser = await webSignIn(userId);
    const first = await fetchSite("/store/v1/admin/queue", { cookie: browser.cookie });
    expect(first.status).toBe(200);
    const rotated = setCookies(first).get("__Host-zeron_session");
    expect(rotated).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(rotated).not.toBe(browser.token);
    // In flight with the old token: still fine for the grace.
    expect((await fetchSite("/store/v1/admin/queue", { cookie: browser.cookie })).status).toBe(200);
    // The CSRF token is the session's, not the token's.
    const after = (await (await sessionOf(sessionHeader(rotated!))).json()) as { csrf: string };
    expect(after.csrf).toBe(browser.csrf);
    // Used again soon: no new rotation.
    const again = await fetchSite("/store/v1/admin/queue", { cookie: sessionHeader(rotated!) });
    expect(again.status).toBe(200);
    expect(setCookies(again).has("__Host-zeron_session")).toBe(false);
    // The grace over, the old token is dead.
    await runInDurableObject(directory() as unknown as DurableObjectStub<UserDirectory>, async (_i, state) => {
      state.storage.sql.exec("UPDATE web_sessions SET prev_valid_until = ? WHERE token_hash = ?", Date.now() - 1, await sessionTokenHash(rotated));
    });
    expect((await sessionOf(browser.cookie)).status).toBe(401);
    expect((await sessionOf(sessionHeader(rotated!))).status).toBe(200);
    // After the rotation interval, privileged use rotates again.
    await age(rotated!, { token_issued_at: STAFF_TOKEN_ROTATION_MS + 1000 });
    const later = await fetchSite("/store/v1/admin/queue", { cookie: sessionHeader(rotated!) });
    expect(setCookies(later).get("__Host-zeron_session")).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(ROTATED_TOKEN_GRACE_MS).toBeGreaterThan(0);
  });

  it("never rotate on a refused change, so the browser keeps a working cookie", async () => {
    const userId = await person();
    await grantAdmin(userId);
    const browser = await webSignIn(userId);
    const refused = await fetchSite("/store/v1/admin/versions/demo/1.0.0/verify", {
      method: "POST",
      cookie: browser.cookie,
      headers: { origin: SITE, "x-csrf-token": "A".repeat(43) }
    });
    expect(refused.status).toBe(403);
    expect(setCookies(refused).has("__Host-zeron_session")).toBe(false);
    // Still the current token: its first privileged use rotates it now.
    const first = await fetchSite("/store/v1/admin/queue", { cookie: browser.cookie });
    expect(setCookies(first).get("__Host-zeron_session")).toMatch(/^[A-Za-z0-9_-]{43}$/);
  });

  it("never rotate for an account without a role", async () => {
    const browser = await webSignIn(await person());
    const res = await fetchSite("/store/v1/admin/queue", { cookie: browser.cookie });
    expect(res.status).toBe(403);
    expect(setCookies(res).has("__Host-zeron_session")).toBe(false);
  });

  it("end with the account", async () => {
    const userId = await person();
    await grantAdmin(userId);
    const browser = await webSignIn(userId);
    const stamp = new Date().toISOString();
    await directory().remove({ id: userId, email: "", first_name: null, last_name: null, profile_picture_url: null, created_at: stamp, updated_at: stamp }, stamp);
    expect((await sessionOf(browser.cookie)).status).toBe(401);
    expect(await directory().roles(userId)).toEqual([]);
    // And a deleted account can't sign in again.
    const { authorize, loginCookie } = await startLogin();
    const code = newId("code");
    await workos("__codes", { code, user_id: userId, code_challenge: authorize.searchParams.get("code_challenge") });
    const res = await fetchSite(`/auth/web/callback?code=${code}&state=${authorize.searchParams.get("state")}`, { cookie: loginCookie });
    expect(res.status).toBe(403);
    expect(setCookies(res).has("__Host-zeron_session")).toBe(false);
  });
});

describe("signing out", () => {
  it("needs the web origin and the session's CSRF token", async () => {
    const userId = await person();
    const browser = await webSignIn(userId);
    const other = await webSignIn(await person());
    const attempts: [string, Record<string, string>][] = [
      ["no Origin", { "x-csrf-token": browser.csrf }],
      ["foreign Origin", { origin: "https://evil.example", "x-csrf-token": browser.csrf }],
      ["cross-site fetch", { origin: SITE, "sec-fetch-site": "cross-site", "x-csrf-token": browser.csrf }],
      ["no token", { origin: SITE }],
      ["another session's token", { origin: SITE, "x-csrf-token": other.csrf }]
    ];
    for (const [what, headers] of attempts) {
      const res = await fetchSite("/auth/web/logout", { method: "POST", cookie: browser.cookie, headers });
      expect(res.status, what).toBe(403);
      // The same word as every store route's refused cookie write.
      expect(await errorOf(res), what).toBe("bad_csrf");
    }
    expect((await sessionOf(browser.cookie)).status).toBe(200);
  });

  it("ends the session and its WorkOS session, and clears the cookie", async () => {
    const sid = newId("session");
    const browser = await webSignIn(await person(), { sid });
    const res = await siteChange("/auth/web/logout", browser);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
    expect(res.headers.getSetCookie().some((c) => c.startsWith("__Host-zeron_session=;") && c.includes("Max-Age=0"))).toBe(true);
    expect((await sessionOf(browser.cookie)).status).toBe(401);
    expect(((await workos("__state")) as { revokedSessions: string[] }).revokedSessions).toContain(sid);
  });

  it("takes a form post, with its token as a field", async () => {
    const browser = await webSignIn(await person());
    const res = await fetchSite("/auth/web/logout", {
      method: "POST",
      cookie: browser.cookie,
      headers: { origin: SITE, "content-type": "application/x-www-form-urlencoded" },
      body: `csrf=${browser.csrf}`
    });
    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe(`${SITE}/`);
    expect((await sessionOf(browser.cookie)).status).toBe(401);
  });

  it("clears a dead cookie without a token, from the site only", async () => {
    const res = await fetchSite("/auth/web/logout", { method: "POST", cookie: sessionHeader("A".repeat(43)), headers: { origin: SITE } });
    expect(res.status).toBe(200);
    const foreign = await fetchSite("/auth/web/logout", { method: "POST", headers: { origin: "https://evil.example" } });
    expect([foreign.status, await errorOf(foreign)]).toEqual([403, "bad_csrf"]);
  });
});

/** The fixture's principal probes answer who the principal says is calling
 * (fixture.ts `storeStandIn`); without this the store's real routes answer. */
const PROBE = { "x-test-principal": "1" };

describe("cookie-authenticated store routes", () => {
  it("take a change only from the site with the session's own CSRF token", async () => {
    const userId = await person();
    const browser = await webSignIn(userId);
    const other = await webSignIn(await person());
    const path = "/store/v1/plugins/demo/versions";
    const refused: [string, Record<string, string>, string][] = [
      // (A wrong Origin is bad_csrf too on a store route: amendment 35.)
      ["no Origin", { "x-csrf-token": browser.csrf }, "bad_csrf"],
      ["foreign Origin", { origin: "https://evil.example", "x-csrf-token": browser.csrf }, "bad_csrf"],
      ["the edge's origin", { origin: "https://edge.test", "x-csrf-token": browser.csrf }, "bad_csrf"],
      ["no token", { origin: SITE }, "bad_csrf"],
      ["another session's token", { origin: SITE, "x-csrf-token": other.csrf }, "bad_csrf"],
      ["a guessed token", { origin: SITE, "x-csrf-token": "A".repeat(43) }, "bad_csrf"]
    ];
    for (const [what, headers, error] of refused) {
      const res = await fetchSite(path, { method: "POST", cookie: browser.cookie, headers: { ...headers, ...PROBE }, body: "pkg" });
      expect(res.status, what).toBe(403);
      expect(await errorOf(res), what).toBe(error);
    }
    const ok = await siteChange(path, browser, { body: "pkg", headers: PROBE });
    expect(ok.status).toBe(200);
    expect(await ok.json()).toMatchObject({ publisher: { userId, via: "session" } });
    // A plain form post carries the token as a field, and the route still
    // reads the whole body.
    const form = await fetchSite(path, {
      method: "POST",
      cookie: browser.cookie,
      headers: { origin: SITE, "content-type": "application/x-www-form-urlencoded", ...PROBE },
      body: `csrf=${browser.csrf}&note=hi`
    });
    expect(form.status).toBe(200);
    expect(((await form.json()) as { body: string }).body).toBe(await sha256Hex(new TextEncoder().encode(`csrf=${browser.csrf}&note=hi`)));
    const wrongField = await fetchSite(path, {
      method: "POST",
      cookie: browser.cookie,
      headers: { origin: SITE, "content-type": "application/x-www-form-urlencoded", ...PROBE },
      body: `csrf=${other.csrf}`
    });
    expect(wrongField.status).toBe(403);
    // A read needs no token.
    expect((await fetchSite("/store/v1/me/plugins", { cookie: browser.cookie })).status).toBe(200);
  });

  it("never fall back to the cookie when Authorization is there", async () => {
    const browser = await webSignIn(await person());
    const res = await siteChange("/store/v1/plugins/demo/versions", browser, { headers: { authorization: "Basic eDp5" } });
    expect(res.status).toBe(401);
    const runner = await siteChange("/store/v1/plugins/demo/versions", browser, { headers: { authorization: "Bearer runner:user_x@org1:cloud-1" } });
    expect(runner.status).toBe(403);
  });

  it("answer optionalUser: the user on a read, nobody on a change without the token", async () => {
    const userId = await person();
    const browser = await webSignIn(userId);
    expect(await (await fetchSite("/store/v1/public", { cookie: browser.cookie, headers: PROBE })).json()).toEqual({ user: { userId }, credentials: true });
    expect(await (await fetchSite("/store/v1/public", { method: "POST", cookie: browser.cookie, headers: PROBE })).json()).toEqual({ user: null, credentials: true });
    expect(await (await siteChange("/store/v1/public", browser, { headers: PROBE })).json()).toEqual({ user: { userId }, credentials: true });
    expect(await (await fetchSite("/store/v1/public", { headers: PROBE })).json()).toEqual({ user: null, credentials: false });
    expect(
      await (await SELF.fetch(`${SITE}/store/v1/public`, { headers: { authorization: "Bearer runner:user_y@org1:cloud-1", ...PROBE } })).json()
    ).toEqual({ user: null, credentials: true });
    expect(await (await SELF.fetch(`${SITE}/store/v1/public`, { headers: { authorization: `Bearer ${userId}`, ...PROBE } })).json()).toEqual({ user: { userId }, credentials: true });
    // hasCredentials needs no lookup: a dead cookie, or one among others, counts; a look-alike doesn't.
    expect(await (await fetchSite("/store/v1/public", { cookie: sessionHeader("A".repeat(43)), headers: PROBE })).json()).toEqual({ user: null, credentials: true });
    expect(await (await fetchSite("/store/v1/public", { cookie: `theme=dark; ${browser.cookie}`, headers: PROBE })).json()).toMatchObject({ credentials: true });
    expect(await (await fetchSite("/store/v1/public", { cookie: "theme=dark; x__Host-zeron_session=1", headers: PROBE })).json()).toEqual({ user: null, credentials: false });
    expect(await (await fetchSite("/store/v1/public?token=x", { headers: PROBE })).json()).toEqual({ user: null, credentials: true });
  });

  it("admit an admin's session only while the edge says admin", async () => {
    const userId = await person();
    const browser = await webSignIn(userId);
    expect((await fetchSite("/store/v1/admin/queue", { cookie: browser.cookie })).status).toBe(403);
    await grantAdmin(userId);
    const admin = await webSignIn(userId);
    const res = await fetchSite("/store/v1/admin/queue", { cookie: admin.cookie, headers: PROBE });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ admin: { userId } });
    const cookie = sessionHeader(setCookies(res).get("__Host-zeron_session") ?? admin.token);
    // A change needs the token, as everywhere.
    expect((await fetchSite("/store/v1/admin/versions/demo/1.0.0/verify", { method: "POST", cookie, headers: { origin: SITE, ...PROBE } })).status).toBe(403);
    expect((await siteChange("/store/v1/admin/versions/demo/1.0.0/verify", { cookie, csrf: admin.csrf }, { headers: PROBE })).status).toBe(200);
    await revokeAdmin(userId);
    expect((await fetchSite("/store/v1/admin/queue", { cookie })).status).toBe(403);
  });
});

describe("through the site's service binding", () => {
  /** A browser's request to the site, which relays it to the edge. */
  const viaSite = (path: string, init: RequestInit = {}) => env.SITE.fetch(`${SITE}${path}`, { redirect: "manual", ...init });

  it("reads the session at the binding's own address", async () => {
    const userId = await person();
    const browser = await webSignIn(userId);
    const res = await viaSite("/auth/web/session", { headers: { cookie: browser.cookie } });
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({ csrf: browser.csrf, roles: [] });
    expect((await viaSite("/auth/web/session")).status).toBe(401);
    // What sets cookies stays on the web origin.
    expect((await viaSite("/auth/web/login")).status).toBe(404);
    expect((await viaSite("/auth/web/logout", { method: "POST", headers: { cookie: browser.cookie, origin: SITE, "x-csrf-token": browser.csrf } })).status).toBe(404);
  });

  it("takes a change only with the browser's own Origin and token, forwarded", async () => {
    const userId = await person();
    const browser = await webSignIn(userId);
    const other = await webSignIn(await person());
    const path = "/store/v1/plugins/demo/versions";
    const ok = await viaSite(path, { method: "POST", headers: { cookie: browser.cookie, origin: SITE, "x-csrf-token": browser.csrf, ...PROBE }, body: "pkg" });
    expect(ok.status).toBe(200);
    expect(await ok.json()).toMatchObject({ publisher: { userId, via: "session" }, body: await sha256Hex(new TextEncoder().encode("pkg")) });
    const form = await viaSite(path, {
      method: "POST",
      headers: { cookie: browser.cookie, origin: SITE, "content-type": "application/x-www-form-urlencoded", ...PROBE },
      body: `csrf=${browser.csrf}`
    });
    expect(form.status).toBe(200);
    const refused: [string, Record<string, string>][] = [
      // Another site's page posting to the site: its Origin is forwarded.
      ["foreign Origin", { cookie: browser.cookie, origin: "https://evil.example", "x-csrf-token": browser.csrf }],
      // The binding is no proof of origin: no Origin forwarded, no change.
      ["no Origin", { cookie: browser.cookie, "x-csrf-token": browser.csrf }],
      ["no token", { cookie: browser.cookie, origin: SITE }],
      ["another session's token", { cookie: browser.cookie, origin: SITE, "x-csrf-token": other.csrf }]
    ];
    for (const [what, headers] of refused) {
      const res = await viaSite(path, { method: "POST", headers, body: "pkg" });
      expect(res.status, what).toBe(403);
    }
  });

  it("carries a rotated staff cookie back to the browser", async () => {
    const userId = await person();
    await grantAdmin(userId);
    const browser = await webSignIn(userId);
    const res = await viaSite("/store/v1/admin/queue", { headers: { cookie: browser.cookie } });
    expect(res.status).toBe(200);
    expect(setCookies(res).get("__Host-zeron_session")).toMatch(/^[A-Za-z0-9_-]{43}$/);
  });
});

describe("web sign-in without PKCE (WEB_SIGNIN_PKCE=off, the fallback)", () => {
  const offEnv = { ...(env as unknown as Env), WEB_SIGNIN_PKCE: "off" };
  const call = async (path: string, cookie?: string) => {
    const url = new URL(`${SITE}${path}`);
    return (await handleWebAuthRoute(new Request(url, { headers: cookie ? { cookie } : {}, redirect: "manual" }), offEnv, url))!;
  };

  it("sends no challenge and no verifier, and the state cookie still binds the sign-in to the browser", async () => {
    expect(usesPkce(offEnv)).toBe(false);
    expect(usesPkce(env as unknown as Env)).toBe(true);
    const userId = await person();
    const login = await call("/auth/web/login?return_to=/plugins");
    expect(login.status).toBe(302);
    const authorize = new URL(login.headers.get("location")!);
    expect(authorize.searchParams.has("code_challenge")).toBe(false);
    expect(authorize.searchParams.has("code_challenge_method")).toBe(false);
    expect(authorize.searchParams.get("max_age")).toBe("0");
    const loginCookie = `__Host-zeron_web_login=${setCookies(login).get("__Host-zeron_web_login")}`;
    const state = authorize.searchParams.get("state")!;

    // Someone else's state in this browser: refused, as with PKCE.
    const code = newId("code");
    await workos("__codes", { code, user_id: userId });
    const other = await startLogin();
    expect((await call(`/auth/web/callback?code=${code}&state=${state}`, other.loginCookie)).status).toBe(400);
    expect((await call(`/auth/web/callback?code=${code}&state=${state}`)).status).toBe(400);

    // Its own state: signed in, the code sent with the secret and no verifier.
    const res = await call(`/auth/web/callback?code=${code}&state=${state}`, loginCookie);
    expect(res.status).toBe(303);
    expect(res.headers.get("location")).toBe(`${SITE}/plugins`);
    const { authenticated } = (await workos("__state")) as { authenticated: { code: string; code_verifier: string | null; client_secret: string | null }[] };
    expect(authenticated.find((a) => a.code === code)).toEqual({ code, code_verifier: null, client_secret: "sk_test" });
    const token = setCookies(res).get("__Host-zeron_session")!;
    expect((await sessionOf(sessionHeader(token))).status).toBe(200);
  });
});
