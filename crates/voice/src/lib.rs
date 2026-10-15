//! Desktop-local Parakeet v3. No engine, document, RPC or audio persistence.
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parakeet_rs::{ParakeetTDT, Transcriber};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

mod resample;
pub mod spectrum;

pub const MAX_SECONDS: usize = 60;
/// Where the speech model lives under a data directory, on every device.
pub const MODEL_DIR: &str = "models/parakeet-tdt-0.6b-v3-int8";
#[derive(serde::Deserialize)]
pub struct Artifact {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(serde::Deserialize)]
pub struct Manifest {
    pub repository: String,
    pub revision: String,
    pub files: Vec<Artifact>,
}
pub fn manifest() -> Manifest {
    serde_json::from_str(include_str!("../model.json")).expect("pinned model manifest")
}
pub fn download_size() -> u64 {
    manifest().files.iter().map(|f| f.size).sum()
}
pub fn installed(dir: &Path) -> bool {
    manifest()
        .files
        .iter()
        .all(|f| std::fs::metadata(dir.join(&f.name)).is_ok_and(|m| m.len() == f.size))
        && std::fs::read_to_string(dir.join("verified")).is_ok_and(|r| r == manifest().revision)
}

/// Called only on a worker. Temporary files never establish readiness.
pub fn download(dir: &Path, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> Result<()> {
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async {
        std::fs::create_dir_all(dir)?;
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(20)).build()?;
        let m=manifest(); let mut total=0;
        for file in m.files {
            if cancel.load(Ordering::Acquire) { bail!("Download cancelled") }
            let staging=dir.join(format!("{}.part",file.name));
            let result=async {
                let mut response=tokio::select! {
                    _=cancelled(cancel)=>bail!("Download cancelled"),
                    response=tokio::time::timeout(std::time::Duration::from_secs(30),client.get(format!("https://huggingface.co/{}/resolve/{}/{}",m.repository,m.revision,file.name)).send())=>response??.error_for_status()?,
                };
                let mut out=std::fs::File::create(&staging)?;let mut hash=Sha256::new();let mut size=0;
                loop {
                    let chunk=tokio::select! {
                        _=cancelled(cancel)=>bail!("Download cancelled"),
                        chunk=tokio::time::timeout(std::time::Duration::from_secs(30),response.chunk())=>chunk??,
                    };
                    let Some(chunk)=chunk else {break};
                    size+=chunk.len() as u64;if size>file.size {bail!("Unexpected model size")}
                    hash.update(&chunk);out.write_all(&chunk)?;total+=chunk.len() as u64;progress(total);
                }
                if size!=file.size || format!("{:x}",hash.finalize())!=file.sha256 {bail!("Model checksum verification failed")}
                out.sync_all()?;std::fs::rename(&staging,dir.join(&file.name))?;Ok::<_,anyhow::Error>(())
            }.await;
            if result.is_err() {let _=std::fs::remove_file(staging);} result?;
        }
        if cancel.load(Ordering::Acquire) {bail!("Download cancelled")}
        std::fs::write(dir.join("verified"),manifest().revision)?;Ok(())
    })
}
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

pub struct Recognizer(ParakeetTDT);
impl Recognizer {
    pub fn load(dir: &Path) -> Result<Self> {
        // Verify before handing bytes to the native runtime, including after restart.
        for f in manifest().files {
            let mut input = std::fs::File::open(dir.join(&f.name))?;
            let mut hash = Sha256::new();
            std::io::copy(&mut input, &mut hash)?;
            if format!("{:x}", hash.finalize()) != f.sha256 {
                bail!("Model is damaged. Remove it in Settings and download again.")
            }
        }
        Ok(Self(ParakeetTDT::from_pretrained(dir, None).map_err(
            |_| anyhow::anyhow!("Could not load Parakeet v3"),
        )?))
    }
    pub fn transcribe(&mut self, samples: Vec<f32>, rate: u32) -> Result<String> {
        if !(8_000..=192_000).contains(&rate) {
            bail!("Unsupported microphone sample rate")
        }
        if samples.len() > rate as usize * MAX_SECONDS {
            bail!("Recording exceeds one minute")
        }
        if samples.len() < rate as usize / 5 || samples.iter().all(|s| s.abs() < 0.0001) {
            return Ok(String::new());
        }
        let samples = resample::for_model(samples, rate)?;
        self.0
            .transcribe_samples(samples, resample::MODEL_RATE, 1, None)
            .map(|r| r.text)
            .map_err(|_| anyhow::anyhow!("Could not transcribe this recording"))
    }
}

