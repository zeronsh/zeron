//! Persist only successful live catalogs, partitioned by credential/binary context.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeron_harness::{
    CatalogFailure, CatalogFailureCode, Harness, HarnessError, ModelCatalog, ModelContext,
};
use zeron_proto::Model;

#[derive(Serialize, Deserialize)]
struct Saved {
    schema_version: u32,
    fetched_at: u64,
    binary_version: Option<String>,
    catalog: Vec<Model>,
}

fn location(root: &Path, harness: &dyn Harness, context: &ModelContext) -> PathBuf {
    let id = serde_json::to_value(harness.id()).unwrap();
    root.join("model-catalogs")
        .join(id.as_str().unwrap())
        .join(format!("{}.json", context.hash))
}
fn read(path: &Path, context: &ModelContext) -> Option<Vec<Model>> {
    let saved: Saved = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (saved.schema_version == 1
        && saved.binary_version == context.binary_version
        && !saved.catalog.is_empty())
    .then_some(saved.catalog)
}
fn save(path: &Path, context: &ModelContext, models: &[Model]) -> std::io::Result<()> {
    if models.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    let bytes = serde_json::to_vec(&Saved {
        schema_version: 1,
        fetched_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        binary_version: context.binary_version.clone(),
        catalog: models.to_vec(),
    })?;
    let staging = path.with_extension(format!("{}.partial", uuid::Uuid::new_v4()));
    let result = (|| {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&staging)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&staging, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(staging);
    }
    result
}
fn unchanged(harness: &dyn Harness, context: &ModelContext) -> bool {
    harness
        .model_context()
        .ok()
        .flatten()
        .is_some_and(|now| now.hash == context.hash)
}

// Both the request and any background refresh retain the execution lease so
// cancellation or an early disk-cache response cannot race a binary update.
pub(crate) async fn list_with_lease(
    root: &Path,
    harness: Arc<dyn Harness>,
    force: bool,
    lease: Option<Arc<tokio::sync::OwnedRwLockReadGuard<()>>>,
) -> Result<Vec<Model>, HarnessError> {
    let root = root.to_path_buf();
    tokio::spawn(async move {
        let _lease = lease.clone();
        list_inner(&root, harness, force, lease).await
    })
    .await
    .map_err(|error| HarnessError::Protocol(format!("model discovery task failed: {error}")))?
}

#[cfg(test)]
async fn list(
    root: &Path,
    harness: Arc<dyn Harness>,
    force: bool,
) -> Result<Vec<Model>, HarnessError> {
    list_with_lease(root, harness, force, None).await
}

