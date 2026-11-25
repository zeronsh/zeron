//! What `board.rs` works out beside the ladder: a card's row from its old
//! column ([`seed_row`]), who keeps a board ([`keeper_of`]), which of a
//! thread's cards it is at work for ([`at_work_elsewhere`]), whether a
//! branch has landed ([`merged_in`]), and what a move says ([`move_words`]).
//! The board end to end (ops, sessions, restarts) is in `tests.rs`; the
//! ladder itself in `derive_tests.rs`.

use chrono::{DateTime, Duration as TimeDelta, Utc};
use zeron_proto::orchestration::{AskOrigin, CloseHow, FinishKind, ReportHead, ReportState};

use super::super::derive::tests::{ask, item, t0};
use super::*;

fn old(state: WorkState) -> WorkItem {
    let mut i = item();
    i.state = state;
    i.threads_at = Some(t0() - TimeDelta::hours(3));
    i.created_at = t0() - TimeDelta::hours(4);
    i.updated_at = t0() - TimeDelta::hours(1);
    i
}

fn report(state: ReportState, at: DateTime<Utc>) -> (String, ReportHead) {
    ("t1".to_owned(), ReportHead { id: "r1".into(), state, summary: "Fixed and tested".into(), at })
}

// ── From the old column (the proposal's table) ──────────────────────────────

#[test]
fn every_old_column_maps_to_its_place() {
    let now = t0();
    let at = |i: &WorkItem, asks: &[Ask], prs: &[Pr]| {
        let row = seed_row(i, asks, prs, &[], now);
        (row.place, row.status)
    };
    let open = [ask("a1", AskOrigin::Tool, now)];
    // Later: planned, held (never starts on its own).
    let later = seed_row(&old(WorkState::Inbox), &[], &[], &[], now);
    assert_eq!((later.place, later.status, later.held), (Place::Planned, CardStatus::Held, true));
    // Up next: planned. With work behind it (a thread on it, or one
    // archived off it), parked by its orchestrator: held. Queued for a
    // crew slot: planned, to start when one frees.
    let mut fresh = old(WorkState::Planned);
    fresh.threads.clear();
    assert_eq!(at(&fresh, &[], &[]), (Place::Planned, CardStatus::Next));
    let parked = seed_row(&old(WorkState::Planned), &[], &[], &[], now);
    assert_eq!((parked.place, parked.status, parked.held), (Place::Planned, CardStatus::Held, true));
    let mut queued = fresh.clone();
    queued.waiting = Some("Auth's crew is full: 2 of 2 working".into());
    assert!(!seed_row(&queued, &[], &[], &[], now).held);
    // Now: running (then worked out like any card).
    let running = seed_row(&old(WorkState::Active), &[], &[], &[], now);
    assert_eq!((running.place, running.status), (Place::Running, CardStatus::Working));
    assert_eq!(running.work_at, Some(t0() - TimeDelta::hours(3)), "started when its thread was put on it");
    // Blocked: needs you (it must have an ask).
    assert_eq!(at(&old(WorkState::Blocked), &open, &[]), (Place::NeedsYou, CardStatus::Asked));
    // Paused: planned, "Paused by you".
    let mut paused = old(WorkState::Blocked);
    paused.paused_at = Some(now);
    assert_eq!(at(&paused, &[], &[]), (Place::Planned, CardStatus::Paused));
    // Review: needs you with an ask or an open PR; otherwise done.
    assert_eq!(at(&old(WorkState::Review), &open, &[]), (Place::NeedsYou, CardStatus::Review));
    let with_pr = seed_row(&old(WorkState::Review), &[], &[Pr { open: Some(true), ci_failed: false }], &[], now);
    assert_eq!((with_pr.place, with_pr.finish.line), (Place::NeedsYou, FinishKind::Pr), "its PR is its finish line");
    assert!(with_pr.closed.is_none());
    let merged = seed_row(&old(WorkState::Review), &[], &[Pr { open: Some(false), ci_failed: false }], &[], now);
    assert_eq!(merged.place, Place::Done, "its PR merged or closed: nothing left to look at");
    let nothing = seed_row(&old(WorkState::Review), &[], &[], &[], now);
    assert_eq!((nothing.place, nothing.closed.as_ref().map(|c| c.how)), (Place::Done, Some(CloseHow::Mapped)));
    // Done: done, when it was done, and nothing shown as new.
    let mut finished = old(WorkState::Done);
    finished.done_at = Some(t0() - TimeDelta::days(2));
    let done = seed_row(&finished, &[], &[], &[], now);
    assert_eq!((done.place, done.since), (Place::Done, t0() - TimeDelta::days(2)));
    assert_eq!(done.closed.as_ref().map(|c| c.at), Some(t0() - TimeDelta::days(2)));
    assert!(!done.unseen());
    // Approved by the person: theirs.
    finished.approved_at = Some(t0() - TimeDelta::days(2));
    let approved = seed_row(&finished, &[], &[], &[], now);
    assert_eq!(approved.closed.as_ref().map(|c| (c.how, c.by)), Some((CloseHow::Accepted, Mover::You)));
    // Dropped: off the board.
    assert_eq!(at(&old(WorkState::Dropped), &[], &[]), (Place::Dropped, CardStatus::Dropped));
    // A move it never made: no number.
    assert_eq!((done.seq, done.cause), (0, MoveCause::Mapped));
}

