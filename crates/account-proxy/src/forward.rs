//! Request handling: route lookup, credential swap, failover, streaming.

use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use futures::{StreamExt, TryStreamExt};
use http::header::{CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, HOST, UPGRADE};
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyDataStream, BodyExt, Empty, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper_util::rt::TokioIo;

use crate::provider::{Classification, GENERIC_CREDENTIAL_HEADERS, Provider, StreamSignal};
use crate::server::{Inner, ProxyConfig, RouteEntry};
use crate::sse::{SseEvent, SseScanner};
use crate::types::{
    AccountId, CredentialSource, HistoryRecovery, Lease, LeaseError, Outcome, ProxyEvent,
    RouteInfo, SwitchReason,
};

pub(crate) type BoxError = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type ProxyBody = UnsyncBoxBody<Bytes, BoxError>;

/// Hop-by-hop headers (RFC 9110 7.6.1) plus legacy ones; never forwarded.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Shown with a provider's refusal of reasoning another account produced.
const INCOMPATIBLE_HISTORY_NOTE: &str = "This chat's history holds reasoning from another \
     account (Zeron moves a chat to another account when one reaches its usage limit), \
     and the provider won't accept it on this account. Start a new chat to continue";

/// How many times a request is retried on another account after a 401/403.
const MAX_REJECTED_RETRIES: usize = 1;

pub(crate) async fn handle(inner: Arc<Inner>, req: Request<Incoming>) -> Response<ProxyBody> {
    // Not a forward proxy: no CONNECT, no absolute-form targets.
    if req.method() == Method::CONNECT {
        return status_only(StatusCode::METHOD_NOT_ALLOWED);
    }
    if req.uri().scheme().is_some() || req.uri().authority().is_some() {
        return status_only(StatusCode::BAD_REQUEST);
    }
    // Guards against DNS rebinding: only loopback names reach a route.
    if !host_is_local(req.headers(), inner.addr.port()) {
        return status_only(StatusCode::FORBIDDEN);
    }
    let Some((token, rest)) = split_route_path(req.uri().path()) else {
        return status_only(StatusCode::NOT_FOUND);
    };
    let Some(route) = inner.lookup(token) else {
        return status_only(StatusCode::NOT_FOUND);
    };
    let provider = route.info.provider;
    let (base, allowed_hosts) = inner.upstream_base(provider);
    let mut target = format!("{}{}", base.trim_end_matches('/'), rest);
    if let Some(query) = req.uri().query() {
        target.push('?');
        target.push_str(query);
    }
    let url = match reqwest::Url::parse(&target) {
        Ok(url)
            if url
                .host_str()
                .is_some_and(|h| allowed_hosts.iter().any(|a| a == h)) =>
        {
            url
        }
        _ => return status_only(StatusCode::BAD_REQUEST),
    };
    // Checked on the normalized path: `..` segments are already resolved.
    if !provider.allows_path(url.path()) {
        return status_only(StatusCode::NOT_FOUND);
    }

    if is_upgrade_request(req.headers()) {
        if inner.config.refuse_websocket.contains(&provider) {
            // Codex answers 426 by switching the session to HTTP streaming,
            // where every turn can fail over.
            return status_only(StatusCode::UPGRADE_REQUIRED);
        }
        return handle_upgrade(inner, route, req, url).await;
    }

    let (parts, body) = req.into_parts();
    let mut headers = request_headers(&parts.headers, Some(provider), false);
    let mut own_headers = request_headers(&parts.headers, None, false);
    let mut body = match RequestBody::read(body, &parts.headers, inner.config.max_replay_body).await
    {
        Ok(body) => body,
        Err(err) => {
            tracing::debug!(chat = %route.info.chat_id, %err, "reading request body failed");
            return status_only(StatusCode::BAD_REQUEST);
        }
    };
    if !body.replayable()
        && let Some(length) = parts.headers.get(CONTENT_LENGTH)
    {
        // Streamed once and unmodified: keep the declared length.
        headers.insert(CONTENT_LENGTH, length.clone());
        own_headers.insert(CONTENT_LENGTH, length.clone());
    }
    let json_eligible = body_edits_allowed(&parts.headers);
    let mut attempt = Attempts::new(&inner, &route, &parts.headers);
    loop {
        let lease = match attempt.next_lease().await {
            Next::Lease(lease) => lease,
            Next::Done(response) => return response,
        };
        let replayable = body.replayable();
        let request = inner
            .client
            .request(parts.method.clone(), url.clone())
            .headers(upstream_headers(&headers, &own_headers, &lease, provider))
            .body(body.for_attempt(&lease, json_eligible));
        let upstream = match request.send().await {
            Ok(resp) => resp,
            Err(err) => return attempt.connect_failed(err),
        };
        if let Some(serve) = attempt
            .classify(&lease, Upstream::new(upstream), replayable)
            .await
        {
            return serve.into_downstream();
        }
    }
}

