//! Real TCP/WebSocket impairment test. All listeners are loopback-only.
//! This models delayed bytes, a temporary blackout and a connection reset;
//! it does not claim to reproduce TCP packet loss or an airline network.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, watch};
use zeron_doc::RegistryDoc;
use zeron_sync::{RegistryClient, registry::mock_server::MockRegistryServer};

struct ImpairedProxy {
    url: String,
    pause: watch::Sender<bool>,
    reset: broadcast::Sender<()>,
    connections: Arc<AtomicU64>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ImpairedProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn forward<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: R,
    mut output: W,
    mut pause: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let mut bytes = [0; 4096];
    loop {
        let n = input.read(&mut bytes).await?;
        if n == 0 {
            return Ok(());
        }
        // Conservative store-and-forward model: 600 ms per chunk plus
        // serialization at 150,000 bytes/s (1.2 Mbit/s ceiling).
        tokio::time::sleep(
            Duration::from_millis(600) + Duration::from_secs_f64(n as f64 / 150_000.0),
        )
        .await;
        while *pause.borrow_and_update() {
            if pause.changed().await.is_err() {
                return Ok(());
            }
        }
        output.write_all(&bytes[..n]).await?;
    }
}

impl ImpairedProxy {
    async fn start(upstream: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let (pause, paused) = watch::channel(false);
        let (reset, _) = broadcast::channel(1);
        let resets = reset.clone();
        let connections = Arc::new(AtomicU64::new(0));
        let count = connections.clone();
        let task = tokio::spawn(async move {
            let mut sessions = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        let upstream = upstream.clone();
                        let paused = paused.clone();
                        let mut reset = resets.subscribe();
                        count.fetch_add(1, Ordering::Relaxed);
                        sessions.spawn(async move {
                            let peer = TcpStream::connect(upstream).await.unwrap();
                            let (read, write) = socket.into_split();
                            let (peer_read, peer_write) = peer.into_split();
                            tokio::select! {
                                _ = forward(read, peer_write, paused.clone()) => {},
                                _ = forward(peer_read, write, paused) => {},
                                _ = reset.recv() => {},
                            }
                        });
                    },
                    _ = sessions.join_next(), if !sessions.is_empty() => {},
                }
            }
        });
        Self {
            url,
            pause,
            reset,
            connections,
            task,
        }
    }
}

async fn until(mut predicate: impl FnMut() -> bool, bound: Duration) {
    tokio::time::timeout(bound, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("registry did not converge before the deadline");
}

#[tokio::test]
async fn delayed_registry_survives_blackout_and_reset_without_losing_writes() {
    let server = MockRegistryServer::start().await;
    let upstream = server
        .url()
        .strip_prefix("ws://")
        .unwrap()
        .trim_end_matches('/')
        .to_owned();
    let proxy = ImpairedProxy::start(upstream).await;
    let doc = Arc::new(std::sync::Mutex::new(RegistryDoc::new("impaired")));
    let client = RegistryClient::connect(&proxy.url, doc.clone(), "impaired")
        .await
        .unwrap();
    let started = std::time::Instant::now();
    proxy.pause.send(true).unwrap();
    {
        let mut doc = doc.lock().unwrap();
        for i in 0..20 {
            doc.upsert_device(&zeron_proto::Device {
                id: format!("offline-{i}"),
                name: format!("queued-{i}"),
                platform: "test".into(),
                last_seen_at: None,
                created_at: None,
                version: None,
                capabilities: Vec::new(),
            })
            .unwrap();
        }
    }
    client.nudge();
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(
        doc.lock().unwrap().pending_len(),
        20,
        "unacknowledged writes must stay queued"
    );
    assert_eq!(
        server.row_count(),
        0,
        "blackout must actually block delivery"
    );
    // Discard the old TCP stream and its in-flight bytes, then restore the
    // path. Recovery must replay the pending batches through a new socket.
    proxy.reset.send(()).unwrap();
    proxy.pause.send(false).unwrap();
    until(
        || doc.lock().unwrap().pending_len() == 0,
        Duration::from_secs(25),
    )
    .await;
    assert!(proxy.connections.load(Ordering::Relaxed) >= 2);
    assert_eq!(server.row_count(), 20);
    for i in 0..20 {
        assert!(server.row("devices", &format!("offline-{i}")).is_some());
    }
    println!(
        "impairment: 600ms/chunk/direction, 1.2Mbit/s ceiling, 30s blackout + TCP reset; 20/20 rows, pending=0, connections={}, convergence={:.3}s",
        proxy.connections.load(Ordering::Relaxed),
        started.elapsed().as_secs_f64()
    );
    client.shutdown().await;
}
