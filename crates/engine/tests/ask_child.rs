//! The child ask against a real engine: hidden child chat, run-scoped
//! `submit_result`, repair rounds, nudge, typed failures, cancellation and
//! cleanup. The "model" is a scripted harness that calls the engine's RPC the
//! way the injected `zeron mcp` server would.

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use tokio_util::sync::CancellationToken;
use zeron_engine::ask::{AskBackend, AskError, AskSpec};
use zeron_proto::{AskSubmitReply, SandboxLevel};
use zeron_rpc::{RpcClient, methods};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "passed": {"type": "boolean"},
            "reason": {"type": "string", "minLength": 1}
        },
        "required": ["passed", "reason"],
        "additionalProperties": false
    })
}

fn spec() -> AskSpec {
    let mut spec = AskSpec::new("Verifier", "Is it done?", schema());
    spec.result_description = "the verdict".into();
    spec
}

type ClientSlot = Arc<OnceLock<Arc<RpcClient>>>;

async fn submit(
    client: &RpcClient,
    request: &zeron_proto::RunRequest,
    result: Value,
) -> AskSubmitReply {
    let env = &request.mcp.as_ref().expect("child carries the ask MCP").env;
    client
        .call_as(
            methods::SUBMIT_ASK_RESULT,
            json!({
                "chatId": env["ZERON_CHAT_ID"],
                "askId": env["ZERON_ASK_ID"],
                "result": result,
            }),
        )
        .await
        .expect("submit rpc")
}

struct Rig {
    env: Env,
    ask: Arc<dyn AskBackend>,
    _dir: tempfile::TempDir,
}

fn rig(
    handler: impl Fn(usize, zeron_proto::RunRequest, Out, zeron_harness::RunControls, Arc<RpcClient>)
    + Send
    + Sync
    + 'static,
) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let slot: ClientSlot = Arc::new(OnceLock::new());
    let handler_slot = slot.clone();
    let handler: Handler = Arc::new(move |i, request, out, controls| {
        let client = handler_slot.get().expect("client wired").clone();
        handler(i, request, out, controls, client);
    });
    let env = assemble(&dir.path().join("data"), Default::default(), handler);
    env.core.sessions.set_ipc_port(1);
    let client = Arc::new(zeron_rpc::memory_client(env.core.rpc_service()));
    slot.set(client).ok();
    env.chat("parent", SandboxLevel::WorkspaceWrite);
    let ask = env.core.doc_host.ask_backend().expect("ask backend");
    Rig {
        env,
        ask,
        _dir: dir,
    }
}

fn child_of(rig: &Rig, child: &str) -> zeron_proto::Chat {
    rig.env
        .core
        .workspace
        .chat(child)
        .unwrap()
        .expect("child chat row")
}

#[tokio::test]
async fn a_typed_result_comes_back_and_the_child_is_hidden_and_archived() {
    let advertised = Arc::new(Mutex::new(None));
    let seen = advertised.clone();
    let rig = rig(move |_, request, out, _controls, client| {
        let seen = seen.clone();
        tokio::spawn(async move {
            let env = &request.mcp.as_ref().unwrap().env;
            let spec: zeron_proto::AskSpecInfo = client
                .call_as(
                    methods::GET_ASK_SPEC,
                    json!({"chatId": env["ZERON_CHAT_ID"], "askId": env["ZERON_ASK_ID"]}),
                )
                .await
                .unwrap();
            *seen.lock().unwrap() = Some((spec, request.sandbox));
            let reply = submit(
                &client,
                &request,
                json!({"passed": true, "reason": "ran the tests"}),
            )
            .await;
            assert!(reply.accepted, "{reply:?}");
            text_turn(&out, "done", Some((100, 20)));
        });
    });
    let outcome = rig
        .ask
        .ask("parent", spec().read_only(), CancellationToken::new())
        .await
        .expect("ask succeeds");

    assert_eq!(outcome.result["reason"], "ran the tests");
    assert_eq!(outcome.repairs, 0);
    assert!(!outcome.nudged);
    assert_eq!(outcome.usage.input_tokens, 100);
    assert_eq!(outcome.usage.output_tokens, 20);
    assert_eq!(outcome.usage.total_tokens(), 120);
    assert_eq!(outcome.usage.turns, 1);

    // What the child's MCP server was told to advertise, and the permissions
    // it ran with: read-only, never wider than the parent's.
    let (advertised, sandbox) = advertised.lock().unwrap().clone().expect("spec fetched");
    assert_eq!(advertised.result_schema, schema());
    assert_eq!(advertised.result_description, "the verdict");
    assert_eq!(sandbox, SandboxLevel::ReadOnly);
    let prompt = rig.env.prompts().remove(0);
    assert!(prompt.starts_with("Is it done?"));
    assert!(prompt.contains("submit_result"));
    assert!(prompt.contains("You are read-only"));

    // Hidden (a child of the requester), archived, marked as an ask child.
    let child = child_of(&rig, &outcome.child_chat_id);
    assert_eq!(child.parent_chat_id.as_deref(), Some("parent"));
    assert!(child.archived);
    assert_eq!(child.title.as_deref(), Some("Verifier"));
    let handle = rig.env.core.doc_host.open(&outcome.child_chat_id).unwrap();
    assert!(handle.doc().ask_child().is_some());
    // The ask is gone: a late submit has nowhere to land.
    let late: AskSubmitReply = {
        let client = zeron_rpc::memory_client(rig.env.core.rpc_service());
        client
            .call_as(
                methods::SUBMIT_ASK_RESULT,
                json!({"chatId": outcome.child_chat_id, "askId": "x", "result": {}}),
            )
            .await
            .unwrap()
    };
    assert!(!late.accepted);
}

