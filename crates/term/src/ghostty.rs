//! The desktop's terminal emulator: Ghostty's libghostty-vt (the `ghostty-vt`
//! crate, built from Ghostty's source with Zig 0.16) behind the same API as
//! [`crate::emulator`], which the mobile apps keep (it is `Send`, and builds
//! for Android).
//!
//! Bytes in ([`Emulator::feed`]), grid snapshots out ([`Emulator::lines`],
//! [`Emulator::cursor`]). No I/O, no timers, no gpui. What this adds over the
//! alacritty emulator: a cell keeps its whole grapheme cluster (flags, skin
//! tones, ZWJ sequences, combining marks) and its text decorations
//! (strikethrough, undercurl, underline color); keys encode through
//! Ghostty's encoder, which reads the terminal's modes ([`Emulator::terminal`]).
//!
//! API notes for the pinned `ghostty-vt 0.7.0` (Ghostty `b40acce`):
//! - Snapshots read through a `RenderState`, rereading only rows Ghostty marked
//!   dirty; the handles are `!Send`, which suits an emulator inside a gpui
//!   entity.
//! - Query responses (DSR/DA/…) surface as `Event::PtyWrite` after `vt_write`;
//!   [`Emulator::feed`] returns them so the panel can write them back.
//! - A synchronized update (mode 2026) applies output at once; the render-hold
//!   callback captures the frame on screen when it starts, and snapshots show
//!   that frame until the update ends or times out.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ghostty_vt::key::KittyKeyFlags;
use ghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RowIterator};
use ghostty_vt::screen::{Cell, CellContentTag, CellWide, GridRef, Screen, TrackedGridRef};
use ghostty_vt::selection::{
    FormatOptions, SelectLineOptions, SelectWordBetweenOptions, Selection,
};
use ghostty_vt::style::{Style, StyleColor, Underline};
use ghostty_vt::terminal::{
    CursorStyle as GhosttyCursorStyle, Mode, Point, PointCoordinate, PointSpace, ScrollViewport,
};
use ghostty_vt::{Error, Event, RenderState, Terminal, TerminalOptions};

pub use crate::emulator::{
    CellColor, CursorShape, CursorSnapshot, MouseTracking, SCROLLBACK_LINES,
};

/// How long a synchronized update may hold the screen: Ghostty's one second.
const SYNC_TIMEOUT: Duration = Duration::from_secs(1);

/// A cell in screen coordinates: row 0 is the oldest line of history, so a
/// point names the same text however the view is scrolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GridPoint {
    pub row: usize,
    pub col: usize,
}

/// Which half of a cell the pointer is on: a drag starting on a cell's right
/// half leaves that cell out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Selection granularity: a drag, a double-click word, a triple-click row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionType {
    Simple,
    Semantic,
    Lines,
}

fn map_color(color: StyleColor, default: CellColor) -> CellColor {
    match color {
        StyleColor::None => default,
        StyleColor::Palette(ix) => CellColor::Indexed(ix.0),
        StyleColor::Rgb(c) => CellColor::Rgb(c.r, c.g, c.b),
    }
}

/// One rendered cell: char + colors + the flags paint cares about.
#[derive(Debug, Clone, PartialEq)]
pub struct CellSnapshot {
    pub ch: char,
    /// The whole grapheme cluster, `ch` first, when the cell holds more than
    /// `ch`: a flag, a skin tone, a ZWJ sequence, a letter and its combining
    /// marks.
    pub cluster: Option<Arc<str>>,
    pub fg: CellColor,
    pub bg: CellColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    /// The underline is curly (SGR 4:3): spelling and diagnostics.
    // ponytail: double, dotted and dashed underlines draw as a single line.
    pub undercurl: bool,
    /// SGR 58; `None` underlines in the text's color.
    pub underline_color: Option<CellColor>,
    pub strikethrough: bool,
    pub inverse: bool,
    pub hidden: bool,
    /// A double-width char (occupies this cell plus the next spacer cell).
    pub wide: bool,
    /// The spacer half of a wide char — never shaped, only background-painted.
    pub wide_spacer: bool,
    /// Inside the active selection: the view paints a wash over this cell.
    pub selected: bool,
    /// Typed locally and shown before the remote echo confirmed it.
    pub predicted: bool,
}

impl CellSnapshot {
    /// Effective paint colors after INVERSE/HIDDEN resolution.
    pub fn display_colors(&self) -> (CellColor, CellColor) {
        let (fg, bg) = if self.inverse {
            (self.bg, self.fg)
        } else {
            (self.fg, self.bg)
        };
        if self.hidden { (bg, bg) } else { (fg, bg) }
    }

    /// A cell from libghostty's raw cell, its style and its grapheme cluster.
    fn new(
        raw: Cell,
        style: &Style,
        cluster: Option<Arc<str>>,
        selected: bool,
    ) -> Result<Self, Error> {
        // An erase with a background set stores the color in the cell, not a style.
        let bg = match raw.content_tag()? {
            CellContentTag::BgColorPalette => CellColor::Indexed(raw.bg_color_palette()?.0),
            CellContentTag::BgColorRgb => {
                let c = raw.bg_color_rgb()?;
                CellColor::Rgb(c.r, c.g, c.b)
            }
            _ => map_color(style.bg_color, CellColor::Background),
        };
        let wide = raw.wide()?;
        Ok(Self {
            ch: char::from_u32(raw.codepoint()?)
                .filter(|&c| c != '\0')
                .unwrap_or(' '),
            cluster,
            fg: map_color(style.fg_color, CellColor::Foreground),
            bg,
            bold: style.bold,
            dim: style.faint,
            italic: style.italic,
            underline: style.underline != Underline::None,
            undercurl: style.underline == Underline::Curly,
            underline_color: match style.underline_color {
                StyleColor::None => None,
                color => Some(map_color(color, CellColor::Foreground)),
            },
            strikethrough: style.strikethrough,
            inverse: style.inverse,
            hidden: style.invisible,
            wide: wide == CellWide::Wide,
            wide_spacer: matches!(wide, CellWide::SpacerTail | CellWide::SpacerHead),
            selected,
            predicted: false,
        })
    }
}

