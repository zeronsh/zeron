//! End-to-end tests against fake upstream servers on loopback.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::{SinkExt, StreamExt};
use http::{HeaderMap, HeaderName, Request, Response, StatusCode};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zeron_account_proxy::{
    AccountId, AccountProxy, CredentialSource, HistoryRecovery, Lease, LeaseError, Outcome,
    Provider, ProxyConfig, ProxyEvent, Route, RouteInfo, SwitchReason,
};

type Body = UnsyncBoxBody<Bytes, Infallible>;

// --- fake credential source --------------------------------------------------

struct FakeSource {
    accounts: Vec<&'static str>,
    reports: Mutex<Vec<(String, Outcome)>>,
    leases: AtomicUsize,
    claude_identity: bool,
    /// This account is the CLI's own login: its leases pass the inbound
    /// credential through.
    passthrough: Option<&'static str>,
    /// The answer to a history refusal.
    history: HistoryRecovery,
}

impl FakeSource {
    fn new(accounts: &[&'static str]) -> Arc<Self> {
        Arc::new(Self {
            accounts: accounts.to_vec(),
            reports: Mutex::new(Vec::new()),
            leases: AtomicUsize::new(0),
            claude_identity: false,
            passthrough: None,
            history: HistoryRecovery::PassThrough,
        })
    }

    fn reports(&self) -> Vec<(String, Outcome)> {
        self.reports.lock().unwrap().clone()
    }
}

#[async_trait]
impl CredentialSource for FakeSource {
    async fn lease(
        &self,
        route: &RouteInfo,
        _inbound: &HeaderMap,
        exclude: &[AccountId],
    ) -> Result<Lease, LeaseError> {
        self.leases.fetch_add(1, Ordering::SeqCst);
        let id = self
            .accounts
            .iter()
            .find(|id| !exclude.iter().any(|e| e.as_str() == **id))
            .ok_or(LeaseError::NoAccount)?;
        if self.passthrough == Some(*id) {
            return Ok(Lease::passthrough(*id));
        }
        let mut lease = Lease::new(*id).bearer(&format!("tok-{id}")).unwrap();
        if route.provider == Provider::ChatGpt {
            lease = lease
                .header(
                    HeaderName::from_static("chatgpt-account-id"),
                    &format!("acct-{id}"),
                )
                .unwrap();
        }
        if self.claude_identity {
            lease = lease.claude_identity(&format!("device-{id}"), &format!("uuid-{id}"));
        }
        Ok(lease)
    }

    async fn incompatible_history(
        &self,
        _route: &RouteInfo,
        _account: &AccountId,
    ) -> HistoryRecovery {
        self.history.clone()
    }

    fn report(&self, _route: &RouteInfo, account: &AccountId, outcome: Outcome) {
        self.reports
            .lock()
            .unwrap()
            .push((account.as_str().to_string(), outcome));
    }
}

// --- fake upstream -----------------------------------------------------------

#[derive(Clone, Debug)]
struct Recorded {
    path_and_query: String,
    headers: HeaderMap,
    body: Bytes,
}

impl Recorded {
    /// The account a request was sent for, from the swapped bearer.
    fn account(&self) -> String {
        self.headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer tok-"))
            .unwrap_or("")
            .to_string()
    }
}

type Handler = Arc<dyn Fn(Recorded) -> BoxFuture<'static, Response<Body>> + Send + Sync>;

struct Upstream {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

impl Upstream {
    /// Requests the proxy forwarded (they all carry a credential). Stray
    /// local connections to the ephemeral port from other processes on the
    /// machine are ignored.
    fn seen(&self) -> Vec<Recorded> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.headers.contains_key("authorization"))
            .cloned()
            .collect()
    }
}

async fn upstream(handler: Handler) -> Upstream {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let handler = handler.clone();
            let log = log.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let handler = handler.clone();
                    let log = log.clone();
                    async move {
                        let (parts, body) = req.into_parts();
                        let body = body.collect().await.unwrap().to_bytes();
                        let recorded = Recorded {
                            path_and_query: parts
                                .uri
                                .path_and_query()
                                .map(|p| p.to_string())
                                .unwrap_or_default(),
                            headers: parts.headers,
                            body,
                        };
                        log.lock().unwrap().push(recorded.clone());
                        Ok::<_, Infallible>(handler(recorded).await)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    Upstream { addr, seen }
}

fn handler<F>(f: F) -> Handler
where
    F: Fn(Recorded) -> Response<Body> + Send + Sync + 'static,
{
    Arc::new(move |r| {
        let response = f(r);
        Box::pin(async move { response })
    })
}

fn full(status: u16, headers: &[(&str, &str)], body: &str) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    for (n, v) in headers {
        builder = builder.header(*n, *v);
    }
    builder
        .body(Full::new(Bytes::from(body.to_string())).boxed_unsync())
        .unwrap()
}

/// A response whose body is fed by the returned sender.
fn streaming(
    status: u16,
    headers: &[(&str, &str)],
) -> (Response<Body>, mpsc::UnboundedSender<Bytes>) {
    let (tx, rx) = mpsc::unbounded::<Bytes>();
    let mut builder = Response::builder().status(status);
    for (n, v) in headers {
        builder = builder.header(*n, *v);
    }
    let body = StreamBody::new(rx.map(|b| Ok::<_, Infallible>(Frame::data(b)))).boxed_unsync();
    (builder.body(body).unwrap(), tx)
}

