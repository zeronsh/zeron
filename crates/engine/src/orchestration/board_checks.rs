//! What the board reads of a card's threads, and the person's own hold on
//! a card.
//!
//! - [`Thread::of`]: how a card's thread stands (working, held on its own
//!   work, stopped by a usage limit it will be resumed from, finished,
//!   errored, stopped, gone), from this engine's own runs for the chats it
//!   hosts, else its host's row while that host's heartbeat is fresh, and
//!   its progress row. `derive.rs` reads it; nothing here places a card.
//! - **After a restart**: the chats the stop left at work are resumed
//!   (`crate::restart_resume`, whatever Orchestrators is), and their cards
//!   left alone for [`RESUMED_GRACE_SECS`]: the stop marked their turn
//!   interrupted or errored, which is nobody's word on any card.
//! - **Pause**: only the person pauses or resumes a card; its threads are
//!   stopped and their queued messages held meanwhile, and an agent never
//!   changes a paused card's threads ([`agent_edit_allowed`]).

use chrono::{DateTime, Duration as TimeDelta, Utc};
use serde_json::{Value, json};
use zeron_proto::orchestration::{
    Ask, AskKind, AskOrigin, AskState, CheckKind, CommKind, ItemAsk, LinkKind, Mover, PROGRESS_LIVE_SECS, WorkItem,
    prompt_text::one_line,
};
use zeron_proto::{Chat, DoneStatus, Session, SessionStatus};
use zeron_rpc::RpcError;

use super::Orchestration;
use super::comms::YOU;
pub(crate) use crate::restart_resume::RESUMED_GRACE_SECS;
use crate::rpc::EngineRpc;

/// How a card's thread stands, for its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Thread {
    /// Working, waiting on the person's input, or held on background work
    /// or subagents. `started_at`: when its turn started.
    Live { started_at: Option<DateTime<Utc>> },
    /// Never ran (no session yet).
    Idle,
    /// Its last turn completed.
    Finished,
    /// Its last turn failed.
    Errored { at: Option<DateTime<Utc>> },
    /// Its last turn was stopped.
    Stopped { at: DateTime<Utc> },
    /// Archived or gone.
    Gone,
}

impl Thread {
    /// How `chat`'s thread stands at `now`: from its session (`None`: never
    /// ran) and its progress row, the same truth `progress` reads.
    ///
    /// Live:
    /// - working or waiting on input: by this engine's own run when it hosts
    ///   the chat (`local`: no heartbeat gate, its run is the truth, a long
    ///   silent tool call included), else by its host's row while that
    ///   host's heartbeat is fresh;
    /// - its turn ended into a wait it will be woken from, or subagents still
    ///   running ([`zeron_proto::view`], which drops what a gone engine left);
    /// - its progress row says it's on it (working, waiting, asking) with a
    ///   tool running or a step in the last [`PROGRESS_LIVE_SECS`]: a host
    ///   whose row reached here late still counts.
    ///
    /// A process merely left running in the background (a dev server, a
    /// watcher) isn't work on the card.
    pub(crate) fn of(
        chat: Option<&Chat>,
        session: Option<&Session>,
        progress: Option<&zeron_proto::orchestration::Progress>,
        local: bool,
        now: DateTime<Utc>,
    ) -> Thread {
        use zeron_proto::view;
        if chat.is_none() {
            return Thread::Gone;
        }
        let progressing = progress.filter(|p| {
            p.state.active()
                && (p.tool_since.is_some()
                    || [p.last_step_at, Some(p.updated_at)]
                        .into_iter()
                        .flatten()
                        .any(|at| now - at < TimeDelta::seconds(PROGRESS_LIVE_SECS)))
        });
        let fresh = |s: &Session| local || !view::session_stale(s, now);
        let held = |s: &Session| {
            if local {
                s.waiting.is_some() || s.running_subagents > 0
            } else {
                view::waiting(Some(s), now).is_some() || view::running_subagents(Some(s), now) > 0
            }
        };
        let live = session
            .filter(|s| (matches!(s.status, SessionStatus::Working | SessionStatus::AwaitingInput) && fresh(s)) || held(s))
            .map(|s| s.started_at)
            .or(progressing.map(|p| p.started_at.or(session.and_then(|s| s.started_at))));
        if let Some(started_at) = live {
            // (Archived or not: a crew thread messaged out of the archive
            // works before its row says it's back.)
            return Thread::Live { started_at };
        }
        if chat.is_some_and(|c| c.archived) {
            return Thread::Gone;
        }
        // Stopped by its usage limit: waiting for the reset (its host resumes
        // it then) or running on another account meanwhile. At work: no
        // stale flag, no Blocked, no ask. At work since the limit stopped
        // it, so a card paused after that stays paused.
        if let Some(limit) = session.and_then(|s| s.limited.as_ref()) {
            return Thread::Live { started_at: Some(limit.since) };
        }
        let Some(s) = session else { return Thread::Idle };
        let fresh = fresh(s);
        if s.status == SessionStatus::Errored {
            return Thread::Errored { at: s.last_turn_outcome.map(|o| o.finished_at).or(Some(s.updated_at)) };
        }
        match s.last_turn_outcome {
            Some(o) if o.status == DoneStatus::Errored => Thread::Errored { at: Some(o.finished_at) },
            Some(o) if o.status == DoneStatus::Interrupted => Thread::Stopped { at: o.finished_at },
            None if s.last_completed_turn.is_none() && fresh && s.status == SessionStatus::Idle => Thread::Idle,
            _ => Thread::Finished,
        }
    }

