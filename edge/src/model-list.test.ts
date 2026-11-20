import { describe, expect, it } from "vitest";
import type { Env } from "./env";
import { modelListRoute } from "./model-list";
import staging from "./models/staging-v1.json";

const at = (env: Partial<Env>, init?: RequestInit, path = "/models/v1.json") => {
  const url = new URL(`https://edge.test${path}`);
  return modelListRoute(new Request(url, init), env as Env, url);
};

describe("hosted model list (/models/v1.json)", () => {
  it("serves the staging list with an ETag and answers a matching If-None-Match with 304", async () => {
    const response = (await at({ MODEL_LIST: "staging" }))!;
    expect(response.status).toBe(200);
    expect(response.headers.get("content-type")).toBe("application/json");
    expect(response.headers.get("cache-control")).toBe("public, max-age=300");
    expect(await response.json()).toEqual(staging);
    const etag = response.headers.get("etag")!;
    expect(etag).toMatch(/^"[0-9a-f]{32}"$/);

    const again = (await at({ MODEL_LIST: "staging" }, { headers: { "if-none-match": `"other", ${etag}` } }))!;
    expect(again.status).toBe(304);
    expect(await again.text()).toBe("");

    const stale = (await at({ MODEL_LIST: "staging" }, { headers: { "if-none-match": '"other"' } }))!;
    expect(stale.status).toBe(200);
    await stale.arrayBuffer();
  });

  it("is a 404 where no list is set (production today) or the name is unknown", async () => {
    for (const env of [{}, { MODEL_LIST: "" }, { MODEL_LIST: "production" }, { MODEL_LIST: "__proto__" }]) {
      const response = (await at(env))!;
      expect(response.status).toBe(404);
      await response.arrayBuffer();
    }
  });

  it("leaves other paths and methods to later routes", async () => {
    expect(await at({ MODEL_LIST: "staging" }, undefined, "/models/v2.json")).toBeNull();
    expect(await at({ MODEL_LIST: "staging" }, { method: "POST", body: "{}" })).toBeNull();
  });

  it("stays well under the engine's 64 KiB limit", () => {
    expect(new TextEncoder().encode(JSON.stringify(staging)).length).toBeLessThan(16 * 1024);
  });
});
