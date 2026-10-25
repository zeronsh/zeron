//! MessageRail (feature-inventory §1.8): a left vertical minimap of the user's
//! prompts — at most [`RAIL_MAX_TICKS`] ticks, each lit while its prompts are
//! on screen. Hovering it swaps it for a list of every prompt with the opening
//! of its reply ([`RailPopover`]); clicking one smooth-scrolls the transcript
//! to it and the list stays open. Hidden below a 48rem container width.
//!
//! Pure logic (tick extraction, active detection, width gate, previews) lives
//! in free functions with unit tests; rendering is an `impl Transcript`
//! extension since the rail shares the transcript's rows and `ListState`.

use gpui::{
    AnyElement, Bounds, Context, ListAlignment, ListOffset, ListState, MouseMoveEvent, Pixels,
    SharedString, Window, canvas, div, list, prelude::*, px,
};
use std::{
    cell::Cell,
    ops::Range,
    rc::Rc,
    time::{Duration, Instant},
};

use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};

use crate::motion;
use crate::popover;
use crate::theme::Theme;
use crate::transcript::Transcript;

/// 48rem — the container width below which the rail (and wide gutters) collapse.
pub const RAIL_MIN_CONTAINER_WIDTH: f32 = 768.0;

pub fn rail_visible(container_width: f32) -> bool {
    container_width >= RAIL_MIN_CONTAINER_WIDTH
}

/// The most ticks the rail draws. Past it each tick stands for an even slice
/// of the conversation; the hover list still reaches every prompt.
pub const RAIL_MAX_TICKS: usize = 10;
/// Lines a reply keeps in the hover list; its prompt keeps one.
pub const REPLY_PREVIEW_LINES: usize = 3;
/// The hover list sits in the gutter left of the reading column when that
/// has room for it, never narrower than [`POPOVER_MIN_WIDTH`].
pub const POPOVER_MAX_WIDTH: f32 = 320.0;
pub const POPOVER_MIN_WIDTH: f32 = 260.0;
/// The hover list's height cap, as a share of the band it opens in.
pub const POPOVER_HEIGHT_SHARE: f32 = 0.75;
/// How long the pointer rests on the rail before the list opens, so passing
/// over it (on the way to the sidebar) doesn't flash the list.
pub const RAIL_HOVER_DELAY: Duration = Duration::from_millis(150);
/// Characters kept per preview before layout: well past three lines at the
/// popover's width, so a long reply never lays out in full.
pub const PREVIEW_CHARS: usize = 360;
/// The shortest an item can be (a one-line prompt, no reply). A list with
/// more items than fit the popover at this size must overflow it, so it
/// virtualizes at full height; a shorter one lays out whole and hugs its
/// content.
pub const POPOVER_MIN_ITEM_HEIGHT: f32 = 36.0;

/// One rail tick: a user prompt and the opening of the reply that followed.
#[derive(Debug, Clone, PartialEq)]
pub struct RailTick {
    /// Message id — equals the user row's id in the transcript row model.
    pub message_id: String,
    pub prompt: String,
    pub reply: Option<String>,
}

fn user_text(entry: &SessionMessageEntry) -> String {
    let raw = entry
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    // Attachment refs ride the message text — the rail shows the visible
    // prompt, or "Attached image(s)" for image-only sends
    // (message-attachments.ts `userMessageRailText`).
    crate::attachments::user_message_rail_text(&raw)
}

fn first_reply_text(entries: &[SessionMessageEntry]) -> Option<String> {
    entries
        .iter()
        .find(|e| e.role == MessageRole::Assistant)
        .and_then(|entry| {
            entry.parts.iter().find_map(|part| match part {
                MessagePart::Text { text, .. } if !text.trim().is_empty() => {
                    Some(text.trim().to_string())
                }
                _ => None,
            })
        })
}

/// Extract rail ticks from the transcript: one per user entry (doc entries
/// first, then unconfirmed echoes — matching transcript row order). Each tick
/// carries the opening of the assistant reply that followed it, for the hover
/// preview card.
pub fn rail_ticks(
    entries: &[SessionMessageEntry],
    echoes: &[SessionMessageEntry],
) -> Vec<RailTick> {
    let mut ticks: Vec<RailTick> = Vec::new();
    for (ix, entry) in entries.iter().enumerate() {
        if entry.role != MessageRole::User {
            continue;
        }
        ticks.push(RailTick {
            message_id: entry.id.clone(),
            prompt: user_text(entry),
            reply: first_reply_text(&entries[ix + 1..]),
        });
    }
    for echo in echoes {
        if echo.role == MessageRole::User && !ticks.iter().any(|t| t.message_id == echo.id) {
            ticks.push(RailTick {
                message_id: echo.id.clone(),
                prompt: user_text(echo),
                reply: None,
            });
        }
    }
    ticks
}

/// The active tick for a scroll position: the last tick whose transcript row is
/// at or above the viewport-top row (the prompt whose section you're reading).
/// Before the first tick's row, the first tick is active.
pub fn active_tick(tick_rows: &[usize], top_row: usize) -> Option<usize> {
    if tick_rows.is_empty() {
        return None;
    }
    match tick_rows.iter().rposition(|&row| row <= top_row) {
        Some(ix) => Some(ix),
        None => Some(0),
    }
}

