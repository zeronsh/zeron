//! Deterministic barriers exercise the production capture/loading coordinator.
use super::*;
use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    thread::JoinHandle,
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(5);

struct FakeCapture {
    closed: SyncSender<()>,
    ended: Arc<AtomicBool>,
}
impl Recording for FakeCapture {
    fn ended(&self) -> bool {
        self.ended.load(Ordering::Acquire)
    }
    fn finish(self) -> Result<(Vec<f32>, u32)> {
        Ok((vec![0.25; 3200], 16_000))
    }
}
impl Drop for FakeCapture {
    fn drop(&mut self) {
        self.closed.try_send(()).unwrap();
    }
}

struct Fixture {
    session: Session,
    load: SyncSender<Result<()>>,
    closed: Receiver<()>,
    ended: Arc<AtomicBool>,
    result: Receiver<Result<Option<String>>>,
    worker: JoinHandle<()>,
}
impl Fixture {
    fn new(open: Option<(Receiver<()>, SyncSender<()>)>) -> Self {
        Self::with_admission(open, None)
    }
    fn with_admission(
        open: Option<(Receiver<()>, SyncSender<()>)>,
        admission: Option<BusyGuard>,
    ) -> Self {
        let (events_tx, events) = mpsc::sync_channel(4);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let job = Job {
            dir: Default::default(),
            stop: stop.clone(),
            cancel: cancel.clone(),
            level: level.clone(),
            device: None,
            events: events_tx,
        };
        let (load, ready) = mpsc::sync_channel::<Result<()>>(1);
        let (loading, started) = mpsc::sync_channel(1);
        let (closed_tx, closed) = mpsc::sync_channel(1);
        let (result_tx, result) = mpsc::sync_channel(1);
        let ended = Arc::new(AtomicBool::new(false));
        let end_capture = ended.clone();
        let worker = std::thread::spawn(move || {
            let _admission = admission;
            let outcome = run_job(
                &job,
                || {
                    if let Some((open, opening)) = open {
                        opening.send(()).unwrap();
                        open.recv_timeout(WAIT).unwrap();
                    }
                    Ok(FakeCapture {
                        closed: closed_tx,
                        ended: end_capture,
                    })
                },
                || {
                    loading.send(()).unwrap();
                    ready.recv_timeout(WAIT).unwrap()?;
                    Ok(|samples, rate| {
                        // Stop during load must retain the exact recording.
                        assert_eq!(samples, vec![0.25; 3200]);
                        assert_eq!(rate, 16_000);
                        Ok("retained speech".into())
                    })
                },
            );
            result_tx.send(outcome).unwrap();
        });
        started.recv_timeout(WAIT).unwrap();
        Self {
            session: Session {
                stop,
                cancel,
                level,
                events,
            },
            load,
            closed,
            ended,
            result,
            worker,
        }
    }
    fn listening(&self) {
        assert!(matches!(
            self.session.events.recv_timeout(WAIT).unwrap(),
            Event::Listening
        ));
    }
    fn release(self, result: Result<()>) -> Result<Option<String>> {
        self.load.send(result).unwrap();
        let result = self.result.recv_timeout(WAIT).unwrap();
        self.worker.join().unwrap();
        result
    }
}

#[test]
fn listening_and_stop_do_not_wait_for_model_and_audio_is_retained() {
    let mut f = Fixture::new(None);
    f.listening();
    assert!(f.result.try_recv().is_err());
    f.session.finish();
    f.session.finish(); // Repeated Stop is idempotent.
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(matches!(
        f.session.events.recv_timeout(WAIT).unwrap(),
        Event::Finalizing
    ));
    assert!(f.result.try_recv().is_err());
    assert_eq!(
        f.release(Ok(())).unwrap().as_deref(),
        Some("retained speech")
    );
}

#[test]
fn cancel_during_load_closes_microphone_without_waiting_for_model() {
    let f = Fixture::new(None);
    f.listening();
    // Exercise the real Session destructor, including after Stop.
    let Fixture {
        session,
        load,
        closed,
        result,
        worker,
        ..
    } = f;
    drop(session);
    closed.recv_timeout(WAIT).unwrap();
    assert!(result.try_recv().is_err());
    load.send(Ok(())).unwrap();
    assert_eq!(result.recv_timeout(WAIT).unwrap().unwrap(), None);
    worker.join().unwrap();
}

#[test]
fn cancel_after_stop_during_load_discards_retained_audio() {
    let mut f = Fixture::new(None);
    f.listening();
    f.session.finish();
    f.closed.recv_timeout(WAIT).unwrap();
    f.session.cancel.store(true, Ordering::Release);
    assert_eq!(f.release(Ok(())).unwrap(), None);
}

