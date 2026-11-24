/**
 * Review and moderation (amendments 12 and 45; contract
 * 2026-11-19-store-shapes-44-45). Every route needs a staff web session
 * (principal.ts `requireAdmin`).
 *
 *   GET  /store/v1/admin/queue?state=undecided|verified|declined|all&kind=plugin|template&cursor&limit
 *   GET  /store/v1/admin/versions/:id/:version                (with its review base; either kind)
 *   POST /store/v1/admin/versions/:id/:version/verify
 *   POST /store/v1/admin/versions/:id/:version/decline { reason, message }
 *   GET  /store/v1/admin/plugins/:id
 *   POST /store/v1/admin/plugins/:id/suspend { reason } | restore | feature { featured } | transfer { to_username }
 *   POST /store/v1/admin/plugins/:id/versions/:version/take-down { reason }
 *   POST /store/v1/admin/plugins/:id/versions/:version/remove { reason }
 *   POST /store/v1/admin/plugins/:id/versions/:version/restore { reason }
 *   … the `/admin/plugins/:id/…` routes also as `/admin/templates/:id/…`, each
 *   of its own kind only (an id of the other kind is 404, for staff too)
 *   GET  /store/v1/admin/reports?status&cursor   POST /store/v1/admin/reports/:id/resolve { resolution, note? }
 *   GET  /store/v1/admin/audit?cursor
 *   GET  /store/v1/admin/reservations   POST /store/v1/admin/reservations { id, to_username?, note? }
 *   DELETE /store/v1/admin/reservations/:id
 *
 * A version is listed when it is published; review only marks it. The
 * mark: undecided → verified | declined (still unverified, listed);
 * declined → verified. A verified version stays verified until a take-down.
 * Its state: listed ↔ yanked (its author; or yanked for good by a newer
 * version superseding it unverified, which verifying it undoes);
 * listed | yanked → taken_down (blocked) | removed (staff); taken_down →
 * listed | yanked (restored, unverified). A plugin: active ↔ suspended.
 * Anything else is 409 `invalid_state`.
 *
 * Every change is one D1 batch: the UPDATE guarded by the state it leaves,
 * then the audit row (written only if the UPDATE changed a row), then a
 * take-down feed event where there is one, then the listing's statements.
 * Then this data center's cache is purged (purge.ts).
 */
import type { Env } from "../env";
import { userDirectoryStub } from "../user-directory";
import { authorCard, authorsOf, listingState, ownerCard, parseStoredJson, publisherIds, reviewOf, versionInfo } from "./cards";
import { ASK_PATHS, type ConsentItem, askParts, askPartsOf, asksChanged, consentDiff } from "./consent";
import { VERSION_ORDER, type VersionRow, auditIfChanged, listedOf, pluginRow, relist, supersedeOlder, supersededAt, versionRow } from "./db";
import { answer, fail, notFound, objectBody, refusedBody } from "./http";
import { KINDS, type Kind, kindOfSegment } from "./kind";
import { dropFromDeployTokens, readBody, requireAdmin } from "./principal";
import { purgeCatalog, purgeTakedowns } from "./purge";
import { isUnused, releaseId } from "./release";
import { SITE_IDS } from "./listing";
import { decodeSegment } from "./serve";
import { foldUsername } from "../username";

const MAX_BODY = 8 * 1024;
const PAGE = 50;
const REASON_RE = /^[a-z][a-z0-9_]{0,63}$/;
const MAX_MESSAGE = 2000;
const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;

const b64 = (value: unknown): string => btoa(JSON.stringify(value)).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
const unb64 = (text: string | null): unknown => {
  if (text === null) return undefined;
  try {
    return JSON.parse(atob(text.replaceAll("-", "+").replaceAll("_", "/")));
  } catch {
    return null;
  }
};
/** A keyset cursor: the item's two sort numbers (a time and a rowid, or a
 * seq twice), and whether it pages back from it (amendment 29). */
interface Keyset {
  readonly a: number;
  readonly b: number;
  readonly back: boolean;
}

/** The cursor in `text`: undefined for none, null when it isn't one. */
const keyset = (text: string | null): Keyset | undefined | null => {
  const v = unb64(text);
  if (v === undefined) return undefined;
  if (!Array.isArray(v) || (v.length !== 2 && !(v.length === 3 && v[2] === "prev"))) return null;
  const [a, b] = v as unknown[];
  return Number.isSafeInteger(a) && Number.isSafeInteger(b) ? { a: a as number, b: b as number, back: v.length === 3 } : null;
};

/** The SQL that pages a list ordered by (`colA`, `colB`), ascending or not,
 * from `after`: its condition (with a leading AND), its order and binds. */
const keysetSql = (colA: string, colB: string, asc: boolean, after: Keyset | undefined) => {
  const forward = asc !== (after?.back ?? false);
  const cmp = forward ? ">" : "<";
  const dir = forward ? "ASC" : "DESC";
  return {
    where: after ? `AND (${colA} ${cmp} ? OR (${colA} = ? AND ${colB} ${cmp} ?))` : "",
    order: `${colA} ${dir}, ${colB} ${dir}`,
    binds: after ? [after.a, after.a, after.b] : []
  };
};

/** `?limit`: 1 to PAGE (the default), or null when it's something else. */
const limitOf = (url: URL): number | null => {
  const raw = url.searchParams.get("limit");
  if (raw === null) return PAGE;
  const n = Number(raw);
  return Number.isSafeInteger(n) && n >= 1 && n <= PAGE ? n : null;
};

/** A page of `rows` (read limit + 1, in the order `keysetSql` gave), turned
 * the right way round, with its `next_cursor` and `prev_cursor`. */
const pageOf = <R>(rows: R[], limit: number, after: Keyset | undefined, key: (row: R) => [number, number]) => {
  const back = after?.back ?? false;
  const more = rows.length > limit;
  const page = rows.slice(0, limit);
  if (back) page.reverse();
  const first = page[0];
  const last = page[page.length - 1];
  const hasNext = back || more;
  const hasPrev = back ? more : after !== undefined;
  return {
    page,
    links: {
      ...(hasNext && last ? { next_cursor: b64(key(last)) } : {}),
      ...(hasPrev && first ? { prev_cursor: b64([...key(first), "prev"]) } : {})
    }
  };
};

