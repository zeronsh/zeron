//! The ladder, one card at a time ([`derive`]), and what must hold of every
//! card whatever its facts: one place, the one the ladder gives; nothing in
//! Running without something provably on it; the same facts, the same place
//! (a restart changes nothing).

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{
    Ask, AskOrigin, AskState, BoardCard, CardStatus, CheckWhy, Checking, CloseHow, Closed, Finish, FinishKind, LinkKind,
    Mover, Place, Priority, ReportState, RouteState, Stall, Word, WorkItem, WorkLink, WorkState,
};

use super::super::board_checks::{Pr, Thread};
use super::*;

pub(crate) fn t0() -> DateTime<Utc> {
    "2026-11-22T12:00:00Z".parse().unwrap()
}

pub(crate) fn secs(n: i64) -> DateTime<Utc> {
    t0() + TimeDelta::seconds(n)
}

pub(crate) fn mins(n: i64) -> DateTime<Utc> {
    t0() + TimeDelta::minutes(n)
}

pub(crate) fn item() -> WorkItem {
    WorkItem {
        id: "c1".into(),
        orchestrator_id: Some("o1".into()),
        title: "Tool output error".into(),
        detail: String::new(),
        // (Never read: whatever an older writer left.)
        state: WorkState::Other,
        priority: Priority::Normal,
        progress: None,
        threads: vec!["t1".into()],
        links: vec![],
        outcome: None,
        source: None,
        order: 1.0,
        created_at: t0(),
        updated_at: t0(),
        done_at: None,
        removed: false,
        removed_at: None,
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

/// A started card's row: work was sent at the start.
pub(crate) fn row() -> BoardCard {
    let mut row = BoardCard::new("c1", t0());
    row.work_at = Some(t0());
    row
}

pub(crate) fn thread(standing: Thread) -> ThreadFact {
    ThreadFact {
        id: "t1".into(),
        standing,
        waiting: None,
        awaiting_input: false,
        limited: None,
        active_at: None,
        ended_at: None,
        elsewhere: false,
        resuming: false,
    }
}

pub(crate) fn live() -> ThreadFact {
    ThreadFact { active_at: Some(secs(5)), ..thread(Thread::Live { started_at: Some(secs(5)) }) }
}

/// Its turn completed at `at`.
pub(crate) fn finished(at: DateTime<Utc>) -> ThreadFact {
    ThreadFact { active_at: Some(at), ended_at: Some(at), ..thread(Thread::Finished) }
}

pub(crate) fn owner() -> Owner {
    Owner { wakeable: true, live: false, started_at: None, ended_at: None }
}

pub(crate) fn ask(id: &str, origin: AskOrigin, at: DateTime<Utc>) -> Ask {
    serde_json::from_value(serde_json::json!({
        "id": id, "fromChat": "t1", "orchestratorId": "o1", "itemId": "c1", "kind": "decision",
        "question": "Ship it?", "state": "open", "createdAt": at, "origin": origin,
    }))
    .unwrap()
}

pub(crate) struct Case {
    pub item: WorkItem,
    pub row: BoardCard,
    pub threads: Vec<ThreadFact>,
    pub asks: Vec<Ask>,
    pub owner: Option<Owner>,
    pub waits_on: Vec<String>,
    pub route: Option<RouteFact>,
    pub prs: Vec<Pr>,
    pub merged: Option<String>,
    pub pr_ask: Option<Ask>,
}

impl Case {
    pub fn new() -> Self {
        Case {
            item: item(),
            row: row(),
            threads: vec![],
            asks: vec![],
            owner: Some(owner()),
            waits_on: vec![],
            route: None,
            prs: vec![],
            merged: None,
            pr_ask: None,
        }
    }

    pub fn with(thread: ThreadFact) -> Self {
        Case { threads: vec![thread], ..Case::new() }
    }

    pub fn at(&self, now: DateTime<Utc>) -> Derived {
        derive(
            &Facts {
                item: &self.item,
                card: &self.row,
                threads: self.threads.clone(),
                asks: self.asks.iter().collect(),
                owner: self.owner.clone(),
                waits_on: self.waits_on.clone(),
                route: self.route.clone(),
                prs: self.prs.clone(),
                merged: self.merged.clone(),
                pr_ask: self.pr_ask.as_ref(),
            },
            now,
        )
    }

    /// The acts' facts written to the row, as `board.rs` applies them (the
    /// asks it raises land in `asks`).
    pub fn apply(&mut self, d: &Derived, now: DateTime<Utc>) {
        for act in &d.acts {
            match act {
                Act::Reopen => self.row.closed = None,
                Act::Close { how, note } => {
                    self.row.closed = Some(Closed { at: now, by: Mover::Engine, how: *how, note: note.clone() })
                }
                Act::ClearChecking => self.row.checking = None,
                Act::StallBegin { wake } => {
                    self.row.stall = Some(Stall { since: now, woken_at: wake.then_some(now), settled_at: None, ask: None });
                    if *wake {
                        self.row.checking = Some(Checking { since: now, why: CheckWhy::Stall });
                    }
                }
                Act::StallSettled { at } => self.row.stall.as_mut().unwrap().settled_at = Some(*at),
                Act::ClearStall => self.row.stall = None,
                Act::Unpause => self.item.paused_at = None,
                Act::Hold => self.row.held = true,
                Act::DropThreads(gone) => {
                    self.item.threads.retain(|t| !gone.contains(t));
                    self.threads.retain(|t| !gone.contains(&t.id));
                    for t in gone {
                        self.item.links.push(WorkLink { kind: LinkKind::Chat, label: "Archived thread".into(), target: t.clone() });
                    }
                }
                Act::Raise(raise) => {
                    let origin = match raise {
                        Raise::Failed => AskOrigin::BoardCheck,
                        Raise::Stalled => AskOrigin::Stall,
                        Raise::Review(_) => AskOrigin::Review,
                    };
                    let id = format!("raised-{}", self.asks.len());
                    if *raise == Raise::Stalled {
                        self.row.stall.as_mut().unwrap().ask = Some(id.clone());
                    }
                    self.asks.push(ask(&id, origin, now));
                }
                Act::JoinPrAsk => {
                    if self.pr_ask.is_none() {
                        let mut a = ask("prs-o1", AskOrigin::Review, now);
                        a.item_id = None;
                        self.pr_ask = Some(a);
                    }
                }
                Act::Resolve { ask, .. } => {
                    for a in self.asks.iter_mut().filter(|a| a.id == *ask) {
                        a.state = AskState::Answered;
                        a.answered_at = Some(now);
                    }
                }
            }
        }
    }

    /// Worked out and applied at `now`.
    pub fn step(&mut self, now: DateTime<Utc>) -> Derived {
        let d = self.at(now);
        self.apply(&d, now);
        d
    }
}

fn place(c: &Case, now: DateTime<Utc>) -> (Place, CardStatus) {
    let d = c.at(now);
    (d.place, d.status)
}

fn word(state: ReportState, at: DateTime<Utc>) -> Option<Word> {
    Some(Word { state, at, from: "t1".into(), summary: "Fixed and tested".into(), report: Some("r1".into()) })
}

// ── 1. The finish line ──────────────────────────────────────────────────────

#[test]
fn a_closed_card_is_done_whatever_its_thread_does() {
    let mut c = Case::with(live());
    c.row.closed = Some(Closed { at: mins(1), by: Mover::You, how: CloseHow::Marked, note: None });
    assert_eq!(place(&c, mins(2)), (Place::Done, CardStatus::Done));
    c.row.closed.as_mut().unwrap().how = CloseHow::Dropped;
    assert_eq!(place(&c, mins(2)), (Place::Dropped, CardStatus::Dropped));
}

#[test]
fn a_done_report_closes_it_once_its_orchestrators_turn_on_it_is_over() {
    let mut c = Case::with(finished(mins(5)));
    c.row.word = word(ReportState::Done, mins(5));
    c.row.checking = Some(Checking { since: mins(5), why: CheckWhy::Report });
    // The orchestrator's wake turn runs: it is checking.
    c.owner = Some(Owner { live: true, started_at: Some(mins(5) + TimeDelta::seconds(15)), ..owner() });
    assert_eq!(place(&c, mins(6)), (Place::Running, CardStatus::Checking));
    // Its turn ended without sending more work or asking: done.
    c.owner = Some(Owner { started_at: Some(mins(5) + TimeDelta::seconds(15)), ended_at: Some(mins(7)), ..owner() });
    let d = c.at(mins(7) + TimeDelta::seconds(1));
    assert_eq!((d.place, d.status), (Place::Done, CardStatus::Done));
    assert!(d.acts.contains(&Act::ClearChecking));
    assert!(d.acts.contains(&Act::Close { how: CloseHow::Report, note: Some("Fixed and tested".into()) }));
}

#[test]
fn more_work_sent_after_a_done_report_keeps_it_open() {
    let mut c = Case::with(finished(mins(5)));
    c.row.word = word(ReportState::Done, mins(5));
    // Its orchestrator sent its thread more work in the turn on the report.
    c.row.work_at = Some(mins(6));
    assert_eq!(place(&c, mins(6) + TimeDelta::seconds(5)), (Place::Running, CardStatus::Queued));
}

#[test]
fn an_ask_after_a_done_report_keeps_it_open() {
    let mut c = Case::with(finished(mins(5)));
    c.row.word = word(ReportState::Done, mins(5));
    c.asks = vec![ask("a1", AskOrigin::Tool, mins(6))];
    assert_eq!(place(&c, mins(7)), (Place::NeedsYou, CardStatus::Asked));
    // Put off, it still isn't done: it waits in Snoozed.
    c.asks[0].deferred_until = Some(mins(600));
    assert_eq!(place(&c, mins(7)), (Place::Snoozed, CardStatus::Asked));
}

#[test]
fn an_agents_word_alone_never_closes_code_work() {
    // Merged: a done report isn't the finish; the branch landing is.
    let mut c = Case::with(finished(mins(5)));
    c.row.finish = Finish::of(FinishKind::Merged);
    c.row.word = word(ReportState::Done, mins(5));
    let d = c.at(mins(10));
    assert_ne!(d.place, Place::Done);
    c.merged = Some("e99f817".into());
    let d = c.at(mins(10));
    assert_eq!(d.place, Place::Done);
    assert_eq!(d.why, Why::Closed { how: CloseHow::Merged, by: Mover::Engine, note: Some("e99f817".into()) });
}

#[test]
fn a_merged_branch_doesnt_close_a_card_its_thread_still_works_on() {
    let mut c = Case::with(live());
    c.row.finish = Finish::of(FinishKind::Merged);
    c.merged = Some("e99f817".into());
    assert_eq!(place(&c, mins(1)), (Place::Running, CardStatus::Working));
}

#[test]
fn a_pr_card_is_done_when_its_pr_is_merged_or_closed() {
    let mut c = Case::with(finished(mins(5)));
    c.row.finish = Finish::of(FinishKind::Pr);
    c.row.word = word(ReportState::Done, mins(5));
    c.prs = vec![Pr { open: Some(false), ci_failed: false }];
    assert_eq!(c.at(mins(10)).place, Place::Done);
    // Open (or not looked up yet): it waits on the person, in its
    // orchestrator's one PR ask.
    c.prs = vec![Pr { open: Some(true), ci_failed: false }];
    let d = c.at(mins(10));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Review));
    assert!(d.acts.contains(&Act::JoinPrAsk));
    c.prs = vec![Pr::default()];
    assert_eq!(c.at(mins(10)).place, Place::NeedsYou);
}

