/**
 * Helpers for the identity tests (web sign-in, roles, deploy keys): people
 * in WorkOS (workos-fake.ts) and the directory, browsers that sign in on
 * the web site, and ed25519 signers.
 */
import { SELF, env } from "cloudflare:test";
import { expect } from "vitest";
import { b64urlEncode } from "../../src/cloud/crypto";
import { MAX_LIVE_STEP_UPS, STEP_UP_TTL_MS, newStepUpToken, stepUpHash } from "../../src/step-up";

export const SITE = "https://site.test";
export const ADMIN = { authorization: "Bearer test-admin-token", "content-type": "application/json" };

export const directory = () => env.USER_DIRECTORY!.get(env.USER_DIRECTORY!.idFromName("users1"));
export const newId = (prefix: string) => `${prefix}_${crypto.randomUUID().replaceAll("-", "").slice(0, 16)}`;

export const workos = (path: string, body?: unknown) =>
  fetch(`https://api.workos.com/${path}`, body === undefined ? undefined : { method: "POST", body: JSON.stringify(body) }).then(
    (r) => r.json() as Promise<Record<string, unknown>>
  );

/** A WorkOS user, in the directory too, optionally with a username. */
export const person = async (username?: string): Promise<string> => {
  const userId = newId("user");
  const seeded = (await workos("__users", { id: userId, email: `${newId("p")}@example.net`, first_name: "Ada", last_name: "Lovelace" })) as Record<string, string>;
  await directory().upsert({ ...(seeded as never), profile_picture_url: null, created_at: seeded.updated_at });
  if (username) {
    const res = await SELF.fetch("https://edge.test/username", {
      method: "POST",
      headers: { authorization: `Bearer ${userId}`, "content-type": "application/json" },
      body: JSON.stringify({ username })
    });
    expect(res.status).toBe(200);
  }
  return userId;
};

/** `name=value` of each Set-Cookie on a response. */
export const setCookies = (res: Response): Map<string, string> =>
  new Map(res.headers.getSetCookie().map((c) => {
    const pair = c.split(";")[0]!;
    const eq = pair.indexOf("=");
    return [pair.slice(0, eq), pair.slice(eq + 1)];
  }));

export const fetchSite = (path: string, init: RequestInit & { cookie?: string } = {}) => {
  const { cookie, headers, ...rest } = init;
  return SELF.fetch(`${SITE}${path}`, {
    redirect: "manual",
    ...rest,
    headers: { ...(cookie ? { cookie } : {}), ...(headers as Record<string, string> | undefined) }
  });
};

export interface Browser {
  /** The session cookie's value. */
  token: string;
  /** `Cookie` header carrying it. */
  cookie: string;
  csrf: string;
  /** Where the callback sent the browser. */
  location: string;
}

export const sessionHeader = (token: string) => `__Host-zeron_session=${token}`;

/** Start a sign-in: the login answer, its AuthKit URL and the attempt's cookie. */
export const startLogin = async (returnTo?: string) => {
  const res = await fetchSite(`/auth/web/login${returnTo === undefined ? "" : `?return_to=${encodeURIComponent(returnTo)}`}`);
  expect(res.status).toBe(302);
  const authorize = new URL(res.headers.get("location")!);
  const attempt = setCookies(res).get("__Host-zeron_web_login")!;
  return { res, authorize, loginCookie: `__Host-zeron_web_login=${attempt}` };
};

/** Sign `userId` in on the web site, as AuthKit would send the browser back. */
export const webSignIn = async (userId: string, opts: { returnTo?: string; sid?: string; cookie?: string } = {}): Promise<Browser> => {
  const { authorize, loginCookie } = await startLogin(opts.returnTo);
  const code = newId("code");
  await workos("__codes", { code, user_id: userId, code_challenge: authorize.searchParams.get("code_challenge"), sid: opts.sid });
  const res = await fetchSite(`/auth/web/callback?code=${code}&state=${authorize.searchParams.get("state")}`, {
    cookie: [loginCookie, opts.cookie].filter(Boolean).join("; ")
  });
  expect(res.status).toBe(303);
  const token = setCookies(res).get("__Host-zeron_session")!;
  expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
  const cookie = sessionHeader(token);
  const session = await fetchSite("/auth/web/session", { cookie });
  expect(session.status).toBe(200);
  const { csrf } = (await session.json()) as { csrf: string };
  return { token, cookie, csrf, location: res.headers.get("location")! };
};

/** A cookie-authenticated change, as the web site's own page makes it. */
export const siteChange = (path: string, browser: Pick<Browser, "cookie" | "csrf">, init: RequestInit = {}, method = "POST") =>
  fetchSite(path, {
    method,
    ...init,
    cookie: browser.cookie,
    headers: { origin: SITE, "x-csrf-token": browser.csrf, ...(init.headers as Record<string, string> | undefined) }
  });

export const grantAdmin = async (userId: string) => {
  const res = await SELF.fetch("https://edge.test/admin/roles/grant", {
    method: "POST",
    headers: ADMIN,
    body: JSON.stringify({ userId, role: "admin", by: "tests" })
  });
  expect(res.status).toBe(200);
};

export const revokeAdmin = async (userId: string) => {
  const res = await SELF.fetch("https://edge.test/admin/roles/revoke", {
    method: "POST",
    headers: ADMIN,
    body: JSON.stringify({ userId, role: "admin", by: "tests" })
  });
  expect(res.status).toBe(200);
};

/** A fresh sign-in's step-up ticket, as `/auth/exchange` mints. */
export const stepUp = async (userId: string): Promise<string> => {
  const token = newStepUpToken();
  await directory().issueStepUp(userId, (await stepUpHash(token))!, STEP_UP_TTL_MS, MAX_LIVE_STEP_UPS);
  return token;
};

export interface Signer {
  readonly key: string;
  sign(message: string): Promise<string>;
}

export const newSigner = async (): Promise<Signer> => {
  const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const key = b64urlEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  return {
    key,
    sign: async (m) => b64urlEncode(new Uint8Array(await crypto.subtle.sign({ name: "Ed25519" }, pair.privateKey, new TextEncoder().encode(m))))
  };
};

export const setOwner = (pluginId: string, userId: string | null) =>
  SELF.fetch("https://edge.test/__test/owner", { method: "POST", body: JSON.stringify({ pluginId, userId }) });

export const errorOf = async (res: Response) => ((await res.json()) as { error: string }).error;