/** A page's rows and its list's `total` (amendment 38): what the filter
 * matches, whichever page this is, read in one batch so the two agree. */
const withTotal = async <R>(db: D1Database, page: D1PreparedStatement, count: D1PreparedStatement): Promise<{ rows: R[]; total: number }> => {
  const [rows, counted] = await db.batch<unknown>([page, count]);
  return { rows: rows!.results as R[], total: (counted!.results[0] as { n: number }).n };
};

/** Who each of `ids` is, as admin views show a person: publisher id and username. */
const people = async (env: Env, ids: readonly string[]): Promise<Record<string, { id: string; username: string | null }>> => {
  const found = await authorsOf(env, ids);
  return Object.fromEntries(Object.entries(found).map(([userId, a]) => [userId, { id: a.id, username: a.username }]));
};

// ── the queue and a submission ───────────────────────────────────────────────

/** The queue's states (amendment 45): `undecided` (no reviewer decided it
 * yet, nor did a newer version supersede it), `verified`, `declined`; and
 * `taken_down` (taken down and not restored, newest take-down first, for
 * staff to find what to restore). */
const QUEUE_STATES = ["undecided", "verified", "declined", "taken_down", "all"] as const;
type QueueState = (typeof QUEUE_STATES)[number];

const QUEUE_WHERE: Record<QueueState, string> = {
  undecided: `v.verification = 'unverified' AND v.declined_at IS NULL AND v.superseded_at IS NULL AND v.status IN ('listed', 'yanked')`,
  verified: "v.verification = 'verified'",
  declined: "v.verification = 'unverified' AND v.declined_at IS NOT NULL",
  taken_down: "v.status = 'taken_down'",
  all: "1 = 1"
};

/** Open reports on version `alias` (naming it, or no version). */
const reportsSql = (alias: string): string =>
  `(SELECT COUNT(*) FROM reports r WHERE r.plugin_id = ${alias}.plugin_id AND r.status = 'open' AND (r.version IS NULL OR r.version = ${alias}.version))`;

/** Earlier undecided versions of `alias`'s plugin that it, or one before
 * it, superseded: on its line (a release's, any; a pre-release's, older
 * pre-releases). */
const earlierSql = (alias: string): string =>
  `(SELECT COUNT(*) FROM versions e WHERE e.plugin_id = ${alias}.plugin_id AND e.verification = 'unverified' AND e.declined_at IS NULL
      AND e.superseded_at IS NOT NULL AND e.published_at < ${alias}.published_at AND (${alias}.is_release = 1 OR e.is_release = 0))`;

const queue = async (env: Env, db: D1Database, url: URL): Promise<Response> => {
  const state = (url.searchParams.get("state") ?? "undecided") as QueueState;
  if (!QUEUE_STATES.includes(state)) return fail(400, "bad_request", "state is undecided, verified, declined, taken_down or all.");
  const after = keyset(url.searchParams.get("cursor"));
  if (after === null) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  const limit = limitOf(url);
  if (limit === null) return fail(400, "bad_request", `limit is 1 to ${PAGE}.`);
  // Both kinds, unless it names one.
  const kindRaw = url.searchParams.get("kind");
  const kind = kindRaw === null ? undefined : KINDS.find((k) => k === kindRaw);
  if (kindRaw !== null && kind === undefined) return fail(400, "bad_request", "kind is plugin or template.");
  const kindWhere = kind === undefined ? "" : "AND (SELECT k.kind FROM plugins k WHERE k.id = v.plugin_id) = ?";
  const kindBinds = kind === undefined ? [] : [kind];
  // The undecided read oldest first, versions with open reports before the
  // rest (their key is the publish time, or that plus 10^14 when nothing is
  // reported); every other state, newest first. The undecided hold at most
  // two rows a plugin: its newest undecided release, and its newest
  // undecided pre-release when that is newer (the superseding rule yanked
  // the rest, which a reviewer can still open from the plugin's page).
  const undecided = state === "undecided";
  const sortKey = undecided
    ? `(CASE WHEN ${reportsSql("v")} > 0 THEN 0 ELSE 100000000000000 END + v.published_at)`
    : state === "taken_down"
      ? "COALESCE(v.removed_at, v.published_at)"
      : "v.published_at";
  const ks = keysetSql("sort_key", "rid", undecided, after);
  const { rows, total } = await withTotal<{
    rid: number; sort_key: number; plugin_id: string; kind: Kind; version: string; status: string; published_at: number; publisher_id: string; checks: string;
    permissions: string; name: string; icon: string | null; is_release: number; parts: string | null; reports: number; earlier: number; installs: number;
    verification: "unverified" | "verified"; verified_at: number | null; declined_at: number | null; decline_reason: string | null; decline_message: string | null;
    base_version: string | null; base_permissions: string | null; base_parts: string | null;
  }>(
    db,
    db
      .prepare(
        `SELECT * FROM (
           SELECT v.rowid AS rid, ${sortKey} AS sort_key, v.plugin_id, v.version, v.status, v.published_at, v.publisher_id, v.checks, v.permissions,
                  v.name, v.icon, v.is_release, ${partsSql("v")} AS parts, ${reportsSql("v")} AS reports, ${earlierSql("v")} AS earlier,
                  (SELECT p.installs FROM plugins p WHERE p.id = v.plugin_id) AS installs,
                  (SELECT p.kind FROM plugins p WHERE p.id = v.plugin_id) AS kind,
                  v.verification, v.verified_at, v.declined_at, v.decline_reason, v.decline_message,
                  b.version AS base_version, b.permissions AS base_permissions, ${partsSql("b")} AS base_parts
           FROM versions v LEFT JOIN versions b ON b.rowid = (${baseSql("v")})
           WHERE ${QUEUE_WHERE[state]} ${kindWhere}
         ) WHERE 1 = 1 ${ks.where}
         ORDER BY ${ks.order} LIMIT ?`
      )
      .bind(...kindBinds, ...ks.binds, limit + 1),
    db.prepare(`SELECT COUNT(*) AS n FROM versions v WHERE ${QUEUE_WHERE[state]} ${kindWhere}`).bind(...kindBinds)
  );
  const { page, links } = pageOf(rows, limit, after, (r) => [r.sort_key, r.rid]);
  const names = await people(env, page.map((r) => r.publisher_id));
  return answer({
    items: page.map((r) => {
      const checks = parseStoredJson<{ level: string }[]>(r.checks, []);
      const kind = r.kind;
      // (A template asks for nothing: nothing to diff.)
      const asks = kind === "template" ? null : parseStoredJson<ConsentItem | null>(r.permissions, null);
      const baseAsks = parseStoredJson<ConsentItem | null>(r.base_permissions, null);
      const diff = asks ? consentDiff(baseAsks, asks, { before: baseAsks ? askParts(r.base_parts) : null, after: askParts(r.parts) }) : null;
      const count = (level: string) => checks.filter((c) => c.level === level).length;
      return {
        kind,
        plugin: { id: r.plugin_id, name: r.name },
        version: r.version,
        status: r.status,
        prerelease: r.is_release === 0,
        submitted_at: r.published_at,
        published_at: r.published_at,
        publisher: names[r.publisher_id] ?? null,
        // The version's icon, a path in its package (as a card's), fetched
        // through the files route with this version; none, a template's.
        icon: kind === "template" ? null : r.icon,
        // Against the newest version a reviewer verified (none: a first look).
        ...(r.base_version !== null ? { base: r.base_version } : {}),
        new_asks: diff?.added ?? [],
        // Anything else that moved in what it asks, all of it named in the
        // version's permissions_diff (amendments 40, 41); false for a first look.
        asks_changed: diff !== null && baseAsks !== null && asksChanged(diff),
        checks: { ok: count("ok"), warn: count("warn"), fail: count("fail") },
        review: reviewOf({ verification: r.verification, verified_at: r.verified_at, declined_at: r.declined_at, decline_reason: r.decline_reason, decline_message: r.decline_message } as VersionRow),
        reports: r.reports,
        earlier_undecided: r.earlier,
        // The plugin's installs: of equally reported rows, the most used first.
        installs: r.installs ?? 0
      };
    }),
    total,
    ...links
  });
};

