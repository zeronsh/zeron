/**
 * JSON exactly as the engine reads and writes it (serde_json, built with
 * `float_roundtrip` and without `preserve_order`), for what a signature and a
 * content hash cover (crates/engine/src/plugins/signing.rs `canonical_json`).
 *
 * Why not `JSON.parse`: it keeps neither a number's kind (serde_json prints
 * the integer `1` as `1` and the float `1.0` as `1.0`), nor refuses what
 * serde_json refuses (lone surrogate escapes, nesting past its limit, a
 * byte-order mark), so the bytes a signature covers would differ.
 *
 * What serde_json does, and this does:
 * - input is UTF-8; any invalid byte, a BOM, or anything after the value but
 *   JSON whitespace is an error;
 * - at most 127 containers deep (`remaining_depth` 128, an error at 0);
 * - strings: no raw control characters, escapes `\" \\ \/ \b \f \n \r \t
 *   \uXXXX`, a `\u` surrogate only as a high-low pair;
 * - numbers: `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`. Without a
 *   fraction or exponent: an unsigned 64-bit integer, or a signed one when
 *   negative (`-0` is the float -0.0); anything else, or out of those ranges,
 *   a float, correctly rounded (an infinite one is an error);
 * - duplicate keys: the last one wins.
 *
 * The canonical form (`canonicalJson`): objects with keys sorted bytewise on
 * UTF-8, no whitespace, strings escaped as serde_json escapes them, integers
 * in decimal and floats as serde_json prints them (`1.0`, `1e+21`, `1.5e-7`).
 */

export class JsonError extends Error {}

/** A number as serde_json holds it: an integer (in decimal) or a float. */
export class JsonNumber {
  private constructor(
    /** The integer's decimal digits (with `-`), or null for a float. */
    readonly integer: string | null,
    /** The float's value; for an integer, its nearest double. */
    readonly value: number
  ) {}

  static int(text: string): JsonNumber {
    return new JsonNumber(text, Number(text));
  }

  static float(value: number): JsonNumber {
    return new JsonNumber(null, value);
  }
}

export type Json = null | boolean | string | JsonNumber | Json[] | JsonObject;

/**
 * A string left in the input as a byte range (`parseJson`'s `raw` option):
 * one of at least `RAW_MIN` bytes, all ASCII, with no escapes, so its text
 * is exactly those bytes. A package's file data is read this way, straight
 * from the request's bytes, never copied into a string first.
 */
export class RawString {
  constructor(
    readonly bytes: Uint8Array,
    readonly start: number,
    readonly end: number
  ) {}

  get length(): number {
    return this.end - this.start;
  }

  text(): string {
    let out = "";
    for (let i = this.start; i < this.end; i += 8192) out += String.fromCharCode(...this.bytes.subarray(i, Math.min(i + 8192, this.end)));
    return out;
  }
}

export const RAW_MIN = 64;
/** Insertion order is irrelevant: serde_json's map is sorted. */
export type JsonObject = Map<string, Json>;

export const isObject = (value: Json | undefined): value is JsonObject => value instanceof Map;
export const isArray = (value: Json | undefined): value is Json[] => Array.isArray(value);

/** serde_json's recursion limit: containers nested deeper than this fail. */
export const MAX_DEPTH = 127;

const U64_MAX = 18446744073709551615n;
const I64_MIN = -9223372036854775808n;

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const encoder = new TextEncoder();

const isDigit = (b: number): boolean => b >= 0x30 && b <= 0x39;
const isWhitespace = (b: number): boolean => b === 0x20 || b === 0x0a || b === 0x0d || b === 0x09;

const hexValue = (b: number): number => {
  if (b >= 0x30 && b <= 0x39) return b - 0x30;
  if (b >= 0x61 && b <= 0x66) return b - 0x57;
  if (b >= 0x41 && b <= 0x46) return b - 0x37;
  return -1;
};

class Parser {
  at = 0;
  constructor(
    private readonly bytes: Uint8Array,
    /** Leave long plain strings as `RawString`s (typed as strings: only
     * `parsePackageJson`'s callers see them). */
    private readonly raw = false
  ) {}

  fail(why: string): never {
    throw new JsonError(`${why} at byte ${this.at}`);
  }

  skipWhitespace(): void {
    while (this.at < this.bytes.length && isWhitespace(this.bytes[this.at]!)) this.at++;
  }

