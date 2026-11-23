//! Vault Connections HTTP client & Ed25519 device enrollment/grant signing.
//!
//! Matches `edge/src/vault-routes.ts`, `edge/vault/src/api.ts`, and
//! `edge/vault/src/devices.ts` verbatim:
//! - Cloud runners reuse [`RunnerIdentity`]'s Ed25519 key & `deviceId`.
//! - Laptops lazily load or generate an Ed25519 PKCS#8 key at
//!   `{data_dir}/vault-device-key` (`0600`), enroll via
//!   `POST /vault/{orgId}/devices/enroll`, and re-enroll once if a grant
//!   returns `device_unknown` / `not_enrolled`.
//! - Grant message format:
//!   `"zeron-vault-grant\n{userId}\n{deviceId}\n{subject}\n{ts}"`
//!   where `subject` is `"conn:mcp:{id}"` (single) or
//!   `"conns:{sorted_ids.join(',')}"` (batch).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};

use zeron_proto::{
    McpAuthKind, McpServerStatus, McpServerView, McpStorage, McpTransport,
};
use zeron_rpc::TokenSource;

use crate::http_error::describe_http_error;
use crate::now_ms;
use crate::runner::{EdgeErrorBody, RunnerIdentity, write_private_atomic};

const GRANT_DOMAIN: &str = "zeron-vault-grant";
const LAPTOP_KEY_FILE: &str = "vault-device-key";
/// Refresh cached OAuth grants when less than 60s remains before expiry.
const GRANT_EXPIRY_SLACK_MS: i64 = 60_000;
/// Cache static (non-expiring) grants for 5 minutes; mutations invalidate immediately.
const STATIC_GRANT_CACHE_TTL: Duration = Duration::from_secs(300);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Exact Ed25519 message verified by `edge/vault/src/devices.ts:grantMessage`.
pub fn vault_grant_message(user_id: &str, device_id: &str, subject: &str, ts: i64) -> String {
    format!("{GRANT_DOMAIN}\n{user_id}\n{device_id}\n{subject}\n{ts}")
}

/// Canonical Vault connection id for an MCP server (`mcp:{id}`).
pub fn mcp_connection_id(id: &str) -> String {
    if id.starts_with("mcp:") {
        id.to_owned()
    } else {
        format!("mcp:{id}")
    }
}

/// Strip the `mcp:` prefix from a Vault connection id.
pub fn strip_mcp_prefix(id: &str) -> &str {
    id.strip_prefix("mcp:").unwrap_or(id)
}

fn encoded_mcp_connection_id(id: &str) -> String {
    format!("mcp%3A{}", strip_mcp_prefix(id))
}

/// Error returned by Vault Connections HTTP calls.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct VaultClientError {
    pub code: Option<String>,
    pub status: Option<u16>,
    pub message: String,
}

impl VaultClientError {
    fn other(message: impl Into<String>) -> Self {
        Self {
            code: None,
            status: None,
            message: message.into(),
        }
    }

    pub fn is_not_enrolled(&self) -> bool {
        matches!(
            self.code.as_deref(),
            Some("device_unknown") | Some("not_enrolled")
        )
    }

    pub fn is_device_revoked(&self) -> bool {
        self.code.as_deref() == Some("device_revoked")
    }

    pub fn is_needs_reconnect(&self) -> bool {
        self.code.as_deref() == Some("needs_reconnect")
    }

    /// Refused by the device's replay fence (`ts` not newer than its last):
    /// another signer of this device's (a provider grant) got there first.
    /// Signing again, later, goes through.
    pub fn is_stale(&self) -> bool {
        self.code.as_deref() == Some("stale")
    }
}

/// Wire type matching `VaultConnectionsView` in `edge/vault/src/api.ts`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConnectionsView {
    #[serde(default)]
    pub disabled: bool,
    #[serde(default)]
    pub connections: Vec<VaultConnectionView>,
}

