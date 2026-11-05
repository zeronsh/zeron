/**
 * A PNG stripped to its picture: IHDR, PLTE, tRNS, IDAT and IEND kept, every
 * other chunk (tEXt, zTXt, iTXt, eXIf, tIME, pHYs, iCCP …) dropped, so a
 * generated card picture carries no metadata. apps/card-web/tools/png-strip.mjs
 * is the same in JavaScript.
 */
const KEEP = new Set(["IHDR", "PLTE", "tRNS", "IDAT", "IEND"]);
const SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

export const isPng = (u8: Uint8Array): boolean => u8.length >= 8 && SIGNATURE.every((b, i) => u8[i] === b);

export const stripPng = (bytes: Uint8Array | ArrayBuffer): Uint8Array => {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  if (!isPng(u8)) throw new Error("not a PNG");
  const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  const parts: Uint8Array[] = [u8.subarray(0, 8)];
  let at = 8;
  while (at + 8 <= u8.length) {
    const len = view.getUint32(at);
    const type = String.fromCharCode(u8[at + 4]!, u8[at + 5]!, u8[at + 6]!, u8[at + 7]!);
    const end = at + 12 + len;
    if (end > u8.length) throw new Error("truncated PNG");
    if (KEEP.has(type)) parts.push(u8.subarray(at, end));
    at = end;
    if (type === "IEND") break;
  }
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
};

/** The chunk types of a PNG, in order (a test's eyes). */
export const pngChunkTypes = (bytes: Uint8Array): string[] => {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const types: string[] = [];
  let at = 8;
  while (at + 8 <= bytes.length) {
    const len = view.getUint32(at);
    types.push(String.fromCharCode(bytes[at + 4]!, bytes[at + 5]!, bytes[at + 6]!, bytes[at + 7]!));
    at += 12 + len;
  }
  return types;
};