#[test]
fn load_failure_closes_live_microphone_before_returning_error() {
    let f = Fixture::new(None);
    f.listening();
    f.load
        .send(Err(anyhow::anyhow!("model removed or corrupt")))
        .unwrap();
    assert!(f.result.recv_timeout(WAIT).unwrap().is_err());
    // Closure is guaranteed before the caller can report Failed/release BUSY.
    f.closed.try_recv().unwrap();
    f.worker.join().unwrap();
}

#[test]
fn listening_is_not_emitted_until_device_opens_and_stop_during_open_is_safe() {
    let (open, opened) = mpsc::sync_channel(1);
    let (opening, started) = mpsc::sync_channel(1);
    let mut f = Fixture::new(Some((opened, opening)));
    started.recv_timeout(WAIT).unwrap();
    assert!(f.session.poll().is_none());
    f.session.finish();
    open.send(()).unwrap();
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(matches!(
        f.session.events.recv_timeout(WAIT).unwrap(),
        Event::Finalizing
    ));
    assert!(f.session.poll().is_none());
    assert_eq!(
        f.release(Ok(())).unwrap().as_deref(),
        Some("retained speech")
    );
}

#[test]
fn recording_limit_closes_capture_while_model_is_still_loading() {
    let f = Fixture::new(None);
    f.listening();
    f.ended.store(true, Ordering::Release);
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(matches!(
        f.session.events.recv_timeout(WAIT).unwrap(),
        Event::Finalizing
    ));
    assert!(f.result.try_recv().is_err());
    assert_eq!(
        f.release(Ok(())).unwrap().as_deref(),
        Some("retained speech")
    );
}

#[test]
fn cancelled_load_keeps_start_and_model_removal_guarded_until_native_work_returns() {
    let f = Fixture::with_admission(None, Some(BusyGuard::acquire().unwrap()));
    f.listening();
    f.session.cancel.store(true, Ordering::Release);
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(busy(), "Settings must refuse model removal during loading");
    assert!(Session::start(Default::default(), None).is_err());
    assert_eq!(f.release(Ok(())).unwrap(), None);
    assert!(!busy());
    // Admission is reusable and releases even when a queued command is dropped.
    let (tx, rx) = mpsc::sync_channel(1);
    tx.send(BusyGuard::acquire().unwrap()).unwrap();
    drop(rx);
    assert!(!busy());
}

#[test]
fn stop_before_capture_start_never_opens_the_microphone() {
    let (events, _) = mpsc::sync_channel(4);
    let job = Job {
        dir: Default::default(),
        stop: Arc::new(AtomicBool::new(true)),
        cancel: Arc::new(AtomicBool::new(false)),
        level: Arc::new(AtomicU32::new(0)),
        device: None,
        events,
    };
    let result = run_job::<FakeCapture, _>(
        &job,
        || panic!("microphone must not open"),
        || Ok(|_, _| -> Result<String> { panic!("no audio to transcribe") }),
    )
    .unwrap();
    assert_eq!(result.as_deref(), Some(""));
}

#[test]
fn cancel_during_device_open_does_not_emit_listening_or_transcribe() {
    let (open, opened) = mpsc::sync_channel(1);
    let (opening, started) = mpsc::sync_channel(1);
    let mut f = Fixture::new(Some((opened, opening)));
    started.recv_timeout(WAIT).unwrap();
    f.session.cancel.store(true, Ordering::Release);
    open.send(()).unwrap();
    f.closed.recv_timeout(WAIT).unwrap();
    assert!(f.session.poll().is_none());
    assert_eq!(f.release(Ok(())).unwrap(), None);
}

#[test]
fn ready_model_waits_for_stop_and_transcribes_only_once() {
    let mut f = Fixture::new(None);
    f.listening();
    f.load.send(Ok(())).unwrap();
    assert!(f.result.try_recv().is_err());
    f.session.finish();
    f.session.finish();
    assert_eq!(
        f.result.recv_timeout(WAIT).unwrap().unwrap().as_deref(),
        Some("retained speech")
    );
    f.closed.try_recv().unwrap();
    f.worker.join().unwrap();
}

#[test]
fn audio_buffer_still_caps_recording_at_sixty_seconds() {
    let audio = Audio::with_capacity(0);
    let rate = 8_000;
    append(
        &vec![0.25_f32; rate as usize * MAX_SECONDS + 1],
        1,
        rate,
        &audio,
    );
    assert_eq!(audio.samples.lock().unwrap().len(), rate as usize * 60);
    assert!(audio.full.load(Ordering::Acquire));
}
