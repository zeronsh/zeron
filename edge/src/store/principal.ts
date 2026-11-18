/**
 * Edge principals: who is calling a store route (the seam between the two
 * edge lanes; plan "Edge principals"). The ways in are in
 * identity/callers.ts and identity/signed-request.ts; what each kind of
 * route accepts is here.
 *
 * - Publisher routes: a session bearer, a web session's cookie, or a deploy
 *   key's signed request (only to publish its plugin's versions and read its
 *   plugin's state: identity/signing.ts `deployKeyRoute`; the store answers
 *   `GET /store/v1/me/plugins` for `pluginScope` alone).
 * - Admin routes: a session bearer or a web session's cookie, of an account
 *   holding the edge's `admin` role (roles.ts); never a deploy key.
 * - Public routes: anyone; `optionalUser` says who, when it can tell.
 *
 * A route that reads a principal answers through `withSessionCookie` (a
 * staff session's rotated cookie rides on its answer), errors included, and
 * reads its body only through `readBody` or `bodyStream` (identity/body.ts),
 * which hold a deploy key's request to the body it signed. A response for a
 * request with a user must not be cached.
 */
import type { Env } from "../env";
import { hasRole } from "../roles";
import { userDirectoryStub } from "../user-directory";
import { bearerUser, cookieSession, forbidden, headerBearer, unauthenticated } from "./identity/callers";
import { verifySignedRequest } from "./identity/signed-request";

export { BodyRefused, MAX_PUBLISH_BODY_BYTES, bodyStream, readBody } from "./identity/body";
export { revokeDeployKeys } from "./identity/deploy-keys";
export { withSessionCookie } from "../web-session";

export type Publisher = {
  userId: string;
  username: string | null;
  via: "session" | "deploy_key";
  key?: string;
  pluginScope?: string;
};

const usernameOf = async (env: Env, userId: string): Promise<string | null> =>
  (await userDirectoryStub(env)?.username(userId)) ?? null;

export const authenticatePublisher = async (req: Request, env: Env): Promise<Publisher | Response> => {
  const header = req.headers.get("authorization");
  if (header !== null) {
    if (/^ZeronKey\s/i.test(header)) {
      const caller = await verifySignedRequest(req, env, new URL(req.url));
      if (caller instanceof Response) return caller;
      return { userId: caller.userId, username: caller.username, via: "deploy_key", key: caller.key, pluginScope: caller.pluginId };
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
  const header = req.headers.get("authorization");
  if (header !== null) {
    const token = headerBearer(req);
    if (!token) return /^ZeronKey\s/i.test(header) ? forbidden("A deploy key can't do this.") : unauthenticated();
    const user = await bearerUser(env, token);
    if (user instanceof Response) return user;
    if (!(await hasRole(env, user.userId, "admin"))) return forbidden();
    return { userId: user.userId };
  }
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
 * Whether `publisher` may submit a package of plugin `id` signed by `key`
 * (contract amendment 24). With a session: `key` is one of the caller's live
 * device keys, or the caller's live deploy key for `id`. With a deploy key:
 * `key` is that very key, and `id` its plugin. Revoked and expired keys sign
 * nothing; a deleted account's keys are all revoked.
 */
export const signingKeyAllowed = async (env: Env, publisher: Publisher, id: string, key: string): Promise<boolean> => {
  if (publisher.via === "deploy_key") {
    if (publisher.key !== key || publisher.pluginScope !== id) return false;
  }
  const directory = userDirectoryStub(env);
  return directory ? directory.signingKeyLive(publisher.userId, key, id) : false;
};
