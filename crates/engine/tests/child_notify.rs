//! Durable parent notifications: an agent-spawned child's settle
//! lands on its parent as a ChildUpdate card + one agent-only prompt, exactly
//! once — across a busy parent, coalesced siblings, acks, archiving, and
//! engine restarts.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel, Session,
    SessionStatus, SteeringMode, UserInputQuestion,
};
use zeron_rpc::methods;

/// Prompts drive the script: `__HOLD__` parks the turn until `release`
/// fires, `__ERR__` settles errored, `__ASK__` requests input and never
/// completes. Unique tokens because the engine prepends a "side
/// conversation" context block to child prompts — prefix matching would
/// miss, and plain words ("TASK" contains "ASK") collide.
struct Scripted {
    requests: Arc<Mutex<Vec<RunRequest>>>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Harness for Scripted {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Scripted"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let prompt = request.prompt.clone();
        self.requests.lock().unwrap().push(request);
        let release = self.release.clone();
        let stream = if prompt.contains("__HOLD__") {
            futures::stream::once(async move {
                release.notified().await;
                Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some("held-session".into()),
                })
            })
            .boxed()
        } else if prompt.contains("__ERR__") {
            futures::stream::iter(vec![
                Ok(AgentEvent::Error {
                    message: "scripted failure".into(),
                }),
                Ok(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some("scripted failure".into()),
                    session_id: None,
                }),
            ])
            .boxed()
        } else if prompt.contains("__ASK__") {
            // The engine input bridge owns this lifecycle: ask through
            // `request_input` so the request id is registered and the session
            // parks on AwaitingInput; the run ends only when answered.
            let rx = (controls.request_input)(vec![UserInputQuestion {
                id: "q1".into(),
                header: "Pick a lane".into(),
                question: "Which lane should this take?".into(),
                options: vec!["left".into(), "right".into()],
                multi_select: false,
            }]);
            futures::stream::once(async move {
                let _ = rx.await;
                Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some("asked-session".into()),
                })
            })
            .boxed()
        } else {
            // Echo only the caller's own line — child prompts carry the
            // engine's side-conversation prefix.
            let last = prompt.lines().last().unwrap_or(prompt.as_str());
            futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta {
                    text: format!("child reply to: {last}"),
                }),
                Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some("child-session".into()),
                }),
            ])
            .boxed()
        };
        Ok(stream)
    }
}

struct TestBed {
    core: EngineCore,
    requests: Arc<Mutex<Vec<RunRequest>>>,
    release: Arc<tokio::sync::Notify>,
    /// Keeps the data dir alive; `None` when the caller owns it (restarts).
    #[allow(dead_code)]
    dir: Option<tempfile::TempDir>,
}

fn bed() -> TestBed {
    let dir = tempfile::tempdir().unwrap();
    let mut bed = assemble_at(dir.path());
    bed.dir = Some(dir);
    bed
}

fn assemble_at(dir: &std::path::Path) -> TestBed {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Scripted {
        requests: requests.clone(),
        release: release.clone(),
    }));
    let core = EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).unwrap();
    TestBed {
        core,
        requests,
        release,
        dir: None,
    }
}

fn agent_chat(core: &EngineCore, id: &str, parent: &str, spawned_by_agent: bool) {
    core.workspace
        .create_chat_with_parent(
            id,
            None,
            Some(&core.device_id),
            None,
            Some("/tmp".into()),
            Some(parent.to_string()),
            spawned_by_agent,
        )
        .unwrap();
}

fn top_chat(core: &EngineCore, id: &str) {
    core.workspace
        .create_chat(id, None, Some(&core.device_id), None, Some("/tmp".into()))
        .unwrap();
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: None,
        attachments: vec![],
        worktree: None,
        agent: None,
    }
}

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Captured hidden prompts delivered to the parent agent (they start with
/// the `[Zeron system]` frame and never mint a user bubble).
fn system_prompts(requests: &Arc<Mutex<Vec<RunRequest>>>) -> Vec<String> {
    requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.prompt.clone())
        .filter(|p| p.starts_with("[Zeron system]"))
        .collect()
}

