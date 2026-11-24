/**
 * Web sign-in on the web site (`WEB_ORIGIN`): WorkOS AuthKit's
 * authorization-code flow with PKCE, ending in a web session
 * (web-session.ts). Served only on the web origin, where the cookies live.
 *
 *   GET  /auth/web/login?return_to=/path → 302 to AuthKit
 *   GET  /auth/web/callback?code&state   → 303 to return_to, signed in
 *   POST /auth/web/logout                → signed out (Origin + CSRF: the
 *                                          X-CSRF-Token header or a `csrf` form
 *                                          field; 303 to / for a form post,
 *                                          else {ok: true})
 *   GET  /auth/web/session               → { user: {name, email, avatar?},
 *                                          username, roles, csrf, expires_at,
 *                                          fresh_sign_in } or 401 signed_out
 *
 * Cross-site checks (web-session.ts) read the request's `Origin` whichever
 * way it arrives: straight from the browser (`POST /auth/web/logout`, a
 * plain form post), or forwarded by the site over its service binding, which
 * copies the browser's Cookie, Origin and X-CSRF-Token (or form body)
 * verbatim. The binding itself proves nothing (the site relays any
 * browser's request), so a forwarded change without the browser's Origin is
 * refused like any other.
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
 * sign-in allows (making a deploy token) works from the web for
 * `STEP_UP_TTL_MS` after it. Signing out ends the session and its WorkOS
 * session.
 */
import { decodeJwt } from "jose";
import { escapeHtml } from "./auth-routes";
import { constantTimeEqual } from "./cloud/crypto";
import type { Env } from "./env";
import { MAX_LIVE_STEP_UPS, STEP_UP_TTL_MS, newStepUpToken, stepUpHash } from "./step-up";
import { WEB_SESSION_ABSOLUTE_MS, userDirectoryStub } from "./user-directory";
import {
  LOGIN_COOKIE,
  LOGIN_TTL_MS,
  SESSION_COOKIE,
  clearSessionCookie,
  codeChallenge,
  crossSiteRefusal,
  hostCookie,
  loginCookieValue,
  newToken,
  parseLoginCookie,
  readCookie,
  safeReturnTo,
  sendsCookie,
  sessionCookie,
  sessionTokenHash,
  webOrigin
} from "./web-cookies";
import { MAX_FORM_BYTES, requestCsrf, resolveWebSession, withSessionCookie } from "./web-session";
import { WorkOsAuthFailed, authorizeUrl, exchange, getUser, revokeSession } from "./workos";

export const CALLBACK_PATH = "/auth/web/callback";

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
  if (!origin) return jsonAnswer({ error: "not_found" }, 404);
  const route = url.pathname.slice("/auth/web".length);
  // Reading the session sets no cookie but a rotated one, so the site may
  // ask over its service binding at any address. Everything else sets this
  // host's cookies: only on the web origin.
  if (route !== "/session" && url.origin !== origin) return jsonAnswer({ error: "not_found" }, 404);
  if (route === "/login") return request.method === "GET" ? login(env, url, origin) : notAllowed("GET");
  if (route === "/callback") return request.method === "GET" ? callback(request, env, url, origin) : notAllowed("GET");
  if (route === "/logout") return request.method === "POST" ? logout(request, env, origin) : notAllowed("POST");
  if (route === "/session") return request.method === "GET" ? session(request, env, url) : notAllowed("GET");
  return jsonAnswer({ error: "not_found" }, 404);
};

const notAllowed = (allow: string): Response =>
  new Response(JSON.stringify({ error: "method_not_allowed" }), { status: 405, headers: { "content-type": "application/json", allow } });

/** Whether web sign-in uses PKCE (the default). `WEB_SIGNIN_PKCE=off` is
 * the fallback for a WorkOS that refuses a verifier sent beside the client
 * secret: the attempt's `state` cookie still binds the callback to the
 * browser that began it (no login CSRF), but a code stolen in transit could
 * then be redeemed by whoever holds it, so turn it off only if that refusal
 * is real, and only until it's resolved. */
export const usesPkce = (env: Pick<Env, "WEB_SIGNIN_PKCE">): boolean => env.WEB_SIGNIN_PKCE !== "off";

const login = async (env: Env, url: URL, origin: string): Promise<Response> => {
  if (!env.WORKOS_API_KEY) return jsonAnswer({ error: "workos not configured" }, 501);
  const returnTo = safeReturnTo(url.searchParams.get("return_to"), origin);
  const state = newToken();
  const verifier = newToken();
  const location = authorizeUrl(env, {
    redirectUri: `${origin}${CALLBACK_PATH}`,
    state,
    codeChallenge: usesPkce(env) ? await codeChallenge(verifier) : null
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
    exchanged = await exchange(env, apiKey, code, usesPkce(env) ? attempt.verifier : undefined);
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

const logout = async (request: Request, env: Env, origin: string): Promise<Response> => {
  // (Nothing else reads a logout's body: a small form at most.)
  const { csrf, form } = await requestCsrf(request, MAX_FORM_BYTES);
  const live = await resolveWebSession(request, env);
  const refusal = crossSiteRefusal(request, env, live?.csrf ?? "", csrf);
  // Without a live session there's nothing to protect but the request's
  // origin: the cookie is cleared either way.
  // One word for a refused cookie write, as on every store route (amendment 35).
  if (refusal === "bad_origin" || (live && refusal)) {
    const message = refusal === "bad_origin" ? "This request didn't come from the Zeron site." : "This page is out of date. Reload it and try again.";
    return jsonAnswer({ error: "bad_csrf", message }, 403);
  }
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
    const stale = sendsCookie(request, SESSION_COOKIE) ? [clearSessionCookie()] : [];
    return jsonAnswer({ error: "signed_out" }, 401, stale);
  }
  let profile: Awaited<ReturnType<NonNullable<ReturnType<typeof userDirectoryStub>>["profiles"]>>[string] | undefined;
  try {
    profile = (await userDirectoryStub(env)?.profiles([live.userId]))?.[live.userId];
  } catch (e) {
    console.warn("auth/web: directory read failed", e instanceof Error ? e.name : "error");
  }
  const edgeOrigin = env.CLOUD_EDGE_URL ?? url.origin;
  const avatar = profile?.photoKey ? `${edgeOrigin}/auth/avatars/${profile.photoKey}` : (profile?.profilePictureUrl ?? null);
  return withSessionCookie(
    request,
    jsonAnswer({
      user: {
        name: [profile?.firstName, profile?.lastName].filter(Boolean).join(" ") || null,
        email: profile?.email ?? null,
        ...(avatar ? { avatar } : {})
      },
      username: live.username,
      roles: live.roles,
      csrf: live.csrf,
      expires_at: live.expiresAt,
      fresh_sign_in: live.stepUpHash !== null
    })
  );
};
