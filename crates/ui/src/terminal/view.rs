//! Terminal paint + input encoding.
//!
//! - theme-owned background/ANSI16 plus the xterm 256-color cube/grayscale;
//! - keystroke routing: typed text through the platform text input, other
//!   keys through Ghostty's encoder ([`super::keys`]);
//! - the ordered input queue and the 80 ms resize debounce constant (the
//!   panel drives the writes and timers; the buffer logic here is pure);
//! - [`TerminalElement`] — a custom gpui element that measures cell metrics
//!   from the real mono font (the "font probe"), reports the resulting
//!   cols×rows back to the panel, and paints the grid: background quads for
//!   non-default cells, shaped segments whose glyphs sit on their cells'
//!   columns (ligatures on, see [`shape_row`]), Ghostty's sprite glyphs for
//!   box drawing, blocks and Powerline, and the cursor.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{
    App, Bounds, Entity, GlobalElementId, Hsla, LayoutId, Modifiers, PaintQuad, Pixels,
    RenderImage, ShapedLine, SharedString, Style, TextRun, Window, fill, font, outline, point, px,
    relative, size,
};

use crate::theme::{Appearance, Theme, rgb_to_hsl};

use super::emulator::{CellColor, CellSnapshot, CursorShape, Emulator, Side};
use super::keys;
use super::panel::TerminalPanel;
use super::sprite;

/// Default terminal font metrics; the rendered values come from the theme.
pub const TERM_FONT_SIZE: f32 = 13.0;
pub const TERM_LINE_HEIGHT: f32 = 18.0;
/// Line height as a multiple of the font size, so a user-chosen size keeps the
/// default's row rhythm.
const TERM_LINE_HEIGHT_RATIO: f32 = TERM_LINE_HEIGHT / TERM_FONT_SIZE;
// ponytail: a fixed list of common Nerd Font families; a setting when someone uses another.
const NERD_FALLBACKS: [&str; 3] = [
    "Symbols Nerd Font Mono",
    "IoskeleyMonoNL Nerd Font Mono",
    "FiraCode Nerd Font Mono",
];
/// Inner padding of the grid area.
pub const TERM_PADDING: f32 = 12.0;

/// Debounce for `ResizeTerminal` after viewport-driven size changes.
pub const RESIZE_DEBOUNCE_MS: u64 = 80;

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// The panel fill behind the grid. On glass the opaque tone thins to a
/// translucent wash so the blurred desktop reads through like the rest of the
/// chrome (same move as [`Theme::card_glass_bg`]); opaque platforms keep the
/// true tone. Explicit cell backgrounds (vim colorschemes etc.) still paint
/// their own opaque quads on top.
pub fn terminal_panel_bg(theme: &Theme) -> Hsla {
    if theme.is_glass() {
        theme.terminal.background.opacity(0.4)
    } else {
        theme.terminal.background
    }
}

fn rgb8(r: u8, g: u8, b: u8) -> Hsla {
    let (h, s, l) = rgb_to_hsl(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    gpui::hsla(h, s, l, 1.0)
}

/// xterm 256-color cube component levels.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Resolve the xterm extended-color range to RGB components.
///
/// Three ranges, treated differently on purpose:
///
/// - **0-15** are named slots and are resolved directly from `Theme::terminal`.
/// - **16-231** is the 6×6×6 cube: a program asking for index 196 is asking for
///   `#ff0000` by arithmetic, and remapping it would be inventing colors the
///   caller did not pick. Left alone in both appearances, same as iTerm/Apple
///   Terminal light themes.
/// - **232-255** is the grayscale ramp, which tools use for *de-emphasis*
///   rather than for a specific grey. Its dark→light direction only reads as
///   "dim" on a dark background, so light mode mirrors the ramp: index 232
///   stays the faintest and 255 the strongest in both. Without this the ramp is
///   the single biggest legibility hole on white, because its bright end —
///   where most "dim hint" text lands — is the end that vanishes.
fn extended_indexed_rgb(appearance: Appearance, index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => unreachable!("ANSI16 is resolved from the active theme"),
        16..=231 => {
            let n = index as usize - 16;
            (
                CUBE_LEVELS[n / 36],
                CUBE_LEVELS[(n / 6) % 6],
                CUBE_LEVELS[n % 6],
            )
        }
        232..=255 => {
            let step = index - 232;
            let step = match appearance {
                Appearance::Dark => step,
                Appearance::Light => 23 - step,
            };
            let v = 8 + 10 * step;
            (v, v, v)
        }
    }
}

/// Resolve a cell color to paint against the theme.
pub fn resolve_color(color: CellColor, theme: &Theme) -> Hsla {
    match color {
        CellColor::Foreground => theme.terminal.foreground,
        CellColor::Background => theme.terminal.background,
        CellColor::Indexed(ix @ 0..=15) => theme.terminal.ansi[ix as usize],
        CellColor::Indexed(ix) => {
            let (r, g, b) = extended_indexed_rgb(theme.appearance, ix);
            rgb8(r, g, b)
        }
        CellColor::Rgb(r, g, b) => rgb8(r, g, b),
    }
}

// ---------------------------------------------------------------------------
// Pointer → cell
// ---------------------------------------------------------------------------

/// Minimum pointer travel before a press turns into a selection.
///
/// Without it, the click that focuses the panel starts a one-cell selection if
/// the hand moves a pixel — and once anything copies on selection change, that
/// silently clobbers the clipboard. Matches the threshold zed uses for the same
/// reason (`SELECTION_DRAG_THRESHOLD`), which is gpui's own `div` drag
/// threshold.
pub const SELECTION_DRAG_THRESHOLD: f32 = 2.0;

/// Which cell a pointer landed on, and which edge of it a selection anchors to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellHit {
    pub row: usize,
    pub col: usize,
    /// Selections anchor to a cell *edge*, not a cell: pressing on the left
    /// half of a glyph includes it, the right half excludes it.
    pub side: Side,
}

