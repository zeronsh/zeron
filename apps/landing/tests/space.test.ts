// One spacing scale, by proximity (styles/look/store.css :root): 4, 8, 12, 16, 24, 32, 48, 64.
// 1. Every margin, padding and gap of the look (and every custom property that holds one) is written in its
//    steps: a token, 0, auto, or a calc of tokens (a size plus a step is allowed: a tile's width and a gap).
// 2. On every page of the look, in its states, at a desktop's width and a phone's, light and dark: each gap
//    between blocks stacked one above another is a step of the scale, and gaps grow outward (what belongs
//    together sits closer than what doesn't): no block sits further from its neighbours inside it than it sits
//    from its own.
import { readdirSync, readFileSync } from "node:fs";
import { chromium, type Browser } from "playwright";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { ALIGN_PAGES, RESET } from "./align/pages.ts";
import { startSite, type Site } from "./harness.ts";

const SCALE = [0, 4, 8, 12, 16, 24, 32, 48, 64];
const LOOK = new URL("../src/styles/look/", import.meta.url);
const SPACING = /(?<![\w-])(margin(?:-[a-z-]+)?|padding(?:-[a-z-]+)?|gap|row-gap|column-gap|--(?:pad|gap|pill-gap|rail-gap|get-hang|gutter|card))\s*:\s*([^;}]+)/g;

