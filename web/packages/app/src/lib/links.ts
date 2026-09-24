/**
 * Link destination policy — the web port of the desktop's
 * `crates/ui/src/browser/model.rs` link validation
 * (`transcript_address`/`normalize_address`), the width-bounded label
 * truncation of `crates/ui/src/markdown/link_presentation.rs` (`truncate` +
 * `OffsetMap`), and the safe workspace-file resolution of
 * `crates/ui/src/workspace_links.rs` (`resolve_workspace_file_link`).
 *
 * Pure functions only: the renderer decides what a validated, internal or
 * rejected destination looks like, and the tests drive the accept/reject
 * tables straight from `markdown/links.rs` and `workspace_links.rs`.
 */

// ---------------------------------------------------------------------------
// Address validation (browser/model.rs)
// ---------------------------------------------------------------------------

/** Rust `char::is_control` — Unicode Cc (C0 + 0x7F..0x9F). */
function isControlChar(code: number): boolean {
  return code <= 0x1f || (code >= 0x7f && code <= 0x9f);
}

/** `loopback` (model.rs) — localhost names and loopback IPs. */
function isLoopback(url: URL): boolean {
  const host = url.hostname;
  if (host === "localhost" || host.endsWith(".localhost")) {
    return true;
  }
  if (/^127\.\d+\.\d+\.\d+$/.test(host) || host === "[::1]") {
    return true;
  }
  return false;
}

/**
 * `normalize_address` (model.rs:37-74): trim, reject controls, infer an
 * https:// scheme for bare hosts (never for a bare host:port that only looks
 * like a scheme), parse, then allow only http/https with a host and no
 * embedded credentials. Returns the normalized URL, or null when rejected.
 */
