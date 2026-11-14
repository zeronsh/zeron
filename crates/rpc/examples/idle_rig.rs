//! Engine idle/streaming cost rig (not shipped): drives a headless engine over
//! its IPC the way the app does, so its CPU and wakeups can be measured.
//!
//! ```sh
//! idle_rig <port> <data dir> seed <n>                  # n mock chats with one finished turn each; prints ids
//! idle_rig <port> <data dir> client <secs> [chat]      # the app at rest: its watches (and one viewed chat)
//! idle_rig <port> <data dir> stream <secs> <chat>...   # a run in each chat, watched like the app
//! idle_rig <port> <data dir> latency <rounds> <chat>   # QueueCommand → user echo / first assistant text
//! idle_rig <a port> <a dir> sync <rounds> <chat> <b port> <b dir>  # A's run → B sees it
//! ```

use std::time::{Duration, Instant};

use zeron_rpc::{RpcClient, connect_ipc, methods};

fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1);
}

async fn connect(port: &str, dir: &str) -> RpcClient {
    let port: u16 = port.parse().unwrap_or_else(|_| fail("bad port"));
    connect_ipc(port, Some(std::path::Path::new(dir)))
        .await
        .unwrap_or_else(|err| fail(&format!("connect: {err}")))
}

async fn queue_run(client: &RpcClient, chat: &str, prompt: &str) {
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
}

