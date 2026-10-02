//! Laptop-side Cloud + credential vault calls (docs/design/cloud-device.md,
//! "Engine changes → Laptop side").
//!
//! A session run on Cloud (the checkout picker's Cloud, on a project whose
//! GitHub repository Cloud reaches) gets its own *session device* in its own
//! sandbox; the account's one logical Cloud device id has no engine and is
//! what provider connections are authorized for. This module holds the edge
//! client for the account, its sessions and the vault.
//!
//! Every edge call acts for the signed-in user with their own bearer — the
//! RPCs that reach this module are IPC-only, never routed by `targetDeviceId`.
//! A runtime that cannot use Cloud at all (local-only profile, development
//! without an edge, signed out, no organization) answers the status calls with
//! `available: false` and never touches the network.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use zeron_proto::{
    CloudSession, CloudSessions, CloudStatus, CloudUsage, GithubConnectProgress,
    GithubDeviceFlow, GithubRepo, HarnessId, VaultProvider, VaultStatus,
};
use zeron_rpc::{RpcError, RpcReply, TokenError, TokenSource, methods, parse_params};

use crate::EngineError;
use crate::agent_accounts::{AgentAccounts, CaptureSink};
use crate::http_error::describe_http_error;

/// Edge calls here are a few DO hops; the vault adds, for GitHub polls and
/// repository listings, a github.com round trip or two.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Shown when a mutation is attempted on a runtime that can't use Cloud.
const UNAVAILABLE: &str =
    "Cloud needs a signed-in Zeron account with sync on. Sign in from Settings → Account.";

fn http() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        })
        .clone()
}

/// Lifecycle actions on one session's sandbox
/// (`POST /cloud/{orgId}/sessions/{chatId}/{action}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    Wake,
    Sleep,
}

impl SessionAction {
    fn path(self) -> &'static str {
        match self {
            SessionAction::Wake => "wake",
            SessionAction::Sleep => "sleep",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            SessionAction::Wake => "Wake the Cloud session",
            SessionAction::Sleep => "Put the Cloud session to sleep",
        }
    }
}

/// Shown for anything forwarded to the logical Cloud device, which has no
/// engine: work happens on a session's own device.
pub const ACCOUNT_DEVICE_HAS_NO_ENGINE: &str = "Cloud itself isn't a machine — open a session";

/// The edge client for one account + organization, or the "unavailable"
/// stand-in. Cheap to clone.
#[derive(Clone)]
pub struct CloudClient {
    target: Option<Arc<Target>>,
}

struct Target {
    edge_url: String,
    org_id: String,
    token: Arc<dyn TokenSource>,
    http: reqwest::Client,
}

/// Why a request produced no value.
enum Failure {
    /// A read the edge can't route (404 / 501 / 503 — an edge from before
    /// Cloud, or one without its vault binding), or a 404 on a delete. Status
    /// reads turn it into `available: false`; anything else shows the error.
    Missing(u16, EngineError),
    Error(EngineError),
}

impl From<EngineError> for Failure {
    fn from(error: EngineError) -> Self {
        Failure::Error(error)
    }
}

impl CloudClient {
    /// A runtime that can't use Cloud: status reads answer `available: false`
    /// without a network call; mutations fail with a sign-in hint.
    pub fn unavailable() -> Self {
        Self { target: None }
    }

    /// Calls `{edge_url}/cloud/{org_id}/…` and `{edge_url}/vault/{org_id}/…`
    /// with `token`'s current bearer.
    pub fn new(
        edge_url: impl Into<String>,
        org_id: impl Into<String>,
        token: Arc<dyn TokenSource>,
    ) -> Self {
        Self::with_http(edge_url, org_id, token, http())
    }

    fn with_http(
        edge_url: impl Into<String>,
        org_id: impl Into<String>,
        token: Arc<dyn TokenSource>,
        http: reqwest::Client,
    ) -> Self {
        let org_id = org_id.into();
        if org_id.trim().is_empty() {
            return Self::unavailable();
        }
        Self {
            target: Some(Arc::new(Target {
                edge_url: edge_url.into(),
                org_id,
                token,
                http,
            })),
        }
    }

    pub fn is_available(&self) -> bool {
        self.target.is_some()
    }

    fn target(&self) -> Result<&Target, EngineError> {
        self.target
            .as_deref()
            .ok_or_else(|| EngineError::Other(UNAVAILABLE.into()))
    }

    // ── Cloud device ────────────────────────────────────────────────────────

