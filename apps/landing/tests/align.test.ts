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

async function open(p: Pick<AlignPage, "path" | "user" | "act">, width: number, scheme: "light" | "dark", height = 900): Promise<Page> {
  const context = await browser.newContext({ viewport: { width, height }, colorScheme: scheme, reducedMotion: "reduce" });
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

describe("the bar scrolls away with the page (nothing sticks over what you read)", () => {
  test("on a store page, a plugin's, the guide, your own and the staff's, wide and on a phone", async () => {
    for (const p of [{ path: "/plugins" }, { path: "/plugins/focus-timer" }, { path: "/plugins/publishing" }, { path: "/account", user: "user_ada" }, { path: "/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", user: "user_grace" }])
      for (const width of [1440, 390]) {
        const page = await open(p, width, "dark");
        try {
          await page.evaluate(() => window.scrollTo(0, 600));
          const bar = await page.evaluate(() => { const top = document.querySelector(".top")!; return { position: getComputedStyle(top).position, bottom: top.getBoundingClientRect().bottom }; });
          expect(bar.position, `${p.path} at ${width}`).not.toMatch(/sticky|fixed/);
          expect(bar.bottom, `${p.path} at ${width}: scrolled off`).toBeLessThanOrEqual(0);
        } finally {
          await page.context().close();
        }
      }
  }, 120_000);
});

describe("every icon is drawn at an icon's size", () => {
  // (An icon whose size went with its rule draws as the browser's default, a black square over the page.)
  for (const p of ALIGN_PAGES)
    test(p.name, async () => {
      for (const [what, params] of p.fake ?? []) await site.fake(what, params);
      try {
        for (const width of [1440, 390]) {
          const page = await open(p, width, "dark");
          try {
            const big = await page.evaluate(() =>
              [...document.querySelectorAll('svg:has(> use[href^="#i-"])')]
                .filter((el) => (el as unknown as HTMLElement).checkVisibility?.() ?? true)
                .map((el) => [el.getBoundingClientRect(), el.parentElement] as const)
                .filter(([r]) => r.width > 32 || r.height > 32)
                .map(([r, parent]) => `${parent?.tagName.toLowerCase()}.${parent?.className}: ${Math.round(r.width)}x${Math.round(r.height)}`),
            );
            expect(big, `${p.name} at ${width}`).toEqual([]);
          } finally {
            await page.context().close();
          }
        }
      } finally {
        for (const [what, params] of RESET) if (p.fake?.some(([w]) => w === what)) await site.fake(what, params);
      }
    }, 60_000);
});

describe("what's empty, and what to mind", () => {
  /** Every page of the look, at a desktop's width and a phone's. */
  const everywhere = (what: string, run: (page: Page, where: string) => Promise<void>) => {
    for (const p of ALIGN_PAGES)
      test(`${what}: ${p.name}`, async () => {
        for (const [w, params] of p.fake ?? []) await site.fake(w, params);
        try {
          for (const width of [1440, 390]) {
            const page = await open(p, width, "dark");
            try {
              await run(page, `${p.name} at ${width}`);
            } finally {
              await page.context().close();
            }
          }
        } finally {
          for (const [w, params] of RESET) if (p.fake?.some(([x]) => x === w)) await site.fake(w, params);
        }
      }, 60_000);
  };

  everywhere("a callout spans its column's box width, its words on the column's two edges", async (page, where) => {
    const off = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>(".flash, .warnline, .said-was")]
        .filter((el) => el.checkVisibility())
        .map((el) => {
          const s = getComputedStyle(el), parent = el.parentElement!, ps = getComputedStyle(parent), [b, pb] = [el.getBoundingClientRect(), parent.getBoundingClientRect()];
          const left = b.left + parseFloat(s.paddingLeft) - (pb.left + parseFloat(ps.paddingLeft)), right = pb.right - parseFloat(ps.paddingRight) - (b.right - parseFloat(s.paddingRight));
          return { what: `${el.className}: ${el.textContent!.trim().slice(0, 30)}`, left: Math.round(left), right: Math.round(right) };
        })
        .filter((c) => Math.abs(c.left) > 1 || Math.abs(c.right) > 1),
    );
    expect(off, where).toEqual([]);
  });

  everywhere("no count of 0 in a tab, a sidebar or a heading", async (page, where) => {
    const zeros = await page.evaluate(() => [...document.querySelectorAll(":is(.sub, .tabs, .staff-tabs, .facets, .rhead, h1, h2, h3) .num")].filter((el) => el.textContent!.trim() === "0").map((el) => el.parentElement!.textContent!.trim()));
    expect(zeros, where).toEqual([]);
  });

  everywhere("an empty block is centred in its area, 96 above and below; one action at most", async (page, where) => {
    const blocks = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>(".empty")].map((el) => {
        const b = el.getBoundingClientRect(), icon = el.querySelector(".empty-i")!.getBoundingClientRect(), p = el.parentElement!.getBoundingClientRect(), ps = getComputedStyle(el.parentElement!);
        const last = el.lastElementChild!.getBoundingClientRect(), line = el.querySelector("p")?.getBoundingClientRect().width ?? 0;
        const before = el.previousElementSibling?.getBoundingClientRect(), next = el.nextElementSibling?.getBoundingClientRect(), wrap = el.closest(".wrap")!;
        return {
          middle: Math.round(icon.left + icon.width / 2 - (p.left + parseFloat(ps.paddingLeft) + p.right - parseFloat(ps.paddingRight)) / 2),
          above: Math.round(icon.top - b.top), below: Math.round(b.bottom - last.bottom),
          // (Nothing adds to its step: what's before it ends where it starts; ending the page, so does the page, and the foot keeps no step of its own.)
          gap: before && before.height > 1 && before.bottom <= b.top + 1 ? Math.round(b.top - before.bottom) : 0, after: next && next.height > 1 ? Math.round(next.top - b.bottom) : 0,
          foot: el.parentElement === wrap && el === wrap.lastElementChild ? Math.round(wrap.getBoundingClientRect().bottom - b.bottom) + parseFloat(getComputedStyle(document.querySelector(".foot")!).marginTop) : 0,
          tile: [icon.width, icon.height], line, actions: el.querySelectorAll("a, button").length,
        };
      }),
    );
    // (The pages with nothing to show do show it.)
    if (/empty|down|signed-out|no-plugins|^store-none|^tokens-none$/.test(where.split(" at ")[0]!)) expect(blocks.length, where).toBe(1);
    for (const b of blocks) {
      expect({ above: b.above, below: b.below, tile: b.tile, gap: b.gap, after: b.after, foot: b.foot }, where).toEqual({ above: 96, below: 96, tile: [40, 40], gap: 0, after: 0, foot: 0 });
      expect(Math.abs(b.middle), where).toBeLessThanOrEqual(1);
      expect(b.line, where).toBeLessThanOrEqual(440);
      expect(b.actions, where).toBeLessThanOrEqual(1);
    }
  });

  test("New token: 32 from its title to its first field, on its page and in its dialog", async () => {
    for (const path of ["/account/tokens/new", "/account/tokens"])
      for (const width of [1440, 390]) {
        const page = await open({ path, user: "user_ada" }, width, "dark");
        try {
          if (path.endsWith("tokens")) await page.getByRole("link", { name: "New token" }).click();
          const gap = await page.evaluate(() => {
            const form = [...document.querySelectorAll<HTMLElement>(".tokform")].find((f) => f.checkVisibility())!;
            return form.querySelector(".field-label")!.getBoundingClientRect().top - form.parentElement!.querySelector("h1, h2")!.getBoundingClientRect().bottom;
          });
          expect(Math.round(gap), `${path} at ${width}`).toBe(32);
        } finally {
          await page.context().close();
        }
      }
  }, 60_000);

  test("a row's second line is at most 720 wide", async () => {
    for (const p of [{ path: "/account/plugins/worst-case-listing-at-every-limit-xyz12", user: "user_ada" }, { path: "/admin/plugins/worst-case-listing-at-every-limit-xyz12", user: "user_grace" }, { path: "/account", user: "user_ada" }]) {
      const page = await open(p, 1728, "dark");
      try {
        const widths = await page.evaluate(() => [...document.querySelectorAll(".c-note")].map((el) => el.getBoundingClientRect().width));
        expect(widths.length, p.path).toBeGreaterThan(0);
        for (const w of widths) expect(w, p.path).toBeLessThanOrEqual(720);
      } finally {
        await page.context().close();
      }
    }
  }, 60_000);
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

  test("the staff's tables: no pill in a row sits outside its cell, nor past the row", async () => {
    const pages = ["/admin/plugins", "/admin/plugins?state=declined", "/admin/plugins?state=taken_down", "/admin/reports", "/admin/reports?status=actioned", "/admin/reservations", "/admin/audit", "/account", "/account/tokens"];
    for (const path of pages)
      for (const width of [1440, 1024, 390]) {
        const page = await open({ path, user: path.startsWith("/admin") ? "user_grace" : "user_ada" }, width, "dark");
        try {
          const out = await page.evaluate(() => {
            const misses: string[] = [];
            let pills = 0;
            for (const row of document.querySelectorAll(".tbl .trow:not(.th)")) {
              const box = row.getBoundingClientRect();
              for (const pill of row.querySelectorAll(".state")) {
                if (!(pill as HTMLElement).offsetParent) continue;
                pills++;
                // (Its cell: the row's own child that holds it.)
                let cell = pill as Element;
                while (cell.parentElement && cell.parentElement !== row && getComputedStyle(cell.parentElement).display !== "contents") cell = cell.parentElement;
                const [p, c] = [pill.getBoundingClientRect(), cell.getBoundingClientRect()];
                const name = `${(row.textContent ?? "").trim().slice(0, 30)} — ${(pill.textContent ?? "").trim()}`;
                // (A pill that starts its cell's line hangs left by its own padding, its words on the column: the store's way.
                // Past that, or past its cell's right edge into the next, it's out.)
                const hang = parseFloat(getComputedStyle(pill).paddingLeft) || 0;
                if (p.left < c.left - hang - 1 || p.right > c.right + 1) misses.push(`${name}: outside its cell (${Math.round(p.left)}–${Math.round(p.right)} in ${Math.round(c.left)}–${Math.round(c.right)})`);
                if (p.left < box.left || p.right > box.right) misses.push(`${name}: past the row`);
              }
            }
            return { misses, pills };
          });
          expect(out.misses, `${path} at ${width}`).toEqual([]);
        } finally {
          await page.context().close();
        }
      }
  }, 240_000);

  test("Review: the decision stays in view as the page scrolls, a step from the window's top, at 1728 by 1117", async () => {
    const page = await open({ path: "/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", user: "user_grace" }, 1728, "dark", 1117);
    try {
      const height = await page.evaluate(() => document.documentElement.scrollHeight);
      expect(height).toBeGreaterThan(2 * 1117);
      for (let y = 0; y < height; y += 400) {
        await page.evaluate((y) => window.scrollTo(0, y), y);
        const box = await page.locator("#decide").evaluate((el) => { const r = el.getBoundingClientRect(); return [r.top, r.bottom]; });
        // (The bar scrolls away: the card sticks 32 from the window's top, once the page is past where it starts.)
        expect(box[0]!, `at ${y}: its top, a step from the window's`).toBeGreaterThanOrEqual(31);
        expect(box[1]!, `at ${y}: its bottom, on screen`).toBeLessThanOrEqual(1117);
      }
    } finally {
      await page.context().close();
    }
  }, 60_000);

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
      // (Review's versions are a table now, as your own pages' are: rows a rule apart, only the hovered one filled.)
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
