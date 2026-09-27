//! The new-session canvas owns a draft identity, independent of chat routing.
//! Debounced writes are serialized; navigation flushes the captured old target.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use zeron_proto::{DraftAsset, DraftAttachment, DraftBundle, DraftContent, PromptDraft, SaveDraft};

#[derive(Clone)]
pub(super) struct EditingDraft {
    pub id: String,
    pub revision: Option<String>,
    pub base_revision: Option<String>,
    pub created_at: i64,
    pub reserved: bool,
    pub deferred: bool,
    pub saved_deferred: bool,
    pub saved: Option<DraftContent>,
    pub snapshot: DraftBundle,
}
#[derive(Default)]
pub(crate) struct DraftQuitSavers(pub Vec<gpui::WeakEntity<Composer>>);
impl gpui::Global for DraftQuitSavers {}

pub(crate) fn quit_saves(
    cx: &mut App,
) -> Vec<(
    EngineHandle,
    SaveDraft,
    Arc<tokio::sync::Mutex<std::collections::HashSet<String>>>,
)> {
    let editors = cx.default_global::<DraftQuitSavers>().0.clone();
    editors
        .into_iter()
        .filter_map(|editor| {
            editor
                .update(cx, |composer, cx| {
                    if composer.current_key.is_empty() {
                        composer.capture_prompt_draft(cx);
                    }
                    let request = composer.park_prompt_draft(cx)?;
                    Some((
                        composer.prompt_draft_engine.clone()?,
                        request,
                        composer.prompt_draft_gate.clone(),
                    ))
                })
                .ok()
                .flatten()
        })
        .collect()
}

