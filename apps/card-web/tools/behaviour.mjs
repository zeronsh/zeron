// Checks the card's behaviour in a headless Chrome: the arrival, hover and
// touch, Reduce motion, a lost and restored GL context, no WebGL at all,
// and what the player does at 480x480 and in a phone column. Prints one
// line per check. `node tools/behaviour.mjs [--out dir]`.
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { launch, settle } from "./browser.mjs";
import { argsOf } from "./capture-web.mjs";
import { startServer } from "./serve.mjs";

const o = argsOf(process.argv.slice(2));
const out = o.out || null;
if (out) await mkdir(out, { recursive: true });
const { server, url } = await startServer();
const browser = await launch();
const results = [];
const check = (name, ok, detail = "") => { results.push({ name, ok, detail }); console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const state = (page) => page.evaluate(() => { const s = window.__card.state; return { inside: s.inside, lit: s.lit, strength: s.strength.x, rotX: s.rotX.x, rotY: s.rotY.x, foilAt: s.foilAt, z: s.z, origin: s.origin, frame: s.frame, arrived: s.arrived, raf: !!s.raf, failed: s.failed, lost: s.lost }; });
const shot = async (page, name) => { if (out) await writeFile(join(out, `${name}.png`), await page.screenshot()); };

try {
  // The player at the post's size: the card and its caption fully inside.
  let page = await browser.newPage();
  await page.setViewport({ width: 480, height: 480, deviceScaleFactor: 2 });
  await page.goto(`${url}/@mara/player?arrive=0`, { waitUntil: "load" });
  await settle(page);
  let s = await state(page);
  check("480x480: card inside the frame", s.origin.x >= 0 && s.origin.y >= 0 && s.origin.x + 300 * s.z <= 480 && s.origin.y + (420 + 32) * s.z <= 480, `scale ${s.z.toFixed(3)}, card ${Math.round(300 * s.z)}x${Math.round(420 * s.z)} at ${Math.round(s.origin.x)},${Math.round(s.origin.y)}`);
  await shot(page, "player-480");
  // Hover: lit, tilted toward the pointer, then back at rest.
  await page.mouse.move(s.origin.x + 60 * s.z, s.origin.y + 100 * s.z);
  await sleep(400);
  const hovered = await state(page);
  check("hover lights the foil and tilts", hovered.inside && hovered.lit && hovered.strength > 0.95 && Math.abs(hovered.rotX) > 1 && Math.abs(hovered.rotY) > 1, `strength ${hovered.strength.toFixed(2)}, rotX ${hovered.rotX.toFixed(1)}, rotY ${hovered.rotY.toFixed(1)}`);
  await shot(page, "player-480-hover");
  await page.mouse.move(2, 2);
  await sleep(900);
  const rested = await state(page);
  check("leaving rests the card", !rested.inside && !rested.lit && rested.strength === 0 && rested.rotX === 0 && rested.rotY === 0, `rotX ${rested.rotX}, rotY ${rested.rotY}, lit ${rested.lit}`);
  // A lost context stops the loop; restored, it draws again.
  const lost = await page.evaluate(async () => {
    const c = window.__card.canvas; const gl = c.getContext("webgl2"); const ext = gl.getExtension("WEBGL_lose_context");
    if (!ext) return "no WEBGL_lose_context";
    ext.loseContext(); await new Promise((r) => setTimeout(r, 100));
    const during = window.__card.state.lost;
    ext.restoreContext(); await new Promise((r) => setTimeout(r, 300));
    return { during, after: window.__card.state.lost, failed: window.__card.state.failed };
  });
  check("lost GL context pauses, restored draws again", typeof lost === "object" && lost.during === true && lost.after === false && !lost.failed, JSON.stringify(lost));
  await page.close();

  // Touch: a finger down tilts and lights it; lifting it rests it.
  page = await browser.newPage();
  await page.setViewport({ width: 375, height: 375, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
  await page.goto(`${url}/@mara/player?arrive=0`, { waitUntil: "load" });
  await settle(page);
  s = await state(page);
  check("375x375: card inside the frame", s.origin.x >= 0 && s.origin.y >= 0 && s.origin.x + 300 * s.z <= 375 && s.origin.y + (420 + 32) * s.z <= 375, `scale ${s.z.toFixed(3)}`);
  await shot(page, "player-375");
  await page.touchscreen.touchStart(s.origin.x + 40 * s.z, s.origin.y + 80 * s.z);
  await page.touchscreen.touchMove(s.origin.x + 200 * s.z, s.origin.y + 300 * s.z);
  await sleep(400);
  const touched = await state(page);
  await page.touchscreen.touchEnd();
  await sleep(900);
  const released = await state(page);
  check("touch drags the tilt, lifting rests it", touched.inside && touched.lit && Math.abs(touched.rotY) > 1 && !released.inside && !released.lit, `touched rotY ${touched.rotY.toFixed(1)}, released lit ${released.lit}`);
  await shot(page, "player-375-touch");
  await page.close();

  // Reduce motion: still, no drift, no tilt, no arrival.
  page = await browser.newPage();
  await page.emulateMediaFeatures([{ name: "prefers-reduced-motion", value: "reduce" }]);
  await page.setViewport({ width: 480, height: 480, deviceScaleFactor: 2 });
  await page.goto(`${url}/@mara/player`, { waitUntil: "load" });
  await settle(page);
  const a = await page.screenshot();
  await page.mouse.move(240, 240);
  await sleep(600);
  const b = await page.screenshot();
  const st = await state(page);
  check("reduced motion: still, appears without arriving, no tilt under the pointer", a.equals(b) && st.rotX === 0 && st.arrived && !st.raf, `frames equal ${a.equals(b)}, loop running ${st.raf}`);
  await page.close();

  // The arrival: unseen first, up and in within a second, foil swept.
  page = await browser.newPage();
  await page.setViewport({ width: 480, height: 480, deviceScaleFactor: 2 });
  await page.goto(`${url}/@mara/player`, { waitUntil: "load" });
  await page.waitForFunction(() => window.__card && window.__card.state.ready);
  const early = await state(page);
  await sleep(500);
  const mid = await state(page);
  await shot(page, "player-arriving");
  await sleep(1000);
  const done = await state(page);
  check("arrive: lit and rising, settled within 1.5 s", early.lit && !early.arrived && mid.lit && done.arrived && !done.lit, `t≈0 lit ${early.lit}, t≈0.5 s lit ${mid.lit} foil x ${mid.foilAt[0].toFixed(2)}, t≈1.5 s arrived ${done.arrived} lit ${done.lit}`);
  await page.close();

  // No WebGL: the still picture shows instead.
  page = await browser.newPage();
  await page.setViewport({ width: 480, height: 480, deviceScaleFactor: 2 });
  await page.evaluateOnNewDocument(() => { const g = HTMLCanvasElement.prototype.getContext; HTMLCanvasElement.prototype.getContext = function (t, ...a) { return t === "webgl2" ? null : g.call(this, t, ...a); }; });
  await page.goto(`${url}/@mara/player?arrive=0`, { waitUntil: "load" });
  await page.waitForFunction(() => window.__cardReady !== undefined);
  await sleep(500);
  const fallback = await page.evaluate(() => ({ failed: document.getElementById("stage").classList.contains("failed"), img: getComputedStyle(document.querySelector(".fallback")).display, canvas: document.querySelectorAll("canvas").length }));
  check("no WebGL: the still image shows", fallback.failed && fallback.img === "block" && fallback.canvas === 0, JSON.stringify(fallback));
  await page.close();

  // The embed test page: the iframe in a 480 column and a phone column.
  page = await browser.newPage();
  await page.setViewport({ width: 600, height: 760, deviceScaleFactor: 2 });
  await page.goto(`${url}/embed-test.html?u=mara`, { waitUntil: "load" });
  await sleep(1200);
  const frame = page.frames().find((f) => f.url().includes("/player"));
  const inner = frame ? await frame.evaluate(() => { const s = window.__card && window.__card.state; return s ? { z: s.z, w: s.frame.w, h: s.frame.h } : null; }) : null;
  check("embed test: the player draws inside the post's iframe", !!inner && inner.w > 400, JSON.stringify(inner));
  await shot(page, "embed-480");
  await page.setViewport({ width: 375, height: 812, deviceScaleFactor: 3, isMobile: true, hasTouch: true });
  await page.goto(`${url}/embed-test.html?u=mara&w=343`, { waitUntil: "load" });
  await sleep(1200);
  const frame2 = page.frames().find((f) => f.url().includes("/player"));
  const inner2 = frame2 ? await frame2.evaluate(() => { const s = window.__card && window.__card.state; return s ? { z: s.z, w: s.frame.w, h: s.frame.h, inside: s.origin.x >= 0 && s.origin.y >= 0 } : null; }) : null;
  check("embed test, phone column: the card fits", !!inner2 && inner2.inside, JSON.stringify(inner2));
  await shot(page, "embed-375");
  await page.close();
} finally {
  await browser.close();
  server.close();
}
console.log(`${results.filter((r) => r.ok).length}/${results.length} checks pass`);
