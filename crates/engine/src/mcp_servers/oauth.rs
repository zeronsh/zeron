//! MCP OAuth 2.1 + PKCE (`S256`) + Dynamic Client Registration (RFC 7591)
//! client with RFC 9728 Protected Resource Metadata discovery, RFC 8414 /
//! OpenID Connect Authorization Server Metadata discovery, and a loopback
//! redirect callback listener.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use reqwest::Url;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use zeron_proto::{McpAuthFlowStatus, PollMcpAuthResponse, StartMcpAuthResponse};

use crate::{new_id, now_ms};

const AUTH_FLOW_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const FINISHED_FLOW_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_CALLBACK_REQUEST_BYTES: usize = 32 * 1024;
/// The loopback port a pre-registered OAuth client (one whose client ID the
/// person brings, as Slack requires) redirects to: fixed, so it can be set
/// up once in the vendor's app settings.
const PREREGISTERED_CALLBACK_PORT: u16 = 38417;

/// The redirect URI to allow in a pre-registered OAuth client's settings.
/// (`localhost`, which vendors accept as a desktop redirect; browsers try
/// its IPv4 address, where the listener is.)
pub fn preregistered_redirect_uri() -> String {
    format!("http://localhost:{PREREGISTERED_CALLBACK_PORT}/callback")
}

/// Bind the fixed callback port, giving a cancelled flow that held it a
/// moment to let go.
async fn bind_preregistered_callback() -> Result<TcpListener, String> {
    let mut last_err = None;
    for _ in 0..20 {
        match TcpListener::bind(("127.0.0.1", PREREGISTERED_CALLBACK_PORT)).await {
            Ok(listener) => return Ok(listener),
            Err(e) => last_err = Some(e),
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!(
        "port {PREREGISTERED_CALLBACK_PORT} is in use, so the sign-in can't return here: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Discovered OAuth endpoints and metadata for an MCP server.
#[derive(Debug, Clone)]
pub struct DiscoveredOAuth {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub resource: Option<String>,
    pub scopes: Vec<String>,
}

/// Completed OAuth token bundle produced by a successful callback exchange.
#[derive(Clone)]
pub struct OAuthCompletedTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub expires_at: Option<i64>,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub resource: Option<String>,
}

impl std::fmt::Debug for OAuthCompletedTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthCompletedTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_type", &self.token_type)
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("scopes", &self.scopes)
            .field("resource", &self.resource)
            .finish()
    }
}

/// Completion callback invoked once tokens are exchanged on the loopback
/// callback listener.
pub type OnOAuthComplete = Arc<
    dyn Fn(OAuthCompletedTokens) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// Notification callback invoked whenever a flow changes state (e.g. finished,
/// failed, cancelled) so the manager can republish `McpServersSnapshot`.
pub type OnFlowChange = Arc<dyn Fn() + Send + Sync>;

/// Parameters to begin an interactive OAuth flow for one MCP server.
pub struct StartFlowOptions {
    pub server_id: String,
    pub server_name: String,
    pub server_url: String,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DynamicClientRegistrationRequest {
    client_name: String,
    redirect_uris: Vec<String>,
    grant_types: Vec<String>,
    response_types: Vec<String>,
    token_endpoint_auth_method: String,
}

#[derive(Debug, Deserialize)]
struct DynamicClientRegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TokenExchangeResponse {
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

#[derive(Debug, Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    error_description: String,
}

/// Generate a random base64url (no padding) string of `num_bytes` bytes.
fn random_b64url(num_bytes: usize) -> Result<String, String> {
    let mut buf = vec![0u8; num_bytes];
    SystemRandom::new()
        .fill(&mut buf)
        .map_err(|_| "system RNG unavailable".to_owned())?;
    Ok(B64URL.encode(buf))
}

/// Compute the RFC 7636 PKCE `S256` code challenge for `verifier`.
pub fn pkce_s256_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    B64URL.encode(digest)
}

/// Parse `resource_metadata="..."` and `scope="..."` directives out of a
/// `WWW-Authenticate` response header (RFC 9728 Section 5.1).
pub fn parse_www_authenticate(header: &str) -> (Option<String>, Option<String>) {
    let mut resource_metadata = None;
    let mut scope = None;

    for (key, value) in extract_auth_params(header) {
        if key.eq_ignore_ascii_case("resource_metadata") && !value.is_empty() {
            resource_metadata = Some(value);
        } else if key.eq_ignore_ascii_case("scope") && !value.is_empty() {
            scope = Some(value);
        }
    }
    (resource_metadata, scope)
}

fn extract_auth_params(input: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b',') {
            i += 1;
        }
        let key_start = i;
        while i < bytes.len()
            && !bytes[i].is_ascii_whitespace()
            && bytes[i] != b'='
            && bytes[i] != b','
        {
            i += 1;
        }
        let key = &input[key_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            // Scheme token like `Bearer` — skip and continue parsing params.
            continue;
        }
        i += 1; // consume '='
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let mut val = String::new();
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                val.push(bytes[i] as char);
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'"' {
                i += 1;
            }
            val
        } else {
            let val_start = i;
            while i < bytes.len() && bytes[i] != b',' && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            input[val_start..i].to_owned()
        };
        if !key.is_empty() {
            out.push((key.to_owned(), value));
        }
    }
    out
}