struct Audio {
    /// Only the audio callback touches this until the stream is dropped.
    samples: Mutex<Vec<f32>>,
    // Outside the lock: the capture loop polls these every 10 ms and must
    // never make the real-time callback's `try_lock` drop a buffer.
    failed: AtomicBool,
    full: AtomicBool,
}
impl Audio {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(capacity)),
            failed: AtomicBool::new(false),
            full: AtomicBool::new(false),
        }
    }
}
/// Loudest RMS since the UI last read it. Non-negative `f32` bit patterns sort
/// like their values, so `fetch_max` keeps the peak without a lock.
fn meter<T: cpal::Sample>(data: &[T], channels: usize, level: &AtomicU32)
where
    f32: cpal::FromSample<T>,
{
    let frames = data.len() / channels.max(1);
    if frames == 0 {
        return;
    }
    let energy = data
        .chunks_exact(channels)
        .map(|frame| {
            let s = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32;
            s * s
        })
        .sum::<f32>();
    let rms = (energy / frames as f32).sqrt();
    if rms.is_finite() {
        level.fetch_max(rms.to_bits(), Ordering::Relaxed);
    }
}
fn append<T: cpal::Sample>(data: &[T], channels: usize, rate: u32, a: &Audio)
where
    f32: cpal::FromSample<T>,
{
    if let Ok(mut samples) = a.samples.try_lock() {
        for frame in data.chunks_exact(channels) {
            if samples.len() >= rate as usize * MAX_SECONDS {
                a.full.store(true, Ordering::Release);
                break;
            }
            samples.push(frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32);
        }
    }
}
/// Audio arriving from elsewhere, already mono at `rate`: a host takes a
/// phone's dictation through one of these in place of a microphone. Samples
/// are kept in memory for the length of the dictation and nowhere else.
pub struct Feed {
    audio: Arc<Audio>,
    rate: u32,
    ended: AtomicBool,
    level: AtomicU32,
}
impl Feed {
    pub fn new(rate: u32) -> Arc<Self> {
        Arc::new(Self {
            audio: Arc::new(Audio::with_capacity(rate as usize * MAX_SECONDS)),
            rate,
            ended: AtomicBool::new(false),
            level: AtomicU32::new(0),
        })
    }
    /// Appends PCM16LE samples. Past a minute the rest is dropped and the
    /// feed ends, as a microphone's capture does.
    pub fn push(&self, pcm16le: &[u8]) {
        if self.ended.load(Ordering::Acquire) {
            return;
        }
        let samples: Vec<i16> = pcm16le
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        meter(&samples, 1, &self.level);
        append(&samples, 1, self.rate, &self.audio);
    }
    /// Peak RMS since last read, 0–1.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
    /// No more audio: what was pushed is transcribed.
    pub fn end(&self) {
        self.ended.store(true, Ordering::Release);
    }
}

/// A microphone the user can choose: a stable identifier and a display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub id: String,
    pub name: String,
}

