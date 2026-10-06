//! Viewport-local dictation. Only transcript strings cross this boundary; audio
//! never enters the engine, a document, an attachment, or the RPC transport.
use std::ops::Range;
use std::time::{Duration, Instant};

pub(crate) mod glass;
mod meter;
mod model;
pub(crate) mod waveform;
pub(crate) use meter::{Bar, FLOOR, Meter};
pub(crate) use model::{card, enabled, init};

pub(crate) const FINALIZE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub(crate) enum Event {
    Listening,
    Finalizing,
    Partial(String),
    Final(String),
    Denied(String),
    Unavailable(String),
    Failed(String),
    Cancelled,
}

/// Implementations and their callbacks are owned by the UI thread. Tests inject
/// a fake; permission prompts and microphone access are never needed in CI.
pub(crate) trait Transcriber {
    fn poll(&mut self) -> Option<Event>;
    fn finish(&mut self);
    /// Peak input RMS since the previous call, for the live waveform.
    fn level(&mut self) -> f32 {
        0.0
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn zeron_microphone_permission() -> i32;
    fn zeron_request_microphone();
    fn zeron_microphone_window() -> usize;
}
fn permission() -> i32 {
    #[cfg(target_os = "macos")]
    {
        unsafe { zeron_microphone_permission() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        1
    }
}
pub(crate) fn permission_pending() -> bool {
    permission() == 0
}
fn origin_window() -> usize {
    #[cfg(target_os = "macos")]
    {
        unsafe { zeron_microphone_window() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        1
    }
}
struct Native {
    pending: Option<Event>,
    origin_window: usize,
    dir: std::path::PathBuf,
    device: Option<String>,
    session: Option<zeron_voice::Session>,
    finished: bool,
}
impl Transcriber for Native {
    fn poll(&mut self) -> Option<Event> {
        if let Some(event) = self.pending.take() {
            return Some(event);
        }
        if self.finished {
            return None;
        }
        if self.session.is_none() {
            match permission() {
                0 => return None,
                -2 => {
                    self.finished = true;
                    return Some(Event::Unavailable(
                        "Dictation requires the packaged Zeron app.".into(),
                    ));
                }
                -1 => {
                    self.finished = true;
                    return Some(Event::Denied("Allow Microphone access for Zeron in System Settings → Privacy & Security, then retry.".into()));
                }
                _ if self.origin_window == 0 || origin_window() != self.origin_window => {
                    self.finished = true;
                    return Some(Event::Cancelled);
                }
                _ => match zeron_voice::Session::start(self.dir.clone(), self.device.clone()) {
                    Ok(s) => self.session = Some(s),
                    Err(e) => {
                        self.finished = true;
                        return Some(Event::Failed(e.to_string()));
                    }
                },
            }
        }
        self.session.as_mut()?.poll().map(|event| match event {
            zeron_voice::Event::Listening => Event::Listening,
            zeron_voice::Event::Finalizing => Event::Finalizing,
            zeron_voice::Event::Final(t) => Event::Final(t),
            zeron_voice::Event::Failed(e) => Event::Failed(e),
        })
    }
    fn level(&mut self) -> f32 {
        self.session.as_ref().map_or(0.0, |s| s.take_level())
    }
    fn finish(&mut self) {
        if let Some(s) = &mut self.session {
            s.finish();
        } else {
            self.finished = true;
            self.pending = Some(Event::Final(String::new()));
        }
    }
}
pub(crate) fn start(cx: &gpui::App) -> Option<Box<dyn Transcriber>> {
    if !enabled(cx) {
        return None;
    }
    let origin_window = origin_window();
    #[cfg(target_os = "macos")]
    if permission() == 0 {
        unsafe {
            zeron_request_microphone();
        }
    }
    Some(Box::new(Native {
        pending: None,
        origin_window,
        dir: model::directory(cx),
        device: crate::settings::current(cx).dictation_input.clone(),
        session: None,
        finished: false,
    }))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    #[default]
    Idle,
    Requesting,
    Listening,
    Finalizing,
    NoSpeech,
    /// Released almost immediately: dictation is hold to talk.
    Tapped,
    Denied(String),
    Unavailable(String),
    Failed(String),
}

impl Phase {
    pub fn active(&self) -> bool {
        matches!(self, Self::Requesting | Self::Listening | Self::Finalizing)
    }

    /// Button names describe the action, independently of the live status.
    pub fn action_label(&self) -> &'static str {
        match self {
            Self::Idle | Self::Tapped => "Hold to dictate",
            Self::Requesting | Self::Listening => "Release to transcribe",
            Self::Finalizing => "Transcribing",
            Self::NoSpeech | Self::Denied(_) | Self::Unavailable(_) | Self::Failed(_) => {
                "Hold to retry dictation"
            }
        }
    }

    pub fn status(&self) -> Option<(&str, &str)> {
        Some(match self {
            Self::Idle => return None,
            Self::Requesting => ("Getting ready…", "Wait for Listening before speaking."),
            Self::Listening => ("Listening", "Release when you’re done · Up to 1 minute"),
            Self::Finalizing => ("Transcribing…", "Processing on this device."),
            Self::NoSpeech => (
                "No speech detected",
                "Check your microphone, then try again.",
            ),
            Self::Tapped => (
                "Hold to dictate",
                "Keep holding the microphone or the shortcut while you speak.",
            ),
            Self::Denied(message) | Self::Unavailable(message) => {
                ("Dictation unavailable", message)
            }
            Self::Failed(message) => ("Dictation stopped", message),
        })
    }
}

#[derive(Default)]
pub(crate) struct Dictation {
    pub phase: Phase,
    pub generation: u64,
    range: Range<usize>,
    expected: String,
    pub has_partial: bool,
    pending_send: bool,
    finish_started: Option<Instant>,
    pub meter: Meter,
}

impl Dictation {
    pub fn begin(&mut self, content: &str, selection: Range<usize>) {
        self.cancel();
        self.phase = Phase::Requesting;
        self.range = selection;
        self.expected = content.to_owned();
        self.has_partial = false;
    }

    /// Invalidates late callbacks AND pending sends, preserving committed text.
    pub fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.phase = Phase::Idle;
        self.pending_send = false;
        self.finish_started = None;
        self.expected.clear();
        self.meter.reset();
    }

    /// Returns true only on the transition that must finish the audio stream.
    pub fn finish(&mut self, send: bool, now: Instant) -> bool {
        if !self.phase.active() {
            return false;
        }
        self.pending_send |= send;
        if self.phase == Phase::Finalizing {
            return false;
        }
        self.phase = Phase::Finalizing;
        self.finish_started = Some(now);
        self.meter.freeze(now);
        true
    }

    pub fn timed_out(&self, now: Instant) -> bool {
        self.finish_started
            .is_some_and(|at| now.duration_since(at) >= FINALIZE_TIMEOUT)
    }

    /// Empty recognition does not erase selected text or the latest partial.
    /// A changed draft cannot be overwritten, even if a caller missed cancellation.
    pub fn replace(&mut self, content: &mut String, transcript: &str) -> Option<usize> {
        if !self.phase.active() || *content != self.expected {
            self.cancel();
            return None;
        }
        if transcript.trim().is_empty() {
            return None;
        }
        content.replace_range(self.range.clone(), transcript);
        self.range.end = self.range.start + transcript.len();
        self.expected.clone_from(content);
        self.has_partial = true;
        Some(self.range.end)
    }

    pub fn complete(&mut self, phase: Phase) -> bool {
        let send = self.pending_send && phase == Phase::Idle;
        self.cancel();
        self.phase = phase;
        send
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finishing_before_permission_does_not_wait_or_start_capture() {
        let mut native = Native {
            pending: None,
            origin_window: 0,
            dir: std::path::PathBuf::new(),
            device: None,
            session: None,
            finished: false,
        };
        native.finish();
        assert_eq!(native.poll(), Some(Event::Final(String::new())));
        assert_eq!(native.poll(), None);
        assert!(native.session.is_none());
    }

    #[test]
    fn finalization_deadline_does_not_restart_when_send_follows_stop() {
        let mut state = Dictation::default();
        state.begin("draft", 5..5);
        let start = Instant::now();
        assert!(state.finish(false, start));
        assert!(!state.finish(true, start + Duration::from_secs(1)));
        assert!(!state.timed_out(start + FINALIZE_TIMEOUT - Duration::from_millis(1)));
        assert!(state.timed_out(start + FINALIZE_TIMEOUT));
        assert!(state.complete(Phase::Idle));
        assert!(!state.complete(Phase::Idle));
    }

    #[test]
    fn unexpected_draft_change_rejects_late_partial_and_send() {
        let mut state = Dictation::default();
        state.begin("old", 0..3);
        state.finish(true, Instant::now());
        let mut current = "new draft".to_owned();
        assert_eq!(state.replace(&mut current, "late"), None);
        assert_eq!(current, "new draft");
        assert!(!state.complete(Phase::Idle));
    }
}
