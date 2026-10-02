//! Goal strip, todo strip, workflow run cards and the marker rows: what the
//! layout produces for them at phone widths, and what a tap on them does.

use std::fmt::Write as _;
use std::sync::Arc;

use zeron_client::demo_workflows::{self as demo, PhaseSpec, RunSpec};
use zeron_doc::parts::MessagePart;
use zeron_doc::schema::{MessageRole, SessionMessageEntry};
use zeron_proto::{
    Goal, GoalCommand, GoalLimits, GoalReasonKind, GoalStatus, MessageOrigin, TodoItem, TodoStatus,
    WorkflowEventMarker, WorkflowRunsState, WorkflowStatus, WorkflowStopReason,
};

use super::display::WidgetKind;
use super::status::{ArtifactView, PREVIEW_LINES};
use super::tests::worker;
use super::*;

const NOW: i64 = 1_800_000_000_000;

fn entry(id: &str, role: MessageRole, origin: Option<MessageOrigin>, body: &str) -> Arc<SessionMessageEntry> {
    Arc::new(SessionMessageEntry {
        origin,
        id: id.into(),
        role,
        parts: vec![MessagePart::Text { id: "t0".into(), text: body.into() }],
        created_at: 0,
        device_id: "d".into(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    })
}

fn input(entries: Vec<Arc<SessionMessageEntry>>) -> TranscriptInput {
    TranscriptInput { entries, now_ms: NOW, ..Default::default() }
}

fn demo_input() -> TranscriptInput {
    let entries = demo::transcript("host", NOW).into_iter().map(Arc::new).collect();
    TranscriptInput {
        entries,
        goal: Some(Arc::new(demo::demo_goal(NOW))),
        todo: Some(Arc::new(vec![
            TodoItem::new("Map the attack surface", TodoStatus::Completed),
            TodoItem::new("Fix the path traversal in the upload handler", TodoStatus::InProgress),
            TodoItem::new("Add a regression test for the upload path", TodoStatus::Pending),
        ])),
        workflows: Arc::new(demo::demo_runs(NOW)),
        now_ms: NOW,
        ..Default::default()
    }
}

fn frame_at(width: f32, input: TranscriptInput) -> Arc<LayoutFrame> {
    let mut w = worker(width);
    w.input = input;
    w.pass()
}

/// Every action payload a row exposes, in paint order.
fn actions(frame: &LayoutFrame, i: u32) -> Vec<String> {
    frame
        .display(i)
        .unwrap()
        .widgets
        .into_iter()
        .filter_map(|w| match w.kind {
            WidgetKind::Action { .. } => w.payload,
            _ => None,
        })
        .collect()
}

fn text(frame: &LayoutFrame, i: u32) -> String {
    frame.display(i).unwrap().text
}

fn row_with(frame: &LayoutFrame, needle: &str) -> Option<u32> {
    (0..frame.row_count()).find(|i| text(frame, *i).contains(needle))
}

fn cards(frame: &LayoutFrame) -> Vec<u32> {
    (0..frame.row_count()).filter(|i| frame.placement(*i).unwrap().kind == RowKind::Card).collect()
}

/// A readable dump of a frame: one block per row with its kind, size, text
/// and tap targets. Used by the tests and by docs/workflows-mobile.md.
pub(crate) fn dump(frame: &LayoutFrame) -> String {
    let mut out = String::new();
    for i in 0..frame.row_count() {
        let p = frame.placement(i).unwrap();
        let d = frame.display(i).unwrap();
        let _ = writeln!(out, "[{i}] {:?} y={:.0} h={:.0}", p.kind, p.y, p.height);
        // Text runs grouped by baseline give the visual lines.
        let units: Vec<u16> = d.text.encode_utf16().collect();
        let mut lines: Vec<(f32, f32, String)> = Vec::new();
        for r in d.runs.iter().filter(|r| r.scroller.is_none()) {
            let s = String::from_utf16_lossy(&units[r.start as usize..(r.start + r.len) as usize]);
            match lines.iter_mut().find(|l| (l.0 - r.baseline).abs() < 1.0) {
                Some(l) => l.2.push_str(&format!("  {s}")),
                None => lines.push((r.baseline, r.x, s)),
            }
        }
        lines.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, x, s) in lines {
            let _ = writeln!(out, "      x={x:>5.0} {s}");
        }
        for (si, sc) in d.scrollers.iter().enumerate() {
            let content: Vec<String> = d
                .runs
                .iter()
                .filter(|r| r.scroller == Some(si as u32))
                .map(|r| String::from_utf16_lossy(&units[r.start as usize..(r.start + r.len) as usize]))
                .collect();
            let _ = writeln!(out, "      scroller {:.0}x{:.0} (content {:.0}): {}", sc.w, sc.h, sc.content_width, content.join(" | "));
        }
        for w in &d.widgets {
            match &w.kind {
                WidgetKind::Action { label } => {
                    let _ = writeln!(out, "      action {:?} -> {} @({:.0},{:.0} {:.0}x{:.0})", label, w.payload.clone().unwrap_or_default(), w.x, w.y, w.w, w.h);
                }
                WidgetKind::Icon { name, .. } => {
                    let _ = writeln!(out, "      icon {name}");
                }
                WidgetKind::Spinner => {
                    let _ = writeln!(out, "      spinner");
                }
                _ => {}
            }
        }
    }
    out
}

