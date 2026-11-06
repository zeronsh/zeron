//! Drive a headless engine by hand while testing a phone against it
//! (docs/design/relay-mobile.md). Each command prints one line.
//!
//! ```sh
//! relay_rig <ipc port> <data dir> create              # a mock-harness chat: prints its id
//! relay_rig <ipc port> <data dir> run <chat> <prompt> # queue a turn from the host's side
//! relay_rig <ipc port> <data dir> gen <chat>          # the chat's room generation
//! relay_rig <ipc port> <data dir> replies <chat>      # finished assistant entries
//! relay_rig <ipc port> <data dir> has <chat> <text>   # does the doc hold this text?
//! ```

use std::time::{Duration, Instant};

use zeron_rpc::{RpcClient, connect_ipc, methods};

fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1);
}

/// The first value `pick` accepts from a watch stream, within 20 s. `None`: it never did.
async fn watch<T>(
    client: &RpcClient,
    method: &str,
    params: serde_json::Value,
    mut pick: impl FnMut(&serde_json::Value) -> Option<T>,
) -> Option<T> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut stream = client.subscribe_checked(method, params).await.ok()?;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let item = tokio::time::timeout(left, stream.recv()).await.ok()??;
        if let Some(found) = pick(&item) {
            return Some(found);
        }
    }
}

async fn transcript(client: &RpcClient, chat: &str) -> Vec<zeron_doc::SessionMessageEntry> {
    let mut entries = Vec::new();
    let params = serde_json::json!({ "chatId": chat });
    watch(client, methods::WATCH_DOC_MESSAGES, params, |item| {
        let frame: zeron_doc::TranscriptFrame = serde_json::from_value(item.clone()).ok()?;
        zeron_doc::apply_transcript_frame(&mut entries, frame).ok()?;
        Some(())
    })
    .await;
    entries
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [port, dir, command, rest @ ..] = args.as_slice() else {
        fail("usage: relay_rig <ipc port> <data dir> <create|run|gen|replies|has> ...");
    };
    let port: u16 = port.parse().unwrap_or_else(|_| fail("bad port"));
    let client = connect_ipc(port, Some(std::path::Path::new(dir)))
        .await
        .unwrap_or_else(|err| fail(&format!("connect: {err}")));
    match (command.as_str(), rest) {
        ("create", []) => {
            let device = client
                .call(methods::LOCAL_DEVICE, serde_json::json!({}))
                .await
                .unwrap_or_else(|err| fail(&format!("LocalDevice: {err}")));
            let space = uuid::Uuid::new_v4().to_string();
            // A folder of its own: a second space at the same path is the first one.
            let path = std::env::temp_dir().join(format!("relay-rig-{space}"));
            std::fs::create_dir_all(&path).unwrap_or_else(|err| fail(&format!("mkdir: {err}")));
            client
                .call(
                    methods::MUTATE,
                    serde_json::json!({
                        "op": "createSpace",
                        "spaceId": space,
                        "deviceId": device["deviceId"],
                        "path": path,
                    }),
                )
                .await
                .unwrap_or_else(|err| fail(&format!("createSpace: {err}")));
            let chat = uuid::Uuid::new_v4().to_string();
            // A space just created can take a moment to read back.
            let mut attempts = 0;
            loop {
                let created = client
                    .call(
                        methods::MUTATE,
                        serde_json::json!({
                            "op": "createChat",
                            "chatId": chat,
                            "spaceId": space,
                            "config": { "harness": "mock", "model": null, "reasoning": null, "sandbox": "workspace-write" },
                        }),
                    )
                    .await;
                match created {
                    Ok(_) => break,
                    Err(err) if attempts >= 20 => fail(&format!("createChat: {err}")),
                    Err(_) => {
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis(250)).await;
                    }
                }
            }
            println!("{chat}");
        }
        ("run", [chat, prompt]) => {
            client
                .call(
                    methods::QUEUE_COMMAND,
                    serde_json::json!({
                        "chatId": chat,
                        "command": {
                            "kind": "run",
                            "messageId": uuid::Uuid::new_v4().to_string(),
                            "request": {
                                "prompt": prompt,
                                "model": null,
                                "reasoning": null,
                                "cwd": "/tmp",
                                "sandbox": "workspace-write",
                                "autoApprove": true,
                                "resume": null,
                            },
                        },
                    }),
                )
                .await
                .unwrap_or_else(|err| fail(&format!("QueueCommand: {err}")));
            println!("queued");
        }
        ("gen", [chat]) => {
            let generation = watch(&client, methods::WATCH_CHATS, serde_json::json!({}), |item| {
                item.as_array()?
                    .iter()
                    .find(|row| row.get("id").and_then(|v| v.as_str()) == Some(chat.as_str()))
                    .map(|row| row.get("roomGen").and_then(|v| v.as_u64()).unwrap_or(0))
            })
            .await;
            println!("{}", generation.unwrap_or_else(|| fail("no such chat")));
        }
        ("replies", [chat]) => {
            let entries = transcript(&client, chat).await;
            let done = entries
                .iter()
                .filter(|entry| {
                    entry.role == zeron_doc::MessageRole::Assistant
                        && entry.status == Some(zeron_doc::MessageStatus::Complete)
                })
                .count();
            println!("{done}");
        }
        ("has", [chat, text]) => {
            let entries = transcript(&client, chat).await;
            let found = entries
                .iter()
                .any(|entry| serde_json::to_string(entry).is_ok_and(|json| json.contains(text.as_str())));
            println!("{found}");
        }
        _ => fail("unknown command"),
    }
}
