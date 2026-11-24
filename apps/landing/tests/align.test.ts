// Text lines up with text, not with boxes (Wing's rule), on every page of the
// look in its full and spare states, at a desktop's width and a phone's, in
// both schemes (align/ink.ts the measure, align/pages.ts the pages and the
// rule's exceptions). Then what the rule asks by name: a plugin's Get is its
// rail's column; Deploy tokens' words start on one column, its header's; and
// no two of a group's items that fill (hover, chosen, focus) ever touch.
import { chromium, type Browser, type Page } from "playwright";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { startSite, type Site } from "./harness.ts";
import { inkAudit } from "./align/ink.ts";
import { ALIGN_PAGES, COLUMNS, EXCEPTIONS, RESET, type AlignPage } from "./align/pages.ts";

let site: Site;
let browser: Browser;

beforeAll(async () => {
  [site, browser] = await Promise.all([startSite(), chromium.launch()]);
}, 60_000);

afterAll(async () => {
  await browser?.close();
  await site?.dispose();
});

async function open(p: Pick<AlignPage, "path" | "user" | "act">, width: number, scheme: "light" | "dark"): Promise<Page> {
  const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: scheme, reducedMotion: "reduce" });
  const page = await context.newPage();
  if (p.user) await page.goto(new URL(`/auth/web/callback?as=${p.user}&return_to=/plugins`, site.url).href);
  await page.goto(new URL(p.path, site.url).href, { waitUntil: "networkidle" });
  if (p.act) await p.act(page);
  await page.evaluate(() => document.fonts.ready);
  return page;
}

describe("every run of text that begins a line puts its ink on its column", () => {
  for (const p of ALIGN_PAGES)
    test(p.name, async () => {
      for (const [what, params] of p.fake ?? []) await site.fake(what, params);
      try {
        const misses: string[] = [];
        for (const width of [1440, 390])
          for (const scheme of ["light", "dark"] as const) {
            const page = await open(p, width, scheme);
            try {
              const report = await page.evaluate(inkAudit, { columns: COLUMNS, window: 40, tolerance: 1, exceptions: EXCEPTIONS });
              expect(report.checked, `${p.name} at ${width}, ${scheme}: nothing measured`).toBeGreaterThan(0);
              for (const m of report.misses) misses.push(`${width} ${scheme}: ${m.selector} "${m.what}" at ${m.ink}, ${m.delta} px off ${m.column} (${m.x})`);
            } finally {
              await page.context().close();
            }
          }
        expect(misses).toEqual([]);
      } finally {
        for (const [what, params] of RESET) if (p.fake?.some(([w]) => w === what)) await site.fake(what, params);
      }
    }, 120_000);
});

