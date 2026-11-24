// The guide at Wing's size, for judging it (never committed pictures): 1728x1117 dark and light, the first
// screen and the whole page; the first screen scrolled to "Publishing from CI" in dark (the rail's current
// part); and a phone's, 390, dark and light. Run on a build: `node --experimental-strip-types tests/look/guide-shots.ts <dir>`.
import { mkdirSync } from "node:fs";
import { chromium } from "playwright";
import { startSite } from "../harness.ts";

const out = process.argv[2] ?? "guide-shots";
mkdirSync(out, { recursive: true });
const site = await startSite();
const browser = await chromium.launch();
try {
  for (const [width, height] of [[1728, 1117], [390, 844]] as const)
    for (const scheme of ["dark", "light"] as const) {
      const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: 2, colorScheme: scheme, reducedMotion: "reduce" });
      const page = await context.newPage();
      await page.goto(new URL("/plugins/publishing", site.url).href, { waitUntil: "networkidle" });
      await page.evaluate(() => document.fonts.ready);
      await page.screenshot({ path: `${out}/guide-${width}-${scheme}-screen.png` });
      await page.screenshot({ path: `${out}/guide-${width}-${scheme}-full.png`, fullPage: true });
      if (width === 1728 && scheme === "dark") {
        await page.goto(new URL("/plugins/publishing#ci", site.url).href, { waitUntil: "networkidle" });
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${out}/guide-${width}-${scheme}-at-ci.png` });
      }
      await context.close();
    }
} finally {
  await browser.close();
  await site.dispose();
}
