// Captures the web card in a frozen pose, as the native comparisons need it:
// `node tools/capture-web.mjs --member graham --out web.png [--theme dark]
// [--pose rest|0.25,0.3] [--lit 0|1] [--drift 4,-5|ms] [--frame 420x560]
// [--origin 60,40] [--scale 1] [--dpr 2] [--caption 1] [--route gallery]
// [--tilt 0] [--swiftshader]`. `--route gallery` captures one of the
// comparison fixtures (tools/fixtures/gallery.json) instead of a member.
// The frame is the page's size in CSS px; the card's top-left lands at
// `origin`; the PNG is the frame at `dpr`.
import { writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { launch, settle } from "./browser.mjs";
import { startServer } from "./serve.mjs";

export function argsOf(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith("--")) {
      const next = argv[i + 1];
      if (next === undefined || next.startsWith("--")) out[argv[i].slice(2)] = true;
      else { out[argv[i].slice(2)] = next; i++; }
    }
  }
  return out;
}

export async function captureWeb(browser, url, o) {
  const [fw, fh] = String(o.frame || "420x560").split("x").map(Number);
  const dpr = Number(o.dpr || 2);
  const q = new URLSearchParams();
  q.set("theme", o.theme || "dark");
  q.set("arrive", "0");
  q.set("scale", String(o.scale || 1));
  q.set("origin", o.origin || "60,40");
  q.set("frame", `${fw}x${fh}`);
  q.set("caption", o.caption === "0" ? "0" : "1");
  q.set("pose", o.pose || "rest");
  q.set("lit", o.lit === "1" ? "1" : "0");
  if (o.strength != null) q.set("strength", String(o.strength));
  q.set("drift", o.drift != null ? String(o.drift) : "0");
  if (o.still) q.set("still", "1");
  if (o.tilt === "0" || o.tilt === false) q.set("tilt", "0");
  const page = await browser.newPage();
  const errors = [];
  page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") errors.push(m.text()); });
  page.on("pageerror", (e) => errors.push(String(e)));
  await page.setViewport({ width: fw, height: fh, deviceScaleFactor: dpr });
  const t0 = performance.now();
  const route = o.route === "gallery" ? `/_gallery/${encodeURIComponent(o.member)}/player` : `/@${encodeURIComponent(o.member)}/player`;
  await page.goto(`${url}${route}?${q}`, { waitUntil: "load" });
  await settle(page);
  const ms = Math.round(performance.now() - t0);
  const png = await page.screenshot({ type: "png", clip: { x: 0, y: 0, width: fw, height: fh } });
  await page.close();
  return { png, errors, ms };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const o = argsOf(process.argv.slice(2));
  const { server, url } = await startServer();
  const browser = await launch({ swiftshader: !!o.swiftshader });
  try {
    const { png, errors, ms } = await captureWeb(browser, url, o);
    await writeFile(o.out, png);
    console.log(`${o.out}: ${png.length} bytes, ${ms} ms${errors.length ? `\n${errors.join("\n")}` : ""}`);
  } finally {
    await browser.close();
    server.close();
  }
}
