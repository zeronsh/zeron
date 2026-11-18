/**
 * Reading a package's JSON without building it: a validating scanner that
 * holds a whole document to serde_json's rules (json.ts says which) while
 * allocating nothing per value, and keeps only where the top level's
 * `format`, `manifest` and `files` members are. A package is mostly its
 * files' base64; the store reads each file's data straight from the
 * request's bytes (`decodeBase64String`), escapes and all, and builds JSON
 * values only for the manifest, under a bound (package.ts).
 *
 * So what a hostile body costs is linear in its size and bounded in memory,
 * whatever its shape: an 8 MB array of zeros, 4 MB of digits in one number,
 * a string of `\/` escapes, a million empty objects. Every such document is
 * walked once, byte by byte, with no allocation but a 128-entry stack.
 */
import { JsonError, MAX_DEPTH } from "./json";

/** Bytes `[start, end)` of the input. For a string, its content between
 * the quotes. */
export interface Span {
  readonly start: number;
  readonly end: number;
}

const isDigit = (b: number): boolean => b >= 0x30 && b <= 0x39;
const isWhitespace = (b: number): boolean => b === 0x20 || b === 0x0a || b === 0x0d || b === 0x09;
const hexValue = (b: number): number => {
  if (b >= 0x30 && b <= 0x39) return b - 0x30;
  if (b >= 0x61 && b <= 0x66) return b - 0x57;
  if (b >= 0x41 && b <= 0x46) return b - 0x37;
  return -1;
};

/** Significant digits kept to decide a number's range: with a sticky digit
 * for the rest, enough for any double's correct rounding (768 suffice). */
const KEEP = 800;

export class Scanner {
  at = 0;
  /** The first significant digits of the number being scanned. */
  private readonly digits = new Uint8Array(KEEP);
  constructor(readonly bytes: Uint8Array) {}

  fail(why: string): never {
    throw new JsonError(`${why} at byte ${this.at}`);
  }

  skipWhitespace(): void {
    const bytes = this.bytes;
    while (this.at < bytes.length && isWhitespace(bytes[this.at]!)) this.at++;
  }

  /** A string at `at` (its opening quote): checked, its content's span. */
  string(): Span {
    const bytes = this.bytes;
    this.at++;
    const start = this.at;
    for (;;) {
      const b = bytes[this.at];
      if (b === undefined) this.fail("EOF while parsing a string");
      if (b === 0x22) {
        const end = this.at++;
        return { start, end };
      }
      if (b < 0x20) this.fail("control character in a string");
      if (b === 0x5c) {
        this.at++;
        const e = bytes[this.at++];
        if (e === 0x75) {
          const unit = this.hex4();
          if (unit >= 0xdc00 && unit <= 0xdfff) this.fail("lone trailing surrogate");
          if (unit >= 0xd800 && unit <= 0xdbff) {
            if (bytes[this.at] !== 0x5c || bytes[this.at + 1] !== 0x75) this.fail("lone leading surrogate");
            this.at += 2;
            const low = this.hex4();
            if (!(low >= 0xdc00 && low <= 0xdfff)) this.fail("lone leading surrogate");
          }
        } else if (!(e === 0x22 || e === 0x5c || e === 0x2f || e === 0x62 || e === 0x66 || e === 0x6e || e === 0x72 || e === 0x74)) {
          this.fail("invalid escape");
        }
        continue;
      }
      if (b < 0x80) {
        this.at++;
        continue;
      }
      this.utf8(b);
    }
  }