// MARK: - builders

fn goal(status: GoalStatus) -> Goal {
    let mut g = demo::demo_goal(NOW);
    g.status = status;
    match status {
        GoalStatus::Paused => g.stop(GoalStatus::Paused, GoalReasonKind::User, "Paused by you"),
        GoalStatus::BudgetLimited => g.stop(GoalStatus::BudgetLimited, GoalReasonKind::MaxRounds, "Reached the round limit"),
        _ => {}
    }
    g
}

fn todo(spec: &str) -> Arc<Vec<TodoItem>> {
    // x = completed, > = in progress, . = pending
    Arc::new(
        spec.chars()
            .enumerate()
            .map(|(i, c)| {
                TodoItem::new(
                    format!("item {i}"),
                    match c {
                        'x' => TodoStatus::Completed,
                        '>' => TodoStatus::InProgress,
                        _ => TodoStatus::Pending,
                    },
                )
            })
            .collect(),
    )
}

fn wf_marker(id: &str, run: &str, marker: WorkflowEventMarker) -> Arc<SessionMessageEntry> {
    let name = "review";
    entry(
        id,
        MessageRole::System,
        Some(MessageOrigin::WorkflowEvent { run_id: run.into(), marker, name: name.into(), detail: String::new() }),
        &zeron_proto::workflow_marker_text(marker, name, ""),
    )
}

fn run_state(specs: &[RunSpec]) -> Arc<WorkflowRunsState> {
    Arc::new(WorkflowRunsState { revision: 1, runs: specs.iter().map(demo::build_run).collect() })
}

fn live_spec(id: &str) -> RunSpec {
    let mut spec = RunSpec::new(
        id,
        "review",
        WorkflowStatus::Running,
        vec![PhaseSpec::new("scan", 3, 3, 0), PhaseSpec::new("review", 8, 4, 1), PhaseSpec::new("verify", 2, 0, 0)],
    );
    spec.artifacts = true;
    spec.reports = 5;
    spec.result = Some("Four findings across 412 files.".into());
    spec
}

fn with_run(spec: &RunSpec) -> TranscriptInput {
    TranscriptInput {
        entries: vec![
            entry("u1", MessageRole::User, None, "review the repo"),
            wf_marker("wf-start", &spec.id, WorkflowEventMarker::Started),
            entry("a1", MessageRole::Assistant, None, "Started."),
        ],
        workflows: run_state(std::slice::from_ref(spec)),
        now_ms: NOW,
        ..Default::default()
    }
}

// MARK: - todo

#[test]
fn the_todo_strip_closes_the_transcript_and_shows_the_current_item() {
    let mut inp = input(vec![entry("u", MessageRole::User, None, "go"), entry("a", MessageRole::Assistant, None, "Working on it.")]);
    inp.todo = Some(todo("x>.."));
    let frame = frame_at(390.0, inp.clone());
    let last = frame.row_count() - 1;
    assert_eq!(frame.placement(last).unwrap().kind, RowKind::Card);
    let t = text(&frame, last);
    assert!(t.starts_with("Todo · 1/4"), "{t}");
    // Open while work remains: every item is listed, the current one included.
    for i in 0..4 {
        assert!(t.contains(&format!("item {i}")), "{t}");
    }
    assert_eq!(actions(&frame, last), ["todo.toggle"]);

    // The current item spins only while a turn is live.
    let spinners = |f: &LayoutFrame| f.display(f.row_count() - 1).unwrap().widgets.iter().filter(|w| matches!(w.kind, WidgetKind::Spinner)).count();
    assert_eq!(spinners(&frame), 0);
    inp.working = true;
    let live = frame_at(390.0, inp);
    assert_eq!(spinners(&live), 1);
}

