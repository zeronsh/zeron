/**
 * The catalog's shapes (contract "Shapes", amendments 1 and 11): a
 * `PluginCard` for lists, `VersionInfo` for versions. Who wrote a plugin
 * is read live from the user directory, never from the manifest: its
 * current username, and whether it holds the operator badge `verified`.
 */
import type { Env } from "../env";
import { userDirectoryStub } from "../user-directory";
import type { StoreAuthor } from "../user-directory";
import { type PluginRow, type VersionRow, parseList } from "./db";

/** The operator badge (`/admin/badges`) that reads as a verified publisher. */
export const VERIFIED_BADGE = "verified";

export interface Author {
  readonly username: string | null;
  readonly verified: boolean;
  readonly badges: readonly { readonly id: string; readonly name: string; readonly icon: string | null }[];
}

export const authorsOf = async (env: Env, userIds: readonly string[]): Promise<Record<string, StoreAuthor>> => {
  const directory = userDirectoryStub(env);
  if (!directory || userIds.length === 0) return {};
  return directory.storeAuthors([...new Set(userIds)]);
};

export const authorCard = (found: StoreAuthor | undefined): Author => ({
  username: found?.username ?? null,
  verified: found?.username != null && found.badges.some((b) => b.id === VERIFIED_BADGE),
  badges: found?.badges ?? []
});

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
