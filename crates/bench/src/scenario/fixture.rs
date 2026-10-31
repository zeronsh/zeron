//! Data-dir fixtures built through a real engine and `zb-replay`.
//!
//! TODO(b4): the shared fixture generator (`perf/b4-fixtures`) only builds the
//! single-chat `T-*` profiles so far (`profile::generate` rejects `P20` and
//! `P200`), so the scenarios build their own multi-chat profiles here, with the
//! same method as bench.md 3.2: drive an engine of the fixture revision at
//! `speed=0`, stop it with `StopEngine`, freeze the data dir with a manifest.
//! Switch to B4 once it offers `P20`/`P200`; keep the manifest shape below
//! (it follows bench.md 3.1-3.3).
//!
//! Layout: `ZB_ROOT/fixtures/b7/<id>/{data,projects,manifest.json}`. The data
//! dir records absolute project paths, so the project repos stay in place and
//! trials share them (they are read-mostly git repos).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::RunEnv;
use super::content::{Cadence, Rng, TurnOpts, build_turn};
use crate::rpc::Client;
use crate::transcript::ChatView;
use crate::{Trial, launch};

/// Bump when the generator output changes, so cached fixtures are rebuilt.
const GENERATOR_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSpec {
    /// Target transcript payload in bytes. Zero makes an empty chat.
    pub bytes: usize,
    pub space: usize,
    /// Role in the scenario: `target`, `bg`, `empty`, `outlier`...
    pub tag: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FixtureSpec {
    pub name: String,
    pub seed: u64,
    pub spaces: usize,
    pub repo_files: usize,
    pub repo_commits: usize,
    pub dirty_files: usize,
    pub chats: Vec<ChatSpec>,
    /// Number of chats to leave mid-stream when the engine is SIGKILLed.
    pub crash_residue: usize,
}

impl FixtureSpec {
    pub fn id(&self) -> String {
        let body = serde_json::to_vec(&(GENERATOR_VERSION, self)).unwrap_or_default();
        format!("{}-{}", self.name, &format!("{:x}", Sha256::digest(body))[..10])
    }
}

/// `100k`, `1m`, `5m` (and plain byte counts) to bytes.
pub fn parse_size(text: &str) -> Result<usize> {
    let t = text.trim().to_ascii_lowercase();
    let (num, mul) = if let Some(n) = t.strip_suffix('m') {
        (n, 1_000_000.0)
    } else if let Some(n) = t.strip_suffix('k') {
        (n, 1_000.0)
    } else {
        (t.as_str(), 1.0)
    };
    let value: f64 = num.parse().with_context(|| format!("size {text}"))?;
    Ok((value * mul) as usize)
}

/// bench.md 3.2 `P20`: 8x100 KB, 6x1 MB, 3x5 MB, 3 empty chats over 5 spaces,
/// plus `extra` dedicated chats of `extra_bytes` (S-stream targets).
pub fn p20(name: &str, seed: u64, extra: usize, extra_bytes: usize) -> FixtureSpec {
    let mut sizes: Vec<(usize, &str)> = Vec::new();
    sizes.extend(std::iter::repeat_n((100_000, "bg"), 8));
    sizes.extend(std::iter::repeat_n((1_000_000, "bg"), 6));
    sizes.extend(std::iter::repeat_n((5_000_000, "bg"), 3));
    sizes.extend(std::iter::repeat_n((0, "empty"), 3));
    let mut chats: Vec<ChatSpec> = sizes
        .into_iter()
        .enumerate()
        .map(|(i, (bytes, tag))| ChatSpec {
            bytes,
            space: i % 5,
            tag: tag.into(),
        })
        .collect();
    for i in 0..extra {
        chats.push(ChatSpec {
            bytes: extra_bytes,
            space: i % 5,
            tag: "target".into(),
        });
    }
    FixtureSpec {
        name: name.into(),
        seed,
        spaces: 5,
        repo_files: 200,
        repo_commits: 50,
        dirty_files: 30,
        chats,
        crash_residue: 0,
    }
}

/// S-switch: 12 target chats (4 each of 100 KB, 1 MB, 5 MB) plus background chats.
pub fn p20_switch(seed: u64) -> FixtureSpec {
    let mut chats = Vec::new();
    for (class, bytes) in [(0, 100_000), (1, 1_000_000), (2, 5_000_000)] {
        for i in 0..4 {
            chats.push(ChatSpec {
                bytes,
                space: (class * 4 + i) % 5,
                tag: "target".into(),
            });
        }
    }
    for (i, bytes) in [(0usize, 100_000usize), (1, 100_000), (2, 100_000), (3, 100_000), (4, 1_000_000), (5, 1_000_000), (6, 0), (7, 0)] {
        chats.push(ChatSpec {
            bytes,
            space: i % 5,
            tag: if bytes == 0 { "empty" } else { "bg" }.into(),
        });
    }
    FixtureSpec {
        name: "P20s".into(),
        seed,
        spaces: 5,
        repo_files: 200,
        repo_commits: 50,
        dirty_files: 30,
        chats,
        crash_residue: 0,
    }
}

/// bench.md 3.2 `P200`: sizes from a log-normal fitted to the real profile
/// (p50 176 KB, p90 4.4 MB), two 30 MB outliers, 30 spaces, crash residue in 20 chats.
/// `cap` bounds the log-normal tail so the fixture builds in minutes.
pub fn p200(seed: u64, chats: usize, cap: usize, outliers: usize) -> FixtureSpec {
    let mut rng = Rng::new(seed ^ 0x2000);
    let mut specs: Vec<ChatSpec> = (0..chats)
        .map(|i| {
            let bytes = rng.lognormal(176_000.0, 2.5).clamp(4_000.0, cap as f64) as usize;
            ChatSpec {
                bytes,
                space: i % 30,
                tag: "bg".into(),
            }
        })
        .collect();
    for spec in specs.iter_mut().take(outliers) {
        spec.bytes = 30_000_000;
        spec.tag = "outlier".into();
    }
    FixtureSpec {
        name: "P200".into(),
        seed,
        spaces: 30,
        repo_files: 30,
        repo_commits: 8,
        dirty_files: 0,
        chats: specs,
        crash_residue: 20,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatInfo {
    pub n: usize,
    pub id: String,
    pub chat8: String,
    pub space: usize,
    pub tag: String,
    pub bytes_target: usize,
    pub turns: usize,
    /// Number of transcript entries as the engine reports them.
    pub entries: usize,
    /// JSON bytes of the transcript entries (the full-reset payload).
    pub transcript_bytes: usize,
    /// Concatenated assistant text parts.
    pub text_bytes: usize,
    pub text_sha256: String,
    #[serde(default)]
    pub crash_residue: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: String,
    pub id: String,
    pub spec: FixtureSpec,
    pub engine_sha256: String,
    pub generation_s: f64,
    pub spaces: Vec<SpaceInfo>,
    pub chats: Vec<ChatInfo>,
    pub data_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct Fixture {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Fixture {
    pub fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    pub fn chat(&self, n: usize) -> &ChatInfo {
        &self.manifest.chats[n]
    }

    pub fn space_path(&self, chat: &ChatInfo) -> &Path {
        &self.manifest.spaces[chat.space].path
    }

    pub fn chats_tagged<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a ChatInfo> + 'a {
        self.manifest.chats.iter().filter(move |c| c.tag == tag)
    }
}

pub fn chat_uuid(n: usize) -> String {
    format!("b7{:06x}-0000-4000-8000-{:012x}", n + 1, n + 1)
}

pub fn space_uuid(n: usize) -> String {
    format!("b5{:06x}-0000-4000-8000-{:012x}", n + 1, n + 1)
}

pub fn message_uuid(chat: usize, seq: usize) -> String {
    format!("b8{:06x}-0000-4000-9000-{:012x}", chat + 1, seq)
}

/// SHA-256 of the assistant text parts, concatenated. Matches how zb-replay
/// hashes the text it plays.
pub fn text_of(view: &ChatView) -> String {
    let mut out = String::new();
    for entry in &view.entries {
        if entry.get("role").and_then(Value::as_str) == Some("assistant") {
            append_text_parts(entry, &mut out);
        }
    }
    out
}

/// Assistant text after the last user entry (the turn in flight or just finished).
pub fn last_turn_text(view: &ChatView) -> String {
    let start = view
        .entries
        .iter()
        .rposition(|e| e.get("role").and_then(Value::as_str) == Some("user"))
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut out = String::new();
    for entry in &view.entries[start..] {
        if entry.get("role").and_then(Value::as_str) == Some("assistant") {
            append_text_parts(entry, &mut out);
        }
    }
    out
}

fn append_text_parts(entry: &Value, out: &mut String) {
    for part in entry
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if part.get("kind").and_then(Value::as_str) == Some("text")
            && let Some(text) = part.get("text").and_then(Value::as_str)
        {
            out.push_str(text);
        }
    }
}

pub fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// True once nothing in `view` is still streaming and an assistant entry closes it.
pub fn turn_settled(view: &ChatView) -> bool {
    let Some(last) = view.entries.last() else {
        return false;
    };
    last.get("role").and_then(Value::as_str) == Some("assistant")
        && !view.entries.iter().any(|e| {
            e.get("role").and_then(Value::as_str) == Some("assistant")
                && e.get("status").and_then(Value::as_str) == Some("streaming")
        })
}

/// An assistant entry that is still streaming and already holds text.
pub fn streaming_with_text(view: &ChatView) -> bool {
    view.entries.iter().any(|e| {
        e.get("role").and_then(Value::as_str) == Some("assistant")
            && e.get("status").and_then(Value::as_str) == Some("streaming")
            && {
                let mut text = String::new();
                append_text_parts(e, &mut text);
                !text.is_empty()
            }
    })
}

pub fn bad_status(view: &ChatView) -> Option<String> {
    view.entries.iter().find_map(|e| {
        let status = e.get("status").and_then(Value::as_str)?;
        (e.get("role").and_then(Value::as_str) == Some("assistant")
            && matches!(status, "failed" | "aborted" | "error"))
        .then(|| format!("assistant {status}: {}", short(e)))
    })
}

fn short(value: &Value) -> String {
    let text = value.to_string();
    text.chars().take(400).collect()
}

/// Cached fixture for `spec`, building it with the engine at `env.fixture_engine` when missing.
pub async fn ensure(env: &RunEnv, spec: &FixtureSpec) -> Result<Arc<Fixture>> {
    let id = spec.id();
    let dir = env.zb_root.join("fixtures").join("b7").join(&id);
    if let Some(fx) = load(&dir)? {
        return Ok(Arc::new(fx));
    }
    eprintln!("zeron-bench: building fixture {id} (once; cached under {})", dir.display());
    let fx = build(env, spec, &id, &dir).await?;
    Ok(Arc::new(fx))
}

fn load(dir: &Path) -> Result<Option<Fixture>> {
    let path = dir.join("manifest.json");
    if !path.is_file() {
        return Ok(None);
    }
    let manifest: Manifest = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("parse {}", path.display()))?;
    ensure!(dir.join("data").is_dir(), "fixture {} has no data dir", dir.display());
    Ok(Some(Fixture {
        dir: dir.to_path_buf(),
        manifest,
    }))
}

/// Copy the pristine data dir into the trial (reflink where the filesystem allows).
pub fn install(fx: &Fixture, trial: &Trial) -> Result<()> {
    let status = Command::new("cp")
        .args(["-a", "--reflink=auto"])
        .arg(format!("{}/.", fx.data().display()))
        .arg(&trial.data)
        .status()
        .context("run cp")?;
    ensure!(status.success(), "cp of fixture data failed");
    // A fresh token and lock come with the new engine.
    for stale in ["ipc-token", "engine.lock"] {
        let _ = fs::remove_file(trial.data.join(stale));
    }
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=zeron-bench", "-c", "user.email=bench@example.invalid"])
        .args(["-c", "commit.gpgsign=false", "-c", "core.fsmonitor=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("run git")?;
    ensure!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}

fn make_repo(path: &Path, files: usize, commits: usize, dirty: usize, seed: u64) -> Result<()> {
    fs::create_dir_all(path)?;
    git(path, &["init", "-q", "-b", "main"])?;
    let mut rng = Rng::new(seed);
    let body = |rng: &mut Rng, i: usize| -> String {
        let mut s = format!("// file {i}\n");
        for line in 0..rng.range(6, 18) {
            s.push_str(&format!("pub fn f{i}_{line}(x: u64) -> u64 {{ x.wrapping_mul({}) }}\n", rng.range(3, 999)));
        }
        s
    };
    let per_commit = files.div_ceil(commits.max(1)).max(1);
    for c in 0..commits.max(1) {
        for i in (c * per_commit)..((c + 1) * per_commit).min(files) {
            let dir = path.join(format!("src/m{}", i % 12));
            fs::create_dir_all(&dir)?;
            fs::write(dir.join(format!("f{i}.rs")), body(&mut rng, i))?;
        }
        git(path, &["add", "-A"])?;
        git(path, &["commit", "-q", "--allow-empty", "-m", &format!("commit {c}")])?;
    }
    for i in 0..dirty {
        let rel = if i % 6 == 0 {
            path.join(format!("untracked{i}.txt"))
        } else {
            path.join(format!("src/m{}/f{}.rs", i % 12, i))
        };
        let mut text = fs::read_to_string(&rel).unwrap_or_default();
        text.push_str("// dirty\n");
        fs::write(rel, text)?;
    }
    Ok(())
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    for ent in rd.flatten() {
        let Ok(kind) = ent.file_type() else {
            continue;
        };
        if kind.is_dir() {
            total += dir_bytes(&ent.path());
        } else if let Ok(meta) = ent.metadata() {
            total += meta.len();
        }
    }
    total
}

struct Build<'a> {
    spec: &'a FixtureSpec,
    scripts: PathBuf,
}

impl Build<'_> {
    fn chat_turns(&self, bytes: usize) -> usize {
        bytes.div_ceil(100_000).max(1)
    }
}

pub(super) async fn queue_play(
    client: &Client,
    chat: &str,
    cwd: &Path,
    message: &str,
    script: &Path,
    speed: f64,
) -> Result<()> {
    ensure!(
        !script.to_string_lossy().contains(char::is_whitespace),
        "script path has whitespace"
    );
    client
        .call(
            "QueueCommand",
            json!({"chatId":chat,"command":{"kind":"run","messageId":message,"request":{
                "prompt":format!("zb:play {} speed={speed}", script.display()),
                "model":"haiku","reasoning":null,"modelOptions":{},"cwd":cwd,
                "sandbox":"workspace-write","autoApprove":true,"resume":null}}}),
        )
        .await?;
    Ok(())
}

/// Start a trial engine that serves `fixture-style` work (also used for the crash phase).
fn fixture_trial(env: &RunEnv, label: &str, index: u32) -> Result<Trial> {
    let mut trial = Trial::at(&env.zb_root, label, index, "fx")?;
    trial.replay_bin = env.replay_bin.clone();
    trial.bench_bin = env.bench_bin.clone();
    trial.probe_sock = env.short_sock(label, index)?;
    trial.extra.push(("ZERON_HARNESS".into(), "claude-code".into()));
    trial.extra.push((
        "ZB_REPLAY_STATE_DIR".into(),
        trial.root.join("replay-state").into_os_string(),
    ));
    trial.allocate_port()?;
    trial.ready_timeout = Duration::from_secs(180);
    Ok(trial)
}

async fn build(env: &RunEnv, spec: &FixtureSpec, id: &str, dir: &Path) -> Result<Fixture> {
    let started = std::time::Instant::now();
    if dir.exists() {
        fs::remove_dir_all(dir).with_context(|| format!("clear partial {}", dir.display()))?;
    }
    fs::create_dir_all(dir.join("projects"))?;
    let plan = Build {
        spec,
        scripts: dir.join("scripts"),
    };
    fs::create_dir_all(&plan.scripts)?;
    let mut spaces = Vec::new();
    for s in 0..spec.spaces {
        let path = dir.join("projects").join(format!("p{s}"));
        make_repo(
            &path,
            spec.repo_files,
            spec.repo_commits,
            if s == 0 { spec.dirty_files } else { 0 },
            spec.seed ^ ((s as u64) << 8),
        )?;
        spaces.push(SpaceInfo {
            id: space_uuid(s),
            path,
        });
    }

    let label = format!("fixture-{id}");
    let trial = fixture_trial(env, &label, 0)?;
    let engine = launch(&env.fixture_engine, &trial).await?;
    let client = Client::connect(trial.port, &trial.data).await?;
    let (device, _) = client.call("LocalDevice", json!({})).await?;
    for (s, space) in spaces.iter().enumerate() {
        client
            .call(
                "Mutate",
                json!({"op":"createSpace","spaceId":space.id,"deviceId":device["deviceId"],"path":space.path}),
            )
            .await
            .with_context(|| format!("createSpace {s}"))?;
    }
    let mut chat_ids = Vec::new();
    for (n, chat) in spec.chats.iter().enumerate() {
        let id = chat_uuid(n);
        client
            .call(
                "Mutate",
                json!({"op":"createChat","chatId":id,"spaceId":spaces[chat.space].id,
                    "config":{"harness":"claude-code","model":"haiku","reasoning":null,"sandbox":"workspace-write"}}),
            )
            .await
            .with_context(|| format!("createChat {n}"))?;
        client
            .call(
                "Mutate",
                json!({"op":"renameChat","chatId":id,"title":format!("{} {}", chat.tag, n + 1)}),
            )
            .await?;
        chat_ids.push(id);
    }

    let jobs = spec
        .chats
        .iter()
        .enumerate()
        .map(|(n, chat)| {
            let client = client.clone();
            let cwd = spaces[chat.space].path.clone();
            let plan = &plan;
            async move { build_chat(plan, &client, n, chat, &cwd).await }
        })
        .collect::<Vec<_>>();
    let mut infos: Vec<ChatInfo> = Vec::new();
    let mut stream = futures::stream::iter(jobs).buffer_unordered(6);
    while let Some(info) = stream.next().await {
        infos.push(info?);
    }
    drop(stream);
    infos.sort_by_key(|c| c.n);
    drop(client);
    let report = engine.stop().await?;
    ensure!(
        report.stop_engine_answered,
        "fixture engine did not stop through StopEngine"
    );

    // Move the frozen data dir into place, then (optionally) leave crash residue.
    let data = dir.join("data");
    fs::rename(&trial.data, &data).context("freeze fixture data dir")?;
    if spec.crash_residue > 0 {
        crash_residue(env, spec, id, dir, &spaces, &mut infos)
            .await
            .context("crash residue phase")?;
    }
    let engine_bytes = fs::read(&env.fixture_engine).context("read fixture engine")?;
    let manifest = Manifest {
        schema: "zeron-bench/fixture-b7/v1".into(),
        id: id.to_string(),
        spec: spec.clone(),
        engine_sha256: format!("{:x}", Sha256::digest(&engine_bytes)),
        generation_s: started.elapsed().as_secs_f64(),
        spaces,
        chats: infos,
        data_bytes: dir_bytes(&data),
    };
    let _ = fs::remove_dir_all(&plan.scripts);
    let _ = fs::remove_dir_all(trial.root.join("replay-state"));
    fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    eprintln!(
        "zeron-bench: fixture {id} ready in {:.0}s ({} chats, {:.1} MB data)",
        manifest.generation_s,
        manifest.chats.len(),
        manifest.data_bytes as f64 / 1e6
    );
    Ok(Fixture {
        dir: dir.to_path_buf(),
        manifest,
    })
}

async fn build_chat(
    plan: &Build<'_>,
    client: &Client,
    n: usize,
    chat: &ChatSpec,
    cwd: &Path,
) -> Result<ChatInfo> {
    let id = chat_uuid(n);
    let chat8 = id[..8].to_string();
    let mut sub = client
        .subscribe("WatchDocMessages", json!({"chatId":id,"openingTail":true}))
        .await?;
    let mut view = ChatView::default();
    let turns = if chat.bytes == 0 { 0 } else { plan.chat_turns(chat.bytes) };
    let mut expected = String::new();
    for turn in 0..turns {
        let built = build_turn(&TurnOpts {
            seed: plan.spec.seed ^ ((n as u64) << 24) ^ turn as u64,
            chat8: chat8.clone(),
            payload_bytes: chat.bytes / turns,
            min_ms: 0,
            cadence: Cadence::burst(),
            marker_every: 0,
            tools: true,
            thinking: true,
        });
        let script = plan.scripts.join(format!("c{n}t{turn}.zbr.jsonl"));
        fs::write(&script, &built.source)?;
        expected.push_str(&built.text);
        queue_play(client, &id, cwd, &message_uuid(n, turn), &script, 0.0).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        loop {
            let item = tokio::time::timeout_at(deadline, sub.recv())
                .await
                .with_context(|| format!("chat {n} turn {turn} timed out"))?
                .context("transcript stream closed")??;
            view.apply(&item.value)?;
            if let Some(bad) = bad_status(&view) {
                bail!("chat {n}: {bad}");
            }
            if turn_settled(&view) && text_of(&view).len() >= expected.len() {
                break;
            }
        }
    }
    let text = text_of(&view);
    ensure!(
        text == expected,
        "chat {n}: engine transcript text ({} B) differs from the played scripts ({} B)",
        text.len(),
        expected.len()
    );
    Ok(ChatInfo {
        n,
        id,
        chat8,
        space: chat.space,
        tag: chat.tag.clone(),
        bytes_target: chat.bytes,
        turns,
        entries: view.entries.len(),
        transcript_bytes: serde_json::to_vec(&view.entries)?.len(),
        text_bytes: text.len(),
        text_sha256: sha256_hex(&text),
        crash_residue: false,
    })
}

/// Reboot the frozen profile, start a slow turn in `spec.crash_residue` chats,
/// wait until each streams, then SIGKILL the engine (unfinished journals).
async fn crash_residue(
    env: &RunEnv,
    spec: &FixtureSpec,
    id: &str,
    dir: &Path,
    spaces: &[SpaceInfo],
    infos: &mut [ChatInfo],
) -> Result<()> {
    let label = format!("fixture-{id}");
    let trial = fixture_trial(env, &label, 1)?;
    trial.prepare()?;
    let data = dir.join("data");
    fs::remove_dir_all(&trial.data)?;
    fs::rename(&data, &trial.data)?;
    let result = crash_inner(env, spec, dir, spaces, infos, &trial).await;
    // Whatever happened, the data dir goes back into the fixture.
    let _ = fs::remove_file(trial.data.join("engine.lock"));
    fs::rename(&trial.data, &data).context("return fixture data dir")?;
    result
}

async fn crash_inner(
    env: &RunEnv,
    spec: &FixtureSpec,
    dir: &Path,
    spaces: &[SpaceInfo],
    infos: &mut [ChatInfo],
    trial: &Trial,
) -> Result<()> {
    let slow = build_turn(&TurnOpts {
        seed: spec.seed ^ 0x5105,
        chat8: "00000000".into(),
        payload_bytes: 1,
        min_ms: 600_000,
        cadence: Cadence::opus(),
        marker_every: 0,
        tools: false,
        thinking: false,
    });
    let script = dir.join("slow.zbr.jsonl");
    fs::write(&script, &slow.source)?;
    let engine = launch(&env.fixture_engine, trial).await?;
    let client = Client::connect(trial.port, &trial.data).await?;
    // Victims: the smallest non-empty chats, so their streams start quickly.
    let mut order: Vec<usize> = infos
        .iter()
        .filter(|c| c.turns > 0)
        .map(|c| c.n)
        .collect();
    order.sort_by_key(|n| infos[*n].transcript_bytes);
    let victims: Vec<usize> = order.into_iter().take(spec.crash_residue).collect();
    let mut subs = Vec::new();
    for n in &victims {
        let chat = chat_uuid(*n);
        let sub = client
            .subscribe("WatchDocMessages", json!({"chatId":chat,"openingTail":true}))
            .await?;
        let cwd = spaces[infos[*n].space].path.clone();
        queue_play(&client, &chat, &cwd, &message_uuid(*n, 9000), &script, 1.0).await?;
        subs.push((*n, sub, ChatView::default()));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    for (n, sub, view) in &mut subs {
        loop {
            let item = tokio::time::timeout_at(deadline, sub.recv())
                .await
                .with_context(|| format!("victim {n} never streamed"))?
                .context("victim stream closed")??;
            view.apply(&item.value)?;
            let streaming = streaming_with_text(view);
            if streaming {
                break;
            }
        }
    }
    // Let a snapshot tick and the journals land, then pull the plug.
    tokio::time::sleep(Duration::from_secs(4)).await;
    drop(subs);
    drop(client);
    drop(engine); // Drop kills the scope: no StopEngine, no clean shutdown.
    tokio::time::sleep(Duration::from_millis(500)).await;
    for n in &victims {
        infos[*n].crash_residue = true;
    }
    let _ = fs::remove_file(&script);
    Ok(())
}

pub fn chat_map(fx: &Fixture) -> BTreeMap<String, &ChatInfo> {
    fx.manifest.chats.iter().map(|c| (c.id.clone(), c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_ids_are_stable_and_sizes_parse() {
        let a = p20("P20-t", 1, 4, 1_000_000);
        let b = p20("P20-t", 1, 4, 1_000_000);
        assert_eq!(a.id(), b.id());
        assert_ne!(a.id(), p20("P20-t", 2, 4, 1_000_000).id());
        assert_eq!(a.chats.len(), 20 + 4);
        assert_eq!(parse_size("5m").unwrap(), 5_000_000);
        assert_eq!(parse_size("100k").unwrap(), 100_000);
    }

    #[test]
    fn p200_has_the_planned_shape() {
        let spec = p200(7, 200, 6_000_000, 2);
        assert_eq!(spec.chats.len(), 200);
        assert_eq!(spec.spaces, 30);
        assert_eq!(spec.chats.iter().filter(|c| c.tag == "outlier").count(), 2);
        assert!(spec.chats.iter().filter(|c| c.bytes <= 176_000).count() > 60);
    }

    #[test]
    fn text_helpers_follow_the_last_user_entry() {
        let view = ChatView {
            entries: vec![
                json!({"id":"u1","role":"user","parts":[{"kind":"text","text":"hi"}]}),
                json!({"id":"a1","role":"assistant","status":"complete","parts":[{"kind":"text","text":"one"},{"kind":"reasoning","text":"x"}]}),
                json!({"id":"u2","role":"user","parts":[]}),
                json!({"id":"a2","role":"assistant","status":"complete","parts":[{"kind":"text","text":"two"}]}),
            ],
            history_pending: false,
        };
        assert_eq!(text_of(&view), "onetwo");
        assert_eq!(last_turn_text(&view), "two");
        assert!(turn_settled(&view));
    }
}
