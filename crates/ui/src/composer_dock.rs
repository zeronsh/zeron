//! One retargetable clock for the main composer's route choreography. Geometry
//! is measured in prepaint, so a resize never substitutes a guessed endpoint.

use std::{cell::RefCell, rc::Rc, time::Instant};

mod panel_handoff;

use gpui::{
    AnyElement, App, Bounds, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Window, point, px,
};

/// Critically damped motion: no oscillation, and both position and velocity
/// survive a new target. Twelve time constants settle within a fraction of a
/// pixel over the intended 420/470ms handoff, even across a large window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Glide {
    pub value: f32,
    pub velocity: f32,
    target: f32,
}

impl Glide {
    pub fn new(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
            target: value,
        }
    }

    pub fn advance(&mut self, target: f32, seconds: f32, duration: f32) {
        self.target = target;
        let omega = 12.0 / duration;
        let displacement = self.value - target;
        let c = self.velocity + omega * displacement;
        let decay = (-omega * seconds).exp();
        self.value = target + (displacement + c * seconds) * decay;
        self.velocity = (self.velocity - omega * c * seconds) * decay;
        if !self.active() {
            *self = Self::new(target);
        }
    }

    fn active(&self) -> bool {
        (self.value - self.target).abs() > 0.0005 || self.velocity.abs() > 0.005
    }
}

