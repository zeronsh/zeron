//! A phone's dictation taken on this host (`zeron_proto::voice::dictation`):
//! the audio it streams goes into the desktop dictation's own speech model,
//! held in memory for the length of the dictation and nowhere else. One at a
//! time, as the model is.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use zeron_proto::voice::dictation::{self as wire, DictationEvent, DictationRejection};

pub struct DictationHost {
    data_dir: PathBuf,
    active: Arc<Mutex<Option<Active>>>,
}

struct Active {
    session_id: String,
    feed: Arc<zeron_voice::Feed>,
    /// Taken by the one `Own` subscription.
    events: Option<mpsc::Receiver<DictationEvent>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    sequence: u64,
}

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|id| id.get_version_num() == 4)
}

impl DictationHost {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            active: Default::default(),
        }
    }

    fn model_dir(&self) -> PathBuf {
        self.data_dir.join(zeron_voice::MODEL_DIR)
    }

    /// Opens a dictation: the model must be downloaded here, and no other
    /// dictation may be running.
    pub fn start(&self, session_id: &str) -> Result<(), DictationRejection> {
        if !valid_id(session_id) {
            return Err(DictationRejection::Protocol);
        }
        let dir = self.model_dir();
        if !zeron_voice::installed(&dir) {
            return Err(DictationRejection::ModelMissing);
        }
        let mut active = self.active.lock().unwrap();
        if let Some(current) = active.as_ref() {
            if current.started.elapsed() < Duration::from_secs(wire::IDLE_SECS) {
                return Err(DictationRejection::Busy);
            }
            current.cancel.store(true, Ordering::Release);
        }
        let feed = zeron_voice::Feed::new(wire::SAMPLE_RATE);
        let mut session = zeron_voice::Session::start_fed(dir, feed.clone())
            .map_err(|_| DictationRejection::Busy)?;
        let (tx, rx) = mpsc::channel(16);
        let cancel = Arc::new(AtomicBool::new(false));
        let watch = cancel.clone();
        let slot = self.active.clone();
        let id = session_id.to_owned();
        let started = Instant::now();
        // The session is polled off the runtime; dropping it cancels native
        // work that is still running.
        std::thread::Builder::new()
            .name("dictation-host".into())
            .spawn(move || {
                let mut done = false;
                while !done && !watch.load(Ordering::Acquire) {
                    if started.elapsed() > Duration::from_secs(wire::IDLE_SECS + 60) {
                        break;
                    }
                    let Some(event) = session.poll() else {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    };
                    let event = match event {
                        zeron_voice::Event::Listening => DictationEvent::Listening,
                        zeron_voice::Event::Partial(text) => DictationEvent::Partial { text },
                        zeron_voice::Event::Finalizing => DictationEvent::Finalizing,
                        zeron_voice::Event::Final(text) => DictationEvent::Final { text },
                        zeron_voice::Event::Failed(message) => DictationEvent::Failed { message },
                    };
                    done = matches!(
                        event,
                        DictationEvent::Final { .. } | DictationEvent::Failed { .. }
                    );
                    if tx.blocking_send(event).is_err() {
                        break;
                    }
                }
                drop(session);
                let mut slot = slot.lock().unwrap();
                if slot.as_ref().is_some_and(|a| a.session_id == id) {
                    *slot = None;
                }
            })
            .map_err(|_| DictationRejection::Protocol)?;
        *active = Some(Active {
            session_id: session_id.to_owned(),
            feed,
            events: Some(rx),
            cancel,
            started,
            sequence: 0,
        });
        Ok(())
    }

    fn with<T>(
        &self,
        session_id: &str,
        f: impl FnOnce(&mut Active) -> Result<T, DictationRejection>,
    ) -> Result<T, DictationRejection> {
        let mut active = self.active.lock().unwrap();
        match active.as_mut() {
            Some(current) if current.session_id == session_id => f(current),
            _ => Err(DictationRejection::Protocol),
        }
    }

    /// More audio, in order.
    pub fn feed(&self, chunk: &wire::Chunk) -> Result<(), DictationRejection> {
        use base64::Engine as _;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&chunk.data)
            .map_err(|_| DictationRejection::Protocol)?;
        if bytes.is_empty() || bytes.len() > wire::MAX_CHUNK_BYTES || !bytes.len().is_multiple_of(2)
        {
            return Err(DictationRejection::Overflow);
        }
        self.with(&chunk.session_id, |active| {
            if chunk.sequence <= active.sequence {
                return Err(DictationRejection::Protocol);
            }
            active.sequence = chunk.sequence;
            active.feed.push(&bytes);
            Ok(())
        })
    }

    /// No more audio: what arrived is transcribed.
    pub fn finish(&self, session_id: &str) -> Result<(), DictationRejection> {
        self.with(session_id, |active| {
            active.feed.end();
            Ok(())
        })
    }

    /// Drops the dictation and anything it still owed.
    pub fn cancel(&self, session_id: &str) -> Result<(), DictationRejection> {
        let mut active = self.active.lock().unwrap();
        if let Some(current) = active.as_ref().filter(|a| a.session_id == session_id) {
            current.cancel.store(true, Ordering::Release);
            *active = None;
        }
        Ok(())
    }

    /// The dictation's events, once.
    pub fn own(
        &self,
        session_id: &str,
    ) -> Result<mpsc::Receiver<DictationEvent>, DictationRejection> {
        self.with(session_id, |active| {
            active.events.take().ok_or(DictationRejection::Protocol)
        })
    }
}
