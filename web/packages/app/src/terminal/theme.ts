import type { ITheme } from "@xterm/xterm";
import type { TerminalPalette } from "@zeron/theme";

/**
 * The xterm theme derived from the theme artifact's terminal roles: ANSI
 * slots 0–7 normal / 8–15 bright map onto xterm's named colors, background /
 * foreground / selection ride the palette roles.
 *
 * The cursor is its own role (`--rb-cursor` = `text` at 40%/55% alpha,
 * `theme.rs::Theme::cursor`): a translucent fill that keeps the glyph
 * underneath legible — `cursorAccent` repaints the glyph in the normal
 * foreground, so the block reads as an overlay, not an inversion. The
 * unfocused outline is xterm's `cursorInactiveStyle: "outline"`.
 *
 * The scrollbar thumb is xterm 6's DOM scrollbar slider, themed at
 * `text_faint.opacity(0.52)` (`panel.rs::render_scrollbar`); hover widens it
 * in CSS (the color itself does not change, matching the desktop).
 */

/** `#rrggbb` + an alpha channel → the `#rrggbbaa` xterm's parser accepts. */
export function hexWithAlpha(hex: string, alpha: number): string {
  const tight = hex.trim();
  const six = /^#[0-9a-fA-F]{6}$/.test(tight) ? tight.slice(1) : null;
  if (six === null) {
    return tight;
  }
  const byte = Math.round(Math.min(Math.max(alpha, 0), 1) * 255);
  return `#${six}${byte.toString(16).padStart(2, "0")}`;
}

export function xtermThemeFromPalette(
  palette: TerminalPalette,
  cursor: string,
  scrollbarThumb?: string,
): ITheme {
  const ansi = palette.ansi;
  const thumb = scrollbarThumb ?? hexWithAlpha(palette.foreground, 0.52);
  return {
    background: palette.background,
    foreground: palette.foreground,
    cursor,
    cursorAccent: palette.foreground,
    selectionBackground: palette.selection,
    scrollbarSliderBackground: thumb,
    scrollbarSliderHoverBackground: thumb,
    scrollbarSliderActiveBackground: thumb,
    black: ansi[0],
    red: ansi[1],
    green: ansi[2],
    yellow: ansi[3],
    blue: ansi[4],
    magenta: ansi[5],
    cyan: ansi[6],
    white: ansi[7],
    brightBlack: ansi[8],
    brightRed: ansi[9],
    brightGreen: ansi[10],
    brightYellow: ansi[11],
    brightBlue: ansi[12],
    brightMagenta: ansi[13],
    brightCyan: ansi[14],
    brightWhite: ansi[15],
  };
}

/**
 * Read the palette the installed theme variant exposes on `<html>` as
 * `--rb-term-*` custom properties (see `variantCssVars` in @zeron/theme).
 * Reading computed style (instead of the artifact) means appearance changes
 * from any source — including ticket 16's preferences — are picked up by
 * re-calling this and reassigning `terminal.options.theme`.
 */
export function currentTerminalTheme(root: HTMLElement = document.documentElement): ITheme | undefined {
  const style = getComputedStyle(root);
  const read = (name: string): string => style.getPropertyValue(name).trim();
  const background = read("--rb-term-bg");
  const foreground = read("--rb-term-fg");
  if (background.length === 0 || foreground.length === 0) {
    return undefined;
  }
  const ansi: string[] = [];
  for (let i = 0; i < 16; i++) {
    ansi.push(read(`--rb-term-ansi-${i}`));
  }
  // `--rb-cursor` is the theme's own translucent cursor role; fall back to
  // the foreground if a variant ever omits it.
  const cursor = read("--rb-cursor");
  return xtermThemeFromPalette(
    {
      background,
      foreground,
      selection: read("--rb-term-selection"),
      ansi,
    },
    cursor.length > 0 ? cursor : foreground,
    // The scrollbar thumb rides the UI theme's text-faint at 52% — matching
    // the desktop's `render_scrollbar`, which does NOT read the terminal
    // palette for it.
    read("--rb-text-faint"),
  );
}