// ---------------------------------------------------------------------------
// Continuous prompt ring (NeverWrite ChatPromptRing)
// ---------------------------------------------------------------------------

/// Natural spacing between adjacent ticks while the stack fits.
pub const TICK_SPACING: f32 = 8.0;
/// One tick's hit-row height: the stack pads half of it above the first tick
/// and below the last so the end ticks stay as easy to hit as the rest.
pub const TICK_SLOT: f32 = 10.0;
/// Vertical room kept clear of the stack (NeverWrite `100vh - 18rem`), so a
/// long conversation compresses its ticks instead of overflowing.
pub const RAIL_RESERVED_V: f32 = 288.0;
/// The rail's hit strip width (the widest tick plus breathing room).
pub const RAIL_HIT_WIDTH: f32 = 32.0;

/// The tick stack's height: one [`TICK_SPACING`] per gap while it fits, then
/// every prompt shares the available height — every prompt keeps its own tick
/// at any conversation length.
pub fn rail_height(n: usize, viewport_height: f32) -> f32 {
    let natural = n.saturating_sub(1) as f32 * TICK_SPACING;
    let available = (viewport_height - RAIL_RESERVED_V).max(TICK_SPACING);
    natural.min(available)
}

/// Tick `ix`'s offset from the stack's first tick: evenly spread over
/// `height`.
pub fn tick_offset(ix: usize, n: usize, height: f32) -> f32 {
    if n <= 1 {
        0.0
    } else {
        ix.min(n - 1) as f32 / (n - 1) as f32 * height
    }
}

/// The hover list's width: what the gutter left of the reading column fits
/// (it opens 8px in from the transcript's edge and keeps 12px clear of the
/// column), within [`POPOVER_MIN_WIDTH`]..=[`POPOVER_MAX_WIDTH`].
pub fn popover_width(column_left: f32) -> f32 {
    (column_left - 20.0).clamp(POPOVER_MIN_WIDTH, POPOVER_MAX_WIDTH)
}

/// The prompts each tick stands for: one tick per prompt up to `max`, then
/// `max` ticks over even slices of the conversation.
pub fn tick_spans(n: usize, max: usize) -> Vec<Range<usize>> {
    let ticks = n.min(max);
    (0..ticks)
        .map(|k| k * n / ticks..(k + 1) * n / ticks)
        .collect()
}

/// A reply's opening as plain text: the hover list shows prose, not the
/// Markdown markup (`**`, backticks, link targets) it was written in.
pub fn plain_preview(markdown: &str, max_chars: usize) -> String {
    use pulldown_cmark::{Event, Parser, TagEnd};
    let mut text = String::new();
    for event in Parser::new(markdown) {
        match event {
            Event::Text(part) | Event::Code(part) => text.push_str(&part),
            Event::SoftBreak
            | Event::HardBreak
            | Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::CodeBlock
                | TagEnd::TableCell,
            ) => text.push(' '),
            _ => {}
        }
        // Enough for the cap even after whitespace collapses.
        if text.len() > max_chars * 4 {
            break;
        }
    }
    truncate_preview(&text, max_chars)
}