/// `/r/<token>/<rest>` -> (token, "/<rest>").
fn split_route_path(path: &str) -> Option<(&str, &str)> {
    let after = path.strip_prefix("/r/")?;
    match after.find('/') {
        Some(i) => Some((&after[..i], &after[i..])),
        None => Some((after, "/")),
    }
}

fn host_is_local(headers: &HeaderMap, port: u16) -> bool {
    let Some(host) = headers.get(HOST).and_then(|h| h.to_str().ok()) else {
        return false;
    };
    let (name, host_port) = if host.starts_with('[') {
        match host.split_once(']') {
            Some((_, after)) => (&host[..host.len() - after.len()], after.strip_prefix(':')),
            None => return false,
        }
    } else {
        match host.rsplit_once(':') {
            Some((name, p)) => (name, Some(p)),
            None => (host, None),
        }
    };
    let local = matches!(
        name.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "[::1]"
    );
    local && host_port.is_none_or(|p| p.parse::<u16>().ok() == Some(port))
}

fn is_upgrade_request(headers: &HeaderMap) -> bool {
    headers.contains_key(UPGRADE) && connection_tokens(headers).any(|t| t == "upgrade")
}

fn connection_tokens(headers: &HeaderMap) -> impl Iterator<Item = String> + '_ {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
}

/// Headers that must not cross the proxy in either direction.
fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let listed: Vec<String> = connection_tokens(headers).collect();
    for name in HOP_BY_HOP
        .iter()
        .copied()
        .chain(listed.iter().map(String::as_str))
    {
        headers.remove(name);
    }
}

/// The inbound headers minus hop-by-hop, Host, length, and every inbound
/// credential. Lease headers are added per attempt.
fn request_headers(inbound: &HeaderMap, provider: Option<Provider>, upgrade: bool) -> HeaderMap {
    let mut headers = inbound.clone();
    let upgrade_value = headers.get(UPGRADE).cloned();
    strip_hop_by_hop(&mut headers);
    headers.remove(HOST);
    headers.remove(CONTENT_LENGTH);
    // `None`: a passthrough lease keeps the CLI's own credential.
    if let Some(provider) = provider {
        for name in GENERIC_CREDENTIAL_HEADERS
            .iter()
            .chain(provider.credential_headers())
        {
            headers.remove(*name);
        }
    }
    if upgrade && let Some(value) = upgrade_value {
        headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(UPGRADE, value);
    }
    headers
}

fn upstream_headers(
    stripped: &HeaderMap,
    own: &HeaderMap,
    lease: &Lease,
    provider: Provider,
) -> HeaderMap {
    let mut headers = if lease.passthrough { own } else { stripped }.clone();
    for (name, value) in &lease.headers {
        headers.insert(name.clone(), value.clone());
    }
    provider.finalize_headers(&mut headers);
    headers
}

fn response_headers(upstream: &HeaderMap, upgrade: bool) -> HeaderMap {
    let mut headers = upstream.clone();
    let upgrade_value = headers.get(UPGRADE).cloned();
    strip_hop_by_hop(&mut headers);
    if upgrade && let Some(value) = upgrade_value {
        headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(UPGRADE, value);
    }
    headers
}

