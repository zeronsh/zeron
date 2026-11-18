/**
 * Packages and packaged files.
 *
 *   GET /store/v1/plugins/:id/versions/:version/package
 *   GET /store/v1/plugins/:id/files/:version/*path
 *
 * Who may read: a listed or yanked version of an active plugin's package
 * (a yanked one for the devices that have it) and its pictures (PNG and
 * JPEG that image-check.ts takes) and icon, by anyone; anything else (any
 * other file, a taken-down or removed version, a suspended plugin's) by its
 * owner or an admin. To anyone else it doesn't exist (404).
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
import { STATE_TTL, cacheKey, cachedBytes, cachedValue } from "./cache";
import { filesKey, packageKey } from "./db";
import { fail, notFound } from "./http";
import { plainPath } from "./paths";
import { MAX_VERSION_LENGTH, parseVersion } from "./semver";
import { hasCredentials, optionalUser, requireAdmin } from "./principal";

export const SANDBOX = "sandbox; default-src 'none'";

/** Clients and shared caches keep a public package or picture no longer
 * than the state behind it. */
const PUBLIC_CACHE = `public, max-age=${STATE_TTL}`;

const attachmentName = (name: string): string => name.replace(/[^A-Za-z0-9._-]/g, "_").slice(0, 120) || "file";

const baseHeaders = (cacheControl: string): Headers =>
  new Headers({
    "x-content-type-options": "nosniff",
    "content-security-policy": SANDBOX,
    "cache-control": cacheControl
  });

/** A URL segment, decoded (versions may carry `+`). */
export const decodeSegment = (segment: string): string | undefined => {
  try {
    return decodeURIComponent(segment);
  } catch {
    return undefined;
  }
};

/** A version named in a URL segment: decoded, and a version the store
 * takes (so at most 128 characters), before it keys anything. */
const versionSegment = (segment: string): string | undefined => {
  const text = segment.length <= 3 * MAX_VERSION_LENGTH ? decodeSegment(segment) : undefined;
  return text !== undefined && parseVersion(text) ? text : undefined;
};

/** Whether the caller owns the plugin or is an admin. */
const privileged = async (request: Request, env: Env, owner: string): Promise<boolean> => {
  const caller = await optionalUser(request, env);
  if (!caller) return false;
  if (caller.userId === owner) return true;
  return !((await requireAdmin(request, env)) instanceof Response);
};

/** What decides who may read a version's package. */
interface PackageState {
  readonly public: boolean;
  readonly owner: string;
  readonly version: string;
  readonly sha256: string;
}

export const packageStateKey = (id: string, version: string): string => cacheKey(["state", "package", id, version]);
export const packageBytesKey = (sha256: string): string => cacheKey(["bytes", "package", sha256]);

const PUBLIC_SQL = "p.status = 'active' AND v.status IN ('listed', 'yanked')";

export const servePackage = async (request: Request, env: Env, db: D1Database, bucket: R2Bucket, id: string, segment: string): Promise<Response> => {
  const versionText = versionSegment(segment);
  if (!versionText) return notFound();
  const state = await cachedValue<PackageState>(request, packageStateKey(id, versionText), STATE_TTL, async () => {
    const row = await db
      .prepare(`SELECT ${PUBLIC_SQL} AS public, p.owner_id, v.version, v.package_sha256 FROM versions v JOIN plugins p ON p.id = v.plugin_id WHERE v.plugin_id = ? AND v.version = ?`)
      .bind(id, versionText)
      .first<{ public: number; owner_id: string; version: string; package_sha256: string }>();
    return row ? { public: row.public === 1, owner: row.owner_id, version: row.version, sha256: row.package_sha256 } : undefined;
  });
  if (!state) return notFound();
  const isPublic = state.public;
  if (!isPublic && !(await privileged(request, env, state.owner))) return notFound();
  const read = async () => {
    const object = await bucket.get(packageKey(id, state.version, state.sha256));
    return object ? new Uint8Array(await object.arrayBuffer()) : undefined;
  };
  const bytes = isPublic ? await cachedBytes(request, packageBytesKey(state.sha256), read) : await read();
  if (!bytes) return fail(503, "unavailable", "The package can't be read right now.");
  const headers = baseHeaders(isPublic && !hasCredentials(request) ? PUBLIC_CACHE : "private, no-store");
  headers.set("content-type", "application/octet-stream");
  headers.set("content-disposition", `attachment; filename="${attachmentName(`${id}-${state.version}`)}.zeron-plugin"`);
  headers.set("content-length", String(bytes.length));
  headers.set("etag", `"${state.sha256}"`);
  return new Response(request.method === "HEAD" ? null : bytes, { headers });
};

/** What decides who may read one packaged file, and where its bytes are. */
interface FileState {
  readonly public: boolean;
  readonly owner: string;
  readonly version: string;
  readonly packageSha256: string;
  readonly at: number;
  readonly bytes: number;
  readonly sha256: string;
  readonly kind: "png" | "jpeg" | "svg" | "text" | "binary";
  readonly icon: boolean;
}

