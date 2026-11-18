/**
 * Web sign-in on the web site (`WEB_ORIGIN`): WorkOS AuthKit's
 * authorization-code flow with PKCE, ending in a web session
 * (web-session.ts). Served only on the web origin, where the cookies live.
 *
 *   GET  /auth/web/login?return_to=/path → 302 to AuthKit
 *   GET  /auth/web/callback?code&state   → 303 to return_to, signed in
 *   POST /auth/web/logout                → signed out (Origin + CSRF; 303 to
 *                                          / for a form post, else {ok: true})
 *   GET  /auth/web/session               → { user: {name, email, avatar},
 *                                          username, roles, csrf, expiresAt,
 *                                          freshSignIn } or 401
 *
 * Login binds the attempt to this browser: `state`, the PKCE verifier and
 * where to go after are kept in a 10-minute `__Host-` cookie (nothing is
 * stored server-side before a sign-in succeeds). The callback answers only
 * when its `state` matches that cookie, and the code exchange only succeeds
 * with that browser's verifier, so a code from someone else's sign-in can't
 * sign this browser in. `return_to` is a path on the web origin, never
 * anywhere else; anything else goes to `/`.
 *
 * A web sign-in always asks for credentials (`max_age=0`, workos.ts), so it
 * earns a step-up ticket (step-up.ts), kept with the session: what a fresh
 * sign-in allows (registering a deploy key) works from the web for
 * `STEP_UP_TTL_MS` after it. Signing out ends the session and its WorkOS
 * session.
 */
import { decodeJwt } from "jose";
import { escapeHtml } from "./auth-routes";
import { b64urlDecode, b64urlEncode, constantTimeEqual } from "./cloud/crypto";
import type { Env } from "./env";
import { MAX_LIVE_STEP_UPS, STEP_UP_TTL_MS, newStepUpToken, stepUpHash } from "./step-up";
import { WEB_SESSION_ABSOLUTE_MS, userDirectoryStub } from "./user-directory";
import {
  CSRF_HEADER,
  SESSION_COOKIE,
  clearSessionCookie,
  crossSiteRefusal,
  hostCookie,
  newToken,
  readCookie,
  resolveWebSession,
  sessionCookie,
  sessionTokenHash,
  webOrigin,
  withSessionCookie
} from "./web-session";
import { WorkOsAuthFailed, authorizeUrl, exchange, getUser, revokeSession } from "./workos";

export const LOGIN_COOKIE = "__Host-zeron_web_login";
/** How long a sign-in may take from login to callback. */
export const LOGIN_TTL_MS = 10 * 60_000;
export const CALLBACK_PATH = "/auth/web/callback";
const MAX_RETURN_TO = 2048;
const MAX_FORM_BODY = 4096;

/**
 * Where to send the browser after signing in: a path (and query) on the web
 * origin, or `/`. Refuses anything that could leave it (`//host`, `/\host`,
 * a scheme, backslashes, control characters) and the sign-in routes
 * themselves.
 */
export const safeReturnTo = (raw: string | null | undefined, origin: string): string => {
  if (!raw || raw.length > MAX_RETURN_TO || !raw.startsWith("/") || raw.startsWith("//")) return "/";
  if (/[\\\u0000-\u001f\u007f]/.test(raw)) return "/";
  let url: URL;
  try {
    url = new URL(raw, origin);
  } catch {
    return "/";
  }
  if (url.origin !== origin) return "/";
  if (url.pathname === "/auth/web" || url.pathname.startsWith("/auth/web/")) return "/";
  return `${url.pathname}${url.search}`;
};

const TOKEN_RE = /^[A-Za-z0-9_-]{43}$/;

/** The login cookie: `state.verifier.b64url(returnTo)`. */
export const loginCookieValue = (state: string, verifier: string, returnTo: string): string =>
  `${state}.${verifier}.${b64urlEncode(new TextEncoder().encode(returnTo))}`;

export const parseLoginCookie = (value: string | undefined): { state: string; verifier: string; returnTo: string } | undefined => {
  const [state, verifier, encoded, extra] = value?.split(".") ?? [];
  if (extra !== undefined || !state || !verifier || encoded === undefined) return undefined;
  if (!TOKEN_RE.test(state) || !TOKEN_RE.test(verifier)) return undefined;
  const bytes = b64urlDecode(encoded);
  if (!bytes) return undefined;
  try {
    return { state, verifier, returnTo: new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes) };
  } catch {
    return undefined;
  }
};

/** PKCE S256: b64url(SHA-256(verifier)). */
export const codeChallenge = async (verifier: string): Promise<string> =>
  b64urlEncode(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier))));

const NO_STORE = { "cache-control": "no-store", vary: "Cookie" };

const jsonAnswer = (value: unknown, status = 200, cookies: string[] = []): Response => {
  const headers = new Headers({ "content-type": "application/json", ...NO_STORE });
  for (const cookie of cookies) headers.append("set-cookie", cookie);
  return new Response(JSON.stringify(value), { status, headers });
};

