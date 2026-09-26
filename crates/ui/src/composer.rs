//! The composer: a hand-rolled multiline text input (adapted from gpui's
//! `examples/input.rs`), the compact↔expanded flip, the Send/Queue/Stop morph,
//! optimistic send with failure recovery, per-chat drafts, and the question
//! wizard that replaces the composer while a run awaits input.
//!
//! Pure decision logic (flip, auto-grow math, button morph, wizard reducer,
//! pending-input detection) lives in free functions/structs with unit tests;
//! the gpui element only feeds them measurements.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyTooltip, App, BorderStyle, Bounds, ClipboardEntry, ClipboardItem, Context, CursorStyle,
    DispatchPhase, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, GlobalElementId, KeyBinding, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, PaintQuad, PathPromptOptions, Pixels, Point, Role,
    ScrollWheelEvent, SharedString, Style, StyledImage as _, Subscription, Task, TextRun,
    TextStyle, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, img, point,
    prelude::*, px, quad, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use zeron_doc::{MessagePart, MessageRole, SessionCommandPayload, SessionMessageEntry};
use zeron_proto::{
    FileSearchMatch, HarnessId, RunRequest, SandboxLevel, SlashCommand, UserInputAnswer,
    UserInputQuestion, capabilities,
};
use zeron_rpc::{RpcError, methods};

use crate::appshots::{self, CapturedAppshot};
use crate::attachments::{self, StagedAttachment};
use crate::composer_markdown::{self, in_code};
use crate::motion;
use crate::notice::{NoticeChipIcon, notice_chip};
use crate::pickers::Pickers;
use crate::settings::{ComposerSendBehavior, platform_combo};
use crate::state::{AppState, Indicator};
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// Constants + pure decision logic
// ---------------------------------------------------------------------------

/// Expanded-mode textarea vertical padding: `pt-4 pb-1` (zeron composer.tsx
/// line 578) = 16 + 4.
pub const TEXTAREA_PAD_V: f32 = 20.0;
/// The expanded textarea BOX (content + padding) is clamped by the original's
/// auto-grow effect: `ta.style.height = Math.min(Math.max(scrollHeight, 76),
/// 260)` (zeron composer.tsx line 235). The 76px floor applies even when
/// empty — it's what makes the always-expanded new-chat composer tall.
pub const TEXTAREA_MIN: f32 = 76.0;
pub const TEXTAREA_MAX: f32 = 260.0;
/// Expanded actions row: 2px top + 32px picker + 8px bottom.
const ACTIONS_BOTTOM_PAD: f32 = 8.0;
pub const ACTIONS_ROW_HEIGHT: f32 = 2.0 + 32.0 + ACTIONS_BOTTOM_PAD;
/// The pill's 1px hairline, top + bottom (`rounded-[26px] border`).
pub const PILL_BORDER_V: f32 = 2.0;
/// Corner radius shared by the composer and the queue tray behind it.
pub(crate) const COMPOSER_RADIUS: f32 = 26.0;
/// Expanded composer bounds: 120px when empty, 304px at the content cap.
pub const COMPOSER_MIN_HEIGHT: f32 = TEXTAREA_MIN + ACTIONS_ROW_HEIGHT + PILL_BORDER_V;
pub const COMPOSER_MAX_HEIGHT: f32 = TEXTAREA_MAX + ACTIONS_ROW_HEIGHT + PILL_BORDER_V;
/// Compact pill, border-box: one-line textarea `py-3` (24) + one 22.75px line
/// (scrollHeight rounds to 47 in the original) + the 2px hairline = 49. The
/// compact cluster (`py-1.5` + h-8 = 44) is shorter, so the textarea wins.
pub const COMPACT_TOTAL_HEIGHT: f32 = 49.0;
/// `max-w-3xl`: outer width of the new-chat composer, also used when no shell
/// width is supplied. Established threads follow the conversation column.
pub const COMPOSER_MAX_WIDTH: f32 = 768.0;
/// The queue reads as a narrower tray emerging from behind the composer.
const QUEUE_SIDE_INSET: f32 = 16.0;
/// The composer covers the tray's lower padding so the queue reads as emerging
/// from behind it instead of as a separate rounded pill.
pub(crate) const QUEUE_COMPOSER_OVERLAP: f32 = 18.0;
/// The original floating selector rows use the same 20px chip height as the
/// established-thread footer. Their surrounding rows own no plate or border.
const NEW_THREAD_SELECTOR_ROW_HEIGHT: f32 = 20.0;
// Accommodate the 24px usage indicator and PR badge without overflowing the
// row's equal 8px top/bottom gutters.
pub(crate) const SESSION_FOOTER_HEIGHT: f32 = 24.0;

/// Route chrome dissolves around the middle of the shared-element move. The
/// two ramps never overlap, which avoids duplicate picker ids/popovers while
/// still letting their surrounding geometry collapse continuously.
fn route_chrome_opacities(new_thread_chrome: f32) -> (f32, f32) {
    let new_thread = ((new_thread_chrome.clamp(0.0, 1.0) - 0.5) * 2.0).clamp(0.0, 1.0);
    let session = (((1.0 - new_thread_chrome.clamp(0.0, 1.0)) - 0.5) * 2.0).clamp(0.0, 1.0);
    (new_thread, session)
}
/// Ignore subpixel noise when the shell reports the conversation width.
const COMPOSER_WIDTH_EPSILON: f32 = 0.5;
/// Below this pill input width the composer always expands.
pub const MIN_COMPACT_INPUT_WIDTH: f32 = 200.0;
/// Input text metrics: `text-[14px] leading-relaxed` = 14 × 1.625 = 22.75.
pub const INPUT_LINE_HEIGHT: f32 = 22.75;
pub const INPUT_TEXT_SIZE: f32 = 14.0;
/// A compact ramp; the glyph-ascent inset keeps the clip edge invisible.
const INPUT_FADE_BAND: f32 = 12.0;
/// Single-select questions auto-advance after this long.
pub const AUTO_ADVANCE_MS: u64 = 220;
/// Drag-selection autoscroll runs at the display-friendly 60fps cadence.
pub const DRAG_SCROLL_FRAME_MS: u64 = 16;

/// Hysteresis slack for the expanded→compact flip: once expanded, the composer
/// only collapses when the text is comfortably narrower than the compact
/// capacity — expanding and collapsing share no boundary, so a width right at
/// the flip threshold can't oscillate between the two layouts.
pub const COLLAPSE_HYSTERESIS: f32 = 32.0;
/// During an interactive resize, collapsing back to the compact mode waits
/// until the measured widths have been stable this long. Expansion remains
/// immediate so a narrowing panel never traps the controls in a compact row.
pub const RESIZE_SETTLE_MS: u64 = 150;

/// Compact↔expanded flip with hysteresis. `capacity` is the *compact-mode*
/// input capacity (a layout-stable width: measured while compact, tracked by
/// container-width deltas while expanded — never the post-flip measured width,
/// which differs per mode and would feed back into the decision):
/// - a newline always expands;
/// - while `resizing`, an expanded composer stays expanded until sizes settle;
/// - a too-narrow pill (`capacity < MIN_COMPACT_INPUT_WIDTH`) always expands;
/// - compact expands only when `text_width > capacity`; expanded collapses
///   only when `text_width < capacity - COLLAPSE_HYSTERESIS`.
pub fn composer_flip(
    expanded: bool,
    text_width: f32,
    capacity: f32,
    has_newline: bool,
    resizing: bool,
) -> bool {
    if has_newline {
        return true;
    }
    if capacity < MIN_COMPACT_INPUT_WIDTH {
        return true;
    }
    if expanded {
        resizing || text_width >= capacity - COLLAPSE_HYSTERESIS
    } else {
        text_width > capacity
    }
}

fn composer_width_changed(previous: Option<f32>, current: f32) -> bool {
    previous.is_none_or(|previous| (current - previous).abs() > COMPOSER_WIDTH_EPSILON)
}

/// Caret blink half-period (standard textarea cadence: ~500ms on / 500ms off).
pub const CARET_BLINK_MS: u64 = 500;

/// Caret blink phase for a time since the last keystroke/caret move: solid
/// through the first half-period (typing bursts never blink — each keystroke
/// resets the phase), then alternating.
pub fn caret_visible(ms_since_activity: u64) -> bool {
    (ms_since_activity / CARET_BLINK_MS) % 2 == 0
}

/// Auto-grow: content height for a wrapped-line count.
pub fn input_content_height(wrapped_lines: usize) -> f32 {
    wrapped_lines.max(1) as f32 * INPUT_LINE_HEIGHT
}

/// Total expanded composer height (border-box) for a content height: the
/// textarea BOX (content + `pt-4 pb-1`) clamps to 76–260 exactly like the
/// original's auto-grow effect, then the actions row and the hairline
/// ride on top. Range 120–304.
pub fn composer_total_height(content_height: f32) -> f32 {
    (content_height + TEXTAREA_PAD_V).clamp(TEXTAREA_MIN, TEXTAREA_MAX)
        + ACTIONS_ROW_HEIGHT
        + PILL_BORDER_V
}

fn input_max_scroll(content_height: f32, viewport_height: f32) -> f32 {
    (content_height - viewport_height).max(0.0)
}

/// Only settled overflow gets a scroll fade. The animated viewport can be
/// smaller for a few frames while an otherwise fitting draft grows into it.
fn input_overflow_edges(
    content_height: f32,
    settled_height: f32,
    visible_height: f32,
    scroll_top: f32,
) -> (bool, bool) {
    if input_max_scroll(content_height, settled_height) <= 1.0 {
        return (false, false);
    }
    let max_scroll = input_max_scroll(content_height, visible_height);
    (scroll_top > 1.0, scroll_top < max_scroll - 1.0)
}

/// During the reveal, stop at a complete row boundary instead of slicing
/// glyphs with a moving clip. Scrolling offsets the row grid inside the box.
fn input_reveal_height(visible: f32, scroll: f32, line_height: f32, resizing: bool) -> f32 {
    if !resizing {
        return visible;
    }
    let row_end = ((scroll + visible + 0.001) / line_height).floor() * line_height;
    (row_end - scroll).clamp(0.0, visible)
}

/// Apply GPUI's wheel delta to a top-origin input offset. Positive deltas mean
/// scrolling toward the start, matching gpui's built-in list/div behavior.
fn input_scroll_offset(
    current: f32,
    delta_y: f32,
    content_height: f32,
    viewport_height: f32,
) -> f32 {
    (current - delta_y).clamp(0.0, input_max_scroll(content_height, viewport_height))
}

/// Minimally adjust the viewport so the caret row is fully visible.
fn input_scroll_offset_for_cursor(
    current: f32,
    cursor_top: f32,
    cursor_height: f32,
    content_height: f32,
    viewport_height: f32,
    settled_height: Option<f32>,
) -> f32 {
    // Resize the reveal, not the scroll position: existing text stays fixed
    // relative to the input origin throughout the height animation.
    let viewport_height = settled_height.unwrap_or(viewport_height);
    let mut next = current;
    if cursor_top < next {
        next = cursor_top;
    } else if cursor_top + cursor_height > next + viewport_height {
        next = cursor_top + cursor_height - viewport_height;
    }
    next.clamp(0.0, input_max_scroll(content_height, viewport_height))
}

/// Pointer selection granularity; multi-click drags retain the initial unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressIntent {
    Word,
    Line,
    ExtendSelection,
    PlaceCaret,
}

fn press_intent(click_count: usize, shift: bool) -> PressIntent {
    if click_count >= 3 {
        PressIntent::Line
    } else if click_count == 2 {
        PressIntent::Word
    } else if shift {
        PressIntent::ExtendSelection
    } else {
        PressIntent::PlaceCaret
    }
}

fn word_range(text: &str, offset: usize) -> Range<usize> {
    let offset = offset.min(text.len());
    text.split_word_bound_indices()
        .find(|(at, word)| *at <= offset && offset < at + word.len())
        .map(|(at, word)| at..at + word.len())
        .unwrap_or(offset..offset)
}

/// Per-frame drag-selection scroll. Distance increases speed, capped at one
/// text row per frame so crossing the input boundary never causes a jump.
fn input_drag_scroll_delta(
    pointer_y: f32,
    viewport_top: f32,
    viewport_bottom: f32,
    line_height: f32,
) -> f32 {
    let distance = if pointer_y < viewport_top {
        pointer_y - viewport_top
    } else if pointer_y > viewport_bottom {
        pointer_y - viewport_bottom
    } else {
        return 0.0;
    };
    distance.signum() * (distance.abs() * 0.2).clamp(1.0, line_height)
}

/// Staged-attachment strip metrics (zeron attachment-ui.tsx AttachmentStrip:
/// `flex flex-wrap gap-2 px-4 pt-3`, `size-14` thumbs).
pub const STRIP_THUMB: f32 = 56.0;
pub const STRIP_GAP: f32 = 8.0;
pub const STRIP_PAD_TOP: f32 = 12.0;
pub const STRIP_PAD_X: f32 = 16.0;

/// Height the wrap strip adds to the pill for `count` staged thumbnails at an
/// `inner_width` pill content width (0 when empty). Mirrors flex-wrap: as many
/// 56px thumbs per row as fit with 8px gaps inside the 16px side insets.
pub fn attachment_strip_height(count: usize, inner_width: f32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let usable = (inner_width - 2.0 * STRIP_PAD_X).max(STRIP_THUMB);
    let per_row = (((usable + STRIP_GAP) / (STRIP_THUMB + STRIP_GAP)).floor() as usize).max(1);
    let rows = count.div_ceil(per_row);
    STRIP_PAD_TOP + rows as f32 * STRIP_THUMB + (rows - 1) as f32 * STRIP_GAP
}

pub fn comment_strip_height(count: usize) -> f32 {
    if count == 0 {
        return 0.0;
    }
    STRIP_PAD_TOP + crate::badges::BADGE_HEIGHT
}

/// Compact↔expanded flip morph (round 9): the flip used to snap between the
/// two pill layouts. The original has no height transition (its shell carries
/// only `transition-colors`), so this is a native nicety: ONE committed flip
/// starts exactly one 180ms ease-out morph ([`motion::COLLAPSE`]); the blank-
/// thread handoff swaps in the coordinated 420ms route-transition spec. Both use the
/// manual-drive pattern from shell.rs `WidthTween` — never `with_animation`,
/// whose element-id keying replays tweens on remount, round-6 §1–3.
///
/// The morph animates the pill's COMMITTED height: the flip commits its final
/// layout immediately (the input entity never remounts — the caret survives,
/// exactly as before) while the pill clips toward the live target. The pill's
/// bottom edge is stationary on screen, so the controls stay pinned to it
/// (constant screen-y; see the anchoring helpers below) and only the text
/// glides with the sweeping top edge. [`composer_flip`]'s hysteresis already
/// guarantees no oscillation at the boundary, and [`flip_morph_step`] never
/// restarts a morph while the committed mode holds. Reduced motion snaps: no
/// morph is ever created.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlipMorph {
    /// Rendered height when the flip committed — the animation's start point.
    pub from: f32,
    /// Commit time in ms on the caller's monotonic clock.
    pub start_ms: f32,
    /// Ordinary typing flips use the quick collapse spec; the first-send
    /// handoff uses the shell's longer coordinated route timeline.
    pub spec: motion::MotionSpec,
}

impl FlipMorph {
    fn collapse(from: f32, start_ms: f32) -> Self {
        Self {
            from,
            start_ms,
            spec: motion::COLLAPSE,
        }
    }

    fn new_thread_transition(from: f32, start_ms: f32) -> Self {
        Self {
            from,
            start_ms,
            spec: motion::NEW_THREAD_TRANSITION,
        }
    }

    /// Raw timeline position 0..1 over this morph's motion spec.
    fn raw(&self, now_ms: f32) -> f32 {
        let total = self.spec.total().as_secs_f32() * 1000.0;
        ((now_ms - self.start_ms) / total).clamp(0.0, 1.0)
    }

    /// Eased progress 0..1 — also drives the inner geometry handoff.
    pub fn progress(&self, now_ms: f32) -> f32 {
        self.spec.progress(self.raw(now_ms))
    }

    pub fn done(&self, now_ms: f32) -> bool {
        self.raw(now_ms) >= 1.0
    }

    /// Committed-height evaluation: eased lerp from the flip-time height to
    /// the LIVE target (auto-grow may move the target mid-morph — the morph
    /// tracks it instead of finishing on a stale height).
    pub fn height(&self, target: f32, now_ms: f32) -> f32 {
        motion::lerp(self.from, target, self.progress(now_ms))
    }
}

// -- morph anchoring (round-9 follow-up) ------------------------------------
// The pill sits at the BOTTOM of the shell column: growing it moves its TOP
// edge; the bottom edge is stationary on screen. The first morph cut anchored
// the pill's inner content to the top, so the actions/cluster (laid out at
// the inner bottom) rode the animating height up and down. The controls are
// therefore pinned to the stationary bottom edge (absolute bottom row when
// expanded, a bottom-justified row when compact) and only the TEXT glides
// with the sweeping top edge. The helpers below are the pure math.

/// Send/attach center sits 25px above the expanded pill's bottom (8px
/// padding + half the 32px control zone + 1px border), versus 24.5px in
/// compact. The morph glides this optical adjustment instead of snapping.
pub const CLUSTER_Y_DELTA: f32 =
    ACTIONS_BOTTOM_PAD + 16.0 + PILL_BORDER_V / 2.0 - COMPACT_TOTAL_HEIGHT / 2.0;

/// Attachment and Send share an outer inset: compact 8px, expanded 12px.
/// Glide both edges together while the model picker changes groups.
pub const CLUSTER_X_DELTA: f32 = 4.0;
/// Optical join between the picker group and the paperclip. This is tighter
/// than the structural spacing ladder because the narrow paperclip glyph
/// otherwise looks farther away than its hit target actually is.
pub const ACTION_UTILITY_GAP: f32 = 2.0;
/// Structural separation between utility actions and the primary Send action.
pub const ACTION_PRIMARY_GAP: f32 = Theme::SPACE_SM;

/// Fade out at the old endpoint, relocate while invisible, then fade in at
/// the new endpoint. Only a six-pixel nudge is visible; a long label never
/// sweeps across the prompt. Compact amount is reversible with the shared clock.
fn model_handoff(compact: f32) -> (f32, f32, f32) {
    let compact = compact.clamp(0.0, 1.0);
    let side = if compact < 0.5 { 0.0 } else { 1.0 };
    let opacity = ((compact - 0.5).abs() - 0.06).max(0.0) / 0.44;
    let drift = (1.0 - opacity) * if side == 0.0 { 6.0 } else { -6.0 };
    (side, opacity, drift)
}

/// The shared outer inset for the in-flight morph: eases from the OLD mode's resting
/// inset to the committed mode's (compact 8 ↔ expanded 12).
pub fn morph_cluster_inset(expanded: bool, progress: f32) -> f32 {
    let (from, to) = if expanded {
        (8.0, 8.0 + CLUSTER_X_DELTA)
    } else {
        (8.0 + CLUSTER_X_DELTA, 8.0)
    };
    motion::lerp(from, to, progress)
}

/// Expanded text top padding across the morph: starts at the compact resting
/// inset (12 ≈ `py-3`) and eases to `pt-4` (16) — the first line glides with
/// the rising top edge instead of jumping at the commit.
pub fn morph_text_pad(progress: f32) -> f32 {
    motion::lerp(12.0, 16.0, progress)
}

/// Collapse-morph text glide: the committed compact row is bottom-anchored
/// (text resting top = 36px above the pill's outer bottom: 49 − 1 hairline −
/// 12 centering inset), while at the commit instant the text sat 17px below
/// the expanded pill's top (1 hairline + 16 `pt-4`) — i.e. `from − 17` above
/// the bottom. The decaying relative offset walks it down smoothly.
pub fn collapse_text_glide(from: f32, progress: f32) -> f32 {
    (from - 53.0).max(0.0) * (1.0 - progress)
}

/// The decaying [`CLUSTER_Y_DELTA`] offset for the in-flight morph.
/// Controls share this bottom anchor; the model's horizontal fade is applied
/// independently so its endpoint matches Attachment and Send.
pub fn morph_cluster_dy(progress: f32) -> f32 {
    CLUSTER_Y_DELTA * (1.0 - progress)
}

/// Session/route changes SNAP the composer (same rule as the header inset
/// tween, round 6: route swaps remount in the original — zero motion). The
/// nav-driven flip doesn't commit on the first render after a switch (the
/// draft swap has to be laid out and re-measured first), so a plain reset at
/// the nav instant leaks: `last_rendered_height` is repopulated before the
/// flip lands and the session change morphs 49↔124. Instead, every flip
/// committed within this wall-clock window of a navigation snaps. User-driven
/// flips need typing and can't land this fast after a switch.
pub const ROUTE_SNAP_MS: u64 = 250;

/// Advance the flip morph across one render pass. While the committed mode
/// holds, the morph is kept (a finished one clears) — same-mode renders can
/// NEVER restart the animation. A committed mode change starts one morph from
/// the last rendered height, which mid-flight is the CURRENT animated height,
/// so a reverse flip hands off seamlessly instead of popping to an endpoint.
/// Reduced motion (or a first paint with no measured height yet) snaps, and
/// `route_snap` (a session/route change within [`ROUTE_SNAP_MS`]) both blocks
/// arming AND kills anything in flight — navigation never animates the pill.
pub fn flip_morph_step(
    morph: Option<FlipMorph>,
    mode_changed: bool,
    last_height: f32,
    now_ms: f32,
    reduced_motion: bool,
    route_snap: bool,
) -> Option<FlipMorph> {
    if route_snap || reduced_motion {
        return None;
    }
    if !mode_changed {
        return morph.filter(|m| !m.done(now_ms));
    }
    if reduced_motion || last_height <= 0.0 {
        return None;
    }
    Some(FlipMorph::collapse(last_height, now_ms))
}

/// Engines at or above this version understand `pending://` attachment refs
/// and QueueCommand `transfers` (send-is-a-local-write attachments). Gated on
/// BOTH the local engine (an IPC daemon may be older than this UI) and, for
/// remotely-hosted chats, the host device's stamped registry version.
const QUEUED_ATTACHMENTS_MIN: (u64, u64, u64) = (0, 2, 12);

/// What the send button is right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendButtonMode {
    /// No live run: plain send.
    Send,
    /// Live run with text typed: queue for the next turn.
    Queue,
    /// Live run, nothing typed: red stop square.
    Stop,
}

/// What the composer holds that a send could carry. A staged image or diff
/// comment counts: both synthesize their own prompt body, so either alone is
/// a legal send — and during a live run has to read as Queue, not Stop.
pub fn composer_has_content(text: &str, attachments: usize, comments: usize) -> bool {
    !text.trim().is_empty() || attachments > 0 || comments > 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModifiedSubmitTarget {
    SubmitContent,
    ActivateLatestQueued,
}

fn modified_submit_target(has_content: bool) -> ModifiedSubmitTarget {
    if has_content {
        ModifiedSubmitTarget::SubmitContent
    } else {
        ModifiedSubmitTarget::ActivateLatestQueued
    }
}

pub const APPSHOT_TILE_MIN_WIDTH: f32 = 96.0;
pub const APPSHOT_IMAGE_INSET: f32 = 12.0;
pub const APPSHOT_PREVIEW_HEIGHT: f32 = 148.0;
pub const APPSHOT_IMAGE_MAX_WIDTH: f32 = 320.0;
pub const APPSHOT_IMAGE_MAX_HEIGHT: f32 = 132.0;
pub const APPSHOT_TILE_HEIGHT: f32 = 192.0;

struct AppshotActionTooltip(SharedString);

impl Render for AppshotActionTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(11.0))
            .text_color(theme.text)
            .child(self.0.clone())
    }
}

/// Give ordinary captures a shared height while their width follows the
/// source window. Extreme panoramas and narrow composers cap width without
/// cropping. Explicit dimensions also bound the native image while decoding.
pub fn appshot_contained_size(dimensions: Option<(u32, u32)>, max_width: f32) -> (f32, f32) {
    let max_width = if max_width.is_finite() {
        max_width.clamp(1.0, APPSHOT_IMAGE_MAX_WIDTH)
    } else {
        APPSHOT_IMAGE_MAX_WIDTH
    };
    let (width, height) = dimensions
        .filter(|(width, height)| *width > 0 && *height > 0)
        .unwrap_or((16, 10));
    let scale = (max_width / width as f32).min(APPSHOT_IMAGE_MAX_HEIGHT / height as f32);
    (width as f32 * scale, height as f32 * scale)
}

pub fn appshot_strip_height(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        STRIP_PAD_TOP + APPSHOT_TILE_HEIGHT
    }
}

pub fn send_button_mode(run_live: bool, has_text: bool) -> SendButtonMode {
    match (run_live, has_text) {
        (false, _) => SendButtonMode::Send,
        (true, true) => SendButtonMode::Queue,
        (true, false) => SendButtonMode::Stop,
    }
}

/// Queue rows are represented by the queue panel until the host promotes them
/// into the transcript. They must never publish (or refresh) a local echo.
fn should_publish_optimistic_echo(queue: bool) -> bool {
    !queue
}

fn begin_interrupt(pending: &mut HashSet<String>, chat_id: &str) -> bool {
    pending.insert(chat_id.to_string())
}

fn retain_live_interrupts(pending: &mut HashSet<String>, mut is_live: impl FnMut(&str) -> bool) {
    pending.retain(|chat_id| is_live(chat_id));
}

fn interrupt_params(chat_id: &str) -> serde_json::Value {
    serde_json::json!({
        "chatId": chat_id,
        "command": { "kind": "interrupt" },
    })
}

fn escape_dismisses_completion(key: &str, completion_open: bool) -> bool {
    key == "escape" && completion_open
}

fn wizard_escape_goes_back(key: &str, input_focused: bool, input_empty: bool) -> bool {
    key == "escape" && (!input_focused || input_empty)
}

/// Find the unresolved input request the panel should serve, if any: an
/// unresolved input part on the LAST assistant entry — regardless of the
/// entry's run status. The question stays answerable until the user actually
/// answers it (user requirement): a run that died under its question (engine
/// restart reaping it) leaves an aborted entry whose answer the engine
/// delivers as a resumed turn (`RespondInput`'s dead-run fallback). A newer
/// assistant entry supersedes an unanswered question. Assistant-entry-scoped,
/// not last-entry: a steer prompt sent while the agent waits appends a USER
/// entry after the streaming assistant entry, and a last-entry-only read made
/// the QuestionPanel vanish exactly when the user typed (earlier forensics;
/// matches the original composer.tsx, which reads the live-assistant fold —
/// rebuilt from replay even after the run died).
pub fn pending_input_request(
    transcript: &[SessionMessageEntry],
) -> Option<(String, Vec<UserInputQuestion>)> {
    transcript
        .iter()
        .rev()
        .find(|entry| entry.role == MessageRole::Assistant)
        .and_then(|entry| {
            entry.parts.iter().find_map(|part| match part {
                MessagePart::Input {
                    request_id,
                    questions,
                    resolved: false,
                    ..
                } => Some((request_id.clone(), questions.clone())),
                _ => None,
            })
        })
}

/// Whether the transcript shows `request_id` explicitly resolved (here or on
/// another device) — the wizard latch's release condition.
pub fn input_request_resolved(transcript: &[SessionMessageEntry], request_id: &str) -> bool {
    transcript.iter().any(|entry| {
        entry.parts.iter().any(|part| {
            matches!(
                part,
                MessagePart::Input {
                    request_id: rid,
                    resolved: true,
                    ..
                } if rid == request_id
            )
        })
    })
}

// ---------------------------------------------------------------------------
// Question wizard (pure reducer)
// ---------------------------------------------------------------------------

/// Reducer outcome of a wizard interaction.
#[derive(Debug, Clone, PartialEq)]
pub enum WizardStep {
    Stay,
    /// Single-select landed — advance after [`AUTO_ADVANCE_MS`].
    AutoAdvance,
    /// All pages answered — submit these answers.
    Done(Vec<UserInputAnswer>),
}

/// Paged question state ("1/3"): single-select auto-advances, multi-select and
/// typed answers advance explicitly, number keys 1-9 select, Back pages back.
#[derive(Debug, Clone)]
pub struct Wizard {
    pub request_id: String,
    pub questions: Vec<UserInputQuestion>,
    pub page: usize,
    picked: Vec<Vec<usize>>,
    typed: Vec<String>,
}

impl Wizard {
    pub fn new(request_id: String, questions: Vec<UserInputQuestion>) -> Self {
        let n = questions.len();
        Self {
            request_id,
            questions,
            page: 0,
            picked: vec![Vec::new(); n],
            typed: vec![String::new(); n],
        }
    }

    pub fn counter(&self) -> String {
        format!("{}/{}", self.page + 1, self.questions.len().max(1))
    }

    pub fn current(&self) -> Option<&UserInputQuestion> {
        self.questions.get(self.page)
    }

    pub fn is_picked(&self, option_ix: usize) -> bool {
        self.picked
            .get(self.page)
            .is_some_and(|p| p.contains(&option_ix))
    }

    /// Whether the current page has any picked option.
    pub fn page_has_pick(&self) -> bool {
        self.picked.get(self.page).is_some_and(|p| !p.is_empty())
    }

    /// Click/tap an option.
    pub fn select(&mut self, option_ix: usize) -> WizardStep {
        let Some(question) = self.questions.get(self.page) else {
            return WizardStep::Stay;
        };
        if option_ix >= question.options.len() {
            return WizardStep::Stay;
        }
        let multi = question.multi_select;
        let Some(picked) = self.picked.get_mut(self.page) else {
            return WizardStep::Stay;
        };
        if multi {
            match picked.iter().position(|&p| p == option_ix) {
                Some(at) => {
                    picked.remove(at);
                }
                None => picked.push(option_ix),
            }
            WizardStep::Stay
        } else {
            *picked = vec![option_ix];
            WizardStep::AutoAdvance
        }
    }

    /// Number key 1-9.
    pub fn press_number(&mut self, number: usize) -> WizardStep {
        if number == 0 {
            return WizardStep::Stay;
        }
        self.select(number - 1)
    }

    pub fn set_typed(&mut self, text: String) {
        if let Some(slot) = self.typed.get_mut(self.page) {
            *slot = text;
        }
    }

    /// Explicit submit / auto-advance landing.
    pub fn advance(&mut self) -> WizardStep {
        if self.page + 1 < self.questions.len() {
            self.page += 1;
            WizardStep::Stay
        } else {
            WizardStep::Done(self.answers())
        }
    }

    /// Page back; false when already on the first page.
    pub fn back(&mut self) -> bool {
        if self.page > 0 {
            self.page -= 1;
            true
        } else {
            false
        }
    }

    /// Answers per question: free text overrides picked labels.
    pub fn answers(&self) -> Vec<UserInputAnswer> {
        self.questions
            .iter()
            .enumerate()
            .map(|(ix, q)| {
                let typed = self.typed.get(ix).map(|s| s.trim()).unwrap_or("");
                let labels = if !typed.is_empty() {
                    vec![typed.to_string()]
                } else {
                    self.picked
                        .get(ix)
                        .map(|picked| {
                            picked
                                .iter()
                                .filter_map(|&p| q.options.get(p).cloned())
                                .collect()
                        })
                        .unwrap_or_default()
                };
                UserInputAnswer {
                    question_id: q.id.clone(),
                    labels,
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Multiline text input (adapted from gpui examples/input.rs)
// ---------------------------------------------------------------------------

actions!(
    composer,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        Copy,
        Cut,
        Paste,
        Newline,
        MessageNewlineOrAccept,
        ModifiedSubmit,
        Submit,
        Undo,
        Redo,
        MentionTab,
        OutdentList,
    ]
);

/// How long a run of single-character edits keeps merging into one undo step.
/// A pause longer than this starts a fresh step, so undo rewinds in the
/// bursts the user actually typed rather than one character at a time.
const UNDO_COALESCE: Duration = Duration::from_millis(700);

/// Cap on retained undo steps — a long-lived composer must not grow forever.
const UNDO_LIMIT: usize = 200;

const MENTION_TOOLTIP_DELAY: Duration = Duration::from_millis(420);
const MENTION_TOOLTIP_HEIGHT: f32 = 24.0;
// Narrow nonbreaking spaces give UI-font chips compact insets and gaps,
// while preserving the chip's atomic wrapping and source/caret projection.
const MENTION_SIDE_PAD: &str = "\u{00A0}";
/// A private URI scheme keeps file mentions distinguishable from ordinary
/// Markdown links pasted into the composer.
use zeron_proto::file_mentions::{FILE_MENTION_SCHEME, local_file_link, local_path_is_safe};

/// A restorable point in the input's history: text plus where the caret and
/// selection sat when the edit landed.
#[derive(Clone)]
struct EditSnapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    caret_affinity: CaretAffinity,
}

/// A strict, local-only Markdown representation of a file mention. The
/// underlying prompt always contains this form; the editor projects it to a
/// chip for display without leaking a second data model into submission.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileMentionLink {
    range: Range<usize>,
    basename: String,
    path: String,
    is_dir: bool,
    prefix: char,
}

/// Build the text inserted when a workspace item is dropped at an arbitrary
/// selection. Unlike completion, a drop does not necessarily happen at a
/// token boundary, so it supplies its own leading separator when needed.
/// A reference must not introduce whitespace before closing Markdown syntax.
/// Existing horizontal whitespace is reused; line endings stay on their row.
fn reference_suffix(next: Option<char>) -> (&'static str, usize) {
    match next {
        Some('\n' | '\r') => ("", 0),
        Some(ch) if ch.is_whitespace() => ("", ch.len_utf8()),
        Some(')' | ']' | '}' | '*' | '_' | '~' | ',' | '.' | ';' | ':' | '!' | '?') => ("", 0),
        _ => (" ", 0),
    }
}

fn dropped_file_mention(
    content: &str,
    range: Range<usize>,
    path: &str,
    is_dir: bool,
) -> Option<(String, usize)> {
    if range.start > range.end
        || !local_path_is_safe(path)
        || !content.is_char_boundary(range.start)
    {
        return None;
    }
    let suffix = content.get(range.end..)?;
    let prefix = if range.start > 0
        && content[..range.start]
            .chars()
            .next_back()
            .is_some_and(|ch| {
                !ch.is_whitespace() && !matches!(ch, '(' | '[' | '{' | '*' | '_' | '~')
            }) {
        " "
    } else {
        ""
    };
    let (trailing, advance) = reference_suffix(suffix.chars().next());
    let inserted = format!("{prefix}{}{trailing}", local_file_link(path, is_dir));
    let cursor_advance = inserted.len() + advance;
    Some((inserted, cursor_advance))
}

fn file_mention_links(text: &str) -> Vec<FileMentionLink> {
    zeron_proto::file_mentions::file_mention_links(text)
        .into_iter()
        .map(|link| FileMentionLink {
            range: link.range,
            basename: link.basename,
            path: link.path,
            is_dir: link.is_dir,
            prefix: '@',
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
struct TextProjection {
    display: String,
    mentions: Vec<(FileMentionLink, Range<usize>)>,
    mappings: Vec<(Range<usize>, Range<usize>)>,
}

/// A path alone is not enough: two identical relative paths can appear in a
/// draft, so the raw range remains part of the hover identity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MentionTooltipTarget {
    range: Range<usize>,
    path: SharedString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MentionTooltipPhase {
    Hidden,
    Waiting {
        target: MentionTooltipTarget,
        generation: u64,
    },
    Visible {
        target: MentionTooltipTarget,
        generation: u64,
    },
}

impl MentionTooltipPhase {
    fn target(&self) -> Option<&MentionTooltipTarget> {
        match self {
            Self::Hidden => None,
            Self::Waiting { target, .. } | Self::Visible { target, .. } => Some(target),
        }
    }
}

/// Pure tooltip lifecycle reducer. Motion within the same chip preserves both
/// waiting and visible phases, so normal pointer jitter cannot starve the
/// delay or flicker an already-visible tooltip.
fn mention_tooltip_reduce(
    phase: MentionTooltipPhase,
    pointer_target: Option<MentionTooltipTarget>,
    pointer_in_popup: bool,
    generation: u64,
) -> MentionTooltipPhase {
    match pointer_target {
        Some(target) if phase.target() == Some(&target) => phase,
        Some(target) => MentionTooltipPhase::Waiting { target, generation },
        None if pointer_in_popup && matches!(phase, MentionTooltipPhase::Visible { .. }) => phase,
        None => MentionTooltipPhase::Hidden,
    }
}

fn mention_tooltip_promote(
    phase: MentionTooltipPhase,
    generation: u64,
    target_is_live: bool,
) -> MentionTooltipPhase {
    match phase {
        MentionTooltipPhase::Waiting {
            target,
            generation: current,
        } if current == generation && target_is_live => MentionTooltipPhase::Visible {
            target,
            generation: current,
        },
        MentionTooltipPhase::Waiting {
            generation: current,
            ..
        } if current == generation => MentionTooltipPhase::Hidden,
        phase => phase,
    }
}

fn mention_tooltip_contains(in_chip: bool, in_popup: bool) -> bool {
    in_chip || in_popup
}

fn display_row_segments(
    range: Range<usize>,
    row_ends: impl IntoIterator<Item = usize>,
) -> Vec<(usize, usize, Range<usize>)> {
    let mut segments = Vec::new();
    let mut row_start = 0usize;
    for (row_ix, row_end) in row_ends.into_iter().enumerate() {
        let start = range.start.max(row_start);
        let end = range.end.min(row_end);
        if start < end {
            segments.push((row_ix, row_start, start..end));
        }
        row_start = row_end;
        if row_start >= range.end {
            break;
        }
    }
    segments
}

#[derive(Debug, Clone)]
struct MentionHit {
    target: MentionTooltipTarget,
    bounds: Bounds<Pixels>,
    anchor: Point<Pixels>,
}

/// A chip is a compact identity, with the complete path available in its tooltip.
/// Truncate by grapheme so emoji and combining marks remain intact.
fn compact_chip_label(label: &str) -> String {
    compact_chip_label_with_limit(label, 32)
}

fn compact_chip_label_with_limit(label: &str, limit: usize) -> String {
    let graphemes: Vec<_> = label.graphemes(true).collect();
    if graphemes.len() <= limit {
        return label.to_string();
    }
    format!(
        "{}…{}",
        graphemes[..limit - 14].concat(),
        graphemes[graphemes.len() - 12..].concat()
    )
}

/// Preserve distinctions established by path disambiguation when shortening.
fn compact_chip_labels(labels: &[String]) -> Vec<String> {
    let mut shortened: Vec<_> = labels
        .iter()
        .map(|label| compact_chip_label(label))
        .collect();
    let mut limit = 32usize;
    loop {
        let mut distinct: HashMap<&str, std::collections::HashSet<&str>> = HashMap::new();
        for (shown, full) in shortened.iter().zip(labels) {
            distinct.entry(shown).or_default().insert(full);
        }
        let ambiguous: Vec<_> = shortened
            .iter()
            .enumerate()
            .filter_map(|(ix, shown)| (distinct[shown.as_str()].len() > 1).then_some(ix))
            .collect();
        if ambiguous.is_empty() {
            return shortened;
        }
        limit = limit.saturating_mul(2);
        for ix in ambiguous {
            shortened[ix] = compact_chip_label_with_limit(&labels[ix], limit);
        }
    }
}

/// GPUI's text wrapper permits breaks at Unicode spacer glyphs. Reference
/// chips need stronger boundaries than ordinary prose, without changing ZUI.
fn wrap_reference_chips(line: &mut WrappedLine, chips: &[Range<usize>], width: Pixels) {
    if chips.is_empty() {
        return;
    }
    let glyphs: Vec<_> = line
        .runs()
        .iter()
        .enumerate()
        .flat_map(|(run_ix, run)| {
            run.glyphs.iter().enumerate().map(move |(glyph_ix, glyph)| {
                (
                    glyph.index,
                    glyph.position.x,
                    gpui::WrapBoundary { run_ix, glyph_ix },
                )
            })
        })
        .collect();
    let inside = |index| {
        chips
            .iter()
            .any(|range| range.start < index && index < range.end)
    };
    // Keep complete references together; oversized references can still wrap.
    let mut boundaries = line.wrap_boundaries.clone();
    boundaries.clear();
    let mut row = 0;
    while row < glyphs.len() {
        let limit = glyphs[row].1 + width;
        let overflow = (row..glyphs.len()).find(|&i| {
            glyphs
                .get(i + 1)
                .map_or(line.unwrapped_layout.width, |g| g.1)
                > limit
        });
        let Some(overflow) = overflow else {
            break;
        };
        // Prefer whitespace or a complete chip boundary. If an individual
        // token is wider than the viewport, retain the usual emergency wrap.
        let preferred = (row + 1..=overflow).rev().find(|&i| {
            let index = glyphs[i].0;
            !inside(index)
                && (chips.iter().any(|r| r.start == index || r.end == index)
                    || line.text[..index]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_whitespace))
        });
        let at = preferred
            .or_else(|| (row + 1..=overflow).rev().find(|&i| !inside(glyphs[i].0)))
            .unwrap_or(overflow.max(row + 1));
        if at >= glyphs.len() {
            break;
        }
        boundaries.push(glyphs[at].2);
        row = at;
    }
    let layout = gpui::WrappedLineLayout {
        unwrapped_layout: line.unwrapped_layout.clone(),
        wrap_boundaries: boundaries,
        wrap_width: line.wrap_width,
    };
    *std::ops::DerefMut::deref_mut(line) = std::sync::Arc::new(layout);
}

impl TextProjection {
    fn new(raw: &str) -> Self {
        Self::project(raw, None, false)
    }

    fn rich(raw: &str, active: Option<Range<usize>>) -> Self {
        Self::project(raw, active, true)
    }

    fn project(raw: &str, active: Option<Range<usize>>, compact: bool) -> Self {
        let mut links = file_mention_links(raw);
        links.extend(
            zeron_proto::invocation::invocation_links(raw)
                .into_iter()
                .map(|(range, invocation)| FileMentionLink {
                    range,
                    basename: if compact && invocation.prefix() == '$' {
                        skill_display_name(invocation.name())
                    } else {
                        invocation.name().to_string()
                    },
                    path: invocation.detail(),
                    is_dir: false,
                    prefix: invocation.prefix(),
                }),
        );
        links.sort_by_key(|link| link.range.start);
        let labels = mention_display_labels(&links);
        let labels = if compact {
            compact_chip_labels(&labels)
        } else {
            labels
        };
        let mut projection = Self::default();
        let mut edits: Vec<(Range<usize>, String, Option<FileMentionLink>)> = links
            .into_iter()
            .zip(labels)
            .map(|(link, label)| {
                let label = label.replace(' ', "\u{00A0}");
                let marker = link.prefix;
                let pad = MENTION_SIDE_PAD;
                (
                    link.range.clone(),
                    format!("{pad}{marker}{label}{pad}"),
                    Some(link),
                )
            })
            .collect();
        if let Some(active) = active {
            let links: Vec<_> = edits.iter().map(|(range, _, _)| range.clone()).collect();
            for (range, replacement) in composer_markdown::decorations(raw, active) {
                let candidate = links.partition_point(|link| link.end <= range.start);
                if !links
                    .get(candidate)
                    .is_some_and(|link| link.start < range.end)
                {
                    edits.push((range, replacement, None));
                }
            }
        }
        edits.sort_by_key(|(r, _, _)| r.start);
        let mut raw_at = 0;
        for (range, replacement, link) in edits {
            if range.start < raw_at {
                continue;
            }
            projection.display.push_str(&raw[raw_at..range.start]);
            let start = projection.display.len();
            projection.display.push_str(&replacement);
            let display = start..projection.display.len();
            if let Some(link) = link {
                projection.mentions.push((link, display.clone()));
            }
            raw_at = range.end;
            projection.mappings.push((range, display));
        }
        projection.display.push_str(&raw[raw_at..]);
        projection
    }

    fn raw_to_display(&self, raw: usize) -> usize {
        let ix = self.mappings.partition_point(|(range, _)| range.end <= raw);
        if let Some((range, display)) = self.mappings.get(ix) {
            if raw > range.start {
                return display.start;
            }
        }
        let (raw_at, display_at) = ix
            .checked_sub(1)
            .map(|previous| (&self.mappings[previous].0, &self.mappings[previous].1))
            .map_or((0, 0), |(range, display)| (range.end, display.end));
        display_at + raw.saturating_sub(raw_at)
    }

    fn display_to_raw(&self, display_offset: usize) -> usize {
        // Equal zero-width boundaries are consumed together so clicks on the
        // first rendered character land after nested hidden opening markers.
        let ix = self
            .mappings
            .partition_point(|(_, display)| display.end <= display_offset);
        if let Some((range, display)) = self.mappings.get(ix) {
            if display_offset > display.start {
                return if display_offset - display.start < display.len() / 2 {
                    range.start
                } else {
                    range.end
                };
            }
        }
        let (raw_at, display_at) = ix
            .checked_sub(1)
            .map(|previous| (&self.mappings[previous].0, &self.mappings[previous].1))
            .map_or((0, 0), |(range, display)| (range.end, display.end));
        raw_at + display_offset.saturating_sub(display_at)
    }

    fn normalize_range(&self, range: Range<usize>) -> Range<usize> {
        if range.is_empty() {
            for (link, _) in &self.mentions {
                if link.range.start < range.start && range.start < link.range.end {
                    let midpoint = link.range.start + link.range.len() / 2;
                    let at = if range.start < midpoint {
                        link.range.start
                    } else {
                        link.range.end
                    };
                    return at..at;
                }
            }
            return range;
        }
        let mut normalized = range;
        for (link, _) in &self.mentions {
            if normalized.start < link.range.end && normalized.end > link.range.start {
                normalized.start = normalized.start.min(link.range.start);
                normalized.end = normalized.end.max(link.range.end);
            }
        }
        normalized
    }

    fn previous_boundary(&self, raw: usize) -> Option<usize> {
        self.mentions
            .iter()
            .find_map(|(link, _)| (raw == link.range.end).then_some(link.range.start))
    }

    fn next_boundary(&self, raw: usize) -> Option<usize> {
        self.mentions
            .iter()
            .find_map(|(link, _)| (raw == link.range.start).then_some(link.range.end))
    }
}

/// Basenames are compact in the common case. When the same basename appears
/// more than once, use the shortest unique path suffix so chips remain
/// distinguishable without always expanding to full paths.
fn mention_display_labels(links: &[FileMentionLink]) -> Vec<String> {
    // Only references with the same visible name can need disambiguation.
    // Repeated references share the result instead of rescanning the draft.
    let mut groups: HashMap<(char, &str), Vec<&FileMentionLink>> = HashMap::new();
    for link in links {
        groups
            .entry((link.prefix, &link.basename))
            .or_default()
            .push(link);
    }
    let mut labels: HashMap<(char, &str, &str), String> = HashMap::new();
    links
        .iter()
        .map(|link| {
            labels
                .entry((link.prefix, &link.basename, &link.path))
                .or_insert_with(|| {
                    let duplicates: Vec<_> = groups[&(link.prefix, link.basename.as_str())]
                        .iter()
                        .filter(|other| other.path != link.path)
                        .collect();
                    if duplicates.is_empty() {
                        return link.basename.clone();
                    }
                    let parts: Vec<_> = link.path.split('/').collect();
                    let suffix = (1..=parts.len())
                        .map(|count| parts[parts.len() - count..].join("/"))
                        .find(|suffix| {
                            if link.prefix != '@' {
                                duplicates.iter().all(|other| !other.path.ends_with(suffix))
                            } else {
                                let suffix: Vec<_> = suffix.split('/').collect();
                                duplicates.iter().all(|other| {
                                    !other
                                        .path
                                        .split('/')
                                        .rev()
                                        .take(suffix.len())
                                        .eq(suffix.iter().rev().copied())
                                })
                            }
                        })
                        .unwrap_or_else(|| link.path.clone());
                    if link.prefix == '@' {
                        suffix
                    } else {
                        format!("{} · {suffix}", link.basename)
                    }
                })
                .clone()
        })
        .collect()
}

/// One chip in a *sent* message: its byte range over the projected display
/// string (`@label` between side bearings). The transcript renders these
/// read-only — no editing state, no tooltip machinery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMentionSpan {
    pub range: Range<usize>,
    /// Full workspace-relative path (labels can be shortened to basenames).
    pub path: SharedString,
    pub is_dir: bool,
}

/// Project a sent message's raw Markdown for transcript display: mention links
/// collapse to the same chip labels the composer shows, everything else passes
/// through untouched. `None` when the text has no valid mention — the
/// substring probe keeps ordinary prompts on the zero-allocation path, so this
/// is safe to call for every user row.
pub fn sent_mention_display(raw: &str) -> Option<(String, Vec<SentMentionSpan>)> {
    if !raw.contains(FILE_MENTION_SCHEME)
        && !raw.contains(zeron_proto::invocation::INVOCATION_SCHEME)
    {
        return None;
    }
    let projection = TextProjection::new(raw);
    if projection.mentions.is_empty() {
        return None;
    }
    let spans = projection
        .mentions
        .iter()
        .map(|(link, display)| SentMentionSpan {
            range: display.clone(),
            path: SharedString::from(format!(
                "{}{}",
                link.path,
                if link.is_dir { "/" } else { "" }
            )),
            is_dir: link.is_dir,
        })
        .collect();
    Some((projection.display, spans))
}

/// Direction of the last edit — a run only merges with edits of its own kind.
#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Insert,
    Delete,
}

const GENERIC_COMPOSER_CONTEXT: &str = "Composer";
const MESSAGE_COMPOSER_CONTEXT: &str = "MessageComposer";
const PALETTE_SEARCH_CONTEXT: &str = "PaletteSearch";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageEnterBindingAction {
    Submit,
    ModifiedSubmit,
    NewlineOrAccept,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageEnterBinding {
    keystroke: String,
    action: MessageEnterBindingAction,
}

fn message_enter_bindings(
    behavior: ComposerSendBehavior,
    modifier_combo: &str,
) -> Vec<MessageEnterBinding> {
    match behavior {
        ComposerSendBehavior::Enter => vec![
            MessageEnterBinding {
                keystroke: "enter".into(),
                action: MessageEnterBindingAction::Submit,
            },
            MessageEnterBinding {
                keystroke: modifier_combo.into(),
                action: MessageEnterBindingAction::ModifiedSubmit,
            },
        ],
        ComposerSendBehavior::ModEnter => vec![
            MessageEnterBinding {
                keystroke: "enter".into(),
                action: MessageEnterBindingAction::NewlineOrAccept,
            },
            MessageEnterBinding {
                keystroke: modifier_combo.into(),
                action: MessageEnterBindingAction::ModifiedSubmit,
            },
        ],
    }
}

fn message_input_context(wizard_active: bool) -> &'static str {
    if wizard_active {
        GENERIC_COMPOSER_CONTEXT
    } else {
        MESSAGE_COMPOSER_CONTEXT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnterOutcome {
    AcceptCompletion,
    Submit,
    Newline,
}

fn enter_outcome(has_completion: bool, fallback: EnterOutcome) -> EnterOutcome {
    if has_completion {
        EnterOutcome::AcceptCompletion
    } else {
        fallback
    }
}

fn input_bindings(context: &'static str) -> Vec<KeyBinding> {
    let ctx = Some(context);
    let mut bindings = vec![
        KeyBinding::new("tab", MentionTab, ctx),
        KeyBinding::new("shift-tab", OutdentList, ctx),
        KeyBinding::new("shift-enter", Newline, ctx),
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("shift-home", SelectHome, ctx),
        KeyBinding::new("shift-end", SelectEnd, ctx),
        // macOS line/document motion — a laptop keyboard has no home/end keys,
        // so Cmd+arrow is the only way users reach either edge.
        KeyBinding::new("cmd-left", Home, ctx),
        KeyBinding::new("cmd-right", End, ctx),
        KeyBinding::new("cmd-up", DocStart, ctx),
        KeyBinding::new("cmd-down", DocEnd, ctx),
        KeyBinding::new("shift-cmd-left", SelectHome, ctx),
        KeyBinding::new("shift-cmd-right", SelectEnd, ctx),
        KeyBinding::new("shift-cmd-up", SelectDocStart, ctx),
        KeyBinding::new("shift-cmd-down", SelectDocEnd, ctx),
        // Line-edge deletion (Cmd+Delete on macOS).
        KeyBinding::new("cmd-backspace", DeleteToLineStart, ctx),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, ctx),
    ];
    for prefix in ["cmd", "ctrl"] {
        bindings.push(KeyBinding::new(&format!("{prefix}-z"), Undo, ctx));
        bindings.push(KeyBinding::new(&format!("shift-{prefix}-z"), Redo, ctx));
    }
    // Word-level editing: Option on macOS, Ctrl on Windows/Linux.
    let word_edit_prefix = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-backspace"),
        DeleteWordLeft,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-delete"),
        DeleteWordRight,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-left"),
        WordLeft,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-right"),
        WordRight,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-left"),
        SelectWordLeft,
        ctx,
    ));
    bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-right"),
        SelectWordRight,
        ctx,
    ));
    for prefix in ["cmd", "ctrl"] {
        bindings.push(KeyBinding::new(&format!("{prefix}-a"), SelectAll, ctx));
        bindings.push(KeyBinding::new(&format!("{prefix}-c"), Copy, ctx));
        bindings.push(KeyBinding::new(&format!("{prefix}-x"), Cut, ctx));
        bindings.push(KeyBinding::new(&format!("{prefix}-v"), Paste, ctx));
    }
    bindings
}

/// Bind the composer keymap. Call once at app boot.
pub fn init(cx: &mut App, send_behavior: ComposerSendBehavior) {
    let mut generic_bindings = input_bindings(GENERIC_COMPOSER_CONTEXT);
    generic_bindings.push(KeyBinding::new(
        "enter",
        Submit,
        Some(GENERIC_COMPOSER_CONTEXT),
    ));

    let mut message_bindings = input_bindings(MESSAGE_COMPOSER_CONTEXT);
    for binding in message_enter_bindings(send_behavior, &platform_combo("mod-enter")) {
        match binding.action {
            MessageEnterBindingAction::Submit => message_bindings.push(KeyBinding::new(
                &binding.keystroke,
                Submit,
                Some(MESSAGE_COMPOSER_CONTEXT),
            )),
            MessageEnterBindingAction::ModifiedSubmit => message_bindings.push(KeyBinding::new(
                &binding.keystroke,
                ModifiedSubmit,
                Some(MESSAGE_COMPOSER_CONTEXT),
            )),
            MessageEnterBindingAction::NewlineOrAccept => message_bindings.push(KeyBinding::new(
                &binding.keystroke,
                MessageNewlineOrAccept,
                Some(MESSAGE_COMPOSER_CONTEXT),
            )),
        }
    }

    let word_edit_prefix = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    // Palette-search context: TEXT-EDITING keys only. gpui dispatches matched
    // keybindings BEFORE raw key listeners (window.rs `dispatch_key_event`),
    // so anything bound here can never reach a palette's `on_key_down` —
    // navigation keys (up/down/left/right/enter) are deliberately unbound and
    // bubble to the palette frame instead.
    let palette = Some(PALETTE_SEARCH_CONTEXT);
    let mut palette_bindings = vec![
        KeyBinding::new("backspace", Backspace, palette),
        KeyBinding::new("delete", Delete, palette),
        KeyBinding::new("home", Home, palette),
        KeyBinding::new("end", End, palette),
        KeyBinding::new("shift-left", SelectLeft, palette),
        KeyBinding::new("shift-right", SelectRight, palette),
        // Modifier-qualified motion is safe here: the palette's own navigation
        // uses BARE arrows/enter, which stay unbound and bubble to its frame.
        KeyBinding::new("cmd-left", Home, palette),
        KeyBinding::new("cmd-right", End, palette),
        KeyBinding::new("shift-cmd-left", SelectHome, palette),
        KeyBinding::new("shift-cmd-right", SelectEnd, palette),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, palette),
    ];
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-backspace"),
        DeleteWordLeft,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-delete"),
        DeleteWordRight,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-left"),
        WordLeft,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("{word_edit_prefix}-right"),
        WordRight,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-left"),
        SelectWordLeft,
        palette,
    ));
    palette_bindings.push(KeyBinding::new(
        &format!("shift-{word_edit_prefix}-right"),
        SelectWordRight,
        palette,
    ));
    for prefix in ["cmd", "ctrl"] {
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-a"), SelectAll, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-c"), Copy, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-x"), Cut, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-v"), Paste, palette));
        palette_bindings.push(KeyBinding::new(&format!("{prefix}-z"), Undo, palette));
        palette_bindings.push(KeyBinding::new(&format!("shift-{prefix}-z"), Redo, palette));
    }
    cx.bind_keys(palette_bindings);
    cx.bind_keys(generic_bindings);
    cx.bind_keys(message_bindings);
}

/// Events the composer wrapper listens for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposerInputEvent {
    Submitted,
    ModifiedSubmitted,
    Edited,
    CursorMoved,
    ViewportChanged,
    MentionNavigate(isize),
    MentionAccept,
    MentionDismiss,
    /// Images pasted from the clipboard (screenshots / copied image data) —
    /// the wrapper stages them as attachments (use-attachments.ts onPaste).
    PastedImages(Vec<gpui::Image>),
    /// File paths pasted from the clipboard (a file manager "Copy").
    PastedPaths(Vec<PathBuf>),
    PastedText {
        range: Range<usize>,
        revision: u64,
    },
}

/// Shaping inputs excluding mutable viewport and selection geometry.
#[derive(Clone, PartialEq)]
struct InputLayoutKey {
    width: Pixels,
    font: gpui::Font,
    font_size: Pixels,
    color: gpui::Hsla,
    chip_family: SharedString,
    chip_color: gpui::Hsla,
    syntax: crate::theme::SyntaxPalette,
    marked_range: Option<Range<usize>>,
    placeholder: SharedString,
    mentions_enabled: bool,
    active_line: Option<Range<usize>>,
}

/// A soft-wrap boundary is both the previous row's end and the next row's
/// start. Keep the visual side of the caret separately from its source offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CaretAffinity {
    Upstream,
    #[default]
    Downstream,
}

/// Multiline input entity: content + selection + IME marked text + measured
/// layout (wrapped lines) for mouse mapping and auto-grow.
pub struct ComposerInput {
    /// Key context for the binding map ("Composer", or "PaletteSearch" for
    /// palette filters whose navigation keys must bubble).
    key_context: &'static str,
    accessibility_role: Role,
    focus_handle: FocusHandle,
    content: String,
    edit_revision: u64,
    pub(crate) read_only: bool,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    caret_affinity: CaretAffinity,
    marked_range: Option<Range<usize>>,
    /// Retained through vertical moves across short rows; other moves reset it.
    preferred_column: Option<Pixels>,
    is_selecting: bool,
    drag_position: Option<Point<Pixels>>,
    drag_generation: u64,
    drag_unit: Option<(PressIntent, Range<usize>)>,
    drag_autoscroll_active: bool,
    /// Vertical scroll inside the input once content exceeds the max height.
    scroll_top: f32,
    /// Visible content budget supplied by the animated composer.
    viewport_height: Option<f32>,
    /// Final content budget, excluding temporary overflow during a resize.
    settled_viewport_height: Option<f32>,
    resizing: bool,
    overflow_top_padding: f32,
    needs_measure: bool,
    last_layout_key: Option<InputLayoutKey>,
    last_notified_layout: Option<(Pixels, f32)>,
    max_ascent: f32,
    #[cfg(test)]
    layout_rebuilds: usize,
    /// Normally keeps the caret visible through edits and rewraps. Manual
    /// wheel scrolling pauses it until the next caret move or edit.
    follow_cursor: bool,
    text_size: f32,
    configured_line_height: f32,
    single_line: bool,
    scroll_left: f32,
    // -- measured state (written during layout/paint) --
    last_lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    line_indents: Vec<Pixels>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    content_height: f32,
    max_line_width: f32,
    last_width: f32,
    /// Raw Markdown → chip display projection from the last layout pass.
    projection: TextProjection,
    syntax_source: String,
    syntax_spans: Vec<zeron_syntax::HighlightSpan>,
    syntax_task: Option<Task<()>>,
    /// Inline completion preview: painted in faint ink after the text while
    /// the caret sits at the end (palette tab-completion). Owned by the
    /// wrapper — it recomputes and re-sets this on every render pass, so the
    /// input never has to know what the completion means.
    ghost: Option<SharedString>,
    /// File mentions are a composer feature, not a behavior of generic inputs
    /// (picker searches and rename fields also use this type).
    mentions_enabled: bool,
    /// Bumped once per `layout_text` pass — the flip logic uses it to apply at
    /// most one compact↔expanded flip per layout (a flip is only re-evaluated
    /// after the input has been measured in the new mode).
    layout_epoch: u64,
    display_is_placeholder: bool,
    /// Caret blink anchor: reset on every keystroke/caret move so the caret is
    /// solid while typing and blinks at [`CARET_BLINK_MS`] when idle.
    blink_anchor: Instant,
    /// Half-period repaint driver, alive only while the input is focused.
    blink_task: Option<Task<()>>,
    // -- undo history --
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    /// Kind, trailing offset, and time of the last edit — the merge test that
    /// decides whether the next edit extends the current undo step.
    last_edit: Option<(EditKind, usize, Instant)>,
    /// The wrapper owns mention state; this only redirects bound keys while a
    /// mention token is active, keeping input focus and native text editing.
    mention_open: bool,
    mention_has_selection: bool,
    /// Last prepainted chip bounds; the paint-phase pointer listener uses
    /// these instead of attempting to infer text geometry from the cursor.
    mention_hits: Vec<MentionHit>,
    mention_tooltip: MentionTooltipPhase,
    mention_tooltip_generation: u64,
    mention_tooltip_popup: Option<Bounds<Pixels>>,
    mention_tooltip_task: Option<Task<()>>,
    /// Created once when Waiting promotes; retaining this entity preserves
    /// GPUI's global animation state across prepaint frames.
    mention_tooltip_view: Option<Entity<MentionPathTooltip>>,
}

impl ComposerInput {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self::with_context(placeholder, GENERIC_COMPOSER_CONTEXT, cx)
    }

    /// An input in a custom KEY context — palettes use `"PaletteSearch"`,
    /// whose keymap binds only text-editing keys so navigation keys bubble to
    /// the surrounding frame (see `init`).
    pub fn with_context(
        placeholder: impl Into<SharedString>,
        key_context: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            key_context,
            accessibility_role: Role::MultilineTextInput,
            focus_handle: cx.focus_handle(),
            content: String::new(),
            edit_revision: 0,
            read_only: false,
            placeholder: placeholder.into(),
            selected_range: 0..0,
            selection_reversed: false,
            caret_affinity: CaretAffinity::Downstream,
            marked_range: None,
            preferred_column: None,
            is_selecting: false,
            drag_position: None,
            drag_generation: 0,
            drag_unit: None,
            drag_autoscroll_active: false,
            scroll_top: 0.0,
            viewport_height: None,
            settled_viewport_height: None,
            resizing: false,
            overflow_top_padding: 0.0,
            needs_measure: true,
            last_layout_key: None,
            last_notified_layout: None,
            max_ascent: INPUT_TEXT_SIZE,
            #[cfg(test)]
            layout_rebuilds: 0,
            follow_cursor: true,
            text_size: INPUT_TEXT_SIZE,
            configured_line_height: INPUT_LINE_HEIGHT,
            single_line: false,
            scroll_left: 0.0,
            last_lines: Vec::new(),
            line_starts: vec![0],
            line_indents: Vec::new(),
            last_bounds: None,
            line_height: px(INPUT_LINE_HEIGHT),
            content_height: INPUT_LINE_HEIGHT,
            max_line_width: 0.0,
            last_width: 0.0,
            projection: TextProjection::default(),
            syntax_source: String::new(),
            syntax_spans: Vec::new(),
            syntax_task: None,
            ghost: None,
            mentions_enabled: false,
            layout_epoch: 0,
            display_is_placeholder: true,
            blink_anchor: Instant::now(),
            blink_task: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: None,
            mention_open: false,
            mention_has_selection: false,
            mention_hits: Vec::new(),
            mention_tooltip: MentionTooltipPhase::Hidden,
            mention_tooltip_generation: 0,
            mention_tooltip_popup: None,
            mention_tooltip_task: None,
            mention_tooltip_view: None,
        }
    }

    /// Override the text metrics for compact one-line surfaces such as
    /// toolbar searches without changing the main composer typography.
    pub fn with_text_metrics(mut self, text_size: f32, line_height: f32) -> Self {
        self.text_size = text_size;
        self.configured_line_height = line_height;
        self.line_height = px(line_height);
        self.content_height = line_height;
        self
    }

    /// Keep compact fields on one row and reveal the caret horizontally.
    pub fn with_single_line(mut self) -> Self {
        self.single_line = true;
        self
    }

    fn set_key_context(&mut self, key_context: &'static str, cx: &mut Context<Self>) {
        if self.key_context != key_context {
            self.key_context = key_context;
            cx.notify();
        }
    }

    pub fn with_accessibility_role(mut self, role: Role) -> Self {
        self.accessibility_role = role;
        self
    }

    /// Reset the caret blink phase (solid again) — called on every edit and
    /// caret move, matching textarea behavior.
    fn reset_blink(&mut self) {
        self.blink_anchor = Instant::now();
    }

    /// Caret paint gate: focused input in an active window, in the "on" blink
    /// phase. Also (re)arms the half-period repaint driver while focused, and
    /// drops it on blur so an unfocused input schedules no frames.
    fn caret_shown(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        let focused = self.focus_handle.is_focused(window);
        if !focused || !window.is_window_active() {
            self.blink_task = None;
            return false;
        }
        if self.blink_task.is_none() {
            self.blink_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(CARET_BLINK_MS))
                        .await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
        caret_visible(self.blink_anchor.elapsed().as_millis() as u64)
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_mention_controls(
        &mut self,
        open: bool,
        has_selection: bool,
        cx: &mut Context<Self>,
    ) {
        if self.mention_open == open && self.mention_has_selection == has_selection {
            return;
        }
        self.mention_open = open;
        self.mention_has_selection = has_selection;
        cx.notify();
    }

    fn enable_mentions(&mut self) {
        self.mentions_enabled = true;
        self.refresh_projection();
    }

    fn editing_source_range(&self) -> Range<usize> {
        let selected = self.selected_range.clone();
        let range = self
            .marked_range
            .as_ref()
            .map_or(selected.clone(), |marked| {
                selected.start.min(marked.start)..selected.end.max(marked.end)
            });
        self.line_range_at(range.start).start..self.line_range_at(range.end).end
    }

    fn refresh_projection(&mut self) {
        self.projection = if self.mentions_enabled {
            TextProjection::rich(&self.content, Some(self.editing_source_range()))
        } else {
            TextProjection {
                display: self.content.clone(),
                mentions: Vec::new(),
                mappings: Vec::new(),
            }
        };
    }

    /// Replace a completed `@query` token as one non-coalescing undo step.
    pub fn replace_mention(
        &mut self,
        range: Range<usize>,
        path: &str,
        is_dir: bool,
        cx: &mut Context<Self>,
    ) {
        if self.read_only || self.marked_range.is_some() || !local_path_is_safe(path) {
            return;
        }
        self.invalidate_mention_tooltip();
        let path = local_file_link(path, is_dir);
        let (trailing, advance) = reference_suffix(self.content[range.end..].chars().next());
        let inserted = format!("{path}{trailing}");
        self.record_edit(&range, &inserted);
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content =
            self.content[..range.start].to_owned() + &inserted + &self.content[range.end..];
        let cursor = range.start + inserted.len() + advance;
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.refresh_projection();
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    /// Insert a workspace reference at the current selection. Drag-and-drop
    /// uses the same strict local Markdown transport and projected chip as an
    /// `@` mention selected from completion.
    fn insert_dropped_mention(&mut self, path: &str, is_dir: bool, cx: &mut Context<Self>) -> bool {
        // The platform still owns the marked range during IME composition.
        // Inserting a chip into it would leave those offsets pointing inside
        // the new reference, and the next IME update could delete the chip.
        if self.read_only || self.marked_range.is_some() {
            return false;
        }
        let range = self.selected_range.clone();
        let Some((inserted, cursor_advance)) =
            dropped_file_mention(&self.content, range.clone(), path, is_dir)
        else {
            return false;
        };
        self.invalidate_mention_tooltip();
        self.record_edit(&range, &inserted);
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content =
            self.content[..range.start].to_owned() + &inserted + &self.content[range.end..];
        let cursor = range.start + cursor_advance;
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.refresh_projection();
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
        true
    }

    /// Replace a completed plain-text token (slash commands) as one
    /// non-coalescing undo step. Unlike [`Self::replace_mention`], the
    /// replacement is ordinary text — no link, no chip projection.
    pub fn replace_plain_token(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let (trailing, advance) = reference_suffix(self.content[range.end..].chars().next());
        let inserted = format!("{replacement}{trailing}");
        self.record_edit(&range, &inserted);
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content =
            self.content[..range.start].to_owned() + &inserted + &self.content[range.end..];
        let cursor = range.start + inserted.len() + advance;
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.refresh_projection();
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn remove_completion_token(&mut self, mut range: Range<usize>, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.content[..range.start].trim().is_empty()
            && self.content[range.end..].trim().is_empty()
        {
            range = 0..self.content.len();
        } else if (range.start == 0 || self.content[..range.start].ends_with(' '))
            && self.content[range.end..].starts_with(' ')
        {
            range.end += 1;
        }
        self.last_edit = None;
        self.record_edit(&range, "");
        self.last_edit = None;
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content.replace_range(range.clone(), "");
        self.selected_range = range.start..range.start;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.refresh_projection();
        self.follow_cursor = true;
        self.needs_measure = true;
        self.reset_blink();
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    /// Set (or clear) the inline completion preview. Only paints while the
    /// caret sits at the end of a non-empty draft — see the prepaint gate.
    pub fn set_ghost(&mut self, ghost: Option<SharedString>, cx: &mut Context<Self>) {
        if self.ghost == ghost {
            return;
        }
        self.ghost = ghost;
        cx.notify();
    }

    pub fn has_newline(&self) -> bool {
        self.content.contains('\n')
    }

    /// Unwrapped width of the widest line — feeds the compact/expanded flip.
    pub fn measured_text_width(&self) -> f32 {
        self.max_line_width
    }

    pub fn measured_content_height(&self) -> f32 {
        self.content_height
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.invalidate_mention_tooltip();
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content = text.into();
        if self.single_line {
            self.content = self.content.replace(['\r', '\n'], " ");
        }
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.marked_range = None;
        self.scroll_top = 0.0;
        self.scroll_left = 0.0;
        self.refresh_projection();
        self.follow_cursor = true;
        // Programmatic replacement (draft load, clear-on-submit) is a new
        // document, not an edit — undo must not reach back past it.
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit = None;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn invalidate_mention_tooltip(&mut self) {
        self.mention_tooltip_generation = self.mention_tooltip_generation.wrapping_add(1);
        self.mention_tooltip = MentionTooltipPhase::Hidden;
        self.mention_tooltip_popup = None;
        self.mention_tooltip_task = None;
        self.mention_tooltip_view = None;
    }

    fn set_mention_hits(&mut self, hits: Vec<MentionHit>) {
        self.mention_hits = hits;
        let live = self
            .mention_tooltip
            .target()
            .is_none_or(|target| self.mention_hits.iter().any(|hit| &hit.target == target));
        if !live {
            self.invalidate_mention_tooltip();
        }
    }

    fn start_mention_tooltip_wait(&mut self, target: MentionTooltipTarget, cx: &mut Context<Self>) {
        self.mention_tooltip_generation = self.mention_tooltip_generation.wrapping_add(1);
        let generation = self.mention_tooltip_generation;
        self.mention_tooltip = MentionTooltipPhase::Waiting { target, generation };
        self.mention_tooltip_popup = None;
        self.mention_tooltip_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MENTION_TOOLTIP_DELAY).await;
            this.update(cx, |input, cx| {
                let live = input.mention_tooltip.target().is_some_and(|target| {
                    input.mention_hits.iter().any(|hit| &hit.target == target)
                });
                let next = mention_tooltip_promote(input.mention_tooltip.clone(), generation, live);
                if next != input.mention_tooltip {
                    input.mention_tooltip = next;
                    input.mention_tooltip_task = None;
                    if let MentionTooltipPhase::Visible { target, generation } =
                        &input.mention_tooltip
                    {
                        input.mention_tooltip_view = Some(cx.new(|_| MentionPathTooltip {
                            path: target.path.clone(),
                            activation: *generation,
                        }));
                    }
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn on_mention_pointer_move(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.invalidate_mention_tooltip();
            return;
        }
        let target = self
            .mention_hits
            .iter()
            .find(|hit| hit.bounds.contains(&position))
            .map(|hit| hit.target.clone());
        let in_popup = self
            .mention_tooltip_popup
            .is_some_and(|popup| popup.contains(&position));
        let next_generation = self.mention_tooltip_generation.wrapping_add(1);
        let next = mention_tooltip_reduce(
            self.mention_tooltip.clone(),
            target.clone(),
            in_popup,
            next_generation,
        );
        if next == self.mention_tooltip {
            return;
        }
        match next {
            MentionTooltipPhase::Waiting { target, .. } => {
                self.start_mention_tooltip_wait(target, cx)
            }
            _ => {
                self.invalidate_mention_tooltip();
                self.mention_tooltip = next;
                cx.notify();
            }
        }
    }

    fn visible_mention_tooltip(
        &self,
    ) -> Option<(
        MentionTooltipTarget,
        Point<Pixels>,
        u64,
        Entity<MentionPathTooltip>,
    )> {
        let MentionTooltipPhase::Visible { target, generation } = &self.mention_tooltip else {
            return None;
        };
        self.mention_hits
            .iter()
            .find(|hit| hit.target == *target)
            .and_then(|hit| {
                let view = self.mention_tooltip_view.clone()?;
                Some((target.clone(), hit.anchor, *generation, view))
            })
    }

    fn check_mention_tooltip_visibility(
        &mut self,
        popup: Bounds<Pixels>,
        pointer: Point<Pixels>,
    ) -> bool {
        let Some((target, _, _, _)) = self.visible_mention_tooltip() else {
            return false;
        };
        let in_chip = self
            .mention_hits
            .iter()
            .any(|hit| hit.target == target && hit.bounds.contains(&pointer));
        if mention_tooltip_contains(in_chip, popup.contains(&pointer)) {
            self.mention_tooltip_popup = Some(popup);
            true
        } else {
            self.invalidate_mention_tooltip();
            false
        }
    }

    // ---- undo history ----

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
            caret_affinity: self.caret_affinity,
        }
    }

    /// Called with the range about to be replaced, BEFORE the content changes,
    /// so the pushed snapshot is the pre-edit state.
    fn record_edit(&mut self, range: &Range<usize>, new_text: &str) {
        let kind = if new_text.is_empty() {
            EditKind::Delete
        } else {
            EditKind::Insert
        };
        // A run merges only while it stays single-character, contiguous with
        // the previous edit, of the same kind, and inside the idle window. A
        // pause, a word break, a paste, or a caret jump all break the run so
        // undo lands on a boundary the user recognizes.
        let coalescible = match kind {
            EditKind::Insert => range.is_empty() && new_text.chars().count() == 1,
            EditKind::Delete => self
                .content
                .get(range.clone())
                .is_some_and(|text| text.graphemes(true).count() == 1),
        };
        let mergeable = coalescible
            && match (kind, &self.last_edit) {
                (EditKind::Insert, Some((EditKind::Insert, at, when))) => {
                    range.is_empty()
                        && range.start == *at
                        && new_text.chars().count() == 1
                        && !new_text.starts_with(['\n', ' ', '\t'])
                        && when.elapsed() < UNDO_COALESCE
                }
                (EditKind::Delete, Some((EditKind::Delete, at, when))) => {
                    range.end == *at && when.elapsed() < UNDO_COALESCE
                }
                _ => false,
            };
        if !mergeable {
            self.undo_stack.push(self.snapshot());
            if self.undo_stack.len() > UNDO_LIMIT {
                self.undo_stack.remove(0);
            }
        }
        // Any fresh edit invalidates the redo branch.
        self.redo_stack.clear();
        let tail = match kind {
            EditKind::Insert => range.start + new_text.len(),
            EditKind::Delete => range.start,
        };
        self.last_edit = coalescible.then(|| (kind, tail, Instant::now()));
    }

    fn restore(&mut self, snapshot: EditSnapshot, cx: &mut Context<Self>) {
        self.invalidate_mention_tooltip();
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content = snapshot.content;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.caret_affinity = snapshot.caret_affinity;
        self.preferred_column = None;
        self.marked_range = None;
        self.refresh_projection();
        self.follow_cursor = true;
        // Never merge a subsequent edit into a step that undo just crossed.
        self.last_edit = None;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(previous) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.restore(previous, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        self.restore(next, cx);
    }

    // ---- editing ops ----

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.last_edit = None;
        let offset = self.projection.normalize_range(offset..offset).start;
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.follow_cursor = true;
        self.reset_blink();
        cx.emit(ComposerInputEvent::CursorMoved);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.last_edit = None;
        self.extend_selection(offset, cx);
    }

    // Deletion extends the selection internally without breaking a typing run.
    fn extend_selection(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        let offset = self.projection.normalize_range(offset..offset).start;
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.follow_cursor = true;
        self.reset_blink();
        cx.emit(ComposerInputEvent::CursorMoved);
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        if let Some(boundary) = self.projection.previous_boundary(offset) {
            return boundary;
        }
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(ix, _)| (ix < offset).then_some(ix))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        if let Some(boundary) = self.projection.next_boundary(offset) {
            return boundary;
        }
        self.content
            .grapheme_indices(true)
            .find_map(|(ix, _)| (ix > offset).then_some(ix))
            .unwrap_or(self.content.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        if let Some(boundary) = self.projection.previous_boundary(offset) {
            return boundary;
        }
        self.content
            .split_word_bound_indices()
            .rev()
            .find_map(|(ix, word)| (ix < offset && !word.trim().is_empty()).then_some(ix))
            .unwrap_or(0)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        if let Some(boundary) = self.projection.next_boundary(offset) {
            return boundary;
        }
        self.content
            .split_word_bound_indices()
            .find_map(|(ix, word)| {
                let end = ix + word.len();
                (end > offset && !word.trim().is_empty()).then_some(end)
            })
            .unwrap_or(self.content.len())
    }

    /// Byte range of the logical line containing `offset`.
    fn line_range_at(&self, offset: usize) -> Range<usize> {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        let start = self.content[..offset]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
        let end = self.content[offset..]
            .find('\n')
            .map(|i| offset + i)
            .unwrap_or(self.content.len());
        start..end
    }

    /// Navigation stops before the whole line ending. The raw line range
    /// intentionally retains CR for full-line selection and block edits.
    fn line_content_end_at(&self, offset: usize) -> usize {
        let end = self.line_range_at(offset).end;
        if self.content.as_bytes().get(end) == Some(&b'\n')
            && end > 0
            && self.content.as_bytes()[end - 1] == b'\r'
        {
            end - 1
        } else {
            end
        }
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                return;
            }
            self.extend_selection(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.extend_selection(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            self.move_to(prev, cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.selected_range.end);
            self.move_to(next, cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.mention_has_selection {
            cx.emit(ComposerInputEvent::MentionNavigate(-1));
            return;
        }
        if let Some((ix, affinity, column)) = self.vertical_target(-1.0) {
            self.move_to(ix, cx);
            self.caret_affinity = affinity;
            self.preferred_column = Some(column);
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.mention_has_selection {
            cx.emit(ComposerInputEvent::MentionNavigate(1));
            return;
        }
        if let Some((ix, affinity, column)) = self.vertical_target(1.0) {
            self.move_to(ix, cx);
            self.caret_affinity = affinity;
            self.preferred_column = Some(column);
        }
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((ix, affinity, column)) = self.vertical_target(-1.0) {
            self.select_to(ix, cx);
            self.caret_affinity = affinity;
            self.preferred_column = Some(column);
        }
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((ix, affinity, column)) = self.vertical_target(1.0) {
            self.select_to(ix, cx);
            self.caret_affinity = affinity;
            self.preferred_column = Some(column);
        }
    }

    /// Offset one wrapped line above/below the cursor, keeping its x column.
    /// Clamps to the document edges, matching the platform's behavior on the
    /// first and last line.
    fn vertical_target(&self, dir: f32) -> Option<(usize, CaretAffinity, Pixels)> {
        let current = self.cursor_point()?;
        let column = self.preferred_column.unwrap_or(current.x);
        let target_y = f32::from(current.y) + dir * f32::from(self.line_height);
        if target_y < 0.0 {
            return Some((0, CaretAffinity::Downstream, column));
        }
        if target_y >= self.content_height {
            return Some((self.content.len(), CaretAffinity::Upstream, column));
        }
        let (index, affinity) = self.caret_for_point(point(column, px(target_y)));
        Some((index, affinity, column))
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.line_range_at(self.cursor_offset());
        self.move_to(line.start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_content_end_at(self.cursor_offset()), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.line_range_at(self.cursor_offset());
        self.select_to(line.start, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.line_content_end_at(self.cursor_offset()), cx);
    }

    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let prev = self.previous_word_boundary(self.cursor_offset());
        self.move_to(prev, cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.next_word_boundary(self.cursor_offset());
        self.move_to(next, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let prev = self.previous_word_boundary(self.cursor_offset());
        self.select_to(prev, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let next = self.next_word_boundary(self.cursor_offset());
        self.select_to(next, cx);
    }

    /// Opt/Cmd + Delete family. With a live selection these delete the
    /// selection only (platform behavior) — the extend runs off the cursor.
    fn delete_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            if self.cursor_offset() == offset {
                return;
            }
            self.extend_selection(offset, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prev = self.previous_word_boundary(self.cursor_offset());
        self.delete_to(prev, window, cx);
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = self.next_word_boundary(self.cursor_offset());
        self.delete_to(next, window, cx);
    }

    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let start = self.line_range_at(self.cursor_offset()).start;
        self.delete_to(start, window, cx);
    }

    fn delete_to_line_end(
        &mut self,
        _: &DeleteToLineEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.line_content_end_at(self.cursor_offset());
        self.delete_to(end, window, cx);
    }

    fn clipboard_selection(&self) -> Option<(String, String)> {
        if self.selected_range.is_empty() {
            return None;
        }
        let selected = self.projection.normalize_range(self.selected_range.clone());
        let raw = self.content[selected.clone()].to_string();
        let mut text = String::new();
        let mut at = selected.start;
        // Use the document's actual chips: parsing a selected substring alone
        // would activate literal examples selected from inside code or images.
        for (link, _) in &self.projection.mentions {
            if link.range.start < selected.start || link.range.end > selected.end {
                continue;
            }
            text.push_str(&self.content[at..link.range.start]);
            text.push_str(&zeron_proto::invocation::invocation_prompt(
                &zeron_proto::file_mentions::file_mention_prompt(&self.content[link.range.clone()]),
            ));
            at = link.range.end;
        }
        text.push_str(&self.content[at..selected.end]);
        Some((raw, text))
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((raw, text)) = self.clipboard_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                text.clone(),
                serde_json::json!({ "zeronComposerV1": raw, "text": text }),
            ));
        } else if let Some(text) = crate::markdown::selection::selected_text() {
            // The composer keeps focus while the user reads the transcript —
            // Cmd+C with no input selection copies the markdown selection.
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some((raw, text)) = self.clipboard_selection() {
            cx.write_to_clipboard(ClipboardItem::new_string_with_json_metadata(
                text.clone(),
                serde_json::json!({ "zeronComposerV1": raw, "text": text }),
            ));

            self.last_edit = None;
            self.replace_text_in_range(None, "", window, cx);
            self.last_edit = None;
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        // Image data (or copied files) beats text — the original composer's
        // onPaste prevents the default text insert when `clipboardData.files`
        // is non-empty and stages the images instead.
        let mut images: Vec<gpui::Image> = Vec::new();
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in &item.entries {
            match entry {
                ClipboardEntry::Image(image) => images.push(image.clone()),
                ClipboardEntry::ExternalPaths(files) => {
                    paths.extend(files.paths().iter().cloned());
                }
                ClipboardEntry::String(_) => {}
            }
        }
        if !images.is_empty() {
            cx.emit(ComposerInputEvent::PastedImages(images));
            return;
        }
        if !paths.is_empty() {
            cx.emit(ComposerInputEvent::PastedPaths(paths));
            return;
        }
        if let Some(mut text) = item.text() {
            if self.mentions_enabled {
                if let Some(value) = item
                    .metadata()
                    .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                {
                    if value.get("text").and_then(|v| v.as_str()) == Some(text.as_str()) {
                        if let Some(raw) = value.get("zeronComposerV1").and_then(|v| v.as_str()) {
                            text = raw.to_owned();
                        }
                    }
                }
            }
            // Clipboard operations remain separate undo steps, even a one-character paste.
            self.last_edit = None;
            self.replace_text_in_range(None, &text, window, cx);
            self.last_edit = None;
            if self.mentions_enabled && !self.single_line {
                let end = self.cursor_offset();
                cx.emit(ComposerInputEvent::PastedText {
                    range: end.saturating_sub(text.len())..end,
                    revision: self.edit_revision,
                });
            }
        }
    }

    fn apply_pasted_references(
        &mut self,
        original: &str,
        paste_end: usize,
        revision: u64,
        replacements: Vec<(Range<usize>, String)>,
        cx: &mut Context<Self>,
    ) {
        if self.read_only
            || self.edit_revision != revision
            || self.text() != original
            || self.marked_range.is_some()
            || self.selected_range != (paste_end..paste_end)
        {
            return;
        }
        self.edit_revision = self.edit_revision.wrapping_add(1);
        let mut cursor = paste_end;
        for (range, link) in replacements.into_iter().rev() {
            cursor = cursor - range.len() + link.len();
            self.content.replace_range(range, &link);
        }
        // Canonicalization belongs to the paste's existing undo step.
        self.selected_range = cursor..cursor;
        self.refresh_projection();
        self.needs_measure = true;
        self.last_edit = None;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.mentions_enabled && self.selected_range.is_empty() && self.marked_range.is_none() {
            if let Some((range, inserted)) =
                composer_markdown::newline_edit(&self.content, self.cursor_offset())
            {
                let range = self.range_to_utf16(&range);
                self.replace_text_in_range(Some(range), &inserted, window, cx);
                return;
            }
        }
        let line_end = self.line_range_at(self.cursor_offset()).end;
        let newline = if self.line_content_end_at(self.cursor_offset()) < line_end {
            "\r\n"
        } else {
            "\n"
        };
        self.replace_text_in_range(None, newline, window, cx);
    }

    fn message_newline_or_accept(
        &mut self,
        _: &MessageNewlineOrAccept,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match enter_outcome(self.mention_has_selection, EnterOutcome::Newline) {
            EnterOutcome::AcceptCompletion => cx.emit(ComposerInputEvent::MentionAccept),
            EnterOutcome::Newline => self.newline(&Newline, window, cx),
            EnterOutcome::Submit => unreachable!("newline action cannot submit"),
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        match enter_outcome(self.mention_has_selection, EnterOutcome::Submit) {
            EnterOutcome::AcceptCompletion => cx.emit(ComposerInputEvent::MentionAccept),
            EnterOutcome::Submit => cx.emit(ComposerInputEvent::Submitted),
            EnterOutcome::Newline => unreachable!("submit action cannot insert a newline"),
        }
    }

    fn modified_submit(&mut self, _: &ModifiedSubmit, _: &mut Window, cx: &mut Context<Self>) {
        match enter_outcome(self.mention_has_selection, EnterOutcome::Submit) {
            EnterOutcome::AcceptCompletion => cx.emit(ComposerInputEvent::MentionAccept),
            EnterOutcome::Submit => cx.emit(ComposerInputEvent::ModifiedSubmitted),
            EnterOutcome::Newline => unreachable!("submit action cannot insert a newline"),
        }
    }

    fn mention_tab(&mut self, _: &MentionTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.mention_has_selection {
            cx.emit(ComposerInputEvent::MentionAccept);
        } else if !self.indent_list(false, window, cx) {
            cx.propagate();
        }
    }

    fn outdent_list(&mut self, _: &OutdentList, window: &mut Window, cx: &mut Context<Self>) {
        if self.mention_open || !self.indent_list(true, window, cx) {
            cx.propagate();
        }
    }

    fn indent_list(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.mentions_enabled
            || self.read_only
            || self.marked_range.is_some()
            || self.mention_open
        {
            return false;
        }
        let selection = self.selected_range.clone();
        let start = self.line_range_at(selection.start).start;
        let last = if !selection.is_empty() && self.content[..selection.end].ends_with('\n') {
            selection.end - 1
        } else {
            selection.end
        };
        let end = self.line_range_at(last).end;
        let block = &self.content[start..end];
        let mut replacement = String::new();
        let mut changes = Vec::new();
        let mut at = start;
        for (ix, line) in block.split('\n').enumerate() {
            let Some(prefix) = composer_markdown::list_prefix(line) else {
                return false;
            };
            if in_code(&self.content, at + prefix.indent) {
                return false;
            }
            if ix > 0 {
                replacement.push('\n');
            }
            let removed = if outdent {
                line[prefix.indent_start..prefix.indent]
                    .chars()
                    .take(2)
                    .map(char::len_utf8)
                    .sum()
            } else {
                0
            };
            let added = if outdent { 0 } else { 2 };
            replacement.push_str(&line[..prefix.indent_start]);
            if !outdent {
                replacement.push_str("  ");
            }
            replacement.push_str(&line[prefix.indent_start + removed..]);
            changes.push((at + prefix.indent_start, removed, added));
            at += line.len() + 1;
        }
        let remap = |offset: usize| -> usize {
            let mut result = offset as isize;
            for &(at, removed, added) in &changes {
                if offset >= at {
                    result += added as isize - removed.min(offset - at) as isize;
                }
            }
            result.max(0) as usize
        };
        let next = remap(selection.start)..remap(selection.end);
        let reversed = self.selection_reversed;
        let edit = self.range_to_utf16(&(start..end));
        self.last_edit = None;
        self.replace_text_in_range(Some(edit), &replacement, window, cx);
        self.selected_range = next;
        self.selection_reversed = reversed;
        self.refresh_projection();
        self.last_edit = None;
        cx.notify();
        true
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if escape_dismisses_completion(&event.keystroke.key, self.mention_open) {
            cx.emit(ComposerInputEvent::MentionDismiss);
            cx.stop_propagation();
        }
    }

    // ---- geometry ----

    /// Content-local point for a byte index (y grows down from content top).
    fn point_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        self.point_for_display_index(self.projection.raw_to_display(index))
    }

    fn cursor_point(&self) -> Option<Point<Pixels>> {
        self.point_for_display_index_with_affinity(
            self.projection.raw_to_display(self.cursor_offset()),
            self.caret_affinity,
        )
    }

    /// Content-local point for a shaped projection byte index. The icon layer
    /// uses this to occupy its explicit projection slot without inventing a
    /// second coordinate system beside the custom text editor.
    fn point_for_display_index(&self, index: usize) -> Option<Point<Pixels>> {
        self.point_for_display_index_with_affinity(index, CaretAffinity::Downstream)
    }

    fn point_for_display_index_with_affinity(
        &self,
        index: usize,
        affinity: CaretAffinity,
    ) -> Option<Point<Pixels>> {
        for (line_ix, line) in self.last_lines.iter().enumerate() {
            let line_start = *self.line_starts.get(line_ix)?;
            let line_len = line.len();
            if index < line_start {
                continue;
            }
            if index <= line_start + line_len {
                let relative_index = index - line_start;
                let local = if affinity == CaretAffinity::Downstream {
                    line.wrap_boundaries()
                        .iter()
                        .position(|boundary| {
                            line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index
                                == relative_index
                        })
                        .map(|row| point(px(0.0), self.line_height * (row + 1)))
                        .or_else(|| line.position_for_index(relative_index, self.line_height))?
                } else {
                    line.position_for_index(relative_index, self.line_height)?
                };
                let y_offset: f32 = self
                    .last_lines
                    .iter()
                    .take(line_ix)
                    .map(|l| f32::from(l.size(self.line_height).height))
                    .sum();
                let indent = if local.y > px(0.0) {
                    self.line_indents.get(line_ix).copied().unwrap_or_default()
                } else {
                    px(0.0)
                };
                return Some(point(local.x + indent, local.y + px(y_offset)));
            }
        }
        None
    }

    /// Content-local boxes occupied by a projected byte range, split at every
    /// soft wrap. A caret exactly at a wrap boundary belongs visually to both
    /// rows in GPUI; using the explicit wrap indices lets the range's first
    /// glyph start at x=0 on the new row instead of inheriting the old row's
    /// end caret (which previously caused mention washes to be discarded).
    fn bounds_for_display_range(&self, range: Range<usize>) -> Vec<Bounds<Pixels>> {
        let mut bounds = Vec::new();
        let mut y_offset = px(0.0);
        for (line_ix, line) in self.last_lines.iter().enumerate() {
            let line_start = self.line_starts.get(line_ix).copied().unwrap_or(0);
            let local_start = range.start.saturating_sub(line_start).min(line.len());
            let local_end = range.end.saturating_sub(line_start).min(line.len());
            if local_start >= local_end
                || range.end <= line_start
                || range.start >= line_start + line.len()
            {
                y_offset += line.size(self.line_height).height;
                continue;
            }

            let row_ends = line
                .wrap_boundaries()
                .iter()
                .map(|boundary| line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index)
                .chain(std::iter::once(line.len()));
            for (row_ix, row_start, segment) in
                display_row_segments(local_start..local_end, row_ends)
            {
                let row_y = y_offset + self.line_height * row_ix;
                let start_x = if segment.start == row_start {
                    px(0.0)
                } else {
                    line.position_for_index(segment.start, self.line_height)
                        .map(|point| point.x)
                        .unwrap_or(px(0.0))
                };
                if let Some(end_point) = line.position_for_index(segment.end, self.line_height)
                    && end_point.x > start_x
                {
                    bounds.push(Bounds::new(
                        point(
                            start_x
                                + if row_ix > 0 {
                                    self.line_indents.get(line_ix).copied().unwrap_or_default()
                                } else {
                                    px(0.0)
                                },
                            row_y,
                        ),
                        size(end_point.x - start_x, self.line_height),
                    ));
                }
            }
            y_offset += line.size(self.line_height).height;
        }
        bounds
    }

    /// Byte index and visual side closest to a content-local point.
    fn caret_for_point(&self, position: Point<Pixels>) -> (usize, CaretAffinity) {
        if self.display_is_placeholder {
            return (0, CaretAffinity::Downstream);
        }
        let mut y = f32::from(position.y);
        if y < 0.0 {
            return (0, CaretAffinity::Downstream);
        }
        for (line_ix, line) in self.last_lines.iter().enumerate() {
            let height = f32::from(line.size(self.line_height).height);
            let line_start = self.line_starts.get(line_ix).copied().unwrap_or(0);
            if y < height || line_ix + 1 == self.last_lines.len() {
                let indent = if y >= f32::from(self.line_height) {
                    self.line_indents.get(line_ix).copied().unwrap_or_default()
                } else {
                    px(0.0)
                };
                let local = point(position.x - indent, px(y.min(height - 1.0).max(0.0)));
                let ix = line
                    .closest_index_for_position(local, self.line_height)
                    .unwrap_or_else(|ix| ix);
                let affinity = line
                    .wrap_boundaries()
                    .iter()
                    .position(|boundary| {
                        line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index == ix
                    })
                    .filter(|row| local.y < self.line_height * (row + 1))
                    .map_or(CaretAffinity::Downstream, |_| CaretAffinity::Upstream);
                let mut raw = self
                    .projection
                    .display_to_raw((line_start + ix).min(self.projection.display.len()));
                // CRLF is one newline grapheme. A click past the text on its
                // row must land before CR, never between its two bytes.
                if raw > 0
                    && self.content.as_bytes().get(raw) == Some(&b'\n')
                    && self.content.as_bytes()[raw - 1] == b'\r'
                {
                    raw -= 1;
                }
                return (raw, affinity);
            }
            y -= height;
        }
        (self.content.len(), CaretAffinity::Upstream)
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        self.caret_for_mouse_position(position).0
    }

    fn caret_for_mouse_position(&self, position: Point<Pixels>) -> (usize, CaretAffinity) {
        let Some(bounds) = self.last_bounds else {
            return (0, CaretAffinity::Downstream);
        };
        let local = point(
            position.x - bounds.left() + px(self.scroll_left),
            position.y - bounds.top() + px(self.scroll_top),
        );
        self.caret_for_point(local)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invalidate_mention_tooltip();
        window.focus(&self.focus_handle, cx);
        let intent = press_intent(event.click_count, event.modifiers.shift);
        self.is_selecting = true;
        self.drag_position = Some(event.position);
        self.drag_unit = None;
        self.drag_generation = self.drag_generation.wrapping_add(1);
        self.drag_autoscroll_active = false;
        match intent {
            PressIntent::Word | PressIntent::Line => {
                let index = self.index_for_mouse_position(event.position);
                let range = if intent == PressIntent::Word {
                    self.mention_hits
                        .iter()
                        .find(|hit| hit.bounds.contains(&event.position))
                        .map(|hit| hit.target.range.clone())
                        .unwrap_or_else(|| self.selection_unit(intent, index))
                } else {
                    self.selection_unit(intent, index)
                };
                self.move_to(range.start, cx);
                self.select_to(range.end, cx);
                self.drag_unit = Some((intent, range));
            }
            PressIntent::ExtendSelection => {
                let (index, affinity) = self.caret_for_mouse_position(event.position);
                self.select_to(index, cx);
                self.caret_affinity = affinity;
            }
            PressIntent::PlaceCaret => {
                let (index, affinity) = self.caret_for_mouse_position(event.position);
                self.move_to(index, cx);
                self.caret_affinity = affinity;
            }
        }
    }

    fn selection_unit(&self, intent: PressIntent, index: usize) -> Range<usize> {
        let range = if intent == PressIntent::Line {
            let mut range = self.line_range_at(index);
            if range.end < self.content.len() {
                range.end += 1;
            }
            range
        } else {
            // Hit testing snaps to chip edges; prefer the chip at that edge.
            if let Some((link, _)) = self
                .projection
                .mentions
                .iter()
                .find(|(link, _)| link.range.start <= index && index < link.range.end)
            {
                return link.range.clone();
            }
            word_range(&self.content, index)
        };
        self.projection.normalize_range(range)
    }

    fn drag_select_to(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some((intent, anchor)) = self.drag_unit.clone() {
            let range = self.selection_unit(intent, index);
            if range.start < anchor.start {
                self.move_to(anchor.end, cx);
                self.select_to(range.start, cx);
            } else {
                self.move_to(anchor.start, cx);
                self.select_to(range.end.max(anchor.end), cx);
            }
        } else {
            self.select_to(index, cx);
        }
    }

    fn drag_select_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (index, affinity) = self.caret_for_mouse_position(position);
        self.drag_select_to(index, cx);
        if self.drag_unit.is_none() {
            self.caret_affinity = affinity;
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
        self.drag_position = None;
        self.drag_generation = self.drag_generation.wrapping_add(1);
        self.drag_autoscroll_active = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.on_mention_pointer_move(event.position, cx);
        if self.is_selecting {
            self.drag_position = Some(event.position);
            let position = self.drag_selection_position(event.position);
            self.drag_select_at(position, cx);
            if self.drag_scroll_delta(event.position) != 0.0 && !self.drag_autoscroll_active {
                self.start_drag_autoscroll(cx);
            }
        }
    }

    fn start_drag_autoscroll(&mut self, cx: &mut Context<Self>) {
        self.drag_autoscroll_active = true;
        let generation = self.drag_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(DRAG_SCROLL_FRAME_MS))
                    .await;
                let keep_running = this
                    .update(cx, |input, cx| input.drag_autoscroll_tick(generation, cx))
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    fn drag_selection_position(&self, position: Point<Pixels>) -> Point<Pixels> {
        let Some(bounds) = self.last_bounds else {
            return position;
        };
        point(
            position.x.clamp(bounds.left(), bounds.right() - px(0.5)),
            position.y.clamp(bounds.top(), bounds.bottom() - px(0.5)),
        )
    }

    fn drag_scroll_delta(&self, position: Point<Pixels>) -> f32 {
        let Some(bounds) = self.last_bounds else {
            return 0.0;
        };
        input_drag_scroll_delta(
            f32::from(position.y),
            f32::from(bounds.top()),
            f32::from(bounds.bottom()),
            f32::from(self.line_height),
        )
    }

    fn drag_autoscroll_tick(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        if !self.is_selecting || self.drag_generation != generation {
            return false;
        }
        let (Some(position), Some(bounds)) = (self.drag_position, self.last_bounds) else {
            self.drag_autoscroll_active = false;
            return false;
        };
        let delta = self.drag_scroll_delta(position);
        if delta == 0.0 {
            self.drag_autoscroll_active = false;
            return false;
        }
        let next = (self.scroll_top + delta).clamp(
            0.0,
            input_max_scroll(
                self.content_height,
                self.settled_viewport_height
                    .unwrap_or(f32::from(bounds.size.height)),
            ),
        );
        if next == self.scroll_top {
            self.drag_autoscroll_active = false;
            return false;
        }
        self.scroll_top = next;
        let edge_position = self.drag_selection_position(position);
        self.drag_select_at(edge_position, cx);
        // Selection motion normally resumes caret following. During an edge
        // drag the autoscroll loop owns the viewport instead.
        self.follow_cursor = false;
        true
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.last_bounds else {
            return;
        };
        let viewport_height = self
            .settled_viewport_height
            .unwrap_or(f32::from(bounds.size.height));
        let delta_y = f32::from(event.delta.pixel_delta(self.line_height).y);
        let next = input_scroll_offset(
            self.scroll_top,
            delta_y,
            self.content_height,
            viewport_height,
        );
        if next == self.scroll_top {
            // Overscroll guard: when the input itself is scrollable (content
            // taller than the viewport), swallow the wheel event even at the
            // scroll boundary so it never chains into the outer transcript
            // list (the native equivalent of `overscroll-behavior: contain`).
            if delta_y != 0.0 && input_max_scroll(self.content_height, viewport_height) > 0.0 {
                cx.stop_propagation();
            }
            return;
        }
        self.invalidate_mention_tooltip();
        self.scroll_top = next;
        self.follow_cursor = false;
        cx.stop_propagation();
        cx.emit(ComposerInputEvent::ViewportChanged);
        cx.notify();
    }

    // ---- utf16 mapping (IME) ----

    fn offset_from_utf16(&self, offset: usize) -> usize {
        Self::utf8_offset(&self.content, offset)
    }

    fn utf8_offset(text: &str, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in text.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    /// Shape the text at a width; store measured layout; return content height.
    /// Called from the element's measured-layout closure.
    fn layout_text(
        &mut self,
        width: Pixels,
        style: &TextStyle,
        window: &mut Window,
        cx: &App,
    ) -> f32 {
        let theme = Theme::of(cx);
        let key = InputLayoutKey {
            width,
            font: style.font(),
            font_size: style.font_size.to_pixels(window.rem_size()),
            color: style.color,
            chip_family: theme.font_mono.clone(),
            chip_color: theme.code_text,
            syntax: theme.syntax.clone(),
            marked_range: self.marked_range.clone(),
            placeholder: self.placeholder.clone(),
            mentions_enabled: self.mentions_enabled,
            active_line: self.mentions_enabled.then(|| self.editing_source_range()),
        };
        // Height-only animation, scrolling, selection and caret blinking do
        // not change shaping. Reuse the entity's single retained layout,
        // including the parent's early measurement of this same edit.
        if !self.needs_measure && self.last_layout_key.as_ref() == Some(&key) {
            self.layout_epoch += 1;
            return self.content_height;
        }
        #[cfg(test)]
        {
            self.layout_rebuilds += 1;
        }
        // Rebuild this even for an empty draft. Otherwise deleting the final
        // mention can leave its previous paint geometry alive while the
        // placeholder is already being shaped, tinting "Do anything" for a
        // frame (or longer when no subsequent layout is requested).
        self.refresh_projection();
        let (display, is_placeholder) = if self.content.is_empty() {
            (self.placeholder.clone(), true)
        } else {
            (SharedString::from(self.projection.display.clone()), false)
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        self.line_height = px(self.configured_line_height);

        // Chips read as inline code: the markdown renderer's recipe (mono font
        // + the spectrum's `code_text`) over the rounded `code_wash` beneath.
        let (chip_font, chip_color) = {
            let theme = Theme::of(cx);
            (gpui::font(theme.font_mono.clone()), theme.code_text)
        };
        let run_for = |len: usize, underline: bool, chip: bool| TextRun {
            len,
            font: if chip {
                chip_font.clone()
            } else {
                style.font()
            },
            color: if chip { chip_color } else { style.color },
            // Rounded mention washes are painted explicitly beneath the text;
            // TextRun backgrounds are square and can disappear in wrapped runs.
            background_color: None,
            underline: underline.then_some(UnderlineStyle {
                color: Some(style.color),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: None,
        };
        let raw_faces = if self.mentions_enabled && !is_placeholder {
            composer_markdown::faces(&self.content)
        } else {
            Vec::new()
        };
        let faces = if self.mentions_enabled && !is_placeholder {
            raw_faces
                .iter()
                .cloned()
                .map(|(range, face)| {
                    (
                        self.projection.raw_to_display(range.start)
                            ..self.projection.raw_to_display(range.end),
                        face,
                    )
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let syntax: Vec<_> = if self.mentions_enabled && self.syntax_source == self.content {
            self.syntax_spans
                .iter()
                .map(|span| {
                    (
                        self.projection.raw_to_display(span.range.start)
                            ..self.projection.raw_to_display(span.range.end),
                        span.kind,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let marked = self.marked_range.as_ref().map(|r| {
            self.projection.raw_to_display(r.start)..self.projection.raw_to_display(r.end)
        });
        let mut boundaries = vec![0, display.len()];
        for (range, _) in &syntax {
            boundaries.extend([range.start, range.end]);
        }
        for (range, _) in &faces {
            boundaries.extend([range.start, range.end]);
        }
        for (_, range) in &self.projection.mentions {
            boundaries.extend([range.start, range.end]);
        }
        if let Some(range) = &marked {
            boundaries.extend([range.start, range.end]);
        }
        boundaries.sort_unstable();
        boundaries.dedup();
        let mut face_events: Vec<_> = faces
            .iter()
            .flat_map(|(range, face)| {
                [
                    (range.start, *face as usize, 1isize),
                    (range.end, *face as usize, -1isize),
                ]
            })
            .collect();
        face_events.sort_by_key(|event| event.0);
        let mut face_event = 0;
        let mut face_depth = [0isize; 4];
        let runs: Vec<TextRun> = boundaries
            .windows(2)
            .filter(|r| r[1] > r[0])
            .map(|r| {
                while face_event < face_events.len() && face_events[face_event].0 <= r[0] {
                    let (_, face, delta) = face_events[face_event];
                    face_depth[face] += delta;
                    face_event += 1;
                }
                let mention_ix = self
                    .projection
                    .mentions
                    .partition_point(|(_, range)| range.end <= r[0]);
                let chip = self
                    .projection
                    .mentions
                    .get(mention_ix)
                    .is_some_and(|(_, range)| range.contains(&r[0]));
                let code = face_depth[composer_markdown::Face::Code as usize] > 0;
                let mut run = run_for(
                    r[1] - r[0],
                    marked.as_ref().is_some_and(|range| range.contains(&r[0])),
                    chip || code,
                );
                if !chip {
                    if face_depth[composer_markdown::Face::Bold as usize] > 0 {
                        run.font.weight = gpui::FontWeight::BOLD;
                    }
                    if face_depth[composer_markdown::Face::Italic as usize] > 0 {
                        run.font.style = gpui::FontStyle::Italic;
                    }
                    if code {
                        run.background_color = Some(theme.code_wash);
                    }
                    if face_depth[composer_markdown::Face::Strikethrough as usize] > 0 {
                        run.strikethrough = Some(gpui::StrikethroughStyle {
                            thickness: px(1.0),
                            color: Some(style.color),
                        });
                    }
                }
                if code && !chip {
                    run.color = style.color;
                    if let Some((_, kind)) = syntax
                        .get(syntax.partition_point(|(range, _)| range.end <= r[0]))
                        .filter(|(range, _)| range.contains(&r[0]))
                    {
                        run.color = Theme::of(cx).syntax.color(*kind);
                    }
                }
                run
            })
            .collect();

        // Each logical list line reserves its marker width for continuation
        // rows. Painting and hit testing apply the same continuation offset.
        let mut lines = Vec::new();
        let mut indents = Vec::new();
        let mut display_at = 0;
        let mut raw_at = 0;
        let mut run_at = 0;
        let run_ranges: Vec<_> = runs
            .iter()
            .map(|run| {
                let range = run_at..run_at + run.len;
                run_at = range.end;
                (range, run)
            })
            .collect();
        let code_ranges: Vec<_> = raw_faces
            .iter()
            .filter(|(_, face)| *face == composer_markdown::Face::Code)
            .map(|(range, _)| range)
            .collect();
        for text in display.split('\n') {
            let raw_line = self
                .content
                .get(raw_at..)
                .unwrap_or_default()
                .split('\n')
                .next()
                .unwrap_or_default();
            let mut indent = px(0.0);
            if self.mentions_enabled && !is_placeholder {
                if let Some(prefix) = composer_markdown::list_prefix(raw_line).filter(|prefix| {
                    let marker_at = raw_at + prefix.indent;
                    let code_ix = code_ranges.partition_point(|range| range.end <= marker_at);
                    !code_ranges
                        .get(code_ix)
                        .is_some_and(|range| range.contains(&marker_at))
                }) {
                    let end = self
                        .projection
                        .raw_to_display(raw_at + prefix.end)
                        .saturating_sub(display_at)
                        .min(text.len());
                    let marker: SharedString = text[..end].to_string().into();
                    let marker_run = run_for(marker.len(), false, false);
                    indent = window
                        .text_system()
                        .shape_line(marker, font_size, &[marker_run], None)
                        .width
                        .min(width * 0.4);
                }
            }
            let end = display_at + text.len();
            let first_run = run_ranges.partition_point(|(r, _)| r.end <= display_at);
            let line_runs: Vec<TextRun> = run_ranges[first_run..]
                .iter()
                .take_while(|(r, _)| r.start < end)
                .filter_map(|(range, run)| {
                    let len = range
                        .end
                        .min(end)
                        .saturating_sub(range.start.max(display_at));
                    (len > 0).then(|| TextRun {
                        len,
                        ..(*run).clone()
                    })
                })
                .collect();
            if let Ok(shaped) = window.text_system().shape_text(
                text.to_string().into(),
                font_size,
                &line_runs,
                (!self.single_line).then_some((width - indent).max(px(20.0))),
                None,
            ) {
                let chips: Vec<_> = self.projection.mentions[self
                    .projection
                    .mentions
                    .partition_point(|(_, r)| r.end <= display_at)..]
                    .iter()
                    .take_while(|(_, r)| r.start < end)
                    .filter(|(_, r)| r.start >= display_at && r.end <= end)
                    .map(|(_, r)| r.start - display_at..r.end - display_at)
                    .collect();
                for mut line in shaped {
                    if !self.single_line {
                        wrap_reference_chips(&mut line, &chips, (width - indent).max(px(20.0)));
                    }
                    lines.push(line);
                    indents.push(indent);
                }
            }
            display_at = end + 1;
            raw_at += raw_line.len() + 1;
        }
        self.line_indents = indents;

        // Logical line byte offsets (each shaped line covers one \n-split line).
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut at = 0usize;
        for line in &lines {
            line_starts.push(at);
            at += line.len() + 1; // + '\n'
        }
        if line_starts.is_empty() {
            line_starts.push(0);
        }

        let content_height: f32 = lines
            .iter()
            .map(|l| f32::from(l.size(self.line_height).height))
            .sum();
        let max_line_width: f32 = lines
            .iter()
            .map(|l| f32::from(l.unwrapped_layout.width))
            .fold(0.0, f32::max);

        self.display_is_placeholder = is_placeholder;
        self.max_ascent = lines
            .iter()
            .map(|line| f32::from(line.unwrapped_layout.ascent))
            .fold(INPUT_TEXT_SIZE, f32::max);
        self.last_layout_key = Some(key);
        self.last_lines = lines;
        self.line_starts = line_starts;
        self.content_height = content_height.max(self.configured_line_height);
        self.max_line_width = if is_placeholder { 0.0 } else { max_line_width };
        self.last_width = f32::from(width);
        self.needs_measure = false;
        self.layout_epoch += 1;
        self.content_height
    }

    fn paint_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let visible = f32::from(bounds.size.height);
        let top_overflow = input_overflow_edges(
            self.content_height,
            self.settled_viewport_height.unwrap_or(visible),
            visible,
            self.scroll_top,
        )
        .0;
        let top_padding = if top_overflow {
            self.overflow_top_padding
        } else {
            0.0
        };
        let height = input_reveal_height(
            visible,
            self.scroll_top,
            f32::from(self.line_height),
            self.resizing,
        );
        Bounds::new(
            point(bounds.left(), bounds.top() - px(top_padding)),
            size(bounds.size.width, px(height + top_padding)),
        )
    }

    /// Keep the cursor visible when content exceeds the element height.
    fn clamp_scroll(&mut self, element_height: f32) -> bool {
        if self.single_line {
            let previous = self.scroll_left;
            let width = (self.last_width - 2.0).max(1.0);
            if let Some(cursor) = self.cursor_point() {
                let x = f32::from(cursor.x);
                self.scroll_left = self.scroll_left.min(x).max(x - width).max(0.0);
            }
            self.scroll_left = self.scroll_left.min((self.max_line_width - width).max(0.0));
            self.scroll_top = 0.0;
            return self.scroll_left != previous;
        }
        let previous = self.scroll_top;
        if self.follow_cursor {
            if let Some(cursor) = self.cursor_point() {
                self.scroll_top = input_scroll_offset_for_cursor(
                    self.scroll_top,
                    f32::from(cursor.y),
                    f32::from(self.line_height),
                    self.content_height,
                    element_height,
                    self.settled_viewport_height,
                );
            }
        }
        self.scroll_top = self.scroll_top.clamp(
            0.0,
            input_max_scroll(
                self.content_height,
                self.settled_viewport_height.unwrap_or(element_height),
            ),
        );
        self.scroll_top != previous
    }
}

impl EventEmitter<ComposerInputEvent> for ComposerInput {}

impl Focusable for ComposerInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for ComposerInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self
            .projection
            .normalize_range(self.range_from_utf16(&range_utf16));
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content.get(range)?.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.selected_range = self.projection.normalize_range(self.selected_range.clone());
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_range.take().is_some() {
            self.refresh_projection();
            self.needs_measure = true;
            self.last_edit = None;
            cx.emit(ComposerInputEvent::CursorMoved);
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let single_line_text;
        let new_text = if self.single_line {
            single_line_text = new_text.replace(['\r', '\n'], " ");
            single_line_text.as_str()
        } else {
            new_text
        };
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.projection.normalize_range(range);
        self.invalidate_mention_tooltip();
        // An IME commit is the tail of a composition whose pre-composition
        // snapshot was already taken (`replace_and_mark_text_in_range`);
        // recording here would pin undo to the half-composed text instead.
        if self.marked_range.is_none() {
            self.record_edit(&range, new_text);
        }
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.marked_range.take();
        self.refresh_projection();
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let single_line_text;
        let new_text = if self.single_line {
            single_line_text = new_text.replace(['\r', '\n'], " ");
            single_line_text.as_str()
        } else {
            new_text
        };
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = self.projection.normalize_range(range);
        self.invalidate_mention_tooltip();
        // First keystroke of a composition: snapshot the text as it stood
        // before any of it existed, so one undo drops the whole composition.
        if self.marked_range.is_none() {
            self.undo_stack.push(self.snapshot());
            if self.undo_stack.len() > UNDO_LIMIT {
                self.undo_stack.remove(0);
            }
            self.redo_stack.clear();
            self.last_edit = None;
        }
        self.edit_revision = self.edit_revision.wrapping_add(1);
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| Self::utf8_offset(new_text, r.start)..Self::utf8_offset(new_text, r.end))
            .map(|new_range| new_range.start + range.start..new_range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.caret_affinity = CaretAffinity::Downstream;
        self.preferred_column = None;
        self.refresh_projection();
        self.follow_cursor = true;
        self.reset_blink();
        self.needs_measure = true;
        cx.emit(ComposerInputEvent::Edited);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self
            .projection
            .normalize_range(self.range_from_utf16(&range_utf16));
        let start = if range.is_empty() && range.start == self.cursor_offset() {
            self.cursor_point()?
        } else {
            self.point_for_index(range.start)?
        };
        let origin = point(
            bounds.left() + start.x - px(self.scroll_left),
            bounds.top() + start.y - px(self.scroll_top),
        );
        Some(Bounds::new(origin, size(px(2.0), self.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        point_in_window: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.index_for_mouse_position(point_in_window);
        Some(self.offset_to_utf16(index))
    }
}

/// The custom element: measured auto-grow layout + shaped-line painting.
struct ComposerTextElement {
    input: Entity<ComposerInput>,
    /// Max content height before internal scrolling kicks in.
    max_content_height: f32,
}

struct MentionPathTooltip {
    path: SharedString,
    /// Stable for one `Waiting → Visible` promotion; a later activation gets
    /// a new key and therefore exactly one fresh fade-in.
    activation: u64,
}

impl Render for MentionPathTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_popup();
        motion::fade_quick(
            ("file-mention-path-tooltip", self.activation),
            div().child(crate::frost::frosted(
                crate::popover::CARD_RADIUS,
                crate::frost::MENU_BLUR,
                crate::popover::popover_card(&theme)
                    .h(px(MENTION_TOOLTIP_HEIGHT))
                    .max_w(px(480.0))
                    .flex()
                    .items_center()
                    .p_0()
                    .px(px(8.0))
                    .font_family(theme.font_mono.clone())
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(div().min_w_0().truncate().child(self.path.clone())),
            )),
        )
    }
}

struct ComposerTextPrepaint {
    cursor: Option<PaintQuad>,
    mention_quads: Vec<PaintQuad>,
    mention_hits: Vec<MentionHit>,
    selection_quads: Vec<PaintQuad>,
    /// Completion preview: window-space origin of the end-of-text caret plus
    /// the suffix to paint there (shaped at paint time — it never joins the
    /// content's own layout, so hit-testing and the caret ignore it).
    ghost: Option<(Point<Pixels>, SharedString)>,
}

impl IntoElement for ComposerTextElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for ComposerTextElement {
    type RequestLayoutState = ();
    type PrepaintState = ComposerTextPrepaint;

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
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        let input = self.input.clone();
        let text_style = window.text_style();
        let max_content = self.max_content_height;
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, cx| {
                let width = known.width.unwrap_or(match available.width {
                    gpui::AvailableSpace::Definite(width) => width,
                    _ => px(320.0),
                });
                let content_height = input.update(cx, |input, cx| {
                    input.layout_text(width, &text_style, window, cx)
                });
                size(width, px(content_height.min(max_content)))
            });
        (layout_id, ())
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
        let text_style = window.text_style();
        self.input.update(cx, |input, cx| {
            // Intrinsic measurement may try several widths in one layout.
            // Only publish the resolved geometry, once: notifying for each
            // provisional width starts an endless measure/notify loop.
            input.layout_text(bounds.size.width, &text_style, window, cx);
            let layout = (bounds.size.width, input.content_height);
            let layout_changed = input.last_notified_layout != Some(layout);
            input.last_notified_layout = Some(layout);
            let scrolled = input.clamp_scroll(f32::from(bounds.size.height));
            input.last_bounds = Some(bounds);
            if scrolled || layout_changed {
                cx.emit(ComposerInputEvent::ViewportChanged);
            }
        });
        let input = self.input.read(cx);
        let paint_bounds = input.paint_bounds(bounds);
        let scroll = px(input.scroll_top);
        let origin = point(bounds.left() - px(input.scroll_left), bounds.top() - scroll);
        let selection_color = Theme::of(cx).selection;
        let caret_color = Theme::of(cx).caret;
        // The inline-code recipe: chips use the spectrum wash like `code` spans.
        let mention_color = Theme::of(cx).code_wash;

        let mut mention_quads = Vec::new();
        let mut mention_hits = Vec::new();
        for (mention, display) in &input.projection.mentions {
            let target = MentionTooltipTarget {
                range: mention.range.clone(),
                path: SharedString::from(format!(
                    "{}{}",
                    mention.path,
                    if mention.is_dir { "/" } else { "" }
                )),
            };
            for local_bounds in input.bounds_for_display_range(display.clone()) {
                let chip_bounds = Bounds::new(
                    point(
                        origin.x + local_bounds.origin.x,
                        origin.y + local_bounds.origin.y + px(2.0),
                    ),
                    size(local_bounds.size.width, local_bounds.size.height - px(4.0)),
                );
                mention_quads.push(quad(
                    chip_bounds,
                    px(5.0),
                    mention_color,
                    px(0.0),
                    gpui::transparent_black(),
                    BorderStyle::default(),
                ));
                let above_anchor = chip_bounds.top() - px(MENTION_TOOLTIP_HEIGHT) - px(1.0);
                let anchor_y = if above_anchor >= px(0.0) {
                    above_anchor
                } else {
                    // GPUI positions at anchor + 1px; subtracting one keeps the
                    // below fallback flush so the pointer can enter the popup.
                    chip_bounds.bottom() - px(1.0)
                };
                let visible_bounds = chip_bounds.intersect(&paint_bounds);
                if visible_bounds.size.width == px(0.0) || visible_bounds.size.height == px(0.0) {
                    continue;
                }
                mention_hits.push(MentionHit {
                    target: target.clone(),
                    bounds: visible_bounds,
                    // The fixed-height popup starts at anchor + 1px. Moving
                    // the anchor above the chip therefore yields conventional
                    // above-target placement without cursor tracking.
                    anchor: point(chip_bounds.left(), anchor_y),
                });
            }
        }
        let mut selection_quads = Vec::new();
        let mut cursor = None;
        if input.selected_range.is_empty() || input.display_is_placeholder {
            if let Some(p) = input.cursor_point() {
                cursor = Some(fill(
                    Bounds::new(
                        point(origin.x + p.x, origin.y + p.y),
                        size(px(2.0), input.line_height),
                    ),
                    caret_color,
                ));
            } else if input.display_is_placeholder {
                cursor = Some(fill(
                    Bounds::new(origin, size(px(2.0), input.line_height)),
                    caret_color,
                ));
            }
        } else if let (Some(start), Some(end)) = (
            input.point_for_index(input.selected_range.start),
            input.point_for_display_index_with_affinity(
                input.projection.raw_to_display(input.selected_range.end),
                CaretAffinity::Upstream,
            ),
        ) {
            let lh = input.line_height;
            if start.y == end.y {
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(origin.x + end.x, origin.y + start.y + lh),
                    ),
                    selection_color,
                ));
            } else {
                // First visual row, full middle rows, last visual row.
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(bounds.right(), origin.y + start.y + lh),
                    ),
                    selection_color,
                ));
                if end.y > start.y + lh {
                    selection_quads.push(fill(
                        Bounds::from_corners(
                            point(origin.x, origin.y + start.y + lh),
                            point(bounds.right(), origin.y + end.y),
                        ),
                        selection_color,
                    ));
                }
                selection_quads.push(fill(
                    Bounds::from_corners(
                        point(origin.x, origin.y + end.y),
                        point(origin.x + end.x, origin.y + end.y + lh),
                    ),
                    selection_color,
                ));
            }
        }
        let tooltip = input.visible_mention_tooltip();
        if let Some((_target, anchor, _activation, view)) = tooltip {
            let view = view.into();
            let input = self.input.clone();
            window.set_tooltip(AnyTooltip {
                view,
                mouse_position: anchor,
                check_visible_and_update: Rc::new(move |popup, window, cx| {
                    input.update(cx, |input, _| {
                        input.check_mention_tooltip_visibility(popup, window.mouse_position())
                    })
                }),
            });
        }
        // The ghost only shows where accepting it would insert: a collapsed
        // caret at the end of real (non-placeholder, non-IME) text.
        let ghost = input
            .ghost
            .clone()
            .filter(|g| {
                !g.is_empty()
                    && !input.display_is_placeholder
                    && input.marked_range.is_none()
                    && input.selected_range.is_empty()
                    && input.cursor_offset() == input.content.len()
            })
            .and_then(|g| {
                input
                    .point_for_index(input.content.len())
                    .map(|p| (point(origin.x + p.x, origin.y + p.y), g))
            });
        ComposerTextPrepaint {
            cursor,
            mention_quads,
            mention_hits,
            selection_quads,
            ghost,
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
        let focus_handle = self.input.read(cx).focus_handle.clone();
        self.input.update(cx, |input, _| {
            input.set_mention_hits(prepaint.mention_hits.clone())
        });
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let input = self.input.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                input.update(cx, |input, cx| input.on_mouse_move(event, cx));
            }
        });

        // WrappedLine isn't Clone — temporarily take the shaped lines out of the
        // entity for painting, then put them back for mouse mapping.
        let (lines, indents, line_height, scroll, scroll_left) =
            self.input.update(cx, |input, _| {
                (
                    std::mem::take(&mut input.last_lines),
                    input.line_indents.clone(),
                    input.line_height,
                    input.scroll_top,
                    input.scroll_left,
                )
            });

        let paint_bounds = self.input.read(cx).paint_bounds(bounds);
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: paint_bounds,
            }),
            |window| {
                for quad in prepaint.mention_quads.drain(..) {
                    window.paint_quad(quad);
                }
                for quad in prepaint.selection_quads.drain(..) {
                    window.paint_quad(quad);
                }
                let mut y = bounds.top() - px(scroll);
                for (line_ix, line) in lines.iter().enumerate() {
                    let height = line.size(line_height).height;
                    let indent = indents.get(line_ix).copied().unwrap_or_default();
                    if indent > px(0.0) && !line.wrap_boundaries().is_empty() {
                        for row in 0..=line.wrap_boundaries().len() {
                            let row_bounds = Bounds::new(
                                point(bounds.left(), y + line_height * row),
                                size(bounds.size.width, line_height),
                            );
                            window.with_content_mask(
                                Some(gpui::ContentMask { bounds: row_bounds }),
                                |window| {
                                    let _ = line.paint(
                                        point(
                                            bounds.left() - px(scroll_left)
                                                + if row > 0 { indent } else { px(0.0) },
                                            y,
                                        ),
                                        line_height,
                                        gpui::TextAlign::Left,
                                        Some(bounds),
                                        window,
                                        cx,
                                    );
                                },
                            );
                        }
                    } else {
                        let _ = line.paint(
                            point(bounds.left() - px(scroll_left), y),
                            line_height,
                            gpui::TextAlign::Left,
                            Some(bounds),
                            window,
                            cx,
                        );
                    }
                    y += height;
                }
                if let Some((ghost_origin, ghost)) = prepaint.ghost.take() {
                    let style = window.text_style();
                    let font_size = style.font_size.to_pixels(window.rem_size());
                    let run = TextRun {
                        len: ghost.len(),
                        font: style.font(),
                        color: Theme::of(cx).text_faint,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(ghost, font_size, &[run], None);
                    // (Clipping comes from the surrounding content mask.)
                    let _ = line.paint(
                        ghost_origin,
                        line_height,
                        gpui::TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
                // Caret only when this input is actually focused in an active
                // window (Electron hides it on window deactivation too), and only
                // in the "on" blink phase — solid while typing, ~500ms blink idle.
                if self
                    .input
                    .update(cx, |input, cx| input.caret_shown(window, cx))
                    && let Some(cursor) = prepaint.cursor.take()
                {
                    window.paint_quad(cursor);
                }
            },
        );
        self.input.update(cx, |input, _| {
            input.last_lines = lines;
        });
    }
}

impl Render for ComposerInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.mentions_enabled && self.syntax_source != self.content {
            self.syntax_source = self.content.clone();
            self.syntax_spans.clear();
            let source = self.syntax_source.clone();
            // Keep grammar loading and parsing off the input/paint thread. A
            // dropped task and source check prevent stale edits recoloring text.
            self.syntax_task = Some(cx.spawn(async move |input, cx| {
                let (source, spans) = cx
                    .background_executor()
                    .spawn(async move {
                        let spans = composer_markdown::syntax_spans(&source);
                        (source, spans)
                    })
                    .await;
                input
                    .update(cx, |input, cx| {
                        if input.content == source {
                            input.syntax_spans = spans;
                            input.needs_measure = true;
                            cx.notify();
                        }
                    })
                    .ok();
            }));
        }
        let theme = Theme::of(cx);
        let popup_theme = theme.for_popup();
        let theme = if self.key_context == "PaletteSearch"
            || self.accessibility_role == gpui::Role::SearchInput
        {
            &popup_theme
        } else {
            theme
        };
        let text_color = if self.content.is_empty() {
            theme.text_faint
        } else {
            theme.text
        };
        div()
            .id(("composer-input", cx.entity_id()))
            .role(self.accessibility_role)
            .aria_label(self.placeholder.clone())
            .aria_placeholder(self.placeholder.clone())
            .key_context(self.key_context)
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::mention_tab))
            .on_action(cx.listener(Self::outdent_list))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::message_newline_or_accept))
            .on_action(cx.listener(Self::modified_submit))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, window, _| {
                // Capture runs before the clicked control handles the press, so
                // another input can take focus normally during bubbling.
                if event.button == MouseButton::Left && this.focus_handle.is_focused(window) {
                    window.blur();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
            .w_full()
            .text_size(crate::typography::ui_rems(self.text_size))
            .line_height(crate::typography::ui_rems(self.configured_line_height))
            .text_color(text_color)
            .font_family(theme.font_sans.clone())
            .child({
                let input = cx.entity();
                let ascent = self.max_ascent;
                crate::edge_fade::edge_faded(
                    INPUT_FADE_BAND,
                    true,
                    true,
                    ComposerTextElement {
                        input: input.clone(),
                        max_content_height: self
                            .viewport_height
                            .unwrap_or(TEXTAREA_MAX - TEXTAREA_PAD_V),
                    },
                )
                // Fade through the existing top padding, like the transcript
                // scrolling under its chrome. Account for GPUI's baseline
                // sampling without consuming another inset inside the text box.
                .inset_top(ascent - self.overflow_top_padding)
                .fade_overflow_y_with(move |cx| {
                    let input = input.read(cx);
                    let visible_height = input
                        .last_bounds
                        .map_or(0.0, |bounds| f32::from(bounds.size.height));
                    input_overflow_edges(
                        input.content_height,
                        input
                            .settled_viewport_height
                            .unwrap_or(TEXTAREA_MAX - TEXTAREA_PAD_V),
                        visible_height,
                        input.scroll_top,
                    )
                })
            })
    }
}

// ---------------------------------------------------------------------------
// Composer wrapper
// ---------------------------------------------------------------------------

/// Events the shell listens for.
#[derive(Debug, Clone)]
pub enum ComposerEvent {
    WorkspaceCommand(WorkspaceCommand),
    /// Arm the shared-element transition before the draft route is replaced
    /// by the newly-created session. Emitting this before `select_chat` keeps
    /// the first destination frame on the same timeline as the source frame.
    NewThreadTransitionStarted,
    /// A prompt was sent optimistically — give the transcript its exact row
    /// identity so it can anchor the prompt at the top with the reply's
    /// reserved space below it.
    Sent {
        chat_id: String,
        message_id: String,
    },
    /// A new worktree's host-side setup attempt completed after its chat id
    /// was minted. The shell attaches an already-open terminal to that exact
    /// chat, even when the user has selected another chat in the meantime.
    WorktreeSetup {
        chat_id: String,
        setup_action: Option<zeron_proto::ProjectActionRun>,
        setup_error: Option<String>,
        target_device_id: Option<String>,
    },
    /// A locally-authored queue row was accepted. It is not a transcript send
    /// yet: the transcript remembers the stable id and promotes it to an
    /// own-turn anchor only when the host materializes the matching bubble.
    Queued {
        chat_id: String,
        message_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MentionToken {
    range: Range<usize>,
    query: String,
}

/// Refine a locally identified token using Markdown source ranges. This is
/// deliberately called only after a trigger has passed cheap boundary checks.
fn completion_markdown_end(
    text: &str,
    start: usize,
    cursor: usize,
    mut end: usize,
) -> Option<usize> {
    use pulldown_cmark::{Event, Options, Parser, Tag};
    let needs_quote_boundary = text[..start].ends_with('>');
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let mut paragraph_start = line_start;
    let mut token_is_prose = false;
    let mut quote_boundary = false;
    let parser = Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH);
    // Definitions have no body events, but their destinations are still link
    // syntax and must never be replaced by a nested canonical link.
    if parser
        .reference_definitions()
        .iter()
        .any(|(_, definition)| definition.span.contains(&start))
    {
        return None;
    }
    for (event, range) in parser.into_offset_iter() {
        if !range.contains(&start) {
            continue;
        }
        match event {
            Event::Start(Tag::Link { .. } | Tag::Image { .. }) => return None,
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::Item) => {
                paragraph_start = range.start;
            }
            Event::Text(_) => token_is_prose = true,
            Event::Start(Tag::BlockQuote(_)) if needs_quote_boundary => {
                let prefix = &text[range.start.max(line_start)..start];
                quote_boundary |= prefix.chars().all(|ch| matches!(ch, '>' | ' ' | '\t'));
            }
            Event::Start(tag @ (Tag::Strong | Tag::Emphasis | Tag::Strikethrough)) => {
                let delimiter = match tag {
                    Tag::Strong => 2,
                    Tag::Emphasis => 1,
                    _ => text[range.clone()]
                        .bytes()
                        .take_while(|byte| *byte == b'~')
                        .count(),
                };
                let closing = range.end.saturating_sub(delimiter);
                if cursor > closing {
                    return None;
                }
                end = end.min(closing);
            }
            _ => {}
        }
    }
    if needs_quote_boundary && !quote_boundary {
        return None;
    }
    // Duplicate reference definitions are consumed without body events and
    // only the first definition is retained by reference_definitions(). Requiring
    // text also protects those later definitions and raw HTML without excluding
    // tight-list items, which omit paragraph events.
    if !token_is_prose {
        return None;
    }
    // The parser leaves an unfinished link as prose. Once its destination is
    // being authored, inserting a canonical link would create nested syntax.
    // An unfinished destination can continue across a soft line break. Stay
    // within its parsed paragraph so a malformed link in an earlier block does
    // not disable completion in later prose.
    let before = &text[paragraph_start..cursor];
    for (destination, _) in before.rmatch_indices("](") {
        if let Some(label) = before[..destination].rfind('[') {
            let escaped = before[..label]
                .bytes()
                .rev()
                .take_while(|byte| *byte == b'\\')
                .count()
                % 2
                == 1;
            let closing_escaped = before[..destination]
                .bytes()
                .rev()
                .take_while(|byte| *byte == b'\\')
                .count()
                % 2
                == 1;
            if !escaped && !closing_escaped && paragraph_start + destination + 2 <= start {
                let mut depth = 1usize;
                let mut chars = before[destination + 2..].chars();
                while let Some(ch) = chars.next() {
                    match ch {
                        '\\' => {
                            chars.next();
                        }
                        '(' => depth += 1,
                        ')' => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                if depth > 0 {
                    return None;
                }
            }
        }
    }
    (!in_code(text, cursor)).then_some(end)
}

/// The `@` must begin a token. This intentionally excludes `name@example.com`
/// and ordinary words while allowing punctuation such as `(@src`.
fn mention_token(text: &str, cursor: usize) -> Option<MentionToken> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let token_start = text[..cursor]
        .char_indices()
        .rev()
        .find_map(|(at, ch)| ch.is_whitespace().then_some(at + ch.len_utf8()))
        .unwrap_or(0);
    let Some(relative_at) = text[token_start..cursor].rfind('@') else {
        return None;
    };
    let at = token_start + relative_at;
    let valid_boundary = at == 0
        || text[..at]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_whitespace() || matches!(ch, '(' | '[' | '{' | '>'));
    if text[at + 1..cursor].contains('@') || !valid_boundary {
        return None;
    }
    let closing = text[..at].chars().next_back().and_then(|ch| match ch {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    });
    let end = text[cursor..]
        .char_indices()
        .find_map(|(offset, ch)| {
            (ch.is_whitespace() || Some(ch) == closing).then_some(cursor + offset)
        })
        .unwrap_or(text.len());
    if closing.is_some_and(|ch| text[at + 1..cursor].contains(ch)) {
        return None;
    }
    let end = completion_markdown_end(text, at, cursor, end)?;
    Some(MentionToken {
        range: at..end,
        query: text[at + 1..cursor].to_string(),
    })
}

/// Invocation triggers share file completion's boundary rules, but never
/// interpret paths, currency amounts or code as invocations.
fn slash_token(text: &str, cursor: usize) -> Option<MentionToken> {
    invocation_token(text, cursor, '/')
}

fn invocation_token(text: &str, cursor: usize, prefix: char) -> Option<MentionToken> {
    if cursor > text.len() || !text.is_char_boundary(cursor) {
        return None;
    }
    let start = text[..cursor]
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace() || matches!(c, '(' | '[' | '{' | '>'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    if !text[start..cursor].starts_with(prefix) {
        return None;
    }
    let query = &text[start + 1..cursor];
    let name_grapheme = |grapheme: &str| {
        grapheme.chars().next().is_some_and(char::is_alphanumeric)
            || matches!(grapheme, "-" | "_" | ":" | ".")
    };
    if !query.graphemes(true).all(name_grapheme)
        || (prefix == '$' && query.starts_with(char::is_numeric))
    {
        return None;
    }
    // Scan from the token's start, including any combining marks after a caret
    // positioned between code points of the same grapheme.
    let end = text[start + 1..]
        .grapheme_indices(true)
        .find_map(|(offset, grapheme)| (!name_grapheme(grapheme)).then_some(start + 1 + offset))
        .unwrap_or(text.len());
    if text.get(end..end + 1) == Some("/") {
        return None;
    }
    let end = completion_markdown_end(text, start, cursor, end)?;
    Some(MentionToken {
        range: start..end,
        query: query.to_string(),
    })
}

/// Pasted prose is eligible only at completion boundaries, never inside
/// Markdown code/links. Catalog matching below decides whether it is a reference.
fn pasted_reference_tokens(text: &str, pasted: Range<usize>) -> Vec<(char, MentionToken)> {
    if text.len() > 256 * 1024 || text.get(pasted.clone()).is_none() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    let mut attempts = 0;
    for (relative, prefix) in text[pasted.clone()].char_indices() {
        if !matches!(prefix, '@' | '$' | '/') {
            continue;
        }
        let start = pasted.start + relative;
        let mut end = start + 1;
        for ch in text[end..].chars() {
            if !(ch.is_alphanumeric()
                || matches!(ch, '_' | '-' | '.' | ':')
                || (prefix == '@' && ch == '/'))
            {
                break;
            }
            end += ch.len_utf8();
        }
        if end > pasted.end {
            continue;
        }
        // Do not chip a prefix of an email, URL query, or shell expression.
        if text[end..].starts_with(['@', '=', '\\'])
            || text[end..]
                .strip_prefix(['?', '#'])
                .is_some_and(|tail| tail.chars().next().is_some_and(|ch| !ch.is_whitespace()))
        {
            continue;
        }
        while end > start + 1 && matches!(text.as_bytes()[end - 1], b'.' | b':') {
            end -= 1;
        }
        if end == start + 1 {
            continue;
        }
        // Rejected code/link candidates still parse the whole document. Bound
        // attempts, not just accepted chips, so pasted examples cannot stall UI.
        if attempts == 32 {
            break;
        }
        attempts += 1;
        let token = if prefix == '@' {
            mention_token(text, end)
        } else {
            invocation_token(text, end, prefix)
        };
        if let Some(mut token) = token.filter(|token| token.range.start == start) {
            token.range.end = end;
            token.query = text[start + 1..end].to_owned();
            tokens.push((prefix, token));
        }
    }
    tokens
}

/// Interpret the trigger before discovery so a disabled $ stays ordinary text.
fn completion_trigger(
    text: &str,
    cursor: usize,
    preferences: crate::settings::SkillCompletionSettings,
) -> (Option<MentionToken>, bool, bool, bool) {
    let skill_token = preferences
        .dollar
        .then(|| invocation_token(text, cursor, '$'))
        .flatten();
    let skill = skill_token.is_some();
    let include_skills = skill || !preferences.separate_from_slash;
    let token = skill_token.or_else(|| slash_token(text, cursor));
    let commands_allowed = token.is_some() && !skill;
    (token, skill, include_skills, commands_allowed)
}

/// Human-readable presentation only; invocation names and paths stay canonical.
fn skill_display_name(name: &str) -> String {
    name.rsplit(':')
        .next()
        .unwrap_or(name)
        .split(|c: char| c == '-' || c == '_' || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            let first = chars.next().unwrap();
            first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Commands implemented by Zeron, independently of the provider protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceCommand {
    Model,
    New,
    Resume,
    Settings,
    Diff,
    Files,
    Terminal,
    Rename,
    Stop,
}

impl WorkspaceCommand {
    fn catalog() -> &'static [(Self, &'static str, &'static str, bool)] {
        &[
            (
                Self::Model,
                "model",
                "Zeron: choose agent, model, and reasoning",
                false,
            ),
            (Self::New, "new", "Zeron: start a new conversation", false),
            (
                Self::Resume,
                "resume",
                "Zeron: search and open conversations",
                false,
            ),
            (Self::Settings, "settings", "Zeron: open settings", false),
            (Self::Diff, "diff", "Zeron: open changes", true),
            (Self::Files, "files", "Zeron: open project files", true),
            (Self::Terminal, "terminal", "Zeron: open a terminal", true),
            (
                Self::Rename,
                "rename",
                "Zeron: rename this conversation",
                true,
            ),
            (Self::Stop, "stop", "Zeron: stop the active run", true),
        ]
    }
}

fn with_workspace_commands(
    mut rows: Vec<InvocationCandidate>,
    in_chat: bool,
) -> Vec<InvocationCandidate> {
    rows.retain(|row| row.workspace_command.is_none());
    for &(command, name, description, needs_chat) in WorkspaceCommand::catalog() {
        if needs_chat && !in_chat {
            continue;
        }
        // Keep provider commands intact. Explicit Zeron names remain available
        // when a provider owns the unqualified name.
        let mut name = name.to_string();
        while rows.iter().any(|row| row.name == name) {
            name = format!("zeron:{name}");
        }
        rows.push(InvocationCandidate {
            invocation: zeron_proto::invocation::Invocation::Command { name: name.clone() },
            name,
            description: description.into(),
            input_hint: None,
            workspace_command: Some(command),
        });
    }
    rows
}

fn workspace_command_for_text(
    text: &str,
    rows: &[InvocationCandidate],
) -> Option<WorkspaceCommand> {
    let end = text.trim_end().len();
    let token = slash_token(text, end)?;
    if !text[..token.range.start].trim().is_empty() {
        return None;
    }
    rows.iter()
        .find(|row| row.name == token.query)?
        .workspace_command
}

#[derive(Debug, Clone)]
struct InvocationCandidate {
    workspace_command: Option<WorkspaceCommand>,
    name: String,
    description: String,
    input_hint: Option<String>,
    invocation: zeron_proto::invocation::Invocation,
}

fn invocation_insertion(
    invocation: &zeron_proto::invocation::Invocation,
    supported: bool,
) -> String {
    if !supported
        && matches!(
            invocation,
            zeron_proto::invocation::Invocation::Command { .. }
        )
    {
        invocation.prompt_text()
    } else {
        invocation.link()
    }
}

fn references_require_update(text: &str, supported: bool) -> bool {
    !supported
        && (!zeron_proto::invocation::invocation_links(text).is_empty()
            || !zeron_proto::file_mentions::file_mention_links(text).is_empty())
}

/// Slash-command completion state: like [`FileMentionState`] but the
/// candidate list is scoped to device, harness and workspace, then filtered
/// locally per keystroke. Commands and skills share focus and keyboard handling.
#[derive(Debug, Clone, Default)]
struct SlashState {
    token: Option<MentionToken>,
    /// Indices into the cached command list, filter-ranked for the query.
    filtered: Vec<usize>,
    active: Option<usize>,
    /// Harness the popup is showing commands for (cache key).
    harness: Option<HarnessId>,
    /// Shared identity for the command and skill catalogs. Switching trigger
    /// characters should not invalidate another catalog for the same provider.
    catalog_context: String,
    context: String,
    skill: bool,
    supported: bool,
    request: u64,
    loading: bool,
    error: Option<SharedString>,
    dismissed: Option<(Range<usize>, String)>,
}

#[derive(Debug, Clone, Default)]
struct FileMentionState {
    /// Workspace/device identity; unchanged text must still refresh after a checkout switch.
    context: String,
    token: Option<MentionToken>,
    results: Vec<FileSearchMatch>,
    active: Option<usize>,
    request: u64,
    loading: bool,
    /// Why the last search failed, for the popup. A failure MUST NOT render
    /// as "No matching files": cross-device searches fail for reasons the
    /// user can act on (host daemon too old for `SearchFiles`, device
    /// offline), and the empty state hid them (user report).
    error: Option<SharedString>,
    /// Full token text, not just the cursor-relative query: moving within a
    /// dismissed token keeps it closed, while any edit re-enables completion.
    dismissed: Option<(Range<usize>, String)>,
}

fn mention_response_is_current(state: &FileMentionState, request: u64) -> bool {
    state.request == request && state.token.is_some()
}

/// A failed file search, translated for the popup. `UnknownMethod` is the
/// version-skew case: `SearchFiles` shipped after v0.1.9, so a session hosted
/// by a device on an older daemon answers "unknown method" while the same
/// search works for local sessions.
fn mention_error_message(err: &RpcError) -> SharedString {
    match err {
        RpcError::UnknownMethod(_) => {
            "The session's device runs an older zeron — update it to search its files".into()
        }
        RpcError::Transport(_) | RpcError::Closed => "The session's device is unreachable".into(),
        RpcError::BadParams(_) | RpcError::Failed(_) => "File search failed".into(),
    }
}

/// A failed command discovery, translated for the popup.
fn invocation_candidates(
    commands: Vec<SlashCommand>,
    skills: Vec<zeron_proto::invocation::Skill>,
) -> Vec<InvocationCandidate> {
    use zeron_proto::invocation::{
        valid_invocation_name, valid_skill_command_name, valid_skill_path,
    };
    // A remote engine may use an older catalog decoder. Every visible choice
    // must still survive the local canonical-reference decoder unchanged.
    let skills: Vec<_> = skills
        .into_iter()
        .filter(|skill| {
            valid_invocation_name(&skill.name)
                && valid_skill_path(&skill.path)
                && skill
                    .command
                    .as_ref()
                    .is_none_or(|command| valid_skill_command_name(&command.name))
        })
        .collect();
    let skill_commands: std::collections::HashSet<_> = skills
        .iter()
        .filter_map(|skill| skill.command.as_ref().map(|command| command.name.as_str()))
        .collect();
    let commands: Vec<_> = commands
        .into_iter()
        .filter(|command| {
            valid_invocation_name(&command.name) && !skill_commands.contains(command.name.as_str())
        })
        .collect();
    commands
        .into_iter()
        .map(|c| InvocationCandidate {
            workspace_command: None,
            input_hint: c.input_hint,
            name: c.name.clone(),
            description: c.description,
            invocation: zeron_proto::invocation::Invocation::Command { name: c.name },
        })
        .chain(
            skills
                .into_iter()
                .filter(|s| s.enabled)
                .map(|s| InvocationCandidate {
                    workspace_command: None,
                    input_hint: None,
                    name: s.name.clone(),
                    description: if zeron_proto::invocation::native_skill_identity(&s.path) {
                        s.description.clone()
                    } else {
                        format!("{} — {}", s.description, s.path)
                    },
                    invocation: zeron_proto::invocation::Invocation::Skill {
                        name: s.name,
                        path: s.path,
                        command: s.command,
                    },
                }),
        )
        .collect()
}

fn merge_invocation_results(
    commands: Result<Vec<SlashCommand>, RpcError>,
    skills: Result<Option<Vec<zeron_proto::invocation::Skill>>, RpcError>,
    skill_only: bool,
) -> Result<(Vec<InvocationCandidate>, bool, Option<SharedString>), RpcError> {
    match (commands, skills) {
        (Ok(commands), Ok(skills)) => {
            let supported = !skill_only || skills.is_some();
            Ok((
                invocation_candidates(commands, skills.unwrap_or_default()),
                supported,
                None,
            ))
        }
        (Ok(commands), Err(error)) if !skill_only && !commands.is_empty() => Ok((
            invocation_candidates(commands, vec![]),
            true,
            Some(slash_error_message(&error, true)),
        )),
        (Err(error), Ok(Some(skills))) if skills.iter().any(|skill| skill.enabled) => Ok((
            invocation_candidates(vec![], skills),
            true,
            Some(slash_error_message(&error, false)),
        )),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

fn slash_error_message(err: &RpcError, skill: bool) -> SharedString {
    match err {
        RpcError::UnknownMethod(_) => {
            if skill {
                "Skills require an updated engine on the selected device. Restart that device’s Zeron after updating.".into()
            } else {
                "Commands require an updated engine on the selected device. Restart that device’s Zeron after updating.".into()
            }
        }
        RpcError::Transport(_) | RpcError::Closed => "The session's device is unreachable".into(),
        RpcError::BadParams(_) | RpcError::Failed(_) => {
            if skill {
                "Couldn't load this agent's skills".into()
            } else {
                "Couldn't load this agent's commands".into()
            }
        }
    }
}

pub struct Composer {
    pub(crate) state: Entity<AppState>,
    pub(crate) input: Entity<ComposerInput>,
    /// Draft displaced while a queued message occupies the composer.
    pub(crate) queue_edit_draft: Option<(String, Vec<StagedAttachment>, Vec<CapturedAppshot>)>,
    /// Composer actions row plus the new-session floating target tab
    /// ([`Pickers::render_new_thread_target_selectors`]).
    pickers: Entity<Pickers>,
    /// Draft text per chat key ("" = new-chat canvas), surviving navigation.
    drafts: HashMap<String, String>,
    /// Staged-but-unsent attachments per chat key (use-attachments.ts `stash`):
    /// navigating away and back restores them; memory-only, like the original.
    pub(crate) attachments: HashMap<String, Vec<StagedAttachment>>,
    /// Rich window captures keyed exactly like drafts and ordinary staged
    /// attachments. Each owns one screenshot that joins the existing upload
    /// path only at send time.
    pub(crate) appshots: HashMap<String, Vec<CapturedAppshot>>,
    appshot_entrances: HashMap<String, Instant>,
    /// The staged attachment being viewed full-size (click a thumbnail).
    preview: Option<attachments::PreviewImage>,
    /// Focused while the lightbox is open so Escape reaches it; the input
    /// gets focus back on close.
    preview_focus: FocusHandle,
    /// Focus grab deferred to the next render (open sites don't all have a
    /// `Window` — the `ZERON_ATTACH_PREVIEW` boot knob opens in `new`).
    preview_focus_pending: bool,
    /// In-flight file-picker prompt (paperclip).
    picker_task: Option<Task<()>>,
    mention_task: Option<Task<()>>,
    mention: FileMentionState,
    slash_task: Option<Task<()>>,
    slash: SlashState,
    /// Advertised invocations for the current device/harness/workspace.
    /// Invalidated on context changes; filtering stays local while typing.
    slash_cache: HashMap<String, Vec<InvocationCandidate>>,
    /// Slash-popup row scroll — the stack overflows into a wheel/keyboard-
    /// scrollable list once it outgrows the card.
    slash_scroll: gpui::ScrollHandle,
    /// File-mention popup row scroll (same treatment).
    mention_scroll: gpui::ScrollHandle,
    /// Shared scrollbar hover/drag state for both popups' floating rails —
    /// they never show at once (mutually exclusive by token shape).
    popup_bar: crate::popover::MenuScrollbarState,
    pub(crate) current_key: String,
    sending: bool,
    /// Armed immediately before a blank-canvas send selects its minted chat.
    /// The state observer consumes it to distinguish that handoff from normal
    /// session navigation, which must continue to snap.
    launching_new_chat: bool,
    pub(crate) failure: Option<SharedString>,
    /// The chat key `failure` belongs to (`None` = global, e.g. "Engine not
    /// connected"). Chat-scoped failures survive navigation and render only
    /// under their own chat — a blanket clear-on-switch erased the one
    /// visible trace of a failed send (2026-08-19).
    failure_key: Option<String>,
    wizard: Option<Wizard>,
    wizard_focus: FocusHandle,
    /// Requests already answered locally (suppresses the panel until the doc
    /// frame marks them resolved).
    answered_requests: HashSet<String>,
    advance_task: Option<Task<()>>,
    send_task: Option<Task<()>>,
    /// The queued message being edited in the composer (see
    /// [`Composer::begin_queue_edit`]).
    pub(crate) editing_queued: Option<String>,
    /// Host-issued generation protecting `editing_queued` from automatic
    /// delivery. The text buffer is kept until Finish receives an ACK.
    pub(crate) queue_edit_lease_id: Option<String>,
    pub(crate) queue_edit_base_text_hash: Option<String>,
    pub(crate) queue_edit_chat_id: Option<String>,
    pub(crate) queue_edit_host_device_id: Option<String>,
    pub(crate) queue_edit_instance_id: String,
    pub(crate) queue_edit_pending_id: Option<String>,
    pub(crate) queue_edit_finishing: bool,
    pub(crate) queue_edit_task: Option<Task<()>>,
    pub(crate) queue_edit_renew_task: Option<Task<()>>,
    /// Focus once on mount, navigation, or after opening/closing a queue edit.
    pub(crate) focus_pending: bool,
    /// Live drag over the queue panel: which row, and where it would land.
    pub(crate) queue_drag: Option<crate::queue::QueueDragState>,
    pub(crate) queue_scroll: gpui::ScrollHandle,
    pub(crate) queue_full_preview: Option<Task<()>>,
    pub(crate) queue_previews: HashMap<(String, String), crate::queue::QueuePreview>,
    /// Rows awaiting a host-authoritative removal acknowledgement. They stay
    /// visible but inert until the host wins the race against queue delivery.
    pub(crate) queue_removing: HashSet<String>,
    /// Whether the modifier overlay should currently reveal the queue hint.
    /// The shell owns modifier tracking and clears this on window deactivation.
    queue_shortcut_revealed: bool,
    /// Interrupt/answer commands get their own slot: assigning `send_task`
    /// DROPPED an in-flight send future mid-upload — no banner, no cleanup,
    /// `sending` stuck true forever (2026-08-19 incident, "press Stop while
    /// a send grinds" shape).
    action_task: Option<Task<()>>,
    /// Chats whose durable Interrupt command has been accepted or is still
    /// being queued. Kept independently so stopping one chat cannot replace
    /// another chat's request when the user navigates quickly.
    interrupting: HashSet<String>,
    interrupt_tasks: HashMap<String, Task<()>>,
    // -- compact/expanded flip state (hysteresis; see `composer_flip`) --
    /// Current layout mode (persisted across frames — never derived fresh).
    expanded_mode: bool,
    /// `layout_epoch` of the measurement that caused the last flip: the flip is
    /// re-evaluated only after the input has been laid out in the new mode, so
    /// at most one flip can happen per layout pass.
    flip_epoch: u64,
    /// Compact-mode input capacity, learned while compact (layout-stable).
    compact_capacity: f32,
    /// Input width first measured after expanding — container-width deltas
    /// while expanded shift `compact_capacity` by the same amount.
    expanded_anchor: f32,
    /// Last input width seen in the current mode (resize detection).
    last_seen_width: f32,
    /// Stable outer composer width supplied by the shell. Unlike Taffy's
    /// provisional input measurements, this changes only when the actual
    /// conversation column changes and can safely drive a follow-up render.
    last_available_width: Option<f32>,
    /// Set while an interactive resize is in flight; collapse is deferred
    /// until widths have settled for [`RESIZE_SETTLE_MS`].
    width_changed_at: Option<Instant>,
    settle_task: Option<Task<()>>,
    /// In-flight compact↔expanded morph (one per committed flip; manual
    /// drive — see [`FlipMorph`]).
    flip_morph: Option<FlipMorph>,
    /// Pill height actually rendered last frame — a committed flip morphs
    /// from here, so mid-flight reversals hand off without a jump.
    last_rendered_height: f32,
    model_handoff_position: f32,
    model_handoff_from: f32,
    model_handoff_morph: Option<FlipMorph>,
    model_bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    dock_frame: Option<crate::composer_dock::DockFrame>,
    dock_reflow: crate::composer_dock::DockReflow,
    /// The shared clock owns this frame's height, including its final step.
    dock_height_changed: bool,
    dock_clearance_correction: f32,
    surface_bounds: crate::new_thread_background_mask::SurfaceBounds,
    last_target_height: f32,
    height_morph: Option<FlipMorph>,
    /// Monotonic clock anchor for the morph timeline.
    morph_clock: Instant,
    /// Set on every session/route change: flips committed before this instant
    /// SNAP instead of morphing (see [`ROUTE_SNAP_MS`]).
    route_snap_until: Option<Instant>,
    _observe: Subscription,
    _pickers_observe: Subscription,
    /// The footer's rings: plan usage of the session harness's live
    /// account, and context occupancy — each opening a popover.
    account_usage: Entity<crate::account_usage::AccountUsage>,
    /// A side chat's composer: its footer keeps only the context ring.
    side_chat: bool,
    _picker_focus: Subscription,
    _input_events: Subscription,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub(crate) fn set_dock_frame(
        &mut self,
        frame: crate::composer_dock::DockFrame,
        cx: &mut Context<Self>,
    ) {
        let changed = self.dock_frame != Some(frame);
        self.dock_height_changed |= self
            .dock_frame
            .is_none_or(|previous| previous.amount != frame.amount);
        self.dock_frame = Some(frame);
        if frame.active {
            self.flip_morph = None;
            self.height_morph = None;
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn dock_clearance_correction(&self) -> f32 {
        self.dock_clearance_correction
    }

    pub(crate) fn surface_bounds(&self) -> crate::new_thread_background_mask::SurfaceBounds {
        self.surface_bounds.clone()
    }

    /// The picker entity, for the shell's canvas target selectors.
    pub fn pickers(&self) -> &Entity<Pickers> {
        &self.pickers
    }

    /// Feed the shell's current outer width into responsive composer controls.
    /// During a route transition this is the interpolated width, including
    /// when returning from a wider conversation to the new-chat composer.
    pub fn set_available_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let composer_width = width.max(0.0);
        if composer_width_changed(self.last_available_width, composer_width) {
            self.last_available_width = Some(composer_width);
            // The shell renders before this child, so this queues one more
            // pass after the input has been laid out at its final width. That
            // pass can consume the completed measurement without emitting an
            // event from inside Taffy's multi-pass measurement callback.
            cx.notify();
        }
    }

    pub(crate) fn set_queue_shortcut_revealed(&mut self, revealed: bool, cx: &mut Context<Self>) {
        if self.queue_shortcut_revealed != revealed {
            self.queue_shortcut_revealed = revealed;
            cx.notify();
        }
    }

    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        cx.on_release(|this, cx| this.release_queue_previews(cx))
            .detach();
        let input = cx.new(|cx| {
            let mut input =
                ComposerInput::with_context("Do anything…", MESSAGE_COMPOSER_CONTEXT, cx);
            input.enable_mentions();
            input
        });
        let pickers = cx.new(|cx| Pickers::new(state.clone(), cx));
        let account_usage = cx.new(|cx| crate::account_usage::AccountUsage::new(state.clone(), cx));
        // The footer toolbar (checkout kind + ref picker) is rendered INLINE
        // by the composer from picker state — a pickers-side notify (refs
        // loaded, popover toggled, pick made) must repaint the composer too.
        let pickers_observe = cx.observe(&pickers, |this, _, cx| {
            this.on_input_edited(cx);
            cx.notify();
        });
        let picker_focus = cx.subscribe(
            &pickers,
            |this: &mut Self, _, _: &crate::pickers::ReturnComposerFocus, cx| {
                this.focus_pending = true;
                cx.notify();
            },
        );
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.on_state_changed(cx));
        let input_events = cx.subscribe(&input, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Submitted => this.on_submit(cx),
            ComposerInputEvent::ModifiedSubmitted => this.on_modified_submit(cx),
            ComposerInputEvent::Edited | ComposerInputEvent::CursorMoved => {
                this.on_input_edited(cx)
            }
            ComposerInputEvent::ViewportChanged => cx.notify(),
            // The slash popup and the mention popup share the input's
            // completion key routing; they are mutually exclusive by token
            // shape (`/`, `$`, or `@` at a token boundary).
            ComposerInputEvent::MentionNavigate(delta) => {
                if this.slash.token.is_some() {
                    this.move_slash(*delta, cx)
                } else {
                    this.move_mention(*delta, cx)
                }
            }
            ComposerInputEvent::MentionAccept => {
                if this.slash.token.is_some() {
                    this.accept_slash(cx)
                } else {
                    this.accept_mention(cx)
                }
            }
            ComposerInputEvent::MentionDismiss => {
                if this.slash.token.is_some() {
                    this.dismiss_slash(cx)
                } else {
                    this.dismiss_mention(cx)
                }
            }
            ComposerInputEvent::PastedImages(images) => {
                let staged = images
                    .iter()
                    .map(|image| attachments::stage_clipboard_image(image.clone()))
                    .collect();
                this.add_staged(staged, cx);
            }
            ComposerInputEvent::PastedPaths(paths) => this.add_paths(paths.clone(), cx),
            ComposerInputEvent::PastedText { range, revision } => {
                this.resolve_pasted_references(range.clone(), *revision, cx);
            }
        });
        cx.observe_global::<crate::settings::SettingsStore>(|this, cx| {
            this.on_input_edited(cx);
        })
        .detach();
        let current_key = state.read(cx).selected_chat.clone().unwrap_or_default();
        let mut composer = Self {
            state,
            input,
            queue_edit_draft: None,
            pickers,
            drafts: HashMap::new(),
            attachments: HashMap::new(),
            appshots: HashMap::new(),
            appshot_entrances: HashMap::new(),
            preview: None,
            preview_focus: cx.focus_handle(),
            preview_focus_pending: false,
            picker_task: None,
            mention_task: None,
            mention: FileMentionState::default(),
            slash_task: None,
            slash: SlashState::default(),
            slash_cache: HashMap::new(),
            slash_scroll: gpui::ScrollHandle::new(),
            mention_scroll: gpui::ScrollHandle::new(),
            popup_bar: crate::popover::MenuScrollbarState::default(),
            current_key,
            sending: false,
            launching_new_chat: false,
            failure: None,
            wizard: None,
            wizard_focus: cx.focus_handle(),
            answered_requests: HashSet::new(),
            failure_key: None,
            action_task: None,
            advance_task: None,
            send_task: None,
            interrupting: HashSet::new(),
            interrupt_tasks: HashMap::new(),
            editing_queued: None,
            queue_edit_lease_id: None,
            queue_edit_base_text_hash: None,
            queue_edit_chat_id: None,
            queue_edit_host_device_id: None,
            queue_edit_instance_id: uuid::Uuid::new_v4().to_string(),
            queue_edit_pending_id: None,
            queue_edit_finishing: false,
            queue_edit_task: None,
            queue_edit_renew_task: None,
            focus_pending: true,
            queue_drag: None,
            queue_scroll: gpui::ScrollHandle::new(),
            queue_full_preview: None,
            queue_previews: HashMap::new(),
            queue_removing: HashSet::new(),
            queue_shortcut_revealed: false,
            expanded_mode: false,
            flip_epoch: 0,
            compact_capacity: 0.0,
            expanded_anchor: 0.0,
            last_seen_width: 0.0,
            last_available_width: None,
            width_changed_at: None,
            settle_task: None,
            flip_morph: None,
            last_rendered_height: 0.0,
            model_handoff_position: 1.0,
            model_handoff_from: 1.0,
            model_handoff_morph: None,
            model_bounds: Default::default(),
            dock_frame: None,
            dock_reflow: Default::default(),
            dock_height_changed: false,
            dock_clearance_correction: 0.0,
            surface_bounds: Default::default(),
            last_target_height: 0.0,
            height_morph: None,
            morph_clock: Instant::now(),
            route_snap_until: None,
            _observe: observe,
            _pickers_observe: pickers_observe,
            account_usage,
            side_chat: false,
            _picker_focus: picker_focus,
            _input_events: input_events,
        };
        // Dev knob: pre-stage attachments (drop/paste can't be synthesized on
        // a rig) — `ZERON_ATTACH=/path/a.png[,/path/b.png]`, and
        // `ZERON_ATTACH_PREVIEW=1` boots with the first one's lightbox open.
        if let Ok(spec) = std::env::var("ZERON_ATTACH") {
            let staged: Vec<StagedAttachment> = spec
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .filter_map(|path| {
                    match attachments::stage_file(std::path::Path::new(path.trim())) {
                        Ok(att) => Some(att),
                        Err(err) => {
                            tracing::warn!(%path, error = %err, "ZERON_ATTACH stage failed");
                            None
                        }
                    }
                })
                .collect();
            if std::env::var("ZERON_ATTACH_PREVIEW").is_ok_and(|v| v == "1")
                && let Some(first) = staged.first()
            {
                composer.preview = Some(attachments::PreviewImage::new(
                    first.name.clone(),
                    first.image.clone(),
                ));
                composer.preview_focus_pending = true;
            }
            if !staged.is_empty() {
                composer
                    .attachments
                    .entry(composer.current_key.clone())
                    .or_default()
                    .extend(staged);
            }
        }
        composer
    }

    /// Capture-knob passthrough (`ZERON_OPEN_DIALOG=model`): open the
    /// combined harness/model menu.
    pub fn open_model_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pickers
            .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
    }

    pub fn is_sending(&self) -> bool {
        self.sending
    }

    pub(crate) fn can_edit_queue_in_composer(&self) -> bool {
        !self.sending && self.wizard.is_none()
    }

    // ---- attachment staging (use-attachments.ts) ----

    /// Staged attachments for the chat the composer is showing.
    pub(crate) fn staged(&self) -> &[StagedAttachment] {
        self.attachments
            .get(&self.current_key)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    pub(crate) fn queue_preview_limit(&self) -> usize {
        if self.last_available_width.unwrap_or(COMPOSER_MAX_WIDTH) < 520.0 {
            1
        } else {
            2
        }
    }

    pub(crate) fn show_queue_image(
        &mut self,
        preview: attachments::PreviewImage,
        cx: &mut Context<Self>,
    ) {
        self.preview = Some(preview);
        self.preview_focus_pending = true;
        cx.notify();
    }

    pub(crate) fn staged_appshots(&self) -> &[CapturedAppshot] {
        self.appshots
            .get(&self.current_key)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn stage_appshot(&mut self, appshot: CapturedAppshot, cx: &mut Context<Self>) {
        self.stage_appshot_for(self.current_key.clone(), appshot, cx);
    }

    pub fn stage_appshot_for(
        &mut self,
        key: String,
        appshot: CapturedAppshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let staged_bytes = self
            .appshots
            .get(&key)
            .into_iter()
            .flatten()
            .map(|shot| shot.screenshot.bytes().len() as u64)
            .sum::<u64>();
        let saved_bytes = self
            .queue_edit_draft
            .as_ref()
            .filter(|_| key == self.current_key)
            .map(|(_, _, shots)| {
                shots
                    .iter()
                    .map(|shot| shot.screenshot.bytes().len() as u64)
                    .sum::<u64>()
            })
            .unwrap_or_default();
        let staged_bytes = staged_bytes.saturating_add(saved_bytes);
        let incoming = appshot.screenshot.bytes().len() as u64;
        if incoming > attachments::MAX_ATTACHMENT_BYTES
            || staged_bytes.saturating_add(incoming) > appshots::MAX_STAGED_APPSHOT_BYTES
        {
            self.failure = Some(
                "Remove an Appshot before adding another (96 MB staged Appshot limit).".into(),
            );
            self.failure_key = Some(key);
            cx.notify();
            return false;
        }
        self.appshot_entrances
            .retain(|_, start| start.elapsed().as_secs_f32() < motion::speed_scale());
        if !motion::reduced_motion(cx) {
            self.appshot_entrances
                .insert(appshot.id.clone(), Instant::now());
        }
        // A capture completing while a queue edit is being saved belongs to
        // the displaced draft; it must not be lost or change the in-flight edit.
        if self.queue_edit_finishing && key == self.current_key {
            if let Some((_, _, saved_appshots)) = &mut self.queue_edit_draft {
                saved_appshots.push(appshot);
            } else {
                self.appshots.entry(key).or_default().push(appshot);
            }
        } else {
            self.appshots.entry(key).or_default().push(appshot);
        }
        self.failure = None;
        self.failure_key = None;
        cx.notify();
        true
    }

    pub fn show_appshot_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.show_error(message, cx);
    }

    /// Mark this as a side chat's composer: below the input it shows only
    /// the context ring (no checkout/ref footer, no plan usage).
    pub(crate) fn set_side_chat(&mut self, cx: &mut Context<Self>) {
        self.side_chat = true;
        cx.notify();
    }

    /// Whether the draft holds anything a close would lose: text, staged
    /// attachments or appshots, or staged review comments.
    pub(crate) fn has_draft(&self, cx: &App) -> bool {
        composer_has_content(
            self.input.read(cx).text(),
            self.staged().len() + self.staged_appshots().len(),
            self.staged_comments(cx).len(),
        )
    }

    #[cfg(test)]
    pub(crate) fn failure(&self) -> Option<&SharedString> {
        self.failure.as_ref()
    }

    /// Show a dismissable failure chip for the current draft's session.
    pub(crate) fn show_error(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.failure = Some(message.into());
        self.failure_key = Some(self.current_key.clone());
        cx.notify();
    }

    fn add_staged(&mut self, staged: Vec<StagedAttachment>, cx: &mut Context<Self>) {
        if self.queue_edit_finishing {
            return;
        }
        if staged.is_empty() {
            return;
        }
        self.attachments
            .entry(self.current_key.clone())
            .or_default()
            .extend(staged);
        self.focus_pending = true;
        cx.notify();
    }

    /// Stage image files (picker / drop / pasted paths). Non-images are
    /// skipped silently (matching the original's `image/*` filter); read
    /// failures and oversize files surface in the failure notice.
    pub(crate) fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut staged = Vec::new();
        for path in &paths {
            if attachments::format_by_extension(path).is_none() {
                continue;
            }
            match attachments::stage_file(path) {
                Ok(att) => staged.push(att),
                Err(message) => {
                    self.failure = Some(message.into());
                    self.failure_key = Some(self.current_key.clone());
                    cx.notify();
                }
            }
        }
        self.add_staged(staged, cx);
    }

    /// Add a file-tree or file-tab drop through the existing file-mention
    /// pipeline. This keeps the reference workspace-relative and therefore
    /// valid for local and remote sessions alike.
    pub(crate) fn add_workspace_path(
        &mut self,
        path: &str,
        is_directory: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inserted = self.input.update(cx, |input, cx| {
            input.insert_dropped_mention(path, is_directory, cx)
        });
        if inserted {
            self.reset_mention(None, cx);
            self.reset_slash(None, cx);
            let focus = self.input.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            cx.notify();
        }
    }

    fn remove_attachment(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.queue_edit_finishing {
            return;
        }
        if let Some(list) = self.attachments.get_mut(&self.current_key) {
            list.retain(|a| a.id != id);
            if list.is_empty() {
                self.attachments.remove(&self.current_key);
            }
        }
        cx.notify();
    }

    fn restore_failed_appshots(
        &mut self,
        sent: &[CapturedAppshot],
        failed_key: &str,
        restore_key: &str,
    ) {
        if sent.is_empty() {
            return;
        }
        let mut merged = sent.to_vec();
        for key in [failed_key, restore_key] {
            for shot in self.appshots.remove(key).unwrap_or_default() {
                if !merged.iter().any(|existing| existing.id == shot.id) {
                    merged.push(shot);
                }
            }
        }
        self.appshots.insert(restore_key.to_string(), merged);
    }

    fn remove_appshot(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.queue_edit_finishing {
            return;
        }
        if let Some(list) = self.appshots.get_mut(&self.current_key) {
            list.retain(|appshot| appshot.id != id);
            if list.is_empty() {
                self.appshots.remove(&self.current_key);
            }
        }
        cx.notify();
    }

    /// Drop a deleted chat's per-chat composer state — staged attachments hold
    /// raw image bytes, and a deleted chat's stage could never be sent again.
    pub fn purge_chat(&mut self, chat_id: &str, cx: &mut Context<Self>) {
        self.attachments.remove(chat_id);
        self.appshots.remove(chat_id);
        self.state.update(cx, |state, _| {
            state.purge_review_comments(chat_id);
        });
    }

    /// Staged in `AppState` because the changes pane writes them.
    fn staged_comments(&self, cx: &App) -> Vec<crate::comments::ReviewComment> {
        self.state
            .read(cx)
            .review_comments(&self.current_key)
            .to_vec()
    }

    fn render_comments_chip(&self, theme: &Theme, cx: &App) -> Option<gpui::Div> {
        let count = self.staged_comments(cx).len();
        if count == 0 {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_row()
                .px(px(STRIP_PAD_X))
                .pt(px(STRIP_PAD_TOP))
                .child(crate::badges::render(
                    "composer-comments",
                    &crate::badges::MessageBadge {
                        icon: crate::icons::CHAT_ROUND_LINE,
                        label: crate::comments::chip_label(count).into(),
                        // The staged set is already on screen in the changes
                        // pane, so a hover card would only repeat it.
                        details: Vec::new(),
                    },
                    theme,
                )),
        )
    }

    /// The staged-thumbnail strip (attachment-ui.tsx AttachmentStrip):
    /// `flex flex-wrap gap-2 px-4 pt-3`, 56px rounded thumbs, a remove button
    /// revealed on hover, click opens the full-size preview.
    fn render_attachment_strip(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let staged = self.staged();
        if staged.is_empty() {
            return None;
        }
        let mut strip = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(STRIP_GAP))
            .px(px(STRIP_PAD_X))
            .pt(px(STRIP_PAD_TOP));
        for (ix, att) in staged.iter().enumerate() {
            let group: SharedString = format!("composer-att-{}", att.id).into();
            let preview = attachments::PreviewImage::new(att.name.clone(), att.image.clone());
            let remove_id = att.id.clone();
            strip = strip.child(
                div()
                    .group(group.clone())
                    .flex_none()
                    .relative()
                    .child(
                        div()
                            .id(("composer-att-thumb", ix))
                            .size(px(STRIP_THUMB))
                            .rounded(px(8.0))
                            .overflow_hidden()
                            .border_1()
                            .border_color(crate::theme::hairline(0.10))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                preview.viewer.reset();
                                this.preview = Some(preview.clone());
                                this.preview_focus_pending = true;
                                cx.notify();
                            }))
                            .child(
                                img(att.image.clone())
                                    // EXPLICIT dims, not size_full: img layout
                                    // honors the image's intrinsic aspect
                                    // ratio over a percent height (gpui
                                    // f8d8a90 repoint), so size_full let a
                                    // tall photo grow past the frame — the
                                    // rectangular overflow clip then squared
                                    // the bottom corners (2026-08-19 report).
                                    // 56−2 = frame minus its 1px borders.
                                    .w(px(STRIP_THUMB - 2.0))
                                    .h(px(STRIP_THUMB - 2.0))
                                    // Own radii — the frame's rounding only
                                    // clips rectangularly (7 = 8 - border).
                                    .rounded(px(7.0))
                                    .object_fit(ObjectFit::Cover),
                            ),
                    )
                    // Own layer: inside the frosted pill everything shares one
                    // draw order and images render last, so without it the
                    // thumbnail paints OVER this button (user report).
                    .child(crate::frost::layered(
                        div()
                            .id(("composer-att-remove", ix))
                            .absolute()
                            .top(px(-6.0))
                            .right(px(-6.0))
                            .size(px(18.0))
                            .rounded_full()
                            .bg(theme.bg)
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .shadow_sm()
                            .opacity(0.0)
                            .group_hover(group, |s| s.opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                // The button overhangs the thumbnail, whose
                                // hitbox is right underneath — don't let the
                                // same click also open the preview.
                                cx.stop_propagation();
                                this.remove_attachment(&remove_id, cx);
                            }))
                            .child(
                                crate::icons::icon(crate::icons::CLOSE_CIRCLE)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            ),
                    )),
            );
        }
        Some(strip)
    }

    fn render_appshot_strip(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let appshots = self.staged_appshots();
        if appshots.is_empty() {
            return None;
        }
        let mut strip = div()
            .id("composer-appshots-strip")
            .flex()
            .flex_row()
            .gap(px(STRIP_GAP))
            .px(px(STRIP_PAD_X))
            .pt(px(STRIP_PAD_TOP))
            .overflow_x_scroll();
        let max_image_width = self.last_available_width.unwrap_or(COMPOSER_MAX_WIDTH)
            - 2.0 * Theme::SPACE_LG
            - 2.0
            - 2.0 * STRIP_PAD_X
            - 2.0 * APPSHOT_IMAGE_INSET;
        for (ix, appshot) in appshots.iter().enumerate() {
            let group: SharedString = format!("composer-appshot-{}", appshot.id).into();
            let preview = crate::attachments::PreviewImage::new(
                appshot.screenshot.name.clone(),
                appshot.screenshot.image.clone(),
            );
            let preview_on_key = preview.clone();
            let preview_on_a11y = preview.clone();
            let composer_for_preview = cx.entity().downgrade();
            let remove_id = appshot.id.clone();
            let remove_on_key_id = remove_id.clone();
            let remove_on_a11y_id = remove_id.clone();
            let composer_for_remove = cx.entity().downgrade();
            let source: SharedString = appshot
                .window_title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| appshot.app_name.clone())
                .into();
            let preview_label: SharedString = format!("Preview {source}").into();
            let remove_label: SharedString = format!("Remove {source}").into();
            let preview_aria = preview_label.clone();
            let remove_aria = remove_label.clone();
            let (image_width, image_height) =
                appshot_contained_size(appshot.screenshot_dimensions, max_image_width);
            let tile_width = (image_width + 2.0 * APPSHOT_IMAGE_INSET).max(APPSHOT_TILE_MIN_WIDTH);
            let mut card = div()
                .id(("composer-appshot", ix))
                .group(group.clone())
                .relative()
                .w(px(tile_width))
                .h(px(APPSHOT_TILE_HEIGHT))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .rounded(px(14.0))
                .overflow_hidden()
                .cursor_pointer()
                .hover(|style| style.bg(crate::theme::ink(0.045)))
                .tooltip(move |_, cx| {
                    cx.new(|_| AppshotActionTooltip(preview_label.clone()))
                        .into()
                })
                .role(gpui::Role::Button)
                .aria_label(preview_aria)
                .tab_index(0)
                .focus_visible(|style| {
                    style
                        .bg(crate::theme::ink(0.06))
                        .border_1()
                        .border_color(theme.accent)
                })
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        preview_on_key.viewer.reset();
                        this.preview = Some(preview_on_key.clone());
                        this.preview_focus_pending = true;
                        cx.notify();
                    }
                }))
                .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                    composer_for_preview
                        .update(cx, |this, cx| {
                            preview_on_a11y.viewer.reset();
                            this.preview = Some(preview_on_a11y.clone());
                            this.preview_focus_pending = true;
                            cx.notify();
                        })
                        .ok();
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    preview.viewer.reset();
                    this.preview = Some(preview.clone());
                    this.preview_focus_pending = true;
                    cx.notify();
                }))
                .child(
                    div()
                        .id(("composer-appshot-preview", ix))
                        .w(px(tile_width))
                        .h(px(APPSHOT_PREVIEW_HEIGHT))
                        .relative()
                        .flex_none()
                        .flex()
                        .items_end()
                        .justify_center()
                        .overflow_hidden()
                        .rounded(px(12.0))
                        .child(
                            div()
                                .w(px(image_width))
                                .h(px(image_height))
                                .flex_none()
                                .overflow_hidden()
                                .rounded(px(4.0))
                                .shadow_sm()
                                .child(crate::edge_fade::edge_faded(
                                    44.0,
                                    false,
                                    true,
                                    img(appshot.screenshot.image.clone())
                                        .w(px(image_width))
                                        .h(px(image_height))
                                        .object_fit(ObjectFit::Contain),
                                )),
                        ),
                )
                .child(
                    div()
                        .mt(px(20.0))
                        .max_w(px(tile_width - 20.0))
                        .truncate()
                        .text_center()
                        .text_size(px(12.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(source),
                );
            if let Some(icon) = &appshot.app_icon {
                card = card.child(crate::frost::layered(
                    div()
                        .absolute()
                        .top(px(APPSHOT_PREVIEW_HEIGHT - 22.0))
                        .left(px((tile_width - 28.0) / 2.0))
                        .size(px(28.0))
                        .rounded(px(7.0))
                        .bg(theme.bg)
                        .border_1()
                        .border_color(theme.border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            img(icon.clone())
                                .size(px(24.0))
                                .rounded(px(5.0))
                                .object_fit(ObjectFit::Contain),
                        ),
                ));
            }
            card = card.child(crate::frost::layered(
                div()
                    .id(("composer-appshot-remove", ix))
                    .absolute()
                    .top(px(6.0))
                    .right(px(6.0))
                    .size(px(22.0))
                    .rounded_full()
                    .bg(theme.bg.opacity(0.92))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .shadow_sm()
                    .opacity(0.0)
                    .group_hover(group, |style| style.opacity(1.0))
                    .tooltip(move |_, cx| {
                        cx.new(|_| AppshotActionTooltip(remove_label.clone()))
                            .into()
                    })
                    .role(gpui::Role::Button)
                    .aria_label(remove_aria)
                    .tab_index(0)
                    .focus_visible(|style| style.opacity(1.0).border_1().border_color(theme.accent))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            this.remove_appshot(&remove_on_key_id, cx);
                        }
                    }))
                    .on_a11y_action(gpui::AccessibleAction::Click, move |_, _, cx| {
                        composer_for_remove
                            .update(cx, |this, cx| {
                                this.remove_appshot(&remove_on_a11y_id, cx);
                            })
                            .ok();
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.remove_appshot(&remove_id, cx);
                    }))
                    .child(
                        crate::icons::icon(crate::icons::CLOSE_CIRCLE)
                            .size(px(15.0))
                            .text_color(theme.text_muted),
                    ),
            ));
            // Entity-owned timestamps prevent the entrance replaying on route remount.
            if let Some(start) = self.appshot_entrances.get(&appshot.id) {
                let raw = (start.elapsed().as_secs_f32() / (0.24 * motion::speed_scale()))
                    .clamp(0.0, 1.0);
                if raw < 1.0 && !motion::reduced_motion(cx) {
                    let progress =
                        motion::MotionSpec::new(240, motion::EASE_OUT_EXPO).progress(raw);
                    card = card.opacity(progress).top(px(8.0 * (1.0 - progress)));
                    window.request_animation_frame();
                }
            }
            strip = strip.child(card);
        }
        Some(strip.into_any_element())
    }

    /// Paperclip: the native image picker (the original's hidden
    /// `<input type=file accept=image/* multiple>`).
    fn open_file_picker(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        self.picker_task = Some(cx.spawn(async move |this, cx| {
            let result = rx.await;
            this.update(cx, |composer, cx| {
                if let Ok(Ok(Some(paths))) = result {
                    composer.add_paths(paths, cx);
                }
                // Both Attach and Cancel return to the draft.
                composer.focus_pending = true;
                cx.notify();
            })
            .ok();
        }));
    }

    fn sync_mention_controls(&mut self, cx: &mut Context<Self>) {
        let open = self.mention.token.is_some() || self.slash.token.is_some();
        let has_selection = if self.slash.token.is_some() {
            self.slash.active.is_some()
        } else {
            self.mention.active.is_some()
        };
        self.input.update(cx, |input, cx| {
            input.set_mention_controls(open, has_selection, cx)
        });
    }

    /// Tear down the entire completion lifecycle. Advancing the generation is
    /// important even when the spawned task is dropped: an RPC response may
    /// already be queued for delivery on the UI executor.
    fn reset_mention(&mut self, dismissed: Option<(Range<usize>, String)>, cx: &mut Context<Self>) {
        let request = self.mention.request.wrapping_add(1);
        self.mention_task = None;
        self.mention = FileMentionState {
            context: self.mention.context.clone(),
            request,
            dismissed,
            ..FileMentionState::default()
        };
        self.sync_mention_controls(cx);
    }

    fn file_search_params(&self, query: &str, cx: &App) -> Option<serde_json::Value> {
        let selected_worktree = match self.pickers.read(cx).checkout_plan() {
            crate::pickers::CheckoutPlan::ReuseWorktree { path, .. } => Some(path),
            _ => None,
        };
        let (params, target) = {
            let state = self.state.read(cx);
            let mut params = serde_json::Map::new();
            params.insert("query".into(), query.into());
            let target = if let Some(chat) = state.selected_chat_row() {
                params.insert("chatId".into(), chat.id.clone().into());
                params.insert("cwd".into(), chat.cwd.clone().into());
                Some(chat.device_id.clone())
            } else if let Some(space) = state.selected_space_row() {
                params.insert("spaceId".into(), space.id.clone().into());
                params.insert("cwd".into(), space.path.clone().into());
                if let Some(path) = selected_worktree {
                    params.insert("path".into(), path.into());
                }
                Some(space.device_id.clone())
            } else {
                None
            };
            if let Some(target) = &target {
                params.insert("targetDeviceId".into(), target.clone().into());
            }
            (serde_json::Value::Object(params), target)
        };
        target.map(|_| params)
    }

    fn catalog_params(&self, cx: &App) -> serde_json::Value {
        let harness = self.pickers.read(cx).resolved(cx).harness;
        let selected_worktree = match self.pickers.read(cx).checkout_plan() {
            crate::pickers::CheckoutPlan::ReuseWorktree { path, .. } => Some(path),
            _ => None,
        };
        let mut params = serde_json::json!({ "harness": harness });
        {
            let state = self.state.read(cx);
            if let Some(chat) = state.selected_chat_row() {
                params["chatId"] = chat.id.clone().into();
                params["targetDeviceId"] = chat.device_id.clone().into();
                // Include the resolved cwd in the cache identity, too.
                params["cwd"] = chat.cwd.clone().into();
            } else if let Some(space) = state.selected_space_row() {
                params["spaceId"] = space.id.clone().into();
                params["targetDeviceId"] = space.device_id.clone().into();
                params["cwd"] = space.path.clone().into();
                if let Some(path) = selected_worktree {
                    params["path"] = path.into();
                }
            } else if let Some(device) = state.effective_device_id() {
                params["targetDeviceId"] = device.into();
            }
        }
        params
    }

    fn completion_connection_context(&self, cx: &App) -> String {
        let state = self.state.read(cx);
        let engine = state
            .engine()
            .map(|engine| engine.client() as *const _ as usize);
        let target = state
            .selected_chat_row()
            .map(|chat| chat.device_id.clone())
            .or_else(|| {
                state
                    .selected_space_row()
                    .map(|space| space.device_id.clone())
            })
            .or_else(|| state.effective_device_id());
        let online = target
            .as_deref()
            .is_none_or(|device| state.device_online(device, chrono::Utc::now()));
        format!(
            "{engine:?}:{:?}:{:?}:{online}",
            state.connection, state.connectivity.state
        )
    }

    fn resolve_pasted_references(
        &mut self,
        range: Range<usize>,
        revision: u64,
        cx: &mut Context<Self>,
    ) {
        let original = self.input.read(cx).text().to_owned();
        let tokens = pasted_reference_tokens(&original, range.clone());
        if tokens.is_empty() {
            return;
        }
        let params = self.catalog_params(cx);
        let file_params = self.file_search_params("", cx);
        if !params["targetDeviceId"].as_str().is_some_and(|target| {
            self.state
                .read(cx)
                .device_supports(target, capabilities::COMPOSER_REFERENCES_V1)
        }) {
            return;
        }
        let harness = self.pickers.read(cx).resolved(cx).harness;
        let preferences =
            crate::settings::current(cx).skill_completion(harness.unwrap_or(HarnessId::Codex));
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let context = format!(
            "{}:{params}:{preferences:?}",
            self.completion_connection_context(cx)
        );
        cx.spawn(async move |this, cx| {
            let mut candidates = Vec::new();
            if tokens.iter().any(|(prefix, _)| *prefix != '@') && harness.is_some() {
                let commands = async {
                    engine
                        .client()
                        .call(methods::LIST_COMMANDS, params.clone())
                        .await
                        .ok()
                        .and_then(|v| serde_json::from_value::<Vec<SlashCommand>>(v).ok())
                        .unwrap_or_default()
                };
                let skills = async {
                    engine
                        .client()
                        .call(methods::LIST_SKILLS, params.clone())
                        .await
                        .ok()
                        .and_then(|v| {
                            serde_json::from_value::<Option<Vec<zeron_proto::invocation::Skill>>>(v)
                                .ok()
                        })
                        .flatten()
                        .unwrap_or_default()
                };
                let (commands, skills) = futures::join!(commands, skills);
                candidates = invocation_candidates(commands, skills);
            }
            let mut replacements = Vec::new();
            let mut files = HashMap::new();
            for (prefix, token) in tokens {
                if this
                    .update(cx, |this, cx| this.input.read(cx).edit_revision != revision)
                    .unwrap_or(true)
                {
                    return;
                }
                let replacement = if prefix == '@' {
                    if !files.contains_key(&token.query) {
                        let Some(mut search) = file_params.clone() else {
                            continue;
                        };
                        search["query"] = token.query.clone().into();
                        let results = engine
                            .client()
                            .call(methods::SEARCH_FILES, search)
                            .await
                            .ok()
                            .and_then(|v| serde_json::from_value::<Vec<FileSearchMatch>>(v).ok())
                            .unwrap_or_default();
                        let exact: Vec<_> = results
                            .into_iter()
                            .filter(|file| {
                                file.path.trim_end_matches('/') == token.query.trim_end_matches('/')
                                    && local_path_is_safe(&file.path)
                            })
                            .collect();
                        files.insert(
                            token.query.clone(),
                            (exact.len() == 1)
                                .then(|| local_file_link(&exact[0].path, exact[0].is_dir)),
                        );
                    }
                    files.get(&token.query).cloned().flatten()
                } else if prefix == '$' && !preferences.dollar {
                    None
                } else {
                    let exact: Vec<_> = candidates
                        .iter()
                        .filter(|candidate| {
                            candidate.invocation.prefix() == prefix && candidate.name == token.query
                        })
                        .collect();
                    (exact.len() == 1).then(|| exact[0].invocation.link())
                };
                if let Some(replacement) = replacement {
                    replacements.push((token.range, replacement));
                }
            }
            if replacements.is_empty() {
                return;
            }
            this.update(cx, |this, cx| {
                let harness = this.pickers.read(cx).resolved(cx).harness;
                let preferences = crate::settings::current(cx)
                    .skill_completion(harness.unwrap_or(HarnessId::Codex));
                let current = format!(
                    "{}:{}:{preferences:?}",
                    this.completion_connection_context(cx),
                    this.catalog_params(cx)
                );
                if current != context {
                    return;
                }
                this.input.update(cx, |input, cx| {
                    input.apply_pasted_references(&original, range.end, revision, replacements, cx);
                });
            })
            .ok();
        })
        .detach();
    }

    fn on_input_edited(&mut self, cx: &mut Context<Self>) {
        if self.wizard.is_some() {
            if self.mention.token.is_some() || self.mention_task.is_some() {
                self.reset_mention(None, cx);
            }
            if self.slash.token.is_some() || self.slash_task.is_some() {
                self.reset_slash(None, cx);
            }
            return;
        }
        let input = self.input.read(cx);
        if !input.selected_range.is_empty() || input.marked_range.is_some() {
            if self.mention.token.is_some() {
                self.reset_mention(None, cx);
            }
            if self.slash.token.is_some() {
                self.reset_slash(None, cx);
            }
            return;
        }
        let (text, cursor) = (input.text().to_string(), input.cursor_offset());
        self.update_slash(&text, cursor, cx);
        let token = mention_token(&text, cursor);
        let context = self
            .file_search_params("", cx)
            .map(|params| format!("{}:{params}", self.completion_connection_context(cx)))
            .unwrap_or_default();
        if self.mention.context != context {
            self.reset_mention(None, cx);
            self.mention.context = context;
        }
        let still_dismissed = token.as_ref().is_some_and(|token| {
            self.mention
                .dismissed
                .as_ref()
                .is_some_and(|(range, value)| {
                    token.range == *range && text.get(range.clone()) == Some(value.as_str())
                })
        });
        if still_dismissed {
            self.mention.token = None;
            self.mention_task = None;
            self.sync_mention_controls(cx);
            cx.notify();
            return;
        }
        self.mention.dismissed = None;
        if token == self.mention.token {
            self.sync_mention_controls(cx);
            cx.notify();
            return;
        }
        self.mention.request = self.mention.request.wrapping_add(1);
        self.mention_task = None;
        // Refining an open menu keeps the stale rows visible until the new
        // response lands — clearing here made the popup bounce through the
        // skeleton (and a different height) on every keystroke.
        let refining = self.mention.token.is_some() && token.is_some();
        self.mention.token = token.clone();
        if !refining {
            self.mention.results.clear();
            self.mention.active = None;
            // Fresh open: the row stack restarts at the top.
            crate::popover::reset_menu_scroll(&self.mention_scroll, &mut self.popup_bar);
        }
        self.mention.error = None;
        self.mention.loading = token.is_some();
        self.sync_mention_controls(cx);
        let Some(token) = token else {
            cx.notify();
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.mention.loading = false;
            cx.notify();
            return;
        };
        let Some(params) = self.file_search_params(&token.query, cx) else {
            self.mention.loading = false;
            cx.notify();
            return;
        };
        let request = self.mention.request;
        self.mention_task = Some(cx.spawn(async move |this, cx| {
            // A short debounce prevents one full workspace walk per keystroke
            // during normal typing. The generation check below still guards
            // requests that were already in flight when the query changed.
            cx.background_executor()
                .timer(Duration::from_millis(80))
                .await;
            let mut result = engine
                .client()
                .call(methods::SEARCH_FILES, params.clone())
                .await;
            if matches!(result, Err(RpcError::Transport(_)) | Err(RpcError::Closed)) {
                // One retry rides out a cold relay dial to the host device
                // (the diffs pane retries forever; a keystroke-scoped search
                // gets a single second chance).
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                result = engine.client().call(methods::SEARCH_FILES, params).await;
            }
            this.update(cx, |composer, cx| {
                if !mention_response_is_current(&composer.mention, request) {
                    return;
                }
                composer.mention.loading = false;
                match result {
                    Ok(value) => match serde_json::from_value::<Vec<FileSearchMatch>>(value) {
                        Ok(mut results) => {
                            // Search results can include legal filesystem names
                            // that the canonical reference format cannot encode.
                            results.retain(|result| local_path_is_safe(&result.path));
                            composer.mention.error = None;
                            composer.mention.active = (!results.is_empty()).then_some(0);
                            composer.mention.results = results;
                            // New result set: the row stack restarts at the top.
                            crate::popover::reset_menu_scroll(
                                &composer.mention_scroll,
                                &mut composer.popup_bar,
                            );
                        }
                        Err(err) => tracing::warn!(%err, "file mention response decode failed"),
                    },
                    Err(err) => {
                        tracing::warn!(%err, "file mention search failed");
                        composer.mention.results.clear();
                        composer.mention.active = None;
                        composer.mention.error = Some(mention_error_message(&err));
                    }
                }
                composer.sync_mention_controls(cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn move_mention(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.mention.active =
            crate::popover::menu_step(self.mention.active, self.mention.results.len(), delta);
        if let Some(active) = self.mention.active {
            // Keep the keyboard cursor visible in the scrolled row stack.
            self.mention_scroll.scroll_to_item(active);
        }
        self.sync_mention_controls(cx);
        cx.notify();
    }

    fn dismiss_mention(&mut self, cx: &mut Context<Self>) {
        let dismissed = self.mention.token.as_ref().and_then(|token| {
            self.input
                .read(cx)
                .text()
                .get(token.range.clone())
                .map(|text| (token.range.clone(), text.to_string()))
        });
        self.reset_mention(dismissed, cx);
        cx.notify();
    }

    fn accept_mention(&mut self, cx: &mut Context<Self>) {
        let Some(token) = self.mention.token.clone() else {
            return;
        };
        let Some((path, is_dir)) = self
            .mention
            .active
            .and_then(|active| self.mention.results.get(active))
            .map(|result| (result.path.clone(), result.is_dir))
        else {
            return;
        };
        self.input.update(cx, |input, cx| {
            input.replace_mention(token.range, &path, is_dir, cx)
        });
        self.reset_mention(None, cx);
        cx.notify();
    }

    fn render_file_mention_popup(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = &theme.for_popup();
        let token = self.mention.token.as_ref()?;
        let mut card = crate::popover::completion_card(theme)
            // Completion choices belong to the input. Keep it focused until
            // mouse-up can accept a choice (or while dragging the scrollbar).
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.input.focus_handle(cx), cx);
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss_mention(cx)));
        if self.mention.loading && self.mention.results.is_empty() {
            card = card.child(crate::popover::skeleton_rows(
                "file-mention-loading",
                theme,
                3,
                cx.entity_id(),
                cx,
            ));
        } else if let Some(error) = self.mention.error.clone() {
            card = card.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.danger_muted)
                    .child(error),
            );
        } else if self.mention.results.is_empty() {
            card = card.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(if token.query.is_empty() {
                        "No files available"
                    } else {
                        "No matching files"
                    }),
            );
        } else {
            let mut rows: Vec<gpui::AnyElement> = Vec::with_capacity(self.mention.results.len());
            for (ix, result) in self.mention.results.iter().enumerate() {
                let selected = self.mention.active == Some(ix);
                let (directory, name) = match result.path.rsplit_once('/') {
                    Some((directory, name)) => (directory.to_string(), name.to_string()),
                    None => (String::new(), result.path.clone()),
                };
                rows.push(
                    crate::popover::menu_row(theme, selected, format!("file-mention-result-{ix}"))
                        .id(("file-mention-result", ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.mention.active = Some(ix);
                            this.accept_mention(cx);
                        }))
                        .child(crate::popover::completion_row_content(
                            theme,
                            crate::file_icons::icon(
                                if result.is_dir {
                                    crate::file_icons::FileIconIdentity::directory(
                                        &result.path,
                                        false,
                                    )
                                } else {
                                    crate::file_icons::FileIconIdentity::file(&result.path)
                                },
                                theme.appearance,
                            )
                            .size(px(16.0))
                            .into_any_element(),
                            name.into(),
                            directory.into(),
                        ))
                        .into_any_element(),
                );
            }
            // Overflowing rows wheel-scroll inside a bounded viewport; the
            // floating rail mirrors the model-list scrollbar treatment.
            card = card.child(
                crate::popover::menu_scroll_host("mention-scroll-host")
                    .on_hover(cx.listener(Self::on_popup_list_hover))
                    .child(crate::popover::completion_list(
                        "mention-list",
                        &self.mention_scroll,
                        rows,
                    ))
                    .children(crate::popover::rail(self, "mention-scrollbar", theme, cx)),
            );
        }
        Some(crate::popover::full_width_menu_above(
            "file-mention-popup",
            card.into_any_element(),
            None,
        ))
    }

    fn render_input_with_completion(&self) -> gpui::Div {
        div().relative().child(self.input.clone())
    }

    // ---- slash commands ---------------------------------------------------

    /// Track the `/` token on every edit: open/refresh the popup, fetch the
    /// harness's command list on each open, filter locally per keystroke.
    fn update_slash(&mut self, text: &str, cursor: usize, cx: &mut Context<Self>) {
        let harness = self.pickers.read(cx).resolved(cx).harness;
        let preferences =
            crate::settings::current(cx).skill_completion(harness.unwrap_or(HarnessId::Codex));
        let (token, skill, include_skills, commands_allowed) =
            completion_trigger(text, cursor, preferences);
        if token.is_none() {
            // Leaving a token must not replace the catalog identity with the
            // idle (commands_allowed=false) context and evict the warm cache.
            self.reset_slash(None, cx);
            return;
        }
        let params = self.catalog_params(cx);
        let catalog_context = format!(
            "{preferences:?}:{}:{params}",
            self.completion_connection_context(cx),
        );
        let context = format!(
            "{}:{include_skills}:{commands_allowed}:{catalog_context}",
            if skill { "skill" } else { "command" },
        );
        let context_changed = self.slash.context != context;
        if !context_changed
            && token.as_ref().is_some_and(|token| {
                self.slash.dismissed.as_ref().is_some_and(|(range, value)| {
                    token.range == *range && text.get(range.clone()) == Some(value.as_str())
                })
            })
        {
            return;
        }
        if !context_changed && token == self.slash.token {
            return;
        }
        let refresh_catalog = context_changed || self.slash.token.is_none();
        self.slash.dismissed = None;
        if context_changed {
            self.slash.request = self.slash.request.wrapping_add(1);
            self.slash_task = None;
            self.slash.loading = false;
            if self.slash.catalog_context != catalog_context {
                self.slash_cache.clear();
            } else if self.slash.error.is_some() || !self.slash.supported {
                self.slash_cache.remove(&self.slash.context);
            }
        }
        self.slash.catalog_context = catalog_context;
        self.slash.context = context.clone();
        self.slash.harness = harness;
        self.slash.skill = skill;
        if context_changed {
            self.slash.supported = true;
        }
        self.slash.token = token;
        if context_changed {
            self.slash.error = None;
        }
        if harness.is_none() && !skill && commands_allowed {
            self.slash_cache.insert(
                context.clone(),
                with_workspace_commands(vec![], self.state.read(cx).selected_chat.is_some()),
            );
        }
        if harness.is_none()
            || (self.slash_cache.contains_key(&context) && !refresh_catalog)
            || self.slash.loading
        {
            self.refilter_slash(cx);
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if !skill && commands_allowed {
                self.slash_cache.insert(
                    context,
                    with_workspace_commands(vec![], self.state.read(cx).selected_chat.is_some()),
                );
                self.slash.error = Some("Agent command discovery requires a connection".into());
                self.refilter_slash(cx);
            }
            return;
        };
        self.slash.request = self.slash.request.wrapping_add(1);
        let request = self.slash.request;
        self.slash.loading = true;
        self.slash.error = None;
        self.refilter_slash(cx);
        self.slash_task = Some(cx.spawn(async move |this, cx| {
            let result = async {
                let commands = async {
                    if skill || !commands_allowed {
                        return Ok(Vec::new());
                    }
                    let value = engine
                        .client()
                        .call(methods::LIST_COMMANDS, params.clone())
                        .await?;
                    serde_json::from_value::<Vec<SlashCommand>>(value)
                        .map_err(|e| RpcError::Failed(e.to_string()))
                };
                let skills = async {
                    // Even separated slash menus need this metadata to exclude
                    // provider commands that are actually skill aliases.
                    let value = engine
                        .client()
                        .call(methods::LIST_SKILLS, params.clone())
                        .await?;
                    serde_json::from_value::<Option<Vec<zeron_proto::invocation::Skill>>>(value)
                        .map_err(|e| RpcError::Failed(e.to_string()))
                };
                let (commands, skills) = futures::join!(commands, skills);
                merge_invocation_results(commands, skills, skill).map(
                    |(mut rows, supported, warning)| {
                        if !include_skills {
                            rows.retain(|row| row.invocation.prefix() == '/');
                        }
                        (rows, supported, warning)
                    },
                )
            }
            .await;
            this.update(cx, |composer, cx| {
                if composer.slash.request != request || composer.slash.context != context {
                    return;
                }
                composer.slash.loading = false;
                let decoded = result.map(|(candidates, supported, warning)| {
                    composer.slash.supported = supported;
                    composer.slash.error = warning;
                    candidates
                });
                match decoded {
                    Ok(candidates) => {
                        let candidates = if !skill && commands_allowed {
                            with_workspace_commands(
                                candidates,
                                composer.state.read(cx).selected_chat.is_some(),
                            )
                        } else {
                            candidates
                        };
                        composer.slash_cache.insert(context, candidates);
                    }
                    Err(err) => {
                        composer.slash_cache.remove(&context);
                        composer.slash.error = Some(slash_error_message(&err, skill));
                        if !skill && commands_allowed {
                            composer.slash_cache.insert(
                                context,
                                with_workspace_commands(
                                    vec![],
                                    composer.state.read(cx).selected_chat.is_some(),
                                ),
                            );
                        }
                    }
                }
                composer.refilter_slash(cx);
            })
            .ok();
        }));
        cx.notify();
    }

    /// Re-rank the cached list for the current query (pure local filter).
    fn refilter_slash(&mut self, cx: &mut Context<Self>) {
        let query = self
            .slash
            .token
            .as_ref()
            .map(|t| t.query.clone())
            .unwrap_or_default();
        let commands = self
            .slash_cache
            .get(&self.slash.context)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        self.slash.filtered = crate::popover::filter_indices(&query, &names);
        self.slash.active = (!self.slash.filtered.is_empty()).then_some(0);
        // A fresh query/reopen restarts the row stack at the top.
        crate::popover::reset_menu_scroll(&self.slash_scroll, &mut self.popup_bar);
        self.sync_mention_controls(cx);
        cx.notify();
    }

    fn move_slash(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.slash.active =
            crate::popover::menu_step(self.slash.active, self.slash.filtered.len(), delta);
        if let Some(active) = self.slash.active {
            // Keep the keyboard cursor visible in the scrolled row stack.
            self.slash_scroll.scroll_to_item(active);
        }
        self.sync_mention_controls(cx);
        cx.notify();
    }

    fn dismiss_slash(&mut self, cx: &mut Context<Self>) {
        let dismissed = self.slash.token.as_ref().and_then(|token| {
            self.input
                .read(cx)
                .text()
                .get(token.range.clone())
                .map(|text| (token.range.clone(), text.to_string()))
        });
        self.reset_slash(dismissed, cx);
        cx.notify();
    }

    fn accept_slash(&mut self, cx: &mut Context<Self>) {
        let Some(token) = self.slash.token.clone() else {
            return;
        };
        let Some(command) = self
            .slash
            .active
            .and_then(|active| self.slash.filtered.get(active))
            .and_then(|&ix| {
                self.slash_cache
                    .get(&self.slash.context)
                    .and_then(|c| c.get(ix))
            })
            .cloned()
        else {
            return;
        };
        if let Some(action) = command.workspace_command {
            self.execute_workspace_command(action, token.range, cx);
            return;
        }
        let insertion =
            invocation_insertion(&command.invocation, self.reference_delivery_supported(cx));
        self.input.update(cx, |input, cx| {
            input.replace_plain_token(token.range, &insertion, cx)
        });
        self.reset_slash(None, cx);
        cx.notify();
    }

    /// Tear down the slash completion (mirrors [`Self::reset_mention`]).
    fn reset_slash(&mut self, dismissed: Option<(Range<usize>, String)>, cx: &mut Context<Self>) {
        // Partial discovery is useful now, but reopening must retry the failed provider call.
        if self.slash.error.is_some() {
            self.slash_cache.remove(&self.slash.context);
        }
        let request = self.slash.request.wrapping_add(1);
        self.slash_task = None;
        self.slash = SlashState {
            request,
            dismissed,
            harness: self.slash.harness,
            catalog_context: self.slash.catalog_context.clone(),
            context: self.slash.context.clone(),
            supported: self.slash.supported,
            ..SlashState::default()
        };
        self.sync_mention_controls(cx);
    }

    fn render_slash_popup(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = &theme.for_popup();
        // Only while a slash token is active.
        self.slash.token.as_ref()?;
        let commands = self
            .slash_cache
            .get(&self.slash.context)
            .map(Vec::as_slice)
            .unwrap_or_default();
        // Full pill width at the mention card's height budget — both composer
        // completions share the same surface shape.
        let mut card = crate::popover::completion_card(theme)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.input.focus_handle(cx), cx);
                }),
            )
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss_slash(cx)));
        if let Some(error) = self.slash.error.clone() {
            card = card.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.danger_muted)
                    .child(error),
            );
        }
        if self.slash.loading && commands.is_empty() {
            card = card.child(crate::popover::skeleton_rows(
                "slash-loading",
                theme,
                3,
                cx.entity_id(),
                cx,
            ));
        } else if self.slash.filtered.is_empty() && self.slash.error.is_none() {
            card = card.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(if commands.is_empty() {
                        if self.slash.skill {
                            if self.slash.supported {
                                "No skills available for this project"
                            } else {
                                "This agent does not advertise skills"
                            }
                        } else if !crate::settings::current(cx)
                            .skill_completion(self.slash.harness.unwrap_or(HarnessId::Codex))
                            .separate_from_slash
                        {
                            "No commands or skills available"
                        } else {
                            "No slash commands available in this integration"
                        }
                    } else {
                        if self.slash.skill {
                            "No matching skills"
                        } else if !crate::settings::current(cx)
                            .skill_completion(self.slash.harness.unwrap_or(HarnessId::Codex))
                            .separate_from_slash
                        {
                            "No matching commands or skills"
                        } else {
                            "No matching commands"
                        }
                    }),
            );
        } else {
            let mut rows: Vec<gpui::AnyElement> = Vec::with_capacity(self.slash.filtered.len());
            for (row_ix, &cmd_ix) in self.slash.filtered.iter().enumerate() {
                let Some(command) = commands.get(cmd_ix) else {
                    continue;
                };
                let selected = self.slash.active == Some(row_ix);
                let name: SharedString = if command.invocation.prefix() == '$' {
                    skill_display_name(&command.name)
                } else {
                    format!("/{}", command.name)
                }
                .into();
                let mut description = command.description.clone();
                if let Some(hint) = &command.input_hint {
                    if description.is_empty() {
                        description = format!("<{hint}>");
                    } else {
                        description = format!("{description} · <{hint}>");
                    }
                }
                let description: SharedString = description.into();
                rows.push(
                    crate::popover::menu_row(theme, selected, format!("slash-result-{row_ix}"))
                        .id(("slash-result", row_ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.slash.active = Some(row_ix);
                            this.accept_slash(cx);
                        }))
                        .child(crate::popover::completion_row_content(
                            theme,
                            crate::icons::icon(if command.invocation.prefix() == '$' {
                                crate::icons::MAGIC_STICK_3
                            } else {
                                crate::icons::COMMAND
                            })
                            .size(px(16.0))
                            .text_color(theme.text_muted)
                            .into_any_element(),
                            name,
                            description,
                        ))
                        .into_any_element(),
                );
            }
            // Overflowing rows wheel-scroll inside a bounded viewport; the
            // floating rail mirrors the model-list scrollbar treatment.
            card = card.child(
                crate::popover::menu_scroll_host("slash-scroll-host")
                    .on_hover(cx.listener(Self::on_popup_list_hover))
                    .child(crate::popover::completion_list(
                        "slash-list",
                        &self.slash_scroll,
                        rows,
                    ))
                    .children(crate::popover::rail(self, "slash-scrollbar", theme, cx)),
            );
        }
        // Full pill width above the composer, matching the file-mention popup.
        Some(crate::popover::full_width_menu_above(
            "slash-popup",
            card.into_any_element(),
            None,
        ))
    }

    /// The popup whose rows a scrollbar drag is moving — the tokens are
    /// mutually exclusive, so at most one exists.
    fn active_popup_scroll(&self) -> Option<gpui::ScrollHandle> {
        if self.slash.token.is_some() {
            Some(self.slash_scroll.clone())
        } else if self.mention.token.is_some() {
            Some(self.mention_scroll.clone())
        } else {
            None
        }
    }

    fn on_popup_list_hover(
        &mut self,
        hovered: &bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.popup_bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        {
            let state = self.state.read(cx);
            let now = chrono::Utc::now();
            retain_live_interrupts(&mut self.interrupting, |chat_id| {
                matches!(
                    state.indicator_for(chat_id, now),
                    Indicator::Working | Indicator::AwaitingInput
                )
            });
            self.queue_removing
                .retain(|id| state.queue.iter().any(|item| item.id == *id));
        }
        self.interrupt_tasks
            .retain(|chat_id, _| self.interrupting.contains(chat_id));

        let editing_id = self.editing_queued.clone();
        let (key, pending, edited_row_exists) = {
            let s = self.state.read(cx);
            (
                s.selected_chat.clone().unwrap_or_default(),
                pending_input_request(&s.transcript),
                editing_id
                    .as_ref()
                    .is_none_or(|id| s.queue.iter().any(|item| item.id == *id)),
            )
        };

        // A queue edit belongs to exactly one visible row. Navigation or a
        // remote drain/removal cancels it instead of leaving a focused but
        // unmounted editor entity behind.
        if key != self.current_key && self.editing_queued.is_some() {
            self.clear_queue_edit(cx);
        } else if !edited_row_exists && self.editing_queued.is_some() && !self.queue_edit_finishing
        {
            self.failure =
                Some("The queued message was removed; your edit remains in the composer".into());
            // Recover both drafts when another device removes the reserved row.
            if let Some((draft, mut attachments, mut appshots)) = self.queue_edit_draft.take() {
                appshots.extend(self.appshots.remove(&self.current_key).unwrap_or_default());
                self.appshots.insert(self.current_key.clone(), appshots);
                let edited = self.input.read(cx).text().to_string();
                let text = [draft, edited]
                    .into_iter()
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                self.input.update(cx, |input, cx| input.set_text(text, cx));
                attachments.extend(
                    self.attachments
                        .remove(&self.current_key)
                        .unwrap_or_default(),
                );
                self.attachments
                    .insert(self.current_key.clone(), attachments);
            }
            self.clear_queue_edit(cx);
        }

        // Draft swap on chat navigation — the input entity itself survives.
        if key != self.current_key {
            if !key.is_empty() && !self.current_key.is_empty() {
                // Switching between established chats still snaps to the new
                // draft; a tail from a hero transition belongs to its old chat.
                self.dock_reflow = Default::default();
            }
            let new_thread_launch =
                self.launching_new_chat && self.current_key.is_empty() && !key.is_empty();
            let returning_to_new_thread = !self.current_key.is_empty() && key.is_empty();
            self.launching_new_chat = false;
            let old_text = self.input.read(cx).text().to_string();
            if old_text.is_empty() {
                self.drafts.remove(&self.current_key);
            } else {
                self.drafts.insert(self.current_key.clone(), old_text);
            }
            let draft = self.drafts.get(&key).cloned().unwrap_or_default();
            self.current_key = key;
            // `failure` deliberately survives navigation: chat-scoped
            // failures render only under their own chat (see `failure_key`),
            // so switching away and back must not erase the one visible
            // trace of a failed send.
            self.wizard = None;
            // Attachments stay stashed under their chat key (the map swap IS
            // the navigation); only the transient chrome resets.
            self.preview = None;
            self.reset_mention(None, cx);
            self.reset_slash(None, cx);
            // Route changes snap (round 5/6): a mode difference between the
            // old and new session's composer must not glide across
            // navigation. Killing the in-flight morph here isn't enough —
            // the nav-driven flip only commits AFTER the swapped draft has
            // been re-measured, one or two renders later, so the whole
            // window snaps (see ROUTE_SNAP_MS).
            self.height_morph = None;
            self.last_target_height = 0.0;
            if (new_thread_launch || returning_to_new_thread)
                && !motion::reduced_motion(cx)
                && self.last_rendered_height > 0.0
            {
                // Both directions share one timeline. The blank canvas is
                // always expanded; an established session begins compact.
                self.expanded_mode = returning_to_new_thread;
                let now_ms =
                    self.morph_clock.elapsed().as_secs_f32() * 1000.0 / motion::speed_scale();
                self.flip_morph = Some(FlipMorph::new_thread_transition(
                    self.last_rendered_height,
                    now_ms,
                ));
                self.route_snap_until = None;
            } else {
                self.flip_morph = None;
                self.last_rendered_height = 0.0;
                self.route_snap_until = Some(Instant::now() + Duration::from_millis(ROUTE_SNAP_MS));
            }
            self.input.update(cx, |input, cx| input.set_text(draft, cx));
        }

        // A pending agent question must not take over an active queue edit.
        if self.editing_queued.is_some() {
            self.on_input_edited(cx);
            cx.notify();
            return;
        }
        // Question panel lifecycle (wizard state cached per request id).
        match pending {
            Some((request_id, questions)) if !self.answered_requests.contains(&request_id) => {
                let same = self
                    .wizard
                    .as_ref()
                    .is_some_and(|w| w.request_id == request_id);
                if !same {
                    self.reset_mention(None, cx);
                    self.wizard = Some(Wizard::new(request_id, questions));
                    self.advance_task = None;
                    // The shared input becomes the panel's free-text override.
                    self.input.update(cx, |input, cx| {
                        input.set_placeholder("Type your own answer, or pick an option above", cx)
                    });
                }
            }
            _ => {
                if let Some(wizard) = self.wizard.as_ref() {
                    // LATCH (original composer.tsx `inputLatch`): a transient
                    // fold/sync blip — or a steer appended behind the
                    // streaming entry — must not unmount the panel and lose
                    // the user's picks. Release only on explicit resolution
                    // (here or on another device) or when a NON-EMPTY
                    // transcript shows the question superseded (a newer
                    // assistant entry took over). Never on run death: the
                    // question stays answerable until answered — the engine
                    // delivers a dead run's answer as a resumed turn.
                    let transcript = self.state.read(cx).transcript.clone();
                    let released = input_request_resolved(&transcript, &wizard.request_id)
                        || (!transcript.is_empty()
                            && !self.answered_requests.contains(&wizard.request_id));
                    if released {
                        self.wizard = None;
                        self.advance_task = None;
                        self.input
                            .update(cx, |input, cx| input.set_placeholder("Do anything…", cx));
                    }
                }
            }
        }
        let input_context = message_input_context(self.wizard.is_some());
        self.input
            .update(cx, |input, cx| input.set_key_context(input_context, cx));
        self.on_input_edited(cx);
        cx.notify();
    }

    pub(crate) fn run_live(&self, cx: &App) -> bool {
        let s = self.state.read(cx);
        let Some(chat_id) = s.selected_chat.as_deref() else {
            return false;
        };
        matches!(
            s.indicator_for(chat_id, chrono::Utc::now()),
            Indicator::Working | Indicator::AwaitingInput
        )
    }

    fn reference_delivery_supported(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        let target = state
            .selected_chat_row()
            .map(|chat| chat.device_id.clone())
            .or_else(|| state.effective_device_id());
        target.is_some_and(|device| {
            state.device_supports(&device, capabilities::COMPOSER_REFERENCES_V1)
        })
    }

    /// Check before consuming drafts, attachments, or an edited queue row.
    pub(crate) fn check_reference_delivery(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        if references_require_update(text, self.reference_delivery_supported(cx)) {
            self.failure = Some("Update the selected device’s Zeron to send file, command, or skill references. Your draft is preserved.".into());
            self.failure_key = Some(self.current_key.clone());
            cx.notify();
            return false;
        }
        true
    }

    /// New chats need a runnable agent, but may target the device's home
    /// directory without a project. Existing chats carry their own run config.
    fn send_blocked(&self, cx: &App) -> bool {
        if self.queue_edit_finishing {
            return true;
        }
        let state = self.state.read(cx);
        if state.review_comment_flush_pending(&self.current_key) {
            return true;
        }
        if state.selected_chat.is_some() {
            return false;
        }
        // New-chat canvas: needs a runnable agent. The
        // no-agents check only fires once the catalog is loaded — offline
        // and still-loading states must not block (the harness resolves from
        // the remembered default and the engine reports real failures).
        self.pickers.read(cx).no_agents_available()
    }

    fn button_mode(&self, cx: &App) -> SendButtonMode {
        if self.editing_queued.is_some() {
            return SendButtonMode::Send;
        }
        let has_text = composer_has_content(
            self.input.read(cx).text(),
            self.staged().len() + self.staged_appshots().len(),
            self.staged_comments(cx).len(),
        );
        send_button_mode(self.run_live(cx), has_text)
    }

    fn execute_workspace_command(
        &mut self,
        command: WorkspaceCommand,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) {
        // Actions consume only their trigger. Draft text, attachments and queued
        // edits remain in the composer; selecting a command never sends them.
        self.input
            .update(cx, |input, cx| input.remove_completion_token(range, cx));
        self.reset_slash(None, cx);
        self.failure = None;
        self.failure_key = None;
        cx.emit(ComposerEvent::WorkspaceCommand(command));
        cx.notify();
    }

    fn on_submit(&mut self, cx: &mut Context<Self>) {
        if self.commit_queue_edit(cx) {
            return;
        }
        if self.wizard.is_some() {
            // Enter inside the panel's free-text input submits the page.
            let typed = self.input.read(cx).text().trim().to_string();
            if let Some(w) = self.wizard.as_mut() {
                w.set_typed(typed);
            }
            self.wizard_advance(cx);
            return;
        }
        // Leading indentation distinguishes literal Markdown from native commands
        // and skill invocations. Only the empty-content check may trim the draft.
        let text = self.input.read(cx).text().to_string();
        if let Some(action) = self
            .slash_cache
            .get(&self.slash.context)
            .and_then(|rows| workspace_command_for_text(self.input.read(cx).text(), rows))
        {
            self.execute_workspace_command(action, 0..self.input.read(cx).text().len(), cx);
            return;
        }
        let no_content = !composer_has_content(
            &text,
            self.staged().len() + self.staged_appshots().len(),
            self.staged_comments(cx).len(),
        );
        match self.button_mode(cx) {
            // Enter never stops a run: Stop mode implies an empty composer,
            // so a stray extra Enter right after sending landed an interrupt
            // on the just-dispatched prompt and the agent ate it silently
            // (issue #406). Stop stays on the button — and on Esc when
            // escape_stops_active_agent is enabled.
            SendButtonMode::Stop => {}
            _ if no_content => {}
            _ if self.send_blocked(cx) => {}
            SendButtonMode::Send => self.send(text, false, cx),
            // Busy: keep the message queued until the current turn ends.
            SendButtonMode::Queue => self.send(text, true, cx),
        }
    }

    /// Cmd/Ctrl+Enter remains an ordinary submit while the composer carries
    /// content. With a truly empty composer it instead activates the most
    /// recently queued row, and never turns an empty chord into Stop.
    fn on_modified_submit(&mut self, cx: &mut Context<Self>) {
        if self.commit_queue_edit(cx) {
            return;
        }
        let has_content = composer_has_content(
            self.input.read(cx).text(),
            self.staged().len() + self.staged_appshots().len(),
            self.staged_comments(cx).len(),
        );
        match modified_submit_target(has_content) {
            ModifiedSubmitTarget::SubmitContent => self.on_submit(cx),
            ModifiedSubmitTarget::ActivateLatestQueued => self.activate_latest_queued(cx),
        }
    }

    /// Queue a Run doc command with an optimistic echo — or, with the agent
    /// busy, park the message on the chat's pending queue instead. New chats
    /// thread the picked config in: worktree creation (when the isolated toggle
    /// is on), `Mutate createChat` with the `ChatConfig` + cwd, and the model /
    /// reasoning / options on the Run request itself (§1.7).
    fn send(&mut self, text: String, queue: bool, cx: &mut Context<Self>) {
        if !self.check_reference_delivery(&text, cx) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.failure = Some("Engine not connected".into());
            self.failure_key = None; // global — meaningful on every chat
            cx.notify();
            return;
        };
        // Chat id: existing selection, or client-minted for the new-chat canvas
        // (the chat then appears from the doc host once the doc materializes).
        let (chat_id, is_new) = match self.state.read(cx).selected_chat.clone() {
            Some(id) => (id, false),
            None => (uuid::Uuid::new_v4().to_string(), true),
        };
        // Where the new session runs (Current checkout / reuse an existing
        // worktree / fresh worktree off the picked base) — resolved NOW so
        // the async block needs no picker access.
        let plan = self.pickers.read(cx).checkout_plan();
        // Fully-resolved model/reasoning/options — concrete values (chat config
        // or defaults), so the engine never has to guess a "default".
        let resolved = self.pickers.read(cx).resolved(cx);
        let existing_cwd = self
            .state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.cwd.clone());
        // The PROJECT fixes the new chat's device + base folder — sessions are
        // minted onto the project's device, not necessarily this one. With no
        // project ("Don't work in a project") the composer's device pick is
        // the host and the session runs from `~` there.
        let space = self.state.read(cx).selected_space_row().cloned();
        let local_device_id = self.state.read(cx).local_device_id.clone();
        let target_device_id = self.state.read(cx).effective_device_id();
        let device_id = if is_new {
            target_device_id
                .clone()
                .unwrap_or_else(|| "local".to_string())
        } else {
            self.state
                .read(cx)
                .selected_chat_row()
                .map(|c| c.device_id.clone())
                .or_else(|| local_device_id.clone())
                .unwrap_or_else(|| "local".to_string())
        };
        // Uploads/read-backs target the chat's HOST device (forwardable RPCs);
        // for a new chat that's the target device (None when it's local).
        let host_device_id = if is_new {
            target_device_id
                .clone()
                .filter(|id| local_device_id.as_deref() != Some(id.as_str()))
        } else {
            self.state
                .read(cx)
                .selected_chat_row()
                .map(|c| c.device_id.clone())
        };
        let space_id = space.as_ref().map(|s| s.id.clone());
        let space_path = space.as_ref().map(|s| s.path.clone());
        let create_side_chat = self.state.read(cx).unsaved_side_chat_create(&chat_id);
        if queue && !is_new {
            let capability = if self.staged().is_empty() && self.staged_appshots().is_empty() {
                capabilities::MESSAGE_QUEUE_V1
            } else {
                capabilities::MESSAGE_QUEUE_ATTACHMENTS_V1
            };
            if !engine.engine_info().supports(capability)
                || !self.state.read(cx).chat_host_supports(&chat_id, capability)
            {
                self.failure =
                    Some("Update the chat's engine to queue messages during a response.".into());
                cx.notify();
                return;
            }
        }
        // Snapshot-and-clear NOW (use-attachments.ts takeAttachments): the
        // strip empties the instant you hit send; a failure hands the files
        // back into the chat's stash.
        let ordinary_staged = self
            .attachments
            .remove(&self.current_key)
            .unwrap_or_default();
        let staged_appshots = self.appshots.remove(&self.current_key).unwrap_or_default();
        let mut staged = ordinary_staged.clone();
        staged.extend(
            staged_appshots
                .iter()
                .map(|appshot| appshot.screenshot.clone()),
        );
        // `typed` keeps the user's own words for the failure hand-back below:
        // restoring the folded prompt would paste the comment block into the
        // input as literal text.
        let key = self.current_key.clone();
        let comments = self.state.update(cx, |state, cx| {
            let taken = state.take_review_comments(&key);
            if !taken.is_empty() {
                cx.notify();
            }
            taken
        });
        let typed = text.clone();
        let text = crate::comments::with_comments(&text, &comments);
        self.preview = None;
        let message_id = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().timestamp_millis();
        // Existing busy chats always queue; compatibility was checked before
        // taking the draft, attachments, or review comments.
        let queue = queue && !is_new;
        let clean_queue_attachment_text = staged.is_empty()
            || (engine
                .engine_info()
                .supports(capabilities::MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1)
                && self.state.read(cx).chat_host_supports(
                    &chat_id,
                    capabilities::MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1,
                ));

        // Queued-attachment flow (durable-by-design): stage the bytes on the
        // LOCAL engine, queue the command immediately with `pending://` refs,
        // and let the engine push the bytes to a remote host afterwards —
        // staging must never gate the queue (2026-08-19 incident: a send
        // died with a zombie peer link because the upload sat in front of
        // QueueCommand). Requires every engine involved to understand the
        // ref scheme — the local engine (an IPC daemon may be older than
        // this UI) and, for remotely-hosted chats, the host; anything older
        // keeps the legacy blocking upload.
        let host_is_remote = host_device_id
            .as_deref()
            .is_some_and(|id| local_device_id.as_deref() != Some(id));
        // Queue rows do not carry upstream's attachment-transfer escort, so
        // they retain the proven host-upload path and store absolute refs.
        // Appshot XML attributes require escaped final paths. The engine's plain
        // string replacement of pending refs cannot safely rewrite those, so
        // rich captures use the existing upload-before-send path.
        let queued_flow = !queue && staged_appshots.is_empty() && !staged.is_empty() && {
            let state = self.state.read(cx);
            let local_ok = local_device_id
                .as_deref()
                .is_some_and(|id| state.device_version_at_least(id, QUEUED_ATTACHMENTS_MIN));
            let host_ok = !host_is_remote
                || host_device_id
                    .as_deref()
                    .is_some_and(|id| state.device_version_at_least(id, QUEUED_ATTACHMENTS_MIN));
            local_ok && host_ok
        };
        // Upload identities minted NOW: in the queued flow the `pending://`
        // ref IS the persisted transport until the host rewrites it, so the
        // id must exist before any bytes move.
        let upload_ids: Vec<String> = staged
            .iter()
            .map(|_| uuid::Uuid::new_v4().to_string())
            .collect();
        // The echo carries attachment refs from the first frame, so photos
        // render while the send is still pending. Queued flow: the refs are
        // the real `pending://` identities (stable — no post-upload refresh).
        // Legacy flow: synthetic `pending/…` paths that the post-upload
        // refresh replaces with the host's absolute paths. Either way the
        // staged bytes are seeded into the transcript cache under every
        // device key the transcript consults.
        let echo_paths: Vec<String> = if queued_flow {
            staged
                .iter()
                .zip(&upload_ids)
                .map(|(att, id)| format!("pending://{id}/{}", att.name))
                .collect()
        } else {
            staged
                .iter()
                .map(|att| format!("pending/{}/{}", att.id, att.name))
                .collect()
        };
        let echo_appshot_paths: HashMap<String, String> = staged
            .iter()
            .zip(&echo_paths)
            .map(|(attachment, path)| (attachment.id.clone(), path.clone()))
            .collect();
        let echo_text = attachments::with_attachments(
            &appshots::with_appshots(&text, &staged_appshots, &echo_appshot_paths),
            &echo_paths,
        );
        // Queued flow also seeds the UPLOAD ALIAS: the host rewrites the
        // persisted ref to `{its uploads dir}/{id8}-{name}` — an absolute
        // path the sender can't predict, but whose id8 it minted. The alias
        // keeps the thumbnail on the already-local bytes through that
        // rewrite instead of blanking into a reload skeleton.
        if queued_flow {
            for (upload_id, att) in upload_ids.iter().zip(&staged) {
                attachments::seed_attachment_alias(
                    &device_id,
                    upload_id,
                    &att.name,
                    att.image.clone(),
                );
                if let Some(local) = local_device_id.as_deref()
                    && local != device_id
                {
                    attachments::seed_attachment_alias(
                        local,
                        upload_id,
                        &att.name,
                        att.image.clone(),
                    );
                }
            }
        }
        for (path, att) in echo_paths.iter().zip(&staged) {
            attachments::seed_attachment(&device_id, path, &att.name, att.image.clone());
            if let Some(local) = local_device_id.as_deref()
                && local != device_id
            {
                attachments::seed_attachment(local, path, &att.name, att.image.clone());
            }
        }

        // Optimistic echo (client-minted id doubles as the persisted message id,
        // so the doc frame dedups it away).
        let echo = SessionMessageEntry {
            id: message_id.clone(),
            role: zeron_doc::MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: echo_text.clone(),
            }],
            created_at,
            device_id: "local".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        self.launching_new_chat = is_new;
        if is_new {
            cx.emit(ComposerEvent::NewThreadTransitionStarted);
        }
        // A queued message is not in the transcript yet — the queue panel is
        // its echo, and it gets a real bubble when the host sends it.
        self.state.update(cx, |s, cx| {
            if is_new {
                s.select_chat(Some(chat_id.clone()), cx);
            }
            if should_publish_optimistic_echo(queue) {
                s.push_echo(&chat_id, echo);
                // Working overlay until the host executes the queued command —
                // without it a remote send flashed Completed (and could ring
                // the done-chime) in the queue→drain→sync gap.
                s.begin_pending_send(&chat_id, &message_id, chrono::Utc::now());
            }
            cx.notify();
        });

        self.input.update(cx, |input, cx| input.set_text("", cx));
        self.drafts.remove(&self.current_key);
        self.failure = None;
        self.sending = true;
        // A queued row is represented by the queue panel, not the transcript.
        // Claiming an own-turn anchor for it here would replace the live
        // turn's runway with an id that has no transcript row yet.
        if should_publish_optimistic_echo(queue) {
            cx.emit(ComposerEvent::Sent {
                chat_id: chat_id.clone(),
                message_id: message_id.clone(),
            });
        }
        cx.notify();

        let restore_text = typed;
        let err_chat_id = chat_id.clone();
        let err_message_id = message_id.clone();
        self.send_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<Option<String>, String> = async {
                // Attachments stage FIRST — before the chat row or anything
                // else exists. Staging is chat-independent (keyed by
                // uploadId), and ordering it first makes a new-chat send
                // atomic: a staging failure aborts with NOTHING created,
                // instead of stranding a just-minted empty chat (v0.2.12
                // "failed to stage → empty transcript" report).
                //
                // Queued flow: commit the bytes to the LOCAL engine's uploads
                // dir (fast, offline-safe) — the queued command carries the
                // `pending://` refs and the engine delivers the bytes to a
                // remote host afterwards, retrying until they land. Legacy
                // flow (old engines): stage on the host device up front,
                // bounded by a total budget so a degraded link fails the send
                // loudly instead of grinding through silent per-chunk retries
                // for minutes.
                let mut content = text.clone();
                let mut attachment_paths: Vec<String> = Vec::new();
                let mut transfers: Vec<serde_json::Value> = Vec::new();
                if !staged.is_empty() && queued_flow {
                    // Local staging is disk-speed; publish progress anyway so
                    // huge files still narrate.
                    let progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let total: u64 = staged.iter().map(|a| a.bytes().len() as u64).sum();
                    {
                        let progress = progress.clone();
                        this.update(cx, |composer, cx| {
                            composer.state.update(cx, |s, cx| {
                                s.begin_upload_progress(total, progress);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                    for (att, upload_id) in staged.iter().zip(&upload_ids) {
                        if let Err(err) = attachments::upload_attachment(
                            &engine,
                            cx.background_executor(),
                            None,
                            upload_id,
                            att,
                            Some(progress.clone()),
                        )
                        .await
                        {
                            tracing::warn!(name = %att.name, error = %err, "local attachment stage failed");
                            return Err("Couldn't stage the attachment locally.".to_string());
                        }
                        transfers.push(serde_json::json!({
                            "uploadId": upload_id,
                            "fileName": att.name,
                        }));
                    }
                    // The echo refs ARE the persisted refs — no refresh pass.
                    attachment_paths = echo_paths.clone();
                    content = echo_text.clone();
                } else if !staged.is_empty() {
                    let progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                    let total: u64 = staged.iter().map(|a| a.bytes().len() as u64).sum();
                    {
                        let progress = progress.clone();
                        this.update(cx, |composer, cx| {
                            composer.state.update(cx, |s, cx| {
                                s.begin_upload_progress(total, progress);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                    for (att, upload_id) in staged.iter().zip(&upload_ids) {
                        match attachments::upload_attachment(
                            &engine,
                            cx.background_executor(),
                            host_device_id.as_deref(),
                            upload_id,
                            att,
                            Some(progress.clone()),
                        )
                        .await
                        {
                            Ok(path) => attachment_paths.push(path),
                            Err(err) => {
                                tracing::warn!(name = %att.name, error = %err, "attachment upload failed");
                                return Err(
                                    "Couldn't upload the attachment — the device may be offline."
                                        .to_string(),
                                );
                            }
                        }
                    }
                    // Seed the transcript cache from local bytes so the sent
                    // bubble's thumbnails never round-trip (seedTranscript-
                    // Attachment in the original send path).
                    let seed_device = host_device_id.clone().unwrap_or_else(|| device_id.clone());
                    for (path, att) in attachment_paths.iter().zip(&staged) {
                        attachments::seed_attachment(&seed_device, path, &att.name, att.image.clone());
                        if seed_device != device_id {
                            attachments::seed_attachment(&device_id, path, &att.name, att.image.clone());
                        }
                    }
                    let appshot_paths: HashMap<String, String> = staged
                        .iter()
                        .zip(&attachment_paths)
                        .map(|(attachment, path)| (attachment.id.clone(), path.clone()))
                        .collect();
                    content = attachments::with_attachments(
                        &appshots::with_appshots(&text, &staged_appshots, &appshot_paths),
                        &attachment_paths,
                    );
                    // A normal send already has an optimistic echo: refresh it
                    // in place with the uploaded refs so its thumbnails never
                    // flicker. A queued message has no transcript echo at all;
                    // its queue row is the only representation until dispatch.
                    if should_publish_optimistic_echo(queue) {
                        let refreshed = SessionMessageEntry {
                            id: message_id.clone(),
                            role: zeron_doc::MessageRole::User,
                            parts: vec![MessagePart::Text {
                                id: "t0".into(),
                                text: content.clone(),
                            }],
                            created_at,
                            device_id: "local".into(),
                            status: None,
                            continuation_of: None,
                            duration_ms: None,
                        };
                        let echo_chat_id = chat_id.clone();
                        this.update(cx, |composer, cx| {
                            composer.state.update(cx, |s, cx| {
                                s.remove_echo(&echo_chat_id, &message_id);
                                s.push_echo(&echo_chat_id, refreshed);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                }

                // Resolve the working directory: existing chats keep theirs;
                // new chats run per the checkout plan (t3code env-mode): the
                // space's folder as-is, an EXISTING worktree of the picked ref
                // (a plain cwd override — multiple sessions share one
                // worktree), or a fresh isolated worktree created off the
                // picked base ref (CreateWorktree on send, targeted at the
                // space's device; the RPC relay-forwards).
                let mut cwd = if is_new {
                    // Project-less sessions run from the host's home dir —
                    // "~" is expanded on the host when the run spawns.
                    space_path.clone().or_else(|| Some("~".to_string()))
                } else {
                    existing_cwd
                }
                .unwrap_or_else(|| ".".to_string());
                let mut worktree_cwd: Option<String> = None;
                // Fresh-worktree plans ride the QUEUED Run command (a
                // WorktreeSpec the HOST materializes at drain time) instead of
                // a blocking CreateWorktree relay RPC here: the RPC had no
                // timeout, so a lost relay frame wedged the send on "Sending…"
                // forever while the session ran remotely anyway (2026-08-18).
                let mut run_worktree: Option<zeron_proto::WorktreeSpec> = None;
                // The picked ref rides createChat so the session footer names
                // it from the first frame (it read "Select ref" until the
                // host's diff reconciler got around to stamping the branch).
                let mut chat_branch: Option<String> = None;
                if is_new && space_path.is_some() {
                    match &plan {
                        crate::pickers::CheckoutPlan::CurrentCheckout { branch } => {
                            chat_branch = branch.clone();
                        }
                        crate::pickers::CheckoutPlan::ReuseWorktree { path, branch } => {
                            cwd = path.clone();
                            worktree_cwd = Some(path.clone());
                            chat_branch = Some(branch.clone());
                        }
                        crate::pickers::CheckoutPlan::NewWorktree { base } => {
                            // Footer shows the base until the host stamps the
                            // actual zeron/<name> branch post-creation. cwd
                            // stays the repo folder — an old host that doesn't
                            // know the spec degrades to the main checkout
                            // instead of failing the run.
                            chat_branch = base.clone();
                            if let Some(repo_path) = &space_path {
                                // A remote repo's branch list loads over the
                                // relay — on a bad link it may never arrive
                                // and the picker has no base. That must NOT
                                // silently drop the isolation the user picked
                                // (2026-08-19: "New worktree" ran in the main
                                // checkout): default to HEAD, which git — any
                                // host version — resolves as the repo's
                                // current checkout state.
                                let base =
                                    base.clone().unwrap_or_else(|| "HEAD".to_string());
                                run_worktree = Some(zeron_proto::WorktreeSpec {
                                    repo_path: repo_path.clone(),
                                    base,
                                    space_id: space_id.clone(),
                                });
                            }
                        }
                    }
                }

                // Best-effort Mutate createChat with the picked config: the
                // engine resolves device + cwd from the PROJECT row when one
                // is picked; project-less chats name the host device outright
                // (idempotent; the doc host would materialize the chat on
                // first command anyway, so failures are non-fatal).
                if is_new {
                    let mut mutate = serde_json::json!({
                        "op": "createChat",
                        "chatId": chat_id,
                    });
                    if let Some(object) = mutate.as_object_mut() {
                        match &space_id {
                            Some(space_id) => {
                                object.insert(
                                    "spaceId".into(),
                                    serde_json::Value::String(space_id.clone()),
                                );
                            }
                            None => {
                                object.insert(
                                    "deviceId".into(),
                                    serde_json::Value::String(device_id.clone()),
                                );
                            }
                        }
                    }
                    if let Some(object) = mutate.as_object_mut() {
                        if let Some(worktree_cwd) = &worktree_cwd {
                            object.insert(
                                "cwd".into(),
                                serde_json::Value::String(worktree_cwd.clone()),
                            );
                        }
                        if let Some(branch) = &chat_branch {
                            object.insert(
                                "branch".into(),
                                serde_json::Value::String(branch.clone()),
                            );
                        }
                        if let Some(config) = resolved.chat_config()
                            && let Ok(config) = serde_json::to_value(&config)
                        {
                            object.insert("config".into(), config);
                        }
                    }
                    if let Err(err) = attachments::call_with_timeout(
                        &engine,
                        cx.background_executor(),
                        methods::MUTATE,
                        mutate,
                        std::time::Duration::from_secs(30),
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "CreateChat mutate unavailable; doc host will materialize the chat");
                    }
                }
                // A hand-started side chat is minted by its first send. Unlike
                // a fresh session this one must land: the doc host would
                // materialize it without its parent link.
                if let Some(create) = create_side_chat {
                    if let Err(err) = attachments::call_with_timeout(
                        &engine,
                        cx.background_executor(),
                        methods::MUTATE,
                        create,
                        std::time::Duration::from_secs(30),
                    )
                    .await
                    {
                        tracing::warn!(error = %err, "side chat createChat failed");
                        return Err("Couldn't create the side chat.".to_string());
                    }
                    let saved = chat_id.clone();
                    this.update(cx, |composer, cx| {
                        composer
                            .state
                            .update(cx, |s, cx| s.side_chat_saved(&saved, cx));
                    })
                    .ok();
                }

                if queue {
                    // A queue row is editable UI state, so its text must stay
                    // free of the internal attachment-path trailer. The host
                    // rebuilds that transport when it promotes the row.
                    let appshot_paths = staged.iter().zip(&attachment_paths)
                        .map(|(attachment, path)| (attachment.id.clone(), path.clone())).collect();
                    let queue_body = appshots::with_appshots(&text, &staged_appshots, &appshot_paths);
                    let queue_text = if !clean_queue_attachment_text {
                        content.as_str()
                    } else if queue_body.trim().is_empty() && !attachment_paths.is_empty() {
                        attachments::ATTACHMENT_ONLY_TEXT
                    } else {
                        queue_body.as_str()
                    };
                    let params = serde_json::json!({
                        "chatId": chat_id,
                        "text": queue_text,
                        "attachments": attachment_paths,
                        "holdForTurnEnd": true,
                    });
                    let reply = engine
                        .client()
                        .call(methods::QUEUE_MESSAGE, params)
                        .await
                        .map_err(|e| format!("Send failed: {e}"))?;
                    let queue_id = reply
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| "Send failed: queue did not return an id".to_string())?;
                    return Ok(Some(queue_id.to_string()));
                }

                let expects_setup_handoff = run_worktree
                    .as_ref()
                    .and_then(|spec| spec.space_id.as_ref())
                    .is_some();
                let command = SessionCommandPayload::Run {
                    request: RunRequest {
                        mcp: None,
                        prompt: content.clone(),
                        harness: resolved.harness,
                        model: resolved.model.clone(),
                        reasoning: resolved.reasoning,
                        model_options: resolved.model_options.clone(),
                        cwd,
                        sandbox: SandboxLevel::WorkspaceWrite,
                        auto_approve: false,
                        resume: None,
                        attachments: attachment_paths,
                        worktree: run_worktree,
                    },
                    message_id: message_id.clone(),
                };
                let command = serde_json::to_value(&command)
                    .map_err(|e| format!("Send failed: {e}"))?;
                let mut params = serde_json::json!({ "chatId": chat_id, "command": command });
                if !transfers.is_empty() {
                    params["transfers"] = serde_json::Value::Array(transfers);
                }
                // Deadline-bounded: QueueCommand is a local write (in-process
                // or IPC), but a deferred engine handle can park forever.
                let queued = attachments::call_with_timeout(
                    &engine,
                    cx.background_executor(),
                    methods::QUEUE_COMMAND,
                    params,
                    std::time::Duration::from_secs(30),
                )
                .await
                .map_err(|e| format!("Send failed: {e}"))?;
                if expects_setup_handoff
                    && let Some(command_id) = queued
                        .get("commandId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                {
                    let poll_engine = engine.clone();
                    let poll_chat_id = chat_id.clone();
                    let poll_target_device_id = host_device_id.clone();
                    this.update(cx, |_, cx| {
                        cx.spawn(async move |this, cx| {
                            for _ in 0..480 {
                                let mut params = serde_json::json!({
                                    "chatId": poll_chat_id,
                                    "commandId": command_id,
                                });
                                if let (Some(target), Some(object)) =
                                    (&poll_target_device_id, params.as_object_mut())
                                {
                                    object.insert(
                                        "targetDeviceId".into(),
                                        serde_json::Value::String(target.clone()),
                                    );
                                }
                                match attachments::call_with_timeout(
                                    &poll_engine,
                                    cx.background_executor(),
                                    methods::TAKE_PROJECT_ACTION_SETUP,
                                    params,
                                    std::time::Duration::from_secs(10),
                                )
                                .await
                                {
                                    Ok(value) if value.get("ready").and_then(|v| v.as_bool()) == Some(true) => {
                                        let setup_action = value
                                            .get("setupAction")
                                            .cloned()
                                            .filter(|value| !value.is_null())
                                            .and_then(|value| serde_json::from_value(value).ok());
                                        let setup_error = value
                                            .get("setupError")
                                            .and_then(|value| value.as_str())
                                            .map(str::to_string);
                                        this.update(cx, |_, cx| {
                                            cx.emit(ComposerEvent::WorktreeSetup {
                                                chat_id: poll_chat_id.clone(),
                                                setup_action,
                                                setup_error,
                                                target_device_id: poll_target_device_id.clone(),
                                            });
                                        })
                                        .ok();
                                        return;
                                    }
                                    Err(error) if error.starts_with("unknown method: ") => return,
                                    Ok(_) | Err(_) => {}
                                }
                                cx.background_executor()
                                    .timer(Duration::from_millis(250))
                                    .await;
                            }
                            tracing::warn!(
                                chat = %poll_chat_id,
                                command = %command_id,
                                "worktree setup handoff timed out"
                            );
                        })
                        .detach();
                    })
                    .ok();
                }
                Ok(None)
            }
            .await;
            if result.is_err() && is_new {
                // A failed new-chat send must not strand a just-minted empty
                // chat in the sidebar (v0.2.12 "empty transcript" report).
                // Staging now runs before CreateChat, so usually nothing was
                // created — but a post-mutate failure (QueueCommand) still
                // leaves a row. Best-effort delete; a no-op if the chat was
                // never materialized.
                let _ = attachments::call_with_timeout(
                    &engine,
                    cx.background_executor(),
                    methods::MUTATE,
                    serde_json::json!({ "op": "deleteChat", "chatId": err_chat_id }),
                    std::time::Duration::from_secs(5),
                )
                .await;
            }
            this.update(cx, |composer, cx| {
                composer.sending = false;
                composer
                    .state
                    .update(cx, |s, _| s.end_upload_progress());
                if let Ok(Some(message_id)) = &result {
                    cx.emit(ComposerEvent::Queued {
                        chat_id: err_chat_id.clone(),
                        message_id: message_id.clone(),
                    });
                }
                if let Err(message) = result {
                    // Failure: red banner, echo removed, prompt back in the
                    // draft, staged files back in the stash. A failed NEW
                    // chat restores to the CANVAS (key "") and navigates back
                    // there — the minted chat is gone (deleted above), so
                    // nothing may restore under its key.
                    let restore_key = if is_new {
                        String::new()
                    } else {
                        err_chat_id.clone()
                    };
                    composer.failure = Some(message.into());
                    composer.failure_key = Some(restore_key.clone());
                    composer.state.update(cx, |s, cx| {
                        s.remove_echo(&err_chat_id, &err_message_id);
                        s.end_pending_send(&err_chat_id, &err_message_id);
                        if is_new && s.selected_chat.as_deref() == Some(err_chat_id.as_str()) {
                            // Back to the canvas; the navigation draft-swap
                            // loads the restored draft below.
                            s.select_chat(None, cx);
                        }
                        for comment in &comments {
                            s.add_review_comment(&restore_key, comment.clone());
                        }
                        cx.notify();
                    });
                    if is_new && composer.current_key != restore_key {
                        // A re-key swap to the canvas is pending (the
                        // select_chat(None) above); it loads this draft into
                        // the input on flush — setting the input directly
                        // here would be clobbered by that same swap.
                        composer.drafts.insert(restore_key.clone(), restore_text.clone());
                    } else {
                        // Already keyed to the restore target (either an
                        // existing chat, or the deleted row's watch event
                        // re-keyed to the canvas before this handler ran —
                        // no further swap will fire). Set the input directly.
                        composer.input.update(cx, |input, cx| input.set_text(restore_text, cx));
                    }
                    if !ordinary_staged.is_empty() {
                        // Merge by id (stashAttachments): files the user staged
                        // while the send was in flight survive the hand-back —
                        // draining the minted chat's slot too when the restore
                        // target is the canvas.
                        let mut merged = ordinary_staged.clone();
                        for key in [err_chat_id.clone(), restore_key.clone()] {
                            if let Some(slot) = composer.attachments.get_mut(&key) {
                                let fresh: Vec<_> = slot
                                    .drain(..)
                                    .filter(|e| !merged.iter().any(|f| f.id == e.id))
                                    .collect();
                                merged.extend(fresh);
                            }
                        }
                        composer.attachments.insert(restore_key.clone(), merged);
                    }
                    composer.restore_failed_appshots(&staged_appshots, &err_chat_id, &restore_key);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn interrupt_selected(&mut self, cx: &mut Context<Self>) {
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        self.interrupt_chat(chat_id, cx);
    }

    pub(crate) fn interrupt_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if !begin_interrupt(&mut self.interrupting, &chat_id) {
            return;
        }
        let params = interrupt_params(&chat_id);
        let task_chat_id = chat_id.clone();
        let failure_chat = chat_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::QUEUE_COMMAND, params).await;
            if let Err(err) = result {
                this.update(cx, |composer, cx| {
                    composer.interrupting.remove(&task_chat_id);
                    composer.failure = Some(format!("Stop failed: {err}").into());
                    composer.failure_key = Some(failure_chat);
                    cx.notify();
                })
                .ok();
            }
        });
        self.interrupt_tasks.insert(chat_id, task);
    }

    pub(crate) fn is_interrupting(&self, chat_id: &str) -> bool {
        self.interrupting.contains(chat_id)
    }

    // ---- wizard glue ----

    fn wizard_select(&mut self, option_ix: usize, cx: &mut Context<Self>) {
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        let step = wizard.select(option_ix);
        let has_pick = wizard.page_has_pick();
        self.input.update(cx, |input, cx| {
            input.set_placeholder(
                if has_pick {
                    "Type your own answer, or leave this blank to use the selected option"
                } else {
                    "Type your own answer, or pick an option above"
                },
                cx,
            )
        });
        match step {
            WizardStep::AutoAdvance => self.schedule_auto_advance(cx),
            WizardStep::Done(answers) => self.wizard_finish(answers, cx),
            WizardStep::Stay => {}
        }
        cx.notify();
    }

    fn schedule_auto_advance(&mut self, cx: &mut Context<Self>) {
        self.advance_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(AUTO_ADVANCE_MS))
                .await;
            this.update(cx, |composer, cx| composer.wizard_advance(cx))
                .ok();
        }));
    }

    fn wizard_advance(&mut self, cx: &mut Context<Self>) {
        let Some(wizard) = self.wizard.as_mut() else {
            return;
        };
        match wizard.advance() {
            WizardStep::Done(answers) => self.wizard_finish(answers, cx),
            _ => {
                // Moving on: clear the shared free-text input for the next page.
                self.input.update(cx, |input, cx| input.set_text("", cx));
                cx.notify();
            }
        }
    }

    fn wizard_back(&mut self, cx: &mut Context<Self>) {
        if let Some(wizard) = self.wizard.as_mut() {
            wizard.back();
            cx.notify();
        }
    }

    /// Submit RespondInput and retire the panel.
    fn wizard_finish(&mut self, answers: Vec<UserInputAnswer>, cx: &mut Context<Self>) {
        let Some(wizard) = self.wizard.take() else {
            return;
        };
        self.advance_task = None;
        self.answered_requests.insert(wizard.request_id.clone());
        self.input.update(cx, |input, cx| {
            input.set_text("", cx);
            // The panel borrowed the composer input; hand back its identity.
            input.set_placeholder("Do anything…", cx);
            input.set_key_context(MESSAGE_COMPOSER_CONTEXT, cx);
        });
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(chat_id) = self.state.read(cx).selected_chat.clone() else {
            return;
        };
        let request_id = wizard.request_id.clone();
        let command = SessionCommandPayload::RespondInput {
            request_id: request_id.clone(),
            answers,
        };
        let failure_chat = chat_id.clone();
        let params = match serde_json::to_value(&command) {
            Ok(value) => serde_json::json!({ "chatId": chat_id, "command": value }),
            Err(_) => return,
        };
        // `action_task`, NOT `send_task` — see `interrupt`.
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::QUEUE_COMMAND, params).await;
            if let Err(err) = result {
                this.update(cx, |composer, cx| {
                    composer.failure = Some(format!("Answer failed: {err}").into());
                    composer.failure_key = Some(failure_chat);
                    // The answer never left this device — put the panel back.
                    composer.answered_requests.remove(&request_id);
                    cx.notify();
                })
                .ok();
                return;
            }
            // Safety net against a dead-looking session: the command queued,
            // but the host may still REJECT it (e.g. the run's resolver is
            // gone). If the very same request is still the live pending input
            // once the host has had ample time to execute and the resolved
            // flag to sync back, the answer demonstrably didn't take —
            // un-hide the panel instead of leaving the question unanswerable.
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |composer, cx| {
                let transcript = composer.state.read(cx).transcript.clone();
                let still_pending = pending_input_request(&transcript)
                    .is_some_and(|(pending_id, _)| pending_id == request_id);
                if still_pending && composer.answered_requests.remove(&request_id) {
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn on_wizard_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        // Keys bubbling out of the free-text input must not double-handle:
        // digits select options only while the input is empty, and Enter is the
        // input's own Submit action when it has focus.
        let input_focused = self.input.read(cx).focus_handle.is_focused(window);
        let input_empty = self.input.read(cx).is_empty();
        let key = event.keystroke.key.as_str();
        // A BARE digit picks an option. With a modifier held the keystroke
        // belongs to an app shortcut — ⌘1..⌘9 jump to a sidebar row — and the
        // panel must not also consume it as a selection.
        if let Ok(digit) = key.parse::<usize>()
            && (1..=9).contains(&digit)
            && !event.keystroke.modifiers.modified()
        {
            if !input_focused || input_empty {
                self.wizard_select(digit - 1, cx);
                // Consumed as a selection: stop the platform from also
                // inserting the digit into the focused free-text input.
                cx.stop_propagation();
            }
        } else if key == "enter" {
            if !input_focused {
                self.wizard_advance(cx);
                cx.stop_propagation();
            }
        } else if key == "escape" {
            if wizard_escape_goes_back(key, input_focused, input_empty) {
                self.wizard_back(cx);
            }
            cx.stop_propagation();
        }
    }

    // ---- render pieces ----

    /// The agent-asked-a-question panel (zeron question-panel.tsx), rendered in
    /// place of the composer: the same floating-pill chrome (`rounded-[26px]
    /// border-white/[0.08] bg-white/[0.03] shadow-xl`), uppercase header +
    /// "1/3" counter chip, option rows with number kbd chips, a free-text
    /// override over a hairline, and Back / Next-Submit footer.
    fn render_wizard(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let theme = Theme::of(cx).clone();
        let Some(wizard) = self.wizard.clone() else {
            return gpui::Empty.into_any_element();
        };
        let counter = wizard.counter();
        let Some(question) = wizard.current().cloned() else {
            return gpui::Empty.into_any_element();
        };
        let page = wizard.page;
        let last = page + 1 >= wizard.questions.len();
        let typed_empty = self.input.read(cx).is_empty();
        let can_advance = wizard.page_has_pick() || !typed_empty;

        let options = question.options.iter().enumerate().map(|(ix, label)| {
            // Selection reads on the row only while no typed override exists
            // (typed answers win — zeron question-panel.tsx `isSel`).
            let picked = wizard.is_picked(ix) && typed_empty;
            div()
                .id(("wizard-option", ix))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .px(px(14.0))
                .py(px(10.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(if picked {
                    crate::theme::ink(0.16)
                } else {
                    gpui::transparent_black()
                })
                // zeron question-panel.tsx option rows: `transition-colors`.
                .bg(if picked {
                    crate::theme::ink(0.09)
                } else {
                    motion::hover_blend(
                        &format!("wizard-option-{ix}"),
                        crate::theme::ink(0.025),
                        crate::theme::ink(0.06),
                    )
                })
                .on_hover(motion::hover_listener(format!("wizard-option-{ix}")))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.wizard_select(ix, cx)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(crate::typography::ui_rems(13.5))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(if picked {
                            theme.text
                        } else {
                            theme.text.opacity(0.9)
                        })
                        .child(SharedString::from(label.clone())),
                )
                .when(ix < 9, |el| {
                    el.child(
                        // Number kbd chip: `size-[22px] rounded-md text-[11px]`.
                        div()
                            .flex_none()
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .bg(if picked {
                                crate::theme::ink(0.16)
                            } else {
                                crate::theme::ink(0.05)
                            })
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(if picked {
                                theme.text
                            } else {
                                theme.text_muted.opacity(0.6)
                            })
                            .child(SharedString::from(format!("{}", ix + 1))),
                    )
                })
        });

        div()
            .id("question-panel")
            .track_focus(&self.wizard_focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_wizard_key(event, window, cx)
            }))
            .rounded(px(COMPOSER_RADIUS))
            .border_1()
            .border_color(theme.border)
            .bg(theme.input_glass_bg())
            .when(!theme.is_frost(), |el| el.shadow_lg())
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(16.0))
                    .pt(px(16.0))
                    .flex()
                    .flex_col()
                    // Header: tracked uppercase + counter chip when paged.
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text_muted.opacity(0.6))
                                    .child(SharedString::from(crate::popover::tracked_upper(
                                        &question.header,
                                    ))),
                            )
                            .when(wizard.questions.len() > 1, |el| {
                                el.child(
                                    div()
                                        .h(px(20.0))
                                        .px(px(6.0))
                                        .flex()
                                        .items_center()
                                        .rounded(px(6.0))
                                        .bg(crate::theme::ink(0.06))
                                        .text_size(crate::typography::ui_rems(10.0))
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(theme.text_muted.opacity(0.6))
                                        .child(SharedString::from(counter)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .mt(px(6.0))
                            .text_size(crate::typography::ui_rems(15.0))
                            .line_height(px(20.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(SharedString::from(question.question.clone())),
                    )
                    .when(question.multi_select, |el| {
                        el.child(
                            div()
                                .mt(px(4.0))
                                .text_size(crate::typography::ui_rems(12.0))
                                .text_color(theme.text_muted.opacity(0.65))
                                .child(SharedString::from("Select one or more options.")),
                        )
                    })
                    .child(
                        div()
                            .mt(px(12.0))
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .children(options),
                    )
                    // Free-text override over a hairline (shares the composer
                    // input entity).
                    .child(
                        div()
                            .mt(px(12.0))
                            .border_t_1()
                            .border_color(crate::theme::hairline(0.06))
                            .pt(px(12.0))
                            .pb(px(4.0))
                            .px(px(4.0))
                            .child(self.input.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .px(px(16.0))
                    .pb(px(16.0))
                    .pt(px(4.0))
                    .child(if page > 0 {
                        crate::popover::btn_ghost(&theme, "Back", "wizard-back")
                            .id("wizard-back")
                            .on_click(cx.listener(|this, _, _, cx| this.wizard_back(cx)))
                            .into_any_element()
                    } else {
                        gpui::Empty.into_any_element()
                    })
                    .child(
                        crate::popover::btn_primary(&theme, if last { "Submit" } else { "Next" })
                            .id("wizard-submit")
                            .px(px(16.0))
                            .when(!can_advance, |el| el.opacity(0.4))
                            .on_click(cx.listener(|this, _, _, cx| this.wizard_advance(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_send_button(
        &mut self,
        mode: SendButtonMode,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = Theme::of(cx);
        // Zeron composer-actions.tsx: a size-7 filled circle — up-arrow to
        // send/queue, a dark rounded square on the same light circle to stop.
        match mode {
            SendButtonMode::Stop => div()
                .id("composer-stop")
                .size(px(28.0))
                .flex_none()
                .rounded_full()
                .bg(theme.text)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.opacity(0.85))
                .on_click(cx.listener(|this, _, _, cx| this.interrupt_selected(cx)))
                .child(div().size(px(11.0)).rounded(px(3.0)).bg(theme.bg))
                .into_any_element(),
            SendButtonMode::Send | SendButtonMode::Queue => {
                // Share the submission guard with Enter, including pending
                // edits and the new-session runnable-agent check.
                let blocked = self.send_blocked(cx);
                div()
                    .id("composer-send")
                    .size(px(28.0))
                    .flex_none()
                    .rounded_full()
                    .bg(theme.text)
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(blocked, |el| el.opacity(0.35))
                    .when(!blocked, |el| {
                        el.cursor_pointer()
                            .hover(|s| s.opacity(0.85))
                            .on_click(cx.listener(|this, _, _, cx| this.on_submit(cx)))
                    })
                    .child(
                        crate::icons::icon(crate::icons::ARROW_UP)
                            .size(px(14.0))
                            .text_color(theme.bg),
                    )
                    .into_any_element()
            }
        }
    }
}

/// The completion popups' floating rails run through
/// [`crate::popover::rail`]: the shared `popup_bar` state plus whichever
/// popup's rows are mounted — see [`Composer::active_popup_scroll`].
impl crate::popover::ScrollRailHost for Composer {
    fn rail_bar(&mut self) -> &mut crate::popover::MenuScrollbarState {
        &mut self.popup_bar
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.active_popup_scroll()
    }
}

/// Focus lands on the prompt input (window-level focus fallbacks — e.g. after
/// the focused terminal panel is hidden — route here).
impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_pending {
            self.focus_pending = false;
            let focus = self.input.focus_handle(cx);
            window.focus(&focus, cx);
        }
        let theme = Theme::of(cx).clone();
        let wizard_active = self.wizard.is_some();
        if self.mention.token.is_some()
            && (wizard_active || !self.input.focus_handle(cx).is_focused(window))
        {
            self.reset_mention(None, cx);
        }
        if self.slash.token.is_some()
            && (wizard_active || !self.input.focus_handle(cx).is_focused(window))
        {
            self.reset_slash(None, cx);
        }
        let mode = self.button_mode(cx);
        // Shape the current draft before sizing the pill. Waiting for the child
        // layout leaves the parent using the previous edit's height.
        self.input.update(cx, |input, cx| {
            if input.needs_measure && input.last_width > 0.0 {
                let mut style = window.text_style();
                style.font_family = theme.font_sans.clone();
                style.font_size = crate::typography::ui_rems(INPUT_TEXT_SIZE).into();
                style.color = if input.content.is_empty() {
                    theme.text_faint
                } else {
                    theme.text
                };
                input.layout_text(px(input.last_width), &style, window, cx);
            }
        });
        let (text_width, has_newline, content_height, last_width, epoch) = {
            let input = self.input.read(cx);
            (
                input.measured_text_width(),
                input.has_newline(),
                input.measured_content_height(),
                input.last_width,
                input.layout_epoch,
            )
        };
        let now = Instant::now();
        // Only measurements taken *after* the last flip may drive the next one
        // (at most one flip per layout pass — a flip invalidates the widths).
        let measured_since_flip = epoch > self.flip_epoch && last_width > 0.0;
        if measured_since_flip {
            // A same-mode width change is an interactive window/pane resize:
            // defer collapse until sizes settle for RESIZE_SETTLE_MS. Expansion
            // remains live so compact controls never squeeze the input away.
            if self.last_seen_width > 0.0 && (last_width - self.last_seen_width).abs() > 0.5 {
                self.width_changed_at = Some(now);
            }
            self.last_seen_width = last_width;
            if self.expanded_mode {
                if self.expanded_anchor <= 0.0 {
                    self.expanded_anchor = last_width;
                }
            } else {
                // The compact pill's content box is the layout-stable capacity
                // both thresholds measure against.
                self.compact_capacity = last_width - 8.0;
            }
        }
        let resizing = self
            .width_changed_at
            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(RESIZE_SETTLE_MS));
        if resizing && self.settle_task.is_none() {
            // Re-evaluate once the settle window has passed.
            self.settle_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(RESIZE_SETTLE_MS + 20))
                    .await;
                this.update(cx, |composer, cx| {
                    composer.settle_task = None;
                    cx.notify();
                })
                .ok();
            }));
        }
        // Layout-stable compact capacity: measured directly while compact;
        // while expanded, the learned value shifted by any container resize
        // (the expanded input width tracks the container 1:1).
        let capacity = if !self.expanded_mode {
            if last_width > 0.0 {
                last_width - 8.0
            } else {
                f32::MAX // before first measure default to compact
            }
        } else if self.compact_capacity > 0.0 {
            if self.expanded_anchor > 0.0 && last_width > 0.0 {
                self.compact_capacity + (last_width - self.expanded_anchor)
            } else {
                self.compact_capacity
            }
        } else {
            f32::MAX
        };
        let next = composer_flip(
            self.expanded_mode,
            text_width,
            capacity,
            has_newline,
            resizing,
        );
        let committed_flip = next != self.expanded_mode && measured_since_flip;
        if committed_flip {
            self.expanded_mode = next;
            self.flip_epoch = epoch;
            self.expanded_anchor = 0.0;
            // The mode change moves the input width; don't read that jump as
            // an interactive resize.
            self.last_seen_width = 0.0;
        }
        // New chats render expanded regardless of `expanded_mode` (see below),
        // so a mode flip there changes nothing visible — never morph it.
        let new_chat = self.state.read(cx).selected_chat.is_none();
        // Morph clock in ms; dividing by the measurement knob stretches the
        // timeline exactly like shell.rs eval_tween's scaled duration.
        let now_ms = self.morph_clock.elapsed().as_secs_f32() * 1000.0 / motion::speed_scale();
        let route_snap = self
            .route_snap_until
            .is_some_and(|until| Instant::now() < until);
        let dock_height_changed = std::mem::take(&mut self.dock_height_changed);
        self.flip_morph = if dock_height_changed
            || self.dock_frame.is_some_and(|frame| frame.active)
            || self.dock_reflow.active()
        {
            None
        } else {
            flip_morph_step(
                self.flip_morph,
                committed_flip && !new_chat,
                self.last_rendered_height,
                now_ms,
                motion::reduced_motion(cx),
                route_snap,
            )
        };
        let expanded = self.expanded_mode;

        // Chat-scoped failures render only under their own chat; a global
        // failure (no key) renders everywhere.
        let failure = self.failure.clone().filter(|_| {
            self.failure_key
                .as_ref()
                .is_none_or(|key| *key == self.current_key)
        });
        // Composer honesty: when the target's delivery path is degraded, say
        // UP FRONT that a send will queue (a durable local write delivered on
        // reconnect) instead of letting the button imply instant delivery.
        let queue_notice: Option<(SharedString, bool)> = {
            use zeron_proto::ConnectivityState as S;
            let state = self.state.read(cx);
            let degraded = match state.selected_chat.as_deref() {
                Some(id) => state.chat_delivery_degraded(id),
                None => {
                    // New-chat canvas: judge by the picked target device.
                    let remote_target = state
                        .effective_device_id()
                        .is_some_and(|id| state.local_device_id.as_deref() != Some(id.as_str()));
                    remote_target
                        && (matches!(state.connectivity.state, S::Offline | S::Reconnecting)
                            || state
                                .effective_device_id()
                                .is_some_and(|id| !state.device_online(&id, chrono::Utc::now())))
                }
            };
            let offline = state.connectivity.state == S::Offline;
            degraded.then(|| {
                let text: SharedString = if offline {
                    "Offline — messages will send when you're back online.".into()
                } else {
                    "Messages will send once the connection recovers.".into()
                };
                (text, offline)
            })
        };
        // The shell owns the width and its route animation. A route-dependent
        // cap here would cut a wide composer before its return glide finishes.
        let container = div()
            .w_full()
            .max_w(px(self.last_available_width.unwrap_or(COMPOSER_MAX_WIDTH)))
            .mx_auto()
            .flex()
            .flex_col()
            .gap(px(Theme::SPACE_SM))
            .px(px(Theme::SPACE_LG))
            .pb(px(Theme::SPACE_LG))
            .when_some(failure, |el, message| {
                // Amber with "Warning" for the offline-ish case (engine not
                // connected), red with "Error" for send/run failures. Click
                // dismisses.
                let offline = message.as_ref() == "Engine not connected";
                el.child(
                    notice_chip(
                        &theme,
                        offline,
                        if offline { "Warning" } else { "Error" },
                        message,
                        NoticeChipIcon::Plain,
                    )
                    .id("composer-failure")
                    .mx(px(4.0))
                    .mt(px(6.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.failure = None;
                        this.failure_key = None;
                        cx.notify();
                    })),
                )
            })
            .when_some(queue_notice, |el, (notice, offline)| {
                // Not a warning box (v0.2.12 feedback: the amber Notice read
                // as an error and flashed on every blip — pre-grace). One
                // quiet caption line, amber dot only for hard offline; it
                // clears itself the moment the path heals.
                let dot = if offline {
                    theme.warning
                } else {
                    theme.text_faint
                };
                el.child(crate::motion::fade_in(
                    "composer-queue-notice",
                    div()
                        .id("composer-queue-notice")
                        .mx(px(8.0))
                        .mt(px(6.0))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(11.0))
                        .line_height(px(14.0))
                        .text_color(theme.text_faint)
                        .child(div().size(px(5.0)).rounded_full().bg(dot))
                        .child(div().min_w_0().truncate().child(notice)),
                ))
            });

        if wizard_active {
            let wizard = self.render_wizard(cx);
            return container.child(motion::fade_quick("composer-wizard", div().child(wizard)));
        }

        // What is waiting to be sent, stacked directly above the box it was
        // typed in — the queue is a property of this composer, not a panel
        // somewhere else.
        let show_queue_latest_shortcut = self.queue_shortcut_revealed
            && self.editing_queued.is_none()
            && !self.pickers.read(cx).is_open()
            && !composer_has_content(
                self.input.read(cx).text(),
                self.staged().len() + self.staged_appshots().len(),
                self.staged_comments(cx).len(),
            );
        let container = container.when_some(
            self.render_queue_panel(show_queue_latest_shortcut, window, cx),
            |el, panel| {
                el.child(motion::fade_quick(
                    "composer-queue",
                    div()
                        .mx(px(QUEUE_SIDE_INSET))
                        // Cancel the column gap, then tuck the tray one pixel
                        // behind the composer painted after it.
                        .mb(px(-(Theme::SPACE_SM + QUEUE_COMPOSER_OVERLAP)))
                        .child(panel),
                ))
            },
        );
        // Escape backs out of a queue-row edit (the row keeps its old text).
        // Bound here rather than in the input: the input's own Escape belongs
        // to the mention/slash popups, which outrank this while they're open.
        let container = container.when(self.editing_queued.is_some(), |el| {
            el.on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape"
                    && this.mention.token.is_none()
                    && this.slash.token.is_none()
                    && this.cancel_queue_edit(cx)
                {
                    cx.stop_propagation();
                }
            }))
        });

        // Route coordination must not force an established thread into the
        // two-row layout. Short drafts keep the original skinny composer.
        let session_expanded = expanded;
        let expanded = expanded || new_chat;
        let dock_amount = self.dock_frame.map_or(0.0, |frame| frame.amount);
        let mut dock_layout = crate::composer_dock::DockLayout {
            hero_height: composer_total_height(content_height),
            thread_height: if session_expanded {
                (content_height + TEXTAREA_PAD_V).clamp(TEXTAREA_MIN - 16.0, TEXTAREA_MAX)
                    + ACTIONS_ROW_HEIGHT
                    + PILL_BORDER_V
            } else {
                COMPACT_TOTAL_HEIGHT
            },
            extra_height: 0.0,
            compact: !session_expanded,
        };

        // Committed-height morph: the layout below is already the NEW mode's;
        // only the pill's height (and the entrance fade/text glide driven by
        // `morph_t`) animates. Steady state renders exactly the target.
        // Staged attachments add the wrap strip's height to the pill in BOTH
        // modes (attachment-ui.tsx AttachmentStrip sits above the input row).
        let staged_count = self.staged().len();
        // The input width excludes the inline controls in compact mode.
        // Wrap against the pill's content width in both modes, accounting
        // for the outer container padding and the pill's 1px borders.
        let strip_width_hint =
            self.last_available_width.unwrap_or(COMPOSER_MAX_WIDTH) - 2.0 * Theme::SPACE_LG - 2.0;
        let appshot_count = self.staged_appshots().len();
        let strip_h = attachment_strip_height(staged_count, strip_width_hint);
        let comment_strip_h = comment_strip_height(self.staged_comments(cx).len());
        let base_height = if self.dock_frame.is_some() {
            dock_layout.height(dock_amount)
        } else if expanded {
            composer_total_height(content_height)
        } else {
            COMPACT_TOTAL_HEIGHT
        };
        let target_height =
            base_height + strip_h + appshot_strip_height(appshot_count) + comment_strip_h;
        dock_layout.extra_height = strip_h + appshot_strip_height(appshot_count) + comment_strip_h;
        let dock_owns_layout = self.dock_frame.is_some_and(|frame| frame.active)
            || dock_height_changed
            || self.dock_reflow.active();
        let (reflow_height, dock_compact_amount) =
            self.dock_frame.map_or((0.0, dock_amount), |frame| {
                self.dock_reflow
                    .sample(dock_layout, frame, motion::reduced_motion(cx), now)
            });
        let coordinated_route_morph = self
            .flip_morph
            .filter(|m| m.spec == motion::NEW_THREAD_TRANSITION && !m.done(now_ms));
        // The route state commits before its shared-element animation begins.
        // Reconstruct the departing chrome at t=0, then progressively trade
        // it for the destination chrome so neither route changes the outer
        // composer geometry in a single frame.
        let new_thread_chrome = self
            .dock_frame
            .map(|frame| frame.selectors())
            .unwrap_or_else(|| {
                coordinated_route_morph.map_or_else(
                    || if new_chat { 1.0 } else { 0.0 },
                    |morph| {
                        let progress = morph.progress(now_ms);
                        if new_chat { progress } else { 1.0 - progress }
                    },
                )
            });
        let (new_thread_chrome_opacity, session_chrome_opacity) = self.dock_frame.map_or_else(
            || route_chrome_opacities(new_thread_chrome),
            |frame| (frame.selectors(), frame.footer()),
        );
        self.height_morph = if dock_owns_layout {
            None
        } else if coordinated_route_morph.is_some() {
            coordinated_route_morph
        } else {
            flip_morph_step(
                self.height_morph,
                (target_height - self.last_target_height).abs() > 0.5,
                self.last_rendered_height,
                now_ms,
                motion::reduced_motion(cx),
                route_snap,
            )
        };
        self.last_target_height = target_height;
        let pill_height = self
            .height_morph
            .map_or(target_height, |m| m.height(target_height, now_ms))
            + reflow_height;
        if self.height_morph.is_some() || self.dock_reflow.active() {
            window.request_animation_frame();
        }
        let (_, morph_t, morphing) = match self.flip_morph {
            Some(m) if !m.done(now_ms) => {
                (m.height(target_height, now_ms), m.progress(now_ms), true)
            }
            _ => (target_height, 1.0, false),
        };
        if !morphing {
            self.flip_morph = None;
        } else {
            // Manual tween drive: keep frames coming (shell.rs motion_active).
            window.request_animation_frame();
        }
        self.last_rendered_height = pill_height;
        self.dock_clearance_correction = self.dock_frame.map_or(0.0, |frame| {
            dock_layout.height(if frame.docked { 1.0 } else { 0.0 }) - pill_height
        });
        // Route morphs use the dock's reversible clock; typing flips keep
        // their existing local clock once the composer reaches its dock.
        let layout_morph_t = if dock_owns_layout {
            if expanded {
                1.0 - dock_compact_amount
            } else {
                dock_compact_amount
            }
        } else {
            morph_t
        };
        let text_pt = morph_text_pad(layout_morph_t);
        let surface_radius = COMPOSER_RADIUS - 4.0 * dock_amount;
        let route_to_single_line =
            self.dock_frame.is_some_and(|frame| frame.active) && !session_expanded;
        let textarea_height = (pill_height
            - strip_h
            - appshot_strip_height(appshot_count)
            - comment_strip_h
            - PILL_BORDER_V
            - ACTIONS_ROW_HEIGHT)
            .max(if route_to_single_line {
                INPUT_LINE_HEIGHT + text_pt + 4.0
            } else {
                0.0
            });
        self.input.update(cx, |input, cx| {
            let height = if expanded {
                (textarea_height - text_pt - 4.0).max(0.0)
            } else {
                INPUT_LINE_HEIGHT
            };
            let settled_height = if expanded {
                (base_height - PILL_BORDER_V - ACTIONS_ROW_HEIGHT - TEXTAREA_PAD_V).max(
                    if route_to_single_line {
                        INPUT_LINE_HEIGHT
                    } else {
                        0.0
                    },
                )
            } else {
                INPUT_LINE_HEIGHT
            };
            let resizing = self.height_morph.is_some() || self.dock_reflow.active();
            let top_padding = if expanded { text_pt } else { 0.0 };
            if input.viewport_height != Some(height)
                || input.settled_viewport_height != Some(settled_height)
                || input.resizing != resizing
                || input.overflow_top_padding != top_padding
            {
                input.resizing = resizing;
                input.overflow_top_padding = top_padding;
                input.viewport_height = Some(height);
                input.settled_viewport_height = Some(settled_height);
                cx.notify();
            }
        });

        let send_button = self.render_send_button(mode, cx);
        // Attach button — opens the native image picker (the original's hidden
        // `<input type=file accept="image/*" multiple>`); paste/drop also feed
        // the same strip. The leading utility group owns the spacing between
        // this button and the model picker.
        let attach = div()
            .id("composer-attach")
            .size(px(28.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            // zeron composer-actions.tsx attach: `transition-colors`.
            .bg(motion::hover_blend(
                "composer-attach",
                gpui::transparent_black(),
                crate::theme::ink(0.10),
            ))
            .on_hover(motion::hover_listener("composer-attach"))
            .on_click(cx.listener(|this, _, _, cx| this.open_file_picker(cx)))
            .child(
                crate::icons::icon(crate::icons::PAPERCLIP)
                    // Its painted bounds are centered in the 24px viewbox;
                    // a larger glyph balances the brand icon without moving
                    // it off-center inside the unchanged 28px hit target.
                    .size(px(18.0))
                    .text_color(theme.text_muted),
            );
        // Staged-thumbnail strip (attachment-ui.tsx AttachmentStrip), above
        // the input inside the pill in both modes.
        let strip = self.render_attachment_strip(&theme, cx);
        let appshot_strip = self.render_appshot_strip(&theme, window, cx);
        let comments_chip = self.render_comments_chip(&theme, cx);

        let pill_border = theme.composer_surface_border();
        // Compensate for the transcript canvas beneath the frosted surface.
        // Keep the opaque fallback when frost is disabled or unsupported.
        let pill = div()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Padding and action controls are part of the text composer.
                    // Open menus keep their own keyboard/search focus.
                    if !this.pickers.read(cx).is_open() {
                        window.focus(&this.input.focus_handle(cx), cx);
                    }
                }),
            )
            .rounded(px(surface_radius))
            .border_1()
            .border_color(pill_border)
            .bg(theme.composer_surface_bg())
            .when(!theme.is_frost(), |el| el.shadow_lg());
        // The pill's bottom edge is stationary on screen (the composer sits at
        // the bottom of the shell column; growth moves the TOP edge), so the
        // controls pin to the bottom and only the text glides with the reveal
        // (round-9 follow-up: the send/attach/chips must not ride the height,
        // while the model picker fades between its two horizontal anchors).
        let cluster_dy = morph_cluster_dy(layout_morph_t);
        let action_inset = morph_cluster_inset(expanded, layout_morph_t);
        // Share the height/route timeline instead of starting an independent
        // animation. Reversals continue from the current handoff phase.
        if self.model_handoff_morph != self.flip_morph {
            self.model_handoff_from = self.model_handoff_position;
            self.model_handoff_morph = self.flip_morph;
        }
        let compact_target = if expanded { 0.0 } else { 1.0 };
        self.model_handoff_position = if dock_owns_layout {
            dock_compact_amount
        } else {
            self.flip_morph.map_or(compact_target, |morph| {
                motion::lerp(
                    self.model_handoff_from,
                    compact_target,
                    motion::EASE_IN_OUT.eval(morph.raw(now_ms)),
                )
            })
        };
        // The shell supplies this frame's animated width before rendering us.
        // Measured bounds still belong to the previous frame here; using them
        // would add the per-frame width delta to the model selector's glide.
        let surface_width = self.last_available_width.map_or_else(
            || {
                self.surface_bounds
                    .get()
                    .map_or(strip_width_hint + PILL_BORDER_V, |bounds| {
                        f32::from(bounds.size.width)
                    })
            },
            |width| (width - 2.0 * Theme::SPACE_LG).max(0.0),
        );
        let model_travel = (surface_width
            - PILL_BORDER_V
            - action_inset
            - 28.0
            - ACTION_UTILITY_GAP
            - self
                .model_bounds
                .get()
                .map_or(0.0, |bounds| f32::from(bounds.size.width))
            - ACTION_PRIMARY_GAP
            - 28.0
            - action_inset)
            .max(0.0);
        let (model_side, model_opacity, model_drift) = model_handoff(self.model_handoff_position);
        let model_offset = (model_side - compact_target) * model_travel + model_drift;
        let measured_model_bounds = self.model_bounds.clone();
        let model_picker = div()
            .min_w_0()
            .max_w(px(surface_width * 0.45))
            .relative()
            .left(px(model_offset))
            .opacity(model_opacity)
            .child(self.pickers.clone())
            .child(
                gpui::canvas(
                    move |bounds, _, _| measured_model_bounds.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        let body = if expanded {
            // Expanded: textarea on top (`px-4 pb-1 pt-4`), actions row
            // (8px bottom + 2px top, 32px chips → 42px) ABSOLUTE at the pill's
            // stationary bottom — constant screen-y through the morph, with
            // the compact↔expanded centering delta gliding out. The
            // text viewport follows the animated height so it cannot paint
            // over the controls. Its width stays fixed (no tween rewraps);
            // top padding eases 12→16. Attachment and Send stay on the bottom
            // anchor while the model chip fades between its horizontal slots.
            pill.h(px(pill_height))
                .overflow_hidden()
                .relative()
                .flex()
                .flex_col()
                .children(comments_chip)
                .children(appshot_strip)
                .children(strip)
                .child(
                    div()
                        .h(px(textarea_height))
                        .flex_none()
                        .overflow_hidden()
                        .px(px(16.0))
                        .pt(px(text_pt))
                        .pb(px(4.0))
                        .child(self.render_input_with_completion()),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(px(-cluster_dy))
                        .h(px(ACTIONS_ROW_HEIGHT))
                        .flex()
                        .flex_row()
                        .items_center()
                        // Shared group geometry (see CLUSTER_X_DELTA): the
                        // attachment belongs to the utility pickers, while
                        // Send has a larger structural separation.
                        .gap(px(ACTION_PRIMARY_GAP))
                        .px(px(action_inset))
                        .pt(px(2.0))
                        .pb(px(ACTIONS_BOTTOM_PAD))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(ACTION_UTILITY_GAP))
                                .child(attach)
                                .child(model_picker),
                        )
                        .child(send_button),
                )
        } else {
            // Compact pill: attachment on the left, input in the middle,
            // then model and Send on the right, all on one 47px line.
            // The row is BOTTOM-justified: during the collapse morph the pill
            // top sweeps down over a stationary row, the text walks down from
            // its expanded resting place via a decaying relative offset, and
            // attachment/Send hold their spots (the centering delta gliding
            // in), with the model handoff sharing that same timeline.
            let text_glide = if dock_owns_layout {
                collapse_text_glide(dock_layout.hero_height, dock_compact_amount)
            } else {
                match self.flip_morph {
                    Some(m) if morphing => collapse_text_glide(m.from, morph_t),
                    _ => 0.0,
                }
            };
            pill.h(px(pill_height))
                .overflow_hidden()
                .flex()
                .flex_col()
                .justify_end()
                .children(comments_chip)
                .children(appshot_strip)
                .children(strip)
                .child(
                    div()
                        .h(px(COMPACT_TOTAL_HEIGHT - PILL_BORDER_V))
                        .flex()
                        .flex_row()
                        .items_center()
                        .child(
                            div()
                                .flex_none()
                                .pl(px(action_inset))
                                .relative()
                                .top(px(-cluster_dy))
                                .child(attach),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .px(px(8.0))
                                .relative()
                                .top(px(-text_glide))
                                .child(self.render_input_with_completion()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .max_w(px(surface_width * 0.45))
                                .relative()
                                .top(px(-cluster_dy))
                                .child(model_picker),
                        )
                        .child(
                            div()
                                .flex_none()
                                .pl(px(ACTION_PRIMARY_GAP))
                                .pr(px(action_inset))
                                .relative()
                                .top(px(-cluster_dy))
                                .child(send_button),
                        ),
                )
        };
        let new_thread_target_selectors = (new_thread_chrome_opacity > 0.0).then(|| {
            self.pickers.update(cx, |pickers, cx| {
                pickers.render_new_thread_target_selectors(cx)
            })
        });
        let new_thread_git_selectors = (new_thread_chrome_opacity > 0.0)
            .then(|| {
                self.pickers.update(cx, |pickers, cx| {
                    pickers.render_new_thread_git_selectors(cx)
                })
            })
            .flatten();
        let has_new_thread_git_selectors = self
            .state
            .read(cx)
            .selected_space_row()
            .is_some_and(|space| space.git_detected);
        // The file dropzone lives in the shell (the whole conversation column,
        // not just the pill — shell.rs `chat-dropzone`); drops land back here
        // via `add_paths`.
        // Frosted: the pill backdrop-blurs the transcript scrolling under it
        // (the popover glass treatment; radius matches the pill's rounding).
        // The shell keeps this entity under one parent on both routes. The
        // surface itself never fades, and frost follows the same morph radius.
        let pill_surface = div()
            .relative()
            .id("composer-surface")
            .child(crate::frost::frosted(surface_radius, 16.0, body))
            .child({
                let measured = self.surface_bounds.clone();
                // All prepaint completes before any paint. The background
                // reads this cell during paint, never last frame's geometry.
                gpui::canvas(
                    move |bounds, _, _| measured.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0()
            })
            // Both completion popups span the full pill width above it —
            // the file-mention and slash tokens are mutually exclusive.
            .children(self.render_file_mention_popup(&theme, cx))
            .children(self.render_slash_popup(&theme, cx));
        // Restore the original chip-only selector treatment: destination at
        // the top-right, no surrounding surface. Cancel the column gap as the
        // row collapses so the pill never jumps at the route boundary.
        let container = if self.dock_frame.is_some() {
            // Floating selectors share the surface's origin and never change its height.
            container.relative().child(
                div()
                    .id("dock-target-selectors")
                    .absolute()
                    .top(px(-28.0))
                    .left(px(Theme::SPACE_LG + 10.0))
                    .right(px(Theme::SPACE_LG + 10.0))
                    .h(px(NEW_THREAD_SELECTOR_ROW_HEIGHT))
                    .flex()
                    .items_start()
                    .justify_end()
                    .opacity(new_thread_chrome_opacity)
                    .children(new_thread_target_selectors),
            )
        } else if new_thread_chrome > 0.0 {
            container.child(
                div()
                    .w_full()
                    .h(px(NEW_THREAD_SELECTOR_ROW_HEIGHT * new_thread_chrome))
                    .mb(px(-Theme::SPACE_SM * (1.0 - new_thread_chrome)))
                    .px(px(10.0))
                    .flex()
                    .items_start()
                    .justify_end()
                    .opacity(new_thread_chrome_opacity)
                    .children(new_thread_target_selectors),
            )
        } else {
            container
        };
        let container = container.child(pill_surface);

        // The lower slot keeps a stable footprint for Git projects while its
        // old floating checkout/ref controls dissolve into the session footer.
        // Non-Git sessions grow the slot continuously from zero.
        let session_chrome = 1.0 - new_thread_chrome;
        let bottom_slot = if has_new_thread_git_selectors || self.dock_frame.is_some() {
            1.0
        } else {
            session_chrome
        };
        let container = if bottom_slot > 0.0 {
            let footer = (session_chrome_opacity > 0.0 && !self.side_chat).then(|| {
                self.pickers
                    .update(cx, |pickers, cx| pickers.render_footer(cx))
            });
            if session_chrome_opacity > 0.0 {
                let harness = (!self.side_chat)
                    .then(|| self.pickers.read(cx).resolved(cx).harness)
                    .flatten();
                let target = {
                    let state = self.state.read(cx);
                    state
                        .selected_chat_row()
                        .map(|chat| chat.device_id.clone())
                        .filter(|device| state.local_device_id.as_ref() != Some(device))
                };
                self.account_usage
                    .update(cx, |usage, cx| usage.track(harness, target, cx));
            }
            container.child(
                div()
                    .w_full()
                    .h(px(SESSION_FOOTER_HEIGHT * bottom_slot))
                    .mt(px(-Theme::SPACE_SM * (1.0 - bottom_slot)))
                    .mb(px(-Theme::SPACE_SM * bottom_slot))
                    .relative()
                    .when(new_thread_chrome_opacity > 0.0, |slot| {
                        slot.child(
                            div()
                                .absolute()
                                .inset_0()
                                .px(px(10.0))
                                .flex()
                                .items_center()
                                .opacity(new_thread_chrome_opacity)
                                .children(new_thread_git_selectors),
                        )
                    })
                    .when(session_chrome_opacity > 0.0, |slot| {
                        slot.child(
                            div()
                                .absolute()
                                .inset_0()
                                .w_full()
                                .h(px(SESSION_FOOTER_HEIGHT))
                                .flex()
                                .items_center()
                                .opacity(session_chrome_opacity)
                                .child(div().flex_1().min_w_0().children(footer.flatten()))
                                .child(
                                    // The footer row's own 4px gap: the PR badge
                                    // ends flush with the row, so the rings keep
                                    // their distance here.
                                    div()
                                        .flex_none()
                                        .pl(px(4.0))
                                        .pr(px(10.0))
                                        .child(self.account_usage.clone()),
                                ),
                        )
                    }),
            )
        } else {
            container
        };
        // Full-size preview of a staged thumbnail (AttachmentPreviewDialog).
        if let Some(preview) = self.preview.clone() {
            if std::mem::take(&mut self.preview_focus_pending) {
                window.focus(&self.preview_focus, cx);
            }
            let weak = cx.weak_entity();
            return container.child(attachments::lightbox(
                window,
                &preview,
                &self.preview_focus,
                move |window, cx| {
                    // Hand focus back to the input so typing (and the next
                    // Escape) lands where it did before the lightbox opened.
                    if let Ok(input_focus) = weak.update(cx, |this, cx| {
                        this.preview = None;
                        cx.notify();
                        this.input.read(cx).focus_handle.clone()
                    }) {
                        window.focus(&input_focus, cx);
                    }
                },
                cx,
            ));
        }
        container
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composer_focus_window(
        cx: &mut gpui::TestAppContext,
    ) -> (tempfile::TempDir, gpui::WindowHandle<Composer>) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::dark());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Composer::new(state, cx)
        });
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        (dir, window)
    }

    fn with_composer_input(
        cx: &mut gpui::TestAppContext,
        test: impl FnOnce(&mut ComposerInput, &mut Window, &mut Context<ComposerInput>),
    ) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, window, cx| {
                composer
                    .input
                    .update(cx, |input, cx| test(input, window, cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn dock_morph_restores_skinny_height_with_a_continuous_editor_origin(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = composer_focus_window(cx);
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        for thread_width in [436.0, 592.0, 768.0, 1232.0] {
            for docked in [true, false] {
                let amounts = if docked {
                    [0.0, 0.2, 0.6, 0.98, 1.0]
                } else {
                    [1.0, 0.98, 0.6, 0.2, 0.0]
                };
                for amount in amounts {
                    let outer_width = motion::lerp(COMPOSER_MAX_WIDTH, thread_width, amount);
                    cx.update(|cx| {
                    handle
                        .update(cx, |composer, window, cx| {
                            window.resize(size(px(outer_width), px(800.0)));
                            composer.set_available_width(outer_width, cx);
                            composer.state.update(cx, |state, _| {
                                state.selected_chat = docked.then(|| "chat".into());
                            });
                            composer.on_state_changed(cx);
                            composer
                                .input
                                .update(cx, |input, cx| input.set_text("Hi", cx));
                            composer.expanded_mode = false;
                            let mut frame = crate::composer_dock::DockFrame::settled(docked);
                            frame.amount = amount;
                            frame.active = amount != if docked { 1.0 } else { 0.0 };
                            composer.set_dock_frame(frame, cx);
                        })
                        .unwrap();
                    cx.update_window(handle.into(), |_, window, cx| {
                        window.refresh();
                        window.draw(cx).clear();
                    })
                    .unwrap();
                    // Inspect the first painted frame before TestAppContext
                    // flushes effects and automatically draws dirty views.
                    handle.read_with(cx, |composer, cx| {
                    assert_eq!(composer.input, input);
                    let surface = composer.surface_bounds.get().unwrap();
                    assert!((f32::from(surface.size.width) - (outer_width - 2.0 * Theme::SPACE_LG)).abs() <= 1.0,
                        "surface width clipped: docked={docked}, amount={amount}, outer={outer_width}, surface={surface:?}");
                    let origin = input.read(cx).last_bounds.unwrap().origin;
                    assert!((f32::from(origin.y - surface.top()) - (17.0 - 4.0 * amount)).abs() <= 1.0,
                        "editor jumped: docked={docked}, amount={amount}, origin={origin:?}, surface={surface:?}");
                    let model = composer.model_bounds.get().unwrap();
                    let inset = motion::lerp(12.0, 8.0, amount);
                    let left = surface.left() + px(1.0 + inset + 28.0 + ACTION_UTILITY_GAP);
                    let travel = surface.size.width - px(2.0 + inset + 28.0 + ACTION_UTILITY_GAP
                        + ACTION_PRIMARY_GAP + 28.0 + inset) - model.size.width;
                    let (side, _, drift) = model_handoff(amount);
                    let expected_x = left + travel * side + px(drift);
                    assert!((f32::from(model.left() - expected_x)).abs() <= 1.0,
                        "model jumped: docked={docked}, amount={amount}, actual={model:?}, expected={expected_x:?}");
                    let expected = if docked { COMPACT_TOTAL_HEIGHT } else { COMPOSER_MIN_HEIGHT };
                    assert!((composer.last_rendered_height + composer.dock_clearance_correction - expected).abs() < 0.1);
                    assert!((composer.last_rendered_height - motion::lerp(COMPOSER_MIN_HEIGHT, COMPACT_TOTAL_HEIGHT, amount)).abs() < 0.1);
                        }).unwrap();
                    });
                }
            }
        }
    }

    #[test]
    fn model_handoff_hides_relocation_and_keeps_visible_motion_local() {
        assert_eq!(model_handoff(0.0), (0.0, 1.0, 0.0));
        assert_eq!(model_handoff(1.0), (1.0, 1.0, -0.0));
        for amount in [0.44, 0.49, 0.50, 0.51, 0.56] {
            assert!(model_handoff(amount).1 < 0.0001);
        }
        for step in 0..=100 {
            let (side, opacity, drift) = model_handoff(step as f32 / 100.0);
            assert!((0.0..=1.0).contains(&opacity));
            assert!(drift.abs() <= 6.0);
            assert!(side == 0.0 || side == 1.0);
        }
    }

    #[gpui::test]
    fn dock_reflow_paints_attachment_wrap_without_a_height_jump(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aF1cAAAAASUVORK5CYII=").unwrap();
        handle
            .update(cx, |composer, _, cx| {
                composer
                    .state
                    .update(cx, |state, _| state.selected_chat = Some("chat".into()));
                composer.on_state_changed(cx);
                composer.attachments.insert(
                    composer.current_key.clone(),
                    (0..10)
                        .map(|i| {
                            attachments::stage_png_bytes(format!("image-{i}.png"), png.clone())
                        })
                        .collect(),
                );
                composer.set_available_width(698.0, cx);
                composer.set_dock_frame(crate::composer_dock::DockFrame::settled(true), cx);
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Hi", cx));
            })
            .unwrap();
        let mut frame = crate::composer_dock::DockFrame::settled(true);
        frame.active = true;
        frame.amount = 0.4;
        handle
            .update(cx, |composer, _, cx| composer.set_dock_frame(frame, cx))
            .unwrap();
        let before = handle
            .read_with(cx, |composer, _| composer.last_rendered_height)
            .unwrap();
        // Both widths are within the same route frame: only the thumbnail row
        // changes. Inspect before GPUI's automatic follow-up paints can hide it.
        cx.update(|cx| {
            handle
                .update(cx, |composer, _, cx| {
                    composer.set_available_width(697.0, cx)
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
            handle
                .read_with(cx, |composer, cx| {
                    assert_eq!(composer.staged().len(), 10);
                    assert_eq!(attachment_strip_height(10, 698.0 - 34.0), 68.0);
                    assert_eq!(attachment_strip_height(10, 697.0 - 34.0), 132.0);
                    let surface = composer.surface_bounds.get().unwrap();
                    assert!(
                        (f32::from(surface.size.height) - before).abs() <= 1.0,
                        "attachment reflow jumped from {before} to {:?}",
                        surface.size.height
                    );
                    assert!(composer.dock_reflow.active());
                    assert!(composer.height_morph.is_none());
                    assert!(
                        (composer.last_rendered_height + composer.dock_clearance_correction
                            - (COMPACT_TOTAL_HEIGHT + 132.0))
                            .abs()
                            < 0.1,
                        "transcript clearance must reserve the destination height"
                    );
                    assert_eq!(composer.input.read(cx).text(), "Hi");
                })
                .unwrap();
        });
    }

    #[gpui::test]
    fn dock_reflow_keeps_text_growth_and_model_controls_continuous(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, _, cx| {
                composer
                    .state
                    .update(cx, |state, _| state.selected_chat = Some("chat".into()));
                composer.on_state_changed(cx);
                composer.set_available_width(592.0, cx);
                composer.set_dock_frame(crate::composer_dock::DockFrame::settled(true), cx);
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Hi", cx));
            })
            .unwrap();
        let mut frame = crate::composer_dock::DockFrame::settled(true);
        frame.active = true;
        frame.amount = 0.7;
        handle
            .update(cx, |composer, _, cx| composer.set_dock_frame(frame, cx))
            .unwrap();
        let (before, model_before) = handle
            .read_with(cx, |composer, _| {
                (
                    composer.last_rendered_height,
                    composer.model_bounds.get().unwrap().left(),
                )
            })
            .unwrap();
        cx.update(|cx| {
            handle
                .update(cx, |composer, _, cx| {
                    composer
                        .input
                        .update(cx, |input, cx| input.set_text("One\nTwo\nThree\nFour", cx));
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
            handle
                .read_with(cx, |composer, _| {
                    assert!(composer.expanded_mode);
                    assert!(composer.dock_reflow.active());
                    assert!(composer.flip_morph.is_none());
                    assert!(
                        (composer.last_rendered_height - before).abs() <= 1.0,
                        "text growth jumped from {before} to {}",
                        composer.last_rendered_height
                    );
                    assert!((composer.model_handoff_position - 0.7).abs() < 0.001);
                    assert!(
                        (f32::from(composer.model_bounds.get().unwrap().left() - model_before))
                            .abs()
                            <= 1.0,
                        "model selector jumped on the compact/expanded commit"
                    );
                })
                .unwrap();
        });
    }

    #[gpui::test]
    fn conversation_resize_reflows_the_live_draft_and_preserves_the_flip(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = composer_focus_window(cx);
        let draft = "A draft near the compact boundary. ".repeat(2);
        handle
            .update(cx, |composer, window, cx| {
                composer
                    .state
                    .update(cx, |state, _| state.selected_chat = Some("chat".into()));
                composer.on_state_changed(cx);
                composer.route_snap_until = None;
                composer.set_dock_frame(crate::composer_dock::DockFrame::settled(true), cx);
                composer.set_available_width(1232.0, cx);
                window.resize(size(px(1232.0), px(800.0)));
            })
            .unwrap();
        let draw = |cx: &mut gpui::TestAppContext| {
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
            })
            .unwrap();
        };
        for _ in 0..3 {
            draw(cx);
        }
        // Finish measuring the wide input before typing; the hero's previous
        // width must not accidentally put this fixture into the resize hold.
        handle
            .update(cx, |composer, _, cx| {
                composer.width_changed_at = None;
                composer.input.update(cx, |input, cx| {
                    input.set_text(draft.clone(), cx);
                    input.selected_range = 2..8;
                });
            })
            .unwrap();
        for _ in 0..3 {
            draw(cx);
        }
        let input = handle
            .read_with(cx, |composer, cx| {
                assert!(
                    !composer.expanded_mode,
                    "draft must fit the wide compact input: text={}, capacity={}",
                    composer.input.read(cx).measured_text_width(),
                    composer.compact_capacity,
                );
                assert!(composer.input.read(cx).measured_text_width() < composer.compact_capacity);
                composer.input.clone()
            })
            .unwrap();

        handle
            .update(cx, |composer, window, cx| {
                composer.set_available_width(592.0, cx);
                window.resize(size(px(592.0), px(800.0)));
            })
            .unwrap();
        for _ in 0..3 {
            draw(cx);
        }
        handle
            .update(cx, |composer, _, _| {
                assert!(
                    composer.expanded_mode,
                    "narrowing must expand the overflowing draft immediately"
                );
                assert!(
                    composer.flip_morph.is_some(),
                    "resize must retain the height/controls morph"
                );
                assert!(composer.last_target_height > COMPACT_TOTAL_HEIGHT);
                composer.morph_clock -= Duration::from_secs(1);
            })
            .unwrap();
        draw(cx);
        handle
            .read_with(cx, |composer, _| {
                let surface = composer.surface_bounds.get().unwrap();
                let model = composer.model_bounds.get().unwrap();
                let left = surface.left() + px(1.0 + 12.0 + 28.0 + ACTION_UTILITY_GAP);
                assert!(
                    (f32::from(model.left() - left)).abs() <= 1.0,
                    "expanded controls must finish on the left"
                );
            })
            .unwrap();

        handle
            .update(cx, |composer, window, cx| {
                composer.set_available_width(1232.0, cx);
                window.resize(size(px(1232.0), px(800.0)));
            })
            .unwrap();
        for _ in 0..3 {
            draw(cx);
        }
        handle
            .update(cx, |composer, _, _| {
                assert!(
                    composer.expanded_mode,
                    "widening must wait for resize to settle before collapsing"
                );
                composer.width_changed_at =
                    Some(Instant::now() - Duration::from_millis(RESIZE_SETTLE_MS + 1));
            })
            .unwrap();
        draw(cx);
        handle
            .read_with(cx, |composer, cx| {
                assert!(!composer.expanded_mode);
                assert!(composer.flip_morph.is_some());
                assert_eq!(composer.last_target_height, COMPACT_TOTAL_HEIGHT);
                assert_eq!(composer.input, input);
                assert_eq!(input.read(cx).text(), draft);
                assert_eq!(input.read(cx).selected_range, 2..8);
            })
            .unwrap();
    }

    #[gpui::test]
    fn composer_padding_and_file_prompt_restore_focus(cx: &mut gpui::TestAppContext) {
        let (dir, handle) = composer_focus_window(cx);
        let image_path = dir.path().join("attachment.png");
        let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aF1cAAAAASUVORK5CYII=").unwrap();
        std::fs::write(&image_path, png).unwrap();
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.blur();
            window.draw(cx).clear();
            let bounds = input.read(cx).last_bounds.unwrap();
            // Click padding immediately left of the actual editor.
            let position = point(bounds.left() - px(4.0), bounds.center().y);
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            assert!(input.read(cx).focus_handle.is_focused(window));
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    ..Default::default()
                }),
                cx,
            );
        })
        .unwrap();
        for accepted in [false, true] {
            handle
                .update(cx, |composer, window, cx| {
                    composer
                        .input
                        .update(cx, |input, cx| input.set_text("Keep this draft", cx));
                    window.blur();
                    composer.open_file_picker(cx);
                })
                .unwrap();
            assert!(cx.did_prompt_for_paths());
            let path = image_path.clone();
            cx.simulate_path_prompt_response(move |_| accepted.then(|| vec![path]));
            cx.run_until_parked();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
                assert!(input.read(cx).focus_handle.is_focused(window));
                assert_eq!(input.read(cx).text(), "Keep this draft");
            })
            .unwrap();
            assert_eq!(
                handle
                    .read_with(cx, |composer, _| composer.staged().len())
                    .unwrap(),
                usize::from(accepted)
            );
        }
        // The same staging path handles external file drops.
        handle
            .update(cx, |composer, window, cx| {
                window.blur();
                composer.add_paths(vec![image_path], cx);
            })
            .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear();
            assert!(input.read(cx).focus_handle.is_focused(window));
        })
        .unwrap();
    }

    #[gpui::test]
    fn composer_picker_escape_restores_focus_but_click_away_does_not(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = composer_focus_window(cx);
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        for escape in [true, false] {
            handle
                .update(cx, |composer, window, cx| {
                    composer
                        .pickers
                        .update(cx, |pickers, cx| pickers.open_model_menu(window, cx));
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
                assert!(!input.read(cx).focus_handle.is_focused(window));
            })
            .unwrap();
            if escape {
                cx.simulate_keystrokes(handle.into(), "escape");
            } else {
                cx.update_window(handle.into(), |_, window, cx| {
                    window.dispatch_event(
                        gpui::PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Left,
                            position: point(px(5.0), window.viewport_size().height - px(1.0)),
                            click_count: 1,
                            ..Default::default()
                        }),
                        cx,
                    );
                })
                .unwrap();
            }
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
                assert_eq!(input.read(cx).focus_handle.is_focused(window), escape);
            })
            .unwrap();
        }
    }

    #[gpui::test]
    fn inputs_release_focus_on_click_away(cx: &mut gpui::TestAppContext) {
        struct Inputs(Vec<Entity<ComposerInput>>);
        impl Render for Inputs {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().size_full().flex().flex_col().children(
                    self.0
                        .iter()
                        .map(|input| div().w(px(200.0)).child(input.clone())),
                )
            }
        }
        cx.update(|cx| cx.set_global(Theme::dark()));
        let host = cx.add_window(|_, cx| {
            Inputs(vec![
                cx.new(|cx| ComposerInput::new("Composer", cx)),
                cx.new(|cx| ComposerInput::new("URL", cx).with_single_line()),
            ])
        });
        cx.run_until_parked();
        cx.update_window(host.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        let inputs = host.read_with(cx, |host, _| host.0.clone()).unwrap();
        // Visit both fields and return to the first: click-away capture must
        // never clear focus acquired by the clicked input during bubbling.
        for index in [0, 1, 0] {
            let position =
                inputs[index].read_with(cx, |input, _| input.last_bounds.unwrap().center());
            cx.update_window(host.into(), |_, window, cx| {
                window.dispatch_event(
                    gpui::PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        click_count: 1,
                        ..Default::default()
                    }),
                    cx,
                );
                assert!(inputs[index].read(cx).focus_handle.is_focused(window));
                window.dispatch_event(
                    gpui::PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
            })
            .unwrap();
        }
        cx.update_window(host.into(), |_, window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(400.0), px(300.0)),
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            assert!(window.focused(cx).is_none());
        })
        .unwrap();
    }

    #[test]
    fn workspace_commands_preserve_native_commands_and_avoid_collisions() {
        let native = invocation_candidates(
            vec![
                SlashCommand {
                    name: "model".into(),
                    description: "Native model command".into(),
                    input_hint: Some("model id".into()),
                },
                SlashCommand {
                    name: "zeron:model".into(),
                    description: "Plugin command".into(),
                    input_hint: None,
                },
            ],
            vec![],
        );
        let rows = with_workspace_commands(native, true);
        assert_eq!(rows.len(), 11);
        assert!(rows[0].workspace_command.is_none());
        assert_eq!(rows[0].input_hint.as_deref(), Some("model id"));
        assert_eq!(workspace_command_for_text("/model", &rows), None);
        assert_eq!(workspace_command_for_text("/zeron:model", &rows), None);
        assert_eq!(
            workspace_command_for_text("/zeron:zeron:model", &rows),
            Some(WorkspaceCommand::Model)
        );
        assert_eq!(with_workspace_commands(rows, true).len(), 11);
        let draft_rows = with_workspace_commands(vec![], false);
        assert_eq!(draft_rows.len(), 4);
        assert_eq!(workspace_command_for_text("/diff", &draft_rows), None);
        assert_eq!(
            workspace_command_for_text("/model  ", &draft_rows),
            Some(WorkspaceCommand::Model)
        );
        for literal in [
            "    /model",
            "`/model`",
            "```\n/model\n```",
            "please /model",
            "/model extra",
            "/model/path",
        ] {
            assert_eq!(
                workspace_command_for_text(literal, &draft_rows),
                None,
                "{literal:?}"
            );
        }
    }

    #[gpui::test]
    fn workspace_command_submission_is_local_and_preserves_drafts(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        let actions = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let captured = actions.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&composer, move |_, event: &ComposerEvent, _| {
                if let ComposerEvent::WorkspaceCommand(command) = event {
                    captured.borrow_mut().push(*command);
                }
            })
        });
        composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("/model", cx));
            composer.update_slash("/model", 6, cx);
            composer.on_submit(cx);
            assert!(composer.input.read(cx).text().is_empty());
            assert!(
                composer.failure.is_none(),
                "must not require an engine or create a run"
            );
        });
        assert_eq!(&*actions.borrow(), &[WorkspaceCommand::Model]);
        composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("/model keep this draft", cx));
            composer.update_slash("/model keep this draft", 6, cx);
            composer.accept_slash(cx);
            assert_eq!(composer.input.read(cx).text(), "keep this draft");
            assert!(composer.failure.is_none());
        });
        assert_eq!(actions.borrow().len(), 2);
    }

    #[gpui::test]
    fn inline_slash_completion_distinguishes_actions_from_references(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, window, cx| {
                let attachment = attachments::stage_png_bytes("draft.png".into(), Vec::new());
                let attachment_id = attachment.id.clone();
                composer
                    .attachments
                    .insert(composer.current_key.clone(), vec![attachment]);
                composer.editing_queued = Some("queued-draft".into());
                for draft in [
                    "café /model keep this",
                    "first\n/model\nlast",
                    "(/model) text",
                ] {
                    let cursor = draft.find("/model").unwrap() + 6;
                    composer
                        .input
                        .update(cx, |input, cx| input.set_text(draft, cx));
                    composer.update_slash(draft, cursor, cx);
                    assert!(composer.slash.token.is_some());
                    composer.accept_slash(cx);
                    let expected = draft.replace("/model ", "").replace("/model", "");
                    assert_eq!(composer.input.read(cx).text(), expected);
                    assert!(composer.failure.is_none());
                    assert_eq!(composer.staged()[0].id, attachment_id);
                    assert_eq!(composer.editing_queued.as_deref(), Some("queued-draft"));
                    composer.input.update(cx, |input, cx| {
                        input.undo(&Undo, window, cx);
                        assert_eq!(input.text(), draft, "action removal is undoable");
                    });
                }
                let skill = zeron_proto::invocation::Invocation::Skill {
                    name: "review".into(),
                    path: "/repo/SKILL.md".into(),
                    command: None,
                };
                for invocation in [
                    zeron_proto::invocation::Invocation::Command {
                        name: "review".into(),
                    },
                    skill,
                ] {
                    let draft = "café /rev after";
                    composer
                        .input
                        .update(cx, |input, cx| input.set_text(draft, cx));
                    composer.update_slash(draft, "café /rev".len(), cx);
                    composer.slash_cache.insert(
                        composer.slash.context.clone(),
                        vec![InvocationCandidate {
                            name: "review".into(),
                            description: String::new(),
                            input_hint: None,
                            workspace_command: None,
                            invocation: invocation.clone(),
                        }],
                    );
                    composer.refilter_slash(cx);
                    composer.accept_slash(cx);
                    assert_eq!(
                        composer.input.read(cx).text(),
                        format!(
                            "café {} after",
                            match &invocation {
                                zeron_proto::invocation::Invocation::Command { .. } =>
                                    "/review".to_string(),
                                _ => invocation.link(),
                            }
                        )
                    );
                    assert!(composer.failure.is_none());
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn projectless_composer_allows_send_and_enter_submission(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        // A device with no projects is a valid home-directory target too.
        composer.update(cx, |composer, cx| assert!(!composer.send_blocked(cx)));
        state.update(cx, |state, cx| state.select_space(None, cx));
        composer.update(cx, |composer, cx| {
            composer.input.update(cx, |input, cx| {
                input.set_text("Hello without a project", cx);
            });
            assert_eq!(composer.button_mode(cx), SendButtonMode::Send);
            assert!(
                !composer.send_blocked(cx),
                "The Send button must be enabled without a project"
            );
            composer.on_submit(cx);
            // With no engine attached, reaching the normal send error proves
            // Enter dispatched instead of silently stopping at the UI gate.
            assert_eq!(composer.failure.as_deref(), Some("Engine not connected"));
            composer.queue_edit_finishing = true;
            assert!(
                composer.send_blocked(cx),
                "Pending edits must still block submission"
            );
        });
    }

    #[gpui::test]
    fn submission_preserves_literal_command_and_skill_indentation(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let skill = zeron_proto::invocation::Invocation::Skill {
            name: "review".into(),
            path: "/skills/review/SKILL.md".into(),
            command: None,
        }
        .link();
        for editing_queue in [false, true] {
            for raw in [
                "    /review".to_string(),
                "\t/compact".into(),
                "\u{a0}/review".into(),
                format!("    {skill}"),
                "Keep this hard break  ".into(),
                " \t\n ".into(),
            ] {
                let (out, mut requests) = tokio::sync::mpsc::channel::<String>(64);
                let (_replies, inbound) = tokio::sync::mpsc::channel::<String>(64);
                let state = cx.new(|_| AppState::new());
                state.update(cx, |state, _| {
                    state.set_test_engine(crate::state::EngineHandle::from_test_client(
                        zeron_rpc::RpcClient::new(out, inbound),
                    ));
                    state.selected_chat = Some("literal-draft".into());
                });
                let composer = cx.new(|cx| Composer::new(state, cx));
                composer.update(cx, |composer, cx| {
                    composer
                        .input
                        .update(cx, |input, cx| input.set_text(&raw, cx));
                    if editing_queue {
                        composer.editing_queued = Some("queued-row".into());
                        composer.queue_edit_lease_id = Some("lease".into());
                        composer.queue_edit_chat_id = Some("literal-draft".into());
                        composer.queue_edit_host_device_id = Some("host".into());
                    }
                    composer.on_submit(cx);
                });
                cx.run_until_parked();
                let mut submitted = None;
                let mut discarded = false;
                while let Ok(frame) = requests.try_recv() {
                    let frame: zeron_rpc::ClientFrame = serde_json::from_str(&frame).unwrap();
                    if frame.method.as_deref() == Some(methods::FINISH_QUEUED_MESSAGE_EDIT) {
                        discarded = frame.params["action"] == "discard";
                        submitted = frame.params["text"].as_str().map(str::to_owned);
                    }
                    if frame.method.as_deref() == Some(methods::QUEUE_COMMAND) {
                        submitted = Some(
                            frame.params["command"]["request"]["prompt"]
                                .as_str()
                                .unwrap()
                                .to_owned(),
                        );
                    }
                }
                if raw.trim().is_empty() {
                    assert!(submitted.is_none(), "whitespace alone must not submit");
                    assert_eq!(discarded, editing_queue);
                } else {
                    let submitted = submitted.expect("submission must reach the engine RPC");
                    assert_eq!(submitted, raw);
                    assert!(zeron_proto::invocation::leading_command(&submitted).is_none());
                    assert!(zeron_proto::invocation::invocation_links(&submitted).is_empty());
                }
            }
        }
    }

    /// Issue #406: Enter submits — it must never stop a run. Stop mode only
    /// exists on a live run with an EMPTY composer, so a habitual
    /// double-Enter after sending interrupted the just-dispatched prompt
    /// and the agent ate it silently. Keyboard stop is the Esc setting's
    /// job; Enter on an empty composer is a no-op.
    #[gpui::test]
    fn enter_on_empty_composer_during_a_live_run_never_interrupts(cx: &mut gpui::TestAppContext) {
        // RpcClient::new spawns its reader on tokio — give the test a reactor.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        // The client's write end: any RPC Enter dispatches lands here.
        let (out, mut server_in) = tokio::sync::mpsc::channel::<String>(16);
        let (_server_out, inbound) = tokio::sync::mpsc::channel::<String>(16);
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, _| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::RpcClient::new(out, inbound),
            ));
            state.selected_chat = Some("c".into());
            // A send in flight reads as Working — the double-Enter window.
            state.begin_pending_send("c", "m1", chrono::Utc::now());
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            assert_eq!(composer.button_mode(cx), SendButtonMode::Stop);
            composer.on_submit(cx);
            // interrupt_chat marks the chat before its RPC even flies.
            assert!(!composer.interrupting.contains("c"));
        });
        // If Enter had dispatched an interrupt, the spawned call would have
        // written its frame into the channel by the time the executor parks.
        cx.run_until_parked();
        assert!(server_in.try_recv().is_err());
    }

    #[gpui::test]
    fn inline_reference_text_preserves_selection_and_ime(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        let raw = format!(
            "{} {} {}",
            local_file_link("src/main.rs", false),
            zeron_proto::invocation::Invocation::Skill {
                command: None,
                name: "review".into(),
                path: "/repo/SKILL.md".into(),
            }
            .link(),
            zeron_proto::invocation::Invocation::Command {
                name: "help".into()
            }
            .link(),
        );
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        input.update(cx, |input, cx| input.set_text(&raw, cx));
        cx.update_window(handle.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        input.update(cx, |input, _| {
            assert_eq!(input.projection.mentions.len(), 3);
            for (mention, display) in &input.projection.mentions {
                let label = &input.projection.display[display.clone()];
                assert!(label.starts_with(&format!("{MENTION_SIDE_PAD}{}", mention.prefix)));
                assert!(label.ends_with(MENTION_SIDE_PAD));
                let shell = input.bounds_for_display_range(display.clone());
                assert_eq!(shell.len(), 1);
                assert!(shell[0].size.width > px(0.0));
                let start = display.start + MENTION_SIDE_PAD.len();
                assert_eq!(input.projection.display_to_raw(start), mention.range.start);
                assert_eq!(
                    input
                        .projection
                        .normalize_range(mention.range.start + 1..mention.range.end - 1),
                    mention.range,
                );
            }
            let display = input.projection.display.clone();
            input.marked_range = Some(raw.len()..raw.len());
            input.refresh_projection();
            assert_eq!(
                input.projection.display, display,
                "IME must retain reference text"
            );
            assert_eq!(input.text(), raw);
        });
    }

    #[gpui::test]
    fn inline_chips_wrap_as_units(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle.update(cx, |composer, window, cx| {
            let file = local_file_link("src/composer.rs", false);
            let skill = zeron_proto::invocation::Invocation::Skill { name: "review-changes".into(), path: "/repo/SKILL.md".into(), command: None }.link();
            let raw = format!("Review {file} with {skill} and enough trailing prose to need more than one additional row of wrapping.");
            composer.input.update(cx, |input, cx| {
                input.set_text(&raw, cx);
                for width in [240., 300., 360.] {
                    input.layout_text(px(width), &window.text_style(), window, cx);
                    for (_, display) in &input.projection.mentions {
                        assert_eq!(input.bounds_for_display_range(display.clone()).len(), 1, "chip split at {width}");

                    }
                }
                assert_eq!(input.text(), raw);
            });
        }).unwrap();
    }

    #[gpui::test]
    fn crlf_navigation_deletion_and_newlines_keep_the_pair_intact(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            for (first, continued) in [
                ("plain café", ""),
                ("- item", "- "),
                ("- [x] item", "- [ ] "),
            ] {
                let raw = format!("{first}\r\nnext");
                input.set_text(&raw, cx);
                input.move_to(0, cx);
                input.end(&End, window, cx);
                assert_eq!(input.cursor_offset(), first.len());
                input.layout_text(px(600.0), &window.text_style(), window, cx);
                assert_eq!(
                    input.caret_for_point(point(px(590.0), px(1.0))).0,
                    first.len()
                );
                assert_eq!(
                    input.selection_unit(PressIntent::Line, 0),
                    0..first.len() + 2
                );
                input.move_to(0, cx);
                input.select_end(&SelectEnd, window, cx);
                assert_eq!(input.selected_range, 0..first.len());
                input.move_to(0, cx);
                input.end(&End, window, cx);
                input.backspace(&Backspace, window, cx);
                let previous = first.grapheme_indices(true).last().unwrap().0;
                assert_eq!(input.text(), format!("{}\r\nnext", &first[..previous]));
                input.undo(&Undo, window, cx);
                input.move_to(first.len(), cx);
                input.delete(&Delete, window, cx);
                assert_eq!(input.text(), format!("{first}next"));
                input.undo(&Undo, window, cx);
                input.move_to(first.len() + 2, cx);
                input.backspace(&Backspace, window, cx);
                assert_eq!(input.text(), format!("{first}next"));
                input.undo(&Undo, window, cx);
                input.move_to(0, cx);
                input.end(&End, window, cx);
                input.newline(&Newline, window, cx);
                assert_eq!(input.text(), format!("{first}\r\n{continued}\r\nnext"));
                input.undo(&Undo, window, cx);
                input.move_to(2, cx);
                input.delete_to_line_end(&DeleteToLineEnd, window, cx);
                assert_eq!(input.text(), format!("{}\r\nnext", &first[..2]));
                input.undo(&Undo, window, cx);
                assert_eq!(input.text(), raw);
            }
            input.set_text("\r\nnext", cx);
            input.move_to(0, cx);
            input.end(&End, window, cx);
            assert_eq!(input.cursor_offset(), 0);
            input.backspace(&Backspace, window, cx);
            assert_eq!(input.text(), "\r\nnext");
            input.delete(&Delete, window, cx);
            assert_eq!(input.text(), "next");
        });
    }

    #[gpui::test]
    fn soft_wrap_carets_preserve_pointer_keyboard_and_ime_affinity(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle.update(cx, |composer, window, cx| {
            composer.input.update(cx, |input, cx| {
                input.set_text("Café and emoji 🦀 wrap across several visual rows with enough text to keep moving downward.", cx);
                input.layout_text(px(140.0), &window.text_style(), window, cx);
                let line = &input.last_lines[0];
                assert!(line.wrap_boundaries().len() >= 2);
                let boundary = line.wrap_boundaries()[0];
                let offset = line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                let upstream = input.point_for_display_index_with_affinity(offset, CaretAffinity::Upstream).unwrap();
                let downstream = input.point_for_display_index(offset).unwrap();
                assert_eq!(upstream.y, px(0.0));
                assert_eq!(downstream, point(px(0.0), input.line_height));
                assert!(upstream.x > downstream.x);
                let raw_offset = input.projection.display_to_raw(offset);
                assert_eq!(input.caret_for_point(upstream + point(px(20.0), px(1.0))), (raw_offset, CaretAffinity::Upstream));
                assert_eq!(input.caret_for_point(downstream + point(px(0.0), px(1.0))), (raw_offset, CaretAffinity::Downstream));
                let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(140.0), px(200.0)));
                input.last_bounds = Some(bounds);
                for (local, affinity) in [(upstream, CaretAffinity::Upstream), (downstream, CaretAffinity::Downstream)] {
                    input.on_mouse_down(&MouseDownEvent {
                        button: MouseButton::Left,
                        position: bounds.origin + local + point(px(0.0), px(1.0)),
                        click_count: 1,
                        ..Default::default()
                    }, window, cx);
                    assert_eq!(input.cursor_offset(), raw_offset);
                    assert_eq!(input.caret_affinity, affinity);
                    assert_eq!(input.cursor_point(), Some(local));
                    let utf16 = input.offset_to_utf16(raw_offset);
                    let ime = input.bounds_for_range(utf16..utf16, bounds, window, cx).unwrap();
                    assert_eq!(ime.origin, bounds.origin + local);
                    input.replace_text_in_range(None, "x", window, cx);
                    input.undo(&Undo, window, cx);
                    input.layout_text(px(140.0), &window.text_style(), window, cx);
                    assert_eq!(input.cursor_offset(), raw_offset);
                    assert_eq!(input.caret_affinity, affinity);
                    assert_eq!(input.cursor_point(), Some(local));
                }
                input.move_to(0, cx);
                input.down(&Down, window, cx);
                assert_eq!(input.cursor_point(), Some(downstream));
                input.down(&Down, window, cx);
                assert_eq!(input.cursor_point().unwrap().y, input.line_height * 2);
                input.up(&Up, window, cx);
                assert_eq!(input.cursor_point(), Some(downstream));
                input.up(&Up, window, cx);
                assert_eq!(input.cursor_offset(), 0);
                input.select_down(&SelectDown, window, cx);
                assert_eq!(input.selected_range, 0..raw_offset);
                assert_eq!(input.cursor_point(), Some(downstream));
                input.select_up(&SelectUp, window, cx);
                assert_eq!(input.selected_range, 0..0);
            });
        }).unwrap();
    }

    #[gpui::test]
    fn vertical_navigation_retains_column_across_short_and_wrapped_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        with_composer_input(cx, |input, window, cx| {
            let long = "abcdefghijklmnopqrstuvwx";
            let raw = format!("{long}\nx\n{long}");
            let last_start = long.len() + 3;
            for width in [1000.0, 140.0] {
                input.set_text(&raw, cx);
                input.layout_text(px(width), &window.text_style(), window, cx);
                input.move_to(7, cx);
                let start = input.cursor_point().unwrap();
                let target = input.point_for_index(last_start + 7).unwrap();
                let rows =
                    (f32::from(target.y - start.y) / f32::from(input.line_height)).round() as usize;
                assert!(rows >= 2);
                for _ in 0..rows {
                    input.down(&Down, window, cx);
                }
                assert_eq!(
                    input.cursor_offset(),
                    last_start + 7,
                    "column lost at width {width}"
                );
                assert_eq!(input.preferred_column, Some(start.x));
                for _ in 0..rows {
                    input.up(&Up, window, cx);
                }
                assert_eq!(input.cursor_offset(), 7);
                for _ in 0..rows {
                    input.select_down(&SelectDown, window, cx);
                }
                assert_eq!(input.selected_range, 7..last_start + 7);
                for _ in 0..rows {
                    input.select_up(&SelectUp, window, cx);
                }
                assert_eq!(input.selected_range, 7..7);
                input.right(&Right, window, cx);
                assert_eq!(input.preferred_column, None);
                input.down(&Down, window, cx);
                assert!(input.preferred_column.is_some());
                input.replace_text_in_range(None, "é", window, cx);
                assert_eq!(input.preferred_column, None);
                input.undo(&Undo, window, cx);
                assert_eq!(input.text(), raw);
            }
        });
    }

    #[gpui::test]
    fn wrapped_rich_lines_keep_caret_and_range_geometry_aligned(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle.update(cx, |composer, window, cx| {
            composer.input.update(cx, |input, cx| {
                let file = local_file_link("src/composer.rs", false);
                let raw = format!("- First **bold** item with café and enough words to wrap over several visual rows\n- Second item with {file} and more trailing content to wrap\n\nDone");
                input.set_text(&raw, cx);
                input.layout_text(px(190.0), &window.text_style(), window, cx);
                let mut y_offset = px(0.0);
                let mut checked = 0;
                for (line_ix, line) in input.last_lines.iter().enumerate() {
                    let line_start = input.line_starts[line_ix];
                    for (row, boundary) in line.wrap_boundaries().iter().enumerate() {
                        let local_offset = line.runs()[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                        let display_offset = line_start + local_offset;
                        let raw_offset = input.projection.display_to_raw(display_offset);
                        let downstream = input.point_for_display_index(display_offset).unwrap();
                        assert_eq!(downstream.y, y_offset + input.line_height * (row + 1));
                        assert_eq!(downstream.x, input.line_indents[line_ix]);
                        assert_eq!(input.caret_for_point(downstream + point(px(0.0), px(1.0))), (raw_offset, CaretAffinity::Downstream));
                        let upstream = input.point_for_display_index_with_affinity(display_offset, CaretAffinity::Upstream).unwrap();
                        assert_eq!(upstream.y + input.line_height, downstream.y);
                        assert_eq!(input.caret_for_point(upstream + point(px(20.0), px(1.0))), (raw_offset, CaretAffinity::Upstream));
                        let end = line_start + line.text[local_offset..].char_indices().nth(1).map_or(line.len(), |(offset, _)| local_offset + offset);
                        let boxes = input.bounds_for_display_range(display_offset..end);
                        assert!(!boxes.is_empty());
                        assert_eq!(boxes[0].origin, downstream);
                        checked += 1;
                    }
                    y_offset += line.size(input.line_height).height;
                }
                assert!(checked >= 3);
                assert_eq!(input.text(), raw);
            });
        }).unwrap();
    }

    #[test]
    fn hidden_markdown_hit_testing_targets_visible_text() {
        let raw = "***café***\nactive";
        let projection = TextProjection::rich(raw, Some(12..raw.len()));
        assert_eq!(projection.display_to_raw(0), 3);
        assert_eq!(
            &raw[projection.display_to_raw(0)..projection.display_to_raw(1)],
            "c"
        );
        for (display, _) in projection.display.char_indices() {
            assert!(raw.is_char_boundary(projection.display_to_raw(display)));
        }
    }

    #[test]
    fn long_chip_labels_keep_graphemes_and_full_source_identity() {
        let label = "ä".repeat(48) + ".rs";
        let shown = compact_chip_label(&label);
        assert_eq!(shown.graphemes(true).count(), 31);
        assert!(shown.ends_with(".rs"));
        let raw = local_file_link(&format!("src/{label}"), false);
        let projected = TextProjection::rich(&raw, Some(0..raw.len()));
        assert_eq!(projected.mentions[0].0.range, 0..raw.len());
        assert!(projected.mentions[0].0.path.ends_with(&label));
        assert!(projected.display.contains('…'));
        assert_eq!(projected.normalize_range(1..raw.len() - 1), 0..raw.len());
    }

    #[test]
    fn chip_labels_stay_compact_for_repeats_and_distinct_after_truncation() {
        let path = "src/components/field.rs";
        let raw = format!(
            "{} {}",
            local_file_link(path, false),
            local_file_link(path, false)
        );
        assert_eq!(
            mention_display_labels(&file_mention_links(&raw)),
            ["field.rs", "field.rs"]
        );
        let left = format!("{}alpha{}", "x".repeat(20), "tail".repeat(8));
        let right = format!("{}bravo{}", "x".repeat(20), "tail".repeat(8));
        assert_eq!(compact_chip_label(&left), compact_chip_label(&right));
        let shown = compact_chip_labels(&[left.clone(), right, left]);
        assert_ne!(shown[0], shown[1]);
        assert_eq!(shown[0], shown[2]);
    }

    #[test]
    fn pasted_references_only_recognize_complete_prose_tokens() {
        let text = "Use @README.md, ($framer) and /review. café@example.com $12.50 /usr/bin x/review `@code.rs` \\@escaped.rs [@label](url) https://host/@remote";
        let tokens = pasted_reference_tokens(text, 0..text.len());
        assert_eq!(
            tokens
                .iter()
                .map(|(prefix, token)| (*prefix, token.query.as_str()))
                .collect::<Vec<_>>(),
            vec![('@', "README.md"), ('$', "framer"), ('/', "review")]
        );
        for text in [
            "```\n@README.md\n```",
            "    @README.md",
            "<user@example.com>",
            "$HOME/bin",
            "[x](https://host/@file)",
        ] {
            assert!(
                pasted_reference_tokens(text, 0..text.len()).is_empty(),
                "{text}"
            );
        }
        let text = "prefix@README.md suffix";
        assert!(pasted_reference_tokens(text, 6..15).is_empty());
        let text = "@README.md";
        assert!(
            pasted_reference_tokens(text, 0..5).is_empty(),
            "partial pasted token"
        );
        // Invalid candidates consume the same work budget as accepted ones.
        // Do not repeatedly parse a large pasted block of literal examples.
        let text = format!("```\n{}\n```\n$review", "@example ".repeat(4_000));
        assert!(pasted_reference_tokens(&text, 0..text.len()).is_empty());
    }

    #[gpui::test]
    fn projectless_paste_discovers_invocations_and_rejects_changed_targets(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        crate::settings::composer::ComposerDefaults {
            harness: Some(HarnessId::Codex),
            ..Default::default()
        }
        .save(directory.path())
        .unwrap();
        cx.update(|cx| crate::settings::init(Default::default(), directory.path(), cx));
        for change_target in [false, true] {
            let (out, mut requests) = tokio::sync::mpsc::channel(64);
            let (replies, inbound) = tokio::sync::mpsc::channel(64);
            let state = cx.new(|_| AppState::new());
            state.update(cx, |state, _| {
                state.data_dir = Some(directory.path().to_path_buf());
                state.selected_device = Some("peer".into());
                state.devices = vec![
                    serde_json::from_value(serde_json::json!({
                        "id": "peer", "name": "Peer", "platform": "linux",
                        "capabilities": [capabilities::COMPOSER_REFERENCES_V1]
                    }))
                    .unwrap(),
                ];
                state.set_test_engine(crate::state::EngineHandle::from_test_client(
                    zeron_rpc::RpcClient::new(out, inbound),
                ));
            });
            let composer = cx.new(|cx| Composer::new(state.clone(), cx));
            let raw = "$review /compact @README.md";
            composer.update(cx, |composer, cx| {
                assert!(composer.file_search_params("", cx).is_none());
                composer
                    .input
                    .update(cx, |input, cx| input.set_text(raw, cx));
                let revision = composer.input.read(cx).edit_revision;
                composer.resolve_pasted_references(0..raw.len(), revision, cx);
            });
            cx.run_until_parked();
            if change_target {
                state.update(cx, |state, _| state.selected_device = Some("other".into()));
            }
            let mut methods = Vec::new();
            while let Ok(frame) = requests.try_recv() {
                let frame: zeron_rpc::ClientFrame = serde_json::from_str(&frame).unwrap();
                assert_eq!(frame.params["targetDeviceId"], "peer");
                assert!(frame.params.get("cwd").is_none());
                let value = match frame.method.as_deref() {
                    Some(methods::LIST_COMMANDS) => {
                        serde_json::json!([{"name":"compact", "description":"Compact"}])
                    }
                    Some(methods::LIST_SKILLS) => {
                        serde_json::json!([{"name":"review", "path":"/skills/SKILL.md", "description":"Review", "enabled":true}])
                    }
                    method => panic!("unexpected projectless request: {method:?}"),
                };
                methods.push(frame.method.unwrap());
                replies
                    .try_send(
                        serde_json::to_string(&zeron_rpc::ServerFrame {
                            id: frame.id,
                            ok: Some(value),
                            ..Default::default()
                        })
                        .unwrap(),
                    )
                    .unwrap();
            }
            methods.sort();
            let mut expected = vec![methods::LIST_COMMANDS, methods::LIST_SKILLS];
            expected.sort();
            assert_eq!(methods, expected);
            runtime.block_on(async { tokio::task::yield_now().await });
            cx.run_until_parked();
            composer.read_with(cx, |composer, cx| {
                let text = composer.input.read(cx).text();
                if change_target {
                    assert_eq!(text, raw);
                } else {
                    assert_eq!(zeron_proto::invocation::invocation_links(text).len(), 2);
                    assert!(text.ends_with(" @README.md"));
                }
            });
        }
    }

    /// A hand-started side chat writes nothing until its first send, which
    /// mints it (with its parent link) before the run and only then opens
    /// its doc.
    #[gpui::test]
    fn unsaved_side_chat_is_created_by_its_first_send(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| crate::settings::init(Default::default(), directory.path(), cx));
        let (out, mut requests) = tokio::sync::mpsc::channel::<String>(256);
        let (replies, inbound) = tokio::sync::mpsc::channel::<String>(256);
        let parent = cx.new(|_| AppState::new());
        parent.update(cx, |state, _| {
            state.data_dir = Some(directory.path().to_path_buf());
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::RpcClient::new(out, inbound),
            ));
        });
        let chat: zeron_proto::Chat = serde_json::from_value(serde_json::json!({
            "id": "side", "parentChatId": "main", "deviceId": "local", "cwd": "/tmp/main",
            "archived": false, "createdAt": chrono::Utc::now(),
        }))
        .unwrap();
        let side = cx.new(|cx| AppState::side_chat_state(&parent, chat, true, cx));
        let mut drain = || {
            let mut frames = Vec::new();
            while let Ok(frame) = requests.try_recv() {
                frames.push(serde_json::from_str::<zeron_rpc::ClientFrame>(&frame).unwrap());
            }
            frames
        };
        let touches_side = |frame: &zeron_rpc::ClientFrame| frame.params["chatId"] == "side";
        cx.run_until_parked();
        assert!(!drain().iter().any(touches_side));

        let composer = cx.new(|cx| Composer::new(side.clone(), cx));
        composer.update(cx, |composer, cx| {
            composer
                .input
                .update(cx, |input, cx| input.set_text("hello", cx));
            composer.on_submit(cx);
        });
        cx.run_until_parked();
        let sent: Vec<_> = drain().into_iter().filter(touches_side).collect();
        let [create] = sent.as_slice() else {
            panic!("expected only createChat, got {sent:?}");
        };
        assert_eq!(create.method.as_deref(), Some(methods::MUTATE));
        assert_eq!(create.params["op"], "createChat");
        assert_eq!(create.params["parentChatId"], "main");
        assert_eq!(create.params["cwd"], "/tmp/main");
        replies
            .try_send(
                serde_json::to_string(&zeron_rpc::ServerFrame {
                    id: create.id,
                    ok: Some(serde_json::json!({})),
                    ..Default::default()
                })
                .unwrap(),
            )
            .unwrap();
        runtime.block_on(async { tokio::task::yield_now().await });
        cx.run_until_parked();
        let after: Vec<_> = drain().into_iter().filter(touches_side).collect();
        let called = |method: &str| after.iter().any(|f| f.method.as_deref() == Some(method));
        assert!(called(methods::WATCH_DOC_MESSAGES), "{after:?}");
        assert!(called(methods::QUEUE_COMMAND), "{after:?}");
        assert!(!after.iter().any(|f| f.params["op"] == "createChat"));
        assert!(!side.read_with(cx, |state, _| state.side_chat_unsaved()));
    }

    #[gpui::test]
    fn pasted_reference_resolution_is_one_undo_step_and_rejects_stale_results(
        cx: &mut gpui::TestAppContext,
    ) {
        with_composer_input(cx, |input, window, cx| {
            input.set_text("Before ", cx);
            cx.write_to_clipboard(ClipboardItem::new_string("@README.md".into()));
            input.paste(&Paste, window, cx);
            let original = input.text().to_owned();
            let revision = input.edit_revision;
            input.layout_text(px(400.0), &window.text_style(), window, cx);
            assert_eq!(
                input.edit_revision, revision,
                "layout must not invalidate discovery"
            );
            let replacement = vec![(7..original.len(), local_file_link("README.md", false))];
            input.apply_pasted_references(
                &original,
                original.len(),
                revision,
                replacement.clone(),
                cx,
            );
            assert_eq!(input.projection.mentions.len(), 1);
            let canonical = input.text().to_owned();
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "Before ");
            input.redo(&Redo, window, cx);
            assert_eq!(input.text(), canonical);
            input.set_text(&original, cx);
            input.apply_pasted_references(&original, original.len(), revision, replacement, cx);
            assert_eq!(
                input.text(),
                original,
                "stale discovery cannot change a restored draft"
            );
        });
    }

    #[gpui::test]
    fn clipboard_is_readable_outside_zeron_and_lossless_inside(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            let raw = format!(
                "**Check** {} with {}",
                local_file_link("src/café.rs", false),
                zeron_proto::invocation::Invocation::Skill {
                    name: "review".into(),
                    path: "/repo/SKILL.md".into(),
                    command: None
                }
                .link()
            );
            input.set_text(&raw, cx);
            input.select_all(&SelectAll, window, cx);
            input.copy(&Copy, window, cx);
            let copied = cx.read_from_clipboard().unwrap().text().unwrap();
            assert_eq!(
                copied,
                "**Check** [café.rs](src/caf%C3%A9.rs) with [$review](/repo/SKILL.md)"
            );
            input.set_text("", cx);
            input.paste(&Paste, window, cx);
            assert_eq!(input.text(), raw);
            assert_eq!(input.projection.mentions.len(), 2);
            input.set_text("x", cx);
            cx.write_to_clipboard(ClipboardItem::new_string("a".into()));
            input.paste(&Paste, window, cx);
            input.replace_text_in_range(None, "b", window, cx);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "xa");
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "x");
        });
    }

    #[gpui::test]
    fn dense_rich_draft_keeps_offsets_and_reuses_unchanged_layout(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            let raw = "- **café** _text_\n".repeat(1000) + "active";
            input.set_text(&raw, cx);
            input.layout_text(px(320.), &window.text_style(), window, cx);
            assert!(input.projection.display.starts_with("• café text\n"));
            assert_eq!(input.last_lines.len(), 1001);
            let mut previous = 0;
            for (offset, _) in raw.char_indices() {
                let display = input.projection.raw_to_display(offset);
                assert!(display >= previous && display <= input.projection.display.len());
                assert!(input.projection.display.is_char_boundary(display));
                previous = display;
            }
            for (offset, _) in input.projection.display.char_indices() {
                assert!(raw.is_char_boundary(input.projection.display_to_raw(offset)));
            }
            let rebuilt = input.layout_rebuilds;
            input.layout_text(px(320.), &window.text_style(), window, cx);
            assert_eq!(input.layout_rebuilds, rebuilt);
        });
    }

    #[gpui::test]
    fn rich_edit_state_is_current_before_the_next_layout(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            let draft = "**first**\n_second_\nlast";
            input.set_text(draft, cx);
            assert_eq!(input.projection.display, "first\nsecond\nlast");
            input.replace_text_in_range(None, "\n**next**", window, cx);
            assert_eq!(input.projection.display, "first\nsecond\nlast\n**next**");
            input.undo(&Undo, window, cx);
            assert_eq!(input.projection.display, "first\nsecond\nlast");
            input.redo(&Redo, window, cx);
            assert_eq!(input.projection.display, "first\nsecond\nlast\n**next**");
        });
    }

    #[gpui::test]
    fn ime_uses_replacement_relative_utf16_and_restores_markdown_on_unmark(
        cx: &mut gpui::TestAppContext,
    ) {
        with_composer_input(cx, |input, window, cx| {
            let before = "**bold**\n😀x";
            input.set_text(before, cx);
            let start = before.len() - 1;
            input.selected_range = start..before.len();
            input.selection_reversed = true;
            input.replace_and_mark_text_in_range(None, "あいう", Some(1..2), window, cx);
            assert_eq!(input.selected_range, start + 3..start + 6);
            assert!(!input.selection_reversed);
            assert_eq!(&input.text()[input.selected_range.clone()], "い");
            assert!(
                input.projection.display.starts_with("bold\n"),
                "composition must not reveal unrelated syntax"
            );
            input.layout_text(px(320.), &window.text_style(), window, cx);
            assert!(!input.needs_measure);
            input.unmark_text(window, cx);
            assert!(input.needs_measure);
            assert!(input.projection.display.starts_with("bold\n"));
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), before);
            assert_eq!(input.selected_range, start..before.len());
            assert!(input.selection_reversed);
        });
    }

    #[gpui::test]
    fn bulk_edits_and_following_typing_are_separate_undo_steps(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            input.set_text("", cx);
            input.replace_text_in_range(None, "paste", window, cx);
            input.replace_text_in_range(None, "d", window, cx);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "paste");
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "");
            input.set_text("@a", cx);
            input.replace_mention(0..2, "src/a.rs", false, cx);
            let chip = input.text().to_owned();
            input.replace_text_in_range(None, "x", window, cx);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), chip);
        });
    }

    #[gpui::test]
    fn inserting_references_preserves_surrounding_markdown_and_punctuation(
        cx: &mut gpui::TestAppContext,
    ) {
        with_composer_input(cx, |input, window, cx| {
            for raw in [
                "**see @src**",
                "_see @src_",
                "~~see @src~~",
                "See (@src)",
                "- @src\r\nnext",
            ] {
                let start = raw.find("@src").unwrap();
                let token = mention_token(raw, start + 4).unwrap();
                input.set_text(raw, cx);
                let file = local_file_link("src/main.rs", false);
                input.replace_mention(token.range, "src/main.rs", false, cx);
                assert_eq!(input.text(), raw.replacen("@src", &file, 1));
                assert_eq!(input.projection.mentions.len(), 1);
                input.undo(&Undo, window, cx);
                assert_eq!(input.text(), raw);
                let source = raw.replace("@src", "$review");
                input.set_text(&source, cx);
                let skill = zeron_proto::invocation::Invocation::Skill {
                    name: "review".into(),
                    path: "/repo/SKILL.md".into(),
                    command: None,
                }
                .link();
                let token = invocation_token(&source, start + 7, '$').unwrap();
                input.replace_plain_token(token.range, &skill, cx);
                assert_eq!(input.text(), source.replacen("$review", &skill, 1));
                assert_eq!(input.projection.mentions.len(), 1);
            }
            input.set_text("**selected**", cx);
            input.selected_range = 2..10;
            assert!(input.insert_dropped_mention("src/main.rs", false, cx));
            assert_eq!(
                input.text(),
                format!("**{}**", local_file_link("src/main.rs", false))
            );
            assert!(
                composer_markdown::faces(input.text())
                    .iter()
                    .any(|(_, face)| *face == composer_markdown::Face::Bold)
            );
        });
    }

    #[gpui::test]
    fn completion_rejects_paths_that_cannot_round_trip_as_chips(cx: &mut gpui::TestAppContext) {
        let input = cx.new(|cx| ComposerInput::new("Draft", cx));
        input.update(cx, |input, cx| {
            input.enable_mentions();
            input.set_text("See @src", cx);
            for path in [
                "src/a\nb.rs",
                "src/a\rb.rs",
                "src/a\tb.rs",
                "",
                "../secret",
                "https://example.com",
            ] {
                input.replace_mention(4..8, path, false, cx);
                assert_eq!(input.text(), "See @src", "{path:?}");
                assert!(input.undo_stack.is_empty());
            }
            input.replace_mention(4..8, "src/café [draft].rs", false, cx);
            assert_eq!(input.projection.mentions.len(), 1);
            assert_eq!(input.projection.mentions[0].0.path, "src/café [draft].rs");
        });
    }

    #[gpui::test]
    fn quoted_list_indentation_preserves_containers_and_selection(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            for original in ["> - café\n> - [x] done", "> > - café\r\n> > - [x] done"] {
                input.set_text(original, cx);
                input.selected_range = 0..original.len();
                input.selection_reversed = true;
                input.refresh_projection();
                assert!(input.indent_list(false, window, cx));
                let indented = original.replace("- ", "  - ");
                assert_eq!(input.text(), indented);
                assert_eq!(input.selected_range, 0..indented.len());
                assert!(input.selection_reversed);
                assert!(input.indent_list(true, window, cx));
                assert_eq!(input.text(), original);
                assert_eq!(input.selected_range, 0..original.len());
                assert!(input.selection_reversed);
                input.undo(&Undo, window, cx);
                assert_eq!(input.text(), indented);
                input.undo(&Undo, window, cx);
                assert_eq!(input.text(), original);
            }
            input.set_text("> ```\n> - literal\n> ```", cx);
            input.move_to(input.text().find("literal").unwrap(), cx);
            assert!(!input.indent_list(false, window, cx));
            input.set_text(
                ">     - literal code that wraps over several rows\n\nactive",
                cx,
            );
            input.layout_text(px(160.), &window.text_style(), window, cx);
            assert_eq!(input.line_indents[0], px(0.));
            assert!(input.projection.display.starts_with(">     - literal"));
        });
    }

    #[gpui::test]
    fn dropped_chip_replacement_rebuilds_same_length_layout(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            input.set_text(local_file_link("src/a.rs", false), cx);
            input.layout_text(px(320.), &window.text_style(), window, cx);
            let rebuilds = input.layout_rebuilds;
            input.selected_range = 0..input.content.len();
            assert!(input.insert_dropped_mention("src/b.rs", false, cx));
            assert!(input.needs_measure);
            input.layout_text(px(320.), &window.text_style(), window, cx);
            assert_eq!(input.layout_rebuilds, rebuilds + 1);
            assert!(input.projection.display.contains("b.rs"));
            input.set_text("    - code\n\nactive", cx);
            input.layout_text(px(160.), &window.text_style(), window, cx);
            assert_eq!(input.line_indents[0], px(0.));
            input.move_to(7, cx);
            assert!(!input.indent_list(false, window, cx));
            assert_eq!(input.text(), "    - code\n\nactive");
        });
    }

    #[gpui::test]
    fn multiline_selection_exposes_all_selected_markdown_source(cx: &mut gpui::TestAppContext) {
        let input = cx.new(|cx| ComposerInput::new("Draft", cx));
        input.update(cx, |input, cx| {
            input.enable_mentions();
            input.set_text("**first**\n_second_\nactive", cx);
            input.selected_range = 2..18;
            input.refresh_projection();
            assert!(input.projection.display.starts_with("**first**\n_second_"));
            input.selection_reversed = true;
            input.refresh_projection();
            assert!(input.projection.display.starts_with("**first**\n_second_"));
            input.selected_range = input.content.len()..input.content.len();
            input.refresh_projection();
            assert!(input.projection.display.starts_with("first\nsecond"));
        });
    }

    #[test]
    fn rich_projection_keeps_unicode_offsets_and_atomic_invocations() {
        let invocation = zeron_proto::invocation::Invocation::Skill {
            command: None,
            name: "bla-bla:bla-bla".into(),
            path: "/repo/SKILL.md".into(),
        };
        let raw = format!("**café** {} end\nactive", invocation.link());
        let active = raw.rfind('\n').unwrap() + 1..raw.len();
        let projection = TextProjection::rich(&raw, Some(active));
        assert!(projection.display.starts_with("café"));
        assert!(projection.display.contains("$Bla\u{00A0}Bla"));
        assert_eq!(projection.mentions.len(), 1);
        let (link, display) = &projection.mentions[0];
        assert_eq!(projection.raw_to_display(link.range.start), display.start);
        assert_eq!(projection.display_to_raw(display.end), link.range.end);
        assert_eq!(
            projection.normalize_range(link.range.start + 1..link.range.end - 1),
            link.range
        );
        assert_eq!(
            projection.previous_boundary(link.range.end),
            Some(link.range.start)
        );
        let (sent, _) = sent_mention_display(&raw).unwrap();
        assert!(sent.contains("$bla-bla:bla-bla"));
        assert!(sent.starts_with("**café**"));
    }

    #[test]
    fn invocation_completion_rejects_code_paths_currency_and_escapes() {
        for text in [
            "cost $100",
            "word$review",
            "\\$review",
            "`$review",
            "```\n$review",
            "path/$review",
        ] {
            assert!(invocation_token(text, text.len(), '$').is_none(), "{text}");
        }
        for text in [
            "use $review",
            "- $review",
            "($review",
            "first\n$review",
            "hello\u{00a0}$review",
        ] {
            assert_eq!(
                invocation_token(text, text.len(), '$').unwrap().query,
                "review"
            );
        }
        assert!(slash_token("try /usr/bin", 12).is_none());
        assert!(mention_token("`@file", 6).is_none());
    }

    #[gpui::test]
    fn rich_editor_selection_lists_and_undo(cx: &mut gpui::TestAppContext) {
        with_composer_input(cx, |input, window, cx| {
            input.set_text("hello café world\n- item", cx);
            assert_eq!(input.selection_unit(PressIntent::Word, 8), 6..11);
            input.drag_unit = Some((PressIntent::Word, 6..11));
            input.drag_select_to(2, cx);
            assert_eq!(input.selected_range, 0..11);
            assert!(input.selection_reversed);
            input.drag_select_to(14, cx);
            assert_eq!(input.selected_range, 6..17);
            input.move_to(input.content.len(), cx);
            input.newline(&Newline, window, cx);
            assert!(input.content.ends_with("\n- "));
            input.undo(&Undo, window, cx);
            assert!(input.content.ends_with("\n- item"));
            input.redo(&Redo, window, cx);
            input.newline(&Newline, window, cx);
            assert!(input.content.ends_with("item\n"));
        });
    }

    #[gpui::test]
    fn rendered_word_selection_uses_pointer_hit_testing(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        let input = handle
            .read_with(cx, |composer, _| composer.input.clone())
            .unwrap();
        input.update(cx, |input, cx| input.set_text("hello café world", cx));
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear();
            let position = {
                let editor = input.read(cx);
                let local = editor.point_for_index(8).unwrap();
                editor.last_bounds.unwrap().origin + local + point(px(1.0), px(5.0))
            };
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 2,
                    ..Default::default()
                }),
                cx,
            );
            assert_eq!(input.read(cx).selected_range, 6..11);
            window.dispatch_event(
                gpui::PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    click_count: 3,
                    ..Default::default()
                }),
                cx,
            );
            assert_eq!(input.read(cx).selected_range, 0..17);
        })
        .unwrap();
    }

    #[test]
    fn multi_click_selects_words_and_lines() {
        assert_eq!(press_intent(1, false), PressIntent::PlaceCaret);
        assert_eq!(press_intent(1, true), PressIntent::ExtendSelection);
        assert_eq!(press_intent(2, false), PressIntent::Word);
        assert_eq!(press_intent(3, false), PressIntent::Line);
        assert_eq!(word_range("hello café world", 8), 6..11);
        assert_eq!(word_range("hello world", 3), 0..5);
    }

    #[test]
    fn message_enter_bindings_cover_both_platform_modifiers() {
        for modifier in ["cmd-enter", "ctrl-enter"] {
            for (behavior, action) in [
                (
                    ComposerSendBehavior::Enter,
                    MessageEnterBindingAction::Submit,
                ),
                (
                    ComposerSendBehavior::ModEnter,
                    MessageEnterBindingAction::NewlineOrAccept,
                ),
            ] {
                assert_eq!(
                    message_enter_bindings(behavior, modifier),
                    vec![
                        MessageEnterBinding {
                            keystroke: "enter".into(),
                            action
                        },
                        MessageEnterBinding {
                            keystroke: modifier.into(),
                            action: MessageEnterBindingAction::ModifiedSubmit,
                        },
                    ]
                );
            }
        }
    }

    #[test]
    fn enter_accepts_a_completion_before_submit_or_newline() {
        assert_eq!(
            enter_outcome(true, EnterOutcome::Submit),
            EnterOutcome::AcceptCompletion
        );
        assert_eq!(
            enter_outcome(true, EnterOutcome::Newline),
            EnterOutcome::AcceptCompletion
        );
        assert_eq!(
            enter_outcome(false, EnterOutcome::Submit),
            EnterOutcome::Submit
        );
        assert_eq!(
            enter_outcome(false, EnterOutcome::Newline),
            EnterOutcome::Newline
        );
    }

    #[test]
    fn wizard_borrows_the_generic_enter_context_only_while_active() {
        assert_eq!(message_input_context(false), MESSAGE_COMPOSER_CONTEXT);
        assert_eq!(message_input_context(true), GENERIC_COMPOSER_CONTEXT);
    }

    #[test]
    fn stable_outer_width_only_schedules_reflow_on_real_changes() {
        assert!(composer_width_changed(None, 400.0));
        assert!(!composer_width_changed(Some(400.0), 400.0));
        assert!(!composer_width_changed(Some(400.0), 400.5));
        assert!(composer_width_changed(Some(400.0), 400.51));
    }

    fn tooltip_target(range: Range<usize>, path: &str) -> MentionTooltipTarget {
        MentionTooltipTarget {
            range,
            path: path.into(),
        }
    }

    #[test]
    fn mention_tooltip_wait_survives_pointer_jitter_and_promotes_once() {
        let target = tooltip_target(3..20, "src/composer.rs");
        let waiting = MentionTooltipPhase::Waiting {
            target: target.clone(),
            generation: 1,
        };
        let restarted = mention_tooltip_reduce(waiting.clone(), Some(target.clone()), false, 2);
        assert_eq!(restarted, waiting);
        assert!(matches!(
            restarted,
            MentionTooltipPhase::Waiting { generation: 1, .. }
        ));
        assert_eq!(
            mention_tooltip_promote(restarted.clone(), 2, true),
            restarted,
            "a stale timer must not reveal the tooltip"
        );
        let visible = mention_tooltip_promote(restarted, 1, true);
        assert!(matches!(
            visible,
            MentionTooltipPhase::Visible { generation: 1, .. }
        ));
        assert_eq!(
            mention_tooltip_reduce(visible.clone(), Some(target), false, 3),
            visible,
            "one visible activation keeps its presentation generation stable"
        );
    }

    #[test]
    fn mention_tooltip_changes_target_and_cancels_disappeared_target() {
        let first = tooltip_target(0..10, "src/a.rs");
        let second = tooltip_target(20..30, "src/a.rs");
        let visible = MentionTooltipPhase::Visible {
            target: first,
            generation: 4,
        };
        assert!(matches!(
            mention_tooltip_reduce(visible, Some(second), false, 5),
            MentionTooltipPhase::Waiting { generation: 5, .. }
        ));
        assert_eq!(
            mention_tooltip_promote(
                MentionTooltipPhase::Waiting {
                    target: tooltip_target(20..30, "src/a.rs"),
                    generation: 5,
                },
                5,
                false,
            ),
            MentionTooltipPhase::Hidden
        );
    }

    #[test]
    fn mention_tooltip_stays_visible_over_chip_or_popup_only() {
        assert!(mention_tooltip_contains(true, false));
        assert!(mention_tooltip_contains(false, true));
        assert!(!mention_tooltip_contains(false, false));
    }

    #[test]
    fn mention_wash_moves_wholly_to_the_next_visual_row_at_a_wrap() {
        assert_eq!(
            display_row_segments(12..24, [12, 40]),
            vec![(1, 12, 12..24)]
        );
        assert_eq!(
            display_row_segments(8..24, [12, 40]),
            vec![(0, 0, 8..12), (1, 12, 12..24)]
        );
    }

    #[test]
    fn mention_token_requires_a_token_boundary_and_tracks_full_token() {
        assert_eq!(
            mention_token("Fix @src/com", 12),
            Some(MentionToken {
                range: 4..12,
                query: "src/com".into(),
            })
        );
        assert!(mention_token("mail@example.com", 16).is_none());
        assert!(mention_token("word@file", 9).is_none());
        assert!(mention_token("path/@file", 10).is_none());
        assert_eq!(
            mention_token("See (@lib", 9).map(|token| token.range),
            Some(5..9)
        );
    }

    #[gpui::test]
    fn delayed_catalogs_cannot_replace_current_harness_or_preferences(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(crate::settings::UiSettings::default(), directory.path(), cx);
        });
        let (out, mut requests) = tokio::sync::mpsc::channel(64);
        let (replies, inbound) = tokio::sync::mpsc::channel(64);
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, _| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::RpcClient::new(out, inbound),
            ));
            state.chats = crate::settings::SKILL_COMPLETION_HARNESSES
                .iter()
                .enumerate()
                .map(|(index, (harness, _))| {
                    serde_json::from_value(serde_json::json!({
                        "id": format!("chat-{index}"), "deviceId": "host", "archived": false,
                        "cwd": format!("/worktree-{index}"), "createdAt": chrono::Utc::now(),
                        "config": { "harness": harness, "sandbox": "workspace-write" }
                    }))
                    .unwrap()
                })
                .collect();
        });
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        let mut delayed = Vec::new();
        for (index, (harness, _)) in crate::settings::SKILL_COMPLETION_HARNESSES
            .iter()
            .enumerate()
        {
            state.update(cx, |state, cx| {
                state.selected_chat = Some(format!("chat-{index}"));
                cx.notify();
            });
            cx.run_until_parked();
            composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("/", cx));
            });
            cx.run_until_parked();
            let mut batch = Vec::new();
            while let Ok(frame) = requests.try_recv() {
                let frame: zeron_rpc::ClientFrame = serde_json::from_str(&frame).unwrap();
                if matches!(
                    frame.method.as_deref(),
                    Some(methods::LIST_COMMANDS | methods::LIST_SKILLS)
                ) {
                    assert_eq!(
                        frame.params["harness"],
                        serde_json::to_value(harness).unwrap()
                    );
                    assert_eq!(frame.params["cwd"], format!("/worktree-{index}"));
                    batch.push(frame);
                }
            }
            assert_eq!(batch.len(), 2, "catalog requests for {harness:?}");
            delayed.extend(batch);
        }
        // Change the active checkout and slash separation while its original
        // catalog is still in flight. The token itself remains unchanged.
        state.update(cx, |state, cx| {
            state.chats.last_mut().unwrap().cwd = Some("/current-checkout".into());
            cx.notify();
        });
        cx.update(|cx| {
            crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
                settings.skill_completion_by_harness.insert(
                    HarnessId::Opencode,
                    crate::settings::SkillCompletionSettings {
                        dollar: true,
                        separate_from_slash: true,
                    },
                );
            });
        });
        cx.run_until_parked();
        let mut current = Vec::new();
        while let Ok(frame) = requests.try_recv() {
            let frame: zeron_rpc::ClientFrame = serde_json::from_str(&frame).unwrap();
            if matches!(
                frame.method.as_deref(),
                Some(methods::LIST_COMMANDS | methods::LIST_SKILLS)
            ) {
                assert_eq!(frame.params["cwd"], "/current-checkout");
                current.push(frame);
            }
        }
        assert_eq!(current.len(), 2);
        let respond = |frames: Vec<zeron_rpc::ClientFrame>, name: &str| {
            for frame in frames {
                let value = if frame.method.as_deref() == Some(methods::LIST_COMMANDS) {
                    serde_json::json!([{ "name": name, "description": "Provider command" }])
                } else {
                    serde_json::json!([{ "name": format!("{name}-skill"), "path": "/skills/SKILL.md", "description": "Skill", "enabled": true }])
                };
                replies
                    .try_send(
                        serde_json::to_string(&zeron_rpc::ServerFrame {
                            id: frame.id,
                            ok: Some(value),
                            ..Default::default()
                        })
                        .unwrap(),
                    )
                    .unwrap();
            }
            runtime.block_on(async { tokio::task::yield_now().await });
        };
        respond(current, "current-command");
        cx.run_until_parked();
        let visible_names = |composer: &Composer| {
            let rows = composer.slash_cache.get(&composer.slash.context).unwrap();
            composer
                .slash
                .filtered
                .iter()
                .map(|&index| rows[index].name.clone())
                .collect::<Vec<_>>()
        };
        let current_names = composer.read_with(cx, |composer, _| {
            assert!(!composer.slash.loading);
            let names = visible_names(composer);
            assert!(names.iter().any(|name| name == "current-command"));
            assert!(
                !names.iter().any(|name| name.ends_with("-skill")),
                "separated slash menu must exclude skills"
            );
            names
        });
        // Every abandoned harness now replies after the visible current result.
        delayed.reverse();
        respond(delayed, "stale-command");
        cx.run_until_parked();
        composer.read_with(cx, |composer, _| {
            assert_eq!(visible_names(composer), current_names);
            assert_eq!(composer.slash.harness, Some(HarnessId::Opencode));
        });

        // Acceptance, ordinary prose and changing trigger characters must all
        // retain catalogs scoped to this same device/harness/checkout.
        composer.update(cx, |composer, cx| composer.update_slash("$", 1, cx));
        cx.run_until_parked();
        let mut skill_requests = Vec::new();
        while let Ok(frame) = requests.try_recv() {
            let frame: zeron_rpc::ClientFrame = serde_json::from_str(&frame).unwrap();
            if matches!(
                frame.method.as_deref(),
                Some(methods::LIST_COMMANDS | methods::LIST_SKILLS)
            ) {
                assert_eq!(frame.method.as_deref(), Some(methods::LIST_SKILLS));
                skill_requests.push(frame);
            }
        }
        assert_eq!(skill_requests.len(), 1);
        respond(skill_requests, "current");
        cx.run_until_parked();
        let mut names = ["current-skill".to_string(), "current-command".to_string()];
        for (index, token) in ["$", "/", "$", "/"].into_iter().enumerate() {
            let kind = usize::from(token == "/");
            composer.update(cx, |composer, cx| {
                composer.reset_slash(None, cx);
                composer.update_slash("ordinary prose ", 15, cx);
                composer.update_slash(token, token.len(), cx);
                assert!(composer.slash.loading, "each open refreshes the catalog");
                assert!(
                    visible_names(composer).contains(&names[kind]),
                    "warm rows stay visible"
                );
                let query = format!("{token}refreshed");
                composer.update_slash(&query, query.len(), cx);
            });
            cx.run_until_parked();
            let mut refresh = Vec::new();
            while let Ok(frame) = requests.try_recv() {
                let frame: zeron_rpc::ClientFrame = serde_json::from_str(&frame).unwrap();
                if matches!(
                    frame.method.as_deref(),
                    Some(methods::LIST_COMMANDS | methods::LIST_SKILLS)
                ) {
                    refresh.push(frame);
                }
            }
            assert_eq!(
                refresh.len(),
                if token == "$" { 1 } else { 2 },
                "typing shares the open's request"
            );
            let fresh = format!("refreshed-{index}");
            respond(refresh, &fresh);
            cx.run_until_parked();
            let expected = if token == "$" {
                format!("{fresh}-skill")
            } else {
                fresh
            };
            composer.read_with(cx, |composer, _| {
                assert!(!composer.slash.loading);
                assert_eq!(visible_names(composer), [expected.clone()]);
                assert!(
                    !composer.slash_cache[&composer.slash.context]
                        .iter()
                        .any(|row| row.name == names[kind]),
                    "removed catalog entries must disappear"
                );
            });
            names[kind] = expected;
        }
    }

    #[gpui::test]
    fn reconnect_reconsiders_unchanged_completion_tokens(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = runtime.enter();
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, _, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("/model", cx));
                composer.on_input_edited(cx);
                let old_context = composer.slash.context.clone();
                let old_request = composer.slash.request;
                composer.state.update(cx, |state, _| {
                    state.connection = crate::state::ConnectionStatus::Ready
                });
                composer.on_state_changed(cx);
                assert_ne!(composer.slash.context, old_context);
                assert!(composer.slash.request > old_request);
                assert!(composer.slash.token.is_some());
                let context = composer.completion_connection_context(cx);
                let (out, _server) = tokio::sync::mpsc::channel(4);
                let (_incoming, inbound) = tokio::sync::mpsc::channel(4);
                composer.state.update(cx, |state, _| {
                    state.set_test_engine(crate::state::EngineHandle::from_test_client(
                        zeron_rpc::RpcClient::new(out, inbound),
                    ))
                });
                assert_ne!(composer.completion_connection_context(cx), context);
            })
            .unwrap();
    }

    #[gpui::test]
    fn explicit_navigation_breaks_undo_runs_but_backspace_remains_coalesced(
        cx: &mut gpui::TestAppContext,
    ) {
        with_composer_input(cx, |input, window, cx| {
            input.set_text("", cx);
            input.replace_text_in_range(None, "a", window, cx);
            input.move_to(0, cx);
            input.move_to(1, cx);
            input.replace_text_in_range(None, "b", window, cx);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "a");
            input.set_text("abcd", cx);
            input.backspace(&Backspace, window, cx);
            input.backspace(&Backspace, window, cx);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), "abcd");
        });
    }

    #[gpui::test]
    fn unmarking_ime_reopens_completion_without_another_keystroke(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, window, cx| {
                composer.input.update(cx, |input, cx| {
                    input.set_text("", cx);
                    input.replace_and_mark_text_in_range(None, "/model", None, window, cx);
                });
            })
            .unwrap();
        cx.run_until_parked();
        handle
            .update(cx, |composer, window, cx| {
                assert!(composer.slash.token.is_none());
                composer
                    .input
                    .update(cx, |input, cx| input.unmark_text(window, cx));
            })
            .unwrap();
        cx.run_until_parked();
        handle
            .read_with(cx, |composer, _| assert!(composer.slash.token.is_some()))
            .unwrap();
    }

    #[gpui::test]
    fn copying_part_of_a_literal_example_preserves_its_source(cx: &mut gpui::TestAppContext) {
        let input = cx.new(|cx| ComposerInput::new("Draft", cx));
        input.update(cx, |input, cx| {
            input.enable_mentions();
            for reference in [
                local_file_link("src/a.rs", false),
                zeron_proto::invocation::Invocation::Command {
                    name: "review".into(),
                }
                .link(),
            ] {
                for document in [
                    format!("`{reference}`"),
                    format!("```\n{reference}\n```"),
                    format!("![example {reference}](image.png)"),
                ] {
                    input.set_text(&document, cx);
                    let start = document.find(&reference).unwrap();
                    input.selected_range = start..start + reference.len();
                    assert_eq!(
                        input.clipboard_selection(),
                        Some((reference.clone(), reference.clone()))
                    );
                }
            }
        });
    }

    #[gpui::test]
    fn changing_slash_skill_preference_invalidates_open_completion(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, _, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("/review", cx));
                composer.on_input_edited(cx);
                composer
                    .slash_cache
                    .insert(composer.slash.context.clone(), vec![]);
            })
            .unwrap();
        let (context, request) = handle
            .read_with(cx, |composer, _| {
                (composer.slash.context.clone(), composer.slash.request)
            })
            .unwrap();
        cx.update(|cx| {
            crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |s| {
                s.skills_in_slash_menu = true
            });
        });
        cx.run_until_parked();
        handle
            .read_with(cx, |composer, _| {
                assert_ne!(composer.slash.context, context);
                assert!(
                    composer.slash.request > request,
                    "late responses must be rejected"
                );
                assert!(!composer.slash_cache.contains_key(&context));
                assert!(
                    composer
                        .slash_cache
                        .values()
                        .flatten()
                        .all(|row| row.workspace_command.is_some())
                );
                assert_eq!(composer.slash.token.as_ref().unwrap().query, "review");
            })
            .unwrap();
    }

    #[gpui::test]
    fn unchanged_mention_refreshes_when_its_workspace_changes(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, _, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("@src", cx));
                composer.on_input_edited(cx);
                composer.mention.context = "departing-worktree".into();
                composer.mention.token = mention_token("@src", 4);
                let old_request = composer.mention.request;
                composer.on_input_edited(cx);
                assert_ne!(composer.mention.context, "departing-worktree");
                assert!(composer.mention.request > old_request);
                assert!(!mention_response_is_current(&composer.mention, old_request));
                let context = composer.mention.context.clone();
                composer.reset_mention(Some((0..4, "@src".into())), cx);
                assert_eq!(composer.mention.context, context);
                composer.on_input_edited(cx);
                assert!(
                    composer.mention.token.is_none(),
                    "Escape must stay dismissed in the same workspace"
                );
            })
            .unwrap();
    }

    fn completion_token(text: &str, cursor: usize, prefix: char) -> Option<MentionToken> {
        if prefix == '@' {
            mention_token(text, cursor)
        } else {
            invocation_token(text, cursor, prefix)
        }
    }

    #[test]
    fn completion_respects_markdown_containers_and_destinations() {
        for prefix in ['@', '$', '/'] {
            let token = |text: &str, cursor| completion_token(text, cursor, prefix);
            for container in [">", ">>", "> >", "- >", "> first\n>"] {
                let text = format!("{container}{prefix}review");
                let found = token(&text, text.len()).unwrap();
                assert_eq!(found.range, container.len()..text.len(), "{text:?}");
            }
            for literal in ["comparison >", "> comparison >", "    >", "\\>"] {
                let text = format!("{literal}{prefix}review");
                assert!(token(&text, text.len()).is_none(), "{text:?}");
            }
            for link in [
                format!("[label]({prefix}review)"),
                format!("[label]({prefix}review"),
                format!("![label]({prefix}review)"),
                format!("[{prefix}review](https://example.com)"),
            ] {
                let cursor = link.find("review").unwrap() + "review".len();
                assert!(token(&link, cursor).is_none(), "{link:?}");
            }
            let after_link = format!("[label](url) ({prefix}review");
            assert!(token(&after_link, after_link.len()).is_some());
        }
        let canonical = zeron_proto::invocation::Invocation::Command {
            name: "review".into(),
        }
        .link();
        assert!(slash_token(&canonical, canonical.find("review").unwrap() + 6).is_none());
    }

    #[test]
    fn completion_keeps_multiline_and_reference_destinations_literal() {
        for prefix in ['@', '$', '/'] {
            let token = |text: &str, cursor| completion_token(text, cursor, prefix);
            for source in [
                format!("[label](\n{prefix}review"),
                format!("[label](foo\n {prefix}review"),
                format!("[label](url \"title\n {prefix}review"),
                format!("[id]: {prefix}review"),
                format!("[id]:\n  {prefix}review"),
                format!("> [id]: {prefix}review"),
                format!("[id]: url\n[id]: {prefix}review"),
                format!("[id]: url\n[id]:\n  {prefix}review"),
                format!("[outer]([inner\\]({prefix}review"),
                format!("<!-- {prefix}review -->"),
            ] {
                let cursor = source.find("review").unwrap() + "review".len();
                assert!(token(&source, cursor).is_none(), "{source:?}");
            }
            for source in [
                format!("[label](unfinished\n\n{prefix}review"),
                format!("[label](unfinished\n- {prefix}review"),
                format!("[id]: url\n\n{prefix}review"),
                format!("[label\\]({prefix}review"),
                format!("[label](url)\n{prefix}review"),
                format!("- first\n- {prefix}review"),
                format!("- first\n\n- {prefix}review"),
                format!("- first\n  - {prefix}review"),
                format!("> - {prefix}review"),
                prefix.to_string(),
            ] {
                assert!(token(&source, source.len()).is_some(), "{source:?}");
            }
        }
    }

    #[test]
    fn completion_closes_at_parsed_emphasis_delimiters() {
        for prefix in ['@', '$', '/'] {
            let token = |text: &str, cursor| completion_token(text, cursor, prefix);
            for delimiter in ["_", "**", "***", "~~"] {
                let source = format!("{delimiter}see {prefix}review{delimiter}");
                let closing = source.len() - delimiter.len();
                assert_eq!(token(&source, closing).unwrap().range.end, closing);
                for cursor in closing + 1..=source.len() {
                    assert!(token(&source, cursor).is_none(), "{source:?}@{cursor}");
                }
            }
        }
    }

    #[test]
    fn invocation_tokens_preserve_unicode_graphemes() {
        for name in ["réview", "re\u{301}view", "確認", "レビュー"] {
            for prefix in ['$', '/'] {
                let text = format!("please {prefix}{name}");
                let token = invocation_token(&text, text.len(), prefix).unwrap();
                assert_eq!(token.query, name);
                assert_eq!(&text[token.range], format!("{prefix}{name}"));
            }
        }
        let text = "$re\u{301}view";
        let token = invocation_token(text, 3, '$').unwrap();
        assert_eq!(token.query, "re");
        assert_eq!(token.range, 0..text.len());
        for text in ["$🧑", "$٣", "$12", "$re/view", "$\u{301}"] {
            assert!(
                invocation_token(text, text.len(), '$').is_none(),
                "{text:?}"
            );
        }
        assert!(invocation_token("$é", 2, '$').is_none());
    }

    #[test]
    fn file_completion_retains_parsed_emphasis_closing_delimiters() {
        for (text, suffix) in [
            ("**see @src**", "**"),
            ("_see @src_", "_"),
            ("~~see @src~~", "~~"),
            ("***see @src***", "***"),
        ] {
            let cursor = text.find("@src").unwrap() + 4;
            let token = mention_token(text, cursor).unwrap();
            assert_eq!(&text[token.range.end..], suffix, "{text:?}");
            assert_eq!(&text[token.range], "@src");
        }
        assert_eq!(
            mention_token("@file*name.rs", 13).unwrap().query,
            "file*name.rs"
        );
    }

    #[test]
    fn file_completion_preserves_surrounding_delimiters() {
        for (text, end) in [("(@src)", 5), ("[@src]", 5), ("{@src}", 5)] {
            let token = mention_token(text, end).unwrap();
            assert_eq!(token.range, 1..end);
            assert_eq!(token.query, "src");
            assert!(mention_token(text, text.len()).is_none());
        }
        assert_eq!(mention_token("@src/(draft).rs", 15).unwrap().range, 0..15);
        assert!(mention_token("\\@src", 5).is_none());
    }

    #[test]
    fn legacy_host_commands_remain_literal_and_saved_references_need_an_update() {
        use zeron_proto::invocation::Invocation;
        let command = Invocation::Command {
            name: "compact".into(),
        };
        assert_eq!(invocation_insertion(&command, false), "/compact");
        assert_eq!(invocation_insertion(&command, true), command.link());
        let skill = Invocation::Skill {
            name: "review".into(),
            path: "/repo/SKILL.md".into(),
            command: None,
        };
        for reference in [
            command.link(),
            skill.link(),
            local_file_link("src/lib.rs", false),
        ] {
            assert!(references_require_update(&reference, false));
            assert!(!references_require_update(&reference, true));
            for literal in [
                format!("`{reference}`"),
                format!("    {reference}"),
                format!("\\{reference}"),
            ] {
                assert!(!references_require_update(&literal, false));
            }
        }
        assert!(!references_require_update("/compact", false));
    }

    #[gpui::test]
    fn unsupported_host_preserves_reference_drafts_and_queue_edits(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, _, cx| {
                let draft = zeron_proto::invocation::Invocation::Command {
                    name: "compact".into(),
                }
                .link();
                composer
                    .input
                    .update(cx, |input, cx| input.set_text(draft.clone(), cx));
                let attachment = attachments::stage_png_bytes("draft.png".into(), Vec::new());
                let attachment_id = attachment.id.clone();
                composer
                    .attachments
                    .insert(composer.current_key.clone(), vec![attachment]);
                composer.state.update(cx, |state, _| {
                    state.selected_device = Some("peer".into());
                    state.devices =
                        vec![serde_json::from_value(serde_json::json!({
                    "id": "peer", "name": "Peer", "platform": "linux", "capabilities": []
                })).unwrap()];
                });
                assert!(!composer.reference_delivery_supported(cx));
                composer.send(draft.clone(), false, cx);
                assert!(
                    composer
                        .failure
                        .as_deref()
                        .unwrap()
                        .contains("Update the selected device")
                );
                assert_eq!(composer.input.read(cx).text(), draft);
                assert_eq!(composer.staged()[0].id, attachment_id);
                composer.editing_queued = Some("queued-draft".into());
                assert!(composer.commit_queue_edit(cx));
                assert_eq!(composer.editing_queued.as_deref(), Some("queued-draft"));
                assert!(!composer.queue_edit_finishing);
                assert_eq!(composer.input.read(cx).text(), draft);
                composer.state.update(cx, |state, _| {
                    state.devices[0]
                        .capabilities
                        .push(capabilities::COMPOSER_REFERENCES_V1.into());
                });
                assert!(composer.reference_delivery_supported(cx));
                assert!(composer.check_reference_delivery(&draft, cx));
            })
            .unwrap();
    }

    #[test]
    fn partial_discovery_keeps_commands_and_skills_independently() {
        let command = SlashCommand {
            name: "review".into(),
            description: String::new(),
            input_hint: None,
        };
        let skill = zeron_proto::invocation::Skill {
            command: None,
            name: "review".into(),
            path: "/repo/SKILL.md".into(),
            description: String::new(),
            enabled: true,
        };
        let (rows, supported, warning) = merge_invocation_results(
            Ok(vec![command]),
            Err(RpcError::UnknownMethod("ListSkills".into())),
            false,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].invocation.prefix(), '/');
        assert!(supported && warning.is_some());
        assert_eq!(invocation_insertion(&rows[0].invocation, false), "/review");
        let (rows, _, warning) = merge_invocation_results(
            Err(RpcError::Failed("commands unavailable".into())),
            Ok(Some(vec![skill])),
            false,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].invocation.prefix(), '$');
        assert!(warning.is_some());
        assert!(merge_invocation_results(Ok(vec![]), Err(RpcError::Closed), true).is_err());
        let (rows, supported, warning) =
            merge_invocation_results(Ok(vec![]), Ok(None), true).unwrap();
        assert!(rows.is_empty() && !supported && warning.is_none());
    }

    #[gpui::test]
    fn commands_complete_inline_and_partial_discovery_retries(cx: &mut gpui::TestAppContext) {
        let (_dir, handle) = composer_focus_window(cx);
        handle
            .update(cx, |composer, _, cx| {
                for text in [
                    "https://example/review",
                    "/repo/review/",
                    "`/review",
                    "```\n/review",
                ] {
                    composer.update_slash(text, text.len(), cx);
                    assert!(composer.slash.token.is_none(), "{text}");
                }
                for text in [
                    "/review",
                    "  /review",
                    "please $review",
                    "please /review",
                    "(/review",
                    "first\n/review",
                ] {
                    composer.update_slash(text, text.len(), cx);
                    assert!(composer.slash.token.is_some(), "{text}");
                }
                composer
                    .slash_cache
                    .insert(composer.slash.context.clone(), vec![]);
                composer.slash.error = Some("Skills unavailable".into());
                composer.reset_slash(None, cx);
                assert!(composer.slash_cache.is_empty());
            })
            .unwrap();
    }

    #[test]
    fn every_harness_respects_both_skill_completion_toggles() {
        use crate::settings::SkillCompletionSettings;
        for (harness, _) in crate::settings::SKILL_COMPLETION_HARNESSES {
            for dollar in [false, true] {
                for separate_from_slash in [false, true] {
                    let preferences = SkillCompletionSettings {
                        dollar,
                        separate_from_slash,
                    };
                    let (token, skill, include, _) = completion_trigger("$review", 7, preferences);
                    assert_eq!(token.is_some(), dollar, "{harness:?}");
                    assert_eq!(skill, dollar);
                    assert_eq!(include, dollar || !separate_from_slash);
                    let (token, skill, include, commands) =
                        completion_trigger("/review", 7, preferences);
                    assert!(token.is_some() && !skill && commands);
                    assert_eq!(include, !separate_from_slash);
                    let (token, _, _, commands) =
                        completion_trigger("please /review", 14, preferences);
                    assert!(token.is_some() && commands);
                }
            }
        }
    }

    #[test]
    fn every_harness_catalog_only_offers_round_trippable_references() {
        use zeron_proto::invocation::{Skill, SkillCommand, invocation_links};
        for (harness, _) in crate::settings::SKILL_COMPLETION_HARNESSES {
            let commands = ["review", "bad\ncommand", "two words"]
                .into_iter()
                .map(|name| SlashCommand {
                    name: name.into(),
                    description: String::new(),
                    input_hint: None,
                })
                .collect();
            let skills = [
                ("审查-é", "/repo/skill dir/SKILL.md", None),
                ("bad name", "/repo/SKILL.md", None),
                ("review", "/repo/bad\npath/SKILL.md", Some("review")),
                ("broken-native", "harness-skill:probe", Some("bad command")),
            ]
            .into_iter()
            .map(|(name, path, command)| Skill {
                name: name.into(),
                path: path.into(),
                description: String::new(),
                enabled: true,
                command: command.map(|name| SkillCommand {
                    name: name.into(),
                    harness,
                }),
            })
            .collect();
            let rows = invocation_candidates(commands, skills);
            assert_eq!(
                rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
                ["review", "审查-é"],
                "{harness:?}"
            );
            for row in rows {
                assert_eq!(
                    invocation_links(&row.invocation.link())[0].1,
                    row.invocation
                );
            }
        }
    }

    #[test]
    fn separated_native_skills_are_not_left_in_the_command_catalog() {
        use zeron_proto::invocation::{Skill, SkillCommand};
        for (harness, _) in crate::settings::SKILL_COMPLETION_HARNESSES {
            let commands = vec![
                SlashCommand {
                    name: "review".into(),
                    description: String::new(),
                    input_hint: None,
                },
                SlashCommand {
                    name: "compact".into(),
                    description: String::new(),
                    input_hint: None,
                },
            ];
            let skills = vec![Skill {
                name: "review".into(),
                path: "/repo/SKILL.md".into(),
                description: String::new(),
                enabled: true,
                command: Some(SkillCommand {
                    name: "review".into(),
                    harness,
                }),
            }];
            let rows = invocation_candidates(commands, skills);
            assert_eq!(rows.len(), 2);
            assert_eq!(
                rows.iter()
                    .filter(|row| row.invocation.prefix() == '/')
                    .map(|row| row.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["compact"]
            );
            assert_eq!(rows[1].invocation.prefix(), '$');
        }
    }

    #[test]
    fn combined_invocations_preserve_skill_identity_and_command_collisions() {
        use zeron_proto::invocation::{Invocation, Skill};
        let commands = vec![SlashCommand {
            name: "review".into(),
            description: "Command".into(),
            input_hint: None,
        }];
        let skills = vec![
            Skill {
                command: None,
                name: "review".into(),
                path: "/a/SKILL.md".into(),
                description: "A".into(),
                enabled: true,
            },
            Skill {
                command: None,
                name: "review".into(),
                path: "/b/SKILL.md".into(),
                description: "B".into(),
                enabled: true,
            },
            Skill {
                command: None,
                name: "hidden".into(),
                path: "/c/SKILL.md".into(),
                description: String::new(),
                enabled: false,
            },
        ];
        assert_eq!(invocation_candidates(commands.clone(), vec![]).len(), 1);
        let dollar = invocation_candidates(vec![], skills.clone());
        assert_eq!(dollar.len(), 2);
        let slash = invocation_candidates(commands, skills);
        assert_eq!(slash.len(), 3);
        assert!(matches!(slash[0].invocation, Invocation::Command { .. }));
        for candidate in &slash[1..] {
            assert!(matches!(candidate.invocation, Invocation::Skill { .. }));
            assert!(candidate.invocation.prompt_text().contains("SKILL.md"));
        }
        assert_ne!(slash[1].invocation, slash[2].invocation);
    }

    #[test]
    fn slash_token_opens_at_prose_boundaries() {
        assert_eq!(
            slash_token("/comp", 5),
            Some(MentionToken {
                range: 0..5,
                query: "comp".into(),
            })
        );
        // Token range spans the whole command word even mid-cursor.
        assert_eq!(
            slash_token("/compact now", 3),
            Some(MentionToken {
                range: 0..8,
                query: "co".into(),
            })
        );
        // Not at offset 0 → prose, not a command.
        assert_eq!(slash_token("run /compact", 12).unwrap().range, 4..12);
        // Cursor past the command word (typing the argument) → closed.
        assert!(slash_token("/goal ship it", 10).is_none());
        // A typed absolute path is not a command.
        assert!(slash_token("/usr/bin", 8).is_none());
        // Bare "/" with cursor at 0 → closed; cursor after it → open-all.
        assert!(slash_token("/", 0).is_none());
        assert_eq!(slash_token("/", 1).map(|t| t.query), Some(String::new()));
    }

    #[test]
    fn dismissed_mentions_reject_stale_responses() {
        let mut state = FileMentionState {
            token: mention_token("@src", 4),
            request: 7,
            ..FileMentionState::default()
        };
        assert!(mention_response_is_current(&state, 7));
        state.request += 1;
        state.token = None;
        assert!(!mention_response_is_current(&state, 7));
        assert!(!mention_response_is_current(&state, 8));
    }

    #[test]
    fn file_mentions_serialize_to_strict_local_markdown() {
        let raw = local_file_link("src/a file#[x].rs", false);
        assert_eq!(
            raw,
            "[a file#\\[x\\].rs](zeron-file:src/a%20file%23%5Bx%5D.rs)"
        );
        let links = file_mention_links(&raw);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].path, "src/a file#[x].rs");
        assert_eq!(links[0].basename, "a file#[x].rs");
        assert!(!links[0].is_dir);

        let folder = local_file_link("src/components", true);
        assert_eq!(folder, "[components](zeron-file:src/components/)");
        let links = file_mention_links(&folder);
        assert_eq!(links[0].path, "src/components");
        assert!(links[0].is_dir);
    }

    #[test]
    fn dropped_mentions_are_separated_from_surrounding_text() {
        let (inserted, cursor_advance) =
            dropped_file_mention("fixnow", 3..3, "src/lib.rs", false).expect("valid drop");
        assert_eq!(inserted, " [lib.rs](zeron-file:src/lib.rs) ");
        assert_eq!(cursor_advance, inserted.len());

        let (inserted, cursor_advance) =
            dropped_file_mention("fix now", 3..3, "src/components", true).expect("valid drop");
        assert_eq!(inserted, " [components](zeron-file:src/components/)");
        assert_eq!(cursor_advance, inserted.len() + 1);
    }

    #[test]
    fn dropped_mentions_reject_paths_outside_the_workspace() {
        assert!(dropped_file_mention("", 0..0, "/tmp/file.rs", false).is_none());
        assert!(dropped_file_mention("", 0..0, "../file.rs", false).is_none());
    }

    #[test]
    fn file_mentions_reject_external_or_noncanonical_markdown() {
        assert!(file_mention_links("[site](https://example.com/a)").is_empty());
        assert!(file_mention_links("[a.rs](../a.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src/a file.rs)").is_empty());
        assert!(file_mention_links("[other](src/a.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src/a.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src%5Cfake%5Ca.rs)").is_empty());
        assert!(file_mention_links("[a.rs](src/a%0A.rs)").is_empty());
    }

    #[test]
    fn duplicate_mention_basenames_use_unique_suffixes() {
        let raw = format!(
            "{} {}",
            local_file_link("src/one/mod.rs", false),
            local_file_link("src/two/mod.rs", false)
        );
        let projection = TextProjection::new(&raw);
        assert!(projection.display.contains("one/mod.rs"));
        assert!(projection.display.contains("two/mod.rs"));
    }

    #[test]
    fn mention_suffixes_compare_path_components() {
        let links = vec![
            FileMentionLink {
                range: 0..0,
                basename: "mod.rs".into(),
                path: "foo/mod.rs".into(),
                is_dir: false,
                prefix: '@',
            },
            FileMentionLink {
                range: 0..0,
                basename: "oomod.rs".into(),
                path: "bar/oomod.rs".into(),
                is_dir: false,
                prefix: '@',
            },
        ];
        assert_eq!(
            mention_display_labels(&links),
            vec!["mod.rs".to_string(), "oomod.rs".to_string()]
        );
    }

    #[test]
    fn projection_maps_and_expands_atomic_chip_ranges() {
        let raw = format!("open {} now", local_file_link("src/composer.rs", false));
        let projection = TextProjection::new(&raw);
        let (link, chip) = &projection.mentions[0];
        assert_eq!(
            &projection.display[chip.clone()],
            "\u{00A0}@composer.rs\u{00A0}"
        );
        assert_eq!(projection.display_to_raw(chip.start + 1), link.range.start);
        assert_eq!(projection.display_to_raw(chip.end - 1), link.range.end);
        assert_eq!(
            projection.previous_boundary(link.range.end),
            Some(link.range.start)
        );
        assert_eq!(
            projection.next_boundary(link.range.start),
            Some(link.range.end)
        );
        assert_eq!(
            projection.normalize_range(link.range.start + 2..link.range.end - 2),
            link.range
        );
    }

    #[test]
    fn sent_mention_display_projects_chips_for_the_transcript() {
        let raw = format!(
            "check {} and {}",
            local_file_link("src/composer.rs", false),
            local_file_link("src/components", true)
        );
        let (display, spans) = sent_mention_display(&raw).expect("mentions project");
        assert!(!display.contains(FILE_MENTION_SCHEME));
        assert!(display.contains("composer.rs"));
        assert!(display.contains("components"));
        assert_eq!(spans.len(), 2);
        assert_eq!(
            &display[spans[0].range.clone()],
            "\u{00A0}@composer.rs\u{00A0}"
        );
        assert!(!spans[0].is_dir);
        assert_eq!(spans[0].path.as_ref(), "src/composer.rs");
        assert!(spans[1].is_dir);
        assert_eq!(spans[1].path.as_ref(), "src/components/");
    }

    /// Ordinary prompts must stay on the zero-cost path, including ones that
    /// merely *talk about* the scheme without containing a valid mention.
    #[test]
    fn sent_mention_display_leaves_plain_prompts_untouched() {
        assert_eq!(sent_mention_display("fix the composer"), None);
        assert_eq!(
            sent_mention_display("what is a zeron-file: link?"),
            None,
            "scheme substring without a valid mention link"
        );
        assert_eq!(
            sent_mention_display("[a.rs](zeron-file:../a.rs)"),
            None,
            "a hostile path never becomes a chip in the transcript either"
        );
    }

    fn question(id: &str, options: &[&str], multi: bool) -> UserInputQuestion {
        UserInputQuestion {
            id: id.into(),
            header: "Header".into(),
            question: format!("Question {id}"),
            options: options.iter().map(|s| s.to_string()).collect(),
            multi_select: multi,
        }
    }

    #[test]
    fn flip_decision() {
        // Fits in the pill → compact stays compact.
        assert!(!composer_flip(false, 150.0, 300.0, false, false));
        // Overflow → expand.
        assert!(composer_flip(false, 320.0, 300.0, false, false));
        // Newline always expands (either mode, even mid-resize).
        assert!(composer_flip(false, 10.0, 300.0, true, false));
        assert!(composer_flip(true, 10.0, 300.0, true, true));
        // Narrow column (< MIN_COMPACT_INPUT_WIDTH) always expands.
        assert!(composer_flip(false, 10.0, 199.0, false, false));
        assert!(!composer_flip(false, 10.0, 200.0, false, false));
    }

    #[test]
    fn flip_hysteresis_band_prevents_oscillation() {
        let cap = 300.0;
        // Text just over capacity expands…
        assert!(composer_flip(false, cap + 1.0, cap, false, false));
        // …and the SAME width, now expanded, does NOT collapse back — the
        // collapse threshold sits COLLAPSE_HYSTERESIS below the expand one.
        assert!(composer_flip(true, cap + 1.0, cap, false, false));
        // Anywhere inside the band the two modes are both stable (no width in
        // (cap - 32, cap] flips in either direction).
        let in_band = cap - COLLAPSE_HYSTERESIS + 1.0;
        assert!(!composer_flip(false, in_band, cap, false, false));
        assert!(composer_flip(true, in_band, cap, false, false));
        // Comfortably under the band → collapses.
        assert!(!composer_flip(
            true,
            cap - COLLAPSE_HYSTERESIS - 1.0,
            cap,
            false,
            false
        ));
    }

    #[test]
    fn resize_expands_live_but_defers_collapse() {
        // A compact composer expands immediately as its text or controls stop
        // fitting, even while the divider is moving.
        assert!(composer_flip(false, 500.0, 300.0, false, true));
        assert!(composer_flip(false, 10.0, 150.0, false, true));
        // An expanded composer waits for the drag to settle before collapsing,
        // avoiding mode chatter while the user reverses direction.
        assert!(composer_flip(true, 0.0, 300.0, false, true));
        // Once settled, the same wide layout may collapse.
        assert!(composer_flip(false, 500.0, 300.0, false, false));
        assert!(!composer_flip(true, 0.0, 300.0, false, false));
        assert!(composer_flip(false, 10.0, 150.0, false, false));
    }

    #[test]
    fn caret_blink_phase() {
        // Solid through the first half-period (typing burst never blinks).
        assert!(caret_visible(0));
        assert!(caret_visible(CARET_BLINK_MS - 1));
        // Off for the second half-period, back on for the third.
        assert!(!caret_visible(CARET_BLINK_MS));
        assert!(!caret_visible(2 * CARET_BLINK_MS - 1));
        assert!(caret_visible(2 * CARET_BLINK_MS));
    }

    #[test]
    fn auto_grow_math() {
        // The source heights (zeron composer.tsx line 235 clamp, composer-
        // actions.tsx row, 1px hairlines): 76+46+2 empty … 260+46+2 capped.
        assert_eq!(COMPOSER_MIN_HEIGHT, 120.0);
        assert_eq!(COMPOSER_MAX_HEIGHT, 304.0);
        // One line sits at the floor: the textarea BOX (content + `pt-4 pb-1`)
        // clamps UP to 76 exactly like `Math.max(scrollHeight, 76)` — this is
        // what makes the always-expanded new-chat composer 120px tall.
        assert_eq!(
            composer_total_height(input_content_height(1)),
            COMPOSER_MIN_HEIGHT
        );
        // Growth is linear once the textarea box exceeds its 76px floor.
        let h4 = composer_total_height(input_content_height(4));
        assert_eq!(
            h4,
            4.0 * INPUT_LINE_HEIGHT + TEXTAREA_PAD_V + ACTIONS_ROW_HEIGHT + PILL_BORDER_V
        );
        // Caps at a 260px textarea box (zeron max-h-[260px] / the JS clamp).
        assert_eq!(
            composer_total_height(input_content_height(100)),
            COMPOSER_MAX_HEIGHT
        );
        // Zero lines still measures one.
        assert_eq!(input_content_height(0), INPUT_LINE_HEIGHT);
    }

    #[test]
    fn appshot_strip_height_tracks_cards() {
        assert_eq!(appshot_strip_height(0), 0.0);
        assert_eq!(appshot_strip_height(1), STRIP_PAD_TOP + APPSHOT_TILE_HEIGHT);
        assert_eq!(appshot_strip_height(2), appshot_strip_height(1));
    }

    #[test]
    fn appshot_images_share_height_and_adapt_width_without_losing_aspect_ratio() {
        let landscape = appshot_contained_size(Some((1600, 900)), 320.0);
        assert!((landscape.0 - 234.66667).abs() < 0.01);
        assert_eq!(landscape.1, APPSHOT_IMAGE_MAX_HEIGHT);
        let portrait = appshot_contained_size(Some((900, 1600)), 320.0);
        assert!((portrait.0 - 74.25).abs() < 0.01);
        assert_eq!(portrait.1, landscape.1);
        assert_eq!(
            appshot_contained_size(Some((1000, 1000)), 320.0),
            (132.0, 132.0)
        );
        // Narrow side-by-side layouts and panoramas fit without distortion.
        let narrow = appshot_contained_size(Some((1600, 900)), 160.0);
        assert_eq!(narrow, (160.0, 90.0));
        assert_eq!(
            appshot_contained_size(Some((4000, 1000)), 900.0),
            (320.0, 80.0)
        );
        let fallback = appshot_contained_size(None, 320.0);
        assert!((fallback.0 - 211.2).abs() < 0.01);
        assert_eq!(fallback.1, 132.0);
    }

    #[test]
    fn input_wheel_scroll_uses_gpui_direction_and_clamps() {
        // Positive wheel delta moves toward the start; negative moves down.
        assert_eq!(input_scroll_offset(40.0, 20.0, 200.0, 100.0), 20.0);
        assert_eq!(input_scroll_offset(40.0, -30.0, 200.0, 100.0), 70.0);
        // Neither edge can be overscrolled.
        assert_eq!(input_scroll_offset(10.0, 50.0, 200.0, 100.0), 0.0);
        assert_eq!(input_scroll_offset(90.0, -50.0, 200.0, 100.0), 100.0);
        // Short content has no internal scroll range.
        assert_eq!(input_scroll_offset(20.0, -50.0, 80.0, 100.0), 0.0);
    }

    #[test]
    fn input_scroll_reveals_only_when_caret_leaves_viewport() {
        // A visible caret preserves the user's viewport.
        assert_eq!(
            input_scroll_offset_for_cursor(40.0, 60.0, 20.0, 300.0, 100.0, None),
            40.0
        );
        // Moving above or below reveals the row with the smallest adjustment.
        assert_eq!(
            input_scroll_offset_for_cursor(80.0, 30.0, 20.0, 300.0, 100.0, None),
            30.0
        );
        assert_eq!(
            input_scroll_offset_for_cursor(20.0, 130.0, 20.0, 300.0, 100.0, None),
            50.0
        );
        // Revealing the final row clamps exactly to the content end.
        assert_eq!(
            input_scroll_offset_for_cursor(0.0, 290.0, 20.0, 300.0, 100.0, None),
            200.0
        );
    }

    #[test]
    fn input_drag_autoscroll_is_edge_proportional_and_capped() {
        let top = 100.0;
        let bottom = 300.0;
        let line = INPUT_LINE_HEIGHT;
        assert_eq!(input_drag_scroll_delta(200.0, top, bottom, line), 0.0);
        assert_eq!(input_drag_scroll_delta(90.0, top, bottom, line), -2.0);
        assert_eq!(input_drag_scroll_delta(315.0, top, bottom, line), 3.0);
        assert_eq!(input_drag_scroll_delta(-100.0, top, bottom, line), -line);
        assert_eq!(input_drag_scroll_delta(500.0, top, bottom, line), line);
    }

    /// One frame short of the full morph timeline (never rounds up to done).
    const ALMOST: f32 = 179.0;

    #[test]
    fn flip_morph_starts_once_per_committed_flip() {
        // No committed flip → no morph.
        assert_eq!(flip_morph_step(None, false, 49.0, 0.0, false, false), None);
        // A committed flip starts one, from the last rendered height…
        let m = flip_morph_step(None, true, 49.0, 100.0, false, false).unwrap();
        assert_eq!(m.from, 49.0);
        assert_eq!(m.start_ms, 100.0);
        // …and same-mode renders keep it UNCHANGED (no restart at the
        // boundary, whatever the heights are doing).
        assert_eq!(
            flip_morph_step(Some(m), false, 80.0, 150.0, false, false),
            Some(m)
        );
        // A finished morph clears on the next same-mode render.
        assert_eq!(
            flip_morph_step(Some(m), false, 124.0, 100.0 + ALMOST, false, false),
            Some(m)
        );
        assert_eq!(
            flip_morph_step(Some(m), false, 124.0, 300.0, false, false),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resolved_layout_does_not_keep_notifying_on_repaint() {
        use std::cell::Cell;
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let handle = cx.open_window(gpui::WindowOptions::default(), |_, cx| {
                cx.new(|cx| {
                    let mut input = ComposerInput::new("Draft", cx);
                    input.set_text("A long line whose wrapping differs between provisional and resolved widths.\n".repeat(100), cx);
                    input
                })
            }).unwrap();
            let changes = Rc::new(Cell::new(0));
            let observed = changes.clone();
            let subscription = cx.subscribe(&handle.entity(cx).unwrap(), move |_, event, _| {
                if matches!(event, ComposerInputEvent::ViewportChanged) {
                    observed.set(observed.get() + 1);
                }
            });
            cx.spawn(async move |cx| {
                let _subscription = subscription;
                cx.update(|cx| {
                    handle.update(cx, |input, _, _| input.last_notified_layout = None).unwrap();
                    cx.update_window(handle.into(), |_, window, cx| { window.refresh(); let _ = window.draw(cx); }).unwrap();
                });
                let settled = changes.get();
                assert!(settled > 0, "the first resolved layout must be published");
                for _ in 0..30 {
                    cx.update(|cx| {
                        cx.update_window(handle.into(), |_, window, cx| { window.refresh(); let _ = window.draw(cx); }).unwrap();
                    });
                }
                assert_eq!(changes.get(), settled, "unchanged draws must not schedule more layout");
                cx.update(|cx| cx.quit());
            }).detach();
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn single_line_address_reveals_caret_and_maps_scrolled_pointer() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let handle = cx
                .open_window(gpui::WindowOptions::default(), |_, cx| {
                    cx.new(|cx| {
                        ComposerInput::new("Address", cx)
                            .with_single_line()
                            .with_text_metrics(11.0, 16.0)
                    })
                })
                .unwrap();
            handle
                .update(cx, |input, window, cx| {
                    let style = window.text_style();
                    input.set_text(
                        "http://device.a-very-long-project-name.localhost:7331/path",
                        cx,
                    );
                    input.layout_text(px(100.0), &style, window, cx);
                    assert_eq!(input.content_height, 16.0, "long hostnames must not wrap");
                    input.clamp_scroll(16.0);
                    assert!(input.scroll_left > 0.0);
                    let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(16.0)));
                    input.last_bounds = Some(bounds);
                    let caret = input
                        .bounds_for_range(
                            input.content.len()..input.content.len(),
                            bounds,
                            window,
                            cx,
                        )
                        .unwrap();
                    assert!(caret.left() >= bounds.left() && caret.right() <= bounds.right());
                    assert_eq!(
                        input.index_for_mouse_position(caret.origin),
                        input.content.len()
                    );
                    input.selected_range = 0..0;
                    input.clamp_scroll(16.0);
                    assert_eq!(input.scroll_left, 0.0, "Home must reveal the URL start");
                    input.replace_text_in_range(None, "one\r\ntwo", window, cx);
                    assert!(!input.content.contains(['\r', '\n']));
                    input.set_text("short", cx);
                    input.layout_text(px(100.0), &style, window, cx);
                    input.clamp_scroll(16.0);
                    assert_eq!(
                        input.scroll_left, 0.0,
                        "short replacement must reset scrolling"
                    );
                })
                .unwrap();
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn layout_cache_reuses_resize_frames_and_invalidates_text_inputs() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let handle = cx
                .open_window(gpui::WindowOptions::default(), |_, cx| {
                    cx.new(|cx| ComposerInput::new("Draft", cx))
                })
                .unwrap();
            handle
                .update(cx, |input, window, cx| {
                    input.layout_rebuilds = 0; // Exclude the window's initial placeholder paint.
                    let mut style = window.text_style();
                    style.font_size = px(INPUT_TEXT_SIZE).into();
                    input.set_text(
                        "A wrapped draft with enough text to measure.\n".repeat(100),
                        cx,
                    );
                    input.layout_text(px(400.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 1);
                    let height = input.content_height;
                    for frame in 0..120 {
                        input.viewport_height = Some(40.0 + frame as f32);
                        input.scroll_top = frame as f32;
                        input.selected_range = 2..8;
                        assert_eq!(input.layout_text(px(400.0), &style, window, cx), height);
                    }
                    assert_eq!(
                        input.layout_rebuilds, 1,
                        "resize/scroll/selection must reuse shaping"
                    );
                    input.set_text("Edited draft", cx);
                    input.layout_text(px(400.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 2);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 3, "width changes must rewrap");
                    style.font_size = px(18.0).into();
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 4);
                    input.marked_range = Some(0..2);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(
                        input.layout_rebuilds, 5,
                        "IME marking must repaint decoration"
                    );
                    input.unmark_text(window, cx);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 6, "IME unmark must also invalidate");
                    input.set_text("", cx);
                    input.layout_text(px(200.0), &style, window, cx);
                    input.set_placeholder("New placeholder", cx);
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 8);
                    style.color = gpui::rgb(0xff0000).into();
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 9);
                    input.enable_mentions();
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 10);
                    cx.set_global(Theme::light());
                    input.layout_text(px(200.0), &style, window, cx);
                    assert_eq!(input.layout_rebuilds, 11, "mention colors follow the theme");
                })
                .unwrap();
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }

    #[test]
    fn resize_reveals_only_complete_rows() {
        for visible in [0.0, 5.0, 22.0, 22.75, 30.0, 45.5, 70.0, 150.0] {
            let height = input_reveal_height(visible, 0.0, INPUT_LINE_HEIGHT, true);
            assert!(height <= visible);
            assert_eq!(height % INPUT_LINE_HEIGHT, 0.0);
        }
        // The row grid moves with scrolling; the clip still ends between rows.
        assert_eq!(input_reveal_height(39.0, 7.0, 20.0, true), 33.0);
        // Normal overflow scrolling keeps its full viewport and existing fades.
        assert_eq!(input_reveal_height(39.0, 7.0, 20.0, false), 39.0);
        assert_eq!(input_reveal_height(100.0, 0.0, 20.0, true), 100.0);
    }

    #[test]
    fn resize_keeps_text_anchored_to_the_input_origin() {
        // A fitting draft grows from one row to seven. Caret-follow must
        // never temporarily scroll earlier lines through the top clip.
        for visible in [0.0, 22.75, 60.0, 110.0, 159.25] {
            assert_eq!(
                input_scroll_offset_for_cursor(0.0, 136.5, 22.75, 159.25, visible, Some(159.25),),
                0.0
            );
        }
        // A genuinely overflowing draft keeps the same caret-follow offset
        // through every frame of the reveal, rather than chasing its height.
        for visible in [30.0, 100.0, 180.0, 240.0] {
            assert_eq!(
                input_scroll_offset_for_cursor(160.0, 377.25, 22.75, 400.0, visible, Some(240.0),),
                160.0
            );
        }
        // Deleting back to a fitting draft resets scroll immediately, even
        // while the old, larger viewport is still shrinking.
        assert_eq!(
            input_scroll_offset_for_cursor(160.0, 77.25, 22.75, 100.0, 240.0, Some(100.0),),
            0.0
        );
    }

    #[test]
    fn scroll_fade_ignores_temporary_resize_overflow() {
        for visible_height in [0.0, 20.0, 60.0, 100.0, 160.0] {
            let scroll = input_max_scroll(160.0, visible_height);
            assert_eq!(
                input_overflow_edges(160.0, 160.0, visible_height, scroll),
                (false, false)
            );
        }
        // Deleting a capped draft disables fading immediately, even while
        // its scroll position and outer height are still settling.
        assert_eq!(
            input_overflow_edges(100.0, 100.0, 240.0, 80.0),
            (false, false)
        );
    }

    #[test]
    fn scroll_fade_tracks_real_overflow_edges() {
        for (scroll, top, bottom) in [(0.0, false, true), (80.0, true, true), (160.0, true, false)]
        {
            assert_eq!(
                input_overflow_edges(400.0, 240.0, 240.0, scroll),
                (top, bottom)
            );
        }
    }

    #[test]
    fn content_resize_retargets_from_visible_height_and_settles() {
        let start = composer_total_height(input_content_height(3));
        let target = composer_total_height(input_content_height(6));
        let grow = flip_morph_step(None, true, start, 0.0, false, false).unwrap();
        let visible = grow.height(target, 60.0);
        assert!(visible > start && visible < target);
        // A delete during growth reverses from what is on screen, with no snap.
        let shrink = flip_morph_step(Some(grow), true, visible, 60.0, false, false).unwrap();
        assert_eq!(shrink.height(start, 60.0), visible);
        assert!(shrink.height(start, 120.0) < visible);
        assert_eq!(shrink.height(start, 240.0), start);
        assert_eq!(
            flip_morph_step(Some(shrink), false, start, 240.0, false, false),
            None
        );
        // Toggling reduced motion also cancels an already running resize.
        assert_eq!(
            flip_morph_step(Some(grow), false, visible, 60.0, true, false),
            None
        );
    }

    #[test]
    fn flip_morph_height_ramps_monotonically_to_target() {
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        // Starts exactly at the committed height…
        let mut prev = m.height(124.0, 0.0);
        assert_eq!(prev, 49.0);
        // …ramps without ever moving backwards…
        for step in 1..=18 {
            let h = m.height(124.0, step as f32 * 10.0);
            assert!(h >= prev, "height regressed at {step}: {h} < {prev}");
            prev = h;
        }
        // …and lands exactly on the target when done (and stays there).
        assert_eq!(m.height(124.0, 180.0), 124.0);
        assert!(m.done(180.0));
        assert_eq!(m.height(124.0, 500.0), 124.0);
        // Collapse runs the same ramp downward.
        assert!(m.height(124.0, 90.0) > 49.0);
        let down = FlipMorph {
            from: 124.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        assert!(down.height(49.0, 90.0) < 124.0);
        assert!(down.height(49.0, 90.0) > 49.0);
    }

    #[test]
    fn flip_morph_reverse_hands_off_from_current_height() {
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        let mid = m.height(124.0, 90.0);
        assert!(mid > 49.0 && mid < 124.0);
        // A reverse flip mid-flight commits a new morph FROM the animated
        // height — continuous at the handoff, no pop to an endpoint.
        let rev = flip_morph_step(Some(m), true, mid, 90.0, false, false).unwrap();
        assert_eq!(rev.from, mid);
        assert_eq!(rev.height(49.0, 90.0), mid);
    }

    #[test]
    fn flip_morph_snaps_for_reduced_motion_and_first_paint() {
        // Reduced motion never creates a morph (the flip just snaps)…
        assert_eq!(flip_morph_step(None, true, 49.0, 0.0, true, false), None);
        // …and neither does a flip before anything was ever rendered.
        assert_eq!(flip_morph_step(None, true, 0.0, 0.0, false, false), None);
    }

    #[test]
    fn route_change_never_arms_the_morph() {
        // A flip committed inside the route-snap window must NOT animate —
        // switching sessions (chat↔chat or chat↔new-session) snaps the
        // composer straight to the target mode, like the header (round 6).
        assert_eq!(flip_morph_step(None, true, 49.0, 0.0, false, true), None);
        // The route change also kills anything already in flight…
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        assert_eq!(
            flip_morph_step(Some(m), false, 80.0, 50.0, false, true),
            None
        );
        assert_eq!(
            flip_morph_step(Some(m), true, 80.0, 50.0, false, true),
            None
        );
        // …while outside the window the same flip animates as usual.
        let armed = flip_morph_step(None, true, 49.0, 300.0, false, false).unwrap();
        assert_eq!(armed.from, 49.0);
    }

    #[test]
    fn morph_anchoring_holds_controls_and_glides_text() {
        // Steady state (progress 1): no offsets, everything at rest.
        assert_eq!(morph_cluster_dy(1.0), 0.0);
        assert_eq!(morph_text_pad(1.0), 16.0);
        assert_eq!(collapse_text_glide(124.0, 1.0), 0.0);
        // At the commit instant the pieces start from the OLD mode's resting
        // geometry: text pad at the compact 12px inset, cluster displaced by
        // exactly the 4.5px centering delta.
        assert_eq!(morph_text_pad(0.0), 12.0);
        assert_eq!(morph_cluster_dy(0.0), CLUSTER_Y_DELTA);
        // Collapse glide: starts where the expanded text sat (17px below the
        // committed pill top → `from − 53` above the compact resting spot)…
        assert_eq!(collapse_text_glide(124.0, 0.0), 71.0);
        // …decays monotonically to zero…
        let mut prev = collapse_text_glide(124.0, 0.0);
        for step in 1..=10 {
            let g = collapse_text_glide(124.0, step as f32 / 10.0);
            assert!(g <= prev, "glide regressed at {step}");
            prev = g;
        }
        // …and can't go negative on shallow mid-flight reversals.
        assert_eq!(collapse_text_glide(50.0, 0.0), 0.0);
    }

    #[test]
    fn cluster_inset_glides_between_the_source_endpoints() {
        assert_eq!(ACTION_UTILITY_GAP, 2.0);
        assert_eq!(ACTION_PRIMARY_GAP, Theme::SPACE_SM);
        assert!(ACTION_UTILITY_GAP < ACTION_PRIMARY_GAP);
        // The morph starts from the OLD mode's resting inset (no sideways
        // step at the commit) and eases to the committed mode's…
        assert_eq!(morph_cluster_inset(true, 0.0), 8.0); // expand: from compact pr-2
        assert_eq!(morph_cluster_inset(true, 1.0), 12.0); // …to expanded px-3
        assert_eq!(morph_cluster_inset(false, 0.0), 12.0); // collapse: from px-3
        assert_eq!(morph_cluster_inset(false, 1.0), 8.0); // …to pr-2
        // …monotonically, bounded by the 4px source delta.
        let mut prev = morph_cluster_inset(true, 0.0);
        for step in 1..=10 {
            let v = morph_cluster_inset(true, step as f32 / 10.0);
            assert!(v >= prev && v <= 8.0 + CLUSTER_X_DELTA);
            prev = v;
        }
        // Internal group spacing is shared between modes — only this wrapper
        // inset may differ across the flip.
    }

    #[test]
    fn flip_morph_tracks_live_target_and_drives_fade() {
        let m = FlipMorph {
            from: 49.0,
            start_ms: 0.0,
            spec: motion::COLLAPSE,
        };
        // Auto-grow can move the target mid-morph: evaluation tracks the
        // live value instead of finishing on a stale height.
        assert!(m.height(159.0, 90.0) > m.height(124.0, 90.0));
        // The eased progress is the actions-row fade: 0 at commit, 1 at rest.
        assert_eq!(m.progress(0.0), 0.0);
        assert_eq!(m.progress(180.0), 1.0);
        let mid = m.progress(90.0);
        assert!(mid > 0.0 && mid < 1.0);
    }

    #[test]
    fn new_thread_route_changes_use_the_coordinated_timeline() {
        let m = FlipMorph::new_thread_transition(124.0, 0.0);
        assert_eq!(m.spec, motion::NEW_THREAD_TRANSITION);
        assert_eq!(m.height(49.0, 0.0), 124.0);
        assert!(m.height(49.0, 250.0) < 124.0);
        assert!(m.height(49.0, 250.0) > 49.0);
        assert_eq!(m.height(49.0, 420.0), 49.0);
        let reverse = FlipMorph::new_thread_transition(49.0, 0.0);
        assert_eq!(reverse.height(124.0, 0.0), 49.0);
        assert_eq!(reverse.height(124.0, 420.0), 124.0);
    }

    #[test]
    fn new_thread_selectors_restore_the_compact_floating_row() {
        assert_eq!(NEW_THREAD_SELECTOR_ROW_HEIGHT, 20.0);
        assert_eq!(SESSION_FOOTER_HEIGHT, 24.0);
    }

    #[test]
    fn route_chrome_crossfade_never_duplicates_picker_controls() {
        assert_eq!(route_chrome_opacities(1.0), (1.0, 0.0));
        assert_eq!(route_chrome_opacities(0.5), (0.0, 0.0));
        assert_eq!(route_chrome_opacities(0.0), (0.0, 1.0));
        for step in 0..=20 {
            let (new_thread, session) = route_chrome_opacities(step as f32 / 20.0);
            assert!(new_thread == 0.0 || session == 0.0);
        }
    }

    #[test]
    fn staged_comments_alone_are_content() {
        assert!(!composer_has_content("   ", 0, 0));
        assert!(composer_has_content("hi", 0, 0));
        assert!(composer_has_content("", 1, 0));
        assert!(composer_has_content("", 0, 1));
    }

    #[test]
    fn modified_submit_sends_content_and_activates_latest_queue_row_when_empty() {
        assert_eq!(
            modified_submit_target(composer_has_content("message", 0, 0)),
            ModifiedSubmitTarget::SubmitContent
        );
        assert_eq!(
            modified_submit_target(composer_has_content("", 1, 0)),
            ModifiedSubmitTarget::SubmitContent
        );
        assert_eq!(
            modified_submit_target(composer_has_content("", 0, 1)),
            ModifiedSubmitTarget::SubmitContent
        );
        assert_eq!(
            modified_submit_target(composer_has_content("  ", 0, 0)),
            ModifiedSubmitTarget::ActivateLatestQueued
        );
    }

    #[test]
    fn a_comment_only_stage_queues_during_a_live_run() {
        let live = true;
        let comment_only = composer_has_content("", 0, 2);
        assert_eq!(
            send_button_mode(live, comment_only),
            SendButtonMode::Queue,
            "comment-only submit must queue without interrupting the run"
        );
        // Nothing staged at all is still the stop square.
        assert_eq!(
            send_button_mode(live, composer_has_content("", 0, 0)),
            SendButtonMode::Stop
        );
    }

    #[test]
    fn send_button_morph() {
        assert_eq!(send_button_mode(false, false), SendButtonMode::Send);
        assert_eq!(send_button_mode(false, true), SendButtonMode::Send);
        assert_eq!(send_button_mode(true, true), SendButtonMode::Queue);
        assert_eq!(send_button_mode(true, false), SendButtonMode::Stop);
    }

    #[test]
    fn queued_submit_does_not_publish_an_optimistic_transcript_echo() {
        assert!(should_publish_optimistic_echo(false));
        assert!(!should_publish_optimistic_echo(true));
    }

    #[test]
    fn interrupt_tracking_is_idempotent_per_chat() {
        let mut pending = HashSet::new();
        assert!(begin_interrupt(&mut pending, "chat-a"));
        assert!(!begin_interrupt(&mut pending, "chat-a"));
        assert!(begin_interrupt(&mut pending, "chat-b"));
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn interrupt_tracking_releases_only_settled_chats() {
        let mut pending = HashSet::from(["chat-a".to_string(), "chat-b".to_string()]);
        retain_live_interrupts(&mut pending, |chat_id| chat_id == "chat-b");
        assert_eq!(pending, HashSet::from(["chat-b".to_string()]));
        assert!(begin_interrupt(&mut pending, "chat-a"));
    }

    #[test]
    fn interrupt_payload_keeps_the_captured_chat() {
        let params = interrupt_params("chat-a");
        assert_eq!(params["chatId"], "chat-a");
        assert_eq!(params["command"]["kind"], "interrupt");
    }

    #[test]
    fn escape_consumers_keep_completion_and_wizard_priority() {
        assert!(escape_dismisses_completion("escape", true));
        assert!(!escape_dismisses_completion("escape", false));
        assert!(!escape_dismisses_completion("enter", true));

        assert!(wizard_escape_goes_back("escape", false, false));
        assert!(wizard_escape_goes_back("escape", true, true));
        assert!(!wizard_escape_goes_back("escape", true, false));
        assert!(!wizard_escape_goes_back("enter", false, true));
    }

    #[test]
    fn wizard_single_select_auto_advances_and_completes() {
        let mut w = Wizard::new(
            "req".into(),
            vec![
                question("q1", &["a", "b"], false),
                question("q2", &["x"], false),
            ],
        );
        assert_eq!(w.counter(), "1/2");
        assert_eq!(w.select(1), WizardStep::AutoAdvance);
        assert!(w.is_picked(1));
        assert_eq!(w.advance(), WizardStep::Stay);
        assert_eq!(w.counter(), "2/2");
        assert_eq!(w.select(0), WizardStep::AutoAdvance);
        let WizardStep::Done(answers) = w.advance() else {
            panic!("expected Done")
        };
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].labels, vec!["b"]);
        assert_eq!(answers[1].labels, vec!["x"]);
    }

    #[test]
    fn wizard_multi_select_toggles_and_stays() {
        let mut w = Wizard::new("req".into(), vec![question("q", &["a", "b", "c"], true)]);
        assert_eq!(w.select(0), WizardStep::Stay);
        assert_eq!(w.select(2), WizardStep::Stay);
        assert!(w.is_picked(0) && w.is_picked(2));
        // Toggle off.
        assert_eq!(w.select(0), WizardStep::Stay);
        assert!(!w.is_picked(0));
        let WizardStep::Done(answers) = w.advance() else {
            panic!()
        };
        assert_eq!(answers[0].labels, vec!["c"]);
    }

    #[test]
    fn wizard_number_keys_and_bounds() {
        let mut w = Wizard::new("req".into(), vec![question("q", &["a", "b"], false)]);
        assert_eq!(w.press_number(9), WizardStep::Stay, "out of range ignored");
        assert_eq!(w.press_number(0), WizardStep::Stay);
        assert_eq!(w.press_number(2), WizardStep::AutoAdvance);
        assert!(w.is_picked(1));
        assert_eq!(w.select(5), WizardStep::Stay, "bad option ix ignored");
    }

    #[test]
    fn wizard_typed_answer_overrides_and_back_pages() {
        let mut w = Wizard::new(
            "req".into(),
            vec![
                question("q1", &["a"], false),
                question("q2", &["x", "y"], false),
            ],
        );
        w.select(0);
        w.advance();
        assert_eq!(w.page, 1);
        assert!(w.back());
        assert_eq!(w.page, 0);
        assert!(!w.back(), "already at first page");
        w.advance();
        w.set_typed("  custom answer  ".into());
        let WizardStep::Done(answers) = w.advance() else {
            panic!()
        };
        assert_eq!(answers[0].labels, vec!["a"]);
        assert_eq!(
            answers[1].labels,
            vec!["custom answer"],
            "typed overrides picked, trimmed"
        );
    }

    #[gpui::test]
    fn composition_drops_and_atomic_chip_edits_preserve_rich_draft_history(
        cx: &mut gpui::TestAppContext,
    ) {
        fn assert_ranges(input: &ComposerInput) {
            for range in std::iter::once(&input.selected_range).chain(input.marked_range.iter()) {
                assert!(range.start <= range.end && range.end <= input.content.len());
                assert!(input.content.is_char_boundary(range.start));
                assert!(input.content.is_char_boundary(range.end));
            }
            for (chip, display) in &input.projection.mentions {
                assert!(input.content.get(chip.range.clone()).is_some());
                assert!(input.projection.display.get(display.clone()).is_some());
            }
        }
        with_composer_input(cx, |input, window, cx| {
            let chip = local_file_link("src/café.rs", false);
            let raw = format!("**bold** e\u{301} 👨‍👩‍👧‍👦 {chip}\n`{chip}`\nactive ");
            input.set_text(&raw, cx);
            assert_eq!(input.projection.mentions.len(), 1);
            input.replace_and_mark_text_in_range(None, "かな", Some(0..1), window, cx);
            let composed = input.text().to_owned();
            let selection = input.selected_range.clone();
            let marked = input.marked_range.clone();
            let history = input.undo_stack.len();
            assert!(!input.insert_dropped_mention("src/dropped.rs", false, cx));
            assert_eq!(input.text(), composed);
            assert_eq!(input.selected_range, selection);
            assert_eq!(input.marked_range, marked);
            assert_eq!(input.undo_stack.len(), history);
            assert_ranges(input);
            input.replace_and_mark_text_in_range(None, "かんじ", Some(1..2), window, cx);
            assert_eq!(&input.content[input.selected_range.clone()], "ん");
            assert_ranges(input);
            input.replace_text_in_range(None, "漢字", window, cx);
            let committed = format!("{raw}漢字");
            assert_eq!(input.text(), committed);
            assert!(input.marked_range.is_none());
            assert_ranges(input);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), raw);
            input.redo(&Redo, window, cx);
            assert_eq!(input.text(), committed);
            assert_ranges(input);
            let link = input.projection.mentions[0].0.range.clone();
            let partial = input.range_to_utf16(&(link.start + 1..link.end - 1));
            input.replace_text_in_range(Some(partial), "🦀", window, cx);
            assert!(input.projection.mentions.is_empty());
            assert!(input.text().contains(&format!("`{chip}`")));
            assert_ranges(input);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), committed);
            assert_eq!(input.projection.mentions.len(), 1);
            input.move_to(input.content.len(), cx);
            assert!(input.insert_dropped_mention("src/dropped.rs", false, cx));
            assert_eq!(input.projection.mentions.len(), 2);
            assert_ranges(input);
            input.undo(&Undo, window, cx);
            assert_eq!(input.text(), committed);
            assert_ranges(input);
        });
    }

    #[test]
    fn pending_input_detection() {
        use zeron_doc::MessageStatus;
        let input_part = MessagePart::Input {
            id: "in-r1".into(),
            request_id: "r1".into(),
            questions: vec![question("q", &["a"], false)],
            resolved: false,
        };
        let entry = |status: Option<MessageStatus>, parts: Vec<MessagePart>| SessionMessageEntry {
            id: "m".into(),
            role: MessageRole::Assistant,
            parts,
            created_at: 0,
            device_id: "d".into(),
            status,
            continuation_of: None,
            duration_ms: None,
        };
        // Streaming entry with unresolved input → panel.
        let t = vec![entry(
            Some(MessageStatus::Streaming),
            vec![input_part.clone()],
        )];
        assert_eq!(
            pending_input_request(&t).map(|(id, _)| id),
            Some("r1".into())
        );
        // DEAD entry with an unresolved input STILL gets the panel: the
        // question stays answerable until answered (the engine delivers the
        // answer as a resumed turn), so a run reaped under its question —
        // engine restart — must not orphan it (user report).
        let t = vec![entry(
            Some(MessageStatus::Aborted),
            vec![input_part.clone()],
        )];
        assert_eq!(
            pending_input_request(&t).map(|(id, _)| id),
            Some("r1".into())
        );
        // A NEWER assistant entry supersedes an unanswered question.
        let t = vec![
            entry(Some(MessageStatus::Aborted), vec![input_part.clone()]),
            SessionMessageEntry {
                id: "m2".into(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::Text {
                    id: "t2".into(),
                    text: "moved on".into(),
                }],
                created_at: 2,
                device_id: "d".into(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            },
        ];
        assert!(pending_input_request(&t).is_none());
        // Resolved part → no panel.
        let resolved = MessagePart::Input {
            id: "in-r1".into(),
            request_id: "r1".into(),
            questions: vec![],
            resolved: true,
        };
        let t = vec![entry(
            Some(MessageStatus::Streaming),
            vec![resolved.clone()],
        )];
        assert!(pending_input_request(&t).is_none());
        assert!(pending_input_request(&[]).is_none());

        // Regression (user forensics): a steer prompt appends a USER entry
        // AFTER the streaming assistant entry — the question must still be
        // found (a last-entry-only read vanished the panel exactly when the
        // user typed, bricking the answer flow).
        let user_echo = SessionMessageEntry {
            id: "u2".into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t".into(),
                text: "I answered".into(),
            }],
            created_at: 1,
            device_id: "d".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        };
        let t = vec![
            entry(Some(MessageStatus::Streaming), vec![input_part.clone()]),
            user_echo,
        ];
        assert_eq!(
            pending_input_request(&t).map(|(id, _)| id),
            Some("r1".into()),
            "question survives entries appended behind the streaming entry"
        );

        // Latch release: only an explicitly resolved matching part releases.
        assert!(!input_request_resolved(&t, "r1"));
        let t = vec![entry(Some(MessageStatus::Streaming), vec![resolved])];
        assert!(input_request_resolved(&t, "r1"));
        assert!(!input_request_resolved(&t, "other"));
    }
}

#[cfg(test)]
mod appshot_rebase_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn appshot_only_draft_counts_as_content_and_removal_clears_it(cx: &mut TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let shot = appshots::tests::shot();
            composer.stage_appshot(shot.clone(), cx);
            assert!(composer_has_content(
                "",
                composer.staged().len() + composer.staged_appshots().len(),
                0
            ));
            composer.remove_appshot(&shot.id, cx);
            assert!(composer.staged_appshots().is_empty());
            assert!(!composer_has_content(
                "",
                composer.staged().len() + composer.staged_appshots().len(),
                0
            ));
        });
    }

    #[gpui::test]
    fn failed_send_restores_complete_appshots_without_duplicates(cx: &mut TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let original = appshots::tests::shot();
            let mut fresh = original.clone();
            fresh.id = "fresh".into();
            composer.stage_appshot_for("minted".into(), original.clone(), cx);
            composer.stage_appshot(fresh, cx);
            composer.restore_failed_appshots(&[original.clone()], "minted", "");
            assert_eq!(composer.staged_appshots().len(), 2);
            assert_eq!(
                composer.staged_appshots()[0].accessibility,
                original.accessibility
            );
            assert_eq!(
                composer.staged_appshots()[0].screenshot.id,
                original.screenshot.id
            );
            assert_eq!(composer.staged_appshots()[1].id, "fresh");
            assert!(!composer.appshots.contains_key("minted"));
        });
    }

    #[gpui::test]
    fn remote_queue_removal_recovers_both_appshot_drafts(cx: &mut TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let original = appshots::tests::shot();
            let mut edited = original.clone();
            edited.id = "edited".into();
            composer.stage_appshot(edited, cx);
            composer.queue_edit_draft = Some(("original".into(), vec![], vec![original]));
            composer.editing_queued = Some("removed-row".into());
            composer
                .input
                .update(cx, |input, cx| input.set_text("edited", cx));
            composer.on_state_changed(cx);
            assert!(composer.editing_queued.is_none());
            assert_eq!(composer.staged_appshots().len(), 2);
            assert_eq!(composer.input.read(cx).text(), "original\n\nedited");
        });
    }
}

#[cfg(feature = "appshots-fixture")]
impl Composer {
    pub fn fixture_clear_appshots(&mut self, cx: &mut Context<Self>) {
        self.appshots.clear();
        cx.notify();
    }
}
