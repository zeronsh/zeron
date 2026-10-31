//! Multi-chat fixture generation through the engine RPC interface.
use crate::{Trial, rpc::Client, transcript::ChatView};
use anyhow::{Context, Result, ensure};
use rand_chacha::{
    ChaCha8Rng,
    rand_core::{RngCore, SeedableRng},
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn id(kind: u32, n: usize) -> String {
    format!("b400{kind:04x}-0000-4000-8000-{n:012x}")
}

fn profile_sizes(seed: u64) -> Vec<usize> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut normals = Vec::new();
    for _ in 0..198 {
        let u1 = (f64::from(rng.next_u32()) + 1.0) / (f64::from(u32::MAX) + 2.0);
        let u2 = (f64::from(rng.next_u32()) + 1.0) / (f64::from(u32::MAX) + 2.0);
        normals.push((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos());
    }
    let mut sorted = normals.clone();
    sorted.sort_by(f64::total_cmp);
    let sigma = (4_400_000.0_f64 / 176_000.0).ln() / (sorted[179] - sorted[99]);
    let mu = 176_000.0_f64.ln() - sigma * sorted[99];
    let mut sizes = vec![30_000_000, 30_000_000];
    sizes.extend(
        normals
            .into_iter()
            .map(|z| (mu + sigma * z).exp().clamp(4_000.0, 10_000_000.0) as usize),
    );
    sizes
}

pub async fn generate(
    name: &str,
    engine: &Path,
    seed: u64,
    fixture_revision: Option<&str>,
) -> Result<PathBuf> {
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
        super::repo::create(&path, n == 0)?;
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
    let p200_sizes = profile_sizes(seed);
    for n in 0..count {
        if name == "P200" && n > 0 && (n <= 2 || n % 10 == 0) {
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
        } else {
            p200_sizes[n]
        };
        if target == 0 {
            sizes.push(2); // JSON encoding of an empty reset array.
            continue;
        }
        let crash = false;
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
                if view.entries.iter().any(|e| {
                    e["role"] == "assistant"
                        && (e["status"] == "failed" || e["status"] == "aborted")
                }) {
                    anyhow::bail!("chat {chat} failed");
                }
                let ready = if crash {
                    view.entries
                        .iter()
                        .any(|e| e["role"] == "assistant" && e["status"] == "streaming")
                } else {
                    super::assistant_text(&view) == text
                        && view
                            .entries
                            .iter()
                            .any(|e| e["role"] == "assistant" && e["status"] == "complete")
                };
                if ready {
                    return Ok::<usize, anyhow::Error>(serde_json::to_vec(&view.entries)?.len());
                }
            }
        })
        .await
        .context("turn timed out")??;
        sizes.push(size);
        eprintln!("{name}: {}/{} transcript_bytes={size}", n + 1, count);
    }
    if name == "P20" {
        fs::write(
            output.join("conformance.zbr.jsonl"),
            super::scripts::conformance_script()?,
        )?;
        let script = output.join("subagent.zbr.jsonl");
        fs::write(&script, super::scripts::subagent_script()?)?;
        for n in 0..2 {
            let chat = id(2, n);
            let path = &space_paths[n % spaces];
            let mut stream = client
                .subscribe(
                    "WatchDocMessages",
                    json!({"chatId":chat,"openingTail":false}),
                )
                .await?;
            let mut view = ChatView::default();
            let initial = stream
                .recv()
                .await
                .context("subagent transcript closed")??;
            view.apply(&initial.value)?;
            let old = view.entries.len();
            client.call("QueueCommand",json!({"chatId":chat,"command":{"kind":"run","messageId":id(5,n),"request":{"prompt":format!("zb:play {} speed=0",script.display()),"model":"haiku","reasoning":null,"modelOptions":{},"cwd":path,"sandbox":"workspace-write","autoApprove":true,"resume":null}}})).await?;
            tokio::time::timeout(Duration::from_secs(60), async {
                loop {
                    let item = stream
                        .recv()
                        .await
                        .context("subagent transcript closed")??;
                    view.apply(&item.value)?;
                    if view.entries.len() > old
                        && view.entries.last().is_some_and(|e| {
                            e["role"] == "assistant"
                                && e["status"] == "complete"
                                && e["parts"]
                                    .as_array()
                                    .is_some_and(|p| p.iter().any(|p| p["kind"] == "text"))
                        })
                    {
                        break Ok::<_, anyhow::Error>(());
                    }
                }
            })
            .await
            .context("subagent turn timed out")??;
            let mut full = client
                .subscribe(
                    "WatchDocMessages",
                    json!({"chatId":chat,"openingTail":false}),
                )
                .await?;
            let item = full.recv().await.context("subagent reset closed")??;
            sizes[n] =
                serde_json::to_vec(item.value.get("reset").context("subagent reset missing")?)?
                    .len();
        }
    }
    let mut crash_chats = Vec::new();
    let mut shutdown_chats = Vec::new();
    if name == "P200" {
        drop(client);
        engine_proc.take().unwrap().stop().await?;
        trial.allocate_port()?;
        engine_proc = Some(crate::launch(engine, &trial).await?);
        client = Client::connect(trial.port, &trial.data).await?;
        let crash_script = output.join("crash.zbr.jsonl");
        let crash_text = super::text::generate(seed, 100_000);
        fs::write(
            &crash_script,
            super::scripts::crash_script(seed, &crash_text)?,
        )?;
        for n in 175..200 {
            let chat = id(2, n);
            let path = &space_paths[n % spaces];
            let mut stream = client
                .subscribe(
                    "WatchDocMessages",
                    json!({"chatId":chat,"openingTail":true}),
                )
                .await?;
            client.call("QueueCommand",json!({"chatId":chat,"command":{"kind":"run","messageId":id(4,n),"request":{"prompt":format!("zb:play {} speed=1",crash_script.display()),"model":"haiku","reasoning":null,"modelOptions":{},"cwd":path,"sandbox":"workspace-write","autoApprove":true,"resume":null}}})).await?;
            let mut view = ChatView::default();
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let item = stream.recv().await.context("crash transcript closed")??;
                    view.apply(&item.value)?;
                    if view
                        .entries
                        .iter()
                        .any(|e| e["role"] == "assistant" && e["status"] == "streaming")
                    {
                        break Ok::<_, anyhow::Error>(());
                    }
                }
            })
            .await
            .context("crash turn did not start")??;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        for n in 175..200 {
            let chat = id(2, n);
            let mut full = client
                .subscribe(
                    "WatchDocMessages",
                    json!({"chatId":chat,"openingTail":false}),
                )
                .await?;
            let item = full.recv().await.context("crash reset closed")??;
            sizes[n] =
                serde_json::to_vec(item.value.get("reset").context("crash reset missing")?)?.len();
        }
        client.call("StopEngine", json!({})).await?;
        let journal_dir = trial.data.join("orgs/dev-org/dev-user/journals");
        let markers = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let markers: Vec<String> = fs::read_dir(&journal_dir)?
                    .filter_map(|entry| {
                        let path = entry.ok()?.path();
                        if path.extension()?.to_str()? == "shutdown" {
                            Some(path.file_stem()?.to_str()?.to_owned())
                        } else {
                            None
                        }
                    })
                    .collect();
                if markers.len() >= 5 {
                    break Ok::<_, anyhow::Error>(markers);
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .context("graceful shutdown did not create markers")??;
        ensure!(
            markers.len() == 5,
            "shutdown passed five markers before crash: {}",
            markers.len()
        );
        drop(engine_proc.take());
        shutdown_chats = markers;
        crash_chats = (175..200)
            .map(|n| id(2, n))
            .filter(|chat| !shutdown_chats.contains(chat))
            .collect();
        ensure!(crash_chats.len() == 20, "expected 20 crash chats");
        let actual = fs::read_dir(&journal_dir)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "shutdown"))
            .count();
        ensure!(
            actual == 5,
            "shutdown marker count changed during crash: {actual}"
        );
    }
    drop(client);
    if name != "P200" {
        engine_proc.take().unwrap().stop().await?;
    }
    if name == "P20" {
        let db = trial.data.join("orgs/dev-org/dev-user/docs.sqlite3");
        let conn =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        for n in 0..2 {
            let pattern = format!("{}--sub--%", id(2, n));
            let count: i64 = conn.query_row(
                "SELECT count(*) FROM snapshots WHERE doc_id LIKE ?1",
                [pattern],
                |row| row.get(0),
            )?;
            ensure!(count > 0, "missing subagent document for chat {}", id(2, n));
        }
    }
    fs::rename(&trial.data, output.join("data"))?;
    let mut hashes = std::collections::BTreeMap::new();
    super::profile::hash_tree(&output, &output, &mut hashes)?;
    let (journal_bytes, sqlite_bytes) = super::profile::data_sizes(&output.join("data"))?;
    let engine_sha256 = format!("{:x}", Sha256::digest(fs::read(engine)?));
    let manifest = json!({"fixture_revision":fixture_revision,"engine_sha256":engine_sha256,"journal_bytes":journal_bytes,"sqlite_bytes":sqlite_bytes,"name":name,"seed":seed,"transcript_bytes":sizes,"crash_chats":crash_chats,"shutdown_chats":shutdown_chats,"generation_ms":started.elapsed().as_millis(),"files":hashes});
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
    let shutdown = manifest["shutdown_chats"]
        .as_array()
        .context("shutdown chat list missing")?;
    ensure!(shutdown.len() == 5, "expected five shutdown chats");
    for chat in shutdown {
        let chat = chat.as_str().context("invalid shutdown chat id")?;
        ensure!(
            fixture
                .join("data/orgs/dev-org/dev-user/journals")
                .join(format!("{chat}.shutdown"))
                .is_file(),
            "missing shutdown marker for {chat}"
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seeded_profile_has_expected_size_distribution() {
        let sizes = profile_sizes(42);
        assert_eq!(sizes, profile_sizes(42));
        assert_eq!(sizes.len(), 200);
        assert_eq!(sizes.iter().filter(|v| **v == 30_000_000).count(), 2);
        let mut sorted = sizes;
        sorted.sort_unstable();
        assert!((175_000..=177_000).contains(&sorted[99]));
        assert!((4_390_000..=4_410_000).contains(&sorted[179]));
    }
}
