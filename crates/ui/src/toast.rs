//! Sonner-style toasts: transient feedback stacked in the window's
//! bottom-right corner.
//!
//! The behaviour follows sonner's defaults:
//! - Toasts last four seconds. The countdown pauses while the pointer is over
//!   the stack and while the window is inactive.
//! - Collapsed, the newest toast sits in front and up to two older ones peek
//!   above it, each a step narrower (sonner's 5% scale).
//! - Hovering the stack fans the toasts out into a list.
//! - New toasts rise in from below; dismissed ones fade away.
//! - A toast is dismissed by swiping it off to the right.
//!
//! Push from anywhere with [`error`], [`success`] or [`info`]; the shell
//! mounts [`render_toaster`] once per window.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, point, px,
};

use crate::motion::CubicBezier;
use crate::theme::Theme;

const DURATION: Duration = Duration::from_millis(4000);
const VISIBLE: usize = 3;
/// Sonner's `--gap` and `--width`, and its desktop viewport offset.
const GAP: f32 = 14.0;
const WIDTH: f32 = 356.0;
const OFFSET: f32 = 24.0;
/// Each toast behind the front one is narrower by this fraction of the width
/// (sonner scales by 5%; gpui has no element scale, so the sides inset).
const STEP_SCALE: f32 = 0.05;
const MOVE: Duration = Duration::from_millis(400);
const EXIT: Duration = Duration::from_millis(200);
const SWIPE_THRESHOLD: f32 = 45.0;
/// Sonner's `cubic-bezier(.215, .61, .355, 1)`.
const EASE: CubicBezier = CubicBezier::new(0.215, 0.61, 0.355, 1.0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Error,
    Success,
    Info,
}

/// A value easing from one target to the next, restarting from wherever it
/// is whenever the target moves.
#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    started: Instant,
}

impl Glide {
    fn new(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            started: now,
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        let t = now.saturating_duration_since(self.started).as_secs_f32() / MOVE.as_secs_f32();
        EASE.eval(t.clamp(0.0, 1.0))
    }

    fn value(&self, now: Instant) -> f32 {
        crate::motion::lerp(self.from, self.to, self.progress(now))
    }

    fn retarget(&mut self, target: f32, now: Instant, snap: bool) {
        if snap {
            *self = Self::new(target, now);
        } else if (self.to - target).abs() > 0.01 {
            self.from = self.value(now);
            self.to = target;
            self.started = now;
        }
    }

    fn moving(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }
}

struct Toast {
    id: u64,
    kind: ToastKind,
    message: SharedString,
    created: Instant,
    /// Countdown left, and when it last resumed (`None` while paused).
    remaining: Duration,
    resumed: Option<Instant>,
    closing: Option<Instant>,
    /// Natural height, measured each paint.
    height: f32,
    bottom: Option<Glide>,
    clip: Option<Glide>,
    swipe: f32,
    swipe_from: Option<f32>,
}

impl Toast {
    fn expired(&self, now: Instant) -> bool {
        self.resumed
            .is_some_and(|since| now.saturating_duration_since(since) >= self.remaining)
    }

    fn pause(&mut self, now: Instant) {
        if let Some(since) = self.resumed.take() {
            self.remaining = self
                .remaining
                .saturating_sub(now.saturating_duration_since(since));
        }
    }

    fn resume(&mut self, now: Instant) {
        if self.resumed.is_none() {
            self.resumed = Some(now);
        }
    }

    fn deadline(&self) -> Option<Instant> {
        match (self.closing, self.resumed) {
            (Some(at), _) => Some(at + EXIT),
            (None, Some(since)) => Some(since + self.remaining),
            (None, None) => None,
        }
    }
}

#[derive(Default)]
struct ToastStore {
    toasts: Vec<Toast>,
    next_id: u64,
    hovered: bool,
    expand: Option<Glide>,
    /// The single wake-up for the next expiry, replaced as deadlines move.
    wake: Option<(Instant, Task<()>)>,
}

impl Global for ToastStore {}

