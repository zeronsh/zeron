//! `tests/fixtures/fake-acp-tool.py`: a fake ACP agent whose turn starts the
//! MCP servers it was handed in `session/new` and calls one tool. Checked
//! here against a tiny stub MCP server, with Zeron's own server in `mcp` and
//! a plugin's in `extra_mcp`.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{AcpHarness, CancellationToken, Harness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, McpServer, RunRequest, SandboxLevel};

/// A stdio MCP server offering the one tool named by `$STUB_TOOL`. Its call
/// result echoes the arguments and the server's `$STUB_LABEL`; a call with
/// `{"fail": true}` is an `isError` result.
const STUB_MCP: &str = r#"#!/usr/bin/env python3
import json, os, sys
tool = os.environ["STUB_TOOL"]
for line in sys.stdin:
    msg = json.loads(line)
    ident, method = msg.get("id"), msg.get("method")
    if ident is None:
        continue
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "stub", "version": "1"}}
        # A server notification in the middle must be skipped.
        print(json.dumps({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}), flush=True)
    elif method == "tools/list":
        result = {"tools": [{"name": tool, "description": "stub", "inputSchema": {"type": "object"}}]}
    elif method == "tools/call":
        args = msg["params"].get("arguments", {})
        value = {"label": os.environ.get("STUB_LABEL"), "args": args}
        result = {"content": [{"type": "text", "text": json.dumps(value)}],
                  "structuredContent": value, "isError": bool(args.get("fail"))}
    else:
        print(json.dumps({"jsonrpc": "2.0", "id": ident, "error": {"code": -32601, "message": method}}), flush=True)
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": ident, "result": result}), flush=True)
"#;

fn fixture_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-acp-tool.py");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn stub(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("stub-mcp.py");
    std::fs::write(&path, STUB_MCP).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn server(name: &str, command: &Path, tool: &str) -> McpServer {
    McpServer {
        name: name.into(),
        command: command.display().to_string(),
        args: vec![],
        env: [
            ("STUB_TOOL".to_owned(), tool.to_owned()),
            ("STUB_LABEL".to_owned(), name.to_owned()),
        ]
        .into_iter()
        .collect(),
    }
}

fn request(prompt: &str, cwd: &Path, env: &[(&str, &str)]) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
        mcp: None,
        extra_mcp: Vec::new(),
        env: env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
    }
}

fn controls() -> (RunControls, mpsc::Sender<SteerMessage>, CancellationToken) {
    let (steer_tx, steer_rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |_questions| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(Vec::new());
            rx
        }),
        steering: steer_rx,
        interrupt: token.clone(),
        turn: Default::default(),
    };
    (controls, steer_tx, token)
}

/// The turn's reply text, once it completes.
async fn reply(req: RunRequest) -> String {
    let (controls, _steer, _token) = controls();
    let harness = AcpHarness::grok().with_executable(fixture_path());
    let mut stream = harness.run(req, controls).await.expect("run starts");
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = stream.next().await {
            match event.expect("stream event") {
                AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                AgentEvent::Done { status, error, .. } => {
                    assert_eq!(status, DoneStatus::Completed, "{error:?}");
                    return;
                }
                _ => {}
            }
        }
        panic!("stream ended before Done");
    })
    .await
    .expect("turn finished in time");
    text
}

#[tokio::test]
async fn calls_the_named_tool_on_the_server_that_lists_it() {
    let dir = tempfile::tempdir().unwrap();
    let stub = stub(dir.path());
    let mut req = request(
        "hello",
        dir.path(),
        &[
            ("FAKE_ACP_CALL_TOOL", "focus__count_words"),
            ("FAKE_ACP_CALL_ARGS", r#"{"text":"a b c"}"#),
        ],
    );
    // Zeron's own server does not list it; the plugin's does.
    req.mcp = Some(server("zeron", &stub, "whoami"));
    req.extra_mcp = vec![server("plugin-focus", &stub, "focus__count_words")];
    let text = reply(req).await;
    let result: serde_json::Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["label"], "plugin-focus");
    assert_eq!(
        result["structuredContent"]["args"],
        serde_json::json!({ "text": "a b c" })
    );
}

#[tokio::test]
async fn a_prompt_can_name_the_tool_and_a_pinned_server() {
    let dir = tempfile::tempdir().unwrap();
    let stub = stub(dir.path());
    let mut req = request(
        r#"call-tool whoami {"fail": true}"#,
        dir.path(),
        &[("FAKE_ACP_MCP_SERVER", "zeron")],
    );
    req.mcp = Some(server("zeron", &stub, "whoami"));
    let text = reply(req).await;
    let result: serde_json::Value = serde_json::from_str(&text).expect(&text);
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["label"], "zeron");

    let mut missing = request("call-tool nope", dir.path(), &[]);
    missing.mcp = Some(server("zeron", &stub, "whoami"));
    assert_eq!(
        reply(missing).await,
        "fake-acp-tool: no MCP server lists nope"
    );
}
