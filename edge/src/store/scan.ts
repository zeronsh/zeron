/**
 * Reading a package's JSON without building it: a validating scanner that
 * holds a document to serde_json's rules (json.ts says which) and keeps only
 * where the top level's `format`, `manifest` and `files` members are. A
 * package is mostly its files' base64; the store decodes each file's data
 * straight from the request's bytes (`decodeBase64String`), escapes and all,
 * and builds JSON values only for the manifest, under a bound (package.ts).
 *
 * What it costs: the whole document is walked once to check it; the
 * `files` object is walked again to read its members, and the manifest's
 * bytes are parsed once more to build it. Every walk is linear and makes no
 * object per JSON value: positions are the scanner's own fields, the
 * container stack is one array per scanner, numbers are judged in place,
 * and a top-level key is compared as bytes. Only `files`' keys (paths, at
 * most 2,001 distinct ones read) become strings.
 */
import { JsonError, MAX_DEPTH } from "./json";

const isDigit = (b: number): boolean => b >= 0x30 && b <= 0x39;
const isWhitespace = (b: number): boolean => b === 0x20 || b === 0x0a || b === 0x0d || b === 0x09;
const hexValue = (b: number): number => {
  if (b >= 0x30 && b <= 0x39) return b - 0x30;
  if (b >= 0x61 && b <= 0x66) return b - 0x57;
  if (b >= 0x41 && b <= 0x46) return b - 0x37;
  return -1;
};

/** The exact midpoint between the largest double and 2^1024, as 309
 * decimal digits: a decimal at or above it rounds to infinity (ties go to
 * the even 2^1024), which serde_json refuses as out of range. */
const MIDPOINT = (2n ** 1024n - 2n ** 970n).toString();

export class Scanner {
  at = 0;
  /** The span `[start, end)` of the last string read (its content). */
  start = 0;
  end = 0;
  /** The kinds of the open containers (1: array, 2: object), by depth. */
  private readonly stack = new Uint8Array(MAX_DEPTH + 2);
  constructor(readonly bytes: Uint8Array) {}

  fail(why: string): never {
    throw new JsonError(`${why} at byte ${this.at}`);
  }

  skipWhitespace(): void {
    const bytes = this.bytes;
    while (this.at < bytes.length && isWhitespace(bytes[this.at]!)) this.at++;
  }

  /** A string at `at` (its opening quote), checked; its content's span in
   * `start` and `end`. */
  string(): void {
    const bytes = this.bytes;
    let at = this.at + 1;
    const start = at;
    for (;;) {
      const b = bytes[at];
      if (b === undefined) {
        this.at = at;
        this.fail("EOF while parsing a string");
      }
      if (b === 0x22) break;
      if (b >= 0x20 && b < 0x80 && b !== 0x5c) {
        at++;
        continue;
      }
      this.at = at;
      if (b < 0x20) this.fail("control character in a string");
      at = b === 0x5c ? this.escape(at) : this.utf8(at, b);
    }
    this.start = start;
    this.end = at;
    this.at = at + 1;
  }

  /** The escape at `at` (its backslash), checked: where it ends. */
  private escape(at: number): number {
    const bytes = this.bytes;
    const e = bytes[at + 1];
    if (e === 0x75) {
      const unit = this.hex4(at + 2);
      if (unit >= 0xdc00 && unit <= 0xdfff) this.fail("lone trailing surrogate");
      if (unit >= 0xd800 && unit <= 0xdbff) {
        if (bytes[at + 6] !== 0x5c || bytes[at + 7] !== 0x75) this.fail("lone leading surrogate");
        const low = this.hex4(at + 8);
        if (!(low >= 0xdc00 && low <= 0xdfff)) this.fail("lone leading surrogate");
        return at + 12;
      }
      return at + 6;
    }
    if (e === 0x22 || e === 0x5c || e === 0x2f || e === 0x62 || e === 0x66 || e === 0x6e || e === 0x72 || e === 0x74) return at + 2;
    return this.fail("invalid escape");
  }

  private hex4(at: number): number {
    let unit = 0;
    for (let i = 0; i < 4; i++) {
      const digit = hexValue(this.bytes[at + i] ?? -1);
      if (digit < 0) this.fail("invalid \\u escape");
      unit = unit * 16 + digit;
    }
    return unit;
  }