  /** One value, its containers iteratively so depth costs no stack. */
  value(): Json {
    type Frame = { readonly container: Json[] | JsonObject; key: string | null };
    const stack: Frame[] = [];
    let result: Json = null;
    for (;;) {
      this.skipWhitespace();
      let value: Json;
      let opened: Frame | undefined;
      const b = this.bytes[this.at];
      if (b === undefined) this.fail("EOF while parsing a value");
      if (b === 0x5b || b === 0x7b) {
        if (stack.length + 1 > MAX_DEPTH) this.fail("recursion limit exceeded");
        this.at++;
        this.skipWhitespace();
        const container: Json[] | JsonObject = b === 0x5b ? [] : new Map();
        const close = b === 0x5b ? 0x5d : 0x7d;
        if (this.bytes[this.at] === close) {
          this.at++;
          value = container;
        } else {
          opened = { container, key: null };
          stack.push(opened);
          if (b === 0x7b) opened.key = this.objectKey();
          continue;
        }
      } else {
        value = this.scalar();
      }
      // Place `value`, then close every container it completes.
      for (;;) {
        const frame = stack[stack.length - 1];
        if (!frame) {
          result = value;
          return result;
        }
        if (Array.isArray(frame.container)) frame.container.push(value);
        else frame.container.set(frame.key!, value);
        this.skipWhitespace();
        const next = this.bytes[this.at];
        const isArrayFrame = Array.isArray(frame.container);
        if (next === 0x2c) {
          this.at++;
          if (!isArrayFrame) frame.key = this.objectKey();
          else {
            this.skipWhitespace();
            if (this.bytes[this.at] === 0x5d) this.fail("trailing comma");
          }
          break;
        }
        if (next === (isArrayFrame ? 0x5d : 0x7d)) {
          this.at++;
          stack.pop();
          value = frame.container;
          continue;
        }
        this.fail(next === undefined ? "EOF while parsing a container" : "expected `,` or the container's end");
      }
    }
  }

  /** `"key"` then `:`, inside an object. */
  objectKey(): string {
    this.skipWhitespace();
    if (this.bytes[this.at] !== 0x22) this.fail(this.bytes[this.at] === 0x7d ? "trailing comma" : "key must be a string");
    // (Keys are always strings: a duplicate must find its first.)
    const key = this.string(false);
    this.skipWhitespace();
    if (this.bytes[this.at] !== 0x3a) this.fail("expected `:`");
    this.at++;
    return key;
  }

  scalar(): Json {
    const b = this.bytes[this.at]!;
    if (b === 0x22) return this.string();
    if (b === 0x2d || isDigit(b)) return this.number();
    for (const [word, value] of [
      ["true", true],
      ["false", false],
      ["null", null]
    ] as const) {
      if (this.matches(word)) {
        this.at += word.length;
        return value;
      }
    }
    return this.fail("expected value");
  }

  matches(word: string): boolean {
    for (let i = 0; i < word.length; i++) if (this.bytes[this.at + i] !== word.charCodeAt(i)) return false;
    return true;
  }

  number(): JsonNumber {
    const start = this.at;
    const bytes = this.bytes;
    if (bytes[this.at] === 0x2d) this.at++;
    const first = bytes[this.at];
    if (first === undefined || !isDigit(first)) this.fail("invalid number");
    this.at++;
    if (first === 0x30) {
      if (this.at < bytes.length && isDigit(bytes[this.at]!)) this.fail("invalid number");
    } else {
      while (this.at < bytes.length && isDigit(bytes[this.at]!)) this.at++;
    }
    let float = false;
    if (bytes[this.at] === 0x2e) {
      float = true;
      this.at++;
      if (!(this.at < bytes.length && isDigit(bytes[this.at]!))) this.fail("invalid number");
      while (this.at < bytes.length && isDigit(bytes[this.at]!)) this.at++;
    }
    if (bytes[this.at] === 0x65 || bytes[this.at] === 0x45) {
      float = true;
      this.at++;
      if (bytes[this.at] === 0x2b || bytes[this.at] === 0x2d) this.at++;
      if (!(this.at < bytes.length && isDigit(bytes[this.at]!))) this.fail("invalid number");
      while (this.at < bytes.length && isDigit(bytes[this.at]!)) this.at++;
    }
    let text = "";
    for (let i = start; i < this.at; i++) text += String.fromCharCode(bytes[i]!);
    if (!float) {
      const integer = BigInt(text);
      const negative = text.startsWith("-");
      // `-0` is serde_json's float -0.0, as is anything out of range.
      if (!negative && integer <= U64_MAX) return JsonNumber.int(text);
      if (negative && integer < 0n && integer >= I64_MIN) return JsonNumber.int(text);
    }
    const value = Number(text);
    if (!Number.isFinite(value)) this.fail("number out of range");
    return JsonNumber.float(value);
  }

