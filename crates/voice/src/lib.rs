//! Desktop-local speech recognition (transcribe.cpp, one model from
//! `models.json`). No engine, document, RPC or audio persistence.
use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

mod resample;

pub const MAX_SECONDS: usize = 60;
#[derive(serde::Deserialize, Clone)]
pub struct Artifact {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    /// "language": the language-ID model a model that can't tell the
    /// language itself is given. (None: the weights.)
    #[serde(default)]
    pub role: Option<String>,
    /// Where it comes from when not the model's own repository.
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub revision: Option<String>,
}
/// One model Zeron can download: pinned bytes, where they come from, what they speak.
#[derive(serde::Deserialize, Clone)]
pub struct Manifest {
    /// Its folder under `<data>/models`.
    pub id: String,
    pub name: String,
    pub repository: String,
    pub revision: String,
    pub source: String,
    pub license: String,
    pub languages: Vec<String>,
    /// The weights (the first file) and what they need beside them.
    pub files: Vec<Artifact>,
    /// Its makers' marks, as Settings shows them (ds.Mark names): the base
    /// model's maker first.
    #[serde(default)]
    pub marks: Vec<String>,
    /// About what it holds in memory while loaded (measured on the CPU).
    #[serde(default)]
    pub memory: u64,
    /// Much slower where it runs on the CPU (Linux, Windows).
    #[serde(default)]
    pub cpu_slow: bool,
}
/// The models on offer, the default first.
pub fn models() -> Vec<Manifest> {
    serde_json::from_str(include_str!("../models.json")).expect("pinned model list")
}
/// The default model.
pub fn manifest() -> Manifest {
    models().swap_remove(0)
}
/// The model a folder holds: the one its name is the id of (each lives in
/// `<data>/models/<id>`). None for any other folder.
pub fn model_for(dir: &Path) -> Option<Manifest> {
    let id = dir.file_name().and_then(|n| n.to_str())?;
    models().into_iter().find(|m| m.id == id)
}
pub fn download_size() -> u64 {
    manifest().size()
}
pub fn installed(dir: &Path) -> bool {
    model_for(dir).is_some_and(|m| m.installed(dir))
}
pub fn download(dir: &Path, cancel: &AtomicBool, progress: impl FnMut(u64)) -> Result<()> {
    model_for(dir)
        .context("No such speech model")?
        .download(dir, cancel, progress)
}

/// Whether this computer's processor runs the models: on x86-64 they're
/// built for AVX2, FMA, F16C and BMI2 (most since 2013, not every Celeron or
/// Pentium: Gemini Lake and Jasper Lake have none of them), and would crash
/// the app without them.
pub fn cpu_supported() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("fma")
            && std::arch::is_x86_feature_detected!("f16c")
            && std::arch::is_x86_feature_detected!("bmi2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    true
}
pub const CPU_UNSUPPORTED: &str =
    "This computer's processor can't run speech models (they need AVX2).";

