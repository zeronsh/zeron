// Every page of the look (tests/align/pages.ts), full page, light, at a
// desktop's width and a phone's: the before and after of a change to the
// whole look. Run on a build: `node --experimental-strip-types tests/look/every.ts <dir>`.
import { mkdirSync } from "node:fs";
import { chromium } from "playwright";
import { startSite } from "../harness.ts";
import { ALIGN_PAGES, RESET } from "../align/pages.ts";

const out = process.argv[2] ?? "every";
mkdirSync(out, { recursive: true });
const only = process.env.EVERY_ONLY?.split(",");
const site = await startSite();
const browser = await chromium.launch();
try {
  for (const p of ALIGN_PAGES.filter((x) => !only || only.some((o) => x.name.startsWith(o)))) {
    for (const [what, params] of p.fake ?? []) await site.fake(what, params);
    for (const width of [1440, 390]) {
      const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: "light", reducedMotion: "reduce", deviceScaleFactor: width < 700 ? 2 : 1 });
      const page = await context.newPage();
      if (p.user) await page.goto(new URL(`/auth/web/callback?as=${p.user}&return_to=/plugins`, site.url).href);
      const response = await page.goto(new URL(p.path, site.url).href, { waitUntil: "networkidle" });
      if (p.act) await p.act(page);
      await page.evaluate(() => document.fonts.ready);
      await page.screenshot({ path: `${out}/${p.name}-${width}.png`, fullPage: true });
      console.log(p.name, width, response?.status());
      await context.close();
    }
    for (const [what, params] of RESET) if (p.fake?.some(([w]) => w === what)) await site.fake(what, params);
  }
} finally {
  await browser.close();
  await site.dispose();
}
