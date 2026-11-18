/**
 * The pure parts of web sign-in (web-auth-routes.ts) and web sessions
 * (web-session.ts): cookies, tokens, where to return to, and the cross-site
 * checks. Nothing here touches storage, so the Node unit tier tests it.
 */
import { b64urlDecode, b64urlEncode, constantTimeEqual } from "./cloud/crypto";
import type { Env } from "./env";

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

/** Whether the request sends cookie `name` at all (once or more). */
export const sendsCookie = (request: Request, name: string): boolean =>
  (request.headers.get("cookie") ?? "").split(";").some((part) => part.split("=", 1)[0]!.trim() === name);

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

export const LOGIN_COOKIE = "__Host-zeron_web_login";
/** How long a sign-in may take from login to callback. */
export const LOGIN_TTL_MS = 10 * 60_000;
const MAX_RETURN_TO = 2048;

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
