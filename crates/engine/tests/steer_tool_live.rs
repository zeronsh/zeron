//! Opt-in real-model check that steering never aborts a running tool: the
//! agent calls a slow MCP tool (15 s), the user steers while it runs, and the
//! tool must still finish and its real result must reach the agent — then
//! the steer is answered.
//!
//! ZERON_TEST_HARNESS=claude cargo test -p zeron-engine --test steer_tool_live -- --ignored --nocapture
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use zeron_doc::{MessagePart, MessageRole, SessionCommandPayload, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{
    AcpHarness, ClaudeHarness, CodexHarness, CursorHarness, Harness, OpencodeHarness,
};
use zeron_proto::{ChatConfig, McpServer, RunRequest, SandboxLevel, SessionStatus};

const CHAT: &str = "steer-tool";

/// A stdio MCP server whose one tool takes 15 s. It records the call's start
/// and whether it ran to completion.
const SLOW_MCP: &str = r#"
import json, os, sys, time
d = os.environ["SLOW_MCP_DIR"]
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    method = m.get("method")
    if method == "initialize":
        r = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
             "serverInfo": {"name": "slow", "version": "1"}}
    elif method == "tools/list":
        r = {"tools": [{"name": "slow_wait",
                        "description": "Waits 15 seconds, then returns a secret word.",
                        "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "tools/call":
        open(os.path.join(d, "called"), "w").write("1")
        time.sleep(15)
        open(os.path.join(d, "finished"), "w").write("1")
        r = {"content": [{"type": "text", "text": "The secret word is MARMALADE."}]}
    else:
        r = {}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

fn harness(name: &str) -> Arc<dyn Harness> {
    match name {
        "claude" => Arc::new(ClaudeHarness::new()),
        "codex" => Arc::new(CodexHarness::new()),
        "cursor" => Arc::new(CursorHarness::new()),
        "opencode" => Arc::new(OpencodeHarness::new()),
        "grok" => Arc::new(AcpHarness::grok()),
        "devin" => Arc::new(AcpHarness::devin()),
        "hermes" => Arc::new(AcpHarness::hermes()),
        "pi" => Arc::new(AcpHarness::pi()),
        "antigravity" => Arc::new(AcpHarness::antigravity()),
        _ => panic!("unknown harness {name}"),
    }
}

fn text(entry: &SessionMessageEntry) -> String {
    entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn dump(entries: &[SessionMessageEntry]) -> String {
    entries
        .iter()
        .map(|e| {
            format!(
                "  {:?} {:?}",
                e.role,
                text(e).chars().take(140).collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
#[ignore = "uses real model quota; select the harness explicitly"]
async fn steering_never_aborts_a_running_tool() {
    let name = std::env::var("ZERON_TEST_HARNESS").expect("select harness");
    let model = std::env::var("ZERON_TEST_MODEL").ok();
    let harness = harness(&name);
    let id = harness.id();
    let dir = tempfile::tempdir().unwrap();
    let mcp_dir = dir.path().join("mcp");
    std::fs::create_dir(&mcp_dir).unwrap();
    let script = dir.path().join("slow_mcp.py");
    std::fs::write(&script, SLOW_MCP).unwrap();
    let registry = HarnessRegistry::new();
    registry.register(harness);
    let core =
        EngineCore::assemble(&dir.path().join("engine"), Arc::new(registry), id, None).unwrap();
    let cwd = dir.path().to_str().unwrap().to_owned();
    core.workspace
        .create_space(CHAT, &core.device_id, &cwd, None, false)
        .unwrap();
    core.workspace
        .create_chat(CHAT, Some(CHAT), None, None, None)
        .unwrap();
    core.workspace
        .set_chat_config(
            CHAT,
            &ChatConfig {
                harness: id,
                model: model.clone(),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::DangerFullAccess,
            },
        )
        .unwrap();
    let entries = || {
        core.doc_host
            .open(CHAT)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
    };
    let status = || core.sessions.session_status(CHAT).map(|s| s.status);
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                message_id: "call".into(),
                request: RunRequest {
                    mcp: Some(McpServer {
                        name: "slow".into(),
                        command: "python3".into(),
                        args: vec![script.to_str().unwrap().into()],
                        env: [("SLOW_MCP_DIR".to_owned(), mcp_dir.display().to_string())]
                            .into_iter()
                            .collect(),
                    }),
                    prompt: "Call the slow_wait tool from the `slow` MCP server exactly once and \
                             wait for it, then tell me the secret word it returned."
                        .into(),
                    harness: Some(id),
                    model,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: cwd.clone(),
                    sandbox: SandboxLevel::DangerFullAccess,
                    auto_approve: true,
                    attachments: vec![],
                    worktree: None,
                    resume: None,
                },
            },
        )
        .unwrap();
    let started = Instant::now();
    while !mcp_dir.join("called").exists() {
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "tool never called\n{}",
            dump(&entries())
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Steer while the tool is running.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let row = core
        .doc_host
        .queue_message(
            CHAT,
            "Thanks! When you answer, also include the word PINEAPPLE.",
            vec![],
        )
        .unwrap();
    assert!(core.doc_host.steer_queued_now(CHAT, &row).await.unwrap());
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        let all: String = entries()
            .iter()
            .filter(|e| e.role == MessageRole::Assistant)
            .map(text)
            .collect();
        if all.contains("PINEAPPLE") && status() == Some(SessionStatus::Idle) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "steer never answered\n{}",
            dump(&entries())
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let es = entries();
    let all: String = es
        .iter()
        .filter(|e| e.role == MessageRole::Assistant)
        .map(text)
        .collect();
    assert!(
        mcp_dir.join("finished").exists(),
        "tool did not finish\n{}",
        dump(&es)
    );
    assert!(
        all.to_uppercase().contains("MARMALADE"),
        "the running tool's result never reached the agent (tool aborted)\n{}",
        dump(&es)
    );
    println!(
        "PASS {name}: tool finished, result reached the agent, steer answered\n{}",
        dump(&es)
    );
    core.shutdown().await;
}
