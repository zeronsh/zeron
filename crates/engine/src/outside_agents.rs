//! Outside agents (docs/mcp.md, "Outside agents"): an agent outside Zeron
//! (Grok, say) using this device's Zeron tools as a remote MCP server,
//! through the edge.
//!
//! The person lets one in on the edge's sign-in page, naming this device;
//! the edge asks here over the device room ([`zeron_rpc::MCP_KIND`] frames)
//! and this device's app shows the ask (Settings → MCP). Allowed, this
//! device makes the grant's id and keeps it (`outside-agents.json` in the
//! account's store), and only then does the edge give the agent a code.
//! Each of the agent's requests comes over the room naming its grant: one
//! this device doesn't keep (revoked here, or never made here) runs nothing,
//! and the edge drops the agent's tokens when told. A kept one runs through
//! the agent MCP endpoint's own JSON-RPC ([`McpHttpServer::relay`]): the
//! agent tool set, speaking for no chat, so with the least a calling chat
//! can have (no commands; chats it makes are read-only and supervised).
//!
//! Frames (JSON, kind `mcp`; `s` names the ask or the call):
//! - room → here: `{t: "ask", client, redirectHost}`, `{t: "call", grant,
//!   headers, body}`, `{t: "cancel"}`;
//! - here → room: `{t: "answer", grant}` or `{t: "answer", denied: true}`;
//!   `{t: "head", status, headers}`, then `{t: "data", data}` and `{t:
//!   "end"}`, for a call; `{t: "grants", grants}` on joining the room, and
//!   `{t: "revoked", grant}`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use bytes::Bytes;
use futures::stream::BoxStream;
use http_body_util::BodyExt;
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::watch;
use zeron_mcp::http::{McpHttpServer, RemoteGrant};
use zeron_rpc::{McpOut, McpRelay, RpcError, RpcReply, methods, parse_params};

use crate::{new_id, now_ms};

/// How long an ask waits for the person (the edge's own wait is the same).
const ASK_TTL: Duration = Duration::from_secs(5 * 60);
/// Asks waiting at once; past this a new one is declined.
const MAX_ASKS: usize = 5;
/// Outside agents' calls running at once on this device.
const MAX_CALLS: usize = 16;
/// What an ask may say of its client, at most.
const MAX_CLIENT: usize = 100;
const MAX_HOST: usize = 253;
/// The most of a response one frame carries (the relay's own cap is higher).
const DATA_CHUNK: usize = 256 << 10;
/// The request headers a relayed call keeps; nothing else is read.
const CALL_HEADERS: [&str; 5] = ["accept", "content-type", "mcp-protocol-version", "mcp-method", "mcp-name"];
const STORE_VERSION: u32 = 1;

/// RPC methods handled by [`OutsideAgents`].
pub fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::WATCH_OUTSIDE_AGENTS | methods::RESOLVE_OUTSIDE_AGENT | methods::REVOKE_OUTSIDE_AGENT
    )
}

/// An outside agent this device let in.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutsideGrant {
    pub id: String,
    /// The name the agent registered with the edge (its own say-so).
    pub client: String,
    /// Where its sign-in returned to.
    pub redirect_host: String,
    pub created_at: i64,
}

/// An outside agent waiting for the person.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OutsideAsk {
    pub id: String,
    pub client: String,
    pub redirect_host: String,
    pub at: i64,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct OutsideAgentsView {
    pub grants: Vec<OutsideGrant>,
    pub asks: Vec<OutsideAsk>,
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    grants: Vec<OutsideGrant>,
}

/// The agent MCP endpoint, once this engine serves one.
pub type EndpointSource = Arc<dyn Fn() -> Option<Arc<McpHttpServer>> + Send + Sync>;

#[derive(Clone)]
pub struct OutsideAgents {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    device_id: String,
    endpoint: EndpointSource,
    state: Mutex<State>,
    view: watch::Sender<OutsideAgentsView>,
}

#[derive(Default)]
struct State {
    grants: Vec<OutsideGrant>,
    asks: Vec<OutsideAsk>,
    /// The room, as of the relay's latest join.
    out: Option<McpOut>,
    /// The kept grants' handles on the endpoint they were made on.
    live: Option<Live>,
    calls: HashMap<String, tokio::task::AbortHandle>,
}

