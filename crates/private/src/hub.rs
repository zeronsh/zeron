use std::{
    collections::HashMap,
    convert::Infallible,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Result, bail, ensure};
use base64::Engine;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{Method, Request, Response, StatusCode, body::Incoming, header, service::service_fn};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::derive_accept_key,
        protocol::{Role, WebSocketConfig},
    },
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use zeron_rpc::device_room::{NUDGE_KIND, RELAY_KIND};
use zeron_rpc::{DeviceFrameHeader, decode_device_frame, encode_device_frame};
use zeron_sync::chat_frames::{self, frame_type as ft};

use crate::{
    Invitation, Node, NodeRole, PrivateConfig, WorkspaceInfo,
    config::{PairRequest, PairResponse, valid_id},
    now_ms,
    store::{RegistryPush, Store},
};

type HttpResponse = Response<Full<Bytes>>;
type HubSocket = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;

struct Backfill {
    chat: String,
    after: u64,
    head: u64,
    exclude: Option<String>,
}

pub struct Hub {
    config: PrivateConfig,
    data_dir: PathBuf,
    state: Mutex<State>,
}

struct State {
    store: Store,
    enabled: bool,
    peers: HashMap<Uuid, Peer>,
    presence: HashMap<String, i64>,
}

#[derive(Clone, PartialEq, Eq)]
enum Room {
    Registry,
    Chat(String),
    Device {
        device: String,
        host: bool,
        conn: String,
    },
}

struct Peer {
    node: String,
    room: Room,
    ready: bool,
    sender: mpsc::Sender<Message>,
    close: CancellationToken,
    last_seen: i64,
}

pub struct HubHandle {
    address: SocketAddr,
    stop: CancellationToken,
    stopped: CancellationToken,
}

impl HubHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.address
    }
    pub fn shutdown(&self) {
        self.stop.cancel();
    }

    pub async fn wait_stopped(&self) {
        self.stopped.cancelled().await;
    }
}
impl Drop for HubHandle {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

impl Hub {
    pub fn open(data_dir: &Path, config: &PrivateConfig) -> Result<Arc<Self>> {
        ensure!(
            config.host_hub,
            "This node is not configured to host the hub"
        );
        Ok(Arc::new(Self {
            config: config.clone(),
            data_dir: data_dir.to_path_buf(),
            state: Mutex::new(State {
                store: Store::open(data_dir, config)?,
                enabled: config.enabled,
                peers: HashMap::new(),
                presence: HashMap::new(),
            }),
        }))
    }

