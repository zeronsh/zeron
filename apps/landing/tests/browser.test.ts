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

/** The person's setting: the shell follows it (styles/app.css), so each is checked. */
type Scheme = "dark" | "light";

async function open(path: string, options: { as?: string; width?: number; scheme?: Scheme } = {}): Promise<{ page: Page; context: BrowserContext }> {
  const context = await browser.newContext({ viewport: { width: options.width ?? 1440, height: 900 }, reducedMotion: "reduce", colorScheme: options.scheme ?? "dark" });
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
  ["/plugins/all", undefined],
  ["/plugins/all?q=zzzzqqq", undefined],
  ["/plugins/all?category=git", undefined],
  ["/plugins/focus-timer", undefined],
  ["/plugins/pr-radar", undefined],
  ["/plugins/xss-probe", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12", undefined],
  ["/plugins/all?q=git", undefined],
  ["/plugins/theme-sync/report", "user_kai"],
  ["/plugins/publishing", undefined],
  ["/plugins/no-such-plugin", undefined],
  ["/account", "user_ada"],
  ["/account", "user_lin"],
  ["/account/plugins/session-badges", "user_ada"],
  ["/account/plugins/focus-timer", "user_ada"],
  ["/account/plugins/session-badges/versions/1.2.1/yank", "user_ada"],
  ["/account/keys", "user_ada"],
  ["/plugins/all?verified=1", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12/versions", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.4.0", undefined],
  ["/admin/plugins", "user_grace"],
  ["/admin/plugins/session-badges", "user_grace"],
  ["/admin/plugins/crypto-ticker", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1", "user_grace"],
  ["/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", "user_grace"],
  ["/admin/plugins/voice-memos/0.1.0", "user_grace"],
  ["/admin/plugins/focus-timer/1.2.0", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1/remove", "user_grace"],
  ["/admin/reservations/linear", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=files", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=files&path=init.luau", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1?view=changes", "user_grace"],
  ["/admin/plugins/xss-probe/1.1.0?view=changes", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1/decline", "user_grace"],
  ["/admin/plugins/session-badges/1.2.1/verify", "user_grace"],
  ["/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.2.0/restore", "user_grace"],
  ["/admin/plugins?state=all", "user_grace"],
  ["/admin/plugins/session-badges/actions/suspend", "user_grace"],
  ["/admin/plugins/session-badges/actions/transfer", "user_grace"],
  ["/admin/reports", "user_grace"],
  ["/admin/reports?status=actioned", "user_grace"],
  ["/admin/reservations", "user_grace"],
  ["/admin/audit", "user_grace"],
];

/** The states a page opens into: each must be as clean as the page. */
const OPENED: [string, string, string | undefined, (page: Page) => Promise<void>][] = [
  ["Verify, asked once", "/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", "user_grace", async (page) => {
    // (Exact: "Don't verify…" holds the words too, and comes first in a phone's bar.)
    await page.getByRole("link", { name: "Verify…", exact: true }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
    expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Verify");
  }],
  ["Don't verify, written", "/admin/plugins/session-badges/1.2.1", "user_grace", async (page) => {
    await page.getByRole("link", { name: "Don't verify…" }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
    await page.getByRole("dialog").getByLabel("Reason").selectOption("listing");
    await page.getByRole("dialog").getByLabel("For the author").fill("Say what each panel shows.");
  }],
  ["Take down", "/admin/plugins/focus-timer/1.2.0", "user_grace", async (page) => {
    await page.getByRole("link", { name: "Take down…" }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["closing a report", "/admin/reports", "user_grace", async (page) => {
    await page.getByRole("link", { name: "Close it…" }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["Remove", "/admin/plugins/focus-timer/1.2.0", "user_grace", async (page) => {
    await page.getByRole("link", { name: "Remove…" }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["the new key sheet", "/account/keys", "user_ada", async (page) => {
    await page.locator("[data-dialog='new-key']").first().click();
    await expect.poll(() => page.locator("dialog#new-key").isVisible()).toBe(true);
  }],
  ["the stop new installs dialog", "/account/plugins/session-badges", "user_ada", async (page) => {
    await page.getByRole("link", { name: "Stop new installs" }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["the screenshots", "/plugins/focus-timer", undefined, async (page) => {
    await page.getByRole("link", { name: /Picture 1 of/ }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["Install in Zeron's help", "/plugins/focus-timer", undefined, async (page) => {
    await page.evaluate(() => {
      const a = document.querySelector<HTMLAnchorElement>("[data-install-link]")!;
      a.addEventListener("click", (e) => e.preventDefault(), { once: true });
      a.click();
    });
    await expect.poll(() => page.locator("[data-install-help]").evaluate((d) => (d as HTMLDetailsElement).open), { timeout: 4_000 }).toBe(true);
  }],
];

describe("no violations axe can find", () => {
  for (const scheme of ["dark", "light"] as const)
  for (const width of [1440, 390]) {
    for (const [path, as] of PAGES) {
      test(`${path} at ${width}, ${scheme}`, async () => {
        const { page, context } = await open(path, { as, width, scheme });
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

describe("no violations axe can find, opened", () => {
  for (const scheme of ["dark", "light"] as const)
  for (const width of [1440, 390]) {
    for (const [what, path, as, opening] of OPENED) {
      test(`${what} at ${width}, ${scheme}`, async () => {
        const { page, context } = await open(path, { as, width, scheme });
        try {
          await opening(page);
          expect(await violations(page)).toEqual([]);
          expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        } finally {
          await context.close();
        }
      }, 30_000);
    }
  }
});

describe("a package's pictures can't run", () => {
  test("a plugin's icon with script, opened on its own in a tab, runs nothing", async () => {
    const { page, context } = await open("/plugins/xss-probe/files/1.0.0/icon.svg");
    try {
      await page.waitForTimeout(300);
      expect(await page.evaluate(() => document.documentElement.getAttribute("data-ran"))).toBeNull();
    } finally {
      await context.close();
    }
  });

  test("an SVG with script, opened on its own, runs nothing", async () => {
    const { page, context } = await open("/plugins/theme-sync/files/1.0.0/shots/script.svg");
    try {
      await page.waitForTimeout(300);
      expect(await page.evaluate(() => document.documentElement.getAttribute("data-ran"))).toBeNull();
    } finally {
      await context.close();
    }
  });

  test("an HTML page named .png, which the edge gives only as a download, isn't shown at all", async () => {
    const { page, context } = await open("/plugins/theme-sync/files/1.0.0/shots/page.png");
    try {
      expect(await page.evaluate(() => document.getElementById("ran"))).toBeNull();
      expect(await page.title()).not.toBe("ran");
    } finally {
      await context.close();
    }
  });

  test("a picture won't load as a script, even from the site's own page: its type forbids it", async () => {
    const { page, context } = await open("/plugins/theme-sync");
    const refusals: string[] = [];
    page.on("console", (message) => refusals.push(message.text()));
    try {
      const outcome = await page.evaluate(
        () =>
          new Promise<string>((resolve) => {
            const script = document.createElement("script");
            script.src = "/plugins/theme-sync/files/1.0.0/shots/script.svg";
            script.onload = () => resolve("loaded");
            script.onerror = () => resolve("refused");
            document.body.append(script);
          }),
      );
      expect(outcome).toBe("refused");
      // (Refused for what it is, not because it's missing.)
      expect(refusals.join("\n")).toMatch(/MIME type \('image\/svg\+xml'\) is not executable/);
      expect(await page.evaluate(() => document.documentElement.getAttribute("data-ran"))).toBeNull();
    } finally {
      await context.close();
    }
  });
});

describe("an author's words stay in their place", () => {
  test("a right-to-left override in a name reorders nothing after it", async () => {
    const { page, context } = await open("/admin/plugins/xss-probe/1.1.0", { as: "user_grace" });
    try {
      const [name, version] = await Promise.all([page.locator("h1 > span").first().boundingBox(), page.locator("h1 > .v").boundingBox()]);
      expect(version!.x).toBeGreaterThanOrEqual(name!.x + name!.width - 1);
    } finally {
      await context.close();
    }
  });
});

describe("who you are, in the bar of a public page", () => {
  test("signed in, the bar has your name; its menu, Sign out with the session's token; Review only for staff", async () => {
    const { page, context } = await open("/plugins", { as: "user_ada" });
    try {
      const me = page.locator(".top button[data-me]");
      await expect.poll(() => me.isVisible()).toBe(true);
      expect(await me.locator("[data-handle]").textContent()).toBe("@ada");
      expect(await page.locator(".top [data-signin]").isVisible()).toBe(false);
      expect(await page.locator(".top .nav a[data-staff]").isVisible()).toBe(false);
      await me.click();
      const signOut = page.locator("#me-menu form[data-signout]");
      await expect.poll(() => signOut.isVisible()).toBe(true);
      expect(await signOut.locator("input[name=csrf]").inputValue()).not.toBe("");
    } finally {
      await context.close();
    }
    const staff = await open("/plugins", { as: "user_grace" });
    try {
      await expect.poll(() => staff.page.locator(".top .nav a[data-staff]").isVisible()).toBe(true);
    } finally {
      await staff.context.close();
    }
  });

  test("signed out, it says Sign in, and no name and no Sign out", async () => {
    const { page, context } = await open("/plugins");
    try {
      await page.waitForLoadState("networkidle");
      expect(await page.locator(".top [data-signin]").isVisible()).toBe(true);
      expect(await page.locator(".top button[data-me]").isVisible()).toBe(false);
      expect(await page.locator("form[data-signout]").first().isVisible()).toBe(false);
    } finally {
      await context.close();
    }
  });
});

describe("by keyboard", () => {
  test("a destructive action asks in a dialog that starts on Cancel and closes on Escape, back where it was", async () => {
    const { page, context } = await open("/account/plugins/session-badges", { as: "user_ada" });
    try {
      const opener = page.getByRole("link", { name: "Stop new installs" }).first();
      await opener.focus();
      await page.keyboard.press("Enter");
      const dialog = page.getByRole("dialog", { name: "Stop new installs of Session badges 1.2.1?" });
      await expect.poll(() => dialog.isVisible()).toBe(true);
      expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Cancel");
      await page.keyboard.press("Escape");
      await expect.poll(() => dialog.isVisible()).toBe(false);
      expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Stop new installs");
    } finally {
      await context.close();
    }
  });

  test("a screenshot opens large; arrows move between them; Escape closes", async () => {
    const { page, context } = await open("/plugins/focus-timer");
    try {
      await page.getByRole("link", { name: "Picture 1 of 2, larger" }).focus();
      await page.keyboard.press("Enter");
      const box = page.getByRole("dialog", { name: "Focus timer, pictures" });
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

  test("All plugins: the sort and Next are links; while the next page comes, its rows' places are held", async () => {
    const { page, context } = await open("/plugins/all");
    try {
      // (The click and what it does at once, read before the next page can arrive.)
      const now = await page.evaluate(() => {
        const rows = document.querySelectorAll(".plugins .rows tr").length;
        const rowHeight = document.querySelector(".plugins .rows tr")!.getBoundingClientRect().height;
        [...document.querySelectorAll<HTMLAnchorElement>(".segmented a")].find((a) => a.textContent === "Newest")!.click();
        const skeleton = document.querySelector(".plugins .skeleton-rows")!;
        return { rows, rowHeight, heldHeight: skeleton.querySelector("tr")!.getBoundingClientRect().height, busy: document.querySelector("[data-list]")!.getAttribute("aria-busy"), shown: getComputedStyle(skeleton).display, held: skeleton.querySelectorAll("tr").length };
      });
      expect(now.busy).toBe("true");
      // The skeleton rows stand where the rows were, as many and as tall.
      expect(now.shown).toBe("table-row-group");
      expect(now.held).toBe(now.rows);
      expect(now.heldHeight).toBe(now.rowHeight);
      await page.waitForURL(/\/plugins\/all\?sort=new$/);
      expect(await page.locator("[data-list]").getAttribute("aria-busy")).toBeNull();
    } finally {
      await context.close();
    }
  });

  test("the search field: / puts you in it, Escape empties it, Enter leads to All plugins", async () => {
    const { page, context } = await open("/plugins/all?category=git");
    try {
      await page.keyboard.press("/");
      const field = page.getByRole("searchbox", { name: "Search plugins" });
      await expect.poll(() => field.evaluate((el) => el === document.activeElement)).toBe(true);
      await page.keyboard.type("timer");
      await page.keyboard.press("Escape");
      expect(await field.inputValue()).toBe("");
      await page.keyboard.type("timer");
      await page.keyboard.press("Enter");
      await page.waitForURL(/\/plugins\/all\?q=timer$/);
      expect(await page.getByRole("heading", { level: 1 }).textContent()).toMatch(/^Results for “timer” · \d+$/);
    } finally {
      await context.close();
    }
  });
});
