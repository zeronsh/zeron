//! Local administration for a private workspace. Never exposed through the relay.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{OwnedMutexGuard, watch};
use zeron_private::{Hub, NodeRole, PrivateConfig};
use zeron_rpc::{RpcError, RpcReply, TokenError, TokenSource, methods};

pub struct PrivateAccess {
    data_dir: PathBuf,
    config: Mutex<Option<PrivateConfig>>,
    hub: Option<Arc<Hub>>,
    changed: watch::Sender<u64>,
    operation: Arc<tokio::sync::Mutex<()>>,
    network: Arc<dyn PrivateNetwork>,
}

#[async_trait]
trait PrivateNetwork: Send + Sync {
    async fn discover_url(&self, port: u16) -> anyhow::Result<String>;
    async fn setup(&self, config: &PrivateConfig) -> anyhow::Result<()>;
    async fn disable(&self, config: &PrivateConfig) -> anyhow::Result<()>;
}

struct Tailscale;

#[async_trait]
impl PrivateNetwork for Tailscale {
    async fn discover_url(&self, port: u16) -> anyhow::Result<String> {
        zeron_private::tailscale::discover_url(port).await
    }
    async fn setup(&self, config: &PrivateConfig) -> anyhow::Result<()> {
        zeron_private::tailscale::setup(config).await.map(|_| ())
    }
    async fn disable(&self, config: &PrivateConfig) -> anyhow::Result<()> {
        zeron_private::tailscale::disable(config).await
    }
}

struct PendingRoute {
    config: PrivateConfig,
    staging: Option<tempfile::TempDir>,
    installed_root: Option<PathBuf>,
}

impl Drop for PendingRoute {
    fn drop(&mut self) {
        if let Some(root) = self.installed_root.take() {
            if let Err(error) = std::fs::remove_dir_all(&root) {
                tracing::warn!(%error,"Could not remove an uncommitted private workspace");
            }
        }
    }
}

struct PrivateOperation {
    lock: Option<OwnedMutexGuard<()>>,
    network: Arc<dyn PrivateNetwork>,
    pending: Option<PendingRoute>,
}

impl PrivateOperation {
    fn guard_route(&mut self, config: &PrivateConfig) {
        self.pending = Some(PendingRoute {
            config: config.clone(),
            staging: None,
            installed_root: None,
        });
    }

    fn commit(&mut self) {
        if let Some(mut pending) = self.pending.take() {
            pending.installed_root = None;
        }
    }

    async fn rollback(&mut self) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if let Err(error) = self.network.disable(&pending.config).await {
            tracing::warn!(%error, "Private setup failed; Serve route cleanup failed");
        }
        self.pending.take();
    }
}

impl Drop for PrivateOperation {
    fn drop(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let network = self.network.clone();
        let lock = self.lock.take();
        // Keep the operation lock until cleanup ends: a cancelled setup's
        // cleanup must never remove a subsequent setup's route on that port.
        tokio::spawn(async move {
            if let Err(error) = network.disable(&pending.config).await {
                tracing::warn!(%error,"Private setup was cancelled or failed; Serve route cleanup failed");
            }
            drop(pending);
            drop(lock);
        });
    }
}

impl PrivateAccess {
    pub fn open(data_dir: &Path) -> anyhow::Result<Arc<Self>> {
        let config = PrivateConfig::load(data_dir)?;
        let hub = config
            .as_ref()
            .filter(|c| c.host_hub)
            .map(|c| Hub::open(data_dir, c))
            .transpose()?;
        Ok(Arc::new(Self {
            data_dir: data_dir.into(),
            config: Mutex::new(config),
            hub,
            changed: watch::channel(0).0,
            operation: Arc::new(tokio::sync::Mutex::new(())),
            network: Arc::new(Tailscale),
        }))
    }

