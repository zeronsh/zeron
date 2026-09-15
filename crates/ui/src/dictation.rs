//! Viewport-local dictation. Only transcript strings cross this boundary; audio
//! never enters the engine, a document, an attachment, or the RPC transport.
use std::ops::Range;
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
mod macos;

pub(crate) const FINALIZE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub(crate) enum Event {
    Listening,
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
}

pub(crate) fn start() -> Option<Box<dyn Transcriber>> {
    #[cfg(target_os = "macos")]
    return Some(Box::new(macos::Native::new()));
    #[cfg(not(target_os = "macos"))]
    None
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum Phase {
    #[default]
    Idle,
    Requesting,
    Listening,
    Finalizing,
    Denied(String),
    Unavailable(String),
    Failed(String),
}

impl Phase {
    pub fn active(&self) -> bool {
        matches!(self, Self::Requesting | Self::Listening | Self::Finalizing)
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Idle => "Dictate on this Mac (⌘⇧D)",
            Self::Requesting => "Requesting microphone and speech permission — click to cancel",
            Self::Listening => "Listening on this Mac — stop dictation (⌘⇧D)",
            Self::Finalizing => "Finishing dictation…",
            Self::Denied(message) | Self::Unavailable(message) | Self::Failed(message) => message,
        }
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
        if transcript.is_empty() {
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
