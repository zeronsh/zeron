// The nine pages that were on the old app layout, before and after their
// move to the store's look: each at a desktop's width and a phone's, light
// and dark. Run on a build: `node --experimental-strip-types tests/look/nine.ts <dir>`.
import { mkdirSync } from "node:fs";
import { chromium } from "playwright";
import { startSite } from "../harness.ts";

export const NINE: { name: string; path: string; user?: string }[] = [
  { name: "guide", path: "/plugins/publishing" },
  { name: "report", path: "/plugins/focus-timer/report", user: "user_kai" },
  { name: "audit", path: "/admin/audit", user: "user_grace" },
  { name: "reports", path: "/admin/reports", user: "user_grace" },
  { name: "reservations", path: "/admin/reservations", user: "user_grace" },
  { name: "reservation", path: "/admin/reservations/zeron-tools", user: "user_grace" },
  { name: "staff-plugin", path: "/admin/plugins/focus-timer", user: "user_grace" },
  { name: "plugin-action", path: "/admin/plugins/focus-timer/actions/suspend", user: "user_grace" },
  { name: "version-action", path: "/admin/plugins/session-badges/1.2.1/decline", user: "user_grace" },
];

if (import.meta.url === `file://${process.argv[1]}`) {
  const out = process.argv[2] ?? "nine";
  mkdirSync(out, { recursive: true });
  const site = await startSite();
  const browser = await chromium.launch();
  try {
    for (const p of NINE)
      for (const width of [1440, 390])
        for (const scheme of ["light", "dark"] as const) {
          const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: scheme, reducedMotion: "reduce", deviceScaleFactor: width < 700 ? 2 : 1 });
          const page = await context.newPage();
          if (p.user) await page.goto(new URL(`/auth/web/callback?as=${p.user}&return_to=/plugins`, site.url).href);
          const response = await page.goto(new URL(p.path, site.url).href, { waitUntil: "networkidle" });
          await page.evaluate(() => document.fonts.ready);
          await page.screenshot({ path: `${out}/${p.name}-${width}-${scheme}.png`, fullPage: true });
          console.log(p.name, width, scheme, response?.status());
          await context.close();
        }
  } finally {
    await browser.close();
    await site.dispose();
  }
}
