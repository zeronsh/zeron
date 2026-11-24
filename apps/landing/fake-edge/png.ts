// The fake's template previews: a PNG made here (a template's preview.png is
// a PNG by the store's rules, and the fake carries no binary files), an
// app's window drawn as rounded boxes, once each.

/** A box: x, y, width, height, corner, then its colour (#rrggbb or #rrggbbaa). */
export type Box = [number, number, number, number, number, string];
export interface Picture {
  w: number;
  h: number;
  boxes: Box[];
}

const made = new Map<string, Promise<Uint8Array<ArrayBuffer>>>();
/** What a package's file holds in place of a PNG's bytes: which picture it is (the fake's files are text). */
export const PNG_OF = "\u0089PNG:";
export const pngBytes = (text: string): Promise<Uint8Array<ArrayBuffer>> => {
  let png = made.get(text);
  if (!png) made.set(text, (png = encode(JSON.parse(text.slice(PNG_OF.length)) as Picture)));
  return png;
};

function paint({ w, h, boxes }: Picture): Uint8Array {
  const out = new Uint8Array((w * 3 + 1) * h);
  for (const [x, y, bw, bh, r, colour] of boxes) {
    const at = (i: number) => parseInt(colour.slice(i, i + 2), 16);
    const [red, green, blue, alpha] = [at(1), at(3), at(5), colour.length > 7 ? at(7) / 255 : 1];
    for (let py = Math.max(0, Math.floor(y)); py < Math.min(h, y + bh); py++)
      for (let px = Math.max(0, Math.floor(x)); px < Math.min(w, x + bw); px++) {
        // (How much of the pixel is inside: by its distance into the corner's circle.)
        const dx = Math.max(x + r - (px + 0.5), px + 0.5 - (x + bw - r), 0);
        const dy = Math.max(y + r - (py + 0.5), py + 0.5 - (y + bh - r), 0);
        const cover = r > 0 && dx > 0 && dy > 0 ? Math.min(1, Math.max(0, r - Math.hypot(dx, dy) + 0.5)) : 1;
        const a = alpha * cover;
        const o = py * (w * 3 + 1) + 1 + px * 3;
        out[o] = Math.round(out[o]! * (1 - a) + red * a);
        out[o + 1] = Math.round(out[o + 1]! * (1 - a) + green * a);
        out[o + 2] = Math.round(out[o + 2]! * (1 - a) + blue * a);
      }
  }
  return out;
}

const CRC = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
const crc = (bytes: Uint8Array) => {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC[(c ^ b) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

async function encode(picture: Picture): Promise<Uint8Array<ArrayBuffer>> {
  const { w, h } = picture;
  const pixels = paint(picture) as Uint8Array<ArrayBuffer>;
  const data = new Uint8Array(await new Response(new Blob([pixels]).stream().pipeThrough(new CompressionStream("deflate"))).arrayBuffer());
  const chunk = (type: string, body: Uint8Array) => {
    const out = new Uint8Array(12 + body.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, body.length);
    out.set([...type].map((c) => c.charCodeAt(0)), 4);
    out.set(body, 8);
    view.setUint32(8 + body.length, crc(out.subarray(4, 8 + body.length)));
    return out;
  };
  const head = new Uint8Array(13);
  const view = new DataView(head.buffer);
  view.setUint32(0, w);
  view.setUint32(4, h);
  head.set([8, 2, 0, 0, 0], 8);
  const parts = [new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", head), chunk("IDAT", data), chunk("IEND", new Uint8Array())];
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}
