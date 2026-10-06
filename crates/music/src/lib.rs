pub mod source;

use std::collections::VecDeque;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use rubato::{FftFixedInOut, Resampler};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

const RING_SECONDS: f32 = 1.0;
const FILL_INTERVAL: Duration = Duration::from_millis(40);
const RESAMPLE_CHUNK: usize = 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Started { duration: Option<f64> },
    Ended,
    Failed(String),
}

enum Command {
    Load(PathBuf),
    Play,
    Pause,
    Seek(f64),
    Stop,
}

struct Shared {
    volume: AtomicU32,
    played_frames: AtomicU64,
    rate: AtomicU32,
    base_seconds: AtomicU64,
    duration: AtomicU64,
    playing: AtomicBool,
}

impl Shared {
    fn reset_clock(&self, seconds: f64) {
        self.base_seconds
            .store(seconds.to_bits(), Ordering::Relaxed);
        self.played_frames.store(0, Ordering::Relaxed);
    }
}

pub struct Player {
    commands: mpsc::Sender<Command>,
    shared: Arc<Shared>,
}

impl Player {
    pub fn new() -> (Self, UnboundedReceiver<Event>) {
        let (commands, receiver) = mpsc::channel();
        let (events, event_rx) = unbounded();
        let shared = Arc::new(Shared {
            volume: AtomicU32::new(0.8f32.to_bits()),
            played_frames: AtomicU64::new(0),
            rate: AtomicU32::new(48_000),
            base_seconds: AtomicU64::new(0f64.to_bits()),
            duration: AtomicU64::new(f64::NAN.to_bits()),
            playing: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name("zeron-music".into())
            .spawn(move || run(receiver, thread_shared, events))
            .expect("spawn music thread");
        (Self { commands, shared }, event_rx)
    }

    pub fn load(&self, path: PathBuf) {
        let _ = self.commands.send(Command::Load(path));
    }

    pub fn play(&self) {
        let _ = self.commands.send(Command::Play);
    }

    pub fn pause(&self) {
        let _ = self.commands.send(Command::Pause);
    }

    pub fn seek(&self, seconds: f64) {
        let _ = self.commands.send(Command::Seek(seconds.max(0.0)));
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::Relaxed))
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn position(&self) -> f64 {
        let base = f64::from_bits(self.shared.base_seconds.load(Ordering::Relaxed));
        let frames = self.shared.played_frames.load(Ordering::Relaxed) as f64;
        let rate = self.shared.rate.load(Ordering::Relaxed).max(1) as f64;
        base + frames / rate
    }

    pub fn duration(&self) -> Option<f64> {
        let duration = f64::from_bits(self.shared.duration.load(Ordering::Relaxed));
        duration.is_finite().then_some(duration)
    }
}

fn run(commands: mpsc::Receiver<Command>, shared: Arc<Shared>, events: UnboundedSender<Event>) {
    let mut session: Option<Session> = None;
    loop {
        let active = session.as_ref().is_some_and(|session| session.playing);
        let command = if active {
            match commands.recv_timeout(FILL_INTERVAL) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => return,
            }
        };
        match command {
            Some(Command::Load(path)) => {
                session = None;
                shared.playing.store(false, Ordering::Relaxed);
                match Session::open(&path, shared.clone()) {
                    Ok(mut opened) => {
                        let duration = opened.duration;
                        shared
                            .duration
                            .store(duration.unwrap_or(f64::NAN).to_bits(), Ordering::Relaxed);
                        shared.reset_clock(0.0);
                        match opened.set_playing(true) {
                            Ok(()) => {
                                session = Some(opened);
                                let _ = events.unbounded_send(Event::Started { duration });
                            }
                            Err(err) => {
                                let _ = events.unbounded_send(Event::Failed(err.to_string()));
                            }
                        }
                    }
                    Err(err) => {
                        let _ = events.unbounded_send(Event::Failed(err.to_string()));
                    }
                }
            }
            Some(Command::Play) => {
                if let Some(session) = &mut session
                    && let Err(err) = session.set_playing(true)
                {
                    let _ = events.unbounded_send(Event::Failed(err.to_string()));
                }
            }
            Some(Command::Pause) => {
                if let Some(session) = &mut session {
                    let _ = session.set_playing(false);
                }
            }
            Some(Command::Seek(seconds)) => {
                if let Some(active) = &mut session
                    && active.seek(seconds).is_err()
                {
                    session = None;
                    shared.playing.store(false, Ordering::Relaxed);
                    let _ = events.unbounded_send(Event::Ended);
                }
            }
            Some(Command::Stop) => {
                session = None;
                shared.playing.store(false, Ordering::Relaxed);
                shared.reset_clock(0.0);
            }
            None => {}
        }
        let Some(active) = &mut session else {
            continue;
        };
        if !active.playing {
            continue;
        }
        if let Err(err) = active.fill() {
            session = None;
            shared.playing.store(false, Ordering::Relaxed);
            let _ = events.unbounded_send(Event::Failed(err.to_string()));
        } else if active.finished() {
            session = None;
            shared.playing.store(false, Ordering::Relaxed);
            let _ = events.unbounded_send(Event::Ended);
        }
    }
}

