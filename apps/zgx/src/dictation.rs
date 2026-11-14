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

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn zeron_microphone_permission() -> i32;
    fn zeron_request_microphone();
}
fn authorize() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    match unsafe { zeron_microphone_permission() } {
        1 => (),
        // First use (or, for a dev build, each new signature): macOS asks,
        // and the page says what to do, not that something failed.
        0 => {
            unsafe { zeron_request_microphone() };
            return Err("permission_asked".into());
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
    /// `<data>/models`, a folder a model (none: the data folder isn't known).
    models: Option<PathBuf>,
    session: Option<Box<dyn Recording>>,
    /// Opens a recording; `Err(None)`: the worker is still finishing the
    /// last one (a cancelled recording's model still loading).
    open: Box<dyn Fn(PathBuf, Option<String>) -> Result<Box<dyn Recording>, Option<String>>>,
    /// A start waiting for the worker: its folder, microphone, and when it
    /// gives up. Meanwhile the bar keeps its starting look.
    waiting: Option<(PathBuf, Option<String>, Instant)>,
    download: Option<DownloadJob>,
    deadline: Option<Instant>,
    /// When the recording began to listen, and whether its model is slow
    /// here (Cohere on the CPU): how long finalizing may take.
    listening: Option<Instant>,
    slow: bool,
    state: Json,
    serial: u64,
    /// The voice bar's waveform while listening, and whether its bars
    /// changed since they were last published (`dictation_levels`).
    meter: Meter,
    levels_changed: bool,
}
impl Service {
    fn new(models: Option<PathBuf>) -> Self {
        Self {
            models,
            session: None,
            open: Box::new(|dir, device| match zeron_voice::Session::start(dir, device) {
                Ok(session) => Ok(Box::new(session) as Box<dyn Recording>),
                Err(_) if zeron_voice::busy() => Err(None),
                Err(e) => Err(Some(e.to_string())),
            }),
            waiting: None,
            download: None,
            deadline: None,
            listening: None,
            slow: false,
            state: json!({"phase":"idle"}),
            serial: 0,
            meter: Meter::default(),
            levels_changed: false,
        }
    }
    fn needs_poll(&self) -> bool {
        self.session.is_some() || self.download.is_some() || self.waiting.is_some()
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
        self.waiting = None;
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
        // Let go before the microphone opened: nothing was recorded.
        if self.waiting.take().is_some() {
            self.state = json!({"phase":"cancelled", "serial":self.serial});
            return;
        }
        if let Some(session) = &mut self.session {
            session.finish();
            self.meter.freeze(Instant::now());
            self.state["phase"] = json!("finalizing");
            self.finalizing();
        }
    }
    /// The words are due: 30 s, plus as long again as the recording (three
    /// times for a model slow here), from now. Past it the person is told,
    /// and words that still come land all the same.
    fn finalizing(&mut self) {
        let recorded = self.listening.map_or(Duration::ZERO, |at| at.elapsed());
        let per_second = if self.slow { 3 } else { 1 };
        self.deadline
            .get_or_insert_with(|| Instant::now() + Duration::from_secs(30) + recorded * per_second);
    }
    /// The folder of the model `args` names (`model`, an id; none or empty:
    /// the default). One it doesn't offer is an error, never another's folder.
    fn dir(&self, args: &Json) -> Result<PathBuf, String> {
        let models_dir = self.models.as_ref().ok_or("Zeron's data folder isn't known")?;
        let id = match args["model"].as_str().filter(|id| !id.is_empty()) {
            Some(id) => zeron_voice::models()
                .into_iter()
                .find(|m| m.id == id)
                .ok_or("No such speech model")?
                .id,
            None => zeron_voice::manifest().id,
        };
        Ok(models_dir.join(id))
    }
    fn idle(&self) -> Result<(), String> {
        if self.needs_poll() || zeron_voice::busy() || DOWNLOADING.load(Ordering::Acquire) {
            return Err("Dictation is busy; try again when it finishes".into());
        }
        Ok(())
    }
    /// The models on offer, the default first, and which are here.
    fn status(&self) -> Result<Json, String> {
        let models = self.models.as_ref().ok_or("Zeron's data folder isn't known")?;
        let list: Vec<Json> = zeron_voice::models()
            .into_iter()
            .map(|m| {
                json!({"id":m.id, "name":m.name, "size":m.size(), "languages":m.languages,
                    "installed":m.installed(&models.join(&m.id)),
                    "present":models.join(&m.id).exists(),
                    "memory":m.memory,
                    "slow":m.cpu_slow && cfg!(not(target_os = "macos"))})
            })
            .collect();
        Ok(json!({ "models": list, "supported": zeron_voice::cpu_supported() }))
    }
    fn start(&mut self, args: &Json) -> Result<Json, String> {
        let dir = self.dir(args)?;
        if !zeron_voice::cpu_supported() {
            return Err(zeron_voice::CPU_UNSUPPORTED.into());
        }
        if self.needs_poll() || DOWNLOADING.load(Ordering::Acquire) {
            return Err("Dictation is busy; try again when it finishes".into());
        }
        // (The page says what to do about it: no words for the composer.)
        if !zeron_voice::installed(&dir) {
            return Err("no_model".into());
        }
        authorize()?;
        let device = args["device"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        self.slow = zeron_voice::model_for(&dir).is_some_and(|m| m.cpu_slow)
            && cfg!(not(target_os = "macos"));
        // The worker still finishing the last recording (a tap or a cancel
        // while its model loaded) isn't an error: this one starts as soon
        // as it's done, the bar in its starting look meanwhile.
        match (self.open)(dir.clone(), device.clone()) {
            Ok(session) => self.session = Some(session),
            Err(None) => {
                self.waiting = Some((dir, device, Instant::now() + Duration::from_secs(15)))
            }
            Err(Some(e)) => return Err(e),
        }
        self.listening = None;
        self.clear_levels();
        self.serial += 1;
        self.state = json!({"phase":"starting", "serial":self.serial, "hold":args["hold"], "target":args["target"]});
        Ok(self.state.clone())
    }
    fn download(&mut self, args: &Json) -> Result<Json, String> {
        let dir = self.dir(args)?;
        if !zeron_voice::cpu_supported() {
            return Err(zeron_voice::CPU_UNSUPPORTED.into());
        }
        let total = zeron_voice::model_for(&dir).map_or(0, |m| m.size());
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
            json!({"phase":"downloading", "bytes":0, "total":total});
        Ok(self.state.clone())
    }
    fn remove(&mut self, args: &Json) -> Result<(), String> {
        let dir = self.dir(args)?;
        self.idle()?;
        zeron_voice::unload();
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|e| e.to_string())?;
        }
        self.state = json!({"phase":"idle"});
        Ok(())
    }
    fn poll(&mut self) {
        if let Some((dir, device, until)) = self.waiting.take() {
            match (self.open)(dir.clone(), device.clone()) {
                Ok(session) => self.session = Some(session),
                Err(None) if Instant::now() < until => self.waiting = Some((dir, device, until)),
                Err(e) => {
                    self.state["phase"] = json!("failed");
                    self.state["error"] = json!(e.unwrap_or_else(|| {
                        "Dictation is still finishing the last recording. Try again in a moment."
                            .into()
                    }));
                }
            }
        }
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
        let mut ended = false;
        let mut due = false;
        if let Some(session) = &mut self.session {
            while let Some(event) = session.poll() {
                match event {
                    zeron_voice::Event::Listening => {
                        if self.deadline.is_none() {
                            self.listening = Some(Instant::now());
                            self.state["phase"] = json!("listening");
                            self.meter.start(Instant::now());
                            session.take_level();
                        }
                    }
                    zeron_voice::Event::Finalizing => {
                        self.state["phase"] = json!("finalizing");
                        self.meter.freeze(Instant::now());
                        due = true;
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
        if due {
            self.finalizing();
        }
        // Overdue: said, once; the session goes on, and its words, if they
        // come, land as ever (`late`).
        if !ended && self.deadline.is_some_and(|end| Instant::now() >= end) {
            self.deadline = None;
            self.state["phase"] = json!("failed");
            self.state["late"] = json!(true);
            self.state["error"] = json!("Transcription is taking long. The words will land if they come.");
        }
        if ended {
            self.session = None;
            self.deadline = None;
            self.meter.reset();
        }
    }
}

/// The draft a recording belongs to. Typing on in it is fine (the words
/// land at the caret); it ends as the editor can't take them any more.
struct EditorLease {
    editor: zgx::widgets::EditorHandle,
}
impl EditorLease {
    /// None mid-composition (an IME's): a recording starts on settled text.
    fn new(editor: zgx::widgets::EditorHandle) -> Option<Self> {
        if editor.is_read_only() || editor.editor.borrow().preedit().is_some() {
            return None;
        }
        Some(Self { editor })
    }
    fn valid(&self) -> bool {
        !self.editor.is_read_only()
    }
}
/// Whether `editor` is mid-composition (an IME's): the words wait for it to
/// end, as inserting would throw the composition away.
fn composing(editor: Option<&zgx::widgets::EditorHandle>) -> Value {
    Value::Bool(editor.is_some_and(|e| e.editor.borrow().preedit().is_some()))
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
    let models = crate::engine::data_dir().map(|d| d.join("models"));
    static RETIRED: std::sync::Once = std::sync::Once::new();
    if let Some(models) = models.clone() {
        RETIRED.call_once(|| {
            std::thread::spawn(move || zeron_voice::retire_old_models(&models));
        });
    }

    let me = Rc::new(Dictation {
        shared: host.shared("dictation", json_value(&json!({"phase":"idle"}))),
        levels: host.shared("dictation_levels", json_value(&json!([]))),
        service: RefCell::new(Service::new(models)),
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
                Some("download") => me.service.borrow_mut().download(&args)?,
                Some("remove") => {
                    me.service.borrow_mut().remove(&args)?;
                    Json::Null
                }
                // How long the model stays ready (seconds; 0: released at
                // once; -1: while Zeron is open).
                Some("keep") => {
                    let seconds = args["seconds"].as_i64().unwrap_or(300);
                    zeron_voice::keep_ready(u64::try_from(seconds).ok().map(Duration::from_secs));
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
    let weak = Rc::downgrade(&me);
    host.function("dictation.composing", move |_| {
        let me = weak.upgrade();
        let lease = me.as_ref().map(|me| me.lease.borrow());
        composing(lease.as_ref().and_then(|l| l.as_ref()).map(|l| &l.editor))
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
    fn dictation_stop_waits_for_final_and_late_words_still_land() {
        let (mut s, events, cancelled, stopped) = service();
        s.stop();
        assert!(stopped.get());
        assert!(!cancelled.get());
        assert_eq!(s.state["phase"], "finalizing");
        events.borrow_mut().push_back(Event::Final("hola".into()));
        s.poll();
        assert_eq!(s.state["text"], "hola");
        assert!(s.session.is_none());
        // A minute's recording on a model slow here has 3.5 minutes.
        let (mut s, _, _, _) = service();
        s.listening = Some(Instant::now() - Duration::from_secs(60));
        s.slow = true;
        s.stop();
        let due = s.deadline.unwrap() - Instant::now();
        assert!(due > Duration::from_secs(205) && due < Duration::from_secs(211), "{due:?}");
        // Overdue: said, but the words that come late still land.
        let (mut s, events, cancelled, _) = service();
        s.stop();
        s.deadline = Some(Instant::now());
        s.poll();
        assert!(!cancelled.get());
        assert_eq!(s.state["phase"], "failed");
        assert_eq!(s.state["late"], true);
        events.borrow_mut().push_back(Event::Final("late words".into()));
        s.poll();
        assert_eq!(s.state["phase"], "final");
        assert_eq!(s.state["text"], "late words");
    }
    #[test]
    fn dictation_pressed_while_the_last_one_finishes_waits_instead_of_failing() {
        // The model in use, as downloaded (sizes and marker; no weights read).
        let root = tempfile::tempdir().unwrap();
        let m = zeron_voice::manifest();
        let dir = root.path().join(&m.id);
        std::fs::create_dir_all(&dir).unwrap();
        for f in &m.files {
            std::fs::File::create(dir.join(&f.name)).unwrap().set_len(f.size).unwrap();
        }
        std::fs::write(dir.join("verified"), m.marker()).unwrap();
        // The worker finishes a cancelled recording's load for three tries.
        let tries = Rc::new(Cell::new(0));
        let events = Rc::new(RefCell::new(VecDeque::new()));
        let mut s = Service::new(Some(root.path().to_owned()));
        let (counted, queued) = (tries.clone(), events.clone());
        s.open = Box::new(move |_, _| {
            counted.set(counted.get() + 1);
            if counted.get() <= 3 {
                return Err(None);
            }
            Ok(Box::new(Fake {
                events: queued.clone(),
                cancelled: Rc::default(),
                stopped: Rc::default(),
                level: Rc::default(),
            }) as Box<dyn Recording>)
        });
        // Not an error: starting, as long as it takes.
        assert_eq!(s.start(&json!({})).unwrap()["phase"], "starting");
        s.poll();
        s.poll();
        assert_eq!(s.state["phase"], "starting");
        assert!(s.session.is_none());
        s.poll();
        assert!(s.session.is_some());
        events.borrow_mut().push_back(Event::Listening);
        s.poll();
        assert_eq!(s.state["phase"], "listening");
        // Let go before the microphone opened: nothing recorded, no error.
        let mut s = Service::new(Some(root.path().to_owned()));
        s.open = Box::new(|_, _| Err(None));
        s.start(&json!({})).unwrap();
        s.stop();
        assert_eq!(s.state["phase"], "cancelled");
        assert!(!s.needs_poll());
    }
    #[test]
    fn dictation_without_a_data_folder_says_so_and_never_panics() {
        let mut s = Service::new(None);
        assert!(s.status().is_err());
        assert!(s.start(&json!({})).is_err());
        assert!(s.download(&json!({})).is_err());
        assert!(s.remove(&json!({})).is_err());
    }
    #[test]
    fn dictation_without_the_model_asks_for_it_before_touching_the_microphone() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Service::new(Some(dir.path().to_owned()));
        assert_eq!(s.status().unwrap()["models"][0]["installed"], false);
        assert_eq!(s.start(&json!({})).unwrap_err(), "no_model");
        assert!(s.session.is_none());
    }
    /// The composer of ui/dictation_fixture.luau, with the host's dictation
    /// played: the ops the scripts ask for, start answering as the host does.
    fn fixture() -> (zgx_script::testing::TestApp, Rc<RefCell<Vec<String>>>) {
        let ops = Rc::new(RefCell::new(Vec::<String>::new()));
        let asked = ops.clone();
        let editor = Rc::new(RefCell::new(None::<zgx::widgets::EditorHandle>));
        let leased = editor.clone();
        let mut app = zgx_script::testing::TestApp::new(move |r| {
            let host = zgx_script::ScriptHost::new(
                r,
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/dictation_fixture.luau"),
            );
            let shared = host.shared("dictation", json_value(&json!({"phase":"idle"})));
            host.async_action("dictation.call", move |args: Value| {
                let args = value_json(&args);
                let op = args["op"].as_str().unwrap_or("").to_owned();
                if op != "keep" {
                    asked.borrow_mut().push(op.clone());
                }
                let state = match op.as_str() {
                    "start" => json!({"phase":"listening", "serial":0, "hold":args["hold"], "target":"main"}),
                    "stop" => json!({"phase":"finalizing", "serial":0, "target":"main"}),
                    _ => json!({"phase":"cancelled", "serial":0}),
                };
                shared.set(json_value(&state));
                async move { Ok::<_, String>(json_value(&state)) }
            });
            host.action("dictation.guard", |_: Value| {});
            host.function("dictation.composing", move |_| composing(leased.borrow().as_ref()));
            host
        });
        app.ui.scene.borrow_mut().resize(960., 700.);
        app.frames(30);
        // (The composer's editor stands for the lease the guard takes.)
        app.click("role:textbox \"Message\"");
        *editor.borrow_mut() = app.ui.focused_editor();
        (app, ops)
    }
    #[test]
    fn dictation_shortcut_key_held_past_its_modifier_never_types_over_the_words() {
        use zgx::input::{InputEvent, Modifiers};
        let (mut app, ops) = fixture();
        app.click("role:textbox \"Message\"");
        let (d, chord) = zgx_script::driver::parse_keys("cmd+d").unwrap();
        // A key as the window delivers it: the shortcut runs its command;
        // else a press nothing prevents, with neither Command nor Control
        // held, also types its text.
        let press = |app: &mut zgx_script::testing::TestApp, modifiers: Modifiers, repeat: bool| {
            zgx::input::set_held_modifiers(&app.ui.runtime, modifiers);
            if modifiers == chord {
                assert!(app.host.run_command("shell.dictate"));
            } else {
                let result =
                    app.ui.dispatch(InputEvent::KeyDown { key: d.clone(), modifiers, repeat });
                if !result.default_prevented && !modifiers.control && !modifiers.meta {
                    app.ui.dispatch(InputEvent::Text("d".into()));
                }
            }
            app.settle();
            app.frames(2);
        };
        // ⌘ down, D down (and its repeats while ⌘ is held): dictating.
        press(&mut app, chord, false);
        press(&mut app, chord, true);
        // (Held past a tap.)
        std::thread::sleep(Duration::from_millis(350));
        // ⌘ up first: the hold ends, the words are on their way.
        zgx::input::set_held_modifiers(&app.ui.runtime, Modifiers::default());
        app.settle();
        app.frames(2);
        // D still down: bare repeats, then D up.
        for _ in 0..3 {
            press(&mut app, Modifiers::default(), true);
        }
        app.ui.dispatch(InputEvent::KeyUp { key: d.clone(), modifiers: Modifiers::default() });
        app.settle();
        app.host.run_command("final");
        app.settle();
        app.frames(5);
        assert_eq!(app.error(), None);
        assert_eq!(*ops.borrow(), ["start", "stop", "cancel"]);
        let editor = app.ui.focused_editor().unwrap();
        assert_eq!(editor.editor.borrow().text().as_ref(), "你好 🌎");
        // A fresh press of D after it came up types, as ever.
        press(&mut app, Modifiers::default(), false);
        assert_eq!(editor.editor.borrow().text().as_ref(), "你好 🌎d");
    }
    #[test]
    fn dictation_typing_on_puts_the_words_at_the_caret_and_another_chat_drops_them() {
        let (mut app, _) = fixture();
        app.type_text("role:textbox \"Message\"", "Hello ");
        let editor = app.ui.focused_editor().unwrap();
        let lease = EditorLease::new(editor.clone()).unwrap();
        let text = |e: &zgx::widgets::EditorHandle| e.editor.borrow().text().to_string();
        // Listening, then typed on, an IME's composition included.
        assert!(app.host.run_command("begin"));
        app.settle();
        app.ui.dispatch(zgx::input::InputEvent::Text("there".into()));
        editor.set_composition("正在", None, &[]);
        editor.set_composition("", None, &[]);
        app.settle();
        assert!(lease.valid());
        editor.set_selection(5, 5);
        app.host.run_command("final");
        app.settle();
        app.frames(3);
        assert_eq!(app.error(), None);
        assert_eq!(text(&editor), "Hello你好 🌎 there");
        // One undo takes the words back out, and only them.
        app.keys("cmd+z");
        assert_eq!(text(&editor), "Hello there");
        // Mid-composition when the words come: they wait for it to end,
        // and the composition is kept.
        editor.set_selection(11, 11);
        assert!(app.host.run_command("begin"));
        app.settle();
        editor.set_composition("正在", None, &[]);
        app.host.run_command("final");
        app.settle();
        app.frames(3);
        assert_eq!(editor.editor.borrow().preedit().map(|p| p.text.to_string()).as_deref(), Some("正在"));
        assert_eq!(text(&editor), "Hello there");
        app.ui.dispatch(zgx::input::InputEvent::ImeCommit("正在".into()));
        app.settle();
        std::thread::sleep(Duration::from_millis(150));
        app.frames(10);
        assert_eq!(text(&editor), "Hello there正在你好 🌎");
        app.keys("cmd+z");
        assert_eq!(text(&editor), "Hello there正在");
        // Another chat's draft: the words aren't for it.
        assert!(app.host.run_command("begin"));
        app.settle();
        app.host.run_command("move");
        app.settle();
        app.host.run_command("final");
        app.settle();
        app.frames(3);
        assert_eq!(text(&editor), "Hello there正在");
        // Nor mid-composition does one start.
        editor.set_composition("正在", None, &[]);
        assert!(EditorLease::new(editor.clone()).is_none());
    }
}

