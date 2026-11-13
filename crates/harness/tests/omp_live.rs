//! The real omp, signed in to a provider: a few tiny turns (they bill the
//! signed-in account). Sessions go to a temporary `--session-dir`, never
//! the user's store. Run with:
//!   ZERON_OMP_LIVE=1 cargo test -p zeron-harness --test omp_live -- --ignored --nocapture
#![cfg(unix)]
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{
    CancellationToken, Harness, HarnessError, OmpHarness, RunControls, SteerMessage, TurnControl,
};
use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, ReasoningLevel, RunRequest, SandboxLevel, ToolCall,
    UserInputAnswer,
};

type Events = futures::stream::BoxStream<'static, Result<AgentEvent, HarnessError>>;

fn live() -> bool {
    if std::env::var_os("ZERON_OMP_LIVE").is_none() {
        eprintln!("skipped: set ZERON_OMP_LIVE=1 to run against the real omp");
        return false;
    }
    true
}

fn request(cwd: &std::path::Path, prompt: &str, access: AccessMode) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: Some(ReasoningLevel::Low),
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access,
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env: Default::default(),
        extra_mcp: vec![],
    }
}

struct Live {
    steer: mpsc::Sender<SteerMessage>,
    interrupt: CancellationToken,
    turn: TurnControl,
}

fn controls(answer: &'static str) -> (RunControls, Live) {
    let (steer, steering) = mpsc::channel(8);
    let interrupt = CancellationToken::new();
    let turn = TurnControl::default();
    (
        RunControls {
            realtime: None,
            execution_lease: None,
            steering,
            interrupt: interrupt.clone(),
            request_input: Box::new(move |questions| {
                eprintln!("asked: {questions:?}");
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(
                    questions
                        .iter()
                        .map(|q| UserInputAnswer {
                            question_id: q.id.clone(),
                            labels: vec![answer.into()],
                        })
                        .collect(),
                );
                rx
            }),
            turn: turn.clone(),
        },
        Live {
            steer,
            interrupt,
            turn,
        },
    )
}

async fn turn(events: &mut Events, on: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let mut seen = vec![];
    let mut fired = false;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(180), events.next())
            .await
            .unwrap_or_else(|_| panic!("no Done; saw {seen:?}"))
            .unwrap_or_else(|| panic!("stream ended; saw {seen:?}"))
            .unwrap();
        eprintln!(
            "event: {}",
            format!("{event:?}").chars().take(240).collect::<String>()
        );
        let done = matches!(event, AgentEvent::Done { .. });
        if !fired && on(&event) {
            fired = true;
        }
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn done(events: &[AgentEvent]) -> (DoneStatus, Option<String>) {
    match events.last() {
        Some(AgentEvent::Done {
            status, session_id, ..
        }) => (*status, session_id.clone()),
        other => panic!("not done: {other:?}"),
    }
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
#[ignore = "needs a signed-in omp >= 18.7 (ZERON_OMP_LIVE=1); bills a few tiny turns"]
async fn real_omp_turns_tools_steer_stop_and_resume() {
    if !live() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("alpha.txt"), "a").unwrap();
    std::fs::write(work.join("beta.txt"), "b").unwrap();
    let harness = OmpHarness::new().with_session_dir(dir.path().join("sessions"));

    let models = harness.models().await.unwrap();
    assert!(models.iter().any(|m| m.default), "{models:?}");
    let commands = harness.commands_for(&work).await.unwrap();
    assert!(commands.iter().any(|c| c.name == "compact"));

    // 1. A tiny turn.
    let (ctl, live) = controls("No");
    let mut events = harness
        .run(
            request(&work, "Reply with only the word ok", AccessMode::FullAccess),
            ctl,
        )
        .await
        .unwrap();
    let first = turn(&mut events, |_| false).await;
    let (status, session) = done(&first);
    assert_eq!(status, DoneStatus::Completed);
    assert!(text(&first).to_lowercase().contains("ok"), "{first:?}");
    let session = session.expect("a session id");

    // 2. A local slash command on the same runtime: no model turn.
    live.steer
        .send(SteerMessage::text("/session info"))
        .await
        .unwrap();
    let local = turn(&mut events, |_| false).await;
    assert_eq!(done(&local).0, DoneStatus::Completed);
    assert!(text(&local).contains(&session), "{local:?}");

    // 3. A tool call.
    live.steer
        .send(SteerMessage::text(
            "Use the bash tool to run exactly: ls. Then reply with the file names.",
        ))
        .await
        .unwrap();
    let tool = turn(&mut events, |_| false).await;
    assert!(tool.iter().any(|e| matches!(e, AgentEvent::ToolCall { call: ToolCall::Exec { command }, .. } if command.contains("ls"))), "{tool:?}");
    assert!(tool.iter().any(|e| matches!(e, AgentEvent::ToolResult { output: Some(out), .. } if out.contains("alpha.txt"))));
    assert_eq!(done(&tool).0, DoneStatus::Completed);

    // 4. A stop in place, mid-command.
    live.steer
        .send(SteerMessage::text(
            "Use the bash tool to run exactly: sleep 60",
        ))
        .await
        .unwrap();
    let stop = live.turn.clone();
    let stopped = {
        let mut seen = vec![];
        loop {
            let event = tokio::time::timeout(Duration::from_secs(180), events.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if matches!(event, AgentEvent::ToolCall { .. }) {
                tokio::time::sleep(Duration::from_secs(2)).await;
                stop.stop_turn();
            }
            let end = matches!(event, AgentEvent::Done { .. });
            seen.push(event);
            if end {
                break seen;
            }
        }
    };
    assert_eq!(done(&stopped).0, DoneStatus::Interrupted, "{stopped:?}");
    live.interrupt.cancel();
    while events.next().await.is_some() {}

    // 5. Resume the session in a new runtime.
    let (ctl, live) = controls("No");
    let mut resumed = request(
        &work,
        "Reply with only the word again",
        AccessMode::FullAccess,
    );
    resumed.resume = Some(session.clone());
    let mut events = harness.run(resumed, ctl).await.unwrap();
    let again = turn(&mut events, |_| false).await;
    assert!(
        matches!(&again[0], AgentEvent::SessionStarted { session_id, .. } if *session_id == session)
    );
    assert_eq!(done(&again), (DoneStatus::Completed, Some(session.clone())));
    live.interrupt.cancel();
    while events.next().await.is_some() {}

    // 6. Supervised: the command is asked about, and refused.
    let (ctl, live) = controls("No");
    let mut events = harness
        .run(
            request(
                &work,
                "Use the bash tool to run exactly: echo zeron-live. Then reply with one word.",
                AccessMode::Supervised,
            ),
            ctl,
        )
        .await
        .unwrap();
    let refused = turn(&mut events, |_| false).await;
    assert!(refused.iter().any(|e| matches!(e, AgentEvent::ToolResult { is_error: true, output: Some(out), .. } if out.contains("denied"))), "{refused:?}");
    assert_eq!(done(&refused).0, DoneStatus::Completed);
    live.interrupt.cancel();
    while events.next().await.is_some() {}
}
