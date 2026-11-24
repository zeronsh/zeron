/**
 * Public catalog reads (contract "Public", amendments 10, 38, 39, 45;
 * docs/plans/2026-11-19-store-shapes-44-45.md):
 *
 *   GET /store/v1/plugins?q&category&keyword&author&featured&verified&sort&cursor&limit
 *                                                → { items, total, next_cursor?, prev_cursor? }
 *   GET /store/v1/plugins/:id                    → PluginDetail
 *   GET /store/v1/plugins/:id/versions?cursor&limit → { versions, total, next_cursor?, prev_cursor? }
 *   GET /store/v1/plugins/:id/versions/:version  → VersionInfo
 *   GET /store/v1/categories?top                 → { categories: [{ id, name, count, top? }] }
 *   GET /store/v1/keywords?category&limit        → { keywords: [{ keyword, count }] }
 *   GET /store/v1/stats                          → { plugins, verified, new_7d }
 *   GET /store/v1/takedowns?after=<seq>          → { events, head, reset? }
 *
 * Every listed plugin is in the catalog the moment it is published; a
 * review only marks it. Sorts are honest: each is exactly its own key,
 * verified or not, and `verified=1` is a filter. Search relevance uses the
 * mark only to break a tie. Featured is the vetted shelf: only plugins
 * whose newest version is verified.
 *
 * Search is FTS5 over listed plugins (name, summary, keywords, author,
 * README; weighted 10/5/3/2/1, bm25), every word of the query a quoted
 * prefix term, so nothing typed is ever FTS syntax. Paging is keyset: a
 * cursor names the last item's sort key and seq and the query it belongs
 * to, so pages never repeat or skip an item that stays where it was.
 * An unknown or suspended plugin is 404 `not_found` to the public.
 */
import type { Env } from "../env";
import { userDirectoryStub } from "../user-directory";
import { foldUsername } from "../username";
import { LIST_TTL, cacheKey, cachedValue, throughCache } from "./cache";
import { hasCredentials } from "./principal";
import { authorCard, authorsOf, packageUrl, permissionsOf, pluginCard, versionInfo } from "./cards";
import { LISTED, PUBLIC_STATUSES, type PluginRow, type VersionRow, parseList, pluginRow, publicVersion } from "./db";
import { answer, fail, notFound } from "./http";
import { CATEGORIES, isCategory } from "./listing";
import { type VersionPlace, decodeCursor, decodeVersionCursor, encodeCursor, encodeVersionCursor, ftsQuery, searchTerms } from "./search";
import { decodeSegment } from "./serve";
import { parseVersion } from "./semver";

export const SORTS = ["relevance", "popular", "updated", "new", "verified"] as const;
export type Sort = (typeof SORTS)[number];
export const DEFAULT_LIMIT = 24;
export const MAX_LIMIT = 50;
const MAX_QUERY = 200;
/** The versions a detail carries; the rest page from `/versions`. */
const DETAIL_VERSIONS = 5;
/** The take-down feed's page, and how long it keeps an event. */
export const TAKEDOWN_PAGE = 500;
export const TAKEDOWN_DAYS = 30;

const publicCache = (ttl: number) => `public, max-age=${ttl}`;

interface ListQuery {
  readonly q: string | undefined;
  readonly terms: readonly string[];
  readonly category: string | undefined;
  readonly keyword: string | undefined;
  /** A publisher's username, as the username rule folds it. */
  readonly author: string | undefined;
  readonly featured: boolean;
  readonly verified: boolean;
  readonly sort: Sort;
  readonly limit: number;
  readonly cursor: string | undefined;
}

const flag = (p: URLSearchParams, name: string): boolean | undefined | Response => {
  const raw = p.get(name);
  if (raw === null) return undefined;
  if (raw === "true" || raw === "1") return true;
  if (raw === "false" || raw === "0") return false;
  return fail(400, "bad_request", `${name} is true or false.`);
};

