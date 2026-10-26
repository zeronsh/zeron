//! Prompt drafts (docs/prompt-drafts.md) on the thin client: an unsent
//! new-session canvas survives closing the page, relaunching and switching
//! devices, and lists above Pinned like the desktop sidebar's Drafts.
//!
//! The registry carries only indexes (`promptDrafts` / `draftRevisions`
//! rows, see `zeron_doc` `registry/drafts.rs`); full content and file bytes
//! are immutable objects at `/draft-content/{org}/{id}` on the edge — content
//! JSON by revision id, files by SHA-256. A save lands in a local object
//! store plus a durable outbox first, then publishes: upload the objects,
//! then write the registry rows, so no device ever sees a revision whose
//! content isn't readable. Unpublished saves still list here (`pending`).
//!
//! Demo keeps every object in memory and publishes at once.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use zeron_proto::{
    DraftAttachment, DraftChange, DraftContent, DraftTarget, MAX_DRAFT_ASSET_BYTES,
    MAX_DRAFT_CONTENT_BYTES, PromptDraft, SaveDraft, valid_draft_id,
};

use crate::client::{Client, ClientInner, NewSession};
use crate::error::{ClientError, Result};
use crate::lock;

/// Same cap the desktop engine enforces per draft.
const MAX_DRAFT_FILES: usize = 32;
const OUTBOX: &str = "outbox.json";
const UPLOADED: &str = "uploaded.json";

/// One attached file, as the composer holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftFile {
    /// Stable across revisions (the composer's attachment id).
    pub id: String,
    pub name: String,
    pub data: Vec<u8>,
    /// Desktop Appshot context, carried through untouched.
    pub appshot: Option<serde_json::Value>,
}

/// One save of a canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftEdit {
    pub id: String,
    /// New whenever the content changed; resaving one revision is a no-op
    /// (or a visibility change).
    pub revision: String,
    /// The revision this one was edited from.
    pub base_revision: Option<String>,
    pub created_at_ms: i64,
    /// Still the open canvas: saved everywhere, listed only once left.
    pub deferred: bool,
    pub prompt: String,
    pub target: DraftTarget,
    pub files: Vec<DraftFile>,
}

/// A draft's full content, for reopening its canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedDraft {
    pub prompt: String,
    pub target: DraftTarget,
    pub files: Vec<DraftFile>,
}