const page = (title: string, body: string): string => `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="robots" content="noindex" />
<title>Zeron — ${escapeHtml(title)}</title>
<style>
  body { margin: 0; min-height: 100vh; display: grid; place-items: center;
         background: #0a0a0a; color: #ededed;
         font: 15px/1.6 ui-sans-serif, system-ui, sans-serif; }
  main { max-width: 34rem; padding: 2rem; text-align: center; }
  h1 { font-size: 1.05rem; font-weight: 600; margin: 0 0 0.75rem; }
  p { color: #a1a1a1; margin: 0.25rem 0; }
  a { color: #ededed; }
</style>
</head>
<body><main><h1>${escapeHtml(title)}</h1>${body}</main></body>
</html>`;

/** The sign-in didn't finish: say so, offer to start again, and drop the
 * attempt's cookie. */
const signInFailed = (detail: string, status = 400): Response =>
  new Response(page("Sign-in didn't finish", `<p>${escapeHtml(detail)}</p><p><a href="/auth/web/login">Sign in again</a></p>`), {
    status,
    headers: {
      "content-type": "text/html; charset=utf-8",
      "set-cookie": hostCookie(LOGIN_COOKIE, "", 0),
      "referrer-policy": "no-referrer",
      ...NO_STORE
    }
  });

/** Handle an `/auth/web/*` route; undefined when it isn't one. */
export const handleWebAuthRoute = async (request: Request, env: Env, url: URL): Promise<Response | undefined> => {
  if (url.pathname !== "/auth/web" && !url.pathname.startsWith("/auth/web/")) return undefined;
  const origin = webOrigin(env);
  // Only on the web origin: the cookies set here belong to its host.
  if (!origin || url.origin !== origin) return jsonAnswer({ error: "not_found" }, 404);
  const route = url.pathname.slice("/auth/web".length);
  if (route === "/login") return request.method === "GET" ? login(env, url, origin) : notAllowed("GET");
  if (route === "/callback") return request.method === "GET" ? callback(request, env, url, origin) : notAllowed("GET");
  if (route === "/logout") return request.method === "POST" ? logout(request, env, origin) : notAllowed("POST");
  if (route === "/session") return request.method === "GET" ? session(request, env, url) : notAllowed("GET");
  return jsonAnswer({ error: "not_found" }, 404);
};

const notAllowed = (allow: string): Response =>
  new Response(JSON.stringify({ error: "method_not_allowed" }), { status: 405, headers: { "content-type": "application/json", allow } });

const login = async (env: Env, url: URL, origin: string): Promise<Response> => {
  if (!env.WORKOS_API_KEY) return jsonAnswer({ error: "workos not configured" }, 501);
  const returnTo = safeReturnTo(url.searchParams.get("return_to"), origin);
  const state = newToken();
  const verifier = newToken();
  const location = authorizeUrl(env, {
    redirectUri: `${origin}${CALLBACK_PATH}`,
    state,
    codeChallenge: await codeChallenge(verifier)
  });
  return new Response(null, {
    status: 302,
    headers: {
      location,
      "set-cookie": hostCookie(LOGIN_COOKIE, loginCookieValue(state, verifier, returnTo), LOGIN_TTL_MS),
      "referrer-policy": "no-referrer",
      ...NO_STORE
    }
  });
};

/** The WorkOS session an access token belongs to (`sid`), when it's this
 * user's. The token came straight from WorkOS over TLS in answer to the
 * edge's own authenticated call. */
const workosSessionOf = (accessToken: string, userId: string): string | null => {
  try {
    const claims = decodeJwt(accessToken);
    return claims.sub === userId && typeof claims.sid === "string" && claims.sid ? claims.sid : null;
  } catch {
    return null;
  }
};