fn body_edits_allowed(headers: &HeaderMap) -> bool {
    let json = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("json"));
    let encoded = headers
        .get(CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.trim().eq_ignore_ascii_case("identity"));
    json && !encoded
}

pub(crate) fn status_only(status: StatusCode) -> Response<ProxyBody> {
    let mut response = Response::new(empty());
    *response.status_mut() = status;
    response
}

/// A JSON error shaped so both Anthropic and OpenAI clients show the message.
fn proxy_error(status: StatusCode, message: &str) -> Response<ProxyBody> {
    let body = serde_json::json!({
        "type": "error",
        "error": { "type": "api_error", "message": message },
    });
    let mut response = Response::new(full(Bytes::from(body.to_string())));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn empty() -> ProxyBody {
    Empty::new().map_err(|never| match never {}).boxed_unsync()
}

fn full(bytes: Bytes) -> ProxyBody {
    Full::new(bytes)
        .map_err(|never| match never {})
        .boxed_unsync()
}

// --- request body -----------------------------------------------------------

enum RequestBody {
    /// Fully buffered; can be sent any number of times.
    Buffered {
        bytes: Bytes,
        json: Option<Option<serde_json::Value>>,
    },
    /// Too large to buffer; can be sent once.
    Streaming(Option<reqwest::Body>),
}

impl RequestBody {
    async fn read(body: Incoming, headers: &HeaderMap, cap: usize) -> Result<Self, hyper::Error> {
        let declared = headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        if declared.is_some_and(|len| len > cap as u64) {
            return Ok(Self::streaming(Bytes::new(), body));
        }
        let mut body = body;
        let mut buf = BytesMut::new();
        while let Some(frame) = body.frame().await {
            let Ok(data) = frame?.into_data() else {
                continue;
            };
            if buf.len() + data.len() > cap {
                buf.extend_from_slice(&data);
                return Ok(Self::streaming(buf.freeze(), body));
            }
            buf.extend_from_slice(&data);
        }
        Ok(Self::Buffered {
            bytes: buf.freeze(),
            json: None,
        })
    }

    fn streaming(prefix: Bytes, rest: Incoming) -> Self {
        let rest = BodyDataStream::new(rest).map_err(|e| Box::new(e) as BoxError);
        let stream = futures::stream::once(async move { Ok::<_, BoxError>(prefix) }).chain(rest);
        Self::Streaming(Some(reqwest::Body::wrap_stream(stream)))
    }

    fn replayable(&self) -> bool {
        matches!(self, Self::Buffered { .. })
    }

    fn for_attempt(&mut self, lease: &Lease, json_eligible: bool) -> reqwest::Body {
        match self {
            Self::Streaming(body) => body
                .take()
                .unwrap_or_else(|| reqwest::Body::from(Bytes::new())),
            Self::Buffered { bytes, json } => {
                if lease.body_edits.is_empty() || !json_eligible {
                    return reqwest::Body::from(bytes.clone());
                }
                let parsed = json.get_or_insert_with(|| serde_json::from_slice(bytes).ok());
                let Some(value) = parsed else {
                    return reqwest::Body::from(bytes.clone());
                };
                let mut value = value.clone();
                let mut changed = false;
                for edit in &lease.body_edits {
                    changed |= edit.apply(&mut value);
                }
                if !changed {
                    return reqwest::Body::from(bytes.clone());
                }
                match serde_json::to_vec(&value) {
                    Ok(v) => reqwest::Body::from(v),
                    Err(_) => reqwest::Body::from(bytes.clone()),
                }
            }
        }
    }
}

// --- upstream response --------------------------------------------------------

/// An upstream response whose body may have been partly read for
/// classification. Nothing has been sent downstream yet.
struct Upstream {
    status: StatusCode,
    headers: HeaderMap,
    prefix: Bytes,
    response: reqwest::Response,
    inspector: Option<Inspector>,
    /// Events read during the hold after the commit point; still to be
    /// inspected.
    leftover: Vec<SseEvent>,
    /// The provider refused the leased credential (401/403).
    auth_refused: bool,
}

impl Upstream {
    fn new(response: reqwest::Response) -> Self {
        Self {
            status: response.status(),
            headers: response.headers().clone(),
            prefix: Bytes::new(),
            response,
            inspector: None,
            leftover: Vec::new(),
            auth_refused: false,
        }
    }

    /// Reads up to `cap` bytes of the body (bounded by `timeout`).
    async fn sniff(&mut self, cap: usize, timeout: std::time::Duration) {
        let mut buf = BytesMut::from(&self.prefix[..]);
        let read = async {
            while buf.len() < cap {
                match self.response.chunk().await {
                    Ok(Some(chunk)) => buf.extend_from_slice(&chunk),
                    _ => break,
                }
            }
        };
        let _ = tokio::time::timeout(timeout, read).await;
        self.prefix = buf.freeze();
    }

    /// Holds back the start of a successful event stream while it carries
    /// only lifecycle events, so an error event that arrives before any model
    /// output can still fail over. Returns that error's classification.
    /// Gives up holding (and commits) at the first content event, or when the
    /// event, byte, or time budget runs out.
    async fn hold(
        &mut self,
        scanner: &mut SseScanner,
        provider: Provider,
        config: &ProxyConfig,
    ) -> Option<Classification> {
        let deadline = tokio::time::Instant::now() + config.sse_hold_timeout;
        let mut buf = BytesMut::from(&self.prefix[..]);
        let mut seen = 0usize;
        let mut events: Vec<SseEvent> = Vec::new();
        let mut result = None;
        'read: while seen < config.sse_hold_events && buf.len() < config.sse_hold_bytes {
            let chunk = match tokio::time::timeout_at(deadline, self.response.chunk()).await {
                Ok(Ok(Some(chunk))) => chunk,
                _ => break,
            };
            buf.extend_from_slice(&chunk);
            scanner.push(&chunk, &mut events);
            let mut batch = std::mem::take(&mut events).into_iter();
            while let Some(event) = batch.next() {
                seen += 1;
                match provider.stream_signal(&event, &self.headers) {
                    StreamSignal::Lifecycle => {}
                    StreamSignal::Content => {
                        self.leftover = batch.collect();
                        break 'read;
                    }
                    StreamSignal::Error(class) => {
                        result = Some(class);
                        break 'read;
                    }
                }
            }
        }
        self.prefix = buf.freeze();
        result
    }

    /// A response the proxy answers itself (no upstream request behind it).
    fn synthetic(status: StatusCode, headers: &[(&'static str, String)], body: String) -> Self {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            if let Ok(value) = HeaderValue::from_str(value) {
                map.insert(*name, value);
            }
        }
        map.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
        let response = reqwest::Response::from(http::Response::new(body));
        Self {
            status,
            headers: map,
            prefix: Bytes::new(),
            response,
            inspector: None,
            leftover: Vec::new(),
            auth_refused: false,
        }
    }

    /// Puts an explanation in front of the provider's message in a JSON
    /// error body that was read whole, so the CLI shows it in the chat.
    fn annotate_error(&mut self, note: &str) {
        let Ok(mut body) = serde_json::from_slice::<serde_json::Value>(&self.prefix) else {
            return;
        };
        let slot = match body.pointer_mut("/error/message") {
            Some(slot) => Some(slot),
            None => body.get_mut("message"),
        };
        let Some(slot) = slot.filter(|s| s.is_string()) else {
            return;
        };
        let original = slot.as_str().unwrap_or_default().to_string();
        *slot = serde_json::Value::String(format!("{note} ({original})"));
        let bytes = Bytes::from(body.to_string());
        self.headers.remove(CONTENT_ENCODING);
        self.headers
            .insert(CONTENT_LENGTH, HeaderValue::from(bytes.len()));
        self.prefix = bytes;
    }

    /// What the CLI gets. A refused credential is never shown as 401/403:
    /// the CLI would try to refresh its own login (Grok and Antigravity
    /// then force a re-login or wipe it), but the refused credential was one
    /// the proxy swapped in.
    fn into_downstream(self) -> Response<ProxyBody> {
        if self.auth_refused {
            let mut response = proxy_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Zeron account proxy: the provider refused the sign-in of the account this \
                 chat runs on. Sign in again in Zeron's account settings.",
            );
            response
                .headers_mut()
                .insert(http::header::RETRY_AFTER, HeaderValue::from_static("30"));
            return response;
        }
        self.into_response()
    }

    fn into_response(self) -> Response<ProxyBody> {
        let prefix = self.prefix;
        let mut inspector = self.inspector;
        let rest = self
            .response
            .bytes_stream()
            .map_err(|e| Box::new(e) as BoxError)
            .map_ok(move |chunk| {
                if let Some(inspector) = inspector.as_mut() {
                    inspector.observe(&chunk);
                }
                chunk
            });
        let stream = futures::stream::iter((!prefix.is_empty()).then_some(Ok(prefix)))
            .chain(rest)
            .map_ok(Frame::data);
        let mut response = Response::new(StreamBody::new(stream).boxed_unsync());
        *response.status_mut() = self.status;
        *response.headers_mut() = response_headers(&self.headers, false);
        response
    }
}