#[test]
fn a_put_off_pr_ask_snoozes_every_card_it_stands_for() {
    let mut c = Case::with(finished(mins(5)));
    c.row.finish = Finish::of(FinishKind::Pr);
    c.row.word = word(ReportState::Done, mins(5));
    c.prs = vec![Pr { open: Some(true), ci_failed: false }];
    let mut prs = ask("prs-o1", AskOrigin::Review, mins(6));
    prs.item_id = None;
    prs.deferred_until = Some(mins(600));
    c.pr_ask = Some(prs);
    let d = c.at(mins(10));
    assert_eq!((d.place, d.status), (Place::Snoozed, CardStatus::Review));
    assert_eq!(d.until, Some(mins(600)));
    // Back when it returns.
    assert_eq!(c.at(mins(601)).place, Place::NeedsYou);
}

#[test]
fn an_accept_card_waits_for_looks_good() {
    let mut c = Case::with(finished(mins(5)));
    c.row.finish = Finish::of(FinishKind::Accept);
    c.row.word = word(ReportState::Done, mins(5));
    let d = c.step(mins(10));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Review));
    assert!(d.acts.contains(&Act::Raise(Raise::Review(None))));
    // Asked once: the open ask holds it there.
    let d = c.at(mins(11));
    assert_eq!(d.place, Place::NeedsYou);
    assert!(!d.acts.iter().any(|a| matches!(a, Act::Raise(_))));
    // "Looks good" is the person's: the op records the closing.
    c.row.closed = Some(Closed { at: mins(12), by: Mover::You, how: CloseHow::Accepted, note: None });
    let d = c.at(mins(12));
    assert_eq!(d.place, Place::Done);
    assert!(d.acts.iter().any(|a| matches!(a, Act::Resolve { .. })), "its question closes with it");
}

#[test]
fn a_thread_that_reports_to_nobody_is_done_when_its_turn_completes() {
    // A plain chat put on the board: no orchestrator, no route.
    let mut c = Case::with(finished(mins(5)));
    c.item.orchestrator_id = None;
    c.owner = None;
    // A moment for a question it ended on to be raised.
    assert_eq!(place(&c, mins(5) + TimeDelta::seconds(10)), (Place::Running, CardStatus::Checking));
    assert_eq!(c.at(mins(6)).place, Place::Done);
}

#[test]
fn a_chat_put_on_the_board_after_its_last_turn_isnt_done_by_that_turn() {
    let mut c = Case::with(finished(mins(-30)));
    c.item.orchestrator_id = None;
    c.owner = None;
    c.row.work_at = None;
    assert_eq!(place(&c, mins(1)), (Place::Planned, CardStatus::Next));
}

