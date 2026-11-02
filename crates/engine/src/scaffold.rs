//! Native Scaffold code-sandbox control plane and Comet bootstrap.
//!
//! The long-lived Scaffold OAuth bearer is consulted per request and is used only
//! as the HTTP `Authorization` header. It is never included in sandbox exec input,
//! collaboration records, watch snapshots, errors, or logs. A sandbox receives only
//! a short-lived Comet device-join credential minted by the edge.

mod native_recovery;

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use zeron_harness::CancellationToken;
use zeron_proto::{
    AccessToken, AgentRoute, AgentRouteReceipt, CAPABILITY_SESSION_ANNOTATE,
    CAPABILITY_SESSION_CHAT, CAPABILITY_SESSION_CONTROL, CAPABILITY_SESSION_ENVIRONMENT,
    CAPABILITY_SESSION_FILES, CAPABILITY_SESSION_READ, CollaborationScope, ScaffoldControlGrant,
    ScaffoldDatabaseEnvironment, ScaffoldEnvironmentControl, ScaffoldEnvironmentControlResult,
    ScaffoldEnvironmentLinks, ScaffoldEnvironmentSnapshot, SessionEnvironment,
    SessionEnvironmentSource, SessionRoomProjection,
};
use zeron_rpc::{RpcError, TokenSource};
use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::sync::{Semaphore, watch};
use tokio_util::io::ReaderStream;

use crate::now_ms;
use crate::omp_session_artifact::CapturedOmpSessionFile;
use crate::worktree_handoff::{MAX_HANDOFF_ARCHIVE_BYTES, WorktreeHandoffArchive};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
// VM placement and runtime bootstrap can outlive the metadata request deadline.
const LIFECYCLE_REQUEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const RUNTIME_START_POLL: Duration = Duration::from_millis(500);
// At 30 seconds per request, two retries cap a persistent timeout at about
// 90 seconds plus two lifecycle-validation poll delays.
const ATTACH_PROBE_TRANSPORT_RETRIES: usize = 2;
const JOIN_GRANT_TTL_SECONDS: u32 = 15 * 60;
const DEVICE_ACCESS_TTL_MS: i64 = 12 * 60 * 60 * 1000;
const SCAFFOLD_HANDOFF_CWD: &str = "/workspace/crew-handoff";

// Release compatibility gates native deployment; it does not version persisted authority profiles.
pub(crate) const SCAFFOLD_COMET_RUNTIME_VERSION: &str =
    include_str!("../../../scaffold-runtime-version.txt").trim_ascii();
// The authority profile wire is independent of the installed runtime archive version.
const SCAFFOLD_AUTHORITY_PROFILE_WIRE_VERSION: &str = "scaffold.comet-runtime.v1";
const JOIN_GRANT_PATH: [&str; 2] = ["auth", "device-grants"];
pub fn is_retryable_scaffold_control_error(error: &RpcError) -> bool {
    matches!(
        error,
        RpcError::Failed(message)
            if message.contains("scaffold_api_error:500:scaffold_request_rejected")
                || message.contains("scaffold_api_error:502:scaffold_request_rejected")
                || message.contains("scaffold_api_error:503:scaffold_request_rejected")
                || message.contains("scaffold_api_error:504:scaffold_request_rejected")
                || message.contains(
                    "scaffold_api_error:404:not_found:Sandbox agent route projection failed",
                )
                || message.contains(
                    "scaffold_api_error:409:sandbox_provider_error:Sandbox lifecycle changed while the operation was in flight",
                )
                || message.strip_prefix("scaffold_api_error:503:sandbox_runtime_starting")
                    .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(':'))
                || message.contains("scaffold_session_owner_room_unavailable")
                || message.contains(
                    "scaffold_api_error:404:sandbox_provider_error:E2B sandbox control channel is unavailable",
                )
                || (message.contains("scaffold_response_invalid: sandbox ")
                    && message.contains(" is not ready"))
                || message.contains(
                    "Scaffold agent route update returned unavailable lifecycle",
                )
    )
}

struct OmpHandoffArchive {
    file: NamedTempFile,
    archive_byte_count: u64,
    native_session_id: String,
    cwd: String,
    storage_relative_path: String,
    sha256: String,
    byte_count: u64,
}

impl OmpHandoffArchive {
    fn reopen(&self) -> std::io::Result<File> {
        self.file.reopen()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn scaffold_http_client(
    total_timeout: Option<Duration>,
) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(REQUEST_TIMEOUT)
        // Authorization-bearing requests must never follow a redirect to another origin.
        .redirect(reqwest::redirect::Policy::none());
    if let Some(timeout) = total_timeout {
        builder = builder.timeout(timeout);
    }
    builder.build()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldApiError {
    pub status: u16,
    pub code: String,
    pub message: Option<String>,
    pub retry_after: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaffoldTransportCause {
    RequestTimeout,
    ConnectFailure,
    ConnectionResetOrClosed,
    SendFailure,
}

impl fmt::Display for ScaffoldTransportCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::RequestTimeout => "request_timeout",
            Self::ConnectFailure => "connect_failure",
            Self::ConnectionResetOrClosed => "connection_reset_or_closed",
            Self::SendFailure => "send_failure",
        })
    }
}

