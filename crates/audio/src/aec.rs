//! WebRTC AEC3 at 48 kHz / 10 ms. Feed audio actually rendered by the
//! hardware, including underrun silence; never use received network packets.
use crate::AudioError;
use webrtc_audio_processing::{
    Config, Processor,
    config::{EchoCanceller, HighPassFilter},
};

pub const DSP_SAMPLES: usize = 480;
pub struct EchoProcessor {
    processor: Processor,
}
impl EchoProcessor {
    pub fn new() -> Result<Self, AudioError> {
        let processor = Processor::new(48_000).map_err(|_| AudioError::Device)?;
        processor.set_config(Config {
            echo_canceller: Some(EchoCanceller::Full {
                stream_delay_ms: None,
            }),
            high_pass_filter: Some(HighPassFilter::default()),
            ..Config::default()
        });
        Ok(Self { processor })
    }
    pub fn render(&self, actually_played: &[f32; DSP_SAMPLES]) -> Result<(), AudioError> {
        self.processor
            .analyze_render_frame([actually_played.as_slice()])
            .map_err(|_| AudioError::Frame)
    }
    pub fn capture(&self, microphone: &mut [f32; DSP_SAMPLES]) -> Result<(), AudioError> {
        self.processor
            .process_capture_frame([microphone.as_mut_slice()])
            .map_err(|_| AudioError::Frame)
    }
    /// Device/session changes discard adaptive history as well as samples.
    pub fn reset(&mut self) -> Result<(), AudioError> {
        *self = Self::new()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aec_reduces_delayed_synthetic_echo_without_erasing_double_talk() {
        let processor=EchoProcessor::new().unwrap();
        let mut delayed=std::collections::VecDeque::from([[0.0;DSP_SAMPLES];3]);
        let mut random=1u32;
        let mut before=0.0f64; let mut after=0.0f64;
        let mut near_before=0.0f64; let mut near_after=0.0f64;
        for index in 0..1000 {
            let reference=std::array::from_fn(|_| {
                random=random.wrapping_mul(1664525).wrapping_add(1013904223);
                (random as f64/u32::MAX as f64*0.4-0.2) as f32
            });
            processor.render(&reference).unwrap();
            let echo=delayed.pop_front().unwrap(); delayed.push_back(reference);
            let mut microphone=std::array::from_fn(|sample| {
                let human=if index>=800 { ((index*DSP_SAMPLES+sample) as f32*0.08).sin()*0.15 } else { 0.0 };
                human+echo[sample]*0.55
            });
            let input_energy=microphone.iter().map(|s|(*s as f64).powi(2)).sum::<f64>();
            processor.capture(&mut microphone).unwrap();
            let output_energy=microphone.iter().map(|s|(*s as f64).powi(2)).sum::<f64>();
            if (600..800).contains(&index) { before+=input_energy; after+=output_energy; }
            if index>=900 { near_before+=input_energy; near_after+=output_energy; }
        }
        assert!(after<before*0.5,"echo energy ratio {}",after/before);
        assert!(near_after>near_before*0.1,"double-talk energy ratio {}",near_after/near_before);
    }

    #[test]
    fn reverse_and_capture_frames_are_finite_and_reset_cleanly() {
        let mut processor = EchoProcessor::new().unwrap();
        for frame in 0..400 {
            let mut reference = [0.0; DSP_SAMPLES];
            for (i, v) in reference.iter_mut().enumerate() {
                *v = ((frame * DSP_SAMPLES + i) as f32 * 0.04).sin() * 0.2;
            }
            processor.render(&reference).unwrap();
            let mut microphone = reference.map(|s| s * 0.6);
            processor.capture(&mut microphone).unwrap();
            assert!(microphone.iter().all(|s| s.is_finite()));
        }
        processor.reset().unwrap();
        processor.capture(&mut [0.0; DSP_SAMPLES]).unwrap();
    }
}