/** What `askParts` reads of the manifest of `alias` (a `versions` row):
 * a few paths, extracted in D1. */
const partsSql = (alias: string): string => `json_extract(${alias}.manifest, ${ASK_PATHS.map((p) => `'${p}'`).join(", ")})`;

/** The rowid of the version `alias` (a `versions` row in the outer query) is
 * reviewed against: its plugin's newest verified version other than itself. */
const baseSql = (alias: string): string =>
  `SELECT rowid FROM versions WHERE plugin_id = ${alias}.plugin_id AND version != ${alias}.version AND verification = 'verified' ORDER BY ${VERSION_ORDER} LIMIT 1`;

const versionView = async (env: Env, db: D1Database, id: string, versionText: string): Promise<Response> => {
  const [plugin, v] = await Promise.all([pluginRow(db, id), versionRow(db, id, versionText)]);
  if (!plugin || !v) return notFound();
  const base = await db
    .prepare(`SELECT b.* FROM versions v JOIN versions b ON b.rowid = (${baseSql("v")}) WHERE v.plugin_id = ? AND v.version = ?`)
    .bind(id, v.version)
    .first<VersionRow>();
  const files = (q: VersionRow) =>
    db
      .prepare("SELECT path, bytes, sha256, kind FROM version_files WHERE plugin_id = ? AND version = ? ORDER BY path")
      .bind(id, q.version)
      .all<{ path: string; bytes: number; sha256: string; kind: string }>()
      .then((r) => r.results.map((f) => ({ path: f.path, bytes: f.bytes, sha256: f.sha256, kind: f.kind, binary: f.kind === "binary" })));
  const [ownFiles, baseFiles, decisions, recent, authors, standing] = await Promise.all([
    files(v),
    base ? files(base) : Promise.resolve([]),
    db
      .prepare(
        `SELECT SUM(verification = 'verified') AS verified, SUM(verification = 'unverified' AND declined_at IS NOT NULL) AS declined
         FROM versions WHERE publisher_id = ?`
      )
      .bind(v.publisher_id)
      .first<{ verified: number | null; declined: number | null }>(),
    db
      .prepare("SELECT plugin_id, version, status, verification, published_at FROM versions WHERE publisher_id = ? ORDER BY published_at DESC LIMIT 10")
      .bind(v.publisher_id)
      .all<{ plugin_id: string; version: string; status: string; verification: string; published_at: number }>(),
    authorsOf(env, [v.publisher_id, plugin.owner_id]),
    // When the account first listed a version (publishing lists it), and
    // its listed plugins, how many with a verified version.
    db
      .prepare(
        `SELECT (SELECT MIN(published_at) FROM versions WHERE publisher_id = ?1) AS first_published_at,
                (SELECT COUNT(*) FROM plugins p WHERE p.owner_id = ?1 AND (${KINDS.map((k) => `(${listedOf(k)})`).join(" OR ")})) AS total,
                (SELECT COUNT(*) FROM plugins p WHERE p.owner_id = ?1 AND (${KINDS.map((k) => `(${listedOf(k)})`).join(" OR ")}) AND p.verified_version IS NOT NULL) AS verified`
      )
      .bind(v.publisher_id)
      .first<{ first_published_at: number | null; total: number; verified: number }>()
  ]);
  const publisher = authors[v.publisher_id]!;
  const owner = authors[plugin.owner_id]!;
  const permissions = parseStoredJson<ConsentItem | null>(v.permissions, null);
  const basePermissions = base ? parseStoredJson<ConsentItem | null>(base.permissions, null) : null;
  const template = plugin.kind === "template";
  return answer({
    kind: plugin.kind,
    // (A PluginCard, amendment 35; nothing listed: this version's.)
    plugin: ownerCard(plugin, v, authorCard(owner)),
    version: {
      ...versionInfo(v, plugin.kind, true),
      superseded: v.superseded_at !== null ? { at: v.superseded_at, by: v.superseded_by } : null,
      name: v.name, summary: v.summary, icon: template ? null : v.icon, readme: v.readme, homepage: v.homepage, repository: v.repository, license: v.license,
      categories: parseStoredJson(v.categories, []), keywords: parseStoredJson(v.keywords, []), screenshots: parseStoredJson(v.screenshots, [])
    },
    superseded: v.superseded_at !== null ? { at: v.superseded_at, by: v.superseded_by } : null,
    publisher: {
      id: publisher.id,
      username: publisher.username,
      created_at: publisher.createdAt,
      decisions: { verified: decisions?.verified ?? 0, declined: decisions?.declined ?? 0 },
      recent: recent.results.map((r) => ({ ...r, verified: r.verification === "verified" })),
      first_published_at: standing?.first_published_at ?? null,
      plugins: { total: standing?.total ?? 0, verified: standing?.verified ?? 0 }
    },
    manifest: parseStoredJson(v.manifest, null),
    ...(base ? { base: { version: base.version, manifest: parseStoredJson(base.manifest, null) } } : {}),
    files: ownFiles,
    base_files: baseFiles,
    checks: parseStoredJson(v.checks, []),
    // (A template asks for nothing.)
    permissions: template ? null : permissions,
    permissions_diff: permissions && !template
      ? consentDiff(basePermissions, permissions, { before: base ? askPartsOf(base.manifest) : null, after: askPartsOf(v.manifest) })
      : null
  });
};