const SSE: (&str, &str) = ("content-type", "text/event-stream");

fn quota_429_headers() -> Vec<(&'static str, &'static str)> {
    vec![
        ("content-type", "application/json"),
        ("anthropic-ratelimit-unified-status", "rejected"),
        ("anthropic-ratelimit-unified-5h-status", "rejected"),
        ("anthropic-ratelimit-unified-5h-reset", "1900000000"),
        ("anthropic-ratelimit-unified-reset", "1900000000"),
        (
            "anthropic-ratelimit-unified-representative-claim",
            "five_hour",
        ),
    ]
}

const QUOTA_BODY: &str =
    r#"{"type":"error","error":{"type":"rate_limit_error","message":"usage limit reached"}}"#;

// --- proxy setup -------------------------------------------------------------

struct Harness {
    proxy: AccountProxy,
    route: Route,
    source: Arc<FakeSource>,
    client: reqwest::Client,
}

async fn harness(
    provider: Provider,
    source: Arc<FakeSource>,
    upstream: SocketAddr,
    tweak: impl FnOnce(&mut ProxyConfig),
) -> Harness {
    let mut config = ProxyConfig {
        system_proxy: false,
        upstream_overrides: HashMap::from([(provider, format!("http://{upstream}"))]),
        ..ProxyConfig::default()
    };
    tweak(&mut config);
    let proxy = AccountProxy::start(source.clone(), config).await.unwrap();
    let route = proxy.register(RouteInfo {
        chat_id: "chat-1".into(),
        harness: match provider {
            Provider::Anthropic => "claude-code".into(),
            Provider::ChatGpt => "codex".into(),
            Provider::Grok => "grok".into(),
            Provider::Muse => "muse".into(),
        },
        provider,
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .build()
        .unwrap();
    Harness {
        proxy,
        route,
        source,
        client,
    }
}

impl Harness {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.route.base_url(), path)
    }

    async fn post(&self, path: &str, body: &str) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("authorization", "Bearer placeholder")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .unwrap()
    }
}

async fn next_event(rx: &mut tokio::sync::broadcast::Receiver<ProxyEvent>) -> ProxyEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("event")
        .unwrap()
}

