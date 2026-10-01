//! The engine on this same device, over its IPC port — for a viewer whose
//! device also runs an engine (the Android app, docs/android.md). The two are
//! one device: the viewer takes the engine's device id, signs the account in
//! *through* the engine (the engine owns it, as a desktop app's UI signs in
//! through its engine), and presents the engine's edge bearer
//! ([`crate::Credentials::Engine`]). The engine is the only refresher: WorkOS
//! refresh tokens rotate, so a second refresher would race it.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::{RpcClient, RpcError, methods};

use crate::error::{ClientError, Result};

/// What the engine's `EdgeBearer` answers: the edge it syncs through, the
/// identity it syncs as, and the bearer it presents there.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeBearer {
    pub edge_url: String,
    pub user_id: String,
    pub org_id: String,
    /// `None` once the engine's session is gone (`signedOut`).
    #[serde(default)]
    pub bearer: Option<String>,
    /// When the bearer stops working; `None` = never (a shared-secret or dev
    /// bearer). The engine rotates it [`ENGINE_REFRESH_SLACK_MS`] earlier.
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    #[serde(default)]
    pub signed_out: bool,
}

/// The engine refreshes its access token this long before it expires
/// (`TOKEN_SLACK` in zeron-engine's auth). A viewer re-asking any later than
/// that gets the rotated token.
pub const ENGINE_REFRESH_SLACK_MS: i64 = 30_000;

/// How long before `expiresAtMs` the viewer re-asks — inside the engine's
/// slack, so the answer is a token the engine has just rotated, never the
/// same nearly-expired one again.
pub const REASK_BEFORE_EXPIRY_MS: i64 = 20_000;

/// A lazily (re)dialed IPC connection to the engine on this device.
pub struct EngineLink {
    url: String,
    token: Option<String>,
    rpc: tokio::sync::Mutex<Option<Arc<RpcClient>>>,
}

impl std::fmt::Debug for EngineLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineLink")
            .field("url", &self.url)
            .finish()
    }
}

fn rpc_error(err: RpcError) -> ClientError {
    match err {
        RpcError::Transport(m) => ClientError::Network(format!("engine unreachable: {m}")),
        RpcError::Closed => ClientError::Network("engine connection closed".into()),
        RpcError::UnknownMethod(m) => ClientError::Unsupported(format!("{m} on this engine")),
        RpcError::BadParams(m) => ClientError::InvalidArgument(m),
        RpcError::Failed(m) => ClientError::HostError(m),
    }
}

