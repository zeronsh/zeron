import type { RpcErrorKind } from "@zeron/engine-client";

/**
 * The `@` file-mention library — a line-for-line port of the desktop's
 * composer.rs mention core (constants at `:864-870`, the Markdown
 * transport at `:929-1051`, the tooltip state machine at `:1089-1149`,
 * `TextProjection` at `:1158-1262`, `mention_display_labels` at `:1267`,
 * `sent_mention_display` at `:1316`, `mention_token` at `:3866`, and
 * `mention_error_message` at `:3969`).
 *
 * Offsets are UTF-16 code units (the browser's string indices), the direct
 * equivalent of the desktop's byte indices: every caret value the web feeds
 * these functions comes from `selectionStart`/`selectionEnd`, which are
 * always code-unit indices on char boundaries.
 */

/** The literal `@` a chip displays before its file name (composer.rs:864). */
export const MENTION_PREFIX = "@";
/** Non-breaking side bearings around the chip label (composer.rs:867). */
export const MENTION_SIDE_PAD = "\u00a0";
/**
 * A private URI scheme keeps file mentions distinguishable from ordinary
 * Markdown links pasted into the composer (composer.rs:870).
 */
export const FILE_MENTION_SCHEME = "zeron-file:";
/** Hover dwell before a chip's path tooltip appears (composer.rs:865). */
export const MENTION_TOOLTIP_DELAY_MS = 420;
/** The path tooltip's fixed height (composer.rs:866). */
export const MENTION_TOOLTIP_HEIGHT = 24;
/** The chip wash's corner radius (composer.rs:3500-3506). */
export const MENTION_CHIP_RADIUS = 5;
/** The chip wash's top inset inside its text row (composer.rs:3494-3498). */
export const MENTION_CHIP_TOP_INSET = 2;
/** The chip wash's height reduction inside its text row (composer.rs:3498). */
export const MENTION_CHIP_HEIGHT_CUT = 4;

/** A detected completion token: the range it spans plus the typed query. */
export interface CompletionToken {
  readonly start: number;
  readonly end: number;
  readonly query: string;
}

/** Whether `index` falls between UTF-16 code units (never splits a pair). */
export function isCharBoundary(text: string, index: number): boolean {
  if (index < 0 || index > text.length) {
    return false;
  }
  if (index === text.length) {
    return true;
  }
  const code = text.charCodeAt(index);
  return code < 0xdc00 || code > 0xdfff;
}

function isWhitespaceCode(code: number): boolean {
  return code === 0x20 || (code >= 0x09 && code <= 0x0d) || code === 0xa0 || code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) || code === 0x2028 || code === 0x2029 || code === 0x202f ||
    code === 0x205f || code === 0x3000 || code === 0xfeff;
}

/** The `@` must begin a token (composer.rs:3866-3895): excludes
 * `name@example.com` and mid-word `@`, allows `(@src`, `[@src`, `{@src`. */
export function mentionToken(text: string, cursor: number): CompletionToken | null {
  if (cursor > text.length || !isCharBoundary(text, cursor)) {
    return null;
  }
  let tokenStart = 0;
  for (let at = cursor - 1; at >= 0; at -= 1) {
    if (isWhitespaceCode(text.charCodeAt(at))) {
      tokenStart = at + 1;
      break;
    }
  }
  const at = text.lastIndexOf("@", cursor - 1);
  if (at < tokenStart) {
    return null;
  }
  const validBoundary =
    at === 0 ||
    (() => {
      const previous = text[at - 1];
      return (
        previous !== undefined &&
        (isWhitespaceCode(previous.charCodeAt(0)) || previous === "(" || previous === "[" || previous === "{")
      );
    })();
  if (!validBoundary || text.slice(at + 1, cursor).includes("@")) {
    return null;
  }
  let end = text.length;
  for (let ix = cursor; ix < text.length; ix += 1) {
    if (isWhitespaceCode(text.charCodeAt(ix))) {
      end = ix;
      break;
    }
  }
  return { start: at, end, query: text.slice(at + 1, cursor) };
}

// ---------------------------------------------------------------------------
// Strict local Markdown transport (composer.rs:929-1051)
// ---------------------------------------------------------------------------

/** `percent_encode_path`: keeps `[A-Za-z0-9-._~/]`, everything else `%XX`
 * uppercase hex, over the UTF-8 bytes (composer.rs:892). */
