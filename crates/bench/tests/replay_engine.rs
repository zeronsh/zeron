//! Opt-in release-engine check using the frozen B0 launcher.
//! ZB_TEST_ENGINE=/absolute/path/to/release/zeron-engine cargo test -p
//! zeron-bench --test replay_engine -- --ignored --nocapture
#![cfg(unix)]
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;
use zeron_bench::replay::script::{Script, text_sha256};
use zeron_bench::{Trial, launch};

fn apply(entries: &mut Vec<Value>, frame: &Value) -> Result<()> {
    if let Some(reset) = frame["reset"].as_array() {
        *entries = reset.clone();
        return Ok(());
    }
    for remove in frame["remove"].as_array().into_iter().flatten() {
        entries.retain(|e| e["id"] != *remove);
    }
    for update in frame["upsert"].as_array().into_iter().flatten() {
        let entry = &update["entry"];
        entries.retain(|e| e["id"] != entry["id"]);
        let after = &update["after"];
        let index = if after.is_null() {
            0
        } else {
            entries
                .iter()
                .position(|e| e["id"] == *after)
                .context("unknown upsert anchor")?
                + 1
        };
        entries.insert(index, entry.clone());
    }
    for append in frame["append"].as_array().into_iter().flatten() {
        let entry = entries
            .iter_mut()
            .find(|e| e["id"] == append["entry"])
            .context("append entry missing")?;
        let part = entry["parts"]
            .as_array_mut()
            .context("parts missing")?
            .iter_mut()
            .find(|p| p["id"] == append["part"])
            .context("append part missing")?;
        let mut text = part["text"].as_str().context("append nontext")?.to_owned();
        text.push_str(append["text"].as_str().context("append text missing")?);
        ensure!(
            text.len() as u64 == append["len"].as_u64().context("append len missing")?,
            "append length mismatch"
        );
        part["text"] = text.into();
    }
    if let Some(count) = frame["count"].as_u64() {
        ensure!(entries.len() as u64 == count, "transcript count mismatch");
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires a release engine built in this worktree; set ZB_TEST_ENGINE"]
async fn isolated_release_engine_replays_50kb_and_matches_script_hash() -> Result<()> {
    let engine_bin = PathBuf::from(
        std::env::var_os("ZB_TEST_ENGINE")
            .context("ZB_TEST_ENGINE must name the release engine")?,
    );
    ensure!(
        engine_bin.is_absolute() && engine_bin.is_file(),
        "missing release engine"
    );
    let root = PathBuf::from("/home/user/Documents/dev/tmp/zeron-perf/scratch/b3-replay");
    let mut trial = Trial::create(&root, "replay-engine")?;
    trial.allocate_port()?;
    trial.replay_bin = PathBuf::from(env!("CARGO_BIN_EXE_zb-replay"));
    trial.bench_bin = PathBuf::from(env!("CARGO_BIN_EXE_zeron-bench"));
    trial
        .extra
        .push(("ZERON_HARNESS".into(), "claude-code".into()));
    trial.extra.push((
        "ZB_REPLAY_STATE_DIR".into(),
        trial.root.join("state").into_os_string(),
    ));
    trial.extra.push((
        "ZB_REPLAY_LOG_DIR".into(),
        trial.root.join("replay").into_os_string(),
    ));
    trial.prepare()?;
    let project = trial.root.join("project");
    std::fs::create_dir_all(&project)?;
    let git = std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .arg(&project)
        .env_clear()
        .env("HOME", &trial.home)
        .env("XDG_CONFIG_HOME", trial.home.join(".config"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()?;
    ensure!(git.success(), "git init failed");
    let text =
        "# Replay acceptance\nA deterministic paragraph with `code`, **bold**, and a link.\n"
            .repeat(700)
            .chars()
            .take(50 * 1024)
            .collect::<String>();
    assert_eq!(text.len(), 50 * 1024);
    let path = trial.root.join("50k.zbr.jsonl");
    let mut frames = vec![
        json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
    ];
    frames.extend(text.as_bytes().chunks(100).map(|c|json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":std::str::from_utf8(c).unwrap()}}})));
    frames.push(json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}));
    let mut source = json!({"zb":"meta","v":1,"name":"50k acceptance","seed":42,"frames":frames.len(),"text_bytes":text.len(),"text_sha256":text_sha256(&text)}).to_string()+"\n";
    for f in frames {
        source.push_str(&json!({"dt":0,"f":f}).to_string());
        source.push('\n');
    }
    source.push_str(
        "{\"dt\":0,\"zb\":\"turn_end\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}\n",
    );
    std::fs::write(&path, source)?;
    let script = Script::load(&path)?;
    let engine = launch(&engine_bin, &trial).await?;
    eprintln!(
        "trial: {}, engine pid={}, port={}, containment={}",
        trial.root.display(),
        engine.pid,
        trial.port,
        engine.containment()
    );
    // Explicit token: never consult the agent's own ZERON_IPC_TOKEN env.
    let token = std::fs::read_to_string(trial.data.join("ipc-token"))?;
    let client = zeron_rpc::connect_ws(
        &format!("ws://127.0.0.1:{}", trial.port),
        Some(token.trim()),
    )
    .await?;
    let result: Result<()> = async {
        let device = client.call("LocalDevice",json!({})).await?;
        let space = "b3000000-0000-4000-8000-000000000001";
        let chat = "b3000000-0000-4000-8000-000000000002";
        client.call("Mutate",json!({"op":"createSpace","spaceId":space,"deviceId":device["deviceId"],"path":project})).await?;
        client.call("Mutate",json!({"op":"createChat","chatId":chat,"spaceId":space,"config":{"harness":"claude-code","model":"haiku","reasoning":null,"sandbox":"workspace-write"}})).await?;
        client.call("Mutate",json!({"op":"renameChat","chatId":chat,"title":"Replay acceptance"})).await?;
        let mut stream = client.subscribe("WatchDocMessages",json!({"chatId":chat,"openingTail":true})).await?;
        client.call("QueueCommand",json!({"chatId":chat,"command":{"kind":"run","messageId":"b3000000-0000-4000-8000-000000000003","request":{
            "prompt":format!("zb:play {} speed=0",path.display()),"model":"haiku","reasoning":null,"modelOptions":{},"cwd":project,
            "sandbox":"workspace-write","autoApprove":true,"resume":null}}})).await?;
        let mut entries = Vec::new();
        let mut recorded = String::new();
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                let frame = stream.recv().await.context("transcript stream closed")?;
                recorded.push_str(&frame.to_string()); recorded.push('\n');
                apply(&mut entries, &frame)?;
                let assistants: Vec<_> = entries.iter().filter(|e|e["role"]=="assistant").collect();
                if !assistants.is_empty() && assistants.iter().all(|e|e["status"]=="complete") {
                    let received: String = assistants.iter().flat_map(|e|e["parts"].as_array().into_iter().flatten())
                        .filter(|p|p["kind"]=="text").filter_map(|p|p["text"].as_str()).collect();
                    ensure!(received.len()==script.meta.text_bytes,"received {} text bytes, expected {}",received.len(),script.meta.text_bytes);
                    ensure!(text_sha256(&received)==script.meta.text_sha256,"transcript hash differs from script");
                    eprintln!("transcript: {} bytes, sha256={}",received.len(),text_sha256(&received));
                    return Ok::<_,anyhow::Error>(());
                }
                ensure!(!assistants.iter().any(|e| e["status"]=="failed" || e["status"]=="aborted"), "assistant failed: {entries:?}");
            }
        }).await.context("50k transcript timed out")??;
        std::fs::write(trial.root.join("watch.ndjson"),recorded)?;
        Ok(())
    }.await;
    let stopped = engine.stop().await?;
    eprintln!("shutdown: {:?}", stopped.how);
    result
}
