//! The board check's rules, one card at a time ([`assess`]), the hard rules
//! ([`agent_move_allowed`]) and the plan over a whole board ([`plan_from`]).

use std::collections::HashMap;

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{
    Ask, AskKind, AskState, CardCheck, CheckKind, LinkKind, OrchestrationState, Priority, WorkItem, WorkLink, WorkState,
};
use zeron_proto::{Chat, DoneStatus, NativeTurnOutcome, Session, SessionStatus};

use super::*;

fn t0() -> DateTime<Utc> {
    "2026-11-16T12:00:00Z".parse().unwrap()
}

fn mins(n: i64) -> DateTime<Utc> {
    t0() + TimeDelta::minutes(n)
}

fn card(state: WorkState) -> WorkItem {
    WorkItem {
        id: "c1".into(),
        orchestrator_id: Some("o1".into()),
        title: "Phase 1: ship".into(),
        detail: String::new(),
        state,
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
        moved_at: Some(t0()),
        moved_route_state: None,
        approved_at: None,
        paused_at: None,
        check: None,
        threads_at: None,
        flow: None,
        depends_on: Vec::new(),
    }
}

fn view<'a>(card: &'a WorkItem, thread: Thread) -> CardView<'a> {
    CardView {
        card,
        threads: card.threads.iter().map(|t| (t.clone(), thread)).collect(),
        open_ask: false,
        last_closed: None,
        routed: false,
        route_review: false,
        prs: vec![],
        active_at: None,
    }
}

fn live_at(at: DateTime<Utc>) -> Thread {
    Thread::Live { started_at: Some(at) }
}

fn check(kind: CheckKind, since: DateTime<Utc>) -> Option<CardCheck> {
    Some(CardCheck { kind, note: "n".into(), since, nudged_at: Some(since) })
}

// ── Hard rules ──────────────────────────────────────────────────────────────

#[test]
fn an_agent_blocks_only_with_a_question() {
    let now = card(WorkState::Active);
    let refused = agent_move_allowed(Some(&now), Some(WorkState::Blocked), false, false, false).unwrap_err();
    assert!(refused.contains("`ask`"), "says how: {refused}");
    assert!(agent_move_allowed(Some(&now), Some(WorkState::Blocked), false, false, true).is_ok(), "ask with the move");
    assert!(agent_move_allowed(Some(&now), Some(WorkState::Blocked), false, true, false).is_ok(), "an open ask already");
    assert!(agent_move_allowed(None, Some(WorkState::Blocked), false, false, false).is_err(), "a new card too");
    assert!(agent_move_allowed(None, Some(WorkState::Blocked), false, false, true).is_ok());
    assert!(agent_move_allowed(Some(&now), Some(WorkState::Review), false, false, false).is_ok());
    assert!(agent_move_allowed(Some(&now), None, false, false, false).is_ok(), "no state, no rule");
}

#[test]
fn an_agent_never_moves_a_paused_card() {
    let mut paused = card(WorkState::Blocked);
    paused.paused_at = Some(t0());
    for to in [WorkState::Active, WorkState::Done, WorkState::Planned, WorkState::Blocked] {
        let refused = agent_move_allowed(Some(&paused), Some(to), false, true, true).unwrap_err();
        assert!(refused.contains("paused by the person"), "{to:?}: {refused}");
    }
    assert!(agent_move_allowed(Some(&paused), None, false, false, false).is_ok(), "its outcome may change");
    // Taking its thread off would let that thread's messages run: refused.
    assert!(agent_move_allowed(Some(&paused), None, true, false, false).unwrap_err().contains("paused by the person"));
}

// ── Blocked ─────────────────────────────────────────────────────────────────

#[test]
fn blocked_with_an_open_ask_stays() {
    let c = card(WorkState::Blocked);
    let mut v = view(&c, Thread::Finished);
    v.open_ask = true;
    assert!(assess(&v, mins(30)).is_empty());
}

#[test]
fn blocked_never_asked_raises_an_ask_from_the_card() {
    let c = card(WorkState::Blocked);
    let v = assess(&view(&c, Thread::Finished), mins(1));
    assert_eq!(v.raise, Some(Raise::Blocked));
    assert_eq!(v.state, None, "stays Blocked, now asked");
    let ask = raised_ask(&c, Raise::Blocked);
    assert!(ask.question.contains("Phase 1: ship"));
}

#[test]
fn blocked_whose_thread_errored_asks_retry_or_drop() {
    let c = card(WorkState::Blocked);
    let v = assess(&view(&c, Thread::Errored { at: Some(mins(1)) }), mins(2));
    assert_eq!(v.raise, Some(Raise::Errored));
    let ask = raised_ask(&c, Raise::Errored);
    let labels: Vec<_> = ask.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, [RETRY, DROP]);
}

#[test]
fn blocked_whose_ask_was_answered_goes_back_to_work() {
    let c = card(WorkState::Blocked);
    let mut v = view(&c, Thread::Finished);
    v.last_closed = Some((mins(5), true));
    let out = assess(&v, mins(6));
    assert_eq!(out.state, Some(WorkState::Active), "its thread takes the answer");
    assert_eq!(out.check.unwrap().unwrap().kind, CheckKind::Moved);
    // No thread: Up next.
    let mut loose = card(WorkState::Blocked);
    loose.threads.clear();
    let mut v = view(&loose, Thread::Finished);
    v.last_closed = Some((mins(5), false));
    let out = assess(&v, mins(6));
    assert_eq!(out.state, Some(WorkState::Planned));
    assert!(out.check.unwrap().unwrap().note.contains("dismissed"));
}