/// Every input device the default host reports. Enumeration can block on the
/// audio server, so call it off the UI thread.
pub fn input_devices() -> Vec<InputDevice> {
    let Ok(devices) = cpal::default_host().input_devices() else {
        return Vec::new();
    };
    let mut devices: Vec<_> = devices
        .filter_map(|device| {
            Some(InputDevice {
                id: device.id().ok()?.to_string(),
                name: device.description().ok()?.name().to_owned(),
            })
        })
        .collect();
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

/// The id of the device the system currently records from by default.
pub fn default_input_device() -> Option<String> {
    Some(
        cpal::default_host()
            .default_input_device()?
            .id()
            .ok()?
            .to_string(),
    )
}

/// The chosen device when it is connected, the system default otherwise.
fn input_device(id: Option<&str>) -> Option<cpal::Device> {
    let host = cpal::default_host();
    id.and_then(|id| id.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
        .or_else(|| host.default_input_device())
}

pub struct Capture {
    stream: Option<cpal::Stream>,
    audio: Arc<Audio>,
    rate: u32,
}
impl Capture {
    pub fn start(device: Option<&str>, level: Arc<AtomicU32>) -> Result<Self> {
        let device = input_device(device).context("No microphone available")?;
        let config = device.default_input_config()?;
        let rate = config.sample_rate();
        let channels = config.channels() as usize;
        let audio = Arc::new(Audio::with_capacity(rate as usize * MAX_SECONDS));
        let a = audio.clone();
        let e = audio.clone();
        let err = move |_| e.failed.store(true, Ordering::Release);
        // Preserve the device's native configuration; convert every CPAL
        // sample representation through the same bounded mono callback.
        macro_rules! stream {
            ($sample:ty) => {
                device.build_input_stream(
                    &config.into(),
                    move |d: &[$sample], _| {
                        meter(d, channels, &level);
                        append(d, channels, rate, &a)
                    },
                    err,
                    None,
                )?
            };
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
        stream.play()?;
        Ok(Self {
            stream: Some(stream),
            audio,
            rate,
        })
    }
    pub fn ended(&self) -> bool {
        self.audio.full.load(Ordering::Acquire) || self.audio.failed.load(Ordering::Acquire)
    }
    pub fn finish(mut self) -> Result<(Vec<f32>, u32)> {
        self.stream.take();
        if self.audio.failed.load(Ordering::Acquire) {
            bail!("Microphone disconnected. Your draft is safe.")
        }
        let mut samples = self.audio.samples.lock().unwrap_or_else(|e| e.into_inner());
        Ok((std::mem::take(&mut *samples), self.rate))
    }
}

#[derive(Debug)]
pub enum Event {
    Listening,
    /// Everything said so far, read again while the user is still speaking.
    Partial(String),
    Finalizing,
    Final(String),
    Failed(String),
}

/// A live transcript is read again this often at most; a longer recording
/// that takes longer to read waits twice that long, so it never takes over
/// the processor.
const PARTIAL_EVERY: std::time::Duration = std::time::Duration::from_millis(800);
/// Too little audio to be worth reading yet.
const PARTIAL_FROM: f32 = 0.6;
struct Job {
    dir: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    source: Source,
    events: std::sync::mpsc::SyncSender<Event>,
    /// The audio as it is being captured and its rate, while it is, so the
    /// worker can read a live transcript.
    live: Mutex<Option<(Arc<Audio>, u32)>>,
}
// Capture owns its stream on a separate thread. Model loading and inference
// remain serialized on WORKER, including after the user cancels native work.
trait Recording {
    fn ended(&self) -> bool;
    fn finish(self) -> Result<(Vec<f32>, u32)>;
    /// The audio as it is being captured, for a live transcript.
    fn tap(&self) -> Option<(Arc<Audio>, u32)> {
        None
    }
}
impl Recording for Capture {
    fn ended(&self) -> bool {
        self.ended()
    }
    fn tap(&self) -> Option<(Arc<Audio>, u32)> {
        Some((self.audio.clone(), self.rate))
    }
    fn finish(self) -> Result<(Vec<f32>, u32)> {
        self.finish()
    }
}
impl Recording for Arc<Feed> {
    fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire) || self.audio.full.load(Ordering::Acquire)
    }
    fn tap(&self) -> Option<(Arc<Audio>, u32)> {
        Some((self.audio.clone(), self.rate))
    }
    fn finish(self) -> Result<(Vec<f32>, u32)> {
        let mut samples = self.audio.samples.lock().unwrap_or_else(|e| e.into_inner());
        Ok((std::mem::take(&mut *samples), self.rate))
    }
}
/// Where a job's audio comes from.
enum Source {
    Microphone(Option<String>),
    Feed(Arc<Feed>),
}
enum Input {
    Microphone(Capture),
    Feed(Arc<Feed>),
}
impl Recording for Input {
    fn ended(&self) -> bool {
        match self {
            Self::Microphone(c) => c.ended(),
            Self::Feed(f) => Recording::ended(f),
        }
    }
    fn tap(&self) -> Option<(Arc<Audio>, u32)> {
        match self {
            Self::Microphone(c) => Recording::tap(c),
            Self::Feed(f) => Recording::tap(f),
        }
    }
    fn finish(self) -> Result<(Vec<f32>, u32)> {
        match self {
            Self::Microphone(c) => Recording::finish(c),
            Self::Feed(f) => Recording::finish(f),
        }
    }
}

fn record<C: Recording>(
    job: &Job,
    start: impl FnOnce() -> Result<C>,
) -> Result<Option<(Vec<f32>, u32)>> {
    if job.cancel.load(Ordering::Acquire) || job.stop.load(Ordering::Acquire) {
        return Ok(None);
    }
    let capture = start()?;
    *job.live.lock().unwrap_or_else(|e| e.into_inner()) = capture.tap();
    // Stop/cancel may arrive while the audio device is opening.
    if job.cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    if !job.stop.load(Ordering::Acquire) {
        let _ = job.events.try_send(Event::Listening);
    }
    let started = std::time::Instant::now();
    while !job.stop.load(Ordering::Acquire)
        && !job.cancel.load(Ordering::Acquire)
        && !capture.ended()
        && started.elapsed().as_secs() < MAX_SECONDS as u64
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    job.live.lock().unwrap_or_else(|e| e.into_inner()).take();
    if job.cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    let audio = capture.finish()?; // Close the microphone before waiting for the model.
    let _ = job.events.try_send(Event::Finalizing);
    Ok(Some(audio))
}

fn run_job<C, F>(
    job: &Job,
    start: impl FnOnce() -> Result<C> + Send,
    load: impl FnOnce() -> Result<F>,
) -> Result<Option<String>>
where
    C: Recording,
    F: FnMut(Vec<f32>, u32) -> Result<String>,
{
    if job.cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    std::thread::scope(|scope| {
        let capture = std::thread::Builder::new()
            .name("dictation-capture".into())
            .spawn_scoped(scope, || record(job, start))?;
        // If native loading unwinds, scope teardown still has to close capture
        // before joining it. Normal cancellation uses the same signal.
        struct CancelOnDrop<'a>(&'a AtomicBool);
        impl Drop for CancelOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let _close_capture = CancelOnDrop(&job.cancel);
        let mut model = load();
        // A load failure must stop capture even if the user has not pressed Stop.
        // Keep the admission guard until the stream has actually been dropped.
        if model.is_err() {
            job.cancel.store(true, Ordering::Release);
        }
        if let Ok(transcribe) = model.as_mut() {
            live_transcript(job, &capture, transcribe);
        }
        let audio = capture
            .join()
            .map_err(|_| anyhow::anyhow!("Microphone worker failed"))?;
        let mut transcribe = model?;
        if job.cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let Some((samples, rate)) = audio? else {
            return Ok(Some(String::new()));
        };
        let text = transcribe(samples, rate)?;
        Ok((!job.cancel.load(Ordering::Acquire)).then_some(text))
    })
}

