// What the site's pages ask of the catalog (the contract's "What the site's
// pages ask of the catalog", ac4fc5d3f): a category's most installed plugins
// in one call, the plugins new this week, and a category's keywords. Each is
// held to the lists it summarises, so a fake and the real edge are held to
// the same thing: tests/client.test.ts runs these against the fake, and
// tests/edge/site.test.ts against the edge.
import { expect } from "vitest";
import type { Edge } from "../src/lib/store/edge.ts";

const data = <T>(result: { ok: true; data: T } | { ok: false; error: string }): T => {
  expect(result.ok, JSON.stringify(result)).toBe(true);
  return (result as { data: T }).data;
};

export async function catalogAsks(edge: Edge): Promise<void> {
  // /stats: new this week among what's listed.
  const stats = data(await edge.stats());
  expect(typeof stats.new_7d).toBe("number");
  expect(stats.new_7d!).toBeLessThanOrEqual(stats.plugins);
  // (About as many as the newest list has from the last seven days, by the real clock: the fake's is the hour it started in.)
  const newest = data(await edge.catalog({ sort: "new", limit: 50 }));
  const since = (days: number) => newest.items.filter((c) => (c.listed_at ?? 0) >= Date.now() - days * 86_400_000).length;
  if (newest.total <= 50) {
    expect(stats.new_7d!).toBeGreaterThanOrEqual(since(5));
    expect(stats.new_7d!).toBeLessThanOrEqual(since(10));
  }

  // /categories?top=n: each category's n most installed, as its popular list has them.
  const plain = data(await edge.categories());
  expect(plain.categories.every((c) => c.top === undefined)).toBe(true);
  const topped = data(await edge.categories(2));
  expect(topped.categories.map((c) => c.id)).toEqual(plain.categories.map((c) => c.id));
  for (const c of topped.categories) {
    const list = data(await edge.catalog({ category: c.id, sort: "popular", limit: 2 }));
    expect(c.top!.map((p) => p.id), c.id).toEqual(list.items.map((p) => p.id));
    expect(c.count, c.id).toBe(list.total);
  }
  expect(await edge.categories(9 as never)).toMatchObject({ ok: false, error: "bad_request" });

  // /keywords: most used first, ties by keyword; each count the list's total for ?keyword=.
  const all = data(await edge.keywords()).keywords;
  expect(all.length).toBeGreaterThan(0);
  expect(all.length).toBeLessThanOrEqual(24);
  const ordered = [...all].sort((x, y) => y.count - x.count || (x.keyword < y.keyword ? -1 : x.keyword > y.keyword ? 1 : 0));
  expect(all).toEqual(ordered);
  for (const k of all.slice(0, 3)) expect(data(await edge.catalog({ keyword: k.keyword })).total, k.keyword).toBe(k.count);
  const category = topped.categories.find((c) => (c.count ?? 0) > 0)!.id;
  const inCategory = data(await edge.keywords({ category })).keywords;
  for (const k of inCategory.slice(0, 3)) expect(data(await edge.catalog({ category, keyword: k.keyword })).total, `${category} ${k.keyword}`).toBe(k.count);
  expect(data(await edge.keywords({ limit: 1 })).keywords).toEqual(all.slice(0, 1));
  expect(await edge.keywords({ limit: 25 })).toMatchObject({ ok: false, error: "bad_request" });
  expect(await edge.keywords({ category: "no-such-category" })).toMatchObject({ ok: false, error: "bad_request" });
}
