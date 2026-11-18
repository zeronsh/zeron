/**
 * Review and moderation (contract "Admin", amendment 12). Every route needs
 * a signed-in caller holding `admin` (principal.ts `requireAdmin`).
 *
 *   GET  /store/v1/admin/queue?status&cursor
 *   GET  /store/v1/admin/submissions/:id/:version        (with its diff base)
 *   POST /store/v1/admin/submissions/:id/:version/approve
 *   POST /store/v1/admin/submissions/:id/:version/reject { reason, message }
 *   GET  /store/v1/admin/plugins/:id
 *   POST /store/v1/admin/plugins/:id/suspend { reason } | restore | feature { featured } | transfer { to_username }
 *   POST /store/v1/admin/plugins/:id/versions/:version/remove { block, reason }
 *   GET  /store/v1/admin/reports?status&cursor   POST /store/v1/admin/reports/:id/resolve { resolution, note? }
 *   GET  /store/v1/admin/audit?cursor
 *   GET  /store/v1/admin/reservations   POST /store/v1/admin/reservations { id, to_username?, note? }
 *   DELETE /store/v1/admin/reservations/:id
 *
 * The state machine. A version: pending → approved | rejected (admin) |
 * withdrawn (author); approved → yanked (author) | removed (admin);
 * yanked → approved (author) | removed (admin). Nothing leaves rejected,
 * withdrawn or removed. A plugin: active ↔ suspended. Anything else is 409
 * `invalid_state`.
 *
 * Every change is one D1 batch: the UPDATE guarded by the state it leaves,
 * then the audit row (written only if the UPDATE changed a row), then the
 * listing's statements. Then this data center's cache is purged.
 */
import type { Env } from "../env";
import { userDirectoryStub } from "../user-directory";
import { pluginCacheKeys } from "./author";
import { purge } from "./cache";
import { authorCard, authorsOf, pluginCard, versionInfo, parseStoredJson } from "./cards";
import { type ConsentItem, consentDiff } from "./consent";
import { VERSION_ORDER, type VersionRow, auditIfChanged, pluginRow, relist, versionRow } from "./db";
import { answer, fail, notFound, objectBody, refusedBody } from "./http";
import { readBody, requireAdmin, revokeDeployKeys } from "./principal";
import { isUnused, releaseId } from "./release";
import { decodeSegment } from "./serve";
import { foldUsername } from "../username";

const MAX_BODY = 8 * 1024;
const PAGE = 50;
const REASON_RE = /^[a-z][a-z0-9_]{0,63}$/;
const MAX_MESSAGE = 2000;
const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;
const VERSION_STATUSES = ["pending", "approved", "rejected", "withdrawn", "yanked", "removed"] as const;

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

/** Who each of `ids` is, as admin views show a person: publisher id and username. */
const people = async (env: Env, ids: readonly string[]): Promise<Record<string, { id: string; username: string | null }>> => {
  const found = await authorsOf(env, ids);
  return Object.fromEntries(Object.entries(found).map(([userId, a]) => [userId, { id: a.id, username: a.username }]));
};

// ── the queue and a submission ───────────────────────────────────────────────

const queue = async (env: Env, db: D1Database, url: URL): Promise<Response> => {
  const status = url.searchParams.get("status") ?? "pending";
  if (status !== "all" && !(VERSION_STATUSES as readonly string[]).includes(status)) return fail(400, "bad_request", "status is a version state or all.");
  const after = keyset(url.searchParams.get("cursor"));
  if (after === null) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  const limit = limitOf(url);
  if (limit === null) return fail(400, "bad_request", `limit is 1 to ${PAGE}.`);
  // The queue reads oldest first; any other state, newest first.
  const ks = keysetSql("v.submitted_at", "v.rowid", status === "pending", after);
  const rows = (
    await db
      .prepare(
        `SELECT v.rowid AS rid, v.plugin_id, v.version, v.status, v.submitted_at, v.decided_at, v.publisher_id, v.via, v.checks,
                v.name, v.is_release
         FROM versions v
         WHERE (? = 'all' OR v.status = ?) ${ks.where}
         ORDER BY ${ks.order} LIMIT ?`
      )
      .bind(status, status, ...ks.binds, limit + 1)
      .all<{ rid: number; plugin_id: string; version: string; status: string; submitted_at: number; decided_at: number | null; publisher_id: string; via: string; checks: string; name: string; is_release: number }>()
  ).results;
  const { page, links } = pageOf(rows, limit, after, (r) => [r.submitted_at, r.rid]);
  const names = await people(env, page.map((r) => r.publisher_id));
  return answer({
    items: page.map((r) => {
      const checks = parseStoredJson<{ level: string }[]>(r.checks, []);
      return {
        plugin: { id: r.plugin_id, name: r.name },
        version: r.version,
        status: r.status,
        prerelease: r.is_release === 0,
        submitted_at: r.submitted_at,
        decided_at: r.decided_at,
        publisher: names[r.publisher_id] ?? null,
        via: r.via,
        warnings: checks.filter((c) => c.level === "warn").length
      };
    }),
    ...links
  });
};