#[test]
fn a_routed_cards_route_ending_is_its_report() {
    let mut c = Case::with(finished(mins(5)));
    c.item.orchestrator_id = None;
    c.owner = None;
    c.route = Some(RouteFact { state: RouteState::Answered, review: None, ends_it: true });
    assert_ne!(c.at(mins(6)).place, Place::Done, "an outcome so far isn't the end");
    c.route = Some(RouteFact { state: RouteState::Done, review: None, ends_it: true });
    assert_eq!(c.at(mins(6)).place, Place::Done);
    // Finished leaving the person something: theirs to accept.
    c.route = Some(RouteFact { state: RouteState::Done, review: Some("PR #1403".into()), ends_it: true });
    let d = c.at(mins(6));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Review));
    assert!(d.acts.contains(&Act::Raise(Raise::Review(Some("PR #1403".into())))));
}

#[test]
fn a_route_that_ended_closes_its_card_though_its_session_still_shows_the_turn() {
    // The route's outcome is recorded as the turn ends; the thread's session
    // row may say working for a moment more.
    let mut c = Case::with(live());
    c.item.orchestrator_id = None;
    c.owner = None;
    c.row.heard_at = Some(mins(5));
    c.route = Some(RouteFact { state: RouteState::Done, review: None, ends_it: true });
    assert_eq!(c.at(mins(5)).place, Place::Done);
    // Work sent since (changes asked): its thread is at work on it again.
    c.row.work_at = Some(mins(6));
    assert_eq!(c.at(mins(6) + TimeDelta::seconds(5)).place, Place::Running);
    // A route still going closes nothing.
    c.row.work_at = Some(t0());
    c.route = Some(RouteFact { state: RouteState::Answered, review: None, ends_it: true });
    assert_eq!(c.at(mins(5)).place, Place::Running);
}

#[test]
fn work_sent_reopens_what_an_agents_word_closed_but_not_what_the_person_closed() {
    let mut c = Case::with(live());
    c.row.closed = Some(Closed { at: mins(5), by: Mover::Engine, how: CloseHow::Report, note: None });
    c.row.work_at = Some(mins(8));
    let d = c.at(mins(9));
    assert!(d.acts.contains(&Act::Reopen));
    assert_eq!(d.place, Place::Running);
    for (by, how) in [(Mover::You, CloseHow::Marked), (Mover::You, CloseHow::Accepted), (Mover::Engine, CloseHow::Merged)] {
        c.row.closed = Some(Closed { at: mins(5), by, how, note: None });
        let d = c.at(mins(9));
        assert_eq!(d.place, Place::Done, "{how:?}");
        assert!(!d.acts.contains(&Act::Reopen));
    }
}

// ── 2. Needs you ────────────────────────────────────────────────────────────

#[test]
fn an_open_ask_is_needs_you_even_while_its_thread_works() {
    let mut c = Case::with(live());
    c.asks = vec![ask("a1", AskOrigin::Tool, mins(1))];
    let d = c.at(mins(2));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Asked));
    assert_eq!(d.asks, ["a1"]);
}

#[test]
fn the_urgent_ask_is_answered_first() {
    let mut c = Case::with(finished(mins(1)));
    let mut urgent = ask("a2", AskOrigin::Tool, mins(3));
    urgent.urgent = true;
    c.asks = vec![ask("a1", AskOrigin::Tool, mins(2)), urgent];
    assert_eq!(c.at(mins(5)).asks, ["a2", "a1"]);
}

#[test]
fn an_errored_thread_asks_retry_or_drop_once() {
    let mut c = Case::with(ThreadFact { ended_at: Some(mins(5)), active_at: Some(mins(5)), ..thread(Thread::Errored { at: Some(mins(5)) }) });
    // A moment first: a usage limit's mark, a resume, may be on its way.
    assert_eq!(place(&c, mins(5) + TimeDelta::seconds(5)), (Place::Running, CardStatus::Checking));
    let d = c.step(mins(6));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Failed));
    assert!(d.acts.contains(&Act::Raise(Raise::Failed)));
    let (question, options, _) = raised_question(&c.item, &Raise::Failed);
    assert!(question.contains("Tool output error"));
    assert_eq!(options.iter().map(|(l, _)| *l).collect::<Vec<_>>(), [RETRY, DROP]);
    // The ask stands for it: not raised again.
    let d = c.at(mins(7));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Failed));
    assert!(!d.acts.iter().any(|a| matches!(a, Act::Raise(_))));
}

#[test]
fn a_failure_ask_closes_itself_when_its_thread_works_again() {
    let mut c = Case::with(ThreadFact { ended_at: Some(mins(5)), active_at: Some(mins(5)), ..thread(Thread::Errored { at: Some(mins(5)) }) });
    c.step(mins(6));
    c.threads = vec![ThreadFact { active_at: Some(mins(8)), ..thread(Thread::Live { started_at: Some(mins(8)) }) }];
    let d = c.at(mins(8));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Working));
    assert!(d.acts.iter().any(|a| matches!(a, Act::Resolve { why, .. } if why.contains("working again"))));
}

#[test]
fn a_thread_waiting_for_the_person_in_its_chat_needs_them() {
    let c = Case::with(ThreadFact { awaiting_input: true, ..live() });
    let d = c.at(mins(1));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Asked));
    assert_eq!(d.note.as_deref(), Some("Answer in its thread"));
}

// ── 3. Running ──────────────────────────────────────────────────────────────

#[test]
fn a_working_thread_is_running_with_what_it_is_doing() {
    assert_eq!(place(&Case::with(live()), mins(1)), (Place::Running, CardStatus::Working));
    let waiting = Case::with(ThreadFact { waiting: Some("cargo test -p zeron-engine".into()), ..live() });
    let d = waiting.at(mins(1));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Waiting));
    assert_eq!(d.note.as_deref(), Some("cargo test -p zeron-engine"));
    let limited = Case::with(ThreadFact { limited: Some(Some(mins(90))), ..live() });
    let d = limited.at(mins(1));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Limited));
    assert_eq!(d.until, Some(mins(90)));
}

#[test]
fn work_on_its_way_is_running_until_the_thread_shows_it_or_the_wait_runs_out() {
    let mut c = Case::with(thread(Thread::Idle));
    c.row.work_at = Some(mins(1));
    let d = c.at(mins(1) + TimeDelta::seconds(10));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Queued));
    assert_eq!(d.recheck, Some(mins(1) + TimeDelta::seconds(IN_FLIGHT_SECS)));
    // Never picked up: no longer on its way (it stalls).
    let d = c.at(mins(1) + TimeDelta::seconds(IN_FLIGHT_SECS + 1));
    assert_ne!(d.status, CardStatus::Queued);
    assert!(d.acts.contains(&Act::StallBegin { wake: true }));
}

#[test]
fn a_thread_at_work_for_a_newer_card_isnt_on_this_one() {
    let c = Case::with(ThreadFact { elsewhere: true, ..live() });
    let d = c.at(mins(5));
    assert_ne!((d.place, d.status), (Place::Running, CardStatus::Working));
}

#[test]
fn a_thread_resumed_after_a_restart_keeps_its_card_running() {
    // (Stopped by the engine's own shutdown, long after its work was sent.)
    let c = Case::with(ThreadFact { resuming: true, ended_at: Some(mins(9)), active_at: Some(mins(9)), ..thread(Thread::Stopped { at: mins(9) }) });
    let d = c.at(mins(15));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Working));
    assert_eq!(d.note.as_deref(), Some("Resuming"));
    assert!(d.acts.is_empty(), "no stall, no ask: {:?}", d.acts);
}

