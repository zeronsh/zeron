//! Popover / menu primitives: an anchored floating layer with the `menu-in`
//! animation, outside-click dismissal, and pure keyboard-navigation + search
//! reducers shared by every picker and menu (feature-inventory §1.12 popovers).
//!
//! gpui pattern (examples/popover.rs at the pinned rev): the trigger element
//! conditionally children a `deferred(anchored().child(content))` — deferred
//! paints on a floating layer above everything, anchored positions it relative
//! to the trigger (or an explicit point for context menus).
//!
//! Pure logic (wrap-around list navigation, ranked substring filtering, key
//! classification) lives in free functions with unit tests; the elements only
//! feed them measurements/events.

mod contained;
mod hover_intent;
pub(crate) use contained::contained_menu;
pub use hover_intent::{HoverAction, HoverIntent};

use gpui::{
    Anchor, AnyElement, Context, Div, ElementId, IntoElement, MouseButton, MouseDownEvent,
    MouseUpEvent, Pixels, Point, ScrollHandle, SharedString, Stateful, Window, div, prelude::*, px,
};
use std::time::{Duration, Instant};

use crate::motion::{self, ZERON_PULSE};
use crate::theme::{Theme, hairline, ink};

// ---------------------------------------------------------------------------
// Loadable — async slot state shared by pickers/settings pages
// ---------------------------------------------------------------------------

/// One async-loaded slot: `Idle` (never requested) → `Loading` (skeletons) →
/// `Ready` / `Error` (inline message + Retry).
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Error(String),
}

impl<T> Loadable<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Loadable::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Loadable::Loading)
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Loadable::Error(message) => Some(message),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Popup — open/closing/closed lifecycle (exit animations)
// ---------------------------------------------------------------------------

/// Popup state with an exit phase. gpui unmounts an element the frame its
/// state drops, so a closing animation needs the state held alive while
/// [`motion::menu_out`] plays: `open` → `begin_close` (render keeps mounting,
/// with the out animation and dead hit-testing) → [`reap_popup`]'s timer
/// `finish_close`es ~[`motion::MENU_OUT`] later. Use [`Self::is_open`] for
/// logic (a closing popup already reads as closed) and [`Self::get`] /
/// [`Self::is_closing`] for rendering.
pub struct Popup<T> {
    /// `Some((state, closing_since))` while mounted; `closing_since` is the
    /// exit-phase start.
    inner: Option<(T, Option<std::time::Instant>)>,
    /// Whether the popup was still mounted when the current trigger press
    /// began — see [`Self::note_trigger_press`].
    pressed_while_open: bool,
}

impl<T> Default for Popup<T> {
    fn default() -> Self {
        Self {
            inner: None,
            pressed_while_open: false,
        }
    }
}

impl<T> Popup<T> {
    pub fn open(&mut self, value: T) {
        self.inner = Some((value, None));
    }

    /// Open and interactive (not closing).
    pub fn is_open(&self) -> bool {
        matches!(self.inner, Some((_, None)))
    }

    pub fn is_closing(&self) -> bool {
        matches!(self.inner, Some((_, Some(_))))
    }

    /// When the exit phase began — what the render path hands to the popover
    /// wrappers, which derive the eased exit progress from it each frame.
    pub fn closing_since(&self) -> Option<std::time::Instant> {
        match &self.inner {
            Some((_, Some(since))) => Some(*since),
            _ => None,
        }
    }

    /// The state while mounted — open OR playing the exit animation. Render
    /// paths use this; logic paths use [`Self::as_open`]/[`Self::open_mut`].
    pub fn get(&self) -> Option<&T> {
        self.inner.as_ref().map(|(value, _)| value)
    }

    /// The state only while genuinely open — `None` during the exit phase, so
    /// event handlers on a dying popup fall through.
    pub fn as_open(&self) -> Option<&T> {
        match &self.inner {
            Some((value, None)) => Some(value),
            _ => None,
        }
    }

    pub fn open_mut(&mut self) -> Option<&mut T> {
        match &mut self.inner {
            Some((value, None)) => Some(value),
            _ => None,
        }
    }

    /// Enter the exit phase. Returns `true` when this call started it (the
    /// caller then schedules [`reap_popup`]); `false` if already closing or
    /// closed.
    pub fn begin_close(&mut self) -> bool {
        match &mut self.inner {
            Some((_, closing @ None)) => {
                *closing = Some(std::time::Instant::now());
                true
            }
            _ => false,
        }
    }

    /// Record, from the trigger's `on_mouse_down`, whether this popup is
    /// still mounted. The anchored card's `on_mouse_down_out` fires on that
    /// same press and begins the close, so by click (mouse-up) time the
    /// popup already reads as closed — the click handler alone cannot tell
    /// "this press dismissed it; stay closed" from "open fresh", and a
    /// plain toggle closes-and-reopens (user report). Both handler orders
    /// work: open and mid-exit each count as mounted. Every trigger click
    /// is preceded by a trigger mouse-down, so the note is never stale.
    pub fn note_trigger_press(&mut self) {
        self.note_trigger_press_matching(|_| true);
    }

    /// [`Self::note_trigger_press`] for popups whose state distinguishes
    /// which trigger owns them (e.g. one `Popup<PickerKind>` shared by
    /// several triggers): only a press on the OWNING trigger counts, so
    /// clicking a different trigger switches menus instead of swallowing.
    pub fn note_trigger_press_matching(&mut self, owns: impl FnOnce(&T) -> bool) {
        self.pressed_while_open = self.inner.as_ref().is_some_and(|(value, _)| owns(value));
    }

    /// Consume the press note: `true` when the press that produced the
    /// current click found the popup mounted — the click should leave it
    /// closed rather than reopen it.
    pub fn take_press_was_open(&mut self) -> bool {
        std::mem::take(&mut self.pressed_while_open)
    }

    /// Drop the state if the exit phase has run its course. A popup reopened
    /// (or re-closed) since the matching [`begin_close`] is left alone — the
    /// newer phase's own reap handles it.
    pub fn finish_close(&mut self) {
        if let Some((_, Some(since))) = &self.inner
            && since.elapsed() >= motion::MENU_OUT.total().mul_f32(motion::speed_scale())
        {
            self.inner = None;
        }
    }
}

/// Schedule the reap for a [`Popup::begin_close`]: after the exit animation's
/// span, drop the popup state and repaint. `popup` re-borrows the field from
/// the view (the state can't be captured — the view owns it).
pub fn reap_popup<V: 'static, T: 'static>(
    cx: &mut gpui::Context<V>,
    popup: impl Fn(&mut V) -> &mut Popup<T> + 'static,
) {
    cx.spawn(async move |view, cx| {
        cx.background_executor()
            .timer(
                motion::MENU_OUT
                    .total()
                    .mul_f32(motion::speed_scale())
                    .saturating_add(std::time::Duration::from_millis(20)),
            )
            .await;
        view.update(cx, |view, cx| {
            popup(view).finish_close();
            cx.notify();
        })
        .ok();
    })
    .detach();
}

// ---------------------------------------------------------------------------
// Pure reducers
// ---------------------------------------------------------------------------

/// Step the active row of a menu: wraps at both ends; `None` enters at the
/// edge matching the direction. Empty menus stay `None`.
pub fn menu_step(active: Option<usize>, count: usize, delta: isize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let count_i = count as isize;
    let next = match active {
        None => {
            if delta >= 0 {
                0
            } else {
                count_i - 1
            }
        }
        Some(at) => (at as isize + delta).rem_euclid(count_i),
    };
    Some(next as usize)
}

/// Match rank of a label against a query: `0` prefix match, `1` substring,
/// `None` no match. Case-insensitive; an empty query matches everything at
/// rank 1 (input order preserved).
pub fn match_rank(query: &str, label: &str) -> Option<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(1);
    }
    let label = label.to_lowercase();
    if label.starts_with(&query) {
        Some(0)
    } else if label.contains(&query) {
        Some(1)
    } else {
        None
    }
}

/// Filter + rank labels for a search query: prefix matches first, then
/// substring matches, stable within each rank. Returns indices into `labels`.
pub fn filter_indices<S: AsRef<str>>(query: &str, labels: &[S]) -> Vec<usize> {
    let mut ranked: Vec<(usize, usize)> = labels
        .iter()
        .enumerate()
        .filter_map(|(ix, label)| match_rank(query, label.as_ref()).map(|rank| (rank, ix)))
        .collect();
    ranked.sort_by_key(|&(rank, ix)| (rank, ix));
    ranked.into_iter().map(|(_, ix)| ix).collect()
}

/// Keys the pickers care about, classified from a raw keystroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    Up,
    Down,
    /// Plain Enter — activate the highlighted row.
    Enter,
    /// Cmd/Ctrl+Enter — the "pick this folder" accelerator in the browser.
    ModEnter,
    Escape,
    Backspace,
    Other,
}

pub fn classify_key(key: &str, cmd: bool, ctrl: bool) -> MenuKey {
    match key {
        "up" => MenuKey::Up,
        "down" => MenuKey::Down,
        // Readline/emacs motion: ctrl-n/ctrl-p mirror ↓/↑ in every picker.
        // Safe to claim frame-wide — neither chord is a text-editing binding
        // in the palette keymaps, so they always bubble here unconsumed.
        "n" if ctrl => MenuKey::Down,
        "p" if ctrl => MenuKey::Up,
        "enter" if cmd || ctrl => MenuKey::ModEnter,
        "enter" => MenuKey::Enter,
        "escape" => MenuKey::Escape,
        "backspace" => MenuKey::Backspace,
        _ => MenuKey::Other,
    }
}

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

/// The floating-menu surface (zeron `.glass-surface` + `menuSurface`):
/// Shared floating surface used by palettes, popovers, dropdowns and menus.
/// Mount helpers supply the same 16px backdrop blur as the composer.
/// Corner radius must match the frost wrapper's mask.
pub const CARD_RADIUS: f32 = 12.0;

pub const MENU_GAP: f32 = 2.0;
/// The four-pixel inset of [`popover_card`] that [`menu_scroll_host`] /
/// [`menu_scroll_list`] cancel for card-bleeding scroll hosts.
pub const CARD_INSET: f32 = 4.0;
/// Concentric corners: rows sit inside both the card's 1px border and padding.
pub const MENU_ITEM_RADIUS: f32 = CARD_RADIUS - 1.0 - CARD_INSET;
pub const PALETTE_ITEM_RADIUS: f32 = 14.0 - CARD_INSET;