fn child_update_cards(core: &EngineCore, parent: &str) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(parent)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .filter(|e| {
            e.role == MessageRole::System
                && e.parts
                    .iter()
                    .any(|p| matches!(p, MessagePart::ChildUpdate { .. }))
        })
        .collect()
}

/// User-role entries: the assertion that no notification ever shows up as a
/// user bubble.
fn user_entries(core: &EngineCore, chat: &str) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(chat)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .filter(|e| e.role == MessageRole::User)
        .collect()
}

#[tokio::test]
async fn completed_child_delivers_card_and_one_prompt() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "child", "parent", true);
    bed.core
        .sessions
        .dispatch(
            "child",
            HarnessId::Mock,
            request("CHILD TASK"),
            Some("u1".into()),
        )
        .await
        .unwrap();
    wait_for("parent notification", || {
        !child_update_cards(&bed.core, "parent").is_empty()
            && !system_prompts(&bed.requests).is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    // Exactly one card, one prompt — never a user bubble for it.
    let cards = child_update_cards(&bed.core, "parent");
    assert_eq!(cards.len(), 1);
    assert!(cards[0].id.starts_with("child:child:done:"));
    match &cards[0].parts[0] {
        MessagePart::ChildUpdate {
            child_chat_id,
            outcome,
            excerpt,
            ..
        } => {
            assert_eq!(child_chat_id, "child");
            assert_eq!(
                *outcome,
                zeron_proto::orchestration::ChildOutcome::Completed
            );
            assert!(
                excerpt
                    .as_deref()
                    .unwrap_or_default()
                    .contains("child reply to: CHILD TASK")
            );
        }
        part => panic!("expected ChildUpdate, got {part:?}"),
    }
    let prompts = system_prompts(&bed.requests);
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0],
        "[Zeron system]\n\n@chat:child completed:\n\nchild reply to: CHILD TASK"
    );
    assert!(
        user_entries(&bed.core, "parent")
            .iter()
            .all(|e| !e.id.starts_with("child:")),
        "notification must not write a user bubble"
    );
    assert!(
        user_entries(&bed.core, "parent").is_empty(),
        "hidden dispatch wrote a user entry"
    );
    // Ledger reached its terminal state.
    let rows = bed
        .core
        .doc_host
        .docs_store()
        .child_notifications_for("child")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "delivered");
    bed.core.shutdown().await;
}

#[tokio::test]
async fn busy_parent_holds_the_notification_until_turn_end() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "child", "parent", true);
    bed.core
        .sessions
        .dispatch(
            "parent",
            HarnessId::Mock,
            request("__HOLD__ parent"),
            Some("u-p".into()),
        )
        .await
        .unwrap();
    wait_for("parent turn in flight", || {
        bed.core.sessions.turn_in_flight("parent")
    })
    .await;
    bed.core
        .sessions
        .dispatch(
            "child",
            HarnessId::Mock,
            request("CHILD"),
            Some("u1".into()),
        )
        .await
        .unwrap();
    // Past the 2 s debounce with the parent still working: nothing delivered.
    tokio::time::sleep(Duration::from_millis(3200)).await;
    assert!(system_prompts(&bed.requests).is_empty());
    assert!(child_update_cards(&bed.core, "parent").is_empty());
    // …but the claim is already durable.
    assert!(
        !bed.core
            .doc_host
            .docs_store()
            .child_notifications_for("child")
            .unwrap()
            .is_empty()
    );
    bed.release.notify_waiters();
    wait_for("held notification delivered", || {
        !system_prompts(&bed.requests).is_empty()
    })
    .await;
    assert_eq!(system_prompts(&bed.requests).len(), 1);
    assert_eq!(child_update_cards(&bed.core, "parent").len(), 1);
    bed.core.shutdown().await;
}

