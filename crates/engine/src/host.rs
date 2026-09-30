//! The engine as a long-lived host process the headed app attaches to.
//!
//! An engine that can hand itself over across a live update (see
//! [`crate::handoff`]) cannot live inside the window's process, so the headed
//! app runs no engine of its own on Unix: `zeron headless --host` is the
//! engine, and the window is a viewport like any other. This module holds the
//! part of that both sides share: an RPC front that answers identity and
//! sign-in at once and holds data calls only while a captured cloud profile
//! still needs its organization chosen.

use std::sync::Arc;

use async_trait::async_trait;
use zeron_proto::EngineInfo;
use zeron_rpc::{RpcError, RpcReply, RpcService, methods};

use crate::rpc::AuthRpc;

/// Mark `info` as the app's own engine host when this process was started as
/// one (`ZERON_ENGINE_HOST=app`), so a window that attaches later — after a
/// crash, or one started by an update swap — knows quitting should stop it.
pub fn advertise_app_host(info: &mut EngineInfo) {
    let app_hosted = zeron_update::engine_host().is_some_and(|host| host.trim() == "app");
    if app_hosted && !info.supports(zeron_proto::capabilities::APP_HOSTED) {
        info.capabilities
            .push(zeron_proto::capabilities::APP_HOSTED.to_string());
    }
}

/// Where an engine that answers RPC before it is fully assembled is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeferredEngineState {
    Waiting,
    Ready,
    Failed(String),
}

/// Serves engine identity and AuthRpc immediately, then holds data calls only
/// while a captured synced profile still needs organization onboarding.
/// Existing subscriptions attach to the assembled service without reconnecting.
pub struct DeferredEngineRpc {
    auth: AuthRpc,
    engine_info: EngineInfo,
    state: tokio::sync::watch::Receiver<DeferredEngineState>,
    service: Arc<tokio::sync::OnceCell<Arc<dyn RpcService>>>,
    /// Where `StopEngine` goes while the engine is still assembling (once it
    /// is ready the assembled service owns that method). `None` for a host
    /// that cannot be stopped this way (the in-process engine).
    stop: Option<tokio::sync::mpsc::UnboundedSender<()>>,
}

impl DeferredEngineRpc {
    pub fn new(
        auth: AuthRpc,
        engine_info: EngineInfo,
        state: tokio::sync::watch::Receiver<DeferredEngineState>,
        service: Arc<tokio::sync::OnceCell<Arc<dyn RpcService>>>,
    ) -> Self {
        let mut engine_info = engine_info;
        advertise_app_host(&mut engine_info);
        Self {
            auth,
            engine_info,
            state,
            service,
            stop: None,
        }
    }

    /// Let a client stop an engine that is still waiting for sign-in.
    pub fn with_stop(mut self, stop: tokio::sync::mpsc::UnboundedSender<()>) -> Self {
        self.stop = Some(stop);
        self
    }
}

#[async_trait]
impl RpcService for DeferredEngineRpc {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method == methods::ENGINE_INFO {
            // Once assembled the real service knows more (capabilities that
            // only the assembled engine can serve).
            if let Some(service) = self.service.get() {
                return service.handle(method, params).await;
            }
            return RpcReply::value(&self.engine_info);
        }
        if method == methods::ENGINE_READY {
            let mut state = self.state.clone();
            return match wait_for_deferred_engine(&mut state).await {
                Ok(()) => RpcReply::value(&serde_json::json!({ "ready": true })),
                Err(message) => Err(RpcError::Failed(message)),
            };
        }
        if method == methods::STOP_ENGINE
            && self.service.get().is_none()
            && let Some(stop) = &self.stop
        {
            let stop = stop.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let _ = stop.send(());
            });
            return RpcReply::value(&serde_json::json!({ "stopping": true }));
        }
        if AuthRpc::handles(method) {
            return self.auth.handle(method, params).await;
        }

        let mut state = self.state.clone();
        loop {
            let current = { state.borrow().clone() };
            match current {
                DeferredEngineState::Waiting => {}
                DeferredEngineState::Ready => {
                    let service = self.service.get().ok_or_else(|| {
                        RpcError::Failed("engine became ready without an RPC service".into())
                    })?;
                    return service.handle(method, params).await;
                }
                DeferredEngineState::Failed(message) => return Err(RpcError::Failed(message)),
            }
            state.changed().await.map_err(|_| RpcError::Closed)?;
        }
    }
}

