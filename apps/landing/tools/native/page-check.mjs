// The landing page's hero window, as visitors get it (headless Chrome on the
// local server): desktop (drawn, the same pixels as the rig's, follows the
// mouse), a phone (fits, no sideways scroll, scroll frame times), Reduce
// motion (a still frame), and no WebGL (the still picture).
//   node page-check.mjs <out dir>
import puppeteer from "puppeteer-core";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { serve } from "./serve.mjs";

const out = process.argv[2];
await mkdir(out, { recursive: true });
const server = await serve(0);
const url = `http://127.0.0.1:${server.address().port}/`;
const CHROME = process.env.CHROME || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const launch = (extra = []) => puppeteer.launch({ executablePath: CHROME, headless: true, args: ["--force-color-profile=srgb", "--hide-scrollbars", ...extra] });
const results = {};

async function open(browser, viewport, { reduced = false } = {}) {
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") errors.push(m.text()); });
  await page.setViewport(viewport);
  if (reduced) await page.emulateMediaFeatures([{ name: "prefers-reduced-motion", value: "reduce" }]);
  await page.goto(url, { waitUntil: "networkidle0" });
  return { page, errors };
}

try {
  // Desktop.
  {
    const browser = await launch();
    const { page, errors } = await open(browser, { width: 1440, height: 900, deviceScaleFactor: 2 });
    await page.evaluate(() => document.getElementById("hero-app").scrollIntoView({ block: "center" }));
    await page.waitForFunction(() => document.getElementById("hero-app").classList.contains("is-live"), { timeout: 20000 });
    await new Promise((r) => setTimeout(r, 500));
    const box = await page.evaluate(() => { const r = document.getElementById("hero-app").getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; });
    const canvas = await page.evaluate(() => { const c = document.querySelector("#hero-app canvas"); return { w: c.width, h: c.height }; });
    await (await page.$("#hero-app")).screenshot({ path: join(out, "desktop-rest.png") });
    // The pointer to the page's top-left corner: the layers follow.
    const k = box.w / 1176;
    await page.mouse.move(box.x + 300 * k, box.y + 60 * k);
    await page.mouse.move(box.x + 266 * k, box.y + 44 * k, { steps: 4 });
    await new Promise((r) => setTimeout(r, 1200));
    await (await page.$("#hero-app")).screenshot({ path: join(out, "desktop-pointer.png") });
    const title = await page.evaluate(() => document.querySelector(".hero h1 b").textContent);
    results.desktop = { box, canvas, title, errors };
    await browser.close();
  }
  // Phone.
  {
    const browser = await launch();
    const { page, errors } = await open(browser, { width: 390, height: 844, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
    await page.evaluate(() => document.getElementById("hero-app").scrollIntoView({ block: "center" }));
    await page.waitForFunction(() => document.getElementById("hero-app").classList.contains("is-live"), { timeout: 20000 });
    await new Promise((r) => setTimeout(r, 300));
    await (await page.$("#hero-app")).screenshot({ path: join(out, "phone.png") });
    // The cards: one column, each live, inside the page's width.
    results.phoneCards = [];
    for (const card of ["visuals", "cat", "machines"]) {
      await page.evaluate((c) => document.querySelector(`.p-card[data-card=${c}]`).scrollIntoView({ block: "center" }), card);
      await page.waitForFunction(`document.querySelector(".p-card[data-card=${card}]").classList.contains("is-live")`, { timeout: 20000 });
      await new Promise((r) => setTimeout(r, 300));
      await (await page.$(`.p-card[data-card=${card}]`)).screenshot({ path: join(out, `phone-${card}.png`) });
      results.phoneCards.push(await page.evaluate((c) => { const el = document.querySelector(`.p-card[data-card=${c}]`); const r = el.getBoundingClientRect(); return { card: c, width: r.width, height: Math.round(r.height), canvas: el.querySelector("canvas")?.width }; }, card));
    }
    await page.screenshot({ path: join(out, "phone-page.png") });
    const layout = await page.evaluate(() => {
      const r = document.getElementById("hero-app").getBoundingClientRect();
      return { width: r.width, height: r.height, overflowX: document.documentElement.scrollWidth > innerWidth, canvas: document.querySelector("#hero-app canvas").width };
    });
    // Scrolling past it: frame intervals while the page scrolls 3000 px.
    await page.evaluate(() => window.scrollTo(0, 0));
    const frames = await page.evaluate(() => new Promise((resolve) => {
      const times = [];
      let y = 0;
      const step = (t) => {
        times.push(t);
        y += 40;
        window.scrollTo(0, y);
        if (y < 3000) requestAnimationFrame(step); else resolve(times);
      };
      requestAnimationFrame(step);
    }));
    const gaps = frames.slice(1).map((t, i) => t - frames[i]).sort((a, b) => a - b);
    results.phone = { layout, scrollFrames: gaps.length, medianMs: gaps[gaps.length >> 1], p95Ms: gaps[Math.floor(gaps.length * 0.95)], maxMs: gaps[gaps.length - 1], errors };
    await browser.close();
  }
  // Reduce motion: the pointer moves nothing.
  {
    const browser = await launch();
    const { page, errors } = await open(browser, { width: 1440, height: 900, deviceScaleFactor: 2 }, { reduced: true });
    await page.evaluate(() => document.getElementById("hero-app").scrollIntoView({ block: "center" }));
    await page.waitForFunction(() => document.getElementById("hero-app").classList.contains("is-live"), { timeout: 20000 });
    await new Promise((r) => setTimeout(r, 300));
    const el = await page.$("#hero-app");
    const a = await el.screenshot({ encoding: "base64" });
    const box = await page.evaluate(() => { const r = document.getElementById("hero-app").getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width }; });
    await page.mouse.move(box.x + 280, box.y + 60, { steps: 4 });
    await new Promise((r) => setTimeout(r, 1000));
    const b = await el.screenshot({ encoding: "base64" });
    // A card too: the cat sits still, its scene doesn't follow.
    const cardEl = await page.$(".p-card[data-card=cat]");
    await page.evaluate(() => document.querySelector(".p-card[data-card=cat]").scrollIntoView({ block: "center" }));
    await page.waitForFunction(`document.querySelector(".p-card[data-card=cat]").classList.contains("is-live")`, { timeout: 20000 });
    await new Promise((r) => setTimeout(r, 300));
    const c1 = await cardEl.screenshot({ encoding: "base64" });
    const cb = await page.evaluate(() => { const r = document.querySelector(".p-card[data-card=cat]").getBoundingClientRect(); return { x: r.x, y: r.y }; });
    await page.mouse.move(cb.x + 10, cb.y + 10, { steps: 4 });
    await new Promise((r) => setTimeout(r, 8000));
    const c2 = await cardEl.screenshot({ encoding: "base64" });
    results.reduced = { stillUnderPointer: a === b, catCardStillFor8s: c1 === c2, errors };
    await writeFile(join(out, "reduced.png"), Buffer.from(b, "base64"));
    await browser.close();
  }
  // No WebGL: the still picture.
  {
    const browser = await launch(["--disable-3d-apis"]);
    const { page, errors } = await open(browser, { width: 1440, height: 900, deviceScaleFactor: 2 });
    await page.evaluate(() => document.getElementById("hero-app").scrollIntoView({ block: "center" }));
    await new Promise((r) => setTimeout(r, 1500));
    const state = await page.evaluate(() => {
      const img = document.querySelector("#hero-app img");
      return { canvas: !!document.querySelector("#hero-app canvas"), imgLoaded: img.complete && img.naturalWidth > 0, src: img.currentSrc };
    });
    await (await page.$("#hero-app")).screenshot({ path: join(out, "no-webgl.png") });
    results.noWebGL = { ...state, cards: await page.evaluate(() => [...document.querySelectorAll(".p-card")].map((c) => ({ card: c.dataset.card, canvas: !!c.querySelector("canvas"), img: c.querySelector("img").currentSrc.split("/").pop() }))), errors };
    await browser.close();
  }
} finally {
  server.close();
}
console.log(JSON.stringify(results, null, 1));
