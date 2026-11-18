//! Inbox and Next, and starting a card.
//!
//! - **Inbox** holds what was captured and isn't started; **Next** what is
//!   queued to start after something else. Neither has a thread.
//! - **Starting** a card (`startItem`, or the person moving it to Now) gives
//!   it a thread: in its project (else its orchestrator's crew policy, else
//!   the best project), its title and detail as the first prompt. The card
//!   goes to Now with the thread on it, and the thread records the card as
//!   what it was started for.
//! - An orchestrator whose crew is full (`maxRunning`) keeps its new work in
//!   Next, saying why; when a slot frees, the engine that hosts the
//!   orchestrator starts its oldest most urgent Next card.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use zeron_proto::orchestration::{CommKind, Orchestrator, OrchestratorKind, WorkItem, WorkItemPatch, WorkState};
use zeron_proto::{ChildKind, SessionStatus};
use zeron_rpc::RpcError;

use super::Orchestration;
use super::ops::{When, deliver};
use crate::rpc::EngineRpc;

/// A thread that joined a crew this recently and hasn't run yet counts as
/// working: its first turn is on its way.
const STARTING_FOR: chrono::Duration = chrono::Duration::seconds(90);
/// How long the auto-start waits for changes to settle.
const SETTLE: Duration = Duration::from_millis(500);

/// Cards being started in this process (one thread each, however many ask).
static STARTING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

struct Starting(String);

impl Starting {
    fn claim(id: &str) -> Option<Self> {
        let mut set = STARTING.lock().unwrap_or_else(PoisonError::into_inner);
        set.insert(id.to_owned()).then(|| Starting(id.to_owned()))
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        STARTING.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.0);
    }
}

fn failed(message: impl Into<String>) -> RpcError {
    RpcError::Failed(message.into())
}

/// The first prompt of a card's thread: its title, then its detail (the
/// detail alone when it opens with the title).
pub(crate) fn first_prompt(card: &WorkItem) -> String {
    let (title, detail) = (card.title.trim(), card.detail.trim());
    if detail.is_empty() {
        title.to_owned()
    } else if title.is_empty() || detail.starts_with(title.trim_end_matches('…')) {
        detail.to_owned()
    } else {
        format!("{title}\n\n{detail}")
    }
}

/// Next cards waiting for a crew slot (they say why they wait), in the
/// order they start: most urgent, then oldest. A card put in Next by hand
/// waits for the person.
pub(crate) fn next_up<'a>(items: &'a [WorkItem], orchestrator: &str) -> Vec<&'a WorkItem> {
    let mut queued: Vec<&WorkItem> = items
        .iter()
        .filter(|i| {
            i.orchestrator_id.as_deref() == Some(orchestrator)
                && i.state == WorkState::Planned
                && i.waiting.is_some()
                && !i.removed
                && i.threads.is_empty()
        })
        .collect();
    queued.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then(a.created_at.cmp(&b.created_at))
            .then(a.id.cmp(&b.id))
    });
    queued
}

impl EngineRpc {
    /// `chat`'s status as its session row says, `None` before it ran.
    fn session_status(&self, chat: &str) -> Option<SessionStatus> {
        if let Some(s) = self.sessions_ref().watch_sessions().borrow().iter().find(|s| s.chat_id == chat) {
            return Some(s.status);
        }
        self.workspace_host()
            .read_sessions()
            .unwrap_or_default()
            .into_iter()
            .find(|s| s.chat_id == chat)
            .map(|s| s.status)
    }

    /// Crew threads of `orchestrator` working, waiting on the person, or
    /// just started.
    pub(crate) fn crew_running(&self, orchestrator: &str) -> usize {
        let now = Utc::now();
        self.orchestration()
            .state()
            .crew
            .iter()
            .filter(|m| m.orchestrator_id == orchestrator && !m.released)
            .filter(|m| match self.session_status(&m.chat_id) {
                Some(status) => matches!(status, SessionStatus::Working | SessionStatus::AwaitingInput),
                None => now - m.joined_at < STARTING_FOR,
            })
            .count()
    }

    /// Why `o` can't start crew work now: paused, or its crew is full.
    pub(crate) fn crew_full(&self, o: &Orchestrator) -> Option<String> {
        if o.paused {
            return Some(format!("{} is paused", o.name));
        }
        let max = o.crew.max_running?;
        let running = self.crew_running(&o.id);
        (running >= max as usize).then(|| format!("{}'s crew is full: {running} of {max} working", o.name))
    }

    /// Start card `id` (see the module doc). `force`: the person's start,
    /// which a full crew doesn't hold back. Replies `{id, threadId}`, or
    /// `{id, queued: true, waiting}` when it stays in Next.
    pub(crate) fn start_item(&self, id: &str, force: bool) -> Result<Value, RpcError> {
        self.start_item_as(id, force, None)
    }

