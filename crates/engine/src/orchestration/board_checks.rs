//! What the board reads of a card's threads, and the person's own hold on
//! a card.
//!
//! - [`Thread::of`]: how a card's thread stands (working, held on its own
//!   work, stopped by a usage limit it will be resumed from, finished,
//!   errored, stopped, gone), from this engine's own runs for the chats it
//!   hosts, else its host's row while that host's heartbeat is fresh, and
//!   its progress row. `derive.rs` reads it; nothing here places a card.
//! - **After a restart**: the chats the stop left at work are resumed
//!   ([`EngineRpc::start_up_pass`]), and their cards left alone for
//!   [`RESUMED_GRACE_SECS`]: the stop marked their turn interrupted or
//!   errored, which is nobody's word on any card.
//! - **Pause**: only the person pauses or resumes a card; its threads are
//!   stopped and their queued messages held meanwhile, and an agent never
//!   changes a paused card's threads ([`agent_edit_allowed`]).

use std::collections::HashMap;

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
use crate::rpc::EngineRpc;

/// How long a card of a thread resumed after a restart is left alone.
pub(crate) const RESUMED_GRACE_SECS: i64 = 120;

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

/// A chat to resume after a restart, and the background work its wait was
/// on that the restart stopped (empty: it was mid-turn or running
/// subagents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resume {
    pub chat: String,
    pub waited_on: Vec<String>,
    /// Its subagents the stop ended (their sub-chats; empty when only their
    /// count is known): `None` when it had none.
    pub subagents: Option<Vec<String>>,
}

/// The chats to resume after a restart, hosted here: `before` is this
/// device's rows as stored when the engine stopped, `waits` the chats whose
/// turn had ended into a wait on their own background work when the
/// shutdown began (the shutdown ends the wait, so their rows no longer say
/// it). Resumed: a crew thread that was mid-turn, running subagents, or
/// waiting; any other chat (a standalone one) only when it was waiting,
/// since the engine's own recovery revives a turn cut off mid-way and
/// nothing revives a wait. Not when something ran it since (`now`: this
/// engine's sessions since it started; a turn in flight or a newer
/// completed turn), not a thread of a card the person paused, not one
/// archived, not the Brain dump, not one whose last turn ended normally.
/// Pure.
pub(crate) fn to_resume(
    before: &[Session],
    waits: &[(String, Vec<String>)],
    subagents: &[(String, Vec<String>)],
    now: &HashMap<String, Session>,
    state: &zeron_proto::orchestration::OrchestrationState,
    chats: &HashMap<String, Chat>,
    at: DateTime<Utc>,
) -> Vec<Resume> {
    // chat → (its row at the stop, mid-turn, what its wait was on, its
    // subagents)
    type Seen<'a> = (Option<&'a Session>, bool, Option<Vec<String>>, Option<Vec<String>>);
    let mut seen: std::collections::BTreeMap<&str, Seen> = Default::default();
    for b in before {
        let wait = b.waiting.as_ref().map(|w| w.on.iter().map(|o| o.label.clone()).collect());
        // (A crash leaves the row as it was: its subagents' count, no names.)
        let subs = (b.running_subagents > 0).then(Vec::new);
        seen.insert(&b.chat_id, (Some(b), b.status == SessionStatus::Working, wait, subs));
    }
    for (chat, labels) in waits {
        let entry = seen.entry(chat.as_str()).or_insert((None, false, None, None));
        entry.2 = Some(labels.clone());
    }
    for (chat, docs) in subagents {
        let entry = seen.entry(chat.as_str()).or_insert((None, false, None, None));
        entry.3 = Some(docs.clone());
    }
    let crew = |c: &str| state.crew.iter().any(|m| m.chat_id == c && !m.released);
    let brain = |c: &str| {
        state.orchestrators.iter().any(|o| o.id == c && o.kind != zeron_proto::orchestration::OrchestratorKind::Standard)
    };
    // Waiting on its usage limit: its own resume goes at the reset (one
    // "[Resumed]", not two).
    let limit_waiting = |row: &Option<&Session>| row.is_some_and(|b| b.limited.as_ref().is_some_and(|l| l.waiting()));
    seen.into_iter()
        .filter(|(c, (_, turn, wait, subs))| (*turn && crew(c)) || wait.is_some() || subs.is_some())
        .filter(|(_, (row, _, _, _))| !limit_waiting(row))
        .filter(|(c, _)| !brain(c))
        .filter(|(c, _)| chats.get(*c).is_some_and(|chat| !chat.archived))
        .filter(|(c, _)| !state.items.iter().any(|i| !i.removed && i.paused() && i.threads.iter().any(|t| t == c)))
        .filter(|(c, (row, _, _, _))| match now.get(*c) {
            // (A Working row left from before the stop has no heartbeat:
            // stale, not a turn in flight.)
            Some(n) => {
                let in_flight = matches!(n.status, SessionStatus::Working | SessionStatus::AwaitingInput)
                    && !zeron_proto::view::session_stale(n, at);
                !in_flight && row.is_some_and(|b| n.last_completed_turn == b.last_completed_turn)
            }
            None => true,
        })
        .map(|(c, (_, _, wait, subs))| Resume { chat: c.to_owned(), waited_on: wait.unwrap_or_default(), subagents: subs })
        .collect()
}

