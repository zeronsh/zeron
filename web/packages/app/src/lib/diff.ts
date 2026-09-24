/**
 * The web client's pure patch parser — a port of the desktop's `parse_patch`
 * (`crates/ui/src/changes.rs:432`) and the resolution/scope/geometry helpers
 * used by the Changes surface. The parser is tolerant: unknown header lines
 * are skipped, truncated hunks keep what parsed so far, and file statuses are
 * inferred from the standard git header fields.
 *
 * `FileDiff.notices` carries only PARSER-collected notices (mode changes);
 * the display list — status word, binary marker, parser notices, in that
 * order — is `fileNotices()`, exactly like the desktop's `file_notices`
 * (changes.rs:562). Consumers never index `.notices` directly.
 *
 * Geometry is analytic, never measured: `gutterWidth` fits the number
 * columns, `visualColumns`/`horizontalGeometry` feed the code plane's
 * intrinsic width, and `bodyHeight`/`bodyHeightWith` drive the 180ms fold
 * tween without a layout pass.
 *
 * Colors come from the theme's `--rb-diff-add`/`--rb-diff-delete`/
 * `--rb-diff-hunk` tokens; per-line syntax tokens reuse `lib/syntax.ts` and
 * the theme's `--rb-syntax-*` roles. No color is hardcoded here.
 */

import { cardHeight, DRAFT_CARD_HEIGHT, type CommentSide, type ReviewComment } from "./review-comments";

/** One source line on the old or new side of a patch hunk. */
export type LineKind = "context" | "add" | "del" | "meta";

export interface DiffLine {
  readonly kind: LineKind;
  readonly oldNo: number | null;
  readonly newNo: number | null;
  readonly text: string;
}

export interface Hunk {
  readonly header: string;
  readonly lines: readonly DiffLine[];
}

export type FileStatus = "added" | "deleted" | "modified" | "renamed";

export interface FileDiff {
  /** Display path (post-change side). */
  readonly path: string;
  /** Pre-rename path when different. */
  readonly oldPath: string | null;
  readonly status: FileStatus;
  readonly binary: boolean;
  /** Parser-collected notices only (mode changes) — see `fileNotices()`. */
  readonly notices: readonly string[];
  readonly hunks: readonly Hunk[];
  readonly additions: number;
  readonly deletions: number;
  /**
   * Largest line number on either side — drives the gutter width analytically
   * (a fixed column overflowed past 4 digits; user report).
   */
  readonly maxLine: number;
}

// ---------------------------------------------------------------------------
// Layout constants (changes.rs:67-94, 1486) — the analytic row contract.
// ---------------------------------------------------------------------------

/** `FILE_HEADER_HEIGHT` — `surface_chrome::HEADER_HEIGHT` = `TITLEBAR_HEIGHT`. */
export const FILE_HEADER_HEIGHT = 38;
export const HUNK_HEADER_HEIGHT = 28;
export const DIFF_LINE_HEIGHT = 21;
export const NOTICE_HEIGHT = 24;
/** Trailing pad closing an expanded body (`BODY_BOTTOM_PAD`). */
export const BODY_BOTTOM_PAD = 8;
/** The classic minimum line-number column (`GUTTER_WIDTH`). */
export const GUTTER_WIDTH = 36;
/** Unified marker column (`MARKER_WIDTH`). */
export const MARKER_WIDTH = 28;
/** The colored bar on +/− rows (`ACCENT_BAR_WIDTH`). */
export const ACCENT_BAR_WIDTH = 3;
export const SPLIT_MARKER_WIDTH = 18;
export const SPLIT_DIVIDER_WIDTH = 1;
export const DIFF_TEXT_SIZE = 12;
/** `DIFF_TAB_SIZE` — visual tab advance for the content-width estimate. */
export const DIFF_TAB_SIZE = 4;
export const UNIFIED_CODE_PADDING_LEFT = 12;
export const SPLIT_CODE_PADDING_LEFT = 6;
/** Scroll-extent slack on the right of every code plane. */
export const CODE_PADDING_RIGHT = 24;
/** Ceiling on what a fold tween's stand-in materializes (`FOLD_TWEEN_MAX_PX`). */
export const FOLD_TWEEN_MAX_PX = 2400;
/** Tween arming window after a fold toggle (COLLAPSE's 180ms plus margin). */
export const FOLD_TWEEN_WINDOW_MS = 400;

/**
 * Derived per-file notice rows: status word → binary marker → parser
 * notices. The only place file status is expressed — the header row carries
 * neither (`file_notices`, changes.rs:562).
 */