fn origin_of(url: &Url) -> Result<String, String> {
    let scheme = url.scheme();
    let host = url
        .host_str()
        .ok_or_else(|| format!("URL '{url}' is missing a host"))?;
    let host_formatted = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    match url.port() {
        Some(port) => Ok(format!("{scheme}://{host_formatted}:{port}")),
        None => Ok(format!("{scheme}://{host_formatted}")),
    }
}

/// The site a host belongs to, roughly: its last two labels. Enough for the
/// services' own (`mcp.linear.app`, `api.vercel.com`), which is all it's
/// compared against.
fn site_of(url: &Url) -> Option<String> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    Some(labels[labels.len().saturating_sub(2)..].join("."))
}

/// Whether `resource` (what a server's metadata says its tokens are for)
/// names the server itself: its origin, and its path or one above it.
fn names_server(resource: &Url, server: &Url) -> bool {
    if resource.origin() != server.origin() {
        return false;
    }
    let (above, path) = (resource.path().trim_end_matches('/'), server.path().trim_end_matches('/'));
    path == above || path.strip_prefix(above).is_some_and(|rest| rest.starts_with('/'))
}

/// The resource (RFC 8707) a sign-in to `server` is for, or why it may not
/// go on. A server says itself where its sign-in is, so one that isn't what
/// it seems could name another service's: the person would sign in to that
/// service's real page, and the token would go to the server. So:
///
/// - metadata must be about the server itself (RFC 9728 §3.3);
/// - a token is asked for the server, by name, wherever the sign-in isn't
///   wholly the server's own (a service honouring it refuses a stranger's);
/// - a built-in service's sign-in is for that service's servers alone.
fn bound_resource(
    server: &Url,
    stated: Option<&str>,
    has_metadata: bool,
    endpoints: &[&str],
) -> Result<Option<String>, String> {
    let mut own = true;
    let server_site = site_of(server);
    let services: Vec<(String, String)> = super::presets::default_presets()
        .iter()
        .filter_map(|p| Some((site_of(&Url::parse(&p.url).ok()?)?, p.name.clone())))
        .collect();
    for endpoint in endpoints {
        let url = Url::parse(endpoint).map_err(|e| format!("invalid sign-in address '{endpoint}': {e}"))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if url.scheme() != "https" && !(loopback && server.scheme() == "http") {
            return Err("this server's sign-in isn't on https".into());
        }
        own &= url.origin() == server.origin();
        let site = site_of(&url);
        if site != server_site
            && let Some((_, service)) = services.iter().find(|(s, _)| Some(s) == site.as_ref())
        {
            return Err(format!(
                "this server asks you to sign in to {service}, but it isn't {service}'s: it would \
                 get your {service} account. Add {service} itself in Settings → MCP instead."
            ));
        }
    }
    match stated.filter(|r| !r.is_empty()) {
        Some(stated) => {
            let resource = Url::parse(stated).map_err(|e| format!("invalid resource '{stated}': {e}"))?;
            if !names_server(&resource, server) {
                return Err(format!(
                    "this server's sign-in is for another address ({stated}). If that is this \
                     server's, add it by that address instead"
                ));
            }
            Ok(Some(stated.to_owned()))
        }
        None if has_metadata || !own => {
            let mut resource = server.clone();
            resource.set_query(None);
            resource.set_fragment(None);
            Ok(Some(resource.to_string()))
        }
        // Every endpoint the server's own, and nothing said of a resource
        // (an older server): its own token, for itself.
        None => Ok(None),
    }
}