/// Char-cap a preview with an ellipsis. Whitespace runs (including newlines —
/// prompts and replies are free text) collapse to single spaces first: the
/// hover list wraps each preview to at most [`POPOVER_PREVIEW_LINES`].
pub fn truncate_preview(text: &str, max_chars: usize) -> String {
    let flat = crate::transcript::single_line(text);
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// One prompt in the hover list.
#[derive(Debug, Clone, PartialEq)]
pub struct RailItem {
    pub message_id: SharedString,
    pub prompt: SharedString,
    pub reply: Option<SharedString>,
}

impl RailItem {
    fn new(tick: &RailTick) -> Self {
        Self {
            message_id: tick.message_id.clone().into(),
            prompt: truncate_preview(&tick.prompt, PREVIEW_CHARS).into(),
            reply: tick
                .reply
                .as_deref()
                .map(|reply| plain_preview(reply, PREVIEW_CHARS))
                .filter(|reply| !reply.is_empty())
                .map(SharedString::from),
        }
    }
}

/// The list of every prompt that replaces the rail while the pointer is on
/// it. Virtualized once it can't fit: only the rows in view lay out, however
/// long the chat.
pub struct RailPopover {
    list: ListState,
    /// Scroll position of a list short enough to lay out whole.
    scroll: gpui::ScrollHandle,
    items: Vec<RailItem>,
    /// Per item: its prompt is on screen (or being read) — highlighted.
    lit: Vec<bool>,
    /// Scheduled bringing the prompt being read into view, once, on opening.
    revealed: bool,
    /// The card's bounds (painted last frame): the pointer leaving them
    /// closes the list.
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl Default for RailPopover {
    fn default() -> Self {
        Self {
            // Bottom-anchored: the latest prompts show first, where reading
            // usually is.
            list: ListState::new(0, ListAlignment::Bottom, px(240.0)),
            scroll: gpui::ScrollHandle::new(),
            items: Vec::new(),
            lit: Vec::new(),
            revealed: false,
            bounds: Rc::new(Cell::new(None)),
        }
    }
}

impl RailPopover {
    /// Adopt this frame's items, remeasuring only rows whose text changed
    /// (a streaming reply, a new prompt) so the list keeps its scroll.
    fn sync(&mut self, items: Vec<RailItem>) {
        let old = self.items.len();
        if items.len() == old {
            for (ix, (before, after)) in self.items.iter().zip(&items).enumerate() {
                if before != after {
                    self.list.splice(ix..ix + 1, 1);
                }
            }
        } else {
            let start = self
                .items
                .iter()
                .zip(&items)
                .position(|(before, after)| before != after)
                .unwrap_or(old.min(items.len()));
            self.list.splice(start..old, items.len() - start);
        }
        self.items = items;
    }

    pub fn item(&self, ix: usize) -> Option<(&RailItem, bool)> {
        Some((
            self.items.get(ix)?,
            self.lit.get(ix).copied().unwrap_or(false),
        ))
    }
}

// ---------------------------------------------------------------------------
// Duration-based glide timeline (pure)
// ---------------------------------------------------------------------------

/// Duration-based scroll glide (browser smooth-scroll parity: the Electron
/// rail used `scrollToItem({behavior:"smooth"})` — a fixed-duration gentle
/// ease over the WHOLE distance, never percent-of-remaining).
///
/// Rows above the viewport are unmeasured, so the total pixel distance can
/// only be ESTIMATED per frame. The timeline therefore hands out each frame's
/// movement as a fraction of whatever distance currently remains:
/// `(e_now − e_prev) / (1 − e_prev)` for eased progress `e`. With a stable
/// estimate this telescopes to exactly `start + e(t)·total` — the fixed eased
/// timeline. When the estimate changes mid-flight (a row got measured, the
/// bottom-aligned layout re-glued an anchor), the SAME timeline simply
/// continues over the corrected remainder — no restart, no compensating jump.
#[derive(Debug, Clone)]
pub struct GlideTimeline {
    eased_prev: f32,
}

impl Default for GlideTimeline {
    fn default() -> Self {
        Self::new()
    }
}

impl GlideTimeline {
    pub fn new() -> Self {
        Self { eased_prev: 0.0 }
    }

    /// Fraction of the CURRENT remaining distance to consume for eased
    /// progress `eased` (monotone, `0..=1`; `1.0` lands exactly).
    pub fn step(&mut self, eased: f32) -> f32 {
        let eased = eased.clamp(self.eased_prev, 1.0);
        let denom = 1.0 - self.eased_prev;
        let frac = if denom <= 1e-6 {
            1.0
        } else {
            (eased - self.eased_prev) / denom
        };
        self.eased_prev = eased;
        frac.clamp(0.0, 1.0)
    }
}

/// `ZERON_SCROLL_TRACE=1` logs per-frame glide positions at `warn` level —
/// the smoothness measurement knob (same family as `ZERON_FRAME_STATS`).
fn scroll_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("ZERON_SCROLL_TRACE").is_ok_and(|v| !v.is_empty() && v != "0")
    })
}

// ---------------------------------------------------------------------------
// Rendering + smooth scroll (Transcript extension)
// ---------------------------------------------------------------------------

