//! An engine served before its profile is open. Identity (`EngineInfo`) and
//! the auth surface answer at once; every other call waits until the profile
//! assembles — immediately for local, development and already-resolved
//! synced profiles, after the viewport finishes organization onboarding for a
//! captured synced session that doesn't have one yet. Existing subscriptions
//! attach to the assembled service without reconnecting.
//!
//! Both owners of a desktop engine use it: the app's background engine
//! ([`crate::Engine::run_managed`]) and the app's in-process fallback.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, OnceCell, watch};
use tokio::task::JoinHandle;
use zeron_proto::{EngineInfo, WorkspaceScope};
use zeron_rpc::{RpcError, RpcReply, RpcService, methods};

use crate::rpc::AuthRpc;
use crate::{Auth, Engine, EngineConfig, EngineProfile, EngineRuntime, InstanceLock};

/// Where assembly stands.
#[derive(Clone, Debug)]
pub enum DeferredState {
    Waiting,
    Ready,
    Failed(String),
}

/// Serves engine identity and AuthRpc immediately, then holds data calls only
/// until the runtime assembles.
struct DeferredEngineRpc {
    auth: AuthRpc,
    engine_info: EngineInfo,
    state: watch::Receiver<DeferredState>,
    service: Arc<OnceCell<Arc<dyn RpcService>>>,
}

#[async_trait]
impl RpcService for DeferredEngineRpc {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method == methods::ENGINE_INFO {
            return RpcReply::value(&self.engine_info);
        }
        if method == methods::ENGINE_READY {
            let mut state = self.state.clone();
            return match wait(&mut state).await {
                Ok(()) => RpcReply::value(&serde_json::json!({ "ready": true })),
                Err(message) => Err(RpcError::Failed(message)),
            };
        }
        if AuthRpc::handles(method) {
            return self.auth.handle(method, params).await;
        }

        let mut state = self.state.clone();
        loop {
            let current = { state.borrow().clone() };
            match current {
                DeferredState::Waiting => {}
                DeferredState::Ready => {
                    let service = self.service.get().ok_or_else(|| {
                        RpcError::Failed("engine became ready without an RPC service".into())
                    })?;
                    return service.handle(method, params).await;
                }
                DeferredState::Failed(message) => return Err(RpcError::Failed(message)),
            }
            state.changed().await.map_err(|_| RpcError::Closed)?;
        }
    }
}

/// Wait for assembly's outcome.
pub async fn wait(state: &mut watch::Receiver<DeferredState>) -> Result<(), String> {
    loop {
        let current = { state.borrow().clone() };
        match current {
            DeferredState::Waiting => {}
            DeferredState::Ready => return Ok(()),
            DeferredState::Failed(message) => return Err(message),
        }
        state
            .changed()
            .await
            .map_err(|_| "engine assembly ended without a result".to_string())?;
    }
}

/// A desktop engine between "identity known" and "profile open".
pub struct DeferredEngine {
    /// The service to serve — over the in-memory transport, the IPC port, or
    /// both. Identity and auth now; everything else once assembled.
    pub service: Arc<dyn RpcService>,
    pub engine_info: EngineInfo,
    pub auth: Auth,
    pub workspace_scope: WorkspaceScope,
    /// The profile was known at startup: no onboarding stands between
    /// startup and assembly.
    pub profile_is_resolved: bool,
    /// The assembled runtime, once there is one.
    pub runtime: Arc<Mutex<Option<EngineRuntime>>>,
    pub refresh_task: JoinHandle<()>,
    state_tx: Option<watch::Sender<DeferredState>>,
    state: watch::Receiver<DeferredState>,
    assembled: Arc<OnceCell<Arc<dyn RpcService>>>,
    initial_profile: Option<Option<EngineProfile>>,
}