/// Discover OAuth 2.1 metadata for `server_url` using RFC 9728 Protected
/// Resource Metadata + RFC 8414 Authorization Server Metadata (with OpenID
/// Connect discovery fallback).
pub async fn discover_oauth(
    http: &reqwest::Client,
    server_url: &str,
) -> Result<DiscoveredOAuth, String> {
    let parsed_url =
        Url::parse(server_url).map_err(|e| format!("invalid MCP server URL '{server_url}': {e}"))?;
    let server_origin = origin_of(&parsed_url)?;

    // 1. Probe the MCP endpoint to collect WWW-Authenticate hints (RFC 9728).
    let mut hinted_metadata_url: Option<String> = None;
    let mut hinted_scope: Option<String> = None;

    let probe_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "zeron", "version": env!("CARGO_PKG_VERSION") }
        }
    });

    let probe_resp = http
        .post(server_url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .json(&probe_body)
        .send()
        .await
        .ok();

    if let Some(resp) = probe_resp {
        for value in resp.headers().get_all(reqwest::header::WWW_AUTHENTICATE) {
            if let Ok(s) = value.to_str() {
                let (rm, sc) = parse_www_authenticate(s);
                if hinted_metadata_url.is_none() {
                    hinted_metadata_url = rm;
                }
                if hinted_scope.is_none() {
                    hinted_scope = sc;
                }
            }
        }
    }

    // If POST did not include WWW-Authenticate, try a quick GET probe too.
    if hinted_metadata_url.is_none()
        && let Ok(resp) = http
            .get(server_url)
            .header("Accept", "application/json, text/event-stream")
            .send()
            .await
    {
        for value in resp.headers().get_all(reqwest::header::WWW_AUTHENTICATE) {
            if let Ok(s) = value.to_str() {
                let (rm, sc) = parse_www_authenticate(s);
                if hinted_metadata_url.is_none() {
                    hinted_metadata_url = rm;
                }
                if hinted_scope.is_none() {
                    hinted_scope = sc;
                }
            }
        }
    }

    // 2. Fetch RFC 9728 Protected Resource Metadata.
    let mut prm_candidates = Vec::new();
    if let Some(hint) = hinted_metadata_url {
        prm_candidates.push(hint);
    }
    let trimmed_path = parsed_url.path().trim_end_matches('/');
    if !trimmed_path.is_empty() && trimmed_path != "/" {
        prm_candidates.push(format!(
            "{server_origin}/.well-known/oauth-protected-resource{trimmed_path}"
        ));
    }
    prm_candidates.push(format!(
        "{server_origin}/.well-known/oauth-protected-resource"
    ));

    let mut prm: Option<ProtectedResourceMetadata> = None;
    for candidate in prm_candidates {
        if let Ok(resp) = http
            .get(&candidate)
            .header("Accept", "application/json")
            .send()
            .await
            && resp.status().is_success()
            && let Ok(meta) = resp.json::<ProtectedResourceMetadata>().await
        {
            prm = Some(meta);
            break;
        }
    }

    // 3. Determine Authorization Server base URL(s) and fetch RFC 8414 / OIDC metadata.
    let auth_server_bases: Vec<String> = match &prm {
        Some(meta) if !meta.authorization_servers.is_empty() => meta.authorization_servers.clone(),
        _ => vec![server_origin.clone()],
    };

    let mut auth_meta: Option<AuthServerMetadata> = None;
    for base in &auth_server_bases {
        let Ok(base_url) = Url::parse(base) else {
            continue;
        };
        let Ok(as_origin) = origin_of(&base_url) else {
            continue;
        };
        let as_path = base_url.path().trim_end_matches('/');
        let mut as_candidates = Vec::new();
        if !as_path.is_empty() && as_path != "/" {
            as_candidates.push(format!(
                "{as_origin}/.well-known/oauth-authorization-server{as_path}"
            ));
            as_candidates.push(format!(
                "{as_origin}/.well-known/openid-configuration{as_path}"
            ));
            as_candidates.push(format!(
                "{as_origin}{as_path}/.well-known/openid-configuration"
            ));
        }
        as_candidates.push(format!("{as_origin}/.well-known/oauth-authorization-server"));
        as_candidates.push(format!("{as_origin}/.well-known/openid-configuration"));

        for candidate in as_candidates {
            if let Ok(resp) = http
                .get(&candidate)
                .header("Accept", "application/json")
                .send()
                .await
                && resp.status().is_success()
                && let Ok(meta) = resp.json::<AuthServerMetadata>().await
                && !meta.authorization_endpoint.is_empty()
                && !meta.token_endpoint.is_empty()
            {
                auth_meta = Some(meta);
                break;
            }
        }
        if auth_meta.is_some() {
            break;
        }
    }

    let auth_meta = auth_meta.ok_or_else(|| {
        format!(
            "OAuth discovery failed for '{server_url}': no authorization server metadata found"
        )
    })?;

    let scopes = if let Some(hint) = hinted_scope {
        hint.split_whitespace().map(str::to_owned).collect()
    } else if let Some(ref p) = prm
        && !p.scopes_supported.is_empty()
    {
        p.scopes_supported.clone()
    } else {
        auth_meta.scopes_supported
    };

    let mut endpoints = vec![
        auth_meta.authorization_endpoint.as_str(),
        auth_meta.token_endpoint.as_str(),
    ];
    endpoints.extend(auth_meta.registration_endpoint.as_deref());
    let resource = bound_resource(
        &parsed_url,
        prm.as_ref().and_then(|p| p.resource.as_deref()),
        prm.is_some(),
        &endpoints,
    )?;

    Ok(DiscoveredOAuth {
        authorization_endpoint: auth_meta.authorization_endpoint,
        token_endpoint: auth_meta.token_endpoint,
        registration_endpoint: auth_meta.registration_endpoint,
        resource,
        scopes,
    })
}

/// Perform RFC 7591 Dynamic Client Registration for a loopback redirect URI.
async fn register_dynamic_client(
    http: &reqwest::Client,
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let payload = DynamicClientRegistrationRequest {
        client_name: "Zeron".into(),
        redirect_uris: vec![redirect_uri.to_owned()],
        grant_types: vec!["authorization_code".into(), "refresh_token".into()],
        response_types: vec!["code".into()],
        token_endpoint_auth_method: "none".into(),
    };
    let resp = http
        .post(registration_endpoint)
        .header("Accept", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("OAuth dynamic client registration failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "OAuth dynamic client registration rejected (HTTP {}): {}",
            status.as_u16(),
            body.chars().take(200).collect::<String>()
        ));
    }
    let reg: DynamicClientRegistrationResponse = resp
        .json()
        .await
        .map_err(|e| format!("parse dynamic client registration response: {e}"))?;
    if reg.client_id.trim().is_empty() {
        return Err("dynamic client registration returned an empty client_id".into());
    }
    Ok((
        reg.client_id,
        reg.client_secret.filter(|s| !s.is_empty()),
    ))
}

struct ActiveFlow {
    server_id: String,
    status: McpAuthFlowStatus,
    error: Option<String>,
    cancel: CancellationToken,
    updated_at: Instant,
}

