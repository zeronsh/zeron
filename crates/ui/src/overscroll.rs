//! [`overscroll`] — macOS-style rubber band at a scroll container's ends.
//!
//! GPUI clamps every scroll offset (div prepaint, `ListState::scroll`), so the
//! stretch is a visual layer over the container rather than an offset the
//! container knows about: the wrapper shifts its child's prepaint by the band
//! and clips it to its own bounds, revealing the surface behind the pulled end.
//!
//! Input is read in the CAPTURE phase, before the container's own wheel
//! listener. Outward deltas always propagate — at an end the container clamps
//! them to a no-op and its scroll handler still fires (transcript pin/anchor
//! release, file-tree rail) exactly as without the band. Only inward deltas
//! that the band absorbs stop propagation, so the content returns to the edge
//! before the container scrolls again; a delta larger than the band collapses
//! it and reaches the container whole.
//!
//! GPUI maps NSEvent `phase` but not `momentumPhase`, so inertia is inferred:
//! the phase-less `Moved` events that follow `Ended` without a pause are the
//! fling's momentum. A fling that reaches an end bounces once (an impulse
//! sized from its velocity); the rest of it is clamped as usual. Phase-less
//! streams that follow no touch (precise mouse wheels, synthetic input) are
//! not momentum and never bounce. A touch that never scrolls arrives as
//! `Started` then a zero `Moved` (NSEventPhaseCancelled is unmapped) and is
//! treated as a cancel.
//!
//! Trackpad only (`ScrollDelta::Pixels`), vertical only, macOS only (other
//! platforms don't deliver reliable touch phases), and off under reduced
//! motion or while a drag is active (drag code maps the pointer through the
//! container's unshifted bounds).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ContentMask, DispatchPhase, Element, ElementId, GlobalElementId,
    Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, ListState, Pixels, Point,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, TouchPhase, Window, point, px,
};

/// Apple's UIScrollView coefficient: travel `x` past the end of a `d`-tall
/// viewport shows as `(1 - 1 / (x * c / d + 1)) * d`.
const RUBBER_BAND_COEFFICIENT: f32 = 0.55;
/// Angular frequency (1/s) of the critically damped return; settles in ~0.45s.
const SPRING_OMEGA: f32 = 13.0;
/// Peak of a momentum bounce: a share of the viewport, capped in px.
const BOUNCE_MAX_FRACTION: f32 = 0.12;
const BOUNCE_MAX_PX: f32 = 80.0;
/// A phase-less stream after this much silence is a new fling.
const FLING_GAP: Duration = Duration::from_millis(120);

const ENABLED: bool = cfg!(any(target_os = "macos", test));

/// Wrap the scroll container `child` (the element that owns `source`) so its
/// ends rubber-band. Layout passes straight through to the child.
pub fn overscroll(
    id: impl Into<ElementId>,
    source: OverscrollSource,
    child: impl IntoElement,
) -> Overscroll {
    Overscroll {
        id: id.into(),
        source,
        child: child.into_any_element(),
    }
}

/// The scroll state the band reads the container's ends from.
#[derive(Clone)]
pub enum OverscrollSource {
    Handle(ScrollHandle),
    List(ListState),
}

impl OverscrollSource {
    fn room(&self) -> Room {
        match self {
            Self::Handle(handle) => {
                let scrolled = -f32::from(handle.offset().y);
                Room {
                    top: scrolled,
                    bottom: f32::from(handle.max_offset().y) - scrolled,
                }
            }
            Self::List(list) => {
                // The final item's box includes any tail reservation, so its
                // bottom is the list's end. Unmeasured rows only make the
                // content height an estimate; an unmeasured final row means
                // the end is not on screen.
                let bottom = match list.item_count().checked_sub(1) {
                    None => 0.0,
                    Some(last) => list.bounds_for_item(last).map_or(f32::INFINITY, |item| {
                        f32::from(item.bottom() - list.viewport_bounds().bottom())
                    }),
                };
                Room {
                    top: -f32::from(list.scroll_px_offset_for_scrollbar().y),
                    bottom,
                }
            }
        }
    }
}

