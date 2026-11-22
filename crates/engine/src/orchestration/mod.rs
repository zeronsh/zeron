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

pub(crate) mod ask_images;
pub(crate) mod brain;
pub(crate) mod crew_archive;
pub(crate) mod flow;
pub(crate) mod incoming;
pub mod comms;
pub(crate) mod loads;
pub(crate) mod ops;
pub(crate) mod one_shots;
pub(crate) mod queue;
pub(crate) mod progress;
pub mod report;
pub(crate) mod board;
pub(crate) mod board_cards;
pub(crate) mod board_checks;
pub(crate) mod derive;
pub(crate) mod placement;
pub(crate) mod scope;
pub(crate) mod limits;
pub(crate) mod scratchpad;
pub(crate) mod stale_dumps;
pub(crate) mod turn_asks;
pub(crate) mod watchers;
pub(crate) mod wake;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod flow_tests;

pub use comms::CommBus;

/// The most often [`Orchestration::subscribe`] publishes (4/s).
pub const PUBLISH_EVERY: Duration = Duration::from_millis(250);

/// How long a change waits for the ones right behind it before a watcher on
/// changes hears it: a burst (a turn ending writes its card, crew row and
/// report together) goes as one frame.
pub const COLLECT: Duration = Duration::from_millis(50);
/// The least time between two frames to one watcher on changes: a change
/// still arrives within about this, however busy it gets.
pub const FRAME_SPACING: Duration = Duration::from_millis(100);
/// How often a watcher on changes is sent the whole state again, whatever
/// changed (a reader that lost track is set right).
pub const SNAPSHOT_EVERY: Duration = Duration::from_secs(10 * 60);

/// `rows` as change frames (`first`, when given, the state the first one
/// shows): a snapshot first and every `snapshot_every`,
/// else a delta of what changed, each change waiting `collect` for those
/// right behind it and frames at least `spacing` apart.
pub(crate) fn frames_stream(
    rows: watch::Receiver<OrchestrationState>,
    first: Option<OrchestrationState>,
    collect: Duration,
    spacing: Duration,
    snapshot_every: Duration,
) -> futures::stream::BoxStream<'static, serde_json::Value> {
    use futures::StreamExt;
    use tokio::time::Instant;
    use zeron_proto::orchestration::frames::Framer;
    struct Watcher {
        rows: watch::Receiver<OrchestrationState>,
        first: Option<OrchestrationState>,
        framer: Framer,
        sent: Option<Instant>,
        snapshot_at: Instant,
    }
    let start = Watcher { rows, first, framer: Framer::new(), sent: None, snapshot_at: Instant::now() };
    futures::stream::unfold(start, move |mut w| async move {
        loop {
            let mut whole = w.sent.is_none();
            if !whole {
                tokio::select! {
                    changed = w.rows.changed() => changed.ok()?,
                    _ = tokio::time::sleep_until(w.snapshot_at + snapshot_every) => whole = true,
                }
                if !whole {
                    tokio::time::sleep(collect).await;
                    if let Some(sent) = w.sent {
                        tokio::time::sleep_until(sent + spacing).await;
                    }
                }
            }
            let frame = if let Some(first) = w.first.take() {
                Some(w.framer.snapshot(&first))
            } else {
                let state = w.rows.borrow_and_update();
                if whole { Some(w.framer.snapshot(&state)) } else { w.framer.frame(&state) }
            };
            let Some(frame) = frame else { continue };
            let now = Instant::now();
            w.sent = Some(now);
            if whole {
                w.snapshot_at = now;
            }
            let value = serde_json::to_value(&frame).ok()?;
            return Some((value, w));
        }
    })
    .boxed()
}

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
    /// `createOrchestrator`'s request ids and what each made: a request
    /// sent twice (a double submit, a retry) gets the first one's id back.
    created: Mutex<CreatedBy>,
    /// The cards to work out a moment after a change (board.rs).
    board: board::Board,
    /// This device's session rows as the engine found them on start (what
    /// was at work when it stopped), until the start-up pass takes them.
    before_stop: Mutex<Option<Vec<zeron_proto::Session>>>,
    /// Crew threads resumed after the restart, and when: their cards are
    /// left alone until their new turn has had time to start.
    resumed: Mutex<Vec<(String, chrono::DateTime<Utc>)>>,
}