pub(crate) fn stage(value: f32, start: f32, end: f32) -> f32 {
    let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DockFrame {
    /// Canonical position: zero is the hero, one is the established thread.
    pub amount: f32,
    pub docked: bool,
    pub active: bool,
    visuals: Visuals,
}

impl DockFrame {
    pub fn settled(docked: bool) -> Self {
        Self {
            amount: if docked { 1.0 } else { 0.0 },
            docked,
            active: false,
            visuals: Visuals::settled(docked),
        }
    }

    pub fn transcript(self) -> f32 {
        self.visuals.transcript
    }
    pub fn selectors(self) -> f32 {
        self.visuals.selectors
    }
    pub fn footer(self) -> f32 {
        self.visuals.footer
    }
    pub fn dissolve(self) -> f32 {
        self.visuals.dissolve
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Visuals {
    transcript: f32,
    selectors: f32,
    footer: f32,
    dissolve: f32,
}

impl Visuals {
    fn settled(docked: bool) -> Self {
        let value = if docked { 1.0 } else { 0.0 };
        Self {
            transcript: value,
            selectors: 1.0 - value,
            footer: value,
            dissolve: value,
        }
    }

    fn advance(self, docked: bool, time: f32) -> Self {
        let target = Self::settled(docked);
        let blend = |from, to, start, end| crate::motion::lerp(from, to, stage(time, start, end));
        if docked {
            Self {
                transcript: blend(self.transcript, target.transcript, 0.20, 0.65),
                selectors: blend(self.selectors, target.selectors, 0.55, 0.78),
                footer: blend(self.footer, target.footer, 0.78, 1.0),
                dissolve: blend(self.dissolve, target.dissolve, 0.06, 0.88),
            }
        } else {
            // On return, release thread chrome first; unfold the hero behind
            // the rising input and restore destination selectors near arrival.
            Self {
                transcript: blend(self.transcript, target.transcript, 0.0, 0.25),
                selectors: blend(self.selectors, target.selectors, 0.50, 0.95),
                footer: blend(self.footer, target.footer, 0.0, 0.18),
                dissolve: blend(self.dissolve, target.dissolve, 0.08, 0.85),
            }
        }
    }

    fn return_from_panel(self, time: f32) -> Self {
        // The short fade-through has its own clock: destination controls must
        // arrive with the input, not trail the longer vertical-glide schedule.
        Self {
            transcript: self.transcript * (1.0 - stage(time, 0.0, 0.18)),
            footer: self.footer * (1.0 - stage(time, 0.0, 0.18)),
            selectors: crate::motion::lerp(self.selectors, 1.0, stage(time, 0.26, 0.85)),
            // The mask follows the actual surface. Keep it hidden through
            // the 0.22 horizontal geometry switch, then reveal both together.
            dissolve: self.dissolve * (1.0 - stage(time, 0.26, 0.80)),
        }
    }
}

pub(crate) struct DockState {
    pane: panel_handoff::PanelHandoff,
    phase: Glide,
    last_frame: Option<Instant>,
    pub frame: DockFrame,
    position: Option<(Glide, Glide)>,
    last_geometry: Option<Instant>,
    last_docked: bool,
    moving: bool,
    width: Option<Glide>,
    last_width_frame: Option<Instant>,
    route_changed: bool,
    choreography: Option<(Instant, Visuals)>,
    panel_return: bool,
    panel_departure: bool,
    column_width: Option<f32>,
    departing_column_width: Option<f32>,
}

impl Default for DockState {
    fn default() -> Self {
        Self {
            pane: Default::default(),
            phase: Glide::new(0.0),
            last_frame: None,
            frame: DockFrame::settled(false),
            position: None,
            last_geometry: None,
            last_docked: false,
            moving: false,
            width: None,
            last_width_frame: None,
            route_changed: false,
            choreography: None,
            panel_return: false,
            panel_departure: false,
            column_width: None,
            departing_column_width: None,
        }
    }
}

impl DockState {
    /// Retained transcript pixels belong to the source column. Letting them
    /// reflow into the hero's wider layout before fading creates an exit flash.
    pub fn transcript_width(&mut self, target: f32, docked: bool, panel_handoff: bool) -> f32 {
        if !docked && self.frame.docked && panel_handoff {
            self.departing_column_width = self.column_width;
        }
        if docked || !panel_handoff {
            self.departing_column_width = None;
        }
        self.column_width = Some(target);
        self.departing_column_width.unwrap_or(target)
    }

    pub fn observe_pane(&mut self, docked: bool, target: f32, enabled: bool, now: Instant) -> bool {
        self.pane.sample(
            docked,
            target,
            enabled,
            now,
            0.320 * crate::motion::speed_scale(),
        )
    }

    pub fn opacity(&self) -> f32 {
        self.pane.opacity()
    }

    pub fn layout_width(&mut self, target: f32, reduced: bool, now: Instant) -> f32 {
        let dt = if self.route_changed {
            0.0
        } else {
            self.last_width_frame.map_or(0.0, |last| {
                now.saturating_duration_since(last).as_secs_f32()
            })
        };
        self.last_width_frame = Some(now);
        let width = self.width.get_or_insert(Glide::new(target));
        if let Some(progress) = self.pane.progress {
            // Change horizontal geometry only inside the invisible interval.
            if progress >= 0.22 {
                *width = Glide::new(target);
            }
        } else if reduced || (!self.frame.active && !self.moving) {
            *width = Glide::new(target);
        } else {
            width.advance(target, dt, duration(self.frame.docked));
        }
        width.value.max(0.0)
    }

    pub fn tick(&mut self, docked: bool, reduced: bool, now: Instant) -> DockFrame {
        self.route_changed = docked != self.frame.docked;
        if self.route_changed || reduced {
            self.panel_return = !reduced && !docked && self.pane.progress.is_some();
            self.panel_departure = !reduced && docked && self.pane.progress.is_some();
        }
        let target = if docked { 1.0 } else { 0.0 };
        if reduced || self.last_frame.is_none() || self.position.is_none() {
            self.phase = Glide::new(target);
            self.choreography = None;
        } else {
            // A click after an idle window is the START of the new motion,
            // not elapsed animation time. Keep the last painted velocity.
            let dt = if docked != self.frame.docked {
                0.0
            } else {
                now.saturating_duration_since(self.last_frame.unwrap())
                    .as_secs_f32()
            };
            self.phase.advance(target, dt, duration(docked));
            if self.route_changed {
                // Capture the exact previous visual state on interruption.
                self.choreography = Some((now, self.frame.visuals));
            }
        }
        self.last_frame = Some(now);
        let visuals = if let Some((started, from)) = self.choreography {
            let total = if self.panel_return {
                0.320 * crate::motion::speed_scale()
            } else {
                duration(docked)
            };
            let time = now.saturating_duration_since(started).as_secs_f32() / total;
            if time >= 1.0 {
                self.choreography = None;
            }
            let mut visuals = if self.panel_return {
                from.return_from_panel(time)
            } else {
                from.advance(docked, time)
            };
            if self.panel_departure {
                let panel_time = now.saturating_duration_since(started).as_secs_f32()
                    / (0.320 * crate::motion::speed_scale());
                visuals.dissolve =
                    crate::motion::lerp(from.dissolve, 1.0, stage(panel_time, 0.0, 0.18));
            }
            visuals
        } else {
            Visuals::settled(docked)
        };
        if self.panel_return {
            let amount = if self.pane.progress.is_some_and(|p| p < 0.22) {
                self.frame.amount
            } else {
                0.0
            };
            // Keep the retargetable state aligned with what was painted so a
            // reversal cannot revive the old, longer height animation.
            self.phase = Glide::new(amount);
        }
        self.frame = DockFrame {
            amount: self.phase.value.clamp(0.0, 1.0),
            docked,
            active: self.phase.active() || self.choreography.is_some(),
            visuals,
        };
        self.frame
    }
}

fn duration(docked: bool) -> f32 {
    (if docked { 0.420 } else { 0.470 }) * crate::motion::speed_scale()
}

pub(crate) type SharedDock = Rc<RefCell<DockState>>;

/// The child stays in this same layout slot on both routes. Only its prepaint
/// origin changes; input hitboxes, selection and caret travel with its pixels.
pub(crate) struct DockedComposer {
    child: AnyElement,
    state: SharedDock,
    viewport_height: f32,
    reduced: bool,
    now: Instant,
}

pub(crate) fn docked_composer(
    child: impl IntoElement,
    state: SharedDock,
    viewport_height: f32,
    reduced: bool,
    now: Instant,
) -> DockedComposer {
    DockedComposer {
        child: child.into_any_element(),
        state,
        viewport_height,
        reduced,
        now,
    }
}

impl Element for DockedComposer {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut state = self.state.borrow_mut();
        let docked = state.frame.docked;
        let x = f32::from(bounds.left());
        // Anchor by the top of the input surface, not its shrinking bottom.
        let y = if docked {
            f32::from(bounds.top())
        } else {
            (self.viewport_height - f32::from(bounds.size.height)) * 0.5 + 8.0
        };
        let dt = if state.last_docked != docked {
            0.0
        } else {
            state.last_geometry.map_or(0.0, |last| {
                self.now.saturating_duration_since(last).as_secs_f32()
            })
        };
        state.last_geometry = Some(self.now);
        state.moving |= state.last_docked != docked || state.frame.active;
        state.last_docked = docked;
        let moving = state.moving;
        let handoff = state.pane.progress;
        let position = state.position.get_or_insert((Glide::new(x), Glide::new(y)));
        if let Some(progress) = handoff {
            if progress >= 0.22 {
                let travel = if docked { 12.0 } else { 8.0 };
                *position = (
                    Glide::new(x),
                    Glide::new(y + travel * (1.0 - stage(progress, 0.22, 1.0))),
                );
            }
        } else if self.reduced || !moving {
            *position = (Glide::new(x), Glide::new(y));
        } else {
            position.0.advance(x, dt, duration(docked));
            position.1.advance(y, dt, duration(docked));
        }
        let offset = point(
            px(position.0.value - x),
            px(position.1.value - f32::from(bounds.top())),
        );
        let unsettled = (position.0.value - x).abs() > 0.1
            || (position.1.value - y).abs() > 0.1
            || position.0.velocity.abs() > 1.0
            || position.1.velocity.abs() > 1.0;
        state.moving = !self.reduced
            && (unsettled || state.frame.active || state.width.is_some_and(|width| width.active()));
        if state.moving {
            window.request_animation_frame();
        }
        drop(state);
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

impl IntoElement for DockedComposer {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, canvas, div, prelude::*};

    #[test]
    fn panel_handoff_hides_background_during_geometry_switch_in_both_sidebar_states() {
        for sidebar in [0.0, 224.0] {
            for docked in [false, true] {
                let mut state = DockState::default();
                let now = Instant::now();
                let source_pane = if docked { 0.0 } else { 480.0 };
                let target_pane = if docked { 480.0 } else { 0.0 };
                state.observe_pane(!docked, source_pane, true, now);
                state.tick(!docked, false, now);
                state.position = Some((Glide::new(sidebar), Glide::new(360.0)));
                state.observe_pane(docked, target_pane, true, now);
                state.tick(docked, false, now);
                for progress in [0.19, 0.22, 0.25] {
                    let at = now
                        + std::time::Duration::from_secs_f32(
                            progress * 0.320 * crate::motion::speed_scale(),
                        );
                    state.observe_pane(docked, target_pane, true, at);
                    assert_eq!(state.tick(docked, false, at).dissolve(), 1.0);
                }
                let at =
                    now + std::time::Duration::from_secs_f32(0.321 * crate::motion::speed_scale());
                state.observe_pane(docked, target_pane, true, at);
                assert_eq!(
                    state.tick(docked, false, at).dissolve(),
                    if docked { 1.0 } else { 0.0 }
                );
            }
        }
    }

    #[test]
    fn panel_exit_retains_source_transcript_width_only_until_handoff_ends() {
        let mut state = DockState::default();
        assert_eq!(state.transcript_width(540.0, true, false), 540.0);
        state.frame = DockFrame::settled(true);
        assert_eq!(state.transcript_width(1040.0, false, true), 540.0);
        state.frame = DockFrame::settled(false);
        assert_eq!(state.transcript_width(1040.0, false, true), 540.0);
        assert_eq!(state.transcript_width(1040.0, false, false), 1040.0);
        assert_eq!(state.transcript_width(540.0, true, true), 540.0);
        state.frame = DockFrame::settled(true);
        assert_eq!(state.transcript_width(1040.0, false, false), 1040.0);
    }

    #[test]
    fn panel_return_sizes_while_hidden_and_finishes_controls_with_input() {
        let now = Instant::now();
        let mut state = DockState::default();
        state.observe_pane(true, 480.0, true, now);
        state.tick(true, false, now);
        state.position = Some((Glide::new(100.0), Glide::new(700.0)));
        state.observe_pane(false, 0.0, true, now);
        assert_eq!(state.tick(false, false, now).amount, 1.0);
        let hidden = now + std::time::Duration::from_secs_f32(0.075 * crate::motion::speed_scale());
        state.observe_pane(false, 0.0, true, hidden);
        let frame = state.tick(false, false, hidden);
        assert_eq!(state.opacity(), 0.0);
        assert_eq!(frame.amount, 0.0);
        for seconds in [0.321, 0.400, 0.500] {
            let at =
                now + std::time::Duration::from_secs_f32(seconds * crate::motion::speed_scale());
            state.observe_pane(false, 0.0, true, at);
            let frame = state.tick(false, false, at);
            assert_eq!(frame.selectors(), 1.0);
            assert_eq!(frame.dissolve(), 0.0);
            assert_eq!(frame.amount, 0.0);
        }
    }

    #[gpui::test]
    fn measured_dock_retargets_without_a_first_frame_jump(cx: &mut gpui::TestAppContext) {
        struct Fixture {
            state: SharedDock,
            now: Instant,
            docked: bool,
            width: f32,
            measured: Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
        }
        impl Render for Fixture {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.state.borrow_mut().tick(self.docked, false, self.now);
                let width = self
                    .state
                    .borrow_mut()
                    .layout_width(self.width, false, self.now);
                let measured = self.measured.clone();
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(div().flex_1())
                    .child(docked_composer(
                        div().relative().w(px(width)).h(px(124.0)).mx_auto().child(
                            canvas(
                                move |bounds, _, _| measured.set(Some(bounds)),
                                |_, _, _, _| {},
                            )
                            .absolute()
                            .inset_0(),
                        ),
                        self.state.clone(),
                        f32::from(window.viewport_size().height),
                        false,
                        self.now,
                    ))
            }
        }
        let measured = Rc::new(std::cell::Cell::new(None));
        let now = Instant::now();
        let handle = cx.add_window(|_, _| Fixture {
            state: Default::default(),
            now,
            docked: false,
            width: 400.0,
            measured: measured.clone(),
        });
        let draw = |cx: &mut gpui::TestAppContext| {
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
            })
            .unwrap();
            measured.get().unwrap()
        };
        let origin = draw(cx);
        handle
            .update(cx, |fixture, _, cx| {
                fixture.now = now + std::time::Duration::from_secs(30);
                fixture.docked = true;
                cx.notify();
            })
            .unwrap();
        assert_eq!(draw(cx), origin);
        handle
            .update(cx, |fixture, _, cx| {
                fixture.now += std::time::Duration::from_millis(100);
                cx.notify();
            })
            .unwrap();
        let moving = draw(cx);
        assert!(moving.top() > origin.top());
        handle
            .update(cx, |fixture, _, cx| {
                fixture.docked = false;
                fixture.width = 300.0;
                cx.notify();
            })
            .unwrap();
        assert_eq!(
            draw(cx),
            moving,
            "reversal and resize must start at the painted bounds"
        );
        for _ in 0..90 {
            handle
                .update(cx, |fixture, _, cx| {
                    fixture.now += std::time::Duration::from_millis(16);
                    cx.notify();
                })
                .unwrap();
            draw(cx);
        }
        let settled = draw(cx);
        assert!((f32::from(settled.top() - origin.top())).abs() < 0.1);
        assert!((f32::from(settled.size.width) - 300.0).abs() < 0.1);
    }

    #[test]
    fn idle_time_is_not_consumed_by_a_new_target() {
        let mut state = DockState::default();
        let now = Instant::now();
        state.tick(false, false, now);
        state.position = Some((Glide::new(0.0), Glide::new(300.0)));
        let click = now + std::time::Duration::from_secs(30);
        assert_eq!(state.tick(true, false, click).amount, 0.0);
        let moving = state.tick(true, false, click + std::time::Duration::from_millis(100));
        assert!(moving.amount > 0.0 && moving.amount < 1.0);
        let reverse = state.tick(false, false, click + std::time::Duration::from_millis(100));
        assert_eq!(moving.amount, reverse.amount);
        assert_eq!(moving.visuals, reverse.visuals);
    }

    #[test]
    fn choreography_is_direction_specific_and_selectors_never_duplicate() {
        let new = Visuals::settled(false);
        let thread = Visuals::settled(true);
        assert_eq!(new.advance(true, 0.19).transcript, 0.0);
        assert_eq!(new.advance(true, 0.65).transcript, 1.0);
        assert_eq!(new.advance(true, 0.55).selectors, 1.0);
        assert_eq!(thread.advance(false, 0.25).transcript, 0.0);
        assert_eq!(thread.advance(false, 0.49).selectors, 0.0);
        for step in 0..=100 {
            let time = step as f32 / 100.0;
            for values in [new.advance(true, time), thread.advance(false, time)] {
                assert!(values.selectors == 0.0 || values.footer == 0.0);
            }
        }
    }
    #[test]
    fn reversal_preserves_position_and_velocity() {
        let mut glide = Glide::new(300.0);
        glide.advance(800.0, 0.12, 0.42);
        let before = glide;
        glide.advance(300.0, 0.0, 0.47);
        assert!((glide.value - before.value).abs() < 0.001);
        assert!((glide.velocity - before.velocity).abs() < 0.001);
        for _ in 0..60 {
            glide.advance(300.0, 1.0 / 120.0, 0.47);
        }
        assert!((glide.value - 300.0).abs() < 0.5);
    }
    #[test]
    fn normal_dock_is_monotone_and_frame_rate_independent() {
        for hz in [30, 60, 120] {
            let mut glide = Glide::new(300.0);
            for _ in 0..(hz / 2) {
                let old = glide.value;
                glide.advance(800.0, 1.0 / hz as f32, 0.42);
                assert!(glide.value >= old && glide.value <= 800.0);
            }
            assert!((glide.value - 800.0).abs() < 0.1);
        }
    }
    #[test]
    fn initial_and_reduced_motion_frames_snap() {
        let mut state = DockState::default();
        let now = Instant::now();
        assert_eq!(state.tick(true, false, now).amount, 1.0);
        assert_eq!(state.tick(false, true, now).amount, 0.0);
        assert!(!state.frame.active);
    }
}