/// Whether a response is an uncompressed event stream we can read.
fn sse_inspectable(headers: &HeaderMap) -> bool {
    let sse = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.trim()
                .to_ascii_lowercase()
                .starts_with("text/event-stream")
        });
    let encoded = headers
        .get(CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.trim().eq_ignore_ascii_case("identity"));
    sse && !encoded
}

/// Watches a committed stream for in-band quota or credential errors and
/// reports the first one. Bytes pass through unchanged.
struct Inspector {
    scanner: SseScanner,
    events: Vec<SseEvent>,
    provider: Provider,
    headers: HeaderMap,
    source: Arc<dyn CredentialSource>,
    route: RouteInfo,
    account: AccountId,
    /// The CLI's own login went out: a credential error is its to handle.
    passthrough: bool,
    status: u16,
    done: bool,
}

impl Inspector {
    fn observe(&mut self, chunk: &[u8]) {
        if self.done {
            return;
        }
        self.scanner.push(chunk, &mut self.events);
        let events = std::mem::take(&mut self.events);
        self.check(events);
    }

    fn check(&mut self, events: Vec<SseEvent>) {
        for event in events {
            let StreamSignal::Error(class) = self.provider.stream_signal(&event, &self.headers)
            else {
                continue;
            };
            let rate_limits = self.provider.rate_limits(&self.headers);
            let own_login_refused = self.passthrough && class == Classification::Rejected;
            if let Some(outcome) =
                error_outcome(class, self.status, rate_limits).filter(|_| !own_login_refused)
            {
                tracing::info!(chat = %self.route.chat_id, account = %self.account, ?outcome, "in-band error after the stream started");
                self.source.report(&self.route, &self.account, outcome);
            }
            self.done = true;
            return;
        }
    }
}

