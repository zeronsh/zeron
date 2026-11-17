//! Zeron MCP's command tools against a real engine: what an agent runs,
//! leaves in the background, reads and stops, with real processes.
#![cfg(unix)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zeron_engine::{EngineCore, HarnessId, default_registry};
use zeron_mcp::{Origin, Tools, Zeron};
use zeron_rpc::memory_client;

#[tokio::test]
async fn an_agent_runs_commands_as_its_own_shell_tool_would() {
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        dir.path(),
        Arc::new(default_registry()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let tools = Tools::new(Arc::new(Zeron::with_client(
        memory_client(core.rpc_service()),
        Origin::default(),
    )));
    let work = tempfile::tempdir().unwrap();
    std::fs::create_dir(work.path().join("app")).unwrap();
    let app = std::fs::canonicalize(work.path().join("app")).unwrap();

    // Output and exit code; cd carries over to the next command.
    let ran = tools
        .call(
            "run_command",
            json!({ "command": "cd app && echo hi && exit 3", "cwd": work.path() }),
        )
        .await
        .unwrap();
    assert_eq!(
        (ran["output"].as_str(), ran["exitCode"].as_i64()),
        (Some("hi\n"), Some(3)),
        "{ran}"
    );
    let ran = tools
        .call("run_command", json!({ "command": "pwd -P" }))
        .await
        .unwrap();
    assert_eq!(ran["output"].as_str().map(str::trim), app.to_str(), "{ran}");

    // A server left running: read as it goes, then stopped.
    let started = tools
        .call(
            "run_command",
            json!({ "command": "echo listening; while true; do sleep 0.05; done", "background": true }),
        )
        .await
        .unwrap();
    let id = started["id"].as_str().unwrap().to_owned();
    let mut printed = String::new();
    let give_up = Instant::now() + Duration::from_secs(10);
    while !printed.contains("listening") {
        assert!(Instant::now() < give_up, "{printed:?}");
        let read: Value = tools
            .call("command_output", json!({ "id": id }))
            .await
            .unwrap();
        printed.push_str(read["output"].as_str().unwrap_or_default());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let listed = tools.call("list_commands", json!({})).await.unwrap();
    assert!(
        listed["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == id.as_str() && c["status"] == "running")
    );
    let stopped = tools
        .call("stop_command", json!({ "id": id }))
        .await
        .unwrap();
    assert_eq!(stopped["status"], "stopped", "{stopped}");
    core.sessions.shutdown().await;
}
