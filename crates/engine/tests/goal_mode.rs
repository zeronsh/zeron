//! Goal mode end to end against a real engine core: the controller, the
//! command plane, the queue, the transcript markers and restart recovery. The
//! agent is a scripted harness; the verifier is a [`FakeAsk`] (the child-ask
//! mechanics have their own tests in `ask_child.rs`).

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use support::*;
use tokio::sync::Notify;
use zeron_doc::{MessageRole, SessionCommandPayload, SessionCommandStatus};
use zeron_engine::ask::{AskError, AskUsage, FakeAsk, FakeReply};
use zeron_proto::{
    Goal, GoalCommand, GoalEventKind, GoalLimits, GoalReasonKind, GoalStatus, MessageOrigin,
    SandboxLevel, ToolCall, VerdictOutcome,
};

const CHAT: &str = "main";

struct Rig {
    env: Env,
    ask: Arc<FakeAsk>,
    _dir: tempfile::TempDir,
}

fn not_satisfied(next: &str) -> serde_json::Value {
    json!({"passed": false, "reason": format!("missing: {next}"), "nextAction": next})
}

fn pass() -> serde_json::Value {
    json!({"passed": true, "reason": "ran the checks: all good"})
}

fn rig_with(handler: Handler) -> Rig {
    zeron_engine::goal::set_restart_grace(Duration::from_millis(300));
    let dir = tempfile::tempdir().unwrap();
    let env = assemble(&dir.path().join("data"), Default::default(), handler);
    env.chat(CHAT, SandboxLevel::WorkspaceWrite);
    let ask = FakeAsk::new();
    env.core.doc_host.set_ask_backend(ask.clone());
    Rig {
        env,
        ask,
        _dir: dir,
    }
}

/// Every turn completes at once, spending 400 input + 200 output tokens.
fn quick_agent() -> Handler {
    Arc::new(|_, request, out, _controls| {
        text_turn(
            &out,
            &format!("worked on: {}", request.prompt.len()),
            Some((400, 200)),
        );
    })
}

fn rig() -> Rig {
    rig_with(quick_agent())
}

fn goal_of(rig: &Rig) -> Goal {
    rig.env.goal(CHAT).expect("the chat has a goal")
}

async fn wait_status(rig: &Rig, status: GoalStatus) {
    wait_for(
        || rig.env.goal(CHAT).is_some_and(|g| g.status == status),
        &format!("goal status {status:?}"),
    )
    .await;
}

fn run_command(core: &zeron_engine::EngineCore, text: &str, id: &str) {
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: zeron_proto::RunRequest {
                    mcp: None,
                    prompt: text.into(),
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: "/tmp".into(),
                    sandbox: SandboxLevel::WorkspaceWrite,
                    auto_approve: true,
                    attachments: Vec::new(),
                    worktree: None,
                    resume: None,
                },
                message_id: id.into(),
            },
        )
        .unwrap();
}

fn markers(rig: &Rig) -> Vec<GoalEventKind> {
    rig.env
        .entries(CHAT)
        .into_iter()
        .filter(|e| e.role == MessageRole::System)
        .filter_map(|e| match e.origin {
            Some(MessageOrigin::GoalEvent { event, .. }) => Some(event),
            _ => None,
        })
        .collect()
}

fn queue_is_empty(rig: &Rig) -> bool {
    rig.env
        .core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_queue()
        .unwrap()
        .is_empty()
}