// ── changes ─────────────────────────────────────────────────────────────────

/** Run a guarded change: its UPDATE, the audit row, anything after it, the
 * listing; 409 if the UPDATE found nothing in the state it leaves. */
const change = async (
  db: D1Database,
  update: D1PreparedStatement,
  audit: D1PreparedStatement,
  id: string,
  author: string | null,
  conflict: string,
  then: readonly D1PreparedStatement[] = []
): Promise<Response | undefined> => {
  const results = await db.batch([update, audit, ...then, ...relist(db, id, author)]);
  return (results[0]?.meta.changes ?? 0) === 0 ? fail(409, "invalid_state", conflict) : undefined;
};

const ownerUsername = async (env: Env, ownerId: string): Promise<string | null> => (await authorsOf(env, [ownerId]))[ownerId]!.username;

const changed = async (db: D1Database, id: string, version: string): Promise<Response> => {
  await purgeCatalog(db, id, [version]);
  const [plugin, v] = await Promise.all([pluginRow(db, id), versionRow(db, id, version)]);
  return answer({ version: versionInfo(v!, plugin!.kind, true) });
};

const reasonOf = (body: Record<string, unknown> | undefined, what: string): string | Response =>
  typeof body?.reason !== "string" || body.reason.trim() === "" || body.reason.length > MAX_MESSAGE
    ? fail(400, "bad_request", `reason is ${what}, at most ${MAX_MESSAGE} characters.`)
    : body.reason.trim();

/** Verify a version (from undecided or declined): the mark is one-way, and
 * only a take-down ends it. A version the superseding rule yanked is
 * listed again: every verified version stays installable. */
const verify = async (db: D1Database, adminId: string, id: string, v: VersionRow): Promise<Response> => {
  if (v.verification === "verified") return fail(409, "invalid_state", `Version ${v.version} is verified already.`);
  if (v.status !== "listed" && v.status !== "yanked") return fail(409, "invalid_state", `Version ${v.version} is ${v.status}: only a listed or yanked version can be verified.`);
  const now = Date.now();
  const conflict = await change(
    db,
    db
      .prepare(
        `UPDATE versions SET verification = 'verified', verified_at = ?, verified_by = ?,
           status = CASE WHEN yanked_by = 'superseded' THEN 'listed' ELSE status END,
           yanked_by = CASE WHEN yanked_by = 'superseded' THEN NULL ELSE yanked_by END
         WHERE plugin_id = ? AND version = ? AND verification = 'unverified' AND status IN ('listed', 'yanked')`
      )
      .bind(now, adminId, id, v.version),
    auditIfChanged(db, now, adminId, "version.verify", id, v.version, undefined),
    id,
    null,
    "The version changed meanwhile: look again."
  );
  return conflict ?? changed(db, id, v.version);
};

/** Decline to verify an undecided version: it stays listed, unverified; its
 * author reads why (the public sees only that it isn't verified). */
const decline = async (db: D1Database, adminId: string, id: string, v: VersionRow, body: Record<string, unknown> | undefined): Promise<Response> => {
  if (typeof body?.reason !== "string" || !REASON_RE.test(body.reason)) return fail(400, "bad_request", "reason is a short word (malware, broken, misleading, …).");
  if (typeof body.message !== "string" || body.message.trim() === "" || body.message.length > MAX_MESSAGE) {
    return fail(400, "bad_request", `message is what the author reads, at most ${MAX_MESSAGE} characters.`);
  }
  if (v.verification === "verified" || v.declined_at !== null) return fail(409, "invalid_state", `Version ${v.version} is decided already.`);
  if (v.status !== "listed" && v.status !== "yanked") return fail(409, "invalid_state", `Version ${v.version} is ${v.status}.`);
  const now = Date.now();
  const reason = body.reason;
  const message = body.message.trim();
  const conflict = await change(
    db,
    db
      .prepare(
        `UPDATE versions SET declined_at = ?, declined_by = ?, decline_reason = ?, decline_message = ?
         WHERE plugin_id = ? AND version = ? AND verification = 'unverified' AND declined_at IS NULL AND status IN ('listed', 'yanked')`
      )
      .bind(now, adminId, reason, message, id, v.version),
    auditIfChanged(db, now, adminId, "version.decline", id, v.version, { reason, message }),
    id,
    null,
    "The version changed meanwhile: look again."
  );
  return conflict ?? changed(db, id, v.version);
};

/** A take-down's or a restore's event in the feed, numbered by this same
 * write (after its audit row, so only when the change landed). */
const takedownEvent = (db: D1Database, kind: "take_down" | "restore", id: string, version: string, reason: string, at: number): D1PreparedStatement =>
  db
    // (The item's kind and the version's content hash with it: a released
    // id's row is gone by the time an old event is read, and a device refuses
    // a file or link import of the same package.)
    .prepare(
      `INSERT INTO takedown_events (kind, item_kind, plugin_id, version, hash, reason, at)
       SELECT ?1, (SELECT kind FROM plugins WHERE id = ?2), ?2, ?3, (SELECT content_hash FROM versions WHERE plugin_id = ?2 AND version = ?3), ?4, ?5 WHERE changes() > 0`
    )
    .bind(kind, id, version, reason, at);

