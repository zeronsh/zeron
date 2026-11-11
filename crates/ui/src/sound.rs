//! Session notification sounds — the herdr approach (state-transition chimes
//! played through the platform's own audio CLI, zero Rust audio deps):
//!
//! - embedded completion, input-request, attention and Appshot chimes, and
//!   the start and end of a voice call;
//! - macOS Appshots use a sound registered once with the System Sound
//!   server; other cues write to a temp file and use the system player on a
//!   background thread: the System Sound server (macOS, see
//!   `macos_system_sound` for why not `afplay`), PowerShell
//!   `Media.SoundPlayer` (Windows), first of
//!   `paplay`/`pw-play`/`aplay`/`ffplay`/`mpv` (Linux — WAV, so even bare
//!   ALSA `aplay` decodes it);
//! - `ZERON_DISABLE_SOUND` env kill-switch + the `soundEnabled` ui-setting;
//! - failures are logged and swallowed — a missing player must never bother
//!   the session flow.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DISABLE_ENV: &str = "ZERON_DISABLE_SOUND";
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(any(target_os = "macos", target_os = "linux"))]
static SOUND_APPSHOT: &[u8] = include_bytes!("../assets/sounds/appshot.wav");

static SOUND_DONE: &[u8] = include_bytes!("../assets/sounds/done.wav");
static SOUND_REQUEST: &[u8] = include_bytes!("../assets/sounds/request.wav");
static SOUND_ATTENTION: &[u8] = include_bytes!("../assets/sounds/attention.wav");
static SOUND_VOICE_START: &[u8] = include_bytes!("../assets/sounds/voice-start.wav");
static SOUND_VOICE_END: &[u8] = include_bytes!("../assets/sounds/voice-end.wav");

/// Which notification chime to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sound {
    /// An agent turn completed successfully.
    Done,
    /// The agent is waiting on a question (→ AwaitingInput).
    Request,
    /// A run failed or the durable connection state degraded.
    Attention,
}

/// Play a chime on a background thread. Silently a no-op when disabled or no
/// player is available.
pub fn play(sound: Sound) {
    let data = match sound {
        Sound::Done => SOUND_DONE,
        Sound::Request => SOUND_REQUEST,
        Sound::Attention => SOUND_ATTENTION,
    };
    play_in_background(data);
}

/// A voice call starting (`true`) or ending. Only its two ends sound: the
/// caller plays each once per call, never in between.
pub fn play_voice(start: bool) {
    play_in_background(if start {
        SOUND_VOICE_START
    } else {
        SOUND_VOICE_END
    });
}