pub fn surface_bg(theme: &Theme) -> gpui::Hsla {
    if theme.is_frost() {
        if matches!(theme.appearance, crate::theme::Appearance::Dark) {
            theme.composer_sidebar_tint()
        } else {
            theme.glass_overlay()
        }
    } else {
        theme.input_glass_bg()
    }
}

pub fn popover_card(theme: &Theme) -> gpui::Div {
    div()
        .border_1()
        .border_color(theme.border)
        .rounded(px(CARD_RADIUS))
        .when(!theme.is_frost(), |el| el.shadow_lg())
        .bg(surface_bg(theme))
        .p(px(CARD_INSET))
        .gap(px(MENU_GAP))
        .overflow_hidden()
        .text_size(crate::typography::ui_rems(13.0))
        .text_color(theme.text)
}

/// [`popover_card`] without the shared inset — for popovers that manage their
/// own internal panes (the harness/model picker's rail + list split).
pub fn popover_card_flush(theme: &Theme) -> gpui::Div {
    popover_card(theme).p(px(0.0))
}

/// Host half of the card-bleed scroll treatment: bleed the rail to the card
/// edge; the inner list ([`menu_scroll_list`]) re-pads so rows stay put.
/// The rail mounts as a SIBLING of the scroller, above its clip — a rail
/// inside the scroller would scroll away with the content. Geometry only:
/// chain the view's own listeners (`.on_hover` for the list-hover note) onto
/// the returned element, plus `.my(px(-CARD_INSET))` when the card carries
/// content above the list and the bleed must run vertically too.
pub fn menu_scroll_host(id: &'static str) -> Stateful<Div> {
    div().id(id).relative().mx(px(-CARD_INSET))
}

/// List half of the card-bleed treatment (see [`menu_scroll_host`]): the
/// re-padded scroller itself. Chain the height budget (`.max_h`), layout
/// (`.flex().flex_col()`), and row children.
pub fn menu_scroll_list(id: &'static str, scroll: &ScrollHandle) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(CARD_INSET))
        .overflow_y_scroll()
        .track_scroll(scroll)
}

/// Completion surfaces share the picker card, inset and scroll fade. Keep
/// rails outside this wrapper so fading text never fades the scrollbar.
pub fn completion_card(theme: &Theme) -> Div {
    popover_card(theme).w_full().max_h(px(320.0))
}

pub fn completion_list(
    id: &'static str,
    scroll: &ScrollHandle,
    rows: impl IntoIterator<Item = AnyElement>,
) -> crate::edge_fade::EdgeFaded {
    faded_menu_list(
        scroll,
        menu_scroll_list(id, scroll)
            .max_h(px(310.0))
            .flex()
            .flex_col()
            .gap(px(MENU_GAP))
            .children(rows),
    )
}

/// All picker lists use the same paint-time, overflow-dependent edge fades.
pub fn faded_menu_list(
    scroll: &ScrollHandle,
    list: impl IntoElement,
) -> crate::edge_fade::EdgeFaded {
    crate::edge_fade::edge_faded(12.0, true, true, list).fade_overflow_y(scroll)
}

/// Shared completion-row typography and shrink rules. Long skill names and
/// paths must truncate inside the card rather than push the detail offscreen.
pub fn completion_row_content(
    theme: &Theme,
    icon: AnyElement,
    label: SharedString,
    detail: SharedString,
) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(div().size(px(16.0)).flex_none().child(icon))
        .child(
            div()
                .flex_none()
                .when(detail.is_empty(), |label| label.flex_1().min_w_0())
                .when(!detail.is_empty(), |label| {
                    label.max_w(gpui::relative(0.55))
                })
                .overflow_hidden()
                .truncate()
                .text_size(crate::typography::ui_rems(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(label),
        )
        .when(!detail.is_empty(), |row| {
            row.child(
                div()
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.5))
                    .text_color(theme.text_muted)
                    .child(detail),
            )
        })
}

/// Pin a floating layer's origin to the trigger's top-left. The anchored
/// element is absolutely positioned; without explicit insets its *static*
/// position is subject to the trigger's own flex alignment (an `items_center`
/// trigger would vertically center the whole floating layer). A zero-size
/// absolutely-inset wrapper fixes the origin at the corner.
fn pinned_layer(layer: AnyElement) -> AnyElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_0()
        .child(layer)
        .into_any_element()
}

/// Eased exit progress (0..=1) for a [`Popup`] closing instant, computed from
/// the wall clock at render time. Monotonic by construction — unlike the
/// animation element's own clock, it can never replay from 0 mid-exit.
fn exit_progress(since: std::time::Instant) -> f32 {
    let total = motion::MENU_OUT
        .total()
        .mul_f32(motion::speed_scale())
        .as_secs_f32();
    let raw = if total <= 0.0 {
        1.0
    } else {
        (since.elapsed().as_secs_f32() / total).clamp(0.0, 1.0)
    };
    motion::MENU_OUT.progress(raw)
}

/// The frosted card for a popover layer: full blur while open; while exiting
/// the blur radius rides the exit progress down to 0 — the `BackdropBlur`
/// primitive ignores `element_opacity`, so without this the glass slab would
/// hold full strength through the fade and pop off at unmount.
fn frosted_menu(exit: Option<f32>, content: AnyElement) -> AnyElement {
    let blur = crate::frost::MENU_BLUR * (1.0 - exit.unwrap_or(0.0));
    // Outside-dismiss listeners run during capture. Consume that same press
    // during bubble, after dismissal, so content behind the menu cannot act
    // on it too. The following click can reach that content normally.
    let guard = gpui::canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            window.on_mouse_event(move |event: &gpui::MouseDownEvent, phase, _, cx| {
                if phase == gpui::DispatchPhase::Bubble && !bounds.contains(&event.position) {
                    cx.stop_propagation();
                }
            });
        },
    )
    .absolute()
    .inset_0();
    crate::frost::frosted(
        CARD_RADIUS,
        blur,
        div()
            .relative()
            .child(guard)
            .child(content)
            .into_any_element(),
    )
    .into_any_element()
}

/// Entrance or exit motion for a popover layer. While exiting (the [`Popup`]
/// closing phase, `exit = Some(progress)`) the content plays
/// [`motion::menu_out`] under a fresh animation id (same-id reuse would
/// inherit the entrance's finished clock and snap to the end state) and gets
/// an occluding overlay on top — the dying menu's rows must not take clicks,
/// and the overlay also keeps stray clicks from reaching whatever sits
/// underneath.
fn menu_motion(id: SharedString, exit: Option<f32>, inner: gpui::Div) -> AnyElement {
    if let Some(t) = exit {
        let inner = inner.relative().child(div().absolute().inset_0().occlude());
        motion::menu_out(SharedString::from(format!("{id}-out")), t, inner).into_any_element()
    } else {
        motion::menu_in(id, inner).into_any_element()
    }
}

/// Wrap popover content in a floating anchored layer attached to the trigger:
/// the caller `.child(anchored_menu(...))`s this from the trigger element while
/// open. Plays `menu-in` (0.14s fade + 2px drop); `closing` (the [`Popup`]
/// exit phase) swaps in `menu-out`. Dismissal is the caller's
/// `.on_mouse_down_out` on the content. The layer `.occlude()`s: hitboxes are
/// paint-order only in gpui, so without it clicks on menu rows would ALSO fire
/// whatever clickable sits under the floating layer.
pub fn anchored_menu(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    pinned_layer(
        gpui::deferred(
            gpui::anchored()
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(px(8.0))
                .child(menu_motion(
                    id.into(),
                    exit,
                    div().occlude().pt(px(6.0)).child(content),
                )),
        )
        .priority(1)
        .into_any_element(),
    )
}

/// [`anchored_menu`] opening DOWNWARD from the trigger's bottom edge — a
/// dropdown proper (the sidebar's space filter). The default variant pins to
/// the trigger's top-left, which reads fine for context-style menus but
/// covers a button-shaped trigger.
pub fn anchored_menu_below(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    anchored_menu_below_gap(id, content, closing, 6.0)
}

/// [`anchored_menu_below`] right-aligned to the trigger's right edge. This is
/// the dropdown counterpart to [`anchored_menu_above_end`]: trailing sidebar
/// controls can open a full-width card leftward without leaving the sidebar.
pub fn anchored_menu_below_end(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .bottom_0()
        .right_0()
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(
                        id.into(),
                        exit,
                        div().occlude().pt(px(6.0)).child(content),
                    )),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// Open a top-level menu beside the trigger, clamped to the window.
pub fn anchored_menu_right(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .top_0()
        .right(px(-6.0))
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(id.into(), exit, div().occlude().child(content))),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// A nested menu beside its trigger. Callers choose the side that has room;
/// vertical placement still stays within the window's eight-pixel gutter.
pub fn nested_menu(id: impl Into<SharedString>, content: AnyElement, left: bool) -> AnyElement {
    // A nested menu shares the parent's interaction surface. Its outside
    // clicks must reach sibling controls and its trigger; the top-level menu
    // still consumes dismissal clicks before they reach the app underneath.
    let content =
        crate::frost::frosted(CARD_RADIUS, crate::frost::MENU_BLUR, content).into_any_element();
    div()
        .absolute()
        .top_0()
        .size_0()
        .when(left, |el| el.left(px(-(CARD_INSET + 6.0))))
        .when(!left, |el| el.right(px(-(CARD_INSET + 6.0))))
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(if left {
                        Anchor::TopRight
                    } else {
                        Anchor::TopLeft
                    })
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(id.into(), None, div().occlude().child(content))),
            )
            .priority(2),
        )
        .into_any_element()
}

/// [`anchored_menu_below`] with a caller-chosen trigger→card gap — the
/// changes-header dropdowns hang off a tight titlebar band and need more
/// breathing room than the default 6px (user report; t3code sits near 10).
pub fn anchored_menu_below_gap(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
    gap: f32,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .bottom_0()
        .left_0()
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(
                        id.into(),
                        exit,
                        div().occlude().pt(px(gap)).child(content),
                    )),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// [`anchored_menu`] opening UPWARD from the trigger (composer pickers, the
/// user menu — anything anchored near the window bottom; Radix flips these
/// automatically, gpui's `anchored` needs the side picked).
pub fn anchored_menu_above(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    pinned_layer(
        gpui::deferred(
            gpui::anchored()
                .anchor(Anchor::BottomLeft)
                .snap_to_window_with_margin(px(8.0))
                .child(menu_motion(
                    id.into(),
                    exit,
                    div().occlude().pb(px(6.0)).child(content),
                )),
        )
        .priority(1)
        .into_any_element(),
    )
}

