//! The microphone on Linux: ALSA, loaded only when dictation lists
//! microphones or records (`libasound.so.2` through `dlopen`), never linked.
//!
//! `zeron` is also what a server or a Cloud machine runs (`zeron headless`,
//! `zeron update`, …), and those often have no ALSA. Linked in (as CPAL's ALSA
//! backend is), the library would be one the loader needs before `main`: the
//! binary couldn't start at all there, not even `zeron --version`. Loaded here
//! instead, a computer without it records nothing and says why.
//!
//! What CPAL did here, kept: microphones are ALSA's PCM hints that can capture
//! (ids `alsa:<pcm name>`, CPAL's own, so a microphone chosen before stays
//! chosen), the system default is `alsa:default`, and a recording takes the
//! device's own rate and channels, mixed down to mono by the caller.

use std::{
    ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_ulong, c_void},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use anyhow::{Result, bail};

use super::{Audio, InputDevice, MAX_SECONDS, append, meter};

/// The library's soname: what every distribution's ALSA package ships.
const LIBRARY: &str = "libasound.so.2";
/// `alsa:` ids: the host CPAL names, so ids it gave out still resolve.
const HOST: &str = "alsa";
const DEFAULT_DEVICE: &str = "default";

const SND_PCM_STREAM_CAPTURE: c_int = 1;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_FORMAT_S32_LE: c_int = 10;
const SND_PCM_FORMAT_FLOAT_LE: c_int = 14;
/// Frames read at a time: ~20 ms at 48 kHz, how soon Stop is heard.
const CHUNK_FRAMES: usize = 1024;

type Pcm = c_void;
type HwParams = c_void;

/// The few ALSA calls dictation makes.
struct Api {
    pcm_open: unsafe extern "C" fn(*mut *mut Pcm, *const c_char, c_int, c_int) -> c_int,
    pcm_close: unsafe extern "C" fn(*mut Pcm) -> c_int,
    hw_params_malloc: unsafe extern "C" fn(*mut *mut HwParams) -> c_int,
    hw_params_free: unsafe extern "C" fn(*mut HwParams),
    hw_params_any: unsafe extern "C" fn(*mut Pcm, *mut HwParams) -> c_int,
    hw_params_set_access: unsafe extern "C" fn(*mut Pcm, *mut HwParams, c_int) -> c_int,
    hw_params_test_format: unsafe extern "C" fn(*mut Pcm, *mut HwParams, c_int) -> c_int,
    hw_params_set_format: unsafe extern "C" fn(*mut Pcm, *mut HwParams, c_int) -> c_int,
    hw_params_set_channels_near: unsafe extern "C" fn(*mut Pcm, *mut HwParams, *mut c_uint) -> c_int,
    hw_params_set_rate_near:
        unsafe extern "C" fn(*mut Pcm, *mut HwParams, *mut c_uint, *mut c_int) -> c_int,
    hw_params: unsafe extern "C" fn(*mut Pcm, *mut HwParams) -> c_int,
    pcm_readi: unsafe extern "C" fn(*mut Pcm, *mut c_void, c_ulong) -> c_long,
    pcm_recover: unsafe extern "C" fn(*mut Pcm, c_int, c_int) -> c_int,
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    device_name_hint: unsafe extern "C" fn(c_int, *const c_char, *mut *mut *mut c_void) -> c_int,
    device_name_get_hint: unsafe extern "C" fn(*const c_void, *const c_char) -> *mut c_char,
    device_name_free_hint: unsafe extern "C" fn(*mut *mut c_void) -> c_int,
    // Last: the calls above point into it.
    _library: libloading::Library,
}

/// ALSA, loaded the first time it's asked for; why not, when it can't be.
fn api() -> Result<&'static Api, &'static str> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(load).as_ref().map_err(String::as_str)
}