/// Confirm captured pixels with a soft shutter and clear chime.
/// The caller honors the dedicated capture sound setting.
pub fn play_appshot() {
    if std::env::var_os(DISABLE_ENV).is_some() {
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some(appshot) = macos_appshot::sound() {
        appshot.play();
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    play_in_background(SOUND_APPSHOT);
}

/// Register the macOS capture cue before the first capture, without playing
/// it, so the first capture sounds as quickly as the rest.
pub fn prepare_appshot() {
    #[cfg(target_os = "macos")]
    if std::env::var_os(DISABLE_ENV).is_none() {
        let _ = std::thread::Builder::new()
            .name("appshot-audio".into())
            .spawn(|| {
                macos_appshot::sound();
            });
    }
}

/// The Appshot cue, registered with the System Sound server once and played
/// from there on every capture (see `macos_system_sound` for why not in-app:
/// an AVAudioPlayer takes the CoreAudio assertion in our coalition).
#[cfg(target_os = "macos")]
mod macos_appshot {
    use super::macos_system_sound::{self, SystemSound};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    pub(super) struct Appshot {
        sound: SystemSound,
        path: PathBuf,
    }

    /// The registered cue, registered on first use; `None` if the server
    /// refused it, and then each capture uses the system player instead.
    pub(super) fn sound() -> Option<&'static Appshot> {
        static SOUND: OnceLock<Option<Appshot>> = OnceLock::new();
        SOUND
            .get_or_init(|| match register() {
                Ok(appshot) => Some(appshot),
                Err(err) => {
                    tracing::warn!(error = %err, "Could not register the Appshot sound; using system player");
                    None
                }
            })
            .as_ref()
    }

    /// The server reads the file when the sound plays, not only when it is
    /// registered, so the file stays for as long as the app runs: one per
    /// user, rewritten at launch, never one per launch.
    fn register() -> Result<Appshot, String> {
        let path = std::env::temp_dir().join("zeron-appshot.wav");
        write(&path)?;
        let sound = macos_system_sound::register(&path)?;
        Ok(Appshot { sound, path })
    }

    /// The cue's bytes at `path`, through an exclusive file of its own moved
    /// over it, so a planted file or a second app's copy is replaced whole.
    fn write(path: &Path) -> Result<(), String> {
        let (mut file, tmp) = super::create_temp_file().map_err(|e| e.to_string())?;
        file.write_all(super::SOUND_APPSHOT).map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&tmp.path, path).map_err(|e| e.to_string())
    }

    impl Appshot {
        /// Starts the cue and returns at once.
        pub(super) fn play(&self) {
            // The system cleans its temp folder now and then; a long-running
            // app writes the file back rather than playing nothing.
            if !self.path.exists()
                && let Err(err) = write(&self.path)
            {
                tracing::debug!(error = %err, "could not restore the Appshot sound file");
            }
            let started = std::time::Instant::now();
            self.sound.play();
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis(),
                "Appshot system sound requested"
            );
        }
    }

    #[test]
    fn appshot_registers_with_the_system_sound_server_without_playback() {
        let appshot = register().expect("the Appshot sound should register");
        assert!(appshot.path.exists());
    }

    // Sounds the cue twice from one registration, for checking by hand who
    // holds the audio assertion: `log stream --process runningboardd`
    // should name systemsoundserverd, not this process.
    #[test]
    #[ignore = "plays a sound"]
    fn appshot_plays_twice_from_one_registration() {
        let appshot = sound().expect("the Appshot sound should register");
        for _ in 0..2 {
            appshot.play();
            std::thread::sleep(std::time::Duration::from_millis(900));
        }
    }
}

fn play_in_background(data: &'static [u8]) {
    if std::env::var_os(DISABLE_ENV).is_some() {
        return;
    }
    std::thread::spawn(move || {
        if let Err(err) = play_bytes(data) {
            tracing::debug!(error = %err, "sound playback failed");
        }
    });
}

fn play_bytes(data: &[u8]) -> Result<(), String> {
    // The system players want a file path. Exclusive creation prevents a
    // predictable-name collision (including a pre-planted symlink) from
    // redirecting the embedded bytes to another file.
    let (mut file, tmp) = create_temp_file().map_err(|e| e.to_string())?;
    file.write_all(data).map_err(|e| e.to_string())?;
    drop(file);
    let result = run_player(&tmp.path);
    drop(tmp);
    result
}

struct TempSoundFile {
    path: PathBuf,
}

impl Drop for TempSoundFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn create_temp_file() -> std::io::Result<(std::fs::File, TempSoundFile)> {
    for _ in 0..128 {
        let id = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("zeron-sound-{}-{id}.wav", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((file, TempSoundFile { path })),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not reserve a notification sound file",
    ))
}

#[cfg(target_os = "macos")]
fn run_player(path: &Path) -> Result<(), String> {
    macos_system_sound::play(path)
}

/// Chimes play in the System Sound server, not in a process of ours.
///
/// Whoever plays audio holds a CoreAudio assertion for as long as it sounds.
/// While a game holds the foreground macOS keeps every other app's process
/// coalition in the background, and the assertion of an `afplay` child lifts
/// the whole coalition out and drops it back when the sound ends.
/// AVAudioPlayer and NSSound take the same assertion, in the app. On macOS
/// 27.2 such a change can deadlock in the kernel against a process of the
/// same coalition that is in `exec`, and agents exec all the time: the window
/// server hangs behind it and the machine needs a reboot. The sound server
/// holds the assertion itself, so a chime leaves this app's processes alone.
#[cfg(target_os = "macos")]
mod macos_system_sound {
    use core_foundation::base::TCFType;
    use core_foundation::url::{CFURL, CFURLRef};
    use std::ffi::c_void;
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    type SystemSoundId = u32;

