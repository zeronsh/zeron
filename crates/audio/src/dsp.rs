//! Stateful resampling and fixed framing outside real-time callbacks.
/// Continuous linear resampler. One sample of lookahead, no chunk-edge reset.
pub struct Resampler {
    step: f64,
    phase: f64,
    previous: Option<f32>,
}
impl Resampler {
    pub fn new(input: u32, output: u32) -> Option<Self> {
        (input >= 8_000 && output >= 8_000 && input <= 192_000 && output <= 192_000).then_some(
            Self {
                step: input as f64 / output as f64,
                phase: 0.0,
                previous: None,
            },
        )
    }
    pub fn push(&mut self, sample: f32, output: &mut Vec<f32>) {
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if let Some(previous) = self.previous {
            while self.phase < 1.0 {
                output.push(previous + (sample - previous) * self.phase as f32);
                self.phase += self.step;
            }
            self.phase -= 1.0;
        }
        self.previous = Some(sample);
    }
}
pub fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples
        .iter()
        .map(|s| if s.is_finite() { s * s } else { 0.0 })
        .sum::<f32>()
        / samples.len() as f32)
        .sqrt()
        .clamp(0.0, 1.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampling_keeps_continuity_and_exact_duration_across_chunks() {
        let input: Vec<f32> = (0..4801).map(|i| (i as f32 * 0.03).sin()).collect();
        let mut a = Resampler::new(48_000, 24_000).unwrap();
        let mut all = Vec::new();
        for x in &input {
            a.push(*x, &mut all);
        }
        let mut b = Resampler::new(48_000, 24_000).unwrap();
        let mut chunks = Vec::new();
        for chunk in input.chunks(37) {
            for x in chunk {
                b.push(*x, &mut chunks);
            }
        }
        assert_eq!(all, chunks);
        assert_eq!(all.len(), 2400);
        assert!(all.iter().all(|s| s.is_finite()));
    }
}