impl Transcript {
    /// Smooth-scroll the list so `target` rests at the own-send reading line
    /// (just below the titlebar overlaying the list — landing at the raw
    /// viewport top parked the prompt under the chrome blur), reusing the
    /// transcript scroll-task slot (any running stick/jump animation yields).
    ///
    /// A [`motion::SCROLL_GLIDE`] (500ms ease-in-out) timeline drives every
    /// frame's position; per-frame movement comes from the timeline, never
    /// from a percent of the remaining distance:
    ///
    /// - a glued bottom anchor (`item_ix == len`, one viewport BELOW the
    ///   visible top) is first materialized as the true viewport-top anchor —
    ///   stepping straight from the glued anchor lands inside the re-glue band
    ///   and layout undoes it every frame (the old stall→double-jump path);
    /// - rows above the viewport are unmeasured, so the anchor glides in item
    ///   space along the same timeline, estimating sub-row offsets from a
    ///   local row-height EMA; the position is read back each frame, so a
    ///   measurement correcting the estimate just re-enters the timeline;
    /// - once the target row is measured the glide is pixel-exact.
    pub fn scroll_to_row(&mut self, target: usize, cx: &mut Context<Self>) {
        self.begin_scroll_navigation();
        // Row 0 carries the chrome inside its own top gap; every other row
        // lands `inset` below the viewport top, like a freshly-sent prompt.
        let inset = Self::own_send_inset(target);
        let landing = ListOffset {
            item_ix: target,
            offset_in_item: px(-inset),
        };
        if motion::reduced_motion(cx) {
            self.list_state().scroll_to(landing);
            cx.notify();
            return;
        }
        self.set_scroll_task(cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let total = motion::SCROLL_GLIDE.total().mul_f32(motion::speed_scale());
            let mut timeline = GlideTimeline::new();
            let mut height_ema: Option<f32> = None;
            let trace = scroll_trace_enabled();
            let frames = (total.as_millis() / 16) as usize + 90;
            for _ in 0..frames {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let raw = (started.elapsed().as_secs_f32() / total.as_secs_f32()).min(1.0);
                let eased = motion::SCROLL_GLIDE.curve.eval(raw);
                let frac = timeline.step(eased);
                let done = this.update(cx, |t, cx| {
                    let list = t.list_state().clone();
                    if raw >= 1.0 {
                        list.scroll_to(landing);
                        cx.notify();
                        return true;
                    }
                    // Materialize the glued bottom representation as the true
                    // top anchor (same visual position, sticky anchor).
                    let viewport = f32::from(list.viewport_bounds().size.height);
                    if t.is_glued() && viewport > 0.0 {
                        list.scroll_by(px(-(viewport + 0.5)));
                    }
                    let top = list.logical_scroll_top();
                    let top_height = list
                        .bounds_for_item(top.item_ix)
                        .map(|b| f32::from(b.size.height).max(1.0));
                    // Row-height estimate for unmeasured territory: the mean
                    // over the whole visible span, recomputed per frame (the
                    // ~dozen mixed row kinds in a viewport average out — a
                    // single-row estimate whipsaws between paragraphs and
                    // code blocks and modulates the per-frame step visibly).
                    if viewport > 0.0 {
                        let bottom = f32::from(list.viewport_bounds().bottom());
                        let mut ix = top.item_ix;
                        let mut count = 0.0f32;
                        while let Some(b) = list.bounds_for_item(ix) {
                            if f32::from(b.top()) >= bottom {
                                break;
                            }
                            count += 1.0;
                            ix += 1;
                        }
                        if count > 0.0 {
                            let mean = viewport / count;
                            let ema = height_ema.get_or_insert(mean);
                            *ema += 0.5 * (mean - *ema);
                        }
                    }
                    if height_ema.is_none() {
                        height_ema = top_height;
                    }
                    // Where the viewport top actually is, in fractional item
                    // space — read back per frame (self-correcting: an anchor
                    // the layout adjusted or re-glued keeps its real remaining
                    // distance and continues the same timeline).
                    let here = top.item_ix as f32
                        + top_height
                            .map(|h| (f32::from(top.offset_in_item) / h).clamp(0.0, 1.0))
                            .unwrap_or(0.0);
                    if trace {
                        tracing::warn!(
                            ms = started.elapsed().as_millis() as u64,
                            eased,
                            here,
                            dist = t.distance_from_bottom(),
                            "scroll-glide"
                        );
                    }

                    if target < top.item_ix {
                        // Above the viewport (unmeasured): progressive
                        // item-space anchoring within the eased timeline,
                        // aiming `inset` px above the target row (estimated
                        // in rows; the measured branch below takes over
                        // pixel-exactly once the target is laid out).
                        let goal = (target as f32 - height_ema.map_or(0.0, |h| inset / h.max(1.0)))
                            .max(0.0);
                        let next = here - frac * (here - goal);
                        // Small steps ride `scroll_by` — the list keeps a
                        // 320px measured leading overdraw, so a step that
                        // fits inside it crosses rows at their TRUE heights
                        // (pixel-exact frames through the gentle start and
                        // landing, where jitter would show most).
                        let step_px = (here - next) * height_ema.unwrap_or(0.0);
                        if step_px > 0.0 && step_px <= crate::transcript::OVERDRAW_PX * 0.8 {
                            list.scroll_by(px(-step_px));
                            cx.notify();
                            return false;
                        }
                        let ix = (next.floor().max(0.0) as usize).min(top.item_ix);
                        let within = next - ix as f32;
                        let offset = if ix == top.item_ix {
                            // Same row as the current anchor: measured height,
                            // pixel-exact — and never below the current offset,
                            // so motion stays monotone even when a height
                            // estimate was corrected.
                            top_height
                                .map(|h| (within * h).min(f32::from(top.offset_in_item)))
                                .unwrap_or(0.0)
                        } else {
                            within * height_ema.unwrap_or(0.0)
                        };
                        list.scroll_to(ListOffset {
                            item_ix: ix,
                            offset_in_item: px(offset),
                        });
                        cx.notify();
                        return false;
                    }
                    match list.bounds_for_item(target) {
                        Some(bounds) => {
                            // Measured: pixel-exact step along the timeline.
                            let delta =
                                f32::from(bounds.top() - list.viewport_bounds().top()) - inset;
                            list.scroll_by(px(frac * delta));
                        }
                        None => {
                            // Below but unmeasured: item space, same timeline.
                            let next = here + frac * (target as f32 - here);
                            let ix = (next.floor().max(0.0) as usize).min(target);
                            let within = next - ix as f32;
                            list.scroll_to(ListOffset {
                                item_ix: ix,
                                offset_in_item: px(within * height_ema.unwrap_or(0.0)),
                            });
                        }
                    }
                    cx.notify();
                    false
                });
                match done {
                    Ok(true) | Err(_) => return,
                    Ok(false) => {}
                }
            }
            // Timeline exhausted (shouldn't happen): land exactly.
            this.update(cx, |t, cx| {
                t.list_state().scroll_to(landing);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Row `row`'s top and bottom in window coordinates, `None` while it's
    /// unmeasured. Glued to the bottom, the list reports no row bounds (its
    /// anchor sits past the last row), but the content's end rests on the
    /// viewport's bottom, so measured rows place from there.
    fn row_span(&self, row: usize) -> Option<(f32, f32)> {
        let list = self.list_state();
        if let Some(bounds) = list.bounds_for_item(row) {
            return Some((f32::from(bounds.top()), f32::from(bounds.bottom())));
        }
        if !self.is_glued() {
            return None;
        }
        let end = list.offset_for_item(list.item_count());
        let bottom = f32::from(list.viewport_bounds().bottom());
        let top = bottom - f32::from(end - list.offset_for_item(row));
        let row_bottom = bottom - f32::from(end - list.offset_for_item(row + 1));
        (row_bottom > top).then_some((top, row_bottom))
    }

    /// The rail element — an absolute overlay along the transcript's left
    /// edge: the tick minimap, or the prompt list while the pointer is on it.
    pub fn render_rail(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.rail_enabled() {
            *self.rail_popover_mut() = None;
            return gpui::Empty.into_any_element();
        }
        let (entries, echoes) = {
            let state = self.state_entity().read(cx);
            (state.transcript.clone(), state.pending_echoes().to_vec())
        };
        let ticks = rail_ticks(&entries, &echoes);
        // Map each tick to its transcript row (user rows share the entry id).
        let pairs: Vec<(RailTick, usize)> = ticks
            .into_iter()
            .filter_map(|tick| {
                let row = self
                    .rows()
                    .iter()
                    .position(|r| r.id.as_ref() == tick.message_id.as_str())?;
                Some((tick, row))
            })
            .collect();
        // A minimap of one exchange is noise, not navigation — the original
        // rail hides below two marks (message-rail.tsx `marks.length < 2`).
        if pairs.len() < 2 {
            *self.rail_popover_mut() = None;
            return gpui::Empty.into_any_element();
        }
        let tick_rows: Vec<usize> = pairs.iter().map(|(_, row)| *row).collect();
        // Active detection reads from the READING line, not the raw clip top:
        // the titlebar overlays the list, so a row whose top sits within that
        // chrome band is what you're reading — the sliver of the previous row
        // above it is behind the blur. Concretely, the own-turn hold parks the
        // newest prompt exactly at the chrome inset, and crediting the row at
        // the raw clip top kept the PREVIOUS tick lit for the whole runway
        // (user report). Walk forward over measured rows whose tops are at or
        // above the reading line; unmeasured rows (None bounds) stop the walk,
        // leaving the raw top row — the pre-fix behavior.
        let read_top = f32::from(self.list_state().viewport_bounds().top())
            + crate::transcript::OWN_SEND_TOP_INSET_PX
            + 0.5;
        let top_row = if self.is_glued() {
            // Glued to the bottom the list reports no row bounds; place them
            // from the end instead.
            (0..self.rows().len())
                .rev()
                .find(|&row| self.row_span(row).is_some_and(|(top, _)| top <= read_top))
                .unwrap_or(0)
        } else {
            let mut top_row = self.list_state().logical_scroll_top().item_ix;
            while let Some(bounds) = self.list_state().bounds_for_item(top_row + 1) {
                if f32::from(bounds.top()) <= read_top {
                    top_row += 1;
                } else {
                    break;
                }
            }
            top_row
        };
        let active = active_tick(&tick_rows, top_row);
        // Every prompt whose row shows between the titlebar and the bottom
        // chrome is lit; with none on screen (a long reply), the prompt being
        // read stays lit.
        let viewport = self.list_state().viewport_bounds();
        let view_top = f32::from(viewport.top()) + crate::transcript::OWN_SEND_TOP_INSET_PX;
        let view_bottom = f32::from(viewport.bottom()) - self.bottom_clearance();
        let in_view: Vec<bool> = tick_rows
            .iter()
            .map(|&row| {
                self.row_span(row)
                    .is_some_and(|(top, bottom)| bottom > view_top && top < view_bottom)
            })
            .collect();
        let any_in_view = in_view.iter().any(|&v| v);
        let lit: Vec<bool> = (0..pairs.len())
            .map(|ix| in_view[ix] || (!any_in_view && active == Some(ix)))
            .collect();
        // Rail and list share the band between the titlebar and the bottom
        // chrome, centered alike, so the list opens under the pointer.
        let band_top = crate::transcript::OWN_SEND_TOP_INSET_PX;
        let band_bottom = self.bottom_clearance();
        // Pre-layout the viewport reads 0; assume a typical height for that
        // one frame rather than collapsing the stack.
        let viewport_h = match f32::from(viewport.size.height) {
            h if h > 0.0 => h,
            _ => 600.0,
        };
        let band = div()
            .absolute()
            .left(px(16.0))
            .top(px(band_top))
            .bottom(px(band_bottom))
            .flex()
            .flex_col()
            .justify_center();

        if self.rail_popover().is_some() {
            let items: Vec<RailItem> = pairs.iter().map(|(tick, _)| RailItem::new(tick)).collect();
            let max_height =
                ((viewport_h - band_top - band_bottom - 16.0) * POPOVER_HEIGHT_SHARE).max(120.0);
            let width = popover_width(self.column_left());
            let virtualized = items.len() as f32 * POPOVER_MIN_ITEM_HEIGHT > max_height;
            let count = items.len();
            let popover = self.rail_popover_mut().as_mut().expect("checked above");
            popover.sync(items);
            popover.lit = lit;
            // Bring the prompt being read into view once rows are measured
            // (they lay out on this frame; before that nothing can place it).
            if !popover.revealed {
                popover.revealed = true;
                if let Some(active) = active {
                    cx.on_next_frame(window, move |this, _, cx| {
                        if let Some(popover) = this.rail_popover() {
                            if virtualized {
                                popover.list.scroll_to_reveal_item(active);
                            } else {
                                popover.scroll.scroll_to_item(active);
                            }
                            cx.notify();
                        }
                    });
                }
            }
            let list_state = popover.list.clone();
            let scroll = popover.scroll.clone();
            let bounds_cell = popover.bounds.clone();
            let body = if virtualized {
                list(list_state, cx.processor(Self::render_rail_item))
                    .w_full()
                    .h(px(max_height))
                    .into_any_element()
            } else {
                div()
                    .id("rail-popover-items")
                    .w_full()
                    .max_h(px(max_height))
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .flex()
                    .flex_col()
                    .children((0..count).map(|ix| self.render_rail_item(ix, window, cx)))
                    .into_any_element()
            };
            let theme = Theme::of(cx).for_popup();
            let card = popover::popover_card(&theme)
                .id("rail-popover")
                .debug_selector(|| "rail-popover".into())
                .relative()
                .w(px(width))
                .occlude()
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if !*hovered && this.rail_popover().is_some() {
                        *this.rail_popover_mut() = None;
                        cx.notify();
                    }
                }))
                .child(
                    canvas(
                        move |bounds, _, _| bounds_cell.set(Some(bounds)),
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(body);
            // Mounted straight through deferred (not a popover mount
            // helper), so the frost wrap happens here.
            return band
                .ml(px(-8.0))
                .child(gpui::deferred(crate::frost::frosted(
                    popover::CARD_RADIUS,
                    crate::frost::MENU_BLUR,
                    card,
                )))
                .into_any_element();
        }

        let theme = Theme::of(cx).clone();
        let spans = tick_spans(pairs.len(), RAIL_MAX_TICKS);
        let height = rail_height(spans.len(), viewport_h);
        let n = spans.len();
        let ticks = spans.into_iter().enumerate().map(|(ix, span)| {
            let bar_color = if span.clone().any(|prompt| lit[prompt]) {
                theme.text.opacity(0.8)
            } else {
                crate::theme::ink(0.16)
            };
            div()
                .absolute()
                .left_0()
                .top(px(tick_offset(ix, n, height)))
                .h(px(TICK_SLOT))
                .w_full()
                .flex()
                .items_center()
                .child(div().h(px(2.0)).w(px(8.0)).rounded(px(1.0)).bg(bar_color))
        });
        band.w(px(RAIL_HIT_WIDTH))
            .child(
                div()
                    .id("rail-ticks")
                    .debug_selector(|| "rail-ticks".into())
                    .relative()
                    .w_full()
                    .h(px(height + TICK_SLOT))
                    .cursor_pointer()
                    // The pointer resting on the rail swaps it for the prompt
                    // list; passing over it doesn't.
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !*hovered {
                            *this.rail_hover_intent_mut() = None;
                            return;
                        }
                        if this.rail_popover().is_some() {
                            return;
                        }
                        let open = cx.spawn(async move |this, cx| {
                            cx.background_executor().timer(RAIL_HOVER_DELAY).await;
                            this.update(cx, |this, cx| {
                                *this.rail_hover_intent_mut() = None;
                                if this.rail_popover().is_none() {
                                    *this.rail_popover_mut() = Some(RailPopover::default());
                                    cx.notify();
                                }
                            })
                            .ok();
                        });
                        *this.rail_hover_intent_mut() = Some(open);
                    }))
                    .children(ticks),
            )
            .into_any_element()
    }

