//! Events → the chat doc's `meta.workflowRuns`.
//!
//! Every event is folded into an in-memory [`WorkflowRunsState`] per chat by
//! the pure reducer; the delta it returns is merged into a pending batch that
//! is written to the doc at most once per [`BATCH`] (a 500-node run produces
//! thousands of events; the doc, and every synced client, sees a few writes a
//! second of small per-entry values). Lifecycle events (created, launched,
//! settled, escalations) flush at once so a client never waits on a quarter
//! second to see a run appear or finish.
//!
//! The doc is the durable copy; the in-memory state is loaded from it the
//! first time a chat is touched, so a restarted host continues from what the
//! doc holds.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use zeron_proto::{
    WorkflowEvent, WorkflowEventKind, WorkflowRun, WorkflowRunsDelta, WorkflowRunsState,
};
use zeron_workflow::reduce;

use crate::DocHost;

/// Minimum time between doc writes for one chat (coalescing window).
pub const BATCH: Duration = Duration::from_millis(250);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct ChatProjection {
    state: WorkflowRunsState,
    pending: Option<WorkflowRunsDelta>,
    flush_armed: bool,
}

/// What the doc was asked to write: the sync-cost figures `docs/workflows.md`
/// quotes and a test asserts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectionStats {
    pub events: u64,
    pub doc_writes: u64,
    pub delta_bytes: u64,
}

#[derive(Clone)]
pub struct Projection {
    inner: Arc<Inner>,
}

struct Inner {
    doc_host: DocHost,
    chats: Mutex<HashMap<String, ChatProjection>>,
    events: AtomicU64,
    writes: AtomicU64,
    bytes: AtomicU64,
}

fn flush_at_once(kind: &WorkflowEventKind) -> bool {
    matches!(
        kind,
        WorkflowEventKind::RunCreated { .. }
            | WorkflowEventKind::RunLaunched { .. }
            | WorkflowEventKind::RunSettled { .. }
            | WorkflowEventKind::EscalationRaised { .. }
            | WorkflowEventKind::EscalationResolved { .. }
    )
}

impl Projection {
    pub fn new(doc_host: DocHost) -> Self {
        Self {
            inner: Arc::new(Inner {
                doc_host,
                chats: Mutex::default(),
                events: AtomicU64::new(0),
                writes: AtomicU64::new(0),
                bytes: AtomicU64::new(0),
            }),
        }
    }

    pub fn stats(&self) -> ProjectionStats {
        ProjectionStats {
            events: self.inner.events.load(Ordering::Relaxed),
            doc_writes: self.inner.writes.load(Ordering::Relaxed),
            delta_bytes: self.inner.bytes.load(Ordering::Relaxed),
        }
    }

    fn entry<'a>(
        &self,
        chats: &'a mut HashMap<String, ChatProjection>,
        chat_id: &str,
    ) -> &'a mut ChatProjection {
        chats
            .entry(chat_id.to_owned())
            .or_insert_with(|| ChatProjection {
                state: self
                    .inner
                    .doc_host
                    .open(chat_id)
                    .map(|h| h.doc().workflow_runs())
                    .unwrap_or_default(),
                ..ChatProjection::default()
            })
    }

    /// Fold one event; schedule (or perform) the doc write.
    pub fn apply(&self, chat_id: &str, event: &WorkflowEvent) {
        self.inner.events.fetch_add(1, Ordering::Relaxed);
        let (immediate, arm) = {
            let mut chats = lock(&self.inner.chats);
            let chat = self.entry(&mut chats, chat_id);
            if let Some(delta) = reduce(&mut chat.state, event) {
                match &mut chat.pending {
                    Some(pending) => pending.merge(delta),
                    None => chat.pending = Some(delta),
                }
            }
            let immediate = flush_at_once(&event.kind);
            let arm = !immediate && chat.pending.is_some() && !chat.flush_armed;
            if arm {
                chat.flush_armed = true;
            }
            (immediate && chat.pending.is_some(), arm)
        };
        if immediate {
            self.flush(chat_id);
        } else if arm {
            let this = self.clone();
            let chat_id = chat_id.to_owned();
            let spawn = tokio::runtime::Handle::try_current();
            match spawn {
                Ok(rt) => {
                    rt.spawn(async move {
                        tokio::time::sleep(BATCH).await;
                        this.flush(&chat_id);
                    });
                }
                Err(_) => self.flush(&chat_id),
            }
        }
    }

    /// Write whatever is pending for `chat_id` to its doc.
    pub fn flush(&self, chat_id: &str) {
        let delta = {
            let mut chats = lock(&self.inner.chats);
            let Some(chat) = chats.get_mut(chat_id) else {
                return;
            };
            chat.flush_armed = false;
            chat.pending.take()
        };
        let Some(delta) = delta else { return };
        if delta.is_empty() {
            return;
        }
        match self.inner.doc_host.open(chat_id) {
            Ok(handle) => {
                let bytes = serde_json::to_vec(&delta)
                    .map(|b| b.len() as u64)
                    .unwrap_or(0);
                if let Err(err) = handle.doc().apply_workflow_delta(&delta) {
                    tracing::warn!(chat = %chat_id, error = %err, "workflow state write failed");
                    return;
                }
                self.inner.writes.fetch_add(1, Ordering::Relaxed);
                self.inner.bytes.fetch_add(bytes, Ordering::Relaxed);
            }
            Err(err) => {
                tracing::warn!(chat = %chat_id, error = %err, "workflow state: chat unavailable")
            }
        }
    }

    /// The chat's current state (in memory; includes unflushed changes).
    pub fn state(&self, chat_id: &str) -> WorkflowRunsState {
        let mut chats = lock(&self.inner.chats);
        self.entry(&mut chats, chat_id).state.clone()
    }

    pub fn run(&self, chat_id: &str, run_id: &str) -> Option<WorkflowRun> {
        let mut chats = lock(&self.inner.chats);
        self.entry(&mut chats, chat_id).state.run(run_id).cloned()
    }

    /// Replace a run wholesale (restart reconciliation of a run the live
    /// events never described).
    pub fn replace_run(&self, chat_id: &str, run: &WorkflowRun) {
        {
            let mut chats = lock(&self.inner.chats);
            let chat = self.entry(&mut chats, chat_id);
            chat.state
                .runs
                .retain(|r| r.header.run_id != run.header.run_id);
            chat.state.runs.push(run.clone());
            chat.state.runs.sort_by_key(|r| r.header.created_at);
            chat.state.revision += 1;
            chat.pending = None;
        }
        if let Ok(handle) = self.inner.doc_host.open(chat_id) {
            let _ = handle.doc().replace_workflow_run(run);
        }
    }

    /// Flush every chat with pending changes (shutdown, tests).
    pub fn flush_all(&self) {
        let ids: Vec<String> = lock(&self.inner.chats).keys().cloned().collect();
        for id in ids {
            self.flush(&id);
        }
    }
}
