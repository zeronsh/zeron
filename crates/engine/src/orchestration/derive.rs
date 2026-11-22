//! Where a card is on the board, worked out from facts. Pure.
//!
//! Nobody sets a card's column. [`derive`] reads what is known of it (its
//! threads as the engine runs them, the asks on it, its finish line, what
//! the person did) and walks the ladder, first yes wins:
//!
//! 1. **It crossed its finish line: Done.** Closed already (a closing is
//!    recorded once, [`Closed`]), or crossing it now: its thread's done
//!    report stands (no more work sent since, nothing asked, its
//!    orchestrator's turn on it over), its branch landed, its PR merged or
//!    closed. Accept is only the person's.
//! 2. **An open ask to the person, not put off: Needs you.** An agent's
//!    ask, or the engine's own: its thread stopped on an error (Retry /
//!    Drop), a result or PR to look at, nothing moving. A thread waiting
//!    for the person's answer in its own chat counts too.
//! 3. **An agent is provably on it: Running.** One of: a thread of it is
//!    working or held on its own background work or subagents, or stopped
//!    by a usage limit it will be resumed from; work sent to one of its
//!    threads is on its way; a thread is being resumed after a restart; its
//!    orchestrator has a turn on it (its report, the person's answer, a
//!    card made inside the turn); a thread's turn ended a moment ago (what
//!    it ended on is on its way). Every one of these ends by itself: none
//!    can hold a card in Running for good.
//! 4. **Its asks are all put off: Snoozed**, until the first comes back.
//! 5. **Paused by the person, waiting on another card, held, or never
//!    started: Planned.** A held card (kept for later, or left alone after
//!    the engine's question about it was dismissed) is Running only while a
//!    thread of it works, and never stalls: it waits until it is started,
//!    resumed or let go.
//! 6. **None of the above (started, nobody on it, nobody asked): stalled.**
//!    The engine wakes its orchestrator once with the three real choices
//!    (continue, ask, close); still stalled [`STALL_ASK_SECS`] after that
//!    turn ends, the engine asks the person itself ("Nothing is moving on
//!    this": Continue / Close / Drop), which is rung 2. While the engine's
//!    own check runs the card reads "Orchestrator checking" in Running: the
//!    check is on it, and it has a deadline.
//!
//! [`derive`] also says what the engine must do for the place to be true
//! ([`Act`]): record a closing, raise or resolve one of its own asks, wake
//! the orchestrator. `board.rs` gathers the facts and applies the result.

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{
    Ask, AskOrigin, AskState, BoardCard, CardStatus, CheckWhy, Checking, CloseHow, FinishKind, LinkKind, Mover, Place,
    ReportState, RouteState, WorkItem, prompt_text::one_line,
};

use super::board_checks::{Pr, Thread};

/// How long after a thread's turn ended its card is left where it is: what
/// the turn ended on (a report, a question to the person, its next turn, a
/// usage limit's mark) is on its way.
pub(crate) const HANDOFF_SECS: i64 = 30;
/// How long after the last turn on a card ended (its thread's, or its
/// orchestrator's) the engine waits before it calls the card stalled and
/// wakes its orchestrator: a turn that ends is often a moment from the
/// next (a wait it didn't declare, a reply on its way).
pub(crate) const STALL_SETTLE_SECS: i64 = 90;
/// How long work sent to a thread counts as on its way before the thread
/// shows it (its turn starting, a step).
pub(crate) const IN_FLIGHT_SECS: i64 = 120;
/// How long an orchestrator woken for a card has to start that turn.
pub(crate) const CHECK_HANDOFF_SECS: i64 = 90;
/// How long after its orchestrator's turn on a stalled card ended (or, with
/// no orchestrator to wake, after the stall was found) the person is asked.
pub(crate) const STALL_ASK_SECS: i64 = 120;

