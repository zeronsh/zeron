//! Durable content cache + publication outbox. A revision is published only
//! after its immutable content and every referenced asset have reached edge.
use crate::{EdgeConfig, EngineError};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use zeron_proto::*;
use zeron_sync::DocsStore;
const OUTBOX: &str = "prompt-drafts-outbox-v1";

#[derive(Clone)]
pub struct DraftStore {
    store: Arc<DocsStore>,
    edge: Option<EdgeConfig>,
    org: String,
    http: reqwest::Client,
    claims: Arc<std::sync::Mutex<()>>,
}
fn error(s: impl ToString) -> EngineError {
    EngineError::Other(s.to_string())
}
impl DraftStore {
    pub fn new(store: Arc<DocsStore>, edge: Option<EdgeConfig>, org: String) -> Self {
        Self {
            store,
            edge,
            org,
            claims: Arc::new(std::sync::Mutex::new(())),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("HTTP client"),
        }
    }
    fn key(id: &str) -> String {
        format!("draft-blob:{id}")
    }
    pub fn save_asset_chunk(&self, chunk: &DraftAssetChunk) -> Result<(), EngineError> {
        if !valid_blob(&chunk.blob) || chunk.total_bytes > MAX_DRAFT_ASSET_BYTES {
            return Err(error("Invalid draft asset size or hash"));
        }
        let offset = chunk
            .index
            .checked_mul(DRAFT_CHUNK_BYTES)
            .ok_or_else(|| error("Invalid chunk index"))?;
        if offset > chunk.total_bytes || (offset == chunk.total_bytes && offset != 0) {
            return Err(error("Invalid chunk index"));
        }
        let bytes = STANDARD.decode(&chunk.data).map_err(error)?;
        if bytes.len() != (chunk.total_bytes - offset).min(DRAFT_CHUNK_BYTES) {
            return Err(error("Invalid draft chunk length"));
        }
        if self.store.has_snapshot(&Self::key(&chunk.blob))? {
            return Ok(());
        }
        let key = |index| format!("draft-part:{}:{index}", chunk.blob);
        self.store.save_snapshot(&key(chunk.index), &bytes)?;
        if offset + bytes.len() == chunk.total_bytes {
            let mut asset = Vec::with_capacity(chunk.total_bytes);
            for index in 0..=chunk.index {
                asset.extend(
                    self.store
                        .load_snapshot(&key(index))?
                        .ok_or_else(|| error("Missing draft chunk"))?,
                );
            }
            self.save_asset(&DraftAsset {
                blob: chunk.blob.clone(),
                data: STANDARD.encode(&asset),
            })?;
            for index in 0..=chunk.index {
                self.store.delete_snapshot(&key(index))?;
            }
        }
        Ok(())
    }
    pub async fn load_asset_chunk(
        &self,
        blob: &str,
        index: usize,
    ) -> Result<DraftAssetChunk, EngineError> {
        if !valid_blob(blob) {
            return Err(error("Invalid draft asset hash"));
        }
        let bytes = self.read(blob).await?;
        let offset = index
            .checked_mul(DRAFT_CHUNK_BYTES)
            .ok_or_else(|| error("Invalid chunk index"))?;
        if offset > bytes.len() || (offset == bytes.len() && offset != 0) {
            return Err(error("Invalid chunk index"));
        }
        Ok(DraftAssetChunk {
            blob: blob.into(),
            data: STANDARD.encode(&bytes[offset..(offset + DRAFT_CHUNK_BYTES).min(bytes.len())]),
            index,
            total_bytes: bytes.len(),
        })
    }
    pub async fn load_content(&self, revision: &str) -> Result<DraftContent, EngineError> {
        serde_json::from_slice(&self.read(revision).await?).map_err(error)
    }

