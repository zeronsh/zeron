//! Multi-chat fixture generation through the engine RPC interface.
use crate::{Trial, rpc::Client, transcript::ChatView};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn id(kind: u32, n: usize) -> String {
    format!("b400{kind:04x}-0000-4000-8000-{n:012x}")
}

pub async fn generate(name: &str, engine: &Path, seed: u64) -> Result<PathBuf> {
    let started = Instant::now();
    let (count, spaces, repos) = match name {
        "P20" => (20, 5, 5),
        "P200" => (200, 30, 10),
        _ => unreachable!(),
    };
    let root = super::safe_root()?;
    let output = super::fixture_dir(
        &root,
        &["fixtures", name, &format!("{}-{}", seed, crate::mono_now())],
    )?;
    let mut trial = Trial::create(&root, "fixture-multi")?;
    trial.allocate_port()?;
    trial.prepare()?;
    let mut paths = Vec::new();
    for n in 0..repos {
        let path = output.join(format!("repo-{n}"));
        fs::create_dir(&path)?;
        ensure!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(&path)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .status()?
                .success(),
            "git init failed"
        );
        paths.push(path);
    }
    let mut space_paths = Vec::new();
    for n in 0..spaces {
        let path = paths[n % repos].join(format!("space-{n}"));
        fs::create_dir(&path)?;
        space_paths.push(path);
    }
    let mut engine_proc = Some(crate::launch(engine, &trial).await?);
    let mut client = Client::connect(trial.port, &trial.data).await?;
    let (device, _) = client.call("LocalDevice", json!({})).await?;
    for n in 0..spaces {
        client.call("Mutate",json!({"op":"createSpace","spaceId":id(1,n),"deviceId":device["deviceId"],"path":space_paths[n]})).await?;
    }
    let mut sizes = Vec::new();
    let crash_start = if name == "P200" { 180 } else { count };
    for n in 0..count {
        if name == "P200" && n < 180 && n > 0 && (n <= 2 || n % 10 == 0) {
            drop(client);
            engine_proc.take().unwrap().stop().await?;
            trial.allocate_port()?;
            engine_proc = Some(crate::launch(engine, &trial).await?);
            client = Client::connect(trial.port, &trial.data).await?;
        }
        let chat = id(2, n);
        let path = &space_paths[n % spaces];
        client.call("Mutate",json!({"op":"createChat","chatId":chat,"spaceId":id(1,n%spaces),"config":{"harness":"claude-code","model":"haiku","reasoning":null,"sandbox":"workspace-write"}})).await?;
        let target: usize = if name == "P20" {
            match n {
                0..=7 => 100_000,
                8..=13 => 1_000_000,
                14..=16 => 5_000_000,
                _ => 0,
            }
        } else if n < 2 {
            30_000_000
        } else if n < 20 {
            4_400_000
        } else {
            176_000
        };
        if target == 0 {
            sizes.push(0);
            continue;
        }
        let crash = n >= crash_start;
        let text = super::text::generate(
            seed.wrapping_add(n as u64),
            if crash {
                100_000
            } else {
                target.saturating_sub(2000)
            },
        );
        let script = output.join(format!("turn-{n}.zbr.jsonl"));
        fs::write(
            &script,
            super::scripts::from_text(
                &format!("{name}-{n}"),
                seed.wrapping_add(n as u64),
                &text,
                if crash { "opus" } else { "burst" },
            )?,
        )?;
        let mut sub = client
            .subscribe(
                "WatchDocMessages",
                json!({"chatId":chat,"openingTail":true}),
            )
            .await?;
        client.call("QueueCommand",json!({"chatId":chat,"command":{"kind":"run","messageId":id(3,n),"request":{"prompt":format!("zb:play {} speed={}",script.display(),if crash {1} else {0}),"model":"haiku","reasoning":null,"modelOptions":{},"cwd":path,"sandbox":"workspace-write","autoApprove":true,"resume":null}}})).await?;
        let mut view = ChatView::default();
        let size = tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let item = sub.recv().await.context("transcript closed")??;
                view.apply(&item.value)?;
                if let Some(entry) = view.entries.iter().find(|e| e["role"] == "assistant") {
                    if entry["status"] == "failed" || entry["status"] == "aborted" {
                        anyhow::bail!("chat {chat} failed");
                    }
                    if (crash && entry["status"] == "streaming")
                        || (!crash && entry["status"] == "complete")
                    {
                        if !crash {
                            ensure!(view.text() == text, "text mismatch for {chat}");
                        }
                        return Ok::<usize, anyhow::Error>(
                            serde_json::to_vec(&view.entries)?.len(),
                        );
                    }
                }
            }
        })
        .await
        .context("turn timed out")??;
        sizes.push(size);
        eprintln!("{name}: {}/{} transcript_bytes={size}", n + 1, count);
    }
    if name == "P200" {
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    drop(client);
    if name == "P200" {
        drop(engine_proc.take());
    } else {
        engine_proc.take().unwrap().stop().await?;
    }
    fs::rename(&trial.data, output.join("data"))?;
    let mut hashes = std::collections::BTreeMap::new();
    super::profile::hash_tree(&output, &output, &mut hashes)?;
    let manifest = json!({"name":name,"seed":seed,"transcript_bytes":sizes,"crash_chats":if name=="P200" {(180..200).map(|n|id(2,n)).collect::<Vec<_>>()} else {vec![]},"generation_ms":started.elapsed().as_millis(),"files":hashes});
    fs::write(
        output.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    Ok(output)
}

pub async fn verify(path: &Path, engine: &Path) -> Result<()> {
    let root = super::safe_root()?;
    let fixture = fs::canonicalize(path)?;
    ensure!(
        fixture.starts_with(root.join("fixtures")),
        "fixture must be under the bench fixture root"
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.join("manifest.json"))?)?;
    let crash = manifest["crash_chats"]
        .as_array()
        .context("crash chat list missing")?;
    ensure!(crash.len() == 20, "expected 20 crash chats");
    let mut trial = Trial::create(&root, "fixture-verify")?;
    trial.allocate_port()?;
    trial.prepare()?;
    ensure!(
        Command::new("cp")
            .arg("-a")
            .arg("--reflink=auto")
            .arg(fixture.join("data/."))
            .arg(&trial.data)
            .status()?
            .success(),
        "copy fixture failed"
    );
    let engine_proc = crate::launch(engine, &trial).await?;
    let result = async {
        let client = Client::connect(trial.port, &trial.data).await?;
        let mut chats = client.subscribe("WatchChats", json!({})).await?;
        let item = chats.recv().await.context("chats stream closed")??;
        let list = item
            .value
            .as_array()
            .context("chat listing is not an array")?;
        ensure!(list.len() == 200, "expected 200 chats, got {}", list.len());
        for chat in crash {
            let chat = chat.as_str().context("invalid crash id")?;
            let mut stream = client
                .subscribe(
                    "WatchDocMessages",
                    json!({"chatId":chat,"openingTail":false}),
                )
                .await?;
            let mut view = ChatView::default();
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let item = stream.recv().await.context("transcript closed")??;
                    view.apply(&item.value)?;
                    if view
                        .entries
                        .iter()
                        .any(|e| e["role"] == "assistant" && e["status"] == "aborted")
                    {
                        break Ok::<_, anyhow::Error>(());
                    }
                }
            })
            .await
            .with_context(|| format!("chat {chat} did not recover aborted"))??;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let stopped = engine_proc.stop().await;
    result?;
    stopped?;
    Ok(())
}