impl DeferredEngine {
    /// Resolve auth and identity (no stores opened yet).
    pub async fn prepare(config: &EngineConfig) -> anyhow::Result<Self> {
        let auth = Engine::build_auth(config).await;
        let workspace_scope = Engine::initial_workspace_scope(&auth);
        let initial_profile = Engine::resolve_profile(config, &auth, workspace_scope)?;
        let profile_is_resolved = initial_profile.is_some();
        let engine_info = Engine::engine_info(config, workspace_scope)?;
        let refresh_task = auth.spawn_refresh_loop();
        let (state_tx, state) = watch::channel(DeferredState::Waiting);
        let assembled = Arc::new(OnceCell::new());
        let service: Arc<dyn RpcService> = Arc::new(DeferredEngineRpc {
            auth: AuthRpc::new(auth.clone()),
            engine_info: engine_info.clone(),
            state: state.clone(),
            service: assembled.clone(),
        });
        Ok(Self {
            service,
            engine_info,
            auth,
            workspace_scope,
            profile_is_resolved,
            runtime: Arc::new(Mutex::new(None)),
            refresh_task,
            state_tx: Some(state_tx),
            state,
            assembled,
            initial_profile: Some(initial_profile),
        })
    }

    /// Watch assembly's outcome.
    pub fn state(&self) -> watch::Receiver<DeferredState> {
        self.state.clone()
    }

    /// Assemble in the background: first waiting for sign-in to pick an
    /// organization when the captured session has none. `served_ipc_port` is
    /// the port this process actually serves (agents' injected MCP server
    /// dials it); `on_ready` runs once the runtime is open, before data
    /// calls are released. Call once.
    pub fn spawn_boot(
        &mut self,
        config: EngineConfig,
        lock: InstanceLock,
        served_ipc_port: Option<u16>,
        on_ready: impl FnOnce(&EngineRuntime) + Send + 'static,
    ) -> JoinHandle<()> {
        let state_tx = self.state_tx.take().expect("spawn_boot called twice");
        let initial_profile = self
            .initial_profile
            .take()
            .expect("spawn_boot called twice");
        let auth = self.auth.clone();
        let workspace_scope = self.workspace_scope;
        let runtime_slot = self.runtime.clone();
        let assembled = self.assembled.clone();
        let ipc_token_file = zeron_rpc::ipc_auth::token_path(&config.data_dir);
        // The instance lock rides into the boot task and is consumed by
        // assembly — held through sign-in onboarding too, because this
        // process owns the data dir from the moment it decided to host it.
        tokio::spawn(async move {
            let profile = match initial_profile {
                Some(profile) => profile,
                None => {
                    let mut auth_state = auth.watch_state();
                    while !auth_state.borrow().is_signed_in() {
                        if auth_state.changed().await.is_err() {
                            state_tx.send_replace(DeferredState::Failed(
                                "authentication state closed before workspace onboarding".into(),
                            ));
                            return;
                        }
                    }
                    match Engine::resolve_profile(&config, &auth, workspace_scope) {
                        Ok(Some(profile)) => profile,
                        Ok(None) => {
                            state_tx.send_replace(DeferredState::Failed(
                                "workspace onboarding completed without an organization".into(),
                            ));
                            return;
                        }
                        Err(err) => {
                            state_tx.send_replace(DeferredState::Failed(err.to_string()));
                            return;
                        }
                    }
                }
            };

            match Engine::assemble_runtime_with_lock(&config, auth, profile, lock).await {
                Ok(runtime) => {
                    let service: Arc<dyn RpcService> = runtime.core().rpc_service();
                    if let Some(port) = served_ipc_port {
                        runtime.core().sessions.set_ipc_port(port);
                        runtime.core().sessions.set_ipc_token_file(ipc_token_file);
                    }
                    on_ready(&runtime);
                    *runtime_slot.lock().await = Some(runtime);
                    if assembled.set(service).is_err() {
                        state_tx.send_replace(DeferredState::Failed(
                            "engine RPC service was assembled more than once".into(),
                        ));
                        return;
                    }
                    state_tx.send_replace(DeferredState::Ready);
                }
                Err(err) => {
                    tracing::error!(error = %err, "engine assembly failed");
                    state_tx.send_replace(DeferredState::Failed(format!("{err:#}")));
                }
            }
        })
    }
}