#[test]
fn a_card_made_inside_a_turn_is_running_until_that_turn_ends() {
    let mut c = Case::new();
    c.item.threads.clear();
    c.row.work_at = None;
    c.row.checking = Some(Checking { since: mins(1), why: CheckWhy::Created });
    c.owner = Some(Owner { live: true, started_at: Some(mins(0)), ..owner() });
    assert_eq!(place(&c, mins(2)), (Place::Running, CardStatus::Checking));
    c.owner = Some(Owner { started_at: Some(mins(0)), ended_at: Some(mins(3)), ..owner() });
    let d = c.at(mins(3) + TimeDelta::seconds(1));
    assert!(d.acts.contains(&Act::ClearChecking));
    assert_eq!((d.place, d.status), (Place::Planned, CardStatus::Next));
}

#[test]
fn a_wake_that_never_comes_stops_counting() {
    let mut c = Case::with(finished(mins(5)));
    c.row.finish = Finish::of(FinishKind::Merged);
    c.row.checking = Some(Checking { since: mins(5), why: CheckWhy::Report });
    assert_eq!(place(&c, mins(5) + TimeDelta::seconds(60)), (Place::Running, CardStatus::Checking));
    let d = c.at(mins(5) + TimeDelta::seconds(CHECK_HANDOFF_SECS + 1));
    assert!(d.acts.contains(&Act::ClearChecking));
}

// ── 4 and 5. Snoozed, Planned ───────────────────────────────────────────────

#[test]
fn a_snoozed_card_whose_thread_works_is_running() {
    let mut c = Case::with(live());
    c.asks = vec![ask("a1", AskOrigin::Tool, mins(1))];
    c.asks[0].deferred_until = Some(mins(600));
    assert_eq!(place(&c, mins(2)), (Place::Running, CardStatus::Working));
    c.threads = vec![finished(mins(3))];
    let d = c.at(mins(10));
    assert_eq!(d.place, Place::Snoozed);
    assert_eq!(d.recheck, Some(mins(600)));
}

#[test]
fn planned_holds_what_is_paused_waiting_held_or_not_started() {
    let mut c = Case::with(thread(Thread::Idle));
    c.row.work_at = None;
    assert_eq!(place(&c, mins(1)), (Place::Planned, CardStatus::Next));
    c.row.held = true;
    assert_eq!(place(&c, mins(1)), (Place::Planned, CardStatus::Held));
    c.row.held = false;
    c.waits_on = vec!["Keyed sidebar shape".into(), "Other".into()];
    let d = c.at(mins(1));
    assert_eq!((d.place, d.status), (Place::Planned, CardStatus::After));
    assert_eq!(d.note.as_deref(), Some("Keyed sidebar shape and 1 more"));
    c.item.paused_at = Some(mins(0));
    assert_eq!(place(&c, mins(1)), (Place::Planned, CardStatus::Paused));
}

#[test]
fn a_paused_card_is_never_closed_by_an_agents_word() {
    // Its thread said done as the pause stopped it: the card is the
    // person's until they resume it.
    let mut c = Case::with(ThreadFact { ended_at: Some(mins(2)), active_at: Some(mins(2)), ..thread(Thread::Stopped { at: mins(2) }) });
    c.item.paused_at = Some(mins(2));
    c.row.word = word(ReportState::Done, mins(2));
    let d = c.at(mins(30));
    assert_eq!((d.place, d.status), (Place::Planned, CardStatus::Paused));
    assert!(!d.acts.iter().any(|a| matches!(a, Act::Close { .. })), "{:?}", d.acts);
    // Resumed, the word it had stands.
    c.item.paused_at = None;
    assert_eq!(c.at(mins(31)).place, Place::Done);
}

#[test]
fn a_paused_card_stays_paused_until_its_thread_starts_again() {
    let mut c = Case::with(ThreadFact { ended_at: Some(mins(2)), active_at: Some(mins(2)), ..thread(Thread::Stopped { at: mins(2) }) });
    c.item.paused_at = Some(mins(2));
    assert_eq!(place(&c, mins(2) + TimeDelta::seconds(5)), (Place::Planned, CardStatus::Paused));
    assert_eq!(place(&c, mins(30)), (Place::Planned, CardStatus::Paused), "paused never stalls");
    // Still winding down from before the pause: paused.
    c.threads = vec![ThreadFact { active_at: Some(mins(1)), ..thread(Thread::Live { started_at: Some(mins(1)) }) }];
    assert_eq!(place(&c, mins(2) + TimeDelta::seconds(1)), (Place::Planned, CardStatus::Paused));
    // Started after it (the person, in its chat): no longer paused.
    c.threads = vec![ThreadFact { active_at: Some(mins(9)), ..thread(Thread::Live { started_at: Some(mins(9)) }) }];
    let d = c.at(mins(9));
    assert!(d.acts.contains(&Act::Unpause));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Working));
}

// ── 6. Stalled ──────────────────────────────────────────────────────────────

#[test]
fn a_stalled_card_wakes_its_orchestrator_once_then_asks_the_person() {
    // Its thread finished and reported nothing.
    let mut c = Case::with(finished(mins(5)));
    c.row.finish = Finish::of(FinishKind::Merged);
    // A while first: its report, its question or its next turn may be on
    // its way.
    for after in [10, HANDOFF_SECS + 1, STALL_SETTLE_SECS - 1] {
        let d = c.step(mins(5) + TimeDelta::seconds(after));
        assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking), "{after}s");
        assert!(d.acts.is_empty(), "{after}s: {:?}", d.acts);
        // (Looked at again when the wait runs out, at the latest.)
        assert!(d.recheck.is_some_and(|at| at <= mins(5) + TimeDelta::seconds(STALL_SETTLE_SECS)), "{after}s: {:?}", d.recheck);
    }
    // Then its orchestrator is woken, once.
    let woke = mins(5) + TimeDelta::seconds(STALL_SETTLE_SECS + 1);
    let d = c.step(woke);
    assert_eq!(d.acts, [Act::StallBegin { wake: true }]);
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking));
    // Its turn runs.
    c.owner = Some(Owner { live: true, started_at: Some(woke + TimeDelta::seconds(2)), ..owner() });
    let d = c.step(woke + TimeDelta::seconds(30));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking));
    assert!(d.acts.is_empty(), "not woken again: {:?}", d.acts);
    // It ended having done nothing about the card.
    let ended = woke + TimeDelta::seconds(60);
    c.owner = Some(Owner { started_at: Some(woke + TimeDelta::seconds(2)), ended_at: Some(ended), ..owner() });
    let d = c.step(ended + TimeDelta::seconds(1));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking));
    assert!(d.acts.contains(&Act::StallSettled { at: ended }));
    assert!(!d.acts.iter().any(|a| matches!(a, Act::StallBegin { .. })), "one wake");
    assert_eq!(d.recheck, Some(ended + TimeDelta::seconds(STALL_ASK_SECS)));
    // Two minutes after that turn: the person is asked.
    let d = c.step(ended + TimeDelta::seconds(STALL_ASK_SECS + 1));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Stalled));
    assert!(d.acts.contains(&Act::Raise(Raise::Stalled)));
    let (question, options, _) = raised_question(&c.item, &Raise::Stalled);
    assert!(question.starts_with("Nothing is moving on"));
    assert_eq!(options.iter().map(|(l, _)| *l).collect::<Vec<_>>(), [CONTINUE, CLOSE, DROP]);
    // And stays asked, not asked again.
    let d = c.step(ended + TimeDelta::seconds(STALL_ASK_SECS + 600));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Stalled));
    assert!(!d.acts.iter().any(|a| matches!(a, Act::Raise(_))));
}

