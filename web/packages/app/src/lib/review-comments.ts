/**
 * Review comments — `crates/ui/src/comments.rs` ported 1:1: the
 * `ReviewComment` model, the plain-text codec (`with_comments` — the writer;
 * `lib/badges.ts` owns the reader half `extract_badge`, landed with ticket
 * 20), and the card geometry (`card_height` and friends) the diff and editor
 * surfaces share.
 *
 * Comments are NEVER sent as structured wire data — the only transport is
 * the fold-in: `withComments` appends a header + `"- {location}"` bullet
 * list to the prompt at send time, and the transcript recovers them by
 * pattern-matching that block back out (`splitBadges`).
 */

import { COMMENT_BLOCK_HEADER, COMMENT_ONLY_TEXT, REVIEW_COMMENT_BLOCK_HEADER } from "./badges";
import { mintId } from "./id";

export type { BadgeDetail, Extractor, MessageBadge } from "./badges";
export {
  COMMENT_BLOCK_HEADER,
  COMMENT_ONLY_TEXT,
  REVIEW_COMMENT_BLOCK_HEADER,
  chipLabel,
  extractCommentBadge,
  parseBullets,
  splitBadges,
} from "./badges";

/** `CommentSide` (comments.rs:8-20) — `L` cites the original file, `R` the changed one. */
export type CommentSide = "old" | "new";

/** `CommentSource` (comments.rs:23-29). */
export type CommentSource =
  | { readonly kind: "diff"; readonly side: CommentSide; readonly oldPath: string | null }
  | { readonly kind: "file" };

/** `ReviewComment` (comments.rs:32-40). */
export interface ReviewComment {
  readonly id: string;
  /** The current workspace path; an Old-side diff comment cites `oldPath` instead. */
  readonly path: string;
  readonly line: number;
  readonly body: string;
  readonly source: CommentSource;
}

/** `ReviewComment::new` + `renamed_from` (comments.rs:44-83), one constructor. */
export function newDiffComment(
  path: string,
  side: CommentSide,
  line: number,
  body: string,
  oldPath: string | null = null,
): ReviewComment {
  return { id: mintId(), path, line, body, source: { kind: "diff", side, oldPath } };
}

/** `ReviewComment::file` (comments.rs:63-71). */
export function newFileComment(path: string, line: number, body: string): ReviewComment {
  return { id: mintId(), path, line, body, source: { kind: "file" } };
}

/** `diff_anchor` (comments.rs:85-90) — `null` for file-sourced comments. */
export function diffAnchor(comment: ReviewComment): { side: CommentSide; line: number } | null {
  return comment.source.kind === "diff" ? { side: comment.source.side, line: comment.line } : null;
}

/** `is_file` (comments.rs:92-94). */
export function isFileComment(comment: ReviewComment): boolean {
  return comment.source.kind === "file";
}

/**
 * `cite_path` (comments.rs:106-114): the path the line number is valid in.
 * An Old-side line only exists in the pre-rename file, so citing `path`
 * there points the agent at a line of a file that never held it.
 */
export function citePath(comment: ReviewComment): string {
  if (comment.source.kind === "diff" && comment.source.side === "old" && comment.source.oldPath !== null) {
    return comment.source.oldPath;
  }
  return comment.path;
}

/** `location` (comments.rs:116-118) — `"{cite_path}:{line}"`. */
export function location(comment: ReviewComment): string {
  return `${citePath(comment)}:${comment.line}`;
}

/** `side` (comments.rs:96-101) — the `L`/`R` tag, `null` for file comments. */
export function commentSide(comment: ReviewComment): CommentSide | null {
  return comment.source.kind === "diff" ? comment.source.side : null;
}

/**
 * `with_comments` (comments.rs:131-157): the writer half of the codec. One
 * header for the whole set — `COMMENT_BLOCK_HEADER` when every comment is
 * diff-anchored, `REVIEW_COMMENT_BLOCK_HEADER` as soon as any is
 * file-sourced — then one bullet per comment, continuation lines indented
 * two spaces so they round-trip as `"  "`-prefixed lines, not new bullets.
 */
export function withComments(text: string, comments: readonly ReviewComment[]): string {
  if (comments.length === 0) {
    return text;
  }
  const bullets = comments.map((comment) => {
    const body = comment.body.trim().replace(/\n/g, "\n  ");
    const side = commentSide(comment);
    const separator = side === null ? ": " : ` (${side === "old" ? "L" : "R"}): `;
    return `- ${location(comment)}${separator}${body}`;
  });
  const body = text.trim().length === 0 ? COMMENT_ONLY_TEXT : text;
  const header = comments.every((comment) => !isFileComment(comment))
    ? COMMENT_BLOCK_HEADER
    : REVIEW_COMMENT_BLOCK_HEADER;
  return `${body}\n\n${header}\n${bullets.join("\n")}`;
}