/// Wait until the engine is ready, or report why it never will be.
pub async fn wait_for_deferred_engine(
    state: &mut tokio::sync::watch::Receiver<DeferredEngineState>,
) -> Result<(), String> {
    loop {
        let current = { state.borrow().clone() };
        match current {
            DeferredEngineState::Waiting => {}
            DeferredEngineState::Ready => return Ok(()),
            DeferredEngineState::Failed(message) => return Err(message),
        }
        state
            .changed()
            .await
            .map_err(|_| "engine assembly ended without a result".to_string())?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct Assembled;

    #[async_trait]
    impl RpcService for Assembled {
        async fn handle(&self, method: &str, _: serde_json::Value) -> Result<RpcReply, RpcError> {
            RpcReply::value(&serde_json::json!({ "assembled": method }))
        }
    }

    fn info() -> EngineInfo {
        EngineInfo {
            device_id: "d".into(),
            workspace_scope: zeron_proto::WorkspaceScope::Synced,
            cursor_sdk_version: None,
            capabilities: Vec::new(),
            version: None,
        }
    }

    fn rpc(
        state: tokio::sync::watch::Receiver<DeferredEngineState>,
        cell: Arc<tokio::sync::OnceCell<Arc<dyn RpcService>>>,
    ) -> Arc<DeferredEngineRpc> {
        let dir = tempfile::tempdir().unwrap();
        let auth = crate::Auth::new(crate::AuthConfig::new(
            "http://127.0.0.1:1",
            dir.path().to_path_buf(),
        ));
        // The directory only has to outlive construction: nothing here signs in.
        Arc::new(DeferredEngineRpc::new(
            AuthRpc::new(auth),
            info(),
            state,
            cell,
        ))
    }

    #[tokio::test]
    async fn deferred_service_answers_identity_before_the_engine_is_ready() {
        let (tx, rx) = tokio::sync::watch::channel(DeferredEngineState::Waiting);
        let rpc = rpc(rx, Arc::default());
        assert!(
            rpc.handle(methods::ENGINE_INFO, serde_json::json!({}))
                .await
                .is_ok()
        );
        let pending = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.handle("listChats", serde_json::json!({})).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!pending.is_finished(), "data calls wait");
        tx.send_replace(DeferredEngineState::Failed("boom".into()));
        assert!(matches!(
            pending.await.unwrap(),
            Err(RpcError::Failed(message)) if message == "boom"
        ));
    }

    #[tokio::test]
    async fn waiting_calls_reach_the_assembled_service_and_identity_follows_it() {
        let (tx, rx) = tokio::sync::watch::channel(DeferredEngineState::Waiting);
        let cell: Arc<tokio::sync::OnceCell<Arc<dyn RpcService>>> = Arc::default();
        let rpc = rpc(rx, cell.clone());
        let waiting = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.handle("listChats", serde_json::json!({})).await }
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiting.is_finished());
        cell.set(Arc::new(Assembled)).ok().unwrap();
        tx.send_replace(DeferredEngineState::Ready);
        let reply = waiting.await.unwrap().unwrap();
        assert!(matches!(reply, RpcReply::Value(v) if v["assembled"] == "listChats"));
        // Once assembled, identity comes from the real service.
        let identity = rpc
            .handle(methods::ENGINE_INFO, serde_json::json!({}))
            .await
            .unwrap();
        assert!(matches!(identity, RpcReply::Value(v) if v["assembled"] == methods::ENGINE_INFO));
    }

    #[tokio::test]
    async fn stop_is_honoured_while_waiting_and_left_to_the_service_once_ready() {
        let (tx, rx) = tokio::sync::watch::channel(DeferredEngineState::Waiting);
        let cell: Arc<tokio::sync::OnceCell<Arc<dyn RpcService>>> = Arc::default();
        let (stop_tx, mut stop_rx) = tokio::sync::mpsc::unbounded_channel();
        let dir = tempfile::tempdir().unwrap();
        let auth = crate::Auth::new(crate::AuthConfig::new(
            "http://127.0.0.1:1",
            dir.path().to_path_buf(),
        ));
        let rpc =
            DeferredEngineRpc::new(AuthRpc::new(auth), info(), rx, cell.clone()).with_stop(stop_tx);
        rpc.handle(methods::STOP_ENGINE, serde_json::json!({}))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), stop_rx.recv())
            .await
            .expect("the stop is delivered")
            .expect("open channel");
        // Ready: the assembled service owns the method.
        cell.set(Arc::new(Assembled)).ok().unwrap();
        tx.send_replace(DeferredEngineState::Ready);
        let reply = rpc
            .handle(methods::STOP_ENGINE, serde_json::json!({}))
            .await
            .unwrap();
        assert!(matches!(reply, RpcReply::Value(v) if v["assembled"] == methods::STOP_ENGINE));
    }
}
