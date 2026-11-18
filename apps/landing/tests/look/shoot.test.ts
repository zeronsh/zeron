// Pictures of the store's pages, for judging the look (never committed):
// each page at a desktop's and a phone's width, light and dark, from the
// fake's richest data and from its emptiest. Runs only when asked:
//   SHOOT=/some/dir [SHOOT_PAGES=discover,plugin] npx vitest run tests/look/shoot.test.ts
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { chromium, type Browser } from "playwright";
import { afterAll, beforeAll, test } from "vitest";
import { startSite, type Site } from "../harness.ts";

const DIR = process.env.SHOOT;
const ONLY = process.env.SHOOT_PAGES?.split(",");

interface Shot {
  page: string;
  /** What it shows, in its file's name. */
  as: string;
  path: string;
  user?: string;
  /** The fake's switches before it's taken (POST /_fake/<what>?<params>), reset after. */
  fake?: [string, Record<string, string>][];
}

const SHOTS: Shot[] = [
  { page: "discover", as: "rich", path: "/plugins" },
  { page: "discover", as: "signed-in", path: "/plugins", user: "user_grace" },
  { page: "discover", as: "no-pictures", path: "/plugins", fake: [["bare", { id: "focus-timer" }]] },
  { page: "discover", as: "empty", path: "/plugins", fake: [["empty", { on: "1" }]] },
  { page: "discover", as: "down", path: "/plugins", fake: [["down", { on: "1" }]] },
];
const RESET: [string, Record<string, string>][] = [["empty", {}], ["down", {}], ["bare", {}]];

let site: Site;
let browser: Browser;
beforeAll(async () => {
  if (!DIR) return;
  mkdirSync(DIR, { recursive: true });
  [site, browser] = await Promise.all([startSite(), chromium.launch()]);
}, 60_000);
afterAll(async () => {
  await browser?.close();
  await site?.dispose();
});

test.skipIf(!DIR)("pictures of the store's pages", async () => {
  for (const shot of SHOTS.filter((s) => !ONLY || ONLY.includes(s.page))) {
    for (const [what, params] of shot.fake ?? []) await site.fake(what, params);
    for (const width of [1440, 390]) {
      for (const scheme of ["light", "dark"] as const) {
        const context = await browser.newContext({ viewport: { width, height: 900 }, deviceScaleFactor: width < 700 ? 3 : 2, colorScheme: scheme, reducedMotion: "reduce" });
        const page = await context.newPage();
        if (shot.user) await page.goto(new URL(`/auth/web/callback?as=${shot.user}&return_to=/plugins`, site.url).href);
        await page.goto(new URL(shot.path, site.url).href, { waitUntil: "networkidle" });
        await page.evaluate(() => document.fonts.ready);
        await page.screenshot({ path: join(DIR!, `${shot.page}-${shot.as}-${width}-${scheme}.png`), fullPage: true });
        await context.close();
      }
    }
    for (const [what, params] of RESET) if (shot.fake?.some(([w]) => w === what)) await site.fake(what, params);
  }
}, 600_000);
