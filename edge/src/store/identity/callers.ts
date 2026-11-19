/**
 * Who is calling a store route, by each way the store accepts: a WorkOS
 * session bearer (the app, the CLI), a web session's cookie (the web site),
 * a deploy token (deploy-token.ts), a bearer of its own. principal.ts
 * decides which a route takes.
 *
 * - A bearer comes only in `Authorization` (never `?token=`: query strings
 *   end up in logs). A runner token (a Cloud machine) is never a publisher
 *   or an admin.
 * - When `Authorization` is present, it alone decides: a cookie beside it
 *   is ignored, so a bad bearer can't fall back to a browser's session.
 * - A cookie authenticates a state-changing request only from the web
 *   origin with the session's CSRF token (web-session.ts).
 */
import { verifyToken } from "../../auth";
import type { Env } from "../../env";
import { jsonError } from "../../http";
import { crossSiteRefusal, isSafeMethod } from "../../web-cookies";
import { requestCsrf, resolveWebSession, type ResolvedSession } from "../../web-session";

/** The bearer token in `Authorization`, or undefined (none, or not Bearer). */
export const headerBearer = (request: Request): string | undefined => {
  const header = request.headers.get("authorization");
  if (!header?.toLowerCase().startsWith("bearer ")) return undefined;
  const token = header.slice(7).trim();
  return token || undefined;
};

export const unauthenticated = (): Response => jsonError(401, "unauthenticated", "Sign in to do this.");
export const forbidden = (message = "This account can't do this."): Response => jsonError(403, "forbidden", message);

/** The user a bearer names, or the answer refusing it: 401 when it doesn't
 * verify, 403 for a runner token. */
export const bearerUser = async (env: Env, token: string): Promise<{ readonly userId: string } | Response> => {
  const verified = await verifyToken(env, token);
  if (!verified) return unauthenticated();
  if (verified.kind === "runner") return forbidden("A Cloud machine's token can't do this.");
  return { userId: verified.userId };
};

/** The web session the request's cookie names, refused when the request
 * changes something and isn't from the web origin with the session's CSRF
 * token; undefined when there's no live session. */
export const cookieSession = async (
  request: Request,
  env: Env,
  privileged = false
): Promise<ResolvedSession | Response | undefined> => {
  if (isSafeMethod(request.method)) return resolveWebSession(request, env, privileged);
  // A change is checked before a staff token rotates: a refused request
  // never costs the browser its cookie.
  const session = await resolveWebSession(request, env);
  if (!session) return undefined;
  const refusal = crossSiteRefusal(request, env, session.csrf, (await requestCsrf(request)).csrf);
  // Both `403 bad_csrf` on every store route (contract amendment 35).
  if (refusal === "bad_origin") return jsonError(403, "bad_csrf", "This request didn't come from the Zeron site.");
  if (refusal) return jsonError(403, "bad_csrf", "This page is out of date. Reload it and try again.");
  return privileged ? resolveWebSession(request, env, true) : session;
};
