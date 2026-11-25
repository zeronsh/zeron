// What a plugin's pages (its Overview, its versions, what it asks for, one
// version) ask the edge for, asked the same way on each: the plugin, a
// page of its versions, the categories' names, and the latest Zeron (its
// Install is off when no released Zeron can install it).
//
// An id no plugin is listed under (unknown, taken down or removed, or not
// an id at all) is one answer for all: "missing", which says nothing of
// why. (A malformed id never reaches the edge.)
import type { Edge } from "./edge.ts";
import type { Kind } from "./kind.ts";
import { isPluginId } from "./names.ts";
import { CATEGORIES } from "./nav.ts";
import type { PluginDetail, VersionsPage } from "./types.ts";

export type PluginPage =
  | { state: "missing" }
  | { state: "down" }
  | { state: "ok"; plugin: PluginDetail & { version: string }; versions: VersionsPage | null; category: string | null; latestZeron: string | null };

export async function loadPlugin(edge: Edge, kind: Kind, asked: string, versions: { cursor?: string; limit: number } = { limit: 20 }): Promise<PluginPage> {
  if (!isPluginId(asked)) return { state: "missing" };
  const [result, versionsResult, categoriesResult, latestZeron] = await Promise.all([
    edge.plugin(asked, kind),
    edge.versions(asked, versions.cursor, versions.limit, kind),
    // (A template has no categories.)
    kind === "plugin" ? edge.categories() : null,
    edge.latestRelease(),
  ]);
  // (A listed plugin always has a version; one without is not the public's.)
  if ((!result.ok && result.error === "not_found") || (result.ok && !result.data.version)) return { state: "missing" };
  if (!result.ok) return { state: "down" };
  const plugin = { ...result.data, version: result.data.version! };
  const first = plugin.categories[0];
  // (The app's names for the fixed list, else the edge's.)
  const category = first ? (CATEGORIES.find((c) => c.id === first)?.label ?? (categoriesResult?.ok ? categoriesResult.data.categories.find((c) => c.id === first)?.name : undefined) ?? first) : null;
  return { state: "ok", plugin, versions: versionsResult.ok ? versionsResult.data : null, category, latestZeron };
}

/**
 * A version's neighbours in the store's order (newest first): the newer one
 * and the older one, each null at its end (a version page's stepper). From
 * the versions the plugin's own answer carries, then on from its cursor, 50
 * at a time and at most 10 pages, only when the version or its older
 * neighbour lies past them. Null when the list can't be walked.
 */
export async function neighbours(edge: Edge, kind: Kind, plugin: PluginDetail, version: string): Promise<{ newer: string | null; older: string | null } | null> {
  const seen = plugin.versions.map((v) => v.version);
  let cursor = plugin.versions_next_cursor;
  for (let pages = 0; ; pages++) {
    const at = seen.indexOf(version);
    // (Found, with what follows it known: the next on the list, or none after it.)
    if (at >= 0 && (at < seen.length - 1 || !cursor)) return { newer: seen[at - 1] ?? null, older: seen[at + 1] ?? null };
    if (!cursor || pages >= 10) return null;
    const page = await edge.versions(plugin.id, cursor, 50, kind);
    if (!page.ok) return null;
    seen.push(...page.data.versions.map((v) => v.version));
    cursor = page.data.next_cursor;
  }
}
