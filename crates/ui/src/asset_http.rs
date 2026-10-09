//! Public HTTP image transport for GPUI. Uses the app's Tokio runtime even when
//! GPUI requests an asset from its own executor; no GitHub credentials are sent.
//! Images come from untrusted PR text, so only public HTTPS hosts are reached,
//! redirects included: never plain HTTP, this machine, or the local network.
use futures::{FutureExt, StreamExt};
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, http::HeaderValue};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

fn public_ip(ip: IpAddr) -> bool {
    fn public_v4(ip: Ipv4Addr) -> bool {
        let [a, b, c, _] = ip.octets();
        !(ip.is_loopback()
            || ip.is_private()
            || ip.is_link_local()
            || ip.is_unspecified()
            || ip.is_broadcast()
            || ip.is_documentation()
            || ip.is_multicast()
            || a == 0
            // Shared address space (CGNAT, Tailscale).
            || (a == 100 && (64..128).contains(&b))
            // IETF protocol assignments and the 6to4 relay anycast.
            || (a == 192 && b == 0 && c == 0)
            || (a == 192 && b == 88 && c == 99)
            // Benchmarking.
            || (a == 198 && (18..20).contains(&b))
            // Reserved, including the limited broadcast.
            || a >= 240)
    }
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            let embedded = |high: u16, low: u16| {
                let [a, b] = high.to_be_bytes();
                let [c, d] = low.to_be_bytes();
                Ipv4Addr::new(a, b, c, d)
            };
            // IPv4-mapped (::ffff:a.b.c.d) and IPv4-compatible (::a.b.c.d)
            // addresses reach the IPv4 host they embed.
            if let Some(v4) = ip.to_ipv4() {
                return !ip.is_unspecified() && !ip.is_loopback() && public_v4(v4);
            }
            // NAT64 (64:ff9b::/96) and 6to4 (2002::/16) also embed one.
            if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                return public_v4(embedded(segments[6], segments[7]));
            }
            if segments[0] == 0x2002 {
                return public_v4(embedded(segments[1], segments[2]));
            }
            let first = segments[0];
            !(ip.is_multicast()
                // Unique local (fc00::/7), link-local (fe80::/10), and the
                // deprecated site-local (fec0::/10).
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                || (first & 0xffc0) == 0xfec0
                // Teredo (2001::/32) hides its IPv4 server; images never need it.
                || (first == 0x2001 && segments[1] == 0))
        }
    }
}

/// An HTTPS URL on a public host: no credentials, no loopback, private,
/// link-local, or shared (CGNAT/Tailscale) addresses, and no local names.
pub(crate) fn public_https(url: &url::Url) -> bool {
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

/// Resolves names to their public addresses only, so a public-looking name
/// cannot point the transport at this machine or the local network.
struct PublicDns;

impl reqwest::dns::Resolve for PublicDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addresses: Vec<SocketAddr> = tokio::net::lookup_host((name.as_str(), 0))
                .await?
                .filter(|address| public_ip(address.ip()))
                .collect();
            if addresses.is_empty() {
                return Err("no public address".into());
            }
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Hop {
    Follow,
    Stop,
    TooMany,
}

fn next_hop(public_only: bool, hops: usize, url: &url::Url) -> Hop {
    if hops >= MAX_REDIRECTS {
        Hop::TooMany
    } else if public_only && !public_https(url) {
        Hop::Stop
    } else {
        Hop::Follow
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
            match next_hop(public_only, attempt.previous().len(), attempt.url()) {
                Hop::Follow => attempt.follow(),
                Hop::Stop => attempt.stop(),
                Hop::TooMany => attempt.error("too many redirects"),
            }
        });
        let mut client = reqwest::Client::builder()
            .user_agent("Zeron/desktop")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            // A redirect never tells the next host which image linked to it.
            .referer(false)
            .redirect(redirects);
        if public_only {
            // Through a proxy the proxy resolves names, and an intranet name
            // would never meet `PublicDns`. Images always connect directly.
            client = client.dns_resolver(Arc::new(PublicDns)).no_proxy();
        }
        Arc::new(Self {
            client: client.build().expect("image HTTP client"),
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

    #[test]
    fn public_ip_refuses_every_local_reserved_and_embedded_private_range() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "192.0.0.8",
            "192.88.99.1",
            "::1",
            "::",
            "::ffff:127.0.0.1",
            "::127.0.0.1",
            "::10.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "2002:7f00:1::",
            "2002:c0a8:101::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "2001:0:4136:e378::1",
        ] {
            let ip: IpAddr = address.parse().unwrap();
            assert!(!public_ip(ip), "{address} must be refused");
        }
        for address in [
            "140.82.112.3",
            "185.199.108.133",
            "::ffff:140.82.112.3",
            "64:ff9b::8c52:7003",
            "2606:50c0:8000::153",
        ] {
            let ip: IpAddr = address.parse().unwrap();
            assert!(public_ip(ip), "{address} is public");
        }
    }

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
        let url = |text: &str| url::Url::parse(text).unwrap();
        // Every redirect hop is held to the same rule.
        assert_eq!(
            next_hop(true, 0, &url("https://avatars.githubusercontent.com/u/1")),
            Hop::Follow
        );
        assert_eq!(
            next_hop(true, 0, &url("http://169.254.169.254/latest")),
            Hop::Stop
        );
        assert_eq!(
            next_hop(true, MAX_REDIRECTS, &url("https://github.com/a.png")),
            Hop::TooMany
        );
        // A name that resolves to this machine has no usable address.
        use reqwest::dns::Resolve;
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        assert!(PublicDns.resolve(name).await.is_err());
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