/// The report for an error classification, if it concerns the account.
fn error_outcome(
    class: Classification,
    status: u16,
    rate_limits: crate::types::RateLimitSnapshot,
) -> Option<Outcome> {
    match class {
        Classification::LimitReached { resets_at, window } => Some(Outcome::LimitReached {
            resets_at,
            window,
            rate_limits,
        }),
        Classification::Rejected => Some(Outcome::Rejected { status }),
        Classification::IncompatibleHistory => Some(Outcome::IncompatibleHistory { status }),
        Classification::Success | Classification::Transient | Classification::Other => None,
    }
}

// --- failover ---------------------------------------------------------------

enum Next {
    Lease(Lease),
    Done(Response<ProxyBody>),
}

/// Failover state for one inbound request.
struct Attempts<'a> {
    inner: &'a Inner,
    route: &'a RouteEntry,
    /// The CLI's request headers, for the source's lease.
    inbound: &'a HeaderMap,
    exclude: Vec<AccountId>,
    /// A history refusal was already handled for this request (never loop).
    history_recovered: bool,
    rejected_retries: usize,
    /// The last error response, returned unchanged when no account is left.
    held: Option<Upstream>,
    /// Why the previous account was dropped, and which one it was.
    switched: Option<(AccountId, SwitchReason)>,
}

