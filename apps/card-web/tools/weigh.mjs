// What the player costs: every response a cold load of /@u/player fetches
// (bytes on the wire, uncompressed and gzip-compressed as a host would
// send them), the time to the first drawn frame, and frame times while the
// pointer moves over the card. `node tools/weigh.mjs [username] [--mobile]
// [--swiftshader]`.
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { launch, settle } from "./browser.mjs";
import { argsOf } from "./capture-web.mjs";
import { startServer } from "./serve.mjs";

const o = argsOf(process.argv.slice(2));
const member = process.argv.slice(2).find((a) => !a.startsWith("--")) || "graham";
const { server, url } = await startServer();
const browser = await launch({ swiftshader: !!o.swiftshader });
const page = await browser.newPage();
if (o.mobile) await page.setViewport({ width: 375, height: 375, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
else await page.setViewport({ width: 480, height: 480, deviceScaleFactor: 2 });
const responses = [];
page.on("response", async (r) => {
  try {
    const buf = await r.buffer();
    responses.push({ url: new URL(r.url()).pathname, status: r.status(), bytes: buf.length, gzip: gzipSync(buf).length, type: r.headers()["content-type"] || "" });
  } catch { /* a 404 or a cancelled body */ }
});
const t0 = performance.now();
await page.goto(`${url}/@${encodeURIComponent(member)}/player?arrive=0`, { waitUntil: "load" });
await settle(page);
const firstFrame = Math.round(performance.now() - t0);
const timing = await page.evaluate(() => {
  const n = performance.getEntriesByType("navigation")[0];
  return { domContentLoaded: Math.round(n.domContentLoadedEventEnd), load: Math.round(n.loadEventEnd) };
});
// The pointer over the card for two seconds: tilt, parallax and foil moving.
const box = await page.evaluate(() => { const s = window.__card.state; return { x: s.origin.x, y: s.origin.y, z: s.z, w: 300, h: 420 }; });
await page.mouse.move(box.x + box.w * box.z * 0.2, box.y + box.h * box.z * 0.2);
for (let i = 0; i < 48; i++) {
  const t = i / 47;
  await page.mouse.move(box.x + box.w * box.z * (0.2 + 0.6 * t), box.y + box.h * box.z * (0.2 + 0.6 * Math.abs(Math.sin(t * 6))));
  await new Promise((r) => setTimeout(r, 40));
}
const frames = await page.evaluate(() => {
  const s = window.__card.state;
  const q = (arr, p) => { const a = [...arr].sort((x, y) => x - y); return a.length ? +a[Math.min(a.length - 1, Math.floor(p * a.length))].toFixed(2) : null; };
  return { frames: s.renderMs.length, renderMs: { p50: q(s.renderMs, 0.5), p90: q(s.renderMs, 0.9), max: q(s.renderMs, 1) }, frameGapMs: { p50: q(s.frameGapMs, 0.5), p90: q(s.frameGapMs, 0.9), max: q(s.frameGapMs, 1) }, canvas: [window.__card.canvas.width, window.__card.canvas.height] };
});
await browser.close();
server.close();
const total = responses.reduce((a, r) => a + r.bytes, 0), gz = responses.reduce((a, r) => a + r.gzip, 0);
console.log(JSON.stringify({ member, viewport: o.mobile ? "375x375@3" : "480x480@2", firstFrameMs: firstFrame, timing, bytes: total, gzipBytes: gz, responses, ...frames }, null, 1));
