//! Live input level for the dictation waveform. Only a loudness envelope is
//! kept here: one normalized amplitude per fixed time slot, never audio.
//!
//! Slots sit on a fixed clock from the moment capture starts, so bars stay
//! evenly spaced however irregularly the UI polls. Drawing trails the clock by
//! [`LAG`], longer than one poll, which means a slot is always filled before
//! it scrolls into view and the waveform can glide continuously.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One bar per slot.
pub(crate) const STEP: Duration = Duration::from_millis(60);
const LAG: Duration = Duration::from_millis(70);
/// Enough slots for the widest composer; older bars have scrolled away.
const CAPACITY: usize = 320;
/// A bar never drops below its neighbour faster than this, so syllables leave
/// a short tail instead of flickering between loud and silent.
const RELEASE: f32 = 0.62;
/// The quietest bar still reads as a dot on the baseline.
pub(crate) const FLOOR: f32 = 0.08;

#[derive(Debug, Default, Clone)]
pub(crate) struct Meter {
    started: Option<Instant>,
    frozen: Option<Instant>,
    slots: VecDeque<f32>,
    filled: u64,
    pending: f32,
}

/// One visible bar: `age` in slots behind the trailing edge (0 = newest) and
/// its normalized height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bar {
    pub age: f32,
    pub amplitude: f32,
}

/// Maps microphone RMS onto a perceptual 0–1 scale: −50 dBFS is silence and
/// −12 dBFS, a close, raised voice, fills the bar.
pub(crate) fn normalize(rms: f32) -> f32 {
    if !rms.is_finite() || rms <= 1e-6 {
        return 0.0;
    }
    ((20.0 * rms.log10() + 50.0) / 38.0).clamp(0.0, 1.0)
}

impl Meter {
    pub fn start(&mut self, now: Instant) {
        *self = Self {
            started: Some(now),
            ..Self::default()
        };
    }

    /// Time since capture began; `None` until the microphone is live.
    pub fn since_start(&self, now: Instant) -> Option<Duration> {
        self.started.map(|at| now.saturating_duration_since(at))
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Stops the clock where capture ended; bars and timer hold still.
    pub fn freeze(&mut self, now: Instant) {
        if self.started.is_some() && self.frozen.is_none() {
            self.frozen = Some(now);
        }
    }

    /// Folds in the loudest RMS since the previous call. Returns true when a
    /// new slot was filled.
    pub fn record(&mut self, now: Instant, rms: f32) -> bool {
        let Some(started) = self.started.filter(|_| self.frozen.is_none()) else {
            return false;
        };
        self.pending = self.pending.max(normalize(rms));
        let due = now.saturating_duration_since(started).as_nanos() / STEP.as_nanos() + 1;
        let due = due as u64;
        if self.filled >= due {
            return false;
        }
        while self.filled < due {
            let previous = self.slots.back().copied().unwrap_or(0.0);
            self.slots.push_back(self.pending.max(previous * RELEASE));
            if self.slots.len() > CAPACITY {
                self.slots.pop_front();
            }
            self.filled += 1;
        }
        self.pending = 0.0;
        true
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        self.started.map_or(Duration::ZERO, |started| {
            self.frozen
                .unwrap_or(now)
                .saturating_duration_since(started)
        })
    }

    /// Bars visible at `now`, newest first. `continuous` false snaps the
    /// scroll to whole slots for reduced motion.
    pub fn bars(&self, now: Instant, continuous: bool) -> Vec<Bar> {
        let Some(started) = self.started else {
            return Vec::new();
        };
        let clock = self
            .frozen
            .map_or(now, |frozen| frozen.min(now))
            .saturating_duration_since(started);
        let Some(clock) = clock.checked_sub(LAG) else {
            return Vec::new();
        };
        let head = clock.as_secs_f32() / STEP.as_secs_f32();
        let first = self.filled - self.slots.len() as u64;
        self.slots
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(i, amplitude)| {
                let age = head - (first + i as u64) as f32;
                (age >= 0.0).then(|| Bar {
                    age: if continuous { age } else { age.floor() },
                    amplitude: *amplitude,
                })
            })
            .collect()
    }

