//! Cross-authority native profile import (for example Crew Canary → Crew Staging).
//!
//! Scaffold profiles are isolated per authority (`profiles/scaffold/<key>`), so a
//! user moving to another deployment cannot simply reopen the old store. This
//! importer reads a stopped-writer COPY of another native profile and carries its
//! chats into the running owner controller through the same write paths the
//! local→synced import uses ([`crate::local_import`]):
//!
//!  - documents (chat + `--sub--` subagent docs) are merged with the source's
//!    unacknowledged outbox rows and stored in the born-chat2 shape
//!    (`cursor 0`, epoch 2). The source authority's room cursors are meaningless
//!    to this Edge and are never copied.
//!  - each document is seeded into this Edge's chat2 room as a seq-0 checkpoint
//!    *before* its registry row lands, so other devices can read the history.
//!  - registry rows are rebuilt, never copied as a snapshot: `crew` metadata is
//!    replaced by this verified identity's, and chats hosted by the source device
//!    are re-homed to this device. Chats hosted elsewhere stay addressed to their
//!    original device (inert history) until that host runs its own import, which
//!    re-homes the existing row.
//!  - every Pending command in an imported document receives an outcome-less
//!    ledger claim, so a host drain rejects it as interrupted and never replays
//!    work the source authority accepted.
//!  - attachments are not rewritten: the source uploads root becomes a read-only
//!    root, recorded in `native-profile-imports.json` for later boots.
//!
//! Idempotence is structural: rows already present are skipped (except the
//! re-home case above), so re-running after a partial import finishes the rest.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zeron_doc::{REGISTRY_DOC_ID, RegistryDoc, SessionCommandStatus, SessionDoc};
use zeron_proto::{Chat, Space};
use zeron_sync::DocsStore;

use crate::EngineError;
use crate::chat2_host::CHAT2_DOC_EPOCH;
use crate::doc_host::DocHost;
use crate::run_journal::journal_paths;
use crate::uploads::Uploads;
use crate::workspace_host::WorkspaceHost;

const MARKER_FILE: &str = "native-profile-imports.json";
const ACK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// What one import run should do.
#[derive(Debug, Clone, Default)]
pub struct NativeImportOptions {
    /// A consistent copy of the source profile directory (the one holding
    /// `docs.sqlite3` and `journals/`), taken after its writer stopped.
    pub source_copy: PathBuf,
    /// The source profile's original uploads directory; transcripts embed
    /// absolute paths under it. Granted read-only, never copied.
    pub source_uploads: Option<PathBuf>,
    /// Limit the run to these chat ids (experiments, partial retries).
    pub chat_ids: Option<HashSet<String>>,
    /// Plan and validate only; write nothing.
    pub validate_only: bool,
}

/// Verified target identity the rebuilt rows are bound to.
#[derive(Debug, Clone)]
pub struct NativeImportTarget {
    pub owner_subject: String,
    pub project_id: String,
    pub deployment_id: Option<String>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeImportReport {
    pub validate_only: bool,
    pub source_device_id: String,
    pub target_device_id: String,
    pub source_project_id: Option<String>,
    pub chats_considered: usize,
    pub rehomed_chats: usize,
    pub remote_hosted_chats: usize,
    pub skipped_existing: usize,
    pub imported_chats: usize,
    pub published_documents: usize,
    pub ledger_rows_merged: usize,
    pub pending_commands_neutralized: usize,
    pub journals_copied: usize,
    pub spaces_imported: usize,
    pub spaces_mapped: usize,
    pub chats_refiled: usize,
    pub unacknowledged: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    #[serde(default)]
    read_only_upload_roots: Vec<PathBuf>,
}

/// Read-only upload roots granted by earlier imports into `store_root`.
pub fn marker_read_roots(store_root: &Path) -> Vec<PathBuf> {
    std::fs::read(store_root.join(MARKER_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Marker>(&bytes).ok())
        .map(|marker| marker.read_only_upload_roots.into_iter().filter(|root| root.is_dir()).collect())
        .unwrap_or_default()
}

fn record_upload_root(store_root: &Path, root: &Path) -> Result<(), EngineError> {
    let path = store_root.join(MARKER_FILE);
    let mut marker: Marker = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|err| EngineError::Other(format!("{MARKER_FILE} is unreadable: {err}")))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Marker::default(),
        Err(err) => return Err(err.into()),
    };
    if marker.read_only_upload_roots.iter().any(|existing| existing == root) {
        return Ok(());
    }
    marker.read_only_upload_roots.push(root.to_path_buf());
    let bytes = serde_json::to_vec_pretty(&marker).map_err(|err| EngineError::Other(err.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    let publish = || -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    publish().map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        EngineError::Other(format!("{MARKER_FILE} write failed: {err}"))
    })
}