export const fileStateKey = (id: string, version: string, path: string): string => cacheKey(["state", "file", id, version, path]);
const fileBytesKey = (sha256: string, kind: string): string => cacheKey(["bytes", "file", sha256, kind]);

export const serveFile = async (
  request: Request,
  env: Env,
  db: D1Database,
  bucket: R2Bucket,
  id: string,
  segment: string,
  pathSegments: readonly string[]
): Promise<Response> => {
  const versionText = versionSegment(segment);
  if (!versionText) return notFound();
  const decoded = pathSegments.map(decodeSegment);
  if (decoded.some((p) => p === undefined)) return fail(400, "bad_path", "That isn't a file's path.");
  const path = decoded.join("/");
  if (!plainPath(path)) return fail(400, "bad_path", "That isn't a file's path.");
  const state = await cachedValue<FileState>(request, fileStateKey(id, versionText, path), STATE_TTL, async () => {
    const row = await db
      .prepare(
        `SELECT ${PUBLIC_SQL} AS public, p.owner_id, v.version, v.package_sha256, v.icon, f.at, f.bytes, f.sha256, f.kind
         FROM version_files f JOIN versions v ON v.plugin_id = f.plugin_id AND v.version = f.version JOIN plugins p ON p.id = f.plugin_id
         WHERE f.plugin_id = ? AND f.version = ? AND f.path = ?`
      )
      .bind(id, versionText, path)
      .first<{ public: number; owner_id: string; version: string; package_sha256: string; icon: string; at: number; bytes: number; sha256: string; kind: FileState["kind"] }>();
    if (!row) return undefined;
    return {
      public: row.public === 1,
      owner: row.owner_id,
      version: row.version,
      packageSha256: row.package_sha256,
      at: row.at,
      bytes: row.bytes,
      sha256: row.sha256,
      kind: row.kind,
      icon: row.icon === path
    };
  });
  if (!state) return notFound();
  const pictureKind = state.kind === "png" || state.kind === "jpeg" || (state.kind === "svg" && state.icon);
  const readRaw = async (): Promise<Uint8Array | undefined> => {
    if (state.bytes === 0) return new Uint8Array(0);
    const object = await bucket.get(filesKey(id, state.version, state.packageSha256), { range: { offset: state.at, length: state.bytes } });
    return object ? new Uint8Array(await object.arrayBuffer()) : undefined;
  };
  /** A picture as image-check.ts rebuilds it; the icon as it is; else none. */
  const readPicture = async (): Promise<Uint8Array | undefined> => {
    const raw = await readRaw();
    if (!raw || state.kind === "svg") return raw;
    // What isn't the picture it claims to be is never shown as one.
    return cleanImage(state.kind === "png" ? "image/png" : "image/jpeg", raw);
  };
  // Who may read: anyone, a public version's picture; else its owner and
  // admins only (to anyone else it doesn't exist).
  const isPublic = pictureKind && state.public;
  const mayRead = isPublic || (await privileged(request, env, state.owner));
  if (!mayRead) return notFound();
  if (pictureKind) {
    const bytes = isPublic ? await cachedBytes(request, fileBytesKey(state.sha256, state.kind), readPicture) : await readPicture();
    if (bytes) {
      const headers = baseHeaders(isPublic && !hasCredentials(request) ? PUBLIC_CACHE : "private, no-store");
      headers.set("etag", `"${state.sha256}"`);
      headers.set("content-type", state.kind === "png" ? "image/png" : state.kind === "jpeg" ? "image/jpeg" : "image/svg+xml");
      return new Response(request.method === "HEAD" ? null : bytes, { headers });
    }
  }
  // Anything else, a picture that won't clean included: its owner's and
  // admins' only, and only as an attachment.
  if (isPublic && !(await privileged(request, env, state.owner))) return notFound();
  const raw = await readRaw();
  if (!raw) return fail(503, "unavailable", "The file can't be read right now.");
  const headers = baseHeaders("private, no-store");
  headers.set("etag", `"${state.sha256}"`);
  headers.set("content-type", "application/octet-stream");
  headers.set("content-disposition", `attachment; filename="${attachmentName(path.split("/").pop()!)}"`);
  return new Response(request.method === "HEAD" ? null : raw, { headers });
};

/** The state keys a change to a version moves: its package's and its
 * public pictures'. (The bytes behind them never change.) */
export const versionFileKeys = async (db: D1Database, id: string, version: string): Promise<string[]> => {
  const rows = await db
    .prepare(
      `SELECT path FROM version_files WHERE plugin_id = ? AND version = ?
         AND (kind IN ('png', 'jpeg') OR path = (SELECT icon FROM versions WHERE plugin_id = ? AND version = ?))`
    )
    .bind(id, version, id, version)
    .all<{ path: string }>();
  return [packageStateKey(id, version), ...rows.results.map((r) => fileStateKey(id, version, r.path))];
};
