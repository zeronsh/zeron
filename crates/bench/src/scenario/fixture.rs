//! Scenario view of the B4 data-dir fixtures (`crate::fixture`, bench.md 3.2).
//!
//! `P20` and `P200` are built by `zeron-bench fixture gen` with an engine of the
//! pinned fixture revision (the older side of a pair) and live under
//! `ZB_ROOT/fixtures/<NAME>/<seed>-<stamp>/`. A cached fixture is reused when
//! its manifest's `fixture_revision` matches that revision; otherwise it is
//! generated here with the same API. Layout and naming come from
//! `fixture::multi`: chat `n` is `b4000002-0000-4000-8000-<n:012x>`, space `s`
//! lives at `repo-<s % repos>/space-<s>`, and `turn-<n>.zbr.jsonl` is the
//! script that built chat `n`.
//!
//! The fixtures carry no Claude session state, so the engine cannot `--resume`
//! a chat's old session. The first turn on a chat reports "previous session
//! could not be restored" and continues in a new one. Scenarios therefore warm
//! each chat they stream with a `zb:echo` turn first, and look for error parts
//! only in the turn they measure.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::RunEnv;
use crate::Trial;
use crate::rpc::Client;
use crate::transcript::ChatView;

/// Seed of the shared fixtures.
pub const FIXTURE_SEED: u64 = 42;

/// Which B4 profile a scenario needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    P20,
    P200,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Self::P20 => "P20",
            Self::P200 => "P200",
        }
    }

    fn spaces(self) -> usize {
        match self {
            Self::P20 => 5,
            Self::P200 => 30,
        }
    }

    fn repos(self) -> usize {
        match self {
            Self::P20 => 5,
            Self::P200 => 10,
        }
    }

    fn chats(self) -> usize {
        match self {
            Self::P20 => 20,
            Self::P200 => 200,
        }
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

/// Size class of a chat by its transcript JSON bytes.
pub fn class_of(bytes: usize) -> &'static str {
    match bytes {
        0..=2 => "empty",
        3..=300_000 => "100k",
        300_001..=2_000_000 => "1m",
        2_000_001..=9_000_000 => "5m",
        _ => "big",
    }
}

#[derive(Clone, Debug)]
pub struct ChatInfo {
    pub n: usize,
    pub id: String,
    /// Marker prefix zb-replay renders: the last eight characters of the id,
    /// because the fixture ids share their first group.
    pub chat8: String,
    pub space: usize,
    pub class: &'static str,
    /// JSON bytes of the transcript entries (the full-reset payload).
    pub transcript_bytes: usize,
    /// Played assistant text (bytes, sha256) when the build script covers the whole text.
    pub text: Option<(usize, String)>,
    pub crash_residue: bool,
    pub shutdown_residue: bool,
}

#[derive(Clone, Debug)]
pub struct Fixture {
    pub dir: PathBuf,
    pub profile: Profile,
    /// `P20-<seed>-<stamp>`, recorded in every trial.
    pub id: String,
    pub fixture_revision: Option<String>,
    pub chats: Vec<ChatInfo>,
}

impl Fixture {
    pub fn data(&self) -> PathBuf {
        self.dir.join("data")
    }

    pub fn space_path(&self, chat: &ChatInfo) -> PathBuf {
        self.dir
            .join(format!("repo-{}", chat.space % self.profile.repos()))
            .join(format!("space-{}", chat.space))
    }

    /// Chats of a size class in fixture order. `plain` drops the chats whose
    /// played text is not fully known (the two subagent chats of P20).
    pub fn class(&self, class: &str, plain: bool) -> Vec<ChatInfo> {
        self.chats
            .iter()
            .filter(|c| c.class == class && (!plain || c.text.is_some()))
            .cloned()
            .collect()
    }
}

pub fn chat_uuid(n: usize) -> String {
    format!("b4000002-0000-4000-8000-{n:012x}")
}

/// Message ids that cannot collide with the fixture builder's `b4000003`..`5`.
pub fn message_uuid(chat: usize, seq: usize) -> String {
    format!("b7{:06x}-0000-4000-9000-{:012x}", chat + 1, seq)
}

