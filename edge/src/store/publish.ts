/**
 * `POST /store/v1/plugins/:id/versions`, body: the `.zeron-plugin` file.
 * → 201 { id, version, status, checks }
 *
 * In order: who publishes (a session or a deploy key scoped to this id),
 * that they have a username, the body's size, the package exactly as a
 * device verifies it (package.ts), what the store needs to list it
 * (listing.ts), that the key that signed it may sign for this account, the
 * id (owned by them, or free and not reserved), the version (new, and later
 * than the versions it follows), the rate. Then the automated checks
 * (checks.ts), stored with the version.
 *
 * Storing it can't leave half a version, and two publishes at once can't
 * both win: the package and the version's files (one object, every file in
 * path order) go to R2 first under keys named by the package's sha256, then
 * one D1 batch claims the id and inserts the version, guarded by every rule
 * above, so whichever insert lands first wins and the other is answered as
 * it would have been a moment later. A loser deletes its own two objects.
 * The same bytes again answer as the first time did.
 */
import type { Env } from "../env";
import { isReservedUsername, skeleton } from "../username";
import { type FileKind, fileKind, runChecks } from "./checks";
import { type ConsentItem, consentItem } from "./consent";
import { VERSION_ORDER, filesKey, packageKey, pluginRow } from "./db";
import { fail, answer, rateLimited } from "./http";
import { readListing } from "./listing";
import { PackageRefusal, verifyPackage } from "./package";
import { type Publisher, authenticatePublisher, readBody, signingKeyAllowed } from "./principal";
import { isPrerelease, preKey, precedenceText } from "./semver";

/**
 * The largest package the store takes, in bytes. Measured, not guessed:
 * scripts/store-package-limit.mjs publishes packages of growing size to a
 * local workerd and records its peak memory and CPU per publish
 * (docs/plugin-store-limits.md). Below the engine's own 50 MB of files.
 */
export const STORE_MAX_PACKAGE_BYTES = 20 * 1024 * 1024;

/** Versions an account may submit, across its plugins. */
export const PUBLISHES_PER_HOUR = 10;
export const PUBLISHES_PER_DAY = 50;

/** Built-in plugins' ids (the app ships them), beside the engine's own
 * reserved ones (manifest.ts `RESERVED_IDS`, refused as a bad id). */
export const BUILTIN_IDS = ["git", "github", "parallax-scenes"];

const HOUR = 3_600_000;
const DAY = 86_400_000;

/** Whether `userId` may take the new id `id`: an operator's reservation
 * decides first; then built-in ids and the username rules' brand, staff and
 * lookalike words (username.ts) are kept back. */
export const idReserved = async (db: D1Database, id: string, userId: string): Promise<boolean> => {
  const held = await db.prepare("SELECT user_id FROM reservations WHERE id = ?").bind(id).first<{ user_id: string | null }>();
  if (held) return held.user_id !== userId;
  return BUILTIN_IDS.includes(id) || isReservedUsername(id);
};

/** A write by `userId` within the per-account limit (STORE_WRITES). */
export const allowWrite = async (env: Env, userId: string): Promise<boolean> =>
  !env.STORE_WRITES || (await env.STORE_WRITES.limit({ key: `user:${userId}` })).success;

const refusal = (e: PackageRefusal): Response => {
  switch (e.code) {
    case "too_big":
      return fail(413, "too_large", e.message, { reason: e.code });
    case "unsigned":
    case "tampered":
    case "bad_signature":
      return fail(400, "bad_signature", e.message, { reason: e.code });
    default:
      return fail(400, "bad_package", e.message, { reason: e.code });
  }
};

const accepted = (id: string, version: string, status: string, checks: unknown): Response =>
  answer({ id, version, status, checks }, 201);