struct Live {
    endpoint: Weak<McpHttpServer>,
    grants: HashMap<String, RemoteGrant>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Trimmed, without control characters, at most `max` characters.
fn clean(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).collect::<String>().trim().chars().take(max).collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolveParams {
    id: String,
    allow: bool,
}

#[derive(Deserialize)]
struct RevokeParams {
    id: String,
}

impl OutsideAgents {
    /// Grants kept at `path`, served on whatever `endpoint` answers.
    pub fn new(path: PathBuf, device_id: impl Into<String>, endpoint: EndpointSource) -> Self {
        let grants = std::fs::read(&path)
            .ok()
            .and_then(|bytes| match serde_json::from_slice::<StoreFile>(&bytes) {
                Ok(file) => Some(file.grants),
                Err(err) => {
                    tracing::warn!(error = %err, "outside agents: unreadable grants file; keeping none");
                    None
                }
            })
            .unwrap_or_default();
        let (view, _) = watch::channel(OutsideAgentsView {
            grants: grants.clone(),
            asks: Vec::new(),
        });
        Self {
            inner: Arc::new(Inner {
                path,
                device_id: device_id.into(),
                endpoint,
                state: Mutex::new(State {
                    grants,
                    ..State::default()
                }),
                view,
            }),
        }
    }

    pub fn snapshot(&self) -> OutsideAgentsView {
        self.inner.view.borrow().clone()
    }

    pub async fn handle_rpc(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::WATCH_OUTSIDE_AGENTS => Ok(RpcReply::Stream(self.watch_stream())),
            methods::RESOLVE_OUTSIDE_AGENT => {
                let params: ResolveParams = parse_params(params)?;
                self.resolve(&params.id, params.allow).await.map_err(RpcError::Failed)?;
                RpcReply::value(&self.snapshot())
            }
            methods::REVOKE_OUTSIDE_AGENT => {
                let params: RevokeParams = parse_params(params)?;
                self.revoke(&params.id).await.map_err(RpcError::Failed)?;
                RpcReply::value(&self.snapshot())
            }
            other => Err(RpcError::UnknownMethod(other.into())),
        }
    }