fn script_text(dir: &Path, n: usize) -> Option<(usize, String)> {
    let text = fs::read_to_string(dir.join(format!("turn-{n}.zbr.jsonl"))).ok()?;
    let meta: Value = serde_json::from_str(text.lines().next()?).ok()?;
    Some((
        meta["text_bytes"].as_u64()? as usize,
        meta["text_sha256"].as_str()?.to_string(),
    ))
}

fn load(dir: &Path, profile: Profile) -> Result<Option<Fixture>> {
    let path = dir.join("manifest.json");
    if !path.is_file() || !dir.join("data").is_dir() {
        return Ok(None);
    }
    let manifest: Value = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("parse {}", path.display()))?;
    let sizes = manifest["transcript_bytes"]
        .as_array()
        .context("manifest has no transcript_bytes")?;
    let list = |key: &str| -> Vec<String> {
        manifest[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let crash = list("crash_chats");
    let shutdown = list("shutdown_chats");
    let chats = sizes
        .iter()
        .enumerate()
        .map(|(n, size)| {
            let id = chat_uuid(n);
            let bytes = size.as_u64().unwrap_or(0) as usize;
            // Chats 0 and 1 of P20 also played the subagent script, whose text
            // the build script's hash does not cover. Crash chats were replaced.
            let plain = bytes > 2
                && !(profile == Profile::P20 && n < 2)
                && !crash.contains(&id)
                && !shutdown.contains(&id);
            ChatInfo {
                n,
                chat8: id[id.len() - 8..].to_string(),
                crash_residue: crash.contains(&id),
                shutdown_residue: shutdown.contains(&id),
                text: if plain { script_text(dir, n) } else { None },
                id,
                space: n % profile.spaces(),
                class: class_of(bytes),
                transcript_bytes: bytes,
            }
        })
        .collect();
    let name = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Some(Fixture {
        dir: dir.to_path_buf(),
        profile,
        id: format!("{}-{name}", profile.name()),
        fixture_revision: manifest["fixture_revision"].as_str().map(str::to_owned),
        chats,
    }))
}

/// The newest cached fixture of `profile` generated by the fixture revision.
fn find_cached(env: &RunEnv, profile: Profile) -> Result<Option<Fixture>> {
    let root = env.zb_root.join("fixtures").join(profile.name());
    let Ok(rd) = fs::read_dir(&root) else {
        return Ok(None);
    };
    let mut dirs: Vec<(u64, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stamp = name.rsplit('-').next()?.parse().ok()?;
            Some((stamp, e.path()))
        })
        .collect();
    dirs.sort();
    for (_, dir) in dirs.into_iter().rev() {
        if let Some(fx) = load(&dir, profile)?
            && fx.fixture_revision.as_deref() == Some(env.fixture_rev.as_str())
            && fx.chats.len() == profile.chats()
        {
            return Ok(Some(fx));
        }
    }
    Ok(None)
}

