// Pictures of the store's pages, for judging the look (never committed):
// each page at a desktop's and a phone's width, light and dark, from the
// fake's richest data and from its emptiest. Runs only when asked:
//   SHOOT=/some/dir [SHOOT_PAGES=discover,plugin] npx vitest run tests/look/shoot.test.ts
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { chromium, type Browser, type Page } from "playwright";
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
  /** What to do on the page first (open a disclosure, follow Next), at the widths given. */
  act?: (page: Page) => Promise<void>;
  widths?: number[];
}

const SHOTS: Shot[] = [
  { page: "discover", as: "rich", path: "/plugins" },
  { page: "discover", as: "signed-in", path: "/plugins", user: "user_grace" },
  { page: "discover", as: "no-pictures", path: "/plugins", fake: [["bare", { id: "focus-timer" }]] },
  { page: "discover", as: "empty", path: "/plugins", fake: [["empty", { on: "1" }]] },
  { page: "discover", as: "down", path: "/plugins", fake: [["down", { on: "1" }]] },
  // A plugin: every version verified, the README with every form; its newest not verified (the amber note); nothing verified yet; no pictures.
  { page: "plugin", as: "all-verified", path: "/plugins/focus-timer" },
  { page: "plugin", as: "unverified-newest", path: "/plugins/worst-case-listing-at-every-limit-xyz12" },
  { page: "plugin", as: "none-verified", path: "/plugins/voice-memos" },
  { page: "plugin", as: "no-pictures", path: "/plugins/focus-timer", fake: [["bare", { id: "focus-timer" }]] },
  { page: "plugin", as: "versions", path: "/plugins/worst-case-listing-at-every-limit-xyz12/versions" },
  { page: "plugin", as: "asks", path: "/plugins/worst-case-listing-at-every-limit-xyz12/asks" },
  { page: "plugin", as: "version", path: "/plugins/focus-timer/versions/1.1.0" },
  { page: "plugin", as: "version-superseded", path: "/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.4.0" },
  { page: "plugin", as: "taken-down", path: "/plugins/clipboard-sync" },
  { page: "plugin", as: "version-gone", path: "/plugins/focus-timer/versions/9.9.9" },
  // Browse: everything, a category, a search with results, verified only, a keyword, no results; the next page by its cursor; the phone's filters, open.
  { page: "browse", as: "all", path: "/plugins/all" },
  { page: "browse", as: "category", path: "/plugins/all?category=developer-tools" },
  { page: "browse", as: "search", path: "/plugins/all?q=git" },
  { page: "browse", as: "verified-only", path: "/plugins/all?verified=1" },
  { page: "browse", as: "keyword", path: "/plugins/all?keyword=focus" },
  { page: "browse", as: "no-results", path: "/plugins/all?q=zzzzqqq" },
  { page: "browse", as: "page-2", path: "/plugins/all", act: async (page) => { await page.getByRole("link", { name: "Next" }).click(); await page.waitForLoadState("networkidle"); } },
  { page: "browse", as: "filters-open", path: "/plugins/all?category=productivity&keyword=focus", widths: [390], act: async (page) => { await page.locator("details.drop summary", { hasText: "Filters" }).click(); } },
  { page: "browse", as: "order-open", path: "/plugins/all?category=productivity", widths: [390], act: async (page) => { await page.locator("details.drop summary").first().click(); } },
  // Your pages: every state of a plugin and a key (Ada's), signed out, nothing published; the many-plugin key opened; the revoke question.
  { page: "account", as: "plugins", path: "/account", user: "user_ada" },
  { page: "account", as: "plugins-signed-out", path: "/account" },
  { page: "account", as: "plugins-none", path: "/account", user: "user_noor" },
  { page: "account", as: "keys", path: "/account/keys", user: "user_ada" },
  { page: "account", as: "keys-all-open", path: "/account/keys", user: "user_ada", act: async (page) => { await page.locator("details.every summary").first().click(); await page.locator("details.gone summary").click(); } },
  { page: "account", as: "keys-signed-out", path: "/account/keys" },
  { page: "account", as: "keys-none", path: "/account/keys", user: "user_noor" },
  { page: "account", as: "revoke", path: "/account/keys/Qm81LzR2sT3uV4wX5yZ6aB7cD8eF9gH0iJ1kL2mN3oP", user: "user_ada" },
  { page: "account", as: "new-key", path: "/account/keys", user: "user_ada", widths: [1440, 390], act: async (page) => { await page.getByRole("button", { name: "New key" }).click(); } },
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
    for (const width of shot.widths ?? [1440, 390]) {
      for (const scheme of ["light", "dark"] as const) {
        const context = await browser.newContext({ viewport: { width, height: 900 }, deviceScaleFactor: width < 700 ? 3 : 2, colorScheme: scheme, reducedMotion: "reduce" });
        const page = await context.newPage();
        if (shot.user) await page.goto(new URL(`/auth/web/callback?as=${shot.user}&return_to=/plugins`, site.url).href);
        await page.goto(new URL(shot.path, site.url).href, { waitUntil: "networkidle" });
        if (shot.act) {
          await shot.act(page);
          // (A full page is taken from its top: else the bar, fixed, shows where the page was scrolled to.)
          await page.evaluate(() => window.scrollTo(0, 0));
        }
        await page.evaluate(() => document.fonts.ready);
        await page.screenshot({ path: join(DIR!, `${shot.page}-${shot.as}-${width}-${scheme}.png`), fullPage: true });
        await context.close();
      }
    }
    for (const [what, params] of RESET) if (shot.fake?.some(([w]) => w === what)) await site.fake(what, params);
  }
}, 600_000);
