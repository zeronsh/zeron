/**
 * Zeron's own template files, as each release had them (contract
 * "Templates: as the edge built them"): `stock/<zeron_version>.json` =
 * `{version, files: {rel: [sha256, …]}}` in PLUGIN_STORE, written per release by
 * the generator, never by an author: the file as Zeron's default look has it,
 * and as each of its official templates does. A match with any is the same.
 * Each version of a template under review is read against the ones of the
 * Zeron it says it was made from, so staff read what its author wrote rather
 * than all of Zeron. A Zeron whose files aren't there (or can't be read) is
 * unknown, and then nothing is marked: every file is shown.
 */
import { fold } from "./paths";
import { parseVersion } from "./semver";

export const STOCK_KEY = { prefix: "stock/", suffix: ".json" } as const;
export const stockKey = (zeronVersion: string): string => `${STOCK_KEY.prefix}${zeronVersion}${STOCK_KEY.suffix}`;

/** More than any release's: one past it isn't read. */
const MAX_STOCK_BYTES = 4 * 1024 * 1024;
const SHA256_RE = /^[0-9a-f]{64}$/;

export type StockMark = "same" | "changed" | "new";
/** The mark of a file by its path and sha256. */
export type Stock = (path: string, sha256: string) => StockMark;

/** Each file's hashes, by its path. */
type StockFiles = ReadonlyMap<string, ReadonlySet<string>>;

const isHashes = (value: unknown): value is string[] => Array.isArray(value) && value.length > 0 && value.every((hash) => typeof hash === "string" && SHA256_RE.test(hash));

/** `raw` as `zeronVersion`'s files, or undefined when it isn't. */
const readStock = (raw: unknown, zeronVersion: string): StockFiles | undefined => {
  const doc = raw as { version?: unknown; files?: unknown } | null;
  if (!doc || typeof doc !== "object" || doc.version !== zeronVersion) return undefined;
  if (!doc.files || typeof doc.files !== "object" || Array.isArray(doc.files)) return undefined;
  const files = Object.entries(doc.files as Record<string, unknown>);
  const known = new Map<string, ReadonlySet<string>>();
  for (const [path, hashes] of files) {
    if (!isHashes(hashes)) return undefined;
    known.set(path, new Set(hashes));
  }
  return known.size > 0 ? known : undefined;
};

/** Releases' files as this isolate last read them: a release's are written
 * once, so they're kept a while; one not known isn't kept at all, so a file
 * is read as soon as it's there. The oldest goes first. */
const CACHE_MS = 10 * 60_000;
const CACHE_MAX = 8;
const cache = new Map<string, { readonly stock: Stock; readonly until: number }>();

/** The files of Zeron `zeronVersion`, or undefined when they aren't known. */
export const stockOf = async (bucket: R2Bucket, zeronVersion: string, now = Date.now()): Promise<Stock | undefined> => {
  // (A version the store takes, so at most MAX_VERSION_LENGTH, before it keys
  // anything.)
  if (!parseVersion(zeronVersion)) return undefined;
  const kept = cache.get(zeronVersion);
  if (kept && now < kept.until) return kept.stock;
  cache.delete(zeronVersion);
  const stock = await readStockOf(bucket, zeronVersion);
  if (!stock) return undefined;
  if (cache.size >= CACHE_MAX) cache.delete(cache.keys().next().value!);
  cache.set(zeronVersion, { stock, until: now + CACHE_MS });
  return stock;
};

const readStockOf = async (bucket: R2Bucket, zeronVersion: string): Promise<Stock | undefined> => {
  let files: StockFiles | undefined;
  try {
    const object = await bucket.get(stockKey(zeronVersion));
    if (!object) return undefined;
    if (object.size > MAX_STOCK_BYTES) await object.body.cancel();
    else files = readStock(await object.json(), zeronVersion);
  } catch (e) {
    console.error("store: couldn't read Zeron's own files", zeronVersion, String(e));
    return undefined;
  }
  if (!files) {
    console.error("store: Zeron's own files aren't as the generator writes them", zeronVersion);
    return undefined;
  }
  const known = files;
  // (One of Zeron's spelt another way replaces it on a disk folding names.)
  const folded = new Set([...known.keys()].map(fold));
  return (path, sha256) => {
    const own = known.get(path);
    if (own !== undefined) return own.has(sha256) ? "same" : "changed";
    return folded.has(fold(path)) ? "changed" : "new";
  };
};
