//! Engine-backed fixture construction.
use crate::{Trial, rpc::Client, transcript::ChatView};
use anyhow::{Context, Result, bail, ensure};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub async fn generate(
    name: &str,
    engine: &Path,
    seed: u64,
    fixture_revision: Option<&str>,
    cadence: Option<&str>,
) -> Result<PathBuf> {
    if matches!(name, "P20" | "P200") {
        ensure!(
            cadence.is_none() || cadence == Some("burst"),
            "P profiles are built with burst cadence"
        );
        return super::multi::generate(name, engine, seed, fixture_revision).await;
    }
    let target: usize = match name {
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
    super::repo::create(&project, false)?;
    let text = super::text::generate(seed, target.saturating_sub(2000));
    let script_path = output.join("turn.zbr.jsonl");
    ensure!(
        !script_path
            .to_string_lossy()
            .chars()
            .any(char::is_whitespace),
        "replay path contains whitespace"
    );
    let cadence = cadence.unwrap_or("opus");
    fs::write(
        &script_path,
        super::scripts::from_text(name, seed, &text, cadence)?,
    )?;
    let build_script = trial.root.join("fixture-build.zbr.jsonl");
    fs::write(
        &build_script,
        super::scripts::from_text(name, seed, &text, "burst")?,
    )?;
    let launched = crate::launch(engine, &trial).await?;
    // Exercise the published script through the engine at least on the small fixture.
    // Larger histories use coalesced frames to keep fixture builds bounded.
    let replay_path = if name == "T-100k" {
        &script_path
    } else {
        &build_script
    };
    let result = build_chat(&trial, &project, replay_path, &text).await;
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
    let (journal_bytes, sqlite_bytes) = data_sizes(&output.join("data"))?;
    let manifest = json!({"fixture_revision":fixture_revision,"journal_bytes":journal_bytes,"sqlite_bytes":sqlite_bytes,"name":name,"seed":seed,"engine_sha256":format!("{:x}",Sha256::digest(engine_bytes)),"transcript_bytes":[transcript_bytes],"generation_ms":started.elapsed().as_millis(),"files":hashes});
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
            if view.entries.iter().any(|e| {
                e["role"] == "assistant" && (e["status"] == "failed" || e["status"] == "aborted")
            }) {
                bail!("assistant failed");
            }
            if super::assistant_text(&view) == expected
                && view
                    .entries
                    .iter()
                    .any(|e| e["role"] == "assistant" && e["status"] == "complete")
            {
                return Ok::<_, anyhow::Error>(());
            }
        }
    })
    .await
    .context("transcript timed out")??;
    drop(stream);
    let mut full = client
        .subscribe(
            "WatchDocMessages",
            json!({"chatId":chat,"openingTail":false}),
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let item = full
                .recv()
                .await
                .context("full transcript stream closed")??;
            if item.value["historyPending"] != true {
                if let Some(reset) = item.value.get("reset") {
                    return Ok::<_, anyhow::Error>(serde_json::to_vec(reset)?.len());
                }
            }
        }
    })
    .await
    .context("authoritative reset timed out")?
}

pub(super) fn hash_tree(
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
            use std::io::Read;
            let mut hasher = Sha256::new();
            let mut file = fs::File::open(&path)?;
            let mut buffer = [0u8; 65536];
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            hashes.insert(
                path.strip_prefix(base)?.to_string_lossy().into_owned(),
                format!("{:x}", hasher.finalize()),
            );
        }
    }
    Ok(())
}

pub(super) fn data_sizes(dir: &Path) -> Result<(u64, u64)> {
    fn walk(path: &Path, journals: &mut u64, sqlite: &mut u64) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            if ty.is_dir() {
                walk(&path, journals, sqlite)?;
            } else if ty.is_file() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let len = entry.metadata()?.len();
                if path.components().any(|c| c.as_os_str() == "journals")
                    && name.ends_with(".jsonl")
                {
                    *journals += len;
                }
                if name.contains(".sqlite") {
                    *sqlite += len;
                }
            }
        }
        Ok(())
    }
    let (mut journals, mut sqlite) = (0, 0);
    walk(dir, &mut journals, &mut sqlite)?;
    Ok((journals, sqlite))
}
