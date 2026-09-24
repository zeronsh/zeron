//! `SearchTranscripts` end to end. Two chats run mock-harness turns queued
//! over the RPC transport, then the palette's search call finds words that
//! exist only inside their transcripts. Covers prefix matching, FTS operator
//! text, hidden reasoning, chat deletion, restart, and rebuilding a deleted
//! index. Writes the observed replies to `target/transcript-search-e2e.json`.

use std::sync::Arc;
use std::time::Duration;

use zeron_doc::{MessageRole, MessageStatus, SessionCommandPayload};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::mock::MockHarness;
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel};
use zeron_rpc::{RpcClient, methods};

const DEPLOY: &str = "chat-deploy";
const LUNCH: &str = "chat-lunch";

fn script() -> Vec<AgentEvent> {
    vec![
        AgentEvent::ReasoningDelta {
            text: "hiddenthought".into(),
        },
        AgentEvent::TextDelta {
            text: "The rollout stalled on a stale Kubernetes configmap.".into(),
        },
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        },
    ]
}

fn assemble(dir: &std::path::Path) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness { script: script() }));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).expect("engine assembles")
}

async fn create_chat(client: &RpcClient, core: &EngineCore, chat: &str) {
    client
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": chat, "deviceId": core.device_id }),
        )
        .await
        .expect("createChat");
    core.workspace
        .rename_chat(chat, "Untitled")
        .expect("pre-title keeps the titler out");
}

async fn run_turn(client: &RpcClient, chat: &str, prompt: &str) {
    let command = serde_json::to_value(SessionCommandPayload::Run {
        request: RunRequest {
            prompt: prompt.into(),
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
        message_id: format!("{chat}-m1"),
    })
    .unwrap();
    client
        .call(
            methods::QUEUE_COMMAND,
            serde_json::json!({ "chatId": chat, "command": command }),
        )
        .await
        .expect("QueueCommand");
}

fn turn_complete(core: &EngineCore, chat: &str) -> bool {
    core.doc_host
        .open(chat)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
        .iter()
        .any(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
}

async fn search(client: &RpcClient, query: &str) -> Vec<(String, String)> {
    let reply = client
        .call(
            methods::SEARCH_TRANSCRIPTS,
            serde_json::json!({ "query": query, "limit": 30 }),
        )
        .await
        .unwrap_or_else(|e| panic!("SearchTranscripts {query:?}: {e}"));
    let hits: Vec<zeron_proto::TranscriptSearchHit> = serde_json::from_value(reply).unwrap();
    let mut hits: Vec<_> = hits.into_iter().map(|h| (h.chat_id, h.snippet)).collect();
    hits.sort();
    hits
}

async fn chats_for(client: &RpcClient, query: &str) -> Vec<String> {
    search(client, query)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

async fn eventually(client: &RpcClient, query: &str, want: &[&str]) -> Vec<(String, String)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let hits = search(client, query).await;
        if hits
            .iter()
            .map(|(id, _)| id.as_str())
            .eq(want.iter().copied())
        {
            return hits;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{query:?}: wanted {want:?}, still {hits:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn palette_search_finds_words_inside_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let mut log = serde_json::Map::new();

    let core = assemble(&data);
    let client = zeron_rpc::memory_client(core.rpc_service());
    create_chat(&client, &core, DEPLOY).await;
    create_chat(&client, &core, LUNCH).await;
    run_turn(&client, DEPLOY, "why does the pipeline fail on tuesdays?").await;
    run_turn(&client, LUNCH, "where should we get lunch").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !(turn_complete(&core, DEPLOY) && turn_complete(&core, LUNCH)) {
        assert!(tokio::time::Instant::now() < deadline, "turns complete");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let user_only = eventually(&client, "tuesdays", &[DEPLOY]).await;
    assert!(
        user_only[0].1.contains("tuesdays"),
        "snippet shows the match: {user_only:?}"
    );
    log.insert("tuesdays".into(), serde_json::json!(user_only));

    let assistant = eventually(&client, "kube config", &[DEPLOY, LUNCH]).await;
    log.insert("kube config".into(), serde_json::json!(assistant));

    assert!(
        chats_for(&client, "hiddenthought").await.is_empty(),
        "reasoning is not indexed"
    );
    assert!(
        chats_for(&client, "lunch tuesdays").await.is_empty(),
        "all words must match"
    );
    assert!(
        chats_for(&client, "").await.is_empty(),
        "empty query only warms"
    );
    for hostile in ["-", "\"", "NEAR(", "lunch OR", "a:b*", "\"lunch"] {
        let hits = chats_for(&client, hostile).await;
        log.insert(format!("hostile {hostile}"), serde_json::json!(hits));
    }
    assert_eq!(chats_for(&client, "\"lunch").await, vec![LUNCH.to_string()]);

    client
        .call(
            methods::MUTATE,
            serde_json::json!({ "op": "deleteChat", "chatId": LUNCH }),
        )
        .await
        .expect("deleteChat");
    let after_delete = eventually(&client, "configmap", &[DEPLOY]).await;
    log.insert(
        "configmap after delete".into(),
        serde_json::json!(after_delete),
    );

    core.shutdown().await;
    drop(client);
    drop(core);

    let core = assemble(&data);
    let client = zeron_rpc::memory_client(core.rpc_service());
    let restarted = eventually(&client, "tuesdays", &[DEPLOY]).await;
    log.insert(
        "tuesdays after restart".into(),
        serde_json::json!(restarted),
    );
    core.shutdown().await;
    drop(client);
    drop(core);

    let index = walk(&data)
        .into_iter()
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n == "transcript-search.sqlite3")
        })
        .expect("index file exists under the profile store");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", index.display()));
    }
    let core = assemble(&data);
    let client = zeron_rpc::memory_client(core.rpc_service());
    let rebuilt = eventually(&client, "tuesdays", &[DEPLOY]).await;
    log.insert(
        "tuesdays after index deleted".into(),
        serde_json::json!(rebuilt),
    );
    core.shutdown().await;

    let artifact = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/transcript-search-e2e.json");
    std::fs::write(&artifact, serde_json::to_string_pretty(&log).unwrap()).unwrap();
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
