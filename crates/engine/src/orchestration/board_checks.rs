//! The board stays honest: what a card's column says is checked against its
//! threads and asks, by the engine, not left to the agents.
//!
//! Rules (refused in `upsertItem`, see `ops.rs`):
//! - An agent moves a card to Blocked only with an open ask on it, or one
//!   raised with the move (`item_upsert` `ask`).
//! - Only the person pauses or resumes a card (Blocked's Paused group); an
//!   agent never changes a paused card's state.
//!
//! Checks ([`assess`], applied by [`EngineRpc::check_boards`]):
//! - **Blocked** (not paused) needs an open ask: its ask answered or
//!   dismissed, back to Now (Up next with no thread); never asked, one is
//!   raised from the card (a thread that errored: Retry / Drop).
//! - **Paused**: its thread working again (started after the pause), back to
//!   Now.
//! - **Now** needs a live thread, or an open ask it waits on ("Waiting on
//!   you" on the card). Errored: Blocked with an ask. Stopped:
//!   Paused. Finished (or archived after finishing): flagged stale, its
//!   orchestrator told, and in Review after [`STALE_TO_REVIEW_SECS`]. A
//!   thread its usage limit stopped is at work (it is resumed at the
//!   reset, `orchestration::limits`). No
//!   thread at all after [`NO_THREAD_GRACE_SECS`]: Up next.
//! - **Up next / Later**: a thread at work on it, Now.
//! - **Review** means an open ask or an open PR (or its route's review
//!   line): a thread at work on it, Now; neither, flagged; its PR's CI
//!   failing (as the app last saw it, no polling of its own), flagged.
//! - **Done** with a thread still at work: flagged.
//!
//! Every move the engine makes is marked as a move (`moved_at`), with a
//! `moved` note on the card; a flag tells its orchestrator once. Runs a
//! moment after any session, ask or board change, every minute (the grace
//! timers), and once at start, over every board this engine keeps (a card
//! whose orchestrator, else first thread, is hosted here).

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration as TimeDelta, Utc};
use serde_json::{Value, json};
use zeron_proto::orchestration::{
    AnswerVia, Ask, AskKind, AskOption, AskOrigin, AskState, CardCheck, CheckKind, CommKind, ItemAsk, LinkKind, NO_THREAD_GRACE_SECS,
    PROGRESS_LIVE_SECS, STALE_GRACE_SECS,
    STALE_TO_REVIEW_SECS, WorkItem, WorkItemPatch, WorkState, prompt_text::one_line,
};
use zeron_proto::{Chat, DoneStatus, Session, SessionStatus};
use zeron_rpc::RpcError;

use super::Orchestration;
use super::comms::YOU;
use crate::rpc::EngineRpc;

/// How long after a change the boards are checked (changes in a burst, one
/// check).
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);
/// How often the boards are checked anyway (the grace timers).
const EVERY: std::time::Duration = std::time::Duration::from_secs(60);
/// How long after a start the first check runs (synced rows land first).
const AFTER_START: std::time::Duration = std::time::Duration::from_secs(25);

/// How long a card of a thread resumed after a restart is left alone.
pub(crate) const RESUMED_GRACE_SECS: i64 = 120;

/// The errored-thread ask's options.
pub(crate) const RETRY: &str = "Retry";
pub(crate) const DROP: &str = "Drop it";

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

    fn live(&self) -> bool {
        matches!(self, Thread::Live { .. })
    }

    /// At work, its turn started after `since` (or when, unknown).
    fn live_since(&self, since: DateTime<Utc>) -> bool {
        matches!(self, Thread::Live { started_at } if started_at.is_none_or(|s| s > since))
    }
}

/// What the check reads of one card.
#[derive(Debug, Clone)]
pub(crate) struct CardView<'a> {
    pub card: &'a WorkItem,
    pub threads: Vec<(String, Thread)>,
    /// An ask on it (or from one of its threads, on no other card) is open.
    pub open_ask: bool,
    /// Its latest closed ask: when, and whether answered (else dismissed).
    pub last_closed: Option<(DateTime<Utc>, bool)>,
    /// It came from a Brain dump route (its route keeps it, while it has no
    /// thread).
    pub routed: bool,
    /// Its route left the person something to review.
    pub route_review: bool,
    /// Its PR links, as the app last saw each (unknown: not looked up here).
    pub prs: Vec<Pr>,
    /// When any of its threads last did anything ([`Thread::active_at`]).
    pub active_at: Option<DateTime<Utc>>,
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

/// An ask the engine raises for a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Raise {
    /// Blocked and never asked about.
    Blocked,
    /// Its thread stopped on an error.
    Errored,
}

