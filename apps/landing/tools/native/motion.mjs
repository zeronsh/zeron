// The motion, against the app's: the web hero's layers sampled every frame
// (requestAnimationFrame, headless Chrome) as the pointer goes from the
// middle to the page's top-left corner, beside the app's own layer boxes
// over time (reference.py MOTION=0:0 → motion.json: the tree's boxes, to
// the point). Prints how often the rounded positions disagree, at the best
// time alignment, and writes both curves (motion-compare.json).
//   node motion.mjs <native motion.json> <out dir>
import puppeteer from "puppeteer-core";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { join } from "node:path";
import { serve } from "./serve.mjs";

const native = JSON.parse(await readFile(process.argv[2], "utf8"));
const out = process.argv[3];
await mkdir(out, { recursive: true });
const server = await serve(0);
const browser = await puppeteer.launch({
  executablePath: process.env.CHROME || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  headless: true, args: ["--force-color-profile=srgb"],
});
try {
  const page = await browser.newPage();
  // CARD=<name>: a gallery card's scene (its own plugin, its own layers).
  const card = process.env.CARD;
  await page.setViewport(card ? { width: 560, height: 388, deviceScaleFactor: 2 } : { width: 1176, height: 768, deviceScaleFactor: 2 });
  await page.goto(card ? `http://127.0.0.1:${server.address().port}/_rig/card.html?card=${card}` : `http://127.0.0.1:${server.address().port}/_rig/?live=1`);
  await page.waitForFunction(() => window.__heroReady, { timeout: 20000 });
  const web = await page.evaluate(() => new Promise((resolve) => {
    const hero = window.__hero, samples = [];
    const t0 = performance.now();
    hero.pose(0, 0);
    const tick = (now) => {
      samples.push({ t: (now - t0) / 1000, layers: hero.springs.map(([x, y]) => [x.x, y.x]) });
      if (now - t0 < 1200) requestAnimationFrame(tick); else resolve(samples);
    };
    requestAnimationFrame(tick);
  }));
  // The app's boxes: layer i's box is at (-48 + x, 16 + y).
  // (Each box's offset from where it rests: the first sample's.)
  // UNTIL=<s>: the app's samples only up to then (a real pointer reaching
  // the fixture window later sends its layers home).
  const until = Number(process.env.UNTIL || 1.2);
  const at0 = native[0].boxes;
  const nat = native.filter((s) => s.t <= until).map((s) => ({ t: s.t, layers: s.boxes.map((b, i) => [b[0] - at0[i][0], b[1] - at0[i][1]]) }));
  const N = at0.length;
  const at = (samples, t) => {
    let best = samples[0];
    for (const s of samples) if (s.t <= t) best = s;
    return best;
  };
  let bestFit = null;
  for (let lag = -0.05; lag <= 0.05; lag += 0.001) {
    let off = 0, n = 0;
    for (const s of nat) {
      if (s.t + lag < 0) continue;
      const w = at(web, s.t + lag);
      for (let i = 1; i < N; i++) for (let a = 0; a < 2; a++) {
        n++;
        if (Math.abs(Math.round(w.layers[i][a]) - s.layers[i][a]) > 0) off++;
      }
    }
    if (!bestFit || off < bestFit.off) bestFit = { lag, off, n };
  }
  // The largest gap anywhere, in points, at that alignment.
  let worst = 0;
  for (const s of nat) {
    if (s.t + bestFit.lag < 0) continue;
    const w = at(web, s.t + bestFit.lag);
    for (let i = 1; i < N; i++) for (let a = 0; a < 2; a++) worst = Math.max(worst, Math.abs(w.layers[i][a] - s.layers[i][a]));
  }
  console.log(`web frames: ${web.length} over 1.2 s; app samples: ${nat.length}`);
  console.log(`best alignment ${(bestFit.lag * 1000).toFixed(0)} ms: ${bestFit.off} of ${bestFit.n} rounded positions disagree (${(100 * bestFit.off / bestFit.n).toFixed(2)}%), the largest gap ${worst.toFixed(2)} pt (the app's tree rounds to the point)`);
  // Each web frame against the model the app follows (the plugin's springs,
  // stiffness 140, damping 20, semi-implicit Euler at the app's 120 Hz):
  // every frame should be one of its step states, exactly.
  const model = [];
  {
    const st = Array.from({ length: N - 1 }, (_, i) => i + 1).map((d) => [{ x: 0, v: 0, g: d * 14 }, { x: 0, v: 0, g: d * 9 }]);
    model.push(st.map((p) => p.map((q) => q.x)));
    for (let k = 0; k < 240; k++) {
      for (const p of st) for (const q of p) { q.v += (-140 * (q.x - q.g) - 20 * q.v) / 120; q.x += q.v / 120; }
      model.push(st.map((p) => p.map((q) => q.x)));
    }
  }
  let onModel = 0, worstModel = 0;
  for (const w of web) {
    let best = Infinity;
    for (const m of model) {
      let e = 0;
      for (let i = 0; i < N - 1; i++) for (let a = 0; a < 2; a++) e = Math.max(e, Math.abs(w.layers[i + 1][a] - m[i][a]));
      best = Math.min(best, e);
    }
    worstModel = Math.max(worstModel, best);
    if (best < 1e-3) onModel++;
  }
  // And the app's samples against the same model (at its best lag).
  let appOff = 0, appN = 0, bestLag = null;
  for (let lag = 0; lag < 0.05; lag += 0.001) {
    let off = 0, n = 0;
    for (const s of nat) {
      const k = Math.floor((s.t - lag) * 120);
      const m = model[Math.max(0, Math.min(model.length - 1, k))];
      for (let i = 0; i < N - 1; i++) for (let a = 0; a < 2; a++) { n++; if (Math.round(k < 0 ? 0 : m[i][a]) !== s.layers[i + 1][a]) off++; }
    }
    if (bestLag === null || off < appOff) { appOff = off; appN = n; bestLag = lag; }
  }
  console.log(`web frames on the 120 Hz model's step states: ${onModel} of ${web.length} (largest gap ${worstModel.toExponential(1)} pt)`);
  console.log(`app samples off the model (rounded to the point, as its tree reports): ${appOff} of ${appN} at ${(bestLag * 1000).toFixed(0)} ms latency`);
  const settledWeb = web.find((s) => s.layers.every(([x, y], i) => i === 0 || (Math.abs(x - i * 14) < 0.5 && Math.abs(y - i * 9) < 0.5)));
  console.log(`web within half a point of rest at ${settledWeb ? (settledWeb.t * 1000).toFixed(0) : "never"} ms`);
  await writeFile(join(out, "motion-compare.json"), JSON.stringify({ fit: bestFit, worst, web, native: nat }));
} finally {
  await browser.close();
  server.close();
}