const parseListQuery = (url: URL): ListQuery | Response => {
  const p = url.searchParams;
  const qRaw = p.get("q")?.trim();
  const q = qRaw ? qRaw : undefined;
  if (q && q.length > MAX_QUERY) return fail(400, "bad_request", `A search is at most ${MAX_QUERY} characters.`);
  const category = p.get("category") ?? undefined;
  if (category !== undefined && !isCategory(category)) return fail(400, "bad_request", "That isn't a category.");
  const keyword = p.get("keyword")?.trim().toLowerCase() || undefined;
  if (keyword !== undefined && keyword.length > 32) return fail(400, "bad_request", "A keyword is at most 32 characters.");
  // Checked against the username rule and folded (lowercased) before it is
  // a cache key; one that isn't a username is refused, one nobody holds
  // answers the same empty list as one with no plugins.
  const authorRaw = p.get("author");
  const author = authorRaw === null ? undefined : foldUsername(authorRaw);
  if (authorRaw !== null && author === undefined) return fail(400, "bad_request", "author is a username.");
  const featured = flag(p, "featured");
  if (featured instanceof Response) return featured;
  const verified = flag(p, "verified");
  if (verified instanceof Response) return verified;
  const sortRaw = p.get("sort");
  if (sortRaw !== null && !SORTS.includes(sortRaw as Sort)) return fail(400, "bad_request", `sort is one of ${SORTS.join(", ")}.`);
  const sort: Sort = (sortRaw as Sort | null) ?? (q ? "relevance" : "popular");
  if (sort === "relevance" && !q) return fail(400, "bad_request", "Sorting by relevance needs a search.");
  const limit = limitOf(p, DEFAULT_LIMIT);
  if (limit === undefined) return fail(400, "bad_request", `limit is 1 to ${MAX_LIMIT}.`);
  return { q, terms: q ? searchTerms(q) : [], category, keyword, author, featured: featured ?? false, verified: verified ?? false, sort, limit, cursor: p.get("cursor") ?? undefined };
};

const limitOf = (p: URLSearchParams, fallback: number): number | undefined => {
  const raw = p.get("limit");
  const limit = raw === null ? fallback : Number(raw);
  return Number.isSafeInteger(limit) && limit >= 1 && limit <= MAX_LIMIT ? limit : undefined;
};

type ListKey = Pick<ListQuery, "terms" | "category" | "keyword" | "author" | "featured" | "verified" | "sort" | "limit" | "cursor"> & { readonly q?: string };

/** The cache key params of a list query: only what changes its answer. */
export const listParams = (lq: ListKey) => ({
  // A search with no searchable word ("!!!") answers nothing: keyed as a
  // search (`q=`), never as the list without one.
  q: lq.q === undefined ? undefined : lq.terms.join(" "),
  category: lq.category,
  keyword: lq.keyword,
  author: lq.author,
  featured: lq.featured ? "true" : undefined,
  verified: lq.verified ? "true" : undefined,
  sort: lq.sort,
  limit: lq.limit === DEFAULT_LIMIT ? undefined : String(lq.limit),
  cursor: lq.cursor
});

/** The keys of the first pages a change to a plugin in `categories` moves:
 * each sort, featured or not, verified or not. */
export const firstPageKeys = (categories: readonly string[]): string[] => {
  const keys: string[] = [];
  for (const sort of ["popular", "updated", "new", "verified"] as const) {
    for (const featured of [false, true]) {
      for (const verified of [false, true]) {
        for (const category of [undefined, ...categories]) {
          keys.push(
            cacheKey(["plugins"], listParams({ terms: [], category, keyword: undefined, author: undefined, featured, verified, sort, limit: DEFAULT_LIMIT, cursor: undefined }))
          );
        }
      }
    }
  }
  return keys;
};

/** The cache keys of a plugin's versions' first page and one version. */
export const STATS_KEY = cacheKey(["stats"]);
/** The categories with their counts and each one's most installed plugins
 * (as many as `top` may ask for): one cached value every `?top` reads. */
export const CATEGORIES_KEY = cacheKey(["categories"]);
/** The most used keywords, of all listed plugins or of one category's (as
 * many as `limit` may ask for): one cached value each. */
export const keywordsKey = (category: string | undefined): string => cacheKey(["keywords"], { category });
/** The take-down feed's answer for a device at `after` (none: one that
 * starts afresh). */