/// One of a card's threads, as the ladder reads it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThreadFact {
    pub id: String,
    pub standing: Thread,
    /// Its turn ended into a wait on its own background work or subagents:
    /// what on.
    pub waiting: Option<String>,
    /// Waiting for the person's answer in its own chat.
    pub awaiting_input: bool,
    /// Stopped by its usage limit and waiting for the reset (when known).
    pub limited: Option<Option<DateTime<Utc>>>,
    /// When it last did anything.
    pub active_at: Option<DateTime<Utc>>,
    /// When its last turn ended.
    pub ended_at: Option<DateTime<Utc>>,
    /// It was last given work for another open card: its being at work says
    /// nothing of this one.
    pub elsewhere: bool,
    /// At work when the engine stopped, its resume on its way.
    pub resuming: bool,
}

impl ThreadFact {
    fn live(&self) -> bool {
        matches!(self.standing, Thread::Live { .. }) && !self.elsewhere
    }

    fn gone(&self) -> bool {
        self.standing == Thread::Gone
    }
}

/// A card's orchestrator, as the ladder reads it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Owner {
    /// It can be woken for the card: a standard orchestrator, not paused or
    /// archived, its chat there.
    pub wakeable: bool,
    /// In a turn now.
    pub live: bool,
    /// When its latest turn started, and when its last one ended.
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// The Brain dump route a card came from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RouteFact {
    pub state: RouteState,
    /// What its route left the person to look at.
    pub review: Option<String>,
    /// The route's target does the card's work itself (its thread, or its
    /// orchestrator with no thread on the card): the route's end is the
    /// card's report. Not once other threads were put on it.
    pub ends_it: bool,
}

/// Everything [`derive`] reads of one card.
#[derive(Debug, Clone)]
pub(crate) struct Facts<'a> {
    pub item: &'a WorkItem,
    /// Its row: only the facts in it are read (the finish line, how it
    /// closed, its thread's last word…), never the place it gave before.
    pub card: &'a BoardCard,
    pub threads: Vec<ThreadFact>,
    /// Every ask about it: on it, or from one of its threads on no card.
    pub asks: Vec<&'a Ask>,
    pub owner: Option<Owner>,
    /// The cards it waits on that aren't closed (their titles).
    pub waits_on: Vec<String>,
    pub route: Option<RouteFact>,
    /// Its PR links, as last looked up.
    pub prs: Vec<Pr>,
    /// Its branch has landed in its target: the commit, short.
    pub merged: Option<String>,
    /// The ask the engine keeps for its orchestrator's open PRs.
    pub pr_ask: Option<&'a Ask>,
}

/// An ask the engine raises for a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Raise {
    /// Its thread stopped on an error: Retry / Drop.
    Failed,
    /// Nothing is moving on it: Continue / Close / Drop.
    Stalled,
    /// A result to accept (its line, when its route left one): Looks good,
    /// or what to change.
    Review(Option<String>),
}

/// What the engine does for the place to be true.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Act {
    /// The closing no longer stands (more work was sent): cleared.
    Reopen,
    /// It crossed its finish line: recorded.
    Close { how: CloseHow, note: Option<String> },
    /// Its orchestrator's turn on it is over (or never came).
    ClearChecking,
    Raise(Raise),
    /// Its PR is open and its thread finished: it joins its orchestrator's
    /// PR ask.
    JoinPrAsk,
    /// An ask of the engine's own whose condition cleared: closed
    /// "resolved: why".
    Resolve { ask: String, why: String },
    /// Nothing is moving on it: the check begins (its orchestrator woken,
    /// when it has one to wake).
    StallBegin { wake: bool },
    /// The wake turn ended (or was given up on).
    StallSettled { at: DateTime<Utc> },
    /// Something moves on it again, or the person was asked and answered.
    ClearStall,
    /// Its thread works again, started after the pause: no longer paused.
    Unpause,
    /// The engine asked about it and nothing came of the answer: left alone
    /// until work is sent to it again.
    Hold,
    /// Threads archived or gone: off the card, kept as chat links.
    DropThreads(Vec<String>),
}