/// Distance the container can still scroll toward each end, in px.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Room {
    top: f32,
    bottom: f32,
}

impl Room {
    const UNKNOWN: Self = Self {
        top: f32::INFINITY,
        bottom: f32::INFINITY,
    };

    /// Room in the direction of a gpui wheel delta (positive scrolls up).
    fn toward(self, dy: f32) -> f32 {
        if dy > 0.0 { self.top } else { self.bottom }.max(0.0)
    }
}

fn rubber_band(distance: f32, viewport: f32) -> f32 {
    let viewport = viewport.max(1.0);
    let x = distance.abs();
    ((1.0 - 1.0 / (x * RUBBER_BAND_COEFFICIENT / viewport + 1.0)) * viewport).copysign(distance)
}

fn rubber_band_inverse(offset: f32, viewport: f32) -> f32 {
    let viewport = viewport.max(1.0);
    let shown = offset.abs().min(viewport * 0.99);
    (viewport / RUBBER_BAND_COEFFICIENT * (1.0 / (1.0 - shown / viewport) - 1.0)).copysign(offset)
}

/// Move a nonzero `stretch` by `dy`. `None` when the delta is larger than the
/// band: it collapses, and the whole delta belongs to the container — a big
/// reverse swipe scrolls instead of vanishing into a few px of band.
fn travel(stretch: f32, dy: f32) -> Option<f32> {
    let next = stretch + dy;
    (next.signum() == stretch.signum()).then_some(next)
}

#[derive(Clone, Copy, Debug)]
struct Spring {
    from: f32,
    velocity: f32,
    start: Instant,
}

impl Spring {
    /// Critically damped toward 0: `x(t) = (x0 + (v0 + ωx0)t)e^(-ωt)`.
    /// Returns position and velocity.
    fn sample(self, now: Instant) -> (f32, f32) {
        let t = now.saturating_duration_since(self.start).as_secs_f32();
        let b = self.velocity + SPRING_OMEGA * self.from;
        let position = self.from + b * t;
        let decay = (-SPRING_OMEGA * t).exp();
        (position * decay, (b - SPRING_OMEGA * position) * decay)
    }
}

#[derive(Default)]
struct Band {
    /// Shift applied to the child; positive pulls content down past the top.
    offset: f32,
    /// Finger travel past the end, before resistance.
    stretch: f32,
    viewport: f32,
    touching: bool,
    touch_moved: bool,
    /// Phase-less events now are the momentum of a released touch.
    momentum: bool,
    fling_bounced: bool,
    fling_velocity: f32,
    last_event: Option<Instant>,
    spring: Option<Spring>,
}

impl Band {
    /// Feed one trackpad event. Returns whether it must not reach the
    /// container (it unwound the stretch instead of scrolling).
    fn wheel(&mut self, phase: TouchPhase, delta: Point<f32>, room: Room, now: Instant) -> bool {
        let gap = self
            .last_event
            .replace(now)
            .map(|last| now.saturating_duration_since(last));
        match phase {
            TouchPhase::Started => {
                self.touching = true;
                self.touch_moved = false;
                self.momentum = false;
                self.fling_bounced = false;
                return false;
            }
            TouchPhase::Cancelled => {
                self.release(now);
                return false;
            }
            TouchPhase::Moved | TouchPhase::Ended => {}
        }
        let still = delta.x == 0.0 && delta.y == 0.0;
        if phase == TouchPhase::Moved && self.touching && !self.touch_moved && still {
            self.release(now);
            return false;
        }
        if !self.touching && gap.is_none_or(|gap| gap > FLING_GAP) {
            // A pause ends the fling; what follows is not its momentum.
            self.momentum = false;
            self.fling_bounced = false;
            self.fling_velocity = 0.0;
        }
        // Dominantly horizontal travel belongs to nested x-scrollers.
        let dy = if delta.y.abs() >= delta.x.abs() {
            delta.y
        } else {
            0.0
        };
        let consumed = if self.touching {
            self.drag(dy, room)
        } else {
            self.fling(dy, room, gap, now)
        };
        if phase == TouchPhase::Ended {
            self.release(now);
            self.momentum = true;
        }
        consumed
    }