/// What the check changes on a card. Empty: nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Verdict {
    pub state: Option<WorkState>,
    /// `Some(true)`: paused now; `Some(false)`: no longer.
    pub paused: Option<bool>,
    /// `Some(None)` clears the check.
    pub check: Option<Option<CardCheck>>,
    pub raise: Option<Raise>,
    /// Threads gone (archived): off the card, kept as chat links.
    pub drop_threads: Vec<String>,
    /// Tell its orchestrator (the check's note).
    pub nudge: bool,
    /// Asks the board check raised on the card whose condition cleared:
    /// `(ask id, why)`, closed "resolved: why".
    pub resolve: Vec<(String, String)>,
}

impl Verdict {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Verdict::default()
    }

    fn moved(state: WorkState, note: &str, now: DateTime<Utc>) -> Verdict {
        Verdict { state: Some(state), check: Some(Some(note_of(CheckKind::Moved, note, now))), ..Default::default() }
    }

    fn flag(kind: CheckKind, note: &str, now: DateTime<Utc>) -> Verdict {
        Verdict { check: Some(Some(note_of(kind, note, now))), nudge: true, ..Default::default() }
    }
}

fn note_of(kind: CheckKind, note: &str, now: DateTime<Utc>) -> CardCheck {
    CardCheck { kind, note: note.to_owned(), since: now, nudged_at: None }
}

/// A flag of `kind` the card no longer earns, cleared (a `moved` note
/// stays until the card is moved again).
fn clear_unless(card: &WorkItem, keep: &[CheckKind]) -> Verdict {
    match &card.check {
        Some(c) if c.kind != CheckKind::Moved && !keep.contains(&c.kind) => {
            Verdict { check: Some(None), ..Default::default() }
        }
        _ => Verdict::default(),
    }
}

/// When `view`'s card last came to be as it is, or its threads last did
/// anything: made, moved, given a thread, a turn, a step, a wait. (A turn
/// that just ended is often a moment from its wait, or the next turn.)
fn grace_from(view: &CardView) -> DateTime<Utc> {
    let card = view.card;
    [Some(card.created_at), card.moved_at, card.threads_at, view.active_at]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(card.created_at)
}