/// Open an upward menu at a point inside a relative trigger. Useful for text
/// completions, whose natural anchor is the token/caret rather than the input
/// element's outer edge.
pub fn anchored_menu_above_at(
    id: impl Into<SharedString>,
    position: Point<Pixels>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    div()
        .absolute()
        .left(position.x)
        .top(position.y)
        .size_0()
        .child(anchored_menu_above(id, content, closing))
        .into_any_element()
}

pub fn full_width_menu_above(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .bottom_full()
        .left_0()
        .right_0()
        .child(
            gpui::deferred(menu_motion(
                id.into(),
                exit,
                div().occlude().pb(px(6.0)).child(content),
            ))
            .priority(1),
        )
        .into_any_element()
}

/// [`anchored_menu_above`] right-aligned to the trigger's right edge (t3code
/// ComboboxPopup `align="end"` — right-side triggers like the composer's ref
/// picker open leftward instead of running off the window).
pub fn anchored_menu_above_end(
    id: impl Into<SharedString>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    div()
        .absolute()
        .top_0()
        .right_0()
        .size_0()
        .child(
            gpui::deferred(
                gpui::anchored()
                    .anchor(Anchor::BottomRight)
                    .snap_to_window_with_margin(px(8.0))
                    .child(menu_motion(
                        id.into(),
                        exit,
                        div().occlude().pb(px(6.0)).child(content),
                    )),
            )
            .priority(1)
            .into_any_element(),
        )
        .into_any_element()
}

/// A floating menu at an explicit window position (context menus). Occludes
/// like [`anchored_menu`] so row clicks never reach elements underneath.
pub fn menu_at(
    id: impl Into<SharedString>,
    position: Point<Pixels>,
    content: AnyElement,
    closing: Option<std::time::Instant>,
) -> AnyElement {
    let exit = closing.map(exit_progress);
    let content = frosted_menu(exit, content);
    gpui::deferred(
        gpui::anchored()
            .position(position)
            .anchor(Anchor::TopLeft)
            .snap_to_window_with_margin(px(8.0))
            .child(menu_motion(id.into(), exit, div().occlude().child(content))),
    )
    .priority(1)
    .into_any_element()
}

/// Modal/overlay scrim at the *current* appearance, quoted in dark-mode terms
/// like [`ink`]/[`hairline`] — for callers (`modal`, the attachment lightbox)
/// that paint from a `deferred`/`anchored` layer with no `Theme`/`cx` in
/// scope. Mirrors [`Theme::scrim`], which is pinned at `X = 0.6` dark /
/// `0.32` light; other dark-mode alphas scale the light side by the same
/// ratio so the *dark* result is always exactly `alpha_dark` (never routed
/// through [`Hsla::opacity`], whose `0..=1` clamp would clip a
/// larger-than-0.6 alpha before it could scale the light side).
pub(crate) fn scrim_alpha(alpha_dark: f32) -> gpui::Hsla {
    crate::theme::scrim(alpha_dark)
}

/// Full-window modal: dim scrim + centered card with the `dialog-in` entrance.
/// The scrim swallows clicks; the caller wires its own dismiss/confirm.
/// `viewport` is the window size (an `anchored` layer sizes to its children,
/// so the scrim needs explicit dimensions). The frost radius matches
/// [`dialog_card`]'s 16px rounding.
pub fn modal(
    id: impl Into<ElementId>,
    viewport: gpui::Size<Pixels>,
    card: AnyElement,
) -> AnyElement {
    modal_with(id, viewport, card, 16.0, 0.35)
}

/// [`modal`] with custom rounding for glass palettes. Both use a light
/// scrim so the blurred backdrop retains its hue instead of becoming gray.
/// `corner_radius` must match the card's rounding.
pub fn modal_glass(
    id: impl Into<ElementId>,
    viewport: gpui::Size<Pixels>,
    card: AnyElement,
    corner_radius: f32,
) -> AnyElement {
    modal_with(id, viewport, card, corner_radius, 0.35)
}

fn modal_with(
    id: impl Into<ElementId>,
    viewport: gpui::Size<Pixels>,
    card: AnyElement,
    corner_radius: f32,
    scrim: f32,
) -> AnyElement {
    let card =
        crate::frost::frosted(corner_radius, crate::frost::MENU_BLUR, card).into_any_element();
    gpui::deferred(
        gpui::anchored()
            .position(gpui::point(px(0.0), px(0.0)))
            .child(
                div()
                    .occlude()
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .w(viewport.width)
                    .h(viewport.height)
                    .bg(scrim_alpha(scrim))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(motion::dialog_in(id, div().child(card))),
            ),
    )
    .priority(2)
    .into_any_element()
}

/// One menu row (zeron `menuItem`): `gap-2.5 rounded-lg px-2 py-1.5
/// text-[13px]`, active = `bg-white/10 text-foreground`, hover wash
/// `white/[0.08]` fading over `transition-colors` (floating-styles.ts) via the
/// per-`fade_key` [`motion::hover_blend`]. The caller adds the id/click
/// listener — `fade_key` must be unique app-wide and stable across frames
/// (the id string is a good choice).
pub fn menu_row(theme: &Theme, active: bool, fade_key: impl Into<SharedString>) -> gpui::Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(MENU_ITEM_RADIUS))
        .text_size(crate::typography::ui_rems(13.0))
        .cursor_pointer();
    if active {
        row.bg(crate::theme::card_selected_bg())
            .text_color(theme.text)
    } else {
        let fade_key = fade_key.into();
        let mut row = row
            .text_color(motion::hover_blend(
                &fade_key,
                theme.text.opacity(0.9),
                theme.text,
            ))
            .bg(motion::hover_blend(
                &fade_key,
                crate::theme::wash(0.0),
                crate::theme::card_selected_bg(),
            ));
        // Imperative form — the caller's `.id(...)` makes the element stateful
        // (hover listeners need element state, `.on_hover` needs `Stateful`).
        row.interactivity()
            .on_hover(motion::hover_listener(fade_key));
        row
    }
}

/// [`menu_row`] with a distinct keyboard-navigation highlight: a selected row
/// carries the full `bg-white/10` wash, the keyboard cursor the lighter
/// `bg-white/[0.08]` (zeron's `data-[highlighted]` styling) — two selected-
/// looking rows never appear at once.
pub fn menu_row_nav(
    theme: &Theme,
    selected: bool,
    highlighted: bool,
    fade_key: impl Into<SharedString>,
) -> gpui::Div {
    let row = menu_row(theme, selected, fade_key);
    if !selected && highlighted {
        row.bg(crate::theme::card_selected_bg())
            .text_color(theme.text)
    } else {
        row
    }
}

