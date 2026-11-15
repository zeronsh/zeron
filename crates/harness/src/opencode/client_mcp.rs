//! Zeron's MCP servers on OpenCode's background service, over its client
//! MCP socket (`GET /api/experimental/client-mcp`, 2.x): the client hosts an
//! MCP server named after `name` for a directory, and its tools stay
//! available there while the socket is open (verified live, 2.3.0).
//!
//! A private server got Zeron's servers in its inline config, one server per
//! run, each stamped with its chat. The service is shared, so its MCP
//! servers can't be per-chat; but every `tools/call` says which session made
//! it (`_meta["ai.opencode/sessionID"]`, verified live, 2.3.0). So there is
//! one socket per directory and server name, and each call goes to the MCP
//! process of the chat that owns the calling session — started on first use
//! with that chat's own environment, as the private server would have.
//! Listing (initialize, `tools/list`) is answered by any one of them: every
//! chat's server offers the same tools.
//!
//! A session no Zeron chat owns (one started in the TUI, in a directory where
//! Zeron has a chat) is refused its calls rather than given another chat's
//! identity.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use zeron_proto::McpServer;

use crate::HarnessError;
use crate::jsonrpc::{Incoming, RpcClient};
use crate::process::{Child, Command, Stdio};

/// Where a client MCP call names the session that made it.
const SESSION_META: &str = "ai.opencode/sessionID";

/// The MCP protocol revision offered to a chat's server when the service's
/// own `initialize` didn't name one.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How long to wait before dialing a dropped socket again (doubling, capped).
const RECONNECT_FIRST: Duration = Duration::from_millis(500);
const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// How long a chat's MCP server may take to answer `initialize`.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// The routes from sessions to their chats' MCP servers, and the sockets
/// that offer those servers to the service.
#[derive(Clone, Default)]
pub(super) struct Routes {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Session id → its chat's servers, by name. Kept for the engine's life:
    /// a Zeron session continued in the TUI still reaches its chat's tools.
    sessions: Mutex<HashMap<String, HashMap<String, McpServer>>>,
    /// One socket per service, directory and server name.
    sockets: Mutex<HashMap<SocketKey, Socket>>,
    /// Live MCP processes, by session and server name.
    processes: tokio::sync::Mutex<HashMap<(String, String), Arc<Process>>>,
}

/// A socket's task, and whether the service has listed its tools yet.
struct Socket {
    task: tokio::task::JoinHandle<()>,
    listed: tokio::sync::watch::Receiver<bool>,
}

/// How long a run waits for the service to list a new socket's tools before
/// sending its prompt anyway.
const LISTED_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
struct SocketKey {
    base: String,
    directory: String,
    name: String,
}

/// Where to dial the service.
#[derive(Clone)]
pub(super) struct Service {
    pub base: String,
    pub auth: Option<String>,
}

impl Routes {
    /// Route `session`'s calls to `servers` (its chat's own Zeron server
    /// and its plugins'), and make sure the service has a socket for each in
    /// `directory` — waiting until it has listed their tools, so the prompt
    /// that follows can use them.
    pub async fn offer(
        &self,
        service: &Service,
        directory: &str,
        session: &str,
        servers: &[McpServer],
    ) {
        if servers.is_empty() {
            return;
        }
        {
            let mut sessions = self.inner.sessions.lock().expect("routes");
            let entry = sessions.entry(session.to_owned()).or_default();
            for server in servers {
                entry.insert(server.name.clone(), server.clone());
            }
        }
        let mut waiting = Vec::new();
        {
            let mut sockets = self.inner.sockets.lock().expect("sockets");
            for server in servers {
                let key = SocketKey {
                    base: service.base.clone(),
                    directory: directory.to_owned(),
                    name: server.name.clone(),
                };
                if let Some(socket) = sockets.get(&key)
                    && !socket.task.is_finished()
                {
                    waiting.push(socket.listed.clone());
                    continue;
                }
                let (listed_tx, listed) = tokio::sync::watch::channel(false);
                let task = tokio::spawn(hold_socket(
                    self.clone(),
                    service.clone(),
                    key.clone(),
                    listed_tx,
                ));
                waiting.push(listed.clone());
                sockets.insert(key, Socket { task, listed });
            }
        }
        for mut listed in waiting {
            let _ = tokio::time::timeout(LISTED_TIMEOUT, listed.wait_for(|listed| *listed)).await;
        }
    }

    /// Stop `session`'s MCP processes once the returned lease goes (its run
    /// ended). Its route stays, so a later call — the session continued in
    /// the TUI — starts them again.
    pub fn lease(&self, session: &str) -> Lease {
        Lease {
            routes: self.clone(),
            session: session.to_owned(),
        }
    }

    /// `session`'s server named `name`, if a Zeron chat owns the session.
    fn route(&self, session: &str, name: &str) -> Option<McpServer> {
        self.inner
            .sessions
            .lock()
            .expect("routes")
            .get(session)?
            .get(name)
            .cloned()
    }

