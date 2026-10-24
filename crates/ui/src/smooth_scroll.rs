//! [`smooth_scroll`] — mouse-wheel notches glide instead of jumping.
//!
//! GPUI applies a wheel notch as one offset jump: every platform reports a
//! non-precise wheel as `ScrollDelta::Lines`, which containers convert to
//! pixels and apply at once, while trackpads already stream small `Pixels`
//! deltas. With the preference on, the window root takes each notch at the
//! end of the CAPTURE phase, before the bubble phase where containers scroll,
//! and replays it on the following frames as fractional `Lines` slices.
//!
//! The glide is Chrome's wheel animation (`cc::ScrollOffsetAnimationCurve`
//! for `kMouseWheel`, which Chrome runs on macOS while AppKit's
//! `NSScrollAnimationEnabled` is on, as it is by default): ease-in-out,
//! cubic-bezier(0.42, 0, 0.58, 1), over 200ms, shortening to 100ms for long
//! moves. A notch in flight moves the target on and starts a new leg at the
//! current speed, so a spin is one continuous glide. One departure: Chrome
//! nets a notch against the glide into a target clamped to the scroller's
//! bounds, which the root can't see, so here such a notch drops what was left
//! on its axis instead; netted, it could be spent on lines past an edge.
//!
//! Slices stay in lines so every container keeps its own line height (lists
//! 20px, divs the text line height, the terminal its rows): a notch still
//! travels exactly as far as before, only spread over time. Slices land on a
//! 1/4096-line grid, so an integral notch sums to exactly its lines and a
//! whole-row accumulator (the terminal's wheel reports to a program) never
//! strands a row. Like a trackpad stream, each slice goes to the current
//! pointer position with the current modifiers, so the window's input state
//! never goes stale.
//!
//! Left untouched, and each ends a glide in flight: precise input; notches
//! with Ctrl, Alt, Cmd or Fn held (zoom and other modified gestures stay
//! discrete, and pressing one mid-glide drops the rest rather than replaying
//! it as a zoom); notches over a [`raw_wheel_region`], for content that eases
//! them itself. A press ends a glide too. Off under reduced motion.
//!
//! A macOS web page's wheel never reaches the root: AppKit hands it to the
//! page, and `browser::macos` glides it on the same [`Glide`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Element, ElementId, GlobalElementId, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, Modifiers, MouseDownEvent, Pixels, PlatformInput,
    Point, ScrollDelta, ScrollWheelEvent, Styled as _, TouchPhase, Window, canvas, point,
};

/// Chrome's wheel easing control points, `(x1, y1, x2, y2)`
/// (`kEaseInOutControlPoints`).
const EASE_IN_OUT: [f64; 4] = [0.42, 0.0, 0.58, 1.0];
/// The pixels Chrome scales a macOS wheel's line delta by
/// (`kScrollbarPixelsPerCocoaTick`); GPUI hands that delta on as `Lines`.
/// Chrome sets its durations in these pixels, so a notch here takes as long
/// as it would there.
const PX_PER_LINE: f64 = 40.0;
/// Chrome's `kEpsilon`, 0.01px, for distances and speeds.
const EPSILON_LINES: f64 = 0.01 / PX_PER_LINE;
/// Slices are multiples of this. Sums of grid values stay exact in f32, so an
/// integral notch delivers exactly its lines.
const GRID_LINES: f32 = 1.0 / 4096.0;

thread_local! {
    /// Set in a wheel event's capture phase by the [`raw_wheel_region`] under
    /// the pointer; read by the root's listener later in the same phase.
    static KEEP_RAW: Cell<bool> = const { Cell::new(false) };
    /// [`enabled`] as of the last root paint, for AppKit callbacks, which
    /// can't read the App.
    static GLIDING: Cell<bool> = const { Cell::new(false) };
}

/// Whether notches glide, for wheels that never reach a window root: a macOS
/// web page gets its wheel from AppKit (`browser::macos`).
pub(crate) fn gliding() -> bool {
    GLIDING.get()
}