const afterEvent = async (db: D1Database, id: string, version: string): Promise<Response> => {
  const seq = await db.prepare("SELECT MAX(seq) AS n FROM takedown_events").first<number>("n");
  if (seq) await purgeTakedowns(seq);
  return changed(db, id, version);
};

/** Take a listed or yanked version down (harmful): blocked on devices, its
 * mark ended, an event in the take-down feed. Nothing it superseded comes
 * back; its author publishes again. */
const takeDown = async (db: D1Database, adminId: string, id: string, v: VersionRow, body: Record<string, unknown> | undefined): Promise<Response> => {
  const reason = reasonOf(body, "what people read");
  if (reason instanceof Response) return reason;
  if (v.status !== "listed" && v.status !== "yanked") return fail(409, "invalid_state", `Version ${v.version} is ${v.status}: only a listed or yanked version can be taken down.`);
  const now = Date.now();
  const conflict = await change(
    db,
    db
      .prepare(
        `UPDATE versions SET status = 'taken_down', removed_at = ?, removed_by = ?, removal_reason = ?, yanked_by = NULL,
           verification = 'unverified', verified_at = NULL, verified_by = NULL
         WHERE plugin_id = ? AND version = ? AND status IN ('listed', 'yanked')`
      )
      .bind(now, adminId, reason, id, v.version),
    auditIfChanged(db, now, adminId, "version.take_down", id, v.version, { reason }),
    id,
    null,
    "The version changed meanwhile: look again.",
    [takedownEvent(db, "take_down", id, v.version, reason, now)]
  );
  return conflict ?? afterEvent(db, id, v.version);
};

/** Remove a listed or yanked version (not harmful: copyright, broken): no
 * longer served or offered, not blocked. */
const remove = async (db: D1Database, adminId: string, id: string, v: VersionRow, body: Record<string, unknown> | undefined): Promise<Response> => {
  const reason = reasonOf(body, "what people read");
  if (reason instanceof Response) return reason;
  if (v.status !== "listed" && v.status !== "yanked") return fail(409, "invalid_state", `Version ${v.version} is ${v.status}: only a listed or yanked version can be removed.`);
  const now = Date.now();
  const conflict = await change(
    db,
    db
      .prepare(
        `UPDATE versions SET status = 'removed', removed_at = ?, removed_by = ?, removal_reason = ?, yanked_by = NULL
         WHERE plugin_id = ? AND version = ? AND status IN ('listed', 'yanked')`
      )
      .bind(now, adminId, reason, id, v.version),
    auditIfChanged(db, now, adminId, "version.remove", id, v.version, { reason }),
    id,
    null,
    "The version changed meanwhile: look again."
  );
  return conflict ?? changed(db, id, v.version);
};

/** Restore a taken-down version: unverified and undecided again, and
 * straight to yanked by the superseding rule when a newer version on its
 * line is listed or yanked (a release's line: newer releases; a
 * pre-release's: any newer version), decided in the UPDATE itself so a
 * publish racing it can't slip between; listed, it supersedes what is
 * older, as any version listed does. The block lifts (an event in the feed). */
const restore = async (db: D1Database, adminId: string, id: string, v: VersionRow, body: Record<string, unknown> | undefined): Promise<Response> => {
  const reason = reasonOf(body, "why it comes back");
  if (reason instanceof Response) return reason;
  if (v.status !== "taken_down") return fail(409, "invalid_state", `Version ${v.version} is ${v.status}: only a taken-down version can be restored.`);
  const now = Date.now();
  // (`versions` in the subquery is the row being restored: its line and
  // place never change.)
  const newer = `(SELECT n.version FROM versions n WHERE n.plugin_id = versions.plugin_id AND n.status IN ('listed', 'yanked')
      AND (versions.is_release = 0 OR n.is_release = 1)
      AND (n.major, n.minor, n.patch, n.is_release, n.pre_key) > (versions.major, versions.minor, versions.patch, versions.is_release, versions.pre_key)
      ORDER BY n.major DESC, n.minor DESC, n.patch DESC, n.is_release DESC, n.pre_key DESC LIMIT 1)`;
  const conflict = await change(
    db,
    db
      .prepare(
        `UPDATE versions SET status = CASE WHEN ${newer} IS NULL THEN 'listed' ELSE 'yanked' END,
           yanked_by = CASE WHEN ${newer} IS NULL THEN NULL ELSE 'superseded' END,
           superseded_at = CASE WHEN ${newer} IS NULL THEN NULL ELSE ?1 END,
           superseded_by = ${newer},
           removed_at = NULL, removed_by = NULL, removal_reason = NULL,
           verification = 'unverified', verified_at = NULL, verified_by = NULL,
           declined_at = NULL, declined_by = NULL, decline_reason = NULL, decline_message = NULL
         WHERE plugin_id = ?2 AND version = ?3 AND status = 'taken_down'`
      )
      .bind(now, id, v.version),
    // The audit row names what superseded it, as the UPDATE found it.
    db
      .prepare(
        `INSERT INTO audit (at, actor_id, action, plugin_id, version, detail)
         SELECT ?1, ?2, 'version.restore', ?3, ?4,
           CASE WHEN superseded_by IS NULL THEN json_object('reason', ?5) ELSE json_object('reason', ?5, 'superseded_by', superseded_by) END
         FROM versions WHERE plugin_id = ?3 AND version = ?4 AND changes() > 0`
      )
      .bind(now, adminId, id, v.version, reason),
    id,
    null,
    "The version changed meanwhile: look again.",
    [takedownEvent(db, "restore", id, v.version, reason, now), supersedeOlder(db, id, v.version, now)]
  );
  if (conflict) return conflict;
  await purgeCatalog(db, id, await supersededAt(db, id, v.version, now));
  return afterEvent(db, id, v.version);
};

/** (`kind`: the prefix it came by, or undefined for the routes that take
 * only an id, which take either.) */