    /// Any chat's server named `name`, to answer listing calls with.
    fn any(&self, name: &str) -> Option<(String, McpServer)> {
        let sessions = self.inner.sessions.lock().expect("routes");
        sessions.iter().find_map(|(session, servers)| {
            servers
                .get(name)
                .map(|server| (session.clone(), server.clone()))
        })
    }

    /// The live process for `session`'s server, started (and initialized)
    /// on first use.
    async fn process(
        &self,
        session: &str,
        server: &McpServer,
        protocol_version: &str,
    ) -> Result<Arc<Process>, HarnessError> {
        let key = (session.to_owned(), server.name.clone());
        let mut processes = self.inner.processes.lock().await;
        if let Some(process) = processes.get(&key)
            && !process.rpc.is_closed()
        {
            return Ok(process.clone());
        }
        let process = Arc::new(Process::start(server, protocol_version).await?);
        processes.insert(key, process.clone());
        Ok(process)
    }
}

/// A run's hold on its session's MCP processes ([`Routes::lease`]).
pub(super) struct Lease {
    routes: Routes,
    session: String,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let routes = self.routes.clone();
        let session = std::mem::take(&mut self.session);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let mut processes = routes.inner.processes.lock().await;
                processes.retain(|(owner, _), _| *owner != session);
            });
        }
    }
}

/// A chat's MCP server process, spoken to over its stdio.
struct Process {
    rpc: RpcClient,
    /// Killed when the last handle goes (`kill_on_drop`).
    _child: Mutex<Child>,
}

impl Process {
    async fn start(server: &McpServer, protocol_version: &str) -> Result<Self, HarnessError> {
        let mut cmd = Command::new(&server.command);
        for arg in &server.args {
            cmd.arg(arg);
        }
        for (key, value) in &server.env {
            cmd.env(key, value);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            HarnessError::Protocol(format!("could not start MCP server {}: {e}", server.name))
        })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(HarnessError::Protocol(format!(
                "MCP server {} has no stdio",
                server.name
            )));
        };
        let (rpc, mut incoming) = RpcClient::new(stdin, stdout);
        // (It may ask things of its client — roots, sampling — that a call
        // routed through here can't answer.)
        let asked = rpc.clone();
        tokio::spawn(async move {
            while let Some(message) = incoming.recv().await {
                match message {
                    Incoming::Request { id, method, .. } => {
                        asked.respond_error(
                            &id,
                            -32601,
                            &format!("{method} is not supported here"),
                        );
                    }
                    Incoming::Eof => break,
                    Incoming::Notification { .. } => {}
                }
            }
        });
        let params = json!({
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": { "name": "zeron-opencode", "version": env!("CARGO_PKG_VERSION") },
        });
        tokio::time::timeout(HANDSHAKE_TIMEOUT, rpc.request("initialize", params))
            .await
            .map_err(|_| {
                HarnessError::Protocol(format!("MCP server {} did not initialize", server.name))
            })??;
        rpc.notify("notifications/initialized", None);
        Ok(Self {
            rpc,
            _child: Mutex::new(child),
        })
    }
}

/// Keep `key`'s socket open, dialing again when it drops, for as long as a
/// chat routes to a server of that name.
async fn hold_socket(
    routes: Routes,
    service: Service,
    key: SocketKey,
    listed: tokio::sync::watch::Sender<bool>,
) {
    let mut backoff = RECONNECT_FIRST;
    loop {
        if routes.any(&key.name).is_none() {
            return;
        }
        match serve_socket(&routes, &service, &key, &listed).await {
            // (A socket that served and then closed is dialed again at once.)
            Ok(()) => backoff = RECONNECT_FIRST,
            Err(error) => {
                tracing::debug!(
                    target: "zeron_harness::opencode",
                    name = %key.name,
                    directory = %key.directory,
                    %error,
                    "client MCP socket dropped"
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(RECONNECT_MAX);
            }
        }
    }
}

/// One socket's life: answer what the service asks until it closes.
async fn serve_socket(
    routes: &Routes,
    service: &Service,
    key: &SocketKey,
    listed: &tokio::sync::watch::Sender<bool>,
) -> Result<(), String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::{HeaderValue, header};

    let url = socket_url(&service.base, &key.directory, &key.name);
    let mut request = url.into_client_request().map_err(|e| e.to_string())?;
    if let Some(auth) = &service.auth {
        let value = HeaderValue::from_str(auth).map_err(|e| e.to_string())?;
        request.headers_mut().insert(header::AUTHORIZATION, value);
    }
    let (socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .map_err(|_| "timed out dialing".to_owned())?
    .map_err(|e| e.to_string())?;
    let (mut sink, mut stream) = socket.split();
    let (reply_tx, mut reply_rx) = tokio::sync::mpsc::unbounded_channel::<(Value, bool)>();
    let mut protocol_version = PROTOCOL_VERSION.to_owned();
    loop {
        tokio::select! {
            Some((reply, lists_tools)) = reply_rx.recv() => {
                sink.send(Message::text(reply.to_string())).await.map_err(|e| e.to_string())?;
                if lists_tools {
                    listed.send_replace(true);
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else { return Ok(()) };
                let text = match frame.map_err(|e| e.to_string())? {
                    Message::Text(text) => text.to_string(),
                    Message::Ping(data) => {
                        sink.send(Message::Pong(data)).await.map_err(|e| e.to_string())?;
                        continue;
                    }
                    Message::Close(_) => return Ok(()),
                    _ => continue,
                };
                let Ok(message) = serde_json::from_str::<Value>(&text) else { continue };
                let Some(method) = message.get("method").and_then(Value::as_str) else {
                    continue;
                };
                // Notifications need no answer; the chats' servers had theirs
                // at their own handshake.
                let Some(id) = message.get("id").cloned() else { continue };
                // (A listing comes with no params; an MCP server may want an
                // object there all the same.)
                let params = message
                    .get("params")
                    .filter(|params| !params.is_null())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if method == "initialize"
                    && let Some(version) = params.get("protocolVersion").and_then(Value::as_str)
                {
                    protocol_version = version.to_owned();
                }
                let routes = routes.clone();
                let name = key.name.clone();
                let method = method.to_owned();
                let version = protocol_version.clone();
                let reply_tx = reply_tx.clone();
                // (Each call answers on its own: a slow tool must not hold up
                // the next listing.)
                tokio::spawn(async move {
                    let lists_tools = method == "tools/list";
                    let reply = match answer(&routes, &name, &method, params, &version).await {
                        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                        Err(message) => json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32603, "message": message },
                        }),
                    };
                    let _ = reply_tx.send((reply, lists_tools));
                });
            }
        }
    }
}