export const takedownsKey = (after: number | undefined): string => cacheKey(["takedowns"], { after: after === undefined ? undefined : String(after) });
export const versionsPageKey = (id: string): string => cacheKey(["plugins", id, "versions"]);
export const versionKey = (id: string, version: string): string => cacheKey(["plugins", id, "versions", version]);

const SORT_KEY: Record<Sort, { readonly expr: string; readonly asc: boolean }> = {
  // (The mark breaks a tie between equal ranks, and nothing more.)
  relevance: { expr: "bm25(plugin_search, 10.0, 5.0, 3.0, 2.0, 1.0) - p.verified * 1e-9", asc: true },
  popular: { expr: "p.installs", asc: false },
  updated: { expr: "p.updated_at", asc: false },
  new: { expr: "p.listed_at", asc: false },
  // Newly verified: when its newest verified version was verified (only
  // plugins with one).
  verified: { expr: "p.verified_at", asc: false }
};

const listPlugins = async (env: Env, db: D1Database, lq: ListQuery): Promise<Response> => {
  const filter = JSON.stringify(listParams({ ...lq, cursor: undefined, limit: DEFAULT_LIMIT }));
  let after: { key: number; seq: number; dir: "next" | "prev" } | undefined;
  if (lq.cursor !== undefined) {
    after = decodeCursor(lq.cursor, filter);
    if (!after) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  }
  const none = () => answer({ items: [], total: 0 }, 200, publicCache(LIST_TTL));
  if (lq.q && lq.terms.length === 0) return none();
  let ownerId: string | undefined;
  if (lq.author !== undefined) {
    const directory = userDirectoryStub(env);
    ownerId = directory ? await directory.userIdForUsername(lq.author) : undefined;
    if (!ownerId) return none();
  }
  // Each clause with its own binds, assembled in SQL order.
  const joins: { sql: string; binds: unknown[] }[] = [];
  const where: { sql: string; binds: unknown[] }[] = [{ sql: LISTED, binds: [] }];
  if (lq.terms.length) {
    joins.push({ sql: "JOIN plugin_search ON plugin_search.rowid = p.seq", binds: [] });
    where.push({ sql: "plugin_search MATCH ?", binds: [ftsQuery(lq.terms)] });
  }
  if (lq.category) joins.push({ sql: "JOIN plugin_categories c ON c.plugin_id = p.id AND c.category = ?", binds: [lq.category] });
  if (lq.keyword) joins.push({ sql: "JOIN plugin_keywords k ON k.plugin_id = p.id AND k.keyword = ?", binds: [lq.keyword] });
  if (ownerId) where.push({ sql: "p.owner_id = ?", binds: [ownerId] });
  // Featured is the vetted shelf: only a plugin whose newest version is verified.
  if (lq.featured) where.push({ sql: "p.featured_at IS NOT NULL AND p.verified = 1", binds: [] });
  if (lq.verified) where.push({ sql: "p.verified = 1", binds: [] });
  if (lq.sort === "verified") where.push({ sql: "p.verified_at IS NOT NULL", binds: [] });
  const sort = SORT_KEY[lq.sort];
  // Paging back reads the other way from the cursor's item, then turns round.
  const back = after?.dir === "prev";
  const forwardAsc = sort.asc !== back;
  const keyset = after
    ? { sql: `WHERE (sort_key ${forwardAsc ? ">" : "<"} ? OR (sort_key = ? AND seq ${back ? ">" : "<"} ?))`, binds: [after.key, after.key, after.seq] }
    : { sql: "", binds: [] };
  const sql = `SELECT * FROM (
      SELECT p.*, COALESCE(${sort.expr}, 0) AS sort_key FROM plugins p
      ${joins.map((j) => j.sql).join(" ")}
      WHERE ${where.map((w) => w.sql).join(" AND ")}
    ) ${keyset.sql}
    ORDER BY sort_key ${forwardAsc ? "ASC" : "DESC"}, seq ${back ? "ASC" : "DESC"} LIMIT ?`;
  const filterBinds = [...joins.flatMap((j) => j.binds), ...where.flatMap((w) => w.binds)];
  // `total` (amendment 38): what the same filter matches, whichever page
  // this is, read in the same batch as the page so the two agree. Only
  // listed plugins count, as only they are shown.
  const countSql = `SELECT COUNT(*) AS n FROM plugins p ${joins.map((j) => j.sql).join(" ")} WHERE ${where.map((w) => w.sql).join(" AND ")}`;
  const [pageResult, countResult] = await db.batch<unknown>([
    db.prepare(sql).bind(...filterBinds, ...keyset.binds, lq.limit + 1),
    db.prepare(countSql).bind(...filterBinds)
  ]);
  const rows = pageResult!.results as (PluginRow & { sort_key: number })[];
  const total = (countResult!.results[0] as { n: number }).n;
  const more = rows.length > lq.limit;
  const page = rows.slice(0, lq.limit);
  if (back) page.reverse();
  const authors = await authorsOf(env, page.map((p) => p.owner_id));
  const first = page[0];
  const last = page[page.length - 1];
  // Forward: more after, and (with a cursor) something before. Back: more
  // before, and the page it came from after.
  const hasNext = back ? true : more;
  const hasPrev = back ? more : after !== undefined;
  return answer(
    {
      items: page.map((p) => pluginCard(p, authorCard(authors[p.owner_id]!))),
      total,
      ...(hasNext && last ? { next_cursor: encodeCursor(filter, last.sort_key, last.seq) } : {}),
      ...(hasPrev && first ? { prev_cursor: encodeCursor(filter, first.sort_key, first.seq, "prev") } : {})
    },
    200,
    publicCache(LIST_TTL)
  );
};

