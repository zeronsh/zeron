//! A redialing client survives its server going away and coming back; a plain
//! `connect_ws` client does not.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use zeron_rpc::{
    RpcError, RpcReply, RpcService, connect_ws, connect_ws_redialing, serve_ws_listener,
};

struct Echo;

#[async_trait]
impl RpcService for Echo {
    async fn handle(&self, _method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        RpcReply::value(&params)
    }
}

/// An echo server on its own runtime. `serve_ws_listener` detaches a task per
/// connection, so aborting the accept loop would leave live connections
/// behind; dropping the whole runtime is what an engine exit looks like.
struct Server {
    port: u16,
    stop: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Server {
    fn start(port: u16) -> Self {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (stop, stop_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let listener = runtime
                .block_on(tokio::net::TcpListener::bind(("127.0.0.1", port)))
                .unwrap();
            ready_tx
                .send(listener.local_addr().unwrap().port())
                .unwrap();
            let service: Arc<dyn RpcService> = Arc::new(Echo);
            runtime.spawn(serve_ws_listener(listener, service));
            let _ = stop_rx.recv();
            // `runtime` drops here, cancelling the accept loop and every connection.
        });
        Self {
            port: ready_rx.recv().unwrap(),
            stop,
            thread,
        }
    }

    fn stop(self) {
        let _ = self.stop.send(());
        self.thread.join().unwrap();
    }
}

#[tokio::test]
async fn client_survives_a_server_restart() {
    let server = Server::start(0);
    let port = server.port;
    let client = connect_ws_redialing(&format!("ws://127.0.0.1:{port}"))
        .await
        .unwrap();
    let mut reconnected = client.reconnected();
    assert_eq!(
        client.call("echo", serde_json::json!(1)).await.unwrap(),
        serde_json::json!(1)
    );

    server.stop();
    // The call observes the closed transport rather than hanging.
    let during_gap = tokio::time::timeout(
        Duration::from_secs(5),
        client.call("echo", serde_json::json!(2)),
    )
    .await
    .expect("a call during the gap must not hang");
    assert!(matches!(during_gap, Err(RpcError::Closed)));

    let _server = Server::start(port);
    tokio::time::timeout(Duration::from_secs(10), reconnected.changed())
        .await
        .expect("client redials once the server is back")
        .unwrap();
    assert_eq!(
        client.call("echo", serde_json::json!(3)).await.unwrap(),
        serde_json::json!(3)
    );
}

#[tokio::test]
async fn plain_connect_ws_still_dies_with_its_transport() {
    let server = Server::start(0);
    let port = server.port;
    let client = connect_ws(&format!("ws://127.0.0.1:{port}")).await.unwrap();
    server.stop();
    let _server = Server::start(port);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(matches!(
        client.call("echo", serde_json::json!(1)).await,
        Err(RpcError::Closed)
    ));
}