  /** One UTF-8 sequence at `at` starting with `lead` (RFC 3629): where it ends. */
  private utf8(at: number, lead: number): number {
    const bytes = this.bytes;
    let need: number;
    let min: number;
    let cp: number;
    if (lead >= 0xc2 && lead <= 0xdf) (need = 1), (min = 0x80), (cp = lead & 0x1f);
    else if (lead >= 0xe0 && lead <= 0xef) (need = 2), (min = 0x800), (cp = lead & 0x0f);
    else if (lead >= 0xf0 && lead <= 0xf4) (need = 3), (min = 0x10000), (cp = lead & 0x07);
    else return this.fail("invalid UTF-8");
    for (let k = 1; k <= need; k++) {
      const c = bytes[at + k];
      if (c === undefined || (c & 0xc0) !== 0x80) this.fail("invalid UTF-8");
      cp = (cp << 6) | (c & 0x3f);
    }
    if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) this.fail("invalid UTF-8");
    return at + need + 1;
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

  /** Fail unless the float `int.frac e exp` rounds to a finite double:
   * decided from where its first significant digit falls, and only at
   * 10^308 from its digits against the midpoint's, in place. */
  private inRange(intStart: number, intEnd: number, fracStart: number, fracEnd: number, expStart: number, expEnd: number, expNegative: boolean): void {
    const bytes = this.bytes;
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
    let k = 0;
    let order = 0;
    const compare = (from: number, to: number): void => {
      for (let i = from; i < to && order === 0; i++, k++) {
        const d = bytes[i]!;
        if (k < MIDPOINT.length) {
          const m = MIDPOINT.charCodeAt(k);
          if (d !== m) order = d < m ? -1 : 1;
        } else if (d !== 0x30) order = 1;
      }
    };
    if (firstAt < intEnd) {
      compare(firstAt, intEnd);
      compare(fracStart, fracEnd);
    } else compare(firstAt, fracEnd);
    // Fewer digits than the midpoint, all equal so far: below it.
    if (order === 0 && k < MIDPOINT.length) order = -1;
    if (order >= 0) this.fail("number out of range");
  }

