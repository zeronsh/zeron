// The store in a real browser, in front of the real edge: the browser is on
// the site's own https origin (its name resolved to a local https front that
// hands each request to the built site, as Cloudflare hands it one), signs
// in through AuthKit's page (the same front, the runner's page behind it),
// keeps the cookies the edge sets (the rotated staff session among them),
// opens the dialogs and posts the forms; and axe finds nothing on the pages
// as the edge's real data fills them.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";
import AxeBuilder from "@axe-core/playwright";
import { chromium, type Browser, type BrowserContext, type Page } from "playwright";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import { Publisher, startRealEdge, type RealEdge } from "./real-edge.ts";
import { FLASH } from "../../src/lib/store/flash.ts";

const VERIFIED = FLASH.verified.message;

let edge: RealEdge;
let browser: Browser;
let front: Server;
/** Who AuthKit's page signs in next. */
let signingIn: string | null = null;

/**
 * The https front for the site's name and AuthKit's: a certificate made for
 * this run (the browser told to take it), each request to the site handed
 * to the built site whole, its answer (redirects, every cookie) passed back
 * as it came.
 */
async function startFront(): Promise<{ server: Server; port: number }> {
  const dir = mkdtempSync(join(tmpdir(), "zeron-site-front-"));
  execFileSync("openssl", ["req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes", "-days", "1", "-subj", "/CN=site.e2e.test", "-addext", "subjectAltName=DNS:site.e2e.test,DNS:api.workos.com", "-keyout", join(dir, "key.pem"), "-out", join(dir, "cert.pem")], { stdio: "ignore" });
  const server = createServer({ key: readFileSync(join(dir, "key.pem")), cert: readFileSync(join(dir, "cert.pem")) }, async (req, res) => {
    const path = req.url ?? "/";
    if (req.headers.host === "api.workos.com") {
      if (!signingIn || !path.startsWith("/user_management/authorize?")) return res.writeHead(404).end();
      const page = await edge.authorize(new URL(path, "https://api.workos.com"), signingIn);
      return res.writeHead(page.status, { location: page.headers.get("location") ?? "" }).end();
    }
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    const headers = new Headers();
    for (const [name, value] of Object.entries(req.headers)) if (name !== "host" && value !== undefined) headers.set(name, Array.isArray(value) ? value.join(", ") : value);
    const response = await edge.site(path, { method: req.method, headers, body: req.method === "GET" || req.method === "HEAD" ? undefined : Buffer.concat(chunks) });
    const out: Record<string, string | string[]> = {};
    response.headers.forEach((value, name) => {
      if (name !== "set-cookie") out[name] = value;
    });
    if (response.headers.getSetCookie().length) out["set-cookie"] = response.headers.getSetCookie();
    res.writeHead(response.status, out).end(Buffer.from(await response.arrayBuffer()));
  });
  await new Promise<void>((ok) => server.listen(0, "127.0.0.1", ok));
  rmSync(dir, { recursive: true, force: true });
  return { server, port: (server.address() as { port: number }).port };
}

beforeAll(async () => {
  edge = await startRealEdge();
  const { server, port } = await startFront();
  front = server;
  browser = await chromium.launch({ args: [`--host-resolver-rules=MAP site.e2e.test:443 127.0.0.1:${port}, MAP api.workos.com:443 127.0.0.1:${port}`] });
  const ada = await Publisher.signIn(edge.url, "user_ada", "adawren");
  await Publisher.signIn(edge.url, "user_grace", "gracehop");
  await edge.grant("user_grace", "admin");
  await ada.publish("focus-timer", "1.0.0", {
    files: { "README.md": "# Focus timer\n\nTwenty-five minutes on, five off.\n\n![The timer](shots/one.png)\n\n| Setting | Default |\n| --- | --- |\n| Focus | 25 min |\n" },
  });
  await ada.publish("pr-review", "0.3.0", { manifest: { categories: ["git"], permissions: ["ui.add", "messages.send"] } });
}, 300_000);

afterAll(async () => {
  await browser?.close();
  await new Promise((done) => (front ? front.close(done) : done(undefined)));
  await edge?.stop();
});

/** A new browser on the site, that signs in as `userId` when a page asks it to. */
async function visitor(userId: string | null, width = 1440): Promise<{ context: BrowserContext; page: Page }> {
  signingIn = userId;
  const context = await browser.newContext({ viewport: { width, height: 900 }, reducedMotion: "reduce", ignoreHTTPSErrors: true });
  return { context, page: await context.newPage() };
}

const session = async (context: BrowserContext) => (await context.cookies(edge.origin)).find((c) => c.name === "__Host-zeron_session");

