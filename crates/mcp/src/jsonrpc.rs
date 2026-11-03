//! MCP stdio transport: newline-delimited JSON-RPC 2.0 on stdin/stdout.
//!
//! Requests are handled concurrently (a `wait_for_turn` may block for
//! minutes while the client keeps pinging); responses are serialized through
//! one writer task so frames never interleave. Notifications from the client
//! (`notifications/initialized`, `notifications/cancelled`) are accepted and
//! ignored — there is no server-side state to initialize or cancel. The one
//! notification the server sends, `notifications/tools/list_changed` (when
//! the engine's plugin tools change), goes through the same writer.

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::tools::Tools;

/// Protocol revisions this server knows. The client's requested revision
/// is echoed when it is one of these; otherwise the newest is offered and
/// the client decides whether to continue (per the MCP handshake rules).
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST_PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = "\
Zeron runs coding agents in chats, each hosted on a device, optionally in a project \
(a folder on that specific device). These tools operate the local Zeron engine: \
discover devices/projects/chats, create chats with a chosen harness and \
model, read transcripts, and send messages between chats.\n\
\n\
Chats are referenced by full id, a unique id prefix, or an exact title. Use \
`whoami` to learn which chat you are speaking from; messages you send are \
attributed to it and a chat cannot message itself. For request/response \
with a single chat, `send_message` with `wait: true` returns its reply. \
For parallel delegation, use `create_chats` with a prompt for each chat or \
`send_messages` for existing chats. These batch tools launch requests concurrently \
even if your harness executes tool calls sequentially. Alternatively, launch ALL \
chats/messages with `wait: false` first, then use `wait_for_turn` to collect replies. \
Do not wait for one worker before launching the next independent worker. \
A chat you create from your chat is your subagent by default (`kind: subagent`): \
it lists under your chat's Subagents and in its transcript's subagent block. Use \
`kind: side` only for a side conversation the person should treat as their own, \
and `kind: chat` for a standalone session visible in Sessions (no parent). Omitted \
kind: subagent with an origin/parent, standalone otherwise. \
Discover hosts with `list_devices`, then `list_projects {device}`, \
`list_harnesses {device}` and `list_models {device, harness}`. A project fixes \
its host; with both project and device it must belong to that device. Use listed \
ids when names/paths repeat. Without project or device, creation uses the local engine. \
If a chat is `awaitingInput`, answer it with `respond_to_input`.\n\
\n\
Delegation is provider-agnostic: `list_harnesses` shows every provider, and \
those marked `connected` can take work from you whichever provider you run \
on (a claude-code agent may hand a task to codex, and the reverse). Pass \
`harness` (and a model from `list_models`) to `create_chat` to choose one. \
Delegation is one level deep: a chat that has a parent cannot create chats. \
Pass `cloud: true` to run a child on a Cloud machine of its own (Codex or Claude Code); \
from a Cloud chat it clones the same repository, and without `cloud` it shares your machine.\n\
\n\
When a chart, table, diagram, dashboard, or 3D view would say more than prose and \
`show_visual` is available, write a Luau visual script and show it with `show_visual` before \
your final reply. Zeron renders it with its own native interface and enforces its style: \
content sits directly on the chat with no cards or boxed panels, text uses the interface font \
and type scale, and colors are theme tokens that work on frosted glass or opaque, light or dark. \
The reader sees the visual above that reply, so don't announce or restate it. To show the \
reader an image file (a screenshot, a rendered PNG), call `show_image` with its path; don't \
draw images through `show_visual`.";

/// Appended to [`INSTRUCTIONS`] when the host launched the agent without its
/// own subagent tools ([`zeron_proto::McpServer::SIDE_CHATS_ONLY_ENV`]).
const SIDE_CHATS_ONLY: &str = "\n\n\
Your own subagent tools are turned off in Zeron's settings: where you would start a \
subagent, delegate through Zeron chats instead, with `create_chats` (or `create_chat`).";

/// What `initialize` tells the agent, with the side-chat rule when its host
/// turned the agent's own subagents off.
fn instructions(side_chats_only: bool) -> std::borrow::Cow<'static, str> {
    if side_chats_only {
        format!("{INSTRUCTIONS}{SIDE_CHATS_ONLY}").into()
    } else {
        INSTRUCTIONS.into()
    }
}

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Serve MCP on this process's stdin/stdout until stdin closes.
pub async fn serve_stdio(tools: Arc<Tools>) -> anyhow::Result<()> {
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
    let list_watch = spawn_tool_list_watch(tools.clone(), out_tx.clone());
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = out_rx.recv().await {
            if stdout.write_all(line.as_bytes()).await.is_err()
                || stdout.write_all(b"\n").await.is_err()
                || stdout.flush().await.is_err()
            {
                break;
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(err) => {
                let _ = out_tx
                    .send(
                        error_response(Value::Null, PARSE_ERROR, &format!("parse error: {err}"))
                            .to_string(),
                    )
                    .await;
                continue;
            }
        };
        let Some(response) = route(&tools, message, &out_tx) else {
            continue;
        };
        let _ = out_tx.send(response.to_string()).await;
    }
    // The watch holds a writer handle: stop it so the writer can drain.
    list_watch.abort();
    let _ = list_watch.await;
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

/// The notification a client re-lists its tools on.
fn list_changed_notification() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" })
}

/// Follow the engine's plugin tools and write `notifications/tools/list_changed`
/// to `out` (the shared writer) whenever their names change.
fn spawn_tool_list_watch(
    tools: Arc<Tools>,
    out: mpsc::Sender<String>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (changed_tx, mut changed) = mpsc::channel::<()>(1);
        let watch = tools.watch_plugin_tools(changed_tx);
        let forward = async {
            while changed.recv().await.is_some() {
                if out
                    .send(list_changed_notification().to_string())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        };
        tokio::join!(watch, forward);
    })
}

