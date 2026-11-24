// Characters as markdown reads them: ASCII punctuation, backslash escapes,
// character references. Shared by the block pass (a definition's address,
// undone once) and the inline pass.

import { decodeNamedCharacterReference } from "decode-named-character-reference";
import type { Meter } from "./meter.ts";

export const PUNCTUATION = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
const ASCII_PUNCTUATION = new Uint8Array(128);
for (const c of PUNCTUATION) ASCII_PUNCTUATION[c.charCodeAt(0)] = 1;

export const isAsciiPunctuation = (c: string | undefined) => c !== undefined && c.length === 1 && c.charCodeAt(0) < 128 && ASCII_PUNCTUATION[c.charCodeAt(0)] === 1;

const isHex = (c: string | undefined) => c !== undefined && ((c >= "0" && c <= "9") || (c >= "a" && c <= "f") || (c >= "A" && c <= "F"));
const isAlnum = (c: string | undefined) => c !== undefined && ((c >= "0" && c <= "9") || (c >= "a" && c <= "z") || (c >= "A" && c <= "Z"));

/** A character reference at `i`: what it stands for and where it ends, or null. (At most 34 characters read.) */
export function reference(s: string, i: number): { text: string; end: number } | null {
  let j = i + 1;
  if (s[j] === "#") {
    j++;
    const hex = s[j] === "x" || s[j] === "X";
    if (hex) j++;
    const from = j;
    while (j - from < (hex ? 6 : 7) && (hex ? isHex(s[j]) : s[j] !== undefined && s[j] >= "0" && s[j] <= "9")) j++;
    if (j === from || s[j] !== ";") return null;
    const code = parseInt(s.slice(from, j), hex ? 16 : 10);
    const ok = code > 0 && code <= 0x10ffff && !(code >= 0xd800 && code <= 0xdfff);
    return { text: ok ? String.fromCodePoint(code) : "�", end: j + 1 };
  }
  const from = j;
  while (j - from < 32 && isAlnum(s[j])) j++;
  if (j === from || s[j] !== ";") return null;
  const decoded = decodeNamedCharacterReference(s.slice(from, j));
  return decoded === false ? null : { text: decoded, end: j + 1 };
}

/** Backslash escapes and character references undone (an address's, a title's, an info string's). */
export function unescape(s: string, m?: Meter): string {
  if (m) m.read += s.length;
  if (!s.includes("\\") && !s.includes("&")) return s;
  const out: string[] = [];
  let from = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === "\\" && isAsciiPunctuation(s[i + 1])) {
      out.push(s.slice(from, i));
      from = ++i;
    } else if (c === "&") {
      const ref = reference(s, i);
      if (ref) {
        out.push(s.slice(from, i), ref.text);
        i = ref.end - 1;
        from = ref.end;
      }
    }
  }
  out.push(s.slice(from));
  return out.join("");
}
