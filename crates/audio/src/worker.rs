//! Poll on an audio worker, never GPUI or a hardware callback. Exact 10 ms
//! DSP windows retain partial frames between polls; network frames are 20 ms.
use crate::{
    AudioError,
    aec::{DSP_SAMPLES, EchoProcessor},
    dsp::Resampler,
    native::AudioIo,
};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use zeron_proto::voice::VoiceFrame;

pub struct AudioWorker {
    pub io: AudioIo,
    echo: EchoProcessor,
    mic_resampler: Resampler,
    reference_resampler: Resampler,
    mic: VecDeque<f32>,
    reference: VecDeque<f32>,
    network: Vec<f32>,
    scratch: Vec<f32>,
    generation: u64,
    sequence: u64,
}
impl AudioWorker {
    pub fn open(generation: u64) -> Result<Self, AudioError> {
        let io = AudioIo::open(generation)?;
        Ok(Self {
            echo: EchoProcessor::new()?,
            mic_resampler: Resampler::new(io.input_rate, 48_000).ok_or(AudioError::Device)?,
            reference_resampler: Resampler::new(io.output_rate, 48_000)
                .ok_or(AudioError::Device)?,
            io,
            mic: VecDeque::with_capacity(9600),
            reference: VecDeque::with_capacity(9600),
            network: Vec::with_capacity(480),
            scratch: Vec::with_capacity(8),
            generation,
            sequence: 0,
        })
    }
    pub fn poll(&mut self) -> Result<Vec<VoiceFrame>, AudioError> {
        if self.io.failed.load(Ordering::Acquire) {
            return Err(AudioError::Device);
        }
        while let Some(sample) = self.io.reference.pop() {
            self.scratch.clear();
            self.reference_resampler.push(sample, &mut self.scratch);
            self.reference.extend(self.scratch.iter().copied());
            if self.reference.len() > 9600 {
                return Err(AudioError::Overflow);
            }
        }
        while self.reference.len() >= DSP_SAMPLES {
            let frame = std::array::from_fn(|_| self.reference.pop_front().unwrap());
            self.echo.render(&frame)?;
        }
        if self.io.muted.load(Ordering::Acquire) {
            while self.io.capture.pop().is_some() {}
            self.mic.clear();
            self.network.clear();
            self.mic_resampler = Resampler::new(self.io.input_rate, 48_000).unwrap();
            return Ok(Vec::new());
        }
        while let Some(sample) = self.io.capture.pop() {
            self.scratch.clear();
            self.mic_resampler.push(sample, &mut self.scratch);
            self.mic.extend(self.scratch.iter().copied());
            if self.mic.len() > 9600 {
                return Err(AudioError::Overflow);
            }
        }
        let mut frames = Vec::with_capacity(10);
        while self.mic.len() >= DSP_SAMPLES {
            let mut frame = std::array::from_fn(|_| self.mic.pop_front().unwrap());
            self.echo.capture(&mut frame)?;
            // Exact 48 -> 24 kHz decimation with a pair average after AEC.
            self.network
                .extend(frame.chunks_exact(2).map(|s| (s[0] + s[1]) * 0.5));
            if self.network.len() == 480 {
                self.sequence += 1;
                frames.push(crate::encode(
                    &self.network,
                    self.generation,
                    self.sequence,
                )?);
                self.network.clear();
            }
        }
        Ok(frames)
    }
    pub fn invalidate_playout(&mut self, next_generation: u64) -> Result<(), AudioError> {
        self.io.flush(next_generation);
        self.echo.reset()?;
        self.generation = next_generation;
        self.sequence = 0;
        self.mic.clear();
        self.reference.clear();
        self.network.clear();
        self.mic_resampler = Resampler::new(self.io.input_rate, 48_000).unwrap();
        self.reference_resampler = Resampler::new(self.io.output_rate, 48_000).unwrap();
        Ok(())
    }
}