export function percentEncodePath(path: string): string {
  let out = "";
  for (const byte of new TextEncoder().encode(path)) {
    const ch = String.fromCharCode(byte);
    if (/[A-Za-z0-9\-._~/]/.test(ch)) {
      out += ch;
    } else {
      out += `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
    }
  }
  return out;
}

function percentDecodeBytes(encoded: string): number[] {
  const bytes: number[] = [];
  let at = 0;
  while (at < encoded.length) {
    if (encoded[at] === "%") {
      const hex = encoded.slice(at + 1, at + 3);
      const value = Number.parseInt(hex, 16);
      if (hex.length !== 2 || Number.isNaN(value)) {
        throw new Error("bad escape");
      }
      bytes.push(value);
      at += 3;
    } else {
      bytes.push(encoded.charCodeAt(at));
      at += 1;
    }
  }
  return bytes;
}

function percentDecodePath(encoded: string): string | null {
  try {
    return new TextDecoder().decode(new Uint8Array(percentDecodeBytes(encoded)));
  } catch {
    return null;
  }
}

/** `escape_mention_label` (composer.rs:922): `\` → `\\`, `[`/`]` escaped. */
export function escapeMentionLabel(label: string): string {
  return label.replaceAll("\\", "\\\\").replaceAll("[", "\\[").replaceAll("]", "\\]");
}

/** `local_file_link` (composer.rs:929): the strict local Markdown form
 * `[{escaped basename}](zeron-file:{percent-encoded path}{"/" if dir})`. */
export function localFileLink(path: string, isDir: boolean): string {
  const trimmed = path.replace(/\/+$/, "");
  const parts = trimmed.split("/");
  let basename = parts[parts.length - 1] ?? "";
  if (basename === "") {
    basename = trimmed;
  }
  return `[${escapeMentionLabel(basename)}](${FILE_MENTION_SCHEME}${percentEncodePath(
    `${trimmed}${isDir ? "/" : ""}`,
  )})`;
}

/** A strict, workspace-relative, no-traversal path (composer.rs:984). */
export function localPathIsSafe(path: string): boolean {
  return (
    path.length > 0 &&
    !path.startsWith("/") &&
    !path.includes("\\") &&
    ![...path].some((ch) => ch.charCodeAt(0) < 32) &&
    !path.split("/").some((part) => part.length === 0 || part === "." || part === "..")
  );
}

/**
 * `dropped_file_mention` (composer.rs:947): the insertion a workspace-path
 * drop produces at an arbitrary selection — its own leading separator when
 * the drop point abuts text, a trailing space unless a non-newline
 * whitespace already follows. Returns the inserted string plus how far the
 * cursor advances past the insertion point.
 */
export function droppedFileMention(
  content: string,
  range: { start: number; end: number },
  path: string,
  isDir: boolean,
): { inserted: string; cursorAdvance: number } | null {
  if (
    range.start > range.end ||
    !localPathIsSafe(path) ||
    !isCharBoundary(content, range.start) ||
    !isCharBoundary(content, range.end)
  ) {
    return null;
  }
  const suffix = content.slice(range.end);
  const prefix =
    range.start > 0 && !isWhitespaceCode(content.charCodeAt(range.start - 1)) ? " " : "";
  const existingSeparator =
    suffix.length > 0 &&
    isWhitespaceCode(suffix.charCodeAt(0)) &&
    suffix[0] !== "\n" &&
    suffix[0] !== "\r"
      ? suffix[0]
      : null;
  const existing = existingSeparator ?? null;
  const trailing = existing !== null ? "" : " ";
  const inserted = `${prefix}${localFileLink(path, isDir)}${trailing}`;
  const cursorAdvance = inserted.length + (existing !== null ? existing.length : 0);
  return { inserted, cursorAdvance };
}

/** The `](` that closes a mention label (composer.rs:994). */
function labelClose(text: string, start: number): number | null {
  let escaped = false;
  for (let at = start; at < text.length; at += 1) {
    const ch = text[at]!;
    if (escaped) {
      escaped = false;
    } else if (ch === "\\") {
      escaped = true;
    } else if (ch === "]" && text.slice(at + 1).startsWith("(")) {
      return at;
    }
  }
  return null;
}

/** One canonical `[label](zeron-file:target)` link in the raw text. */
export interface FileMentionLink {
  readonly start: number;
  readonly end: number;
  readonly basename: string;
  readonly path: string;
  readonly isDir: boolean;
}

/** Scan the raw text for strict, canonical links (composer.rs:1008): the
 * target must decode, round-trip, be a safe local path, and its basename
 * must equal the (escaped) label. Anything else is skipped. */
export function fileMentionLinks(text: string): FileMentionLink[] {
  const links: FileMentionLink[] = [];
  let search = 0;
  for (;;) {
    const start = text.indexOf("[", search);
    if (start < 0) {
      return links;
    }
    const labelEnd = labelClose(text, start + 1);
    if (labelEnd === null) {
      search = start + 1;
      continue;
    }
    const targetStart = labelEnd + 2;
    const close = text.indexOf(")", targetStart);
    if (close < 0) {
      search = start + 1;
      continue;
    }
    const end = close + 1;
    const label = text.slice(start + 1, labelEnd);
    const encoded = text.slice(targetStart, end - 1);
    if (!encoded.startsWith(FILE_MENTION_SCHEME)) {
      search = end;
      continue;
    }
    const payload = encoded.slice(FILE_MENTION_SCHEME.length);
    const decoded = percentDecodePath(payload);
    let parsed: { path: string; isDir: boolean } | null = null;
    if (decoded !== null) {
      const isDir = decoded.endsWith("/");
      const path = isDir ? decoded.slice(0, -1) : decoded;
      const basename = path.split("/").pop() ?? "";
      if (
        localPathIsSafe(path) &&
        percentEncodePath(decoded) === payload &&
        escapeMentionLabel(basename) === label
      ) {
        parsed = { path, isDir };
      }
    }
    if (parsed !== null) {
      links.push({
        start,
        end,
        basename: parsed.path.split("/").pop() ?? "",
        path: parsed.path,
        isDir: parsed.isDir,
      });
    }
    search = end;
  }
}

// ---------------------------------------------------------------------------
// Display labels + the chip projection (composer.rs:1267, 1158-1262)
// ---------------------------------------------------------------------------

/**
 * `mention_display_labels` (composer.rs:1267): the basename when unique;
 * otherwise the shortest unique path-component suffix (per-component
 * comparison, never substring); the full path as the last resort.
 */
export function mentionDisplayLabels(links: readonly FileMentionLink[]): string[] {
  return links.map((link, ix) => {
    if (links.filter((other) => other.basename === link.basename).length === 1) {
      return link.basename;
    }
    const parts = link.path.split("/");
    for (let count = 1; count <= parts.length; count += 1) {
      const suffix = parts.slice(parts.length - count).join("/");
      const suffixParts = suffix.split("/");
      const unique = links.every(
        (other, otherIx) =>
          otherIx === ix || other.path.split("/").slice(-suffixParts.length).join("/") !== suffix,
      );
      if (unique) {
        return suffix;
      }
    }
    return link.path;
  });
}

/** One chip in the projected display string (a link plus its display range). */
export interface ProjectedMention {
  readonly link: FileMentionLink;
  /** The chip's range over the display string: `NBSP @label NBSP`. */
  readonly start: number;
  readonly end: number;
}

/**
 * `TextProjection` (composer.rs:1153-1262): the display string with links
 * collapsed to `NBSP @label NBSP`, plus the mapping functions that make the
 * chip atomic — a collapsed caret inside a link snaps to the nearer end, a
 * selection overlapping a link swallows it whole, and the boundary helpers
 * let Left/Right/Backspace step over a chip in one press.
 */
export class TextProjection {
  readonly display: string;
  readonly mentions: readonly ProjectedMention[];

  constructor(raw: string) {
    const links = fileMentionLinks(raw);
    const labels = mentionDisplayLabels(links);
    let display = "";
    const mentions: ProjectedMention[] = [];
    let rawAt = 0;
    links.forEach((link, ix) => {
      display += raw.slice(rawAt, link.start);
      const start = display.length;
      display += MENTION_SIDE_PAD;
      display += MENTION_PREFIX;
      display += labels[ix]!.replaceAll(" ", MENTION_SIDE_PAD);
      display += MENTION_SIDE_PAD;
      mentions.push({ link, start, end: display.length });
      rawAt = link.end;
    });
    display += raw.slice(rawAt);
    this.display = display;
    this.mentions = mentions;
  }

  /** `raw_to_display` (composer.rs:1190): a raw offset into the display
   * string; inside a link maps to the chip's display start. */
  rawToDisplay(raw: number): number {
    let rawAt = 0;
    let displayAt = 0;
    for (const { link, start, end } of this.mentions) {
      if (raw <= link.start) {
        return displayAt + Math.max(0, raw - rawAt);
      }
      if (raw < link.end) {
        return start;
      }
      rawAt = link.end;
      displayAt = end;
    }
    return displayAt + Math.max(0, raw - rawAt);
  }

  /** `display_to_raw` (composer.rs:1206): a display offset back to raw;
   * inside a chip snaps to the link start (first half) or end. */
  displayToRaw(displayOffset: number): number {
    let rawAt = 0;
    let displayAt = 0;
    for (const { link, start, end } of this.mentions) {
      if (displayOffset <= start) {
        return rawAt + Math.max(0, displayOffset - displayAt);
      }
      if (displayOffset < end) {
        return displayOffset - start < (end - start) / 2 ? link.start : link.end;
      }
      rawAt = link.end;
      displayAt = end;
    }
    return rawAt + Math.max(0, displayOffset - displayAt);
  }

  /** `normalize_range` (composer.rs:1226): collapsed carets inside a link
   * snap to the nearer end; selections expand to swallow every overlapping
   * link whole. */
  normalizeRange(start: number, end: number): { start: number; end: number } {
    if (start === end) {
      for (const { link } of this.mentions) {
        if (link.start < start && start < link.end) {
          const midpoint = link.start + (link.end - link.start) / 2;
          const at = start < midpoint ? link.start : link.end;
          return { start: at, end: at };
        }
      }
      return { start, end };
    }
    let normalizedStart = start;
    let normalizedEnd = end;
    for (const { link } of this.mentions) {
      if (normalizedStart < link.end && normalizedEnd > link.start) {
        normalizedStart = Math.min(normalizedStart, link.start);
        normalizedEnd = Math.max(normalizedEnd, link.end);
      }
    }
    return { start: normalizedStart, end: normalizedEnd };
  }

  /** `previous_boundary` (composer.rs:1251): the link start when the caret
   * sits at a link's end, else null. */
  previousBoundary(raw: number): number | null {
    for (const { link } of this.mentions) {
      if (raw === link.end) {
        return link.start;
      }
    }
    return null;
  }

  /** `next_boundary` (composer.rs:1257): the link end when the caret sits
   * at a link's start, else null. */
  nextBoundary(raw: number): number | null {
    for (const { link } of this.mentions) {
      if (raw === link.start) {
        return link.end;
      }
    }
    return null;
  }
}

/**
 * `display_row_segments` (composer.rs:1131): split a display range at every
 * soft-wrap boundary — a range crossing a wrap gets a fresh segment starting
 * at x = 0 on the new row (the wash never bleeds across rows). Only the
 * tooltip anchor and the unit test need this on the web; CSS wrapping
 * handles the visual split.
 */
export function displayRowSegments(
  range: { start: number; end: number },
  rowEnds: readonly number[],
): Array<{ row: number; rowStart: number; start: number; end: number }> {
  const segments: Array<{ row: number; rowStart: number; start: number; end: number }> = [];
  let rowStart = 0;
  for (let rowIx = 0; rowIx < rowEnds.length; rowIx += 1) {
    const rowEnd = rowEnds[rowIx]!;
    const start = Math.max(range.start, rowStart);
    const end = Math.min(range.end, rowEnd);
    if (start < end) {
      segments.push({ row: rowIx, rowStart, start, end });
    }
    rowStart = rowEnd;
    if (rowStart >= range.end) {
      break;
    }
  }
  return segments;
}

// ---------------------------------------------------------------------------
// Sent-message projection (composer.rs:1303-1338)
// ---------------------------------------------------------------------------

/** One chip in a *sent* message: its display range, plus the full path. */
export interface SentMentionSpan {
  readonly start: number;
  readonly end: number;
  /** Full workspace-relative path (labels can be shortened to suffixes). */
  readonly path: string;
  readonly isDir: boolean;
}

/**
 * `sent_mention_display` (composer.rs:1316): project a sent prompt's raw
 * Markdown — mention links collapse to the same chip labels the composer
 * shows. `null` on the zero-allocation fast path: no scheme substring or
 * no valid mention parses.
 */
export function sentMentionDisplay(
  raw: string,
): { display: string; mentions: readonly SentMentionSpan[] } | null {
  if (!raw.includes(FILE_MENTION_SCHEME)) {
    return null;
  }
  const links = fileMentionLinks(raw);
  if (links.length === 0) {
    return null;
  }
  const labels = mentionDisplayLabels(links);
  let display = "";
  const mentions: SentMentionSpan[] = [];
  let rawAt = 0;
  links.forEach((link, ix) => {
    display += raw.slice(rawAt, link.start);
    const start = display.length;
    display += MENTION_SIDE_PAD;
    display += MENTION_PREFIX;
    display += labels[ix]!.replaceAll(" ", MENTION_SIDE_PAD);
    display += MENTION_SIDE_PAD;
    mentions.push({
      start,
      end: display.length,
      path: link.isDir ? `${link.path}/` : link.path,
      isDir: link.isDir,
    });
    rawAt = link.end;
  });
  display += raw.slice(rawAt);
  return { display, mentions };
}

// ---------------------------------------------------------------------------
// The path-tooltip state machine (composer.rs:1061-1129)
// ---------------------------------------------------------------------------

/** A path alone is not enough: the raw range is part of the hover identity. */
export interface MentionTooltipTarget {
  readonly start: number;
  readonly end: number;
  readonly path: string;
}

/** `MentionTooltipPhase` (composer.rs:1067-1078). */
export type MentionTooltipPhase =
  | { readonly kind: "hidden" }
  | { readonly kind: "waiting"; readonly target: MentionTooltipTarget; readonly generation: number }
  | { readonly kind: "visible"; readonly target: MentionTooltipTarget; readonly generation: number };

function phaseTarget(phase: MentionTooltipPhase): MentionTooltipTarget | null {
  return phase.kind === "hidden" ? null : phase.target;
}

function sameTarget(a: MentionTooltipTarget | null, b: MentionTooltipTarget | null): boolean {
  return a !== null && b !== null && a.start === b.start && a.end === b.end && a.path === b.path;
}

/**
 * `mention_tooltip_reduce` (composer.rs:1092): motion within the same chip
 * preserves both phases (jitter cannot starve the delay or flicker a
 * visible tooltip); a different chip restarts the wait; the pointer inside
 * the tooltip while visible keeps it; anything else hides it.
 */
export function mentionTooltipReduce(
  phase: MentionTooltipPhase,
  pointerTarget: MentionTooltipTarget | null,
  pointerInPopup: boolean,
  generation: number,
): MentionTooltipPhase {
  if (pointerTarget !== null) {
    if (sameTarget(phaseTarget(phase), pointerTarget)) {
      return phase;
    }
    return { kind: "waiting", target: pointerTarget, generation };
  }
  if (pointerInPopup && phase.kind === "visible") {
    return phase;
  }
  return { kind: "hidden" };
}

/**
 * `mention_tooltip_promote` (composer.rs:1106): a wait whose generation
 * matches the fired timer becomes visible when the chip still exists;
 * a matching generation with a dead target hides; a stale timer changes
 * nothing.
 */
export function mentionTooltipPromote(
  phase: MentionTooltipPhase,
  generation: number,
  targetIsLive: boolean,
): MentionTooltipPhase {
  if (phase.kind !== "waiting") {
    return phase;
  }
  if (phase.generation !== generation) {
    return phase;
  }
  return targetIsLive
    ? { kind: "visible", target: phase.target, generation }
    : { kind: "hidden" };
}

/** `mention_tooltip_contains` (composer.rs:1127): the tooltip stays up only
 * over its chip or its own popup. */
export function mentionTooltipContains(inChip: boolean, inPopup: boolean): boolean {
  return inChip || inPopup;
}

// ---------------------------------------------------------------------------
// Response currency + error copy (composer.rs:3961, 3969)
// ---------------------------------------------------------------------------

/**
 * `mention_response_is_current` (composer.rs:3961): a reply only lands when
 * its request generation matches AND a token is still open.
 */
export function mentionResponseIsCurrent(
  state: { request: number; token: unknown },
  request: number,
): boolean {
  return state.request === request && state.token !== null && state.token !== undefined;
}

/**
 * `mention_error_message` (composer.rs:3969): a failure must never render as
 * "No matching files" — cross-device searches fail for reasons the user can
 * act on. `timeout`/`parked` (web-client-only kinds) read as unreachable:
 * both mean the reply never came back over the transport.
 */
export function mentionErrorMessage(kind: RpcErrorKind): string {
  switch (kind) {
    case "unknown-method":
      return "The session's device runs an older zeron — update it to search its files";
    case "transport":
    case "closed":
    case "timeout":
    case "parked":
      return "The session's device is unreachable";
    default:
      return "File search failed";
  }
}