    pub async fn start(self: Arc<Self>) -> Result<HubHandle> {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, self.config.listen_port)).await?;
        let address = listener.local_addr()?;
        let stop = CancellationToken::new();
        let stopped = CancellationToken::new();
        let finished = stopped.clone();
        let shutdown = stop.clone();
        tokio::spawn(async move {
            struct Finished(CancellationToken);
            impl Drop for Finished {
                fn drop(&mut self) {
                    self.0.cancel();
                }
            }
            let _finished = Finished(finished);
            loop {
                tokio::select! {
                    _=shutdown.cancelled()=>break,
                    incoming=listener.accept()=>{
                        let Ok((stream,_))=incoming else {continue;};
                        let hub=self.clone();
                        let connection_stop=shutdown.clone();
                        tokio::spawn(async move {
                            let service=service_fn(move |request|{
                                let hub=hub.clone();
                                async move {Ok::<_,Infallible>(hub.handle(request).await)}
                            });
                            let connection=hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream),service).with_upgrades();
                            tokio::select! {_=connection_stop.cancelled()=>{},result=connection=>{if let Err(error)=result {tracing::debug!(%error,"Private hub connection ended");}}}
                        });
                    }
                }
            }
            let mut state = self.state.lock().unwrap();
            for peer in state.peers.values() {
                peer.close.cancel();
            }
            state.peers.clear();
            drop(listener);
        });
        Ok(HubHandle {
            address,
            stop,
            stopped,
        })
    }

    pub fn info(&self) -> WorkspaceInfo {
        WorkspaceInfo {
            protocol_version: 1,
            workspace_id: self.config.workspace_id.clone(),
            name: self.config.name.clone(),
            capabilities: vec![
                "registry".into(),
                "chat2".into(),
                "device-relay".into(),
                "blobs".into(),
            ],
        }
    }

    pub fn create_invitation(&self, role: NodeRole) -> Result<Invitation> {
        let state = self.state.lock().unwrap();
        ensure!(state.enabled, "Private access is disabled");
        let (code, expires_at) = state.store.invite(role)?;
        Ok(Invitation {
            code,
            expires_at,
            hub_url: self.config.hub_url.clone(),
        })
    }

    pub fn nodes(&self) -> Result<Vec<Node>> {
        self.state.lock().unwrap().store.nodes()
    }

    pub fn revoke(&self, device_id: &str) -> Result<()> {
        ensure!(
            device_id != self.config.device_id,
            "The hub's own node cannot be revoked; disable private access instead"
        );
        let state = self.state.lock().unwrap();
        state.store.revoke(device_id)?;
        for peer in state.peers.values().filter(|p| p.node == device_id) {
            peer.close.cancel();
        }
        Ok(())
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let mut config = PrivateConfig::load(&self.data_dir)?
            .ok_or_else(|| anyhow::anyhow!("Private configuration was removed"))?;
        config.enabled = enabled;
        config.save(&self.data_dir)?;
        state.enabled = enabled;
        if !enabled {
            for peer in state.peers.values() {
                peer.close.cancel();
            }
        }
        Ok(())
    }

    async fn handle(self: Arc<Self>, request: Request<Incoming>) -> HttpResponse {
        match self.route(request).await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(%error,"Private hub request rejected");
                json_response(StatusCode::BAD_REQUEST, json!({"error":"invalid_request"}))
            }
        }
    }

    async fn route(self: Arc<Self>, mut request: Request<Incoming>) -> Result<HttpResponse> {
        let path = request.uri().path().to_owned();
        let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
        let query: HashMap<String, String> =
            url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        let method = request.method().clone();
        if query.contains_key("token") {
            return Ok(json_response(
                StatusCode::UNAUTHORIZED,
                json!({"error":"use_authorization_header"}),
            ));
        }
        if !self.state.lock().unwrap().enabled {
            return Ok(json_response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":"private_access_disabled"}),
            ));
        }
        if path == "/private/info" && method == Method::GET {
            return Ok(json_response(
                StatusCode::OK,
                serde_json::to_value(self.info())?,
            ));
        }
        if path == "/private/pair" && method == Method::POST {
            let body = read_body(request, 4096).await?;
            let pair: PairRequest = serde_json::from_slice(&body)?;
            let token = {
                let mut state = self.state.lock().unwrap();
                ensure!(state.enabled, "Private access is disabled");
                if pair.device_id == self.config.device_id {
                    None
                } else {
                    let token = state.store.pair(&pair)?;
                    if token.is_some() {
                        for peer in state
                            .peers
                            .values()
                            .filter(|peer| peer.node == pair.device_id)
                        {
                            peer.close.cancel();
                        }
                        state.presence.remove(&pair.device_id);
                    }
                    token
                }
            };
            return Ok(match token {
                Some(token) => json_response(
                    StatusCode::OK,
                    serde_json::to_value(PairResponse {
                        workspace_id: self.config.workspace_id.clone(),
                        user_id: self.config.user_id.clone(),
                        device_id: pair.device_id,
                        token,
                    })?,
                ),
                None => json_response(
                    StatusCode::UNAUTHORIZED,
                    json!({"error":"invalid_invitation"}),
                ),
            });
        }
        let (node, credential_hash) = {
            let token = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.strip_prefix("Bearer "));
            let state = self.state.lock().unwrap();
            match token {
                Some(token) => (state.store.authenticate(token)?, crate::digest(token)),
                None => (None, String::new()),
            }
        };
        let Some(node) = node else {
            return Ok(json_response(
                StatusCode::UNAUTHORIZED,
                json!({"error":"unauthenticated"}),
            ));
        };
        if query
            .get("device")
            .is_some_and(|device| !device.is_empty() && device != &node.device_id)
        {
            return Ok(json_response(
                StatusCode::FORBIDDEN,
                json!({"error":"device_identity_mismatch"}),
            ));
        }
        let room = match segments.as_slice() {
            ["registry", workspace, "ws"] if *workspace == self.config.workspace_id => {
                Some(Room::Registry)
            }
            ["chat2", chat, "ws"] if valid_id(chat) => Some(Room::Chat((*chat).into())),
            ["device", device, "ws"] if valid_id(device) => {
                let host = query.get("role").is_some_and(|role| role == "host");
                let allowed = {
                    let state = self.state.lock().unwrap();
                    state
                        .store
                        .node(device)?
                        .is_some_and(|target| target.role == NodeRole::Server)
                };
                if !allowed
                    || (host && (node.role != NodeRole::Server || node.device_id != *device))
                {
                    return Ok(json_response(
                        StatusCode::FORBIDDEN,
                        json!({"error":"forbidden_device_role"}),
                    ));
                }
                let conn = query
                    .get("connId")
                    .cloned()
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                ensure!(valid_id(&conn), "Invalid relay connection identity");
                Some(Room::Device {
                    device: (*device).into(),
                    host,
                    conn,
                })
            }
            _ => None,
        };
        if let Some(room) = room {
            ensure!(method == Method::GET, "WebSocket upgrade requires GET");
            let upgrade = request
                .headers()
                .get(header::UPGRADE)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.eq_ignore_ascii_case("websocket"));
            let connection = request
                .headers()
                .get(header::CONNECTION)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| {
                    h.split(',')
                        .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
                });
            ensure!(
                upgrade
                    && connection
                    && request
                        .headers()
                        .get("sec-websocket-version")
                        .is_some_and(|v| v == "13"),
                "WebSocket upgrade required"
            );
            let key = request
                .headers()
                .get("sec-websocket-key")
                .ok_or_else(|| anyhow::anyhow!("Missing WebSocket key"))?;
            let accept = derive_accept_key(key.as_bytes());
            let pending = hyper::upgrade::on(&mut request);
            let hub = self.clone();
            tokio::spawn(async move {
                if let Ok(upgraded) = pending.await {
                    let limits = WebSocketConfig {
                        max_message_size: Some(1024 * 1024 + 4096),
                        max_frame_size: Some(1024 * 1024 + 4096),
                        ..Default::default()
                    };
                    let websocket = WebSocketStream::from_raw_socket(
                        TokioIo::new(upgraded),
                        Role::Server,
                        Some(limits),
                    )
                    .await;
                    hub.websocket(websocket, node, credential_hash, room).await;
                }
            });
            return Ok(Response::builder()
                .status(StatusCode::SWITCHING_PROTOCOLS)
                .header(header::UPGRADE, "websocket")
                .header(header::CONNECTION, "Upgrade")
                .header("sec-websocket-accept", accept)
                .body(Full::new(Bytes::new()))?);
        }
        let headers = request.headers().clone();
        let limit = match segments.as_slice() {
            ["chat2", _, "checkpoint"] => 64 * 1024 * 1024,
            ["chat2", _, "tail" | "diff"] => 4 * 1024 * 1024,
            ["chat2", _, "rows"] => 1024 * 1024 + 4096,
            ["blob", _, _] => 1024 * 1024,
            _ => 2 * 1024 * 1024,
        };
        let body = read_body(request, limit).await?;
        let mut state = self.state.lock().unwrap();
        if !state.enabled || !state.store.active(&node.device_id, &credential_hash) {
            return Ok(json_response(
                StatusCode::UNAUTHORIZED,
                json!({"error":"access_revoked"}),
            ));
        }
        match segments.as_slice() {
            ["registry", workspace, action] if *workspace == self.config.workspace_id => {
                match (*action, method) {
                    ("rows", Method::GET) => {
                        if query.get("beat").is_some_and(|v| v == "1") {
                            presence(&mut state, &node.device_id, None);
                        }
                        let mut value = state
                            .store
                            .registry_rows(query.get("since").and_then(|v| v.parse().ok()))?;
                        value["presence"] = serde_json::to_value(&state.presence)?;
                        Ok(json_response(StatusCode::OK, value))
                    }
                    ("push", Method::POST) => {
                        let push: RegistryPush = serde_json::from_slice(&body)?;
                        let (ack, broadcast) = state.store.registry_push(push)?;
                        if let Some(value) = broadcast {
                            broadcast_room(
                                &state,
                                &Room::Registry,
                                Message::Text(value.to_string()),
                                None,
                            );
                        }
                        Ok(json_response(StatusCode::OK, ack))
                    }
                    ("stats", Method::GET) => Ok(json_response(
                        StatusCode::OK,
                        json!({"seq":state.store.registry_seq()?,"gcFloor":0,"connectedSockets":state.peers.values().filter(|p|p.room==Room::Registry).count()}),
                    )),
                    _ => Ok(not_found()),
                }
            }
            ["chat2", chat, action] if valid_id(chat) => chat_http(
                &mut state, chat, action, method, &query, &headers, &body, &node,
            ),
            ["device", device, action] if valid_id(device) => {
                ensure!(
                    state
                        .store
                        .node(device)?
                        .is_some_and(|target| target.role == NodeRole::Server),
                    "Unknown agent server"
                );
                match (*action, method) {
                    ("status", Method::GET) => Ok(json_response(
                        StatusCode::OK,
                        json!({"hostConnected":live_host(&state,device).is_some(),"hostSockets":state.peers.values().filter(|p|matches!(&p.room,Room::Device{device:d,host:true,..} if d==device)).count()}),
                    )),
                    ("nudge", Method::POST) => {
                        let body: Value = serde_json::from_slice(&body)?;
                        let chat = body
                            .get("chatId")
                            .and_then(Value::as_str)
                            .filter(|v| valid_id(v))
                            .ok_or_else(|| anyhow::anyhow!("Invalid chat ID"))?;
                        state.store.queue_nudge(device, chat)?;
                        let delivered = if let Some(host) = live_host(&state, device) {
                            deliver_nudge(host, chat)
                        } else {
                            false
                        };
                        // Keep a durable wake-up until the target has written to that chat.
                        Ok(json_response(
                            StatusCode::OK,
                            json!({"delivered":delivered,"queued":true}),
                        ))
                    }
                    _ => Ok(not_found()),
                }
            }
            ["device", device, "sidecar", name] if valid_id(device) && valid_id(name) => {
                ensure!(
                    state
                        .store
                        .node(device)?
                        .is_some_and(|target| target.role == NodeRole::Server),
                    "Unknown agent server"
                );
                let key = format!("device/{device}/{name}");
                if method == Method::POST {
                    if node.device_id != *device || node.role != NodeRole::Server {
                        return Ok(json_response(
                            StatusCode::FORBIDDEN,
                            json!({"error":"forbidden_device_role"}),
                        ));
                    }
                    serde_json::from_slice::<Value>(&body)?;
                    state.store.put_blob(&key, "application/json", &body)?;
                    Ok(json_response(StatusCode::OK, json!({"ok":true})))
                } else if method == Method::GET {
                    get_blob(&state.store, &key, false)
                } else {
                    Ok(not_found())
                }
            }
            ["blob", chat, part] if valid_id(chat) => {
                let part = url::form_urlencoded::parse(format!("p={part}").as_bytes())
                    .next()
                    .map(|(_, v)| v.into_owned())
                    .unwrap_or_default();
                ensure!(
                    !part.is_empty()
                        && part.len() <= 512
                        && part
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"_-.#:@".contains(&c)),
                    "Invalid blob identity"
                );
                let key = format!("blob/{chat}/{part}");
                if method == Method::PUT {
                    state.store.put_blob(
                        &key,
                        content_type(&headers, "text/plain; charset=utf-8"),
                        &body,
                    )?;
                    Ok(json_response(
                        StatusCode::OK,
                        json!({"ok":true,"bytes":body.len()}),
                    ))
                } else if method == Method::GET || method == Method::HEAD {
                    get_blob(&state.store, &key, method == Method::HEAD)
                } else {
                    Ok(not_found())
                }
            }
            _ => Ok(not_found()),
        }
    }

    async fn websocket(
        self: Arc<Self>,
        mut websocket: HubSocket,
        node: Node,
        credential_hash: String,
        room: Room,
    ) {
        let id = Uuid::new_v4();
        let close = CancellationToken::new();
        let (sender, mut outgoing) = mpsc::channel(64);
        {
            let mut state = self.state.lock().unwrap();
            if !state.enabled || !state.store.active(&node.device_id, &credential_hash) {
                return;
            }
            if let Room::Device { device, host, conn } = &room {
                for peer in state.peers.values().filter(|p|matches!(&p.room,Room::Device{device:d,host:h,conn:c} if d==device && h==host && (*host || c==conn))){peer.close.cancel();}
            }
            let ready = matches!(room, Room::Device { .. });
            state.peers.insert(
                id,
                Peer {
                    node: node.device_id.clone(),
                    room: room.clone(),
                    ready,
                    sender,
                    close: close.clone(),
                    last_seen: now_ms(),
                },
            );
            if let Room::Device {
                device, host: true, ..
            } = &room
            {
                if let Ok(nudges) = state.store.nudges(device) {
                    for chat in nudges {
                        if let Some(peer) = state.peers.get(&id) {
                            deliver_nudge(peer, &chat);
                        }
                    }
                }
            }
        }
        let mut deadline = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                biased;
                _=close.cancelled()=>{let _=tokio::time::timeout(std::time::Duration::from_secs(2),websocket.close(None)).await;break;},
                _=deadline.tick()=>{
                    let alive={let state=self.state.lock().unwrap();state.enabled && state.store.active(&node.device_id,&credential_hash) && state.peers.get(&id).is_some_and(|p|now_ms()-p.last_seen<90_000)};
                    if !alive{break;}
                },
                message=outgoing.recv()=>match message {Some(message)=>{if write_socket(&mut websocket,&close,message).await.is_err(){break;}},None=>break},
                message=websocket.next()=>match message {
                    Some(Ok(Message::Close(_)))|None|Some(Err(_))=>break,
                    Some(Ok(message))=>{
                        let mut backfill = None;
                        let result={let mut state=self.state.lock().unwrap();
                            if !state.enabled || !state.store.active(&node.device_id,&credential_hash){break;}
                            if let Some(peer)=state.peers.get_mut(&id){peer.last_seen=now_ms();}
                            process_message(&mut state,id,&node,&room,message,&mut backfill)
                        };
                        if let Err(error)=result {tracing::debug!(%error,"Private WebSocket protocol violation");break;}
                        if let Some(backfill) = backfill {
                            if self.send_backfill(&mut websocket,&close,backfill).await.is_err(){break;}
                        }
                    }
                }
            }
        }
        let mut state = self.state.lock().unwrap();
        state.peers.remove(&id);
        if let Room::Device { device, host, conn } = &room {
            if *host {
                if live_host(&state, device).is_none() {
                    for peer in state.peers.values().filter(
                        |p| matches!(&p.room,Room::Device{device:d,host:false,..} if d==device),
                    ) {
                        relay_error(peer, "", "host_closed", None, None);
                    }
                }
            } else if let Some(peer) = live_host(&state, device) {
                relay_error(peer, "", "client_closed", None, Some(conn));
            }
        }
    }

    async fn send_backfill(
        &self,
        websocket: &mut HubSocket,
        close: &CancellationToken,
        mut backfill: Backfill,
    ) -> Result<()> {
        loop {
            let (rows, more) = self.state.lock().unwrap().store.chat_rows(
                &backfill.chat,
                backfill.after,
                backfill.head,
                backfill.exclude.as_deref(),
            )?;
            for row in rows {
                backfill.after = row.seq;
                write_socket(
                    websocket,
                    close,
                    Message::Binary(chat_frames::encode(
                        ft::ROW,
                        &json!({"seq":row.seq,"device":row.device,"batchId":row.batch}),
                        &row.bytes,
                    )),
                )
                .await?;
            }
            if !more {
                break;
            }
        }
        write_socket(
            websocket,
            close,
            Message::Binary(chat_frames::encode(
                ft::ROWS_DONE,
                &json!({"headSeq":backfill.head}),
                &[],
            )),
        )
        .await
    }
}

