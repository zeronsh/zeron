use std::{sync::Arc, time::Duration};

use futures::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, client::IntoClientRequest},
};
use zeron_private::{Hub, HubHandle, NodeRole, PrivateConfig};
use zeron_rpc::{DeviceFrameHeader, decode_device_frame, encode_device_frame};
use zeron_sync::chat_frames::{self, frame_type as ft};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Fixture {
    directory: TempDir,
    config: PrivateConfig,
    hub: Arc<Hub>,
    handle: HubHandle,
    client: reqwest::Client,
}
impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut config = PrivateConfig::create(
            directory.path(),
            "Test workspace",
            "server-a",
            "Server A",
            NodeRole::Server,
            "http://127.0.0.1:27655",
        )
        .unwrap();
        config.listen_port = 0;
        config.save(directory.path()).unwrap();
        let hub = Hub::open(directory.path(), &config).unwrap();
        let handle = hub.clone().start().await.unwrap();
        Self {
            directory,
            config,
            hub,
            handle,
            client: reqwest::Client::new(),
        }
    }
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.handle.local_addr())
    }
    async fn pair(&self, id: &str, role: NodeRole) -> String {
        let invitation = self.hub.create_invitation(role).unwrap();
        let response = self
            .client
            .post(self.url("/private/pair"))
            .json(&json!({"code":invitation.code,"name":id,"deviceId":id,"role":role}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let value: Value = response.json().await.unwrap();
        assert_eq!(value["workspaceId"], self.config.workspace_id);
        assert_eq!(value["userId"], self.config.user_id);
        assert_eq!(value["deviceId"], id);
        value["token"].as_str().unwrap().into()
    }
    async fn socket(&self, path: &str, token: &str) -> Socket {
        socket(&self.url(path), token).await
    }
    async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(self.url(path))
            .bearer_auth(&self.config.token)
            .send()
            .await
            .unwrap()
    }
}
async fn socket(url: &str, token: &str) -> Socket {
    let mut request = url.replacen("http", "ws", 1).into_client_request().unwrap();
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {token}").parse().unwrap());
    tokio_tungstenite::connect_async(request).await.unwrap().0
}
async fn receive(socket: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .expect("socket response timed out")
        .expect("socket ended")
        .unwrap()
}
async fn text(socket: &mut Socket, value: Value) {
    socket.send(Message::Text(value.to_string())).await.unwrap();
}
async fn json_frame(socket: &mut Socket) -> Value {
    serde_json::from_str(receive(socket).await.to_text().unwrap()).unwrap()
}
async fn chat_frame(socket: &mut Socket) -> chat_frames::WireFrame {
    chat_frames::decode(&receive(socket).await.into_data()).unwrap()
}
fn registry_path(f: &Fixture, action: &str) -> String {
    format!("/registry/{}/{action}", f.config.workspace_id)
}
fn batch(id: &str, name: &str) -> Value {
    json!({"batch":id,"ops":[{"kind":"spaces","id":"space","op":"upsert","hlc":"1777777777777-000001-server-a","set":{"name":name}}]})
}