/// Models Zeron no longer offers, by folder.
const RETIRED: &[&str] = &["parakeet-tdt-0.6b-v3-int8"];
/// Frees what retired models take in `models` (Parakeet v3: 670 MB). Whoever
/// had one is offered the current model on their next dictation, as anyone is.
pub fn retire_old_models(models: &Path) {
    for id in RETIRED {
        let dir = models.join(id);
        if dir.exists() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Manifest {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    /// Downloaded and verified: each file was hashed as it arrived, and
    /// `verified` names the revision they matched.
    pub fn installed(&self, dir: &Path) -> bool {
        self.files
            .iter()
            .all(|f| std::fs::metadata(dir.join(&f.name)).is_ok_and(|m| m.len() == f.size))
            && std::fs::read_to_string(dir.join("verified")).is_ok_and(|r| r == self.marker())
    }
    /// What `verified` says once every file matched its pin: each file's
    /// SHA-256 and name, so a pin changed on any of them verifies again.
    pub fn marker(&self) -> String {
        self.files
            .iter()
            .map(|f| format!("{}  {}\n", f.sha256, f.name))
            .collect()
    }
    /// Called only on a worker. Temporary files never establish readiness.
    /// A file already here that matches its pin is kept (an earlier attempt
    /// stopped after it); a cancelled download leaves nothing behind.
    pub fn download(
        &self,
        dir: &Path,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64),
    ) -> Result<()> {
        let result = self.fetch(dir, cancel, &mut progress);
        if cancel.load(Ordering::Acquire) {
            let _ = std::fs::remove_dir_all(dir);
            bail!("Download cancelled")
        }
        result
    }
    fn fetch(&self, dir: &Path, cancel: &AtomicBool, progress: &mut impl FnMut(u64)) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::remove_file(dir.join("verified"));
        let mut total = 0;
        let mut wanted = Vec::new();
        for file in &self.files {
            let path = dir.join(&file.name);
            if std::fs::metadata(&path).is_ok_and(|m| m.len() == file.size) {
                let mut hash = Sha256::new();
                std::io::copy(&mut std::fs::File::open(&path)?, &mut hash)?;
                if format!("{:x}", hash.finalize()) == file.sha256 {
                    total += file.size;
                    progress(total);
                    continue;
                }
            }
            let _ = std::fs::remove_file(&path);
            wanted.push(file);
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async {
                let client = reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(20))
                    .build()?;
                for file in wanted {
                    if cancel.load(Ordering::Acquire) {
                        bail!("Download cancelled")
                    }
                    let staging = dir.join(format!("{}.part", file.name));
                    let url = format!(
                        "https://huggingface.co/{}/resolve/{}/{}",
                        file.repository.as_ref().unwrap_or(&self.repository),
                        file.revision.as_ref().unwrap_or(&self.revision),
                        file.name
                    );
                    let result = async {
                        let mut response = tokio::select! {
                            _ = cancelled(cancel) => bail!("Download cancelled"),
                            response = tokio::time::timeout(Duration::from_secs(30), client.get(&url).send()) =>
                                response??.error_for_status()?,
                        };
                        let mut out = std::fs::File::create(&staging)?;
                        let mut hash = Sha256::new();
                        let mut size = 0;
                        loop {
                            let chunk = tokio::select! {
                                _ = cancelled(cancel) => bail!("Download cancelled"),
                                chunk = tokio::time::timeout(Duration::from_secs(30), response.chunk()) => chunk??,
                            };
                            let Some(chunk) = chunk else { break };
                            size += chunk.len() as u64;
                            if size > file.size {
                                bail!("Unexpected model size")
                            }
                            hash.update(&chunk);
                            out.write_all(&chunk)?;
                            total += chunk.len() as u64;
                            progress(total);
                        }
                        if size != file.size || format!("{:x}", hash.finalize()) != file.sha256 {
                            bail!("Model checksum verification failed")
                        }
                        out.sync_all()?;
                        std::fs::rename(&staging, dir.join(&file.name))?;
                        Ok::<_, anyhow::Error>(())
                    }
                    .await;
                    if result.is_err() {
                        let _ = std::fs::remove_file(staging);
                    }
                    result?;
                }
                Ok::<_, anyhow::Error>(())
            })?;
        if cancel.load(Ordering::Acquire) {
            bail!("Download cancelled")
        }
        std::fs::write(dir.join("verified"), self.marker())?;
        Ok(())
    }
}
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
}