export const handlePublish = async (request: Request, env: Env, db: D1Database, bucket: R2Bucket, id: string): Promise<Response> => {
  const publisher = await authenticatePublisher(request, env);
  if (publisher instanceof Response) return publisher;
  if (!(await allowWrite(env, publisher.userId))) return rateLimited();
  if (publisher.via === "deploy_key" && publisher.pluginScope !== id) {
    return fail(403, "key_not_allowed", "This deploy key is for another plugin.");
  }
  if (!publisher.username) return fail(403, "needs_username", "Publishing needs a username: every listing reads \"by @username\".");

  const body = await readBody(request, STORE_MAX_PACKAGE_BYTES);
  if (!body) return fail(413, "too_large", `The store takes packages up to ${STORE_MAX_PACKAGE_BYTES / (1024 * 1024)} MB.`);
  let pkg;
  try {
    pkg = await verifyPackage(body, STORE_MAX_PACKAGE_BYTES);
  } catch (e) {
    if (e instanceof PackageRefusal) return refusal(e);
    throw e;
  }
  if (pkg.manifest.id !== id) return fail(400, "bad_package", `The package is "${pkg.manifest.id}", not "${id}".`, { reason: "id_mismatch" });
  const listing = readListing(pkg.manifest, pkg.files);
  if ("error" in listing) return fail(listing.error === "too_large" ? 413 : 400, listing.error, listing.message);
  if (!(await signingKeyAllowed(env, publisher, id, pkg.key))) {
    return fail(403, "key_not_allowed", "The key that signed this package isn't one of this account's.");
  }

  const plugin = await pluginRow(db, id);
  if (plugin && plugin.owner_id !== publisher.userId) return fail(403, "id_taken", `"${id}" belongs to another account.`);
  if (plugin?.status === "suspended") return fail(403, "suspended", "This plugin is suspended: it can't take new versions.");
  if (!plugin && (await idReserved(db, id, publisher.userId))) return fail(403, "id_reserved", `"${id}" is kept back: ask Zeron for it.`);

  const version = listing.version;
  const precedence = precedenceText(version);
  const existing = await db
    .prepare("SELECT version, status, package_sha256, checks FROM versions WHERE plugin_id = ? AND precedence = ?")
    .bind(id, precedence)
    .first<{ version: string; status: string; package_sha256: string; checks: string }>();
  if (existing) {
    if (existing.package_sha256 === pkg.sha256) return accepted(id, existing.version, existing.status, JSON.parse(existing.checks));
    return fail(409, "version_exists", `Version ${existing.version} was already submitted, with other contents. Submit a new version.`);
  }
  const order = [version.major, version.minor, version.patch, isPrerelease(version) ? 0 : 1, preKey(version.pre)] as const;
  const blocking = await notNewerThan(db, id, order);
  if (blocking) return fail(409, "not_newer", `Version ${version.text} isn't later than ${blocking}.`);
  const now = Date.now();
  if (await overRate(db, publisher.userId, now)) return fail(429, "rate_limited", "Too many versions submitted lately. Try again later.");

  const base = await db
    .prepare(`SELECT version, permissions FROM versions WHERE plugin_id = ? AND status IN ('approved', 'yanked') ORDER BY ${VERSION_ORDER} LIMIT 1`)
    .bind(id)
    .first<{ version: string; permissions: string }>();
  const previous = await db
    .prepare(`SELECT version FROM versions WHERE plugin_id = ? AND status NOT IN ('rejected', 'withdrawn') ORDER BY ${VERSION_ORDER} LIMIT 1`)
    .bind(id)
    .first<{ version: string }>();
  const kinds = new Map<string, FileKind>([...pkg.files].map(([path, bytes]) => [path, fileKind(path, bytes)]));
  const consent = consentItem(pkg.manifest);
  const checks = runChecks({
    pkg,
    listing,
    kinds,
    via: publisher.via,
    consent,
    base: base ? { version: base.version, consent: JSON.parse(base.permissions) as ConsentItem } : null,
    previous: previous?.version ?? null
  });

  const pkgKey = packageKey(id, version.text, pkg.sha256);
  const blobKey = filesKey(id, version.text, pkg.sha256);
  await Promise.all([
    bucket.put(pkgKey, body, { sha256: pkg.sha256, httpMetadata: { contentType: "application/octet-stream" } }),
    bucket.put(blobKey, pkg.blob, { httpMetadata: { contentType: "application/octet-stream" } })
  ]);

  const files: [string, number, number, string, FileKind][] = [];
  let at = 0;
  for (const [path, bytes] of pkg.files) {
    files.push([path, at, bytes.length, pkg.hashes.get(path)!, kinds.get(path)!]);
    at += bytes.length;
  }
  const statements: D1PreparedStatement[] = [
    db.prepare("INSERT INTO plugins (id, owner_id, skeleton, created_at) VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING").bind(id, publisher.userId, skeleton(id), now),
    insertVersion(db, publisher, id, now, {
      version: version.text,
      precedence,
      order,
      sha256: pkg.sha256,
      bytes: pkg.bytes,
      key: pkg.key,
      hash: pkg.hash,
      filesCount: pkg.files.size,
      filesBytes: at,
      manifest: pkg.manifestCanonical,
      listing,
      consent,
      checks
    }),
    ...fileRows(db, id, version.text, pkg.sha256, files)
  ];
  await db.batch(statements);

  const landed = await db
    .prepare("SELECT version, status, package_sha256 FROM versions WHERE plugin_id = ? AND precedence = ?")
    .bind(id, precedence)
    .first<{ version: string; status: string; package_sha256: string }>();
  if (landed?.package_sha256 === pkg.sha256) return accepted(id, landed.version, landed.status, checks);

  // Lost: to another publish of this version, or to a change since the checks above.
  await Promise.all([bucket.delete(pkgKey), bucket.delete(blobKey)]);
  if (landed) return fail(409, "version_exists", `Version ${landed.version} was already submitted, with other contents. Submit a new version.`);
  const now2 = await pluginRow(db, id);
  if (!now2) return fail(403, "id_reserved", `"${id}" is too close to the id of a plugin already in the store.`);
  if (now2.owner_id !== publisher.userId) return fail(403, "id_taken", `"${id}" belongs to another account.`);
  if (now2.status === "suspended") return fail(403, "suspended", "This plugin is suspended: it can't take new versions.");
  if (await overRate(db, publisher.userId, Date.now())) return fail(429, "rate_limited", "Too many versions submitted lately. Try again later.");
  const later = await notNewerThan(db, id, order);
  return fail(409, "not_newer", `Version ${version.text} isn't later than ${later ?? "a version submitted just now"}.`);
};