#[tokio::test]
async fn the_chat_keeps_going_until_the_verifier_passes() {
    let rig = rig();
    rig.ask.push_result(not_satisfied("Add the missing test"));
    rig.ask.push_result(pass());
    rig.env.set_goal(CHAT, "Make the suite pass");
    wait_status(&rig, GoalStatus::Complete).await;

    let goal = goal_of(&rig);
    assert_eq!(goal.iteration, 2);
    assert_eq!(goal.verdicts.len(), 2);
    assert_eq!(goal.verdicts[0].outcome, VerdictOutcome::NotSatisfied);
    assert_eq!(goal.verdicts[1].outcome, VerdictOutcome::Pass);
    assert_eq!(goal.reason.as_ref().unwrap().kind, GoalReasonKind::Verified);
    assert_eq!(goal.tokens_used, 1200, "two agent turns of 600 tokens");
    assert_eq!(goal.pending, None);

    // The agent received the objective wrapped as data, then the verdict.
    let prompts = rig.env.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[0].starts_with("Start working toward the goal"),
        "{}",
        prompts[0]
    );
    assert!(
        prompts[0].contains("<untrusted_objective>\nMake the suite pass\n</untrusted_objective>")
    );
    assert!(prompts[1].starts_with("Continue working toward the active goal"));
    assert!(prompts[1].contains("Add the missing test"));
    assert!(prompts[1].contains("Round") || prompts[1].contains("round 1"));

    // Round prompts are machine-origin messages; markers bracket the rounds.
    let users = rig.env.user_text(CHAT);
    assert_eq!(users.len(), 2);
    assert!(matches!(
        &users[0].1,
        Some(MessageOrigin::Goal { round: 1, title, .. }) if title == "Make the suite pass"
    ));
    assert!(matches!(
        &users[1].1,
        Some(MessageOrigin::Goal { round: 2, title, .. }) if title == "Add the missing test"
    ));
    assert_eq!(
        markers(&rig),
        [
            GoalEventKind::Set,
            GoalEventKind::NotSatisfied,
            GoalEventKind::Complete
        ]
    );

    // The verifier was asked read-only about this chat, with the typed schema.
    let calls = rig.ask.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|c| c.parent_chat_id == CHAT && c.spec.read_only)
    );
    assert!(calls[0].spec.prompt.contains("\"main\""));
    assert!(calls[0].spec.prompt.contains("read_chat"));
    assert_eq!(
        calls[0].spec.result_schema,
        zeron_engine::goal::verdict_schema()
    );
    assert!(queue_is_empty(&rig));
    // Done means done: nothing else is sent.
    stays_false(
        || rig.env.prompts().len() > 2,
        Duration::from_millis(300),
        "extra round after completion",
    )
    .await;
}

#[tokio::test]
async fn verifier_cost_is_accounted_separately_from_the_agents() {
    let rig = rig();
    rig.ask.push(FakeReply::ResultWithUsage(
        pass(),
        AskUsage {
            input_tokens: 700,
            output_tokens: 300,
            elapsed_ms: 5_000,
            turns: 1,
        },
    ));
    rig.env.set_goal(CHAT, "x");
    wait_status(&rig, GoalStatus::Complete).await;
    let goal = goal_of(&rig);
    assert_eq!(goal.tokens_used, 600);
    assert_eq!(goal.verifier_tokens_used, 1000);
    assert_eq!(goal.verdicts[0].verifier_tokens, Some(1000));
    assert!(goal.time_used_seconds >= 5);
}

#[tokio::test]
async fn a_user_interrupt_pauses_the_goal_and_resume_continues_it() {
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    let rig = rig_with(Arc::new(move |index, _request, out, controls| {
        let held = held.clone();
        tokio::spawn(async move {
            if index == 0 {
                tokio::select! {
                    _ = controls.interrupt.cancelled() => interrupted(&out),
                    _ = held.notified() => text_turn(&out, "late", None),
                }
            } else {
                text_turn(&out, "resumed work", None);
            }
        });
    }));
    rig.env.set_goal(CHAT, "Do the long thing");
    wait_for(
        || rig.env.runs.lock().unwrap().len() == 1,
        "round 1 to start",
    )
    .await;

    rig.env
        .core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    wait_status(&rig, GoalStatus::Paused).await;
    let goal = goal_of(&rig);
    assert_eq!(
        goal.reason.as_ref().unwrap().kind,
        GoalReasonKind::Interrupted
    );

    // Stop means stop: no verifier, no continuation.
    stays_false(
        || !rig.ask.calls().is_empty() || rig.env.runs.lock().unwrap().len() > 1,
        Duration::from_millis(500),
        "work after the user's Stop",
    )
    .await;

    rig.ask.push_result(pass());
    rig.env.goal_command(CHAT, GoalCommand::Resume);
    wait_status(&rig, GoalStatus::Complete).await;
    let prompts = rig.env.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].starts_with("Resume working toward the active goal"),
        "{}",
        prompts[1]
    );
    assert!(markers(&rig).contains(&GoalEventKind::Resumed));
}

