/**
 * Byte/text codecs shared by the vault. Hand-rolled on purpose: the vault has
 * zero runtime dependencies, and everything here must run identically in
 * workerd and in Node (the unit tier).
 */

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });

export const utf8 = (text: string): Uint8Array<ArrayBuffer> => encoder.encode(text) as Uint8Array<ArrayBuffer>;
export const fromUtf8 = (bytes: Uint8Array): string => decoder.decode(bytes);

export const toHex = (bytes: ArrayBuffer | Uint8Array): string => {
  const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let out = "";
  for (const byte of view) out += byte.toString(16).padStart(2, "0");
  return out;
};

export const toBase64 = (bytes: Uint8Array): string => {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
};

/** Strict standard base64 (padding optional). Throws on anything else. */
export const fromBase64 = (text: string): Uint8Array<ArrayBuffer> => {
  const trimmed = text.trim();
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(trimmed)) throw new Error("invalid base64");
  const binary = atob(trimmed);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
};

export const toBase64Url = (bytes: Uint8Array): string =>
  toBase64(bytes).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/** base64url without padding (padding tolerated). Throws on other alphabets. */
export const fromBase64Url = (text: string): Uint8Array<ArrayBuffer> => {
  if (!/^[A-Za-z0-9_-]*={0,2}$/.test(text)) throw new Error("invalid base64url");
  const std = text.replace(/=+$/, "").replace(/-/g, "+").replace(/_/g, "/");
  return fromBase64(std + "=".repeat((4 - (std.length % 4)) % 4));
};

export const randomId = (bytes = 16): string => toBase64Url(crypto.getRandomValues(new Uint8Array(bytes)));