fn classify_transport_error(error: &reqwest::Error) -> ScaffoldTransportCause {
    if error.is_timeout() {
        return ScaffoldTransportCause::RequestTimeout;
    }
    let mut source: &(dyn std::error::Error + 'static) = error;
    loop {
        if let Some(io_error) = source.downcast_ref::<std::io::Error>()
            && matches!(
                io_error.kind(),
                std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            )
        {
            return ScaffoldTransportCause::ConnectionResetOrClosed;
        }
        if let Some(hyper_error) = source.downcast_ref::<hyper::Error>()
            && (hyper_error.is_closed() || hyper_error.is_incomplete_message())
        {
            return ScaffoldTransportCause::ConnectionResetOrClosed;
        }
        let Some(next) = source.source() else {
            break;
        };
        source = next;
    }
    if error.is_connect() {
        ScaffoldTransportCause::ConnectFailure
    } else {
        ScaffoldTransportCause::SendFailure
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryImportDiagnostic {
    pub sandbox_id: String,
    pub exit_code: Option<i32>,
    pub stderr: String,
    pub manifest_sha256: String,
    pub base_sha: String,
    pub archive_bytes: u64,
    pub bundle_sha256: String,
    pub bundle_bytes: u64,
    pub prerequisite_sha: Option<String>,
    pub shallow: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IsolatedRecoveryDiagnostic {
    pub operation_id: String,
    pub sandbox_id: String,
    pub descriptor_sha256: Option<String>,
    pub exit_code: Option<i32>,
    pub stderr: String,
    pub artifacts: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("scaffold_auth_unavailable")]
    AuthUnavailable,
    #[error("scaffold_auth_temporarily_unavailable: {0}")]
    AuthTemporarilyUnavailable(String),
    #[error("scaffold_request_cancelled")]
    Cancelled,
    #[error("scaffold_request_failed: {cause}")]
    Transport {
        cause: ScaffoldTransportCause,
        before_response: bool,
        #[source]
        source: reqwest::Error,
    },
    #[error("scaffold_response_invalid: {0}")]
    InvalidResponse(String),
    #[error("scaffold_operation_unsupported: {0}")]
    UnsupportedPlatform(&'static str),
    #[error("scaffold_attach_rejected: sandbox lifecycle {0:?}")]
    AttachRejected(zeron_proto::ScaffoldLifecycle),
    #[error("scaffold_scope_invalid: {0}")]
    InvalidScope(String),
    #[error("{0}")]
    Api(ScaffoldApiErrorDisplay),
    #[error("device_join_grant_unavailable")]
    DeviceJoinGrantUnavailable,
    #[error("comet_not_installed_in_sandbox")]
    CometNotInstalled,
    /// The sandbox accepted the launch as an ordinary exec instead of routing it
    /// to native supervision: its runtime does not serve this build's contract.
    /// Deterministic for the deployment, so observation can never confirm it.
    #[error("scaffold_runtime_contract_mismatch: this Scaffold deployment ran the Crew host launch as a generic exec instead of supervising Crew runtime {SCAFFOLD_COMET_RUNTIME_VERSION}; update the deployment's sandbox runtime or use a matching Crew build")]
    RuntimeContractMismatch,
    #[error("omp_session_handoff_failed")]
    OmpSessionHandoffFailed,
    #[error("omp_session_handoff_failed:{0}")]
    HandoffStage(&'static str),
    #[error("omp_session_handoff_failed:{stage}")]
    RepositoryImport { stage: &'static str, diagnostic: Box<RepositoryImportDiagnostic> },
    #[error("omp_session_handoff_failed:{stage}")]
    IsolatedRecovery { stage: &'static str, diagnostic: Box<IsolatedRecoveryDiagnostic> },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OriginalEpochBootstrap {
    pub proof_json: String,
    pub command_outcomes: std::collections::BTreeMap<String, String>,
}

/// Separate display wrapper prevents an upstream body from accidentally entering
/// logs while preserving the exact status/code/message as typed fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldApiErrorDisplay(pub ScaffoldApiError);

impl fmt::Display for ScaffoldApiErrorDisplay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "scaffold_api_error:{}:{}", self.0.status, self.0.code)?;
        if let Some(message) = &self.0.message {
            write!(f, ":{message}")?;
        }
        Ok(())
    }
}

impl From<reqwest::Error> for ScaffoldError {
    fn from(source: reqwest::Error) -> Self {
        Self::Transport {
            cause: classify_transport_error(&source),
            before_response: false,
            source: source.without_url(),
        }
    }
}

impl ScaffoldError {
    fn request_send_failure(source: reqwest::Error) -> Self {
        Self::Transport {
            cause: classify_transport_error(&source),
            before_response: true,
            source: source.without_url(),
        }
    }

    pub(crate) fn transport_cause(&self) -> Option<ScaffoldTransportCause> {
        match self {
            Self::Transport { cause, .. } => Some(*cause),
            _ => None,
        }
    }

    pub(crate) fn is_attach_probe_transport_retryable(&self) -> bool {
        matches!(
            self,
            Self::Transport {
                cause:
                    ScaffoldTransportCause::RequestTimeout
                    | ScaffoldTransportCause::ConnectionResetOrClosed,
                before_response: true,
                ..
            }
        )
    }

    pub(crate) fn retry_after(&self) -> Option<Duration> {
        self.api_error().and_then(|error| error.retry_after)
    }

    pub(crate) fn is_transient(&self) -> bool {
        matches!(self, Self::Transport { .. } | Self::AuthTemporarilyUnavailable(_))
            // Match the existing control-plane retry contract for temporary 5xx.
            || self.api_error().is_some_and(|error| matches!(error.status, 429 | 500 | 502 | 503 | 504))
    }

    fn is_runtime_starting(&self) -> bool {
        self.api_error()
            .is_some_and(|error| error.status == 503 && error.code == "sandbox_runtime_starting")
    }

    pub fn api_error(&self) -> Option<&ScaffoldApiError> {
        match self {
            Self::Api(error) => Some(&error.0),
            _ => None,
        }
    }
}

async fn control_credential(source: &dyn TokenSource) -> Result<AccessToken, ScaffoldError> {
    match source.token().await {
        Ok(token) if !token.value.trim().is_empty() => Ok(token),
        Ok(_) | Err(zeron_rpc::TokenError::SignedOut) => Err(ScaffoldError::AuthUnavailable),
        Err(zeron_rpc::TokenError::TemporarilyUnavailable(reason)) => Err(ScaffoldError::AuthTemporarilyUnavailable(reason)),
    }
}

/// Invalidate only the credential sent, before decoding a rejected response.
async fn send_authenticated(
    source: &dyn TokenSource,
    request: reqwest::RequestBuilder,
    cancellation: &CancellationToken,
) -> Result<(reqwest::Response, u64), ScaffoldError> {
    let credential = control_credential(source).await?;
    let generation = credential.generation;
    let send = request.bearer_auth(&credential.value).send();
    drop(credential);
    tokio::pin!(send);
    let response = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
        response = &mut send => response.map_err(|error| {
            let error = ScaffoldError::request_send_failure(error);
            tracing::warn!(
                transport_cause = %error.transport_cause().expect("transport error"),
                "Scaffold request send failed"
            );
            error
        })?,
    };
    if response.status() == StatusCode::UNAUTHORIZED {
        source.invalidate(generation);
    }
    Ok((response, generation))
}

fn authenticated_error(source: &dyn TokenSource, generation: u64, error: ScaffoldError) -> ScaffoldError {
    // Resource-level 403s and temporary 503 validation failures do not revoke credentials.
    if error.api_error().is_some_and(|error| error.status == 403 && matches!(error.code.as_str(),
        "remote_code_token_forbidden" | "crew_device_authority_unavailable" | "crew_device_scope_denied"
    )) {
        source.invalidate(generation);
    }
    error
}

#[derive(Clone)]
pub struct ScaffoldClient {
    http: reqwest::Client,
    inference_http: reqwest::Client,
    origin: Url,
    project_scope: String,
    bearer: Arc<dyn TokenSource>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentAccountCredentialImport {
    pub provider: String,
    pub provider_account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub capabilities: Vec<String>,
    pub credential: AgentAccountOAuthCredential,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentAccountOAuthCredential {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google: Option<AgentAccountGoogleCredential>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentAccountGoogleCredential {
    pub client_id: String,
    pub client_secret: String,
    pub project_id: String,
    pub endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteAgentUsageWindow {
    pub label: String,
    pub used_fraction: f32,
    #[serde(rename(serialize = "resetsAt", deserialize = "resetAt"))]
    pub reset_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AgentAccountStatus {
    Connected,
    Disabled,
    AttentionRequired,
    Revoked,
    #[serde(other)]
    Unknown,
}

pub(crate) struct AntigravityModelCatalog {
    pub(crate) models: Vec<zeron_proto::Model>,
    pub(crate) default_model_id: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteAgentAccount {
    pub id: String,
    pub provider: String,
    pub provider_account_id: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization: Option<String>,
    #[serde(rename(serialize = "planLabel", deserialize = "plan"))]
    pub plan: Option<String>,
    pub status: AgentAccountStatus,
    #[serde(default)]
    pub usage_windows: Vec<RemoteAgentUsageWindow>,
}

#[derive(Deserialize)]
struct RemoteAgentAccountsEnvelope {
    accounts: Vec<RemoteAgentAccount>,
}

#[derive(Deserialize)]
struct RemoteAgentAccountEnvelope {
    account: RemoteAgentAccount,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AgentInferenceAuthority {
    pub contract_version: u8,
    pub token: String,
    pub token_type: String,
    pub authority_id: String,
    pub principal_id: String,
    pub authority_scope: Option<String>,
    pub expires_at: String,
}

pub(crate) struct AgentInferenceProxyRequest<'a> {
    pub harness: Option<&'a str>,
    pub bound_model: Option<&'a str>,
    pub endpoint: &'a str,
    pub query: Option<&'a str>,
    pub authority: &'a AgentInferenceAuthority,
    pub conversation_id: &'a str,
    pub requested_account_id: Option<&'a str>,
    pub request_id: &'a str,
    pub headers: reqwest::header::HeaderMap,
    pub content_length: u64,
    pub body: reqwest::Body,
    pub cancellation: &'a CancellationToken,
}

impl fmt::Debug for ScaffoldClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScaffoldClient")
            .field("origin", &self.origin)
            .field("project_scope", &self.project_scope)
            .field("bearer", &"<provider>")
            .finish()
    }
}

impl ScaffoldClient {
    pub fn new(
        origin: impl AsRef<str>,
        project_scope: impl Into<String>,
        bearer: Arc<dyn TokenSource>,
    ) -> Result<Self, ScaffoldError> {
        let origin = exact_origin(origin.as_ref())?;
        let project_scope = project_scope.into();
        if project_scope.trim().is_empty() {
            return Err(ScaffoldError::InvalidScope("projectId is required".into()));
        }
        let http = scaffold_http_client(Some(REQUEST_TIMEOUT))?;
        let inference_http = scaffold_http_client(None)?;
        Ok(Self {
            http,
            inference_http,
            origin,
            project_scope,
            bearer,
        })
    }

    pub fn project_scope(&self) -> &str {
        &self.project_scope
    }

    pub(crate) async fn credential(&self) -> Result<AccessToken, ScaffoldError> {
        control_credential(self.bearer.as_ref()).await
    }

    pub async fn search_crew_sessions(&self, input: &Value) -> Result<Value, ScaffoldError> {
        self.crew_directory_request("search", input).await
    }

    pub async fn get_crew_session(&self, session_id: &str, deployment_id: Option<&str>) -> Result<Value, ScaffoldError> {
        self.crew_directory_request("get", &serde_json::json!({"sessionId": session_id, "deploymentId": deployment_id})).await
    }

    pub async fn upsert_crew_session(&self, input: &Value) -> Result<Value, ScaffoldError> {
        self.crew_directory_request("upsert", input).await
    }

    async fn crew_directory_request(&self, operation: &str, input: &Value) -> Result<Value, ScaffoldError> {
        if operation != "search" && !input.get("sessionId").and_then(Value::as_str).is_some_and(zeron_proto::is_cloud_session_id) {
            return Err(ScaffoldError::InvalidResponse("Cloud directory requires a canonical session UUID".into()));
        }
        let body = serde_json::to_vec(input).map_err(|_| ScaffoldError::InvalidResponse("invalid directory request".into()))?;
        if body.len() > 60 * 1024 {
            return Err(ScaffoldError::InvalidResponse("directory request exceeds 60 KiB".into()));
        }
        let request = self.http.post(self.origin.join(&format!("/api/crew-directory/{operation}")).expect("directory API path"))
            .header(reqwest::header::CONTENT_TYPE, "application/json").body(body);
        let (response, generation) = send_authenticated(self.bearer.as_ref(), request, &CancellationToken::new()).await?;
        self.bounded_json(response, generation).await
    }

    async fn bounded_json(&self, mut response: reqwest::Response, generation: u64) -> Result<Value, ScaffoldError> {
        let status = response.status();
        let retry_after = response_retry_after(&response);
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if body.len() + chunk.len() > 128 * 1024 {
                return Err(ScaffoldError::InvalidResponse("response exceeds 128 KiB".into()));
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(authenticated_error(self.bearer.as_ref(), generation, api_error(status, &body, retry_after)));
        }
        serde_json::from_slice(&body).map_err(|_| ScaffoldError::InvalidResponse("invalid JSON response".into()))
    }

    pub(crate) async fn generate_crew_title(&self, session_id: &str, deployment_id: Option<&str>, input: &str) -> Result<String, ScaffoldError> {
        // The authenticated gateway fixes gpt-5.6-luna and a session-isolated
        // authority/conversation; device grants cannot obtain general inference.
        let value = self.crew_directory_request("title", &serde_json::json!({
            "sessionId": session_id, "deploymentId": deployment_id, "input": input,
        })).await?;
        let title = value.get("title").and_then(Value::as_str).unwrap_or_default();
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(140).collect::<String>();
        if title.is_empty() { return Err(ScaffoldError::InvalidResponse("title response is empty".into())); }
        Ok(title)
    }

    pub(crate) async fn list_agent_accounts(
        &self,
    ) -> Result<Vec<RemoteAgentAccount>, ScaffoldError> {
        let url = self
            .origin
            .join("/api/agent-accounts")
            .expect("static account list path");
        let response: RemoteAgentAccountsEnvelope = self
            .request(
                Method::GET,
                url,
                Option::<&()>::None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        Ok(response.accounts)
    }

    pub(crate) async fn list_antigravity_models(&self, account_id: Option<&str>) -> Result<AntigravityModelCatalog, ScaffoldError> {
        const MAX_BYTES: usize = 256 * 1024;
        let mut url = self.origin.join("/api/agent-accounts/models?provider=antigravity").expect("static models path");
        if let Some(account_id) = account_id { url.query_pairs_mut().append_pair("accountId", account_id); }
        let cancellation = CancellationToken::new();
        let request = self.http.get(url).header(reqwest::header::ACCEPT, "application/json").timeout(REQUEST_TIMEOUT);
        let (mut response, generation) = send_authenticated(self.bearer.as_ref(), request, &cancellation).await?;
        let status = response.status();
        let retry_after = response_retry_after(&response);
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if chunk.len() > MAX_BYTES - bytes.len() {
                return Err(ScaffoldError::InvalidResponse("Google model catalog exceeded 256 KiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(authenticated_error(self.bearer.as_ref(), generation, api_error(status, &bytes, retry_after)));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Catalog { source_account_id: String, models: Vec<CatalogModel>, default_model_id: Option<String> }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct CatalogModel { id: String, display_name: String, context_window: u64 }
        let mut catalog: Catalog = serde_json::from_slice(&bytes)
            .map_err(|_| ScaffoldError::InvalidResponse("Google model catalog is invalid".into()))?;
        if catalog.source_account_id.is_empty()
            || account_id.is_some_and(|id| id != catalog.source_account_id)
            || catalog.models.len() > 128
            || catalog.default_model_id.as_ref().is_some_and(|id| !catalog.models.iter().any(|model| &model.id == id))
            || catalog.models.iter().any(|model|
            !model.id.starts_with("gemini-") || model.id.len() > 128
            || !model.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            || model.display_name.trim().is_empty() || model.display_name.len() > 256
            || model.context_window == 0 || model.context_window > 10_000_000) {
            return Err(ScaffoldError::InvalidResponse("Google model catalog has invalid models".into()));
        }
        catalog.models.sort_by_key(|model| Some(&model.id) != catalog.default_model_id.as_ref());
        Ok(AntigravityModelCatalog { default_model_id: catalog.default_model_id, models: catalog.models.into_iter().map(|model| zeron_proto::Model {
            id: model.id, label: model.display_name, description: Some(format!("{} token context", model.context_window)),
            reasoning_levels: Vec::new(), options: Vec::new(),
        }).collect() })
    }

    pub(crate) async fn import_agent_account(
        &self,
        account: &AgentAccountCredentialImport,
    ) -> Result<RemoteAgentAccount, ScaffoldError> {
        let url = self
            .origin
            .join("/api/agent-accounts/import")
            .expect("static account import path");
        let response: RemoteAgentAccountEnvelope = self
            .request(
                Method::POST,
                url,
                Some(account),
                &CancellationToken::new(),
                None,
            )
            .await?;
        Ok(response.account)
    }

    pub(crate) async fn revoke_agent_account(&self, account_id: &str) -> Result<(), ScaffoldError> {
        let mut url = self
            .origin
            .join("/api/agent-accounts/")
            .expect("static account revoke path");
        url.path_segments_mut()
            .expect("Scaffold origin is hierarchical")
            .pop_if_empty()
            .push(account_id);
        let _: Value = self
            .request(
                Method::DELETE,
                url,
                Option::<&()>::None,
                &CancellationToken::new(),
                None,
            )
            .await?;
        Ok(())
    }

    pub(crate) async fn issue_agent_inference_authority(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<AgentInferenceAuthority, ScaffoldError> {
        let url = self
            .origin
            .join("/api/agent-auth/v2/authority")
            .expect("static Agent Auth authority path");
        let body = serde_json::json!({});
        self.request(Method::POST, url, Some(&body), cancellation, None)
            .await
    }

    pub(crate) async fn proxy_agent_inference(
        &self,
        proxy: AgentInferenceProxyRequest<'_>,
    ) -> Result<reqwest::Response, ScaffoldError> {
        let AgentInferenceProxyRequest {
            harness,
            bound_model,
            endpoint,
            query,
            authority,
            conversation_id,
            requested_account_id,
            request_id,
            mut headers,
            content_length,
            body,
            cancellation,
        } = proxy;
        let path = match endpoint {
            "responses" => "/api/agent-auth/v2/responses",
            "messages" => "/api/agent-auth/v2/messages",
            "antigravity/generateContent" => "/api/agent-auth/v2/antigravity/generateContent",
            "antigravity/streamGenerateContent" => "/api/agent-auth/v2/antigravity/streamGenerateContent",
            _ => {
                return Err(ScaffoldError::InvalidResponse(
                    "unsupported inference endpoint".into(),
                ));
            }
        };
        for name in [
            reqwest::header::AUTHORIZATION,
            reqwest::header::HOST,
            reqwest::header::CONTENT_LENGTH,
            reqwest::header::CONNECTION,
        ] {
            headers.remove(name);
        }
        for name in [
            "x-agent-auth-owner-subject",
            "x-agent-auth-session-id",
            "x-api-key",
            "x-goog-api-key",
            "x-agent-auth-provider",
            "x-agent-auth-model",
            "x-agent-auth-harness",
            "x-agent-auth-source",
            "x-agent-auth-environment",
            "x-agent-auth-lifecycle-epoch",
            "x-agent-auth-request-id",
            "x-agent-auth-authority-scope",
            "x-agent-auth-routing-mode",
            "x-agent-auth-requested-account-id",
            "x-agent-auth-account-id",
            "x-agent-auth-conversation-id",
            "x-agent-auth-internal-secret",
        ] {
            headers.remove(name);
        }
        let mut url = self.origin.join(path).expect("static inference proxy path");
        url.set_query(query);
        let mut request = self
            .inference_http
            .post(url)
            .headers(headers)
            .bearer_auth(&authority.token)
            .header(reqwest::header::CONTENT_LENGTH, content_length)
            .header("x-agent-auth-request-id", request_id)
            .header("x-agent-auth-conversation-id", conversation_id);
        if endpoint.starts_with("antigravity/") { request = request.header("x-agent-auth-harness", harness.unwrap_or("antigravity"));
            if let Some(model) = bound_model { request = request.header("x-agent-auth-model", model); } }
        if let Some(authority_scope) = authority.authority_scope.as_deref() {
            request = request.header("x-agent-auth-authority-scope", authority_scope);
        }
        if let Some(account_id) = requested_account_id {
            request = request.header("x-agent-auth-account-id", account_id);
        }
        let request = request.body(body);
        tokio::select! {
            response = request.send() => response.map_err(ScaffoldError::from),
            () = cancellation.cancelled() => Err(ScaffoldError::Cancelled),
        }
    }

    pub(crate) async fn get_agent_route_receipt(
        &self,
        logical_session_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<AgentRouteReceipt, ScaffoldError> {
        if logical_session_id.trim().is_empty() {
            return Err(ScaffoldError::InvalidScope(
                "logicalSessionId is required".into(),
            ));
        }
        let mut url = self
            .origin
            .join("/api/agent-auth/routes/")
            .expect("static Agent Auth route receipt path");
        url.path_segments_mut()
            .expect("Scaffold origin is hierarchical")
            .pop_if_empty()
            .push(logical_session_id);
        self.request(Method::GET, url, Option::<&()>::None, cancellation, None)
            .await
    }

    pub async fn list(
        &self,
        scope: &CollaborationScope,
        cancellation: &CancellationToken,
    ) -> Result<Vec<SessionEnvironment>, ScaffoldError> {
        let mut validation_scope = scope.clone();
        validation_scope.session_id.get_or_insert_with(|| "list".into());
        self.validate_scope(&validation_scope)?;
        let response: SandboxListEnvelope = self
            .request(
                Method::GET,
                self.code_sandboxes_url(),
                Option::<&()>::None,
                cancellation,
                None,
            )
            .await?;
        if !response.ok {
            return Err(ScaffoldError::InvalidResponse(
                "list response reported ok=false without an HTTP error".into(),
            ));
        }
        response
            .sandboxes
            .into_iter()
            .filter(|sandbox| sandbox.comet_runtime_profile.as_ref().is_some_and(|profile| {
                profile.project_id == scope.project_id
                    && Some(profile.deployment_id.as_str()) == scope.deployment_id.as_deref()
                    && scope.session_id.as_deref().is_none_or(|id| id == profile.session_id)
            }))
            .map(|sandbox| {
                let mut scope = scope.clone();
                scope.session_id = sandbox.comet_runtime_profile.as_ref().map(|profile| profile.session_id.clone());
                sandbox.into_environment(scope)
            })
            .collect()
    }

    pub async fn inspect(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        self.validate_scope(scope)?;
        let response: SandboxEnvelope = self
            .request(
                Method::GET,
                self.sandbox_url(sandbox_id, None)?,
                Option::<&()>::None,
                cancellation,
                None,
            )
            .await?;
        response.sandbox.into_environment(scope.clone())
    }

    pub(crate) async fn create(
        &self,
        scope: &CollaborationScope,
        options: CreateSandboxOptions<'_>,
        agent_route: &AgentRoute,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        let CreateSandboxOptions {
            name,
            source_ref,
            region,
            database_environment,
        } = options;
        self.validate_scope(scope)?;
        agent_route
            .validate()
            .map_err(|message| ScaffoldError::InvalidScope(message.into()))?;
        let deployment_id = scope
            .deployment_id
            .as_deref()
            .expect("validated deploymentId");
        let body = CreateSandboxBody {
            name,
            source: source_ref.map(|reference| CreateSandboxSource { reference }),
            region,
            database_environment,
            agent_route,
            comet_runtime_profile: CreateCometRuntimeProfile {
                version: SCAFFOLD_AUTHORITY_PROFILE_WIRE_VERSION,
                project_id: &scope.project_id,
                deployment_id,
                session_id: scope.session_id.as_deref().expect("validated sessionId"),
            },
        };
        let response: SandboxEnvelope = self
            .request(
                Method::POST,
                self.code_sandboxes_url(),
                Some(&body),
                cancellation,
                None,
            )
            .await?;
        response.sandbox.into_environment(scope.clone())
    }

    pub(crate) async fn original_epoch_history(
        &self, sandbox_id: &str, scope: &CollaborationScope, cancellation: &CancellationToken,
    ) -> Result<Option<OriginalEpochBootstrap>, ScaffoldError> {
        self.validate_scope(scope)?;
        // ponytail: bounded bootstrap, not pagination; larger histories need a streaming import protocol.
        const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
        const MAX_OUTCOMES: usize = 10_000;
        let request = self.http.get(self.sandbox_url(sandbox_id, Some("original-epoch-history"))?)
            .header(reqwest::header::ACCEPT, "application/json")
            // Reqwest's total deadline covers headers and all decoded body chunks, not each chunk.
            .timeout(REQUEST_TIMEOUT);
        let (mut response, generation) = send_authenticated(self.bearer.as_ref(), request, cancellation).await?;
        let status = response.status();
        let retry_after = response_retry_after(&response);
        // Content-Length is only an allocation hint; decoded bytes are authoritative.
        let mut bytes = Vec::with_capacity(response.content_length().unwrap_or(0).min(8192) as usize);
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
                chunk = response.chunk() => chunk?,
            };
            let Some(chunk) = chunk else { break; };
            if chunk.len() > MAX_BODY_BYTES - bytes.len() {
                return Err(ScaffoldError::InvalidResponse("original epoch bootstrap response exceeds 4 MiB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            return Err(authenticated_error(self.bearer.as_ref(), generation, api_error(status, &bytes, retry_after)));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Response { original_epoch_history: serde_json::Value }
        let response: Response = serde_json::from_slice(&bytes)
            .map_err(|error| ScaffoldError::InvalidResponse(error.to_string()))?;
        if response.original_epoch_history.is_null() { return Ok(None); }
        let evidence: OriginalEpochBootstrap = serde_json::from_value(response.original_epoch_history)
            .map_err(|_| ScaffoldError::InvalidResponse("invalid original epoch bootstrap evidence".into()))?;
        // Reject the entire import before callers can persist a ledger or any command outcome.
        if evidence.command_outcomes.len() > MAX_OUTCOMES {
            return Err(ScaffoldError::InvalidResponse("original epoch bootstrap exceeds 10000 command outcomes".into()));
        }
        Ok(Some(evidence))
    }

    pub async fn pause(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        self.lifecycle(sandbox_id, "pause", scope, cancellation)
            .await
    }

    pub async fn resume(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        self.lifecycle(sandbox_id, "resume", scope, cancellation)
            .await
    }

    pub async fn start(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        expected_lifecycle_epoch: u64,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        self.validate_scope(scope)?;
        let response: StartEnvelope = self.request(
            Method::POST, self.sandbox_url(sandbox_id, Some("start"))?,
            Some(&serde_json::json!({"expectedLifecycleEpoch": expected_lifecycle_epoch})),
            cancellation, Some(LIFECYCLE_REQUEST_TIMEOUT),
        ).await?;
        let transition = response.lifecycle_transition;
        let mut environment = response.sandbox.into_environment(scope.clone())?;
        if transition.action != "start" || transition.previous_lifecycle_epoch != expected_lifecycle_epoch
            || expected_lifecycle_epoch.checked_add(1) != Some(transition.current_lifecycle_epoch)
            || scaffold_source(&environment)?.1 != Some(transition.current_lifecycle_epoch)
            || transition.sandbox_id != sandbox_id || environment_sandbox_id(&environment)? != sandbox_id
            || Some(transition.logical_session_id.as_str()) != scope.session_id.as_deref()
            || transition.project_id != scope.project_id
            || Some(transition.deployment_id.as_str()) != scope.deployment_id.as_deref()
            || transition.owner_subject.is_empty() {
            return Err(ScaffoldError::InvalidResponse("Start response lacks the exact authoritative lifecycle transition".into()));
        }
        environment.unknown.insert("startTransition".into(), serde_json::to_value(transition)
            .map_err(|_| ScaffoldError::InvalidResponse("Invalid Start transition".into()))?);
        Ok(environment)
    }

    pub async fn stop(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        self.lifecycle(sandbox_id, "stop", scope, cancellation)
            .await
    }
    pub async fn update_agent_route(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        agent_route: &AgentRoute,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        self.validate_scope(scope)?;
        let response: SandboxEnvelope = self
            .request(
                Method::POST,
                self.sandbox_url(sandbox_id, Some("agent-route"))?,
                Some(&AgentRouteBody { agent_route }),
                cancellation,
                None,
            )
            .await?;
        response.sandbox.into_environment(scope.clone())
    }

    async fn lifecycle(
        &self,
        sandbox_id: &str,
        action: &str,
        scope: &CollaborationScope,
        cancellation: &CancellationToken,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        self.validate_scope(scope)?;
        let response: SandboxEnvelope = self
            .request(
                Method::POST,
                self.sandbox_url(sandbox_id, Some(action))?,
                Some(&EmptyBody {}),
                cancellation,
                Some(LIFECYCLE_REQUEST_TIMEOUT),
            )
            .await?;
        response.sandbox.into_environment(scope.clone())
    }

    async fn clear_handoff_staging(
        &self,
        sandbox_id: &str,
        path: &'static str,
        cancellation: &CancellationToken,
    ) -> Result<(), ScaffoldError> {
        if !matches!(
            path,
            ".scaffold/omp-handoff-staging" | ".scaffold/crew-handoff-staging"
        ) {
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        let argv = vec![
            "rm".to_string(),
            "-rf".to_string(),
            "--".to_string(),
            path.to_string(),
        ];
        let cleared = self
            .exec(
                sandbox_id,
                &ExecBody {
                    argv: &argv,
                    mode: "inline",
                    timeout_ms: 10_000,
                },
                cancellation,
            )
            .await?;
        if !cleared.ok || cleared.exit_code != Some(0) {
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        Ok(())
    }

    async fn require_absent_omp_session(
        &self,
        sandbox_id: &str,
        artifact: &OmpHandoffArchive,
        remote_cwd: &str,
        cancellation: &CancellationToken,
    ) -> Result<(), ScaffoldError> {
        let verify_path = format!(".scaffold/omp-probe-{}.py", uuid::Uuid::new_v4().simple());
        self.put_file(
            sandbox_id,
            &verify_path,
            VERIFY_HANDOFF_PYTHON,
            cancellation,
        )
        .await?;
        let argv = vec![
            "python3".to_string(),
            verify_path.clone(),
            "--probe".to_string(),
            artifact.storage_relative_path.clone(),
            artifact.sha256.clone(),
            artifact.byte_count.to_string(),
            artifact.native_session_id.clone(),
            artifact.cwd.clone(),
            remote_cwd.to_string(),
        ];
        let result = self
            .exec(
                sandbox_id,
                &ExecBody {
                    argv: &argv,
                    mode: "inline",
                    timeout_ms: 10_000,
                },
                cancellation,
            )
            .await;
        self.remove_file(sandbox_id, &verify_path, &CancellationToken::new())
            .await;
        let result = result?;
        if !result.ok || result.exit_code != Some(0) {
            return Err(ScaffoldError::HandoffStage("omp_session_probe"));
        }
        match result.stdout.as_deref().map(str::trim) {
            Some("absent") => Ok(()),
            Some("present") => Err(ScaffoldError::HandoffStage("omp_session_already_imported")),
            _ => Err(ScaffoldError::HandoffStage("omp_session_probe")),
        }
    }

    async fn handoff_omp_session(
        &self,
        sandbox_id: &str,
        artifact: &OmpHandoffArchive,
        remote_cwd: &str,
        cancellation: &CancellationToken,
    ) -> Result<String, ScaffoldError> {
        const DESTINATION: &str = ".scaffold/omp-handoff-staging";
        self.clear_handoff_staging(sandbox_id, DESTINATION, cancellation)
            .await?;
        let grant: UploadGrantEnvelope = self
            .request(
                Method::POST,
                self.sandbox_url(sandbox_id, Some("uploads"))?,
                Some(&UploadGrantRequest {
                    destination_path: DESTINATION,
                }),
                cancellation,
                None,
            )
            .await?;
        if !grant.ok {
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        let grant = grant.upload;
        validate_upload_grant(&grant)?;
        if grant.destination_path != DESTINATION {
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        let file = artifact
            .reopen()
            .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
        self.upload_granted_file(&grant, file, artifact.archive_byte_count, cancellation)
            .await?;

        // Do not start OMP here: that would create a second writer. The
        // ordinary first remote RunRequest resumes the returned native id after
        // the verified transcript has been materialized in the active profile.
        let verify_path = format!(".scaffold/omp-import-{}.py", uuid::Uuid::new_v4().simple());
        self.put_file(
            sandbox_id,
            &verify_path,
            VERIFY_HANDOFF_PYTHON,
            cancellation,
        )
        .await?;
        let verify_argv = vec![
            "python3".to_string(),
            verify_path.clone(),
            grant.destination_path.clone(),
            artifact.storage_relative_path.clone(),
            artifact.sha256.clone(),
            artifact.byte_count.to_string(),
            artifact.native_session_id.clone(),
            artifact.cwd.clone(),
            remote_cwd.to_string(),
        ];
        let verified = self
            .exec(
                sandbox_id,
                &ExecBody {
                    argv: &verify_argv,
                    mode: "inline",
                    timeout_ms: 30_000,
                },
                cancellation,
            )
            .await;
        self.remove_file(sandbox_id, &verify_path, &CancellationToken::new())
            .await;
        let verified = verified?;
        if !verified.ok
            || verified.exit_code != Some(0)
            || verified.stdout.as_deref().map(str::trim) != Some("verified")
        {
            return Err(ScaffoldError::HandoffStage("omp_session_import"));
        }
        Ok(remote_cwd.to_string())
    }

    async fn exec(
        &self,
        sandbox_id: &str,
        body: &ExecBody<'_>,
        cancellation: &CancellationToken,
    ) -> Result<ExecResponse, ScaffoldError> {
        self.request(
            Method::POST,
            self.sandbox_url(sandbox_id, Some("exec"))?,
            Some(body),
            cancellation,
            None,
        )
        .await
    }

    async fn put_file(
        &self,
        sandbox_id: &str,
        path: &str,
        content: &str,
        cancellation: &CancellationToken,
    ) -> Result<(), ScaffoldError> {
        let mut url = self.sandbox_url(sandbox_id, Some("files"))?;
        url.query_pairs_mut().append_pair("path", path);
        let _: Value = self
            .request(
                Method::PUT,
                url,
                Some(&serde_json::json!({ "content": content })),
                cancellation,
                None,
            )
            .await?;
        Ok(())
    }

    async fn remove_file(&self, sandbox_id: &str, path: &str, cancellation: &CancellationToken) {
        let argv = vec![
            "rm".to_string(),
            "-f".to_string(),
            "--".to_string(),
            path.to_string(),
        ];
        let _ = self
            .exec(
                sandbox_id,
                &ExecBody {
                    argv: &argv,
                    mode: "inline",
                    timeout_ms: 10_000,
                },
                cancellation,
            )
            .await;
    }

    async fn request<T, B>(
        &self,
        method: Method,
        url: Url,
        body: Option<&B>,
        cancellation: &CancellationToken,
        timeout: Option<Duration>,
    ) -> Result<T, ScaffoldError>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        let mut request = self
            .http
            .request(method, url)
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let (response, generation) = send_authenticated(self.bearer.as_ref(), request, cancellation).await?;
        let status = response.status();
        let retry_after = response_retry_after(&response);
        let bytes = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
            bytes = response.bytes() => bytes?,
        };
        if !status.is_success() {
            return Err(authenticated_error(self.bearer.as_ref(), generation, api_error(status, &bytes, retry_after)));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| ScaffoldError::InvalidResponse(error.to_string()))
    }

    fn validate_scope(&self, scope: &CollaborationScope) -> Result<(), ScaffoldError> {
        if scope.project_id != self.project_scope {
            return Err(ScaffoldError::InvalidScope(format!(
                "projectId must equal configured Scaffold scope {}",
                self.project_scope
            )));
        }
        if scope
            .deployment_id
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ScaffoldError::InvalidScope(
                "deploymentId is required".into(),
            ));
        }
        if scope
            .session_id
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(ScaffoldError::InvalidScope("sessionId is required".into()));
        }
        Ok(())
    }

    fn code_sandboxes_url(&self) -> Url {
        let mut url = self.origin.clone();
        url.path_segments_mut()
            .expect("validated HTTP origin")
            .extend(["api", "code-sandboxes"]);
        url
    }

    fn sandbox_url(&self, sandbox_id: &str, action: Option<&str>) -> Result<Url, ScaffoldError> {
        if sandbox_id.trim().is_empty() {
            return Err(ScaffoldError::InvalidResponse(
                "sandboxId is required".into(),
            ));
        }
        let mut url = self.code_sandboxes_url();
        let mut segments = url.path_segments_mut().expect("validated HTTP origin");
        segments.push(sandbox_id);
        if let Some(action) = action {
            segments.push(action);
        }
        drop(segments);
        Ok(url)
    }

    async fn handoff_worktree(
        &self,
        sandbox_id: &str,
        snapshot: &WorktreeHandoffArchive,
        remote_cwd: String,
        cancellation: &CancellationToken,
    ) -> Result<String, ScaffoldError> {
        const DESTINATION: &str = ".scaffold/crew-handoff-staging";
        self.clear_handoff_staging(sandbox_id, DESTINATION, cancellation)
            .await?;
        let grant: UploadGrantEnvelope = self
            .request(
                Method::POST,
                self.sandbox_url(sandbox_id, Some("uploads"))?,
                Some(&UploadGrantRequest {
                    destination_path: DESTINATION,
                }),
                cancellation,
                None,
            )
            .await?;
        if !grant.ok {
            tracing::warn!("Scaffold worktree upload grant denied");
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        let grant = grant.upload;
        validate_upload_grant(&grant).inspect_err(|error| {
            tracing::warn!(error = %error, "Scaffold worktree upload grant invalid");
        })?;
        if grant.destination_path != DESTINATION {
            tracing::warn!("Scaffold worktree upload grant destination mismatched");
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        let file = snapshot.reopen().map_err(|error| {
            tracing::warn!(error = %error, "Scaffold worktree handoff archive reopen failed");
            ScaffoldError::OmpSessionHandoffFailed
        })?;
        self.upload_granted_file(&grant, file, snapshot.byte_count, cancellation)
            .await
            .inspect_err(|error| {
                tracing::warn!(error = %error, "Scaffold worktree handoff archive upload failed");
            })?;

        let verify_path = format!(".scaffold/crew-handoff-verify-{}.py", crate::new_id());
        self.put_file(
            sandbox_id,
            &verify_path,
            VERIFY_WORKTREE_HANDOFF_PYTHON,
            cancellation,
        )
        .await?;
        let verify_argv = vec![
            "python3".to_string(),
            verify_path.clone(),
            grant.destination_path.clone(),
            snapshot.manifest_sha256.clone(),
            snapshot.base_sha.clone(),
            snapshot.entry_count.to_string(),
            serde_json::to_string(&snapshot.cwd_relative_path)
                .map_err(|_| ScaffoldError::HandoffStage("source_repository_cwd"))?,
        ];
        let verified = self
            .exec(
                sandbox_id,
                &ExecBody {
                    argv: &verify_argv,
                    mode: "inline",
                    timeout_ms: 60_000,
                },
                cancellation,
            )
            .await;
        self.remove_file(sandbox_id, &verify_path, cancellation)
            .await;
        let verified = verified?;
        let expected = format!("verified:{}:{remote_cwd}", snapshot.manifest_sha256);
        if !verified.ok
            || verified.exit_code != Some(0)
            || verified.stdout.as_deref().map(str::trim) != Some(expected.as_str())
        {
            let stderr = verified.stderr.as_deref().unwrap_or("");
            let mut stderr_end = stderr.len().min(2048);
            while !stderr.is_char_boundary(stderr_end) {
                stderr_end -= 1;
            }
            tracing::warn!(
                ok = verified.ok,
                exit_code = ?verified.exit_code,
                stderr = &stderr[..stderr_end],
                sandbox_id,
                manifest_sha256 = %snapshot.manifest_sha256,
                base_sha = %snapshot.base_sha,
                archive_bytes = snapshot.byte_count,
                bundle_sha256 = %snapshot.repository.sha256,
                bundle_bytes = snapshot.repository.byte_count,
                prerequisite_sha = ?snapshot.repository.prerequisite_sha,
                shallow = ?snapshot.repository.shallow,
                "Scaffold worktree handoff verification failed"
            );
            let stage = match verified.exit_code {
                Some(40 | 41 | 42) => "source_import_manifest",
                Some(43 | 44) => "source_import_archive",
                Some(45 | 47) => "source_import_repository",
                Some(46 | 48) => "source_import_overlay",
                Some(49) => "source_import_destination",
                _ => "source_repository_import",
            };
            return Err(ScaffoldError::RepositoryImport { stage, diagnostic: Box::new(RepositoryImportDiagnostic {
                sandbox_id: sandbox_id.into(), exit_code: verified.exit_code, stderr: stderr[..stderr_end].into(),
                manifest_sha256: snapshot.manifest_sha256.clone(), base_sha: snapshot.base_sha.clone(), archive_bytes: snapshot.byte_count,
                bundle_sha256: snapshot.repository.sha256.clone(), bundle_bytes: snapshot.repository.byte_count,
                prerequisite_sha: snapshot.repository.prerequisite_sha.clone(), shallow: snapshot.repository.shallow,
            }) });
        }
        Ok(remote_cwd)
    }

    async fn upload_granted_file(
        &self,
        grant: &UploadGrant,
        file: std::fs::File,
        byte_count: u64,
        cancellation: &CancellationToken,
    ) -> Result<(), ScaffoldError> {
        if byte_count == 0 || byte_count > crate::worktree_handoff::MAX_HANDOFF_ARCHIVE_BYTES {
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        let url = safe_upload_url(&grant.url)?;
        let token = grant.token.clone();
        let file = tokio::fs::File::from_std(file);
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let send = self
            .http
            .post(url)
            .timeout(Duration::from_secs(5 * 60))
            .bearer_auth(&token)
            .header(reqwest::header::CONTENT_TYPE, "application/x-tar")
            .header(reqwest::header::CONTENT_LENGTH, byte_count)
            .body(body)
            .send();
        tokio::pin!(send);
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
            response = &mut send => response?,
        };
        drop(token);
        if !response.status().is_success() {
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        Ok(())
    }
}

fn prepare_omp_handoff_archive(
    artifact: CapturedOmpSessionFile,
    cancellation: &CancellationToken,
) -> Result<OmpHandoffArchive, ScaffoldError> {
    use sha2::{Digest as _, Sha256};

    if artifact.native_session_id.trim().is_empty()
        || artifact.cwd.trim().is_empty()
        || artifact.byte_count > crate::omp_session_artifact::MAX_OMP_SESSION_ARTIFACT_BYTES
        || artifact.sha256.len() != 64
        || !artifact
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }
    let _ = safe_archive_path(&artifact.storage_relative_path)?;
    let (name, prefix) = split_ustar_path(&artifact.storage_relative_path)?;
    let mut header = [0_u8; 512];
    put_tar_text(&mut header[0..100], name)?;
    put_tar_octal(&mut header[100..108], 0o600)?;
    put_tar_octal(&mut header[108..116], 0)?;
    put_tar_octal(&mut header[116..124], 0)?;
    put_tar_octal(&mut header[124..136], artifact.byte_count)?;
    put_tar_octal(&mut header[136..148], 0)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    put_tar_text(&mut header[345..500], prefix)?;
    let checksum: u64 = header.iter().map(|byte| u64::from(*byte)).sum();
    let checksum_field = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(checksum_field.as_bytes());

    let mut source = artifact
        .reopen()
        .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
    let mut archive = NamedTempFile::new().map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
    archive
        .as_file_mut()
        .write_all(&header)
        .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancellation.is_cancelled() {
            return Err(ScaffoldError::Cancelled);
        }
        let read = source
            .read(&mut buffer)
            .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
        if read == 0 {
            break;
        }
        copied = copied.saturating_add(read as u64);
        if copied > artifact.byte_count {
            return Err(ScaffoldError::OmpSessionHandoffFailed);
        }
        digest.update(&buffer[..read]);
        archive
            .as_file_mut()
            .write_all(&buffer[..read])
            .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
    }
    if copied != artifact.byte_count || format!("{:x}", digest.finalize()) != artifact.sha256 {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }
    let padding = (512 - copied % 512) % 512;
    if padding > 0 {
        archive
            .as_file_mut()
            .write_all(&[0_u8; 512][..padding as usize])
            .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
    }
    archive
        .as_file_mut()
        .write_all(&[0_u8; 1024])
        .and_then(|_| archive.as_file_mut().flush())
        .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
    let archive_byte_count = archive
        .as_file()
        .metadata()
        .map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?
        .len();
    if archive_byte_count == 0 || archive_byte_count > MAX_HANDOFF_ARCHIVE_BYTES {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }

    Ok(OmpHandoffArchive {
        file: archive,
        archive_byte_count,
        native_session_id: artifact.native_session_id,
        cwd: artifact.cwd,
        storage_relative_path: artifact.storage_relative_path,
        sha256: artifact.sha256,
        byte_count: artifact.byte_count,
    })
}

fn safe_archive_path(path: &str) -> Result<Vec<&str>, ScaffoldError> {
    let parts: Vec<_> = path.split('/').collect();
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || parts
            .iter()
            .any(|part| part.is_empty() || matches!(*part, "." | ".."))
    {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }
    Ok(parts)
}

fn validate_upload_grant(grant: &UploadGrant) -> Result<(), ScaffoldError> {
    if grant.token.trim().is_empty()
        || grant.token_env != "SCAFFOLD_UPLOAD_TOKEN"
        || grant.destination_path.trim().is_empty()
        || grant.expires_at <= now_ms()
    {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }
    safe_upload_url(&grant.url)?;
    Ok(())
}

fn safe_upload_url(value: &str) -> Result<Url, ScaffoldError> {
    let url = Url::parse(value).map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if url.host_str().is_none()
        || (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }
    Ok(url)
}

fn split_ustar_path(path: &str) -> Result<(&str, &str), ScaffoldError> {
    if path.len() <= 100 {
        return Ok((path, ""));
    }
    for (index, _) in path.match_indices('/').rev() {
        let (prefix, rest) = path.split_at(index);
        let name = &rest[1..];
        if prefix.len() <= 155 && !name.is_empty() && name.len() <= 100 {
            return Ok((name, prefix));
        }
    }
    Err(ScaffoldError::OmpSessionHandoffFailed)
}

fn put_tar_text(field: &mut [u8], value: &str) -> Result<(), ScaffoldError> {
    if value.len() > field.len() || !value.is_ascii() {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }
    field[..value.len()].copy_from_slice(value.as_bytes());
    Ok(())
}

fn put_tar_octal(field: &mut [u8], value: u64) -> Result<(), ScaffoldError> {
    let digits = field.len() - 1;
    let value = format!("{value:0digits$o}");
    if value.len() != digits {
        return Err(ScaffoldError::OmpSessionHandoffFailed);
    }
    field[..digits].copy_from_slice(value.as_bytes());
    Ok(())
}

fn exact_origin(value: &str) -> Result<Url, ScaffoldError> {
    let mut url = Url::parse(value).map_err(|error| {
        ScaffoldError::InvalidResponse(format!("invalid Scaffold origin: {error}"))
    })?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if url.host_str().is_none()
        || (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(ScaffoldError::InvalidResponse(
            "Scaffold URL must be an exact HTTPS origin (HTTP is loopback-only)".into(),
        ));
    }
    url.set_path("");
    Ok(url)
}

fn response_retry_after(response: &reqwest::Response) -> Option<Duration> {
    let value = response.headers().get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse().ok().map(Duration::from_secs);
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some((date.with_timezone(&chrono::Utc) - chrono::Utc::now()).to_std().unwrap_or_default())
}

fn api_error(status: StatusCode, body: &[u8], retry_after: Option<Duration>) -> ScaffoldError {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        error: Value,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        error_description: Option<String>,
        #[serde(default)]
        body: Value,
    }

    let decoded = serde_json::from_slice::<Envelope>(body).ok();
    let error_value = decoded.as_ref().map(|value| &value.error);
    // The provider wrapper is retained on the wire for installed clients.
    // Promote only its exact provisioning code, never free-form messages.
    let runtime_starting = status == StatusCode::SERVICE_UNAVAILABLE
        && error_value.and_then(Value::as_str) == Some("sandbox_provider_error")
        && decoded
            .as_ref()
            .and_then(|value| value.body.get("error"))
            .and_then(Value::as_str)
            == Some("sandbox_runtime_starting");
    let code = runtime_starting
        .then_some("sandbox_runtime_starting")
        .or_else(|| error_value.and_then(Value::as_str))
        .or_else(|| {
            error_value
                .and_then(|value| value.get("error"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            error_value
                .and_then(|value| value.get("code"))
                .and_then(Value::as_str)
        })
        .unwrap_or("scaffold_request_rejected")
        .to_string();
    let nested_message = error_value
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let message = decoded
        .and_then(|value| value.message.or(value.error_description))
        .or(nested_message);
    ScaffoldError::Api(ScaffoldApiErrorDisplay(ScaffoldApiError {
        status: status.as_u16(),
        code,
        message,
        retry_after,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxListEnvelope {
    ok: bool,
    #[serde(default)]
    sandboxes: Vec<ScaffoldSandbox>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SandboxEnvelope {
    sandbox: ScaffoldSandbox,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartEnvelope {
    sandbox: ScaffoldSandbox,
    lifecycle_transition: StartTransition,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartTransition {
    action: String,
    previous_lifecycle_epoch: u64,
    current_lifecycle_epoch: u64,
    logical_session_id: String,
    sandbox_id: String,
    project_id: String,
    deployment_id: String,
    owner_subject: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScaffoldSandbox {
    id: String,
    name: Option<String>,
    lifecycle_epoch: Option<u64>,
    status: zeron_proto::ScaffoldLifecycle,
    kind: Option<String>,
    runtime_profile: Option<String>,
    region: Option<String>,
    selected_region: Option<String>,
    source_ref: Option<String>,
    #[serde(default)]
    database_environment: ScaffoldDatabaseEnvironment,
    owner_email: Option<String>,
    created_at: String,
    updated_at: String,
    last_activity_at: Option<String>,
    #[serde(default)]
    links: ScaffoldEnvironmentLinks,
    comet_runtime_profile: Option<ScaffoldCometRuntimeProfile>,
    agent_route: Option<AgentRoute>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScaffoldCometRuntimeProfile {
    version: String,
    project_id: String,
    deployment_id: String,
    session_id: String,
    sandbox_id: String,
}

impl ScaffoldSandbox {
    fn into_environment(
        self,
        scope: CollaborationScope,
    ) -> Result<SessionEnvironment, ScaffoldError> {
        if self.id.trim().is_empty() {
            return Err(ScaffoldError::InvalidResponse("sandbox id is empty".into()));
        }
        if self.kind.as_deref().is_some_and(|kind| {
            kind != "remote_code"
                && !(kind == "agent" && self.runtime_profile.as_deref() == Some("comet_remote"))
        }) || self
            .runtime_profile
            .as_deref()
            .is_some_and(|profile| !matches!(profile, "remote_code" | "comet_remote"))
        {
            return Err(ScaffoldError::InvalidResponse(format!(
                "{} is not a Crew-compatible remote sandbox",
                self.id
            )));
        }
        let owner_email = self.owner_email.ok_or_else(|| {
            ScaffoldError::InvalidResponse(format!("sandbox {} has no verified owner", self.id))
        })?;
        let owner_email = owner_email.trim();
        if owner_email.is_empty() || !owner_email.contains('@') {
            return Err(ScaffoldError::InvalidResponse(format!(
                "sandbox {} has an invalid verified owner",
                self.id
            )));
        }
        let owner_principal = owner_email.to_string();
        let activity = self.last_activity_at.as_deref().unwrap_or(&self.updated_at);
        let last_activity_at = Some(parse_rfc3339_ms(activity, "lastActivityAt")?);
        // Validate required timestamps even though only last activity enters the projection.
        let _ = parse_rfc3339_ms(&self.created_at, "createdAt")?;
        let _ = parse_rfc3339_ms(&self.updated_at, "updatedAt")?;
        let scope = {
            let profile = self.comet_runtime_profile.as_ref().ok_or_else(|| {
                ScaffoldError::InvalidResponse(format!(
                    "sandbox {} has no authoritative Crew runtime profile",
                    self.id
                ))
            })?;
            if profile.version != SCAFFOLD_AUTHORITY_PROFILE_WIRE_VERSION
                || profile.project_id != scope.project_id
                || Some(profile.deployment_id.as_str()) != scope.deployment_id.as_deref()
                || Some(profile.session_id.as_str()) != scope.session_id.as_deref()
                || profile.sandbox_id != self.id
            {
                return Err(ScaffoldError::InvalidResponse(format!(
                    "sandbox {} returned a mismatched Crew runtime profile",
                    self.id
                )));
            }
            CollaborationScope {
                project_id: profile.project_id.clone(),
                deployment_id: Some(profile.deployment_id.clone()),
                session_id: Some(profile.session_id.clone()),
                unknown: Default::default(),
            }
        };
        let lifecycle_epoch = self.lifecycle_epoch;
        let mut unknown: zeron_proto::UnknownFields = Default::default();
        if let Some(route) = self.agent_route {
            route.validate().map_err(|message| ScaffoldError::InvalidResponse(message.into()))?;
            unknown.insert("agentRoute".into(), serde_json::to_value(route)
                .map_err(|error| ScaffoldError::InvalidResponse(error.to_string()))?);
        }
        Ok(SessionEnvironment {
            source: SessionEnvironmentSource::Scaffold {
                sandbox_id: self.id,
                region: self.selected_region.or(self.region),
                lifecycle: self.status,
                lifecycle_epoch,
                links: Box::new(self.links),
            },
            name: self.name,
            owner_principal,
            scope,
            source_ref: self.source_ref,
            last_activity_at,
            database_environment: Some(self.database_environment),
            unknown,
        })
    }
}

fn parse_rfc3339_ms(value: &str, field: &str) -> Result<i64, ScaffoldError> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|timestamp| timestamp.timestamp_millis())
        .map_err(|error| ScaffoldError::InvalidResponse(format!("invalid {field}: {error}")))
}

#[derive(Clone, Copy, Default)]
pub(crate) struct CreateSandboxOptions<'a> {
    pub name: Option<&'a str>,
    pub source_ref: Option<&'a str>,
    pub region: Option<&'a str>,
    pub database_environment: ScaffoldDatabaseEnvironment,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateSandboxBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<CreateSandboxSource<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<&'a str>,
    database_environment: ScaffoldDatabaseEnvironment,
    agent_route: &'a AgentRoute,
    comet_runtime_profile: CreateCometRuntimeProfile<'a>,
}

#[derive(Debug, Serialize)]
struct CreateSandboxSource<'a> {
    #[serde(rename = "ref")]
    reference: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateCometRuntimeProfile<'a> {
    version: &'static str,
    project_id: &'a str,
    deployment_id: &'a str,
    session_id: &'a str,
}

#[derive(Debug, Serialize)]
struct EmptyBody {}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AgentRouteBody<'a> {
    agent_route: &'a AgentRoute,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExecBody<'a> {
    argv: &'a [String],
    mode: &'static str,
    timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScaffoldHostAuthorityResponse {
    grant_id: String,
    expires_at: i64,
    principal_subject: String,
    scope: CollaborationScope,
    sandbox_id: String,
    device_id: String,
    lifecycle_epoch: u64,
    capabilities: Vec<String>,
}

fn confirmed_host_authority(response: &ExecResponse, environment: &SessionEnvironment)
    -> Option<ScaffoldHostAuthorityResponse> {
    if !response.ok || response.exit_code != Some(0) { return None; }
    let SessionEnvironmentSource::Scaffold { sandbox_id, lifecycle_epoch: Some(epoch), .. } = &environment.source else { return None };
    let existing: ScaffoldHostAuthorityResponse = serde_json::from_str(response.stdout.as_deref()?).ok()?;
    (!existing.grant_id.is_empty() && existing.expires_at > now_ms()
        && existing.principal_subject == environment.owner_principal
        && existing.scope == environment.scope && &existing.sandbox_id == sandbox_id
        && existing.device_id == device_id_for(sandbox_id, *epoch)
        && existing.lifecycle_epoch == *epoch
        && existing.capabilities == scaffold_host_capabilities()).then_some(existing)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExecResponse {
    ok: bool,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    supervised: bool,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    stdout: Option<String>,
    #[serde(default)]
    stderr: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UploadGrantRequest<'a> {
    destination_path: &'a str,
}

#[derive(Debug, Deserialize)]
struct UploadGrantEnvelope {
    ok: bool,
    upload: UploadGrant,
}

fn deserialize_timestamp_millis<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum WireTimestamp {
        Millis(i64),
        Rfc3339(String),
    }

    match WireTimestamp::deserialize(deserializer)? {
        WireTimestamp::Millis(value) => Ok(value),
        WireTimestamp::Rfc3339(value) => chrono::DateTime::parse_from_rfc3339(&value)
            .map(|timestamp| timestamp.timestamp_millis())
            .map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadGrant {
    url: String,
    token: String,
    token_env: String,
    destination_path: String,
    #[serde(deserialize_with = "deserialize_timestamp_millis")]
    expires_at: i64,
    // Intentionally decoded only so schema drift is caught. Never execute or
    // log this shell command because it projects the upload bearer externally.
    #[serde(rename = "command")]
    _command: String,
}

const VERIFY_WORKTREE_HANDOFF_PYTHON: &str = r#"import hashlib, json, os, pathlib, posixpath, resource, shutil, signal, stat, struct, subprocess, sys, tempfile, time
platform = pathlib.Path("/workspace/ashler-platform").resolve(strict=True)
recovery = globals().get("CREW_ISOLATED_RECOVERY")
destination = pathlib.Path("/workspace/crew-handoff")
staging = pathlib.Path(sys.argv[1]).resolve(strict=True)
expected_manifest_sha, expected_base, expected_count = sys.argv[2], sys.argv[3].lower(), int(sys.argv[4])
expected_cwd = json.loads(sys.argv[5])
if recovery is not None:
    namespace = recovery["namespace"]
    if len(namespace) != 64 or any(c not in "0123456789abcdef" for c in namespace): raise SystemExit(40)
    operation = platform / ".scaffold" / ("crew-native-" + namespace)
    if operation != pathlib.Path(recovery["operation_root"]) or operation.resolve(strict=True) != operation or operation not in staging.parents: raise SystemExit(40)
    if stat.S_IMODE(staging.stat().st_mode) != 0o700: raise SystemExit(40)
    destination = platform.parent / ("crew-native-" + namespace)
elif platform not in staging.parents or staging.relative_to(platform).as_posix() != ".scaffold/crew-handoff-staging":
    raise SystemExit(40)
MAX_TOTAL = 256 * 1024 * 1024
MAX_CHECKOUT_BYTES = 1024 * 1024 * 1024
MAX_HISTORY_BYTES = 4 * 1024 * 1024 * 1024
MAX_OBJECTS = 250000
MAX_FILES = 50000
# Bound directory traversal independently of the regular-file/symlink budget.
MAX_DIRECTORIES = 25000
MAX_MEMORY = 512 * 1024 * 1024
deadline = time.monotonic() + 50

def repository_failure(reason):
    print("crew_repository:" + reason, file=sys.stderr)
    raise SystemExit(45)

def sha(value):
    return isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)

if not sha(expected_manifest_sha) or len(expected_base) not in (40, 64) or any(c not in "0123456789abcdef" for c in expected_base) or not 0 <= expected_count <= 25000:
    raise SystemExit(41)

def safe_path(value):
    if not isinstance(value, str) or not value or len(value) > 4096 or "\\" in value or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise SystemExit(43)
    parts = value.split("/")
    if any(part in ("", ".", "..") for part in parts) or parts[0] in (".git", ".scaffold"):
        raise SystemExit(43)
    return pathlib.PurePosixPath(value)

def digest_file(path, limit, contents=False):
    fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | (getattr(os, "O_NONBLOCK", 0) if recovery is not None else 0))
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size > limit:
            raise SystemExit(44)
        digest, count, data = hashlib.sha256(), 0, bytearray()
        while True:
            block = source.read(min(1024 * 1024, limit - count + 1))
            if not block: break
            count += len(block)
            if count > limit: raise SystemExit(44)
            digest.update(block)
            if contents: data.extend(block)
    if count != info.st_size: raise SystemExit(44)
    return digest.hexdigest(), count, stat.S_IMODE(info.st_mode), data

def git(cwd, args, output=False):
    # No inherited repository overrides, configuration, hooks, prompts or filters.
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    environment.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_TERMINAL_PROMPT="0", GIT_NO_REPLACE_OBJECTS="1", ASHLER_INCREMENTAL_TSC_CHECKS="false")
    def limits():
        resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_TOTAL, MAX_TOTAL))
        if sys.platform == "linux":
            resource.setrlimit(resource.RLIMIT_AS, (MAX_MEMORY, MAX_MEMORY))
    with tempfile.TemporaryFile(dir=temporary) as stdout, tempfile.TemporaryFile(dir=temporary) as stderr:
        process = subprocess.Popen(["git", "-c", "core.hooksPath=/dev/null", "-c", "pack.threads=1", "-c", "core.preloadIndex=false", "-c", "index.threads=1", "-C", str(cwd), *args], stdout=stdout if output else subprocess.DEVNULL, stderr=stderr, env=environment, preexec_fn=limits, start_new_session=True, pass_fds=(recovery["lock_fd"],) if recovery is not None else ())
        try:
            if process.wait(timeout=max(0.01, deadline - time.monotonic())) != 0:
                stderr.seek(0)
                print("git " + args[0] + ": " + stderr.read(2048).decode("utf-8", "replace"), file=sys.stderr)
                raise SystemExit(45)
            if output:
                stdout.seek(0)
                result = stdout.read(32 * 1024 * 1024 + 1)
                if len(result) > 32 * 1024 * 1024: repository_failure("git_output_bounds")
                return result
        finally:
            # wait() already reaped completed commands; never signal a stale PGID.
            if process.returncode is None:
                try: os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError: pass
            process.wait()

def remove_path(path):
    try: info = path.lstat()
    except FileNotFoundError: return
    if stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode): shutil.rmtree(path)
    else: path.unlink()

def secure_parent(workspace, rel, create):
    current = workspace
    for part in rel.parts[:-1]:
        current = current / part
        try: info = current.lstat()
        except FileNotFoundError:
            if not create: return False
            current.mkdir(mode=0o755)
            continue
        if not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode): raise SystemExit(46)
    return True

def inspect_bundle(path):
    # Reject excessive object counts before index-pack allocates its object table.
    with path.open("rb") as source:
        header_bytes = 0
        while True:
            line = source.readline(65537)
            header_bytes += len(line)
            if not line or header_bytes > 65536: repository_failure("bundle_header_bounds")
            if line == b"\n": break
        header = source.read(12)
        if len(header) != 12: repository_failure("bundle_pack_header")
        signature, version, count = struct.unpack(">4sII", header)
        if signature != b"PACK" or version not in (2, 3) or not 0 < count <= MAX_OBJECTS:
            repository_failure("bundle_pack_bounds")

def inspect_objects(workspace):
    # Enumerate imported objects only, not the platform's entire object store.
    # Thin-pack bases are copied into the local pack by index-pack.
    alternates = workspace / ".git/objects/info/alternates"
    alternate_bytes = alternates.read_bytes() if alternates.exists() else None
    if alternate_bytes is not None: alternates.unlink()
    try: listing = git(workspace, ["cat-file", "--batch-all-objects", "--batch-check=%(objectsize)"], True)
    finally:
        if alternate_bytes is not None: alternates.write_bytes(alternate_bytes)
    count, expanded = 0, 0
    for line in listing.splitlines():
        size = int(line)
        if size < 0: repository_failure("object_size_invalid")
        count += 1; expanded += size
        if size > MAX_TOTAL or count > MAX_OBJECTS or expanded > MAX_HISTORY_BYTES: repository_failure("object_expansion_bounds")
    listing = git(workspace, ["ls-tree", "-r", "-t", "-l", "-z", expected_base], True)
    count, directories, expanded = 0, 0, 0
    for entry in listing.split(b"\0"):
        if not entry: continue
        metadata, path = entry.split(b"\t", 1)
        mode, kind, oid, size = metadata.split()
        safe_path(path.decode("utf-8"))
        if kind == b"tree" and mode == b"040000":
            directories += 1
            if directories > MAX_DIRECTORIES: repository_failure("tree_directory_bounds")
            continue
        # Gitlinks cannot be reconstructed from the source repository's bundle.
        if kind != b"blob" or mode not in (b"100644", b"100755", b"120000"): repository_failure("tree_entry_unsupported")
        count += 1
        size = int(size)
        expanded += size
        if size < 0 or count > MAX_FILES or expanded > MAX_CHECKOUT_BYTES: repository_failure("tree_expansion_bounds")

def inspect_materialized_tree(workspace):
    count, directories, expanded = 0, 0, 0
    for root, dirs, files in os.walk(workspace, followlinks=False):
        if pathlib.Path(root) == workspace: dirs.remove(".git")
        for name in [*dirs, *files]:
            candidate = pathlib.Path(root) / name
            rel = candidate.relative_to(workspace)
            safe_path(rel.as_posix())
            info = candidate.lstat()
            if stat.S_ISDIR(info.st_mode):
                directories += 1
                if directories > MAX_DIRECTORIES: raise SystemExit(46)
                continue
            count += 1
            if stat.S_ISLNK(info.st_mode):
                target = os.readlink(candidate)
                if not target or target.startswith("/") or "\\" in target or len(target) > 4096 or any(ord(c) < 32 or ord(c) == 127 for c in target): raise SystemExit(46)
                normalized = posixpath.normpath(posixpath.join(rel.parent.as_posix(), target))
                if normalized == ".." or normalized.startswith("../") or normalized.split("/")[0] in (".git", ".scaffold"): raise SystemExit(46)
                # Resolve chains as well as the immediate lexical target. Loops,
                # absolute indirection and links into metadata fail closed.
                try: resolved = candidate.resolve(strict=False)
                except (OSError, RuntimeError): raise SystemExit(46)
                if resolved != workspace and workspace not in resolved.parents: raise SystemExit(46)
                resolved_parts = resolved.relative_to(workspace).parts
                if resolved_parts and resolved_parts[0] in (".git", ".scaffold"): raise SystemExit(46)
                expanded += info.st_size
            elif stat.S_ISREG(info.st_mode): expanded += info.st_size
            else: raise SystemExit(46)
            if count > MAX_FILES or expanded > MAX_CHECKOUT_BYTES: raise SystemExit(46)

temporary = None
published = False
try:
    manifest_sha, total, _, manifest_bytes = digest_file(staging / ".crew-handoff-manifest.json", 16 * 1024 * 1024, True)
    if manifest_sha != expected_manifest_sha: raise SystemExit(42)
    try: manifest = json.loads(manifest_bytes)
    except (UnicodeDecodeError, ValueError): raise SystemExit(42)
    if not isinstance(manifest, dict) or manifest.get("version") != "crew.scaffold.worktree.v2" or manifest.get("baseSha") != expected_base or manifest.get("cwdRelativePath") != expected_cwd:
        raise SystemExit(42)
    cwd_parts = safe_path(expected_cwd).parts if expected_cwd else ()
    entries, repository = manifest.get("entries"), manifest.get("repository")
    if not isinstance(entries, list) or len(entries) != expected_count or not isinstance(repository, dict) or repository.get("path") != "source.bundle" or not sha(repository.get("sha256")) or type(repository.get("byteCount")) is not int or not 0 <= repository["byteCount"] <= MAX_TOTAL:
        raise SystemExit(42)
    prerequisite = repository.get("prerequisiteSha")
    shallow = repository.get("shallow", False)
    if "prerequisiteSha" in repository and (not isinstance(prerequisite, str) or len(prerequisite) != len(expected_base) or any(c not in "0123456789abcdef" for c in prerequisite)):
        raise SystemExit(42)
    if type(shallow) is not bool or (prerequisite is not None and shallow): raise SystemExit(42)
    if repository["byteCount"] == 0 and prerequisite != expected_base: raise SystemExit(42)
    bundle = staging / "source.bundle"
    bundle_sha, bundle_bytes, _, _ = digest_file(bundle, MAX_TOTAL)
    if (bundle_sha, bundle_bytes) != (repository["sha256"], repository["byteCount"]): raise SystemExit(44)
    total += bundle_bytes
    by_path, regular_paths = {}, set()
    for entry in entries:
        if not isinstance(entry, dict): raise SystemExit(43)
        rel = safe_path(entry.get("path"))
        key, kind = rel.as_posix(), entry.get("kind")
        if key in by_path or kind not in ("regular", "symlink", "delete"): raise SystemExit(43)
        by_path[key] = (rel, entry)
        if kind == "regular": regular_paths.add(key)
    paths = set(by_path)
    for key, (rel, entry) in by_path.items():
        if any(pathlib.PurePosixPath(*rel.parts[:i]).as_posix() in paths for i in range(1, len(rel.parts))): raise SystemExit(43)
        if entry["kind"] == "regular":
            if not sha(entry.get("sha256")) or type(entry.get("byteCount")) is not int or not 0 <= entry["byteCount"] <= 64 * 1024 * 1024 or not isinstance(entry.get("executable"), bool): raise SystemExit(44)
        elif entry["kind"] == "symlink":
            target = entry.get("target")
            if not isinstance(target, str) or not target or len(target) > 4096 or target.startswith("/") or "\\" in target or any(ord(c) < 32 or ord(c) == 127 for c in target): raise SystemExit(43)
            normalized = posixpath.normpath(posixpath.join(posixpath.dirname(key), target))
            if normalized == ".." or normalized.startswith("../") or normalized.startswith("/") or normalized.split("/")[0] in (".git", ".scaffold"): raise SystemExit(43)

    expected_files = {".crew-handoff-manifest.json", "source.bundle"} | {"files/" + key for key in regular_paths}
    expected_dirs = {parent.as_posix() for name in expected_files for parent in pathlib.PurePosixPath(name).parents if parent.as_posix() != "."}
    found = set()
    for root, dirs, files in os.walk(staging, followlinks=False):
        for name in [*dirs, *files]:
            candidate = pathlib.Path(root) / name
            key = candidate.relative_to(staging).as_posix()
            info = candidate.lstat()
            if stat.S_ISDIR(info.st_mode):
                if key not in expected_dirs: raise SystemExit(44)
                if recovery is not None and stat.S_IMODE(info.st_mode) != 0o700: raise SystemExit(44)
            elif stat.S_ISREG(info.st_mode):
                if key not in expected_files: raise SystemExit(44)
                if recovery is not None and stat.S_IMODE(info.st_mode) != 0o600: raise SystemExit(44)
                found.add(key)
            else: raise SystemExit(44)
    if found != expected_files: raise SystemExit(44)
    for key in regular_paths:
        entry = by_path[key][1]
        actual_sha, count, _, _ = digest_file(staging / "files" / key, 64 * 1024 * 1024)
        if (actual_sha, count) != (entry["sha256"], entry["byteCount"]): raise SystemExit(44)
        total += count
    if total > MAX_TOTAL: raise SystemExit(44)
    if recovery is not None and recovery.get("validate_only"):
        raise SystemExit(0)

    # Platform commands are read-only; all refs and imported objects stay isolated.
    temporary = pathlib.Path(tempfile.mkdtemp(prefix="build-" if recovery is not None else ".crew-handoff-", dir=operation if recovery is not None else destination.parent))
    workspace = temporary / "checkout"
    workspace.mkdir()
    git(workspace, ["init", "-q", "--template=", "--object-format=" + ("sha256" if len(expected_base) == 64 else "sha1")])
    shallow_path = workspace / ".git/shallow"
    platform_shallow = b""
    if prerequisite is not None:
        objects_path = pathlib.Path(git(platform, ["rev-parse", "--path-format=absolute", "--git-path", "objects"], True).decode("utf-8").strip()).resolve(strict=True)
        if not objects_path.is_dir() or any(c in str(objects_path) for c in "\r\n"): repository_failure("prerequisite_objects_path")
        (workspace / ".git/objects/info/alternates").write_text(str(objects_path) + "\n")
        if git(workspace, ["cat-file", "-t", prerequisite], True).strip() != b"commit": repository_failure("prerequisite_not_commit")
        git(workspace, ["update-ref", "refs/crew-handoff/prerequisite", prerequisite])
        platform_shallow_path = pathlib.Path(git(platform, ["rev-parse", "--path-format=absolute", "--git-path", "shallow"], True).decode("utf-8").strip())
        if platform_shallow_path.exists():
            with platform_shallow_path.open("rb") as source: platform_shallow = source.read(MAX_OBJECTS * (len(expected_base) + 1) + 1)
            boundaries = platform_shallow.splitlines()
            if len(boundaries) > MAX_OBJECTS or any(len(oid) != len(expected_base) or any(c not in b"0123456789abcdef" for c in oid) for oid in boundaries): repository_failure("prerequisite_shallow_bounds")
            shallow_path.write_bytes(platform_shallow)
    elif shallow:
        shallow_path.write_text(expected_base + "\n")
    if bundle_bytes:
        inspect_bundle(bundle)
        git(workspace, ["bundle", "verify", str(bundle)])
        git(workspace, ["bundle", "unbundle", str(bundle)])
    inspect_objects(workspace)
    if git(workspace, ["cat-file", "-t", expected_base], True).strip() != b"commit": repository_failure("base_not_commit")
    if prerequisite is not None:
        # Validate the imported graph and complete checkout tree, stopping at the
        # trusted prerequisite instead of traversing all platform history.
        shallow_path.write_text(prerequisite + "\n")
        try:
            git(workspace, ["merge-base", "--is-ancestor", prerequisite, expected_base])
            # Verify imported pack contents, then walk only the bounded graph.
            # fsck --no-full cannot validate packed HEAD; --full scans alternates.
            for pack in (workspace / ".git/objects/pack").glob("*.pack"):
                git(workspace, ["index-pack", "--strict", "--verify", str(pack)])
            reachable = git(workspace, ["rev-list", "--objects", "--no-object-names", "--missing=error", expected_base], True)
            if reachable.count(b"\n") > MAX_OBJECTS: repository_failure("reachable_object_bounds")
        finally:
            if platform_shallow: shallow_path.write_bytes(platform_shallow)
            else: shallow_path.unlink()
        git(workspace, ["update-ref", "-d", "refs/crew-handoff/prerequisite"])
    else:
        git(workspace, ["fsck", "--full", "--strict"])
    git(workspace, ["checkout", "-q", "--detach", expected_base])
    if git(workspace, ["rev-parse", "HEAD"], True).strip().decode("ascii") != expected_base: raise SystemExit(47)
    inspect_materialized_tree(workspace)
    for key, (rel, entry) in by_path.items():
        if secure_parent(workspace, rel, False): remove_path(workspace.joinpath(*rel.parts))
    for key, (rel, entry) in by_path.items():
        if entry["kind"] == "delete": continue
        secure_parent(workspace, rel, True)
        target = workspace.joinpath(*rel.parts)
        if entry["kind"] == "regular":
            shutil.copyfile(staging / "files" / key, target, follow_symlinks=False)
            os.chmod(target, 0o755 if entry["executable"] else 0o644, follow_symlinks=False)
        else: os.symlink(entry["target"], target)
    for key, (rel, entry) in by_path.items():
        target = workspace.joinpath(*rel.parts)
        if entry["kind"] == "delete":
            if target.exists() or target.is_symlink(): raise SystemExit(48)
        elif entry["kind"] == "symlink":
            if not target.is_symlink() or os.readlink(target) != entry["target"]: raise SystemExit(48)
        else:
            actual_sha, count, mode, _ = digest_file(target, 64 * 1024 * 1024)
            if (actual_sha, count, mode) != (entry["sha256"], entry["byteCount"], 0o755 if entry["executable"] else 0o644): raise SystemExit(48)
    def git_paths(args):
        return {entry.decode("utf-8") for entry in git(workspace, args, True).split(b"\0") if entry}
    visible = git_paths(["diff", "--name-only", "--no-renames", "-z", expected_base, "--"]) | git_paths(["ls-files", "--others", "--exclude-standard", "-z"]) | git_paths(["ls-files", "--others", "--ignored", "--exclude-standard", "-z", "--", ".omx/specs", ".omx/interviews", ".omx/plans"])
    if not visible.issubset(paths): raise SystemExit(48)
    if cwd_parts:
        secure_parent(workspace, pathlib.PurePosixPath(*cwd_parts, ".cwd-check"), True)
    inspect_materialized_tree(workspace)
    remote_cwd = workspace.joinpath(*cwd_parts)
    if not remote_cwd.is_dir() or remote_cwd.resolve(strict=True) != remote_cwd: raise SystemExit(49)
    if recovery is not None:
        if destination.exists() or destination.is_symlink():
            if destination.is_symlink() or not destination.is_dir() or destination.resolve(strict=True) != destination: raise SystemExit(49)
            recovery["compare_tree"](workspace, destination)
            for relative in ("config", "HEAD", "objects/info/alternates", "shallow"):
                left, right = workspace / ".git" / relative, destination / ".git" / relative
                if left.exists() != right.exists(): raise SystemExit(49)
                if left.exists() and digest_file(left, MAX_TOTAL)[:3] != digest_file(right, MAX_TOTAL)[:3]: raise SystemExit(49)
            recovery["check_metadata"](workspace / ".git", destination / ".git")
            inspect_objects(destination)
            if git(destination, ["rev-parse", "HEAD"], True).strip().decode("ascii") != expected_base: raise SystemExit(49)
            if git(workspace, ["ls-files", "-v", "--stage", "-z"], True) != git(destination, ["ls-files", "-v", "--stage", "-z"], True): raise SystemExit(49)
            if prerequisite is None: git(destination, ["fsck", "--full", "--strict"])
        else:
            recovery["sync_tree"](workspace)
            recovery["publish_directory"](workspace, destination)
        published = True
        recovery["phase"]("repository")
        recovery["cwd"] = str(destination.joinpath(*cwd_parts))
    else:
        backup = temporary / "previous"
        if destination.exists() or destination.is_symlink():
            if destination.is_symlink() or not destination.is_dir(): raise SystemExit(49)
            os.replace(destination, backup)
        try: os.replace(workspace, destination)
        except BaseException:
            if backup.exists(): os.replace(backup, destination)
            raise
        published = True
        print("verified:" + expected_manifest_sha + ":" + str(destination.joinpath(*cwd_parts)))
except (OSError, ValueError, subprocess.SubprocessError):
    raise SystemExit(50)
finally:
    if temporary is not None and (published or not (temporary / "previous").exists()):
        shutil.rmtree(temporary, ignore_errors=True)
    if recovery is None: shutil.rmtree(staging, ignore_errors=True)
"#;

const VERIFY_HANDOFF_PYTHON: &str = r#"import hashlib, json, os, pathlib, shutil, stat, sys, tempfile, uuid
probe_only = sys.argv[1] == "--probe"
platform_root = pathlib.Path("/workspace/ashler-platform").resolve(strict=True)
recovery = globals().get("CREW_ISOLATED_RECOVERY")
if recovery is not None:
    namespace = recovery["namespace"]
    if probe_only or len(namespace) != 64 or any(c not in "0123456789abcdef" for c in namespace): raise SystemExit(20)
    operation = platform_root / ".scaffold" / ("crew-native-" + namespace)
    if operation != pathlib.Path(recovery["operation_root"]) or operation.resolve(strict=True) != operation: raise SystemExit(20)
if not probe_only:
    staging_root = pathlib.Path(sys.argv[1]).resolve(strict=True)
    workspace = platform_root
    if (recovery is not None and operation not in staging_root.parents) or (recovery is None and (workspace not in staging_root.parents or staging_root.relative_to(workspace).as_posix() != ".scaffold/omp-handoff-staging")):
        raise SystemExit(20)
handoff_root = pathlib.Path("/workspace/crew-handoff")
workspace = pathlib.Path(sys.argv[7])
if recovery is not None: handoff_root = platform_root if recovery["session_only"] else platform_root.parent / ("crew-native-" + namespace)
validate_only = recovery is not None and recovery.get("validate_only")
if workspace != workspace.resolve(strict=not probe_only and not validate_only) or (not probe_only and not validate_only and not workspace.is_dir()) or (workspace != platform_root and workspace != handoff_root and handoff_root not in workspace.parents) or (recovery is not None and workspace != handoff_root and handoff_root not in workspace.parents):
    raise SystemExit(29)
if recovery is not None and recovery["session_only"] and workspace != platform_root: raise SystemExit(29)
rel = pathlib.PurePosixPath(sys.argv[2])
if rel.is_absolute() or not rel.parts or any(p in ("", ".", "..") for p in rel.parts):
    raise SystemExit(21)
if not probe_only: staged = staging_root.joinpath(*rel.parts)
expected_sha, expected_bytes = sys.argv[3], int(sys.argv[4])
if len(expected_sha) != 64 or any(c not in "0123456789abcdef" for c in expected_sha):
    raise SystemExit(29)
expected_digest = (expected_sha, expected_bytes)
expected_native_id, expected_local_cwd = sys.argv[5], sys.argv[6]
if not expected_native_id or len(expected_native_id) > 128 or any(ord(c) < 32 for c in expected_native_id):
    raise SystemExit(29)
if not expected_local_cwd or len(expected_local_cwd) > 4096 or any(ord(c) < 32 for c in expected_local_cwd):
    raise SystemExit(29)
if recovery is not None and not 0 < expected_bytes <= 64 * 1024 * 1024: raise SystemExit(29)
target_native_id = expected_native_id
if recovery is not None:
    identity = bytearray(bytes.fromhex(namespace)[:16])
    identity[6] = (identity[6] & 15) | 128
    identity[8] = (identity[8] & 63) | 128
    target_native_id = str(uuid.UUID(bytes=bytes(identity)))
    if recovery["target_native_id"] != target_native_id or target_native_id == expected_native_id: raise SystemExit(29)

def file_digest(path, require_mode):
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | (getattr(os, "O_NONBLOCK", 0) if recovery is not None else 0))
    except (FileNotFoundError, OSError):
        return None
    with os.fdopen(fd, "rb") as f:
        st = os.fstat(f.fileno())
        if not stat.S_ISREG(st.st_mode) or stat.S_IMODE(st.st_mode) != require_mode:
            return None
        if recovery is not None and (st.st_nlink != 1 or st.st_size > expected_bytes + 65536): return None
        h, count = hashlib.sha256(), 0
        while True:
            block = f.read(1024 * 1024)
            if not block: break
            count += len(block); h.update(block)
            if recovery is not None and count > expected_bytes + 65536: return None
    return h.hexdigest(), count

def exact_file(path, require_mode, digest):
    return file_digest(path, require_mode) == digest

def bounded_regular(path, allowed_modes, limit):
    try:
        fd = os.open(path, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    except (FileNotFoundError, OSError):
        return None
    with os.fdopen(fd, "rb") as f:
        st = os.fstat(f.fileno())
        if not stat.S_ISREG(st.st_mode) or stat.S_IMODE(st.st_mode) not in allowed_modes or st.st_size <= 0 or st.st_size > limit:
            return None
        contents = f.read(limit + 1)
    return contents if len(contents) == st.st_size else None

def exact_dir(path):
    try:
        st = path.lstat()
    except (FileNotFoundError, OSError):
        return False
    return stat.S_ISDIR(st.st_mode) and not stat.S_ISLNK(st.st_mode)

def copy_rebound_session(source, out):
    prefix, scanned = [], 0
    while scanned <= 65536:
        line = source.readline(65537)
        if not line: break
        scanned += len(line)
        if scanned > 65536: raise SystemExit(29)
        prefix.append(line)
        body = line[:-1] if line.endswith(b"\n") else line
        try: record = json.loads(body)
        except (UnicodeDecodeError, ValueError): continue
        if not isinstance(record, dict) or record.get("type") != "session": continue
        if record.get("id") != expected_native_id or record.get("cwd") != expected_local_cwd:
            raise SystemExit(29)
        if validate_only: return
        record["cwd"] = str(workspace)
        if recovery is not None: record["id"] = target_native_id
        for original in prefix[:-1]: out.write(original)
        out.write(json.dumps(record, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))
        if line.endswith(b"\n"): out.write(b"\n")
        if recovery is None:
            shutil.copyfileobj(source, out, length=1024 * 1024)
        else:
            copied = scanned
            while True:
                block = source.read(min(1024 * 1024, expected_bytes - copied + 1))
                if not block: break
                copied += len(block)
                if copied > expected_bytes: raise SystemExit(29)
                out.write(block)
            if copied != expected_bytes: raise SystemExit(29)
        return
    raise SystemExit(29)

def relative_under(root, path):
    if path == root:
        return ""
    try:
        return path.relative_to(root).as_posix()
    except ValueError:
        return None

def encode_relative_session_dir(prefix, relative):
    encoded = relative.replace("/", "-").replace("\\", "-").replace(":", "-")
    if not encoded:
        return prefix
    return prefix + encoded if prefix.endswith("-") else prefix + "-" + encoded

def omp_session_dir_name(cwd):
    home_relative = relative_under(pathlib.Path.home().resolve(strict=True), cwd)
    if home_relative is not None:
        return encode_relative_session_dir("-", home_relative)
    temp_relative = relative_under(pathlib.Path(tempfile.gettempdir()).resolve(strict=True), cwd)
    if temp_relative is not None:
        return encode_relative_session_dir("-tmp", temp_relative)
    absolute = str(cwd)
    if absolute.startswith(("/", "\\")):
        absolute = absolute[1:]
    return "--" + absolute.replace("/", "-").replace("\\", "-").replace(":", "-") + "--"

def secure_dirs(root, parts):
    current = root
    for part in parts:
        candidate = current / part
        try:
            candidate.mkdir(mode=0o700)
        except FileExistsError:
            st = candidate.lstat()
            if not stat.S_ISDIR(st.st_mode) or stat.S_ISLNK(st.st_mode):
                raise SystemExit(25)
            if recovery is None or (not recovery["session_only"] and current != root):
                os.chmod(candidate, 0o700, follow_symlinks=False)
        current = candidate
    return current

try:
    if not probe_only:
        try:
            staged_resolved = staged.resolve(strict=True)
        except (FileNotFoundError, OSError):
            raise SystemExit(22)
        if staging_root not in staged_resolved.parents or staged_resolved != staged:
            raise SystemExit(22)
        if not exact_file(staged, 0o600, expected_digest):
            raise SystemExit(22)
        if validate_only:
            with staged.open("rb") as source:
                copy_rebound_session(source, None)
            raise SystemExit(0)
    runtime_value = os.environ.get("SCAFFOLD_RUNTIME_DIR", "")
    runtime_input = pathlib.Path(runtime_value)
    if not runtime_value or not runtime_input.is_absolute():
        raise SystemExit(28)
    try:
        runtime_dir = runtime_input.resolve(strict=True)
        profile_path = (runtime_dir / "omp-inference/profile.json").resolve(strict=True)
    except (FileNotFoundError, OSError):
        raise SystemExit(28)
    profile_bytes = bounded_regular(profile_path, (0o600, 0o644), 4096)
    if profile_bytes is None:
        raise SystemExit(28)
    try:
        profile = json.loads(profile_bytes)
    except (TypeError, ValueError, json.JSONDecodeError):
        raise SystemExit(28)
    models_value = profile.get("modelsPath") if isinstance(profile, dict) else None
    if not isinstance(models_value, str) or not models_value or len(models_value) > 4096:
        raise SystemExit(28)
    models_input = pathlib.Path(models_value)
    if not models_input.is_absolute() or models_input.name != "models.yml":
        raise SystemExit(28)
    try:
        models_path = models_input.resolve(strict=True)
    except (FileNotFoundError, OSError):
        raise SystemExit(28)
    if models_path.name != "models.yml" or bounded_regular(models_path, (0o600,), 1024 * 1024) is None:
        raise SystemExit(28)
    agent_dir = models_path.parent
    if tuple(agent_dir.parts[-4:]) != (".omp", "profiles", "scaffold-host", "agent") or not exact_dir(agent_dir):
        raise SystemExit(28)
    def native_identity_present(exclude=None):
        roots = [agent_dir / "sessions"]
        home_root = pathlib.Path.home().resolve(strict=True) / os.environ.get("PI_CONFIG_DIR", ".omp") / "profiles/scaffold-host/agent/sessions"
        if home_root not in roots: roots.append(home_root)
        xdg = os.environ.get("XDG_DATA_HOME", "")
        if xdg:
            xdg_profile = pathlib.Path(xdg) / "omp/profiles/scaffold-host"
            if not xdg_profile.is_absolute(): raise SystemExit(25)
            if xdg_profile.exists(): roots.append(xdg_profile / "sessions")
        pending, entries, observed = list(roots), 0, 0
        while pending:
            directory = pending.pop()
            if directory.is_symlink(): raise SystemExit(25)
            try: children = os.scandir(directory)
            except FileNotFoundError:
                if directory not in roots: raise SystemExit(25)
                continue
            with children:
                for entry in children:
                    entries += 1
                    if entries > 4096: raise SystemExit(25)
                    mode = entry.stat(follow_symlinks=False).st_mode
                    if stat.S_ISDIR(mode):
                        pending.append(pathlib.Path(entry.path)); continue
                    if not stat.S_ISREG(mode): raise SystemExit(25)
                    if not entry.name.endswith(".jsonl"): continue
                    if exclude is not None and pathlib.Path(entry.path) == exclude: continue
                    fd = os.open(entry.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
                    with os.fdopen(fd, "rb") as journal:
                        if not stat.S_ISREG(os.fstat(journal.fileno()).st_mode): raise SystemExit(25)
                        scanned, identified = 0, False
                        while scanned < 65536:
                            line = journal.readline(65537 - scanned)
                            if not line: break
                            scanned += len(line); observed += len(line)
                            if scanned > 65536 or observed > 16 * 1024 * 1024: raise SystemExit(25)
                            try: record = json.loads(line)
                            except (UnicodeDecodeError, ValueError): continue
                            if isinstance(record, dict) and record.get("type") == "session":
                                native_id = record.get("id")
                                if not isinstance(native_id, str) or not native_id: raise SystemExit(25)
                                if native_id == target_native_id: return True
                                identified = True; break
                        if not identified: raise SystemExit(25)
        return False
    if probe_only:
        parent = agent_dir / "sessions" / omp_session_dir_name(workspace)
        for directory in (parent.parent, parent):
            if directory.is_symlink() or (directory.exists() and not exact_dir(directory)):
                raise SystemExit(25)
        target = parent / rel.name
        if target.exists() or target.is_symlink() or native_identity_present():
            print("present"); raise SystemExit(0)
        print("absent"); raise SystemExit(0)
    if recovery is None: os.chmod(agent_dir, 0o700, follow_symlinks=False)


    parent = secure_dirs(agent_dir, ("sessions", omp_session_dir_name(workspace)))
    target = parent / (("recovered-" + namespace + ".jsonl") if recovery is not None else rel.name)
    if recovery is not None:
        if not recovery["session_only"] and omp_session_dir_name(workspace) in (omp_session_dir_name(platform_root), omp_session_dir_name(pathlib.Path("/workspace/crew-handoff"))): raise SystemExit(25)
        if parent.resolve(strict=True) != parent or not target.is_absolute(): raise SystemExit(25)
    temporary = parent / ((".crew-native-" + namespace + "-") if recovery is not None else ".comet-handoff-")
    temporary = temporary.with_name(temporary.name + uuid.uuid4().hex)
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        with os.fdopen(fd, "wb") as out, staged.open("rb") as source:
            copy_rebound_session(source, out)
            out.flush(); os.fsync(out.fileno())
        os.chmod(temporary, 0o600, follow_symlinks=False)
        rebound_digest = file_digest(temporary, 0o600)
        if rebound_digest is None: raise SystemExit(27)
        if recovery is not None:
            excluded = None
            if target.exists() or target.is_symlink():
                if target.is_symlink() or not exact_file(target, 0o600, rebound_digest): raise SystemExit(26)
                excluded = target
            if native_identity_present(excluded): raise SystemExit(26)
        if target.exists() or target.is_symlink():
            if target.is_symlink(): raise SystemExit(26)
            if recovery is None and exact_file(target, 0o600, expected_digest):
                os.replace(temporary, target)
            elif not exact_file(target, 0o600, rebound_digest):
                raise SystemExit(26)
        else:
            try:
                if recovery is not None: recovery["publish_directory"](temporary, target)
                else: os.link(temporary, target, follow_symlinks=False)
            except FileExistsError:
                if target.is_symlink() or not exact_file(target, 0o600, rebound_digest):
                    raise SystemExit(26)
        if not exact_file(target, 0o600, rebound_digest): raise SystemExit(27)
    finally:
        try: temporary.unlink()
        except FileNotFoundError: pass
    if recovery is not None:
        directory_fd = os.open(parent, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try: os.fsync(directory_fd)
        finally: os.close(directory_fd)
        recovery["launch_selector"] = str(target)
        recovery["rebound_sha256"], recovery["rebound_bytes"] = rebound_digest
        recovery["phase"]("native")
    else: print("verified")
finally:
    if not probe_only and recovery is None:
        try: staged.unlink()
        except FileNotFoundError: pass
"#;

fn device_join_grant_id(credential: &str) -> Option<String> {
    let mut parts = credential.split('.');
    let prefix = parts.next()?;
    let grant_id = parts.next()?;
    let secret = parts.next()?;
    if prefix != "cg1"
        || grant_id.len() != 32
        || !grant_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || secret.is_empty()
        || parts.next().is_some()
    {
        return None;
    }
    Some(grant_id.to_string())
}

#[derive(Debug, Clone)]
pub struct DeviceJoinGrantRequest {
    pub principal_subject: String,
    pub scope: CollaborationScope,
    pub sandbox_id: String,
    pub device_id: String,
    pub lifecycle_epoch: u64,
    pub capabilities: Vec<String>,
    pub expires_in_seconds: u32,
}

pub struct DeviceJoinCredential {
    credential: String,
    grant_id: String,
    join_expires_at: i64,
    control_expires_at: i64,
}

impl fmt::Debug for DeviceJoinCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceJoinCredential")
            .field("credential", &"<redacted>")
            .field("grant_id", &self.grant_id)
            .field("join_expires_at", &self.join_expires_at)
            .field("control_expires_at", &self.control_expires_at)
            .finish()
    }
}

#[async_trait]
pub trait DeviceJoinGrantProvider: Send + Sync {
    async fn mint(
        &self,
        request: &DeviceJoinGrantRequest,
        cancellation: &CancellationToken,
    ) -> Result<DeviceJoinCredential, ScaffoldError>;
}

#[derive(Clone)]
pub struct EdgeDeviceJoinGrantClient {
    http: reqwest::Client,
    origin: Url,
    bearer: Arc<dyn TokenSource>,
}

impl EdgeDeviceJoinGrantClient {
    pub fn new(
        origin: impl AsRef<str>,
        bearer: Arc<dyn TokenSource>,
    ) -> Result<Self, ScaffoldError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(REQUEST_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()?,
            origin: exact_origin(origin.as_ref())?,
            bearer,
        })
    }
}

#[async_trait]
impl DeviceJoinGrantProvider for EdgeDeviceJoinGrantClient {
    async fn mint(
        &self,
        request: &DeviceJoinGrantRequest,
        cancellation: &CancellationToken,
    ) -> Result<DeviceJoinCredential, ScaffoldError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Response {
            grant: String,
            expires_at: i64,
            access_expires_at: Option<i64>,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Request<'a> {
            target_device_id: &'a str,
            session_id: &'a str,
            deployment_id: &'a str,
            sandbox_id: &'a str,
            lifecycle_epoch: u64,
            capabilities: &'a [String],
            ttl_seconds: u32,
        }

        let session_id = request
            .scope
            .session_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(ScaffoldError::DeviceJoinGrantUnavailable)?;
        let deployment_id = request
            .scope
            .deployment_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(ScaffoldError::DeviceJoinGrantUnavailable)?;
        if request.sandbox_id.trim().is_empty() {
            return Err(ScaffoldError::DeviceJoinGrantUnavailable);
        }
        let body = Request {
            target_device_id: &request.device_id,
            session_id,
            deployment_id,
            sandbox_id: &request.sandbox_id,
            lifecycle_epoch: request.lifecycle_epoch,
            capabilities: &request.capabilities,
            ttl_seconds: request.expires_in_seconds,
        };

        let mut url = self.origin.clone();
        url.path_segments_mut()
            .expect("validated HTTP origin")
            .extend(JOIN_GRANT_PATH);
        let http_request = self
            .http
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body);
        let (response, generation) = send_authenticated(self.bearer.as_ref(), http_request, cancellation).await.map_err(|error| {
            if matches!(error, ScaffoldError::Transport { .. }) {
                ScaffoldError::DeviceJoinGrantUnavailable
            } else {
                error
            }
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let retry_after = response_retry_after(&response);
            let mut response = response;
            let mut bytes = Vec::new();
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
                    chunk = response.chunk() => chunk,
                };
                match chunk {
                    Ok(Some(chunk)) if bytes.len() + chunk.len() <= 16 * 1024 => {
                        bytes.extend_from_slice(&chunk)
                    }
                    Ok(None) => break,
                    _ => {
                        bytes.clear();
                        break;
                    }
                }
            }
            let decoded = serde_json::from_slice::<Value>(&bytes).ok();
            let code = decoded
                .as_ref()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .or_else(|| value.pointer("/error/code").and_then(Value::as_str))
                        .or_else(|| value.get("code").and_then(Value::as_str))
                })
                .filter(|code| {
                    !code.is_empty()
                        && code.len() <= 128
                        && code.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                        })
                })
                .unwrap_or("device_join_grant_rejected");
            return Err(authenticated_error(self.bearer.as_ref(), generation,
                ScaffoldError::Api(ScaffoldApiErrorDisplay(ScaffoldApiError {
                    status: status.as_u16(),
                    code: code.to_string(),
                    message: None,
                    retry_after,
                })),
            ));
        }
        let response: Response = response
            .json()
            .await
            .map_err(|_| ScaffoldError::DeviceJoinGrantUnavailable)?;
        let now = now_ms();
        let control_expires_at = response.access_expires_at.unwrap_or_else(|| {
            response
                .expires_at
                .saturating_sub(i64::from(request.expires_in_seconds) * 1000)
                .saturating_add(DEVICE_ACCESS_TTL_MS)
        });
        let grant_id = device_join_grant_id(&response.grant)
            .filter(|_| response.expires_at > now)
            .filter(|_| control_expires_at > response.expires_at)
            .filter(|_| !request.principal_subject.trim().is_empty())
            .filter(|_| !request.sandbox_id.trim().is_empty())
            .ok_or(ScaffoldError::DeviceJoinGrantUnavailable)?;
        Ok(DeviceJoinCredential {
            credential: response.grant,
            grant_id,
            join_expires_at: response.expires_at,
            control_expires_at,
        })
    }
}

pub struct UnavailableDeviceJoinGrantProvider;

#[async_trait]
impl DeviceJoinGrantProvider for UnavailableDeviceJoinGrantProvider {
    async fn mint(
        &self,
        _request: &DeviceJoinGrantRequest,
        _cancellation: &CancellationToken,
    ) -> Result<DeviceJoinCredential, ScaffoldError> {
        Err(ScaffoldError::DeviceJoinGrantUnavailable)
    }
}


#[derive(Clone)]
pub struct ScaffoldRuntime {
    inner: Arc<ScaffoldRuntimeInner>,
    deployment_id: Arc<str>,
}

struct ScaffoldRuntimeInner {
    client: ScaffoldClient,
    edge_origin: String,
    grants: Arc<dyn DeviceJoinGrantProvider>,
    handoff_permits: Arc<Semaphore>,
    environments: Mutex<BTreeMap<String, SessionEnvironment>>,
    watch_tx: watch::Sender<ScaffoldEnvironmentSnapshot>,
}

impl ScaffoldRuntime {
    pub fn new(
        client: ScaffoldClient,
        edge_origin: impl Into<String>,
        grants: Arc<dyn DeviceJoinGrantProvider>,
    ) -> Self {
        let snapshot = ScaffoldEnvironmentSnapshot {
            environments: Vec::new(),
            refreshed_at: now_ms(),
        };
        let (watch_tx, _) = watch::channel(snapshot);
        Self {
            deployment_id: client.project_scope.as_str().into(),
            inner: Arc::new(ScaffoldRuntimeInner {
                client,
                edge_origin: edge_origin.into(),
                grants,
                handoff_permits: Arc::new(Semaphore::new(1)),
                environments: Mutex::new(BTreeMap::new()),
                watch_tx,
            }),
        }
    }

    pub fn with_deployment_id(mut self, deployment_id: String) -> Self {
        self.deployment_id = deployment_id.into();
        self
    }



    fn remote_argv(&self, command: &str) -> Vec<String> {
        vec![".scaffold/crew/bin/comet".into(), "--data-dir".into(),
            ".scaffold/crew-data".into(), "--ipc-port".into(), "39400".into(), command.into()]
    }

    pub(crate) fn deployment_id(&self) -> &str {
        &self.deployment_id
    }

    pub(crate) fn client(&self) -> ScaffoldClient {
        self.inner.client.clone()
    }

    pub fn watch(&self) -> watch::Receiver<ScaffoldEnvironmentSnapshot> {
        self.inner.watch_tx.subscribe()
    }

    /// Explicit event-driven refresh. There is deliberately no polling task.
    pub async fn refresh(
        &self,
        scope: &CollaborationScope,
        cancellation: &CancellationToken,
    ) -> Result<ScaffoldEnvironmentSnapshot, ScaffoldError> {
        let environments = self.inner.client.list(scope, cancellation).await?;
        {
            let mut current = lock(&self.inner.environments);
            current.clear();
            for environment in environments {
                current.insert(
                    environment_sandbox_id(&environment)?.to_string(),
                    environment,
                );
            }
        }
        Ok(self.publish())
    }

    pub async fn control(
        &self,
        control: ScaffoldEnvironmentControl,
        cancellation: &CancellationToken,
    ) -> Result<ScaffoldEnvironmentControlResult, ScaffoldError> {
        let resume_requested = matches!(control, ScaffoldEnvironmentControl::Resume { .. });
        let mut control_grant = None;
        let (environment, attached_device_id, run_id, handoff) = match control {
            ScaffoldEnvironmentControl::Inspect { sandbox_id, scope } => (
                self.inner
                    .client
                    .inspect(&sandbox_id, &scope, cancellation)
                    .await?,
                None,
                None,
                None,
            ),
            ScaffoldEnvironmentControl::Create {
                scope,
                name,
                source_ref,
                region,
                database_environment,
                agent_route,
            } => (
                self.inner
                    .client
                    .create(
                        &scope,
                        CreateSandboxOptions {
                            name: name.as_deref(),
                            source_ref: source_ref.as_deref(),
                            region: region.as_deref(),
                            database_environment,
                        },
                        &agent_route,
                        cancellation,
                    )
                    .await?,
                None,
                None,
                None,
            ),
            ScaffoldEnvironmentControl::Pause { sandbox_id, scope } => (
                self.inner
                    .client
                    .pause(&sandbox_id, &scope, cancellation)
                    .await?,
                None,
                None,
                None,
            ),
            ScaffoldEnvironmentControl::Stop { sandbox_id, scope } => (
                self.inner
                    .client
                    .stop(&sandbox_id, &scope, cancellation)
                    .await?,
                None,
                None,
                None,
            ),
            ScaffoldEnvironmentControl::UpdateAgentRoute {
                sandbox_id,
                scope,
                agent_route,
            } => (
                self.inner
                    .client
                    .update_agent_route(&sandbox_id, &scope, &agent_route, cancellation)
                    .await?,
                None,
                None,
                None,
            ),
            ScaffoldEnvironmentControl::Start { sandbox_id, scope, expected_lifecycle_epoch } => (
                self.inner.client.start(&sandbox_id, &scope, expected_lifecycle_epoch, cancellation).await?,
                None, None, None,
            ),
            ScaffoldEnvironmentControl::Attach { sandbox_id, scope }
            | ScaffoldEnvironmentControl::Resume { sandbox_id, scope } => {
                let mut environment = self
                    .inner
                    .client
                    .inspect(&sandbox_id, &scope, cancellation)
                    .await?;
                let lifecycle = scaffold_source(&environment)?.0;
                if matches!(lifecycle, zeron_proto::ScaffoldLifecycle::Failed | zeron_proto::ScaffoldLifecycle::Stopped) {
                    return Err(ScaffoldError::AttachRejected(lifecycle));
                }
                if resume_requested || lifecycle == zeron_proto::ScaffoldLifecycle::Paused {
                    let resumed = self
                        .inner
                        .client
                        .resume(&sandbox_id, &scope, cancellation)
                        .await?;
                    if environment_sandbox_id(&resumed)? != sandbox_id
                        || resumed.scope != environment.scope
                        || resumed.owner_principal != environment.owner_principal
                        || scaffold_source(&resumed)?.1 < scaffold_source(&environment)?.1
                    {
                        return Err(ScaffoldError::InvalidResponse(
                            "sandbox identity changed while resuming".into(),
                        ));
                    }
                    environment = resumed;
                }
                let (device_id, run_id, grant) = self
                    .attach(&sandbox_id, &scope, &environment, cancellation)
                    .await?;
                control_grant = Some(grant);
                (environment, Some(device_id), run_id, None)
            }
            ScaffoldEnvironmentControl::HandoffOmpSession {
                sandbox_id,
                scope,
                native_session_id,
                cwd,
                session_only,
            } => {
                let environment = self
                    .inner
                    .client
                    .inspect(&sandbox_id, &scope, cancellation)
                    .await?;
                let (lifecycle, _, _) = scaffold_source(&environment)?;
                if !matches!(
                    lifecycle,
                    zeron_proto::ScaffoldLifecycle::Ready
                        | zeron_proto::ScaffoldLifecycle::AgentRunning
                ) {
                    return Err(ScaffoldError::OmpSessionHandoffFailed);
                }
                let handoff_permit = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
                    permit = self.inner.handoff_permits.clone().acquire_owned() => {
                        permit.map_err(|_| ScaffoldError::OmpSessionHandoffFailed)?
                    }
                };
                let remote_base = if session_only {
                    None
                } else {
                    // Capture against the actual immutable checkout, not a source-ref label.
                    let base_argv = vec![
                        "git".to_string(), "-C".to_string(),
                        "/workspace/ashler-platform".to_string(),
                        "rev-parse".to_string(), "--verify".to_string(), "HEAD^{commit}".to_string(),
                    ];
                    let remote_base = self.inner.client.exec(
                        &sandbox_id,
                        &ExecBody { argv: &base_argv, mode: "inline", timeout_ms: 10_000 },
                        cancellation,
                    ).await?;
                    if !remote_base.ok || remote_base.exit_code != Some(0) {
                        return Err(ScaffoldError::HandoffStage("source_repository_base"));
                    }
                    let remote_base = remote_base.stdout.as_deref().unwrap_or_default().trim();
                    if !(remote_base.len() == 40 || remote_base.len() == 64)
                        || !remote_base.bytes().all(|byte| byte.is_ascii_hexdigit())
                    {
                        return Err(ScaffoldError::HandoffStage("source_repository_base"));
                    }
                    Some(remote_base.to_ascii_lowercase())
                };
                let capture_native_session_id = native_session_id.clone();
                let capture_cwd = cwd.clone();
                let capture_cancellation = cancellation.clone();
                let (handoff_permit, artifact, worktree) = tokio::task::spawn_blocking(move || {
                    let captured = crate::omp_session_artifact::capture_omp_file_for_session(
                        &capture_native_session_id,
                        &capture_cwd,
                        &capture_cancellation,
                    )
                    .map_err(|error| {
                        tracing::warn!(error = %error, "Scaffold OMP handoff capture failed");
                        if capture_cancellation.is_cancelled() {
                            ScaffoldError::Cancelled
                        } else {
                            ScaffoldError::HandoffStage("omp_session_capture")
                        }
                    })?;
                    let artifact = prepare_omp_handoff_archive(captured, &capture_cancellation)?;
                    let worktree = remote_base.map(|remote_base| {
                        crate::worktree_handoff::capture_worktree_handoff_cancellable(
                            std::path::Path::new(&capture_cwd), Some(&remote_base), &capture_cancellation,
                        ).map_err(|error| {
                            tracing::warn!(error = %error, "Scaffold worktree handoff capture failed");
                            if capture_cancellation.is_cancelled() {
                                ScaffoldError::Cancelled
                            } else {
                                ScaffoldError::HandoffStage("source_repository_capture")
                            }
                        })
                    }).transpose()?;
                    Ok::<_, ScaffoldError>((handoff_permit, artifact, worktree))
                })
                .await
                .map_err(|error| {
                    tracing::warn!(error = %error, "Scaffold handoff capture task failed");
                    ScaffoldError::OmpSessionHandoffFailed
                })??;
                let _handoff_permit = handoff_permit;
                let prospective_cwd = match worktree.as_ref() {
                    Some(worktree) if worktree.cwd_relative_path.is_empty() => SCAFFOLD_HANDOFF_CWD.to_string(),
                    Some(worktree) => format!("{SCAFFOLD_HANDOFF_CWD}/{}", worktree.cwd_relative_path),
                    None => "/workspace/ashler-platform".to_string(),
                };
                self.inner
                    .client
                    .require_absent_omp_session(
                        &sandbox_id,
                        &artifact,
                        &prospective_cwd,
                        cancellation,
                    )
                    .await?;
                let remote_cwd = if let Some(worktree) = worktree {
                    self.inner.client.handoff_worktree(&sandbox_id, &worktree, prospective_cwd, cancellation)
                        .await.inspect_err(|error| {
                            tracing::warn!(error = %error, "Scaffold worktree handoff upload failed");
                        })?
                } else {
                    prospective_cwd
                };
                let remote_cwd = self
                    .inner
                    .client
                    .handoff_omp_session(&sandbox_id, &artifact, &remote_cwd, cancellation)
                    .await
                    .inspect_err(|error| {
                        tracing::warn!(error = %error, "Scaffold OMP handoff upload failed");
                    })?;
                let handoff = Some((artifact.native_session_id.clone(), remote_cwd));
                (environment, None, None, handoff)
            }
        };
        let sandbox_id = environment_sandbox_id(&environment)?.to_string();
        lock(&self.inner.environments).insert(sandbox_id, environment.clone());
        self.publish();
        let room_projection = (attached_device_id.is_some() || handoff.is_some())
            .then(|| {
                Some(SessionRoomProjection {
                    project_id: environment.scope.project_id.clone(),
                    deployment_id: environment.scope.deployment_id.clone()?,
                    session_id: environment.scope.session_id.clone()?,
                })
            })
            .flatten();
        let (handoff_native_session_id, handoff_cwd) = handoff
            .map(|(native_id, cwd)| (Some(native_id), Some(cwd)))
            .unwrap_or((None, None));
        Ok(ScaffoldEnvironmentControlResult {
            environment,
            preparation_generation: None,
            attached_device_id,
            run_id,
            room_projection,
            control_grant,
            handoff_native_session_id,
            handoff_cwd,
        })
    }

    /// Observe an already-running host without installing, minting, starting or resuming.
    pub(crate) async fn observe_attachment(&self, environment: SessionEnvironment,
        cancellation: &CancellationToken) -> Result<Option<ScaffoldEnvironmentControlResult>, ScaffoldError> {
        let (lifecycle, _, sandbox_id) = scaffold_source(&environment)?;
        if !matches!(lifecycle, zeron_proto::ScaffoldLifecycle::Creating
            | zeron_proto::ScaffoldLifecycle::RestoringSnapshot | zeron_proto::ScaffoldLifecycle::Starting
            | zeron_proto::ScaffoldLifecycle::Resuming | zeron_proto::ScaffoldLifecycle::Ready
            | zeron_proto::ScaffoldLifecycle::AgentRunning) {
            return Ok(None);
        }
        let argv = self.remote_argv("scaffold-authority");
        let response = self.probe_attach_authority(sandbox_id, &environment.scope, &environment, &argv, cancellation).await?;
        let Some(authority) = confirmed_host_authority(&response, &environment) else { return Ok(None) };
        let room_projection = Some(SessionRoomProjection {
            project_id: authority.scope.project_id,
            deployment_id: authority.scope.deployment_id.ok_or_else(|| ScaffoldError::InvalidScope("Missing deployment binding".into()))?,
            session_id: authority.scope.session_id.ok_or_else(|| ScaffoldError::InvalidScope("Missing session binding".into()))?,
        });
        Ok(Some(ScaffoldEnvironmentControlResult {
            environment, preparation_generation: None, attached_device_id: Some(authority.device_id),
            run_id: None, room_projection,
            control_grant: Some(ScaffoldControlGrant { id: authority.grant_id, expires_at: authority.expires_at, capabilities: authority.capabilities }),
            handoff_native_session_id: None, handoff_cwd: None,
        }))
    }

    // Only retry read-only authority probes, never credential minting
    // or process launch. Bootstrap itself makes the app ready.
    async fn probe_attach_authority(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        environment: &SessionEnvironment,
        argv: &[String],
        cancellation: &CancellationToken,
    ) -> Result<ExecResponse, ScaffoldError> {
        let mut transport_retries = 0;
        loop {
            let body = ExecBody {
                argv,
                mode: "inline",
                timeout_ms: 10_000,
            };
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
                result = self.inner.client.exec(sandbox_id, &body, cancellation) => result,
            };
            match result {
                Err(error) if error.is_runtime_starting() => {}
                Err(error)
                    if error.is_attach_probe_transport_retryable()
                        && transport_retries < ATTACH_PROBE_TRANSPORT_RETRIES =>
                {
                    // The command is read-only; the background bootstrap exec
                    // remains outside this bounded retry.
                    transport_retries += 1;
                }
                result => return result,
            }
            // Only authoritative startup for this same lifecycle can extend the
            // wait; transport retries are bounded above and use this same check.
            let pending = async {
                let current = self
                    .inner
                    .client
                    .inspect(sandbox_id, scope, cancellation)
                    .await?;
                let (lifecycle, epoch, current_id) = scaffold_source(&current)?;
                if current_id != sandbox_id
                    || current.scope != environment.scope
                    || current.owner_principal != environment.owner_principal
                    || epoch != scaffold_source(environment)?.1
                    || !matches!(
                        lifecycle,
                        zeron_proto::ScaffoldLifecycle::Creating
                            | zeron_proto::ScaffoldLifecycle::RestoringSnapshot
                            | zeron_proto::ScaffoldLifecycle::Starting
                            | zeron_proto::ScaffoldLifecycle::Resuming
                            | zeron_proto::ScaffoldLifecycle::Ready
                            | zeron_proto::ScaffoldLifecycle::AgentRunning
                    )
                {
                    return Err(ScaffoldError::InvalidResponse(
                        "sandbox lifecycle changed while waiting for its runtime".into(),
                    ));
                }
                tokio::time::sleep(RUNTIME_START_POLL).await;
                Ok(())
            };
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
                result = pending => result?,
            }
        }
    }

    async fn attach(
        &self,
        sandbox_id: &str,
        scope: &CollaborationScope,
        environment: &SessionEnvironment,
        cancellation: &CancellationToken,
    ) -> Result<(String, Option<String>, ScaffoldControlGrant), ScaffoldError> {
        let (lifecycle, lifecycle_epoch, _) = scaffold_source(environment)?;
        if !matches!(
            lifecycle,
            zeron_proto::ScaffoldLifecycle::Creating
                | zeron_proto::ScaffoldLifecycle::RestoringSnapshot
                | zeron_proto::ScaffoldLifecycle::Resuming
                | zeron_proto::ScaffoldLifecycle::Starting
                | zeron_proto::ScaffoldLifecycle::Ready
                | zeron_proto::ScaffoldLifecycle::AgentRunning
        ) {
            return Err(ScaffoldError::AttachRejected(lifecycle));
        }
        let deployment_id = scope
            .deployment_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ScaffoldError::InvalidScope("deploymentId is required to attach".into())
            })?;
        let session_id = scope
            .session_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ScaffoldError::InvalidScope("sessionId is required to attach".into()))?;
        let lifecycle_epoch = lifecycle_epoch.ok_or_else(|| {
            ScaffoldError::InvalidResponse("sandbox lifecycle epoch is required to attach".into())
        })?;


        // The server owns installation and pinned archive readiness admission.
        // Reuse only an exactly bound live host; otherwise the single background
        // exec below enters the native runner's verified admission path.
        let authority_argv = self.remote_argv("scaffold-authority");
        let authority = self
            .probe_attach_authority(
                sandbox_id,
                scope,
                environment,
                &authority_argv,
                cancellation,
            )
            .await?;
        if let Some(existing) = confirmed_host_authority(&authority, environment) {
            return Ok((
                existing.device_id,
                None,
                ScaffoldControlGrant {
                    id: existing.grant_id,
                    expires_at: existing.expires_at,
                    capabilities: existing.capabilities,
                },
            ));
        }


        let device_id = device_id_for(sandbox_id, lifecycle_epoch);
        let request = DeviceJoinGrantRequest {
            principal_subject: environment.owner_principal.clone(),
            scope: scope.clone(),
            sandbox_id: sandbox_id.to_string(),
            device_id: device_id.clone(),
            lifecycle_epoch,
            capabilities: scaffold_host_capabilities(),
            expires_in_seconds: JOIN_GRANT_TTL_SECONDS,
        };
        let join = self.inner.grants.mint(&request, cancellation).await?;

        // Deliver the one-time credential as a mode-0600 file, never as argv.
        // Comet consumes and removes this file before making the exchange.
        let bootstrap_path = format!(".comet-device-bootstrap-{}.json", uuid::Uuid::new_v4());
        let bootstrap = serde_json::json!({
            "deviceJoinGrant": join.credential,
            "projectId": scope.project_id,
            "deploymentId": deployment_id,
            "sessionId": session_id,
            "deviceId": device_id,
            "lifecycleEpoch": lifecycle_epoch,
            "sandboxId": sandbox_id
        })
        .to_string();
        let result = async {
            self.inner
                .client
                .put_file(sandbox_id, &bootstrap_path, &bootstrap, cancellation)
                .await?;
            let chmod_argv = vec![
                "chmod".to_string(),
                "600".to_string(),
                bootstrap_path.clone(),
            ];
            let chmod = self.inner.client.exec(
                sandbox_id,
                &ExecBody {
                    argv: &chmod_argv,
                    mode: "inline",
                    timeout_ms: 10_000,
                },
                cancellation,
            ).await?;
            if !chmod.ok || chmod.exit_code != Some(0) {
                return Err(ScaffoldError::InvalidResponse(
                    "could not secure the device bootstrap file".into(),
                ));
            }
            let mut argv = self.remote_argv("headless");
            argv.extend([
                "--device-bootstrap-file".to_string(),
                bootstrap_path.clone(),
                "--edge-url".to_string(),
                self.inner.edge_origin.clone(),
                "--scaffold-url".to_string(),
                // The supervisor pins SCAFFOLD_CONTROL_PLANE_URL exactly; a parsed
                // origin's implicit "/" path would fail its server-pinned scope check.
                self.inner.client.origin.as_str().trim_end_matches('/').to_owned(),
                "--project-scope".to_string(),
                scope.project_id.clone(),
                "--deployment-id".to_string(),
                deployment_id.to_owned(),
            ]);
            let started = self.inner.client.exec(
                sandbox_id,
                &ExecBody {
                    argv: &argv,
                    mode: "background",
                    timeout_ms: 0,
                },
                cancellation,
            ).await?;
            if !started.ok {
                if started.error.as_deref() == Some("exec_spawn_failed") {
                    return Err(ScaffoldError::CometNotInstalled);
                }
                return Err(ScaffoldError::InvalidResponse(
                    started.error.unwrap_or_else(|| "sandbox exec rejected".into()),
                ));
            }
            // A current supervisor marks every native launch it admits. An
            // unmarked success is the generic exec path, never a pending host.
            if !started.supervised {
                return Err(ScaffoldError::RuntimeContractMismatch);
            }
            let Some(run_id) = started.run_id.filter(|id| !id.trim().is_empty()) else {
                return Err(ScaffoldError::InvalidResponse("sandbox exec returned no supervised native runId".into()));
            };
            // Bootstrap can reuse an existing supervised host. Its newly minted
            // grant is not evidence that the serving host adopted it.
            let wait_for_authority = async {
                loop {
                    let authority = self
                        .probe_attach_authority(
                            sandbox_id,
                            scope,
                            environment,
                            &authority_argv,
                            cancellation,
                        )
                        .await?;
                    if let Some(confirmed) = confirmed_host_authority(&authority, environment) {
                        return Ok(confirmed);
                    }
                    // A completed authority response is authoritative: never wait
                    // through an expired grant or a different owner/scope/epoch.
                    if authority.ok && authority.exit_code == Some(0) {
                        return Err(ScaffoldError::InvalidResponse(
                            "Scaffold host returned invalid attachment authority".into(),
                        ));
                    }
                    // Background process creation precedes IPC readiness. Only this
                    // fresh-bootstrap window permits retrying an unavailable host.
                    tokio::time::sleep(RUNTIME_START_POLL).await;
                }
            };
            let confirmed = tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(ScaffoldError::Cancelled),
                result = tokio::time::timeout(REQUEST_TIMEOUT, wait_for_authority) => {
                    result.map_err(|_| ScaffoldError::InvalidResponse(
                        "Scaffold host did not confirm attachment authority within 30 seconds".into(),
                    ))??
                }
            };
            Ok((
                device_id,
                Some(run_id),
                ScaffoldControlGrant {
                    id: confirmed.grant_id,
                    expires_at: confirmed.expires_at,
                    capabilities: confirmed.capabilities,
                },
            ))
        }.await;
        // A failed upload response may still mean the file was committed. Always
        // remove our unique path, even after cancellation or supervised host reuse.
        // Cleanup is independent and bounded; it must never replace the outcome.
        let cleanup = CancellationToken::new();
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            self.inner.client.remove_file(sandbox_id, &bootstrap_path, &cleanup),
        ).await;
        result
    }

    fn publish(&self) -> ScaffoldEnvironmentSnapshot {
        let snapshot = ScaffoldEnvironmentSnapshot {
            environments: lock(&self.inner.environments).values().cloned().collect(),
            refreshed_at: now_ms(),
        };
        self.inner.watch_tx.send_replace(snapshot.clone());
        snapshot
    }
}
pub(crate) fn device_id_for(sandbox_id: &str, lifecycle_epoch: u64) -> String {
    format!("comet-scaffold-{sandbox_id}-e{lifecycle_epoch}")
}

