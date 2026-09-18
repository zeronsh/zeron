//! live probe: the installed antigravity acp server's raw model catalog and,
//! with `--prompt`, the session updates and usage one short turn produces.
//! run from a directory outside any zeron project so preview discovery never
//! probes the server's ports.
use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() {
    let server = std::env::var_os("ANTIGRAVITY_ACP_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            zeron_harness::AcpHarness::antigravity()
                .launch_program()
                .expect("installed antigravity acp server")
        });
    let workspace = std::env::temp_dir().join("antigravity-acp-probe");
    std::fs::create_dir_all(&workspace).unwrap();
    let mut command = tokio::process::Command::new(&server);
    #[cfg(target_os = "linux")]
    command.arg("--uid=");
    let mut child = command
        .current_dir(&workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn server");
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut next_id = 0;

    let mut call = async |method: &str, params: Value| -> Value {
        next_id += 1;
        let id = next_id;
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        stdin
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        while let Some(line) = lines.next_line().await.unwrap() {
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
            if let Some(update) = message.pointer("/params/update") {
                let kind = update
                    .get("sessionUpdate")
                    .and_then(Value::as_str)
                    .unwrap_or("?");
                match kind {
                    "agent_message_chunk" | "agent_thought_chunk" => println!("update {kind}"),
                    _ => println!("update {kind}: {update}"),
                }
            } else if message.get("method").is_some() {
                println!("server request/notification: {message}");
            }
        }
        panic!("server closed stdout");
    };

    let init = call("initialize", json!({
        "protocolVersion": 1,
        "clientInfo": { "name": "zeron-probe", "version": "0" },
        "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
    }))
    .await;
    println!(
        "initialize agentCapabilities: {}",
        init.pointer("/result/agentCapabilities")
            .unwrap_or(&Value::Null)
    );

    let session = call("session/new", json!({ "cwd": workspace, "mcpServers": [] })).await;
    if let Some(error) = session.get("error") {
        println!("session/new error: {error}");
        return;
    }
    let result = &session["result"];
    println!(
        "\nconfigOptions:\n{}",
        serde_json::to_string_pretty(&result["configOptions"]).unwrap()
    );
    println!(
        "\nlegacy models:\n{}",
        serde_json::to_string_pretty(&result["models"]).unwrap()
    );

    if std::env::args().any(|arg| arg == "--prompt") {
        let session_id = result["sessionId"].as_str().unwrap().to_owned();
        let response = call(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "Reply with just the word ok." }],
            }),
        )
        .await;
        println!("\nprompt response: {response}");
    }
}
