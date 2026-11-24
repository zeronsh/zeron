//! Local-only MCP server store (0600 atomic JSON persistence) with secret
//! redaction and single-flight local OAuth token refresh.
//!
//! Storage path isolation:
//! - [`WorkspaceScope::Local`] (signed out): `{profile.store_root}/mcp-servers.json`
//!   so the local profile stays completely isolated from synced accounts.
//! - [`WorkspaceScope::Synced`] / [`WorkspaceScope::Development`]:
//!   `{data_dir}/mcp-local-servers.json` (stores `Stdio` servers and any remote
//!   servers explicitly marked `local_only: true`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use zeron_proto::{
    McpAuthKind, McpServerStatus, McpServerView, McpStorage, McpTransport, WorkspaceScope,
};

use crate::now_ms;
use crate::profile::EngineProfile;
use crate::runner::write_private_atomic;

const STORE_VERSION: u32 = 1;
/// Refresh a local OAuth access token when it has less than 60s remaining.
const OAUTH_REFRESH_LEAD_MS: i64 = 60_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Resolve the local store file path for a profile + device data dir.
pub fn store_path_for(profile: &EngineProfile, data_dir: &Path) -> PathBuf {
    match profile.scope() {
        WorkspaceScope::Local => profile.store_root().join("mcp-servers.json"),
        WorkspaceScope::Synced | WorkspaceScope::Development => {
            data_dir.join("mcp-local-servers.json")
        }
    }
}

/// Persisted OAuth token bundle for a locally-stored remote MCP server.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalOAuthState {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    /// Unix ms timestamp when `access_token` expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

impl std::fmt::Debug for LocalOAuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalOAuthState")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_type", &self.token_type)
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("scopes", &self.scopes)
            .field("resource", &self.resource)
            .finish()
    }
}

/// Full local MCP server record persisted on disk (0600).
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LocalMcpServer {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    pub transport: McpTransport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub auth_kind: McpAuthKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<LocalOAuthState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_secret: Option<String>,
    #[serde(default)]
    pub oauth_scopes: Vec<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default)]
    pub reauth_required: bool,
    pub updated_at: i64,
}

impl Default for LocalMcpServer {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            preset: None,
            transport: McpTransport::Http,
            url: None,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            auth_kind: McpAuthKind::OAuth,
            bearer_token: None,
            headers: BTreeMap::new(),
            oauth: None,
            oauth_client_id: None,
            oauth_client_secret: None,
            oauth_scopes: Vec::new(),
            enabled: true,
            account_label: None,
            last_error: None,
            reauth_required: false,
            updated_at: 0,
        }
    }
}

impl std::fmt::Debug for LocalMcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMcpServer")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("preset", &self.preset)
            .field("transport", &self.transport)
            .field("url", &self.url)
            .field("command", &self.command)
            .field("args", &self.args)
            .field("env_keys", &self.env.keys().collect::<Vec<_>>())
            .field("auth_kind", &self.auth_kind)
            .field(
                "bearer_token",
                &self.bearer_token.as_ref().map(|_| "<redacted>"),
            )
            .field("header_keys", &self.headers.keys().collect::<Vec<_>>())
            .field("oauth", &self.oauth)
            .field("oauth_client_id", &self.oauth_client_id)
            .field(
                "oauth_client_secret",
                &self.oauth_client_secret.as_ref().map(|_| "<redacted>"),
            )
            .field("oauth_scopes", &self.oauth_scopes)
            .field("enabled", &self.enabled)
            .field("account_label", &self.account_label)
            .field("last_error", &self.last_error)
            .field("reauth_required", &self.reauth_required)
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

impl LocalMcpServer {
    /// Convert to a redacted [`McpServerView`]. Never exposes `env` values,
    /// `headers` values, bearer tokens, or OAuth credentials.
    pub fn to_view(&self) -> McpServerView {
        let mut env_keys: Vec<String> = self.env.keys().cloned().collect();
        env_keys.sort();

        let mut header_keys: Vec<String> = self.headers.keys().cloned().collect();
        if self.auth_kind == McpAuthKind::Bearer
            && self
                .bearer_token
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty())
            && !header_keys
                .iter()
                .any(|k| k.eq_ignore_ascii_case("authorization"))
        {
            header_keys.push("Authorization".into());
        }
        header_keys.sort();

        let scopes = match &self.oauth {
            Some(oauth) if !oauth.scopes.is_empty() => oauth.scopes.clone(),
            _ => self.oauth_scopes.clone(),
        };