export function fileNotices(file: FileDiff): string[] {
  const notices: string[] = [];
  switch (file.status) {
    case "added":
      notices.push("New file");
      break;
    case "deleted":
      notices.push("Deleted file");
      break;
    case "renamed":
      notices.push(`Renamed from ${file.oldPath ?? "?"}`);
      break;
    case "modified":
      break;
  }
  if (file.binary) {
    notices.push("Binary file — contents not shown");
  }
  notices.push(...file.notices);
  return notices;
}

/** Width of one line-number gutter column, fitted to the file's max line. */
export function gutterWidth(file: FileDiff): number {
  const digits = Math.max(1, Math.floor(Math.log10(Math.max(file.maxLine, 1)))) + 1;
  return Math.max(GUTTER_WIDTH, digits * 6.6 + 8 + 6);
}

// ---------------------------------------------------------------------------
// Horizontal geometry (changes.rs:264-340)
// ---------------------------------------------------------------------------

const WIDE_CHAR =
  /[\u1100-\u115F\u2E80-\u303E\u3041-\u33FF\u3400-\u4DBF\u4E00-\u9FFF\uA000-\uA4CF\uA960-\uA97F\uAC00-\uD7A3\uF900-\uFAFF\uFE10-\uFE19\uFE30-\uFE6F\uFF00-\uFF60\uFFE0-\uFFE6]/u;
const COMBINING_MARK = /\p{M}/u;

/** Display columns of one character: wide glyphs 2, combining marks 0. */
function charColumns(ch: string): number {
  if (COMBINING_MARK.test(ch)) {
    return 0;
  }
  if (WIDE_CHAR.test(ch)) {
    return 2;
  }
  return 1;
}

/**
 * Terminal-style display columns, including tab stops and wide Unicode
 * glyphs (`visual_columns`, changes.rs:306). Feeds the content-width
 * estimate only — raw text with literal tabs is still drawn verbatim.
 */
export function visualColumns(text: string): number {
  let columns = 0;
  for (const ch of text) {
    if (ch === "\t") {
      columns += DIFF_TAB_SIZE - (columns % DIFF_TAB_SIZE);
    } else {
      columns += charColumns(ch);
    }
  }
  return columns;
}

/** Width inputs independent of the active window's font metrics. */
export interface DiffHorizontalGeometry {
  readonly maxCodeColumns: number;
  readonly maxGutterWidth: number;
}

/** `DiffHorizontalGeometry::from_file` — the widest line, in columns. */
export function horizontalGeometry(file: FileDiff): DiffHorizontalGeometry {
  let maxCodeColumns = 0;
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      const columns = visualColumns(line.text);
      if (columns > maxCodeColumns) {
        maxCodeColumns = columns;
      }
    }
  }
  return { maxCodeColumns, maxGutterWidth: gutterWidth(file) };
}

/**
 * Unified code-plane extent, compensating for the file-local gutter so every
 * viewport's effective scroll range is identical (`unified_content_width`).
 */
export function unifiedContentWidth(
  maxTextWidth: number,
  maxGutterWidth: number,
  gutterWidth: number,
): number {
  return maxTextWidth + UNIFIED_CODE_PADDING_LEFT + CODE_PADDING_RIGHT + 2 * (maxGutterWidth - gutterWidth);
}

/**
 * Split code-plane extent — one gutter per half, both halves synchronized
 * (`split_content_width`).
 */
export function splitContentWidth(
  maxTextWidth: number,
  maxGutterWidth: number,
  gutterWidth: number,
): number {
  return maxTextWidth + SPLIT_CODE_PADDING_LEFT + CODE_PADDING_RIGHT + (maxGutterWidth - gutterWidth);
}

// ---------------------------------------------------------------------------
// The parser (changes.rs:432-559)
// ---------------------------------------------------------------------------

/** Strip the `a/` or `b/` prefix git uses on file paths. */
function stripGitPrefix(path: string): string {
  if (path.startsWith("a/") || path.startsWith("b/")) {
    return path.slice(2);
  }
  return path;
}

