/**
 * Packages and packaged files.
 *
 *   GET /store/v1/plugins/:id/versions/:version/package
 *   GET /store/v1/plugins/:id/files/:version/*path
 *
 * Who may read: an approved or yanked version of an active plugin's package
 * and its pictures (PNG and JPEG that image-check.ts takes) and icon, by
 * anyone; anything else (any file, a pending, rejected, withdrawn or removed
 * version) by its owner or an admin. To anyone else it doesn't exist (404).
 *
 * A packaged file is never active content on our origin (amendment 14):
 * every answer is `nosniff` with `Content-Security-Policy: sandbox;
 * default-src 'none'`; pictures are served inline as image-check.ts rebuilt
 * them, the icon inline as SVG (scripts can't run under that policy), and
 * everything else as an `application/octet-stream` attachment.
 *
 * A file's path is checked with the package path rules before anything is
 * looked up, and is only ever a key into the version's own rows: the bytes
 * come from that version's files object by offset, so no path reaches
 * another object.
 */
import type { Env } from "../env";
import { cleanImage } from "../image-check";
import { FILE_TTL, cacheKey, throughCache } from "./cache";
import { type PluginRow, type VersionRow, filesKey, packageKey, pluginRow, versionRow } from "./db";
import { fail, notFound } from "./http";
import { plainPath } from "./paths";
import { optionalUser, requireAdmin } from "./principal";

export const SANDBOX = "sandbox; default-src 'none'";

const isPublic = (plugin: PluginRow, version: VersionRow): boolean =>
  plugin.status === "active" && (version.status === "approved" || version.status === "yanked");

/** Whether the caller owns the plugin or is an admin. */
const privileged = async (request: Request, env: Env, plugin: PluginRow): Promise<boolean> => {
  const caller = await optionalUser(request, env);
  if (!caller) return false;
  if (caller.userId === plugin.owner_id) return true;
  return !((await requireAdmin(request, env)) instanceof Response);
};

const attachmentName = (name: string): string => name.replace(/[^A-Za-z0-9._-]/g, "_").slice(0, 120) || "file";

const baseHeaders = (cacheControl: string): Headers =>
  new Headers({
    "x-content-type-options": "nosniff",
    "content-security-policy": SANDBOX,
    "cache-control": cacheControl
  });

/** A version named in a URL segment, decoded (versions may carry `+`). */
export const decodeSegment = (segment: string): string | undefined => {
  try {
    return decodeURIComponent(segment);
  } catch {
    return undefined;
  }
};

export const servePackage = async (request: Request, env: Env, db: D1Database, bucket: R2Bucket, id: string, versionSegment: string): Promise<Response> => {
  const versionText = decodeSegment(versionSegment);
  if (!versionText) return notFound();
  const [plugin, version] = await Promise.all([pluginRow(db, id), versionRow(db, id, versionText)]);
  if (!plugin || !version) return notFound();
  const read = async (cacheControl: string): Promise<Response> => {
    const object = await bucket.get(packageKey(id, version.version, version.package_sha256));
    if (!object) return fail(503, "unavailable", "The package can't be read right now.");
    const headers = baseHeaders(cacheControl);
    headers.set("content-type", "application/octet-stream");
    headers.set("content-disposition", `attachment; filename="${attachmentName(`${id}-${version.version}`)}.zeron-plugin"`);
    headers.set("content-length", String(object.size));
    headers.set("etag", `"${version.package_sha256}"`);
    return new Response(request.method === "HEAD" ? null : object.body, { headers });
  };
  if (isPublic(plugin, version)) {
    return throughCache(request, cacheKey(`package/${id}/${version.version}`), FILE_TTL, () => read(`public, max-age=${FILE_TTL}`));
  }
  if (!(await privileged(request, env, plugin))) return notFound();
  return read("private, no-store");
};

interface FileRow {
  readonly path: string;
  readonly at: number;
  readonly bytes: number;
  readonly sha256: string;
  readonly kind: "png" | "jpeg" | "svg" | "text" | "binary";
}

export const serveFile = async (
  request: Request,
  env: Env,
  db: D1Database,
  bucket: R2Bucket,
  id: string,
  versionSegment: string,
  pathSegments: readonly string[]
): Promise<Response> => {
  const versionText = decodeSegment(versionSegment);
  const decoded = pathSegments.map(decodeSegment);
  if (!versionText || decoded.some((p) => p === undefined)) return fail(400, "bad_path", "That isn't a file's path.");
  const path = decoded.join("/");
  if (!plainPath(path)) return fail(400, "bad_path", "That isn't a file's path.");
  const [plugin, version, file] = await Promise.all([
    pluginRow(db, id),
    versionRow(db, id, versionText),
    db.prepare("SELECT path, at, bytes, sha256, kind FROM version_files WHERE plugin_id = ? AND version = ? AND path = ?").bind(id, versionText, path).first<FileRow>()
  ]);
  if (!plugin || !version || !file) return notFound();
  const picture = file.kind === "png" || file.kind === "jpeg" || (file.kind === "svg" && file.path === version.icon);
  const read = async (cacheControl: string, inline: boolean): Promise<Response> => {
    let bytes: Uint8Array;
    if (file.bytes === 0) bytes = new Uint8Array(0);
    else {
      const object = await bucket.get(filesKey(id, version.version, version.package_sha256), { range: { offset: file.at, length: file.bytes } });
      if (!object) return fail(503, "unavailable", "The file can't be read right now.");
      bytes = new Uint8Array(await object.arrayBuffer());
    }
    const headers = baseHeaders(cacheControl);
    headers.set("etag", `"${file.sha256}"`);
    const name = file.path.split("/").pop()!;
    if (inline && (file.kind === "png" || file.kind === "jpeg")) {
      const type = file.kind === "png" ? "image/png" : "image/jpeg";
      const clean = cleanImage(type, bytes);
      if (clean) {
        headers.set("content-type", type);
        return new Response(request.method === "HEAD" ? null : clean, { headers });
      }
    } else if (inline && file.kind === "svg") {
      headers.set("content-type", "image/svg+xml");
      return new Response(request.method === "HEAD" ? null : bytes, { headers });
    }
    headers.set("content-type", "application/octet-stream");
    headers.set("content-disposition", `attachment; filename="${attachmentName(name)}"`);
    return new Response(request.method === "HEAD" ? null : bytes, { headers });
  };
  if (picture && isPublic(plugin, version)) {
    return throughCache(request, cacheKey(`files/${id}/${version.version}/${path}`), FILE_TTL, () => read(`public, max-age=${FILE_TTL}`, true));
  }
  if (!(await privileged(request, env, plugin))) return notFound();
  return read("private, no-store", picture);
};

/** The cache keys of a version's public package and pictures. */
export const versionFileKeys = async (db: D1Database, id: string, version: string): Promise<string[]> => {
  const rows = await db
    .prepare(
      `SELECT path FROM version_files WHERE plugin_id = ? AND version = ?
         AND (kind IN ('png', 'jpeg') OR path = (SELECT icon FROM versions WHERE plugin_id = ? AND version = ?))`
    )
    .bind(id, version, id, version)
    .all<{ path: string }>();
  return [cacheKey(`package/${id}/${version}`), ...rows.results.map((r) => cacheKey(`files/${id}/${version}/${r.path}`))];
};
