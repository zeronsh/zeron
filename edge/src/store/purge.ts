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
import { purge } from "./cache";
import { CATEGORIES_KEY, firstPageKeys, itemKey, keywordsKey, statsKey, takedownsKey, versionKey, versionsPageKey } from "./catalog";
import { KINDS, type Kind } from "./kind";
import { CATEGORIES } from "./listing";
import { versionFileKeys } from "./serve";


/** What a change to an item of `kind` moves for the whole store: the first
 * pages of its kind's lists, its totals, and its keywords; and, a plugin's,
 * the categories (a template is in none). */
const sharedKeys = (kind: Kind): string[] => {
  const categories = kind === "plugin" ? CATEGORIES.map((c) => c.id) : [];
  return [
    ...firstPageKeys(kind, categories),
    statsKey(kind),
    ...(kind === "plugin" ? [CATEGORIES_KEY] : []),
    keywordsKey(kind, undefined),
    ...categories.map((c) => keywordsKey(kind, c))
  ];
};

/** Purge what a change to item `id`, and to `versions` of it, moves. The
 * store-wide keys are its kind's; its own are under both prefixes, since an
 * id names one item, and a request under the other prefix may have cached
 * its 404 (a released id can be claimed as the other kind). */
export const purgeCatalog = async (db: D1Database, id: string, versions: readonly string[] = []): Promise<void> => {
  const files = await Promise.all(versions.map((v) => versionFileKeys(db, id, v)));
  const row = await db.prepare("SELECT kind FROM plugins WHERE id = ?").bind(id).first<{ kind: Kind }>();
  await purge([
    ...KINDS.flatMap((kind) => [itemKey(kind, id), versionsPageKey(kind, id), ...versions.map((v) => versionKey(kind, id, v))]),
    ...(row ? sharedKeys(row.kind) : KINDS.flatMap(sharedKeys)),
    ...files.flat()
  ]);
};

/** Purge the take-down feed after event `seq`: the answer every device that
 * was up to date held (`after` = the head before it), and a fresh start's.
 * Any other device's cached answer only delays the event, never skips it. */
export const purgeTakedowns = (seq: number): Promise<void> => purge([takedownsKey(seq - 1), takedownsKey(undefined)]);