#[test]
fn a_mapped_card_keeps_its_threads_last_word() {
    // Its thread reported done long ago and nothing moved the card (the old
    // check's most frequent flag): the word is there for the ladder.
    let said = t0() - TimeDelta::hours(2);
    let row = seed_row(&old(WorkState::Active), &[], &[], &[report(ReportState::Done, said)], t0());
    let word = row.word.expect("its word");
    assert_eq!((word.state, word.at, word.from.as_str()), (ReportState::Done, said, "t1"));
    // Progress isn't a word on where it stands.
    assert!(seed_row(&old(WorkState::Active), &[], &[], &[report(ReportState::Progress, said)], t0()).word.is_none());
}

// ── Who keeps a board ───────────────────────────────────────────────────────

fn chat(id: &str, device: &str) -> (String, Chat) {
    let chat: Chat = serde_json::from_value(serde_json::json!({
        "id": id, "deviceId": device, "createdAt": t0(), "updatedAt": t0(), "lastMessageAt": t0(), "archived": false,
    }))
    .unwrap();
    (id.to_owned(), chat)
}

#[test]
fn a_board_is_kept_by_its_orchestrators_host_else_its_threads_else_its_dumps() {
    let all = |_: &str| true;
    let none = |_: &str| false;
    let chats: HashMap<String, Chat> = [chat("o1", "laptop"), chat("t1", "cloud")].into();
    let card = item();
    assert_eq!(keeper_of(&card, None, &chats, "laptop", all), Keeper::Here);
    assert_eq!(keeper_of(&card, None, &chats, "cloud", all), Keeper::Device("laptop".into()), "the orchestrator's host first");
    // No orchestrator: its first thread's host.
    let mut loose = item();
    loose.orchestrator_id = None;
    assert_eq!(keeper_of(&loose, Some("laptop"), &chats, "cloud", all), Keeper::Here);
    assert_eq!(keeper_of(&loose, Some("laptop"), &chats, "laptop", all), Keeper::Device("cloud".into()));
    // A host from before places is passed over for the next in line.
    assert_eq!(keeper_of(&card, None, &chats, "cloud", none), Keeper::Here, "the orchestrator's host can't; the thread's is here");
    assert_eq!(keeper_of(&loose, Some("laptop"), &chats, "phone", |d| d == "laptop"), Keeper::Device("laptop".into()));
    // None of them works places out: nobody keeps it.
    assert_eq!(keeper_of(&card, None, &chats, "phone", none), Keeper::Older);
    // No orchestrator, thread or dump at all: whoever runs an op on it.
    loose.threads.clear();
    assert_eq!(keeper_of(&loose, None, &chats, "phone", all), Keeper::Anyone);
}

// ── One thread, several cards ───────────────────────────────────────────────

