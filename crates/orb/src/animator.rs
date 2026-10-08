//! Renderer-agnostic orb playback: the continuous clock, audio-driven speed
//! and state crossfades. Hosts decide when to tick and paint the frames.
use std::time::{Duration, Instant};

use crate::{
    REDUCED_MOTION_T,
    engine::{Frame, draw_mode_into_resolved},
    motion::{AnimationClock, AudioResponse},
    presets::{Resolved, resolve_preset},
    types::{OrbSize, OrbState},
};

struct StateTransition {
    from: Frame,
    source: Option<Resolved>,
    started: f64,
}

/// Blend two frames by opacity with a smoothstep weight.
pub fn crossfade(from: &Frame, to: &Frame, progress: f32, out: &mut Frame) {
    out.clear();
    let progress = progress.clamp(0.0, 1.0);
    let weight = progress * progress * (3.0 - 2.0 * progress);
    for (frame, opacity) in [(from, 1.0 - weight), (to, weight)] {
        out.dots.extend(frame.dots.iter().filter_map(|dot| {
            let mut dot = *dot;
            dot.a *= opacity;
            (dot.a >= 0.005).then_some(dot)
        }));
        out.lines.extend(frame.lines.iter().filter_map(|line| {
            let mut line = *line;
            line.a *= opacity;
            (line.a >= 0.005).then_some(line)
        }));
    }
}

/// One orb's animation state. The engine takes continuous, unbounded time —
/// its modes mix incommensurate frequencies, so there is no seamless wrap
/// point — and this owns that clock.
pub struct OrbAnimator {
    state: OrbState,
    size: OrbSize,
    /// Multiplier on top of the preset's baked speed.
    speed: f32,
    clock: AnimationClock,
    audio: Option<AudioResponse>,
    transition_duration: Duration,
    transition: Option<StateTransition>,
    resolved: Resolved,
    target_frame: Frame,
    t: f32,
}

impl OrbAnimator {
    pub fn new(state: OrbState, size: OrbSize) -> Self {
        Self {
            state,
            size,
            speed: 1.0,
            clock: AnimationClock::default(),
            audio: None,
            transition_duration: Duration::ZERO,
            transition: None,
            resolved: resolve_preset(state, size),
            target_frame: Frame::new(),
            t: 0.0,
        }
    }

    /// Crossfade state geometry without restarting the clock.
    pub fn with_transition(mut self, duration: Duration) -> Self {
        self.set_transition(duration);
        self
    }

    pub fn set_transition(&mut self, duration: Duration) {
        self.transition_duration = duration;
    }

    pub fn state(&self) -> OrbState {
        self.state
    }

    pub fn size(&self) -> OrbSize {
        self.size
    }

    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// Logical edge length of the preset's artwork.
    pub fn pixels(&self) -> f32 {
        self.size.pixels()
    }

    /// Smallest painted dot radius for the current preset.
    pub fn r_min(&self) -> f32 {
        self.resolved.opts.r_min.unwrap_or(0.3)
    }

    pub fn is_running(&self) -> bool {
        self.clock.is_running()
    }

    pub fn is_transitioning(&self) -> bool {
        self.transition.is_some()
    }

    /// Change state. `visible` is the frame on screen now: an interrupted
    /// fade continues from its visible composite instead of jumping back.
    pub fn set_state(&mut self, state: OrbState, visible: &Frame) -> bool {
        if self.state == state {
            return false;
        }
        self.transition = (!self.transition_duration.is_zero()
            && self.clock.is_running()
            && (!visible.dots.is_empty() || !visible.lines.is_empty()))
        .then(|| StateTransition {
            from: visible.clone(),
            // Animate the outgoing form, unless it is itself mid-fade.
            source: self.transition.is_none().then(|| self.resolved.clone()),
            started: self.clock.active_seconds,
        });
        self.state = state;
        self.resolved = resolve_preset(state, self.size);
        true
    }

    pub fn set_size(&mut self, size: OrbSize) -> bool {
        if self.size == size {
            return false;
        }
        self.size = size;
        self.resolved = resolve_preset(self.state, size);
        true
    }

    pub fn set_speed(&mut self, speed: f32) -> bool {
        let speed = if speed.is_finite() {
            speed.clamp(0.0, 100.0)
        } else {
            1.0
        };
        let changed = self.speed != speed;
        self.speed = speed;
        changed
    }

    /// Audio affects only visual motion, with independent microphone and
    /// speaker envelopes.
    pub fn set_audio_levels(&mut self, microphone: f32, speaker: f32, now: Instant) -> bool {
        self.audio
            .get_or_insert_with(|| AudioResponse::new(now))
            .set(microphone, speaker, now)
    }

    /// Freeze accumulated time (paused, hidden or reduced motion).
    pub fn stop(&mut self, now: Instant) {
        self.clock.stop(now);
    }