    pub fn config(&self) -> Option<PrivateConfig> {
        self.config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn hub(&self) -> Option<&Arc<Hub>> {
        self.hub.as_ref()
    }

    pub fn handles(method: &str) -> bool {
        matches!(
            method,
            methods::PRIVATE_STATUS
                | methods::CREATE_PRIVATE_WORKSPACE
                | methods::JOIN_PRIVATE_WORKSPACE
                | methods::CREATE_PRIVATE_INVITATION
                | methods::REVOKE_PRIVATE_NODE
                | methods::SET_PRIVATE_ACCESS_ENABLED
                | methods::LEAVE_PRIVATE_WORKSPACE
        )
    }

    pub fn changes_workspace(method: &str) -> bool {
        matches!(
            method,
            methods::CREATE_PRIVATE_WORKSPACE
                | methods::JOIN_PRIVATE_WORKSPACE
                | methods::LEAVE_PRIVATE_WORKSPACE
        )
    }

    pub async fn handle(
        &self,
        method: &str,
        params: Value,
        device_id: &str,
    ) -> Result<RpcReply, RpcError> {
        let mut operation = PrivateOperation {
            lock: Some(self.operation.clone().lock_owned().await),
            network: self.network.clone(),
            pending: None,
        };
        let result = self
            .handle_inner(method, params, device_id, &mut operation)
            .await;
        if result.is_err() {
            operation.rollback().await;
        }
        result.map_err(|e| RpcError::Failed(e.to_string()))
    }

    async fn handle_inner(
        &self,
        method: &str,
        params: Value,
        device_id: &str,
        operation: &mut PrivateOperation,
    ) -> anyhow::Result<RpcReply> {
        match method {
            methods::PRIVATE_STATUS => {
                let Some(c) = self.config() else {
                    return Ok(RpcReply::Value(json!({"state":"unconfigured"})));
                };
                let nodes = self
                    .hub
                    .as_ref()
                    .map(|hub| hub.nodes())
                    .transpose()?
                    .unwrap_or_default();
                Ok(RpcReply::Value(
                    json!({"state":"configured", "workspaceId":c.workspace_id,
                    "hubUrl":c.hub_url,"name":c.name,"role":c.role,"hostHub":c.host_hub,
                    "enabled":c.enabled,"nodes":nodes}),
                ))
            }
            methods::CREATE_PRIVATE_WORKSPACE => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    name: String,
                    role: NodeRole,
                    #[serde(default)]
                    listen_port: Option<u16>,
                    #[serde(default)]
                    serve_port: Option<u16>,
                }
                let p: Params = serde_json::from_value(params)?;
                anyhow::ensure!(
                    self.config().is_none(),
                    "Leave the current private workspace first"
                );
                let serve_port = p.serve_port.unwrap_or(8443);
                let listen_port = p.listen_port.unwrap_or(27655);
                anyhow::ensure!(
                    serve_port != 0 && listen_port != 0 && listen_port != 27654,
                    "Choose nonzero ports; local IPC uses 27654"
                );
                let url = self.network.discover_url(serve_port).await?;
                let staging = tempfile::Builder::new()
                    .prefix(".private-setup-")
                    .tempdir_in(&self.data_dir)?;
                let mut config = PrivateConfig::create(
                    staging.path(),
                    &p.name,
                    device_id,
                    &crate::local_device_name(device_id),
                    p.role,
                    &url,
                )?;
                config.listen_port = listen_port;
                config.serve_port = serve_port;
                operation.pending = Some(PendingRoute {
                    config: config.clone(),
                    staging: Some(staging),
                    installed_root: None,
                });
                self.network.setup(&config).await?;
                let parent = self.data_dir.join("profiles/private");
                std::fs::create_dir_all(&parent)?;
                let destination = parent.join(&config.workspace_id);
                let pending = operation
                    .pending
                    .as_mut()
                    .expect("setup retains its staging directory");
                let source = pending
                    .staging
                    .as_ref()
                    .expect("create has a staged workspace")
                    .path()
                    .join("profiles/private")
                    .join(&config.workspace_id);
                std::fs::rename(source, &destination)?;
                pending.installed_root = Some(destination);
                config.save(&self.data_dir)?;
                self.replace(Some(config));
                operation.commit();
                Ok(RpcReply::Value(json!({"restartRequired":true})))
            }
            methods::JOIN_PRIVATE_WORKSPACE => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    hub_url: String,
                    code: String,
                    name: String,
                    role: NodeRole,
                }
                let p: Params = serde_json::from_value(params)?;
                anyhow::ensure!(
                    self.config().is_none(),
                    "Leave the current private workspace first"
                );
                let config = PrivateConfig::join(
                    &self.data_dir,
                    &p.hub_url,
                    &p.code,
                    &p.name,
                    device_id,
                    p.role,
                )
                .await?;
                self.replace(Some(config));
                Ok(RpcReply::Value(json!({"restartRequired":true})))
            }
            methods::CREATE_PRIVATE_INVITATION => {
                #[derive(Deserialize)]
                struct Params {
                    role: NodeRole,
                }
                let p: Params = serde_json::from_value(params)?;
                Ok(RpcReply::Value(serde_json::to_value(
                    self.admin()?.create_invitation(p.role)?,
                )?))
            }
            methods::REVOKE_PRIVATE_NODE => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Params {
                    device_id: String,
                }
                let p: Params = serde_json::from_value(params)?;
                anyhow::ensure!(
                    p.device_id != device_id,
                    "The hub cannot revoke itself; disable private access instead"
                );
                self.admin()?.revoke(&p.device_id)?;
                Ok(RpcReply::Value(json!({"ok":true})))
            }
            methods::SET_PRIVATE_ACCESS_ENABLED => {
                #[derive(Deserialize)]
                struct Params {
                    enabled: bool,
                }
                let p: Params = serde_json::from_value(params)?;
                let mut config = self
                    .config()
                    .ok_or_else(|| anyhow::anyhow!("No private workspace configured"))?;
                if config.host_hub {
                    if p.enabled {
                        if !config.enabled {
                            operation.guard_route(&config);
                        }
                        self.network.setup(&config).await?;
                    }
                    self.admin()?.set_enabled(p.enabled)?;
                    config.enabled = p.enabled;
                    self.replace(Some(config.clone()));
                    // Admission closes first even if the system command fails.
                    if !p.enabled {
                        operation.guard_route(&config);
                        if let Err(error) = self.network.disable(&config).await {
                            tracing::warn!(%error, "private hub disabled; Tailscale route cleanup failed");
                        }
                    }
                    operation.commit();
                } else {
                    config.enabled = p.enabled;
                    config.save(&self.data_dir)?;
                    self.replace(Some(config));
                }
                Ok(RpcReply::Value(json!({"ok":true})))
            }
            methods::LEAVE_PRIVATE_WORKSPACE => {
                if let Some(mut config) = self.config() {
                    if config.host_hub {
                        self.admin()?.set_enabled(false)?;
                        config.enabled = false;
                        self.replace(Some(config.clone()));
                        operation.guard_route(&config);
                        self.network.disable(&config).await?;
                        operation.commit();
                    }
                }
                PrivateConfig::remove(&self.data_dir)?;
                self.replace(None);
                Ok(RpcReply::Value(json!({"restartRequired":true})))
            }
            _ => anyhow::bail!("Unknown private workspace method"),
        }
    }

    fn admin(&self) -> anyhow::Result<&Arc<Hub>> {
        self.hub
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Manage pairing on the sync hub"))
    }

    fn replace(&self, config: Option<PrivateConfig>) {
        *self.config.lock().unwrap_or_else(|e| e.into_inner()) = config;
        self.changed.send_modify(|revision| *revision += 1);
    }
}