/** Unquote a quoted git path (spaces / unicode). */
function unquote(s: string): string {
  const trimmed = s.trim();
  if (trimmed.length >= 2 && trimmed.startsWith('"') && trimmed.endsWith('"')) {
    return trimmed
      .slice(1, -1)
      .replace(/\\"/g, '"')
      .replace(/\\\\/g, "\\");
  }
  return trimmed;
}

/**
 * Split the tail of a `diff --git a/… b/…` line into (old, next) paths.
 * Handles quoted paths; for unquoted paths with spaces favors the last ` b/`
 * separator — git's own convention.
 */
function parseGitPaths(rest: string): { old: string; next: string } {
  let bIx = rest.lastIndexOf(" b/");
  if (bIx < 0) {
    bIx = rest.lastIndexOf(' "b/');
  }
  if (bIx >= 0) {
    const oldPart = rest.slice(0, bIx);
    const newPart = rest.slice(bIx + 1);
    const old = stripGitPrefix(unquote(oldPart));
    const next = stripGitPrefix(unquote(newPart));
    return { old, next };
  }
  const single = stripGitPrefix(unquote(rest));
  return { old: single, next: single };
}

/**
 * Parse one `@@ -a[,b] +c[,d] @@ …` header into starting line numbers on
 * each side (one-based). The count fields are ignored — the row model
 * carries `oldNo`/`newNo` per line.
 */
function parseHunkHeader(line: string): { oldStart: number; newStart: number } | null {
  const rest = line.startsWith("@@") ? line.slice(2) : null;
  if (rest === null) {
    return null;
  }
  const minusIx = rest.indexOf("-");
  const plusIx = rest.indexOf("+");
  if (minusIx < 0 || plusIx < 0 || plusIx < minusIx) {
    return null;
  }
  const oldField = rest
    .slice(minusIx + 1, plusIx)
    .split(/[,\s]/)
    .find((part) => part.length > 0);
  const newField = rest
    .slice(plusIx + 1)
    .split(/[,\s]/)
    .find((part) => part.length > 0);
  if (oldField === undefined || newField === undefined) {
    return null;
  }
  const oldStart = Number.parseInt(oldField, 10);
  const newStart = Number.parseInt(newField, 10);
  if (!Number.isFinite(oldStart) || !Number.isFinite(newStart)) {
    return null;
  }
  return { oldStart, newStart };
}

/**
 * Parse a unified git patch into file sections. Tolerant: unknown header
 * lines are skipped, truncated hunks keep what parsed so far.
 */
export function parsePatch(patch: string): FileDiff[] {
  const files: MutableFileDiff[] = [];
  let inHunk = false;
  let oldNo = 0;
  let newNo = 0;

  const flushFile = (raw: string, current: MutableFileDiff | undefined): void => {
    if (current === undefined) {
      return;
    }
    if (raw.startsWith("new file mode")) {
      current.status = "added";
    } else if (raw.startsWith("deleted file mode")) {
      current.status = "deleted";
    } else if (raw.startsWith("rename from ")) {
      current.status = "renamed";
      current.oldPath = raw.slice("rename from ".length).trim();
    } else if (raw.startsWith("rename to ")) {
      current.status = "renamed";
      current.path = raw.slice("rename to ".length).trim();
    } else if (raw.startsWith("Binary files") || raw.startsWith("GIT binary patch")) {
      current.binary = true;
    } else if (raw.startsWith("new mode ")) {
      current.notices.push(`Mode changed to ${raw.slice("new mode ".length).trim()}`);
    } else if (raw.startsWith("+++ ")) {
      const path = raw.slice(4).trim();
      if (path === "/dev/null") {
        current.status = "deleted";
      } else if (current.oldPath === null) {
        current.path = stripGitPrefix(path);
      }
    } else if (raw.startsWith("--- ") && raw.slice(4).trim() === "/dev/null") {
      current.status = "added";
    }
    // index …, similarity index …, old mode … etc. — silently skipped.
  };

  for (const raw of patch.split(/\r?\n/)) {
    if (raw.startsWith("diff --git ")) {
      const rest = raw.slice("diff --git ".length);
      const { old, next } = parseGitPaths(rest);
      const oldPath = old !== next ? old : null;
      files.push({
        path: next,
        oldPath,
        status: "modified",
        binary: false,
        notices: [],
        hunks: [],
        additions: 0,
        deletions: 0,
        maxLine: 0,
      });
      inHunk = false;
      continue;
    }
    const file = files[files.length - 1];
    if (file === undefined) {
      continue;
    }

    if (raw.startsWith("@@")) {
      const header = parseHunkHeader(raw);
      if (header !== null) {
        oldNo = header.oldStart;
        newNo = header.newStart;
        file.hunks.push({ header: raw, lines: [] });
        inHunk = true;
      }
      continue;
    }

    if (inHunk) {
      const marker = raw[0];
      const body = raw.slice(1);
      if (marker === "+") {
        file.additions += 1;
        const line: DiffLine = { kind: "add", oldNo: null, newNo, text: body };
        newNo += 1;
        file.hunks[file.hunks.length - 1]!.lines.push(line);
        file.maxLine = Math.max(file.maxLine, line.oldNo ?? 0, line.newNo ?? 0);
        continue;
      }
      if (marker === "-") {
        file.deletions += 1;
        const line: DiffLine = { kind: "del", oldNo, newNo: null, text: body };
        oldNo += 1;
        file.hunks[file.hunks.length - 1]!.lines.push(line);
        file.maxLine = Math.max(file.maxLine, line.oldNo ?? 0, line.newNo ?? 0);
        continue;
      }
      if (marker === " " || marker === undefined) {
        const line: DiffLine = { kind: "context", oldNo, newNo, text: body };
        oldNo += 1;
        newNo += 1;
        file.hunks[file.hunks.length - 1]!.lines.push(line);
        file.maxLine = Math.max(file.maxLine, line.oldNo ?? 0, line.newNo ?? 0);
        continue;
      }
      if (marker === "\\") {
        const line: DiffLine = { kind: "meta", oldNo: null, newNo: null, text: raw.replace(/^\\/, "").trim() };
        file.hunks[file.hunks.length - 1]!.lines.push(line);
        continue;
      }
      // Non-hunk line ends the hunk; reprocess as a header.
      inHunk = false;
    }

    flushFile(raw, file);
  }

  // Seal each file's data into a readonly value.
  return files.map((file): FileDiff => ({
    path: file.path,
    oldPath: file.oldPath,
    status: file.status,
    binary: file.binary,
    notices: file.notices,
    hunks: file.hunks.map((h) => ({ header: h.header, lines: h.lines })),
    additions: file.additions,
    deletions: file.deletions,
    maxLine: file.maxLine,
  }));
}

interface MutableFileDiff {
  path: string;
  oldPath: string | null;
  status: FileStatus;
  binary: boolean;
  notices: string[];
  hunks: { header: string; lines: DiffLine[] }[];
  additions: number;
  deletions: number;
  maxLine: number;
}

/**
 * Cap a file's hunks at `maxLines` total diff lines, appending a parser
 * notice when lines were dropped (`truncate_file_lines`, changes.rs:585).
 * The transcript renders a tool diff as ONE stacked element, so an unbounded
 * diff would build tens of thousands of rows; the gutter refits to what
 * actually renders. Immutable — returns the input when under the cap.
 */
export function truncateFileLines(file: FileDiff, maxLines: number): FileDiff {
  const total = file.hunks.reduce((sum, hunk) => sum + hunk.lines.length, 0);
  if (total <= maxLines) {
    return file;
  }
  let budget = maxLines;
  const hunks: Hunk[] = [];
  for (const hunk of file.hunks) {
    if (budget === 0) {
      break;
    }
    const lines = hunk.lines.length > budget ? hunk.lines.slice(0, budget) : hunk.lines;
    budget -= lines.length;
    hunks.push({ header: hunk.header, lines });
  }
  let maxLine = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      maxLine = Math.max(maxLine, line.oldNo ?? 0, line.newNo ?? 0);
    }
  }
  return {
    ...file,
    hunks,
    notices: [...file.notices, `Diff truncated — showing first ${maxLines} of ${total} lines`],
    maxLine,
  };
}

