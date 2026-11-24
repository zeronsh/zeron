/**
 * Versions for the store: strict SemVer 2.0.0 as the engine's `semver`
 * crate (1.0.28) parses it, with the store's limits on top (contract
 * amendment 3): each numeric part at most 2^53−1, at most 128 characters.
 * Build metadata is kept but ignored for precedence, so two versions that
 * differ only in it are the same version.
 *
 * `engines.zeron` ranges (amendment 2): an npm-style subset. `||` between
 * sets, whitespace between comparators, `>= > <= < = ^ ~` before a full
 * `x.y.z` version (pre-release allowed), a bare version meaning exactly that
 * one, and `*`. Versions compare by precedence; `^` and `~` expand as npm's
 * do, 0.x included.
 */
import { LIMITS } from "./data";

export const MAX_VERSION_LENGTH = LIMITS.max_version_len;

export interface Version {
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
  /** Dot-separated identifiers; empty for a release. */
  readonly pre: readonly string[];
  readonly build: readonly string[];
  /** As written. */
  readonly text: string;
}

const U64_MAX = 18446744073709551615n;

/** One run of digits (semver's `numeric_identifier`): no leading zero, at
 * most u64; the store's limit is applied after. */
const numeric = (text: string, at: number): { value: bigint; end: number } | undefined => {
  let end = at;
  while (end < text.length && text.charCodeAt(end) >= 0x30 && text.charCodeAt(end) <= 0x39) end++;
  if (end === at) return undefined;
  if (end - at > 1 && text[at] === "0") return undefined;
  const value = BigInt(text.slice(at, end));
  return value > U64_MAX ? undefined : { value, end };
};

/** semver's `identifier`: dot-separated non-empty segments of
 * `[0-9A-Za-z-]`; in a pre-release, an all-digit segment has no leading
 * zero. Answers the identifiers and where they end, or undefined. */
const identifiers = (text: string, at: number, pre: boolean): { parts: string[]; end: number } | undefined => {
  const parts: string[] = [];
  let start = at;
  let end = at;
  for (;;) {
    const c = text.charCodeAt(end);
    const ok = (c >= 0x30 && c <= 0x39) || (c >= 0x41 && c <= 0x5a) || (c >= 0x61 && c <= 0x7a) || c === 0x2d;
    if (ok) {
      end++;
      continue;
    }
    const part = text.slice(start, end);
    if (part.length === 0) return undefined;
    if (pre && part.length > 1 && /^[0-9]+$/.test(part) && part[0] === "0") return undefined;
    parts.push(part);
    if (text[end] !== ".") return { parts, end };
    end++;
    start = end;
  }
};

/** `text` as the `semver` crate parses a Version (u64 parts), or undefined. */
export const parseSemver = (
  text: string
): { major: bigint; minor: bigint; patch: bigint; pre: string[]; build: string[] } | undefined => {
  const major = numeric(text, 0);
  if (!major || text[major.end] !== ".") return undefined;
  const minor = numeric(text, major.end + 1);
  if (!minor || text[minor.end] !== ".") return undefined;
  const patch = numeric(text, minor.end + 1);
  if (!patch) return undefined;
  let at = patch.end;
  let pre: string[] = [];
  let build: string[] = [];
  if (text[at] === "-") {
    const found = identifiers(text, at + 1, true);
    if (!found) return undefined;
    pre = found.parts;
    at = found.end;
  }
  if (text[at] === "+") {
    const found = identifiers(text, at + 1, false);
    if (!found) return undefined;
    build = found.parts;
    at = found.end;
  }
  if (at !== text.length) return undefined;
  return { major: major.value, minor: minor.value, patch: patch.value, pre, build };
};

/** A version the store takes (semver's rules and the store's limits), or
 * undefined. */
export const parseVersion = (text: string): Version | undefined => {
  if (text.length === 0 || text.length > MAX_VERSION_LENGTH) return undefined;
  const parsed = parseSemver(text);
  if (!parsed) return undefined;
  const limit = BigInt(Number.MAX_SAFE_INTEGER);
  if (parsed.major > limit || parsed.minor > limit || parsed.patch > limit) return undefined;
  // (A numeric pre-release identifier too, as the engine's version.rs.)
  if (parsed.pre.some((p) => /^[0-9]+$/.test(p) && (p.length > 16 || BigInt(p) > limit))) return undefined;
  return {
    major: Number(parsed.major),
    minor: Number(parsed.minor),
    patch: Number(parsed.patch),
    pre: parsed.pre,
    build: parsed.build,
    text
  };
};

const isNumeric = (identifier: string): boolean => /^[0-9]+$/.test(identifier);

/** SemVer pre-release precedence (empty = a release, above any pre-release). */
export const comparePre = (a: readonly string[], b: readonly string[]): number => {
  if (a.length === 0 || b.length === 0) return a.length === b.length ? 0 : a.length === 0 ? 1 : -1;
  for (let i = 0; i < a.length; i++) {
    const x = a[i]!;
    const y = b[i];
    if (y === undefined) return 1;
    const xn = isNumeric(x);
    const yn = isNumeric(y);
    let order: number;
    if (xn && yn) order = x.length - y.length || (x < y ? -1 : x > y ? 1 : 0);
    else if (xn) return -1;
    else if (yn) return 1;
    else order = x < y ? -1 : x > y ? 1 : 0;
    if (order !== 0) return Math.sign(order);
  }
  return a.length === b.length ? 0 : -1;
};