    fn watch_stream(&self) -> BoxStream<'static, Value> {
        let rx = self.inner.view.subscribe();
        Box::pin(futures::stream::unfold((rx, true), |(mut rx, first)| async move {
            if !first {
                rx.changed().await.ok()?;
            }
            let view = serde_json::to_value(&*rx.borrow_and_update()).ok()?;
            Some((view, (rx, false)))
        }))
    }

    fn publish(&self, state: &State) {
        self.inner.view.send_replace(OutsideAgentsView {
            grants: state.grants.clone(),
            asks: state.asks.clone(),
        });
    }

    fn persist(&self, grants: &[OutsideGrant]) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(&StoreFile {
            version: STORE_VERSION,
            grants: grants.to_vec(),
        })
        .map_err(|e| format!("serialize outside agents: {e}"))?;
        bytes.push(b'\n');
        crate::runner::write_private_atomic(&self.inner.path, &bytes)
            .map_err(|e| format!("couldn't save outside agents: {e}"))
    }

    /// The person's answer to ask `id`. Allowed, the grant is kept before
    /// the edge hears of it; if the edge can't be told, it isn't kept.
    pub async fn resolve(&self, id: &str, allow: bool) -> Result<(), String> {
        let (ask, out) = {
            let mut state = lock(&self.inner.state);
            let Some(at) = state.asks.iter().position(|ask| ask.id == id) else {
                return Err("That request isn't waiting anymore.".into());
            };
            let Some(out) = state.out.clone() else {
                return Err("This device isn't connected to Zeron right now. Try again once it is.".into());
            };
            let ask = state.asks.remove(at);
            self.publish(&state);
            (ask, out)
        };
        let denied = json!({ "t": "answer", "denied": true }).to_string();
        if !allow {
            out.send(&ask.id, denied.as_bytes()).await;
            return Ok(());
        }
        let grant = OutsideGrant {
            id: new_id(),
            client: ask.client.clone(),
            redirect_host: ask.redirect_host.clone(),
            created_at: now_ms(),
        };
        let kept = {
            let mut state = lock(&self.inner.state);
            let mut grants = state.grants.clone();
            grants.push(grant.clone());
            let kept = self.persist(&grants);
            if kept.is_ok() {
                state.grants = grants;
                self.add_live(&mut state, &grant.id);
                self.publish(&state);
            }
            kept
        };
        if let Err(err) = kept {
            out.send(&ask.id, denied.as_bytes()).await;
            return Err(err);
        }
        let answer = json!({ "t": "answer", "grant": grant.id });
        if !out.send(&ask.id, answer.to_string().as_bytes()).await {
            // The edge never heard: no agent holds this grant, so none is kept.
            let _ = self.forget(&grant.id);
            return Err("This device lost its connection to Zeron before the agent could be let in.".into());
        }
        tracing::info!("outside agents: a grant was made");
        Ok(())
    }

    /// Take grant `id` back: its calls end, and the edge drops its tokens.
    pub async fn revoke(&self, id: &str) -> Result<(), String> {
        if !self.forget(id)? {
            return Err("No outside agent has that grant.".into());
        }
        let out = lock(&self.inner.state).out.clone();
        if let Some(out) = out {
            // Not connected: the next join's grant list tells the edge.
            out.send("grants", json!({ "t": "revoked", "grant": id }).to_string().as_bytes()).await;
        }
        tracing::info!("outside agents: a grant was revoked");
        Ok(())
    }

    /// Drop grant `id` here; false when there was none.
    fn forget(&self, id: &str) -> Result<bool, String> {
        let removed = {
            let mut state = lock(&self.inner.state);
            if !state.grants.iter().any(|grant| grant.id == id) {
                return Ok(false);
            }
            let grants: Vec<OutsideGrant> = state.grants.iter().filter(|grant| grant.id != id).cloned().collect();
            self.persist(&grants)?;
            state.grants = grants;
            let removed = state.live.as_mut().and_then(|live| live.grants.remove(id));
            self.publish(&state);
            removed
        };
        // Dropped outside the lock: it ends the grant's calls in flight.
        drop(removed);
        Ok(true)
    }

    fn add_live(&self, state: &mut State, id: &str) {
        if let Some(live) = state.live.as_mut()
            && let Some(endpoint) = live.endpoint.upgrade()
        {
            live.grants.insert(id.to_owned(), endpoint.remote_grant(id, self.origin()));
        }
    }

    /// What an outside agent's tools speak as: this device, and no chat.
    /// `outside` is what keeps the shell off the list and a new chat at
    /// read-only and supervised; the engine's agent gate caps the same
    /// calls again, because this chat id is one it doesn't know.
    fn origin(&self) -> zeron_mcp::Origin {
        zeron_mcp::Origin {
            chat_id: None,
            device_id: Some(self.inner.device_id.clone()),
            outside: true,
            ..Default::default()
        }
    }

    /// The endpoint, with a handle on it for every kept grant (and no other).
    fn endpoint(&self) -> Option<Arc<McpHttpServer>> {
        let endpoint = (self.inner.endpoint)()?;
        let mut state = lock(&self.inner.state);
        let current = state
            .live
            .as_ref()
            .is_some_and(|live| std::ptr::eq(live.endpoint.as_ptr(), Arc::as_ptr(&endpoint)));
        if !current {
            let grants = state
                .grants
                .iter()
                .map(|grant| (grant.id.clone(), endpoint.remote_grant(&grant.id, self.origin())))
                .collect();
            state.live = Some(Live {
                endpoint: Arc::downgrade(&endpoint),
                grants,
            });
        }
        Some(endpoint)
    }

    fn ask(&self, stream: String, payload: &Value, out: McpOut) {
        let client = clean(payload["client"].as_str().unwrap_or_default(), MAX_CLIENT);
        let redirect_host = clean(payload["redirectHost"].as_str().unwrap_or_default(), MAX_HOST);
        let mut state = lock(&self.inner.state);
        if state.asks.iter().any(|ask| ask.id == stream) {
            return;
        }
        if client.is_empty() || redirect_host.is_empty() || state.asks.len() >= MAX_ASKS {
            drop(state);
            tokio::spawn(async move {
                out.send(&stream, json!({ "t": "answer", "denied": true }).to_string().as_bytes()).await;
            });
            return;
        }
        state.asks.push(OutsideAsk {
            id: stream.clone(),
            client,
            redirect_host,
            at: now_ms(),
        });
        self.publish(&state);
        drop(state);
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(ASK_TTL).await;
            let mut state = lock(&this.inner.state);
            let before = state.asks.len();
            state.asks.retain(|ask| ask.id != stream);
            if state.asks.len() != before {
                this.publish(&state);
            }
        });
    }

    fn call(&self, stream: String, payload: Value, out: McpOut) {
        let grant = payload["grant"].as_str().unwrap_or_default().to_owned();
        let body = Bytes::from(payload["body"].as_str().unwrap_or_default().to_owned());
        let mut headers = HeaderMap::new();
        if let Some(sent) = payload["headers"].as_object() {
            for name in CALL_HEADERS {
                if let Some(value) = sent.get(name).and_then(Value::as_str)
                    && let Ok(value) = HeaderValue::from_str(value)
                {
                    headers.insert(HeaderName::from_static(name), value);
                }
            }
        }
        let mut state = lock(&self.inner.state);
        if state.calls.len() >= MAX_CALLS || state.calls.contains_key(&stream) {
            drop(state);
            tokio::spawn(async move {
                send_refusal(&out, &stream, 503, "too many calls at once on this device").await;
            });
            return;
        }
        let this = self.clone();
        let key = stream.clone();
        let task = tokio::spawn(async move {
            this.serve(&stream, &grant, headers, body, &out).await;
            lock(&this.inner.state).calls.remove(&stream);
        });
        state.calls.insert(key, task.abort_handle());
    }

    async fn serve(&self, stream: &str, grant: &str, headers: HeaderMap, body: Bytes, out: &McpOut) {
        let Some(endpoint) = self.endpoint() else {
            send_refusal(out, stream, 503, "Zeron's tools aren't up on this device yet").await;
            return;
        };
        let response = endpoint.relay(grant, headers, body).await;
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let head = json!({
            "t": "head",
            "status": response.status().as_u16(),
            "headers": content_type.map(|value| json!({ "content-type": value })).unwrap_or_else(|| json!({})),
        });
        if !out.send(stream, head.to_string().as_bytes()).await {
            return;
        }
        let mut body = response.into_body();
        while let Some(Ok(frame)) = body.frame().await {
            let Ok(data) = frame.into_data() else { continue };
            let text = String::from_utf8_lossy(&data);
            let mut rest: &str = &text;
            while !rest.is_empty() {
                let mut end = rest.len().min(DATA_CHUNK);
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                let (piece, tail) = rest.split_at(end);
                if !out.send(stream, json!({ "t": "data", "data": piece }).to_string().as_bytes()).await {
                    return;
                }
                rest = tail;
            }
        }
        out.send(stream, json!({ "t": "end" }).to_string().as_bytes()).await;
    }
}

