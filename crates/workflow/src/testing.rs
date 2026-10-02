//! A scriptable [`Host`] for tests (this crate's and callers').
//!
//! Replies come from closures; every effect is recorded in order. Asks can be
//! delayed (to prove fan-out is concurrent) and are completed from a
//! short-lived thread, like a real host completing from its executor.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::host::*;
use crate::site::SiteKey;

/// One recorded effect.
#[derive(Debug, Clone, PartialEq)]
pub enum Recorded {
    Phase(String),
    Actor(SiteKey, ActorSpec),
    Ask(AskRequest),
    Run(RunRequest),
    Read(SiteKey, ReadOp),
    Log(String),
    Report(Value, Option<String>),
    Artifact(ArtifactRequest),
}

type AskFn = dyn Fn(&AskRequest) -> AskReply + Send + Sync;
type RunFn = dyn Fn(&RunRequest) -> RunReply + Send + Sync;
type ReadFn = dyn Fn(&ReadOp) -> Result<Value, String> + Send + Sync;

pub struct FakeHost {
    pub events: Mutex<Vec<Recorded>>,
    ask_fn: Mutex<Arc<AskFn>>,
    run_fn: Mutex<Arc<RunFn>>,
    read_fn: Mutex<Arc<ReadFn>>,
    ask_delay: Mutex<Duration>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: AtomicUsize,
}

impl FakeHost {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::default(),
            ask_fn: Mutex::new(Arc::new(|req: &AskRequest| {
                AskReply::ok(json!({"echo": req.instructions}))
            })),
            run_fn: Mutex::new(Arc::new(|_| RunReply {
                exit_code: Some(0),
                ..RunReply::default()
            })),
            read_fn: Mutex::new(Arc::new(|_| Ok(json!([])))),
            ask_delay: Mutex::new(Duration::ZERO),
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: AtomicUsize::new(0),
        })
    }

    pub fn on_ask(&self, f: impl Fn(&AskRequest) -> AskReply + Send + Sync + 'static) {
        *self.ask_fn.lock().unwrap() = Arc::new(f);
    }

    pub fn on_run(&self, f: impl Fn(&RunRequest) -> RunReply + Send + Sync + 'static) {
        *self.run_fn.lock().unwrap() = Arc::new(f);
    }

    pub fn on_read(&self, f: impl Fn(&ReadOp) -> Result<Value, String> + Send + Sync + 'static) {
        *self.read_fn.lock().unwrap() = Arc::new(f);
    }

    pub fn delay_asks(&self, d: Duration) {
        *self.ask_delay.lock().unwrap() = d;
    }

    /// The most asks ever in flight at once.
    pub fn max_in_flight(&self) -> usize {
        self.max_in_flight.load(Ordering::SeqCst)
    }

    pub fn recorded(&self) -> Vec<Recorded> {
        self.events.lock().unwrap().clone()
    }

    pub fn asks(&self) -> Vec<AskRequest> {
        self.recorded()
            .into_iter()
            .filter_map(|e| match e {
                Recorded::Ask(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    fn record(&self, e: Recorded) {
        self.events.lock().unwrap().push(e);
    }
}

impl Host for FakeHost {
    fn enter_phase(&self, name: &str) {
        self.record(Recorded::Phase(name.to_owned()));
    }

    fn create_actor(&self, key: &SiteKey, spec: &ActorSpec) {
        self.record(Recorded::Actor(key.clone(), spec.clone()));
    }

    fn ask(&self, req: AskRequest) -> Completion<AskReply> {
        self.record(Recorded::Ask(req.clone()));
        let reply_fn = self.ask_fn.lock().unwrap().clone();
        let delay = *self.ask_delay.lock().unwrap();
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        let (completer, completion) = completion();
        let in_flight = self.in_flight.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let reply = reply_fn(&req);
            in_flight.fetch_sub(1, Ordering::SeqCst);
            completer.complete(reply);
        });
        completion
    }

    fn run(&self, req: RunRequest) -> Completion<RunReply> {
        self.record(Recorded::Run(req.clone()));
        let f = self.run_fn.lock().unwrap().clone();
        completed(f(&req))
    }

    fn read(&self, key: &SiteKey, op: &ReadOp) -> Result<Value, String> {
        self.record(Recorded::Read(key.clone(), op.clone()));
        let f = self.read_fn.lock().unwrap().clone();
        f(op)
    }

    fn log(&self, line: &str) {
        self.record(Recorded::Log(line.to_owned()));
    }

    fn report(&self, item: Value, artifact_id: Option<String>) -> Result<(), String> {
        self.record(Recorded::Report(item, artifact_id));
        Ok(())
    }

    fn artifact(&self, req: ArtifactRequest) -> Result<(), String> {
        self.record(Recorded::Artifact(req));
        Ok(())
    }
}