  /** One UTF-8 sequence starting with `lead` (RFC 3629). */
  private utf8(lead: number): void {
    const bytes = this.bytes;
    let need: number;
    let min: number;
    let cp: number;
    if (lead >= 0xc2 && lead <= 0xdf) (need = 1), (min = 0x80), (cp = lead & 0x1f);
    else if (lead >= 0xe0 && lead <= 0xef) (need = 2), (min = 0x800), (cp = lead & 0x0f);
    else if (lead >= 0xf0 && lead <= 0xf4) (need = 3), (min = 0x10000), (cp = lead & 0x07);
    else return this.fail("invalid UTF-8");
    for (let k = 1; k <= need; k++) {
      const c = bytes[this.at + k];
      if (c === undefined || (c & 0xc0) !== 0x80) this.fail("invalid UTF-8");
      cp = (cp << 6) | (c & 0x3f);
    }
    if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) this.fail("invalid UTF-8");
    this.at += need + 1;
  }

  private hex4(): number {
    let unit = 0;
    for (let i = 0; i < 4; i++) {
      const digit = hexValue(this.bytes[this.at++] ?? -1);
      if (digit < 0) this.fail("invalid \\u escape");
      unit = unit * 16 + digit;
    }
    return unit;
  }

  /** A number at `at`, checked as serde_json reads one: its grammar, and
   * (as a float) that it isn't out of range. One pass over its bytes; the
   * range is worked out only for a long integer or a float. */
  number(): void {
    const bytes = this.bytes;
    const n = bytes.length;
    let at = this.at;
    if (bytes[at] === 0x2d) at++;
    const intStart = at;
    const first = bytes[at];
    if (first === undefined || first < 0x30 || first > 0x39) {
      this.at = at;
      this.fail("invalid number");
    }
    at++;
    if (first === 0x30) {
      const next = bytes[at];
      if (next !== undefined && next >= 0x30 && next <= 0x39) {
        this.at = at;
        this.fail("invalid number");
      }
    } else {
      while (at < n && bytes[at]! >= 0x30 && bytes[at]! <= 0x39) at++;
    }
    const intEnd = at;
    let fracStart = at;
    let fracEnd = at;
    let float = false;
    if (bytes[at] === 0x2e) {
      float = true;
      at++;
      fracStart = at;
      while (at < n && bytes[at]! >= 0x30 && bytes[at]! <= 0x39) at++;
      fracEnd = at;
      if (fracEnd === fracStart) {
        this.at = at;
        this.fail("invalid number");
      }
    }
    let expStart = at;
    let expEnd = at;
    let expNegative = false;
    if (bytes[at] === 0x65 || bytes[at] === 0x45) {
      float = true;
      at++;
      if (bytes[at] === 0x2b || bytes[at] === 0x2d) expNegative = bytes[at++] === 0x2d;
      expStart = at;
      while (at < n && bytes[at]! >= 0x30 && bytes[at]! <= 0x39) at++;
      expEnd = at;
      if (expEnd === expStart) {
        this.at = at;
        this.fail("invalid number");
      }
    }
    this.at = at;
    // An integer within u64/i64 is in range; anything else is a float.
    if (!float && intEnd - intStart <= 19) return;
    this.inRange(intStart, intEnd, fracStart, fracEnd, expStart, expEnd, expNegative);
  }

  /** Fail unless the float `int.frac e exp` rounds to a finite double. */
  private inRange(intStart: number, intEnd: number, fracStart: number, fracEnd: number, expStart: number, expEnd: number, expNegative: boolean): void {
    const bytes = this.bytes;
    // The first significant digit, and its decimal exponent before `exp`.
    let firstAt = -1;
    let place = 0;
    for (let i = intStart; i < intEnd; i++) {
      if (bytes[i] !== 0x30) {
        firstAt = i;
        place = intEnd - i - 1;
        break;
      }
    }
    if (firstAt < 0) {
      for (let i = fracStart; i < fracEnd; i++) {
        if (bytes[i] !== 0x30) {
          firstAt = i;
          place = -(i - fracStart + 1);
          break;
        }
      }
    }
    if (firstAt < 0) return; // zero
    let exponent = 0;
    for (let i = expStart; i < expEnd && exponent < 1e9; i++) exponent = exponent * 10 + (bytes[i]! - 0x30);
    const e = place + (expNegative ? -exponent : exponent);
    if (e < 308) return;
    if (e > 308) this.fail("number out of range");
    // On the edge of the range: the first KEEP significant digits, a sticky
    // 1 for any non-zero after them, rounded as serde_json (float_roundtrip)
    // and V8 both round, correctly.
    let kept = 0;
    let sticky = false;
    const take = (from: number, to: number) => {
      for (let i = from; i < to; i++) {
        if (kept < KEEP) this.digits[kept++] = bytes[i]!;
        else if (bytes[i] !== 0x30) sticky = true;
      }
    };
    if (firstAt < intEnd) {
      take(firstAt, intEnd);
      take(fracStart, fracEnd);
    } else take(firstAt, fracEnd);
    let text = "0.";
    for (let i = 0; i < kept; i++) text += String.fromCharCode(this.digits[i]!);
    if (sticky) text += "1";
    if (!Number.isFinite(Number(`${text}e${e + 1}`))) this.fail("number out of range");
  }

  /** A whole value at `at` (after whitespace), its containers walked with
   * a stack of their kinds; answers its span. */
  value(): Span {
    this.skipWhitespace();
    const start = this.at;
    const bytes = this.bytes;
    const stack = new Uint8Array(MAX_DEPTH + 1); // 1: array, 2: object
    let depth = 0;
    for (;;) {
      this.skipWhitespace();
      const b = bytes[this.at];
      if (b === undefined) this.fail("EOF while parsing a value");
      let opened = false;
      if (b === 0x5b || b === 0x7b) {
        if (depth + 1 > MAX_DEPTH) this.fail("recursion limit exceeded");
        this.at++;
        this.skipWhitespace();
        const close = b === 0x5b ? 0x5d : 0x7d;
        if (bytes[this.at] === close) this.at++;
        else {
          stack[++depth] = b === 0x5b ? 1 : 2;
          opened = true;
          if (b === 0x7b) this.key();
        }
      } else if (b === 0x22) this.string();
      else if (b === 0x2d || isDigit(b)) this.number();
      else if (this.matches("true")) this.at += 4;
      else if (this.matches("false")) this.at += 5;
      else if (this.matches("null")) this.at += 4;
      else this.fail("expected value");
      if (opened) continue;
      // Close every container the value completed.
      for (;;) {
        if (depth === 0) return { start, end: this.at };
        this.skipWhitespace();
        const next = bytes[this.at];
        const isArray = stack[depth] === 1;
        if (next === 0x2c) {
          this.at++;
          if (isArray) {
            this.skipWhitespace();
            if (bytes[this.at] === 0x5d) this.fail("trailing comma");
          } else this.key();
          break;
        }
        if (next === (isArray ? 0x5d : 0x7d)) {
          this.at++;
          depth--;
          continue;
        }
        this.fail(next === undefined ? "EOF while parsing a container" : "expected `,` or the container's end");
      }
    }
  }

  /** `"key"` then `:`, inside an object: the key's span. */
  key(): Span {
    this.skipWhitespace();
    if (this.bytes[this.at] !== 0x22) this.fail(this.bytes[this.at] === 0x7d ? "trailing comma" : "key must be a string");
    const span = this.string();
    this.skipWhitespace();
    if (this.bytes[this.at] !== 0x3a) this.fail("expected `:`");
    this.at++;
    return span;
  }

  private matches(word: string): boolean {
    for (let i = 0; i < word.length; i++) if (this.bytes[this.at + i] !== word.charCodeAt(i)) return false;
    return true;
  }

  /** Each member of the object at `at` (`{`), checked, as spans: the key's
   * content and the value. Stops early when `each` answers false (the
   * rest is still checked). */
  members(each: (key: Span, value: Span) => boolean | void): Span {
    this.skipWhitespace();
    const start = this.at;
    if (this.bytes[this.at] !== 0x7b) this.fail("expected an object");
    this.at++;
    this.skipWhitespace();
    let open = true;
    if (this.bytes[this.at] === 0x7d) {
      this.at++;
      return { start, end: this.at };
    }
    for (;;) {
      const key = this.key();
      const value = this.value();
      if (open && each(key, value) === false) open = false;
      this.skipWhitespace();
      const next = this.bytes[this.at];
      if (next === 0x2c) {
        this.at++;
        this.skipWhitespace();
        if (this.bytes[this.at] === 0x7d) this.fail("trailing comma");
        continue;
      }
      if (next === 0x7d) {
        this.at++;
        return { start, end: this.at };
      }
      this.fail(next === undefined ? "EOF while parsing an object" : "expected `,` or `}`");
    }
  }

  end(): void {
    this.skipWhitespace();
    if (this.at !== this.bytes.length) this.fail("trailing characters");
  }
}

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** The text of a string's content span (already checked by the scanner). */
export const stringText = (bytes: Uint8Array, span: Span): string => {
  let out = "";
  let run = span.start;
  const units: number[] = [];
  const flushUnits = () => {
    for (let i = 0; i < units.length; i += 8192) out += String.fromCharCode(...units.slice(i, i + 8192));
    units.length = 0;
  };
  let i = span.start;
  while (i < span.end) {
    if (bytes[i] !== 0x5c) {
      i++;
      continue;
    }
    if (i > run) {
      flushUnits();
      out += utf8.decode(bytes.subarray(run, i));
    }
    const e = bytes[i + 1]!;
    if (e === 0x75) {
      units.push(parseInt(String.fromCharCode(...bytes.subarray(i + 2, i + 6)), 16));
      i += 6;
    } else {
      units.push({ 0x22: 0x22, 0x5c: 0x5c, 0x2f: 0x2f, 0x62: 0x08, 0x66: 0x0c, 0x6e: 0x0a, 0x72: 0x0d, 0x74: 0x09 }[e]!);
      i += 2;
    }
    if (units.length >= 8192) flushUnits();
    run = i;
  }
  flushUnits();
  if (span.end > run) out += utf8.decode(bytes.subarray(run, span.end));
  return out;
};

