/**
 * What `POST /store/v1/updates` is asked (updates.ts): the Zeron asking and
 * the items it has, read as a device writes them (docs/plugin-package-
 * vectors.json `update_ask`).
 */
import { LIMITS } from "./data";
import type { Kind } from "./kind";
import { type Version, parseVersion } from "./semver";

/** Both lists together. */
export const MAX_UPDATE_ITEMS = LIMITS.max_update_items;
const ID_RE = /^[a-z0-9][a-z0-9-]{1,39}$/;
const HASH_RE = /^[0-9a-f]{64}$/;

export interface Installed {
  readonly kind: Kind;
  readonly id: string;
  readonly version: string;
  readonly hash: string;
  readonly prerelease: boolean;
  /** Offer only verified versions of it. */
  readonly verifiedOnly: boolean;
}

/** One list of the request, as `kind`, or undefined when it isn't one. */
export const parseList = (kind: Kind, value: unknown): Installed[] | undefined => {
  if (!Array.isArray(value)) return undefined;
  const items: Installed[] = [];
  const seen = new Set<string>();
  for (const item of value as unknown[]) {
    const p = item as Record<string, unknown> | null;
    if (
      !p ||
      typeof p.id !== "string" ||
      !ID_RE.test(p.id) ||
      typeof p.version !== "string" ||
      p.version.length > 128 ||
      typeof p.hash !== "string" ||
      !HASH_RE.test(p.hash) ||
      (p.prerelease !== undefined && typeof p.prerelease !== "boolean") ||
      (p.verified_only !== undefined && typeof p.verified_only !== "boolean") ||
      seen.has(p.id)
    ) {
      return undefined;
    }
    seen.add(p.id);
    items.push({ kind, id: p.id, version: p.version, hash: p.hash, prerelease: p.prerelease === true, verifiedOnly: p.verified_only === true });
  }
  return items;
};

export const parseRequest = (body: Record<string, unknown> | undefined): { zeron: Version; items: Installed[] } | string => {
  if (!body) return "The body is { zeron, plugins: [{ id, version, hash, prerelease? }], templates? }.";
  const zeron = typeof body.zeron === "string" ? parseVersion(body.zeron) : undefined;
  if (!zeron) return "zeron is this device's Zeron version (1.2.3).";
  if (!Array.isArray(body.plugins) || (body.templates !== undefined && !Array.isArray(body.templates)) || body.plugins.length + (body.templates?.length ?? 0) > MAX_UPDATE_ITEMS) {
    return `plugins and templates are lists of at most ${MAX_UPDATE_ITEMS} together.`;
  }
  const plugins = parseList("plugin", body.plugins);
  const templates = parseList("template", body.templates ?? []);
  if (!plugins || !templates) return "Each item is { id, version, hash, prerelease?, verified_only? }, each id once in its list.";
  return { zeron, items: [...plugins, ...templates] };
};
