//! Managed external MCP server coordinator.
//!
//! Coordinates:
//! - Built-in official remote MCP server presets ([`presets`])
//! - Local-only atomic `0600` store ([`local_store`])
//! - Vault Connections client + Ed25519 device enrollment/grant signing ([`vault_client`])
//! - Standards-compliant OAuth 2.1 + PKCE `S256` + RFC 7591 DCR client ([`oauth`])
//! - Loopback MCP Gateway implementing [`zeron_mcp::http::ExternalMcpProxy`] ([`gateway`])
//! - All 10 managed MCP server RPC methods (`WatchMcpServers`, `ListMcpServers`,
//!   `UpsertMcpServer`, `SetMcpServerEnabled`, `DeleteMcpServer`, `StartMcpAuth`,
//!   `PollMcpAuth`, `CancelMcpAuth`, `DisconnectMcpAuth`, `RefreshMcpServers`)
//! - Agent run `extra_mcp` stamping (dual-transport HTTP + `mcp-proxy` fallback
//!   for remote servers, direct stdio for local stdio servers).

pub mod gateway;
pub mod local_store;
pub mod oauth;
pub mod presets;
mod requests;
pub mod vault_client;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures::stream::BoxStream;
use serde_json::Value;
use tokio::sync::watch;

use zeron_proto::{
    CancelMcpAuthRequest, DeleteMcpServerRequest, DisconnectMcpAuthRequest, McpAuthKind, McpHttp,
    McpPreset, McpServer, McpServerStatus, McpServerView, McpServersSnapshot, McpStorage,
    McpTransport, PollMcpAuthRequest, SetMcpServerEnabledRequest, StartMcpAuthRequest,
    UpsertMcpServerRequest, WorkspaceScope,
};
use zeron_rpc::{RpcError, RpcReply, TokenSource, methods, parse_params};

use crate::auth::Auth;
use crate::doc_host::EdgeConfig;
use crate::profile::EngineProfile;

use self::gateway::McpGateway;
use self::local_store::{LocalMcpServer, LocalOAuthState, LocalStore};
use self::oauth::{OAuthCompletedTokens, OAuthFlowManager, StartFlowOptions};
use self::vault_client::{
    LaptopDeviceKey, VaultClient, VaultConnectionMaterial, VaultConnectionView,
    VaultDeviceSigner, VaultPutConnectionRequest, VaultUpdateConnectionRequest, strip_mcp_prefix,
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const RESERVED_SERVER_ID: &str = "zeron";
const MAX_SERVER_ID_LEN: usize = 64;
/// The longest account name (it rides into agents' tool names, slugged).
const MAX_ACCOUNT_LABEL_LEN: usize = 40;
/// The longest name agents see a server by: their tool names are
/// `mcp__{name}__{tool}` and some cap those at 64.
const MAX_AGENT_NAME_LEN: usize = 32;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// RPC methods handled by [`McpServers`].
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::WATCH_MCP_SERVERS
            | methods::LIST_MCP_SERVERS
            | methods::UPSERT_MCP_SERVER
            | methods::SET_MCP_SERVER_ENABLED
            | methods::DELETE_MCP_SERVER
            | methods::START_MCP_AUTH
            | methods::POLL_MCP_AUTH
            | methods::CANCEL_MCP_AUTH
            | methods::DISCONNECT_MCP_AUTH
            | methods::REFRESH_MCP_SERVERS
            | methods::REQUEST_MCP_CHANGE
            | methods::GET_MCP_REQUEST
            | methods::RESOLVE_MCP_REQUEST
    )
}

/// Resolved upstream target and auth headers for [`McpGateway`].
#[derive(Clone)]
pub struct ResolvedUpstream {
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub is_oauth: bool,
}

impl std::fmt::Debug for ResolvedUpstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedUpstream")
            .field("url", &self.url)
            .field("header_keys", &self.headers.keys().collect::<Vec<_>>())
            .field("is_oauth", &self.is_oauth)
            .finish()
    }
}

/// In-memory OAuth client override remembered between `UpsertMcpServer` and
/// `StartMcpAuth` for Vault-backed servers (before tokens exist).
#[derive(Clone, Default)]
struct OAuthClientOverride {
    client_id: Option<String>,
    client_secret: Option<String>,
    scopes: Vec<String>,
}

struct VaultState {
    client: Option<Arc<VaultClient>>,
    disabled: bool,
    connections: Vec<VaultConnectionView>,
}

/// How old the vault's connection list may be when a run starts before it's
/// listed again (see [`McpServers::refresh_for_run`]).
const RUN_REFRESH_AGE: Duration = Duration::from_secs(30);
/// The longest a run waits for that listing.
const RUN_REFRESH_WAIT: Duration = Duration::from_secs(3);

/// Coordinator for managed external MCP servers across Local and Vault storage.
pub struct McpServers {
    scope: WorkspaceScope,
    org_id: String,
    user_id: String,
    device_id: String,
    data_dir: PathBuf,
    edge_url: Option<String>,
    local_store: LocalStore,
    presets: Vec<McpPreset>,
    oauth_flows: OAuthFlowManager,
    vault: Mutex<VaultState>,
    oauth_overrides: Mutex<HashMap<String, OAuthClientOverride>>,
    http: reqwest::Client,
    snapshot_tx: watch::Sender<McpServersSnapshot>,
    /// When the vault's connections were last listed: a run on this device
    /// lists them again once this is older than [`RUN_REFRESH_AGE`], so a
    /// server added, removed or switched on another device reaches it.
    vault_listed_at: Mutex<Option<Instant>>,
    /// One listing at a time for runs starting together.
    run_refresh: tokio::sync::Mutex<()>,
    /// Changes agents asked for, waiting for the person here (`requests`).
    requests: Mutex<Vec<zeron_proto::McpRequestView>>,
    /// Those being answered now (`requests`).
    answering: Mutex<std::collections::HashSet<String>>,
    /// This device's windows following the servers: who a request can ask.
    windows: Arc<std::sync::atomic::AtomicUsize>,
}

impl McpServers {
    /// Construct the MCP servers manager for `profile` + `data_dir`.
    ///
    /// - In [`WorkspaceScope::Local`], stores all servers in
    ///   `{profile.store_root}/mcp-servers.json` and never talks to Vault.
    /// - In [`WorkspaceScope::Synced`] / [`WorkspaceScope::Development`],
    ///   stores stdio and `local_only` servers in
    ///   `{data_dir}/mcp-local-servers.json` and configures a lazy laptop
    ///   [`VaultClient`] when `edge` is provided (Cloud runners switch to their
    ///   [`crate::runner::RunnerIdentity`] when [`Self::attach_auth`] runs).
    pub fn new(
        profile: &EngineProfile,
        data_dir: &Path,
        device_id: impl Into<String>,
        edge: Option<&EdgeConfig>,
    ) -> Arc<Self> {
        let device_id = device_id.into();
        let http = build_http_client();
        let local_store = LocalStore::open(local_store::store_path_for(profile, data_dir));
        let presets = presets::default_presets();

        let vault_client = if profile.scope() != WorkspaceScope::Local
            && !crate::runner::is_cloud_platform()
            && let Some(edge) = edge
        {
            let signer = VaultDeviceSigner::Laptop(Arc::new(LaptopDeviceKey::new(
                data_dir,
                device_id.clone(),
            )));
            Some(Arc::new(VaultClient::new(
                edge.url.clone(),
                profile.org_id(),
                profile.user_id(),
                edge.token.clone(),
                signer,
                http.clone(),
            )))
        } else {
            None
        };

        let (snapshot_tx, _) = watch::channel(McpServersSnapshot::default());
        let manager = Arc::new(Self {
            scope: profile.scope(),
            org_id: profile.org_id().to_owned(),
            user_id: profile.user_id().to_owned(),
            device_id,
            data_dir: data_dir.to_path_buf(),
            edge_url: edge.map(|e| e.url.clone()),
            local_store,
            presets,
            oauth_flows: OAuthFlowManager::new(),
            vault: Mutex::new(VaultState {
                client: vault_client,
                disabled: false,
                connections: Vec::new(),
            }),
            oauth_overrides: Mutex::new(HashMap::new()),
            http,
            snapshot_tx,
            vault_listed_at: Mutex::new(None),
            run_refresh: tokio::sync::Mutex::new(()),
            requests: Mutex::new(Vec::new()),
            answering: Mutex::default(),
            windows: Arc::default(),
        });

        manager.publish_snapshot();
        manager.spawn_background_refresh();
        manager
    }

    /// Construct a manager with an explicit [`VaultClient`] (for tests or
    /// custom wiring).
    pub fn with_vault_client(
        profile: &EngineProfile,
        data_dir: &Path,
        device_id: impl Into<String>,
        vault_client: Option<Arc<VaultClient>>,
    ) -> Arc<Self> {
        let http = build_http_client();
        let local_store = LocalStore::open(local_store::store_path_for(profile, data_dir));
        let presets = presets::default_presets();
        let (snapshot_tx, _) = watch::channel(McpServersSnapshot::default());
        let manager = Arc::new(Self {
            scope: profile.scope(),
            org_id: profile.org_id().to_owned(),
            user_id: profile.user_id().to_owned(),
            device_id: device_id.into(),
            data_dir: data_dir.to_path_buf(),
            edge_url: None,
            local_store,
            presets,
            oauth_flows: OAuthFlowManager::new(),
            vault: Mutex::new(VaultState {
                client: vault_client,
                disabled: false,
                connections: Vec::new(),
            }),
            oauth_overrides: Mutex::new(HashMap::new()),
            http,
            snapshot_tx,
            vault_listed_at: Mutex::new(None),
            run_refresh: tokio::sync::Mutex::new(()),
            requests: Mutex::new(Vec::new()),
            answering: Mutex::default(),
            windows: Arc::default(),
        });
        manager.publish_snapshot();
        manager
    }

    /// Wire [`Auth`] once attached to [`crate::EngineCore`]: Cloud runners use
    /// their enrolled [`crate::runner::RunnerIdentity`] for Vault grant signing;
    /// signed-in desktop/headless runtimes ensure a laptop [`VaultClient`] is
    /// configured and trigger a best-effort Vault refresh.
    pub fn attach_auth(self: &Arc<Self>, auth: Auth) {
        if self.scope == WorkspaceScope::Local {
            return;
        }
        let edge_url = self
            .edge_url
            .clone()
            .unwrap_or_else(|| auth.edge_url().to_owned());
        if edge_url.trim().is_empty() {
            return;
        }

        let token_source: Arc<dyn TokenSource> = Arc::new(auth.clone());
        let mut should_refresh = false;
        {
            let mut state = lock(&self.vault);
            if let Some(identity) = auth.runner_identity() {
                let client = VaultClient::new(
                    edge_url,
                    identity.org_id().to_owned(),
                    identity.user_id().to_owned(),
                    token_source,
                    VaultDeviceSigner::Runner(identity),
                    self.http.clone(),
                );
                state.client = Some(Arc::new(client));
                state.disabled = false;
                should_refresh = true;
            } else if state.client.is_none() && !crate::runner::is_cloud_platform() {
                let signer = VaultDeviceSigner::Laptop(Arc::new(LaptopDeviceKey::new(
                    &self.data_dir,
                    self.device_id.clone(),
                )));
                let client = VaultClient::new(
                    edge_url,
                    self.org_id.clone(),
                    self.user_id.clone(),
                    token_source,
                    signer,
                    self.http.clone(),
                );
                state.client = Some(Arc::new(client));
                state.disabled = false;
                should_refresh = true;
            } else if state.client.is_some() {
                should_refresh = true;
            }
        }
        self.publish_snapshot();
        if should_refresh {
            self.spawn_background_refresh();
        }
    }

