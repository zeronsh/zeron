//! Orchestrators: long-lived manager chats with crews of threads, the board
//! their work lives on, the asks that need the person, and the Brain dump
//! that routes what the person says (docs/design/orchestrators.md).
//!
//! The rows are synced registry kinds (`zeron_doc::registry::orchestration`);
//! [`Orchestration`] is this engine's handle on them: what each chat is
//! ([`ChatRole`]), the coalesced state watch, the comms feed, and the few
//! writes other parts of the engine make. The `Orchestrate` ops live in
//! [`ops`], the Brain dump's routing in [`brain`], scheduled wakes in
//! [`wake`], machine load in [`loads`], crew reports in [`report`], how far
//! each chat's work has got in [`progress`].

use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use chrono::Utc;
use tokio::sync::watch;
use zeron_proto::orchestration::{
    CrewMember, LinkKind, OrchestrationState, Orchestrator, OrchestratorKind, Priority, ReportHead,
    WorkItem, WorkItemPatch, WorkState,
};

use crate::workspace_host::WorkspaceHost;

pub(crate) mod brain;
pub(crate) mod crew_archive;
pub(crate) mod incoming;
pub mod comms;
pub(crate) mod loads;
pub(crate) mod ops;
pub(crate) mod queue;
pub(crate) mod progress;
pub mod report;
pub(crate) mod board_cards;
pub(crate) mod placement;
pub(crate) mod scope;
pub(crate) mod scratchpad;
pub(crate) mod stale_dumps;
pub(crate) mod turn_asks;
pub(crate) mod watchers;
pub(crate) mod wake;

#[cfg(test)]
mod tests;

pub use comms::CommBus;

/// The most often [`Orchestration::subscribe`] publishes (4/s).
pub const PUBLISH_EVERY: Duration = Duration::from_millis(250);

/// What a chat is to orchestration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRole {
    Plain,
    /// The chat is a standard orchestrator (same id).
    Orchestrator {
        id: String,
    },
    /// The chat is a Brain dump (same id).
    BrainDump {
        id: String,
    },
    /// A thread in an orchestrator's crew (not released).
    Crew {
        orchestrator_id: String,
        item_id: Option<String>,
    },
}

impl ChatRole {
    /// The orchestrator this chat belongs to or is.
    pub fn orchestrator_id(&self) -> Option<&str> {
        match self {
            ChatRole::Plain => None,
            ChatRole::Orchestrator { id } | ChatRole::BrainDump { id } => Some(id),
            ChatRole::Crew {
                orchestrator_id, ..
            } => Some(orchestrator_id),
        }
    }
}

/// This engine's orchestration: cheap to clone, one per engine (the
/// runtime's `EngineCore` owns it; every `EngineRpc` shares it).
#[derive(Clone)]
pub struct Orchestration {
    inner: Arc<Inner>,
}

struct Inner {
    workspace: WorkspaceHost,
    comms: CommBus,
    /// The coalesced state (≤ 4/s).
    state_rx: watch::Receiver<OrchestrationState>,
    /// The newest RPC service: what scheduled wakes send through.
    rpc: Mutex<Weak<crate::rpc::EngineRpc>>,
    wake_started: std::sync::atomic::AtomicBool,
    /// Progress of the chats this engine hosts.
    progress: progress::ProgressBook,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Orchestration {
    /// Needs a Tokio runtime (it spawns the coalescing task).
    pub fn new(workspace: WorkspaceHost) -> Self {
        let mut rows = workspace.watch_orchestration_rows();
        let (tx, state_rx) = watch::channel(rows.borrow_and_update().clone());
        tokio::spawn(async move {
            loop {
                if rows.changed().await.is_err() {
                    break;
                }
                let next = rows.borrow_and_update().clone();
                tx.send_if_modified(|current| {
                    if *current == next {
                        false
                    } else {
                        *current = next;
                        true
                    }
                });
                if tx.receiver_count() == 0 {
                    break; // the handle (which holds a receiver) is gone
                }
                tokio::time::sleep(PUBLISH_EVERY).await;
            }
        });
        Self {
            inner: Arc::new(Inner {
                workspace,
                comms: CommBus::new(),
                state_rx,
                rpc: Mutex::new(Weak::new()),
                wake_started: std::sync::atomic::AtomicBool::new(false),
                progress: progress::ProgressBook::default(),
            }),
        }
    }

    pub fn workspace(&self) -> &WorkspaceHost {
        &self.inner.workspace
    }

    pub fn device_id(&self) -> &str {
        self.inner.workspace.device_id()
    }

    /// Everything, read now.
    pub fn state(&self) -> OrchestrationState {
        self.inner.workspace.orchestration_state()
    }

    /// Everything, now and on every change, at most [`PUBLISH_EVERY`].
    pub fn subscribe(&self) -> watch::Receiver<OrchestrationState> {
        self.inner.state_rx.clone()
    }

    /// `WatchOrchestration`: the state as it is now (a write just made is
    /// in it), then every change, at most [`PUBLISH_EVERY`].
    pub(crate) fn stream(&self) -> futures::stream::BoxStream<'static, serde_json::Value> {
        use futures::StreamExt;
        let mut rx = self.subscribe();
        rx.mark_unchanged();
        let first = serde_json::to_value(self.state()).ok();
        futures::stream::iter(first)
            .chain(futures::stream::unfold(rx, |mut rx| async move {
                rx.changed().await.ok()?;
                let value = serde_json::to_value(&*rx.borrow_and_update()).ok()?;
                Some((value, rx))
            }))
            .boxed()
    }

