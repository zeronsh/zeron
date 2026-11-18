// An admin list's pages (the queue, reports, the audit log): the edge's
// cursors both ways, and which page a cursor's is, carried beside it, so
// the pager can say where you are ("51–100 of 128"): the cursor says
// where, not how far. Each list asks for ADMIN_PAGE at a time.
import { count } from "./format.ts";

/** The edge's own most for an admin list (and its default). */
export const ADMIN_PAGE = 50;

const CURSOR = /^[A-Za-z0-9_-]{1,512}$/;

export interface Place {
  cursor: string | undefined;
  /** Which page (1 with no cursor); null for a cursor that came without its page. */
  page: number | null;
}

export function placeOf(url: URL): Place {
  const asked = url.searchParams.get("cursor");
  if (!asked || !CURSOR.test(asked)) return { cursor: undefined, page: 1 };
  const page = Number(url.searchParams.get("page"));
  return { cursor: asked, page: Number.isInteger(page) && page >= 2 && page <= 10_000 ? page : null };
}

export interface Pager {
  previous: string | null;
  next: string | null;
  /** "51–100 of 128", or "of 128" when which page this is isn't known. */
  where: string;
}

/**
 * Previous and Next for a list at `path` (its own query in `keep`), and
 * where this page is in it. Previous from the second page is the list's
 * own address, with no cursor.
 */
export function pagerOf(path: string, keep: Record<string, string>, place: Place, page: { next_cursor?: string; prev_cursor?: string; total: number }, shown: number): Pager {
  const href = (cursor?: string, n?: number | null) => {
    const params = new URLSearchParams(keep);
    if (cursor) params.set("cursor", cursor);
    if (cursor && n && n >= 2) params.set("page", String(n));
    return `${path}${params.size ? `?${params}` : ""}`;
  };
  const back = place.page === 2 || (place.cursor && !page.prev_cursor) ? href() : page.prev_cursor ? href(page.prev_cursor, place.page && place.page - 1) : null;
  const from = place.page ? (place.page - 1) * ADMIN_PAGE + 1 : null;
  return {
    previous: place.cursor ? back : null,
    next: page.next_cursor ? href(page.next_cursor, place.page && place.page + 1) : null,
    where: from && shown ? `${count(from)}–${count(from + shown - 1)} of ${count(page.total)}` : `of ${count(page.total)}`,
  };
}
