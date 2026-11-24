// The site's inline styles are allowed by a fixed list of their hashes
// (src/lib/store/inline-styles.json, read by security.ts), never by what a
// page carries. Every kind of page is rendered here: a style on one that
// isn't in the list fails (it would be refused in a browser), and so does a
// listed one no page carries any more. After changing a component's style:
// `npm run build && WRITE_STYLE_HASHES=1 npx vitest run tests/styles.test.ts`.
import { readFileSync, writeFileSync } from "node:fs";
import { afterAll, beforeAll, expect, test } from "vitest";
import { inlineStyles, styleHash } from "../src/lib/store/security.ts";
import { startSite, type Site } from "./harness.ts";

const LIST = new URL("../src/lib/store/inline-styles.json", import.meta.url);

let site: Site;
beforeAll(async () => {
  site = await startSite();
}, 60_000);
afterAll(() => site?.dispose());

/** Every kind of page, each as the one who sees it. */
const PAGES: [string, string | undefined][] = [
  ["/plugins", undefined],
  ["/plugins?sort=new", undefined],
  ["/plugins?q=zzzzqqq", undefined],
  ["/plugins/focus-timer", undefined],
  ["/plugins/xss-probe", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12", undefined],
  ["/plugins/slow-words", undefined],
  ["/plugins/no-such-plugin", undefined],
  ["/plugins/publishing", undefined],
  ["/nothing-here", undefined],
  ["/plugins/theme-sync/report", "user_kai"],
  ["/account", "user_ada"],
  ["/account", "user_noor"],
  ["/account/plugins/worst-case-listing-at-every-limit-xyz12", "user_ada"],
  ["/account/plugins/session-badges/versions/1.2.1/withdraw", "user_ada"],
  ["/admin/plugins", "user_grace"],
  ["/admin/plugins/worst-case-listing-at-every-limit-xyz12", "user_grace"],
  ["/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=files", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=files&path=init.luau", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=changes", "user_grace"],
  ["/admin/plugins/slow-words/1.2.0?view=changes", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1/reject", "user_grace"],
  ["/admin/plugins/session-badges/actions/suspend", "user_grace"],
  ["/admin/reports", "user_grace"],
  ["/admin/audit", "user_grace"],
  ["/admin/reservations", "user_grace"],
  ["/admin/reservations/linear", "user_grace"],
];

test("every inline style on every kind of page is in the list, and every listed one is on a page", async () => {
  const seen = new Map<string, string>();
  for (const [path, as] of PAGES) {
    const cookie = as ? await site.signIn(as) : undefined;
    const response = await site.fetch(path, cookie ? { headers: { cookie } } : {});
    const html = await response.text();
    for (const style of inlineStyles(html)) seen.set(await styleHash(style), path);
  }
  const found = [...seen.keys()].sort();
  if (process.env.WRITE_STYLE_HASHES === "1") writeFileSync(LIST, `${JSON.stringify({ hashes: found }, null, 2)}\n`);
  const listed = (JSON.parse(readFileSync(LIST, "utf8")) as { hashes: string[] }).hashes;
  const missing = found.filter((h) => !listed.includes(h)).map((h) => `${h} (on ${seen.get(h)})`);
  expect(missing, "styles a page carries that the list doesn't: run with WRITE_STYLE_HASHES=1").toEqual([]);
  expect(listed.filter((h) => !found.includes(h)), "listed styles no page carries any more").toEqual([]);
}, 120_000);
