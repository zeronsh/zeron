// No general-purpose markdown parser is left anywhere the site's server
// could run it on an author's text: the store's markdown is read by its own
// renderer (src/lib/store/md), whose cost is linear by construction. Held
// on the build (npm test builds first) and on the lockfile, so one coming
// back with a dependency fails here.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";

const ROOT = new URL("../", import.meta.url).pathname;

/**
 * Markdown parsers: micromark's and its extensions, mdast's builder from
 * markdown, remark's and rehype's plugins, and the other well-known ones.
 * (Not unified alone, or micromark's character tables: Astro carries them
 * for its own pages, and neither reads markdown.)
 */
const PARSERS = /^(micromark|micromark-core-commonmark|micromark-extension-.*|mdast-util-from-markdown|mdast-util-gfm.*|remark(-.*)?|rehype-(?!.*types).*|markdown-it|marked|commonmark)$/;

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

describe("no markdown parser but the store's own", () => {
  test("none is installed", () => {
    const lock = JSON.parse(readFileSync(join(ROOT, "package-lock.json"), "utf8")) as { packages: Record<string, unknown> };
    const installed = Object.keys(lock.packages)
      .map((path) => path.split("node_modules/").at(-1)!)
      .filter((name) => PARSERS.test(name));
    expect(installed).toEqual([]);
  });

  test("none is in the server's bundle", () => {
    const code = files(join(ROOT, "dist/server")).filter((path) => /\.m?js$/.test(path));
    expect(code.length).toBeGreaterThan(0);
    for (const path of code) {
      const text = readFileSync(path, "utf8");
      // (micromark's tokenizer names itself throughout; remark's and rehype's plugins by their attacher names.)
      expect(text, path).not.toMatch(/micromark|fromMarkdown\(|remarkParse|remarkGfm|remarkRehype|rehypeSanitize|rehypeStringify/);
    }
  });
});