    /// Close the prompt list once the pointer moves over the transcript
    /// outside it (bounds from the last paint): the card's own hover-out
    /// covers leaving it, this covers a list that never saw the pointer
    /// arrive.
    pub(crate) fn rail_pointer_moved(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let outside = self
            .rail_popover()
            .and_then(|popover| popover.bounds.get())
            .is_some_and(|bounds| !bounds.contains(&event.position));
        if outside {
            *self.rail_popover_mut() = None;
            cx.notify();
        }
    }

    /// One prompt in the hover list: the prompt on one line and the opening
    /// of its reply on up to three; a click glides the transcript to it.
    fn render_rail_item(
        &mut self,
        ix: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some((item, lit)) = self
            .rail_popover()
            .and_then(|popover| popover.item(ix))
            .map(|(item, lit)| (item.clone(), lit))
        else {
            return gpui::Empty.into_any_element();
        };
        let theme = Theme::of(cx).for_popup();
        let id = item.message_id.clone();
        div()
            .pb(px(popover::MENU_GAP))
            .child(
                div()
                    .id(("rail-item", ix))
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(popover::MENU_ITEM_RADIUS))
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .cursor_pointer()
                    .when(lit, |row| row.bg(crate::theme::ink(0.06)))
                    .hover(|row| row.bg(crate::theme::ink(0.09)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Resolved at click time: rows can splice between the
                        // render that drew the item and the click.
                        if let Some(row) = this.rows().iter().position(|r| r.id == id) {
                            this.scroll_to_row(row, cx);
                        }
                    }))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .line_height(px(18.0))
                            .text_color(theme.text)
                            .truncate()
                            .child(item.prompt),
                    )
                    .when_some(item.reply, |el, reply| {
                        el.child(
                            div()
                                .text_size(crate::typography::ui_rems(12.0))
                                .line_height(px(17.0))
                                .text_color(theme.text_muted)
                                .line_clamp(REPLY_PREVIEW_LINES)
                                .text_ellipsis()
                                .child(reply),
                        )
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::MessageStatus;

    fn entry(id: &str, role: MessageRole, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: 0,
            device_id: "d".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn rail_height_spaces_ticks_then_compresses() {
        // Natural spacing while it fits.
        assert_eq!(rail_height(2, 900.0), TICK_SPACING);
        assert_eq!(rail_height(10, 900.0), 9.0 * TICK_SPACING);
        // A long conversation shares the available height instead of growing.
        assert_eq!(rail_height(500, 900.0), 900.0 - RAIL_RESERVED_V);
        // Tiny (or unmeasured) heights still leave a usable stack.
        assert_eq!(rail_height(500, 0.0), TICK_SPACING);
        assert_eq!(rail_height(1, 900.0), 0.0);
    }

    #[test]
    fn tick_offsets_spread_evenly_end_to_end() {
        assert_eq!(tick_offset(0, 5, 40.0), 0.0);
        assert_eq!(tick_offset(2, 5, 40.0), 20.0);
        assert_eq!(tick_offset(4, 5, 40.0), 40.0);
        assert_eq!(tick_offset(0, 1, 40.0), 0.0);
    }

    /// Ten ticks at most; past ten each covers an even, gapless slice, so
    /// every prompt belongs to exactly one tick.
    #[test]
    fn ticks_cap_at_ten_and_cover_every_prompt() {
        assert!(tick_spans(0, RAIL_MAX_TICKS).is_empty());
        assert_eq!(tick_spans(3, RAIL_MAX_TICKS), vec![0..1, 1..2, 2..3]);
        assert_eq!(tick_spans(10, RAIL_MAX_TICKS).len(), 10);
        for n in [11usize, 13, 25, 99, 300] {
            let spans = tick_spans(n, RAIL_MAX_TICKS);
            assert_eq!(spans.len(), RAIL_MAX_TICKS, "n={n}");
            assert_eq!(spans.first().unwrap().start, 0);
            assert_eq!(spans.last().unwrap().end, n);
            for pair in spans.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "n={n}");
                assert!(!pair[0].is_empty());
            }
        }
    }

    /// The list fits the gutter left of the reading column when it can, and
    /// otherwise stays readable rather than shrinking to nothing.
    #[test]
    fn popover_width_fits_the_gutter_within_bounds() {
        assert_eq!(popover_width(600.0), POPOVER_MAX_WIDTH);
        assert_eq!(popover_width(300.0), 280.0);
        assert_eq!(popover_width(204.0), POPOVER_MIN_WIDTH);
        assert_eq!(popover_width(0.0), POPOVER_MIN_WIDTH);
    }

    #[test]
    fn reply_previews_drop_markdown_markup() {
        assert_eq!(
            plain_preview(
                "**Done.** Ran `cargo test`:\n\n- one\n- [two](https://x.y)",
                200
            ),
            "Done. Ran cargo test: one two"
        );
        assert_eq!(
            plain_preview("# Title\n\n```rust\nfn a() {}\n```", 200),
            "Title fn a() {}"
        );
        let long = "word ".repeat(500);
        let cut = plain_preview(&long, PREVIEW_CHARS);
        assert!(cut.chars().count() <= PREVIEW_CHARS && cut.ends_with('…'));
    }

    /// The hover list remeasures only rows whose text changed, and grows or
    /// shrinks from the first change, keeping the rest of its layout.
    #[test]
    fn popover_sync_tracks_item_changes() {
        let item = |id: &str, reply: Option<&str>| RailItem {
            message_id: id.to_owned().into(),
            prompt: format!("prompt {id}").into(),
            reply: reply.map(|r| r.to_owned().into()),
        };
        let mut popover = RailPopover::default();
        popover.sync(vec![item("a", Some("x")), item("b", None)]);
        assert_eq!(popover.list.item_count(), 2);
        // The streaming reply grows: same count.
        popover.sync(vec![item("a", Some("x")), item("b", Some("partial"))]);
        assert_eq!(popover.list.item_count(), 2);
        assert_eq!(popover.items[1].reply.as_deref(), Some("partial"));
        // A new prompt.
        popover.sync(vec![
            item("a", Some("x")),
            item("b", Some("partial")),
            item("c", None),
        ]);
        assert_eq!(popover.list.item_count(), 3);
        // A rewind drops the tail.
        popover.sync(vec![item("a", Some("x"))]);
        assert_eq!(popover.list.item_count(), 1);
    }

    #[test]
    fn ticks_map_user_prompts_with_reply_openings() {
        let entries = vec![
            entry("u1", MessageRole::User, "first question"),
            entry("a1", MessageRole::Assistant, "first answer"),
            entry("u2", MessageRole::User, "second question"),
            entry("a2", MessageRole::Assistant, "second answer"),
        ];
        let ticks = rail_ticks(&entries, &[]);
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].message_id, "u1");
        assert_eq!(ticks[0].prompt, "first question");
        assert_eq!(ticks[0].reply.as_deref(), Some("first answer"));
        assert_eq!(ticks[1].reply.as_deref(), Some("second answer"));
    }

    #[test]
    fn ticks_include_echoes_deduped() {
        let entries = vec![entry("u1", MessageRole::User, "sent")];
        let echoes = vec![
            entry("u1", MessageRole::User, "sent"), // confirmed already → deduped
            entry("u2", MessageRole::User, "pending"),
        ];
        let ticks = rail_ticks(&entries, &echoes);
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[1].message_id, "u2");
        assert_eq!(ticks[1].reply, None);
    }

    #[test]
    fn tick_without_reply_yet() {
        let entries = vec![
            entry("u1", MessageRole::User, "q"),
            entry("a1", MessageRole::Assistant, "reply to first"),
            entry("u2", MessageRole::User, "latest"),
        ];
        let ticks = rail_ticks(&entries, &[]);
        // The last prompt has no assistant entry after it.
        assert_eq!(ticks[1].reply, None);
        // Empty transcript → no ticks.
        assert!(rail_ticks(&[], &[]).is_empty());
    }

    #[test]
    fn active_tick_tracks_viewport_top() {
        let tick_rows = [0, 5, 9];
        assert_eq!(active_tick(&tick_rows, 0), Some(0));
        assert_eq!(active_tick(&tick_rows, 4), Some(0));
        assert_eq!(active_tick(&tick_rows, 5), Some(1));
        assert_eq!(active_tick(&tick_rows, 8), Some(1));
        assert_eq!(active_tick(&tick_rows, 100), Some(2));
        // Above the first tick row → first tick still active.
        assert_eq!(active_tick(&[3, 7], 1), Some(0));
        assert_eq!(active_tick(&[], 4), None);
    }

    #[test]
    fn rail_width_gate() {
        assert!(rail_visible(768.0));
        assert!(rail_visible(1200.0));
        assert!(!rail_visible(767.9));
        assert!(!rail_visible(0.0));
    }

    /// Consuming `(e'−e)/(1−e)` of the current remainder telescopes to exactly
    /// the absolute eased timeline `start + e(t)·total` when the distance
    /// estimate is stable — the glide is timeline-driven, not
    /// percent-of-remaining.
    #[test]
    fn glide_timeline_matches_absolute_eased_interpolation() {
        let curve = motion::SCROLL_GLIDE.curve;
        let mut timeline = GlideTimeline::new();
        let (start, target) = (1000.0f32, 0.0f32);
        let mut pos = start;
        for i in 1..=60 {
            let t = i as f32 / 60.0;
            let eased = curve.eval(t);
            let frac = timeline.step(eased);
            pos -= frac * (pos - target);
            let absolute = start + eased * (target - start);
            assert!(
                (pos - absolute).abs() < 0.05,
                "frame {i}: pos {pos} != absolute {absolute}"
            );
        }
        assert_eq!(pos, target); // eased hits 1.0 → frac 1.0 → exact landing.
    }

    /// A mid-flight distance re-estimate (anchor re-glued / row measured)
    /// continues the SAME timeline over the corrected remainder: no restart,
    /// no compensating jump, exact landing.
    #[test]
    fn glide_timeline_survives_remaining_distance_reestimate() {
        let curve = motion::SCROLL_GLIDE.curve;
        let mut timeline = GlideTimeline::new();
        let mut pos = 500.0f32;
        let mut prev_frac = 0.0f32;
        for i in 1..=60 {
            let t = i as f32 / 60.0;
            let frac = timeline.step(curve.eval(t));
            if i == 30 {
                // The layout re-glued the anchor: remaining distance doubles.
                pos *= 2.0;
            }
            pos -= frac * pos;
            // Fractions depend only on the timeline — the re-estimate cannot
            // make a step consume a larger share than the curve dictates.
            assert!((0.0..=1.0).contains(&frac));
            if i > 1 && i < 55 {
                assert!(frac >= prev_frac - 0.05, "frame {i}: frac regressed");
            }
            prev_frac = frac;
        }
        assert_eq!(pos, 0.0);
    }

    /// Timeline steps clamp: regressions in eased input yield zero movement,
    /// and completion always yields the full remainder.
    #[test]
    fn glide_timeline_step_clamps() {
        let mut timeline = GlideTimeline::new();
        assert_eq!(timeline.step(0.4), 0.4);
        assert_eq!(timeline.step(0.3), 0.0); // non-monotone input → no move
        assert_eq!(timeline.step(1.0), 1.0); // done → land exactly
        assert_eq!(timeline.step(1.0), 1.0); // idempotent at the end
    }

    /// The first 16ms frame of the 500ms glide covers under 2% of the
    /// distance — no first-frame majority jump by construction.
    #[test]
    fn glide_first_frame_is_gentle() {
        let spec = motion::SCROLL_GLIDE;
        assert_eq!(spec.duration_ms, 500);
        let first = spec.curve.eval(16.0 / 500.0);
        assert!(first < 0.02, "first frame covered {first} of the distance");
        // And the ease-in-out midpoint is exactly half the distance.
        let mid = spec.curve.eval(0.5);
        assert!((mid - 0.5).abs() < 0.01);
    }

    #[test]
    fn preview_truncation() {
        assert_eq!(truncate_preview("short", 10), "short");
        assert_eq!(truncate_preview("  padded  ", 10), "padded");
        let long = "x".repeat(50);
        let cut = truncate_preview(&long, 10);
        assert!(cut.chars().count() <= 10);
        assert!(cut.ends_with('…'));
        // Multi-byte safety.
        let uni = "héllo wörld attaché case overflowing";
        let cut = truncate_preview(uni, 12);
        assert!(cut.ends_with('…'));
    }
}