/// Cached fixture, or one generated through B4 with the engine of the fixture revision.
pub fn resolve(env: &RunEnv, profile: Profile) -> Result<Arc<Fixture>> {
    if let Some(fx) = find_cached(env, profile)? {
        return Ok(Arc::new(fx));
    }
    eprintln!(
        "zeron-bench: generating fixture {} at revision {} (once)",
        profile.name(),
        &env.fixture_rev[..env.fixture_rev.len().min(12)]
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let dir = runtime.block_on(crate::fixture::multi::generate(
        profile.name(),
        &env.fixture_engine,
        FIXTURE_SEED,
        Some(&env.fixture_rev),
    ))?;
    runtime.shutdown_background();
    let Some(fx) = load(&dir, profile)? else {
        bail!("fixture {} was generated without a manifest", dir.display());
    };
    Ok(Arc::new(fx))
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

/// Assistant text parts, concatenated (how zb-replay hashes the text it plays).
pub fn text_of(view: &ChatView) -> String {
    let mut out = String::new();
    for entry in &view.entries {
        if entry.get("role").and_then(Value::as_str) == Some("assistant") {
            append_text_parts(entry, &mut out);
        }
    }
    out
}

fn last_user(view: &ChatView) -> usize {
    view.entries
        .iter()
        .rposition(|e| e.get("role").and_then(Value::as_str) == Some("user"))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// Assistant text after the last user entry (the turn in flight or just finished).
pub fn last_turn_text(view: &ChatView) -> String {
    let mut out = String::new();
    for entry in &view.entries[last_user(view)..] {
        if entry.get("role").and_then(Value::as_str) == Some("assistant") {
            append_text_parts(entry, &mut out);
        }
    }
    out
}

/// An `error` part in the entries after the last user entry.
pub fn last_turn_has_error_part(view: &ChatView) -> bool {
    view.entries[last_user(view)..].iter().any(|e| {
        e.get("parts").and_then(Value::as_array).is_some_and(|parts| {
            parts
                .iter()
                .any(|p| p.get("kind").and_then(Value::as_str) == Some("error"))
        })
    })
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

/// The first assistant entry that ended `failed`, `aborted` or `error`.
pub fn bad_status(view: &ChatView) -> Option<String> {
    view.entries.iter().find_map(|e| {
        let status = e.get("status").and_then(Value::as_str)?;
        (e.get("role").and_then(Value::as_str) == Some("assistant")
            && matches!(status, "failed" | "aborted" | "error"))
        .then(|| {
            let text = e.to_string();
            format!("assistant {status}: {}", &text[..text.len().min(400)])
        })
    })
}

/// Does a freshly opened chat match its manifest? Size always; the played
/// text hash too where the build script covers it.
pub fn base_matches(chat: &ChatInfo, view: &ChatView) -> (bool, String) {
    let bytes = serde_json::to_vec(&view.entries)
        .map(|v| v.len())
        .unwrap_or(0);
    let tolerance = (chat.transcript_bytes / 200).max(64);
    if bytes.abs_diff(chat.transcript_bytes) > tolerance {
        return (
            false,
            format!(
                "transcript is {bytes} B, manifest says {} B",
                chat.transcript_bytes
            ),
        );
    }
    if let Some((len, sha)) = &chat.text {
        let text = text_of(view);
        if text.len() != *len || &sha256_hex(&text) != sha {
            return (
                false,
                format!("text is {} B (script {len} B) or its hash differs", text.len()),
            );
        }
    }
    (true, String::new())
}

/// Queue one `run` command with a literal prompt.
pub(super) async fn queue_prompt(
    client: &Client,
    chat: &str,
    cwd: &Path,
    message: &str,
    prompt: &str,
) -> Result<()> {
    client
        .call(
            "QueueCommand",
            json!({"chatId":chat,"command":{"kind":"run","messageId":message,"request":{
                "prompt":prompt,
                "model":"haiku","reasoning":null,"modelOptions":{},"cwd":cwd,
                "sandbox":"workspace-write","autoApprove":true,"resume":null}}}),
        )
        .await?;
    Ok(())
}

/// Queue `zb:play <script> speed=<speed>`.
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
    queue_prompt(
        client,
        chat,
        cwd,
        message,
        &format!("zb:play {} speed={speed}", script.display()),
    )
    .await
}

/// A warm-up turn (`zb:echo ready`) starts the chat's session and adds this text.
pub const WARM_PROMPT: &str = "zb:echo ready";
pub const WARM_TEXT: &str = "ready";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_classes_and_ids() {
        assert_eq!(parse_size("5m").unwrap(), 5_000_000);
        assert_eq!(parse_size("100k").unwrap(), 100_000);
        assert_eq!(class_of(2), "empty");
        assert_eq!(class_of(104_396), "100k");
        assert_eq!(class_of(1_056_533), "1m");
        assert_eq!(class_of(5_289_154), "5m");
        assert_eq!(class_of(31_742_450), "big");
        let id = chat_uuid(0xb4);
        assert!(id.starts_with("b4000002-") && id.ends_with("0000000000b4"));
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
        assert!(!last_turn_has_error_part(&view));
    }
}