    /// How far the chats hosted here have got.
    pub(crate) fn progress(&self) -> &progress::ProgressBook {
        &self.inner.progress
    }

    /// Turn the automatic progress notes on or off (on unless
    /// `ZERON_PROGRESS_NOTES=0`). On-demand notes and the deterministic
    /// fields stay on.
    pub fn set_auto_progress_notes(&self, on: bool) {
        self.inner.progress.set_auto_notes(on);
    }

    pub fn comms(&self) -> &CommBus {
        &self.inner.comms
    }

    /// What `chat_id` is: an orchestrator or Brain dump (its row has the
    /// chat's id), a thread in a crew (not released), or plain. From the
    /// rows alone, never from an id's shape: a role is a privilege.
    pub fn role_of(&self, chat_id: &str) -> ChatRole {
        self.inner.workspace.read_orchestration_with(|doc| {
            if let Some(o) = doc.orchestrator(chat_id) {
                return match o.kind {
                    OrchestratorKind::BrainDump => ChatRole::BrainDump { id: o.id },
                    _ => ChatRole::Orchestrator { id: o.id },
                };
            }
            match doc.crew_member(chat_id) {
                Some(member) if !member.released => ChatRole::Crew {
                    orchestrator_id: member.orchestrator_id,
                    item_id: member.item_id,
                },
                _ => ChatRole::Plain,
            }
        })
    }

    /// A thread's crew row, unless it was released.
    pub fn crew_of(&self, chat_id: &str) -> Option<CrewMember> {
        self.inner
            .workspace
            .read_orchestration_with(|doc| doc.crew_member(chat_id))
            .filter(|m| !m.released)
    }

    pub fn orchestrator(&self, id: &str) -> Option<Orchestrator> {
        self.inner
            .workspace
            .read_orchestration_with(|doc| doc.orchestrator(id))
    }

    /// A card on the board (`None` once it was taken off).
    pub fn item(&self, id: &str) -> Option<WorkItem> {
        self.inner
            .workspace
            .read_orchestration_with(|doc| doc.work_item(id))
            .filter(|i| !i.removed)
    }

    /// Record a crew thread's newest report on its crew row. `false` when
    /// the chat isn't in a crew.
    pub fn set_last_report(&self, chat_id: &str, head: ReportHead) -> bool {
        self.inner.workspace.write_orchestration(|doc| {
            let Some(mut member) = doc.crew_member(chat_id) else {
                return false;
            };
            member.last_report = Some(head);
            doc.upsert_crew_member(&member);
            true
        })
    }

    /// Create (`id: None`) or patch a work item. The item's id, or why not.
    pub fn patch_item(&self, patch: WorkItemPatch) -> Result<String, String> {
        self.inner
            .workspace
            .write_orchestration(|doc| apply_item_patch(doc, patch))
    }

    /// The newest RPC service: what scheduled wakes and other engine-side
    /// sends go through. Called by `EngineCore::rpc_service`; starts the wake
    /// timer the first time.
    pub(crate) fn attach_rpc(&self, rpc: &Arc<crate::rpc::EngineRpc>) {
        *lock(&self.inner.rpc) = Arc::downgrade(rpc);
        if !self
            .inner
            .wake_started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            wake::start(self.clone());
            watchers::start(self.clone());
            progress::start(self.clone());
            queue::start(self.clone());
        }
    }

    pub(crate) fn rpc(&self) -> Option<Arc<crate::rpc::EngineRpc>> {
        lock(&self.inner.rpc).upgrade()
    }
}