/// Classify one inbound message. Requests are answered asynchronously via
/// `out`; an immediate `Some` is a synchronous error reply.
fn route(tools: &Arc<Tools>, message: Value, out: &mpsc::Sender<String>) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "expected a JSON-RPC object",
        ));
    };
    let id = object.get("id").cloned();
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(method)) => {
            let tools = tools.clone();
            let out = out.clone();
            tokio::spawn(async move {
                let response = handle_request(&tools, id, &method, params).await;
                let _ = out.send(response.to_string()).await;
            });
            None
        }
        (None, Some(method)) => {
            tracing::debug!(method, "mcp: notification");
            None
        }
        // A response to a server-initiated request: we never send any.
        (Some(_), None) => None,
        (None, None) => Some(error_response(
            Value::Null,
            INVALID_REQUEST,
            "missing method",
        )),
    }
}

/// Answer one request. Public for tests and for embedding the dispatcher
/// behind another transport.
pub async fn handle_request(tools: &Tools, id: Value, method: &str, params: Value) -> Value {
    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(LATEST_PROTOCOL);
            let version = if PROTOCOL_VERSIONS.contains(&requested) {
                requested
            } else {
                LATEST_PROTOCOL
            };
            ok_response(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": true } },
                    "serverInfo": {
                        "name": "zeron",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": instructions(tools.side_chats_only()),
                }),
            )
        }
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, json!({ "tools": tools.list_all().await })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return error_response(id, INVALID_PARAMS, "tools/call needs a tool name");
            };
            if !tools.has(name) && !crate::tools::is_plugin_tool_name(name) {
                return error_response(id, INVALID_PARAMS, &format!("unknown tool: {name}"));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match tools.call(name, arguments).await {
                Ok(value) => ok_response(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": pretty(&value) }],
                        "structuredContent": wrap_structured(value),
                        "isError": false,
                    }),
                ),
                Err(message) => ok_response(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": message }],
                        "isError": true,
                    }),
                ),
            }
        }
        // Optional capabilities we do not advertise; answer empty rather
        // than erroring so permissive clients do not log noise.
        "resources/list" => ok_response(id, json!({ "resources": [] })),
        "prompts/list" => ok_response(id, json!({ "prompts": [] })),
        other => error_response(id, METHOD_NOT_FOUND, &format!("unknown method: {other}")),
    }
}

/// `structuredContent` must be an object; arrays and scalars are wrapped.
fn wrap_structured(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        json!({ "result": value })
    }
}

fn pretty(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    }
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::zeron::{Origin, Zeron};
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::time::Duration;
    use zeron_rpc::{RpcError, RpcReply, RpcService, memory_client, methods};

    /// An engine whose plugin tools go from one to two, then stay.
    struct Watching;

    #[async_trait]
    impl RpcService for Watching {
        async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
            let item = |names: &[&str]| {
                json!({
                    "device": { "id": "d", "name": "Studio" },
                    "tools": names.iter().map(|n| json!({
                        "plugin": "p", "plugin_name": "P", "tool": "t", "mcp_name": n,
                        "description": "", "input": {}, "open": true,
                    })).collect::<Vec<_>>(),
                })
            };
            match method {
                methods::WATCH_PLUGIN_TOOLS => Ok(RpcReply::Stream(
                    futures::stream::iter(vec![item(&["p__one"]), item(&["p__one", "p__two"])])
                        .chain(futures::stream::pending())
                        .boxed(),
                )),
                methods::LIST_PLUGIN_TOOLS => Ok(RpcReply::Value(item(&["p__one"]))),
                other => Err(RpcError::UnknownMethod(other.into())),
            }
        }
    }

    fn watching_tools() -> Arc<Tools> {
        Arc::new(Tools::new(Arc::new(Zeron::with_client(
            memory_client(Arc::new(Watching)),
            Origin::default(),
        ))))
    }

    #[tokio::test]
    async fn list_changed_goes_out_when_the_plugin_tool_names_change() {
        let (out_tx, mut out) = mpsc::channel(8);
        let watch = spawn_tool_list_watch(watching_tools(), out_tx);
        let frame = tokio::time::timeout(Duration::from_secs(5), out.recv())
            .await
            .expect("a notification in time")
            .expect("writer open");
        let frame: Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(frame, list_changed_notification());
        assert!(frame.get("id").is_none(), "a notification has no id");
        assert!(
            tokio::time::timeout(Duration::from_millis(200), out.recv())
                .await
                .is_err(),
            "one change, one notification"
        );
        watch.abort();
    }

    #[tokio::test]
    async fn initialize_advertises_list_changed_and_the_listing_names_plugin_tools() {
        let tools = watching_tools();
        let init = handle_request(&tools, json!(1), "initialize", json!({})).await;
        assert_eq!(init["result"]["capabilities"]["tools"]["listChanged"], true);
        let listed = handle_request(&tools, json!(2), "tools/list", json!({})).await;
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"whoami"));
        assert!(names.contains(&"p__one"));
        for tool in listed["result"]["tools"].as_array().unwrap() {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        }
    }

    #[test]
    fn side_chats_only_instructions_add_the_delegation_rule() {
        assert_eq!(instructions(false), INSTRUCTIONS);
        let side_chats = instructions(true);
        assert!(side_chats.starts_with(INSTRUCTIONS));
        assert!(side_chats.ends_with("with `create_chats` (or `create_chat`)."));
    }
}
