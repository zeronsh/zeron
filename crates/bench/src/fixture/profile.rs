//! Engine-backed fixture construction.
use crate::{Trial, rpc::Client, transcript::ChatView};
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

pub async fn generate(name: &str, engine: &Path, seed: u64) -> Result<PathBuf> {
    let target = match name {
        "T-100k" => 100_000,
        "T-1m" => 1_000_000,
        "T-5m" => 5_000_000,
        _ => bail!("profile {name} is not implemented"),
    };
    let started = Instant::now();
    let root = super::safe_root()?;
    let id = format!("{}-{}", seed, crate::mono_now());
    let output = super::fixture_dir(&root, &["fixtures", name, &id])?;
    let mut trial = Trial::create(&root, "fixture")?;
    trial.allocate_port()?;
    trial.prepare()?;
    let project = output.join("project");
    fs::create_dir(&project)?;
    let status = Command::new("git")
        .args(["init", "-q"])
        .arg(&project)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()?;
    ensure!(status.success(), "git init failed");
    let text = super::text::generate(seed, target.saturating_sub(2000));
    let script_path = output.join("turn.zbr.jsonl");
    ensure!(
        !script_path
            .to_string_lossy()
            .chars()
            .any(char::is_whitespace),
        "replay path contains whitespace"
    );
    fs::write(
        &script_path,
        super::scripts::from_text(name, seed, &text, "burst")?,
    )?;
    let launched = crate::launch(engine, &trial).await?;
    let result = build_chat(&trial, &project, &script_path, &text).await;
    let stopped = launched.stop().await;
    let transcript_bytes = result?;
    stopped?;
    ensure!(
        (target * 9 / 10..=target * 11 / 10).contains(&transcript_bytes),
        "transcript {transcript_bytes} outside target tolerance"
    );
    fs::rename(&trial.data, output.join("data"))?;
    let mut hashes = std::collections::BTreeMap::new();
    hash_tree(&output, &output, &mut hashes)?;
    let engine_bytes = fs::read(engine).context("read fixture engine")?;
    let manifest = json!({"name":name,"seed":seed,"engine_sha256":format!("{:x}",Sha256::digest(engine_bytes)),"transcript_bytes":[transcript_bytes],"generation_ms":started.elapsed().as_millis(),"files":hashes});
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(output)
}

async fn build_chat(trial: &Trial, project: &Path, script: &Path, expected: &str) -> Result<usize> {
    let client = Client::connect(trial.port, &trial.data).await?;
    let (device, _) = client.call("LocalDevice", json!({})).await?;
    let space = "b4000000-0000-4000-8000-000000000001";
    let chat = "b4000000-0000-4000-8000-000000000002";
    client.call("Mutate", json!({"op":"createSpace","spaceId":space,"deviceId":device["deviceId"],"path":project})).await?;
    client.call("Mutate", json!({"op":"createChat","chatId":chat,"spaceId":space,"config":{"harness":"claude-code","model":"haiku","reasoning":null,"sandbox":"workspace-write"}})).await?;
    let mut stream = client
        .subscribe(
            "WatchDocMessages",
            json!({"chatId":chat,"openingTail":true}),
        )
        .await?;
    client.call("QueueCommand",json!({"chatId":chat,"command":{"kind":"run","messageId":"b4000000-0000-4000-8000-000000000003","request":{"prompt":format!("zb:play {} speed=0",script.display()),"model":"haiku","reasoning":null,"modelOptions":{},"cwd":project,"sandbox":"workspace-write","autoApprove":true,"resume":null}}})).await?;
    let mut view = ChatView::default();
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let item = stream.recv().await.context("transcript stream closed")??;
            view.apply(&item.value)?;
            if let Some(entry) = view.entries.iter().find(|e| e["role"] == "assistant") {
                if entry["status"] == "failed" || entry["status"] == "aborted" {
                    bail!("assistant failed: {entry}");
                }
                if entry["status"] == "complete" {
                    ensure!(
                        view.text() == expected,
                        "engine transcript differs from replay text"
                    );
                    return Ok(serde_json::to_vec(&view.entries)?.len());
                }
            }
        }
    })
    .await
    .context("transcript timed out")?
}

fn hash_tree(
    base: &Path,
    dir: &Path,
    hashes: &mut std::collections::BTreeMap<String, String>,
) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            hash_tree(base, &path, hashes)?;
        } else if kind.is_file() {
            hashes.insert(
                path.strip_prefix(base)?.to_string_lossy().into_owned(),
                format!("{:x}", Sha256::digest(fs::read(&path)?)),
            );
        }
    }
    Ok(())
}