    const PROPERTY_IS_UI_SOUND: u32 = u32::from_be_bytes(*b"isui");

    #[link(name = "AudioToolbox", kind = "framework")]
    unsafe extern "C" {
        fn AudioServicesCreateSystemSoundID(url: CFURLRef, id: *mut SystemSoundId) -> i32;
        fn AudioServicesDisposeSystemSoundID(id: SystemSoundId) -> i32;
        fn AudioServicesSetProperty(
            property: u32,
            specifier_size: u32,
            specifier: *const c_void,
            data_size: u32,
            data: *const c_void,
        ) -> i32;
        fn AudioServicesPlaySystemSoundWithCompletion(
            id: SystemSoundId,
            completion: &block2::Block<dyn Fn()>,
        );
        fn AudioServicesPlaySystemSound(id: SystemSoundId);
    }

    pub(super) struct SystemSound(SystemSoundId);

    impl Drop for SystemSound {
        fn drop(&mut self) {
            unsafe {
                AudioServicesDisposeSystemSoundID(self.0);
            }
        }
    }

    impl SystemSound {
        /// Starts the sound and returns at once.
        pub(super) fn play(&self) {
            unsafe { AudioServicesPlaySystemSound(self.0) };
        }
    }

    pub(super) fn register(path: &Path) -> Result<SystemSound, String> {
        let url = CFURL::from_path(path, false)
            .ok_or_else(|| format!("no file URL for {}", path.display()))?;
        let mut id: SystemSoundId = 0;
        let status = unsafe { AudioServicesCreateSystemSoundID(url.as_concrete_TypeRef(), &mut id) };
        if status != 0 {
            return Err(format!("system sound server refused the chime ({status})"));
        }
        let sound = SystemSound(id);
        // System sounds default to interface feedback, silent when the user
        // turns interface sound effects off. A chime has its own setting.
        let ui_sound: u32 = 0;
        unsafe {
            AudioServicesSetProperty(
                PROPERTY_IS_UI_SOUND,
                size_of::<SystemSoundId>() as u32,
                (&raw const sound.0).cast(),
                size_of::<u32>() as u32,
                (&raw const ui_sound).cast(),
            );
        }
        Ok(sound)
    }

    pub(super) fn play(path: &Path) -> Result<(), String> {
        let sound = register(path)?;
        let (finished, wait) = mpsc::channel();
        let completion = block2::RcBlock::new(move || {
            let _ = finished.send(());
        });
        unsafe { AudioServicesPlaySystemSoundWithCompletion(sound.0, &completion) };
        // The caller deletes the file when this returns, so wait the chime
        // out. Bounded like the other players: a wedged sound server must not
        // accumulate threads.
        wait.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "system sound timed out".to_string())
    }
}

