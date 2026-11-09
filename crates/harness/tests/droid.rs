use futures::StreamExt;
use std::{path::Path, time::Duration};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, DroidHarness, Harness, RunControls, SteerMessage};
use zeron_proto::{AccessMode, AgentEvent, DoneStatus, RunRequest, SandboxLevel, UserInputAnswer};

fn request(cwd: &Path, prompt: &str) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: AccessMode::Supervised,
        plan: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    }
}
fn controls(
    answer: Option<&'static str>,
) -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (sender, steering) = mpsc::channel(8);
    let interrupt = CancellationToken::new();
    (
        RunControls {
            realtime: None,
            execution_lease: None,
            steering,
            interrupt: interrupt.clone(),
            turn: Default::default(),
            request_input: Box::new(move |questions| {
                let (sender, receiver) = oneshot::channel();
                if let Some(label) = answer {
                    let _ = sender.send(
                        questions
                            .iter()
                            .map(|q| UserInputAnswer {
                                question_id: q.id.clone(),
                                labels: label.split(", ").map(str::to_owned).collect(),
                            })
                            .collect(),
                    );
                }
                receiver
            }),
        },
        sender,
        interrupt,
    )
}
fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}
fn status(events: &[AgentEvent]) -> DoneStatus {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            AgentEvent::Done { status, .. } => Some(*status),
            _ => None,
        })
        .expect("Done")
}