    /// Build the [`zeron_mcp::http::ExternalMcpProxy`] gateway backed by this
    /// manager.
    pub fn gateway(self: &Arc<Self>) -> Arc<dyn zeron_mcp::http::ExternalMcpProxy> {
        Arc::new(McpGateway::new(self.clone()))
    }

    /// Subscribe to live [`McpServersSnapshot`] updates.
    pub fn watch_snapshot(&self) -> watch::Receiver<McpServersSnapshot> {
        self.snapshot_tx.subscribe()
    }

    /// Current [`McpServersSnapshot`].
    pub fn snapshot(&self) -> McpServersSnapshot {
        self.build_snapshot()
    }

    /// Cancel any pending interactive OAuth flows on engine shutdown.
    pub fn shutdown(&self) {
        self.oauth_flows.shutdown();
    }

    fn vault_client(&self) -> Option<Arc<VaultClient>> {
        let state = lock(&self.vault);
        if state.disabled {
            None
        } else {
            state.client.clone()
        }
    }

    fn vault_client_raw(&self) -> Option<Arc<VaultClient>> {
        lock(&self.vault).client.clone()
    }

    fn spawn_background_refresh(self: &Arc<Self>) {
        if self.vault_client_raw().is_none() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let me = self.clone();
        handle.spawn(async move {
            let _ = me.refresh().await;
        });
    }

    /// Before a run: the vault's connections listed again if the last
    /// listing is older than [`RUN_REFRESH_AGE`] (another device may have
    /// added, removed or switched a server since), waiting at most
    /// [`RUN_REFRESH_WAIT`]; past that, or offline, the run takes what's
    /// known. Runs starting together share one listing.
    pub async fn refresh_for_run(&self) {
        if self.vault_client_raw().is_none() {
            return;
        }
        let fresh = |listed: Option<Instant>| listed.is_some_and(|at| at.elapsed() < RUN_REFRESH_AGE);
        if fresh(*lock(&self.vault_listed_at)) {
            return;
        }
        let _one = self.run_refresh.lock().await;
        if fresh(*lock(&self.vault_listed_at)) {
            return;
        }
        if tokio::time::timeout(RUN_REFRESH_WAIT, self.refresh()).await.is_err() {
            tracing::warn!("mcp: the vault's servers didn't list in time; the run takes those known");
        }
    }

    /// Refresh Vault-backed MCP connections (best-effort when offline) and
    /// republish the merged snapshot.
    pub async fn refresh(&self) -> McpServersSnapshot {
        if let Some(client) = self.vault_client_raw() {
            match client.list_connections().await {
                Ok(view) => {
                    let mut state = lock(&self.vault);
                    state.disabled = view.disabled;
                    state.connections = view.connections;
                    *lock(&self.vault_listed_at) = Some(Instant::now());
                }
                Err(err) => {
                    tracing::debug!(
                        error = %err,
                        "mcp_servers: vault refresh failed; keeping cached/local view"
                    );
                }
            }
        }
        self.publish_snapshot()
    }

    fn build_snapshot(&self) -> McpServersSnapshot {
        let (vault_available, vault_views) = {
            let state = lock(&self.vault);
            let available = state.client.is_some() && !state.disabled;
            let views: Vec<McpServerView> = if available {
                state
                    .connections
                    .iter()
                    .map(VaultConnectionView::to_mcp_server_view)
                    .collect()
            } else {
                Vec::new()
            };
            (available, views)
        };

        let mut seen = HashSet::new();
        let mut servers = Vec::new();
        for mut view in vault_views {
            if self.oauth_flows.is_authorizing(&view.id) {
                view.status = McpServerStatus::Authorizing;
            }
            if seen.insert(view.id.clone()) {
                servers.push(view);
            }
        }
        for mut view in self.local_store.list_views() {
            if self.oauth_flows.is_authorizing(&view.id) {
                view.status = McpServerStatus::Authorizing;
            }
            if seen.insert(view.id.clone()) {
                servers.push(view);
            }
        }

        // A service's accounts together, by their names.
        servers.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then_with(|| {
                    let label = |v: &McpServerView| v.account_label.clone().unwrap_or_default().to_lowercase();
                    label(a).cmp(&label(b))
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        assign_agent_names(&mut servers);

        McpServersSnapshot {
            storage: if vault_available {
                McpStorage::Vault
            } else {
                McpStorage::Local
            },
            vault_available,
            presets: self.presets.clone(),
            servers,
            client_redirect_uri: oauth::preregistered_redirect_uri(),
            requests: lock(&self.requests).clone(),
        }
    }

    fn publish_snapshot(&self) -> McpServersSnapshot {
        let snapshot = self.build_snapshot();
        self.snapshot_tx.send_replace(snapshot.clone());
        snapshot
    }

    fn upsert_vault_cache(&self, view: VaultConnectionView) {
        let mut state = lock(&self.vault);
        state.disabled = false;
        let short_id = strip_mcp_prefix(&view.id).to_owned();
        if let Some(existing) = state
            .connections
            .iter_mut()
            .find(|c| strip_mcp_prefix(&c.id) == short_id)
        {
            *existing = view;
        } else {
            state.connections.push(view);
        }
    }

    fn remove_vault_cache(&self, id: &str) {
        let mut state = lock(&self.vault);
        state
            .connections
            .retain(|c| strip_mcp_prefix(&c.id) != id);
    }

    fn get_vault_cached(&self, id: &str) -> Option<VaultConnectionView> {
        lock(&self.vault)
            .connections
            .iter()
            .find(|c| strip_mcp_prefix(&c.id) == id)
            .cloned()
    }

    fn mark_vault_reauth_required(&self, id: &str, message: String) {
        let mut updated = false;
        {
            let mut state = lock(&self.vault);
            if let Some(conn) = state
                .connections
                .iter_mut()
                .find(|c| strip_mcp_prefix(&c.id) == id)
            {
                conn.status = "reauth_required".into();
                conn.last_error = Some(message);
                updated = true;
            }
        }
        if updated {
            self.publish_snapshot();
        }
    }

    /// Resolve upstream URL and auth headers for [`McpGateway`].
    pub async fn resolve_upstream(
        &self,
        server_id: &str,
        force_refresh: bool,
    ) -> Result<ResolvedUpstream, String> {
        if self.local_store.contains(server_id) {
            let result = self
                .local_store
                .resolve_upstream(server_id, force_refresh, &self.http)
                .await;
            if result.is_err() {
                self.publish_snapshot();
            }
            let local = result?;
            return Ok(ResolvedUpstream {
                url: local.url,
                headers: local.headers,
                is_oauth: local.is_oauth,
            });
        }

        let Some(vault) = self.vault_client() else {
            return Err(format!("MCP server '{server_id}' not found"));
        };

        let cached = self.get_vault_cached(server_id);
        if let Some(ref conn) = cached
            && !conn.enabled
        {
            return Err(format!("MCP server '{server_id}' is disabled"));
        }
        let is_oauth = cached
            .as_ref()
            .is_some_and(|c| c.auth_kind == "oauth");

        match vault.grant_connection(server_id, force_refresh).await {
            Ok(grant) => Ok(ResolvedUpstream {
                url: grant.url,
                headers: grant.headers,
                is_oauth,
            }),
            Err(err) => {
                if err.is_needs_reconnect() {
                    self.mark_vault_reauth_required(server_id, err.message.clone());
                }
                Err(err.message)
            }
        }
    }

    /// Construct [`McpServer`] entries to stamp into `RunRequest.extra_mcp`
    /// for an agent run.
    ///
    /// - Remote (`Http` / `Sse`) servers that are enabled and ready (`Connected`
    ///   or `auth_kind == None`) get dual-transport configs pointing at the
    ///   loopback MCP Gateway (`endpoint.server_url(&id)`) with
    ///   `Authorization: Bearer {grant_token}` AND fallback `mcp-proxy` stdio
    ///   command (`ZERON_MCP_URL`, `ZERON_MCP_TOKEN`).
    /// - Local (`Stdio`) servers that are enabled get direct stdio configs.
    /// - Never emits a server named `"zeron"`.
    pub fn extra_mcp_servers(
        &self,
        endpoint: Option<&zeron_mcp::http::McpHttpServer>,
        grant_token: Option<&str>,
    ) -> Vec<McpServer> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        seen.insert(RESERVED_SERVER_ID.to_owned());

        let proxy_cmd = crate::managed::mcp_executable()
            .and_then(|p| p.to_str().map(str::to_owned))
            .unwrap_or_default();
        // Each by the name agents know it by (`linear_work`), reached by its id.
        let agent_names: HashMap<String, String> = self
            .build_snapshot()
            .servers
            .into_iter()
            .map(|v| (v.id, v.agent_name))
            .collect();
        let agent_name = |id: &str| -> String {
            agent_names
                .get(id)
                .filter(|n| !n.is_empty())
                .cloned()
                .unwrap_or_else(|| id.to_owned())
        };

        // 1. Vault-backed remote servers (always HTTP/SSE).
        let vault_views: Vec<McpServerView> = {
            let state = lock(&self.vault);
            if state.client.is_some() && !state.disabled {
                state
                    .connections
                    .iter()
                    .map(VaultConnectionView::to_mcp_server_view)
                    .collect()
            } else {
                Vec::new()
            }
        };

        for view in vault_views {
            if !view.enabled || !is_remote_ready(&view) {
                continue;
            }
            if !seen.insert(view.id.clone()) {
                continue;
            }
            if let (Some(ep), Some(token)) = (endpoint, grant_token) {
                out.push(build_gateway_mcp_server(&agent_name(&view.id), &view.id, ep, token, &proxy_cmd));
            }
        }

        // 2. Local servers (both Stdio and local-only Http/Sse).
        for server in self.local_store.list_raw() {
            if !server.enabled {
                continue;
            }
            let view = server.to_view();
            match server.transport {
                McpTransport::Stdio => {
                    let Some(command) = server.command.filter(|c| !c.trim().is_empty()) else {
                        continue;
                    };
                    if !seen.insert(server.id.clone()) {
                        continue;
                    }
                    out.push(McpServer {
                        name: agent_name(&server.id),
                        command,
                        args: server.args,
                        env: server.env,
                        http: None,
                    });
                }
                McpTransport::Http | McpTransport::Sse => {
                    if !is_remote_ready(&view) {
                        continue;
                    }
                    if !seen.insert(server.id.clone()) {
                        continue;
                    }
                    if let (Some(ep), Some(token)) = (endpoint, grant_token) {
                        out.push(build_gateway_mcp_server(&agent_name(&server.id), &server.id, ep, token, &proxy_cmd));
                    }
                }
            }
        }

        // What each run gets, and what it can't (no endpoint or grant: a
        // remote server can't reach its gateway), so a missing tool is
        // explained in the log.
        let names: Vec<&str> = out.iter().map(|s| s.name.as_str()).collect();
        tracing::info!(
            servers = ?names,
            gateway = endpoint.is_some(),
            grant = grant_token.is_some(),
            "mcp: managed servers for a run"
        );
        out
    }

    /// Dispatch any of the 10 managed MCP server RPC methods.
    pub async fn handle_rpc(
        self: &Arc<Self>,
        method: &str,
        params: Value,
    ) -> Result<RpcReply, RpcError> {
        match method {
            methods::WATCH_MCP_SERVERS => Ok(RpcReply::Stream(self.watch_stream(zeron_rpc::caller()))),
            methods::LIST_MCP_SERVERS => {
                let caller = zeron_rpc::caller();
                to_unary(&shown_to(&caller, self.snapshot()))
            }
            methods::REFRESH_MCP_SERVERS => {
                let caller = zeron_rpc::caller();
                let snap = self.refresh().await;
                to_unary(&shown_to(&caller, snap))
            }
            methods::UPSERT_MCP_SERVER => {
                let req: UpsertMcpServerRequest = parse_params(params)?;
                let snap = self
                    .upsert_server(req)
                    .await
                    .map_err(RpcError::Failed)?;
                to_unary(&snap)
            }
            methods::SET_MCP_SERVER_ENABLED => {
                let req: SetMcpServerEnabledRequest = parse_params(params)?;
                let snap = self
                    .set_server_enabled(&req.id, req.enabled)
                    .await
                    .map_err(RpcError::Failed)?;
                to_unary(&snap)
            }
            methods::DELETE_MCP_SERVER => {
                let req: DeleteMcpServerRequest = parse_params(params)?;
                let snap = self
                    .delete_server(&req.id)
                    .await
                    .map_err(RpcError::Failed)?;
                to_unary(&snap)
            }
            methods::REQUEST_MCP_CHANGE => {
                let req: zeron_proto::RequestMcpChangeParams = parse_params(params)?;
                let view = self.request_change(req).await.map_err(RpcError::Failed)?;
                to_unary(&view)
            }
            methods::GET_MCP_REQUEST => {
                let req: zeron_proto::McpRequestParams = parse_params(params)?;
                let view = self.get_request(&req.id).map_err(RpcError::Failed)?;
                to_unary(&view)
            }
            methods::RESOLVE_MCP_REQUEST => {
                let req: zeron_proto::McpRequestParams = parse_params(params)?;
                let view = self
                    .resolve_request(&req.id, req.approve)
                    .await
                    .map_err(RpcError::Failed)?;
                to_unary(&view)
            }
            methods::START_MCP_AUTH => {
                let req: StartMcpAuthRequest = parse_params(params)?;
                let resp = self
                    .start_auth(req)
                    .await
                    .map_err(RpcError::Failed)?;
                to_unary(&resp)
            }
            methods::POLL_MCP_AUTH => {
                let req: PollMcpAuthRequest = parse_params(params)?;
                let resp = self
                    .oauth_flows
                    .poll(&req.flow_id)
                    .map_err(RpcError::Failed)?;
                to_unary(&resp)
            }
            methods::CANCEL_MCP_AUTH => {
                let req: CancelMcpAuthRequest = parse_params(params)?;
                let cancelled = self.oauth_flows.cancel(&req.flow_id);
                if cancelled {
                    self.publish_snapshot();
                }
                Ok(RpcReply::Value(serde_json::json!({ "cancelled": cancelled })))
            }
            methods::DISCONNECT_MCP_AUTH => {
                let req: DisconnectMcpAuthRequest = parse_params(params)?;
                let snap = self
                    .disconnect_auth(&req.id)
                    .await
                    .map_err(RpcError::Failed)?;
                to_unary(&snap)
            }
            _ => Err(RpcError::UnknownMethod(method.to_owned())),
        }
    }

    /// The snapshot, followed by `caller`; this device's own window counted
    /// while it does (it's whom a request asks).
    fn watch_stream(&self, caller: zeron_rpc::Caller) -> BoxStream<'static, Value> {
        let rx = self.snapshot_tx.subscribe();
        let window = caller.is_local().then(|| Window::open(&self.windows));
        Box::pin(futures::stream::unfold(
            (rx, true, caller, window),
            |(mut rx, first, caller, window)| async move {
                if !first {
                    rx.changed().await.ok()?;
                }
                let snapshot = rx.borrow_and_update().clone();
                let val = serde_json::to_value(shown_to(&caller, snapshot)).ok()?;
                Some((val, (rx, false, caller, window)))
            },
        ))
    }