async fn write_socket(
    socket: &mut HubSocket,
    close: &CancellationToken,
    message: Message,
) -> Result<()> {
    tokio::select! {
        biased;
        _=close.cancelled()=>bail!("Connection closed"),
        result=tokio::time::timeout(std::time::Duration::from_secs(30),socket.send(message))=>{result??;Ok(())}
    }
}

async fn read_body(request: Request<Incoming>, limit: usize) -> Result<Bytes> {
    Ok(Limited::new(request.into_body(), limit)
        .collect()
        .await
        .map_err(|_| anyhow::anyhow!("Request body exceeds limit or ended early"))?
        .to_bytes())
}
fn json_response(status: StatusCode, value: Value) -> HttpResponse {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from(value.to_string())))
        .unwrap()
}
fn not_found() -> HttpResponse {
    json_response(StatusCode::NOT_FOUND, json!({"error":"not_found"}))
}
fn content_type<'a>(headers: &'a hyper::HeaderMap, default: &'a str) -> &'a str {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or(default)
}
fn binary_response(bytes: Vec<u8>, mime: &str) -> Result<HttpResponse> {
    Ok(Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len())
        .body(Full::new(Bytes::from(bytes)))?)
}
fn get_blob(store: &Store, key: &str, head: bool) -> Result<HttpResponse> {
    match store.blob(key)? {
        Some((mime, bytes)) => {
            let len = bytes.len();
            let mut response = binary_response(if head { Vec::new() } else { bytes }, &mime)?;
            response
                .headers_mut()
                .insert(header::CONTENT_LENGTH, len.into());
            Ok(response)
        }
        None => Ok(not_found()),
    }
}