    pub fn cancel_transition(&mut self) {
        self.transition = None;
    }

    /// Advance the clock to `now`. Time only accumulates while `animating`.
    pub fn tick(&mut self, now: Instant, animating: bool, reduced: bool) {
        let audio_speed = self.audio.as_ref().map_or(1.0, |audio| audio.speed(now));
        let rate = f64::from(self.resolved.speed) * f64::from(self.speed) * audio_speed;
        self.clock.advance(now, rate, animating);
        self.t = if reduced {
            REDUCED_MOTION_T
        } else {
            self.clock.seconds as f32
        };
        if reduced {
            self.transition = None;
        }
    }

    /// Geometry at the last tick, in the preset's logical pixels.
    pub fn draw(&mut self, out: &mut Frame) {
        let progress = self.transition.as_ref().map_or(1.0, |transition| {
            ((self.clock.active_seconds - transition.started)
                / self.transition_duration.as_secs_f64()) as f32
        });
        if progress >= 1.0 {
            self.transition = None;
        }
        let size = self.size.pixels();
        let Some(transition) = &mut self.transition else {
            // The allocation-free path outside a fade.
            draw_mode_into_resolved(self.resolved.mode, size, self.t, &self.resolved.opts, out);
            return;
        };
        draw_mode_into_resolved(
            self.resolved.mode,
            size,
            self.t,
            &self.resolved.opts,
            &mut self.target_frame,
        );
        if let Some(source) = &transition.source {
            draw_mode_into_resolved(
                source.mode,
                size,
                self.t,
                &source.opts,
                &mut transition.from,
            );
        }
        crossfade(&transition.from, &self.target_frame, progress, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Dot, Line};

    #[test]
    fn state_crossfade_preserves_endpoints_and_blends_dots_and_lines() {
        let from = Frame {
            dots: vec![Dot::new(1.0, 2.0, 0.0, 1.0, 0.5)],
            lines: vec![Line {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 1.0,
                white: 0.5,
                a: 1.0,
                w: 1.0,
            }],
        };
        let to = Frame {
            dots: vec![Dot::new(3.0, 4.0, 0.0, 1.0, 0.5)],
            lines: vec![],
        };
        let mut out = Frame::new();
        crossfade(&from, &to, 0.0, &mut out);
        assert_eq!(out.dots.len(), 1);
        assert_eq!(out.dots[0].x, 1.0);
        assert_eq!(out.lines[0].a, 1.0);
        crossfade(&from, &to, 0.5, &mut out);
        assert_eq!(out.dots.len(), 2);
        assert_eq!(out.dots[0].a, 0.5);
        assert_eq!(out.dots[1].a, 0.5);
        assert_eq!(out.lines[0].a, 0.5);
        crossfade(&from, &to, 1.0, &mut out);
        assert_eq!(out.dots.len(), 1);
        assert_eq!(out.dots[0].x, 3.0);
        assert!(out.lines.is_empty());
    }

    #[test]
    fn interrupted_state_transition_starts_from_the_visible_frame() {
        let now = Instant::now();
        let mut orb = OrbAnimator::new(OrbState::Working, OrbSize::Avatar)
            .with_transition(Duration::from_millis(300));
        orb.tick(now, true, false);
        let mut visible = Frame::new();
        visible
            .dots
            .push(Dot::new(42.0, 0.0, 0.0, 1.0, 0.5).with_a(0.4));
        assert!(orb.set_state(OrbState::Listening, &visible));
        assert_eq!(orb.transition.as_ref().unwrap().from.dots[0].x, 42.0);
        assert!(orb.transition.as_ref().unwrap().source.is_some());
        visible.dots[0].a = 0.2;
        orb.set_state(OrbState::Composing, &visible);
        let transition = orb.transition.as_ref().unwrap();
        assert_eq!(transition.from.dots[0].a, 0.2);
        // A fade from a fade keeps the composite rather than redrawing it.
        assert!(transition.source.is_none());
        orb.tick(now + Duration::from_millis(16), false, true);
        assert!(orb.transition.is_none());
        assert!(!orb.is_running());
    }

    #[test]
    fn stopped_orbs_switch_state_without_a_fade_and_finish_fades() {
        let now = Instant::now();
        let mut orb = OrbAnimator::new(OrbState::Working, OrbSize::Hero)
            .with_transition(Duration::from_millis(300));
        let mut frame = Frame::new();
        orb.tick(now, false, false);
        orb.draw(&mut frame);
        orb.set_state(OrbState::Listening, &frame);
        assert!(!orb.is_transitioning());
        orb.tick(now, true, false);
        orb.draw(&mut frame);
        orb.set_state(OrbState::Composing, &frame);
        assert!(orb.is_transitioning());
        orb.tick(now + Duration::from_millis(400), true, false);
        orb.draw(&mut frame);
        assert!(!orb.is_transitioning());
        assert!(!frame.dots.is_empty());
    }
}