type Order = readonly [number, number, number, number, string];

/** The highest version `order` must follow and doesn't, if any: a release
 * comes after every release still standing, a pre-release after every
 * version still standing (rejected and withdrawn ones don't stand). */
const NOT_NEWER_SQL = `plugin_id = ? AND status NOT IN ('rejected', 'withdrawn') AND (? = 0 OR is_release = 1)
  AND (major, minor, patch, is_release, pre_key) >= (?, ?, ?, ?, ?)`;

const notNewerThan = async (db: D1Database, id: string, order: Order): Promise<string | undefined> => {
  const row = await db
    .prepare(`SELECT version FROM versions WHERE ${NOT_NEWER_SQL} ORDER BY ${VERSION_ORDER} LIMIT 1`)
    .bind(id, order[3], ...order)
    .first<{ version: string }>();
  return row?.version;
};

const RATE_SQL = `(SELECT COUNT(*) FROM versions WHERE publisher_id = ? AND submitted_at > ?) < ?
  AND (SELECT COUNT(*) FROM versions WHERE publisher_id = ? AND submitted_at > ?) < ?`;
const rateBinds = (userId: string, now: number) => [userId, now - HOUR, PUBLISHES_PER_HOUR, userId, now - DAY, PUBLISHES_PER_DAY];

const overRate = async (db: D1Database, userId: string, now: number): Promise<boolean> =>
  (await db.prepare(`SELECT ${RATE_SQL} AS ok`).bind(...rateBinds(userId, now)).first<{ ok: number }>())?.ok !== 1;

