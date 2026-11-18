/**
 * Edge principals: who is calling a store route (the seam between the two
 * edge lanes; plan "Edge principals"). The ways in are in
 * identity/callers.ts and identity/signed-request.ts; what each kind of
 * route accepts is here.
 *
 * - Publisher routes: a session bearer, a web session's cookie, or a deploy
 *   key's signed request (only to publish versions of plugins in its scope
 *   and read its account's plugins in its scope: identity/signing.ts
 *   `deployKeyRoute`, `keyInScope` below).
 * - Admin routes: a web session's cookie only (contract amendment 27), of an
 *   account holding the edge's `admin` role (roles.ts); never a bearer or a
 *   deploy key.
 * - Public routes: anyone; `optionalUser` says who, when it can tell.
 *
 * A route that reads a principal answers through `withSessionCookie` (a
 * staff session's rotated cookie rides on its answer), errors included, and
 * reads its body only through `readBody` or `bodyStream` (identity/body.ts),
 * which hold a deploy key's request to the body it signed. A response for a
 * request with a user must not be cached.
 */
import type { Env } from "../env";
import { SESSION_COOKIE, sendsCookie } from "../web-cookies";
import { type DeployKeyScope, scopeHoldsPlugin, userDirectoryStub } from "../user-directory";
import { bearerUser, cookieSession, forbidden, headerBearer, unauthenticated } from "./identity/callers";
import { verifySignedRequest } from "./identity/signed-request";

export { BodyRefused, MAX_PUBLISH_BODY_BYTES, bodyStream, readBody } from "./identity/body";
export { dropFromDeployKeys } from "./identity/deploy-keys";
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

export type Publisher = {
  userId: string;
  username: string | null;
  via: "session" | "deploy_key";
  /** A deploy key's: the key, its name, its scope and when it was made. */
  key?: string;
  keyName?: string;
  scope?: DeployKeyScope;
  keyCreatedAt?: number;
};

/** A plugin as a scope is judged against it (the store's row). */
export interface ScopedPlugin {
  readonly id: string;
  readonly owner_id: string;
  readonly owner_since: number;
  readonly created_at: number;
}

/**
 * Whether `publisher` may act on `plugin` (amendment 44): a signed-in
 * person, on any plugin (ownership is the route's check); a deploy key, only
 * on a plugin its account owns now that its scope holds (`all`: owned when
 * the key was made, or claimed by the account and never moved).
 */
export const keyInScope = (publisher: Publisher, plugin: ScopedPlugin): boolean =>
  publisher.via === "session" ||
  (publisher.scope !== undefined &&
    publisher.keyCreatedAt !== undefined &&
    scopeHoldsPlugin(publisher.userId, publisher.scope, publisher.keyCreatedAt, plugin.id, {
      owner: plugin.owner_id,
      ownerSince: plugin.owner_since,
      createdAt: plugin.created_at
    }));

const usernameOf = async (env: Env, userId: string): Promise<string | null> =>
  (await userDirectoryStub(env)?.username(userId)) ?? null;

export const authenticatePublisher = async (req: Request, env: Env): Promise<Publisher | Response> => {
  const header = req.headers.get("authorization");
  if (header !== null) {
    if (/^ZeronKey\s/i.test(header)) {
      const caller = await verifySignedRequest(req, env, new URL(req.url));
      if (caller instanceof Response) return caller;
      return {
        userId: caller.userId,
        username: caller.username,
        via: "deploy_key",
        key: caller.key,
        keyName: caller.name,
        scope: caller.scope,
        keyCreatedAt: caller.createdAt
      };
    }
    const token = headerBearer(req);
    if (!token) return unauthenticated();
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
  // short lifetime, rotation and CSRF checks. A deploy key never is staff.
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
 * Whether `publisher` may publish a package of plugin `id` (the store's row,
 * null for a new id) signed by `key` (amendments 24 and 44). With a
 * session: `key` is one of the caller's live device keys, or one of their
 * live deploy keys whose scope holds `id`. With a deploy key: `key` is that
 * very key. A deploy key never signs a new id. Revoked and expired keys
 * sign nothing; a deleted account's keys are all revoked.
 */
export const signingKeyAllowed = async (env: Env, publisher: Publisher, id: string, key: string, plugin: ScopedPlugin | null): Promise<boolean> => {
  if (publisher.via === "deploy_key" && publisher.key !== key) return false;
  const directory = userDirectoryStub(env);
  const signing = directory ? await directory.signingKeyLive(publisher.userId, key, id) : undefined;
  if (!signing) return false;
  if (signing.kind === "device") return true;
  return plugin !== null && scopeHoldsPlugin(publisher.userId, signing.scope, signing.createdAt, id, { owner: plugin.owner_id, ownerSince: plugin.owner_since, createdAt: plugin.created_at });
};