/// `patch` applied to the board (under the registry lock).
pub(crate) fn apply_item_patch(
    doc: &mut zeron_doc::RegistryDoc,
    patch: WorkItemPatch,
) -> Result<String, String> {
    let now = Utc::now();
    let existing = match patch.id.as_deref().filter(|id| !id.trim().is_empty()) {
        Some(id) => Some(
            doc.work_item(id)
                .filter(|i| !i.removed)
                .ok_or_else(|| format!("No board item {id}"))?,
        ),
        None => None,
    };
    let created = existing.is_none();
    let mut item = match existing {
        Some(item) => item,
        None => {
            let title = patch
                .title
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or("A new board item needs a title")?;
            let owner = patch.orchestrator_id.clone().flatten();
            // New items go to the end of their board.
            let order = doc
                .read_work_items()
                .iter()
                .filter(|i| i.orchestrator_id == owner)
                .map(|i| i.order)
                .fold(0.0f64, f64::max)
                + 1.0;
            WorkItem {
                id: uuid::Uuid::new_v4().simple().to_string(),
                orchestrator_id: owner,
                title: title.to_owned(),
                detail: String::new(),
                state: WorkState::Inbox,
                priority: Priority::Normal,
                progress: None,
                threads: Vec::new(),
                links: Vec::new(),
                outcome: None,
                source: None,
                order,
                created_at: now,
                updated_at: now,
                done_at: None,
                removed: Default::default(),
                removed_at: Default::default(),
                space_id: None,
                device_id: None,
                waiting: None,
                moved_at: None,
                moved_route_state: None,
            }
        }
    };
    if let Some(owner) = patch.orchestrator_id {
        if let Some(id) = owner.as_deref()
            && doc.orchestrator(id).is_none()
        {
            return Err(format!("No orchestrator {id}"));
        }
        item.orchestrator_id = owner;
    }
    if let Some(title) = patch
        .title
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
    {
        item.title = title;
    }
    if let Some(detail) = patch.detail {
        item.detail = detail;
    }
    if let Some(state) = patch.state {
        if state != item.state || created {
            item.done_at = (state == WorkState::Done).then_some(now);
        }
        item.state = state;
    }
    if let Some(priority) = patch.priority {
        item.priority = priority;
    }
    if let Some(progress) = patch.progress {
        item.progress = progress.map(|p| p.clamp(0.0, 1.0));
    }
    if let Some(threads) = patch.threads {
        let mut seen = std::collections::HashSet::new();
        item.threads = threads
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .collect();
    }
    if let Some(links) = patch.links {
        item.links = links;
    }
    if let Some(outcome) = patch.outcome {
        item.outcome = outcome;
    }
    if let Some(source) = patch.source {
        item.source = Some(source);
    }
    if let Some(order) = patch.order.filter(|o| o.is_finite()) {
        item.order = order;
    }
    if let Some(space) = patch.space_id {
        item.space_id = space.filter(|s| !s.trim().is_empty());
    }
    if let Some(device) = patch.device_id {
        item.device_id = device.filter(|d| !d.trim().is_empty());
    }
    // A card that leaves Next waits on nothing.
    match patch.waiting {
        Some(waiting) => item.waiting = waiting.map(|w| excerpt(&w, 140)).filter(|w| !w.is_empty()),
        None if item.state != WorkState::Planned => item.waiting = None,
        None => {}
    }
    item.updated_at = now;
    let id = item.id.clone();
    doc.upsert_work_item(&item);
    Ok(id)
}

/// `links` merged into `into`: a link to a target already there replaces it.
pub(crate) fn merge_links(
    into: &mut Vec<zeron_proto::orchestration::WorkLink>,
    links: Vec<zeron_proto::orchestration::WorkLink>,
) {
    for link in links {
        if link.kind == LinkKind::Other && link.target.is_empty() {
            continue;
        }
        match into.iter_mut().find(|l| l.target == link.target) {
            Some(existing) => *existing = link,
            None => into.push(link),
        }
    }
}

/// The first line of `text`, at most `max` characters (an ellipsis when cut).
pub(crate) fn excerpt(text: &str, max: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}