/// A call answered without reaching the tools: a JSON-RPC error, as the
/// endpoint's own refusals are.
async fn send_refusal(out: &McpOut, stream: &str, status: u16, message: &str) {
    let head = json!({ "t": "head", "status": status, "headers": { "content-type": "application/json" } });
    let body = json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32000, "message": message } });
    if out.send(stream, head.to_string().as_bytes()).await
        && out.send(stream, json!({ "t": "data", "data": body.to_string() }).to_string().as_bytes()).await
    {
        out.send(stream, json!({ "t": "end" }).to_string().as_bytes()).await;
    }
}

impl McpRelay for OutsideAgents {
    fn joined(&self, out: McpOut) {
        let grants: Vec<String> = {
            let mut state = lock(&self.inner.state);
            state.out = Some(out.clone());
            state.grants.iter().map(|grant| grant.id.clone()).collect()
        };
        tokio::spawn(async move {
            out.send("grants", json!({ "t": "grants", "grants": grants }).to_string().as_bytes()).await;
        });
    }

    fn frame(&self, stream: String, payload: Vec<u8>, out: McpOut) {
        let Ok(payload) = serde_json::from_slice::<Value>(&payload) else {
            tracing::warn!("outside agents: malformed frame — ignoring");
            return;
        };
        match payload["t"].as_str() {
            Some("ask") => self.ask(stream, &payload, out),
            Some("call") => self.call(stream, payload, out),
            Some("cancel") => {
                if let Some(call) = lock(&self.inner.state).calls.remove(&stream) {
                    call.abort();
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "outside_agents_tests.rs"]
mod tests;