/// Wire type matching `VaultConnectionView` in `edge/vault/src/api.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConnectionView {
    pub id: String,
    #[serde(default)]
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub preset: Option<String>,
    pub url: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auth_kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub header_keys: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub authorized_devices: Vec<String>,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl VaultConnectionView {
    /// Convert a Vault connection view to a redacted [`McpServerView`].
    pub fn to_mcp_server_view(&self) -> McpServerView {
        let id = strip_mcp_prefix(&self.id).to_owned();
        let transport = match self.transport.as_str() {
            "sse" => McpTransport::Sse,
            _ => McpTransport::Http,
        };
        let auth_kind = match self.auth_kind.as_str() {
            "none" => McpAuthKind::None,
            "oauth" => McpAuthKind::OAuth,
            "headers" => {
                if self.header_keys.len() == 1
                    && self.header_keys[0].eq_ignore_ascii_case("authorization")
                {
                    McpAuthKind::Bearer
                } else {
                    McpAuthKind::Headers
                }
            }
            _ => McpAuthKind::OAuth,
        };
        let status = if !self.enabled {
            McpServerStatus::Disabled
        } else {
            match self.status.as_str() {
                "connected" => McpServerStatus::Connected,
                "needs_auth" => McpServerStatus::NeedsAuth,
                "reauth_required" => McpServerStatus::ReauthRequired,
                "error" => McpServerStatus::Error,
                _ => McpServerStatus::NeedsAuth,
            }
        };
        McpServerView {
            id,
            name: self.name.clone(),
            preset: self.preset.clone(),
            transport,
            url: Some(self.url.clone()),
            command: None,
            args: Vec::new(),
            env_keys: Vec::new(),
            auth_kind,
            header_keys: self.header_keys.clone(),
            enabled: self.enabled,
            storage: McpStorage::Vault,
            status,
            account_label: self.account_label.clone(),
            last_error: self.last_error.clone(),
            scopes: self.scopes.clone(),
            updated_at: self.updated_at,
        }
    }
}

/// Wire type matching `VaultConnectionMaterial` in `edge/vault/src/api.ts`.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VaultConnectionMaterial {
    Headers {
        headers: BTreeMap<String, String>,
    },
    #[serde(rename_all = "camelCase")]
    Oauth {
        access_token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        token_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<i64>,
        token_endpoint: String,
        client_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        client_secret: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        scopes: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resource: Option<String>,
    },
}

impl std::fmt::Debug for VaultConnectionMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Headers { headers } => f
                .debug_struct("Headers")
                .field("header_keys", &headers.keys().collect::<Vec<_>>())
                .finish(),
            Self::Oauth {
                token_endpoint,
                client_id,
                expires_at,
                scopes,
                ..
            } => f
                .debug_struct("Oauth")
                .field("token_endpoint", token_endpoint)
                .field("client_id", client_id)
                .field("expires_at", expires_at)
                .field("scopes", scopes)
                .finish_non_exhaustive(),
        }
    }
}

/// Wire type matching `PutConnectionRequest` in `edge/vault/src/api.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultPutConnectionRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    pub auth_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<VaultConnectionMaterial>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorized_devices: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
}

/// Wire type matching `UpdateConnectionRequest` in `edge/vault/src/api.ts`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultUpdateConnectionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorized_devices: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_label: Option<Option<String>>,
}

/// Wire type matching `ConnectionGrant` in `edge/vault/src/api.ts`.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConnectionGrant {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub generation: i64,
}

impl std::fmt::Debug for VaultConnectionGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultConnectionGrant")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("transport", &self.transport)
            .field("header_keys", &self.headers.keys().collect::<Vec<_>>())
            .field("expires_at", &self.expires_at)
            .field("generation", &self.generation)
            .finish()
    }
}