/// While the user speaks, reads what has been captured so far again and
/// again, one reading at a time, and sends each new reading as a partial.
/// The samples are copied in small steps, only what is new, so the capture
/// callback (which never waits for the lock) loses nothing.
fn live_transcript<T>(
    job: &Job,
    capture: &std::thread::ScopedJoinHandle<'_, T>,
    transcribe: &mut impl FnMut(Vec<f32>, u32) -> Result<String>,
) {
    let mut heard: Vec<f32> = Vec::new();
    let mut said = String::new();
    let mut next = std::time::Instant::now() + PARTIAL_EVERY;
    while !capture.is_finished() {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let live = job.live.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some((audio, rate)) = live else {
            continue;
        };
        if std::time::Instant::now() < next
            || job.stop.load(Ordering::Acquire)
            || job.cancel.load(Ordering::Acquire)
        {
            continue;
        }
        if let Ok(samples) = audio.samples.try_lock() {
            heard.extend_from_slice(&samples[heard.len().min(samples.len())..]);
        }
        if (heard.len() as f32) < rate as f32 * PARTIAL_FROM {
            continue;
        }
        let started = std::time::Instant::now();
        if let Ok(text) = transcribe(heard.clone(), rate) {
            let text = text.trim();
            if !text.is_empty() && text != said {
                said = text.to_owned();
                let _ = job.events.try_send(Event::Partial(said.clone()));
            }
        }
        next = std::time::Instant::now() + PARTIAL_EVERY.max(started.elapsed() * 2);
    }
}