/// What the board check changes on `view`'s card at `now`. Pure.
pub(crate) fn assess(view: &CardView, now: DateTime<Utc>) -> Verdict {
    let card = view.card;
    if card.removed {
        return Verdict::default();
    }
    // When it came into its column (a move by hand, by an agent, or by
    // this check), else when it was made.
    let since = card.moved_at.unwrap_or(card.created_at);
    let threads = &view.threads;
    let gone: Vec<String> = threads.iter().filter(|(_, t)| *t == Thread::Gone).map(|(id, _)| id.clone()).collect();
    let here: Vec<&Thread> = threads.iter().map(|(_, t)| t).filter(|t| **t != Thread::Gone).collect();
    let any_live = here.iter().any(|t| t.live());
    let live_since = |at: DateTime<Utc>| here.iter().any(|t| t.live_since(at));
    let flagged = |kind: CheckKind| card.check.as_ref().is_some_and(|c| c.kind == kind);
    match card.state {
        WorkState::Blocked if card.paused_at.is_some() => {
            let paused = card.paused_at.unwrap_or(since);
            if live_since(paused) {
                Verdict { paused: Some(false), ..Verdict::moved(WorkState::Active, "Back in Now: its thread is working again", now) }
            } else {
                clear_unless(card, &[])
            }
        }
        WorkState::Blocked => {
            if view.open_ask {
                return clear_unless(card, &[]);
            }
            let back = if here.is_empty() { WorkState::Planned } else { WorkState::Active };
            match view.last_closed {
                Some((at, answered)) if at >= since => {
                    let note = match (answered, back) {
                        (true, WorkState::Active) => "Back in Now: its ask was answered",
                        (true, _) => "Back to Up next: its ask was answered",
                        (false, WorkState::Active) => "Back in Now: its ask was dismissed",
                        (false, _) => "Back to Up next: its ask was dismissed",
                    };
                    Verdict::moved(back, note, now)
                }
                _ if card.orchestrator_id.is_some() || !here.is_empty() => {
                    let errored = here.iter().any(|t| matches!(t, Thread::Errored { .. }));
                    Verdict { raise: Some(if errored { Raise::Errored } else { Raise::Blocked }), ..clear_unless(card, &[]) }
                }
                // Loose, with no thread: nobody to ask for.
                _ => Verdict::moved(WorkState::Planned, "Back to Up next: nothing it waited on was asked", now),
            }
        }
        WorkState::Active => {
            if any_live {
                return clear_unless(card, &[]);
            }
            // Waiting on the person's answer to its ask ("Waiting on you"
            // on the card): true as it is.
            if view.open_ask {
                return clear_unless(card, &[]);
            }
            // A routed card with no thread is its route's (the target is an
            // orchestrator itself).
            if view.routed && threads.is_empty() {
                return Verdict::default();
            }
            let mut v = if here.iter().any(|t| matches!(t, Thread::Errored { at } if at.is_none_or(|a| a > since))) {
                Verdict {
                    raise: Some(Raise::Errored),
                    ..Verdict::moved(WorkState::Blocked, "Blocked: its thread stopped on an error", now)
                }
            // (A stopped thread never pauses its card: the board can't tell
            // the person's Stop from an engine shutdown, a recovery, a crash,
            // a limit or an agent's interrupt. Only the person's own Pause
            // pauses a card. A stop reads as its work ended.)
            } else if here.iter().any(|t| matches!(t, Thread::Finished | Thread::Errored { .. } | Thread::Stopped { .. }))
                || (here.is_empty() && (!gone.is_empty() || card.links.iter().any(|l| l.kind == LinkKind::Chat)))
            {
                // Its work ended and nothing moved it.
                match &card.check {
                    Some(c) if c.kind == CheckKind::Stale => {
                        // Checked again as it falls due: a thread that did
                        // anything since the flag (a turn, a step, a wait)
                        // is at work on it after all: the flag goes, no move.
                        if view.active_at.is_some_and(|at| at > c.since) {
                            Verdict { check: Some(None), ..Default::default() }
                        } else if now - c.since >= TimeDelta::seconds(STALE_TO_REVIEW_SECS) {
                            Verdict::moved(WorkState::Review, "In Review: its thread finished and nothing moved it", now)
                        } else {
                            Verdict::default()
                        }
                    }
                    // Just made, moved here, or given a thread (its thread is
                    // about to start), or its thread was at it a moment ago.
                    _ if now - grace_from(view) < TimeDelta::seconds(STALE_GRACE_SECS) => Verdict::default(),
                    _ => Verdict::flag(CheckKind::Stale, "Its thread finished; still in Now", now),
                }
            } else if now - since >= TimeDelta::seconds(NO_THREAD_GRACE_SECS) {
                // No thread, or none that ever ran.
                Verdict::moved(WorkState::Planned, "Back to Up next: no thread was working on it", now)
            } else {
                Verdict::default()
            };
            // (A thread just put on it may still be archived: the message
            // that starts it brings it back. Not dropped within the grace.)
            if now - grace_from(view) >= TimeDelta::seconds(STALE_GRACE_SECS) {
                v.drop_threads = gone;
            }
            v
        }
        WorkState::Planned | WorkState::Inbox => {
            if live_since(since) {
                Verdict::moved(WorkState::Active, "In Now: its thread is working on it", now)
            } else {
                clear_unless(card, &[])
            }
        }
        WorkState::Review => {
            if live_since(since) {
                return Verdict::moved(WorkState::Active, "Back in Now: its thread is working again", now);
            }
            // Review holds what waits on the person: an open ask, an open PR
            // (or one the app hasn't looked up), or its route's review line.
            let open_prs: Vec<&Pr> = view.prs.iter().filter(|p| p.open != Some(false)).collect();
            if open_prs.iter().any(|p| p.ci_failed) {
                return if flagged(CheckKind::CiFailing) {
                    Verdict::default()
                } else {
                    Verdict::flag(CheckKind::CiFailing, "CI failing on its PR", now)
                };
            }
            if view.open_ask || !open_prs.is_empty() || view.route_review {
                return clear_unless(card, &[]);
            }
            if flagged(CheckKind::NothingToReview) {
                Verdict::default()
            } else if view.prs.is_empty() {
                Verdict::flag(CheckKind::NothingToReview, "Nothing to review: no open ask or PR", now)
            } else {
                Verdict::flag(CheckKind::NothingToReview, "Its PR is merged or closed: nothing left to review", now)
            }
        }
        WorkState::Done => {
            if live_since(since) {
                if flagged(CheckKind::StillWorking) {
                    Verdict::default()
                } else {
                    Verdict::flag(CheckKind::StillWorking, "Done, but its thread is still working", now)
                }
            } else {
                clear_unless(card, &[])
            }
        }
        WorkState::Dropped | WorkState::Other => Verdict::default(),
    }
}

