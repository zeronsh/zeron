//! What a script can ask of the world, and the seam a host implements.
//!
//! The interpreter is pure: it never touches a file, a process or a chat. Every
//! effect is a call on [`Host`], keyed by a [`SiteKey`] so a host can journal
//! it and answer a re-run from the journal. The engine implements `Host` over
//! its scheduler; tests implement it with scripted fakes.
//!
//! Asks and commands are *dispatched* (`Host::ask` returns at once with a
//! [`Completion`]) and *joined* by the script (`.result()`), which is what lets
//! a script fan out: every ask of a list comprehension is in flight before the
//! first join. No async runtime is involved here — the host completes the
//! [`Completer`] from wherever it runs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::site::SiteKey;

// ── completion ────────────────────────────────────────────────────────────

struct Slot<T> {
    value: Mutex<SlotState<T>>,
    ready: Condvar,
}

enum SlotState<T> {
    Pending,
    Done(T),
    /// The completer was dropped without completing.
    Abandoned,
}

/// The host's half: complete exactly once. Dropping it unfulfilled wakes the
/// waiter with [`Wait::Abandoned`] (a cancelled run drops its pending asks).
pub struct Completer<T> {
    slot: Arc<Slot<T>>,
    done: bool,
}

/// The script's half; clonable, readable any number of times.
pub struct Completion<T> {
    slot: Arc<Slot<T>>,
}

impl<T> Clone for Completion<T> {
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
        }
    }
}

pub fn completion<T>() -> (Completer<T>, Completion<T>) {
    let slot = Arc::new(Slot {
        value: Mutex::new(SlotState::Pending),
        ready: Condvar::new(),
    });
    (
        Completer {
            slot: slot.clone(),
            done: false,
        },
        Completion { slot },
    )
}

/// An already-completed [`Completion`] (a journal replay, a validation error).
pub fn completed<T>(value: T) -> Completion<T> {
    let (completer, completion) = completion();
    completer.complete(value);
    completion
}

impl<T> Completer<T> {
    pub fn complete(mut self, value: T) {
        self.done = true;
        *self.slot.value.lock().unwrap_or_else(|e| e.into_inner()) = SlotState::Done(value);
        self.slot.ready.notify_all();
    }
}

impl<T> Drop for Completer<T> {
    fn drop(&mut self) {
        if !self.done {
            let mut state = self.slot.value.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*state, SlotState::Pending) {
                *state = SlotState::Abandoned;
            }
            drop(state);
            self.slot.ready.notify_all();
        }
    }
}

/// How a wait ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wait<T> {
    Ready(T),
    /// The run was cancelled while waiting.
    Cancelled,
    /// The host dropped the completer.
    Abandoned,
}

impl<T: Clone> Completion<T> {
    /// Block until completed, polling `cancel` every 25 ms. `blocked` receives
    /// the time spent waiting (the compute budget excludes it).
    pub fn wait(&self, cancel: &AtomicBool, blocked: &mut Duration) -> Wait<T> {
        let started = Instant::now();
        let mut state = self.slot.value.lock().unwrap_or_else(|e| e.into_inner());
        let out = loop {
            match &*state {
                SlotState::Done(v) => break Wait::Ready(v.clone()),
                SlotState::Abandoned => break Wait::Abandoned,
                SlotState::Pending => {}
            }
            if cancel.load(Ordering::Relaxed) {
                break Wait::Cancelled;
            }
            let (next, _) = self
                .slot
                .ready
                .wait_timeout(state, Duration::from_millis(25))
                .unwrap_or_else(|e| e.into_inner());
            state = next;
        };
        *blocked += started.elapsed();
        out
    }

    /// Non-blocking peek.
    pub fn try_get(&self) -> Option<T> {
        match &*self.slot.value.lock().unwrap_or_else(|e| e.into_inner()) {
            SlotState::Done(v) => Some(v.clone()),
            _ => None,
        }
    }
}

// ── requests and replies ──────────────────────────────────────────────────

/// `agent(...)`'s arguments.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ActorSpec {
    pub name: String,
    pub persona: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AskRequest {
    pub key: SiteKey,
    /// The actor's key (its `agent()` call).
    pub actor: SiteKey,
    pub instructions: String,
    /// JSON Schema for a typed result; text otherwise.
    pub schema: Option<Value>,
    pub read_only: bool,
    pub timeout_s: Option<u64>,
}

