//! Per-provider knowledge: upstream host, credential headers, and how to read
//! usage limits out of a response.

use chrono::{DateTime, TimeZone, Utc};
use http::{HeaderMap, HeaderValue, StatusCode};

use crate::sse::SseEvent;
use crate::types::{RateLimitSnapshot, RateLimitWindow};

/// The subscription backends the proxy can front. Route paths map onto the
/// provider's host root: `/r/<token>/backend-api/codex/responses` goes to
/// `https://chatgpt.com/backend-api/codex/responses`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    /// Claude subscription (OAuth bearer, `sk-ant-oat...`) on
    /// `api.anthropic.com`. Used by Pi's `anthropic` provider. Claude Code
    /// itself is not routed here: a non-Anthropic `ANTHROPIC_BASE_URL` puts it
    /// in a reduced "custom endpoint" mode.
    Anthropic,
    /// ChatGPT subscription on `chatgpt.com/backend-api` (Codex, OpenCode's
    /// and Pi's ChatGPT providers): `Authorization` + `ChatGPT-Account-ID`.
    ChatGpt,
    /// Grok Build on `cli-chat-proxy.grok.com` (`GROK_CLI_CHAT_PROXY_BASE_URL`).
    Grok,
    /// Muse Code on `api.meta.ai` (`endpoint_transport.base_url`). The host
    /// and error shapes come from CLIProxyAPI's Meta executor and have not
    /// been checked against the Muse CLI.
    Muse,
}

/// How an upstream response bears on the account that served it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Classification {
    /// Anything that is not an error.
    Success,
    /// The account's quota is used up; another account may serve the request.
    LimitReached {
        resets_at: Option<DateTime<Utc>>,
        window: Option<String>,
    },
    /// The credential was refused.
    Rejected,
    /// Overload or short-lived throttling: not the account's fault, so it is
    /// passed through without burning the account.
    Transient,
    /// The request carried reasoning (thinking signatures, encrypted
    /// content) the account will not accept, typically produced by another
    /// account. Passed through, never retried.
    IncompatibleHistory,
    /// Any other error (bad request, server error...): passed through.
    Other,
}

/// What one SSE event of a successful stream means for failover.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StreamSignal {
    /// Bookkeeping that carries no model output (`message_start`, `ping`,
    /// `response.created`...). May be held back while waiting to see whether
    /// the stream fails.
    Lifecycle,
    /// Model output: from here on the stream is committed downstream.
    Content,
    /// An in-band error event.
    Error(Classification),
}

/// Retry-After at or above this is treated as a quota window rather than
/// short-lived throttling when the provider gives no better signal.
const LONG_RETRY_AFTER_SECS: u64 = 10 * 60;

/// Inbound headers that may carry a credential for any provider. Always
/// stripped before forwarding.
pub(crate) const GENERIC_CREDENTIAL_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "api-key",
    "x-goog-api-key",
    "cookie",
];