/// Wrap a window's root so wheel notches anywhere in it glide.
pub fn smooth_scroll(child: impl IntoElement) -> SmoothScroll {
    SmoothScroll {
        child: child.into_any_element(),
    }
}

/// Keep wheel notches over this region raw, for content that eases them
/// itself (a native web view). Absolutely positioned: make it a child of the
/// region it covers. Occluding overlays above it still glide.
pub fn raw_wheel_region() -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
                if phase == DispatchPhase::Capture
                    && !event.delta.precise()
                    && hitbox.should_handle_scroll(window)
                {
                    KEEP_RAW.set(true);
                }
            });
        },
    )
    .absolute()
    .inset_0()
}

/// Whether notches glide: the preference, unless motion is reduced.
fn enabled(cx: &App) -> bool {
    crate::settings::smooth_scrolling(cx) && !crate::motion::reduced_motion(cx)
}

/// A modifier that makes a wheel gesture something other than a scroll (zoom,
/// for one). Shift only picks the axis, and the platform already applied it.
fn commanded(modifiers: Modifiers) -> bool {
    modifiers.control || modifiers.alt || modifiers.platform || modifiers.function
}

/// Frame time. Test frames arrive microseconds apart, so tests set the clock.
fn now() -> Instant {
    #[cfg(test)]
    if let Some(now) = tests::CLOCK.get() {
        return now;
    }
    Instant::now()
}

/// One glide of wheel notches into slices of lines. A window root feeds it
/// GPUI's notches ([`intercept`], [`frame`]); a macOS web page, AppKit's.
#[derive(Default)]
pub(crate) struct Glide {
    /// Lines not yet delivered, per axis, in gpui's sign (positive scrolls
    /// toward the top or left).
    pending: Point<f32>,
    /// The leg in flight; `None` while idle.
    segment: Option<Segment>,
}

/// One window's glide, and where its frame loop is.
#[derive(Default)]
struct Root {
    glide: Glide,
    /// A frame callback is queued.
    scheduled: bool,
    /// A slice is being dispatched: the root listener lets it through.
    replaying: bool,
}

impl Glide {
    pub(crate) fn active(&self) -> bool {
        self.segment.is_some()
    }

    /// Queue a notch: the target moves on by it, and a glide in flight keeps
    /// its speed into the new leg (Chrome's `UpdateTarget`).
    pub(crate) fn push(&mut self, lines: Point<f32>, now: Instant) {
        let Some(segment) = self.segment.take() else {
            self.pending = lines;
            self.segment = Some(Segment::from_rest(lines, now));
            return;
        };
        let left = segment.remaining(now);
        let against = |heading: f32, lines: f32| heading * lines < 0.0;
        if !against(segment.delta.x, lines.x) && !against(segment.delta.y, lines.y) {
            self.pending = point(self.pending.x + lines.x, self.pending.y + lines.y);
            let delta = point(left.x + lines.x, left.y + lines.y);
            self.segment = Some(segment.retarget(delta, now, duration(delta)));
            return;
        }
        // Against the glide: drop what was left on that axis and turn from
        // rest, as Chrome does at an edge its target was clamped to.
        let axis = |pending: &mut f32, heading: f32, left: f32, lines: f32| {
            if against(heading, lines) {
                *pending = lines;
                lines
            } else {
                *pending += lines;
                left + lines
            }
        };
        let delta = point(
            axis(&mut self.pending.x, segment.delta.x, left.x, lines.x),
            axis(&mut self.pending.y, segment.delta.y, left.y, lines.y),
        );
        self.segment = Some(Segment::from_rest(delta, now));
    }