    fn drag(&mut self, dy: f32, room: Room) -> bool {
        if dy == 0.0 {
            return false;
        }
        self.touch_moved = true;
        if self.stretch == 0.0 && self.offset != 0.0 {
            // Caught mid-return: continue from where the content is.
            self.stretch = rubber_band_inverse(self.offset, self.viewport);
        }
        self.spring = None;
        if self.stretch != 0.0 {
            let inward = dy.signum() != self.stretch.signum();
            let Some(next) = travel(self.stretch, dy) else {
                self.stretch = 0.0;
                self.offset = 0.0;
                return false;
            };
            self.stretch = next;
            self.offset = rubber_band(next, self.viewport);
            return inward;
        }
        // The container takes the room it has; the rest stretches the band.
        let past = dy.abs() - room.toward(dy);
        if past > 0.0 {
            self.stretch = past.copysign(dy);
            self.offset = rubber_band(self.stretch, self.viewport);
        }
        false
    }

    fn fling(&mut self, dy: f32, room: Room, gap: Option<Duration>, now: Instant) -> bool {
        if dy == 0.0 {
            return false;
        }
        if self.offset != 0.0 || self.spring.is_some() {
            if self.offset == 0.0 || dy.signum() == self.offset.signum() {
                // Momentum is spent against the end while the band settles.
                return false;
            }
            // Inward input unwinds the settling band like a drag would,
            // restarting the return from where it leaves the content.
            let stretch = rubber_band_inverse(self.offset, self.viewport);
            let Some(next) = travel(stretch, dy) else {
                self.offset = 0.0;
                self.spring = None;
                return false;
            };
            self.offset = rubber_band(next, self.viewport);
            self.spring = Some(Spring {
                from: self.offset,
                velocity: 0.0,
                start: now,
            });
            return true;
        }
        let dt = gap
            .map_or(1.0 / 60.0, |gap| gap.as_secs_f32())
            .clamp(1.0 / 240.0, 1.0 / 30.0);
        let velocity = dy / dt;
        self.fling_velocity =
            if self.fling_velocity != 0.0 && velocity.signum() == self.fling_velocity.signum() {
                0.5 * (self.fling_velocity + velocity)
            } else {
                velocity
            };
        if !self.momentum || self.fling_bounced || dy.abs() <= room.toward(dy) {
            return false;
        }
        self.fling_bounced = true;
        // A critically damped impulse peaks at v / (ωe).
        let peak = (self.viewport * BOUNCE_MAX_FRACTION).min(BOUNCE_MAX_PX);
        let max_velocity = peak * SPRING_OMEGA * std::f32::consts::E;
        self.spring = Some(Spring {
            from: 0.0,
            velocity: self.fling_velocity.clamp(-max_velocity, max_velocity),
            start: now,
        });
        false
    }

    fn release(&mut self, now: Instant) {
        self.touching = false;
        self.touch_moved = false;
        self.stretch = 0.0;
        self.fling_velocity = 0.0;
        // The momentum that follows a stretched release must not bounce again.
        self.fling_bounced = self.offset != 0.0;
        if self.offset != 0.0 && self.spring.is_none() {
            self.spring = Some(Spring {
                from: self.offset,
                velocity: 0.0,
                start: now,
            });
        }
    }

    /// Advance the return spring; true while it still needs frames.
    fn tick(&mut self, now: Instant) -> bool {
        let Some(spring) = self.spring else {
            return false;
        };
        let (offset, velocity) = spring.sample(now);
        if offset.abs() < 0.25 && velocity.abs() < 10.0 {
            self.spring = None;
            self.offset = 0.0;
            return false;
        }
        self.offset = offset;
        true
    }

    fn reset(&mut self) {
        *self = Self {
            viewport: self.viewport,
            ..Self::default()
        };
    }
}