// ---------------------------------------------------------------------------
// Resolution / phases / scopes
// ---------------------------------------------------------------------------

/**
 * Resolve a per-checkout diff list to the diff that matches the given chat.
 * `checkout_id` first, then device+cwd, then cwd alone — desktop parity
 * (`crates/ui/src/changes.rs::resolve_diff`).
 */
export function resolveDiff<T extends { readonly checkoutId: string; readonly deviceId: string; readonly cwd: string }>(
  diffs: readonly T[],
  chat: { readonly checkoutId: string | null; readonly deviceId: string; readonly cwd: string | null },
): T | null {
  if (chat.checkoutId !== null) {
    const match = diffs.find((d) => d.checkoutId === chat.checkoutId);
    if (match !== undefined) {
      return match;
    }
  }
  const cwd = chat.cwd;
  if (cwd === null) {
    return null;
  }
  const local = diffs.find((d) => d.deviceId === chat.deviceId && d.cwd === cwd);
  if (local !== undefined) {
    return local;
  }
  return diffs.find((d) => d.cwd === cwd) ?? null;
}

export type DiffPhase = "preparing" | "clean" | "list";

export function diffPhase(resolved: { readonly patch: string; readonly files: readonly unknown[] } | null): DiffPhase {
  if (resolved === null) {
    return "preparing";
  }
  if (resolved.patch.trim().length === 0 && resolved.files.length === 0) {
    return "clean";
  }
  return "list";
}

/**
 * The single-frame arm of the desktop's `apply_diff_frame`: an unknown
 * checkout appends, a known one upserts in place, and an identical frame is
 * a no-op (identity-stable, so `getSnapshot` never churns). List frames
 * replace wholesale — that branch lives in the store's watch handler.
 */