    /// The slice due at `now`: the leg's position less what was delivered,
    /// on the grid, and all that is left once the leg is over.
    pub(crate) fn advance(&mut self, now: Instant) -> Point<f32> {
        let Some(segment) = &self.segment else {
            return Point::default();
        };
        if segment.time(now) >= 1.0 {
            return self.finish();
        }
        let left = segment.remaining(now);
        let snap = |lines: f32| (lines / GRID_LINES).round() * GRID_LINES;
        let slice = point(snap(self.pending.x - left.x), snap(self.pending.y - left.y));
        self.pending = point(self.pending.x - slice.x, self.pending.y - slice.y);
        slice
    }

    /// Everything still pending, at once.
    pub(crate) fn finish(&mut self) -> Point<f32> {
        let rest = self.pending;
        self.stop();
        rest
    }

    pub(crate) fn stop(&mut self) {
        self.pending = Point::default();
        self.segment = None;
    }
}

/// How long Chrome takes to move `delta` lines from rest (`kInverseDelta`):
/// 12 frames at 60Hz (200ms) up to 120px, ramping down to 6 (100ms) at 480px
/// so a fast spin keeps up.
fn duration(delta: Point<f32>) -> f64 {
    let px = f64::from(major(delta)).abs() * PX_PER_LINE;
    let frames = 12.0 - (px - 120.0) * (12.0 - 6.0) / (480.0 - 120.0);
    frames.clamp(6.0, 12.0) / 60.0
}

/// The larger axis, with its sign (Chrome's `MaximumDimension`).
fn major(delta: Point<f32>) -> f32 {
    if delta.x.abs() > delta.y.abs() {
        delta.x
    } else {
        delta.y
    }
}

/// One leg of a glide: `delta` lines from `start`, over `duration` seconds.
struct Segment {
    delta: Point<f32>,
    start: Instant,
    duration: f64,
    /// The easing's control points; a retarget raises `y1` to carry speed.
    points: [f64; 4],
    ease: Bezier,
}

impl Segment {
    /// `slope`, when given, replaces `y1` with `x1 * slope`, so the leg starts
    /// at `slope` times its average speed (`EaseInOutWithInitialSlope`).
    fn new(
        delta: Point<f32>,
        start: Instant,
        duration: f64,
        points: [f64; 4],
        slope: Option<f64>,
    ) -> Self {
        let [x1, y1, x2, y2] = points;
        let y1 = slope.map_or(y1, |slope| x1 * slope.clamp(-1000.0, 1000.0));
        Self {
            delta,
            start,
            duration,
            points,
            ease: Bezier::new(x1, y1, x2, y2),
        }
    }

    fn from_rest(delta: Point<f32>, start: Instant) -> Self {
        Self::new(delta, start, duration(delta), EASE_IN_OUT, None)
    }

    /// How far through its time the leg is at `now`, 0 to 1.
    fn time(&self, now: Instant) -> f64 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.start).as_secs_f64();
        (elapsed / self.duration).min(1.0)
    }

    /// What the leg has still to travel after `now`.
    fn remaining(&self, now: Instant) -> Point<f32> {
        let left = 1.0 - self.ease.value(self.time(now));
        let axis = |delta: f32| (f64::from(delta) * left) as f32;
        point(axis(self.delta.x), axis(self.delta.y))
    }

    /// Speed along the larger axis at `now`, in lines per second
    /// (`CalculateVelocity`).
    fn velocity(&self, now: Instant) -> f64 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        self.ease.slope(self.time(now)) * f64::from(major(self.delta)) / self.duration
    }

    /// Chrome's `UpdateTarget`, for a target `delta` lines from where this
    /// leg is at `now`: the new leg starts at this one's speed, and is cut
    /// short of `duration` when that speed would carry it there sooner.
    fn retarget(&self, delta: Point<f32>, now: Instant, duration: f64) -> Self {
        let distance = f64::from(major(delta));
        if distance.abs() < EPSILON_LINES {
            return Self::new(delta, now, 0.0, self.points, None);
        }
        let velocity = self.velocity(now);
        // `VelocityBasedDurationBound`: at this speed, with some allowance
        // for the ease-out. None when still or heading the other way.
        let bound = (velocity.abs() >= EPSILON_LINES)
            .then(|| distance / velocity * 2.5)
            .filter(|bound| *bound >= 0.0);
        let duration = bound.map_or(duration, |bound| duration.min(bound));
        if duration < 0.01 {
            return Self::new(delta, now, 0.0, self.points, None);
        }
        let slope = velocity * duration / distance;
        Self::new(delta, now, duration, self.points, Some(slope))
    }
}

