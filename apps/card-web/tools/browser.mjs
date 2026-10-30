// A headless Chrome for the tools, through puppeteer-core: the Chrome on this
// machine (`CHROME` names another), with WebGL on. `page.goto` then
// `settle(page)` waits for the card to have drawn.
import puppeteer from "puppeteer-core";

const CHROMES = [
  process.env.CHROME,
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/Applications/Chromium.app/Contents/MacOS/Chromium",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium"
].filter(Boolean);

export async function launch({ swiftshader = false } = {}) {
  const { access } = await import("node:fs/promises");
  let executablePath = null;
  for (const c of CHROMES) {
    try { await access(c); executablePath = c; break; } catch { /* next */ }
  }
  if (!executablePath) throw new Error("no Chrome found; set CHROME to its executable");
  const args = ["--no-first-run", "--hide-scrollbars", "--disable-gpu-sandbox", "--force-color-profile=srgb"];
  if (swiftshader) args.push("--use-angle=swiftshader", "--enable-unsafe-swiftshader");
  return puppeteer.launch({ executablePath, headless: true, args });
}

export async function settle(page, timeout = 20000) {
  const ok = await page.waitForFunction(() => window.__cardReady, { timeout }).then((h) => h.jsonValue());
  if (!ok) throw new Error("the card failed to draw (see cardfailed)");
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
}
