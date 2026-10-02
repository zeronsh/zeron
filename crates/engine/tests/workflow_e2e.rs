//! A workflow against real child chats: the engine's own ask service, a
//! scripted harness that answers like `zeron mcp` would (reads the ask's spec,
//! calls `submit_result`), and the real approval question riding the parent
//! chat's live turn. The parent agent is scripted to call `WorkflowStart` the
//! way the MCP tool does.

mod support;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};
use support::*;
use zeron_proto::{
    AgentEvent, ChatConfig, HarnessId, MessageOrigin, RunRequest, SandboxLevel, SessionStatus,
    UserInputAnswer, WorkflowStatus,
};
use zeron_rpc::{RpcClient, methods};

const CHAT: &str = "parent";

type ClientSlot = Arc<OnceLock<Arc<RpcClient>>>;

struct Rig {
    env: Env,
    start_result: Arc<Mutex<Option<Result<Value, String>>>>,
    project: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

const SCRIPT: &str = r#"
def main(args):
    phase("review")
    hs = [agent("reviewer-" + str(i)).ask("REVIEW " + str(i)) for i in range(3)]
    notes = results(hs)
    report({"reviewed": len(notes)})
    phase("gate")
    gate = run("sh", ["-c", "echo gate-ok"])
    artifact.markdown("summary", "Summary", "reviewed " + str(len(notes)))
    return {"notes": notes, "gate": gate.stdout.strip()}
"#;

async fn answer_ask(client: &RpcClient, request: &RunRequest, out: &Out) {
    let env = &request.mcp.as_ref().expect("child carries the ask MCP").env;
    let info: zeron_proto::AskSpecInfo = client
        .call_as(
            methods::GET_ASK_SPEC,
            json!({"chatId": env["ZERON_CHAT_ID"], "askId": env["ZERON_ASK_ID"]}),
        )
        .await
        .unwrap();
    let props = &info.result_schema["properties"];
    let result = if props.get("text").is_some() {
        json!({"text": format!("looked at it: {}", request.prompt.lines().find(|l| l.starts_with("REVIEW")).unwrap_or("?"))})
    } else {
        json!({})
    };
    let reply: zeron_proto::AskSubmitReply = client
        .call_as(
            methods::SUBMIT_ASK_RESULT,
            json!({"chatId": env["ZERON_CHAT_ID"], "askId": env["ZERON_ASK_ID"], "result": result}),
        )
        .await
        .unwrap();
    assert!(reply.accepted, "{reply:?}");
    text_turn(out, "submitted", Some((20, 5)));
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let slot: ClientSlot = Arc::new(OnceLock::new());
    let start_result = Arc::new(Mutex::new(None));
    let (handler_slot, result_slot) = (slot.clone(), start_result.clone());
    let handler: Handler = Arc::new(move |_, request, out, _controls| {
        let client = handler_slot.get().expect("client wired").clone();
        let result_slot = result_slot.clone();
        tokio::spawn(async move {
            if request
                .mcp
                .as_ref()
                .is_some_and(|m| m.env.contains_key("ZERON_ASK_ID"))
            {
                answer_ask(&client, &request, &out).await;
            } else if request.prompt == "start the workflow" {
                // The parent agent calling the `start_workflow` MCP tool.
                let r: Result<Value, _> = client
                    .call_as(
                        methods::WORKFLOW_START,
                        json!({"chatId": CHAT, "name": "Demo", "script": SCRIPT}),
                    )
                    .await;
                *result_slot.lock().unwrap() = Some(r.map_err(|e| e.to_string()));
                text_turn(&out, "I started the workflow.", None);
            } else {
                text_turn(&out, "Relaying the workflow result.", None);
            }
        });
    });
    let env = assemble(&dir.path().join("data"), Default::default(), handler);
    env.core.sessions.set_ipc_port(1);
    let client = Arc::new(zeron_rpc::memory_client(env.core.rpc_service()));
    slot.set(client).ok();
    let space = "space-main".to_string();
    env.core
        .workspace
        .create_space(
            &space,
            &env.core.device_id,
            &project.to_string_lossy(),
            None,
            false,
        )
        .unwrap();
    env.core
        .workspace
        .create_chat(
            CHAT,
            Some(&space),
            None,
            Some(ChatConfig {
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            Some(project.to_string_lossy().into_owned()),
        )
        .unwrap();
    env.core.workspace.rename_chat(CHAT, "Parent").unwrap();
    Rig {
        env,
        start_result,
        project,
        _dir: dir,
    }
}

async fn run_parent_turn(rig: &Rig) {
    rig.env
        .core
        .sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            RunRequest {
                prompt: "start the workflow".into(),
                harness: Some(HarnessId::Mock),
                model: None,
                reasoning: None,
                model_options: Default::default(),
                cwd: rig.project.to_string_lossy().into_owned(),
                sandbox: SandboxLevel::WorkspaceWrite,
                auto_approve: false,
                resume: None,
                attachments: Vec::new(),
                worktree: None,
                mcp: None,
            },
            None,
        )
        .await
        .unwrap();
}

/// The pending approval question of the parent turn.
fn pending_question(rig: &Rig) -> Option<(String, zeron_proto::UserInputQuestion)> {
    let (replay, _rx) = rig.env.core.sessions.subscribe(CHAT, 0).ok()?;
    replay.iter().rev().find_map(|e| match &e.event {
        AgentEvent::InputRequested {
            request_id,
            questions,
        } => Some((request_id.clone(), questions[0].clone())),
        _ => None,
    })
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approved_workflow_runs_real_child_chats_and_reports_back_to_the_parent() {
    let rig = rig();
    run_parent_turn(&rig).await;
    // The approval question is a normal input request on the live turn.
    wait_for(|| pending_question(&rig).is_some(), "the approval question").await;
    let (request_id, question) = pending_question(&rig).unwrap();
    assert!(
        question.question.contains("Run workflow \"Demo\"?"),
        "{}",
        question.question
    );
    assert!(
        question.question.contains("Phases: review (1) → gate (1)"),
        "{}",
        question.question
    );
    assert!(question.question.contains("$ sh -c echo gate-ok"));
    assert_eq!(question.meta.as_ref().unwrap()["kind"], "workflowApproval");
    wait_for(
        || {
            rig.env
                .core
                .sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::AwaitingInput)
        },
        "the chat to show it awaits input",
    )
    .await;
    assert!(
        rig.env
            .core
            .sessions
            .respond_input(
                CHAT,
                &request_id,
                vec![UserInputAnswer {
                    question_id: question.id.clone(),
                    labels: vec!["Run workflow".into()]
                }],
            )
            .unwrap()
    );

    // start_workflow returns once approved, with the run id.
    wait_for(
        || rig.start_result.lock().unwrap().is_some(),
        "start_workflow to return",
    )
    .await;
    let started = rig
        .start_result
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .expect("approved");
    let run_id = started["runId"].as_str().unwrap().to_owned();
    assert_eq!(started["graph"]["phases"][0]["name"], "review");

    let svc = rig.env.core.doc_host.workflows().unwrap();
    wait_for_within(
        || {
            svc.get(&run_id)
                .map(|v| v.run.header.status.is_settled())
                .unwrap_or(false)
        },
        "the run to settle",
        Duration::from_secs(30),
    )
    .await;
    let view = svc.get(&run_id).unwrap();
    assert_eq!(
        view.run.header.status,
        WorkflowStatus::Completed,
        "{:?}",
        view.run.header
    );
    let result = view.result.clone().unwrap();
    assert_eq!(result["gate"], "gate-ok");
    assert_eq!(result["notes"].as_array().unwrap().len(), 3);
    assert!(
        result["notes"][0]
            .as_str()
            .unwrap()
            .starts_with("looked at it: REVIEW")
    );

    // Three real child chats: hidden (archived), one level under the parent,
    // titled by run and actor, tagged, and still readable by id.
    let chats = rig.env.core.workspace.read_chats().unwrap();
    let children: Vec<_> = chats
        .iter()
        .filter(|c| c.parent_chat_id.as_deref() == Some(CHAT))
        .collect();
    assert_eq!(
        children.len(),
        3,
        "{:?}",
        chats
            .iter()
            .map(|c| (c.id.clone(), c.parent_chat_id.clone(), c.archived))
            .collect::<Vec<_>>()
    );
    for child in &children {
        assert!(child.archived);
        assert!(
            child
                .title
                .as_deref()
                .unwrap()
                .starts_with("Demo · reviewer-"),
            "{:?}",
            child.title
        );
        let handle = rig.env.core.doc_host.open(&child.id).unwrap();
        let tag = handle
            .doc()
            .workflow_actor()
            .expect("tagged as a workflow actor");
        assert_eq!(tag.run_id, run_id);
        assert!(
            !handle.doc().read_entries().unwrap().is_empty(),
            "its transcript is readable"
        );
    }
    let actor_children: Vec<_> = view
        .run
        .actors
        .iter()
        .filter_map(|a| a.child_chat_id.clone())
        .collect();
    assert_eq!(
        actor_children.len(),
        3,
        "the state links each actor to its chat"
    );
    assert!(
        view.run.header.usage.total_tokens() >= 75,
        "{:?}",
        view.run.header.usage
    );

    // The completion message reached the parent chat, as a machine-origin turn.
    wait_for(
        || {
            rig.env.user_text(CHAT).iter().any(|(_, o)| {
                matches!(
                    o,
                    Some(MessageOrigin::Workflow {
                        status: WorkflowStatus::Completed,
                        ..
                    })
                )
            })
        },
        "the completion message in the parent chat",
    )
    .await;
    let (body, _) = rig
        .env
        .user_text(CHAT)
        .into_iter()
        .find(|(_, o)| matches!(o, Some(MessageOrigin::Workflow { .. })))
        .unwrap();
    assert!(body.starts_with("[Workflow completed] Demo"), "{body}");
    assert!(
        body.contains("gate-ok") && body.contains("summary (markdown): Summary"),
        "{body}"
    );
    // …and it woke the idle parent: one more agent turn replied to it.
    wait_for(
        || {
            rig.env
                .prompts()
                .iter()
                .any(|p| p.starts_with("[Workflow completed]"))
        },
        "the parent to be woken",
    )
    .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_workflow_returns_an_error_and_runs_nothing() {
    let rig = rig();
    run_parent_turn(&rig).await;
    wait_for(|| pending_question(&rig).is_some(), "the approval question").await;
    let (request_id, question) = pending_question(&rig).unwrap();
    rig.env
        .core
        .sessions
        .respond_input(
            CHAT,
            &request_id,
            vec![UserInputAnswer {
                question_id: question.id,
                labels: vec!["Deny".into()],
            }],
        )
        .unwrap();
    wait_for(
        || rig.start_result.lock().unwrap().is_some(),
        "start_workflow to return",
    )
    .await;
    let err = rig
        .start_result
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .unwrap_err();
    assert!(err.contains("denied"), "{err}");
    let chats = rig.env.core.workspace.read_chats().unwrap();
    assert!(
        chats.iter().all(|c| c.parent_chat_id.is_none()),
        "no child chat was created"
    );
    let runs = rig.env.core.doc_host.workflows().unwrap().list(Some(CHAT));
    assert_eq!(runs[0].status, WorkflowStatus::Stopped);
}
