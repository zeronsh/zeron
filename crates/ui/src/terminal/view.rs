//! Terminal paint + input encoding.
//!
//! - theme-owned background/ANSI16 plus the xterm 256-color cube/grayscale;
//! - keystroke → PTY byte encoding (printables, control keys, arrows/nav
//!   escape sequences, Ctrl- combos, Alt prefixing);
//! - the ordered input queue and the 80 ms resize debounce constant (the
//!   panel drives the writes and timers; the buffer logic here is pure);
//! - [`TerminalElement`] — a custom gpui element that measures cell metrics
//!   from the real mono font (the "font probe"), reports the resulting
//!   cols×rows back to the panel, and paints the grid: background quads for
//!   non-default cells, one `ShapedLine` per row (same font whatever the
//!   colors — paint never changes layout), and the cursor block.

use gpui::{
    App, Bounds, Entity, GlobalElementId, Hsla, LayoutId, Modifiers, PaintQuad, Pixels, ShapedLine,
    SharedString, Style, TextRun, Window, fill, font, outline, point, px, relative, size,
};

use crate::theme::{Appearance, Theme, rgb_to_hsl};

use super::emulator::{CellColor, CellSnapshot, CursorShape, Side};
use super::panel::TerminalPanel;

/// Default terminal font metrics; the rendered values come from the theme.
pub const TERM_FONT_SIZE: f32 = 13.0;
pub const TERM_LINE_HEIGHT: f32 = 18.0;
/// Line height as a multiple of the font size, so a user-chosen size keeps the
/// default's row rhythm.
const TERM_LINE_HEIGHT_RATIO: f32 = TERM_LINE_HEIGHT / TERM_FONT_SIZE;
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

