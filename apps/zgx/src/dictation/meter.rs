//! Live input level for the voice bar's waveform (the old app's
//! crates/ui/src/dictation/meter.rs): only a loudness envelope, one
//! normalised amplitude per fixed time slot, never audio.
//!
//! Slots sit on a fixed clock from the moment capture goes live, so bars stay
//! evenly spaced however irregularly the host polls. The composer draws the
//! newest [`BARS`] of them (`dictation_levels`), one per slot.
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// One bar per slot.
pub(crate) const STEP: Duration = Duration::from_millis(60);
/// What's kept and published, as many as ds/voice_wave draws (3 pt bars,
/// 3 pt gaps); older bars have scrolled away.
pub(crate) const BARS: usize = 128;
/// A bar never drops below its neighbour faster than this, so syllables
/// leave a short tail instead of flickering between loud and silent.
const RELEASE: f32 = 0.62;

#[derive(Debug, Default, Clone)]
pub(crate) struct Meter {
    started: Option<Instant>,
    frozen: Option<Instant>,
    slots: VecDeque<f32>,
    filled: u64,
    pending: f32,
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
    /// The microphone is live: slots start from `now`.
    pub fn start(&mut self, now: Instant) {
        *self = Self {
            started: Some(now),
            ..Self::default()
        };
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Capture ended: the bars hold still.
    pub fn freeze(&mut self, now: Instant) {
        if self.started.is_some() && self.frozen.is_none() {
            self.frozen = Some(now);
        }
    }

    fn running(&self) -> Option<Instant> {
        self.started.filter(|_| self.frozen.is_none())
    }

    /// Folds in the loudest RMS since the previous call. True when a new
    /// slot was filled (what's drawn changed).
    pub fn record(&mut self, now: Instant, rms: f32) -> bool {
        let Some(started) = self.running() else {
            return false;
        };
        self.pending = self.pending.max(normalize(rms));
        let due = (now.saturating_duration_since(started).as_nanos() / STEP.as_nanos()) as u64 + 1;
        if self.filled >= due {
            return false;
        }
        while self.filled < due {
            let previous = self.slots.back().copied().unwrap_or(0.0);
            self.slots.push_back(self.pending.max(previous * RELEASE));
            if self.slots.len() > BARS {
                self.slots.pop_front();
            }
            self.filled += 1;
        }
        self.pending = 0.0;
        true
    }

    /// How long until the next slot is due (none: not running).
    pub fn until_next(&self, now: Instant) -> Option<Duration> {
        let started = self.running()?;
        let next = started + STEP * self.filled as u32;
        Some(next.saturating_duration_since(now))
    }

    /// The bars, newest first.
    pub fn newest(&self) -> Vec<f32> {
        self.slots.iter().rev().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn meter_normalizes_speech_range_and_rejects_invalid_levels() {
        assert_eq!(normalize(0.0), 0.0);
        assert_eq!(normalize(f32::NAN), 0.0);
        assert_eq!(normalize(-1.0), 0.0);
        assert_eq!(normalize(10f32.powf(-50.0 / 20.0)), 0.0);
        assert!((normalize(10f32.powf(-12.0 / 20.0)) - 1.0).abs() < 1e-5);
        assert_eq!(normalize(1.0), 1.0);
        let mid = normalize(0.02);
        assert!(mid > 0.2 && mid < 0.8, "{mid}");
    }

    #[test]
    fn meter_slots_stay_on_the_clock_when_polls_are_irregular() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        assert!(meter.record(t0 + ms(5), 0.1));
        assert!(!meter.record(t0 + ms(30), 0.0));
        // A late poll fills every slot it skipped.
        assert!(meter.record(t0 + STEP * 3 + ms(1), 0.0));
        let bars = meter.newest();
        assert_eq!(bars.len(), 4);
        // The loud slot decays into its neighbours instead of dropping to zero.
        let loud = normalize(0.1);
        assert_eq!(bars[3], loud);
        assert!((bars[2] - loud * RELEASE).abs() < 1e-6);
        assert!((bars[0] - loud * RELEASE.powi(3)).abs() < 1e-6);
    }

    #[test]
    fn meter_keeps_a_peak_between_slots_for_the_next_slot() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0, 0.0);
        assert!(!meter.record(t0 + ms(20), 0.2));
        assert!(meter.record(t0 + STEP, 0.0));
        assert_eq!(meter.newest()[0], normalize(0.2));
    }

    #[test]
    fn meter_says_when_the_next_slot_is_due() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        assert_eq!(meter.until_next(t0), None);
        meter.start(t0);
        assert_eq!(meter.until_next(t0), Some(Duration::ZERO));
        meter.record(t0 + ms(10), 0.0);
        assert_eq!(meter.until_next(t0 + ms(10)), Some(ms(50)));
        meter.freeze(t0 + ms(20));
        assert_eq!(meter.until_next(t0 + ms(20)), None);
    }

    #[test]
    fn meter_freezing_holds_the_bars() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0 + STEP * 5, 0.05);
        let held = meter.newest();
        meter.freeze(t0 + STEP * 5);
        assert!(!meter.record(t0 + STEP * 9, 0.5));
        assert_eq!(meter.newest(), held);
        meter.reset();
        assert!(meter.newest().is_empty());
        assert!(!meter.record(t0 + STEP * 10, 0.5));
    }

    #[test]
    fn meter_history_is_bounded() {
        let t0 = Instant::now();
        let mut meter = Meter::default();
        meter.start(t0);
        meter.record(t0 + STEP * (BARS as u32 * 3), 0.05);
        assert_eq!(meter.newest().len(), BARS);
    }
}
