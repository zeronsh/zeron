//! MessageRail (feature-inventory §1.8): a left vertical minimap of the user's
//! prompts. The active tick brightens, hover grows the tick and shows a preview
//! card (prompt + reply opening), click smooth-scrolls the transcript to that
//! row. A conversation too long for the rail's window scrolls its ticks (the
//! Codex prompt rail), following the prompts on screen. Hidden below a 48rem
//! container width.
//!
//! Pure logic (tick extraction, active detection, width gate, previews) lives
//! in free functions with unit tests; rendering is an `impl Transcript`
//! extension since the rail shares the transcript's rows and `ListState`.

use gpui::{
    AnyElement, ClickEvent, Context, ListOffset, MouseMoveEvent, Pixels, SharedString, StyledText,
    canvas, div, prelude::*, px,
};
use std::time::{Duration, Instant};

use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};

use crate::markdown::{Block, InlineRun, InlineStyle};
use crate::motion;
use crate::popover;
use crate::theme::Theme;
use crate::transcript::{AgentSender, ChatIdentity, Transcript};

/// 48rem — the container width below which the rail (and wide gutters) collapse.
pub const RAIL_MIN_CONTAINER_WIDTH: f32 = 768.0;

pub fn rail_visible(container_width: f32) -> bool {
    container_width >= RAIL_MIN_CONTAINER_WIDTH
}

/// Preview line caps: the prompt and the reply's opening clamp to these.
pub const PREVIEW_PROMPT_LINES: usize = 3;
pub const PREVIEW_REPLY_LINES: usize = 4;
/// Source a preview parses at most; the clamp shows far less of it.
const PREVIEW_SOURCE_BYTES: usize = 2048;

/// One rail tick: a user prompt and the opening of the reply that followed.
#[derive(Debug, Clone, PartialEq)]
pub struct RailTick {
    /// Message id — equals the user row's id in the transcript row model.
    pub message_id: String,
    pub prompt: String,
    /// The chat that sent the prompt through Zeron's `send_message`; its
    /// routing header is off `prompt`, as in the transcript.
    pub sender: Option<AgentSender>,
    pub reply: Option<String>,
}

