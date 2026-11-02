//! Instrumented WebSocket RPC driver. Byte counts are WebSocket message payload
//! bytes (including the RPC envelope), excluding WebSocket/TCP headers.
use anyhow::{Context as _, Result, bail};
use futures::{SinkExt, Stream, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{net::TcpStream, sync::mpsc, task::JoinHandle};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{StatusCode, header},
        protocol::WebSocketConfig,
    },
};
use zeron_rpc::methods;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub type RttNs = u64;
#[derive(Debug, Clone)]
pub struct Item {
    pub t_recv_ns: u64,
    pub bytes: usize,
    pub value: Value,
}

/// A multiplexed connection, shared by calls and subscriptions like the UI.
#[derive(Clone)]
pub struct Client {
    inner: std::sync::Arc<Inner>,
}
struct Inner {
    out: mpsc::UnboundedSender<Command>,
    next_id: std::sync::atomic::AtomicU64,
    task: JoinHandle<()>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.task.abort();
    }
}
enum Command {
    Request {
        id: u64,
        frame: String,
        tx: mpsc::UnboundedSender<Result<Wire>>,
    },
    Cancel(u64),
    Forget(u64),
}
struct Wire {
    frame: Value,
    t: u64,
    bytes: usize,
}
impl Client {
    pub async fn connect(port: u16, data_dir: impl AsRef<Path>) -> Result<Self> {
        let url = format!("ws://127.0.0.1:{port}");
        let tokens = zeron_rpc::ipc_auth::client_tokens(Some(data_dir.as_ref()));
        let candidates: Vec<Option<&str>> = if tokens.is_empty() {
            vec![None]
        } else {
            tokens.iter().map(|s| Some(s.as_str())).collect()
        };
        let mut socket = None;
        for token in candidates {
            let mut request = url.as_str().into_client_request()?;
            if let Some(token) = token {
                let Ok(value) = format!("Bearer {token}").parse() else {
                    continue;
                };
                request.headers_mut().insert(header::AUTHORIZATION, value);
            }
            let config = WebSocketConfig {
                max_message_size: Some(512 << 20),
                max_frame_size: Some(512 << 20),
                ..Default::default()
            };
            match tokio::time::timeout(
                Duration::from_secs(5),
                tokio_tungstenite::connect_async_with_config(request, Some(config), true),
            )
            .await?
            {
                Ok((ws, _)) => {
                    socket = Some(ws);
                    break;
                }
                Err(tokio_tungstenite::tungstenite::Error::Http(response))
                    if response.status() == StatusCode::UNAUTHORIZED =>
                {
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }
        let socket = socket.context("engine rejected IPC credentials")?;
        let (out, commands) = mpsc::unbounded_channel();
        let task = tokio::spawn(pump(socket, commands));
        Ok(Self {
            inner: std::sync::Arc::new(Inner {
                out,
                next_id: std::sync::atomic::AtomicU64::new(1),
                task,
            }),
        })
    }
    fn request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(u64, mpsc::UnboundedReceiver<Result<Wire>>)> {
        let id = self
            .inner
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        let frame = json!({"id":id,"method":method,"params":params}).to_string();
        self.inner
            .out
            .send(Command::Request { id, frame, tx })
            .map_err(|_| anyhow::anyhow!("RPC connection closed"))?;
        Ok((id, rx))
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<(Value, RttNs)> {
        let start = crate::mono_now();
        let (id, mut rx) = self.request(method, params)?;
        let result = async {
            loop {
                let wire = rx.recv().await.context("RPC connection closed")??;
                if let Some(err) = wire.frame.get("err") {
                    bail!("{method}: {err}");
                }
                if let Some(ok) = wire.frame.get("ok") {
                    return Ok((ok.clone(), wire.t.saturating_sub(start)));
                }
                if wire.frame.get("done").is_some() {
                    bail!("{method}: ended without reply");
                }
            }
        }
        .await;
        let _ = self.inner.out.send(Command::Forget(id));
        result
    }
    /// Wait for the first item or explicit readiness, surfacing immediate server errors.
    pub async fn subscribe(&self, method: &str, params: Value) -> Result<Subscription> {
        let (id, rx) = self.request(method, params)?;
        let mut subscription = Subscription {
            id,
            rx,
            client: self.clone(),
            first: None,
        };
        let wire = subscription
            .rx
            .recv()
            .await
            .context("RPC connection closed")??;
        if let Some(err) = wire.frame.get("err") {
            bail!("{method}: {err}");
        }
        if let Some(value) = wire.frame.get("item") {
            subscription.first = Some(Ok(Item {
                t_recv_ns: wire.t,
                bytes: wire.bytes,
                value: value.clone(),
            }));
        } else if wire.frame.get("ok").is_none() {
            bail!("{method}: stream ended before readiness");
        }
        Ok(subscription)
    }
}
async fn pump(mut socket: Socket, mut commands: mpsc::UnboundedReceiver<Command>) {
    let mut pending: BTreeMap<u64, mpsc::UnboundedSender<Result<Wire>>> = BTreeMap::new();
    let error = loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Request { id, frame, tx }) => {
                    pending.insert(id, tx);
                    if let Err(e) = socket.send(Message::Text(frame)).await { break e.to_string(); }
                }
                Some(Command::Forget(id)) => { pending.remove(&id); }
                Some(Command::Cancel(id)) => {
                    if pending.remove(&id).is_some() && let Err(e) = socket.send(Message::Text(json!({"id":id,"cancel":true}).to_string())).await { break e.to_string(); }
                }
                None => break "RPC client dropped".into(),
            },
            message = socket.next() => {
                // Take the timestamp and payload length immediately after socket receive,
                // before decoding the RPC envelope.
                let t = crate::mono_now();
                match message {
                    Some(Ok(Message::Text(text))) => {
                        let bytes = text.len();
                        let frame: Value = match serde_json::from_str(&text) { Ok(v) => v, Err(e) => break format!("invalid RPC frame: {e}") };
                        let Some(id) = frame.get("id").and_then(Value::as_u64) else { break "RPC frame missing id".into(); };
                        let terminal = frame.get("err").is_some() || frame.get("done").is_some();
                        if let Some(tx) = pending.get(&id) { let _ = tx.send(Ok(Wire { frame, t, bytes })); }
                        if terminal { pending.remove(&id); }
                    }
                    Some(Ok(Message::Close(_))) | None => break "RPC socket closed".into(),
                    Some(Err(e)) => break e.to_string(),
                    _ => {}
                }
            }
        }
    };
    for (_, tx) in pending {
        let _ = tx.send(Err(anyhow::anyhow!("{error}")));
    }
}
pub struct Subscription {
    id: u64,
    rx: mpsc::UnboundedReceiver<Result<Wire>>,
    client: Client,
    first: Option<Result<Item>>,
}
impl Subscription {
    pub async fn recv(&mut self) -> Option<Result<Item>> {
        self.next().await
    }
}
impl Stream for Subscription {
    type Item = Result<Item>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(item) = self.first.take() {
            return Poll::Ready(Some(item));
        }
        loop {
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(Ok(wire))) => {
                    if let Some(err) = wire.frame.get("err") {
                        return Poll::Ready(Some(Err(anyhow::anyhow!("{err}"))));
                    }
                    if wire.frame.get("done").is_some() {
                        return Poll::Ready(None);
                    }
                    if let Some(value) = wire.frame.get("item") {
                        return Poll::Ready(Some(Ok(Item {
                            t_recv_ns: wire.t,
                            bytes: wire.bytes,
                            value: value.clone(),
                        })));
                    }
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.client.inner.out.send(Command::Cancel(self.id));
    }
}

