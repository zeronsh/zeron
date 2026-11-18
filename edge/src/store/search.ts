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
export const encodeCursor = (filter: string, key: number, seq: number): string => b64url(JSON.stringify({ f: filter, k: key, i: seq }));
export const decodeCursor = (cursor: string, filter: string): { key: number; seq: number } | undefined => {
  if (cursor.length > 512) return undefined;
  const text = unb64url(cursor);
  if (text === undefined) return undefined;
  try {
    const value = JSON.parse(text) as { f?: unknown; k?: unknown; i?: unknown };
    if (value.f !== filter || typeof value.k !== "number" || !Number.isFinite(value.k) || !Number.isSafeInteger(value.i)) return undefined;
    return { key: value.k, seq: value.i as number };
  } catch {
    return undefined;
  }
};