export function upsertDiffFrame<T extends { readonly checkoutId: string }>(
  list: readonly T[],
  item: T,
): readonly T[] {
  const ix = list.findIndex((row) => row.checkoutId === item.checkoutId);
  if (ix < 0) {
    return [...list, item];
  }
  if (list[ix] === item) {
    return list;
  }
  const next = list.slice();
  next[ix] = item;
  return next;
}

/**
 * The diff scope — Working tree / Branch changes / Latest turn, plus the
 * commit-pinned flavour a commit-diff tab mounts (ticket 27 reaches it via
 * `addDiffSurface(chatId, "commit", …)`; no scope row exposes it).
 * History is NOT a scope on the web: it is its own pane surface (ticket 27).
 */
export type DiffScope = "workingTree" | "branch" | "turn" | "commit";

export const DIFF_SCOPE_LABELS: Readonly<Record<DiffScope, string>> = {
  workingTree: "Working tree",
  branch: "Branch changes",
  turn: "Latest turn",
  commit: "Commit",
};

/** `DiffScope::ALL` — the scope menu's rows; commit is tab-mounted only. */
export const DIFF_SCOPE_CHIPS: readonly DiffScope[] = ["workingTree", "branch", "turn"];

/** Wire value for `GetCheckoutDiff` `mode`. */
export function scopeMode(scope: DiffScope): string {
  switch (scope) {
    case "workingTree":
      return "workingTree";
    case "branch":
      return "branch";
    case "turn":
      return "turn";
    case "commit":
      return "commit";
  }
}

export interface ScopeLabelInputs {
  readonly scope: DiffScope;
  readonly count: number;
  readonly base?: string | null;
}

export function scopeLabel({ scope, count, base }: ScopeLabelInputs): string {
  const files = count === 1 ? "file" : "files";
  switch (scope) {
    case "workingTree":
      return count === 1 ? "1 Uncommitted change" : `${count} Uncommitted changes`;
    case "branch":
      return base !== undefined && base !== null
        ? `${count} Changed ${files} vs ${base}`
        : `${count} Changed ${files}`;
    case "turn":
      return `${count} Changed ${files} this turn`;
    case "commit":
      return `${count} Changed ${files} in this commit`;
  }
}

/** Empty-state copy per scope. */
export function cleanMessage(scope: DiffScope, base: string | null): string {
  switch (scope) {
    case "workingTree":
      return "No uncommitted changes";
    case "branch":
      return base === null ? "No branch changes" : `No changes vs ${base}`;
    case "turn":
      return "No changes this turn";
    case "commit":
      return "No changes in this commit";
  }
}

/**
 * Default base for the branch scope: first branch that's not the current one,
 * else `main`/`master` if present, else first entry.
 */
export function defaultBaseRef(branches: readonly string[], current: string | null): string | null {
  const first = branches[0];
  if (first === undefined) {
    return null;
  }
  if (current !== first) {
    return first;
  }
  for (const candidate of ["main", "master"]) {
    if (branches.includes(candidate)) {
      return candidate;
    }
  }
  const other = branches.find((branch) => branch !== current);
  return other ?? first;
}

// ---------------------------------------------------------------------------
// Split pairing (changes.rs:648-757)
// ---------------------------------------------------------------------------

/**
 * Pair a hunk's lines into split rows. Pure & index-only, the desktop
 * reference (`changes.rs::split_pairs`). A run of deletions followed by
 * additions yields one paired row per line; the longer side's leftovers
 * become one-sided rows. A deletion arriving after additions opens a new
 * block. The marker (`\ No newline at end of file`) belongs to whichever
 * side it trails and pairs with itself, never with code.
 */
export type SplitPair = readonly [number | null, number | null];

export function splitPairs(lines: readonly DiffLine[]): SplitPair[] {
  return splitPairsUpto(lines, Number.POSITIVE_INFINITY);
}