fn send(peer: &Peer, message: Message) -> bool {
    if peer.close.is_cancelled() {
        return false;
    }
    if peer.sender.try_send(message).is_err() {
        peer.close.cancel();
        false
    } else {
        true
    }
}
fn broadcast_room(state: &State, room: &Room, message: Message, exclude: Option<Uuid>) {
    for (id, peer) in &state.peers {
        if peer.ready && &peer.room == room && Some(*id) != exclude {
            send(peer, message.clone());
        }
    }
}
fn presence(state: &mut State, device: &str, exclude: Option<Uuid>) {
    let at = now_ms();
    state.presence.retain(|_, time| at - *time < 75_000);
    state.presence.insert(device.into(), at);
    broadcast_room(
        state,
        &Room::Registry,
        Message::Text(json!({"t":"presence","device":device,"at":at}).to_string()),
        exclude,
    );
}
fn live_host<'a>(state: &'a State, device: &str) -> Option<&'a Peer> {
    state
        .peers
        .values()
        .filter(|p| {
            !p.close.is_cancelled()
                && now_ms() - p.last_seen < 75_000
                && matches!(&p.room,Room::Device{device:d,host:true,..} if d==device)
        })
        .max_by_key(|p| p.last_seen)
}
fn relay_error(peer: &Peer, stream: &str, code: &str, to: Option<&str>, from: Option<&str>) {
    let header = DeviceFrameHeader {
        s: stream.into(),
        k: RELAY_KIND.into(),
        to: to.map(str::to_owned),
        from: from.map(str::to_owned),
    };
    if let Ok(bytes) = encode_device_frame(&header, json!({"error":code}).to_string().as_bytes()) {
        send(peer, Message::Binary(bytes));
    }
}
fn deliver_nudge(peer: &Peer, chat: &str) -> bool {
    encode_device_frame(
        &DeviceFrameHeader::new(chat, NUDGE_KIND),
        json!({"chatId":chat}).to_string().as_bytes(),
    )
    .is_ok_and(|bytes| send(peer, Message::Binary(bytes)))
}