pub struct Overscroll {
    id: ElementId,
    source: OverscrollSource,
    child: AnyElement,
}

pub struct OverscrollPrepaint {
    band: Option<(Rc<RefCell<Band>>, Hitbox)>,
    offset: f32,
}

impl Element for Overscroll {
    type RequestLayoutState = ();
    type PrepaintState = OverscrollPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> OverscrollPrepaint {
        let enabled = ENABLED && !crate::motion::reduced_motion(cx);
        let band = window.with_element_state(
            id.expect("overscroll has an element id"),
            |band: Option<Rc<RefCell<Band>>>, _| {
                let band = band.unwrap_or_default();
                (band.clone(), band)
            },
        );
        let offset = {
            let mut state = band.borrow_mut();
            state.viewport = f32::from(bounds.size.height);
            if !enabled {
                state.reset();
            } else if state.tick(Instant::now()) {
                window.request_animation_frame();
            }
            state.offset
        };
        let band = enabled.then(|| (band, window.insert_hitbox(bounds, HitboxBehavior::Normal)));
        if offset == 0.0 {
            self.child.prepaint(window, cx);
        } else {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                window.with_element_offset(point(px(0.0), px(offset)), |window| {
                    self.child.prepaint(window, cx)
                })
            });
        }
        OverscrollPrepaint { band, offset }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut OverscrollPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some((band, hitbox)) = prepaint.band.clone() {
            let view = window.current_view();
            let source = self.source.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || cx.has_active_drag() {
                    return;
                }
                let ScrollDelta::Pixels(delta) = event.delta else {
                    return;
                };
                let mut band = band.borrow_mut();
                let before = (band.offset, band.spring.is_some());
                let now = Instant::now();
                let consumed = if hitbox.should_handle_scroll(window) {
                    let delta = point(f32::from(delta.x), f32::from(delta.y));
                    band.wheel(event.touch_phase, delta, source.room(), now)
                } else if event.touch_phase != TouchPhase::Moved {
                    // Elsewhere in the window: follow the gesture, not its travel.
                    band.wheel(event.touch_phase, point(0.0, 0.0), Room::UNKNOWN, now)
                } else {
                    false
                };
                if (band.offset, band.spring.is_some()) != before {
                    cx.notify(view);
                }
                if consumed {
                    cx.stop_propagation();
                }
            });
        }
        if prepaint.offset == 0.0 {
            self.child.paint(window, cx);
        } else {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                self.child.paint(window, cx)
            });
        }
    }
}

