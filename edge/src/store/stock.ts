/**
 * Zeron's own template files, as each release had them (contract
 * "Templates: as the edge built them"): `stock/<zeron_version>.json` =
 * `{version, files: {rel: sha256}}` in PLUGIN_STORE, written per release by
 * the generator, never by an author. A template under review is read against
 * the ones of the Zeron it says it was made from, so staff read what its
 * author wrote rather than all of Zeron. A Zeron whose files aren't there (or
 * can't be read) is unknown, and then nothing is marked: every file is shown.
 */
import { fold } from "./paths";
import { parseVersion } from "./semver";

export const STOCK_KEY = { prefix: "stock/", suffix: ".json" } as const;
export const stockKey = (zeronVersion: string): string => `${STOCK_KEY.prefix}${zeronVersion}${STOCK_KEY.suffix}`;

/** More than any release's: one past it isn't read. */
const MAX_STOCK_BYTES = 4 * 1024 * 1024;
const MAX_VERSION_LEN = 128;
const SHA256_RE = /^[0-9a-f]{64}$/;

export type StockMark = "same" | "changed" | "new";
/** The mark of a file by its path and sha256. */
export type Stock = (path: string, sha256: string) => StockMark;

/** `raw` as `zeronVersion`'s files, or undefined when it isn't. */
const readStock = (raw: unknown, zeronVersion: string): ReadonlyMap<string, string> | undefined => {
  const doc = raw as { version?: unknown; files?: unknown } | null;
  if (!doc || typeof doc !== "object" || doc.version !== zeronVersion) return undefined;
  if (!doc.files || typeof doc.files !== "object" || Array.isArray(doc.files)) return undefined;
  const files = Object.entries(doc.files as Record<string, unknown>);
  return files.length > 0 && files.every(([, hash]) => typeof hash === "string" && SHA256_RE.test(hash)) ? new Map(files as [string, string][]) : undefined;
};

/** The files of Zeron `zeronVersion`, or undefined when they aren't known. */
export const stockOf = async (bucket: R2Bucket, zeronVersion: string): Promise<Stock | undefined> => {
  if (zeronVersion.length > MAX_VERSION_LEN || !parseVersion(zeronVersion)) return undefined;
  let files: ReadonlyMap<string, string> | undefined;
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
    if (own !== undefined) return own === sha256 ? "same" : "changed";
    return folded.has(fold(path)) ? "changed" : "new";
  };
};