/// What the service asked of the server called `name`: a tool call goes to
/// the calling session's chat, anything else to any chat's server.
async fn answer(
    routes: &Routes,
    name: &str,
    method: &str,
    params: Value,
    protocol_version: &str,
) -> Result<Value, String> {
    let (session, server) = if method == "tools/call" {
        let session = calling_session(&params).ok_or("the call names no session")?;
        let server = routes
            .route(&session, name)
            .ok_or("this session isn't a Zeron chat, so Zeron's tools can't act for it")?;
        (session, server)
    } else {
        routes.any(name).ok_or("no Zeron chat offers this server")?
    };
    let process = routes
        .process(&session, &server, protocol_version)
        .await
        .map_err(|e| e.to_string())?;
    if method == "initialize" {
        // (The chat's server already had its own handshake; this answers
        // with what it said it can do.)
        return Ok(json!({
            "protocolVersion": protocol_version,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": name, "version": env!("CARGO_PKG_VERSION") },
        }));
    }
    process
        .rpc
        .request(method, params)
        .await
        .map_err(|e| e.to_string())
}

/// The session a client MCP call came from.
fn calling_session(params: &Value) -> Option<String> {
    params
        .get("_meta")?
        .get(SESSION_META)?
        .as_str()
        .filter(|session| !session.is_empty())
        .map(str::to_owned)
}

/// `ws://…/api/experimental/client-mcp?location[directory]=…&name=…`.
fn socket_url(base: &str, directory: &str, name: &str) -> String {
    let base = base.trim_end_matches('/');
    let base = base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            base.strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
        .unwrap_or_else(|| base.to_owned());
    format!(
        "{base}/api/experimental/client-mcp?location%5Bdirectory%5D={}&name={}",
        super::encode_directory(directory),
        super::encode_directory(name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_names_its_session_in_meta() {
        let params = json!({
            "name": "whoami",
            "arguments": {},
            "_meta": { "ai.opencode/sessionID": "ses_abc", "progressToken": 2 },
        });
        assert_eq!(calling_session(&params).as_deref(), Some("ses_abc"));
        assert_eq!(calling_session(&json!({ "name": "whoami" })), None);
        assert_eq!(
            calling_session(&json!({ "_meta": { "ai.opencode/sessionID": "" } })),
            None
        );
    }

    #[test]
    fn the_socket_url_scopes_to_a_directory() {
        assert_eq!(
            socket_url("http://127.0.0.1:49374/", "/Users/a b", "zeron"),
            "ws://127.0.0.1:49374/api/experimental/client-mcp?location%5Bdirectory%5D=/Users/a%20b&name=zeron"
        );
        assert!(socket_url("https://host", "/d", "z").starts_with("wss://host/"));
    }

    #[test]
    fn calls_go_only_to_the_chat_that_owns_the_session() {
        let routes = Routes::default();
        let server = |chat: &str| McpServer {
            name: "zeron".into(),
            command: "zeron".into(),
            args: vec!["mcp".into()],
            env: [("ZERON_CHAT_ID".to_owned(), chat.to_owned())].into(),
        };
        routes.inner.sessions.lock().unwrap().insert(
            "ses_a".into(),
            [("zeron".to_owned(), server("chat-a"))].into(),
        );
        assert_eq!(
            routes.route("ses_a", "zeron").unwrap().env["ZERON_CHAT_ID"],
            "chat-a"
        );
        // (A session no chat owns gets nothing, never another chat's server.)
        assert!(routes.route("ses_tui", "zeron").is_none());
        assert!(routes.any("zeron").is_some());
        assert!(routes.any("plugin").is_none());
    }
}