fn storage(err: impl std::fmt::Display) -> ClientError {
    ClientError::Storage(err.to_string())
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_blob(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Immutable draft objects plus the publication outbox.
pub(crate) struct DraftStore {
    /// `None`: memory only (Demo).
    dir: Option<PathBuf>,
    objects: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    outbox: Mutex<Vec<SaveDraft>>,
    uploaded: Mutex<HashSet<String>>,
    publishing: AtomicBool,
}

impl DraftStore {
    pub(crate) fn open(dir: Option<PathBuf>) -> Self {
        let read = |name: &str| {
            dir.as_ref()
                .and_then(|dir| std::fs::read(dir.join(name)).ok())
        };
        let outbox = read(OUTBOX)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let uploaded = read(UPLOADED)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if let Some(dir) = &dir
            && let Err(err) = std::fs::create_dir_all(dir.join("objects"))
        {
            tracing::warn!(error = %err, "draft store unavailable");
        }
        Self {
            dir,
            objects: Mutex::new(HashMap::new()),
            outbox: Mutex::new(outbox),
            uploaded: Mutex::new(uploaded),
            publishing: AtomicBool::new(false),
        }
    }

    fn write_file(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        // Write-then-rename: a crash never leaves a torn outbox or object.
        let tmp = dir.join(format!("{name}.tmp"));
        std::fs::write(&tmp, bytes).map_err(storage)?;
        std::fs::rename(&tmp, dir.join(name)).map_err(storage)
    }

    pub(crate) fn object(&self, id: &str) -> Option<Arc<Vec<u8>>> {
        if let Some(bytes) = lock(&self.objects).get(id) {
            return Some(bytes.clone());
        }
        let bytes = Arc::new(std::fs::read(self.dir.as_ref()?.join("objects").join(id)).ok()?);
        Some(bytes)
    }

    pub(crate) fn put_object(&self, id: &str, bytes: Vec<u8>) -> Result<()> {
        if !valid_draft_id(id) {
            return Err(ClientError::InvalidArgument("invalid draft object".into()));
        }
        if self.dir.is_some() {
            self.write_file(&format!("objects/{id}"), &bytes)
        } else {
            lock(&self.objects).insert(id.to_owned(), Arc::new(bytes));
            Ok(())
        }
    }

    /// Store a save's objects and queue its publication.
    fn stage(&self, edit: DraftEdit) -> Result<SaveDraft> {
        if !valid_draft_id(&edit.id)
            || !valid_draft_id(&edit.revision)
            || edit
                .base_revision
                .as_deref()
                .is_some_and(|b| !valid_draft_id(b))
        {
            return Err(ClientError::InvalidArgument("invalid draft id".into()));
        }
        if edit.files.len() > MAX_DRAFT_FILES {
            return Err(ClientError::InvalidArgument("too many draft files".into()));
        }
        let mut content = DraftContent {
            prompt: edit.prompt,
            target: edit.target,
            attachments: Vec::with_capacity(edit.files.len()),
        };
        for file in edit.files {
            if file.data.len() > MAX_DRAFT_ASSET_BYTES {
                return Err(ClientError::InvalidArgument(format!(
                    "{} is too large for a draft",
                    file.name
                )));
            }
            let blob = sha256(&file.data);
            if self.object(&blob).is_none() {
                self.put_object(&blob, file.data)?;
            }
            content.attachments.push(DraftAttachment {
                id: file.id,
                name: file.name,
                blob,
                appshot: file.appshot,
            });
        }
        let bytes = serde_json::to_vec(&content).map_err(storage)?;
        if bytes.len() > MAX_DRAFT_CONTENT_BYTES {
            return Err(ClientError::InvalidArgument("draft is too large".into()));
        }
        // A revision is immutable: a retried save must carry the same content.
        match self.object(&edit.revision) {
            Some(old) => {
                let previous: DraftContent = serde_json::from_slice(&old).map_err(storage)?;
                if previous != content {
                    return Err(ClientError::InvalidArgument(
                        "draft revision already holds different content".into(),
                    ));
                }
            }
            None => self.put_object(&edit.revision, bytes)?,
        }
        let save = SaveDraft {
            deferred: edit.deferred,
            id: edit.id,
            revision: edit.revision,
            base_revision: edit.base_revision,
            created_at: edit.created_at_ms,
            content,
            assets: Vec::new(),
        };
        let mut outbox = lock(&self.outbox);
        let key = save.publication_key();
        if !outbox.iter().any(|s| s.publication_key() == key) {
            outbox.push(save.clone());
            self.persist_outbox(&outbox)?;
        }
        Ok(save)
    }

    fn persist_outbox(&self, outbox: &[SaveDraft]) -> Result<()> {
        self.write_file(OUTBOX, &serde_json::to_vec(outbox).map_err(storage)?)
    }

    pub(crate) fn pending(&self) -> Vec<SaveDraft> {
        lock(&self.outbox).clone()
    }

    fn acknowledge(&self, key: &str) -> Result<()> {
        let mut outbox = lock(&self.outbox);
        outbox.retain(|s| s.publication_key() != key);
        self.persist_outbox(&outbox)
    }

    /// A discarded or sent draft: nothing of it is left to publish.
    fn forget(&self, id: &str) -> Result<()> {
        let mut outbox = lock(&self.outbox);
        outbox.retain(|s| s.id != id);
        self.persist_outbox(&outbox)
    }

    fn uploaded(&self, id: &str) -> bool {
        lock(&self.uploaded).contains(id)
    }

    fn mark_uploaded(&self, id: &str) -> Result<()> {
        let mut uploaded = lock(&self.uploaded);
        uploaded.insert(id.to_owned());
        self.write_file(UPLOADED, &serde_json::to_vec(&*uploaded).map_err(storage)?)
    }
}

/// Registry drafts with this device's unpublished saves laid over them: an
/// edit replaces its draft's revision in place; a new draft lists first.
pub(crate) fn overlay(
    registry: &[PromptDraft],
    pending: &[SaveDraft],
    closed: impl Fn(&str) -> bool,
) -> Vec<PromptDraft> {
    let mut rows = registry.to_vec();
    for save in pending {
        if closed(&save.id) {
            continue;
        }
        let row = PromptDraft {
            id: save.id.clone(),
            revision: save.revision.clone(),
            base_revision: save.base_revision.clone(),
            created_at: save.created_at,
            preview: save.content.preview(),
            target: save.content.target.clone(),
            order_key: String::new(),
            conflict: false,
            pending: true,
        };
        if let Some(existing) = rows.iter_mut().find(|r| r.id == save.id) {
            *existing = PromptDraft {
                order_key: std::mem::take(&mut existing.order_key),
                conflict: existing.conflict,
                ..row
            };
        } else if !save.deferred {
            rows.push(row);
        }
    }
    rows.sort_by(|a, b| a.order_key.cmp(&b.order_key).then(a.id.cmp(&b.id)));
    rows
}

impl ClientInner {
    /// Drain the outbox in the background (one drain at a time). Failures
    /// stay queued; the next save, foreground or reconnect retries.
    pub(crate) fn publish_drafts(self: &Arc<Self>) {
        if self.drafts.pending().is_empty() || self.drafts.publishing.swap(true, Ordering::AcqRel) {
            return;
        }
        let inner = self.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                let drained = inner.drain_drafts().await;
                inner.drafts.publishing.store(false, Ordering::Release);
                if let Err(err) = drained {
                    tracing::debug!(error = %err, "draft publication deferred");
                    return;
                }
                // A save that raced the drain goes out now, unless another
                // drain already took it.
                if inner.drafts.pending().is_empty()
                    || inner.drafts.publishing.swap(true, Ordering::AcqRel)
                {
                    return;
                }
            }
        });
    }

    async fn drain_drafts(self: &Arc<Self>) -> Result<()> {
        for save in self.drafts.pending() {
            let discarded = self.workspace.mutate(|doc| doc.draft_discarded(&save.id));
            if !discarded {
                self.upload_draft(&save).await?;
                self.registry_write(|doc| doc.publish_draft(&save))?;
            }
            self.drafts.acknowledge(&save.publication_key())?;
            self.refresh_pending_drafts();
        }
        Ok(())
    }

    async fn upload_draft(&self, save: &SaveDraft) -> Result<()> {
        let Some(live) = self.live() else {
            return Ok(());
        };
        let ids = save
            .content
            .attachments
            .iter()
            .map(|a| a.blob.as_str())
            .chain(std::iter::once(save.revision.as_str()));
        for id in ids {
            if self.drafts.uploaded(id) {
                continue;
            }
            let bytes = self
                .drafts
                .object(id)
                .ok_or_else(|| ClientError::Storage(format!("draft object {id} is missing")))?;
            let token = self.tokens.bearer().await?;
            let response = crate::auth::http()
                .put(draft_object_url(&live.edge, self.credentials.org_id(), id))
                .bearer_auth(token)
                .body((*bytes).clone())
                .send()
                .await
                .map_err(|e| ClientError::Network(e.to_string()))?;
            if !response.status().is_success() {
                return Err(ClientError::Network(format!(
                    "draft upload http {}",
                    response.status()
                )));
            }
            self.drafts.mark_uploaded(id)?;
        }
        Ok(())
    }

    async fn draft_object(&self, id: &str) -> Result<Arc<Vec<u8>>> {
        if let Some(bytes) = self.drafts.object(id) {
            return Ok(bytes);
        }
        let live = self
            .live()
            .ok_or_else(|| ClientError::NotFound(format!("draft object {id}")))?;
        let token = self.tokens.bearer().await?;
        let response = crate::auth::http()
            .get(draft_object_url(&live.edge, self.credentials.org_id(), id))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ClientError::NotFound(format!("draft object {id}")));
        }
        if !response.status().is_success() {
            return Err(ClientError::Network(format!(
                "draft download http {}",
                response.status()
            )));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| ClientError::Network(e.to_string()))?;
        if bytes.len() > MAX_DRAFT_ASSET_BYTES || (is_blob(id) && sha256(&bytes) != id) {
            return Err(ClientError::Storage(format!(
                "draft object {id} is corrupt"
            )));
        }
        self.drafts.put_object(id, bytes.to_vec())?;
        // Already on the edge: never upload it back.
        self.drafts.mark_uploaded(id)?;
        Ok(Arc::new(bytes.to_vec()))
    }

    /// The workspace view lists unpublished saves too.
    pub(crate) fn refresh_pending_drafts(self: &Arc<Self>) {
        self.workspace.set_pending_drafts(self.drafts.pending());
        self.recompute_workspace();
    }
}