/// Map a position *relative to the grid's top-left glyph* onto a cell.
///
/// Positions outside the grid clamp to the nearest cell rather than returning
/// `None`, because that is what a drag needs: the pointer routinely leaves the
/// panel mid-gesture, and the selection should extend to the edge it left
/// through instead of freezing at the last sample taken inside.
///
/// Overshoot also *forces the side*, which clamping alone does not give you.
/// Dragging past the bottom should take the last line whole, even when the
/// pointer drifted left of where it started — deriving the side from x there
/// would stop the selection mid-row. Same rule going up, mirrored. This is the
/// behaviour alacritty and zed's `grid_point_and_side` both implement.
pub fn cell_at(x: f32, y: f32, cell_w: f32, line_h: f32, cols: usize, rows: usize) -> CellHit {
    // Degenerate metrics (a zero-size grid, or a font probe that returned NaN)
    // would otherwise divide into garbage cell indices.
    let usable = |v: f32| v.is_finite() && v > 0.0;
    if cols == 0 || rows == 0 || !usable(cell_w) || !usable(line_h) {
        return CellHit {
            row: 0,
            col: 0,
            side: Side::Left,
        };
    }
    let x = if x.is_finite() { x } else { 0.0 };
    let y = if y.is_finite() { y } else { 0.0 };
    let last_col = cols - 1;
    let last_row = rows - 1;

    let raw_col = (x / cell_w).floor();
    let mut side = if x.max(0.0) % cell_w > cell_w / 2.0 {
        Side::Right
    } else {
        Side::Left
    };
    let col = if raw_col > last_col as f32 {
        side = Side::Right;
        last_col
    } else {
        raw_col.max(0.0) as usize
    };

    let raw_row = (y / line_h).floor();
    let row = if raw_row > last_row as f32 {
        side = Side::Right;
        last_row
    } else if raw_row < 0.0 {
        side = Side::Left;
        0
    } else {
        raw_row as usize
    };

    CellHit { row, col, side }
}

// ---------------------------------------------------------------------------
// Keyboard → bytes
// ---------------------------------------------------------------------------

/// How a keydown reaches the PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyRoute {
    /// Write these bytes now.
    Bytes(Vec<u8>),
    /// Typed text: let the platform's text input deliver it, so input
    /// methods, dead keys and keyboard layouts compose it first.
    Text,
    /// Not the terminal's (app shortcuts).
    NotOurs,
}

/// Route a keydown. Printable text goes through the platform text input;
/// named keys and modified keys are encoded by Ghostty's key encoder against
/// `emulator`'s modes (cursor keys, modifyOtherKeys, the kitty keyboard
/// protocol), after Ghostty's macOS line-editing keybinds.
///
/// Option follows the layout: where Option types an ASCII symbol (`@`, `|`,
/// `\` on many European layouts) that symbol is typed; where it would type
/// something else (`∫` for Option-B on US layouts) it acts as Alt/Meta, so
/// shell word motions and editor bindings work without a setting.
pub fn route_key(
    key: &str,
    key_char: Option<&str>,
    mods: &Modifiers,
    emulator: &Emulator,
) -> KeyRoute {
    if let Some(bytes) = keys::macos_keybind(key, mods) {
        return KeyRoute::Bytes(bytes);
    }
    if mods.platform {
        return KeyRoute::NotOurs;
    }
    let encode = |text: Option<&str>| {
        keys::encode(emulator.terminal(), key, text, mods)
            .map_or(KeyRoute::NotOurs, KeyRoute::Bytes)
    };
    let printable = key_char.filter(|c| !c.is_empty() && !c.chars().any(char::is_control));
    if mods.alt && !mods.control {
        if let Some(text) = printable
            && text.is_ascii()
            && !text.eq_ignore_ascii_case(key)
            && key.chars().count() == 1
        {
            return KeyRoute::Text;
        }
        // Meta: the key as typed without Option.
        let base = (key.chars().count() == 1).then(|| {
            if mods.shift {
                key.to_uppercase()
            } else {
                key.to_string()
            }
        });
        return encode(base.as_deref());
    }
    if !mods.control && !mods.function && printable.is_some() && !is_named_key(key) {
        return KeyRoute::Text;
    }
    encode(key_char)
}

