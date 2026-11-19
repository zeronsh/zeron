/**
 * The catalog's shapes (contract "Shapes", amendments 1, 11 and 20): a
 * `PluginCard` for lists, `VersionInfo` for versions. Who wrote a plugin is
 * read live from the user directory, never from the manifest: its current
 * username and its badges (granted by operators). There is no separate
 * "verified": publishing needs a username, so every listing is by a known
 * account.
 */
import type { Env } from "../env";
import { userDirectoryStub } from "../user-directory";
import type { StoreAuthor } from "../user-directory";
import { type PluginRow, type VersionRow, parseList } from "./db";

export interface Author {
  /** `publisherId`: stable and opaque (amendment 33). */
  readonly id: string;
  readonly username: string | null;
  readonly badges: readonly { readonly id: string; readonly name: string; readonly icon: string | null }[];
}

/**
 * An account's publisher id (amendment 33): `pub_` and the first 32 hex
 * characters of sha256("zeron-publisher:v1:" + its user id). It never
 * changes for an account, says nothing of its user id, and is what a device
 * binds a store install's consent to, so a username an operator moves never
 * carries another account's decisions.
 */
export const publisherId = async (userId: string): Promise<string> => {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`zeron-publisher:v1:${userId}`)));
  return `pub_${[...digest].map((b) => b.toString(16).padStart(2, "0")).join("").slice(0, 32)}`;
};

/** Each of `userIds`' publisher id, by user id. */
export const publisherIds = async (userIds: readonly string[]): Promise<Map<string, string>> => {
  const unique = [...new Set(userIds)];
  return new Map(await Promise.all(unique.map(async (u) => [u, await publisherId(u)] as const)));
};

/** An ISO time (the user directory keeps WorkOS's) as ms since the epoch,
 * as every time in a store answer is (amendment 35); null if it isn't one. */
export const epochMs = (iso: string | null | undefined): number | null => {
  const ms = iso ? Date.parse(iso) : Number.NaN;
  return Number.isFinite(ms) ? ms : null;
};

/** What the store shows of each of `userIds`: one entry for every one, a
 * deleted account's without a username. `createdAt`: when the account was
 * made, ms since the epoch. */
export const authorsOf = async (env: Env, userIds: readonly string[]): Promise<Record<string, Author & { readonly createdAt: number | null }>> => {
  const unique = [...new Set(userIds)];
  const directory = userDirectoryStub(env);
  const found: Record<string, StoreAuthor> = directory && unique.length ? await directory.storeAuthors(unique) : {};
  const ids = await Promise.all(unique.map(publisherId));
  return Object.fromEntries(
    unique.map((userId, i) => [
      userId,
      { id: ids[i]!, username: found[userId]?.username ?? null, badges: found[userId]?.badges ?? [], createdAt: epochMs(found[userId]?.createdAt) }
    ])
  );
};

/** The card's author: never the user id. */
export const authorCard = (found: Author): Author => ({ id: found.id, username: found.username, badges: found.badges });

export const packageUrl = (id: string, version: string): string =>
  `/store/v1/plugins/${id}/versions/${encodeURIComponent(version)}/package`;

/**
 * A plugin's card. `icon` (like a detail's `screenshots`) is a path in the
 * package of the card's `version` (amendment 35), fetched through the files
 * route with that version; never a URL. No version, no icon.
 */
export const pluginCard = (p: PluginRow, author: Author) => ({
  id: p.id,
  name: p.name,
  summary: p.summary,
  icon: p.listed_version ? p.icon : null,
  author,
  categories: parseList(p.categories),
  keywords: parseList(p.keywords),
  version: p.listed_version,
  // Its `version` is verified, and the newest verified listed version (amendment 45).
  verified: p.listed_version !== null && p.verified === 1,
  verified_version: p.verified_version,
  // When that version was verified (the "Newly verified" shelf's order).
  verified_at: p.verified_version !== null ? p.verified_at : null,
  updated_at: p.updated_at,
  // First listed, what "new" sorts by (amendment 39); null only on an owner's
  // or admin's card of a plugin never listed.
  listed_at: p.listed_version ? p.listed_at : null,
  installs: p.installs,
  featured: p.featured_at !== null
});

const parseJson = <T>(text: string | null, fallback: T): T => {
  if (!text) return fallback;
  try {
    return JSON.parse(text) as T;
  } catch {
    return fallback;
  }
};

/**
 * A reviewer's decision on version `v`, as its owner and staff read it
 * (amendment 45): verified, declined (still unverified, with the reason and
 * the words for its author), or not decided yet. The public sees only
 * `verified`.
 */
export const reviewOf = (v: VersionRow) =>
  v.verification === "verified"
    ? { decided: true, decision: "verified" as const, decided_at: v.verified_at ?? undefined }
    : v.declined_at !== null
      ? { decided: true, decision: "declined" as const, reason: v.decline_reason ?? undefined, message: v.decline_message ?? undefined, decided_at: v.declined_at }
      : { decided: false, decision: null };

/** A version as the public sees it (the contract's `VersionInfo`); `full`
 * adds what its owner and staff read: the review, the checks, the deploy
 * key that published it, and why staff took it down or removed it. */
export const versionInfo = (v: VersionRow, full = false) => ({
  version: v.version,
  status: v.status,
  published_at: v.published_at,
  notes: v.notes,
  engines: v.engines_zeron === null ? null : { zeron: v.engines_zeron },
  yanked: v.status === "yanked",
  // Why it takes no new installs (public): its author's yank, or a newer
  // version listed before anyone verified it, named.
  yanked_by: v.status === "yanked" ? v.yanked_by : null,
  ...(v.status === "yanked" && v.yanked_by === "superseded" ? { superseded_by: v.superseded_by } : {}),
  prerelease: v.is_release === 0,
  key: v.signer_key,
  hash: v.content_hash,
  sha256: v.package_sha256,
  bytes: v.package_bytes,
  verified: v.verification === "verified",
  verified_at: v.verification === "verified" ? v.verified_at : null,
  via: v.via,
  ...(full
    ? {
        review: reviewOf(v),
        checks: parseJson(v.checks, []),
        // Which of the owner's deploy tokens published it, by its name and
        // short id then (never more of it)
        // (a notice on the author's own devices; amendment 44).
        deploy_token: v.via === "deploy_token" ? { short: v.deploy_token_short, name: v.deploy_token_name } : null,
        ...(v.status === "taken_down" || v.status === "removed" ? { removal: { at: v.removed_at, reason: v.removal_reason } } : {})
      }
    : {})
});

/**
 * The card an owner or an admin sees of plugin `p`: its listed card, or,
 * never listed, `shown`'s name, summary, categories and keywords (a
 * submission's), with no version and so no icon.
 */
export const ownerCard = (p: PluginRow, shown: VersionRow | undefined, author: Author) =>
  pluginCard(
    p.listed_version || !shown ? p : { ...p, name: shown.name, summary: shown.summary, icon: null, categories: shown.categories, keywords: shown.keywords },
    author
  );

/** `/me/plugins`' and the admin view's word for where plugin `p` stands. */
export const listingState = (p: PluginRow): "listed" | "unlisted" | "suspended" =>
  p.status === "suspended" ? "suspended" : p.listed_version ? "listed" : "unlisted";

export const permissionsOf = (v: VersionRow): unknown => parseJson(v.permissions, null);
export { parseJson as parseStoredJson };