/// Demo: a draft as if another device had published it long ago.
pub(crate) fn seed(inner: &Arc<ClientInner>, edit: DraftEdit) -> Result<()> {
    let save = inner.drafts.stage(edit)?;
    inner
        .workspace
        .mutate(|doc| doc.publish_draft(&save))
        .map_err(ClientError::from)?;
    inner.drafts.acknowledge(&save.publication_key())
}

fn draft_object_url(edge: &str, org_id: &str, id: &str) -> String {
    format!(
        "{}/draft-content/{}/{}",
        edge.trim_end_matches('/'),
        crate::live::urls::encode(org_id),
        crate::live::urls::encode(id)
    )
}

impl Client {
    /// Save a canvas (autosave, leaving the page, backgrounding). Cheap and
    /// local; publication to other devices follows in the background.
    pub fn save_draft(&self, edit: DraftEdit) -> Result<()> {
        if self
            .inner
            .workspace
            .mutate(|doc| doc.draft_closed(&edit.id))
        {
            return Err(ClientError::NotFound(edit.id));
        }
        self.inner.drafts.stage(edit)?;
        self.inner.refresh_pending_drafts();
        self.inner.publish_drafts();
        Ok(())
    }

    /// A draft's full content (local objects first, else the edge).
    pub async fn load_draft(&self, revision: &str) -> Result<LoadedDraft> {
        if !valid_draft_id(revision) {
            return Err(ClientError::InvalidArgument(
                "invalid draft revision".into(),
            ));
        }
        let inner = self.inner.clone();
        let revision = revision.to_owned();
        crate::runtime::run(async move {
            let content: DraftContent =
                serde_json::from_slice(inner.draft_object(&revision).await?.as_slice())
                    .map_err(storage)?;
            let mut files = Vec::with_capacity(content.attachments.len());
            for attachment in content.attachments {
                let data = inner.draft_object(&attachment.blob).await?;
                files.push(DraftFile {
                    id: attachment.id,
                    name: attachment.name,
                    data: (*data).clone(),
                    appshot: attachment.appshot,
                });
            }
            Ok(LoadedDraft {
                prompt: content.prompt,
                target: content.target,
                files,
            })
        })
        .await
    }