fn is_named_key(key: &str) -> bool {
    matches!(
        key,
        "enter"
            | "backspace"
            | "tab"
            | "escape"
            | "space"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "insert"
            | "delete"
            | "pageup"
            | "pagedown"
    ) || (key.len() >= 2 && key.starts_with('f') && key[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Wrap pasted text for the PTY (bracketed-paste aware; strips the one control
/// sequence a paste could inject).
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let sanitized = text.replace("\x1b[201~", "");
    if bracketed {
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(sanitized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        sanitized.into_bytes()
    }
}

// ---------------------------------------------------------------------------
// Pointer and focus reports
// ---------------------------------------------------------------------------

/// Button codes for mouse reports.
pub const MOUSE_LEFT: u8 = 0;
pub const MOUSE_MIDDLE: u8 = 1;
pub const MOUSE_RIGHT: u8 = 2;
/// Legacy encoding's "some button released"; motion with nothing held.
pub const MOUSE_NONE: u8 = 3;
pub const MOUSE_WHEEL_UP: u8 = 64;
pub const MOUSE_WHEEL_DOWN: u8 = 65;
/// Added to a button code for motion reports.
pub const MOUSE_MOTION: u8 = 32;

/// Encode a pointer event for a program that asked for mouse reports.
/// `col`/`row` are 0-based viewport cells. SGR (1006) reports releases with
/// their button; the legacy encoding cannot name one and cannot address
/// cells past 223, so such events are dropped rather than sent garbled.
pub fn mouse_report(
    button: u8,
    col: usize,
    row: usize,
    release: bool,
    mods: &Modifiers,
    sgr: bool,
) -> Option<Vec<u8>> {
    let mut code = button;
    if mods.shift {
        code += 4;
    }
    if mods.alt {
        code += 8;
    }
    if mods.control {
        code += 16;
    }
    if sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    if release {
        code = (code & !0b11) | MOUSE_NONE;
    }
    let encode = |v: usize| u8::try_from(v + 1 + 32).ok().filter(|b| *b < 255);
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + code,
        encode(col)?,
        encode(row)?,
    ])
}

/// Focus change report (1004).
pub fn focus_report(focused: bool) -> &'static [u8] {
    if focused { b"\x1b[I" } else { b"\x1b[O" }
}

/// Wheel rows as arrow keys for a full-screen program without mouse
/// reporting (alternate scroll, 1007). Positive rows scroll up.
pub fn wheel_arrows(rows: i32, app_cursor: bool) -> Vec<u8> {
    let arrow: &[u8] = match (rows > 0, app_cursor) {
        (true, true) => b"\x1bOA",
        (true, false) => b"\x1b[A",
        (false, true) => b"\x1bOB",
        (false, false) => b"\x1b[B",
    };
    arrow.repeat(rows.unsigned_abs() as usize)
}

// ---------------------------------------------------------------------------
// Contrast
// ---------------------------------------------------------------------------

/// Contrast floor for text the program colored itself (WCAG ratio). Themes
/// are tuned for their own default foreground; explicit colors are not
/// (blue on black, a 256-color grey on a dark panel), and below this they
/// stop being readable.
pub const MIN_CONTRAST: f32 = 3.0;

fn relative_luminance(color: Hsla) -> f32 {
    let rgb = color.to_rgb();
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(rgb.r) + 0.7152 * lin(rgb.g) + 0.0722 * lin(rgb.b)
}

pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// `fg`, moved in lightness just far enough from `bg` to reach `min`
/// contrast, keeping its hue. A foreground identical to its background is
/// deliberate (hidden blocks) and left alone.
pub fn ensure_contrast(fg: Hsla, bg: Hsla, min: f32) -> Hsla {
    if fg == bg || contrast_ratio(fg, bg) >= min {
        return fg;
    }
    let toward_light = relative_luminance(bg) < 0.18;
    let (mut lo, mut hi) = if toward_light {
        (fg.l, 1.0)
    } else {
        (0.0, fg.l)
    };
    let mut best = Hsla {
        l: if toward_light { 1.0 } else { 0.0 },
        ..fg
    };
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        let candidate = Hsla { l: mid, ..fg };
        if contrast_ratio(candidate, bg) >= min {
            best = candidate;
            if toward_light { hi = mid } else { lo = mid }
        } else if toward_light {
            lo = mid
        } else {
            hi = mid
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Input queue (pure buffer; the panel owns the write)
// ---------------------------------------------------------------------------

/// Keyboard bytes on their way to the PTY. A keystroke is written the moment
/// it lands; while that write is in flight, later keys gather here and ride
/// the next one. One write at a time keeps keys in typed order, and nothing
/// ever waits on a timer.
#[derive(Debug, Default)]
pub struct InputQueue {
    pending: Vec<u8>,
    in_flight: bool,
}

impl InputQueue {
    pub fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// The next write, when one may start now: nothing in flight and
    /// something pending. The caller must [`Self::finish`] it.
    pub fn start(&mut self) -> Option<Vec<u8>> {
        if self.in_flight || self.pending.is_empty() {
            return None;
        }
        self.in_flight = true;
        Some(std::mem::take(&mut self.pending))
    }

    /// The in-flight write completed (or failed); the next may start.
    pub fn finish(&mut self) {
        self.in_flight = false;
    }

    #[cfg(test)]
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }
}

// ---------------------------------------------------------------------------
// Grid element
// ---------------------------------------------------------------------------

/// Paints the active tab's grid. Cell metrics come from the resolved mono font
/// each frame (font probe): `em_advance` for the cell width, the fixed line
/// height for rows. The measured cols×rows feed back into the panel, which
/// resizes the emulator immediately and debounces the `ResizeTerminal` RPC.
pub struct TerminalElement {
    panel: Entity<TerminalPanel>,
    focused: bool,
}

impl TerminalElement {
    pub fn new(panel: Entity<TerminalPanel>, focused: bool) -> Self {
        Self { panel, focused }
    }
}

pub struct TerminalPrepaint {
    bg_quads: Vec<PaintQuad>,
    /// Selection wash. Painted after [`Self::bg_quads`] and before the glyphs:
    /// it has to tint a cell's own background rather than replace it, and it
    /// must not wash out the text it is highlighting.
    sel_quads: Vec<PaintQuad>,
    /// Per row, its top edge and the shaped segments with the grid COLUMN
    /// each one starts at. Not one line per row: see [`shape_row`].
    lines: Vec<(Pixels, Vec<(usize, ShapedLine)>)>,
    /// Per row, its top edge and the cells drawn by [`sprite`] instead of the
    /// font.
    glyphs: Vec<(Pixels, Vec<CustomGlyph>)>,
    /// Light line width for sprite glyphs, in logical pixels.
    stroke: f32,
    /// Grid cell advance, so paint can place segments by column.
    cell_w: Pixels,
    /// Grid row height, so paint places rows on the same baselines prepaint
    /// measured the grid with.
    line_h: Pixels,
    cursor: Option<PaintQuad>,
    /// The focused block cursor is opaque, so it goes under the text (whose
    /// cell flips to the background color); bars and outlines go over it.
    cursor_under_text: bool,
    /// In-progress input-method composition at the cursor: its backdrop and
    /// shaped text.
    marked: Option<(PaintQuad, gpui::Point<Pixels>, ShapedLine)>,
}

/// Selection corner rounding, capped for small fonts.
const SELECTION_RADIUS: f32 = 3.0;
/// Beam and underline cursor thickness.
const CURSOR_BAR: f32 = 2.0;
/// Extra selection opacity at the instant of a copy (the flash eases out).
const COPY_FLASH_ALPHA: f32 = 0.3;

impl gpui::IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = Theme::of(cx).clone();
        // Ligatures stay on: `shape_row` puts every glyph back on its cell's
        // column, so a ligature narrower than its cells no longer pulls the
        // rest of the row off the grid.
        let mut mono = font(theme.font_terminal.clone());
        // Nerd Font icons (eza, prompts) are missing from Geist Mono; borrow
        // them from installed Nerd Fonts.
        mono.fallbacks = Some(gpui::FontFallbacks::from_fonts(
            NERD_FALLBACKS.iter().map(|f| f.to_string()).collect(),
        ));
        // Font probe: measure the actual advance of the resolved mono font so
        // cols/rows track real glyph metrics, not a guessed aspect ratio.
        let font_size = px(theme.terminal_font_size);
        let font_id = window.text_system().resolve_font(&mono);
        let cell_w = window
            .text_system()
            .em_advance(font_id, font_size)
            .unwrap_or(px(theme.terminal_font_size * 0.6));
        let line_h = px(theme.terminal_font_size * TERM_LINE_HEIGHT_RATIO);

        let inner_w = f32::from(bounds.size.width) - 2.0 * TERM_PADDING;
        let inner_h = f32::from(bounds.size.height) - 2.0 * TERM_PADDING;
        let cols = ((inner_w / f32::from(cell_w)).floor() as i64).clamp(2, 500) as u16;
        let rows = ((inner_h / f32::from(line_h)).floor() as i64).clamp(1, 500) as u16;

        // Report the measured grid, then snapshot for painting. Safe: the
        // panel entity is not borrowed during element prepaint.
        let origin = point(
            bounds.left() + px(TERM_PADDING),
            bounds.top() + px(TERM_PADDING),
        );
        let snapshot = self.panel.update(cx, |panel, cx| {
            panel.on_grid_metrics(
                super::panel::GridGeometry {
                    bounds,
                    origin,
                    cell_w: f32::from(cell_w),
                    line_h: f32::from(line_h),
                    cols,
                    rows,
                },
                cx,
            );
            let snapshot = panel.active_grid_snapshot(cx);
            // Pointer mapping follows the rows where they are drawn.
            panel.shift_grid_origin(line_h * snapshot.as_ref().map_or(0.0, |s| s.scroll_frac));
            snapshot
        });
        let Some(snapshot) = snapshot else {
            return TerminalPrepaint {
                bg_quads: Vec::new(),
                sel_quads: Vec::new(),
                lines: Vec::new(),
                glyphs: Vec::new(),
                stroke: 1.0,
                cell_w,
                line_h,
                cursor: None,
                cursor_under_text: false,
                marked: None,
            };
        };

        let mut bg_quads = Vec::new();
        let mut sel_quads = Vec::new();
        let mut lines = Vec::with_capacity(snapshot.lines.len() + 1);
        // A partial scroll shifts every row down by the fraction and reveals
        // the history row above; the content mask clips both edges.
        let shift = line_h * snapshot.scroll_frac;
        let first_row: i32 = if snapshot.line_above.is_some() { -1 } else { 0 };
        let rows: Vec<&Vec<CellSnapshot>> = snapshot
            .line_above
            .iter()
            .chain(snapshot.lines.iter())
            .collect();
        let row_count = rows.len();
        let mut selection = theme.terminal.selection;
        selection.a = (selection.a + COPY_FLASH_ALPHA * snapshot.copy_flash).min(0.9);
        let radius = px(SELECTION_RADIUS.min(f32::from(line_h) / 4.0));
        let mut glyphs = Vec::with_capacity(rows.len());
        // Ghostty's block cursor: an opaque block under the text, whose cell
        // takes the background color.
        let block_cursor = snapshot
            .cursor
            .filter(|_| {
                snapshot.cursor_on && self.focused && snapshot.cursor_shape == CursorShape::Block
            })
            .map(|c| (c.row, c.col));
        let cursor_text = Hsla {
            a: 1.0,
            ..theme.terminal.background
        };

        for (row_ix, row) in rows.iter().copied().enumerate() {
            let y = origin.y + shift + line_h * (first_row + row_ix as i32) as f32;
            // Selected runs, merged the same way background runs are: one quad
            // per contiguous span instead of one per cell.
            let mut sel_start: Option<usize> = None;
            for col in 0..=row.len() {
                let selected = row.get(col).is_some_and(|cell| cell.selected);
                match (sel_start, selected) {
                    (None, true) => sel_start = Some(col),
                    (Some(start), false) => {
                        sel_quads.push(
                            fill(
                                Bounds::new(
                                    point(origin.x + cell_w * start as f32, y),
                                    size(cell_w * (col - start) as f32, line_h),
                                ),
                                selection,
                            )
                            .corner_radii(radius),
                        );
                        sel_start = None;
                    }
                    _ => {}
                }
            }
            // Merge consecutive non-default background cells into quads. A
            // run touching the grid's edge carries its color into the padding
            // beside it, so full-bleed program backgrounds (editors, status
            // lines) reach the panel edge instead of stopping a few px short.
            let mut run_start: Option<(usize, Hsla)> = None;
            let edge_quad = |start: usize, end: usize, color: Hsla| {
                let x0 = if start == 0 {
                    bounds.left()
                } else {
                    origin.x + cell_w * start as f32
                };
                let x1 = if end == row.len() {
                    bounds.right()
                } else {
                    origin.x + cell_w * end as f32
                };
                let y0 = if row_ix == 0 { bounds.top() } else { y };
                let y1 = if row_ix + 1 == row_count {
                    bounds.bottom()
                } else {
                    y + line_h
                };
                fill(Bounds::from_corners(point(x0, y0), point(x1, y1)), color)
            };
            for (col, color) in row
                .iter()
                .map(|cell| cell.display_colors().1)
                .chain(std::iter::once(CellColor::Background))
                .enumerate()
            {
                let paint = match color {
                    CellColor::Background => None,
                    other => Some(resolve_color(other, &theme)),
                };
                match (&run_start, paint) {
                    (None, Some(color)) => run_start = Some((col, color)),
                    (Some((start, current)), next) if next != Some(*current) => {
                        bg_quads.push(edge_quad(*start, col, *current));
                        run_start = next.map(|color| (col, color));
                    }
                    _ => {}
                }
            }
            let viewport_row = row_ix as i32 + first_row;
            let cursor_col = block_cursor
                .filter(|&(r, _)| r as i32 == viewport_row)
                .map(|(_, c)| (c, cursor_text));
            let mut row_glyphs = Vec::new();
            lines.push((
                y,
                shape_row(
                    row,
                    &theme,
                    &mono,
                    font_size,
                    cell_w,
                    cursor_col,
                    &mut row_glyphs,
                    window,
                ),
            ));
            glyphs.push((y, row_glyphs));
        }

        let cursor = snapshot.cursor.filter(|_| snapshot.cursor_on).map(|c| {
            let cell = Bounds::new(
                point(
                    origin.x + cell_w * c.col as f32,
                    origin.y + shift + line_h * c.row as f32,
                ),
                size(cell_w, line_h),
            );
            let bar = px(CURSOR_BAR);
            let color = cursor_color(&theme);
            match (self.focused, snapshot.cursor_shape) {
                (false, _) | (true, CursorShape::HollowBlock) => {
                    outline(cell, color, gpui::BorderStyle::Solid)
                }
                (true, CursorShape::Block) => fill(cell, color),
                (true, CursorShape::Beam) => {
                    fill(Bounds::new(cell.origin, size(bar, line_h)), color)
                }
                (true, CursorShape::Underline) => fill(
                    Bounds::new(point(cell.left(), cell.bottom() - bar), size(cell_w, bar)),
                    color,
                ),
            }
        });

        // Composition text sits over the cursor cell on an opaque backdrop,
        // underlined like every platform marks unconfirmed input.
        let marked = snapshot
            .marked_text
            .as_deref()
            .filter(|text| !text.is_empty())
            .zip(snapshot.cursor)
            .map(|(text, c)| {
                let at = point(
                    origin.x + cell_w * c.col as f32,
                    origin.y + shift + line_h * c.row as f32,
                );
                let shaped = window.text_system().shape_line(
                    SharedString::from(text.to_string()),
                    font_size,
                    &[TextRun {
                        len: text.len(),
                        font: mono.clone(),
                        color: theme.terminal.foreground,
                        background_color: None,
                        underline: Some(gpui::UnderlineStyle {
                            color: Some(theme.terminal.foreground),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        strikethrough: None,
                    }],
                    None,
                );
                let backdrop = fill(
                    Bounds::new(at, size(shaped.width.max(cell_w), line_h)),
                    theme.terminal.background.opacity(1.0),
                );
                (backdrop, at, shaped)
            });

        TerminalPrepaint {
            bg_quads,
            sel_quads,
            lines,
            glyphs,
            // The font's underline thickness is ~0.05em; Ghostty strokes box
            // lines with it. ponytail: gpui doesn't expose the real metric, so
            // approximate it.
            stroke: theme.terminal_font_size * 0.05,
            cell_w,
            line_h,
            cursor,
            cursor_under_text: block_cursor.is_some(),
            marked,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_h = prepaint.line_h;
        let origin = point(
            bounds.left() + px(TERM_PADDING),
            bounds.top() + px(TERM_PADDING),
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.bg_quads.drain(..) {
                window.paint_quad(quad);
            }
            for quad in prepaint.sel_quads.drain(..) {
                window.paint_quad(quad);
            }
            if prepaint.cursor_under_text
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
            let cell_w = prepaint.cell_w;
            for (y, segments) in &prepaint.lines {
                let y = *y;
                for (col, line) in segments {
                    let _ = line.paint(
                        point(origin.x + cell_w * *col as f32, y),
                        line_h,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            for (y, row) in &prepaint.glyphs {
                for glyph in row {
                    let left = origin.x + cell_w * glyph.col as f32;
                    let bounds = Bounds::from_corners(
                        point(left, *y),
                        point(left + cell_w * glyph.cells as f32, *y + line_h),
                    );
                    paint_custom_glyph(glyph, bounds, prepaint.stroke, window, cx);
                }
            }
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
            if let Some((backdrop, at, text)) = prepaint.marked.take() {
                window.paint_quad(backdrop);
                let _ = text.paint(at, line_h, gpui::TextAlign::Left, None, window, cx);
            }
        });
        if self.panel.read(cx).copy_flash_running() {
            window.request_animation_frame();
        }
        // Typed text (and input-method composition) enters here.
        if self.focused {
            let focus = self.panel.read(cx).focus_handle();
            window.handle_input(
                &focus,
                gpui::ElementInputHandler::new(bounds, self.panel.clone()),
                cx,
            );
        }
    }
}

/// The cursor block, opaque as Ghostty draws it: the theme's accent.
fn cursor_color(theme: &Theme) -> Hsla {
    Hsla {
        a: 1.0,
        ..theme.accent
    }
}

/// Shape one grid row into segments placed by column.
///
/// A terminal is a fixed grid, but a shaped line places glyphs by their font
/// advances, and those drift off it: a glyph from FONT FALLBACK (arrows `→`,
/// emoji, CJK) advances by whatever its own font says, and a ligature (Geist
/// Mono's `--`, `->`) covers several cells with one narrower glyph. Laid out
/// as shaped, the rest of the row slides (user reports: "one of the pipes is
/// broken", `codex --yolo` rendering as `codex--yolo`), while backgrounds,
/// selection and the cursor stay put as quads at `cell_w * col`.
///
/// So, as Ghostty, kitty and WezTerm do, every glyph moves to the column of
/// the cell it came from (its cluster): a glyph narrower than its cells leaves
/// a gap, and nothing after it slides. Own segments remain only for sprite
/// glyphs (drawn by paint), right-to-left runs (CoreText reorders and joins
/// their glyphs, so they cannot be placed letter by letter) and the cursor
/// cell, which splits a ligature under the cursor as Ghostty does.
#[allow(clippy::too_many_arguments)]
fn shape_row(
    row: &[CellSnapshot],
    theme: &Theme,
    mono: &gpui::Font,
    font_size: Pixels,
    cell_w: Pixels,
    cursor: Option<(usize, Hsla)>,
    glyphs: &mut Vec<CustomGlyph>,
    window: &Window,
) -> Vec<(usize, ShapedLine)> {
    let mut segments: Vec<(usize, ShapedLine)> = Vec::new();
    let mut seg = Segment::default();
    let mut rtl_run = false;

    for (col, cell) in row.iter().enumerate() {
        if cell.wide_spacer {
            continue;
        }
        let ch = if cell.hidden { ' ' } else { cell.ch };
        let (fg, bg) = cell.display_colors();
        let mut color = resolve_color(fg, theme);
        // The theme's own default pair is already tuned; anything the
        // program colored gets the contrast floor.
        if !(fg == CellColor::Foreground && bg == CellColor::Background) {
            color = ensure_contrast(color, resolve_color(bg, theme), MIN_CONTRAST);
        }
        if cell.dim {
            color.a *= 0.6;
        }
        let at_cursor = cursor.is_some_and(|(c, _)| c == col);
        if let Some((_, cursor_text)) = cursor.filter(|_| at_cursor) {
            color = cursor_text;
        }
        // An RTL run ending here fits into the cells it spans.
        let rtl_fit = rtl_run.then(|| cell_w * (col - seg.col) as f32);
        if !ch.is_ascii() && sprite::has_char(ch) {
            // Drawn to the cell by paint, not shaped.
            seg.flush(rtl_fit, &mut segments, font_size, cell_w, window);
            rtl_run = false;
            glyphs.push(CustomGlyph {
                col,
                ch,
                cells: if cell.wide { 2 } else { 1 },
                color,
            });
            continue;
        }
        // Arabic/Hebrew letters (and the spaces between them) shape as ONE
        // segment: alone, each letter gets its isolated form and the words
        // read left to right; together CoreText joins and reorders them.
        let rtl = is_rtl(ch) && !cell.wide;
        let joins = rtl_run && (rtl || ch == ' ');
        if !joins && (at_cursor || rtl || rtl_run) {
            seg.flush(rtl_fit, &mut segments, font_size, cell_w, window);
        }
        if seg.text.is_empty() {
            seg.col = col;
        }
        let weight = if cell.bold {
            gpui::FontWeight::BOLD
        } else {
            gpui::FontWeight::NORMAL
        };
        let style = if cell.italic {
            gpui::FontStyle::Italic
        } else {
            gpui::FontStyle::Normal
        };
        // A predicted (not yet echoed) character carries a faint underline:
        // it reads as typed, while still owning that it is provisional.
        let underline = if cell.predicted {
            Some(gpui::UnderlineStyle {
                color: Some(color.opacity(0.45)),
                thickness: px(1.0),
                wavy: false,
            })
        } else {
            cell.underline.then_some(gpui::UnderlineStyle {
                color: Some(
                    cell.underline_color
                        .filter(|_| !at_cursor)
                        .map_or(color, |c| resolve_color(c, theme)),
                ),
                thickness: px(1.0),
                wavy: cell.undercurl,
            })
        };
        let strikethrough = cell.strikethrough.then_some(gpui::StrikethroughStyle {
            color: Some(color),
            thickness: px(1.0),
        });
        // A cell's whole grapheme cluster (a flag, a skin tone, a letter and
        // its marks) shapes together, on its column.
        let mut buf = [0; 4];
        let text = cell
            .cluster
            .as_deref()
            .filter(|_| !cell.hidden)
            .unwrap_or_else(|| ch.encode_utf8(&mut buf));
        let len = text.len();
        seg.text.push_str(text);
        seg.cols.extend(std::iter::repeat_n(col, len));
        // Every run is `mono` but for weight and style: comparing those spares
        // a font clone and compare per cell.
        match seg.runs.last_mut() {
            Some(last)
                if last.color == color
                    && last.font.weight == weight
                    && last.font.style == style
                    && last.underline == underline
                    && last.strikethrough == strikethrough =>
            {
                last.len += len;
            }
            _ => seg.runs.push(TextRun {
                len,
                font: gpui::Font {
                    weight,
                    style,
                    ..mono.clone()
                },
                color,
                background_color: None,
                underline,
                strikethrough,
            }),
        }
        rtl_run = rtl || joins;
        if at_cursor && !rtl_run {
            seg.flush(None, &mut segments, font_size, cell_w, window);
        }
    }
    let rtl_fit = rtl_run.then(|| cell_w * (row.len() - seg.col) as f32);
    seg.flush(rtl_fit, &mut segments, font_size, cell_w, window);
    segments
}

/// The row text being gathered into one shaped segment, with the grid column
/// of each of its bytes.
#[derive(Default)]
struct Segment {
    text: String,
    cols: Vec<usize>,
    runs: Vec<TextRun>,
    col: usize,
}

impl Segment {
    /// Shape the gathered text into `out`. `rtl_fit` marks a right-to-left
    /// run and the width of its cells.
    fn flush(
        &mut self,
        rtl_fit: Option<Pixels>,
        out: &mut Vec<(usize, ShapedLine)>,
        font_size: Pixels,
        cell_w: Pixels,
        window: &Window,
    ) {
        if self.text.is_empty() {
            return;
        }
        let text = SharedString::from(std::mem::take(&mut self.text));
        let system = window.text_system();
        let mut shaped = system.shape_line(text.clone(), font_size, &self.runs, None);
        match rtl_fit {
            // A joined RTL run wider than its cells shrinks to fit them.
            // Spacing glyphs apart instead would break the joins between
            // letters.
            // ponytail: narrower runs stay as they are; Arabic stretches with
            // kashida (U+0640), not by spacing.
            Some(fit) if shaped.width > fit => {
                shaped =
                    system.shape_line(text, font_size * (fit / shaped.width), &self.runs, None);
            }
            Some(_) => {}
            None => snap_to_cells(&mut shaped, &self.cols, self.col, cell_w),
        }
        out.push((self.col, shaped));
        self.runs.clear();
        self.cols.clear();
    }
}

/// Move every glyph of `line` to the column its cluster starts at. Glyphs of
/// one cluster (a letter and its combining marks) keep their offsets from the
/// cluster's first glyph.
fn snap_to_cells(line: &mut ShapedLine, cols: &[usize], seg_col: usize, cell_w: Pixels) {
    let mut runs = line.runs.clone();
    let mut cluster = (usize::MAX, px(0.));
    for glyph in runs.iter_mut().flat_map(|run| run.glyphs.iter_mut()) {
        // A combining mark is its own glyph at its own index, but on its
        // letter's column.
        let col = cols.get(glyph.index).copied().unwrap_or(seg_col);
        if col != cluster.0 {
            cluster = (col, glyph.position.x);
        }
        glyph.position.x = cell_w * (col - seg_col) as f32 + (glyph.position.x - cluster.1);
    }
    **line = Arc::new(gpui::LineLayout {
        font_size: line.font_size,
        width: line.width,
        ascent: line.ascent,
        descent: line.descent,
        runs,
        len: line.len,
    });
}

/// Hebrew and Arabic blocks (letters, marks, presentation forms).
fn is_rtl(ch: char) -> bool {
    matches!(ch, '\u{0590}'..='\u{08FF}' | '\u{FB1D}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFC}')
}

struct CustomGlyph {
    col: usize,
    ch: char,
    cells: usize,
    color: Hsla,
}

/// Drawn sprite glyphs, keyed by char, device-pixel cell size, line thickness
/// and color (images are full color, so the color is baked in).
#[derive(Default)]
struct CustomGlyphCache(HashMap<(char, u32, u32, u32, [u8; 4]), (Arc<RenderImage>, u32, u32)>);

impl gpui::Global for CustomGlyphCache {}

/// Paint one sprite glyph drawn at the exact device-pixel size of its cell, so
/// neighbouring cells tile without seams or resampling. The sprite is padded
/// on every side (a glyph may spill past its cell), so it is placed up-left of
/// the cell by the padding.
fn paint_custom_glyph(
    glyph: &CustomGlyph,
    bounds: Bounds<Pixels>,
    stroke: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let device = |v: Pixels| (f32::from(v) * scale).round();
    let (left, top) = (device(bounds.left()), device(bounds.top()));
    let w = (device(bounds.right()) - left) as u32;
    let h = (device(bounds.bottom()) - top) as u32;
    let thickness = (stroke * scale).ceil().max(1.0) as u32;
    let rgba = gpui::Rgba::from(glyph.color);
    let color = [rgba.r, rgba.g, rgba.b, rgba.a].map(|c| (c * 255.0).round() as u8);
    let key = (glyph.ch, w, h, thickness, color);

    let cache = cx.default_global::<CustomGlyphCache>();
    let (image, pad_x, pad_y) = match cache.0.get(&key) {
        Some(hit) => hit.clone(),
        None => {
            let metrics = sprite::Metrics {
                cell_width: w / glyph.cells as u32,
                cell_height: h,
                box_thickness: thickness,
            };
            let Some(drawn) = sprite::render(glyph.ch, w, h, &metrics) else {
                return;
            };
            // Coverage → straight-alpha BGRA in the glyph color.
            let [r, g, b, a] = color;
            let bgra: Vec<u8> = drawn
                .alpha
                .iter()
                .flat_map(|&cov| [b, g, r, (cov as u16 * a as u16 / 255) as u8])
                .collect();
            let Some(buffer) = image::RgbaImage::from_raw(drawn.width, drawn.height, bgra) else {
                return;
            };
            let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
            // ponytail: drop everything past a cap (old sizes after a zoom,
            // truecolor gradients); an LRU if that ever thrashes. Images live
            // in this window's atlas, so other windows may keep stale copies.
            if cache.0.len() >= 4096 {
                for (_, (old, _, _)) in cache.0.drain() {
                    let _ = window.drop_image(old);
                }
            }
            let hit = (image, drawn.padding_x, drawn.padding_y);
            cache.0.insert(key, hit.clone());
            hit
        }
    };
    let size = image.size(0);
    let logical = |v: f32| px(v / scale);
    let bounds = Bounds::new(
        point(logical(left - pad_x as f32), logical(top - pad_y as f32)),
        gpui::size(logical(size.width.0 as f32), logical(size.height.0 as f32)),
    );
    let _ = window.paint_image(bounds, Default::default(), image, 0, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> Modifiers {
        Modifiers::default()
    }

    fn emu(setup: &[u8]) -> Emulator {
        let mut e = Emulator::new(20, 4);
        e.feed(setup);
        e
    }

    #[test]
    fn printable_keys_go_through_text_input_and_named_keys_are_encoded() {
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        let e = emu(b"");
        assert_eq!(route_key("a", Some("a"), &mods(), &e), KeyRoute::Text);
        assert_eq!(route_key("a", Some("A"), &shift, &e), KeyRoute::Text);
        assert_eq!(route_key("ж", Some("ж"), &mods(), &e), KeyRoute::Text);
        assert_eq!(
            route_key("enter", Some("\r"), &mods(), &e),
            KeyRoute::Bytes(b"\r".to_vec())
        );
        assert_eq!(
            route_key("up", None, &mods(), &emu(b"\x1b[?1h")),
            KeyRoute::Bytes(b"\x1bOA".to_vec())
        );
        assert_eq!(
            route_key(
                "c",
                Some("c"),
                &Modifiers {
                    control: true,
                    ..mods()
                },
                &e
            ),
            KeyRoute::Bytes(vec![0x03])
        );
        assert_eq!(
            route_key(
                "j",
                Some("j"),
                &Modifiers {
                    platform: true,
                    ..mods()
                },
                &e
            ),
            KeyRoute::NotOurs
        );
    }

    #[test]
    fn option_types_layout_symbols_and_otherwise_acts_as_meta() {
        let alt = Modifiers {
            alt: true,
            ..mods()
        };
        let e = emu(b"");
        // US layout: Option-B would type `∫` — Meta-b for word motion.
        assert_eq!(
            route_key("b", Some("∫"), &alt, &e),
            KeyRoute::Bytes(vec![0x1b, b'b'])
        );
        assert_eq!(
            route_key("b", Some("ı"), &Modifiers { shift: true, ..alt }, &e),
            KeyRoute::Bytes(vec![0x1b, b'B'])
        );
        // German layout: Option-L types `@`, which is what the user meant.
        assert_eq!(route_key("l", Some("@"), &alt, &e), KeyRoute::Text);
        // Named keys carry Alt as a modifier, as Ghostty sends them; on macOS
        // Option+Left/Right are a word back and forward (Ghostty's keybinds).
        assert_eq!(
            route_key("up", None, &alt, &e),
            KeyRoute::Bytes(b"\x1b[1;3A".to_vec())
        );
        let left = if cfg!(target_os = "macos") {
            b"\x1bb".to_vec()
        } else {
            b"\x1b[1;3D".to_vec()
        };
        assert_eq!(route_key("left", None, &alt, &e), KeyRoute::Bytes(left));
    }

    /// The program's keyboard protocol reaches the encoder: Claude Code's
    /// Shift+Enter is a newline once it pushes kitty's flags.
    #[test]
    fn keys_follow_the_programs_keyboard_protocol() {
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        assert_eq!(
            route_key("enter", Some("\r"), &shift, &emu(b"\x1b[>1u")),
            KeyRoute::Bytes(b"\x1b[13;2u".to_vec())
        );
    }

    #[test]
    fn mouse_reports_in_both_encodings() {
        let none = mods();
        assert_eq!(
            mouse_report(MOUSE_LEFT, 4, 1, false, &none, true).unwrap(),
            b"\x1b[<0;5;2M"
        );
        assert_eq!(
            mouse_report(MOUSE_LEFT, 4, 1, true, &none, true).unwrap(),
            b"\x1b[<0;5;2m",
            "SGR names the released button"
        );
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert_eq!(
            mouse_report(MOUSE_WHEEL_UP, 0, 0, false, &ctrl, true).unwrap(),
            b"\x1b[<80;1;1M"
        );
        assert_eq!(
            mouse_report(MOUSE_RIGHT, 0, 0, false, &none, false).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 2, 33, 33]
        );
        assert_eq!(
            mouse_report(MOUSE_RIGHT, 0, 0, true, &none, false).unwrap(),
            vec![0x1b, b'[', b'M', 32 + 3, 33, 33],
            "legacy release is button 3"
        );
        assert_eq!(
            mouse_report(MOUSE_LEFT, 300, 0, false, &none, false),
            None,
            "legacy encoding cannot address column 301"
        );
    }

    #[test]
    fn focus_and_wheel_arrow_encodings() {
        assert_eq!(focus_report(true), b"\x1b[I");
        assert_eq!(focus_report(false), b"\x1b[O");
        assert_eq!(wheel_arrows(2, false), b"\x1b[A\x1b[A");
        assert_eq!(wheel_arrows(-1, true), b"\x1bOB");
    }

    #[test]
    fn low_contrast_program_colors_are_lifted_and_good_ones_kept() {
        let black = gpui::hsla(0.0, 0.0, 0.0, 1.0);
        let navy = rgb8(0, 0, 128);
        assert!(contrast_ratio(navy, black) < MIN_CONTRAST);
        let lifted = ensure_contrast(navy, black, MIN_CONTRAST);
        assert!(contrast_ratio(lifted, black) >= MIN_CONTRAST);
        assert!((lifted.h - navy.h).abs() < 1e-3, "hue is kept");
        let white = gpui::hsla(0.0, 0.0, 1.0, 1.0);
        assert_eq!(ensure_contrast(white, black, MIN_CONTRAST), white);
        // Same color on itself is a deliberate block, not a contrast bug.
        assert_eq!(ensure_contrast(navy, navy, MIN_CONTRAST), navy);
        // Light backgrounds darken the text instead.
        let pale = rgb8(250, 250, 160);
        let darkened = ensure_contrast(pale, white, MIN_CONTRAST);
        assert!(darkened.l < pale.l && contrast_ratio(darkened, white) >= MIN_CONTRAST);
    }

    #[test]
    fn paste_wraps_when_bracketed() {
        assert_eq!(paste_bytes("hi", false), b"hi".to_vec());
        assert_eq!(paste_bytes("hi", true), b"\x1b[200~hi\x1b[201~".to_vec());
        // Close-bracket injection is stripped.
        assert_eq!(
            paste_bytes("a\x1b[201~rm -rf", true),
            b"\x1b[200~arm -rf\x1b[201~".to_vec()
        );
    }

    #[test]
    fn input_queue_keeps_one_write_in_flight_and_order() {
        let mut q = InputQueue::default();
        assert_eq!(q.start(), None, "nothing pending");
        q.push(b"a");
        assert_eq!(q.start(), Some(b"a".to_vec()), "first key goes at once");
        q.push(b"b");
        q.push(b"c");
        assert_eq!(q.start(), None, "keys typed mid-write wait for it");
        q.finish();
        assert_eq!(q.start(), Some(b"bc".to_vec()), "then ride one write");
        q.finish();
        assert_eq!(q.start(), None);
    }

    #[test]
    fn cube_is_appearance_independent() {
        for appearance in [Appearance::Dark, Appearance::Light] {
            // 16 = cube origin (0,0,0); 231 = cube max (255,255,255).
            assert_eq!(extended_indexed_rgb(appearance, 16), (0, 0, 0));
            assert_eq!(extended_indexed_rgb(appearance, 231), (255, 255, 255));
            // 196 = pure red corner: 16 + 36*5.
            assert_eq!(extended_indexed_rgb(appearance, 196), (255, 0, 0));
            // 21 = pure blue corner.
            assert_eq!(extended_indexed_rgb(appearance, 21), (0, 0, 255));
        }
    }

    #[test]
    fn grayscale_ramp_mirrors_in_light() {
        // Dark: 232 → 8 (faintest), 255 → 238 (strongest).
        assert_eq!(extended_indexed_rgb(Appearance::Dark, 232), (8, 8, 8));
        assert_eq!(extended_indexed_rgb(Appearance::Dark, 255), (238, 238, 238));
        // Light reverses it so the faint end stays faint against white.
        assert_eq!(
            extended_indexed_rgb(Appearance::Light, 232),
            (238, 238, 238)
        );
        assert_eq!(extended_indexed_rgb(Appearance::Light, 255), (8, 8, 8));
    }

    #[test]
    fn registered_theme_owns_terminal_background_foreground_selection_and_ansi() {
        let theme = Theme::for_selection(
            Appearance::Dark,
            "dracula",
            zeron_theme::AccentSelection::ThemeDefault,
            zeron_theme::SurfacePreference::ThemeDefault,
        );
        assert_eq!(terminal_panel_bg(&theme), theme.terminal.background);
        assert_eq!(
            resolve_color(CellColor::Foreground, &theme),
            theme.terminal.foreground
        );
        assert_eq!(
            resolve_color(CellColor::Background, &theme),
            theme.terminal.background
        );
        assert_eq!(
            resolve_color(CellColor::Indexed(1), &theme),
            theme.terminal.ansi[1]
        );
        assert!(theme.terminal.selection.a > 0.0 && theme.terminal.selection.a < 0.5);
    }

    #[test]
    fn every_registered_variant_resolves_all_ansi_slots_from_its_theme() {
        for variant in zeron_theme::ThemeRegistry::builtin()
            .families
            .iter()
            .flat_map(|family| &family.variants)
        {
            let appearance = match variant.appearance {
                zeron_theme::Appearance::Dark => Appearance::Dark,
                zeron_theme::Appearance::Light => Appearance::Light,
            };
            let theme = Theme::for_selection(
                appearance,
                &variant.id,
                zeron_theme::AccentSelection::ThemeDefault,
                zeron_theme::SurfacePreference::ThemeDefault,
            );
            for index in 0..16 {
                assert_eq!(
                    resolve_color(CellColor::Indexed(index), &theme),
                    theme.terminal.ansi[index as usize],
                    "{} ANSI {index}",
                    variant.id
                );
            }
        }
    }

    #[test]
    fn timing_constants_match_spec() {
        assert_eq!(RESIZE_DEBOUNCE_MS, 80);
    }

    // ---- pointer → cell ----

    /// 10x20 cells, an 8x4 grid: cols 0..7, rows 0..3.
    fn hit(x: f32, y: f32) -> CellHit {
        cell_at(x, y, 10.0, 20.0, 8, 4)
    }

    #[test]
    fn pointer_maps_to_the_cell_it_is_over() {
        assert_eq!(
            hit(0.0, 0.0),
            CellHit {
                row: 0,
                col: 0,
                side: Side::Left
            }
        );
        assert_eq!(
            hit(25.0, 45.0),
            CellHit {
                row: 2,
                col: 2,
                side: Side::Left
            }
        );
        // Last cell, exactly.
        assert_eq!(
            hit(70.0, 60.0),
            CellHit {
                row: 3,
                col: 7,
                side: Side::Left
            }
        );
    }

    #[test]
    fn side_splits_the_cell_at_its_midpoint() {
        // Cell 2 spans x 20..30, so the midpoint is 25.
        assert_eq!(hit(21.0, 0.0).side, Side::Left);
        assert_eq!(
            hit(25.0, 0.0).side,
            Side::Left,
            "the midpoint itself is left"
        );
        assert_eq!(hit(26.0, 0.0).side, Side::Right);
        // The cell is unaffected by which half.
        assert_eq!(hit(21.0, 0.0).col, 2);
        assert_eq!(hit(29.0, 0.0).col, 2);
    }

    /// Dragging out of the panel must extend to the edge it left through, not
    /// freeze at the last sample inside.
    #[test]
    fn overshoot_clamps_into_the_grid() {
        assert_eq!(hit(9_999.0, 0.0).col, 7);
        assert_eq!(hit(0.0, 9_999.0).row, 3);
        assert_eq!(hit(-50.0, 0.0).col, 0);
        assert_eq!(hit(0.0, -50.0).row, 0);
    }

    /// The part clamping alone does not give you: past the right or bottom
    /// edge the side is forced Right so the last cell is *included*, and above
    /// the top it is forced Left. Dragging below-and-left must still take the
    /// bottom row whole.
    #[test]
    fn overshoot_forces_the_side_to_the_edge() {
        assert_eq!(hit(9_999.0, 10.0).side, Side::Right);
        // x sits in cell 0's left half, but the row overshot — Right wins.
        assert_eq!(hit(1.0, 9_999.0).side, Side::Right);
        assert_eq!(hit(1.0, 9_999.0).col, 0);
        // Above the top, mirrored.
        assert_eq!(hit(75.0, -50.0).side, Side::Left);
    }

    #[test]
    fn degenerate_metrics_do_not_panic() {
        assert_eq!(
            cell_at(5.0, 5.0, 0.0, 20.0, 8, 4),
            CellHit {
                row: 0,
                col: 0,
                side: Side::Left
            }
        );
        assert_eq!(
            cell_at(5.0, 5.0, 10.0, 20.0, 0, 0),
            CellHit {
                row: 0,
                col: 0,
                side: Side::Left
            }
        );
        assert_eq!(cell_at(f32::NAN, f32::INFINITY, 10.0, 20.0, 8, 4).col, 0);
    }

    #[test]
    fn drag_threshold_matches_the_gpui_default() {
        assert_eq!(SELECTION_DRAG_THRESHOLD, 2.0);
    }

    #[test]
    fn glyphs_snap_to_their_cluster_column() {
        let glyph = |index, x: f32| gpui::ShapedGlyph {
            id: gpui::GlyphId(0),
            position: point(px(x), px(0.)),
            index,
            is_emoji: false,
        };
        // `->x` from col 3: a ligature 7px wide for `->`, a combining mark 2px
        // into its base `x`.
        let mut line = ShapedLine::default();
        *line = Arc::new(gpui::LineLayout {
            runs: vec![gpui::ShapedRun {
                font_id: gpui::FontId(0),
                glyphs: vec![glyph(0, 0.), glyph(2, 7.), glyph(2, 9.)],
            }],
            ..Default::default()
        });
        snap_to_cells(&mut line, &[3, 4, 5, 5], 3, px(10.));
        let xs: Vec<f32> = line.runs[0]
            .glyphs
            .iter()
            .map(|g| f32::from(g.position.x))
            .collect();
        assert_eq!(xs, [0., 20., 22.]);
    }
}
