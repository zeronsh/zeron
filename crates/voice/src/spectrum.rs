//! How loud each band of a voice is, for a visual to follow. Levels only:
//! the samples are filtered where they arrive and never kept, sent or
//! written anywhere.
//!
//! [`Analyser`] is the measure itself and needs no device; [`Listener`] runs
//! it on the microphone while a call that already uses it is live.
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, StreamTrait};
use std::{
    f32::consts::TAU,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

/// The bands a voice is split into, lowest first.
pub const BANDS: usize = 5;
/// Their edges in hertz: the voice's pitch, its two vowel resonances, the
/// consonants' presence, and the hiss of sibilants.
const EDGES: [f32; BANDS + 1] = [90.0, 280.0, 750.0, 2000.0, 4500.0, 9000.0];
/// The rate the filters run at: samples are averaged down to about this.
const WORKING_RATE: u32 = 22_000;
/// How fast a band's power rises to a sound and falls after it, in seconds.
const FOLLOW: (f32, f32) = (0.004, 0.07);
/// A voice is quieter the higher it goes: what each band is given, in
/// decibels, so a vowel and a consonant fill the same scale.
const TILT: [f32; BANDS] = [2.0, 0.0, 6.0, 12.0, 16.0];
/// The scale is the voice's own. Its top is the loudest any band has lately
/// been, easing down this many decibels a second once the voice drops; each
/// band's floor is the quietest it has been, creeping up this many, so a
/// steady noise sinks into it.
const ADAPT: (f32, f32) = (4.0, 5.0);
/// A band reads from the top of the scale down over this many decibels…
const SPAN: f32 = 18.0;
/// …once it clears its floor by this much, and the top is never closer to
/// the floors than `RANGE`: room noise stays at zero.
const GATE: f32 = 8.0;
const RANGE: f32 = 30.0;
/// The quietest a band is taken to be, in decibels.
const SILENCE: f32 = -78.0;

/// One second-order section of a filter.
#[derive(Clone, Copy, Default)]
struct Filter {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Filter {
    /// A section that passes what is above `edge` (or below it), with the
    /// sharpness `q` that makes two of them a fourth-order Butterworth.
    fn pass(rate: f32, edge: f32, above: bool, q: f32) -> Self {
        let (sin, cos) = (TAU * edge / rate).sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b = if above {
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0]
        } else {
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0]
        };
        Self {
            b: b.map(|b| b / a0),
            a: [-2.0 * cos / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    /// The four sections that keep `low` to `high` hertz, with skirts of
    /// 24 dB an octave on either side.
    fn band(rate: f32, low: f32, high: f32) -> [Self; 4] {
        const SHARPNESS: [f32; 2] = [0.541_196_1, 1.306_563];
        [
            Self::pass(rate, low, true, SHARPNESS[0]),
            Self::pass(rate, low, true, SHARPNESS[1]),
            Self::pass(rate, high, false, SHARPNESS[0]),
            Self::pass(rate, high, false, SHARPNESS[1]),
        ]
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

#[derive(Clone, Copy, Default)]
struct Band {
    filters: [Filter; 4],
    /// The device's rate reaches this band.
    open: bool,
    power: f32,
    /// The quietest it has lately been, in decibels, once it has heard
    /// anything.
    floor: Option<f32>,
}

/// Splits a voice into [`BANDS`] and says how loud each is, 0–1, on the
/// voice's own scale: the same whatever the microphone's gain, with a
/// steady noise reading as silence.
pub struct Analyser {
    bands: [Band; BANDS],
    /// The top of the scale, in decibels.
    ceiling: f32,
    /// Samples averaged into one before the filters, and those waiting.
    stride: usize,
    waiting: (f32, usize),
    rise: f32,
    fall: f32,
}

impl Analyser {
    pub fn new(rate: u32) -> Self {
        let stride = (rate / WORKING_RATE).max(1) as usize;
        let rate = rate as f32 / stride as f32;
        let mut bands = [Band::default(); BANDS];
        for (index, band) in bands.iter_mut().enumerate() {
            let (low, high) = (EDGES[index], EDGES[index + 1].min(rate * 0.45));
            band.open = high > low * 1.2;
            if band.open {
                band.filters = Filter::band(rate, low, high);
            }
        }
        Self {
            bands,
            ceiling: f32::NEG_INFINITY,
            stride,
            waiting: (0.0, 0),
            rise: 1.0 - (-1.0 / (FOLLOW.0 * rate)).exp(),
            fall: 1.0 - (-1.0 / (FOLLOW.1 * rate)).exp(),
        }
    }

    /// One sample of the voice.
    #[inline]
    pub fn push(&mut self, sample: f32) {
        self.waiting.0 += sample;
        self.waiting.1 += 1;
        if self.waiting.1 < self.stride {
            return;
        }
        let x = self.waiting.0 / self.stride as f32;
        self.waiting = (0.0, 0);
        for band in self.bands.iter_mut().filter(|band| band.open) {
            let y = band.filters.iter_mut().fold(x, |x, filter| filter.run(x));
            let power = y * y;
            let follow = if power > band.power {
                self.rise
            } else {
                self.fall
            };
            band.power += (power - band.power) * follow;
        }
    }

    /// How loud each band is now, `seconds` after the last reading.
    pub fn levels(&mut self, seconds: f32) -> [f32; BANDS] {
        let mut loud = [f32::NEG_INFINITY; BANDS];
        let (mut top, mut ground) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
        for (index, band) in self.bands.iter_mut().enumerate() {
            if !band.open || !band.power.is_finite() {
                band.power = 0.0;
                continue;
            }
            let now = (10.0 * (band.power + 1e-12).log10()).max(SILENCE) + TILT[index];
            let floor = now.min(band.floor.map_or(now, |floor| floor + ADAPT.0 * seconds));
            band.floor = Some(floor);
            loud[index] = now;
            top = top.max(now);
            ground = ground.max(floor - TILT[index]);
        }
        self.ceiling = top
            .max(self.ceiling - ADAPT.1 * seconds)
            .max(ground + RANGE);
        std::array::from_fn(|index| {
            let Some(floor) = self.bands[index].floor else {
                return 0.0;
            };
            let clear = ((loud[index] - floor - GATE) / GATE).clamp(0.0, 1.0);
            clear * (1.0 - (self.ceiling - loud[index]) / SPAN).clamp(0.0, 1.0)
        })
    }
}

struct Shared {
    levels: [AtomicU32; BANDS],
    paused: AtomicBool,
    stop: AtomicBool,
    failed: AtomicBool,
}

/// The microphone's bands while a call is live. It opens the default input
/// beside the call's own capture, measures, and keeps nothing; dropping it
/// closes the input.
pub struct Listener {
    shared: Arc<Shared>,
    thread: std::thread::Thread,
}

impl Listener {
    /// Starts listening. The input opens on a thread of its own, so this
    /// never waits on the device.
    pub fn start() -> Result<Self> {
        let shared = Arc::new(Shared {
            levels: Default::default(),
            paused: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
        });
        let theirs = shared.clone();
        let thread = std::thread::Builder::new()
            .name("voice-spectrum".into())
            .spawn(move || {
                if listen(&theirs).is_err() {
                    theirs.failed.store(true, Ordering::Release);
                }
                for level in &theirs.levels {
                    level.store(0, Ordering::Relaxed);
                }
            })?
            .thread()
            .clone();
        Ok(Self { shared, thread })
    }

    /// Stops measuring while the microphone is muted, and resumes after.
    pub fn pause(&self, paused: bool) {
        if self.shared.paused.swap(paused, Ordering::AcqRel) != paused {
            self.thread.unpark();
        }
    }

    /// Each band's level now, or `None` if the input could not be heard.
    pub fn levels(&self) -> Option<[f32; BANDS]> {
        (!self.shared.failed.load(Ordering::Acquire)).then(|| {
            std::array::from_fn(|index| {
                f32::from_bits(self.shared.levels[index].load(Ordering::Relaxed))
            })
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

/// Owns the input until told to stop. The stream stays on this thread: it
/// may not move between threads on every platform.
fn listen(shared: &Arc<Shared>) -> Result<()> {
    let device = super::input_device(None).context("No microphone available")?;
    let config = device.default_input_config()?;
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    if channels == 0 {
        bail!("Unsupported microphone format");
    }
    let failed = shared.clone();
    let err = move |_| failed.failed.store(true, Ordering::Release);
    macro_rules! stream {
        ($sample:ty) => {{
            let (mut analyser, shared) = (Analyser::new(rate), shared.clone());
            device.build_input_stream(
                &config.into(),
                move |data: &[$sample], _| {
                    measure(data, channels, rate, &mut analyser, &shared.levels)
                },
                err,
                None,
            )?
        }};
    }
    let stream = match config.sample_format() {
        cpal::SampleFormat::I8 => stream!(i8),
        cpal::SampleFormat::I16 => stream!(i16),
        cpal::SampleFormat::I24 => stream!(cpal::I24),
        cpal::SampleFormat::I32 => stream!(i32),
        cpal::SampleFormat::I64 => stream!(i64),
        cpal::SampleFormat::U8 => stream!(u8),
        cpal::SampleFormat::U16 => stream!(u16),
        cpal::SampleFormat::U24 => stream!(cpal::U24),
        cpal::SampleFormat::U32 => stream!(u32),
        cpal::SampleFormat::U64 => stream!(u64),
        cpal::SampleFormat::F32 => stream!(f32),
        cpal::SampleFormat::F64 => stream!(f64),
        _ => bail!("Unsupported microphone format"),
    };
    let mut playing = false;
    while !shared.stop.load(Ordering::Acquire) && !shared.failed.load(Ordering::Acquire) {
        let wanted = !shared.paused.load(Ordering::Acquire);
        if wanted != playing {
            if wanted {
                stream.play()?;
            } else {
                stream.pause()?;
                for level in &shared.levels {
                    level.store(0, Ordering::Relaxed);
                }
            }
            playing = wanted;
        }
        std::thread::park_timeout(Duration::from_millis(250));
    }
    Ok(())
}

/// One buffer from the device: through the analyser, and its bands out.
fn measure<T: cpal::Sample>(
    data: &[T],
    channels: usize,
    rate: u32,
    analyser: &mut Analyser,
    levels: &[AtomicU32; BANDS],
) where
    f32: cpal::FromSample<T>,
{
    let frames = data.len() / channels;
    if frames == 0 {
        return;
    }
    for frame in data.chunks_exact(channels) {
        analyser.push(frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32);
    }
    let heard = analyser.levels(frames as f32 / rate as f32);
    for (level, heard) in levels.iter().zip(heard) {
        level.store(heard.to_bits(), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second of `tone` hertz at `gain` over a faint hiss, read every
    /// 10 ms as a device would deliver it; the last reading.
    fn hear(analyser: &mut Analyser, rate: u32, tone: Option<f32>, gain: f32) -> [f32; BANDS] {
        let mut noise = 0x2545_f491u32;
        let mut levels = [0.0; BANDS];
        for block in 0..100 {
            for index in 0..rate / 100 {
                noise ^= noise << 13;
                noise ^= noise >> 17;
                noise ^= noise << 5;
                let hiss = (noise as f32 / u32::MAX as f32 - 0.5) * 0.002;
                let t = (block * (rate / 100) + index) as f32 / rate as f32;
                analyser.push(hiss + tone.map_or(0.0, |tone| gain * (TAU * tone * t).sin()));
            }
            levels = analyser.levels(0.01);
        }
        levels
    }

    #[test]
    fn a_voice_is_split_into_its_bands_whatever_the_microphone() {
        for (rate, gain) in [(48_000, 0.3), (44_100, 0.03), (16_000, 0.3)] {
            let mut analyser = Analyser::new(rate);
            // A room's hiss is silence in every band.
            assert_eq!(hear(&mut analyser, rate, None, 0.0), [0.0; BANDS], "{rate}");
            // Each part of a voice lights its own band and leaves the
            // others near dark, at a raised voice as at a low one.
            {
                for (band, tone) in [(0, 160.0), (1, 450.0), (2, 1200.0), (3, 3000.0)] {
                    let levels = hear(&mut analyser, rate, Some(tone), gain);
                    assert!(levels[band] > 0.6, "{rate} {tone} {gain}: {levels:?}");
                    for (other, level) in levels.iter().enumerate() {
                        assert!(
                            other == band || *level < levels[band] * 0.5,
                            "{rate} {tone} {gain}: {levels:?}"
                        );
                    }
                    // …and it falls back once the sound stops.
                    assert_eq!(hear(&mut analyser, rate, None, 0.0)[band], 0.0);
                }
            }
        }
        // The hiss of an "s" needs a microphone that reaches it.
        let mut analyser = Analyser::new(48_000);
        hear(&mut analyser, 48_000, None, 0.0);
        assert!(hear(&mut analyser, 48_000, Some(6500.0), 0.1)[4] > 0.6);
        let mut narrow = Analyser::new(8_000);
        hear(&mut narrow, 8_000, None, 0.0);
        assert_eq!(hear(&mut narrow, 8_000, Some(3000.0), 0.1)[4], 0.0);
    }
}