#[test]
fn a_stall_is_over_when_something_moves() {
    let mut c = Case::with(finished(mins(5)));
    c.row.finish = Finish::of(FinishKind::Merged);
    c.step(mins(7));
    assert!(c.row.stall.is_some());
    // Its orchestrator sent its thread on.
    c.row.work_at = Some(mins(8));
    let d = c.step(mins(8) + TimeDelta::seconds(1));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Queued));
    assert!(c.row.stall.is_none());
}

#[test]
fn with_no_orchestrator_to_wake_the_person_is_asked_after_the_wait() {
    let mut c = Case::with(ThreadFact { ended_at: Some(mins(5)), active_at: Some(mins(5)), ..thread(Thread::Stopped { at: mins(5) }) });
    c.owner = Some(Owner { wakeable: false, ..owner() });
    let began = mins(7);
    let d = c.step(began);
    assert_eq!(d.acts, [Act::StallBegin { wake: false }]);
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking));
    let d = c.step(began + TimeDelta::seconds(STALL_ASK_SECS + 1));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Stalled));
}

#[test]
fn a_dismissed_stall_question_leaves_the_card_alone() {
    let mut c = Case::with(finished(mins(5)));
    c.owner = Some(Owner { wakeable: false, ..owner() });
    c.row.finish = Finish::of(FinishKind::Merged);
    c.step(mins(7));
    c.step(mins(7) + TimeDelta::seconds(STALL_ASK_SECS + 1));
    assert_eq!(c.asks.len(), 1);
    // Dismissed by an app from before the hold (which sets `held` itself).
    c.asks[0].state = AskState::Dismissed;
    c.asks[0].answered_at = Some(mins(10));
    let d = c.step(mins(10));
    assert_eq!((d.place, d.status), (Place::Planned, CardStatus::Held));
    assert!(c.row.held);
    let d = c.step(mins(60));
    assert_eq!((d.place, d.status), (Place::Planned, CardStatus::Held), "not asked again");
    assert!(d.acts.is_empty());
    // Its thread put to work is Running while it works, and held again
    // after, with no question: left alone until the person lets it go (or
    // answers Continue, or it is started again), which takes the hold off.
    c.threads = vec![ThreadFact { active_at: Some(mins(61)), ..thread(Thread::Live { started_at: Some(mins(61)) }) }];
    assert_eq!(c.step(mins(62)).place, Place::Running);
    c.threads = vec![finished(mins(70))];
    let d = c.step(mins(75));
    assert_eq!((d.place, d.status), (Place::Planned, CardStatus::Held));
    assert!(d.acts.is_empty(), "{:?}", d.acts);
    c.row.held = false;
    let d = c.step(mins(76));
    assert!(d.acts.contains(&Act::StallBegin { wake: false }), "let go, it is checked again: {:?}", d.acts);
}

#[test]
fn an_archived_thread_comes_off_a_stalled_card_and_it_stays_started() {
    let mut c = Case::with(thread(Thread::Gone));
    c.row.work_at = None;
    let d = c.step(mins(30));
    assert!(d.acts.contains(&Act::DropThreads(vec!["t1".into()])));
    assert!(c.item.threads.is_empty());
    // Still a started card (its chat link says so): its stall goes on.
    let d = c.step(mins(31));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking));
    assert!(c.row.stall.is_some());
}

// ── Invariants ──────────────────────────────────────────────────────────────

/// Every combination of the facts that pick a rung.
fn every_case() -> Vec<(String, Case)> {
    let threads: Vec<(&str, Vec<ThreadFact>)> = vec![
        ("no thread", vec![]),
        ("idle", vec![thread(Thread::Idle)]),
        ("live", vec![live()]),
        ("live elsewhere", vec![ThreadFact { elsewhere: true, ..live() }]),
        ("waiting", vec![ThreadFact { waiting: Some("tests".into()), ..live() }]),
        ("limited", vec![ThreadFact { limited: Some(None), ..live() }]),
        ("awaiting input", vec![ThreadFact { awaiting_input: true, ..live() }]),
        ("just finished", vec![finished(mins(20) - TimeDelta::seconds(5))]),
        ("finished", vec![finished(mins(5))]),
        ("errored", vec![ThreadFact { ended_at: Some(mins(5)), active_at: Some(mins(5)), ..thread(Thread::Errored { at: Some(mins(5)) }) }]),
        ("stopped", vec![ThreadFact { ended_at: Some(mins(5)), active_at: Some(mins(5)), ..thread(Thread::Stopped { at: mins(5) }) }]),
        ("resuming", vec![ThreadFact { resuming: true, ..thread(Thread::Stopped { at: mins(5) }) }]),
        ("gone", vec![thread(Thread::Gone)]),
    ];
    let asks: Vec<(&str, Vec<Ask>)> = vec![
        ("no ask", vec![]),
        ("open ask", vec![ask("a1", AskOrigin::Tool, mins(6))]),
        ("snoozed ask", vec![{
            let mut a = ask("a1", AskOrigin::Tool, mins(6));
            a.deferred_until = Some(mins(900));
            a
        }]),
        ("stall ask", vec![ask("s1", AskOrigin::Stall, mins(6))]),
        ("failure ask", vec![ask("b1", AskOrigin::BoardCheck, mins(6))]),
        ("answered ask", vec![{
            let mut a = ask("a1", AskOrigin::Tool, mins(6));
            a.state = AskState::Answered;
            a.answered_at = Some(mins(7));
            a
        }]),
    ];
    type Change = Box<dyn Fn(&mut Case)>;
    let rows: Vec<(&str, Change)> = vec![
        ("plain", Box::new(|_| {})),
        ("not started", Box::new(|c| c.row.work_at = None)),
        ("said done", Box::new(|c| c.row.word = word(ReportState::Done, mins(5)))),
        ("said blocked", Box::new(|c| c.row.word = word(ReportState::Blocked, mins(5)))),
        ("work just sent", Box::new(|c| c.row.work_at = Some(mins(20) - TimeDelta::seconds(20)))),
        ("checking", Box::new(|c| c.row.checking = Some(Checking { since: mins(19), why: CheckWhy::Report }))),
        ("stalled woken", Box::new(|c| {
            c.row.stall = Some(Stall { since: mins(10), woken_at: Some(mins(10)), settled_at: Some(mins(11)), ask: None })
        })),
        ("stall asked", Box::new(|c| {
            c.row.stall = Some(Stall { since: mins(10), woken_at: Some(mins(10)), settled_at: Some(mins(11)), ask: Some("s1".into()) })
        })),
        ("held", Box::new(|c| c.row.held = true)),
        ("paused", Box::new(|c| c.item.paused_at = Some(mins(8)))),
        ("waits on", Box::new(|c| c.waits_on = vec!["Another".into()])),
        ("closed by report", Box::new(|c| {
            c.row.closed = Some(Closed { at: mins(9), by: Mover::Engine, how: CloseHow::Report, note: None })
        })),
        ("closed by you", Box::new(|c| {
            c.row.closed = Some(Closed { at: mins(9), by: Mover::You, how: CloseHow::Marked, note: None })
        })),
        ("dropped", Box::new(|c| {
            c.row.closed = Some(Closed { at: mins(9), by: Mover::You, how: CloseHow::Dropped, note: None })
        })),
        ("merged line", Box::new(|c| c.row.finish = Finish::of(FinishKind::Merged))),
        ("merged", Box::new(|c| {
            c.row.finish = Finish::of(FinishKind::Merged);
            c.merged = Some("abc1234".into());
        })),
        ("pr open", Box::new(|c| {
            c.row.finish = Finish::of(FinishKind::Pr);
            c.prs = vec![Pr { open: Some(true), ci_failed: false }];
        })),
        ("accept line", Box::new(|c| c.row.finish = Finish::of(FinishKind::Accept))),
        ("loose", Box::new(|c| {
            c.item.orchestrator_id = None;
            c.owner = None;
        })),
        ("owner paused", Box::new(|c| c.owner = Some(Owner { wakeable: false, ..owner() }))),
        ("owner in a turn", Box::new(|c| c.owner = Some(Owner { live: true, started_at: Some(mins(19)), ..owner() }))),
        ("routed, done", Box::new(|c| c.route = Some(RouteFact { state: RouteState::Done, review: None, ends_it: true }))),
        ("routed, to review", Box::new(|c| {
            c.route = Some(RouteFact { state: RouteState::Done, review: Some("a PR".into()), ends_it: true })
        })),
    ];
    let mut all = Vec::new();
    for (tn, ts) in &threads {
        for (an, asks) in &asks {
            for (rn, change) in &rows {
                let mut c = Case::new();
                c.threads = ts.clone();
                c.item.threads = ts.iter().map(|t| t.id.clone()).collect();
                c.asks = asks.clone();
                change(&mut c);
                all.push((format!("{tn} / {an} / {rn}"), c));
            }
        }
    }
    all
}

