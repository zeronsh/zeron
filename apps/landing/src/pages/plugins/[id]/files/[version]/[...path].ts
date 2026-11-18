// A picture from a version's package (its icon, its screenshots, the
// pictures in its README), on the site's own origin so its pages' CSP can
// say "pictures only from here". Only pictures pass, typed by their
// extension and never by what the file claims, and each is sandboxed: an
// SVG opened on its own runs nothing here.
//
// An approved version's files are public and never change (a version
// belongs to its content for good), so they are kept a year. A pending
// one's are its owner's and the admins': asked for again with the
// visitor's cookie, and kept by no one.
import type { APIRoute } from "astro";
import { isPluginId, isVersion, packagePath } from "../../../../../lib/store/names.ts";

export const prerender = false;

const PICTURES: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  avif: "image/avif",
  svg: "image/svg+xml",
};

const YEAR = 31_536_000;

const notFound = () => new Response(null, { status: 404 });

export const GET: APIRoute = async ({ params, request, locals, cache }) => {
  const { id, version } = params;
  const path = packagePath(params.path ?? "");
  if (!isPluginId(id) || !isVersion(version) || !path) return notFound();
  const type = PICTURES[path.split(".").pop()!.toLowerCase()];
  if (!type) return notFound();

  let open = true;
  let answer = await locals.edge.file(id, version, path, false);
  if (answer.status === 404 && request.headers.has("cookie")) {
    open = false;
    answer = await locals.edge.file(id, version, path, true);
  }
  if (!answer.ok || !answer.body) return notFound();
  // The Worker cache keeps a public one as long as the browser may.
  if (open) cache.set({ maxAge: YEAR });

  return new Response(answer.body, {
    headers: {
      "content-type": type,
      "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; sandbox",
      "content-disposition": "inline",
      "cross-origin-resource-policy": "same-origin",
      "cache-control": open ? `public, max-age=${YEAR}, immutable` : "private, no-store",
    },
  });
};
