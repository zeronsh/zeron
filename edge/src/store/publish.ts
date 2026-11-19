/**
 * `POST /store/v1/plugins/:id/versions`, body: the `.zeron-plugin` file.
 * → 201 { id, version, status: "listed", verified: false, checks }
 *
 * A version that passes is listed and installable by everyone at once
 * (amendment 45); a reviewer's decision later only marks it verified.
 *
 * In order: who publishes (a signed-in device, or a deploy token whose scope
 * holds this id, an id its account owns already: a token never claims one),
 * that they have a username, the body's size, the package exactly as a
 * device verifies it (package.ts), what the store needs to list it
 * (listing.ts, the engine's rules), that the key that signed it may sign
 * for this account, the id (owned by them, or free and not reserved), the
 * version (new, and later than every version the plugin has, in any
 * state), the rate. Then the automated checks (checks.ts), stored with the
 * version.
 *
 * Storing it can't leave half a version, and two publishes at once can't
 * both win: the package and the version's files (one object, every file in
 * path order) go to R2 first under keys named by the package's sha256, then
 * one D1 batch claims the id, inserts the version (guarded by every rule
 * above, so whichever insert lands first wins and the other is answered as
 * it would have been a moment later), logs it, takes the plugin's earlier
 * undecided versions out of the review queue, and relists the plugin. Then
 * this colo's cached catalog pages are purged. The same bytes again answer
 * as the first time did.
 */
import type { Env } from "../env";
import { isReservedUsername, skeleton } from "../username";
import { type FileKind, fileKind, isText, runChecks } from "./checks";
import { type ConsentItem, consentItem } from "./consent";
import { VERSION_ORDER, auditIfChanged, filesKey, packageKey, pluginRow, relist } from "./db";
import { purgeCatalog } from "./purge";
import { fail, answer, rateLimited } from "./http";
import { type Listing, type Problem, checkListing, listingOf } from "./listing";
import { PackageRefusal, verifyPackage } from "./package";
import { MAX_PUBLISH_BODY_BYTES, type Publisher, authenticatePublisher, invalidToken, readBody, signingKeyAllowed, tokenInScope, tokenNotAllowed } from "./principal";
import { userDirectoryStub } from "../user-directory";
import { isPrerelease, preKey, precedenceText } from "./semver";

/**
 * The largest package the store takes, in bytes. Measured, not guessed:
 * scripts/store-package-limit.mjs publishes packages of growing size to a
 * local workerd and records its peak memory and CPU per publish
 * (docs/plugin-store-limits.md): about 60 MB at its peak, 0.3 s of CPU.
 * Below the engine's own 50 MB of files.
 */
export const STORE_MAX_PACKAGE_BYTES = MAX_PUBLISH_BODY_BYTES;

/** Versions an account may submit, across its plugins. */
export const PUBLISHES_PER_HOUR = 10;
export const PUBLISHES_PER_DAY = 50;
/** New ids an account may claim in any 7 days (amendment 30). */
export const CLAIMS_PER_WEEK = 5;
const WEEK = 7 * 86_400_000;

const CLAIMS_SQL = "(SELECT COUNT(*) FROM id_claims WHERE user_id = ? AND at > ?) < ?";
/** The same, numbered for the claim's insert (?2 the user, ?5 the window, ?6 the limit). */
const CLAIMS_SQL_AT = "(SELECT COUNT(*) FROM id_claims WHERE user_id = ?2 AND at > ?5) < ?6";

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

/** How many installs make an id one a new claim must not be confused with. */
export const SIMILAR_INSTALLS = 100;

/** Whether `a` and `b` are at most one edit apart (an insertion, a
 * deletion or a substitution). */
export const oneEditApart = (a: string, b: string): boolean => {
  if (a === b) return true;
  const [s, t] = a.length <= b.length ? [a, b] : [b, a];
  if (t.length - s.length > 1) return false;
  let i = 0;
  while (i < s.length && s[i] === t[i]) i++;
  // Past the first difference the rest must match: shifted by one when the
  // lengths differ, aligned when one letter was swapped.
  return s.length === t.length ? s.slice(i + 1) === t.slice(i + 1) : s.slice(i) === t.slice(i + 1);
};

