//! macOS: sounds play in the System Sound server, not in a process of ours.
//!
//! Whoever plays audio holds a CoreAudio assertion for as long as it sounds.
//! While a game holds the foreground, macOS keeps every other app's process
//! coalition in the background, and the assertion of an `afplay` child lifts
//! the whole coalition out and drops it back when the sound ends.
//! AVAudioPlayer and NSSound take the same assertion, in the app. On macOS
//! 27.2 such a change can deadlock in the kernel against a process of the
//! same coalition that is in `exec` (an agent's `git` or `gh`): the window
//! server hangs behind it and the Mac needs a reboot. The sound server holds
//! the assertion itself, so a sound leaves this app's processes alone. The
//! shipping app does the same (crates/ui/src/sound.rs).

use block::ConcreteBlock;
use core_foundation::base::TCFType;
use core_foundation::url::{CFURL, CFURLRef};
use std::ffi::c_void;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

type SystemSoundId = u32;

/// `kAudioServicesPropertyIsUISound`.
const PROPERTY_IS_UI_SOUND: u32 = u32::from_be_bytes(*b"isui");

/// The longest a sound is waited for. Bounded so that a wedged sound server
/// can't pile up threads; every sound zgx plays is about a second.
const WAIT: Duration = Duration::from_secs(10);

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
    // `completion` is a `void (^)(void)` block.
    fn AudioServicesPlaySystemSoundWithCompletion(id: SystemSoundId, completion: *const c_void);
}

/// A sound the server has opened; disposed when dropped.
pub struct SystemSound(SystemSoundId);

impl Drop for SystemSound {
    fn drop(&mut self) {
        // SAFETY: the id came from AudioServicesCreateSystemSoundID and is
        // disposed only here, once.
        unsafe {
            AudioServicesDisposeSystemSoundID(self.0);
        }
    }
}

/// The file at `path`, opened by the sound server.
pub fn register(path: &Path) -> Result<SystemSound, String> {
    let url = CFURL::from_path(path, false).ok_or_else(|| format!("no file URL for {}", path.display()))?;
    let mut id: SystemSoundId = 0;
    // SAFETY: `url` is a live CFURL for the call; `id` is written on success.
    let status = unsafe { AudioServicesCreateSystemSoundID(url.as_concrete_TypeRef(), &mut id) };
    if status != 0 {
        return Err(format!("the system sound server refused the sound ({status})"));
    }
    let sound = SystemSound(id);
    // A system sound is interface feedback by default, silent when "Play
    // user interface sound effects" is off. zgx's sounds have settings of
    // their own.
    let ui_sound: u32 = 0;
    // SAFETY: the specifier is the sound's id and the data a UInt32, as the
    // property takes; both outlive the call.
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

impl SystemSound {
    /// Plays it, and returns once it has played (or after `WAIT`).
    pub fn play(&self) -> Result<(), String> {
        let (finished, wait) = mpsc::channel();
        let completion = ConcreteBlock::new(move || {
            let _ = finished.send(());
        })
        .copy();
        // SAFETY: the id is live; the server copies the block it keeps.
        unsafe { AudioServicesPlaySystemSoundWithCompletion(self.0, (&raw const *completion).cast()) };
        wait.recv_timeout(WAIT).map_err(|_| "the system sound timed out".to_string())
    }
}

/// The file at `path`, played once, then the sound disposed. It returns once
/// the sound has played, so the caller may then remove the file.
pub fn play_file(path: &Path) -> Result<(), String> {
    register(path)?.play()
}
