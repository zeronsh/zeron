/**
 * Who is calling a store route: the one seam between the store and the
 * edge's identity (contract "Edge principals", amendment 18). Everything the
 * store knows about a caller comes through these functions; the identity
 * lane owns their bodies (deploy keys and their signed requests, the
 * `admin` role in users1, web sessions) and the store owns the rest.
 *
 * What's here now is the small honest version: a session bearer through the
 * edge's `authenticate`; no deploy keys; the admin role read from a test
 * binding (`STORE_ADMIN_USERS`), so no one is an admin in a deployment until
 * the role lands.
 */
import { authenticate } from "../auth";
import type { Env } from "../env";
import { jsonError } from "../http";
import { readCapped } from "../read-capped";
import { userDirectoryStub } from "../user-directory";

export type Publisher = {
  userId: string;
  username: string | null;
  via: "session" | "deploy_key";
  /** deploy_key: the key that signed the request. */
  key?: string;
  /** deploy_key: the one plugin it may publish. */
  pluginScope?: string;
};

const bodies = new WeakMap<Request, { body: Promise<Uint8Array | undefined> }>();

/**
 * The request's body, at most `maxBytes` (undefined past it), read once per
 * request: a signed request's body is hashed before it's trusted, and the
 * route reads the same bytes after. The only way a store route reads one.
 */
export const readBody = (request: Request, maxBytes: number): Promise<Uint8Array | undefined> => {
  const cached = bodies.get(request);
  // (A body past the first read's cap is gone: it reads as too large.)
  if (cached) return cached.body.then((body) => (body !== undefined && body.length <= maxBytes ? body : undefined));
  const body = readCapped(request, maxBytes);
  bodies.set(request, { body });
  return body;
};

/** Whether the request carries anyone's credentials: such a request never
 * reads or fills the store's cache. */
export const hasCredentials = (request: Request): boolean =>
  request.headers.has("authorization") || new URL(request.url).searchParams.has("token") || /(?:^|;\s*)__Host-zeron/.test(request.headers.get("cookie") ?? "");

const user = async (request: Request, env: Env): Promise<{ userId: string } | Response> => {
  const auth = await authenticate(env, request);
  if (!auth) return jsonError(401, "unauthenticated", "Sign in to do this.");
  // A Cloud runner acts as its device, never as its person's account.
  if (auth.kind !== "user") return jsonError(403, "forbidden", "Only a signed-in person can do this.");
  return { userId: auth.userId };
};

/** A caller who may publish: a signed-in session, or (identity lane) a
 * deploy key's signed request. */
export const authenticatePublisher = async (request: Request, env: Env): Promise<Publisher | Response> => {
  const caller = await user(request, env);
  if (caller instanceof Response) return caller;
  const directory = userDirectoryStub(env);
  if (!directory) return jsonError(503, "unavailable", "The user directory isn't set up on this server.");
  return { userId: caller.userId, username: await directory.username(caller.userId), via: "session" };
};

/** Whether `userId` holds `role` (the identity lane reads users1's
 * `user_roles`; until then, the test binding). */
const hasRole = async (env: Env, userId: string, role: "admin"): Promise<boolean> =>
  role === "admin" && (env.STORE_ADMIN_USERS ?? "").split(/[\s,]+/).filter(Boolean).includes(userId);

/** A signed-in caller who holds `admin`, or the refusal to answer. */
export const requireAdmin = async (request: Request, env: Env): Promise<{ userId: string } | Response> => {
  const caller = await user(request, env);
  if (caller instanceof Response) return caller;
  if (!(await hasRole(env, caller.userId, "admin"))) return jsonError(403, "forbidden", "This needs a Zeron admin.");
  return caller;
};

/** The signed-in caller, if any (a bad or runner credential reads as none). */
export const optionalUser = async (request: Request, env: Env): Promise<{ userId: string } | null> => {
  if (!hasCredentials(request)) return null;
  const caller = await user(request, env);
  return caller instanceof Response ? null : caller;
};

/**
 * Whether `key` (the package's signer) may sign a version of `pluginId` for
 * `publisher`: from a session, a live plugin key of the caller's account;
 * from a deploy key, that same key, scoped to that plugin.
 */
export const signingKeyAllowed = async (env: Env, publisher: Publisher, pluginId: string, key: string): Promise<boolean> => {
  if (publisher.via === "deploy_key") return publisher.key === key && publisher.pluginScope === pluginId;
  const directory = userDirectoryStub(env);
  const found = directory ? await directory.pluginKey(key, publisher.userId) : undefined;
  return found !== undefined && found.mine && !found.revoked;
};

/** A plugin changed hands: its deploy keys stop working (amendment 17).
 * The identity lane's; nothing to revoke until deploy keys exist. */
export const revokeDeployKeys = async (_env: Env, _pluginId: string): Promise<void> => {};