#[tokio::test]
async fn two_children_coalesce_into_one_prompt_and_two_cards() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "child-a", "parent", true);
    agent_chat(&bed.core, "child-b", "parent", true);
    for child in ["child-a", "child-b"] {
        bed.core
            .sessions
            .dispatch(
                child,
                HarnessId::Mock,
                request("CHILD"),
                Some(format!("u-{child}")),
            )
            .await
            .unwrap();
    }
    wait_for("coalesced notification", || {
        child_update_cards(&bed.core, "parent").len() == 2
            && !system_prompts(&bed.requests).is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let prompts = system_prompts(&bed.requests);
    assert_eq!(prompts.len(), 1, "two settles must share one prompt");
    // Sibling settles race; the bullet order follows claim order.
    let prompt = &prompts[0];
    assert!(prompt.starts_with("[Zeron system]\n\nChild chat updates:\n\n"));
    for child in ["child-a", "child-b"] {
        assert!(
            prompt.contains(&format!("- @chat:{child} completed.")),
            "missing {child} bullet in {prompt}"
        );
    }
    assert_eq!(child_update_cards(&bed.core, "parent").len(), 2);
    bed.core.shutdown().await;
}

#[tokio::test]
async fn ack_before_flush_suppresses_prompt_and_card() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "child", "parent", true);
    bed.core
        .sessions
        .dispatch(
            "child",
            HarnessId::Mock,
            request("CHILD"),
            Some("u1".into()),
        )
        .await
        .unwrap();
    // Wait for the durable claim, then ack through the RPC surface before
    // the 2 s flush fires.
    wait_for("ledger claim", || {
        !bed.core
            .doc_host
            .docs_store()
            .child_notifications_for("child")
            .unwrap()
            .is_empty()
    })
    .await;
    let key = bed
        .core
        .doc_host
        .docs_store()
        .child_notifications_for("child")
        .unwrap()
        .remove(0)
        .turn_key;
    let client = zeron_rpc::memory_client(bed.core.rpc_service());
    client
        .call(
            methods::ACK_CHILD_UPDATES,
            serde_json::json!({
                "parentChatId": "parent",
                "updates": [{ "childChatId": "child", "turnKey": key }],
            }),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert!(system_prompts(&bed.requests).is_empty());
    assert!(child_update_cards(&bed.core, "parent").is_empty());
    assert_eq!(
        bed.core
            .doc_host
            .docs_store()
            .child_notifications_for("child")
            .unwrap()[0]
            .state,
        "acked"
    );
    bed.core.shutdown().await;
}

#[tokio::test]
async fn archived_parent_drops_the_update_and_stays_archived() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "child", "parent", true);
    let client = zeron_rpc::memory_client(bed.core.rpc_service());
    client
        .call(
            methods::MUTATE,
            serde_json::json!({"op": "setChatArchived", "chatId": "parent", "archived": true}),
        )
        .await
        .unwrap();
    bed.core
        .sessions
        .dispatch(
            "child",
            HarnessId::Mock,
            request("CHILD"),
            Some("u1".into()),
        )
        .await
        .unwrap();
    wait_for("dropped ledger row", || {
        bed.core
            .doc_host
            .docs_store()
            .child_notifications_for("child")
            .unwrap()
            .iter()
            .any(|r| r.state == "dropped")
    })
    .await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(system_prompts(&bed.requests).is_empty());
    assert!(child_update_cards(&bed.core, "parent").is_empty());
    assert!(
        bed.core.workspace.chat("parent").unwrap().unwrap().archived,
        "delivery must not unarchive the parent"
    );
    bed.core.shutdown().await;
}

