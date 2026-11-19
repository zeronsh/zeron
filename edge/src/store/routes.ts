/**
 * `/store/v1/…`: the plugin store's routes (docs/plans/2026-11-17-plugin-store.md
 * "Contract"). Mounted before the edge's bearer gate: public reads need no
 * sign-in, and every other route asks principal.ts who the caller is.
 *
 * Public reads (and update checks) are limited per client IP (STORE_READS);
 * writes per account (STORE_WRITES, in each route). Anything that throws is
 * one plain 500, with no detail.
 *
 * `/store/v1/me/tokens` (deploy tokens) is the identity lane's,
 * mounted before this; here it is not found.
 */
import type { Env } from "../env";
import { handleAdmin } from "./admin";
import { handleAuthorAction, handleInstalled, handleMyPlugins, handleReport } from "./author";
import { handleCatalog } from "./catalog";
import { fail, notFound, rateLimited } from "./http";
import { handlePublish } from "./publish";
import { serveFile, servePackage } from "./serve";
import { handleUpdates } from "./updates";
import { sessionRequired, withSessionCookie } from "./principal";
import { headerBearer } from "./identity/callers";
import { looksLikeDeployToken } from "./identity/deploy-token";

const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;

const allowRead = async (request: Request, env: Env): Promise<boolean> => {
  const ip = request.headers.get("cf-connecting-ip");
  return !ip || !env.STORE_READS || (await env.STORE_READS.limit({ key: ip })).success;
};

/** What a deploy token may reach: publishing a version, and nothing else
 * (contract "A token publishes and reads nothing"). */
const deployTokenMayCall = (method: string, parts: readonly string[]): boolean =>
  method === "POST" && parts[2] === "plugins" && parts.length === 5 && parts[4] === "versions";

const route = async (request: Request, env: Env, db: D1Database, bucket: R2Bucket, url: URL, parts: readonly string[]): Promise<Response> => {
  const method = request.method;
  const read = method === "GET" || method === "HEAD";
  // A deploy token anywhere else: refused before it's looked up.
  const bearer = headerBearer(request);
  if (bearer !== undefined && looksLikeDeployToken(bearer) && !deployTokenMayCall(method, parts)) return sessionRequired();
  if (parts[2] === "admin") return handleAdmin(request, env, db, bucket, url, parts.slice(2));
  if (parts[2] === "me" && parts[3] === "plugins" && parts.length === 4 && read) return handleMyPlugins(request, env, db);
  if (parts[2] === "updates" && parts.length === 3 && method === "POST") {
    if (!(await allowRead(request, env))) return rateLimited();
    return handleUpdates(request, env, db);
  }
  if (read && (parts[2] === "categories" || parts[2] === "plugins") && !(await allowRead(request, env))) return rateLimited();
  const catalog = await handleCatalog(request, env, db, url, parts);
  if (catalog) return catalog;
  if (parts[2] !== "plugins" || parts.length < 5) return notFound();
  const id = parts[3]!;
  if (!ID_RE.test(id)) return notFound();
  const sub = parts[4];
  if (sub === "versions" && parts.length === 5 && method === "POST") return handlePublish(request, env, db, bucket, id);
  if (sub === "installed" && parts.length === 5 && method === "POST") return handleInstalled(request, env, db, id);
  if (sub === "reports" && parts.length === 5 && method === "POST") return handleReport(request, env, db, id);
  if (sub === "versions" && parts.length === 7) {
    const action = parts[6];
    if (action === "package" && read) return servePackage(request, env, db, bucket, id, parts[5]!);
    if ((action === "yank" || action === "unyank") && method === "POST") {
      return handleAuthorAction(request, env, db, id, parts[5]!, action);
    }
  }
  if (sub === "files" && parts.length >= 7 && read) return serveFile(request, env, db, bucket, id, parts[5]!, parts.slice(6));
  return notFound();
};

export const handleStoreRoute = async (request: Request, env: Env, url: URL): Promise<Response | undefined> => {
  // Raw segments (empty ones kept, so `a//b` is no path), the leading one dropped.
  const parts = url.pathname.split("/").slice(1);
  if (parts[0] !== "store" || parts[1] !== "v1") return undefined;
  // (`/store/v1/me/tokens` is the identity lane's, mounted before this.)
  if (parts[2] === "me" && parts[3] === "keys") return undefined;
  const db = env.STORE_DB;
  const bucket = env.PLUGIN_STORE;
  if (!db || !bucket) return withSessionCookie(request, fail(503, "unavailable", "The plugin store isn't set up on this server."));
  let response: Response;
  try {
    response = await route(request, env, db, bucket, url, parts);
  } catch (e) {
    console.error("store: a request failed", request.method, url.pathname, String(e));
    response = fail(500, "internal", "Something went wrong. Try again.");
  }
  // A staff session's rotated cookie rides on every answer, errors included.
  return withSessionCookie(request, response);
};