/// The render-path handles, reused across snapshots.
struct Render {
    state: RenderState,
    rows: RowIterator,
    cells: CellIterator,
    /// The rows as last read: a row Ghostty did not mark dirty since is taken
    /// from here, not read cell by cell again.
    lines: Vec<Vec<CellSnapshot>>,
}

/// What snapshots show while a synchronized update holds the screen.
struct Frame {
    lines: Vec<Vec<CellSnapshot>>,
    cursor: Option<CursorSnapshot>,
    since: Instant,
}

/// State the render-hold callback shares with the emulator. Behind a
/// `RefCell` because snapshots are `&self` reads that update the render state.
struct Shared {
    render: Render,
    held: Option<Frame>,
}

/// The selection being made: its granularity and where it started. The start
/// is tracked, so it stays on its text while output scrolls the grid under a
/// drag.
struct Anchor {
    ty: SelectionType,
    start: TrackedGridRef,
    side: Side,
}

/// The emulator: a pure fold of PTY bytes into a renderable grid.
pub struct Emulator {
    term: Terminal,
    shared: Rc<RefCell<Shared>>,
    anchor: Option<Anchor>,
    bell: bool,
}

impl Emulator {
    pub fn new(cols: u16, rows: u16) -> Self {
        let mut term = Terminal::new(TerminalOptions {
            cols: cols.max(2),
            rows: rows.max(1),
            max_scrollback: TerminalOptions::default().max_scrollback,
        })
        .expect("libghostty-vt terminal");
        // Lines, as the alacritty emulator keeps them, not Ghostty's byte
        // estimate (pruned by ~400 KiB pages, it keeps far fewer lines on a
        // narrow grid).
        let _ = term.set_scrollback_max_bytes(None);
        let _ = term.set_scrollback_max_lines(Some(SCROLLBACK_LINES));
        // A blinking block until the program asks otherwise (DECSCUSR);
        // `CSI 0 q` returns here.
        let _ = term.set_default_cursor_style(Some(GhosttyCursorStyle::Block));
        let _ = term.set_default_cursor_blink(Some(true));
        let shared = Rc::new(RefCell::new(Shared {
            render: Render {
                state: RenderState::new().expect("libghostty-vt render state"),
                rows: RowIterator::new().expect("libghostty-vt row iterator"),
                cells: CellIterator::new().expect("libghostty-vt cell iterator"),
                lines: Vec::new(),
            },
            held: None,
        }));
        let hold = Rc::downgrade(&shared);
        term.set_render_hold(Some(Box::new(move |term, held| {
            let Some(shared) = hold.upgrade() else { return };
            let mut shared = shared.borrow_mut();
            shared.held = held.then(|| Frame {
                lines: read_lines(term, &mut shared.render).unwrap_or_default(),
                cursor: cursor(term),
                since: Instant::now(),
            });
        })));
        Self {
            term,
            shared,
            anchor: None,
            bell: false,
        }
    }