    /// [`Self::start_item`], its thread on `agent` (harness, model) rather
    /// than the crew's (a flow step's own).
    pub(crate) fn start_item_as(
        &self,
        id: &str,
        force: bool,
        agent: Option<(Option<String>, Option<String>)>,
    ) -> Result<Value, RpcError> {
        let orch = self.orchestration().clone();
        let Some(_starting) = Starting::claim(id) else {
            return Err(failed("That card is starting already"));
        };
        let card = orch
            .item(id)
            .filter(|i| !i.removed)
            .ok_or_else(|| failed(format!("No board item {id}")))?;
        if matches!(card.state, WorkState::Done | WorkState::Dropped) {
            return Err(failed("That card is done"));
        }
        // Started already: the thread it has.
        if let Some(thread) = card.threads.first().cloned() {
            if matches!(card.state, WorkState::Inbox | WorkState::Planned) {
                orch.patch_item(WorkItemPatch {
                    id: Some(card.id.clone()),
                    state: Some(WorkState::Active),
                    waiting: Some(None),
                    ..Default::default()
                })
                .map_err(RpcError::Failed)?;
            }
            return Ok(json!({ "id": card.id, "threadId": thread }));
        }
        let owner = card
            .orchestrator_id
            .as_deref()
            .and_then(|o| orch.orchestrator(o))
            .filter(|o| o.kind == OrchestratorKind::Standard && !o.archived);
        if let Some(o) = &owner
            && !force
            && let Some(waiting) = self.crew_full(o)
        {
            orch.patch_item(WorkItemPatch {
                id: Some(card.id.clone()),
                state: Some(WorkState::Planned),
                waiting: Some(Some(waiting.clone())),
                ..Default::default()
            })
            .map_err(RpcError::Failed)?;
            return Ok(json!({ "id": card.id, "queued": true, "waiting": waiting }));
        }
        let prompt = first_prompt(&card);
        let (space, device) = self.card_place(&card, owner.as_ref(), &prompt);
        let chat = uuid::Uuid::new_v4().to_string();
        let (harness, model) = agent.unwrap_or_else(|| {
            owner
                .as_ref()
                .map(|o| (o.crew.harness.clone(), o.crew.model.clone()))
                .unwrap_or_default()
        });
        self.create_chat_for(
            &chat,
            space.as_deref(),
            device.as_deref(),
            harness,
            model,
            owner.as_ref().map(|o| (o.id.as_str(), ChildKind::Crew)),
        )?;
        if let Some(o) = &owner {
            self.add_to_crew(&o.id, &chat, "", Some(card.id.clone()))?;
        }
        self.workspace_host()
            .rename_chat(&chat, &card.title)
            .map_err(|e| failed(e.to_string()))?;
        let by = owner
            .as_ref()
            .map(|o| o.id.clone())
            .or_else(|| {
                let dump = card.source.as_ref()?.dump_id.clone();
                let dump = orch.workspace().read_orchestration_with(|doc| doc.dump(&dump))?;
                self.brain_of(&dump)
            });
        if let Some(by) = by {
            self.mark_started(
                &chat,
                &zeron_proto::StartedBy {
                    chat_id: by,
                    dump_id: card.source.as_ref().map(|s| s.dump_id.clone()),
                    route_id: card.source.as_ref().map(|s| s.route_id.clone()),
                    item_id: Some(card.id.clone()),
                },
            );
        }
        let mut threads = card.threads.clone();
        threads.push(chat.clone());
        orch.patch_item(WorkItemPatch {
            id: Some(card.id.clone()),
            state: Some(WorkState::Active),
            threads: Some(threads),
            waiting: Some(None),
            ..Default::default()
        })
        .map_err(RpcError::Failed)?;
        deliver(self, &chat, &prompt, owner.as_ref().map(|o| o.id.as_str()), When::Now, None)?;
        orch.comms().say(
            CommKind::Spawn,
            owner.as_ref().map_or(super::comms::YOU, |o| o.id.as_str()),
            &chat,
            &card.title,
        );
        Ok(json!({ "id": card.id, "threadId": chat }))
    }

    /// Where a card's thread starts: the card's project or device (on a
    /// device its orchestrator's crew may use); else its orchestrator's
    /// first crew device (else the orchestrator's own project); else the
    /// best project for its text.
    fn card_place(&self, card: &WorkItem, owner: Option<&Orchestrator>, text: &str) -> (Option<String>, Option<String>) {
        let allowed = |device: &Option<String>| {
            owner.is_none_or(|o| o.crew.devices.is_empty() || device.as_ref().is_some_and(|d| o.crew.devices.contains(d)))
        };
        if card.space_id.is_some() || card.device_id.is_some() {
            match self.place(text, card.space_id.clone(), card.device_id.clone()) {
                Ok(placed) if allowed(&placed.1) => return placed,
                Ok(_) => tracing::warn!(card = %card.id, "board: a card's project is off its crew's devices; placing it by the crew policy"),
                Err(error) => tracing::warn!(card = %card.id, %error, "board: a card's project is gone; placing it anew"),
            }
        }
        if let Some(o) = owner {
            if let Some(device) = o.crew.devices.first() {
                return (None, Some(device.clone()));
            }
            if let Ok(Some(chat)) = self.workspace_host().chat(&o.id) {
                return (chat.space_id, Some(chat.device_id));
            }
        }
        self.place(text, None, None).unwrap_or((None, None))
    }