const versionAction = async (db: D1Database, adminId: string, kind: Kind | undefined, id: string, versionText: string, action: string, body: Record<string, unknown> | undefined): Promise<Response> => {
  const [plugin, v] = await Promise.all([pluginRow(db, id), versionRow(db, id, versionText)]);
  if (!plugin || (kind !== undefined && plugin.kind !== kind) || !v) return notFound();
  switch (action) {
    case "verify":
      return verify(db, adminId, id, v);
    case "decline":
      return decline(db, adminId, id, v, body);
    case "take-down":
      return takeDown(db, adminId, id, v, body);
    case "remove":
      return remove(db, adminId, id, v, body);
    case "restore":
      return restore(db, adminId, id, v, body);
    default:
      return notFound();
  }
};

const pluginAction = async (env: Env, db: D1Database, adminId: string, kind: Kind, id: string, action: string, body: Record<string, unknown> | undefined): Promise<Response> => {
  const plugin = await pluginRow(db, id);
  if (!plugin || plugin.kind !== kind) return notFound();
  const now = Date.now();
  let conflict: Response | undefined;
  switch (action) {
    case "suspend": {
      if (typeof body?.reason !== "string" || body.reason.trim() === "" || body.reason.length > MAX_MESSAGE) return fail(400, "bad_request", "reason says why.");
      if (plugin.status !== "active") return fail(409, "invalid_state", "It's already suspended.");
      conflict = await change(
        db,
        db.prepare("UPDATE plugins SET status = 'suspended', suspend_reason = ? WHERE id = ? AND status = 'active'").bind(body.reason.trim(), id),
        auditIfChanged(db, now, adminId, "plugin.suspend", id, null, { reason: body.reason.trim() }),
        id,
        null,
        "It changed meanwhile: look again."
      );
      break;
    }
    case "restore":
      if (plugin.status !== "suspended") return fail(409, "invalid_state", "It isn't suspended.");
      conflict = await change(
        db,
        db.prepare("UPDATE plugins SET status = 'active', suspend_reason = NULL WHERE id = ? AND status = 'suspended'").bind(id),
        auditIfChanged(db, now, adminId, "plugin.restore", id, null, undefined),
        id,
        await ownerUsername(env, plugin.owner_id),
        "It changed meanwhile: look again."
      );
      break;
    case "feature": {
      if (typeof body?.featured !== "boolean") return fail(400, "bad_request", "featured is true or false.");
      // Featured is the vetted shelf: only an item whose newest version is verified.
      if (body.featured && !(plugin.listed_version && plugin.verified === 1)) {
        return fail(409, "not_verified", `Only a ${kind} whose newest version is verified can be featured.`);
      }
      conflict = await change(
        db,
        body.featured
          ? db.prepare("UPDATE plugins SET featured_at = ? WHERE id = ? AND featured_at IS NULL AND verified = 1").bind(now, id)
          : db.prepare("UPDATE plugins SET featured_at = NULL WHERE id = ? AND featured_at IS NOT NULL").bind(id),
        auditIfChanged(db, now, adminId, body.featured ? "plugin.feature" : "plugin.unfeature", id, null, undefined),
        id,
        null,
        body.featured ? "It's already featured." : "It isn't featured."
      );
      break;
    }
    case "transfer": {
      const username = foldUsername(body?.to_username);
      const directory = userDirectoryStub(env);
      const to = username && directory ? await directory.userIdForUsername(username) : undefined;
      if (!to) return fail(400, "bad_request", "to_username is a member's username.");
      if (to === plugin.owner_id) return fail(409, "invalid_state", "They already own it.");
      conflict = await change(
        db,
        // Its new owner holds it from now (a deploy token's `all` never takes it
        // in), and always later than it was claimed or last moved, even in the
        // same millisecond: a moved plugin never reads as never moved.
        db
          .prepare("UPDATE plugins SET owner_id = ?, owner_since = MAX(?, created_at + 1, owner_since + 1) WHERE id = ? AND owner_id = ?")
          .bind(to, now, id, plugin.owner_id),
        auditIfChanged(db, now, adminId, "plugin.transfer", id, null, { from: plugin.owner_id, to, to_username: username }),
        id,
        username!,
        "It changed hands meanwhile: look again."
      );
      // It leaves the old owner's deploy tokens' lists (their scopes hold it no
      // more in any case: its owner and `owner_since` changed).
      if (!conflict) await dropFromDeployTokens(env, plugin.owner_id, id).catch((e) => console.error("store: couldn't drop a transferred plugin from deploy tokens", id, String(e)));
      break;
    }
    default:
      return notFound();
  }
  if (conflict) return conflict;
  await purgeCatalog(db, id);
  return adminPlugin(env, db, kind, id);
};

// ── a plugin, reports, audit, reservations ──────────────────────────────────

const adminPlugin = async (env: Env, db: D1Database, kind: Kind, id: string): Promise<Response> => {
  const plugin = await pluginRow(db, id);
  if (!plugin || plugin.kind !== kind) return notFound();
  const [versions, reportRows, authors] = await Promise.all([
    db.prepare(`SELECT * FROM versions WHERE plugin_id = ? ORDER BY ${VERSION_ORDER}`).bind(id).all<VersionRow>(),
    db.prepare(`SELECT ${REPORT_COLUMNS} FROM reports r LEFT JOIN plugins p ON p.id = r.plugin_id WHERE r.plugin_id = ? ORDER BY r.id DESC LIMIT 100`).bind(id).all<ReportRow>(),
    authorsOf(env, [plugin.owner_id])
  ]);
  const owner = authors[plugin.owner_id]!;
  const publishers = await publisherIds(versions.results.map((v) => v.publisher_id));
  return answer({
    plugin: {
      ...ownerCard(plugin, versions.results[0], authorCard(owner)),
      listed_version: plugin.listed_version,
      created_at: plugin.created_at,
      suspend_reason: plugin.suspend_reason
    },
    owner: { id: owner.id, username: owner.username, user_id: plugin.owner_id, created_at: owner.createdAt },
    versions: versions.results.map((v) => ({
      ...versionInfo(v, plugin.kind, true),
      publisher_id: publishers.get(v.publisher_id)!,
      superseded: v.superseded_at !== null ? { at: v.superseded_at, by: v.superseded_by } : null
    })),
    // As /admin/reports gives them.
    reports: await reportItems(env, reportRows.results),
    featured: plugin.featured_at !== null,
    // The /me/plugins words (amendment 35).
    status: listingState(plugin)
  });
};