    /// Advance the state machine over decoded PTY output. Returns bytes the
    /// terminal wants written back to the PTY (DSR/DA query responses etc.).
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.term.vt_write(bytes);
        let mut responses = Vec::new();
        for event in self.term.events() {
            match event {
                Event::PtyWrite(bytes) => responses.extend_from_slice(&bytes),
                Event::Bell => self.bell = true,
                _ => {}
            }
        }
        responses
    }

    /// When a synchronized update (DEC 2026) is holding the screen, the moment
    /// it must be shown anyway even if the program never ends it.
    pub fn sync_deadline(&self) -> Option<Instant> {
        self.shared
            .borrow()
            .held
            .as_ref()
            .map(|frame| frame.since + SYNC_TIMEOUT)
    }

    /// Show a synchronized update whose deadline passed. Returns any query
    /// responses it produced, like [`Self::feed`].
    pub fn finish_sync(&mut self) -> Vec<u8> {
        // Resetting the mode ends the hold without a callback.
        let _ = self.term.set_mode(Mode::SYNC_OUTPUT, false);
        self.shared.borrow_mut().held = None;
        Vec::new()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        // ponytail: no pixel size; only CSI 14/16 t size reports would want it.
        let _ = self.term.resize(cols.max(2), rows.max(1), 0, 0);
    }

    pub fn cols(&self) -> usize {
        self.term.cols().map_or(0, usize::from)
    }

    pub fn rows(&self) -> usize {
        self.term.rows().map_or(0, usize::from)
    }

    /// OSC title, if the running program set one.
    pub fn title(&self) -> Option<&str> {
        self.term.title().ok().filter(|t| !t.is_empty())
    }

    /// True once a BEL arrived; reading clears it.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    fn mode(&self, mode: Mode) -> bool {
        self.term.mode(mode).unwrap_or(false)
    }

    /// Arrow keys should send SS3 (`ESC O A`) instead of CSI.
    pub fn app_cursor_mode(&self) -> bool {
        self.mode(Mode::DECCKM)
    }

    /// The cursor's shape and whether it blinks, as the program set them.
    pub fn cursor_style(&self) -> (CursorShape, bool) {
        let mut shared = self.shared.borrow_mut();
        let Ok(snapshot) = shared.render.state.update(&self.term) else {
            return (CursorShape::Block, true);
        };
        let shape = match snapshot.cursor_visual_style() {
            Ok(CursorVisualStyle::Bar) => CursorShape::Beam,
            Ok(CursorVisualStyle::Underline) => CursorShape::Underline,
            Ok(CursorVisualStyle::BlockHollow) => CursorShape::HollowBlock,
            _ => CursorShape::Block,
        };
        (shape, snapshot.cursor_blinking().unwrap_or(true))
    }

    /// Pointer reporting the program asked for, if any.
    pub fn mouse_tracking(&self) -> Option<MouseTracking> {
        if self.mode(Mode::ANY_MOUSE) {
            Some(MouseTracking::Motion)
        } else if self.mode(Mode::BUTTON_MOUSE) {
            Some(MouseTracking::Drag)
        } else if self.mode(Mode::NORMAL_MOUSE) {
            Some(MouseTracking::Click)
        } else {
            None
        }
    }

    /// Mouse reports use the SGR encoding (1006) rather than the legacy one.
    pub fn sgr_mouse(&self) -> bool {
        self.mode(Mode::SGR_MOUSE)
    }

    /// A full-screen program owns the alternate screen.
    pub fn alt_screen(&self) -> bool {
        self.term
            .active_screen()
            .is_ok_and(|s| s == Screen::Alternate)
    }

    /// The wheel should arrive as arrow keys on the alternate screen (1007).
    pub fn alternate_scroll(&self) -> bool {
        self.mode(Mode::ALT_SCROLL)
    }

    /// The program wants `CSI I` / `CSI O` on focus changes (1004).
    pub fn focus_reporting(&self) -> bool {
        self.mode(Mode::FOCUS_EVENT)
    }

    /// Pastes should be wrapped in `ESC [200~` / `ESC [201~`.
    pub fn bracketed_paste_mode(&self) -> bool {
        self.mode(Mode::BRACKETED_PASTE)
    }

    /// The kitty keyboard flags the running program pushed (empty: legacy keys).
    pub fn kitty_keyboard_flags(&self) -> KittyKeyFlags {
        self.term
            .kitty_keyboard_flags()
            .unwrap_or(KittyKeyFlags::empty())
    }

    /// The terminal, for Ghostty's key encoder to read its modes from.
    pub fn terminal(&self) -> &Terminal {
        &self.term
    }

    /// The character in a live (not scrolled-back) viewport cell.
    pub fn live_char(&self, row: usize, col: usize) -> Option<char> {
        let point = Point::Active(PointCoordinate {
            x: u16::try_from(col).ok()?,
            y: u32::try_from(row).ok()?,
        });
        let codepoint = self
            .term
            .grid_ref(point)
            .ok()?
            .cell()
            .ok()?
            .codepoint()
            .ok()?;
        Some(
            char::from_u32(codepoint)
                .filter(|&c| c != '\0')
                .unwrap_or(' '),
        )
    }

    /// The cursor in live viewport coordinates, regardless of scrollback or
    /// visibility.
    pub fn live_cursor(&self) -> CursorSnapshot {
        CursorSnapshot {
            row: self.term.cursor_y().map_or(0, usize::from),
            col: self.term.cursor_x().map_or(0, usize::from),
        }
    }

    /// Lines scrolled back into history (0 = pinned to the live bottom).
    pub fn display_offset(&self) -> usize {
        self.term
            .scrollbar()
            .map_or(0, |s| s.total.saturating_sub(s.offset + s.len) as usize)
    }

    /// Lines available above the viewport.
    pub fn history_lines(&self) -> usize {
        self.term.scrollback_rows().unwrap_or(0)
    }

    /// Scroll the view: positive = up into history, negative = toward live.
    pub fn scroll(&mut self, delta: i32) {
        self.term
            .scroll_viewport(ScrollViewport::Delta(-(delta as isize)));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_viewport(ScrollViewport::Bottom);
    }

    /// Set the scrollback offset directly (0 = live bottom).
    pub fn scroll_to_offset(&mut self, offset: usize) {
        let top = self.history_lines().saturating_sub(offset);
        self.term.scroll_viewport(ScrollViewport::Row(top));
    }

    // ---- selection ----
    //
    // The terminal owns the installed selection and moves it with its text as
    // output scrolls the grid. What stays here is the tracked anchor of the one
    // being made, so each pointer move rebuilds it from anchor to pointer.

    fn grid_ref(&self, point: GridPoint) -> Option<GridRef<'_>> {
        let (x, y) = (
            u16::try_from(point.col).ok()?,
            u32::try_from(point.row).ok()?,
        );
        self.term
            .grid_ref(Point::Screen(PointCoordinate { x, y }))
            .ok()
    }

    fn point_of(&self, grid_ref: &GridRef<'_>) -> Option<GridPoint> {
        let c = self
            .term
            .point_from_grid_ref(grid_ref, PointSpace::Screen)
            .ok()??;
        Some(GridPoint {
            row: c.y as usize,
            col: c.x as usize,
        })
    }

    /// A selection's ends, top-left first.
    fn bounds(&self, selection: Option<Selection<'_>>) -> Option<(GridPoint, GridPoint)> {
        let selection = selection?;
        let (a, b) = (
            self.point_of(&selection.start())?,
            self.point_of(&selection.end())?,
        );
        Some((a.min(b), a.max(b)))
    }

    /// The viewport's top row in screen coordinates.
    fn viewport_top(&self) -> usize {
        self.term.scrollbar().map_or(0, |s| s.offset as usize)
    }

    /// The grid point under a viewport cell (row 0 = top of the visible area).
    ///
    /// Viewport rows are what the pointer hits; screen rows are what a
    /// selection anchors to, and the two differ by the scrollback position.
    pub fn grid_point(&self, viewport_row: usize, col: usize) -> GridPoint {
        GridPoint {
            row: self.viewport_top() + viewport_row,
            col: col.min(self.cols().saturating_sub(1)),
        }
    }

    /// Begin a selection. `ty` picks the granularity: [`SelectionType::Simple`]
    /// for a drag, `Semantic` for a double-click word, `Lines` for a triple-
    /// click row.
    pub fn start_selection(&mut self, ty: SelectionType, point: GridPoint, side: Side) {
        let Ok(start) = self.term.track_grid_ref(Point::Screen(PointCoordinate {
            x: point.col as u16,
            y: point.row as u32,
        })) else {
            return;
        };
        self.anchor = Some(Anchor { ty, start, side });
        self.update_selection(point, side);
    }

    /// Extend the in-progress selection to `point`. No-op without one.
    pub fn update_selection(&mut self, point: GridPoint, side: Side) {
        let Some(anchor) = &self.anchor else { return };
        let Some(start) = anchor
            .start
            .point(PointSpace::Screen)
            .ok()
            .flatten()
            .map(|c| GridPoint {
                row: c.y as usize,
                col: c.x as usize,
            })
        else {
            return;
        };
        let range = match anchor.ty {
            SelectionType::Simple => simple_range((start, anchor.side), (point, side), self.cols()),
            // Ghostty's double-click drag: the nearest word from each end toward the other.
            SelectionType::Semantic => {
                let word = |from, to| {
                    let options =
                        SelectWordBetweenOptions::new(self.grid_ref(from)?, self.grid_ref(to)?);
                    self.bounds(self.term.select_word_between(options).ok()?)
                };
                union(word(start, point), word(point, start))
            }
            SelectionType::Lines => {
                let line = |at| {
                    self.bounds(
                        self.term
                            .select_line(SelectLineOptions::new(self.grid_ref(at)?))
                            .ok()?,
                    )
                };
                union(line(start), line(point))
            }
        };
        let selection = range.and_then(|(start, end)| {
            Some(Selection::new(
                self.grid_ref(start)?,
                self.grid_ref(end)?,
                false,
            ))
        });
        let _ = self.term.set_selection(selection.as_ref());
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
        let _ = self.term.set_selection(None);
    }

    /// The selected text, or `None` when there is no selection or it covers
    /// nothing (a click without a drag leaves an empty one behind).
    pub fn selection_text(&self) -> Option<String> {
        // Ghostty's copy: plain text, soft wraps joined, trailing blanks trimmed.
        let options = FormatOptions::new().with_unwrap(true).with_trim(true);
        let bytes = self.term.format_selection_alloc(options).ok()??;
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        // A row selection pastes as a row, line break included (Ghostty's trim drops it).
        if self
            .anchor
            .as_ref()
            .is_some_and(|a| a.ty == SelectionType::Lines)
            && !text.is_empty()
        {
            text.push('\n');
        }
        Some(text).filter(|s| !s.is_empty())
    }

    /// Whether a non-empty selection is active — drives the copy action and
    /// the "clear it" branch on the next click.
    pub fn has_selection(&self) -> bool {
        self.term.selection().is_ok_and(|s| s.is_some())
    }

    /// Snapshot one viewport row (0 = top) honoring the scrollback offset.
    // ponytail: snapshots the whole viewport; only tests ask for one row.
    pub fn line(&self, viewport_row: usize) -> Vec<CellSnapshot> {
        self.lines()
            .into_iter()
            .nth(viewport_row)
            .unwrap_or_default()
    }

    /// All viewport rows, top to bottom.
    pub fn lines(&self) -> Vec<Vec<CellSnapshot>> {
        let mut shared = self.shared.borrow_mut();
        if let Some(frame) = &shared.held {
            return frame.lines.clone();
        }
        read_lines(&self.term, &mut shared.render).unwrap_or_default()
    }

    /// The history row just above the viewport, which a partial scroll
    /// reveals; `None` at the top of the scrollback.
    pub fn line_above(&self) -> Option<Vec<CellSnapshot>> {
        let row = self.viewport_top().checked_sub(1)?;
        let selection = self.term.selection().ok().flatten();
        (0..self.cols())
            .map(|col| {
                let point = Point::Screen(PointCoordinate {
                    x: col as u16,
                    y: row as u32,
                });
                let grid_ref = self.term.grid_ref(point).ok()?;
                let raw = grid_ref.cell().ok()?;
                let style = grid_ref.style().ok()?;
                let cluster = match raw.content_tag().ok()? {
                    CellContentTag::CodepointGrapheme => {
                        let mut buf = ['\0'; 16];
                        let len = grid_ref.graphemes(&mut buf).ok()?;
                        Some(buf[..len].iter().collect::<String>().into())
                    }
                    _ => None,
                };
                let selected = selection
                    .as_ref()
                    .is_some_and(|s| s.contains(&self.term, point).unwrap_or(false));
                CellSnapshot::new(raw, &style, cluster, selected).ok()
            })
            .collect()
    }

    /// Cursor in viewport coordinates; `None` when hidden or scrolled out.
    pub fn cursor(&self) -> Option<CursorSnapshot> {
        if let Some(frame) = &self.shared.borrow().held {
            return frame.cursor;
        }
        cursor(&self.term)
    }

    /// Test/diagnostic helper: a viewport row as trimmed text (wide-char
    /// spacers skipped).
    pub fn row_text(&self, viewport_row: usize) -> String {
        let mut text: String = self
            .line(viewport_row)
            .iter()
            .filter(|c| !c.wide_spacer)
            .map(|c| c.ch)
            .collect();
        while text.ends_with(' ') {
            text.pop();
        }
        text
    }
}