pub struct Recognizer {
    session: transcribe_cpp::Session,
    /// For a model that can't tell the language itself: who tells it, the
    /// model's languages, and the person's among them, the likeliest first.
    language: Option<(transcribe_cpp::LangIdSession, Vec<String>, Vec<String>)>,
}
impl Recognizer {
    pub fn load(dir: &Path) -> Result<Self> {
        if !cpu_supported() {
            bail!(CPU_UNSUPPORTED)
        }
        let m = model_for(dir).context("No such speech model")?;
        // Hashed once, at download: a load reads only what it needs.
        if !m.installed(dir) {
            bail!("Model is damaged. Remove it in Settings and download again.")
        }
        static LOGS: std::sync::Once = std::sync::Once::new();
        LOGS.call_once(transcribe_cpp::init_logging);
        let failed = |e: transcribe_cpp::Error| anyhow::anyhow!("Could not load {}: {e}", m.name);
        let session = transcribe_cpp::Model::load(dir.join(&m.files[0].name))
            .and_then(|model| model.session())
            .map_err(failed)?;
        let language = match m
            .files
            .iter()
            .find(|f| f.role.as_deref() == Some("language"))
        {
            Some(f) => Some((
                transcribe_cpp::Model::load(dir.join(&f.name))
                    .and_then(|model| model.langid_session())
                    .map_err(failed)?,
                m.languages.clone(),
                spoken(sys_locale::get_locales(), &m.languages),
            )),
            None => None,
        };
        Ok(Self { session, language })
    }
    pub fn transcribe(&mut self, samples: Vec<f32>, rate: u32) -> Result<String> {
        if !(8_000..=192_000).contains(&rate) {
            bail!("Unsupported microphone sample rate")
        }
        if samples.len() > rate as usize * MAX_SECONDS {
            bail!("Recording exceeds one minute")
        }
        if samples.len() < rate as usize / 5 || samples.iter().all(|s| s.abs() < SILENT) {
            return Ok(String::new());
        }
        let samples = resample::for_model(samples, rate)?;
        let language = self.language(&samples);
        self.session
            .run(
                &samples,
                &transcribe_cpp::RunOptions {
                    language,
                    ..Default::default()
                },
            )
            .map(|r| r.text.trim().to_owned())
            .map_err(|_| anyhow::anyhow!("Could not transcribe this recording"))
    }
    /// The language of the whole recording: what language ID hears among
    /// all the model speaks, when it's sure; else the likeliest of the
    /// person's own (their system's that the model speaks, in its order);
    /// else the first of those. (None: the model tells by itself.)
    fn language(&mut self, pcm: &[f32]) -> Option<String> {
        let (detect, all, mine) = self.language.as_mut()?;
        let all = transcribe_cpp::LangIdOptions {
            allowed: Some(all.clone()),
        };
        if let Ok(heard) = detect.run(pcm, &all) {
            if let Some(top) = heard.candidates.first()
                && top.p >= SURE
                && heard.allowed_mass >= 0.5
            {
                return Some(top.code.clone());
            }
            if let Some(theirs) = heard.candidates.iter().find(|c| mine.contains(&c.code)) {
                return Some(theirs.code.clone());
            }
        }
        mine.first().cloned()
    }
    #[cfg(test)]
    fn speaks(&mut self, languages: &[&str]) {
        if let Some((_, _, mine)) = &mut self.language {
            *mine = languages.iter().map(|l| l.to_string()).collect();
        }
    }
}

/// The languages of `locales` (BCP 47, preferred first) that `supported`
/// has, once each; English when none.
fn spoken(locales: impl IntoIterator<Item = String>, supported: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for locale in locales {
        let code = locale
            .split(['-', '_', '.'])
            .next()
            .unwrap_or("")
            .to_lowercase();
        if supported.contains(&code) && !out.contains(&code) {
            out.push(code);
        }
    }
    if out.is_empty() {
        out.push("en".into());
    }
    out
}

/// How sure language ID must be to override the person's own languages.
const SURE: f32 = 0.8;

/// Below this, a sample is silence: no microphone is that quiet while it hears a room.
const SILENT: f32 = 0.0001;

struct Audio {
    /// Only the audio callback touches this until the stream is dropped.
    samples: Mutex<Vec<f32>>,
    // Outside the lock: the capture loop polls these every 10 ms and must
    // never make the real-time callback's `try_lock` drop a buffer.
    failed: AtomicBool,
    full: AtomicBool,
    /// The microphone delivered its first real sound.
    heard: AtomicBool,
}
impl Audio {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(capacity)),
            failed: AtomicBool::new(false),
            full: AtomicBool::new(false),
            heard: AtomicBool::new(false),
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
            let s = frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32;
            if s.abs() >= SILENT {
                a.heard.store(true, Ordering::Release);
            }
            samples.push(s);
        }
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

