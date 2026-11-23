//! Local loopback proxy for Antigravity's CCPA backend that requests Gemini/Claude
//! thought summaries (`generationConfig.thinkingConfig.includeThoughts = true`).
//!
//! `agy_acp_server` (1.5.0) builds OAuth model targets via `_build_literal_model_target`,
//! leaving `endpoint.options = None`, so `localharness_external` omits `thinkingConfig`
//! on `v1internal:streamGenerateContent`. CCPA still runs internal thinking (spending
//! `thoughtsTokenCount` and returning encrypted `thoughtSignature`), but withholds
//! human-readable `"thought": true` summary parts unless `includeThoughts: true` is sent.
//!
//! Passing `AGY_ACP_CCPA_BASE_URL=http://127.0.0.1:<port>` routes `agy_acp_server`'s
//! CCPA HTTP calls through this lightweight in-process forwarder, which:
//! 1. Resolves the user's real CCPA endpoint identically to `onboard._determine_endpoint`
//!    (`cloudcode-pa.googleapis.com` for GCP-TOS tiers, `daily-cloudcode-pa.googleapis.com`
//!    for consumer tiers), or forwards to an explicit caller-supplied `AGY_ACP_CCPA_BASE_URL`.
//! 2. Injects `request.generationConfig.thinkingConfig.includeThoughts = true` on
//!    `streamGenerateContent` / `generateContent` when not already set.
//! 3. Streams SSE responses chunk-by-chunk with zero full-response buffering, while
//!    transparently retrying once without `includeThoughts` (and caching the model in a
//!    skip list) if upstream returns HTTP 400 rejecting thinking config for a model.

use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, watch};
use tokio_util::sync::CancellationToken;

const PROD_CCPA_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const DAILY_CCPA_ENDPOINT: &str = "https://daily-cloudcode-pa.googleapis.com";
const MAX_HEADER_BYTES: usize = 64 * 1024;
const LOAD_CODE_ASSIST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) const CCPA_BASE_URL_ENV: &str = "AGY_ACP_CCPA_BASE_URL";
const DISABLE_PROXY_ENV: &str = "ZERON_ANTIGRAVITY_THOUGHTS_PROXY";

/// Running loopback CCPA proxy whose lifetime is tied to the spawned Antigravity child.
pub(crate) struct ThoughtsProxy {
    base_url: String,
    shutdown: CancellationToken,
    activity: watch::Receiver<u64>,
}

impl ThoughtsProxy {
    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Subscribe to monotonic CCPA HTTP liveness ticks (incremented on every
    /// proxied request start and streaming response chunk).
    pub(crate) fn subscribe_activity(&self) -> watch::Receiver<u64> {
        self.activity.clone()
    }

    /// Start the loopback proxy unless disabled via `ZERON_ANTIGRAVITY_THOUGHTS_PROXY=0`.
    pub(crate) async fn start(
        run_env: &BTreeMap<String, String>,
    ) -> std::io::Result<Option<Self>> {
        if is_disabled(run_env) {
            return Ok(None);
        }
        let fixed_upstream = configured_upstream(run_env);
        Self::start_with_upstream(fixed_upstream).await.map(Some)
    }

    pub(crate) async fn start_with_upstream(
        fixed_upstream: Option<String>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        let base_url = format!("http://{addr}");
        let shutdown = CancellationToken::new();
        let (activity_tx, activity_rx) = watch::channel(0u64);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(std::io::Error::other)?;
        let state = Arc::new(ProxyState {
            client,
            fixed_upstream: fixed_upstream.map(|u| u.trim_end_matches('/').to_owned()),
            resolved_upstream: Mutex::new(None),
            skip_models: Mutex::new(HashSet::new()),
            activity: activity_tx,
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
                                            target: "zeron_harness::acp::ccpa_thoughts",
                                            %err,
                                            "CCPA thoughts proxy connection ended with error"
                                        );
                                    }
                                }
                            }
                        });
                    }
                }
            }
        });
        Ok(Self {
            base_url,
            shutdown,
            activity: activity_rx,
        })
    }
}