/// Small uppercase section heading inside a floating menu (zeron
/// `MenuHeading`): `px-2 pb-1 pt-1.5 text-[10px] font-medium uppercase
/// tracking-[0.1em] text-muted-foreground/60`. gpui has no letter-spacing at
/// the pinned rev; the tracking is approximated with hair spaces.
pub fn menu_heading(theme: &Theme, label: &str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .px(px(8.0))
        .pb(px(4.0))
        .pt(px(6.0))
        .text_size(crate::typography::ui_rems(10.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(SharedString::from(tracked_upper(label)))
}

/// Uppercase + hair-space tracking (see [`menu_heading`]).
pub fn tracked_upper(label: &str) -> String {
    let upper = label.to_uppercase();
    let mut out = String::with_capacity(upper.len() * 2);
    let mut first = true;
    for ch in upper.chars() {
        if !first {
            out.push('\u{200A}'); // hair space ≈ 0.1em tracking
        }
        out.push(ch);
        first = false;
    }
    out
}

/// Hairline divider between menu sections (zeron `MenuSeparator`:
/// `mx-1 my-1 h-px bg-white/[0.07]`).
pub fn menu_separator() -> gpui::Div {
    // Full-bleed: negative margins cancel the card's inset so the hairline
    // runs border to border (user request).
    div()
        .h(px(1.0))
        .mx(px(-CARD_INSET))
        .my(px(MENU_GAP))
        .bg(hairline(0.07))
}

/// The recessed band tone for a palette/picker header or footer strip — a
/// translucent black so the glass still reads through (the add-space palette
/// converged on this; measured subtler tones vanish against the dim scrim).
/// Free function (like [`ink`]/[`hairline`]/[`wash`]), mirroring
/// [`Theme::band`], for the several callers with no `Theme`/`cx` in scope
/// (some outside this crate's `ui` module tree — threading a `&Theme` param
/// would ripple past this task's file scope).
pub fn band() -> gpui::Hsla {
    crate::theme::band()
}

/// Shared shell for command-palette-style flows. The recessed header/footer
/// bands are supplied by callers, while this owns the glass tint, outline,
/// radius, clipping, and shadow that make Cmd+K and its sibling flows read as
/// one component family.
pub fn palette_card(theme: &Theme, width: Pixels, corner_radius: f32) -> gpui::Div {
    div()
        .w(width)
        .rounded(px(corner_radius))
        .border_1()
        .border_color(hairline(0.10))
        .bg(surface_bg(theme))
        .shadow_lg()
        .overflow_hidden()
        .flex()
        .flex_col()
        .text_color(theme.text)
}

/// A compact search glyph with the same 16px slot as palette action icons.
pub fn palette_search_icon(theme: &Theme) -> gpui::Div {
    div()
        .size(px(16.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            crate::icons::icon(crate::icons::PALETTE_SEARCH)
                .size(px(16.0))
                .text_color(theme.text_muted),
        )
}

/// One footer key-cap (22px, rounded-5, `white/[0.05]`) holding arbitrary
/// children — the base of [`key_hint`]/[`key_hint_pair`] and the search-bar
/// chips ("⌘K", "esc").
pub fn key_cap(_theme: &Theme) -> gpui::Div {
    div()
        .h(px(22.0))
        .px(px(5.0))
        .rounded(px(5.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .bg(ink(0.05))
}

/// The tiny verb after a key-cap.
fn key_hint_label(theme: &Theme, label: &'static str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .text_size(crate::typography::ui_rems(10.5))
        .text_color(theme.text_muted)
        .child(SharedString::from(label))
}

/// A footer legend: one icon key-cap + tiny verb (the add-space palette's
/// footer voice, shared by the pickers).
pub fn key_hint(theme: &Theme, icon_path: &'static str, label: &'static str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .child(
            key_cap(theme).child(
                crate::icons::icon(icon_path)
                    .size(px(12.5))
                    .text_color(theme.text_muted),
            ),
        )
        .child(key_hint_label(theme, label))
}

/// A footer legend whose cap holds a WORD ("tab", "esc") instead of a glyph
/// — for keys with no icon in the set.
pub fn key_hint_text(theme: &Theme, cap: &'static str, label: &'static str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .child(
            key_cap(theme)
                .text_size(px(11.0))
                .font_family(theme.font_mono.clone())
                .text_color(theme.text_muted)
                .child(SharedString::from(cap)),
        )
        .child(key_hint_label(theme, label))
}

/// A footer legend whose cap holds TWO glyphs split by a hairline
/// ("[ ↑ | ↓ ] Navigate") sharing one verb.
pub fn key_hint_pair(
    theme: &Theme,
    first: &'static str,
    second: &'static str,
    label: &'static str,
) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .child(
            key_cap(theme)
                .child(
                    crate::icons::icon(first)
                        .size(px(12.5))
                        .text_color(theme.text_muted),
                )
                .child(div().w(px(1.0)).h(px(11.0)).bg(hairline(0.10)))
                .child(
                    crate::icons::icon(second)
                        .size(px(12.5))
                        .text_color(theme.text_muted),
                ),
        )
        .child(key_hint_label(theme, label))
}

/// A muted kbd hint chip inside menu rows (`⌘↵`-style accelerators).
pub fn kbd_hint(theme: &Theme, label: &str) -> gpui::Div {
    let theme = &theme.for_popup();
    div()
        .flex_none()
        .px(px(5.0))
        .py(px(1.0))
        .rounded(px(5.0))
        .bg(ink(0.05))
        .text_size(crate::typography::ui_rems(10.0))
        .font_family(theme.font_mono.clone())
        .text_color(theme.text_muted)
        .child(SharedString::from(label.to_string()))
}

/// The search/text input frame at the top of a picker popover (zeron
/// `searchInput`: `w-full rounded-lg bg-white/[0.04] px-2.5 py-1.5
/// text-[13px]` + `mb-1`, borderless — full width inside the card's own
/// p-1, only a 4px bottom margin).
pub fn search_input_frame(_theme: &Theme, input: AnyElement) -> gpui::Div {
    div()
        .mb(px(4.0))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(MENU_ITEM_RADIUS))
        .bg(ink(0.04))
        .text_size(crate::typography::ui_rems(13.0))
        .child(input)
}

/// A bordered trailing menu section (zeron picker action groups /
/// branch-picker worktree block: `mt-1 flex flex-col gap-0.5 border-t
/// border-white/[0.06] pt-1` — the hairline runs edge-to-edge of the card's
/// p-1 inset, unlike [`menu_separator`]'s mx-1).
pub fn menu_section() -> gpui::Div {
    div()
        .mt(px(4.0))
        .pt(px(4.0))
        .border_t_1()
        .border_color(hairline(0.06))
        .flex()
        .flex_col()
        .gap(px(2.0))
}

// ---------------------------------------------------------------------------
// Dialog primitives (zeron dialog.tsx / sidebar dialogs.tsx)
// ---------------------------------------------------------------------------

/// Centered dialog with the shared popover surface. A filled drop shadow
/// would show through the translucent card, so only opaque cards use it.
pub fn dialog_card(theme: &Theme) -> gpui::Div {
    div()
        .w(px(360.0))
        .p(px(20.0))
        .rounded(px(16.0))
        .bg(surface_bg(theme))
        .border_1()
        .border_color(hairline(0.10))
        .when(!theme.is_frost(), |el| el.shadow_lg())
        .flex()
        .flex_col()
        .text_color(theme.text)
}

/// Dialog title: `text-[15px] font-semibold tracking-tight`.
pub fn dialog_title(theme: &Theme, title: &str) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(15.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text)
        .child(SharedString::from(title.to_string()))
}

/// Dialog body copy: `text-[13px] leading-relaxed text-muted-foreground`.
pub fn dialog_body(theme: &Theme, copy: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(crate::typography::ui_rems(13.0))
        .line_height(px(19.0))
        .text_color(theme.text_muted)
        .child(copy.into())
}

/// Dialog text-field frame: `rounded-lg border border-white/[0.08]
/// bg-white/[0.04] px-3 py-2 text-[14px]`.
pub fn dialog_field(input: AnyElement) -> gpui::Div {
    div()
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(hairline(0.08))
        .bg(ink(0.04))
        .text_size(crate::typography::ui_rems(14.0))
        .child(input)
}

/// Ghost button (`btnGhost`): quiet text, hover wash fading over
/// `transition-colors` (zeron dialogs.tsx). Caller adds id + click; `fade_key`
/// as in [`menu_row`].
pub fn btn_ghost(theme: &Theme, label: &str, fade_key: impl Into<SharedString>) -> gpui::Div {
    let fade_key = fade_key.into();
    let mut btn = div()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .text_size(crate::typography::ui_rems(13.0))
        .text_color(motion::hover_blend(&fade_key, theme.text_muted, theme.text))
        .bg(motion::hover_blend(
            &fade_key,
            crate::theme::wash(0.0),
            ink(0.06),
        ))
        .cursor_pointer()
        .child(SharedString::from(label.to_string()));
    btn.interactivity()
        .on_hover(motion::hover_listener(fade_key));
    btn
}

/// Primary button (`btnPrimary`): white fill, near-black text.
pub fn btn_primary(theme: &Theme, label: &str) -> gpui::Div {
    div()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(theme.text)
        .text_size(crate::typography::ui_rems(13.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.on_solid)
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(SharedString::from(label.to_string()))
}

/// Destructive button (`btnDestructive`): the muted red fill.
pub fn btn_danger(theme: &Theme, label: &str) -> gpui::Div {
    div()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .bg(theme.danger_strong)
        .text_size(crate::typography::ui_rems(13.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(gpui::white())
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(SharedString::from(label.to_string()))
}

/// Pulsing skeleton rows shown while a list loads (zeron:
/// `h-7 animate-pulse rounded-md bg-white/[0.04]`).
pub fn skeleton_rows(
    _id: &'static str,
    _theme: &Theme,
    count: usize,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    let wash = ink(0.04);
    let delta = motion::pulse_delta(&ZERON_PULSE, view, cx);
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .py(px(4.0))
        .children((0..count).map(move |i| {
            let phase = motion::staggered_phase(delta, i, 0.08);
            div()
                .h(px(28.0))
                .rounded(px(Theme::CONTROL_RADIUS))
                .bg(wash)
                .opacity(0.35 + 0.4 * motion::pulse_wave(phase))
        }))
        .into_any_element()
}

/// One pulsing ghost label — the trigger chip's label slot while the
/// selected model still resolves (a chip collapsing to its bare icon read
/// as broken; user report).
pub fn skeleton_bar(width: f32, view: gpui::EntityId, cx: &mut gpui::App) -> AnyElement {
    let delta = motion::pulse_delta(&ZERON_PULSE, view, cx);
    div()
        .w(px(width))
        .h(px(11.0))
        .rounded(px(5.5))
        .bg(ink(0.08))
        .opacity(0.35 + 0.4 * motion::pulse_wave(motion::staggered_phase(delta, 0, 0.0)))
        .into_any_element()
}

/// [`skeleton_rows`] shaped like a MENU loading: shorter bars of varied
/// widths reading as ghost labels rather than full-width slabs (the model
/// picker's loading state — reference design's skeleton). Widths cycle a
/// small deterministic ladder so the stagger reads organic without
/// randomness (randomness would repaint differently every open).
pub fn skeleton_menu_rows(
    _id: &'static str,
    _theme: &Theme,
    count: usize,
    view: gpui::EntityId,
    cx: &mut gpui::App,
) -> AnyElement {
    const WIDTHS: [f32; 4] = [0.42, 0.58, 0.48, 0.66];
    let wash = ink(0.05);
    let delta = motion::pulse_delta(&ZERON_PULSE, view, cx);
    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .py(px(6.0))
        .px(px(4.0))
        .children((0..count).map(move |i| {
            let phase = motion::staggered_phase(delta, i, 0.08);
            div()
                .h(px(14.0))
                .w(gpui::relative(WIDTHS[i % WIDTHS.len()]))
                .rounded(px(7.0))
                .bg(wash)
                .opacity(0.35 + 0.4 * motion::pulse_wave(phase))
        }))
        .into_any_element()
}

/// Inline error row + Retry affordance (the caller attaches the listener to the
/// returned id).
pub fn error_row(theme: &Theme, message: &str) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .p(px(Theme::SPACE_SM))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.danger)
        .child(gpui::SharedString::from(message.to_string()))
}

// ---------------------------------------------------------------------------
// Floating menu scrollbar — the model-list treatment, shared
// ---------------------------------------------------------------------------

/// Track inset top/bottom; the thumb travels inside it.
pub const MENU_SCROLLBAR_TRACK_INSET: f32 = 4.0;
/// Invisible hit strip width on the right edge.
pub const MENU_SCROLLBAR_HIT_WIDTH: f32 = 10.0;
/// Resting thumb width.
pub const MENU_SCROLLBAR_THUMB_WIDTH: f32 = 3.0;
/// Thumb width while hovered/dragged.
pub const MENU_SCROLLBAR_HOVER_THUMB_WIDTH: f32 = 5.0;
/// Smallest readable thumb on very long lists.
pub const MENU_SCROLLBAR_MIN_THUMB: f32 = 24.0;
/// How long the rail stays fully visible after the last scroll motion.
pub const MENU_SCROLLBAR_LINGER_MS: u64 = 1400;
/// How long the rail takes to fade out after the linger window.
pub const MENU_SCROLLBAR_FADE_MS: u64 = 260;
/// Repaint cadence through the fade window — each wake repaints the next
/// intermediate [`MenuScrollbarState::fade`] value.
const MENU_SCROLLBAR_FADE_FRAME_MS: u64 = 16;

/// Geometry of the floating thumb for a scroll viewport at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MenuScrollbarMetrics {
    pub track_height: f32,
    pub thumb_top: f32,
    pub thumb_height: f32,
    pub max_scroll: f32,
}

impl MenuScrollbarMetrics {
    /// Distance the thumb itself can travel.
    pub fn travel(self) -> f32 {
        (self.track_height - self.thumb_height).max(0.0)
    }

    /// Pure geometry from the viewport and scroll distances. `None` when the
    /// content fits (`max_scroll <= 0`) or the viewport is too small to hold
    /// a track.
    pub fn from_viewport(
        viewport_height: f32,
        max_scroll: f32,
        current_scroll: f32,
    ) -> Option<Self> {
        let max_scroll = max_scroll.max(0.0);
        if viewport_height <= 0.0 || max_scroll <= 0.0 {
            return None;
        }
        let track_height = (viewport_height - MENU_SCROLLBAR_TRACK_INSET * 2.0).max(0.0);
        if track_height <= 0.0 {
            return None;
        }
        let content_height = viewport_height + max_scroll;
        let thumb_height = (track_height * viewport_height / content_height)
            .max(MENU_SCROLLBAR_MIN_THUMB)
            .min(track_height);
        let current_scroll = current_scroll.clamp(0.0, max_scroll);
        let travel = (track_height - thumb_height).max(0.0);
        Some(Self {
            track_height,
            thumb_top: travel * current_scroll / max_scroll,
            thumb_height,
            max_scroll,
        })
    }

    /// [`Self::from_viewport`] from raw viewport/content extents and the
    /// current offset — the shape of owners that know a total content height
    /// rather than a max-scroll distance (the terminal's emulator geometry:
    /// total vs. visible rows).
    pub fn from_parts(viewport_height: f32, content_height: f32, offset_y: f32) -> Option<Self> {
        Self::from_viewport(viewport_height, content_height - viewport_height, offset_y)
    }
}

/// Marker for GPUI's captured drag stream. The actual grab geometry stays in
/// [`MenuScrollbarState`] so a track click can center the thumb first.
pub struct MenuScrollbarDrag;

/// Invisible drag preview: scrollbar drags manipulate the existing thumb.
pub struct MenuScrollbarDragGhost;

impl gpui::Render for MenuScrollbarDragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::Empty
    }
}