impl IntoElement for Overscroll {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Modifiers, Render, Styled, canvas, div, prelude::*};

    const AT_TOP: Room = Room {
        top: 0.0,
        bottom: 500.0,
    };

    fn band() -> Band {
        Band {
            viewport: 400.0,
            ..Band::default()
        }
    }

    fn dy(y: f32) -> Point<f32> {
        point(0.0, y)
    }

    #[test]
    fn rubber_band_resists_and_inverts() {
        let mut previous = 0.0;
        for step in 1..=50 {
            let distance = step as f32 * 20.0;
            let shown = rubber_band(distance, 400.0);
            assert!(shown > previous && shown < distance && shown < 400.0);
            assert!((rubber_band_inverse(shown, 400.0) - distance).abs() < 0.5);
            assert_eq!(rubber_band(-distance, 400.0), -shown);
            previous = shown;
        }
    }

    #[test]
    fn drag_past_an_end_stretches_and_unwinds_before_scrolling() {
        let now = Instant::now();
        let mut band = band();
        band.wheel(TouchPhase::Started, dy(0.0), AT_TOP, now);
        // Outward at the top: the container clamps it, the band shows it.
        assert!(!band.wheel(TouchPhase::Moved, dy(60.0), AT_TOP, now));
        let stretched = band.offset;
        assert!(stretched > 0.0 && stretched < 60.0);
        // Inward while stretched: unwinds the band, never reaches the list.
        assert!(band.wheel(TouchPhase::Moved, dy(-20.0), AT_TOP, now));
        assert!(band.offset > 0.0 && band.offset < stretched);
        // Larger than the band: it collapses and the list gets the delta.
        assert!(!band.wheel(TouchPhase::Moved, dy(-100.0), AT_TOP, now));
        assert_eq!(band.offset, 0.0);
        // At rest again, inward travel scrolls the container normally.
        assert!(!band.wheel(TouchPhase::Moved, dy(-20.0), AT_TOP, now));
        assert_eq!(band.offset, 0.0);
    }

    #[test]
    fn phase_less_input_without_a_touch_never_bounces() {
        // Precise wheels and synthetic input: no Started/Ended around them.
        let mut now = Instant::now();
        let mut band = band();
        for _ in 0..20 {
            now += Duration::from_millis(8);
            assert!(!band.wheel(TouchPhase::Moved, dy(-60.0), AT_TOP, now));
            assert!(!band.wheel(TouchPhase::Moved, dy(60.0), AT_TOP, now));
        }
        assert!(band.spring.is_none());
        assert_eq!(band.offset, 0.0);
    }

    #[test]
    fn reverse_input_during_a_bounce_reaches_the_container() {
        let mut now = Instant::now();
        let mut band = band();
        band.wheel(TouchPhase::Started, dy(0.0), AT_TOP, now);
        band.wheel(TouchPhase::Moved, dy(-10.0), AT_TOP, now);
        band.wheel(TouchPhase::Ended, dy(0.0), AT_TOP, now);
        now += Duration::from_millis(8);
        band.wheel(TouchPhase::Moved, dy(40.0), AT_TOP, now);
        now += Duration::from_millis(40);
        band.tick(now);
        let bounced = band.offset;
        assert!(bounced > 1.0);
        // A small inward nudge is absorbed and restarts the return there.
        now += Duration::from_millis(8);
        assert!(band.wheel(TouchPhase::Moved, dy(-0.5), AT_TOP, now));
        assert!(band.offset > 0.0 && band.offset < bounced);
        // A real reverse swipe collapses the band and scrolls.
        now += Duration::from_millis(8);
        assert!(!band.wheel(TouchPhase::Moved, dy(-300.0), AT_TOP, now));
        assert_eq!(band.offset, 0.0);
        assert!(band.spring.is_none());
    }

    #[test]
    fn drag_near_an_end_stretches_only_by_the_excess() {
        let now = Instant::now();
        let mut band = band();
        band.wheel(TouchPhase::Started, dy(0.0), AT_TOP, now);
        let room = Room {
            top: 500.0,
            bottom: 10.0,
        };
        assert!(!band.wheel(TouchPhase::Moved, dy(-30.0), room, now));
        assert_eq!(band.stretch, -20.0);
        assert!(band.offset < 0.0);
    }

    #[test]
    fn release_springs_back_to_rest() {
        let now = Instant::now();
        let mut band = band();
        band.wheel(TouchPhase::Started, dy(0.0), AT_TOP, now);
        band.wheel(TouchPhase::Moved, dy(80.0), AT_TOP, now);
        band.wheel(TouchPhase::Ended, dy(0.0), AT_TOP, now);
        assert!(band.tick(now + Duration::from_millis(50)));
        assert!(band.offset > 0.0);
        // The fling that follows a stretched release keeps pushing at the
        // end through and after the return, and never bounces again.
        for frame in 1..=120 {
            let at = now + Duration::from_millis(8 * frame);
            band.tick(at);
            assert!(!band.wheel(TouchPhase::Moved, dy(5.0), AT_TOP, at));
            assert!(band.spring.is_none_or(|spring| spring.from != 0.0));
        }
        assert!(!band.tick(now + Duration::from_secs(2)));
        assert_eq!(band.offset, 0.0);
    }

    #[test]
    fn momentum_reaching_an_end_bounces_once() {
        let mut now = Instant::now();
        let mut band = band();
        let room = Room {
            top: 50.0,
            bottom: 500.0,
        };
        band.wheel(TouchPhase::Started, dy(0.0), room, now);
        band.wheel(TouchPhase::Moved, dy(20.0), room, now);
        band.wheel(TouchPhase::Ended, dy(0.0), room, now);
        assert!(band.spring.is_none());
        for _ in 0..3 {
            now += Duration::from_millis(8);
            assert!(!band.wheel(TouchPhase::Moved, dy(40.0), AT_TOP, now));
        }
        let spring = band.spring.expect("momentum at the end bounces");
        assert!(spring.velocity > 0.0);
        // The bounce peaks within its cap, then settles.
        let mut peak: f32 = 0.0;
        for ms in (0..600).step_by(4) {
            band.tick(now + Duration::from_millis(ms));
            peak = peak.max(band.offset);
        }
        assert!(peak > 10.0 && peak <= 400.0 * BOUNCE_MAX_FRACTION + 0.5);
        assert!(!band.tick(now + Duration::from_secs(2)));
        assert_eq!(band.offset, 0.0);
    }

    #[test]
    fn touch_without_scrolling_cancels_and_releases() {
        let now = Instant::now();
        let mut band = band();
        band.wheel(TouchPhase::Started, dy(0.0), AT_TOP, now);
        band.wheel(TouchPhase::Moved, dy(60.0), AT_TOP, now);
        band.wheel(TouchPhase::Ended, dy(0.0), AT_TOP, now);
        // Fingers rest mid-return and lift without moving.
        band.wheel(TouchPhase::Started, dy(0.0), AT_TOP, now);
        band.wheel(TouchPhase::Moved, dy(0.0), AT_TOP, now);
        assert!(!band.touching);
        assert!(band.spring.is_some());
    }

    #[test]
    fn horizontal_travel_is_ignored() {
        let now = Instant::now();
        let mut band = band();
        band.wheel(TouchPhase::Started, dy(0.0), AT_TOP, now);
        band.wheel(TouchPhase::Moved, point(-50.0, 20.0), AT_TOP, now);
        assert_eq!(band.offset, 0.0);
    }

    #[gpui::test]
    fn stretched_div_shifts_content_without_scrolling_it(cx: &mut gpui::TestAppContext) {
        struct Fixture {
            scroll: ScrollHandle,
            top: Rc<RefCell<f32>>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let top = self.top.clone();
                div().size(px(200.0)).child(overscroll(
                    "band",
                    OverscrollSource::Handle(self.scroll.clone()),
                    div()
                        .id("list")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(
                            canvas(
                                |_, _, _| (),
                                move |bounds, _, _, _| *top.borrow_mut() = f32::from(bounds.top()),
                            )
                            .w_full()
                            .h(px(1000.0)),
                        ),
                ))
            }
        }
        let scroll = ScrollHandle::new();
        let top = Rc::new(RefCell::new(0.0));
        let (_, cx) = cx.add_window_view(|_, _| Fixture {
            scroll: scroll.clone(),
            top: top.clone(),
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let mut wheel = |y: f32, phase| {
            cx.simulate_event(ScrollWheelEvent {
                position: point(px(100.0), px(100.0)),
                delta: ScrollDelta::Pixels(point(px(0.0), px(y))),
                modifiers: Modifiers::default(),
                touch_phase: phase,
            });
            cx.update(|window, cx| window.draw(cx).clear());
        };
        wheel(0.0, TouchPhase::Started);
        wheel(40.0, TouchPhase::Moved);
        assert_eq!(scroll.offset().y, px(0.0));
        let stretched = *top.borrow();
        assert!(stretched > 0.0 && stretched < 40.0);
        wheel(-10.0, TouchPhase::Moved);
        assert_eq!(scroll.offset().y, px(0.0), "unwinding must not scroll");
        assert!(*top.borrow() < stretched);
        wheel(-100.0, TouchPhase::Moved);
        assert_eq!(
            scroll.offset().y,
            px(-100.0),
            "a swipe past the band scrolls"
        );
        assert_eq!(*top.borrow(), -100.0);
        wheel(-30.0, TouchPhase::Moved);
        assert_eq!(scroll.offset().y, px(-130.0));
    }
}