#[test]
fn a_thread_works_for_the_card_last_given_work() {
    let (earlier, later) = (Some(t0()), Some(t0() + TimeDelta::minutes(5)));
    // Alone on its thread: its own.
    assert!(!at_work_elsewhere(earlier, "a", None));
    assert!(!at_work_elsewhere(None, "a", None));
    // Another open card was given work since: that one's.
    assert!(at_work_elsewhere(earlier, "a", Some(&(later, "b".into()))));
    assert!(!at_work_elsewhere(later, "b", Some(&(earlier, "a".into()))));
    // A card planned for a busy thread, never given work: not this one's.
    assert!(at_work_elsewhere(None, "planned", Some(&(earlier, "a".into()))));
    assert!(at_work_elsewhere(None, "planned", Some(&(None, "other".into()))));
    // The other never given work: this one's.
    assert!(!at_work_elsewhere(earlier, "a", Some(&(None, "planned".into()))));
    // The same moment: one of them, always the same one.
    assert_ne!(at_work_elsewhere(earlier, "a", Some(&(earlier, "b".into()))), at_work_elsewhere(earlier, "b", Some(&(earlier, "a".into()))));
}

// ── A branch landed ─────────────────────────────────────────────────────────

fn sh(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn a_branch_has_landed_when_its_own_work_is_in_its_target() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path();
    let dir = repo.to_str().unwrap();
    sh(repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    sh(repo, &["add", "."]);
    sh(repo, &["commit", "-q", "-m", "base"]);
    let made = Utc::now() - TimeDelta::seconds(5);
    sh(repo, &["branch", "work"]);
    // Nobody has committed to it yet: contained in main from the start,
    // which is nothing landing.
    let before_any_work = Utc::now() + TimeDelta::seconds(5);
    assert_eq!(merged_in(dir, "work", "main", before_any_work), None, "no work of the card's own");
    // Its own work, not merged yet.
    sh(repo, &["checkout", "-q", "work"]);
    std::fs::write(repo.join("b.txt"), "two\n").unwrap();
    sh(repo, &["add", "."]);
    sh(repo, &["commit", "-q", "-m", "work"]);
    assert_eq!(merged_in(dir, "work", "main", made), None, "not in main yet");
    // Merged: the commit, short.
    sh(repo, &["checkout", "-q", "main"]);
    sh(repo, &["merge", "-q", "--no-ff", "-m", "merge work", "work"]);
    let sha = merged_in(dir, "work", "main", made).expect("landed");
    assert!(sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit()), "{sha}");
    // A branch or a target that isn't there: not landed, no error.
    assert_eq!(merged_in(dir, "gone", "main", made), None);
    assert_eq!(merged_in(dir, "work", "nowhere", made), None);
    assert_eq!(merged_in("/nonexistent/repo", "work", "main", made), None);
}

// ── What a move says ────────────────────────────────────────────────────────

/// A snooze's time is the person's own, as the app writes times: never UTC.
#[test]
fn a_snooze_says_its_time_in_the_persons_zone() {
    let until = t0() + TimeDelta::hours(3);
    let mut d = derived(Place::Snoozed, Why::Snoozed);
    d.until = Some(until);
    let (reason, cause, by) = move_words(Some(Place::NeedsYou), &d, Mover::You);
    assert_eq!((cause, by), (MoveCause::Snoozed, Mover::You));
    let local = until.with_timezone(&chrono::Local);
    assert_eq!(reason, format!("Snoozed until {}", local.format("%b %-d %-I:%M %p")));
    assert!(!reason.contains("UTC"), "{reason}");
    assert!(reason.ends_with(" AM") || reason.ends_with(" PM"), "{reason}");
}

fn derived(place: Place, why: Why) -> Derived {
    Derived { place, status: CardStatus::Working, note: None, until: None, asks: vec![], why, acts: vec![], recheck: None }
}