/// Wire type matching `ConnectionGrantManyReply` in `edge/vault/src/api.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConnectionGrantManyReply {
    #[serde(default)]
    pub grants: Vec<VaultConnectionGrant>,
    #[serde(default)]
    pub missing: Vec<VaultConnectionGrantMissing>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConnectionGrantMissing {
    pub id: String,
    pub error: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EnrollDeviceBody<'a> {
    device_id: &'a str,
    kind: &'a str,
    public_key: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionGrantBody {
    id: String,
    device_id: String,
    ts: i64,
    sig: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    force_refresh: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionGrantManyBody {
    ids: Vec<String>,
    device_id: String,
    ts: i64,
    sig: String,
}

#[derive(Debug, Deserialize)]
struct DeleteReply {
    #[serde(default)]
    deleted: bool,
}

/// Lazily loaded/generated Ed25519 keypair for a laptop/headless device.
pub struct LaptopDeviceKey {
    device_id: String,
    key_path: PathBuf,
    key: Mutex<Option<Arc<Ed25519KeyPair>>>,
    last_ts: Mutex<i64>,
    enrolled: AtomicBool,
}

impl LaptopDeviceKey {
    pub fn new(data_dir: &Path, device_id: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            key_path: data_dir.join(LAPTOP_KEY_FILE),
            key: Mutex::new(None),
            last_ts: Mutex::new(0),
            enrolled: AtomicBool::new(false),
        }
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Load `{data_dir}/vault-device-key` if present, otherwise generate a new
    /// Ed25519 PKCS#8 keypair and persist it atomically at `0600`.
    pub fn keypair(&self) -> Result<Arc<Ed25519KeyPair>, VaultClientError> {
        let mut slot = lock(&self.key);
        if let Some(existing) = slot.as_ref() {
            return Ok(existing.clone());
        }
        let pair = match std::fs::read(&self.key_path) {
            Ok(bytes) => Ed25519KeyPair::from_pkcs8(&bytes).map_err(|err| {
                VaultClientError::other(format!(
                    "invalid vault device key {}: {err}",
                    self.key_path.display()
                ))
            })?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.generate_and_persist()?
            }
            Err(err) => {
                return Err(VaultClientError::other(format!(
                    "read vault device key {}: {err}",
                    self.key_path.display()
                )));
            }
        };
        let pair = Arc::new(pair);
        *slot = Some(pair.clone());
        Ok(pair)
    }

    /// Generate a fresh Ed25519 keypair (e.g., after a previous key was revoked)
    /// and atomically replace `{data_dir}/vault-device-key` at `0600`.
    pub fn rotate(&self) -> Result<Arc<Ed25519KeyPair>, VaultClientError> {
        let mut slot = lock(&self.key);
        let pair = Arc::new(self.generate_and_persist()?);
        *slot = Some(pair.clone());
        self.enrolled.store(false, Ordering::Release);
        *lock(&self.last_ts) = 0;
        Ok(pair)
    }

    fn generate_and_persist(&self) -> Result<Ed25519KeyPair, VaultClientError> {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| VaultClientError::other("generate Ed25519 vault device key failed"))?;
        write_private_atomic(&self.key_path, pkcs8.as_ref()).map_err(|err| {
            VaultClientError::other(format!(
                "write vault device key {}: {err}",
                self.key_path.display()
            ))
        })?;
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|err| VaultClientError::other(format!("decode generated vault key: {err}")))
    }

    pub fn public_key_b64url(&self) -> Result<String, VaultClientError> {
        let pair = self.keypair()?;
        Ok(B64URL.encode(pair.public_key().as_ref()))
    }

    pub fn next_ts(&self) -> i64 {
        let mut last = lock(&self.last_ts);
        let ts = now_ms().max(*last + 1);
        *last = ts;
        ts
    }

    pub fn sign(&self, message: &str) -> Result<String, VaultClientError> {
        let pair = self.keypair()?;
        Ok(B64URL.encode(pair.sign(message.as_bytes()).as_ref()))
    }

    pub fn is_enrolled(&self) -> bool {
        self.enrolled.load(Ordering::Acquire)
    }

    pub fn set_enrolled(&self, enrolled: bool) {
        self.enrolled.store(enrolled, Ordering::Release);
    }
}