/// Tracks active and recently completed interactive OAuth flows.
#[derive(Clone, Default)]
pub struct OAuthFlowManager {
    flows: Arc<Mutex<HashMap<String, ActiveFlow>>>,
}

impl OAuthFlowManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// True if `server_id` currently has a `Pending` OAuth flow in flight.
    pub fn is_authorizing(&self, server_id: &str) -> bool {
        lock(&self.flows)
            .values()
            .any(|f| f.server_id == server_id && f.status == McpAuthFlowStatus::Pending)
    }

    /// Cancel all pending flows for `server_id`.
    pub fn cancel_for_server(&self, server_id: &str) -> bool {
        let mut flows = lock(&self.flows);
        let mut changed = false;
        for flow in flows.values_mut() {
            if flow.server_id == server_id && flow.status == McpAuthFlowStatus::Pending {
                flow.status = McpAuthFlowStatus::Cancelled;
                flow.updated_at = Instant::now();
                flow.cancel.cancel();
                changed = true;
            }
        }
        changed
    }

    /// Cancel every active flow (called on engine shutdown).
    pub fn shutdown(&self) {
        let mut flows = lock(&self.flows);
        for flow in flows.values_mut() {
            if flow.status == McpAuthFlowStatus::Pending {
                flow.status = McpAuthFlowStatus::Cancelled;
                flow.updated_at = Instant::now();
            }
            flow.cancel.cancel();
        }
    }

    /// Poll the status of a flow by `flow_id`.
    pub fn poll(&self, flow_id: &str) -> Result<PollMcpAuthResponse, String> {
        let mut flows = lock(&self.flows);
        prune_finished(&mut flows);
        let flow = flows
            .get(flow_id)
            .ok_or_else(|| format!("OAuth flow '{flow_id}' not found"))?;
        Ok(PollMcpAuthResponse {
            status: flow.status,
            error: flow.error.clone(),
        })
    }

    /// Cancel an in-flight OAuth flow by `flow_id`.
    pub fn cancel(&self, flow_id: &str) -> bool {
        let mut flows = lock(&self.flows);
        let Some(flow) = flows.get_mut(flow_id) else {
            return false;
        };
        if flow.status == McpAuthFlowStatus::Pending {
            flow.status = McpAuthFlowStatus::Cancelled;
            flow.updated_at = Instant::now();
            flow.cancel.cancel();
            return true;
        }
        false
    }

    fn set_result(&self, flow_id: &str, status: McpAuthFlowStatus, error: Option<String>) {
        let mut flows = lock(&self.flows);
        if let Some(flow) = flows.get_mut(flow_id)
            && flow.status == McpAuthFlowStatus::Pending
        {
            flow.status = status;
            flow.error = error;
            flow.updated_at = Instant::now();
        }
    }

    /// Start an interactive OAuth 2.1 + PKCE S256 (+ DCR) flow for one MCP server.
    pub async fn start_flow(
        &self,
        http: reqwest::Client,
        opts: StartFlowOptions,
        on_complete: OnOAuthComplete,
        on_change: OnFlowChange,
    ) -> Result<StartMcpAuthResponse, String> {
        let discovered = discover_oauth(&http, &opts.server_url).await?;
        let preregistered = opts.client_id.filter(|s| !s.trim().is_empty());
        if preregistered.is_none() && discovered.registration_endpoint.is_none() {
            // No way to register a client: the caller asks the person for
            // their app's (Slack's OAuth is only ever a registered app's).
            return Err(zeron_proto::MCP_CLIENT_REQUIRED.to_owned());
        }

        // A server's previous flow ends first: a pre-registered client's
        // flows share one fixed port.
        self.cancel_for_server(&opts.server_id);

        // Bind a loopback listener for the redirect callback: any free port
        // for a client registered just now for it, the fixed one a
        // pre-registered client's redirect was set up with.
        let (listener, redirect_uri) = if preregistered.is_some() {
            (bind_preregistered_callback().await?, preregistered_redirect_uri())
        } else {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|e| format!("bind OAuth loopback callback listener: {e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("read callback listener port: {e}"))?
                .port();
            (listener, format!("http://127.0.0.1:{port}/callback"))
        };
        let callback_port = listener
            .local_addr()
            .map_err(|e| format!("read callback listener port: {e}"))?
            .port();

        // Resolve client_id via explicit override or RFC 7591 DCR.
        let (client_id, client_secret) = match preregistered {
            Some(cid) => (cid, opts.client_secret.filter(|s| !s.is_empty())),
            None => {
                let reg_endpoint = discovered
                    .registration_endpoint
                    .as_deref()
                    .ok_or_else(|| zeron_proto::MCP_CLIENT_REQUIRED.to_owned())?;
                register_dynamic_client(&http, reg_endpoint, &redirect_uri).await?
            }
        };

        let code_verifier = random_b64url(32)?;
        let code_challenge = pkce_s256_challenge(&code_verifier);
        let state = random_b64url(24)?;

        let scopes = if !opts.scopes.is_empty() {
            opts.scopes
        } else {
            discovered.scopes
        };

        let mut auth_url = Url::parse(&discovered.authorization_endpoint).map_err(|e| {
            format!(
                "invalid authorization_endpoint '{}': {e}",
                discovered.authorization_endpoint
            )
        })?;
        {
            let mut pairs = auth_url.query_pairs_mut();
            pairs.append_pair("response_type", "code");
            pairs.append_pair("client_id", &client_id);
            pairs.append_pair("redirect_uri", &redirect_uri);
            pairs.append_pair("code_challenge", &code_challenge);
            pairs.append_pair("code_challenge_method", "S256");
            pairs.append_pair("state", &state);
            if !scopes.is_empty() {
                pairs.append_pair("scope", &scopes.join(" "));
            }
            if let Some(ref resource) = discovered.resource {
                pairs.append_pair("resource", resource);
            }
        }

        let flow_id = new_id();
        let cancel = CancellationToken::new();
        {
            let mut flows = lock(&self.flows);
            prune_finished(&mut flows);
            flows.insert(
                flow_id.clone(),
                ActiveFlow {
                    server_id: opts.server_id.clone(),
                    status: McpAuthFlowStatus::Pending,
                    error: None,
                    cancel: cancel.clone(),
                    updated_at: Instant::now(),
                },
            );
        }

        let manager = self.clone();
        let bg_flow_id = flow_id.clone();
        let server_name = opts.server_name;
        let token_endpoint = discovered.token_endpoint;
        let resource = discovered.resource;

        tokio::spawn(async move {
            let outcome = tokio::select! {
                _ = cancel.cancelled() => {
                    Err((McpAuthFlowStatus::Cancelled, None))
                }
                res = tokio::time::timeout(
                    AUTH_FLOW_TIMEOUT,
                    run_callback_exchange(
                        listener,
                        &http,
                        &server_name,
                        &state,
                        &redirect_uri,
                        &token_endpoint,
                        &client_id,
                        client_secret.as_deref(),
                        &code_verifier,
                        &scopes,
                        resource.as_deref(),
                        on_complete,
                    ),
                ) => match res {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(err)) => Err((McpAuthFlowStatus::Failed, Some(err))),
                    Err(_) => Err((
                        McpAuthFlowStatus::Failed,
                        Some("OAuth authorization timed out after 5 minutes".into()),
                    )),
                }
            };

            match outcome {
                Ok(()) => manager.set_result(&bg_flow_id, McpAuthFlowStatus::Completed, None),
                Err((status, err)) => manager.set_result(&bg_flow_id, status, err),
            }
            on_change();
        });

        Ok(StartMcpAuthResponse {
            flow_id,
            auth_url: auth_url.to_string(),
            callback_port,
        })
    }
}