async fn list_inner(
    root: &Path,
    harness: Arc<dyn Harness>,
    force: bool,
    lease: Option<Arc<tokio::sync::OwnedRwLockReadGuard<()>>>,
) -> Result<Vec<Model>, HarnessError> {
    let Some(context) = harness.model_context().map_err(|error| {
        let failure = CatalogFailure::from(error);
        tracing::warn!(code = %failure.code, error = %failure, "Model discovery context unavailable");
        HarnessError::from(failure)
    })? else {
        return harness
            .model_catalog(force)
            .await
            .map(|catalog| catalog.models);
    };
    let path = location(root, harness.as_ref(), &context);
    let disk = read(&path, &context);
    // The refresh owns its lifetime; returning disk early must not cancel it.
    let mut refresh = tokio::spawn({
        let harness = harness.clone();
        let context = context.clone();
        let path = path.clone();
        async move {
            let _lease = lease;
            let result = harness.model_catalog(force).await;
            if !unchanged(harness.as_ref(), &context) {
                return Err(HarnessError::Protocol(
                    "model discovery context changed; retry".into(),
                ));
            }
            if let Err(error) = &result {
                let code = CatalogFailure::classify(error);
                tracing::warn!(%code, %error, "Model discovery failed");
                if !code.allows_stale() {
                    // A revoked credential must not be resurrected from disk on
                    // the next outage, even if its file contents did not change.
                    if let Err(error) = std::fs::remove_file(&path)
                        && error.kind() != std::io::ErrorKind::NotFound
                    {
                        tracing::warn!(%error, "Could not retire model catalog");
                    }
                }
            }
            if let Ok(catalog) = &result
                && (catalog.source == "live"
                    || (catalog.source == "cache" && read(&path, &context).is_none()))
                && let Err(error) = save(&path, &context, &catalog.models)
            {
                tracing::warn!(%error, "Could not persist model catalog");
            }
            result
        }
    });
    let result = if disk.is_some() {
        tokio::time::timeout(Duration::from_millis(100), &mut refresh)
            .await
            .ok()
    } else {
        Some(refresh.await)
    };
    if !unchanged(harness.as_ref(), &context) {
        return Err(HarnessError::Protocol(
            "model discovery context changed; retry".into(),
        ));
    }
    let catalog = match result {
        Some(Ok(Ok(catalog))) if !catalog.models.is_empty() => catalog,
        Some(Ok(Err(error))) if !CatalogFailure::classify(&error).allows_stale() => {
            // Claude intentionally offers its manifest even while logged out.
            if harness.id() == zeron_proto::HarnessId::ClaudeCode
                && CatalogFailure::classify(&error) == CatalogFailureCode::AuthRequired
            {
                ModelCatalog {
                    models: harness.fallback_models(),
                    source: "static",
                }
            } else {
                return Err(CatalogFailure::from(error).into());
            }
        }
        result => {
            tracing::warn!(error = ?result, binary_path = %context.binary_path.display(), binary_version = ?context.binary_version, "Model discovery unavailable");
            match disk {
                Some(models) => ModelCatalog {
                    models,
                    source: "cache",
                },
                None => {
                    let models = harness.fallback_models();
                    if models.is_empty() {
                        return Err(HarnessError::Protocol(
                            "model catalog unavailable; retry".into(),
                        ));
                    }
                    ModelCatalog {
                        models,
                        source: "static",
                    }
                }
            }
        }
    };
    tracing::info!(harness = ?harness.id(), source = catalog.source, binary_path = %context.binary_path.display(), binary_version = ?context.binary_version, model_count = catalog.models.len(), "Model discovery");
    Ok(catalog.models)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn context(hash: &str) -> ModelContext {
        ModelContext {
            hash: hash.into(),
            binary_path: "fixture".into(),
            binary_version: Some("1.0.0".into()),
        }
    }
    fn models() -> Vec<Model> {
        vec![Model {
            id: "live".into(),
            label: "Live".into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
        }]
    }
    struct Probe {
        account: std::sync::atomic::AtomicUsize,
        fail: std::sync::atomic::AtomicBool,
        delay: std::sync::atomic::AtomicBool,
        forced: std::sync::atomic::AtomicBool,
        cached: std::sync::atomic::AtomicBool,
        failure: std::sync::Mutex<String>,
        harness: zeron_proto::HarnessId,
    }
    impl Probe {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                account: 1.into(),
                fail: false.into(),
                delay: false.into(),
                forced: false.into(),
                cached: false.into(),
                failure: std::sync::Mutex::new("offline".into()),
                harness: zeron_proto::HarnessId::Codex,
            })
        }
    }
    use std::sync::atomic::Ordering::SeqCst;
    #[async_trait::async_trait]
    impl Harness for Probe {
        fn id(&self) -> zeron_proto::HarnessId {
            self.harness
        }
        fn display_name(&self) -> &str {
            "Fixture"
        }
        fn supports_steering(&self) -> bool {
            false
        }
        fn steering_mode(&self) -> zeron_proto::SteeringMode {
            zeron_proto::SteeringMode::TurnBoundary
        }
        fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] {
            &[]
        }
        fn model_context(&self) -> Result<Option<ModelContext>, HarnessError> {
            Ok(Some(context(&self.account.load(SeqCst).to_string())))
        }
        fn fallback_models(&self) -> Vec<Model> {
            let mut rows = models();
            rows[0].id = "static".into();
            rows
        }
        async fn models(&self) -> Result<Vec<Model>, HarnessError> {
            Ok(models())
        }
        async fn model_catalog(&self, force: bool) -> Result<ModelCatalog, HarnessError> {
            self.forced.store(force, SeqCst);
            if self.delay.load(SeqCst) {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            if self.fail.load(SeqCst) {
                return Err(HarnessError::Protocol(self.failure.lock().unwrap().clone()));
            }
            Ok(ModelCatalog {
                models: models(),
                source: if self.cached.load(SeqCst) {
                    "cache"
                } else {
                    "live"
                },
            })
        }
        async fn run(
            &self,
            _: zeron_proto::RunRequest,
            _: zeron_harness::RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<zeron_proto::AgentEvent, HarnessError>>,
            HarnessError,
        > {
            unreachable!()
        }
    }
    #[tokio::test]
    async fn background_refresh_holds_lease_after_cached_reply() {
        let dir = tempfile::tempdir().unwrap();
        let probe = Probe::new();
        list(dir.path(), probe.clone(), false).await.unwrap();
        probe.delay.store(true, SeqCst);
        let gate = Arc::new(tokio::sync::RwLock::new(()));
        let lease = Arc::new(gate.clone().read_owned().await);
        list_with_lease(dir.path(), probe, true, Some(lease))
            .await
            .unwrap();
        assert!(
            gate.try_write().is_err(),
            "background probe must block updates"
        );
        let _writer = tokio::time::timeout(Duration::from_secs(2), gate.write())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn cancelled_catalog_request_holds_lease_until_probe_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let probe = Probe::new();
        probe.delay.store(true, SeqCst);
        let gate = Arc::new(tokio::sync::RwLock::new(()));
        let lease = Arc::new(gate.clone().read_owned().await);
        let task = tokio::spawn({
            let probe = probe.clone();
            async move { list_with_lease(&root, probe, true, Some(lease)).await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !probe.forced.load(SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        task.abort();
        let _ = task.await;
        assert!(
            gate.try_write().is_err(),
            "cancelled RPC must not unlock a running probe"
        );
        let _writer = tokio::time::timeout(Duration::from_secs(2), gate.write())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn auth_and_missing_binary_failures_retire_disk_instead_of_serving_it() {
        for message in ["not logged in", "spawn ENOENT"] {
            let dir = tempfile::tempdir().unwrap();
            let probe = Probe::new();
            list(dir.path(), probe.clone(), false).await.unwrap();
            probe.fail.store(true, SeqCst);
            *probe.failure.lock().unwrap() = message.into();
            let error = list(dir.path(), probe.clone(), true).await.unwrap_err();
            assert!(!CatalogFailure::classify(&error).allows_stale());
            let context = probe.model_context().unwrap().unwrap();
            assert!(!location(dir.path(), probe.as_ref(), &context).exists());
        }
    }
    #[tokio::test]
    async fn logged_out_claude_uses_curated_rows_instead_of_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut probe = Probe::new();
        Arc::get_mut(&mut probe).unwrap().harness = zeron_proto::HarnessId::ClaudeCode;
        list(dir.path(), probe.clone(), false).await.unwrap();
        probe.fail.store(true, SeqCst);
        *probe.failure.lock().unwrap() = "authentication required".into();
        assert_eq!(list(dir.path(), probe, true).await.unwrap()[0].id, "static");
    }

    #[tokio::test]
    async fn successful_memory_catalog_is_saved_if_disk_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let probe = Probe::new();
        probe.cached.store(true, SeqCst);
        assert_eq!(
            list(dir.path(), probe.clone(), false).await.unwrap(),
            models()
        );
        let context = probe.model_context().unwrap().unwrap();
        assert_eq!(
            read(&location(dir.path(), probe.as_ref(), &context), &context),
            Some(models())
        );
    }

    #[tokio::test]
    async fn live_catalog_persists_and_same_context_failure_uses_disk_before_static() {
        let dir = tempfile::tempdir().unwrap();
        let probe = Probe::new();
        assert_eq!(
            list(dir.path(), probe.clone(), true).await.unwrap(),
            models()
        );
        assert!(probe.forced.load(SeqCst));
        // A fresh harness instance represents an engine restart (no memory cache).
        let restarted = Probe::new();
        restarted.fail.store(true, SeqCst);
        assert_eq!(
            list(dir.path(), restarted.clone(), false).await.unwrap(),
            models()
        );
        restarted.account.store(2, SeqCst);
        assert_eq!(
            list(dir.path(), restarted, false).await.unwrap()[0].id,
            "static"
        );
        assert!(!dir.path().join("model-catalogs/codex/2.json").exists());
    }
    #[tokio::test]
    async fn slow_live_probe_returns_disk_and_finishes_in_background() {
        let dir = tempfile::tempdir().unwrap();
        let probe = Probe::new();
        let context = probe.model_context().unwrap().unwrap();
        let path = location(dir.path(), probe.as_ref(), &context);
        let mut old = models();
        old[0].id = "old-live".into();
        save(&path, &context, &old).unwrap();
        probe.delay.store(true, SeqCst);
        assert_eq!(list(dir.path(), probe, false).await.unwrap(), old);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(read(&path, &context), Some(models()));
    }
    #[tokio::test]
    async fn account_swap_during_probe_never_publishes_or_persists_old_account() {
        let dir = tempfile::tempdir().unwrap();
        let probe = Probe::new();
        probe.delay.store(true, SeqCst);
        let swap = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            probe.account.store(2, SeqCst);
        };
        let (result, _) = tokio::join!(list(dir.path(), probe.clone(), false), swap);
        assert!(result.is_err());
        assert!(!dir.path().join("model-catalogs").exists());
    }

    #[test]
    fn saved_catalog_roundtrip_rejects_corrupt_empty_and_future_schemas() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.json");
        let context = context("first");
        assert!(read(&path, &context).is_none());
        save(&path, &context, &models()).unwrap();
        assert_eq!(read(&path, &context), Some(models()));
        let mut wrong = context.clone();
        wrong.binary_version = Some("2.0.0".into());
        assert!(read(&path, &wrong).is_none());
        for value in ["broken".to_owned(), serde_json::json!({"schema_version":2,"fetched_at":0,"binary_version":"1.0.0","catalog":models()}).to_string(), serde_json::json!({"schema_version":1,"fetched_at":0,"binary_version":"1.0.0","catalog":[]}).to_string()] {
            std::fs::write(&path, value).unwrap(); assert!(read(&path, &context).is_none());
        }
    }
}