    /// Current loudness for the glow, blended between the two newest visible
    /// slots as the newest one scrolls in, so it changes without steps.
    pub fn level(&self, now: Instant) -> f32 {
        let bars = self.bars(now, true);
        match bars.as_slice() {
            [] => 0.0,
            [only] => only.amplitude * only.age.clamp(0.0, 1.0),
            [newest, previous, ..] => {
                let t = newest.age.clamp(0.0, 1.0);
                previous.amplitude + (newest.amplitude - previous.amplitude) * t
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn normalizes_speech_range_and_rejects_invalid_levels() {
        assert_eq!(normalize(0.0), 0.0);
        assert_eq!(normalize(f32::NAN), 0.0);
        assert_eq!(normalize(10f32.powf(-50.0 / 20.0)), 0.0);
        assert_eq!(normalize(1.0), 1.0);
        let mid = normalize(0.02);
        assert!(mid > 0.2 && mid < 0.8, "{mid}");
    }

    #[test]
    fn slots_stay_on_the_clock_when_polls_are_irregular() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        assert!(meter.record(t0 + ms(5), 0.1));
        assert!(!meter.record(t0 + ms(30), 0.0));
        // A late poll fills every slot it skipped.
        assert!(meter.record(t0 + STEP * 3 + ms(1), 0.0));
        assert_eq!(meter.filled, 4);
        let bars = meter.bars(t0 + STEP * 3 + LAG + ms(1), true);
        assert_eq!(bars.len(), 4);
        for pair in bars.windows(2) {
            assert!((pair[1].age - pair[0].age - 1.0).abs() < 1e-3);
        }
        // Loud slot decays into its neighbours instead of dropping to zero.
        let loud = normalize(0.1);
        assert_eq!(bars[3].amplitude, loud);
        assert!((bars[2].amplitude - loud * RELEASE).abs() < 1e-6);
    }

    #[test]
    fn peak_between_slots_is_kept_for_the_next_slot() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0, 0.0);
        assert!(!meter.record(t0 + ms(20), 0.2));
        assert!(meter.record(t0 + STEP, 0.0));
        assert_eq!(meter.slots.back().copied(), Some(normalize(0.2)));
    }

    #[test]
    fn freezing_stops_scroll_and_timer() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0 + STEP * 5, 0.05);
        meter.freeze(t0 + STEP * 5);
        assert!(!meter.record(t0 + STEP * 9, 0.5));
        let later = t0 + Duration::from_secs(10);
        assert_eq!(meter.elapsed(later), STEP * 5);
        assert_eq!(meter.bars(later, true), meter.bars(later + ms(500), true));
    }

    #[test]
    fn level_glides_between_slots() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0, 0.0);
        meter.record(t0 + STEP, 0.1);
        let at = |offset: Duration| meter.level(t0 + STEP + LAG + ms(1) + offset);
        let loud = normalize(0.1);
        assert!(at(Duration::ZERO) < 0.03);
        let half = at(STEP / 2);
        assert!((half - loud / 2.0).abs() < 0.03, "{half}");
        assert!((at(STEP) - loud).abs() < 1e-6);
    }

    #[test]
    fn reduced_motion_snaps_to_whole_slots() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0 + STEP * 2, 0.05);
        let bars = meter.bars(t0 + STEP * 2 + LAG + ms(20), false);
        assert!(bars.iter().all(|bar| bar.age.fract() == 0.0));
    }

    #[test]
    fn capacity_bounds_history() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0 + STEP * (CAPACITY as u32 * 2), 0.05);
        assert_eq!(meter.slots.len(), CAPACITY);
        let bars = meter.bars(t0 + STEP * (CAPACITY as u32 * 2) + LAG + ms(1), true);
        assert_eq!(bars.len(), CAPACITY);
        assert!(bars[0].age < 1.0);
    }
}
