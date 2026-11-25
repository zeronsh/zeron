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

/** Whether a member of an item is left out, or a boolean. */
const isFlag = (value: unknown): boolean => value === undefined || typeof value === "boolean";

/** One list of the request, as `kind`, or undefined when it isn't one: not a
 * list, an item that isn't an object, an id twice. An item whose id, version,
 * hash or flags aren't ones is as an item the store hasn't got: left out, and
 * the rest answered. */
export const parseList = (kind: Kind, value: unknown): Installed[] | undefined => {
  if (!Array.isArray(value)) return undefined;
  const items: Installed[] = [];
  const seen = new Set<string>();
  for (const item of value as unknown[]) {
    if (!item || typeof item !== "object" || Array.isArray(item)) return undefined;
    const { id, version, hash, prerelease, verified_only } = item as Record<string, unknown>;
    if (typeof id !== "string" || !ID_RE.test(id)) continue;
    if (seen.has(id)) return undefined;
    seen.add(id);
    if (typeof version !== "string" || !parseVersion(version) || typeof hash !== "string" || !HASH_RE.test(hash)) continue;
    if (!isFlag(prerelease) || !isFlag(verified_only)) continue;
    items.push({ kind, id, version, hash, prerelease: prerelease === true, verifiedOnly: verified_only === true });
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
  if (!plugins || !templates) return "Each item is an object, { id, version, hash, prerelease?, verified_only? }, each id once in its list.";
  return { zeron, items: [...plugins, ...templates] };
};
