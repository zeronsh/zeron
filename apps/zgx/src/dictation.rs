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
static DOWNLOADING: AtomicBool = AtomicBool::new(false);
struct DownloadGuard;
impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, Ordering::Release);
    }
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
    pub fn needs_poll(&self) -> bool {
        self.session.is_some() || self.download.is_some()
    }
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
    pub fn target(&mut self, id: &str) {
        if self.session.is_some() {
            self.state["target"] = json!(id);
        }
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
        let can_claim = matches!(op, "start" | "download" | "remove")
            && self.session.is_none()
            && self.download.is_none();
        if self.owner.as_deref().is_some_and(|id| id != owner) && !can_claim {
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
                    return Ok(json!(
                        catalog::installed(&self.root, id).map_err(|e| e.to_string())?
                    ));
                }
                if self.session.is_some()
                    || self.download.is_some()
                    || zeron_voice::busy()
                    || DOWNLOADING.load(Ordering::Acquire)
                {
                    return Err("Dictation is busy; try again when it finishes".into());
                }
                self.owner = Some(owner.into());
                self.state = json!({"phase":"idle"});
                if op == "remove" {
                    catalog::remove(&self.root, id).map_err(|e| e.to_string())?;
                    self.phase("idle");
                } else if op == "download" {
                    if DOWNLOADING
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_err()
                    {
                        return Err("Another window is downloading a model".into());
                    }
                    let guard = DownloadGuard;
                    let cancel = Arc::new(AtomicBool::new(false));
                    let worker_cancel = cancel.clone();
                    let (tx, events) = mpsc::channel();
                    let (root, id) = (self.root.clone(), id.to_string());
                    std::thread::spawn(move || {
                        let _guard = guard;
                        let result = catalog::download(&root, &id, &worker_cancel, |n| {
                            let _ = tx.send(DownloadEvent::Progress(n));
                        })
                        .map_err(|e| e.to_string());
                        let _ = tx.send(DownloadEvent::Done(result));
                    });
                    self.download = Some(Download { cancel, events });
                    self.state =
                        json!({"phase":"downloading", "bytes":0, "total":model.download_bytes});
                } else {
                    if !catalog::installed(&self.root, id).map_err(|e| e.to_string())? {
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
                    self.state = json!({"phase":"starting", "serial":self.serial, "level":0, "hold":args["hold"], "target":args["target"]});
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
                    zeron_voice::Event::Listening => {
                        if self.deadline.is_none() {
                            self.state["phase"] = json!("listening");
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::{Cell, RefCell},
        collections::VecDeque,
        rc::Rc,
    };
    struct Fake {
        events: Rc<RefCell<VecDeque<zeron_voice::Event>>>,
        cancelled: Rc<Cell<bool>>,
        stopped: Rc<Cell<bool>>,
    }
    impl DictationSession for Fake {
        fn poll(&mut self) -> Option<zeron_voice::Event> {
            self.events.borrow_mut().pop_front()
        }
        fn finish(&mut self) {
            self.stopped.set(true);
        }
        fn cancel(&mut self) {
            self.cancelled.set(true);
        }
        fn take_level(&self) -> f32 {
            0.25
        }
    }
    fn service() -> (
        Service,
        Rc<RefCell<VecDeque<zeron_voice::Event>>>,
        Rc<Cell<bool>>,
        Rc<Cell<bool>>,
    ) {
        let events = Rc::new(RefCell::new(VecDeque::new()));
        let cancelled = Rc::new(Cell::new(false));
        let stopped = Rc::new(Cell::new(false));
        let mut s = Service::new(std::env::temp_dir().join("zeron-dictation-tests-unused"));
        s.owner = Some("one".into());
        s.serial = 7;
        s.state = json!({"phase":"starting", "serial":7});
        s.session = Some(Box::new(Fake {
            events: events.clone(),
            cancelled: cancelled.clone(),
            stopped: stopped.clone(),
        }));
        (s, events, cancelled, stopped)
    }
    #[test]
    fn dictation_isolates_leases_and_cancels_without_publishing_stale_text() {
        let (mut s, events, cancelled, _) = service();
        assert!(s.call("two", &json!({"op":"cancel"})).is_err());
        assert!(s.call("two", &json!({"op":"status"})).is_err());
        events.borrow_mut().push_back(zeron_voice::Event::Listening);
        s.poll();
        assert_eq!(s.snapshot()["phase"], "listening");
        s.release();
        assert!(cancelled.get());
        assert_eq!(s.owner(), None);
        events
            .borrow_mut()
            .push_back(zeron_voice::Event::Final("private".into()));
        s.poll();
        assert!(s.snapshot()["text"].is_null());
    }
    #[test]
    fn dictation_stop_waits_for_final_and_timeout_invalidates_results() {
        let (mut s, events, cancelled, stopped) = service();
        s.call("one", &json!({"op":"stop"})).unwrap();
        assert!(stopped.get());
        assert!(!cancelled.get());
        events
            .borrow_mut()
            .push_back(zeron_voice::Event::Final("hola".into()));
        s.poll();
        assert_eq!(s.snapshot()["text"], "hola");
        assert!(s.session.is_none());
        let (mut s, events, cancelled, _) = service();
        s.deadline = Some(Instant::now());
        events
            .borrow_mut()
            .push_back(zeron_voice::Event::Final("too late".into()));
        s.poll();
        assert!(cancelled.get());
        assert_eq!(s.snapshot()["phase"], "failed");
        assert!(s.snapshot()["text"].is_null());
    }
    #[test]
    fn dictation_native_lease_rejects_ime_and_edits_changed_back() {
        let mut app = zgx_script::testing::TestApp::new(|r| {
            zgx_script::ScriptHost::new(
                r,
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/dictation_fixture.luau"),
            )
        });
        app.ui.scene.borrow_mut().resize(960., 700.);
        app.frames(30);
        assert_eq!(app.error(), None);
        app.click("role:textbox \"Message\"");
        let editor = app.ui.focused_editor().unwrap();
        let lease = super::EditorLease::new(editor.clone()).unwrap();
        editor.set_composition("正在", None, &[]);
        assert!(!lease.valid());
        assert!(super::EditorLease::new(editor.clone()).is_none());
        editor.set_composition("", None, &[]);
        let lease = super::EditorLease::new(editor.clone()).unwrap();
        editor.paste("x");
        editor.set_selection(0, 1);
        editor.paste("");
        assert_eq!(editor.editor.borrow().text().as_ref(), "");
        assert!(!lease.valid(), "revision protects against edit-and-revert");
    }

    #[test]
    fn dictation_rejects_unknown_models_before_touching_audio_or_disk() {
        let mut s = Service::new(PathBuf::from("/unused"));
        for op in ["start", "download", "remove", "installed"] {
            assert!(
                s.call("one", &json!({"op":op, "model":"../../other"}))
                    .is_err()
            );
        }
        assert!(s.owner().is_none());
    }
}

/// A native editor lease catches edits changed back before a script effect,
/// and uncommitted IME composition. It never crosses into a plugin VM.
pub struct EditorLease {
    editor: zgx::widgets::EditorHandle,
    revision: u64,
    selection: (usize, usize),
}
impl EditorLease {
    pub fn new(editor: zgx::widgets::EditorHandle) -> Option<Self> {
        if editor.is_read_only() {
            return None;
        }
        let revision = {
            let model = editor.editor.borrow();
            if model.preedit().is_some() {
                return None;
            }
            model.revision()
        };
        let selection = editor.selection();
        Some(Self {
            editor,
            revision,
            selection,
        })
    }
    pub fn valid(&self) -> bool {
        let model = self.editor.editor.borrow();
        !self.editor.is_read_only()
            && model.revision() == self.revision
            && model.preedit().is_none()
            && self.editor.selection() == self.selection
    }
}