        let status = if !self.enabled {
            McpServerStatus::Disabled
        } else if self.transport == McpTransport::Stdio {
            McpServerStatus::Connected
        } else {
            match self.auth_kind {
                McpAuthKind::None => McpServerStatus::Connected,
                McpAuthKind::Bearer => {
                    if self
                        .bearer_token
                        .as_deref()
                        .is_some_and(|t| !t.trim().is_empty())
                    {
                        McpServerStatus::Connected
                    } else {
                        McpServerStatus::NeedsAuth
                    }
                }
                McpAuthKind::Headers => {
                    if !self.headers.is_empty() {
                        McpServerStatus::Connected
                    } else {
                        McpServerStatus::NeedsAuth
                    }
                }
                McpAuthKind::OAuth => {
                    if self.reauth_required {
                        McpServerStatus::ReauthRequired
                    } else if self
                        .oauth
                        .as_ref()
                        .is_some_and(|o| !o.access_token.is_empty())
                    {
                        McpServerStatus::Connected
                    } else if self.last_error.is_some() {
                        McpServerStatus::Error
                    } else {
                        McpServerStatus::NeedsAuth
                    }
                }
            }
        };

        McpServerView {
            id: self.id.clone(),
            name: self.name.clone(),
            preset: self.preset.clone(),
            transport: self.transport,
            url: self.url.clone(),
            command: self.command.clone(),
            args: self.args.clone(),
            env_keys,
            auth_kind: self.auth_kind,
            header_keys,
            enabled: self.enabled,
            storage: McpStorage::Local,
            status,
            account_label: self.account_label.clone(),
            agent_name: String::new(),
            last_error: self.last_error.clone(),
            scopes,
            updated_at: self.updated_at,
        }
    }
}

/// Resolved upstream URL and auth headers ready for the loopback gateway.
#[derive(Clone)]
pub struct ResolvedLocalUpstream {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub is_oauth: bool,
}

impl std::fmt::Debug for ResolvedLocalUpstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedLocalUpstream")
            .field("url", &self.url)
            .field("header_keys", &self.headers.keys().collect::<Vec<_>>())
            .field("is_oauth", &self.is_oauth)
            .finish()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalStoreFile {
    version: u32,
    #[serde(default)]
    servers: Vec<LocalMcpServer>,
}