// ---------------------------------------------------------------------------
// Card geometry (comments.rs:257-277) — analytic, never measured: the
// changes pane sizes bodies by arithmetic so a card composes with the fold
// tween's height math exactly.
// ---------------------------------------------------------------------------

export const CARD_PAD_V = 20;
export const CARD_HEADER_HEIGHT = 22;
export const CARD_LINE_HEIGHT = 18;
export const CARD_GAP = 6;
const CARD_WRAP_COLUMNS = 64;
const CARD_MAX_LINES = 8;
/** `DRAFT_CARD_HEIGHT` (comments.rs:260) — fixed so the fold tween never fights it. */
export const DRAFT_CARD_HEIGHT = 116;
/** `COMMENT_ADDER_SIZE` (comments.rs:261; changes.rs:4545). */
export const COMMENT_ADDER_SIZE = 16;

/** Rust `str::lines` semantics: a trailing newline yields no final empty line. */
function bodyLines(body: string): string[] {
  if (body.length === 0) {
    return [];
  }
  const parts = body.split("\n");
  if (body.endsWith("\n")) {
    parts.pop();
  }
  return parts;
}

/** `card_body_lines` (comments.rs:268-273): wraps guessed at 64 columns, clamped 1..8. */
export function cardBodyLines(body: string): number {
  let total = 0;
  for (const line of bodyLines(body)) {
    total += Math.max(1, Math.ceil(line.length / CARD_WRAP_COLUMNS));
  }
  return Math.min(Math.max(total, 1), CARD_MAX_LINES);
}

/** `card_height` (comments.rs:275-277). */
export function cardHeight(body: string): number {
  return CARD_PAD_V + CARD_HEADER_HEIGHT + cardBodyLines(body) * CARD_LINE_HEIGHT + CARD_GAP;
}

/**
 * `comment_strip_height` (composer.rs:306-310): the composer chip strip's
 * arithmetic height — `0` when nothing is staged, else `STRIP_PAD_TOP +
 * BADGE_HEIGHT` (36px).
 */
export function commentStripHeight(count: number): number {
  return count === 0 ? 0 : 12 + 24;
}

// ---------------------------------------------------------------------------
// The editor-side overlay geometry (preview.rs:40-43, 3301-3326) — the
// Files preview's floating card anchors.
// ---------------------------------------------------------------------------

export const EDITOR_COMMENT_CARD_WIDTH = 320;
export const EDITOR_COMMENT_CARD_MARGIN = 8;
export const EDITOR_COMMENT_CARD_MIN_ANCHORED_WIDTH = 220;
export const EDITOR_COMMENT_DRAFT_HEIGHT = 92;

/**
 * `editor_comment_overlay_horizontal` (preview.rs:3311-3326): an anchored
 * column of up to 320px at `gutter_width - 8` when the viewport allows at
 * least 220px there; else the full viewport width minus 8px margins.
 */
export function editorCommentOverlayHorizontal(
  gutterWidth: number,
  viewportWidth: number,
): { readonly left: number; readonly width: number } {
  const anchoredLeft = Math.max(gutterWidth - EDITOR_COMMENT_CARD_MARGIN, EDITOR_COMMENT_CARD_MARGIN);
  const anchoredWidth = Math.min(Math.max(viewportWidth - anchoredLeft - EDITOR_COMMENT_CARD_MARGIN, 0), EDITOR_COMMENT_CARD_WIDTH);
  if (anchoredWidth >= EDITOR_COMMENT_CARD_MIN_ANCHORED_WIDTH) {
    return { left: anchoredLeft, width: anchoredWidth };
  }
  return {
    left: EDITOR_COMMENT_CARD_MARGIN,
    width: Math.max(viewportWidth - EDITOR_COMMENT_CARD_MARGIN * 2, 0),
  };
}

/**
 * `editor_comment_overlay_top` (preview.rs:3301-3309): the row's bottom
 * edge, clamped into the viewport. `null` when the row is not on screen.
 */
export function editorCommentOverlayTop(
  rowTop: number,
  lineHeight: number,
  cardHeightPx: number,
  viewportHeight: number,
): number | null {
  const preferred = rowTop + lineHeight;
  return Math.min(Math.max(preferred, 0), Math.max(viewportHeight - cardHeightPx, 0));
}
