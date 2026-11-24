/**
 * Search terms and keyset cursors for the catalog (catalog.ts): pure, so
 * they're unit tested in Node.
 */

const MAX_TERMS = 8;

/** The words of a search, as FTS5 prefix terms: letters and digits only,
 * each quoted, so the query can never be FTS syntax. */
export const searchTerms = (q: string): string[] =>
  (q.normalize("NFKC").toLowerCase().match(/[\p{L}\p{N}]+/gu) ?? []).slice(0, MAX_TERMS).map((t) => t.slice(0, 64));

export const ftsQuery = (terms: readonly string[]): string => terms.map((t) => `"${t.replaceAll('"', '""')}"*`).join(" ");

const b64url = (text: string): string => btoa(String.fromCharCode(...new TextEncoder().encode(text))).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
const unb64url = (text: string): string | undefined => {
  try {
    const bin = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
    return new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(Uint8Array.from(bin, (c) => c.charCodeAt(0)));
  } catch {
    return undefined;
  }
};

/** A keyset cursor: the query it pages (`f`), and the last item's sort
 * key (`k`) and seq (`i`). */
/** Which way a cursor pages: past its item (`next`) or before it (`prev`). */
export type Direction = "next" | "prev";

export const encodeCursor = (filter: string, key: number, seq: number, dir: Direction = "next"): string =>
  b64url(JSON.stringify({ f: filter, k: key, i: seq, ...(dir === "prev" ? { d: "prev" } : {}) }));

export const decodeCursor = (cursor: string, filter: string): { key: number; seq: number; dir: Direction } | undefined => {
  if (cursor.length > 512) return undefined;
  const text = unb64url(cursor);
  if (text === undefined) return undefined;
  try {
    const value = JSON.parse(text) as { f?: unknown; k?: unknown; i?: unknown; d?: unknown };
    if (value.f !== filter || typeof value.k !== "number" || !Number.isFinite(value.k) || !Number.isSafeInteger(value.i)) return undefined;
    if (value.d !== undefined && value.d !== "prev") return undefined;
    return { key: value.k, seq: value.i as number, dir: value.d === "prev" ? "prev" : "next" };
  } catch {
    return undefined;
  }
};

/** A version's place in its plugin's list, newest first by precedence:
 * `[major, minor, patch, is_release, pre_key]`. */
export type VersionPlace = readonly [number, number, number, number, string];

/** A cursor into plugin `id`'s versions, past (or, `prev`, before) `place`. */
export const encodeVersionCursor = (id: string, place: VersionPlace, dir: Direction = "next"): string =>
  b64url(JSON.stringify({ f: `versions:${id}`, v: place, ...(dir === "prev" ? { d: "prev" } : {}) }));

export const decodeVersionCursor = (cursor: string, id: string): { place: VersionPlace; dir: Direction } | undefined => {
  if (cursor.length > 512) return undefined;
  const text = unb64url(cursor);
  if (text === undefined) return undefined;
  try {
    const value = JSON.parse(text) as { f?: unknown; v?: unknown; d?: unknown };
    const v = value.v;
    if (value.f !== `versions:${id}` || !Array.isArray(v) || v.length !== 5) return undefined;
    if (!v.slice(0, 4).every((n) => Number.isSafeInteger(n)) || typeof v[4] !== "string" || v[4].length > 1024) return undefined;
    if (value.d !== undefined && value.d !== "prev") return undefined;
    return { place: v as unknown as VersionPlace, dir: value.d === "prev" ? "prev" : "next" };
  } catch {
    return undefined;
  }
};
