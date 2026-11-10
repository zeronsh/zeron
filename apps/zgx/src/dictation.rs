//! Dictation: hold to talk, release to transcribe, the words in the draft
//! (ui/screens/chat/dictation.luau, Settings → Voice). Everything happens in
//! this window with `zeron_voice`: no engine, audio files or transcript logs.
//! The weights stay in `<data>/models`, where the shipping app keeps them too.
use serde_json::{Value as Json, json};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

use crate::plugins::{json_value, off_thread, value_json};

mod meter;
use meter::Meter;

const MODEL: &str = "parakeet-tdt-0.6b-v3-int8";

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

/// One recording. Dropping it stops capture and throws away its result.
trait Recording {
    fn poll(&mut self) -> Option<zeron_voice::Event>;
    fn finish(&mut self);
    /// Peak microphone RMS since the previous take.
    fn take_level(&self) -> f32;
}
impl Recording for zeron_voice::Session {
    fn poll(&mut self) -> Option<zeron_voice::Event> {
        self.poll()
    }
    fn finish(&mut self) {
        self.finish()
    }
    fn take_level(&self) -> f32 {
        self.take_level()
    }
}

/// One download at a time, across windows.
static DOWNLOADING: AtomicBool = AtomicBool::new(false);
enum Download {
    Progress(u64),
    Done(Result<(), String>),
}
struct DownloadJob {
    cancel: Arc<AtomicBool>,
    events: mpsc::Receiver<Download>,
}
impl Drop for DownloadJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