#[tokio::test]
async fn enrollment_is_single_use_and_requires_header_credentials() {
    let f = Fixture::new().await;
    let info: Value = f
        .client
        .get(f.url("/private/info"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["protocolVersion"], 1);
    let path = registry_path(&f, "rows");
    assert_eq!(
        f.client.get(f.url(&path)).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.client
            .get(f.url(&format!("{path}?token={}", f.config.token)))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let invitation = f.hub.create_invitation(NodeRole::Client).unwrap();
    let request =
        json!({"code":invitation.code,"name":"Client","deviceId":"client-a","role":"client"});
    let first = f
        .client
        .post(f.url("/private/pair"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let second = f
        .client
        .post(f.url("/private/pair"))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(f.hub.nodes().unwrap().len(), 2);
    let invitation = f.hub.create_invitation(NodeRole::Client).unwrap();
    for _ in 0..5 {
        let response = f
            .client
            .post(f.url("/private/pair"))
            .json(&json!({"code":"wrong","name":"Client","deviceId":"client-b","role":"client"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = f
        .client
        .post(f.url("/private/pair"))
        .json(
            &json!({"code":invitation.code,"name":"Client","deviceId":"client-b","role":"client"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(!format!("{:?}", f.config).contains(&f.config.token));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(PrivateConfig::path(f.directory.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn registry_websockets_and_http_merge_once_and_survive_restart() {
    let mut f = Fixture::new().await;
    let client_token = f.pair("client-a", NodeRole::Client).await;
    let mut a = f.socket(&registry_path(&f, "ws"), &f.config.token).await;
    let mut b = f.socket(&registry_path(&f, "ws"), &client_token).await;
    text(&mut a, json!({"t":"hello","cursor":0,"device":"server-a"})).await;
    text(&mut b, json!({"t":"hello","cursor":0,"device":"client-a"})).await;
    assert_eq!(json_frame(&mut a).await["seq"], 0);
    assert_eq!(json_frame(&mut b).await["seq"], 0);
    let mut push = batch("one", "Shared project");
    push["t"] = json!("push");
    text(&mut a, push.clone()).await;
    assert_eq!(
        json_frame(&mut a).await["rows"][0]["fields"]["name"],
        "Shared project"
    );
    assert_eq!(
        json_frame(&mut a).await,
        json!({"t":"ack","batch":"one","seq":1,"applied":1})
    );
    assert_eq!(json_frame(&mut b).await["seq"], 1);
    let response: Value = f
        .client
        .post(f.url(&registry_path(&f, "push")))
        .bearer_auth(&f.config.token)
        .json(&push)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["applied"], 0);
    text(&mut b, json!({"t":"presence","at":0})).await;
    let presence = json_frame(&mut a).await;
    assert_eq!(presence["device"], "client-a");
    assert!(presence["at"].as_i64().unwrap() > 0);
    assert_eq!(
        f.get(&format!("{}?device=client-a", registry_path(&f, "rows")))
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut invalid = batch("bad", "Rejected");
    invalid["ops"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"spaces","id":"other","op":"upsert","hlc":"bad","set":{"name":"no"}}));
    assert_eq!(
        f.client
            .post(f.url(&registry_path(&f, "push")))
            .bearer_auth(&f.config.token)
            .json(&invalid)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    f.handle.shutdown();
    let restarted = Hub::open(f.directory.path(), &f.config).unwrap();
    f.handle = restarted.clone().start().await.unwrap();
    f.hub = restarted;
    let rows: Value = f
        .get(&registry_path(&f, "rows"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(rows["seq"], 1);
    assert_eq!(rows["rows"].as_array().unwrap().len(), 1);
    assert_eq!(rows["rows"][0]["fields"]["name"], "Shared project");
}

#[tokio::test]
async fn chat_log_checkpoint_range_and_blobs_are_durable_and_deduplicated() {
    let f = Fixture::new().await;
    let client_token = f.pair("client-a", NodeRole::Client).await;
    let mut a = f.socket("/chat2/chat-a/ws", &f.config.token).await;
    let mut b = f.socket("/chat2/chat-a/ws", &client_token).await;
    for (socket, device) in [(&mut a, "server-a"), (&mut b, "client-a")] {
        socket
            .send(Message::Binary(chat_frames::encode(
                ft::HELLO,
                &json!({"cursor":0,"device":device}),
                &[],
            )))
            .await
            .unwrap();
        assert_eq!(chat_frame(socket).await.header["headSeq"], 0);
    }
    a.send(Message::Binary(chat_frames::encode(
        ft::PUSH,
        &json!({"batchId":"batch-a"}),
        b"update-a",
    )))
    .await
    .unwrap();
    let ack = chat_frame(&mut a).await;
    assert_eq!(ack.kind, ft::ACK);
    assert_eq!(ack.header["seq"], 1);
    let row = chat_frame(&mut b).await;
    assert_eq!(row.kind, ft::ROW);
    assert_eq!(row.payload, b"update-a");
    let response = f
        .client
        .post(f.url("/chat2/chat-a/checkpoint?seqCovered=1"))
        .bearer_auth(&f.config.token)
        .header("x-chat2-frontier", "AQ==")
        .body("checkpoint-content")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let checkpoint = f
        .client
        .get(f.url("/chat2/chat-a/checkpoint"))
        .bearer_auth(&f.config.token)
        .header("range", "bytes=11-")
        .send()
        .await
        .unwrap();
    assert_eq!(checkpoint.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(checkpoint.headers()["x-chat2-checkpoint-seq"], "1");
    assert_eq!(checkpoint.text().await.unwrap(), "content");
    let duplicate: Value = f
        .client
        .post(f.url("/chat2/chat-a/rows?batchId=batch-a"))
        .bearer_auth(&f.config.token)
        .body("update-a")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(duplicate, json!({"batchId":"batch-a","seq":1,"dup":true}));
    let stats: Value = f.get("/chat2/chat-a/stats").await.json().await.unwrap();
    assert_eq!(stats["rowCount"], 0);
    assert_eq!(stats["headSeq"], 1);
    let invalid = f
        .client
        .post(f.url("/chat2/chat-a/checkpoint?seqCovered=2"))
        .bearer_auth(&f.config.token)
        .header("x-chat2-frontier", "AQ==")
        .body("ahead")
        .send()
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::CONFLICT);
    for path in [
        "/chat2/chat-a/tail",
        "/chat2/chat-a/diff",
        "/blob/chat-a/tool%23call",
    ] {
        let put = f
            .client
            .put(f.url(path))
            .bearer_auth(&f.config.token)
            .header("content-type", "application/json")
            .body("{\"content\":\"kept\"}")
            .send()
            .await
            .unwrap();
        assert_eq!(put.status(), StatusCode::OK);
        assert_eq!(
            f.get(path).await.text().await.unwrap(),
            "{\"content\":\"kept\"}"
        );
    }
    let bytes = f
        .get("/chat2/chat-a/rows?after=0")
        .await
        .bytes()
        .await
        .unwrap();
    let length = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let state = chat_frames::decode(&bytes[4..4 + length]).unwrap();
    assert_eq!(state.kind, ft::STATE);
    assert_eq!(state.header["seqFloor"], 1);
    assert_eq!(state.payload, vec![1]);
    let reopened = Hub::open(f.directory.path(), &f.config).unwrap();
    assert_eq!(reopened.nodes().unwrap().len(), 2);
}

#[tokio::test]
async fn relay_binds_host_identity_routes_two_servers_and_revokes_live_clients() {
    let f = Fixture::new().await;
    let server_b = f.pair("server-b", NodeRole::Server).await;
    let client_token = f.pair("client-a", NodeRole::Client).await;
    let mut host_a = f
        .socket("/device/server-a/ws?role=host", &f.config.token)
        .await;
    let mut host_b = f.socket("/device/server-b/ws?role=host", &server_b).await;
    let mut client_a = f
        .socket(
            "/device/server-a/ws?role=client&connId=connection-a",
            &client_token,
        )
        .await;
    let mut client_b = f
        .socket(
            "/device/server-b/ws?role=client&connId=connection-b",
            &client_token,
        )
        .await;
    for (client, host, connection, payload) in [
        (
            &mut client_a,
            &mut host_a,
            "connection-a",
            b"server A".as_slice(),
        ),
        (
            &mut client_b,
            &mut host_b,
            "connection-b",
            b"server B".as_slice(),
        ),
    ] {
        client
            .send(Message::Binary(
                encode_device_frame(
                    &DeviceFrameHeader {
                        s: "rpc".into(),
                        k: "rpc".into(),
                        to: None,
                        from: Some("spoof".into()),
                    },
                    payload,
                )
                .unwrap(),
            ))
            .await
            .unwrap();
        let (header, received) = decode_device_frame(&receive(host).await.into_data()).unwrap();
        assert_eq!(header.from.as_deref(), Some(connection));
        assert_eq!(received, payload);
        host.send(Message::Binary(
            encode_device_frame(
                &DeviceFrameHeader::new("rpc", "rpc").with_to(connection),
                b"reply",
            )
            .unwrap(),
        ))
        .await
        .unwrap();
        let (header, received) = decode_device_frame(&receive(client).await.into_data()).unwrap();
        assert_eq!(header.to, None);
        assert_eq!(header.from, None);
        assert_eq!(received, b"reply");
    }
    let mut unauthorized = f
        .url("/device/server-a/ws?role=host")
        .replacen("http", "ws", 1)
        .into_client_request()
        .unwrap();
    unauthorized.headers_mut().insert(
        "authorization",
        format!("Bearer {server_b}").parse().unwrap(),
    );
    assert!(
        tokio_tungstenite::connect_async(unauthorized)
            .await
            .is_err()
    );
    assert_eq!(
        f.client
            .post(f.url("/device/server-a/sidecar/repos"))
            .bearer_auth(&server_b)
            .json(&json!([]))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    f.hub.revoke("client-a").unwrap();
    assert!(matches!(receive(&mut client_a).await, Message::Close(_)));
    assert!(matches!(receive(&mut client_b).await, Message::Close(_)));
    assert_eq!(
        f.client
            .get(f.url(&registry_path(&f, "rows")))
            .bearer_auth(&client_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn offline_nudges_survive_restart_and_disabling_disconnects_every_socket() {
    let mut f = Fixture::new().await;
    let response: Value = f
        .client
        .post(f.url("/device/server-a/nudge"))
        .bearer_auth(&f.config.token)
        .json(&json!({"chatId":"chat-a"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["queued"], true);
    assert_eq!(response["delivered"], false);
    f.handle.shutdown();
    let reopened = Hub::open(f.directory.path(), &f.config).unwrap();
    f.handle = reopened.clone().start().await.unwrap();
    f.hub = reopened;
    let mut host = f
        .socket("/device/server-a/ws?role=host", &f.config.token)
        .await;
    let (header, payload) = decode_device_frame(&receive(&mut host).await.into_data()).unwrap();
    assert_eq!(header.k, "nudge");
    assert_eq!(
        serde_json::from_slice::<Value>(&payload).unwrap(),
        json!({"chatId":"chat-a"})
    );
    f.hub.set_enabled(false).unwrap();
    assert!(matches!(receive(&mut host).await, Message::Close(_)));
    assert_eq!(
        f.get("/private/info").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(
        !PrivateConfig::load(f.directory.path())
            .unwrap()
            .unwrap()
            .enabled
    );
    f.hub.set_enabled(true).unwrap();
    assert_eq!(f.get("/private/info").await.status(), StatusCode::OK);
}

struct PrivateUrl {
    url: String,
    token: String,
}
impl zeron_sync::UrlProvider for PrivateUrl {
    fn url(&self) -> futures::future::BoxFuture<'static, Result<String, zeron_sync::SyncError>> {
        let url = self.url.clone();
        Box::pin(async move { Ok(url) })
    }
    fn authorization(&self) -> futures::future::BoxFuture<'static, Option<String>> {
        let token = self.token.clone();
        Box::pin(async move { Some(token) })
    }
}

async fn eventually(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn production_registry_clients_converge_over_header_authenticated_hub() {
    use std::sync::Mutex;
    use zeron_doc::RegistryDoc;
    use zeron_sync::RegistryClient;
    let f = Fixture::new().await;
    let token_b = f.pair("server-b", NodeRole::Server).await;
    let a = Arc::new(Mutex::new(RegistryDoc::new("server-a")));
    let b = Arc::new(Mutex::new(RegistryDoc::new("server-b")));
    a.lock().unwrap().upsert_space(&serde_json::from_value(json!({"id":"project","deviceId":"server-a","path":"/tmp/project","name":"Original","createdAt":"2026-01-01T00:00:00Z"})).unwrap()).unwrap();
    let url = f.url(&registry_path(&f, "ws")).replacen("http", "ws", 1);
    let client_a = RegistryClient::connect_via(
        Arc::new(PrivateUrl {
            url: url.clone(),
            token: f.config.token.clone(),
        }),
        a.clone(),
        "server-a",
    )
    .await
    .unwrap();
    client_a.nudge();
    eventually(|| a.lock().unwrap().pending_len() == 0).await;
    let client_b = RegistryClient::connect_via(
        Arc::new(PrivateUrl {
            url,
            token: token_b,
        }),
        b.clone(),
        "server-b",
    )
    .await
    .unwrap();
    eventually(|| b.lock().unwrap().read_spaces().unwrap().len() == 1).await;
    assert_eq!(
        b.lock().unwrap().read_spaces().unwrap()[0].name.as_deref(),
        Some("Original")
    );
    b.lock()
        .unwrap()
        .rename_space("project", Some("Changed remotely"))
        .unwrap();
    client_b.nudge();
    eventually(|| {
        a.lock().unwrap().read_spaces().unwrap()[0].name.as_deref() == Some("Changed remotely")
    })
    .await;
    client_a.shutdown().await;
    client_b.shutdown().await;
}

#[tokio::test]
async fn large_backfill_streams_without_overflowing_the_live_message_queue() {
    let f = Fixture::new().await;
    for sequence in 1..=180 {
        let response = f
            .client
            .post(f.url(&format!("/chat2/backfill/rows?batchId=b-{sequence}")))
            .bearer_auth(&f.config.token)
            .body(format!("row-{sequence}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let token = f.pair("reader", NodeRole::Client).await;
    let mut socket = f.socket("/chat2/backfill/ws", &token).await;
    socket
        .send(Message::Binary(chat_frames::encode(
            ft::HELLO,
            &json!({"device":"reader","cursor":0}),
            &[],
        )))
        .await
        .unwrap();
    assert_eq!(chat_frame(&mut socket).await.header["headSeq"], 180);
    socket
        .send(Message::Binary(chat_frames::encode(
            ft::ROWS_REQ,
            &json!({"after":0,"excludeOwn":true}),
            &[],
        )))
        .await
        .unwrap();
    for sequence in 1..=180 {
        let frame = chat_frame(&mut socket).await;
        assert_eq!(frame.kind, ft::ROW);
        assert_eq!(frame.header["seq"], sequence);
        assert_eq!(frame.payload, format!("row-{sequence}").as_bytes());
    }
    let done = chat_frame(&mut socket).await;
    assert_eq!(done.kind, ft::ROWS_DONE);
    assert_eq!(done.header["headSeq"], 180);
}

#[tokio::test]
async fn expired_invitations_are_refused_and_credentials_are_not_stored_in_plaintext() {
    let f = Fixture::new().await;
    let invitation = f.hub.create_invitation(NodeRole::Client).unwrap();
    let path = f
        .directory
        .path()
        .join("profiles/private")
        .join(&f.config.workspace_id)
        .join("hub.sqlite");
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE invitation SET expires_at=0", [])
        .unwrap();
    let response = f
        .client
        .post(f.url("/private/pair"))
        .json(
            &json!({"code":invitation.code,"name":"Expired","deviceId":"expired","role":"client"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let stored: String = db
        .query_row(
            "SELECT token_hash FROM nodes WHERE id='server-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(stored, f.config.token);
    assert_eq!(stored.len(), 64);
    let client = f.pair("new-client", NodeRole::Client).await;
    let stored: String = db
        .query_row(
            "SELECT token_hash FROM nodes WHERE id='new-client'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(stored, client);
    assert_eq!(stored.len(), 64);
}

struct Replica {
    doc: std::sync::Mutex<zeron_doc::SessionDoc>,
    cursor: std::sync::atomic::AtomicU64,
}
impl Replica {
    fn new() -> Self {
        Self {
            doc: std::sync::Mutex::new(zeron_doc::SessionDoc::init("real-chat").unwrap()),
            cursor: std::sync::atomic::AtomicU64::new(0),
        }
    }
    fn text(&self) -> String {
        self.doc
            .lock()
            .unwrap()
            .doc()
            .get_text("transport-test")
            .to_string()
    }
    fn snapshot(&self) -> Vec<u8> {
        self.doc.lock().unwrap().export_snapshot().unwrap()
    }
}
impl zeron_sync::ChatDocSink for Replica {
    fn apply_row(&self, bytes: &[u8], cursor: u64) -> zeron_sync::chat_client::RowImportOutcome {
        self.doc.lock().unwrap().doc().import(bytes).unwrap();
        self.cursor
            .store(cursor, std::sync::atomic::Ordering::Relaxed);
        zeron_sync::chat_client::RowImportOutcome::Applied
    }
    fn apply_checkpoint(&self, bytes: &[u8], cursor: u64) -> Result<(), String> {
        self.apply_row(bytes, cursor);
        Ok(())
    }
    fn contains_frontier(&self, frontier: &[u8]) -> bool {
        frontier.is_empty()
    }
    fn advance_cursor(&self, cursor: u64) {
        self.cursor
            .store(cursor, std::sync::atomic::Ordering::Relaxed);
    }
}
struct HttpCheckpoint {
    url: String,
    token: String,
}
impl zeron_sync::CheckpointFetcher for HttpCheckpoint {
    fn fetch(&self) -> futures::future::BoxFuture<'static, Result<Vec<u8>, zeron_sync::SyncError>> {
        let url = self.url.clone();
        let token = self.token.clone();
        Box::pin(async move {
            Ok(reqwest::Client::new()
                .get(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(|e| zeron_sync::SyncError::Protocol(e.to_string()))?
                .error_for_status()
                .map_err(|e| zeron_sync::SyncError::Protocol(e.to_string()))?
                .bytes()
                .await
                .map_err(|e| zeron_sync::SyncError::Protocol(e.to_string()))?
                .to_vec())
        })
    }
}

#[tokio::test]
async fn production_chat_clients_exchange_real_documents_and_bootstrap_from_checkpoint() {
    use zeron_sync::ChatClient;
    let f = Fixture::new().await;
    let token_b = f.pair("server-b", NodeRole::Server).await;
    let url = f.url("/chat2/real-chat/ws").replacen("http", "ws", 1);
    let fetcher = Arc::new(HttpCheckpoint {
        url: f.url("/chat2/real-chat/checkpoint"),
        token: f.config.token.clone(),
    });
    let a = Arc::new(Replica::new());
    let b = Arc::new(Replica::new());
    let client_a = ChatClient::connect_via(
        Arc::new(PrivateUrl {
            url: url.clone(),
            token: f.config.token.clone(),
        }),
        a.clone(),
        fetcher.clone(),
        "server-a",
        0,
    )
    .await
    .unwrap();
    let client_b = ChatClient::connect_via(
        Arc::new(PrivateUrl {
            url: url.clone(),
            token: token_b,
        }),
        b.clone(),
        fetcher.clone(),
        "server-b",
        0,
    )
    .await
    .unwrap();
    a.doc
        .lock()
        .unwrap()
        .doc()
        .get_text("transport-test")
        .insert(0, "Hello")
        .unwrap();
    client_a.enqueue_batch("actual-loro-a".into(), a.snapshot());
    eventually(|| b.text() == "Hello" && client_a.stats().pending_pushes == 0).await;
    b.doc
        .lock()
        .unwrap()
        .doc()
        .get_text("transport-test")
        .insert(5, " from B")
        .unwrap();
    client_b.enqueue_batch("actual-loro-b".into(), b.snapshot());
    eventually(|| a.text() == "Hello from B" && client_b.stats().pending_pushes == 0).await;
    let response = f
        .client
        .post(f.url("/chat2/real-chat/checkpoint?seqCovered=2"))
        .bearer_auth(&f.config.token)
        .header("x-chat2-frontier", "AQ==")
        .body(a.snapshot())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let token_c = f.pair("client-c", NodeRole::Client).await;
    let c = Arc::new(Replica::new());
    let client_c = ChatClient::connect_via(
        Arc::new(PrivateUrl {
            url,
            token: token_c,
        }),
        c.clone(),
        fetcher,
        "client-c",
        0,
    )
    .await
    .unwrap();
    assert_eq!(c.text(), "Hello from B");
    assert_eq!(client_c.stats().cursor, 2);
    client_a.shutdown().await;
    client_b.shutdown().await;
    client_c.shutdown().await;
}

#[tokio::test]
async fn rejected_chat_batches_receive_permanent_error_without_losing_the_connection() {
    let f = Fixture::new().await;
    let mut socket = f.socket("/chat2/reject/ws", &f.config.token).await;
    socket
        .send(Message::Binary(chat_frames::encode(
            ft::HELLO,
            &json!({"device":"server-a","cursor":0}),
            &[],
        )))
        .await
        .unwrap();
    chat_frame(&mut socket).await;
    socket
        .send(Message::Binary(chat_frames::encode(
            ft::PUSH,
            &json!({"batchId":"empty-update"}),
            &[],
        )))
        .await
        .unwrap();
    let error = chat_frame(&mut socket).await;
    assert_eq!(error.kind, ft::ERROR);
    assert_eq!(error.header["code"], "empty");
    assert_eq!(error.header["batchId"], "empty-update");
    socket
        .send(Message::Binary(chat_frames::encode(
            ft::PUSH,
            &json!({"batchId":"good-update"}),
            b"valid",
        )))
        .await
        .unwrap();
    let ack = chat_frame(&mut socket).await;
    assert_eq!(ack.kind, ft::ACK);
    assert_eq!(ack.header["seq"], 1);
    f.handle.shutdown();
    f.handle.wait_stopped().await;
    let mut same_port = f.config.clone();
    same_port.listen_port = f.handle.local_addr().port();
    let next = Hub::open(f.directory.path(), &same_port)
        .unwrap()
        .start()
        .await
        .unwrap();
    assert_eq!(next.local_addr(), f.handle.local_addr());
}

#[tokio::test]
async fn rejoining_rotates_credentials_disconnects_old_sessions_and_preserves_hub_identity() {
    let f = Fixture::new().await;
    let old_token = f.pair("returning-client", NodeRole::Client).await;
    let mut old_socket = f.socket(&registry_path(&f, "ws"), &old_token).await;
    text(
        &mut old_socket,
        json!({"t":"hello","device":"returning-client","cursor":0}),
    )
    .await;
    assert_eq!(json_frame(&mut old_socket).await["t"], "state");
    let new_token = f.pair("returning-client", NodeRole::Client).await;
    assert_ne!(new_token, old_token);
    assert!(matches!(receive(&mut old_socket).await, Message::Close(_)));
    let url = f.url(&registry_path(&f, "rows"));
    assert_eq!(
        f.client
            .get(&url)
            .bearer_auth(&old_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        f.client
            .get(&url)
            .bearer_auth(&new_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    f.hub.revoke("returning-client").unwrap();
    let replacement = f.pair("returning-client", NodeRole::Server).await;
    assert_ne!(replacement, new_token);
    assert_eq!(
        f.hub
            .nodes()
            .unwrap()
            .iter()
            .find(|node| node.device_id == "returning-client")
            .unwrap()
            .role,
        NodeRole::Server
    );
    assert_eq!(
        f.client
            .get(&url)
            .bearer_auth(&new_token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let invitation = f.hub.create_invitation(NodeRole::Server).unwrap();
    let attempt=f.client.post(f.url("/private/pair")).json(&json!({"code":invitation.code,"deviceId":"server-a","name":"Replacement hub","role":"server"})).send().await.unwrap();
    assert_eq!(attempt.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        f.get(&registry_path(&f, "rows")).await.status(),
        StatusCode::OK
    );
    assert_eq!(f.hub.nodes().unwrap().len(), 2);
}