/// The newest request ids, oldest dropped past [`CREATED_KEPT`].
#[derive(Default)]
pub(crate) struct CreatedBy {
    ids: std::collections::HashMap<String, String>,
    order: std::collections::VecDeque<String>,
}

const CREATED_KEPT: usize = 256;

impl CreatedBy {
    pub(crate) fn get(&self, request: &str) -> Option<String> {
        self.ids.get(request).cloned()
    }

    pub(crate) fn put(&mut self, request: String, id: String) {
        if self.ids.insert(request.clone(), id).is_none() {
            self.order.push_back(request);
            while self.order.len() > CREATED_KEPT {
                if let Some(old) = self.order.pop_front() {
                    self.ids.remove(&old);
                }
            }
        }
    }
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
                created: Mutex::new(CreatedBy::default()),
                board: board::Board::default(),
                before_stop: Mutex::new(None),
                resumed: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn workspace(&self) -> &WorkspaceHost {
        &self.inner.workspace
    }

    pub fn device_id(&self) -> &str {
        self.inner.workspace.device_id()
    }

    /// This device's session rows as found on start, before anything here
    /// rewrote them (`EngineCore` sets them once).
    pub(crate) fn set_before_stop(&self, rows: Vec<zeron_proto::Session>) {
        *lock(&self.inner.before_stop) = Some(rows);
    }

    /// [`Orchestration::set_before_stop`]'s rows, taken (once).
    pub(crate) fn take_before_stop(&self) -> Vec<zeron_proto::Session> {
        lock(&self.inner.before_stop).take().unwrap_or_default()
    }

    /// Note `chats` resumed after the restart at `at`.
    pub(crate) fn note_resumed(&self, chats: &[String], at: chrono::DateTime<Utc>) {
        lock(&self.inner.resumed).extend(chats.iter().map(|c| (c.clone(), at)));
    }

    /// The threads resumed after the restart less than `within` before `now`.
    pub(crate) fn resumed_since(&self, now: chrono::DateTime<Utc>, within: chrono::Duration) -> Vec<String> {
        let mut list = lock(&self.inner.resumed);
        list.retain(|(_, at)| now - *at < within);
        list.iter().map(|(c, _)| c.clone()).collect()
    }

    /// The board's touched cards and timers (board.rs).
    pub(crate) fn board(&self) -> &board::Board {
        &self.inner.board
    }

    /// Everything, read now.
    pub fn state(&self) -> OrchestrationState {
        self.inner.workspace.orchestration_state()
    }

    /// `f` of the rows without waiting on the registry lock or copying them
    /// ([`WorkspaceHost::scan_orchestration`]): for the timers' scans, whose
    /// writes read their rows again under the lock. `f` must be quick.
    pub(crate) fn scan<R>(&self, f: impl FnOnce(&OrchestrationState) -> R) -> R {
        self.inner.workspace.scan_orchestration(f)
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
                let perf_t = std::time::Instant::now();
                let value = serde_json::to_value(&*rx.borrow_and_update()).ok()?;
                tracing::debug!(target: "perf", what = "orch_push", bytes = value.to_string().len() as u64, us = perf_t.elapsed().as_micros() as u64);
                Some((value, rx))
            }))
            .boxed()
    }

    /// `WatchOrchestration {changes: true}`: the state as it is now, then
    /// only what changed, coalesced ([`COLLECT`], [`FRAME_SPACING`]), and
    /// the whole state again every [`SNAPSHOT_EVERY`]
    /// (`zeron_proto::orchestration::frames`).
    pub(crate) fn changes_stream(&self) -> futures::stream::BoxStream<'static, serde_json::Value> {
        // (The first frame is read now, so a write just made is in it.)
        let mut rows = self.inner.workspace.watch_orchestration_rows();
        rows.mark_unchanged();
        frames_stream(rows, Some(self.state()), COLLECT, FRAME_SPACING, SNAPSHOT_EVERY)
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

    /// What `createOrchestrator` made for each request id (held while one
    /// is created, so a second with the same id waits for the first's).
    pub(crate) fn created(&self) -> std::sync::MutexGuard<'_, CreatedBy> {
        lock(&self.inner.created)
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
            board::start(self.clone());
            limits::start(self.clone());
            start_stats(self.clone());
        }
    }

    pub(crate) fn rpc(&self) -> Option<Arc<crate::rpc::EngineRpc>> {
        lock(&self.inner.rpc).upgrade()
    }
}