export function splitPairsUpto(lines: readonly DiffLine[], maxRows: number): SplitPair[] {
  const pairs: SplitPair[] = [];
  let dels: number[] = [];
  let adds: number[] = [];
  let delMeta: number[] = [];
  let addMeta: number[] = [];
  let pending: LineKind | null = null;

  const flush = (): void => {
    const drain = (left: number[], right: number[]): void => {
      const len = Math.max(left.length, right.length);
      for (let ix = 0; ix < len; ix += 1) {
        if (pairs.length >= maxRows) {
          break;
        }
        const l = ix < left.length ? left[ix]! : null;
        const r = ix < right.length ? right[ix]! : null;
        pairs.push([l, r] as const);
      }
      left.length = 0;
      right.length = 0;
    };
    drain(dels, adds);
    drain(delMeta, addMeta);
  };

  for (let ix = 0; ix < lines.length; ix += 1) {
    const line = lines[ix]!;
    switch (line.kind) {
      case "del": {
        if (adds.length > 0 || delMeta.length > 0 || addMeta.length > 0) {
          flush();
        }
        const remaining = maxRows - pairs.length;
        if (remaining === 0) {
          break;
        }
        if (dels.length < remaining) {
          dels.push(ix);
        }
        pending = "del";
        break;
      }
      case "add": {
        if (addMeta.length > 0) {
          flush();
        }
        const remaining = maxRows - pairs.length;
        if (remaining === 0) {
          break;
        }
        if (adds.length < remaining) {
          adds.push(ix);
        }
        pending = "add";
        break;
      }
      case "meta": {
        if (pending === "del") {
          delMeta.push(ix);
        } else if (pending === "add") {
          addMeta.push(ix);
        } else {
          delMeta.push(ix);
          addMeta.push(ix);
        }
        break;
      }
      case "context": {
        flush();
        if (pairs.length >= maxRows) {
          break;
        }
        pairs.push([ix, ix] as const);
        pending = "context";
        break;
      }
    }
  }
  flush();
  return pairs.slice(0, maxRows);
}

/**
 * Fold one pair of split indices into two line refs, or `null` on empty
 * sides. Used to bind review comments to a row.
 */
export function pairLine(lines: readonly DiffLine[], pair: SplitPair): { left: DiffLine | null; right: DiffLine | null } {
  const left = pair[0] !== null ? lines[pair[0]] ?? null : null;
  const right = pair[1] !== null ? lines[pair[1]] ?? null : null;
  return { left, right };
}

// ---------------------------------------------------------------------------
// The row model (changes.rs:1177-1369)
// ---------------------------------------------------------------------------

export type DiffMode = "unified" | "split";

/**
 * One fold's state (`FileFold`, changes.rs:1466). `folding` marks a body
 * mid-tween: the row list carries a single height-animated stand-in until
 * the window elapses.
 */
export interface FileFold {
  readonly collapsed: boolean;
  /** Bumped per toggle — keys the height tween + chevron transition. */
  readonly epoch: number;
  readonly from: number;
  readonly to: number;
  /** When the toggle happened (epoch ms), null once settled. */
  readonly toggledAt: number | null;
  /** True while the stand-in row owns the body. */
  readonly folding: boolean;
}

/** Flatten one file into rows for a virtualized list — see `flattenFiles`. */
export type DiffRow =
  | { readonly kind: "fileHeader"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly expanded: boolean; readonly animating: boolean }
  | { readonly kind: "hunkHeader"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly hunkIx: number }
  | { readonly kind: "notice"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly messageIx: number }
  | { readonly kind: "line"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly hunkIx: number; readonly lineIx: number }
  | { readonly kind: "splitLine"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly hunkIx: number; readonly pairIx: number; readonly left: number | null; readonly right: number | null }
  /** One staged diff comment inline after its anchor line (`DiffRow::CommentCard`). */
  | { readonly kind: "commentCard"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly comment: ReviewComment }
  /** The open diff-side draft after its anchor line (`DiffRow::CommentDraft`). */
  | { readonly kind: "commentDraft"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly path: string; readonly side: CommentSide; readonly line: number; readonly editingId: string | null }
  | { readonly kind: "bodyPad"; readonly id: string; readonly file: FileDiff; readonly fileIx: number }
  /** One body mid-fold-tween: a height-animated, clipped stand-in row. */
  | { readonly kind: "foldingBody"; readonly id: string; readonly file: FileDiff; readonly fileIx: number; readonly from: number; readonly to: number; readonly epoch: number };

/**
 * `line_anchor` (changes.rs:783-789): a deletion only exists in the
 * pre-change file; everything else is cited against the post-change file,
 * which is what the agent edits. Meta lines carry no anchor.
 */
export function diffLineAnchor(line: DiffLine): { side: CommentSide; line: number } | null {
  if (line.kind === "meta") {
    return null;
  }
  if (line.kind === "del") {
    return line.oldNo !== null ? { side: "old", line: line.oldNo } : null;
  }
  return line.newNo !== null ? { side: "new", line: line.newNo } : null;
}

/**
 * `pair_anchors` (changes.rs:768-780): a split row's two side anchors,
 * deduped when the row is a mirrored context pair. Cards for Old-side notes
 * still render (they are pushed by the row, not the column), so switching
 * layouts never hides an already-staged one.
 */