impl<'a> Attempts<'a> {
    fn new(inner: &'a Inner, route: &'a RouteEntry, inbound: &'a HeaderMap) -> Self {
        Self {
            inner,
            route,
            inbound,
            exclude: Vec::new(),
            history_recovered: false,
            rejected_retries: 0,
            held: None,
            switched: None,
        }
    }

    async fn next_lease(&mut self) -> Next {
        let info = &self.route.info;
        let result = self
            .inner
            .source
            .lease(info, self.inbound, &self.exclude)
            .await;
        let lease = match result {
            Ok(lease) if !self.exclude.contains(&lease.account) => lease,
            Ok(_) | Err(LeaseError::NoAccount) => {
                return Next::Done(self.give_up("no account available"));
            }
            Err(LeaseError::Failed(message)) => {
                tracing::warn!(chat = %info.chat_id, harness = %info.harness, %message, "credential lease failed");
                return Next::Done(self.give_up("credential source failed"));
            }
        };
        // Another account takes over; the held error is no longer needed.
        self.held = None;
        Next::Lease(lease)
    }

    /// Nothing left to try: the last upstream error if there was one.
    fn give_up(&mut self, why: &str) -> Response<ProxyBody> {
        match self.held.take() {
            Some(held) => {
                tracing::info!(
                    chat = %self.route.info.chat_id,
                    harness = %self.route.info.harness,
                    tried = self.exclude.len(),
                    "every account failed; passing the last error through"
                );
                self.inner.emit(ProxyEvent::Exhausted {
                    route: self.route.info.clone(),
                    tried: self.exclude.clone(),
                });
                held.into_downstream()
            }
            None => proxy_error(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("Zeron account proxy: {why}"),
            ),
        }
    }

    fn connect_failed(&mut self, err: reqwest::Error) -> Response<ProxyBody> {
        tracing::warn!(chat = %self.route.info.chat_id, err = %err.without_url(), "upstream request failed");
        match self.held.take() {
            Some(held) => held.into_downstream(),
            None => proxy_error(
                StatusCode::BAD_GATEWAY,
                "Zeron account proxy: upstream unreachable",
            ),
        }
    }