const ANTHROPIC_OAUTH_BETA: &str = "oauth-2025-04-20";

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::ChatGpt => "chatgpt",
            Provider::Grok => "grok",
            Provider::Muse => "muse",
        }
    }

    /// Scheme + host the proxy forwards to. Route paths are appended as-is,
    /// so a CLI pointed at `<route base>/v1` reaches `<upstream>/v1`.
    pub fn upstream_base(self) -> &'static str {
        match self {
            Provider::Anthropic => "https://api.anthropic.com",
            Provider::ChatGpt => "https://chatgpt.com",
            Provider::Grok => "https://cli-chat-proxy.grok.com",
            Provider::Muse => "https://api.meta.ai",
        }
    }

    /// Hosts a forwarded URL may resolve to. The proxy never forwards anywhere
    /// else (it is not an open proxy).
    pub(crate) fn allowed_hosts(self) -> &'static [&'static str] {
        match self {
            Provider::Anthropic => &["api.anthropic.com"],
            Provider::ChatGpt => &["chatgpt.com"],
            Provider::Grok => &["cli-chat-proxy.grok.com"],
            Provider::Muse => &["api.meta.ai"],
        }
    }

    /// Provider-specific identity headers stripped from inbound requests (on
    /// top of [`GENERIC_CREDENTIAL_HEADERS`]); the lease sets its own.
    pub(crate) fn credential_headers(self) -> &'static [&'static str] {
        match self {
            Provider::Anthropic | Provider::Grok | Provider::Muse => &[],
            Provider::ChatGpt => &["chatgpt-account-id"],
        }
    }

    /// Last-step header fixups after the lease headers are applied.
    pub(crate) fn finalize_headers(self, headers: &mut HeaderMap) {
        match self {
            Provider::Anthropic => {
                // An OAuth bearer is only accepted with the oauth beta flag.
                // A CLI that held a placeholder API key would not send it.
                let bearer = headers
                    .get(http::header::AUTHORIZATION)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|v| v.starts_with("Bearer "));
                if !bearer || headers.contains_key("x-api-key") {
                    return;
                }
                let current: Vec<String> = headers
                    .get_all("anthropic-beta")
                    .iter()
                    .filter_map(|v| v.to_str().ok())
                    .flat_map(|v| v.split(','))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if current.iter().any(|b| b == ANTHROPIC_OAUTH_BETA) {
                    return;
                }
                let mut joined = current;
                joined.push(ANTHROPIC_OAUTH_BETA.to_string());
                if let Ok(value) = HeaderValue::from_str(&joined.join(",")) {
                    headers.insert("anthropic-beta", value);
                }
            }
            Provider::ChatGpt | Provider::Grok | Provider::Muse => {}
        }
    }

    /// Whether a (normalized) upstream path is one the proxied CLIs use. A
    /// route is visible to the agent's own tools (it rides the agent
    /// process's environment or config), so it only reaches the inference
    /// endpoints, never the rest of the account's API.
    pub(crate) fn allows_path(self, path: &str) -> bool {
        // None of the CLIs' paths carry escapes, backslashes or matrix
        // parameters; refusing them keeps an upstream that decodes `%2f` or
        // `%2e` differently from walking out of the allowed prefix.
        if path.contains(['%', '\\', ';']) {
            return false;
        }
        let under = |prefix: &str| {
            path == prefix
                || path
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        match self {
            // Codex, OpenCode and Pi: `/responses`, `/responses/compact`,
            // `/models` under the Codex backend.
            Provider::ChatGpt => under("/backend-api/codex"),
            // Pi: messages (and token counting), model listing.
            Provider::Anthropic => under("/v1/messages") || under("/v1/models"),
            // Grok: its host serves only the CLI, and its auxiliary calls
            // (settings, login config, feedback) share the inference base.
            Provider::Grok => path.starts_with("/v1/"),
            // Muse: chat completions and model listing.
            Provider::Muse => under("/v1/chat/completions") || under("/v1/models"),
        }
    }

    /// The provider's own "usage limit reached" answer (status, headers,
    /// body), for a request Zeron refuses because the account it must go to
    /// is out of usage: the CLI shows and handles it like the provider's.
    pub(crate) fn usage_limit_response(
        self,
        resets_at: Option<DateTime<Utc>>,
        window: Option<&str>,
    ) -> (StatusCode, Vec<(&'static str, String)>, String) {
        let reset_secs = resets_at.map(|at| at.timestamp());
        let reset_in = resets_at.map(|at| (at - Utc::now()).num_seconds().max(0));
        let json = ("content-type", "application/json".to_string());
        let (headers, body) = match self {
            Provider::ChatGpt => {
                let mut error = serde_json::json!({
                    "type": "usage_limit_reached",
                    "message": "The usage limit has been reached",
                });
                if let (Some(at), Some(secs)) = (reset_secs, reset_in) {
                    error["resets_at"] = at.into();
                    error["resets_in_seconds"] = secs.into();
                }
                (vec![json], serde_json::json!({ "error": error }))
            }
            Provider::Anthropic => {
                let mut headers = vec![
                    json,
                    ("anthropic-ratelimit-unified-status", "rejected".to_string()),
                    (
                        "anthropic-ratelimit-unified-representative-claim",
                        window.unwrap_or("five_hour").to_string(),
                    ),
                ];
                if let (Some(at), Some(secs)) = (reset_secs, reset_in) {
                    headers.push(("anthropic-ratelimit-unified-reset", at.to_string()));
                    headers.push(("retry-after", secs.to_string()));
                }
                let body = serde_json::json!({
                    "type": "error",
                    "error": {
                        "type": "rate_limit_error",
                        "message": "This request would exceed your account's rate limit. \
                                    Please try again later.",
                    },
                });
                (headers, body)
            }
            Provider::Grok => (
                vec![json],
                serde_json::json!({
                    "code": "rate_limited",
                    "error": "Plan limit reached, try again later",
                }),
            ),
            Provider::Muse => {
                let mut error = serde_json::json!({
                    "code": "usage_limited",
                    "message": "Usage limit reached",
                });
                if let Some(at) = reset_secs {
                    error["resets_at"] = at.into();
                }
                (vec![json], serde_json::json!({ "error": error }))
            }
        };
        (StatusCode::TOO_MANY_REQUESTS, headers, body.to_string())
    }

    /// Whether the body of an error response with this status should be read
    /// (bounded) before classifying it.
    pub(crate) fn sniff_body(self, status: StatusCode) -> bool {
        status.is_client_error() || status.is_server_error()
    }

    pub(crate) fn rate_limits(self, headers: &HeaderMap) -> RateLimitSnapshot {
        match self {
            Provider::Anthropic => anthropic_rate_limits(headers),
            Provider::ChatGpt => codex_rate_limits(headers),
            Provider::Grok | Provider::Muse => RateLimitSnapshot::default(),
        }
    }

    /// Classifies one event of a successful SSE stream.
    pub(crate) fn stream_signal(self, event: &SseEvent, headers: &HeaderMap) -> StreamSignal {
        match self {
            Provider::Anthropic => anthropic_stream_signal(event, headers),
            Provider::ChatGpt => codex_stream_signal(event, headers),
            Provider::Grok => grok_stream_signal(event, headers),
            Provider::Muse => muse_stream_signal(event, headers),
        }
    }

    pub(crate) fn classify(
        self,
        status: StatusCode,
        headers: &HeaderMap,
        body: &[u8],
    ) -> Classification {
        if status.is_informational() || status.is_success() || status.is_redirection() {
            return Classification::Success;
        }
        match self {
            Provider::Anthropic => classify_anthropic(status, headers, body),
            Provider::ChatGpt => classify_codex(status, headers, body),
            Provider::Grok => classify_grok(status, headers, body),
            Provider::Muse => classify_muse(status, headers, body),
        }
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
}

fn epoch_secs(value: &str) -> Option<DateTime<Utc>> {
    let secs: f64 = value.trim().parse().ok()?;
    if !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    Utc.timestamp_opt(secs as i64, 0).single()
}

/// `Retry-After` as seconds (delta-seconds or HTTP date).
fn retry_after_secs(headers: &HeaderMap) -> Option<u64> {
    let value = header_str(headers, "retry-after")?;
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs);
    }
    let at = DateTime::parse_from_rfc2822(value).ok()?;
    Some((at.with_timezone(&Utc) - Utc::now()).num_seconds().max(0) as u64)
}