const REPORT_COLUMNS = "r.id, r.plugin_id, p.name AS plugin_name, p.kind AS plugin_kind, r.version, r.reporter_id, r.reason, r.text, r.created_at, r.status, r.resolved_by, r.resolved_at, r.note";
interface ReportRow {
  readonly id: number;
  readonly plugin_id: string;
  readonly plugin_name: string | null;
  readonly plugin_kind: Kind | null;
  readonly version: string | null;
  readonly reporter_id: string;
  readonly reason: string;
  readonly text: string;
  readonly created_at: number;
  readonly status: string;
  readonly resolved_by: string | null;
  readonly resolved_at: number | null;
  readonly note: string | null;
}

/** Reports as admin views give them: with the plugin and the people. */
const reportItems = async (env: Env, rows: readonly ReportRow[]) => {
  const names = await people(env, [...rows.map((r) => r.reporter_id), ...rows.flatMap((r) => (r.resolved_by ? [r.resolved_by] : []))]);
  return rows.map((r) => ({
    id: r.id,
    // (Its kind says which prefix its staff page is under.)
    plugin: { id: r.plugin_id, name: r.plugin_name, kind: r.plugin_kind },
    version: r.version,
    reporter: names[r.reporter_id] ?? null,
    reason: r.reason,
    text: r.text,
    created_at: r.created_at,
    status: r.status,
    resolved_by: r.resolved_by ? (names[r.resolved_by] ?? null) : null,
    resolved_at: r.resolved_at,
    note: r.note
  }));
};

const reports = async (env: Env, db: D1Database, url: URL): Promise<Response> => {
  const status = url.searchParams.get("status") ?? "open";
  if (!["open", "dismissed", "actioned", "all"].includes(status)) return fail(400, "bad_request", "status is open, dismissed, actioned or all.");
  const after = keyset(url.searchParams.get("cursor"));
  if (after === null) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  const limit = limitOf(url);
  if (limit === null) return fail(400, "bad_request", `limit is 1 to ${PAGE}.`);
  const ks = keysetSql("r.id", "r.id", false, after);
  const { rows, total } = await withTotal<ReportRow>(
    db,
    db
      .prepare(
        `SELECT ${REPORT_COLUMNS} FROM reports r LEFT JOIN plugins p ON p.id = r.plugin_id
         WHERE (? = 'all' OR r.status = ?) ${ks.where} ORDER BY ${ks.order} LIMIT ?`
      )
      .bind(status, status, ...ks.binds, limit + 1),
    // (reports_open serves a status's count.)
    status === "all" ? db.prepare("SELECT COUNT(*) AS n FROM reports") : db.prepare("SELECT COUNT(*) AS n FROM reports WHERE status = ?").bind(status)
  );
  const { page, links } = pageOf(rows, limit, after, (r) => [r.id, r.id]);
  return answer({ reports: await reportItems(env, page), total, ...links });
};

const resolveReport = async (db: D1Database, adminId: string, reportId: number, body: Record<string, unknown> | undefined): Promise<Response> => {
  const resolution = body?.resolution;
  if (resolution !== "dismissed" && resolution !== "actioned") return fail(400, "bad_request", "resolution is dismissed or actioned.");
  const note = body?.note ?? null;
  if (note !== null && (typeof note !== "string" || note.length > MAX_MESSAGE)) return fail(400, "bad_request", `note is at most ${MAX_MESSAGE} characters.`);
  const report = await db.prepare("SELECT plugin_id, status FROM reports WHERE id = ?").bind(reportId).first<{ plugin_id: string; status: string }>();
  if (!report) return notFound();
  if (report.status !== "open") return fail(409, "invalid_state", "It's already resolved.");
  const now = Date.now();
  const results = await db.batch([
    db.prepare("UPDATE reports SET status = ?, resolved_by = ?, resolved_at = ?, note = ? WHERE id = ? AND status = 'open'").bind(resolution, adminId, now, note, reportId),
    auditIfChanged(db, now, adminId, `report.${resolution}`, report.plugin_id, null, { report: reportId, note })
  ]);
  if ((results[0]?.meta.changes ?? 0) === 0) return fail(409, "invalid_state", "It's already resolved.");
  return answer({ id: reportId, status: resolution });
};

const audit = async (env: Env, db: D1Database, url: URL): Promise<Response> => {
  const after = keyset(url.searchParams.get("cursor"));
  if (after === null) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  const limit = limitOf(url);
  if (limit === null) return fail(400, "bad_request", `limit is 1 to ${PAGE}.`);
  const ks = keysetSql("seq", "seq", false, after);
  const pluginFilter = url.searchParams.get("plugin");
  if (pluginFilter !== null && !ID_RE.test(pluginFilter)) return fail(400, "bad_request", "plugin is a plugin's or a template's id.");
  // One plugin's log by its index (audit_plugin), or the whole log.
  const byPlugin = pluginFilter === null ? [] : [pluginFilter];
  const conditions = [...(pluginFilter === null ? [] : ["plugin_id = ?"]), ...(ks.where ? [ks.where.replace(/^AND /, "")] : [])];
  const { rows, total } = await withTotal<{ seq: number; at: number; actor_id: string; action: string; plugin_id: string | null; version: string | null; detail: string | null; item_kind: Kind | null }>(
    db,
    db
      .prepare(`SELECT audit.*, (SELECT p.kind FROM plugins p WHERE p.id = audit.plugin_id) AS item_kind FROM audit ${conditions.length ? `WHERE ${conditions.join(" AND ")}` : ""} ORDER BY ${ks.order} LIMIT ?`)
      .bind(...byPlugin, ...ks.binds, limit + 1),
    db.prepare(`SELECT COUNT(*) AS n FROM audit ${pluginFilter === null ? "" : "WHERE plugin_id = ?"}`).bind(...byPlugin)
  );
  const { page, links } = pageOf(rows, limit, after, (r) => [r.seq, r.seq]);
  const names = await people(env, page.map((r) => r.actor_id));
  return answer({
    entries: page.map((r) => ({
      seq: r.seq,
      at: r.at,
      actor: { id: names[r.actor_id]?.id ?? null, username: names[r.actor_id]?.username ?? null, user_id: r.actor_id },
      action: r.action,
      plugin_id: r.plugin_id,
      // The item's kind, for the prefix of its staff page (null: released).
      item_kind: r.item_kind,
      version: r.version,
      detail: parseStoredJson(r.detail, null)
    })),
    total,
    ...links
  });
};

