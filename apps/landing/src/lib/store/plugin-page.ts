// What a plugin's pages (its Overview, its versions, what it asks for, one
// version) ask the edge for, asked the same way on each: the plugin, a
// page of its versions, the categories' names, and the latest Zeron (its
// Install is off when no released Zeron can install it).
//
// An id no plugin is listed under (unknown, taken down or removed, or not
// an id at all) is one answer for all: "missing", which says nothing of
// why. (A malformed id never reaches the edge.)
import type { Edge } from "./edge.ts";
import { isPluginId } from "./names.ts";
import { CATEGORIES } from "./nav.ts";
import type { PluginDetail, VersionsPage } from "./types.ts";

export type PluginPage =
  | { state: "missing" }
  | { state: "down" }
  | { state: "ok"; plugin: PluginDetail & { version: string }; versions: VersionsPage | null; category: string | null; latestZeron: string | null };

export async function loadPlugin(edge: Edge, asked: string, versions: { cursor?: string; limit: number } = { limit: 20 }): Promise<PluginPage> {
  if (!isPluginId(asked)) return { state: "missing" };
  const [result, versionsResult, categoriesResult, latestZeron] = await Promise.all([
    edge.plugin(asked),
    edge.versions(asked, versions.cursor, versions.limit),
    edge.categories(),
    edge.latestRelease(),
  ]);
  // (A listed plugin always has a version; one without is not the public's.)
  if ((!result.ok && result.error === "not_found") || (result.ok && !result.data.version)) return { state: "missing" };
  if (!result.ok) return { state: "down" };
  const plugin = { ...result.data, version: result.data.version! };
  const first = plugin.categories[0];
  // (The app's names for the fixed list, else the edge's.)
  const category = first ? (CATEGORIES.find((c) => c.id === first)?.label ?? (categoriesResult.ok ? categoriesResult.data.categories.find((c) => c.id === first)?.name : undefined) ?? first) : null;
  return { state: "ok", plugin, versions: versionsResult.ok ? versionsResult.data : null, category, latestZeron };
}