impl crate::predict::LiveScreen for Emulator {
    fn live_cursor(&self) -> CursorSnapshot {
        Emulator::live_cursor(self)
    }
    fn live_char(&self, row: usize, col: usize) -> Option<char> {
        Emulator::live_char(self, row, col)
    }
}

/// The cursor in viewport coordinates; `None` when hidden or scrolled out.
fn cursor(term: &Terminal) -> Option<CursorSnapshot> {
    if !term.is_cursor_visible().ok()? {
        return None;
    }
    let scrollbar = term.scrollbar().ok()?;
    let offset = scrollbar
        .total
        .saturating_sub(scrollbar.offset + scrollbar.len) as usize;
    let row = term.cursor_y().ok()? as usize + offset;
    let rows = usize::from(term.rows().ok()?);
    (row < rows).then_some(CursorSnapshot {
        row,
        col: term.cursor_x().ok()? as usize,
    })
}

/// The viewport's rows, rereading only those Ghostty marked dirty since the
/// last read.
fn read_lines(term: &Terminal, render: &mut Render) -> Result<Vec<Vec<CellSnapshot>>, Error> {
    let Render {
        state,
        rows,
        cells,
        lines: cache,
    } = render;
    let cols = usize::from(term.cols()?);
    let plain = Style::default();
    let snapshot = state.update(term)?;
    let full = snapshot.dirty()? == Dirty::Full;
    let mut lines = Vec::with_capacity(usize::from(term.rows()?));
    {
        let mut row_iter = rows.update(&snapshot)?;
        while let Some(row) = row_iter.next() {
            let selected = row.selection()?.map(|s| s.start_x..=s.end_x);
            // The selection is taken afresh for a clean row too: a changed one
            // may leave the row clean.
            if let Some(mut line) = cache
                .get(lines.len())
                .filter(|l| !full && l.len() == cols && !row.dirty().unwrap_or(true))
                .cloned()
            {
                for (col, cell) in line.iter_mut().enumerate() {
                    cell.selected = selected.as_ref().is_some_and(|r| r.contains(&(col as u16)));
                }
                lines.push(line);
                continue;
            }
            let mut line = Vec::with_capacity(cols);
            let mut cell_iter = cells.update(row)?;
            while let Some(cell) = cell_iter.next() {
                let raw = cell.raw_cell()?;
                let style = if cell.has_styling()? {
                    cell.style()?
                } else {
                    plain
                };
                let cluster = match raw.content_tag()? {
                    CellContentTag::CodepointGrapheme => {
                        let mut text = String::new();
                        cell.graphemes_utf8(&mut text)?;
                        Some(text.into())
                    }
                    _ => None,
                };
                let col = line.len() as u16;
                let selected = selected.as_ref().is_some_and(|r| r.contains(&col));
                line.push(CellSnapshot::new(raw, &style, cluster, selected)?);
            }
            lines.push(line);
        }
    }
    snapshot.clean()?;
    cache.clone_from(&lines);
    Ok(lines)
}