#[async_trait]
impl TokenSource for PrivateAccess {
    async fn token(&self) -> Result<String, TokenError> {
        self.config()
            .filter(|c| c.enabled)
            .map(|c| c.token)
            .ok_or(TokenError::SignedOut)
    }
    fn header_auth(&self) -> bool {
        true
    }
    fn subscribe(&self) -> Option<watch::Receiver<u64>> {
        Some(self.changed.subscribe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::{Notify, Semaphore};

    #[derive(Clone, Copy)]
    enum Behavior {
        Ready,
        Fail,
        Blocked,
    }

    struct NetworkFixture {
        setup: Behavior,
        disable: Behavior,
        setup_started: Notify,
        disable_started: Notify,
        setup_gate: Semaphore,
        disable_gate: Semaphore,
        configured: AtomicBool,
    }

    impl NetworkFixture {
        fn new(setup: Behavior, disable: Behavior) -> Arc<Self> {
            Arc::new(Self {
                setup,
                disable,
                setup_started: Notify::new(),
                disable_started: Notify::new(),
                setup_gate: Semaphore::new(0),
                disable_gate: Semaphore::new(0),
                configured: AtomicBool::new(false),
            })
        }
    }

    #[async_trait]
    impl PrivateNetwork for NetworkFixture {
        async fn discover_url(&self, port: u16) -> anyhow::Result<String> {
            Ok(format!("https://private.example.ts.net:{port}"))
        }
        async fn setup(&self, _: &PrivateConfig) -> anyhow::Result<()> {
            self.configured.store(true, Ordering::SeqCst);
            self.setup_started.notify_one();
            match self.setup {
                Behavior::Ready => {}
                Behavior::Fail => anyhow::bail!("Serve command failed after installing the route"),
                Behavior::Blocked => self.setup_gate.acquire().await?.forget(),
            }
            Ok(())
        }
        async fn disable(&self, _: &PrivateConfig) -> anyhow::Result<()> {
            self.disable_started.notify_one();
            match self.disable {
                Behavior::Ready => {}
                Behavior::Fail => anyhow::bail!("Serve cleanup unavailable"),
                Behavior::Blocked => self.disable_gate.acquire().await?.forget(),
            }
            self.configured.store(false, Ordering::SeqCst);
            Ok(())
        }
    }

    fn access(
        path: &Path,
        network: Arc<NetworkFixture>,
        enabled: Option<bool>,
    ) -> Arc<PrivateAccess> {
        if let Some(enabled) = enabled {
            let mut config = PrivateConfig::create(
                path,
                "Existing workspace",
                "server-a",
                "Server A",
                NodeRole::Server,
                "https://private.example.ts.net:8443",
            )
            .unwrap();
            config.enabled = enabled;
            config.save(path).unwrap();
            network.configured.store(enabled, Ordering::SeqCst);
        }
        let mut access = PrivateAccess::open(path).unwrap();
        Arc::get_mut(&mut access).unwrap().network = network;
        access
    }

    async fn completed_cleanup(access: &PrivateAccess) {
        let lock = tokio::time::timeout(std::time::Duration::from_secs(2), access.operation.lock())
            .await
            .expect("cancelled operation did not complete cleanup");
        drop(lock);
    }

    fn create(access: Arc<PrivateAccess>) -> tokio::task::JoinHandle<Result<RpcReply, RpcError>> {
        tokio::spawn(async move {
            access
                .handle(
                    methods::CREATE_PRIVATE_WORKSPACE,
                    json!({"name":"New workspace","role":"server"}),
                    "server-a",
                )
                .await
        })
    }

    #[tokio::test]
    async fn cancelled_create_keeps_the_old_workspace_and_cleans_its_route_before_retry() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Blocked, Behavior::Blocked);
        let access = access(directory.path(), network.clone(), None);
        let first = create(access.clone());
        network.setup_started.notified().await;
        assert!(!PrivateConfig::path(directory.path()).exists());
        assert!(access.config().is_none());
        first.abort();
        assert!(matches!(first.await, Err(error) if error.is_cancelled()));
        network.disable_started.notified().await;
        let second = create(access.clone());
        network.setup_gate.add_permits(1);
        assert!(!second.is_finished());
        network.disable_gate.add_permits(1);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), second)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
        assert!(network.configured.load(Ordering::SeqCst));
        let saved = PrivateConfig::load(directory.path()).unwrap().unwrap();
        assert_eq!(access.config().unwrap().workspace_id, saved.workspace_id);
        let nodes = Hub::open(directory.path(), &saved)
            .unwrap()
            .nodes()
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].device_id, "server-a");
        assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".private-setup-")
        }));
    }

    #[tokio::test]
    async fn failed_create_does_not_publish_configuration_or_leave_a_route() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Fail, Behavior::Ready);
        let access = access(directory.path(), network.clone(), None);
        assert!(create(access.clone()).await.unwrap().is_err());
        completed_cleanup(&access).await;
        assert!(access.config().is_none());
        assert!(!PrivateConfig::path(directory.path()).exists());
        assert!(!network.configured.load(Ordering::SeqCst));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn failed_create_waits_for_route_cleanup_before_returning() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Fail, Behavior::Blocked);
        let access = access(directory.path(), network.clone(), None);
        let operation = create(access.clone());
        network.disable_started.notified().await;
        assert!(!operation.is_finished());
        assert!(network.configured.load(Ordering::SeqCst));
        assert!(access.config().is_none());
        network.disable_gate.add_permits(1);
        assert!(operation.await.unwrap().is_err());
        assert!(!network.configured.load(Ordering::SeqCst));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancelled_error_rollback_keeps_cleanup_armed() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Fail, Behavior::Blocked);
        let access = access(directory.path(), network.clone(), None);
        let operation = create(access.clone());
        network.disable_started.notified().await;
        operation.abort();
        assert!(matches!(operation.await, Err(error) if error.is_cancelled()));
        network.disable_started.notified().await;
        assert!(access.operation.try_lock().is_err());
        network.disable_gate.add_permits(1);
        completed_cleanup(&access).await;
        assert!(access.config().is_none());
        assert!(!network.configured.load(Ordering::SeqCst));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn failed_configuration_publish_removes_the_uncommitted_database_and_route() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Ready, Behavior::Ready);
        let access = access(directory.path(), network.clone(), None);
        std::fs::create_dir(PrivateConfig::path(directory.path())).unwrap();
        assert!(create(access.clone()).await.unwrap().is_err());
        completed_cleanup(&access).await;
        assert!(access.config().is_none());
        assert!(!network.configured.load(Ordering::SeqCst));
        assert_eq!(
            std::fs::read_dir(directory.path().join("profiles/private"))
                .unwrap()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn cancelled_disable_publishes_disabled_tokens_before_serve_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Ready, Behavior::Blocked);
        let access = access(directory.path(), network.clone(), Some(true));
        let operation = tokio::spawn({
            let access = access.clone();
            async move {
                access
                    .handle(
                        methods::SET_PRIVATE_ACCESS_ENABLED,
                        json!({"enabled":false}),
                        "server-a",
                    )
                    .await
            }
        });
        network.disable_started.notified().await;
        assert!(!access.config().unwrap().enabled);
        assert!(
            !PrivateConfig::load(directory.path())
                .unwrap()
                .unwrap()
                .enabled
        );
        assert_eq!(access.token().await, Err(TokenError::SignedOut));
        operation.abort();
        assert!(matches!(operation.await, Err(error) if error.is_cancelled()));
        network.disable_gate.add_permits(1);
        completed_cleanup(&access).await;
        assert!(!network.configured.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancelled_enable_keeps_the_hub_disabled_and_rolls_back_serve() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Blocked, Behavior::Ready);
        let access = access(directory.path(), network.clone(), Some(false));
        let operation = tokio::spawn({
            let access = access.clone();
            async move {
                access
                    .handle(
                        methods::SET_PRIVATE_ACCESS_ENABLED,
                        json!({"enabled":true}),
                        "server-a",
                    )
                    .await
            }
        });
        network.setup_started.notified().await;
        operation.abort();
        assert!(matches!(operation.await, Err(error) if error.is_cancelled()));
        completed_cleanup(&access).await;
        assert!(!access.config().unwrap().enabled);
        assert!(
            !PrivateConfig::load(directory.path())
                .unwrap()
                .unwrap()
                .enabled
        );
        assert_eq!(access.token().await, Err(TokenError::SignedOut));
        assert!(!network.configured.load(Ordering::SeqCst));
        assert!(
            access
                .admin()
                .unwrap()
                .create_invitation(NodeRole::Client)
                .is_err()
        );
    }

    #[tokio::test]
    async fn serve_cleanup_failure_does_not_restore_disabled_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let network = NetworkFixture::new(Behavior::Ready, Behavior::Fail);
        let access = access(directory.path(), network, Some(true));
        assert!(
            access
                .handle(
                    methods::SET_PRIVATE_ACCESS_ENABLED,
                    json!({"enabled":false}),
                    "server-a"
                )
                .await
                .is_ok()
        );
        assert!(!access.config().unwrap().enabled);
        assert!(
            !PrivateConfig::load(directory.path())
                .unwrap()
                .unwrap()
                .enabled
        );
        assert_eq!(access.token().await, Err(TokenError::SignedOut));
        assert!(
            access
                .admin()
                .unwrap()
                .create_invitation(NodeRole::Client)
                .is_err()
        );
    }
}