fn push(cx: &mut App, kind: ToastKind, message: SharedString) {
    let message: SharedString = message.trim().to_owned().into();
    if message.is_empty() {
        return;
    }
    let now = Instant::now();
    let store = cx.default_global::<ToastStore>();
    // A repeat of the live front toast restarts it instead of stacking.
    if let Some(front) = store
        .toasts
        .iter_mut()
        .rev()
        .find(|toast| toast.closing.is_none())
        && front.kind == kind
        && front.message == message
    {
        front.remaining = DURATION;
        front.resumed = (!store.hovered).then_some(now);
    } else {
        store.next_id += 1;
        let id = store.next_id;
        let hovered = store.hovered;
        store.toasts.push(Toast {
            id,
            kind,
            message,
            created: now,
            remaining: DURATION,
            resumed: (!hovered).then_some(now),
            closing: None,
            height: 0.0,
            bottom: None,
            clip: None,
            swipe: 0.0,
            swipe_from: None,
        });
    }
    cx.refresh_windows();
}

/// A toast of `kind`.
pub fn show(cx: &mut App, kind: ToastKind, message: impl Into<SharedString>) {
    push(cx, kind, message.into());
}

/// A failure toast.
pub fn error(cx: &mut App, message: impl Into<SharedString>) {
    push(cx, ToastKind::Error, message.into());
}

/// A confirmation toast ("Link copied").
pub fn success(cx: &mut App, message: impl Into<SharedString>) {
    push(cx, ToastKind::Success, message.into());
}

/// A neutral notice.
pub fn info(cx: &mut App, message: impl Into<SharedString>) {
    push(cx, ToastKind::Info, message.into());
}

/// Surface a view's pending error as a toast, once: takes it out of `slot`.
/// For views that record a failure in an `Option` field and used to draw it
/// inline; call from render with the field.
pub fn take_error<M: Into<SharedString>>(slot: &mut Option<M>, cx: &mut App) {
    if let Some(message) = slot.take() {
        let message: SharedString = message.into();
        // Toasting mutates a global and refreshes windows: never mid-render.
        cx.defer(move |cx| error(cx, message));
    }
}

