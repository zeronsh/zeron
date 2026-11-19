// Measures every page of the look against Wing's rule (ink.ts) and draws
// each column on a picture of it: the sheet, and its numbers. Run on a
// build: `npm run build && node --experimental-strip-types tests/align/report.ts <dir>`.
// ALIGN_ONLY (a page name's start), ALIGN_WIDTHS ("1440,390") and
// ALIGN_SCHEMES ("light,dark") narrow it; ALIGN_SHOTS=0 measures alone.
import { mkdirSync, writeFileSync } from "node:fs";
import { chromium } from "playwright";
import { startSite } from "../harness.ts";
import { inkAudit, type InkReport } from "./ink.ts";
import { ALIGN_PAGES, COLUMNS, EXCEPTIONS, RESET } from "./pages.ts";

const out = process.argv[2] ?? "align-report";
mkdirSync(out, { recursive: true });
const only = process.env.ALIGN_ONLY;
const widths = (process.env.ALIGN_WIDTHS ?? "1440,390").split(",").map(Number);
const schemes = (process.env.ALIGN_SCHEMES ?? "light,dark").split(",") as ("light" | "dark")[];
const shots = process.env.ALIGN_SHOTS !== "0";

const site = await startSite();
const browser = await chromium.launch();
const results: Record<string, InkReport> = {};
try {
  for (const p of ALIGN_PAGES.filter((x) => !only || only.split(",").some((o) => x.name.startsWith(o)))) {
    for (const [what, params] of p.fake ?? []) await site.fake(what, params);
    for (const width of widths)
      for (const scheme of schemes) {
        const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: scheme, reducedMotion: "reduce", deviceScaleFactor: 1 });
        const page = await context.newPage();
        if (p.user) await page.goto(new URL(`/auth/web/callback?as=${p.user}&return_to=/plugins`, site.url).href);
        await page.goto(new URL(p.path, site.url).href, { waitUntil: "networkidle" });
        await page.evaluate(() => document.fonts.ready);
        const report = await page.evaluate(inkAudit, { columns: COLUMNS, window: 40, tolerance: 1, exceptions: EXCEPTIONS });
        const key = `${p.name}-${width}-${scheme}`;
        results[key] = report;
        if (shots) {
          // (Each column a thin line; each miss a short tick at its ink.)
          await page.evaluate(({ report }) => {
            const layer = document.createElement("div");
            layer.style.cssText = "position:absolute;left:0;top:0;width:100%;pointer-events:none;z-index:99999";
            layer.style.height = `${document.documentElement.scrollHeight}px`;
            for (const c of report.columns) {
              const line = document.createElement("div");
              line.style.cssText = `position:absolute;top:0;bottom:0;width:1px;left:${c.x + scrollX}px;background:rgba(0,170,255,.55)`;
              layer.append(line);
            }
            document.body.append(layer);
          }, { report });
          await page.screenshot({ path: `${out}/${key}.png`, fullPage: true });
        }
        await context.close();
      }
    for (const [what, params] of RESET) if (p.fake?.some(([w]) => w === what)) await site.fake(what, params);
  }
} finally {
  await browser.close();
  await site.dispose();
}
writeFileSync(`${out}/report.json`, JSON.stringify(results, null, 1));
let total = 0;
for (const [key, r] of Object.entries(results)) {
  total += r.misses.length;
  if (r.misses.length) console.log(`${key}: ${r.misses.length} of ${r.checked}`);
}
console.log(`misses: ${total}`);