/** SemVer precedence (build metadata ignored): negative, zero or positive. */
export const comparePrecedence = (a: Version, b: Version): number =>
  Math.sign(a.major - b.major) || Math.sign(a.minor - b.minor) || Math.sign(a.patch - b.patch) || comparePre(a.pre, b.pre);

/** The version without build metadata: what identifies it in the store. */
export const precedenceText = (v: Version): string =>
  `${v.major}.${v.minor}.${v.patch}${v.pre.length ? `-${v.pre.join(".")}` : ""}`;

/**
 * A pre-release as text whose binary order is SemVer's: each identifier is
 * `0` + its length in three digits + its digits when numeric (so 9 < 10),
 * or `1` + itself (numeric before alphanumeric; ASCII order), joined by a
 * space, which sorts before every character an identifier may hold (so a
 * shorter list of equal identifiers comes first). Empty for a release.
 */
export const preKey = (pre: readonly string[]): string =>
  pre.map((p) => (isNumeric(p) ? `0${String(p.length).padStart(3, "0")}${p}` : `1${p}`)).join(" ");

export const isPrerelease = (v: Version): boolean => v.pre.length > 0;

// ── ranges ──────────────────────────────────────────────────────────────────

type Op = ">" | ">=" | "<" | "<=" | "=";
interface Comparator {
  readonly op: Op;
  readonly version: Version;
}
/** Sets joined by `||`; each set's comparators all hold. An empty set is `*`. */
export type Range = readonly (readonly Comparator[])[];

export const MAX_RANGE_LENGTH = LIMITS.max_range_len;

/** The lowest version of `major.minor.patch`: `x.y.z-0`. */
const floor = (major: number, minor: number, patch: number): Version => ({
  major,
  minor,
  patch,
  pre: ["0"],
  build: [],
  text: `${major}.${minor}.${patch}-0`
});

const expand = (op: string, v: Version): Comparator[] => {
  switch (op) {
    case "^":
      return [
        { op: ">=", version: v },
        {
          op: "<",
          version: v.major > 0 ? floor(v.major + 1, 0, 0) : v.minor > 0 ? floor(0, v.minor + 1, 0) : floor(0, 0, v.patch + 1)
        }
      ];
    case "~":
      return [
        { op: ">=", version: v },
        { op: "<", version: floor(v.major, v.minor + 1, 0) }
      ];
    case "":
    case "=":
      return [{ op: "=", version: v }];
    default:
      return [{ op: op as Op, version: v }];
  }
};

const OPS = [">=", "<=", ">", "<", "=", "^", "~"] as const;
/** `token`'s operator, if it starts with one, and the rest. */
const splitOp = (token: string): [string | undefined, string] => {
  for (const op of OPS) if (token.startsWith(op)) return [op, token.slice(op.length)];
  return [undefined, token];
};

/** Unicode White_Space other than the space (Rust's `char::is_whitespace`). */
const OTHER_WHITESPACE = /[\t\n\u000b\f\r\u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]/;

/**
 * An `engines.zeron` range, or undefined if it isn't one: the engine's
 * version.rs `Range::parse`. Sets split on `||`, comparators on spaces (and
 * nothing else); an operator alone takes the next token as its version
 * (`>= 1.0.0`); `*` stands alone as a whole set.
 */
export const parseRange = (text: string): Range | undefined => {
  if (text.trim() === "" || new TextEncoder().encode(text).length > MAX_RANGE_LENGTH || OTHER_WHITESPACE.test(text)) return undefined;
  const sets: Comparator[][] = [];
  for (const rawSet of text.split("||")) {
    const tokens = rawSet.split(" ").filter((t) => t.length > 0);
    if (tokens.length === 0) return undefined;
    if (tokens.length === 1 && tokens[0] === "*") {
      sets.push([]);
      continue;
    }
    const set: Comparator[] = [];
    for (let i = 0; i < tokens.length; i++) {
      const [op, rest] = splitOp(tokens[i]!);
      let versionText = rest;
      if (op !== undefined && rest === "") {
        const next = tokens[++i];
        if (next === undefined || splitOp(next)[0] !== undefined) return undefined;
        versionText = next;
      }
      if (versionText === "*") return undefined;
      const version = parseVersion(versionText);
      if (!version) return undefined;
      set.push(...expand(op ?? "", version));
    }
    sets.push(set);
  }
  return sets;
};

const holds = (c: Comparator, v: Version): boolean => {
  const order = comparePrecedence(v, c.version);
  switch (c.op) {
    case ">":
      return order > 0;
    case ">=":
      return order >= 0;
    case "<":
      return order < 0;
    case "<=":
      return order <= 0;
    case "=":
      return order === 0;
  }
};

/** Whether `version` satisfies `range`. */
export const satisfies = (version: Version, range: Range): boolean => range.some((set) => set.every((c) => holds(c, version)));
