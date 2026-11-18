// A picture from a version's package (its icon, its screenshots, the
// pictures in its README), on the site's own origin so its pages' CSP can
// say "pictures only from here". Only pictures pass, typed by their
// extension and never by what the file claims, and each is sandboxed: an
// SVG opened on its own runs nothing here.
//
// A public file is kept as the edge says, and by shared caches for a
// minute at most: the edge checks the version's state before it serves a
// file, so a version taken down stops showing within that (amendment 23).
// A pending one's are its owner's and the admins': asked for again with the
// visitor's cookie, and kept by no one.
import type { APIRoute } from "astro";
import { pluginTag } from "../../../../../lib/store/http.ts";
import { isPluginId, isVersion, packagePath } from "../../../../../lib/store/names.ts";

export const prerender = false;

const PICTURES = new Map([
  ["png", "image/png"],
  ["jpg", "image/jpeg"],
  ["jpeg", "image/jpeg"],
  ["gif", "image/gif"],
  ["webp", "image/webp"],
  ["avif", "image/avif"],
  ["svg", "image/svg+xml"],
]);

/** The most a shared cache keeps a public file (seconds). */
const SHARED = 60;

/** On every answer from this route, a picture or not: nothing here ever runs, nothing is sniffed. */
const SANDBOX = {
  "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; sandbox",
  "x-content-type-options": "nosniff",
  "cross-origin-resource-policy": "same-origin",
};

const notFound = () => new Response(null, { status: 404, headers: { ...SANDBOX, "cache-control": "no-store" } });

export const GET: APIRoute = async ({ params, request, locals, cache, url, redirect }) => {
  // One address per file: a query can't make the edge be asked again.
  if (url.search) return redirect(url.pathname, 301);
  const { id, version } = params;
  // The file's path from the address as sent, each part decoded once: a
  // path may hold "#", "%" and spaces, which the route's own decoding
  // (decodeURI) would leave half-done.
  const raw = new URL(request.url).pathname.split("/").slice(5);
  let path: string | null = null;
  try {
    path = packagePath(raw.map(decodeURIComponent).join("/"));
  } catch {
    path = null;
  }
  if (!isPluginId(id) || !isVersion(version) || !path) return notFound();
  const type = PICTURES.get(path.split(".").pop()!.toLowerCase());
  if (!type) return notFound();

  let open = true;
  let answer = await locals.edge.file(id, version, path, false);
  if (answer.status === 404 && request.headers.has("cookie")) {
    open = false;
    answer = await locals.edge.file(id, version, path, true);
  }
  // A file the edge gives only as a download is never shown inline from here.
  const attachment = /^\s*attachment/i.test(answer.headers.get("content-disposition") ?? "");
  if (!answer.ok || !answer.body || attachment) {
    await answer.body?.cancel();
    return notFound();
  }
  if (open) cache.set({ maxAge: SHARED, tags: [pluginTag(id)] });

  return new Response(answer.body, {
    headers: {
      ...SANDBOX,
      "content-type": type,
      "content-disposition": "inline",
      "cache-control": open ? (answer.headers.get("cache-control") ?? `public, max-age=${SHARED}`) : "private, no-store",
    },
  });
};