const submission = async (env: Env, db: D1Database, id: string, versionText: string): Promise<Response> => {
  const [plugin, v] = await Promise.all([pluginRow(db, id), versionRow(db, id, versionText)]);
  if (!plugin || !v) return notFound();
  const base = await db
    .prepare(`SELECT * FROM versions WHERE plugin_id = ? AND version != ? AND status IN ('approved', 'yanked') ORDER BY ${VERSION_ORDER} LIMIT 1`)
    .bind(id, v.version)
    .first<VersionRow>();
  const files = (q: VersionRow) =>
    db
      .prepare("SELECT path, bytes, sha256, kind FROM version_files WHERE plugin_id = ? AND version = ? ORDER BY path")
      .bind(id, q.version)
      .all<{ path: string; bytes: number; sha256: string; kind: string }>()
      .then((r) => r.results.map((f) => ({ path: f.path, bytes: f.bytes, sha256: f.sha256, kind: f.kind, binary: f.kind === "binary" })));
  const [ownFiles, baseFiles, decisions, recent, authors] = await Promise.all([
    files(v),
    base ? files(base) : Promise.resolve([]),
    db
      .prepare(
        `SELECT SUM(status IN ('approved', 'yanked', 'removed')) AS approved, SUM(status = 'rejected') AS rejected
         FROM versions WHERE publisher_id = ?`
      )
      .bind(v.publisher_id)
      .first<{ approved: number | null; rejected: number | null }>(),
    db
      .prepare("SELECT plugin_id, version, status, submitted_at, reason FROM versions WHERE publisher_id = ? ORDER BY submitted_at DESC LIMIT 10")
      .bind(v.publisher_id)
      .all<{ plugin_id: string; version: string; status: string; submitted_at: number; reason: string | null }>(),
    authorsOf(env, [v.publisher_id, plugin.owner_id])
  ]);
  const publisher = authors[v.publisher_id]!;
  const owner = authors[plugin.owner_id]!;
  const permissions = parseStoredJson<ConsentItem | null>(v.permissions, null);
  const basePermissions = base ? parseStoredJson<ConsentItem | null>(base.permissions, null) : null;
  return answer({
    plugin: { id: plugin.id, status: plugin.status, listed_version: plugin.listed_version, owner: { id: owner.id, username: owner.username } },
    version: { ...versionInfo(v, true), name: v.name, summary: v.summary, icon: v.icon, readme: v.readme, homepage: v.homepage, repository: v.repository, license: v.license, categories: parseStoredJson(v.categories, []), keywords: parseStoredJson(v.keywords, []), screenshots: parseStoredJson(v.screenshots, []) },
    publisher: {
      id: publisher.id,
      username: publisher.username,
      created_at: publisher.createdAt,
      decisions: { approved: decisions?.approved ?? 0, rejected: decisions?.rejected ?? 0 },
      recent: recent.results
    },
    manifest: parseStoredJson(v.manifest, null),
    ...(base ? { base: { version: base.version, manifest: parseStoredJson(base.manifest, null) } } : {}),
    files: ownFiles,
    base_files: baseFiles,
    checks: parseStoredJson(v.checks, []),
    permissions,
    permissions_diff: permissions ? consentDiff(basePermissions, permissions) : null
  });
};

// ── changes ─────────────────────────────────────────────────────────────────

/** Run a guarded change: its UPDATE, the audit row, the listing; 409 if the
 * UPDATE found nothing in the state it leaves. */
const change = async (
  db: D1Database,
  update: D1PreparedStatement,
  audit: D1PreparedStatement,
  id: string,
  author: string | null,
  conflict: string
): Promise<Response | undefined> => {
  const results = await db.batch([update, audit, ...relist(db, id, author)]);
  return (results[0]?.meta.changes ?? 0) === 0 ? fail(409, "invalid_state", conflict) : undefined;
};

const ownerUsername = async (env: Env, ownerId: string): Promise<string | null> => (await authorsOf(env, [ownerId]))[ownerId]!.username;

