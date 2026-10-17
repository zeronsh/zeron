//! MCP stdio transport: newline-delimited JSON-RPC 2.0 on stdin/stdout.
//!
//! Requests are handled concurrently (a `wait_for_turn` may block for
//! minutes while the client keeps pinging); responses are serialized through
//! one writer task so frames never interleave. `notifications/cancelled`
//! (the client's Stop) aborts the request, and a cancelled computer action
//! also interrupts the driver's native input, as Synara's Stop does. When
//! stdin closes the computer's turn ends: running input is interrupted and
//! the agent cursor hidden before the process exits.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

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
Use `kind: chat` in `create_chat` or each `create_chats` request for a standalone \
session visible in Sessions; it cannot have a parent. Use `kind: side` for a child \
with an explicit parent or your origin chat. Omitted kind keeps legacy defaults: \
side with an origin/parent, standalone otherwise. Side chats cannot create chats. \
Discover hosts with `list_devices`, then `list_projects {device}`, \
`list_harnesses {device}` and `list_models {device, harness}`. A project fixes \
its host; with both project and device it must belong to that device. Use listed \
ids when names/paths repeat. Without project or device, creation uses the local engine. \
If a chat is `awaitingInput`, answer it with `respond_to_input`.\n\
\n\
When a chart, table, diagram, dashboard, or 3D view would say more than prose and \
`show_visual` is available, write a Luau visual script and show it with `show_visual` before \
your final reply. Zeron renders it with its own native interface and enforces its style: \
content sits directly on the chat with no cards or boxed panels, text uses the interface font \
and type scale, and colors are theme tokens that work on frosted glass or opaque, light or dark. \
The reader sees the visual above that reply, so don't announce or restate it.";

/// Appended when the computer-use driver is available.
const COMPUTER_INSTRUCTIONS: &str = "\n\n\
Computer use: tools prefixed `computer_` control this Mac through cua-driver \
(screenshots, accessibility trees, clicks, typing, app and window control, browser \
automation). The user sees an agent cursor labelled Zeron wherever you act. If a tool reports missing Accessibility or Screen Recording \
permission, ask the user to grant it to Zeron in System Settings > Privacy & Security.\n\n";

/// How long `initialize` waits for the driver so its guidance can be included.
const COMPUTER_INIT_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// Requests in flight by id (JSON text), so a cancellation can abort them.
/// The flag marks computer actions, whose cancellation interrupts input.
type InFlight = Arc<Mutex<HashMap<String, (AbortHandle, bool)>>>;

/// Serve MCP on this process's stdin/stdout until stdin closes.
pub async fn serve_stdio(tools: Arc<Tools>) -> anyhow::Result<()> {
    let (out_tx, mut out_rx) = mpsc::channel::<String>(64);
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

    let in_flight: InFlight = Arc::default();
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
        let Some(response) = route(&tools, message, &out_tx, &in_flight) else {
            continue;
        };
        let _ = out_tx.send(response.to_string()).await;
    }
    // The client went away (turn over, Stop, or harness exit): leave no
    // input running and no agent cursor on screen behind it.
    if let Some(computer) = tools.computer() {
        computer.turn_ended().await;
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

/// `notifications/cancelled`: abort the request; a computer action also
/// interrupts the driver's input (the action may already be running).
fn cancel(tools: &Arc<Tools>, params: &Value, in_flight: &InFlight) {
    let Some(id) = params.get("requestId") else {
        return;
    };
    let Some((handle, action)) = in_flight.lock().ok().and_then(|mut map| map.remove(&id.to_string())) else {
        return;
    };
    handle.abort();
    if action && let Some(computer) = tools.computer().cloned() {
        tokio::spawn(async move {
            computer.stop_input().await;
        });
    }
}

/// Classify one inbound message. Requests are answered asynchronously via
/// `out`; an immediate `Some` is a synchronous error reply.
fn route(tools: &Arc<Tools>, message: Value, out: &mpsc::Sender<String>, in_flight: &InFlight) -> Option<Value> {
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
            let key = id.to_string();
            let action = method == "tools/call"
                && params
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(crate::computer::Computer::is_action_tool);
            let tools = tools.clone();
            let out = out.clone();
            // Registered before the task can finish, so its own removal
            // never races the insert.
            let mut registry = in_flight.lock().ok();
            let done = in_flight.clone();
            let task_key = key.clone();
            let task = tokio::spawn(async move {
                let response = handle_request(&tools, id, &method, params).await;
                if let Ok(mut map) = done.lock() {
                    map.remove(&task_key);
                }
                let _ = out.send(response.to_string()).await;
            });
            if let Some(map) = registry.as_mut() {
                map.insert(key, (task.abort_handle(), action));
            }
            None
        }
        (None, Some(method)) => {
            tracing::debug!(method, "mcp: notification");
            if method == "notifications/cancelled" {
                cancel(tools, &params, in_flight);
            }
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

async fn instructions(tools: &Tools) -> String {
    let Some(computer) = tools.computer() else {
        return INSTRUCTIONS.to_owned();
    };
    match tokio::time::timeout(COMPUTER_INIT_WAIT, computer.instructions()).await {
        Ok(Some(driver)) => format!("{INSTRUCTIONS}{COMPUTER_INSTRUCTIONS}{driver}"),
        _ => INSTRUCTIONS.to_owned(),
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
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "zeron",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": instructions(tools).await,
                }),
            )
        }
        "ping" => ok_response(id, json!({})),
        "tools/list" => {
            let mut list: Vec<Value> = tools
                .list()
                .into_iter()
                .filter_map(|tool| serde_json::to_value(tool).ok())
                .collect();
            if let Some(computer) = tools.computer() {
                list.extend(computer.tools().await);
            }
            ok_response(id, json!({ "tools": list }))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return error_response(id, INVALID_PARAMS, "tools/call needs a tool name");
            };
            if let Some(computer) = tools.computer()
                && crate::computer::Computer::is_computer_tool(name)
            {
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                // The driver's result (text + screenshots) passes through verbatim.
                return match computer.call(name, arguments).await {
                    Ok(result) => ok_response(id, result),
                    Err(message) => ok_response(
                        id,
                        json!({
                            "content": [{ "type": "text", "text": message }],
                            "isError": true,
                        }),
                    ),
                };
            }
            if !tools.has(name) {
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