fn chat_http(
    state: &mut State,
    chat: &str,
    action: &str,
    method: Method,
    query: &HashMap<String, String>,
    headers: &hyper::HeaderMap,
    body: &[u8],
    node: &Node,
) -> Result<HttpResponse> {
    match (action, method) {
        ("rows", Method::GET) => {
            let stats = state.store.chat_stats(chat)?;
            let mut frames = vec![chat_frames::encode(
                ft::STATE,
                &stats,
                &state.store.frontier(chat)?,
            )];
            let after = query.get("after").and_then(|v| v.parse().ok()).unwrap_or(0);
            let exclude = query
                .get("excludeOwn")
                .is_some_and(|v| v == "1")
                .then_some(node.device_id.as_str());
            let mut total = frames[0].len() + 4;
            let (rows, more) = state
                .store
                .chat_rows(chat, after, stats.head_seq, exclude)?;
            let mut truncated = more;
            for row in rows {
                let frame = chat_frames::encode(
                    ft::ROW,
                    &json!({"seq":row.seq,"device":row.device,"batchId":row.batch}),
                    &row.bytes,
                );
                if total + frame.len() + 4 > 4 * 1024 * 1024 {
                    truncated = true;
                    break;
                }
                total += frame.len() + 4;
                frames.push(frame);
            }
            if !truncated {
                frames.push(chat_frames::encode(
                    ft::ROWS_DONE,
                    &json!({"headSeq":stats.head_seq}),
                    &[],
                ));
            }
            let mut bytes = Vec::new();
            for frame in frames {
                bytes.extend_from_slice(&(frame.len() as u32).to_le_bytes());
                bytes.extend(frame);
            }
            binary_response(bytes, "application/octet-stream")
        }
        ("rows", Method::POST) => {
            let batch = query.get("batchId").map(String::as_str).unwrap_or("");
            if let Some(code) = chat_push_error(batch, body) {
                return Ok(json_response(
                    if code == "too_large" {
                        StatusCode::PAYLOAD_TOO_LARGE
                    } else {
                        StatusCode::BAD_REQUEST
                    },
                    json!({"error":code}),
                ));
            }
            let (seq, dup) = state
                .store
                .append_chat(chat, &node.device_id, batch, body)?;
            state.store.remove_nudge(&node.device_id, chat)?;
            if !dup {
                broadcast_room(
                    state,
                    &Room::Chat(chat.into()),
                    Message::Binary(chat_frames::encode(
                        ft::ROW,
                        &json!({"seq":seq,"device":node.device_id,"batchId":batch}),
                        body,
                    )),
                    None,
                );
            }
            Ok(json_response(
                StatusCode::OK,
                json!({"batchId":batch,"seq":seq,"dup":dup}),
            ))
        }
        ("checkpoint", Method::POST) => {
            let covered: u64 = query
                .get("seqCovered")
                .ok_or_else(|| anyhow::anyhow!("Missing checkpoint sequence"))?
                .parse()?;
            let frontier = base64::engine::general_purpose::STANDARD.decode(
                headers
                    .get("x-chat2-frontier")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or(""),
            )?;
            match state.store.checkpoint(chat, covered, &frontier, body) {
                Ok(pruned) => Ok(json_response(
                    StatusCode::OK,
                    json!({"ok":true,"seqFloor":covered,"pruned":pruned}),
                )),
                Err(_) => Ok(json_response(
                    StatusCode::CONFLICT,
                    json!({"error":"invalid_checkpoint"}),
                )),
            }
        }
        ("checkpoint", Method::GET) => {
            let Some((seq, bytes)) = state.store.checkpoint_bytes(chat)? else {
                return Ok(not_found());
            };
            let range = headers
                .get(header::RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|v| v.strip_suffix('-'))
                .and_then(|v| v.parse::<usize>().ok());
            let length = bytes.len();
            if range.is_some_and(|start| start >= length) {
                return Ok(Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{length}"))
                    .body(Full::new(Bytes::new()))?);
            }
            let mut response = binary_response(
                bytes[range.unwrap_or(0)..].to_vec(),
                "application/octet-stream",
            )?;
            response
                .headers_mut()
                .insert(header::ACCEPT_RANGES, "bytes".parse()?);
            response
                .headers_mut()
                .insert("x-chat2-checkpoint-seq", seq.to_string().parse()?);
            if let Some(start) = range {
                *response.status_mut() = StatusCode::PARTIAL_CONTENT;
                response.headers_mut().insert(
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{}/{length}", length - 1).parse()?,
                );
            }
            Ok(response)
        }
        ("stats", Method::GET) => {
            let mut value = serde_json::to_value(state.store.chat_stats(chat)?)?;
            value["connectedSockets"] = json!(
                state
                    .peers
                    .values()
                    .filter(|p| p.room == Room::Chat(chat.into()))
                    .count()
            );
            Ok(json_response(StatusCode::OK, value))
        }
        ("tail" | "diff", Method::PUT) => {
            state.store.put_blob(
                &format!("chat/{chat}/{action}"),
                content_type(headers, "application/json"),
                body,
            )?;
            Ok(json_response(
                StatusCode::OK,
                json!({"ok":true,"bytes":body.len()}),
            ))
        }
        ("tail" | "diff", Method::GET) => {
            get_blob(&state.store, &format!("chat/{chat}/{action}"), false)
        }
        _ => Ok(not_found()),
    }
}