fn load() -> Result<Api, String> {
    // SAFETY: libasound's initialisers only set up its own state.
    let library = unsafe { libloading::Library::new(LIBRARY) }
        .map_err(|_| format!("No microphone available: ALSA ({LIBRARY}) isn't installed on this computer"))?;
    macro_rules! symbol {
        ($name:literal) => {
            // SAFETY: each is declared with alsa-lib's C signature (alsa/pcm.h,
            // alsa/control.h, alsa/error.h), and only called while `library`,
            // kept in the same `Api`, stays loaded.
            {
                let symbol: libloading::Symbol<'_, _> = unsafe { library.get(concat!($name, "\0").as_bytes()) }
                    .map_err(|error| format!("No microphone available: {LIBRARY} has no {} ({error})", $name))?;
                *symbol
            }
        };
    }
    Ok(Api {
        pcm_open: symbol!("snd_pcm_open"),
        pcm_close: symbol!("snd_pcm_close"),
        hw_params_malloc: symbol!("snd_pcm_hw_params_malloc"),
        hw_params_free: symbol!("snd_pcm_hw_params_free"),
        hw_params_any: symbol!("snd_pcm_hw_params_any"),
        hw_params_set_access: symbol!("snd_pcm_hw_params_set_access"),
        hw_params_test_format: symbol!("snd_pcm_hw_params_test_format"),
        hw_params_set_format: symbol!("snd_pcm_hw_params_set_format"),
        hw_params_set_channels_near: symbol!("snd_pcm_hw_params_set_channels_near"),
        hw_params_set_rate_near: symbol!("snd_pcm_hw_params_set_rate_near"),
        hw_params: symbol!("snd_pcm_hw_params"),
        pcm_readi: symbol!("snd_pcm_readi"),
        pcm_recover: symbol!("snd_pcm_recover"),
        strerror: symbol!("snd_strerror"),
        device_name_hint: symbol!("snd_device_name_hint"),
        device_name_get_hint: symbol!("snd_device_name_get_hint"),
        device_name_free_hint: symbol!("snd_device_name_free_hint"),
        _library: library,
    })
}

impl Api {
    fn error(&self, code: c_int) -> String {
        // SAFETY: snd_strerror returns a static string for any code.
        unsafe { CStr::from_ptr((self.strerror)(code)) }
            .to_string_lossy()
            .into_owned()
    }

    /// One field of a hint (`NAME`, `DESC`, `IOID`), freed once read.
    fn hint_field(&self, hint: *const c_void, field: &CStr) -> Option<String> {
        // SAFETY: `hint` is one of snd_device_name_hint's entries, alive until
        // the list is freed; the returned string is malloc'd and ours to free.
        unsafe {
            let value = (self.device_name_get_hint)(hint, field.as_ptr());
            if value.is_null() {
                return None;
            }
            let text = CStr::from_ptr(value).to_string_lossy().into_owned();
            libc::free(value.cast());
            Some(text)
        }
    }
}

/// A PCM hint as a microphone, when it can capture: no `IOID` means both
/// ways; `Output` alone can't. Named by the first line of its description,
/// as CPAL names it.
fn hint_device(name: Option<String>, desc: Option<String>, ioid: Option<&str>) -> Option<InputDevice> {
    let name = name?;
    if ioid == Some("Output") {
        return None;
    }
    let display = desc
        .as_deref()
        .and_then(|desc| desc.lines().next())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .unwrap_or(&name)
        .to_owned();
    Some(InputDevice {
        id: format!("{HOST}:{name}"),
        name: display,
    })
}

/// The PCM an id names: `alsa:<pcm>`; anything else (another platform's id)
/// none.
fn pcm_name(id: &str) -> Option<&str> {
    id.strip_prefix(HOST)?.strip_prefix(':').filter(|name| !name.is_empty())
}

/// Every PCM ALSA's configuration offers for capture. Empty without ALSA.
pub fn input_devices() -> Vec<InputDevice> {
    let Ok(api) = api() else {
        return Vec::new();
    };
    let mut hints: *mut *mut c_void = std::ptr::null_mut();
    // SAFETY: card -1 is every card; `hints` receives a NULL-terminated list
    // freed below.
    if unsafe { (api.device_name_hint)(-1, c"pcm".as_ptr(), &mut hints) } < 0 || hints.is_null() {
        return Vec::new();
    }
    let mut devices = Vec::new();
    let mut at = hints;
    // SAFETY: walks the list up to its NULL end.
    while let Some(hint) = unsafe { at.as_ref() }.copied().filter(|hint| !hint.is_null()) {
        let ioid = api.hint_field(hint, c"IOID");
        if let Some(device) = hint_device(
            api.hint_field(hint, c"NAME"),
            api.hint_field(hint, c"DESC"),
            ioid.as_deref(),
        ) {
            devices.push(device);
        }
        // SAFETY: still inside the list (its end not reached).
        at = unsafe { at.add(1) };
    }
    // SAFETY: the list snd_device_name_hint gave, freed once.
    unsafe { (api.device_name_free_hint)(hints) };
    devices.dedup_by(|a, b| a.id == b.id);
    devices
}

