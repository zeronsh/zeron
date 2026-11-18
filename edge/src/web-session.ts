/**
 * Web sessions: a browser signed in on the web site (`WEB_ORIGIN`, zeron.sh)
 * through `/auth/web/*` (web-auth-routes.ts). The device and session API
 * never reads a cookie: only `/auth/web/*` and the store's publisher and
 * admin routes (store/principal.ts) do.
 *
 * - The cookie, `__Host-zeron_session`: HttpOnly, Secure, SameSite=Lax,
 *   Path=/, host-only (the `__Host-` prefix makes the browser refuse one
 *   set by a subdomain or for another path). Its value is 32 random bytes,
 *   b64url; the directory keeps only its SHA-256 (`web_sessions` in
 *   user-directory.ts), so the server can revoke it at any time: signing
 *   out, deleting the account, or the lifetimes below.
 * - Lifetimes: 7 days idle, 30 days at most; for an account holding a role
 *   (roles.ts), 30 minutes idle and 12 hours at most, decided on every read.
 * - Rotation: a staff session's token is replaced on its first privileged
 *   use and every 10 minutes of privileged use. The new cookie goes out on
 *   that response (`withSessionCookie`); the old token works 60 seconds more,
 *   for requests already in flight.
 * - CSRF: every state-changing request a cookie authenticates must come
 *   from the web origin (`Origin` equal to `WEB_ORIGIN`; `Sec-Fetch-Site`,
 *   when sent, `same-origin`) and carry the session's CSRF token in
 *   `X-CSRF-Token` (or a form post's `csrf` field). The token is random per session, kept server-side and
 *   stays the same across rotation; one session's token is worthless with
 *   another's cookie.
 */
import type { Env } from "./env";
import { userDirectoryStub, type WebSessionRead } from "./user-directory";
import { readBody } from "./store/identity/body";
import { CSRF_HEADER, SESSION_COOKIE, newToken, readCookie, sessionCookie, sessionTokenHash, webOrigin } from "./web-cookies";

/** A live session, resolved for one request. */
export interface ResolvedSession extends WebSessionRead {
  /** The token this request presented (or its successor, once rotated). */
  readonly token: string;
}

/** Cookies a request's answer must set (a rotated session token). */
const pendingCookies = new WeakMap<Request, string[]>();

/**
 * The live web session the request's cookie names, or undefined: no cookie,
 * web sign-in not set up, or no such live session. `privileged`: this use
 * exercises a role, which rotates a staff session's token when it's due; the
 * new cookie is queued for `withSessionCookie`.
 */
export const resolveWebSession = async (
  request: Request,
  env: Env,
  privileged = false
): Promise<ResolvedSession | undefined> => {
  if (!webOrigin(env)) return undefined;
  const token = readCookie(request, SESSION_COOKIE);
  const hash = await sessionTokenHash(token);
  const directory = userDirectoryStub(env);
  if (!hash || !directory) return undefined;
  const successor = privileged ? newToken() : undefined;
  const session = await directory.webSession(hash, successor && (await sessionTokenHash(successor)));
  if (!session) return undefined;
  if (session.rotated && successor) {
    const cookies = pendingCookies.get(request) ?? [];
    cookies.push(sessionCookie(successor, session.absoluteExpiresAt));
    pendingCookies.set(request, cookies);
    return { ...session, token: successor };
  }
  return { ...session, token: token! };
};

/** `response`, with any cookie resolving the request's session queued (a
 * rotated token). Every route that reads a web session answers through
 * this. */
export const withSessionCookie = (request: Request, response: Response): Response => {
  const cookies = pendingCookies.get(request);
  if (!cookies?.length || response.status === 101) return response;
  pendingCookies.delete(request);
  const answer = new Response(response.body, response);
  for (const cookie of cookies) answer.headers.append("set-cookie", cookie);
  return answer;
};

/** The largest form post read for its CSRF field. */
export const MAX_FORM_BYTES = 64 * 1024;

/**
 * The CSRF token a request carries: the `X-CSRF-Token` header, or a form
 * post's `csrf` field (read through the shared body reader, so the route can
 * still read the body). `form`: it's a form post.
 */
export const requestCsrf = async (request: Request): Promise<{ readonly csrf: string | null; readonly form: boolean }> => {
  const header = request.headers.get(CSRF_HEADER);
  const form = request.headers.get("content-type")?.split(";")[0]?.trim().toLowerCase() === "application/x-www-form-urlencoded";
  if (header !== null || !form) return { csrf: header, form };
  const body = await readBody(request, MAX_FORM_BYTES);
  if (body === "too_large") return { csrf: null, form };
  return { csrf: new URLSearchParams(new TextDecoder().decode(body)).get("csrf"), form };
};