const reviewVersion = async (env: Env, db: D1Database, adminId: string, id: string, versionText: string, action: "approve" | "reject", body: Record<string, unknown> | undefined): Promise<Response> => {
  const [plugin, v] = await Promise.all([pluginRow(db, id), versionRow(db, id, versionText)]);
  if (!plugin || !v) return notFound();
  let reason: string | null = null;
  let message: string | null = null;
  if (action === "reject") {
    if (typeof body?.reason !== "string" || !REASON_RE.test(body.reason)) return fail(400, "bad_request", "reason is a short word (malware, broken, misleading, …).");
    if (typeof body.message !== "string" || body.message.trim() === "" || body.message.length > MAX_MESSAGE) {
      return fail(400, "bad_request", `message is what the author reads, at most ${MAX_MESSAGE} characters.`);
    }
    reason = body.reason;
    message = body.message.trim();
  }
  if (v.status !== "pending") return fail(409, "invalid_state", `Version ${v.version} is ${v.status}: only a pending version can be reviewed.`);
  const now = Date.now();
  const to = action === "approve" ? "approved" : "rejected";
  const conflict = await change(
    db,
    db
      .prepare("UPDATE versions SET status = ?, decided_at = ?, decided_by = ?, reason = ?, message = ? WHERE plugin_id = ? AND version = ? AND status = 'pending'")
      .bind(to, now, adminId, reason, message, id, v.version),
    auditIfChanged(db, now, adminId, `version.${action}`, id, v.version, reason ? { reason, message } : undefined),
    id,
    await ownerUsername(env, plugin.owner_id),
    "The version changed meanwhile: look again."
  );
  if (conflict) return conflict;
  if (action === "approve") await purge(await pluginCacheKeys(db, plugin, [v.version]));
  return answer({ version: versionInfo((await versionRow(db, id, v.version))!, true) });
};

const removeVersion = async (db: D1Database, adminId: string, id: string, versionText: string, body: Record<string, unknown> | undefined): Promise<Response> => {
  const [plugin, v] = await Promise.all([pluginRow(db, id), versionRow(db, id, versionText)]);
  if (!plugin || !v) return notFound();
  if (typeof body?.block !== "boolean") return fail(400, "bad_request", "block is true (turn it off on devices) or false.");
  if (typeof body.reason !== "string" || body.reason.trim() === "" || body.reason.length > MAX_MESSAGE) {
    return fail(400, "bad_request", `reason is what people read, at most ${MAX_MESSAGE} characters.`);
  }
  if (v.status !== "approved" && v.status !== "yanked") return fail(409, "invalid_state", `Version ${v.version} is ${v.status}: only an approved or yanked version can be removed.`);
  const now = Date.now();
  const reason = body.reason.trim();
  const conflict = await change(
    db,
    db
      .prepare(
        `UPDATE versions SET status = 'removed', blocked = ?, block_reason = ?, decided_at = ?, decided_by = ?, reason = 'removed', message = ?
         WHERE plugin_id = ? AND version = ? AND status IN ('approved', 'yanked')`
      )
      .bind(body.block ? 1 : 0, body.block ? reason : null, now, adminId, reason, id, v.version),
    auditIfChanged(db, now, adminId, "version.remove", id, v.version, { block: body.block, reason }),
    id,
    null,
    "The version changed meanwhile: look again."
  );
  if (conflict) return conflict;
  await purge(await pluginCacheKeys(db, plugin, [v.version]));
  return answer({ version: versionInfo((await versionRow(db, id, v.version))!, true) });
};

const listedVersions = async (db: D1Database, id: string): Promise<string[]> =>
  (await db.prepare("SELECT version FROM versions WHERE plugin_id = ? AND status IN ('approved', 'yanked')").bind(id).all<{ version: string }>()).results.map((r) => r.version);

const pluginAction = async (env: Env, db: D1Database, adminId: string, id: string, action: string, body: Record<string, unknown> | undefined): Promise<Response> => {
  const plugin = await pluginRow(db, id);
  if (!plugin) return notFound();
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
      conflict = await change(
        db,
        body.featured
          ? db.prepare("UPDATE plugins SET featured_at = ? WHERE id = ? AND featured_at IS NULL").bind(now, id)
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
        db.prepare("UPDATE plugins SET owner_id = ? WHERE id = ? AND owner_id = ?").bind(to, id, plugin.owner_id),
        auditIfChanged(db, now, adminId, "plugin.transfer", id, null, { from: plugin.owner_id, to, to_username: username }),
        id,
        username!,
        "It changed hands meanwhile: look again."
      );
      if (!conflict) await revokeDeployKeys(env, id);
      break;
    }
    default:
      return notFound();
  }
  if (conflict) return conflict;
  await purge(await pluginCacheKeys(db, plugin, action === "feature" ? [] : await listedVersions(db, id)));
  return adminPlugin(env, db, id);
};

// ── a plugin, reports, audit, reservations ──────────────────────────────────