export function normalizeAddress(input: string): string | null {
  const text = input.trim();
  if (text.length === 0) {
    return null;
  }
  for (const c of text) {
    if (isControlChar(c.codePointAt(0)!)) {
      return null;
    }
  }
  // A bare host:port looks like a URI scheme to a URL parser. Only accept
  // that ambiguity when the suffix is an actual numeric port.
  const authority = text.split(/[/?#]/, 1)[0] ?? text;
  const colon = authority.lastIndexOf(":");
  const port = colon >= 0 ? authority.slice(colon + 1) : "";
  const hostPort = colon >= 0 && port.length > 0 && /^\d+$/.test(port);
  const explicit = text.includes("://") || (text.includes(":") && !hostPort && !text.startsWith("["));
  let parsed: URL;
  try {
    parsed = new URL(explicit ? text : `https://${text}`);
  } catch {
    return null;
  }
  if ((parsed.protocol !== "http:" && parsed.protocol !== "https:") || parsed.hostname.length === 0) {
    return null;
  }
  if (parsed.username.length > 0 || parsed.password.length > 0) {
    return null;
  }
  if (!explicit && isLoopback(parsed)) {
    parsed.protocol = "http:";
  }
  return parsed.href;
}

/**
 * `transcript_address` (model.rs:76-97) — the transcript's stricter gate:
 * chat links require an explicit web authority; never infer a scheme or
 * silently strip controls, credentials, malformed escapes or backslashes.
 * Returns the normalized destination, or null when the link stays inert.
 */
export function transcriptAddress(input: string): string | null {
  const lower = input.toLowerCase();
  const scheme = lower.startsWith("https://")
    ? "https://"
    : lower.startsWith("http://")
      ? "http://"
      : null;
  if (scheme === null) {
    return null;
  }
  const authority = lower.slice(scheme.length);
  if (
    [...input].some((c) => {
      const code = c.codePointAt(0)!;
      return isControlChar(code) || /\s/.test(c);
    }) ||
    input.includes("\\") ||
    authority.length === 0 ||
    authority.startsWith("/") ||
    authority.startsWith("?") ||
    authority.startsWith("#")
  ) {
    return null;
  }
  for (let i = 0; i < input.length; i++) {
    if (input.charCodeAt(i) === 0x25 && !isHexPair(input, i + 1)) {
      return null;
    }
  }
  return normalizeAddress(input);
}

function isHexPair(text: string, at: number): boolean {
  return isHexDigit(text.charCodeAt(at)) && isHexDigit(text.charCodeAt(at + 1));
}

function isHexDigit(code: number | undefined): boolean {
  return (
    code !== undefined &&
    ((code >= 0x30 && code <= 0x39) || (code >= 0x41 && code <= 0x46) || (code >= 0x61 && code <= 0x66))
  );
}

// ---------------------------------------------------------------------------
// Presentation truncation (link_presentation.rs)
// ---------------------------------------------------------------------------

/** An omission: the original range that was cut, and the `…` that replaced it. */
export interface LinkOmission {
  readonly originalStart: number;
  readonly originalEnd: number;
  readonly shownStart: number;
  readonly shownEnd: number;
}

/**
 * `OffsetMap` (link_presentation.rs:10-49) — maps offsets between the
 * displayed (truncated) text and the original, so selection and copy still
 * resolve against the untruncated source.
 */
export class OffsetMap {
  readonly omissions: readonly LinkOmission[];

  constructor(omissions: readonly LinkOmission[] = []) {
    this.omissions = omissions;
  }

  original(displayed: number): number {
    let shift = 0;
    for (const { originalStart, originalEnd, shownStart, shownEnd } of this.omissions) {
      if (displayed < shownStart) {
        break;
      }
      if (displayed < shownEnd) {
        return originalStart;
      }
      shift = originalEnd - shownEnd;
    }
    return displayed + shift;
  }

  displayed(original: number): number {
    let shift = 0;
    for (const { originalStart, originalEnd, shownStart, shownEnd } of this.omissions) {
      if (original < originalStart) {
        break;
      }
      if (original < originalEnd) {
        return shownStart;
      }
      shift = originalEnd - shownEnd;
    }
    return original - shift;
  }
}

/**
 * Grapheme boundaries as string indices (for the truncation binary search
 * and the destination card's wrap breaks). Falls back to code points on
 * runtimes without `Intl.Segmenter`.
 */
export function graphemeBoundaries(text: string): number[] {
  const ctor = (Intl as { Segmenter?: new (locale?: string, options?: { granularity: string }) => SegmenterLike }).Segmenter;
  if (ctor !== undefined) {
    const out: number[] = [];
    for (const { index } of new ctor(undefined, { granularity: "grapheme" }).segment(text)) {
      out.push(index);
    }
    return out;
  }
  const out: number[] = [];
  let at = 0;
  for (const c of text) {
    out.push(at);
    at += c.length;
  }
  return out;
}

interface SegmenterLike {
  segment(input: string): Iterable<{ index: number }>;
}

export interface LinkTruncationInput {
  /** The whole flattened text of the element. */
  readonly text: string;
  /** Link ranges within `text` (only these are truncation candidates). */
  readonly links: readonly { readonly start: number; readonly end: number; readonly url: string }[];
}

export interface LinkTruncationResult {
  readonly text: string;
  readonly offsets: OffsetMap;
}

/** `'…'.len_utf8()` — the ellipsis's byte length in the source's guard. */
const ELLIPSIS_UTF8_LEN = 3;

/**
 * `truncate` (link_presentation.rs:71-147) — width-bounded label truncation:
 * every link whose flattened label overflows `width` (measured through the
 * caller's `measure`) is cut to a grapheme boundary + `…`, binary-searched on
 * shaped width, and never lets a short label get LONGER just to add the
 * ellipsis. Links that fail destination validation are left alone.
 */
export function linkPresentationTruncate(
  input: LinkTruncationInput,
  width: number,
  measure: (label: string) => number,
): LinkTruncationResult {
  const omissions: LinkOmission[] = [];
  for (const range of input.links) {
    if (transcriptAddress(range.url) === null) {
      continue;
    }
    const label = input.text.slice(range.start, range.end);
    if (measure(label) <= width) {
      continue;
    }
    const boundaries = graphemeBoundaries(label);
    let low = 0;
    let high = boundaries.length;
    while (low < high) {
      const middle = Math.floor((low + high) / 2);
      const candidate = `${label.slice(0, boundaries[middle])}…`;
      if (measure(candidate) <= width) {
        low = middle + 1;
      } else {
        high = middle;
      }
    }
    const prefix = boundaries[Math.max(0, low - 1)] ?? 0;
    // Never make a short label longer just to show an ellipsis: only cut
    // when the removed text outlives the ellipsis that replaces it (the
    // source compares against `'…'.len_utf8()` = 3, in the same units).
    const removed = input.text.slice(range.start + prefix, range.end);
    if ([...removed].length > ELLIPSIS_UTF8_LEN) {
      omissions.push({ originalStart: range.start + prefix, originalEnd: range.end, shownStart: 0, shownEnd: 0 });
    }
  }

  let text = "";
  let at = 0;
  const recorded: LinkOmission[] = [];
  for (const omission of omissions) {
    text += input.text.slice(at, omission.originalStart);
    const shownStart = text.length;
    text += "…";
    recorded.push({
      originalStart: omission.originalStart,
      originalEnd: omission.originalEnd,
      shownStart,
      shownEnd: shownStart + 1,
    });
    at = omission.originalEnd;
  }
  text += input.text.slice(at);
  return { text, offsets: new OffsetMap(recorded) };
}

/**
 * Zero-width-space grapheme breaks (link_destination.rs:22-25): the hover
 * card wraps even a single long path segment by breaking at graphemes.
 */
export function graphemeBreaks(text: string): string {
  let out = "";
  const boundaries = graphemeBoundaries(text);
  for (let i = 0; i < boundaries.length; i++) {
    const start = boundaries[i]!;
    const end = i + 1 < boundaries.length ? boundaries[i + 1]! : text.length;
    out += text.slice(start, end);
    out += "​";
  }
  return out;
}

// ---------------------------------------------------------------------------
// Workspace file links (workspace_links.rs)
// ---------------------------------------------------------------------------

export interface WorkspaceFileLink {
  readonly path: string;
  readonly line: number | null;
  readonly column: number | null;
}

const FILE_MENTION_SCHEME = "zeron-file:";

/**
 * `resolve_workspace_file_link` (workspace_links.rs:14-104) — resolve an
 * agent-authored link or bare token into a safe workspace-relative path with
 * an optional line/column. Rejects empty/`?`/NUL targets, `file://` absolute
 * paths outside the root, foreign schemes, `..`/`.`/empty segments,
 * backslashes, drive letters and non-round-tripping percent escapes.
 */
export function resolveWorkspaceFileLink(target: string, workspaceRoot: string): WorkspaceFileLink | null {
  let candidate = target.trim();
  if (candidate.length === 0 || candidate.includes("?") || candidate.includes("\0")) {
    return null;
  }

  const isFileMention = candidate.startsWith(FILE_MENTION_SCHEME);
  if (isFileMention) {
    const encoded = candidate.slice(FILE_MENTION_SCHEME.length);
    const decoded = percentDecodePath(encoded);
    if (decoded === null || percentEncodePath(decoded) !== encoded || decoded.endsWith("/")) {
      return null;
    }
    candidate = decoded;
  } else if (candidate.startsWith("file://")) {
    candidate = candidate.slice("file://".length);
  } else if (candidate.includes("://") || candidate.startsWith("mailto:")) {
    return null;
  }

  let fragmentLine: number | null = null;
  if (!isFileMention) {
    const split = splitLineFragment(candidate);
    candidate = split.path;
    fragmentLine = split.line;
  }
  let suffixLine: number | null = null;
  let column: number | null = null;
  if (!isFileMention) {
    const split = splitLineSuffix(candidate);
    candidate = split.path;
    suffixLine = split.line;
    column = split.column;
  }
  if (
    candidate.includes("\\") ||
    candidate.includes("\n") ||
    candidate.includes("\r") ||
    candidate
      .split("/")
      .some((part, index) => (part.length === 0 && index !== 0) || part === "." || part === "..")
  ) {
    return null;
  }
  if (candidate.includes(":")) {
    return null;
  }
  const line = fragmentLine ?? suffixLine;

  // A remote engine may supply POSIX paths to a Windows viewport: classify
  // by whether the target has a root, not by the viewer's absolute-path rules.
  const relative = hasRoot(candidate) ? stripRootPrefix(candidate, workspaceRoot) : candidate;
  if (relative === null) {
    return null;
  }
  const path = safeRelativePath(relative);
  if (path === null) {
    return null;
  }
  return { path, line, column };
}

/** `Path::has_root` for our purposes — a leading separator or drive prefix. */
function hasRoot(path: string): boolean {
  return path.startsWith("/") || path.startsWith("\\") || /^[A-Za-z]:/.test(path);
}

/** `Path::strip_prefix` component-wise (both sides may use `/` or `\`). */
function stripRootPrefix(target: string, root: string): string | null {
  const targetParts = splitComponents(target);
  const rootParts = splitComponents(root);
  if (rootParts.length === 0 || targetParts.length < rootParts.length) {
    return null;
  }
  for (let i = 0; i < rootParts.length; i++) {
    if (targetParts[i] !== rootParts[i]) {
      return null;
    }
  }
  return targetParts.slice(rootParts.length).join("/");
}

function splitComponents(path: string): string[] {
  return path.split(/[\\/]/);
}

/** `safe_relative_path` — every component must be a normal, non-empty name. */
function safeRelativePath(path: string): string | null {
  if (path.length === 0) {
    return null;
  }
  const parts: string[] = [];
  for (const component of path.split("/")) {
    if (component.length === 0 || component === "." || component === "..") {
      return null;
    }
    parts.push(component);
  }
  return parts.length > 0 ? parts.join("/") : null;
}

/** `split_line_fragment` — a `#L123` fragment (1-based, positive). */
function splitLineFragment(target: string): { path: string; line: number | null } {
  const hash = target.lastIndexOf("#");
  if (hash < 0) {
    return { path: target, line: null };
  }
  const fragment = target.slice(hash + 1);
  const match = /^L(\d+)$/.exec(fragment);
  const line = match === null ? 0 : Number.parseInt(match[1]!, 10);
  if (match !== null && line > 0) {
    return { path: target.slice(0, hash), line };
  }
  return { path: target, line: null };
}

/** `split_line_suffix` — trailing `:123` or `:123:45` (1-based, positive). */
function splitLineSuffix(target: string): { path: string; line: number | null; column: number | null } {
  // `rsplitn(3, ':')`: the two rightmost pieces, then the head.
  const last = target.lastIndexOf(":");
  if (last < 0) {
    return { path: target, line: null, column: null };
  }
  const lastPiece = target.slice(last + 1);
  const lastNumber = positiveNumber(lastPiece);
  if (lastNumber === null) {
    return { path: target, line: null, column: null };
  }
  const before = target.slice(0, last);
  const beforeColon = before.lastIndexOf(":");
  if (beforeColon >= 0) {
    const beforePiece = before.slice(beforeColon + 1);
    const line = positiveNumber(beforePiece);
    if (line !== null) {
      return { path: before.slice(0, beforeColon), line, column: lastNumber };
    }
  }
  return { path: before, line: lastNumber, column: null };
}

function positiveNumber(value: string): number | null {
  if (!/^\d+$/.test(value)) {
    return null;
  }
  const number = Number.parseInt(value, 10);
  return number > 0 ? number : null;
}

function percentDecodePath(encoded: string): string | null {
  const bytes: number[] = [];
  for (let i = 0; i < encoded.length; ) {
    if (encoded[i] === "%") {
      if (!isHexPair(encoded, i + 1)) {
        return null;
      }
      bytes.push(Number.parseInt(encoded.slice(i + 1, i + 3), 16));
      i += 3;
    } else {
      // Non-ASCII in a file-mention target cannot round-trip the byte-wise
      // re-encode below, so reject instead of lossy-decoding.
      const code = encoded.codePointAt(i)!;
      if (code > 0x7f) {
        return null;
      }
      bytes.push(code);
      i += code >= 0x10000 ? 2 : 1;
    }
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(bytes));
  } catch {
    return null;
  }
}

function percentEncodePath(path: string): string {
  let out = "";
  for (const byte of new TextEncoder().encode(path)) {
    if (
      (byte >= 0x30 && byte <= 0x39) ||
      (byte >= 0x41 && byte <= 0x5a) ||
      (byte >= 0x61 && byte <= 0x7a) ||
      byte === 0x2d ||
      byte === 0x2e ||
      byte === 0x5f ||
      byte === 0x7e ||
      byte === 0x2f
    ) {
      out += String.fromCharCode(byte);
    } else {
      out += `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    }
  }
  return out;
}