    /// Discard for good: a delayed save or move can't bring it back.
    pub fn discard_draft(&self, id: &str) -> Result<()> {
        self.inner.drafts.forget(id)?;
        self.inner
            .registry_write(|doc| doc.change_draft(&DraftChange::Discard { id: id.to_owned() }))?;
        self.inner.refresh_pending_drafts();
        Ok(())
    }

    /// Reorder between neighbours (either may be `None` at the ends).
    pub fn move_draft(
        &self,
        id: &str,
        after: Option<String>,
        before: Option<String>,
    ) -> Result<()> {
        self.inner.registry_write(|doc| {
            doc.change_draft(&DraftChange::Move {
                id: id.to_owned(),
                after,
                before,
            })
        })
    }

    /// Reserve one exact revision for sending (the registry room serializes
    /// claims): another device can't send the same draft twice.
    pub async fn claim_draft(&self, id: &str, revision: &str) -> Result<()> {
        if !valid_draft_id(id) || !valid_draft_id(revision) {
            return Err(ClientError::InvalidArgument("invalid draft claim".into()));
        }
        let Some(live) = self.inner.live() else {
            return Ok(());
        };
        let url = format!(
            "{}/registry/{}/draft-claim",
            live.edge.trim_end_matches('/'),
            crate::live::urls::encode(self.inner.credentials.org_id())
        );
        let inner = self.inner.clone();
        let body = serde_json::json!({ "id": id, "revision": revision });
        crate::runtime::run(async move {
            let token = inner.tokens.bearer().await?;
            let response = crate::auth::http()
                .post(url)
                .bearer_auth(token)
                .json(&body)
                .send()
                .await
                .map_err(|e| ClientError::Network(e.to_string()))?;
            if response.status() == reqwest::StatusCode::CONFLICT {
                return Err(ClientError::InvalidArgument(
                    "this draft was already sent from another device".into(),
                ));
            }
            if !response.status().is_success() {
                return Err(ClientError::Network(format!(
                    "draft claim http {}",
                    response.status()
                )));
            }
            Ok(())
        })
        .await
    }

