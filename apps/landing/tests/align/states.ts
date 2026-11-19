// Every interactive state on every page of the look (a pass to look at, with
// its numbers): what moves when anything is hovered, pressed or focused by
// keyboard (nothing should), and a crop of each group of choices with each
// neighbour of the chosen one hovered, and one focused beside a hovered one.
// Run on a build: `node --experimental-strip-types tests/align/states.ts <dir>`.
import { mkdirSync, writeFileSync } from "node:fs";
import { chromium, type Page } from "playwright";
import { startSite } from "../harness.ts";
import { ALIGN_PAGES, RESET } from "./pages.ts";

const out = process.argv[2] ?? "states-report";
mkdirSync(out, { recursive: true });
const only = process.env.ALIGN_ONLY;
const schemes = (process.env.ALIGN_SCHEMES ?? "light,dark").split(",") as ("light" | "dark")[];
const width = Number(process.env.STATES_WIDTH ?? 1440);

/** Groups of choices: their items, the chosen one among them. */
const GROUPS = [".catlist", ".sort", ".nav", ".tabs", ".sub", ".cats", ".vmini", ".flist", ".pick", ".tags"];

const layout = (page: Page) =>
  page.evaluate(() => [...document.querySelectorAll("body *")].map((el) => {
    const r = el.getBoundingClientRect();
    return [Math.round((r.left + scrollX) * 2) / 2, Math.round((r.top + scrollY) * 2) / 2, Math.round(r.width * 2) / 2, Math.round(r.height * 2) / 2];
  }));
/** What moved, but the thing itself, inside it, and what holds it (a press may scale it). */
const moved = async (page: Page, before: number[][], index: number) => {
  const after = await layout(page);
  return page.evaluate(({ before, after, index }) => {
    const all = [...document.querySelectorAll("body *")];
    const target = all[index]!;
    const out: string[] = [];
    for (let i = 0; i < all.length && i < before.length; i++) {
      const el = all[i]!;
      if (el === target || target.contains(el) || el.contains(target)) continue;
      const [a, b] = [before[i]!, after[i]!];
      if (a.some((v, k) => Math.abs(v - b[k]!) > 0.5)) out.push(`${el.tagName.toLowerCase()}.${[...el.classList].slice(0, 2).join(".")} [${a}] → [${b}]`);
    }
    return out.slice(0, 5);
  }, { before, after, index });
};

const site = await startSite();
const browser = await chromium.launch();
const shifts: Record<string, string[]> = {};
let crops = 0;
try {
  for (const p of ALIGN_PAGES.filter((x) => !only || only.split(",").some((o) => x.name.startsWith(o)))) {
    for (const [what, params] of p.fake ?? []) await site.fake(what, params);
    for (const scheme of schemes) {
      const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: scheme, reducedMotion: "reduce" });
      const page = await context.newPage();
      if (p.user) await page.goto(new URL(`/auth/web/callback?as=${p.user}&return_to=/plugins`, site.url).href);
      await page.goto(new URL(p.path, site.url).href, { waitUntil: "networkidle" });
      await page.evaluate(() => document.fonts.ready);
      const key = `${p.name}-${width}-${scheme}`;
      // Hover and press, each interactive thing in its turn.
      const targets = await page.evaluate(() => {
        const all = [...document.querySelectorAll("body *")];
        return [...document.querySelectorAll("a[href], button, summary, input:not([type=hidden]), select, textarea")]
          .filter((el) => (el as HTMLElement).checkVisibility?.({ checkVisibilityCSS: true }) && el.getBoundingClientRect().width > 0 && !el.closest("[popover]:not(:popover-open), dialog:not([open])"))
          .slice(0, 160)
          .map((el) => all.indexOf(el));
      });
      for (const index of targets) {
        const handle = (await page.evaluateHandle((i) => document.querySelectorAll("body *")[i] ?? null, index)).asElement();
        if (!handle) continue;
        await handle.scrollIntoViewIfNeeded().catch(() => undefined);
        await page.mouse.move(0, 0);
        const base = await layout(page);
        const box = await handle.boundingBox();
        if (!box) continue;
        await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
        const onHover = await moved(page, base, index);
        await page.mouse.down();
        const onPress = await moved(page, base, index);
        await page.mouse.move(0, 0);
        await page.mouse.up();
        for (const m of onHover) (shifts[key] ??= []).push(`hover ${index}: ${m}`);
        for (const m of onPress) (shifts[key] ??= []).push(`press ${index}: ${m}`);
      }
      // Focus by keyboard, all the way round.
      await page.mouse.move(0, 0);
      await page.evaluate(() => scrollTo(0, 0));
      const base = await layout(page);
      for (let i = 0; i < 120; i++) {
        await page.keyboard.press("Tab");
        const index = await page.evaluate(() => [...document.querySelectorAll("body *")].indexOf(document.activeElement!));
        if (index < 0) break;
        // (Compared where it sits on the page: tabbing scrolls.)
        for (const m of await moved(page, base, index)) (shifts[key] ??= []).push(`focus ${index}: ${m}`);
      }
      // Each group of choices: each neighbour of the chosen one hovered; one focused beside a hovered one.
      for (const group of GROUPS) {
        const groups = await page.$$(group);
        for (const [gi, g] of groups.entries()) {
          if (!(await g.isVisible())) continue;
          const items = await g.$$(":scope > a, :scope > li > a, :scope > .chip, :scope > li > .tag");
          const current = await g.evaluate((el) => [...el.querySelectorAll(":scope > a, :scope > li > a, :scope > .chip, :scope > li > .tag")].findIndex((a) => a.hasAttribute("aria-current") || a.classList.contains("solid")));
          const near = current >= 0 ? [current - 1, current + 1] : [0, items.length - 1];
          for (const n of near) {
            const item = items[n];
            if (!item) continue;
            await item.scrollIntoViewIfNeeded().catch(() => undefined);
            await item.hover();
            const clip = await g.boundingBox();
            if (clip) await page.screenshot({ path: `${out}/${key}-${group.slice(1)}${gi}-hover${n}.png`, clip: { x: Math.max(0, clip.x - 12), y: Math.max(0, clip.y - 10), width: Math.min(clip.width + 24, width), height: clip.height + 20 } });
            crops++;
          }
          if (items.length > 2) {
            await page.keyboard.press("Shift");
            await items[0]!.evaluate((el) => (el as HTMLElement).focus());
            await items[1]!.hover();
            const clip = await g.boundingBox();
            if (clip) await page.screenshot({ path: `${out}/${key}-${group.slice(1)}${gi}-focus0-hover1.png`, clip: { x: Math.max(0, clip.x - 12), y: Math.max(0, clip.y - 10), width: Math.min(clip.width + 24, width), height: clip.height + 20 } });
            crops++;
          }
          await page.mouse.move(0, 0);
        }
      }
      await context.close();
    }
    for (const [what, params] of RESET) if (p.fake?.some(([w]) => w === what)) await site.fake(what, params);
  }
} finally {
  await browser.close();
  await site.dispose();
}
writeFileSync(`${out}/shifts.json`, JSON.stringify(shifts, null, 1));
for (const [key, list] of Object.entries(shifts)) console.log(`${key}: ${list.length}`);
console.log(`crops: ${crops}; pages with a shift: ${Object.keys(shifts).length}`);