    pub fn save_asset(&self, asset: &DraftAsset) -> Result<(), EngineError> {
        let bytes = STANDARD.decode(&asset.data).map_err(error)?;
        if bytes.len() > MAX_DRAFT_ASSET_BYTES
            || format!("{:x}", Sha256::digest(&bytes)) != asset.blob
        {
            return Err(error("Invalid draft attachment"));
        }
        self.store.save_snapshot(&Self::key(&asset.blob), &bytes)?;
        Ok(())
    }
    pub fn stage(&self, mut draft: SaveDraft) -> Result<SaveDraft, EngineError> {
        if !valid_draft_id(&draft.id)
            || !valid_draft_id(&draft.revision)
            || draft
                .base_revision
                .as_ref()
                .is_some_and(|id| !valid_draft_id(id))
        {
            return Err(error("Invalid draft ID"));
        }
        if draft.content.attachments.len() > 32 {
            return Err(error("Too many draft attachments"));
        }
        let mut bytes = serde_json::to_vec(&draft.content).map_err(error)?;
        if bytes.len() > MAX_DRAFT_CONTENT_BYTES {
            return Err(error("Draft content is too large"));
        }
        for asset in &draft.assets {
            self.save_asset(asset)?;
        }
        for asset in &draft.content.attachments {
            if !valid_blob(&asset.blob) || !self.store.has_snapshot(&Self::key(&asset.blob))? {
                return Err(error("Draft attachment is unavailable"));
            }
        }
        // Never silently overwrite an immutable revision on a retried request.
        if let Some(old) = self.store.load_snapshot(&Self::key(&draft.revision))? {
            let previous: DraftContent = serde_json::from_slice(&old).map_err(error)?;
            if previous != draft.content {
                return Err(error("Draft revision already contains different content"));
            }
            bytes = old;
        }
        self.store
            .save_snapshot(&Self::key(&draft.revision), &bytes)?;
        draft.assets.clear();
        self.store.enqueue_chat_update(
            OUTBOX,
            &draft.publication_key(),
            &serde_json::to_vec(&draft).map_err(error)?,
        )?;
        Ok(draft)
    }
    pub async fn claim(&self, id: &str, revision: &str) -> Result<(), EngineError> {
        if !valid_draft_id(id) || !valid_draft_id(revision) {
            return Err(error("Invalid draft claim"));
        }
        if let Some(edge) = &self.edge {
            let response = self
                .http
                .post(format!(
                    "{}/registry/{}/draft-claim",
                    edge.url.trim_end_matches('/'),
                    self.org
                ))
                .bearer_auth(edge.token.token().await?)
                .json(&serde_json::json!({ "id": id, "revision": revision }))
                .send()
                .await
                .map_err(error)?;
            if !response.status().is_success() {
                return Err(error(
                    "Draft could not be reserved for sending. It may already have been sent from another device.",
                ));
            }
        } else {
            let _guard = self
                .claims
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let key = format!("draft-claim:{id}");
            if let Some(old) = self.store.load_snapshot(&key)? {
                if old != revision.as_bytes() {
                    return Err(error("Draft already reserved with a different revision"));
                }
            } else {
                self.store.save_snapshot(&key, revision.as_bytes())?;
            }
        }
        Ok(())
    }
    pub fn pending(&self) -> Result<Vec<SaveDraft>, EngineError> {
        self.store
            .pending_chat_updates(OUTBOX)?
            .into_iter()
            .map(|(_, b)| serde_json::from_slice(&b).map_err(error))
            .collect()
    }
    pub fn acknowledge(&self, revision: &str) -> Result<(), EngineError> {
        self.store.acknowledge_chat_update(OUTBOX, revision)?;
        Ok(())
    }
    async fn object(&self, id: &str, put: Option<Vec<u8>>) -> Result<Vec<u8>, EngineError> {
        if !valid_draft_id(id) {
            return Err(error("Invalid draft object"));
        }
        let edge = self
            .edge
            .as_ref()
            .ok_or_else(|| error("Draft content is not cached on this device"))?;
        let url = format!(
            "{}/draft-content/{}/{}",
            edge.url.trim_end_matches('/'),
            self.org,
            id
        );
        let request = if let Some(bytes) = put {
            self.http.put(url).body(bytes)
        } else {
            self.http.get(url)
        };
        let response = request
            .bearer_auth(edge.token.token().await?)
            .send()
            .await
            .map_err(error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(error(format!("Draft content: HTTP {status}")));
        }
        let bytes = response.bytes().await.map_err(error)?;
        if bytes.len() > MAX_DRAFT_ASSET_BYTES {
            return Err(error("Draft object is too large"));
        }
        Ok(bytes.to_vec())
    }
    async fn read(&self, id: &str) -> Result<Vec<u8>, EngineError> {
        if !valid_draft_id(id) {
            return Err(error("Invalid draft object"));
        }
        if let Some(bytes) = self.store.load_snapshot(&Self::key(id))? {
            return Ok(bytes);
        }
        let bytes = self.object(id, None).await?;
        if valid_blob(id) && format!("{:x}", Sha256::digest(&bytes)) != id {
            return Err(error("Draft attachment checksum mismatch"));
        }
        self.store.save_snapshot(&Self::key(id), &bytes)?;
        Ok(bytes)
    }
    pub async fn load(&self, revision: &str) -> Result<DraftBundle, EngineError> {
        let content: DraftContent =
            serde_json::from_slice(&self.read(revision).await?).map_err(error)?;
        let mut assets = Vec::new();
        for a in &content.attachments {
            assets.push(DraftAsset {
                blob: a.blob.clone(),
                data: STANDARD.encode(self.read(&a.blob).await?),
            });
        }
        Ok(DraftBundle { content, assets })
    }
    pub async fn upload(&self, draft: &SaveDraft) -> Result<(), EngineError> {
        if self.edge.is_none() {
            return Ok(());
        }
        for id in draft
            .content
            .attachments
            .iter()
            .map(|a| a.blob.as_str())
            .chain(std::iter::once(draft.revision.as_str()))
        {
            // Asset ACKs are durable too: keystrokes never upload the same image again.
            let marker = format!("draft-uploaded:{id}");
            if self.store.has_snapshot(&marker)? {
                continue;
            }
            self.object(id, Some(self.read(id).await?)).await?;
            self.store.save_snapshot(&marker, b"1")?;
        }
        Ok(())
    }
}
fn valid_blob(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn draft_outbox_and_large_content_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let drafts = DraftStore::new(store, None, "local".into());
        let draft = SaveDraft {
            deferred: false,
            id: "draft".into(),
            revision: "revision".into(),
            base_revision: None,
            created_at: 1,
            content: DraftContent {
                prompt: "large prompt ".repeat(4096),
                ..Default::default()
            },
            assets: vec![],
        };
        drafts.stage(draft.clone()).unwrap();
        drop(drafts);
        let drafts = DraftStore::new(
            Arc::new(DocsStore::open(dir.path()).unwrap()),
            None,
            "local".into(),
        );
        assert_eq!(drafts.pending().unwrap(), vec![draft.clone()]);
        assert_eq!(
            drafts.load("revision").await.unwrap().content,
            draft.content
        );
        let mut changed = draft;
        changed.content.prompt = "different".into();
        assert!(drafts.stage(changed).is_err());
        drafts.acknowledge("revision").unwrap();
        assert!(drafts.pending().unwrap().is_empty());
    }
    #[tokio::test]
    async fn large_draft_assets_round_trip_in_bounded_rpc_frames() {
        use zeron_rpc::{RpcError, RpcReply, RpcService, methods};
        struct Service(DraftStore);
        #[async_trait::async_trait]
        impl RpcService for Service {
            async fn handle(
                &self,
                method: &str,
                params: serde_json::Value,
            ) -> Result<RpcReply, RpcError> {
                if method == methods::SAVE_DRAFT_ASSET {
                    let chunk: DraftAssetChunk = zeron_rpc::parse_params(params)?;
                    self.0
                        .save_asset_chunk(&chunk)
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    RpcReply::value(&true)
                } else {
                    let blob = params["blob"].as_str().unwrap();
                    let index = params["index"].as_u64().unwrap() as usize;
                    RpcReply::value(
                        &self
                            .0
                            .load_asset_chunk(blob, index)
                            .await
                            .map_err(|e| RpcError::Failed(e.to_string()))?,
                    )
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let drafts = DraftStore::new(
            Arc::new(DocsStore::open(directory.path()).unwrap()),
            None,
            "local".into(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(zeron_rpc::serve_ws_listener(
            listener,
            Arc::new(Service(drafts)),
        ));
        let client = zeron_rpc::connect_ws(&format!("ws://127.0.0.1:{port}"))
            .await
            .unwrap();
        // The previous single-frame base64 upload exceeded Tungstenite's 16 MiB limit.
        let bytes = vec![73u8; 13 * 1024 * 1024];
        let blob = format!("{:x}", Sha256::digest(&bytes));
        for (index, part) in bytes.chunks(DRAFT_CHUNK_BYTES).enumerate() {
            let chunk = DraftAssetChunk {
                blob: blob.clone(),
                data: STANDARD.encode(part),
                index,
                total_bytes: bytes.len(),
            };
            let value = serde_json::to_value(&chunk).unwrap();
            assert!(serde_json::to_vec(&value).unwrap().len() < 2 * 1024 * 1024);
            client.call(methods::SAVE_DRAFT_ASSET, value).await.unwrap();
        }
        let mut restored = Vec::new();
        for index in 0..bytes.len().div_ceil(DRAFT_CHUNK_BYTES) {
            let value = client
                .call(
                    methods::LOAD_DRAFT_ASSET,
                    serde_json::json!({"blob": blob, "index": index}),
                )
                .await
                .unwrap();
            let chunk: DraftAssetChunk = serde_json::from_value(value).unwrap();
            restored.extend(STANDARD.decode(chunk.data).unwrap());
        }
        assert_eq!(restored, bytes);
        server.abort();
    }

    #[test]
    fn deferred_save_ack_cannot_erase_pending_visibility_promotion() {
        let directory = tempfile::tempdir().unwrap();
        let store = DraftStore::new(
            Arc::new(DocsStore::open(directory.path()).unwrap()),
            None,
            "local".into(),
        );
        let draft = SaveDraft {
            deferred: true,
            id: "a".into(),
            revision: "v1".into(),
            base_revision: None,
            created_at: 1,
            content: DraftContent {
                prompt: "Still typing".into(),
                ..Default::default()
            },
            assets: vec![],
        };
        store.stage(draft.clone()).unwrap();
        let mut listed = draft.clone();
        listed.deferred = false;
        store.stage(listed).unwrap();
        assert_eq!(store.pending().unwrap().len(), 2);
        store.acknowledge(&draft.publication_key()).unwrap();
        let pending = store.pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(!pending[0].deferred);
    }

    #[test]
    fn retrying_swift_content_preserves_omitted_optionals_and_original_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let drafts = DraftStore::new(store.clone(), None, "local".into());
        // Synthesized Swift Codable omits nil properties; Rust emits null.
        let raw = br#"{"prompt":"From iOS","target":{"deviceId":"host","newWorktree":false},"attachments":[]}"#;
        store
            .save_snapshot(&DraftStore::key("swift-revision"), raw)
            .unwrap();
        let draft = SaveDraft {
            deferred: false,
            id: "swift-draft".into(),
            revision: "swift-revision".into(),
            base_revision: None,
            created_at: 1,
            content: serde_json::from_slice(raw).unwrap(),
            assets: vec![],
        };
        drafts.stage(draft).unwrap();
        assert_eq!(
            store
                .load_snapshot(&DraftStore::key("swift-revision"))
                .unwrap()
                .unwrap(),
            raw
        );
    }

    #[tokio::test]
    async fn attachments_and_revision_claims_are_durable_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let drafts = DraftStore::new(store.clone(), None, "local".into());
        let bytes = b"image bytes";
        let blob = format!("{:x}", Sha256::digest(bytes));
        let draft = SaveDraft {
            deferred: false,
            id: "draft".into(),
            revision: "revision".into(),
            base_revision: None,
            created_at: 1,
            content: DraftContent {
                prompt: "".into(),
                attachments: vec![DraftAttachment {
                    id: "img".into(),
                    name: "img.png".into(),
                    blob: blob.clone(),
                    appshot: None,
                }],
                ..Default::default()
            },
            assets: vec![DraftAsset {
                blob,
                data: STANDARD.encode(bytes),
            }],
        };
        drafts.stage(draft.clone()).unwrap();
        drafts.claim("draft", "revision").await.unwrap();
        drafts.claim("draft", "revision").await.unwrap();
        assert!(drafts.claim("draft", "different").await.is_err());
        drop(drafts);
        let drafts = DraftStore::new(store, None, "local".into());
        assert_eq!(
            STANDARD
                .decode(&drafts.load("revision").await.unwrap().assets[0].data)
                .unwrap(),
            bytes
        );
        // Swift JSON has a different property order; semantic equality must
        // preserve the original immutable object bytes when it is retried.
        let raw = serde_json::to_vec_pretty(&draft.content).unwrap();
        drafts
            .store
            .save_snapshot(&DraftStore::key("revision"), &raw)
            .unwrap();
        drafts.stage(draft).unwrap();
        assert_eq!(
            drafts
                .store
                .load_snapshot(&DraftStore::key("revision"))
                .unwrap()
                .unwrap(),
            raw
        );
    }
}
