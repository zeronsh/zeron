import { env as testEnv } from "cloudflare:test";
import { afterEach, describe, expect, it, vi } from "vitest";
import { exportJWK, generateKeyPair, SignJWT } from "jose";
import { browserDeviceRoute, handleBrowserRoute } from "../../src/browser-routes";
import { registerBrowserDevice, SESSION_COOKIE, tokenHash } from "../../src/browser-sessions";
import { AUTH_USER_HEADER, type Env } from "../../src/env";

const env = testEnv as Env;

// Explicit production inputs: no single-user subject and no development login.
const config = (): Env => ({
  BROWSER_SESSIONS: env.BROWSER_SESSIONS,
  DEVICE_ROOMS: env.DEVICE_ROOMS,
  AUTH_MODE: "workos",
  WORKOS_BROWSER_ORIGIN: "https://test",
  WORKOS_CLIENT_ID: "client_test",
  WORKOS_API_KEY: "test-only",
  WORKOS_ISSUER: "https://multi-user.test",
  WORKOS_JWKS_URL: `https://multi-user.test/${crypto.randomUUID()}/jwks`,
  BROWSER_SESSION_KEY: "workerd-test-session-key"
}) as Env;

const call = async (bindings: Env, path: string, cookie = "", method = "GET", csrf = "") => {
  const url = new URL(`https://test/api/browser/${path}`);
  return (await handleBrowserRoute(new Request(url, {
    method, headers: { cookie, origin: "https://test", "x-csrf-token": csrf }
  }), bindings, url))!;
};

afterEach(() => vi.unstubAllGlobals());

describe("multi-user browser authentication", () => {
  it("accepts two verified WorkOS users while isolating sessions, revocation, and device connections", async () => {
    const bindings = config();
    const pair = await generateKeyPair("RS256");
    const jwk = await exportJWK(pair.publicKey);
    const replies: unknown[] = [];
    let challenge = "";
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : input.toString();
      if (url === bindings.WORKOS_JWKS_URL) return Response.json({ keys: [{ ...jwk, kid: "multi-user" }] });
      expect(url).toBe("https://api.workos.com/user_management/authenticate");
      const value = JSON.parse(init!.body as string) as { code_verifier: string; grant_type: string };
      expect(value.grant_type).toBe("authorization_code");
      expect(value.code_verifier).toMatch(/^[A-Za-z0-9._~-]{43,128}$/);
      const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(value.code_verifier)));
      expect(btoa(String.fromCharCode(...digest)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "")).toBe(challenge);
      expect(replies.length).toBeGreaterThan(0);
      return Response.json(replies.shift());
    }));

    const login = async (owner: string, responseOwner = owner) => {
      const started = await call(bindings, "login", "", "POST");
      expect(started.status).toBe(200);
      const authorization = new URL((await started.json() as { authorizationUrl: string }).authorizationUrl);
      expect(authorization.searchParams.get("code_challenge_method")).toBe("S256");
      challenge = authorization.searchParams.get("code_challenge")!;
      const transactionCookie = started.headers.get("set-cookie")!.split(";")[0]!;
      const jwt = await new SignJWT({ sid: `sid-${owner}`, auth_time: Math.floor(Date.now() / 1000) })
        .setProtectedHeader({ alg: "RS256", kid: "multi-user" }).setSubject(owner)
        .setIssuer(bindings.WORKOS_ISSUER!).setExpirationTime("1h").sign(pair.privateKey);
      replies.push({ user: { id: responseOwner }, access_token: jwt, refresh_token: `refresh-${owner}` });
      const path = `callback?code=code&state=${authorization.searchParams.get("state")}`;
      const response = await call(bindings, path, transactionCookie);
      expect((await call(bindings, path, transactionCookie)).status).toBe(401);
      return response;
    };

    const users = [];
    for (const owner of [`alice-${crypto.randomUUID()}`, `bob-${crypto.randomUUID()}`]) {
      const response = await login(owner);
      expect(response.status).toBe(303);
      const setCookie = response.headers.get("set-cookie")!;
      expect(setCookie).toContain("Secure; HttpOnly; Path=/; SameSite=Strict");
      const cookie = `${SESSION_COOKIE}=${setCookie.match(/__Host-comet_session=([^;]+)/)![1]}`;
      const session = await (await call(bindings, "session", cookie)).json() as { ownerId: string; csrfToken: string };
      expect(session).toMatchObject({ authenticated: true, ownerId: owner });
      users.push({ owner, cookie, csrf: session.csrfToken, hash: await tokenHash(cookie.slice(cookie.indexOf("=") + 1)), device: `device-${crypto.randomUUID()}` });
    }
    expect((await login("signed-user", "different-response-user")).status).toBe(403);

    for (const user of users) {
      const room = env.DEVICE_ROOMS.get(env.DEVICE_ROOMS.idFromName(`d2/${user.device}`));
      const host = await room.fetch(new Request("https://device/ws?role=host&connId=host", { headers: { upgrade: "websocket", [AUTH_USER_HEADER]: user.owner } }));
      expect(host.status).toBe(101);
      host.webSocket!.accept();
      await registerBrowserDevice(bindings, user.owner, user.device);
    }
    for (const [index, user] of users.entries()) {
      const other = users[1 - index]!;
      const sessions = await (await call(bindings, `sessions?ownerId=${other.owner}`, user.cookie)).json() as { sessions: { hash: string }[] };
      expect(sessions.sessions.map(({ hash }) => hash)).toEqual([user.hash]);
      expect(await (await call(bindings, `devices?ownerId=${other.owner}`, user.cookie)).json()).toEqual({ devices: [{ id: user.device, online: true }] });
      expect((await call(bindings, `sessions/${other.hash}/revoke`, user.cookie, "POST", user.csrf)).status).toBe(200);
      expect(await (await call(bindings, "session", other.cookie)).json()).toMatchObject({ authenticated: true, ownerId: other.owner });
      expect((await call(bindings, "revoke-all", user.cookie, "POST", other.csrf)).status).toBe(401);
      for (const target of [user, other]) {
        const url = new URL(`https://test/api/browser/device/${target.device}/ws`);
        const response = (await browserDeviceRoute(new Request(url, { headers: {
          upgrade: "websocket", origin: "https://test", cookie: user.cookie,
          [AUTH_USER_HEADER]: target.owner
        } }), bindings, url))!;
        expect(response.status).toBe(target === user ? 101 : 403);
        if (response.webSocket) { response.webSocket.accept(); response.webSocket.close(); }
      }
    }
    const [alice, bob] = users;
    expect((await call(bindings, "revoke-all", alice!.cookie, "POST", alice!.csrf)).status).toBe(200);
    expect(await (await call(bindings, "session", alice!.cookie)).json()).toEqual({ authenticated: false });
    expect(await (await call(bindings, "session", bob!.cookie)).json()).toMatchObject({ authenticated: true, ownerId: bob!.owner });
    expect(replies).toEqual([]);
  });
});