impl EngineRpc {
    /// The chats the stop left at work (`before`: this device's rows when the
    /// engine stopped; and the waits its shutdown ended, read from the run
    /// journal), each sent [`resumed_after_restart`] once ([`to_resume`]).
    /// The ones resumed.
    ///
    /// [`resumed_after_restart`]: zeron_proto::orchestration::prompt_text::resumed_after_restart
    pub(crate) fn resume_after_restart(&self, before: Vec<Session>) -> Vec<String> {
        let waits = self.sessions_ref().take_shutdown_waits();
        let subagents = self.sessions_ref().take_shutdown_subagents();
        if before.is_empty() && waits.is_empty() && subagents.is_empty() {
            return Vec::new();
        }
        let orch = self.orchestration();
        let chats: HashMap<String, Chat> =
            orch.workspace().read_chats().unwrap_or_default().into_iter().map(|c| (c.id.clone(), c)).collect();
        // What ran here since the start: this engine's own sessions (its
        // stored rows still say what they said at the stop, heartbeat or not
        // after a quick restart).
        let now: HashMap<String, Session> =
            self.sessions_ref().watch_sessions().borrow().iter().map(|s| (s.chat_id.clone(), s.clone())).collect();
        let mut resumed = Vec::new();
        for r in to_resume(&before, &waits, &subagents, &now, &orch.state(), &chats, Utc::now()) {
            let text = match &r.subagents {
                Some(subs) => zeron_proto::orchestration::prompt_text::resumed_after_restart_subagents(subs),
                None => zeron_proto::orchestration::prompt_text::resumed_after_restart(&r.waited_on),
            };
            let injected = zeron_doc::Injected { chat_id: Some(r.chat.clone()), ..zeron_doc::Injected::of(zeron_doc::injected::RESUMED) };
            match super::ops::deliver(self, &r.chat, &text, None, super::ops::When::Next, Some(injected)) {
                Ok(_) => resumed.push(r.chat),
                Err(error) => tracing::warn!(%error, chat = %r.chat, "board: a chat wasn't resumed after the restart"),
            }
        }
        resumed
    }

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