    /// Create or update an MCP server in Vault or Local storage.
    pub async fn upsert_server(
        &self,
        req: UpsertMcpServerRequest,
    ) -> Result<McpServersSnapshot, String> {
        let name = req.name.trim().to_owned();
        if name.is_empty() {
            return Err("server name must not be empty".into());
        }

        let id = self.resolve_server_id(&req, &name)?;
        let label = requested_account_label(&req)?;

        match req.transport {
            McpTransport::Stdio => {
                let cmd = req
                    .command
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| "stdio MCP server requires a non-empty command".to_owned())?
                    .to_owned();

                let existing = self.local_store.get(&id);
                let enabled = req
                    .enabled
                    .unwrap_or_else(|| existing.as_ref().map_or(true, |e| e.enabled));
                let env = if !req.env.is_empty() {
                    req.env
                } else {
                    existing.map(|e| e.env).unwrap_or_default()
                };

                let record = LocalMcpServer {
                    id: id.clone(),
                    name,
                    preset: req.preset.filter(|p| !p.trim().is_empty()),
                    transport: McpTransport::Stdio,
                    url: None,
                    command: Some(cmd),
                    args: req.args,
                    env,
                    auth_kind: McpAuthKind::None,
                    bearer_token: None,
                    headers: BTreeMap::new(),
                    oauth: None,
                    oauth_client_id: None,
                    oauth_client_secret: None,
                    oauth_scopes: Vec::new(),
                    enabled,
                    account_label: match label {
                        Some(set) => set,
                        None => self.local_store.get(&id).and_then(|e| e.account_label),
                    },
                    last_error: None,
                    reauth_required: false,
                    updated_at: 0,
                };
                self.local_store.upsert(record)?;
                return Ok(self.publish_snapshot());
            }
            McpTransport::Http | McpTransport::Sse => {
                let raw_url = req
                    .url
                    .as_deref()
                    .map(str::trim)
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| "remote MCP server requires a URL".to_owned())?
                    .to_owned();
                let parsed_url = reqwest::Url::parse(&raw_url)
                    .map_err(|e| format!("invalid MCP server URL '{raw_url}': {e}"))?;
                let scheme = parsed_url.scheme();
                if scheme != "https" && scheme != "http" {
                    return Err("MCP server URL must use http:// or https://".into());
                }

                // Decide storage: Vault when available, not marked local_only,
                // and URL is HTTPS (Vault rejects non-HTTPS URLs).
                let vault = if !req.local_only && scheme == "https" {
                    self.vault_client()
                } else {
                    None
                };

                if let Some(vault) = vault {
                    self.upsert_vault_server(&vault, &id, name, &raw_url, req)
                        .await?;
                    // If the server previously lived in local_store, remove the local copy.
                    let _ = self.local_store.delete(&id);
                    return Ok(self.publish_snapshot());
                }

                self.upsert_local_remote_server(&id, name, &raw_url, req)?;
                // If it previously lived in Vault and caller switched it to local_only,
                // remove from Vault best-effort.
                if let Some(vault) = self.vault_client()
                    && self.get_vault_cached(&id).is_some()
                {
                    let _ = vault.delete_connection(&id).await;
                    self.remove_vault_cache(&id);
                }
                Ok(self.publish_snapshot())
            }
        }
    }

    async fn upsert_vault_server(
        &self,
        vault: &VaultClient,
        id: &str,
        name: String,
        url: &str,
        req: UpsertMcpServerRequest,
    ) -> Result<(), String> {
        let label = requested_account_label(&req)?;
        let transport_str = match req.transport {
            McpTransport::Sse => "sse",
            _ => "http",
        };
        let target_vault_auth_kind = match req.auth_kind {
            McpAuthKind::None => "none",
            McpAuthKind::OAuth => "oauth",
            McpAuthKind::Bearer | McpAuthKind::Headers => "headers",
        };

        // Remember custom OAuth client overrides for a subsequent StartMcpAuth.
        if req.auth_kind == McpAuthKind::OAuth {
            let has_override = req.oauth_client_id.as_deref().is_some_and(|s| !s.trim().is_empty())
                || req.oauth_client_secret.as_deref().is_some_and(|s| !s.is_empty())
                || !req.oauth_scopes.is_empty();
            if has_override {
                lock(&self.oauth_overrides).insert(
                    id.to_owned(),
                    OAuthClientOverride {
                        client_id: req.oauth_client_id.clone().filter(|s| !s.trim().is_empty()),
                        client_secret: req.oauth_client_secret.clone().filter(|s| !s.is_empty()),
                        scopes: req.oauth_scopes.clone(),
                    },
                );
            }
        } else {
            lock(&self.oauth_overrides).remove(id);
        }

        let new_material = build_vault_static_material(&req)?;

        // Vault PUT clears stored envelope when `material` is omitted. If we
        // are updating an existing Vault server with the same authKind and no
        // new credentials were provided, use PATCH to preserve existing tokens!
        if let Some(existing) = self.get_vault_cached(id)
            && existing.auth_kind == target_vault_auth_kind
            && new_material.is_none()
        {
            let patch = VaultUpdateConnectionRequest {
                name: Some(name),
                url: Some(url.to_owned()),
                transport: Some(transport_str.to_owned()),
                enabled: req.enabled,
                authorized_devices: None,
                account_label: label,
            };
            let updated = vault
                .update_connection(id, &patch)
                .await
                .map_err(|e| e.message)?;
            self.upsert_vault_cache(updated);
            return Ok(());
        }

        let existing = self.get_vault_cached(id);
        let enabled = req
            .enabled
            .or_else(|| existing.as_ref().map(|e| e.enabled))
            .unwrap_or(true);
        let authorized_devices = existing
            .as_ref()
            .map(|e| e.authorized_devices.clone())
            .filter(|d| !d.is_empty());
        let scopes = if !req.oauth_scopes.is_empty() {
            Some(req.oauth_scopes.clone())
        } else {
            existing.as_ref().map(|e| e.scopes.clone()).filter(|s| !s.is_empty())
        };

        let put_req = VaultPutConnectionRequest {
            name,
            preset: req
                .preset
                .filter(|p| !p.trim().is_empty())
                .or_else(|| existing.as_ref().and_then(|e| e.preset.clone())),
            url: url.to_owned(),
            transport: Some(transport_str.to_owned()),
            enabled: Some(enabled),
            auth_kind: target_vault_auth_kind.to_owned(),
            material: new_material,
            authorized_devices,
            scopes,
            account_label: match label {
                Some(set) => set,
                None => existing.and_then(|e| e.account_label),
            },
        };
        let created = vault
            .put_connection(id, &put_req)
            .await
            .map_err(|e| e.message)?;
        self.upsert_vault_cache(created);
        Ok(())
    }

    fn upsert_local_remote_server(
        &self,
        id: &str,
        name: String,
        url: &str,
        req: UpsertMcpServerRequest,
    ) -> Result<(), String> {
        let label = requested_account_label(&req)?;
        let existing = self.local_store.get(id);
        // (What it was given is for where it was: moved to another origin,
        // it's asked for again.)
        let same_place = existing.as_ref().is_some_and(|e| {
            let origin = e.url.as_deref().and_then(gateway::url_origin);
            origin.is_some() && origin == gateway::url_origin(url)
        });
        let same_auth = same_place
            && existing
                .as_ref()
                .is_some_and(|e| e.auth_kind == req.auth_kind);
        let enabled = req
            .enabled
            .unwrap_or_else(|| existing.as_ref().map_or(true, |e| e.enabled));

        let bearer_token = match req.auth_kind {
            McpAuthKind::Bearer => req
                .bearer_token
                .filter(|t| !t.trim().is_empty())
                .or_else(|| {
                    if same_auth {
                        existing.as_ref().and_then(|e| e.bearer_token.clone())
                    } else {
                        None
                    }
                }),
            _ => None,
        };

        let headers = match req.auth_kind {
            McpAuthKind::Headers | McpAuthKind::Bearer => {
                if !req.headers.is_empty() {
                    req.headers
                } else if same_auth {
                    existing
                        .as_ref()
                        .map(|e| e.headers.clone())
                        .unwrap_or_default()
                } else {
                    BTreeMap::new()
                }
            }
            _ => BTreeMap::new(),
        };

        let oauth = if req.auth_kind == McpAuthKind::OAuth && same_auth {
            existing.as_ref().and_then(|e| e.oauth.clone())
        } else {
            None
        };
        let oauth_client_id = if req.auth_kind == McpAuthKind::OAuth {
            req.oauth_client_id
                .filter(|s| !s.trim().is_empty())
                .or_else(|| existing.as_ref().filter(|_| same_place).and_then(|e| e.oauth_client_id.clone()))
        } else {
            None
        };
        let oauth_client_secret = if req.auth_kind == McpAuthKind::OAuth {
            req.oauth_client_secret
                .filter(|s| !s.is_empty())
                .or_else(|| existing.as_ref().filter(|_| same_place).and_then(|e| e.oauth_client_secret.clone()))
        } else {
            None
        };
        let oauth_scopes = if req.auth_kind == McpAuthKind::OAuth {
            if !req.oauth_scopes.is_empty() {
                req.oauth_scopes
            } else {
                existing
                    .as_ref()
                    .map(|e| e.oauth_scopes.clone())
                    .unwrap_or_default()
            }
        } else {
            Vec::new()
        };

        let record = LocalMcpServer {
            id: id.to_owned(),
            name,
            preset: req
                .preset
                .filter(|p| !p.trim().is_empty())
                .or_else(|| existing.as_ref().and_then(|e| e.preset.clone())),
            transport: req.transport,
            url: Some(url.to_owned()),
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            auth_kind: req.auth_kind,
            bearer_token,
            headers,
            oauth,
            oauth_client_id,
            oauth_client_secret,
            oauth_scopes,
            enabled,
            // (The person's name for the account: kept whatever its sign-in.)
            account_label: match label {
                Some(set) => set,
                None => existing.as_ref().and_then(|e| e.account_label.clone()),
            },
            last_error: if same_auth {
                existing.as_ref().and_then(|e| e.last_error.clone())
            } else {
                None
            },
            reauth_required: if same_auth {
                existing.as_ref().is_some_and(|e| e.reauth_required)
            } else {
                false
            },
            updated_at: 0,
        };
        self.local_store.upsert(record)?;
        Ok(())
    }

    fn resolve_server_id(&self, req: &UpsertMcpServerRequest, name: &str) -> Result<String, String> {
        if let Some(raw_id) = req.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            let id = strip_mcp_prefix(raw_id);
            validate_server_id(id)?;
            return Ok(id.to_owned());
        }

        let seed = req
            .preset
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(name);
        let base = slugify_id(seed);
        let base = if base.is_empty() || base == RESERVED_SERVER_ID {
            "mcp-server".to_owned()
        } else {
            base
        };

        let existing_ids: HashSet<String> = self
            .snapshot()
            .servers
            .into_iter()
            .map(|s| s.id)
            .collect();
        if !existing_ids.contains(&base) {
            return Ok(base);
        }
        for n in 2..=999 {
            let suffix = format!("-{n}");
            let prefix_len = MAX_SERVER_ID_LEN.saturating_sub(suffix.len());
            let candidate = format!("{}{}", &base[..base.len().min(prefix_len)], suffix);
            if !existing_ids.contains(&candidate) {
                return Ok(candidate);
            }
        }
        Err("could not allocate a unique MCP server id".into())
    }

    /// Enable or disable an MCP server by id.
    pub async fn set_server_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<McpServersSnapshot, String> {
        let id = strip_mcp_prefix(id.trim());
        if self.local_store.contains(id) {
            self.local_store.set_enabled(id, enabled)?;
            return Ok(self.publish_snapshot());
        }
        if let Some(vault) = self.vault_client() {
            let patch = VaultUpdateConnectionRequest {
                enabled: Some(enabled),
                ..Default::default()
            };
            let updated = vault
                .update_connection(id, &patch)
                .await
                .map_err(|e| e.message)?;
            self.upsert_vault_cache(updated);
            return Ok(self.publish_snapshot());
        }
        Err(format!("MCP server '{id}' not found"))
    }

    /// Delete an MCP server by id.
    pub async fn delete_server(&self, id: &str) -> Result<McpServersSnapshot, String> {
        let id = strip_mcp_prefix(id.trim());
        self.oauth_flows.cancel_for_server(id);
        lock(&self.oauth_overrides).remove(id);

        let mut removed = self.local_store.delete(id)?;
        if let Some(vault) = self.vault_client()
            && (!removed || self.get_vault_cached(id).is_some())
        {
            match vault.delete_connection(id).await {
                Ok(deleted) => {
                    removed |= deleted;
                    self.remove_vault_cache(id);
                }
                Err(err) => {
                    if !removed {
                        return Err(err.message);
                    }
                }
            }
        }
        if !removed {
            return Err(format!("MCP server '{id}' not found"));
        }
        Ok(self.publish_snapshot())
    }

    /// Disconnect stored credentials for an MCP server while keeping its config.
    pub async fn disconnect_auth(&self, id: &str) -> Result<McpServersSnapshot, String> {
        let id = strip_mcp_prefix(id.trim());
        self.oauth_flows.cancel_for_server(id);

        if self.local_store.contains(id) {
            self.local_store.disconnect_auth(id)?;
            return Ok(self.publish_snapshot());
        }
        if let Some(vault) = self.vault_client() {
            let updated = vault.disconnect_auth(id).await.map_err(|e| e.message)?;
            self.upsert_vault_cache(updated);
            return Ok(self.publish_snapshot());
        }
        Err(format!("MCP server '{id}' not found"))
    }

    /// Start an interactive OAuth 2.1 + PKCE + DCR flow for server `req.id`.
    pub async fn start_auth(
        self: &Arc<Self>,
        req: StartMcpAuthRequest,
    ) -> Result<zeron_proto::StartMcpAuthResponse, String> {
        let id = strip_mcp_prefix(req.id.trim()).to_owned();
        let (is_vault, server_name, server_url, stored_client_id, stored_client_secret, stored_scopes) =
            if let Some(local) = self.local_store.get(&id) {
                if local.transport == McpTransport::Stdio {
                    return Err(format!("MCP server '{id}' uses stdio and does not support OAuth"));
                }
                let url = local
                    .url
                    .filter(|u| !u.trim().is_empty())
                    .ok_or_else(|| format!("MCP server '{id}' has no URL"))?;
                (
                    false,
                    local.name,
                    url,
                    local.oauth_client_id,
                    local.oauth_client_secret,
                    local.oauth_scopes,
                )
            } else if let Some(vault_conn) = self.get_vault_cached(&id) {
                let overrides = lock(&self.oauth_overrides).get(&id).cloned().unwrap_or_default();
                let scopes = if !overrides.scopes.is_empty() {
                    overrides.scopes
                } else {
                    vault_conn.scopes
                };
                (
                    true,
                    vault_conn.name,
                    vault_conn.url,
                    overrides.client_id,
                    overrides.client_secret,
                    scopes,
                )
            } else {
                return Err(format!("MCP server '{id}' not found"));
            };

        let client_id = req
            .client_id
            .filter(|s| !s.trim().is_empty())
            .or(stored_client_id);
        let client_secret = req
            .client_secret
            .filter(|s| !s.is_empty())
            .or(stored_client_secret);
        let scopes = if !req.scopes.is_empty() {
            req.scopes
        } else {
            stored_scopes
        };

        let me_complete = self.clone();
        let complete_id = id.clone();
        let on_complete: oauth::OnOAuthComplete = Arc::new(move |tokens: OAuthCompletedTokens| {
            let me = me_complete.clone();
            let server_id = complete_id.clone();
            Box::pin(async move { me.persist_oauth_completion(&server_id, is_vault, tokens).await })
        });

        let me_change = self.clone();
        let on_change: oauth::OnFlowChange = Arc::new(move || {
            me_change.publish_snapshot();
        });

        let resp = self
            .oauth_flows
            .start_flow(
                self.http.clone(),
                StartFlowOptions {
                    server_id: id,
                    server_name,
                    server_url,
                    client_id,
                    client_secret,
                    scopes,
                },
                on_complete,
                on_change,
            )
            .await?;

        self.publish_snapshot();
        Ok(resp)
    }

    async fn persist_oauth_completion(
        &self,
        server_id: &str,
        is_vault: bool,
        tokens: OAuthCompletedTokens,
    ) -> Result<(), String> {
        if is_vault {
            let vault = self
                .vault_client()
                .ok_or_else(|| "Vault is unavailable to persist OAuth credentials".to_owned())?;
            let existing = self
                .get_vault_cached(server_id)
                .ok_or_else(|| format!("MCP server '{server_id}' no longer exists"))?;
            let scopes = if !tokens.scopes.is_empty() {
                Some(tokens.scopes.clone())
            } else if !existing.scopes.is_empty() {
                Some(existing.scopes.clone())
            } else {
                None
            };
            let put_req = VaultPutConnectionRequest {
                name: existing.name,
                preset: existing.preset,
                url: existing.url,
                transport: Some(existing.transport),
                enabled: Some(existing.enabled),
                auth_kind: "oauth".into(),
                material: Some(VaultConnectionMaterial::Oauth {
                    access_token: tokens.access_token,
                    refresh_token: tokens.refresh_token,
                    token_type: tokens.token_type,
                    expires_at: tokens.expires_at,
                    token_endpoint: tokens.token_endpoint,
                    client_id: tokens.client_id,
                    client_secret: tokens.client_secret,
                    scopes: scopes.clone(),
                    resource: tokens.resource,
                }),
                authorized_devices: if existing.authorized_devices.is_empty() {
                    None
                } else {
                    Some(existing.authorized_devices)
                },
                scopes,
                account_label: existing.account_label,
            };
            let updated = vault
                .put_connection(server_id, &put_req)
                .await
                .map_err(|e| e.message)?;
            self.upsert_vault_cache(updated);
            self.publish_snapshot();
            Ok(())
        } else {
            let state = LocalOAuthState {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                token_type: tokens.token_type,
                expires_at: tokens.expires_at,
                token_endpoint: tokens.token_endpoint,
                client_id: tokens.client_id,
                client_secret: tokens.client_secret,
                scopes: tokens.scopes,
                resource: tokens.resource,
            };
            self.local_store
                .set_oauth_tokens(server_id, state, None)?
                .ok_or_else(|| format!("MCP server '{server_id}' no longer exists"))?;
            self.publish_snapshot();
            Ok(())
        }
    }
}