/// Sends raw bytes and returns everything read until close (or 3 s).
async fn raw(addr: SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

// --- tests -------------------------------------------------------------------

#[tokio::test]
async fn swaps_auth_and_never_forwards_inbound_credentials() {
    let up = upstream(handler(|_| {
        full(
            200,
            &[
                ("content-type", "application/json"),
                ("anthropic-ratelimit-unified-status", "allowed"),
                ("anthropic-ratelimit-unified-5h-utilization", "0.25"),
            ],
            "{\"ok\":true}",
        )
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h
        .client
        .post(h.url("/v1/messages?beta=true"))
        .header("authorization", "Bearer placeholder")
        .header("x-api-key", "placeholder-key")
        .header("cookie", "session=secret")
        .header("anthropic-beta", "claude-code-20250219")
        .header("anthropic-version", "2023-06-01")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // Upstream rate-limit headers reach the CLI.
    assert_eq!(
        resp.headers()["anthropic-ratelimit-unified-status"],
        "allowed"
    );
    assert_eq!(resp.text().await.unwrap(), "{\"ok\":true}");

    let seen = up.seen();
    assert_eq!(seen.len(), 1);
    let r = &seen[0];
    assert_eq!(r.path_and_query, "/v1/messages?beta=true");
    assert_eq!(r.headers["authorization"], "Bearer tok-a");
    assert!(r.headers.get("x-api-key").is_none());
    assert!(r.headers.get("cookie").is_none());
    assert_eq!(r.headers["anthropic-version"], "2023-06-01");
    assert_eq!(
        r.headers["anthropic-beta"],
        "claude-code-20250219,oauth-2025-04-20"
    );
    assert_eq!(r.headers["host"], up.addr.to_string());
    let all_headers = format!("{:?}", r.headers);
    assert!(!all_headers.contains("placeholder"));
    assert!(!all_headers.contains(h.route.base_url().rsplit('/').next().unwrap()));

    let reports = h.source.reports();
    assert_eq!(reports.len(), 1);
    match &reports[0] {
        (account, Outcome::Ok { rate_limits }) => {
            assert_eq!(account, "a");
            assert_eq!(rate_limits.status.as_deref(), Some("allowed"));
            assert_eq!(rate_limits.windows[0].used_percent, Some(25.0));
        }
        other => panic!("unexpected report {other:?}"),
    }
}

#[tokio::test]
async fn codex_swaps_account_id_header() {
    let up = upstream(handler(|_| full(200, &[], "ok"))).await;
    let h = harness(Provider::ChatGpt, FakeSource::new(&["a"]), up.addr, |_| {}).await;
    let resp = h
        .client
        .post(h.url("/backend-api/codex/responses"))
        .header("authorization", "Bearer placeholder")
        .header("chatgpt-account-id", "placeholder-account")
        .header("session_id", "s-1")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let r = &up.seen()[0];
    assert_eq!(r.path_and_query, "/backend-api/codex/responses");
    assert_eq!(r.headers["authorization"], "Bearer tok-a");
    assert_eq!(r.headers["chatgpt-account-id"], "acct-a");
    assert_eq!(r.headers["session_id"], "s-1");
}

#[tokio::test]
async fn sse_arrives_incrementally() {
    let senders: Arc<Mutex<Vec<mpsc::UnboundedSender<Bytes>>>> = Arc::default();
    let slot = senders.clone();
    let up = upstream(handler(move |_| {
        let (resp, tx) = streaming(200, &[SSE]);
        slot.lock().unwrap().push(tx);
        resp
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a"]),
        up.addr,
        |_| {},
    )
    .await;

    let request = tokio::spawn({
        let client = h.client.clone();
        let url = h.url("/v1/messages");
        async move { client.post(url).body("{}").send().await.unwrap() }
    });
    // Wait until the upstream got the request.
    let tx = loop {
        if let Some(tx) = senders.lock().unwrap().first().cloned() {
            break tx;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    tx.unbounded_send(Bytes::from_static(
        b"event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"Hel\"}}\n\n",
    ))
    .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .expect("headers arrive while the stream is open")
        .unwrap();
    assert_eq!(resp.status(), 200);
    let mut body = resp.bytes_stream();
    let mut got = Vec::new();
    while !String::from_utf8_lossy(&got).contains("Hel") {
        let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
            .await
            .expect("first event flushed before the stream ends")
            .unwrap()
            .unwrap();
        got.extend_from_slice(&chunk);
    }
    // The second event is only sent after the first was received.
    tx.unbounded_send(Bytes::from_static(
        b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ))
    .unwrap();
    drop(tx);
    senders.lock().unwrap().clear();
    while let Some(chunk) = body.next().await {
        got.extend_from_slice(&chunk.unwrap());
    }
    assert!(String::from_utf8_lossy(&got).ends_with("data: {\"type\":\"message_stop\"}\n\n"));
}

#[tokio::test]
async fn stalled_lifecycle_stream_is_released_after_the_hold() {
    let senders: Arc<Mutex<Vec<mpsc::UnboundedSender<Bytes>>>> = Arc::default();
    let slot = senders.clone();
    let up = upstream(handler(move |_| {
        let (resp, tx) = streaming(200, &[SSE]);
        tx.unbounded_send(Bytes::from_static(
            b"event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
        ))
        .unwrap();
        slot.lock().unwrap().push(tx);
        resp
    }))
    .await;
    let h = harness(Provider::Anthropic, FakeSource::new(&["a"]), up.addr, |c| {
        c.sse_hold_timeout = Duration::from_millis(300);
    })
    .await;
    let started = std::time::Instant::now();
    let resp = tokio::time::timeout(Duration::from_secs(5), h.post("/v1/messages", "{}"))
        .await
        .expect("released after the hold timeout");
    assert!(started.elapsed() >= Duration::from_millis(250));
    let mut body = resp.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("message_start"));
    senders.lock().unwrap().clear();
}

#[tokio::test]
async fn quota_429_fails_over_to_next_account() {
    let up = upstream(handler(|r| {
        if r.account() == "a" {
            full(429, &quota_429_headers(), QUOTA_BODY)
        } else {
            full(
                200,
                &[("content-type", "application/json")],
                "{\"from\":\"b\"}",
            )
        }
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let mut events = h.proxy.subscribe();
    let resp = h
        .post("/v1/messages", r#"{"model":"m","messages":[]}"#)
        .await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "{\"from\":\"b\"}");

    let seen = up.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].account(), "a");
    assert_eq!(seen[1].account(), "b");
    assert_eq!(seen[0].body, seen[1].body, "the same body is replayed");

    let reports = h.source.reports();
    assert_eq!(reports.len(), 2);
    match &reports[0] {
        (
            a,
            Outcome::LimitReached {
                resets_at, window, ..
            },
        ) => {
            assert_eq!(a, "a");
            assert_eq!(resets_at.unwrap().timestamp(), 1_900_000_000);
            assert_eq!(window.as_deref(), Some("five_hour"));
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(&reports[1], (b, Outcome::Ok { .. }) if b == "b"));

    match next_event(&mut events).await {
        ProxyEvent::AccountSwitched {
            route,
            from,
            to,
            reason,
        } => {
            assert_eq!(route.chat_id, "chat-1");
            assert_eq!(from.unwrap().as_str(), "a");
            assert_eq!(to.as_str(), "b");
            assert!(matches!(reason, SwitchReason::LimitReached { .. }));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn in_band_quota_before_content_fails_over() {
    let up = upstream(handler(|r| {
        if r.account() == "a" {
            full(
                200,
                &[SSE],
                "data: {\"type\":\"response.created\"}\n\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_limit_reached\",\"message\":\"limit\",\"resets_in_seconds\":600}}}\n\n",
            )
        } else {
            full(200, &[SSE], "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n")
        }
    }))
    .await;
    let h = harness(
        Provider::ChatGpt,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h.post("/backend-api/codex/responses", "{}").await;
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(text.contains("output_text.delta"), "{text}");
    assert!(!text.contains("usage_limit_reached"));
    let reports = h.source.reports();
    assert!(
        matches!(&reports[0], (a, Outcome::LimitReached { resets_at: Some(_), .. }) if a == "a")
    );
    assert!(matches!(&reports[1], (b, Outcome::Ok { .. }) if b == "b"));
}

#[tokio::test]
async fn error_after_stream_started_passes_through() {
    let up = upstream(handler(|_| {
        full(
            200,
            // The stream was admitted, then its 5-hour window ran out.
            &[
                SSE,
                ("anthropic-ratelimit-unified-status", "rejected"),
                ("anthropic-ratelimit-unified-5h-status", "rejected"),
                ("anthropic-ratelimit-unified-5h-reset", "1900000000"),
            ],
            "event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\"}\n\nevent: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"limit\"}}\n\n",
        )
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h.post("/v1/messages", "{}").await;
    assert_eq!(resp.status(), 200);
    let text = resp.text().await.unwrap();
    assert!(text.contains("content_block_delta"));
    assert!(
        text.contains("rate_limit_error"),
        "the in-band error reaches the CLI"
    );
    assert_eq!(up.seen().len(), 1, "never retried after bytes flowed");
    let reports = h.source.reports();
    assert_eq!(reports.len(), 2);
    assert!(matches!(&reports[0], (a, Outcome::Ok { .. }) if a == "a"));
    assert!(matches!(&reports[1], (a, Outcome::LimitReached { .. }) if a == "a"));
}

#[tokio::test]
async fn all_accounts_exhausted_returns_last_error() {
    let up = upstream(handler(|r| {
        let mut headers = quota_429_headers();
        let marker = if r.account() == "a" { "a" } else { "b" };
        headers.push(("x-upstream-account", marker));
        full(429, &headers, &format!("{{\"type\":\"error\",\"error\":{{\"type\":\"rate_limit_error\",\"message\":\"limit {marker}\"}}}}"))
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let mut events = h.proxy.subscribe();
    let resp = h.post("/v1/messages", "{}").await;
    assert_eq!(resp.status(), 429);
    assert_eq!(resp.headers()["x-upstream-account"], "b");
    assert_eq!(
        resp.headers()["anthropic-ratelimit-unified-status"],
        "rejected"
    );
    assert_eq!(
        resp.text().await.unwrap(),
        r#"{"type":"error","error":{"type":"rate_limit_error","message":"limit b"}}"#
    );
    assert_eq!(up.seen().len(), 2);
    let reports = h.source.reports();
    assert_eq!(reports.len(), 2);
    assert!(
        reports
            .iter()
            .all(|(_, o)| matches!(o, Outcome::LimitReached { .. }))
    );
    match next_event(&mut events).await {
        ProxyEvent::Exhausted { tried, .. } => {
            assert_eq!(
                tried.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
                vec!["a", "b"]
            );
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn unauthorized_reports_rejected_retries_once_and_hides_the_401() {
    let up = upstream(handler(|r| {
        if r.account() == "c" {
            full(200, &[], "ok")
        } else {
            full(
                401,
                &[("content-type", "application/json")],
                r#"{"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired"}}"#,
            )
        }
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b", "c"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h.post("/v1/messages", "{}").await;
    // a -> 401, retried once on b -> 401 (c is never tried). The CLI never
    // sees the 401: it would try to refresh a login the proxy swapped out.
    assert_eq!(resp.status(), 503);
    assert_eq!(resp.headers()["retry-after"], "30");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Sign in again")
    );
    assert_eq!(
        up.seen().iter().map(Recorded::account).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    let reports = h.source.reports();
    assert_eq!(
        reports,
        vec![
            ("a".to_string(), Outcome::Rejected { status: 401 }),
            ("b".to_string(), Outcome::Rejected { status: 401 }),
        ]
    );
}

#[tokio::test]
async fn unauthorized_then_success_on_next_account() {
    let up = upstream(handler(|r| {
        if r.account() == "a" {
            full(401, &[], "")
        } else {
            full(200, &[], "ok")
        }
    }))
    .await;
    let h = harness(
        Provider::ChatGpt,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let mut events = h.proxy.subscribe();
    let resp = h.post("/backend-api/codex/responses", "{}").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
    let reports = h.source.reports();
    assert_eq!(
        reports[0],
        ("a".to_string(), Outcome::Rejected { status: 401 })
    );
    assert!(matches!(
        next_event(&mut events).await,
        ProxyEvent::AccountSwitched {
            reason: SwitchReason::Rejected,
            ..
        }
    ));
}

#[tokio::test]
async fn grok_never_sees_an_auth_failure() {
    let up = upstream(handler(|r| match r.account().as_str() {
        "a" => full(
            403,
            &[("content-type", "application/json")],
            r#"{"code":"unauthenticated:bad-credentials","error":"The OAuth2 access token could not be validated."}"#,
        ),
        _ => full(200, &[SSE], "data: {\"type\":\"response.output_text.delta\"}\n\n"),
    }))
    .await;
    let h = harness(
        Provider::Grok,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h
        .client
        .post(h.url("/v1/responses"))
        .header("authorization", "Bearer live-login")
        .header("x-xai-token-auth", "xai-grok-cli")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let seen = up.seen();
    assert_eq!(
        seen.iter().map(Recorded::account).collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(seen[1].path_and_query, "/v1/responses");
    assert_eq!(seen[1].headers["x-xai-token-auth"], "xai-grok-cli");
    assert_eq!(
        h.source.reports()[0],
        ("a".to_string(), Outcome::Rejected { status: 403 })
    );

    // A lone account whose token is refused: 503, not 401.
    let up = upstream(handler(|_| full(401, &[], ""))).await;
    let h = harness(Provider::Grok, FakeSource::new(&["a"]), up.addr, |_| {}).await;
    let resp = h.post("/v1/responses", "{}").await;
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn passthrough_keeps_the_clis_login_and_still_fails_over() {
    let up = upstream(handler(|r| {
        match r.headers["authorization"].to_str().unwrap() {
            "Bearer cli-own" => full(429, &quota_429_headers(), QUOTA_BODY),
            _ => full(200, &[], "ok"),
        }
    }))
    .await;
    let source = Arc::new(FakeSource {
        passthrough: Some("live"),
        ..Arc::try_unwrap(FakeSource::new(&["live", "b"]))
            .ok()
            .unwrap()
    });
    let h = harness(Provider::Anthropic, source, up.addr, |_| {}).await;
    let resp = h
        .client
        .post(h.url("/v1/messages"))
        .header("authorization", "Bearer cli-own")
        .header("x-api-key", "cli-key")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let seen = up.seen();
    assert_eq!(seen.len(), 2);
    // The live account's request carried the CLI's own credentials...
    assert_eq!(seen[0].headers["authorization"], "Bearer cli-own");
    assert_eq!(seen[0].headers["x-api-key"], "cli-key");
    // ...and the failover a swapped one, with none of the CLI's.
    assert_eq!(seen[1].headers["authorization"], "Bearer tok-b");
    assert!(seen[1].headers.get("x-api-key").is_none());
    assert!(matches!(&h.source.reports()[0], (a, Outcome::LimitReached { .. }) if a == "live"));
}

#[tokio::test]
async fn transient_overload_is_not_retried() {
    let up = upstream(handler(|_| {
        full(
            529,
            &[("content-type", "application/json")],
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        )
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h.post("/v1/messages", "{}").await;
    assert_eq!(resp.status(), 529);
    assert!(resp.text().await.unwrap().contains("overloaded_error"));
    assert_eq!(up.seen().len(), 1);
    assert!(
        h.source.reports().is_empty(),
        "overload says nothing about the account"
    );
}

#[tokio::test]
async fn foreign_signature_is_reported_not_retried() {
    let up = upstream(handler(|_| {
        full(
            400,
            &[("content-type", "application/json")],
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"messages.3.content.0: Invalid `signature` in `thinking` block"}}"#,
        )
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h.post("/v1/messages", "{}").await;
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("This chat's history holds reasoning from another account"));
    assert!(message.contains("Invalid `signature` in `thinking` block"));
    assert_eq!(body["error"]["type"], "invalid_request_error");
    assert_eq!(up.seen().len(), 1);
    assert_eq!(
        h.source.reports(),
        vec![(
            "a".to_string(),
            Outcome::IncompatibleHistory { status: 400 }
        )]
    );
}

#[tokio::test]
async fn unknown_or_dropped_route_is_404() {
    let up = upstream(handler(|_| full(200, &[], "ok"))).await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a"]),
        up.addr,
        |_| {},
    )
    .await;
    let addr = h.proxy.local_addr();
    for path in [
        format!("/r/{}/v1/messages", "0".repeat(64)),
        "/r/short/v1/messages".to_string(),
        "/v1/messages".to_string(),
        "/".to_string(),
    ] {
        let resp = h
            .client
            .get(format!("http://{addr}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404, "{path}");
        assert!(resp.bytes().await.unwrap().is_empty());
    }
    let url = h.url("/v1/messages");
    assert_eq!(h.client.get(&url).send().await.unwrap().status(), 200);
    let Harness {
        proxy,
        route,
        client,
        ..
    } = h;
    drop(route);
    assert_eq!(proxy.route_count(), 0);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 404);
    assert_eq!(up.seen().len(), 1);
}

#[tokio::test]
async fn refuses_forward_proxy_use_and_foreign_hosts() {
    let up = upstream(handler(|_| full(200, &[], "ok"))).await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a"]),
        up.addr,
        |_| {},
    )
    .await;
    let addr = h.proxy.local_addr();
    let path = h
        .route
        .base_url()
        .strip_prefix(&format!("http://{addr}"))
        .unwrap()
        .to_string();

    let connect = raw(
        addr,
        "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
    )
    .await;
    assert!(connect.starts_with("HTTP/1.1 405"), "{connect}");
    let absolute = raw(
        addr,
        &format!("GET http://{addr}{path}/v1/messages HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert!(absolute.starts_with("HTTP/1.1 400"), "{absolute}");
    let rebinding = raw(
        addr,
        &format!(
            "GET {path}/v1/messages HTTP/1.1\r\nHost: evil.example:{}\r\nConnection: close\r\n\r\n",
            addr.port()
        ),
    )
    .await;
    assert!(rebinding.starts_with("HTTP/1.1 403"), "{rebinding}");
    assert!(up.seen().is_empty(), "{:?}", up.seen());
}

#[tokio::test]
async fn hop_by_hop_headers_are_dropped_both_ways() {
    let up = upstream(handler(|_| {
        full(
            200,
            &[
                ("connection", "x-upstream-hop"),
                ("x-upstream-hop", "1"),
                ("x-kept", "1"),
            ],
            "ok",
        )
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a"]),
        up.addr,
        |_| {},
    )
    .await;
    let addr = h.proxy.local_addr();
    let path = h
        .route
        .base_url()
        .strip_prefix(&format!("http://{addr}"))
        .unwrap()
        .to_string();
    let response = raw(
        addr,
        &format!(
            "POST {path}/v1/messages HTTP/1.1\r\nHost: {addr}\r\nConnection: close, x-client-hop\r\nX-Client-Hop: 1\r\nKeep-Alive: timeout=5\r\nTE: trailers\r\nProxy-Authorization: Basic Zm9vOmJhcg==\r\nX-Normal: yes\r\nContent-Length: 2\r\n\r\n{{}}"
        ),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let lower = response.to_ascii_lowercase();
    assert!(lower.contains("x-kept: 1"));
    assert!(!lower.contains("x-upstream-hop"), "{response}");

    let r = &up.seen()[0];
    assert_eq!(r.headers["x-normal"], "yes");
    for name in ["x-client-hop", "keep-alive", "te", "proxy-authorization"] {
        assert!(r.headers.get(name).is_none(), "{name} forwarded");
    }
    assert_eq!(r.body, Bytes::from_static(b"{}"));
}

#[tokio::test]
async fn claude_identity_is_rewritten_per_account() {
    let up = upstream(handler(|r| {
        if r.account() == "a" {
            full(429, &quota_429_headers(), QUOTA_BODY)
        } else {
            full(200, &[], "ok")
        }
    }))
    .await;
    let source = Arc::new(FakeSource {
        claude_identity: true,
        ..Arc::try_unwrap(FakeSource::new(&["a", "b"])).ok().unwrap()
    });
    let h = harness(Provider::Anthropic, source, up.addr, |_| {}).await;
    let user_id = r#"{"device_id":"cli-device","account_uuid":"cli-uuid","session_id":"sess-1","parent_session_id":"p"}"#;
    let body = serde_json::json!({
        "model": "claude",
        "metadata": { "user_id": user_id },
        "messages": [],
        "stream": true,
    })
    .to_string();
    let resp = h.post("/v1/messages", &body).await;
    assert_eq!(resp.status(), 200);
    let seen = up.seen();
    for (r, id) in seen.iter().zip(["a", "b"]) {
        let sent: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
        let keys: Vec<_> = sent.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            vec!["model", "metadata", "messages", "stream"],
            "key order kept"
        );
        let inner: serde_json::Value =
            serde_json::from_str(sent["metadata"]["user_id"].as_str().unwrap()).unwrap();
        assert_eq!(inner["device_id"], format!("device-{id}"));
        assert_eq!(inner["account_uuid"], format!("uuid-{id}"));
        assert_eq!(inner["session_id"], "sess-1");
        assert_eq!(inner["parent_session_id"], "p");
    }
}

#[tokio::test]
async fn compressed_request_body_passes_through_untouched() {
    let up = upstream(handler(|_| full(200, &[], "ok"))).await;
    let source = Arc::new(FakeSource {
        claude_identity: true,
        ..Arc::try_unwrap(FakeSource::new(&["a"])).ok().unwrap()
    });
    let h = harness(Provider::ChatGpt, source, up.addr, |_| {}).await;
    let payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    let resp = h
        .client
        .post(h.url("/backend-api/codex/responses"))
        .header("content-type", "application/json")
        .header("content-encoding", "zstd")
        .body(payload.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let r = &up.seen()[0];
    assert_eq!(r.headers["content-encoding"], "zstd");
    assert_eq!(&r.body[..], &payload[..]);
}

#[tokio::test]
async fn encoded_response_passes_through_untouched() {
    // Not valid gzip on purpose: the proxy must not try to decode it.
    let up = upstream(handler(|_| {
        full(
            200,
            &[
                ("content-encoding", "gzip"),
                ("content-type", "application/json"),
            ],
            "\u{1f}\u{8b}raw-bytes",
        )
    }))
    .await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h
        .client
        .post(h.url("/v1/messages"))
        .header("accept-encoding", "gzip, br")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.headers()["content-encoding"], "gzip");
    assert_eq!(resp.text().await.unwrap(), "\u{1f}\u{8b}raw-bytes");
    assert_eq!(up.seen()[0].headers["accept-encoding"], "gzip, br");
}

#[tokio::test]
async fn oversized_body_streams_once_without_failover() {
    let up = upstream(handler(|_| full(429, &quota_429_headers(), QUOTA_BODY))).await;
    let h = harness(
        Provider::Anthropic,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |c| {
            c.max_replay_body = 1024;
        },
    )
    .await;
    let big = "x".repeat(4096);
    let resp = h.post("/v1/messages", &big).await;
    assert_eq!(resp.status(), 429);
    let seen = up.seen();
    assert_eq!(seen.len(), 1, "not replayable, so not retried");
    assert_eq!(seen[0].body.len(), 4096);
    assert!(matches!(&h.source.reports()[0], (a, Outcome::LimitReached { .. }) if a == "a"));
}

#[tokio::test]
async fn no_account_at_all_is_a_clear_503() {
    let up = upstream(handler(|_| full(200, &[], "ok"))).await;
    let h = harness(Provider::Anthropic, FakeSource::new(&[]), up.addr, |_| {}).await;
    let resp = h.post("/v1/messages", "{}").await;
    assert_eq!(resp.status(), 503);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert!(up.seen().is_empty());
}

// --- WebSocket ---------------------------------------------------------------

/// A WebSocket echo server that records the handshake's Authorization.
#[allow(clippy::result_large_err)] // tungstenite's handshake callback signature
async fn ws_upstream() -> (SocketAddr, Arc<Mutex<Vec<HeaderMap>>>) {
    use tokio_tungstenite::tungstenite::handshake::server::{
        Request as WsRequest, Response as WsResponse,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let log = log.clone();
            tokio::spawn(async move {
                let callback = |req: &WsRequest, resp: WsResponse| {
                    log.lock().unwrap().push(req.headers().clone());
                    Ok(resp)
                };
                let mut ws = tokio_tungstenite::accept_hdr_async(stream, callback)
                    .await
                    .unwrap();
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_text() || msg.is_binary() {
                        ws.send(msg).await.unwrap();
                    } else if msg.is_close() {
                        break;
                    }
                }
            });
        }
    });
    (addr, seen)
}

fn ws_request(url: &str) -> http::Request<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url
        .replace("http://", "ws://")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer placeholder".parse().unwrap());
    request
}

#[tokio::test]
async fn websocket_tunnel_round_trip_with_swapped_auth() {
    use tokio_tungstenite::tungstenite::Message;
    let (addr, seen) = ws_upstream().await;
    let h = harness(Provider::ChatGpt, FakeSource::new(&["a"]), addr, |c| {
        c.refuse_websocket.clear();
    })
    .await;
    let (mut ws, response) =
        tokio_tungstenite::connect_async(ws_request(&h.url("/backend-api/codex/responses")))
            .await
            .unwrap();
    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    ws.send(Message::text("hello")).await.unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(echoed.into_text().unwrap().as_str(), "hello");
    let big = "y".repeat(200_000);
    ws.send(Message::text(big.clone())).await.unwrap();
    let echoed = ws.next().await.unwrap().unwrap();
    assert_eq!(echoed.into_text().unwrap().as_str(), big);
    ws.close(None).await.unwrap();

    let handshakes = seen.lock().unwrap().clone();
    assert_eq!(handshakes.len(), 1);
    assert_eq!(handshakes[0]["authorization"], "Bearer tok-a");
    assert_eq!(handshakes[0]["chatgpt-account-id"], "acct-a");
    assert!(matches!(&h.source.reports()[0], (a, Outcome::Ok { .. }) if a == "a"));
}

#[tokio::test]
async fn codex_websocket_is_refused_by_default() {
    let (addr, seen) = ws_upstream().await;
    let h = harness(Provider::ChatGpt, FakeSource::new(&["a"]), addr, |_| {}).await;
    let err = tokio_tungstenite::connect_async(ws_request(&h.url("/backend-api/codex/responses")))
        .await
        .unwrap_err();
    match err {
        tokio_tungstenite::tungstenite::Error::Http(resp) => {
            assert_eq!(resp.status(), StatusCode::UPGRADE_REQUIRED)
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(seen.lock().unwrap().is_empty());
}

// --- review fixes ------------------------------------------------------------

fn with_source(accounts: &[&'static str], tweak: impl FnOnce(&mut FakeSource)) -> Arc<FakeSource> {
    let mut source = Arc::try_unwrap(FakeSource::new(accounts)).ok().unwrap();
    tweak(&mut source);
    Arc::new(source)
}

#[tokio::test]
async fn a_refused_own_login_reaches_the_cli_unchanged() {
    let up = upstream(handler(|r| {
        match r.headers["authorization"].to_str().unwrap() {
            "Bearer cli-own" => full(
                401,
                &[("content-type", "application/json")],
                r#"{"error":{"message":"token expired","code":"token_expired"}}"#,
            ),
            _ => full(200, &[], "ok"),
        }
    }))
    .await;
    let source = with_source(&["live", "b"], |s| s.passthrough = Some("live"));
    let h = harness(Provider::ChatGpt, source, up.addr, |_| {}).await;
    let resp = h
        .client
        .post(h.url("/backend-api/codex/responses"))
        .header("authorization", "Bearer cli-own")
        .body("{}")
        .send()
        .await
        .unwrap();
    // Codex sees its own 401 and refreshes; no failover, no rejection.
    assert_eq!(resp.status(), 401);
    assert!(resp.text().await.unwrap().contains("token_expired"));
    assert_eq!(up.seen().len(), 1);
    assert!(h.source.reports().is_empty());
}

#[tokio::test]
async fn grok_never_sees_a_401_even_for_its_own_login() {
    let up = upstream(handler(|_| full(401, &[], ""))).await;
    let source = with_source(&["live"], |s| s.passthrough = Some("live"));
    let h = harness(Provider::Grok, source, up.addr, |_| {}).await;
    let resp = h.post("/v1/responses", "{}").await;
    assert_eq!(resp.status(), 503);
    assert!(h.source.reports().is_empty());
}

#[tokio::test]
async fn an_unexplained_403_is_passed_on_without_marking_the_account() {
    let up = upstream(handler(|_| {
        full(
            403,
            &[("content-type", "text/html")],
            "<html>Forbidden</html>",
        )
    }))
    .await;
    let h = harness(
        Provider::ChatGpt,
        FakeSource::new(&["a", "b"]),
        up.addr,
        |_| {},
    )
    .await;
    let resp = h.post("/backend-api/codex/responses", "{}").await;
    assert_eq!(resp.status(), 403);
    assert_eq!(resp.text().await.unwrap(), "<html>Forbidden</html>");
    assert_eq!(up.seen().len(), 1);
    assert!(h.source.reports().is_empty());
}

#[tokio::test]
async fn routes_only_reach_the_inference_endpoints() {
    let up = upstream(handler(|_| full(200, &[], "ok"))).await;
    let h = harness(Provider::ChatGpt, FakeSource::new(&["a"]), up.addr, |_| {}).await;
    for path in [
        "/backend-api/wham/usage",
        "/backend-api/accounts/check/v4-2023-04-27",
        "/v1/messages",
        "/backend-api/codex/../wham/usage",
        "/backend-api/codex/%2e%2e/wham/usage",
    ] {
        let resp = h.client.get(h.url(path)).send().await.unwrap();
        assert_eq!(resp.status(), 404, "{path}");
    }
    assert!(up.seen().is_empty());
    let resp = h
        .client
        .get(h.url("/backend-api/codex/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}

const SIGNATURE_400: &str = r#"{"error":{"type":"invalid_request_error","code":"invalid_encrypted_content","message":"The encrypted content could not be verified."}}"#;

#[tokio::test]
async fn a_history_refusal_retries_on_the_account_the_history_belongs_to() {
    // The chat runs on `b` after a failover; its history is `a`'s.
    let up = upstream(handler(|r| match r.account().as_str() {
        "b" => full(400, &[("content-type", "application/json")], SIGNATURE_400),
        _ => full(200, &[], "ok"),
    }))
    .await;
    let source = with_source(&["b", "a"], |s| s.history = HistoryRecovery::Retry);
    let h = harness(Provider::ChatGpt, source, up.addr, |_| {}).await;
    let mut events = h.proxy.subscribe();
    let resp = h.post("/backend-api/codex/responses", "{}").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        up.seen().iter().map(Recorded::account).collect::<Vec<_>>(),
        vec!["b", "a"]
    );
    assert_eq!(
        h.source.reports()[0],
        (
            "b".to_string(),
            Outcome::IncompatibleHistory { status: 400 }
        )
    );
    assert!(matches!(
        next_event(&mut events).await,
        ProxyEvent::AccountSwitched {
            reason: SwitchReason::HistoryBound,
            ..
        }
    ));
}

#[tokio::test]
async fn a_history_refusal_on_an_exhausted_owner_answers_with_a_native_limit() {
    let up = upstream(handler(|_| {
        full(400, &[("content-type", "application/json")], SIGNATURE_400)
    }))
    .await;
    let resets = chrono::Utc::now() + chrono::Duration::hours(3);
    let source = with_source(&["b", "a"], |s| {
        s.history = HistoryRecovery::LimitReached {
            resets_at: Some(resets),
            window: None,
        }
    });
    let h = harness(Provider::ChatGpt, source, up.addr, |_| {}).await;
    let resp = h.post("/backend-api/codex/responses", "{}").await;
    assert_eq!(resp.status(), 429);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["type"], "usage_limit_reached");
    assert_eq!(body["error"]["resets_at"], resets.timestamp());
    // Never retried, never looped.
    assert_eq!(up.seen().len(), 1);
}

#[tokio::test]
async fn a_history_refusal_is_recovered_at_most_once_per_request() {
    // Both accounts refuse: the second refusal goes to the CLI as is.
    let up = upstream(handler(|_| {
        full(400, &[("content-type", "application/json")], SIGNATURE_400)
    }))
    .await;
    let source = with_source(&["b", "a"], |s| s.history = HistoryRecovery::Retry);
    let h = harness(Provider::ChatGpt, source, up.addr, |_| {}).await;
    let resp = h.post("/backend-api/codex/responses", "{}").await;
    assert_eq!(resp.status(), 400);
    assert_eq!(up.seen().len(), 2);
}
