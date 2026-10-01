//! Seeds a running mock-harness engine with chats whose titles don't contain
//! the words in their transcripts, for driving the Cmd+K palette by hand.
//! Usage: transcript_search_seed <ipc port>

use std::time::Duration;

use zeron_rpc::{connect_ws, methods};

const CHATS: &[(&str, &str, &str)] = &[
    (
        "seed-deploy",
        "Friday cleanup",
        "the helm rollout keeps failing because the configmap is stale",
    ),
    (
        "seed-lunch",
        "Team offsite",
        "can you find a vegetarian ramen place near the office",
    ),
    ("seed-ramen", "Ramen notes", "summarize the offsite agenda"),
];

#[tokio::main]
async fn main() {
    let port = std::env::args().nth(1).unwrap_or_else(|| "27870".into());
    let client = connect_ws(&format!("ws://127.0.0.1:{port}"))
        .await
        .expect("connect engine");
    let device = client
        .call(methods::LOCAL_DEVICE, serde_json::json!({}))
        .await
        .expect("LocalDevice")["deviceId"]
        .as_str()
        .expect("deviceId")
        .to_string();
    for (chat, title, prompt) in CHATS {
        client
            .call(
                methods::MUTATE,
                serde_json::json!({ "op": "createChat", "chatId": chat, "deviceId": device }),
            )
            .await
            .expect("createChat");
        client
            .call(
                methods::MUTATE,
                serde_json::json!({ "op": "renameChat", "chatId": chat, "title": title }),
            )
            .await
            .expect("renameChat");
        client
            .call(
                methods::QUEUE_COMMAND,
                serde_json::json!({
                    "chatId": chat,
                    "command": {
                        "kind": "run",
                        "messageId": format!("{chat}-m1"),
                        "request": {
                            "prompt": prompt, "cwd": "/tmp", "sandbox": "workspace-write",
                            "autoApprove": true, "harness": "mock",
                        },
                    },
                }),
            )
            .await
            .expect("QueueCommand");
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    for query in ["configmap", "ramen", "vegetarian ram", "harness reporting"] {
        let hits = client
            .call(
                methods::SEARCH_TRANSCRIPTS,
                serde_json::json!({ "query": query, "limit": 30 }),
            )
            .await
            .expect("SearchTranscripts");
        println!("{query}: {hits}");
    }
}
