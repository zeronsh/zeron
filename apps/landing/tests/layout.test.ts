// One look for the store: every page under /plugins, /account and /admin
// is drawn by the store's layout (layouts/Store.astro) in the store's
// styles, and the old app window (layouts/App.astro, styles/app.css,
// scripts/app.ts), which the store replaced, can't come back: nothing
// names it, and it isn't there. A route that draws no page of its own (it
// answers a post with a redirect, or redirects for good) says so here.
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, test } from "vitest";

const root = new URL("..", import.meta.url).pathname;
const src = join(root, "src");

const files = (dir: string): string[] =>
  readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });

/** Routes that draw no page: a post's answer (a redirect), or a redirect for good. */
const NO_PAGE = new Set(["pages/admin/reports/[report].astro", "pages/account/keys/[...rest].astro", "pages/plugins/all.astro"]);

describe("the store's pages are the store's look", () => {
  test("the old app window is gone, and nothing names it", () => {
    for (const gone of ["layouts/App.astro", "styles/app.css", "scripts/app.ts"]) {
      expect(existsSync(join(src, gone)), gone).toBe(false);
      for (const file of files(src)) expect(readFileSync(file, "utf8"), `${relative(src, file)} names ${gone}`).not.toContain(gone);
    }
  });

  test("every page under /plugins, /account and /admin is drawn by layouts/Store.astro", () => {
    const pages = ["plugins", "account", "admin"].flatMap((dir) => files(join(src, "pages", dir))).filter((f) => f.endsWith(".astro"));
    expect(pages.length).toBeGreaterThan(20);
    for (const page of pages) {
      const name = relative(src, page);
      if (NO_PAGE.has(name)) {
        expect(readFileSync(page, "utf8"), `${name} draws no page`).not.toMatch(/<html|<Store\b|<Base\b/);
        continue;
      }
      expect(readFileSync(page, "utf8"), name).toMatch(/import Store from "(\.\.\/)+layouts\/Store\.astro";/);
    }
  });
});
