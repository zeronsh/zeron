/**
 * Uploaded pictures (`PUT /auth/me/photo`, `PUT /auth/me/images`), checked
 * strictly and rebuilt before they're stored and served from our origin:
 *
 * - PNG: signature, then chunks whose lengths and CRCs check out, IHDR first
 *   with sane dimensions, at least one IDAT, IEND last. Only the chunks a
 *   picture needs are kept (IHDR, PLTE, tRNS, IDAT, IEND, and gAMA, cHRM,
 *   sRGB for colour). Text, EXIF, ICC profiles and animation are dropped. An
 *   unknown critical chunk refuses the file.
 * - JPEG: SOI, marker segments with valid lengths, one frame header with sane
 *   dimensions, scans up to EOI. Kept: tables (DQT, DHT, DAC, DRI), the frame,
 *   scans, JFIF (APP0) and Adobe (APP14, needed to decode CMYK). EXIF, XMP,
 *   ICC and comments are dropped. The EXIF orientation alone is carried over
 *   in a minimal EXIF of its own, so a phone photo stays upright.
 *
 * Anything after the end marker is dropped. Undefined means the bytes aren't
 * the picture they claim to be.
 */

/** Widest or tallest picture taken. */
export const MAX_DIMENSION = 4096;

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
const PNG_KEEP = new Set(["IHDR", "PLTE", "tRNS", "IDAT", "IEND", "gAMA", "cHRM", "sRGB"]);

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

export const crc32 = (bytes: Uint8Array): number => {
  let c = 0xffffffff;
  for (const b of bytes) c = CRC_TABLE[(c ^ b) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
};

const concat = (parts: readonly Uint8Array[]): Uint8Array => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
};

const dimensionsOk = (width: number, height: number): boolean =>
  width >= 1 && height >= 1 && width <= MAX_DIMENSION && height <= MAX_DIMENSION;

export const cleanPng = (u8: Uint8Array): Uint8Array | undefined => {
  if (u8.length < 8 || !PNG_SIGNATURE.every((b, i) => u8[i] === b)) return undefined;
  const view = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
  const parts: Uint8Array[] = [u8.subarray(0, 8)];
  let at = 8;
  let first = true;
  let idat = false;
  for (;;) {
    if (at + 12 > u8.length) return undefined;
    const len = view.getUint32(at);
    const end = at + 12 + len;
    if (len > 0x7fffffff || end > u8.length) return undefined;
    const typeBytes = u8.subarray(at + 4, at + 8);
    if (!typeBytes.every((b) => (b >= 0x41 && b <= 0x5a) || (b >= 0x61 && b <= 0x7a))) return undefined;
    const type = String.fromCharCode(...typeBytes);
    if (crc32(u8.subarray(at + 4, at + 8 + len)) !== view.getUint32(at + 8 + len)) return undefined;
    if (first) {
      if (type !== "IHDR" || len !== 13) return undefined;
      if (!dimensionsOk(view.getUint32(at + 8), view.getUint32(at + 12))) return undefined;
      first = false;
    } else if (type === "IHDR") {
      return undefined;
    }
    if (type === "IDAT") idat = true;
    const critical = (typeBytes[0]! & 0x20) === 0;
    if (PNG_KEEP.has(type)) parts.push(u8.subarray(at, end));
    else if (critical) return undefined;
    at = end;
    if (type === "IEND") return idat ? concat(parts) : undefined;
  }
};

/** SOFn markers: baseline, extended, progressive, lossless, arithmetic. */
const isFrame = (m: number): boolean => m >= 0xc0 && m <= 0xcf && m !== 0xc4 && m !== 0xc8 && m !== 0xcc;
const JPEG_KEEP = (m: number): boolean =>
  isFrame(m) || m === 0xdb || m === 0xc4 || m === 0xcc || m === 0xdd || m === 0xda;