/// The board's rules for an agent's `upsertItem` on `current` (`None`: a
/// new card) to `state`: never a paused card's state or threads, nor a flow
/// step's (its run moves it); Blocked only with an
/// open ask on the card (`open_ask`) or one raised with the move
/// (`ask_given`). The refusal says what to do instead.
pub(crate) fn agent_move_allowed(
    current: Option<&WorkItem>,
    state: Option<WorkState>,
    threads_changed: bool,
    open_ask: bool,
    ask_given: bool,
) -> Result<(), String> {
    // A paused card's state and threads are the person's: its threads are
    // what keeps their messages held.
    if current.is_some_and(WorkItem::paused) && (state.is_some() || threads_changed) {
        return Err("That card is paused by the person: it stays in Paused, with its threads, until they resume it. Don't move it or message its thread.".into());
    }
    // A flow's card is its run's: the engine starts and moves it as each
    // step reports (its outcome may still be added to).
    if let Some(card) = current.filter(|c| c.flow.is_some())
        && (state.is_some_and(|s| s != card.state) || threads_changed)
    {
        return Err("That card is a step of its flow's run: the engine moves it as each step reports. Leave its column and threads; add to its outcome if you need to.".into());
    }
    let Some(state) = state else { return Ok(()) };
    let already = current.is_some_and(|c| c.state == WorkState::Blocked);
    if state == WorkState::Blocked && !already && !open_ask && !ask_given {
        let on = current.map(|c| format!(" (item {})", c.id)).unwrap_or_default();
        return Err(format!(
            "A Blocked card waits on the person, so it needs their question in Needs you: pass `ask` ({{question, options, recommendation}}) with this move, or raise one first with `ask`{on}. If nothing needs the person, it isn't Blocked: keep it in Now or Up next."
        ));
    }
    Ok(())
}

/// The open asks the board check raised on `card` whose condition has
/// cleared: the card left Blocked, or one of its `threads` works again.
/// `(ask id, why)`. Asks an agent or the person raised are theirs. Pure.
pub(crate) fn resolved_asks(card: &WorkItem, threads: &[(String, Thread)], asks: &[Ask]) -> Vec<(String, String)> {
    let why = if card.state != WorkState::Blocked {
        "the card left Blocked"
    } else if threads.iter().any(|(_, t)| t.live()) {
        "its thread is working again"
    } else {
        return Vec::new();
    };
    asks.iter()
        .filter(|a| a.state == AskState::Open && a.origin == AskOrigin::BoardCheck)
        .filter(|a| a.item_id.as_deref() == Some(card.id.as_str()))
        .map(|a| (a.id.clone(), why.to_owned()))
        .collect()
}

/// The question an engine-raised ask puts for `card`.
pub(crate) fn raised_ask(card: &WorkItem, raise: Raise) -> ItemAsk {
    let title = one_line(&card.title, 80);
    match raise {
        Raise::Errored => ItemAsk {
            question: format!("\"{title}\" stopped on an error. What now?"),
            options: vec![
                AskOption { detail: "Run its thread again".into(), ..AskOption::new(RETRY) },
                AskOption { detail: "Take it off the plan".into(), ..AskOption::new(DROP) },
            ],
            ..Default::default()
        },
        Raise::Blocked => {
            let reason = card
                .outcome
                .as_deref()
                .or(Some(card.detail.as_str()))
                .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
                .map(|l| one_line(l, 200));
            ItemAsk {
                question: format!("\"{title}\" is blocked. What does it need from you?"),
                context: reason.map(|r| format!("Last word on it: {r}")),
                ..Default::default()
            }
        }
    }
}

/// The message its orchestrator gets for flagged cards.
pub(crate) fn nudge_text(cards: &[(&str, &str, &str)]) -> String {
    let mut out = String::from(
        "[Board check] Some of your cards' columns aren't true. Move each (`item_upsert`), or say in its outcome why it stays:",
    );
    for (id, title, note) in cards {
        out.push_str(&format!("\n- \"{}\" ({}): {}", one_line(title, 80), one_line(id, 40), one_line(note, 140)));
    }
    out
}

/// Runs the board check a moment after a change.
#[derive(Clone, Default)]
pub(crate) struct Poke(Arc<tokio::sync::Notify>);

impl Poke {
    pub(crate) fn poke(&self) {
        self.0.notify_one();
    }
}

/// The check: once a moment after a start, then after every change and
/// every [`EVERY`].
pub(crate) fn start(orchestration: Orchestration) {
    let poke = orchestration.board_poke();
    // Session changes (here and synced) that matter to a card, and board
    // or ask changes, poke it.
    let watch = orchestration.clone();
    tokio::spawn(async move {
        let mut state_rx = watch.subscribe();
        let mut rows_rx = watch.workspace().watch_session_rows();
        let mut sessions_rx = loop {
            match watch.rpc() {
                Some(rpc) => break rpc.sessions_ref().watch_sessions(),
                None => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
            }
        };
        let (mut seen_rows, mut seen_here): (HashMap<String, Sig>, HashMap<String, Sig>) = Default::default();
        loop {
            let poke = tokio::select! {
                c = state_rx.changed() => match c { Ok(()) => true, Err(_) => break },
                c = rows_rx.changed() => match c {
                    Ok(()) => changed_sigs(&mut seen_rows, &rows_rx.borrow_and_update()),
                    Err(_) => break,
                },
                c = sessions_rx.changed() => match c {
                    Ok(()) => changed_sigs(&mut seen_here, &sessions_rx.borrow_and_update()),
                    Err(_) => break,
                },
            };
            if poke {
                watch.board_poke().poke();
            }
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(AFTER_START).await;
        if let Some(rpc) = orchestration.rpc() {
            let (resumed, changed) = rpc.start_up_pass(false).await;
            tracing::info!(?resumed, cards = changed.len(), ?changed, "board: start-up pass");
        }
        let mut tick = tokio::time::interval(EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = poke.0.notified() => tokio::time::sleep(SETTLE).await,
            }
            if let Some(rpc) = orchestration.rpc() {
                let perf_t = std::time::Instant::now();
                let changed = rpc.check_boards(Utc::now()).await;
                if !changed.is_empty() {
                    tracing::info!(?changed, "board: cards set straight");
                }
                tracing::debug!(target: "perf", what = "board_check", us = perf_t.elapsed().as_micros() as u64);
            }
        }
    });
}

