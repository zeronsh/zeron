//! Two-device e2e smoke driver (`scripts/e2e-smoke.sh` runs it).
//!
//! Connects to two running headless engines over their localhost IPC ports and proves
//! the cross-device command plane end to end against a real edge:
//!
//! 1. `LocalDevice` on both — distinct device ids;
//! 2. `Mutate createChat` on A (A hosts the chat, mock harness config);
//! 3. waits until the chat row syncs A → edge → B (`WatchChats` on B);
//! 4. `QueueCommand` a Run on **B** — the doc path: B commits the command into its
//!    replica, nudges A's device room, A's host executor drains and runs it;
//! 5. waits on **B** until the assistant entry lands `complete` with the mock
//!    transcript (A → edge → B), and the session-status row round-trips.
//!
//! Prints `PASS`/`FAIL` lines; exits nonzero on failure.

use std::time::{Duration, Instant};

use zeron_rpc::{RpcClient, connect_ipc, methods};

const STEP_TIMEOUT: Duration = Duration::from_secs(90);
const MOCK_TEXT: &str = "Mock harness reporting in.";

fn fail(message: &str) -> ! {
    eprintln!("FAIL: {message}");
    std::process::exit(1);
}

fn pass(message: &str) {
    println!("PASS: {message}");
}

async fn device_id(client: &RpcClient, label: &str) -> String {
    match client
        .call(methods::LOCAL_DEVICE, serde_json::json!({}))
        .await
    {
        Ok(value) => value
            .get("deviceId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| fail(&format!("{label}: LocalDevice reply missing deviceId"))),
        Err(err) => fail(&format!("{label}: LocalDevice call failed: {err}")),
    }
}

/// Subscribe to a watch-stream and poll items until `predicate` returns `Some`.
///
/// Stream end is NOT a failure: the engine ends watch streams deliberately at
/// lifecycle boundaries (the chat2 cutover drops the s2 handle so watchers
/// converge onto the new room), and the client contract — the UI does exactly
/// this — is to resubscribe. The step deadline still bounds the whole wait.
async fn wait_stream<T>(
    client: &RpcClient,
    method: &str,
    params: serde_json::Value,
    what: &str,
    mut predicate: impl FnMut(&serde_json::Value) -> Option<T>,
) -> T {
    let deadline = Instant::now() + STEP_TIMEOUT;
    'resubscribe: loop {
        if deadline.saturating_duration_since(Instant::now()).is_zero() {
            fail(&format!(
                "{what}: timed out after {}s",
                STEP_TIMEOUT.as_secs()
            ));
        }
        let mut rx = match client.subscribe(method, params.clone()).await {
            Ok(rx) => rx,
            Err(err) => fail(&format!("{what}: subscribe {method} failed: {err}")),
        };
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                fail(&format!(
                    "{what}: timed out after {}s",
                    STEP_TIMEOUT.as_secs()
                ));
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(item)) => {
                    if let Some(found) = predicate(&item) {
                        return found;
                    }
                }
                Ok(None) => {
                    // Lifecycle boundary (e.g. chat2 cutover): resubscribe.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    continue 'resubscribe;
                }
                Err(_) => fail(&format!(
                    "{what}: timed out after {}s",
                    STEP_TIMEOUT.as_secs()
                )),
            }
        }
    }
}

fn apply_transcript_item(
    current: &mut Vec<zeron_doc::SessionMessageEntry>,
    item: &serde_json::Value,
) -> Result<(), String> {
    let frame: zeron_doc::TranscriptFrame =
        serde_json::from_value(item.clone()).map_err(|err| err.to_string())?;
    zeron_doc::apply_transcript_frame(current, frame).map_err(|err| err.to_string())
}

/// The chat's row on `client` reaching `generation`.
async fn wait_for_generation(client: &RpcClient, chat_id: &str, generation: u64, label: &str) {
    wait_stream(client, methods::WATCH_CHATS, serde_json::json!({}), label, |item| {
        item.as_array()?.iter().find(|chat| {
            chat.get("id").and_then(|v| v.as_str()) == Some(chat_id)
                && chat.get("roomGen").and_then(|v| v.as_u64()) == Some(generation)
        })?;
        Some(())
    })
    .await;
}

