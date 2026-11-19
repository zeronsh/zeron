// The site's own pages never depend on the edge: with no EDGE binding at
// all, or one that fails every request, they are whole, and the store's
// pages say the store can't be reached rather than failing.
import { readFileSync } from "node:fs";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { startSite, type Site } from "./harness.ts";

const file = (name: string) => readFileSync(new URL(`../dist/client/${name}`, import.meta.url), "utf8");

for (const edge of ["missing", "broken"] as const) {
  describe(`with the edge ${edge}`, () => {
    let site: Site;
    beforeAll(async () => {
      site = await startSite({ edge });
    }, 60_000);
    afterAll(() => site?.dispose());

    test("the home page, privacy, the 404 page, robots and the sitemaps are whole", async () => {
      for (const [path, body] of [["/", file("index.html")], ["/privacy", file("privacy.html")]] as const) {
        const response = await site.fetch(path);
        expect(response.status, path).toBe(200);
        expect(await response.text(), path).toBe(body);
      }
      const missing = await site.fetch("/no/such/page");
      expect(missing.status).toBe(404);
      expect(await missing.text()).toBe(file("404.html"));
      for (const path of ["/robots.txt", "/sitemap.xml", "/sitemap-pages.xml"]) expect((await site.fetch(path)).status, path).toBe(200);
    });

    test("the store's pages say it can't be reached, and are never kept", async () => {
      for (const [path, words] of [["/plugins", /The plugin store didn(&#39;|')t answer/], ["/plugins/focus-timer", /The plugin store didn(&#39;|')t answer/]] as const) {
        const response = await site.fetch(path);
        expect(response.status, path).toBe(503);
        expect(await response.text(), path).toMatch(words);
        expect(response.headers.get("cloudflare-cdn-cache-control") ?? "", path).not.toMatch(/public/);
      }
      // All plugins says it in the app's words, and offers to try again.
      const list = await site.fetch("/plugins/all");
      expect(list.status).toBe(503);
      const words = await list.text();
      expect(words).toMatch(/The plugin store didn(&#39;|')t answer/);
      expect(words).toContain('href="/plugins/all">Try again</a>');
      expect(list.headers.get("cloudflare-cdn-cache-control") ?? "").not.toMatch(/public/);
      expect((await site.fetch("/plugins/focus-timer/files/1.2.0/icon.svg")).status).toBe(404);
      expect((await site.fetch("/plugins/sitemap.xml")).status).toBe(503);
    });

    test("your own pages say the store didn't answer (never that you're signed out), and the admin pages stay the 404", async () => {
      const cookie = "__Host-zeron_session=whatever";
      const account = await site.fetch("/account", { headers: { cookie } });
      expect(account.status).toBe(503);
      expect(await account.text()).toContain("The plugin store didn&#39;t answer");
      expect((await site.fetch("/admin/plugins", { headers: { cookie } })).status).toBe(404);
    });
  });
}