fn prune_finished(flows: &mut HashMap<String, ActiveFlow>) {
    flows.retain(|_, flow| {
        flow.status == McpAuthFlowStatus::Pending || flow.updated_at.elapsed() < FINISHED_FLOW_TTL
    });
}

#[allow(clippy::too_many_arguments)]
async fn run_callback_exchange(
    listener: TcpListener,
    http: &reqwest::Client,
    server_name: &str,
    expected_state: &str,
    redirect_uri: &str,
    token_endpoint: &str,
    client_id: &str,
    client_secret: Option<&str>,
    code_verifier: &str,
    requested_scopes: &[String],
    resource: Option<&str>,
    on_complete: OnOAuthComplete,
) -> Result<(), String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("accept OAuth callback connection: {e}"))?;

        let Some(target) = read_http_request_target(&mut stream).await else {
            continue;
        };

        let parsed = match Url::parse(&format!("http://127.0.0.1{target}")) {
            Ok(u) => u,
            Err(_) => {
                let _ = write_html_response(&mut stream, 400, "Bad Request", "Invalid callback URL.")
                    .await;
                continue;
            }
        };

        if parsed.path() != "/callback" {
            let _ = write_html_response(&mut stream, 404, "Not Found", "Not found.").await;
            continue;
        }

        let params: HashMap<String, String> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        // (The state first: any page may send a browser here, and only the
        // sign-in this listens for may end it.)
        let got_state = params.get("state").map(String::as_str).unwrap_or("");
        if got_state != expected_state {
            let _ = write_html_response(
                &mut stream,
                400,
                "Invalid State",
                "OAuth state mismatch. Please retry from Zeron.",
            )
            .await;
            continue;
        }

        if let Some(err) = params.get("error") {
            let desc = params
                .get("error_description")
                .filter(|d| !d.is_empty())
                .map(|d| format!("{err}: {d}"))
                .unwrap_or_else(|| err.clone());
            let _ = write_html_response(
                &mut stream,
                400,
                "Authorization Failed",
                &format!("Could not connect to {server_name}: {desc}"),
            )
            .await;
            return Err(format!("OAuth provider denied authorization: {desc}"));
        }

        let Some(code) = params.get("code").filter(|c| !c.is_empty()) else {
            let _ = write_html_response(
                &mut stream,
                400,
                "Missing Code",
                "OAuth callback did not include an authorization code.",
            )
            .await;
            return Err("OAuth callback did not include an authorization code".into());
        };

        // Exchange authorization code for tokens.
        let mut form: Vec<(&str, String)> = vec![
            ("grant_type", "authorization_code".into()),
            ("code", code.clone()),
            ("redirect_uri", redirect_uri.to_owned()),
            ("client_id", client_id.to_owned()),
            ("code_verifier", code_verifier.to_owned()),
        ];
        if let Some(secret) = client_secret.filter(|s| !s.is_empty()) {
            form.push(("client_secret", secret.to_owned()));
        }
        if let Some(res) = resource.filter(|r| !r.is_empty()) {
            form.push(("resource", res.to_owned()));
        }

        let token_res = exchange_code(
            http,
            token_endpoint,
            &form,
            client_id,
            client_secret,
            requested_scopes,
            resource,
        )
        .await;

        let tokens = match token_res {
            Ok(tokens) => tokens,
            Err(err) => {
                let _ = write_html_response(
                    &mut stream,
                    502,
                    "Token Exchange Failed",
                    &format!("Failed to finish connecting to {server_name}: {err}"),
                )
                .await;
                return Err(err);
            }
        };

        if let Err(err) = on_complete(tokens).await {
            let _ = write_html_response(
                &mut stream,
                500,
                "Save Failed",
                &format!("Connected to {server_name}, but failed to store credentials: {err}"),
            )
            .await;
            return Err(err);
        }

        let _ = write_html_response(
            &mut stream,
            200,
            "Connected",
            &format!("Connected to {server_name}. You can close this tab and return to Zeron."),
        )
        .await;
        return Ok(());
    }
}