#[test]
fn a_collapsed_todo_strip_names_the_item_being_worked_on() {
    let mut w = worker(390.0);
    w.input = input(vec![]);
    w.input.todo = Some(todo("xx>.."));
    w.act("todo.toggle");
    let frame = w.pass();
    let t = text(&frame, 0);
    assert!(t.starts_with("Todo · 2/5"), "{t}");
    assert!(t.contains("item 2") && !t.contains("item 3"), "only the headline: {t}");
    // Opening it again lists everything.
    w.act("todo.toggle");
    assert!(text(&w.pass(), 0).contains("item 4"));
}

#[test]
fn a_finished_list_is_compact_and_can_be_dismissed() {
    let mut w = worker(390.0);
    w.input = input(vec![]);
    w.input.todo = Some(todo("xxx"));
    let frame = w.pass();
    let t = text(&frame, 0);
    assert!(t.starts_with("Todo · 3/3 · All done"), "{t}");
    assert!(!t.contains("item 0"), "compact once everything is done: {t}");
    assert_eq!(actions(&frame, 0), ["todo.toggle", "todo.dismiss"]);
    w.act("todo.dismiss");
    assert_eq!(w.pass().row_count(), 0, "dismissed");
    // A different list is worth showing again.
    w.input.todo = Some(todo("xxxx"));
    assert_eq!(w.pass().row_count(), 1);
}

#[test]
fn long_todo_lists_fold_around_the_current_item() {
    let mut w = worker(390.0);
    w.input = input(vec![]);
    w.input.todo = Some(todo("xxxxxxxxx>.........."));
    let frame = w.pass();
    let t = text(&frame, 0);
    assert!(t.contains("item 8") && t.contains("item 9") && t.contains("item 10"), "{t}");
    assert!(!t.contains("item 0") && !t.contains("item 15"), "{t}");
    assert!(t.contains("8 earlier") && t.contains("9 later"), "{t}");
    assert_eq!(actions(&frame, 0), ["todo.toggle", "todo.earlier", "todo.later"]);
    w.act("todo.earlier");
    let t = text(&w.pass(), 0);
    assert!(t.contains("item 0") && t.contains("Hide 8 earlier"), "{t}");
    w.act("todo.later");
    let t = text(&w.pass(), 0);
    assert!(t.contains("item 19") && t.contains("Hide 9 later"), "{t}");
}

#[test]
fn the_todo_tool_chip_marks_the_item_in_progress() {
    use zeron_proto::ToolCall;
    let part = MessagePart::Tool {
        id: "td".into(),
        call: ToolCall::Todo { items: todo("x>.").to_vec() },
        is_error: false,
        resolved: true,
        output: None,
        diff: None,
        output_ref: None,
        output_bytes: None,
        diff_ref: None,
        diff_stats: None,
        subagent_ref: None,
        subagent_status: None,
        subagent_tail: None,
    };
    let e = Arc::new(SessionMessageEntry {
        origin: None,
        id: "a".into(),
        role: MessageRole::Assistant,
        parts: vec![part],
        created_at: 0,
        device_id: "d".into(),
        // Streaming: the group is the live tail, so it is open.
        status: Some(MessageStatus::Streaming),
        continuation_of: None,
        duration_ms: None,
    });
    let mut w = worker(390.0);
    w.input = input(vec![e]);
    // Open the row's inline detail (what a tap on the chip does).
    w.builder.detail_open.insert(rows::row_key("a#g0/td"), true);
    let frame = w.pass();
    let t = text(&frame, 0);
    assert!(t.contains("1/3 done · item 1"), "the chip names the current item: {t}");
    assert!(t.contains("[x] item 0") && t.contains("[~] item 1") && t.contains("[ ] item 2"), "{t}");
}