#[test]
fn a_move_says_why_and_who() {
    let says = |from: Option<Place>, to: Place, why: Why, actor: Mover| move_words(from, &derived(to, why), actor);
    // The engine's own moves.
    assert_eq!(
        says(Some(Place::Planned), Place::Running, Why::ThreadLive, Mover::Engine),
        ("Thread started".into(), MoveCause::Started, Mover::Engine)
    );
    assert_eq!(says(Some(Place::Running), Place::Running, Why::Settling, Mover::Engine).0, "Thread finished");
    assert_eq!(
        says(Some(Place::Running), Place::Done, Why::Closed { how: CloseHow::Merged, by: Mover::Engine, note: Some("e99f817".into()) }, Mover::Engine),
        ("Merged in e99f817".into(), MoveCause::Merged, Mover::Engine)
    );
    assert_eq!(
        says(Some(Place::Running), Place::Done, Why::Closed { how: CloseHow::Report, by: Mover::Engine, note: Some("48 conflicts resolved".into()) }, Mover::Engine).0,
        "Reported done: 48 conflicts resolved"
    );
    assert_eq!(says(Some(Place::Done), Place::Running, Why::WorkSent, Mover::Engine).1, MoveCause::Reopened);
    // The person's: theirs whatever the ladder's reason.
    assert_eq!(
        says(Some(Place::NeedsYou), Place::Running, Why::ThreadLive, Mover::You),
        ("You answered".into(), MoveCause::Answered, Mover::You)
    );
    assert_eq!(
        says(Some(Place::Running), Place::Done, Why::Closed { how: CloseHow::Accepted, by: Mover::You, note: None }, Mover::You),
        ("You approved it".into(), MoveCause::Accepted, Mover::You)
    );
    assert_eq!(says(Some(Place::Running), Place::Planned, Why::Paused, Mover::You).2, Mover::You);
    // An orchestrator's closing, with its reason.
    assert_eq!(
        says(Some(Place::Running), Place::Done, Why::Closed { how: CloseHow::Closed, by: Mover::Agent, note: Some("superseded by #1503".into()) }, Mover::Agent),
        ("Closed: superseded by #1503".into(), MoveCause::Closed, Mover::Agent)
    );
}

#[test]
fn a_cards_chip_is_its_merge_its_pr_or_its_report() {
    let now = t0();
    let mut i = item();
    let mut row = BoardCard::new("c1", now);
    assert_eq!(chip_of(&i, &row), None);
    i.links.push(WorkLink { kind: LinkKind::Pr, label: "PR".into(), target: "https://github.com/zeronsh/zeron/pull/1503/".into() });
    assert_eq!(chip_of(&i, &row).map(|c| c.label), Some("PR #1503".into()));
    row.closed = Some(Closed { at: now, by: Mover::Engine, how: CloseHow::Merged, note: Some("e99f817".into()) });
    assert_eq!(chip_of(&i, &row).map(|c| (c.kind, c.label)), Some((ChipKind::Merged, "merged e99f817".into())));
    row.closed = Some(Closed { at: now, by: Mover::Engine, how: CloseHow::Report, note: None });
    assert_eq!(chip_of(&i, &row).map(|c| c.label), Some("report".into()));
}

#[test]
fn the_stall_wake_names_each_card_and_the_three_choices() {
    let text = stall_text(&[
        ("c1".into(), "Phase 1:\nship".into(), "its thread ended without a report".into()),
        ("c2".into(), "Phase 2".into(), "its thread last reported done: Fixed".into()),
    ]);
    assert!(text.starts_with("[Board] Nothing is moving on these cards of yours."));
    for choice in ["`send_message`", "`ask`", "`item_close`"] {
        assert!(text.contains(choice), "{choice}: {text}");
    }
    assert!(text.contains("\n- \"Phase 1: ship\" (c1): its thread ended without a report"));
    assert!(text.contains("\n- \"Phase 2\" (c2): its thread last reported done: Fixed"));
    assert_eq!(text.lines().count(), 3);
}

#[test]
fn the_pr_ask_has_one_id_per_orchestrator() {
    assert_eq!(pr_ask_id(Some("o1")), "prs-o1");
    assert_eq!(pr_ask_id(None), "prs-loose");
}
