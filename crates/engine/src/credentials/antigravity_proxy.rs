//! Loopback CCPA auth reverse proxy for Cloud-managed Antigravity sessions.
//!
//! `agy_acp_server` loads `acp_token.json` once at process startup and never
//! re-reads it from disk. Because Cloud vault grants intentionally omit
//! `refresh_token`, a long-lived Antigravity turn (> ~50 minutes) used to hit
//! `Credentials.valid == False`, fall back to interactive browser OAuth, and
//! terminate with "Antigravity isn't signed in".
//!
//! Setting `AGY_ACP_CCPA_BASE_URL=http://127.0.0.1:<port>/<secret>` routes all
//! post-startup CCPA calls (`streamGenerateContent`, `generateContent`,
//! `fetchAvailableModels`, `retrieveUserQuotaSummary`, chained cleanly behind
//! `zeron-harness`'s `ThoughtsProxy`) through this loopback forwarder, which:
//! 1. Authenticates the caller via a per-broker random path secret and rejects
//!    browser `Origin` / non-loopback `Host` / non-`v1internal` paths.
//! 2. Fetches the live vault `antigravity` access token on every request and
//!    replaces the `Authorization: Bearer` header.
//! 3. Transparently force-refreshes once from the vault and retries if upstream
//!    CCPA responds with HTTP 401.
//! 4. Resolves the upstream CCPA host identically to `agy_acp_server`'s
//!    `onboard._determine_endpoint` (`cloudcode-pa.googleapis.com` when
//!    `paidTier.usesGcpTos == true`, else `daily-cloudcode-pa.googleapis.com`),
//!    re-resolving if the active vault account or project changes.
//! 5. Streams SSE responses chunk-by-chunk with no full-response buffering.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;

use base64::Engine as _;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{CredentialBroker, Grant, GrantOutcome, Inner};

pub(super) const CCPA_BASE_URL_ENV: &str = "AGY_ACP_CCPA_BASE_URL";
/// Far-future naive UTC expiry written to broker-managed `acp_token.json` when
/// this proxy is active, so `agy_acp_server`'s in-memory `Credentials.valid`
/// never flips to `False` mid-turn while fresh vault tokens are injected here.
pub(super) const PROXIED_TOKEN_EXPIRY: &str = "2099-01-01T00:00:00";

const PROD_CCPA_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const DAILY_CCPA_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
const MAX_HEADER_BYTES: usize = 64 * 1024;
const LOAD_CODE_ASSIST_TIMEOUT: Duration = Duration::from_secs(10);

type GrantFuture<'a> = Pin<Box<dyn Future<Output = GrantOutcome> + Send + 'a>>;

trait TokenProvider: Send + Sync + 'static {
    fn get_grant(&self, force_refresh: bool) -> GrantFuture<'_>;
}

struct BrokerTokenProvider {
    broker: Weak<Inner>,
}

impl TokenProvider for BrokerTokenProvider {
    fn get_grant(&self, force_refresh: bool) -> GrantFuture<'_> {
        let weak = self.broker.clone();
        Box::pin(async move {
            let Some(inner) = weak.upgrade() else {
                return GrantOutcome::Unavailable("credential broker shut down".into());
            };
            let broker = CredentialBroker { inner };
            broker.antigravity_proxy_grant(force_refresh).await
        })
    }
}

/// Running loopback CCPA auth proxy owned by [`CredentialBroker`].
pub(super) struct AntigravityProxy {
    base_url: String,
    shutdown: CancellationToken,
}

impl AntigravityProxy {
    pub(super) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(super) async fn start(
        broker: Weak<Inner>,
        fixed_upstream: Option<String>,
    ) -> std::io::Result<Self> {
        Self::start_with_provider(
            Arc::new(BrokerTokenProvider { broker }),
            fixed_upstream,
            PROD_CCPA_ENDPOINT.to_owned(),
        )
        .await
    }

