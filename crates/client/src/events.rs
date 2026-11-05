//! Change notifications. The platform registers one [`ClientListener`]; events
//! are *coalesced* (at most one delivery burst per [`MIN_INTERVAL`]) and carry
//! only revisions — the platform pulls the latest snapshot after an event, so
//! a skipped intermediate revision costs nothing.
//!
//! Delivery happens on a client runtime thread. Implementations must return
//! quickly (hop to the UI thread and pull there).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::config::AuthTokens;
use crate::connectivity::Connectivity;
use crate::lock;

/// Minimum spacing between delivery bursts (one display frame).
pub const MIN_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    /// [`crate::WorkspaceSnapshot`] advanced to `revision`.
    WorkspaceChanged { revision: u64 },
    /// A session's transcript ([`crate::SessionSnapshot`]) advanced.
    SessionChanged { chat_id: String, revision: u64 },
    /// A session's [`crate::ComposerState`] advanced.
    ComposerChanged { chat_id: String, revision: u64 },
    /// Graced connectivity posture changed.
    ConnectivityChanged(Connectivity),
    /// The WorkOS pair rotated — persist it (Keychain) now; the old refresh
    /// token is already spent.
    AuthRefreshed(AuthTokens),
    /// The session cannot be refreshed any more (refresh token rejected) —
    /// the platform should sign out and show sign-in.
    AuthExpired { reason: String },
}

pub trait ClientListener: Send + Sync + 'static {
    fn on_event(&self, event: ClientEvent);
}

/// A listener that drops everything (tests, headless tools).
pub struct NullListener;

impl ClientListener for NullListener {
    fn on_event(&self, _event: ClientEvent) {}
}

#[derive(Default)]
struct Pending {
    ordered: Vec<ClientEvent>,
    workspace: Option<u64>,
    sessions: BTreeMap<String, u64>,
    composers: BTreeMap<String, u64>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.ordered.is_empty()
            && self.workspace.is_none()
            && self.sessions.is_empty()
            && self.composers.is_empty()
    }

    fn drain(&mut self) -> Vec<ClientEvent> {
        let mut out = std::mem::take(&mut self.ordered);
        if let Some(revision) = self.workspace.take() {
            out.push(ClientEvent::WorkspaceChanged { revision });
        }
        for (chat_id, revision) in std::mem::take(&mut self.sessions) {
            out.push(ClientEvent::SessionChanged { chat_id, revision });
        }
        for (chat_id, revision) in std::mem::take(&mut self.composers) {
            out.push(ClientEvent::ComposerChanged { chat_id, revision });
        }
        out
    }
}

/// Coalescing delivery queue in front of the platform listener.
pub(crate) struct EventPump {
    listener: Arc<dyn ClientListener>,
    pending: Mutex<Pending>,
    notify: Notify,
}

impl EventPump {
    pub(crate) fn new(listener: Arc<dyn ClientListener>) -> Arc<Self> {
        Arc::new(Self {
            listener,
            pending: Mutex::new(Pending::default()),
            notify: Notify::new(),
        })
    }

    pub(crate) fn start(self: &Arc<Self>, cancel: CancellationToken) {
        let pump = self.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = pump.notify.notified() => {}
                }
                let events = lock(&pump.pending).drain();
                for event in events {
                    pump.listener.on_event(event);
                }
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(MIN_INTERVAL) => {}
                }
                if !lock(&pump.pending).is_empty() {
                    pump.notify.notify_one();
                }
            }
        });
    }

    pub(crate) fn workspace(&self, revision: u64) {
        {
            let mut pending = lock(&self.pending);
            pending.workspace = Some(pending.workspace.map_or(revision, |r| r.max(revision)));
        }
        self.notify.notify_one();
    }

    pub(crate) fn session(&self, chat_id: &str, revision: u64) {
        {
            let mut pending = lock(&self.pending);
            let slot = pending.sessions.entry(chat_id.to_owned()).or_default();
            *slot = (*slot).max(revision);
        }
        self.notify.notify_one();
    }

    pub(crate) fn composer(&self, chat_id: &str, revision: u64) {
        {
            let mut pending = lock(&self.pending);
            let slot = pending.composers.entry(chat_id.to_owned()).or_default();
            *slot = (*slot).max(revision);
        }
        self.notify.notify_one();
    }

    pub(crate) fn ordered(&self, event: ClientEvent) {
        lock(&self.pending).ordered.push(event);
        self.notify.notify_one();
    }
}