/// The system default's id, when ALSA is there to record from it.
pub fn default_input_device() -> Option<String> {
    api().ok()?;
    Some(format!("{HOST}:{DEFAULT_DEVICE}"))
}

/// A sample format ALSA delivers, as dictation reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    F32,
    I32,
    I16,
}

impl Format {
    /// Preferred first: floats need no scaling; 32-bit keeps a 24-bit
    /// microphone's depth.
    const ALL: [(Self, c_int); 3] = [
        (Self::F32, SND_PCM_FORMAT_FLOAT_LE),
        (Self::I32, SND_PCM_FORMAT_S32_LE),
        (Self::I16, SND_PCM_FORMAT_S16_LE),
    ];

    fn bytes(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 4,
            Self::I16 => 2,
        }
    }
}

/// `frames` interleaved frames of `format` from `bytes`, as samples from -1 to 1,
/// into the level meter and the recording.
fn deliver(bytes: &[u8], format: Format, channels: usize, rate: u32, level: &AtomicU32, audio: &Audio) {
    fn each<const N: usize, T: Copy>(
        bytes: &[u8],
        decode: fn([u8; N]) -> T,
        to_f32: fn(T) -> f32,
        channels: usize,
        rate: u32,
        level: &AtomicU32,
        audio: &Audio,
    ) {
        let samples: Vec<T> = bytes
            .chunks_exact(N)
            .map(|chunk| decode(chunk.try_into().expect("N bytes")))
            .collect();
        meter(&samples, channels, level, to_f32);
        append(&samples, channels, rate, audio, to_f32);
    }
    match format {
        Format::F32 => each(bytes, f32::from_le_bytes, |s| s, channels, rate, level, audio),
        Format::I32 => each(
            bytes,
            i32::from_le_bytes,
            |s| s as f32 / 2_147_483_648.0,
            channels,
            rate,
            level,
            audio,
        ),
        Format::I16 => each(
            bytes,
            i16::from_le_bytes,
            |s| s as f32 / 32_768.0,
            channels,
            rate,
            level,
            audio,
        ),
    }
}

/// An open capture PCM; closed when dropped.
struct Device {
    api: &'static Api,
    pcm: *mut Pcm,
}

// SAFETY: an ALSA PCM handle may be used from any one thread at a time; it's
// opened on the dictation worker and from then on used by the reader alone.
unsafe impl Send for Device {}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: opened by snd_pcm_open, closed once.
        unsafe { (self.api.pcm_close)(self.pcm) };
    }
}

/// The device's own setup: interleaved, the best format it offers, its
/// channels and its rate as near 48 kHz as it goes (the recognizer resamples).
struct Setup {
    format: Format,
    channels: usize,
    rate: u32,
}

impl Device {
    fn open(api: &'static Api, name: &str) -> Result<(Self, Setup)> {
        let c_name = CString::new(name)?;
        let mut pcm = std::ptr::null_mut();
        // SAFETY: a blocking capture open; `pcm` is set on success.
        let opened = unsafe { (api.pcm_open)(&mut pcm, c_name.as_ptr(), SND_PCM_STREAM_CAPTURE, 0) };
        if opened < 0 {
            bail!("Could not open the microphone {name}: {}", api.error(opened))
        }
        let device = Self { api, pcm };
        let setup = device.configure()?;
        Ok((device, setup))
    }

