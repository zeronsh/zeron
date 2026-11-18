// The list's address: what /plugins/all?… asks for (every plugin, a
// category's, a search's), read strictly (anything
// it can't use is dropped, never passed on), and the addresses of the
// pages around it. A page's address is the whole of its state, so a search
// can be shared, and Previous and Next are links.

import type { Category, Sort } from "./types.ts";

export const PAGE_SIZE = 24;
/** Where the list of every plugin is (/plugins itself is Discover). */
export const LIST = "/plugins/all";
const SORTS: readonly Sort[] = ["relevance", "popular", "updated", "new"];
export const SORT_LABELS: Record<Sort, string> = {
  relevance: "Best match",
  popular: "Most installed",
  updated: "Recently updated",
  new: "Newest",
};

export interface CatalogState {
  q: string;
  category: string | null;
  keyword: string | null;
  /** As asked; null is the default for the search (relevance with words, else popular). */
  sort: Sort | null;
  cursor: string | null;
}

/** A cursor as the edge makes them: opaque, url-safe, bounded. */
const CURSOR = /^[A-Za-z0-9_-]{1,512}$/;

/** The sort a state means: what it asks, else the default for its search. */
export function effectiveSort(state: Pick<CatalogState, "q" | "sort">): Sort {
  if (state.sort && (state.sort !== "relevance" || state.q)) return state.sort;
  return state.q ? "relevance" : "popular";
}

/** Reads the catalog's state from its address. `categories` is the edge's list. */
export function readCatalog(params: URLSearchParams, categories: readonly Category[] | null): CatalogState {
  const q = clean(params.get("q") ?? "").slice(0, 100);
  const category = params.get("category");
  // (A keyword is 1 to 32 characters: amendment 22.)
  const keyword = clean(params.get("keyword") ?? "").toLowerCase().slice(0, 32);
  const sort = params.get("sort");
  const cursor = params.get("cursor");
  return {
    q,
    // (An unknown category is no filter at all, rather than an empty page.)
    category: category && (categories === null || categories.some((c) => c.id === category)) && /^[a-z0-9-]{1,40}$/.test(category) ? category : null,
    keyword: keyword || null,
    sort: sort && (SORTS as readonly string[]).includes(sort) ? (sort as Sort) : null,
    cursor: cursor && CURSOR.test(cursor) ? cursor : null,
  };
}

/** Whether the state's address says anything it shouldn't (it's then sent to the clean one). */
export function canonicalSearch(params: URLSearchParams, state: CatalogState): string | null {
  const clean = catalogHref(state);
  const asked = `${LIST}${params.size ? `?${params}` : ""}`;
  return clean === asked ? null : clean;
}

/** The address of a catalog state. Defaults are left out, in a fixed order. */
export function catalogHref(state: Partial<CatalogState>): string {
  const params = new URLSearchParams();
  if (state.q) params.set("q", state.q);
  if (state.category) params.set("category", state.category);
  if (state.keyword) params.set("keyword", state.keyword);
  const sort = state.sort ?? null;
  if (sort && sort !== effectiveSort({ q: state.q ?? "", sort: null })) params.set("sort", sort);
  if (state.cursor) params.set("cursor", state.cursor);
  return `${LIST}${params.size ? `?${params}` : ""}`;
}

/** The same search on another page (or its first: `cursor` null). */
export function pageHref(state: CatalogState, cursor: string | null | undefined): string | null {
  if (cursor === undefined) return null;
  return catalogHref({ ...state, cursor });
}

/** Whether this is the store's front: no search, no filter, its first page. */
export const isFront = (state: CatalogState) => !state.q && !state.category && !state.keyword && !state.cursor && effectiveSort(state) === "popular";

/** Words as typed, with runs of space made one and control characters gone. */
function clean(value: string): string {
  // eslint-disable-next-line no-control-regex
  return value.replace(/[\u0000-\u001f\u007f]/g, " ").replace(/\s+/g, " ").trim();
}