/// What of a session a card's check reads.
type Sig = (SessionStatus, Option<String>, Option<(DoneStatus, DateTime<Utc>)>, bool, Option<DateTime<Utc>>);

fn sig(s: &Session) -> Sig {
    (
        s.status,
        s.last_completed_turn.clone(),
        s.last_turn_outcome.map(|o| (o.status, o.finished_at)),
        s.waiting.is_some() || s.running_subagents > 0,
        s.limited.as_ref().map(|l| l.since),
    )
}

/// Whether any of `sessions` changed in a way a card's check reads.
fn changed_sigs(seen: &mut HashMap<String, Sig>, sessions: &[Session]) -> bool {
    let mut changed = false;
    for s in sessions {
        let now = sig(s);
        if seen.get(&s.chat_id) != Some(&now) {
            seen.insert(s.chat_id.clone(), now);
            changed = true;
        }
    }
    changed
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

/// One card's check result, for the start-up report and the dry run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Change {
    pub item: String,
    pub title: String,
    pub orchestrator: Option<String>,
    pub from: WorkState,
    pub verdict: Verdict,
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = &self.verdict;
        write!(f, "{} \"{}\": {:?}", self.item, one_line(&self.title, 60), self.from)?;
        if let Some(to) = v.state {
            write!(f, " -> {to:?}")?;
        }
        match v.paused {
            Some(true) => write!(f, " (paused)")?,
            Some(false) => write!(f, " (unpaused)")?,
            None => {}
        }
        match &v.check {
            Some(Some(c)) => write!(f, "; {:?}: {}", c.kind, c.note)?,
            Some(None) => write!(f, "; check cleared")?,
            None => {}
        }
        if let Some(r) = v.raise {
            write!(f, "; ask raised ({r:?})")?;
        }
        if !v.drop_threads.is_empty() {
            write!(f, "; archived threads off it: {}", v.drop_threads.join(", "))?;
        }
        if v.nudge {
            write!(f, "; orchestrator told")?;
        }
        for (id, why) in &v.resolve {
            write!(f, "; ask {id} resolved: {why}")?;
        }
        Ok(())
    }
}

/// What says a thread is at work besides its session row: the chats this
/// engine runs itself (its own runs are the truth for them), and each
/// chat's progress row (what `progress` reads, synced from its host).
#[derive(Debug, Clone, Default)]
pub(crate) struct Live {
    pub local: std::collections::HashSet<String>,
    pub progress: HashMap<String, zeron_proto::orchestration::Progress>,
}

