// Captures the web hero at the window's size and scale, at pointer poses:
//   node capture.mjs <out dir> [name=x:y,...] [--scale 2]
// Each pose is set at once (springs at rest), as the native reference
// waits for its springs to settle.
import puppeteer from "puppeteer-core";
import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { serve } from "./serve.mjs";

const CHROME = process.env.CHROME || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const out = process.argv[2];
const shots = (process.argv[3] && !process.argv[3].startsWith("--") ? process.argv[3] : "rest=0.5:0.5").split(",");
const scale = Number(process.argv[process.argv.indexOf("--scale") + 1] || 2) || 2;
await mkdir(out, { recursive: true });

const server = await serve(0);
const port = server.address().port;
const browser = await puppeteer.launch({
  executablePath: CHROME, headless: true,
  args: ["--no-first-run", "--hide-scrollbars", "--force-color-profile=srgb", "--use-angle=metal", "--enable-gpu"],
});
try {
  const page = await browser.newPage();
  page.on("console", (m) => console.log("page:", m.text()));
  page.on("pageerror", (e) => console.log("page error:", e.message));
  // CARD=<name>: a gallery card (560×388) instead of the hero.
  const card = process.env.CARD;
  await page.setViewport(card ? { width: 560, height: 388, deviceScaleFactor: scale } : { width: 1176, height: 768, deviceScaleFactor: scale });
  for (const shot of shots) {
    const [name, xy] = shot.split("=");
    const [x, y] = xy.split(":").map(Number);
    const extra = process.env.QUERY ? "&" + process.env.QUERY : "";
    await page.goto(card ? `http://127.0.0.1:${port}/_rig/card.html?card=${card}&pose=${x},${y}${extra}` : `http://127.0.0.1:${port}/_rig/?pose=${x},${y}${extra}`);
    await page.waitForFunction(() => window.__heroReady, { timeout: 20000 });
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    // The canvas' own pixels (not a screenshot through the compositor).
    const data = await page.evaluate(() => window.__hero.canvas.toDataURL("image/png"));
    const { writeFile } = await import("node:fs/promises");
    await writeFile(join(out, `${name}.png`), Buffer.from(data.split(",")[1], "base64"));
    console.log(name, "done");
  }
} finally {
  await browser.close();
  server.close();
}
