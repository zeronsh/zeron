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
 *   `x-zeron-csrf`. The token is random per session, kept server-side and
 *   stays the same across rotation; one session's token is worthless with
 *   another's cookie.
 */
import { b64urlEncode, constantTimeEqual } from "./cloud/crypto";
import type { Env } from "./env";
import { userDirectoryStub, type Role, type WebSessionRead } from "./user-directory";

export const SESSION_COOKIE = "__Host-zeron_session";
export const CSRF_HEADER = "x-zeron-csrf";

const TOKEN_RE = /^[A-Za-z0-9_-]{43}$/;

/** 32 random bytes, b64url: a session token or a CSRF token. */
export const newToken = (): string => b64urlEncode(crypto.getRandomValues(new Uint8Array(32)));

/** The stored form of a session token: SHA-256, hex. Undefined for
 * something that isn't one. */
export const sessionTokenHash = async (token: unknown): Promise<string | undefined> => {
  if (typeof token !== "string" || !TOKEN_RE.test(token)) return undefined;
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
};

/** The one value of cookie `name` in the request; undefined when it's absent
 * or sent more than once (a cookie can't be told from a planted twin). */
export const readCookie = (request: Request, name: string): string | undefined => {
  const header = request.headers.get("cookie");
  if (!header) return undefined;
  let found: string | undefined;
  for (const part of header.split(";")) {
    const eq = part.indexOf("=");
    if (eq < 0 || part.slice(0, eq).trim() !== name) continue;
    if (found !== undefined) return undefined;
    found = part.slice(eq + 1).trim();
  }
  return found;
};

/** A `__Host-` cookie: Secure, Path=/, no Domain; HttpOnly and SameSite=Lax.
 * `maxAgeMs` 0 clears it. */
export const hostCookie = (name: string, value: string, maxAgeMs: number): string =>
  `${name}=${value}; Path=/; Secure; HttpOnly; SameSite=Lax; Max-Age=${Math.max(0, Math.floor(maxAgeMs / 1000))}`;

export const sessionCookie = (token: string, absoluteExpiresAt: number, now = Date.now()): string =>
  hostCookie(SESSION_COOKIE, token, absoluteExpiresAt - now);

export const clearSessionCookie = (): string => hostCookie(SESSION_COOKIE, "", 0);

/** The site's origin, when web sign-in is set up here. */
export const webOrigin = (env: Pick<Env, "WEB_ORIGIN">): string | undefined => {
  const raw = env.WEB_ORIGIN;
  if (!raw) return undefined;
  try {
    const url = new URL(raw);
    return url.protocol === "https:" && url.origin === raw ? url.origin : undefined;
  } catch {
    return undefined;
  }
};

const SAFE_METHODS = new Set(["GET", "HEAD"]);

export const isSafeMethod = (method: string): boolean => SAFE_METHODS.has(method);

/** Why a cookie-authenticated state-changing request is refused: not sent
 * from the web origin, or without the session's CSRF token. `csrf` is the
 * token the request carried (its header, or a form field). */
export const crossSiteRefusal = (request: Request, env: Pick<Env, "WEB_ORIGIN">, expected: string, csrf: string | null): string | undefined => {
  const origin = webOrigin(env);
  if (!origin || request.headers.get("origin") !== origin) return "bad_origin";
  const site = request.headers.get("sec-fetch-site");
  if (site !== null && site !== "same-origin") return "bad_origin";
  if (!csrf || !constantTimeEqual(csrf, expected)) return "bad_csrf";
  return undefined;
};

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

export const sessionHasRole = (session: Pick<WebSessionRead, "roles">, role: Role): boolean => session.roles.includes(role);