/// Hover/drag interaction state for one floating scrollbar, owned by the view
/// that renders the list. Event handlers stay on the view (they need its
/// listeners) and delegate here; only one list surface owns a state at a
/// time, so mutually exclusive popups may share one instance.
///
/// Visibility: the rail paints while a drag or a track-hover holds it open,
/// or while scroll motion is recent ([`Self::note_scroll`]) — scrolling
/// shows it with or without hover; hovering the list alone shows nothing.
/// When the motion stops the rail lingers [`MENU_SCROLLBAR_LINGER_MS`], fades
/// over [`MENU_SCROLLBAR_FADE_MS`], and disappears — but hopping onto the
/// track mid-linger freezes it open, and leaving the track restarts the wait
/// (it never hides under the pointer). [`schedule_scrollbar_hide`] keeps
/// wake-ups scheduled so the fade lands without any further input.
#[derive(Default)]
pub struct MenuScrollbarState {
    list_hovered: bool,
    bar_hovered: bool,
    grab: Option<f32>,
    /// Last seen scroll position — a change marks fresh scroll activity.
    last_scroll_y: Option<f32>,
    /// When that change was seen.
    last_scroll_at: Option<Instant>,
    /// When the in-flight hide wake-up fires; one wake is in flight at a
    /// time, and its render re-arms while the countdown runs.
    hide_wake_at: Option<Instant>,
}

impl MenuScrollbarState {
    /// Record scroll activity from the live handle. Call once per render
    /// before [`Self::metrics`]/[`Self::render_rail`]; a changed offset marks
    /// the rail as recently scrolled.
    pub fn note_scroll(&mut self, scroll: &gpui::ScrollHandle) {
        self.note_scroll_offset(-f32::from(scroll.offset().y));
    }

    /// Same as [`Self::note_scroll`] for owners whose scroll position is not
    /// a gpui handle (the terminal's display offset, for one) — any value
    /// that changes exactly when the visible scroll position does.
    pub fn note_scroll_offset(&mut self, position: f32) {
        self.note_scroll_offset_at(position, Instant::now());
    }

    fn note_scroll_offset_at(&mut self, position: f32, at: Instant) {
        match self.last_scroll_y {
            // First observation: establish the baseline only — a fresh mount
            // always reports its initial offset, and that is not scrolling.
            None => self.last_scroll_y = Some(position),
            Some(previous) if previous != position => {
                self.last_scroll_y = Some(position);
                self.last_scroll_at = Some(at);
            }
            Some(_) => {}
        }
    }

    /// Forget the baseline entirely: the next [`Self::note_scroll`] observes
    /// whatever offset the layout settled on (e.g. after `scroll_to_item`)
    /// as the fresh baseline, again without marking motion.
    pub fn clear_scroll_baseline(&mut self) {
        self.last_scroll_y = None;
        self.last_scroll_at = None;
    }

    fn scroll_countdown(&self, now: Instant) -> bool {
        self.last_scroll_at.is_some_and(|at| {
            now.duration_since(at)
                < Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS)
        })
    }

    /// Metrics from any scroll handle's live bounds/offset — both
    /// `ScrollHandle` and a virtualized list's base handle qualify. `None`
    /// when the content fits.
    pub fn metrics(&self, scroll: &gpui::ScrollHandle) -> Option<MenuScrollbarMetrics> {
        let bounds = scroll.bounds();
        // GPUI stores the maximum as a positive distance; only the live
        // scroll offset is negative while content moves upward.
        let max_scroll = f32::from(scroll.max_offset().y).max(0.0);
        let current_scroll = (-f32::from(scroll.offset().y)).clamp(0.0, max_scroll);
        MenuScrollbarMetrics::from_viewport(
            f32::from(bounds.size.height),
            max_scroll,
            current_scroll,
        )
    }
    /// Whether the rail paints at all: a drag or a track-hover holds it
    /// open, otherwise **recent scroll motion** does — hovering the list
    /// alone shows nothing, scrolling shows it even with the pointer
    /// elsewhere (touchpad momentum), and hover+scroll is the common case.
    pub fn visible(&self) -> bool {
        self.grab.is_some() || self.bar_hovered || self.scroll_countdown(Instant::now())
    }

    /// 1 → 0 across the fade window once the scroll motion stops; full while
    /// a drag or a track-hover holds the rail open.
    pub fn fade(&self) -> f32 {
        if self.grab.is_some() || self.bar_hovered {
            return 1.0;
        }
        let Some(at) = self.last_scroll_at else {
            return 0.0;
        };
        let elapsed = at.elapsed().as_millis() as f32;
        let linger = MENU_SCROLLBAR_LINGER_MS as f32;
        let fade = MENU_SCROLLBAR_FADE_MS as f32;
        (1.0 - (elapsed - linger) / fade).clamp(0.0, 1.0)
    }

    fn animating_at(&self, now: Instant) -> bool {
        self.grab.is_none() && !self.bar_hovered && self.scroll_countdown(now)
    }

    /// When the countdown next needs a repaint: the rest of the linger (the
    /// wake lands as the fade starts), then frame steps through the fade so
    /// [`Self::fade`]'s intermediate values actually get painted. `None`
    /// when nothing is winding down or the window has fully elapsed.
    fn next_wake_at(&self, now: Instant) -> Option<Instant> {
        if !self.animating_at(now) {
            return None;
        }
        let linger = Duration::from_millis(MENU_SCROLLBAR_LINGER_MS);
        let total = Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS);
        let motion = self.last_scroll_at?;
        let elapsed = now.duration_since(motion);
        if elapsed < linger {
            Some(motion + linger)
        } else if elapsed < total {
            Some(now + Duration::from_millis(MENU_SCROLLBAR_FADE_FRAME_MS))
        } else {
            None
        }
    }

    /// Arm the next hide wake-up and return how long the caller should wait.
    /// `None` when there is nothing to schedule. One wake is in flight at a
    /// time; its render re-arms for whatever the countdown needs then, so
    /// resumed scrolling or a refreshed linger converges on the next wake.
    pub fn arm_hide_timer(&mut self) -> Option<Duration> {
        self.arm_hide_timer_at(Instant::now())
    }

    fn arm_hide_timer_at(&mut self, now: Instant) -> Option<Duration> {
        let Some(wake) = self.next_wake_at(now) else {
            self.hide_wake_at = None;
            return None;
        };
        if self.hide_wake_at.is_some_and(|pending| pending > now) {
            return None;
        }
        self.hide_wake_at = Some(wake);
        Some(wake - now)
    }

    /// Whether the thumb carries the expanded/stronger treatment.
    pub fn active(&self) -> bool {
        self.bar_hovered || self.grab.is_some()
    }

    /// The pointer entered/left the LIST. Returns whether anything changed.
    pub fn set_list_hovered(&mut self, hovered: bool) -> bool {
        if self.list_hovered == hovered {
            return false;
        }
        self.list_hovered = hovered;
        if !hovered && self.grab.is_none() && self.bar_hovered {
            // Leaving the host straight off the track: restart the linger so
            // the rail doesn't vanish under a departing pointer (the strip's
            // own leave event usually does this first).
            self.bar_hovered = false;
            self.last_scroll_at = Some(Instant::now());
        }
        true
    }

    /// The pointer entered/left the RAIL. Keeps the active treatment while a
    /// captured drag travels outside (the hover callback correctly turns
    /// false there). Leaving the track after the rail lingered restarts the
    /// wait — it hides only a beat later, not mid-hover. Returns whether
    /// anything changed.
    pub fn set_bar_hovered(&mut self, hovered: bool) -> bool {
        let active = hovered || self.grab.is_some();
        if self.bar_hovered == active {
            return false;
        }
        self.bar_hovered = active;
        if !active {
            self.last_scroll_at = Some(Instant::now());
        }
        true
    }

    /// A press landed on the rail: choose the grab point (pressing the thumb
    /// keeps its relative position; pressing the track centers the thumb
    /// under the pointer first), engage the drag, and scroll to the pointer.
    /// `false` when there is nothing to scroll.
    pub fn begin_press(&mut self, scroll: &ScrollHandle, pointer_y: Pixels) -> bool {
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        self.begin_press_in(&metrics, scroll.bounds().top(), pointer_y);
        self.drag_to(scroll, pointer_y);
        true
    }

    /// [`Self::begin_press`] in the metrics domain, for owners with no
    /// `ScrollHandle` (virtualized lists): engage the grab from raw geometry
    /// — `track_top` is the window y of the track's bounds (the scroller's
    /// top). Apply the initial target with [`Self::drag_target_in`].
    pub fn begin_press_in(
        &mut self,
        metrics: &MenuScrollbarMetrics,
        track_top: Pixels,
        pointer_y: Pixels,
    ) {
        let pointer_in_track = Self::pointer_in_track_at(track_top, pointer_y);
        let grab_offset = if (metrics.thumb_top..=metrics.thumb_top + metrics.thumb_height)
            .contains(&pointer_in_track)
        {
            pointer_in_track - metrics.thumb_top
        } else {
            metrics.thumb_height / 2.0
        };
        self.grab = Some(grab_offset);
    }

    /// Move an engaged drag to `pointer_y`. `false` when no drag is engaged
    /// or the content stopped scrolling mid-drag.
    pub fn drag_to(&self, scroll: &ScrollHandle, pointer_y: Pixels) -> bool {
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        let Some(fraction) = self.drag_target_in(&metrics, scroll.bounds().top(), pointer_y) else {
            return false;
        };
        let offset = scroll.offset();
        scroll.set_offset(Point::new(offset.x, px(-fraction * metrics.max_scroll)));
        true
    }

    /// [`Self::drag_to`] in the metrics domain: the target scroll position as
    /// a fraction of `metrics.max_scroll` (apply it in the owner's own scroll
    /// units). `None` when no drag is engaged; a full-track thumb (zero
    /// travel) always targets 0.
    pub fn drag_target_in(
        &self,
        metrics: &MenuScrollbarMetrics,
        track_top: Pixels,
        pointer_y: Pixels,
    ) -> Option<f32> {
        let grab_offset = self.grab?;
        let pointer_in_track = Self::pointer_in_track_at(track_top, pointer_y);
        let thumb_top = (pointer_in_track - grab_offset).clamp(0.0, metrics.travel());
        Some(if metrics.travel() <= 0.0 {
            0.0
        } else {
            thumb_top / metrics.travel()
        })
    }

    /// The press ended anywhere: drop the drag; the rail stays armed only
    /// while the list is still hovered. Returns whether anything changed.
    pub fn end_press(&mut self) -> bool {
        self.grab = None;
        // Releasing a drag lingers like a stopped scroll.
        self.last_scroll_at = Some(Instant::now());
        if !self.list_hovered && self.bar_hovered {
            self.bar_hovered = false;
            true
        } else {
            false
        }
    }

    /// Window-y → track-y; `track_top` is the window y of the track's bounds.
    fn pointer_in_track_at(track_top: Pixels, pointer_y: Pixels) -> f32 {
        f32::from(pointer_y - track_top) - MENU_SCROLLBAR_TRACK_INSET
    }

    /// The positioned rail visuals: a full-height hit strip on the right with
    /// the thumb inside. `None` while hidden or the content fits. Prefer
    /// [`rail`], which layers identity + the six standard pointer listeners
    /// onto this strip; reach for the raw element only when a surface needs
    /// different listeners. Either way `.id(...)` comes first (hover needs
    /// element state).
    pub fn render_rail(&self, theme: &Theme, metrics: MenuScrollbarMetrics) -> Option<Div> {
        if !self.visible() {
            return None;
        }
        let active = self.active();
        let thumb_width = if active {
            MENU_SCROLLBAR_HOVER_THUMB_WIDTH
        } else {
            MENU_SCROLLBAR_THUMB_WIDTH
        };
        Some(
            div()
                .absolute()
                .top(px(0.0))
                .bottom(px(0.0))
                .right(px(0.0))
                .w(px(MENU_SCROLLBAR_HIT_WIDTH))
                // Post-scroll fade: full while hovered/held, then 1 → 0
                // across the fade window (see [`MenuScrollbarState::fade`]).
                .opacity(self.fade())
                // The thumb is an absolute child inside a fixed-width hit
                // rail, so hover expansion never reflows rows. Hovering the
                // thumb itself brightens it on top of the strip-hover
                // widening — pure paint-level hover styling, no notify.
                .child(
                    div()
                        .id("scrollbar-thumb")
                        .absolute()
                        .top(px(MENU_SCROLLBAR_TRACK_INSET + metrics.thumb_top))
                        .right(px(2.0))
                        .w(px(thumb_width))
                        .h(px(metrics.thumb_height))
                        .rounded(px(thumb_width / 2.0))
                        .bg(theme.text_faint.opacity(if active { 0.68 } else { 0.5 }))
                        .hover(|s| s.bg(theme.text_faint.opacity(0.85))),
                ),
        )
    }
}