export function pairAnchors(
  lines: readonly DiffLine[],
  pair: SplitPair,
): ({ side: CommentSide; line: number } | null)[] {
  const anchor = (ix: number | null): { side: CommentSide; line: number } | null => {
    if (ix === null) {
      return null;
    }
    const line = lines[ix];
    return line === undefined ? null : diffLineAnchor(line);
  };
  const left = anchor(pair[0]);
  const right = anchor(pair[1]);
  return left !== null && right !== null && left.side === right.side && left.line === right.line
    ? [left]
    : [left, right];
}

/** The diff-side draft's anchor, as the row model carries it. */
export interface DiffDraftAnchor {
  readonly path: string;
  readonly side: CommentSide;
  readonly line: number;
  /** The staged comment being edited — the draft row's button reads "Save". */
  readonly editingId: string | null;
}

/** Capacity hint only — comment cards are not counted (changes.rs:1252-1257). */
export function bodyRowCount(file: FileDiff): number {
  const lines = file.hunks.reduce((sum, hunk) => sum + hunk.lines.length, 0);
  return fileNotices(file).length + file.hunks.length + lines + 1;
}

/**
 * One expanded file's body rows: notices → hunk headers → (unified lines or
 * split pairs, each followed by its comment cards and the draft row when an
 * anchor matches) → trailing `BodyPad` (`body_rows`, changes.rs:1266-1337).
 * `comments` is the file's own diff-sourced staged slice, in staged order,
 * with the comment being edited already excluded by the caller.
 */
export function bodyRows(
  file: FileDiff,
  fileIx: number,
  mode: DiffMode,
  comments: readonly ReviewComment[] = [],
  draft: DiffDraftAnchor | null = null,
): DiffRow[] {
  const rows: DiffRow[] = [];
  const pushCards = (anchors: readonly ({ side: CommentSide; line: number } | null)[]): void => {
    for (const anchor of anchors) {
      if (anchor === null) {
        continue;
      }
      for (const comment of comments) {
        const source = comment.source;
        if (
          source.kind === "diff" &&
          source.side === anchor.side &&
          comment.line === anchor.line
        ) {
          rows.push({ kind: "commentCard", id: rowId(fileIx, "c", comment.id), file, fileIx, comment });
        }
      }
      if (draft !== null && draft.side === anchor.side && draft.line === anchor.line) {
        rows.push({
          kind: "commentDraft",
          id: rowId(fileIx, "d", `${draft.side}${draft.line}`),
          file,
          fileIx,
          path: draft.path,
          side: draft.side,
          line: draft.line,
          editingId: draft.editingId,
        });
      }
    }
  };
  const notices = fileNotices(file);
  for (let ix = 0; ix < notices.length; ix += 1) {
    rows.push({ kind: "notice", id: rowId(fileIx, "n", ix), file, fileIx, messageIx: ix });
  }
  for (let hunkIx = 0; hunkIx < file.hunks.length; hunkIx += 1) {
    rows.push({ kind: "hunkHeader", id: rowId(fileIx, "k", hunkIx), file, fileIx, hunkIx });
    const hunk = file.hunks[hunkIx]!;
    if (mode === "split") {
      const pairs = splitPairs(hunk.lines);
      for (let pairIx = 0; pairIx < pairs.length; pairIx += 1) {
        const [left, right] = pairs[pairIx]!;
        rows.push({ kind: "splitLine", id: rowId(fileIx, `s${hunkIx}`, pairIx), file, fileIx, hunkIx, pairIx, left, right });
        pushCards(pairAnchors(hunk.lines, pairs[pairIx]!));
      }
    } else {
      for (let lineIx = 0; lineIx < hunk.lines.length; lineIx += 1) {
        const line = hunk.lines[lineIx]!;
        rows.push({ kind: "line", id: rowId(fileIx, `${hunkIx}`, lineIx), file, fileIx, hunkIx, lineIx });
        pushCards([diffLineAnchor(line)]);
      }
    }
  }
  rows.push({ kind: "bodyPad", id: rowId(fileIx, "pad", 0), file, fileIx });
  return rows;
}

/** Analytic expanded-body height — `bodyHeightWith` for one mode. */
export function bodyHeightWith(
  file: FileDiff,
  mode: DiffMode,
  comments: readonly ReviewComment[] = [],
  draft: DiffDraftAnchor | null = null,
  lineHeight: number = DIFF_LINE_HEIGHT,
): number {
  return bodyRows(file, 0, mode, comments, draft).reduce(
    (sum, row) => sum + estimateRowHeight(row, lineHeight),
    0,
  );
}

/** `body_height(file)` — the unified analytic height. */
export function bodyHeight(file: FileDiff, lineHeight: number = DIFF_LINE_HEIGHT): number {
  return bodyHeightWith(file, "unified", [], null, lineHeight);
}

