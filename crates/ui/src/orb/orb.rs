// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! The [`Orb`] component: a Rust builder over the animation engine.
//!
//! The engine takes continuous, unbounded time — its modes mix incommensurate
//! frequencies, so there is no seamless wrap point and gpui's folded
//! `with_animation` clock would jump at every loop. An entity owns the real
//! clock; [`orb_element`] is the same paint one layer down for hosts that
//! already tick.

use std::{cell::RefCell, rc::Rc, time::Duration};

use gpui::{
    Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled, Task, Window, canvas, div,
    px,
};
use std::time::Instant;
use zeron_orb::{
    OrbAnimator, OrbSize, OrbState, OrbTheme,
    engine::{Frame, draw_mode_into},
    resolve_preset,
};

use crate::orb::paint::paint_frame;

/// Frames per second the orb redraws at when no explicit rate is set.
///
/// The orb is a status indicator, not a game: its motion is slow and organic,
/// and at 30 fps it is indistinguishable from 60 while costing half as much.
/// Every redraw walks the whole element tree, so the tick rate — not the
/// geometry — is what dominates CPU.
pub const DEFAULT_TARGET_FPS: f32 = 30.0;

/// Animated thinking-orb status indicator for AI / agent UIs.
///
/// ```ignore
/// cx.new(|_| Orb::new().state(OrbState::Searching).size(OrbSize::Avatar))
/// ```
pub struct Orb {
    /// State, size, speed, clock, audio and crossfades — shared with mobile.
    animator: OrbAnimator,
    /// Uniform magnification of the size preset's painted geometry.
    scale: f32,
    theme: OrbTheme,
    paused: bool,
    /// When true, freeze on a static representative frame (`t = 0.6`). The
    /// system `reduce_motion` setting forces the same, so hosts only set this
    /// for their own per-surface motion preferences.
    reduced_motion: bool,
    /// Redraw rate ceiling.
    target_fps: f32,
    /// Stop animating while the host window is not the active one.
    pause_when_inactive: bool,
    /// Host-controlled visibility. When false the timer is cancelled and the
    /// orb freezes — use this when the entity is still mounted but scrolled
    /// off-screen (gpui has no intersection observer).
    visible: bool,

    /// Geometry buffer reused across frames. Behind `Rc<RefCell<_>>` because
    /// the canvas paint callback must be `'static` and so cannot borrow `self`.
    frame: Rc<RefCell<Frame>>,
    /// Explicit invalidation for animation ticks and semantic changes. Parent
    /// renders between ticks reuse the retained geometry.
    geometry_dirty: bool,

    /// Pending redraw timer. At most one stays in flight; dropping it cancels
    /// animation immediately when the orb becomes paused or invisible.
    tick: Option<Task<()>>,
    /// Window-activation subscription, registered lazily on first render since
    /// it needs a `Window`. Dropping it unsubscribes.
    activation: Option<gpui::Subscription>,
}

impl Default for Orb {
    fn default() -> Self {
        Self::new()
    }
}

impl Orb {
    pub fn new() -> Self {
        Self {
            animator: OrbAnimator::new(OrbState::Working, OrbSize::Avatar),
            scale: 1.0,
            theme: OrbTheme::Auto,
            paused: false,
            reduced_motion: false,
            target_fps: DEFAULT_TARGET_FPS,
            pause_when_inactive: true,
            visible: true,
            frame: Rc::new(RefCell::new(Frame::new())),
            geometry_dirty: true,
            tick: None,
            activation: None,
        }
    }

    pub fn state(mut self, state: OrbState) -> Self {
        // Nothing is on screen yet, so there is no frame to fade from.
        self.animator.set_state(state, &Frame::new());
        self
    }

    pub fn size(mut self, size: OrbSize) -> Self {
        self.animator.set_size(size);
        self
    }

    /// Paint the size preset magnified, e.g. a hero orb filling a stage.
    pub fn scale(mut self, scale: f32) -> Self {
        self.scale = sanitize_scale(scale);
        self
    }

    pub fn theme(mut self, theme: OrbTheme) -> Self {
        self.theme = theme;
        self
    }

    pub fn speed(mut self, speed: f32) -> Self {
        self.animator.set_speed(speed);
        self
    }

    /// Freeze the accumulated animation time while paused.
    pub fn paused(mut self, paused: bool) -> Self {
        self.paused = paused;
        self
    }

