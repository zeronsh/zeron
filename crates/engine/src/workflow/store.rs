//! On-disk state of workflow runs: one directory per run under the device's
//! store root, holding the authoritative journal (JSONL), the script, a small
//! `meta.json` and the artifacts. Nothing here is synced; clients get
//! previews through the chat doc and fetch full content over RPC.
//!
//! ```text
//! workflows/<run_id>/
//!   meta.json            RunMeta: identity, options, status, delivery flag
//!   script.star          the approved script (hash-pinned by meta)
//!   journal.jsonl        one Record per line, append-only
//!   artifacts/<id>/
//!     index.json         versions: kind, title, content type, bytes
//!     v<N>.md|json|<ext> the content of each version
//! ```

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeron_proto::{
    ArtifactKind, WorkflowBudgets, WorkflowEvent, WorkflowStatus, WorkflowStopReason,
};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("not a valid {what}: {value:?}")]
    BadName { what: &'static str, value: String },
    #[error("no such run: {0}")]
    NoRun(String),
    #[error("no such artifact: {0}")]
    NoArtifact(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

/// Run ids are engine-minted but arrive over RPC: only a safe alphabet may
/// ever become a path component.
pub fn safe_component(what: &'static str, value: &str) -> Result<(), StoreError> {
    let ok = !value.is_empty()
        && value.len() <= 96
        && !value.starts_with('.')
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(StoreError::BadName {
            what,
            value: value.to_owned(),
        })
    }
}

/// What a run was started with; resumed runs must match it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunOptions {
    pub max_concurrency: u32,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub budgets: WorkflowBudgets,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RunMeta {
    pub run_id: String,
    pub chat_id: String,
    pub name: String,
    pub script_hash: String,
    pub args_hash: String,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub options: RunOptions,
    /// The project folder the run's world reads and commands are confined to.
    #[serde(default)]
    pub project_root: String,
    #[serde(default)]
    pub draft_path: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub status: WorkflowStatus,
    #[serde(default)]
    pub stop_reason: Option<WorkflowStopReason>,
    #[serde(default)]
    pub resumed_from: Option<String>,
    /// The completion message was queued into the parent chat.
    #[serde(default)]
    pub delivered: bool,
    /// Highest event sequence journaled (resumes continue from it).
    #[serde(default)]
    pub last_seq: u64,
}

/// One journal line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rec", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Record {
    Event(WorkflowEvent),
    /// A settled ask, in full: what a replay answers the script with.
    AskDone {
        key: String,
        /// Hash of the instructions (+ schema): a replay refuses a mismatch.
        digest: String,
        ok: bool,
        value: Value,
        error: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        child_chat_id: Option<String>,
    },
    RunDone {
        key: String,
        digest: String,
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
        timed_out: bool,
        truncated: bool,
        start_error: Option<String>,
    },
    ReadDone {
        key: String,
        digest: String,
        value: Value,
    },
    /// A `report()` item in full (events carry previews).
    Report {
        index: u32,
        item: Value,
        artifact_id: Option<String>,
    },
    Log {
        line: String,
    },
    /// `main`'s return value, in full.
    Result {
        value: Value,
    },
}

/// Append-only writer; one line per record, flushed per record.
pub struct Journal {
    file: Mutex<File>,
}

impl Journal {
    pub fn append(&self, record: &Record) -> Result<(), StoreError> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        let mut file = lock(&self.file);
        file.write_all(&line)?;
        file.flush()?;
        Ok(())
    }
}

/// A journal read back: everything a resume or a `get_workflow_run` needs.
#[derive(Debug, Default)]
pub struct Replay {
    pub events: Vec<WorkflowEvent>,
    pub asks: HashMap<String, Record>,
    pub runs: HashMap<String, Record>,
    pub reads: HashMap<String, Record>,
    pub reports: Vec<(u32, Value, Option<String>)>,
    pub logs: Vec<String>,
    pub result: Option<Value>,
    pub last_seq: u64,
}

impl Replay {
    fn absorb(&mut self, record: Record) {
        match record {
            Record::Event(e) => {
                self.last_seq = self.last_seq.max(e.seq);
                self.events.push(e);
            }
            r @ Record::AskDone { .. } => {
                if let Record::AskDone { key, .. } = &r {
                    self.asks.insert(key.clone(), r.clone());
                }
            }
            r @ Record::RunDone { .. } => {
                if let Record::RunDone { key, .. } = &r {
                    self.runs.insert(key.clone(), r.clone());
                }
            }
            r @ Record::ReadDone { .. } => {
                if let Record::ReadDone { key, .. } = &r {
                    self.reads.insert(key.clone(), r.clone());
                }
            }
            Record::Report {
                index,
                item,
                artifact_id,
            } => self.reports.push((index, item, artifact_id)),
            Record::Log { line } => self.logs.push(line),
            Record::Result { value } => self.result = Some(value),
        }
    }
}