/// The opened, validated source copy.
pub struct NativeProfileSource {
    root: PathBuf,
    store: DocsStore,
    registry: RegistryDoc,
    device_id: String,
}

impl NativeProfileSource {
    /// Open a stopped-writer copy. A live profile (non-empty WAL) is refused:
    /// opening it here would race its engine.
    pub fn open(root: &Path, target_store_root: &Path) -> Result<Self, EngineError> {
        if !root.is_absolute() {
            return Err(EngineError::Other("source copy must be an absolute path".into()));
        }
        let root = root.canonicalize()?;
        if target_store_root.canonicalize().ok().as_deref() == Some(root.as_path()) {
            return Err(EngineError::Other("source copy is this engine's own profile".into()));
        }
        let database = root.join("docs.sqlite3");
        if !database.is_file() {
            return Err(EngineError::Other(format!("{} has no docs.sqlite3", root.display())));
        }
        let wal = root.join("docs.sqlite3-wal");
        if std::fs::metadata(&wal).is_ok_and(|meta| meta.len() > 0) {
            return Err(EngineError::Other(
                "source has a live write-ahead log: stop its writer and import a consistent copy (sqlite3 .backup)".into(),
            ));
        }
        let store = DocsStore::open(&root)?;
        let bytes = store
            .load_snapshot(REGISTRY_DOC_ID)?
            .ok_or_else(|| EngineError::Other("source profile has no registry replica".into()))?;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Owner {
            device_id: String,
        }
        let owner: Owner = serde_json::from_slice(&bytes)
            .map_err(|err| EngineError::Other(format!("source registry is unreadable: {err}")))?;
        if owner.device_id.trim().is_empty() {
            return Err(EngineError::Other("source registry has no owning device".into()));
        }
        let registry = RegistryDoc::from_bytes(&bytes, &owner.device_id)?;
        Ok(Self { root, store, registry, device_id: owner.device_id })
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }
}

/// How one source chat lands in the target.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Disposition {
    /// New row; host stays the original (non-source) device.
    Remote,
    /// New row, hosted by this device.
    Rehome,
    /// Existing inert row addressed to the source device: this run is from its
    /// host, so merge the authoritative documents and re-home it.
    RehomeExisting,
    /// Already re-homed here; only its project reference moves to this device's.
    RepairSpace,
    Skip,
}

