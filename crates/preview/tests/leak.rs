//! Remote-preview churn must not accumulate tasks, streams, or memory. This
//! drives many mixed requests (small, echo, partially consumed then dropped,
//! WebSocket) through the proxy on the viewing device over a real WebRTC pair
//! to a backend on the hosting device and compares live tasks and RSS
//! against the warmed-up baseline.
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, StreamBody, combinators::UnsyncBoxBody};
use hyper::{
    Request, Response,
    body::{Frame, Incoming},
};
use hyper_util::rt::TokioIo;
use std::{
    convert::Infallible,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest, protocol::Role},
};
use tokio_util::sync::CancellationToken;
use zeron_preview::{
    catalog::Catalog,
    discovery::Listener,
    mux::{self, BoxIo, Connector},
    peer::Peers,
    proxy::{self, Router},
};
type Body = UnsyncBoxBody<Bytes, hyper::Error>;
fn full(value: impl Into<Bytes>) -> Body {
    Full::new(value.into())
        .map_err(|never: Infallible| match never {})
        .boxed_unsync()
}
struct Count(Arc<AtomicUsize>);
impl Drop for Count {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn server(stop: CancellationToken, active: Arc<AtomicUsize>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let socket = tokio::select! { _ = stop.cancelled() => break, value = listener.accept() => value.unwrap().0 };
            active.fetch_add(1, Ordering::SeqCst);
            let guard = Count(active.clone());
            let stop = stop.clone();
            tokio::spawn(async move {
                let _guard = guard;
                let service =
                    hyper::service::service_fn(move |mut request: Request<Incoming>| async move {
                        let response = match request.uri().path() {
                            "/echo" => Response::new(request.into_body().boxed_unsync()),
                            "/infinite" => {
                                let body = futures::stream::unfold(0u64, move |index| async move {
                                    tokio::time::sleep(Duration::from_millis(5)).await;
                                    Some((
                                        Ok::<_, hyper::Error>(Frame::data(Bytes::from(vec![
                                            42;
                                            8192
                                        ]))),
                                        index + 1,
                                    ))
                                });
                                Response::new(StreamBody::new(body).boxed_unsync())
                            }
                            "/ws" => {
                                let accept =
                                    tokio_tungstenite::tungstenite::handshake::derive_accept_key(
                                        request.headers()["sec-websocket-key"].as_bytes(),
                                    );
                                let upgrade = hyper::upgrade::on(&mut request);
                                tokio::spawn(async move {
                                    let Ok(upgraded) = upgrade.await else { return };
                                    let mut socket = WebSocketStream::from_raw_socket(
                                        TokioIo::new(upgraded),
                                        Role::Server,
                                        None,
                                    )
                                    .await;
                                    while let Some(Ok(message)) = socket.next().await {
                                        if message.is_close() {
                                            let _ = socket.flush().await;
                                            break;
                                        }
                                        if (message.is_text() || message.is_binary())
                                            && socket.send(message).await.is_err()
                                        {
                                            break;
                                        }
                                    }
                                });
                                Response::builder()
                                    .status(101)
                                    .header("connection", "upgrade")
                                    .header("upgrade", "websocket")
                                    .header("sec-websocket-accept", accept)
                                    .body(full(""))
                                    .unwrap()
                            }
                            _ => Response::new(full(format!("server:{port}"))),
                        };
                        Ok::<_, Infallible>(response)
                    });
                tokio::select! { _ = stop.cancelled() => {}, _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).with_upgrades() => {} }
            });
        }
    });
    port
}
struct Backend(Catalog);
#[async_trait::async_trait]
impl Connector for Backend {
    async fn connect(&self, id: &str) -> anyhow::Result<BoxIo> {
        let route = self
            .0
            .local_route(id)
            .ok_or_else(|| anyhow::anyhow!("unknown service"))?;
        Ok(Box::new(TcpStream::connect(route.listener.address).await?))
    }
}
struct NoLocal;
#[async_trait::async_trait]
impl Connector for NoLocal {
    async fn connect(&self, _: &str) -> anyhow::Result<BoxIo> {
        anyhow::bail!("viewer has no local services")
    }
}
fn rss_kb() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    pages * 4
}
fn alive_tasks() -> usize {
    tokio::runtime::Handle::current()
        .metrics()
        .num_alive_tasks()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_preview_churn_does_not_accumulate_tasks_or_memory() {
    let iterations: usize = std::env::var("PREVIEW_LEAK_ITERATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let temp = tempfile::tempdir().unwrap();
    let stop = CancellationToken::new();
    let active = Arc::new(AtomicUsize::new(0));
    let backend_port = server(stop.child_token(), active.clone()).await;

    // Hosting device "a" advertises the backend; viewing device "b" proxies.
    let host_catalog =
        Catalog::open(temp.path().join("a.json"), "a".into(), "Desk".into()).unwrap();
    host_catalog
        .replace_local(vec![(
            "/work/app".into(),
            Listener {
                pid: 1,
                parent: 1,
                cwd: "/work/app".into(),
                args: vec!["node".into(), "vite".into()],
                started_at: 1,
                address: ([127, 0, 0, 1], backend_port).into(),
                zeron_owned: true,
            },
        )])
        .unwrap();
    let (mut a, mut a_out) = Peers::new(
        "a".into(),
        Arc::new(Backend(host_catalog.clone())),
        stop.clone(),
    );
    a.set_ice_servers(Vec::new()).unwrap();
    let viewer_catalog =
        Catalog::open(temp.path().join("b.json"), "b".into(), "Laptop".into()).unwrap();
    viewer_catalog
        .set_remote("a", host_catalog.local_services())
        .unwrap();
    let (mut b, mut b_out) = Peers::new("b".into(), Arc::new(NoLocal), stop.clone());
    b.set_ice_servers(Vec::new()).unwrap();
    let peer_b = b.clone();
    tokio::spawn(async move {
        while let Some(message) = a_out.recv().await {
            let _ = peer_b.signal("a", message.signal).await;
        }
    });
    let peer_a = a.clone();
    tokio::spawn(async move {
        while let Some(message) = b_out.recv().await {
            let _ = peer_a.signal("b", message.signal).await;
        }
    });
    let local = mux::local(Arc::new(NoLocal), stop.clone());
    let (port, _listeners) = proxy::serve(
        Router {
            catalog: viewer_catalog.clone(),
            local,
            peers: b.clone(),
        },
        0,
        stop.clone(),
    )
    .await
    .unwrap();
    viewer_catalog.set_proxy_status(port, None);
    let service = viewer_catalog.snapshot().services[0].clone();
    assert_eq!(service.device_id, "a");
    let url = service.url(port);
    let client = reqwest::Client::builder()
        .no_proxy()
        .resolve(&service.hostname, ([127, 0, 0, 1], port).into())
        .build()
        .unwrap();

    let round = |client: reqwest::Client, url: String, hostname: String| async move {
        let text = client.get(&url).send().await.unwrap().text().await.unwrap();
        assert!(text.starts_with("server:"), "{text}");
        let body = vec![9u8; 64 * 1024 + 3];
        let echoed = client
            .post(format!("{url}/echo"))
            .body(body.clone())
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(echoed.len(), body.len());
        // A page navigated away mid-download: body dropped after one chunk.
        let mut infinite = client
            .get(format!("{url}/infinite"))
            .send()
            .await
            .unwrap()
            .bytes_stream();
        infinite.next().await.unwrap().unwrap();
        drop(infinite);
        // A fresh connection abandoned before its response headers arrive.
        let mut early = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut early,
            format!("GET /infinite HTTP/1.1\r\nHost: {hostname}:{port}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        drop(early);
        let request = format!("ws://{hostname}:{port}/ws")
            .into_client_request()
            .unwrap();
        let tcp = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        let (mut ws, _) = tokio_tungstenite::client_async(request, tcp).await.unwrap();
        ws.send(Message::Text("hmr".into())).await.unwrap();
        assert!(ws.next().await.unwrap().unwrap().is_text());
        ws.close(None).await.unwrap();
        let _ = ws.next().await;
    };
    let hostname = service.hostname.clone();
    let quiesce = |active: Arc<AtomicUsize>| async move {
        tokio::time::timeout(Duration::from_secs(10), async {
            while active.load(Ordering::SeqCst) > 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("backend connections never drained");
        tokio::time::sleep(Duration::from_millis(300)).await;
    };

    // Warm up: pairs the WebRTC peer and fills connection pools.
    for _ in 0..3 {
        round(client.clone(), url.clone(), hostname.clone()).await;
    }
    quiesce(active.clone()).await;
    let tasks_before = alive_tasks();
    let rss_before = rss_kb();
    for i in 0..iterations {
        round(client.clone(), url.clone(), hostname.clone()).await;
        if (i + 1) % 50 == 0 {
            quiesce(active.clone()).await;
            eprintln!(
                "iteration {}: alive tasks {} (baseline {}), rss {} KiB (baseline {})",
                i + 1,
                alive_tasks(),
                tasks_before,
                rss_kb(),
                rss_before
            );
        }
    }
    quiesce(active.clone()).await;
    let tasks_after = alive_tasks();
    let rss_after = rss_kb();
    eprintln!("tasks {tasks_before} -> {tasks_after}; rss {rss_before} -> {rss_after} KiB");
    assert!(
        tasks_after <= tasks_before + 4,
        "tasks leaked across {iterations} rounds: {tasks_before} -> {tasks_after}"
    );
    // Re-pairing churn: a laptop that sleeps, roams networks, or loses the
    // coordinator lease tears the peer down and pairs again. Closed peers
    // must release their WebRTC state.
    let churn: usize = std::env::var("PREVIEW_LEAK_CHURN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    for i in 0..churn {
        b.remove("a").await;
        round(client.clone(), url.clone(), hostname.clone()).await;
        if (i + 1) % 4 == 0 {
            quiesce(active.clone()).await;
            eprintln!(
                "re-pair {}: alive tasks {} (baseline {}), rss {} KiB (baseline {})",
                i + 1,
                alive_tasks(),
                tasks_before,
                rss_kb(),
                rss_before
            );
        }
    }
    quiesce(active.clone()).await;
    let tasks_churned = alive_tasks();
    eprintln!(
        "after {churn} re-pairs: tasks {tasks_before} -> {tasks_churned}; rss {} KiB",
        rss_kb()
    );
    assert!(
        tasks_churned <= tasks_before + 4,
        "tasks leaked across {churn} re-pairs: {tasks_before} -> {tasks_churned}"
    );
    stop.cancel();
    a.clear().await;
    b.clear().await;
}