/// Keep wake-ups scheduled while a scroll-triggered linger/fade countdown
/// runs: one wake at the end of the linger (the fade's first repaint), then
/// frame-cadence wakes through the fade, so the rail fades out and hides
/// [`MENU_SCROLLBAR_LINGER_MS`] + [`MENU_SCROLLBAR_FADE_MS`] after the last
/// scroll motion instead of sticking until the next unrelated render. Call
/// once per render after the rail's activity note
/// ([`MenuScrollbarState::note_scroll`] / [`MenuScrollbarState::note_scroll_offset`]);
/// at most one wake is in flight at a time.
pub fn schedule_scrollbar_hide<T: 'static>(bar: &mut MenuScrollbarState, cx: &mut Context<T>) {
    let Some(delay) = bar.arm_hide_timer() else {
        return;
    };
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(delay).await;
        this.update(cx, |_, cx| cx.notify()).ok();
    })
    .detach();
}

/// The view-side halves of one floating rail. Implement on the view that owns
/// the list, and [`rail`] folds the whole treatment — activity note, hide
/// scheduling, metrics, visuals, and all six pointer listeners — into one
/// call.
pub trait ScrollRailHost: 'static {
    /// The rail's hover/drag interaction state.
    fn rail_bar(&mut self) -> &mut MenuScrollbarState;

    /// The scroll handle feeding the rail. `None` for handle-less owners
    /// (virtualized lists, the terminal), which override [`Self::rail_metrics`]
    /// and [`Self::rail_press`]/[`Self::rail_drag_to`] to work from their own
    /// geometry.
    fn rail_scroll(&self) -> Option<ScrollHandle> {
        None
    }

    /// Record this frame's scroll activity and return the rail geometry.
    /// `None` while the content fits. The default notes + measures
    /// [`Self::rail_scroll`]; handle-less owners override to build
    /// [`MenuScrollbarMetrics`] from their own geometry and report their
    /// position proxy via [`MenuScrollbarState::note_scroll_offset`].
    fn rail_metrics(&mut self) -> Option<MenuScrollbarMetrics> {
        let scroll = self.rail_scroll()?;
        self.rail_bar().note_scroll(&scroll);
        self.rail_bar().metrics(&scroll)
    }

    /// A press landed on the rail at window-y `pointer_y`; `true` when it
    /// engaged a drag (the event is then consumed). The default presses
    /// [`Self::rail_scroll`]'s handle; handle-less owners engage via
    /// [`MenuScrollbarState::begin_press_in`] and apply the initial
    /// [`MenuScrollbarState::drag_target_in`] target themselves.
    fn rail_press(&mut self, pointer_y: Pixels) -> bool {
        self.rail_scroll()
            .is_some_and(|scroll| self.rail_bar().begin_press(&scroll, pointer_y))
    }

    /// An engaged drag moved to window-y `pointer_y`; `true` when it moved
    /// the scroll position. The default drags [`Self::rail_scroll`]'s handle;
    /// handle-less owners apply [`MenuScrollbarState::drag_target_in`]
    /// themselves.
    fn rail_drag_to(&mut self, pointer_y: Pixels) -> bool {
        self.rail_scroll()
            .is_some_and(|scroll| self.rail_bar().drag_to(&scroll, pointer_y))
    }
}

/// The whole floating-scrollbar treatment for any [`ScrollRailHost`] view:
/// note the frame's scroll activity, keep the hide countdown scheduled,
/// render the rail, and wire the six pointer listeners (strip hover, press,
/// drag capture, drag move, and both mouse-up ends) onto it. The drag-move
/// listener rides on the strip itself — gpui dispatches the captured drag
/// stream to it wherever the pointer travels. `None` while the rail is
/// hidden or the content fits; the host keeps only its own list-hover
/// listener. Handle-less owners get their overrides called at the same
/// points the default handle calls would be.
pub fn rail<V: ScrollRailHost>(
    host: &mut V,
    id: &'static str,
    theme: &Theme,
    cx: &mut Context<V>,
) -> Option<AnyElement> {
    let metrics = host.rail_metrics()?;
    schedule_scrollbar_hide(host.rail_bar(), cx);
    let strip = host.rail_bar().render_rail(theme, metrics)?;
    Some(
        strip
            .id(id)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.rail_bar().set_bar_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.rail_press(event.position.y) {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .on_drag(MenuScrollbarDrag, |_, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| MenuScrollbarDragGhost)
            })
            .on_drag_move(cx.listener(
                |this, event: &gpui::DragMoveEvent<MenuScrollbarDrag>, _, cx| {
                    if this.rail_drag_to(event.event.position.y) {
                        cx.notify();
                    }
                },
            ))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.rail_bar().end_press();
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.rail_bar().end_press();
                    cx.notify();
                }),
            )
            .into_any_element(),
    )
}

/// Rewind a handle-owned menu list to its top and drop its rail activity
/// baseline in one step — the pair every "reopen this menu fresh" site must
/// make. Skipping the offset half strands the list mid-scroll; skipping the
/// baseline half lets the next note read the jump back to the top as
/// scrolling and show the rail.
pub fn reset_menu_scroll(scroll: &ScrollHandle, bar: &mut MenuScrollbarState) {
    scroll.set_offset(Point::default());
    bar.clear_scroll_baseline();
}

// ---------------------------------------------------------------------------
// Horizontal floating scrollbar — the same quiet rail used by menus, rotated
// for local code planes. Kept separate from `MenuScrollbarState` so a code
// fence can own one state per stable block while existing menu callers retain
// their vertical API.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizontalScrollbarMetrics {
    pub track_width: f32,
    pub thumb_left: f32,
    pub thumb_width: f32,
    pub max_scroll: f32,
}

impl HorizontalScrollbarMetrics {
    pub fn travel(self) -> f32 {
        (self.track_width - self.thumb_width).max(0.0)
    }

    pub fn from_viewport(
        viewport_width: f32,
        max_scroll: f32,
        current_scroll: f32,
    ) -> Option<Self> {
        let max_scroll = max_scroll.max(0.0);
        if viewport_width <= 0.0 || max_scroll <= 0.0 {
            return None;
        }
        let track_width = (viewport_width - MENU_SCROLLBAR_TRACK_INSET * 2.0).max(0.0);
        if track_width <= 0.0 {
            return None;
        }
        let content_width = viewport_width + max_scroll;
        let thumb_width = (track_width * viewport_width / content_width)
            .max(MENU_SCROLLBAR_MIN_THUMB)
            .min(track_width);
        let current_scroll = current_scroll.clamp(0.0, max_scroll);
        let travel = (track_width - thumb_width).max(0.0);
        Some(Self {
            track_width,
            thumb_left: travel * current_scroll / max_scroll,
            thumb_width,
            max_scroll,
        })
    }
}

