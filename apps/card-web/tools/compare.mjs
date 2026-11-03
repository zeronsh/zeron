// Compares two PNGs of the same size: `node tools/compare.mjs a.png b.png
// [out.png] [--crop x,y,w,h]` prints the difference (mean absolute error
// per channel, the share of pixels off by more than 8 and 32 of 255, PSNR)
// and writes a side-by-side: A, B, and the difference amplified four times.
import { readFile, writeFile } from "node:fs/promises";
import { PNG } from "pngjs";

export function decode(buffer) { return PNG.sync.read(buffer); }

export function crop(png, x, y, w, h) {
  const out = new PNG({ width: w, height: h });
  PNG.bitblt(png, out, x, y, w, h, 0, 0);
  return out;
}

export function compare(a, b) {
  if (a.width !== b.width || a.height !== b.height) throw new Error(`sizes differ: ${a.width}x${a.height} vs ${b.width}x${b.height}`);
  const n = a.width * a.height;
  let sum = [0, 0, 0], over8 = 0, over32 = 0, sq = 0, max = 0;
  const diff = new PNG({ width: a.width, height: a.height });
  for (let i = 0; i < n; i++) {
    const o = i * 4;
    // Over an opaque page both are opaque; compare the colour as shown.
    let m = 0;
    for (let c = 0; c < 3; c++) {
      const d = Math.abs(a.data[o + c] - b.data[o + c]);
      sum[c] += d; sq += d * d; if (d > m) m = d;
      diff.data[o + c] = Math.min(255, d * 4);
    }
    diff.data[o + 3] = 255;
    if (m > 8) over8++;
    if (m > 32) over32++;
    if (m > max) max = m;
  }
  const mse = sq / (n * 3);
  return {
    width: a.width, height: a.height,
    mae: sum.map((s) => +(s / n).toFixed(3)),
    over8: +((over8 / n) * 100).toFixed(3), over32: +((over32 / n) * 100).toFixed(3),
    max, psnr: mse > 0 ? +(10 * Math.log10((255 * 255) / mse)).toFixed(2) : Infinity,
    diff
  };
}

export function sideBySide(a, b, diff, gap = 8) {
  const out = new PNG({ width: a.width * 3 + gap * 2, height: a.height });
  out.data.fill(40);
  for (let i = 3; i < out.data.length; i += 4) out.data[i] = 255;
  PNG.bitblt(a, out, 0, 0, a.width, a.height, 0, 0);
  PNG.bitblt(b, out, 0, 0, b.width, b.height, a.width + gap, 0);
  PNG.bitblt(diff, out, 0, 0, diff.width, diff.height, (a.width + gap) * 2, 0);
  return out;
}

if (process.argv[1] && import.meta.url.endsWith(process.argv[1].split("/").pop())) {
  const args = process.argv.slice(2);
  const ci = args.indexOf("--crop");
  let box = null;
  if (ci >= 0) { box = args[ci + 1].split(",").map(Number); args.splice(ci, 2); }
  const [pa, pb, out] = args;
  let a = decode(await readFile(pa)), b = decode(await readFile(pb));
  if (box) { a = crop(a, ...box); b = crop(b, ...box); }
  const r = compare(a, b);
  if (out) await writeFile(out, PNG.sync.write(sideBySide(a, b, r.diff)));
  const { diff, ...rest } = r;
  console.log(JSON.stringify(rest));
}