const reservations = async (request: Request, env: Env, db: D1Database, bucket: R2Bucket, adminId: string, rest: readonly string[], body: Record<string, unknown> | undefined): Promise<Response> => {
  if (rest.length === 0 && request.method === "GET") {
    const rows = (await db.prepare("SELECT * FROM reservations ORDER BY id").all<{ id: string; user_id: string | null; note: string | null; created_at: number }>()).results;
    const names = await people(env, rows.flatMap((r) => (r.user_id ? [r.user_id] : [])));
    return answer({ reservations: rows.map((r) => ({ id: r.id, to: r.user_id ? (names[r.user_id] ?? null) : null, note: r.note, created_at: r.created_at })) });
  }
  const now = Date.now();
  if (rest.length === 0 && request.method === "POST") {
    const id = body?.id;
    if (typeof id !== "string" || !ID_RE.test(id)) return fail(400, "bad_request", "id is a plugin id.");
    // The site's own pages (listing.ts `SITE_IDS`): never anyone's.
    if (SITE_IDS.includes(id)) return fail(400, "site_id", `"${id}" is one of the site's pages: no plugin can have it.`);
    let userId: string | null = null;
    if (body?.to_username !== undefined && body.to_username !== null) {
      const username = foldUsername(body.to_username);
      const directory = userDirectoryStub(env);
      userId = (username && directory ? await directory.userIdForUsername(username) : undefined) ?? null;
      if (!userId) return fail(400, "bad_request", "to_username is a member's username.");
    }
    const note = body?.note ?? null;
    if (note !== null && (typeof note !== "string" || note.length > MAX_MESSAGE)) return fail(400, "bad_request", "note is short text.");
    // An id someone claimed and never used is released for the reservation;
    // one in use changes hands only by a transfer.
    let released = false;
    if (await pluginRow(db, id)) {
      if (!(await isUnused(db, id))) return fail(409, "invalid_state", `"${id}" is in use: transfer it instead.`);
      released = await releaseId(env, db, bucket, id, Number.POSITIVE_INFINITY, adminId);
    }
    await db.batch([
      db
        .prepare(
          `INSERT INTO reservations (id, user_id, note, created_by, created_at) VALUES (?, ?, ?, ?, ?)
           ON CONFLICT (id) DO UPDATE SET user_id = excluded.user_id, note = excluded.note`
        )
        .bind(id, userId, note, adminId, now),
      auditIfChanged(db, now, adminId, "reservation.set", id, null, { user_id: userId, note })
    ]);
    return answer({ id, to: userId ? { username: foldUsername(body?.to_username) } : null, note, released }, 200);
  }
  if (rest.length === 1 && request.method === "DELETE") {
    // Frees the id: its reservation goes, and a claim never used is released.
    const id = rest[0]!;
    const results = await db.batch([
      db.prepare("DELETE FROM reservations WHERE id = ?").bind(id),
      auditIfChanged(db, now, adminId, "reservation.release", id, null, undefined)
    ]);
    const unreserved = (results[0]?.meta.changes ?? 0) > 0;
    const released = ID_RE.test(id) && (await pluginRow(db, id)) !== null && (await isUnused(db, id)) && (await releaseId(env, db, bucket, id, Number.POSITIVE_INFINITY, adminId));
    return unreserved || released ? answer({ id, released: true }) : notFound();
  }
  return fail(405, "method_not_allowed", "That method isn't allowed here.");
};

/** `/store/v1/admin/…`: `parts` from `admin` on. */
export const handleAdmin = async (request: Request, env: Env, db: D1Database, bucket: R2Bucket, url: URL, parts: readonly string[]): Promise<Response> => {
  const admin = await requireAdmin(request, env);
  if (admin instanceof Response) return admin;
  const [, section, a, b, c, d] = parts;
  const get = request.method === "GET";
  const post = request.method === "POST";
  let body: Record<string, unknown> | undefined;
  if (post) {
    const raw = await readBody(request, MAX_BODY);
    if (typeof raw === "string") return refusedBody(raw);
    body = objectBody(raw);
  }

  if (section === "queue" && parts.length === 2 && get) return queue(env, db, url);
  if (section === "versions" && a && b) {
    const versionText = decodeSegment(b);
    if (!versionText || !ID_RE.test(a)) return notFound();
    if (parts.length === 4 && get) return versionView(env, db, a, versionText);
    if (parts.length === 5 && post && (c === "verify" || c === "decline")) return versionAction(db, admin.userId, undefined, a, versionText, c, body);
  }
  const kind = kindOfSegment(section);
  if (kind !== undefined && a && ID_RE.test(a)) {
    if (parts.length === 3 && get) return adminPlugin(env, db, kind, a);
    if (parts.length === 4 && post && b) return pluginAction(env, db, admin.userId, kind, a, b, body);
    if (parts.length === 6 && post && b === "versions" && c && (d === "take-down" || d === "remove" || d === "restore")) {
      const versionText = decodeSegment(c);
      return versionText ? versionAction(db, admin.userId, kind, a, versionText, d, body) : notFound();
    }
  }
  if (section === "reports") {
    if (parts.length === 2 && get) return reports(env, db, url);
    if (parts.length === 4 && post && b === "resolve" && a && /^[0-9]{1,15}$/.test(a)) return resolveReport(db, admin.userId, Number(a), body);
  }
  if (section === "audit" && parts.length === 2 && get) return audit(env, db, url);
  if (section === "reservations") return reservations(request, env, db, bucket, admin.userId, parts.slice(2), body);
  return notFound();
};
