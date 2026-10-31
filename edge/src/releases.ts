import type { Env } from "./env";
import { json } from "./http";

/**
 * GET/HEAD /releases/<key>: the release artifacts in the RELEASES bucket
 * (served at zeron.sh/releases/*) — what the installer and the website download.
 */
export async function releaseRoute(request: Request, env: Env, url: URL): Promise<Response | null> {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts[0] !== "releases" || parts.length < 2 || (request.method !== "GET" && request.method !== "HEAD")) {
    return null;
  }
  const key = decodeURIComponent(url.pathname.slice("/releases/".length));
  if (key.length === 0 || key.includes("..")) return json({ error: "bad request" }, 400);
  const object = await env.RELEASES.get(key);
  if (!object) return json({ error: "not_found" }, 404);
  // latest.txt / manifest.json flip on release; artifacts are immutable by
  // name — unless uploaded with their own policy (the Android APK's stable
  // name is replaced by every build: `no-cache`).
  const mutable = key.endsWith(".txt") || key.endsWith(".json");
  const headers = new Headers({
    "content-type": key.endsWith(".txt")
      ? "text/plain; charset=utf-8"
      : key.endsWith(".json")
        ? "application/json"
        : key.endsWith(".apk")
          ? "application/vnd.android.package-archive"
          : "application/octet-stream",
    "content-length": String(object.size),
    "cache-control":
      object.httpMetadata?.cacheControl ??
      (mutable ? "public, max-age=60" : "public, max-age=86400, immutable"),
    etag: object.httpEtag
  });
  return new Response(request.method === "HEAD" ? null : object.body, { headers });
}