#[cfg(feature = "native-fixture")]
mod fixture {
    use super::*;
    use zeron_proto::{ReasoningLevel, ToolCall};
    fn harness() -> DroidHarness {
        DroidHarness::new()
            .with_executable(env!("CARGO_BIN_EXE_harness-droid-fixture"))
            .with_graces(Duration::from_millis(100), Duration::from_millis(100))
    }
    async fn collect(request: RunRequest, answer: Option<&'static str>) -> Vec<AgentEvent> {
        let (controls, sender, _) = controls(answer);
        drop(sender);
        tokio::time::timeout(
            Duration::from_secs(5),
            harness()
                .run(request, controls)
                .await
                .unwrap()
                .map(Result::unwrap)
                .collect(),
        )
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn streams_without_replaying_complete_messages_and_deduplicates_tools() {
        let dir = tempfile::tempdir().unwrap();
        let events = collect(request(dir.path(), "hello"), None).await;
        assert_eq!(text(&events), "reply:hello:done");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::TextDelta { .. }))
                .count(),
            3
        );
        assert_eq!(status(&events), DoneStatus::Completed);
        let events = collect(request(dir.path(), "tools"), None).await;
        let calls: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCall { call, .. } => Some(call),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 1);
        assert!(
            matches!(calls[0],ToolCall::WriteFile { path, content: Some(content) } if path=="note.txt" && content=="hello")
        );
        assert!(events.iter().any(
            |e| matches!(e,AgentEvent::ToolResult { diff:Some(diff),.. } if diff.new_text=="hello")
        ));
    }
    #[tokio::test]
    async fn background_completion_starts_a_parent_turn_without_a_new_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (controls, sender, _) = controls(None);
        let turn = controls.turn.clone();
        let mut stream = harness()
            .run(request(dir.path(), "background"), controls)
            .await
            .unwrap();
        let events = tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = vec![];
            let mut completed = 0;
            while completed < 2 {
                let event = stream.next().await.unwrap().unwrap();
                if matches!(
                    event,
                    AgentEvent::Done {
                        status: DoneStatus::Completed,
                        ..
                    }
                ) {
                    completed += 1;
                }
                events.push(event);
            }
            events
        })
        .await
        .unwrap();
        assert_eq!(text(&events), "initialautonomous");
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
                .count(),
            2
        );
        assert!(!turn.background_live());
        drop(sender);
        while stream.next().await.is_some() {}
    }
    #[tokio::test]
    async fn child_transcripts_stream_separately_and_drain_after_settlement() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = request(dir.path(), "child-transcript");
        req.env.insert(
            "FACTORY_HOME_OVERRIDE".into(),
            dir.path().join("factory").display().to_string(),
        );
        let (controls, sender, _) = controls(None);
        let turn = controls.turn.clone();
        drop(sender);
        let mut stream = harness().run(req, controls).await.unwrap();
        let events = tokio::time::timeout(Duration::from_secs(5), async {
            let mut events = Vec::new();
            let mut saw_live_call = false;
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                if let AgentEvent::Subagent { event, .. } = &event
                    && matches!(event.as_ref(), AgentEvent::ToolCall { .. })
                {
                    saw_live_call = true;
                    assert!(
                        turn.background_live(),
                        "child tools must arrive before settlement"
                    );
                }
                events.push(event);
            }
            assert!(saw_live_call);
            events
        })
        .await
        .unwrap();
        assert_eq!(text(&events), "initialautonomous");
        let spawn = events
            .iter()
            .position(|event| matches!(event, AgentEvent::ToolCall { id, call } if id == "spawn" && call.is_subagent_spawn()))
            .unwrap();
        let tagged: Vec<_> = events
            .iter()
            .enumerate()
            .filter_map(|(ix, event)| match event {
                AgentEvent::Subagent {
                    parent_tool_use_id,
                    event,
                } => {
                    assert_eq!(parent_tool_use_id, "spawn");
                    assert!(ix > spawn);
                    Some(event.as_ref().clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(text(&tagged), "child finished\n\n");
        assert_eq!(status(&tagged), DoneStatus::Completed);
        assert_eq!(
            tagged
                .iter()
                .filter(|e| matches!(e, AgentEvent::ToolCall { .. }))
                .count(),
            1
        );
        assert!(tagged.iter().any(|event| matches!(event, AgentEvent::ToolResult { id, output: Some(output), .. } if id == "child-read" && output == "fixture contents")));
        assert!(!tagged.iter().any(|e| matches!(
            e,
            AgentEvent::UserMessage { .. } | AgentEvent::ReasoningDelta { .. }
        )));
        assert!(!turn.background_live());
    }
    async fn child_events(prompt: &str) -> Vec<AgentEvent> {
        let dir = tempfile::tempdir().unwrap();
        let mut req = request(dir.path(), prompt);
        req.env.insert(
            "FACTORY_HOME_OVERRIDE".into(),
            dir.path().join("factory").display().to_string(),
        );
        let (controls, sender, _) = controls(None);
        drop(sender);
        tokio::time::timeout(
            Duration::from_secs(8),
            harness()
                .run(req, controls)
                .await
                .unwrap()
                .map(Result::unwrap)
                .collect(),
        )
        .await
        .unwrap()
    }
    fn child_parts(events: &[AgentEvent]) -> Vec<AgentEvent> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Subagent { event, .. } => Some(event.as_ref().clone()),
                _ => None,
            })
            .collect()
    }
    #[tokio::test]
    async fn parent_frames_naming_their_session_still_render() {
        let events = child_events("session-ids").await;
        assert_eq!(text(&events), "initialautonomous");
        assert_eq!(status(&events), DoneStatus::Completed);
        let child = child_parts(&events);
        assert_eq!(text(&child), "child finished\n\n");
        assert_eq!(status(&child), DoneStatus::Completed);
    }
    #[tokio::test]
    async fn child_settlement_uses_the_child_id_not_optional_tool_metadata() {
        for prompt in [
            "settle-without-tool",
            "settle-empty-tool",
            "settle-mismatched-tool",
        ] {
            let events = child_events(prompt).await;
            assert_eq!(text(&events), "initialautonomous", "{prompt}");
            let child = child_parts(&events);
            assert_eq!(text(&child), "child finished\n\n", "{prompt}");
            assert_eq!(status(&child), DoneStatus::Completed, "{prompt}");
        }
    }
    #[tokio::test]
    async fn child_text_written_after_the_drain_window_still_renders() {
        let events = child_events("late-append").await;
        let child = child_parts(&events);
        assert_eq!(text(&child), "child finished\n\n");
        assert_eq!(status(&child), DoneStatus::Completed);
        assert!(matches!(child.last(), Some(AgentEvent::Done { .. })));
    }
    #[tokio::test]
    async fn catalog_preserves_ids_efforts_and_uses_normal_cache() {
        let harness = harness();
        let first = harness.model_catalog(false).await.unwrap();
        assert_eq!(first.source, "live");
        assert_eq!(
            first
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["auto", "model-fast"]
        );
        assert!(first.models[0].reasoning_levels.is_empty());
        assert_eq!(
            first.models[1].reasoning_levels,
            [
                ReasoningLevel::Minimal,
                ReasoningLevel::Low,
                ReasoningLevel::Medium,
                ReasoningLevel::High,
                ReasoningLevel::XHigh,
                ReasoningLevel::Max
            ]
        );
        let second = harness.model_catalog(false).await.unwrap();
        assert_eq!(second.source, "cache");
        assert_eq!(first.models, second.models);
    }

    #[tokio::test]
    async fn auto_model_uses_none_for_new_resumed_and_plan_sessions() {
        let dir = tempfile::tempdir().unwrap();
        for plan in [false, true] {
            for resume in [None, Some("droid-fixture-session")] {
                for effort in [None, Some(ReasoningLevel::High)] {
                    let mut req = request(dir.path(), "settings");
                    req.model = Some("auto".into());
                    req.reasoning = effort;
                    req.plan = plan;
                    req.resume = resume.map(str::to_owned);
                    let events = collect(req, None).await;
                    assert_eq!(status(&events), DoneStatus::Completed, "{events:?}");
                    let effective: serde_json::Value =
                        serde_json::from_str(&text(&events)).unwrap();
                    assert_eq!(effective["modelId"], "auto");
                    assert_eq!(effective["specModeModelId"], "auto");
                    assert_eq!(effective["reasoningEffort"], "none");
                    assert_eq!(effective["specModeReasoningEffort"], "none");
                    assert_eq!(
                        effective["interactionMode"],
                        if plan { "spec" } else { "auto" }
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn auto_model_reuse_and_model_switches_keep_effective_reasoning() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = request(dir.path(), "settings");
        req.model = Some("auto".into());
        req.reasoning = Some(ReasoningLevel::High);
        let (controls, sender, _) = controls(None);
        let mut stream = harness().run(req.clone(), controls).await.unwrap();
        let mut expected = ("auto", "none", false);
        for next in [
            None,
            Some(("model-fast", Some(ReasoningLevel::High), false)),
            Some(("auto", Some(ReasoningLevel::High), true)),
            Some(("auto", None, false)),
        ] {
            let events = tokio::time::timeout(Duration::from_secs(5), async {
                let mut events = Vec::new();
                while let Some(event) = stream.next().await {
                    let event = event.unwrap();
                    let done = matches!(event, AgentEvent::Done { .. });
                    events.push(event);
                    if done {
                        break;
                    }
                }
                events
            })
            .await
            .unwrap();
            assert_eq!(status(&events), DoneStatus::Completed, "{events:?}");
            let effective: serde_json::Value = serde_json::from_str(&text(&events)).unwrap();
            assert_eq!(effective["modelId"], expected.0);
            assert_eq!(effective["specModeModelId"], expected.0);
            assert_eq!(effective["reasoningEffort"], expected.1);
            assert_eq!(effective["specModeReasoningEffort"], expected.1);
            assert_eq!(
                effective["interactionMode"],
                if expected.2 { "spec" } else { "auto" }
            );
            let mut steer = SteerMessage::text("settings");
            if let Some((model, effort, plan)) = next {
                let mut config = req.clone();
                config.model = Some(model.into());
                config.reasoning = effort;
                config.plan = plan;
                steer.config = Some(Box::new(config));
                expected = (model, if model == "auto" { "none" } else { "high" }, plan);
            }
            sender.send(steer).await.unwrap();
        }
        drop(sender);
        let events = tokio::time::timeout(
            Duration::from_secs(5),
            stream.map(Result::unwrap).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        assert_eq!(status(&events), DoneStatus::Completed, "{events:?}");
        let effective: serde_json::Value = serde_json::from_str(&text(&events)).unwrap();
        assert_eq!(effective["reasoningEffort"], "none");
        assert_eq!(effective["specModeReasoningEffort"], "none");
        assert_eq!(effective["interactionMode"], "auto");
    }
    #[tokio::test]
    async fn tool_permissions_follow_access_but_plans_always_ask() {
        let dir = tempfile::tempdir().unwrap();
        for access in AccessMode::ALL {
            for kind in ["create", "exec", "batch", "spec", "spec-batch"] {
                for answer in [None, Some("No"), Some("Yes")] {
                    let mut req = request(dir.path(), &format!("permission:{kind}"));
                    req.access = access;
                    let plan = kind.starts_with("spec");
                    req.plan = plan;
                    let events = collect(req, answer).await;
                    if plan {
                        let proposals: Vec<_> = events
                            .iter()
                            .enumerate()
                            .filter_map(|(ix, event)| {
                                let AgentEvent::ToolCall { call, .. } = event else {
                                    return None;
                                };
                                zeron_proto::agent_plans::proposed_plan(call).map(|plan| (ix, plan))
                            })
                            .collect();
                        assert_eq!(proposals.len(), 1);
                        assert_eq!(proposals[0].1.body, "Create note.txt");
                        assert_eq!(proposals[0].1.title, Some("A tiny plan"));
                        let question = events
                            .iter()
                            .position(|e| matches!(e, AgentEvent::InputRequested { .. }))
                            .expect("plans require approval at every access level");
                        assert!(proposals[0].0 < question);
                        assert!(
                            matches!(&events[question], AgentEvent::InputRequested { questions, .. }
                            if questions.len() == 1 && questions[0].header == "Approve plan")
                        );
                    }
                    let automatic = !plan
                        && (access == AccessMode::FullAccess
                            || (access == AccessMode::AutoAcceptEdits && kind == "create"));
                    let allow = automatic || answer == Some("Yes");
                    assert_eq!(
                        text(&events),
                        if !allow {
                            "cancel"
                        } else if plan {
                            match access {
                                AccessMode::Supervised => "proceed_once",
                                AccessMode::AutoAcceptEdits => "proceed_auto_run_medium",
                                AccessMode::FullAccess => "proceed_auto_run_high",
                            }
                        } else {
                            "proceed_once"
                        },
                        "{access:?} {kind} {answer:?}"
                    );
                    assert_eq!(
                        events
                            .iter()
                            .filter(|e| matches!(e, AgentEvent::InputRequested { .. }))
                            .count(),
                        usize::from(!automatic)
                    );
                    assert_eq!(
                        events
                            .iter()
                            .filter(|e| matches!(e, AgentEvent::InputResolved { .. }))
                            .count(),
                        usize::from(!automatic)
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn full_access_plan_waits_for_explicit_answer() {
        let dir = tempfile::tempdir().unwrap();
        for auto_approve in [false, true] {
            for answer in ["No", "Yes"] {
                let mut req = request(dir.path(), "permission:spec");
                req.access = if auto_approve {
                    AccessMode::Supervised
                } else {
                    AccessMode::FullAccess
                };
                req.auto_approve = auto_approve;
                req.plan = true;
                let (mut controls, sender, _) = controls(None);
                let (pending_tx, mut pending_rx) = mpsc::unbounded_channel();
                controls.request_input = Box::new(move |questions| {
                    let (tx, rx) = oneshot::channel();
                    pending_tx.send((questions, tx)).unwrap();
                    rx
                });
                let mut stream = harness().run(req, controls).await.unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        match stream.next().await.unwrap().unwrap() {
                            AgentEvent::InputRequested { .. } => break,
                            AgentEvent::TextDelta { .. } | AgentEvent::Done { .. } => {
                                panic!("Full Access must wait for plan approval");
                            }
                            _ => {}
                        }
                    }
                })
                .await
                .unwrap();
                let (questions, resolver) =
                    tokio::time::timeout(Duration::from_secs(5), pending_rx.recv())
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(questions.len(), 1);
                assert_eq!(questions[0].header, "Approve plan");
                assert!(
                    tokio::time::timeout(Duration::from_millis(50), stream.next())
                        .await
                        .is_err(),
                    "Droid must not proceed or cancel before the person answers"
                );
                resolver
                    .send(vec![UserInputAnswer {
                        question_id: questions[0].id.clone(),
                        labels: vec![answer.into()],
                    }])
                    .unwrap();
                drop(sender);
                let events = tokio::time::timeout(
                    Duration::from_secs(5),
                    stream.map(Result::unwrap).collect::<Vec<_>>(),
                )
                .await
                .unwrap();
                assert_eq!(
                    text(&events),
                    if answer == "Yes" {
                        "proceed_auto_run_high"
                    } else {
                        "cancel"
                    },
                    "auto_approve={auto_approve}, answer={answer}"
                );
                assert_eq!(status(&events), DoneStatus::Completed);
            }
        }
    }
    #[tokio::test]
    async fn plan_approval_preserves_native_autonomy() {
        let dir = tempfile::tempdir().unwrap();
        for (access, auto_approve, level) in [
            (AccessMode::Supervised, false, "off"),
            (AccessMode::AutoAcceptEdits, false, "medium"),
            (AccessMode::FullAccess, false, "high"),
            (AccessMode::Supervised, true, "high"),
        ] {
            for answer in [None, Some("No"), Some("Yes")] {
                let mut req = request(dir.path(), "permission:spec-settings");
                req.access = access;
                req.auto_approve = auto_approve;
                req.plan = true;
                let events = collect(req, answer).await;
                let effective: serde_json::Value = serde_json::from_str(&text(&events)).unwrap();
                assert_eq!(
                    effective["selectedOption"],
                    if answer != Some("Yes") {
                        "cancel".into()
                    } else if level == "off" {
                        "proceed_once".into()
                    } else {
                        format!("proceed_auto_run_{level}")
                    }
                );
                assert_eq!(effective["autonomyLevel"], level, "{effective}");
                assert_eq!(
                    effective["interactionMode"],
                    if answer == Some("Yes") {
                        "auto"
                    } else {
                        "spec"
                    }
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|e| matches!(e, AgentEvent::InputRequested { .. }))
                        .count(),
                    1
                );
                assert_eq!(status(&events), DoneStatus::Completed);
            }
        }
    }
    #[tokio::test]
    async fn unavailable_plan_autonomy_cancels_without_fallback() {
        let dir = tempfile::tempdir().unwrap();
        for access in AccessMode::ALL {
            for kind in ["spec-unavailable", "spec-no-options"] {
                let mut req = request(dir.path(), &format!("permission:{kind}"));
                req.access = access;
                req.plan = true;
                let events = collect(req, Some("Yes")).await;
                assert_eq!(text(&events), "cancel");
                assert!(events.iter().any(|e| matches!(e, AgentEvent::Error { message } if message.contains("not offered"))));
                assert!(
                    !events
                        .iter()
                        .any(|e| matches!(e, AgentEvent::InputRequested { .. }))
                );
            }
        }
    }
    #[tokio::test]
    async fn invalid_plans_cancel_even_with_yes_or_full_access() {
        let dir = tempfile::tempdir().unwrap();
        for access in AccessMode::ALL {
            for kind in ["spec-missing", "spec-oversized"] {
                let mut req = request(dir.path(), &format!("permission:{kind}"));
                req.access = access;
                req.plan = true;
                let events = collect(req, Some("Yes")).await;
                assert_eq!(text(&events), "cancel");
                assert!(events.iter().any(|e| matches!(e, AgentEvent::Error { message } if message.contains("not approved"))));
                assert!(
                    !events
                        .iter()
                        .any(|e| matches!(e, AgentEvent::InputRequested { .. }))
                );
            }
        }
    }

    #[tokio::test]
    async fn ask_user_multiselect_and_cancellation_and_unsupported_pick_options() {
        let dir = tempfile::tempdir().unwrap();
        for answer in [None, Some("Red, Blue")] {
            let events = collect(request(dir.path(), "ask"), answer).await;
            assert_eq!(status(&events), DoneStatus::Completed);
            let result: serde_json::Value = serde_json::from_str(&text(&events)).unwrap();
            if answer.is_none() {
                assert_eq!(result, serde_json::json!({"cancelled":true,"answers":[]}));
            } else {
                assert_eq!(result["answers"][0]["answer"], "Red, Blue");
            }
        }
        assert_eq!(
            text(&collect(request(dir.path(), "pick"), None).await),
            "-32601"
        );
    }
    #[tokio::test]
    async fn resume_ignores_history_and_missing_session_has_notice() {
        let dir = tempfile::tempdir().unwrap();
        let mut req = request(dir.path(), "hello");
        req.resume = Some("droid-fixture-session".into());
        let events = collect(req.clone(), None).await;
        assert_eq!(text(&events), "reply:hello:done");
        req.resume = Some("missing".into());
        let events = collect(req, None).await;
        assert!(text(&events).contains("without the previous context"));
        assert_eq!(status(&events), DoneStatus::Completed);
    }
    #[tokio::test]
    async fn wrong_effective_effort_or_org_cap_fails_before_prompt() {
        let dir = tempfile::tempdir().unwrap();
        for access in AccessMode::ALL {
            for (model, needle) in [
                ("coerce", "reasoningEffort"),
                ("cap", "organization allows"),
            ] {
                let mut req = request(dir.path(), "NEVER PROMPT");
                req.model = Some(model.into());
                req.reasoning = Some(ReasoningLevel::High);
                req.access = access;
                let events = collect(req, None).await;
                assert!(text(&events).is_empty());
                assert_eq!(status(&events), DoneStatus::Errored);
                assert!(events.iter().any(|e| matches!(e,AgentEvent::Done { error:Some(error),.. } if error.contains(needle))),"{events:?}");
            }
        }
    }
    #[tokio::test]
    async fn authentication_error_and_crash_are_terminal() {
        let dir = tempfile::tempdir().unwrap();
        for (prompt, needle) in [
            ("auth", "/login"),
            ("error", "provider refused"),
            ("crash", "fixture crashed deliberately"),
        ] {
            let events = collect(request(dir.path(), prompt), None).await;
            assert_eq!(status(&events), DoneStatus::Errored);
            assert!(events.iter().any(|e| matches!(e,AgentEvent::Done { error:Some(error),.. } if error.contains(needle))),"{events:?}");
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, AgentEvent::Done { .. }))
                    .count(),
                1
            );
        }
    }
    #[tokio::test]
    async fn steers_are_receipted_only_on_matching_user_message_and_discards_are_not() {
        for steer in ["next", "discard-steer"] {
            let dir = tempfile::tempdir().unwrap();
            let (controls, sender, _) = controls(None);
            let mut stream = harness()
                .run(request(dir.path(), "hold"), controls)
                .await
                .unwrap();
            let mut events = vec![];
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                let ready = matches!(&event,AgentEvent::TextDelta { text } if text=="working");
                events.push(event);
                if ready {
                    break;
                }
            }
            sender.send(SteerMessage::text(steer)).await.unwrap();
            drop(sender);
            events.extend(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    stream.map(Result::unwrap).collect::<Vec<_>>(),
                )
                .await
                .unwrap(),
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(e, AgentEvent::Steered { .. }))
                    .count(),
                usize::from(steer == "next")
            );
            assert_eq!(status(&events), DoneStatus::Completed);
        }
    }
    #[tokio::test]
    async fn configured_steer_waits_for_turn_end_and_verifies_new_settings() {
        for (model, effort, expected) in [
            ("other", Some(ReasoningLevel::High), DoneStatus::Completed),
            ("auto", Some(ReasoningLevel::High), DoneStatus::Completed),
            ("new-default", None, DoneStatus::Completed),
            ("coerce", Some(ReasoningLevel::High), DoneStatus::Errored),
            ("uncorrelated-settings", None, DoneStatus::Errored),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (controls, sender, _) = controls(None);
            let req = request(dir.path(), "delayed");
            let mut stream = harness().run(req.clone(), controls).await.unwrap();
            while let Some(event) = stream.next().await {
                if matches!(event.unwrap(), AgentEvent::TextDelta { .. }) {
                    break;
                }
            }
            let mut config = req;
            config.model = Some(model.into());
            config.reasoning = effort;
            let mut steer = SteerMessage::text("configured");
            steer.config = Some(Box::new(config));
            sender.send(steer).await.unwrap();
            drop(sender);
            let events = tokio::time::timeout(
                Duration::from_secs(if model == "uncorrelated-settings" {
                    25
                } else {
                    5
                }),
                stream.map(Result::unwrap).collect::<Vec<_>>(),
            )
            .await
            .unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|e| matches!(
                        e,
                        AgentEvent::Done {
                            status: DoneStatus::Completed,
                            ..
                        }
                    ))
                    .count(),
                if expected == DoneStatus::Completed {
                    2
                } else {
                    1
                },
                "{events:?}"
            );
            assert_eq!(status(&events), expected);
            if model == "uncorrelated-settings" {
                assert!(events.iter().any(|e| matches!(e, AgentEvent::Done { error: Some(error), .. } if error.contains("Droid settings update timed out"))), "{events:?}");
            }
            assert_eq!(
                text(&events).contains("reply:configured:done"),
                expected == DoneStatus::Completed
            );
        }
    }
    #[tokio::test]
    async fn stop_cancels_pending_questions_and_reuses_runtime() {
        for prompt in [
            "hold",
            "stop-completes",
            "permission:exec",
            "permission:spec",
            "ask",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (mut controls, sender, _) = controls(None);
            // Keep the resolver alive until stop, rather than declining immediately.
            let resolvers = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
            let keep = resolvers.clone();
            controls.request_input = Box::new(move |_| {
                let (tx, rx) = oneshot::channel();
                keep.lock().unwrap().push(tx);
                rx
            });
            let turn = controls.turn.clone();
            let mut stream = harness()
                .run(request(dir.path(), prompt), controls)
                .await
                .unwrap();
            let mut events = vec![];
            loop {
                let event = tokio::time::timeout(Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                let ready = matches!(
                    &event,
                    AgentEvent::TextDelta { .. } | AgentEvent::InputRequested { .. }
                );
                events.push(event);
                if ready {
                    break;
                }
            }
            turn.stop_turn();
            loop {
                let event = tokio::time::timeout(Duration::from_secs(5), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                let ended = matches!(
                    &event,
                    AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        ..
                    }
                );
                events.push(event);
                if ended {
                    break;
                }
            }
            if !["hold", "stop-completes"].contains(&prompt) {
                assert!(events.iter().any(|e| matches!(e,AgentEvent::InputResolved { answers:Some(answers),..} if answers.is_empty())));
            }
            sender.send(SteerMessage::text("continued")).await.unwrap();
            drop(sender);
            events.extend(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    stream.map(Result::unwrap).collect::<Vec<_>>(),
                )
                .await
                .unwrap(),
            );
            assert!(text(&events).contains("reply:continued:done"), "{events:?}");
            assert_eq!(status(&events), DoneStatus::Completed);
        }
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn stubborn_child_is_reaped_when_consumer_drops() {
        let dir = tempfile::tempdir().unwrap();
        let (controls, sender, _) = controls(None);
        let mut stream = harness()
            .run(request(dir.path(), "stubborn"), controls)
            .await
            .unwrap();
        let pid = loop {
            let event = stream.next().await.unwrap().unwrap();
            if let AgentEvent::TextDelta { text } = event {
                break text.strip_prefix("pid:").unwrap().parse::<i32>().unwrap();
            }
        };
        drop(stream);
        drop(sender);
        tokio::time::timeout(Duration::from_secs(5), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned stubborn child must be killed and reaped");
    }
    #[tokio::test]
    async fn fixture_rejects_wrong_factory_envelopes() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_harness-droid-fixture"))
            .env("FACTORY_DROID_AUTO_UPDATE_ENABLED", "false")
            .env("DROID_DISABLE_AUTO_UPDATE", "true")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        for frame in [
            serde_json::json!({"type":"request","factoryApiVersion":"1.0.0","id":1,"method":"droid.list_models","params":{}}),
            serde_json::json!({"factoryApiVersion":"1.0.0","id":"1","method":"droid.list_models","params":{}}),
            serde_json::json!({"type":"request","id":"1","method":"droid.list_models","params":{}}),
        ] {
            input
                .write_all(format!("{frame}\n").as_bytes())
                .await
                .unwrap();
            input.flush().await.unwrap();
            let response: serde_json::Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["error"]["code"], -32700);
            assert!(response["id"].is_null());
        }
        drop(input);
        child.wait().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "uses real Factory credits; set DROID_LIVE_EXECUTABLE"]