/// Keep a stream drained in the background; counts its items.
fn drain(
    mut rx: tokio::sync::mpsc::Receiver<serde_json::Value>,
    count: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

/// The watches the zgx app holds at rest (apps/zgx/src/sessions.rs, chat.rs,
/// plugins, updates), plus a viewed chat's when one is given.
async fn app_watches(
    client: &RpcClient,
    chat: Option<&str>,
) -> std::sync::Arc<std::sync::atomic::AtomicU64> {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    for method in [
        "WatchChats",
        "WatchDevices",
        "WatchSessions",
        "WatchSpaces",
        "WatchSidebarPreferences",
        "WatchConnectivity",
        "WatchHeldSends",
        "WatchPlugins",
        "WatchHarnessUpdates",
        "WatchThemes",
    ] {
        match client.subscribe(method, serde_json::json!({})).await {
            Ok(rx) => drain(rx, count.clone()),
            Err(err) => eprintln!("{method}: {err}"),
        }
    }
    if let Some(chat) = chat {
        for (method, params) in [
            (
                "WatchDocMessages",
                serde_json::json!({ "chatId": chat, "openingTail": true }),
            ),
            ("WatchQueue", serde_json::json!({ "chatId": chat })),
            (
                "WatchWaitingMessages",
                serde_json::json!({ "chatId": chat }),
            ),
            ("WatchPreviews", serde_json::json!({ "chatId": chat })),
        ] {
            match client.subscribe(method, params).await {
                Ok(rx) => drain(rx, count.clone()),
                Err(err) => eprintln!("{method}: {err}"),
            }
        }
    }
    count
}

async fn create_space(client: &RpcClient) -> String {
    let device = client
        .call(methods::LOCAL_DEVICE, serde_json::json!({}))
        .await
        .unwrap_or_else(|err| fail(&format!("LocalDevice: {err}")));
    let space = uuid::Uuid::new_v4().to_string();
    let path = std::env::temp_dir().join(format!("idle-rig-{space}"));
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
    space
}

async fn create_chat(client: &RpcClient, space: &str) -> String {
    let chat = uuid::Uuid::new_v4().to_string();
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
            Ok(_) => return chat,
            Err(err) if attempts >= 20 => fail(&format!("createChat: {err}")),
            Err(_) => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

/// Wait until a doc-messages stream carries `needle`; the time it took.
async fn wait_text(
    rx: &mut tokio::sync::mpsc::Receiver<serde_json::Value>,
    needle: &str,
    start: Instant,
) -> Option<Duration> {
    let deadline = start + Duration::from_secs(30);
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let item = tokio::time::timeout(left, rx.recv()).await.ok()??;
        if item.to_string().contains(needle) {
            return Some(start.elapsed());
        }
    }
}

fn summary(label: &str, mut samples: Vec<Duration>) {
    if samples.is_empty() {
        println!("{label}: no samples");
        return;
    }
    samples.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = samples.iter().map(|d| ms(*d)).sum::<f64>() / samples.len() as f64;
    println!(
        "{label}: n={} min={:.1}ms p50={:.1}ms mean={:.1}ms p90={:.1}ms max={:.1}ms",
        samples.len(),
        ms(samples[0]),
        ms(samples[samples.len() / 2]),
        mean,
        ms(samples[(samples.len() * 9 / 10).min(samples.len() - 1)]),
        ms(*samples.last().unwrap()),
    );
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [port, dir, command, rest @ ..] = args.as_slice() else {
        fail("usage: idle_rig <port> <data dir> <seed|client|stream|latency|sync> ...");
    };
    let client = connect(port, dir).await;
    match (command.as_str(), rest) {
        ("seed", [n]) => {
            let n: usize = n.parse().unwrap_or_else(|_| fail("bad n"));
            let space = create_space(&client).await;
            let mut chats = Vec::new();
            for _ in 0..n {
                chats.push(create_chat(&client, &space).await);
            }
            // One finished turn each, so every chat has a transcript.
            for (i, chat) in chats.iter().enumerate() {
                let mut rx = client
                    .subscribe(
                        methods::WATCH_DOC_MESSAGES,
                        serde_json::json!({ "chatId": chat }),
                    )
                    .await
                    .unwrap_or_else(|err| fail(&format!("watch: {err}")));
                queue_run(&client, chat, &format!("seed turn {i}")).await;
                // The mock's scripted reply ends with a complete status.
                let _ = wait_text(&mut rx, "\"complete\"", Instant::now()).await;
                println!("{chat}");
            }
        }
        ("create", [n]) => {
            let n: usize = n.parse().unwrap_or_else(|_| fail("bad n"));
            let space = create_space(&client).await;
            let mut chats = Vec::new();
            for _ in 0..n {
                chats.push(create_chat(&client, &space).await);
            }
            println!("{}", chats.join(" "));
        }
        ("device", []) => {
            let device = client
                .call(methods::LOCAL_DEVICE, serde_json::json!({}))
                .await
                .unwrap_or_else(|err| fail(&format!("LocalDevice: {err}")));
            println!("{}", device["deviceId"].as_str().unwrap_or_default());
        }
        // Forwarded calls to another device: each one's time (ms), one a line.
        ("call", [target, n, gap_ms]) => {
            let n: usize = n.parse().unwrap_or_else(|_| fail("bad n"));
            let gap: u64 = gap_ms.parse().unwrap_or_else(|_| fail("bad gap"));
            for _ in 0..n {
                let start = Instant::now();
                let result = client
                    .call(
                        methods::LIST_HARNESSES,
                        serde_json::json!({ "targetDeviceId": target }),
                    )
                    .await;
                match result {
                    Ok(_) => println!("{:.1}", start.elapsed().as_secs_f64() * 1000.0),
                    Err(err) => println!("error {err}"),
                }
                tokio::time::sleep(Duration::from_millis(gap)).await;
            }
        }
        // Until a forwarded call to `target` succeeds: how long that took (ms).
        ("reachable", [target, timeout_s]) => {
            let timeout: u64 = timeout_s.parse().unwrap_or_else(|_| fail("bad timeout"));
            let start = Instant::now();
            loop {
                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    client.call(
                        methods::LIST_HARNESSES,
                        serde_json::json!({ "targetDeviceId": target }),
                    ),
                )
                .await;
                if matches!(result, Ok(Ok(_))) {
                    println!("{:.0}", start.elapsed().as_secs_f64() * 1000.0);
                    break;
                }
                if start.elapsed() > Duration::from_secs(timeout) {
                    println!("never");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        ("client", [secs, chat @ ..]) => {
            let secs: u64 = secs.parse().unwrap_or_else(|_| fail("bad secs"));
            let count = app_watches(&client, chat.first().map(String::as_str)).await;
            // The app follows each other device's agent updates, as zgx's
            // updates view does (`IDLE_RIG_PEERS`: their ids).
            for peer in std::env::var("IDLE_RIG_PEERS")
                .unwrap_or_default()
                .split_whitespace()
            {
                match client
                    .subscribe(
                        methods::WATCH_HARNESS_UPDATES,
                        serde_json::json!({ "targetDeviceId": peer }),
                    )
                    .await
                {
                    Ok(rx) => drain(rx, count.clone()),
                    Err(err) => eprintln!("WatchHarnessUpdates {peer}: {err}"),
                }
            }
            tokio::time::sleep(Duration::from_secs(secs)).await;
            println!(
                "frames={}",
                count.load(std::sync::atomic::Ordering::Relaxed)
            );
        }
        ("stream", [secs, chats @ ..]) => {
            let secs: u64 = secs.parse().unwrap_or_else(|_| fail("bad secs"));
            let count = app_watches(&client, chats.first().map(String::as_str)).await;
            for chat in chats {
                queue_run(&client, chat, "stream").await;
            }
            tokio::time::sleep(Duration::from_secs(secs)).await;
            println!(
                "frames={}",
                count.load(std::sync::atomic::Ordering::Relaxed)
            );
        }
        ("latency", [rounds, chat]) => {
            let rounds: usize = rounds.parse().unwrap_or_else(|_| fail("bad rounds"));
            let _watches = app_watches(&client, None).await;
            let mut echo = Vec::new();
            let mut first = Vec::new();
            for round in 0..rounds {
                let mut rx = client
                    .subscribe(
                        methods::WATCH_DOC_MESSAGES,
                        serde_json::json!({ "chatId": chat }),
                    )
                    .await
                    .unwrap_or_else(|err| fail(&format!("watch: {err}")));
                // Let the opening frame pass.
                let _ = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                while rx.try_recv().is_ok() {}
                let marker = format!("latency-{}-{round}", uuid::Uuid::new_v4().simple());
                let start = Instant::now();
                queue_run(&client, chat, &marker).await;
                match wait_text(&mut rx, &marker, start).await {
                    Some(d) => echo.push(d),
                    None => eprintln!("round {round}: no echo"),
                }
                // The mock's first text delta (crates/engine/src/registry.rs).
                match wait_text(&mut rx, "Streaming pipeline", start).await {
                    Some(d) => first.push(d),
                    None => eprintln!("round {round}: no assistant text"),
                }
                // Let the run end before the next round (the mock's is instant).
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
            summary("user echo", echo);
            summary("first assistant", first);
        }
        ("sync", [rounds, chat, b_port, b_dir]) => {
            let rounds: usize = rounds.parse().unwrap_or_else(|_| fail("bad rounds"));
            let b = connect(b_port, b_dir).await;
            let mut a_rx = client
                .subscribe(
                    methods::WATCH_DOC_MESSAGES,
                    serde_json::json!({ "chatId": chat }),
                )
                .await
                .unwrap_or_else(|err| fail(&format!("watch A: {err}")));
            let mut b_rx = b
                .subscribe(
                    methods::WATCH_DOC_MESSAGES,
                    serde_json::json!({ "chatId": chat }),
                )
                .await
                .unwrap_or_else(|err| fail(&format!("watch B: {err}")));
            tokio::time::sleep(Duration::from_secs(2)).await;
            let mut samples = Vec::new();
            let mut from_queue = Vec::new();
            for round in 0..rounds {
                while a_rx.try_recv().is_ok() {}
                while b_rx.try_recv().is_ok() {}
                let marker = format!("sync-{}-{round}", uuid::Uuid::new_v4().simple());
                let start = Instant::now();
                queue_run(&client, chat, &marker).await;
                let Some(on_a) = wait_text(&mut a_rx, &marker, start).await else {
                    eprintln!("round {round}: A never showed it");
                    continue;
                };
                match wait_text(&mut b_rx, &marker, start).await {
                    Some(on_b) => {
                        samples.push(on_b.saturating_sub(on_a));
                        from_queue.push(on_b);
                    }
                    None => eprintln!("round {round}: B never showed it"),
                }
                tokio::time::sleep(Duration::from_millis(1500)).await;
            }
            summary("queued here → shown here, there: there minus here", samples);
            summary("queued here → shown there", from_queue);
        }
        _ => fail("unknown command"),
    }
}