#[test]
fn an_ask_closed_before_the_card_was_blocked_doesnt_count() {
    let mut c = card(WorkState::Blocked);
    c.moved_at = Some(mins(10));
    let mut v = view(&c, Thread::Finished);
    v.last_closed = Some((mins(5), true));
    assert_eq!(assess(&v, mins(11)).raise, Some(Raise::Blocked));
}

#[test]
fn a_loose_blocked_card_with_nobody_to_ask_goes_to_up_next() {
    let mut c = card(WorkState::Blocked);
    c.orchestrator_id = None;
    c.threads.clear();
    let v = assess(&view(&c, Thread::Finished), mins(1));
    assert_eq!(v.state, Some(WorkState::Planned));
    assert_eq!(v.raise, None);
}

// ── Paused ──────────────────────────────────────────────────────────────────

#[test]
fn paused_stays_paused_until_its_thread_works_again() {
    let mut c = card(WorkState::Blocked);
    c.paused_at = Some(mins(5));
    // No ask needed: it's the person's hold.
    assert!(assess(&view(&c, Thread::Stopped { at: mins(5) }), mins(60)).is_empty());
    // Still winding down from the stop (its turn started before the pause).
    assert!(assess(&view(&c, live_at(mins(1))), mins(6)).is_empty());
    // A turn started since (the person wrote in the thread).
    let v = assess(&view(&c, live_at(mins(7))), mins(8));
    assert_eq!(v.state, Some(WorkState::Active));
    assert_eq!(v.paused, Some(false));
}

// ── Now ─────────────────────────────────────────────────────────────────────

#[test]
fn now_with_a_live_thread_is_true() {
    let c = card(WorkState::Active);
    assert!(assess(&view(&c, live_at(mins(1))), mins(90)).is_empty());
    // A stale flag from before is cleared.
    let mut c = card(WorkState::Active);
    c.check = check(CheckKind::Stale, mins(1));
    assert_eq!(assess(&view(&c, live_at(mins(2))), mins(3)).check, Some(None));
}

#[test]
fn now_whose_thread_finished_is_flagged_then_reviewed_after_ten_minutes() {
    let c = card(WorkState::Active);
    let first = assess(&view(&c, Thread::Finished), mins(3));
    assert_eq!(first.state, None);
    assert_eq!(first.check.as_ref().unwrap().as_ref().unwrap().kind, CheckKind::Stale);
    assert!(first.nudge, "its orchestrator is told");
    let mut flagged = card(WorkState::Active);
    flagged.check = check(CheckKind::Stale, mins(1));
    assert!(assess(&view(&flagged, Thread::Finished), mins(10)).is_empty(), "told once, waits");
    let late = assess(&view(&flagged, Thread::Finished), mins(11));
    assert_eq!(late.state, Some(WorkState::Review));
    assert!(!late.nudge);
}

#[test]
fn now_waiting_on_the_persons_answer_is_true() {
    let mut c = card(WorkState::Active);
    c.check = check(CheckKind::Stale, mins(1));
    let mut v = view(&c, Thread::Finished);
    v.open_ask = true;
    assert_eq!(assess(&v, mins(30)), Verdict { check: Some(None), ..Default::default() }, "no stale flag, no Review");
}

#[test]
fn now_whose_thread_errored_is_blocked_with_an_ask() {
    let c = card(WorkState::Active);
    let v = assess(&view(&c, Thread::Errored { at: Some(mins(2)) }), mins(3));
    assert_eq!(v.state, Some(WorkState::Blocked));
    assert_eq!(v.raise, Some(Raise::Errored));
    // An error from before it came into Now is old news: just finished.
    let mut c = card(WorkState::Active);
    c.moved_at = Some(mins(5));
    let v = assess(&view(&c, Thread::Errored { at: Some(mins(2)) }), mins(8));
    assert_eq!(v.state, None);
    assert_eq!(v.check.unwrap().unwrap().kind, CheckKind::Stale);
}

#[test]
fn now_whose_thread_was_stopped_is_paused() {
    let c = card(WorkState::Active);
    let v = assess(&view(&c, Thread::Stopped { at: mins(2) }), mins(3));
    assert_eq!(v.state, Some(WorkState::Blocked));
    assert_eq!(v.paused, Some(true));
}

#[test]
fn now_whose_thread_was_archived_after_its_work_is_stale() {
    let c = card(WorkState::Active);
    let v = assess(&view(&c, Thread::Gone), mins(3));
    assert_eq!(v.drop_threads, ["t1"], "off the card, kept as a link");
    assert_eq!(v.check.unwrap().unwrap().kind, CheckKind::Stale);
    // Already unlinked by the crew archive (a chat link left).
    let mut c = card(WorkState::Active);
    c.threads.clear();
    c.links.push(WorkLink { kind: LinkKind::Chat, label: "x".into(), target: "t1".into() });
    assert_eq!(assess(&view(&c, Thread::Gone), mins(3)).check.unwrap().unwrap().kind, CheckKind::Stale);
}