struct Service {
    /// The model's folder (none: the data folder isn't known).
    dir: Option<PathBuf>,
    session: Option<Box<dyn Recording>>,
    download: Option<DownloadJob>,
    deadline: Option<Instant>,
    state: Json,
    serial: u64,
    /// The voice bar's waveform while listening, and whether its bars
    /// changed since they were last published (`dictation_levels`).
    meter: Meter,
    levels_changed: bool,
}
impl Service {
    fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            session: None,
            download: None,
            deadline: None,
            state: json!({"phase":"idle"}),
            serial: 0,
            meter: Meter::default(),
            levels_changed: false,
        }
    }
    fn needs_poll(&self) -> bool {
        self.session.is_some() || self.download.is_some()
    }
    fn clear_levels(&mut self) {
        self.meter.reset();
        self.levels_changed = true;
    }
    /// The waveform's bars, newest first, once each time they change.
    fn take_levels(&mut self) -> Option<Vec<f32>> {
        std::mem::take(&mut self.levels_changed).then(|| self.meter.newest())
    }
    /// How long the poll loop waits: to the next slot while listening (the
    /// bars move once a slot), else its usual beat.
    fn poll_wait(&self) -> Duration {
        const BEAT: Duration = Duration::from_millis(40);
        self.meter
            .until_next(Instant::now())
            .map_or(BEAT, |d| (d + Duration::from_millis(1)).min(BEAT))
    }
    fn cancel(&mut self) {
        self.session = None;
        // (What was drawn stays until the next recording: the bar is on
        // its way out.)
        self.meter.reset();
        if let Some(job) = &self.download {
            job.cancel.store(true, Ordering::Release);
        }
        self.deadline = None;
        self.state = json!({"phase":"cancelled", "serial":self.serial});
    }
    fn stop(&mut self) {
        if let Some(session) = &mut self.session {
            session.finish();
            self.meter.freeze(Instant::now());
            self.deadline
                .get_or_insert_with(|| Instant::now() + Duration::from_secs(30));
            self.state["phase"] = json!("finalizing");
        }
    }
    fn dir(&self) -> Result<PathBuf, String> {
        self.dir
            .clone()
            .ok_or_else(|| "Zeron's data folder isn't known".to_string())
    }
    fn idle(&self) -> Result<(), String> {
        if self.needs_poll() || zeron_voice::busy() || DOWNLOADING.load(Ordering::Acquire) {
            return Err("Dictation is busy; try again when it finishes".into());
        }
        Ok(())
    }
    fn status(&self) -> Result<Json, String> {
        Ok(json!({
            "installed": zeron_voice::installed(&self.dir()?),
            "size": zeron_voice::download_size(),
        }))
    }
    fn start(&mut self, args: &Json) -> Result<Json, String> {
        let dir = self.dir()?;
        self.idle()?;
        // (The page says what to do about it: no words for the composer.)
        if !zeron_voice::installed(&dir) {
            return Err("no_model".into());
        }
        authorize()?;
        let device = args["device"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let session = zeron_voice::Session::start(dir, device).map_err(|e| e.to_string())?;
        self.session = Some(Box::new(session));
        self.clear_levels();
        self.serial += 1;
        self.state = json!({"phase":"starting", "serial":self.serial, "hold":args["hold"], "target":args["target"]});
        Ok(self.state.clone())
    }
    fn download(&mut self) -> Result<Json, String> {
        let dir = self.dir()?;
        self.idle()?;
        if DOWNLOADING.swap(true, Ordering::AcqRel) {
            return Err("Another window is downloading the model".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (tell, events) = mpsc::channel();
        std::thread::spawn(move || {
            let result = zeron_voice::download(&dir, &worker_cancel, |n| {
                let _ = tell.send(Download::Progress(n));
            })
            .map_err(|e| e.to_string());
            DOWNLOADING.store(false, Ordering::Release);
            let _ = tell.send(Download::Done(result));
        });
        self.download = Some(DownloadJob { cancel, events });
        self.state =
            json!({"phase":"downloading", "bytes":0, "total":zeron_voice::download_size()});
        Ok(self.state.clone())
    }
    fn remove(&mut self) -> Result<(), String> {
        let dir = self.dir()?;
        self.idle()?;
        zeron_voice::unload();
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
        }
        self.state = json!({"phase":"idle"});
        Ok(())
    }
    fn poll(&mut self) {
        let mut done = None;
        if let Some(job) = &self.download {
            while let Ok(event) = job.events.try_recv() {
                match event {
                    Download::Progress(n) => self.state["bytes"] = json!(n),
                    Download::Done(result) => done = Some(result),
                }
            }
        }
        if let Some(result) = done {
            let cancelled = self
                .download
                .take()
                .is_some_and(|job| job.cancel.load(Ordering::Acquire));
            match result {
                _ if cancelled => self.state["phase"] = json!("cancelled"),
                Ok(()) => self.state["phase"] = json!("ready"),
                Err(e) => {
                    self.state["phase"] = json!("download_failed");
                    self.state["error"] = json!(e);
                }
            }
        }
        if self.deadline.is_some_and(|end| Instant::now() >= end) {
            self.cancel();
            self.state["phase"] = json!("failed");
            self.state["error"] = json!("Transcription timed out. Your draft is safe.");
        }
        let mut ended = false;
        if let Some(session) = &mut self.session {
            while let Some(event) = session.poll() {
                match event {
                    zeron_voice::Event::Listening => {
                        if self.deadline.is_none() {
                            self.state["phase"] = json!("listening");
                            self.meter.start(Instant::now());
                            session.take_level();
                        }
                    }
                    zeron_voice::Event::Finalizing => {
                        self.state["phase"] = json!("finalizing");
                        self.meter.freeze(Instant::now());
                        self.deadline
                            .get_or_insert_with(|| Instant::now() + Duration::from_secs(30));
                    }
                    zeron_voice::Event::Final(text) => {
                        self.state["phase"] = json!("final");
                        self.state["text"] = json!(text);
                        ended = true;
                    }
                    zeron_voice::Event::Failed(error) => {
                        self.state["phase"] = json!("failed");
                        self.state["error"] = json!(error);
                        ended = true;
                    }
                }
            }
            if !ended && self.meter.record(Instant::now(), session.take_level()) {
                self.levels_changed = true;
            }
        }
        if ended {
            self.session = None;
            self.deadline = None;
            self.meter.reset();
        }
    }
}

/// The draft a recording belongs to, as it was when it began. Catches an edit
/// changed back before a script's effect runs, and IME composition.
struct EditorLease {
    editor: zgx::widgets::EditorHandle,
    revision: u64,
    selection: (usize, usize),
}
impl EditorLease {
    fn new(editor: zgx::widgets::EditorHandle) -> Option<Self> {
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
    fn valid(&self) -> bool {
        let model = self.editor.editor.borrow();
        !self.editor.is_read_only()
            && model.revision() == self.revision
            && model.preedit().is_none()
            && self.editor.selection() == self.selection
    }
}

struct Dictation {
    service: RefCell<Service>,
    lease: RefCell<Option<EditorLease>>,
    shared: zgx_script::Shared,
    /// The voice bar's bars, newest first (0–1), on their own so readers of
    /// `dictation` don't run again at each slot.
    levels: zgx_script::Shared,
    wake: zgx::task::LatestSender<()>,
}
impl Dictation {
    fn publish(&self) {
        let now = json_value(&self.service.borrow().state);
        if self.shared.signal().with_untracked(Value::clone) != now {
            self.shared.set(now);
        }
        let levels = self.service.borrow_mut().take_levels();
        if let Some(levels) = levels {
            self.levels.set(levels_value(&levels));
        }
    }
}

/// The bars as Luau reads them, to the thousandth (finer is never drawn).
fn levels_value(levels: &[f32]) -> Value {
    let rounded: Vec<f64> = levels
        .iter()
        .map(|a| (f64::from(*a) * 1000.0).round() / 1000.0)
        .collect();
    json_value(&json!(rounded))
}

/// `dictation.call { op, … }` and `dictation.guard`, for the window's scripts;
/// what's happening in the shared `dictation`.
pub fn install(cx: &mut WindowContext, host: &ScriptHost) {
    let (wake, mut work) = zgx::task::latest::<()>();
    let dir = crate::engine::data_dir().map(|d| d.join("models").join(MODEL));
    let me = Rc::new(Dictation {
        shared: host.shared("dictation", json_value(&json!({"phase":"idle"}))),
        levels: host.shared("dictation_levels", json_value(&json!([]))),
        service: RefCell::new(Service::new(dir)),
        lease: RefCell::default(),
        wake,
    });
    let weak = Rc::downgrade(&me);
    host.async_action("dictation.call", move |asked: Value| {
        let weak = weak.clone();
        async move {
            let me = weak.upgrade().ok_or("The window closed")?;
            let args = value_json(&asked);
            let result = match args["op"].as_str() {
                Some("inputs") => {
                    let inputs = off_thread(zeron_voice::input_devices)
                        .await
                        .ok_or("Could not list microphones")?;
                    let list: Vec<_> = inputs
                        .into_iter()
                        .map(|d| json!({"id":d.id, "name":d.name}))
                        .collect();
                    return Ok(json_value(&json!(list)));
                }
                Some("status") => me.service.borrow().status()?,
                Some("start") => me.service.borrow_mut().start(&args)?,
                Some("download") => me.service.borrow_mut().download()?,
                Some("remove") => {
                    me.service.borrow_mut().remove()?;
                    Json::Null
                }
                Some("stop") => {
                    me.service.borrow_mut().stop();
                    Json::Null
                }
                Some("cancel") => {
                    me.lease.borrow_mut().take();
                    me.service.borrow_mut().cancel();
                    Json::Null
                }
                _ => return Err("Unknown dictation operation".to_string()),
            };
            me.wake.send(());
            me.publish();
            Ok(json_value(&result))
        }
    });
    let weak = Rc::downgrade(&me);
    let ui = cx.ui.downgrade();
    host.action("dictation.guard", move |asked: Value| {
        let Some(me) = weak.upgrade() else { return };
        let lease = ui
            .upgrade()
            .and_then(|ui| ui.focused_editor())
            .and_then(EditorLease::new);
        if lease.is_none() {
            me.service.borrow_mut().cancel();
        } else if let Some(target) = value_json(&asked)["target"].as_str() {
            if me.service.borrow().session.is_some() {
                me.service.borrow_mut().state["target"] = json!(target);
            }
        }
        *me.lease.borrow_mut() = lease;
        me.publish();
    });
    // The window's tasks keep it, so closing the window ends a recording.
    // No timer while nothing is recording or downloading.
    cx.tasks.spawn(async move {
        while work.recv().await.is_some() {
            while me.service.borrow().needs_poll() {
                if me.lease.borrow().as_ref().is_some_and(|l| !l.valid()) {
                    me.lease.borrow_mut().take();
                    me.service.borrow_mut().cancel();
                }
                me.service.borrow_mut().poll();
                me.publish();
                let wait = me.service.borrow().poll_wait();
                zgx::timer::sleep(wait).await;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};
    use zeron_voice::Event;
    struct Fake {
        events: Rc<RefCell<VecDeque<Event>>>,
        cancelled: Rc<Cell<bool>>,
        stopped: Rc<Cell<bool>>,
        level: Rc<Cell<f32>>,
    }
    impl Recording for Fake {
        fn poll(&mut self) -> Option<Event> {
            self.events.borrow_mut().pop_front()
        }
        fn finish(&mut self) {
            self.stopped.set(true);
        }
        fn take_level(&self) -> f32 {
            self.level.replace(0.0)
        }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            self.cancelled.set(true);
        }
    }
    fn service() -> (
        Service,
        Rc<RefCell<VecDeque<Event>>>,
        Rc<Cell<bool>>,
        Rc<Cell<bool>>,
    ) {
        let events = Rc::new(RefCell::new(VecDeque::new()));
        let cancelled = Rc::new(Cell::new(false));
        let stopped = Rc::new(Cell::new(false));
        let mut s = Service::new(None);
        s.serial = 7;
        s.state = json!({"phase":"starting", "serial":7});
        s.session = Some(Box::new(Fake {
            events: events.clone(),
            cancelled: cancelled.clone(),
            stopped: stopped.clone(),
            level: Rc::default(),
        }));
        (s, events, cancelled, stopped)
    }
    /// Listening, with the microphone's level to set.
    fn metered() -> (Service, Rc<RefCell<VecDeque<Event>>>, Rc<Cell<f32>>) {
        let events = Rc::new(RefCell::new(VecDeque::new()));
        let level = Rc::new(Cell::new(0.0));
        let mut s = Service::new(None);
        s.state = json!({"phase":"starting", "serial":1});
        s.session = Some(Box::new(Fake {
            events: events.clone(),
            cancelled: Rc::default(),
            stopped: Rc::default(),
            level: level.clone(),
        }));
        (s, events, level)
    }
    #[test]
    fn dictation_levels_follow_the_microphone_at_slot_rate_while_listening() {
        let (mut s, events, level) = metered();
        // Starting: the microphone isn't live, no bars.
        level.set(0.3);
        s.poll();
        assert_eq!(s.take_levels(), None);
        assert_eq!(s.poll_wait(), Duration::from_millis(40));
        events.borrow_mut().push_back(Event::Listening);
        s.poll();
        // The first slot, at once (what came before listening isn't in it).
        assert_eq!(s.take_levels(), Some(vec![0.0]));
        // Within the slot nothing is published again, and the loop waits
        // for the next slot rather than its usual beat.
        level.set(0.1);
        s.poll();
        assert_eq!(s.take_levels(), None);
        let wait = s.poll_wait();
        assert!(wait <= Duration::from_millis(61) && wait > Duration::ZERO, "{wait:?}");
        std::thread::sleep(meter::STEP);
        s.poll();
        let bars = s.take_levels().unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0], meter::normalize(0.1));
        // Finalizing holds them: no more slots.
        events.borrow_mut().push_back(Event::Finalizing);
        s.poll();
        let _ = s.take_levels();
        level.set(0.5);
        std::thread::sleep(meter::STEP);
        s.poll();
        assert_eq!(s.take_levels(), None);
        assert_eq!(s.poll_wait(), Duration::from_millis(40));
    }
    #[test]
    fn dictation_cancel_stops_capture_and_publishes_no_stale_text() {
        let (mut s, events, cancelled, _) = service();
        events.borrow_mut().push_back(Event::Listening);
        s.poll();
        assert_eq!(s.state["phase"], "listening");
        s.cancel();
        assert!(cancelled.get());
        events.borrow_mut().push_back(Event::Final("private".into()));
        s.poll();
        assert!(s.state["text"].is_null());
    }
    #[test]
    fn dictation_stop_waits_for_final_and_timeout_invalidates_results() {
        let (mut s, events, cancelled, stopped) = service();
        s.stop();
        assert!(stopped.get());
        assert!(!cancelled.get());
        assert_eq!(s.state["phase"], "finalizing");
        events.borrow_mut().push_back(Event::Final("hola".into()));
        s.poll();
        assert_eq!(s.state["text"], "hola");
        assert!(s.session.is_none());
        let (mut s, events, cancelled, _) = service();
        s.deadline = Some(Instant::now());
        events.borrow_mut().push_back(Event::Final("too late".into()));
        s.poll();
        assert!(cancelled.get());
        assert_eq!(s.state["phase"], "failed");
        assert!(s.state["text"].is_null());
    }
    #[test]
    fn dictation_without_a_data_folder_says_so_and_never_panics() {
        let mut s = Service::new(None);
        assert!(s.status().is_err());
        assert!(s.start(&json!({})).is_err());
        assert!(s.download().is_err());
        assert!(s.remove().is_err());
    }
    #[test]
    fn dictation_without_the_model_asks_for_it_before_touching_the_microphone() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Service::new(Some(dir.path().join(MODEL)));
        assert_eq!(s.status().unwrap()["installed"], false);
        assert_eq!(s.start(&json!({})).unwrap_err(), "no_model");
        assert!(s.session.is_none());
    }
    #[test]
    fn dictation_lease_rejects_ime_and_edits_changed_back() {
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
        let lease = EditorLease::new(editor.clone()).unwrap();
        editor.set_composition("正在", None, &[]);
        assert!(!lease.valid());
        assert!(EditorLease::new(editor.clone()).is_none());
        editor.set_composition("", None, &[]);
        let lease = EditorLease::new(editor.clone()).unwrap();
        editor.paste("x");
        editor.set_selection(0, 1);
        editor.paste("");
        assert_eq!(editor.editor.borrow().text().as_ref(), "");
        assert!(!lease.valid(), "revision protects against edit-and-revert");
    }
}