fn json_body(body: &[u8]) -> Option<serde_json::Value> {
    if body.is_empty() {
        return None;
    }
    serde_json::from_slice(body).ok()
}

/// Generic fallback shared by every provider.
fn classify_generic(status: StatusCode, headers: &HeaderMap) -> Classification {
    // A Cloudflare challenge is about this machine's connection, not the account.
    if headers.contains_key("cf-mitigated") {
        return Classification::Transient;
    }
    match status.as_u16() {
        // Only a 401 says the credential is bad. A 403 without a known
        // credential error in its body (an HTML block page, a permission
        // on one resource) says nothing about the account.
        401 => Classification::Rejected,
        429 => match retry_after_secs(headers) {
            Some(secs) if secs >= LONG_RETRY_AFTER_SECS => Classification::LimitReached {
                resets_at: Some(Utc::now() + chrono::Duration::seconds(secs as i64)),
                window: None,
            },
            _ => Classification::Transient,
        },
        500..=599 => Classification::Transient,
        _ => Classification::Other,
    }
}

// --- Anthropic -------------------------------------------------------------

const ANTHROPIC_UNIFIED: &str = "anthropic-ratelimit-unified-";

fn anthropic_rate_limits(headers: &HeaderMap) -> RateLimitSnapshot {
    let mut snapshot = RateLimitSnapshot::default();
    for (name, value) in headers {
        let name = name.as_str();
        if name.starts_with("anthropic-ratelimit-")
            && let Ok(value) = value.to_str()
        {
            snapshot.headers.push((name.to_string(), value.to_string()));
        }
    }
    snapshot.status = header_str(headers, "anthropic-ratelimit-unified-status").map(String::from);
    for (key, name) in [("5h", "five_hour"), ("7d", "seven_day")] {
        let get = |suffix: &str| header_str(headers, &format!("{ANTHROPIC_UNIFIED}{key}-{suffix}"));
        let used = get("utilization")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|f| f * 100.0);
        let resets_at = get("reset").and_then(epoch_secs);
        let status = get("status").map(String::from);
        if used.is_none() && resets_at.is_none() && status.is_none() {
            continue;
        }
        snapshot.windows.push(RateLimitWindow {
            name: name.to_string(),
            used_percent: used,
            window_minutes: Some(if key == "5h" { 5 * 60 } else { 7 * 24 * 60 }),
            resets_at,
            status,
        });
    }
    snapshot
}