#[test]
fn now_with_no_thread_goes_back_to_up_next_after_its_grace() {
    let mut c = card(WorkState::Active);
    c.threads.clear();
    assert!(assess(&view(&c, Thread::Idle), mins(1)).is_empty(), "the orchestrator starts one right after");
    let v = assess(&view(&c, Thread::Idle), mins(3));
    assert_eq!(v.state, Some(WorkState::Planned));
    // A thread made but never run: the same.
    let c = card(WorkState::Active);
    assert_eq!(assess(&view(&c, Thread::Idle), mins(3)).state, Some(WorkState::Planned));
    // A routed card with no thread is its route's.
    let mut c = card(WorkState::Active);
    c.threads.clear();
    let mut v = view(&c, Thread::Idle);
    v.routed = true;
    assert!(assess(&v, mins(30)).is_empty());
}

// ── Up next, Later, Review, Done ────────────────────────────────────────────

#[test]
fn up_next_or_later_whose_thread_works_is_now() {
    for state in [WorkState::Planned, WorkState::Inbox] {
        let c = card(state);
        assert_eq!(assess(&view(&c, live_at(mins(2))), mins(3)).state, Some(WorkState::Active), "{state:?}");
        assert!(assess(&view(&c, Thread::Finished), mins(3)).is_empty());
        // At work since before it was moved here (moved out by hand).
        let mut moved = card(state);
        moved.moved_at = Some(mins(5));
        assert!(assess(&view(&moved, live_at(mins(2))), mins(6)).is_empty());
    }
}

#[test]
fn review_with_nothing_to_review_is_flagged_once() {
    let c = card(WorkState::Review);
    let v = assess(&view(&c, Thread::Finished), mins(1));
    assert_eq!(v.check.unwrap().unwrap().kind, CheckKind::NothingToReview);
    assert!(v.nudge);
    let mut flagged = card(WorkState::Review);
    flagged.check = check(CheckKind::NothingToReview, mins(1));
    assert!(assess(&view(&flagged, Thread::Finished), mins(2)).is_empty(), "told once");
    // An outcome alone isn't something to review (shipped, left in Review).
    let mut shipped = card(WorkState::Review);
    shipped.outcome = Some("Merged locally into main".into());
    assert_eq!(assess(&view(&shipped, Thread::Finished), mins(1)).check.unwrap().unwrap().kind, CheckKind::NothingToReview);
    // Moved here by the stale rule with nothing for the person: flagged too.
    let mut moved = card(WorkState::Review);
    moved.check = check(CheckKind::Moved, mins(1));
    assert_eq!(assess(&view(&moved, Thread::Finished), mins(2)).check.unwrap().unwrap().kind, CheckKind::NothingToReview);
}

#[test]
fn review_means_an_open_ask_or_an_open_pr() {
    let mut flagged = card(WorkState::Review);
    flagged.check = check(CheckKind::NothingToReview, mins(1));
    // An open ask.
    let mut v = view(&flagged, Thread::Finished);
    v.open_ask = true;
    assert_eq!(assess(&v, mins(2)).check, Some(None), "flag cleared");
    // An open PR, or one the app hasn't looked up.
    for pr in [Pr { open: Some(true), ci_failed: false }, Pr::default()] {
        let mut v = view(&flagged, Thread::Finished);
        v.prs = vec![pr];
        assert_eq!(assess(&v, mins(2)).check, Some(None), "{pr:?}");
    }
    // Its route's review line.
    let mut v = view(&flagged, Thread::Finished);
    v.route_review = true;
    assert_eq!(assess(&v, mins(2)).check, Some(None));
    // Its PR merged: nothing left.
    let c = card(WorkState::Review);
    let mut v = view(&c, Thread::Finished);
    v.prs = vec![Pr { open: Some(false), ci_failed: false }];
    let out = assess(&v, mins(1));
    assert!(out.check.unwrap().unwrap().note.contains("merged"));
}

#[test]
fn review_whose_pr_fails_ci_is_flagged_once() {
    let c = card(WorkState::Review);
    let mut v = view(&c, Thread::Finished);
    v.prs = vec![Pr { open: Some(true), ci_failed: true }];
    v.open_ask = true;
    let out = assess(&v, mins(1));
    assert_eq!(out.check.as_ref().unwrap().as_ref().unwrap().kind, CheckKind::CiFailing);
    assert!(out.nudge);
    let mut flagged = card(WorkState::Review);
    flagged.check = check(CheckKind::CiFailing, mins(1));
    let mut v = view(&flagged, Thread::Finished);
    v.prs = vec![Pr { open: Some(true), ci_failed: true }];
    assert!(assess(&v, mins(2)).is_empty());
    // Fixed: cleared.
    v.prs = vec![Pr { open: Some(true), ci_failed: false }];
    assert_eq!(assess(&v, mins(3)).check, Some(None));
}

#[test]
fn a_pr_link_reads_from_what_the_app_resolved() {
    let mut c = card(WorkState::Review);
    c.links = vec![
        WorkLink { kind: LinkKind::Pr, label: "PR".into(), target: "https://github.com/z/z/pull/12/".into() },
        WorkLink { kind: LinkKind::Pr, label: "PR".into(), target: "https://github.com/z/z/pull/13".into() },
        WorkLink { kind: LinkKind::Url, label: "doc".into(), target: "https://x".into() },
    ];
    let known: Vec<zeron_proto::ChangeRequestSummary> = vec![serde_json::from_value(serde_json::json!({
        "provider": "github", "number": 12, "title": "t", "url": "https://github.com/z/z/pull/12", "state": "open",
        "baseRef": "main", "headRef": "b", "signals": { "reviewDecision": "reviewRequired", "ci": "failed" }
    }))
    .unwrap()];
    assert_eq!(prs_of(&c, &known), [Pr { open: Some(true), ci_failed: true }, Pr::default()]);
}

