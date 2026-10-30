//! Scriptable stdio MCP peer. All controls are inherited environment variables.
use serde_json::{Value, json};
use std::{collections::HashMap, fs::OpenOptions, io::Write, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn append(path: &str, value: impl std::fmt::Display) {
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{value}"
    )
    .unwrap();
}
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}
fn number(name: &str) -> u64 {
    env(name).parse().unwrap_or(0)
}
fn reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if std::env::args().any(|a| a.contains("telemetry/watchdog/main.js")) {
        // Deliberately survives parent exit: the proxy must find this separate group.
        tokio::time::sleep(Duration::from_secs(3600)).await;
        return;
    }
    if !env("FAKE_SPAWN_MARK").is_empty() {
        append(&env("FAKE_SPAWN_MARK"), std::process::id());
    }
    if !env("FAKE_DUMP_ENV").is_empty() {
        std::fs::write(
            env("FAKE_DUMP_ENV"),
            serde_json::to_vec(&std::env::vars().collect::<HashMap<_, _>>()).unwrap(),
        )
        .unwrap();
    }
    let mut descendants = Vec::new();
    #[cfg(unix)]
    {
        if env("FAKE_GRANDCHILD") == "1" {
            let child = std::process::Command::new("sleep")
                .arg("3600")
                .spawn()
                .unwrap();
            if !env("FAKE_CHILD_MARK").is_empty() {
                append(&env("FAKE_CHILD_MARK"), child.id());
            }
            descendants.push(child);
        }
        if env("FAKE_WATCHDOG") == "1" {
            use std::os::unix::process::CommandExt;
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("telemetry/watchdog/main.js")
                .arg(format!("--parent-pid={}", std::process::id()))
                .process_group(0)
                .spawn()
                .unwrap();
            if !env("FAKE_CHILD_MARK").is_empty() {
                append(&env("FAKE_CHILD_MARK"), child.id());
            }
            descendants.push(child);
        }
    }
    eprintln!(
        "fake MCP stderr {}",
        "x".repeat(number("FAKE_STDERR_BYTES") as usize)
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Value>(64);
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(v) = rx.recv().await {
            if stdout.write_all(format!("{v}\n").as_bytes()).await.is_err() {
                break;
            }
            if stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let mut sampling = HashMap::new();
    let mut tools: Value = serde_json::from_str(&env("FAKE_TOOLS"))
        .unwrap_or_else(|_| serde_json::from_str(include_str!("../fixtures/tools.json")).unwrap());
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if !env("FAKE_REQUEST_LOG").is_empty() {
            append(&env("FAKE_REQUEST_LOG"), &v);
        }
        let id = v["id"].clone();
        let method = v["method"].as_str().unwrap_or("");
        let params = &v["params"];
        let result = match method {
            "initialize" => {
                tokio::time::sleep(Duration::from_millis(number("FAKE_INIT_DELAY_MS"))).await;
                if env("FAKE_INIT_ERROR") == "1" {
                    tx.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32000,"message":"fake init failure"}})).await.unwrap();
                    continue;
                }
                json!({"protocolVersion":params["protocolVersion"],"serverInfo":{"name":"fake","version":"1"},"instructions":env("FAKE_INSTRUCTIONS"),"capabilities":{"tools":{"listChanged":true},"prompts":{},"resources":{"subscribe":true}}})
            }
            "tools/list" => {
                let mut result = json!({"tools":tools});
                if env("FAKE_PAGED") == "1" && tools.as_array().unwrap().len() > 1 {
                    if params["cursor"] == "page2" {
                        result["tools"] = json!(&tools.as_array().unwrap()[1..]);
                    } else {
                        result["tools"] = json!([tools[0]]);
                        result["nextCursor"] = json!("page2");
                    }
                }
                if let Ok(ttl) = env("FAKE_TTL_MS").parse::<u64>() {
                    result["ttlMs"] = json!(ttl);
                }
                if !env("FAKE_CACHE_SCOPE").is_empty() {
                    result["cacheScope"] = json!(env("FAKE_CACHE_SCOPE"));
                }
                result
            }
            "prompts/list" => json!({"prompts":[{"name":"hello"}]}),
            "resources/list" => json!({"resources":[{"name":"test","uri":"fake://test"}]}),
            "resources/templates/list" => json!({"resourceTemplates":[]}),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("");
                if name == env("FAKE_CRASH_ON") {
                    std::process::exit(42);
                }
                for progress in 0..number("FAKE_PROGRESS") {
                    tx.send(json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":params["_meta"]["progressToken"],"progress":progress}})).await.unwrap();
                }
                if name == env("FAKE_LIST_CHANGED_AFTER") {
                    tools = serde_json::from_str(&env("FAKE_CHANGED_TOOLS")).unwrap_or(json!([]));
                    tx.send(json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"}))
                        .await
                        .unwrap();
                }
                if name == env("FAKE_SAMPLING_ON") {
                    let server_id = format!("sample:{}", sampling.len());
                    sampling.insert(server_id.clone(), id);
                    tx.send(json!({"jsonrpc":"2.0","id":server_id,"method":"sampling/createMessage","params":{"messages":[],"maxTokens":10}})).await.unwrap();
                    continue;
                }
                let result = json!({"content":[{"type":"text","text":name}],"isError":false});
                let tx = tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(number("FAKE_CALL_DELAY_MS"))).await;
                    let _ = tx.send(reply(id, result)).await;
                });
                continue;
            }
            "" => {
                if let Some(original) = sampling.remove(id.as_str().unwrap_or("")) {
                    tx.send(reply(original, json!({"content":[{"type":"text","text":v["result"].to_string()}],"isError":false}))).await.unwrap();
                }
                continue;
            }
            m if m.starts_with("notifications/") => continue,
            _ => json!({}),
        };
        if v.get("id").is_some() {
            tx.send(reply(id, result)).await.unwrap();
        }
    }
    for mut child in descendants {
        let _ = child.kill();
        let _ = child.wait();
    }
    drop(tx);
    let _ = writer.await;
}