/// The chosen device when it is connected (and whether it is), the system
/// default otherwise.
fn input_device(id: Option<&str>) -> Option<(cpal::Device, bool)> {
    let host = cpal::default_host();
    id.and_then(|id| id.parse::<cpal::DeviceId>().ok())
        .and_then(|id| host.device_by_id(&id))
        .map(|device| (device, true))
        .or_else(|| Some((host.default_input_device()?, false)))
}

/// The chosen microphone, found by the last recording. Finding it again
/// costs time before the first sample; its configuration is asked each time
/// (the system's audio settings may have changed it).
static OPENED: Mutex<Option<(String, cpal::Device)>> = Mutex::new(None);

pub struct Capture {
    stream: Option<cpal::Stream>,
    audio: Arc<Audio>,
    rate: u32,
}
impl Capture {
    pub fn start(id: Option<&str>, level: Arc<AtomicU32>) -> Result<Self> {
        let opened = OPENED.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some((_, device)) = opened.filter(|(was, _)| Some(was.as_str()) == id)
            && let Ok(config) = device.default_input_config()
            && let Ok(capture) = Self::open(&device, config, level.clone())
        {
            return Ok(capture);
        }
        *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let (device, chosen) = input_device(id).context("No microphone available")?;
        let config = device.default_input_config()?;
        let capture = Self::open(&device, config, level)?;
        if let (Some(id), true) = (id, chosen) {
            *OPENED.lock().unwrap_or_else(|e| e.into_inner()) = Some((id.into(), device));
        }
        Ok(capture)
    }
    fn open(
        device: &cpal::Device,
        config: cpal::SupportedStreamConfig,
        level: Arc<AtomicU32>,
    ) -> Result<Self> {
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
                    &config.config(),
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
    Finalizing,
    Final(String),
    Failed(String),
}
struct Job {
    dir: std::path::PathBuf,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    device: Option<String>,
    events: std::sync::mpsc::SyncSender<Event>,
}
// Capture owns its stream on a separate thread. Model loading and inference
// remain serialized on WORKER, including after the user cancels native work.
trait Recording {
    fn ended(&self) -> bool;
    /// Real sound has arrived: what is said from now on is kept.
    fn flowing(&self) -> bool;
    fn finish(self) -> Result<(Vec<f32>, u32)>;
}
impl Recording for Capture {
    fn ended(&self) -> bool {
        self.ended()
    }
    fn flowing(&self) -> bool {
        self.audio.heard.load(Ordering::Acquire)
    }
    fn finish(self) -> Result<(Vec<f32>, u32)> {
        self.finish()
    }
}

/// The longest "Listening" waits for a first sound.
const WARM_UP: Duration = Duration::from_millis(600);

fn record<C: Recording>(
    job: &Job,
    start: impl FnOnce() -> Result<C>,
) -> Result<Option<(Vec<f32>, u32)>> {
    if job.cancel.load(Ordering::Acquire) || job.stop.load(Ordering::Acquire) {
        return Ok(None);
    }
    let capture = start()?;
    // A microphone delivers its first sound some 150 ms after it opens, and
    // words said before it are lost: "Listening" waits for that sound, or
    // for WARM_UP in a silent room. Stop/cancel may arrive meanwhile.
    let opened = Instant::now();
    while !capture.flowing()
        && opened.elapsed() < WARM_UP
        && !job.stop.load(Ordering::Acquire)
        && !job.cancel.load(Ordering::Acquire)
        && !capture.ended()
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    if job.cancel.load(Ordering::Acquire) {
        return Ok(None);
    }
    if !job.stop.load(Ordering::Acquire) {
        let _ = job.events.try_send(Event::Listening);
    }
    let started = Instant::now();
    while !job.stop.load(Ordering::Acquire)
        && !job.cancel.load(Ordering::Acquire)
        && !capture.ended()
        && started.elapsed().as_secs() < MAX_SECONDS as u64
    {
        std::thread::sleep(Duration::from_millis(10));
    }
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
    F: FnOnce(Vec<f32>, u32) -> Result<String>,
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
        let model = load();
        // A load failure must stop capture even if the user has not pressed Stop.
        // Keep the admission guard until the stream has actually been dropped.
        if model.is_err() {
            job.cancel.store(true, Ordering::Release);
        }
        let audio = capture
            .join()
            .map_err(|_| anyhow::anyhow!("Microphone worker failed"))?;
        let transcribe = model?;
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
    /// (The admission is none only in tests: it is process-wide.)
    Transcribe(Job, Option<BusyGuard>),
    /// How long to keep the model changed: look again.
    Keep,
    /// Answers once the weights are gone: a loaded model maps its file, which
    /// Windows won't delete while it is mapped.
    Unload(std::sync::mpsc::SyncSender<()>),
}
static WORKER: std::sync::OnceLock<std::sync::mpsc::SyncSender<Command>> =
    std::sync::OnceLock::new();
/// How long the worker keeps a model after a dictation, in ms (0: released
/// at once; ALWAYS: while Zeron is open).
static KEEP_MS: AtomicU64 = AtomicU64::new(300_000);
const ALWAYS: u64 = u64::MAX;

/// Keep the model loaded this long after a dictation (zero: released at
/// once; `None`: while Zeron is open). Reloading costs well under a second
/// from the disk's cache, seconds from a cold one; a model held costs its
/// memory.
pub fn keep_ready(time: Option<Duration>) {
    let ms = time.map_or(ALWAYS, |t| (t.as_millis() as u64).min(ALWAYS - 1));
    KEEP_MS.store(ms, Ordering::Release);
    if let Some(worker) = WORKER.get() {
        let _ = worker.try_send(Command::Keep);
    }
}

/// What the worker transcribes with: a model loaded from its folder.
trait Model {
    fn transcribe(&mut self, samples: Vec<f32>, rate: u32) -> Result<String>;
}
impl Model for Recognizer {
    fn transcribe(&mut self, samples: Vec<f32>, rate: u32) -> Result<String> {
        Recognizer::transcribe(self, samples, rate)
    }
}

/// The worker: one job at a time, the model kept between them for `keep`
/// after the last, one model in memory at a time.
fn serve<C: Recording, M: Model>(
    rx: std::sync::mpsc::Receiver<Command>,
    keep: &AtomicU64,
    start: impl Fn(&Job) -> Result<C> + Sync,
    mut load: impl FnMut(&Path) -> Result<M>,
) {
    let mut cached: Option<(PathBuf, M)> = None;
    let mut since = Instant::now();
    loop {
        let keep_ms = keep.load(Ordering::Acquire);
        let command = if cached.is_none() || keep_ms == ALWAYS {
            rx.recv()
                .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(Duration::from_millis(keep_ms).saturating_sub(since.elapsed()))
        };
        let (job, _admission) = match command {
            Ok(Command::Transcribe(j, admission)) => (j, admission),
            Ok(Command::Keep) => continue,
            Ok(Command::Unload(done)) => {
                cached = None;
                let _ = done.try_send(());
                continue;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                cached = None;
                continue;
            }
            Err(_) => break,
        };
        // A native panic must not take down the worker, and with it every
        // later session, until the app restarts.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_job(
                &job,
                || start(&job),
                || {
                    if cached.as_ref().is_none_or(|(path, _)| *path != job.dir) {
                        // One model in memory at a time: the other goes first.
                        cached = None;
                        let model = load(&job.dir).context(
                            "Could not load the model. Remove it in Settings and download again.",
                        )?;
                        cached = Some((job.dir.clone(), model));
                    }
                    let model = &mut cached.as_mut().unwrap().1;
                    Ok(move |samples, rate| model.transcribe(samples, rate))
                },
            )
        }))
        .unwrap_or_else(|_| {
            cached = None;
            Err(anyhow::anyhow!(
                "Dictation stopped unexpectedly. Your draft is safe."
            ))
        });
        since = Instant::now();
        match result {
            Ok(Some(text)) => {
                let _ = job.events.try_send(Event::Final(text));
            }
            Ok(None) => {}
            // Keep loaded weights: a microphone or recording error says
            // nothing about the model.
            Err(e) => {
                let _ = job.events.try_send(Event::Failed(e.to_string()));
            }
        }
    }
}
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
        let admission = BusyGuard::acquire()?;
        let sender = WORKER.get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<Command>(1);
            std::thread::spawn(move || {
                serve(
                    rx,
                    &KEEP_MS,
                    |job| Capture::start(job.device.as_deref(), job.level.clone()),
                    |dir| Recognizer::load(dir),
                )
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
                    device,
                    events: tx,
                },
                Some(admission),
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

/// Retire cached weights on the worker, and wait (briefly) until they are.
pub fn unload() {
    if let Some(worker) = WORKER.get() {
        let (done, unloaded) = std::sync::mpsc::sync_channel(1);
        if worker.try_send(Command::Unload(done)).is_ok() {
            let _ = unloaded.recv_timeout(Duration::from_secs(2));
        }
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
    fn models_pin_exact_bytes_and_ultra_is_the_default() {
        for m in models() {
            assert_eq!(m.revision.len(), 40);
            assert!(!m.languages.is_empty() && !RETIRED.contains(&m.id.as_str()));
            assert!(
                m.files
                    .iter()
                    .all(|f| f.sha256.len() == 64 && !f.name.contains('/'))
            );
        }
        assert_eq!(manifest().name, "Parakeet Ultra");
        assert_eq!(download_size(), 740363168);
        let dir = Path::new("models/cohere-transcribe-q5");
        assert_eq!(model_for(dir).unwrap().name, "Cohere Transcribe");
        // Any other folder is no model's, not the default's.
        assert!(model_for(Path::new("models/parakeet-tdt-0.6b-v3-int8")).is_none());
        // A model that can't tell the language hears the person's own.
        let cohere = model_for(dir).unwrap().languages;
        let locales = ["fr-FR", "en_GB.UTF-8", "fr-CA", "uk-UA"].map(String::from);
        assert_eq!(spoken(locales, &cohere), ["fr", "en"]);
        assert_eq!(spoken(["uk-UA".to_string()], &cohere), ["en"]);
    }
    #[test]
    fn cancelled_download_never_establishes_readiness() {
        let root = tempfile::tempdir().unwrap();
        let m = manifest();
        let dir = root.path().join(&m.id);
        assert!(
            m.download(&dir, &AtomicBool::new(true), |_| panic!(
                "no progress after cancellation"
            ))
            .is_err()
        );
        assert!(!m.installed(&dir));
        assert!(!dir.exists(), "a cancelled download leaves nothing behind");
    }
    #[test]
    fn files_already_verified_are_kept_not_fetched_again() {
        // A download that stopped after its first file: retried, that file
        // is kept as it matches its pin, and only what's missing is fetched
        // (here nothing: no network is touched).
        let root = tempfile::tempdir().unwrap();
        let bytes = b"weights".as_slice();
        let m = Manifest {
            id: "tiny".into(),
            files: vec![Artifact {
                name: "weights.gguf".into(),
                size: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
                role: None,
                repository: Some("nowhere/never".into()),
                revision: None,
            }],
            ..manifest()
        };
        let dir = root.path().join("tiny");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("weights.gguf"), bytes).unwrap();
        let mut seen = 0;
        m.download(&dir, &AtomicBool::new(false), |n| seen = n)
            .unwrap();
        assert_eq!(seen, bytes.len() as u64);
        assert!(m.installed(&dir));
        // A pin changed on any file verifies again.
        let mut changed = m.clone();
        changed.files[0].sha256 = "0".repeat(64);
        assert!(!changed.installed(&dir));
    }
    #[test]
    fn partial_or_corrupt_model_cannot_load() {
        let root = tempfile::tempdir().unwrap();
        let m = manifest();
        let dir = root.path().join(&m.id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(&m.files[0].name), b"corrupt").unwrap();
        std::fs::write(dir.join("verified"), m.marker()).unwrap();
        assert!(!installed(&dir));
        assert!(Recognizer::load(&dir).is_err());
    }
}

#[cfg(test)]
mod model_tests;
#[cfg(test)]
mod session_tests;