describe("the look's spacing is the scale's", () => {
  test("every margin, padding and gap in styles/look is a step of the scale, as a token", () => {
    const off: string[] = [];
    for (const file of readdirSync(LOOK).filter((f) => f.endsWith(".css"))) {
      const css = readFileSync(new URL(file, LOOK), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
      for (const m of css.matchAll(SPACING)) {
        const [, prop, value] = m as unknown as [string, string, string];
        // (Inside a calc, a px literal is a size added to a step: allowed only beside a token.)
        const bare = value.replace(/calc\(([^()]|\([^()]*\))*\)/g, (calc) => (/var\(--s\d+\)|var\(--(in|label|items|head|groups|title|sections|pad|gap|gutter|card|rail-gap|get-hang|pill-gap)\)/.test(calc) ? "" : calc));
        for (const px of bare.matchAll(/(-?\d+(?:\.\d+)?)px/g)) off.push(`${file}: ${prop}: ${value.trim()} (${px[0]})`);
      }
    }
    expect(off).toEqual([]);
  });
});

let site: Site;
let browser: Browser;
beforeAll(async () => {
  [site, browser] = await Promise.all([startSite(), chromium.launch()]);
}, 60_000);
afterAll(async () => {
  await browser?.close();
  await site?.dispose();
});

/** In the page: every block in <main> whose children stack, each gap between them, and whether gaps grow outward. */
function measure(scale: number[]) {
  const shown = (el: Element) => {
    const s = getComputedStyle(el);
    const r = el.getBoundingClientRect();
    return s.display !== "none" && s.visibility !== "hidden" && s.position !== "absolute" && s.position !== "fixed" && r.height > 0 && r.width > 0 && !el.closest("dialog:not([open]), [popover]:not(:popover-open), .sr-only");
  };
  const blocky = (el: Element) => /^(block|flex|grid|list-item|table|flow-root)$/.test(getComputedStyle(el).display);
  const name = (el: Element) => `${el.tagName.toLowerCase()}${el.id ? `#${el.id}` : ""}${[...el.classList].slice(0, 2).map((c) => `.${c}`).join("")}`;
  const path = (el: Element) => {
    const out: string[] = [];
    for (let e: Element | null = el; e && e.tagName !== "MAIN" && out.length < 4; e = e.parentElement) out.unshift(name(e));
    return out.join(" › ");
  };
  /** The vertical gaps between an element's children that stack: each child's gap to the lowest one above it (in a grid, whatever their order in the markup). */
  const stacked = (parent: Element) => {
    // (A box drawn as its children, display: contents, is looked through: its children are the parent's.)
    const flat = (el: Element): Element[] => [...el.children].flatMap((k) => (getComputedStyle(k).display === "contents" ? flat(k) : [k]));
    const kids = flat(parent).filter((k) => shown(k) && blocky(k));
    const boxes = kids.map((k) => k.getBoundingClientRect());
    const gaps: { gap: number; a: Element; b: Element }[] = [];
    for (let i = 0; i < kids.length; i++) {
      const b = boxes[i]!;
      let above = -1;
      for (let j = 0; j < kids.length; j++) {
        if (j === i) continue;
        const a = boxes[j]!;
        // (Above it, overlapping across: a stack, not a row.)
        if (a.bottom <= b.top + 0.5 && Math.min(a.right, b.right) - Math.max(a.left, b.left) > 0 && (above < 0 || a.bottom > boxes[above]!.bottom)) above = j;
      }
      if (above >= 0) gaps.push({ gap: Math.round(b.top - boxes[above]!.bottom), a: kids[above]!, b: kids[i]! });
    }
    return gaps;
  };
  /** A head: what it heads sits closer to it than the heads' content's own parts sit to each other (a title, a section's head, a lead, crumbs). */
  const head = (el: Element) => el.matches("h1, h2, h3, h4, h5, h6, header, legend, label, .sec-head, .hd, .lead, .lead2, .crumbs, .field-label, .facet-h, .rail-h");
  const off: string[] = [];
  const inverted: string[] = [];
  const main = document.querySelector("main");
  if (!main) return { off, inverted, checked: 0 };
  let checked = 0;
  /** The largest gap inside an element, among its descendants' stacks. */
  const inner = new Map<Element, number>();
  const walk = (el: Element): number => {
    let most = 0;
    for (const { gap, a, b } of stacked(el)) {
      checked++;
      if (!scale.some((s) => Math.abs(s - gap) <= 1)) off.push(`${path(b)}: ${gap} px after ${name(a)}`);
      most = Math.max(most, gap);
    }
    for (const k of el.children) if (shown(k)) most = Math.max(most, walk(k));
    inner.set(el, most);
    return most;
  };
  walk(main);
  // Gaps grow outward: a block's own gap to the block before it is no smaller than any gap inside either.
  const order = (el: Element) => {
    for (const { gap, a, b } of stacked(el)) {
      const within = Math.max(inner.get(a) ?? 0, inner.get(b) ?? 0);
      if (gap > 0 && !head(a) && gap + 1 < within) inverted.push(`${path(b)}: ${gap} px from ${name(a)}, but ${within} px inside`);
    }
    for (const k of el.children) if (shown(k)) order(k);
  };
  order(main);
  return { off, inverted, checked };
}

describe("on every page, gaps between blocks are steps of the scale, and grow outward", () => {
  for (const p of ALIGN_PAGES)
    test(p.name, async () => {
      for (const [what, params] of p.fake ?? []) await site.fake(what, params);
      try {
        const found: string[] = [];
        for (const width of [1440, 390])
          for (const scheme of ["light", "dark"] as const) {
            const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: scheme, reducedMotion: "reduce" });
            const page = await context.newPage();
            try {
              if (p.user) await page.goto(new URL(`/auth/web/callback?as=${p.user}&return_to=/plugins`, site.url).href);
              await page.goto(new URL(p.path, site.url).href, { waitUntil: "networkidle" });
              if (p.act) await p.act(page);
              await page.evaluate(() => document.fonts.ready);
              const r = await page.evaluate(measure, SCALE);
              expect(r.checked, `${p.name} at ${width}: nothing measured`).toBeGreaterThan(0);
              for (const o of r.off) found.push(`${width} ${scheme}: off the scale: ${o}`);
              for (const o of r.inverted) found.push(`${width} ${scheme}: closer outside than in: ${o}`);
            } finally {
              await context.close();
            }
          }
        expect([...new Set(found)]).toEqual([]);
      } finally {
        for (const [what, params] of RESET) if (p.fake?.some(([w]) => w === what)) await site.fake(what, params);
      }
    }, 120_000);
});