/// The live toasts' messages, oldest first.
#[cfg(test)]
pub fn messages(cx: &App) -> Vec<SharedString> {
    cx.try_global::<ToastStore>()
        .map(|store| {
            store
                .toasts
                .iter()
                .filter(|toast| toast.closing.is_none())
                .map(|toast| toast.message.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn dismiss(id: u64, cx: &mut App) {
    let now = Instant::now();
    if let Some(toast) = cx
        .default_global::<ToastStore>()
        .toasts
        .iter_mut()
        .find(|toast| toast.id == id && toast.closing.is_none())
    {
        toast.closing = Some(now);
    }
    cx.refresh_windows();
}

fn set_hovered(hovered: bool, cx: &mut App) {
    let now = Instant::now();
    let store = cx.default_global::<ToastStore>();
    if store.hovered == hovered {
        return;
    }
    store.hovered = hovered;
    for toast in &mut store.toasts {
        if hovered {
            toast.pause(now);
        } else {
            toast.resume(now);
        }
    }
    cx.refresh_windows();
}

/// Advance the stack: expire, remove finished exits, and pause while the
/// window is inactive. Returns whether anything is still animating.
fn tick(store: &mut ToastStore, active: bool, now: Instant) -> bool {
    let paused = store.hovered || !active;
    for toast in &mut store.toasts {
        if paused {
            toast.pause(now);
        } else {
            toast.resume(now);
        }
        if toast.closing.is_none() && toast.expired(now) {
            toast.closing = Some(now);
        }
    }
    store.toasts.retain(|toast| {
        toast
            .closing
            .is_none_or(|at| now.saturating_duration_since(at) < EXIT)
    });
    if store.toasts.is_empty() {
        store.hovered = false;
        store.expand = None;
    }
    store.toasts.iter().any(|toast| {
        toast.closing.is_some()
            || now.saturating_duration_since(toast.created) < MOVE
            || toast.bottom.is_some_and(|glide| glide.moving(now))
            || toast.clip.is_some_and(|glide| glide.moving(now))
    }) || store.expand.is_some_and(|glide| glide.moving(now))
}

/// Wake the window at the next expiry; animation frames cover the rest.
fn schedule_wake(window: &Window, cx: &mut App) {
    let store = cx.global::<ToastStore>();
    let next = store.toasts.iter().filter_map(Toast::deadline).min();
    let scheduled = store.wake.as_ref().map(|(due, _)| *due);
    let wake = match next {
        None => None,
        Some(at) if scheduled == Some(at) => return,
        Some(at) => {
            let handle = window.window_handle();
            let delay = at.saturating_duration_since(Instant::now());
            let task = cx.spawn(async move |cx| {
                cx.background_executor().timer(delay).await;
                let _ = handle.update(cx, |_, window, _| window.refresh());
            });
            Some((at, task))
        }
    };
    cx.global_mut::<ToastStore>().wake = wake;
}

fn kind_icon(kind: ToastKind, theme: &Theme) -> AnyElement {
    let (path, color) = match kind {
        ToastKind::Error => (crate::icons::DANGER_TRIANGLE, theme.danger),
        ToastKind::Success => (crate::icons::CHECK, theme.success),
        ToastKind::Info => (crate::icons::INFO_CIRCLE, theme.text_muted),
    };
    crate::icons::icon(path)
        .size(px(16.0))
        .flex_none()
        .text_color(color)
        .into_any_element()
}

/// The toaster for this window, or `None` when nothing is showing.
pub fn render_toaster(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
    let now = Instant::now();
    let reduced = cx.reduce_motion();
    let active = window.is_window_active();
    let theme = Theme::of(cx).for_popup();
    let animating = {
        let store = cx.try_global::<ToastStore>()?;
        if store.toasts.is_empty() {
            return None;
        }
        let store = cx.global_mut::<ToastStore>();
        tick(store, active, now)
    };
    let store = cx.global_mut::<ToastStore>();
    if store.toasts.is_empty() {
        store.wake = None;
        return None;
    }

    let expanded = store.hovered;
    let expand = {
        let glide = store
            .expand
            .get_or_insert_with(|| Glide::new(if expanded { 1.0 } else { 0.0 }, now));
        glide.retarget(if expanded { 1.0 } else { 0.0 }, now, reduced);
        glide.value(now)
    };

    // Newest first: index 0 is the front toast.
    let order: Vec<usize> = (0..store.toasts.len()).rev().collect();
    let live: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&ix| store.toasts[ix].closing.is_none())
        .collect();
    let front_height = live
        .first()
        .map(|&ix| store.toasts[ix].height)
        .filter(|height| *height > 0.0)
        .unwrap_or(52.0);

    // Targets: where each toast rests collapsed and fanned out.
    let mut targets: HashMap<u64, (f32, f32, usize)> = HashMap::new();
    let mut stacked = 0.0;
    for (index, &ix) in live.iter().enumerate() {
        let toast = &store.toasts[ix];
        let height = if toast.height > 0.0 {
            toast.height
        } else {
            front_height
        };
        let collapsed = index as f32 * GAP;
        let fanned = stacked;
        stacked += height + GAP;
        let bottom = crate::motion::lerp(collapsed, fanned, expand);
        let clip = crate::motion::lerp(front_height, height, expand);
        targets.insert(toast.id, (bottom, clip, index));
    }
    let stack_height = live
        .iter()
        .map(|&ix| {
            let (bottom, clip, _) = targets[&store.toasts[ix].id];
            bottom + clip
        })
        .fold(0.0_f32, f32::max)
        .max(front_height);

    let mut moving = animating;
    let mut children: Vec<AnyElement> = Vec::new();
    // Paint oldest first so the front toast lands on top.
    for &ix in order.iter().rev() {
        let toast = &mut store.toasts[ix];
        let id = toast.id;
        let (target_bottom, target_clip, index) = targets.get(&id).copied().unwrap_or((
            toast.bottom.map_or(0.0, |glide| glide.to),
            toast.clip.map_or(front_height, |glide| glide.to),
            usize::MAX,
        ));
        let first = toast.bottom.is_none();
        let bottom = toast
            .bottom
            .get_or_insert_with(|| Glide::new(target_bottom, now));
        bottom.retarget(target_bottom, now, reduced || first);
        let clip = toast
            .clip
            .get_or_insert_with(|| Glide::new(target_clip, now));
        clip.retarget(target_clip, now, reduced || first);
        let mut y = bottom.value(now);
        let clip_height = clip.value(now);
        moving |= bottom.moving(now) || clip.moving(now);

        // Enter: rise from below the stack and fade in.
        let enter = if reduced {
            1.0
        } else {
            EASE.eval(
                (now.saturating_duration_since(toast.created).as_secs_f32()
                    / MOVE.as_secs_f32())
                .clamp(0.0, 1.0),
            )
        };
        y -= (1.0 - enter) * (clip_height + OFFSET);
        let mut opacity = enter;
        // Exit: fade out, the front toast sinking as it goes.
        if let Some(at) = toast.closing {
            let t = (now.saturating_duration_since(at).as_secs_f32() / EXIT.as_secs_f32())
                .clamp(0.0, 1.0);
            opacity *= 1.0 - t;
            if toast.swipe.abs() < 0.5 && index == usize::MAX {
                y -= t * GAP;
            }
        }
        // Past the visible count, toasts wait unseen behind the stack.
        let depth = if index == usize::MAX { 0 } else { index };
        let hidden = depth >= VISIBLE;
        if hidden {
            opacity *= expand;
        }
        let inset = WIDTH * STEP_SCALE * 0.5 * depth.min(VISIBLE) as f32 * (1.0 - expand);
        let swipe = toast.swipe;
        if swipe > 0.0 {
            opacity *= (1.0 - swipe / (WIDTH * 0.6)).clamp(0.0, 1.0);
        }
        if opacity <= 0.001 && toast.closing.is_none() && !hidden {
            // Still rising in on its first frame: nothing to hit or see yet,
            // but keep measuring below.
        } else if opacity <= 0.001 {
            continue;
        }
        let kind = toast.kind;
        let message = toast.message.clone();
        let content = div()
            .id(("toast-content", id as usize))
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .px(px(14.0))
            .py(px(13.0))
            .text_size(crate::typography::ui_rems(13.0))
            .text_color(theme.text)
            .child(kind_icon(kind, &theme))
            .child(div().flex_1().min_w_0().child(message.clone()))
            .child(
                // Measure the natural height for the stack's layout.
                gpui::canvas(
                    move |bounds, _, cx| {
                        let height = f32::from(bounds.size.height);
                        if let Some(store) = cx.try_global::<ToastStore>()
                            && store
                                .toasts
                                .iter()
                                .find(|toast| toast.id == id)
                                .is_some_and(|toast| (toast.height - height).abs() > 0.5)
                        {
                            if let Some(toast) = cx
                                .global_mut::<ToastStore>()
                                .toasts
                                .iter_mut()
                                .find(|toast| toast.id == id)
                            {
                                toast.height = height;
                            }
                            cx.refresh_windows();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            );
        let card = div()
            .size_full()
            .rounded(px(crate::popover::CARD_RADIUS))
            .border_1()
            .border_color(theme.border)
            .bg(crate::popover::surface_bg(&theme))
            .overflow_hidden()
            .child(content);
        let toast_el = div()
            .id(("toast", id as usize))
            .role(match kind {
                ToastKind::Error => gpui::Role::Alert,
                _ => gpui::Role::Status,
            })
            .aria_label(message)
            .absolute()
            .left(px(inset + swipe.max(0.0)))
            .right(px(inset - swipe.max(0.0)))
            .bottom(px(y))
            .h(px(clip_height.max(0.0)))
            .opacity(opacity)
            // Swipe right to dismiss, as sonner does for the bottom-right
            // position; short swipes spring back.
            .on_mouse_down(gpui::MouseButton::Left, move |event, _, cx| {
                if let Some(toast) = cx
                    .global_mut::<ToastStore>()
                    .toasts
                    .iter_mut()
                    .find(|toast| toast.id == id)
                {
                    toast.swipe_from = Some(f32::from(event.position.x));
                }
            })
            .on_mouse_move(move |event, window, cx| {
                let Some(toast) = cx
                    .global_mut::<ToastStore>()
                    .toasts
                    .iter_mut()
                    .find(|toast| toast.id == id)
                else {
                    return;
                };
                let Some(from) = toast.swipe_from else { return };
                if !event.dragging() {
                    toast.swipe_from = None;
                    toast.swipe = 0.0;
                } else {
                    toast.swipe = (f32::from(event.position.x) - from).max(0.0);
                }
                window.refresh();
            })
            .on_mouse_up(gpui::MouseButton::Left, move |_, window, cx| {
                release_swipe(id, cx);
                window.refresh();
            })
            .on_mouse_up_out(gpui::MouseButton::Left, move |_, window, cx| {
                release_swipe(id, cx);
                window.refresh();
            })
            .child(crate::frost::frosted(
                crate::popover::CARD_RADIUS,
                crate::frost::MENU_BLUR,
                card,
            ));
        children.push(toast_el.into_any_element());
    }

    if moving {
        window.request_animation_frame();
    }
    schedule_wake(window, cx);

    let viewport = window.viewport_size();
    let width = WIDTH.min(f32::from(viewport.width) - 2.0 * OFFSET).max(0.0);
    let stack = div()
        .id("toaster")
        .debug_selector(|| "toaster".into())
        .relative()
        .w(px(width))
        .h(px(stack_height))
        // The stack, not each toast, blocks what's beneath: an occluding
        // toast would stop the hit test before the stack and its hover —
        // the hover that holds every countdown.
        .occlude()
        .on_hover(|hovered, _, cx| set_hovered(*hovered, cx))
        .children(children);
    Some(
        gpui::deferred(
            gpui::anchored()
                .position(point(
                    viewport.width - px(OFFSET) - px(width),
                    viewport.height - px(OFFSET) - px(stack_height),
                ))
                .child(stack),
        )
        .with_priority(4)
        .into_any_element(),
    )
}

fn release_swipe(id: u64, cx: &mut App) {
    let Some(toast) = cx
        .global_mut::<ToastStore>()
        .toasts
        .iter_mut()
        .find(|toast| toast.id == id)
    else {
        return;
    };
    let swiped = toast.swipe;
    toast.swipe_from = None;
    if swiped >= SWIPE_THRESHOLD {
        dismiss(id, cx);
    } else if let Some(toast) = cx
        .global_mut::<ToastStore>()
        .toasts
        .iter_mut()
        .find(|toast| toast.id == id)
    {
        toast.swipe = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(store: &mut ToastStore, message: &str, now: Instant) {
        store.next_id += 1;
        store.toasts.push(Toast {
            id: store.next_id,
            kind: ToastKind::Error,
            message: message.to_owned().into(),
            created: now,
            remaining: DURATION,
            resumed: Some(now),
            closing: None,
            height: 52.0,
            bottom: None,
            clip: None,
            swipe: 0.0,
            swipe_from: None,
        });
    }

    #[test]
    fn toasts_expire_pause_while_hovered_or_inactive_and_leave_after_exit() {
        let start = Instant::now();
        let mut store = ToastStore::default();
        toast(&mut store, "one", start);
        // Hovering banks the elapsed second and stops the clock.
        tick(&mut store, true, start + Duration::from_secs(1));
        store.hovered = true;
        tick(&mut store, true, start + Duration::from_secs(1));
        tick(&mut store, true, start + Duration::from_secs(30));
        assert!(store.toasts[0].closing.is_none(), "paused while hovered");
        store.hovered = false;
        // An inactive window holds the clock too.
        tick(&mut store, false, start + Duration::from_secs(31));
        tick(&mut store, false, start + Duration::from_secs(60));
        assert!(store.toasts[0].closing.is_none(), "paused while inactive");
        // Three seconds remain once resumed.
        tick(&mut store, true, start + Duration::from_secs(61));
        tick(&mut store, true, start + Duration::from_millis(63_900));
        assert!(store.toasts[0].closing.is_none());
        tick(&mut store, true, start + Duration::from_millis(64_100));
        assert!(store.toasts[0].closing.is_some(), "expires on time");
        tick(&mut store, true, start + Duration::from_millis(64_400));
        assert!(store.toasts.is_empty(), "removed after its exit");
    }

    struct Host;

    impl gpui::Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
            div().size_full().children(render_toaster(window, cx))
        }
    }

    #[gpui::test]
    fn toaster_shows_dedupes_and_closes(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let (_, cx) = cx.add_window_view(|_, _| Host);
        cx.update(|_, cx| {
            error(cx, "Couldn't save");
            error(cx, "Couldn't save");
            success(cx, "Link copied");
        });
        cx.run_until_parked();
        // A repeat of the front toast restarts it rather than stacking.
        cx.update(|_, cx| {
            assert_eq!(messages(cx), ["Couldn't save", "Link copied"]);
        });
        let id = cx.update(|_, cx| cx.global::<ToastStore>().toasts[1].id);
        cx.update(|_, cx| dismiss(id, cx));
        cx.update(|_, cx| assert_eq!(messages(cx), ["Couldn't save"]));
    }

    #[test]
    fn glide_restarts_from_its_current_value() {
        let now = Instant::now();
        let mut glide = Glide::new(0.0, now);
        glide.retarget(100.0, now, false);
        let mid = now + MOVE / 2;
        let halfway = glide.value(mid);
        assert!(halfway > 0.0 && halfway < 100.0);
        glide.retarget(0.0, mid, false);
        assert!((glide.value(mid) - halfway).abs() < 0.01, "no jump on reversal");
        assert_eq!(glide.value(mid + MOVE), 0.0);
    }
}
