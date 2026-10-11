//! Convert device-rate mono capture to the model's 16 kHz input on the worker.
use anyhow::{Result, ensure};
use rubato::{FftFixedInOut, Resampler};

pub(crate) const MODEL_RATE: u32 = 16_000;

pub(crate) struct Converter {
    inner: Option<FftFixedInOut<f32>>,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    fill: usize,
    skip: usize,
    pushed: usize,
    rate: u32,
}

impl Converter {
    pub(crate) fn new(rate: u32) -> Result<Self> {
        ensure!(
            (8_000..=192_000).contains(&rate),
            "Unsupported microphone sample rate"
        );
        let inner = (rate != MODEL_RATE)
            .then(|| FftFixedInOut::<f32>::new(rate as usize, MODEL_RATE as usize, 1024, 1))
            .transpose()?;
        let (input, output, skip) = match &inner {
            Some(r) => (
                vec![vec![0.0; r.input_frames_next()]],
                r.output_buffer_allocate(true),
                r.output_delay(),
            ),
            None => (Vec::new(), Vec::new(), 0),
        };
        Ok(Self {
            inner,
            input,
            output,
            fill: 0,
            skip,
            pushed: 0,
            rate,
        })
    }

    pub(crate) fn push(&mut self, samples: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.pushed += samples.len();
        let Some(resampler) = &mut self.inner else {
            out.extend_from_slice(samples);
            return Ok(());
        };
        let chunk = self.input[0].len();
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (chunk - self.fill).min(rest.len());
            self.input[0][self.fill..self.fill + take].copy_from_slice(&rest[..take]);
            self.fill += take;
            rest = &rest[take..];
            if self.fill == chunk {
                convert(
                    resampler,
                    &self.input,
                    &mut self.output,
                    &mut self.skip,
                    out,
                )?;
                self.fill = 0;
            }
        }
        Ok(())
    }

    // Zero padding also flushes the filter tail. Remove its delay so short
    // utterances keep both their beginning and end and retain their duration.
    pub(crate) fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let length = (self.pushed as u64 * MODEL_RATE as u64 / self.rate as u64) as usize;
        if let Some(resampler) = &mut self.inner {
            self.input[0][self.fill..].fill(0.0);
            while self.fill > 0 || out.len() < length {
                convert(
                    resampler,
                    &self.input,
                    &mut self.output,
                    &mut self.skip,
                    out,
                )?;
                self.input[0].fill(0.0);
                self.fill = 0;
            }
        }
        out.truncate(length);
        Ok(())
    }
}

fn convert(
    resampler: &mut FftFixedInOut<f32>,
    input: &[Vec<f32>],
    output: &mut [Vec<f32>],
    skip: &mut usize,
    out: &mut Vec<f32>,
) -> Result<()> {
    let (_, written) = resampler.process_into_buffer(input, output, None)?;
    let dropped = (*skip).min(written);
    *skip -= dropped;
    out.extend_from_slice(&output[0][dropped..written]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn for_model(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>> {
        let mut converter = Converter::new(rate)?;
        let mut out = Vec::new();
        converter.push(&samples, &mut out)?;
        converter.finish(&mut out)?;
        Ok(out)
    }
    fn tone(rate: u32, hz: f32) -> Vec<f32> {
        (0..rate)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin())
            .collect()
    }
    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
    }
    #[test]
    fn device_rates_preserve_duration_and_speech_band() {
        for rate in [8_000, 16_000, 24_000, 44_100, 48_000, 96_000] {
            let converted = for_model(tone(rate, 1_000.0), rate).unwrap();
            assert_eq!(converted.len(), MODEL_RATE as usize, "{rate}");
            // FFT filtering can leave a fractional-sample phase offset at
            // noninteger ratios. Assert frequency and amplitude, not phase.
            let steady = &converted[1000..15000];
            let cycles = steady
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            assert!((874..=876).contains(&cycles), "{rate}: {cycles}");
            assert!(
                (rms(steady) - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01,
                "{rate}"
            );
        }
    }
    #[test]
    fn downsampling_filters_frequencies_above_model_nyquist() {
        let converted = for_model(tone(48_000, 12_000.0), 48_000).unwrap();
        assert!(rms(&converted[1000..15000]) < 0.01);
    }
    #[test]
    fn short_capture_and_invalid_rates_are_bounded() {
        assert_eq!(for_model(vec![0.0; 147], 44_100).unwrap().len(), 53);
        assert!(for_model(Vec::new(), 48_000).unwrap().is_empty());
        assert!(for_model(vec![1.0], 0).is_err());
    }
}