/// Ed25519 signer used to authenticate Vault connection grant requests.
#[derive(Clone)]
pub enum VaultDeviceSigner {
    Runner(Arc<RunnerIdentity>),
    Laptop(Arc<LaptopDeviceKey>),
}

impl VaultDeviceSigner {
    pub fn device_id(&self) -> &str {
        match self {
            Self::Runner(id) => id.device_id(),
            Self::Laptop(key) => key.device_id(),
        }
    }

    pub fn is_laptop(&self) -> bool {
        matches!(self, Self::Laptop(_))
    }

    pub fn next_ts(&self) -> i64 {
        match self {
            Self::Runner(id) => id.next_ts(),
            Self::Laptop(key) => key.next_ts(),
        }
    }

    pub fn sign_grant(
        &self,
        user_id: &str,
        subject: &str,
    ) -> Result<(i64, String), VaultClientError> {
        let ts = self.next_ts();
        let message = vault_grant_message(user_id, self.device_id(), subject, ts);
        let sig = match self {
            Self::Runner(id) => id.sign(&message),
            Self::Laptop(key) => key.sign(&message)?,
        };
        Ok((ts, sig))
    }
}

#[derive(Clone)]
struct CachedGrant {
    grant: VaultConnectionGrant,
    fetched_at: Instant,
}

impl CachedGrant {
    fn is_fresh(&self, now: i64) -> bool {
        match self.grant.expires_at {
            Some(exp) => exp - now > GRANT_EXPIRY_SLACK_MS,
            None => self.fetched_at.elapsed() < STATIC_GRANT_CACHE_TTL,
        }
    }
}

/// Client for `/vault/{orgId}/connections/*` and laptop device enrollment.
pub struct VaultClient {
    edge_url: String,
    org_id: String,
    user_id: String,
    tokens: Arc<dyn TokenSource>,
    signer: VaultDeviceSigner,
    http: reqwest::Client,
    grants: Mutex<HashMap<String, CachedGrant>>,
    enroll_lock: tokio::sync::Mutex<()>,
    /// Held from signing a grant request until its answer: the vault's
    /// replay fence takes only a `ts` newer than the device's last, so two
    /// grants in flight at once (a run's servers starting together) must
    /// reach it in the order they were signed.
    grant_lock: tokio::sync::Mutex<()>,
}