    /// When `chat`'s thread last did anything, as its session and progress
    /// row say: a turn started or ended, a wait began, a step, a tool.
    pub(crate) fn active_at(
        session: Option<&Session>,
        progress: Option<&zeron_proto::orchestration::Progress>,
    ) -> Option<DateTime<Utc>> {
        let s = session.into_iter().flat_map(|s| {
            [s.started_at, s.last_turn_outcome.map(|o| o.finished_at), s.waiting.as_ref().map(|w| w.since)]
        });
        let p = progress.into_iter().flat_map(|p| [p.started_at, p.last_step_at, p.tool_since]);
        s.chain(p).flatten().max()
    }

    pub(crate) fn live(&self) -> bool {
        matches!(self, Thread::Live { .. })
    }
}

/// A card's PR link as the app last saw it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Pr {
    /// Open, or merged/closed; `None` when the app hasn't resolved it.
    pub open: Option<bool>,
    /// Its checks failed.
    pub ci_failed: bool,
}

/// `card`'s PR links, each as `known` (the app's resolved change requests)
/// has it.
pub(crate) fn prs_of(card: &WorkItem, known: &[zeron_proto::ChangeRequestSummary]) -> Vec<Pr> {
    let norm = |u: &str| u.trim().trim_end_matches('/').to_ascii_lowercase();
    card.links
        .iter()
        .filter(|l| l.kind == LinkKind::Pr)
        .map(|l| match known.iter().find(|k| norm(&k.url) == norm(&l.target)) {
            Some(k) => Pr {
                open: Some(k.state == zeron_proto::ChangeRequestState::Open),
                ci_failed: k.signals.as_ref().is_some_and(|s| s.ci == zeron_proto::change_request_assessment::CiState::Failed),
            },
            None => Pr::default(),
        })
        .collect()
}

/// The board's rule for an agent's `upsertItem` on `current` (`None`: a new
/// card): never a paused card's threads (they are what keeps their messages
/// held), nor a flow step's (its run puts them on). The refusal says what to
/// do instead.
pub(crate) fn agent_edit_allowed(current: Option<&WorkItem>, threads_changed: bool) -> Result<(), String> {
    if current.is_some_and(WorkItem::paused) && threads_changed {
        return Err("That card is paused by the person: it stays paused, with its threads, until they resume it. Don't change its threads or message them.".into());
    }
    if current.is_some_and(|c| c.flow.is_some()) && threads_changed {
        return Err("That card is a step of its flow's run: the engine starts its thread when its turn comes. Leave its threads; add to its outcome if you need to.".into());
    }
    Ok(())
}

impl EngineRpc {
    /// The cards the old board check paused itself (an engine shutdown or
    /// recovery read as a Stop, before only the person could pause): no
    /// longer paused, and their threads' held messages sent. Run once at
    /// start; logged. The ones un-paused.
    pub(crate) async fn unpause_engine_pauses(&self, now: DateTime<Utc>) -> Vec<String> {
        let orch = self.orchestration();
        let cards: Vec<WorkItem> = orch.state().items.into_iter().filter(|i| !i.removed && engine_paused(i)).collect();
        let mut done = Vec::new();
        for card in cards {
            orch.workspace().write_orchestration(|doc| {
                if let Some(mut c) = doc.work_item(&card.id) {
                    c.paused_at = None;
                    c.check = None;
                    c.updated_at = now;
                    doc.upsert_work_item(&c);
                }
            });
            for t in &card.threads {
                match self.doc_host_ref().release_queue(t).await {
                    Ok(Some(n)) if n > 0 => tracing::info!(thread = %t, held = n, "board: held messages released"),
                    Ok(_) => {}
                    Err(error) => tracing::warn!(%error, thread = %t, "board: held messages weren't released"),
                }
            }
            tracing::warn!(card = %card.id, title = %card.title, "board: un-paused a card the engine had paused");
            done.push(card.id);
        }
        done
    }