/** A listed plugin, or undefined (unknown, suspended, or nothing installable). */
const listedPlugin = async (db: D1Database, id: string): Promise<PluginRow | undefined> => {
  const p = await pluginRow(db, id);
  return p && p.status === "active" && p.listed_version ? p : undefined;
};

/** An active plugin (listed or not): its listed and yanked versions stay
 * readable, a yanked one for the devices that have it. */
const activePlugin = async (db: D1Database, id: string): Promise<PluginRow | undefined> => {
  const p = await pluginRow(db, id);
  return p && p.status === "active" ? p : undefined;
};

const placeOf = (v: VersionRow): VersionPlace => [v.major, v.minor, v.patch, v.is_release, v.pre_key];

/** A page of plugin `id`'s public versions, newest first, from `after`. */
const versionsPage = async (db: D1Database, id: string, limit: number, after: { place: VersionPlace; dir: "next" | "prev" } | undefined) => {
  const back = after?.dir === "prev";
  const cmp = back ? ">" : "<";
  const dir = back ? "ASC" : "DESC";
  const order = ["major", "minor", "patch", "is_release", "pre_key"].map((c) => `${c} ${dir}`).join(", ");
  const [rows, counted] = await db.batch<unknown>([
    db
      .prepare(
        `SELECT * FROM versions WHERE plugin_id = ? AND ${publicVersion()}
           ${after ? `AND (major, minor, patch, is_release, pre_key) ${cmp} (?, ?, ?, ?, ?)` : ""}
         ORDER BY ${order} LIMIT ?`
      )
      .bind(id, ...(after ? after.place : []), limit + 1),
    db.prepare(`SELECT COUNT(*) AS n FROM versions WHERE plugin_id = ? AND ${publicVersion()}`).bind(id)
  ]);
  const found = rows!.results as VersionRow[];
  const more = found.length > limit;
  const page = found.slice(0, limit);
  if (back) page.reverse();
  const first = page[0];
  const last = page[page.length - 1];
  const hasNext = back ? true : more;
  const hasPrev = back ? more : after !== undefined;
  return {
    versions: page,
    total: (counted!.results[0] as { n: number }).n,
    next: hasNext && last ? encodeVersionCursor(id, placeOf(last)) : undefined,
    prev: hasPrev && first ? encodeVersionCursor(id, placeOf(first), "prev") : undefined
  };
};

