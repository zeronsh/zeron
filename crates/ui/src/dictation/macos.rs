use super::{Event, Transcriber};
use std::ffi::{CStr, c_char, c_void};

unsafe extern "C" {
    fn zeron_dictation_start() -> *mut c_void;
    fn zeron_dictation_poll(session: *mut c_void) -> *const c_char;
    fn zeron_dictation_finish(session: *mut c_void);
    fn zeron_dictation_release(session: *mut c_void);
}

/// Deliberately !Send/!Sync: the bridge and its notifications belong to AppKit's
/// main thread, just like the owning ComposerInput entity.
pub(super) struct Native(*mut c_void);

impl Native {
    pub fn new() -> Self {
        Self(unsafe { zeron_dictation_start() })
    }
}

impl Transcriber for Native {
    fn poll(&mut self) -> Option<Event> {
        let json = unsafe { zeron_dictation_poll(self.0) };
        if json.is_null() {
            return None;
        }
        // The bridge retains the string until the next poll/release.
        Some(
            serde_json::from_slice(unsafe { CStr::from_ptr(json) }.to_bytes()).unwrap_or_else(
                |_| Event::Failed("Could not read the dictation result. Try again.".into()),
            ),
        )
    }

    fn finish(&mut self) {
        unsafe { zeron_dictation_finish(self.0) }
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        unsafe { zeron_dictation_release(self.0) }
    }
}