/// Hover/drag state for one horizontal code viewport. Geometry comes from the
/// same tracked [`gpui::ScrollHandle`] that moves the code, so the thumb always
/// represents the block's real local overflow (virtual transcript height is
/// irrelevant here).
#[derive(Default)]
pub struct HorizontalScrollbarState {
    viewport_hovered: bool,
    bar_hovered: bool,
    grab: Option<f32>,
}

impl HorizontalScrollbarState {
    pub fn metrics(&self, scroll: &gpui::ScrollHandle) -> Option<HorizontalScrollbarMetrics> {
        let bounds = scroll.bounds();
        let max_scroll = f32::from(scroll.max_offset().x).max(0.0);
        let current_scroll = (-f32::from(scroll.offset().x)).clamp(0.0, max_scroll);
        HorizontalScrollbarMetrics::from_viewport(
            f32::from(bounds.size.width),
            max_scroll,
            current_scroll,
        )
    }

    pub fn visible(&self) -> bool {
        self.viewport_hovered || self.grab.is_some()
    }

    pub fn active(&self) -> bool {
        self.bar_hovered || self.grab.is_some()
    }

    pub fn set_viewport_hovered(&mut self, hovered: bool) -> bool {
        if self.viewport_hovered == hovered {
            return false;
        }
        self.viewport_hovered = hovered;
        if !hovered && self.grab.is_none() {
            self.bar_hovered = false;
        }
        true
    }

    pub fn set_bar_hovered(&mut self, hovered: bool) -> bool {
        let active = hovered || self.grab.is_some();
        if self.bar_hovered == active {
            return false;
        }
        self.bar_hovered = active;
        true
    }

    pub fn begin_press(&mut self, scroll: &gpui::ScrollHandle, pointer_x: Pixels) -> bool {
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        let pointer_in_track = self.pointer_in_track(scroll, pointer_x);
        let grab_offset = if (metrics.thumb_left..=metrics.thumb_left + metrics.thumb_width)
            .contains(&pointer_in_track)
        {
            pointer_in_track - metrics.thumb_left
        } else {
            metrics.thumb_width / 2.0
        };
        self.grab = Some(grab_offset);
        self.drag_to(scroll, pointer_x);
        true
    }

    pub fn drag_to(&self, scroll: &gpui::ScrollHandle, pointer_x: Pixels) -> bool {
        let Some(grab_offset) = self.grab else {
            return false;
        };
        let Some(metrics) = self.metrics(scroll) else {
            return false;
        };
        let thumb_left =
            (self.pointer_in_track(scroll, pointer_x) - grab_offset).clamp(0.0, metrics.travel());
        let scroll_to = if metrics.travel() <= 0.0 {
            0.0
        } else {
            thumb_left / metrics.travel() * metrics.max_scroll
        };
        let offset = scroll.offset();
        scroll.set_offset(gpui::Point::new(px(-scroll_to), offset.y));
        true
    }

    pub fn end_press(&mut self) -> bool {
        self.grab = None;
        if !self.viewport_hovered && self.bar_hovered {
            self.bar_hovered = false;
            return true;
        }
        false
    }

    fn pointer_in_track(&self, scroll: &gpui::ScrollHandle, pointer_x: Pixels) -> f32 {
        f32::from(pointer_x - scroll.bounds().left()) - MENU_SCROLLBAR_TRACK_INSET
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_press_note_distinguishes_dismiss_from_open() {
        let mut popup: Popup<u8> = Popup::default();

        // Fresh open: press finds nothing mounted → click opens.
        popup.note_trigger_press();
        assert!(!popup.take_press_was_open());
        popup.open(1);

        // Trigger click while open: the card's mouse-down-out begins the
        // close on the press (either handler order) — the note still reads
        // mounted, so the click must NOT reopen.
        popup.note_trigger_press();
        popup.begin_close();
        assert!(popup.take_press_was_open());
        // Out-handler first, trigger note second: mid-exit still counts.
        popup.open(1);
        popup.begin_close();
        popup.note_trigger_press();
        assert!(popup.take_press_was_open());

        // The note is consumed — a later click starts clean.
        assert!(!popup.take_press_was_open());

        // Kind-keyed popups: a press on a DIFFERENT trigger doesn't count,
        // so that click switches menus instead of swallowing.
        let mut popup: Popup<u8> = Popup::default();
        popup.open(1);
        popup.note_trigger_press_matching(|kind| *kind == 2);
        assert!(!popup.take_press_was_open());
        popup.note_trigger_press_matching(|kind| *kind == 1);
        assert!(popup.take_press_was_open());
    }

    #[test]
    fn menu_step_wraps_and_enters() {
        // Entering an empty menu stays out.
        assert_eq!(menu_step(None, 0, 1), None);
        assert_eq!(menu_step(Some(3), 0, 1), None);
        // Entering from nothing lands on the matching edge.
        assert_eq!(menu_step(None, 3, 1), Some(0));
        assert_eq!(menu_step(None, 3, -1), Some(2));
        // Stepping wraps both ways.
        assert_eq!(menu_step(Some(2), 3, 1), Some(0));
        assert_eq!(menu_step(Some(0), 3, -1), Some(2));
        assert_eq!(menu_step(Some(1), 3, 1), Some(2));
    }

    #[test]
    fn filter_ranks_prefix_before_substring() {
        let labels = ["main", "feature/main-sync", "master", "dev"];
        // Prefix matches ("main", "master") come before the substring match.
        assert_eq!(filter_indices("ma", &labels), vec![0, 2, 1]);
        // Case-insensitive.
        assert_eq!(filter_indices("MA", &labels), vec![0, 2, 1]);
        // No matches → empty.
        assert!(filter_indices("zzz", &labels).is_empty());
        // Empty / whitespace query keeps input order.
        assert_eq!(filter_indices("", &labels), vec![0, 1, 2, 3]);
        assert_eq!(filter_indices("   ", &labels), vec![0, 1, 2, 3]);
    }

    #[test]
    fn match_rank_kinds() {
        assert_eq!(match_rank("re", "release"), Some(0));
        assert_eq!(match_rank("lease", "release"), Some(1));
        assert_eq!(match_rank("x", "release"), None);
        assert_eq!(match_rank("", "anything"), Some(1));
    }

    #[test]
    fn key_classification() {
        assert_eq!(classify_key("up", false, false), MenuKey::Up);
        assert_eq!(classify_key("down", false, false), MenuKey::Down);
        assert_eq!(classify_key("enter", false, false), MenuKey::Enter);
        assert_eq!(classify_key("enter", true, false), MenuKey::ModEnter);
        assert_eq!(classify_key("enter", false, true), MenuKey::ModEnter);
        assert_eq!(classify_key("escape", false, false), MenuKey::Escape);
        assert_eq!(classify_key("backspace", false, false), MenuKey::Backspace);
        assert_eq!(classify_key("a", false, false), MenuKey::Other);
        // Readline motion — only with ctrl held.
        assert_eq!(classify_key("n", false, true), MenuKey::Down);
        assert_eq!(classify_key("p", false, true), MenuKey::Up);
        assert_eq!(classify_key("n", false, false), MenuKey::Other);
        assert_eq!(classify_key("p", true, false), MenuKey::Other);
    }

    #[test]
    fn tracked_upper_spaces_letters() {
        assert_eq!(tracked_upper("ab"), "A\u{200A}B");
        assert_eq!(
            tracked_upper("Question"),
            "Q\u{200A}U\u{200A}E\u{200A}S\u{200A}T\u{200A}I\u{200A}O\u{200A}N"
        );
        assert_eq!(tracked_upper(""), "");
    }

    #[test]
    fn loadable_accessors() {
        let l: Loadable<u32> = Loadable::Ready(7);
        assert_eq!(l.ready(), Some(&7));
        assert!(!l.is_loading());
        let e: Loadable<u32> = Loadable::Error("boom".into());
        assert_eq!(e.error(), Some("boom"));
        assert!(Loadable::<u32>::Loading.is_loading());
        assert_eq!(Loadable::<u32>::default(), Loadable::Idle);
    }

    #[test]
    fn scrollbar_metrics_hidden_when_content_fits_or_viewport_tiny() {
        // No overflow → no scrollbar.
        assert_eq!(MenuScrollbarMetrics::from_viewport(300.0, 0.0, 0.0), None);
        assert_eq!(MenuScrollbarMetrics::from_viewport(300.0, -5.0, 0.0), None);
        // No viewport → no scrollbar.
        assert_eq!(MenuScrollbarMetrics::from_viewport(0.0, 300.0, 0.0), None);
        // Viewport smaller than two track insets → no track.
        assert_eq!(MenuScrollbarMetrics::from_viewport(8.0, 300.0, 0.0), None);
    }

    #[test]
    fn scrollbar_metrics_scales_thumb_to_content_ratio() {
        let m = MenuScrollbarMetrics::from_viewport(300.0, 300.0, 150.0).unwrap();
        // Track = 300 - 2*4; thumb = half the content (600) → 146.
        assert_eq!(m.track_height, 292.0);
        assert_eq!(m.thumb_height, 146.0);
        assert_eq!(m.travel(), 146.0);
        // Half-scrolled puts the thumb mid-track.
        assert_eq!(m.thumb_top, 73.0);
        assert_eq!(m.max_scroll, 300.0);
    }

    #[test]
    fn scrollbar_metrics_clamps_min_thumb_and_position() {
        let m = MenuScrollbarMetrics::from_viewport(100.0, 9900.0, 4950.0).unwrap();
        // Raw ratio (92 * 100 / 10000 ≈ 0.92px) clamps to the readable minimum.
        assert_eq!(m.thumb_height, MENU_SCROLLBAR_MIN_THUMB);
        assert_eq!(m.travel(), 92.0 - MENU_SCROLLBAR_MIN_THUMB);
        assert_eq!(m.thumb_top, (92.0 - MENU_SCROLLBAR_MIN_THUMB) / 2.0);
        // Overscroll clamps to the bottom of the track.
        let m = MenuScrollbarMetrics::from_viewport(100.0, 9900.0, 99_999.0).unwrap();
        assert_eq!(m.thumb_top, 92.0 - MENU_SCROLLBAR_MIN_THUMB);
        // Negative offsets clamp to the top.
        let m = MenuScrollbarMetrics::from_viewport(100.0, 9900.0, -3.0).unwrap();
        assert_eq!(m.thumb_top, 0.0);
    }

    #[test]
    fn horizontal_scrollbar_metrics_match_the_vertical_treatment() {
        let m = HorizontalScrollbarMetrics::from_viewport(300.0, 300.0, 150.0).unwrap();
        assert_eq!(m.track_width, 292.0);
        assert_eq!(m.thumb_width, 146.0);
        assert_eq!(m.travel(), 146.0);
        assert_eq!(m.thumb_left, 73.0);
        assert_eq!(m.max_scroll, 300.0);
    }

    #[test]
    fn horizontal_scrollbar_hides_without_overflow_and_clamps_position() {
        assert_eq!(
            HorizontalScrollbarMetrics::from_viewport(300.0, 0.0, 0.0),
            None
        );
        let m = HorizontalScrollbarMetrics::from_viewport(100.0, 9900.0, 99_999.0).unwrap();
        assert_eq!(m.thumb_left, 92.0 - MENU_SCROLLBAR_MIN_THUMB);
        let m = HorizontalScrollbarMetrics::from_viewport(100.0, 9900.0, -3.0).unwrap();
        assert_eq!(m.thumb_left, 0.0);
    }

    #[test]
    fn scrollbar_press_and_drag_work_in_the_metrics_domain() {
        let mut bar = MenuScrollbarState::default();
        // track 292, thumb 146, travel 146, thumb at the top.
        let metrics = MenuScrollbarMetrics::from_viewport(300.0, 300.0, 0.0).unwrap();
        let track_top = px(100.0);
        let pointer = |track_y: f32| px(100.0 + MENU_SCROLLBAR_TRACK_INSET + track_y);

        // No press engaged → no target.
        assert_eq!(bar.drag_target_in(&metrics, track_top, pointer(73.0)), None);

        // Pressing the track centers the thumb under the pointer: grab is
        // half the thumb, so the pointer 104px into the track targets the
        // far end.
        bar.begin_press_in(&metrics, track_top, pointer(250.0));
        assert_eq!(
            bar.drag_target_in(&metrics, track_top, pointer(250.0)),
            Some(1.0)
        );
        // Dragging past the top clamps to the very top.
        assert_eq!(
            bar.drag_target_in(&metrics, track_top, pointer(-100.0)),
            Some(0.0)
        );

        // Pressing the thumb keeps its relative position under the pointer:
        // grabbing the thumb's middle and dragging half its travel lands
        // mid-scroll.
        let mut bar = MenuScrollbarState::default();
        bar.begin_press_in(&metrics, track_top, pointer(73.0));
        assert_eq!(
            bar.drag_target_in(&metrics, track_top, pointer(146.0)),
            Some(0.5)
        );
    }

    #[test]
    fn hide_wakes_step_from_linger_end_through_the_fade() {
        let mut bar = MenuScrollbarState::default();
        let t0 = Instant::now();
        // Nothing winding down → nothing to schedule.
        assert_eq!(bar.arm_hide_timer_at(t0), None);

        bar.note_scroll_offset_at(0.0, t0);
        bar.note_scroll_offset_at(10.0, t0);
        // Mid-linger: the next wake is the rest of the linger, so the first
        // repaint lands as the fade starts.
        let mid = t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS / 2);
        assert_eq!(
            bar.arm_hide_timer_at(mid),
            Some(Duration::from_millis(MENU_SCROLLBAR_LINGER_MS / 2))
        );
        // The pending wake fired and rendered; from the fade's first frame
        // on, wakes run at frame cadence so intermediate fade values paint.
        bar.hide_wake_at = None;
        let fade_start = t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS);
        assert_eq!(
            bar.arm_hide_timer_at(fade_start),
            Some(Duration::from_millis(MENU_SCROLLBAR_FADE_FRAME_MS))
        );
        bar.hide_wake_at = None;
        let mid_fade =
            t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS / 2);
        assert_eq!(
            bar.arm_hide_timer_at(mid_fade),
            Some(Duration::from_millis(MENU_SCROLLBAR_FADE_FRAME_MS))
        );
        // Past the window the countdown is over — the rail is hidden.
        bar.hide_wake_at = None;
        let past =
            t0 + Duration::from_millis(MENU_SCROLLBAR_LINGER_MS + MENU_SCROLLBAR_FADE_MS + 1);
        assert_eq!(bar.arm_hide_timer_at(past), None);
    }

    #[test]
    fn hide_timer_keeps_one_wake_in_flight() {
        let mut bar = MenuScrollbarState::default();
        let t0 = Instant::now();
        bar.note_scroll_offset_at(0.0, t0);
        bar.note_scroll_offset_at(10.0, t0);
        assert_eq!(
            bar.arm_hide_timer_at(t0),
            Some(Duration::from_millis(MENU_SCROLLBAR_LINGER_MS))
        );
        // A pending wake covers re-renders while it is still in flight; its
        // own render re-arms for whatever the countdown needs then.
        assert_eq!(bar.arm_hide_timer_at(t0 + Duration::from_millis(10)), None);
        // A drag or track-hover holds the rail open — no countdown, no wake.
        bar.hide_wake_at = None;
        bar.begin_press_in(
            &MenuScrollbarMetrics {
                track_height: 300.0,
                thumb_top: 0.0,
                thumb_height: 30.0,
                max_scroll: 1000.0,
            },
            px(0.0),
            px(10.0),
        );
        assert_eq!(bar.arm_hide_timer_at(t0 + Duration::from_millis(20)), None);
    }

    #[test]
    fn scrollbar_metrics_from_parts_matches_viewport_shape() {
        // Content extent + offset instead of a max-scroll distance (the
        // terminal's emulator geometry).
        let from_parts = MenuScrollbarMetrics::from_parts(300.0, 600.0, 150.0);
        assert_eq!(
            from_parts,
            MenuScrollbarMetrics::from_viewport(300.0, 300.0, 150.0)
        );
        // Content that fits the viewport stays rail-less.
        assert_eq!(MenuScrollbarMetrics::from_parts(300.0, 200.0, 0.0), None);
    }

    #[test]
    fn reset_menu_scroll_rewinds_without_marking_motion() {
        let scroll = ScrollHandle::new();
        let mut bar = MenuScrollbarState::default();
        bar.note_scroll(&scroll);
        reset_menu_scroll(&scroll, &mut bar);
        assert_eq!(scroll.offset(), Point::default());
        // The re-observed top offset is a fresh baseline, not motion.
        bar.note_scroll(&scroll);
        assert!(!bar.visible());
    }
}