impl VaultClient {
    pub fn new(
        edge_url: impl Into<String>,
        org_id: impl Into<String>,
        user_id: impl Into<String>,
        tokens: Arc<dyn TokenSource>,
        signer: VaultDeviceSigner,
        http: reqwest::Client,
    ) -> Self {
        Self {
            edge_url: edge_url.into().trim_end_matches('/').to_owned(),
            org_id: org_id.into(),
            user_id: user_id.into(),
            tokens,
            signer,
            http,
            grants: Mutex::new(HashMap::new()),
            enroll_lock: tokio::sync::Mutex::new(()),
            grant_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn org_id(&self) -> &str {
        &self.org_id
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn device_id(&self) -> &str {
        self.signer.device_id()
    }

    pub fn is_runner(&self) -> bool {
        !self.signer.is_laptop()
    }

    fn base_url(&self) -> String {
        format!("{}/vault/{}", self.edge_url, self.org_id)
    }

    async fn bearer(&self) -> Result<String, VaultClientError> {
        self.tokens
            .token()
            .await
            .map_err(|err| VaultClientError::other(format!("vault bearer unavailable: {err}")))
    }

    /// Invalidate any cached grant for `id`.
    pub fn invalidate_grant(&self, id: &str) {
        lock(&self.grants).remove(strip_mcp_prefix(id));
    }

    /// Clear all cached grants.
    pub fn clear_grants(&self) {
        lock(&self.grants).clear();
    }

    /// Ensure this laptop device's Ed25519 key is enrolled with Vault.
    /// No-op for Cloud runners (enrolled during machine provisioning).
    pub async fn ensure_enrolled(&self) -> Result<(), VaultClientError> {
        let VaultDeviceSigner::Laptop(laptop) = &self.signer else {
            return Ok(());
        };
        if laptop.is_enrolled() {
            return Ok(());
        }
        let _guard = self.enroll_lock.lock().await;
        if laptop.is_enrolled() {
            return Ok(());
        }
        self.enroll_laptop(laptop).await
    }

    async fn enroll_laptop(&self, laptop: &LaptopDeviceKey) -> Result<(), VaultClientError> {
        let public_key = laptop.public_key_b64url()?;
        self.send_enroll(laptop.device_id(), &public_key).await?;
        laptop.set_enrolled(true);
        Ok(())
    }

    async fn send_enroll(&self, device_id: &str, public_key: &str) -> Result<(), VaultClientError> {
        let bearer = self.bearer().await?;
        let url = format!("{}/devices/enroll", self.base_url());
        let body = EnrollDeviceBody {
            device_id,
            kind: "laptop",
            public_key,
        };
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(parse_error_response(resp).await)
        }
    }

    /// `GET /vault/{orgId}/connections?kind=mcp`
    pub async fn list_connections(&self) -> Result<VaultConnectionsView, VaultClientError> {
        let bearer = self.bearer().await?;
        let url = format!("{}/connections?kind=mcp", self.base_url());
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&bearer)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        parse_json_response(resp).await
    }

    /// `PUT /vault/{orgId}/connections/mcp%3A{id}`
    pub async fn put_connection(
        &self,
        id: &str,
        req: &VaultPutConnectionRequest,
    ) -> Result<VaultConnectionView, VaultClientError> {
        let bearer = self.bearer().await?;
        let url = format!("{}/connections/{}", self.base_url(), encoded_mcp_connection_id(id));
        let resp = self
            .http
            .put(&url)
            .bearer_auth(&bearer)
            .json(req)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        let view: VaultConnectionView = parse_json_response(resp).await?;
        self.invalidate_grant(id);
        Ok(view)
    }

    /// `PATCH /vault/{orgId}/connections/mcp%3A{id}`
    pub async fn update_connection(
        &self,
        id: &str,
        patch: &VaultUpdateConnectionRequest,
    ) -> Result<VaultConnectionView, VaultClientError> {
        let bearer = self.bearer().await?;
        let url = format!("{}/connections/{}", self.base_url(), encoded_mcp_connection_id(id));
        let resp = self
            .http
            .patch(&url)
            .bearer_auth(&bearer)
            .json(patch)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        let view: VaultConnectionView = parse_json_response(resp).await?;
        self.invalidate_grant(id);
        Ok(view)
    }

    /// `DELETE /vault/{orgId}/connections/mcp%3A{id}`
    pub async fn delete_connection(&self, id: &str) -> Result<bool, VaultClientError> {
        let bearer = self.bearer().await?;
        let url = format!("{}/connections/{}", self.base_url(), encoded_mcp_connection_id(id));
        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&bearer)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        let reply: DeleteReply = parse_json_response(resp).await?;
        self.invalidate_grant(id);
        Ok(reply.deleted)
    }

    /// `POST /vault/{orgId}/connections/mcp%3A{id}/disconnect`
    pub async fn disconnect_auth(&self, id: &str) -> Result<VaultConnectionView, VaultClientError> {
        let bearer = self.bearer().await?;
        let url = format!(
            "{}/connections/{}/disconnect",
            self.base_url(),
            encoded_mcp_connection_id(id)
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&bearer)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        let view: VaultConnectionView = parse_json_response(resp).await?;
        self.invalidate_grant(id);
        Ok(view)
    }