#[tokio::test]
async fn the_child_never_runs_wider_than_its_parent() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let rig = rig(move |_, request, out, _c, client| {
        log.lock()
            .unwrap()
            .push((request.sandbox, request.auto_approve));
        tokio::spawn(async move {
            submit(&client, &request, json!({"passed": true, "reason": "ok"})).await;
            text_turn(&out, "ok", None);
        });
    });
    // A read-only parent: even a non-read-only ask stays read-only.
    rig.env.chat("ro-parent", SandboxLevel::ReadOnly);
    rig.ask
        .ask("ro-parent", spec(), CancellationToken::new())
        .await
        .unwrap();
    // A writable parent with a read-only ask: narrowed.
    rig.ask
        .ask("parent", spec().read_only(), CancellationToken::new())
        .await
        .unwrap();
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].0, SandboxLevel::ReadOnly);
    assert_eq!(seen[1].0, SandboxLevel::ReadOnly);
    // Nothing in the parent's history asked for auto-approval, so the child
    // does not get it either.
    assert!(!seen[0].1 && !seen[1].1);
}

#[tokio::test]
async fn schema_violations_are_repaired_in_the_same_turn() {
    let replies = Arc::new(Mutex::new(Vec::new()));
    let log = replies.clone();
    let rig = rig(move |_, request, out, _c, client| {
        let log = log.clone();
        tokio::spawn(async move {
            let first = submit(&client, &request, json!({"passed": "yes", "reason": ""})).await;
            let second = submit(
                &client,
                &request,
                json!({"passed": true, "reason": "fixed"}),
            )
            .await;
            log.lock().unwrap().extend([first, second]);
            text_turn(&out, "ok", None);
        });
    });
    let outcome = rig
        .ask
        .ask("parent", spec(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(outcome.repairs, 1);
    assert_eq!(outcome.result["reason"], "fixed");
    let replies = replies.lock().unwrap();
    assert!(!replies[0].accepted);
    let paths: Vec<_> = replies[0]
        .violations
        .iter()
        .map(|v| v.path.as_str())
        .collect();
    assert!(
        paths.contains(&"/passed") && paths.contains(&"/reason"),
        "{paths:?}"
    );
    // The model reads the violations as the tool result.
    assert!(replies[0].message.contains("/passed"));
    assert_eq!(replies[0].repairs_left, 3);
    assert!(replies[1].accepted);
}

#[tokio::test]
async fn after_three_repairs_the_ask_fails_with_the_violations() {
    let rig = rig(|_, request, out, _c, client| {
        tokio::spawn(async move {
            for attempt in 0..5 {
                let reply = submit(&client, &request, json!({"passed": attempt})).await;
                assert!(!reply.accepted);
                if reply.repairs_left == 0 {
                    break;
                }
            }
            text_turn(&out, "giving up", None);
        });
    });
    let failure = rig
        .ask
        .ask("parent", spec(), CancellationToken::new())
        .await
        .unwrap_err();
    let AskError::InvalidResult(violations) = &failure.error else {
        panic!("expected InvalidResult, got {:?}", failure.error);
    };
    assert!(!violations.is_empty());
    // Archived even on failure, and the cost is still reported.
    let child = child_of(&rig, failure.child_chat_id.as_deref().unwrap());
    assert!(child.archived);
}

#[tokio::test]
async fn a_turn_without_a_submission_is_nudged_once() {
    let rig = rig(|index, request, out, _c, client| {
        tokio::spawn(async move {
            if index == 0 {
                text_turn(&out, "I think it is fine.", Some((10, 5)));
            } else {
                assert!(
                    request
                        .prompt
                        .contains("without calling the `submit_result`")
                );
                submit(
                    &client,
                    &request,
                    json!({"passed": false, "reason": "late"}),
                )
                .await;
                text_turn(&out, "submitted", Some((7, 3)));
            }
        });
    });
    let outcome = rig
        .ask
        .ask("parent", spec(), CancellationToken::new())
        .await
        .unwrap();
    assert!(outcome.nudged);
    assert_eq!(outcome.usage.turns, 2);
    assert_eq!(outcome.usage.total_tokens(), 25, "both turns are counted");
    assert_eq!(rig.env.runs.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn two_turns_without_a_submission_is_a_typed_failure() {
    let rig = rig(|_, _request, out, _c, _client| {
        text_turn(&out, "no tool call", None);
    });
    let failure = rig
        .ask
        .ask("parent", spec(), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(failure.error, AskError::NoResult);
    assert_eq!(rig.env.runs.lock().unwrap().len(), 2, "one nudge, no more");
    assert!(child_of(&rig, failure.child_chat_id.as_deref().unwrap()).archived);
}

#[tokio::test]
async fn cancelling_interrupts_the_child_and_archives_it() {
    let interrupted_run = Arc::new(AtomicBool::new(false));
    let flag = interrupted_run.clone();
    let rig = rig(move |_, _request, out, controls, _client| {
        let flag = flag.clone();
        tokio::spawn(async move {
            controls.interrupt.cancelled().await;
            flag.store(true, Ordering::SeqCst);
            interrupted(&out);
        });
    });
    let token = CancellationToken::new();
    let ask = {
        let ask = rig.ask.clone();
        let token = token.clone();
        tokio::spawn(async move { ask.ask("parent", spec(), token).await })
    };
    wait_for(
        || !rig.env.runs.lock().unwrap().is_empty(),
        "the child to start",
    )
    .await;
    token.cancel();
    let failure = ask.await.unwrap().unwrap_err();
    assert_eq!(failure.error, AskError::Cancelled);
    assert!(
        interrupted_run.load(Ordering::SeqCst),
        "the child's run was interrupted"
    );
    assert!(child_of(&rig, failure.child_chat_id.as_deref().unwrap()).archived);
}

#[tokio::test]
async fn a_silent_child_times_out() {
    let rig = rig(|_, _request, out, controls, _client| {
        tokio::spawn(async move {
            controls.interrupt.cancelled().await;
            interrupted(&out);
        });
    });
    let failure = rig
        .ask
        .ask(
            "parent",
            spec().with_timeout(Duration::from_millis(300)),
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(failure.error, AskError::Timeout(Duration::from_millis(300)));
    assert!(child_of(&rig, failure.child_chat_id.as_deref().unwrap()).archived);
}

#[tokio::test]
async fn a_child_that_asks_a_question_fails_at_once_instead_of_hanging() {
    let interrupted_run = Arc::new(AtomicUsize::new(0));
    let count = interrupted_run.clone();
    let rig = rig(move |_, _request, out, controls, _client| {
        let count = count.clone();
        tokio::spawn(async move {
            let _answer = (controls.request_input)(vec![zeron_proto::UserInputQuestion {
                meta: None,
                id: "q".into(),
                header: "Q".into(),
                question: "Which file should I read?".into(),
                options: vec![],
                prefill: None,
                multiline: false,
                multi_select: false,
            }]);
            controls.interrupt.cancelled().await;
            count.fetch_add(1, Ordering::SeqCst);
            interrupted(&out);
        });
    });
    let started = std::time::Instant::now();
    let failure = rig
        .ask
        .ask("parent", spec(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    let AskError::NeedsInput(question) = &failure.error else {
        panic!("expected NeedsInput, got {:?}", failure.error);
    };
    assert!(question.contains("Which file"), "{question}");
    assert_eq!(interrupted_run.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_errored_turn_is_reported_with_the_harness_message() {
    let rig = rig(|_, _request, out, _c, _client| {
        let _ = out.send(zeron_proto::AgentEvent::Error {
            message: "usage limit reached".into(),
        });
        let _ = out.send(zeron_proto::AgentEvent::Done {
            status: zeron_proto::DoneStatus::Errored,
            result: None,
            error: Some("usage limit reached".into()),
            session_id: None,
        });
    });
    let failure = rig
        .ask
        .ask("parent", spec(), CancellationToken::new())
        .await
        .unwrap_err();
    let AskError::TurnFailed(message) = &failure.error else {
        panic!("expected TurnFailed, got {:?}", failure.error);
    };
    assert!(message.contains("usage limit"), "{message}");
}

#[tokio::test]
async fn setup_problems_fail_before_any_child_exists() {
    let rig = rig(|_, _r, out, _c, _client| text_turn(&out, "x", None));
    let chats_before = rig.env.core.workspace.watch_chats().borrow().len();
    let bad_schema = AskSpec::new("v", "p", json!({"type": "nonsense"}));
    let failure = rig
        .ask
        .ask("parent", bad_schema, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(failure.error, AskError::Setup(_)),
        "{:?}",
        failure.error
    );
    let failure = rig
        .ask
        .ask("no-such-chat", spec(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(failure.error, AskError::Setup(_)));
    // No IPC port => the child could never submit.
    rig.env.core.sessions.set_ipc_port(0);
    let failure = rig
        .ask
        .ask("parent", spec(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(failure.error, AskError::Setup(_)));
    assert!(failure.child_chat_id.is_none());
    assert_eq!(
        rig.env.core.workspace.watch_chats().borrow().len(),
        chats_before
    );
    assert!(rig.env.runs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_child_of_a_side_chat_hangs_off_the_root_chat() {
    let rig = rig(|_, request, out, _c, client| {
        tokio::spawn(async move {
            submit(&client, &request, json!({"passed": true, "reason": "ok"})).await;
            text_turn(&out, "ok", None);
        });
    });
    rig.env
        .core
        .workspace
        .create_chat_with_parent(
            "side",
            Some("space-main"),
            None,
            None,
            None,
            Some("parent".into()),
        )
        .unwrap();
    let outcome = rig
        .ask
        .ask("side", spec(), CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        child_of(&rig, &outcome.child_chat_id)
            .parent_chat_id
            .as_deref(),
        Some("parent"),
        "one level of nesting"
    );
}

#[tokio::test]
async fn an_ask_child_is_not_revived_by_boot_recovery() {
    // The state a kill -9 mid-run leaves behind: a fresh, unfinished turn in
    // a chat that was a child ask. An ordinary chat would be auto-resumed; the
    // ask's verdict has no waiter any more, so this one must not be.
    use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionDoc, SessionMessageEntry};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("device-id"), "dev-crash").unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let entry = |id: &str, role, status, text: &str| SessionMessageEntry {
        origin: None,
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: "t0".into(),
            text: text.into(),
        }],
        created_at: now - 30_000,
        device_id: "dev-crash".into(),
        status: Some(status),
        continuation_of: None,
        duration_ms: None,
    };
    {
        let store = zeron_sync::DocsStore::open(dir.join("orgs/dev-org/dev-user")).unwrap();
        let doc = SessionDoc::init("verifier").unwrap();
        doc.set_ask_child("ask-1").unwrap();
        doc.push_message(&entry(
            "u1",
            MessageRole::User,
            MessageStatus::Complete,
            "verify",
        ))
        .unwrap();
        doc.push_message(&entry(
            "a1",
            MessageRole::Assistant,
            MessageStatus::Streaming,
            "partial",
        ))
        .unwrap();
        store
            .save_snapshot("verifier", &doc.export_snapshot().unwrap())
            .unwrap();
        let journal =
            zeron_engine::RunJournal::open(dir.join("orgs/dev-org/dev-user/journals")).unwrap();
        journal
            .append(
                "verifier",
                &zeron_proto::AgentEvent::TextDelta {
                    text: "partial".into(),
                },
            )
            .unwrap();
    }
    let runs: Arc<Mutex<Vec<zeron_proto::RunRequest>>> = Default::default();
    let handler: Handler = Arc::new(|_, _r, out, _c| text_turn(&out, "revived", None));
    let env = assemble(&dir, runs.clone(), handler);
    // Give recovery's re-dispatch (the thing under test) every chance to fire.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        runs.lock().unwrap().is_empty(),
        "an ask child must not be revived"
    );
    let entries = env.entries("verifier");
    assert!(
        entries
            .iter()
            .any(|e| e.status == Some(MessageStatus::Aborted)),
        "the crashed turn is still stamped aborted"
    );
}

// ── persistent children: workflow actors ──────────────────────────────────

fn actor_spec(title: &str) -> AskSpec {
    let mut spec = AskSpec::new("agent", "Do part of the job.", schema());
    spec.title = Some(title.into());
    spec.persistent = true;
    spec
}

#[tokio::test]
async fn a_persistent_child_is_hidden_at_once_reused_by_later_asks_and_never_archived_by_them() {
    let rig = rig(|_, request, out, _c, client| {
        tokio::spawn(async move {
            submit(&client, &request, json!({"passed": true, "reason": "ok"})).await;
            text_turn(&out, "done", Some((10, 2)));
        });
    });
    let mut first = actor_spec("Review · security");
    first.workflow_actor = Some(zeron_proto::WorkflowActorTag {
        run_id: "run-1".into(),
        site_id: "3:5-3:20".into(),
        ordinal: 0,
        name: "security".into(),
    });
    let one = rig
        .ask
        .ask("parent", first, CancellationToken::new())
        .await
        .unwrap();
    let child = child_of(&rig, &one.child_chat_id);
    assert!(child.archived, "hidden from the sidebar from the start");
    assert_eq!(child.title.as_deref(), Some("Review · security"));
    assert_eq!(child.parent_chat_id.as_deref(), Some("parent"));
    let handle = rig.env.core.doc_host.open(&one.child_chat_id).unwrap();
    assert_eq!(handle.doc().workflow_actor().unwrap().run_id, "run-1");

    // The next ask continues the same chat: same id, no second child, and
    // only its own tokens are reported.
    let mut second = actor_spec("ignored");
    second.reuse_child = Some(one.child_chat_id.clone());
    let two = rig
        .ask
        .ask("parent", second, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(two.child_chat_id, one.child_chat_id);
    assert_eq!(two.usage.total_tokens(), 12, "{:?}", two.usage);
    assert_eq!(
        child_of(&rig, &two.child_chat_id).title.as_deref(),
        Some("Review · security"),
        "a reused child is not renamed"
    );
    let chats = rig.env.core.workspace.read_chats().unwrap();
    let children = chats.iter().filter(|c| c.parent_chat_id.is_some()).count();
    assert_eq!(children, 1);
    // Two prompts reached the one conversation, each with its own ask id.
    let runs = rig.env.runs.lock().unwrap();
    assert_eq!(runs.len(), 2);
    let ask_ids: Vec<_> = runs
        .iter()
        .map(|r| r.mcp.as_ref().unwrap().env["ZERON_ASK_ID"].clone())
        .collect();
    assert_ne!(ask_ids[0], ask_ids[1]);
}

#[tokio::test]
async fn an_escalation_parks_only_its_ask_and_the_answer_is_the_tool_result() {
    let reply = Arc::new(Mutex::new(None));
    let seen = reply.clone();
    let rig = rig(move |_, request, out, _c, client| {
        let seen = seen.clone();
        tokio::spawn(async move {
            let env = request.mcp.as_ref().unwrap().env.clone();
            // The advertised spec says the child may escalate.
            let info: zeron_proto::AskSpecInfo = client
                .call_as(
                    methods::GET_ASK_SPEC,
                    json!({"chatId": env["ZERON_CHAT_ID"], "askId": env["ZERON_ASK_ID"]}),
                )
                .await
                .unwrap();
            assert!(info.escalation);
            let raised: zeron_proto::EscalateReply = client
                .call_as(
                    methods::ASK_ESCALATE,
                    json!({"chatId": env["ZERON_CHAT_ID"], "askId": env["ZERON_ASK_ID"],
                           "question": "Which database?", "context": "two are configured",
                           "maxWaitMs": 150}),
                )
                .await
                .unwrap();
            assert_eq!(raised.status, zeron_proto::EscalateStatus::Pending);
            // Keep waiting with the question id: this is the answer.
            let answered: zeron_proto::EscalateReply = client
                .call_as(
                    methods::ASK_ESCALATE,
                    json!({"chatId": env["ZERON_CHAT_ID"], "askId": env["ZERON_ASK_ID"],
                           "questionId": raised.question_id, "maxWaitMs": 10000}),
                )
                .await
                .unwrap();
            *seen.lock().unwrap() = Some(answered.clone());
            submit(
                &client,
                &request,
                json!({"passed": true, "reason": answered.answer.unwrap_or_default()}),
            )
            .await;
            text_turn(&out, "done", None);
        });
    });
    let raised = Arc::new(Mutex::new(Vec::new()));
    let sink = raised.clone();
    let mut spec = actor_spec("Asker");
    spec.escalation = Some(zeron_engine::ask::EscalationHook(Arc::new(move |e| {
        sink.lock().unwrap().push(e);
    })));
    // A short timeout proves the parked time is not charged to the ask.
    spec.timeout = Duration::from_millis(1500);
    let ask = rig.ask.clone();
    let task = tokio::spawn(async move { ask.ask("parent", spec, CancellationToken::new()).await });
    wait_for(|| !raised.lock().unwrap().is_empty(), "the escalation").await;
    tokio::time::sleep(Duration::from_millis(2000)).await; // longer than the ask's timeout
    let e = raised.lock().unwrap()[0].clone();
    assert_eq!(e.question, "Which database?");
    assert_eq!(e.context, "two are configured");
    assert!(
        rig.ask
            .answer_escalation(&e.child_chat_id, &e.qid, "Postgres".into())
            .await
    );
    let outcome = task.await.unwrap().expect("ask completes after the answer");
    assert_eq!(outcome.result["reason"], "Postgres");
    let answered = reply.lock().unwrap().clone().unwrap();
    assert_eq!(answered.status, zeron_proto::EscalateStatus::Answered);
    assert!(answered.message.contains("Postgres"));
    assert_eq!(answered.left, 2);
}

#[tokio::test]
async fn escalations_are_limited_per_ask_and_refused_where_not_offered() {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let log = statuses.clone();
    let rig = rig(move |_, request, out, _c, client| {
        let log = log.clone();
        tokio::spawn(async move {
            let env = request.mcp.as_ref().unwrap().env.clone();
            for n in 0..3 {
                let reply: zeron_proto::EscalateReply = client
                    .call_as(
                        methods::ASK_ESCALATE,
                        json!({"chatId": env["ZERON_CHAT_ID"], "askId": env["ZERON_ASK_ID"],
                               "question": format!("q{n}"), "maxWaitMs": 5000}),
                    )
                    .await
                    .unwrap();
                log.lock().unwrap().push((reply.status, reply.left));
            }
            submit(&client, &request, json!({"passed": true, "reason": "ok"})).await;
            text_turn(&out, "done", None);
        });
    });
    let mut spec = actor_spec("Asker");
    spec.max_escalations = 2;
    let ask_backend = rig.ask.clone();
    spec.escalation = Some(zeron_engine::ask::EscalationHook(Arc::new({
        let ask_backend = ask_backend.clone();
        move |e| {
            let ask_backend = ask_backend.clone();
            tokio::spawn(async move {
                ask_backend
                    .answer_escalation(&e.child_chat_id, &e.qid, "yes".into())
                    .await;
            });
        }
    })));
    rig.ask
        .ask("parent", spec, CancellationToken::new())
        .await
        .unwrap();
    use zeron_proto::EscalateStatus::*;
    assert_eq!(
        statuses.lock().unwrap().clone(),
        vec![(Answered, 1), (Answered, 0), (Refused, 0)]
    );
}