pub(crate) async fn save_request(
    engine: &EngineHandle,
    mut request: SaveDraft,
    known: &mut std::collections::HashSet<String>,
) -> Result<(), String> {
    for asset in std::mem::take(&mut request.assets) {
        if known.contains(&asset.blob) {
            continue;
        }
        let bytes = STANDARD.decode(&asset.data).map_err(|e| e.to_string())?;
        for index in 0..bytes.len().div_ceil(zeron_proto::DRAFT_CHUNK_BYTES).max(1) {
            let offset = index * zeron_proto::DRAFT_CHUNK_BYTES;
            let chunk = zeron_proto::DraftAssetChunk {
                blob: asset.blob.clone(),
                index,
                total_bytes: bytes.len(),
                data: STANDARD.encode(
                    &bytes[offset..(offset + zeron_proto::DRAFT_CHUNK_BYTES).min(bytes.len())],
                ),
            };
            engine
                .client()
                .call(
                    methods::SAVE_DRAFT_ASSET,
                    serde_json::to_value(chunk).unwrap(),
                )
                .await
                .map_err(|e| e.to_string())?;
        }
        known.insert(asset.blob);
    }
    engine
        .client()
        .call(methods::SAVE_DRAFT, serde_json::to_value(&request).unwrap())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn load_bundle(engine: &EngineHandle, revision: &str) -> Result<DraftBundle, String> {
    let value = engine
        .client()
        .call(
            methods::LOAD_DRAFT,
            serde_json::json!({"revision": revision}),
        )
        .await
        .map_err(|e| e.to_string())?;
    let content: DraftContent = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let mut assets = Vec::<DraftAsset>::new();
    for asset in &content.attachments {
        if assets.iter().any(|a| a.blob == asset.blob) {
            continue;
        }
        let mut bytes = Vec::new();
        for index in 0..=zeron_proto::MAX_DRAFT_ASSET_BYTES / zeron_proto::DRAFT_CHUNK_BYTES {
            let value = engine
                .client()
                .call(
                    methods::LOAD_DRAFT_ASSET,
                    serde_json::json!({"blob": asset.blob, "index": index}),
                )
                .await
                .map_err(|e| e.to_string())?;
            let chunk: zeron_proto::DraftAssetChunk =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
            if chunk.total_bytes > zeron_proto::MAX_DRAFT_ASSET_BYTES
                || chunk.index != index
                || chunk.blob != asset.blob
            {
                return Err("Invalid draft attachment response".into());
            }
            bytes.extend(STANDARD.decode(&chunk.data).map_err(|e| e.to_string())?);
            if bytes.len() >= chunk.total_bytes {
                break;
            }
        }
        if format!("{:x}", Sha256::digest(&bytes)) != asset.blob {
            return Err("Draft attachment checksum mismatch".into());
        }
        assets.push(DraftAsset {
            blob: asset.blob.clone(),
            data: STANDARD.encode(bytes),
        });
    }
    Ok(DraftBundle { content, assets })
}

fn restore_images(bundle: &DraftBundle) -> Result<Vec<attachments::StagedAttachment>, String> {
    bundle
        .content
        .attachments
        .iter()
        .map(|asset| {
            let bytes = bundle
                .assets
                .iter()
                .find(|b| b.blob == asset.blob)
                .ok_or_else(|| format!("Missing draft attachment: {}", asset.name))?;
            let bytes = STANDARD.decode(&bytes.data).map_err(|e| e.to_string())?;
            let format = attachments::format_by_extension(std::path::Path::new(&asset.name))
                .ok_or_else(|| format!("Unsupported draft image: {}", asset.name))?;
            if let Some(meta) = &asset.appshot {
                serde_json::from_value::<appshots::AccessibilitySnapshot>(
                    meta["accessibility"].clone(),
                )
                .map_err(|e| format!("Invalid Appshot context: {e}"))?;
            }
            Ok(attachments::StagedAttachment {
                id: asset.id.clone(),
                name: asset.name.clone(),
                image: Arc::new(gpui::Image::from_bytes(format, bytes)),
            })
        })
        .collect()
}

fn recovery_directory(state: &AppState) -> Option<std::path::PathBuf> {
    let engine = state.engine()?;
    let profile = crate::settings::sidebar_pin_profile_key(
        Some(engine.engine_info().workspace_scope),
        state.auth.as_ref(),
        None,
    )?;
    let identity = format!("{}:{profile}", engine.engine_info().device_id);
    Some(
        state
            .data_dir
            .as_ref()?
            .join("draft-recovery")
            .join(format!("{:x}", Sha256::digest(identity.as_bytes()))),
    )
}
fn write_recovery_file(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok::<_, std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(|e| e.to_string())
}
type RecoveryMemory =
    std::collections::HashMap<std::path::PathBuf, std::collections::HashMap<String, SaveDraft>>;
fn recovery_memory() -> &'static std::sync::Mutex<RecoveryMemory> {
    static MEMORY: std::sync::OnceLock<std::sync::Mutex<RecoveryMemory>> =
        std::sync::OnceLock::new();
    MEMORY.get_or_init(Default::default)
}
fn stage_recovery(directory: &std::path::Path, request: &SaveDraft) -> Result<(), String> {
    let result = stage_recovery_on_disk(directory, request);
    if result.is_err() {
        recovery_memory()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(directory.to_owned())
            .or_default()
            .insert(request.publication_key(), request.clone());
    }
    result
}
fn stage_recovery_on_disk(directory: &std::path::Path, request: &SaveDraft) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    for asset in &request.assets {
        let path = directory.join(format!("{}.asset", asset.blob));
        if !path.exists() {
            write_recovery_file(&path, asset.data.as_bytes())?;
        }
    }
    let mut manifest = request.clone();
    manifest.assets.clear();
    let bytes = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    write_recovery_file(
        &directory.join(format!("{}.json", request.publication_key())),
        &bytes,
    )?;
    if request.deferred {
        write_recovery_file(&directory.join(format!("{}.canvas", request.id)), &bytes)?;
    } else {
        clear_canvas_checkpoint(directory, &request.id);
    }
    Ok(())
}
pub(super) fn clear_canvas_checkpoint(directory: &std::path::Path, id: &str) {
    let _ = std::fs::remove_file(directory.join(format!("{id}.canvas")));
}
fn recover_abandoned_canvases(directory: &std::path::Path, active_id: Option<&str>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("canvas") {
            continue;
        }
        let recover = || -> Option<()> {
            let mut request: SaveDraft =
                serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()?;
            if active_id == Some(request.id.as_str()) {
                return None;
            }
            for asset in &request.content.attachments {
                request.assets.push(DraftAsset {
                    blob: asset.blob.clone(),
                    data: std::fs::read_to_string(directory.join(format!("{}.asset", asset.blob)))
                        .ok()?,
                });
            }
            request.deferred = false;
            stage_recovery(directory, &request).ok()
        };
        recover();
    }
}
fn pending_recovery(directory: &std::path::Path) -> Vec<SaveDraft> {
    let mut requests = std::collections::HashMap::<String, SaveDraft>::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let request = (|| {
                let bytes = std::fs::read(entry.path()).ok()?;
                let mut request: SaveDraft = serde_json::from_slice(&bytes).ok()?;
                for asset in &request.content.attachments {
                    request.assets.push(DraftAsset {
                        blob: asset.blob.clone(),
                        data: std::fs::read_to_string(
                            directory.join(format!("{}.asset", asset.blob)),
                        )
                        .ok()?,
                    });
                }
                Some(request)
            })();
            if let Some(request) = request {
                requests.insert(request.publication_key(), request);
            }
        }
    }
    if let Some(memory) = recovery_memory()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(directory)
    {
        requests.extend(memory.clone());
    }
    let mut ordered = Vec::with_capacity(requests.len());
    while !requests.is_empty() {
        let next = requests
            .values()
            .find(|request| {
                !request
                    .base_revision
                    .as_ref()
                    .is_some_and(|base| requests.values().any(|pending| &pending.revision == base))
            })
            .map(|request| request.publication_key());
        let Some(next) = next else { break }; // corrupt ancestry must not invent a publication order
        ordered.push(requests.remove(&next).unwrap());
    }
    ordered
}
fn acknowledge_recovery(directory: &std::path::Path, revision: &str) {
    let _ = std::fs::remove_file(directory.join(format!("{revision}.json")));
    if let Some(memory) = recovery_memory()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(directory)
    {
        memory.remove(revision);
    }
}