const pluginDetail = async (env: Env, db: D1Database, id: string): Promise<Response> => {
  const p = await listedPlugin(db, id);
  if (!p) return notFound();
  const [listed, versions, authors] = await Promise.all([
    db.prepare("SELECT * FROM versions WHERE plugin_id = ? AND version = ?").bind(id, p.listed_version).first<VersionRow>(),
    versionsPage(db, id, DETAIL_VERSIONS, undefined),
    authorsOf(env, [p.owner_id])
  ]);
  if (!listed) return notFound();
  return answer(
    {
      ...pluginCard(p, authorCard(authors[p.owner_id]!)),
      readme: listed.readme,
      screenshots: parseList(listed.screenshots),
      permissions: permissionsOf(listed),
      engines: listed.engines_zeron === null ? null : { zeron: listed.engines_zeron },
      homepage: listed.homepage,
      repository: listed.repository,
      license: listed.license,
      versions: versions.versions.map((v) => versionInfo(v)),
      ...(versions.next ? { versions_next_cursor: versions.next } : {}),
      package: {
        url: packageUrl(id, listed.version),
        bytes: listed.package_bytes,
        sha256: listed.package_sha256,
        hash: listed.content_hash,
        key: listed.signer_key
      }
    },
    200,
    publicCache(LIST_TTL)
  );
};

const pluginVersions = async (db: D1Database, id: string, url: URL): Promise<Response> => {
  if (!(await activePlugin(db, id))) return notFound();
  const limit = limitOf(url.searchParams, 20);
  if (limit === undefined) return fail(400, "bad_request", `limit is 1 to ${MAX_LIMIT}.`);
  const cursor = url.searchParams.get("cursor");
  const after = cursor === null ? undefined : decodeVersionCursor(cursor, id);
  if (after === undefined && cursor !== null) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  const page = await versionsPage(db, id, limit, after);
  return answer(
    {
      versions: page.versions.map((v) => versionInfo(v)),
      total: page.total,
      ...(page.next ? { next_cursor: page.next } : {}),
      ...(page.prev ? { prev_cursor: page.prev } : {})
    },
    200,
    publicCache(LIST_TTL)
  );
};

/** One public version of an active plugin, by its name. */
const oneVersion = async (db: D1Database, id: string, version: string): Promise<Response> => {
  if (!(await activePlugin(db, id))) return notFound();
  const v = await db.prepare(`SELECT * FROM versions WHERE plugin_id = ? AND version = ? AND ${publicVersion()}`).bind(id, version).first<VersionRow>();
  return v ? answer(versionInfo(v), 200, publicCache(LIST_TTL)) : notFound();
};

/** Each category with the number of listed plugins in it: exactly the set
 * its list shows. Counted when read (nothing to keep in step), cached and
 * purged with the lists. */
const MAX_TOP = 4;
const MAX_KEYWORDS_SHOWN = 24;
const DAY_MS = 86_400_000;

/** A small count from `p.get(name)`: digits only, `min` to `max`; the
 * default when absent; undefined when it isn't one. */
const countParam = (p: URLSearchParams, name: string, min: number, max: number, fallback: number | undefined): number | undefined | null => {
  const raw = p.get(name);
  if (raw === null) return fallback;
  if (!/^[0-9]{1,3}$/.test(raw)) return null;
  const n = Number(raw);
  return n >= min && n <= max ? n : null;
};

/** An answer from a cached value, public unless the request carries
 * credentials (cachedValue reads afresh for them). */
const fromValue = (request: Request, value: unknown): Response =>
  answer(value, 200, hasCredentials(request) ? "private, no-store" : publicCache(LIST_TTL));

interface CategoryEntry {
  readonly id: string;
  readonly name: string;
  readonly count: number;
  readonly top: readonly ReturnType<typeof pluginCard>[];
}

/** Each category: how many listed plugins it has, and its `MAX_TOP` most
 * installed, in the order and set `?category=<id>&sort=popular` lists. */