/// Encode a keystroke as PTY bytes. `None` means "not ours" — the event should
/// fall through (e.g. the platform-primary shortcuts that drive app actions).
///
/// `app_cursor` switches arrows/home/end from CSI to SS3 per DECCKM.
pub fn keystroke_bytes(
    key: &str,
    key_char: Option<&str>,
    mods: &Modifiers,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    // Platform-primary combos (Cmd on macOS, the super key elsewhere) belong to
    // the app keymap, never the PTY.
    if mods.platform {
        return None;
    }
    if mods.alt {
        // ESC-prefix the same keystroke without alt.
        let inner = keystroke_bytes(
            key,
            key_char,
            &Modifiers {
                alt: false,
                ..*mods
            },
            app_cursor,
        )?;
        let mut out = vec![0x1b];
        out.extend(inner);
        return Some(out);
    }
    if mods.control {
        return control_bytes(key);
    }

    let seq = |csi: &[u8], ss3: &[u8]| {
        Some(if app_cursor {
            ss3.to_vec()
        } else {
            csi.to_vec()
        })
    };
    match key {
        "enter" => Some(b"\r".to_vec()),
        "backspace" => Some(vec![0x7f]),
        "tab" => Some(if mods.shift {
            b"\x1b[Z".to_vec()
        } else {
            b"\t".to_vec()
        }),
        "escape" => Some(vec![0x1b]),
        "space" => Some(b" ".to_vec()),
        "up" => seq(b"\x1b[A", b"\x1bOA"),
        "down" => seq(b"\x1b[B", b"\x1bOB"),
        "right" => seq(b"\x1b[C", b"\x1bOC"),
        "left" => seq(b"\x1b[D", b"\x1bOD"),
        "home" => seq(b"\x1b[H", b"\x1bOH"),
        "end" => seq(b"\x1b[F", b"\x1bOF"),
        "insert" => Some(b"\x1b[2~".to_vec()),
        "delete" => Some(b"\x1b[3~".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        "f1" => Some(b"\x1bOP".to_vec()),
        "f2" => Some(b"\x1bOQ".to_vec()),
        "f3" => Some(b"\x1bOR".to_vec()),
        "f4" => Some(b"\x1bOS".to_vec()),
        "f5" => Some(b"\x1b[15~".to_vec()),
        "f6" => Some(b"\x1b[17~".to_vec()),
        "f7" => Some(b"\x1b[18~".to_vec()),
        "f8" => Some(b"\x1b[19~".to_vec()),
        "f9" => Some(b"\x1b[20~".to_vec()),
        "f10" => Some(b"\x1b[21~".to_vec()),
        "f11" => Some(b"\x1b[23~".to_vec()),
        "f12" => Some(b"\x1b[24~".to_vec()),
        _ => {
            // Printable: prefer the typed character (IME/shift-aware).
            let text = key_char.filter(|c| !c.is_empty()).or({
                // Fall back to single-char key names ("a", "/", …).
                if key.chars().count() == 1 {
                    Some(key)
                } else {
                    None
                }
            })?;
            Some(text.as_bytes().to_vec())
        }
    }
}

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
/// named keys and modified keys are encoded here.
///
/// Option follows the layout: where Option types an ASCII symbol (`@`, `|`,
/// `\` on many European layouts) that symbol is typed; where it would type
/// something else (`∫` for Option-B on US layouts) it acts as Alt/Meta, so
/// shell word motions and editor bindings work without a setting.
pub fn route_key(
    key: &str,
    key_char: Option<&str>,
    mods: &Modifiers,
    app_cursor: bool,
) -> KeyRoute {
    if mods.platform {
        return KeyRoute::NotOurs;
    }
    let printable = key_char.filter(|c| !c.is_empty() && !c.chars().any(char::is_control));
    if mods.alt && !mods.control {
        if let Some(text) = printable
            && text.is_ascii()
            && !text.eq_ignore_ascii_case(key)
            && key.chars().count() == 1
        {
            return KeyRoute::Text;
        }
        // Meta: ESC + the key as typed without Option.
        let base = (key.chars().count() == 1).then(|| {
            if mods.shift {
                key.to_uppercase()
            } else {
                key.to_string()
            }
        });
        return keystroke_bytes(key, base.as_deref(), mods, app_cursor)
            .map_or(KeyRoute::NotOurs, KeyRoute::Bytes);
    }
    if !mods.control && !mods.function && printable.is_some() && !is_named_key(key) {
        return KeyRoute::Text;
    }
    keystroke_bytes(key, key_char, mods, app_cursor).map_or(KeyRoute::NotOurs, KeyRoute::Bytes)
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

/// Ctrl-key encoding (caret notation).
fn control_bytes(key: &str) -> Option<Vec<u8>> {
    let mut chars = key.chars();
    let (c, rest) = (chars.next()?, chars.next());
    if rest.is_some() {
        return match key {
            "space" => Some(vec![0x00]),
            "backspace" => Some(vec![0x08]),
            "enter" => Some(b"\r".to_vec()),
            _ => None,
        };
    }
    match c {
        'a'..='z' => Some(vec![c as u8 - b'a' + 1]),
        '@' => Some(vec![0x00]),
        '[' => Some(vec![0x1b]),
        '\\' => Some(vec![0x1c]),
        ']' => Some(vec![0x1d]),
        '^' => Some(vec![0x1e]),
        '_' | '/' => Some(vec![0x1f]),
        '?' => Some(vec![0x7f]),
        _ => None,
    }
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
    /// Grid cell advance, so paint can place segments by column.
    cell_w: Pixels,
    /// Grid row height, so paint places rows on the same baselines prepaint
    /// measured the grid with.
    line_h: Pixels,
    cursor: Option<PaintQuad>,
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
        // Ligatures OFF. A terminal is a fixed grid: the shaper must emit one
        // cell-width advance per character, and a contextual substitution
        // (Geist Mono ligates `--`, `->`, …) collapses several cells into
        // fewer glyphs, so the row renders SHORT while the cursor — a quad at
        // `cell_w * col` — stays on the true column. That is the `codex
        // --yolo` → `codex--yolo` report: the space is in the grid and went
        // to the pty (the command runs), only the painted run lost a cell.
        // The landing page disables the same three features on its ASCII art
        // for the same reason.
        let mut mono = font(theme.font_terminal.clone());
        mono.features = gpui::FontFeatures(std::sync::Arc::new(vec![
            ("liga".into(), 0),
            ("calt".into(), 0),
            ("dlig".into(), 0),
        ]));
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
                cell_w,
                line_h,
                cursor: None,
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
            lines.push((y, shape_row(row, &theme, &mono, font_size, window)));
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
            match (self.focused, snapshot.cursor_shape) {
                (false, _) | (true, CursorShape::HollowBlock) => {
                    outline(cell, theme.cursor, gpui::BorderStyle::Solid)
                }
                // Translucent block: the glyph underneath stays legible.
                (true, CursorShape::Block) => fill(cell, theme.cursor),
                (true, CursorShape::Beam) => fill(
                    Bounds::new(cell.origin, size(bar, line_h)),
                    theme.cursor.opacity(1.0),
                ),
                (true, CursorShape::Underline) => fill(
                    Bounds::new(point(cell.left(), cell.bottom() - bar), size(cell_w, bar)),
                    theme.cursor.opacity(1.0),
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
            cell_w,
            line_h,
            cursor,
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

/// Shape one grid row into COLUMN-PINNED segments.
///
/// A terminal is a fixed grid, but a shaped line places glyphs by their font
/// advances. Those agree only while every glyph is monospace-width — the row's
/// `cell_w` IS the mono font's em advance. The moment a glyph resolves through
/// FONT FALLBACK (box drawing `│─╭`, arrows `→`, emoji, CJK) its advance is
/// whatever that other font uses, and the whole rest of the line slides out of
/// the grid: box borders land a few pixels off (user report: "one of the pipes
/// is broken"), and a double-width glyph whose fallback advances only one cell
/// swallows the column after it (user report: `codex --yolo` rendering as
/// `codex--yolo`). Backgrounds, selection and the cursor never drifted because
/// those are quads placed at `cell_w * col`.
///
/// So: runs of ASCII shape together (guaranteed cell-width in a mono font),
/// and every other glyph is its own segment pinned at its own column. Wide
/// spacers are still skipped — the wide glyph covers both columns, and the
/// NEXT segment re-pins regardless.
fn shape_row(
    row: &[CellSnapshot],
    theme: &Theme,
    mono: &gpui::Font,
    font_size: Pixels,
    window: &Window,
) -> Vec<(usize, ShapedLine)> {
    fn flush(
        segments: &mut Vec<(usize, ShapedLine)>,
        text: &mut String,
        runs: &mut Vec<TextRun>,
        seg_col: usize,
        font_size: Pixels,
        window: &Window,
    ) {
        if text.is_empty() {
            return;
        }
        let shaped = window.text_system().shape_line(
            SharedString::from(std::mem::take(text)),
            font_size,
            runs,
            None,
        );
        segments.push((seg_col, shaped));
        runs.clear();
    }

    let mut segments: Vec<(usize, ShapedLine)> = Vec::new();
    let mut text = String::with_capacity(row.len());
    let mut runs: Vec<TextRun> = Vec::new();
    let mut seg_col = 0usize;

    for (col, cell) in row.iter().enumerate() {
        if cell.wide_spacer {
            continue;
        }
        let ch = if cell.hidden { ' ' } else { cell.ch };
        // Anything that can leave the mono font gets its own pinned segment.
        let pinned = !ch.is_ascii() || cell.wide;
        if pinned {
            flush(
                &mut segments,
                &mut text,
                &mut runs,
                seg_col,
                font_size,
                window,
            );
        }
        if text.is_empty() {
            seg_col = col;
        }
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
        let mut cell_font = mono.clone();
        cell_font.weight = if cell.bold {
            gpui::FontWeight::BOLD
        } else {
            gpui::FontWeight::NORMAL
        };
        cell_font.style = if cell.italic {
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
                color: Some(color),
                thickness: px(1.0),
                wavy: false,
            })
        };
        let len = ch.len_utf8();
        text.push(ch);
        match runs.last_mut() {
            Some(last)
                if last.color == color && last.font == cell_font && last.underline == underline =>
            {
                last.len += len;
            }
            _ => runs.push(TextRun {
                len,
                font: cell_font,
                color,
                background_color: None,
                underline,
                strikethrough: None,
            }),
        }
        if pinned {
            flush(
                &mut segments,
                &mut text,
                &mut runs,
                seg_col,
                font_size,
                window,
            );
        }
    }
    flush(
        &mut segments,
        &mut text,
        &mut runs,
        seg_col,
        font_size,
        window,
    );
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods() -> Modifiers {
        Modifiers::default()
    }

    #[test]
    fn printables_prefer_key_char() {
        assert_eq!(
            keystroke_bytes("a", Some("a"), &mods(), false),
            Some(b"a".to_vec())
        );
        assert_eq!(
            keystroke_bytes(
                "a",
                Some("A"),
                &Modifiers {
                    shift: true,
                    ..mods()
                },
                false
            ),
            Some(b"A".to_vec())
        );
        // Multi-byte characters pass through as UTF-8.
        assert_eq!(
            keystroke_bytes("e", Some("é"), &mods(), false),
            Some("é".as_bytes().to_vec())
        );
        // Named single-char keys fall back to the key name.
        assert_eq!(
            keystroke_bytes("/", None, &mods(), false),
            Some(b"/".to_vec())
        );
        // Unknown multi-char keys are not ours.
        assert_eq!(keystroke_bytes("capslock", None, &mods(), false), None);
    }

    #[test]
    fn control_keys_and_sequences() {
        assert_eq!(
            keystroke_bytes("enter", None, &mods(), false),
            Some(b"\r".to_vec())
        );
        assert_eq!(
            keystroke_bytes("backspace", None, &mods(), false),
            Some(vec![0x7f])
        );
        assert_eq!(
            keystroke_bytes("tab", None, &mods(), false),
            Some(b"\t".to_vec())
        );
        assert_eq!(
            keystroke_bytes(
                "tab",
                None,
                &Modifiers {
                    shift: true,
                    ..mods()
                },
                false
            ),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            keystroke_bytes("escape", None, &mods(), false),
            Some(vec![0x1b])
        );
        assert_eq!(
            keystroke_bytes("delete", None, &mods(), false),
            Some(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            keystroke_bytes("pageup", None, &mods(), false),
            Some(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            keystroke_bytes("f5", None, &mods(), false),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn arrows_respect_app_cursor_mode() {
        assert_eq!(
            keystroke_bytes("up", None, &mods(), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            keystroke_bytes("up", None, &mods(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            keystroke_bytes("home", None, &mods(), false),
            Some(b"\x1b[H".to_vec())
        );
        assert_eq!(
            keystroke_bytes("end", None, &mods(), true),
            Some(b"\x1bOF".to_vec())
        );
    }

    #[test]
    fn ctrl_combos_map_to_control_bytes() {
        let ctrl = Modifiers {
            control: true,
            ..mods()
        };
        assert_eq!(
            keystroke_bytes("c", Some("c"), &ctrl, false),
            Some(vec![0x03])
        );
        assert_eq!(keystroke_bytes("z", None, &ctrl, false), Some(vec![0x1a]));
        assert_eq!(
            keystroke_bytes("space", None, &ctrl, false),
            Some(vec![0x00])
        );
        assert_eq!(keystroke_bytes("[", None, &ctrl, false), Some(vec![0x1b]));
        assert_eq!(keystroke_bytes("_", None, &ctrl, false), Some(vec![0x1f]));
        // Ctrl+1 has no caret encoding — not ours.
        assert_eq!(keystroke_bytes("1", Some("1"), &ctrl, false), None);
    }

    #[test]
    fn alt_prefixes_escape() {
        let alt = Modifiers {
            alt: true,
            ..mods()
        };
        assert_eq!(
            keystroke_bytes("b", Some("b"), &alt, false),
            Some(vec![0x1b, b'b'])
        );
        let alt_ctrl = Modifiers {
            alt: true,
            control: true,
            ..mods()
        };
        assert_eq!(
            keystroke_bytes("c", None, &alt_ctrl, false),
            Some(vec![0x1b, 0x03])
        );
    }

    #[test]
    fn platform_primary_combos_fall_through() {
        let cmd = Modifiers {
            platform: true,
            ..mods()
        };
        assert_eq!(keystroke_bytes("j", Some("j"), &cmd, false), None);
        assert_eq!(keystroke_bytes("enter", None, &cmd, false), None);
    }

    #[test]
    fn printable_keys_go_through_text_input_and_named_keys_are_encoded() {
        let shift = Modifiers {
            shift: true,
            ..mods()
        };
        assert_eq!(route_key("a", Some("a"), &mods(), false), KeyRoute::Text);
        assert_eq!(route_key("a", Some("A"), &shift, false), KeyRoute::Text);
        assert_eq!(
            route_key("enter", Some("\r"), &mods(), false),
            KeyRoute::Bytes(b"\r".to_vec())
        );
        assert_eq!(
            route_key("up", None, &mods(), true),
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
                false
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
                false
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
        // US layout: Option-B would type `∫` — Meta-b for word motion.
        assert_eq!(
            route_key("b", Some("∫"), &alt, false),
            KeyRoute::Bytes(vec![0x1b, b'b'])
        );
        assert_eq!(
            route_key("b", Some("ı"), &Modifiers { shift: true, ..alt }, false),
            KeyRoute::Bytes(vec![0x1b, b'B'])
        );
        // German layout: Option-L types `@`, which is what the user meant.
        assert_eq!(route_key("l", Some("@"), &alt, false), KeyRoute::Text);
        // Named keys keep their Meta prefix.
        assert_eq!(
            route_key("left", None, &alt, false),
            KeyRoute::Bytes(b"\x1b\x1b[D".to_vec())
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
}
