// Compares a native capture of one of the gallery's cards (the fixtures in
// tools/fixtures/gallery.json; zgx inspect snapshot of the gallery's pane,
// cropped 60 points left and 40 above the card, 420 by 560) with the web
// card in the same pose:
//   node tools/compare-native.mjs --member graham --native region.png
//     --layers before.txt [--layers-after after.txt] --cx 295 --cy 350
//     --theme dark [--pose rest|x,y] [--lit 0|1] [--fit] --out dir --label L
// The layer files hold the card's layer nodes as `zgx inspect tree` printed
// them just before and after the snapshot (`@id img x,y 346x450`, back to
// front): the drift of each layer is read from them (their mean), and with
// `--fit` refined within ±0.75 points (the tree rounds to whole points, and
// the layers move between the reads). Writes <label>-{region,card,column}.png
// side-by-sides and prints the numbers as JSON: for the card and its
// column, and for the art (the card's top 240 points) and the words (its
// bottom 120) apart.
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { PNG } from "pngjs";
import { launch } from "./browser.mjs";
import { argsOf, captureWeb } from "./capture-web.mjs";
import { compare, crop, decode, sideBySide } from "./compare.mjs";
import { startServer } from "./serve.mjs";

const here = fileURLToPath(new URL(".", import.meta.url));
const lerp = (a, b, t) => a + (b - a) * t;

// Each layer's drift in points, from where the gallery drew it.
export function driftFrom(layersText, depths, cx, pose) {
  const lines = layersText.split("\n").map((l) => /img\s+(-?\d+),(-?\d+)\s+(\d+)x(\d+)/.exec(l)).filter(Boolean);
  const px = pose ? pose[0] : 0.5;
  const baseX = cx + 1 - 24;
  return depths.map((d, i) => {
    if (d === 0 || !lines[i]) return 0;
    const x = Number(lines[i][1]);
    const parallax = lerp(d * 8, -d * 8, px);
    return x - baseX - parallax;
  });
}

const REGIONS = { card: [120, 80, 600, 840], column: [112, 80, 616, 904], art: [120, 80, 600, 480], words: [120, 600, 600, 240], region: [0, 0, 840, 1120] };

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const o = argsOf(process.argv.slice(2));
  const members = JSON.parse(await readFile(join(here, "fixtures", "gallery.json"), "utf8")).members;
  const manifest = JSON.parse(await readFile(join(here, "..", "art", "manifest.json"), "utf8"));
  const member = members.find((m) => m.username === o.member);
  const scene = [...manifest].sort((a, b) => a.number - b.number)[member.scene];
  const depths = scene.layers.map((l) => l.depth);
  const pose = o.pose && o.pose !== "rest" ? o.pose.split(",").map(Number) : null;
  const before = o.layers ? await readFile(o.layers, "utf8") : "";
  const after = o["layers-after"] ? await readFile(o["layers-after"], "utf8").catch(() => "") : "";
  const db = driftFrom(before, depths, Number(o.cx), pose);
  const da = after.trim() ? driftFrom(after, depths, Number(o.cx), pose) : db;
  let drift = db.map((v, i) => (v + da[i]) / 2);
  const moving = depths.map((d, i) => i).filter((i) => depths[i] > 0);
  await mkdir(o.out, { recursive: true });
  const native = decode(await readFile(o.native));
  const nativeArt = crop(native, ...REGIONS.art);
  const { server, url } = await startServer();
  const browser = await launch({ swiftshader: !!o.swiftshader });
  const shoot = (d) => captureWeb(browser, url, { member: o.member, theme: o.theme || "dark", pose: o.pose || "rest", lit: o.lit || "0", drift: moving.map((i) => d[i]).join(","), frame: "420x560", origin: "60,40", scale: 1, dpr: 2, caption: "1", tilt: o.tilt === "0" ? "0" : "1", route: "gallery" });
  const score = (png) => compare(nativeArt, crop(decode(png), ...REGIONS.art)).mae.reduce((a, b) => a + b, 0);
  let web, fit = null;
  try {
    web = await shoot(drift);
    if (o.fit && moving.length) {
      fit = [];
      let best = score(web.png);
      // Coarse within ±2 points, then fine around the best.
      for (const i of moving) {
        let bestDelta = 0;
        for (const deltas of [[-2, -1.5, -1, 1, 1.5, 2], [-0.75, -0.5, -0.25, 0.25, 0.5, 0.75]]) {
          const base = bestDelta;
          for (const delta of deltas) {
            const trial = [...drift]; trial[i] += base + delta;
            const shot = await shoot(trial);
            const s = score(shot.png);
            if (s < best) { best = s; bestDelta = base + delta; web = shot; }
          }
        }
        drift[i] += bestDelta;
        fit.push(bestDelta);
      }
    }
  } finally {
    await browser.close();
    server.close();
  }
  await writeFile(join(o.out, `${o.label}-web.png`), web.png);
  const whole = decode(web.png);
  const result = { label: o.label, driftRead: [db, da], drift, fit, webErrors: web.errors.filter((e) => !/404/.test(e)), loadMs: web.ms };
  for (const [name, box] of Object.entries(REGIONS)) {
    const a = crop(native, ...box), b = crop(whole, ...box);
    const r = compare(a, b);
    if (name === "card" || name === "column" || name === "region") await writeFile(join(o.out, `${o.label}-${name}.png`), PNG.sync.write(sideBySide(a, b, r.diff)));
    const { diff, ...rest } = r;
    result[name] = rest;
  }
  console.log(JSON.stringify(result));
}
