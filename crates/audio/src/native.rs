//! CPAL streams: callbacks touch only preallocated lock-free rings and atomics.
use crate::{AudioError, dsp::Resampler};
use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_queue::ArrayQueue;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const MAX_RING_SAMPLES: usize = 38_400;
fn ring_samples(rate: u32) -> usize {
    (rate as usize / 5).min(MAX_RING_SAMPLES)
}
#[derive(Clone, Copy)]
struct OutputSample {
    generation: u64,
    value: f32,
}
/// Open returns paused streams. Caller starts after explicit user action and
/// eligibility; merely constructing preferences must never open a microphone.
pub struct AudioIo {
    input: Stream,
    output: Stream,
    pub capture: Arc<ArrayQueue<f32>>,
    pub reference: Arc<ArrayQueue<f32>>,
    playout: Arc<ArrayQueue<OutputSample>>,
    pub failed: Arc<AtomicBool>,
    pub muted: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    pub input_rate: u32,
    pub output_rate: u32,
    output_resampler: Resampler,
    scratch: Vec<f32>,
}
impl AudioIo {
    pub fn open(generation: u64) -> Result<Self, AudioError> {
        let host = cpal::default_host();
        let input_device = host.default_input_device().ok_or(AudioError::Device)?;
        let output_device = host.default_output_device().ok_or(AudioError::Device)?;
        let input_config = input_device
            .default_input_config()
            .map_err(|_| AudioError::Device)?;
        let output_config = output_device
            .default_output_config()
            .map_err(|_| AudioError::Device)?;
        let input_rate = input_config.sample_rate();
        let output_rate = output_config.sample_rate();
        let capture = Arc::new(ArrayQueue::new(ring_samples(input_rate)));
        let reference = Arc::new(ArrayQueue::new(ring_samples(output_rate)));
        let playout = Arc::new(ArrayQueue::new(ring_samples(output_rate)));
        let failed = Arc::new(AtomicBool::new(false));
        let muted = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(generation));
        let ic: StreamConfig = input_config.clone().into();
        let oc: StreamConfig = output_config.clone().into();
        if ic.channels == 0 || oc.channels == 0 || ic.channels > 8 || oc.channels > 8 {
            return Err(AudioError::Device);
        }
        let output_resampler = Resampler::new(24_000, output_rate).ok_or(AudioError::Device)?;
        if Resampler::new(input_rate, 48_000).is_none() {
            return Err(AudioError::Device);
        }
        macro_rules! input {
            ($t:ty) => {
                input_stream::<$t>(
                    &input_device,
                    ic,
                    capture.clone(),
                    failed.clone(),
                    muted.clone(),
                )?
            };
        }
        macro_rules! output {
            ($t:ty) => {
                output_stream::<$t>(
                    &output_device,
                    oc,
                    playout.clone(),
                    reference.clone(),
                    failed.clone(),
                    generation.clone(),
                )?
            };
        }
        let input = match input_config.sample_format() {
            SampleFormat::F32 => input!(f32),
            SampleFormat::F64 => input!(f64),
            SampleFormat::I16 => input!(i16),
            SampleFormat::U16 => input!(u16),
            SampleFormat::I32 => input!(i32),
            SampleFormat::U32 => input!(u32),
            SampleFormat::I8 => input!(i8),
            SampleFormat::U8 => input!(u8),
            _ => return Err(AudioError::Device),
        };
        let output = match output_config.sample_format() {
            SampleFormat::F32 => output!(f32),
            SampleFormat::F64 => output!(f64),
            SampleFormat::I16 => output!(i16),
            SampleFormat::U16 => output!(u16),
            SampleFormat::I32 => output!(i32),
            SampleFormat::U32 => output!(u32),
            SampleFormat::I8 => output!(i8),
            SampleFormat::U8 => output!(u8),
            _ => return Err(AudioError::Device),
        };
        Ok(Self {
            input,
            output,
            capture,
            reference,
            playout,
            failed,
            muted,
            generation,
            input_rate,
            output_rate,
            output_resampler,
            scratch: Vec::with_capacity(MAX_RING_SAMPLES),
        })
    }
    pub fn start(&self) -> Result<(), AudioError> {
        self.output.play().map_err(|_| AudioError::Device)?;
        if self.input.play().is_err() {
            let _ = self.output.pause();
            return Err(AudioError::Device);
        }
        Ok(())
    }
    pub fn enqueue(&mut self, frame: &zeron_proto::voice::VoiceFrame) -> Result<(), AudioError> {
        if frame.generation != self.generation.load(Ordering::Acquire) {
            return Err(AudioError::Frame);
        }
        let samples = crate::decode(frame)?;
        self.scratch.clear();
        for sample in samples {
            self.output_resampler.push(sample, &mut self.scratch);
        }
        if self.scratch.len() > self.playout.capacity() - self.playout.len() {
            return Err(AudioError::Overflow);
        }
        for value in &self.scratch {
            self.playout
                .push(OutputSample {
                    generation: frame.generation,
                    value: *value,
                })
                .map_err(|_| AudioError::Overflow)?;
        }
        Ok(())
    }
    pub fn mute(&self, value: bool) {
        self.muted.store(value, Ordering::Release);
        while self.capture.pop().is_some() {}
    }
    pub fn flush(&mut self, next_generation: u64) {
        self.generation.store(next_generation, Ordering::Release);
        while self.playout.pop().is_some() {}
        while self.reference.pop().is_some() {}
        while self.capture.pop().is_some() {}
        self.output_resampler = Resampler::new(24_000, self.output_rate).unwrap();
    }
}
impl Drop for AudioIo {
    fn drop(&mut self) {
        self.muted.store(true, Ordering::Release);
        let _ = self.input.pause();
        let _ = self.output.pause();
        self.generation.fetch_add(1, Ordering::AcqRel);
        while self.playout.pop().is_some() {}
    }
}
fn input_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    ring: Arc<ArrayQueue<f32>>,
    failed: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
) -> Result<Stream, AudioError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    let errors = failed.clone();
    device
        .build_input_stream(
            config,
            move |samples: &[T], _| {
                if muted.load(Ordering::Acquire) {
                    return;
                }
                for frame in samples.chunks_exact(channels) {
                    let mono =
                        frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / channels as f32;
                    if ring.push(mono).is_err() {
                        failed.store(true, Ordering::Release);
                        break;
                    }
                }
            },
            move |_| {
                errors.store(true, Ordering::Release);
            },
            Some(std::time::Duration::from_secs(2)),
        )
        .map_err(|_| AudioError::Device)
}
fn output_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    ring: Arc<ArrayQueue<OutputSample>>,
    reference: Arc<ArrayQueue<f32>>,
    failed: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
) -> Result<Stream, AudioError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let errors = failed.clone();
    device
        .build_output_stream(
            config,
            move |samples: &mut [T], _| {
                for frame in samples.chunks_exact_mut(channels) {
                    let value = ring
                        .pop()
                        .filter(|s| s.generation == generation.load(Ordering::Acquire))
                        .map_or(0.0, |s| s.value);
                    for sample in frame {
                        *sample = T::from_sample(value);
                    }
                    // Exactly what was sent to the device, including underrun silence.
                    if reference.push(value).is_err() {
                        failed.store(true, Ordering::Release);
                    }
                }
            },
            move |_| {
                errors.store(true, Ordering::Release);
            },
            Some(std::time::Duration::from_secs(2)),
        )
        .map_err(|_| AudioError::Device)
}
