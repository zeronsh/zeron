//! Loopback listener, route table, and the account proxy handle.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::task::AbortHandle;

use crate::provider::Provider;
use crate::types::{AccountId, CredentialSource, ProxyEvent, RouteInfo};

/// Request bodies up to this size are buffered so a request can be replayed
/// on another account. Model requests are JSON of a few MB at most (a long
/// conversation with images); anything larger is streamed once, without
/// failover.
pub const DEFAULT_MAX_REPLAY_BODY: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// Must be a loopback address.
    pub bind: IpAddr,
    pub max_replay_body: usize,
    /// Error bodies are read up to this size to classify them.
    pub max_error_sniff: usize,
    /// How long to wait for an error body before classifying what arrived.
    pub error_sniff_timeout: Duration,
    pub connect_timeout: Duration,
    /// Longest silence tolerated between two upstream reads. Model streams
    /// send keep-alive pings, but extended thinking can be quiet for minutes.
    pub idle_read_timeout: Duration,
    /// Honor the system/env HTTP proxy for upstream connections, like the
    /// CLIs themselves do.
    pub system_proxy: bool,
    /// Replaces a provider's upstream base (scheme + host[:port]). The
    /// override's host becomes the only allowed host for that provider.
    /// Meant for tests against local fake servers.
    pub upstream_overrides: HashMap<Provider, String>,
    /// Providers whose WebSocket upgrades are answered with 426 instead of
    /// tunneled. Codex then streams over HTTP for the rest of the session,
    /// where a quota error can fail over; a tunneled socket is bound to one
    /// account until the CLI reconnects.
    pub refuse_websocket: Vec<Provider>,
    /// Pre-content hold for successful event streams (see the crate docs):
    /// at most this many events...
    pub sse_hold_events: usize,
    /// ...this many bytes...
    pub sse_hold_bytes: usize,
    /// ...and this long before the held bytes are released.
    pub sse_hold_timeout: Duration,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
            max_replay_body: DEFAULT_MAX_REPLAY_BODY,
            max_error_sniff: 256 * 1024,
            error_sniff_timeout: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(30),
            idle_read_timeout: Duration::from_secs(15 * 60),
            system_proxy: true,
            upstream_overrides: HashMap::new(),
            refuse_websocket: vec![Provider::ChatGpt],
            sse_hold_events: 48,
            sse_hold_bytes: 1024 * 1024,
            sse_hold_timeout: Duration::from_secs(10),
        }
    }
}

pub(crate) type TokenKey = [u8; 32];

pub(crate) struct RouteEntry {
    token: [u8; 32],
    pub(crate) info: RouteInfo,
    /// The account that served this route last; drives switch events.
    pub(crate) last_account: Mutex<Option<AccountId>>,
}

pub(crate) struct Inner {
    pub(crate) source: Arc<dyn CredentialSource>,
    pub(crate) config: ProxyConfig,
    pub(crate) addr: SocketAddr,
    routes: RwLock<HashMap<TokenKey, Arc<RouteEntry>>>,
    pub(crate) client: reqwest::Client,
    /// HTTP/1.1-only client for WebSocket upgrades.
    pub(crate) upgrade_client: reqwest::Client,
    pub(crate) events: broadcast::Sender<ProxyEvent>,
    accept_task: Mutex<Option<AbortHandle>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(task) = self
            .accept_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
    }
}

impl Inner {
    pub(crate) fn emit(&self, event: ProxyEvent) {
        let _ = self.events.send(event);
    }

    /// Finds the route for a presented token (64 lowercase hex chars).
    pub(crate) fn lookup(&self, token: &str) -> Option<Arc<RouteEntry>> {
        let presented = decode_token(token)?;
        let key = token_key(&presented);
        let entry = self
            .routes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()?;
        bool::from(entry.token.ct_eq(&presented)).then_some(entry)
    }

    pub(crate) fn upstream_base(&self, provider: Provider) -> (&str, Vec<String>) {
        match self.config.upstream_overrides.get(&provider) {
            Some(base) => {
                let host = reqwest::Url::parse(base)
                    .ok()
                    .and_then(|u| u.host_str().map(String::from))
                    .unwrap_or_default();
                (base.as_str(), vec![host])
            }
            None => (
                provider.upstream_base(),
                provider
                    .allowed_hosts()
                    .iter()
                    .map(|h| h.to_string())
                    .collect(),
            ),
        }
    }
}

/// The running proxy. Cheap to clone; the listener stops when the last clone
/// (and the last in-flight connection) is gone, or on [`AccountProxy::shutdown`].
#[derive(Clone)]
pub struct AccountProxy {
    inner: Arc<Inner>,
}