const categoryEntries = async (env: Env, db: D1Database): Promise<CategoryEntry[]> => {
  const [counted, ranked] = await db.batch<unknown>([
    db.prepare(`SELECT c.category, COUNT(*) AS n FROM plugin_categories c JOIN plugins p ON p.id = c.plugin_id WHERE ${LISTED} GROUP BY c.category`),
    db
      .prepare(
        `SELECT * FROM (
           SELECT c.category AS top_category, p.*,
             ROW_NUMBER() OVER (PARTITION BY c.category ORDER BY COALESCE(${SORT_KEY.popular.expr}, 0) DESC, p.seq DESC) AS top_rank
           FROM plugin_categories c JOIN plugins p ON p.id = c.plugin_id WHERE ${LISTED}
         ) WHERE top_rank <= ? ORDER BY top_category, top_rank`
      )
      .bind(MAX_TOP)
  ]);
  const counts = new Map((counted!.results as { category: string; n: number }[]).map((r) => [r.category, r.n]));
  const rows = ranked!.results as (PluginRow & { top_category: string })[];
  const authors = await authorsOf(env, rows.map((r) => r.owner_id));
  return CATEGORIES.map((c) => ({
    ...c,
    count: counts.get(c.id) ?? 0,
    top: rows.filter((r) => r.top_category === c.id).map((r) => pluginCard(r, authorCard(authors[r.owner_id]!)))
  }));
};

/** `GET /store/v1/categories[?top=1..4]`: each category with its count,
 * and with `top`, its most installed plugins as cards. */
const categories = async (request: Request, env: Env, db: D1Database, url: URL): Promise<Response> => {
  const top = countParam(url.searchParams, "top", 1, MAX_TOP, undefined);
  if (top === null) return fail(400, "bad_request", `top is 1 to ${MAX_TOP}.`);
  const entries = (await cachedValue(request, CATEGORIES_KEY, LIST_TTL, () => categoryEntries(env, db)))!;
  return fromValue(request, { categories: entries.map(({ top: cards, ...c }) => (top === undefined ? c : { ...c, top: cards.slice(0, top) })) });
};

/** `GET /store/v1/keywords[?category&limit=1..24]`: the keywords listed
 * plugins use most (of one category's, with `category`), most used first,
 * ties by keyword. */
const keywords = async (request: Request, db: D1Database, url: URL): Promise<Response> => {
  const category = url.searchParams.get("category") ?? undefined;
  if (category !== undefined && !isCategory(category)) return fail(400, "bad_request", "That isn't a category.");
  const limit = countParam(url.searchParams, "limit", 1, MAX_KEYWORDS_SHOWN, MAX_KEYWORDS_SHOWN);
  if (limit === null || limit === undefined) return fail(400, "bad_request", `limit is 1 to ${MAX_KEYWORDS_SHOWN}.`);
  const all = (await cachedValue(request, keywordsKey(category), LIST_TTL, async () =>
    (
      await db
        .prepare(
          `SELECT k.keyword, COUNT(*) AS count FROM plugin_keywords k JOIN plugins p ON p.id = k.plugin_id
             ${category ? "JOIN plugin_categories c ON c.plugin_id = p.id AND c.category = ?" : ""}
           WHERE ${LISTED} GROUP BY k.keyword ORDER BY count DESC, k.keyword ASC LIMIT ?`
        )
        .bind(...(category ? [category] : []), MAX_KEYWORDS_SHOWN)
        .all<{ keyword: string; count: number }>()
    ).results
  ))!;
  return fromValue(request, { keywords: all.slice(0, limit) });
};

/** The store's totals: listed plugins, those whose newest version is
 * verified (what `?verified=1` lists), and those first listed in the last
 * seven days. */
const stats = async (db: D1Database): Promise<Response> => {
  const row = await db
    .prepare(`SELECT COUNT(*) AS plugins, COALESCE(SUM(p.verified), 0) AS verified, COALESCE(SUM(p.listed_at >= ?), 0) AS new_7d FROM plugins p WHERE ${LISTED}`)
    .bind(Date.now() - 7 * DAY_MS)
    .first<{ plugins: number; verified: number; new_7d: number }>();
  return answer({ plugins: row?.plugins ?? 0, verified: row?.verified ?? 0, new_7d: row?.new_7d ?? 0 }, 200, publicCache(LIST_TTL));
};