async fn live_auto_child_transcripts_stay_separate_from_the_parent() {
    let exe = std::env::var_os("DROID_LIVE_EXECUTABLE").expect("DROID_LIVE_EXECUTABLE");
    let harness = DroidHarness::new().with_executable(exe);
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("fixture.txt"), "DROID_CHILD_FIXTURE_OK").unwrap();
    for (file, seconds) in [("slow-child.py", 20), ("slow-parent.py", 35)] {
        std::fs::write(
            dir.path().join(file),
            format!("import time\ntime.sleep({seconds})\nprint('DROID_TOOL_OK')\n"),
        )
        .unwrap();
    }
    let mut req = request(
        dir.path(),
        &format!(
            "This is an authorized isolated dummy protocol test. Work ONLY inside {}. \
        Do not read credentials, environment variables, session logs, other directories, \
        websites, or external services. Do not edit anything. Launch exactly one Task \
        subagent using the built-in worker type, description 'Droid sidebar probe', \
        using the inherited Auto model. Tell the child to Read fixture.txt, then Execute \
        exactly 'python3 slow-child.py', then Read fixture.txt again, then reply \
        DROID_CHILD_FINAL_OK. The files already exist. After launching the Task, \
        Execute exactly 'python3 slow-parent.py' to keep the parent active during the \
        child. When the child settles, reply DROID_PARENT_FINAL_OK. Do not use AskUser \
        or create a plan.",
            dir.path().display()
        ),
    );
    req.model = Some("auto".into());
    let (controls, sender, _) = controls(Some("Yes"));
    let turn = controls.turn.clone();
    let mut stream = harness.run(req, controls).await.unwrap();
    drop(sender);
    let events = tokio::time::timeout(Duration::from_secs(180), async {
        let mut events = Vec::new();
        let mut saw_live_tool = false;
        while let Some(event) = stream.next().await {
            let event = event.unwrap();
            if let AgentEvent::Subagent { event, .. } = &event
                && matches!(event.as_ref(), AgentEvent::ToolCall { .. })
                && turn.background_live()
            {
                saw_live_tool = true;
            }
            events.push(event);
        }
        assert!(saw_live_tool, "child tools must stream before settlement");
        events
    })
    .await
    .unwrap();
    assert!(
        events.iter().any(
            |event| matches!(event, AgentEvent::SessionStarted { model, .. } if model == "auto")
        )
    );
    assert_eq!(status(&events), DoneStatus::Completed);
    let children: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Subagent { event, .. } => Some(event.as_ref().clone()),
            _ => None,
        })
        .collect();
    assert_eq!(status(&children), DoneStatus::Completed);
    assert!(text(&children).contains("DROID_CHILD_FINAL_OK"));
    assert!(!text(&events).contains("DROID_CHILD_FINAL_OK"));
    assert!(children.iter().any(|event| matches!(
        event,
        AgentEvent::ToolCall {
            call: zeron_proto::ToolCall::ReadFile { .. },
            ..
        }
    )));
    assert!(children.iter().any(|event| matches!(event, AgentEvent::ToolCall { call: zeron_proto::ToolCall::Exec { command }, .. } if command.contains("slow-child.py"))));
    assert!(children.iter().any(|event| matches!(event, AgentEvent::ToolResult { output: Some(output), .. } if output.contains("DROID_TOOL_OK"))));
    assert!(!events.iter().any(|event| matches!(event, AgentEvent::ToolCall { call: zeron_proto::ToolCall::Exec { command }, .. } if command.contains("slow-child.py"))), "relayed child permissions must not create parent chips");
    println!(
        "auto-model live check: {} parent events, {} child events, child text/tools/results and completion verified",
        events.len(),
        children.len()
    );
}