#[test]
fn review_whose_thread_works_again_is_now() {
    let c = card(WorkState::Review);
    assert_eq!(assess(&view(&c, live_at(mins(2))), mins(3)).state, Some(WorkState::Active));
}

#[test]
fn done_with_a_thread_still_working_is_flagged_not_moved() {
    let c = card(WorkState::Done);
    let v = assess(&view(&c, live_at(mins(2))), mins(3));
    assert_eq!(v.state, None);
    assert_eq!(v.check.unwrap().unwrap().kind, CheckKind::StillWorking);
    let mut flagged = card(WorkState::Done);
    flagged.check = check(CheckKind::StillWorking, mins(3));
    assert_eq!(assess(&view(&flagged, Thread::Finished), mins(4)).check, Some(None));
}

#[test]
fn a_moved_note_stays_until_the_card_moves_again() {
    let mut c = card(WorkState::Planned);
    c.check = check(CheckKind::Moved, mins(1));
    assert!(assess(&view(&c, Thread::Finished), mins(30)).is_empty());
}

// ── Threads from sessions ───────────────────────────────────────────────────

fn chat(id: &str, device: &str) -> Chat {
    serde_json::from_value(serde_json::json!({
        "id": id, "deviceId": device, "createdAt": t0(), "updatedAt": t0(), "lastMessageAt": t0(), "archived": false,
    }))
    .unwrap()
}

fn session(chat: &str, status: SessionStatus) -> Session {
    Session {
        last_completed_turn: Some("t".into()),
        chat_id: chat.into(),
        device_id: "d1".into(),
        status,
        started_at: Some(mins(1)),
        updated_at: mins(1),
        last_turn_outcome: None,
        terminal_activity: None,
        running_subagents: 0,
        background_tasks: vec![],
        waiting: None,
        activity: None,
        todo: None,
    }
}

#[test]
fn a_thread_reads_from_its_session() {
    let c = chat("t1", "d1");
    assert_eq!(Thread::of(None, None, None, false, mins(1)), Thread::Gone);
    let mut archived = c.clone();
    archived.archived = true;
    assert_eq!(Thread::of(Some(&archived), None, None, false, mins(1)), Thread::Gone);
    assert_eq!(Thread::of(Some(&c), None, None, false, mins(1)), Thread::Idle);
    assert!(Thread::of(Some(&c), Some(&session("t1", SessionStatus::Working)), None, false, mins(1)).live());
    assert!(Thread::of(Some(&c), Some(&session("t1", SessionStatus::AwaitingInput)), None, false, mins(1)).live());
    let mut bg = session("t1", SessionStatus::Idle);
    bg.running_subagents = 1;
    assert!(Thread::of(Some(&c), Some(&bg), None, false, mins(1)).live(), "held on its subagents");
    assert!(!Thread::of(Some(&c), Some(&bg), None, false, mins(5)).live(), "its engine gone (no heartbeat)");
    let mut shell = session("t1", SessionStatus::Idle);
    shell.background_tasks =
        vec![serde_json::from_value(serde_json::json!({"id": "b1", "command": "npm run dev", "startedAt": mins(1)})).unwrap()];
    assert_eq!(Thread::of(Some(&c), Some(&shell), None, false, mins(1)), Thread::Finished, "a dev server left running isn't work");
    let working = session("t1", SessionStatus::Working);
    assert!(!Thread::of(Some(&c), Some(&working), None, false, mins(5)).live(), "a Working row nobody refreshed");
    assert_eq!(Thread::of(Some(&c), Some(&session("t1", SessionStatus::Idle)), None, false, mins(1)), Thread::Finished);
    assert!(matches!(Thread::of(Some(&c), Some(&session("t1", SessionStatus::Errored)), None, false, mins(1)), Thread::Errored { .. }));
    let mut stopped = session("t1", SessionStatus::Idle);
    stopped.last_turn_outcome = Some(NativeTurnOutcome { status: DoneStatus::Interrupted, finished_at: mins(2) });
    assert_eq!(Thread::of(Some(&c), Some(&stopped), None, false, mins(3)), Thread::Stopped { at: mins(2) });
}

// ── A whole board ───────────────────────────────────────────────────────────

fn ask(id: &str, item: &str, state: AskState, at: DateTime<Utc>) -> Ask {
    serde_json::from_value(serde_json::json!({
        "id": id, "fromChat": "o1", "itemId": item, "kind": "question", "question": "q?",
        "state": state, "createdAt": at, "answeredAt": (state != AskState::Open).then_some(at),
    }))
    .unwrap()
}