    /// Crossfade state geometry without remounting or restarting the clock.
    pub fn state_transition(mut self, duration: Duration) -> Self {
        self.animator.set_transition(duration);
        self
    }

    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = reduced;
        self
    }

    /// Cap the redraw rate. Values are clamped to `1.0..=240.0`.
    ///
    /// Lower is cheaper: cost scales linearly with this number.
    pub fn target_fps(mut self, fps: f32) -> Self {
        self.target_fps = sanitize_fps(fps);
        self
    }

    /// Whether to freeze while the host window is inactive. Defaults to `true`.
    ///
    /// A background window's animation is not visible to anyone, so this is
    /// usually free. Set it to `false` if the orb must keep moving in a window
    /// that is visible but unfocused — a side panel, or a floating HUD.
    pub fn pause_when_inactive(mut self, pause: bool) -> Self {
        self.pause_when_inactive = pause;
        self
    }

    /// Whether the host considers this orb on-screen. Defaults to `true`.
    ///
    /// gpui has no intersection observer. When you keep the entity mounted in
    /// a scrollable list but it has scrolled away, call
    /// [`Self::set_visible`]`(false)` (or build with `.visible(false)`) so the
    /// timer stops. Prefer unmounting when you can.
    pub fn visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// Mutable setters for interactive playgrounds.
    pub fn set_state(&mut self, state: OrbState, cx: &mut Context<Self>) {
        // A hidden, paused or reduced orb has a stopped clock, so it switches
        // without a fade.
        if self.animator.set_state(state, &self.frame.borrow()) {
            self.geometry_dirty = true;
            cx.notify();
        }
    }

    pub fn set_size(&mut self, size: OrbSize, cx: &mut Context<Self>) {
        if self.animator.set_size(size) {
            self.geometry_dirty = true;
            cx.notify();
        }
    }

    pub fn set_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        let scale = sanitize_scale(scale);
        if (self.scale - scale).abs() > f32::EPSILON {
            self.scale = scale;
            cx.notify();
        }
    }

    pub fn set_theme(&mut self, theme: OrbTheme, cx: &mut Context<Self>) {
        if self.theme != theme {
            self.theme = theme;
            cx.notify();
        }
    }

    pub fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        if self.animator.set_speed(speed) {
            self.geometry_dirty = true;
            cx.notify();
        }
    }

    /// Audio affects only visual motion, with independent microphone and
    /// speaker envelopes evaluated at the orb's frame rate.
    pub fn set_audio_levels(&mut self, microphone: f32, speaker: f32, cx: &mut Context<Self>) {
        if self
            .animator
            .set_audio_levels(microphone, speaker, Instant::now())
        {
            self.geometry_dirty = true;
            cx.notify();
        }
    }

    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.animator.stop(Instant::now());
        }
        self.geometry_dirty = true;
        cx.notify();
    }

    pub fn set_reduced_motion(&mut self, reduced: bool, cx: &mut Context<Self>) {
        if self.reduced_motion != reduced {
            self.reduced_motion = reduced;
            if reduced {
                self.animator.stop(Instant::now());
                self.animator.cancel_transition();
            }
            self.geometry_dirty = true;
            cx.notify();
        }
    }

    pub fn set_target_fps(&mut self, fps: f32, cx: &mut Context<Self>) {
        self.target_fps = sanitize_fps(fps);
        cx.notify();
    }

    pub fn set_pause_when_inactive(&mut self, pause: bool, cx: &mut Context<Self>) {
        if self.pause_when_inactive != pause {
            self.pause_when_inactive = pause;
            cx.notify();
        }
    }

    /// Host visibility gate — see [`Self::visible`].
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible != visible {
            self.visible = visible;
            if !visible {
                self.animator.stop(Instant::now());
                self.animator.cancel_transition();
            }
            self.geometry_dirty = true;
            cx.notify();
        }
    }

    pub fn state_value(&self) -> OrbState {
        self.animator.state()
    }

    pub fn size_value(&self) -> OrbSize {
        self.animator.size()
    }

    pub fn theme_value(&self) -> OrbTheme {
        self.theme
    }

    pub fn speed_value(&self) -> f32 {
        self.animator.speed()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn reduced_motion_value(&self) -> bool {
        self.reduced_motion
    }

    pub fn target_fps_value(&self) -> f32 {
        self.target_fps
    }

    pub fn pause_when_inactive_value(&self) -> bool {
        self.pause_when_inactive
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Ink direction, resolved against the installed bezel appearance —
    /// `Auto` follows it, no window subscription needed (the appearance is
    /// process-wide and its switch repaints the window anyway).
    fn dark(&self, cx: &gpui::App) -> bool {
        match self.theme {
            OrbTheme::Dark => true,
            OrbTheme::Light => false,
            OrbTheme::Auto => crate::theme::Theme::of(cx).appearance.is_dark(),
        }
    }

    /// Queue the next redraw, honouring [`Self::target_fps`].
    fn schedule_tick(&mut self, cx: &mut Context<Self>) {
        // Parent renders may happen between animation frames. Keep the timer
        // already in flight rather than cancel/reallocate it and push the next
        // frame farther into the future.
        if self.tick.is_some() {
            return;
        }
        let period = Duration::from_secs_f32(1.0 / self.target_fps);
        self.tick = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(period).await;
            let _ = this.update(cx, |orb, cx| {
                orb.tick = None;
                orb.geometry_dirty = true;
                cx.notify();
            });
        }));
    }
}

