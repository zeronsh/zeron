// The site's client against the fake edge, for what no page asks yet: the
// catalog's summaries the coming pages will (tests/catalog-asks.ts). The
// real edge is held to the same in tests/edge/site.test.ts.
import { test } from "vitest";
import fake from "../fake-edge/index.ts";
import { Edge } from "../src/lib/store/edge.ts";
import { catalogAsks } from "./catalog-asks.ts";

test("what the site's pages ask of the catalog, from the fake: a category's top, new this week, keywords", async () => {
  await catalogAsks(new Edge({ fetch: (request) => fake.fetch(request) }, new Request("https://zeron.sh/plugins")));
});