describe("the columns the rule names", () => {
  test("a plugin's Get ends on the page's right edge, the rail's; its button its own width, level with the tile's middle", async () => {
    for (const path of ["/plugins/focus-timer", "/plugins/first-light", "/plugins/worst-case-listing-at-every-limit-xyz12", "/plugins/focus-timer/versions/1.2.0"])
      for (const width of [1440, 1024]) {
        const page = await open({ path }, width, "light");
        try {
          const got = await page.evaluate(() => {
            const box = (q: string) => document.querySelector(q)!.getBoundingClientRect();
            const [get, rail, line, button, tile] = [box(".head .get"), box(".rail"), box(".get .ver"), box(".get .btn"), box(".head > .tile")];
            return { get: get.right, rail: rail.right, line: line.right, button: [button.right, button.width, button.top + button.height / 2], tile: tile.top + tile.height / 2, railWidth: rail.width };
          });
          expect(Math.abs(got.get - got.rail), `${path} at ${width}: Get's right edge`).toBeLessThanOrEqual(1);
          expect(Math.abs(got.button[0]! - got.rail), `${path} at ${width}: the button's right edge`).toBeLessThanOrEqual(1);
          expect(Math.abs(got.line - got.rail), `${path} at ${width}: the line under the button`).toBeLessThanOrEqual(1);
          expect(got.button[1]!, `${path} at ${width}: the button, its own width`).toBeLessThan(got.railWidth * 0.75);
          expect(Math.abs(got.button[2]! - got.tile), `${path} at ${width}: the button level with the tile's middle`).toBeLessThanOrEqual(1);
        } finally {
          await page.context().close();
        }
      }
  }, 120_000);

  test("Deploy tokens: every row's words start on one column, its header's, and its tiles end one gap before it, whatever the token holds", async () => {
    for (const width of [1440, 1024]) {
      const page = await open({ path: "/account/tokens", user: "user_ada" }, width, "dark");
      try {
        const starts = await page.evaluate(() => {
          const first = (el: Element | null) => {
            if (!el) return null;
            const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
            for (let n = walker.nextNode(); n; n = walker.nextNode()) {
              if (!n.textContent!.trim()) continue;
              const r = document.createRange();
              r.setStart(n, n.textContent!.search(/\S/));
              r.setEnd(n, n.textContent!.search(/\S/) + 1);
              return r.getClientRects()[0]?.left ?? null;
            }
            return null;
          };
          const header = document.querySelector(".key.h > .sc");
          const live = [...document.querySelectorAll("ul > li.key:not(.was)")];
          const rows = live.map((row) => first(row.querySelector(".scope > .w, .scope .count")));
          // (Each row's tiles, or the grid of all of yours: where they end, however many.)
          const ends = live.map((row) => row.querySelector(".scope .tiles, .scope > .all")!.getBoundingClientRect().right);
          return { header: header && (header as HTMLElement).offsetParent ? first(header) : null, rows, ends };
        });
        const column = starts.header ?? starts.rows[0]!;
        expect(starts.rows.length).toBeGreaterThan(5);
        for (const [i, x] of starts.rows.entries()) expect(Math.abs(x! - column), `${width}: row ${i + 1}'s words`).toBeLessThanOrEqual(1);
        // The tiles end one gap before the words, one tile or four, or the grid of all of yours.
        const counts = new Set<number>();
        for (const [i, end] of starts.ends.entries()) {
          expect(Math.abs(column - end - 12), `${width}: row ${i + 1}'s tiles end 12 before its words`).toBeLessThanOrEqual(1);
          counts.add(Math.round(end));
        }
        expect(counts.size, `${width}: one place where every row's tiles end`).toBe(1);
      } finally {
        await page.context().close();
      }
    }
  }, 120_000);

  test("Deploy tokens on a phone: each row's tiles start on its name's column, its words on their own line under them, on the same column", async () => {
    for (const width of [390, 768]) {
      const page = await open({ path: "/account/tokens", user: "user_ada" }, width, "dark");
      try {
        const rows = await page.evaluate(() => {
          const first = (el: Element | null) => {
            const walker = document.createTreeWalker(el!, NodeFilter.SHOW_TEXT);
            for (let n = walker.nextNode(); n; n = walker.nextNode()) {
              if (!n.textContent!.trim()) continue;
              const r = document.createRange();
              r.setStart(n, n.textContent!.search(/\S/));
              r.setEnd(n, n.textContent!.search(/\S/) + 1);
              return r.getClientRects()[0]!;
            }
            return null;
          };
          return [...document.querySelectorAll("ul > li.key:not(.was)")].map((row) => {
            const tiles = row.querySelector(".scope .tiles, .scope > .all")!.getBoundingClientRect();
            const words = first(row.querySelector(".scope > .w, .scope .count"))!;
            return { name: first(row.querySelector(".kname .nm"))!.left, tiles: [tiles.left, tiles.bottom], words: [words.left, words.top] };
          });
        });
        expect(rows.length).toBeGreaterThan(5);
        for (const [i, r] of rows.entries()) {
          expect(Math.abs(r.tiles[0]! - r.name), `${width}: row ${i + 1}'s tiles on its name's column`).toBeLessThanOrEqual(1);
          expect(Math.abs(r.words[0]! - r.name), `${width}: row ${i + 1}'s words on its name's column`).toBeLessThanOrEqual(1);
          expect(r.words[1]!, `${width}: row ${i + 1}'s words on their own line, under the tiles`).toBeGreaterThanOrEqual(r.tiles[1]!);
        }
      } finally {
        await page.context().close();
      }
    }
  }, 120_000);

  test("no two items of a group that fill (hover, chosen, focus) touch: 2 px between any two", async () => {
    // (Each group, its items: the gaps within each one.)
    const groups: [string, string][] = [
      ["/plugins", ".catlist|:scope > a"],
      // (The order is a menu now: its choices fill inside its panel, a step apart, the drop's own; the pages, a row.)
      ["/plugins", ".pages|:scope > .pg:not(.gap)"],
      ["/plugins", ".nav|:scope > a"],
      ["/plugins", ".results|:scope > li"],
      ["/plugins", ".feat-row|:scope > li"],
      ["/plugins", ".tags|:scope > li"],
      ["/plugins/focus-timer", ".tabs|:scope > a"],
      ["/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", ".vmini|:scope > li"],
      ["/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", ".flist|:scope > li"],
    ];
    for (const [path, selector] of groups) {
      const page = await open({ path, user: path.startsWith("/admin") ? "user_grace" : undefined }, 1440, "light");
      try {
        const gaps = await page.evaluate((selector) => {
          const [group, item] = selector.split("|") as [string, string];
          const out: number[] = [];
          for (const g of document.querySelectorAll(group)) {
            const items = [...g.querySelectorAll(item)].map((el) => el.getBoundingClientRect()).filter((r) => r.width > 0);
            for (let i = 1; i < items.length; i++) {
              const [a, b] = [items[i - 1]!, items[i]!];
              // (Side by side, the gap across; one under the other, the gap down.)
              out.push(Math.abs(b.top - a.top) < 2 ? b.left - a.right : b.top - a.bottom);
            }
          }
          return out;
        }, selector);
        expect(gaps.length, `${selector} on ${path}`).toBeGreaterThan(0);
        for (const gap of gaps) expect(gap, `${selector} on ${path}`).toBeGreaterThanOrEqual(2);
        // (Focused by keyboard, its ring falls short of the gap: it never meets a neighbour's fill.)
        await page.keyboard.press("Shift");
        const ring = await page.evaluate((selector) => {
          const [group, inner] = selector.split("|") as [string, string];
          const item = document.querySelector(group)!.querySelector(inner)!;
          const target = (item.matches("a, button") ? item : item.querySelector("a, button")) as HTMLElement | null;
          if (!target) return null;
          target.focus();
          const s = getComputedStyle(target);
          return target.matches(":focus-visible") ? parseFloat(s.outlineOffset) + parseFloat(s.outlineWidth) : null;
        }, selector);
        if (ring !== null) expect(ring, `${selector} on ${path}: the focus ring's reach`).toBeLessThan(Math.min(...gaps));
      } finally {
        await page.context().close();
      }
    }
  }, 120_000);
});
