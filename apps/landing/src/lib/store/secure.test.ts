// The guard that keeps one person's cookie out of every shared cache, held
// to Astro's own cache: a page marks itself cacheable, the middleware's
// last step runs, and only then does Astro add the page's cache headers
// (astro/dist/core/cache/handler.js), as on the Worker. (Astro's cache
// runtime is internal; if it moves, this test says so before the guard
// quietly stops working.)
import { describe, expect, test } from "vitest";
import { secure } from "./secure.ts";

interface Cache {
  set(input: { maxAge?: number; swr?: number; tags?: string[] } | false): void;
}
const runtime = (await import(new URL("../../../node_modules/astro/dist/core/cache/runtime/cache.js", import.meta.url).href)) as {
  AstroCache: new (provider?: unknown) => Cache;
  applyCacheHeaders(cache: Cache, response: Response, request: Request): void;
};

/** A public page's answer, as Astro sends it: secured by the middleware, then given the page's cache headers. */
async function served(status: number, cookies: string[]): Promise<Response> {
  const cache = new runtime.AstroCache();
  // (What a public page asks for: lib/store/http.ts publicPage.)
  cache.set({ maxAge: 30, swr: 30, tags: ["store:catalog"] });
  const page = new Response(status === 404 ? null : "<!doctype html><title>x</title>", { status, headers: status === 404 ? {} : { "content-type": "text/html; charset=utf-8" } });
  const response = await secure(cache, page, cookies);
  runtime.applyCacheHeaders(cache, response, new Request("https://zeron.sh/plugins"));
  return response;
}

const ROTATED = "__Host-zeron_session=next; Path=/; Secure; HttpOnly; SameSite=Lax";

describe("a response that carries a cookie is kept by no one", () => {
  test("without a cookie, a public page keeps the shared-cache headers it asked for (the test can see them)", async () => {
    const response = await served(200, []);
    expect(response.headers.get("cdn-cache-control")).toBe("max-age=30, stale-while-revalidate=30");
    expect(response.headers.get("cache-tag")).toBe("store:catalog");
  });

  test("with the edge's rotated session, it gets none of them, and goes out private", async () => {
    const response = await served(200, [ROTATED]);
    expect(response.headers.get("cdn-cache-control")).toBeNull();
    expect(response.headers.get("cache-tag")).toBeNull();
    expect(response.headers.get("cache-control")).toBe("private, no-store");
    expect(response.headers.getSetCookie()).toEqual([ROTATED]);
  });

  test("the same for a 404 that carries one", async () => {
    const response = await served(404, [ROTATED]);
    expect(response.headers.get("cdn-cache-control")).toBeNull();
    expect(response.headers.get("cache-tag")).toBeNull();
    expect(response.headers.get("cache-control")).toBe("private, no-store");
  });
});