/// Chromium's `gfx::CubicBezier`: the unit cubic from (0, 0) to (1, 1),
/// read as y of x.
struct Bezier {
    ax: f64,
    bx: f64,
    cx: f64,
    ay: f64,
    by: f64,
    cy: f64,
}

impl Bezier {
    const EPSILON: f64 = 1e-7;

    fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        let (cx, cy) = (3.0 * x1, 3.0 * y1);
        let (bx, by) = (3.0 * (x2 - x1) - cx, 3.0 * (y2 - y1) - cy);
        Self {
            ax: 1.0 - cx - bx,
            bx,
            cx,
            ay: 1.0 - cy - by,
            by,
            cy,
        }
    }

    fn x(&self, t: f64) -> f64 {
        ((self.ax * t + self.bx) * t + self.cx) * t
    }

    fn y(&self, t: f64) -> f64 {
        ((self.ay * t + self.by) * t + self.cy) * t
    }

    fn dx(&self, t: f64) -> f64 {
        (3.0 * self.ax * t + 2.0 * self.bx) * t + self.cx
    }

    fn dy(&self, t: f64) -> f64 {
        (3.0 * self.ay * t + 2.0 * self.by) * t + self.cy
    }

    /// The parameter where the curve reaches `x`: Newton's method, then
    /// bisection (`SolveCurveX`).
    fn solve(&self, x: f64) -> f64 {
        let mut t = x;
        for _ in 0..4 {
            let error = self.x(t) - x;
            if error.abs() < Self::EPSILON {
                return t;
            }
            let slope = self.dx(t);
            if slope.abs() < Self::EPSILON {
                break;
            }
            t -= error / slope;
        }
        let (mut low, mut high) = (0.0, 1.0);
        t = x;
        for _ in 0..64 {
            let at = self.x(t);
            if (at - x).abs() < Self::EPSILON {
                break;
            }
            if at < x {
                low = t;
            } else {
                high = t;
            }
            t = (low + high) / 2.0;
        }
        t
    }

    fn value(&self, x: f64) -> f64 {
        self.y(self.solve(x.clamp(0.0, 1.0)))
    }

    /// dy/dx at `x` (`Slope`).
    fn slope(&self, x: f64) -> f64 {
        let t = self.solve(x.clamp(0.0, 1.0));
        let (dx, dy) = (self.dx(t), self.dy(t));
        if dx == 0.0 && dy == 0.0 { 0.0 } else { dy / dx }
    }
}

fn intercept(
    root: &Rc<RefCell<Root>>,
    event: &ScrollWheelEvent,
    keep_raw: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let mut state = root.borrow_mut();
    if state.replaying {
        return;
    }
    let lines = match event.delta {
        // Nothing to glide, and nothing to interrupt one in flight.
        ScrollDelta::Lines(lines) if lines == Point::default() => return,
        ScrollDelta::Lines(lines) if !keep_raw && !commanded(event.modifiers) && enabled(cx) => {
            lines
        }
        _ => {
            state.glide.stop();
            return;
        }
    };
    cx.stop_propagation();
    state.glide.push(lines, now());
    if !state.scheduled {
        state.scheduled = true;
        drop(state);
        schedule(root.clone(), window);
    }
}

fn schedule(root: Rc<RefCell<Root>>, window: &Window) {
    window.on_next_frame(move |window, cx| frame(root, window, cx));
}