    /// The draft became a session: it leaves the list on every device. A
    /// concurrent edit from elsewhere stays listed as its own draft.
    pub fn consume_draft(&self, id: &str, revision: &str) -> Result<()> {
        self.inner.drafts.forget(id)?;
        self.inner.registry_write(|doc| {
            doc.change_draft(&DraftChange::Consume {
                id: id.to_owned(),
                revision: revision.to_owned(),
            })
        })?;
        self.inner.refresh_pending_drafts();
        Ok(())
    }

    /// The chat a draft becomes: its id derives from the draft (`draft-{id}`,
    /// as on desktop), so a retried send never mints a second chat.
    pub fn create_draft_session(&self, draft_id: &str, new: NewSession) -> Result<String> {
        if !valid_draft_id(draft_id) {
            return Err(ClientError::InvalidArgument("invalid draft id".into()));
        }
        let chat_id = format!("draft-{draft_id}");
        if self.inner.workspace.chat(&chat_id).is_some() {
            return Ok(chat_id);
        }
        self.create_session_as(chat_id, new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(id: &str, revision: &str, prompt: &str) -> DraftEdit {
        DraftEdit {
            id: id.into(),
            revision: revision.into(),
            base_revision: None,
            created_at_ms: 1,
            deferred: false,
            prompt: prompt.into(),
            target: DraftTarget {
                device_id: "mac".into(),
                ..Default::default()
            },
            files: vec![DraftFile {
                id: "file".into(),
                name: "shot.png".into(),
                data: b"png bytes".to_vec(),
                appshot: None,
            }],
        }
    }

    #[test]
    fn outbox_and_objects_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let store = DraftStore::open(Some(dir.path().into()));
        let save = store
            .stage(edit("draft", "rev-1", "Fix the login"))
            .unwrap();
        assert_eq!(save.content.attachments[0].blob, sha256(b"png bytes"));
        drop(store);

        let store = DraftStore::open(Some(dir.path().into()));
        assert_eq!(store.pending(), vec![save.clone()]);
        let content: DraftContent =
            serde_json::from_slice(&store.object("rev-1").unwrap()).unwrap();
        assert_eq!(content, save.content);
        assert_eq!(
            *store.object(&sha256(b"png bytes")).unwrap(),
            b"png bytes".to_vec()
        );
        store.acknowledge(&save.publication_key()).unwrap();
        assert!(
            DraftStore::open(Some(dir.path().into()))
                .pending()
                .is_empty()
        );
    }

    #[test]
    fn a_revision_never_changes_content() {
        let store = DraftStore::open(None);
        store.stage(edit("draft", "rev-1", "one")).unwrap();
        // Retrying the same save is fine; reusing its revision is not.
        store.stage(edit("draft", "rev-1", "one")).unwrap();
        assert_eq!(store.pending().len(), 1);
        assert!(store.stage(edit("draft", "rev-1", "two")).is_err());
    }

    #[test]
    fn unpublished_saves_list_over_the_registry() {
        let store = DraftStore::open(None);
        let listed = PromptDraft {
            id: "a".into(),
            revision: "a-1".into(),
            base_revision: None,
            created_at: 1,
            preview: "old".into(),
            target: DraftTarget::default(),
            order_key: "8".into(),
            conflict: false,
            pending: false,
        };
        let mut edited = edit("a", "a-2", "edited");
        edited.base_revision = Some("a-1".into());
        // The open canvas stays hidden until it is left.
        let mut canvas = edit("b", "b-1", "typing");
        canvas.deferred = true;
        let saves = vec![
            store.stage(edited).unwrap(),
            store.stage(canvas).unwrap(),
            store.stage(edit("c", "c-1", "fresh")).unwrap(),
        ];
        let rows = overlay(&[listed], &saves, |_| false);
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c", "a"]
        );
        assert_eq!(rows[1].revision, "a-2");
        assert_eq!(rows[1].preview, "edited");
        assert_eq!(rows[1].order_key, "8");
        assert!(rows.iter().all(|r| r.pending));
        // Discarded or sent elsewhere: gone, even before publication.
        let rows = overlay(&[], &saves, |id| id == "c");
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
    }
}
