//! Attachment escorts (queued-attachments flow, hosts ≥ 0.2.12).
//!
//! A send with images writes its command at once with `pending://{id}/{name}`
//! refs; the bytes are stashed on disk and pushed to the host with chunked
//! `UploadChunk`/`UploadCommit` under the SAME upload id + file name the ref
//! names (the host resolves the ref to that path and defers the command
//! until it lands). Escorts retry with backoff for up to the host's wait
//! window and are re-derived from the stash on relaunch.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::client::ClientInner;
use crate::{lock, now_ms};

/// The host stops waiting for a deferred attachment after this long.
const ESCORT_MAX_MS: i64 = 15 * 60_000;
const BACKOFF_BASE: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StashMeta {
    pub upload_id: String,
    pub chat_id: String,
    pub host_device_id: String,
    pub name: String,
    pub created_at_ms: i64,
}

pub(crate) struct Escorts {
    dir: PathBuf,
    active: Mutex<HashSet<String>>,
}

impl Escorts {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("uploads"),
            active: Mutex::new(HashSet::new()),
        }
    }

    fn bin(&self, upload_id: &str) -> PathBuf {
        self.dir.join(format!("{upload_id}.bin"))
    }

    fn meta(&self, upload_id: &str) -> PathBuf {
        self.dir.join(format!("{upload_id}.json"))
    }

    /// Durable copy of the bytes (before the command is written).
    pub(crate) fn stash(&self, meta: &StashMeta, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.bin(&meta.upload_id), bytes)?;
        std::fs::write(
            self.meta(&meta.upload_id),
            serde_json::to_vec(meta).unwrap_or_default(),
        )
    }

    fn remove(&self, upload_id: &str) {
        let _ = std::fs::remove_file(self.bin(upload_id));
        let _ = std::fs::remove_file(self.meta(upload_id));
    }

    fn load(&self, upload_id: &str) -> Option<(StashMeta, Vec<u8>)> {
        let meta: StashMeta =
            serde_json::from_slice(&std::fs::read(self.meta(upload_id)).ok()?).ok()?;
        let bytes = std::fs::read(self.bin(upload_id)).ok()?;
        Some((meta, bytes))
    }

    /// Stashed escorts not yet delivered for `chat_id`.
    pub(crate) fn pending_for(&self, chat_id: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let id = name.strip_suffix(".json")?.to_owned();
                let meta: StashMeta =
                    serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
                (meta.chat_id == chat_id).then_some(id)
            })
            .collect()
    }

    /// Relaunch: re-derive every stashed escort.
    pub(crate) fn respawn(&self, inner: &Arc<ClientInner>) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json") {
                self.spawn(inner, id);
            }
        }
    }

    /// Retry (user-driven): restart every escort for the chat.
    pub(crate) fn respawn_chat(&self, inner: &Arc<ClientInner>, chat_id: &str) {
        for id in self.pending_for(chat_id) {
            self.spawn(inner, &id);
        }
    }

    pub(crate) fn spawn(&self, inner: &Arc<ClientInner>, upload_id: &str) {
        if !lock(&self.active).insert(upload_id.to_owned()) {
            return;
        }
        let weak: Weak<ClientInner> = Arc::downgrade(inner);
        let upload_id = upload_id.to_owned();
        let cancel = inner.cancel.clone();
        crate::runtime::shared().spawn(async move {
            run(weak.clone(), &upload_id, cancel).await;
            if let Some(inner) = weak.upgrade()
                && let Some(live) = inner.live()
            {
                lock(&live.escorts.active).remove(&upload_id);
            }
        });
    }
}

async fn run(
    weak: Weak<ClientInner>,
    upload_id: &str,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut backoff = BACKOFF_BASE;
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let Some(live) = inner.live() else { return };
        let Some((meta, bytes)) = live.escorts.load(upload_id) else {
            return;
        };
        if now_ms() - meta.created_at_ms > ESCORT_MAX_MS {
            tracing::warn!(upload = %upload_id, chat = %meta.chat_id, "attachment escort expired");
            live.escorts.remove(upload_id);
            return;
        }
        let core = inner.session_core(&meta.chat_id);
        let progress: Option<crate::rpc::ProgressFn> = core.as_ref().map(|core| {
            let core = Arc::downgrade(core);
            Arc::new(move |fraction: f64| {
                if let Some(core) = core.upgrade() {
                    core.set_transfer_progress(Some(fraction));
                }
            }) as crate::rpc::ProgressFn
        });
        let relay_result = live
            .relay
            .upload(
                &meta.host_device_id,
                upload_id,
                &meta.name,
                &bytes,
                progress,
            )
            .await;
        if let Some(core) = &core {
            core.set_transfer_progress(None);
        }
        match relay_result {
            Ok(path) => {
                tracing::info!(upload = %upload_id, %path, "attachment escorted");
                live.escorts.remove(upload_id);
                inner.nudge_host(&meta.host_device_id, &meta.chat_id);
                return;
            }
            Err(err) => {
                tracing::warn!(upload = %upload_id, error = %err, "attachment escort failed; retrying");
            }
        }
        drop(inner);
        if !super::wait_backoff(&cancel, backoff).await {
            return;
        }
        backoff = (backoff * 2).min(BACKOFF_CAP);
    }
}
