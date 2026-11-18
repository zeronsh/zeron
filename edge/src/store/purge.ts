/**
 * What a change to the catalog purges from this data center's cache (other
 * data centers see it within the list TTL, 60 s). A publish is public at
 * once (amendment 45), so every change that moves what the public sees
 * purges the same set: the plugin's detail, its versions' first page and
 * each changed version's own record, the first page of every list (each
 * sort, featured or not, verified or not, every category: a version can
 * move a plugin in or out of any of them), the store's totals and the
 * categories' counts; and each changed version's package and picture
 * states, so a take-down, a removal or a yank stops serving at once here.
 *
 * Which changes call it: publish (the new version and those it superseded),
 * verify, decline, take-down, restore, remove, yank, unyank, feature,
 * suspend, restore of a plugin, transfer and release.
 */
import { cacheKey, purge } from "./cache";
import { CATEGORIES_KEY, STATS_KEY, firstPageKeys, keywordsKey, takedownsKey, versionKey, versionsPageKey } from "./catalog";
import { CATEGORIES } from "./listing";
import { versionFileKeys } from "./serve";



/** Purge what a change to plugin `id`, and to `versions` of it, moves. */
export const purgeCatalog = async (db: D1Database, id: string, versions: readonly string[] = []): Promise<void> => {
  const files = await Promise.all(versions.map((v) => versionFileKeys(db, id, v)));
  await purge([
    cacheKey(["plugins", id]),
    versionsPageKey(id),
    ...versions.map((v) => versionKey(id, v)),
    ...firstPageKeys(CATEGORIES.map((c) => c.id)),
    STATS_KEY,
    CATEGORIES_KEY,
    keywordsKey(undefined),
    ...CATEGORIES.map((c) => keywordsKey(c.id)),
    ...files.flat()
  ]);
};

/** Purge the take-down feed after event `seq`: the answer every device that
 * was up to date held (`after` = the head before it), and a fresh start's.
 * Any other device's cached answer only delays the event, never skips it. */
export const purgeTakedowns = (seq: number): Promise<void> => purge([takedownsKey(seq - 1), takedownsKey(undefined)]);