/// Once a minute, when the `perf` target logs at debug: what the engine
/// holds (registry rows per kind, unsent ops, warm chat docs, tracked
/// progress, one-shot runs) and its RSS, to tell what grows.
fn start_stats(orch: Orchestration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if !tracing::enabled!(target: "perf", tracing::Level::DEBUG) {
                continue;
            }
            let Some(rpc) = orch.rpc() else { continue };
            let (rows, pending_ops) = orch.workspace().registry_stats();
            let (warm_docs, warm_bytes) = rpc.doc_host_ref().warm_stats();
            let rss_kb = std::fs::read_to_string("/proc/self/statm")
                .ok()
                .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
                .map_or(0, |pages| pages * 4);
            let rows: Vec<String> = rows.into_iter().map(|(kind, n)| format!("{kind}={n}")).collect();
            let shots = orch.progress().one_shot_stats();
            tracing::debug!(
                target: "perf",
                what = "engine_stats",
                rss_kb,
                rows = %rows.join(","),
                pending_ops,
                warm_docs,
                warm_bytes,
                progress_chats = orch.progress().tracked(),
                %shots,
            );
        }
    });
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
                approved_at: None,
                paused_at: None,
                check: None,
                threads_at: None,
                flow: None,
                depends_on: Vec::new(),
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
    // (Its column is worked out, never set: `patch.state` is read by the
    // op, as what the person did, and ignored from an agent. A new card
    // shows as planned to a reader from before places until it is.)
    if created {
        item.state = if patch.held == Some(true) { WorkState::Inbox } else { WorkState::Planned };
    }
    if let Some(priority) = patch.priority {
        item.priority = priority;
    }
    if let Some(progress) = patch.progress {
        item.progress = progress.map(|p| p.clamp(0.0, 1.0));
    }
    if let Some(threads) = patch.threads {
        let mut seen = std::collections::HashSet::new();
        let threads: Vec<String> = threads
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .collect();
        // A thread new to it: about to start (the board check's grace).
        if threads.iter().any(|t| !item.threads.contains(t)) {
            item.threads_at = Some(now);
        }
        item.threads = threads;
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
    if let Some(waiting) = patch.waiting {
        item.waiting = waiting.map(|w| excerpt(&w, 140)).filter(|w| !w.is_empty());
    }
    item.updated_at = now;
    let id = item.id.clone();
    doc.upsert_work_item(&item);
    // What the engine keeps of it: its finish line, held for later or not.
    // A new card's row is made with it; a card from before places gets its
    // row when it's first worked out.
    if created || patch.finish.is_some() || patch.held.is_some() {
        let mut row = match doc.board_card(&id) {
            Some(row) => Some(row),
            None if created => {
                let mut row = zeron_proto::orchestration::BoardCard::new(&id, now);
                row.reason = "Planned".into();
                Some(row)
            }
            None => None,
        };
        if let Some(row) = row.as_mut() {
            if let Some(finish) = patch.finish {
                row.finish = finish;
            }
            if let Some(held) = patch.held {
                row.held = held;
                row.status = if held {
                    zeron_proto::orchestration::CardStatus::Held
                } else {
                    zeron_proto::orchestration::CardStatus::Next
                };
            }
            row.updated_at = now;
            doc.upsert_board_card(row);
        }
    }
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

#[cfg(test)]
mod frames_stream_tests {
    use std::time::Duration;

    use futures::StreamExt;
    use tokio::sync::watch;
    use zeron_proto::orchestration::frames::{Followed, Follower, Heard};
    use zeron_proto::orchestration::{OrchestrationState, Orchestrator};

    use super::frames_stream;

    fn orchestrator(id: &str, name: &str) -> Orchestrator {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind": "standard", "name": name, "hue": 0,
            "createdAt": "2026-11-15T00:00:00Z", "updatedAt": "2026-11-15T00:00:00Z",
        }))
        .unwrap()
    }

    fn state(names: &[(&str, &str)]) -> OrchestrationState {
        OrchestrationState {
            orchestrators: names.iter().map(|(id, name)| orchestrator(id, name)).collect(),
            ..Default::default()
        }
    }

    const COLLECT: Duration = Duration::from_millis(50);
    const SPACING: Duration = Duration::from_millis(100);
    const WHOLE: Duration = Duration::from_secs(600);

    /// A burst of writes goes as one delta of what moved, soon after the
    /// first; a reader applying the frames has the state the engine has.
    #[tokio::test(start_paused = true)]
    async fn a_burst_is_one_delta_and_the_reader_keeps_up() {
        let (tx, rx) = watch::channel(state(&[("o1", "one"), ("o2", "two")]));
        let mut frames = frames_stream(rx, None, COLLECT, SPACING, WHOLE);
        let mut reader = Follower::default();
        let first = frames.next().await.unwrap();
        assert_eq!(first["frame"], "snapshot");
        assert_eq!(reader.hear(Heard::read(&first).unwrap()), Followed::Changed);

        let started = tokio::time::Instant::now();
        tx.send_replace(state(&[("o1", "one!"), ("o2", "two")]));
        tx.send_replace(state(&[("o1", "one!!"), ("o2", "two")]));
        tx.send_replace(state(&[("o1", "one!!"), ("o2", "two"), ("o3", "three")]));
        let delta = frames.next().await.unwrap();
        // Within the collect window plus the spacing, never sooner than it.
        let waited = started.elapsed();
        assert!(waited >= COLLECT && waited <= SPACING + COLLECT, "{waited:?}");
        assert_eq!(delta["frame"], "delta");
        assert_eq!(delta["version"], 2);
        let changed: Vec<&str> =
            delta["orchestrators"]["changed"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
        assert_eq!(changed, ["o1", "o3"]);
        assert!(delta.get("items").is_none() && delta.get("dumps").is_none());
        assert_eq!(reader.hear(Heard::read(&delta).unwrap()), Followed::Changed);
        assert_eq!(reader.state.as_ref(), Some(&*tx.borrow()));
    }

    /// A write that changes nothing sends nothing; the whole state is sent
    /// again on its period.
    #[tokio::test(start_paused = true)]
    async fn nothing_changed_sends_nothing_and_the_whole_state_comes_again() {
        let s = state(&[("o1", "one")]);
        let (tx, rx) = watch::channel(s.clone());
        let mut frames = frames_stream(rx, Some(s.clone()), COLLECT, SPACING, WHOLE);
        assert_eq!(frames.next().await.unwrap()["version"], 1);
        tx.send_replace(s.clone());
        let next = tokio::time::timeout(WHOLE - Duration::from_secs(1), frames.next()).await;
        assert!(next.is_err(), "nothing changed, nothing sent");
        let again = frames.next().await.unwrap();
        assert_eq!(again["frame"], "snapshot");
        assert_eq!(again["version"], 2);
    }
}
