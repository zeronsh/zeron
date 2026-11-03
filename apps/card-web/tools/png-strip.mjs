// Strips a PNG to its picture: keeps IHDR, PLTE, tRNS, IDAT and IEND, drops
// every other chunk (tEXt, zTXt, iTXt, eXIf, tIME, pHYs, iCCP …), so a
// generated picture carries no metadata. The edge has the same in TypeScript
// (edge/src/png.ts).
const KEEP = new Set(["IHDR", "PLTE", "tRNS", "IDAT", "IEND"]);
export function stripPng(bytes) {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const sig = [137, 80, 78, 71, 13, 10, 26, 10];
  if (u8.length < 8 || sig.some((b, i) => u8[i] !== b)) throw new Error("not a PNG");
  const out = [u8.subarray(0, 8)];
  let at = 8;
  const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  while (at + 8 <= u8.length) {
    const len = view.getUint32(at);
    const type = String.fromCharCode(u8[at + 4], u8[at + 5], u8[at + 6], u8[at + 7]);
    const end = at + 12 + len;
    if (end > u8.length) throw new Error("truncated PNG");
    if (KEEP.has(type)) out.push(u8.subarray(at, end));
    at = end;
    if (type === "IEND") break;
  }
  const total = out.reduce((n, p) => n + p.length, 0);
  const joined = new Uint8Array(total);
  let o = 0;
  for (const p of out) { joined.set(p, o); o += p.length; }
  return joined;
}
export function pngChunkTypes(bytes) {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  const types = [];
  let at = 8;
  while (at + 8 <= u8.length) {
    const len = view.getUint32(at);
    types.push(String.fromCharCode(u8[at + 4], u8[at + 5], u8[at + 6], u8[at + 7]));
    at += 12 + len;
  }
  return types;
}