fn user_text(entry: &SessionMessageEntry) -> (String, Option<AgentSender>) {
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
    let text = crate::attachments::user_message_rail_text(
        &zeron_proto::annotation::annotation_display(&raw),
    );
    match crate::transcript::agent_message_sender(&text) {
        Some((sender, body)) => (body.to_owned(), Some(sender)),
        None => (text, None),
    }
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
        let (prompt, sender) = user_text(entry);
        ticks.push(RailTick {
            message_id: entry.id.clone(),
            prompt,
            sender,
            reply: first_reply_text(&entries[ix + 1..]),
        });
    }
    for echo in echoes {
        if echo.role == MessageRole::User && !ticks.iter().any(|t| t.message_id == echo.id) {
            let (prompt, sender) = user_text(echo);
            ticks.push(RailTick {
                message_id: echo.id.clone(),
                prompt,
                sender,
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
/// The rail's tallest window, as a share of the band between the titlebar
/// and the bottom chrome, and in absolute terms (Codex `min(70vh, 40rem)`).
/// A longer conversation scrolls its ticks inside it.
pub const RAIL_HEIGHT_SHARE: f32 = 0.7;
pub const RAIL_MAX_HEIGHT: f32 = 640.0;
/// The fade at a scrolled rail's clipped ends; following keeps lit ticks
/// clear of it.
pub const RAIL_EDGE_FADE: f32 = 24.0;
/// The rail's hit strip width (the widest tick plus breathing room).
pub const RAIL_HIT_WIDTH: f32 = 32.0;

/// The tick stack's height from first tick to last: one [`TICK_SPACING`] per
/// gap at any conversation length (it scrolls rather than compresses).
pub fn rail_height(n: usize) -> f32 {
    n.saturating_sub(1) as f32 * TICK_SPACING
}

/// The rail window's height for a stack `content` tall (ticks plus their
/// slot) in a `band`-tall space: all of it while it fits, else the cap.
pub fn rail_window(content: f32, band: f32) -> f32 {
    let cap = (band * RAIL_HEIGHT_SHARE)
        .min(RAIL_MAX_HEIGHT)
        .max(TICK_SLOT * 4.0);
    content.min(cap)
}

/// The rail scroll position that brings the stack span `top..bottom` into a
/// `window`-tall view now at `scroll`, `margin` clear of either edge (the
/// fade): the least movement, the span's top winning when it can't all fit.
/// Clamped to a stack `content` tall.
pub fn follow_scroll(
    scroll: f32,
    window: f32,
    content: f32,
    top: f32,
    bottom: f32,
    margin: f32,
) -> f32 {
    let margin = margin.min(((window - (bottom - top)) / 2.0).max(0.0));
    let mut scroll = scroll;
    if bottom + margin > scroll + window {
        scroll = bottom + margin - window;
    }
    if top - margin < scroll {
        scroll = top - margin;
    }
    scroll.clamp(0.0, (content - window).max(0.0))
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

/// The prompt under pointer `y` (relative to the stack's first tick): the
/// nearest tick, clamped to the ends.
pub fn tick_at(n: usize, height: f32, y: f32) -> Option<usize> {
    if n == 0 {
        return None;
    }
    if n == 1 || height <= 0.0 {
        return Some(0);
    }
    let progress = (y / height).clamp(0.0, 1.0);
    Some(((progress * (n - 1) as f32).round() as usize).min(n - 1))
}

/// Fisheye width for a tick `distance` ticks from the hovered one.
pub fn tick_width(distance: Option<usize>) -> f32 {
    match distance {
        Some(0) => 24.0,
        Some(1) => 16.0,
        Some(2) => 10.0,
        _ => 8.0,
    }
}

/// A preview's Markdown as one run of prose: the styling stays (strong,
/// emphasis, code, links), while block structure, which the card's few lines
/// have no room for, flattens to spaces. Only prose shows (paragraphs,
/// headings, list items): a fence, quote, table or display formula would
/// spend those lines on its least telling part, so it is left out, and a
/// message of nothing else shows its first fence's opening line. Inline math
/// and images show their source.
pub fn preview_runs(source: &str) -> Vec<InlineRun> {
    let mut end = source.len().min(PREVIEW_SOURCE_BYTES);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    let tree = crate::markdown::parse_full(&source[..end]);
    let mut runs = Vec::new();
    for top in &tree.blocks {
        push_block_runs(&top.block, &mut runs);
    }
    if runs.iter().all(|run| run.text.trim().is_empty()) {
        runs = tree
            .blocks
            .iter()
            .find_map(|top| first_code_line(&top.block))
            .map(|line| InlineRun {
                text: line.to_owned(),
                style: InlineStyle {
                    code: true,
                    ..InlineStyle::default()
                },
            })
            .into_iter()
            .collect();
    }
    // Whitespace (line breaks included) collapses, like the transcript's
    // one-line titles.
    // A collapsed space between runs is its own plain run, so a code wash
    // never takes in a blank.
    let mut out: Vec<InlineRun> = Vec::with_capacity(runs.len());
    let mut space = false;
    for run in runs {
        let mut text = String::with_capacity(run.text.len());
        for c in run.text.chars() {
            if c.is_whitespace() {
                space = !out.is_empty() || !text.is_empty();
            } else {
                if space {
                    if text.is_empty() {
                        out.push(InlineRun {
                            text: " ".into(),
                            style: InlineStyle::default(),
                        });
                    } else {
                        text.push(' ');
                    }
                    space = false;
                }
                text.push(c);
            }
        }
        if !text.is_empty() {
            out.push(InlineRun {
                text,
                style: run.style,
            });
        }
    }
    out
}

fn push_block_runs(block: &Block, runs: &mut Vec<InlineRun>) {
    let inline = |run: &InlineRun, bold: bool| {
        let mut style = run.style.clone();
        style.bold |= bold;
        let text = match (&style.image, style.math) {
            (Some(image), _) => image.alt.clone(),
            (None, true) => {
                // A formula needs a painted slot; the card shows its source.
                style.math = false;
                style.code = true;
                format!("${}$", run.text)
            }
            (None, false) => run.text.clone(),
        };
        InlineRun { text, style }
    };
    match block {
        Block::Paragraph { runs: inlines } => runs.extend(inlines.iter().map(|r| inline(r, false))),
        Block::Heading { runs: inlines, .. } => {
            runs.extend(inlines.iter().map(|r| inline(r, true)))
        }
        Block::List { items, .. } => {
            for child in items.iter().flatten() {
                push_block_runs(child, runs);
            }
        }
        Block::CodeBlock { .. }
        | Block::Math { .. }
        | Block::BlockQuote { .. }
        | Block::Table { .. }
        | Block::Rule => return,
    }
    runs.push(InlineRun {
        text: " ".into(),
        style: InlineStyle::default(),
    });
}

/// The first non-blank line of the first fence in `block`, at any depth.
fn first_code_line(block: &Block) -> Option<&str> {
    match block {
        Block::CodeBlock { code, .. } => code.lines().map(str::trim).find(|line| !line.is_empty()),
        Block::BlockQuote { children } => children.iter().find_map(first_code_line),
        Block::List { items, .. } => items.iter().flatten().find_map(first_code_line),
        _ => None,
    }
}

/// A preview's text at the card's size: [`preview_runs`] styled like a
/// reply's Markdown, in `color` where the reply would use body text, with
/// inline code on its rounded wash.
fn preview_text(source: &str, color: gpui::Hsla, theme: &Theme) -> Option<AnyElement> {
    use crate::markdown::render;
    let runs = preview_runs(source);
    if runs.is_empty() {
        return None;
    }
    let mut theme = theme.clone();
    theme.text = color;
    let flat = render::flatten_runs(&runs, &theme, false);
    let styled = StyledText::new(flat.text).with_runs(flat.runs);
    let layout = styled.layout().clone();
    let wash = render::inline_code_wash(&theme);
    let code_ranges = flat.code_ranges;
    div()
        .relative()
        .child(
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    for range in &code_ranges {
                        for rect in render::range_rects(
                            &layout,
                            range,
                            render::INLINE_CODE_PAD_X,
                            render::INLINE_CODE_INSET_Y / 2.0,
                        ) {
                            window.paint_quad(gpui::fill(rect, wash).corner_radii(px(3.5)));
                        }
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(styled)
        .into_any_element()
        .into()
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

    /// The rail element — an absolute overlay along the transcript's left edge.
    pub fn render_rail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.rail_enabled() {
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
        // chrome lights its tick; with none on screen (a long reply), the
        // prompt being read stays lit.
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
        let n = pairs.len();
        let lit: Vec<bool> = (0..n)
            .map(|ix| in_view[ix] || (!any_in_view && active == Some(ix)))
            .collect();
        let hover = self.rail_hover().filter(|&ix| ix < n);
        let theme = Theme::of(cx).clone();
        let senders: Vec<Option<ChatIdentity>> = {
            let state = self.state_entity().read(cx);
            pairs
                .iter()
                .map(|(tick, _)| {
                    tick.sender.as_ref().map(|sender| {
                        crate::transcript::chat_identity(
                            state,
                            Some(sender.chat_id.as_ref()),
                            &sender.title,
                        )
                    })
                })
                .collect()
        };

        // The rail centres in the band between the titlebar and the bottom
        // chrome. Pre-layout the viewport reads 0; assume a typical height
        // for that one frame rather than collapsing the stack.
        let band_top = crate::transcript::OWN_SEND_TOP_INSET_PX;
        let band_bottom = self.bottom_clearance();
        let viewport_h = match f32::from(viewport.size.height) {
            h if h > 0.0 => h,
            _ => 600.0,
        };
        let height = rail_height(n);
        let content = height + TICK_SLOT;
        let window_h = rail_window(content, viewport_h - band_top - band_bottom);
        let scrolls = content > window_h + 0.5;
        let ids: std::rc::Rc<[SharedString]> = pairs
            .iter()
            .map(|(tick, _)| SharedString::from(tick.message_id.clone()))
            .collect();
        // A scrolling rail keeps the lit ticks in view, moving only when
        // what's lit changes (and not under the pointer), so scrolling it by
        // hand holds.
        let first_lit = lit.iter().position(|&l| l);
        let last_lit = lit.iter().rposition(|&l| l);
        if let (true, None, Some(first), Some(last)) = (scrolls, hover, first_lit, last_lit) {
            let key = (ids[first].clone(), ids[last].clone());
            if self.rail_followed_mut().as_ref() != Some(&key) {
                let scroll = self.rail_scroll().clone();
                let target = follow_scroll(
                    -f32::from(scroll.offset().y),
                    window_h,
                    content,
                    tick_offset(first, n, height),
                    tick_offset(last, n, height) + TICK_SLOT,
                    RAIL_EDGE_FADE,
                );
                scroll.set_offset(gpui::point(px(0.0), px(-target)));
                *self.rail_followed_mut() = Some(key);
            }
        }
        let rail_scroll = self.rail_scroll().clone();
        let bounds_cell = self.rail_bounds().clone();
        let tick_under = {
            let bounds_cell = bounds_cell.clone();
            move |y: Pixels| {
                let bounds = bounds_cell.get()?;
                tick_at(n, height, f32::from(y - bounds.top()) - TICK_SLOT / 2.0)
            }
        };
        let move_under = tick_under.clone();

        let ticks = pairs
            .into_iter()
            .zip(senders)
            .enumerate()
            .map(|(ix, ((tick, _), sender))| {
                let distance = hover.map(|h| h.abs_diff(ix));
                let bar_color = if distance == Some(0) || lit[ix] {
                    theme.text.opacity(0.8)
                } else {
                    crate::theme::ink(0.16)
                };
                let card: Option<AnyElement> = (distance == Some(0)).then(|| {
                    let theme = theme.for_popup();
                    // The sender rides over the prompt as it does over the
                    // bubble: `Message from [mark] Chat`.
                    let sender = sender.map(|ChatIdentity { title, mark, .. }| {
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(div().flex_none().child("Message from"))
                            .child(crate::transcript::caption_label(
                                title,
                                Some(mark),
                                theme.text_muted,
                            ))
                    });
                    let prompt = preview_text(&tick.prompt, theme.text, &theme);
                    let reply = tick
                        .reply
                        .as_deref()
                        .and_then(|reply| preview_text(reply, theme.text_muted, &theme));
                    let card = popover::popover_card(&theme)
                        .w(px(280.0))
                        .p(px(Theme::SPACE_SM))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .children(sender)
                        .when_some(prompt, |el, prompt| {
                            el.child(
                                div()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text)
                                    .line_clamp(PREVIEW_PROMPT_LINES)
                                    .text_ellipsis()
                                    .child(prompt),
                            )
                        })
                        .when_some(reply, |el, reply| {
                            el.child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .line_clamp(PREVIEW_REPLY_LINES)
                                    .text_ellipsis()
                                    .child(reply),
                            )
                        });
                    // Mounted straight through deferred/anchored (not a popover
                    // mount helper), so the frost wrap happens here.
                    crate::frost::frosted(12.0, crate::frost::MENU_BLUR, card).into_any_element()
                });
                div()
                    .absolute()
                    .left_0()
                    .top(px(tick_offset(ix, n, height)))
                    .h(px(TICK_SLOT))
                    .w_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .h(px(2.0))
                            .w(px(tick_width(distance)))
                            .rounded(px(1.0))
                            .bg(bar_color),
                    )
                    .when_some(card, |el, card| {
                        el.child(
                            div().absolute().left_0().top(px(TICK_SLOT / 2.0)).child(
                                gpui::deferred(
                                    gpui::anchored()
                                        .anchor(gpui::Anchor::LeftCenter)
                                        .snap_to_window_with_margin(px(8.0))
                                        .child(div().pl(px(RAIL_HIT_WIDTH + 4.0)).child(card)),
                                ),
                            ),
                        )
                    })
            });

        let stack = div()
            .id("rail-ticks")
            .relative()
            .w_full()
            .h(px(content))
            .cursor_pointer()
            .child(
                canvas(
                    move |bounds, _, _| bounds_cell.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            // One hit strip resolves the nearest tick from the pointer,
            // so dense stacks never fall between per-tick hitboxes.
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                let ix = move_under(event.position.y);
                if this.rail_hover() != ix {
                    this.set_rail_hover(ix);
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.rail_hover().is_some() {
                    this.set_rail_hover(None);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                let ix = match event {
                    ClickEvent::Mouse(_) => tick_under(event.position().y),
                    _ => None,
                }
                .or(this.rail_hover());
                // Resolve the row at click time: rows can splice
                // between the render that drew the tick and the click.
                let Some(id) = ix.and_then(|ix| ids.get(ix)) else {
                    return;
                };
                if let Some(row) = this.rows().iter().position(|r| r.id == *id) {
                    this.scroll_to_row(row, cx);
                }
            }))
            .children(ticks);
        let body = if scrolls {
            crate::edge_fade::edge_faded(
                RAIL_EDGE_FADE,
                true,
                true,
                div()
                    .id("rail-scroll")
                    .w_full()
                    .h(px(window_h))
                    .overflow_y_scroll()
                    .track_scroll(&rail_scroll)
                    .child(stack),
            )
            .fade_overflow_y(&rail_scroll)
            .into_any_element()
        } else {
            stack.into_any_element()
        };

        div()
            .absolute()
            .left(px(16.0))
            .top(px(band_top))
            .bottom(px(band_bottom))
            .w(px(RAIL_HIT_WIDTH))
            .flex()
            .flex_col()
            .justify_center()
            .child(
                div()
                    .id("rail")
                    .w_full()
                    // A scrolling rail keeps the wheel at its ends instead of
                    // handing it on to the transcript underneath.
                    .when(scrolls, |rail| {
                        rail.on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    })
                    .child(body),
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
    fn rail_keeps_natural_spacing_and_windows_long_stacks() {
        assert_eq!(rail_height(2), TICK_SPACING);
        assert_eq!(rail_height(10), 9.0 * TICK_SPACING);
        // Long conversations keep their spacing; the window scrolls instead.
        assert_eq!(rail_height(500), 499.0 * TICK_SPACING);
        assert_eq!(rail_height(1), 0.0);
        // A short stack shows whole; a long one caps at 70% of the band, and
        // at RAIL_MAX_HEIGHT on a tall window.
        assert_eq!(rail_window(90.0, 700.0), 90.0);
        assert_eq!(rail_window(4000.0, 700.0), 490.0);
        assert_eq!(rail_window(4000.0, 2000.0), RAIL_MAX_HEIGHT);
        // A cramped band still leaves a usable window.
        assert_eq!(rail_window(4000.0, 0.0), TICK_SLOT * 4.0);
    }

    #[test]
    fn follow_scroll_moves_least_and_clears_the_fade() {
        // Already comfortably in view: no movement.
        assert_eq!(
            follow_scroll(100.0, 200.0, 1000.0, 150.0, 160.0, 24.0),
            100.0
        );
        // Below the window: lands just above the bottom fade.
        assert_eq!(follow_scroll(0.0, 200.0, 1000.0, 400.0, 410.0, 24.0), 234.0);
        // Above it: lands just below the top fade.
        assert_eq!(
            follow_scroll(500.0, 200.0, 1000.0, 300.0, 310.0, 24.0),
            276.0
        );
        // A span taller than the window shows its top.
        assert_eq!(follow_scroll(0.0, 200.0, 1000.0, 400.0, 700.0, 24.0), 400.0);
        // Clamped to the stack's ends.
        assert_eq!(
            follow_scroll(0.0, 200.0, 1000.0, 990.0, 1000.0, 24.0),
            800.0
        );
        assert_eq!(follow_scroll(300.0, 200.0, 1000.0, 0.0, 10.0, 24.0), 0.0);
    }

    #[test]
    fn tick_offsets_spread_evenly_end_to_end() {
        assert_eq!(tick_offset(0, 5, 40.0), 0.0);
        assert_eq!(tick_offset(2, 5, 40.0), 20.0);
        assert_eq!(tick_offset(4, 5, 40.0), 40.0);
        assert_eq!(tick_offset(0, 1, 40.0), 0.0);
    }

    /// Every prompt is reachable: pointer → nearest tick → same tick's
    /// offset, at any density (the old fixed 12-slot buckets skipped prompts).
    #[test]
    fn pointer_maps_to_every_tick() {
        for n in [2usize, 7, 12, 13, 80, 300] {
            let height = rail_height(n);
            for ix in 0..n {
                let y = tick_offset(ix, n, height);
                assert_eq!(tick_at(n, height, y), Some(ix), "n={n} ix={ix}");
            }
        }
        // Clamped past either end.
        assert_eq!(tick_at(5, 32.0, -20.0), Some(0));
        assert_eq!(tick_at(5, 32.0, 99.0), Some(4));
        assert_eq!(tick_at(0, 32.0, 0.0), None);
        assert_eq!(tick_at(1, 0.0, 5.0), Some(0));
    }

    #[test]
    fn fisheye_widths_fall_off_from_hover() {
        assert_eq!(tick_width(Some(0)), 24.0);
        assert!(tick_width(Some(1)) < tick_width(Some(0)));
        assert!(tick_width(Some(2)) < tick_width(Some(1)));
        assert_eq!(tick_width(Some(3)), tick_width(None));
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
    fn previews_render_markdown_as_one_line_of_prose() {
        let runs = preview_runs("Use **bold** and `code`.\n\n- one\n- *two*");
        let text: String = runs.iter().map(|run| run.text.as_str()).collect();
        assert_eq!(text, "Use bold and code. one two");
        let style = |needle: &str| &runs.iter().find(|run| run.text == needle).unwrap().style;
        assert!(style("bold").bold);
        assert!(style("code").code);
        assert!(style("two").italic);
        // Spaces between runs stay outside their styling.
        assert!(
            runs.iter()
                .filter(|run| run.style != InlineStyle::default())
                .all(|run| !run.text.starts_with(' ') && !run.text.ends_with(' '))
        );
        // Markers and list bullets never show as source.
        assert!(!text.contains('*') && !text.contains('`'));
        // Whitespace collapses; nothing leads or trails.
        let runs = preview_runs("  padded\nlines  ");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "padded lines");
        // A long source is cut on a char boundary before parsing.
        let long = "é".repeat(PREVIEW_SOURCE_BYTES);
        assert!(!preview_runs(&long).is_empty());
        // Inline formulas show their source as code.
        let runs = preview_runs("area $x^2$");
        assert_eq!(runs.last().unwrap().text, "$x^2$");
        assert!(runs.last().unwrap().style.code);
    }

    #[test]
    fn previews_leave_out_blocks_that_are_not_prose() {
        let text = |source: &str| -> String {
            preview_runs(source)
                .iter()
                .map(|run| run.text.as_str())
                .collect()
        };
        let source = "Run this:\n\n```sh\ncargo test\n```\n\n> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n$$x$$\n\n---\n\nthen report.";
        assert_eq!(text(source), "Run this: then report.");
        // A fence inside a list item goes too.
        assert_eq!(text("- step\n\n  ```\n  make\n  ```"), "step");
        // A message of nothing but a fence shows its opening line.
        let runs = preview_runs("```rust\n\nfn main() {\n}\n```");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "fn main() {");
        assert!(runs[0].style.code);
        // Nothing to show at all stays empty.
        assert!(preview_runs("---").is_empty());
    }

    #[test]
    fn agent_sent_prompts_carry_their_sender() {
        let id = "c563133c-2825-4056-8d1e-4b2a18c210ae";
        let routed = format!(
            "[Message from Zeron chat Harden Orbit (c563133c). Reply to it with the Zeron `send_message` tool, chat {id}.]\n\nYou own the **soak**."
        );
        let entries = vec![
            entry("u1", MessageRole::User, &routed),
            entry("u2", MessageRole::User, "plain"),
        ];
        let ticks = rail_ticks(&entries, &[]);
        let sender = ticks[0].sender.as_ref().unwrap();
        assert_eq!(sender.chat_id.as_ref(), id);
        assert_eq!(sender.title.as_ref(), "Harden Orbit");
        assert_eq!(ticks[0].prompt, "You own the **soak**.");
        assert!(ticks[1].sender.is_none());
    }
}