/// A window of this device's following the servers, while it does.
struct Window(Arc<std::sync::atomic::AtomicUsize>);

impl Window {
    fn open(windows: &Arc<std::sync::atomic::AtomicUsize>) -> Self {
        windows.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self(windows.clone())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// The snapshot as `caller` may have it. This device's own app sees it
/// whole. An agent or another device sees where a server is, never a key
/// its address or its command's arguments may hold; and another device none
/// of what agents asked for here, which only a window here answers.
fn shown_to(caller: &zeron_rpc::Caller, mut snapshot: McpServersSnapshot) -> McpServersSnapshot {
    if caller.is_local() {
        return snapshot;
    }
    for server in &mut snapshot.servers {
        server.url = server.url.as_deref().and_then(gateway::url_origin);
        server.args.clear();
    }
    if matches!(caller, zeron_rpc::Caller::Peer { .. }) {
        snapshot.requests.clear();
    }
    snapshot
}

fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("zeron/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn to_unary<T: serde::Serialize>(value: &T) -> Result<RpcReply, RpcError> {
    RpcReply::value(value)
}

fn is_remote_ready(view: &McpServerView) -> bool {
    view.enabled && (view.status == McpServerStatus::Connected || view.auth_kind == McpAuthKind::None)
}

fn build_gateway_mcp_server(
    agent_name: &str,
    server_id: &str,
    endpoint: &zeron_mcp::http::McpHttpServer,
    grant_token: &str,
    proxy_cmd: &str,
) -> McpServer {
    let url = endpoint.server_url(server_id);
    let headers: BTreeMap<String, String> = [(
        "Authorization".to_owned(),
        format!("Bearer {grant_token}"),
    )]
    .into_iter()
    .collect();
    let mut env = BTreeMap::new();
    env.insert(McpServer::URL_ENV.to_owned(), url.clone());
    env.insert(McpServer::TOKEN_ENV.to_owned(), grant_token.to_owned());

    let args = if proxy_cmd.is_empty() {
        Vec::new()
    } else {
        vec![crate::sessions::MCP_PROXY_SUBCOMMAND.to_owned()]
    };

    McpServer {
        name: agent_name.to_owned(),
        command: proxy_cmd.to_owned(),
        args,
        env,
        http: Some(McpHttp { url, headers }),
    }
}

fn build_vault_static_material(
    req: &UpsertMcpServerRequest,
) -> Result<Option<VaultConnectionMaterial>, String> {
    match req.auth_kind {
        McpAuthKind::None | McpAuthKind::OAuth => Ok(None),
        McpAuthKind::Bearer => {
            let token = req
                .bearer_token
                .as_deref()
                .map(str::trim)
                .filter(|t| !t.is_empty());
            match token {
                Some(tok) => {
                    let mut headers = req.headers.clone();
                    headers.insert("Authorization".into(), format!("Bearer {tok}"));
                    Ok(Some(VaultConnectionMaterial::Headers { headers }))
                }
                None => Ok(None),
            }
        }
        McpAuthKind::Headers => {
            if req.headers.is_empty() {
                Ok(None)
            } else {
                Ok(Some(VaultConnectionMaterial::Headers {
                    headers: req.headers.clone(),
                }))
            }
        }
    }
}

fn validate_server_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_SERVER_ID_LEN {
        return Err(format!(
            "MCP server id must be 1..={MAX_SERVER_ID_LEN} characters"
        ));
    }
    if id == RESERVED_SERVER_ID {
        return Err("MCP server id 'zeron' is reserved".into());
    }
    let bytes = id.as_bytes();
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return Err("MCP server id must start with a lowercase letter or digit".into());
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(
            "MCP server id may only contain lowercase letters, digits, '.', '_', or '-'".into(),
        );
    }
    Ok(())
}

