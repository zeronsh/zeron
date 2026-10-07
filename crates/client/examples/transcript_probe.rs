//! Measure engine-side session-open cost over the local WebSocket the
//! phone's SSH tunnel ends on (default ws://127.0.0.1:27654).
//!
//! ```sh
//! cargo run -p zeron-client --example transcript_probe -- [PORT] [CHAT_ID_PREFIX]
//! ```
//!
//! No chat argument probes the chats the user has not opened for the
//! longest time (oldest `lastSeenAt`), three of them plus the freshest for
//! contrast. For each it times: subscribe -> tail (first `historyPending`
//! reset), tail -> complete reset, rows and wire bytes.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;
use zeron_rpc::methods;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Chat {
    id: String,
    title: Option<String>,
    last_message_at: Option<String>,
    last_seen_at: Option<String>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port: u16 = args
        .iter()
        .find(|a| a.parse::<u16>().is_ok())
        .map(|a| a.parse().unwrap())
        .unwrap_or(27654);
    let prefix = args.iter().find(|a| a.parse::<u16>().is_err()).cloned();
    let rt = zeron_client::runtime::shared();
    rt.block_on(async move {
        let rpc = zeron_rpc::connect_ws(&format!("ws://127.0.0.1:{port}/"))
            .await
            .expect("connect");
        let mut chats_sub = rpc
            .subscribe_scoped(methods::WATCH_CHATS, json!({}))
            .await
            .expect("watch chats");
        let first = chats_sub
            .recv()
            .await
            .expect("chats frame");
        drop(chats_sub);
        // The frame is the bare chat array (older engines may wrap it).
        let list_value = first
            .get("chats")
            .cloned()
            .unwrap_or(first);
        let mut chats: Vec<Chat> = serde_json::from_value(list_value).expect("chat list");
        chats.sort_by(|a, b| {
            a.last_seen_at
                .as_deref()
                .unwrap_or("")
                .cmp(b.last_seen_at.as_deref().unwrap_or(""))
        });
        println!("{} chats; stalest first:", chats.len());
        for (i, c) in chats.iter().take(8).enumerate() {
            println!(
                "  {i}: {} seen={} msg={} {}",
                &c.id[..8],
                c.last_seen_at.as_deref().unwrap_or("-"),
                c.last_message_at.as_deref().unwrap_or("-"),
                c.title.as_deref().unwrap_or("?").chars().take(40).collect::<String>(),
            );
        }
        let targets: Vec<&Chat> = if let Some(prefix) = prefix {
            chats.iter().filter(|c| c.id.starts_with(&prefix)).collect()
        } else {
            let mut t: Vec<&Chat> = chats.iter().filter(|c| c.last_message_at.is_some()).take(3).collect();
            if let Some(fresh) = chats.iter().filter(|c| c.last_message_at.is_some()).last() {
                t.push(fresh);
            }
            t
        };
        for chat in targets {
            probe(&rpc, chat).await;
        }
    });
}

async fn probe(rpc: &zeron_rpc::RpcClient, chat: &Chat) {
    let title = chat.title.as_deref().unwrap_or("?");
    println!(
        "\n== {} ({}…) seen={} msg={}",
        title.chars().take(40).collect::<String>(),
        &chat.id[..8],
        chat.last_seen_at.as_deref().unwrap_or("-"),
        chat.last_message_at.as_deref().unwrap_or("-"),
    );
    let began = Instant::now();
    let mut sub = match rpc
        .subscribe_scoped(
            methods::WATCH_DOC_MESSAGES,
            json!({ "chatId": chat.id, "openingTail": true }),
        )
        .await
    {
        Ok(sub) => sub,
        Err(err) => {
            println!("  subscribe failed: {err}");
            return;
        }
    };
    let mut bytes = 0usize;
    let mut tail_rows = 0usize;
    let mut tail_done = false;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let item = tokio::time::timeout_at(deadline.into(), sub.recv()).await;
        let Ok(item) = item else {
            println!("  timeout at {:.1}s", began.elapsed().as_secs_f64());
            return;
        };
        let Some(value) = item else {
            println!("  stream ended at {:.1}s", began.elapsed().as_secs_f64());
            return;
        };
        bytes += value.to_string().len();
        let pending = value
            .get("historyPending")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let (kind, rows) = if let Some(reset) = value.get("reset") {
            ("reset", reset.as_array().map_or(0, |a| a.len()))
        } else {
            (
                "delta",
                value
                    .get("count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
            )
        };
        let t = began.elapsed().as_secs_f64();
        if !tail_done {
            println!("  {t:6.2}s  tail {kind} rows={rows}");
            tail_rows = rows;
            tail_done = true;
            if !pending {
                println!("  (no opening tail — engine sent the full reset first)");
                println!("  == full history in {t:.2}s, {rows} rows, ~{bytes} bytes");
                return;
            }
            continue;
        }
        if !pending {
            println!("  {t:6.2}s  full {kind} rows={rows}");
            println!(
                "  == tail {} rows; full {rows} rows in {t:.2}s, ~{bytes} bytes",
                tail_rows
            );
            return;
        }
        println!("  {t:6.2}s  {kind} rows={rows} (still pending)");
    }
}