/// Thread-safe local MCP server store backed by a 0600 JSON file.
pub struct LocalStore {
    path: PathBuf,
    servers: Mutex<Vec<LocalMcpServer>>,
    /// Per-server single-flight mutex for local OAuth `refresh_token` grants.
    refresh_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl LocalStore {
    /// Open or initialize the store at `path`.
    pub fn open(path: PathBuf) -> Self {
        let servers = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<LocalStoreFile>(&bytes) {
                Ok(file) => file.servers,
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        error = %err,
                        "mcp_servers: failed to parse local store; starting empty"
                    );
                    Vec::new()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "mcp_servers: failed to read local store; starting empty"
                );
                Vec::new()
            }
        };
        Self {
            path,
            servers: Mutex::new(servers),
            refresh_locks: Mutex::new(HashMap::new()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn contains(&self, id: &str) -> bool {
        lock(&self.servers).iter().any(|s| s.id == id)
    }

    pub fn get(&self, id: &str) -> Option<LocalMcpServer> {
        lock(&self.servers).iter().find(|s| s.id == id).cloned()
    }

    pub fn list_raw(&self) -> Vec<LocalMcpServer> {
        lock(&self.servers).clone()
    }

    pub fn list_views(&self) -> Vec<McpServerView> {
        lock(&self.servers)
            .iter()
            .map(LocalMcpServer::to_view)
            .collect()
    }

    /// Insert or replace a server entry and persist atomically at `0600`.
    pub fn upsert(&self, mut server: LocalMcpServer) -> Result<McpServerView, String> {
        server.updated_at = now_ms();
        let mut servers = lock(&self.servers);
        if let Some(existing) = servers.iter_mut().find(|s| s.id == server.id) {
            *existing = server.clone();
        } else {
            servers.push(server.clone());
        }
        self.persist_locked(&servers)?;
        Ok(server.to_view())
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<Option<McpServerView>, String> {
        let mut servers = lock(&self.servers);
        let Some(server) = servers.iter_mut().find(|s| s.id == id) else {
            return Ok(None);
        };
        server.enabled = enabled;
        server.updated_at = now_ms();
        let view = server.to_view();
        self.persist_locked(&servers)?;
        Ok(Some(view))
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let mut servers = lock(&self.servers);
        let before = servers.len();
        servers.retain(|s| s.id != id);
        if servers.len() == before {
            return Ok(false);
        }
        self.persist_locked(&servers)?;
        Ok(true)
    }

    pub fn set_oauth_tokens(
        &self,
        id: &str,
        oauth: LocalOAuthState,
        account_label: Option<String>,
    ) -> Result<Option<McpServerView>, String> {
        let mut servers = lock(&self.servers);
        let Some(server) = servers.iter_mut().find(|s| s.id == id) else {
            return Ok(None);
        };
        server.oauth_client_id = Some(oauth.client_id.clone());
        if let Some(secret) = &oauth.client_secret {
            server.oauth_client_secret = Some(secret.clone());
        }
        if !oauth.scopes.is_empty() {
            server.oauth_scopes = oauth.scopes.clone();
        }
        server.oauth = Some(oauth);
        if let Some(label) = account_label {
            server.account_label = Some(label);
        }
        server.reauth_required = false;
        server.last_error = None;
        server.updated_at = now_ms();
        let view = server.to_view();
        self.persist_locked(&servers)?;
        Ok(Some(view))
    }

    pub fn disconnect_auth(&self, id: &str) -> Result<Option<McpServerView>, String> {
        let mut servers = lock(&self.servers);
        let Some(server) = servers.iter_mut().find(|s| s.id == id) else {
            return Ok(None);
        };
        server.oauth = None;
        server.bearer_token = None;
        server.headers.clear();
        server.reauth_required = false;
        server.last_error = None;
        server.updated_at = now_ms();
        let view = server.to_view();
        self.persist_locked(&servers)?;
        Ok(Some(view))
    }

    fn mark_oauth_error(&self, id: &str, reauth_required: bool, error: String) {
        let mut servers = lock(&self.servers);
        if let Some(server) = servers.iter_mut().find(|s| s.id == id) {
            server.reauth_required = reauth_required;
            server.last_error = Some(error);
            server.updated_at = now_ms();
            let _ = self.persist_locked(&servers);
        }
    }

    fn persist_locked(&self, servers: &[LocalMcpServer]) -> Result<(), String> {
        let payload = LocalStoreFile {
            version: STORE_VERSION,
            servers: servers.to_vec(),
        };
        let mut bytes = serde_json::to_vec_pretty(&payload)
            .map_err(|e| format!("serialize local MCP store: {e}"))?;
        bytes.push(b'\n');
        write_private_atomic(&self.path, &bytes)
            .map_err(|e| format!("persist local MCP store: {e}"))
    }

    fn refresh_lock_for(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut map = lock(&self.refresh_locks);
        map.entry(id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Resolve the upstream URL and auth headers for a local remote server,
    /// performing a single-flight OAuth `refresh_token` grant if expiring or
    /// `force_refresh` is requested.
    pub async fn resolve_upstream(
        &self,
        id: &str,
        force_refresh: bool,
        http: &reqwest::Client,
    ) -> Result<ResolvedLocalUpstream, String> {
        let server = self
            .get(id)
            .ok_or_else(|| format!("MCP server '{id}' not found"))?;
        if !server.enabled {
            return Err(format!("MCP server '{id}' is disabled"));
        }
        if server.transport == McpTransport::Stdio {
            return Err(format!("MCP server '{id}' uses stdio, not HTTP"));
        }
        let url = server
            .url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .ok_or_else(|| format!("MCP server '{id}' has no URL"))?;

        match server.auth_kind {
            McpAuthKind::None => Ok(ResolvedLocalUpstream {
                url,
                headers: BTreeMap::new(),
                is_oauth: false,
            }),
            McpAuthKind::Bearer => {
                let token = server
                    .bearer_token
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .ok_or_else(|| format!("MCP server '{id}' needs authentication"))?;
                let mut headers = server.headers.clone();
                headers.insert("Authorization".into(), format!("Bearer {token}"));
                Ok(ResolvedLocalUpstream {
                    url,
                    headers,
                    is_oauth: false,
                })
            }
            McpAuthKind::Headers => {
                if server.headers.is_empty() {
                    return Err(format!("MCP server '{id}' has no configured headers"));
                }
                Ok(ResolvedLocalUpstream {
                    url,
                    headers: server.headers.clone(),
                    is_oauth: false,
                })
            }
            McpAuthKind::OAuth => {
                let oauth = server
                    .oauth
                    .clone()
                    .ok_or_else(|| format!("MCP server '{id}' needs OAuth authentication"))?;
                let now = now_ms();
                let expiring = oauth
                    .expires_at
                    .is_some_and(|exp| exp - now <= OAUTH_REFRESH_LEAD_MS);

                if !force_refresh && !expiring {
                    let mut headers = server.headers.clone();
                    headers.insert("Authorization".into(), format_oauth_header(&oauth));
                    return Ok(ResolvedLocalUpstream {
                        url,
                        headers,
                        is_oauth: true,
                    });
                }

                // Single-flight local refresh per server id.
                let previous_token = oauth.access_token.clone();
                let gate = self.refresh_lock_for(id);
                let _guard = gate.lock().await;

                // Re-read after acquiring the gate: a concurrent waiter may
                // have already refreshed the token.
                let current = self
                    .get(id)
                    .ok_or_else(|| format!("MCP server '{id}' not found"))?;
                let current_oauth = current
                    .oauth
                    .clone()
                    .ok_or_else(|| format!("MCP server '{id}' needs OAuth authentication"))?;
                let now = now_ms();
                let current_expiring = current_oauth
                    .expires_at
                    .is_some_and(|exp| exp - now <= OAUTH_REFRESH_LEAD_MS);
                if (!force_refresh && !current_expiring)
                    || (force_refresh && current_oauth.access_token != previous_token)
                {
                    let mut headers = current.headers.clone();
                    headers.insert("Authorization".into(), format_oauth_header(&current_oauth));
                    return Ok(ResolvedLocalUpstream {
                        url,
                        headers,
                        is_oauth: true,
                    });
                }

                let Some(refresh_token) = current_oauth.refresh_token.clone() else {
                    if current_oauth.expires_at.is_some_and(|exp| exp <= now) || force_refresh {
                        self.mark_oauth_error(
                            id,
                            true,
                            "access token expired and no refresh token is stored".into(),
                        );
                        return Err(format!("MCP server '{id}' requires re-authentication"));
                    }
                    let mut headers = current.headers.clone();
                    headers.insert("Authorization".into(), format_oauth_header(&current_oauth));
                    return Ok(ResolvedLocalUpstream {
                        url,
                        headers,
                        is_oauth: true,
                    });
                };

                let refreshed = refresh_local_oauth_token(http, &current_oauth, &refresh_token)
                    .await
                    .map_err(|err| {
                        self.mark_oauth_error(id, err.reauth_required, err.message.clone());
                        err.message
                    })?;

                self.set_oauth_tokens(id, refreshed.clone(), None)?;
                let mut headers = current.headers.clone();
                headers.insert("Authorization".into(), format_oauth_header(&refreshed));
                Ok(ResolvedLocalUpstream {
                    url,
                    headers,
                    is_oauth: true,
                })
            }
        }
    }
}

fn format_oauth_header(oauth: &LocalOAuthState) -> String {
    let token_type = oauth
        .token_type
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("Bearer");
    format!("{token_type} {}", oauth.access_token)
}

struct LocalRefreshError {
    reauth_required: bool,
    message: String,
}

#[derive(Deserialize)]
struct OAuthRefreshResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

async fn refresh_local_oauth_token(
    http: &reqwest::Client,
    current: &LocalOAuthState,
    refresh_token: &str,
) -> Result<LocalOAuthState, LocalRefreshError> {
    let mut form: Vec<(&str, String)> = vec![
        ("grant_type", "refresh_token".into()),
        ("refresh_token", refresh_token.to_owned()),
        ("client_id", current.client_id.clone()),
    ];
    if let Some(secret) = current
        .client_secret
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        form.push(("client_secret", secret.to_owned()));
    }
    if let Some(resource) = current.resource.as_deref().filter(|r| !r.is_empty()) {
        form.push(("resource", resource.to_owned()));
    }

    let response = http
        .post(&current.token_endpoint)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|e| LocalRefreshError {
            reauth_required: false,
            message: format!("OAuth token refresh request failed: {e}"),
        })?;

    let status = response.status();
    let body = response.bytes().await.map_err(|e| LocalRefreshError {
        reauth_required: false,
        message: format!("read OAuth refresh response: {e}"),
    })?;

    if !status.is_success() {
        let parsed = serde_json::from_slice::<OAuthErrorBody>(&body).ok();
        let code = parsed.as_ref().map(|p| p.error.as_str()).unwrap_or("");
        let reauth = matches!(
            code,
            "invalid_grant" | "invalid_client" | "unauthorized_client" | "access_denied"
        ) || status.as_u16() == 400
            || status.as_u16() == 401;
        let detail = parsed
            .map(|p| {
                if !p.error_description.is_empty() {
                    format!("{}: {}", p.error, p.error_description)
                } else if !p.error.is_empty() {
                    p.error
                } else {
                    format!("HTTP {status}")
                }
            })
            .unwrap_or_else(|| format!("HTTP {status}"));
        return Err(LocalRefreshError {
            reauth_required: reauth,
            message: format!("OAuth refresh failed ({detail})"),
        });
    }

    let parsed: OAuthRefreshResponse =
        serde_json::from_slice(&body).map_err(|e| LocalRefreshError {
            reauth_required: false,
            message: format!("parse OAuth refresh response: {e}"),
        })?;
    if parsed.access_token.is_empty() {
        return Err(LocalRefreshError {
            reauth_required: false,
            message: "OAuth refresh returned an empty access_token".into(),
        });
    }

    let expires_at = parsed
        .expires_in
        .filter(|secs| *secs > 0)
        .map(|secs| now_ms() + secs.saturating_mul(1000));
    let scopes = parsed
        .scope
        .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| current.scopes.clone());

    Ok(LocalOAuthState {
        access_token: parsed.access_token,
        refresh_token: parsed
            .refresh_token
            .filter(|t| !t.is_empty())
            .or_else(|| current.refresh_token.clone()),
        token_type: parsed
            .token_type
            .filter(|t| !t.is_empty())
            .or_else(|| current.token_type.clone()),
        expires_at,
        token_endpoint: current.token_endpoint.clone(),
        client_id: current.client_id.clone(),
        client_secret: current.client_secret.clone(),
        scopes,
        resource: current.resource.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_isolation_between_local_and_synced_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let local_profile = EngineProfile::local(dir.path()).unwrap();
        let synced_profile = EngineProfile::synced(dir.path(), "org-1", "user-1");
        let dev_profile = EngineProfile::development(dir.path(), "dev-org", "dev-user");

        assert_eq!(
            store_path_for(&local_profile, dir.path()),
            local_profile.store_root().join("mcp-servers.json")
        );
        assert_eq!(
            store_path_for(&synced_profile, dir.path()),
            dir.path().join("mcp-local-servers.json")
        );
        assert_eq!(
            store_path_for(&dev_profile, dir.path()),
            dir.path().join("mcp-local-servers.json")
        );
        assert_ne!(
            store_path_for(&local_profile, dir.path()),
            store_path_for(&synced_profile, dir.path())
        );
    }

    #[test]
    fn crud_atomic_0600_and_secret_redaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-servers.json");
        let store = LocalStore::open(path.clone());

        let server = LocalMcpServer {
            id: "my-tool".into(),
            name: "My Tool".into(),
            transport: McpTransport::Stdio,
            command: Some("/usr/bin/tool".into()),
            args: vec!["--flag".into()],
            env: [("API_SECRET".into(), "super-secret-env-val".into())].into(),
            auth_kind: McpAuthKind::Headers,
            headers: [("X-Api-Key".into(), "super-secret-header-val".into())].into(),
            bearer_token: Some("super-secret-bearer".into()),
            enabled: true,
            ..Default::default()
        };

        let view = store.upsert(server).unwrap();
        assert_eq!(view.id, "my-tool");
        assert_eq!(view.env_keys, vec!["API_SECRET"]);
        assert_eq!(view.header_keys, vec!["X-Api-Key"]);
        assert_eq!(view.storage, McpStorage::Local);
        assert_eq!(view.status, McpServerStatus::Connected);

        // View JSON must never leak secret values.
        let view_json = serde_json::to_string(&view).unwrap();
        assert!(!view_json.contains("super-secret-env-val"));
        assert!(!view_json.contains("super-secret-header-val"));
        assert!(!view_json.contains("super-secret-bearer"));

        // Permissions on Unix must be 0600.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Disable & reload from disk.
        let disabled = store.set_enabled("my-tool", false).unwrap().unwrap();
        assert_eq!(disabled.status, McpServerStatus::Disabled);

        let reloaded = LocalStore::open(path);
        assert_eq!(reloaded.list_views().len(), 1);
        assert!(!reloaded.list_views()[0].enabled);

        assert!(reloaded.delete("my-tool").unwrap());
        assert!(reloaded.list_views().is_empty());
    }
}