#[tokio::test]
async fn a_read_only_chat_records_the_goal_but_never_pursues_it() {
    // Zeron has no plan mode; a read-only chat is its stand-in.
    let rig = rig();
    rig.env.chat("ro", SandboxLevel::ReadOnly);
    rig.env
        .core
        .doc_host
        .queue_command(
            "ro",
            SessionCommandPayload::Goal {
                issuer: None,
                command: GoalCommand::Set {
                    objective: "Look around".into(),
                    limits: Default::default(),
                    replace: false,
                },
            },
        )
        .unwrap();
    wait_for(
        || {
            rig.env
                .goal("ro")
                .is_some_and(|g| g.status == GoalStatus::Paused)
        },
        "read-only goal to settle",
    )
    .await;
    let goal = rig.env.goal("ro").unwrap();
    assert_eq!(goal.reason.as_ref().unwrap().kind, GoalReasonKind::ReadOnly);
    assert!(rig.env.runs.lock().unwrap().is_empty());
    assert!(rig.ask.calls().is_empty());
}

#[tokio::test]
async fn a_queued_user_message_goes_first_and_the_goal_continues_after_it() {
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    let rig = rig_with(Arc::new(move |index, request, out, _controls| {
        let held = held.clone();
        tokio::spawn(async move {
            if index == 0 {
                held.notified().await;
            }
            text_turn(
                &out,
                &format!(
                    "reply to {}",
                    &request.prompt[..5.min(request.prompt.len())]
                ),
                None,
            );
        });
    }));
    rig.ask.push_result(pass());
    run_command(&rig.env.core, "hello there", "u-hello");
    wait_for(|| rig.env.runs.lock().unwrap().len() == 1, "the first turn").await;
    // The goal arrives mid-turn: accepted, and begins after this turn ends.
    rig.env.set_goal(CHAT, "Finish the work");
    wait_for(|| rig.env.goal(CHAT).is_some(), "goal to be accepted").await;
    assert_eq!(
        goal_of(&rig).iteration,
        0,
        "nothing is queued while a turn runs"
    );
    // The user also queues a follow-up behind the running turn.
    rig.env
        .core
        .doc_host
        .queue_message(CHAT, "and one more thing", Vec::new())
        .unwrap();
    gate.notify_one();

    wait_status(&rig, GoalStatus::Complete).await;
    let prompts = rig.env.prompts();
    assert_eq!(prompts.len(), 3, "{prompts:?}");
    assert_eq!(prompts[0], "hello there");
    assert_eq!(
        prompts[1], "and one more thing",
        "user input outranks the controller"
    );
    assert!(prompts[2].starts_with("Start working toward the goal"));
}