/// What the check would change on the boards in `state`, card by card,
/// reading each thread's `chats` row and `sessions` status. Only the cards
/// `keeper` keeps (hosted on that device: its orchestrator, else its first
/// thread); `None`: every card (a dry run over copied data).
pub(crate) fn plan_from(
    state: &zeron_proto::orchestration::OrchestrationState,
    chats: &HashMap<String, Chat>,
    sessions: &HashMap<String, Session>,
    live: &Live,
    known_prs: &[zeron_proto::ChangeRequestSummary],
    keeper: Option<&str>,
    now: DateTime<Utc>,
) -> Vec<Change> {
    // Threads on more than one card (Done ones too: a crew thread moves on
    // to its next card): at work on one of them, which one isn't known, so
    // they move or flag none but a card in Now.
    let mut uses: HashMap<&str, usize> = HashMap::new();
    for card in state.items.iter().filter(|i| !i.removed && i.state != WorkState::Dropped) {
        for t in &card.threads {
            *uses.entry(t.as_str()).or_default() += 1;
        }
    }
    let mut out = Vec::new();
    for card in state.items.iter().filter(|i| !i.removed) {
        if let Some(here) = keeper {
            let host = card
                .orchestrator_id
                .as_deref()
                .or(card.threads.first().map(String::as_str))
                .and_then(|c| chats.get(c))
                .map(|c| c.device_id.as_str());
            if host != Some(here) {
                continue;
            }
        }
        // A Now card's threads; with none left, the chats it links (a thread
        // the crew archive unlinked, messaged back to work on it).
        let linked: Vec<String> = if card.state == WorkState::Active && card.threads.is_empty() {
            card.links.iter().filter(|l| l.kind == LinkKind::Chat).map(|l| l.target.clone()).collect()
        } else {
            Vec::new()
        };
        let mut threads: Vec<(String, Thread)> = card
            .threads
            .iter()
            .map(|t| {
                let mut thread =
                    Thread::of(chats.get(t), sessions.get(t), live.progress.get(t), live.local.contains(t), now);
                if thread.live() && card.state != WorkState::Active && uses.get(t.as_str()).is_some_and(|n| *n > 1) {
                    thread = Thread::Finished;
                }
                (t.clone(), thread)
            })
            .collect();
        for t in &linked {
            let thread = Thread::of(chats.get(t), sessions.get(t), live.progress.get(t), live.local.contains(t), now);
            if thread.live() {
                threads.push((t.clone(), thread));
            }
        }
        let asks: Vec<&Ask> = state
            .asks
            .iter()
            .filter(|a| {
                a.item_id.as_deref() == Some(card.id.as_str())
                    || (a.item_id.is_none() && card.threads.iter().any(|t| *t == a.from_chat))
            })
            .collect();
        let open_ask = asks.iter().any(|a| a.state == AskState::Open);
        let last_closed = asks
            .iter()
            .filter(|a| a.state != AskState::Open)
            .filter_map(|a| a.answered_at.map(|at| (at, a.state == AskState::Answered)))
            .max_by_key(|(at, _)| *at);
        let route = card.source.as_ref().and_then(|s| {
            state.dumps.iter().find(|d| d.id == s.dump_id).and_then(|d| d.routes.iter().find(|r| r.id == s.route_id))
        });
        let view = CardView {
            card,
            threads: threads.clone(),
            open_ask,
            last_closed,
            routed: card.source.is_some(),
            route_review: route.is_some_and(|r| r.review.is_some()),
            prs: prs_of(card, known_prs),
            active_at: card
                .threads
                .iter()
                .chain(linked.iter())
                .filter_map(|t| Thread::active_at(sessions.get(t), live.progress.get(t)))
                .max(),
        };
        let mut verdict = assess(&view, now);
        verdict.resolve = resolved_asks(card, &view.threads, &state.asks);
        if !verdict.is_empty() {
            out.push(Change {
                item: card.id.clone(),
                title: card.title.clone(),
                orchestrator: card.orchestrator_id.clone(),
                from: card.state,
                verdict,
            });
        }
    }
    out
}

impl EngineRpc {
    /// What the check would change on the boards this engine keeps (`all`:
    /// every board), without changing it.
    pub(crate) fn plan_board_checks(&self, now: DateTime<Utc>, all: bool) -> Vec<Change> {
        let orch = self.orchestration();
        let state = orch.state();
        let chats: HashMap<String, Chat> =
            orch.workspace().read_chats().unwrap_or_default().into_iter().map(|c| (c.id.clone(), c)).collect();
        let mut sessions: HashMap<String, Session> = orch
            .workspace()
            .watch_session_rows()
            .borrow()
            .iter()
            .map(|s| (s.chat_id.clone(), s.clone()))
            .collect();
        let mut live = Live::default();
        for s in self.sessions_ref().watch_sessions().borrow().iter() {
            live.local.insert(s.chat_id.clone());
            sessions.insert(s.chat_id.clone(), s.clone());
        }
        live.progress = orch
            .workspace()
            .read_orchestration_with(|doc| doc.read_progress())
            .into_iter()
            .map(|p| (p.chat_id.clone(), p))
            .collect();
        plan_from(&state, &chats, &sessions, &live, &self.known_change_requests(), (!all).then(|| orch.device_id()), now)
    }

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