impl Drop for ThoughtsProxy {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

struct ProxyState {
    client: reqwest::Client,
    fixed_upstream: Option<String>,
    resolved_upstream: Mutex<Option<String>>,
    skip_models: Mutex<HashSet<String>>,
    activity: watch::Sender<u64>,
}

impl ProxyState {
    fn note_activity(&self) {
        self.activity.send_modify(|n| *n = n.wrapping_add(1));
    }
}

fn is_disabled(run_env: &BTreeMap<String, String>) -> bool {
    let val = run_env
        .get(DISABLE_PROXY_ENV)
        .cloned()
        .or_else(|| std::env::var(DISABLE_PROXY_ENV).ok());
    matches!(
        val.as_deref().map(str::trim),
        Some("0" | "false" | "FALSE" | "off" | "OFF")
    )
}

fn configured_upstream(run_env: &BTreeMap<String, String>) -> Option<String> {
    run_env
        .get(CCPA_BASE_URL_ENV)
        .cloned()
        .or_else(|| std::env::var(CCPA_BASE_URL_ENV).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
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
    let n = reader.read_line(&mut request_line).await?;
    if n == 0 {
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
            // Drain trailer headers through empty line.
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

async fn resolve_upstream(state: &ProxyState, headers: &[(String, String)]) -> String {
    if let Some(fixed) = &state.fixed_upstream {
        return fixed.clone();
    }
    let mut cached = state.resolved_upstream.lock().await;
    if let Some(url) = cached.as_ref() {
        return url.clone();
    }
    let auth = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.as_str());
    let Some(auth) = auth else {
        return DAILY_CCPA_ENDPOINT.to_owned();
    };
    let ua = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("user-agent"))
        .map(|(_, v)| v.as_str());

    let mut req = state
        .client
        .post(format!("{PROD_CCPA_ENDPOINT}/v1internal:loadCodeAssist"))
        .timeout(LOAD_CODE_ASSIST_TIMEOUT)
        .header("authorization", auth)
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
                    target: "zeron_harness::acp::ccpa_thoughts",
                    endpoint = %resolved,
                    "resolved CCPA upstream endpoint from loadCodeAssist"
                );
                *cached = Some(resolved.clone());
                return resolved;
            }
        }
        Ok(resp) => {
            tracing::debug!(
                target: "zeron_harness::acp::ccpa_thoughts",
                status = %resp.status(),
                "loadCodeAssist returned non-2xx; defaulting to daily CCPA endpoint"
            );
        }
        Err(err) => {
            tracing::debug!(
                target: "zeron_harness::acp::ccpa_thoughts",
                %err,
                "loadCodeAssist failed; defaulting to daily CCPA endpoint"
            );
        }
    }
    DAILY_CCPA_ENDPOINT.to_owned()
}

/// Mirrors `onboard._determine_endpoint` from `agy_acp_server`:
/// Enterprise/Workspace (`paidTier.usesGcpTos == true`) -> `cloudcode-pa`,
/// Consumer tiers -> `daily-cloudcode-pa`.
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

fn is_generate_content_path(target: &str) -> bool {
    let path = target.split('?').next().unwrap_or(target);
    path.ends_with(":streamGenerateContent") || path.ends_with(":generateContent")
}

/// Inject `request.generationConfig.thinkingConfig.includeThoughts = true` if
/// absent and `model` is not in `skip_models`. Returns `Some((new_body, model))`
/// when modified.
fn maybe_inject_include_thoughts(
    body: &[u8],
    skip_models: &HashSet<String>,
) -> Option<(Vec<u8>, Option<String>)> {
    let mut root: Value = serde_json::from_slice(body).ok()?;
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(m) = model.as_deref()
        && skip_models.contains(m)
    {
        return None;
    }
    let request = root.get_mut("request")?.as_object_mut()?;
    let gen_cfg = request
        .entry("generationConfig")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    let thinking_cfg = gen_cfg
        .entry("thinkingConfig")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    if thinking_cfg.contains_key("includeThoughts")
        || thinking_cfg.contains_key("include_thoughts")
    {
        return None;
    }
    thinking_cfg.insert("includeThoughts".to_owned(), Value::Bool(true));
    let encoded = serde_json::to_vec(&root).ok()?;
    Some((encoded, model))
}