    /// The start-up pass with Orchestrators on: the cards the engine had
    /// paused itself un-paused, the chats the stop left at work resumed
    /// ([`EngineRpc::restart_resume_pass`], the same pass as with it off),
    /// then every card this engine keeps worked out (`board.rs`; a card from
    /// before places gets its row from its old column). `(resumed, moved)`.
    pub(crate) async fn start_up_pass(&self) -> (Vec<String>, Vec<super::board::Moved>) {
        let unpaused = self.unpause_engine_pauses(Utc::now()).await;
        if !unpaused.is_empty() {
            tracing::warn!(cards = ?unpaused, "board: cards the engine had paused (not the person) un-paused");
        }
        let resumed = self.restart_resume_pass();
        let moved = self.board_pass(true, Utc::now());
        (resumed, moved)
    }

    /// The start-up pass's orchestration part, when Orchestrators is turned
    /// on after the engine's start-up resume ran: the cards the engine had
    /// paused itself un-paused, then every card worked out.
    pub(crate) async fn start_up_pass_on(&self) -> (Vec<String>, Vec<super::board::Moved>) {
        let unpaused = self.unpause_engine_pauses(Utc::now()).await;
        if !unpaused.is_empty() {
            tracing::warn!(cards = ?unpaused, "board: cards the engine had paused (not the person) un-paused");
        }
        (Vec::new(), self.board_pass(true, Utc::now()))
    }

    /// Paused cards' threads hosted here keep their queues held (a doc
    /// reopened since lost the hold).
    pub(crate) fn hold_paused_queues(&self) {
        let paused: Vec<String> = self.orchestration().scan(|state| {
            state.items.iter().filter(|i| !i.removed && i.paused()).flat_map(|i| i.threads.iter().cloned()).collect()
        });
        for t in paused {
            self.doc_host_ref().hold_queue(&t);
        }
    }

    /// Raise `ask` on `card` from `from` (its asker: the answer goes there).
    /// Its id.
    pub(crate) fn raise_item_ask(
        &self,
        from: &str,
        card: &WorkItem,
        ask: ItemAsk,
        origin: AskOrigin,
        now: DateTime<Utc>,
    ) -> String {
        let orch = self.orchestration();
        let row = Ask {
            id: uuid::Uuid::new_v4().simple().to_string(),
            from_chat: from.to_owned(),
            orchestrator_id: card.orchestrator_id.clone(),
            item_id: Some(card.id.clone()),
            kind: if ask.options.is_empty() { AskKind::Question } else { AskKind::Decision },
            question: ask.question.trim().to_owned(),
            options: ask.options.into_iter().filter(|o| !o.label.trim().is_empty()).collect(),
            urgent: ask.urgent,
            state: AskState::Open,
            answer: None,
            created_at: now,
            answered_at: None,
            dump_id: None,
            route_id: None,
            quiet: false,
            origin,
            turn_id: None,
            updated_at: None,
            escalated_at: None,
            answered_via: None,
            images: Vec::new(),
            recommendation: ask.recommendation.filter(|r| !r.is_empty()),
            links: Vec::new(),
            context: ask.context.filter(|c| !c.trim().is_empty()),
            deferred_until: None,
        };
        orch.workspace().write_orchestration(|doc| doc.upsert_ask(&row));
        orch.comms().say(CommKind::Ask, from, YOU, &row.question);
        row.id
    }

    /// Whether card `id` has an open ask (on it, or from one of its threads
    /// on no other card).
    pub(crate) fn card_has_open_ask(&self, id: &str) -> bool {
        let state = self.orchestration().state();
        let threads = state.items.iter().find(|i| i.id == id).map(|i| i.threads.clone()).unwrap_or_default();
        state.asks.iter().any(|a| {
            a.state == AskState::Open
                && (a.item_id.as_deref() == Some(id) || (a.item_id.is_none() && threads.contains(&a.from_chat)))
        })
    }