impl EngineLink {
    /// `url` is the IPC WebSocket (`ws://127.0.0.1:27654`); `token` the
    /// engine's `ZERON_IPC_TOKEN`.
    pub fn new(url: impl Into<String>, token: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            url: url.into(),
            token: token.filter(|t| !t.trim().is_empty()),
            rpc: tokio::sync::Mutex::new(None),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// One call, on the client runtime. A dropped connection (the engine
    /// restarted) is redialed once.
    pub async fn call(self: &Arc<Self>, method: &str, params: Value) -> Result<Value> {
        let link = self.clone();
        let method = method.to_owned();
        crate::runtime::run(async move { link.call_rpc(&method, params).await.map_err(rpc_error) })
            .await
    }

    /// [`Self::call`] on the caller's runtime, with the wire error — for the
    /// relay, which routes this device's own host RPCs here.
    pub(crate) async fn call_rpc(
        &self,
        method: &str,
        params: Value,
    ) -> std::result::Result<Value, RpcError> {
        let mut retried = false;
        loop {
            let rpc = self.connected_rpc().await?;
            match rpc.call(method, params.clone()).await {
                Err(RpcError::Closed | RpcError::Transport(_)) if !retried => {
                    retried = true;
                    self.forget(&rpc).await;
                }
                Err(err) => {
                    if matches!(err, RpcError::Closed | RpcError::Transport(_)) {
                        self.forget(&rpc).await;
                    }
                    return Err(err);
                }
                Ok(value) => return Ok(value),
            }
        }
    }

    /// A drop-cancelled stream, acknowledged by the engine before it
    /// returns (an unknown method or bad params fail here, not as an empty
    /// stream). A dropped connection is redialed once.
    pub(crate) async fn subscribe_rpc(
        &self,
        method: &str,
        params: Value,
    ) -> std::result::Result<zeron_rpc::RpcSubscription, RpcError> {
        let mut retried = false;
        loop {
            let rpc = self.connected_rpc().await?;
            match rpc.subscribe_checked(method, params.clone()).await {
                Err(RpcError::Closed | RpcError::Transport(_)) if !retried => {
                    retried = true;
                    self.forget(&rpc).await;
                }
                Err(err) => {
                    if matches!(err, RpcError::Closed | RpcError::Transport(_)) {
                        self.forget(&rpc).await;
                    }
                    return Err(err);
                }
                Ok(stream) => return Ok(stream),
            }
        }
    }

    /// The first item of a stream method (its current value).
    async fn first(self: &Arc<Self>, method: &str) -> Result<Value> {
        let link = self.clone();
        let method = method.to_owned();
        crate::runtime::run(async move {
            let rpc = link.connected().await?;
            let mut stream = match rpc.subscribe(&method, json!({})).await {
                Ok(stream) => stream,
                Err(err) => {
                    link.forget(&rpc).await;
                    return Err(rpc_error(err));
                }
            };
            match tokio::time::timeout(std::time::Duration::from_secs(10), stream.recv()).await {
                Ok(Some(value)) => Ok(value),
                Ok(None) => {
                    link.forget(&rpc).await;
                    Err(ClientError::Network("engine stream ended".into()))
                }
                Err(_) => Err(ClientError::Network(format!("{method} timed out"))),
            }
        })
        .await
    }

    async fn connected(&self) -> Result<Arc<RpcClient>> {
        self.connected_rpc().await.map_err(rpc_error)
    }

    async fn connected_rpc(&self) -> std::result::Result<Arc<RpcClient>, RpcError> {
        let mut slot = self.rpc.lock().await;
        if let Some(rpc) = slot.as_ref() {
            return Ok(rpc.clone());
        }
        let rpc =
            Arc::new(zeron_rpc::connect_ws_with_token(&self.url, self.token.as_deref()).await?);
        *slot = Some(rpc.clone());
        Ok(rpc)
    }

    async fn forget(&self, failed: &Arc<RpcClient>) {
        let mut slot = self.rpc.lock().await;
        if slot.as_ref().is_some_and(|rpc| Arc::ptr_eq(rpc, failed)) {
            *slot = None;
        }
    }

    /// The engine's fixed device and workspace identity.
    pub async fn info(self: &Arc<Self>) -> Result<zeron_proto::EngineInfo> {
        let value = self.call(methods::ENGINE_INFO, json!({})).await?;
        serde_json::from_value(value).map_err(|e| ClientError::Internal(e.to_string()))
    }

    /// Edge, identity and a live bearer (refreshed by the engine as needed).
    pub async fn edge_bearer(self: &Arc<Self>) -> Result<EdgeBearer> {
        let value = self.call(methods::EDGE_BEARER, json!({})).await?;
        serde_json::from_value(value).map_err(|e| ClientError::Internal(e.to_string()))
    }

    /// The engine's account state (the AuthStatus stream's current value).
    pub async fn auth_state(self: &Arc<Self>) -> Result<zeron_proto::AuthState> {
        let value = self.first(methods::AUTH_STATUS).await?;
        serde_json::from_value(value).map_err(|e| ClientError::Internal(e.to_string()))
    }

    /// Start a WorkOS sign-in redirecting to `redirect_uri` (the app's deep
    /// link); finish with [`Self::complete_sign_in`]. Empty = the engine has
    /// no WorkOS (development).
    pub async fn sign_in_url(self: &Arc<Self>, redirect_uri: &str) -> Result<String> {
        let value = self
            .call(
                methods::SIGN_IN_HEADLESS,
                json!({ "redirectUri": redirect_uri }),
            )
            .await?;
        Ok(value
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
    }

    /// Hand the callback's `state` and `code` to the engine, which exchanges
    /// them and saves the session (the next engine start is synced).
    pub async fn complete_sign_in(self: &Arc<Self>, state: &str, code: &str) -> Result<()> {
        self.call(
            methods::COMPLETE_SIGN_IN,
            json!({ "code": format!("{state}.{code}") }),
        )
        .await
        .map(drop)
    }

    pub async fn list_orgs(self: &Arc<Self>) -> Result<Vec<crate::auth::AuthOrg>> {
        #[derive(Deserialize)]
        struct Reply {
            #[serde(default)]
            orgs: Vec<crate::auth::AuthOrg>,
        }
        let value = self.call(methods::LIST_ORGS, json!({})).await?;
        let reply: Reply =
            serde_json::from_value(value).map_err(|e| ClientError::Internal(e.to_string()))?;
        Ok(reply.orgs)
    }

    pub async fn select_org(self: &Arc<Self>, organization_id: &str) -> Result<()> {
        self.call(
            methods::SELECT_ORG,
            json!({ "organizationId": organization_id }),
        )
        .await
        .map(drop)
    }

    /// Remove the engine's saved session (the next start is local-only; a
    /// synced engine stops at once).
    pub async fn sign_out(self: &Arc<Self>) -> Result<()> {
        self.call(methods::SIGN_OUT, json!({})).await.map(drop)
    }
}