  /**
   * A whole value at `at` (after whitespace), inside `depth` open
   * containers (0 for a document; an object's member is inside it), its
   * own containers walked on the scanner's stack. serde_json's limit is on
   * the document: at most MAX_DEPTH containers open at once.
   */
  value(depth = 0): void {
    const base = depth;
    const bytes = this.bytes;
    const stack = this.stack;
    for (;;) {
      this.skipWhitespace();
      const b = bytes[this.at];
      if (b === undefined) this.fail("EOF while parsing a value");
      if (b === 0x5b || b === 0x7b) {
        if (depth + 1 > MAX_DEPTH) this.fail("recursion limit exceeded");
        this.at++;
        this.skipWhitespace();
        if (bytes[this.at] === (b === 0x5b ? 0x5d : 0x7d)) this.at++;
        else {
          stack[++depth] = b === 0x5b ? 1 : 2;
          if (b === 0x7b) this.key();
          continue;
        }
      } else if (b === 0x22) this.string();
      else if (b === 0x2d || isDigit(b)) this.number();
      else if (this.matches("true")) this.at += 4;
      else if (this.matches("false")) this.at += 5;
      else if (this.matches("null")) this.at += 4;
      else this.fail("expected value");
      // Close every container the value completed.
      for (;;) {
        if (depth === base) return;
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

  /** `"key"` then `:`, inside an object: the key's span in `start`/`end`. */
  key(): void {
    this.skipWhitespace();
    if (this.bytes[this.at] !== 0x22) this.fail(this.bytes[this.at] === 0x7d ? "trailing comma" : "key must be a string");
    this.string();
    this.skipWhitespace();
    if (this.bytes[this.at] !== 0x3a) this.fail("expected `:`");
    this.at++;
  }

  private matches(word: string): boolean {
    for (let i = 0; i < word.length; i++) if (this.bytes[this.at + i] !== word.charCodeAt(i)) return false;
    return true;
  }

  /**
   * Each member of the object at `at` (its `{`), the object itself inside
   * `depth` open containers (0 for a document's top level), checked. `each`
   * is told the key's content span and the value's span; once it answers
   * false it isn't called again (the rest is still checked).
   */
  members(depth: number, each: (keyStart: number, keyEnd: number, valueStart: number, valueEnd: number) => boolean | void): void {
    this.skipWhitespace();
    if (this.bytes[this.at] !== 0x7b) this.fail("expected an object");
    if (depth + 1 > MAX_DEPTH) this.fail("recursion limit exceeded");
    this.at++;
    this.skipWhitespace();
    if (this.bytes[this.at] === 0x7d) {
      this.at++;
      return;
    }
    let open = true;
    for (;;) {
      this.key();
      const keyStart = this.start;
      const keyEnd = this.end;
      this.skipWhitespace();
      const valueStart = this.at;
      // The member's value is inside this object: one container deeper.
      this.value(depth + 1);
      if (open && each(keyStart, keyEnd, valueStart, this.at) === false) open = false;
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
        return;
      }
      this.fail(next === undefined ? "EOF while parsing an object" : "expected `,` or `}`");
    }
  }

  /** Nothing but whitespace may follow the document. */
  finish(): void {
    this.skipWhitespace();
    if (this.at !== this.bytes.length) this.fail("trailing characters");
  }
}

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** One `\uXXXX` escape's code unit at `at` (already checked). */
const unitAt = (bytes: Uint8Array, at: number): number =>
  (hexValue(bytes[at]!) << 12) | (hexValue(bytes[at + 1]!) << 8) | (hexValue(bytes[at + 2]!) << 4) | hexValue(bytes[at + 3]!);

const SHORT_ESCAPES: Record<number, number> = { 0x22: 0x22, 0x5c: 0x5c, 0x2f: 0x2f, 0x62: 0x08, 0x66: 0x0c, 0x6e: 0x0a, 0x72: 0x0d, 0x74: 0x09 };

/** The text of a checked string's content `[start, end)`. */
export const stringText = (bytes: Uint8Array, start: number, end: number): string => {
  // A short one of plain ASCII (a path, a key): straight from its bytes.
  if (end - start <= 64) {
    let plain = true;
    for (let i = start; i < end && plain; i++) plain = bytes[i]! < 0x80 && bytes[i] !== 0x5c;
    if (plain) return String.fromCharCode.apply(null, bytes.subarray(start, end) as unknown as number[]);
  }
  let out = "";
  let run = start;
  const units: number[] = [];
  const flushUnits = () => {
    for (let i = 0; i < units.length; i += 8192) out += String.fromCharCode(...units.slice(i, i + 8192));
    units.length = 0;
  };
  let i = start;
  while (i < end) {
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
      units.push(unitAt(bytes, i + 2));
      i += 6;
    } else {
      units.push(SHORT_ESCAPES[e]!);
      i += 2;
    }
    if (units.length >= 8192) flushUnits();
    run = i;
  }
  flushUnits();
  if (end > run) out += utf8.decode(bytes.subarray(run, end));
  return out;
};

/** Whether a checked string's content `[start, end)` is the ASCII text
 * `name`: compared as bytes when it has no escape (the usual case). */
export const stringIs = (bytes: Uint8Array, start: number, end: number, name: string): boolean => {
  if (end - start === name.length) {
    let same = true;
    for (let i = 0; i < name.length && same; i++) same = bytes[start + i] === name.charCodeAt(i);
    if (same) return true;
  }
  // With escapes, at most six bytes a character.
  if (end - start > name.length * 6) return false;
  for (let i = start; i < end; i++) if (bytes[i] === 0x5c) return stringText(bytes, start, end) === name;
  return false;
};

/** The most bytes a string's base64 content can decode to. */
export const decodedBound = (start: number, end: number): number => Math.ceil((end - start) / 4) * 3;

const STANDARD = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const LOOKUP = (() => {
  const t = new Int16Array(128).fill(-1);
  for (let i = 0; i < STANDARD.length; i++) t[STANDARD.charCodeAt(i)] = i;
  return t;
})();

/**
 * Decode a checked string's content `[start, end)` as STANDARD base64
 * (padding required and canonical, zero trailing bits, nothing outside the
 * alphabet: the base64 crate's), unescaping as it goes, into `out` at
 * `at`. Answers the bytes written, or -1 if the text isn't such base64.
 */
export const decodeBase64String = (bytes: Uint8Array, start: number, end: number, out: Uint8Array, at: number): number => {
  let o = at;
  let quad = 0;
  let filled = 0;
  let padding = 0;
  let i = start;
  while (i < end) {
    let c = bytes[i]!;
    if (c === 0x5c) {
      const e = bytes[i + 1]!;
      if (e === 0x75) {
        c = unitAt(bytes, i + 2);
        i += 6;
      } else {
        c = e === 0x2f ? 0x2f : -1; // `\/` is `/`; every other escape is no base64
        i += 2;
      }
    } else i++;
    if (c === 0x3d) {
      if (++padding > 2) return -1;
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