#[tokio::test]
#[ignore = "uses real Factory credits; set DROID_LIVE_EXECUTABLE"]
async fn live_stream_edit_mcp_stop_resume_spec_and_steer() {
    let exe = std::env::var_os("DROID_LIVE_EXECUTABLE").expect("DROID_LIVE_EXECUTABLE");
    let harness = DroidHarness::new().with_executable(exe);
    let models = harness.models().await.unwrap();
    assert!(models.len() > 1);
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(
        dir.path(),
        "Remember token violet-otter. Reply with the numbers one through twenty, one per line. Do not use tools.",
    );
    req.model = Some("claude-haiku-4-5-20251001".into());
    req.reasoning = Some(zeron_proto::ReasoningLevel::Low);
    let (controls, sender, interrupt) = controls(Some("Yes"));
    let turn = controls.turn.clone();
    let mut stream = harness.run(req.clone(), controls).await.unwrap();
    let mut all = vec![];
    async fn until_done(
        stream: &mut futures::stream::BoxStream<
            'static,
            Result<AgentEvent, zeron_harness::HarnessError>,
        >,
    ) -> Vec<AgentEvent> {
        tokio::time::timeout(Duration::from_secs(120), async {
            let mut events = vec![];
            while let Some(event) = stream.next().await {
                let event = event.unwrap();
                let done = matches!(event, AgentEvent::Done { .. });
                events.push(event);
                if done {
                    break;
                }
            }
            events
        })
        .await
        .unwrap()
    }
    let first = until_done(&mut stream).await;
    assert_eq!(status(&first), DoneStatus::Completed);
    assert!(
        first
            .iter()
            .filter(|e| matches!(e, AgentEvent::TextDelta { .. }))
            .count()
            > 1
    );
    let session = first
        .iter()
        .find_map(|e| match e {
            AgentEvent::SessionStarted { session_id, .. } => Some(session_id.clone()),
            _ => None,
        })
        .unwrap();
    all.extend(first);
    sender.send(SteerMessage::text("Use Create to write exactly hello to note.txt in the current directory. Do nothing else.")).await.unwrap();
    let edit = until_done(&mut stream).await;
    assert!(edit.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCall {
            call: zeron_proto::ToolCall::WriteFile { .. },
            ..
        }
    )));
    assert_eq!(status(&edit), DoneStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("note.txt"))
            .unwrap()
            .trim(),
        "hello"
    );
    all.extend(edit);
    sender
        .send(SteerMessage::text(
            "Use Execute to run `sleep 10` in this disposable directory, then reply done.",
        ))
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        let running = matches!(event, AgentEvent::ToolCall { .. });
        all.push(event);
        if running {
            break;
        }
    }
    turn.stop_turn();
    assert_eq!(
        status(&until_done(&mut stream).await),
        DoneStatus::Interrupted
    );
    sender
        .send(SteerMessage::text(
            "Use Execute to run `sleep 2` in the current directory, then reply waiting.",
        ))
        .await
        .unwrap();
    while let Some(event) = stream.next().await {
        let event = event.unwrap();
        if matches!(event, AgentEvent::ToolCall { .. }) {
            break;
        }
    }
    sender
        .send(SteerMessage::text("Instead end with exactly steered-pong."))
        .await
        .unwrap();
    let steered = until_done(&mut stream).await;
    assert!(
        steered
            .iter()
            .any(|e| matches!(e, AgentEvent::Steered { .. }))
    );
    assert!(text(&steered).contains("steered-pong"));
    interrupt.cancel();
    drop(sender);
    while stream.next().await.is_some() {}
    req.resume = Some(session);
    req.prompt =
        "What token did I ask you to remember? Reply only that token; do not use tools.".into();
    let (controls, sender, _) = self::controls(Some("Yes"));
    drop(sender);
    let events: Vec<_> = harness
        .run(req.clone(), controls)
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(status(&events), DoneStatus::Completed);
    assert!(text(&events).contains("violet-otter"));
    req.prompt="Use ExitSpecMode to propose creating plan.txt containing planned. After approval, create the file. Do not inspect files or run commands.".into();
    req.plan = true;
    let (controls, sender, _) = self::controls(Some("Yes"));
    drop(sender);
    let events: Vec<_> = harness
        .run(req, controls)
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;
    assert!(events.iter().any(|e| matches!(e,AgentEvent::InputRequested { questions,.. } if questions.iter().any(|q|q.header=="Approve plan"))));
    assert_eq!(status(&events), DoneStatus::Completed);
    let zeron = std::env::var("DROID_LIVE_ZERON_EXECUTABLE").expect("DROID_LIVE_ZERON_EXECUTABLE");
    let mut req = request(
        dir.path(),
        "Use ToolSearch to load zeron whoami, call it once, then reply pong. Do not inspect files or run commands.",
    );
    req.model = Some("claude-haiku-4-5-20251001".into());
    req.mcp = Some(zeron_proto::McpServer {
        name: "zeron".into(),
        command: zeron,
        args: vec!["mcp".into()],
        env: Default::default(),
    });
    let (controls, sender, _) = self::controls(Some("Yes"));
    drop(sender);
    let events: Vec<_> = harness
        .run(req, controls)
        .await
        .unwrap()
        .map(Result::unwrap)
        .collect()
        .await;
    assert!(events.iter().any(|e| matches!(e,AgentEvent::ToolCall {call:zeron_proto::ToolCall::Mcp {server,..},..} if server=="zeron")));
    assert_eq!(status(&events), DoneStatus::Completed);
}