/// What in the facts says an agent is on the card, however the ladder is
/// written: the claim Running makes.
fn something_is_on_it(c: &Case, now: DateTime<Utc>) -> bool {
    let live = c.threads.iter().any(|t| matches!(t.standing, Thread::Live { .. }) && !t.elsewhere);
    let last = c.threads.iter().filter_map(|t| t.active_at).max();
    let in_flight =
        c.row.work_at.is_some_and(|s| last.is_none_or(|a| a < s) && now - s < TimeDelta::seconds(IN_FLIGHT_SECS));
    let resuming = c.threads.iter().any(|t| t.resuming && !t.elsewhere);
    let owner_turn = c.row.checking.is_some() && c.owner.as_ref().is_some_and(|o| o.wakeable);
    let just_ended = c
        .threads
        .iter()
        .filter_map(|t| t.ended_at)
        .chain(c.owner.as_ref().and_then(|o| o.ended_at))
        .any(|e| now - e < TimeDelta::seconds(HANDOFF_SECS));
    // The engine's own check of a stalled card, which has a deadline.
    let checking_a_stall = c.row.stall.as_ref().is_some_and(|s| s.ask.is_none());
    live || in_flight || resuming || owner_turn || just_ended || checking_a_stall
}

#[test]
fn every_card_is_in_exactly_one_place_and_running_is_never_empty() {
    let now = mins(20);
    let cases = every_case();
    assert!(cases.len() > 1500, "{}", cases.len());
    for (name, mut c) in cases {
        let d = c.at(now);
        // One place, a real one.
        assert_ne!(d.place, Place::Other, "{name}");
        assert_ne!(d.status, CardStatus::Other, "{name}");
        // The same facts, the same answer: nothing remembered between
        // calls (what a restart loses changes nothing).
        assert_eq!(c.at(now), d, "{name}");
        // The status belongs to the place.
        let fits = match d.place {
            Place::NeedsYou | Place::Snoozed => {
                matches!(d.status, CardStatus::Asked | CardStatus::Failed | CardStatus::Review | CardStatus::Stalled)
            }
            Place::Running => matches!(
                d.status,
                CardStatus::Working | CardStatus::Waiting | CardStatus::Limited | CardStatus::Checking | CardStatus::Queued
            ),
            Place::Planned => matches!(d.status, CardStatus::Next | CardStatus::Held | CardStatus::Paused | CardStatus::After),
            Place::Done => d.status == CardStatus::Done,
            Place::Dropped => d.status == CardStatus::Dropped,
            Place::Other => false,
        };
        assert!(fits, "{name}: {:?} in {:?}", d.status, d.place);
        // The ladder's order: a closing first, then an open ask.
        let closed_stands = c.row.closed.as_ref().is_some_and(|k| {
            k.by == Mover::You || !c.row.work_at.is_some_and(|s| s > k.at) || matches!(k.how, CloseHow::Merged | CloseHow::Pr)
        });
        if closed_stands {
            assert!(d.place.closed(), "{name}: closed, yet {:?}", d.place);
        }
        if d.place == Place::NeedsYou {
            let asked = c.asks.iter().any(|a| a.state == AskState::Open && !a.deferred(now))
                || d.acts.iter().any(|a| matches!(a, Act::Raise(_) | Act::JoinPrAsk))
                || c.threads.iter().any(|t| t.awaiting_input);
            assert!(asked, "{name}: Needs you with nothing asked");
        }
        if d.place == Place::Snoozed {
            assert!(c.asks.iter().any(|a| a.state == AskState::Open && a.deferred(now)), "{name}");
            assert!(d.until.is_some(), "{name}: snoozed until when?");
        }
        // Nothing sits in Running with nothing provably on it.
        if d.place == Place::Running {
            let beginning = d.acts.iter().any(|a| matches!(a, Act::StallBegin { .. }));
            assert!(something_is_on_it(&c, now) || beginning, "{name}: Running on nothing ({:?}, {:?})", d.status, d.why);
        }
        // Applied, it is stable: worked out again at once, the same place
        // and nothing more to do.
        c.apply(&d, now);
        let again = c.at(now);
        assert_eq!(again.place, d.place, "{name}: {:?} then {:?}", d.status, again.status);
        assert!(
            !again.acts.iter().any(|a| matches!(a, Act::Raise(_) | Act::StallBegin { .. } | Act::Close { .. } | Act::Reopen)),
            "{name}: acts twice: {:?}",
            again.acts
        );
    }
}

#[test]
fn running_always_ends_by_itself() {
    // Whatever holds a card in Running without a live thread or turn runs
    // out: with nothing changing, time alone takes it out of Running.
    let now = mins(20);
    for (name, mut c) in every_case() {
        let live = c.threads.iter().any(|t| matches!(t.standing, Thread::Live { .. }) && !t.elsewhere);
        let turn = c.owner.as_ref().is_some_and(|o| o.live);
        let resuming = c.threads.iter().any(|t| t.resuming);
        if live || turn || resuming {
            continue;
        }
        let mut at = now;
        let mut d = c.step(at);
        let mut steps = 0;
        while d.place == Place::Running {
            let next = d.recheck.unwrap_or_else(|| panic!("{name}: Running ({:?}) with nothing to end it", d.status));
            assert!(next > at, "{name}: its recheck isn't ahead");
            at = next + TimeDelta::seconds(1);
            d = c.step(at);
            steps += 1;
            assert!(steps < 8, "{name}: still Running after {steps} deadlines");
        }
        assert!(at - now < TimeDelta::minutes(10), "{name}: Running for {}", at - now);
    }
}