    /// Classifies an upstream response and reports it. Returns the response
    /// when it should go downstream, `None` to retry on another account.
    async fn classify(
        &mut self,
        lease: &Lease,
        mut upstream: Upstream,
        replayable: bool,
    ) -> Option<Upstream> {
        let config = &self.inner.config;
        let provider = self.route.info.provider;
        let status = upstream.status;
        if provider.sniff_body(status) {
            upstream
                .sniff(config.max_error_sniff, config.error_sniff_timeout)
                .await;
        }
        let mut class = provider.classify(status, &upstream.headers, &upstream.prefix);
        let mut scanner = (class == Classification::Success && sse_inspectable(&upstream.headers))
            .then(|| SseScanner::new(config.max_error_sniff));
        if replayable
            && let Some(scanner) = scanner.as_mut()
            && let Some(in_band) = upstream.hold(scanner, provider, config).await
        {
            class = in_band;
        }
        let source = &self.inner.source;
        let info = &self.route.info;
        let account = &lease.account;
        let rate_limits = provider.rate_limits(&upstream.headers);
        tracing::debug!(chat = %info.chat_id, %account, status = status.as_u16(), ?class, "upstream answered");
        let reason = match class {
            Classification::Success => {
                source.report(info, account, Outcome::Ok { rate_limits });
                if let Some(scanner) = scanner {
                    let mut inspector = Inspector {
                        scanner,
                        events: Vec::new(),
                        provider,
                        headers: upstream.headers.clone(),
                        source: source.clone(),
                        route: info.clone(),
                        account: account.clone(),
                        passthrough: lease.passthrough,
                        status: status.as_u16(),
                        done: false,
                    };
                    inspector.check(std::mem::take(&mut upstream.leftover));
                    upstream.inspector = Some(inspector);
                }
                self.served(account);
                return Some(upstream);
            }
            Classification::LimitReached { resets_at, window } => {
                source.report(
                    info,
                    account,
                    Outcome::LimitReached {
                        resets_at,
                        window: window.clone(),
                        rate_limits,
                    },
                );
                SwitchReason::LimitReached { resets_at, window }
            }
            // The CLI's own login was refused: pass the 401 on so the CLI's
            // own refresh runs, and leave the account's standing alone.
            // Grok answers a 401 by forcing a new sign-in, so it gets the
            // 503 there too.
            Classification::Rejected if lease.passthrough => {
                upstream.auth_refused = provider == Provider::Grok;
                self.served(account);
                return Some(upstream);
            }
            Classification::Rejected => {
                upstream.auth_refused = true;
                source.report(
                    info,
                    account,
                    Outcome::Rejected {
                        status: status.as_u16(),
                    },
                );
                if self.rejected_retries >= MAX_REJECTED_RETRIES {
                    self.served(account);
                    return Some(upstream);
                }
                self.rejected_retries += 1;
                SwitchReason::Rejected
            }
            Classification::IncompatibleHistory => {
                source.report(
                    info,
                    account,
                    Outcome::IncompatibleHistory {
                        status: status.as_u16(),
                    },
                );
                upstream.annotate_error(INCOMPATIBLE_HISTORY_NOTE);
                if replayable && !self.history_recovered {
                    self.history_recovered = true;
                    match source.incompatible_history(info, account).await {
                        HistoryRecovery::Retry => {
                            tracing::info!(chat = %info.chat_id, %account,
                                "history tied to another account; retrying there");
                            self.exclude.push(account.clone());
                            self.switched = Some((account.clone(), SwitchReason::HistoryBound));
                            self.held = Some(upstream);
                            return None;
                        }
                        HistoryRecovery::LimitReached { resets_at, window } => {
                            tracing::info!(chat = %info.chat_id, %account,
                                "history tied to an account that is out of usage");
                            self.served(account);
                            let (status, headers, body) =
                                provider.usage_limit_response(resets_at, window.as_deref());
                            return Some(Upstream::synthetic(status, &headers, body));
                        }
                        HistoryRecovery::PassThrough => {}
                    }
                }
                self.served(account);
                return Some(upstream);
            }
            Classification::Transient | Classification::Other => {
                self.served(account);
                return Some(upstream);
            }
        };
        if !replayable {
            self.served(account);
            return Some(upstream);
        }
        tracing::info!(chat = %info.chat_id, harness = %info.harness, %account, ?reason, "account failed; trying another");
        self.exclude.push(account.clone());
        self.switched = Some((account.clone(), reason));
        // Keep the response: it is what the CLI gets if no account is left.
        self.held = Some(upstream);
        None
    }

    /// Records the account whose response goes downstream and emits a switch
    /// event when it differs from the route's previous one.
    fn served(&mut self, account: &AccountId) {
        let mut last = self
            .route
            .last_account
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = last.replace(account.clone());
        let event = match self.switched.take() {
            Some((from, reason)) if from != *account => Some((Some(from), reason)),
            Some(_) => None,
            None => match previous {
                Some(prev) if prev != *account => Some((Some(prev), SwitchReason::Reassigned)),
                _ => None,
            },
        };
        drop(last);
        if let Some((from, reason)) = event {
            self.inner.emit(ProxyEvent::AccountSwitched {
                route: self.route.info.clone(),
                from,
                to: account.clone(),
                reason,
            });
        }
    }
}

// --- WebSocket / upgrade ----------------------------------------------------

