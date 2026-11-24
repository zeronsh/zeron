//! Loopback MCP Gateway implementing [`zeron_mcp::http::ExternalMcpProxy`].
//!
//! Forwards authenticated `/mcp/server/{server_id}` requests to the configured
//! upstream remote MCP server with managed credentials injected by the engine,
//! streaming responses (including SSE) with zero buffering and performing one
//! automatic token refresh + retry on upstream `401 Unauthorized`.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Method, StatusCode};

use zeron_mcp::http::{ExternalMcpProxy, ExternalProxyRequest, ExternalProxyResponse};

use super::{McpServers, ResolvedUpstream};

/// Safe client headers forwarded to upstream MCP servers.
const FORWARDED_CLIENT_HEADERS: [&str; 5] = [
    "content-type",
    "accept",
    "mcp-protocol-version",
    "mcp-session-id",
    "last-event-id",
];

/// `Authorization: bearer …` (a token endpoint's own `token_type`, kept as
/// it said it) as `Bearer …`. The scheme is case-insensitive (RFC 9110), but
/// servers that match `Bearer ` exactly (Linear's) take the lower-case one for
/// no credential at all and answer 401.
fn canonical_bearer(value: &str) -> std::borrow::Cow<'_, str> {
    match value.split_once(' ') {
        Some((scheme, token)) if scheme.eq_ignore_ascii_case("bearer") && scheme != "Bearer" => {
            std::borrow::Cow::Owned(format!("Bearer {token}"))
        }
        _ => std::borrow::Cow::Borrowed(value),
    }
}

/// A URL's origin alone, with nothing of its path or query (where some
/// services put a key).
pub(super) fn url_origin(url: &str) -> Option<String> {
    let origin = reqwest::Url::parse(url).ok()?.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    )
}

/// Loopback gateway forwarding external MCP traffic with managed credentials.
pub struct McpGateway {
    servers: Arc<McpServers>,
    /// Dedicated HTTP client with no overall request timeout so long-lived SSE
    /// streams (`GET /mcp/server/{id}` or streaming `POST`) never time out.
    http: reqwest::Client,
}

impl McpGateway {
    pub fn new(servers: Arc<McpServers>) -> Self {
        let http = reqwest::Client::builder()
            // A server's credentials go to that server: a redirect off its
            // origin isn't followed with them (reqwest keeps custom headers,
            // an `X-Api-Key`, on one).
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let same = attempt
                    .previous()
                    .first()
                    .is_some_and(|first| first.origin() == attempt.url().origin());
                if same && attempt.previous().len() <= 5 { attempt.follow() } else { attempt.stop() }
            }))
            .connect_timeout(Duration::from_secs(30))
            .user_agent(concat!("zeron/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { servers, http }
    }

    async fn send_upstream(
        &self,
        upstream: &ResolvedUpstream,
        method: &Method,
        client_headers: &HeaderMap,
        body: &Bytes,
    ) -> Result<reqwest::Response, String> {
        let reqwest_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
            .map_err(|e| format!("unsupported HTTP method {method}: {e}"))?;
        let mut builder = self.http.request(reqwest_method, &upstream.url);

        for name in FORWARDED_CLIENT_HEADERS {
            if let Some(value) = client_headers.get(name)
                && let Ok(val_str) = value.to_str()
            {
                builder = builder.header(name, val_str);
            }
        }

        // Injected auth / custom headers always override client headers.
        for (name, value) in &upstream.headers {
            let value = if name.eq_ignore_ascii_case("authorization") {
                canonical_bearer(value)
            } else {
                std::borrow::Cow::Borrowed(value.as_str())
            };
            builder = builder.header(name.as_str(), value.as_ref());
        }

        if !body.is_empty() {
            builder = builder.body(body.clone());
        }

        builder
            .send()
            .await
            .map_err(|e| format!("upstream MCP request failed: {e}"))
    }
}

impl ExternalMcpProxy for McpGateway {
    fn forward(
        &self,
        req: ExternalProxyRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ExternalProxyResponse, String>> + Send>> {
        let servers = self.servers.clone();
        let http = self.http.clone();
        Box::pin(async move {
            let gateway = McpGateway { servers, http };
            let upstream = gateway
                .servers
                .resolve_upstream(&req.server_id, false)
                .await?;

            let mut resp = gateway
                .send_upstream(&upstream, &req.method, &req.headers, &req.body)
                .await?;

            // If upstream returns 401 for an OAuth server, force a single refresh
            // and retry once with the fresh token.
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED && upstream.is_oauth {
                tracing::debug!(
                    server_id = %req.server_id,
                    "mcp gateway: upstream returned 401; forcing OAuth refresh and retrying once"
                );
                if let Ok(refreshed) = gateway
                    .servers
                    .resolve_upstream(&req.server_id, true)
                    .await
                {
                    resp = gateway
                        .send_upstream(&refreshed, &req.method, &req.headers, &req.body)
                        .await?;
                }
            }

            if !resp.status().is_success() {
                // What the upstream said of the credential (never the
                // credential): its WWW-Authenticate, and the shape of what
                // was sent.
                let challenge = resp
                    .headers()
                    .get(reqwest::header::WWW_AUTHENTICATE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                let (scheme, token_len) = upstream
                    .headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                    .map(|(_, value)| match value.split_once(' ') {
                        Some((scheme, token)) => (scheme.to_owned(), token.len()),
                        None => (String::new(), value.len()),
                    })
                    .unwrap_or_default();
                tracing::warn!(
                    server_id = %req.server_id,
                    method = %req.method,
                    status = resp.status().as_u16(),
                    challenge = %challenge,
                    auth_scheme = %scheme,
                    token_len,
                    // (Its origin: a key may be in the rest of it.)
                    origin = %url_origin(&upstream.url).unwrap_or_default(),
                    "mcp gateway: upstream answered with an error"
                );
            }
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let mut headers = HeaderMap::new();
            for (name, value) in resp.headers() {
                let lower = name.as_str().to_ascii_lowercase();
                if is_hop_by_hop(&lower) {
                    continue;
                }
                if let (Ok(h_name), Ok(h_val)) = (
                    HeaderName::from_bytes(name.as_str().as_bytes()),
                    HeaderValue::from_bytes(value.as_bytes()),
                ) {
                    headers.append(h_name, h_val);
                }
            }

            // Stream upstream bytes directly into ExternalProxyResponse with zero buffering.
            let stream = resp.bytes_stream().filter_map(|chunk| async move {
                match chunk {
                    Ok(bytes) => Some(Ok::<Frame<Bytes>, Infallible>(Frame::data(bytes))),
                    Err(err) => {
                        tracing::debug!(error = %err, "mcp gateway: upstream stream closed with error");
                        None
                    }
                }
            });
            let body = BodyExt::boxed_unsync(StreamBody::new(stream));

            Ok(ExternalProxyResponse {
                status,
                headers,
                body,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::canonical_bearer;

    #[test]
    fn bearer_scheme_is_sent_capitalised() {
        assert_eq!(canonical_bearer("bearer abc.def"), "Bearer abc.def");
        assert_eq!(canonical_bearer("BEARER abc"), "Bearer abc");
        assert_eq!(canonical_bearer("Bearer abc"), "Bearer abc");
        // Another scheme, or no scheme, stays as it is.
        assert_eq!(canonical_bearer("Basic dXNlcjpwYXNz"), "Basic dXNlcjpwYXNz");
        assert_eq!(canonical_bearer("token-only"), "token-only");
    }
}
