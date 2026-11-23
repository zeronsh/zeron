//! How a card's thread stands ([`Thread::of`]), a card's PR links
//! ([`prs_of`]), what an agent may not change on a card
//! ([`agent_edit_allowed`]). Where a card is: `derive_tests.rs`; which
//! chats a restart resumes: `restart_resume_tests.rs`.

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{LinkKind, Priority, WorkItem, WorkLink, WorkState};
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

// ── Hard rules ──────────────────────────────────────────────────────────────

#[test]
fn an_agent_never_changes_a_paused_cards_threads() {
    let mut paused = card(WorkState::Blocked);
    paused.paused_at = Some(t0());
    assert!(agent_edit_allowed(Some(&paused), false).is_ok(), "its outcome may change");
    // Taking its thread off would let that thread's messages run: refused.
    assert!(agent_edit_allowed(Some(&paused), true).unwrap_err().contains("paused by the person"));
    let open = card(WorkState::Active);
    assert!(agent_edit_allowed(Some(&open), true).is_ok());
    assert!(agent_edit_allowed(None, true).is_ok(), "a new card");
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
        limited: None,
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

// ── After a restart ─────────────────────────────────────────────────────────

// ── Progress rows ───────────────────────────────────────────────────────────

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

// ── An archived crew thread put back to work; a thread's own wait ──────────

#[test]
fn an_archived_thread_at_work_is_live() {
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
}

#[test]
fn a_thread_waiting_on_its_own_work_is_at_work() {
    let c = chat("t1", "here");
    // 2d358590: its turn ended into a wait on its own watcher; this engine
    // runs it, its stamp minutes old.
    let mut waiting = session("t1", SessionStatus::Idle);
    waiting.waiting = Some(serde_json::from_value(serde_json::json!({"since": mins(20), "on": []})).unwrap());
    waiting.updated_at = mins(20);
    assert!(Thread::of(Some(&c), Some(&waiting), None, true, mins(30)).live(), "its own wait");
}

// ── Stopped by its usage limit ──────────────────────────────────────────────

fn limited(chat: &str, since: DateTime<Utc>) -> Session {
    Session {
        status: SessionStatus::Errored,
        started_at: None,
        updated_at: since,
        last_turn_outcome: Some(NativeTurnOutcome { status: DoneStatus::Errored, finished_at: since }),
        limited: Some(zeron_proto::usage_limit::SessionLimit {
            provider: "claude".into(),
            account_slot: None,
            account_label: None,
            window: Some("5-hour".into()),
            until: Some(since + TimeDelta::hours(2)),
            since,
            attempt: 0,
            switched_to: None,
        }),
        ..session(chat, SessionStatus::Errored)
    }
}

#[test]
fn a_thread_stopped_by_its_usage_limit_is_at_work() {
    let c = chat("t1", "d1");
    // Its row says errored, and stale hours later (no heartbeat): at work
    // all the same, here or on another device.
    let row = limited("t1", mins(2));
    for local in [true, false] {
        assert_eq!(Thread::of(Some(&c), Some(&row), None, local, mins(300)), Thread::Live { started_at: Some(mins(2)) });
    }
    // Archived, it's gone.
    let mut archived = c.clone();
    archived.archived = true;
    assert_eq!(Thread::of(Some(&archived), Some(&row), None, true, mins(300)), Thread::Gone);
}