#[tokio::test]
async fn the_round_cap_stops_the_goal_and_resuming_grants_another_allowance() {
    let rig = rig();
    let n = Arc::new(Mutex::new(0));
    rig.ask.on_call(move |_| {
        let mut n = n.lock().unwrap();
        *n += 1;
        Some(FakeReply::Result(not_satisfied(&format!("step {n}"))))
    });
    rig.env.goal_command(
        CHAT,
        GoalCommand::Set {
            objective: "Never satisfied".into(),
            limits: GoalLimits {
                max_rounds: Some(2),
                ..Default::default()
            },
            replace: false,
        },
    );
    wait_status(&rig, GoalStatus::BudgetLimited).await;
    let goal = goal_of(&rig);
    assert_eq!(goal.iteration, 2);
    assert_eq!(
        goal.reason.as_ref().unwrap().kind,
        GoalReasonKind::MaxRounds
    );
    assert_eq!(rig.env.runs.lock().unwrap().len(), 2);
    stays_false(
        || rig.env.runs.lock().unwrap().len() > 2,
        Duration::from_millis(400),
        "round past the cap",
    )
    .await;
    assert!(markers(&rig).contains(&GoalEventKind::BudgetLimited));

    rig.env.goal_command(CHAT, GoalCommand::Resume);
    wait_for(
        || {
            rig.env
                .goal(CHAT)
                .is_some_and(|g| g.status == GoalStatus::BudgetLimited && g.iteration == 4)
        },
        "the second allowance to be used up",
    )
    .await;
    assert_eq!(goal_of(&rig).extensions, 1);
    assert_eq!(rig.env.runs.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn the_token_budget_stops_the_goal_before_paying_for_another_verifier() {
    let rig = rig();
    rig.ask.push_result(not_satisfied("go on"));
    rig.env.goal_command(
        CHAT,
        GoalCommand::Set {
            objective: "Budgeted".into(),
            limits: GoalLimits {
                token_budget: Some(1000),
                ..Default::default()
            },
            replace: false,
        },
    );
    wait_status(&rig, GoalStatus::BudgetLimited).await;
    let goal = goal_of(&rig);
    assert_eq!(
        goal.reason.as_ref().unwrap().kind,
        GoalReasonKind::TokenBudget
    );
    assert_eq!(goal.tokens_used, 1200, "two 600-token turns");
    assert_eq!(rig.env.runs.lock().unwrap().len(), 2);
    assert_eq!(
        rig.ask.calls().len(),
        1,
        "no verifier after the budget was gone"
    );
}

#[tokio::test]
async fn agents_cannot_loop_their_own_chat_or_touch_a_persons_goal() {
    // Hold every turn open (the event sender is never dropped) so goals sit
    // still while commands are tried.
    let rig = rig_with(Arc::new(|_, _, out, _| std::mem::forget(out)));
    let set = |objective: &str| GoalCommand::Set {
        objective: objective.into(),
        limits: GoalLimits::default(),
        replace: true,
    };
    // An agent can't put its own chat into a loop.
    rig.env.goal_command_as(CHAT, set("Keep myself busy"), CHAT);
    stays_false(
        || rig.env.goal(CHAT).is_some(),
        Duration::from_millis(400),
        "an agent's goal on its own chat",
    )
    .await;

    // A person's goal: no agent can pause, replace or clear it.
    rig.env.set_goal(CHAT, "The person's objective");
    wait_for(|| rig.env.goal(CHAT).is_some(), "the person's goal").await;
    let original = goal_of(&rig);
    assert_eq!(original.set_by_agent, None);
    for command in [GoalCommand::Pause, set("Say hi"), GoalCommand::Clear] {
        rig.env.goal_command_as(CHAT, command, "side-chat");
    }
    stays_false(
        || {
            rig.env
                .goal(CHAT)
                .is_none_or(|g| g.id != original.id || g.status == GoalStatus::Paused)
        },
        Duration::from_millis(500),
        "an agent changing a person's goal",
    )
    .await;

    // An agent-set goal records its setter; only that agent manages it.
    rig.env.goal_command(CHAT, GoalCommand::Clear);
    wait_for(|| rig.env.goal(CHAT).is_none(), "the person clears it").await;
    rig.env
        .goal_command_as(CHAT, set("Supervised work"), "boss");
    wait_for(|| rig.env.goal(CHAT).is_some(), "the agent's goal").await;
    assert_eq!(goal_of(&rig).set_by_agent.as_deref(), Some("boss"));
    rig.env
        .goal_command_as(CHAT, GoalCommand::Pause, "another-agent");
    stays_false(
        || {
            rig.env
                .goal(CHAT)
                .is_some_and(|g| g.status == GoalStatus::Paused)
        },
        Duration::from_millis(400),
        "a different agent pausing it",
    )
    .await;
    rig.env.goal_command_as(CHAT, GoalCommand::Pause, "boss");
    wait_status(&rig, GoalStatus::Paused).await;
}

#[tokio::test]
async fn a_round_that_cannot_be_sent_pauses_the_goal_instead_of_looping() {
    let rig = rig();
    // A chat on a harness this engine doesn't have (uninstalled or disabled):
    // every attempt to send its round prompt fails.
    let chat = "unsendable";
    rig.env
        .core
        .workspace
        .create_chat(
            chat,
            Some("space-main"),
            None,
            Some(zeron_proto::ChatConfig {
                harness: zeron_proto::HarnessId::Codex,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            None,
        )
        .unwrap();
    rig.env.set_goal(chat, "Never deliverable");
    wait_for(
        || {
            rig.env
                .goal(chat)
                .is_some_and(|g| g.status == GoalStatus::Paused)
        },
        "the goal pauses on the failed send",
    )
    .await;
    let goal = rig.env.goal(chat).unwrap();
    assert_eq!(
        goal.reason.as_ref().unwrap().kind,
        GoalReasonKind::TurnFailed
    );
    // Paused, not retrying: the goal and its queue stay still.
    let settled = rig.env.goal(chat).unwrap().updated_at;
    stays_false(
        || {
            rig.env.goal(chat).is_some_and(|g| g.updated_at != settled)
                || !rig.env.runs.lock().unwrap().is_empty()
        },
        Duration::from_millis(800),
        "retrying the failed send on its own",
    )
    .await;
}

#[tokio::test]
async fn a_failing_verifier_pauses_the_goal_instead_of_looping() {
    let rig = rig();
    rig.ask
        .push(FakeReply::Fail(AskError::Timeout(Duration::from_secs(600))));
    rig.env.set_goal(CHAT, "Verified by a flaky verifier");
    wait_status(&rig, GoalStatus::Paused).await;
    let goal = goal_of(&rig);
    assert_eq!(
        goal.reason.as_ref().unwrap().kind,
        GoalReasonKind::VerifierFailed
    );
    assert!(
        goal.reason
            .as_ref()
            .unwrap()
            .message
            .contains("did not answer")
    );
    assert_eq!(
        goal.verdicts.last().unwrap().outcome,
        VerdictOutcome::Failed
    );
    assert!(markers(&rig).contains(&GoalEventKind::VerifierFailed));
    stays_false(
        || rig.env.runs.lock().unwrap().len() > 1 || rig.ask.calls().len() > 1,
        Duration::from_millis(500),
        "retrying a failed verifier on its own",
    )
    .await;
    // Resuming is the retry.
    rig.ask.push_result(pass());
    rig.env.goal_command(CHAT, GoalCommand::Resume);
    wait_status(&rig, GoalStatus::Complete).await;
}

#[tokio::test]
async fn the_same_next_step_twice_with_no_work_in_between_is_no_progress() {
    let rig = rig();
    rig.ask
        .on_call(|_| Some(FakeReply::Result(not_satisfied("Run the tests"))));
    rig.env.set_goal(CHAT, "Stalls");
    wait_status(&rig, GoalStatus::Paused).await;
    let goal = goal_of(&rig);
    assert_eq!(
        goal.reason.as_ref().unwrap().kind,
        GoalReasonKind::NoProgress
    );
    assert_eq!(goal.iteration, 2);
    assert_eq!(rig.env.runs.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn real_work_between_identical_verdicts_is_not_a_stall() {
    let rig = rig_with(Arc::new(|index, _request, out, _controls| {
        tool_call(
            &out,
            &format!("t{index}"),
            ToolCall::Exec {
                command: "cargo test".into(),
            },
        );
        text_turn(&out, "ran something", None);
    }));
    let calls = Arc::new(Mutex::new(0));
    rig.ask.on_call(move |_| {
        let mut n = calls.lock().unwrap();
        *n += 1;
        Some(FakeReply::Result(if *n < 3 {
            not_satisfied("Run the tests")
        } else {
            pass()
        }))
    });
    rig.env.set_goal(CHAT, "Keeps working");
    wait_status(&rig, GoalStatus::Complete).await;
    assert_eq!(rig.env.runs.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn setting_replacing_pausing_and_clearing_follow_the_documented_rules() {
    let gate = Arc::new(Notify::new());
    let held = gate.clone();
    let rig = rig_with(Arc::new(move |_, _request, out, controls| {
        let held = held.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = controls.interrupt.cancelled() => interrupted(&out),
                _ = held.notified() => text_turn(&out, "ok", None),
            }
        });
    }));
    let doc = rig.env.core.doc_host.open(CHAT).unwrap();
    let statuses =
        |doc: &zeron_engine::ChatDocHandle| -> Vec<(SessionCommandStatus, Option<String>)> {
            doc.doc()
                .read_commands()
                .unwrap()
                .into_iter()
                .map(|c| (c.status, c.resolution))
                .collect()
        };
    // Over the RPC, the way an MCP tool or a client sends it.
    let client = zeron_rpc::memory_client(rig.env.core.rpc_service());
    let send = |command: serde_json::Value| {
        let client = &client;
        async move {
            client
                .call(
                    zeron_rpc::methods::QUEUE_COMMAND,
                    json!({"chatId": CHAT, "command": {"kind": "goal", "command": command}}),
                )
                .await
                .unwrap();
        }
    };
    send(json!({"action": "set", "objective": "First"})).await;
    wait_for(|| rig.env.goal(CHAT).is_some(), "first goal").await;
    let first = goal_of(&rig);

    // A second set without replace is refused and changes nothing.
    send(json!({"action": "set", "objective": "Second"})).await;
    wait_for(
        || {
            statuses(&doc)
                .iter()
                .any(|(s, _)| *s == SessionCommandStatus::Rejected)
        },
        "rejection",
    )
    .await;
    assert_eq!(goal_of(&rig).id, first.id);
    assert!(statuses(&doc).iter().any(|(_, r)| {
        r.as_deref()
            .is_some_and(|r| r.contains("already has a goal"))
    }));

    // Oversized objectives are refused too.
    send(json!({"action": "set", "objective": "x".repeat(4001), "replace": true})).await;
    wait_for(
        || {
            statuses(&doc)
                .iter()
                .filter(|(s, _)| *s == SessionCommandStatus::Rejected)
                .count()
                == 2
        },
        "length rejection",
    )
    .await;
    assert_eq!(goal_of(&rig).id, first.id);

    // Replace supersedes it.
    send(json!({"action": "set", "objective": "Second", "replace": true})).await;
    wait_for(
        || rig.env.goal(CHAT).is_some_and(|g| g.objective == "Second"),
        "replacement",
    )
    .await;
    assert_ne!(goal_of(&rig).id, first.id);

    // The transcript watch carries the goal (and its removal).
    let mut watch = client
        .subscribe_scoped(
            zeron_rpc::methods::WATCH_DOC_MESSAGES,
            json!({"chatId": CHAT}),
        )
        .await
        .unwrap();
    let opening = watch.recv().await.unwrap();
    assert_eq!(opening["goal"]["objective"], "Second");

    send(json!({"action": "pause"})).await;
    wait_status(&rig, GoalStatus::Paused).await;
    send(json!({"action": "clear"})).await;
    wait_for(|| rig.env.goal(CHAT).is_none(), "goal to clear").await;
    let mut cleared = false;
    while let Ok(Some(frame)) = tokio::time::timeout(Duration::from_secs(5), watch.recv()).await {
        if frame["goalCleared"] == true {
            cleared = true;
            break;
        }
    }
    assert!(cleared, "the watch reports the removal");
    assert!(markers(&rig).contains(&GoalEventKind::Cleared));
}

#[tokio::test]
async fn pausing_or_clearing_while_the_verifier_runs_cancels_it() {
    let rig = rig();
    rig.ask.push(FakeReply::Hang);
    rig.env.set_goal(CHAT, "Slow to verify");
    wait_status(&rig, GoalStatus::Verifying).await;
    wait_for(|| rig.ask.calls().len() == 1, "verifier to start").await;
    rig.env.goal_command(CHAT, GoalCommand::Pause);
    wait_status(&rig, GoalStatus::Paused).await;
    wait_for(|| rig.ask.cancelled() == 1, "the verifier to be cancelled").await;
    assert_eq!(
        goal_of(&rig).reason.as_ref().unwrap().kind,
        GoalReasonKind::User
    );

    // Resume, verify again, then clear mid-verification.
    rig.ask.push(FakeReply::Hang);
    rig.env.goal_command(CHAT, GoalCommand::Resume);
    wait_for(|| rig.ask.calls().len() == 2, "second verifier").await;
    rig.env.goal_command(CHAT, GoalCommand::Clear);
    wait_for(|| rig.env.goal(CHAT).is_none(), "clear").await;
    wait_for(
        || rig.ask.cancelled() == 2,
        "the second verifier to be cancelled",
    )
    .await;
}

/// The controller hooks (turn completed, queue-flush watcher, status change)
/// all end in `goal_tick`. Hammer it from several threads while verdicts land:
/// whenever one of those ticks falls between a verifier returning and its
/// verdict being applied, it must see a verification still in flight, not
/// start a second one for the same round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tick_between_a_verdict_returning_and_being_applied_starts_no_second_verifier() {
    use std::sync::atomic::{AtomicBool, Ordering};

    const ROUNDS: usize = 20;
    let rig = rig();
    for n in 1..ROUNDS {
        rig.ask.push(FakeReply::After(
            Duration::from_millis(3),
            Box::new(FakeReply::Result(not_satisfied(&format!("step {n}")))),
        ));
    }
    rig.ask.push(FakeReply::After(
        Duration::from_millis(3),
        Box::new(FakeReply::Result(pass())),
    ));

    let host = rig.env.core.doc_host.clone();
    let handle = host.open(CHAT).unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let hammers: Vec<_> = (0..6)
        .map(|_| {
            let (host, handle, done) = (host.clone(), handle.clone(), done.clone());
            tokio::spawn(async move {
                while !done.load(Ordering::Acquire) {
                    host.goal_tick(&handle).await;
                    tokio::task::yield_now().await;
                }
            })
        })
        .collect();

    rig.env.set_goal(CHAT, "Verify every step once");
    wait_for(
        || {
            rig.env
                .goal(CHAT)
                .is_some_and(|g| matches!(g.status, GoalStatus::Complete | GoalStatus::Paused))
        },
        "the goal to finish",
    )
    .await;
    done.store(true, Ordering::Release);
    for hammer in hammers {
        hammer.await.unwrap();
    }
    // A late second verifier would still be asking; give it room to show up.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let goal = goal_of(&rig);
    assert_eq!(goal.status, GoalStatus::Complete, "{:?}", goal.reason);
    let rounds: Vec<u32> = rig
        .ask
        .calls()
        .iter()
        .map(|c| {
            let at = c.spec.prompt.find("round ").map_or(0, |i| i + 6);
            c.spec.prompt[at..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(0)
        })
        .collect();
    assert_eq!(
        rig.ask.calls().len(),
        ROUNDS,
        "one verifier per round, got rounds {rounds:?}"
    );
    assert_eq!(goal.iteration as usize, ROUNDS);
    assert_eq!(goal.verdicts.len(), ROUNDS);
}

// ── restart recovery ───────────────────────────────────────────────────────

#[tokio::test]
async fn a_verification_interrupted_by_a_restart_is_run_again() {
    zeron_engine::goal::set_restart_grace(Duration::from_millis(300));
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    {
        let env = assemble(&data, Default::default(), quick_agent());
        env.chat(CHAT, SandboxLevel::WorkspaceWrite);
        let ask = FakeAsk::new();
        ask.push(FakeReply::Hang);
        env.core.doc_host.set_ask_backend(ask.clone());
        env.set_goal(CHAT, "Survive a restart");
        wait_for(
            || {
                env.goal(CHAT)
                    .is_some_and(|g| g.status == GoalStatus::Verifying)
            },
            "verifying",
        )
        .await;
        wait_for(|| ask.calls().len() == 1, "verifier started").await;
        env.core.shutdown().await;
    }
    let env = assemble(&data, Default::default(), quick_agent());
    let ask = FakeAsk::new();
    ask.push_result(pass());
    env.core.doc_host.set_ask_backend(ask.clone());
    wait_for(
        || {
            env.goal(CHAT)
                .is_some_and(|g| g.status == GoalStatus::Complete)
        },
        "recovered verification to complete",
    )
    .await;
    assert_eq!(
        ask.calls().len(),
        1,
        "the dead verifier was replaced by one new one"
    );
    assert_eq!(
        env.runs.lock().unwrap().len(),
        0,
        "the round was not re-run, only re-judged"
    );
}

#[tokio::test]
async fn a_crashed_round_resumes_once_and_never_double_continues() {
    use zeron_doc::{MessagePart, MessageStatus, SessionDoc, SessionMessageEntry};
    zeron_engine::goal::set_restart_grace(Duration::from_millis(300));
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("device-id"), "dev-crash").unwrap();
    {
        // A chat row first (an engine that ran this chat before the crash).
        let env = assemble(&data, Default::default(), quick_agent());
        env.chat(CHAT, SandboxLevel::WorkspaceWrite);
        env.core.shutdown().await;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    {
        let store = zeron_sync::DocsStore::open(data.join("orgs/dev-org/dev-user")).unwrap();
        let doc = SessionDoc::init(CHAT).unwrap();
        let mut goal = Goal::new(
            "g1",
            "Survive a crash",
            &GoalLimits::default(),
            now - 90_000,
        )
        .unwrap();
        goal.iteration = 1;
        goal.pending = Some(zeron_proto::GoalPending {
            kind: zeron_proto::GoalPendingKind::Turn,
            round: 1,
            message_id: goal.round_message_id(1),
            started_at: now - 60_000,
        });
        doc.set_goal(&goal).unwrap();
        let entry = |id: String, role, status, text: &str, origin| SessionMessageEntry {
            origin,
            id,
            role,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: now - 60_000,
            device_id: "dev-crash".into(),
            status: Some(status),
            continuation_of: None,
            duration_ms: None,
        };
        doc.push_message(&entry(
            goal.round_message_id(1),
            MessageRole::User,
            MessageStatus::Complete,
            "Start working toward the goal for this chat.",
            Some(MessageOrigin::Goal {
                goal_id: "g1".into(),
                round: 1,
                title: "Survive a crash".into(),
            }),
        ))
        .unwrap();
        doc.push_message(&entry(
            "a1".into(),
            MessageRole::Assistant,
            MessageStatus::Streaming,
            "partial…",
            None,
        ))
        .unwrap();
        store
            .save_snapshot(CHAT, &doc.export_snapshot().unwrap())
            .unwrap();
        let journal =
            zeron_engine::RunJournal::open(data.join("orgs/dev-org/dev-user/journals")).unwrap();
        journal
            .append(
                CHAT,
                &zeron_proto::AgentEvent::TextDelta {
                    text: "partial…".into(),
                },
            )
            .unwrap();
        std::fs::write(
            data.join("orgs/dev-org/dev-user/goals.json"),
            serde_json::to_vec(&[CHAT]).unwrap(),
        )
        .unwrap();
    }
    let env = assemble(&data, Default::default(), quick_agent());
    let ask = FakeAsk::new();
    ask.push_result(pass());
    env.core.doc_host.set_ask_backend(ask.clone());
    wait_for(
        || {
            env.goal(CHAT)
                .is_some_and(|g| g.status == GoalStatus::Complete)
        },
        "the revived round to be verified",
    )
    .await;
    // The crashed round was revived under its own message id: one run, one
    // user entry, no second round.
    assert_eq!(env.runs.lock().unwrap().len(), 1);
    let users = env.user_text(CHAT);
    assert_eq!(users.len(), 1, "{users:?}");
    assert_eq!(env.goal(CHAT).unwrap().iteration, 1);
    assert_eq!(ask.calls().len(), 1);
    stays_false(
        || env.runs.lock().unwrap().len() > 1,
        Duration::from_millis(500),
        "a duplicate continuation",
    )
    .await;
}

#[tokio::test]
async fn a_chat_hosted_elsewhere_never_runs_the_controller() {
    zeron_engine::goal::set_restart_grace(Duration::from_millis(300));
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let make_doc = |chat: &str| {
        let store = zeron_sync::DocsStore::open(data.join("orgs/dev-org/dev-user")).unwrap();
        let doc = zeron_doc::SessionDoc::init(chat).unwrap();
        doc.set_goal(&Goal::new("g", "Not mine to drive", &GoalLimits::default(), 1).unwrap())
            .unwrap();
        store
            .save_snapshot(chat, &doc.export_snapshot().unwrap())
            .unwrap();
    };
    let device;
    {
        let env = assemble(&data, Default::default(), quick_agent());
        device = env.core.device_id.clone();
        env.chat("local", SandboxLevel::WorkspaceWrite);
        // A project-less chat hosted by some other device.
        env.core
            .workspace
            .create_chat(
                "remote",
                None,
                Some("device-elsewhere"),
                None,
                Some("/tmp".into()),
            )
            .unwrap();
        env.core.workspace.rename_chat("remote", "Remote").unwrap();
        env.core.shutdown().await;
    }
    let _ = device;
    make_doc("local");
    make_doc("remote");
    let env = assemble(&data, Default::default(), quick_agent());
    let ask = FakeAsk::new();
    ask.on_call(|_| Some(FakeReply::Result(pass())));
    env.core.doc_host.set_ask_backend(ask.clone());
    env.core.doc_host.open("remote").unwrap();
    env.core.doc_host.open("local").unwrap();
    // The positive control: the hosted chat is driven.
    wait_for(
        || env.runs.lock().unwrap().len() == 1,
        "the hosted chat's round",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let prompts = env.prompts();
    assert_eq!(prompts.len(), 1, "only the hosted chat ran: {prompts:?}");
    let remote = env.core.doc_host.open("remote").unwrap();
    assert!(remote.doc().read_queue().unwrap().is_empty());
    assert!(remote.doc().read_entries().unwrap().is_empty());
    assert_eq!(
        remote.doc().goal().unwrap().iteration,
        0,
        "its goal was never advanced"
    );
    assert!(ask.calls().iter().all(|c| c.parent_chat_id != "remote"));
}
