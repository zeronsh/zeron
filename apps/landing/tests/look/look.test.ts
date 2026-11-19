// The store's own look, in a real browser, held to what its bar asks
// (the coordinator's): nothing clips or scrolls the page sideways at a
// desktop's, a laptop's, a tablet's and a phone's width; the keyboard goes
// through a page in its reading order, every stop seen and ringed; "/" and
// the phone's menu work. (Axe, both schemes, is tests/browser.test.ts.)
import { chromium, type Browser, type Page } from "playwright";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { startSite, type Site } from "../harness.ts";

let site: Site;
let browser: Browser;
beforeAll(async () => {
  [site, browser] = await Promise.all([startSite(), chromium.launch()]);
}, 60_000);
afterAll(async () => {
  await browser?.close();
  await site?.dispose();
});

async function open(path: string, width: number, scheme: "light" | "dark" = "light", user?: string): Promise<Page> {
  const context = await browser.newContext({ viewport: { width, height: 900 }, colorScheme: scheme, reducedMotion: "reduce" });
  const page = await context.newPage();
  if (user) await page.goto(new URL(`/auth/web/callback?as=${user}&return_to=/plugins`, site.url).href);
  await page.goto(new URL(path, site.url).href, { waitUntil: "networkidle" });
  return page;
}

/** What reaches past the page's right edge, innermost first (none, on a page that fits). */
const overflowing = (page: Page) =>
  page.evaluate(() => {
    if (document.documentElement.scrollWidth <= window.innerWidth) return [];
    // (Text past its own box shows in no box's rect: named by its element.)
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    const texts: string[] = [];
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const range = document.createRange();
      range.selectNodeContents(n);
      if ([...range.getClientRects()].some((r) => r.right > window.innerWidth + 0.5) && !n.parentElement?.closest(".cats, .stage, .tabs, pre, .table-scroll, .crumbs")) texts.push(`text in ${n.parentElement?.tagName.toLowerCase()}.${[...(n.parentElement?.classList ?? [])].join(".")}`);
    }
    const wide = [...document.querySelectorAll("body *")].filter((e) => e.getBoundingClientRect().right > window.innerWidth + 0.5 && !e.closest(".cats, .stage, .tabs, pre, .table-scroll"));
    const boxes = wide.filter((e) => !wide.some((o) => o !== e && e.contains(o))).map((e) => `${e.tagName.toLowerCase()}.${[...e.classList].join(".")}`);
    return [...boxes, ...texts, ...(boxes.length + texts.length === 0 ? [`the page, ${document.documentElement.scrollWidth} wide`] : [])];
  });

const PAGES = [
  "/plugins",
  "/plugins/focus-timer",
  "/plugins/worst-case-listing-at-every-limit-xyz12",
  "/plugins/worst-case-listing-at-every-limit-xyz12/versions",
  "/plugins/worst-case-listing-at-every-limit-xyz12/asks",
  "/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.4.0",
  "/plugins/focus-timer/versions/1.1.0",
  "/plugins/clipboard-sync",
  "/plugins/all",
  "/plugins/all?category=developer-tools",
  "/plugins/all?q=git",
  "/plugins/all?keyword=eywordthatisthirtytwocharacters0",
  "/plugins/all?q=zzzzqqq",
];

/** Your own pages, signed in as Ada (every state of a plugin and a key) and as no one. */
const YOURS: [string, string | undefined][] = [["/account", "user_ada"], ["/account/keys", "user_ada"], ["/account", undefined], ["/account/keys", "user_noor"], ["/account/keys/Qm81LzR2sT3uV4wX5yZ6aB7cD8eF9gH0iJ1kL2mN3oP", "user_ada"]];

describe("nothing runs off the side", () => {
  for (const [path, user] of [...PAGES.map((p): [string, string | undefined] => [p, undefined]), ...YOURS]) {
    for (const width of [1440, 1024, 768, 390]) {
      test(`${path}${user ? ` as ${user}` : ""} at ${width}`, async () => {
        const page = await open(path, width, "light", user);
        try {
          expect(await overflowing(page)).toEqual([]);
        } finally {
          await page.context().close();
        }
      });
    }
  }
});

describe("by keyboard", () => {
  test.each(["/plugins", "/plugins/focus-timer", "/plugins/worst-case-listing-at-every-limit-xyz12/versions", "/plugins/all?category=developer-tools"])("Tab goes through %s in its reading order, each stop on screen and ringed", async (path) => {
    const page = await open(path, 1440);
    try {
      const stops: { tag: string; top: number; left: number; ring: string; label: string }[] = [];
      for (let i = 0; i < 40; i++) {
        await page.keyboard.press("Tab");
        const stop = await page.evaluate(() => {
          const el = document.activeElement as HTMLElement;
          // (Past the last stop, focus leaves the page.)
          if (!el || el === document.body) return null;
          el.scrollIntoView({ block: "nearest" });
          const box = el.getBoundingClientRect();
          const style = getComputedStyle(el);
          // (A search field's ring is its box's: the field sits inside it, edge to edge.)
          const field = el.matches("input") && el.parentElement ? getComputedStyle(el.parentElement).boxShadow.includes("0px 0px 0px 4px") : false;
          return { tag: el.tagName.toLowerCase(), top: box.top + scrollY, left: box.left, ring: field ? "solid" : style.outlineStyle, label: (el.getAttribute("aria-label") ?? el.textContent ?? "").trim().slice(0, 40), w: box.width, h: box.height };
        });
        if (!stop) break;
        expect(stop.w * stop.h, stop.label).toBeGreaterThan(0);
        stops.push(stop);
      }
      // (The skip link first, then the bar, then the page.)
      expect(stops[0]!.label).toBe("Skip to the page");
      expect(stops[1]!.tag).toBe("a");
      expect(stops.slice(2, 7).map((s) => s.label)).toEqual(["Discover", "Browse", "Publish", "Search plugins", "Sign in"]);
      // Every stop shows the ring.
      for (const s of stops) expect(s.ring, s.label).toBe("solid");
      // Down the page: a later stop is never above an earlier one's row (a row's stops go left to right).
      const page_ = stops.slice(8);
      for (let i = 1; i < page_.length; i++) {
        const [a, b] = [page_[i - 1]!, page_[i]!];
        expect(b.top >= a.top - 4 || b.left > a.left, `${a.label} → ${b.label}`).toBe(true);
      }
    } finally {
      await page.context().close();
    }
  });

  test('"/" puts you in Discover\'s own search; Escape leaves it', async () => {
    const page = await open("/plugins", 1440);
    try {
      await page.keyboard.press("/");
      expect(await page.evaluate(() => document.activeElement?.hasAttribute("data-page-search"))).toBe(true);
      await page.keyboard.press("Escape");
      expect(await page.evaluate(() => document.activeElement === document.body)).toBe(true);
    } finally {
      await page.context().close();
    }
  });

  test("on a phone, the menu opens over the page with the store's places, and Escape closes it", async () => {
    const page = await open("/plugins", 390);
    try {
      await page.getByRole("button", { name: "Menu" }).click();
      const menu = page.locator("#nav-menu");
      await expect.poll(() => menu.isVisible()).toBe(true);
      expect(await menu.getByRole("link").allTextContents()).toEqual(expect.arrayContaining(["Discover", "Browse", "Publish", "Publishing guide", "Sign in"]));
      await page.keyboard.press("Escape");
      await expect.poll(() => menu.isVisible()).toBe(false);
    } finally {
      await page.context().close();
    }
  });
});