impl Composer {
    pub(super) fn resume_prompt_draft_saves(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let Some(directory) = recovery_directory(state) else {
            return;
        };
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        if !engine
            .engine_info()
            .supports(zeron_proto::DRAFTS_CAPABILITY)
        {
            return;
        }
        if self.prompt_draft_recovery.as_ref() == Some(&directory)
            && self.prompt_draft_recovery_task.is_some()
        {
            return;
        }
        self.prompt_draft_recovery = Some(directory.clone());
        let gate = self.prompt_draft_gate.clone();
        self.prompt_draft_recovery_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let active = this
                    .update(cx, |composer, cx| {
                        let state = composer.state.read(cx);
                        (recovery_directory(state).as_ref() == Some(&directory))
                            .then(|| {
                                state.engine().cloned().map(|engine| {
                                    (engine, composer.active_prompt_draft().map(str::to_owned))
                                })
                            })
                            .flatten()
                    })
                    .ok()
                    .flatten();
                let Some((engine, active_id)) = active else {
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                    continue;
                };
                recover_abandoned_canvases(&directory, active_id.as_deref());
                let pending = pending_recovery(&directory);
                for request in pending {
                    let mut known = gate.lock().await;
                    match save_request(&engine, request.clone(), &mut known).await {
                        Ok(()) => acknowledge_recovery(&directory, &request.publication_key()),
                        Err(error) if error.contains("already discarded") => {
                            acknowledge_recovery(&directory, &request.publication_key())
                        }
                        Err(_) => break,
                    }
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        }));
    }
    pub(super) fn reserve_prompt_draft_attempt(
        &mut self,
        request: &SaveDraft,
    ) -> Result<(), String> {
        if let Some(directory) = &self.prompt_draft_recovery {
            std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
            write_recovery_file(
                &directory.join(format!("{}.reservation", request.id)),
                request.revision.as_bytes(),
            )?;
        }
        if let Some(draft) = &mut self.prompt_draft {
            if draft.id == request.id {
                draft.reserved = true;
            }
        }
        Ok(())
    }

    pub(super) fn cancel_prompt_draft_load(&mut self) {
        if self.prompt_draft_loading {
            self.prompt_draft_loading = false;
            self.prompt_draft_load_generation += 1;
        }
    }

    pub(crate) fn active_prompt_draft(&self) -> Option<&str> {
        self.prompt_draft.as_ref().map(|d| d.id.as_str())
    }

    pub(super) fn capture_prompt_draft(&mut self, cx: &mut Context<Self>) {
        if !self.current_key.is_empty() || self.sending || self.prompt_draft_loading {
            return;
        }
        let state = self.state.read(cx);
        if state.selected_chat.is_some()
            || !state
                .engine()
                .is_some_and(|e| e.engine_info().supports(zeron_proto::DRAFTS_CAPABILITY))
        {
            return;
        }
        self.prompt_draft_engine = state.engine().cloned();
        self.resume_prompt_draft_saves(cx);
        let target = self.pickers.read(cx).prompt_draft_target(cx);
        let mut content = DraftContent {
            prompt: self.input.read(cx).text().to_string(),
            target,
            attachments: vec![],
        };
        let assets = Vec::new();
        for image in self.attachments.get("").into_iter().flatten() {
            append_asset(&mut content, &mut self.prompt_draft_assets, image, None);
        }
        for appshot in self.appshots.get("").into_iter().flatten() {
            append_asset(
                &mut content,
                &mut self.prompt_draft_assets,
                &appshot.screenshot,
                Some(serde_json::json!({
                    "id": appshot.id, "appName": appshot.app_name, "bundleIdentifier": appshot.bundle_identifier,
                    "windowTitle": appshot.window_title, "accessibility": appshot.accessibility,
                    "capturedAt": appshot.captured_at, "dimensions": appshot.screenshot_dimensions,
                })),
            );
        }
        if !content.has_content() && self.prompt_draft.is_none() {
            return;
        }
        let editor = self.prompt_draft.get_or_insert_with(|| EditingDraft {
            id: uuid::Uuid::new_v4().to_string(),
            revision: None,
            base_revision: None,
            created_at: chrono::Utc::now().timestamp_millis(),
            reserved: false,
            deferred: true,
            saved_deferred: true,
            saved: None,
            snapshot: DraftBundle::default(),
        });
        if editor.snapshot.content == content {
            return;
        }
        if editor.reserved {
            // Editing is a new intent; keep the exact uncertain send retryable.
            editor.id = uuid::Uuid::new_v4().to_string();
            editor.revision = None;
            editor.base_revision = None;
            editor.created_at = chrono::Utc::now().timestamp_millis();
            editor.saved = None;
            editor.reserved = false;
            editor.deferred = true;
            editor.saved_deferred = true;
        }

        editor.snapshot = DraftBundle { content, assets };
        self.prompt_draft_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            this.update(cx, |composer, cx| {
                composer.flush_prompt_draft(cx);
            })
            .ok();
        }));
    }

    pub(crate) fn flush_prompt_draft(&mut self, cx: &mut Context<Self>) -> Option<SaveDraft> {
        let engine = self
            .prompt_draft_engine
            .clone()
            .or_else(|| self.state.read(cx).engine().cloned())?;
        let editor = self.prompt_draft.as_mut()?;
        if !editor.snapshot.content.has_content() {
            let id = editor.id.clone();
            if let Some(directory) = &self.prompt_draft_recovery {
                clear_canvas_checkpoint(directory, &id);
            }
            let gate = self.prompt_draft_gate.clone();
            if editor.revision.is_some() {
                cx.spawn(async move |_, _| {
                    let _guard = gate.lock().await;
                    let _ = engine
                        .client()
                        .call(
                            methods::CHANGE_DRAFT,
                            serde_json::json!({ "action": "discard", "id": id }),
                        )
                        .await;
                })
                .detach();
            }
            self.prompt_draft = None;
            return None;
        }
        let content_changed = editor.saved.as_ref() != Some(&editor.snapshot.content);
        let changed = content_changed || editor.saved_deferred != editor.deferred;

        if content_changed || editor.revision.is_none() {
            editor.base_revision = editor.revision.clone();
            editor.revision = Some(uuid::Uuid::new_v4().to_string());
            editor.saved = Some(editor.snapshot.content.clone());
        }
        editor.saved_deferred = editor.deferred;
        let request = SaveDraft {
            deferred: editor.deferred,
            id: editor.id.clone(),
            revision: editor.revision.clone()?,
            base_revision: editor.base_revision.clone(),
            created_at: editor.created_at,
            content: editor.snapshot.content.clone(),
            assets: editor
                .snapshot
                .content
                .attachments
                .iter()
                .filter_map(|a| self.prompt_draft_assets.get(&a.id).cloned())
                .collect(),
        };
        if changed {
            let recovery = self.prompt_draft_recovery.clone();
            if let Some(directory) = &recovery {
                if let Err(error) = stage_recovery(directory, &request) {
                    self.failure = Some(
                        format!("Draft retained in memory; recovery storage failed: {error}")
                            .into(),
                    );
                    self.failure_key = Some(String::new());
                    cx.notify();
                }
            }
            let request = request.clone();
            let gate = self.prompt_draft_gate.clone();
            cx.spawn(async move |this, cx| {
                let mut known = gate.lock().await;
                let result = save_request(&engine, request.clone(), &mut known).await;
                if result.is_ok() {
                    if let Some(directory) = &recovery {
                        acknowledge_recovery(directory, &request.publication_key());
                    }
                }
                this.update(cx, |composer, cx| {
                    if let Err(error) = result {
                        composer.failure = Some(format!("Couldn't save draft: {error}").into());
                        composer.failure_key = Some(String::new());
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Some(request)
    }

    pub(crate) fn park_prompt_draft(&mut self, cx: &mut Context<Self>) -> Option<SaveDraft> {
        if let Some(editor) = &mut self.prompt_draft {
            editor.deferred = false;
        }
        self.flush_prompt_draft(cx)
    }

    pub(crate) fn start_prompt_draft(&mut self, cx: &mut Context<Self>) {
        if !self
            .state
            .read(cx)
            .engine()
            .is_some_and(|e| e.engine_info().supports(zeron_proto::DRAFTS_CAPABILITY))
        {
            return;
        }
        if self.current_key.is_empty() {
            self.capture_prompt_draft(cx);
            self.park_prompt_draft(cx);
        }
        self.abandon_prompt_draft(cx);
    }

    pub(crate) fn abandon_prompt_draft(&mut self, cx: &mut Context<Self>) {
        self.prompt_draft_debounce = None;
        self.prompt_draft_loading = false;
        self.prompt_draft_load_generation += 1;
        self.prompt_draft = None;
        self.drafts.remove("");
        self.attachments.remove("");
        self.appshots.remove("");
        if self.current_key.is_empty() {
            self.input.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    pub(crate) fn open_prompt_draft(&mut self, row: PromptDraft, cx: &mut Context<Self>) {
        if self.current_key.is_empty() && self.active_prompt_draft() == Some(&row.id) {
            self.focus_pending = true;
            cx.notify();
            return;
        }
        let origin_chat = self.state.read(cx).selected_chat.clone();
        if self.current_key.is_empty() {
            self.capture_prompt_draft(cx);
            self.park_prompt_draft(cx);
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.prompt_draft_debounce = None;
        self.prompt_draft_load_generation += 1;
        let generation = self.prompt_draft_load_generation;
        self.prompt_draft_loading = true;
        cx.spawn(async move |this, cx| {
            let result = load_bundle(&engine, &row.revision)
                .await
                .and_then(|bundle| restore_images(&bundle).map(|images| (bundle, images)));
            this.update(cx, |composer, cx| {
                if composer.state.read(cx).selected_chat != origin_chat
                    || composer.prompt_draft_load_generation != generation
                    || !composer
                        .state
                        .read(cx)
                        .engine()
                        .is_some_and(|e| e.same_connection(&engine))
                {
                    return;
                }
                composer.prompt_draft_loading = false;
                match result {
                    Err(error) => {
                        composer.failure = Some(format!("Couldn't open draft: {error}").into());
                        composer.failure_key = None;
                    }
                    Ok((bundle, images)) => {
                        composer.prompt_draft = None;
                        composer.state.update(cx, |state, cx| {
                            state.select_chat(None, cx);
                            state.select_space(bundle.content.target.space_id.clone(), cx);
                            state.selected_device = Some(bundle.content.target.device_id.clone());
                            cx.notify();
                        });
                        composer.on_state_changed(cx);
                        composer.pickers.update(cx, |pickers, cx| {
                            pickers.restore_prompt_draft_target(&bundle.content.target, cx)
                        });
                        composer.attachments.remove("");
                        composer.appshots.remove("");
                        for (asset, image) in bundle.content.attachments.iter().zip(images) {
                            if let Some(data) = bundle.assets.iter().find(|b| b.blob == asset.blob)
                            {
                                composer
                                    .prompt_draft_assets
                                    .insert(asset.id.clone(), data.clone());
                            }
                            if let Some(meta) = &asset.appshot {
                                if let Ok(accessibility) =
                                    serde_json::from_value(meta["accessibility"].clone())
                                {
                                    let appshot = CapturedAppshot {
                                        id: meta["id"].as_str().unwrap_or(&asset.id).into(),
                                        app_name: meta["appName"]
                                            .as_str()
                                            .unwrap_or("Application")
                                            .into(),
                                        bundle_identifier: meta["bundleIdentifier"]
                                            .as_str()
                                            .map(str::to_owned),
                                        window_title: meta["windowTitle"]
                                            .as_str()
                                            .map(str::to_owned),
                                        accessibility,
                                        screenshot_dimensions: serde_json::from_value(
                                            meta["dimensions"].clone(),
                                        )
                                        .ok()
                                        .flatten(),
                                        screenshot: image,
                                        app_icon: None,
                                        captured_at: serde_json::from_value(
                                            meta["capturedAt"].clone(),
                                        )
                                        .unwrap_or_else(|_| chrono::Utc::now()),
                                    };
                                    composer
                                        .appshots
                                        .entry(String::new())
                                        .or_default()
                                        .push(appshot);
                                    continue;
                                }
                            }
                            composer
                                .attachments
                                .entry(String::new())
                                .or_default()
                                .push(image);
                        }
                        composer.prompt_draft = Some(EditingDraft {
                            deferred: false,
                            saved_deferred: false,
                            reserved: composer.prompt_draft_recovery.as_ref().is_some_and(|d| {
                                d.join(format!("{}.reservation", row.id)).exists()
                            }),
                            id: row.id,
                            revision: Some(row.revision),
                            base_revision: row.base_revision,
                            created_at: row.created_at,
                            saved: Some(bundle.content.clone()),
                            snapshot: bundle.clone(),
                        });
                        composer
                            .drafts
                            .insert(String::new(), bundle.content.prompt.clone());
                        composer
                            .input
                            .update(cx, |input, cx| input.set_text(bundle.content.prompt, cx));
                        composer.failure = None;
                        composer.focus_pending = true;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}
fn append_asset(
    content: &mut DraftContent,
    cache: &mut HashMap<String, DraftAsset>,
    image: &StagedAttachment,
    appshot: Option<serde_json::Value>,
) {
    let asset = cache.entry(image.id.clone()).or_insert_with(|| DraftAsset {
        blob: format!("{:x}", Sha256::digest(image.bytes())),
        data: STANDARD.encode(image.bytes()),
    });
    content.attachments.push(DraftAttachment {
        id: image.id.clone(),
        name: image.name.clone(),
        blob: asset.blob.clone(),
        appshot,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unacknowledged_saves_survive_navigation_and_reopening_the_recovery_queue() {
        let directory = tempfile::tempdir().unwrap();
        let request = SaveDraft {
            deferred: false,
            id: "a".into(),
            revision: "a1".into(),
            base_revision: None,
            created_at: 1,
            content: DraftContent {
                prompt: "Must survive failed RPC".into(),
                ..Default::default()
            },
            assets: vec![],
        };
        stage_recovery(directory.path(), &request).unwrap();
        drop(request);
        let restored = pending_recovery(directory.path());
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].content.prompt, "Must survive failed RPC");
        acknowledge_recovery(directory.path(), "a1");
        assert!(pending_recovery(directory.path()).is_empty());
    }

    #[test]
    fn failed_recovery_storage_keeps_requests_and_replays_ancestors_first() {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let parent = SaveDraft {
            deferred: false,
            id: "a".into(),
            revision: "z-parent".into(),
            base_revision: None,
            created_at: 1,
            content: DraftContent {
                prompt: "Parent".into(),
                ..Default::default()
            },
            assets: vec![],
        };
        let mut child = parent.clone();
        child.revision = "a-child".into();
        child.base_revision = Some(parent.revision.clone());
        assert!(stage_recovery(&blocked, &child).is_err());
        assert!(stage_recovery(&blocked, &parent).is_err());
        let restored = pending_recovery(&blocked);
        assert_eq!(
            restored
                .iter()
                .map(|r| r.revision.as_str())
                .collect::<Vec<_>>(),
            ["z-parent", "a-child"]
        );
        assert!(pending_recovery(&directory.path().join("another-profile")).is_empty());
        acknowledge_recovery(&blocked, "z-parent");
        acknowledge_recovery(&blocked, "a-child");
        assert!(pending_recovery(&blocked).is_empty());
        // The disk path uses the same causal ordering regardless of filenames/read_dir.
        stage_recovery(directory.path(), &child).unwrap();
        stage_recovery(directory.path(), &parent).unwrap();
        assert_eq!(pending_recovery(directory.path())[0].revision, "z-parent");
    }

    #[test]
    fn canvas_checkpoint_is_recovered_only_after_its_editor_is_gone() {
        let directory = tempfile::tempdir().unwrap();
        let request = SaveDraft {
            deferred: true,
            id: "a".into(),
            revision: "v1".into(),
            base_revision: None,
            created_at: 1,
            content: DraftContent {
                prompt: "Unfinished canvas".into(),
                ..Default::default()
            },
            assets: vec![],
        };
        stage_recovery(directory.path(), &request).unwrap();
        acknowledge_recovery(directory.path(), &request.publication_key());
        recover_abandoned_canvases(directory.path(), Some("a"));
        assert!(pending_recovery(directory.path()).is_empty());
        recover_abandoned_canvases(directory.path(), None);
        let recovered = pending_recovery(directory.path());
        assert_eq!(recovered.len(), 1);
        assert!(!recovered[0].deferred);
        assert_eq!(recovered[0].revision, request.revision);
        assert!(!directory.path().join("a.canvas").exists());
    }

    #[test]
    fn restored_images_keep_their_format_and_missing_content_is_an_error() {
        let mut bundle = DraftBundle {
            content: DraftContent {
                attachments: vec![DraftAttachment {
                    id: "jpeg".into(),
                    name: "photo.jpg".into(),
                    blob: "blob".into(),
                    appshot: None,
                }],
                ..Default::default()
            },
            assets: vec![DraftAsset {
                blob: "blob".into(),
                data: STANDARD.encode(b"jpeg bytes"),
            }],
        };
        let images = restore_images(&bundle).unwrap();
        assert_eq!(images[0].image.format, gpui::ImageFormat::Jpeg);
        assert_eq!(images[0].id, "jpeg");
        bundle.assets.clear();
        assert!(restore_images(&bundle).is_err());
    }

    #[gpui::test]
    fn draft_revision_chain_and_new_canvas_preserve_independent_work(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, _requests) = tokio::sync::mpsc::channel(32);
        let (_replies, inbound) = tokio::sync::mpsc::channel(32);
        let engine = EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound))
            .with_test_capability(zeron_proto::DRAFTS_CAPABILITY);
        let (_dir, window) = crate::composer::tests::composer_focus_window(cx);
        window
            .update(cx, |composer, _, cx| {
                composer.state.update(cx, |state, _| {
                    state.set_test_engine(engine);
                    state.local_device_id = Some("local".into());
                    state.spaces = vec![zeron_proto::Space {
                        id: "project".into(),
                        device_id: "remote".into(),
                        path: "/workspace/my-project".into(),
                        name: None,
                        git_detected: true,
                        git_checked_at: None,
                        checkout_id: None,
                        created_at: chrono::Utc::now(),
                    }];
                    state.selected_space = Some("project".into());
                    state.no_project = false;
                });
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("First prompt", cx));
                composer.capture_prompt_draft(cx);
                let first = composer.flush_prompt_draft(cx).unwrap();
                assert_eq!(first.content.prompt, "First prompt");
                assert_eq!(first.content.target.space_id.as_deref(), Some("project"));
                assert_eq!(
                    first.content.target.project_name.as_deref(),
                    Some("my-project")
                );
                assert_eq!(first.content.target.device_id, "remote");
                assert!(first.deferred);
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Edited prompt", cx));
                composer.capture_prompt_draft(cx);
                let second = composer.flush_prompt_draft(cx).unwrap();
                assert_eq!(second.id, first.id);
                assert_eq!(second.base_revision, Some(first.revision));
                assert_eq!(
                    composer.flush_prompt_draft(cx).unwrap().base_revision,
                    second.base_revision
                );
                let parked = composer.park_prompt_draft(cx).unwrap();
                assert!(!parked.deferred);
                assert_eq!(parked.revision, second.revision);
                composer.reserve_prompt_draft_attempt(&second).unwrap();
                composer.input.update(cx, |input, cx| {
                    input.set_text("Changed after interrupted send", cx)
                });
                composer.capture_prompt_draft(cx);
                let fork = composer.flush_prompt_draft(cx).unwrap();
                assert_ne!(fork.id, second.id);
                assert!(fork.deferred);
                assert!(fork.base_revision.is_none());
                composer.start_prompt_draft(cx);
                assert!(composer.input.read(cx).text().is_empty());
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Independent prompt", cx));
                composer.capture_prompt_draft(cx);
                let third = composer.flush_prompt_draft(cx).unwrap();
                assert!(third.deferred);
                assert_ne!(third.id, second.id);
                assert!(third.base_revision.is_none());
                composer.prompt_draft_loading = true;
                let generation = composer.prompt_draft_load_generation;
                composer.on_input_edited(cx);
                assert!(!composer.prompt_draft_loading);
                assert!(composer.prompt_draft_load_generation > generation);
                composer.prompt_draft_loading = true;
                composer.state.update(cx, |state, _| {
                    state.selected_chat = Some("other-chat".into());
                });
                composer.on_state_changed(cx);
                assert!(!composer.prompt_draft_loading);
                assert!(!composer.prompt_draft.as_ref().unwrap().deferred);
                assert_eq!(
                    composer.prompt_draft.as_ref().unwrap().revision.as_deref(),
                    Some(third.revision.as_str())
                );
            })
            .unwrap();
    }
}