/// Artifact index of one id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactIndex {
    pub versions: Vec<ArtifactVersion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactVersion {
    pub version: u32,
    pub kind: ArtifactKind,
    pub title: String,
    pub content_type: String,
    pub bytes: u64,
    #[serde(default)]
    pub item_count: u32,
    /// File name of the content under `artifacts/<id>/`.
    pub file: String,
    pub at: i64,
}

/// A page of artifact content.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtifactChunk {
    pub version: ArtifactVersion,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub total: u64,
}

#[derive(Debug, Clone)]
pub struct WorkflowStore {
    root: PathBuf,
}

impl WorkflowStore {
    pub fn open(store_root: &Path) -> Self {
        Self {
            root: store_root.join("workflows"),
        }
    }

    /// The store root this store was opened on (its parent of `workflows/`).
    pub fn base(&self) -> PathBuf {
        self.root
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    fn run_dir(&self, run_id: &str) -> Result<PathBuf, StoreError> {
        safe_component("run id", run_id)?;
        Ok(self.root.join(run_id))
    }

    fn existing_run_dir(&self, run_id: &str) -> Result<PathBuf, StoreError> {
        let dir = self.run_dir(run_id)?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(StoreError::NoRun(run_id.to_owned()))
        }
    }

    pub fn create_run(&self, meta: &RunMeta, script: &str) -> Result<(), StoreError> {
        let dir = self.run_dir(&meta.run_id)?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("script.star"), script)?;
        self.write_meta(meta)
    }

    pub fn write_meta(&self, meta: &RunMeta) -> Result<(), StoreError> {
        let dir = self.run_dir(&meta.run_id)?;
        let tmp = dir.join("meta.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(meta)?)?;
        std::fs::rename(tmp, dir.join("meta.json"))?;
        Ok(())
    }

    pub fn read_meta(&self, run_id: &str) -> Result<RunMeta, StoreError> {
        let dir = self.existing_run_dir(run_id)?;
        Ok(serde_json::from_slice(&std::fs::read(
            dir.join("meta.json"),
        )?)?)
    }

    pub fn read_script(&self, run_id: &str) -> Result<String, StoreError> {
        let dir = self.existing_run_dir(run_id)?;
        Ok(std::fs::read_to_string(dir.join("script.star"))?)
    }

    /// Every run on this device (unreadable directories are skipped).
    pub fn list_metas(&self) -> Vec<RunMeta> {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut metas: Vec<RunMeta> = entries
            .flatten()
            .filter_map(|e| {
                let id = e.file_name().to_string_lossy().into_owned();
                self.read_meta(&id).ok()
            })
            .collect();
        metas.sort_by_key(|m| (m.created_at, m.run_id.clone()));
        metas
    }

    pub fn open_journal(&self, run_id: &str) -> Result<Journal, StoreError> {
        let dir = self.existing_run_dir(run_id)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("journal.jsonl"))?;
        Ok(Journal {
            file: Mutex::new(file),
        })
    }

    /// Read the journal. A torn final line (a crash mid-write) is ignored;
    /// an unreadable line elsewhere is skipped, never fatal.
    pub fn load_replay(&self, run_id: &str) -> Result<Replay, StoreError> {
        let dir = self.existing_run_dir(run_id)?;
        let mut replay = Replay::default();
        let Ok(file) = File::open(dir.join("journal.jsonl")) else {
            return Ok(replay);
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Record>(&line) {
                Ok(record) => replay.absorb(record),
                Err(err) => {
                    tracing::warn!(run = %run_id, error = %err, "skipping an unreadable journal line")
                }
            }
        }
        Ok(replay)
    }

    // ── artifacts ──────────────────────────────────────────────────────────

    fn artifact_dir(&self, run_id: &str, id: &str) -> Result<PathBuf, StoreError> {
        safe_component("artifact id", id)?;
        Ok(self.existing_run_dir(run_id)?.join("artifacts").join(id))
    }

    pub fn artifact_index(&self, run_id: &str, id: &str) -> Result<ArtifactIndex, StoreError> {
        let dir = self.artifact_dir(run_id, id)?;
        match std::fs::read(dir.join("index.json")) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ArtifactIndex::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Store one version's content.
    #[allow(clippy::too_many_arguments)]
    pub fn put_artifact(
        &self,
        run_id: &str,
        id: &str,
        version: u32,
        kind: ArtifactKind,
        title: &str,
        content_type: &str,
        extension: &str,
        item_count: u32,
        bytes: &[u8],
        at: i64,
    ) -> Result<ArtifactVersion, StoreError> {
        safe_component("file extension", extension)?;
        let dir = self.artifact_dir(run_id, id)?;
        std::fs::create_dir_all(&dir)?;
        let file = format!("v{version}.{extension}");
        std::fs::write(dir.join(&file), bytes)?;
        let entry = ArtifactVersion {
            version,
            kind,
            title: title.to_owned(),
            content_type: content_type.to_owned(),
            bytes: bytes.len() as u64,
            item_count,
            file,
            at,
        };
        let mut index = self.artifact_index(run_id, id)?;
        index.versions.retain(|v| v.version != version);
        index.versions.push(entry.clone());
        index.versions.sort_by_key(|v| v.version);
        std::fs::write(dir.join("index.json"), serde_json::to_vec(&index)?)?;
        Ok(entry)
    }

    /// Ids that have content, sorted.
    pub fn artifact_ids(&self, run_id: &str) -> Result<Vec<String>, StoreError> {
        let dir = self.existing_run_dir(run_id)?.join("artifacts");
        let mut ids: Vec<String> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        ids.sort();
        Ok(ids)
    }

    /// Read `limit` bytes of an artifact version from `offset`. `version`
    /// `None` is the latest. The file is resolved from the *index*, never
    /// from caller text, and must stay inside the run's directory.
    pub fn read_artifact(
        &self,
        run_id: &str,
        id: &str,
        version: Option<u32>,
        offset: u64,
        limit: u64,
    ) -> Result<ArtifactChunk, StoreError> {
        let dir = self.artifact_dir(run_id, id)?;
        let index = self.artifact_index(run_id, id)?;
        let wanted = match version {
            Some(v) => index.versions.iter().find(|x| x.version == v),
            None => index.versions.last(),
        }
        .ok_or_else(|| StoreError::NoArtifact(id.to_owned()))?
        .clone();
        // The index is our own file, but defend anyway: a single plain name.
        let name = Path::new(&wanted.file);
        if name.components().count() != 1
            || !matches!(name.components().next(), Some(Component::Normal(_)))
        {
            return Err(StoreError::BadName {
                what: "artifact file",
                value: wanted.file.clone(),
            });
        }
        let path = dir.join(name);
        let canonical = path.canonicalize()?;
        let run_root = self.existing_run_dir(run_id)?.canonicalize()?;
        if !canonical.starts_with(&run_root) {
            return Err(StoreError::BadName {
                what: "artifact path",
                value: path.display().to_string(),
            });
        }
        let mut file = File::open(&canonical)?;
        let total = file.metadata()?.len();
        file.seek(SeekFrom::Start(offset.min(total)))?;
        let mut bytes = Vec::new();
        file.take(limit.min(1 << 20)).read_to_end(&mut bytes)?;
        Ok(ArtifactChunk {
            version: wanted,
            offset: offset.min(total),
            bytes,
            total,
        })
    }
}

