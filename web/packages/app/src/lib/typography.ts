/**
 * Font kinds and per-surface scaling — the web peer of the desktop's
 * `crates/ui/src/typography.rs` after upstream #374 split the terminal and
 * code/diff fonts into independent settings.
 *
 * One shared `codeFontSize` setting drives several surfaces that never
 * agreed on a size historically. Each scales off its own baseline so the
 * default setting reproduces the size that surface always had
 * (12.5 markdown code / 12.0 diff / 13.0 editor / 11.5 preview), and a
 * user-chosen size moves them all while keeping those proportions.
 */

import {
  CODE_FONT_SIZE_DEFAULT,
  FONT_SIZE_MAX,
  FONT_SIZE_MIN,
  TERMINAL_FONT_SIZE_DEFAULT,
} from "../state/ui-settings";

export { CODE_FONT_SIZE_DEFAULT, FONT_SIZE_MAX, FONT_SIZE_MIN, TERMINAL_FONT_SIZE_DEFAULT };

/** `clamp_font_size` — a stored or computed size stays inside [8, 32]. */
export function clampFontSize(size: number): number {
  if (!Number.isFinite(size)) {
    return CODE_FONT_SIZE_DEFAULT;
  }
  return Math.min(FONT_SIZE_MAX, Math.max(FONT_SIZE_MIN, size));
}

// Markdown code blocks (render.rs): the surface the shared default was taken
// from — they scale 1:1 and carry no ratio of their own.
export const CODE_BLOCK_TEXT_BASELINE = CODE_FONT_SIZE_DEFAULT;
export const CODE_BLOCK_LINE_HEIGHT_BASELINE = 18;

/** Rendered code-block text size (render.rs `theme.code_font_size`). */
export function codeBlockTextSize(codeFontSize: number): number {
  return clampFontSize(codeFontSize);
}

/** Rendered code-block line height (`CODE_LINE_HEIGHT_RATIO`). */
export function codeBlockLineHeight(codeFontSize: number): number {
  return codeBlockTextSize(codeFontSize) * (CODE_BLOCK_LINE_HEIGHT_BASELINE / CODE_BLOCK_TEXT_BASELINE);
}

// Diffs (changes.rs): 12px text on a 21px row at the default.
export const DIFF_TEXT_BASELINE = 12;
export const DIFF_LINE_BASELINE = 21;

/** `diff_text_size`. */
export function diffTextSize(codeFontSize: number): number {
  return clampFontSize(codeFontSize * (DIFF_TEXT_BASELINE / CODE_FONT_SIZE_DEFAULT));
}

/** `diff_line_height` — the row box and the painted line box agree. */
export function diffLineHeight(codeFontSize: number): number {
  return diffTextSize(codeFontSize) * (DIFF_LINE_BASELINE / DIFF_TEXT_BASELINE);
}

// Workspace files (preview.rs): the editable editor's baseline is 13, the
// plain preview's is 11.5 on a 20px row floor.
export const EDITOR_TEXT_BASELINE = 13;
export const PREVIEW_TEXT_BASELINE = 11.5;
export const PREVIEW_LINE_HEIGHT_BASELINE = 20;

/** `editor_text_size`. */
export function editorTextSize(codeFontSize: number): number {
  return clampFontSize(codeFontSize * (EDITOR_TEXT_BASELINE / CODE_FONT_SIZE_DEFAULT));
}

/** `preview_text_size`. */
export function previewTextSize(codeFontSize: number): number {
  return clampFontSize(codeFontSize * (PREVIEW_TEXT_BASELINE / CODE_FONT_SIZE_DEFAULT));
}

/** `PreviewSurface::line_height` — scaled, never below the 20px floor. */
export function previewLineHeight(codeFontSize: number): number {
  return Math.max(
    previewTextSize(codeFontSize) * (PREVIEW_LINE_HEIGHT_BASELINE / PREVIEW_TEXT_BASELINE),
    PREVIEW_LINE_HEIGHT_BASELINE,
  );
}

// Terminal (view.rs): the row height rides the live size at the 18/13 ratio
// the hardcoded 13px/18px pairing encoded.
export const TERMINAL_TEXT_BASELINE = TERMINAL_FONT_SIZE_DEFAULT;
export const TERMINAL_LINE_HEIGHT_RATIO = 18 / 13;

/** The XTerm `lineHeight` multiplier for a terminal font size. */
export function terminalLineHeight(terminalFontSize: number): number {
  return TERMINAL_LINE_HEIGHT_RATIO;
}