async fn handle_upgrade(
    inner: Arc<Inner>,
    route: Arc<RouteEntry>,
    mut req: Request<Incoming>,
    url: reqwest::Url,
) -> Response<ProxyBody> {
    let provider = route.info.provider;
    let headers = request_headers(req.headers(), Some(provider), true);
    let own_headers = request_headers(req.headers(), None, true);
    let method = req.method().clone();
    let inbound = req.headers().clone();
    let downstream = hyper::upgrade::on(&mut req);
    let mut attempt = Attempts::new(&inner, &route, &inbound);
    loop {
        let lease = match attempt.next_lease().await {
            Next::Lease(lease) => lease,
            Next::Done(response) => return response,
        };
        let request = inner
            .upgrade_client
            .request(method.clone(), url.clone())
            .version(http::Version::HTTP_11)
            .headers(upstream_headers(&headers, &own_headers, &lease, provider));
        let upstream = match request.send().await {
            Ok(resp) => resp,
            Err(err) => return attempt.connect_failed(err),
        };
        if upstream.status() == StatusCode::SWITCHING_PROTOCOLS {
            let rate_limits = provider.rate_limits(upstream.headers());
            inner
                .source
                .report(&route.info, &lease.account, Outcome::Ok { rate_limits });
            attempt.served(&lease.account);
            let mut response = status_only(StatusCode::SWITCHING_PROTOCOLS);
            *response.headers_mut() = response_headers(upstream.headers(), true);
            let chat = route.info.chat_id.clone();
            tokio::spawn(async move {
                let up = match upstream.upgrade().await {
                    Ok(up) => up,
                    Err(err) => {
                        tracing::debug!(%chat, %err, "upstream upgrade failed");
                        return;
                    }
                };
                let down = match downstream.await {
                    Ok(down) => down,
                    Err(err) => {
                        tracing::debug!(%chat, %err, "downstream upgrade failed");
                        return;
                    }
                };
                let mut down = TokioIo::new(down);
                let mut up = up;
                if let Err(err) = tokio::io::copy_bidirectional(&mut down, &mut up).await {
                    tracing::debug!(%chat, %err, "websocket tunnel closed with error");
                }
            });
            return response;
        }
        if let Some(serve) = attempt
            .classify(&lease, Upstream::new(upstream), true)
            .await
        {
            return serve.into_downstream();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_path() {
        assert_eq!(
            split_route_path("/r/abc/v1/messages"),
            Some(("abc", "/v1/messages"))
        );
        assert_eq!(split_route_path("/r/abc"), Some(("abc", "/")));
        assert_eq!(split_route_path("/x/abc"), None);
    }

    #[test]
    fn local_hosts() {
        let mut h = HeaderMap::new();
        h.insert(HOST, HeaderValue::from_static("127.0.0.1:4000"));
        assert!(host_is_local(&h, 4000));
        assert!(!host_is_local(&h, 4001));
        h.insert(HOST, HeaderValue::from_static("localhost:4000"));
        assert!(host_is_local(&h, 4000));
        h.insert(HOST, HeaderValue::from_static("[::1]:4000"));
        assert!(host_is_local(&h, 4000));
        h.insert(HOST, HeaderValue::from_static("evil.example:4000"));
        assert!(!host_is_local(&h, 4000));
        h.remove(HOST);
        assert!(!host_is_local(&h, 4000));
    }

    #[test]
    fn hop_by_hop_and_credentials_stripped() {
        let mut h = HeaderMap::new();
        h.insert(
            CONNECTION,
            HeaderValue::from_static("keep-alive, x-custom-hop"),
        );
        h.insert("x-custom-hop", HeaderValue::from_static("1"));
        h.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        h.insert("te", HeaderValue::from_static("trailers"));
        h.insert(
            "authorization",
            HeaderValue::from_static("Bearer placeholder"),
        );
        h.insert("x-api-key", HeaderValue::from_static("placeholder"));
        h.insert("chatgpt-account-id", HeaderValue::from_static("someone"));
        h.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        h.insert(HOST, HeaderValue::from_static("127.0.0.1:1"));
        let out = request_headers(&h, Some(Provider::ChatGpt), false);
        let names: Vec<_> = out.keys().map(|k| k.as_str()).collect();
        assert_eq!(names, vec!["anthropic-version"]);
    }
}