    fn configure(&self) -> Result<Setup> {
        let api = self.api;
        let mut params = std::ptr::null_mut();
        // SAFETY: allocated here, freed by `Params` on every path.
        if unsafe { (api.hw_params_malloc)(&mut params) } < 0 || params.is_null() {
            bail!("Could not set up the microphone")
        }
        struct Params(&'static Api, *mut HwParams);
        impl Drop for Params {
            fn drop(&mut self) {
                // SAFETY: from snd_pcm_hw_params_malloc, freed once.
                unsafe { (self.0.hw_params_free)(self.1) };
            }
        }
        let params = Params(api, params);
        let check = |code: c_int, what: &str| -> Result<()> {
            if code < 0 {
                bail!("Could not set up the microphone ({what}: {})", api.error(code))
            }
            Ok(())
        };
        // SAFETY: `self.pcm` is open and `params.1` allocated; each call only
        // narrows the configuration space, then installs it.
        unsafe {
            check((api.hw_params_any)(self.pcm, params.1), "its formats")?;
            check(
                (api.hw_params_set_access)(self.pcm, params.1, SND_PCM_ACCESS_RW_INTERLEAVED),
                "interleaved access",
            )?;
            let Some((format, code)) = Format::ALL
                .into_iter()
                .find(|&(_, code)| (api.hw_params_test_format)(self.pcm, params.1, code) == 0)
            else {
                bail!("Unsupported microphone format")
            };
            check((api.hw_params_set_format)(self.pcm, params.1, code), "the format")?;
            let mut channels: c_uint = 1;
            check(
                (api.hw_params_set_channels_near)(self.pcm, params.1, &mut channels),
                "its channels",
            )?;
            let mut rate: c_uint = 48_000;
            let mut direction: c_int = 0;
            check(
                (api.hw_params_set_rate_near)(self.pcm, params.1, &mut rate, &mut direction),
                "its rate",
            )?;
            check((api.hw_params)(self.pcm, params.1), "installing the setup")?;
            Ok(Setup {
                format,
                channels: (channels as usize).max(1),
                rate,
            })
        }
    }

    /// Reads into `buffer` until `stop`; false when the device failed.
    fn read_until(&self, setup: &Setup, stop: &AtomicBool, level: &AtomicU32, audio: &Audio) -> bool {
        let frame_bytes = setup.channels * setup.format.bytes();
        let mut buffer = vec![0_u8; CHUNK_FRAMES * frame_bytes];
        while !stop.load(Ordering::Acquire) && !audio.full.load(Ordering::Acquire) {
            // SAFETY: `buffer` holds CHUNK_FRAMES frames of this setup.
            let read = unsafe {
                (self.api.pcm_readi)(self.pcm, buffer.as_mut_ptr().cast(), CHUNK_FRAMES as c_ulong)
            };
            if read < 0 {
                // An overrun or a suspend: prepared again and read on. Anything
                // else (unplugged): the recording ends, failed.
                // SAFETY: the open PCM and the error it just gave.
                if unsafe { (self.api.pcm_recover)(self.pcm, read as c_int, 1) } < 0 {
                    return false;
                }
                continue;
            }
            let bytes = &buffer[..read as usize * frame_bytes];
            deliver(bytes, setup.format, setup.channels, setup.rate, level, audio);
        }
        true
    }
}

pub struct Capture {
    stop: Arc<AtomicBool>,
    reader: Option<std::thread::JoinHandle<()>>,
    audio: Arc<Audio>,
    rate: u32,
}

impl Capture {
    /// Records from `device` (an [`InputDevice::id`]), or the system default
    /// when it's none, isn't ALSA's, or won't open (unplugged).
    pub fn start(device: Option<&str>, level: Arc<AtomicU32>) -> Result<Self> {
        let api = api().map_err(|why| anyhow::anyhow!("{why}"))?;
        let chosen = device.and_then(pcm_name).filter(|name| *name != DEFAULT_DEVICE);
        let (device, setup) = match chosen.map(|name| Device::open(api, name)) {
            Some(Ok(opened)) => opened,
            _ => Device::open(api, DEFAULT_DEVICE)
                .map_err(|error| anyhow::anyhow!("No microphone available ({error:#})"))?,
        };
        let rate = setup.rate;
        let audio = Arc::new(Audio::with_capacity(rate as usize * MAX_SECONDS));
        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let (audio, stop) = (audio.clone(), stop.clone());
            std::thread::Builder::new()
                .name("dictation-alsa".into())
                .spawn(move || {
                    if !device.read_until(&setup, &stop, &level, &audio) {
                        audio.failed.store(true, Ordering::Release);
                    }
                    // (The device closes here, on the reader.)
                })?
        };
        Ok(Self {
            stop,
            reader: Some(reader),
            audio,
            rate,
        })
    }

    pub fn ended(&self) -> bool {
        self.audio.full.load(Ordering::Acquire) || self.audio.failed.load(Ordering::Acquire)
    }