pub const GLOBAL: &[&str] = &[
    methods::WATCH_CHATS,
    methods::WATCH_SESSIONS,
    methods::WATCH_SPACES,
    methods::WATCH_DEVICES,
    methods::WATCH_TRANSFERS,
    methods::WATCH_SIDEBAR_PREFERENCES,
    methods::WATCH_CONNECTIVITY,
    methods::WATCH_HARNESS_UPDATES,
    methods::WATCH_PREVIEWS,
];
pub const PER_CHAT: &[&str] = &[
    methods::FOCUS_CHAT,
    methods::WATCH_DOC_MESSAGES,
    methods::WATCH_QUEUE,
    methods::WATCH_CHECKOUT_DIFFS,
    methods::WATCH_CHECKOUT_CHANGE_REQUEST,
];

/// Local checkout context for the change-request stream. For a remote chat,
/// supply its device ID as `target_device_id`.
#[derive(Debug, Clone)]
pub struct Checkout {
    pub cwd: String,
    pub branch: Option<String>,
    pub target_device_id: Option<String>,
}
pub struct UiSet {
    pub client: Client,
    pub global: BTreeMap<&'static str, Subscription>,
    pub chats: BTreeMap<String, BTreeMap<&'static str, Subscription>>,
    pub checkout: Checkout,
}
impl UiSet {
    pub fn new(client: Client, checkout: Checkout) -> Self {
        Self {
            client,
            checkout,
            global: BTreeMap::new(),
            chats: BTreeMap::new(),
        }
    }
    pub async fn open_global(&mut self) -> Result<()> {
        let mut opened = BTreeMap::new();
        // WatchPreviews actually requires a chatId and is opened with the chat.
        for &method in GLOBAL.iter().filter(|&&m| m != methods::WATCH_PREVIEWS) {
            opened.insert(method, self.client.subscribe(method, json!({})).await?);
        }
        self.global = opened;
        Ok(())
    }
    pub async fn open_chat(&mut self, id: &str) -> Result<()> {
        self.client
            .call(methods::FOCUS_CHAT, json!({"chatId": id}))
            .await?;
        let mut opened = BTreeMap::new();
        for (method, params) in [
            (
                methods::WATCH_DOC_MESSAGES,
                json!({"chatId":id,"openingTail":true}),
            ),
            (methods::WATCH_QUEUE, json!({"chatId":id})),
            (
                methods::WATCH_CHECKOUT_DIFFS,
                match &self.checkout.target_device_id {
                    Some(d) => json!({"targetDeviceId":d}),
                    None => json!({}),
                },
            ),
            (methods::WATCH_CHECKOUT_CHANGE_REQUEST, {
                let mut p = json!({"cwd":self.checkout.cwd,"branch":self.checkout.branch});
                if let Some(d) = &self.checkout.target_device_id {
                    p["targetDeviceId"] = json!(d);
                }
                p
            }),
            (methods::WATCH_PREVIEWS, json!({"chatId":id})),
        ] {
            opened.insert(method, self.client.subscribe(method, params).await?);
        }
        self.chats.insert(id.to_owned(), opened);
        Ok(())
    }
    pub fn close_chat(&mut self, id: &str) {
        self.chats.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn multiplexes_and_measures_raw_payloads() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
            let first = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let request: Value = serde_json::from_str(&first).unwrap();
            let id = request["id"].as_u64().unwrap();
            let payload = json!({"id":id,"item":{"text":"é [[zb:abcdef12:3]]"}}).to_string();
            let bytes = payload.len();
            ws.send(Message::Text(payload)).await.unwrap();
            let second = ws.next().await.unwrap().unwrap().into_text().unwrap();
            let request: Value = serde_json::from_str(&second).unwrap();
            ws.send(Message::Text(
                json!({"id":request["id"],"ok":null}).to_string(),
            ))
            .await
            .unwrap();
            let cancel = ws.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(serde_json::from_str::<Value>(&cancel).unwrap()["cancel"] == true);
            bytes
        });
        let client = Client::connect(port, "/nonexistent").await.unwrap();
        let mut stream = client.subscribe("Watch", json!({})).await.unwrap();
        let item = stream.recv().await.unwrap().unwrap();
        assert!(item.t_recv_ns > 0);
        assert_eq!(
            crate::transcript::MarkerScan::default()
                .scan(item.value.to_string().as_bytes(), item.t_recv_ns),
            vec![("abcdef12".to_owned(), 3, item.t_recv_ns)]
        );
        let (reply, rtt) = client.call("Call", json!({})).await.unwrap();
        assert!(reply.is_null());
        assert!(rtt > 0);
        drop(stream);
        assert_eq!(item.bytes, server.await.unwrap());
    }
    #[test]
    fn ui_watch_drift() {
        let state = include_str!("../../ui/src/state.rs");
        let shell = include_str!("../../ui/src/shell/session_info.rs");
        let expected = [
            "WATCH_CHATS",
            "WATCH_SESSIONS",
            "WATCH_SPACES",
            "WATCH_DEVICES",
            "WATCH_TRANSFERS",
            "WATCH_SIDEBAR_PREFERENCES",
            "WATCH_CONNECTIVITY",
            "WATCH_HARNESS_UPDATES",
            "WATCH_PREVIEWS",
            "WATCH_DOC_MESSAGES",
            "WATCH_QUEUE",
            "WATCH_CHECKOUT_DIFFS",
            "WATCH_CHECKOUT_CHANGE_REQUEST",
        ];
        let mut actual = std::collections::BTreeSet::new();
        for source in [state, shell] {
            for tail in source.split("methods::WATCH_").skip(1) {
                let suffix: String = tail
                    .chars()
                    .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                    .collect();
                actual.insert(format!("WATCH_{suffix}"));
            }
        }
        assert_eq!(actual, expected.into_iter().map(String::from).collect());
        assert!(state.contains("methods::FOCUS_CHAT"));
        let rpc = include_str!("../../rpc/src/lib.rs");
        let declared: std::collections::BTreeSet<&str> = expected
            .iter()
            .map(|name| {
                let prefix = format!("pub const {name}: &str = \"");
                rpc.split_once(&prefix)
                    .expect("missing RPC constant")
                    .1
                    .split_once("\"")
                    .unwrap()
                    .0
            })
            .collect();
        let configured: std::collections::BTreeSet<&str> = GLOBAL
            .iter()
            .chain(PER_CHAT)
            .copied()
            .filter(|m| *m != methods::FOCUS_CHAT)
            .collect();
        assert_eq!(declared, configured);
    }
}