/** EXIF orientation (1–8) from an APP1 segment's body, if it has one. */
const exifOrientation = (body: Uint8Array): number | undefined => {
  if (body.length < 14 || String.fromCharCode(...body.subarray(0, 6)) !== "Exif\0\0") return undefined;
  const tiff = body.subarray(6);
  const view = new DataView(tiff.buffer, tiff.byteOffset, tiff.byteLength);
  const little = tiff[0] === 0x49 && tiff[1] === 0x49;
  if (!little && !(tiff[0] === 0x4d && tiff[1] === 0x4d)) return undefined;
  const ifd = view.getUint32(4, little);
  if (ifd + 2 > tiff.length) return undefined;
  const count = view.getUint16(ifd, little);
  for (let i = 0; i < count; i++) {
    const entry = ifd + 2 + i * 12;
    if (entry + 12 > tiff.length) return undefined;
    if (view.getUint16(entry, little) === 0x0112 && view.getUint16(entry + 2, little) === 3) {
      const value = view.getUint16(entry + 8, little);
      return value >= 1 && value <= 8 ? value : undefined;
    }
  }
  return undefined;
};

/** APP1 holding an EXIF of one entry: the orientation. */
const orientationSegment = (orientation: number): Uint8Array => {
  const tiff = [0x49, 0x49, 0x2a, 0x00, 8, 0, 0, 0, 1, 0, 0x12, 0x01, 3, 0, 1, 0, 0, 0, orientation, 0, 0, 0, 0, 0, 0, 0];
  const body = [...new TextEncoder().encode("Exif"), 0, 0, ...tiff];
  const len = body.length + 2;
  return new Uint8Array([0xff, 0xe1, len >> 8, len & 0xff, ...body]);
};

export const cleanJpeg = (u8: Uint8Array): Uint8Array | undefined => {
  if (u8.length < 4 || u8[0] !== 0xff || u8[1] !== 0xd8) return undefined;
  const parts: Uint8Array[] = [u8.subarray(0, 2)];
  let orientation: number | undefined;
  let frame = false;
  let scanned = false;
  let at = 2;
  while (at < u8.length) {
    if (u8[at] !== 0xff) return undefined;
    // Fill bytes before a marker.
    while (at < u8.length && u8[at] === 0xff) at++;
    if (at >= u8.length) return undefined;
    const marker = u8[at]!;
    at++;
    if (marker === 0xd9) {
      if (!frame || !scanned) return undefined;
      if (orientation !== undefined && orientation !== 1) parts.splice(1, 0, orientationSegment(orientation));
      parts.push(new Uint8Array([0xff, 0xd9]));
      return concat(parts);
    }
    // No other standalone marker belongs between segments.
    if (marker === 0x00 || marker === 0x01 || marker === 0xd8 || (marker >= 0xd0 && marker <= 0xd7)) return undefined;
    if (at + 2 > u8.length) return undefined;
    const len = (u8[at]! << 8) | u8[at + 1]!;
    const end = at + len;
    if (len < 2 || end > u8.length) return undefined;
    const segment = u8.subarray(at - 2, end);
    const body = u8.subarray(at + 2, end);
    if (isFrame(marker)) {
      if (frame || len < 8) return undefined;
      const height = (body[1]! << 8) | body[2]!;
      const width = (body[3]! << 8) | body[4]!;
      if (!dimensionsOk(width, height)) return undefined;
      frame = true;
    }
    if (marker === 0xe1 && orientation === undefined) orientation = exifOrientation(body);
    const keep =
      JPEG_KEEP(marker) ||
      (marker === 0xe0 && String.fromCharCode(...body.subarray(0, 5)) === "JFIF\0") ||
      (marker === 0xee && String.fromCharCode(...body.subarray(0, 5)) === "Adobe");
    const isApp = marker >= 0xe0 && marker <= 0xef;
    if (!keep && !isApp && marker !== 0xfe) return undefined;
    if (keep) parts.push(segment);
    at = end;
    if (marker === 0xda) {
      if (!frame) return undefined;
      // Entropy-coded data runs to the next marker that isn't a stuffed 0x00
      // or a restart.
      const start = at;
      while (at + 1 < u8.length) {
        if (u8[at] === 0xff) {
          const next = u8[at + 1]!;
          if (next === 0x00 || (next >= 0xd0 && next <= 0xd7) || next === 0xff) {
            at += next === 0xff ? 1 : 2;
            continue;
          }
          break;
        }
        at++;
      }
      if (at + 1 >= u8.length) return undefined;
      parts.push(u8.subarray(start, at));
      scanned = true;
    }
  }
  return undefined;
};

/** The picture, checked and rebuilt, or undefined. */
export const cleanImage = (type: "image/png" | "image/jpeg", bytes: Uint8Array): Uint8Array | undefined =>
  type === "image/png" ? cleanPng(bytes) : cleanJpeg(bytes);
