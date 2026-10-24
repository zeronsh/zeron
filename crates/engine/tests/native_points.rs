use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use std::sync::Arc;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::*;

struct Exact;
#[async_trait]
impl Harness for Exact {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Exact"
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
        _: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        Ok(futures::stream::iter(
            vec![
                AgentEvent::SessionStarted {
                    harness: HarnessId::Mock,
                    model: "test".into(),
                    tools: vec![],
                    cwd: request.cwd.clone(),
                    session_id: "native".into(),
                    assistant_message_id: "wire-reply".into(),
                },
                AgentEvent::TextDelta {
                    text: "first reply".into(),
                },
                AgentEvent::NativeForkReady {
                    assistant_message_id: "wire-reply".into(),
                    point: NativeForkPoint {
                        format_version: 1,
                        harness: HarnessId::Mock,
                        source_device_id: "host".into(),
                        source_session_id: "native".into(),
                        cwd: request.cwd,
                        boundary: NativeForkBoundary::AppServerTurn {
                            turn_id: "turn1".into(),
                        },
                    },
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some("native".into()),
                },
            ]
            .into_iter()
            .map(Ok),
        )
        .boxed())
    }
}
#[tokio::test]
async fn native_fork_point_is_bound_before_done_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(Exact));
    let core = EngineCore::assemble(dir.path(), registry.clone(), HarnessId::Mock, None).unwrap();
    core.workspace
        .create_chat(
            "main",
            None,
            Some(&core.device_id),
            None,
            Some("/tmp".into()),
        )
        .unwrap();
    let request = serde_json::from_value(serde_json::json!({"prompt":"hello", "model":null,"reasoning":null,"cwd":"/tmp","sandbox":"read-only","resume":null})).unwrap();
    core.sessions
        .dispatch("main", HarnessId::Mock, request, Some("u1".into()))
        .await
        .unwrap();
    let doc = core.doc_host.open("main").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if doc.doc().read_entries().unwrap().iter().any(|e| {
                e.native_fork_point.is_some()
                    && e.status == Some(zeron_doc::MessageStatus::Complete)
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let entries = doc.doc().read_entries().unwrap();
    let reply = entries
        .iter()
        .find(|e| e.native_fork_point.is_some())
        .unwrap();
    assert_ne!(reply.id, "wire-reply");
    assert_eq!(
        reply.native_fork_point.as_ref().unwrap().source_device_id,
        core.device_id
    );
    let (events, _) = core.sessions.subscribe("main", 0).unwrap();
    assert!(matches!(
        events.last().map(|event| &event.event),
        Some(AgentEvent::Done { .. })
    ));
    let recorded = events
        .iter()
        .find_map(|event| match &event.event {
            AgentEvent::NativeForkReady {
                assistant_message_id,
                point,
            } => Some((assistant_message_id, point)),
            _ => None,
        })
        .unwrap();
    assert_eq!(recorded.0, &reply.id);
    assert_eq!(recorded.1, reply.native_fork_point.as_ref().unwrap());
    core.shutdown().await;
    drop(doc);
    drop(core);
    let restarted = EngineCore::assemble(dir.path(), registry, HarnessId::Mock, None).unwrap();
    let reopened = restarted
        .doc_host
        .open("main")
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    assert_eq!(reopened, entries);
    restarted.shutdown().await;
}
