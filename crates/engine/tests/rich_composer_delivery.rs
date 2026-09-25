//! Canonical editor selections survive persistence, queue editing and retries;
//! provider text is produced only at each fresh-run or steering boundary.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use tokio::sync::mpsc;
use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::doc_host::{
    BeginQueueEditOutcome, FinishQueueEditAction, FinishQueueEditOutcome,
};
use zeron_engine::{EngineCore, HarnessRegistry, SteerOutcome};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::invocation::Invocation;
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};

const CHAT: &str = "rich-delivery";
const HARNESSES: [HarnessId; 9] = [
    HarnessId::ClaudeCode,
    HarnessId::Codex,
    HarnessId::Cursor,
    HarnessId::Devin,
    HarnessId::Grok,
    HarnessId::Hermes,
    HarnessId::Pi,
    HarnessId::Antigravity,
    HarnessId::Opencode,
];

enum Delivery {
    Run(RunRequest),
    Steer(String),
}
struct RecordingHarness {
    id: HarnessId,
    delivery: mpsc::UnboundedSender<Delivery>,
    fail_start: AtomicBool,
}
#[async_trait]
impl Harness for RecordingHarness {
    fn id(&self) -> HarnessId {
        self.id
    }
    fn display_name(&self) -> &str {
        "Rich reference delivery probe"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn commands_for(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
        Ok(vec![zeron_proto::SlashCommand {
            name: "probe".into(),
            description: cwd.to_string_lossy().into_owned(),
            input_hint: None,
        }])
    }
    async fn skills(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        Ok(Some(vec![zeron_proto::invocation::Skill {
            name: "probe".into(),
            path: cwd.join("SKILL.md").to_string_lossy().into_owned(),
            description: cwd.to_string_lossy().into_owned(),
            enabled: true,
            command: None,
        }]))
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.delivery.send(Delivery::Run(request.clone())).unwrap();
        if self.fail_start.swap(false, Ordering::SeqCst) {
            return Ok(futures::stream::iter([Ok(AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some(
                    "probe exited unexpectedly (exit code 1): temporary startup failure".into(),
                ),
                session_id: None,
            })])
            .boxed());
        }
        let started = futures::stream::iter([Ok(AgentEvent::SessionStarted {
            harness: self.id,
            model: "probe".into(),
            tools: vec![],
            cwd: request.cwd,
            session_id: "rich-session".into(),
            assistant_message_id: "rich-assistant".into(),
        })]);
        let delivery = self.delivery.clone();
        let stream = futures::stream::unfold(
            (controls, delivery),
            |(mut controls, delivery)| async move {
                tokio::select! {
                    _ = controls.interrupt.cancelled() => None,
                    message = controls.steering.recv() => {
                        let message = message?;
                        delivery.send(Delivery::Steer(message.prompt)).unwrap();
                        Some((Ok(AgentEvent::Steered { assistant_message_id: None, next_assistant_message_id: None }), (controls, delivery)))
                    }
                }
            },
        );
        Ok(started.chain(stream).boxed())
    }
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: None,
        attachments: vec![],
        worktree: None,
    }
}
fn rich_text(label: &str) -> (String, String) {
    let skill = Invocation::Skill {
        name: "review".into(),
        path: "/repo/é skills/SKILL.md".into(),
        command: None,
    }
    .link();
    let file = zeron_proto::file_mentions::local_file_link("src/é file.rs", false);
    let raw = format!(
        "{label}: {skill} on {file}\n\n- **keep this markdown**\n- `literal @file $review /compact`"
    );
    let readable = format!(
        "{label}: Use the skill [$review](/repo/%C3%A9%20skills/SKILL.md) on [é file.rs](src/%C3%A9%20file.rs)\n\n- **keep this markdown**\n- `literal @file $review /compact`"
    );
    (raw, readable)
}
fn expected(raw: &str, readable: &str, id: HarnessId) -> String {
    if id == HarnessId::Codex {
        raw.replace(
            &zeron_proto::file_mentions::local_file_link("src/é file.rs", false),
            "[é file.rs](src/%C3%A9%20file.rs)",
        )
    } else if id == HarnessId::Opencode {
        // OpenCode receives canonical identity and converts it locally so a
        // disappearing project command cannot become ordinary slash text.
        raw.into()
    } else {
        readable.into()
    }
}
fn assert_delivered(actual: &str, raw: &str, readable: &str, id: HarnessId) {
    let expected = expected(raw, readable, id);
    if id == HarnessId::Cursor
        && let Some(json) = actual.strip_prefix("The preceding user messages may not have reached a Cursor checkpoint before startup stopped. Retain this JSON as conversation history; do not rerun prior tools or side effects. Respond to the current message.\n")
    {
        let history: serde_json::Value = serde_json::from_str(json)
            .expect("rich selections must not corrupt Cursor's recovery JSON");
        assert_eq!(history["currentUserMessage"], expected, "{id:?}");
        assert!(
            history["previousUserMessages"]
                .as_array()
                .is_some_and(|messages| !messages.is_empty()),
            "Cursor recovery must retain the preceding canonical user turns"
        );
    } else {
        assert_eq!(actual, expected, "{id:?}");
    }
}
async fn receive(rx: &mut mpsc::UnboundedReceiver<Delivery>) -> Delivery {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("delivery timed out")
        .expect("delivery channel closed")
}
async fn setup(
    id: HarnessId,
) -> (
    tempfile::TempDir,
    EngineCore,
    Arc<RecordingHarness>,
    mpsc::UnboundedReceiver<Delivery>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let (delivery, rx) = mpsc::unbounded_channel();
    let harness = Arc::new(RecordingHarness {
        id,
        delivery,
        fail_start: AtomicBool::new(false),
    });
    let registry = HarnessRegistry::new();
    registry.register(harness.clone());
    let core =
        EngineCore::assemble(&tmp.path().join("data"), Arc::new(registry), id, None).unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({"op":"createChat", "chatId":CHAT, "deviceId":core.device_id}),
        )
        .await
        .unwrap();
    core.workspace
        .rename_chat(CHAT, "Rich reference test")
        .unwrap();
    (tmp, core, harness, rx)
}
fn assert_persisted(core: &EngineCore, raw: &str, count: usize) {
    let users: Vec<_> = core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .filter(|entry| entry.role == MessageRole::User)
        .collect();
    assert_eq!(users.len(), count);
    assert!(
        users.iter().any(|entry| entry
            .parts
            .iter()
            .any(|part| matches!(part, MessagePart::Text { text, .. } if text == raw))),
        "canonical rich selections missing from transcript"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rich_selections_survive_fresh_warm_steer_and_attachment_delivery_for_every_harness() {
    for id in HARNESSES {
        let (_tmp, core, _harness, mut rx) = setup(id).await;
        let (raw, readable) = rich_text("first");
        core.sessions
            .dispatch(CHAT, id, request(&raw), None)
            .await
            .unwrap();
        let Delivery::Run(run) = receive(&mut rx).await else {
            panic!("fresh run expected")
        };
        assert_delivered(&run.prompt, &raw, &readable, id);
        assert_eq!(core.sessions.last_request(CHAT).unwrap().prompt, raw);
        assert_persisted(&core, &raw, 1);

        let (warm, readable) = rich_text("warm");
        core.sessions
            .dispatch(CHAT, id, request(&warm), None)
            .await
            .unwrap();
        let Delivery::Steer(text) = receive(&mut rx).await else {
            panic!("warm run must use mailbox")
        };
        assert_eq!(text, expected(&warm, &readable, id), "{id:?}");
        assert_persisted(&core, &warm, 2);

        let (steer, readable) = rich_text("steer");
        assert!(matches!(
            core.sessions.steer(CHAT, &steer, None).await.unwrap(),
            SteerOutcome::Accepted
        ));
        let Delivery::Steer(text) = receive(&mut rx).await else {
            panic!("steer expected")
        };
        assert_eq!(text, expected(&steer, &readable, id), "{id:?}");
        assert_persisted(&core, &steer, 3);

        let (attached, readable) = rich_text("attachment");
        let path = _tmp.path().join("diagram.png");
        std::fs::write(&path, b"image fixture").unwrap();
        let mut req = request(&attached);
        req.attachments.push(path.to_string_lossy().into_owned());
        core.sessions
            .dispatch(CHAT, id, req.clone(), None)
            .await
            .unwrap();
        let Delivery::Run(run) = receive(&mut rx).await else {
            panic!("attachments require a new run")
        };
        assert_delivered(&run.prompt, &attached, &readable, id);
        assert_eq!(run.attachments, req.attachments);
        assert_persisted(&core, &attached, 4);
        core.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rich_selections_are_converted_once_on_startup_retry_for_every_harness() {
    for id in HARNESSES {
        let (_tmp, core, harness, mut rx) = setup(id).await;
        // Startup retries are intentionally limited to an engine-injected
        // resume: establish a real prior session before simulating its crash.
        core.sessions
            .dispatch(CHAT, id, request("seed session"), None)
            .await
            .unwrap();
        let Delivery::Run(_) = receive(&mut rx).await else {
            panic!("seed run expected")
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while core
                .workspace
                .chat(CHAT)
                .unwrap()
                .unwrap()
                .harness_session_id
                .is_none()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("session ID was not persisted");
        core.sessions.interrupt(CHAT).await.unwrap();
        harness.fail_start.store(true, Ordering::SeqCst);
        let (raw, readable) = rich_text("retry");
        core.sessions
            .dispatch(CHAT, id, request(&raw), Some("same-user".into()))
            .await
            .unwrap();
        for _ in 0..2 {
            let Delivery::Run(run) = receive(&mut rx).await else {
                panic!("fresh run expected")
            };
            assert_delivered(&run.prompt, &raw, &readable, id);
        }
        assert_persisted(&core, &raw, 2);
        assert_eq!(core.sessions.last_request(CHAT).unwrap().prompt, raw);
        core.shutdown().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_edits_preserve_reselected_skills_until_delivery_for_every_harness() {
    for id in HARNESSES {
        let (_tmp, core, _harness, mut rx) = setup(id).await;
        core.sessions
            .dispatch(CHAT, id, request("opening"), None)
            .await
            .unwrap();
        let Delivery::Run(_) = receive(&mut rx).await else {
            panic!("fresh run expected")
        };
        let (old, _) = rich_text("old");
        let queued = core.doc_host.queue_message(CHAT, &old, vec![]).unwrap();
        let BeginQueueEditOutcome::Acquired {
            lease_id,
            base_text_hash,
            ..
        } = core
            .doc_host
            .begin_queued_message_edit(CHAT, &queued, "test", "editor")
            .await
            .unwrap()
        else {
            panic!("edit expected")
        };
        let (edited, readable) = rich_text("edited");
        assert!(matches!(
            core.doc_host
                .finish_queued_message_edit(
                    CHAT,
                    &queued,
                    &lease_id,
                    FinishQueueEditAction::Commit,
                    Some(&edited),
                    Some(&base_text_hash)
                )
                .await
                .unwrap(),
            FinishQueueEditOutcome::Committed
        ));
        let queue = core
            .doc_host
            .open(CHAT)
            .unwrap()
            .doc()
            .read_queue()
            .unwrap();
        assert_eq!(queue[0].text, edited);
        assert!(core.doc_host.send_queued_now(CHAT, &queued).await.unwrap());
        let Delivery::Run(run) = receive(&mut rx).await else {
            panic!("send now replaces run")
        };
        assert_delivered(&run.prompt, &edited, &readable, id);
        assert_persisted(&core, &edited, 2);
        core.shutdown().await;
    }
}

#[tokio::test]
async fn projectless_catalogs_use_the_session_directory_and_reject_unknown_targets() {
    let (tmp, core, _harness, _rx) = setup(HarnessId::Codex).await;
    let client = zeron_rpc::memory_client(core.rpc_service());
    for method in [
        zeron_rpc::methods::LIST_COMMANDS,
        zeron_rpc::methods::LIST_SKILLS,
    ] {
        let new_chat = client
            .call(method, serde_json::json!({"harness":"codex"}))
            .await
            .unwrap();
        let existing_chat = client
            .call(
                method,
                serde_json::json!({"harness":"codex", "chatId":CHAT}),
            )
            .await
            .unwrap();
        assert_eq!(
            existing_chat, new_chat,
            "projectless chat should use home: {method}"
        );
        assert!(
            client
                .call(
                    method,
                    serde_json::json!({"harness":"codex", "chatId":"missing-chat"})
                )
                .await
                .is_err()
        );
        assert!(
            client
                .call(
                    method,
                    serde_json::json!({"harness":"codex", "chatId":CHAT, "path":"/tmp"})
                )
                .await
                .is_err()
        );
    }
    let cwd = tmp.path().join("session-folder");
    std::fs::create_dir(&cwd).unwrap();
    core.workspace
        .set_chat_cwd(CHAT, cwd.to_str().unwrap())
        .unwrap();
    for method in [
        zeron_rpc::methods::LIST_COMMANDS,
        zeron_rpc::methods::LIST_SKILLS,
    ] {
        let result = client
            .call(
                method,
                serde_json::json!({"harness":"codex", "chatId":CHAT}),
            )
            .await
            .unwrap();
        assert_eq!(result[0]["description"], cwd.to_str().unwrap(), "{method}");
    }
}