impl fmt::Debug for AccountProxy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountProxy")
            .field("addr", &self.inner.addr)
            .finish_non_exhaustive()
    }
}

impl AccountProxy {
    /// Binds an ephemeral loopback port and starts serving.
    pub async fn start(
        source: Arc<dyn CredentialSource>,
        config: ProxyConfig,
    ) -> std::io::Result<Self> {
        if !config.bind.is_loopback() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the account proxy only binds loopback addresses",
            ));
        }
        let listener = TcpListener::bind(SocketAddr::new(config.bind, 0)).await?;
        let addr = listener.local_addr()?;
        let client = build_client(&config, false).map_err(std::io::Error::other)?;
        let upgrade_client = build_client(&config, true).map_err(std::io::Error::other)?;
        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            source,
            config,
            addr,
            routes: RwLock::new(HashMap::new()),
            client,
            upgrade_client,
            events,
            accept_task: Mutex::new(None),
        });
        let task = tokio::spawn(accept_loop(listener, Arc::downgrade(&inner)));
        *inner.accept_task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task.abort_handle());
        tracing::info!(%addr, "account proxy listening");
        Ok(Self { inner })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.inner.addr
    }

    /// Mints a route for one harness process. The returned [`Route`] holds the
    /// unguessable token; dropping it unregisters the route.
    pub fn register(&self, info: RouteInfo) -> Route {
        let mut token = [0u8; 32];
        getrandom::fill(&mut token).expect("OS random source unavailable");
        let key = token_key(&token);
        let base_url = format!("http://{}/r/{}", self.inner.addr, encode_token(&token));
        let entry = Arc::new(RouteEntry {
            token,
            info: info.clone(),
            last_account: Mutex::new(None),
        });
        self.inner
            .routes
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, entry);
        Route {
            inner: Arc::downgrade(&self.inner),
            key,
            base_url,
            info,
        }
    }

    pub fn route_count(&self) -> usize {
        self.inner
            .routes
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }

    /// Account switches and exhaustion, for the engine/UI. Lagging receivers
    /// lose old events.
    pub fn subscribe(&self) -> broadcast::Receiver<ProxyEvent> {
        self.inner.events.subscribe()
    }

    /// Stops accepting connections. In-flight streams finish.
    pub fn shutdown(&self) {
        if let Some(task) = self
            .inner
            .accept_task
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            task.abort();
        }
    }
}

/// A registered route. Hand [`Route::base_url`] to the harness process (plus
/// the provider path prefix it expects); drop the route when the process
/// exits.
pub struct Route {
    inner: Weak<Inner>,
    key: TokenKey,
    base_url: String,
    info: RouteInfo,
}

impl Route {
    /// `http://127.0.0.1:<port>/r/<token>`, no trailing slash. Contains the
    /// route secret: never log it.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn info(&self) -> &RouteInfo {
        &self.info
    }
}

impl fmt::Debug for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Route")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.upgrade() {
            inner
                .routes
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.key);
        }
    }
}

fn token_key(token: &[u8; 32]) -> TokenKey {
    Sha256::digest(token).into()
}

fn encode_token(token: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for b in token {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn decode_token(s: &str) -> Option<[u8; 32]> {
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(out)
}

fn build_client(config: &ProxyConfig, http1_only: bool) -> reqwest::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        // Bytes pass through untouched: the CLI negotiated its own encoding.
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(config.connect_timeout)
        .read_timeout(config.idle_read_timeout)
        .tcp_keepalive(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(90));
    if http1_only {
        builder = builder.http1_only();
    }
    if !config.system_proxy {
        builder = builder.no_proxy();
    }
    builder.build()
}

async fn accept_loop(listener: TcpListener, inner: Weak<Inner>) {
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(conn) => conn,
            Err(err) => {
                tracing::warn!(%err, "account proxy accept failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let Some(inner) = inner.upgrade() else { return };
        let _ = stream.set_nodelay(true);
        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let inner = inner.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(crate::forward::handle(inner, req).await)
                }
            });
            let conn = http1::Builder::new()
                .timer(TokioTimer::new())
                // Hyper runs this timer while a keep-alive connection idles
                // between requests too; closing it there races the client's
                // next request on a pooled connection. Loopback only, so no
                // slow-header clients to guard against.
                .header_read_timeout(None)
                .keep_alive(true)
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            if let Err(err) = conn.await {
                tracing::debug!(%err, "account proxy connection ended with error");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trip() {
        let mut t = [0u8; 32];
        for (i, b) in t.iter_mut().enumerate() {
            *b = (i * 37) as u8;
        }
        assert_eq!(decode_token(&encode_token(&t)), Some(t));
        assert_eq!(decode_token("zz"), None);
        assert_eq!(decode_token(&"G".repeat(64)), None);
    }
}