type Ring = Arc<Mutex<VecDeque<f32>>>;

struct Session {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    duration: Option<f64>,
    stream: cpal::Stream,
    ring: Ring,
    capacity: usize,
    out_rate: u32,
    resampler: Option<(u32, FftFixedInOut<f32>)>,
    pending: [Vec<f32>; 2],
    resampled: Vec<Vec<f32>>,
    samples: Option<SampleBuffer<f32>>,
    shared: Arc<Shared>,
    playing: bool,
    eof: bool,
}

impl Session {
    fn open(path: &Path, shared: Arc<Shared>) -> Result<Self> {
        let file = File::open(path).context("Could not open the audio file")?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .context("Unsupported audio format")?;
        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| anyhow!("No audio track"))?;
        let params = &track.codec_params;
        let duration = params
            .n_frames
            .zip(params.sample_rate)
            .map(|(frames, rate)| frames as f64 / rate as f64);
        let decoder = symphonia::default::get_codecs()
            .make(params, &DecoderOptions::default())
            .context("Unsupported audio codec")?;
        let track_id = track.id;

        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| anyhow!("No audio output device"))?;
        let config = device.default_output_config()?;
        let out_rate = config.sample_rate();
        let channels = config.channels() as usize;
        let capacity = (out_rate as f32 * RING_SECONDS) as usize * 2;
        let ring: Ring = Arc::new(Mutex::new(VecDeque::with_capacity(capacity + 16_384)));
        shared.rate.store(out_rate, Ordering::Relaxed);
        let output = Output {
            ring: ring.clone(),
            shared: shared.clone(),
            channels,
        };
        macro_rules! stream {
            ($sample:ty) => {
                device.build_output_stream(
                    &config.into(),
                    move |data: &mut [$sample], _| output.write(data),
                    |err| tracing::warn!(%err, "music output stream error"),
                    None,
                )?
            };
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32),
            cpal::SampleFormat::I16 => stream!(i16),
            cpal::SampleFormat::I32 => stream!(i32),
            cpal::SampleFormat::U16 => stream!(u16),
            cpal::SampleFormat::F64 => stream!(f64),
            other => bail!("Unsupported output format {other}"),
        };
        Ok(Self {
            format,
            decoder,
            track_id,
            duration,
            stream,
            ring,
            capacity,
            out_rate,
            resampler: None,
            pending: [Vec::new(), Vec::new()],
            resampled: Vec::new(),
            samples: None,
            shared,
            playing: false,
            eof: false,
        })
    }

    fn set_playing(&mut self, playing: bool) -> Result<()> {
        if playing {
            self.fill()?;
            self.stream.play()?;
        } else {
            self.stream.pause()?;
        }
        self.playing = playing;
        self.shared.playing.store(playing, Ordering::Relaxed);
        Ok(())
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        let seconds = match self.duration {
            Some(duration) => seconds.min((duration - 0.25).max(0.0)),
            None => seconds,
        };
        let seeked = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(seconds),
                track_id: Some(self.track_id),
            },
        )?;
        self.decoder.reset();
        if let Some((_, resampler)) = &mut self.resampler {
            resampler.reset();
        }
        self.pending.iter_mut().for_each(Vec::clear);
        self.ring.lock().expect("music ring").clear();
        self.eof = false;
        let actual = self
            .format
            .tracks()
            .iter()
            .find(|track| track.id == self.track_id)
            .and_then(|track| track.codec_params.time_base)
            .map(|base| {
                let time = base.calc_time(seeked.actual_ts);
                time.seconds as f64 + time.frac
            })
            .unwrap_or(seconds);
        self.shared.reset_clock(actual);
        if self.playing {
            self.fill()?;
        }
        Ok(())
    }

    fn finished(&self) -> bool {
        self.eof && self.ring.lock().expect("music ring").is_empty()
    }

    fn fill(&mut self) -> Result<()> {
        while !self.eof && self.ring.lock().expect("music ring").len() < self.capacity {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(DecodeError::IoError(err))
                    if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.eof = true;
                    self.resample(true)?;
                    break;
                }
                Err(DecodeError::ResetRequired) => {
                    self.decoder.reset();
                    continue;
                }
                Err(err) => return Err(err.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(DecodeError::DecodeError(_)) => continue,
                Err(err) => return Err(err.into()),
            };
            let spec = *decoded.spec();
            let frames = decoded.frames();
            if frames == 0 {
                continue;
            }
            let samples = match &mut self.samples {
                Some(buffer) if buffer.capacity() >= decoded.capacity() * spec.channels.count() => {
                    buffer
                }
                slot => slot.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
            };
            samples.copy_interleaved_ref(decoded);
            let channels = spec.channels.count().max(1);
            for frame in samples.samples().chunks_exact(channels) {
                let left = frame[0];
                let right = if channels > 1 { frame[1] } else { left };
                self.pending[0].push(left);
                self.pending[1].push(right);
            }
            if self
                .resampler
                .as_ref()
                .is_none_or(|(rate, _)| *rate != spec.rate)
            {
                self.resampler = (spec.rate != self.out_rate)
                    .then(|| {
                        FftFixedInOut::new(
                            spec.rate as usize,
                            self.out_rate as usize,
                            RESAMPLE_CHUNK,
                            2,
                        )
                        .map(|resampler| (spec.rate, resampler))
                    })
                    .transpose()?;
                if let Some((_, resampler)) = &self.resampler {
                    self.resampled = resampler.output_buffer_allocate(true);
                }
            }
            self.resample(false)?;
        }
        Ok(())
    }

    fn resample(&mut self, flush: bool) -> Result<()> {
        let Some((_, resampler)) = &mut self.resampler else {
            let mut ring = self.ring.lock().expect("music ring");
            let [lefts, rights] = &mut self.pending;
            for (left, right) in lefts.drain(..).zip(rights.drain(..)) {
                ring.push_back(left);
                ring.push_back(right);
            }
            return Ok(());
        };
        loop {
            let needed = resampler.input_frames_next();
            let available = self.pending[0].len();
            if available < needed {
                if !flush || available == 0 {
                    return Ok(());
                }
                for channel in &mut self.pending {
                    channel.resize(needed, 0.0);
                }
            }
            let input = [&self.pending[0][..needed], &self.pending[1][..needed]];
            let (_, written) = resampler.process_into_buffer(&input, &mut self.resampled, None)?;
            {
                let mut ring = self.ring.lock().expect("music ring");
                for ix in 0..written {
                    ring.push_back(self.resampled[0][ix]);
                    ring.push_back(self.resampled[1][ix]);
                }
            }
            for channel in &mut self.pending {
                channel.drain(..needed);
            }
        }
    }
}

