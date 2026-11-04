//! Window-local dictation. No engine RPC, audio files, or transcript logging.
//! Plugins own an exclusive lease; losing the lease cancels capture and downloads.
use serde_json::{Value as Json, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use zeron_voice::{backend::DictationSession, catalog};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn zeron_microphone_permission() -> i32;
    fn zeron_request_microphone();
}
fn authorize() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    match unsafe { zeron_microphone_permission() } {
        1 => (),
        0 => {
            unsafe { zeron_request_microphone() };
            return Err("Allow microphone access, then hold to dictate again.".into());
        }
        -2 => return Err("Microphone access requires a packaged Zeron app.".into()),
        _ => return Err(
            "Allow Zeron microphone access in System Settings → Privacy & Security → Microphone."
                .into(),
        ),
    }
    Ok(())
}
enum DownloadEvent {
    Progress(u64),
    Done(Result<(), String>),
}
struct Download {
    cancel: Arc<AtomicBool>,
    events: mpsc::Receiver<DownloadEvent>,
}
impl Drop for Download {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

pub struct Service {
    root: PathBuf,
    owner: Option<String>,
    session: Option<Box<dyn DictationSession>>,
    download: Option<Download>,
    deadline: Option<Instant>,
    state: Json,
    serial: u64,
}
impl Service {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            owner: None,
            session: None,
            download: None,
            deadline: None,
            state: json!({"phase":"idle"}),
            serial: 0,
        }
    }
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
    pub fn snapshot(&self) -> Json {
        self.state.clone()
    }
    fn phase(&mut self, phase: &str) {
        self.state["phase"] = json!(phase);
    }
    pub fn cancel(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.cancel();
        }
        if let Some(job) = &self.download {
            job.cancel.store(true, Ordering::Release);
        }
        self.deadline = None;
        self.state = json!({"phase":"cancelled", "serial":self.serial});
    }
    /// Called before unloading or replacing a plugin, even at the same id.
    pub fn release(&mut self) {
        self.cancel();
        self.owner = None;
    }
    pub fn call(&mut self, owner: &str, args: &Json) -> Result<Json, String> {
        let op = args["op"].as_str().unwrap_or("");
        // A different plugin cannot stop, read, or alter another one's lease.
        if self.owner.as_deref().is_some_and(|id| id != owner) {
            return Err("Another plugin owns dictation in this window".into());
        }
        match op {
            "catalog" => return Ok(json!(catalog::models())),
            "status" => return Ok(self.snapshot()),
            "cancel" => self.cancel(),
            "stop" => {
                if let Some(s) = &mut self.session {
                    s.finish();
                    if self.deadline.is_none() {
                        self.deadline = Some(Instant::now() + Duration::from_secs(30));
                    }
                    self.phase("finalizing");
                }
            }
            "start" | "download" | "remove" | "installed" => {
                let id = args["model"].as_str().ok_or("Choose a dictation model")?;
                let model = catalog::model(id).map_err(|e| e.to_string())?;
                let dir = catalog::directory(&self.root, id).map_err(|e| e.to_string())?;
                if op == "installed" {
                    return Ok(json!(zeron_voice::installed(&dir)));
                }
                if self.session.is_some() || self.download.is_some() || zeron_voice::busy() {
                    return Err("Dictation is busy; try again when it finishes".into());
                }
                self.owner = Some(owner.into());
                if op == "remove" {
                    catalog::remove(&self.root, id).map_err(|e| e.to_string())?;
                    self.phase("idle");
                } else if op == "download" {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let worker_cancel = cancel.clone();
                    let (tx, events) = mpsc::channel();
                    std::thread::spawn(move || {
                        let result = zeron_voice::download(&dir, &worker_cancel, |n| {
                            let _ = tx.send(DownloadEvent::Progress(n));
                        })
                        .map_err(|e| e.to_string());
                        let _ = tx.send(DownloadEvent::Done(result));
                    });
                    self.download = Some(Download { cancel, events });
                    self.state =
                        json!({"phase":"downloading", "bytes":0, "total":model.download_bytes});
                } else {
                    if !zeron_voice::installed(&dir) {
                        return Err("Download the dictation model in Settings first".into());
                    }
                    authorize()?;
                    let backend = catalog::backend(model.backend).map_err(|e| e.to_string())?;
                    let device = args["device"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    self.session = Some(backend.start(dir, device).map_err(|e| e.to_string())?);
                    self.serial += 1;
                    self.state = json!({"phase":"starting", "serial":self.serial, "level":0});
                }
            }
            _ => return Err("Unknown dictation operation".into()),
        }
        Ok(self.snapshot())
    }
    pub fn poll(&mut self) {
        let mut done = None;
        if let Some(job) = &self.download {
            while let Ok(event) = job.events.try_recv() {
                match event {
                    DownloadEvent::Progress(n) => self.state["bytes"] = json!(n),
                    DownloadEvent::Done(result) => done = Some(result),
                }
            }
        }
        if let Some(result) = done {
            let cancelled = self
                .download
                .as_ref()
                .is_some_and(|j| j.cancel.load(Ordering::Acquire));
            self.download = None;
            match result {
                _ if cancelled => self.phase("cancelled"),
                Ok(()) => self.phase("ready"),
                Err(e) => {
                    self.phase("failed");
                    self.state["error"] = json!(e);
                }
            }
        }
        if self.deadline.is_some_and(|end| Instant::now() >= end) {
            self.cancel();
            self.phase("failed");
            self.state["error"] = json!("Transcription timed out. Your draft is safe.");
        }
        let mut terminal = false;
        if let Some(session) = &mut self.session {
            self.state["level"] = json!(session.take_level());
            while let Some(event) = session.poll() {
                match event {
                    zeron_voice::Event::Listening => self.state["phase"] = json!("listening"),
                    zeron_voice::Event::Finalizing => {
                        self.state["phase"] = json!("finalizing");
                        self.deadline
                            .get_or_insert_with(|| Instant::now() + Duration::from_secs(30));
                    }
                    zeron_voice::Event::Final(text) => {
                        self.state["phase"] = json!("final");
                        self.state["text"] = json!(text);
                        terminal = true;
                    }
                    zeron_voice::Event::Failed(error) => {
                        self.state["phase"] = json!("failed");
                        self.state["error"] = json!(error);
                        terminal = true;
                    }
                }
            }
        }
        if terminal {
            self.session = None;
            self.deadline = None;
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.cancel();
    }
}
