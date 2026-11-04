//! UI-independent dictation contract. Backends own capture and inference;
//! callers receive text and a scalar meter, never microphone samples.
use anyhow::Result;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Capabilities {
    pub local: bool,
    pub partial_results: bool,
    pub max_seconds: usize,
}

pub trait DictationSession {
    fn poll(&mut self) -> Option<crate::Event>;
    fn finish(&mut self);
    fn cancel(&mut self);
    fn take_level(&self) -> f32;
}

pub trait DictationBackend {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn start(&self, dir: PathBuf, device: Option<String>) -> Result<Box<dyn DictationSession>>;
}

pub struct Parakeet;
impl DictationBackend for Parakeet {
    fn id(&self) -> &'static str {
        "parakeet"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            local: true,
            partial_results: false,
            max_seconds: crate::MAX_SECONDS,
        }
    }
    fn start(&self, dir: PathBuf, device: Option<String>) -> Result<Box<dyn DictationSession>> {
        Ok(Box::new(crate::Session::start(dir, device)?))
    }
}
impl DictationSession for crate::Session {
    fn poll(&mut self) -> Option<crate::Event> {
        self.poll()
    }
    fn finish(&mut self) {
        self.finish()
    }
    fn cancel(&mut self) {
        self.cancel()
    }
    fn take_level(&self) -> f32 {
        self.take_level()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    };
    #[test]
    fn session_contract_signals_cancellation_without_waiting_for_inference() {
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (_sender, events) = mpsc::channel();
        let mut session: Box<dyn DictationSession> = Box::new(crate::Session {
            stop: stop.clone(),
            cancel: cancel.clone(),
            level: Arc::new(AtomicU32::new(0)),
            events,
        });
        session.finish();
        assert!(stop.load(Ordering::Acquire));
        session.cancel();
        assert!(cancel.load(Ordering::Acquire));
        assert_eq!(session.take_level(), 0.0);
    }
}