struct Output {
    ring: Ring,
    shared: Arc<Shared>,
    channels: usize,
}

impl Output {
    fn write<T: cpal::SizedSample + cpal::FromSample<f32>>(&self, data: &mut [T]) {
        let volume = f32::from_bits(self.shared.volume.load(Ordering::Relaxed));
        let gain = volume * volume;
        let mut ring = self.ring.try_lock().ok();
        let mut played = 0u64;
        for frame in data.chunks_mut(self.channels.max(1)) {
            let (left, right) = match ring.as_mut().filter(|ring| ring.len() >= 2) {
                Some(ring) => {
                    played += 1;
                    (
                        ring.pop_front().unwrap_or(0.0),
                        ring.pop_front().unwrap_or(0.0),
                    )
                }
                None => (0.0, 0.0),
            };
            match frame {
                [mono] => *mono = T::from_sample((left + right) * 0.5 * gain),
                [l, r, rest @ ..] => {
                    *l = T::from_sample(left * gain);
                    *r = T::from_sample(right * gain);
                    rest.iter_mut()
                        .for_each(|sample| *sample = T::from_sample(0.0));
                }
                [] => {}
            }
        }
        self.shared
            .played_frames
            .fetch_add(played, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tone(path: &Path, rate: u32, seconds: f32) {
        let frames = (rate as f32 * seconds) as u32;
        let mut data = Vec::with_capacity(44 + frames as usize * 4);
        let byte_rate = rate * 4;
        data.extend_from_slice(b"RIFF");
        data.extend_from_slice(&(36 + frames * 4).to_le_bytes());
        data.extend_from_slice(b"WAVEfmt ");
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&rate.to_le_bytes());
        data.extend_from_slice(&byte_rate.to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes());
        data.extend_from_slice(&16u16.to_le_bytes());
        data.extend_from_slice(b"data");
        data.extend_from_slice(&(frames * 4).to_le_bytes());
        for ix in 0..frames {
            let sample =
                ((ix as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 8_000.0) as i16;
            data.extend_from_slice(&sample.to_le_bytes());
            data.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn output_mixes_down_to_mono_and_counts_only_real_frames() {
        let shared = Arc::new(Shared {
            volume: AtomicU32::new(1.0f32.to_bits()),
            played_frames: AtomicU64::new(0),
            rate: AtomicU32::new(48_000),
            base_seconds: AtomicU64::new(0f64.to_bits()),
            duration: AtomicU64::new(f64::NAN.to_bits()),
            playing: AtomicBool::new(true),
        });
        let ring: Ring = Arc::new(Mutex::new(VecDeque::from(vec![0.5, 0.25, 0.1, 0.3])));
        let output = Output {
            ring,
            shared: shared.clone(),
            channels: 1,
        };
        let mut data = [1.0f32; 3];
        output.write(&mut data);
        assert_eq!(data, [0.375, 0.2, 0.0]);
        assert_eq!(shared.played_frames.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn decoding_fills_the_ring_at_the_device_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        write_tone(&path, 44_100, 0.5);
        let shared = Arc::new(Shared {
            volume: AtomicU32::new(1.0f32.to_bits()),
            played_frames: AtomicU64::new(0),
            rate: AtomicU32::new(0),
            base_seconds: AtomicU64::new(0f64.to_bits()),
            duration: AtomicU64::new(f64::NAN.to_bits()),
            playing: AtomicBool::new(false),
        });
        let Ok(mut session) = Session::open(&path, shared) else {
            return;
        };
        assert!((session.duration.unwrap() - 0.5).abs() < 0.01);
        session.fill().unwrap();
        let expected = session.out_rate as f64 * 0.5 * 2.0;
        let got = session.ring.lock().unwrap().len() as f64;
        assert!(session.eof);
        assert!(
            (got - expected).abs() / expected < 0.05,
            "{got} vs {expected}"
        );
    }
}