/**
 * The take-down feed: every take-down and restore after `after`, in `seq`
 * order (the order they committed), at most `TAKEDOWN_PAGE` a page. A device
 * keeps the last `seq` it applied; an `after` older than the feed keeps (30
 * days), or none, answers `{reset: true, head}`, and the device runs its
 * full update check before going on from `head`. A cached answer can only
 * delay an event, never skip one: a device asks again from the `seq` it got.
 */
const takedowns = async (db: D1Database, after: number | undefined): Promise<Response> => {
  // (The 30-day prune always keeps the newest event, so the head never
  // goes back; AUTOINCREMENT never reuses a number.)
  const [headRow, oldestRow] = await db.batch<{ n: number | null }>([
    db.prepare("SELECT MAX(seq) AS n FROM takedown_events"),
    db.prepare("SELECT MIN(seq) AS n FROM takedown_events")
  ]);
  const head = headRow!.results[0]?.n ?? 0;
  const oldest = oldestRow!.results[0]?.n ?? head + 1;
  // Events after `after` were pruned when the oldest kept is past after + 1.
  if (after === undefined || after > head || oldest > after + 1) return answer({ events: [], head, reset: true }, 200, publicCache(LIST_TTL));
  const rows = (
    await db
      .prepare("SELECT seq, kind, plugin_id, version, reason FROM takedown_events WHERE seq > ? ORDER BY seq LIMIT ?")
      .bind(after, TAKEDOWN_PAGE)
      .all<{ seq: number; kind: string; plugin_id: string; version: string; reason: string | null }>()
  ).results;
  return answer(
    {
      events: rows.map((r) => ({ seq: r.seq, kind: r.kind, id: r.plugin_id, version: r.version, ...(r.reason ? { reason: r.reason } : {}) })),
      head
    },
    200,
    publicCache(LIST_TTL)
  );
};

export const handleCatalog = async (request: Request, env: Env, db: D1Database, url: URL, parts: readonly string[]): Promise<Response | undefined> => {
  if (request.method !== "GET" && request.method !== "HEAD") return undefined;
  if (parts.length === 3 && parts[2] === "categories") return categories(request, env, db, url);
  if (parts.length === 3 && parts[2] === "keywords") return keywords(request, db, url);
  if (parts.length === 3 && parts[2] === "stats") return throughCache(request, STATS_KEY, LIST_TTL, () => stats(db));
  if (parts.length === 3 && parts[2] === "takedowns") {
    // Read once: the key and the answer are the same `after`. Anything but
    // digits is refused, never cached (so no spelling of a seq can fill the
    // fresh start's entry with a page that isn't a reset).
    const raw = url.searchParams.get("after");
    if (raw !== null && !/^[0-9]{1,15}$/.test(raw)) return fail(400, "bad_request", "after is a seq the feed gave.");
    const after = raw === null ? undefined : Number(raw);
    // (Keyed by the `after` it answers, so a device that is up to date and
    // every other at the same place share one answer.)
    return throughCache(request, takedownsKey(after), LIST_TTL, () => takedowns(db, after));
  }
  if (parts[2] !== "plugins") return undefined;
  if (parts.length === 3) {
    const lq = parseListQuery(url);
    if (lq instanceof Response) return lq;
    return throughCache(request, cacheKey(["plugins"], listParams(lq)), LIST_TTL, () => listPlugins(env, db, lq));
  }
  const id = parts[3]!;
  if (!/^[a-z0-9][a-z0-9-]{1,39}$/.test(id)) return notFound();
  if (parts.length === 4) return throughCache(request, cacheKey(["plugins", id]), LIST_TTL, () => pluginDetail(env, db, id));
  if (parts.length === 5 && parts[4] === "versions") {
    // The first page only is cached (and purged); later pages are as fresh as a list's.
    const plain = url.search === "";
    return plain ? throughCache(request, versionsPageKey(id), LIST_TTL, () => pluginVersions(db, id, url)) : pluginVersions(db, id, url);
  }
  if (parts.length === 6 && parts[4] === "versions") {
    const version = decodeSegment(parts[5]!);
    if (!version || !parseVersion(version)) return notFound();
    return throughCache(request, versionKey(id, version), LIST_TTL, () => oneVersion(db, id, version));
  }
  return undefined;
};

export { PUBLIC_STATUSES };
