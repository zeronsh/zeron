//! Continuous animation time and bounded, independent audio envelopes.
use std::time::Instant;

#[derive(Default)]
pub struct AnimationClock {
    pub seconds: f64,
    pub active_seconds: f64,
    last: Option<Instant>,
    rate: f64,
    running: bool,
}

impl AnimationClock {
    pub fn advance(&mut self, now: Instant, rate: f64, running: bool) {
        if let Some(last) = self.last {
            if self.running {
                let dt = now.saturating_duration_since(last).as_secs_f64();
                self.seconds += dt * self.rate;
                self.active_seconds += dt;
            }
        }
        self.last = Some(now);
        self.rate = rate;
        self.running = running;
    }

    pub fn stop(&mut self, now: Instant) {
        self.advance(now, self.rate, false);
    }

    pub fn is_running(&self) -> bool {
        self.running
    }
}

struct Envelope {
    origin: f32,
    target: f32,
    changed: Instant,
}

impl Envelope {
    fn new(now: Instant) -> Self {
        Self {
            origin: 0.0,
            target: 0.0,
            changed: now,
        }
    }

    fn value(&self, now: Instant) -> f32 {
        let tau = if self.target > self.origin {
            0.080
        } else {
            0.300
        };
        let dt = now.saturating_duration_since(self.changed).as_secs_f32();
        self.target + (self.origin - self.target) * (-dt / tau).exp()
    }

    fn set(&mut self, peak: f32, now: Instant) -> bool {
        // Ignore quiet background noise; compress peaks without exceeding 1.
        let peak = if peak.is_finite() {
            peak.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let x = ((peak - 0.015) / 0.985).max(0.0);
        let target = 1.2 * x / (x + 0.2);
        if self.target == target {
            return false;
        }
        self.origin = self.value(now);
        self.target = target;
        self.changed = now;
        true
    }
}

pub struct AudioResponse {
    microphone: Envelope,
    speaker: Envelope,
}

impl AudioResponse {
    pub fn new(now: Instant) -> Self {
        Self {
            microphone: Envelope::new(now),
            speaker: Envelope::new(now),
        }
    }

    pub fn set(&mut self, microphone: f32, speaker: f32, now: Instant) -> bool {
        let mic_changed = self.microphone.set(microphone, now);
        let speaker_changed = self.speaker.set(speaker, now);
        mic_changed || speaker_changed
    }

    pub fn speed(&self, now: Instant) -> f64 {
        let mic = self.microphone.value(now);
        let speaker = self.speaker.value(now);
        // A bounded, continuous combination, even when both sides overlap.
        1.0 + f64::from((1.0 - (1.0 - mic) * (1.0 - speaker)).clamp(0.0, 1.0)) * 0.3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn changing_speed_after_long_uptime_preserves_position() {
        let now = Instant::now();
        let mut clock = AnimationClock::default();
        clock.advance(now, 1.0, true);
        let later = now + Duration::from_secs(3600);
        clock.advance(later, 2.0, true);
        assert_eq!(clock.seconds, 3600.0);
        clock.advance(later + Duration::from_millis(100), 2.0, true);
        assert!((clock.seconds - 3600.2).abs() < 1e-8);
    }

    #[test]
    fn hidden_and_paused_time_does_not_advance_animation() {
        let now = Instant::now();
        let mut clock = AnimationClock::default();
        clock.advance(now, 2.0, true);
        clock.stop(now + Duration::from_secs(1));
        clock.advance(now + Duration::from_secs(60), 3.0, true);
        assert_eq!(clock.seconds, 2.0);
        clock.advance(now + Duration::from_secs(61), 3.0, true);
        assert_eq!(clock.seconds, 5.0);
        assert_eq!(clock.active_seconds, 2.0);
    }

    #[test]
    fn audio_ignores_noise_and_smooths_attack_release_and_channel_overlap() {
        let now = Instant::now();
        let mut audio = AudioResponse::new(now);
        audio.set(0.01, f32::NAN, now);
        assert_eq!(audio.speed(now + Duration::from_secs(1)), 1.0);
        audio.set(1.0, 0.0, now);
        assert_eq!(audio.speed(now), 1.0);
        let attack = now + Duration::from_millis(80);
        assert!((1.15..1.25).contains(&audio.speed(attack)));
        let before = audio.speed(attack);
        audio.set(0.0, 1.0, attack);
        assert_eq!(audio.speed(attack), before);
        for ms in 80..1000 {
            assert!((1.0..=1.300001).contains(&audio.speed(now + Duration::from_millis(ms))));
        }
        let quiet = now + Duration::from_secs(1);
        audio.set(0.0, 0.0, quiet);
        assert!(audio.speed(quiet + Duration::from_millis(300)) > 1.05);
        assert!(audio.speed(quiet + Duration::from_secs(3)) < 1.001);
    }
}