#[test]
fn the_engines_asks_carry_their_answers() {
    let it = item();
    for (raise, labels) in [
        (Raise::Failed, vec![RETRY, DROP]),
        (Raise::Stalled, vec![CONTINUE, CLOSE, DROP]),
        (Raise::Review(Some("PR #7".into())), vec![LOOKS_GOOD]),
    ] {
        let (question, options, context) = raised_question(&it, &raise);
        assert!(question.contains(&it.title));
        assert_eq!(options.iter().map(|(l, _)| *l).collect::<Vec<_>>(), labels);
        assert_eq!(context.is_some(), matches!(raise, Raise::Review(Some(_))));
    }
}

// ── From the log ────────────────────────────────────────────────────────────
//
// Cards the old board check corrected on the dev engine between Nov 17 and
// Nov 22 (ids and titles as logged; `log-cases/cases.md` has each card's
// whole story). What the old check flagged or moved, replayed as facts.

/// Every act the engine took over a replay.
fn acts_of(steps: &[Derived]) -> Vec<&Act> {
    steps.iter().flat_map(|d| d.acts.iter()).collect()
}

/// `8fba58f3` "Admin app redesign": its ask answered at 10:16, its thread
/// woken at 10:23 and done by 10:24 with no report. The old check flagged it
/// stale twice, moved it to Review after ten minutes, and flagged that as
/// "nothing to review" two seconds later (15 of 15 such moves were).
#[test]
fn log_a_thread_that_finishes_silently_is_checked_once_and_never_put_up_for_review() {
    let mut c = Case::with(live());
    c.row.finish = Finish::of(FinishKind::Merged);
    let mut steps = vec![c.step(mins(23))];
    assert_eq!((steps[0].place, steps[0].status), (Place::Running, CardStatus::Working));
    // 10:24: its turn ended, nothing reported.
    c.threads = vec![finished(mins(24))];
    for at in [mins(24) + TimeDelta::seconds(4), mins(24) + TimeDelta::seconds(41)] {
        let d = c.step(at);
        assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking), "no flag, no move");
        assert!(d.acts.is_empty());
        steps.push(d);
    }
    // Still nothing: its orchestrator is woken once, then the person asked.
    let woke = mins(24) + TimeDelta::seconds(STALL_SETTLE_SECS + 1);
    steps.push(c.step(woke));
    c.owner = Some(Owner { started_at: Some(woke + TimeDelta::seconds(3)), ended_at: Some(woke + TimeDelta::seconds(50)), ..owner() });
    steps.push(c.step(woke + TimeDelta::seconds(51)));
    steps.push(c.step(woke + TimeDelta::seconds(50 + STALL_ASK_SECS + 1)));
    // Ten minutes on (when the old check moved it to Review): where it was.
    steps.push(c.step(mins(35)));
    steps.push(c.step(mins(36)));
    let last = steps.last().unwrap();
    assert_eq!((last.place, last.status), (Place::NeedsYou, CardStatus::Stalled));
    let acts = acts_of(&steps);
    assert_eq!(acts.iter().filter(|a| matches!(a, Act::StallBegin { wake: true })).count(), 1, "one wake");
    assert_eq!(acts.iter().filter(|a| matches!(a, Act::Raise(Raise::Stalled))).count(), 1, "one question");
    assert!(steps.iter().all(|d| d.status != CardStatus::Review), "never put up for review with nothing to review");
}

/// `c95f9dd9` "Release 0ae3dabe2: test, build, relaunch": flagged stale at
/// 11:11:16 and cleared 24 s later, its thread between two turns (37 of 47
/// stale flags cleared within two minutes like this).
#[test]
fn log_a_thread_between_two_turns_costs_no_wake() {
    let mut c = Case::with(finished(mins(10) + TimeDelta::seconds(16)));
    c.row.finish = Finish::of(FinishKind::Merged);
    let mut steps = Vec::new();
    for after in [5, 30, 60] {
        let d = c.step(mins(10) + TimeDelta::seconds(16 + after));
        assert_eq!(d.place, Place::Running, "{after}s after its turn");
        steps.push(d);
    }
    // 11:11:40: its next turn.
    c.threads = vec![ThreadFact { active_at: Some(mins(11) + TimeDelta::seconds(40)), ..thread(Thread::Live { started_at: Some(mins(11) + TimeDelta::seconds(40)) }) }];
    let d = c.step(mins(11) + TimeDelta::seconds(41));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Working));
    steps.push(d);
    assert!(acts_of(&steps).is_empty(), "nothing flagged, nobody woken: {:?}", acts_of(&steps));
}

/// `61253a3d` "Board stays honest": Done while its thread was in its last
/// turn; flagged "still working" and cleared two seconds later, five times
/// (all 24 such flags cleared within 20 s).
#[test]
fn log_a_done_card_whose_thread_finishes_its_last_turn_stays_done_quietly() {
    let mut c = Case::with(live());
    c.row.closed = Some(Closed { at: mins(15), by: Mover::Engine, how: CloseHow::Report, note: None });
    let steps = vec![c.step(mins(15) + TimeDelta::seconds(58)), {
        c.threads = vec![finished(mins(16))];
        c.step(mins(16))
    }];
    assert!(steps.iter().all(|d| d.place == Place::Done));
    assert!(acts_of(&steps).is_empty());
}

/// `7149112b` "RU-entry relay": parked by its orchestrator, its thread woke
/// for ten seconds to answer a question; the old check pulled it into Now
/// and flagged it stale ten seconds later (21 of 42 such moves were flagged
/// again within five minutes).
#[test]
fn log_a_parked_cards_thread_answering_a_question_doesnt_swing_it() {
    let mut c = Case::with(finished(mins(0)));
    c.row.held = true;
    assert_eq!(c.step(mins(30)).status, CardStatus::Held);
    // 19:45:19: its thread is woken; 19:45:29: done answering.
    c.threads = vec![ThreadFact { active_at: Some(mins(45)), ..thread(Thread::Live { started_at: Some(mins(45)) }) }];
    let working = c.step(mins(45) + TimeDelta::seconds(2));
    assert_eq!((working.place, working.status), (Place::Running, CardStatus::Working), "true while it works");
    c.threads = vec![finished(mins(45) + TimeDelta::seconds(10))];
    let mut steps = vec![working];
    for after in [11, 60, 300, 3600] {
        steps.push(c.step(mins(45) + TimeDelta::seconds(after)));
    }
    let last = steps.last().unwrap();
    assert_eq!((last.place, last.status), (Place::Planned, CardStatus::Held), "parked again");
    assert!(acts_of(&steps).is_empty(), "no stall, no wake, no question: {:?}", acts_of(&steps));
}