/// What decided the place (the move's reason is made from it).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Why {
    Closed { how: CloseHow, by: Mover, note: Option<String> },
    /// An ask is open: its question.
    Asked(String),
    /// Its thread waits for the person in its own chat.
    AwaitingInput,
    Failed,
    StallAsked,
    Review,
    ThreadLive,
    WorkSent,
    Resuming,
    OwnerChecking(CheckWhy),
    /// A thread's turn just ended, or the engine's own check runs.
    Settling,
    Snoozed,
    Paused,
    After(String),
    Held,
    NotStarted,
}

/// A card's place and what goes with it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Derived {
    pub place: Place,
    pub status: CardStatus,
    pub note: Option<String>,
    pub until: Option<DateTime<Utc>>,
    /// Its open asks, the one to answer first first.
    pub asks: Vec<String>,
    pub why: Why,
    pub acts: Vec<Act>,
    /// When to work it out again though nothing changed (a timed fact runs
    /// out).
    pub recheck: Option<DateTime<Utc>>,
}

impl Derived {
    fn at(place: Place, status: CardStatus, why: Why) -> Self {
        Derived { place, status, note: None, until: None, asks: Vec::new(), why, acts: Vec::new(), recheck: None }
    }
}

/// The earlier of two optional times.
fn sooner(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Whether `checking` still holds for `owner` at `now`: `Ok(until)` while it
/// does (`until`: when to look again, for a turn still to come), `Err(())`
/// once its turn is over or never came.
fn checking_holds(checking: &Checking, owner: Option<&Owner>, now: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, ()> {
    let Some(owner) = owner.filter(|o| o.wakeable) else { return Err(()) };
    if owner.live {
        return Ok(None);
    }
    // The turn that was on it has ended: one started after the check began;
    // or, for a card made inside a turn, that turn itself.
    let ran_since = owner.started_at.is_some_and(|s| s > checking.since);
    let ended_since = owner.ended_at.is_some_and(|e| e > checking.since);
    if ran_since || (checking.why == CheckWhy::Created && ended_since) {
        return Err(());
    }
    // A turn still to come (a wake waits out its debounce, a message is on
    // its way): it has this long to start.
    let from = owner.ended_at.map_or(checking.since, |e| e.max(checking.since));
    let until = from + TimeDelta::seconds(CHECK_HANDOFF_SECS);
    if now < until { Ok(Some(until)) } else { Err(()) }
}

/// Where `f`'s card is at `now`, and what the engine does about it. Pure.
pub(crate) fn derive(f: &Facts, now: DateTime<Utc>) -> Derived {
    let (item, card) = (f.item, f.card);
    let mut acts: Vec<Act> = Vec::new();
    let mut recheck: Option<DateTime<Utc>> = None;

    // ── Facts, read once ────────────────────────────────────────────────────
    let here: Vec<&ThreadFact> = f.threads.iter().filter(|t| !t.gone()).collect();
    let live: Vec<&ThreadFact> = here.iter().copied().filter(|t| t.live()).collect();
    let last_active = f.threads.iter().filter_map(|t| t.active_at).max();
    // Work sent to a thread that hasn't shown it yet (by its session, a
    // report, an outcome of its route).
    let shown = [last_active, card.heard_at].into_iter().flatten().max();
    let in_flight = card.work_at.is_some_and(|sent| {
        shown.is_none_or(|a| a < sent) && now - sent < TimeDelta::seconds(IN_FLIGHT_SECS) && !item.paused()
    });
    if in_flight {
        recheck = sooner(recheck, card.work_at.map(|s| s + TimeDelta::seconds(IN_FLIGHT_SECS)));
    }
    let resuming = here.iter().any(|t| t.resuming && !t.elsewhere);
    let working = !live.is_empty() || in_flight || resuming;
    // A thread's turn ended a moment ago.
    let just_ended = here
        .iter()
        .filter(|t| !t.elsewhere)
        .filter_map(|t| t.ended_at)
        .max()
        .map(|e| e + TimeDelta::seconds(HANDOFF_SECS))
        .filter(|until| now < *until);
    // Its orchestrator's turn on it.
    let mut checking = card.checking.as_ref();
    let mut checking_until = None;
    if let Some(c) = checking {
        match checking_holds(c, f.owner.as_ref(), now) {
            Ok(until) => checking_until = until,
            Err(()) => {
                acts.push(Act::ClearChecking);
                checking = None;
            }
        }
    }
    recheck = sooner(recheck, checking_until);

    // ── 0. A closing that no longer stands ──────────────────────────────────
    // More work was sent to its thread after an agent's word closed it. (A
    // closing the engine checked itself, or the person's own, stands.)
    let mut closed = card.closed.as_ref();
    if let Some(c) = closed
        && c.by != Mover::You
        && matches!(c.how, CloseHow::Report | CloseHow::Closed | CloseHow::Mapped | CloseHow::Other)
        && card.work_at.is_some_and(|sent| sent > c.at)
    {
        acts.push(Act::Reopen);
        closed = None;
    }

    // ── Asks ────────────────────────────────────────────────────────────────
    let mine = |a: &Ask| a.item_id.as_deref() == Some(item.id.as_str());
    let mut open: Vec<&Ask> = f.asks.iter().copied().filter(|a| a.state == AskState::Open).collect();
    // The engine's own asks whose condition cleared close themselves.
    let resolve = |open: &mut Vec<&Ask>, acts: &mut Vec<Act>, origin: AskOrigin, why: &str| {
        open.retain(|a| {
            let gone = a.origin == origin && mine(a);
            if gone {
                acts.push(Act::Resolve { ask: a.id.clone(), why: why.to_owned() });
            }
            !gone
        });
    };

    // ── 1. Its finish line ──────────────────────────────────────────────────
    if let Some(c) = closed {
        for origin in [AskOrigin::BoardCheck, AskOrigin::Stall, AskOrigin::Review] {
            resolve(&mut open, &mut acts, origin, if c.dropped() { "the card was dropped" } else { "the card is done" });
        }
        if card.stall.is_some() {
            acts.push(Act::ClearStall);
        }
        if card.checking.is_some() && !acts.contains(&Act::ClearChecking) {
            acts.push(Act::ClearChecking);
        }
        let (place, status) =
            if c.dropped() { (Place::Dropped, CardStatus::Dropped) } else { (Place::Done, CardStatus::Done) };
        return Derived {
            acts,
            ..Derived::at(place, status, Why::Closed { how: c.how, by: c.by, note: c.note.clone() })
        };
    }
    let reports = f.owner.is_some();
    // Its thread's word, standing: done, and no work sent since.
    let said_done =
        card.word.as_ref().is_some_and(|w| w.state == ReportState::Done && card.work_at.is_none_or(|sent| w.at >= sent));
    // A routed card whose route's target does its work: the route's end.
    // A thread that reports to nobody: its turn completed with nothing left.
    let route_done = f.route.as_ref().is_some_and(|r| r.ends_it && r.state == RouteState::Done);
    // (A turn that ended since the card was made: a chat put on the board
    // after its last turn hasn't done the card's work.)
    let turns_done = !here.is_empty()
        && here.iter().all(|t| {
            t.standing == Thread::Finished && !t.elsewhere && t.ended_at.or(t.active_at).is_some_and(|e| e >= item.created_at)
        })
        && just_ended.is_none();
    let finished = said_done || route_done || (!reports && f.route.is_none() && turns_done);
    // A route that ended (done or failed) says its target went to rest; its
    // session may show the turn a moment longer. Unless work was sent since.
    let route_rests = f.route.as_ref().is_some_and(|r| r.ends_it && matches!(r.state, RouteState::Done | RouteState::Failed))
        && card.work_at.is_none_or(|sent| card.heard_at.is_some_and(|heard| heard >= sent));
    let working = working && !route_rests;
    let just_ended = just_ended.filter(|_| !route_rests);
    let at_rest = !working && checking.is_none();
    let crossing = match card.finish.line {
        // (Checked by the engine itself, once nothing works on it: a branch
        // nobody has committed to yet is contained in its target too.)
        FinishKind::Merged => f.merged.as_ref().filter(|_| !working).map(|sha| (CloseHow::Merged, Some(sha.clone()))),
        FinishKind::Pr => {
            (!working && !f.prs.is_empty() && f.prs.iter().all(|p| p.open == Some(false))).then_some((CloseHow::Pr, None))
        }
        FinishKind::Accept => None,
        FinishKind::Report | FinishKind::Other => {
            let review = f.route.as_ref().is_some_and(|r| r.ends_it && r.review.is_some());
            // (A paused card is the person's: no agent's word closes it.)
            (finished && !review && at_rest && open.is_empty() && !item.paused())
                .then(|| (CloseHow::Report, card.word.as_ref().filter(|_| said_done).map(|w| w.summary.clone())))
        }
    };
    if let Some((how, note)) = crossing {
        for origin in [AskOrigin::BoardCheck, AskOrigin::Stall, AskOrigin::Review] {
            resolve(&mut open, &mut acts, origin, "the card is done");
        }
        if card.stall.is_some() {
            acts.push(Act::ClearStall);
        }
        if card.checking.is_some() && !acts.contains(&Act::ClearChecking) {
            acts.push(Act::ClearChecking);
        }
        acts.push(Act::Close { how, note: note.clone() });
        return Derived {
            acts,
            ..Derived::at(Place::Done, CardStatus::Done, Why::Closed { how, by: Mover::Engine, note })
        };
    }

    // ── 2. Needs the person ─────────────────────────────────────────────────
    if working {
        resolve(&mut open, &mut acts, AskOrigin::BoardCheck, "its thread is working again");
        resolve(&mut open, &mut acts, AskOrigin::Stall, "its thread is working again");
        resolve(&mut open, &mut acts, AskOrigin::Review, "its thread is working again");
    }
    let mut raised: Option<(CardStatus, Why)> = None;
    if !working && just_ended.is_none() && !card.held && !item.paused() {
        // What the card last did: an ask of the engine's raised since then
        // and closed was its one question about that (asked once: a
        // dismissal isn't asked again).
        let last_change = [card.work_at, shown].into_iter().flatten().max();
        let has = |origin: AskOrigin| {
            f.asks.iter().any(|a| {
                a.origin == origin
                    && mine(a)
                    && (a.state == AskState::Open || last_change.is_none_or(|then| a.created_at >= then))
            })
        };
        // Its thread stopped on an error (and nothing was sent to it since),
        // or the route whose target does its work failed.
        let errored = here.iter().any(|t| {
            !t.elsewhere
                && matches!(t.standing, Thread::Errored { at } if card.work_at.is_none_or(|sent| at.is_none_or(|a| a >= sent)))
        }) || f.route.as_ref().is_some_and(|r| r.ends_it && r.state == RouteState::Failed);
        if errored {
            if !has(AskOrigin::BoardCheck) {
                acts.push(Act::Raise(Raise::Failed));
                raised = Some((CardStatus::Failed, Why::Failed));
            }
        } else if checking.is_none() {
            // Something for the person to look at, once its work rests.
            let review_line = f.route.as_ref().filter(|r| r.ends_it).and_then(|r| r.review.clone());
            let to_accept = (card.finish.line == FinishKind::Accept && (finished || turns_done))
                || (route_done && review_line.is_some());
            // (Its PR open, or not looked up yet, and nothing working on it:
            // its thread done, stopped or gone.)
            let pr_open =
                card.finish.line == FinishKind::Pr && !f.prs.is_empty() && f.prs.iter().any(|p| p.open != Some(false));
            if to_accept && !has(AskOrigin::Review) {
                acts.push(Act::Raise(Raise::Review(review_line)));
                raised = Some((CardStatus::Review, Why::Review));
            } else if pr_open {
                acts.push(Act::JoinPrAsk);
                // Its orchestrator's PR ask stands for it: put off, the card
                // is too.
                match f.pr_ask.filter(|a| a.state == AskState::Open) {
                    Some(a) if !open.iter().any(|o| o.id == a.id) => open.push(a),
                    Some(_) => {}
                    None => raised = Some((CardStatus::Review, Why::Review)),
                }
            }
        }
    }
    let (mut awake, mut snoozed): (Vec<&Ask>, Vec<&Ask>) = open.iter().copied().partition(|a| !a.deferred(now));
    // The one to answer first: urgent, then the oldest.
    awake.sort_by(|a, b| b.urgent.cmp(&a.urgent).then(a.created_at.cmp(&b.created_at)).then(a.id.cmp(&b.id)));
    snoozed.sort_by(|a, b| a.deferred_until.cmp(&b.deferred_until).then(a.id.cmp(&b.id)));
    let ask_ids: Vec<String> = awake.iter().chain(snoozed.iter()).map(|a| a.id.clone()).collect();
    let status_of = |a: &Ask| match a.origin {
        AskOrigin::BoardCheck => CardStatus::Failed,
        AskOrigin::Stall => CardStatus::Stalled,
        AskOrigin::Review => CardStatus::Review,
        _ => CardStatus::Asked,
    };
    let why_of = |a: &Ask| match a.origin {
        AskOrigin::BoardCheck => Why::Failed,
        AskOrigin::Stall => Why::StallAsked,
        AskOrigin::Review => Why::Review,
        _ => Why::Asked(one_line(&a.question, 100)),
    };
    // (The first put-off ask to come back moves it.)
    let returns = snoozed.first().and_then(|a| a.deferred_until);
    if let Some(first) = awake.first() {
        // A stall the person is asked about is the ask's now.
        return Derived {
            asks: ask_ids,
            acts,
            recheck: sooner(recheck, returns),
            ..Derived::at(Place::NeedsYou, status_of(first), why_of(first))
        };
    }
    if let Some((status, why)) = raised {
        return Derived { asks: ask_ids, acts, recheck, ..Derived::at(Place::NeedsYou, status, why) };
    }
    if let Some(t) = here.iter().find(|t| t.awaiting_input && !t.elsewhere) {
        let _ = t;
        if card.stall.is_some() {
            acts.push(Act::ClearStall);
        }
        return Derived {
            asks: ask_ids,
            note: Some("Answer in its thread".into()),
            acts,
            recheck,
            ..Derived::at(Place::NeedsYou, CardStatus::Asked, Why::AwaitingInput)
        };
    }

    // ── 3. An agent is provably on it ───────────────────────────────────────
    let running = |status: CardStatus, why: Why, note: Option<String>, until: Option<DateTime<Utc>>, mut acts: Vec<Act>| {
        // Something moves on it: a stall is over. (Not by the engine's own
        // check: the wake turn, a turn that just ended.) A held card stays
        // held: its thread answering a question isn't the card started, and
        // it goes back to Planned when the thread rests.
        let moving = !matches!(why, Why::OwnerChecking(CheckWhy::Stall) | Why::Settling);
        if moving && card.stall.is_some() {
            acts.push(Act::ClearStall);
        }
        Derived { note, until, asks: ask_ids.clone(), acts, recheck: sooner(recheck, returns), ..Derived::at(Place::Running, status, why) }
    };
    if !live.is_empty() {
        // Paused, and its thread started again after: the person's doing
        // (an agent's message to it would have been held).
        if let Some(paused) = item.paused_at
            && live.iter().any(|t| matches!(t.standing, Thread::Live { started_at } if started_at.is_none_or(|s| s > paused)))
        {
            acts.push(Act::Unpause);
        }
        let paused = item.paused() && !acts.contains(&Act::Unpause);
        if !paused {
            let working_now = live.iter().find(|t| t.waiting.is_none() && t.limited.is_none());
            let waiting = live.iter().find_map(|t| t.waiting.clone().filter(|_| t.limited.is_none()));
            let limited = live.iter().find_map(|t| t.limited);
            return match (working_now, waiting, limited) {
                (Some(_), _, _) => running(CardStatus::Working, Why::ThreadLive, None, None, acts),
                (None, Some(on), _) => running(CardStatus::Waiting, Why::ThreadLive, Some(on).filter(|s| !s.is_empty()), None, acts),
                (None, None, Some(until)) => running(CardStatus::Limited, Why::ThreadLive, None, until, acts),
                (None, None, None) => running(CardStatus::Working, Why::ThreadLive, None, None, acts),
            };
        }
    }
    if in_flight {
        return running(CardStatus::Queued, Why::WorkSent, None, None, acts);
    }
    if resuming && !item.paused() {
        return running(CardStatus::Working, Why::Resuming, Some("Resuming".into()), None, acts);
    }
    if let Some(c) = checking {
        return running(CardStatus::Checking, Why::OwnerChecking(c.why), None, None, acts);
    }
    if let Some(until) = just_ended
        && !item.paused()
        && !card.held
    {
        let mut d = running(CardStatus::Checking, Why::Settling, None, None, acts);
        d.recheck = sooner(d.recheck, Some(until));
        return d;
    }

    // ── 4. Its asks are all put off ─────────────────────────────────────────
    if let Some(first) = snoozed.first() {
        return Derived {
            until: first.deferred_until,
            asks: ask_ids,
            acts,
            recheck: sooner(recheck, returns),
            ..Derived::at(Place::Snoozed, status_of(first), Why::Snoozed)
        };
    }

    // ── 5. Planned ──────────────────────────────────────────────────────────
    let planned = |status: CardStatus, why: Why, note: Option<String>, mut acts: Vec<Act>| {
        if card.stall.is_some() {
            acts.push(Act::ClearStall);
        }
        Derived { note, acts, recheck, ..Derived::at(Place::Planned, status, why) }
    };
    if item.paused() {
        return planned(CardStatus::Paused, Why::Paused, None, acts);
    }
    if let Some(first) = f.waits_on.first() {
        let note = match f.waits_on.len() - 1 {
            0 => one_line(first, 60),
            more => format!("{} and {more} more", one_line(first, 60)),
        };
        return planned(CardStatus::After, Why::After(first.clone()), Some(note), acts);
    }
    if card.held {
        return planned(CardStatus::Held, Why::Held, None, acts);
    }
    let gone: Vec<String> = f.threads.iter().filter(|t| t.gone()).map(|t| t.id.clone()).collect();
    // (A chat link is a thread the crew archive took off it.)
    let started = card.work_at.is_some()
        || !gone.is_empty()
        || here.iter().any(|t| t.standing != Thread::Idle && t.active_at.is_none_or(|a| a >= item.created_at))
        || f.route.as_ref().is_some_and(|r| r.ends_it)
        || item.links.iter().any(|l| l.kind == LinkKind::Chat);
    if !started {
        return planned(CardStatus::Next, Why::NotStarted, item.waiting.clone(), acts);
    }

    // ── 6. Stalled: started, nobody on it, nobody asked ─────────────────────
    // (Its archived threads come off it now: nothing is on its way to them.)
    if !gone.is_empty() {
        acts.push(Act::DropThreads(gone));
    }
    let wakeable = f.owner.as_ref().is_some_and(|o| o.wakeable);
    let checking_d = |acts: Vec<Act>, until: DateTime<Utc>| Derived {
        acts,
        recheck: sooner(recheck, Some(until)),
        ..Derived::at(Place::Running, CardStatus::Checking, Why::Settling)
    };
    let ask_now = |mut acts: Vec<Act>| {
        acts.push(Act::Raise(Raise::Stalled));
        Derived { acts, recheck, ..Derived::at(Place::NeedsYou, CardStatus::Stalled, Why::StallAsked) }
    };
    // The last turn on it: its thread's, or its orchestrator's.
    let last_end = here.iter().filter(|t| !t.elsewhere).filter_map(|t| t.ended_at).chain(f.owner.as_ref().and_then(|o| o.ended_at)).max();
    let settles = last_end.map(|e| e + TimeDelta::seconds(STALL_SETTLE_SECS)).filter(|until| now < *until);
    match &card.stall {
        // A turn on it ended a moment ago: what it ended on (a route's
        // outcome, a thread it started, the next turn) may be on its way.
        None if settles.is_some() => checking_d(acts, settles.unwrap_or(now)),
        None => {
            acts.push(Act::StallBegin { wake: wakeable });
            // Woken: its orchestrator's turn is on it. With none to wake,
            // the person is asked once the wait runs out.
            let until = now + TimeDelta::seconds(if wakeable { CHECK_HANDOFF_SECS } else { STALL_ASK_SECS });
            checking_d(acts, until)
        }
        // Asked already, and nothing came of the answer (or it was
        // dismissed): left alone, not asked again until something moves.
        Some(s) if s.ask.is_some() => {
            acts.push(Act::Hold);
            planned(CardStatus::Held, Why::Held, None, acts)
        }
        Some(s) => {
            // The wake turn is over (its `checking` cleared above), or there
            // was nobody to wake.
            let settled = match (s.settled_at, s.woken_at) {
                (Some(at), _) => at,
                (None, Some(woken)) => {
                    let at = f.owner.as_ref().and_then(|o| o.ended_at).filter(|e| *e > woken).unwrap_or(now);
                    acts.push(Act::StallSettled { at });
                    at
                }
                (None, None) => s.since,
            };
            let until = settled + TimeDelta::seconds(STALL_ASK_SECS);
            if now < until { checking_d(acts, until) } else { ask_now(acts) }
        }
    }
}

/// The question an engine-raised ask puts for `card`.
pub(crate) fn raised_question(item: &WorkItem, raise: &Raise) -> (String, Vec<(&'static str, &'static str)>, Option<String>) {
    let title = one_line(&item.title, 80);
    match raise {
        Raise::Failed => (
            format!("\"{title}\" stopped on an error. What now?"),
            vec![(RETRY, "Run its thread again"), (DROP, "Take it off the plan")],
            None,
        ),
        Raise::Stalled => (
            format!("Nothing is moving on \"{title}\". What now?"),
            vec![
                (CONTINUE, "Tell its thread to carry on"),
                (CLOSE, "It's done: close it"),
                (DROP, "Take it off the plan"),
            ],
            None,
        ),
        Raise::Review(line) => (
            format!("\"{title}\" is ready for you to look at."),
            vec![(LOOKS_GOOD, "It's done")],
            line.as_ref().map(|l| format!("To look at: {}", one_line(l, 300))),
        ),
    }
}

/// The engine's asks' options.
pub(crate) const RETRY: &str = "Retry";
pub(crate) const DROP: &str = "Drop it";
pub(crate) const CONTINUE: &str = "Continue";
pub(crate) const CLOSE: &str = "Close it";
pub(crate) const LOOKS_GOOD: &str = "Looks good";

#[cfg(test)]
#[path = "derive_tests.rs"]
pub(crate) mod tests;