    /// `POST /vault/{orgId}/connections/grant` with in-memory expiry cache and
    /// automatic single re-enrollment retry on laptops.
    pub async fn grant_connection(
        &self,
        id: &str,
        force_refresh: bool,
    ) -> Result<VaultConnectionGrant, VaultClientError> {
        let short_id = strip_mcp_prefix(id);
        if !force_refresh {
            let now = now_ms();
            if let Some(cached) = lock(&self.grants).get(short_id)
                && cached.is_fresh(now)
            {
                return Ok(cached.grant.clone());
            }
        }

        self.ensure_enrolled().await?;
        let full_id = mcp_connection_id(short_id);
        let mut result = self.send_grant_connection(&full_id, force_refresh).await;
        if result.as_ref().is_err_and(VaultClientError::is_stale) {
            result = self.send_grant_connection(&full_id, force_refresh).await;
        }
        let grant = match result {
            Ok(grant) => grant,
            Err(err) if err.is_not_enrolled() && self.signer.is_laptop() => {
                if let VaultDeviceSigner::Laptop(laptop) = &self.signer {
                    laptop.set_enrolled(false);
                }
                self.ensure_enrolled().await?;
                self.send_grant_connection(&full_id, force_refresh).await?
            }
            Err(err) => {
                if err.is_needs_reconnect() {
                    self.invalidate_grant(short_id);
                }
                return Err(err);
            }
        };

        lock(&self.grants).insert(
            short_id.to_owned(),
            CachedGrant {
                grant: grant.clone(),
                fetched_at: Instant::now(),
            },
        );
        Ok(grant)
    }

    async fn send_grant_connection(
        &self,
        full_id: &str,
        force_refresh: bool,
    ) -> Result<VaultConnectionGrant, VaultClientError> {
        let bearer = self.bearer().await?;
        let _in_order = self.grant_lock.lock().await;
        let subject = format!("conn:{full_id}");
        let (ts, sig) = self.signer.sign_grant(&self.user_id, &subject)?;
        let body = ConnectionGrantBody {
            id: full_id.to_owned(),
            device_id: self.signer.device_id().to_owned(),
            ts,
            sig,
            force_refresh: force_refresh.then_some(true),
        };
        let url = format!("{}/connections/grant", self.base_url());
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        parse_json_response(resp).await
    }

    /// `POST /vault/{orgId}/connections/grants` for a batch of server IDs.
    pub async fn grant_connections(
        &self,
        ids: &[String],
    ) -> Result<VaultConnectionGrantManyReply, VaultClientError> {
        let mut sorted: Vec<String> = ids.iter().map(|id| mcp_connection_id(id)).collect();
        sorted.sort();
        sorted.dedup();
        if sorted.is_empty() {
            return Ok(VaultConnectionGrantManyReply {
                grants: Vec::new(),
                missing: Vec::new(),
            });
        }

        self.ensure_enrolled().await?;
        let mut result = self.send_grant_connections(&sorted).await;
        if result.as_ref().is_err_and(VaultClientError::is_stale) {
            result = self.send_grant_connections(&sorted).await;
        }
        let reply = match result {
            Ok(reply) => reply,
            Err(err) if err.is_not_enrolled() && self.signer.is_laptop() => {
                if let VaultDeviceSigner::Laptop(laptop) = &self.signer {
                    laptop.set_enrolled(false);
                }
                self.ensure_enrolled().await?;
                self.send_grant_connections(&sorted).await?
            }
            Err(err) => return Err(err),
        };

        let fetched_at = Instant::now();
        let mut cache = lock(&self.grants);
        for grant in &reply.grants {
            cache.insert(
                strip_mcp_prefix(&grant.id).to_owned(),
                CachedGrant {
                    grant: grant.clone(),
                    fetched_at,
                },
            );
        }
        Ok(reply)
    }

