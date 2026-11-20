import type { Env } from "./env";
import { json } from "./http";
import staging from "./models/staging-v1.json";

/**
 * GET/HEAD /models/v1.json: the hosted model list engines lay over their
 * built-in model catalogs (crates/engine/src/hosted_models.rs has the format,
 * the checks and the merge rules). Public: it names models, nothing else.
 *
 * `MODEL_LIST` picks the list this deployment serves; only "staging" exists
 * (src/models/staging-v1.json). Unset (production today) ⇒ 404, which engines
 * read as "no list": the built-in catalog alone.
 */
const LISTS: Record<string, unknown> = { staging };

export async function modelListRoute(request: Request, env: Env, url: URL): Promise<Response | null> {
  if (url.pathname !== "/models/v1.json" || (request.method !== "GET" && request.method !== "HEAD")) {
    return null;
  }
  const list = env.MODEL_LIST && Object.hasOwn(LISTS, env.MODEL_LIST) ? LISTS[env.MODEL_LIST] : undefined;
  if (!list) return json({ error: "not_found" }, 404);
  const body = JSON.stringify(list);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(body)));
  const etag = `"${[...digest.slice(0, 16)].map((b) => b.toString(16).padStart(2, "0")).join("")}"`;
  const headers = new Headers({
    "content-type": "application/json",
    "cache-control": "public, max-age=300",
    etag
  });
  const match = request.headers.get("if-none-match");
  if (match && match.split(",").some((tag) => tag.trim() === etag)) {
    return new Response(null, { status: 304, headers });
  }
  headers.set("content-length", String(new TextEncoder().encode(body).length));
  return new Response(request.method === "HEAD" ? null : body, { headers });
}