/// The subscription limiter refused the request for the account: a 5h or 7d
/// window (or the unified status) is `rejected` and paid overage does not
/// cover it. Overage-only rejections are not the account's quota.
fn anthropic_quota(headers: &HeaderMap) -> Option<Classification> {
    let overage = header_str(headers, "anthropic-ratelimit-unified-overage-status");
    if matches!(overage, Some("allowed" | "allowed_warning")) {
        return None;
    }
    let claim = header_str(headers, "anthropic-ratelimit-unified-representative-claim")
        .filter(|c| !c.contains("overage"))
        .map(String::from);
    let unified_reset =
        header_str(headers, "anthropic-ratelimit-unified-reset").and_then(epoch_secs);
    let rejected_window = [("5h", "five_hour"), ("7d", "seven_day")]
        .into_iter()
        .filter(|(key, _)| {
            header_str(headers, &format!("{ANTHROPIC_UNIFIED}{key}-status")) == Some("rejected")
        })
        .map(|(key, name)| {
            let reset = header_str(headers, &format!("{ANTHROPIC_UNIFIED}{key}-reset"))
                .and_then(epoch_secs);
            (name, reset)
        })
        // The latest reset is when the account is usable again.
        .max_by_key(|(_, reset)| *reset);
    match rejected_window {
        Some((name, reset)) => Some(Classification::LimitReached {
            resets_at: reset.or(unified_reset),
            window: claim.or_else(|| Some(name.to_string())),
        }),
        None => {
            let unified = header_str(headers, "anthropic-ratelimit-unified-status");
            let overage_claim =
                header_str(headers, "anthropic-ratelimit-unified-representative-claim")
                    .is_some_and(|c| c.contains("overage"));
            (unified == Some("rejected") && !overage_claim).then_some(
                Classification::LimitReached {
                    resets_at: unified_reset,
                    window: claim,
                },
            )
        }
    }
}

fn mentions_signature(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("signature")
}

/// An Anthropic `{"type":"error","error":{...}}` error object.
fn classify_anthropic_error(
    status: u16,
    error: Option<&serde_json::Value>,
    headers: &HeaderMap,
) -> Option<Classification> {
    let error_type = error.and_then(|e| e.get("type")).and_then(|t| t.as_str());
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    if headers.contains_key("cf-mitigated") {
        return None;
    }
    Some(match (status, error_type) {
        (529, _) | (_, Some("overloaded_error")) => Classification::Transient,
        (400, _) | (_, Some("invalid_request_error")) if mentions_signature(message) => {
            Classification::IncompatibleHistory
        }
        (401, _) | (_, Some("authentication_error")) => Classification::Rejected,
        _ => return None,
    })
}

