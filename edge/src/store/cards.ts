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

/** What the store shows of each of `userIds`: one entry for every one, a
 * deleted account's without a username. */
export const authorsOf = async (env: Env, userIds: readonly string[]): Promise<Record<string, Author & { readonly createdAt: string | null }>> => {
  const unique = [...new Set(userIds)];
  const directory = userDirectoryStub(env);
  const found: Record<string, StoreAuthor> = directory && unique.length ? await directory.storeAuthors(unique) : {};
  const ids = await Promise.all(unique.map(publisherId));
  return Object.fromEntries(
    unique.map((userId, i) => [
      userId,
      { id: ids[i]!, username: found[userId]?.username ?? null, badges: found[userId]?.badges ?? [], createdAt: found[userId]?.createdAt ?? null }
    ])
  );
};

/** The card's author: never the user id. */
export const authorCard = (found: Author): Author => ({ id: found.id, username: found.username, badges: found.badges });

/** Where a packaged file is served (relative to the edge). */
export const fileUrl = (id: string, version: string, path: string): string =>
  `/store/v1/plugins/${id}/files/${encodeURIComponent(version)}/${path.split("/").map(encodeURIComponent).join("/")}`;

export const packageUrl = (id: string, version: string): string =>
  `/store/v1/plugins/${id}/versions/${encodeURIComponent(version)}/package`;

export const pluginCard = (p: PluginRow, author: Author) => ({
  id: p.id,
  name: p.name,
  summary: p.summary,
  icon: p.listed_version && p.icon ? fileUrl(p.id, p.listed_version, p.icon) : null,
  author,
  categories: parseList(p.categories),
  keywords: parseList(p.keywords),
  version: p.listed_version,
  updated_at: p.updated_at,
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

/** A version as the public sees it; `full` adds the owner's and admins'
 * review state and checks. */
export const versionInfo = (v: VersionRow, full = false) => ({
  version: v.version,
  status: v.status,
  published_at: v.status === "approved" || v.status === "yanked" || v.status === "removed" ? v.decided_at : null,
  submitted_at: v.submitted_at,
  notes: v.notes,
  engines: v.engines_zeron === null ? null : { zeron: v.engines_zeron },
  yanked: v.status === "yanked",
  prerelease: v.is_release === 0,
  key: v.signer_key,
  hash: v.content_hash,
  sha256: v.package_sha256,
  bytes: v.package_bytes,
  ...(full
    ? {
        review: { state: v.status, reason: v.reason ?? undefined, message: v.message ?? undefined, decided_at: v.decided_at ?? undefined },
        checks: parseJson(v.checks, []),
        blocked: v.blocked === 1,
        via: v.via
      }
    : {})
});

export const permissionsOf = (v: VersionRow): unknown => parseJson(v.permissions, null);
export { parseJson as parseStoredJson };