#[cfg(windows)]
fn run_player(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // SoundPlayer handles WAV natively; PlaySync keeps the process alive for
    // the chime's duration. Pass the path through the child environment rather
    // than interpolating it into PowerShell source (paths may contain quotes).
    let script = "(New-Object Media.SoundPlayer $env:ZERON_SOUND_PATH).PlaySync()";
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env("ZERON_SOUND_PATH", path)
        .creation_flags(0x08000000)
        .output()
        .map_err(|e| format!("powershell failed: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("powershell exited with {}", output.status))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn run_player(path: &Path) -> Result<(), String> {
    // WAV everywhere, so even bare ALSA aplay decodes it (herdr must exclude
    // aplay because it ships mp3s).
    let players: &[(&str, &[&str])] = &[
        ("paplay", &[]),
        ("pw-play", &[]),
        ("aplay", &["-q"]),
        ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
        ("mpv", &["--no-video", "--really-quiet"]),
    ];
    let mut errors = Vec::new();
    for (program, args) in players {
        match run_checked(program, args, path) {
            Ok(()) => return Ok(()),
            Err(err) => errors.push(err),
        }
    }
    Err(format!("no audio player available: {}", errors.join("; ")))
}

#[cfg(not(target_os = "macos"))]
fn run_checked(program: &str, args: &[&str], path: &Path) -> Result<(), String> {
    // Bounded wait: a wedged audio daemon must not accumulate zombie threads.
    let mut child = std::process::Command::new(program)
        .args(args)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("{program} exited with {status}")),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{program} timed out"));
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{program}: {err}"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Notification decision (shared with the engine's desktop banners)
// ---------------------------------------------------------------------------

/// The engine decides banners with the same baselines (`desktop_notify`),
/// so a chime and a banner always agree on what happened.
pub(crate) use zeron_engine::desktop_notify::{
    ConnectivityNotificationState, Ping, SessionNotificationState,
};

impl From<Ping> for Sound {
    fn from(ping: Ping) -> Self {
        match ping {
            Ping::Done => Sound::Done,
            Ping::Request => Sound::Request,
            Ping::Attention => Sound::Attention,
        }
    }
}

/// Coalesce attention requests delivered by independent state watches. The
/// persistent gate also collapses multiple failures in one session snapshot.
#[derive(Debug, Default)]
pub(crate) struct AttentionSoundGate {
    last_played: Option<Instant>,
}

impl AttentionSoundGate {
    const COALESCE: Duration = Duration::from_millis(250);

    pub(crate) fn should_play(&mut self, now: Instant) -> bool {
        if self
            .last_played
            .is_some_and(|last| now.duration_since(last) < Self::COALESCE)
        {
            return false;
        }
        self.last_played = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_files_are_exclusive_and_cleaned_up() {
        let (first_file, first) = create_temp_file().expect("reserve first temp file");
        let (second_file, second) = create_temp_file().expect("reserve second temp file");
        assert_ne!(first.path, second.path);
        assert!(first.path.exists());
        assert!(second.path.exists());
        drop(first_file);
        drop(second_file);
        let first_path = first.path.clone();
        let second_path = second.path.clone();
        drop(first);
        drop(second);
        assert!(!first_path.exists());
        assert!(!second_path.exists());
    }

    #[test]
    fn embedded_chimes_are_wav() {
        for data in [
            SOUND_DONE,
            SOUND_REQUEST,
            SOUND_ATTENTION,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            SOUND_APPSHOT,
        ] {
            assert!(data.len() > 1000);
            assert_eq!(&data[..4], b"RIFF");
            assert_eq!(&data[8..12], b"WAVE");
        }
    }

    // Registering is everything short of sounding: the server opens and
    // accepts the file a chime is written to.
    #[cfg(target_os = "macos")]
    #[test]
    fn embedded_chimes_register_with_the_system_sound_server_without_playback() {
        for data in [
            SOUND_DONE,
            SOUND_REQUEST,
            SOUND_ATTENTION,
            SOUND_VOICE_START,
            SOUND_VOICE_END,
            SOUND_APPSHOT,
        ] {
            let (mut file, tmp) = create_temp_file().expect("reserve temp file");
            file.write_all(data).expect("write chime");
            drop(file);
            macos_system_sound::register(&tmp.path).expect("chime should register");
        }
    }

    #[test]
    fn attention_gate_coalesces_session_and_connectivity_watch_callbacks() {
        let t0 = Instant::now();
        let mut gate = AttentionSoundGate::default();
        assert!(gate.should_play(t0));
        assert!(!gate.should_play(t0));
        assert!(!gate.should_play(t0 + Duration::from_millis(200)));
        assert!(gate.should_play(t0 + Duration::from_millis(250)));
    }
}