/// `input` as a part of an agent-facing server name: lower-case ASCII
/// letters and digits, `_` between words, at most `max` long.
fn agent_slug(input: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            out.push(lower);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    out.truncate(max);
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// Each server's `agent_name`, the name agents see it by, from the list as
/// sorted (a service's accounts together). A service is a preset's, else a
/// name's: two custom servers of one name are two accounts of it. One
/// account goes by the service's name (`linear`); several by it and each's
/// account name (`linear_work`), or its place among them when it has none
/// (`linear_2`). Never `zeron` (Zeron's own server), never two alike.
fn assign_agent_names(servers: &mut [McpServerView]) {
    const SERVICE_MAX: usize = 20;
    let service = |v: &McpServerView| {
        let seed = v
            .preset
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .unwrap_or(&v.name);
        let slug = agent_slug(seed, SERVICE_MAX);
        match slug.as_str() {
            "" => "mcp".to_owned(),
            RESERVED_SERVER_ID => format!("{RESERVED_SERVER_ID}_mcp"),
            _ => slug,
        }
    };
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, view) in servers.iter().enumerate() {
        let key = service(view);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(i),
            None => groups.push((key, vec![i])),
        }
    }
    let mut taken: HashSet<String> = HashSet::from([RESERVED_SERVER_ID.to_owned()]);
    for (key, members) in groups {
        let several = members.len() > 1;
        for (place, &i) in members.iter().enumerate() {
            let wanted = if several {
                let label = servers[i]
                    .account_label
                    .as_deref()
                    .map(|l| agent_slug(l, MAX_AGENT_NAME_LEN - key.len() - 1))
                    .unwrap_or_default();
                let label = if label.is_empty() { (place + 1).to_string() } else { label };
                format!("{key}_{label}")
            } else {
                key.clone()
            };
            let mut name = wanted.clone();
            let mut n = 2;
            while taken.contains(&name) {
                let suffix = format!("_{n}");
                let keep = wanted.len().min(MAX_AGENT_NAME_LEN - suffix.len());
                name = format!("{}{suffix}", &wanted[..keep]);
                n += 1;
            }
            taken.insert(name.clone());
            servers[i].agent_name = name;
        }
    }
}

/// The account name a request asks for: `None` to keep the one there is,
/// `Some(None)` to clear it, `Some(Some(name))` to set it.
fn requested_account_label(req: &UpsertMcpServerRequest) -> Result<Option<Option<String>>, String> {
    let Some(raw) = req.account_label.as_deref() else {
        return Ok(None);
    };
    let label = raw.trim();
    if label.chars().count() > MAX_ACCOUNT_LABEL_LEN {
        return Err(format!("account name must be at most {MAX_ACCOUNT_LABEL_LEN} characters"));
    }
    if label.chars().any(char::is_control) {
        return Err("account name must be one line of text".into());
    }
    Ok(Some((!label.is_empty()).then(|| label.to_owned())))
}