/**
 * The popular id `id` would be confused with, if any (contract: "Names too
 * like another's"): one edit, or one confusable letter, from an id another
 * account owns with `SIMILAR_INSTALLS` installs or more, compared on the
 * username rules' skeleton (username.ts, the lookalike table brands are
 * kept by). Checked only when an id is claimed; an id an admin reserved
 * for the claimer passes. Costs one indexed read of the popular ids within
 * one letter of its length, and a linear compare of each.
 */
export const tooSimilar = async (db: D1Database, id: string, userId: string): Promise<string | undefined> => {
  const held = await db.prepare("SELECT 1 FROM reservations WHERE id = ? AND user_id = ?").bind(id, userId).first();
  if (held) return undefined;
  const mine = skeleton(id);
  const rows = (
    await db
      .prepare("SELECT id, skeleton FROM plugins WHERE installs >= ? AND owner_id != ? AND length(skeleton) BETWEEN ? AND ?")
      .bind(SIMILAR_INSTALLS, userId, mine.length - 1, mine.length + 1)
      .all<{ id: string; skeleton: string }>()
  ).results;
  return rows.find((r) => oneEditApart(mine, r.skeleton))?.id;
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

/** A listing problem as the publish error: its rule word is the error word
 * (`id_reserved` 403, `too_large` 413, the rest 400), with every problem
 * found, so one upload shows them all. */
/** The why and where of a bad name, description or keywords
 * (`listing_causes`), when the problem has them. */
const causeOf = (p: Problem) => ({ ...(p.cause === undefined ? {} : { cause: p.cause }), ...(p.word === undefined ? {} : { word: p.word }) });

const listingRefusal = (first: Problem, all: readonly Problem[]): Response =>
  fail(first.rule === "id_reserved" ? 403 : first.rule === "too_large" ? 413 : 400, first.rule, first.words, {
    field: first.field,
    path: first.path,
    ...causeOf(first),
    problems: all.slice(0, 100).map((p) => ({ rule: p.rule, field: p.field, path: p.path, ...causeOf(p), message: p.words }))
  });

/** The publish answer: the version, and the older unverified versions it
 * superseded (now yanked by it, for good). */
const accepted = (id: string, version: string, status: string, verified: boolean, checks: unknown, superseded: readonly string[] = []): Response =>
  answer({ id, version, status, verified, checks, superseded }, 201);

export const handlePublish = async (request: Request, env: Env, db: D1Database, bucket: R2Bucket, id: string): Promise<Response> => {
  const publisher = await authenticatePublisher(request, env, { publishing: true });
  if (publisher instanceof Response) return publisher;
  const res = await publishAs(publisher, request, env, db, bucket, id);
  // A deploy token's last use is a version it published (or the same one
  // again), never an attempt refused.
  if (res.status === 201 && publisher.token) await userDirectoryStub(env)?.deployTokenUsed(publisher.token.id, id);
  return res;
};

const publishAs = async (publisher: Publisher, request: Request, env: Env, db: D1Database, bucket: R2Bucket, id: string): Promise<Response> => {
  if (!(await allowWrite(env, publisher.userId))) return rateLimited();
  // Who may publish what, before the body is read: a deploy token only a
  // plugin its account owns now that its scope holds, never a first version;
  // a live token asking for any other is 403 `token_not_allowed`.
  const plugin = await pluginRow(db, id);
  if (publisher.via === "deploy_token" && (!plugin || plugin.owner_id !== publisher.userId || !tokenInScope(publisher, plugin))) return tokenNotAllowed();
  if (!publisher.username) return fail(403, "needs_username", "Publishing needs a username: every listing reads \"by @username\".");

  const body = await readBody(request, STORE_MAX_PACKAGE_BYTES);
  if (body === "too_large") return fail(413, "too_large", `The store takes packages up to ${STORE_MAX_PACKAGE_BYTES / (1024 * 1024)} MB.`);
  let pkg;
  try {
    // From a device, signed; with a deploy token, not (contract a315eef5a).
    pkg = await verifyPackage(body, STORE_MAX_PACKAGE_BYTES, publisher.via === "session");
  } catch (e) {
    if (e instanceof PackageRefusal) return refusal(e);
    throw e;
  }
  if (pkg.manifest.id !== id) return fail(400, "bad_package", `The package is "${pkg.manifest.id}", not "${id}".`, { reason: "id_mismatch" });
  // Who may publish this, before any work on what it is: a stranger's key,
  // someone else's id, a suspended plugin are refused here. From a device,
  // the package is signed by one of the account's live keys; with a deploy
  // token, the token is the publisher's word (contract a315eef5a).
  if (publisher.via === "session" && !(await signingKeyAllowed(env, publisher, pkg.key!))) {
    return fail(403, "key_not_allowed", "The key that signed this package isn't one of this account's.");
  }
  if (plugin && plugin.owner_id !== publisher.userId) return fail(403, "id_taken", `"${id}" belongs to another account.`);
  if (plugin?.status === "suspended") return fail(403, "suspended", "This plugin is suspended: it can't take new versions.");
  if (!plugin && (await idReserved(db, id, publisher.userId))) return fail(403, "id_reserved", `"${id}" is kept back: ask Zeron for it.`);
  if (!plugin) {
    const near = await tooSimilar(db, id, publisher.userId);
    if (near) return fail(403, "id_too_similar", `"${id}" is too like "${near}", another publisher's plugin. Choose another id, or ask Zeron for this one.`, { similar_to: near });
  }
  if (!plugin && (await overClaims(db, publisher.userId, Date.now()))) return claimsRefusal();

  // The engine's listing rules (what `zeron plugin check` says on the
  // device), then the store's own size limits.
  const { problems, warnings } = checkListing(pkg.manifest, pkg.files, isText);
  if (problems.length > 0) return listingRefusal(problems[0]!, problems);
  const listing = listingOf(pkg.manifest, pkg.files);
  if ("rule" in listing) return listingRefusal(listing, [listing]);

  const version = listing.version;
  const precedence = precedenceText(version);
  const existing = await db
    .prepare("SELECT version, status, verification, package_sha256, checks FROM versions WHERE plugin_id = ? AND precedence = ?")
    .bind(id, precedence)
    .first<{ version: string; status: string; verification: string; package_sha256: string; checks: string }>();
  if (existing) {
    if (existing.package_sha256 === pkg.sha256) {
      const superseded = (
        await db.prepare(`SELECT version FROM versions WHERE plugin_id = ? AND superseded_by = ? AND yanked_by = 'superseded' ORDER BY ${VERSION_ORDER}`).bind(id, existing.version).all<{ version: string }>()
      ).results.map((r) => r.version);
      return accepted(id, existing.version, existing.status, existing.verification === "verified", JSON.parse(existing.checks), superseded);
    }
    return fail(409, "version_exists", `Version ${existing.version} was already published, with other contents. Publish a new version.`);
  }
  const order = [version.major, version.minor, version.patch, isPrerelease(version) ? 0 : 1, preKey(version.pre)] as const;
  const blocking = await notNewerThan(db, id, order);
  if (blocking) return fail(409, "not_newer", `Version ${version.text} isn't later than ${blocking}.`);
  const now = Date.now();
  if (await overRate(db, publisher.userId, now)) return fail(429, "rate_limited", "Too many versions submitted lately. Try again later.");

  // What it asks, against what a reviewer last verified of it.
  const base = await db
    .prepare(`SELECT version, permissions, manifest FROM versions WHERE plugin_id = ? AND verification = 'verified' AND status IN ('listed', 'yanked') ORDER BY ${VERSION_ORDER} LIMIT 1`)
    .bind(id)
    .first<{ version: string; permissions: string; manifest: string }>();
  const previous = await db
    .prepare(`SELECT version FROM versions WHERE plugin_id = ? ORDER BY ${VERSION_ORDER} LIMIT 1`)
    .bind(id)
    .first<{ version: string }>();
  const kinds = new Map<string, FileKind>([...pkg.files].map(([path, bytes]) => [path, fileKind(path, bytes)]));
  const consent = consentItem(pkg.manifest);
  const checks = runChecks({
    pkg,
    listing,
    warnings,
    kinds,
    consent,
    base: base ? { version: base.version, consent: JSON.parse(base.permissions) as ConsentItem, manifest: base.manifest } : null,
    previous: previous?.version ?? null
  });

  const pkgKey = packageKey(id, version.text, pkg.sha256);
  const blobKey = filesKey(id, version.text, pkg.sha256);
  // R2 first (content-addressed: harmless if this publish loses), then the
  // one D1 batch that decides. Whatever throws in between, nothing written
  // here is left behind unless a version row names it.
  //
  // Before the objects are written, their orphan records are made fresh:
  // the sweep (release.ts `collectOrphans`) never deletes an object whose
  // record is younger than its grace, so a retry of bytes an earlier attempt
  // left behind can't lose them to it while its batch is on the way. The
  // batch clears the records once a row names the objects.
  await recordObjects(db, [pkgKey, blobKey]);
  const writes: readonly (readonly [string, () => Promise<unknown>])[] = [
    [pkgKey, () => bucket.put(pkgKey, body, { sha256: pkg.sha256, httpMetadata: { contentType: "application/octet-stream" } })],
    [blobKey, () => bucket.put(blobKey, pkg.blob, { httpMetadata: { contentType: "application/octet-stream" } })]
  ];
  try {
    await Promise.all(writes.map(([, write]) => write()));
    // The key that signed it, or the token that sent it, may have been
    // revoked while the objects were written: asked again, now, so the batch
    // below runs only for one live after the write. (Keys and tokens live in
    // the user directory, which no D1 statement can read, so this is as
    // close to the insert as it gets.)
    const stillLive =
      publisher.via === "session"
        ? await signingKeyAllowed(env, publisher, pkg.key!)
        : ((await userDirectoryStub(env)?.deployTokenLive(publisher.token!.id)) ?? false);
    if (!stillLive) {
      await maybeOrphaned(db, [pkgKey, blobKey]);
      return publisher.via === "session"
        ? fail(403, "key_not_allowed", "The key that signed this package was revoked while it was being published.")
        : invalidToken();
    }

    const files: [string, number, number, string, FileKind][] = [];
    let at = 0;
    for (const [path, bytes] of pkg.files) {
      files.push([path, at, bytes.length, pkg.hashes.get(path)!, kinds.get(path)!]);
      at += bytes.length;
    }
    const statements: D1PreparedStatement[] = [
      // A new id is claimed within the weekly limit, and the claim recorded.
      db
        .prepare(`INSERT INTO plugins (id, owner_id, owner_since, skeleton, created_at) SELECT ?1, ?2, ?4, ?3, ?4 WHERE ${CLAIMS_SQL_AT} ON CONFLICT DO NOTHING`)
        .bind(id, publisher.userId, skeleton(id), now, now - WEEK, CLAIMS_PER_WEEK),
      db.prepare("INSERT INTO id_claims (user_id, plugin_id, at) SELECT ?, ?, ? WHERE changes() = 1").bind(publisher.userId, id, now),
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
      // A public act: in the log, by its author (amendment 45).
      // (A token by its short id: never the secret.)
      auditIfChanged(db, now, publisher.userId, "version.publish", id, version.text, {
        via: publisher.via === "session" ? "device" : "deploy_token",
        key: pkg.key,
        ...(publisher.token ? { token: publisher.token.short } : {})
      }),
      ...fileRows(db, id, version.text, pkg.sha256, files),
      // Every older unverified version it supersedes stops taking new
      // installs, for good, and leaves the review queue (amendment 45): a
      // release supersedes older releases and pre-releases, a pre-release
      // older pre-releases only. Verified versions are untouched.
      db
        .prepare(
          `UPDATE versions SET status = 'yanked', yanked_by = 'superseded', superseded_at = ?1, superseded_by = ?2
           WHERE plugin_id = ?3 AND version != ?2 AND verification = 'unverified' AND status IN ('listed', 'yanked')
             AND superseded_at IS NULL AND (?5 = 1 OR is_release = 0)
             AND (major, minor, patch, is_release, pre_key) < (?6, ?7, ?8, ?5, ?9)
             AND (SELECT package_sha256 FROM versions WHERE plugin_id = ?3 AND version = ?2) = ?4`
        )
        .bind(now, version.text, id, pkg.sha256, order[3], order[0], order[1], order[2], order[4]),
      ...relist(db, id, publisher.username),
      // A claim whose version didn't land isn't kept: no id is held without a
      // submission, and a refused publish doesn't count towards the limit.
      db
        .prepare("DELETE FROM id_claims WHERE user_id = ? AND plugin_id = ? AND at = ? AND NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?)")
        .bind(publisher.userId, id, now, id),
      db.prepare("DELETE FROM plugins WHERE id = ? AND owner_id = ? AND created_at = ? AND NOT EXISTS (SELECT 1 FROM versions WHERE plugin_id = ?)").bind(id, publisher.userId, now, id),
      // Named by a row now: no longer anyone's to sweep.
      db
        .prepare("DELETE FROM object_orphans WHERE key IN (?, ?) AND EXISTS (SELECT 1 FROM versions WHERE plugin_id = ? AND version = ? AND package_sha256 = ?)")
        .bind(pkgKey, blobKey, id, version.text, pkg.sha256)
    ];
    await db.batch(statements);
  } catch (e) {
    // Never deleted here: a retry of these same bytes wrote these same
    // keys, and its row may yet land. The sweep decides, later.
    await maybeOrphaned(db, [pkgKey, blobKey]);
    throw e;
  }

  const landed = await db
    .prepare("SELECT version, status, package_sha256 FROM versions WHERE plugin_id = ? AND precedence = ?")
    .bind(id, precedence)
    .first<{ version: string; status: string; package_sha256: string }>();
  if (landed?.package_sha256 === pkg.sha256) {
    const superseded = (
      await db
        .prepare(`SELECT version FROM versions WHERE plugin_id = ? AND superseded_by = ? AND yanked_by = 'superseded' ORDER BY ${VERSION_ORDER}`)
        .bind(id, version.text)
        .all<{ version: string }>()
    ).results.map((r) => r.version);
    await purgeCatalog(db, id, [version.text, ...superseded]);
    // A sweep that judged an earlier attempt's record before this publish
    // refreshed it may have deleted an object after this publish wrote it.
    // Now that the row names the objects, write back any that is gone (this
    // publish still has the bytes). A sweep checks for a row before it
    // deletes, so the only delete that can still follow is one already
    // decided and slower than this publish's batch and these HEADs.
    for (const [key, write] of writes) if ((await bucket.head(key)) === null) await write();
    return accepted(id, landed.version, landed.status, false, checks, superseded);
  }

  // Lost: to another publish of this version, or to a change since the
  // checks above. Its objects are the sweep's to delete, once no row names them.
  await maybeOrphaned(db, [pkgKey, blobKey]);
  if (landed) return fail(409, "version_exists", `Version ${landed.version} was already published, with other contents. Publish a new version.`);
  const now2 = await pluginRow(db, id);
  if (!now2 && (await overClaims(db, publisher.userId, Date.now()))) return claimsRefusal();
  if (!now2 && (await db.prepare("SELECT 1 FROM plugins WHERE skeleton = ?").bind(skeleton(id)).first())) {
    return fail(403, "id_reserved", `"${id}" is too close to the id of a plugin already in the store.`);
  }
  if (!now2) return fail(409, "not_newer", "Another publish changed this plugin meanwhile: try again.");
  if (now2.owner_id !== publisher.userId) return fail(403, "id_taken", `"${id}" belongs to another account.`);
  if (now2.status === "suspended") return fail(403, "suspended", "This plugin is suspended: it can't take new versions.");
  if (await overRate(db, publisher.userId, Date.now())) return fail(429, "rate_limited", "Too many versions submitted lately. Try again later.");
  const later = await notNewerThan(db, id, order);
  return fail(409, "not_newer", `Version ${version.text} isn't later than ${later ?? "a version published just now"}.`);
};

/** Record (or refresh) `keys` as objects no row may name yet, now. */
const recordObjects = (db: D1Database, keys: readonly string[]): Promise<unknown> =>
  Promise.all(keys.map((key) => db.prepare("INSERT INTO object_orphans (key, at) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET at = excluded.at").bind(key, Date.now()).run()));

/** Record R2 keys a publish wrote but may not need (release.ts
 * `collectOrphans` deletes each after a grace period, if no version row
 * names it). Best effort: if even this fails, the objects are only garbage. */
const maybeOrphaned = async (db: D1Database, keys: readonly string[]): Promise<void> => {
  const now = Date.now();
  await Promise.all(
    keys.map((key) => db.prepare("INSERT INTO object_orphans (key, at) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET at = excluded.at").bind(key, now).run())
  ).catch((e) => console.error("store: couldn't record possibly orphaned objects", String(e)));
};

const overClaims = async (db: D1Database, userId: string, now: number): Promise<boolean> =>
  (await db.prepare(`SELECT ${CLAIMS_SQL} AS ok`).bind(userId, now - WEEK, CLAIMS_PER_WEEK).first<{ ok: number }>())?.ok !== 1;

const claimsRefusal = (): Response =>
  fail(429, "rate_limited", `An account claims at most ${CLAIMS_PER_WEEK} new ids a week. Try again later, or publish a new version of one you have.`);

type Order = readonly [number, number, number, number, string];

/** The highest version `order` must follow and doesn't, if any: a publish
 * comes after every version the plugin has, in any state (amendment 45: a
 * taken-down or removed version's number is never reused or gone round). */
const NOT_NEWER_SQL = `plugin_id = ? AND (major, minor, patch, is_release, pre_key) >= (?, ?, ?, ?, ?)`;

const notNewerThan = async (db: D1Database, id: string, order: Order): Promise<string | undefined> => {
  const row = await db
    .prepare(`SELECT version FROM versions WHERE ${NOT_NEWER_SQL} ORDER BY ${VERSION_ORDER} LIMIT 1`)
    .bind(id, ...order)
    .first<{ version: string }>();
  return row?.version;
};

const RATE_SQL = `(SELECT COUNT(*) FROM versions WHERE publisher_id = ? AND published_at > ?) < ?
  AND (SELECT COUNT(*) FROM versions WHERE publisher_id = ? AND published_at > ?) < ?`;
const rateBinds = (userId: string, now: number) => [userId, now - HOUR, PUBLISHES_PER_HOUR, userId, now - DAY, PUBLISHES_PER_DAY];

const overRate = async (db: D1Database, userId: string, now: number): Promise<boolean> =>
  (await db.prepare(`SELECT ${RATE_SQL} AS ok`).bind(...rateBinds(userId, now)).first<{ ok: number }>())?.ok !== 1;

interface NewVersion {
  readonly version: string;
  readonly precedence: string;
  readonly order: Order;
  readonly sha256: string;
  readonly bytes: number;
  readonly key: string | null;
  readonly hash: string;
  readonly filesCount: number;
  readonly filesBytes: number;
  readonly manifest: string;
  readonly listing: Listing;
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
    ["status", "listed"],
    ["publisher_id", publisher.userId],
    ["via", publisher.via === "session" ? "device" : "deploy_token"],
    ["deploy_token_name", publisher.token?.name ?? null],
    ["deploy_token_short", publisher.token?.short ?? null],
    ["deploy_token_id", publisher.token?.id ?? null],
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
    ["published_at", now]
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
    .bind(...columns.map(([, value]) => value), id, publisher.userId, id, ...v.order, ...rateBinds(publisher.userId, now));
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
