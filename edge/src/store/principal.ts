/**
 * Edge principals: who is calling a store route (the seam between the two
 * edge lanes; plan "Edge principals"). The ways in are in
 * identity/callers.ts and identity/deploy-token.ts; what each kind of route
 * accepts is here.
 *
 * - Publisher routes: a session bearer, a web session's cookie, or (to
 *   publish a version of a plugin in its scope, or read its account's
 *   plugins in its scope) a deploy token, `Authorization: Bearer zdt_…`
 *   (`tokenInScope` below). Every other route refuses a token before
 *   looking it up, so no route tells a live token from a dead one but by
 *   publishing with it.
 * - Admin routes: a web session's cookie only (contract amendment 27), of an
 *   account holding the edge's `admin` role (roles.ts); never a bearer or a
 *   deploy token.
 * - Public routes: anyone; `optionalUser` says who, when it can tell.
 *
 * A route that reads a principal answers through `withSessionCookie` (a
 * staff session's rotated cookie rides on its answer), errors included, and
 * reads its body only through `readBody` or `bodyStream` (identity/body.ts).
 * A response for a request with a user must not be cached.
 */
import type { Env } from "../env";
import { jsonError } from "../http";
import { SESSION_COOKIE, sendsCookie } from "../web-cookies";
import { type DeployTokenScope, scopeHoldsPlugin, userDirectoryStub } from "../user-directory";
import { bearerUser, cookieSession, forbidden, headerBearer, unauthenticated } from "./identity/callers";
import { deployTokenHash, looksLikeDeployToken, parseDeployToken } from "./identity/deploy-token";

export { BodyRefused, MAX_PUBLISH_BODY_BYTES, bodyStream, readBody } from "./identity/body";
export { dropFromDeployTokens } from "./identity/deploy-tokens";
export { withSessionCookie } from "../web-session";

/**
 * Whether the request carries credentials of any kind: an `Authorization`
 * header, a `?token=` (the device API's WebSocket form, never read here but
 * never cached either), or the web session's cookie (live or not). The
 * store's cache neither reads nor stores for such a request, so nobody's
 * answer is ever served to anyone else. Decided from the request alone, with
 * no lookup.
 */
export const hasCredentials = (req: Request): boolean =>
  req.headers.get("authorization") !== null || new URL(req.url).searchParams.has("token") || sendsCookie(req, SESSION_COOKIE);

/** A deploy token, as a request it carried was checked (never its secret). */
export interface PublishingToken {
  readonly id: string;
  readonly short: string;
  readonly name: string;
  readonly scope: DeployTokenScope;
  readonly createdAt: number;
}

export type Publisher = {
  userId: string;
  username: string | null;
  via: "session" | "deploy_token";
  /** A deploy token's: its ids, name, scope and when it was made. */
  token?: PublishingToken;
};

/** A plugin as a scope is judged against it (the store's row). */
export interface ScopedPlugin {
  readonly id: string;
  readonly owner_id: string;
  readonly owner_since: number;
  readonly created_at: number;
}

/**
 * Whether `publisher` may act on `plugin`: a signed-in person, on any plugin
 * (ownership is the route's check); a deploy token, only on a plugin its
 * account owns now that its scope holds (`all`: owned when the token was
 * made, or claimed by the account and never moved).
 */
export const tokenInScope = (publisher: Publisher, plugin: ScopedPlugin): boolean =>
  publisher.via === "session" ||
  (publisher.token !== undefined &&
    scopeHoldsPlugin(publisher.userId, publisher.token.scope, publisher.token.createdAt, plugin.id, {
      owner: plugin.owner_id,
      ownerSince: plugin.owner_since,
      createdAt: plugin.created_at
    }));

/** A deploy token refused, the same for every reason (malformed, unknown,
 * revoked, expired, out of scope, a first version, its account's plugin no
 * longer): nothing in it tells them apart, nor the token itself. */
export const tokenRefused = (): Response => jsonError(401, "invalid_token", "This deploy token can't publish this plugin.");

/** A deploy token where only a person may act: refused before it's looked up. */
export const tokenNotAllowed = (): Response => jsonError(403, "token_not_allowed", "A deploy token can only publish.");

const usernameOf = async (env: Env, userId: string): Promise<string | null> =>
  (await userDirectoryStub(env)?.username(userId)) ?? null;

/**
 * Who publishes: a session bearer, a web session, or (with `tokens`, the
 * publish route and the account's plugin list) a deploy token, its last use
 * recorded for `usedFor` (the plugin it publishes; null to read). Without
 * `tokens`, a token is refused before it's looked up.
 */
export const authenticatePublisher = async (req: Request, env: Env, tokens?: { readonly usedFor: string | null }): Promise<Publisher | Response> => {
  const header = req.headers.get("authorization");
  if (header !== null) {
    const token = headerBearer(req);
    if (!token) return unauthenticated();
    if (looksLikeDeployToken(token)) {
      if (!tokens) return tokenNotAllowed();
      const parsed = parseDeployToken(token);
      const directory = userDirectoryStub(env);
      if (!parsed || !directory) return tokenRefused();
      const holder = await directory.deployTokenHolder(parsed.short, await deployTokenHash(token), tokens.usedFor);
      if (!holder) return tokenRefused();
      return {
        userId: holder.userId,
        username: holder.username,
        via: "deploy_token",
        token: { id: holder.id, short: holder.short, name: holder.name, scope: holder.scope, createdAt: holder.createdAt }
      };
    }
    const user = await bearerUser(env, token);
    if (user instanceof Response) return user;
    return { userId: user.userId, username: await usernameOf(env, user.userId), via: "session" };
  }
  const session = await cookieSession(req, env);
  if (!session) return unauthenticated();
  if (session instanceof Response) return session;
  return { userId: session.userId, username: session.username, via: "session" };
};

export const requireAdmin = async (req: Request, env: Env): Promise<{ userId: string } | Response> => {
  // Web session only (contract amendment 27): a bearer, which any local
  // process an engine hands its token to holds, skips the staff session's
  // short lifetime, rotation and CSRF checks. A deploy token never is staff.
  if (req.headers.get("authorization") !== null) return forbidden("Admin is by signing in on the web only.");
  const session = await cookieSession(req, env, true);
  if (!session) return unauthenticated();
  if (session instanceof Response) return session;
  if (!session.roles.includes("admin")) return forbidden();
  return { userId: session.userId };
};

export const optionalUser = async (req: Request, env: Env): Promise<{ userId: string } | null> => {
  if (req.headers.get("authorization") !== null) {
    const token = headerBearer(req);
    if (!token) return null;
    const user = await bearerUser(env, token);
    return user instanceof Response ? null : { userId: user.userId };
  }
  const session = await cookieSession(req, env);
  return session && !(session instanceof Response) ? { userId: session.userId } : null;
};

/**
 * Whether `key`, which signed a package `publisher` publishes, may (amendment
 * 24): a live plugin key of the publisher's account, a device's. Revoked keys
 * sign nothing; a deleted account's keys are all revoked. (A package
 * published with a deploy token needn't be signed: contract a315eef5a.)
 */
export const signingKeyAllowed = async (env: Env, publisher: Publisher, key: string): Promise<boolean> => {
  const directory = userDirectoryStub(env);
  return directory ? await directory.livePluginKey(publisher.userId, key) : false;
};