  string(mayStayRaw = true): string {
    const bytes = this.bytes;
    this.at++; // the opening quote
    if (this.raw && mayStayRaw) {
      // A long run of plain ASCII up to the closing quote stays in place.
      const start = this.at;
      let end = start;
      for (;;) {
        const b = bytes[end];
        if (b === undefined || b === 0x22 || b === 0x5c || b < 0x20 || b >= 0x80) break;
        end++;
      }
      if (bytes[end] === 0x22 && end - start >= RAW_MIN) {
        this.at = end + 1;
        return new RawString(bytes, start, end) as unknown as string;
      }
    }
    let out = "";
    let runStart = this.at;
    let runAscii = true;
    const flush = (end: number) => {
      if (end === runStart) return;
      const run = bytes.subarray(runStart, end);
      if (runAscii) {
        // Chunked: String.fromCharCode's argument count is bounded.
        for (let i = 0; i < run.length; i += 8192) out += String.fromCharCode(...run.subarray(i, i + 8192));
      } else {
        try {
          out += utf8.decode(run);
        } catch {
          this.fail("invalid UTF-8");
        }
      }
    };
    for (;;) {
      const b = bytes[this.at];
      if (b === undefined) this.fail("EOF while parsing a string");
      if (b === 0x22) {
        flush(this.at);
        this.at++;
        return out;
      }
      if (b < 0x20) this.fail("control character in a string");
      if (b >= 0x80) {
        runAscii = false;
        this.at++;
        continue;
      }
      if (b !== 0x5c) {
        this.at++;
        continue;
      }
      flush(this.at);
      this.at++;
      const escape = bytes[this.at++];
      switch (escape) {
        case 0x22:
          out += '"';
          break;
        case 0x5c:
          out += "\\";
          break;
        case 0x2f:
          out += "/";
          break;
        case 0x62:
          out += "\b";
          break;
        case 0x66:
          out += "\f";
          break;
        case 0x6e:
          out += "\n";
          break;
        case 0x72:
          out += "\r";
          break;
        case 0x74:
          out += "\t";
          break;
        case 0x75: {
          const unit = this.hex4();
          if (unit >= 0xdc00 && unit <= 0xdfff) this.fail("lone trailing surrogate");
          if (unit >= 0xd800 && unit <= 0xdbff) {
            if (bytes[this.at] !== 0x5c || bytes[this.at + 1] !== 0x75) this.fail("lone leading surrogate");
            this.at += 2;
            const low = this.hex4();
            if (!(low >= 0xdc00 && low <= 0xdfff)) this.fail("lone leading surrogate");
            out += String.fromCharCode(unit, low);
          } else {
            out += String.fromCharCode(unit);
          }
          break;
        }
        default:
          this.fail("invalid escape");
      }
      runStart = this.at;
      runAscii = true;
    }
  }

  hex4(): number {
    let unit = 0;
    for (let i = 0; i < 4; i++) {
      const digit = hexValue(this.bytes[this.at++] ?? -1);
      if (digit < 0) this.fail("invalid \\u escape");
      unit = unit * 16 + digit;
    }
    return unit;
  }
}

/** The JSON in `bytes`, as serde_json's `from_slice` reads it. */
export const parseJson = (bytes: Uint8Array): Json => parse(bytes, false);

/** `parseJson`, with long plain strings left as `RawString`s: a value or key
 * typed as a string may be one. Turn a subtree into plain JSON with
 * `materialize` before anything else reads it. */
export const parsePackageJson = (bytes: Uint8Array): Json => parse(bytes, true);

const isRaw = (value: unknown): value is RawString => value instanceof RawString;
export { isRaw as isRawString };