// MARK: - goal

#[test]
fn the_goal_strip_shows_status_round_and_controls() {
    let words = |status| {
        let mut inp = input(vec![]);
        inp.goal = Some(Arc::new(goal(status)));
        let f = frame_at(390.0, inp);
        (text(&f, 0), actions(&f, 0))
    };
    let (t, a) = words(GoalStatus::Active);
    assert!(t.starts_with("Goal") && t.contains("Active") && t.contains("R3 · 17m 0s"), "{t}");
    assert!(t.contains("Round 3: Add a regression test for the upload path"), "{t}");
    assert_eq!(a, ["goal.toggle", "goal.pause"]);
    let (t, a) = words(GoalStatus::Paused);
    assert!(t.contains("Paused") && t.contains("Paused by you"), "{t}");
    assert_eq!(a, ["goal.toggle", "goal.resume"]);
    let (t, a) = words(GoalStatus::BudgetLimited);
    assert!(t.contains("Budget reached") && t.contains("Reached the round limit"), "{t}");
    assert_eq!(a, ["goal.toggle", "goal.resume"]);
    let (t, a) = words(GoalStatus::Complete);
    assert!(t.contains("Complete") && t.contains("Verified"), "{t}");
    assert_eq!(a, ["goal.toggle"], "a finished goal has nothing to pause or resume");
}

#[test]
fn an_open_goal_strip_shows_the_objective_meta_verdict_and_clear() {
    let mut w = worker(390.0);
    w.input = input(vec![]);
    w.input.goal = Some(Arc::new(goal(GoalStatus::Active)));
    w.act("goal.toggle");
    let frame = w.pass();
    let t = text(&frame, 0);
    assert!(t.contains("Make the security review pass: every finding fixed"), "{t}");
    assert!(t.contains("Round 3 of 25 · 51.3k tokens (verifier 3.1k) · 17m 0s"), "{t}");
    assert!(t.contains("Verifier, round 2: not satisfied — The fix has no regression test."), "{t}");
    assert_eq!(actions(&frame, 0), ["goal.toggle", "goal.pause", "goal.pause", "goal.clear"]);
    w.act("goal.toggle");
    assert!(!text(&w.pass(), 0).contains("Verifier, round 2"));
}

#[test]
fn a_refused_goal_command_says_so_on_the_strip() {
    // No session is attached (the platform detached it): the command cannot
    // go anywhere, and the card must not pretend it did.
    let mut w = worker(390.0);
    w.input = input(vec![]);
    w.input.goal = Some(Arc::new(goal(GoalStatus::Active)));
    w.act("goal.pause");
    let t = text(&w.pass(), 0);
    assert!(t.contains("shut down"), "the failure is on the card: {t}");
}

#[test]
fn goal_machinery_is_a_compact_marker_not_a_bubble() {
    let round = entry(
        "r2",
        MessageRole::User,
        Some(MessageOrigin::Goal { goal_id: "g".into(), round: 2, title: "Fix the path traversal".into() }),
        "Continue toward the goal. Next: fix the path traversal.",
    );
    let verdict = entry(
        "v1",
        MessageRole::System,
        Some(MessageOrigin::GoalEvent {
            goal_id: "g".into(),
            event: zeron_proto::GoalEventKind::NotSatisfied,
            round: 1,
            title: "Fix the path traversal".into(),
            detail: "Two findings are still open.".into(),
            verifier_chat_id: None,
        }),
        "Round 1: not satisfied",
    );
    let unknown = entry("x", MessageRole::User, Some(MessageOrigin::Unknown), "an origin from the future");
    let frame = frame_at(390.0, input(vec![entry("u", MessageRole::User, None, "set a goal"), round, verdict, unknown]));
    let kinds: Vec<_> = (0..frame.row_count()).map(|i| frame.placement(i).unwrap().kind).collect();
    assert_eq!(kinds, [RowKind::User, RowKind::Chip, RowKind::Chip, RowKind::User]);
    assert!(text(&frame, 1).contains("Goal · round 2: Fix the path traversal"));
    assert!(!text(&frame, 1).contains("Continue toward"), "the prompt is for the agent");
    assert!(text(&frame, 2).contains("Verifier · round 1 not satisfied") && text(&frame, 2).contains("Two findings are still open."));
    assert!(text(&frame, 3).contains("an origin from the future"), "an unknown origin stays what it was");
}