fn is_hop_by_hop_request_header(name: &str) -> bool {
    name.eq_ignore_ascii_case("host")
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

fn looks_like_thinking_rejection(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    text.contains("thinking") || text.contains("include_thoughts") || text.contains("includethoughts")
}

async fn send_upstream(
    client: &reqwest::Client,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<reqwest::Response, reqwest::Error> {
    let parsed_method =
        reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::POST);
    let mut req = client.request(parsed_method, url);
    for (name, value) in headers {
        if !is_hop_by_hop_request_header(name) {
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

async fn write_buffered_response(
    stream: &mut TcpStream,
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = status.canonical_reason().unwrap_or("OK");
    let mut head = format!("HTTP/1.1 {} {}\r\n", status.as_u16(), reason);
    for (name, value) in headers {
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
    head.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ));
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

async fn stream_chunked_response(
    stream: &mut TcpStream,
    mut resp: reqwest::Response,
    state: &ProxyState,
) -> std::io::Result<()> {
    state.note_activity();
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
        state.note_activity();
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
    state.note_activity();
    let upstream_base = resolve_upstream(&state, &req.headers).await;
    let target = if req.target.starts_with('/') {
        req.target.as_str()
    } else {
        "/"
    };
    let url = format!("{upstream_base}{target}");

    let injected = if req.method.eq_ignore_ascii_case("POST") && is_generate_content_path(&req.target)
    {
        let skip = state.skip_models.lock().await;
        maybe_inject_include_thoughts(&req.body, &skip)
    } else {
        None
    };

    let (send_body, injected_model) = match injected {
        Some((modified, model)) => (modified, Some(model)),
        None => (req.body.clone(), None),
    };

    let resp = match send_upstream(&state.client, &req.method, &url, &req.headers, send_body).await
    {
        Ok(r) => r,
        Err(err) => {
            let msg = format!("upstream CCPA request failed: {err}");
            let _ = write_error_response(&mut stream, 502, "Bad Gateway", &msg).await;
            return Ok(());
        }
    };

    // If upstream rejects our injected `includeThoughts` with HTTP 400, remember
    // the model and retry once with the unmodified original request body.
    if injected_model.is_some() && resp.status() == reqwest::StatusCode::BAD_REQUEST {
        let status = resp.status();
        let headers = resp.headers().clone();
        let err_body = resp.bytes().await.unwrap_or_default();
        if looks_like_thinking_rejection(&err_body) {
            if let Some(Some(model)) = injected_model {
                tracing::debug!(
                    target: "zeron_harness::acp::ccpa_thoughts",
                    %model,
                    "upstream rejected includeThoughts; retrying without it and caching skip"
                );
                state.skip_models.lock().await.insert(model);
            }
            match send_upstream(&state.client, &req.method, &url, &req.headers, req.body).await {
                Ok(retry_resp) => {
                    return stream_chunked_response(&mut stream, retry_resp, &state).await;
                }
                Err(err) => {
                    let msg = format!("upstream CCPA retry failed: {err}");
                    let _ = write_error_response(&mut stream, 502, "Bad Gateway", &msg).await;
                    return Ok(());
                }
            }
        }
        state.note_activity();
        return write_buffered_response(&mut stream, status, &headers, &err_body).await;
    }

    stream_chunked_response(&mut stream, resp, &state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn injects_include_thoughts_when_missing() {
        let body = serde_json::to_vec(&json!({
            "project": "p1",
            "model": "gemini-4-neon-high",
            "request": {
                "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
                "generationConfig": {"maxOutputTokens": 65535}
            }
        }))
        .unwrap();
        let (out, model) = maybe_inject_include_thoughts(&body, &HashSet::new()).unwrap();
        assert_eq!(model.as_deref(), Some("gemini-4-neon-high"));
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["request"]["generationConfig"]["maxOutputTokens"], 65535);
        assert_eq!(
            parsed["request"]["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
    }

    #[test]
    fn preserves_existing_thinking_config_and_skips_when_already_set() {
        let with_budget = serde_json::to_vec(&json!({
            "model": "gemini-4-neon-high",
            "request": {
                "generationConfig": {
                    "thinkingConfig": {"thinkingBudget": 1024}
                }
            }
        }))
        .unwrap();
        let (out, _) = maybe_inject_include_thoughts(&with_budget, &HashSet::new()).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            parsed["request"]["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            1024
        );
        assert_eq!(
            parsed["request"]["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );

        let already_set = serde_json::to_vec(&json!({
            "model": "gemini-4-neon-high",
            "request": {
                "generationConfig": {
                    "thinkingConfig": {"includeThoughts": false}
                }
            }
        }))
        .unwrap();
        assert!(maybe_inject_include_thoughts(&already_set, &HashSet::new()).is_none());
    }

    #[test]
    fn skips_models_in_skip_set() {
        let body = serde_json::to_vec(&json!({
            "model": "non-thinking-model",
            "request": {"generationConfig": {}}
        }))
        .unwrap();
        let mut skip = HashSet::new();
        skip.insert("non-thinking-model".to_owned());
        assert!(maybe_inject_include_thoughts(&body, &skip).is_none());
    }

    #[test]
    fn endpoint_selection_matches_onboard_rules() {
        assert_eq!(
            endpoint_from_load_code_assist(&json!({})),
            DAILY_CCPA_ENDPOINT
        );
        assert_eq!(
            endpoint_from_load_code_assist(&json!({"paidTier": {"usesGcpTos": false}})),
            DAILY_CCPA_ENDPOINT
        );
        assert_eq!(
            endpoint_from_load_code_assist(&json!({"paidTier": {"usesGcpTos": true}})),
            PROD_CCPA_ENDPOINT
        );
    }

    #[tokio::test]
    async fn proxy_injects_and_streams_sse_and_retries_on_400() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let seen_bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = seen_bodies.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = upstream_listener.accept().await else {
                    break;
                };
                let seen = seen_clone.clone();
                tokio::spawn(async move {
                    let Some(req) = read_http_request(&mut sock).await.unwrap() else {
                        return;
                    };
                    let val: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                    let model = val
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let has_thoughts = val
                        .pointer("/request/generationConfig/thinkingConfig/includeThoughts")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    seen.lock().await.push(val);

                    if model == "rejects-thoughts" && has_thoughts {
                        let err = r#"{"error":{"message":"include_thoughts is not supported for this model"}}"#;
                        let resp = format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            err.len(),
                            err
                        );
                        sock.write_all(resp.as_bytes()).await.unwrap();
                    } else {
                        let sse = "data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"thought\":true,\"text\":\"plan\"}]}}]}}\n\ndata: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"answer\"}]}}]}}\n\n";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            sse.len(),
                            sse
                        );
                        sock.write_all(resp.as_bytes()).await.unwrap();
                    }
                });
            }
        });

        let proxy = ThoughtsProxy::start_with_upstream(Some(format!("http://{upstream_addr}")))
            .await
            .unwrap();
        let client = reqwest::Client::new();

        // 1. Normal thinking model receives includeThoughts=true and streams SSE back.
        let r1 = client
            .post(format!("{}/v1internal:streamGenerateContent?alt=sse", proxy.base_url()))
            .json(&json!({
                "project": "proj",
                "model": "gemini-4-neon-high",
                "request": {"generationConfig": {"maxOutputTokens": 65535}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r1.status(), 200);
        let text1 = r1.text().await.unwrap();
        assert!(text1.contains("\"thought\":true"));
        assert!(text1.contains("\"text\":\"answer\""));

        // 2. Model rejecting includeThoughts triggers transparent 400 retry without it,
        //    and subsequent requests skip injection immediately.
        let r2 = client
            .post(format!("{}/v1internal:streamGenerateContent?alt=sse", proxy.base_url()))
            .json(&json!({
                "project": "proj",
                "model": "rejects-thoughts",
                "request": {"generationConfig": {"maxOutputTokens": 1024}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r2.status(), 200);

        let r3 = client
            .post(format!("{}/v1internal:streamGenerateContent?alt=sse", proxy.base_url()))
            .json(&json!({
                "project": "proj",
                "model": "rejects-thoughts",
                "request": {"generationConfig": {"maxOutputTokens": 1024}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r3.status(), 200);

        let seen = seen_bodies.lock().await;
        assert_eq!(seen.len(), 4);
        assert_eq!(
            seen[0].pointer("/request/generationConfig/thinkingConfig/includeThoughts"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            seen[1].pointer("/request/generationConfig/thinkingConfig/includeThoughts"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            seen[2].pointer("/request/generationConfig/thinkingConfig/includeThoughts"),
            None
        );
        // Cached skip: 3rd call didn't even try injecting.
        assert_eq!(
            seen[3].pointer("/request/generationConfig/thinkingConfig/includeThoughts"),
            None
        );
    }
}