/** `value` with every `RawString` made a string. */
export const materialize = (value: Json): Json => {
  if (isRaw(value)) return value.text();
  if (Array.isArray(value)) return value.map(materialize);
  if (value instanceof Map) {
    const out: JsonObject = new Map();
    for (const [key, item] of value) out.set(isRaw(key) ? (key as RawString).text() : key, materialize(item));
    return out;
  }
  return value;
};

const parse = (bytes: Uint8Array, raw: boolean): Json => {
  const parser = new Parser(bytes, raw);
  const value = parser.value();
  parser.skipWhitespace();
  if (parser.at !== bytes.length) parser.fail("trailing characters");
  return value;
};

export const parseJsonText = (text: string): Json => parseJson(encoder.encode(text));

/** Compare by code point, which is UTF-8's byte order (UTF-16's differs
 * above U+D7FF). */
export const compareUtf8 = (a: string, b: string): number => {
  const length = Math.min(a.length, b.length);
  for (let i = 0; i < length; i++) {
    const x = a.codePointAt(i)!;
    const y = b.codePointAt(i)!;
    if (x !== y) return x < y ? -1 : 1;
    if (x > 0xffff) i++;
  }
  return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
};

/**
 * A finite double as serde_json 1.0.151 prints it (the `zmij` crate's
 * `format_finite`): the shortest digits that read back as the value; fixed
 * notation when the first digit's decimal exponent is −5 to 15 (`100.0`,
 * `0.00001`, `1000000000000000.0`), otherwise `d.ddde±x` (`1e+16`, `1e-6`).
 */
export const formatFloat = (value: number): string => {
  if (value === 0) return Object.is(value, -0) ? "-0.0" : "0.0";
  const sign = value < 0 ? "-" : "";
  // ECMAScript's shortest round-trip digits, and the first digit's exponent.
  const [mantissa, exponent] = Math.abs(value).toExponential().split("e") as [string, string];
  const digits = mantissa.replace(".", "");
  const e = Number(exponent);
  if (e >= -5 && e <= 15) {
    if (digits.length - 1 <= e) return `${sign}${digits}${"0".repeat(e + 1 - digits.length)}.0`;
    if (e >= 0) return `${sign}${digits.slice(0, e + 1)}.${digits.slice(e + 1)}`;
    return `${sign}0.${"0".repeat(-e - 1)}${digits}`;
  }
  return `${sign}${digits[0]}${digits.length > 1 ? `.${digits.slice(1)}` : ""}e${e < 0 ? "-" : "+"}${Math.abs(e)}`;
};

const writeCanonical = (value: Json, out: string[]): void => {
  if (value === null) out.push("null");
  else if (value === true) out.push("true");
  else if (value === false) out.push("false");
  else if (typeof value === "string") out.push(JSON.stringify(value));
  else if (value instanceof JsonNumber) out.push(value.integer ?? formatFloat(value.value));
  else if (Array.isArray(value)) {
    out.push("[");
    value.forEach((item, i) => {
      if (i > 0) out.push(",");
      writeCanonical(item, out);
    });
    out.push("]");
  } else {
    out.push("{");
    [...value.keys()].sort(compareUtf8).forEach((key, i) => {
      if (i > 0) out.push(",");
      out.push(JSON.stringify(key), ":");
      writeCanonical(value.get(key)!, out);
    });
    out.push("}");
  }
};

/** signing.rs `canonical_json`, as text (encode it for the bytes). */
export const canonicalJson = (value: Json): string => {
  const out: string[] = [];
  writeCanonical(value, out);
  return out.join("");
};

/** A plain JS value for answers and storage: integers past 2^53 become
 * their decimal text rather than a rounded number. */
export const toPlain = (value: Json): unknown => {
  if (value instanceof JsonNumber) {
    if (value.integer === null) return value.value;
    return Number.isSafeInteger(value.value) ? value.value : value.integer;
  }
  if (Array.isArray(value)) return value.map(toPlain);
  if (value instanceof Map) {
    const out: Record<string, unknown> = {};
    for (const [key, item] of value) Object.defineProperty(out, key, { value: toPlain(item), enumerable: true, writable: true, configurable: true });
    return out;
  }
  return value;
};

/** `value` as a string, if it's one. */
export const str = (value: Json | undefined): string | undefined => (typeof value === "string" ? value : undefined);
