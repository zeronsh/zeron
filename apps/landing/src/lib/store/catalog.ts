// The store's address: what /plugins?… asks for (every plugin, a
// category's, a search's), read strictly (anything
// it can't use is dropped, never passed on), and the addresses of the
// pages around it. A page's address is the whole of its state, so a search
// can be shared, and Previous and Next are links.

import { KIND_PATH, type Kind } from "./kind.ts";
import type { Category, Sort } from "./types.ts";

export const PAGE_SIZE = 24;
const SORTS: readonly Sort[] = ["relevance", "popular", "updated", "new", "verified"];
export const SORT_LABELS: Record<Sort, string> = {
  relevance: "Best match",
  popular: "Most installed",
  updated: "Recently updated",
  new: "Newest",
  verified: "Recently verified",
};

export interface CatalogState {
  /** Which of the store's lists: plugins (/plugins) or templates (/templates). */
  kind: Kind;
  q: string;
  category: string | null;
  keyword: string | null;
  /** Only plugins whose newest version is verified (amendment 45: a filter, never a sort). */
  verified: boolean;
  /** As asked; null is the default for the search (relevance with words, else popular). */
  sort: Sort | null;
  cursor: string | null;
  /** Which page a cursor's is (2 on), for "26–50 of 128": the cursor says where, not how far. Only with a cursor. */
  page: number | null;
}

/** A cursor as the edge makes them: opaque, url-safe, bounded. */
const CURSOR = /^[A-Za-z0-9_-]{1,512}$/;

/** The sort a state means: what it asks, else the default for its search. */
export function effectiveSort(state: Pick<CatalogState, "q" | "sort">): Sort {
  if (state.sort && (state.sort !== "relevance" || state.q)) return state.sort;
  return state.q ? "relevance" : "popular";
}

/** Reads the catalog's state from its address. `categories` is the edge's list. (A template has none.) */
export function readCatalog(params: URLSearchParams, categories: readonly Category[] | null, kind: Kind = "plugin"): CatalogState {
  const q = clean(params.get("q") ?? "").slice(0, 100);
  const category = params.get("category");
  // (A keyword is 1 to 32 characters: amendment 22.)
  const keyword = clean(params.get("keyword") ?? "").toLowerCase().slice(0, 32);
  const sort = params.get("sort");
  const cursor = params.get("cursor");
  const page = Number(params.get("page"));
  return {
    kind,
    q,
    // (An unknown category is no filter at all, rather than an empty page.)
    category: category && (categories === null || categories.some((c) => c.id === category)) && /^[a-z0-9-]{1,40}$/.test(category) ? category : null,
    keyword: keyword || null,
    verified: params.get("verified") === "1",
    sort: sort && (SORTS as readonly string[]).includes(sort) ? (sort as Sort) : null,
    cursor: cursor && CURSOR.test(cursor) ? cursor : null,
    page: cursor && CURSOR.test(cursor) && Number.isInteger(page) && page >= 2 && page <= 10_000 ? page : null,
  };
}

/** Whether the state's address says anything it shouldn't (it's then sent to the clean one). */
export function canonicalSearch(params: URLSearchParams, state: CatalogState): string | null {
  const clean = catalogHref(state);
  const asked = `${KIND_PATH[state.kind]}${params.size ? `?${params}` : ""}`;
  return clean === asked ? null : clean;
}

/** The address of a catalog state. Defaults are left out, in a fixed order. */
export function catalogHref(state: Partial<CatalogState>): string {
  const params = new URLSearchParams();
  if (state.q) params.set("q", state.q);
  if (state.category) params.set("category", state.category);
  if (state.keyword) params.set("keyword", state.keyword);
  if (state.verified) params.set("verified", "1");
  const sort = state.sort ?? null;
  if (sort && sort !== effectiveSort({ q: state.q ?? "", sort: null })) params.set("sort", sort);
  if (state.cursor) params.set("cursor", state.cursor);
  if (state.cursor && state.page) params.set("page", String(state.page));
  return `${KIND_PATH[state.kind ?? "plugin"]}${params.size ? `?${params}` : ""}`;
}

/** The same search on another page: `step` 1 the next, −1 the one before (the first: no cursor). */
export function pageHref(state: CatalogState, cursor: string | null | undefined, step: 1 | -1 = 1): string | null {
  if (cursor === undefined) return null;
  const now = state.page ?? (state.cursor ? null : 1);
  const page = now === null ? null : now + step;
  return catalogHref({ ...state, cursor: page === 1 ? null : cursor, page: page === 1 ? null : page });
}

/** "26–50" of a page's items, when which page it is is known. */
export function range(state: CatalogState, shown: number): { from: number; to: number } | null {
  const page = state.page ?? (state.cursor ? null : 1);
  if (page === null || shown === 0) return null;
  const from = (page - 1) * PAGE_SIZE + 1;
  return { from, to: from + shown - 1 };
}

/** Whether the store is unfiltered on its first page (no search, category, keyword or Verified only), in any order: Featured shows above it. */
export const isFront = (state: CatalogState) => !state.q && !state.category && !state.keyword && !state.verified && !state.cursor;

/** Words as typed, with runs of space made one and control characters gone. */
function clean(value: string): string {
  // eslint-disable-next-line no-control-regex
  return value.replace(/[\u0000-\u001f\u007f]/g, " ").replace(/\s+/g, " ").trim();
}