fn process_message(
    state: &mut State,
    id: Uuid,
    node: &Node,
    room: &Room,
    message: Message,
    backfill: &mut Option<Backfill>,
) -> Result<()> {
    match &message {
        Message::Ping(bytes) => {
            if let Some(peer) = state.peers.get(&id) {
                send(peer, Message::Pong(bytes.clone()));
            }
            return Ok(());
        }
        Message::Pong(_) => return Ok(()),
        Message::Text(text) if text == "ping" => {
            if let Some(peer) = state.peers.get(&id) {
                send(peer, Message::Text("pong".into()));
            }
            return Ok(());
        }
        _ => {}
    }
    match room {
        Room::Registry => {
            let Message::Text(text) = message else {
                bail!("Registry frames must be text");
            };
            ensure!(text.len() <= 1_000_000, "Registry frame too large");
            let frame: Value = serde_json::from_str(&text)?;
            if let Some(device) = frame.get("device").and_then(Value::as_str) {
                ensure!(device == node.device_id, "Device identity mismatch");
            }
            match frame.get("t").and_then(Value::as_str) {
                Some("hello") => {
                    let mut value = state
                        .store
                        .registry_rows(frame.get("cursor").and_then(Value::as_u64))?;
                    value["t"] = json!("state");
                    value["presence"] = json!(state.presence);
                    let peer = state.peers.get_mut(&id).unwrap();
                    peer.ready = true;
                    send(peer, Message::Text(value.to_string()));
                }
                Some("push") => {
                    let push = serde_json::from_value::<RegistryPush>(frame);
                    let valid = state.peers.get(&id).is_some_and(|p| p.ready)
                        && push
                            .as_ref()
                            .is_ok_and(|push| crate::store::validate_registry_push(push).is_ok());
                    if !valid {
                        send(state.peers.get(&id).unwrap(),Message::Text(json!({"t":"error","code":"invalid_op","message":"Invalid registry batch"}).to_string()));
                        return Ok(());
                    }
                    let (mut ack, broadcast) = state.store.registry_push(push?)?;
                    if let Some(value) = broadcast {
                        broadcast_room(state, room, Message::Text(value.to_string()), None);
                    }
                    ack["t"] = json!("ack");
                    if let Some(peer) = state.peers.get(&id) {
                        send(peer, Message::Text(ack.to_string()));
                    }
                }
                Some("presence") => {
                    ensure!(
                        state.peers.get(&id).is_some_and(|p| p.ready),
                        "Hello required"
                    );
                    presence(state, &node.device_id, Some(id));
                }
                Some("probe") => {
                    if let Some(peer) = state.peers.get(&id) {
                        send(
                            peer,
                            Message::Text(
                                json!({"t":"probe-ok","seq":state.store.registry_seq()?})
                                    .to_string(),
                            ),
                        );
                    }
                }
                _ => bail!("Unknown registry frame"),
            }
        }
        Room::Chat(chat) => {
            let Message::Binary(bytes) = message else {
                bail!("Chat frames must be binary");
            };
            let frame =
                chat_frames::decode(&bytes).ok_or_else(|| anyhow::anyhow!("Invalid chat frame"))?;
            if let Some(device) = frame.header.get("device").and_then(Value::as_str) {
                ensure!(device == node.device_id, "Device identity mismatch");
            }
            let ready = state.peers.get(&id).is_some_and(|p| p.ready);
            match frame.kind {
                ft::HELLO => {
                    let stats = state.store.chat_stats(chat)?;
                    let frontier = state.store.frontier(chat)?;
                    let peer = state.peers.get_mut(&id).unwrap();
                    peer.ready = true;
                    send(
                        peer,
                        Message::Binary(chat_frames::encode(ft::STATE, &stats, &frontier)),
                    );
                }
                ft::ROWS_REQ => {
                    ensure!(ready, "Hello required");
                    let after = frame
                        .header
                        .get("after")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    let exclude = (frame.header.get("excludeOwn") == Some(&Value::Bool(true)))
                        .then_some(node.device_id.as_str());
                    *backfill = Some(Backfill {
                        chat: chat.clone(),
                        after,
                        head: state.store.chat_stats(chat)?.head_seq,
                        exclude: exclude.map(str::to_owned),
                    });
                }
                ft::PUSH => {
                    let batch = frame
                        .header
                        .get("batchId")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let error = if ready {
                        chat_push_error(batch, &frame.payload)
                    } else {
                        Some("bad_push")
                    };
                    if let Some(code) = error {
                        send(
                            state.peers.get(&id).unwrap(),
                            Message::Binary(chat_frames::encode(
                                ft::ERROR,
                                &json!({"code":code,"batchId":batch,"message":"Invalid chat update"}),
                                &[],
                            )),
                        );
                        return Ok(());
                    }
                    let (seq, dup) =
                        state
                            .store
                            .append_chat(chat, &node.device_id, batch, &frame.payload)?;
                    state.store.remove_nudge(&node.device_id, chat)?;
                    if !dup {
                        broadcast_room(
                            state,
                            room,
                            Message::Binary(chat_frames::encode(
                                ft::ROW,
                                &json!({"seq":seq,"device":node.device_id,"batchId":batch}),
                                &frame.payload,
                            )),
                            Some(id),
                        );
                    }
                    send(
                        state.peers.get(&id).unwrap(),
                        Message::Binary(chat_frames::encode(
                            ft::ACK,
                            &json!({"batchId":batch,"seq":seq,"dup":dup}),
                            &[],
                        )),
                    );
                }
                ft::PRESENCE => {
                    ensure!(ready, "Hello required");
                    broadcast_room(
                        state,
                        room,
                        Message::Binary(chat_frames::encode(
                            ft::PRESENCE,
                            &json!({"device":node.device_id,"at":now_ms()}),
                            &frame.payload,
                        )),
                        Some(id),
                    );
                }
                ft::PROBE => {
                    send(
                        state.peers.get(&id).unwrap(),
                        Message::Binary(chat_frames::encode(
                            ft::PROBE_OK,
                            &json!({"headSeq":state.store.chat_stats(chat)?.head_seq}),
                            &[],
                        )),
                    );
                }
                _ => bail!("Unknown chat frame"),
            }
        }
        Room::Device { device, host, conn } => {
            let Message::Binary(bytes) = message else {
                return Ok(());
            };
            let (frame, payload) = decode_device_frame(&bytes)?;
            if *host {
                let Some(target) = frame.to else {
                    return Ok(());
                };
                if let Some(peer)=state.peers.values().find(|p|!p.close.is_cancelled() && matches!(&p.room,Room::Device{device:d,host:false,conn:c} if d==device && c==&target)){send(peer,Message::Binary(encode_device_frame(&DeviceFrameHeader::new(&frame.s,&frame.k),&payload)?));}
                else {relay_error(state.peers.get(&id).unwrap(),&frame.s,"client_gone",Some(&target),None);}
            } else if let Some(peer) = live_host(state, device) {
                send(
                    peer,
                    Message::Binary(encode_device_frame(
                        &DeviceFrameHeader {
                            s: frame.s,
                            k: frame.k,
                            to: None,
                            from: Some(conn.clone()),
                        },
                        &payload,
                    )?),
                );
            } else {
                relay_error(
                    state.peers.get(&id).unwrap(),
                    &frame.s,
                    "host_offline",
                    None,
                    None,
                );
            }
        }
    }
    Ok(())
}

fn chat_push_error(batch: &str, bytes: &[u8]) -> Option<&'static str> {
    if batch.is_empty() || batch.len() > 128 {
        Some("bad_push")
    } else if bytes.is_empty() {
        Some("empty")
    } else if bytes.len() > 1024 * 1024 {
        Some("too_large")
    } else {
        None
    }
}
