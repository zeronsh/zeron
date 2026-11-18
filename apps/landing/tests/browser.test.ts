// The store's pages in a real browser: no accessibility violations axe can
// find, at a desktop's width and a phone's, and the parts that move work
// from the keyboard (dialogs, the lightbox, Install in Zeron's help).
import AxeBuilder from "@axe-core/playwright";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { startSite, type Site } from "./harness.ts";

let site: Site;
let browser: Browser;
beforeAll(async () => {
  [site, browser] = await Promise.all([startSite(), chromium.launch()]);
}, 60_000);
afterAll(async () => {
  await browser?.close();
  await site?.dispose();
});

async function open(path: string, options: { as?: string; width?: number } = {}): Promise<{ page: Page; context: BrowserContext }> {
  const context = await browser.newContext({ viewport: { width: options.width ?? 1440, height: 900 }, reducedMotion: "reduce" });
  const page = await context.newPage();
  if (options.as) await page.goto(new URL(`/auth/web/callback?as=${options.as}&return_to=/plugins`, site.url).href);
  await page.goto(new URL(path, site.url).href, { waitUntil: "networkidle" });
  return { page, context };
}

async function violations(page: Page) {
  const result = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"]).analyze();
  return result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
}

const PAGES: [string, string | undefined][] = [
  ["/plugins", undefined],
  ["/plugins?q=zzzzqqq", undefined],
  ["/plugins?category=git", undefined],
  ["/plugins/focus-timer", undefined],
  ["/plugins/pr-radar", undefined],
  ["/plugins/xss-probe", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12", undefined],
  ["/plugins/theme-sync/report", "user_kai"],
  ["/account", "user_ada"],
  ["/account/plugins/session-badges", "user_ada"],
  ["/account/plugins/session-badges/versions/1.2.1/withdraw", "user_ada"],
  ["/admin/plugins", "user_grace"],
  ["/admin/plugins/session-badges", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=readme", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=files&path=init.luau", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=changes", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1/reject", "user_grace"],
  ["/admin/reports", "user_grace"],
  ["/admin/audit", "user_grace"],
];

describe("no violations axe can find", () => {
  for (const width of [1440, 390]) {
    for (const [path, as] of PAGES) {
      test(`${path} at ${width}`, async () => {
        const { page, context } = await open(path, { as, width });
        try {
          expect(await violations(page)).toEqual([]);
          // Nothing runs off the side of a phone.
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        } finally {
          await context.close();
        }
      }, 30_000);
    }
  }
});

describe("a package's pictures can't run", () => {
  test("an SVG with script, opened on its own, runs nothing", async () => {
    const { page, context } = await open("/plugins/theme-sync/files/1.0.0/shots/script.svg");
    try {
      await page.waitForTimeout(300);
      expect(await page.evaluate(() => document.documentElement.getAttribute("data-ran"))).toBeNull();
    } finally {
      await context.close();
    }
  });

  test("an HTML page named .png is shown as a (broken) picture, not a page", async () => {
    const { page, context } = await open("/plugins/theme-sync/files/1.0.0/shots/page.png");
    try {
      expect(await page.evaluate(() => document.getElementById("ran"))).toBeNull();
      expect(await page.title()).not.toBe("ran");
    } finally {
      await context.close();
    }
  });

  test("a picture that is also a script won't load as one, even from the site's own page", async () => {
    const { page, context } = await open("/plugins/theme-sync");
    try {
      const outcome = await page.evaluate(
        () =>
          new Promise<string>((resolve) => {
            const script = document.createElement("script");
            script.src = "/plugins/theme-sync/files/1.0.0/shots/polyglot.gif";
            script.onload = () => resolve("loaded");
            script.onerror = () => resolve("refused");
            document.body.append(script);
          }),
      );
      expect(outcome).toBe("refused");
      expect(await page.evaluate(() => (window as { __ran?: string }).__ran)).toBeUndefined();
    } finally {
      await context.close();
    }
  });
});

describe("by keyboard", () => {
  test("a destructive action asks in a dialog that starts on Cancel and closes on Escape, back where it was", async () => {
    const { page, context } = await open("/account/plugins/session-badges", { as: "user_ada" });
    try {
      const opener = page.getByRole("link", { name: "Withdraw" });
      await opener.focus();
      await page.keyboard.press("Enter");
      const dialog = page.getByRole("dialog", { name: "Withdraw 1.2.1 from review?" });
      await expect.poll(() => dialog.isVisible()).toBe(true);
      expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Cancel");
      await page.keyboard.press("Escape");
      await expect.poll(() => dialog.isVisible()).toBe(false);
      expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Withdraw");
    } finally {
      await context.close();
    }
  });

  test("a screenshot opens large; arrows move between them; Escape closes", async () => {
    const { page, context } = await open("/plugins/focus-timer");
    try {
      await page.getByRole("link", { name: "Screenshot 1 of 2, larger" }).focus();
      await page.keyboard.press("Enter");
      const box = page.getByRole("dialog", { name: "Focus timer screenshots" });
      await expect.poll(() => box.isVisible()).toBe(true);
      expect(await box.locator("figcaption").textContent()).toBe("1 of 2");
      await page.keyboard.press("ArrowRight");
      expect(await box.locator("figcaption").textContent()).toBe("2 of 2");
      await page.keyboard.press("Escape");
      await expect.poll(() => box.isVisible()).toBe(false);
    } finally {
      await context.close();
    }
  });

  test("Install in Zeron: when nothing takes the link, what to do opens, and the page stays", async () => {
    const { page, context } = await open("/plugins/focus-timer");
    try {
      const link = page.getByRole("link", { name: "Install in Zeron" });
      expect(await link.getAttribute("href")).toBe("zeron://open/plugin/focus-timer");
      const before = page.url();
      // (Headless Chromium has no app for zeron:, so the link goes nowhere, as for someone without Zeron.)
      await page.evaluate(() => {
        const a = document.querySelector<HTMLAnchorElement>("[data-install-link]")!;
        a.addEventListener("click", (e) => e.preventDefault(), { once: true });
        a.click();
      });
      await expect.poll(() => page.locator("[data-install-help]").evaluate((d) => (d as HTMLDetailsElement).open), { timeout: 4_000 }).toBe(true);
      expect(page.url()).toBe(before);
      expect(await page.getByText("Settings").first().isVisible()).toBe(true);
    } finally {
      await context.close();
    }
  });

  test("the catalog's sort sends its form without a button; the button is there without a script", async () => {
    const { page, context } = await open("/plugins");
    try {
      await page.getByLabel("Sort by").selectOption("new");
      await page.waitForURL(/\/plugins\?sort=new$/);
      expect(await page.getByRole("button", { name: "Search" }).isVisible()).toBe(false);
    } finally {
      await context.close();
    }
    const noScript = await browser.newContext({ javaScriptEnabled: false });
    try {
      const page = await noScript.newPage();
      await page.goto(new URL("/plugins", site.url).href);
      expect(await page.getByRole("button", { name: "Search" }).isVisible()).toBe(true);
    } finally {
      await noScript.close();
    }
  });
});
