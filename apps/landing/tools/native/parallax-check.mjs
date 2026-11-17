// Does the parallax follow the mouse on the landing page itself? A visible
// (headed) Chrome and WebKit window, the mouse moved through the browser's
// own input pipeline over the hero, at several scroll positions: each
// corner of the page must move the scene, the middle and the page outside
// the window must leave it at rest (as the app: only the page under its
// title strip follows the pointer).
//   node parallax-check.mjs [url] [out dir]
import puppeteer from "puppeteer-core";
import { webkit } from "playwright";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { PNG } from "pngjs";
import { serve } from "./serve.mjs";

let url = process.argv[2];
const out = process.argv[3] || "parallax-out";
await mkdir(out, { recursive: true });
let server = null;
if (!url || url === "local") {
  server = await serve(0);
  url = `http://127.0.0.1:${server.address().port}/`;
}

// Mean absolute difference of two PNG buffers (per channel, of 255).
function diff(a, b) {
  const pa = PNG.sync.read(a), pb = PNG.sync.read(b);
  let sum = 0;
  for (let i = 0; i < pa.data.length; i += 4) for (let c = 0; c < 3; c++) sum += Math.abs(pa.data[i + c] - pb.data[i + c]);
  return sum / (pa.width * pa.height * 3);
}

async function run(name, page) {
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  await page.goto(url, { waitUntil: "networkidle0" in page ? undefined : "load" });
  const rows = [];
  for (const block of ["center", "start", "end"]) {
    await page.evaluate((block) => document.getElementById("hero-app").scrollIntoView({ block }), block);
    await page.waitForFunction(() => document.getElementById("hero-app").classList.contains("is-live"), { timeout: 20000 });
    await wait(600);
    const r = await page.evaluate(() => { const b = document.getElementById("hero-app").getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height, vh: innerHeight }; });
    const k = r.w / 1176;
    const at = (px, py) => [r.x + px * k, Math.min(Math.max(r.y + py * k, 2), r.vh - 2)];
    const el = await page.$("#hero-app");
    const snap = () => el.screenshot();
    // Rest: the pointer away from the window.
    await page.mouse.move(r.x - 20 > 0 ? r.x - 20 : 2, 2);
    await wait(900);
    const rest = await snap();
    const row = { block };
    for (const [label, px, py] of [["top-left", 10, 46], ["bottom-right", 1166, Math.min(760, (r.vh - r.y) / k - 4)], ["middle", 588, 402]]) {
      const [x, y] = at(px, py);
      await page.mouse.move(x, y, { steps: 12 });
      await wait(1000);
      row[label] = +diff(rest, await snap()).toFixed(2);
    }
    // Over the page above the window (its title strip, then the page outside).
    await page.mouse.move(...at(588, 20), { steps: 8 });
    await wait(1000);
    row["title strip"] = +diff(rest, await snap()).toFixed(2);
    rows.push(row);
    if (block === "center") {
      await page.mouse.move(...at(10, 46), { steps: 12 });
      await wait(1000);
      await el.screenshot({ path: join(out, `${name}-top-left.png`) });
    }
  }
  console.log(name);
  console.table(rows);
  // The gallery's cards: each follows the mouse over itself.
  const cardRows = [];
  for (const card of ["visuals", "cat", "machines"]) {
    await page.evaluate((c) => document.querySelector(`.p-card[data-card=${c}]`).scrollIntoView({ block: "center" }), card);
    await page.waitForFunction(`document.querySelector(".p-card[data-card=${card}]").classList.contains("is-live")`, undefined, { timeout: 20000 }).catch(() => page.waitForFunction(`document.querySelector(".p-card[data-card=${card}]").classList.contains("is-live")`, { timeout: 20000 }));
    await wait(600);
    const r = await page.evaluate((c) => { const b = document.querySelector(`.p-card[data-card=${c}]`).getBoundingClientRect(); return { x: b.x, y: b.y, w: b.width, h: b.height }; }, card);
    const el = await page.$(`.p-card[data-card=${card}]`);
    // (The cat's frames are its own life: compared with the pointer away,
    // twice, to see what moves without it.)
    await page.mouse.move(2, 2);
    await wait(900);
    const rest = await el.screenshot();
    const row = { card };
    for (const [label, fx, fy] of [["corner", 0.03, 0.04], ["far corner", 0.97, 0.96], ["middle", 0.5, 0.5]]) {
      await page.mouse.move(r.x + fx * r.w, r.y + fy * r.h, { steps: 12 });
      await wait(1000);
      row[label] = +diff(rest, await el.screenshot()).toFixed(2);
    }
    cardRows.push(row);
  }
  console.table(cardRows);
}

try {
  {
    const browser = await puppeteer.launch({
      executablePath: process.env.CHROME || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      headless: false, defaultViewport: null,
      args: ["--window-size=1500,1000", "--no-first-run", "--force-color-profile=srgb"],
    });
    const [page] = await browser.pages();
    await run("chrome", page);
    await browser.close();
  }
  {
    const browser = await webkit.launch({ headless: false });
    const page = await browser.newPage({ viewport: { width: 1500, height: 950 }, deviceScaleFactor: 2 });
    await run("webkit", page);
    await browser.close();
  }
} finally {
  server?.close();
}