#[test]
fn the_plan_covers_the_boards_this_device_keeps() {
    let mut state = OrchestrationState::default();
    let mut blocked = card(WorkState::Blocked);
    blocked.id = "blocked".into();
    let mut asked = card(WorkState::Blocked);
    asked.id = "asked".into();
    let mut stale = card(WorkState::Active);
    stale.id = "stale".into();
    let mut theirs = card(WorkState::Blocked);
    theirs.id = "theirs".into();
    theirs.orchestrator_id = Some("o2".into());
    state.items = vec![blocked, asked, stale, theirs];
    state.asks = vec![ask("a1", "asked", AskState::Open, mins(1))];
    let chats: HashMap<String, Chat> =
        [chat("o1", "here"), chat("o2", "elsewhere"), chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let sessions: HashMap<String, Session> = [("t1".to_owned(), session("t1", SessionStatus::Idle))].into();
    let plan = plan_from(&state, &chats, &sessions, &Live::default(), &[], Some("here"), mins(3));
    let by: HashMap<&str, &Change> = plan.iter().map(|c| (c.item.as_str(), c)).collect();
    assert_eq!(by["blocked"].verdict.raise, Some(Raise::Blocked));
    assert!(!by.contains_key("asked"), "its ask is open");
    assert_eq!(by["stale"].verdict.check.as_ref().unwrap().as_ref().unwrap().kind, CheckKind::Stale);
    assert!(!by.contains_key("theirs"), "kept by the device hosting o2");
    // A dry run over every board.
    let all = plan_from(&state, &chats, &sessions, &Live::default(), &[], None, mins(3));
    assert!(all.iter().any(|c| c.item == "theirs"));
    // Shown as a person reads it.
    assert!(by["blocked"].to_string().contains("ask raised"));
}

#[test]
fn a_thread_on_two_open_cards_pulls_neither_out_of_up_next() {
    let mut state = OrchestrationState::default();
    let mut a = card(WorkState::Planned);
    a.id = "a".into();
    let mut b = card(WorkState::Planned);
    b.id = "b".into();
    let mut now = card(WorkState::Active);
    now.id = "now".into();
    state.items = vec![a, b, now];
    let chats: HashMap<String, Chat> = [chat("o1", "here"), chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let mut working = session("t1", SessionStatus::Working);
    working.started_at = Some(mins(3));
    working.updated_at = mins(4);
    let sessions: HashMap<String, Session> = [("t1".to_owned(), working)].into();
    let plan = plan_from(&state, &chats, &sessions, &Live::default(), &[], Some("here"), mins(4));
    assert!(plan.is_empty(), "{plan:?}");
    // Nor flags a Done card it worked on before.
    let mut done = card(WorkState::Done);
    done.id = "done".into();
    state.items = vec![done.clone(), card(WorkState::Active)];
    assert!(plan_from(&state, &chats, &sessions, &Live::default(), &[], Some("here"), mins(4)).is_empty());
    // Alone on its Done card, it is flagged.
    state.items = vec![done];
    let plan = plan_from(&state, &chats, &sessions, &Live::default(), &[], Some("here"), mins(4));
    assert_eq!(plan[0].verdict.check.as_ref().unwrap().as_ref().unwrap().kind, CheckKind::StillWorking);
}

#[test]
fn the_nudge_names_each_card() {
    let text = nudge_text(&[("c1", "Phase 2", "Its thread finished; still in Now")]);
    assert!(text.starts_with("[Board check]"));
    assert!(text.contains("\"Phase 2\" (c1): Its thread finished"));
}

#[test]
fn a_kind_is_spelled_for_the_wire() {
    assert_eq!(serde_json::to_value(CheckKind::NothingToReview).unwrap(), "nothingToReview");
    let _ = AskKind::Question;
}


// ── After a restart ─────────────────────────────────────────────────────────

fn crew_state(cards: Vec<WorkItem>) -> OrchestrationState {
    let mut state = OrchestrationState::default();
    state.crew = vec![serde_json::from_value(serde_json::json!({
        "chatId": "t1", "orchestratorId": "o1", "role": "", "joinedAt": t0(),
    }))
    .unwrap()];
    state.items = cards;
    state
}

#[test]
fn a_crew_thread_at_work_when_the_engine_stopped_is_resumed() {
    let chats: HashMap<String, Chat> = [chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let state = crew_state(vec![card(WorkState::Active)]);
    let at = mins(30);
    let none = HashMap::new();
    let idle_now: HashMap<String, Session> = [("t1".to_owned(), session("t1", SessionStatus::Idle))].into();
    // Mid-turn, or waiting on its own work, or running subagents: resumed.
    let working = session("t1", SessionStatus::Working);
    assert_eq!(ids(to_resume(&[working.clone()], &[], &none, &state, &chats, at)), ["t1"]);
    assert_eq!(ids(to_resume(&[working.clone()], &[], &idle_now, &state, &chats, at)), ["t1"], "its row set idle since");
    let stale_working: HashMap<String, Session> = [("t1".to_owned(), working.clone())].into();
    assert_eq!(ids(to_resume(&[working.clone()], &[], &stale_working, &state, &chats, at)), ["t1"], "the old row, no heartbeat");
    let mut waiting = session("t1", SessionStatus::Idle);
    waiting.waiting = Some(serde_json::from_value(serde_json::json!({"since": mins(1), "on": []})).unwrap());
    assert_eq!(ids(to_resume(&[waiting], &[], &none, &state, &chats, at)), ["t1"]);
    let mut subagents = session("t1", SessionStatus::Idle);
    subagents.running_subagents = 2;
    assert_eq!(ids(to_resume(&[subagents], &[], &none, &state, &chats, at)), ["t1"]);
    // Its last turn ended normally, or it waits on the person: nothing to pick up.
    assert!(to_resume(&[session("t1", SessionStatus::Idle)], &[], &none, &state, &chats, at).is_empty());
    assert!(to_resume(&[session("t1", SessionStatus::AwaitingInput)], &[], &none, &state, &chats, at).is_empty());
}

#[test]
fn a_thread_already_picked_up_paused_or_not_crew_is_not_resumed() {
    let chats: HashMap<String, Chat> = [chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let working = session("t1", SessionStatus::Working);
    let at = mins(30);
    let state = crew_state(vec![card(WorkState::Active)]);
    // Revived by the engine's own recovery: in flight now, or a turn done since.
    let mut live = session("t1", SessionStatus::Working);
    live.updated_at = at;
    let now: HashMap<String, Session> = [("t1".to_owned(), live)].into();
    assert!(ids(to_resume(&[working.clone()], &[], &now, &state, &chats, at)).is_empty());
    let mut later = session("t1", SessionStatus::Idle);
    later.last_completed_turn = Some("t2".into());
    let now: HashMap<String, Session> = [("t1".to_owned(), later)].into();
    assert!(ids(to_resume(&[working.clone()], &[], &now, &state, &chats, at)).is_empty());
    // Its card paused by the person.
    let mut paused = card(WorkState::Blocked);
    paused.paused_at = Some(mins(1));
    assert!(to_resume(&[working.clone()], &[], &HashMap::new(), &crew_state(vec![paused]), &chats, at).is_empty());
    // Not in a crew, or archived.
    assert!(to_resume(&[working.clone()], &[], &HashMap::new(), &OrchestrationState::default(), &chats, at).is_empty());
    let mut archived = chat("t1", "here");
    archived.archived = true;
    let gone: HashMap<String, Chat> = [("t1".to_owned(), archived)].into();
    assert!(to_resume(&[working], &[], &HashMap::new(), &state, &gone, at).is_empty());
}


fn ids(list: Vec<Resume>) -> Vec<String> {
    list.into_iter().map(|r| r.chat).collect()
}

#[test]
fn a_chat_whose_wait_the_shutdown_ended_is_resumed_crew_or_not() {
    // Its turn completed into a wait on its own build; the shutdown ended
    // the build and the wait, so its row by the next boot says Idle.
    let chats: HashMap<String, Chat> =
        [chat("t1", "here"), chat("solo", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let state = crew_state(vec![card(WorkState::Active)]);
    let at = mins(30);
    let rows = [session("t1", SessionStatus::Idle), session("solo", SessionStatus::Idle)];
    assert!(to_resume(&rows, &[], &HashMap::new(), &state, &chats, at).is_empty(), "ended normally, as its row reads");
    let waits = vec![
        ("solo".to_owned(), vec!["cargo build -p zeron-engine".to_owned()]),
        ("t1".to_owned(), vec!["deploy build".to_owned()]),
    ];
    let out = to_resume(&rows, &waits, &HashMap::new(), &state, &chats, at);
    assert_eq!(
        out,
        [
            Resume { chat: "solo".into(), waited_on: vec!["cargo build -p zeron-engine".into()] },
            Resume { chat: "t1".into(), waited_on: vec!["deploy build".into()] },
        ],
        "a standalone chat too"
    );
    // A standalone chat that was mid-turn is the engine's own recovery's.
    let mut mid = session("solo", SessionStatus::Working);
    mid.last_completed_turn = None;
    assert!(to_resume(&[mid], &[], &HashMap::new(), &state, &chats, at).is_empty());
    // Ran since (picked up by something else): not again.
    let mut ran = session("solo", SessionStatus::Idle);
    ran.last_completed_turn = Some("t9".into());
    let now: HashMap<String, Session> = [("solo".to_owned(), ran)].into();
    assert!(ids(to_resume(&rows, &waits, &now, &state, &chats, at)) == ["t1"]);
}

#[test]
fn the_resume_says_what_the_restart_stopped() {
    use zeron_proto::orchestration::prompt_text::{RESUMED_AFTER_RESTART, resumed_after_restart};
    assert_eq!(resumed_after_restart(&[]), RESUMED_AFTER_RESTART);
    let text = resumed_after_restart(&["cargo build -p zeron-engine".into()]);
    assert!(text.starts_with(RESUMED_AFTER_RESTART));
    assert!(text.contains("(cargo build -p zeron-engine)") && text.contains("run it again"), "{text}");
}


// ── Asks the board check raised close themselves ───────────────────────────

fn board_ask(id: &str, item: &str) -> Ask {
    let mut a = ask(id, item, AskState::Open, mins(1));
    a.origin = zeron_proto::orchestration::AskOrigin::BoardCheck;
    a
}

#[test]
fn a_board_ask_resolves_when_its_card_leaves_blocked_or_its_thread_works() {
    let blocked = card(WorkState::Blocked);
    let asks = vec![board_ask("b1", "c1"), ask("a1", "c1", AskState::Open, mins(1))];
    // Still Blocked, its thread errored: stays open.
    let errored = vec![("t1".to_owned(), Thread::Errored { at: Some(mins(1)) })];
    assert!(resolved_asks(&blocked, &errored, &asks).is_empty());
    // Its thread works again.
    let live = vec![("t1".to_owned(), live_at(mins(5)))];
    assert_eq!(resolved_asks(&blocked, &live, &asks), [("b1".to_owned(), "its thread is working again".to_owned())]);
    // Moved out of Blocked (by the person).
    let now = card(WorkState::Active);
    assert_eq!(resolved_asks(&now, &errored, &asks), [("b1".to_owned(), "the card left Blocked".to_owned())]);
    // An agent's or the person's ask (a1) is never the check's to close; nor
    // one on another card, nor one closed already.
    let others = vec![ask("a1", "c1", AskState::Open, mins(1)), board_ask("b2", "other"), {
        let mut a = board_ask("b3", "c1");
        a.state = AskState::Answered;
        a
    }];
    assert!(resolved_asks(&now, &live, &others).is_empty());
}

#[test]
fn the_plan_closes_a_board_ask_whose_card_was_moved_back_to_now() {
    let mut state = OrchestrationState::default();
    state.items = vec![card(WorkState::Active)];
    state.asks = vec![board_ask("b1", "c1")];
    let chats: HashMap<String, Chat> = [chat("o1", "here"), chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let mut working = session("t1", SessionStatus::Working);
    working.updated_at = mins(4);
    let sessions: HashMap<String, Session> = [("t1".to_owned(), working)].into();
    let plan = plan_from(&state, &chats, &sessions, &Live::default(), &[], Some("here"), mins(4));
    assert_eq!(plan.len(), 1, "{plan:?}");
    assert_eq!(plan[0].verdict.resolve, [("b1".to_owned(), "the card left Blocked".to_owned())]);
    assert_eq!(plan[0].verdict.state, None, "the card itself is fine");
}

// ── Which card a live thread works on ───────────────────────────────────────

#[test]
fn a_thread_on_a_follow_up_card_is_that_cards_not_its_done_ones() {
    // The crew row still names the card it was started on (now Done); the
    // follow-up card lists it in its threads: that's where it works.
    let mut done = card(WorkState::Done);
    done.id = "61253a3d".into();
    let mut follow_up = card(WorkState::Active);
    follow_up.id = "10f479ef".into();
    let mut state = crew_state(vec![done.clone(), follow_up]);
    state.crew[0].item_id = Some("61253a3d".into());
    let chats: HashMap<String, Chat> = [chat("o1", "here"), chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let mut working = session("t1", SessionStatus::Working);
    working.started_at = Some(mins(3));
    working.updated_at = mins(4);
    let sessions: HashMap<String, Session> = [("t1".to_owned(), working)].into();
    let plan = plan_from(&state, &chats, &sessions, &Live::default(), &[], Some("here"), mins(4));
    assert!(plan.is_empty(), "no 'Done, but its thread is still working': {plan:?}");
    // A flag set before the follow-up listed it is cleared.
    let mut flagged = done;
    flagged.check = check(CheckKind::StillWorking, mins(2));
    state.items[0] = flagged;
    let plan = plan_from(&state, &chats, &sessions, &Live::default(), &[], Some("here"), mins(4));
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].item, "61253a3d");
    assert_eq!(plan[0].verdict.check, Some(None));
}


// ── Stale: checked again as it falls due; a thread about to start ──────────

fn progress_row(chat: &str, state: zeron_proto::orchestration::ProgressState, step: DateTime<Utc>) -> zeron_proto::orchestration::Progress {
    serde_json::from_value(serde_json::json!({
        "chatId": chat, "deviceId": "elsewhere", "state": state, "startedAt": mins(12), "lastStepAt": step,
        "updatedAt": step,
    }))
    .unwrap()
}

#[test]
fn a_long_tool_call_mid_turn_is_work() {
    use zeron_proto::orchestration::ProgressState;
    let c = chat("t1", "here");
    // Mid-turn on this engine, a tool silent for minutes: its own run is the
    // truth, whatever the row's stamp.
    let mut quiet = session("t1", SessionStatus::Working);
    quiet.updated_at = mins(20);
    assert!(Thread::of(Some(&c), Some(&quiet), None, true, mins(30)).live(), "this engine runs it");
    assert!(!Thread::of(Some(&c), Some(&quiet), None, false, mins(30)).live(), "another's row, no heartbeat");
    // Hosted elsewhere, its row late or idle: its progress row (what
    // `progress` reads) says working, a tool running, last step 26 s ago.
    let mut tool = progress_row("t1", ProgressState::Working, mins(22));
    tool.tool_since = Some(mins(22));
    let idle_row = session("t1", SessionStatus::Idle);
    assert!(Thread::of(Some(&c), Some(&idle_row), Some(&tool), false, mins(30)).live(), "a tool running");
    assert!(Thread::of(Some(&c), Some(&quiet), Some(&tool), false, mins(30)).live());
    assert!(Thread::of(Some(&c), None, Some(&tool), false, mins(30)).live(), "no row yet");
    // A step within ten minutes counts; an old one, or a finished state, doesn't.
    let recent = progress_row("t1", ProgressState::Working, mins(25));
    assert!(Thread::of(Some(&c), Some(&idle_row), Some(&recent), false, mins(30)).live());
    let old = progress_row("t1", ProgressState::Working, mins(1));
    assert!(!Thread::of(Some(&c), Some(&idle_row), Some(&old), false, mins(30)).live());
    let done = progress_row("t1", ProgressState::Done, mins(29));
    assert!(!Thread::of(Some(&c), Some(&idle_row), Some(&done), false, mins(30)).live());
}

#[test]
fn a_stale_flag_whose_thread_worked_since_is_cleared_not_moved() {
    // Flagged at 11:22 (thread idle); it worked from 11:22:15. At the
    // ten-minute mark it is idle again in this reading, yet it did work
    // since the flag: the flag goes, the card stays.
    let mut c = card(WorkState::Active);
    c.check = check(CheckKind::Stale, mins(1));
    let mut v = view(&c, Thread::Finished);
    v.active_at = Some(mins(2));
    assert_eq!(assess(&v, mins(12)), Verdict { check: Some(None), ..Default::default() });
    // Nothing since the flag: Review at ten minutes, as before.
    v.active_at = Some(mins(0));
    assert_eq!(assess(&v, mins(12)).state, Some(WorkState::Review));
}

#[test]
fn a_card_just_made_or_given_a_thread_is_not_flagged_yet() {
    // Made at t0 with its thread not started yet.
    let c = card(WorkState::Active);
    assert!(assess(&view(&c, Thread::Finished), mins(1)).is_empty(), "its thread is about to start");
    assert_eq!(assess(&view(&c, Thread::Finished), mins(3)).check.unwrap().unwrap().kind, CheckKind::Stale);
    // An older card given a thread just now.
    let mut given = card(WorkState::Active);
    given.threads_at = Some(mins(10));
    assert!(assess(&view(&given, Thread::Finished), mins(11)).is_empty());
    assert_eq!(assess(&view(&given, Thread::Finished), mins(13)).check.unwrap().unwrap().kind, CheckKind::Stale);
}

#[test]
fn the_plan_reads_a_thread_at_work_by_its_progress_row() {
    use zeron_proto::orchestration::ProgressState;
    // 5fd8f707: flagged stale while its thread was idle; its thread (hosted
    // elsewhere, its row late) has worked since, by `progress`.
    let mut flagged = card(WorkState::Active);
    flagged.check = check(CheckKind::Stale, mins(1));
    let mut state = OrchestrationState::default();
    state.items = vec![flagged];
    let chats: HashMap<String, Chat> = [chat("o1", "here"), chat("t1", "elsewhere")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let mut late = session("t1", SessionStatus::Idle);
    late.updated_at = mins(1);
    let sessions: HashMap<String, Session> = [("t1".to_owned(), late)].into();
    let mut live = Live::default();
    let mut tool = progress_row("t1", ProgressState::Working, mins(11));
    tool.tool_since = Some(mins(11));
    live.progress.insert("t1".into(), tool);
    let plan = plan_from(&state, &chats, &sessions, &live, &[], Some("here"), mins(12));
    assert_eq!(plan.len(), 1, "{plan:?}");
    assert_eq!(plan[0].verdict.state, None, "no move to Review");
    assert_eq!(plan[0].verdict.check, Some(None), "the flag cleared");
}


// ── An archived crew thread put back to work; a turn's gap ──────────────────

#[test]
fn an_archived_thread_at_work_is_live_and_one_just_given_stays_on_its_card() {
    use zeron_proto::orchestration::ProgressState;
    let mut archived = chat("t1", "here");
    archived.archived = true;
    // Messaged out of the archive: at work before its chat row says it's back.
    let mut working = session("t1", SessionStatus::Working);
    working.updated_at = mins(30);
    assert!(Thread::of(Some(&archived), Some(&working), None, true, mins(30)).live());
    let busy = progress_row("t1", ProgressState::Working, mins(29));
    assert!(Thread::of(Some(&archived), None, Some(&busy), false, mins(30)).live());
    assert_eq!(Thread::of(Some(&archived), Some(&session("t1", SessionStatus::Idle)), None, true, mins(30)), Thread::Gone);
    // 5fd8f707: given an archived thread at 11:22; not dropped, not flagged
    // while the message that brings it back is on its way.
    let mut c = card(WorkState::Active);
    c.threads_at = Some(mins(20));
    let v = assess(&view(&c, Thread::Gone), mins(21));
    assert!(v.is_empty(), "{v:?}");
    let v = assess(&view(&c, Thread::Gone), mins(23));
    assert_eq!(v.drop_threads, ["t1"], "archived and idle past the grace: gone");
}

#[test]
fn a_now_card_whose_unlinked_thread_works_again_is_true() {
    // Its thread was taken off it (archived), then messaged back to work on
    // it: the chat it links is at work.
    let mut c = card(WorkState::Active);
    c.threads.clear();
    c.links.push(WorkLink { kind: LinkKind::Chat, label: "x".into(), target: "t1".into() });
    c.check = check(CheckKind::Stale, mins(1));
    let mut state = OrchestrationState::default();
    state.items = vec![c];
    let chats: HashMap<String, Chat> = [chat("o1", "here"), chat("t1", "here")].into_iter().map(|c| (c.id.clone(), c)).collect();
    let mut working = session("t1", SessionStatus::Working);
    working.started_at = Some(mins(5));
    let sessions: HashMap<String, Session> = [("t1".to_owned(), working)].into();
    let live = Live { local: ["t1".to_owned()].into(), ..Default::default() };
    let plan = plan_from(&state, &chats, &sessions, &live, &[], Some("here"), mins(12));
    assert_eq!(plan.len(), 1, "{plan:?}");
    assert_eq!(plan[0].verdict.state, None, "no move to Review");
    assert_eq!(plan[0].verdict.check, Some(None), "its stale flag cleared");
}

#[test]
fn a_thread_waiting_on_its_own_work_or_between_turns_is_not_flagged() {
    let c = chat("t1", "here");
    // 2d358590: its turn ended into a wait on its own watcher; this engine
    // runs it, its stamp minutes old.
    let mut waiting = session("t1", SessionStatus::Idle);
    waiting.waiting = Some(serde_json::from_value(serde_json::json!({"since": mins(20), "on": []})).unwrap());
    waiting.updated_at = mins(20);
    assert!(Thread::of(Some(&c), Some(&waiting), None, true, mins(30)).live(), "its own wait");
    // A turn that ended a moment ago (its wait, or its next turn, about to
    // start): not flagged yet.
    let card = card(WorkState::Active);
    let mut v = view(&card, Thread::Finished);
    v.active_at = Some(mins(29));
    assert!(assess(&v, mins(30)).is_empty());
    v.active_at = Some(mins(27));
    assert_eq!(assess(&v, mins(30)).check.unwrap().unwrap().kind, CheckKind::Stale);
}