const adminPlugin = async (env: Env, db: D1Database, id: string): Promise<Response> => {
  const plugin = await pluginRow(db, id);
  if (!plugin) return notFound();
  const [versions, reports, authors] = await Promise.all([
    db.prepare(`SELECT * FROM versions WHERE plugin_id = ? ORDER BY ${VERSION_ORDER}`).bind(id).all<VersionRow>(),
    db.prepare("SELECT id, version, reason, text, status, created_at, resolved_at, note FROM reports WHERE plugin_id = ? ORDER BY id DESC LIMIT 100").bind(id).all(),
    authorsOf(env, [plugin.owner_id])
  ]);
  const owner = authors[plugin.owner_id]!;
  return answer({
    plugin: { ...pluginCard(plugin, authorCard(owner)), listed_version: plugin.listed_version, created_at: plugin.created_at, suspend_reason: plugin.suspend_reason },
    owner: { id: owner.id, username: owner.username, user_id: plugin.owner_id, created_at: owner.createdAt },
    versions: versions.results.map((v) => versionInfo(v, true)),
    reports: reports.results,
    featured: plugin.featured_at !== null,
    status: plugin.status
  });
};

const reports = async (env: Env, db: D1Database, url: URL): Promise<Response> => {
  const status = url.searchParams.get("status") ?? "open";
  if (!["open", "dismissed", "actioned", "all"].includes(status)) return fail(400, "bad_request", "status is open, dismissed, actioned or all.");
  const after = keyset(url.searchParams.get("cursor"));
  if (after === null) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  const limit = limitOf(url);
  if (limit === null) return fail(400, "bad_request", `limit is 1 to ${PAGE}.`);
  const ks = keysetSql("r.id", "r.id", false, after);
  const rows = (
    await db
      .prepare(
        `SELECT r.*, p.name AS plugin_name FROM reports r LEFT JOIN plugins p ON p.id = r.plugin_id
         WHERE (? = 'all' OR r.status = ?) ${ks.where} ORDER BY ${ks.order} LIMIT ?`
      )
      .bind(status, status, ...ks.binds, limit + 1)
      .all<{ id: number; plugin_id: string; plugin_name: string | null; version: string | null; reporter_id: string; reason: string; text: string; created_at: number; status: string; resolved_by: string | null; resolved_at: number | null; note: string | null }>()
  ).results;
  const { page, links } = pageOf(rows, limit, after, (r) => [r.id, r.id]);
  const names = await people(env, [...page.map((r) => r.reporter_id), ...page.flatMap((r) => (r.resolved_by ? [r.resolved_by] : []))]);
  return answer({
    reports: page.map((r) => ({
      id: r.id,
      plugin: { id: r.plugin_id, name: r.plugin_name },
      version: r.version,
      reporter: names[r.reporter_id] ?? null,
      reason: r.reason,
      text: r.text,
      created_at: r.created_at,
      status: r.status,
      resolved_by: r.resolved_by ? (names[r.resolved_by] ?? null) : null,
      resolved_at: r.resolved_at,
      note: r.note
    })),
    ...links
  });
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
  if (pluginFilter !== null && !ID_RE.test(pluginFilter)) return fail(400, "bad_request", "plugin is a plugin id.");
  const rows = (
    await db
      .prepare(`SELECT * FROM audit WHERE (? IS NULL OR plugin_id = ?) ${ks.where} ORDER BY ${ks.order} LIMIT ?`)
      .bind(pluginFilter, pluginFilter, ...ks.binds, limit + 1)
      .all<{ seq: number; at: number; actor_id: string; action: string; plugin_id: string | null; version: string | null; detail: string | null }>()
  ).results;
  const { page, links } = pageOf(rows, limit, after, (r) => [r.seq, r.seq]);
  const names = await people(env, page.map((r) => r.actor_id));
  return answer({
    entries: page.map((r) => ({
      seq: r.seq,
      at: r.at,
      actor: { id: names[r.actor_id]?.id ?? null, username: names[r.actor_id]?.username ?? null, user_id: r.actor_id },
      action: r.action,
      plugin_id: r.plugin_id,
      version: r.version,
      detail: parseStoredJson(r.detail, null)
    })),
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
  if (section === "submissions" && a && b) {
    const versionText = decodeSegment(b);
    if (!versionText || !ID_RE.test(a)) return notFound();
    if (parts.length === 4 && get) return submission(env, db, a, versionText);
    if (parts.length === 5 && post && (c === "approve" || c === "reject")) return reviewVersion(env, db, admin.userId, a, versionText, c, body);
  }
  if (section === "plugins" && a && ID_RE.test(a)) {
    if (parts.length === 3 && get) return adminPlugin(env, db, a);
    if (parts.length === 4 && post && b) return pluginAction(env, db, admin.userId, a, b, body);
    if (parts.length === 6 && post && b === "versions" && c && d === "remove") {
      const versionText = decodeSegment(c);
      return versionText ? removeVersion(db, admin.userId, a, versionText, body) : notFound();
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