async fn queue_run(client: &RpcClient, chat_id: &str, prompt: &str) {
    client
        .call(
            methods::QUEUE_COMMAND,
            serde_json::json!({
                "chatId": chat_id,
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

/// `client`'s transcript reaching `count` finished assistant entries.
async fn wait_for_replies(client: &RpcClient, chat_id: &str, count: usize, label: &str) {
    let mut transcript = Vec::new();
    wait_stream(
        client,
        methods::WATCH_DOC_MESSAGES,
        serde_json::json!({ "chatId": chat_id }),
        label,
        |item| {
            apply_transcript_item(&mut transcript, item)
                .unwrap_or_else(|err| fail(&format!("{label}: {err}")));
            let done = transcript
                .iter()
                .filter(|entry| {
                    entry.role == zeron_doc::MessageRole::Assistant
                        && entry.status == Some(zeron_doc::MessageStatus::Complete)
                })
                .count();
            (done >= count).then_some(())
        },
    )
    .await;
}

fn room_stats(chat_id: &str) -> serde_json::Value {
    let edge = std::env::var("ZERON_E2E_EDGE_URL").unwrap_or_else(|_| fail("ZERON_E2E_EDGE_URL unset"));
    let token = std::env::var("ZERON_E2E_TOKEN").unwrap_or_else(|_| fail("ZERON_E2E_TOKEN unset"));
    let output = std::process::Command::new("curl")
        .args(["-sf", "-m", "5", "-H", &format!("authorization: Bearer {token}")])
        .arg(format!("{edge}/chat2/{chat_id}/stats"))
        .output()
        .unwrap_or_else(|err| fail(&format!("curl: {err}")));
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| fail(&format!("stats: {err}")))
}

/// Relay layer on in both engines (docs/design/relay-layer.md): the chat moves to
/// generation 3, a turn stores nothing, and, with `ZERON_E2E_RESTART_B` set, device B
/// coming back on a build without the layer gets everything it missed: the chat stays
/// on 3 and the host stores for B only while it's here.
async fn relay_layer(
    a: &RpcClient,
    b: &RpcClient,
    chat_id: &str,
    b_port: u16,
    b_dir: Option<&std::path::Path>,
) {
    wait_for_generation(b, chat_id, 3, "chat on generation 3").await;
    pass("host moved the chat to generation 3; B sees it");
    // Long enough for both engines' next refresh to act on it.
    tokio::time::sleep(Duration::from_secs(12)).await;

    let before = room_stats(chat_id);
    queue_run(b, chat_id, "a turn with nothing stored").await;
    wait_for_replies(b, chat_id, 2, "second reply on B").await;
    pass("a turn on generation 3 reached B");
    let deadline = Instant::now() + STEP_TIMEOUT;
    let after = loop {
        let stats = room_stats(chat_id);
        if stats["relayQueue"] == 0 {
            break stats;
        }
        if Instant::now() > deadline {
            fail(&format!("the queue never emptied: {stats}"));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    if after["headSeq"] != before["headSeq"] || after["checkpointSeq"] != before["checkpointSeq"] {
        fail(&format!("a generation 3 turn stored rows: before {before}, after {after}"));
    }
    pass("the turn stored no rows, and its queued command left the room");

    let Ok(restart) = std::env::var("ZERON_E2E_RESTART_B") else {
        return;
    };
    let stop = std::process::Command::new("bash").arg(&restart).arg("stop").status();
    if !stop.is_ok_and(|status| status.success()) {
        fail("couldn't stop B");
    }
    queue_run(a, chat_id, "only ever relayed; B is away").await;
    wait_for_replies(a, chat_id, 3, "third reply on A").await;
    pass("a turn ran on A while B was away");
    let start = std::process::Command::new("bash").arg(&restart).arg("start-old").status();
    if !start.is_ok_and(|status| status.success()) {
        fail("couldn't restart B");
    }
    let deadline = Instant::now() + STEP_TIMEOUT;
    let old_b = loop {
        match connect_ipc(b_port, b_dir).await {
            Ok(client) => break client,
            Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(250)).await,
            Err(err) => fail(&format!("reconnecting to B: {err}")),
        }
    };
    wait_for_replies(&old_b, chat_id, 3, "B, without the layer, catching up").await;
    pass("B came back without the layer and got the relayed turn");
    if chat_generation(a, chat_id).await != Some(3) {
        fail("an old client on the account moved the chat off generation 3");
    }
    pass("the chat stays on generation 3 with an old client here");
    let while_here = room_stats(chat_id);
    queue_run(&old_b, chat_id, "from the old client").await;
    wait_for_replies(&old_b, chat_id, 4, "fourth reply on B").await;
    pass("B, without the layer, can still send and follow a turn");
    wait_for_replies(a, chat_id, 4, "fourth reply on A").await;
    let stored = room_stats(chat_id);
    if stored["headSeq"] == while_here["headSeq"] && stored["checkpointSeq"] == while_here["checkpointSeq"] {
        fail(&format!("nothing was stored for the old client: {stored}"));
    }
    pass("the host stored for the old client while it was here");

    // It leaves: once its hold is over, turns store nothing again.
    drop(old_b);
    let stop = std::process::Command::new("bash").arg(&restart).arg("stop").status();
    if !stop.is_ok_and(|status| status.success()) {
        fail("couldn't stop the old B");
    }
    let settle: u64 = std::env::var("ZERON_E2E_OLD_GONE_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    tokio::time::sleep(Duration::from_secs(settle)).await;
    let before = room_stats(chat_id);
    queue_run(a, chat_id, "the old client has gone").await;
    wait_for_replies(a, chat_id, 5, "fifth reply on A").await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let after = room_stats(chat_id);
    if after["headSeq"] != before["headSeq"] {
        fail(&format!("a turn stored rows after the old client left: before {before}, after {after}"));
    }
    if chat_generation(a, chat_id).await != Some(3) {
        fail("the chat left generation 3");
    }
    pass("the old client left, and turns store nothing again");
}

/// Text a snapshot can't compress much: its size is what it says.
fn incompressible(len: usize) -> String {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            char::from(b'a' + ((seed >> 58) as u8 % 26))
        })
        .collect()
}

/// The chat's row on `client`, by id.
async fn chat_generation(client: &RpcClient, chat_id: &str) -> Option<u64> {
    wait_stream(client, methods::WATCH_CHATS, serde_json::json!({}), "chat list", |item| {
        let chat = item
            .as_array()?
            .iter()
            .find(|chat| chat.get("id").and_then(|v| v.as_str()) == Some(chat_id))?;
        Some(chat.get("roomGen").and_then(|v| v.as_u64()))
    })
    .await
}

async fn create_chat(client: &RpcClient, space_id: &str) -> String {
    let chat_id = uuid::Uuid::new_v4().to_string();
    client
        .call(
            methods::MUTATE,
            serde_json::json!({
                "op": "createChat",
                "chatId": chat_id,
                "spaceId": space_id,
                "config": { "harness": "mock", "model": null, "reasoning": null, "sandbox": "workspace-write" },
            }),
        )
        .await
        .unwrap_or_else(|err| fail(&format!("createChat: {err}")));
    chat_id
}

/// A chat too big to relay never moves to generation 3: not after its first save,
/// and not when its host restarts and reopens it, before any save. The engines run
/// with a 64 KiB relay limit, so a 100 KB message makes a whale.
async fn whale_chat(a: &RpcClient, b: &RpcClient, space_id: &str, a_port: u16, a_dir: Option<&std::path::Path>) {
    let chat_id = create_chat(a, space_id).await;
    queue_run(a, &chat_id, &incompressible(100_000)).await;
    wait_for_replies(b, &chat_id, 1, "whale reply on B").await;
    tokio::time::sleep(Duration::from_secs(14)).await;
    if chat_generation(a, &chat_id).await != Some(2) {
        fail("a chat over the relay limit moved to generation 3");
    }
    pass("a chat over the relay limit stays on stored rows");
    let Ok(restart) = std::env::var("ZERON_E2E_RESTART_A") else {
        return;
    };
    let restarted = std::process::Command::new("bash").arg(&restart).status();
    if !restarted.is_ok_and(|status| status.success()) {
        fail("couldn't restart A");
    }
    let deadline = Instant::now() + STEP_TIMEOUT;
    let a = loop {
        match connect_ipc(a_port, a_dir).await {
            Ok(client) => break client,
            Err(_) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(250)).await,
            Err(err) => fail(&format!("reconnecting to A: {err}")),
        }
    };
    // Reopened, with nothing new to save: its size is known only from the load.
    wait_for_replies(&a, &chat_id, 1, "whale on A after its restart").await;
    tokio::time::sleep(Duration::from_secs(14)).await;
    if chat_generation(&a, &chat_id).await != Some(2) {
        fail("a reopened chat over the relay limit moved to generation 3 before its first save");
    }
    pass("reopened, it still stays on stored rows");
}

/// A message too big for a stored row is rejected, and reaches the other device
/// through the host's checkpoint. The turns after it depend on it: the other
/// device must take them in, not wait on history it can't get.
async fn rejected_push(a: &RpcClient, b: &RpcClient, space_id: &str) {
    let chat_id = create_chat(a, space_id).await;
    queue_run(a, &chat_id, "before").await;
    wait_for_replies(b, &chat_id, 1, "first reply on B").await;
    queue_run(a, &chat_id, &incompressible(1_100_000)).await;
    wait_for_replies(a, &chat_id, 2, "the oversized turn on A").await;
    queue_run(a, &chat_id, "after").await;
    wait_for_replies(b, &chat_id, 3, "the turns after a rejected push, on B").await;
    let mut transcript = Vec::new();
    let longest = wait_stream(
        b,
        methods::WATCH_DOC_MESSAGES,
        serde_json::json!({ "chatId": chat_id }),
        "the oversized message on B",
        |item| {
            apply_transcript_item(&mut transcript, item).ok()?;
            let longest = transcript
                .iter()
                .map(|entry| serde_json::to_string(entry).map_or(0, |text| text.len()))
                .max()?;
            (longest > 1_000_000).then_some(longest)
        },
    )
    .await;
    pass(&format!("a rejected push reached B by checkpoint ({longest} bytes), and the turns after it too"));
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let a_port: u16 = args
        .next()
        .unwrap_or_else(|| "27801".into())
        .parse()
        .expect("A port");
    let b_port: u16 = args
        .next()
        .unwrap_or_else(|| "27802".into())
        .parse()
        .expect("B port");

    // Each engine publishes its IPC bearer at `{data_dir}/ipc-token`.
    let a_dir = args.next().map(std::path::PathBuf::from);
    let b_dir = args.next().map(std::path::PathBuf::from);

    let a = connect_ipc(a_port, a_dir.as_deref())
        .await
        .unwrap_or_else(|err| fail(&format!("connect device A ipc :{a_port}: {err}")));
    let b = connect_ipc(b_port, b_dir.as_deref())
        .await
        .unwrap_or_else(|err| fail(&format!("connect device B ipc :{b_port}: {err}")));

    // 1. Distinct identities.
    let a_dev = device_id(&a, "A").await;
    let b_dev = device_id(&b, "B").await;
    if a_dev == b_dev {
        fail("A and B report the same deviceId — data dirs are shared");
    }
    pass(&format!("devices distinct (A={a_dev} B={b_dev})"));

    // 2. Create a space + chat on A, hosted by A, with the mock harness (the
    //    space fixes device + cwd — the chat row derives both from it).
    let space_id = uuid::Uuid::new_v4().to_string();
    a.call(
        methods::MUTATE,
        serde_json::json!({
            "op": "createSpace",
            "spaceId": space_id,
            "deviceId": a_dev,
            "path": "/tmp",
        }),
    )
    .await
    .unwrap_or_else(|err| fail(&format!("createSpace on A: {err}")));
    let chat_id = uuid::Uuid::new_v4().to_string();
    a.call(
        methods::MUTATE,
        serde_json::json!({
            "op": "createChat",
            "chatId": chat_id,
            "spaceId": space_id,
            "config": {
                "harness": "mock",
                "model": null,
                "reasoning": null,
                "sandbox": "workspace-write",
            },
        }),
    )
    .await
    .unwrap_or_else(|err| fail(&format!("createChat on A: {err}")));
    pass(&format!("space + chat created on A ({chat_id})"));

    // 2b. Space row syncs A → edge → B (WatchSpaces).
    let space_device = wait_stream(
        &b,
        methods::WATCH_SPACES,
        serde_json::json!({}),
        "space row visible on B",
        |item| {
            item.as_array()?
                .iter()
                .find(|space| space.get("id").and_then(|v| v.as_str()) == Some(space_id.as_str()))
                .and_then(|space| space.get("deviceId")?.as_str().map(str::to_string))
        },
    )
    .await;
    if space_device != a_dev {
        fail(&format!(
            "space synced to B but owned by {space_device}, expected {a_dev}"
        ));
    }
    pass("space row synced A -> edge -> B (owner = A)");

    // 3. Workspace sync A → edge → B: the chat row appears in B's WatchChats.
    let hosted_by = wait_stream(
        &b,
        methods::WATCH_CHATS,
        serde_json::json!({}),
        "chat row visible on B",
        |item| {
            item.as_array()?
                .iter()
                .find(|chat| chat.get("id").and_then(|v| v.as_str()) == Some(chat_id.as_str()))
                .and_then(|chat| chat.get("deviceId")?.as_str().map(str::to_string))
        },
    )
    .await;
    if hosted_by != a_dev {
        fail(&format!(
            "chat synced to B but hosted by {hosted_by}, expected {a_dev}"
        ));
    }
    pass("chat row synced A -> edge -> B (host = A)");

    // 4. Queue the run from B THROUGH THE DOC (no targetDeviceId): B commits the
    //    command into its replica; the nudge + room sync hand it to A's executor.
    let message_id = uuid::Uuid::new_v4().to_string();
    b.call(
        methods::QUEUE_COMMAND,
        serde_json::json!({
            "chatId": chat_id,
            "command": {
                "kind": "run",
                "messageId": message_id,
                "request": {
                    "prompt": "e2e smoke: report in",
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
    .unwrap_or_else(|err| fail(&format!("QueueCommand on B: {err}")));
    pass("run command queued on B via the doc command queue");

    // 5a. Assistant entry executed by A arrives back on B, complete, with the
    // mock text. WatchDocMessages speaks the framed protocol: a full `reset`
    // list on attach, then `upsert`/`append` deltas. Materialize every frame
    // through the shared parser so candidates persist across incremental frames.
    let mut transcript = Vec::new();
    let (by_device, text) = wait_stream(
        &b,
        methods::WATCH_DOC_MESSAGES,
        serde_json::json!({ "chatId": chat_id }),
        "assistant transcript on B",
        |item| {
            apply_transcript_item(&mut transcript, item)
                .unwrap_or_else(|err| fail(&format!("assistant transcript on B: {err}")));
            let entry = transcript.iter().find(|entry| {
                entry.role == zeron_doc::MessageRole::Assistant
                    && entry.status == Some(zeron_doc::MessageStatus::Complete)
            })?;
            let device = entry.device_id.clone();
            let text = serde_json::to_string(entry)
                .unwrap_or_else(|err| fail(&format!("serialize assistant transcript: {err}")));
            Some((device, text))
        },
    )
    .await;
    if by_device != a_dev {
        fail(&format!(
            "assistant entry written by {by_device}, expected host {a_dev}"
        ));
    }
    if !text.contains(MOCK_TEXT) {
        fail(&format!(
            "assistant entry lacks the mock transcript: {text}"
        ));
    }
    pass("assistant entry (host-executed, status=complete) synced back to B");

    // 5b. Session-status row round-trips: B sees A's session for this chat go idle.
    wait_stream(
        &b,
        methods::WATCH_SESSIONS,
        serde_json::json!({}),
        "session status on B",
        |item| {
            item.as_array()?.iter().find(|session| {
                session.get("chatId").and_then(|v| v.as_str()) == Some(chat_id.as_str())
                    && session.get("deviceId").and_then(|v| v.as_str()) == Some(a_dev.as_str())
                    && session.get("status").and_then(|v| v.as_str()) == Some("idle")
            })?;
            Some(())
        },
    )
    .await;
    pass("session status (A working -> idle) round-tripped to B");

    if std::env::var("ZERON_E2E_RELAY").as_deref() == Ok("1") {
        relay_layer(&a, &b, &chat_id, b_port, b_dir.as_deref()).await;
        if std::env::var("ZERON_E2E_RESTART_B").is_err() {
            whale_chat(&a, &b, &space_id, a_port, a_dir.as_deref()).await;
        }
    } else {
        rejected_push(&a, &b, &space_id).await;
    }

    println!("PASS: two-device e2e smoke complete");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_reset_frame_is_materialized() {
        let mut transcript = Vec::new();
        apply_transcript_item(
            &mut transcript,
            &serde_json::json!({
                "reset": [{
                    "id": "assistant-1",
                    "role": "assistant",
                    "parts": [{ "kind": "text", "id": "t0", "text": MOCK_TEXT }],
                    "createdAt": 1,
                    "deviceId": "device-a",
                    "status": "complete"
                }]
            }),
        )
        .expect("reset frame");

        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].device_id, "device-a");
        assert_eq!(
            transcript[0].status,
            Some(zeron_doc::MessageStatus::Complete)
        );
    }
}