#[tokio::test]
async fn user_side_child_never_notifies() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "side", "parent", false);
    bed.core
        .sessions
        .dispatch("side", HarnessId::Mock, request("CHILD"), Some("u1".into()))
        .await
        .unwrap();
    wait_for("side chat turn settled", || {
        bed.core
            .sessions
            .session_status("side")
            .is_some_and(|s| s.status == zeron_proto::SessionStatus::Idle)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert!(system_prompts(&bed.requests).is_empty());
    assert!(child_update_cards(&bed.core, "parent").is_empty());
    assert!(
        bed.core
            .doc_host
            .docs_store()
            .child_notifications_for("side")
            .unwrap()
            .is_empty()
    );
    bed.core.shutdown().await;
}

#[tokio::test]
async fn errored_and_interrupted_children_use_their_templates() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "err-child", "parent", true);
    agent_chat(&bed.core, "stop-child", "parent", true);
    // Deliver each settle on its own flush — sequential claims make the
    // single-update templates deterministic (the batch template is covered
    // by the coalescing test).
    bed.core
        .sessions
        .dispatch(
            "err-child",
            HarnessId::Mock,
            request("__ERR__ now"),
            Some("u-e".into()),
        )
        .await
        .unwrap();
    wait_for("errored update delivered", || {
        !system_prompts(&bed.requests).is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        system_prompts(&bed.requests),
        ["[Zeron system]\n\n@chat:err-child failed.\n\nReview the chat before deciding next steps.".to_string()]
    );

    bed.core
        .sessions
        .dispatch(
            "stop-child",
            HarnessId::Mock,
            request("__HOLD__ work"),
            Some("u-s".into()),
        )
        .await
        .unwrap();
    wait_for("interruptible child working", || {
        bed.core
            .sessions
            .session_status("stop-child")
            .is_some_and(|s| s.status == zeron_proto::SessionStatus::Working)
    })
    .await;
    bed.core.sessions.interrupt("stop-child").await.unwrap();
    wait_for("interrupted update delivered", || {
        system_prompts(&bed.requests).len() == 2
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let prompts = system_prompts(&bed.requests);
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        prompts[1],
        "[Zeron system]\n\n@chat:stop-child was interrupted.\n\nReview the chat before deciding next steps.\n\nIf the user stopped it manually, do not resume, restart, retry, replace, or continue the work unless the user explicitly asks."
    );
    let cards = child_update_cards(&bed.core, "parent");
    let outcomes: Vec<_> = cards
        .iter()
        .map(|c| match &c.parts[0] {
            MessagePart::ChildUpdate {
                child_chat_id,
                outcome,
                ..
            } => (child_chat_id.clone(), *outcome),
            _ => panic!("not a child update"),
        })
        .collect();
    use zeron_proto::orchestration::ChildOutcome;
    assert_eq!(
        outcomes,
        [
            ("err-child".to_string(), ChildOutcome::Errored),
            ("stop-child".to_string(), ChildOutcome::Interrupted),
        ]
    );
    bed.core.shutdown().await;
}

#[tokio::test]
async fn needs_help_carries_the_blocker() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "child", "parent", true);
    bed.core
        .sessions
        .dispatch(
            "child",
            HarnessId::Mock,
            request("__ASK__ me"),
            Some("u1".into()),
        )
        .await
        .unwrap();
    wait_for("needs-input notification", || {
        !system_prompts(&bed.requests).is_empty()
    })
    .await;
    let prompts = system_prompts(&bed.requests);
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0],
        "[Zeron system]\n\n@chat:child needs help.\nBlocked on Pick a lane:\nWhich lane should this take?\noptions: left, right\n\nReview the blocker. If you can resolve it from existing context, reply to the chat with guidance. Otherwise, ask the user for the missing decision."
    );
    let cards = child_update_cards(&bed.core, "parent");
    assert_eq!(cards.len(), 1);
    match &cards[0].parts[0] {
        MessagePart::ChildUpdate { outcome, .. } => {
            assert_eq!(
                *outcome,
                zeron_proto::orchestration::ChildOutcome::NeedsInput
            )
        }
        _ => panic!("not a child update"),
    }
    bed.core.shutdown().await;
}