/// Stable id for a source host's project re-homed onto `device` when another
/// device's row already holds the original id (a re-run derives the same id).
fn rehomed_space_id(space_id: &str, device: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("crew-native-import-space\0{space_id}\0{device}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

/// Map a source chat row onto the target identity and device.
fn rebuild_row(
    chat: &Chat,
    source_device: &str,
    target_device: &str,
    target: &NativeImportTarget,
    spaces: &BTreeMap<String, String>,
) -> Chat {
    let mut row = chat.clone();
    if row.device_id == source_device {
        row.device_id = target_device.to_owned();
    }
    if let Some(space) = &row.space_id
        && let Some(mapped) = spaces.get(space)
    {
        row.space_id = Some(mapped.clone());
    }
    row.room_gen = Some(CHAT2_DOC_EPOCH);
    // Source environments/routes name the old authority's sandboxes; the
    // rebuilt identity is exactly what `CrewServices::session_metadata` issues.
    row.crew = zeron_proto::is_cloud_session_id(&row.id).then(|| {
        zeron_proto::CrewChatMetadata {
            owner_subject: target.owner_subject.clone(),
            scope: zeron_proto::CollaborationScope {
                project_id: target.project_id.clone(),
                deployment_id: target.deployment_id.clone(),
                session_id: Some(row.id.clone()),
                unknown: Default::default(),
            },
            session_id: row.id.clone(),
            environment: None,
            agent_route: None,
            original_epoch_history: None,
        }
    });
    row
}

/// Merge a source document with its unacknowledged outbox rows (and, when
/// re-homing, the target's existing replica), returning the snapshot bytes and
/// the ids of commands that are still Pending.
fn merge_document(
    source: &DocsStore,
    target: &DocsStore,
    doc_id: &str,
    include_target: bool,
) -> Result<Option<(Vec<u8>, Vec<String>)>, EngineError> {
    let Some(bytes) = source.load_snapshot(doc_id)? else {
        return Ok(None);
    };
    let doc = loro::LoroDoc::new();
    doc.import(&bytes).map_err(|err| EngineError::Other(format!("{doc_id}: {err}")))?;
    for (_, update) in source.pending_chat_updates(doc_id)? {
        doc.import(&update).map_err(|err| EngineError::Other(format!("{doc_id} outbox: {err}")))?;
    }
    if include_target && let Some(existing) = target.load_snapshot(doc_id)? {
        doc.import(&existing).map_err(|err| EngineError::Other(format!("{doc_id} target: {err}")))?;
    }
    let merged = doc
        .export(loro::ExportMode::Snapshot)
        .map_err(|err| EngineError::Other(format!("{doc_id} export: {err}")))?;
    let session = SessionDoc::from_doc(doc);
    let mut pending: Vec<String> = session
        .read_commands()
        .map(|commands| {
            commands
                .into_iter()
                .filter(|command| command.status == SessionCommandStatus::Pending)
                .map(|command| command.id)
                .collect()
        })
        .unwrap_or_default();
    pending.extend(session.pending_command_receipt_ids().unwrap_or_default());
    Ok(Some((merged, pending)))
}

pub struct NativeProfileImporter {
    pub doc_host: DocHost,
    pub workspace: WorkspaceHost,
    pub uploads: Uploads,
    pub target: NativeImportTarget,
}

fn import_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

impl NativeProfileImporter {
    pub async fn run(&self, options: NativeImportOptions) -> Result<NativeImportReport, EngineError> {
        let _serial = import_lock().lock().await;
        let target_store = self.doc_host.crew_store();
        let target_root = target_store.profile_root()?;
        let source = Arc::new(NativeProfileSource::open(&options.source_copy, &target_root)?);
        let target_device = self.workspace.device_id().to_owned();
        let mut report = NativeImportReport {
            validate_only: options.validate_only,
            source_device_id: source.device_id.clone(),
            target_device_id: target_device.clone(),
            ..Default::default()
        };
        if source.device_id == target_device {
            return Err(EngineError::Other("source and target are the same device profile".into()));
        }

        let mut chats = source.registry.read_chats()?;
        if let Some(only) = &options.chat_ids {
            chats.retain(|chat| only.contains(&chat.id));
        }
        // Identity gate: every carried session must belong to this owner and to
        // a different deployment (same-deployment rows sync on their own).
        for chat in &chats {
            if let Some(crew) = &chat.crew {
                if crew.owner_subject != self.target.owner_subject {
                    return Err(EngineError::Other(format!(
                        "chat {} belongs to another owner; refusing the whole import", chat.id
                    )));
                }
                if crew.scope.project_id == self.target.project_id {
                    return Err(EngineError::Other(
                        "source already belongs to this project; nothing to migrate across authorities".into(),
                    ));
                }
                report.source_project_id.get_or_insert_with(|| crew.scope.project_id.clone());
            }
        }

        // Spaces: the source host's own projects belong to this device now — even
        // when another host's earlier import already carried them as inert copies.
        // They map onto an existing same-path space here, else are re-homed (under
        // a derived id when another device's row already holds the original id;
        // never rewrite that row). Foreign spaces are carried when referenced.
        let referenced: HashSet<String> = chats.iter().filter_map(|chat| chat.space_id.clone()).collect();
        let own_paths: BTreeMap<String, String> = self
            .workspace
            .read_spaces()?
            .into_iter()
            .filter(|space| space.device_id == target_device)
            .map(|space| (space.path, space.id))
            .collect();
        let mut space_map = BTreeMap::new();
        let mut spaces_to_import: Vec<Space> = Vec::new();
        for space in source.registry.read_spaces()? {
            if !referenced.contains(&space.id) {
                continue;
            }
            let existing = self.workspace.space(&space.id)?;
            if space.device_id != source.device_id {
                if existing.is_none() {
                    spaces_to_import.push(space);
                }
                continue;
            }
            if let Some(own) = own_paths.get(&space.path) {
                if *own != space.id {
                    space_map.insert(space.id.clone(), own.clone());
                    report.spaces_mapped += 1;
                }
                continue;
            }
            match existing {
                None => {
                    let mut space = space;
                    space.device_id = target_device.clone();
                    spaces_to_import.push(space);
                }
                Some(row) if row.device_id == target_device => {}
                Some(_) => {
                    let id = rehomed_space_id(&space.id, &target_device);
                    space_map.insert(space.id.clone(), id.clone());
                    report.spaces_mapped += 1;
                    if self.workspace.space(&id)?.is_none() {
                        let mut space = space;
                        space.id = id;
                        space.device_id = target_device.clone();
                        spaces_to_import.push(space);
                    }
                }
            }
        }

        let mut plan = Vec::new();
        for chat in chats {
            report.chats_considered += 1;
            let existing = self.workspace.chat(&chat.id)?;
            let disposition = match existing {
                None if chat.device_id == source.device_id => Disposition::Rehome,
                None => Disposition::Remote,
                Some(row) if chat.device_id == source.device_id && row.device_id == source.device_id => {
                    Disposition::RehomeExisting
                }
                // Already re-homed here, but still filed under an inert copy of
                // this host's project (an earlier run predates the mapping).
                Some(row) if chat.device_id == source.device_id && row.device_id == target_device
                    && row.space_id.as_ref().is_some_and(|space| space_map.contains_key(space)) => {
                    Disposition::RepairSpace
                }
                Some(_) => Disposition::Skip,
            };
            match disposition {
                Disposition::Skip | Disposition::RepairSpace => report.skipped_existing += 1,
                Disposition::Remote => report.remote_hosted_chats += 1,
                Disposition::Rehome | Disposition::RehomeExisting => report.rehomed_chats += 1,
            }
            if disposition != Disposition::Skip {
                plan.push((chat, disposition));
            }
        }
        if options.validate_only {
            report.spaces_imported = spaces_to_import.len();
            report.chats_refiled = plan.iter().filter(|(_, d)| *d == Disposition::RepairSpace).count();
            return Ok(report);
        }

        for space in &spaces_to_import {
            match self.workspace.import_space_row(space) {
                Ok(()) => report.spaces_imported += 1,
                Err(err) => report.errors.push(format!("space {}: {err}", space.id)),
            }
        }

        // Known outcomes and at-most-once claims carry over first; outcome-less
        // claims for still-Pending commands are added per document below.
        report.ledger_rows_merged = source
            .store
            .processed_commands()
            .and_then(|rows| target_store.import_processed_commands(&rows))
            .unwrap_or_else(|err| {
                report.errors.push(format!("command ledger: {err}"));
                0
            });

        let source_journals = source.root.join("journals");
        let target_journals = target_root.join("journals");
        let mut imported = Vec::new();
        for (chat, disposition) in plan {
            if disposition == Disposition::RepairSpace {
                let refiled = self.workspace.chat(&chat.id).and_then(|row| {
                    let Some(mut row) = row else { return Ok(false) };
                    row.space_id = row.space_id.as_ref().and_then(|space| space_map.get(space)).cloned().or(row.space_id);
                    self.workspace.import_chat_row(&row).map(|()| true)
                });
                match refiled {
                    Ok(true) => report.chats_refiled += 1,
                    Ok(false) => {}
                    Err(err) => report.errors.push(format!("chat {}: {err}", chat.id)),
                }
                continue;
            }
            match self
                .import_chat(&source, &target_store, &chat, &disposition, &source_journals, &target_journals,
                    &space_map, &mut report)
                .await
            {
                Ok(()) => {
                    report.imported_chats += 1;
                    imported.push(chat.id.clone());
                }
                Err(err) => report.errors.push(format!("chat {}: {err}", chat.id)),
            }
        }

        if let Some(uploads) = &options.source_uploads {
            if uploads.is_absolute() && uploads.is_dir() {
                self.uploads.add_read_only_root(uploads);
                if let Err(err) = record_upload_root(&target_root, uploads) {
                    report.errors.push(format!("uploads root: {err}"));
                }
            } else {
                report.errors.push(format!("uploads root {} is not an absolute directory", uploads.display()));
            }
        }

        report.unacknowledged = self.await_acknowledged(&imported).await;
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    async fn import_chat(
        &self,
        source: &Arc<NativeProfileSource>,
        target_store: &Arc<DocsStore>,
        chat: &Chat,
        disposition: &Disposition,
        source_journals: &Path,
        target_journals: &Path,
        space_map: &BTreeMap<String, String>,
        report: &mut NativeImportReport,
    ) -> Result<(), EngineError> {
        let rehome_existing = *disposition == Disposition::RehomeExisting;
        let mut documents = vec![chat.id.clone()];
        documents.extend(source.store.subagent_snapshot_ids(&chat.id)?);
        if rehome_existing && documents.iter().any(|id| self.doc_host.has_open_document(id)) {
            return Err(EngineError::Other("chat is open in this engine; close it and re-run".into()));
        }

        // Documents first (merged + neutralized), then journals, then the row.
        // A document needs publication when this run stored it, or when an
        // earlier run stored it but its seeding POST never succeeded (the
        // outbox marker is set only after a successful publish).
        let mut publish = Vec::new();
        for doc_id in &documents {
            if !rehome_existing && target_store.has_snapshot(doc_id)? {
                if !target_store.chat_outbox_initialized(doc_id)? {
                    publish.push(doc_id.clone());
                }
                continue; // never overwrite a replica this profile already owns
            }
            let source_store = source.clone();
            let target = target_store.clone();
            let id = doc_id.clone();
            let merged = tokio::task::spawn_blocking(move || {
                merge_document(&source_store.store, &target, &id, rehome_existing)
            })
            .await
            .map_err(|err| EngineError::Other(err.to_string()))??;
            let Some((bytes, pending)) = merged else { continue };
            let now = crate::now_ms();
            let claims: Vec<_> = pending.into_iter().map(|id| (id, now, None)).collect();
            report.pending_commands_neutralized += target_store.import_processed_commands(&claims)?;
            target_store.save_snapshot_with_cursor(doc_id, &bytes, 0, CHAT2_DOC_EPOCH)?;
            publish.push(doc_id.clone());
        }

        if chat.device_id == source.device_id {
            let (src_journal, src_resume) = journal_paths(source_journals, &chat.id);
            let (dst_journal, dst_resume) = journal_paths(target_journals, &chat.id);
            for (src, dst) in [(src_journal, dst_journal), (src_resume, dst_resume)] {
                if src.is_file() && !dst.exists() {
                    std::fs::create_dir_all(target_journals)?;
                    std::fs::copy(&src, &dst)?;
                    report.journals_copied += 1;
                }
            }
        }

        // Seed this Edge's rooms before the row becomes visible anywhere.
        for doc_id in &publish {
            self.doc_host
                .publish_stored_snapshot(doc_id)
                .await
                .map_err(|err| EngineError::Other(format!("publish {doc_id}: {err}")))?;
            report.published_documents += 1;
        }

        let row = rebuild_row(chat, &source.device_id, self.workspace.device_id(), &self.target, space_map);
        self.workspace.import_chat_row(&row)?;
        Ok(())
    }

    async fn await_acknowledged(&self, chats: &[String]) -> Vec<String> {
        let pending = |workspace: &WorkspaceHost| -> Vec<String> {
            chats
                .iter()
                .filter(|id| !workspace.acknowledged_chat(id).is_some_and(|row| row.seq > 0 && !row.deleted))
                .cloned()
                .collect()
        };
        let mut changes = self.workspace.watch_registry();
        let waited = tokio::time::timeout(ACK_DEADLINE, async {
            loop {
                changes.borrow_and_update();
                if pending(&self.workspace).is_empty() {
                    return;
                }
                if changes.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        let _ = waited;
        pending(&self.workspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loro::ToJson;

    fn chat(id: &str, device: &str) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": id, "deviceId": device, "archived": false, "createdAt": "2026-11-01T00:00:00Z",
            "spaceId": "space-src", "roomGen": 2,
            "crew": {
                "ownerSubject": "owner@example.com",
                "scope": {"projectId": "crew-upstream-canary", "deploymentId": "crew-upstream-canary",
                    "sessionId": id},
                "sessionId": id
            }
        }))
        .expect("chat fixture")
    }

    fn target() -> NativeImportTarget {
        NativeImportTarget {
            owner_subject: "owner@example.com".into(),
            project_id: "ashler-staging".into(),
            deployment_id: Some("ashler-staging".into()),
        }
    }

    #[test]
    fn rebuilt_rows_rehome_source_device_and_bind_target_identity() {
        let id = "9c1b5d1e-1111-4222-8333-444455556666";
        let spaces = BTreeMap::from([("space-src".to_string(), "space-dst".to_string())]);
        let row = rebuild_row(&chat(id, "source-device"), "source-device", "target-device", &target(), &spaces);
        assert_eq!(row.device_id, "target-device");
        assert_eq!(row.space_id.as_deref(), Some("space-dst"));
        assert_eq!(row.room_gen, Some(CHAT2_DOC_EPOCH));
        let crew = row.crew.expect("cloud session keeps crew metadata");
        assert_eq!(crew.scope.project_id, "ashler-staging");
        assert_eq!(crew.scope.deployment_id.as_deref(), Some("ashler-staging"));
        assert_eq!(crew.scope.session_id.as_deref(), Some(id));
        assert!(crew.environment.is_none() && crew.agent_route.is_none());
        crew.validate().expect("rebuilt metadata validates");
    }

    #[test]
    fn rebuilt_rows_keep_foreign_hosts_inert() {
        let id = "9c1b5d1e-1111-4222-8333-444455556667";
        let row = rebuild_row(&chat(id, "devbox-device"), "source-device", "target-device", &target(), &BTreeMap::new());
        assert_eq!(row.device_id, "devbox-device");
        assert_eq!(row.space_id.as_deref(), Some("space-src"));
    }

    #[test]
    fn merged_documents_include_outbox_and_report_pending_commands() {
        let dir = tempfile::tempdir().unwrap();
        let source = DocsStore::open(dir.path().join("source")).unwrap();
        let target = DocsStore::open(dir.path().join("target")).unwrap();
        let id = "9c1b5d1e-1111-4222-8333-444455556668";
        let doc = SessionDoc::init(id).unwrap();
        source.save_snapshot(id, &doc.doc().export(loro::ExportMode::Snapshot).unwrap()).unwrap();
        let before = doc.doc().oplog_vv();
        doc.doc().get_map("probe").insert("outbox", "only-in-outbox").unwrap();
        doc.doc().commit();
        let update = doc.doc().export(loro::ExportMode::updates(&before)).unwrap();
        source.enqueue_chat_update(id, "batch-1", &update).unwrap();

        let (bytes, pending) = merge_document(&source, &target, id, false).unwrap().expect("document");
        let merged = loro::LoroDoc::new();
        merged.import(&bytes).unwrap();
        assert_eq!(
            merged.get_map("probe").get_deep_value().to_json_value(),
            serde_json::json!({"outbox": "only-in-outbox"}),
            "unacknowledged outbox rows must be carried"
        );
        assert!(pending.is_empty());
        assert!(merge_document(&source, &target, "absent", false).unwrap().is_none());
    }

    #[test]
    fn rehomed_space_ids_are_stable_per_device_and_uuid_shaped() {
        let a = rehomed_space_id("adbfbe9c-e369-4036-a386-d8366a119092", "device-a");
        assert_eq!(a, rehomed_space_id("adbfbe9c-e369-4036-a386-d8366a119092", "device-a"));
        assert_ne!(a, rehomed_space_id("adbfbe9c-e369-4036-a386-d8366a119092", "device-b"));
        assert_ne!(a, "adbfbe9c-e369-4036-a386-d8366a119092");
        assert!(uuid::Uuid::parse_str(&a).is_ok(), "{a}");
    }

    #[test]
    fn upload_root_marker_round_trips_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(&uploads).unwrap();
        record_upload_root(dir.path(), &uploads).unwrap();
        record_upload_root(dir.path(), &uploads).unwrap();
        assert_eq!(marker_read_roots(dir.path()), vec![uploads]);
    }

    #[test]
    fn live_sources_and_self_imports_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        DocsStore::open(&source).unwrap();
        // The just-opened store left a WAL behind only if non-empty; force one.
        std::fs::write(source.join("docs.sqlite3-wal"), b"live").unwrap();
        let err = NativeProfileSource::open(&source, &dir.path().join("target")).err().expect("refused");
        assert!(err.to_string().contains("write-ahead log"), "{err}");
        std::fs::remove_file(source.join("docs.sqlite3-wal")).unwrap();
        let err = NativeProfileSource::open(&source, &source).err().expect("refused");
        assert!(err.to_string().contains("own profile"), "{err}");
        let err = NativeProfileSource::open(&source, &dir.path().join("target")).err().expect("refused");
        assert!(err.to_string().contains("registry"), "{err}");
    }
}
