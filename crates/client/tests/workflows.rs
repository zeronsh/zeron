//! Goal mode, the checklist and workflow runs through the client: what a
//! phone reads from the replicated doc, and the commands it sends back. The
//! demo host plays the engine (it applies the commands to the doc).

use std::sync::Arc;
use std::time::{Duration, Instant};

use zeron_client::events::NullListener;
use zeron_client::{
    Client, ClientConfig, ClientError, Credentials, DemoFixture, DemoOptions, SendOutcome,
    SendRequest, SessionHandle, StreamSpeed, UserInputAnswer,
};
use zeron_proto::{GoalCommand, GoalStatus, TodoStatus, WorkflowCommand, WorkflowStatus};

const CHAT: &str = "chat-workflows";

fn demo(fixture: DemoFixture) -> (Client, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = ClientConfig::new("https://edge.invalid", dir.path());
    config.device_id = "ios-test".into();
    config.device_name = "Test iPhone".into();
    let options = DemoOptions {
        fixture,
        stream_speed: StreamSpeed::Fast,
        ..Default::default()
    };
    let client = Client::new(config, Credentials::Demo(options), Arc::new(NullListener)).unwrap();
    (client, dir)
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open() -> (SessionHandle, Client, tempfile::TempDir) {
    let (client, dir) = demo(DemoFixture::Workflows);
    let session = client.open_session(CHAT).unwrap();
    wait_for("the transcript", || session.snapshot().hydrated);
    (session, client, dir)
}

fn goal_status(session: &SessionHandle) -> Option<GoalStatus> {
    session.snapshot().goal.as_ref().map(|g| g.status)
}

fn run_status(session: &SessionHandle, run: &str) -> Option<WorkflowStatus> {
    session.snapshot().workflows.run(run).map(|r| r.header.status)
}

#[test]
fn the_snapshot_carries_the_goal_the_runs_and_the_checklist() {
    let (session, _client, _dir) = open();
    let snap = session.snapshot();
    let goal = snap.goal.as_ref().expect("the host wrote meta.goal");
    assert_eq!(goal.status, GoalStatus::Active);
    assert_eq!(goal.iteration, 3);
    assert_eq!(goal.verdicts.len(), 2);

    assert_eq!(snap.workflows.runs.len(), 2);
    let live = snap.workflows.run("run-live").unwrap();
    assert_eq!(live.header.status, WorkflowStatus::Running);
    assert_eq!(live.actors.len(), 17);
    assert_eq!(live.pending_questions.len(), 1);
    assert_eq!(live.artifacts.len(), 3);

    let todo = snap.todo.as_ref().expect("the agent wrote a checklist");
    assert_eq!(todo.len(), 5);
    assert_eq!(todo[2].status(), TodoStatus::InProgress);
}

#[test]
fn chats_without_the_new_state_read_as_none() {
    // A chat from an older host: no goal, no runs, no checklist, and nothing
    // fails to read.
    let (client, _dir) = demo(DemoFixture::Standard);
    let session = client.open_session("chat-blog").unwrap();
    wait_for("the transcript", || session.snapshot().hydrated);
    let snap = session.snapshot();
    assert!(snap.goal.is_none() && snap.todo.is_none());
    assert!(snap.workflows.runs.is_empty());
}

#[test]
fn goal_lines_typed_in_the_composer_are_commands_not_messages() {
    let (session, _client, _dir) = open();
    let before = session.snapshot().transcript_len;

    // Bare `/goal` reports; nothing is sent.
    let outcome = session.send(SendRequest::text("/goal")).unwrap();
    assert!(matches!(outcome, SendOutcome::Command { notice: Some(n) } if n.starts_with("Goal active")));

    // Pause and resume reach the host and come back through the doc.
    let outcome = session.send(SendRequest::text("/goal pause")).unwrap();
    assert_eq!(outcome, SendOutcome::Command { notice: None });
    wait_for("the goal to pause", || goal_status(&session) == Some(GoalStatus::Paused));
    session.send(SendRequest::text("/goal resume")).unwrap();
    wait_for("the goal to resume", || goal_status(&session) == Some(GoalStatus::Active));

    // The host left lifecycle markers; the commands themselves are no user turns.
    let snap = session.snapshot();
    assert!(snap.transcript().iter().any(|e| e.id.starts_with("goal-goal-demo-")));
    assert!(
        !snap.transcript()[before..].iter().any(|e| e.role() == zeron_client::MessageRole::User),
        "a slash command is not a message"
    );
}

#[test]
fn a_plain_objective_cannot_replace_a_live_goal() {
    let (session, _client, _dir) = open();
    let err = session.send(SendRequest::text("/goal something else entirely")).unwrap_err();
    assert!(matches!(&err, ClientError::InvalidArgument(m) if m.contains("replace")), "{err}");
    // With `replace` it goes through.
    session.send(SendRequest::text("/goal replace Ship the release")).unwrap();
    wait_for("the new goal", || {
        session.snapshot().goal.as_ref().is_some_and(|g| g.objective == "Ship the release")
    });
    // Clearing it leaves none; pausing nothing is refused.
    session.send(SendRequest::text("/goal clear")).unwrap();
    wait_for("the goal to clear", || session.snapshot().goal.is_none());
    let err = session.send(SendRequest::text("/goal pause")).unwrap_err();
    assert!(matches!(&err, ClientError::InvalidArgument(m) if m.contains("no goal")), "{err}");
    let err = session.send(SendRequest::text("/goal")).unwrap_err();
    assert!(matches!(&err, ClientError::InvalidArgument(m) if m.contains("no goal")), "{err}");
}

#[test]
fn only_a_leading_goal_command_counts_and_oversized_ones_are_refused() {
    let (session, _client, _dir) = open();
    // Indented, or not at the start: an ordinary message.
    for text in [" /goal pause", "please /goal pause", "/goals"] {
        let outcome = session.send(SendRequest::text(text)).unwrap();
        assert!(!matches!(outcome, SendOutcome::Command { .. }), "{text:?}");
    }
    let err = session
        .send(SendRequest::text(format!("/goal replace {}", "x".repeat(5000))))
        .unwrap_err();
    assert!(matches!(&err, ClientError::InvalidArgument(m) if m.contains("limit")), "{err}");
}

#[test]
fn the_strips_commands_travel_the_same_plane() {
    let (session, _client, _dir) = open();
    session.goal_command(GoalCommand::Pause).unwrap();
    wait_for("pause", || goal_status(&session) == Some(GoalStatus::Paused));
    session.goal_command(GoalCommand::Resume).unwrap();
    wait_for("resume", || goal_status(&session) == Some(GoalStatus::Active));
}

#[test]
fn stop_and_resume_a_run() {
    let (session, _client, _dir) = open();
    session
        .workflow_command(WorkflowCommand::Stop { run_id: "run-live".into(), reason: None })
        .unwrap();
    wait_for("the run to stop", || run_status(&session, "run-live") == Some(WorkflowStatus::Stopped));
    let snap = session.snapshot();
    let run = snap.workflows.run("run-live").unwrap();
    assert!(run.header.resumable && run.pending_questions.is_empty());
    session
        .workflow_command(WorkflowCommand::Resume { run_id: "run-live".into() })
        .unwrap();
    wait_for("the run to resume", || run_status(&session, "run-live") == Some(WorkflowStatus::Running));
}

#[test]
fn a_workflow_agents_question_rides_the_question_panel() {
    let (session, _client, _dir) = open();
    let input = session.composer().open_input.clone().expect("a waiting agent asks");
    assert_eq!(input.request_id, "workflow:run-live:q1");
    let q = &input.questions[0];
    assert!(q.header.starts_with("review-6 · security-review"), "{}", q.header);
    assert!(q.question.contains("third_party") && q.multiline && q.options.is_empty());

    // An empty answer is refused; a real one answers the run.
    let answer = |text: &str| {
        vec![UserInputAnswer { question_id: "q1".into(), labels: vec![text.into()] }]
    };
    assert!(matches!(
        session.respond_input(&input.request_id, answer("  ")),
        Err(ClientError::InvalidArgument(_))
    ));
    session.respond_input(&input.request_id, answer("Yes, include it")).unwrap();
    wait_for("the question to clear", || {
        session.snapshot().workflows.run("run-live").is_some_and(|r| r.pending_questions.is_empty())
    });
    wait_for("the panel to empty", || session.composer().open_input.is_none());
}

#[test]
fn artifacts_read_through_the_hosts_rpc() {
    let (session, _client, _dir) = open();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let page = rt.block_on(session.workflow_artifact("run-live", "summary")).unwrap();
    assert_eq!(page.content_type, "text/markdown");
    assert!(page.text.as_deref().unwrap().starts_with("## Summary"));
    assert_eq!(page.total as usize, page.text.as_deref().unwrap().len());
    let table = rt.block_on(session.workflow_artifact("run-live", "findings")).unwrap();
    assert!(table.text.unwrap().contains("\"columns\""));
}
