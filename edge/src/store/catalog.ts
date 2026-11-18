/**
 * Public catalog reads (contract "Public", amendment 10):
 *
 *   GET /store/v1/plugins?q&category&keyword&featured&sort&cursor&limit → { items, next_cursor?, prev_cursor? }
 *   GET /store/v1/plugins/:id            → PluginDetail
 *   GET /store/v1/plugins/:id/versions   → { versions }
 *   GET /store/v1/categories             → { categories }
 *
 * Search is FTS5 over listed plugins (name, summary, keywords, author,
 * README; weighted 10/5/3/2/1, bm25), every word of the query a quoted
 * prefix term, so nothing typed is ever FTS syntax. Paging is keyset: a
 * cursor names the last item's sort key and seq and the query it belongs
 * to, so pages never repeat or skip an item that stays where it was.
 * An unknown or suspended plugin is 404 `not_found` to the public.
 */
import type { Env } from "../env";
import { LIST_TTL, cacheKey, throughCache } from "./cache";
import { authorCard, authorsOf, fileUrl, packageUrl, permissionsOf, pluginCard, versionInfo } from "./cards";
import { LISTED, PUBLIC_STATUSES, type PluginRow, VERSION_ORDER, type VersionRow, parseList, pluginRow } from "./db";
import { answer, fail, notFound } from "./http";
import { CATEGORIES, isCategory } from "./listing";
import { decodeCursor, encodeCursor, ftsQuery, searchTerms } from "./search";

export const SORTS = ["relevance", "popular", "updated", "new"] as const;
export type Sort = (typeof SORTS)[number];
export const DEFAULT_LIMIT = 24;
export const MAX_LIMIT = 50;
const MAX_QUERY = 200;
const DETAIL_VERSIONS = 5;

const publicCache = (ttl: number) => `public, max-age=${ttl}`;

interface ListQuery {
  readonly q: string | undefined;
  readonly terms: readonly string[];
  readonly category: string | undefined;
  readonly keyword: string | undefined;
  readonly featured: boolean;
  readonly sort: Sort;
  readonly limit: number;
  readonly cursor: string | undefined;
}

const parseListQuery = (url: URL): ListQuery | Response => {
  const p = url.searchParams;
  const qRaw = p.get("q")?.trim();
  const q = qRaw ? qRaw : undefined;
  if (q && q.length > MAX_QUERY) return fail(400, "bad_request", `A search is at most ${MAX_QUERY} characters.`);
  const category = p.get("category") ?? undefined;
  if (category !== undefined && !isCategory(category)) return fail(400, "bad_request", "That isn't a category.");
  const keyword = p.get("keyword")?.trim().toLowerCase() || undefined;
  if (keyword !== undefined && keyword.length > 32) return fail(400, "bad_request", "A keyword is at most 32 characters.");
  const featuredRaw = p.get("featured");
  if (featuredRaw !== null && featuredRaw !== "true" && featuredRaw !== "false") return fail(400, "bad_request", "featured is true or false.");
  const sortRaw = p.get("sort");
  if (sortRaw !== null && !SORTS.includes(sortRaw as Sort)) return fail(400, "bad_request", `sort is one of ${SORTS.join(", ")}.`);
  const sort: Sort = (sortRaw as Sort | null) ?? (q ? "relevance" : "popular");
  if (sort === "relevance" && !q) return fail(400, "bad_request", "Sorting by relevance needs a search.");
  const limitRaw = p.get("limit");
  const limit = limitRaw === null ? DEFAULT_LIMIT : Number(limitRaw);
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > MAX_LIMIT) return fail(400, "bad_request", `limit is 1 to ${MAX_LIMIT}.`);
  return { q, terms: q ? searchTerms(q) : [], category, keyword, featured: featuredRaw === "true", sort, limit, cursor: p.get("cursor") ?? undefined };
};

/** The cache key params of a list query: only what changes its answer. */
export const listParams = (lq: Pick<ListQuery, "terms" | "category" | "keyword" | "featured" | "sort" | "limit" | "cursor">) => ({
  q: lq.terms.length ? lq.terms.join(" ") : undefined,
  category: lq.category,
  keyword: lq.keyword,
  featured: lq.featured ? "true" : undefined,
  sort: lq.sort,
  limit: lq.limit === DEFAULT_LIMIT ? undefined : String(lq.limit),
  cursor: lq.cursor
});

/** The keys of the first pages a change to a plugin in `categories` moves. */
export const firstPageKeys = (categories: readonly string[]): string[] => {
  const keys: string[] = [];
  for (const sort of ["popular", "updated", "new"] as const) {
    for (const featured of [false, true]) {
      for (const category of [undefined, ...categories]) {
        keys.push(cacheKey(["plugins"], listParams({ terms: [], category, keyword: undefined, featured, sort, limit: DEFAULT_LIMIT, cursor: undefined })));
      }
    }
  }
  return keys;
};