/// Dispatch this frame's slice, then keep frames coming until the glide lands.
fn frame(root: Rc<RefCell<Root>>, window: &mut Window, cx: &mut App) {
    let slice = {
        let mut state = root.borrow_mut();
        state.scheduled = false;
        if commanded(window.modifiers()) {
            // The rest must not arrive as a zoom.
            state.glide.stop();
            return;
        }
        let slice = if enabled(cx) {
            state.glide.advance(now())
        } else {
            state.glide.finish()
        };
        state.replaying = true;
        slice
    };
    if slice != Point::default() {
        window.dispatch_event(
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: window.mouse_position(),
                delta: ScrollDelta::Lines(slice),
                modifiers: window.modifiers(),
                touch_phase: TouchPhase::Moved,
            }),
            cx,
        );
    }
    let mut state = root.borrow_mut();
    state.replaying = false;
    if state.glide.active() && !state.scheduled {
        state.scheduled = true;
        drop(state);
        schedule(root, window);
    }
}

pub struct SmoothScroll {
    child: AnyElement,
}

impl Element for SmoothScroll {
    type RequestLayoutState = ();
    type PrepaintState = ();

    /// None: the wrapper must not change the global ids beneath it.
    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let root = window.with_global_id("smooth-scroll".into(), |id, window| {
            window.with_element_state(id, |root: Option<Rc<RefCell<Root>>>, _| {
                let root = root.unwrap_or_default();
                (root.clone(), root)
            })
        });
        GLIDING.set(enabled(cx));
        // Both first in their capture phase, where nothing in the tree can
        // stop them: a claim left by a wheel event that never reached the
        // root listener can't carry over, and a press (a scrollbar grab, a
        // click to stop) always takes over from a glide.
        window.on_mouse_event(|_: &ScrollWheelEvent, phase, _, _| {
            if phase == DispatchPhase::Capture {
                KEEP_RAW.set(false);
            }
        });
        let press = root.clone();
        window.on_mouse_event(move |_: &MouseDownEvent, phase, _, _| {
            if phase == DispatchPhase::Capture {
                press.borrow_mut().glide.stop();
            }
        });
        self.child.paint(window, cx);
        // After the whole tree, so every region's claim is in.
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                intercept(&root, event, KEEP_RAW.take(), window, cx);
            }
        });
    }
}