fn scaffold_host_capabilities() -> Vec<String> {
    vec![
        CAPABILITY_SESSION_READ.into(),
        "session.sync.write".into(),
        CAPABILITY_SESSION_CHAT.into(),
        CAPABILITY_SESSION_CONTROL.into(),
        CAPABILITY_SESSION_ANNOTATE.into(),
        CAPABILITY_SESSION_FILES.into(),
        CAPABILITY_SESSION_ENVIRONMENT.into(),
    ]
}

fn scaffold_source(
    environment: &SessionEnvironment,
) -> Result<(zeron_proto::ScaffoldLifecycle, Option<u64>, &str), ScaffoldError> {
    match &environment.source {
        SessionEnvironmentSource::Scaffold {
            sandbox_id,
            lifecycle,
            lifecycle_epoch,
            ..
        } => Ok((*lifecycle, *lifecycle_epoch, sandbox_id)),
        SessionEnvironmentSource::Local => Err(ScaffoldError::InvalidResponse(
            "local environment has no Scaffold sandbox".into(),
        )),
    }
}

fn environment_sandbox_id(environment: &SessionEnvironment) -> Result<&str, ScaffoldError> {
    scaffold_source(environment).map(|(_, _, sandbox_id)| sandbox_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_rpc::StaticToken;
    #[cfg(unix)]
    use sha2::Digest as _;
    struct ReplacingCredential {
        generation: std::sync::atomic::AtomicU64,
        rejected: parking_lot::Mutex<Vec<u64>>,
    }

    #[async_trait]
    impl TokenSource for ReplacingCredential {
        async fn token(&self) -> Result<AccessToken, zeron_rpc::TokenError> {
            let generation = self.generation.load(std::sync::atomic::Ordering::SeqCst);
            Ok(AccessToken { value: format!("test-credential-{generation}"), generation })
        }

        fn invalidate(&self, generation: u64) {
            self.rejected.lock().push(generation);
            let _ = self.generation.compare_exchange(
                generation, generation + 1,
                std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst,
            );
        }
    }

    #[tokio::test]
    async fn authenticated_http_rejects_only_the_sent_credential_without_replaying() {
        use std::sync::atomic::{AtomicU64, Ordering};
        for endpoint in ["directory", "accounts", "authority", "join-grant"] {
            for (status, code, credential_rejected) in [
                (401, "request_rejected", true),
                (403, "request_rejected", false),
                (429, "request_rejected", false),
                (503, "request_rejected", false),
                (403, "remote_code_token_forbidden", true),
                (403, "crew_device_authority_unavailable", true),
                (403, "crew_device_scope_denied", true),
                (503, "remote_code_token_forbidden", false),
                (503, "crew_device_authority_unavailable", false),
                (503, "crew_device_scope_denied", false),
                (503, "remote_code_token_validation_failed", false),
                (503, "remote_code_token_store_unavailable", false),
            ] {
                for replaced in [false, true] {
                    let source = Arc::new(ReplacingCredential {
                        generation: AtomicU64::new(7), rejected: parking_lot::Mutex::new(Vec::new()),
                    });
                    let server_source = source.clone();
                    let (origin, captured) = mock_server_for_requests(move |request, _| {
                        assert!(request.to_ascii_lowercase().contains("authorization: bearer test-credential-7\r\n"));
                        if replaced { server_source.generation.store(8, Ordering::SeqCst); }
                        (MockResponse::Http { status, body: serde_json::json!({"error": code}).to_string() }, true)
                    }).await;
                    let client = ScaffoldClient::new(&origin, "project-a", source.clone()).unwrap();
                    let cancellation = CancellationToken::new();
                    let error = match endpoint {
                        "directory" => client.get_crew_session("9dd173da-fb53-48df-9581-026009c783e2", None).await.err(),
                        "accounts" => client.list_agent_accounts().await.err(),
                        "authority" => client.issue_agent_inference_authority(&cancellation).await.err(),
                        _ => EdgeDeviceJoinGrantClient::new(&origin, source.clone()).unwrap().mint(
                            &DeviceJoinGrantRequest {
                                principal_subject: "subject".into(), scope: scope(),
                                sandbox_id: "sandbox-a".into(), device_id: "device-a".into(),
                                lifecycle_epoch: 1, capabilities: vec![], expires_in_seconds: 900,
                            }, &cancellation,
                        ).await.err(),
                    }.expect("server rejects request");
                    assert_eq!(error.api_error().unwrap().status, status);
                    assert_eq!(captured.await.unwrap().len(), 1, "must not replay the request");
                    let rejected = source.rejected.lock();
                    assert_eq!(rejected.as_slice(), if credential_rejected { &[7][..] } else { &[][..] });
                    assert_eq!(source.generation.load(Ordering::SeqCst),
                        if replaced || credential_rejected { 8 } else { 7 });
                }
            }
        }
    }

    #[tokio::test]
    async fn directory_and_control_preserve_retry_after_from_http() {
        for directory in [true, false] {
            for status in [429, 503] {
                let date = (chrono::Utc::now() + chrono::TimeDelta::seconds(125))
                    .format("%a, %d %b %Y %H:%M:%S GMT").to_string();
                for header in ["125", date.as_str(), "18446744073709551615", "invalid", "Wed, 21 Oct 2015 07:28:00 GMT"] {
                    let (origin, captured) = mock_server_steps(vec![MockResponse::RetryAfter {
                        status, body: r#"{"error":"rate_limited","message":"try later"}"#.into(),
                        retry_after: header.into(),
                    }]).await;
                    let client = ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into()))).unwrap();
                    let before = chrono::Utc::now();
                    let error = if directory {
                        client.get_crew_session("9dd173da-fb53-48df-9581-026009c783e2", None).await.unwrap_err()
                    } else {
                        client.list_agent_accounts().await.err().expect("expected temporary authority failure")
                    };
                    let after = chrono::Utc::now();
                    assert_eq!(captured.await.unwrap().len(), 1, "API client must not replay requests");
                    let api = error.api_error().unwrap();
                    assert_eq!((api.status, api.code.as_str(), api.message.as_deref()),
                        (status, "rate_limited", Some("try later")));
                    assert!(error.is_transient());
                    if header == date {
                        let date = chrono::DateTime::parse_from_rfc2822(header).unwrap().with_timezone(&chrono::Utc);
                        let retry = error.retry_after().unwrap();
                        assert!(retry >= (date - after).to_std().unwrap_or_default());
                        assert!(retry <= (date - before).to_std().unwrap_or_default());
                    } else {
                        let expected = match header {
                            "125" => Some(Duration::from_secs(125)),
                            "18446744073709551615" => Some(Duration::from_secs(u64::MAX)),
                            "invalid" => None,
                            _ => Some(Duration::ZERO),
                        };
                        assert_eq!(error.retry_after(), expected);
                    }
                }
            }
        }
    }

    #[test]
    fn shared_transient_classification_excludes_row_conflicts_and_revocation() {
        for status in [429, 500, 502, 503, 504] {
            assert!(api_error(StatusCode::from_u16(status).unwrap(), b"{}", None).is_transient());
        }
        for status in [400, 401, 403, 404, 409, 410, 501] {
            assert!(!api_error(StatusCode::from_u16(status).unwrap(), b"{}", None).is_transient());
        }
        assert!(ScaffoldError::AuthTemporarilyUnavailable("offline".into()).is_transient());
        assert!(!ScaffoldError::AuthUnavailable.is_transient());
        assert!(!ScaffoldError::Cancelled.is_transient());
    }

    #[test]
    fn remote_agent_account_tolerates_unknown_status() {
        let account: RemoteAgentAccount = serde_json::from_value(serde_json::json!({
            "id": "account-1",
            "provider": "openai",
            "providerAccountId": "provider-1",
            "status": "suspended"
        }))
        .expect("unknown Agent Auth status");
        assert_eq!(account.status, AgentAccountStatus::Unknown);
    }

    #[test]
    fn shared_pool_usage_preserves_reset_deadline_for_ui() {
        let remote: RemoteAgentUsageWindow = serde_json::from_value(serde_json::json!({
            "label": "Five-hour",
            "usedFraction": 0.25,
            "resetAt": "2026-10-16T20:00:00Z"
        })).unwrap();
        let view: zeron_proto::AgentUsageWindow = serde_json::from_value(
            serde_json::to_value(remote).unwrap(),
        ).unwrap();
        assert_eq!(view.resets_at.map(|time| time.timestamp()), Some(1_792_180_800));
    }

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn join_grant_rejection_preserves_safe_status_and_code_without_details() {
        for (body, expected_code) in [
            (
                r#"{"error":"device_join_forbidden","message":"bearer secret","grant":"private"}"#,
                "device_join_forbidden",
            ),
            (
                r#"{"error":"Bearer private-token","message":"secret"}"#,
                "device_join_grant_rejected",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 8192];
                stream.read(&mut request).await.unwrap();
                let response = format!(
                    "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let client = EdgeDeviceJoinGrantClient::new(
                &origin,
                Arc::new(StaticToken("control-secret".into())),
            )
            .unwrap();
            let error = client
                .mint(
                    &DeviceJoinGrantRequest {
                        principal_subject: "subject".into(),
                        scope: scope(),
                        sandbox_id: "sandbox-a".into(),
                        device_id: "device-a".into(),
                        lifecycle_epoch: 1,
                        capabilities: vec![],
                        expires_in_seconds: 900,
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap_err();
            server.await.unwrap();
            assert_eq!(
                error.api_error(),
                Some(&ScaffoldApiError {
                    status: 403,
                    code: expected_code.into(),
                    message: None,
                    retry_after: None,
                })
            );
            assert!(!error.is_runtime_starting());
            assert!(!error.to_string().contains("secret"));
            assert!(!error.to_string().contains("private"));
        }
    }

    #[test]
    fn scaffold_startup_retry_requires_exact_provider_state() {
        let starting =
            br#"{"error":"sandbox_provider_error","body":{"error":"sandbox_runtime_starting"}}"#;
        let error = api_error(StatusCode::SERVICE_UNAVAILABLE, starting, None);
        assert!(error.is_runtime_starting());
        assert!(is_retryable_scaffold_control_error(&RpcError::Failed(
            error.to_string()
        )));
        for (status, body) in [
            (StatusCode::NOT_FOUND, starting.as_slice()),
            (StatusCode::SERVICE_UNAVAILABLE, br#"{"error":"sandbox_provider_error","message":"sandbox_runtime_starting"}"#.as_slice()),
            (StatusCode::SERVICE_UNAVAILABLE, br#"{"error":"sandbox_provider_error","body":{"error":"sandbox_runtime_starting_failed"}}"#.as_slice()),
        ] {
            let error = api_error(status, body, None);
            assert!(!error.is_runtime_starting());
            assert!(!is_retryable_scaffold_control_error(&RpcError::Failed(error.to_string())));
        }
    }
    #[test]
    fn scaffold_origin_requires_https_except_for_loopback() {
        let token: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
        assert!(
            ScaffoldClient::new(
                "https://scaffold.internal.ashler.com",
                "project-a",
                token.clone()
            )
            .is_ok()
        );
        assert!(ScaffoldClient::new("http://127.0.0.1:8787", "project-a", token.clone()).is_ok());
        assert!(
            ScaffoldClient::new(
                "http://scaffold.internal.ashler.com",
                "project-a",
                token.clone()
            )
            .is_err()
        );
        assert!(
            ScaffoldClient::new(
                "https://scaffold.internal.ashler.com/api",
                "project-a",
                token
            )
            .is_err()
        );
    }

    fn scope() -> CollaborationScope {
        CollaborationScope {
            project_id: "project-a".into(),
            deployment_id: Some("deployment-a".into()),
            session_id: Some("011664b5-3660-4fe6-83a2-3647fa6a2f65".into()),
            unknown: Default::default(),
        }
    }

    fn sandbox(status: &str) -> String {
        format!(
            r#"{{"sandbox":{{"id":"sandbox-a","lifecycleEpoch":1,"status":"{status}","kind":"remote_code","runtimeProfile":"remote_code","cometRuntimeProfile":{{"version":"scaffold.comet-runtime.v1","projectId":"project-a","deploymentId":"deployment-a","sessionId":"011664b5-3660-4fe6-83a2-3647fa6a2f65","sandboxId":"sandbox-a"}},"region":"us-central1","sourceRef":"387d6652abd642f0b85e8bd14f9131a9f23b7e70","ownerEmail":"alice@example.com","createdAt":"2026-08-04T00:00:00Z","updatedAt":"2026-08-04T00:01:00Z","lastActivityAt":"2026-08-04T00:02:00Z","links":{{"terminal":"https://terminal.example"}}}}}}"#
        )
    }

    fn host_authority(grant_id: &str, expires_at: i64) -> String {
        serde_json::json!({
            "ok": true,
            "exitCode": 0,
            "stdout": serde_json::json!({
                "grantId": grant_id,
                "expiresAt": expires_at,
                "principalSubject": "alice@example.com",
                "scope": scope(),
                "sandboxId": "sandbox-a",
                "deviceId": "comet-scaffold-sandbox-a-e1",
                "lifecycleEpoch": 1,
                "capabilities": scaffold_host_capabilities(),
            }).to_string(),
        })
        .to_string()
    }

    fn comet_sandbox(status: &str) -> String {
        sandbox(status).replace(
            r#""runtimeProfile":"remote_code""#,
            r#""runtimeProfile":"comet_remote""#,
        )
    }

    #[test]
    fn accepts_the_additive_comet_remote_profile_from_code_sandboxes() {
        let envelope: SandboxEnvelope = serde_json::from_str(&comet_sandbox("ready")).unwrap();
        let environment = envelope.sandbox.into_environment(scope()).unwrap();
        assert_eq!(
            environment.source_ref.as_deref(),
            Some("387d6652abd642f0b85e8bd14f9131a9f23b7e70")
        );
        assert_eq!(
            environment.scope.session_id.as_deref(),
            Some("011664b5-3660-4fe6-83a2-3647fa6a2f65")
        );
    }

    #[test]
    fn accepts_forge_agent_with_comet_remote_profile() {
        let sandbox =
            comet_sandbox("ready").replace(r#""kind":"remote_code""#, r#""kind":"agent""#);
        let envelope: SandboxEnvelope = serde_json::from_str(&sandbox).unwrap();
        assert!(envelope.sandbox.into_environment(scope()).is_ok());
    }

    #[test]
    fn release_marker_does_not_reinterpret_persisted_runtime_profile_version() {
        assert_eq!(SCAFFOLD_AUTHORITY_PROFILE_WIRE_VERSION, "scaffold.comet-runtime.v1");
        assert_ne!(SCAFFOLD_COMET_RUNTIME_VERSION, SCAFFOLD_AUTHORITY_PROFILE_WIRE_VERSION);
        let envelope: SandboxEnvelope = serde_json::from_str(&comet_sandbox("ready")).unwrap();
        assert!(envelope.sandbox.into_environment(scope()).is_ok());
        let mut envelope: SandboxEnvelope = serde_json::from_str(&comet_sandbox("ready")).unwrap();
        envelope.sandbox.comet_runtime_profile.as_mut().unwrap().version = "scaffold.comet-runtime.v2".into();
        assert!(matches!(envelope.sandbox.into_environment(scope()), Err(ScaffoldError::InvalidResponse(_))));
    }

    #[tokio::test]
    async fn explicit_start_requires_server_transition_not_a_larger_sandbox_row() {
        for scenario in ["verified", "missing_transition", "different_session", "wrong_epoch"] {
            let mut body: Value = serde_json::from_str(&comet_sandbox("starting")).unwrap();
            body["sandbox"]["lifecycleEpoch"] = serde_json::json!(2);
            if scenario != "missing_transition" {
                body["lifecycleTransition"] = serde_json::json!({
                    "action":"start", "previousLifecycleEpoch":1, "currentLifecycleEpoch":2,
                    "logicalSessionId":scope().session_id, "sandboxId":"sandbox-a",
                    "projectId":"project-a", "deploymentId":"deployment-a", "ownerSubject":"owner-subject"
                });
                if scenario == "different_session" { body["lifecycleTransition"]["logicalSessionId"] = serde_json::json!("other"); }
                if scenario == "wrong_epoch" { body["lifecycleTransition"]["currentLifecycleEpoch"] = serde_json::json!(3); }
            }
            let (origin, captured) = mock_server(vec![body.to_string()]).await;
            let client = ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into()))).unwrap();
            let result = client.start("sandbox-a", &scope(), 1, &CancellationToken::new()).await;
            assert_eq!(result.is_ok(), scenario == "verified", "{scenario}: {result:?}");
            let requests = captured.await.unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].starts_with("POST /api/code-sandboxes/sandbox-a/start HTTP"));
            assert!(requests[0].contains(r#""expectedLifecycleEpoch":1"#));
        }
    }

    enum MockResponse {
        Http { status: u16, body: String },
        RetryAfter { status: u16, body: String, retry_after: String },
        Close,
        HeadersThenClose,
        CancelThenClose(CancellationToken),
    }

    #[tokio::test]
    async fn antigravity_model_catalog_uses_authorized_metadata_and_rejects_invalid_ids() {
        let valid = serde_json::json!({"sourceAccountId":"owned-google","defaultModelId":"gemini-4-argon-medium","models":[
            {"id":"gemini-4-argon-high","displayName":"Gemini 4 Argon (High)","contextWindow":1048576},
            {"id":"gemini-4-argon-medium","displayName":"Gemini 4 Argon (Medium)","contextWindow":1048576}
        ]});
        let mut invalid = valid.clone();
        invalid["models"][0]["id"] = serde_json::json!("gemini/../../other");
        let (origin, server) = mock_server(vec![valid.to_string(), invalid.to_string()]).await;
        let client = ScaffoldClient::new(&origin, "project", Arc::new(zeron_rpc::StaticToken("owner-session".into()))).unwrap();
        let models = client.list_antigravity_models(None).await.unwrap().models;
        assert_eq!(models[0].id, "gemini-4-argon-medium");
        assert_eq!(models[0].label, "Gemini 4 Argon (Medium)");
        assert!(client.list_antigravity_models(None).await.is_err());
        let requests = server.await.unwrap();
        assert!(requests.iter().all(|request| request.starts_with("GET /api/agent-accounts/models?provider=antigravity ")));
        assert!(requests.iter().all(|request| request.to_ascii_lowercase().contains("authorization: bearer owner-session")));
    }

    async fn mock_server(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        mock_server_steps(
            responses
                .into_iter()
                .map(|body| MockResponse::Http { status: 200, body })
                .collect(),
        )
        .await
    }

    async fn mock_server_with_status(
        responses: Vec<(u16, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        mock_server_steps(
            responses
                .into_iter()
                .map(|(status, body)| MockResponse::Http { status, body })
                .collect(),
        )
        .await
    }

    #[tokio::test]
    async fn original_epoch_bootstrap_requires_the_exact_evidence_envelope() {
        for (body, accepted) in [
            (serde_json::json!({"originalEpochHistory": null}), true),
            (serde_json::json!({"originalEpochHistory": {"proofJson":"signed-proof","commandOutcomes":{"request-a":"completed"}}}), true),
            (serde_json::json!({}), false),
            (serde_json::json!({"originalEpochHistory": {"proofJson":"signed-proof"}}), false),
            (serde_json::json!({"originalEpochHistory": {"proofJson":"signed-proof","commandOutcomes":{},"rollback":true}}), false),
            (serde_json::json!({"originalEpochHistory": null,"rollback":true}), false),
        ] {
            let (origin, captured) = mock_server(vec![body.to_string()]).await;
            let client = ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into()))).unwrap();
            let result = client.original_epoch_history("sandbox-a", &scope(), &CancellationToken::new()).await;
            assert_eq!(result.is_ok(), accepted);
            let requests = captured.await.unwrap();
            assert_eq!(requests.len(), 1);
            assert!(requests[0].starts_with("GET /api/code-sandboxes/sandbox-a/original-epoch-history "));
        }
    }

    #[tokio::test]
    async fn original_epoch_bootstrap_preserves_authentication_generation_and_errors() {
        use std::sync::atomic::{AtomicU64, Ordering};
        for (status, code, rejected) in [
            (401, "request_rejected", true),
            (403, "request_rejected", false),
            (403, "remote_code_token_forbidden", true),
            (403, "crew_device_authority_unavailable", true),
            (403, "crew_device_scope_denied", true),
            (503, "crew_device_authority_unavailable", false),
            (503, "remote_code_token_validation_failed", false),
            (429, "request_rejected", false),
        ] {
            for replaced in [false, true] {
                let source = Arc::new(ReplacingCredential {
                    generation: AtomicU64::new(7), rejected: parking_lot::Mutex::new(Vec::new()),
                });
                let server_source = source.clone();
                let (origin, captured) = mock_server_for_requests(move |request, _| {
                    assert!(request.to_ascii_lowercase().contains("authorization: bearer test-credential-7\r\n"));
                    if replaced { server_source.generation.store(8, Ordering::SeqCst); }
                    (MockResponse::RetryAfter {
                        status, body: serde_json::json!({"error": code}).to_string(), retry_after: "9".into(),
                    }, true)
                }).await;
                let client = ScaffoldClient::new(&origin, "project-a", source.clone()).unwrap();
                let error = client.original_epoch_history("sandbox-a", &scope(), &CancellationToken::new()).await.err().unwrap();
                let api = error.api_error().unwrap();
                assert_eq!(api.status, status);
                assert_eq!(api.code, code);
                assert_eq!(error.retry_after(), Some(Duration::from_secs(9)));
                assert_eq!(captured.await.unwrap().len(), 1, "bootstrap must not replay");
                assert_eq!(source.rejected.lock().as_slice(), if rejected { &[7][..] } else { &[][..] });
                assert_eq!(source.generation.load(Ordering::SeqCst), if replaced || rejected { 8 } else { 7 });
            }
        }
    }

    // A raw peer exercises decoded chunk accounting without relying on Content-Length.
    async fn original_epoch_body_server(
        body: String, chunked: bool, complete: bool,
    ) -> (String, tokio::task::JoinHandle<()>, tokio::sync::oneshot::Receiver<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (sent, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut buffer = [0; 1024];
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                assert!(request.len() < 8192);
            }
            let framing = if chunked {
                "transfer-encoding: chunked".to_owned()
            } else {
                // An oversized hint must not cause rejection before observed bytes exceed the limit.
                format!("content-length: {}", if complete { body.len() } else { 4 * 1024 * 1024 + 1 })
            };
            stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n{framing}\r\nconnection: close\r\n\r\n").as_bytes()).await.unwrap();
            for chunk in body.as_bytes().chunks(16 * 1024) {
                if chunked {
                    stream.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await.unwrap();
                }
                stream.write_all(chunk).await.unwrap();
                if chunked && stream.write_all(b"\r\n").await.is_err() { return; }
            }
            // The client may close immediately on observing the first over-limit byte.
            if chunked && complete { let _ = stream.write_all(b"0\r\n\r\n").await; }
            let _ = sent.send(());
            if !complete {
                let mut byte = [0];
                while matches!(stream.read(&mut byte).await, Ok(1)) {}
            }
        });
        (origin, server, received)
    }

    #[tokio::test]
    async fn original_epoch_bootstrap_body_accepts_limit_and_rejects_limit_plus_one() {
        for chunked in [false, true] {
            for extra in [0, 1] {
                let mut body = r#"{"originalEpochHistory":null}"#.to_owned();
                body.extend(std::iter::repeat_n(' ', 4 * 1024 * 1024 + extra - body.len()));
                let (origin, server, _) = original_epoch_body_server(body, chunked, true).await;
                let client = ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into()))).unwrap();
                let result = client.original_epoch_history("sandbox-a", &scope(), &CancellationToken::new()).await;
                if extra == 0 {
                    assert!(matches!(result, Ok(None)), "exactly 4 MiB must be accepted");
                } else {
                    assert!(matches!(result, Err(ScaffoldError::InvalidResponse(message))
                        if message == "original epoch bootstrap response exceeds 4 MiB"));
                }
                server.await.unwrap();
            }
        }
    }

    #[tokio::test]
    async fn original_epoch_bootstrap_outcomes_accept_limit_and_reject_limit_plus_one() {
        for count in [10_000, 10_001] {
            let outcomes: BTreeMap<_, _> = (0..count).map(|index| (format!("request-{index}"), "completed")).collect();
            let body = serde_json::json!({"originalEpochHistory": {
                "proofJson":"signed-proof", "commandOutcomes":outcomes,
            }}).to_string();
            let (origin, captured) = mock_server(vec![body]).await;
            let client = ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into()))).unwrap();
            let result = client.original_epoch_history("sandbox-a", &scope(), &CancellationToken::new()).await;
            if count == 10_000 {
                let evidence = result.ok().flatten().expect("complete bootstrap evidence at the entry limit");
                assert_eq!(evidence.proof_json, "signed-proof");
                assert_eq!(evidence.command_outcomes.len(), count);
                assert!(outcomes.iter().all(|(id, outcome)| evidence.command_outcomes.get(id).map(String::as_str) == Some(*outcome)));
            } else {
                assert!(matches!(result, Err(ScaffoldError::InvalidResponse(message))
                    if message == "original epoch bootstrap exceeds 10000 command outcomes"));
            }
            assert_eq!(captured.await.unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn original_epoch_bootstrap_incomplete_body_is_cancellable_and_has_total_deadline() {
        for chunked in [false, true] {
            for cancel in [false, true] {
                let (origin, server, body_sent) = original_epoch_body_server("{".into(), chunked, false).await;
                let source = Arc::new(ReplacingCredential {
                    generation: std::sync::atomic::AtomicU64::new(7), rejected: parking_lot::Mutex::new(Vec::new()),
                });
                let client = ScaffoldClient::new(&origin, "project-a", source.clone()).unwrap();
                let cancellation = CancellationToken::new();
                let cancelled = cancellation.clone();
                let request = tokio::spawn(async move {
                    client.original_epoch_history("sandbox-a", &scope(), &cancelled).await
                });
                body_sent.await.unwrap();
                // Let the response headers reach the client before advancing the body deadline.
                tokio::time::sleep(Duration::from_millis(20)).await;
                assert!(!request.is_finished(), "Content-Length is not an observed body size");
                if cancel {
                    cancellation.cancel();
                } else {
                    tokio::time::pause();
                    tokio::time::advance(REQUEST_TIMEOUT + Duration::from_secs(1)).await;
                }
                let result = tokio::time::timeout(Duration::from_secs(1), request).await.unwrap().unwrap();
                if !cancel { tokio::time::resume(); }
                server.abort();
                if cancel {
                    assert!(matches!(result, Err(ScaffoldError::Cancelled)));
                } else {
                    assert!(matches!(result, Err(ScaffoldError::Transport {
                        cause: ScaffoldTransportCause::RequestTimeout, before_response: false, ..
                    })));
                }
                assert!(source.rejected.lock().is_empty());
            }
        }
    }

    #[tokio::test]
    async fn original_epoch_bootstrap_transport_failures_do_not_revoke_credentials() {
        for response in [MockResponse::Close, MockResponse::HeadersThenClose] {
            let source = Arc::new(ReplacingCredential {
                generation: std::sync::atomic::AtomicU64::new(7), rejected: parking_lot::Mutex::new(Vec::new()),
            });
            let (origin, captured) = mock_server_steps(vec![response]).await;
            let client = ScaffoldClient::new(&origin, "project-a", source.clone()).unwrap();
            let error = client.original_epoch_history("sandbox-a", &scope(), &CancellationToken::new()).await.err().unwrap();
            assert!(matches!(error, ScaffoldError::Transport { .. }));
            assert!(source.rejected.lock().is_empty());
            assert_eq!(captured.await.unwrap().len(), 1);
        }
    }



    #[tokio::test]
    async fn attachment_observes_current_native_authority_while_application_starts() {
        for (authority_epoch, expected_grant) in [(1, Some("native-grant")), (2, None)] {
            let authority = host_authority("native-grant", now_ms() + 60_000)
                .replace("sandbox-a-e1", &format!("sandbox-a-e{authority_epoch}"))
                .replace("\\\"lifecycleEpoch\\\":1", &format!("\\\"lifecycleEpoch\\\":{authority_epoch}"));
            let (origin, server) = mock_server(vec![authority]).await;
            let runtime = test_runtime(
                ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into()))).unwrap(),
                "https://edge.example", Arc::new(UnavailableDeviceJoinGrantProvider),
            );
            let envelope: SandboxEnvelope = serde_json::from_str(&comet_sandbox("starting")).unwrap();
            let observed = runtime.observe_attachment(envelope.sandbox.into_environment(scope()).unwrap(), &CancellationToken::new()).await.unwrap();
            assert_eq!(observed.and_then(|result| result.control_grant).map(|grant| grant.id).as_deref(), expected_grant);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn terminal_attach_is_a_known_rejection_before_runtime_mutation() {
        for (status, lifecycle) in [
            ("failed", zeron_proto::ScaffoldLifecycle::Failed),
            ("stopped", zeron_proto::ScaffoldLifecycle::Stopped),
        ] {
            let (origin, captured) = mock_server_with_status(vec![(200, comet_sandbox(status))]).await;
            let runtime = ScaffoldRuntime::new(
                ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into()))).unwrap(),
                "https://edge.example", Arc::new(UnavailableDeviceJoinGrantProvider),
            );
            let error = runtime.control(ScaffoldEnvironmentControl::Attach {
                sandbox_id: "sandbox-a".into(), scope: scope(),
            }, &CancellationToken::new()).await.unwrap_err();
            assert!(matches!(error, ScaffoldError::AttachRejected(state) if state == lifecycle));
            captured.await.unwrap();
        }
    }

    fn test_runtime(client: ScaffoldClient, edge: &str, grants: Arc<dyn DeviceJoinGrantProvider>) -> ScaffoldRuntime {
        ScaffoldRuntime::new(client, edge, grants)
    }

    async fn mock_server_steps(
        responses: Vec<MockResponse>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let mut responses = std::collections::VecDeque::from(responses);
        mock_server_for_requests(move |_, _| {
            let response = responses.pop_front().unwrap();
            (response, responses.is_empty())
        }).await
    }

    async fn mock_server_for_requests(
        mut respond: impl FnMut(&str, &Value) -> (MockResponse, bool) + Send + 'static,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            let mut finished = false;
            while !finished {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0_u8; 4096];
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    if let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let content_length = String::from_utf8_lossy(&bytes[..header_end])
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= header_end + 4 + content_length {
                            break;
                        }
                    }
                }
                // Cancelled sockets and discovery probes are not API requests.
                if bytes.is_empty() { continue; }
                let request = String::from_utf8(bytes).unwrap();
                if request.lines().next() == Some("HEAD / HTTP/1.1") {
                    stream.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.unwrap();
                    continue;
                }
                let body = request.split("\r\n\r\n").nth(1).unwrap_or("");
                let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
                let (response, last) = respond(&request, &value);
                finished = last;
                requests.push(request);
                match response {
                    MockResponse::Close => continue,
                    MockResponse::CancelThenClose(cancellation) => {
                        cancellation.cancel();
                        continue;
                    }
                    MockResponse::HeadersThenClose => {
                        stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1\r\nconnection: close\r\n\r\n",
                            )
                            .await
                            .unwrap();
                        continue;
                    }
                    MockResponse::RetryAfter { status, body, retry_after } => {
                        let response = format!(
                            "HTTP/1.1 {status} Response\r\nRetry-After: {retry_after}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len(),
                        );
                        stream.write_all(response.as_bytes()).await.unwrap();
                    }
                    MockResponse::Http { status, body } => {
                        let response = format!(
                            "HTTP/1.1 {status} Response\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body,
                        );
                        stream.write_all(response.as_bytes()).await.unwrap();
                    }
                }
            }
            requests
        });
        (origin, task)
    }

    fn test_inference_authority() -> AgentInferenceAuthority {
        AgentInferenceAuthority {
            contract_version: 2,
            token: "remote-authority".into(),
            token_type: "Bearer".into(),
            authority_id: "authority-1".into(),
            principal_id: "identity:owner-1".into(),
            authority_scope: Some("user:identity:owner-1".into()),
            expires_at: "2099-01-01T00:00:00Z".into(),
        }
    }

    #[tokio::test]
    async fn lifecycle_deadline_does_not_relax_metadata_reads() {
        for lifecycle in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut chunk = [0_u8; 1024];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                    assert!(request.len() < 8192);
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
                let body = sandbox("resuming");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
            let mut client = ScaffoldClient::new(
                origin,
                "project-a",
                Arc::new(StaticToken("owner-scoped-bearer".into())),
            )
            .unwrap();
            client.http = scaffold_http_client(Some(Duration::from_millis(50))).unwrap();
            let cancellation = CancellationToken::new();
            let result = if lifecycle {
                client.resume("sandbox-a", &scope(), &cancellation).await
            } else {
                client.inspect("sandbox-a", &scope(), &cancellation).await
            };
            if lifecycle {
                let environment = result.unwrap();
                assert_eq!(environment.owner_principal, "alice@example.com");
                assert_eq!(environment.scope, scope());
                assert!(matches!(
                    environment.source,
                    SessionEnvironmentSource::Scaffold {
                        lifecycle: zeron_proto::ScaffoldLifecycle::Resuming,
                        ..
                    }
                ));
            } else {
                assert_eq!(
                    result.unwrap_err().transport_cause(),
                    Some(ScaffoldTransportCause::RequestTimeout)
                );
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn lifecycle_cancellation_interrupts_an_incomplete_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let (headers_sent, headers_received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let mut chunk = [0_u8; 1024];
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                assert!(request.len() < 8192);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1000\r\nconnection: close\r\n\r\n{")
                .await
                .unwrap();
            headers_sent.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let client = ScaffoldClient::new(
            origin,
            "project-a",
            Arc::new(StaticToken("owner-scoped-bearer".into())),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let cancelled = cancellation.clone();
        let request =
            tokio::spawn(async move { client.resume("sandbox-a", &scope(), &cancelled).await });
        headers_received.await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancellation.cancel();
        let result = tokio::time::timeout(Duration::from_secs(1), request).await;
        server.abort();
        assert!(matches!(
            result.unwrap().unwrap(),
            Err(ScaffoldError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn archive_upload_outlives_control_request_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\nx") {
                let mut chunk = [0_u8; 1024];
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
                assert!(request.len() < 8192);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await
                .unwrap();
        });
        let mut client =
            ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("owner".into())))
                .unwrap();
        client.http = scaffold_http_client(Some(Duration::from_millis(50))).unwrap();
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, b"x").unwrap();
        let grant = UploadGrant {
            url: origin,
            token: "upload-token".into(),
            token_env: "UPLOAD_TOKEN".into(),
            destination_path: ".".into(),
            expires_at: i64::MAX,
            _command: String::new(),
        };
        client
            .upload_granted_file(&grant, file.reopen().unwrap(), 1, &CancellationToken::new())
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn inference_response_outlives_the_control_plane_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 4096];
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&chunk[..read]);
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let content_length = String::from_utf8_lossy(&request[..header_end])
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            stream.write_all(b"9\r\ndata: 1\n\n\r\n").await.unwrap();
            tokio::time::sleep(REQUEST_TIMEOUT + Duration::from_secs(1)).await;
            stream
                .write_all(b"9\r\ndata: 2\n\n\r\n0\r\n\r\n")
                .await
                .unwrap();
        });

        let client = ScaffoldClient::new(
            origin,
            "project-a",
            Arc::new(StaticToken("owner-scoped-bearer".into())),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let authority = test_inference_authority();
        let response = client
            .proxy_agent_inference(AgentInferenceProxyRequest {
                harness: None,
                bound_model: None,
                endpoint: "responses",
                query: None,
                authority: &authority,
                conversation_id: "session-1",
                requested_account_id: None,
                request_id: "request-1",
                headers: reqwest::header::HeaderMap::new(),
                content_length: 2,
                body: reqwest::Body::from("{}"),
                cancellation: &cancellation,
            })
            .await
            .unwrap();

        assert_eq!(response.text().await.unwrap(), "data: 1\n\ndata: 2\n\n");
    }

    #[tokio::test]
    async fn agent_auth_uses_the_canonical_v2_contract_by_default() {
        let authority = serde_json::json!({
            "contractVersion": 2,
            "token": "v2-authority-token",
            "tokenType": "Bearer",
            "authorityId": "authority-1",
            "principalId": "identity:owner-1",
            "authorityScope": "user:identity:owner-1",
            "expiresAt": "2099-01-01T00:00:00Z"
        })
        .to_string();
        let (origin, captured) = mock_server(vec![authority, r#"{"ok":true}"#.into()]).await;
        let client = ScaffoldClient::new(
            origin,
            "project-a",
            Arc::new(StaticToken("owner-scoped-bearer".into())),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let authority = client
            .issue_agent_inference_authority(&cancellation)
            .await
            .unwrap();

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", "local-loopback-token".parse().unwrap());
        headers.insert(
            "x-agent-auth-account-id",
            "spoofed-account".parse().unwrap(),
        );
        let response = client
            .proxy_agent_inference(AgentInferenceProxyRequest {
                harness: None,
                bound_model: None,
                endpoint: "messages",
                query: Some("beta=true"),
                authority: &authority,
                conversation_id: "conversation-1",
                requested_account_id: Some("account-1"),
                request_id: "request-1",
                headers,
                content_length: 2,
                body: reqwest::Body::from("{}"),
                cancellation: &cancellation,
            })
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let requests = captured.await.unwrap();
        let authority_request = requests[0].to_ascii_lowercase();
        assert!(authority_request.starts_with("post /api/agent-auth/v2/authority http/1.1"));
        assert!(authority_request.contains("content-length: 2"));
        assert!(authority_request.ends_with("\r\n\r\n{}"));
        let inference = requests[1].to_ascii_lowercase();
        assert!(inference.starts_with("post /api/agent-auth/v2/messages?beta=true http/1.1"));
        assert!(inference.contains("authorization: bearer v2-authority-token"));
        assert!(inference.contains("x-agent-auth-authority-scope: user:identity:owner-1"));
        assert!(inference.contains("x-agent-auth-conversation-id: conversation-1"));
        assert!(inference.contains("x-agent-auth-account-id: account-1"));
        assert!(inference.contains("x-agent-auth-request-id: request-1"));
        assert!(!inference.contains("local-loopback-token"));
        assert!(!inference.contains("x-agent-auth-owner-subject"));
    }

    #[tokio::test]
    async fn agent_auth_accepts_unscoped_v2_authority_during_rollout() {
        let authority = serde_json::json!({
            "contractVersion": 2,
            "token": "legacy-v2-authority-token",
            "tokenType": "Bearer",
            "authorityId": "authority-1",
            "principalId": "identity:owner-1",
            "expiresAt": "2099-01-01T00:00:00Z"
        })
        .to_string();
        let (origin, captured) = mock_server(vec![authority, r#"{"ok":true}"#.into()]).await;
        let client = ScaffoldClient::new(
            origin,
            "project-a",
            Arc::new(StaticToken("owner-scoped-bearer".into())),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let authority = client
            .issue_agent_inference_authority(&cancellation)
            .await
            .unwrap();

        let response = client
            .proxy_agent_inference(AgentInferenceProxyRequest {
                harness: None,
                bound_model: None,
                endpoint: "messages",
                query: None,
                authority: &authority,
                conversation_id: "conversation-1",
                requested_account_id: None,
                request_id: "request-1",
                headers: reqwest::header::HeaderMap::new(),
                content_length: 2,
                body: reqwest::Body::from("{}"),
                cancellation: &cancellation,
            })
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let requests = captured.await.unwrap();
        let inference = requests[1].to_ascii_lowercase();
        assert!(!inference.contains("x-agent-auth-authority-scope:"));
    }

    #[tokio::test]
    async fn route_receipt_returns_attribution_without_credentials() {
        let response = serde_json::json!({
            "logicalSessionId": "session-a",
            "routingMode": "pinned",
            "requestedAccountId": "opaque-account-id",
            "route": {
                "provider": "openai",
                "model": "gpt-5.6-sol",
                "backend": "oauth",
                "accountId": "opaque-account-id",
                "accountGeneration": 3,
                "createdAt": "2026-08-10T00:00:00Z",
                "updatedAt": "2026-08-10T00:01:00Z",
                "expiresAt": "2026-08-10T01:00:00Z"
            },
            "grant": {
                "id": "grant-id",
                "provider": "openai",
                "model": "gpt-5.6-sol",
                "harness": "codex",
                "source": "comet-local",
                "lifecycleEpoch": 1,
                "environment": "local",
                "routingMode": "pinned",
                "requestedAccountId": "opaque-account-id",
                "backend": "oauth",
                "accountId": "opaque-account-id",
                "accountGeneration": 3,
                "createdAt": "2026-08-10T00:00:00Z",
                "expiresAt": "2026-08-10T00:05:00Z",
                "revokedAt": null
            }
        })
        .to_string();
        let (origin, captured) = mock_server(vec![response]).await;
        let client = ScaffoldClient::new(
            origin,
            "project-a",
            Arc::new(StaticToken("owner-scoped-bearer".into())),
        )
        .unwrap();
        let receipt = client
            .get_agent_route_receipt("session-a", &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(receipt.logical_session_id, "session-a");
        assert_eq!(
            receipt.route.account_id.as_deref(),
            Some("opaque-account-id")
        );
        assert_eq!(receipt.grant.account_generation, Some(3));
        let value = serde_json::to_value(receipt).unwrap();
        assert!(value.get("token").is_none());
        assert!(value.get("ownerSubject").is_none());

        let requests = captured.await.unwrap();
        assert!(requests[0].starts_with("GET /api/agent-auth/routes/session-a HTTP/1.1"));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("authorization: bearer owner-scoped-bearer")
        );
    }

    #[tokio::test]
    async fn control_plane_methods_preserve_v1_authority_wire_and_lifecycle_contract() {
        let listed_sandbox: Value = serde_json::from_str(&sandbox("paused")).unwrap();
        let listed = serde_json::json!({
            "ok": true,
            "sandboxes": [listed_sandbox["sandbox"].clone()],
        })
        .to_string();
        let responses = vec![
            listed,
            sandbox("ready"),
            comet_sandbox("creating"),
            sandbox("paused"),
            sandbox("resuming"),
            sandbox("ready"),
            sandbox("stopped"),
            sandbox("ready").replace("scaffold.comet-runtime.v1", "scaffold.comet-runtime.v2"),
        ];
        let (origin, captured) = mock_server(responses).await;
        let client = ScaffoldClient::new(
            &origin,
            "project-a",
            Arc::new(StaticToken("sc_rc_control_secret".into())),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        assert_eq!(client.list(&scope(), &cancellation).await.unwrap().len(), 1);
        client
            .inspect("sandbox-a", &scope(), &cancellation)
            .await
            .unwrap();
        let created = client
            .create(
                &scope(),
                CreateSandboxOptions {
                    name: Some("Crew"),
                    source_ref: Some("main"),
                    region: Some("us-central1"),
                    database_environment: ScaffoldDatabaseEnvironment::StagingSnapshot,
                },
                &AgentRoute::automatic(zeron_proto::AgentProvider::OpenAi, "gpt-5.6-sol"),
                &cancellation,
            )
            .await
            .unwrap();
        assert_eq!(
            created.scope.session_id.as_deref(),
            Some("011664b5-3660-4fe6-83a2-3647fa6a2f65")
        );
        client
            .pause("sandbox-a", &scope(), &cancellation)
            .await
            .unwrap();
        client
            .resume("sandbox-a", &scope(), &cancellation)
            .await
            .unwrap();
        client
            .update_agent_route(
                "sandbox-a",
                &scope(),
                &AgentRoute::automatic(zeron_proto::AgentProvider::OpenAi, "gpt-5.5"),
                &cancellation,
            )
            .await
            .unwrap();
        let stopped = client
            .stop("sandbox-a", &scope(), &cancellation)
            .await
            .unwrap();
        assert!(matches!(
            stopped.source,
            SessionEnvironmentSource::Scaffold {
                lifecycle: zeron_proto::ScaffoldLifecycle::Stopped,
                ..
            }
        ));
        assert!(matches!(
            client.inspect("sandbox-a", &scope(), &cancellation).await,
            Err(ScaffoldError::InvalidResponse(_))
        ));

        let requests = captured.await.unwrap();
        let request_lines: Vec<_> = requests
            .iter()
            .map(|request| request.lines().next().unwrap())
            .collect();
        assert_eq!(
            request_lines,
            [
                "GET /api/code-sandboxes HTTP/1.1",
                "GET /api/code-sandboxes/sandbox-a HTTP/1.1",
                "POST /api/code-sandboxes HTTP/1.1",
                "POST /api/code-sandboxes/sandbox-a/pause HTTP/1.1",
                "POST /api/code-sandboxes/sandbox-a/resume HTTP/1.1",
                "POST /api/code-sandboxes/sandbox-a/agent-route HTTP/1.1",
                "POST /api/code-sandboxes/sandbox-a/stop HTTP/1.1",
                "GET /api/code-sandboxes/sandbox-a HTTP/1.1",
            ]
        );
        assert!(requests.iter().all(|request| {
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer sc_rc_control_secret")
        }));
        assert!(requests[2].contains(r#""databaseEnvironment":"staging_snapshot""#));
        assert!(requests[2].contains(
            r#""agentRoute":{"provider":"openai","model":"gpt-5.6-sol","fallback":"disabled","routingMode":"automatic"}"#
        ));
        assert!(requests[5].contains(
            r#""agentRoute":{"provider":"openai","model":"gpt-5.5","fallback":"disabled","routingMode":"automatic"}"#
        ));
        let create_body: Value = serde_json::from_str(requests[2].split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(create_body["cometRuntimeProfile"], serde_json::json!({
            "version": "scaffold.comet-runtime.v1",
            "projectId": "project-a",
            "deploymentId": "deployment-a",
            "sessionId": "011664b5-3660-4fe6-83a2-3647fa6a2f65",
        }));
    }

    #[test]
    fn create_sandbox_body_serializes_a_pinned_opaque_account() {
        let route = AgentRoute::pinned(
            zeron_proto::AgentProvider::Anthropic,
            "claude-opus-5",
            "opaque-account-id",
        );
        let body = CreateSandboxBody {
            name: None,
            source: None,
            region: None,
            database_environment: ScaffoldDatabaseEnvironment::Local,
            agent_route: &route,
            comet_runtime_profile: CreateCometRuntimeProfile {
                version: SCAFFOLD_AUTHORITY_PROFILE_WIRE_VERSION,
                project_id: "project-a",
                deployment_id: "deployment-a",
                session_id: "session-a",
            },
        };
        assert_eq!(
            serde_json::to_value(body).unwrap()["agentRoute"],
            serde_json::json!({
                "provider": "anthropic",
                "model": "claude-opus-5",
                "fallback": "disabled",
                "routingMode": "pinned",
                "accountId": "opaque-account-id",
            })
        );
    }
    #[tokio::test]
    async fn resume_requires_current_native_authority_and_recovers_without_replay() {
        use crate::{auth::{Auth, AuthConfig}, crew::CrewServices};
        use zeron_sync::{DocsStore, scaffold_auth::{save_session, ScaffoldConfig,
            ScaffoldIdentity, ScaffoldSession, DEFAULT_OAUTH_SCOPES}};

        let auth_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let auth_origin = format!("http://{}", auth_listener.local_addr().unwrap());
        let resource = auth_origin.clone();
        let auth_server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = auth_listener.accept().await.unwrap();
                let mut bytes = [0_u8; 8192];
                let size = stream.read(&mut bytes).await.unwrap();
                let request = String::from_utf8_lossy(&bytes[..size]);
                let body = if request.starts_with("GET /auth/session ") {
                    serde_json::json!({"userId":"alice", "email":"alice@example.com",
                        "projectScope":"project-a", "deploymentId":"deployment-a",
                        "credential":"scaffold", "capabilities":["session.read"]})
                } else {
                    assert!(request.starts_with("GET /api/code-sandboxes/auth/session "));
                    serde_json::json!({"ok":true, "resource":resource,
                        "actor":{"sub":"alice", "auth":"iap"},
                        "scopes":DEFAULT_OAUTH_SCOPES.split_whitespace().collect::<Vec<_>>()})
                }.to_string();
                stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        });
        let directory = tempfile::tempdir().unwrap();
        save_session(&directory.path().join("scaffold-session.json"), &ScaffoldSession {
            access_token: "sc_rc_test".into(), resource: auth_origin.clone(), edge_origin: auth_origin.clone(), device: None,
            identity: ScaffoldIdentity { user_id: "alice".into(), email: "alice@example.com".into(), name: None,
                project_scope: "project-a".into(), deployment_id: Some("deployment-a".into()), capabilities: vec!["session.read".into()] },
        }).unwrap();
        let mut config = AuthConfig::new(&auth_origin, directory.path());
        config.scaffold = Some(ScaffoldConfig::new(&auth_origin, &auth_origin, "project-a"));
        let auth = Auth::detect(config).await;
        assert!(auth.state().is_signed_in());
        auth_server.await.unwrap();

        let grant_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let expires_at = now_ms() + 60_000;
        let authority = host_authority(grant_id, expires_at)
            .replace("sandbox-a-e1", "sandbox-a-e3")
            .replace(r#"\"lifecycleEpoch\":1"#, r#"\"lifecycleEpoch\":3"#);
        let current = |status| comet_sandbox(status).replace("\"lifecycleEpoch\":1", "\"lifecycleEpoch\":3");
        let unavailable = r#"{"ok":false,"exitCode":1}"#.to_string();
        let (origin, captured) = mock_server(vec![
            comet_sandbox("paused"), // Controller validates the old binding.
            comet_sandbox("paused"), // Native attachment validates the resume transition.
            current("resuming"),
            unavailable.clone(),
            format!(r#"{{"grant":"cg1.{grant_id}.secret","expiresAt":{expires_at}}}"#),
            r#"{"ok":true}"#.into(),
            r#"{"ok":true,"exitCode":0}"#.into(),
            r#"{"ok":true,"runId":"resumed-host","supervised":true}"#.into(),
            authority.clone(),
            r#"{"ok":true,"exitCode":0}"#.into(), // Cleanup after confirmed attachment.
            current("ready"),
            unavailable.clone(), // Provider readiness alone must not confirm Resume.
            current("ready"),
            host_authority(grant_id, expires_at), // Old epoch is not live authority.
            current("ready"),
            authority.replace(grant_id, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .replace(&expires_at.to_string(), "0"), // Current epoch with an expired grant is not authority.
            current("ready"),
            authority.clone(),
            current("failed"),
            current("stopped"),
            current("failed"),
            current("failed"),
            current("stopped"),
            current("stopped"),
            current("paused"),
            current("paused"),
            r#"{"ok":true}"#.into(), // Resume delivery is unknown after a malformed reply.
            current("ready"),
            unavailable,
            current("ready"),
            authority,
        ]).await;
        let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("control-token".into()));
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
            &origin, Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
        ).with_deployment_id("deployment-a".into());
        let store = Arc::new(DocsStore::open(&directory.path().join("store")).unwrap());
        let services = CrewServices::new(auth, runtime, store.clone()).unwrap();
        let cancellation = CancellationToken::new();
        let control = ScaffoldEnvironmentControl::Resume { sandbox_id: "sandbox-a".into(), scope: scope() };
        let first = services.control("resume-once", control.clone(), &cancellation).await.unwrap();
        assert_eq!(first.status, "unknown");
        assert!(first.result.is_none());
        assert_eq!(services.control("resume-once", control, &cancellation).await.unwrap().status, "unknown");
        assert_eq!(services.reconcile("resume-once", &cancellation).await.unwrap().status, "unknown");
        assert_eq!(services.reconcile("resume-once", &cancellation).await.unwrap().status, "unknown");
        let confirmed = services.reconcile("resume-once", &cancellation).await.unwrap();
        assert_eq!(confirmed.status, "confirmed");
        let result = confirmed.result.unwrap();
        assert_eq!(result.attached_device_id.as_deref(), Some("comet-scaffold-sandbox-a-e3"));
        assert_eq!(result.control_grant.unwrap().id, grant_id);
        assert_eq!(scaffold_source(&result.environment).unwrap().1, Some(3));
        assert_eq!(result.room_projection.unwrap().session_id, scope().session_id.unwrap());

        // Pending resumes also retire on terminal provider observations, without launching.
        let (request, _) = store.crew_command("resume-once").unwrap().unwrap();
        for id in ["resume-failed", "resume-stopped"] {
            assert!(store.claim_crew_command(id, &request).unwrap());
            assert_eq!(services.reconcile(id, &cancellation).await.unwrap().status, "rejected");
            assert_eq!(services.outcome(id).unwrap().status, "rejected");
        }
        for id in ["direct-resume-failed", "direct-resume-stopped"] {
            let control = ScaffoldEnvironmentControl::Resume { sandbox_id: "sandbox-a".into(), scope: scope() };
            assert_eq!(services.control(id, control, &cancellation).await.unwrap().status, "rejected");
        }
        let control = ScaffoldEnvironmentControl::Resume { sandbox_id: "sandbox-a".into(), scope: scope() };
        assert_eq!(services.control("resume-ambiguous", control.clone(), &cancellation).await.unwrap().status, "unknown");
        assert_eq!(services.control("resume-ambiguous", control, &cancellation).await.unwrap().status, "unknown");
        assert_eq!(services.reconcile("resume-ambiguous", &cancellation).await.unwrap().status, "unknown");
        let recovered = services.reconcile("resume-ambiguous", &cancellation).await.unwrap();
        assert_eq!(recovered.status, "confirmed");
        assert_eq!(recovered.result.unwrap().attached_device_id.as_deref(), Some("comet-scaffold-sandbox-a-e3"));
        let requests = captured.await.unwrap();
        assert_eq!(requests.iter().filter(|request| request.starts_with("POST /api/code-sandboxes/sandbox-a/resume ")).count(), 2);
        assert_eq!(requests.iter().filter(|request| request.contains(r#""mode":"background""#)).count(), 1);
        assert_eq!(requests.iter().filter(|request| request.starts_with("POST /auth/device-grants ")).count(), 1);
        assert!(!requests.iter().any(|request| request.starts_with("POST /api/code-sandboxes HTTP")));
    }

    #[tokio::test]
    async fn attach_resumes_existing_sandbox_and_uses_new_epoch_authority() {
        let grant_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let authority = host_authority(grant_id, now_ms() + 60_000)
            .replace("sandbox-a-e1", "sandbox-a-e3")
            .replace(r#"\"lifecycleEpoch\":1"#, r#"\"lifecycleEpoch\":3"#);
        let current =
            |status| comet_sandbox(status).replace("\"lifecycleEpoch\":1", "\"lifecycleEpoch\":3");
        let (origin, captured) = mock_server_with_status(vec![
            (200, comet_sandbox("paused")),
            (200, current("resuming")),
            (503, r#"{"error":"sandbox_runtime_starting"}"#.into()),
            (200, current("starting")),
            (200, authority.clone()),
            (200, current("ready")),
            (200, authority),
        ])
        .await;
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into())))
                .unwrap(),
            "https://comet-edge.example",
            Arc::new(UnavailableDeviceJoinGrantProvider),
        );
        for _ in 0..2 {
            let result = runtime
                .control(
                    ScaffoldEnvironmentControl::Attach {
                        sandbox_id: "sandbox-a".into(),
                        scope: scope(),
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!(
                result.attached_device_id.as_deref(),
                Some("comet-scaffold-sandbox-a-e3")
            );
            assert_eq!(result.environment.scope, scope());
            assert_eq!(scaffold_source(&result.environment).unwrap().1, Some(3));
            assert_eq!(
                result.room_projection.unwrap().session_id,
                scope().session_id.unwrap()
            );
            assert_eq!(result.control_grant.unwrap().id, grant_id);
            assert!(result.run_id.is_none());
        }
        let requests = captured.await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("POST /api/code-sandboxes/sandbox-a/resume "))
                .count(),
            1
        );
        assert!(!requests.iter().any(|request| {
            request.starts_with("POST /api/code-sandboxes HTTP")
                || request.contains("/auth/device-grants")
                || request.contains("--device-bootstrap-file")
        }));
    }

    #[tokio::test]
    async fn attach_retries_closed_authority_probe_without_recreating() {
        let (origin, captured) = mock_server_steps(vec![
            MockResponse::Http {
                status: 200,
                body: comet_sandbox("starting"),
            },
            MockResponse::Close,
            MockResponse::Http {
                status: 200,
                body: comet_sandbox("starting"),
            },
            MockResponse::Http {
                status: 200,
                body: host_authority("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", now_ms() + 60_000),
            },
        ])
        .await;
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into())))
                .unwrap(),
            "https://comet-edge.example",
            Arc::new(UnavailableDeviceJoinGrantProvider),
        );
        let result = runtime
            .control(
                ScaffoldEnvironmentControl::Attach {
                    sandbox_id: "sandbox-a".into(),
                    scope: scope(),
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.attached_device_id.as_deref(),
            Some("comet-scaffold-sandbox-a-e1")
        );
        let requests = tokio::time::timeout(Duration::from_secs(2), captured)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.starts_with("POST /api/code-sandboxes HTTP"))
                .count(),
            0
        );
        assert_eq!(
            requests
                .iter()
                .filter(|request| {
                    request.starts_with("POST /api/code-sandboxes/sandbox-a/exec ")
                })
                .count(),
            2
        );
        assert!(!requests.iter().any(|request| {
            request.contains("/auth/device-grants") || request.contains("--device-bootstrap-file")
        }));
    }

    #[tokio::test]
    async fn attach_does_not_retry_nontransient_authority_probe_failure() {
        let (origin, captured) = mock_server_with_status(vec![
            (200, comet_sandbox("starting")),
            (403, r#"{"error":"sandbox_profile_mismatch"}"#.into()),
        ])
        .await;
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into())))
                .unwrap(),
            "https://comet-edge.example",
            Arc::new(UnavailableDeviceJoinGrantProvider),
        );
        let error = runtime
            .control(
                ScaffoldEnvironmentControl::Attach {
                    sandbox_id: "sandbox-a".into(),
                    scope: scope(),
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.api_error().unwrap().status, 403);
        assert_eq!(captured.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn attach_does_not_retry_reset_after_response_headers() {
        let (origin, captured) = mock_server_steps(vec![
            MockResponse::Http {
                status: 200,
                body: comet_sandbox("starting"),
            },
            MockResponse::HeadersThenClose,
        ])
        .await;
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into())))
                .unwrap(),
            "https://comet-edge.example",
            Arc::new(UnavailableDeviceJoinGrantProvider),
        );
        let error = runtime
            .control(
                ScaffoldEnvironmentControl::Attach {
                    sandbox_id: "sandbox-a".into(),
                    scope: scope(),
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ScaffoldError::Transport {
                cause: ScaffoldTransportCause::ConnectionResetOrClosed,
                before_response: false,
                ..
            }
        ));
        assert_eq!(captured.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn attach_starts_host_before_application_ready_and_keeps_credentials_private() {
        let join_expires_at = now_ms() + 60_000;
        let access_expires_at =
            join_expires_at - i64::from(JOIN_GRANT_TTL_SECONDS) * 1000 + DEVICE_ACCESS_TTL_MS;
        let responses = vec![
            sandbox("starting"),
            r#"{"ok":false,"exitCode":1}"#.into(),
            format!(
                r#"{{"grant":"cg1.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narrow_secret","expiresAt":{join_expires_at}}}"#
            ),
            r#"{"ok":true}"#.into(),
            r#"{"ok":true,"exitCode":0}"#.into(),
            r#"{"ok":true,"runId":"run-a","supervised":true}"#.into(),
            host_authority("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", access_expires_at),
            r#"{"ok":true,"exitCode":0}"#.into(), // Cleanup after confirmed attachment.
        ];
        let (origin, captured) = mock_server(responses).await;
        let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("sc_rc_broad_secret".into()));
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
            "https://comet-edge.example",
            Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
        );
        let result = runtime
            .control(
                ScaffoldEnvironmentControl::Attach {
                    sandbox_id: "sandbox-a".into(),
                    scope: scope(),
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(result.environment.source, SessionEnvironmentSource::Scaffold {
            lifecycle: zeron_proto::ScaffoldLifecycle::Starting, lifecycle_epoch: Some(1), ..
        }));
        assert_eq!(result.environment.scope, scope());
        assert_eq!(result.control_grant.as_ref().unwrap().id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(
            result.attached_device_id.as_deref(),
            Some("comet-scaffold-sandbox-a-e1")
        );
        assert_eq!(result.run_id.as_deref(), Some("run-a"));
        assert_eq!(result.environment.owner_principal, "alice@example.com");
        assert_eq!(
            result.room_projection,
            Some(SessionRoomProjection {
                project_id: "project-a".into(),
                deployment_id: "deployment-a".into(),
                session_id: "011664b5-3660-4fe6-83a2-3647fa6a2f65".into(),
            })
        );
        let serialized_result = serde_json::to_string(&result).unwrap();
        assert!(serialized_result.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert!(!serialized_result.contains("narrow_secret"));

        let requests = captured.await.unwrap();
        let exec_bodies: Vec<_> = requests
            .iter()
            .filter(|request| request.starts_with("POST /api/code-sandboxes/sandbox-a/exec "))
            .map(|request| request.split_once("\r\n\r\n").unwrap().1)
            .collect();
        assert!(
            exec_bodies
                .iter()
                .all(|body| !body.contains("sc_rc_broad_secret") && !body.contains("cg1."))
        );
        assert!(exec_bodies[0].contains(r#""scaffold-authority""#));
        assert!(exec_bodies[1].contains(r#""chmod","600""#));
        let launch: Value = serde_json::from_str(exec_bodies[2]).unwrap();
        let bootstrap_path = launch["argv"][7].as_str().unwrap();
        assert!(bootstrap_path.starts_with(".comet-device-bootstrap-") && bootstrap_path.ends_with(".json"));
        assert!(!bootstrap_path.contains('/') && !bootstrap_path.contains('\\'));
        assert_eq!(launch, serde_json::json!({
            "argv": [".scaffold/crew/bin/comet", "--data-dir", ".scaffold/crew-data", "--ipc-port", "39400",
                "headless", "--device-bootstrap-file", bootstrap_path, "--edge-url", "https://comet-edge.example",
                "--scaffold-url", origin.as_str(), "--project-scope", "project-a", "--deployment-id", "deployment-a"],
            "mode": "background", "timeoutMs": 0
        }));
        let bootstrap_request = requests
            .iter()
            .find(|request| request.starts_with("PUT /api/code-sandboxes/sandbox-a/files?"))
            .expect("bootstrap file upload");
        let bootstrap_body = bootstrap_request.split_once("\r\n\r\n").unwrap().1;
        let bootstrap_envelope: Value = serde_json::from_str(bootstrap_body).unwrap();
        let bootstrap: Value =
            serde_json::from_str(bootstrap_envelope["content"].as_str().unwrap()).unwrap();
        assert_eq!(
            bootstrap["deviceJoinGrant"],
            "cg1.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narrow_secret"
        );
        assert_eq!(bootstrap["deploymentId"], "deployment-a");
        assert_eq!(bootstrap["lifecycleEpoch"], 1);
        let grant_body = requests[2].split_once("\r\n\r\n").unwrap().1;
        assert!(grant_body.contains(r#""sandboxId":"sandbox-a""#));
        assert!(grant_body.contains(r#""deploymentId":"deployment-a""#));
        assert!(grant_body.contains(r#""lifecycleEpoch":1"#));
    }

    async fn assert_bootstrap_cleanup_after_attempt(stage: &str) {
        let cancellation = CancellationToken::new();
        let expires_at = now_ms() + 60_000;
        let reply = |body: String| MockResponse::Http { status: 200, body };
        let mut responses = vec![
            reply(sandbox("ready")),
            reply(r#"{"ok":false,"exitCode":1}"#.into()),
            reply(format!(r#"{{"grant":"cg1.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narrow_secret","expiresAt":{expires_at}}}"#)),
        ];
        match stage {
            "uncertain-upload" => responses.push(MockResponse::HeadersThenClose),
            "cancelled-upload" => responses.push(MockResponse::CancelThenClose(cancellation.clone())),
            _ => {
                responses.extend([
                    reply(r#"{"ok":true}"#.into()),
                    reply(r#"{"ok":true,"exitCode":0}"#.into()),
                    reply(r#"{"ok":true,"runId":"existing-supervised-run","supervised":true}"#.into()),
                ]);
                responses.push(match stage {
                    "cancelled-authority" => MockResponse::CancelThenClose(cancellation.clone()),
                    "invalid-authority" => reply(host_authority("existing-grant", now_ms() - 1)),
                    "reused-host" => reply(host_authority("existing-grant", expires_at)),
                    _ => panic!("unknown bootstrap cleanup scenario"),
                });
            }
        }
        // Cleanup failure must not replace the original outcome or trigger launch replay.
        responses.push(MockResponse::Http { status: 503, body: r#"{"error":"cleanup_unavailable"}"#.into() });
        let (origin, mut captured) = mock_server_steps(responses).await;
        let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
            "https://comet-edge.example", Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
        );
        let result = tokio::time::timeout(Duration::from_secs(12), runtime.control(
            ScaffoldEnvironmentControl::Attach { sandbox_id: "sandbox-a".into(), scope: scope() }, &cancellation,
        )).await;
        if result.is_err() {
            captured.abort();
            panic!("{stage}: cleanup did not preserve a bounded completion");
        }
        let result = result.unwrap();
        match stage {
            "uncertain-upload" => assert!(matches!(result, Err(ScaffoldError::Transport { .. }))),
            "cancelled-upload" | "cancelled-authority" => assert!(matches!(result, Err(ScaffoldError::Cancelled))),
            "invalid-authority" => assert!(matches!(result, Err(ScaffoldError::InvalidResponse(_)))),
            "reused-host" => {
                let result = result.unwrap();
                assert_eq!(result.run_id.as_deref(), Some("existing-supervised-run"));
                assert_eq!(result.control_grant.unwrap().id, "existing-grant");
            }
            _ => unreachable!(),
        }
        let requests = match tokio::time::timeout(Duration::from_secs(1), &mut captured).await {
            Ok(requests) => requests.unwrap(),
            Err(_) => {
                captured.abort();
                panic!("{stage}: no independent cleanup attempt after bootstrap upload attempt");
            }
        };
        let upload = requests.iter().find(|request| request.starts_with("PUT ")).unwrap();
        let target = upload.split_whitespace().nth(1).unwrap();
        let path = Url::parse(&format!("http://fixture{target}")).unwrap()
            .query_pairs().find(|(key, _)| key == "path").unwrap().1.into_owned();
        let execs: Vec<Value> = requests.iter()
            .filter(|request| request.starts_with("POST /api/code-sandboxes/sandbox-a/exec "))
            .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
            .collect();
        let removals: Vec<_> = execs.iter().filter(|body| body["argv"][0] == "rm").collect();
        assert_eq!(removals.len(), 1, "{stage}: cleanup must target the attempted upload once");
        assert_eq!(removals[0]["argv"], serde_json::json!(["rm", "-f", "--", path]));
        assert_eq!(removals[0]["mode"], "inline");
        assert!((1..=10_000).contains(&removals[0]["timeoutMs"].as_u64().unwrap()));
        let expected_launches = usize::from(!stage.ends_with("upload"));
        assert_eq!(execs.iter().filter(|body| body["mode"] == "background").count(), expected_launches);
        assert_eq!(requests.iter().filter(|request| request.contains("/auth/device-grants ")).count(), 1);
        assert!(execs.iter().all(|body| !body.to_string().contains("cg1.")));
        assert!(!execs.iter().any(|body| matches!(body["argv"][0].as_str(), Some("kill" | "pkill" | "killall"))));
    }

    #[tokio::test]
    async fn bootstrap_cleanup_uncertain_upload() {
        assert_bootstrap_cleanup_after_attempt("uncertain-upload").await;
    }

    #[tokio::test]
    async fn bootstrap_cleanup_cancelled_upload() {
        assert_bootstrap_cleanup_after_attempt("cancelled-upload").await;
    }

    #[tokio::test]
    async fn bootstrap_cleanup_cancelled_authority() {
        // Cancel in-flight authority while the HTTP fixture remains available for
        // independent bootstrap cleanup and no-launch-replay assertions.
        assert_bootstrap_cleanup_after_attempt("cancelled-authority").await;
    }

    #[tokio::test]
    async fn bootstrap_cleanup_invalid_authority() {
        assert_bootstrap_cleanup_after_attempt("invalid-authority").await;
    }

    #[tokio::test]
    async fn bootstrap_cleanup_reused_host() {
        assert_bootstrap_cleanup_after_attempt("reused-host").await;
    }

    #[tokio::test]
    async fn rejected_native_admission_cleans_bootstrap_without_launch_replay() {
        for (status, rejection) in [
            (200, serde_json::json!({"ok": false, "error": "exec_spawn_failed"})),
            (409, serde_json::json!({"error": "crew_runtime_invalid"})),
        ] {
            let expires_at = now_ms() + 60_000;
            let (origin, captured) = mock_server_with_status(vec![
                (200, sandbox("ready")),
                (200, r#"{"ok":false,"exitCode":1}"#.into()),
                (200, format!(r#"{{"grant":"cg1.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narrow_secret","expiresAt":{expires_at}}}"#)),
                (200, r#"{"ok":true}"#.into()),
                (200, r#"{"ok":true,"exitCode":0}"#.into()),
                (status, rejection.to_string()),
                (200, r#"{"ok":true,"exitCode":0}"#.into()),
            ]).await;
            let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
            let runtime = test_runtime(
                ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
                "https://comet-edge.example", Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
            );
            let error = runtime.control(ScaffoldEnvironmentControl::Attach {
                sandbox_id: "sandbox-a".into(), scope: scope(),
            }, &CancellationToken::new()).await.unwrap_err();
            if status == 200 {
                assert!(matches!(error, ScaffoldError::CometNotInstalled));
            } else {
                let api = error.api_error().unwrap();
                assert_eq!((api.status, api.code.as_str()), (409, "crew_runtime_invalid"));
            }
            assert!(!error.to_string().contains("narrow_secret"));
            let requests = captured.await.unwrap();
            let execs: Vec<Value> = requests.iter()
                .filter(|request| request.starts_with("POST /api/code-sandboxes/sandbox-a/exec "))
                .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
                .collect();
            assert_eq!(execs.len(), 4); // Authority, private chmod, one admission, cleanup.
            assert_eq!(execs.iter().filter(|body| body["mode"] == "background").count(), 1);
            assert_eq!(execs[3]["argv"], serde_json::json!(["rm", "-f", "--", execs[2]["argv"][7]]));
            assert!(execs.iter().all(|body| !body.to_string().contains("cg1.")));
            assert_eq!(requests.iter().filter(|request| request.contains("/auth/device-grants ")).count(), 1);
        }
    }

    #[tokio::test]
    async fn attach_rejects_unsupervised_or_unidentified_launch_without_replay() {
        for receipt in [
            serde_json::json!({"ok": true, "runId": "generic-exec"}),
            serde_json::json!({"ok": true, "runId": "generic-exec", "supervised": false}),
            serde_json::json!({"ok": true, "supervised": true}),
            serde_json::json!({"ok": true, "runId": "", "supervised": true}),
            serde_json::json!({"ok": true, "runId": "  ", "supervised": true}),
        ] {
            let expires_at = now_ms() + 60_000;
            let (origin, captured) = mock_server(vec![
                sandbox("ready"),
                r#"{"ok":false,"exitCode":1}"#.into(),
                format!(r#"{{"grant":"cg1.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narrow_secret","expiresAt":{expires_at}}}"#),
                r#"{"ok":true}"#.into(),
                r#"{"ok":true,"exitCode":0}"#.into(),
                receipt.to_string(),
                r#"{"ok":true,"exitCode":0}"#.into(), // Remove any unconsumed bootstrap.
            ]).await;
            let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
            let runtime = test_runtime(
                ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
                "https://comet-edge.example", Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
            );
            let result = runtime.control(ScaffoldEnvironmentControl::Attach {
                sandbox_id: "sandbox-a".into(), scope: scope(),
            }, &CancellationToken::new()).await;
            if receipt["supervised"] == true {
                assert!(matches!(result, Err(ScaffoldError::InvalidResponse(_))));
            } else {
                assert!(matches!(result, Err(ScaffoldError::RuntimeContractMismatch)));
            }
            let requests = captured.await.unwrap();
            assert_eq!(requests.iter().filter(|request| request.contains(r#""mode":"background""#)).count(), 1);
            assert_eq!(requests.iter().filter(|request| request.contains("/auth/device-grants ")).count(), 1);
            assert!(!requests.iter().any(|request| request.contains("/uploads")));
            let execs: Vec<Value> = requests.iter()
                .filter(|request| request.starts_with("POST /api/code-sandboxes/sandbox-a/exec "))
                .map(|request| serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap())
                .collect();
            assert_eq!(execs.len(), 4);
            assert_eq!(execs[3]["argv"], serde_json::json!(["rm", "-f", "--", execs[2]["argv"][7]]));
            assert!(execs.iter().all(|body| !body.to_string().contains("cg1.")));
        }
    }

    #[tokio::test]
    async fn attach_never_returns_an_unconfirmed_bootstrap_grant() {
        let expires_at = now_ms() + 60_000;
        for confirmation in [
            host_authority("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", expires_at),
            r#"{"ok":true,"exitCode":0}"#.into(),
            host_authority("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", now_ms() - 1),
        ] {
            let expected_success =
                confirmation == host_authority("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", expires_at);
            let (origin, captured) = mock_server(vec![
                sandbox("ready"),
                r#"{"ok":false,"exitCode":1}"#.into(),
                format!(r#"{{"grant":"cg1.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narrow_secret","expiresAt":{expires_at}}}"#),
                r#"{"ok":true}"#.into(),
                r#"{"ok":true,"exitCode":0}"#.into(),
                r#"{"ok":true,"runId":"run-a","supervised":true}"#.into(),
                confirmation,
                r#"{"ok":true,"exitCode":0}"#.into(), // Cleanup preserves confirmation outcome.
            ]).await;
            let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
            let runtime = test_runtime(
                ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
                "https://comet-edge.example",
                Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
            );
            let result = runtime
                .control(
                    ScaffoldEnvironmentControl::Attach {
                        sandbox_id: "sandbox-a".into(),
                        scope: scope(),
                    },
                    &CancellationToken::new(),
                )
                .await;
            if expected_success {
                assert_eq!(
                    result.unwrap().control_grant.unwrap().id,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                );
            } else {
                assert!(matches!(result, Err(ScaffoldError::InvalidResponse(_))));
            }
            captured.await.unwrap();
        }
    }

    #[tokio::test]
    async fn attach_waits_for_explicit_runtime_starting_before_bootstrap() {
        let join_expires_at = now_ms() + 60_000;
        let mut authority_probes = 0;
        let mut inspections = 0;
        let mut launched = false;
        let mut confirmation_probes = 0;
        let (origin, captured) = mock_server_for_requests(move |request, body| {
            let mut finished = false;
            let (status, response) = if request.starts_with("GET /api/code-sandboxes/sandbox-a ") {
                inspections += 1;
                (200, comet_sandbox("starting"))
            } else if request.starts_with("POST /auth/device-grants ") {
                assert!(authority_probes >= 2 && inspections >= 2);
                assert!(!launched);
                (200, format!(r#"{{"grant":"cg1.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.narrow_secret","expiresAt":{join_expires_at}}}"#))
            } else if request.starts_with("PUT /api/code-sandboxes/sandbox-a/files?") {
                (200, r#"{"ok":true}"#.into())
            } else {
                assert!(request.starts_with("POST /api/code-sandboxes/sandbox-a/exec "));
                let argv = body["argv"].as_array().unwrap();
                if argv.iter().any(|arg| arg == "scaffold-authority") {
                    authority_probes += 1;
                    if authority_probes == 1 {
                        (503, r#"{"error":"sandbox_provider_error","body":{"error":"sandbox_runtime_starting","sessionId":"sandbox-a","lifecycleEpoch":1,"status":"starting"}}"#.into())
                    } else if !launched {
                        (200, r#"{"ok":false,"exitCode":1}"#.into())
                    } else {
                        confirmation_probes += 1;
                        if confirmation_probes < 3 {
                            (200, r#"{"ok":false,"exitCode":1,"stderr":"scaffold host unavailable"}"#.into())
                        } else {
                            (200, host_authority("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", join_expires_at))
                        }
                    }
                } else if argv.iter().any(|arg| arg == "--device-bootstrap-file") {
                    assert!(!launched);
                    launched = true;
                    (200, r#"{"ok":true,"runId":"run-a","supervised":true}"#.into())
                } else if argv.first().and_then(Value::as_str) == Some("rm") {
                    assert!(launched && confirmation_probes >= 3);
                    finished = true;
                    (200, r#"{"ok":true,"exitCode":0}"#.into())
                } else {
                    assert!(argv.iter().any(|arg| arg == "--version")
                        || (argv[0] == "chmod" && argv[1] == "600"));
                    (200, r#"{"ok":true,"exitCode":0}"#.into())
                }
            };
            (MockResponse::Http { status, body: response }, finished)
        }).await;
        let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
            "https://comet-edge.example",
            Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
        );
        let result = runtime
            .control(
                ScaffoldEnvironmentControl::Attach {
                    sandbox_id: "sandbox-a".into(),
                    scope: scope(),
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            result.attached_device_id.as_deref(),
            Some("comet-scaffold-sandbox-a-e1")
        );
        assert_eq!(
            result.room_projection.unwrap().session_id,
            scope().session_id.unwrap()
        );
        assert_eq!(result.run_id.as_deref(), Some("run-a"));
        let requests = captured.await.unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.contains("POST /auth/device-grants "))
                .count(),
            1
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| r.contains("--device-bootstrap-file"))
                .count(),
            1
        );
        assert!(
            !requests
                .iter()
                .any(|r| r.starts_with("POST /api/code-sandboxes HTTP"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn authority_probe_outlives_the_old_startup_deadline() {
        let mut authority_probes = 0;
        let (origin, captured) = mock_server_for_requests(move |request, body| {
            let (status, response, finished) = if request.starts_with("GET /api/code-sandboxes/sandbox-a ") {
                (200, comet_sandbox("starting"), false)
            } else {
                assert!(
                    request.starts_with("POST /api/code-sandboxes/sandbox-a/exec "),
                    "unexpected fixture request line: {:?}",
                    request.lines().next().unwrap_or("")
                );
                assert!(body["argv"].as_array().unwrap().iter().any(|arg| arg == "scaffold-authority"));
                authority_probes += 1;
                if authority_probes <= 241 {
                    (503, r#"{"error":"sandbox_runtime_starting"}"#.into(), false)
                } else {
                    (200, host_authority("native-grant", now_ms() + 60_000), true)
                }
            };
            (MockResponse::Http { status, body: response }, finished)
        }).await;
        // A listener-discovery probe must not steal a scripted API response.
        let discovery = reqwest::Client::new().head(&origin).send().await.unwrap();
        assert_eq!(discovery.status(), StatusCode::NOT_FOUND);
        let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", bearer).unwrap(),
            "https://comet-edge.example",
            Arc::new(UnavailableDeviceJoinGrantProvider),
        );
        let envelope: SandboxEnvelope = serde_json::from_str(&comet_sandbox("starting")).unwrap();
        let environment = envelope.sandbox.into_environment(scope()).unwrap();
        // Keep yielding for loopback I/O instead of letting paused time jump
        // straight to an in-flight request's 30-second HTTP timeout.
        let clock = tokio::spawn(async {
            loop {
                tokio::task::yield_now().await;
                tokio::time::advance(Duration::from_millis(1)).await;
            }
        });
        let started = tokio::time::Instant::now();
        let result = runtime
            .observe_attachment(environment, &CancellationToken::new())
            .await;
        clock.abort();
        let _ = clock.await;
        let attachment = result.unwrap().unwrap();
        assert_eq!(attachment.attached_device_id.as_deref(), Some("comet-scaffold-sandbox-a-e1"));
        assert_eq!(attachment.control_grant.unwrap().id, "native-grant");
        assert!(started.elapsed() > Duration::from_secs(120));
        let requests = captured.await.unwrap();
        assert_eq!(requests.iter().filter(|request| request.starts_with("GET /api/code-sandboxes/sandbox-a ")).count(), 241);
        assert!(!requests.iter().any(|request| {
            request.contains("/auth/device-grants")
                || request.starts_with("POST /api/code-sandboxes HTTP")
                || request.contains("--device-bootstrap-file")
        }));
    }

    #[tokio::test]
    async fn attach_runtime_wait_fails_closed_on_missing_and_terminal_lifecycle() {
        for (status, code, terminal) in [
            (404, "sandbox_provider_error", None),
            (403, "sandbox_profile_mismatch", None),
            (404, "sandbox_runtime_starting", None),
            (503, "scaffold_request_rejected", None),
            (503, "sandbox_runtime_starting", Some("failed")),
            (503, "sandbox_runtime_starting", Some("paused")),
            (503, "sandbox_runtime_starting", Some("stopped")),
            (
                503,
                "sandbox_runtime_starting",
                Some("unknown_future_state"),
            ),
        ] {
            let mut responses = vec![
                (200, comet_sandbox("starting")),
                (status, serde_json::json!({"error":code}).to_string()),
            ];
            if let Some(lifecycle) = terminal {
                responses.push((200, comet_sandbox(lifecycle)));
            }
            let expected_requests = responses.len();
            let (origin, captured) = mock_server_with_status(responses).await;
            let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
            let runtime = test_runtime(
                ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
                "https://comet-edge.example",
                Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
            );
            let error = runtime
                .control(
                    ScaffoldEnvironmentControl::Attach {
                        sandbox_id: "sandbox-a".into(),
                        scope: scope(),
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap_err();
            if terminal.is_some() {
                assert!(matches!(error, ScaffoldError::InvalidResponse(_)));
            } else {
                let api = error.api_error().unwrap();
                assert_eq!((api.status, api.code.as_str()), (status, code));
            }
            assert_eq!(captured.await.unwrap().len(), expected_requests);
        }
    }

    #[tokio::test]
    async fn runtime_starting_cannot_extend_a_different_lifecycle_authority() {
        for (field, value) in [
            ("id", serde_json::json!("another-sandbox")),
            ("lifecycleEpoch", serde_json::json!(2)),
            ("ownerEmail", serde_json::json!("another-owner@example.com")),
        ] {
            let original: Value = serde_json::from_str(&comet_sandbox("starting")).unwrap();
            let mut changed = original.clone();
            changed["sandbox"][field] = value;
            let (origin, captured) = mock_server_with_status(vec![
                (503, r#"{"error":"sandbox_runtime_starting"}"#.into()),
                (200, changed.to_string()),
            ])
            .await;
            let runtime = test_runtime(
                ScaffoldClient::new(&origin, "project-a", Arc::new(StaticToken("token".into())))
                    .unwrap(),
                "https://comet-edge.example",
                Arc::new(UnavailableDeviceJoinGrantProvider),
            );
            let envelope: SandboxEnvelope = serde_json::from_value(original).unwrap();
            let environment = envelope.sandbox.into_environment(scope()).unwrap();
            let error = runtime
                .probe_attach_authority(
                    "sandbox-a",
                    &scope(),
                    &environment,
                    &["comet".into(), "scaffold-authority".into()],
                    &CancellationToken::new(),
                )
                .await
                .unwrap_err();
            assert!(matches!(error, ScaffoldError::InvalidResponse(_)));
            let requests = captured.await.unwrap();
            assert!(!requests.iter().any(|request| {
                request.contains("/auth/device-grants")
                    || request.starts_with("POST /api/code-sandboxes HTTP")
            }));
        }
    }

    #[tokio::test]
    async fn attach_runtime_wait_is_cancellable_before_credentials_are_minted() {
        let (origin, captured) = mock_server_with_status(vec![
            (200, comet_sandbox("starting")),
            (503, r#"{"error":"sandbox_provider_error","body":{"error":"sandbox_runtime_starting","sessionId":"sandbox-a","lifecycleEpoch":1,"status":"starting"}}"#.into()),
            (200, comet_sandbox("starting")),
        ]).await;
        let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("token".into()));
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
            "https://comet-edge.example",
            Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
        );
        let cancellation = CancellationToken::new();
        let attach = runtime.control(
            ScaffoldEnvironmentControl::Attach {
                sandbox_id: "sandbox-a".into(),
                scope: scope(),
            },
            &cancellation,
        );
        let cancel = async {
            let requests = captured.await.unwrap();
            cancellation.cancel();
            requests
        };
        let (result, requests) = tokio::join!(attach, cancel);
        assert!(matches!(result, Err(ScaffoldError::Cancelled)));
        assert_eq!(requests.len(), 3);
        assert!(!requests.iter().any(|r| r.contains("/auth/device-grants")));
    }

    #[tokio::test]
    async fn repeated_attach_reuses_matching_scaffold_host_authority() {
        let expires_at = now_ms() + 60_000;
        let responses = vec![
            sandbox("ready"),
            serde_json::json!({
                "ok": true,
                "exitCode": 0,
                "stdout": serde_json::json!({
                    "grantId": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "expiresAt": expires_at,
                    "principalSubject": "alice@example.com",
                    "scope": scope(),
                    "sandboxId": "sandbox-a",
                    "deviceId": "comet-scaffold-sandbox-a-e1",
                    "lifecycleEpoch": 1,
                    "capabilities": scaffold_host_capabilities(),
                })
                .to_string(),
            })
            .to_string(),
        ];
        let (origin, captured) = mock_server(responses).await;
        let bearer: Arc<dyn TokenSource> = Arc::new(StaticToken("sc_rc_broad_secret".into()));
        let runtime = test_runtime(
            ScaffoldClient::new(&origin, "project-a", bearer.clone()).unwrap(),
            "https://comet-edge.example",
            Arc::new(EdgeDeviceJoinGrantClient::new(&origin, bearer).unwrap()),
        );

        let result = runtime
            .control(
                ScaffoldEnvironmentControl::Attach {
                    sandbox_id: "sandbox-a".into(),
                    scope: scope(),
                },
                &CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.run_id, None);
        assert_eq!(
            result.control_grant,
            Some(ScaffoldControlGrant {
                id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                expires_at,
                capabilities: scaffold_host_capabilities(),
            })
        );
        let requests = captured.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains(r#""scaffold-authority""#));
        assert!(
            !requests
                .iter()
                .any(|request| request.contains("/auth/device-grants"))
        );
        assert!(
            !requests
                .iter()
                .any(|request| request.contains("--device-bootstrap-file"))
        );
    }

    #[tokio::test]
    async fn scaffold_handoffs_share_one_process_memory_permit() {
        let runtime = test_runtime(
            ScaffoldClient::new(
                "http://127.0.0.1:1",
                "project-a",
                Arc::new(StaticToken("unused".into())),
            )
            .unwrap(),
            "https://comet-edge.example",
            Arc::new(UnavailableDeviceJoinGrantProvider),
        );
        let first = runtime
            .inner
            .handoff_permits
            .clone()
            .acquire_owned()
            .await
            .unwrap();
        assert!(runtime.inner.handoff_permits.try_acquire().is_err());
        drop(first);
        assert!(runtime.inner.handoff_permits.try_acquire().is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn worktree_handoff_streams_a_manifest_bound_archive() {
        let repo = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "crew@example.com"],
            vec!["config", "user.name", "Crew"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(repo.path())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::fs::write(repo.path().join("tracked.txt"), "base\n").unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["add", "."])
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .args(["commit", "-qm", "base"])
                .current_dir(repo.path())
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(repo.path().join("tracked.txt"), "changed\n").unwrap();
        let snapshot = crate::worktree_handoff::capture_worktree_handoff(repo.path()).unwrap();
        let mut expected_archive = Vec::new();
        std::io::Read::read_to_end(&mut snapshot.reopen().unwrap(), &mut expected_archive).unwrap();
        for failure in [false, true] {

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let upload_url = format!("{origin}/worktree-upload");
        let verified_stdout = format!(
            "verified:{}:{SCAFFOLD_HANDOFF_CWD}\n",
            snapshot.manifest_sha256
        );
        let responses = [
            r#"{"ok":true,"exitCode":0}"#.to_string(),
            serde_json::json!({
                "ok": true,
                "upload": {
                    "url": upload_url,
                    "token": "worktree_upload_secret",
                    "tokenEnv": "SCAFFOLD_UPLOAD_TOKEN",
                    "destinationPath": ".scaffold/crew-handoff-staging",
                    "expiresAt": chrono::Utc::now().checked_add_signed(chrono::Duration::minutes(1)).unwrap().to_rfc3339(),
                    "command": "hidden"
                }
            })
            .to_string(),
            "{}".to_string(),
            "{}".to_string(),
            if failure {
                serde_json::json!({"ok":false,"exitCode":45,"stderr":format!("crew_repository:tree_entry_unsupported\n{}", "λ".repeat(4096))}).to_string()
            } else {
                serde_json::json!({"ok": true, "exitCode": 0, "stdout": verified_stdout}).to_string()
            },
            r#"{"ok":true,"exitCode":0}"#.to_string(),
        ];
        let captured = tokio::spawn(async move {
            let mut requests = Vec::new();
            for body in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut chunk = [0_u8; 4096];
                    let read = stream.read(&mut chunk).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..read]);
                    if let Some(header_end) =
                        bytes.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        let length = String::from_utf8_lossy(&bytes[..header_end])
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= header_end + 4 + length {
                            break;
                        }
                    }
                }
                // Match the real provider contract that rejected root-cwd handoffs.
                let header_end = bytes
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .unwrap();
                let payload = serde_json::from_slice::<Value>(&bytes[header_end + 4..]);
                if payload
                    .as_ref()
                    .ok()
                    .and_then(|value| value.get("argv"))
                    .and_then(Value::as_array)
                    .is_some_and(|argv| {
                        argv.is_empty()
                            || argv.len() > 64
                            || argv
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::len)
                                .sum::<usize>()
                                > 16_384
                            || argv
                                .iter()
                                .any(|arg| arg.as_str().is_none_or(str::is_empty))
                    })
                {
                    stream.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.unwrap();
                    panic!(
                        "Scaffold requires 1–64 nonempty exec arguments totaling at most 16 KiB"
                    );
                }
                requests.push(bytes);
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        let client = ScaffoldClient::new(
            &origin,
            "project-a",
            Arc::new(StaticToken("scaffold_control_secret".into())),
        )
        .unwrap();
        let result = client
            .handoff_worktree(
                "sandbox-a",
                &snapshot,
                SCAFFOLD_HANDOFF_CWD.to_string(),
                &CancellationToken::new(),
            )
            .await;
        if failure {
            let ScaffoldError::RepositoryImport { stage, diagnostic } = result.unwrap_err() else { panic!("repository diagnostic lost") };
            assert_eq!(stage, "source_import_repository");
            assert_eq!(diagnostic.sandbox_id, "sandbox-a");
            assert_eq!(diagnostic.exit_code, Some(45));
            assert_eq!(diagnostic.manifest_sha256, snapshot.manifest_sha256);
            assert_eq!(diagnostic.base_sha, snapshot.base_sha);
            assert_eq!(diagnostic.archive_bytes, snapshot.byte_count);
            assert_eq!(diagnostic.bundle_sha256, snapshot.repository.sha256);
            assert_eq!(diagnostic.bundle_bytes, snapshot.repository.byte_count);
            assert!(diagnostic.stderr.starts_with("crew_repository:tree_entry_unsupported\n"));
            assert!(diagnostic.stderr.len() <= 2048);
            assert!(!diagnostic.stderr.contains("worktree_upload_secret"));
        } else { result.unwrap(); }

        let requests = captured.await.unwrap();
        let upload_header_end = requests[2]
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        assert_eq!(&requests[2][upload_header_end + 4..], expected_archive);
        let verify = String::from_utf8_lossy(&requests[4]);
        assert!(verify.contains(&snapshot.manifest_sha256));
        assert!(verify.contains(&snapshot.base_sha));
        assert!(!verify.contains("worktree_upload_secret"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn worktree_materializer_preserves_unrelated_platform_and_rejects_corruption() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let remote = root.join("remote");
        let platform = root.join("platform");
        std::fs::create_dir(&platform).unwrap();
        std::fs::create_dir(&source).unwrap();
        let git = |cwd: &std::path::Path, args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(cwd)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&source, &["init", "-q"]);
        git(&source, &["config", "user.email", "crew@example.com"]);
        git(&source, &["config", "user.name", "Crew"]);
        std::fs::write(source.join("tracked.txt"), "base\n").unwrap();
        std::fs::write(source.join(".gitignore"), ".omx/\n").unwrap();
        std::fs::write(source.join("deleted.txt"), "delete\n").unwrap();
        std::fs::write(source.join("expanded-a.bin"), vec![b'a'; 80 * 1024]).unwrap();
        std::fs::write(source.join("expanded-b.bin"), vec![b'b'; 80 * 1024]).unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-qm", "base"]);
        let base_sha = String::from_utf8(git(&source, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        git(&platform, &["init", "-q"]);
        git(&platform, &["config", "user.email", "platform@example.com"]);
        git(&platform, &["config", "user.name", "Platform"]);
        std::fs::write(platform.join("platform.txt"), "platform base\n").unwrap();
        git(&platform, &["add", "."]);
        git(&platform, &["commit", "-qm", "unrelated platform"]);
        let platform_head = git(&platform, &["rev-parse", "HEAD"]);
        std::fs::write(platform.join("platform.txt"), "platform dirty\n").unwrap();

        std::fs::create_dir_all(source.join(".omx/plans")).unwrap();
        std::fs::write(source.join(".omx/plans/plan.md"), "plan\n").unwrap();
        std::fs::write(source.join("tracked.txt"), "changed\n").unwrap();
        std::fs::write(source.join("new.txt"), "new\n").unwrap();
        std::fs::create_dir(source.join("nested")).unwrap();
        std::fs::write(source.join("nested/file.txt"), "nested\n").unwrap();
        std::fs::remove_file(source.join("deleted.txt")).unwrap();
        symlink("tracked.txt", source.join("tracked-link")).unwrap();
        let snapshot =
            crate::worktree_handoff::capture_worktree_handoff(&source.join("nested")).unwrap();
        assert!(snapshot.byte_count <= 128 * 1024);
        let staging = platform.join(".scaffold/crew-handoff-staging");
        std::fs::create_dir_all(&staging).unwrap();
        let archive = tempfile::NamedTempFile::new().unwrap();
        let mut archive_file = archive.reopen().unwrap();
        std::io::copy(&mut snapshot.reopen().unwrap(), &mut archive_file).unwrap();
        let extracted = std::process::Command::new("tar")
            .args([
                "-xf",
                archive.path().to_str().unwrap(),
                "-C",
                staging.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            extracted.status.success(),
            "{}",
            String::from_utf8_lossy(&extracted.stderr)
        );
        let script = VERIFY_WORKTREE_HANDOFF_PYTHON
            .replace("/workspace/ashler-platform", platform.to_str().unwrap())
            .replace("/workspace/crew-handoff", remote.to_str().unwrap())
            .replace("MAX_TOTAL = 256 * 1024 * 1024", "MAX_TOTAL = 128 * 1024")
            .replace(
                "MAX_CHECKOUT_BYTES = 1024 * 1024 * 1024",
                "MAX_CHECKOUT_BYTES = 256 * 1024",
            );
        let applied = std::process::Command::new("python3")
            .args([
                "-c",
                &script,
                staging.to_str().unwrap(),
                &snapshot.manifest_sha256,
                &snapshot.base_sha,
                &snapshot.entry_count.to_string(),
                &serde_json::to_string(&snapshot.cwd_relative_path).unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            applied.status.success(),
            "status={} stderr={}",
            applied.status,
            String::from_utf8_lossy(&applied.stderr)
        );
        assert_eq!(
            std::fs::read(remote.join("expanded-a.bin")).unwrap(),
            vec![b'a'; 80 * 1024]
        );
        assert_eq!(
            std::fs::read(remote.join("expanded-b.bin")).unwrap(),
            vec![b'b'; 80 * 1024]
        );
        assert_eq!(
            String::from_utf8_lossy(&applied.stdout).trim(),
            format!(
                "verified:{}:{}",
                snapshot.manifest_sha256,
                remote.join("nested").display()
            )
        );
        assert_eq!(
            std::fs::read(remote.join("tracked.txt")).unwrap(),
            b"changed\n"
        );
        assert_eq!(
            git(&remote, &["rev-parse", "HEAD"]),
            format!("{base_sha}\n").as_bytes()
        );
        assert_eq!(
            git(&remote, &["log", "--format=%H:%an:%ae:%s"]),
            git(&source, &["log", "--format=%H:%an:%ae:%s"])
        );
        assert_eq!(git(&platform, &["rev-parse", "HEAD"]), platform_head);
        assert_eq!(
            std::fs::read(platform.join("platform.txt")).unwrap(),
            b"platform dirty\n"
        );
        assert!(!platform.join("tracked.txt").exists());
        assert_eq!(
            std::fs::read(remote.join("nested/file.txt")).unwrap(),
            b"nested\n"
        );
        assert_eq!(std::fs::read(remote.join("new.txt")).unwrap(), b"new\n");
        assert_eq!(
            std::fs::read(remote.join(".omx/plans/plan.md")).unwrap(),
            b"plan\n"
        );
        assert!(!remote.join("deleted.txt").exists());
        assert_eq!(
            std::fs::read_link(remote.join("tracked-link")).unwrap(),
            std::path::Path::new("tracked.txt")
        );
        assert!(!staging.exists());

        // Neither corruption nor an oversized checkout may replace the destination.
        for corrupt_bundle in [true, false] {
            std::fs::write(remote.join("local-marker"), "preserve\n").unwrap();
            std::fs::create_dir_all(&staging).unwrap();
            assert!(
                std::process::Command::new("tar")
                    .args([
                        "-xf",
                        archive.path().to_str().unwrap(),
                        "-C",
                        staging.to_str().unwrap()
                    ])
                    .status()
                    .unwrap()
                    .success()
            );
            if corrupt_bundle {
                std::fs::write(staging.join("source.bundle"), "corrupt").unwrap();
            }
            let rejection_script = if corrupt_bundle {
                script.clone()
            } else {
                script.replace(
                    "MAX_CHECKOUT_BYTES = 256 * 1024",
                    "MAX_CHECKOUT_BYTES = 128 * 1024",
                )
            };
            let rejected = std::process::Command::new("python3")
                .args([
                    "-c",
                    &rejection_script,
                    staging.to_str().unwrap(),
                    &snapshot.manifest_sha256,
                    &snapshot.base_sha,
                    &snapshot.entry_count.to_string(),
                    &serde_json::to_string(&snapshot.cwd_relative_path).unwrap(),
                ])
                .output()
                .unwrap();
            assert_eq!(
                rejected.status.code(),
                Some(if corrupt_bundle { 44 } else { 45 })
            );
            assert_eq!(
                std::fs::read(remote.join("local-marker")).unwrap(),
                b"preserve\n"
            );
            assert_eq!(
                git(&remote, &["rev-parse", "HEAD"]),
                format!("{base_sha}\n").as_bytes()
            );
            assert_eq!(git(&platform, &["rev-parse", "HEAD"]), platform_head);
            assert_eq!(
                std::fs::read(platform.join("platform.txt")).unwrap(),
                b"platform dirty\n"
            );
            assert!(!staging.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn worktree_materializer_imports_delta_and_equal_head_without_platform_mutation() {
        use std::os::unix::fs::symlink;

        for scenario in [
            "delta",
            "equal",
            "shallow_platform",
            "nonshallow_promisor_shallow_platform",
            "missing_prerequisite",
            "unrelated_prerequisite",
            "invalid_prerequisite",
            "invalid_shallow",
            "incompatible_shallow",
            "zero_wrong_prerequisite",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let source = root.join("source");
            let platform = root.join("platform");
            let remote = root.join("remote");
            std::fs::create_dir(&source).unwrap();
            std::fs::create_dir(&remote).unwrap();
            std::fs::write(remote.join("sentinel"), "preserve\n").unwrap();
            let git = |cwd: &std::path::Path, args: &[&str]| {
                let output = std::process::Command::new("git")
                    .args(args)
                    .current_dir(cwd)
                    .env("ASHLER_INCREMENTAL_TSC_CHECKS", "false")
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                output.stdout
            };
            git(&source, &["init", "-q"]);
            git(&source, &["config", "user.email", "crew@example.com"]);
            git(&source, &["config", "user.name", "Crew"]);
            std::fs::write(source.join("history"), vec![b'x'; 600 * 1024]).unwrap();
            if scenario == "nonshallow_promisor_shallow_platform" {
                std::fs::write(source.join("restored.txt"), vec![b'r'; 8192]).unwrap();
            }
            git(&source, &["add", "."]);
            git(&source, &["commit", "-qm", "large historical object"]);
            std::fs::remove_file(source.join("history")).unwrap();
            if scenario == "nonshallow_promisor_shallow_platform" {
                std::fs::remove_file(source.join("restored.txt")).unwrap();
            }
            std::fs::write(source.join("tracked.txt"), "base\n").unwrap();
            std::fs::write(source.join("deleted.txt"), "delete\n").unwrap();
            git(&source, &["add", "-A"]);
            git(&source, &["commit", "-qm", "shared prerequisite"]);
            let prerequisite = String::from_utf8(git(&source, &["rev-parse", "HEAD"]))
                .unwrap()
                .trim()
                .to_string();
            if matches!(scenario, "shallow_platform" | "nonshallow_promisor_shallow_platform") {
                git(
                    &root,
                    &[
                        "clone",
                        "-q",
                        "--depth=1",
                        &format!("file://{}", source.display()),
                        platform.to_str().unwrap(),
                    ],
                );
            } else {
                git(
                    &root,
                    &[
                        "clone",
                        "-q",
                        "--no-hardlinks",
                        source.to_str().unwrap(),
                        platform.to_str().unwrap(),
                    ],
                );
            }
            // The prerequisite is an object dependency, not permission to reset
            // the platform's unrelated branch or dirty working tree.
            git(&platform, &["config", "user.email", "platform@example.com"]);
            git(&platform, &["config", "user.name", "Platform"]);
            git(&platform, &["checkout", "-q", "--orphan", "platform-only"]);
            git(&platform, &["rm", "-q", "-rf", "."]);
            std::fs::write(platform.join("platform.txt"), "platform base\n").unwrap();
            git(&platform, &["add", "."]);
            git(&platform, &["commit", "-qm", "unrelated platform"]);
            let platform_head = git(&platform, &["rev-parse", "HEAD"]);
            let platform_refs = git(&platform, &["show-ref"]);
            let platform_index = std::fs::read(platform.join(".git/index")).unwrap();
            std::fs::write(platform.join("platform.txt"), "platform dirty\n").unwrap();
            if !matches!(scenario, "equal" | "zero_wrong_prerequisite") {
                std::fs::write(source.join("committed.txt"), "source delta\n").unwrap();
                if scenario == "nonshallow_promisor_shallow_platform" {
                    std::fs::write(source.join("restored.txt"), vec![b'r'; 8192]).unwrap();
                }
                git(&source, &["add", "."]);
                git(&source, &["commit", "-qm", "local change"]);
            }
            let origin = root.join("origin");
            if scenario == "nonshallow_promisor_shallow_platform" {
                git(&source, &["config", "uploadpack.allowFilter", "true"]);
                std::fs::rename(&source, &origin).unwrap();
                git(&root, &["clone", "-q", "--filter=blob:none", "--no-checkout", &format!("file://{}", origin.display()), source.to_str().unwrap()]);
                git(&source, &["checkout", "-q", "HEAD"]);
                assert_eq!(String::from_utf8_lossy(&git(&source, &["rev-parse", "--is-shallow-repository"])).trim(), "false");
                assert_eq!(String::from_utf8_lossy(&git(&source, &["config", "remote.origin.promisor"])).trim(), "true");
                assert!(git(&source, &["rev-list", "--objects", "--all", "--missing=print"]).split(|byte| *byte == b'\n').any(|line| line.starts_with(b"?")), "fixture must genuinely lack historical blobs");
            }
            std::fs::write(source.join("tracked.txt"), "overlay\n").unwrap();
            std::fs::remove_file(source.join("deleted.txt")).unwrap();
            std::fs::create_dir(source.join("nested")).unwrap();
            std::fs::write(source.join("nested/new.txt"), "untracked\n").unwrap();
            symlink("tracked.txt", source.join("tracked-link")).unwrap();
            let source_refs = git(&source, &["show-ref"]);
            let source_index = std::fs::read(source.join(".git/index")).unwrap();
            let source_config = std::fs::read(source.join(".git/config")).unwrap();
            let source_objects = git(&source, &["count-objects", "-v"]);
            let pack_contents = || std::fs::read_dir(source.join(".git/objects/pack")).unwrap()
                .map(|entry| { let entry = entry.unwrap(); (entry.file_name(), std::fs::read(entry.path()).unwrap()) })
                .collect::<BTreeMap<_, _>>();
            let source_packs = pack_contents();
            let snapshot = crate::worktree_handoff::capture_worktree_handoff_cancellable(
                &source.join("nested"),
                Some(&prerequisite),
                &CancellationToken::new(),
            )
            .unwrap();
            assert_eq!(git(&source, &["show-ref"]), source_refs);
            assert_eq!(std::fs::read(source.join(".git/index")).unwrap(), source_index);
            assert_eq!(std::fs::read(source.join(".git/config")).unwrap(), source_config);
            assert_eq!(git(&source, &["count-objects", "-v"]), source_objects);
            assert_eq!(pack_contents(), source_packs);
            if scenario == "nonshallow_promisor_shallow_platform" {
                // The receiver must not need either source origin at import time.
                std::fs::rename(&origin, root.join("offline-origin")).unwrap();
            }
            let staging = platform.join(".scaffold/crew-handoff-staging");
            std::fs::create_dir_all(&staging).unwrap();
            let archive = tempfile::NamedTempFile::new().unwrap();
            std::io::copy(
                &mut snapshot.reopen().unwrap(),
                &mut archive.reopen().unwrap(),
            )
            .unwrap();
            assert!(
                std::process::Command::new("tar")
                    .args([
                        "-xf",
                        archive.path().to_str().unwrap(),
                        "-C",
                        staging.to_str().unwrap()
                    ])
                    .status()
                    .unwrap()
                    .success()
            );
            let manifest_path = staging.join(".crew-handoff-manifest.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
            match scenario {
                "missing_prerequisite" | "zero_wrong_prerequisite" => {
                    manifest["repository"]["prerequisiteSha"] =
                        serde_json::json!("f".repeat(prerequisite.len()));
                }
                "unrelated_prerequisite" => {
                    manifest["repository"]["prerequisiteSha"] =
                        serde_json::json!(String::from_utf8_lossy(&platform_head).trim());
                }
                "invalid_prerequisite" => {
                    manifest["repository"]["prerequisiteSha"] = serde_json::json!("not-a-commit");
                }
                "invalid_shallow" => manifest["repository"]["shallow"] = serde_json::json!("true"),
                "incompatible_shallow" => {
                    manifest["repository"]["shallow"] = serde_json::json!(true)
                }
                _ => {}
            }
            let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
            let manifest_sha = format!("{:x}", sha2::Sha256::digest(&manifest_bytes));
            std::fs::write(&manifest_path, manifest_bytes).unwrap();
            let script = VERIFY_WORKTREE_HANDOFF_PYTHON
                .replace("/workspace/ashler-platform", platform.to_str().unwrap())
                .replace("/workspace/crew-handoff", remote.to_str().unwrap())
                .replace(
                    "MAX_HISTORY_BYTES = 4 * 1024 * 1024 * 1024",
                    "MAX_HISTORY_BYTES = 64 * 1024",
                );
            let result = std::process::Command::new("python3")
                .args([
                    "-c",
                    &script,
                    staging.to_str().unwrap(),
                    &manifest_sha,
                    &snapshot.base_sha,
                    &snapshot.entry_count.to_string(),
                    &serde_json::to_string(&snapshot.cwd_relative_path).unwrap(),
                ])
                .output()
                .unwrap();
            if matches!(scenario, "delta" | "equal" | "shallow_platform" | "nonshallow_promisor_shallow_platform") {
                assert!(
                    result.status.success(),
                    "{scenario}: status={} stderr={}",
                    result.status,
                    String::from_utf8_lossy(&result.stderr)
                );
                assert_eq!(
                    git(&remote, &["rev-parse", "HEAD"]),
                    git(&source, &["rev-parse", "HEAD"])
                );
                assert_eq!(
                    std::fs::read(remote.join("tracked.txt")).unwrap(),
                    b"overlay\n"
                );
                assert_eq!(
                    std::fs::read(remote.join("nested/new.txt")).unwrap(),
                    b"untracked\n"
                );
                assert_eq!(
                    std::fs::read_link(remote.join("tracked-link")).unwrap(),
                    std::path::Path::new("tracked.txt")
                );
                assert!(!remote.join("deleted.txt").exists());
                assert!(!remote.join("sentinel").exists());
                if scenario != "equal" {
                    assert_eq!(
                        std::fs::read(remote.join("committed.txt")).unwrap(),
                        b"source delta\n"
                    );
                }
                let history = git(&remote, &["rev-list", "--count", "HEAD"]);
                let expected_history = if scenario == "delta" { "3" } else { "2" };
                assert_eq!(String::from_utf8_lossy(&history).trim(), expected_history);
                if scenario == "nonshallow_promisor_shallow_platform" {
                    assert_eq!(std::fs::read(remote.join("restored.txt")).unwrap(), vec![b'r'; 8192]);
                }
            } else {
                let expected_code =
                    if matches!(scenario, "missing_prerequisite" | "unrelated_prerequisite") {
                        45
                    } else {
                        42
                    };
                assert_eq!(
                    result.status.code(),
                    Some(expected_code),
                    "{scenario}: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                assert_eq!(
                    std::fs::read(remote.join("sentinel")).unwrap(),
                    b"preserve\n"
                );
                assert!(!remote.join(".git").exists());
            }
            assert_eq!(git(&platform, &["rev-parse", "HEAD"]), platform_head);
            assert_eq!(git(&platform, &["show-ref"]), platform_refs);
            assert_eq!(
                std::fs::read(platform.join(".git/index")).unwrap(),
                platform_index
            );
            assert_eq!(
                std::fs::read(platform.join("platform.txt")).unwrap(),
                b"platform dirty\n"
            );
            assert!(!platform.join("tracked.txt").exists());
            assert!(!staging.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn worktree_materializer_imports_more_than_25000_tracked_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let platform = root.join("platform");
        let remote = root.join("remote");
        std::fs::create_dir_all(source.join("files")).unwrap();
        std::fs::create_dir(&platform).unwrap();
        std::fs::write(platform.join("sentinel"), b"untouched").unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git").args(args).current_dir(&source).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "crew@example.com"]);
        git(&["config", "user.name", "Crew"]);
        for index in 0..25_001 {
            std::fs::write(source.join(format!("files/{index}")), b"base\n").unwrap();
        }
        git(&["add", "."]);
        git(&["-c", "core.hooksPath=/dev/null", "commit", "-qm", "large base"]);
        let snapshot = crate::worktree_handoff::capture_worktree_handoff(&source).unwrap();
        assert_eq!(snapshot.entry_count, 0, "tracked base files are not changed entries");
        let staging = platform.join(".scaffold/crew-handoff-staging");
        std::fs::create_dir_all(&staging).unwrap();
        let archive = tempfile::NamedTempFile::new().unwrap();
        std::io::copy(&mut snapshot.reopen().unwrap(), &mut archive.reopen().unwrap()).unwrap();
        assert!(std::process::Command::new("tar").arg("-xf").arg(archive.path()).arg("-C").arg(&staging)
            .status().unwrap().success());
        let script = VERIFY_WORKTREE_HANDOFF_PYTHON
            .replace("/workspace/ashler-platform", platform.to_str().unwrap())
            .replace("/workspace/crew-handoff", remote.to_str().unwrap());
        let result = std::process::Command::new("python3").args([
            "-c", &script, staging.to_str().unwrap(), &snapshot.manifest_sha256,
            &snapshot.base_sha, &snapshot.entry_count.to_string(),
            &serde_json::to_string(&snapshot.cwd_relative_path).unwrap(),
        ]).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        assert_eq!(std::fs::read_dir(remote.join("files")).unwrap().count(), 25_001);
        assert_eq!(std::fs::read(platform.join("sentinel")).unwrap(), b"untouched");
        assert!(!staging.exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_materializer_enforces_containment_and_separate_expansion_budgets() {
        use std::os::unix::fs::symlink;
        for scenario in [
            "absolute",
            "chain",
            "object_expansion",
            "tree_expansion",
            "object_count",
            "shallow_snapshot",
            "import_expansion",
            "directory_heavy_at_file_limit",
            "committed_file_over_limit",
            "overlay_symlink_over_limit",
            "committed_directory_over_limit",
            "cwd_directory_over_limit",
            "reserved_directory",
            "metadata_symlink",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let source = root.join("source");
            let platform = root.join("platform");
            let remote = root.join("remote");
            for directory in [&source, &platform, &remote] {
                std::fs::create_dir(directory).unwrap();
            }
            std::fs::write(platform.join("sentinel"), "platform\n").unwrap();
            std::fs::write(remote.join("sentinel"), "destination\n").unwrap();
            let git = |args: &[&str]| {
                assert!(
                    std::process::Command::new("git")
                        .args(args)
                        .current_dir(&source)
                        .env("ASHLER_INCREMENTAL_TSC_CHECKS", "false")
                        .status()
                        .unwrap()
                        .success()
                );
            };
            git(&["init", "-q"]);
            git(&["config", "user.email", "crew@example.com"]);
            git(&["config", "user.name", "Crew"]);
            match scenario {
                "absolute" => symlink(platform.join("sentinel"), source.join("escape")).unwrap(),
                "chain" => {
                    std::fs::create_dir(source.join("nested")).unwrap();
                    symlink("../../platform", source.join("nested/escape")).unwrap();
                    symlink("nested/escape/sentinel", source.join("chain")).unwrap();
                }
                "directory_heavy_at_file_limit"
                | "committed_file_over_limit"
                | "overlay_symlink_over_limit" => {
                    // Twelve files (including a directory symlink) and three real
                    // directories mirror the file/directory split of a large source.
                    for index in 0..11 {
                        let directory = source.join(format!("group-{}", index % 3));
                        std::fs::create_dir_all(&directory).unwrap();
                        std::fs::write(directory.join(format!("file-{index}")), "file\n").unwrap();
                    }
                    symlink("group-0", source.join("directory-link")).unwrap();
                    if scenario == "committed_file_over_limit" {
                        std::fs::write(source.join("extra"), "one file too many\n").unwrap();
                    }
                }
                "committed_directory_over_limit" => {
                    for index in 0..4 {
                        let directory = source.join(format!("group-{index}"));
                        std::fs::create_dir(&directory).unwrap();
                        std::fs::write(directory.join("file"), "file\n").unwrap();
                    }
                }
                "cwd_directory_over_limit" => {
                    std::fs::write(source.join("file"), "file\n").unwrap();
                    std::fs::create_dir_all(source.join("one/two/three/four")).unwrap();
                }
                "reserved_directory" => {
                    std::fs::create_dir(source.join(".scaffold")).unwrap();
                    std::fs::write(source.join(".scaffold/file"), "metadata\n").unwrap();
                }
                "metadata_symlink" => symlink(".git", source.join("metadata-link")).unwrap(),
                "object_expansion" => {
                    std::fs::write(source.join("large"), vec![b'x'; 2 * 1024 * 1024]).unwrap()
                }
                "tree_expansion" => {
                    let bytes = vec![b'x'; 600 * 1024];
                    std::fs::write(source.join("one"), &bytes).unwrap();
                    std::fs::write(source.join("two"), &bytes).unwrap();
                }
                "object_count" => std::fs::write(source.join("file"), "one\n").unwrap(),
                "shallow_snapshot" => {
                    for value in [b'a', b'b', b'c'] {
                        std::fs::write(source.join("revision"), vec![value; 600 * 1024]).unwrap();
                        git(&["add", "."]);
                        git(&["commit", "-qm", "historical revision"]);
                    }
                    std::fs::remove_file(source.join("revision")).unwrap();
                    std::fs::write(source.join("current"), "small checkout\n").unwrap();
                }
                "import_expansion" => {
                    std::fs::write(source.join("current"), vec![b'x'; 600 * 1024]).unwrap();
                }
                _ => unreachable!(),
            }
            git(&["add", "."]);
            git(&["commit", "-qm", "source"]);
            if scenario == "overlay_symlink_over_limit" {
                symlink("group-1", source.join("extra-link")).unwrap();
            }
            let cwd = if scenario == "cwd_directory_over_limit" {
                source.join("one/two/three/four")
            } else {
                source.clone()
            };
            let snapshot = crate::worktree_handoff::capture_worktree_handoff(&cwd).unwrap();
            assert_eq!(
                snapshot.entry_count,
                usize::from(scenario == "overlay_symlink_over_limit"),
                "only the extra symlink belongs to the overlay"
            );
            let staging = platform.join(".scaffold/crew-handoff-staging");
            std::fs::create_dir_all(&staging).unwrap();
            let archive = tempfile::NamedTempFile::new().unwrap();
            std::io::copy(
                &mut snapshot.reopen().unwrap(),
                &mut archive.reopen().unwrap(),
            )
            .unwrap();
            assert!(
                std::process::Command::new("tar")
                    .args([
                        "-xf",
                        archive.path().to_str().unwrap(),
                        "-C",
                        staging.to_str().unwrap()
                    ])
                    .status()
                    .unwrap()
                    .success()
            );
            let script = VERIFY_WORKTREE_HANDOFF_PYTHON
                .replace("/workspace/ashler-platform", platform.to_str().unwrap())
                .replace("/workspace/crew-handoff", remote.to_str().unwrap())
                .replace("MAX_TOTAL = 256 * 1024 * 1024", "MAX_TOTAL = 1024 * 1024")
                .replace(
                    "MAX_CHECKOUT_BYTES = 1024 * 1024 * 1024",
                    "MAX_CHECKOUT_BYTES = 1024 * 1024",
                )
                .replace("MAX_FILES = 50000", "MAX_FILES = 12")
                .replace("MAX_DIRECTORIES = 25000", "MAX_DIRECTORIES = 3");
            let script = if scenario == "import_expansion" {
                script.replace(
                    "MAX_HISTORY_BYTES = 4 * 1024 * 1024 * 1024",
                    "MAX_HISTORY_BYTES = 512 * 1024",
                )
            } else {
                script
            };
            let script = if scenario == "object_count" {
                script.replace("MAX_OBJECTS = 250000", "MAX_OBJECTS = 1")
            } else {
                script
            };
            let result = std::process::Command::new("python3")
                .args([
                    "-c",
                    &script,
                    staging.to_str().unwrap(),
                    &snapshot.manifest_sha256,
                    &snapshot.base_sha,
                    &snapshot.entry_count.to_string(),
                    &serde_json::to_string(&snapshot.cwd_relative_path).unwrap(),
                ])
                .output()
                .unwrap();
            assert!(!staging.exists(), "{scenario} must clean up staging");
            assert!(
                std::fs::read_dir(&root).unwrap().all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".crew-handoff-")),
                "{scenario} must clean up the transactional checkout"
            );
            if scenario == "directory_heavy_at_file_limit" {
                assert!(
                    result.status.success(),
                    "directories must not consume the file budget: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                for index in 0..11 {
                    assert_eq!(
                        std::fs::read(remote.join(format!("group-{}/file-{index}", index % 3)))
                            .unwrap(),
                        b"file\n"
                    );
                }
                assert_eq!(
                    std::fs::read_link(remote.join("directory-link")).unwrap(),
                    std::path::Path::new("group-0")
                );
                assert!(!remote.join("sentinel").exists());
                assert_eq!(
                    std::fs::read(platform.join("sentinel")).unwrap(),
                    b"platform\n"
                );
                continue;
            }
            if scenario == "shallow_snapshot" {
                assert!(
                    result.status.success(),
                    "unshared history must not be imported: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                assert_eq!(
                    std::fs::read(remote.join("current")).unwrap(),
                    b"small checkout\n"
                );
                assert!(!remote.join("revision").exists());
                let history = std::process::Command::new("git")
                    .args(["rev-list", "--count", "HEAD"])
                    .current_dir(&remote)
                    .env("ASHLER_INCREMENTAL_TSC_CHECKS", "false")
                    .output()
                    .unwrap();
                assert!(history.status.success());
                assert_eq!(String::from_utf8_lossy(&history.stdout).trim(), "1");
                assert_eq!(
                    std::fs::read(platform.join("sentinel")).unwrap(),
                    b"platform\n"
                );
                continue;
            }
            assert_eq!(
                result.status.code(),
                Some(match scenario {
                    "absolute"
                    | "chain"
                    | "metadata_symlink"
                    | "overlay_symlink_over_limit"
                    | "cwd_directory_over_limit" => 46,
                    "reserved_directory" => 43,
                    _ => 45,
                }),
                "{scenario} must fail at the safety bound before publication: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            if result.status.code() == Some(45) {
                let reason = match scenario {
                    "object_count" => "bundle_pack_bounds",
                    "object_expansion" | "import_expansion" => "object_expansion_bounds",
                    "tree_expansion" | "committed_file_over_limit" => "tree_expansion_bounds",
                    "committed_directory_over_limit" => "tree_directory_bounds",
                    _ => panic!("missing diagnostic expectation for {scenario}"),
                };
                assert_eq!(String::from_utf8_lossy(&result.stderr).trim(), format!("crew_repository:{reason}"));
                assert!(result.stderr.len() <= 2048);
            }
            assert_eq!(
                std::fs::read(platform.join("sentinel")).unwrap(),
                b"platform\n"
            );
            assert_eq!(
                std::fs::read(remote.join("sentinel")).unwrap(),
                b"destination\n"
            );
            assert!(!remote.join(".git").exists());
        }
    }

    #[test]
    fn handoff_rejects_escaping_archive_paths_and_changed_capture_files() {
        let bytes = b"{\"type\":\"session\",\"id\":\"native-1\",\"cwd\":\"/repo\"}\n";
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), bytes).unwrap();
        assert!(
            crate::omp_session_artifact::capture_omp_session_file(
                source.path(),
                std::path::Path::new("../escape.jsonl"),
                "native-1",
                "/repo",
                &CancellationToken::new(),
            )
            .is_err()
        );
        let artifact = crate::omp_session_artifact::capture_omp_session_file(
            source.path(),
            std::path::Path::new("safe/native-1.jsonl"),
            "native-1",
            "/repo",
            &CancellationToken::new(),
        )
        .unwrap();
        artifact
            .reopen()
            .unwrap()
            .set_len(artifact.byte_count + 1)
            .unwrap();
        assert!(prepare_omp_handoff_archive(artifact, &CancellationToken::new()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn isolated_recovery_restarts_and_isolates_legacy_and_duplicate_native_ids() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let script = format!("REPOSITORY_SOURCE = {}\nNATIVE_SOURCE = {}\n{}",
            serde_json::to_string(&VERIFY_WORKTREE_HANDOFF_PYTHON.replace("/workspace/ashler-platform", root.join("ashler-platform").to_str().unwrap())).unwrap(),
            serde_json::to_string(&VERIFY_HANDOFF_PYTHON.replace("/workspace", root.to_str().unwrap())).unwrap(),
            include_str!("isolated_recovery.py").replace("/workspace/ashler-platform", root.join("ashler-platform").to_str().unwrap()));
        let helper = root.join("helper.py");
        std::fs::write(&helper, script).unwrap();
        let fixture = r#"import hashlib, json, os, pathlib, subprocess, sys, time, uuid
root, helper = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
platform = root / 'ashler-platform'; platform.mkdir()
source = root / 'source'; source.mkdir()
def git(*args):
    return subprocess.check_output(['git', '-C', str(source), *args], stderr=subprocess.DEVNULL)
git('init', '-q'); git('config', 'user.email', 'crew@example.com'); git('config', 'user.name', 'Crew')
(source / 'tracked').write_text('base\n'); git('add', '.'); git('commit', '-qm', 'base')
base = git('rev-parse', 'HEAD').decode().strip()
operation = 'crew-handoff:original-command:isolated-recovery'
namespace = hashlib.sha256(operation.encode()).hexdigest()
op = platform / '.scaffold' / ('crew-native-' + namespace)
attempt = op / 'uploads' / uuid.uuid4().hex
repo, native = attempt / 'repository', attempt / 'native'
repo.mkdir(parents=True); (native / 'by-cwd').mkdir(parents=True)
git('bundle', 'create', str(repo / 'source.bundle'), 'HEAD')
bundle = (repo / 'source.bundle').read_bytes()
manifest = {'version':'crew.scaffold.worktree.v2','baseSha':base,'cwdRelativePath':'nested','entries':[{'path':'tracked','kind':'regular','sha256':hashlib.sha256(b'overlay\n').hexdigest(),'byteCount':8,'executable':False}], 'repository':{'path':'source.bundle','sha256':hashlib.sha256(bundle).hexdigest(),'byteCount':len(bundle)}}
(repo / 'files').mkdir(); (repo / 'files/tracked').write_bytes(b'overlay\n')
manifest_bytes = json.dumps(manifest).encode(); (repo / '.crew-handoff-manifest.json').write_bytes(manifest_bytes)
journal = b'{"type":"session","version":3,"id":"native-A","cwd":"/original"}\n{"type":"message","content":[{"type":"image","data":"attachment"}]}\n'
source_journal = journal
def recovery_journal(operation_id):
    identity = bytearray(hashlib.sha256(operation_id.encode()).digest()[:16])
    identity[6] = (identity[6] & 15) | 128; identity[8] = (identity[8] & 63) | 128
    native_id = str(uuid.UUID(bytes=bytes(identity)))
    header, history = source_journal.split(b'\n', 1); record = json.loads(header); record['id'] = native_id
    return native_id, json.dumps(record,separators=(',',':')).encode()+b'\n'+history
derived_id, conflicting_journal = recovery_journal(operation)
(native / 'by-cwd/A.jsonl').write_bytes(journal); (native / 'by-cwd/A.jsonl').chmod(0o600)
descriptor = {'version':'crew.native-isolated-recovery.v1','provenance':'legacy-recapture','operation_id':operation,'namespace':namespace,'sandbox_id':'sandbox-a','scope':{'organizationId':'org'},'environment':{'epoch':1,'database':'local'},'source_selector':'/original/A.jsonl','source_cwd':'/original','session_only':False,'native':{'archive_sha256':'a'*64,'archive_bytes':10240,'storage_relative_path':'by-cwd/A.jsonl','sha256':hashlib.sha256(journal).hexdigest(),'byte_count':len(journal),'native_session_id':'native-A','cwd':'/original'},'repository':{'archive_sha256':'b'*64,'archive_bytes':10240,'manifest_sha256':hashlib.sha256(manifest_bytes).hexdigest(),'base_sha':base,'entry_count':1,'cwd_relative_path':'nested'}}
descriptor['target_native_id'] = derived_id
descriptor['published_cwd'] = str(root / ('crew-native-' + namespace) / 'nested')
descriptor_path = attempt / 'descriptor.json'; descriptor_path.write_text(json.dumps(descriptor))
runtime = root / 'runtime'; (runtime / 'omp-inference').mkdir(parents=True)
agent = root / 'home/.omp/profiles/scaffold-host/agent'; agent.mkdir(parents=True)
models = agent / 'models.yml'; models.write_text('providers: {}\n'); models.chmod(0o600)
(runtime / 'omp-inference/profile.json').write_text(json.dumps({'modelsPath':str(models)}))
env = dict(os.environ, SCAFFOLD_RUNTIME_DIR=str(runtime), HOME=str(root / 'home'))
env.pop('XDG_DATA_HOME', None); env.pop('PI_CONFIG_DIR', None)
legacy = root / 'crew-handoff'; legacy.mkdir(); (legacy / 'sentinel').write_text('legacy')
legacy_staging = platform / '.scaffold/crew-handoff-staging'; legacy_staging.mkdir()
(legacy_staging / 'sentinel').write_text('legacy upload')
sessions = agent / 'sessions/legacy-cwd'; sessions.mkdir(parents=True)
(sessions / 'A.jsonl').write_bytes(source_journal); (sessions / 'A.jsonl').chmod(0o600)
# Exercise upload API defaults; only the operation namespace may be tightened.
for owned in (op, op / 'uploads', attempt): owned.chmod(0o755)
descriptor_path.chmod(0o644)
uploaded_helper = attempt / 'recovery.py'; uploaded_helper.write_text(helper.read_text()); uploaded_helper.chmod(0o644)
legacy_modes = {path: path.stat().st_mode & 0o777 for path in (platform, platform / '.scaffold', legacy, legacy_staging, sessions)}
# Object packs/index/logs have their own strict Git verification, not the upload cap.
metadata_check = __import__('runpy').run_path(str(helper))['check_metadata']
metadata_expected, metadata_actual = root / 'metadata-expected', root / 'metadata-actual'
for git_dir in (metadata_expected, metadata_actual):
    (git_dir / 'objects').mkdir(parents=True); (git_dir / 'logs').mkdir()
    (git_dir / 'HEAD').write_text(base+'\n'); (git_dir / 'config').write_text('same protected config\n')
with (metadata_actual / 'objects/large-pack').open('wb') as sparse: sparse.truncate(256*1024*1024+1)
with (metadata_actual / 'index').open('wb') as sparse: sparse.truncate(256*1024*1024+1)
(metadata_actual / 'logs/arbitrary').write_text('different volatile log')
metadata_check(metadata_expected, metadata_actual)
(metadata_actual / 'config').write_text('changed protected config\n')
try:
    metadata_check(metadata_expected, metadata_actual)
except ValueError:
    pass
else:
    raise AssertionError('protected Git metadata changes must still fail')
def run(path=descriptor_path, program=helper):
    return subprocess.run([sys.executable, str(program), str(path), str(attempt)], env=env, capture_output=True)
destination = root / ('crew-native-' + namespace)
destination.mkdir(); (destination / 'unverified').write_text('must remain')
blocked = run(); assert blocked.returncode != 0
assert (destination / 'unverified').read_text() == 'must remain'
assert not (destination / '.git').exists()
(destination / 'unverified').unlink(); destination.rmdir()
# A real process exit immediately after directory publication, before phase/receipt.
crasher = root / 'crasher.py'
crasher.write_text('import sys, os\ndef crash(frame,event,arg):\n if event == "call" and frame.f_code.co_name == "phase" and frame.f_locals.get("value") == "repository": os._exit(77)\nsys.setprofile(crash)\n' + helper.read_text())
crash = run(program=crasher); assert crash.returncode == 77, crash.stderr
destination = root / ('crew-native-' + namespace)
assert (destination / 'tracked').read_bytes() == b'overlay\n'
assert not (op / 'receipt.json').exists()
# A conflicting B anywhere in the native store blocks first publication.
global_conflict = sessions / 'conflicting-B.jsonl'; global_conflict.write_bytes(conflicting_journal); global_conflict.chmod(0o600)
assert run().returncode != 0
assert not list((agent / 'sessions').rglob('recovered-' + namespace + '.jsonl'))
global_conflict.unlink()
# Continue legacy writes while exact isolated publication is adopted.
writer = subprocess.Popen([sys.executable, '-c', 'import pathlib,sys,time\np=pathlib.Path(sys.argv[1])\nwhile True:\n p.write_text("legacy continues"); time.sleep(.005)', str(legacy / 'sentinel')])
try:
    # Exit after the actual native no-replace rename, before cleanup/phase/receipt.
    native_crasher = root / 'native-crasher.py'
    native_crasher.write_text('import sys,os\ndef crash_native(frame,event,arg):\n if event == "return" and frame.f_code.co_name == "publish_directory" and str(frame.f_locals.get("destination", "")).endswith("recovered-' + namespace + '.jsonl"): os._exit(78)\nsys.setprofile(crash_native)\n' + helper.read_text())
    native_crash = run(program=native_crasher); assert native_crash.returncode == 78, native_crash.stderr
    assert not (op / 'receipt.json').exists()
    renamed_native = list((agent / 'sessions').rglob('recovered-' + namespace + '.jsonl'))
    assert len(renamed_native) == 1 and renamed_native[0].stat().st_nlink == 1
    first = run(); assert first.returncode == 0, first.stderr
    receipt = json.loads(first.stdout); selector = pathlib.Path(receipt['launch_selector'])
    assert selector.is_absolute() and selector.parent != sessions
    imported = selector.read_bytes()
    assert imported.split(b'\n',1)[1] == journal.split(b'\n',1)[1]
    assert json.loads(imported.splitlines()[0])['cwd'] == str(destination / 'nested')
    assert json.loads(imported.splitlines()[0])['id'] == derived_id
    assert receipt['native_session_id'] == derived_id
    assert receipt['source_native_session_id'] == 'native-A'
    assert receipt['source_sha256'] == hashlib.sha256(source_journal).hexdigest()
    assert receipt['rebound_sha256'] == hashlib.sha256(imported).hexdigest()
    assert receipt['rebound_bytes'] == len(imported)
    assert (op / 'sealed/native/by-cwd/A.jsonl').read_bytes() == source_journal
    duplicate = sessions / 'duplicate-B.jsonl'; duplicate.write_bytes(imported); duplicate.chmod(0o600)
    assert run().returncode != 0
    assert selector.read_bytes() == imported
    duplicate.unlink()
    assert (sessions / 'A.jsonl').read_bytes() == source_journal
    assert (legacy_staging / 'sentinel').read_text() == 'legacy upload'
    assert (legacy / 'sentinel').exists()
    for owned in (op, op / 'uploads', attempt, op / 'sealed'):
        assert owned.stat().st_mode & 0o777 == 0o700, owned
    for record in (descriptor_path, uploaded_helper, op / 'lock', op / 'descriptor.json', op / 'phase.json', op / 'receipt.json'):
        assert record.stat().st_mode & 0o777 == 0o600, record
    for parent, dirs, files in os.walk(op / 'sealed'):
        assert pathlib.Path(parent).stat().st_mode & 0o777 == 0o700
        for name in files: assert (pathlib.Path(parent) / name).stat().st_mode & 0o777 == 0o600
    assert {path: path.stat().st_mode & 0o777 for path in legacy_modes} == legacy_modes
    # Simulate native publication completed but final receipt write interrupted.
    (op / 'receipt.json').unlink(); (op / 'phase.json').unlink()
    again = run(); assert again.returncode == 0 and json.loads(again.stdout) == receipt, again.stderr
    simultaneous = [subprocess.Popen([sys.executable,str(helper),str(descriptor_path),str(attempt)],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE) for _ in range(2)]
    for child in simultaneous:
        stdout, stderr = child.communicate(); assert child.returncode == 0 and json.loads(stdout) == receipt, stderr
    conflict = dict(descriptor, source_selector='/changed/A.jsonl')
    conflicting_path = attempt / 'conflicting.json'; conflicting_path.write_text(json.dumps(conflict))
    rejected = run(conflicting_path); assert rejected.returncode != 0
    assert selector.read_bytes() == imported
    # Neither receipt nor phase can authorize adopting changed actual contents.
    (destination / 'tracked').write_text('tampered\n'); assert run().returncode != 0
    (destination / 'tracked').write_text('overlay\n')
    selector.write_bytes(imported + b'changed history\n'); assert run().returncode != 0
    selector.write_bytes(imported)
    selector.chmod(0o644); assert run().returncode != 0; selector.chmod(0o600)
    sealed_native = op / 'sealed/native/by-cwd/A.jsonl'
    sealed_native.write_bytes(journal + b'changed source\n'); assert run().returncode != 0
    sealed_native.write_bytes(journal)
    missing_native = sealed_native.with_name('saved.jsonl'); sealed_native.rename(missing_native)
    assert run().returncode != 0
    missing_native.rename(sealed_native)
    # An inherited publisher FD keeps the permanent flock alive after its parent dies.
    lock = op / 'lock'; original_inode = lock.stat().st_ino
    tools = root / 'tools'; tools.mkdir(); marker = root / 'publisher-held'
    real_git = __import__('shutil').which('git')
    wrapper = tools / 'git'
    # Inspect the inherited descriptors themselves, not Darwin's /dev/fd nodes.
    wrapper.write_text('#!' + sys.executable + '\nimport os,pathlib,sys,time\nmarker=pathlib.Path(os.environ["LOCK_MARKER"])\nif not marker.exists():\n held=False\n target=os.stat(os.environ["LOCK_TARGET"])\n for name in os.listdir("/dev/fd"):\n  try:\n   inherited=os.fstat(int(name))\n   if (inherited.st_dev,inherited.st_ino)==(target.st_dev,target.st_ino): held=True\n  except (OSError,ValueError): pass\n marker.write_text("inherited" if held else "missing")\n release=marker.with_suffix(".release"); deadline=time.monotonic()+10\n while not release.exists() and time.monotonic()<deadline: time.sleep(.01)\nos.execv(os.environ["REAL_GIT"],[os.environ["REAL_GIT"],*sys.argv[1:]])\n')
    wrapper.chmod(0o755)
    wrapped_env = dict(env, PATH=str(tools)+os.pathsep+env['PATH'], LOCK_MARKER=str(marker), LOCK_TARGET=str(lock), REAL_GIT=real_git)
    holder = subprocess.Popen([sys.executable,str(helper),str(descriptor_path),str(attempt)],env=wrapped_env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    waiting = None
    try:
        deadline = time.monotonic() + 10
        while (not marker.exists() or not marker.read_text()) and time.monotonic() < deadline: time.sleep(.01)
        assert marker.read_text() == 'inherited', 'actual Git publisher must inherit the helper lock FD'
        holder.kill(); holder.wait()
        waiting = subprocess.Popen([sys.executable,str(helper),str(descriptor_path),str(attempt)],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        time.sleep(.1); assert waiting.poll() is None
        marker.with_suffix('.release').write_text('release')
        stdout,stderr = waiting.communicate(); assert waiting.returncode == 0, stderr
        assert lock.stat().st_ino == original_inode
    finally:
        marker.with_suffix('.release').write_text('release')
        for child in (holder, waiting):
            if child is not None:
                if child.poll() is None: child.kill()
                child.wait()
    # Session-only never touches Git or the published platform checkout.
    (platform / 'platform-file').write_text('dirty platform\n')
    only_operation = 'crew-handoff:session-only:isolated-recovery'
    only_namespace = hashlib.sha256(only_operation.encode()).hexdigest()
    only_attempt = platform / '.scaffold' / ('crew-native-'+only_namespace) / 'uploads' / uuid.uuid4().hex
    (only_attempt / 'native/by-cwd').mkdir(parents=True)
    only_id, _ = recovery_journal(only_operation)
    (only_attempt / 'native/by-cwd/A.jsonl').write_bytes(source_journal)
    only_descriptor = dict(descriptor, operation_id=only_operation, namespace=only_namespace, target_native_id=only_id, repository=None, session_only=True, published_cwd=str(platform))
    only_path = only_attempt / 'descriptor.json'; only_path.write_text(json.dumps(only_descriptor))
    only = subprocess.run([sys.executable,str(helper),str(only_path),str(only_attempt)],env=env,capture_output=True)
    assert only.returncode == 0, only.stderr
    only_receipt = json.loads(only.stdout); only_cwd = pathlib.Path(only_receipt['cwd'])
    assert only_cwd == platform
    assert (only_cwd / 'platform-file').read_text() == 'dirty platform\n'
    only_selector = pathlib.Path(only_receipt['launch_selector'])
    assert only_selector.name == 'recovered-' + only_namespace + '.jsonl'
    assert only_receipt['launch_selector'] != receipt['launch_selector']
    # The legacy writer can keep writing the original basename in the SAME cwd.
    old_same_cwd = only_selector.with_name('A.jsonl'); old_same_cwd.write_bytes(source_journal)
    only_writer = subprocess.Popen([sys.executable,'-c','import pathlib,sys,time\np=pathlib.Path(sys.argv[1])\nwith p.open("ab",buffering=0) as journal:\n while True:\n  journal.write(b"{\\"type\\":\\"message\\",\\"id\\":\\"legacy-live\\"}\\n");time.sleep(.005)',str(old_same_cwd)])
    try:
        only_imported = only_selector.read_bytes()
        only_again = subprocess.run([sys.executable,str(helper),str(only_path),str(only_attempt)],env=env,capture_output=True)
        assert only_again.returncode == 0 and json.loads(only_again.stdout) == only_receipt, only_again.stderr
        assert only_selector.read_bytes() == only_imported
    finally:
        only_writer.terminate(); only_writer.wait()
    assert old_same_cwd.read_bytes().startswith(source_journal)
    assert (platform / 'platform-file').read_text() == 'dirty platform\n'
finally:
    writer.terminate(); writer.wait()
"#;
        let result = std::process::Command::new("python3")
            .args(["-c", fixture, root.to_str().unwrap(), helper.to_str().unwrap()])
            .output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }

    #[cfg(unix)]
    #[test]
    fn materializer_accepts_session_only_in_published_checkout() {
        materializer_process_at(true);
    }

    #[cfg(unix)]
    #[test]
    fn materializer_process_is_exact_idempotent_and_rejects_mismatch() {
        materializer_process_at(false);
    }

    // Executes the sandbox's POSIX materializer, including private modes and symlink guards.
    #[cfg(unix)]
    fn materializer_process_at(session_only: bool) {
        use std::os::unix::fs::PermissionsExt as _;
        use std::process::Command;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("workspace");
        let workspace = root.join("ashler-platform");
        let staging = workspace.join(".scaffold/omp-handoff-staging/by-cwd");
        std::fs::create_dir_all(&staging).unwrap();
        let remote_cwd = if session_only { workspace.clone() } else { root.join("crew-handoff/nested") };
        std::fs::create_dir_all(&remote_cwd).unwrap();
        let staged = staging.join("native-1.jsonl");
        let bytes = b"{\"type\":\"session\",\"version\":3,\"id\":\"native-1\",\"timestamp\":\"2026-08-16T00:00:00.000Z\",\"cwd\":\"/repo\"}\n{\"type\":\"message\",\"id\":\"message-1\"}\n";
        let sha = format!("{:x}", sha2::Sha256::digest(bytes));
        let runtime = root.join(".scaffold");
        let profile_dir = runtime.join("omp-inference");
        let profile_agent_dir = root.join("runtime-home/.omp/profiles/scaffold-host/agent");
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::create_dir_all(&profile_agent_dir).unwrap();
        std::fs::set_permissions(&profile_agent_dir, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let models_path = profile_agent_dir.join("models.yml");
        std::fs::write(&models_path, "providers: {}\n").unwrap();
        std::fs::set_permissions(&models_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let profile_path = profile_dir.join("profile.json");
        std::fs::write(
            &profile_path,
            serde_json::to_vec(&serde_json::json!({
                "profile": "scaffold-host",
                "model": "scaffold-openai/gpt-5.6-sol",
                "modelsPath": models_path,
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&profile_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let script = VERIFY_HANDOFF_PYTHON.replace("/workspace", root.to_str().unwrap());
        let run = |script: &str| {
            Command::new("python3")
                .env("SCAFFOLD_RUNTIME_DIR", &runtime)
                .env("HOME", root.join("runtime-home"))
                .env_remove("XDG_DATA_HOME")
                .env_remove("PI_CONFIG_DIR")
                .args([
                    "-c",
                    script,
                    workspace
                        .join(".scaffold/omp-handoff-staging")
                        .to_str()
                        .unwrap(),
                    "by-cwd/native-1.jsonl",
                    &sha,
                    &bytes.len().to_string(),
                    "native-1",
                    "/repo",
                    remote_cwd.to_str().unwrap(),
                ])
                .output()
                .unwrap()
        };
        let probe = |cwd: &std::path::Path| {
            Command::new("python3")
                .env("SCAFFOLD_RUNTIME_DIR", &runtime)
                .env("HOME", root.join("runtime-home"))
                .env_remove("XDG_DATA_HOME")
                .env_remove("PI_CONFIG_DIR")
                .args([
                    "-c",
                    &script,
                    "--probe",
                    "by-cwd/native-1.jsonl",
                    &sha,
                    &bytes.len().to_string(),
                    "native-1",
                    "/repo",
                    cwd.to_str().unwrap(),
                ])
                .output()
                .unwrap()
        };
        std::fs::set_permissions(&profile_agent_dir, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let absent = probe(&remote_cwd);
        assert!(
            absent.status.success(),
            "{}",
            String::from_utf8_lossy(&absent.stderr)
        );
        assert_eq!(absent.stdout, b"absent\n");
        assert!(!profile_agent_dir.join("sessions").exists());
        assert_eq!(
            std::fs::metadata(&profile_agent_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        let prospective = root.join("crew-handoff/not-created");
        assert_eq!(probe(&prospective).stdout, b"absent\n");
        assert!(!prospective.exists());
        std::fs::write(&staged, bytes).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600)).unwrap();
        let first = run(&script);
        assert!(
            first.status.success(),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert!(!staged.exists(), "staged file must be consumed");
        let sessions = profile_agent_dir.join("sessions");
        let target = std::fs::read_dir(&sessions)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("native-1.jsonl"))
            .find(|path| path.is_file())
            .expect("materializer must use OMP's cwd-scoped session directory");
        assert!(
            target
                .parent()
                .and_then(std::path::Path::file_name)
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with("-tmp"))
        );
        let transformed = std::fs::read(&target).unwrap();
        assert_ne!(transformed, bytes);
        let session: serde_json::Value =
            serde_json::from_slice(transformed.split(|byte| *byte == b'\n').next().unwrap())
                .unwrap();
        assert_eq!(session["id"], "native-1");
        assert_eq!(
            session["cwd"],
            remote_cwd
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
        assert!(transformed.ends_with(b"{\"type\":\"message\",\"id\":\"message-1\"}\n"));
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(target.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let present = probe(&remote_cwd);
        assert!(
            present.status.success(),
            "{}",
            String::from_utf8_lossy(&present.stderr)
        );
        assert_eq!(present.stdout, b"present\n");
        assert_eq!(std::fs::read(&target).unwrap(), transformed);
        let renamed = target.with_file_name("renamed.jsonl");
        std::fs::rename(&target, &renamed).unwrap();
        assert_eq!(probe(&remote_cwd).stdout, b"present\n");
        let relocated = sessions.join("other-cwd");
        std::fs::create_dir(&relocated).unwrap();
        std::fs::rename(&renamed, relocated.join("renamed.jsonl")).unwrap();
        assert_eq!(probe(&remote_cwd).stdout, b"present\n");
        std::fs::rename(relocated.join("renamed.jsonl"), &target).unwrap();
        let saved = profile_agent_dir.join("saved-native.jsonl");
        std::fs::rename(&target, &saved).unwrap();
        let unknown = sessions.join("unknown.jsonl");
        std::fs::write(&unknown, b"not a complete native header\n").unwrap();
        assert_eq!(probe(&prospective).status.code(), Some(25));
        std::fs::remove_file(&unknown).unwrap();
        std::fs::rename(&saved, &target).unwrap();

        // Re-uploading identical bytes is an exact, safe no-op at the destination.
        std::fs::write(&staged, bytes).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(run(&script).status.success());
        assert_eq!(std::fs::read(&target).unwrap(), transformed);

        // An existing mismatched destination fails closed and still cleans staging.
        std::fs::write(&target, b"different").unwrap();
        std::fs::write(&staged, bytes).unwrap();
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mismatch = run(&script);
        assert!(!mismatch.status.success());
        assert!(!staged.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"different");
        std::fs::rename(&sessions, profile_agent_dir.join("saved-sessions")).unwrap();
        std::os::unix::fs::symlink(profile_agent_dir.join("saved-sessions"), &sessions).unwrap();
        assert_eq!(probe(&remote_cwd).status.code(), Some(25));
        assert_eq!(std::fs::read(&target).unwrap(), b"different");
    }
}
