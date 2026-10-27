//! MCP stdio transport: newline-delimited JSON-RPC 2.0 on stdin/stdout.
//!
//! Requests are handled concurrently (a `wait_for_turn` may block for
//! minutes while the client keeps pinging); responses are serialized through
//! one writer task so frames never interleave.
//!
//! The server must outlive anything its client sends: a client never restarts
//! a stdio server that exits (its agent loses the tools for the session), so
//! a bad line is skipped, never fatal, and a reply too big for the client to
//! read is cut down rather than sent whole. A long call reports progress, as
//! clients abort a call that goes quiet; `notifications/cancelled` stops one.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
Use `kind: chat` in `create_chat` or each `create_chats` request for a standalone \
session visible in Sessions; it cannot have a parent. Use `kind: side` for a child \
with an explicit parent or your origin chat. Omitted kind keeps legacy defaults: \
side with an origin/parent, standalone otherwise. \
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

/// How often a call still running reports progress. Clients abort a call
/// that sends nothing for long enough (Claude Code: 30 minutes); a Zeron
/// wait may run an hour.
const PROGRESS_EVERY: Duration = Duration::from_secs(30);

/// The largest reply this server writes. Claude Code drops its stdio
/// connection, for good, on a message past 16 MiB.
const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;

/// Requests in flight, by id, so `notifications/cancelled` can stop one.
type InFlight = Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

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

    let in_flight = InFlight::default();
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match stdin.read_until(b'\n', &mut buf).await {
            Ok(0) => break, // the client closed stdin: we're done
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                tracing::warn!(error = %err, "mcp: stdin failed");
                break;
            }
        }
        // A line that isn't UTF-8 fails to parse below, and is answered as
        // such, rather than ending the server.
        let line = String::from_utf8_lossy(&buf);
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
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

/// Classify one inbound message. Requests are answered asynchronously via
/// `out`; an immediate `Some` is a synchronous error reply.
fn route(
    tools: &Arc<Tools>,
    message: Value,
    out: &mpsc::Sender<String>,
    in_flight: &InFlight,
) -> Option<Value> {
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
            let key = id.to_string();
            let token = params
                .get("_meta")
                .and_then(|meta| meta.get("progressToken"))
                .cloned();
            let registry = in_flight.clone();
            let done_key = key.clone();
            // Held across the spawn, so a request that finishes at once
            // can't remove its entry before it is made.
            let mut requests = lock(in_flight);
            let task = tokio::spawn(async move {
                let progress = token.map(|token| spawn_progress(token, out.clone()));
                let response = handle_request(&tools, id, &method, params).await;
                if let Some(progress) = progress {
                    progress.abort();
                }
                let _ = out.send(cap_reply(response).to_string()).await;
                lock(&registry).remove(&done_key);
            });
            requests.insert(key, task.abort_handle());
            None
        }
        (None, Some(method)) if method == "notifications/cancelled" => {
            // The client gave up on a request: stop it (a wait holding a
            // subscription to the engine releases it) and send nothing back.
            let key = params.get("requestId").map(Value::to_string);
            if let Some(task) = key.and_then(|key| lock(in_flight).remove(&key)) {
                task.abort();
            }
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
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": "zeron",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": instructions(tools.side_chats_only()),
                }),
            )
        }
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, json!({ "tools": tools.list() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return error_response(id, INVALID_PARAMS, "tools/call needs a tool name");
            };
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Report progress on token `token` every [`PROGRESS_EVERY`] until aborted.
fn spawn_progress(token: Value, out: mpsc::Sender<String>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut beat = 0u64;
        loop {
            tokio::time::sleep(PROGRESS_EVERY).await;
            beat += 1;
            let note = json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": { "progressToken": token, "progress": beat, "message": "still working" },
            });
            if out.send(note.to_string()).await.is_err() {
                return;
            }
        }
    })
}

/// `response`, cut down to [`MAX_REPLY_BYTES`] when it is bigger: the
/// structured copy goes, and the text is truncated with a pointer to paging.
fn cap_reply(response: Value) -> Value {
    if response.to_string().len() <= MAX_REPLY_BYTES {
        return response;
    }
    let Some(result) = response.get("result") else {
        return response;
    };
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    let mut end = (MAX_REPLY_BYTES / 2).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = format!(
        "{}\n… (reply cut short: too large to send whole. Ask for less, e.g. a smaller `limit` with `offset`.)",
        &text[..end]
    );
    json!({
        "jsonrpc": "2.0",
        "id": response["id"],
        "result": {
            "content": [{ "type": "text", "text": cut }],
            "isError": result["isError"],
        },
    })
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

    #[test]
    fn side_chats_only_instructions_add_the_delegation_rule() {
        assert_eq!(instructions(false), INSTRUCTIONS);
        let side_chats = instructions(true);
        assert!(side_chats.starts_with(INSTRUCTIONS));
        assert!(side_chats.ends_with("with `create_chats` (or `create_chat`)."));
    }

    #[test]
    fn a_reply_too_big_to_send_is_cut_down_not_dropped() {
        let small = ok_response(json!(1), json!({ "content": [{ "type": "text", "text": "hi" }], "isError": false }));
        assert_eq!(cap_reply(small.clone()), small);
        let huge = "é".repeat(MAX_REPLY_BYTES);
        let big = ok_response(
            json!(2),
            json!({
                "content": [{ "type": "text", "text": huge }],
                "structuredContent": { "messages": huge },
                "isError": false,
            }),
        );
        let cut = cap_reply(big);
        assert!(cut.to_string().len() <= MAX_REPLY_BYTES);
        assert_eq!(cut["id"], 2);
        assert!(cut["result"].get("structuredContent").is_none());
        assert!(cut["result"]["content"][0]["text"].as_str().unwrap().ends_with("`offset`.)"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_long_call_reports_progress_until_it_ends() {
        let (tx, mut rx) = mpsc::channel(8);
        let progress = spawn_progress(json!("tok-1"), tx);
        tokio::time::sleep(PROGRESS_EVERY * 2 + Duration::from_secs(1)).await;
        let first: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(first["method"], "notifications/progress");
        assert_eq!(first["params"]["progressToken"], "tok-1");
        assert_eq!(first["params"]["progress"], 1);
        let second: Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
        assert_eq!(second["params"]["progress"], 2);
        progress.abort();
        tokio::time::sleep(PROGRESS_EVERY * 2).await;
        assert!(rx.try_recv().is_err());
    }
}