/** The most bytes a string span's base64 can decode to. */
export const spanDecodedBound = (span: Span): number => Math.ceil((span.end - span.start) / 4) * 3;

const STANDARD = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const LOOKUP = (() => {
  const t = new Int16Array(128).fill(-1);
  for (let i = 0; i < STANDARD.length; i++) t[STANDARD.charCodeAt(i)] = i;
  return t;
})();

/**
 * Decode a checked string's content as STANDARD base64 (padding required
 * and canonical, zero trailing bits, nothing outside the alphabet: the
 * base64 crate's), unescaping as it goes, into `out` at `at`. Answers the
 * bytes written, or -1 if the string's text isn't such base64.
 */
export const decodeBase64String = (bytes: Uint8Array, span: Span, out: Uint8Array, at: number): number => {
  let o = at;
  let quad = 0;
  let filled = 0;
  let padding = 0;
  let i = span.start;
  while (i < span.end) {
    let c = bytes[i]!;
    if (c === 0x5c) {
      const e = bytes[i + 1]!;
      if (e === 0x75) {
        c = parseInt(String.fromCharCode(...bytes.subarray(i + 2, i + 6)), 16);
        i += 6;
      } else {
        c = e === 0x2f ? 0x2f : -1; // `\/` is `/`; every other escape is no base64
        i += 2;
      }
    } else i++;
    if (c === 0x3d) {
      padding++;
      if (padding > 2) return -1;
      continue;
    }
    if (padding > 0) return -1; // anything after padding
    const v = c >= 0 && c < 128 ? LOOKUP[c]! : -1;
    if (v < 0) return -1;
    quad = (quad << 6) | v;
    if (++filled === 4) {
      out[o++] = (quad >> 16) & 0xff;
      out[o++] = (quad >> 8) & 0xff;
      out[o++] = quad & 0xff;
      quad = 0;
      filled = 0;
    }
  }
  // The text's length is a multiple of four, padding included.
  if ((filled + padding) % 4 !== 0) return -1;
  if (padding === 0) return filled === 0 ? o - at : -1;
  if (padding === 2 && filled === 2) {
    if ((quad & 0x0f) !== 0) return -1;
    out[o++] = (quad >> 4) & 0xff;
  } else if (padding === 1 && filled === 3) {
    if ((quad & 0x03) !== 0) return -1;
    out[o++] = (quad >> 10) & 0xff;
    out[o++] = (quad >> 2) & 0xff;
  } else return -1;
  return o - at;
};
