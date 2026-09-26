//! Public HTTP image transport for GPUI. Uses the app's Tokio runtime even when
//! GPUI requests an asset from its own executor; no GitHub credentials are sent.
use futures::{FutureExt, StreamExt};
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, http::HeaderValue};
use std::{sync::Arc, time::Duration};

const MAX_ASSET_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct AssetHttpClient {
    client: reqwest::Client,
    runtime: tokio::runtime::Handle,
}

impl AssetHttpClient {
    pub(crate) fn new(runtime: tokio::runtime::Handle) -> Arc<Self> {
        Arc::new(Self {
            client: reqwest::Client::builder()
                .user_agent("Zeron/desktop")
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()
                .expect("image HTTP client"),
            runtime,
        })
    }
}

impl HttpClient for AssetHttpClient {
    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let client = self.client.clone();
        let task = self.runtime.spawn(async move {
            anyhow::ensure!(
                request.method() == "GET",
                "Asset transport only supports GET"
            );
            let response = client
                .get(request.uri().to_string())
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
        let client = AssetHttpClient::new(tokio::runtime::Handle::current());
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
}