impl IntoElement for SmoothScroll {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, ModifiersChangedEvent, MouseButton, Render, ScrollHandle, TestAppContext,
        VisualTestContext, div, prelude::*, px,
    };
    use std::time::Duration;

    thread_local! {
        pub(super) static CLOCK: Cell<Option<Instant>> = const { Cell::new(None) };
    }

    /// 120Hz.
    const FRAME: Duration = Duration::from_millis(8);

    /// Cut every slice of a glide to rest, one `frame` apart.
    fn land(glide: &mut Glide, now: &mut Instant, frame: Duration) -> Vec<Point<f32>> {
        let mut slices = Vec::new();
        while glide.active() {
            *now += frame;
            slices.push(glide.advance(*now));
            assert!(slices.len() < 500, "the glide never landed");
        }
        slices
    }

    fn total(slices: &[Point<f32>]) -> Point<f32> {
        slices.iter().fold(Point::default(), |sum, slice| {
            point(sum.x + slice.x, sum.y + slice.y)
        })
    }

    #[test]
    fn a_notch_eases_in_and_out_and_lands_on_exactly_its_lines() {
        let start = Instant::now();
        let mut now = start;
        let mut glide = Glide::default();
        glide.push(point(0.0, -3.0), now);
        // The curve is symmetric: halfway through its 200ms, halfway there.
        let leg = glide.segment.as_ref().unwrap();
        let half = leg.remaining(start + Duration::from_millis(100));
        assert!((half.y + 1.5).abs() < 1e-5, "{half:?}");

        let slices = land(&mut glide, &mut now, FRAME);
        assert_eq!(total(&slices), point(0.0, -3.0));
        assert_eq!(slices.len(), 25, "lands on the first frame from 200ms");
        // Slow, fast, slow: slices grow to the middle frame, then shrink
        // (give or take the grid).
        let speeds: Vec<f32> = slices.iter().map(|slice| -slice.y).collect();
        let peak = (0..speeds.len())
            .max_by(|a, b| speeds[*a].total_cmp(&speeds[*b]))
            .unwrap();
        assert!((11..=13).contains(&peak), "fastest at frame {peak}");
        let mut rising = speeds[..=peak].windows(2);
        assert!(rising.all(|pair| pair[1] >= pair[0] - GRID_LINES));
        let mut falling = speeds[peak..].windows(2);
        assert!(falling.all(|pair| pair[1] <= pair[0] + GRID_LINES));
        assert!(speeds[0] > 0.0 && speeds[0] < speeds[peak] / 10.0);
        assert!(speeds[24] > 0.0 && speeds[24] < speeds[peak] / 10.0);
    }

    #[test]
    fn longer_moves_take_less_time_as_in_chrome() {
        for (lines, ms) in [
            (1.0, 200.0),
            (3.0, 200.0),
            (7.5, 150.0),
            (12.0, 100.0),
            (40.0, 100.0),
        ] {
            let took = duration(point(0.0, -lines)) * 1000.0;
            assert!((took - ms).abs() < 1e-9, "{lines} lines took {took}ms");
        }
        assert_eq!(duration(point(-12.0, 1.0)), duration(point(0.0, 12.0)));
    }

    #[test]
    fn a_retarget_matches_chromiums_own_numbers() {
        // cc's `EaseInOutUpdateTarget` test, on the curve it uses (desktop
        // programmatic scroll, (0.4, 0, 0, 1)) and its constant 150ms legs.
        let points = [0.4, 0.0, 0.0, 1.0];
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let first = Segment::new(point(0.0, 3600.0), start, 0.15, points, None);
        let left = first.remaining(at(75));
        assert!((3600.0 - left.y - 3101.4792).abs() < 0.002, "{left:?}");

        let second = first.retarget(point(0.0, left.y + 6300.0), at(75), 0.15);
        assert!((second.duration - 0.15).abs() < 0.0002);
        let position = |ms| 9900.0 - second.remaining(at(ms)).y;
        assert!((position(150) - 9097.8).abs() < 0.1, "{}", position(150));
        assert!((position(225) - 9900.0).abs() < 0.0002);
    }

    #[test]
    fn a_notch_in_flight_keeps_the_glide_at_speed() {
        let mut now = Instant::now();
        let mut glide = Glide::default();
        glide.push(point(0.0, -3.0), now);
        let mut before = 0.0;
        for _ in 0..6 {
            now += FRAME;
            before = glide.advance(now).y;
        }
        glide.push(point(0.0, -3.0), now);
        now += FRAME;
        let after = glide.advance(now).y;
        // Eased in again from rest, it would slow to a fifth of that.
        assert!(after <= before, "{after} after {before}");
    }

    #[test]
    fn notches_in_flight_merge_into_one_glide() {
        let mut now = Instant::now();
        let mut glide = Glide::default();
        glide.push(point(0.0, -3.0), now);
        let mut delivered = 0.0;
        for _ in 0..3 {
            now += FRAME;
            delivered += glide.advance(now).y;
        }
        glide.push(point(0.0, -3.0), now);
        delivered += total(&land(&mut glide, &mut now, FRAME)).y;
        assert_eq!(delivered, -6.0);
    }

    #[test]
    fn reversing_drops_the_rest_of_the_old_direction() {
        let mut now = Instant::now();
        let mut glide = Glide::default();
        glide.push(point(-1.0, -3.0), now);
        now += FRAME;
        let first = glide.advance(now);
        assert!(first.y < 0.0 && first.y > -3.0);
        // Up on y only: x keeps gliding left.
        glide.push(point(0.0, 3.0), now);
        assert_eq!(glide.pending.y, 3.0);
        let rest = total(&land(&mut glide, &mut now, FRAME));
        assert_eq!(rest.y, 3.0);
        assert_eq!(first.x + rest.x, -1.0);
    }

    #[test]
    fn the_curve_does_not_depend_on_frame_rate() {
        let position = |frame: Duration, frames: u32| {
            let mut now = Instant::now();
            let mut glide = Glide::default();
            glide.push(point(0.0, 3.0), now);
            (0..frames)
                .map(|_| {
                    now += frame;
                    glide.advance(now).y
                })
                .sum::<f32>()
        };
        let at_60hz = position(Duration::from_millis(16), 3);
        let at_120hz = position(FRAME, 6);
        assert!(
            (at_60hz - at_120hz).abs() < 0.002,
            "{at_60hz} vs {at_120hz}"
        );
    }

    #[test]
    fn a_whole_row_accumulator_receives_every_row() {
        // The terminal's: whole rows scroll, the fraction carries over.
        let mut now = Instant::now();
        let mut glide = Glide::default();
        let (mut remainder, mut rows) = (0.0f32, 0.0f32);
        for _ in 0..7 {
            glide.push(point(0.0, -3.0), now);
            for slice in land(&mut glide, &mut now, FRAME) {
                remainder += slice.y;
                let step = remainder.trunc();
                remainder -= step;
                rows += step;
            }
        }
        assert_eq!((rows, remainder), (-21.0, 0.0));
    }

    #[test]
    fn a_fractional_notch_lands_whole() {
        // macOS reports slow notches from some mice as fractions of a line.
        let mut now = Instant::now();
        let mut glide = Glide::default();
        glide.push(point(0.0, 0.1), now);
        let delivered = total(&land(&mut glide, &mut now, FRAME)).y;
        assert!((delivered - 0.1).abs() < 1e-6, "{delivered}");
    }

    struct Fixture {
        scroll: ScrollHandle,
        raw: bool,
    }

    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            smooth_scroll(
                div()
                    .relative()
                    .size(px(200.0))
                    .child(
                        div()
                            .id("list")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(div().w_full().h(px(5000.0))),
                    )
                    .when(self.raw, |el| el.child(raw_wheel_region())),
            )
        }
    }

    fn open(cx: &mut TestAppContext, raw: bool) -> (ScrollHandle, &mut VisualTestContext) {
        let scroll = ScrollHandle::new();
        let (_, cx) = cx.add_window_view(|_, _| Fixture {
            scroll: scroll.clone(),
            raw,
        });
        cx.update(|window, cx| window.draw(cx).clear());
        (scroll, cx)
    }

    fn init(cx: &mut TestAppContext) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx)
        });
        CLOCK.set(Some(Instant::now()));
        dir
    }

    fn wheel(cx: &mut VisualTestContext, delta: ScrollDelta, modifiers: Modifiers) {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(100.0), px(100.0)),
            delta,
            modifiers,
            touch_phase: TouchPhase::Moved,
        });
        cx.update(|window, cx| window.draw(cx).clear());
    }

    fn notch(cx: &mut VisualTestContext) {
        wheel(
            cx,
            ScrollDelta::Lines(point(0.0, -3.0)),
            Modifiers::default(),
        );
    }

    /// One frame later: run the queued frame callbacks and draw. Returns how
    /// many callbacks ran.
    fn next_frame(cx: &mut VisualTestContext) -> usize {
        CLOCK.set(CLOCK.get().map(|now| now + FRAME));
        cx.update(|window, cx| {
            let ran = window.simulate_next_frame(cx);
            window.draw(cx).clear();
            ran
        })
    }

    fn settle(cx: &mut VisualTestContext) {
        for _ in 0..100 {
            if next_frame(cx) == 0 {
                return;
            }
        }
        panic!("the glide never landed");
    }

    fn top(scroll: &ScrollHandle) -> f32 {
        f32::from(scroll.offset().y)
    }

    fn set_smooth(enabled: bool, cx: &mut VisualTestContext) {
        cx.update(|_, cx| crate::settings::set_smooth_scrolling(enabled, cx));
    }

    #[gpui::test]
    fn a_notch_glides_as_far_as_it_used_to_jump(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let (scroll, cx) = open(cx, false);
        set_smooth(false, cx);
        notch(cx);
        let jump = top(&scroll);
        assert!(jump < 0.0);
        assert_eq!(next_frame(cx), 0, "an instant notch queues no frames");

        scroll.set_offset(point(px(0.0), px(0.0)));
        set_smooth(true, cx);
        notch(cx);
        assert_eq!(top(&scroll), 0.0, "the root holds the notch");
        assert_eq!(next_frame(cx), 1);
        let first = top(&scroll);
        assert!(first < 0.0 && first > jump * 0.5, "{first} of {jump}");
        settle(cx);
        assert!(
            (top(&scroll) - jump).abs() < 0.01,
            "{} vs {jump}",
            top(&scroll)
        );
    }

    #[gpui::test]
    fn precise_modified_and_reduced_input_scrolls_at_once(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let (scroll, cx) = open(cx, false);
        let expect_instant = |cx: &mut VisualTestContext, delta, modifiers| {
            scroll.set_offset(point(px(0.0), px(0.0)));
            cx.update(|window, cx| window.draw(cx).clear());
            wheel(cx, delta, modifiers);
            assert!(top(&scroll) < 0.0, "{delta:?} {modifiers:?} was held");
            assert_eq!(next_frame(cx), 0, "{delta:?} {modifiers:?} queued a glide");
        };
        let notch = ScrollDelta::Lines(point(0.0, -3.0));
        expect_instant(
            cx,
            ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            Modifiers::default(),
        );
        // Gestures that mean something else keep their discrete steps; the
        // div itself still scrolls on them.
        for modifiers in [Modifiers::control(), Modifiers::alt(), Modifiers::command()] {
            expect_instant(cx, notch, modifiers);
        }
        cx.update(|_, cx| cx.set_reduce_motion(true));
        expect_instant(cx, notch, Modifiers::default());
    }

    #[gpui::test]
    fn an_empty_notch_queues_no_frames(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let (_, cx) = open(cx, false);
        wheel(
            cx,
            ScrollDelta::Lines(point(0.0, 0.0)),
            Modifiers::default(),
        );
        assert_eq!(next_frame(cx), 0);
    }

    #[gpui::test]
    fn a_raw_region_keeps_its_notches(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let (scroll, cx) = open(cx, true);
        notch(cx);
        assert!(top(&scroll) < 0.0);
        assert_eq!(next_frame(cx), 0);
    }

    #[gpui::test]
    fn a_press_a_trackpad_or_a_modifier_ends_a_glide(cx: &mut TestAppContext) {
        let _dir = init(cx);
        let (scroll, cx) = open(cx, false);
        let interruptions: [&dyn Fn(&mut VisualTestContext); 3] = [
            &|cx| {
                cx.simulate_event(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(100.0), px(100.0)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                })
            },
            &|cx| {
                wheel(
                    cx,
                    ScrollDelta::Pixels(point(px(0.0), px(0.0))),
                    Modifiers::default(),
                )
            },
            &|cx| {
                cx.simulate_event(ModifiersChangedEvent {
                    modifiers: Modifiers::control(),
                    capslock: Default::default(),
                })
            },
        ];
        for interrupt in interruptions {
            cx.simulate_event(ModifiersChangedEvent::default());
            scroll.set_offset(point(px(0.0), px(0.0)));
            notch(cx);
            next_frame(cx);
            let stopped = top(&scroll);
            assert!(stopped < 0.0);
            interrupt(cx);
            settle(cx);
            assert_eq!(top(&scroll), stopped);
        }
    }
}