    /// Start `orchestrator`'s Next cards while its crew has room. The
    /// threads started.
    pub(crate) fn start_queued(&self, orchestrator: &str) -> Vec<String> {
        let Some(o) = self.orchestration().orchestrator(orchestrator) else {
            return Vec::new();
        };
        if o.kind != OrchestratorKind::Standard || o.archived || o.paused {
            return Vec::new();
        }
        let Some(max) = o.crew.max_running else {
            return Vec::new();
        };
        let free = (max as usize).saturating_sub(self.crew_running(orchestrator));
        let items = self.orchestration().state().items;
        let mut started = Vec::new();
        for card in next_up(&items, orchestrator).into_iter().take(free) {
            match self.start_item(&card.id, false) {
                Ok(reply) => {
                    if let Some(thread) = reply["threadId"].as_str() {
                        started.push(thread.to_owned());
                    } else {
                        break; // full after all
                    }
                }
                Err(error) => tracing::warn!(card = %card.id, %error, "board: a Next card didn't start"),
            }
        }
        started
    }
}

/// Watch for crew slots freeing (sessions settling, crews and boards
/// changing) and start the Next cards of orchestrators hosted here.
pub(crate) fn start(orchestration: Orchestration) {
    tokio::spawn(async move {
        let (mut local, mut rows) = loop {
            match orchestration.rpc() {
                Some(rpc) => break (rpc.sessions_ref().watch_sessions(), orchestration.workspace().watch_session_rows()),
                None => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        };
        let mut state = orchestration.subscribe();
        loop {
            tokio::select! {
                changed = local.changed() => if changed.is_err() { break },
                changed = rows.changed() => if changed.is_err() { break },
                changed = state.changed() => if changed.is_err() { break },
            }
            tokio::time::sleep(SETTLE).await;
            local.mark_unchanged();
            rows.mark_unchanged();
            state.mark_unchanged();
            let Some(rpc) = orchestration.rpc() else { continue };
            let perf_t = std::time::Instant::now();
            start_due(&orchestration, &rpc);
            tracing::debug!(target: "perf", what = "queue_start_due", us = perf_t.elapsed().as_micros() as u64);
        }
    });
}

/// Start what has room, for every orchestrator with a crew limit hosted on
/// this device. The threads started.
pub(crate) fn start_due(orchestration: &Orchestration, rpc: &EngineRpc) -> Vec<String> {
    // (Runs on every sessions, crew or board change: scanned without the
    // registry lock; `start_queued` reads what it starts again.)
    let due: Vec<String> = orchestration.scan(|state| {
        state
            .orchestrators
            .iter()
            .filter(|o| o.crew.max_running.is_some() && !next_up(&state.items, &o.id).is_empty())
            .map(|o| o.id.clone())
            .collect()
    });
    if due.is_empty() {
        return Vec::new();
    }
    let device = orchestration.device_id();
    let host = orchestration.workspace();
    due.into_iter()
        .filter(|o| host.chat(o).ok().flatten().is_some_and(|c| c.device_id == device))
        .flat_map(|o| rpc.start_queued(&o))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::orchestration::Priority;

    fn card(id: &str, priority: Priority, created: i64, state: WorkState) -> WorkItem {
        serde_json::from_value(json!({
            "id": id, "orchestratorId": "o", "title": id, "state": state, "priority": priority,
            "createdAt": chrono::DateTime::from_timestamp_millis(created).unwrap(),
            "updatedAt": chrono::DateTime::from_timestamp_millis(created).unwrap(),
        }))
        .unwrap()
    }

    #[test]
    fn next_starts_most_urgent_then_oldest() {
        let items = vec![
            card("late-normal", Priority::Normal, 30, WorkState::Planned),
            card("early-low", Priority::Low, 10, WorkState::Planned),
            card("urgent", Priority::Urgent, 40, WorkState::Planned),
            card("early-normal", Priority::Normal, 20, WorkState::Planned),
            card("inbox", Priority::Urgent, 1, WorkState::Inbox),
        ];
        let mut items = items;
        for i in &mut items {
            i.waiting = Some("full".into());
        }
        items.push(card("by-hand", Priority::Urgent, 1, WorkState::Planned));
        let order: Vec<&str> = next_up(&items, "o").iter().map(|i| i.id.as_str()).collect();
        assert_eq!(order, vec!["urgent", "early-normal", "late-normal", "early-low"]);
    }

    #[test]
    fn a_thread_starts_with_the_title_then_the_detail() {
        let mut c = card("Fix login", Priority::Normal, 1, WorkState::Inbox);
        assert_eq!(first_prompt(&c), "Fix login");
        c.detail = "The redirect loops on Safari.".into();
        assert_eq!(first_prompt(&c), "Fix login\n\nThe redirect loops on Safari.");
        c.detail = "Fix login on Safari: the redirect loops.".into();
        assert_eq!(first_prompt(&c), "Fix login on Safari: the redirect loops.");
    }
}