/// The outcome of an ask, as the script sees it: failures are values.
#[derive(Debug, Clone, PartialEq)]
pub struct AskReply {
    pub ok: bool,
    /// The typed value, or the text answer; `Null` on failure.
    pub value: Value,
    pub error: Option<String>,
    /// Replayed from the journal rather than run.
    pub cached: bool,
    pub tokens: u64,
}

impl AskReply {
    pub fn ok(value: Value) -> Self {
        Self {
            ok: true,
            value,
            error: None,
            cached: false,
            tokens: 0,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            value: Value::Null,
            error: Some(error.into()),
            cached: false,
            tokens: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRequest {
    pub key: SiteKey,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub timeout_s: u64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunReply {
    /// `None` when the process was killed (timeout) or never started.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    /// Output beyond the per-stream cap was dropped.
    pub truncated: bool,
    /// The command could not be started (not found, cwd outside the project).
    pub start_error: Option<String>,
}

/// A read-only look at the project. Hosts reject over-cap results with an
/// error rather than truncating.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadOp {
    Glob {
        pattern: String,
    },
    Read {
        path: String,
    },
    Grep {
        pattern: String,
        glob: Option<String>,
    },
    GitChangedFiles {
        base: Option<String>,
    },
    GitDiff {
        base: Option<String>,
        path: Option<String>,
    },
    GitStatus,
    GitLog {
        limit: u32,
        path: Option<String>,
    },
}

impl ReadOp {
    pub fn name(&self) -> &'static str {
        match self {
            ReadOp::Glob { .. } => "files.glob",
            ReadOp::Read { .. } => "files.read",
            ReadOp::Grep { .. } => "files.grep",
            ReadOp::GitChangedFiles { .. } => "git.changed_files",
            ReadOp::GitDiff { .. } => "git.diff",
            ReadOp::GitStatus => "git.status",
            ReadOp::GitLog { .. } => "git.log",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub label: String,
    pub value: Value,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArtifactContent {
    Markdown(String),
    Table {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    },
    Metrics(Vec<Metric>),
    /// A project-relative path; the host copies the bytes into the run.
    File(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactRequest {
    pub key: SiteKey,
    pub id: String,
    pub title: String,
    pub content: ArtifactContent,
    /// Version of this id the call publishes (1-based).
    pub version: u32,
}

/// The seam. All methods may be called from several threads (`pmap`).
pub trait Host: Send + Sync + 'static {
    /// The script entered `phase(name)`.
    fn enter_phase(&self, name: &str);
    /// `agent(...)` was called; the child chat is created at the first ask.
    fn create_actor(&self, key: &SiteKey, spec: &ActorSpec);
    /// Dispatch an ask; must not block.
    fn ask(&self, req: AskRequest) -> Completion<AskReply>;
    /// Dispatch a shell gate; the script blocks on the completion.
    fn run(&self, req: RunRequest) -> Completion<RunReply>;
    fn read(&self, key: &SiteKey, op: &ReadOp) -> Result<Value, String>;
    fn log(&self, line: &str);
    fn report(&self, item: Value, artifact_id: Option<String>) -> Result<(), String>;
    fn artifact(&self, req: ArtifactRequest) -> Result<(), String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completion_delivers_to_every_clone() {
        let (tx, rx) = completion::<u32>();
        let rx2 = rx.clone();
        let cancel = AtomicBool::new(false);
        let waiter = std::thread::spawn(move || {
            let mut blocked = Duration::ZERO;
            rx2.wait(&AtomicBool::new(false), &mut blocked)
        });
        std::thread::sleep(Duration::from_millis(30));
        tx.complete(7);
        assert_eq!(waiter.join().unwrap(), Wait::Ready(7));
        let mut blocked = Duration::ZERO;
        assert_eq!(rx.wait(&cancel, &mut blocked), Wait::Ready(7));
        assert_eq!(rx.try_get(), Some(7));
    }

    #[test]
    fn dropping_the_completer_abandons_and_cancel_interrupts() {
        let (tx, rx) = completion::<u32>();
        drop(tx);
        let mut blocked = Duration::ZERO;
        assert_eq!(
            rx.wait(&AtomicBool::new(false), &mut blocked),
            Wait::Abandoned
        );
        let (_tx, rx) = completion::<u32>();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            c.store(true, Ordering::SeqCst);
        });
        assert_eq!(rx.wait(&cancel, &mut blocked), Wait::Cancelled);
        assert!(blocked >= Duration::from_millis(30));
    }
}