/**
 * Flatten all files into the list's rows. A collapsed file contributes ONLY
 * its header; a mid-tween file contributes header + one `foldingBody`
 * stand-in (`flatten_rows`, changes.rs:1343-1375). `comments` is the whole
 * staged set (each file takes its own path's diff-sourced slice); `draft`
 * is the diff-side draft anchor, whatever file it belongs to.
 */
export function flattenFiles(
  files: readonly FileDiff[],
  mode: DiffMode,
  folds: ReadonlyMap<string, FileFold>,
  comments: readonly ReviewComment[] = [],
  draft: DiffDraftAnchor | null = null,
): DiffRow[] {
  const rows: DiffRow[] = [];
  for (let fileIx = 0; fileIx < files.length; fileIx += 1) {
    const file = files[fileIx]!;
    const fold = folds.get(file.path);
    rows.push({
      kind: "fileHeader",
      id: rowId(fileIx, "h", 0),
      file,
      fileIx,
      expanded: fold === undefined || !fold.collapsed,
      animating: fold !== undefined && fold.folding,
    });
    if (fold !== undefined && fold.folding) {
      rows.push({ kind: "foldingBody", id: rowId(fileIx, "fold", fold.epoch), file, fileIx, from: fold.from, to: fold.to, epoch: fold.epoch });
      continue;
    }
    if (fold !== undefined && fold.collapsed) {
      continue;
    }
    const fileComments = comments.filter(
      (comment) => comment.source.kind === "diff" && comment.path === file.path,
    );
    const fileDraft = draft !== null && draft.path === file.path ? draft : null;
    rows.push(...bodyRows(file, fileIx, mode, fileComments, fileDraft));
  }
  return rows;
}

function rowId(fileIx: number, scope: string, ix: string | number): string {
  return `${fileIx}:${scope}:${ix}`;
}

/**
 * The comment-adder's anchor geometry (ticket 23 renders the button; the
 * constants live here so both layouts agree). `commentAdderLeft`
 * (changes.rs:4557) — measured from the row's left edge, the adder centers
 * itself in the gutter column it belongs to.
 */
export const COMMENT_ADDER_SIZE = 16;

export function commentAdderLeft(side: "old" | "new", gutterPx: number): number {
  const column = side === "new" ? gutterPx : 0;
  return ACCENT_BAR_WIDTH + column + (gutterPx - COMMENT_ADDER_SIZE) / 2;
}

/** The split row's `+` sits in the right column only (`split_adder_left`). */
export function splitAdderLeft(gutterPx: number): number {
  return ACCENT_BAR_WIDTH + (gutterPx - COMMENT_ADDER_SIZE) / 2;
}

/**
 * Estimate the rendered height of one row (analytic until measured).
 * `lineHeight` is the code-size-scaled row (`diff_line_height`,
 * lib/typography.ts); it defaults to the 12.5px-code setting's 21px so pure
 * callers and older tests need no setting.
 */
export function estimateRowHeight(row: DiffRow, lineHeight: number = DIFF_LINE_HEIGHT): number {
  switch (row.kind) {
    case "fileHeader":
      return FILE_HEADER_HEIGHT;
    case "hunkHeader":
      return HUNK_HEADER_HEIGHT;
    case "notice":
      return NOTICE_HEIGHT;
    case "line":
    case "splitLine":
      return lineHeight;
    case "commentCard":
      // Analytic, never measured (`DiffRow::height`, changes.rs:1234-1237).
      return cardHeight(row.comment.body);
    case "commentDraft":
      return DRAFT_CARD_HEIGHT;
    case "bodyPad":
      return BODY_BOTTOM_PAD;
    case "foldingBody":
      // The tween's end state; the animated element measures itself.
      return row.to;
  }
}

/**
 * Build the per-file patch-key fingerprint: a checksum + scope pair, folded
 * into the parse cache key. The desktop calls this `parse_key`
 * (changes.rs:1867); identical text returns the same identity.
 */
export function parseKey(checkoutId: string, checksum: string, scope: DiffScope, baseRef: string | null): string {
  const base = baseRef ?? "";
  return `${checkoutId}:${checksum}:${scope}:${base}`;
}

/**
 * Summarize the per-file additions + deletions — the corner badge on a file
 * header (`+12 -4`).
 */
export function fileCounts(file: FileDiff): string {
  if (file.binary) {
    return "Binary";
  }
  if (file.additions === 0 && file.deletions === 0) {
    return "";
  }
  const add = file.additions > 0 ? `+${file.additions}` : "";
  const del = file.deletions > 0 ? `-${file.deletions}` : "";
  return add.length > 0 && del.length > 0 ? `${add} ${del}` : add + del;
}