impl Render for Orb {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // An orb in a background window stops ticking entirely, so it needs a
        // nudge to start again when the window comes back. Registered once,
        // here, because it needs a `Window`.
        if self.activation.is_none() {
            self.activation = Some(cx.observe_window_activation(window, |orb, window, cx| {
                if orb.pause_when_inactive && !window.is_window_active() {
                    orb.animator.stop(Instant::now());
                }
                orb.geometry_dirty = true;
                cx.notify();
            }));
        }

        let size_px = self.animator.pixels();
        let scale = self.scale;
        let dark = self.dark(cx);
        let reduced = self.reduced_motion || cx.reduce_motion();
        let animating = self.visible
            && !self.paused
            && !reduced
            && (!self.pause_when_inactive || window.is_window_active());
        self.animator.tick(Instant::now(), animating, reduced);
        let r_min = self.animator.r_min();

        // Only ticks and semantic changes invalidate geometry. A parent can
        // re-render much faster than this orb's target FPS; those extra renders
        // reuse the retained frame rather than running animation math again.
        if self.geometry_dirty {
            self.animator.draw(&mut self.frame.borrow_mut());
            self.geometry_dirty = false;
        }

        if animating {
            self.schedule_tick(cx);
        } else {
            // Drop any in-flight tick so a paused, hidden, or backgrounded orb
            // costs nothing at all.
            self.tick = None;
        }

        let frame = self.frame.clone();
        div()
            .size(px(size_px * scale))
            .flex_shrink_0()
            .overflow_hidden()
            .child(
                canvas(
                    move |_bounds: Bounds<Pixels>, _window, _cx| (),
                    move |bounds, (), window, _cx| {
                        paint_frame(window, bounds, &frame.borrow(), dark, r_min, scale);
                    },
                )
                .size_full(),
            )
    }
}

/// Paint one frame of an orb at animation time `t` (seconds, unbounded) — the
/// pure, host-ticked form of [`Orb`]. Build it inside any render that runs on
/// a clock of its own; the reduced-motion convention is `t = 0.6`.
///
/// `frame` is the caller's geometry buffer, overwritten here and handed to the
/// paint closure. Geometry is a function of `t`, so there is nothing to cache
/// between frames — but a host that keeps one buffer per orb reuses its two
/// `Vec`s forever instead of growing a pair from empty on every tick.
pub fn orb_element(
    state: OrbState,
    size: OrbSize,
    t: f32,
    frame: &Rc<RefCell<Frame>>,
) -> impl IntoElement {
    let resolved = resolve_preset(state, size);
    let size_px = size.pixels();
    let frame = frame.clone();
    draw_mode_into(
        resolved.mode,
        size_px,
        t,
        &resolved.opts,
        &mut frame.borrow_mut(),
    );
    let r_min = resolved.opts.r_min.unwrap_or(0.3);
    div()
        .size(px(size_px))
        .flex_shrink_0()
        .overflow_hidden()
        .child(
            canvas(
                move |_bounds: Bounds<Pixels>, _window, _cx| (),
                move |bounds, (), window, cx| {
                    let dark = crate::theme::Theme::of(cx).appearance.is_dark();
                    paint_frame(window, bounds, &frame.borrow(), dark, r_min, 1.0);
                },
            )
            .size_full(),
        )
}

fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(0.25, 8.0)
    } else {
        1.0
    }
}

fn sanitize_fps(fps: f32) -> f32 {
    if fps.is_finite() {
        fps.clamp(1.0, 30.0)
    } else {
        DEFAULT_TARGET_FPS
    }
}
