//! The chats the engine's last stop left at work, resumed once on its next
//! start (crew, orchestrator or standalone alike, whatever Orchestrators
//! is): a wait on its own background work the shutdown ended, subagents it
//! stopped, a crew thread cut off mid-turn, a send cut off before its turn
//! began ([`to_resume`], [`EngineRpc::resume_cut_off_sends`]). The engine's
//! own recovery revives a standalone turn cut off mid-way; nothing else
//! revives a wait.
//!
//! What orchestration says of a chat (a crew thread, a paused card's, the
//! Brain dump) is read only while Orchestrators is on
//! ([`EngineRpc::orchestration_view`]); off, every chat is a plain one. With
//! it on, its start-up pass wraps this one (`board_checks.rs`).

use std::collections::HashMap;

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::{Chat, Session, SessionStatus};

use crate::orchestration::ops::{When, deliver};
use crate::resumes::Resumes;
use crate::rpc::EngineRpc;

/// How long a card of a thread resumed after a restart is left alone.
pub(crate) const RESUMED_GRACE_SECS: i64 = 120;

/// How long after a start the start-up resume runs (synced rows land first).
const AFTER_START: std::time::Duration = std::time::Duration::from_secs(25);
/// How often the sends cut off by the stop are looked for again (one is
/// found when its chat's commands are next drained).
const EVERY: std::time::Duration = std::time::Duration::from_secs(60);

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
        let chats: HashMap<String, Chat> =
            self.workspace_host().read_chats().unwrap_or_default().into_iter().map(|c| (c.id.clone(), c)).collect();
        // What ran here since the start: this engine's own sessions (its
        // stored rows still say what they said at the stop, heartbeat or not
        // after a quick restart).
        let now: HashMap<String, Session> =
            self.sessions_ref().watch_sessions().borrow().iter().map(|s| (s.chat_id.clone(), s.clone())).collect();
        let mut resumed = Vec::new();
        for r in to_resume(&before, &waits, &subagents, &now, &self.orchestration_view(), &chats, Utc::now()) {
            let text = match &r.subagents {
                Some(subs) => zeron_proto::orchestration::prompt_text::resumed_after_restart_subagents(subs),
                None => zeron_proto::orchestration::prompt_text::resumed_after_restart(&r.waited_on),
            };
            let injected = zeron_doc::Injected { chat_id: Some(r.chat.clone()), ..zeron_doc::Injected::of(zeron_doc::injected::RESUMED) };
            match deliver(self, &r.chat, &text, None, When::Next, Some(injected)) {
                Ok(_) => resumed.push(r.chat),
                Err(error) => tracing::warn!(%error, chat = %r.chat, "a chat wasn't resumed after the restart"),
            }
        }
        resumed
    }

    /// The start-up resume: the chats the stop left at work resumed
    /// ([`EngineRpc::resume_after_restart`], [`EngineRpc::resume_cut_off_sends`]).
    /// Every chat that was at work at the stop (resumed here, or revived by
    /// the engine's own recovery of a turn cut off by a crash) is noted for
    /// [`RESUMED_GRACE_SECS`]: the stop or the crash marked its turn
    /// interrupted or errored, which isn't its work failing. The ones resumed.
    pub(crate) fn restart_resume_pass(&self) -> Vec<String> {
        let resumes = self.resumes();
        let before = resumes.take_before_stop();
        let mut at_work: Vec<String> = before
            .iter()
            .filter(|b| b.status == SessionStatus::Working || b.waiting.is_some() || b.running_subagents > 0)
            .map(|b| b.chat_id.clone())
            .collect();
        let mut resumed = self.resume_after_restart(before);
        resumes.note_resumed(&resumed, Utc::now());
        resumed.extend(self.resume_cut_off_sends());
        at_work.extend(resumed.iter().cloned());
        at_work.sort();
        at_work.dedup();
        resumes.note_resumed(&at_work, Utc::now());
        resumed
    }

    /// The chats a message was being sent to when the engine stopped (the
    /// send consumed, never resolved: the drain's dead-command sweep rejects
    /// it), each resumed once with that message quoted
    /// ([`resumed_after_cut_off_send`]), crew, orchestrator or standalone
    /// alike. Not sent again as it was: it may have started its turn and done
    /// part of its work before the stop. Skipped: a chat resumed after this
    /// restart already, one at work now, one gone, and (while Orchestrators
    /// is on) a thread of a paused card or the Brain dump. The ones resumed.
    ///
    /// [`resumed_after_cut_off_send`]: zeron_proto::orchestration::prompt_text::resumed_after_cut_off_send
    pub(crate) fn resume_cut_off_sends(&self) -> Vec<String> {
        let cut = self.doc_host_ref().take_cut_off_sends();
        if cut.is_empty() {
            return Vec::new();
        }
        let state = self.orchestration_view();
        let now = Utc::now();
        let already = self.resumes().resumed_since(now, TimeDelta::seconds(RESUMED_GRACE_SECS));
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
                .and_then(|s| s.live.limited)
                .or_else(|| {
                    self.workspace_host().read_sessions().ok()?.into_iter().find(|s| s.chat_id == chat)?.live.limited
                })
                .is_some_and(|l| l.waiting());
            if already.contains(&chat) || paused || brain || gone || limit_waiting || self.sessions_ref().turn_in_flight(&chat) {
                continue;
            }
            let text = zeron_proto::orchestration::prompt_text::resumed_after_cut_off_send(&sent);
            let injected = zeron_doc::Injected { chat_id: Some(chat.clone()), ..zeron_doc::Injected::of(zeron_doc::injected::RESUMED) };
            match deliver(self, &chat, &text, None, When::Next, Some(injected)) {
                Ok(_) => resumed.push(chat),
                Err(error) => tracing::warn!(%error, %chat, "a chat whose send the stop cut off wasn't resumed"),
            }
        }
        self.resumes().note_resumed(&resumed, now);
        if !resumed.is_empty() {
            tracing::info!(?resumed, "chats whose send the stop cut off resumed");
        }
        resumed
    }
}

/// The task: the start-up resume a moment after the start (with
/// Orchestrators on, its start-up pass, which wraps it), then the sends the
/// stop cut off looked for every [`EVERY`].
pub(crate) fn start(resumes: Resumes) {
    tokio::spawn(async move {
        tokio::time::sleep(AFTER_START).await;
        if let Some(rpc) = resumes.rpc() {
            let resumed = match rpc.active_orchestration() {
                Some(orch) if orch.take_start_up() => {
                    let (resumed, moved) = rpc.start_up_pass().await;
                    tracing::info!(?resumed, cards = moved.len(), "board: start-up pass");
                    resumed
                }
                _ => rpc.restart_resume_pass(),
            };
            tracing::info!(?resumed, "start-up resume");
        }
        resumes.mark_started_up();
        let mut tick = tokio::time::interval(EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tick.tick().await;
            let Some(rpc) = resumes.rpc() else { break };
            rpc.resume_cut_off_sends();
        }
    });
}

#[cfg(test)]
#[path = "restart_resume_tests.rs"]
mod tests;