// MARK: - workflow cards

#[test]
fn a_run_card_replaces_its_started_marker_and_folds_the_end_marker() {
    let mut inp = with_run(&live_spec("r1"));
    inp.entries.push(wf_marker("wf-end", "r1", WorkflowEventMarker::Stopped));
    let frame = frame_at(390.0, inp.clone());
    assert_eq!(cards(&frame).len(), 1);
    assert_eq!(frame.row_count(), 3, "user, card, assistant text, and nothing for the end marker");
    assert!(text(&frame, 1).starts_with("Workflow running · review"));

    // The state does not list the run (not synced yet): plain markers stay.
    inp.workflows = Arc::default();
    let plain = frame_at(390.0, inp);
    assert!(cards(&plain).is_empty());
    assert!(row_with(&plain, "Workflow started: review").is_some());
    assert!(row_with(&plain, "Workflow stopped: review").is_some());
}

#[test]
fn a_denied_run_keeps_its_marker_and_a_resumed_run_gets_its_own_card() {
    let mut inp = with_run(&live_spec("r1"));
    inp.entries.push(wf_marker("wf-denied", "r0", WorkflowEventMarker::Denied));
    inp.entries.push(wf_marker("wf-resumed", "r2", WorkflowEventMarker::Resumed));
    inp.workflows = run_state(&[live_spec("r1"), live_spec("r2")]);
    let frame = frame_at(390.0, inp);
    assert_eq!(cards(&frame).len(), 2);
    assert!(row_with(&frame, "Workflow denied: review").is_some());
}

#[test]
fn a_folded_card_shows_the_run_at_a_glance() {
    let mut spec = live_spec("r1");
    spec.question = true;
    let frame = frame_at(390.0, with_run(&spec));
    let t = text(&frame, 1);
    assert!(t.contains("Workflow running · review"), "{t}");
    assert!(t.contains("3 phases · 13 agents · 3 working"), "{t}");
    assert!(t.contains("1 agent is waiting for your answer"), "{t}");
    let d = frame.display(1).unwrap();
    // The phase rail: one capsule per phase with its settled / observed count.
    let rail: Vec<String> = d
        .runs
        .iter()
        .filter(|r| r.scroller == Some(0))
        .map(|r| {
            let u: Vec<u16> = d.text.encode_utf16().collect();
            String::from_utf16_lossy(&u[r.start as usize..(r.start + r.len) as usize])
        })
        .collect();
    assert_eq!(rail, ["scan", "3/3", "review", "5/8", "verify", "0/2"]);
    // Artifact capsules open the card on the document; the header folds / unfolds.
    let a = actions(&frame, 1);
    assert_eq!(a[0], "wf.toggle:r1");
    assert!(a.contains(&"wf.artifact:r1:summary".to_owned()), "{a:?}");
    assert!(!a.iter().any(|p| p.starts_with("chat:") || p.starts_with("wf.stop")), "stop lives in the open card: {a:?}");
}

#[test]
fn an_open_card_lists_agents_that_open_their_chats() {
    let mut w = worker(390.0);
    w.input = with_run(&live_spec("r1"));
    w.act("wf.toggle:r1");
    let frame = w.pass();
    let t = text(&frame, 1);
    assert!(t.contains("Agents · 13"), "{t}");
    // Working agents first, with what they are doing.
    assert!(t.contains("review-6") && t.contains("working") && t.contains("turn 2 · 5 tool calls · last: Grep"), "{t}");
    let a = actions(&frame, 1);
    let opens: Vec<_> = a.iter().filter(|p| p.starts_with("chat:")).collect();
    assert_eq!(opens.len(), 10, "one page of agents");
    assert!(a.contains(&"wf.actors:r1".to_owned()), "more agents are a tap away: {a:?}");
    assert!(a.contains(&"wf.stop:r1".to_owned()));
    // An agent that has not started has no chat to open.
    w.act("wf.actors:r1");
    let a = actions(&w.pass(), 1);
    assert_eq!(a.iter().filter(|p| p.starts_with("chat:")).count(), 13 - 2, "the two queued verifiers have no chat yet");
}

