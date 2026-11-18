/**
 * Who is calling a store route: the one seam between the store and the
 * edge's identity (contract "Edge principals", amendments 18, 24, 27). The
 * identity lane's principal.ts replaces this file whole at the merge; this
 * is a stand-in with the same exports and the same answers for what the
 * store relies on:
 *
 * - publisher routes: a session bearer (deploy keys' signed requests are the
 *   identity lane's: refused here);
 * - admin routes: a web session's cookie only, never a bearer. The stand-in
 *   session is `__Host-zeron_session=store-test.<userId>` for a user named
 *   in the test binding `STORE_ADMIN_USERS`, so no one is an admin in a
 *   deployment until the real sessions and the `admin` role land;
 * - public routes: anyone; `optionalUser` says who, when it can tell;
 * - bodies through `readBody` only; answers through `withSessionCookie`.
 */
import { authenticate } from "../auth";
import type { Env } from "../env";
import { jsonError } from "../http";
import { readCapped } from "../read-capped";
import { userDirectoryStub } from "../user-directory";

/** The largest body a store publisher route reads: the `.zeron-plugin` file
 * as uploaded (its files base64 inside JSON), 20 MiB. Measured:
 * docs/plugin-store-limits.md. */
export const MAX_PUBLISH_BODY_BYTES = 20 * 1024 * 1024;

export type Publisher = {
  userId: string;
  username: string | null;
  via: "session" | "deploy_key";
  key?: string;
  pluginScope?: string;
};

const SESSION_COOKIE = "__Host-zeron_session";

const sessionToken = (req: Request): string | undefined => {
  for (const part of (req.headers.get("cookie") ?? "").split(";")) {
    const [name, ...value] = part.trim().split("=");
    if (name === SESSION_COOKIE) return value.join("=");
  }
  return undefined;
};

/** Whether the request carries credentials of any kind: such a request
 * never reads or fills the store's cache. */
export const hasCredentials = (req: Request): boolean => req.headers.get("authorization") !== null || sessionToken(req) !== undefined;

export class BodyRefused extends Error {
  constructor(readonly reason: "too_large" | "body_mismatch") {
    super(reason === "too_large" ? "The request body is too large." : "The body isn't the one that was signed.");
  }
}

const bodies = new WeakMap<Request, Promise<Uint8Array | undefined>>();

/** The request's body, at most `maxBytes` (else `"too_large"`), read from
 * the wire once; later calls answer the same. (`"body_mismatch"`: a signed
 * request's body that isn't the one signed; none here.) */
export const readBody = async (req: Request, maxBytes = MAX_PUBLISH_BODY_BYTES): Promise<Uint8Array | "too_large" | "body_mismatch"> => {
  let read = bodies.get(req);
  if (!read) {
    read = readCapped(req, maxBytes);
    bodies.set(req, read);
  }
  const bytes = await read;
  return bytes === undefined || bytes.byteLength > maxBytes ? "too_large" : bytes;
};

/** The request's body as a stream of at most `maxBytes`. */
export const bodyStream = (req: Request, maxBytes = MAX_PUBLISH_BODY_BYTES): ReadableStream<Uint8Array> => {
  let length = 0;
  const source = req.body ?? new ReadableStream<Uint8Array>({ start: (c) => c.close() });
  return source.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        length += chunk.byteLength;
        if (length > maxBytes) throw new BodyRefused("too_large");
        controller.enqueue(chunk);
      }
    })
  );
};

/** The answer, with any rotated session cookie on it (none here). */
export const withSessionCookie = (_req: Request, response: Response): Response => response;

const unauthenticated = () => jsonError(401, "unauthenticated", "Sign in to do this.");
const forbidden = (message = "You can't do this.") => jsonError(403, "forbidden", message);

const bearerUser = async (req: Request, env: Env): Promise<{ userId: string } | Response> => {
  const auth = await authenticate(env, req);
  if (!auth) return unauthenticated();
  // A Cloud runner acts as its device, never as its person's account.
  if (auth.kind !== "user") return forbidden("Only a signed-in person can do this.");
  return { userId: auth.userId };
};

/** The stand-in web session: `store-test.<userId>`, for test admins only. */
const cookieUser = (req: Request, env: Env): { userId: string; roles: string[] } | null => {
  const token = sessionToken(req);
  const userId = token?.startsWith("store-test.") ? token.slice("store-test.".length) : undefined;
  if (!userId || !(env.STORE_ADMIN_USERS ?? "").split(/[\s,]+/).includes(userId)) return null;
  return { userId, roles: ["admin"] };
};

export const authenticatePublisher = async (req: Request, env: Env): Promise<Publisher | Response> => {
  const header = req.headers.get("authorization");
  if (header !== null && /^ZeronKey\s/i.test(header)) return jsonError(401, "invalid_signature", "Deploy keys aren't accepted here yet.");
  if (header === null) {
    const session = cookieUser(req, env);
    if (!session) return unauthenticated();
    return { userId: session.userId, username: await usernameOf(env, session.userId), via: "session" };
  }
  const user = await bearerUser(req, env);
  if (user instanceof Response) return user;
  return { userId: user.userId, username: await usernameOf(env, user.userId), via: "session" };
};

const usernameOf = async (env: Env, userId: string): Promise<string | null> => (await userDirectoryStub(env)?.username(userId)) ?? null;

/** A web session (never a bearer) of an account holding `admin`. */
export const requireAdmin = async (req: Request, env: Env): Promise<{ userId: string } | Response> => {
  if (req.headers.get("authorization") !== null) return forbidden("Admin is by signing in on the web only.");
  if (sessionToken(req) === undefined) return unauthenticated();
  const session = cookieUser(req, env);
  if (!session) return unauthenticated();
  if (!session.roles.includes("admin")) return forbidden();
  return { userId: session.userId };
};

/** The signed-in caller, if any (a bad or runner credential reads as none). */
export const optionalUser = async (req: Request, env: Env): Promise<{ userId: string } | null> => {
  const header = req.headers.get("authorization");
  if (header === null) {
    const session = cookieUser(req, env);
    return session ? { userId: session.userId } : null;
  }
  if (!/^bearer\s/i.test(header)) return null;
  const user = await bearerUser(req, env);
  return user instanceof Response ? null : user;
};

/**
 * Whether `publisher` may submit a package of plugin `id` signed by `key`
 * (amendment 24): with a session, a live device key of the caller's; with a
 * deploy key, that key, scoped to `id`.
 */
export const signingKeyAllowed = async (env: Env, publisher: Publisher, id: string, key: string): Promise<boolean> => {
  if (publisher.via === "deploy_key") return publisher.key === key && publisher.pluginScope === id;
  const directory = userDirectoryStub(env);
  const found = directory ? await directory.pluginKey(key, publisher.userId) : undefined;
  return found !== undefined && found.mine && !found.revoked;
};

/** A plugin changed hands: its deploy keys stop working (amendment 17). */
export const revokeDeployKeys = async (_env: Env, _pluginId: string): Promise<void> => {};