const callback = async (request: Request, env: Env, url: URL, origin: string): Promise<Response> => {
  const apiKey = env.WORKOS_API_KEY;
  if (!apiKey) return jsonAnswer({ error: "workos not configured" }, 501);
  const attempt = parseLoginCookie(readCookie(request, LOGIN_COOKIE));
  const state = url.searchParams.get("state");
  const code = url.searchParams.get("code");
  if (!attempt) return signInFailed("This sign-in expired or began in another browser.");
  if (!state || !constantTimeEqual(state, attempt.state)) return signInFailed("This sign-in began in another window or browser.");
  if (url.searchParams.get("error")) return signInFailed("Sign-in was cancelled or refused.");
  if (!code) return signInFailed("This link is missing its sign-in code.");
  let exchanged: Awaited<ReturnType<typeof exchange>>;
  try {
    exchanged = await exchange(env, apiKey, code, attempt.verifier);
  } catch (e) {
    console.warn("auth/web: code exchange failed", e instanceof WorkOsAuthFailed ? (e.status ?? "no status") : "error");
    return signInFailed("The sign-in code was refused. It may have been used already.");
  }
  const userId = exchanged.user.id;
  const directory = userDirectoryStub(env);
  if (!directory) return jsonAnswer({ error: "unavailable" }, 503);
  // The directory may not have them yet (their webhook hasn't landed).
  try {
    if (!(await directory.profiles([userId]))[userId]) await directory.upsert(await getUser(apiKey, userId));
  } catch (e) {
    console.warn("auth/web: recording the user failed", e instanceof Error ? e.name : "error");
  }
  // A real sign-in (max_age=0): it earns a step-up ticket, kept with the session.
  const ticket = newStepUpToken();
  const ticketHash = (await stepUpHash(ticket))!;
  await directory.issueStepUp(userId, ticketHash, STEP_UP_TTL_MS, MAX_LIVE_STEP_UPS);
  const token = newToken();
  const created = await directory.createWebSession({
    userId,
    tokenHash: (await sessionTokenHash(token))!,
    csrf: newToken(),
    stepUpHash: ticketHash,
    workosSessionId: workosSessionOf(exchanged.accessToken, userId),
    replacing: (await sessionTokenHash(readCookie(request, SESSION_COOKIE))) ?? null
  });
  if (created === "deleted_user") return signInFailed("This account has been deleted.", 403);
  const headers = new Headers({ location: `${origin}${safeReturnTo(attempt.returnTo, origin)}`, "referrer-policy": "no-referrer", ...NO_STORE });
  // As long as a session can last: the server ends it sooner for staff, or idle.
  headers.append("set-cookie", sessionCookie(token, Date.now() + WEB_SESSION_ABSOLUTE_MS));
  headers.append("set-cookie", hostCookie(LOGIN_COOKIE, "", 0));
  return new Response(null, { status: 303, headers });
};

/** The CSRF token a logout carries: the header, or a form post's `csrf`. */
const logoutCsrf = async (request: Request): Promise<{ csrf: string | null; form: boolean }> => {
  const header = request.headers.get(CSRF_HEADER);
  const type = request.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase();
  if (type !== "application/x-www-form-urlencoded") return { csrf: header, form: false };
  const declared = Number(request.headers.get("content-length") ?? "0");
  if (declared > MAX_FORM_BODY) return { csrf: header, form: true };
  const text = await request.text().catch(() => "");
  if (text.length > MAX_FORM_BODY) return { csrf: header, form: true };
  return { csrf: header ?? new URLSearchParams(text).get("csrf"), form: true };
};

const logout = async (request: Request, env: Env, origin: string): Promise<Response> => {
  const { csrf, form } = await logoutCsrf(request);
  const live = await resolveWebSession(request, env);
  const refusal = crossSiteRefusal(request, env, live?.csrf ?? "", csrf);
  // Without a live session there's nothing to protect but the request's
  // origin: the cookie is cleared either way.
  if (refusal === "bad_origin" || (live && refusal)) return jsonAnswer({ error: "forbidden", reason: refusal }, 403);
  if (live) {
    const ended = await userDirectoryStub(env)?.endWebSession((await sessionTokenHash(live.token))!);
    if (ended?.workosSessionId && env.WORKOS_API_KEY) {
      await revokeSession(env.WORKOS_API_KEY, ended.workosSessionId).catch((e: unknown) =>
        console.warn("auth/web: ending the WorkOS session failed", e instanceof WorkOsAuthFailed ? (e.status ?? "no status") : "error")
      );
    }
  }
  if (form) {
    return new Response(null, { status: 303, headers: { location: `${origin}/`, "set-cookie": clearSessionCookie(), ...NO_STORE } });
  }
  return jsonAnswer({ ok: true }, 200, [clearSessionCookie()]);
};

const session = async (request: Request, env: Env, url: URL): Promise<Response> => {
  const live = await resolveWebSession(request, env);
  if (!live) {
    // A cookie that names no live session goes.
    const stale = readCookie(request, SESSION_COOKIE) !== undefined ? [clearSessionCookie()] : [];
    return jsonAnswer({ error: "unauthenticated" }, 401, stale);
  }
  let profile: Awaited<ReturnType<NonNullable<ReturnType<typeof userDirectoryStub>>["profiles"]>>[string] | undefined;
  try {
    profile = (await userDirectoryStub(env)?.profiles([live.userId]))?.[live.userId];
  } catch (e) {
    console.warn("auth/web: directory read failed", e instanceof Error ? e.name : "error");
  }
  const edgeOrigin = env.CLOUD_EDGE_URL ?? url.origin;
  return withSessionCookie(
    request,
    jsonAnswer({
      user: {
        name: [profile?.firstName, profile?.lastName].filter(Boolean).join(" ") || null,
        email: profile?.email ?? null,
        avatar: profile?.photoKey ? `${edgeOrigin}/auth/avatars/${profile.photoKey}` : (profile?.profilePictureUrl ?? null)
      },
      username: live.username,
      roles: live.roles,
      csrf: live.csrf,
      expiresAt: live.expiresAt,
      freshSignIn: live.stepUpHash !== null
    })
  );
};
