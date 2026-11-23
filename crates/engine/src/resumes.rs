//! The engine's own resumes of chats its stops cut off, whatever plugins are
//! on: a usage limit's wait and its resume at the reset
//! ([`crate::usage_limits`]), and the chats the engine's last stop left at
//! work, resumed on its next start ([`crate::restart_resume`]).
//!
//! [`Resumes`] is their shared handle: one per engine (`EngineCore` owns
//! it, every `EngineRpc` shares it), started once with the first RPC
//! service. What a chat is to orchestration is read only while
//! Orchestrators is on (`EngineRpc::active_orchestration`); off, every chat
//! is a plain one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use chrono::{DateTime, Utc};

/// Cheap to clone; one per engine.
#[derive(Clone, Default)]
pub struct Resumes {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// The newest RPC service: what the resumes are sent through.
    rpc: Mutex<Weak<crate::rpc::EngineRpc>>,
    started: AtomicBool,
    /// This device's session rows as the engine found them on start (what
    /// was at work when it stopped), until the start-up resume takes them.
    before_stop: Mutex<Option<Vec<zeron_proto::Session>>>,
    /// Chats resumed after the restart, and when: a usage limit's resume
    /// leaves them alone a moment, and so do their board cards.
    resumed: Mutex<Vec<(String, DateTime<Utc>)>>,
    /// The start-up resume has run (a board started later goes on at once).
    started_up: tokio::sync::watch::Sender<bool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Resumes {
    /// This device's session rows as found on start, before anything here
    /// rewrote them (`EngineCore` sets them once).
    pub(crate) fn set_before_stop(&self, rows: Vec<zeron_proto::Session>) {
        *lock(&self.inner.before_stop) = Some(rows);
    }

    /// [`Resumes::set_before_stop`]'s rows, taken (once).
    pub(crate) fn take_before_stop(&self) -> Vec<zeron_proto::Session> {
        lock(&self.inner.before_stop).take().unwrap_or_default()
    }

    /// Note `chats` resumed after the restart at `at`.
    pub(crate) fn note_resumed(&self, chats: &[String], at: DateTime<Utc>) {
        lock(&self.inner.resumed).extend(chats.iter().map(|c| (c.clone(), at)));
    }

    /// The chats resumed after the restart less than `within` before `now`.
    pub(crate) fn resumed_since(&self, now: DateTime<Utc>, within: chrono::Duration) -> Vec<String> {
        let mut list = lock(&self.inner.resumed);
        list.retain(|(_, at)| now - *at < within);
        list.iter().map(|(c, _)| c.clone()).collect()
    }

    /// Until the start-up resume has run.
    pub(crate) async fn started_up(&self) {
        let mut rx = self.inner.started_up.subscribe();
        let _ = rx.wait_for(|done| *done).await;
    }

    pub(crate) fn mark_started_up(&self) {
        self.inner.started_up.send_replace(true);
    }

    /// The newest RPC service (`EngineCore::rpc_service`); starts the usage
    /// limit task and the start-up resume the first time.
    pub(crate) fn attach_rpc(&self, rpc: &Arc<crate::rpc::EngineRpc>) {
        *lock(&self.inner.rpc) = Arc::downgrade(rpc);
        if !self.inner.started.swap(true, Ordering::AcqRel) {
            crate::usage_limits::start(self.clone());
            crate::restart_resume::start(self.clone());
        }
    }

    pub(crate) fn rpc(&self) -> Option<Arc<crate::rpc::EngineRpc>> {
        lock(&self.inner.rpc).upgrade()
    }
}