    /// The cards the board check paused itself (an engine shutdown or
    /// recovery read as a Stop, before only the person could pause): back in
    /// Now, a note saying so, and their threads' held messages sent. Run
    /// once at start; logged. The ones un-paused.
    pub(crate) async fn unpause_engine_pauses(&self, now: DateTime<Utc>) -> Vec<String> {
        let orch = self.orchestration();
        let cards: Vec<WorkItem> = orch.state().items.into_iter().filter(|i| !i.removed && engine_paused(i)).collect();
        let mut done = Vec::new();
        for card in cards {
            if let Err(error) =
                orch.patch_item(WorkItemPatch { id: Some(card.id.clone()), state: Some(WorkState::Active), ..Default::default() })
            {
                tracing::warn!(%error, card = %card.id, "board: an engine pause wasn't undone");
                continue;
            }
            self.mark_moved(&card.id, now);
            orch.workspace().write_orchestration(|doc| {
                if let Some(mut c) = doc.work_item(&card.id) {
                    c.paused_at = None;
                    c.check = Some(note_of(CheckKind::Moved, "Back in Now: the engine had paused it, not you", now));
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
    /// ([`EngineRpc::resume_after_restart`]), then the board check over the
    /// boards this engine keeps (`all`: every board). Every chat that was at
    /// work at the stop (resumed here, or revived by the engine's own
    /// recovery of a turn cut off by a crash) has its cards left alone for
    /// [`RESUMED_GRACE_SECS`]: the stop or the crash marked its turn
    /// interrupted or errored, which isn't its work failing. `(resumed,
    /// changed)`.
    pub(crate) async fn start_up_pass(&self, all: bool) -> (Vec<String>, Vec<Change>) {
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
        let changed = self.check_boards_on(Utc::now(), all).await;
        (resumed, changed)
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
            let paused = state.items.iter().any(|i| !i.removed && i.paused() && i.threads.iter().any(|t| *t == chat));
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

    /// The board check, applied. The cards it changed.
    pub(crate) async fn check_boards(&self, now: DateTime<Utc>) -> Vec<Change> {
        // (A send the stop cut off is found when its chat's commands are next
        // drained: its chat is resumed then.)
        self.resume_cut_off_sends();
        self.check_boards_on(now, false).await
    }

    /// [`EngineRpc::check_boards`] over the boards this engine keeps (`all`:
    /// every board).
    pub(crate) async fn check_boards_on(&self, now: DateTime<Utc>, all: bool) -> Vec<Change> {
        // Never while the engine stops: its own shutdown interrupts every turn,
        // which is nobody's word on any card.
        if self.sessions_ref().shutting_down() {
            return Vec::new();
        }
        let mut changes = self.plan_board_checks(now, all);
        // Cards of threads at work when the engine stopped are left alone
        // until their new turn has started: the stop (or a crash) marked
        // their last turn interrupted or errored, which isn't the person
        // stopping them, nor their work failing.
        let resumed = self.orchestration().resumed_since(now, TimeDelta::seconds(RESUMED_GRACE_SECS));
        if !resumed.is_empty() {
            let state = self.orchestration().state();
            changes.retain(|c| {
                !state.items.iter().any(|i| i.id == c.item && i.threads.iter().any(|t| resumed.contains(t)))
            });
        }
        self.apply_changes(&changes, now);
        changes
    }

    /// `changes` (from [`plan_from`]) applied: each card set straight, asks
    /// raised, and each orchestrator told once about its flagged cards.
    pub(crate) fn apply_changes(&self, changes: &[Change], now: DateTime<Utc>) {
        let mut nudges: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
        for change in changes {
            if let Some(to) = self.apply_verdict(&change.item, &change.verdict, now) {
                if change.verdict.nudge
                    && let Some(o) = change.orchestrator.clone()
                {
                    nudges.entry(o).or_default().push((change.item.clone(), change.title.clone(), to));
                }
            }
        }
        // Paused cards' threads hosted here keep their queues held (a doc
        // reopened since lost the hold).
        for card in self.orchestration().state().items.iter().filter(|i| !i.removed && i.paused()) {
            for t in &card.threads {
                self.doc_host_ref().hold_queue(t);
            }
        }
        for (orchestrator, cards) in nudges {
            let row = self.orchestration().state().orchestrators.into_iter().find(|o| o.id == orchestrator);
            if row.is_none_or(|o| o.archived || o.paused) {
                continue;
            }
            let list: Vec<(&str, &str, &str)> = cards.iter().map(|(i, t, n)| (i.as_str(), t.as_str(), n.as_str())).collect();
            let injected = zeron_doc::Injected::board_check(list.iter().map(|(id, _, _)| *id));
            if let Err(error) =
                super::ops::deliver(self, &orchestrator, &nudge_text(&list), None, super::ops::When::Next, Some(injected))
            {
                tracing::warn!(%error, %orchestrator, "board: couldn't tell the orchestrator about its cards");
            }
        }
    }

    /// `verdict` on card `id`. The check's note when it set one (else "").
    fn apply_verdict(&self, id: &str, verdict: &Verdict, now: DateTime<Utc>) -> Option<String> {
        let orch = self.orchestration();
        let card = orch.item(id)?;
        if let Some(state) = verdict.state {
            if let Err(error) = orch.patch_item(WorkItemPatch { id: Some(id.to_owned()), state: Some(state), ..Default::default() }) {
                tracing::warn!(%error, "board: a card wasn't moved");
                return None;
            }
            self.mark_moved(id, now);
        }
        let note = orch.workspace().write_orchestration(|doc| {
            let mut c = doc.work_item(id)?;
            match verdict.paused {
                Some(true) => c.paused_at = Some(now),
                Some(false) => c.paused_at = None,
                None => {}
            }
            if let Some(check) = &verdict.check {
                c.check = check.clone().map(|mut k| {
                    if verdict.nudge {
                        k.nudged_at = Some(now);
                    }
                    k
                });
            }
            for t in &verdict.drop_threads {
                c.threads.retain(|x| x != t);
                if !c.links.iter().any(|l| l.kind == LinkKind::Chat && l.target == *t) {
                    c.links.push(zeron_proto::orchestration::WorkLink {
                        kind: LinkKind::Chat,
                        label: "Archived thread".into(),
                        target: t.clone(),
                    });
                }
            }
            c.updated_at = now;
            doc.upsert_work_item(&c);
            Some(c.check.map(|k| k.note).unwrap_or_default())
        })?;
        if verdict.paused == Some(true) {
            for t in &card.threads {
                self.doc_host_ref().hold_queue(t);
            }
        }
        for (ask, why) in &verdict.resolve {
            if super::turn_asks::close(orch, ask, AnswerVia::Resolved, &format!("resolved: {why}"), now).is_some() {
                tracing::info!(%ask, card = %id, %why, "board: an ask the check raised closed itself");
            }
        }
        if let Some(raise) = verdict.raise {
            let from = card.orchestrator_id.clone().or_else(|| card.threads.first().cloned());
            if let Some(from) = from {
                let ask = raised_ask(&card, raise);
                self.raise_item_ask(&from, &card, ask, AskOrigin::BoardCheck, now);
            }
        }
        Some(note)
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
                && (a.item_id.as_deref() == Some(id) || (a.item_id.is_none() && threads.iter().any(|t| *t == a.from_chat)))
        })
    }

    /// The person pauses card `id`: Blocked in Paused, its threads stopped
    /// (the stop button's command) and their queues held.
    pub(crate) fn pause_item(&self, id: &str, now: DateTime<Utc>) -> Result<Value, RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).filter(|c| !c.removed).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        if card.paused() {
            return Ok(json!({ "id": id }));
        }
        orch.patch_item(WorkItemPatch { id: Some(id.to_owned()), state: Some(WorkState::Blocked), ..Default::default() })
            .map_err(RpcError::Failed)?;
        self.mark_moved(id, now);
        orch.workspace().write_orchestration(|doc| {
            if let Some(mut c) = doc.work_item(id) {
                c.paused_at = Some(now);
                c.check = None;
                c.waiting = None;
                doc.upsert_work_item(&c);
            }
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
        Ok(json!({ "id": id }))
    }

    /// The person resumes paused card `id` into `to` (Now when absent): its
    /// threads' held messages sent, else each told to carry on; with no
    /// thread, Up next (or, into Now, started).
    pub(crate) async fn resume_item(&self, id: &str, to: Option<WorkState>, now: DateTime<Utc>) -> Result<Value, RpcError> {
        let orch = self.orchestration();
        let card = orch.item(id).filter(|c| !c.removed).ok_or_else(|| RpcError::Failed(format!("No board item {id}")))?;
        let to = match to.unwrap_or(WorkState::Active) {
            WorkState::Blocked | WorkState::Other => WorkState::Active,
            s => s,
        };
        let live: Vec<String> = card
            .threads
            .iter()
            .filter(|t| self.workspace_host().chat(t).ok().flatten().is_some_and(|c| !c.archived))
            .cloned()
            .collect();
        // Nothing to resume into Now: back in Up next, then started afresh.
        let set = if live.is_empty() && to == WorkState::Active { WorkState::Planned } else { to };
        orch.patch_item(WorkItemPatch { id: Some(id.to_owned()), state: Some(set), ..Default::default() })
            .map_err(RpcError::Failed)?;
        self.mark_moved(id, now);
        orch.workspace().write_orchestration(|doc| {
            if let Some(mut c) = doc.work_item(id) {
                c.paused_at = None;
                c.check = None;
                doc.upsert_work_item(&c);
            }
        });
        orch.comms().say(CommKind::Message, YOU, card.orchestrator_id.as_deref().unwrap_or(YOU), &format!("Resumed: {}", one_line(&card.title, 80)));
        tracing::info!(card = %id, title = %card.title, to = ?to, threads = ?card.threads, "board: the person resumed a paused card");
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
        // Into a waiting column: nothing else runs.
        if to != WorkState::Active {
            return Ok(json!({ "id": id }));
        }
        if live.is_empty() {
            // Nothing to resume: started afresh (or queued for its crew).
            return self.start_item(id, true);
        }
        // Into Now: each thread carries on (its held messages, else told to).
        for t in &live {
            if !released.contains(t) {
                let text = format!("[Resumed] The person resumed \"{}\": carry on where you stopped.", one_line(&card.title, 80));
                super::ops::deliver(self, t, &text, None, super::ops::When::Next, Some(zeron_doc::Injected::resumed(&card.id)))?;
            }
        }
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
