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

/** The person's setting: the look follows it (styles/look/store.css), so each is checked. */
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
  ["/plugins?sort=new", undefined],
  ["/plugins?q=zzzzqqq", undefined],
  ["/plugins?category=git", undefined],
  ["/plugins/focus-timer", undefined],
  ["/plugins/pr-radar", undefined],
  ["/plugins/xss-probe", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12", undefined],
  ["/plugins?q=git", undefined],
  ["/plugins/theme-sync/report", "user_kai"],
  ["/plugins/publishing", undefined],
  ["/plugins/no-such-plugin", undefined],
  ["/account", "user_ada"],
  ["/account", "user_lin"],
  ["/account/plugins/session-badges", "user_ada"],
  ["/account/plugins/focus-timer", "user_ada"],
  ["/account/plugins/worst-case-listing-at-every-limit-xyz12", "user_ada"],
  ["/account/plugins/focus-timer", undefined],
  ["/account/plugins/session-badges/versions/1.2.1/yank", "user_ada"],
  ["/account/tokens", "user_ada"],
  ["/account/tokens", undefined],
  ["/account/tokens/new", "user_ada"],
  ["/account", "user_noor"],
  ["/account", undefined],
  ["/account/tokens/tok_0OZtGptZyNgbeUuc", "user_ada"],
  ["/plugins?verified=1", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12/versions", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12/versions/1.4.0", undefined],
  ["/plugins/worst-case-listing-at-every-limit-xyz12/asks", undefined],
  ["/plugins/focus-timer/versions/1.1.0", undefined],
  ["/plugins/focus-timer/versions/9.9.9", undefined],
  ["/plugins/voice-memos", undefined],
  ["/plugins/clipboard-sync", undefined],
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
  ["a plugin's picture, large", "/plugins/worst-case-listing-at-every-limit-xyz12", undefined, async (page) => {
    await page.locator("[data-shot]").first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["Verify, asked once", "/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0", "user_grace", async (page) => {
    await page.getByRole("link", { name: /^Mark .+ verified$/ }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
    expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Verify");
  }],
  ["Don't verify, written", "/admin/plugins/session-badges/1.2.1", "user_grace", async (page) => {
    await page.getByRole("link", { name: /^Don't verify/ }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
    await page.getByRole("dialog").getByLabel("Reason").selectOption("listing");
    await page.getByRole("dialog").getByLabel("For the author").fill("Say what each panel shows.");
  }],
  ["Take down", "/admin/plugins/focus-timer/1.2.0", "user_grace", async (page) => {
    await page.getByRole("link", { name: /^Take (this version )?down…$/ }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["closing a report", "/admin/reports", "user_grace", async (page) => {
    await page.getByRole("link", { name: "Close it…" }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["Remove", "/admin/plugins/focus-timer/1.2.0", "user_grace", async (page) => {
    await page.getByRole("link", { name: /^Remove( it)?…$/ }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["New token, its form", "/account/tokens", "user_ada", async (page) => {
    await page.getByRole("link", { name: "New token" }).click();
    await expect.poll(() => page.getByRole("dialog", { name: "New deploy token" }).isVisible()).toBe(true);
    await page.getByLabel("Chosen").check();
    await page.getByLabel("Focus timer", { exact: true }).check();
  }],
  ["a token, shown once", "/account/tokens", "user_ada", async (page) => {
    await page.getByRole("link", { name: "New token" }).click();
    await page.getByRole("dialog").getByLabel("Name", { exact: true }).fill("Release CI");
    await Promise.all([page.waitForURL("**/account/tokens"), page.getByRole("button", { name: "Create token" }).click()]);
    await expect.poll(() => page.getByRole("heading", { name: "Copy your token now" }).isVisible()).toBe(true);
  }],
  ["a token's fourteen plugins, shown", "/account/tokens", "user_ada", async (page) => {
    await page.locator("summary", { hasText: "14 plugins" }).click();
    await expect.poll(() => page.getByRole("list", { name: "What GitHub Actions, the monorepo can publish" }).isVisible()).toBe(true);
  }],
  ["the stop new installs dialog", "/account/plugins/session-badges", "user_ada", async (page) => {
    await page.getByRole("link", { name: "Stop new installs" }).first().click();
    await expect.poll(() => page.getByRole("dialog").isVisible()).toBe(true);
  }],
  ["the pictures, the row taken on", "/plugins/worst-case-listing-at-every-limit-xyz12", undefined, async (page) => {
    // (On a phone the row is a finger's to scroll: its arrows aren't there.)
    const next = page.getByRole("button", { name: "More pictures" });
    if (await next.isVisible()) {
      await next.click();
      await expect.poll(() => page.getByRole("button", { name: "Earlier pictures" }).isVisible()).toBe(true);
    }
  }],
  ["Install in Zeron's help", "/plugins/focus-timer", undefined, async (page) => {
    await page.evaluate(() => {
      const a = document.querySelector<HTMLAnchorElement>("a[data-install]")!;
      a.addEventListener("click", (e) => e.preventDefault(), { once: true });
      a.click();
    });
    await expect.poll(() => page.locator("#get-help-focus-timer").evaluate((d) => d.matches(":popover-open")), { timeout: 4_000 }).toBe(true);
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

describe("the bar is drawn once: nothing in it changes after the page loads", () => {
  test("signed in as an author or staff, or signed out, on public and private pages alike", async () => {
    const pages = ["/plugins", "/plugins?sort=new", "/plugins/focus-timer", "/plugins/publishing", "/account", "/account/tokens"];
    for (const as of [undefined, "user_ada", "user_grace"] as const) {
      const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: "reduce" });
      const page = await context.newPage();
      try {
        if (as) await page.goto(new URL(`/auth/web/callback?as=${as}&return_to=/plugins/publishing`, site.url).href);
        // Every change to the bar and its menus, from before the page is parsed until it has settled.
        await page.addInitScript(() => {
          const seen: string[] = [];
          (window as unknown as { barChanges: string[] }).barChanges = seen;
          new MutationObserver((records) => {
            for (const r of records) {
              const el = r.target instanceof Element ? r.target : r.target.parentElement;
              if (el?.closest("header.top, #me-menu, #nav-menu") && document.readyState !== "loading") seen.push(`${r.type} ${el.tagName.toLowerCase()}.${el.className} ${r.attributeName ?? ""}`);
            }
          }).observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
        });
        for (const path of pages) {
          await page.goto(new URL(path, site.url).href, { waitUntil: "networkidle" });
          await page.waitForTimeout(300);
          const changes = await page.evaluate(() => (window as unknown as { barChanges: string[] }).barChanges);
          expect(changes, `${path} as ${as ?? "no one"}`).toEqual([]);
          const named = await page.locator("header.top button.me").isVisible();
          expect(named, `${path} as ${as ?? "no one"}: the name in the bar`).toBe(as !== undefined);
        }
      } finally {
        await context.close();
      }
    }
  }, 120_000);
});

describe("a deploy token, shown once", () => {
  test("made from the dialog: shown on the page that made it, copied from there, and gone from the page and its history once it's left", async () => {
    const { page, context } = await open("/account/tokens", { as: "user_ada" });
    try {
      await context.grantPermissions(["clipboard-read", "clipboard-write"]);
      await page.getByRole("link", { name: "New token" }).click();
      const dialog = page.getByRole("dialog", { name: "New deploy token" });
      await dialog.getByLabel("Name", { exact: true }).fill("Release CI");
      await dialog.getByLabel("Ends").selectOption("30");
      const hint = dialog.locator("[data-chosen-hint]");
      expect(await hint.textContent()).toBe("Only these, up to 50");
      await dialog.getByLabel("Chosen").check();
      await dialog.getByLabel("Focus timer", { exact: true }).check();
      await dialog.getByLabel("Control room", { exact: true }).check();
      await expect.poll(() => hint.textContent()).toBe("1 plugin and 1 template, up to 50");
      await dialog.getByLabel("Control room", { exact: true }).uncheck();
      await expect.poll(() => hint.textContent()).toBe("1 plugin, up to 50");
      // (What the browser sends: the form's fields, never a token; what comes back has it.)
      const sent: string[] = [];
      page.on("request", (r) => sent.push(`${r.method()} ${r.url()} ${r.postData() ?? ""}`));
      await Promise.all([page.waitForLoadState("networkidle"), dialog.getByRole("button", { name: "Create token" }).click()]);
      const token = (await page.locator("#the-token").textContent())!.trim();
      expect(token).toMatch(/^zdt_[0-9A-Za-z]{49}$/);
      // The address is the list's at once: a reload, or a return, never posts again or shows it.
      expect(new URL(page.url()).pathname).toBe("/account/tokens");
      expect(sent.join("\n")).not.toContain("zdt_");
      // Copy puts it on the clipboard.
      await page.getByRole("button", { name: "Copy the token" }).click();
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(token);
      // Nothing the page keeps holds it.
      const kept = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage }, cookie: document.cookie, history: history.state, address: location.href }));
      expect(kept).not.toContain("zdt_");
      // I've copied it: the list, the token named but never shown, and no step back to it.
      await Promise.all([page.waitForURL("**/account/tokens"), page.getByRole("link", { name: "I've copied it" }).click()]);
      await page.waitForLoadState("networkidle");
      expect(await page.content()).not.toContain("zdt_");
      await expect.poll(() => page.getByText("Release CI").first().isVisible()).toBe(true);
      await page.goBack({ waitUntil: "networkidle" }).catch(() => undefined);
      expect(await page.content()).not.toContain("zdt_");
      expect(sent.join("\n")).not.toContain(token);
    } finally {
      await context.close();
      await site.fake("tokens");
    }
  }, 60_000);

  test("signed in a while ago: Create token signs in again first, then the form comes back as it was", async () => {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, reducedMotion: "reduce" });
    const page = await context.newPage();
    try {
      await page.goto(new URL("/auth/web/callback?as=user_ada&fresh=0&return_to=/account/tokens", site.url).href);
      await page.getByRole("link", { name: "New token" }).click();
      const dialog = page.getByRole("dialog", { name: "New deploy token" });
      await expect.poll(() => dialog.getByText("Zeron will ask you to sign in again first.").isVisible()).toBe(true);
      await dialog.getByLabel("Name", { exact: true }).fill("Nightly");
      await dialog.getByRole("button", { name: "Create token" }).click();
      // (The fake's sign-in: pick the account.)
      await page.waitForURL("**/auth/web/login**");
      await page.getByRole("link", { name: /Ada/ }).click();
      await page.waitForURL("**/account/tokens/new?again=1**");
      expect(await page.getByLabel("Name", { exact: true }).inputValue()).toBe("Nightly");
      await expect.poll(() => page.getByText("You signed in again. Create the token to finish.").isVisible()).toBe(true);
      await page.getByRole("button", { name: "Create token" }).click();
      await expect.poll(() => page.getByRole("heading", { name: "Copy your token now" }).isVisible()).toBe(true);
    } finally {
      await context.close();
      await site.fake("tokens");
    }
  }, 60_000);
});

describe("an author's words stay in their place", () => {
  test("a right-to-left override in a name reorders nothing after it", async () => {
    const { page, context } = await open("/admin/plugins/xss-probe/1.1.0", { as: "user_grace" });
    try {
      const [name, version] = await Promise.all([page.locator("h1 > .name").first().boundingBox(), page.locator("h1 > .vnum").boundingBox()]);
      // (After the name in reading order: further along its line, or on a line below it.)
      const sameLine = Math.abs(version!.y - name!.y) < name!.height / 2;
      if (sameLine) expect(version!.x).toBeGreaterThanOrEqual(name!.x + name!.width - 1);
      else expect(version!.y).toBeGreaterThan(name!.y);
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
      expect(await page.locator('.top .nav a[href="/admin/plugins"]').isVisible()).toBe(false);
      await me.click();
      const signOut = page.locator("#me-menu form[data-signout]");
      await expect.poll(() => signOut.isVisible()).toBe(true);
      expect(await signOut.locator("input[name=csrf]").inputValue()).not.toBe("");
    } finally {
      await context.close();
    }
    const staff = await open("/plugins", { as: "user_grace" });
    try {
      await expect.poll(() => staff.page.locator('.top .nav a[href="/admin/plugins"]').isVisible()).toBe(true);
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

describe("Review: nothing a reviewer reads is cut or hidden past an edge", () => {
  test("every code block, ask value, file name and diff line wraps; the worst case's whole API_URL shows", async () => {
    const base = "/admin/plugins/worst-case-listing-at-every-limit-xyz12/1.5.0";
    for (const view of ["", "?view=files", "?view=changes"])
      for (const width of [1728, 390]) {
        const { page, context } = await open(`${base}${view}`, { as: "user_grace", width });
        try {
          const hidden = await page.evaluate(() =>
            [...document.querySelectorAll("main :is(pre, code, .code, .diff, .codeblock, td.line, .fn, .literal li, .items li, .moved li, .tree-pane .name)")]
              .filter((el) => (el as HTMLElement).offsetParent && el.scrollWidth > el.clientWidth + 1)
              .map((el) => `${el.tagName.toLowerCase()}.${el.className}: ${(el.textContent ?? "").trim().slice(0, 40)} (${el.scrollWidth} in ${el.clientWidth})`),
          );
          expect(hidden, `${view || "overview"} at ${width}`).toEqual([]);
          if (!view) {
            // (The MCP server's environment, before and after: its long address whole, on screen.)
            const url = "API_URL=https://api.averyveryverylongsubdomainname.example.com";
            const shown = await page.evaluate((url) => {
              const blocks = [...document.querySelectorAll(".before-after pre")].filter((el) => el.textContent!.includes(url));
              return blocks.map((el) => { const r = el.getBoundingClientRect(); return el.scrollWidth <= el.clientWidth + 1 && r.left >= 0 && r.right <= window.innerWidth; });
            }, url);
            expect(shown.length, `${width}: the address is in a block`).toBeGreaterThan(0);
            expect(shown.every(Boolean), `${width}: whole, on screen`).toBe(true);
          }
        } finally {
          await context.close();
        }
      }
  }, 120_000);
});

describe("reporting a plugin", () => {
  test("by keyboard alone: a reason chosen (the chosen card ringed), words typed, sent, thanked", async () => {
    const { page, context } = await open("/plugins/theme-sync/report", { as: "user_kai" });
    try {
      // (Tab to the reasons: the first radio; an arrow chooses the next.)
      for (let i = 0; i < 40 && !(await page.evaluate(() => (document.activeElement as HTMLInputElement | null)?.name === "reason")); i++) await page.keyboard.press("Tab");
      await page.keyboard.press("ArrowDown");
      const chosen = await page.evaluate(() => {
        const input = document.activeElement as HTMLInputElement;
        return { value: input.value, checked: input.checked, ring: getComputedStyle(input.closest(".rcard")!).boxShadow };
      });
      expect(chosen.checked).toBe(true);
      expect(chosen.ring).toMatch(/1\.5px/);
      await page.keyboard.press("Tab");
      expect(await page.evaluate(() => document.activeElement?.id)).toBe("report-text");
      await page.keyboard.type("Typed by keyboard alone.");
      await page.keyboard.press("Tab");
      await page.keyboard.press("Tab");
      expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Send report");
      await page.keyboard.press("Enter");
      await expect.poll(() => page.locator("h1").textContent()).toBe("Thanks — the reviewers will read it.");
    } finally {
      await context.close();
    }
  });

  test("with no script: a card's click chooses its reason, and the form sends", async () => {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, javaScriptEnabled: false });
    try {
      const page = await context.newPage();
      await page.goto(new URL("/auth/web/callback?as=user_kai&return_to=/plugins", site.url).href);
      await page.goto(new URL("/plugins/theme-sync/report", site.url).href, { waitUntil: "networkidle" });
      await page.locator(".rcard", { hasText: "It doesn't work" }).click();
      expect(await page.locator('input[name="reason"][value="broken"]').isChecked()).toBe(true);
      await page.locator("#report-text").fill("Sent with no script.");
      await page.getByRole("button", { name: "Send report" }).click();
      await page.waitForLoadState("networkidle");
      expect(await page.locator("h1").textContent()).toBe("Thanks — the reviewers will read it.");
    } finally {
      await context.close();
    }
  });
});

describe("a plugin's tile, in every hue", () => {
  test("its glyph reads on its fill: at least 3:1, dark and light", async () => {
    for (const scheme of ["dark", "light"] as const) {
      const { page, context } = await open("/plugins", { scheme });
      try {
        const low = await page.evaluate(() => {
          // (Each colour as the screen draws it: through a canvas, whatever syntax the style computes to.)
          const pen = document.createElement("canvas").getContext("2d", { willReadFrequently: true })!;
          const rgb = (colour: string) => {
            pen.clearRect(0, 0, 1, 1);
            pen.fillStyle = "#000";
            pen.fillStyle = colour;
            pen.fillRect(0, 0, 1, 1);
            return [...pen.getImageData(0, 0, 1, 1).data.slice(0, 3)];
          };
          const lum = (c: number[]) => {
            const [r, g, b] = c.map((v) => (v / 255 <= 0.04045 ? v / 255 / 12.92 : ((v / 255 + 0.055) / 1.055) ** 2.4));
            return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
          };
          const out: string[] = [];
          for (let h = 0; h < 24; h++) {
            const tile = document.createElement("span");
            tile.className = `tile h${h}`;
            tile.textContent = "A";
            document.body.append(tile);
            const s = getComputedStyle(tile);
            const [a, b] = [lum(rgb(s.color)), lum(rgb(s.backgroundColor))];
            const ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
            if (ratio < 3) out.push(`h${h}: ${ratio.toFixed(2)}`);
            tile.remove();
          }
          return out;
        });
        expect(low, scheme).toEqual([]);
      } finally {
        await context.close();
      }
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
      // (Back on its opener, which names its version to a screen reader.)
      expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Stop new installs 1.2.1");
    } finally {
      await context.close();
    }
  });

  test("a plugin's pictures: a thumbnail opens the picture large, the arrows go through them, Escape closes it, back on the thumbnail", async () => {
    const { page, context } = await open("/plugins/worst-case-listing-at-every-limit-xyz12");
    try {
      const first = page.getByRole("link", { name: /, picture 1 of 8$/ });
      await first.focus();
      await page.keyboard.press("Enter");
      const box = page.getByRole("dialog", { name: /^Pictures of / });
      await expect.poll(() => box.isVisible()).toBe(true);
      // (Focus in it, on Close; the picture and its count.)
      expect(await page.evaluate(() => document.activeElement?.getAttribute("aria-label"))).toBe("Close");
      const count = box.locator("figcaption");
      expect(await count.textContent()).toBe("1 / 8");
      expect(await box.locator("img").getAttribute("alt")).toMatch(/, picture 1 of 8$/);
      await page.keyboard.press("ArrowRight");
      expect(await count.textContent()).toBe("2 / 8");
      expect(await box.locator("img").getAttribute("src")).toBe(await page.getByRole("link", { name: /, picture 2 of 8$/ }).getAttribute("href"));
      await page.keyboard.press("ArrowLeft");
      await page.keyboard.press("ArrowLeft");
      expect(await count.textContent()).toBe("8 / 8");
      await page.getByRole("button", { name: "The next picture" }).click();
      expect(await count.textContent()).toBe("1 / 8");
      await page.keyboard.press("Escape");
      await expect.poll(() => box.isVisible()).toBe(false);
      expect(await page.evaluate(() => document.activeElement?.getAttribute("aria-label") ?? document.activeElement?.querySelector("img")?.alt)).toMatch(/, picture 1 of 8$/);
      // (A click beside the picture closes it too.)
      await first.click();
      await expect.poll(() => box.isVisible()).toBe(true);
      await page.mouse.click(8, 8);
      await expect.poll(() => box.isVisible()).toBe(false);
    } finally {
      await context.close();
    }
  });

  test("a plugin's pictures: more than three, the row's arrow takes it on, and back", async () => {
    const { page, context } = await open("/plugins/worst-case-listing-at-every-limit-xyz12");
    try {
      const row = page.locator("[data-row]");
      const next = page.getByRole("button", { name: "More pictures" });
      const prev = page.getByRole("button", { name: "Earlier pictures" });
      expect(await next.isVisible()).toBe(true);
      expect(await prev.isVisible()).toBe(false);
      await next.click();
      await expect.poll(() => row.evaluate((el) => el.scrollLeft)).toBeGreaterThan(0);
      await expect.poll(() => prev.isVisible()).toBe(true);
      // (Three or fewer: nothing to scroll to, no arrows.)
      await page.goto(new URL("/plugins/focus-timer", site.url).href, { waitUntil: "networkidle" });
      expect(await page.locator(".shots-arrow").count()).toBe(0);
    } finally {
      await context.close();
    }
  });

  test("a plugin's pictures with no script: each thumbnail is the link to its picture, the row scrolls by itself", async () => {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, javaScriptEnabled: false });
    try {
      const page = await context.newPage();
      await page.goto(new URL("/plugins/worst-case-listing-at-every-limit-xyz12", site.url).href, { waitUntil: "networkidle" });
      const thumbs = page.locator("[data-shot]");
      expect(await thumbs.count()).toBe(8);
      for (const thumb of await thumbs.all()) {
        expect(await thumb.getAttribute("href")).toMatch(/\/shots\/screen-\d\.svg$/);
        expect(await thumb.getAttribute("target")).toBe("_blank");
      }
      // (The lightbox is never shown; no arrows that would do nothing; the row is wider than its room, to scroll.)
      expect(await page.getByRole("dialog").count()).toBe(0);
      expect(await page.locator(".shots-arrow:visible").count()).toBe(0);
      expect(await page.locator("[data-row]").evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
      const [tab] = await Promise.all([context.waitForEvent("page"), thumbs.first().click()]);
      await tab.waitForLoadState();
      expect(tab.url()).toBe(new URL((await thumbs.first().getAttribute("href"))!, page.url()).href);
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
        const a = document.querySelector<HTMLAnchorElement>("a[data-install]")!;
        a.addEventListener("click", (e) => e.preventDefault(), { once: true });
        a.click();
      });
      await expect.poll(() => page.locator("#get-help-focus-timer").evaluate((d) => d.matches(":popover-open")), { timeout: 4_000 }).toBe(true);
      expect(page.url()).toBe(before);
      expect(await page.locator("#get-help-focus-timer").getByText("Download Zeron").isVisible()).toBe(true);
    } finally {
      await context.close();
    }
  });

  test("the store: each order, filter and page is a link, the list's whole state its address", async () => {
    const { page, context } = await open("/plugins");
    try {
      await page.locator(".sort summary").click();
      await page.locator(".sort").getByRole("link", { name: "Newest" }).click();
      await page.waitForURL(/\/plugins\?sort=new$/);
      await page.locator(".facets").getByRole("link", { name: /^Git/ }).click();
      await page.waitForURL(/\/plugins\?category=git&sort=new$/);
      expect(await page.getByRole("heading", { level: 1 }).textContent()).toBe("Git");
      await page.locator(".facets").getByRole("link", { name: "Verified only" }).click();
      await page.waitForURL(/\/plugins\?category=git&verified=1&sort=new$/);
    } finally {
      await context.close();
    }
  });

  test("on a phone, the order and the filters open without a script, and Escape closes one with it", async () => {
    const { page, context } = await open("/plugins?category=productivity", { width: 390 });
    try {
      const filters = page.locator("details.drop", { hasText: "Filters" });
      await filters.locator("summary").click();
      await expect.poll(() => filters.evaluate((d) => (d as HTMLDetailsElement).open)).toBe(true);
      await expect.poll(() => filters.getByRole("link", { name: "Verified only" }).isVisible()).toBe(true);
      await page.keyboard.press("Escape");
      await expect.poll(() => filters.evaluate((d) => (d as HTMLDetailsElement).open)).toBe(false);
      // (Opening one closes the other.)
      const order = page.locator("details.drop.sort");
      await order.locator("summary").click();
      await filters.locator("summary").click();
      await expect.poll(() => order.evaluate((d) => (d as HTMLDetailsElement).open)).toBe(false);
    } finally {
      await context.close();
    }
  });

  test("the order, on a wide screen: a menu that opens without a script, its choices real addresses", async () => {
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, javaScriptEnabled: false });
    try {
      const page = await context.newPage();
      await page.goto(new URL("/plugins", site.url).href, { waitUntil: "networkidle" });
      const order = page.locator("details.drop.sort");
      expect(await order.locator("summary").textContent()).toBe("Sort: Most installed, change the order");
      await order.locator("summary").click();
      await page.getByRole("link", { name: "Newest" }).click();
      await page.waitForURL(/\/plugins\?sort=new$/);
      expect(await page.locator("details.drop.sort summary").textContent()).toBe("Sort: Newest, change the order");
    } finally {
      await context.close();
    }
  });

  test("the search: on the store its own, the hero's (the bar has none there); elsewhere the bar's", async () => {
    const { page, context } = await open("/plugins/focus-timer");
    try {
      expect(await page.locator(".top .find").isVisible()).toBe(true);
      await page.keyboard.press("/");
      await expect.poll(() => page.locator(".top").getByRole("searchbox").evaluate((el) => el === document.activeElement)).toBe(true);
      await page.goto(new URL("/plugins", site.url).href, { waitUntil: "networkidle" });
      expect(await page.locator(".top .find").isVisible()).toBe(false);
      expect(await page.locator(".hero").getByRole("searchbox", { name: "Search plugins" }).isVisible()).toBe(true);
    } finally {
      await context.close();
    }
    // (On a phone, the bar's icon isn't there either: the hero's field is.)
    const phone = await open("/plugins", { width: 390 });
    try {
      expect(await phone.page.getByRole("button", { name: "Search plugins" }).isVisible()).toBe(false);
      expect(await phone.page.locator(".hero").getByRole("searchbox").isVisible()).toBe(true);
    } finally {
      await phone.context.close();
    }
  });

  test("the search, the store's: / puts you in it, Escape empties it, Enter searches within the category", async () => {
    const { page, context } = await open("/plugins?category=git");
    try {
      await page.keyboard.press("/");
      const field = page.locator(".hero").getByRole("searchbox", { name: "Search plugins" });
      await expect.poll(() => field.evaluate((el) => el === document.activeElement)).toBe(true);
      await page.keyboard.type("timer");
      await page.keyboard.press("Escape");
      expect(await field.inputValue()).toBe("");
      await page.keyboard.type("pr");
      await page.keyboard.press("Enter");
      await page.waitForURL(/\/plugins\?q=pr&category=git$/);
      expect(await page.getByRole("heading", { level: 1 }).textContent()).toBe("Results for “pr”");
    } finally {
      await context.close();
    }
  });
});