async fn exchange_code(
    http: &reqwest::Client,
    token_endpoint: &str,
    form: &[(&str, String)],
    client_id: &str,
    client_secret: Option<&str>,
    requested_scopes: &[String],
    resource: Option<&str>,
) -> Result<OAuthCompletedTokens, String> {
    let resp = http
        .post(token_endpoint)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|e| format!("token exchange request failed: {e}"))?;
    let status = resp.status();
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("read token exchange response: {e}"))?;
    if !status.is_success() {
        let detail = serde_json::from_slice::<OAuthErrorBody>(&body)
            .ok()
            .map(|e| {
                if !e.error_description.is_empty() {
                    format!("{}: {}", e.error, e.error_description)
                } else if !e.error.is_empty() {
                    e.error
                } else {
                    format!("HTTP {}", status.as_u16())
                }
            })
            .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
        return Err(format!("token endpoint rejected exchange ({detail})"));
    }

    let parsed: TokenExchangeResponse =
        serde_json::from_slice(&body).map_err(|e| format!("parse token exchange response: {e}"))?;
    if parsed.access_token.is_empty() {
        return Err("token endpoint returned an empty access_token".into());
    }
    let expires_at = parsed
        .expires_in
        .filter(|secs| *secs > 0)
        .map(|secs| now_ms() + secs.saturating_mul(1000));
    let scopes = parsed
        .scope
        .map(|s| s.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| requested_scopes.to_vec());

    Ok(OAuthCompletedTokens {
        access_token: parsed.access_token,
        refresh_token: parsed.refresh_token.filter(|t| !t.is_empty()),
        token_type: parsed.token_type.filter(|t| !t.is_empty()),
        expires_at,
        token_endpoint: token_endpoint.to_owned(),
        client_id: client_id.to_owned(),
        client_secret: client_secret.map(str::to_owned),
        scopes,
        resource: resource.map(str::to_owned),
    })
}