    async fn send_grant_connections(
        &self,
        sorted_full_ids: &[String],
    ) -> Result<VaultConnectionGrantManyReply, VaultClientError> {
        let bearer = self.bearer().await?;
        let _in_order = self.grant_lock.lock().await;
        let subject = format!("conns:{}", sorted_full_ids.join(","));
        let (ts, sig) = self.signer.sign_grant(&self.user_id, &subject)?;
        let body = ConnectionGrantManyBody {
            ids: sorted_full_ids.to_vec(),
            device_id: self.signer.device_id().to_owned(),
            ts,
            sig,
        };
        let url = format!("{}/connections/grants", self.base_url());
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| VaultClientError::other(describe_http_error(e)))?;
        parse_json_response(resp).await
    }
}

async fn parse_json_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, VaultClientError> {
    if !resp.status().is_success() {
        return Err(parse_error_response(resp).await);
    }
    resp.json::<T>()
        .await
        .map_err(|e| VaultClientError::other(format!("decode vault response: {e}")))
}

async fn parse_error_response(resp: reqwest::Response) -> VaultClientError {
    let status = resp.status();
    let bytes = resp.bytes().await.unwrap_or_default();
    if let Ok(parsed) = serde_json::from_slice::<EdgeErrorBody>(&bytes) {
        let code = (!parsed.error.is_empty()).then_some(parsed.error);
        let message = if !parsed.message.is_empty() {
            parsed.message
        } else if let Some(c) = &code {
            format!("vault error: {c} (HTTP {})", status.as_u16())
        } else {
            format!("vault HTTP {}", status.as_u16())
        };
        return VaultClientError {
            code,
            status: Some(status.as_u16()),
            message,
        };
    }
    VaultClientError {
        code: None,
        status: Some(status.as_u16()),
        message: format!("vault HTTP {}", status.as_u16()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{ED25519, UnparsedPublicKey};

    #[test]
    fn grant_message_matches_edge_vault_devices_ts_exactly() {
        let single = vault_grant_message("user_123", "dev_abc", "conn:mcp:linear", 1700000000123);
        assert_eq!(
            single,
            "zeron-vault-grant\nuser_123\ndev_abc\nconn:mcp:linear\n1700000000123"
        );

        let batch = vault_grant_message(
            "user_123",
            "dev_abc",
            "conns:mcp:github,mcp:linear,mcp:slack",
            1700000000456,
        );
        assert_eq!(
            batch,
            "zeron-vault-grant\nuser_123\ndev_abc\nconns:mcp:github,mcp:linear,mcp:slack\n1700000000456"
        );
    }

    #[test]
    fn laptop_device_key_persists_0600_signs_verifiably_and_monotonic_ts() {
        let dir = tempfile::tempdir().unwrap();
        let laptop = LaptopDeviceKey::new(dir.path(), "dev-laptop-1");
        let pub_b64 = laptop.public_key_b64url().unwrap();
        let pub_bytes = B64URL.decode(&pub_b64).unwrap();
        assert_eq!(pub_bytes.len(), 32);

        let key_path = dir.path().join(LAPTOP_KEY_FILE);
        assert!(key_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let signer = VaultDeviceSigner::Laptop(Arc::new(laptop));
        let (ts1, sig1) = signer.sign_grant("user-1", "conn:mcp:slack").unwrap();
        let (ts2, _sig2) = signer.sign_grant("user-1", "conn:mcp:slack").unwrap();
        assert!(ts2 > ts1, "timestamps must be strictly monotonic");

        let msg = vault_grant_message("user-1", "dev-laptop-1", "conn:mcp:slack", ts1);
        let sig_bytes = B64URL.decode(&sig1).unwrap();
        UnparsedPublicKey::new(&ED25519, &pub_bytes)
            .verify(msg.as_bytes(), &sig_bytes)
            .expect("Ed25519 signature must verify against raw 32-byte public key");

        // Reloading from disk recovers the exact same public key.
        let reloaded = LaptopDeviceKey::new(dir.path(), "dev-laptop-1");
        assert_eq!(reloaded.public_key_b64url().unwrap(), pub_b64);
    }
}