    async fn start_with_provider(
        tokens: Arc<dyn TokenProvider>,
        fixed_upstream: Option<String>,
        load_code_assist_base: String,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        let secret = generate_secret()?;
        let base_url = format!("http://{addr}/{secret}");
        let shutdown = CancellationToken::new();
        // Dedicated client with NO overall request timeout so long streaming
        // `streamGenerateContent?alt=sse` turns are never cut short.
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(std::io::Error::other)?;
        let state = Arc::new(ProxyState {
            client,
            tokens,
            secret,
            fixed_upstream: fixed_upstream.map(|u| u.trim_end_matches('/').to_owned()),
            load_code_assist_base: load_code_assist_base.trim_end_matches('/').to_owned(),
            resolved_upstream: Mutex::new(None),
        });
        let cancel = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else {
                            break;
                        };
                        let state = state.clone();
                        let conn_cancel = cancel.clone();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = conn_cancel.cancelled() => {}
                                res = handle_connection(stream, state) => {
                                    if let Err(err) = res {
                                        tracing::debug!(
                                            target: "zeron_engine::credentials::antigravity_proxy",
                                            %err,
                                            "Antigravity CCPA auth proxy connection ended with error"
                                        );
                                    }
                                }
                            }
                        });
                    }
                }
            }
        });
        Ok(Self { base_url, shutdown })
    }
}

impl Drop for AntigravityProxy {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AccountKey {
    slot: Option<String>,
    project_id: Option<String>,
}

impl AccountKey {
    fn from_grant(grant: &Grant) -> Self {
        Self {
            slot: grant.slot.clone(),
            project_id: grant.settings.get("project_id").cloned(),
        }
    }
}

struct ProxyState {
    client: reqwest::Client,
    tokens: Arc<dyn TokenProvider>,
    secret: String,
    fixed_upstream: Option<String>,
    load_code_assist_base: String,
    resolved_upstream: Mutex<Option<(AccountKey, String)>>,
}

fn generate_secret() -> std::io::Result<String> {
    let mut bytes = [0u8; 24];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| std::io::Error::other("system RNG unavailable"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

struct ParsedRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn read_http_request(stream: &mut TcpStream) -> std::io::Result<Option<ParsedRequest>> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await? == 0 {
        return Ok(None);
    }
    let trimmed = request_line.trim_end_matches(['\r', '\n']);
    let mut parts = trimmed.split_whitespace();
    let Some(method) = parts.next() else {
        return Ok(None);
    };
    let Some(target) = parts.next() else {
        return Ok(None);
    };
    let method = method.to_owned();
    let target = target.to_owned();

    let mut headers = Vec::new();
    let mut total_header_bytes = request_line.len();
    let mut content_length: Option<usize> = None;
    let mut is_chunked = false;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            break;
        }
        total_header_bytes += read;
        if total_header_bytes > MAX_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP headers too large",
            ));
        }
        let line_trimmed = line.trim_end_matches(['\r', '\n']);
        if line_trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = line_trimmed.split_once(':') {
            let name = name.trim().to_owned();
            let value = value.trim().to_owned();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse::<usize>().ok();
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                is_chunked = true;
            }
            headers.push((name, value));
        }
    }

    let body = if is_chunked {
        read_chunked_body(&mut reader).await?
    } else if let Some(len) = content_length {
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        buf
    } else {
        Vec::new()
    };

    Ok(Some(ParsedRequest {
        method,
        target,
        headers,
        body,
    }))
}