    /// `GET /cloud/{orgId}`.
    pub async fn status(&self) -> Result<CloudStatus, EngineError> {
        let Some(target) = self.target.as_deref() else {
            return Ok(CloudStatus::default());
        };
        match target
            .call::<CloudStatus>(reqwest::Method::GET, &["cloud"], &[], None, "Cloud status")
            .await
        {
            Ok(status) => Ok(available_cloud(status)),
            Err(Failure::Missing(..)) => Ok(CloudStatus::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// `POST /cloud/{orgId}/enable` with `{}` — instant: mints and registers
    /// the logical Cloud device (sandboxes come per session).
    pub async fn enable(&self) -> Result<CloudStatus, EngineError> {
        let Some(target) = self.target.as_deref() else {
            return Ok(CloudStatus::default());
        };
        target
            .call::<CloudStatus>(
                reqwest::Method::POST,
                &["cloud", "enable"],
                &[],
                Some(serde_json::json!({})),
                "Turn on Cloud",
            )
            .await
            .map(available_cloud)
            .map_err(Failure::into_error)
    }

    /// `DELETE /cloud/{orgId}` — explicit, permanent.
    pub async fn delete(&self) -> Result<CloudStatus, EngineError> {
        let Some(target) = self.target.as_deref() else {
            return Ok(CloudStatus::default());
        };
        target
            .call::<CloudStatus>(
                reqwest::Method::DELETE,
                &["cloud"],
                &[],
                None,
                "Delete Cloud",
            )
            .await
            .map(available_cloud)
            .map_err(Failure::into_error)
    }

    // ── Cloud sessions ──────────────────────────────────────────────────────

    /// `GET /cloud/{orgId}/sessions`.
    pub async fn sessions(&self) -> Result<CloudSessions, EngineError> {
        let Some(target) = self.target.as_deref() else {
            return Ok(CloudSessions::default());
        };
        match target
            .call::<CloudSessions>(
                reqwest::Method::GET,
                &["cloud", "sessions"],
                &[],
                None,
                "Cloud sessions",
            )
            .await
        {
            Ok(sessions) => Ok(CloudSessions {
                available: true,
                ..sessions
            }),
            Err(Failure::Missing(..)) => Ok(CloudSessions::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// `POST /cloud/{orgId}/sessions` with `{chatId, spaceId, repo, branch?}` — idempotent
    /// per chat; mints the session device and starts provisioning.
    pub async fn create_session(
        &self,
        chat_id: &str,
        space_id: &str,
        repo: &GithubRepo,
        branch: Option<&str>,
    ) -> Result<CloudSession, EngineError> {
        let mut body = serde_json::json!({
            "chatId": chat_id,
            "spaceId": space_id,
            "repo": {
                "fullName": repo.full_name,
                "cloneUrl": repo.clone_url,
                "defaultBranch": repo.default_branch,
            },
        });
        if let Some(branch) = branch {
            body["branch"] = serde_json::Value::String(branch.to_owned());
        }
        self.target()?
            .call::<CloudSession>(
                reqwest::Method::POST,
                &["cloud", "sessions"],
                &[],
                Some(body),
                "Start a Cloud session",
            )
            .await
            .map_err(Failure::into_error)
    }

    /// `POST /cloud/{orgId}/sessions/{chatId}/{wake|sleep}`.
    pub async fn session_action(
        &self,
        chat_id: &str,
        action: SessionAction,
    ) -> Result<CloudSession, EngineError> {
        require_id(chat_id, "chatId")?;
        self.target()?
            .call::<CloudSession>(
                reqwest::Method::POST,
                &["cloud", "sessions", chat_id, action.path()],
                &[],
                Some(serde_json::json!({})),
                action.verb(),
            )
            .await
            .map_err(Failure::into_error)
    }

    /// `DELETE /cloud/{orgId}/sessions/{chatId}` — the sandbox and anything
    /// uncommitted in it; the transcript stays.
    pub async fn delete_session(&self, chat_id: &str) -> Result<CloudSession, EngineError> {
        require_id(chat_id, "chatId")?;
        self.target()?
            .call::<CloudSession>(
                reqwest::Method::DELETE,
                &["cloud", "sessions", chat_id],
                &[],
                None,
                "Delete the Cloud session",
            )
            .await
            .map_err(Failure::into_error)
    }

    /// `GET /vault/{orgId}/github/repos[?q=]` — the vault calls GitHub with
    /// the account's connection; the token never leaves it.
    pub async fn github_repos(&self, query: Option<&str>) -> Result<Vec<GithubRepo>, EngineError> {
        #[derive(Deserialize)]
        struct Repos {
            #[serde(default)]
            repos: Vec<GithubRepo>,
        }
        let query: Vec<(&str, &str)> = query
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .map(|q| ("q", q))
            .into_iter()
            .collect();
        match self
            .target()?
            .call::<Repos>(
                reqwest::Method::GET,
                &["vault", "github", "repos"],
                &query,
                None,
                "GitHub repositories",
            )
            .await
        {
            Ok(repos) => Ok(repos.repos),
            Err(Failure::Missing(404, _)) => Err(EngineError::Other(
                crate::credentials::GITHUB_NOT_CONNECTED.into(),
            )),
            Err(failure) => Err(failure.into_error()),
        }
    }

    /// `GET /vault/{orgId}/github/branches?repo=owner/name`: branch names
    /// through the user's GitHub connection (the vault calls GitHub).
    pub async fn github_branches(&self, repo: &str) -> Result<Vec<String>, EngineError> {
        #[derive(Deserialize)]
        struct Branches {
            #[serde(default)]
            branches: Vec<String>,
        }
        match self
            .target()?
            .call::<Branches>(
                reqwest::Method::GET,
                &["vault", "github", "branches"],
                &[("repo", repo)],
                None,
                "GitHub branches",
            )
            .await
        {
            Ok(branches) => Ok(branches.branches),
            Err(failure) => Err(failure.into_error()),
        }
    }

    /// `GET /cloud/{orgId}/usage[?month=YYYY-MM]`.
    pub async fn usage(&self, month: Option<&str>) -> Result<CloudUsage, EngineError> {
        if let Some(month) = month
            && !is_month(month)
        {
            return Err(EngineError::Other(format!(
                "month must look like 2026-10, not {month:?}"
            )));
        }
        let Some(target) = self.target.as_deref() else {
            return Ok(CloudUsage::default());
        };
        let query: Vec<(&str, &str)> = month.map(|m| ("month", m)).into_iter().collect();
        match target
            .call::<CloudUsage>(
                reqwest::Method::GET,
                &["cloud", "usage"],
                &query,
                None,
                "Cloud usage",
            )
            .await
        {
            Ok(usage) => Ok(CloudUsage {
                available: true,
                ..usage
            }),
            Err(Failure::Missing(..)) => Ok(CloudUsage::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    // ── Credential vault ────────────────────────────────────────────────────

    /// `GET /vault/{orgId}`. A vault binding that isn't deployed (404 / 501 /
    /// 503) reads as `available: false`, like a signed-out runtime.
    pub async fn vault_status(&self) -> Result<VaultStatus, EngineError> {
        let Some(target) = self.target.as_deref() else {
            return Ok(VaultStatus::default());
        };
        match target
            .call::<VaultStatus>(reqwest::Method::GET, &["vault"], &[], None, "Vault status")
            .await
        {
            Ok(status) => Ok(available_vault(status)),
            Err(Failure::Missing(..)) => Ok(VaultStatus::default()),
            Err(Failure::Error(error)) => Err(error),
        }
    }

    /// `PUT /vault/{orgId}/credentials/{provider}` with
    /// `{material: {key}, authorizedDevices}` — the user's own API key.
    pub async fn put_api_key(
        &self,
        provider: VaultProvider,
        key: &str,
        authorized_devices: Vec<String>,
    ) -> Result<VaultStatus, EngineError> {
        if !matches!(
            provider,
            VaultProvider::AnthropicKey | VaultProvider::OpenaiKey
        ) {
            return Err(EngineError::Other(format!(
                "{} is not an API-key provider (use anthropic-key or openai-key)",
                provider.as_str()
            )));
        }
        let key = key.trim();
        if key.is_empty() {
            return Err(EngineError::Other("Paste an API key first.".into()));
        }
        if key.chars().any(char::is_whitespace) {
            return Err(EngineError::Other(
                "That doesn't look like an API key (it contains spaces).".into(),
            ));
        }
        self.put_credential(
            provider,
            serde_json::json!({ "key": key }),
            authorized_devices,
        )
        .await
    }

    /// `PUT /vault/{orgId}/credentials/codex` with
    /// `{material: {authJson}, authorizedDevices}` — a `codex login` result.
    pub async fn put_codex_auth(
        &self,
        auth_json: serde_json::Value,
        authorized_devices: Vec<String>,
    ) -> Result<VaultStatus, EngineError> {
        self.put_credential(
            VaultProvider::Codex,
            serde_json::json!({ "authJson": auth_json }),
            authorized_devices,
        )
        .await
    }

    /// `PUT /vault/{orgId}/credentials/claude` with
    /// `{material: {claudeAiOauth}, authorizedDevices}` — a Claude sign-in
    /// through Claude's own OAuth flow.
    pub async fn put_claude_auth(
        &self,
        claude_ai_oauth: serde_json::Value,
        authorized_devices: Vec<String>,
    ) -> Result<VaultStatus, EngineError> {
        self.put_credential(
            VaultProvider::Claude,
            serde_json::json!({ "claudeAiOauth": claude_ai_oauth }),
            authorized_devices,
        )
        .await
    }

    async fn put_credential(
        &self,
        provider: VaultProvider,
        material: serde_json::Value,
        authorized_devices: Vec<String>,
    ) -> Result<VaultStatus, EngineError> {
        self.target()?
            .call::<VaultStatus>(
                reqwest::Method::PUT,
                &["vault", "credentials", provider.as_str()],
                &[],
                Some(serde_json::json!({
                    "material": material,
                    "authorizedDevices": authorized_devices,
                })),
                "Save credential",
            )
            .await
            .map(available_vault)
            .map_err(Failure::into_error)
    }

    /// `PATCH /vault/{orgId}/credentials/{provider}` with `{authorizedDevices}`.
    pub async fn authorize(
        &self,
        provider: VaultProvider,
        authorized_devices: Vec<String>,
    ) -> Result<VaultStatus, EngineError> {
        self.target()?
            .call::<VaultStatus>(
                reqwest::Method::PATCH,
                &["vault", "credentials", provider.as_str()],
                &[],
                Some(serde_json::json!({ "authorizedDevices": authorized_devices })),
                "Update devices",
            )
            .await
            .map(available_vault)
            .map_err(Failure::into_error)
    }

    /// `DELETE /vault/{orgId}/credentials/{provider}`.
    pub async fn disconnect(&self, provider: VaultProvider) -> Result<VaultStatus, EngineError> {
        self.target()?
            .call::<VaultStatus>(
                reqwest::Method::DELETE,
                &["vault", "credentials", provider.as_str()],
                &[],
                None,
                "Disconnect",
            )
            .await
            .map(available_vault)
            .map_err(Failure::into_error)
    }

    /// `POST /vault/{orgId}/devices/{deviceId}/revoke`.
    pub async fn revoke_device(&self, device_id: &str) -> Result<VaultStatus, EngineError> {
        if device_id.trim().is_empty() {
            return Err(EngineError::Other("deviceId is required".into()));
        }
        self.target()?
            .call::<VaultStatus>(
                reqwest::Method::POST,
                &["vault", "devices", device_id, "revoke"],
                &[],
                Some(serde_json::json!({})),
                "Revoke device",
            )
            .await
            .map(available_vault)
            .map_err(Failure::into_error)
    }

    /// `POST /vault/{orgId}/github/device` with `{authorizedDevices}`.
    pub async fn github_start(
        &self,
        authorized_devices: Vec<String>,
    ) -> Result<GithubDeviceFlow, EngineError> {
        self.target()?
            .call::<GithubDeviceFlow>(
                reqwest::Method::POST,
                &["vault", "github", "device"],
                &[],
                Some(serde_json::json!({ "authorizedDevices": authorized_devices })),
                "Connect GitHub",
            )
            .await
            .map_err(Failure::into_error)
    }

    /// `POST /vault/{orgId}/github/device/{flowId}`.
    pub async fn github_poll(&self, flow_id: &str) -> Result<GithubConnectProgress, EngineError> {
        if flow_id.trim().is_empty() {
            return Err(EngineError::Other("flowId is required".into()));
        }
        self.target()?
            .call::<GithubConnectProgress>(
                reqwest::Method::POST,
                &["vault", "github", "device", flow_id],
                &[],
                Some(serde_json::json!({})),
                "Connect GitHub",
            )
            .await
            .map_err(Failure::into_error)
    }
}

// ── RPC surface ─────────────────────────────────────────────────────────────

/// The IPC-only methods this module answers (none are in `forwardable`).
pub(crate) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::CLOUD_STATUS
            | methods::CLOUD_ENABLE
            | methods::CLOUD_DELETE
            | methods::CLOUD_USAGE
            | methods::CLOUD_SESSIONS
            | methods::CLOUD_SESSION_WAKE
            | methods::CLOUD_SESSION_SLEEP
            | methods::CLOUD_SESSION_DELETE
            | methods::VAULT_STATUS
            | methods::VAULT_CONNECT_CODEX
            | methods::VAULT_CONNECT_CLAUDE
            | methods::VAULT_PUT_API_KEY
            | methods::VAULT_AUTHORIZE
            | methods::VAULT_DISCONNECT
            | methods::VAULT_REVOKE_DEVICE
            | methods::GITHUB_CONNECT_START
            | methods::GITHUB_CONNECT_POLL
    )
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct UsageParams {
    #[serde(default)]
    month: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DevicesParams {
    #[serde(default)]
    authorized_devices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutKeyParams {
    provider: VaultProvider,
    key: String,
    #[serde(default)]
    authorized_devices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderParams {
    provider: VaultProvider,
    #[serde(default)]
    authorized_devices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceParams {
    device_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlowParams {
    flow_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatParams {
    chat_id: String,
}

/// Ids interpolated into an edge path: the edge's own id charset, so a value
/// can never add a path segment or query.
fn require_id(value: &str, what: &str) -> Result<(), EngineError> {
    let ok = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(EngineError::Other(format!("malformed {what}")))
    }
}

/// `{}`-shaped params may arrive as `null` (the frame omits them).
fn params<T: DeserializeOwned>(params: serde_json::Value) -> Result<T, RpcError> {
    parse_params(if params.is_null() {
        serde_json::json!({})
    } else {
        params
    })
}

fn failed(error: EngineError) -> RpcError {
    RpcError::Failed(error.to_string())
}

/// Answer one Cloud / vault method for this runtime's user.
pub(crate) async fn dispatch(
    cloud: CloudClient,
    accounts: &AgentAccounts,
    method: &str,
    raw: serde_json::Value,
) -> Result<RpcReply, RpcError> {
    match method {
        methods::CLOUD_STATUS => RpcReply::value(&cloud.status().await.map_err(failed)?),
        methods::CLOUD_ENABLE => RpcReply::value(&cloud.enable().await.map_err(failed)?),
        methods::CLOUD_DELETE => RpcReply::value(&cloud.delete().await.map_err(failed)?),
        methods::CLOUD_SESSIONS => RpcReply::value(&cloud.sessions().await.map_err(failed)?),
        methods::CLOUD_SESSION_WAKE => {
            let p: ChatParams = params(raw)?;
            RpcReply::value(
                &cloud
                    .session_action(&p.chat_id, SessionAction::Wake)
                    .await
                    .map_err(failed)?,
            )
        }
        methods::CLOUD_SESSION_SLEEP => {
            let p: ChatParams = params(raw)?;
            RpcReply::value(
                &cloud
                    .session_action(&p.chat_id, SessionAction::Sleep)
                    .await
                    .map_err(failed)?,
            )
        }
        methods::CLOUD_SESSION_DELETE => {
            let p: ChatParams = params(raw)?;
            RpcReply::value(&cloud.delete_session(&p.chat_id).await.map_err(failed)?)
        }
        methods::CLOUD_USAGE => {
            let p: UsageParams = params(raw)?;
            let month = p.month.filter(|m| !m.trim().is_empty());
            RpcReply::value(&cloud.usage(month.as_deref()).await.map_err(failed)?)
        }
        methods::VAULT_STATUS => RpcReply::value(&cloud.vault_status().await.map_err(failed)?),
        methods::VAULT_PUT_API_KEY => {
            let p: PutKeyParams = params(raw)?;
            RpcReply::value(
                &cloud
                    .put_api_key(p.provider, &p.key, p.authorized_devices)
                    .await
                    .map_err(failed)?,
            )
        }
        methods::VAULT_AUTHORIZE => {
            let p: ProviderParams = params(raw)?;
            RpcReply::value(
                &cloud
                    .authorize(p.provider, p.authorized_devices)
                    .await
                    .map_err(failed)?,
            )
        }
        methods::VAULT_DISCONNECT => {
            let p: ProviderParams = params(raw)?;
            RpcReply::value(&cloud.disconnect(p.provider).await.map_err(failed)?)
        }
        methods::VAULT_REVOKE_DEVICE => {
            let p: DeviceParams = params(raw)?;
            RpcReply::value(&cloud.revoke_device(&p.device_id).await.map_err(failed)?)
        }
        methods::GITHUB_CONNECT_START => {
            let p: DevicesParams = params(raw)?;
            RpcReply::value(
                &cloud
                    .github_start(p.authorized_devices)
                    .await
                    .map_err(failed)?,
            )
        }
        methods::GITHUB_CONNECT_POLL => {
            let p: FlowParams = params(raw)?;
            RpcReply::value(&cloud.github_poll(&p.flow_id).await.map_err(failed)?)
        }
        methods::VAULT_CONNECT_CODEX | methods::VAULT_CONNECT_CLAUDE => {
            let p: DevicesParams = params(raw)?;
            // Refuse before any browser opens: nowhere to upload to.
            if !cloud.is_available() {
                return Err(RpcError::Failed(UNAVAILABLE.into()));
            }
            let codex = method == methods::VAULT_CONNECT_CODEX;
            let devices = p.authorized_devices;
            let capture: CaptureSink = Arc::new(move |credential| {
                let cloud = cloud.clone();
                let devices = devices.clone();
                Box::pin(async move {
                    let uploaded = if codex {
                        cloud.put_codex_auth(credential, devices).await
                    } else {
                        cloud.put_claude_auth(credential, devices).await
                    };
                    uploaded.map(drop).map_err(|e| e.to_string())
                })
            });
            let harness = if codex {
                HarnessId::Codex
            } else {
                HarnessId::ClaudeCode
            };
            let start = accounts
                .start_capture_login(harness, capture)
                .await
                .map_err(failed)?;
            RpcReply::value(&start)
        }
        other => Err(RpcError::UnknownMethod(other.to_string())),
    }
}

impl Failure {
    fn into_error(self) -> EngineError {
        match self {
            Failure::Missing(_, error) | Failure::Error(error) => error,
        }
    }
}

impl Target {
    /// `{edge}/{root}/{orgId}/{rest…}` — every segment percent-encoded.
    fn url(&self, segments: &[&str], query: &[(&str, &str)]) -> Result<reqwest::Url, EngineError> {
        let mut url = reqwest::Url::parse(&self.edge_url)
            .map_err(|e| EngineError::Other(format!("invalid edge url: {e}")))?;
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| EngineError::Other("invalid edge url".into()))?;
            path.pop_if_empty();
            let (root, rest) = segments.split_first().expect("a route root");
            path.push(root);
            path.push(&self.org_id);
            path.extend(rest);
        }
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        Ok(url)
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        segments: &[&str],
        query: &[(&str, &str)],
        body: Option<serde_json::Value>,
        what: &str,
    ) -> Result<T, Failure> {
        let bytes = self.call_raw(method, segments, query, body, what).await?;
        serde_json::from_slice::<T>(&bytes).map_err(|e| {
            Failure::Error(EngineError::Other(format!(
                "{what}: unexpected reply from Zeron's servers ({e})"
            )))
        })
    }

    async fn call_raw(
        &self,
        method: reqwest::Method,
        segments: &[&str],
        query: &[(&str, &str)],
        body: Option<serde_json::Value>,
        what: &str,
    ) -> Result<Vec<u8>, Failure> {
        let url = self.url(segments, query)?;
        let is_read = method == reqwest::Method::GET;
        let is_delete = method == reqwest::Method::DELETE;
        let token = self.token.token().await.map_err(token_error)?;
        let mut request = self.http.request(method, url).bearer_auth(token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|e| {
            EngineError::Other(format!(
                "Couldn't reach Zeron's servers: {}",
                describe_http_error(e)
            ))
        })?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(|e| {
            EngineError::Other(format!(
                "{what}: the reply was cut off: {}",
                describe_http_error(e)
            ))
        })?;
        if !(200..300).contains(&status) {
            // A status read the edge can't route (an edge from before Cloud,
            // or one deployed without its vault binding) is "not offered
            // here", not a failure to show.
            let error = edge_error(status, &bytes, what);
            if (is_read && matches!(status, 404 | 501 | 503)) || (is_delete && status == 404) {
                return Err(Failure::Missing(status, error));
            }
            return Err(Failure::Error(error));
        }
        Ok(bytes.to_vec())
    }
}

fn available_cloud(status: CloudStatus) -> CloudStatus {
    CloudStatus {
        available: true,
        ..status
    }
}

fn available_vault(status: VaultStatus) -> VaultStatus {
    VaultStatus {
        available: true,
        ..status
    }
}

fn is_month(month: &str) -> bool {
    let bytes = month.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || b.is_ascii_digit())
        && matches!(
            &month[5..],
            "01" | "02" | "03" | "04" | "05" | "06" | "07" | "08" | "09" | "10" | "11" | "12"
        )
}

fn token_error(error: TokenError) -> Failure {
    Failure::Error(EngineError::Other(match error {
        TokenError::SignedOut => UNAVAILABLE.to_string(),
        other => format!("Couldn't refresh your Zeron session: {other}"),
    }))
}

/// The edge's `{error, message}` body.
#[derive(Deserialize)]
struct EdgeErrorBody {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

/// Turn an edge error reply into text a person can act on. The edge's own
/// `message` is written for users and wins; the code and the status only fill
/// in when it is missing.
fn edge_error(status: u16, body: &[u8], what: &str) -> EngineError {
    let parsed = serde_json::from_slice::<EdgeErrorBody>(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|b| b.message.as_deref())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_owned);
    let code = parsed.and_then(|b| b.error);
    let text = message.unwrap_or_else(|| match (code.as_deref(), status) {
        (Some("disabled"), _) => "The credential vault is turned off for this account.".into(),
        (Some("needs_reconnect"), _) => {
            "The provider rejected the stored sign-in — connect it again from this device.".into()
        }
        (Some("device_revoked"), _) => "That device's access was revoked.".into(),
        (Some("not_authorized"), _) => "That device isn't allowed to use this credential.".into(),
        (Some("upstream"), _) => "The provider didn't answer — try again in a moment.".into(),
        (_, 400) => format!("{what}: the request was rejected."),
        (_, 401) => "Your Zeron session expired — sign in again.".into(),
        (_, 403) => "This account can't manage Cloud for this workspace.".into(),
        (_, 404) => format!("{what}: not found."),
        (_, 409) => "Cloud is busy with another change — try again in a moment.".into(),
        (_, 429) => "Too many requests — try again in a moment.".into(),
        (_, 501 | 503) => "Cloud isn't available on this Zeron server yet.".into(),
        (_, status) if status >= 500 => {
            format!("{what} failed on Zeron's servers ({status}) — try again.")
        }
        (_, status) => format!("{what} failed ({status})."),
    });
    tracing::warn!(status, code = code.as_deref().unwrap_or(""), %what, "cloud: edge refused");
    EngineError::Other(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use zeron_proto::{CloudState, GithubConnectState, VaultConnectionStatus};

    #[derive(Debug, Clone)]
    struct Recorded {
        method: String,
        target: String,
        authorization: Option<String>,
        body: serde_json::Value,
    }

    type Route = fn(&str, &str, &serde_json::Value) -> (u16, serde_json::Value);

    /// A tiny HTTP/1.1 edge: one request per connection, answered by `route`.
    struct MockEdge {
        url: String,
        requests: Arc<Mutex<Vec<Recorded>>>,
    }

    impl MockEdge {
        async fn start(route: Route) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let log = requests.clone();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let log = log.clone();
                    tokio::spawn(async move {
                        let mut raw = Vec::new();
                        let mut chunk = [0u8; 4096];
                        let header_end = loop {
                            let Ok(n) = stream.read(&mut chunk).await else {
                                return;
                            };
                            if n == 0 {
                                return;
                            }
                            raw.extend_from_slice(&chunk[..n]);
                            if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                                break at + 4;
                            }
                        };
                        let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
                        let mut lines = head.lines();
                        let mut request_line = lines.next().unwrap_or("").split_whitespace();
                        let method = request_line.next().unwrap_or("").to_string();
                        let target = request_line.next().unwrap_or("").to_string();
                        let mut length = 0usize;
                        let mut authorization = None;
                        for line in lines {
                            if let Some((name, value)) = line.split_once(':') {
                                let value = value.trim();
                                if name.eq_ignore_ascii_case("content-length") {
                                    length = value.parse().unwrap_or(0);
                                } else if name.eq_ignore_ascii_case("authorization") {
                                    authorization = Some(value.to_string());
                                }
                            }
                        }
                        while raw.len() < header_end + length {
                            let Ok(n) = stream.read(&mut chunk).await else {
                                return;
                            };
                            if n == 0 {
                                break;
                            }
                            raw.extend_from_slice(&chunk[..n]);
                        }
                        let body = serde_json::from_slice(&raw[header_end..])
                            .unwrap_or(serde_json::Value::Null);
                        let (status, reply) = route(&method, &target, &body);
                        log.lock().unwrap().push(Recorded {
                            method,
                            target,
                            authorization,
                            body,
                        });
                        let reply = reply.to_string();
                        let response = format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                            reply.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
            });
            Self { url, requests }
        }

        fn client(&self, org: &str) -> CloudClient {
            CloudClient::new(
                self.url.clone(),
                org,
                Arc::new(zeron_rpc::StaticToken("user-bearer".into())),
            )
        }

        fn requests(&self) -> Vec<Recorded> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn vault_view() -> serde_json::Value {
        serde_json::json!({
            "connections": [{
                "provider": "anthropic-key",
                "status": "connected",
                "authorizedDevices": ["cloud-1"],
                "account": "…abcd",
                "updatedAt": 1_700_000_000_000_i64
            }, {
                "provider": "codex",
                "status": "needsReconnect",
                "authorizedDevices": [],
                "updatedAt": 1_700_000_000_001_i64
            }],
            "devices": [{"deviceId": "cloud-1", "kind": "cloud", "enrolledAt": 1}],
            "available": true
        })
    }

    fn edge(method: &str, target: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
        match (method, target) {
            ("GET", "/cloud/org_1") => (
                200,
                serde_json::json!({"state": "ready", "deviceId": "cloud-1", "lastActiveAt": 5,
                    "awakeSessions": 1, "maxAwakeSessions": 5}),
            ),
            ("POST", "/cloud/org_1/enable") => (
                200,
                serde_json::json!({"state": "ready", "deviceId": "cloud-1"}),
            ),
            ("GET", "/cloud/org_1/sessions") => (
                200,
                serde_json::json!({"sessions": [{"chatId": "chat-1", "deviceId": "cloud-s1",
                    "spaceId": "sp-1", "state": "sleeping", "createdAt": 3}]}),
            ),
            ("POST", "/cloud/org_1/sessions") => {
                assert_eq!(body["chatId"], "chat-2");
                assert_eq!(body["branch"], "feature/x");
                assert_eq!(body["repo"]["fullName"], "acme/app");
                assert_eq!(body["repo"]["defaultBranch"], "main");
                (
                    200,
                    serde_json::json!({"chatId": "chat-2", "deviceId": "cloud-s2",
                        "spaceId": body["spaceId"], "state": "provisioning", "createdAt": 4}),
                )
            }
            ("POST", "/cloud/org_1/sessions/chat-1/wake") => (
                409,
                serde_json::json!({"error": "too_many_awake", "message": "All 5 of your Cloud sessions that may run at once are busy."}),
            ),
            ("POST", "/cloud/org_1/sessions/chat-1/sleep") => (500, serde_json::json!({})),
            ("DELETE", "/cloud/org_1/sessions/chat-1") => (
                200,
                serde_json::json!({"chatId": "chat-1", "deviceId": "cloud-s1",
                    "spaceId": "sp-1", "state": "deleting", "createdAt": 3}),
            ),
            ("DELETE", "/cloud/org_1") => (200, serde_json::json!({"state": "off"})),
            ("GET", "/cloud/org_1/usage?month=2026-09") => (
                200,
                serde_json::json!({
                    "month": "2026-09", "seconds": 7200, "dollars": 0.5,
                    "sandboxes": [{"sandboxId": "bx_1", "sandboxType": "default",
                        "seconds": 7200, "dollars": 0.5, "running": false,
                        "reconciledAt": 9}],
                    "closed": true
                }),
            ),
            ("GET", "/cloud/org_1/usage") => (
                200,
                serde_json::json!({"month": "2026-10", "seconds": 0, "dollars": 0.0}),
            ),
            ("GET", "/vault/org_1") => (200, vault_view()),
            ("PUT", "/vault/org_1/credentials/anthropic-key") => {
                assert_eq!(body["material"]["key"], "sk-ant-secret");
                (200, vault_view())
            }
            ("PUT", "/vault/org_1/credentials/openai-key") => (
                400,
                serde_json::json!({"error": "bad_request", "message": "That key was rejected by OpenAI."}),
            ),
            ("PATCH", "/vault/org_1/credentials/codex") => {
                (403, serde_json::json!({"error": "forbidden"}))
            }
            ("POST", "/vault/org_1/devices/cloud-1/revoke") => (200, vault_view()),
            ("POST", "/vault/org_1/github/device") => (
                200,
                serde_json::json!({"flowId": "f1", "userCode": "ABCD-1234",
                    "verificationUri": "https://github.com/login/device",
                    "intervalSecs": 5, "expiresAt": 99}),
            ),
            ("POST", "/vault/org_1/github/device/f1") => (
                200,
                serde_json::json!({"state": "connected", "account": "octocat"}),
            ),
            // An edge without the vault binding / cloud routes.
            ("GET", "/vault/org_2") => (503, serde_json::json!({})),
            ("GET", "/cloud/org_2") => (404, serde_json::json!({})),
            _ => (
                404,
                serde_json::json!({"error": "not_found", "message": "no route"}),
            ),
        }
    }

    #[tokio::test]
    async fn cloud_lifecycle_against_the_edge() {
        let edge = MockEdge::start(edge).await;
        let client = edge.client("org_1");

        let status = client.status().await.unwrap();
        assert_eq!(status.state, CloudState::Ready);
        assert_eq!(status.device_id.as_deref(), Some("cloud-1"));
        assert_eq!(status.last_active_at, Some(5));
        assert_eq!((status.awake_sessions, status.max_awake_sessions), (1, 5));
        assert!(status.available, "edge replies are available");

        let enabled = client.enable().await.unwrap();
        assert_eq!(enabled.state, CloudState::Ready);
        assert!(enabled.available);

        let sessions = client.sessions().await.unwrap();
        assert_eq!(sessions.sessions[0].device_id, "cloud-s1");
        assert_eq!(sessions.sessions[0].state, CloudState::Sleeping);
        let created = client
            .create_session(
                "chat-2",
                "sp-1",
                &GithubRepo {
                    full_name: "acme/app".into(),
                    clone_url: "https://github.com/acme/app.git".into(),
                    default_branch: "main".into(),
                    private: true,
                    description: None,
                    pushed_at: 0,
                },
                Some("feature/x"),
            )
            .await
            .unwrap();
        assert_eq!(created.device_id, "cloud-s2");
        assert_eq!(created.state, CloudState::Provisioning);

        // Edge `{error, message}` → the edge's own user-facing message.
        let busy = client
            .session_action("chat-1", SessionAction::Wake)
            .await
            .unwrap_err();
        assert_eq!(
            busy.to_string(),
            "All 5 of your Cloud sessions that may run at once are busy."
        );
        // A malformed chat id never reaches the edge.
        assert!(
            client
                .session_action("chat/../x", SessionAction::Wake)
                .await
                .is_err()
        );
        // No message → status-derived text, never a bare code.
        let failed = client
            .session_action("chat-1", SessionAction::Sleep)
            .await
            .unwrap_err();
        assert!(
            failed.to_string().contains("(500)"),
            "{}",
            failed.to_string()
        );

        let deleting = client.delete_session("chat-1").await.unwrap();
        assert_eq!(deleting.state, CloudState::Deleting);
        let deleted = client.delete().await.unwrap();
        assert_eq!(deleted.state, CloudState::Off);
        assert!(deleted.available);

        let requests = edge.requests();
        assert!(
            requests
                .iter()
                .all(|r| r.authorization.as_deref() == Some("Bearer user-bearer")),
            "every call carries the user's bearer: {requests:?}"
        );
        let enable = requests
            .iter()
            .find(|r| r.target == "/cloud/org_1/enable")
            .unwrap();
        assert_eq!(enable.method, "POST");
        assert_eq!(enable.body, serde_json::json!({}));
        assert!(
            requests
                .iter()
                .any(|r| r.method == "DELETE" && r.target == "/cloud/org_1")
        );
    }

    #[tokio::test]
    async fn cloud_usage_month_is_optional_and_validated() {
        let edge = MockEdge::start(edge).await;
        let client = edge.client("org_1");

        let usage = client.usage(Some("2026-09")).await.unwrap();
        assert_eq!(usage.month, "2026-09");
        assert_eq!(usage.seconds, 7200);
        assert_eq!(usage.sandboxes.len(), 1);
        assert!(usage.closed);
        assert!(usage.available);

        let current = client.usage(None).await.unwrap();
        assert_eq!(current.month, "2026-10");
        assert!(current.available);
        assert!(
            edge.requests()
                .iter()
                .any(|r| r.target == "/cloud/org_1/usage"),
            "no query when the month is omitted"
        );

        assert!(client.usage(Some("2026-13")).await.is_err());
        assert!(client.usage(Some("../x")).await.is_err());
    }

    #[tokio::test]
    async fn vault_status_keys_and_github_against_the_edge() {
        let edge = MockEdge::start(edge).await;
        let client = edge.client("org_1");

        let status = client.vault_status().await.unwrap();
        assert!(status.available);
        assert_eq!(status.connections.len(), 2);
        assert_eq!(status.connections[0].provider, VaultProvider::AnthropicKey);
        assert_eq!(
            status.connections[1].status,
            VaultConnectionStatus::NeedsReconnect
        );
        assert_eq!(status.devices[0].device_id, "cloud-1");

        let saved = client
            .put_api_key(
                VaultProvider::AnthropicKey,
                "  sk-ant-secret\n",
                vec!["cloud-1".into()],
            )
            .await
            .unwrap();
        assert!(saved.available);
        let put = edge
            .requests()
            .into_iter()
            .find(|r| r.method == "PUT")
            .unwrap();
        assert_eq!(
            put.body,
            serde_json::json!({
                "material": {"key": "sk-ant-secret"},
                "authorizedDevices": ["cloud-1"]
            })
        );

        // Validation happens before any network call.
        let before = edge.requests().len();
        for (provider, key) in [
            (VaultProvider::Codex, "sk-1"),
            (VaultProvider::Github, "ghp_1"),
            (VaultProvider::AnthropicKey, "   "),
            (VaultProvider::OpenaiKey, "sk 1"),
        ] {
            assert!(
                client.put_api_key(provider, key, vec![]).await.is_err(),
                "{provider:?} {key:?} must be refused"
            );
        }
        assert_eq!(edge.requests().len(), before);

        // Error mapping: edge message, then code/status fallbacks.
        let rejected = client
            .put_api_key(VaultProvider::OpenaiKey, "sk-bad", vec![])
            .await
            .unwrap_err();
        assert_eq!(rejected.to_string(), "That key was rejected by OpenAI.");
        let forbidden = client
            .authorize(VaultProvider::Codex, vec!["cloud-1".into()])
            .await
            .unwrap_err();
        assert_eq!(
            forbidden.to_string(),
            "This account can't manage Cloud for this workspace."
        );

        assert!(client.revoke_device("cloud-1").await.unwrap().available);

        let flow = client.github_start(vec!["cloud-1".into()]).await.unwrap();
        assert_eq!(flow.flow_id, "f1");
        assert_eq!(flow.user_code, "ABCD-1234");
        assert_eq!(flow.interval_secs, 5);
        let progress = client.github_poll("f1").await.unwrap();
        assert_eq!(progress.state, GithubConnectState::Connected);
        assert_eq!(progress.account.as_deref(), Some("octocat"));
        let start = edge
            .requests()
            .into_iter()
            .find(|r| r.target == "/vault/org_1/github/device")
            .unwrap();
        assert_eq!(
            start.body,
            serde_json::json!({"authorizedDevices": ["cloud-1"]})
        );
    }

    #[tokio::test]
    async fn missing_routes_and_unavailable_runtimes_read_as_unavailable() {
        let edge = MockEdge::start(edge).await;
        // Vault binding / cloud routes not deployed on this edge.
        let missing = edge.client("org_2");
        assert!(!missing.vault_status().await.unwrap().available);
        assert!(!missing.status().await.unwrap().available);

        // No edge, signed out, no org: no network call at all.
        let calls = edge.requests().len();
        for client in [
            CloudClient::unavailable(),
            CloudClient::new(
                edge.url.clone(),
                "  ",
                Arc::new(zeron_rpc::StaticToken("t".into())),
            ),
        ] {
            assert!(!client.is_available());
            let status = client.status().await.unwrap();
            assert_eq!(status.state, CloudState::Off);
            assert!(!status.available);
            assert!(!client.enable().await.unwrap().available);
            assert!(!client.sessions().await.unwrap().available);
            assert!(!client.delete().await.unwrap().available);
            assert!(!client.usage(None).await.unwrap().available);
            assert!(!client.vault_status().await.unwrap().available);
            let error = client
                .put_api_key(VaultProvider::AnthropicKey, "sk-1", vec![])
                .await
                .unwrap_err();
            assert!(error.to_string().contains("sign"), "{error}");
            assert!(client.github_start(vec![]).await.is_err());
        }
        assert_eq!(edge.requests().len(), calls);
    }

    #[test]
    fn months_are_strict() {
        assert!(is_month("2026-01"));
        assert!(is_month("1999-12"));
        assert!(!is_month("2026-1"));
        assert!(!is_month("2026-00"));
        assert!(!is_month("2026/01"));
        assert!(!is_month("２０２６-01"));
    }
}