/// Resolve a project-relative path under `root`, refusing anything that
/// leaves it (`..`, absolute paths, symlinks pointing out). Returns the
/// canonical path of an existing file or directory.
pub fn confine(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
    {
        return Err(format!("{relative:?} is outside the project"));
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("project folder unavailable: {e}"))?;
    let full = root
        .join(rel)
        .canonicalize()
        .map_err(|e| format!("{relative:?}: {e}"))?;
    if full.starts_with(&root) {
        Ok(full)
    } else {
        Err(format!("{relative:?} resolves outside the project"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str) -> RunMeta {
        RunMeta {
            run_id: id.into(),
            chat_id: "chat".into(),
            name: "demo".into(),
            script_hash: "h".into(),
            args_hash: "a".into(),
            created_at: 1,
            ..Default::default()
        }
    }

    #[test]
    fn a_run_round_trips_meta_script_and_journal() {
        let dir = tempfile::tempdir().unwrap();
        let store = WorkflowStore::open(dir.path());
        store
            .create_run(&meta("r1"), "def main(args): return 1")
            .unwrap();
        assert_eq!(store.read_meta("r1").unwrap().name, "demo");
        assert!(store.read_script("r1").unwrap().contains("main"));
        let journal = store.open_journal("r1").unwrap();
        journal
            .append(&Record::AskDone {
                key: "s#0".into(),
                digest: "d".into(),
                ok: true,
                value: serde_json::json!({"a": 1}),
                error: None,
                input_tokens: 3,
                output_tokens: 4,
                child_chat_id: Some("c".into()),
            })
            .unwrap();
        journal
            .append(&Record::Report {
                index: 0,
                item: serde_json::json!("x"),
                artifact_id: None,
            })
            .unwrap();
        journal
            .append(&Record::Result {
                value: serde_json::json!(7),
            })
            .unwrap();
        let replay = store.load_replay("r1").unwrap();
        assert!(replay.asks.contains_key("s#0"));
        assert_eq!(replay.reports.len(), 1);
        assert_eq!(replay.result, Some(serde_json::json!(7)));
        assert_eq!(store.list_metas().len(), 1);
    }

    #[test]
    fn a_torn_last_line_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let store = WorkflowStore::open(dir.path());
        store.create_run(&meta("r1"), "").unwrap();
        let journal = store.open_journal("r1").unwrap();
        journal.append(&Record::Log { line: "ok".into() }).unwrap();
        let path = dir.path().join("workflows/r1/journal.jsonl");
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(b"{\"rec\":\"askDone\",\"key\":\"x").unwrap();
        let replay = store.load_replay("r1").unwrap();
        assert_eq!(replay.logs, ["ok"]);
        assert!(replay.asks.is_empty());
    }

    #[test]
    fn ids_cannot_escape_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = WorkflowStore::open(dir.path());
        for bad in ["../x", "a/b", "", ".hidden", "a\\b", "x\0y", "a b"] {
            assert!(store.read_meta(bad).is_err(), "{bad:?}");
            assert!(safe_component("x", bad).is_err(), "{bad:?}");
        }
        store.create_run(&meta("r1"), "").unwrap();
        assert!(
            store
                .read_artifact("r1", "../../meta.json", None, 0, 10)
                .is_err()
        );
        assert!(store.read_artifact("r1", "nope", None, 0, 10).is_err());
        assert!(store.read_artifact("../r1", "x", None, 0, 10).is_err());
    }

    #[test]
    fn artifacts_are_versioned_and_read_in_pages() {
        let dir = tempfile::tempdir().unwrap();
        let store = WorkflowStore::open(dir.path());
        store.create_run(&meta("r1"), "").unwrap();
        for v in 1..=2u32 {
            store
                .put_artifact(
                    "r1",
                    "summary",
                    v,
                    ArtifactKind::Markdown,
                    "Summary",
                    "text/markdown",
                    "md",
                    0,
                    format!("version {v} body").as_bytes(),
                    9,
                )
                .unwrap();
        }
        assert_eq!(store.artifact_ids("r1").unwrap(), ["summary"]);
        let latest = store.read_artifact("r1", "summary", None, 0, 100).unwrap();
        assert_eq!(latest.version.version, 2);
        assert_eq!(String::from_utf8(latest.bytes).unwrap(), "version 2 body");
        let first = store.read_artifact("r1", "summary", Some(1), 8, 4).unwrap();
        assert_eq!(String::from_utf8(first.bytes).unwrap(), "1 bo");
        assert_eq!(first.total, 14);
        assert!(store.read_artifact("r1", "summary", Some(9), 0, 4).is_err());
        // A tampered index cannot point outside the artifact directory.
        let idx = dir.path().join("workflows/r1/artifacts/summary/index.json");
        let mut index: ArtifactIndex =
            serde_json::from_slice(&std::fs::read(&idx).unwrap()).unwrap();
        index.versions[1].file = "../../meta.json".into();
        std::fs::write(&idx, serde_json::to_vec(&index).unwrap()).unwrap();
        assert!(store.read_artifact("r1", "summary", None, 0, 10).is_err());
    }

    #[test]
    fn confine_refuses_escapes_including_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "x").unwrap();
        std::fs::write(dir.path().join("secret"), "s").unwrap();
        assert!(confine(&root, "src/a.rs").is_ok());
        assert!(confine(&root, "../secret").is_err());
        assert!(confine(&root, "/etc/passwd").is_err());
        assert!(confine(&root, "src/../../secret").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret"), root.join("link")).unwrap();
            assert!(confine(&root, "link").is_err());
        }
    }
}