async fn read_chunked_body<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let mut size_line = String::new();
        if reader.read_line(&mut size_line).await? == 0 {
            break;
        }
        let size_hex = size_line
            .trim()
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        let size = usize::from_str_radix(size_hex, 16).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid chunk size")
        })?;
        if size == 0 {
            loop {
                let mut trailer = String::new();
                if reader.read_line(&mut trailer).await? == 0 {
                    break;
                }
                if trailer.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
            }
            break;
        }
        let start = out.len();
        out.resize(start + size, 0);
        reader.read_exact(&mut out[start..]).await?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).await?;
    }
    Ok(out)
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix("[::1]") {
        return rest.is_empty() || rest.starts_with(':');
    }
    let name = host.split(':').next().unwrap_or("");
    name.eq_ignore_ascii_case("127.0.0.1") || name.eq_ignore_ascii_case("localhost")
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Validate the per-broker secret prefix and strip it, then ensure the
/// remaining target is a safe CCPA `v1internal` path.
fn validate_and_strip_target<'a>(target: &'a str, secret: &str) -> Option<&'a str> {
    let after_slash = target.strip_prefix('/')?;
    let (candidate_secret, rest_without_slash) = after_slash.split_once('/')?;
    if !constant_time_eq(candidate_secret.as_bytes(), secret.as_bytes()) {
        return None;
    }
    let stripped = &target[1 + secret.len()..];
    debug_assert_eq!(&stripped[1..], rest_without_slash);
    if !is_allowed_ccpa_target(stripped) {
        return None;
    }
    Some(stripped)
}

fn is_allowed_ccpa_target(target: &str) -> bool {
    if target.contains('\r') || target.contains('\n') || target.contains('@') {
        return false;
    }
    let path = target.split('?').next().unwrap_or(target);
    if path.contains("..") || path.contains('\\') {
        return false;
    }
    path.starts_with("/v1internal:") || path.starts_with("/v1internal/")
}

fn is_hop_by_hop_or_auth_request_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("host")
        || name.eq_ignore_ascii_case("connection")
        || name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("transfer-encoding")
        || name.eq_ignore_ascii_case("keep-alive")
        || name.eq_ignore_ascii_case("proxy-connection")
        || name.eq_ignore_ascii_case("te")
        || name.eq_ignore_ascii_case("trailer")
        || name.eq_ignore_ascii_case("upgrade")
        || name.eq_ignore_ascii_case("accept-encoding")
}

fn is_hop_by_hop_response_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("connection")
        || name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("transfer-encoding")
        || name.eq_ignore_ascii_case("content-encoding")
        || name.eq_ignore_ascii_case("keep-alive")
        || name.eq_ignore_ascii_case("proxy-connection")
        || name.eq_ignore_ascii_case("te")
        || name.eq_ignore_ascii_case("trailer")
        || name.eq_ignore_ascii_case("upgrade")
}

fn endpoint_from_load_code_assist(load_response: &Value) -> &'static str {
    let uses_gcp_tos = load_response
        .get("paidTier")
        .and_then(|t| t.get("usesGcpTos"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if uses_gcp_tos {
        PROD_CCPA_ENDPOINT
    } else {
        DAILY_CCPA_ENDPOINT
    }
}

async fn resolve_upstream(
    state: &ProxyState,
    grant: &Grant,
    headers: &[(String, String)],
) -> String {
    if let Some(fixed) = &state.fixed_upstream {
        return fixed.clone();
    }
    let key = AccountKey::from_grant(grant);
    let mut cached = state.resolved_upstream.lock().await;
    if let Some((cached_key, url)) = cached.as_ref()
        && *cached_key == key
    {
        return url.clone();
    }
    let ua = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
        .map(|(_, v)| v.as_str());
    let mut req = state
        .client
        .post(format!(
            "{}/v1internal:loadCodeAssist",
            state.load_code_assist_base
        ))
        .timeout(LOAD_CODE_ASSIST_TIMEOUT)
        .bearer_auth(&grant.access_token)
        .header("content-type", "application/json")
        .body(r#"{"metadata":{"ideType":"ANTIGRAVITY"}}"#);
    if let Some(ua) = ua {
        req = req.header("user-agent", ua);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<Value>().await {
                let resolved = endpoint_from_load_code_assist(&json).to_owned();
                tracing::debug!(
                    target: "zeron_engine::credentials::antigravity_proxy",
                    endpoint = %resolved,
                    "resolved Antigravity CCPA upstream endpoint"
                );
                *cached = Some((key, resolved.clone()));
                return resolved;
            }
        }
        Ok(resp) => {
            tracing::debug!(
                target: "zeron_engine::credentials::antigravity_proxy",
                status = %resp.status(),
                "loadCodeAssist returned non-2xx; defaulting to daily CCPA endpoint"
            );
        }
        Err(err) => {
            tracing::debug!(
                target: "zeron_engine::credentials::antigravity_proxy",
                %err,
                "loadCodeAssist failed; defaulting to daily CCPA endpoint"
            );
        }
    }
    DAILY_CCPA_ENDPOINT.to_owned()
}

async fn send_upstream(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    access_token: &str,
    body: Vec<u8>,
) -> Result<reqwest::Response, reqwest::Error> {
    let parsed_method =
        reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::POST);
    let mut req = client.request(parsed_method, url).bearer_auth(access_token);
    for (name, value) in headers {
        if !is_hop_by_hop_or_auth_request_header(name) {
            req = req.header(name.as_str(), value.as_str());
        }
    }
    if !body.is_empty() {
        req = req.body(body);
    }
    req.send().await
}