async function violations(page: Page) {
  const result = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"]).analyze();
  return result.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
}

/** The innermost elements that reach past the page's right edge. */
const overflowing = (page: Page) =>
  page.evaluate(() => {
    if (document.documentElement.scrollWidth <= window.innerWidth) return [];
    const wide = [...document.querySelectorAll("body *")].filter((e) => e.getBoundingClientRect().right > window.innerWidth + 0.5);
    return wide.filter((e) => !wide.some((o) => o !== e && e.contains(o))).map((e) => `${e.tagName.toLowerCase()}.${[...e.classList].join(".")}: ${(e.textContent ?? "").slice(0, 60)}`);
  });

describe("a reviewer, start to finish", () => {
  test("signs in, sees the rotated session arrive, verifies, signs out", async () => {
    const { context, page } = await visitor("user_grace");
    try {
      // Signed out, the admin pages aren't there; signing in comes back to where it was asked.
      await page.goto(`${edge.origin}/admin/plugins`);
      expect(await page.title()).toMatch(/not found/i);
      // (Your plugins, signed out, offers sign-in on the page itself; it comes back there.)
      await page.goto(`${edge.origin}/account`);
      await page.locator(".please").getByRole("link", { name: "Sign in" }).click();
      await page.waitForURL(`${edge.origin}/account`);
      await expect.poll(() => page.getByText("@gracehop").first().isVisible()).toBe(true);
      const signedIn = await session(context);
      expect(signedIn).toMatchObject({ secure: true, httpOnly: true, sameSite: "Lax", path: "/" });
      // (The first admin page is the first privileged use: the edge rotated the token.)
      await page.goto(`${edge.origin}/admin/plugins`);
      const rotated = await session(context);
      expect(rotated!.value).not.toBe(signedIn!.value);
      await expect.poll(() => page.getByRole("link", { name: /focus timer/i }).first().isVisible()).toBe(true);

      await page.goto(`${edge.origin}/admin/plugins/focus-timer/1.0.0`);
      // (Verify asks once, its own button focused: a clean update is a click, then Enter.)
      await page.getByRole("complementary", { name: "Decision" }).getByRole("link", { name: "Mark 1.0.0 verified", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Verify focus timer 1.0.0?" });
      await expect.poll(() => dialog.isVisible()).toBe(true);
      expect(await page.evaluate(() => document.activeElement?.textContent?.trim())).toBe("Verify");
      await page.keyboard.press("Enter");
      await page.waitForURL(`${edge.origin}/admin/plugins/focus-timer/1.0.0`);
      await expect.poll(() => page.getByText(VERIFIED).isVisible()).toBe(true);

      // (Sign out is in your menu, under your name in the bar.)
      await page.locator(".top button[data-me]").click();
      await page.locator("#me-menu").getByRole("button", { name: "Sign out" }).click();
      await page.waitForURL(`${edge.origin}/`);
      expect(await session(context)).toBeUndefined();
      await page.goto(`${edge.origin}/admin/plugins`);
      expect(await page.title()).toMatch(/not found/i);
    } finally {
      await context.close();
    }
  }, 60_000);
});

describe("no violations axe can find, with the edge's data", () => {
  const PAGES: [string, string | null][] = [
    ["/plugins", null],
    ["/plugins/all", null],
    ["/plugins/focus-timer", null],
    ["/plugins/focus-timer/versions", null],
    ["/plugins/focus-timer/versions/1.0.0", null],
    ["/account", "user_ada"],
    ["/account/tokens", "user_ada"],
    ["/account/tokens/new", "user_ada"],
    ["/account/plugins/focus-timer", "user_ada"],
    ["/admin/plugins", "user_grace"],
    ["/admin/plugins/pr-review/0.3.0", "user_grace"],
    ["/admin/plugins/focus-timer", "user_grace"],
    ["/admin/audit", "user_grace"],
  ];
  for (const width of [1440, 390]) {
    for (const [path, as] of PAGES) {
      test(`${path} at ${width}`, async () => {
        const { context, page } = await visitor(as, width);
        try {
          if (as) {
            await page.goto(`${edge.origin}/auth/web/login?return_to=%2Faccount`);
            expect(page.url()).toBe(`${edge.origin}/account`);
          }
          await page.goto(`${edge.origin}${path}`, { waitUntil: "networkidle" });
          expect(page.url()).toBe(`${edge.origin}${path}`);
          expect(await violations(page)).toEqual([]);
          // Nothing runs off the side of a phone (named, if something does).
          expect(await overflowing(page)).toEqual([]);
        } finally {
          await context.close();
        }
      }, 30_000);
    }
  }
});