async fn read_http_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= MAX_CALLBACK_REQUEST_BYTES {
            break;
        }
    }
    let text = std::str::from_utf8(&buf).ok()?;
    let request_line = text.lines().next()?;
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    Some(target.to_owned())
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn write_html_response(
    stream: &mut tokio::net::TcpStream,
    status: u16,
    title: &str,
    message: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "Error",
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{font-family:-apple-system,BlinkMacSystemFont,sans-serif;\
         max-width:480px;margin:80px auto;padding:0 24px;color:#111;line-height:1.5;}}\
         h1{{font-size:20px;margin-bottom:8px;}}p{{color:#444;}}</style></head>\
         <body><h1>{}</h1><p>{}</p></body></html>",
        escape_html(title),
        escape_html(message)
    );
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncBufReadExt;

    #[test]
    fn parse_www_authenticate_extracts_resource_metadata_and_scope() {
        let header = r#"Bearer realm="mcp", resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource", scope="read write""#;
        let (rm, scope) = parse_www_authenticate(header);
        assert_eq!(
            rm.as_deref(),
            Some("https://mcp.example.com/.well-known/oauth-protected-resource")
        );
        assert_eq!(scope.as_deref(), Some("read write"));
    }

    #[test]
    fn pkce_s256_matches_rfc7636_test_vector() {
        // RFC 7636 Appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_s256_challenge(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    /// A server may not have the person sign in to another service, nor get
    /// a token meant for another server.
    #[test]
    fn a_sign_in_is_bound_to_the_server_it_is_for() {
        let bound = |server: &str, stated: Option<&str>, meta: bool, endpoints: &[&str]| {
            bound_resource(&Url::parse(server).unwrap(), stated, meta, endpoints)
        };
        let evil = "https://evil.example/mcp";
        // Another service's resource, or its sign-in under the server's name
        // (or with no metadata of its own at all).
        assert!(bound(evil, Some("https://mcp.acme.example/mcp"), true, &["https://auth.acme.example/a"]).is_err());
        for (stated, meta) in [(Some(evil), true), (None, true), (None, false)] {
            let refused = bound(evil, stated, meta, &["https://mcp.linear.app/authorize", "https://mcp.linear.app/token"]);
            assert!(refused.unwrap_err().contains("Linear"), "{stated:?}");
            assert!(bound(evil, stated, meta, &["https://evil.example/a", "https://api.vercel.com/t"]).is_err());
        }
        // Look-alikes of the server's own.
        for other in ["https://evil.example.org/mcp", "https://evil.example/mcpx", "http://evil.example/mcp", "https://evil.example:8443/mcp", "https://evil.example/mcp/deeper"] {
            assert!(bound(evil, Some(other), true, &["https://evil.example/a"]).is_err(), "{other}");
        }
        // A sign-in elsewhere names the server, whatever it left out.
        assert_eq!(bound("https://tools.acme.example/mcp?key=1", None, false, &["https://login.acme.example/a"]).unwrap().as_deref(), Some("https://tools.acme.example/mcp"));
        assert_eq!(bound("https://tools.acme.example/mcp", None, true, &["https://tools.acme.example/a"]).unwrap().as_deref(), Some("https://tools.acme.example/mcp"));
        // Its own path or one above it, as stated.
        for stated in ["https://tools.acme.example", "https://tools.acme.example/", "https://tools.acme.example/mcp/"] {
            assert_eq!(bound("https://tools.acme.example/mcp", Some(stated), true, &["https://login.acme.example/a"]).unwrap().as_deref(), Some(stated));
        }
        // Not on https.
        assert!(bound("https://tools.acme.example/mcp", None, true, &["http://tools.acme.example/a"]).is_err());
    }

    /// The built-in services, as each publishes its sign-in (2026-11-24):
    /// every one of them still may.
    #[test]
    fn the_built_in_services_sign_in_as_they_publish() {
        for (server, stated, endpoints, resource) in [
            ("https://mcp.linear.app/mcp", Some("https://mcp.linear.app/mcp"), &["https://mcp.linear.app/authorize"][..], Some("https://mcp.linear.app/mcp")),
            ("https://mcp.notion.com/mcp", Some("https://mcp.notion.com/mcp"), &["https://mcp.notion.com/token"], Some("https://mcp.notion.com/mcp")),
            ("https://mcp.sentry.dev/mcp", Some("https://mcp.sentry.dev/mcp"), &["https://mcp.sentry.dev/oauth/token"], Some("https://mcp.sentry.dev/mcp")),
            ("https://mcp.atlassian.com/v1/mcp", None, &["https://mcp.atlassian.com/v1/authorize"], None),
            ("https://mcp.stripe.com", Some("https://mcp.stripe.com"), &["https://access.stripe.com/mcp/oauth2/token"], Some("https://mcp.stripe.com")),
            ("https://mcp.supabase.com/mcp", Some("https://mcp.supabase.com/mcp"), &["https://api.supabase.com/v1/oauth/token"], Some("https://mcp.supabase.com/mcp")),
            ("https://mcp.vercel.com", Some("https://mcp.vercel.com/"), &["https://vercel.com/oauth/authorize", "https://api.vercel.com/login/oauth/token"], Some("https://mcp.vercel.com/")),
            ("https://mcp.cloudflare.com/mcp", Some("https://mcp.cloudflare.com/mcp"), &["https://mcp.cloudflare.com/token"], Some("https://mcp.cloudflare.com/mcp")),
            ("https://mcp.posthog.com/mcp", Some("https://mcp.posthog.com/mcp"), &["https://oauth.posthog.com/oauth/token"], Some("https://mcp.posthog.com/mcp")),
            ("https://mcp.context7.com/mcp", Some("https://mcp.context7.com/mcp"), &["https://clerk.context7.com/oauth/token"], Some("https://mcp.context7.com/mcp")),
        ] {
            let got = bound_resource(&Url::parse(server).unwrap(), stated, stated.is_some(), endpoints).unwrap();
            assert_eq!(got.as_deref(), resource, "{server}");
        }
    }

    /// End-to-end test of RFC 9728 + RFC 8414 discovery, RFC 7591 DCR,
    /// PKCE S256 verification, and callback token exchange against a local
    /// mock OAuth + MCP server.
    #[tokio::test]
    async fn full_oauth_discovery_dcr_pkce_and_callback_exchange() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let base_for_server = base.clone();

        let captured_verifier = Arc::new(Mutex::new(Option::<String>::None));
        let captured_for_server = captured_verifier.clone();

        let server_task = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let base = base_for_server.clone();
                let captured = captured_for_server.clone();
                tokio::spawn(async move {
                    let mut reader = tokio::io::BufReader::new(&mut sock);
                    let mut req_line = String::new();
                    if reader.read_line(&mut req_line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let mut content_len = 0usize;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0
                            || line == "\r\n"
                            || line == "\n"
                        {
                            break;
                        }
                        if let Some((k, v)) = line.split_once(':')
                            && k.trim().eq_ignore_ascii_case("content-length")
                        {
                            content_len = v.trim().parse().unwrap_or(0);
                        }
                    }
                    let mut body_bytes = vec![0u8; content_len];
                    if content_len > 0 {
                        let _ = reader.read_exact(&mut body_bytes).await;
                    }
                    let mut parts = req_line.split_whitespace();
                    let _method = parts.next().unwrap_or("");
                    let path = parts.next().unwrap_or("/");

                    let (status, extra_headers, resp_body) = match path {
                        "/mcp" => (
                            "401 Unauthorized",
                            format!(
                                "WWW-Authenticate: Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\"\r\n"
                            ),
                            r#"{"error":"unauthorized"}"#.to_owned(),
                        ),
                        "/.well-known/oauth-protected-resource/mcp" => (
                            "200 OK",
                            String::new(),
                            serde_json::json!({
                                "resource": format!("{base}/mcp"),
                                "authorization_servers": [base],
                                "scopes_supported": ["mcp:read", "mcp:write"]
                            })
                            .to_string(),
                        ),
                        "/.well-known/oauth-authorization-server" => (
                            "200 OK",
                            String::new(),
                            serde_json::json!({
                                "issuer": base,
                                "authorization_endpoint": format!("{base}/authorize"),
                                "token_endpoint": format!("{base}/token"),
                                "registration_endpoint": format!("{base}/register"),
                                "code_challenge_methods_supported": ["S256"]
                            })
                            .to_string(),
                        ),
                        "/register" => (
                            "201 Created",
                            String::new(),
                            serde_json::json!({
                                "client_id": "dcr-client-123"
                            })
                            .to_string(),
                        ),
                        "/token" => {
                            let form_str = String::from_utf8_lossy(&body_bytes);
                            let params: HashMap<String, String> =
                                url_form_pairs(&form_str).into_iter().collect();
                            *lock(&captured) = params.get("code_verifier").cloned();
                            assert_eq!(
                                params.get("grant_type").map(String::as_str),
                                Some("authorization_code")
                            );
                            assert_eq!(
                                params.get("client_id").map(String::as_str),
                                Some("dcr-client-123")
                            );
                            assert_eq!(
                                params.get("code").map(String::as_str),
                                Some("auth-code-xyz")
                            );
                            (
                                "200 OK",
                                String::new(),
                                serde_json::json!({
                                    "access_token": "tok-access-1",
                                    "refresh_token": "tok-refresh-1",
                                    "token_type": "Bearer",
                                    "expires_in": 3600,
                                    "scope": "mcp:read mcp:write"
                                })
                                .to_string(),
                            )
                        }
                        _ => ("404 Not Found", String::new(), "{}".to_owned()),
                    };

                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{resp_body}",
                        resp_body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });

        let http = reqwest::Client::new();
        let manager = OAuthFlowManager::new();
        let saved: Arc<Mutex<Option<OAuthCompletedTokens>>> = Arc::new(Mutex::new(None));
        let saved_clone = saved.clone();

        let started = manager
            .start_flow(
                http.clone(),
                StartFlowOptions {
                    server_id: "demo".into(),
                    server_name: "Demo".into(),
                    server_url: format!("{base}/mcp"),
                    client_id: None,
                    client_secret: None,
                    scopes: Vec::new(),
                },
                Arc::new(move |tokens| {
                    let saved = saved_clone.clone();
                    Box::pin(async move {
                        *lock(&saved) = Some(tokens);
                        Ok(())
                    })
                }),
                Arc::new(|| {}),
            )
            .await
            .unwrap();

        assert!(manager.is_authorizing("demo"));
        let auth_url = Url::parse(&started.auth_url).unwrap();
        let query: HashMap<String, String> = auth_url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(query.get("client_id").map(String::as_str), Some("dcr-client-123"));
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        let challenge = query.get("code_challenge").unwrap().clone();
        let state = query.get("state").unwrap().clone();
        assert_eq!(
            query.get("resource").map(String::as_str),
            Some(format!("{base}/mcp").as_str())
        );

        // Simulate browser redirect hitting the loopback callback.
        let cb_url = format!(
            "http://127.0.0.1:{}/callback?code=auth-code-xyz&state={state}",
            started.callback_port
        );
        let cb_resp = http.get(&cb_url).send().await.unwrap();
        assert_eq!(cb_resp.status(), 200);

        // Wait briefly for flow state to transition to Completed.
        for _ in 0..50 {
            let poll = manager.poll(&started.flow_id).unwrap();
            if poll.status == McpAuthFlowStatus::Completed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let poll = manager.poll(&started.flow_id).unwrap();
        assert_eq!(poll.status, McpAuthFlowStatus::Completed);
        assert!(!manager.is_authorizing("demo"));

        // Verify PKCE S256 challenge matches the verifier sent to /token.
        let verifier = lock(&captured_verifier).clone().unwrap();
        assert_eq!(pkce_s256_challenge(&verifier), challenge);

        let tokens = lock(&saved).clone().unwrap();
        assert_eq!(tokens.access_token, "tok-access-1");
        assert_eq!(tokens.refresh_token.as_deref(), Some("tok-refresh-1"));
        assert_eq!(tokens.client_id, "dcr-client-123");
        assert_eq!(tokens.scopes, vec!["mcp:read", "mcp:write"]);

        server_task.abort();
    }

    #[tokio::test]
    #[ignore = "reads the built-in services' public metadata"]
    async fn live_built_in_services_are_discovered() {
        let http = reqwest::Client::new();
        for p in super::super::presets::default_presets() {
            if p.auth_kind != zeron_proto::McpAuthKind::OAuth { continue; }
            match discover_oauth(&http, &p.url).await {
                Ok(d) => eprintln!("ok {} resource={:?}", p.id, d.resource),
                Err(e) => panic!("{}: {e}", p.id),
            }
        }
    }

    fn url_form_pairs(body: &str) -> Vec<(String, String)> {
        Url::parse(&format!("http://example.invalid/?{body}"))
            .map(|u| {
                u.query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
