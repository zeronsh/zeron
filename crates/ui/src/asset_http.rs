//! Public HTTP image transport for GPUI. Uses the app's Tokio runtime even when
//! GPUI requests an asset from its own executor; no GitHub credentials are sent.
//! Images come from untrusted PR text, so only public HTTPS hosts are reached,
//! redirects included: never plain HTTP, this machine, or the local network.
use futures::{FutureExt, StreamExt};
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, http::HeaderValue};
use std::{sync::Arc, time::Duration};

const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

/// An HTTPS URL on a public host: no credentials, no loopback, private,
/// link-local, or shared (CGNAT/Tailscale) addresses, and no local names.
/// Names that resolve to private addresses are not detected.
pub(crate) fn public_https(url: &url::Url) -> bool {
    use std::net::{IpAddr, Ipv4Addr};
    fn public_v4(ip: Ipv4Addr) -> bool {
        let [a, b, ..] = ip.octets();
        !(ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_documentation()
            || a == 0
            || (a == 100 && (64..128).contains(&b)))
    }
    fn public_ip(ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(ip) => public_v4(ip),
            IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
                Some(ip) => public_v4(ip),
                None => {
                    let first = ip.segments()[0];
                    !(ip.is_loopback()
                        || ip.is_unspecified()
                        || (first & 0xfe00) == 0xfc00
                        || (first & 0xffc0) == 0xfe80)
                }
            },
        }
    }
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && match url.host() {
            Some(url::Host::Domain(name)) => {
                let name = name.trim_end_matches('.').to_ascii_lowercase();
                name.contains('.')
                    && !name.ends_with(".localhost")
                    && !name.ends_with(".local")
                    && !name.ends_with(".internal")
            }
            Some(url::Host::Ipv4(ip)) => public_ip(ip.into()),
            Some(url::Host::Ipv6(ip)) => public_ip(ip.into()),
            None => false,
        }
}

#[derive(Clone)]
pub(crate) struct AssetHttpClient {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
    public_only: bool,
}

impl AssetHttpClient {
    pub(crate) fn new(runtime: tokio::runtime::Handle) -> Arc<Self> {
        Self::with_policy(runtime, true)
    }

    /// `public_only: false` lets tests reach a loopback server.
    fn with_policy(runtime: tokio::runtime::Handle, public_only: bool) -> Arc<Self> {
        let redirects = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if public_only && !public_https(attempt.url()) {
                attempt.stop()
            } else {
                attempt.follow()
            }
        });
        Arc::new(Self {
            client: reqwest::Client::builder()
                .user_agent("Zeron/desktop")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .redirect(redirects)
                .build()
                .expect("image HTTP client"),
            runtime,
            public_only,
        })
    }
}

impl HttpClient for AssetHttpClient {
    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let public_only = self.public_only;
        let task = self.runtime.spawn(async move {
            anyhow::ensure!(
                request.method() == "GET",
                "Asset transport only supports GET"
            );
            let url = url::Url::parse(&request.uri().to_string())?;
            anyhow::ensure!(
                !public_only || public_https(&url),
                "Images load only from public HTTPS hosts"
            );
            let response = client
                .get(url)
                .headers(request.headers().clone())
                .send()
                .await?;
            anyhow::ensure!(
                response.content_length().unwrap_or(0) <= MAX_ASSET_BYTES as u64,
                "Image exceeds 16 MiB"
            );
            let status = response.status();
            let headers = response.headers().clone();
            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                anyhow::ensure!(
                    bytes.len() + chunk.len() <= MAX_ASSET_BYTES,
                    "Image exceeds 16 MiB"
                );
                bytes.extend_from_slice(&chunk);
            }
            let mut result = Response::builder().status(status).body(bytes.into())?;
            *result.headers_mut() = headers;
            Ok(result)
        });
        async move { task.await? }.boxed()
    }

    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }
    fn proxy(&self) -> Option<&url::Url> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::AsyncReadExt;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt};

    #[tokio::test]
    async fn pull_request_production_asset_transport_follows_redirects_and_bounds_downloads() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let base = origin.clone();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let len = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..len]);
                assert!(!request.to_lowercase().contains("authorization:"));
                let response = if request.starts_with("GET /redirect ") {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: {base}/image\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                } else if request.starts_with("GET /large ") {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_ASSET_BYTES + 1
                    )
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 4\r\nConnection: close\r\n\r\nPNG!".into()
                };
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let client = AssetHttpClient::with_policy(tokio::runtime::Handle::current(), false);
        let mut response = client
            .get(&format!("{origin}/redirect"), ().into(), true)
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "image/png");
        let mut bytes = Vec::new();
        response.body_mut().read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"PNG!");
        assert!(
            client
                .get(&format!("{origin}/large"), ().into(), true)
                .await
                .is_err()
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn pull_request_asset_transport_refuses_local_and_plain_http_targets() {
        for (url, public) in [
            ("https://github.com/a.png", true),
            ("https://user-images.githubusercontent.com/1/a.png", true),
            ("https://8.8.8.8/a.png", true),
            ("http://example.com/a.png", false),
            ("https://user:pass@example.com/a.png", false),
            ("https://localhost/a.png", false),
            ("https://printer.local/a.png", false),
            ("https://127.0.0.1/a.png", false),
            ("https://10.0.0.1/a.png", false),
            ("https://192.168.1.1/a.png", false),
            ("https://169.254.169.254/a.png", false),
            ("https://100.100.1.1/a.png", false),
            ("https://[::1]/a.png", false),
            ("https://[fd00::1]/a.png", false),
            ("https://[::ffff:127.0.0.1]/a.png", false),
        ] {
            assert_eq!(
                public_https(&url::Url::parse(url).unwrap()),
                public,
                "{url}"
            );
        }
        // Enforced by the transport too, before any connection is made.
        let client = AssetHttpClient::new(tokio::runtime::Handle::current());
        assert!(
            client
                .get("http://127.0.0.1:9/a.png", ().into(), true)
                .await
                .is_err()
        );
    }
}