fn slugify_id(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            out.push(lower);
            prev_dash = false;
        } else if matches!(lower, '.' | '_' | '-') {
            if !out.is_empty() && !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else if !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.len() > MAX_SERVER_ID_LEN {
        out.truncate(MAX_SERVER_ID_LEN);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures::StreamExt;
    use http_body_util::BodyExt;
    use hyper::{Method, StatusCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use zeron_mcp::http::{ExternalProxyRequest, McpHttpServer, ToolsFactory};

    fn local_manager() -> (tempfile::TempDir, Arc<McpServers>) {
        let dir = tempfile::tempdir().unwrap();
        let profile = EngineProfile::local(dir.path()).unwrap();
        let mgr = McpServers::new(&profile, dir.path(), "dev-local", None);
        (dir, mgr)
    }

    #[test]
    fn handles_all_ten_rpc_methods() {
        for m in [
            methods::WATCH_MCP_SERVERS,
            methods::LIST_MCP_SERVERS,
            methods::UPSERT_MCP_SERVER,
            methods::SET_MCP_SERVER_ENABLED,
            methods::DELETE_MCP_SERVER,
            methods::START_MCP_AUTH,
            methods::POLL_MCP_AUTH,
            methods::CANCEL_MCP_AUTH,
            methods::DISCONNECT_MCP_AUTH,
            methods::REFRESH_MCP_SERVERS,
        ] {
            assert!(handles(m), "expected handles({m}) == true");
        }
        assert!(!handles(methods::LIST_PLUGINS));
    }

    #[tokio::test]
    async fn rpc_upsert_enable_disconnect_delete_and_watch() {
        let (_dir, mgr) = local_manager();
        let mut rx = mgr.watch_snapshot();
        assert_eq!(rx.borrow().storage, McpStorage::Local);
        assert!(!rx.borrow().vault_available);
        assert_eq!(rx.borrow().presets.len(), 10);

        // Upsert bearer remote server via RPC
        let reply = mgr
            .handle_rpc(
                methods::UPSERT_MCP_SERVER,
                serde_json::json!({
                    "name": "Linear",
                    "preset": "linear",
                    "transport": "http",
                    "url": "https://mcp.linear.app/mcp",
                    "authKind": "bearer",
                    "bearerToken": "lin_secret_tok"
                }),
            )
            .await
            .unwrap();
        let RpcReply::Value(val) = reply else {
            panic!("expected unary reply");
        };
        let snap: McpServersSnapshot = serde_json::from_value(val).unwrap();
        assert_eq!(snap.servers.len(), 1);
        let view = &snap.servers[0];
        assert_eq!(view.id, "linear");
        assert_eq!(view.status, McpServerStatus::Connected);
        assert_eq!(view.header_keys, vec!["Authorization".to_owned()]);
        // Never leak bearer token in serialized snapshot
        assert!(!serde_json::to_string(&snap).unwrap().contains("lin_secret_tok"));

        // Watch notified
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().servers.len(), 1);

        // Disable
        let _ = mgr
            .handle_rpc(
                methods::SET_MCP_SERVER_ENABLED,
                serde_json::json!({ "id": "linear", "enabled": false }),
            )
            .await
            .unwrap();
        assert_eq!(mgr.snapshot().servers[0].status, McpServerStatus::Disabled);

        // Disconnect auth
        let _ = mgr
            .handle_rpc(
                methods::SET_MCP_SERVER_ENABLED,
                serde_json::json!({ "id": "linear", "enabled": true }),
            )
            .await
            .unwrap();
        let _ = mgr
            .handle_rpc(
                methods::DISCONNECT_MCP_AUTH,
                serde_json::json!({ "id": "linear" }),
            )
            .await
            .unwrap();
        assert_eq!(mgr.snapshot().servers[0].status, McpServerStatus::NeedsAuth);

        // Delete
        let _ = mgr
            .handle_rpc(
                methods::DELETE_MCP_SERVER,
                serde_json::json!({ "id": "linear" }),
            )
            .await
            .unwrap();
        assert!(mgr.snapshot().servers.is_empty());

        // Reject reserved "zeron" id
        let err = mgr
            .handle_rpc(
                methods::UPSERT_MCP_SERVER,
                serde_json::json!({
                    "id": "zeron",
                    "name": "Zeron",
                    "transport": "http",
                    "url": "https://example.com/mcp",
                    "authKind": "none"
                }),
            )
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn extra_mcp_servers_stamps_dual_transport_remote_and_stdio_and_skips_unready() {
        let (_dir, mgr) = local_manager();

        // 1. Enabled connected remote (no-auth)
        mgr.upsert_server(UpsertMcpServerRequest {
            id: Some("context7".into()),
            name: "Context7".into(),
            transport: McpTransport::Http,
            url: Some("https://mcp.context7.com/mcp".into()),
            auth_kind: McpAuthKind::None,
            ..Default::default()
        })
        .await
        .unwrap();

        // 2. Enabled OAuth remote WITHOUT tokens yet -> should be skipped
        mgr.upsert_server(UpsertMcpServerRequest {
            id: Some("slack".into()),
            name: "Slack".into(),
            transport: McpTransport::Http,
            url: Some("https://mcp.slack.com/mcp".into()),
            auth_kind: McpAuthKind::OAuth,
            ..Default::default()
        })
        .await
        .unwrap();

        // 3. Disabled remote -> should be skipped
        mgr.upsert_server(UpsertMcpServerRequest {
            id: Some("disabled-srv".into()),
            name: "Disabled".into(),
            transport: McpTransport::Http,
            url: Some("https://example.com/mcp".into()),
            auth_kind: McpAuthKind::None,
            enabled: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();

        // 4. Enabled stdio server -> direct stdio config
        mgr.upsert_server(UpsertMcpServerRequest {
            id: Some("local-tool".into()),
            name: "Local Tool".into(),
            transport: McpTransport::Stdio,
            command: Some("/usr/bin/my-mcp".into()),
            args: vec!["--stdio".into()],
            env: [("API_KEY".into(), "secret".into())].into(),
            ..Default::default()
        })
        .await
        .unwrap();

        let factory: ToolsFactory = Arc::new(|origin| {
            let client = zeron_rpc::memory_client(Arc::new(NoopRpc));
            Arc::new(zeron_mcp::Tools::new(Arc::new(
                zeron_mcp::Zeron::with_client(client, origin.clone()),
            )))
        });
        let ep = McpHttpServer::bind_with_external(factory, Some(mgr.gateway()))
            .await
            .unwrap();
        let grant = ep.grant(zeron_mcp::Origin::default()).unwrap();

        let extra = mgr.extra_mcp_servers(Some(&ep), Some(grant.token()));
        assert_eq!(extra.len(), 2);

        let remote = extra.iter().find(|s| s.name == "context7").unwrap();
        let http = remote.http.as_ref().expect("remote has http config");
        assert_eq!(http.url, ep.server_url("context7"));
        assert_eq!(
            http.headers.get("Authorization").map(String::as_str),
            Some(format!("Bearer {}", grant.token()).as_str())
        );
        assert_eq!(
            remote.env.get(McpServer::URL_ENV).map(String::as_str),
            Some(http.url.as_str())
        );
        assert_eq!(
            remote.env.get(McpServer::TOKEN_ENV).map(String::as_str),
            Some(grant.token())
        );
        assert_eq!(remote.args, vec![crate::sessions::MCP_PROXY_SUBCOMMAND.to_owned()]);

        // (By its name, as agents see it: "Local Tool".)
        let stdio = extra.iter().find(|s| s.name == "local_tool").unwrap();
        assert!(stdio.http.is_none());
        assert_eq!(stdio.command, "/usr/bin/my-mcp");
        assert_eq!(stdio.args, vec!["--stdio".to_owned()]);
        assert_eq!(stdio.env.get("API_KEY").map(String::as_str), Some("secret"));
    }

    async fn read_mock_http_request(
        stream: &mut tokio::net::TcpStream,
    ) -> Option<(String, HashMap<String, String>)> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };
        let header_str = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut lines = header_str.lines();
        let request_line = lines.next()?;
        let path = request_line.split_whitespace().nth(1)?.to_owned();
        let mut headers = HashMap::new();
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
            }
        }
        let content_len: usize = headers
            .get("content-length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        while buf.len() - header_end < content_len {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        Some((path, headers))
    }

    /// A server's credentials go to its own origin: not on a redirect off
    /// it, nor to where it's moved.
    #[tokio::test]
    async fn credentials_stay_with_the_server_they_were_given_for() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let serve = |location: Option<String>| {
            let seen = seen.clone();
            async move {
                let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
                let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
                let at = base.clone();
                let task = tokio::spawn(async move {
                    while let Ok((mut stream, _)) = listener.accept().await {
                        let Some((path, headers)) = read_mock_http_request(&mut stream).await else { continue };
                        seen.lock().unwrap().push(format!("{at}{path} {}", headers.get("x-api-key").cloned().unwrap_or_default()));
                        let head = match (&location, path.as_str()) {
                            (Some(to), "/away") => format!("307 Temporary Redirect\r\nLocation: {to}"),
                            (_, "/mcp") => format!("307 Temporary Redirect\r\nLocation: {at}/mcp/"),
                            _ => "200 OK".to_owned(),
                        };
                        let _ = stream.write_all(format!("HTTP/1.1 {head}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await;
                        let _ = stream.shutdown().await;
                    }
                });
                (base, task)
            }
        };
        let (other, other_task) = serve(None).await;
        let (base, task) = serve(Some(format!("{other}/stolen"))).await;
        let (_dir, mgr) = local_manager();
        let upsert = |url: String, headers: BTreeMap<String, String>| UpsertMcpServerRequest {
            id: Some("keyed".into()),
            name: "Keyed".into(),
            transport: McpTransport::Http,
            url: Some(url),
            auth_kind: McpAuthKind::Headers,
            headers,
            local_only: true,
            ..Default::default()
        };
        let key = BTreeMap::from([("X-Api-Key".to_owned(), "the-key".to_owned())]);
        mgr.upsert_server(upsert(format!("{base}/away"), key)).await.unwrap();
        let send = || async {
            mgr.gateway()
                .forward(zeron_mcp::http::ExternalProxyRequest {
                    server_id: "keyed".into(),
                    method: hyper::Method::POST,
                    headers: Default::default(),
                    body: Default::default(),
                })
                .await
                .unwrap()
                .status
                .as_u16()
        };
        assert_eq!(send().await, 307, "the redirect off it is answered, not followed");
        // One on its own origin is, with its key.
        mgr.upsert_server(upsert(format!("{base}/mcp"), BTreeMap::new())).await.unwrap();
        assert_eq!(send().await, 200);
        assert_eq!(*seen.lock().unwrap(), [format!("{base}/away the-key"), format!("{base}/mcp the-key"), format!("{base}/mcp/ the-key")]);
        // Moved: the key isn't sent there, its name and state are kept.
        mgr.set_server_enabled("keyed", false).await.unwrap();
        mgr.upsert_server(upsert(format!("{other}/moved"), BTreeMap::new())).await.unwrap();
        let stored = mgr.local_store.get("keyed").unwrap();
        assert!(stored.headers.is_empty());
        assert!(!stored.enabled && stored.name == "Keyed");
        task.abort();
        other_task.abort();
    }

    #[tokio::test]
    async fn gateway_forwards_json_sse_and_retries_once_on_401_oauth_refresh() {
        let mcp_calls = Arc::new(AtomicUsize::new(0));
        let refresh_calls = Arc::new(AtomicUsize::new(0));

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");

        let mcp_calls_srv = mcp_calls.clone();
        let refresh_calls_srv = refresh_calls.clone();
        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mcp_calls = mcp_calls_srv.clone();
                let refresh_calls = refresh_calls_srv.clone();
                tokio::spawn(async move {
                    let Some((path, headers)) = read_mock_http_request(&mut stream).await else {
                        return;
                    };
                    let auth = headers.get("authorization").map(String::as_str).unwrap_or("");
                    let accept = headers.get("accept").map(String::as_str).unwrap_or("");

                    let (status, extra_headers, body) = if path == "/token" {
                        refresh_calls.fetch_add(1, Ordering::SeqCst);
                        (
                            "200 OK",
                            "Content-Type: application/json\r\n",
                            serde_json::json!({
                                "access_token": "fresh-access-tok",
                                "refresh_token": "fresh-refresh-tok",
                                "token_type": "Bearer",
                                "expires_in": 3600
                            })
                            .to_string(),
                        )
                    } else if path == "/mcp" {
                        mcp_calls.fetch_add(1, Ordering::SeqCst);
                        if auth == "Bearer stale-access-tok" {
                            ("401 Unauthorized", "", "expired".to_owned())
                        } else if auth != "Bearer fresh-access-tok" {
                            ("403 Forbidden", "", "bad auth".to_owned())
                        } else if accept.contains("text/event-stream") {
                            (
                                "200 OK",
                                "Content-Type: text/event-stream\r\n",
                                "event: message\ndata: {\"ok\":true}\n\n".to_owned(),
                            )
                        } else {
                            (
                                "200 OK",
                                "Content-Type: application/json\r\nMcp-Session-Id: sess-42\r\n",
                                r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#.to_owned(),
                            )
                        }
                    } else {
                        ("404 Not Found", "", String::new())
                    };

                    let resp = format!(
                        "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        let (_dir, mgr) = local_manager();
        mgr.upsert_server(UpsertMcpServerRequest {
            id: Some("mock-oauth".into()),
            name: "Mock OAuth".into(),
            transport: McpTransport::Http,
            url: Some(format!("{base}/mcp")),
            auth_kind: McpAuthKind::OAuth,
            ..Default::default()
        })
        .await
        .unwrap();
        // Seed a non-expiring (by clock) but upstream-rejected access token + refresh token
        mgr.local_store
            .set_oauth_tokens(
                "mock-oauth",
                LocalOAuthState {
                    access_token: "stale-access-tok".into(),
                    refresh_token: Some("old-refresh-tok".into()),
                    token_type: Some("Bearer".into()),
                    expires_at: Some(crate::now_ms() + 3_600_000),
                    token_endpoint: format!("{base}/token"),
                    client_id: "cid-1".into(),
                    client_secret: None,
                    scopes: vec!["read".into()],
                    resource: None,
                },
                None,
            )
            .unwrap();

        let gateway = mgr.gateway();

        // 1. JSON POST: hits stale token -> 401 -> refreshes once -> retries -> 200 OK
        let mut headers = hyper::HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("accept", "application/json".parse().unwrap());
        let resp = gateway
            .forward(ExternalProxyRequest {
                server_id: "mock-oauth".into(),
                method: Method::POST,
                headers,
                body: Bytes::from(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
            })
            .await
            .unwrap();
        assert_eq!(resp.status, StatusCode::OK);
        assert_eq!(
            resp.headers.get("mcp-session-id").and_then(|v| v.to_str().ok()),
            Some("sess-42")
        );
        let body = resp.body.collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("\"tools\":[]"));
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
        assert_eq!(mcp_calls.load(Ordering::SeqCst), 2);

        // 2. SSE GET: uses refreshed token directly (no extra refresh) and streams SSE
        let mut sse_headers = hyper::HeaderMap::new();
        sse_headers.insert("accept", "text/event-stream".parse().unwrap());
        let sse_resp = gateway
            .forward(ExternalProxyRequest {
                server_id: "mock-oauth".into(),
                method: Method::GET,
                headers: sse_headers,
                body: Bytes::new(),
            })
            .await
            .unwrap();
        assert_eq!(sse_resp.status, StatusCode::OK);
        assert_eq!(
            sse_resp.headers.get("content-type").and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );
        let sse_body = sse_resp.body.collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&sse_body).contains("event: message"));
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
        assert_eq!(mcp_calls.load(Ordering::SeqCst), 3);

        // Also verify real HTTP routing through McpHttpServer::bind_with_external
        let factory: ToolsFactory = Arc::new(|origin| {
            let client = zeron_rpc::memory_client(Arc::new(NoopRpc));
            Arc::new(zeron_mcp::Tools::new(Arc::new(
                zeron_mcp::Zeron::with_client(client, origin.clone()),
            )))
        });
        let ep = McpHttpServer::bind_with_external(factory, Some(mgr.gateway()))
            .await
            .unwrap();
        let grant = ep.grant(zeron_mcp::Origin::default()).unwrap();
        let client = reqwest::Client::new();

        // Missing grant token -> 401
        let unauth = client
            .post(ep.server_url("mock-oauth"))
            .header("content-type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(unauth.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Valid grant token -> proxied through McpGateway -> upstream 200 OK
        let ok = client
            .post(ep.server_url("mock-oauth"))
            .bearer_auth(grant.token())
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), reqwest::StatusCode::OK);
        assert_eq!(
            ok.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()),
            Some("sess-42")
        );
        let ok_text = ok.text().await.unwrap();
        assert!(ok_text.contains("\"tools\":[]"));

        // Also verify watch stream emits initial snapshot
        let RpcReply::Stream(mut stream) = mgr
            .handle_rpc(methods::WATCH_MCP_SERVERS, serde_json::json!({}))
            .await
            .unwrap()
        else {
            panic!("expected stream reply");
        };
        let first = stream.next().await.unwrap();
        let snap: McpServersSnapshot = serde_json::from_value(first).unwrap();
        assert_eq!(snap.servers.len(), 1);
        assert_eq!(snap.servers[0].id, "mock-oauth");

        server_task.abort();
    }

    #[tokio::test]
    async fn vault_backed_manager_enrolls_puts_patches_and_verifies_signed_grant() {
        use base64::Engine as _;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
        use ring::signature::{ED25519, UnparsedPublicKey};

        let enrolled_pub: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let patch_count = Arc::new(AtomicUsize::new(0));
        let put_count = Arc::new(AtomicUsize::new(0));

        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");

        let enrolled_pub_srv = enrolled_pub.clone();
        let patch_count_srv = patch_count.clone();
        let put_count_srv = put_count.clone();
        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let enrolled_pub = enrolled_pub_srv.clone();
                let patch_count = patch_count_srv.clone();
                let put_count = put_count_srv.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let header_end = loop {
                        let Ok(n) = stream.read(&mut chunk).await else { return };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            break pos + 4;
                        }
                    };
                    let header_str = String::from_utf8_lossy(&buf[..header_end]).into_owned();
                    let mut lines = header_str.lines();
                    let first = lines.next().unwrap_or("");
                    let mut parts = first.split_whitespace();
                    let method = parts.next().unwrap_or("").to_owned();
                    let path = parts.next().unwrap_or("").to_owned();
                    let mut content_len = 0usize;
                    for line in lines {
                        if let Some((k, v)) = line.split_once(':')
                            && k.trim().eq_ignore_ascii_case("content-length")
                        {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                    while buf.len() - header_end < content_len {
                        let Ok(n) = stream.read(&mut chunk).await else { break };
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    let req_body = String::from_utf8_lossy(&buf[header_end..]).into_owned();

                    let (status, resp_body) = if method == "POST"
                        && path == "/vault/org-1/devices/enroll"
                    {
                        let v: Value = serde_json::from_str(&req_body).unwrap();
                        assert_eq!(v["kind"], "laptop");
                        assert_eq!(v["deviceId"], "dev-laptop-1");
                        *lock(&enrolled_pub) =
                            Some(v["publicKey"].as_str().unwrap().to_owned());
                        ("200 OK", r#"{"ok":true}"#.to_owned())
                    } else if method == "PUT" && path == "/vault/org-1/connections/mcp%3Alinear" {
                        put_count.fetch_add(1, Ordering::SeqCst);
                        let v: Value = serde_json::from_str(&req_body).unwrap();
                        assert_eq!(v["authKind"], "headers");
                        assert_eq!(
                            v["material"]["headers"]["Authorization"],
                            "Bearer lin-tok-1"
                        );
                        (
                            "200 OK",
                            serde_json::json!({
                                "id": "mcp:linear",
                                "kind": "mcp",
                                "name": v["name"],
                                "preset": "linear",
                                "url": v["url"],
                                "transport": "http",
                                "enabled": true,
                                "authKind": "headers",
                                "status": "connected",
                                "headerKeys": ["Authorization"],
                                "scopes": [],
                                "authorizedDevices": ["*"],
                                "updatedAt": 1700000000000i64
                            })
                            .to_string(),
                        )
                    } else if method == "PATCH" && path == "/vault/org-1/connections/mcp%3Alinear"
                    {
                        patch_count.fetch_add(1, Ordering::SeqCst);
                        let v: Value = serde_json::from_str(&req_body).unwrap();
                        (
                            "200 OK",
                            serde_json::json!({
                                "id": "mcp:linear",
                                "kind": "mcp",
                                "name": v["name"].as_str().unwrap_or("Linear"),
                                "preset": "linear",
                                "url": v["url"].as_str().unwrap_or("https://mcp.linear.app/mcp"),
                                "transport": "http",
                                "enabled": true,
                                "authKind": "headers",
                                "status": "connected",
                                "headerKeys": ["Authorization"],
                                "scopes": [],
                                "authorizedDevices": ["*"],
                                "updatedAt": 1700000000001i64
                            })
                            .to_string(),
                        )
                    } else if method == "POST" && path == "/vault/org-1/connections/grant" {
                        let v: Value = serde_json::from_str(&req_body).unwrap();
                        let id = v["id"].as_str().unwrap();
                        let device_id = v["deviceId"].as_str().unwrap();
                        let ts = v["ts"].as_i64().unwrap();
                        let sig = B64URL.decode(v["sig"].as_str().unwrap()).unwrap();
                        let pub_b64 = lock(&enrolled_pub).clone().expect("device enrolled first");
                        let pub_bytes = B64URL.decode(&pub_b64).unwrap();
                        let msg = vault_client::vault_grant_message(
                            "user-1",
                            device_id,
                            &format!("conn:{id}"),
                            ts,
                        );
                        UnparsedPublicKey::new(&ED25519, &pub_bytes)
                            .verify(msg.as_bytes(), &sig)
                            .expect("valid Ed25519 grant signature");
                        (
                            "200 OK",
                            serde_json::json!({
                                "id": id,
                                "url": "https://mcp.linear.app/mcp",
                                "transport": "http",
                                "headers": { "Authorization": "Bearer lin-tok-1" },
                                "generation": 1
                            })
                            .to_string(),
                        )
                    } else {
                        ("404 Not Found", r#"{"error":"not_found","message":"nope"}"#.to_owned())
                    };

                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let profile = EngineProfile::synced(dir.path(), "org-1", "user-1");
        let signer = VaultDeviceSigner::Laptop(Arc::new(LaptopDeviceKey::new(
            dir.path(),
            "dev-laptop-1",
        )));
        let vault = Arc::new(VaultClient::new(
            &base,
            "org-1",
            "user-1",
            Arc::new(zeron_rpc::StaticToken("user-bearer".into())),
            signer,
            build_http_client(),
        ));
        let mgr = McpServers::with_vault_client(&profile, dir.path(), "dev-laptop-1", Some(vault));
        assert!(mgr.snapshot().vault_available);
        assert_eq!(mgr.snapshot().storage, McpStorage::Vault);

        // 1. Create bearer server -> PUT to Vault
        mgr.upsert_server(UpsertMcpServerRequest {
            id: Some("linear".into()),
            name: "Linear".into(),
            preset: Some("linear".into()),
            transport: McpTransport::Http,
            url: Some("https://mcp.linear.app/mcp".into()),
            auth_kind: McpAuthKind::Bearer,
            bearer_token: Some("lin-tok-1".into()),
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(put_count.load(Ordering::SeqCst), 1);

        // 2. Rename without re-sending bearer token -> PATCH (preserves stored Vault material!)
        mgr.upsert_server(UpsertMcpServerRequest {
            id: Some("linear".into()),
            name: "Linear Prod".into(),
            preset: Some("linear".into()),
            transport: McpTransport::Http,
            url: Some("https://mcp.linear.app/mcp".into()),
            auth_kind: McpAuthKind::Bearer,
            bearer_token: None,
            ..Default::default()
        })
        .await
        .unwrap();
        assert_eq!(put_count.load(Ordering::SeqCst), 1);
        assert_eq!(patch_count.load(Ordering::SeqCst), 1);
        assert_eq!(mgr.snapshot().servers[0].name, "Linear Prod");

        // 3. Resolve upstream -> lazily enrolls laptop Ed25519 key + verifies Ed25519 grant signature
        let upstream = mgr.resolve_upstream("linear", false).await.unwrap();
        assert_eq!(upstream.url, "https://mcp.linear.app/mcp");
        assert_eq!(
            upstream.headers.get("Authorization").map(String::as_str),
            Some("Bearer lin-tok-1")
        );
        assert!(dir.path().join("vault-device-key").exists());

        server_task.abort();
    }

    fn view(id: &str, name: &str, preset: Option<&str>, label: Option<&str>) -> McpServerView {
        McpServerView {
            id: id.into(),
            name: name.into(),
            preset: preset.map(Into::into),
            account_label: label.map(Into::into),
            ..Default::default()
        }
    }

    fn named(servers: &mut [McpServerView]) -> Vec<(String, String)> {
        assign_agent_names(servers);
        servers.iter().map(|v| (v.id.clone(), v.agent_name.clone())).collect()
    }

    #[test]
    fn one_account_goes_by_its_services_name() {
        let mut servers = [
            view("linear-2", "Linear", Some("linear"), Some("Work")),
            view("my-api", "My API!", None, None),
        ];
        assert_eq!(
            named(&mut servers),
            [("linear-2".into(), "linear".into()), ("my-api".into(), "my_api".into())]
        );
    }

    #[test]
    fn several_accounts_go_by_their_names_or_places() {
        let mut servers = [
            view("linear", "Linear", Some("linear"), Some("Personal")),
            view("linear-2", "Linear", Some("linear"), Some("Acme Corp (EU)")),
            view("notion", "Notion", Some("notion"), None),
            view("notion-2", "Notion", Some("notion"), None),
        ];
        assert_eq!(
            named(&mut servers),
            [
                ("linear".into(), "linear_personal".into()),
                ("linear-2".into(), "linear_acme_corp_eu".into()),
                ("notion".into(), "notion_1".into()),
                ("notion-2".into(), "notion_2".into()),
            ]
        );
    }

    #[test]
    fn agent_names_are_never_zeron_alike_or_long() {
        let mut servers = [
            view("zeron-1", "Zeron", None, None),
            view("a", "Sentry", Some("sentry"), Some("Work")),
            view("b", "Sentry", Some("sentry"), Some("work")),
            view("c", "Sentry", Some("sentry"), Some("an account name far longer than any tool name allows")),
        ];
        let names = named(&mut servers);
        assert_eq!(names[0].1, "zeron_mcp");
        assert_eq!(names[1].1, "sentry_work");
        assert_eq!(names[2].1, "sentry_work_2");
        assert!(names[3].1.starts_with("sentry_an_account"));
        assert!(names.iter().all(|(_, n)| n.len() <= MAX_AGENT_NAME_LEN && !n.is_empty()));
    }

    /// An account's name is the person's: kept through updates that don't
    /// name it and through a change of sign-in, cleared only when asked, and
    /// what agents tell two accounts apart by.
    #[tokio::test]
    async fn account_names_are_kept_and_tell_accounts_apart() {
        let (_dir, mgr) = local_manager();
        let linear = |id: Option<&str>, label: Option<&str>| UpsertMcpServerRequest {
            id: id.map(Into::into),
            name: "Linear".into(),
            preset: Some("linear".into()),
            transport: McpTransport::Http,
            url: Some("https://mcp.linear.app/mcp".into()),
            auth_kind: McpAuthKind::None,
            account_label: label.map(Into::into),
            ..Default::default()
        };
        let label_of = |mgr: &McpServers, id: &str| {
            mgr.snapshot().servers.into_iter().find(|s| s.id == id).and_then(|s| s.account_label)
        };
        let agent_of = |mgr: &McpServers, id: &str| {
            mgr.snapshot().servers.into_iter().find(|s| s.id == id).map(|s| s.agent_name).unwrap()
        };

        // The first account: its service's name.
        mgr.upsert_server(linear(None, None)).await.unwrap();
        assert_eq!(agent_of(&mgr, "linear"), "linear");

        // Another, named, and the first named too: each by its own.
        mgr.upsert_server(linear(Some("linear"), Some(" Work "))).await.unwrap();
        mgr.upsert_server(linear(None, Some("Personal"))).await.unwrap();
        assert_eq!(label_of(&mgr, "linear").as_deref(), Some("Work"));
        assert_eq!(agent_of(&mgr, "linear"), "linear_work");
        assert_eq!(agent_of(&mgr, "linear-2"), "linear_personal");

        // An update that doesn't name it keeps the name, even with a new sign-in.
        let mut keyed = linear(Some("linear"), None);
        keyed.auth_kind = McpAuthKind::Bearer;
        keyed.bearer_token = Some("tok".into());
        mgr.upsert_server(keyed).await.unwrap();
        assert_eq!(label_of(&mgr, "linear").as_deref(), Some("Work"));

        // Cleared when asked; refused when it can't be a name.
        let mut cleared = linear(Some("linear-2"), Some(""));
        cleared.enabled = Some(true);
        mgr.upsert_server(cleared).await.unwrap();
        assert_eq!(label_of(&mgr, "linear-2"), None);
        assert!(mgr.upsert_server(linear(Some("linear-2"), Some("two\nlines"))).await.is_err());
        assert!(mgr.upsert_server(linear(Some("linear-2"), Some(&"x".repeat(41)))).await.is_err());

        // A run gets each account by the name agents know it by, reached by its id.
        let factory: ToolsFactory = Arc::new(|origin| {
            let client = zeron_rpc::memory_client(Arc::new(NoopRpc));
            Arc::new(zeron_mcp::Tools::new(Arc::new(
                zeron_mcp::Zeron::with_client(client, origin.clone()),
            )))
        });
        let ep = McpHttpServer::bind_with_external(factory, Some(mgr.gateway())).await.unwrap();
        let grant = ep.grant(zeron_mcp::Origin::default()).unwrap();
        mgr.upsert_server(linear(Some("linear-2"), Some("Personal"))).await.unwrap();
        let extra = mgr.extra_mcp_servers(Some(&ep), Some(grant.token()));
        let route = |name: &str| {
            extra.iter().find(|s| s.name == name).and_then(|s| s.http.as_ref()).map(|h| h.url.clone())
        };
        // (The keyed one has no key stored in this local test: only Personal is ready.)
        assert_eq!(route("linear_personal"), Some(ep.server_url("linear-2")));
    }

    /// A run lists the vault's servers again once its list is stale, and only
    /// then: a server connected on another device reaches this one's runs.
    #[tokio::test]
    async fn runs_relist_the_vaults_servers_once_stale() {
        let lists = Arc::new(AtomicUsize::new(0));
        // The vault's servers: Linear, then Context7 too (added elsewhere).
        let with_context7 = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let (lists_srv, more_srv) = (lists.clone(), with_context7.clone());
        let server_task = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let (lists, more) = (lists_srv.clone(), more_srv.clone());
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let conn = |id: &str, name: &str| {
                        serde_json::json!({
                            "id": format!("mcp:{id}"), "kind": "mcp", "name": name, "preset": id,
                            "url": format!("https://mcp.{id}.test/mcp"), "transport": "http",
                            "enabled": true, "authKind": "none", "status": "connected",
                            "headerKeys": [], "scopes": [], "authorizedDevices": ["*"],
                            "updatedAt": 1700000000000i64
                        })
                    };
                    let body = if head.starts_with("GET /vault/org-1/connections?kind=mcp ") {
                        lists.fetch_add(1, Ordering::SeqCst);
                        let mut connections = vec![conn("linear", "Linear")];
                        if more.load(Ordering::SeqCst) {
                            connections.push(conn("context7", "Context7"));
                        }
                        serde_json::json!({ "disabled": false, "connections": connections }).to_string()
                    } else {
                        r#"{"error":"not_found","message":"nope"}"#.to_owned()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let profile = EngineProfile::synced(dir.path(), "org-1", "user-1");
        let signer = VaultDeviceSigner::Laptop(Arc::new(LaptopDeviceKey::new(dir.path(), "dev-1")));
        let vault = Arc::new(VaultClient::new(
            &base,
            "org-1",
            "user-1",
            Arc::new(zeron_rpc::StaticToken("user-bearer".into())),
            signer,
            build_http_client(),
        ));
        let mgr = McpServers::with_vault_client(&profile, dir.path(), "dev-1", Some(vault));
        let names = |mgr: &McpServers| -> Vec<String> {
            let mut out: Vec<String> = mgr.snapshot().servers.into_iter().map(|s| s.id).collect();
            out.sort();
            out
        };

        // A run with no list yet: listed (the start's own listing may be it).
        mgr.refresh_for_run().await;
        assert_eq!(names(&mgr), ["linear"]);
        let listed = lists.load(Ordering::SeqCst);
        assert!(listed >= 1);

        // Another run within the age: no listing.
        with_context7.store(true, Ordering::SeqCst);
        mgr.refresh_for_run().await;
        assert_eq!(lists.load(Ordering::SeqCst), listed);
        assert_eq!(names(&mgr), ["linear"]);

        // Past the age: listed again, and Context7 (added elsewhere) is there.
        *lock(&mgr.vault_listed_at) = Some(Instant::now() - RUN_REFRESH_AGE - Duration::from_secs(1));
        mgr.refresh_for_run().await;
        assert_eq!(lists.load(Ordering::SeqCst), listed + 1);
        assert_eq!(names(&mgr), ["context7", "linear"]);

        server_task.abort();
    }

    struct NoopRpc;
    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for NoopRpc {
        async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
            Err(RpcError::UnknownMethod(method.to_owned()))
        }
    }
}