const SORT_KEY: Record<Sort, { readonly expr: string; readonly asc: boolean }> = {
  relevance: { expr: "bm25(plugin_search, 10.0, 5.0, 3.0, 2.0, 1.0)", asc: true },
  popular: { expr: "p.installs", asc: false },
  updated: { expr: "p.updated_at", asc: false },
  new: { expr: "p.listed_at", asc: false }
};

const listPlugins = async (env: Env, db: D1Database, lq: ListQuery): Promise<Response> => {
  const filter = JSON.stringify(listParams({ ...lq, cursor: undefined, limit: DEFAULT_LIMIT }));
  let after: { key: number; seq: number; dir: "next" | "prev" } | undefined;
  if (lq.cursor !== undefined) {
    after = decodeCursor(lq.cursor, filter);
    if (!after) return fail(400, "bad_cursor", "That cursor isn't from this list.");
  }
  if (lq.q && lq.terms.length === 0) return answer({ items: [] }, 200, publicCache(LIST_TTL));
  // Each clause with its own binds, assembled in SQL order.
  const joins: { sql: string; binds: unknown[] }[] = [];
  const where: { sql: string; binds: unknown[] }[] = [{ sql: LISTED, binds: [] }];
  if (lq.terms.length) {
    joins.push({ sql: "JOIN plugin_search ON plugin_search.rowid = p.seq", binds: [] });
    where.push({ sql: "plugin_search MATCH ?", binds: [ftsQuery(lq.terms)] });
  }
  if (lq.category) joins.push({ sql: "JOIN plugin_categories c ON c.plugin_id = p.id AND c.category = ?", binds: [lq.category] });
  if (lq.keyword) joins.push({ sql: "JOIN plugin_keywords k ON k.plugin_id = p.id AND k.keyword = ?", binds: [lq.keyword] });
  if (lq.featured) where.push({ sql: "p.featured_at IS NOT NULL", binds: [] });
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
  const ordered = [...joins.flatMap((j) => j.binds), ...where.flatMap((w) => w.binds), ...keyset.binds];
  const rows = (await db.prepare(sql).bind(...ordered, lq.limit + 1).all<PluginRow & { sort_key: number }>()).results;
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
      items: page.map((p) => pluginCard(p, authorCard(authors[p.owner_id]))),
      ...(hasNext && last ? { next_cursor: encodeCursor(filter, last.sort_key, last.seq) } : {}),
      ...(hasPrev && first ? { prev_cursor: encodeCursor(filter, first.sort_key, first.seq, "prev") } : {})
    },
    200,
    publicCache(LIST_TTL)
  );
};

/** A listed plugin, or undefined (unknown, suspended or never approved). */
const listedPlugin = async (db: D1Database, id: string): Promise<PluginRow | undefined> => {
  const p = await pluginRow(db, id);
  return p && p.status === "active" && p.listed_version ? p : undefined;
};

const publicVersions = (db: D1Database, id: string, limit?: number): Promise<VersionRow[]> =>
  db
    .prepare(`SELECT * FROM versions WHERE plugin_id = ? AND status IN ('approved', 'yanked') ORDER BY ${VERSION_ORDER}${limit ? " LIMIT ?" : ""}`)
    .bind(...(limit ? [id, limit] : [id]))
    .all<VersionRow>()
    .then((r) => r.results);

const pluginDetail = async (env: Env, db: D1Database, id: string): Promise<Response> => {
  const p = await listedPlugin(db, id);
  if (!p) return notFound();
  const [listed, versions, authors] = await Promise.all([
    db.prepare("SELECT * FROM versions WHERE plugin_id = ? AND version = ?").bind(id, p.listed_version).first<VersionRow>(),
    publicVersions(db, id, DETAIL_VERSIONS),
    authorsOf(env, [p.owner_id])
  ]);
  if (!listed) return notFound();
  return answer(
    {
      ...pluginCard(p, authorCard(authors[p.owner_id])),
      readme: listed.readme,
      screenshots: parseList(listed.screenshots).map((s) => fileUrl(id, listed.version, s)),
      permissions: permissionsOf(listed),
      engines: listed.engines_zeron === null ? null : { zeron: listed.engines_zeron },
      homepage: listed.homepage,
      repository: listed.repository,
      license: listed.license,
      versions: versions.map((v) => versionInfo(v)),
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

const pluginVersions = async (db: D1Database, id: string): Promise<Response> => {
  if (!(await listedPlugin(db, id))) return notFound();
  return answer({ versions: (await publicVersions(db, id)).map((v) => versionInfo(v)) }, 200, publicCache(LIST_TTL));
};

export const handleCatalog = async (request: Request, env: Env, db: D1Database, url: URL, parts: readonly string[]): Promise<Response | undefined> => {
  if (request.method !== "GET" && request.method !== "HEAD") return undefined;
  // /store/v1/categories
  if (parts.length === 3 && parts[2] === "categories") {
    return throughCache(request, cacheKey(["categories"]), LIST_TTL, async () => answer({ categories: CATEGORIES }, 200, publicCache(LIST_TTL)));
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
    return throughCache(request, cacheKey(["plugins", id, "versions"]), LIST_TTL, () => pluginVersions(db, id));
  }
  return undefined;
};

export { PUBLIC_STATUSES };