#[test]
fn the_card_never_grows_with_the_size_of_the_run() {
    // 200 agents in one phase: folded, the card is the same height as for 3;
    // open, it lists one page and says how many it left out.
    let big = RunSpec::new("r1", "sweep", WorkflowStatus::Running, vec![PhaseSpec::new("sweep", 200, 120, 7)]);
    let small = RunSpec::new("r1", "sweep", WorkflowStatus::Running, vec![PhaseSpec::new("sweep", 3, 1, 0)]);
    let h = |spec: &RunSpec| frame_at(390.0, with_run(spec)).placement(1).unwrap().height;
    assert_eq!(h(&big), h(&small));
    let mut w = worker(390.0);
    w.input = with_run(&big);
    w.act("wf.toggle:r1");
    let started = std::time::Instant::now();
    let frame = w.pass();
    assert!(started.elapsed() < std::time::Duration::from_millis(500), "{:?}", started.elapsed());
    let t = text(&frame, 1);
    assert!(t.contains("Agents · 200") && t.contains("Show 10 more") && t.contains("190 not shown"), "{t}");
    assert_eq!(actions(&frame, 1).iter().filter(|p| p.starts_with("chat:")).count(), 10);
    assert!(frame.placement(1).unwrap().height < 1_400.0, "{}", frame.placement(1).unwrap().height);
}

#[test]
fn a_stopped_run_explains_and_offers_resume() {
    let mut spec = live_spec("r1");
    spec.status = WorkflowStatus::Stopped;
    spec.stop_reason = Some(WorkflowStopReason::Provider);
    let mut w = worker(390.0);
    w.input = with_run(&spec);
    let t = text(&w.pass(), 1);
    assert!(t.starts_with("Workflow stopped · review") && t.contains("The provider rejected the key (401)."), "{t}");
    w.act("wf.toggle:r1");
    let a = actions(&w.pass(), 1);
    assert!(a.contains(&"wf.resume:r1".to_owned()) && !a.contains(&"wf.stop:r1".to_owned()), "{a:?}");
    // No session attached: the tap is refused on the card, not swallowed.
    w.act("wf.resume:r1");
    assert!(text(&w.pass(), 1).contains("shut down"));
}

#[test]
fn an_open_card_shows_questions_result_reports_and_usage() {
    let mut spec = live_spec("r1");
    spec.question = true;
    let mut w = worker(390.0);
    w.input = with_run(&spec);
    w.act("wf.toggle:r1");
    let frame = w.pass();
    let t = text(&frame, 1);
    assert!(t.contains("review-6: Should I treat the vendored `third_party/` directory as in scope?"), "{t}");
    assert!(t.contains("Answer in the box at the bottom of the chat."), "{t}");
    assert!(t.contains("Result") && t.contains("Four findings across 412 files."), "{t}");
    assert!(t.contains("Reports · 5") && t.contains("Show 2 more"), "{t}");
    assert!(t.contains("3m 12s · 48.3k tokens"), "{t}");
    w.act("wf.reports:r1");
    assert!(text(&w.pass(), 1).contains("Show fewer reports"));
}

