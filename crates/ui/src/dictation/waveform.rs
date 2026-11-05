//! Painted dictation waveform: one rounded bar per meter slot, scrolling from
//! the trailing edge. A single canvas keeps a full-width waveform to one
//! element and a handful of quads per frame.
use std::time::Instant;

use gpui::{Bounds, Hsla, IntoElement, Styled, point, px, size};

use super::{Bar, FLOOR};

pub(crate) const BAR_WIDTH: f32 = 3.0;
const BAR_GAP: f32 = 3.0;
const PITCH: f32 = BAR_WIDTH + BAR_GAP;
/// Bars dissolve over this distance at the leading edge instead of clipping.
const EDGE_FADE: f32 = 28.0;
/// Seconds for the transcribing highlight to cross the bars once.
const SWEEP_PERIOD: f32 = 1.6;
/// Seconds per breath of the placeholder baseline while capture starts.
const BREATH_PERIOD: f32 = 1.4;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    /// Microphone is opening: a quiet baseline breathes.
    Waiting,
    /// Live input: bars follow the voice.
    Live,
    /// Capture ended: bars hold still while a highlight sweeps across them.
    Processing,
}

pub(crate) struct Paint {
    pub ink: Hsla,
    pub quiet: Hsla,
}

/// A process-wide clock so repeated renders stay phase-continuous.
pub(crate) fn seconds() -> f32 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_secs_f32()
}

pub(crate) fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Entrance and exit progress, each 0–1.
#[derive(Clone, Copy, Default)]
pub(crate) struct Phase {
    /// Baseline draws in from the trailing edge as the pill appears.
    pub intro: f32,
    /// Bars settle back into the baseline as the transcript lands.
    pub collapse: f32,
}

/// Bars ride a fixed grid of dots, so live input rises out of the same
/// baseline the waiting state shows. `motion` false freezes the waiting
/// breath and the processing sweep; the indicator still names the state.
pub(crate) fn waveform(
    bars: Vec<Bar>,
    mode: Mode,
    paint: Paint,
    phase: Phase,
    motion: bool,
) -> impl IntoElement {
    let now = if motion { seconds() } else { 0.0 };
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let height = f32::from(bounds.size.height);
            let left = f32::from(bounds.left());
            let right = f32::from(bounds.right()) - BAR_WIDTH;
            let mid = f32::from(bounds.center().y);
            let width = (right - left).max(1.0);
            // Newest slot's scroll position; the dots scroll with the bars.
            let offset = match mode {
                Mode::Waiting => 0.0,
                _ => bars.first().map_or(0.0, |bar| bar.age.fract()),
            };
            let count = ((width + BAR_WIDTH) / PITCH).ceil() as usize + 1;
            let sweep = (now / SWEEP_PERIOD).fract();
            let settle = 1.0 - ease_out(phase.collapse);
            for k in 0..count {
                let x = right - (k as f32 + offset) * PITCH;
                let fade = ((x - left) / EDGE_FADE).clamp(0.0, 1.0);
                // Draw in from the trailing edge: 0 there, 1 at the leading edge.
                let distance = 1.0 - (x - left) / width;
                let reveal = ease_out((phase.intro - 0.5 * distance) / 0.5);
                let alpha = fade * reveal;
                if alpha <= 0.0 {
                    continue;
                }
                let (amplitude, color) = match (mode, bars.get(k)) {
                    (Mode::Waiting, _) | (_, None) => {
                        // A soft swell travels toward the leading edge.
                        let swell = if mode == Mode::Waiting {
                            let wave = now / BREATH_PERIOD - k as f32 / 18.0;
                            (0.5 + 0.5 * (wave * std::f32::consts::TAU).sin()).powi(3)
                        } else {
                            0.0
                        };
                        (FLOOR + 0.1 * swell, paint.quiet)
                    }
                    (_, Some(bar)) => {
                        // The newest bar rises out of the baseline as it enters.
                        let grow = if motion { ease_out(bar.age) } else { 1.0 };
                        let color = if mode == Mode::Live {
                            paint.ink
                        } else {
                            // Right-to-left highlight, matching the scroll.
                            let position = 1.0 - (x - left) / width;
                            let distance = (position - (sweep * 1.4 - 0.2)).abs();
                            let glow = (1.0 - distance / 0.18).clamp(0.0, 1.0);
                            crate::motion::mix(paint.quiet, paint.ink, ease_out(glow))
                        };
                        let lift = (1.0 - FLOOR) * bar.amplitude * grow * settle;
                        (FLOOR + lift, crate::motion::mix(paint.quiet, color, settle))
                    }
                };
                let h =
                    (amplitude.clamp(FLOOR, 1.0) * height * (0.4 + 0.6 * reveal)).max(BAR_WIDTH);
                window.paint_quad(
                    gpui::fill(
                        Bounds::new(point(px(x), px(mid - h / 2.0)), size(px(BAR_WIDTH), px(h))),
                        color.opacity(alpha),
                    )
                    .corner_radii(px(BAR_WIDTH / 2.0)),
                );
            }
        },
    )
    .flex_1()
    .min_w_0()
    .h_full()
}

/// Display time for the recording clock: `0:07`.
pub(crate) fn clock(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats_minutes_and_padded_seconds() {
        assert_eq!(clock(std::time::Duration::from_millis(7_900)), "0:07");
        assert_eq!(clock(std::time::Duration::from_secs(60)), "1:00");
    }
}