/// `8b417990` "Deploy lane (control + nodes from main)": a standing thread
/// that waits between jobs, corrected 49 times. Each job: work sent, done
/// reported, closed; the next job opens it again.
#[test]
fn log_a_lane_card_runs_each_job_and_rests_done_between_them() {
    let mut c = Case::with(thread(Thread::Idle));
    c.row.work_at = None;
    let mut steps = Vec::new();
    for job in 0..3 {
        let start = mins(60 * job);
        // The orchestrator sends the job.
        c.row.work_at = Some(start);
        let d = c.step(start + TimeDelta::seconds(1));
        assert_eq!(d.place, Place::Running, "job {job}: on its way");
        assert_eq!(d.acts.contains(&Act::Reopen), job > 0, "job {job}");
        steps.push(d);
        c.threads = vec![ThreadFact { active_at: Some(start + TimeDelta::seconds(5)), ..thread(Thread::Live { started_at: Some(start + TimeDelta::seconds(5)) }) }];
        steps.push(c.step(start + TimeDelta::seconds(6)));
        // It reports done; its orchestrator's turn on the report runs.
        let done = start + TimeDelta::minutes(9);
        c.row.word = word(ReportState::Done, done);
        c.row.heard_at = Some(done);
        c.row.checking = Some(Checking { since: done, why: CheckWhy::Report });
        c.threads = vec![finished(done + TimeDelta::seconds(4))];
        c.owner = Some(Owner { live: true, started_at: Some(done + TimeDelta::seconds(15)), ..owner() });
        let d = c.step(done + TimeDelta::seconds(20));
        assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking));
        steps.push(d);
        c.owner = Some(Owner { started_at: Some(done + TimeDelta::seconds(15)), ended_at: Some(done + TimeDelta::seconds(40)), ..owner() });
        let d = c.step(done + TimeDelta::seconds(41));
        assert_eq!(d.place, Place::Done, "job {job}: done");
        steps.push(d);
        // Idle until the next job: done, however long.
        let d = c.step(done + TimeDelta::minutes(45));
        assert_eq!(d.place, Place::Done);
        assert!(d.acts.is_empty());
        steps.push(d);
    }
    let acts = acts_of(&steps);
    assert!(!acts.iter().any(|a| matches!(a, Act::StallBegin { .. } | Act::Raise(_))), "never stalled, never asked: {acts:?}");
}

/// `764f1ae0` "iOS: port Android's Live screen": corrected 34 times (stale,
/// review, nothing to review, done but still working), on a thread shared
/// with other cards (76 of the 93 corrected cards shared one). Its thread
/// working for a card given work later says nothing of this one.
#[test]
fn log_a_shared_thread_at_work_for_another_card_doesnt_move_this_one() {
    // Done: its thread moves on to the next card.
    let mut done = Case::with(ThreadFact { elsewhere: true, ..live() });
    done.row.closed = Some(Closed { at: mins(1), by: Mover::Engine, how: CloseHow::Report, note: None });
    let d = done.step(mins(30));
    assert_eq!(d.place, Place::Done);
    assert!(d.acts.is_empty());
    // Planned for the same busy thread, not started: planned.
    let mut planned = Case::with(ThreadFact { elsewhere: true, ..live() });
    planned.row.work_at = None;
    planned.item.created_at = mins(10);
    let d = planned.step(mins(30));
    assert_eq!((d.place, d.status), (Place::Planned, CardStatus::Next));
    assert!(d.acts.is_empty());
    // Waiting on the person: still theirs.
    let mut asked = Case::with(ThreadFact { elsewhere: true, ..live() });
    asked.asks = vec![ask("a1", AskOrigin::Tool, mins(2))];
    assert_eq!(asked.step(mins(30)).place, Place::NeedsYou);
}

/// `b679e3ab` "Release f953da19d": flagged stale at 21:16:15 as the engine
/// shut down and cleared by the start-up pass 49 s later (10 corrections
/// came during shutdowns, 31 within five minutes of a start).
#[test]
fn log_a_turn_cut_off_by_the_engines_own_stop_is_still_work() {
    let stop = mins(16) + TimeDelta::seconds(15);
    let mut c = Case::with(ThreadFact { resuming: true, ended_at: Some(stop), active_at: Some(stop), ..thread(Thread::Stopped { at: stop }) });
    c.row.work_at = Some(mins(-30));
    let mut steps = vec![c.step(mins(16) + TimeDelta::seconds(46)), c.step(mins(17))];
    assert!(steps.iter().all(|d| (d.place, d.status) == (Place::Running, CardStatus::Working)));
    // Its resumed turn.
    c.threads = vec![ThreadFact { active_at: Some(mins(17) + TimeDelta::seconds(5)), ..thread(Thread::Live { started_at: Some(mins(17) + TimeDelta::seconds(5)) }) }];
    steps.push(c.step(mins(17) + TimeDelta::seconds(6)));
    assert!(acts_of(&steps).is_empty(), "{:?}", acts_of(&steps));
}

/// Two cards whose thread was archived while they were in Now and brought
/// back three to six minutes later: the thread comes off the card (kept as
/// a link), and counts again once it works.
#[test]
fn log_an_archived_thread_brought_back_is_the_cards_again() {
    let mut c = Case::with(thread(Thread::Gone));
    let d = c.step(mins(30));
    assert!(d.acts.contains(&Act::DropThreads(vec!["t1".into()])));
    assert!(c.item.threads.is_empty() && c.item.links.iter().any(|l| l.kind == LinkKind::Chat && l.target == "t1"));
    // Unarchived and messaged: the chat it links works.
    c.threads = vec![ThreadFact { active_at: Some(mins(34)), ..thread(Thread::Live { started_at: Some(mins(34)) }) }];
    let d = c.step(mins(34) + TimeDelta::seconds(5));
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Working));
    assert!(c.row.stall.is_none(), "its stall is over");
}

/// Five cards whose asks were dismissed after the Nov 21 restart went to
/// Now; three were flagged stale 2.5 minutes later. A closed ask alone puts
/// nothing in Running for good: only work does.
#[test]
fn log_a_dismissed_ask_alone_doesnt_put_a_card_to_work() {
    let mut c = Case::with(finished(mins(-60)));
    c.row.finish = Finish::of(FinishKind::Merged);
    c.row.work_at = Some(mins(-90));
    let mut dismissed = ask("a1", AskOrigin::Tool, mins(-50));
    dismissed.state = AskState::Dismissed;
    dismissed.answered_at = Some(mins(0));
    c.asks = vec![dismissed];
    // Nothing works on it: the engine checks (its orchestrator woken once),
    // and with nothing still, asks.
    let d = c.step(mins(0) + TimeDelta::seconds(5));
    assert_eq!(d.acts, [Act::StallBegin { wake: true }]);
    assert_eq!((d.place, d.status), (Place::Running, CardStatus::Checking));
    let d = c.step(mins(0) + TimeDelta::seconds(CHECK_HANDOFF_SECS + 5));
    assert!(d.acts.contains(&Act::ClearChecking), "the wake never came: {:?}", d.acts);
    let d = c.step(mins(0) + TimeDelta::seconds(CHECK_HANDOFF_SECS + STALL_ASK_SECS + 10));
    assert_eq!((d.place, d.status), (Place::NeedsYou, CardStatus::Stalled));
}