#[test]
fn artifacts_preview_as_markdown_with_loading_error_and_retry() {
    let mut w = worker(390.0);
    w.input = with_run(&live_spec("r1"));
    w.act("wf.toggle:r1");
    // No session attached: the load fails and offers another try.
    w.act("wf.artifact:r1:summary");
    let t = text(&w.pass(), 1);
    assert!(t.contains("Not connected to the chat.") && t.contains("Try again"), "{t}");
    assert!(actions(&w.pass(), 1).contains(&"wf.artifact-retry:r1:summary".to_owned()));
    // The host answers: a table arrives as markdown.
    let md = zeron_proto::artifact_view::to_markdown(
        zeron_proto::ArtifactKind::Table,
        "application/json",
        Some(r#"{"columns":["Severity","Finding"],"rows":[["high","Path traversal"],["low","Unescaped log line"]]}"#),
        PREVIEW_LINES,
    )
    .unwrap();
    w.builder.ui.artifacts.insert(("r1".into(), "summary".into()), ArtifactView::Ready { title: "Findings".into(), markdown: Arc::new(md), total: 200, truncated: false });
    let t = text(&w.pass(), 1);
    assert!(t.contains("Severity") && t.contains("Path traversal") && t.contains("Unescaped log line"), "{t}");
    assert!(!t.contains("Try again"));
    // Showing the first part of a big document says so.
    w.builder.ui.artifacts.insert(("r1".into(), "summary".into()), ArtifactView::Ready { title: "Doc".into(), markdown: Arc::new("# Heading\n\nBody".into()), total: 900_000, truncated: true });
    assert!(text(&w.pass(), 1).contains(&format!("Showing the first part of {}.", zeron_proto::workflow_view::format_bytes(900_000))));
    // The open one again closes it.
    w.act("wf.artifact:r1:summary");
    assert!(!text(&w.pass(), 1).contains("Heading"));
}

#[test]
fn a_late_artifact_for_a_closed_preview_is_dropped() {
    let mut w = worker(390.0);
    w.input = with_run(&live_spec("r1"));
    w.act("wf.artifact:r1:summary");
    w.act("wf.artifact:r1:summary"); // closed again
    assert!(w.builder.ui.artifact.is_empty());
}

#[test]
fn unchanged_state_reuses_the_card_and_a_change_remeasures_only_it() {
    let mut w = worker(390.0);
    w.input = with_run(&live_spec("r1"));
    w.input.goal = Some(Arc::new(goal(GoalStatus::Active)));
    let a = w.pass();
    let b = w.pass();
    let versions = |f: &LayoutFrame| (0..f.row_count()).map(|i| f.placement(i).unwrap().version).collect::<Vec<_>>();
    assert_eq!(versions(&a), versions(&b), "nothing changed, nothing is rebuilt");
    // The run progresses: its card changes, the goal strip does not.
    let mut spec = live_spec("r1");
    spec.phases[1] = PhaseSpec::new("review", 8, 6, 1);
    w.input.workflows = run_state(&[spec]);
    let c = w.pass();
    let (va, vc) = (versions(&a), versions(&c));
    assert_ne!(va[1], vc[1]);
    assert_eq!(va.last(), vc.last());
}

#[test]
fn the_result_message_is_a_compact_row() {
    let result = entry(
        "res",
        MessageRole::User,
        Some(MessageOrigin::Workflow { run_id: "r1".into(), name: "review".into(), status: WorkflowStatus::Completed }),
        "[Workflow completed] review (run r1)\ncompleted · 6 agents\n<workflow_result>\nlong text for the agent\n</workflow_result>",
    );
    let frame = frame_at(390.0, input(vec![result]));
    assert_eq!(frame.placement(0).unwrap().kind, RowKind::Chip);
    let t = text(&frame, 0);
    assert!(t.contains("Result of workflow review (completed) sent to the agent") && !t.contains("long text"), "{t}");
}

#[test]
fn a_workflow_question_for_the_agent_keeps_a_one_line_marker() {
    let q = entry(
        "q",
        MessageRole::User,
        Some(MessageOrigin::Workflow { run_id: "r1".into(), name: "review".into(), status: WorkflowStatus::Running }),
        "[Workflow question] review (run r1)\n\nShould I include vendored code?\nIt has three findings.",
    );
    let t = text(&frame_at(390.0, input(vec![q])), 0);
    assert!(t.contains("Workflow agent asks a question · review") && t.contains("Should I include vendored code?"), "{t}");
}

// MARK: - the whole thing

#[test]
fn the_trays_close_the_transcript_in_order() {
    let mut inp = demo_input();
    inp.working = true;
    let frame = frame_at(390.0, inp);
    let n = frame.row_count();
    let kinds: Vec<_> = (n - 3..n).map(|i| frame.placement(i).unwrap().kind).collect();
    assert_eq!(kinds, [RowKind::Working, RowKind::Card, RowKind::Card]);
    assert!(text(&frame, n - 2).starts_with("Goal"));
    assert!(text(&frame, n - 1).starts_with("Todo"));
}

#[test]
fn every_card_paints_inside_the_column_at_every_phone_width() {
    for width in [280.0, 320.0, 375.0, 393.0, 430.0, 744.0, 1024.0] {
        let mut w = worker(width);
        w.input = demo_input();
        w.input.workflows = {
            let mut state = demo::demo_runs(NOW);
            state.runs[1].header.name = "a-rather-long-workflow-name-that-cannot-possibly-fit-on-one-line".into();
            Arc::new(state)
        };
        for act in ["wf.toggle:run-live", "wf.toggle:run-done", "goal.toggle", "wf.actors:run-live", "wf.reports:run-live", "wf.result:run-done"] {
            w.act(act);
        }
        w.act("wf.artifact:run-live:summary");
        w.builder.ui.artifacts.insert(
            ("run-live".into(), "summary".into()),
            ArtifactView::Ready { title: "t".into(), markdown: Arc::new("| a | b |\n| --- | --- |\n| 1 | 2 |\n\nA paragraph.".into()), total: 10, truncated: false },
        );
        let frame = w.pass();
        assert!(!cards(&frame).is_empty());
        for i in cards(&frame) {
            let p = frame.placement(i).unwrap();
            let d = frame.display(i).unwrap();
            assert!((d.height - p.height).abs() < 0.01, "paint and measure agree");
            let units = d.text.encode_utf16().count() as u32;
            for run in &d.runs {
                assert!(run.start + run.len <= units);
                if run.scroller.is_none() {
                    // A one-line slot fades its overflow out at the slot's edge.
                    let faded = d.fades.iter().any(|f| f.scroller.is_none() && run.baseline > f.y && run.baseline <= f.y + f.h + 0.5 && f.x + f.w <= width + 0.5);
                    assert!(run.x >= 0.0 && (faded || run.x + run.width <= width + 0.5), "run inside width {width}: {} {run:?}", i);
                    assert!(run.baseline > 0.0 && run.baseline <= d.height + 0.5, "baseline inside the row at {width}");
                }
            }
            for wg in d.widgets.iter().filter(|wg| wg.scroller.is_none()) {
                assert!(wg.x >= -0.5 && wg.x + wg.w <= width + 0.5, "widget {:?} inside width {width}", wg.kind);
                assert!(wg.y >= -0.5 && wg.y + wg.h <= d.height + 0.5, "widget {:?} inside row {} at {width}: y={} h={} row={}", wg.kind, i, wg.y, wg.h, d.height);
            }
            for b in &d.boxes {
                if b.scroller.is_none() {
                    assert!(b.x >= -0.5 && b.x + b.w <= width + 0.5, "box inside width {width}: {b:?}");
                }
            }
        }
    }
}

#[test]
fn view_actions_are_navigation_or_handled_here() {
    let ts = super::tests::text_system();
    let view = TranscriptView::new(ts, Arc::new(super::tests::Quiet));
    assert_eq!(view.act("chat:abc".into()), ActionOutcome::OpenChat { chat_id: "abc".into() });
    assert_eq!(view.act("goal.toggle".into()), ActionOutcome::Done);
    view.close();
}

#[test]
fn goal_commands_have_the_wire_shape_the_host_reads() {
    // The strip's controls and `/goal` use the same commands the desktop sends.
    assert_eq!(serde_json::to_value(GoalCommand::Pause).unwrap(), serde_json::json!({"action": "pause"}));
    let set = GoalCommand::Set { objective: "x".into(), limits: GoalLimits::default(), replace: true };
    assert_eq!(serde_json::to_value(set).unwrap()["action"], "set");
}

// MARK: - snapshots

/// Compare `got` with `snapshots/<name>`; `ZERON_UPDATE_SNAPSHOTS=1` rewrites
/// it. The files are the textual layout dumps docs/workflows-mobile.md quotes.
fn snapshot(name: &str, got: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/layout/snapshots").join(name);
    if std::env::var("ZERON_UPDATE_SNAPSHOTS").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(want == got, "{name} changed; rerun with ZERON_UPDATE_SNAPSHOTS=1 and review the diff\n--- got ---\n{got}");
}

#[test]
fn snapshot_of_the_demo_chat_folded_at_iphone_width() {
    snapshot("workflows_demo_390.txt", &dump(&frame_at(390.0, demo_input())));
}

#[test]
fn snapshot_of_the_demo_chat_open_on_a_small_phone() {
    let mut w = worker(320.0);
    w.input = demo_input();
    for act in ["wf.toggle:run-live", "goal.toggle"] {
        w.act(act);
    }
    snapshot("workflows_demo_320_open.txt", &dump(&w.pass()));
}