#[tokio::test]
async fn pending_updates_survive_a_restart_but_delivered_ones_do_not_repeat() {
    let dir = tempfile::tempdir().unwrap();
    let bed = assemble_at(dir.path());
    top_chat(&bed.core, "parent");
    agent_chat(&bed.core, "child", "parent", true);
    bed.core
        .sessions
        .dispatch(
            "child",
            HarnessId::Mock,
            request("CHILD"),
            Some("u1".into()),
        )
        .await
        .unwrap();
    // Stop the engine while the claim sits pending inside the debounce.
    wait_for("durable claim", || {
        !bed.core
            .doc_host
            .docs_store()
            .pending_child_notifications("parent")
            .unwrap()
            .is_empty()
    })
    .await;
    bed.core.shutdown().await;
    drop(bed.core);
    assert!(system_prompts(&bed.requests).is_empty());

    let bed2 = assemble_at(dir.path());
    wait_for("delivered once after restart", || {
        !system_prompts(&bed2.requests).is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert_eq!(system_prompts(&bed2.requests).len(), 1);
    assert_eq!(child_update_cards(&bed2.core, "parent").len(), 1);
    bed2.core.shutdown().await;
    drop(bed2.core);

    // A second restart must NOT re-deliver: the rows are `delivered`, the
    // card's fixed entry id already exists.
    let bed3 = assemble_at(dir.path());
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert!(system_prompts(&bed3.requests).is_empty());
    assert_eq!(child_update_cards(&bed3.core, "parent").len(), 1);
    bed3.core.shutdown().await;
}

#[tokio::test]
async fn archiving_a_chat_archives_descendants_children_first() {
    let bed = bed();
    top_chat(&bed.core, "grandparent");
    agent_chat(&bed.core, "parent", "grandparent", true);
    agent_chat(&bed.core, "child", "parent", true);
    // Ordering is observable on the helper itself.
    let order = bed.core.workspace.archive_chat_tree("grandparent").unwrap();
    assert_eq!(order, ["child", "parent", "grandparent"]);
    // Un-archive them, then exercise the Mutate RPC path.
    for id in ["grandparent", "parent", "child"] {
        bed.core.workspace.set_chat_archived(id, false).unwrap();
    }
    let client = zeron_rpc::memory_client(bed.core.rpc_service());
    client
        .call(
            methods::MUTATE,
            serde_json::json!({"op": "setChatArchived", "chatId": "grandparent", "archived": true}),
        )
        .await
        .unwrap();
    for id in ["grandparent", "parent", "child"] {
        assert!(
            bed.core.workspace.chat(id).unwrap().unwrap().archived,
            "{id} should be archived"
        );
    }
    // No unarchive cascade.
    client
        .call(
            methods::MUTATE,
            serde_json::json!({"op": "setChatArchived", "chatId": "grandparent", "archived": false}),
        )
        .await
        .unwrap();
    assert!(
        !bed.core
            .workspace
            .chat("grandparent")
            .unwrap()
            .unwrap()
            .archived
    );
    assert!(bed.core.workspace.chat("parent").unwrap().unwrap().archived);
    assert!(bed.core.workspace.chat("child").unwrap().unwrap().archived);
    bed.core.shutdown().await;
}

#[tokio::test]
async fn child_host_restart_mid_turn_reports_interrupted() {
    let bed = bed();
    top_chat(&bed.core, "parent");
    // The child is hosted on ANOTHER device: this engine only ever sees its
    // registry session rows — which is exactly where the dead host's
    // recovery write lands.
    bed.core
        .workspace
        .create_chat_with_parent(
            "child",
            None,
            Some("remote-host"),
            None,
            Some("/tmp".into()),
            Some("parent".into()),
            true,
        )
        .unwrap();
    // Mid-turn row first (the completed-turn marker carried forward), then
    // the restart write: Idle with the marker wiped. Staggered so the
    // notifier observes the Working row before the settle lands.
    let now = chrono::Utc::now();
    bed.core.workspace.record_session(&Session {
        last_completed_turn: Some("t0".into()),
        chat_id: "child".into(),
        device_id: "remote-host".into(),
        status: SessionStatus::Working,
        started_at: Some(now),
        updated_at: now,
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    bed.core.workspace.record_session(&Session {
        last_completed_turn: None,
        chat_id: "child".into(),
        device_id: "remote-host".into(),
        status: SessionStatus::Idle,
        started_at: None,
        updated_at: chrono::Utc::now(),
    });
    wait_for("interrupted notification", || {
        !system_prompts(&bed.requests).is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let prompts = system_prompts(&bed.requests);
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0],
        "[Zeron system]\n\n@chat:child was interrupted.\n\nReview the chat before deciding next steps.\n\nIf the user stopped it manually, do not resume, restart, retry, replace, or continue the work unless the user explicitly asks."
    );
    let cards = child_update_cards(&bed.core, "parent");
    assert_eq!(cards.len(), 1);
    match &cards[0].parts[0] {
        MessagePart::ChildUpdate { outcome, .. } => assert_eq!(
            *outcome,
            zeron_proto::orchestration::ChildOutcome::Interrupted
        ),
        _ => panic!("not a child update"),
    }
    bed.core.shutdown().await;
}
