#![cfg(unix)]
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{path::PathBuf, process::Command, time::Duration};
use zeron_bench::{
    Trial, default_zb_root, launch,
    rpc::{Checkout, Client, GLOBAL, PER_CHAT, UiSet},
    transcript::ChatView,
};

#[tokio::test]
#[ignore = "requires ZB_TEST_ENGINE pointing to an isolated engine build"]
async fn isolated_engine_mock_turn_and_ui_streams() -> Result<()> {
    let engine_bin = PathBuf::from(
        std::env::var_os("ZB_TEST_ENGINE").context("set ZB_TEST_ENGINE to an engine binary")?,
    );
    let root = default_zb_root().join("scratch/b5-rpc-driver/live");
    std::fs::create_dir_all(&root)?;
    let mut trial = Trial::create(root, "rpc-live")?;
    trial
        .extra
        .push(("ZERON_MOCK_DELAY_MS".into(), "150".into()));
    trial.extra.push(("ZERON_MOCK_CHARS".into(), "70".into()));
    trial.allocate_port()?;
    trial.bench_bin = PathBuf::from(env!("CARGO_BIN_EXE_zeron-bench"));
    trial.prepare()?;
    let project = trial.root.join("project");
    std::fs::create_dir_all(&project)?;
    ensure!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&project)
            .status()?
            .success(),
        "git init failed"
    );
    let engine = launch(engine_bin, &trial).await?;
    let run = async {
        let client = Client::connect(trial.port, &trial.data).await?;
        let (device, _) = client.call("LocalDevice", json!({})).await?;
        let space = "16000000-0000-4000-8000-000000000001";
        let chat = "16000000-0000-4000-8000-000000000002";
        let cwd = project.to_str().context("project path")?;
        client.call("Mutate", json!({"op":"createSpace", "spaceId":space, "deviceId":device["deviceId"], "path":cwd})).await?;
        client.call("Mutate", json!({"op":"createChat", "chatId":chat, "spaceId":space, "config":{"harness":"mock", "model":"fable-5", "reasoning":null, "sandbox":"workspace-write"}})).await?;
        let mut ui = UiSet::new(client.clone(), Checkout { cwd: cwd.into(), branch: None, target_device_id: None });
        tokio::time::timeout(Duration::from_secs(30), async { ui.open_global().await?; ui.open_chat(chat).await }).await??;
        assert_eq!(ui.global.len() + ui.chats[chat].len(), GLOBAL.len() + PER_CHAT.len() - 1);
        for method in GLOBAL.iter().chain(PER_CHAT.iter()).filter(|m| **m != "FocusChat") { assert!(ui.global.contains_key(method) || ui.chats[chat].contains_key(method), "missing {method}"); }
        client.call("QueueCommand", json!({"chatId":chat,"command":{"kind":"run","messageId":"16000000-0000-4000-8000-000000000003","request":{"prompt":"test mock", "model":null,"reasoning":null,"modelOptions":{},"cwd":cwd,"sandbox":"workspace-write","autoApprove":true,"resume":null}}})).await?;
        let stream = ui.chats.get_mut(chat).unwrap().get_mut("WatchDocMessages").unwrap();
        let mut view = ChatView::default();
        let mut frames = Vec::new();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let item = stream.recv().await.context("transcript stream closed")??;
                ensure!(item.bytes > 0 && item.t_recv_ns > 0, "missing receive measurement");
                frames.push(item.value.clone());
                view.apply(&item.value)?;
                if view.entries.iter().any(|e| e["role"] == "assistant" && e["status"] == "complete") { break; }
            }
            Result::<()>::Ok(())
        }).await??;
        std::fs::write(trial.root.join("recorded-frames.json"), serde_json::to_vec_pretty(&frames)?)?;
        ensure!(frames.first().is_some_and(|f| f["historyPending"] == true && f.get("reset").is_some()), "missing opening preview");
        ensure!(frames.iter().skip(1).any(|f| f.get("reset").is_some() && f.get("historyPending").is_none()), "missing full reset");
        ensure!(frames.iter().any(|f| f["append"].as_array().is_some_and(|a| !a.is_empty())), "missing append delta");
        let expected = ["## Streaming pipeline\n\nEvery turn flows through the same path:\n\n1. **Doc command** — the composer queues a durable `run` entry\n2. **Host executor** — the chat's host device marks it processed, then dispatches\n3. **Fold** — events fold into parts and diff into the Loro doc every 120ms\n\n", "The `SegmentWriter` appends into `LoroText` so the oplog stays RLE-merged:\n\n```rust\nfolded = fold_event_into_parts(&folded, &event);\nwriter.sync(&folded)?; // 120ms coalesced commits\n```\n\nSynced to every device through the session room. *Mock harness reporting in.*"].join("\n\n");
        assert_eq!(view.text(), expected);
        ui.close_chat(chat);
        assert!(ui.chats.is_empty());
        Result::<()>::Ok(())
    }.await;
    let stopped = engine.stop().await;
    run?;
    stopped?;
    Ok(())
}