fn classify_anthropic(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Classification {
    if status == StatusCode::TOO_MANY_REQUESTS
        && let Some(limit) = anthropic_quota(headers)
    {
        return limit;
    }
    let json = json_body(body);
    classify_anthropic_error(
        status.as_u16(),
        json.as_ref().and_then(|v| v.get("error")),
        headers,
    )
    .unwrap_or_else(|| classify_generic(status, headers))
}

fn anthropic_stream_signal(event: &SseEvent, headers: &HeaderMap) -> StreamSignal {
    match event.kind().as_deref() {
        Some("message_start" | "ping") => StreamSignal::Lifecycle,
        Some("error") => {
            let json = event.json();
            let error = json.as_ref().and_then(|v| v.get("error"));
            let error_type = error.and_then(|e| e.get("type")).and_then(|t| t.as_str());
            if error_type == Some("rate_limit_error") {
                // In-band rate limit: the account's quota only when the
                // stream's unified headers say its window is rejected;
                // otherwise short-lived throttling.
                return StreamSignal::Error(
                    anthropic_quota(headers).unwrap_or(Classification::Transient),
                );
            }
            StreamSignal::Error(
                classify_anthropic_error(200, error, headers).unwrap_or(Classification::Other),
            )
        }
        _ => StreamSignal::Content,
    }
}

// --- Codex (ChatGPT backend) -----------------------------------------------

fn codex_rate_limits(headers: &HeaderMap) -> RateLimitSnapshot {
    let mut snapshot = RateLimitSnapshot::default();
    for (name, value) in headers {
        let name = name.as_str();
        if name.starts_with("x-codex-")
            && let Ok(value) = value.to_str()
        {
            snapshot.headers.push((name.to_string(), value.to_string()));
        }
    }
    for which in ["primary", "secondary"] {
        let get = |suffix: &str| header_str(headers, &format!("x-codex-{which}-{suffix}"));
        let used = get("used-percent").and_then(|v| v.parse::<f64>().ok());
        let window_minutes = get("window-minutes").and_then(|v| v.parse::<u64>().ok());
        let resets_at = get("reset-at").and_then(epoch_secs).or_else(|| {
            get("reset-after-seconds")
                .and_then(|v| v.parse::<i64>().ok())
                .map(|s| Utc::now() + chrono::Duration::seconds(s))
        });
        if used.is_none() && window_minutes.is_none() && resets_at.is_none() {
            continue;
        }
        snapshot.windows.push(RateLimitWindow {
            name: which.to_string(),
            used_percent: used,
            window_minutes,
            resets_at,
            status: None,
        });
    }
    snapshot
}

/// An OpenAI-style error object (`{"type"|"code", "message", ...}`).
fn classify_codex_error(error: &serde_json::Value, headers: &HeaderMap) -> Option<Classification> {
    let field = |k: &str| error.get(k).and_then(|t| t.as_str());
    let kind = field("type");
    let code = field("code");
    let message = field("message").unwrap_or("").to_ascii_lowercase();
    let is = |name: &str| kind == Some(name) || code == Some(name);
    if is("usage_limit_reached") {
        let resets_at = error
            .get("resets_at")
            .and_then(|v| v.as_f64())
            .and_then(|s| epoch_secs(&s.to_string()))
            .or_else(|| {
                error
                    .get("resets_in_seconds")
                    .and_then(|v| v.as_i64())
                    .map(|s| Utc::now() + chrono::Duration::seconds(s))
            });
        let window = codex_exhausted_window(headers);
        return Some(Classification::LimitReached { resets_at, window });
    }
    // The plan does not include Codex: this account cannot serve, but its
    // login is fine. No reset time.
    if is("usage_not_included") {
        return Some(Classification::LimitReached {
            resets_at: None,
            window: Some("usage_not_included".to_string()),
        });
    }
    if is("invalid_encrypted_content")
        || message.contains("encrypted_content")
        || message.contains("encrypted content")
    {
        return Some(Classification::IncompatibleHistory);
    }
    None
}

fn codex_error_object(json: &serde_json::Value) -> Option<&serde_json::Value> {
    json.get("error")
        .filter(|e| e.is_object())
        .or_else(|| json.pointer("/response/error").filter(|e| e.is_object()))
}

fn classify_codex(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Classification {
    json_body(body)
        .as_ref()
        .and_then(codex_error_object)
        .and_then(|error| classify_codex_error(error, headers))
        .unwrap_or_else(|| classify_generic(status, headers))
}

fn codex_stream_signal(event: &SseEvent, headers: &HeaderMap) -> StreamSignal {
    match event.kind().as_deref() {
        Some(
            "response.created" | "response.in_progress" | "response.queued" | "codex.rate_limits",
        ) => StreamSignal::Lifecycle,
        Some("error" | "response.failed") => {
            let json = event.json();
            let class = json
                .as_ref()
                .and_then(|j| codex_error_object(j).or(Some(j)))
                .and_then(|error| classify_codex_error(error, headers));
            StreamSignal::Error(class.unwrap_or(Classification::Other))
        }
        _ => StreamSignal::Content,
    }
}

/// The window whose usage hit 100%, from the rate-limit headers on the 429.
fn codex_exhausted_window(headers: &HeaderMap) -> Option<String> {
    ["primary", "secondary"].into_iter().find_map(|which| {
        let used = header_str(headers, &format!("x-codex-{which}-used-percent"))?
            .parse::<f64>()
            .ok()?;
        (used >= 100.0).then(|| which.to_string())
    })
}

// --- Grok Build -------------------------------------------------------------

/// Grok's included usage is a rolling 24-hour window with no reset time in
/// the error (CLIProxyAPI uses the same figure).
const GROK_FREE_USAGE_WINDOW_HOURS: i64 = 24;

fn classify_grok_error(
    status: u16,
    json: Option<&serde_json::Value>,
    raw: &[u8],
) -> Option<Classification> {
    let field = |k: &str| {
        json.and_then(|j| j.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    };
    let code = field("code");
    let message = match field("error") {
        m if m.is_empty() => String::from_utf8_lossy(raw).to_ascii_lowercase(),
        m => m,
    };
    if status == 429
        && (code.contains("free-usage-exhausted")
            || message.contains("free-usage-exhausted")
            || message.contains("included free usage"))
    {
        return Some(Classification::LimitReached {
            resets_at: Some(Utc::now() + chrono::Duration::hours(GROK_FREE_USAGE_WINDOW_HOURS)),
            window: Some("free_usage".to_string()),
        });
    }
    // Paid plans: Grok's own error display expects "Plan limit reached, try
    // again later" on a 429. No reset time is given.
    if status == 429 && message.contains("plan limit") {
        return Some(Classification::LimitReached {
            resets_at: None,
            window: Some("plan".to_string()),
        });
    }
    // An invalidated OAuth access token comes back as 403 bad-credentials.
    if code.contains("bad-credentials") || message.contains("bad-credentials") {
        return Some(Classification::Rejected);
    }
    None
}

fn classify_grok(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Classification {
    classify_grok_error(status.as_u16(), json_body(body).as_ref(), body)
        .unwrap_or_else(|| classify_generic(status, headers))
}

/// Grok streams OpenAI Responses-style events.
fn grok_stream_signal(event: &SseEvent, headers: &HeaderMap) -> StreamSignal {
    match codex_stream_signal(event, headers) {
        StreamSignal::Error(Classification::Other) => {
            let json = event.json();
            let error = json
                .as_ref()
                .and_then(|j| codex_error_object(j).or(Some(j)));
            StreamSignal::Error(
                classify_grok_error(429, error, event.data.as_bytes())
                    .unwrap_or(Classification::Other),
            )
        }
        signal => signal,
    }
}

// --- Muse Code ---------------------------------------------------------------

/// `{"error":{"code","message","resets_at"}}`.
fn classify_muse_error(status: u16, error: &serde_json::Value) -> Option<Classification> {
    let field = |k: &str| {
        error
            .get(k)
            .or_else(|| if k == "code" { error.get("type") } else { None })
            .and_then(|v| v.as_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default()
    };
    let code = field("code");
    let message = field("message");
    let resets_at = error
        .get("resets_at")
        .and_then(|v| v.as_f64())
        .and_then(|s| epoch_secs(&s.to_string()));
    let quota = message.contains("subscription quota")
        || message.contains("quota exhausted")
        || code == "usage_limited"
        || ((code == "rate_limit_exceeded" || code == "rate_limited" || code.contains("quota"))
            && resets_at.is_some());
    if (status == 429 || status == 200) && quota {
        return Some(Classification::LimitReached {
            resets_at,
            window: (!code.is_empty()).then_some(code),
        });
    }
    None
}

fn classify_muse(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> Classification {
    json_body(body)
        .as_ref()
        .and_then(|j| j.get("error"))
        .filter(|e| e.is_object())
        .and_then(|error| classify_muse_error(status.as_u16(), error))
        .unwrap_or_else(|| classify_generic(status, headers))
}

/// Chat-completions chunks; an error arrives as a chunk with an `error`
/// object.
fn muse_stream_signal(event: &SseEvent, _headers: &HeaderMap) -> StreamSignal {
    let Some(json) = event.json() else {
        return StreamSignal::Content;
    };
    match json.get("error").filter(|e| e.is_object()) {
        Some(error) => {
            StreamSignal::Error(classify_muse_error(429, error).unwrap_or(Classification::Other))
        }
        None => StreamSignal::Content,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (n, v) in pairs {
            map.insert(*n, HeaderValue::from_str(v).unwrap());
        }
        map
    }

    #[test]
    fn anthropic_unified_rejection_is_limit() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-reset", "1900000000"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "five_hour",
            ),
        ]);
        let c = Provider::Anthropic.classify(StatusCode::TOO_MANY_REQUESTS, &h, b"");
        assert_eq!(
            c,
            Classification::LimitReached {
                resets_at: Utc.timestamp_opt(1_900_000_000, 0).single(),
                window: Some("five_hour".into()),
            }
        );
    }

    #[test]
    fn anthropic_overage_covering_is_not_limit() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-overage-status", "allowed"),
            ("retry-after", "5"),
        ]);
        let c = Provider::Anthropic.classify(StatusCode::TOO_MANY_REQUESTS, &h, b"");
        assert_eq!(c, Classification::Transient);
    }

    #[test]
    fn anthropic_transient() {
        let body =
            br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        let c = Provider::Anthropic.classify(
            StatusCode::from_u16(529).unwrap(),
            &HeaderMap::new(),
            body,
        );
        assert_eq!(c, Classification::Transient);
        let h = headers(&[("retry-after", "20")]);
        let body = br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
        let c = Provider::Anthropic.classify(StatusCode::TOO_MANY_REQUESTS, &h, body);
        assert_eq!(c, Classification::Transient);
    }

    #[test]
    fn anthropic_long_retry_after_is_limit() {
        let h = headers(&[("retry-after", "7200")]);
        let c = Provider::Anthropic.classify(StatusCode::TOO_MANY_REQUESTS, &h, b"");
        assert!(matches!(c, Classification::LimitReached { .. }));
    }

    #[test]
    fn anthropic_rejected() {
        let body = br#"{"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired"}}"#;
        let c = Provider::Anthropic.classify(StatusCode::UNAUTHORIZED, &HeaderMap::new(), body);
        assert_eq!(c, Classification::Rejected);
    }

    #[test]
    fn anthropic_snapshot() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-status", "allowed"),
            ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
            ("anthropic-ratelimit-unified-5h-reset", "1900000000"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.1"),
        ]);
        let s = Provider::Anthropic.rate_limits(&h);
        assert_eq!(s.status.as_deref(), Some("allowed"));
        assert_eq!(s.windows.len(), 2);
        assert_eq!(s.windows[0].name, "five_hour");
        assert!((s.windows[0].used_percent.unwrap() - 42.0).abs() < 1e-9);
        assert_eq!(s.headers.len(), 4);
    }

    #[test]
    fn codex_usage_limit() {
        let body = br#"{"error":{"type":"usage_limit_reached","message":"The usage limit has been reached","plan_type":"plus","resets_at":1900000000}}"#;
        let h = headers(&[("x-codex-primary-used-percent", "100.0")]);
        let c = Provider::ChatGpt.classify(StatusCode::TOO_MANY_REQUESTS, &h, body);
        assert_eq!(
            c,
            Classification::LimitReached {
                resets_at: Utc.timestamp_opt(1_900_000_000, 0).single(),
                window: Some("primary".into()),
            }
        );
    }

    #[test]
    fn codex_transient_429() {
        let body = br#"{"error":{"type":"rate_limit_exceeded","message":"slow down"}}"#;
        let c = Provider::ChatGpt.classify(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), body);
        assert_eq!(c, Classification::Transient);
    }

    #[test]
    fn anthropic_weekly_window_rejection() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-5h-status", "allowed"),
            ("anthropic-ratelimit-unified-7d-status", "rejected"),
            ("anthropic-ratelimit-unified-7d-reset", "1900000500"),
            ("anthropic-ratelimit-unified-reset", "1900000000"),
        ]);
        let c = Provider::Anthropic.classify(StatusCode::TOO_MANY_REQUESTS, &h, b"");
        assert_eq!(
            c,
            Classification::LimitReached {
                resets_at: Utc.timestamp_opt(1_900_000_500, 0).single(),
                window: Some("seven_day".into()),
            }
        );
    }

    #[test]
    fn anthropic_overage_only_rejection_is_not_limit() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "overage",
            ),
            ("retry-after", "3"),
        ]);
        let c = Provider::Anthropic.classify(StatusCode::TOO_MANY_REQUESTS, &h, b"");
        assert_eq!(c, Classification::Transient);
    }

    #[test]
    fn anthropic_foreign_signature() {
        let body = br#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.1.content.0: Invalid `signature` in `thinking` block"}}"#;
        let c = Provider::Anthropic.classify(StatusCode::BAD_REQUEST, &HeaderMap::new(), body);
        assert_eq!(c, Classification::IncompatibleHistory);
    }

    #[test]
    fn codex_foreign_encrypted_content() {
        let body = br#"{"error":{"type":"invalid_request_error","code":"invalid_encrypted_content","message":"The encrypted content could not be verified."}}"#;
        let c = Provider::ChatGpt.classify(StatusCode::BAD_REQUEST, &HeaderMap::new(), body);
        assert_eq!(c, Classification::IncompatibleHistory);
    }

    #[test]
    fn cloudflare_challenge_is_transient() {
        let h = headers(&[("cf-mitigated", "challenge")]);
        let c = Provider::Anthropic.classify(
            StatusCode::FORBIDDEN,
            &h,
            b"<html>Just a moment...</html>",
        );
        assert_eq!(c, Classification::Transient);
    }

    #[test]
    fn stream_signals() {
        let ev = |event: Option<&str>, data: &str| SseEvent {
            event: event.map(String::from),
            data: data.to_string(),
        };
        let h = HeaderMap::new();
        let a = Provider::Anthropic;
        assert_eq!(
            a.stream_signal(&ev(Some("message_start"), "{}"), &h),
            StreamSignal::Lifecycle
        );
        assert_eq!(
            a.stream_signal(&ev(Some("content_block_start"), "{}"), &h),
            StreamSignal::Content
        );
        let overloaded =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(
            a.stream_signal(&ev(Some("error"), overloaded), &h),
            StreamSignal::Error(Classification::Transient)
        );
        let limited = r#"{"type":"error","error":{"type":"rate_limit_error","message":"limit"}}"#;
        // Without a rejected window it is throttling...
        assert_eq!(
            a.stream_signal(&ev(Some("error"), limited), &h),
            StreamSignal::Error(Classification::Transient)
        );
        // ...with one, the account's quota.
        let rejected = headers(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-5h-status", "rejected"),
            ("anthropic-ratelimit-unified-5h-reset", "1900000000"),
        ]);
        assert!(matches!(
            a.stream_signal(&ev(Some("error"), limited), &rejected),
            StreamSignal::Error(Classification::LimitReached { .. })
        ));
        let c = Provider::ChatGpt;
        assert_eq!(
            c.stream_signal(&ev(None, r#"{"type":"response.created"}"#), &h),
            StreamSignal::Lifecycle
        );
        let failed = r#"{"type":"response.failed","response":{"error":{"code":"usage_limit_reached","message":"limit","resets_in_seconds":60}}}"#;
        assert!(matches!(
            c.stream_signal(&ev(Some("response.failed"), failed), &h),
            StreamSignal::Error(Classification::LimitReached { .. })
        ));
        assert_eq!(
            c.stream_signal(&ev(Some("response.output_text.delta"), "{}"), &h),
            StreamSignal::Content
        );
    }

    #[test]
    fn grok_classification() {
        let body = br#"{"code":"subscription:free-usage-exhausted","error":"You have used your included free usage."}"#;
        let c = Provider::Grok.classify(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), body);
        assert!(matches!(
            c,
            Classification::LimitReached {
                resets_at: Some(_),
                ..
            }
        ));
        let body = br#"{"code":"unauthenticated:bad-credentials","error":"The OAuth2 access token could not be validated."}"#;
        let c = Provider::Grok.classify(StatusCode::FORBIDDEN, &HeaderMap::new(), body);
        assert_eq!(c, Classification::Rejected);
        let c = Provider::Grok.classify(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), b"{}");
        assert_eq!(c, Classification::Transient);
        let body = br#"{"code":"rate_limited","error":"Plan limit reached, try again later"}"#;
        let c = Provider::Grok.classify(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), body);
        assert!(matches!(c, Classification::LimitReached { .. }));
    }

    #[test]
    fn muse_classification() {
        let body = br#"{"error":{"code":"rate_limit_exceeded","message":"Subscription quota exhausted","resets_at":1900000000}}"#;
        let c = Provider::Muse.classify(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), body);
        assert_eq!(
            c,
            Classification::LimitReached {
                resets_at: Utc.timestamp_opt(1_900_000_000, 0).single(),
                window: Some("rate_limit_exceeded".into()),
            }
        );
        let body = br#"{"error":{"code":"rate_limit_exceeded","message":"slow down"}}"#;
        let c = Provider::Muse.classify(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), body);
        assert_eq!(c, Classification::Transient);
    }

    #[test]
    fn only_401_or_known_credential_errors_are_rejections() {
        for provider in [
            Provider::Anthropic,
            Provider::ChatGpt,
            Provider::Grok,
            Provider::Muse,
        ] {
            let c = provider.classify(StatusCode::UNAUTHORIZED, &HeaderMap::new(), b"");
            assert_eq!(c, Classification::Rejected, "{provider:?}");
            let html = b"<html><body>Access denied</body></html>";
            let c = provider.classify(StatusCode::FORBIDDEN, &HeaderMap::new(), html);
            assert_eq!(c, Classification::Other, "{provider:?}");
        }
        let permission = br#"{"type":"error","error":{"type":"permission_error","message":"not allowed for this model"}}"#;
        let c = Provider::Anthropic.classify(StatusCode::FORBIDDEN, &HeaderMap::new(), permission);
        assert_eq!(c, Classification::Other);
        let auth = br#"{"type":"error","error":{"type":"authentication_error","message":"OAuth token revoked"}}"#;
        let c = Provider::Anthropic.classify(StatusCode::FORBIDDEN, &HeaderMap::new(), auth);
        assert_eq!(c, Classification::Rejected);
    }

    #[test]
    fn paths_are_limited_to_what_the_clis_call() {
        let c = Provider::ChatGpt;
        assert!(c.allows_path("/backend-api/codex/responses"));
        assert!(c.allows_path("/backend-api/codex/models"));
        assert!(!c.allows_path("/backend-api/codexx"));
        assert!(!c.allows_path("/backend-api/wham/usage"));
        assert!(!c.allows_path("/backend-api/accounts/check"));
        let a = Provider::Anthropic;
        assert!(a.allows_path("/v1/messages"));
        assert!(a.allows_path("/v1/messages/count_tokens"));
        assert!(a.allows_path("/v1/models"));
        assert!(!a.allows_path("/api/oauth/profile"));
        assert!(!a.allows_path("/v1/organizations/x"));
        assert!(Provider::Grok.allows_path("/v1/responses"));
        assert!(!Provider::Grok.allows_path("/admin"));
        assert!(Provider::Muse.allows_path("/v1/chat/completions"));
        assert!(!Provider::Muse.allows_path("/v1/muse-code/key"));
        for escaped in [
            "/backend-api/codex/..%2f..%2fwham/usage",
            "/backend-api/codex/%2e%2e/accounts/check",
            "/backend-api/codex/%2E%2E/accounts/check",
            "/backend-api/codex/..%5c..%5cwham",
            "/backend-api/codex/responses;x=..",
            "/v1/messages/..\\..\\api/oauth/profile",
        ] {
            assert!(!c.allows_path(escaped), "{escaped}");
            assert!(!a.allows_path(escaped), "{escaped}");
        }
    }

    #[test]
    fn native_usage_limit_answers_classify_as_limits() {
        let at = Utc.timestamp_opt(1_900_000_000, 0).single();
        for provider in [
            Provider::Anthropic,
            Provider::ChatGpt,
            Provider::Grok,
            Provider::Muse,
        ] {
            let (status, pairs, body) = provider.usage_limit_response(at, Some("seven_day"));
            let mut map = HeaderMap::new();
            for (n, v) in pairs {
                map.insert(n, HeaderValue::from_str(&v).unwrap());
            }
            let c = provider.classify(status, &map, body.as_bytes());
            assert!(
                matches!(c, Classification::LimitReached { .. }),
                "{provider:?}: {c:?}"
            );
        }
    }

    #[test]
    fn oauth_beta_added_once() {
        let mut h = headers(&[
            ("authorization", "Bearer x"),
            ("anthropic-beta", "claude-code-20250219"),
        ]);
        Provider::Anthropic.finalize_headers(&mut h);
        assert_eq!(h["anthropic-beta"], "claude-code-20250219,oauth-2025-04-20");
        Provider::Anthropic.finalize_headers(&mut h);
        assert_eq!(h["anthropic-beta"], "claude-code-20250219,oauth-2025-04-20");
    }
}