interface NewVersion {
  readonly version: string;
  readonly precedence: string;
  readonly order: Order;
  readonly sha256: string;
  readonly bytes: number;
  readonly key: string;
  readonly hash: string;
  readonly filesCount: number;
  readonly filesBytes: number;
  readonly manifest: string;
  readonly listing: Exclude<ReturnType<typeof readListing>, { error: string }>;
  readonly consent: ConsentItem;
  readonly checks: unknown;
}

/** The version's insert, guarded by every rule a publish is held to, so
 * the guard and the insert are one step. */
const insertVersion = (db: D1Database, publisher: Publisher, id: string, now: number, v: NewVersion): D1PreparedStatement => {
  const l = v.listing;
  const columns: [string, unknown][] = [
    ["plugin_id", id],
    ["version", v.version],
    ["precedence", v.precedence],
    ["major", v.order[0]],
    ["minor", v.order[1]],
    ["patch", v.order[2]],
    ["is_release", v.order[3]],
    ["pre_key", v.order[4]],
    ["status", "pending"],
    ["publisher_id", publisher.userId],
    ["via", publisher.via],
    ["signer_key", v.key],
    ["content_hash", v.hash],
    ["package_sha256", v.sha256],
    ["package_bytes", v.bytes],
    ["files_count", v.filesCount],
    ["files_bytes", v.filesBytes],
    ["manifest", v.manifest],
    ["name", l.name],
    ["summary", l.summary],
    ["icon", l.icon],
    ["categories", JSON.stringify(l.categories)],
    ["keywords", JSON.stringify(l.keywords)],
    ["homepage", l.homepage],
    ["repository", l.repository],
    ["license", l.license],
    ["screenshots", JSON.stringify(l.screenshots)],
    ["engines_zeron", l.engines],
    ["permissions", JSON.stringify(v.consent)],
    ["readme", l.readme],
    ["notes", l.notes],
    ["checks", JSON.stringify(v.checks)],
    ["submitted_at", now]
  ];
  return db
    .prepare(
      `INSERT INTO versions (${columns.map(([c]) => c).join(", ")})
       SELECT ${columns.map(() => "?").join(", ")}
       WHERE EXISTS (SELECT 1 FROM plugins WHERE id = ? AND owner_id = ? AND status = 'active')
         AND NOT EXISTS (SELECT 1 FROM versions WHERE ${NOT_NEWER_SQL})
         AND ${RATE_SQL}
       ON CONFLICT DO NOTHING`
    )
    .bind(...columns.map(([, value]) => value), id, publisher.userId, id, v.order[3], ...v.order, ...rateBinds(publisher.userId, now));
};

/** The version's file rows, in statements of a bounded size, each written
 * only for the version these bytes made. */
const fileRows = (db: D1Database, id: string, version: string, sha256: string, files: readonly (readonly [string, number, number, string, FileKind])[]): D1PreparedStatement[] => {
  const statements: D1PreparedStatement[] = [];
  let chunk: (readonly [string, number, number, string, FileKind])[] = [];
  let size = 0;
  const flush = () => {
    if (chunk.length === 0) return;
    statements.push(
      db
        .prepare(
          `INSERT INTO version_files (plugin_id, version, path, at, bytes, sha256, kind)
           SELECT ?1, ?2, json_extract(value, '$[0]'), json_extract(value, '$[1]'), json_extract(value, '$[2]'),
                  json_extract(value, '$[3]'), json_extract(value, '$[4]')
           FROM json_each(?3)
           WHERE (SELECT package_sha256 FROM versions WHERE plugin_id = ?1 AND version = ?2) = ?4
           ON CONFLICT DO NOTHING`
        )
        .bind(id, version, JSON.stringify(chunk), sha256)
    );
    chunk = [];
    size = 0;
  };
  for (const file of files) {
    const entry = JSON.stringify(file).length;
    if (size + entry > 256 * 1024) flush();
    chunk.push(file);
    size += entry;
  }
  flush();
  return statements;
};