/// A drag's cells, top-left first: the side of each end says whether its cell is in.
fn simple_range(
    a: (GridPoint, Side),
    b: (GridPoint, Side),
    cols: usize,
) -> Option<(GridPoint, GridPoint)> {
    let ((mut start, start_side), (mut end, end_side)) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    if start_side == Side::Right {
        start = if start.col + 1 < cols {
            GridPoint {
                col: start.col + 1,
                ..start
            }
        } else {
            GridPoint {
                row: start.row + 1,
                col: 0,
            }
        };
    }
    if end_side == Side::Left {
        end = match end.col {
            0 => GridPoint {
                row: end.row.checked_sub(1)?,
                col: cols.saturating_sub(1),
            },
            col => GridPoint {
                col: col - 1,
                ..end
            },
        };
    }
    (start <= end).then_some((start, end))
}

/// The span covering both ranges.
fn union(
    a: Option<(GridPoint, GridPoint)>,
    b: Option<(GridPoint, GridPoint)>,
) -> Option<(GridPoint, GridPoint)> {
    match (a, b) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
        (a, b) => a.or(b),
    }
}

impl std::fmt::Debug for Emulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emulator")
            .field("cols", &self.cols())
            .field("rows", &self.rows())
            .field("display_offset", &self.display_offset())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emu(cols: u16, rows: u16) -> Emulator {
        Emulator::new(cols, rows)
    }

    #[test]
    fn cursor_style_follows_decscusr_and_resets_to_a_blinking_block() {
        let mut e = emu(10, 2);
        assert_eq!(e.cursor_style(), (CursorShape::Block, true));
        e.feed(b"\x1b[6 q");
        assert_eq!(e.cursor_style(), (CursorShape::Beam, false));
        e.feed(b"\x1b[3 q");
        assert_eq!(e.cursor_style(), (CursorShape::Underline, true));
        e.feed(b"\x1b[0 q");
        assert_eq!(e.cursor_style(), (CursorShape::Block, true));
    }

    #[test]
    fn input_modes_track_the_program() {
        let mut e = emu(10, 2);
        assert_eq!(e.mouse_tracking(), None);
        e.feed(b"\x1b[?1000h\x1b[?1006h\x1b[?1049h\x1b[?1004h\x1b[?1007h");
        assert_eq!(e.mouse_tracking(), Some(MouseTracking::Click));
        assert!(e.sgr_mouse() && e.alt_screen() && e.focus_reporting() && e.alternate_scroll());
        e.feed(b"\x1b[?1002h");
        assert_eq!(e.mouse_tracking(), Some(MouseTracking::Drag));
        e.feed(b"\x1b[?1003h");
        assert_eq!(e.mouse_tracking(), Some(MouseTracking::Motion));
        e.feed(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1049l");
        assert_eq!(e.mouse_tracking(), None);
        assert!(!e.alt_screen());
    }

    /// The frame on screen when a synchronized update starts stays there
    /// until the update ends or times out, the cursor with it.
    #[test]
    fn a_synchronized_update_holds_the_screen_until_it_ends_or_times_out() {
        let mut e = emu(10, 2);
        e.feed(b"before");
        e.feed(b"\x1b[?2026h\r\x1b[Kpartial");
        assert_eq!(e.row_text(0), "before", "held while the update is open");
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 0, col: 6 }));
        assert!(e.sync_deadline().is_some_and(|d| d > Instant::now()));
        e.finish_sync();
        assert_eq!(e.row_text(0), "partial");
        assert!(e.sync_deadline().is_none());
        e.feed(b"\x1b[?2026h\r\x1b[Kwhole\x1b[?2026l");
        assert_eq!(e.row_text(0), "whole", "a closed update lands at once");
        assert!(e.sync_deadline().is_none());
    }

    /// What prediction checks echoes against: the live grid, whatever the
    /// view shows.
    #[test]
    fn live_cells_and_cursor_ignore_the_scrollback_view() {
        let mut e = emu(10, 2);
        e.feed(b"one\r\ntwo\r\nthr");
        e.scroll(1);
        assert_eq!(e.live_cursor(), CursorSnapshot { row: 1, col: 3 });
        assert_eq!(e.live_char(0, 1), Some('w'));
        assert_eq!(e.live_char(1, 2), Some('r'));
        assert_eq!(e.live_char(1, 5), Some(' '));
        assert_eq!(e.live_char(5, 0), None);
    }

    /// A partial scroll reveals the history row above the viewport.
    #[test]
    fn the_line_above_the_view_comes_from_history() {
        let mut e = emu(10, 2);
        assert_eq!(e.line_above(), None, "nothing above a fresh screen");
        e.feed(b"\x1b[1mone\x1b[0m\r\ntwo\r\nthree");
        let above: String = e.line_above().unwrap().iter().map(|c| c.ch).collect();
        assert_eq!(above.trim_end(), "one");
        assert!(e.line_above().unwrap()[0].bold);
        e.scroll(1);
        assert_eq!(e.line_above(), None, "the top of the history");
    }

    #[test]
    fn plain_text_lands_on_row_zero() {
        let mut e = emu(20, 5);
        e.feed(b"hello");
        assert_eq!(e.row_text(0), "hello");
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 0, col: 5 }));
    }

    #[test]
    fn crlf_moves_lines_and_cr_returns_to_column_zero() {
        let mut e = emu(20, 5);
        e.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(e.row_text(0), "one");
        assert_eq!(e.row_text(1), "two");
        assert_eq!(e.row_text(2), "three");
        e.feed(b"\rXX");
        assert_eq!(e.row_text(2), "XXree");
    }

    #[test]
    fn long_line_wraps_at_the_grid_width() {
        let mut e = emu(10, 4);
        e.feed(b"abcdefghijKLM");
        assert_eq!(e.row_text(0), "abcdefghij");
        assert_eq!(e.row_text(1), "KLM");
    }

    #[test]
    fn sgr_colors_and_attributes() {
        let mut e = emu(40, 4);
        e.feed(b"\x1b[31mred\x1b[0m plain \x1b[1;44mboldbg\x1b[0m");
        let line = e.line(0);
        assert_eq!(line[0].fg, CellColor::Indexed(1));
        assert_eq!(line[0].bg, CellColor::Background);
        // After reset: defaults.
        assert_eq!(line[4].fg, CellColor::Foreground);
        // Bold + blue background segment starts at col 10 ("red plain " = 10).
        let bold_cell = &line[10];
        assert!(bold_cell.bold);
        assert_eq!(bold_cell.bg, CellColor::Indexed(4));
    }

    #[test]
    fn bright_256_and_truecolor_sgr() {
        let mut e = emu(40, 2);
        e.feed(b"\x1b[95mA\x1b[38;5;196mB\x1b[38;2;10;20;30mC");
        let line = e.line(0);
        assert_eq!(line[0].fg, CellColor::Indexed(13)); // bright magenta
        assert_eq!(line[1].fg, CellColor::Indexed(196));
        assert_eq!(line[2].fg, CellColor::Rgb(10, 20, 30));
    }

    /// Neovim's diagnostics: an undercurl in its own color (SGR 4:3, 58),
    /// and strikethrough for deprecated names (SGR 9).
    #[test]
    fn strikethrough_undercurl_and_underline_color() {
        let mut e = emu(20, 2);
        e.feed(b"\x1b[9mS\x1b[0m\x1b[4:3;58;2;1;2;3mC\x1b[0m\x1b[4mU\x1b[0m");
        let line = e.line(0);
        assert!(line[0].strikethrough && !line[0].underline);
        assert!(line[1].underline && line[1].undercurl);
        assert_eq!(line[1].underline_color, Some(CellColor::Rgb(1, 2, 3)));
        assert!(line[2].underline && !line[2].undercurl);
        assert_eq!(line[2].underline_color, None);
        assert!(!line[3].strikethrough && !line[3].underline);
    }

    #[test]
    fn inverse_and_hidden_resolve_in_display_colors() {
        let mut e = emu(10, 2);
        e.feed(b"\x1b[7mI\x1b[0m\x1b[8mH");
        let inv = e.line(0)[0].clone();
        assert!(inv.inverse);
        assert_eq!(
            inv.display_colors(),
            (CellColor::Background, CellColor::Foreground)
        );
        let hid = e.line(0)[1].clone();
        assert!(hid.hidden);
        let (fg, bg) = hid.display_colors();
        assert_eq!(fg, bg, "hidden text paints foreground as background");
    }

    /// Ghostty keeps an erase's background in the cell, not in a style.
    #[test]
    fn an_erase_with_a_background_paints_the_erased_cells() {
        let mut e = emu(10, 2);
        e.feed(b"ab\x1b[44m\x1b[K\x1b[48;2;1;2;3m\r\n\x1b[K");
        assert_eq!(e.line(0)[1].bg, CellColor::Background);
        assert_eq!(e.line(0)[5].bg, CellColor::Indexed(4));
        assert_eq!(e.line(1)[0].bg, CellColor::Rgb(1, 2, 3));
    }

    #[test]
    fn a_cell_keeps_its_whole_grapheme_cluster() {
        let mut e = emu(20, 2);
        e.feed("e\u{301}x🇺🇦👍🏽".as_bytes());
        let line = e.line(0);
        assert_eq!(
            (line[0].ch, line[0].cluster.as_deref()),
            ('e', Some("e\u{301}"))
        );
        assert_eq!(line[1].cluster, None);
        assert_eq!(line[2].cluster.as_deref(), Some("🇺🇦"));
        assert_eq!(line[4].cluster.as_deref(), Some("👍🏽"));
    }

    /// The dirty-row cache must not serve a stale row, a stale selection, or
    /// rows of the old width after a resize.
    #[test]
    fn a_reread_takes_changed_rows_and_the_selection_afresh() {
        let mut e = emu(10, 3);
        e.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(e.row_text(1), "two");
        e.feed(b"\x1b[2;1HTWO");
        assert_eq!((e.row_text(0), e.row_text(1)), ("one".into(), "TWO".into()));
        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.update_selection(e.grid_point(0, 2), Side::Right);
        assert!(e.line(0)[1].selected);
        e.clear_selection();
        assert!(!e.line(0)[1].selected);
        e.resize(6, 3);
        assert_eq!(e.line(0).len(), 6);
    }

    #[test]
    fn cursor_addressing_and_relative_moves() {
        let mut e = emu(20, 6);
        e.feed(b"\x1b[3;5Hx");
        // CSI H is 1-based; cell written at row 2, col 4; cursor advanced by 1.
        assert_eq!(e.line(2)[4].ch, 'x');
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 2, col: 5 }));
        e.feed(b"\x1b[2D"); // left twice
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 2, col: 3 }));
        e.feed(b"\x1b[A"); // up
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 1, col: 3 }));
    }

    #[test]
    fn clear_screen_and_home() {
        let mut e = emu(20, 4);
        e.feed(b"aaa\r\nbbb\r\nccc");
        e.feed(b"\x1b[2J\x1b[H");
        for row in 0..4 {
            assert_eq!(e.row_text(row), "");
        }
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 0, col: 0 }));
        e.feed(b"fresh");
        assert_eq!(e.row_text(0), "fresh");
    }

    #[test]
    fn erase_line_variants() {
        let mut e = emu(20, 2);
        e.feed(b"abcdef\x1b[3D\x1b[K"); // erase from cursor (col 3) to end
        assert_eq!(e.row_text(0), "abc");
    }

    #[test]
    fn scrollback_history_and_scrolling() {
        let mut e = emu(10, 3);
        for i in 1..=8 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        // Viewport shows the tail (line7, line8, then the blank prompt row).
        assert_eq!(e.row_text(0), "line7");
        assert_eq!(e.history_lines(), 6);
        assert_eq!(e.display_offset(), 0);
        // Scroll up into history.
        e.scroll(2);
        assert_eq!(e.display_offset(), 2);
        assert_eq!(e.row_text(0), "line5");
        // Cursor is below the viewport while scrolled back.
        assert_eq!(e.cursor(), None);
        // Over-scroll clamps to the top of history.
        e.scroll(100);
        assert_eq!(e.display_offset(), 6);
        assert_eq!(e.row_text(0), "line1");
        e.scroll_to_offset(3);
        assert_eq!(e.display_offset(), 3);
        assert_eq!(e.row_text(0), "line4");
        e.scroll_to_offset(usize::MAX);
        assert_eq!(e.display_offset(), 6);
        e.scroll_to_bottom();
        assert_eq!(e.display_offset(), 0);
        assert_eq!(e.row_text(0), "line7");
    }

    /// Scrolled back one row, the cursor sits one row lower in the viewport.
    #[test]
    fn the_cursor_moves_down_with_a_partial_scroll() {
        let mut e = emu(10, 3);
        for i in 1..=5 {
            e.feed(format!("\r\nl{i}").as_bytes());
        }
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 2, col: 2 }));
        e.scroll(1);
        assert_eq!(e.cursor(), None);
        e.feed(b"\x1b[2;1H");
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 2, col: 0 }));
    }

    #[test]
    fn history_is_capped_in_lines() {
        let mut e = emu(80, 24);
        for i in 0..SCROLLBACK_LINES + 3_000 {
            e.feed(format!("{i}\r\n").as_bytes());
        }
        // Ghostty prunes whole pages, so the cap is a ceiling a page's worth
        // of lines may fall short of.
        let kept = e.history_lines();
        assert!(
            (SCROLLBACK_LINES * 9 / 10..=SCROLLBACK_LINES).contains(&kept),
            "{kept}"
        );
    }

    #[test]
    fn alt_screen_restores_primary_content() {
        let mut e = emu(20, 4);
        e.feed(b"primary");
        // Enter the alt screen; 1049 keeps the cursor position, so home first.
        e.feed(b"\x1b[?1049h\x1b[H");
        e.feed(b"alt-content");
        assert_eq!(e.row_text(0), "alt-content");
        e.feed(b"\x1b[?1049l"); // leave
        assert_eq!(e.row_text(0), "primary");
    }

    #[test]
    fn dsr_cursor_report_produces_pty_response() {
        let mut e = emu(20, 4);
        e.feed(b"\x1b[2;3H");
        let responses = e.feed(b"\x1b[6n");
        assert_eq!(String::from_utf8_lossy(&responses), "\x1b[2;3R");
    }

    #[test]
    fn osc_title_and_bell() {
        let mut e = emu(20, 2);
        assert_eq!(e.title(), None);
        e.feed(b"\x1b]0;my title\x07");
        assert_eq!(e.title(), Some("my title"));
        assert!(!e.take_bell());
        e.feed(b"\x07");
        assert!(e.take_bell());
        assert!(!e.take_bell(), "bell reads clear it");
    }

    #[test]
    fn app_cursor_and_bracketed_paste_modes_toggle() {
        let mut e = emu(10, 2);
        assert!(!e.app_cursor_mode());
        e.feed(b"\x1b[?1h");
        assert!(e.app_cursor_mode());
        e.feed(b"\x1b[?1l");
        assert!(!e.app_cursor_mode());
        e.feed(b"\x1b[?2004h");
        assert!(e.bracketed_paste_mode());
    }

    #[test]
    fn kitty_keyboard_flags_follow_the_program() {
        let mut e = emu(10, 2);
        assert!(e.kitty_keyboard_flags().is_empty());
        e.feed(b"\x1b[>1u");
        assert!(
            e.kitty_keyboard_flags()
                .contains(KittyKeyFlags::DISAMBIGUATE)
        );
        e.feed(b"\x1b[<u");
        assert!(e.kitty_keyboard_flags().is_empty());
    }

    #[test]
    fn hidden_cursor_mode() {
        let mut e = emu(10, 2);
        e.feed(b"\x1b[?25l");
        assert_eq!(e.cursor(), None);
        e.feed(b"\x1b[?25h");
        assert!(e.cursor().is_some());
    }

    #[test]
    fn resize_preserves_content_and_reflows_cursor() {
        let mut e = emu(20, 5);
        e.feed(b"keepme\r\nsecond");
        e.resize(30, 3);
        assert_eq!(e.cols(), 30);
        assert_eq!(e.rows(), 3);
        assert_eq!(e.row_text(0), "keepme");
        assert_eq!(e.row_text(1), "second");
    }

    /// A local shell does not repaint its history on SIGWINCH, so a resize
    /// rewraps it (unlike Shepherd's herdr panes, which skip the reflow).
    #[test]
    fn resize_rewraps_soft_wrapped_lines() {
        let mut e = emu(4, 4);
        e.feed(b"abcdef");
        assert_eq!((e.row_text(0), e.row_text(1)), ("abcd".into(), "ef".into()));
        e.resize(8, 4);
        assert_eq!(e.row_text(0), "abcdef");
    }

    #[test]
    fn wide_chars_occupy_two_cells_with_spacer() {
        let mut e = emu(10, 2);
        e.feed("宽w".as_bytes());
        let line = e.line(0);
        assert!(line[0].wide);
        assert_eq!(line[0].ch, '宽');
        assert!(line[1].wide_spacer);
        assert_eq!(line[2].ch, 'w');
        assert_eq!(e.row_text(0), "宽w");
        assert_eq!(e.cursor(), Some(CursorSnapshot { row: 0, col: 3 }));
    }

    /// Viewport row → screen row, which is the translation every selection
    /// anchor goes through: scrolled back, the same viewport row names a line
    /// further up history.
    #[test]
    fn grid_point_offsets_by_the_scrollback_position() {
        let mut e = emu(10, 3);
        for i in 1..=8 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(e.grid_point(0, 2), GridPoint { row: 6, col: 2 });
        e.scroll(4);
        assert_eq!(e.grid_point(0, 2), GridPoint { row: 2, col: 2 });
        // Columns clamp into the grid so an over-wide pointer cannot anchor
        // outside it.
        assert_eq!(e.grid_point(0, 99).col, 9);
    }

    #[test]
    fn simple_selection_yields_its_text_and_marks_its_cells() {
        let mut e = emu(20, 3);
        e.feed(b"hello world");
        assert!(!e.has_selection());
        assert_eq!(e.selection_text(), None);

        // Drag across "hello".
        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.update_selection(e.grid_point(0, 4), Side::Right);
        assert!(e.has_selection());
        assert_eq!(e.selection_text().as_deref(), Some("hello"));

        let line = e.line(0);
        assert!(line[..5].iter().all(|c| c.selected));
        assert!(!line[5].selected, "the space past the drag is not selected");

        e.clear_selection();
        assert!(!e.has_selection());
        assert!(e.line(0).iter().all(|c| !c.selected));
    }

    /// Double-click granularity: the anchor expands to the whole word without
    /// the caller computing any boundaries.
    #[test]
    fn semantic_selection_expands_to_the_word() {
        let mut e = emu(30, 2);
        e.feed(b"alpha beta gamma");
        e.start_selection(SelectionType::Semantic, e.grid_point(0, 7), Side::Left);
        assert_eq!(e.selection_text().as_deref(), Some("beta"));
    }

    /// Triple-click granularity. The trailing newline is part of the copy —
    /// pasting a line-selection should reproduce the line break, the way it
    /// does in every other terminal.
    #[test]
    fn line_selection_takes_the_whole_row() {
        let mut e = emu(30, 3);
        e.feed(b"first row\r\nsecond row");
        e.start_selection(SelectionType::Lines, e.grid_point(1, 3), Side::Left);
        assert_eq!(e.selection_text().as_deref(), Some("second row\n"));
    }

    /// A selection made across a line break keeps the newline, so pasting the
    /// copy reproduces the rows.
    #[test]
    fn selection_spans_rows_with_a_newline() {
        let mut e = emu(10, 3);
        e.feed(b"ab\r\ncd");
        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.update_selection(e.grid_point(1, 1), Side::Right);
        assert_eq!(e.selection_text().as_deref(), Some("ab\ncd"));
    }

    /// The reason anchors live in screen space: output that scrolls the grid
    /// must carry the selection with its text, not leave it pinned to a
    /// screen row.
    #[test]
    fn selection_follows_its_text_when_output_scrolls() {
        let mut e = emu(10, 3);
        e.feed(b"target\r\n");
        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.update_selection(e.grid_point(0, 5), Side::Right);
        assert_eq!(e.selection_text().as_deref(), Some("target"));
        // Push it up the screen; the text is unchanged, so the copy is too.
        e.feed(b"a\r\nb\r\nc\r\n");
        assert_eq!(e.selection_text().as_deref(), Some("target"));
    }

    /// Output arriving mid-drag scrolls the anchor's text up; the next pointer
    /// move must extend from where that text is now.
    #[test]
    fn a_drag_keeps_its_anchor_on_its_text_while_output_scrolls() {
        let mut e = emu(10, 3);
        e.feed(b"one\r\ntwo\r\n");
        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.feed(b"three\r\nfour\r\n");
        // "one" is in history now; the pointer is on the viewport's last text row.
        e.update_selection(e.grid_point(1, 3), Side::Right);
        assert_eq!(e.selection_text().as_deref(), Some("one\ntwo\nthree\nfour"));
    }

    /// A drag scrolled into history copies from history.
    #[test]
    fn a_selection_reaches_into_history() {
        let mut e = emu(10, 2);
        for i in 1..=6 {
            e.feed(format!("line{i}\r\n").as_bytes());
        }
        e.start_selection(SelectionType::Simple, e.grid_point(0, 0), Side::Left);
        e.scroll(3);
        e.update_selection(e.grid_point(0, 0), Side::Left);
        assert_eq!(e.selection_text().as_deref(), Some("line3\nline4\nline5"));
        assert!(e.line(0)[0].selected, "the history row shows the wash");
    }

    /// A click with no drag selects nothing, and must not report a selection —
    /// otherwise the copy action fires on every bare click.
    #[test]
    fn a_click_without_a_drag_selects_nothing() {
        let mut e = emu(20, 2);
        e.feed(b"hello");
        e.start_selection(SelectionType::Simple, e.grid_point(0, 2), Side::Left);
        assert_eq!(e.selection_text(), None);
        assert!(!e.has_selection());
    }

    #[test]
    fn utf8_split_across_feeds_reassembles() {
        let mut e = emu(10, 2);
        let bytes = "é".as_bytes();
        e.feed(&bytes[..1]);
        e.feed(&bytes[1..]);
        assert_eq!(e.row_text(0), "é");
    }
}