    pub fn finish(mut self) -> Result<(Vec<f32>, u32)> {
        self.close();
        if self.audio.failed.load(Ordering::Acquire) {
            bail!("Microphone disconnected. Your draft is safe.")
        }
        let mut samples = self.audio.samples.lock().unwrap_or_else(|e| e.into_inner());
        Ok((std::mem::take(&mut *samples), self.rate))
    }

    /// Stops the reader and waits for it to close the device.
    fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_that_can_capture_are_microphones_named_as_cpal_named_them() {
        let device = |name: &str, desc: Option<&str>, ioid: Option<&str>| {
            hint_device(Some(name.into()), desc.map(Into::into), ioid)
        };
        assert_eq!(
            device("default", Some("Default ALSA Output (currently PipeWire Media Server)"), None),
            Some(InputDevice {
                id: "alsa:default".into(),
                name: "Default ALSA Output (currently PipeWire Media Server)".into()
            })
        );
        assert_eq!(
            device("hw:CARD=PCH,DEV=0", Some("HDA Intel PCH, ALC257 Analog\nDirect hardware device"), Some("Input")),
            Some(InputDevice {
                id: "alsa:hw:CARD=PCH,DEV=0".into(),
                name: "HDA Intel PCH, ALC257 Analog".into()
            })
        );
        assert_eq!(device("hdmi:CARD=PCH,DEV=0", Some("HDMI"), Some("Output")), None);
        assert_eq!(
            device("null", None, None),
            Some(InputDevice {
                id: "alsa:null".into(),
                name: "null".into()
            })
        );
        assert_eq!(hint_device(None, Some("no name".into()), None), None);
    }

    #[test]
    fn ids_name_alsa_pcms_only() {
        assert_eq!(pcm_name("alsa:default"), Some("default"));
        assert_eq!(pcm_name("alsa:hw:CARD=PCH,DEV=0"), Some("hw:CARD=PCH,DEV=0"));
        assert_eq!(pcm_name("alsa:"), None);
        assert_eq!(pcm_name("coreaudio:BuiltInMicrophoneDevice"), None);
        assert_eq!(pcm_name("alsadefault"), None);
    }

    #[test]
    fn every_format_arrives_as_mono_samples_with_its_level() {
        let frames = [[-0.5_f32, 0.5], [0.25, 0.75], [0.0, 0.0]];
        for format in [Format::F32, Format::I32, Format::I16] {
            let bytes: Vec<u8> = frames
                .iter()
                .flatten()
                .flat_map(|&s| match format {
                    Format::F32 => s.to_le_bytes().to_vec(),
                    Format::I32 => ((s as f64 * 2_147_483_648.0) as i32).to_le_bytes().to_vec(),
                    Format::I16 => ((s * 32_768.0) as i16).to_le_bytes().to_vec(),
                })
                .collect();
            let audio = Audio::with_capacity(0);
            let level = AtomicU32::new(0);
            deliver(&bytes, format, 2, 16_000, &level, &audio);
            let samples = audio.samples.lock().unwrap().clone();
            assert_eq!(samples.len(), 3, "{format:?}");
            for (got, want) in samples.iter().zip([0.0, 0.5, 0.0]) {
                assert!((got - want).abs() < 1e-4, "{format:?}: {samples:?}");
            }
            let rms = f32::from_bits(level.load(Ordering::Relaxed));
            assert!((rms - (0.25_f32 / 3.0).sqrt()).abs() < 0.01, "{format:?}: {rms}");
        }
    }

    /// With ALSA there (CI, desktops), the configured PCMs list and the
    /// default opens or fails with a message; without it, nothing lists and
    /// recording says ALSA is missing. Neither panics nor hangs.
    #[test]
    fn listing_and_starting_never_panic() {
        let devices = input_devices();
        match api() {
            Ok(_) => assert_eq!(default_input_device().as_deref(), Some("alsa:default")),
            Err(why) => {
                assert!(devices.is_empty());
                assert!(why.contains(LIBRARY), "{why}");
                assert!(default_input_device().is_none());
                let error = Capture::start(None, Arc::new(AtomicU32::new(0))).err().unwrap();
                assert!(error.to_string().contains(LIBRARY), "{error}");
            }
        }
    }
}
