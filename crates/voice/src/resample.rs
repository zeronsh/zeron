//! Convert device-rate mono capture to the model's 16 kHz input on the worker.
use anyhow::{Result, ensure};
use rubato::{FftFixedInOut, Resampler};

pub(crate) const MODEL_RATE: u32 = 16_000;

pub(crate) fn for_model(samples: Vec<f32>, rate: u32) -> Result<Vec<f32>> {
    ensure!(
        (8_000..=192_000).contains(&rate),
        "Unsupported microphone sample rate"
    );
    if rate == MODEL_RATE || samples.is_empty() {
        return Ok(samples);
    }
    let length = (samples.len() as u64 * MODEL_RATE as u64 / rate as u64) as usize;
    let mut resampler = FftFixedInOut::<f32>::new(rate as usize, MODEL_RATE as usize, 1024, 1)?;
    let delay = resampler.output_delay();
    let chunk = resampler.input_frames_next();
    let mut input = vec![vec![0.0; chunk]];
    let mut output = resampler.output_buffer_allocate(true);
    let mut converted = Vec::with_capacity(length + delay + resampler.output_frames_max());
    let mut offset = 0;
    // Zero padding also flushes the filter tail. Remove its delay so short
    // utterances keep both their beginning and end and retain their duration.
    while converted.len() < length + delay {
        input[0].fill(0.0);
        let end = (offset + chunk).min(samples.len());
        if offset < end {
            input[0][..end - offset].copy_from_slice(&samples[offset..end]);
        }
        let (_, written) = resampler.process_into_buffer(&input, &mut output, None)?;
        converted.extend_from_slice(&output[0][..written]);
        offset = end;
    }
    converted.drain(..delay);
    converted.truncate(length);
    Ok(converted)
}

#[cfg(test)]
mod tests {
    use super::*;
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