/// Compact mention-style match washes preserve the row's font and spacing.
pub(crate) fn search_highlight(
    text: SharedString,
    query: Option<&str>,
    theme: &Theme,
) -> AnyElement {
    let Some(query) = query.filter(|query| !query.trim().is_empty()) else {
        return text.into_any_element();
    };
    let ranges = search_match_ranges(&text, query);
    if ranges.is_empty() {
        return text.into_any_element();
    }
    let badges = ranges;
    let styled = gpui::StyledText::new(text).with_highlights(badges.iter().cloned().map(|range| {
        (
            range,
            gpui::HighlightStyle {
                color: Some(theme.code_text),
                ..Default::default()
            },
        )
    }));
    let layout = styled.layout().clone();
    let wash = theme.code_wash;
    // As in the composer, paint rounded backgrounds beneath shaped glyphs.
    // A TextRun background would be square and can disappear when clipped.
    let underlay = gpui::canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            for range in &badges {
                for mut bounds in crate::markdown::render::range_rects(&layout, range, 1.0, 1.5) {
                    // Glyphs sit below the line box's optical center. Shift the
                    // wash down half a logical pixel to balance visible top/bottom padding.
                    bounds.origin.y += px(0.5);
                    window.paint_quad(gpui::quad(
                        bounds,
                        px(3.0),
                        wash,
                        px(0.0),
                        gpui::transparent_black(),
                        gpui::BorderStyle::default(),
                    ));
                }
            }
        },
    )
    .absolute()
    .size_full();
    div()
        .relative()
        .child(underlay)
        .child(styled)
        .into_any_element()
}

fn search_match_ranges(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let folded = text.to_lowercase();
    let mut original = Vec::with_capacity(folded.len());
    for (start, ch) in text.char_indices() {
        let range = start..start + ch.len_utf8();
        for lower in ch.to_lowercase() {
            original.extend(std::iter::repeat_n(range.clone(), lower.len_utf8()));
        }
    }
    let mut ranges = Vec::new();
    for word in query.to_lowercase().split_whitespace() {
        for (start, _) in folded.match_indices(word) {
            ranges.push(original[start].start..original[start + word.len() - 1].end);
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

#[cfg(test)]
mod search_highlight_tests {
    use super::search_match_ranges;

    #[test]
    fn inline_matches_keep_adjacent_word_boundaries() {
        let text = "fieldnotes/fix-authentication-redirects";
        let ranges = search_match_ranges(text, "authentication");
        assert_eq!(&text[..ranges[0].start], "fieldnotes/fix-");
        assert_eq!(&text[ranges[0].clone()], "authentication");
        assert_eq!(&text[ranges[0].end..], "-redirects");
    }

    #[test]
    fn highlights_repeated_case_insensitive_and_overlapping_words() {
        assert_eq!(
            search_match_ranges("New chat, new project", "NEW"),
            vec![0..3, 10..13]
        );
        assert_eq!(
            search_match_ranges("authentication", "auth authentication"),
            vec![0..14]
        );
        assert!(search_match_ranges("New chat", "  ").is_empty());
        assert!(search_match_ranges("New chat", "settings").is_empty());
    }

    #[test]
    fn preserves_original_unicode_boundaries_after_lowercase_expansion() {
        assert_eq!(
            search_match_ranges("İstanbul café", "i CAFÉ"),
            vec![0..2, 10..15]
        );
        assert_eq!(search_match_ranges("🚀 CAFÉ", "café"), vec![5..10]);
    }
}

/// Available vertical space at a measured trigger, including the menu's gap
/// and window margin. Prefer above; flip only when it cannot fit useful chrome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuGeometry {
    pub height: f32,
    pub below: bool,
}

pub fn menu_geometry(top: f32, bottom: f32, viewport_height: f32) -> MenuGeometry {
    let above = (top - 14.0).max(0.0);
    let below = (viewport_height - bottom - 14.0).max(0.0);
    let flip = above < 180.0 && below > above;
    MenuGeometry {
        height: (if flip { below } else { above }).min(640.0),
        below: flip,
    }
}

#[cfg(test)]
mod adaptive_menu_tests {
    use super::*;
    #[test]
    fn budget_tracks_trigger_and_flips_when_needed() {
        assert_eq!(
            menu_geometry(300.0, 320.0, 500.0),
            MenuGeometry {
                height: 286.0,
                below: false
            }
        );
        assert_eq!(
            menu_geometry(80.0, 100.0, 500.0),
            MenuGeometry {
                height: 386.0,
                below: true
            }
        );
        assert_eq!(menu_geometry(900.0, 920.0, 1000.0).height, 640.0);
    }
}
