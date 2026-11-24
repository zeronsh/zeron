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

  test("Deploy tokens: what a token can publish stays in its column: every tile and its +n inside it, the column the header's, nothing over the next", async () => {
    for (const width of [1440, 1024, 390]) {
      const page = await open({ path: "/account/tokens", user: "user_ada" }, width, "dark");
      try {
        const got = await page.evaluate(() => {
          const header = document.querySelector<HTMLElement>(".keys .trow.th > :nth-child(2)");
          const column = header && header.offsetParent ? header.getBoundingClientRect() : null;
          return [...document.querySelectorAll(".keys ul > li.trow:not(.was)")].map((row) => {
            const cell = row.querySelector(".c-scope")!.getBoundingClientRect();
            const next = row.querySelector(".c-used")!.getBoundingClientRect();
            const marks = [...row.querySelectorAll(".c-scope :is(.tile, .more)")].filter((el) => (el as HTMLElement).offsetParent).map((el) => el.getBoundingClientRect());
            return { cell: [cell.left, cell.right], next: next.left, top: next.top, cellTop: cell.top, marks: marks.map((m) => [m.left, m.right]), column: column ? [column.left, column.right] : null };
          });
        });
        expect(got.length).toBeGreaterThan(5);
        for (const [i, r] of got.entries()) {
          for (const [left, right] of r.marks) {
            expect(left, `${width}: row ${i + 1}: a tile left of its column`).toBeGreaterThanOrEqual(r.cell[0]! - 1);
            expect(right, `${width}: row ${i + 1}: a tile past its column`).toBeLessThanOrEqual(r.cell[1]! + 1);
            // (On a row's line with the next cell, never reaching it.)
            if (Math.abs(r.top - r.cellTop) < 8) expect(right, `${width}: row ${i + 1}: a tile over Last used`).toBeLessThan(r.next);
          }
          // (Wide, the cell is the header's column: its left edge the header's.)
          if (r.column) expect(Math.abs(r.cell[0]! - r.column[0]!), `${width}: row ${i + 1}'s column`).toBeLessThanOrEqual(1);
        }
        // (Some rows do show tiles: the test isn't empty.)
        expect(got.some((r) => r.marks.length > 0)).toBe(true);
      } finally {
        await page.context().close();
      }
    }
  }, 120_000);

  test("a row of Your plugins or Deploy tokens: hovered, the whole of it fills, edge to edge of the table, its corners round", async () => {
    for (const [path, row] of [["/account", ".mine ul > li:first-child > a.trow"], ["/account/tokens", ".keys ul > li.trow:first-child"]] as const)
      for (const width of [1440, 390]) {
        const page = await open({ path, user: "user_ada" }, width, "light");
        try {
          await page.locator(row).first().hover();
          await page.waitForTimeout(200);
          const got = await page.locator(row).first().evaluate((el) => {
            const table = el.closest(".tbl")!.getBoundingClientRect();
            const box = el.getBoundingClientRect();
            const s = getComputedStyle(el);
            return { fill: s.backgroundColor, radius: s.borderTopLeftRadius, box: [box.left, box.right], table: [table.left, table.right], page: getComputedStyle(document.body).backgroundColor };
          });
          expect(got.fill, `${path} at ${width}: filled`).not.toBe("rgba(0, 0, 0, 0)");
          expect(got.fill, `${path} at ${width}: a tone of its own`).not.toBe(got.page);
          expect(got.radius).toBe("12px");
          expect(Math.abs(got.box[0]! - got.table[0]!), `${path} at ${width}: from the table's left`).toBeLessThanOrEqual(1);
          expect(Math.abs(got.box[1]! - got.table[1]!), `${path} at ${width}: to its right`).toBeLessThanOrEqual(1);
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