    /// The start-up pass: the chats the stop left at work resumed
    /// ([`EngineRpc::resume_after_restart`]), then every card this engine
    /// keeps worked out (`board.rs`; a card from before places gets its row
    /// from its old column). Every chat that was at work at the stop
    /// (resumed here, or revived by the engine's own recovery of a turn cut
    /// off by a crash) keeps its cards where they are for
    /// [`RESUMED_GRACE_SECS`]: the stop or the crash marked its turn
    /// interrupted or errored, which isn't its work failing. `(resumed,
    /// moved)`.
    pub(crate) async fn start_up_pass(&self) -> (Vec<String>, Vec<super::board::Moved>) {
        let orch = self.orchestration();
        let unpaused = self.unpause_engine_pauses(Utc::now()).await;
        if !unpaused.is_empty() {
            tracing::warn!(cards = ?unpaused, "board: cards the engine had paused (not the person) un-paused");
        }
        let before = orch.take_before_stop();
        let mut at_work: Vec<String> = before
            .iter()
            .filter(|b| b.status == SessionStatus::Working || b.waiting.is_some() || b.running_subagents > 0)
            .map(|b| b.chat_id.clone())
            .collect();
        let mut resumed = self.resume_after_restart(before);
        orch.note_resumed(&resumed, Utc::now());
        resumed.extend(self.resume_cut_off_sends());
        at_work.extend(resumed.iter().cloned());
        at_work.sort();
        at_work.dedup();
        orch.note_resumed(&at_work, Utc::now());
        let moved = self.board_pass(true, Utc::now());
        (resumed, moved)
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

    /// The chats a message was being sent to when the engine stopped (the
    /// send consumed, never resolved: the drain's dead-command sweep rejects
    /// it), each resumed once with that message quoted
    /// ([`resumed_after_cut_off_send`]), crew, orchestrator or standalone
    /// alike. Not sent again as it was: it may have started its turn and done
    /// part of its work before the stop. Skipped: a chat resumed after this
    /// restart already, one at work now, a thread of a paused card, the Brain
    /// dump, one gone. The ones resumed.
    ///
    /// [`resumed_after_cut_off_send`]: zeron_proto::orchestration::prompt_text::resumed_after_cut_off_send
    pub(crate) fn resume_cut_off_sends(&self) -> Vec<String> {
        let cut = self.doc_host_ref().take_cut_off_sends();
        if cut.is_empty() {
            return Vec::new();
        }
        let orch = self.orchestration();
        let state = orch.state();
        let now = Utc::now();
        let already = orch.resumed_since(now, TimeDelta::seconds(RESUMED_GRACE_SECS));
        // One per chat: the last message cut off.
        let by_chat: std::collections::BTreeMap<String, String> = cut.into_iter().collect();
        let mut resumed = Vec::new();
        for (chat, sent) in by_chat {
            let paused = state.items.iter().any(|i| !i.removed && i.paused() && i.threads.contains(&chat));
            let brain = state.orchestrators.iter().any(|o| {
                o.id == chat && o.kind != zeron_proto::orchestration::OrchestratorKind::Standard
            });
            let gone = self.workspace_host().chat(&chat).ok().flatten().is_none();
            // Waiting on its usage limit: its own resume goes at the reset.
            let limit_waiting = self
                .sessions_ref()
                .session_status(&chat)
                .and_then(|s| s.limited)
                .or_else(|| {
                    self.workspace_host().read_sessions().ok()?.into_iter().find(|s| s.chat_id == chat)?.limited
                })
                .is_some_and(|l| l.waiting());
            if already.contains(&chat) || paused || brain || gone || limit_waiting || self.sessions_ref().turn_in_flight(&chat) {
                continue;
            }
            let text = zeron_proto::orchestration::prompt_text::resumed_after_cut_off_send(&sent);
            let injected = zeron_doc::Injected { chat_id: Some(chat.clone()), ..zeron_doc::Injected::of(zeron_doc::injected::RESUMED) };
            match super::ops::deliver(self, &chat, &text, None, super::ops::When::Next, Some(injected)) {
                Ok(_) => resumed.push(chat),
                Err(error) => tracing::warn!(%error, %chat, "board: a chat whose send the stop cut off wasn't resumed"),
            }
        }
        orch.note_resumed(&resumed, now);
        if !resumed.is_empty() {
            tracing::info!(?resumed, "board: chats whose send the stop cut off resumed");
        }
        resumed
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