static BUSY: AtomicBool = AtomicBool::new(false);
// The command owns admission through capture teardown and uninterruptible
// native work. Queue failure also releases it, so rapid retries cannot wedge it.
struct BusyGuard;
impl BusyGuard {
    fn acquire() -> Result<Self> {
        if BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            bail!("Dictation is still active or finishing. Wait a moment, then try again.")
        }
        Ok(Self)
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
enum Command {
    Transcribe(Job, BusyGuard),
    Unload,
}
static WORKER: std::sync::OnceLock<std::sync::mpsc::SyncSender<Command>> =
    std::sync::OnceLock::new();
pub struct Session {
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    events: std::sync::mpsc::Receiver<Event>,
}
impl Session {
    /// `device` is an [`InputDevice::id`]; `None` or a disconnected device
    /// records from the system default.
    pub fn start(dir: std::path::PathBuf, device: Option<String>) -> Result<Self> {
        Self::begin(dir, Source::Microphone(device))
    }
    /// Transcribes `feed` instead of a microphone: a dictation arriving from
    /// a phone. Finish by ending the feed (or [`Session::finish`]).
    pub fn start_fed(dir: std::path::PathBuf, feed: Arc<Feed>) -> Result<Self> {
        Self::begin(dir, Source::Feed(feed))
    }
    fn begin(dir: std::path::PathBuf, source: Source) -> Result<Self> {
        let admission = BusyGuard::acquire()?;
        let sender = WORKER.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Command>(1);
            std::thread::spawn(move || {
                let mut cached: Option<(std::path::PathBuf, Recognizer)> = None;
                loop {
                    let (job, _admission) = match rx.recv_timeout(std::time::Duration::from_secs(30)) {
                        Ok(Command::Transcribe(j, admission)) => (j, admission),
                        Ok(Command::Unload) => {
                            cached = None;
                            continue;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            cached = None;
                            continue;
                        }
                        Err(_) => break,
                    };
                    // A native panic must not take down the worker, and with
                        // it every later session, until the app restarts.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(
                        &job,
                        || match &job.source {
                            Source::Microphone(device) => {
                                Capture::start(device.as_deref(), job.level.clone())
                                    .map(Input::Microphone)
                            }
                            Source::Feed(feed) => Ok(Input::Feed(feed.clone())),
                        },
                        || {
                            if cached.as_ref().is_none_or(|(path, _)| *path != job.dir) {
                                let model = Recognizer::load(&job.dir).context(
                                    "Could not load the model. Remove it in Settings and download again.",
                                )?;
                                cached = Some((job.dir.clone(), model));
                            }
                            let model = &mut cached.as_mut().unwrap().1;
                            Ok(move |samples, rate| model.transcribe(samples, rate))
                        },
                    )))
                    .unwrap_or_else(|_| {
                        cached = None;
                        Err(anyhow::anyhow!("Dictation stopped unexpectedly. Your draft is safe."))
                    });
                    match result {
                        Ok(Some(text)) => {
                            let _ = job.events.try_send(Event::Final(text));
                        }
                        Ok(None) => {}
                        // Keep loaded weights: a microphone or recording error says
                        // nothing about the model, and reloading takes seconds.
                        Err(e) => {
                            let _ = job.events.try_send(Event::Failed(e.to_string()));
                        }
                    }
                }
            });
            tx
        });
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let (tx, events) = std::sync::mpsc::sync_channel(4);
        if sender
            .try_send(Command::Transcribe(
                Job {
                    dir,
                    stop: stop.clone(),
                    cancel: cancel.clone(),
                    level: level.clone(),
                    source,
                    events: tx,
                    live: Default::default(),
                },
                admission,
            ))
            .is_err()
        {
            bail!("Dictation worker unavailable")
        }
        Ok(Self {
            stop,
            cancel,
            level,
            events,
        })
    }
    pub fn poll(&mut self) -> Option<Event> {
        self.events.try_recv().ok()
    }
    pub fn finish(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
    /// Peak microphone RMS (0–1) since the previous call. Level only; the
    /// samples themselves stay on the worker.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}
pub fn busy() -> bool {
    BUSY.load(Ordering::Acquire)
}

/// Retire cached weights on the worker, never on the UI thread.
pub fn unload() {
    if let Some(worker) = WORKER.get() {
        let _ = worker.try_send(Command::Unload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::Sample;
    #[test]
    fn native_sample_formats_preserve_levels_and_downmix_channels() {
        fn check<T: cpal::Sample + cpal::FromSample<f32>>()
        where
            f32: cpal::FromSample<T>,
        {
            let audio = Audio::with_capacity(0);
            let input: Vec<T> = [-0.5_f32, 0.5, 0.25, 0.75, 0.0, 0.0]
                .into_iter()
                .map(|s| s.to_sample::<T>())
                .collect();
            append(&input, 2, 16_000, &audio);
            assert_eq!(*audio.samples.lock().unwrap(), vec![0.0, 0.5, 0.0]);
            let level = AtomicU32::new(0);
            meter(&input, 2, &level);
            let rms = f32::from_bits(level.load(Ordering::Relaxed));
            assert!((rms - (0.25_f32 / 3.0).sqrt()).abs() < 0.01, "{rms}");
            meter(&input[4..], 2, &level);
            assert_eq!(f32::from_bits(level.load(Ordering::Relaxed)), rms);
        }
        check::<i8>();
        check::<i16>();
        check::<cpal::I24>();
        check::<i32>();
        check::<i64>();
        check::<u8>();
        check::<u16>();
        check::<cpal::U24>();
        check::<u32>();
        check::<u64>();
        check::<f32>();
        check::<f64>();
    }
    #[test]
    fn manifest_pins_only_v3_artifacts_and_exact_size() {
        let m = manifest();
        assert_eq!(m.revision.len(), 40);
        assert_eq!(m.files.len(), 3);
        assert_eq!(download_size(), 670479942);
        assert!(
            m.files
                .iter()
                .all(|f| f.sha256.len() == 64 && !f.name.contains('/'))
        );
    }
    #[test]
    fn cancelled_download_never_establishes_readiness() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            download(dir.path(), &AtomicBool::new(true), |_| panic!(
                "no progress after cancellation"
            ))
            .is_err()
        );
        assert!(!installed(dir.path()));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[test]
    fn partial_or_corrupt_model_cannot_load() {
        let dir = tempfile::tempdir().unwrap();
        let f = &manifest().files[0];
        std::fs::write(dir.path().join(&f.name), b"corrupt").unwrap();
        assert!(!installed(dir.path()));
        assert!(Recognizer::load(dir.path()).is_err());
    }
}

#[cfg(test)]
mod session_tests;