async fn write_error_response(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await
}

async fn stream_chunked_response(
    stream: &mut TcpStream,
    mut resp: reqwest::Response,
) -> std::io::Result<()> {
    let status = resp.status();
    let reason = status.canonical_reason().unwrap_or("OK");
    let mut head = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), reason);
    for (name, value) in resp.headers() {
        let name_str = name.as_str();
        if is_hop_by_hop_response_header(name_str) {
            continue;
        }
        if let Ok(val) = value.to_str() {
            head.push_str(name_str);
            head.push_str(": ");
            head.push_str(val);
            head.push_str("\r\n");
        }
    }
    head.push_str("Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await?;

    while let Some(chunk) = resp.chunk().await.map_err(std::io::Error::other)? {
        if chunk.is_empty() {
            continue;
        }
        let size_hdr = format!("{:x}\r\n", chunk.len());
        stream.write_all(size_hdr.as_bytes()).await?;
        stream.write_all(&chunk).await?;
        stream.write_all(b"\r\n").await?;
        stream.flush().await?;
    }
    stream.write_all(b"0\r\n\r\n").await?;
    stream.flush().await
}

async fn handle_connection(mut stream: TcpStream, state: Arc<ProxyState>) -> std::io::Result<()> {
    let Some(req) = read_http_request(&mut stream).await? else {
        return Ok(());
    };

    if req
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("origin"))
    {
        return write_error_response(&mut stream, 403, "Forbidden", "Origin not allowed").await;
    }
    if let Some((_, host)) = req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        && !is_loopback_host(host)
    {
        return write_error_response(&mut stream, 403, "Forbidden", "Non-loopback Host").await;
    }
    let Some(target) = validate_and_strip_target(&req.target, &state.secret) else {
        return write_error_response(&mut stream, 403, "Forbidden", "Invalid CCPA proxy path")
            .await;
    };

    let grant = match state.tokens.get_grant(false).await {
        GrantOutcome::Granted(grant) => grant,
        GrantOutcome::NotConnected(reason) => {
            return write_error_response(&mut stream, 401, "Unauthorized", &reason).await;
        }
        GrantOutcome::Unavailable(reason) => {
            return write_error_response(&mut stream, 503, "Service Unavailable", &reason).await;
        }
    };

    let upstream_base = resolve_upstream(&state, &grant, &req.headers).await;
    let url = format!("{upstream_base}{target}");
    let resp = match send_upstream(
        &state.client,
        &req.method,
        &url,
        &req.headers,
        &grant.access_token,
        req.body.clone(),
    )
    .await
    {
        Ok(resp) => resp,
        Err(err) => {
            let msg = format!("upstream CCPA request failed: {err}");
            return write_error_response(&mut stream, 502, "Bad Gateway", &msg).await;
        }
    };

    // If upstream rejects the token with HTTP 401, force-refresh once from the
    // vault and retry before committing response headers.
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        tracing::info!(
            target: "zeron_engine::credentials::antigravity_proxy",
            "CCPA returned 401; forcing vault refresh for Antigravity token"
        );
        if let GrantOutcome::Granted(fresh) = state.tokens.get_grant(true).await {
            let retry_base = resolve_upstream(&state, &fresh, &req.headers).await;
            let retry_url = format!("{retry_base}{target}");
            match send_upstream(
                &state.client,
                &req.method,
                &retry_url,
                &req.headers,
                &fresh.access_token,
                req.body,
            )
            .await
            {
                Ok(retry_resp) => return stream_chunked_response(&mut stream, retry_resp).await,
                Err(err) => {
                    let msg = format!("upstream CCPA retry failed: {err}");
                    return write_error_response(&mut stream, 502, "Bad Gateway", &msg).await;
                }
            }
        }
    }

    stream_chunked_response(&mut stream, resp).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockTokenProvider {
        tokens: std::sync::Mutex<Vec<String>>,
        force_refreshes: AtomicUsize,
    }

    impl MockTokenProvider {
        fn new(initial: &str) -> Self {
            Self {
                tokens: std::sync::Mutex::new(vec![initial.to_owned()]),
                force_refreshes: AtomicUsize::new(0),
            }
        }

        fn push(&self, token: &str) {
            self.tokens.lock().unwrap().push(token.to_owned());
        }
    }

    impl TokenProvider for MockTokenProvider {
        fn get_grant(&self, force_refresh: bool) -> GrantFuture<'_> {
            let token = {
                let mut list = self.tokens.lock().unwrap();
                if force_refresh {
                    self.force_refreshes.fetch_add(1, Ordering::SeqCst);
                    if list.len() > 1 {
                        list.remove(0);
                    }
                }
                list[0].clone()
            };
            Box::pin(async move {
                GrantOutcome::Granted(Grant {
                    provider: "antigravity".into(),
                    access_token: token,
                    expires_at: chrono::Utc::now().timestamp_millis() + 3_600_000,
                    account_id: None,
                    id_token: None,
                    account: None,
                    scopes: None,
                    subscription_type: None,
                    settings: BTreeMap::new(),
                    generation: 1,
                    slot: Some("acct-1".into()),
                })
            })
        }
    }

    async fn start_mock_upstream<F>(handler: F) -> (String, CancellationToken)
    where
        F: Fn(ParsedRequest) -> (u16, Vec<(String, String)>, Vec<Vec<u8>>) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let handler = Arc::new(handler);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((mut stream, _)) = accepted else { break };
                        let handler = handler.clone();
                        tokio::spawn(async move {
                            let Ok(Some(req)) = read_http_request(&mut stream).await else { return };
                            let (status, headers, chunks) = handler(req);
                            let mut head = format!("HTTP/1.1 {status} OK\r\n");
                            for (k, v) in headers {
                                head.push_str(&format!("{k}: {v}\r\n"));
                            }
                            head.push_str("Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n");
                            let _ = stream.write_all(head.as_bytes()).await;
                            for chunk in chunks {
                                let _ = stream.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await;
                                let _ = stream.write_all(&chunk).await;
                                let _ = stream.write_all(b"\r\n").await;
                                let _ = stream.flush().await;
                            }
                            let _ = stream.write_all(b"0\r\n\r\n").await;
                            let _ = stream.flush().await;
                        });
                    }
                }
            }
        });
        (format!("http://{addr}"), cancel)
    }

    #[test]
    fn validates_secret_and_ccpa_target_paths() {
        assert_eq!(
            validate_and_strip_target(
                "/sec123/v1internal:streamGenerateContent?alt=sse",
                "sec123"
            ),
            Some("/v1internal:streamGenerateContent?alt=sse")
        );
        assert_eq!(
            validate_and_strip_target("/sec123/v1internal/operations/op-1", "sec123"),
            Some("/v1internal/operations/op-1")
        );
        assert_eq!(
            validate_and_strip_target("/wrong/v1internal:generateContent", "sec123"),
            None
        );
        assert_eq!(
            validate_and_strip_target("/sec123/other/path", "sec123"),
            None
        );
        assert_eq!(
            validate_and_strip_target("/sec123/v1internal:foo/../bar", "sec123"),
            None
        );
        assert_eq!(
            validate_and_strip_target("/sec123/v1internal:foo@evil.com", "sec123"),
            None
        );
    }

    #[test]
    fn resolves_endpoint_from_load_code_assist_response() {
        assert_eq!(
            endpoint_from_load_code_assist(&serde_json::json!({"paidTier": {"usesGcpTos": true}})),
            PROD_CCPA_ENDPOINT
        );
        assert_eq!(
            endpoint_from_load_code_assist(&serde_json::json!({"paidTier": {"usesGcpTos": false}})),
            DAILY_CCPA_ENDPOINT
        );
        assert_eq!(
            endpoint_from_load_code_assist(&serde_json::json!({})),
            DAILY_CCPA_ENDPOINT
        );
    }

    #[tokio::test]
    async fn injects_fresh_token_streams_sse_and_retries_once_on_401() {
        let seen_auth = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen_auth_clone = seen_auth.clone();
        let (upstream, stop_upstream) = start_mock_upstream(move |req| {
            let auth = req
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            seen_auth_clone.lock().unwrap().push(auth.clone());
            if auth == "Bearer ya29.expired" {
                (
                    401,
                    vec![("content-type".into(), "application/json".into())],
                    vec![b"{\"error\":\"expired\"}".to_vec()],
                )
            } else {
                (
                    200,
                    vec![("content-type".into(), "text/event-stream".into())],
                    vec![
                        b"data: {\"chunk\":1}\n\n".to_vec(),
                        b"data: {\"chunk\":2}\n\n".to_vec(),
                    ],
                )
            }
        })
        .await;

        let provider = Arc::new(MockTokenProvider::new("ya29.expired"));
        provider.push("ya29.fresh");
        let proxy = AntigravityProxy::start_with_provider(
            provider.clone(),
            Some(upstream.clone()),
            upstream,
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        // Client sends a completely stale startup token; proxy replaces it,
        // hits 401 with `ya29.expired`, force-refreshes to `ya29.fresh`, and
        // streams SSE chunks back cleanly.
        let resp = client
            .post(format!(
                "{}/v1internal:streamGenerateContent?alt=sse",
                proxy.base_url()
            ))
            .header("authorization", "Bearer ya29.stale-startup")
            .header("content-type", "application/json")
            .body(r#"{"model":"gemini-4-neon"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body = resp.text().await.unwrap();
        assert_eq!(body, "data: {\"chunk\":1}\n\ndata: {\"chunk\":2}\n\n");
        assert_eq!(provider.force_refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(
            *seen_auth.lock().unwrap(),
            vec!["Bearer ya29.expired", "Bearer ya29.fresh"]
        );

        // Reject browser Origin header and wrong secret.
        let origin_resp = client
            .post(format!(
                "{}/v1internal:generateContent",
                proxy.base_url()
            ))
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(origin_resp.status(), reqwest::StatusCode::FORBIDDEN);

        let host = proxy.base_url().rsplit_once('/').unwrap().0;
        let bad_secret = client
            .post(format!("{host}/bad-secret/v1internal:generateContent"))
            .send()
            .await
            .unwrap();
        assert_eq!(bad_secret.status(), reqwest::StatusCode::FORBIDDEN);

        let bad_path = client
            .post(format!("{}/v1/metadata", proxy.base_url()))
            .send()
            .await
            .unwrap();
        assert_eq!(bad_path.status(), reqwest::StatusCode::FORBIDDEN);

        stop_upstream.cancel();
    }
}