    /// The person pauses card `id`: its threads stopped (the stop button's
    /// command) and their queues held; it waits in Planned, "Paused by you".
    pub(crate) fn pause_item(&self, id: &str, now: DateTime<Utc>) -> Result<Value, RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).filter(|c| !c.removed).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        if card.paused() {
            return Ok(json!({ "id": id }));
        }
        if self.board_card(id).is_some_and(|c| c.closed.is_some()) {
            return Err(RpcError::Failed("That card is done: reopen it first".into()));
        }
        self.with_card(id, now, |row, item| {
            item.paused_at = Some(now);
            item.waiting = None;
            // (Paused, nothing is checked or on its way.)
            row.stall = None;
            row.checking = None;
            row.work_at = row.work_at.filter(|sent| *sent < now);
        });
        for t in &card.threads {
            self.doc_host_ref().hold_queue(t);
            // The stop button's command (a no-op on a thread at rest).
            if let Err(error) = self.doc_host_ref().queue_command(t, zeron_doc::SessionCommandPayload::Interrupt {}) {
                tracing::warn!(%error, thread = %t, "board: a paused card's thread wasn't stopped");
            }
        }
        orch.comms().say(CommKind::Message, YOU, card.orchestrator_id.as_deref().unwrap_or(YOU), &format!("Paused: {}", one_line(&card.title, 80)));
        tracing::info!(card = %id, title = %card.title, threads = ?card.threads, "board: the person paused a card");
        self.derive_cards(&[id.to_owned()], Mover::You, now);
        Ok(json!({ "id": id }))
    }

    /// The person resumes paused card `id`: its threads' held messages
    /// sent, else each told to carry on; with no thread, it is started.
    /// `carry_on: false` only takes the pause off (an older app's drag to a
    /// waiting column): its held messages still go, nothing else is sent.
    pub(crate) async fn resume_item(&self, id: &str, carry_on: bool, now: DateTime<Utc>) -> Result<Value, RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).filter(|c| !c.removed).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        let live: Vec<String> = card
            .threads
            .iter()
            .filter(|t| self.workspace_host().chat(t).ok().flatten().is_some_and(|c| !c.archived))
            .cloned()
            .collect();
        self.with_card(id, now, |row, item| {
            item.paused_at = None;
            row.held = false;
        });
        orch.comms().say(CommKind::Message, YOU, card.orchestrator_id.as_deref().unwrap_or(YOU), &format!("Resumed: {}", one_line(&card.title, 80)));
        tracing::info!(card = %id, title = %card.title, carry_on, threads = ?card.threads, "board: the person resumed a paused card");
        // Its threads' held messages go now, wherever it was taken (a held
        // queue under a card no longer paused would wait for nothing).
        let mut released = std::collections::HashSet::new();
        for t in &live {
            match self.doc_host_ref().release_queue(t).await {
                Ok(Some(n)) if n > 0 => {
                    released.insert(t.clone());
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, thread = %t, "board: a resumed card's held messages weren't sent"),
            }
        }
        if !carry_on {
            self.derive_cards(&[id.to_owned()], Mover::You, now);
            return Ok(json!({ "id": id }));
        }
        if live.is_empty() {
            // Nothing to resume: started afresh (or queued for its crew).
            let started = self.start_item(id, true);
            self.derive_cards(&[id.to_owned()], Mover::You, now);
            return started;
        }
        // Each thread carries on (its held messages, else told to).
        for t in &live {
            if released.contains(t) {
                self.work_sent(t, now);
            } else {
                let text = format!("[Resumed] The person resumed \"{}\": carry on where you stopped.", one_line(&card.title, 80));
                super::ops::deliver(self, t, &text, None, super::ops::When::Next, Some(zeron_doc::Injected::resumed(&card.id)))?;
            }
        }
        self.derive_cards(&[id.to_owned()], Mover::You, now);
        Ok(json!({ "id": id }))
    }
}

/// The threads of a paused card: an agent's message to one waits in its
/// queue until the person resumes the card.
pub(crate) fn paused_thread(orch: &Orchestration, chat: &str) -> bool {
    paused_card_of(orch, chat).is_some()
}

/// The paused card `chat` is a thread of, if any (its id).
pub(crate) fn paused_card_of(orch: &Orchestration, chat: &str) -> Option<String> {
    orch.state().items.into_iter().find(|i| !i.removed && i.paused() && i.threads.iter().any(|t| t == chat)).map(|i| i.id)
}

/// What a paused card's held sends say why: the person paused it.
pub(crate) fn held_reason(card: &str) -> String {
    format!("card {card} is paused by the person")
}

/// The note the board check set when it paused a card itself (before only
/// the person could): such a pause had no person behind it.
pub(crate) const ENGINE_PAUSE_NOTE: &str = "Paused: its thread was stopped";

/// Whether `card`'s pause was the board check's own, not the person's: it
/// carries the check's note (the person's Pause clears any note).
pub(crate) fn engine_paused(card: &WorkItem) -> bool {
    card.paused()
        && card.check.as_ref().is_some_and(|c| c.kind == CheckKind::Moved && c.note == ENGINE_PAUSE_NOTE)
}

#[cfg(test)]
#[path = "board_checks_tests.rs"]
mod tests;