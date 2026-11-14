//! MCP stdio transport: newline-delimited JSON-RPC 2.0 on stdin/stdout.
//!
//! Requests are handled concurrently (a `wait_for_turn` may block for
//! minutes while the client keeps pinging); responses are serialized through
//! one writer task so frames never interleave. The one notification the server
//! sends on its own, `notifications/tools/list_changed` (when the engine's
//! plugin tools change), goes through the same writer.
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

/// Appended to [`INSTRUCTIONS`] where the agent is offered
/// `wait_on_background` ([`zeron_proto::McpServer::WAITS_ON_BACKGROUND_ENV`]).
const WAITS_ON_BACKGROUND: &str = "\n\n\
Before ending a turn while a background command you started is still running and you'll carry on \
once it ends (tests, a build), call `wait_on_background` with its task id, so the person sees the \
chat waiting rather than finished. Not for anything you leave running (a dev server, a watcher).";

/// Appended to [`INSTRUCTIONS`] when the host launched the agent without its
/// own subagent tools ([`zeron_proto::McpServer::SIDE_CHATS_ONLY_ENV`]).
const SIDE_CHATS_ONLY: &str = "\n\n\
Your own subagent tools are turned off in Zeron's settings: where you would start a \
subagent, delegate through Zeron chats instead, with `create_chats` (or `create_chat`).";

/// What `initialize` tells the agent: with when to say it waits where it's
/// offered `wait_on_background`, and the side-chat rule when its host turned
/// the agent's own subagents off.
fn instructions(waits: bool, side_chats_only: bool) -> std::borrow::Cow<'static, str> {
    if !waits && !side_chats_only {
        return INSTRUCTIONS.into();
    }
    let waits = if waits { WAITS_ON_BACKGROUND } else { "" };
    let side_chats = if side_chats_only { SIDE_CHATS_ONLY } else { "" };
    format!("{INSTRUCTIONS}{waits}{side_chats}").into()
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
                    "capabilities": { "tools": { "listChanged": true } },
                    "serverInfo": {
                        "name": "zeron",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "instructions": instructions(tools.waits_on_background(), tools.side_chats_only()),
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
                Ok(value) => {
                    let (content, value) = tool_content(value);
                    // Given `structuredContent`, Codex shows the model only
                    // that (no images), and Claude Code shows it in place of
                    // the texts, so content beyond the result's text (its
                    // images, what was left out) goes without it.
                    let structured = (content.len() == 1).then(|| wrap_structured(value));
                    let mut result = json!({ "content": content, "isError": false });
                    if let Some(structured) = structured {
                        result["structuredContent"] = structured;
                    }
                    ok_response(id, result)
                }
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
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

/// Where a tool's result (a plugin's `zeron.tool` answer included) carries
/// images for the agent: `{type: "image", data: <base64>, mimeType}` each,
/// MCP's own image content. They go out as image content, after the rest of
/// the result as text.
const IMAGE_CONTENT: &str = "imageContent";

/// Image types the agents' models read.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// The largest image passed on, decoded; the models refuse larger ones.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// A tool result as MCP content: the result as text, then the images its
/// [`IMAGE_CONTENT`] carries, and the result without them. An image left
/// out is named in a last text, so the agent knows.
fn tool_content(mut value: Value) -> (Vec<Value>, Value) {
    let text = |value: &Value| json!({ "type": "text", "text": pretty(value) });
    if !value.get(IMAGE_CONTENT).is_some_and(Value::is_array) {
        return (vec![text(&value)], value);
    }
    let images = match value.as_object_mut().and_then(|v| v.remove(IMAGE_CONTENT)) {
        Some(Value::Array(images)) => images,
        _ => Vec::new(),
    };
    let mut content = vec![text(&value)];
    // What the reply has left before `cap_reply` would cut it to text,
    // images and all; a 64 KiB margin for the notes and the envelope.
    let mut room = MAX_REPLY_BYTES.saturating_sub(content[0].to_string().len() + 64 * 1024);
    let mut left_out = Vec::new();
    for (index, image) in images.iter().enumerate() {
        match image_content(image) {
            Ok(image) => {
                let size = image["data"].as_str().map_or(0, str::len);
                if size > room {
                    let mb = MAX_REPLY_BYTES / (1024 * 1024);
                    left_out.push(format!(
                        "{IMAGE_CONTENT}[{index}] left out: the reply would pass {mb} MB"
                    ));
                } else {
                    room -= size;
                    content.push(image);
                }
            }
            Err(why) => left_out.push(format!("{IMAGE_CONTENT}[{index}] left out: {why}")),
        }
    }
    if !left_out.is_empty() {
        content.push(text(&Value::String(left_out.join("\n"))));
    }
    (content, value)
}

fn image_content(image: &Value) -> Result<Value, String> {
    use base64::Engine as _;
    let shape = "not { type = \"image\", data = <base64>, mimeType }";
    if image.get("type").and_then(Value::as_str) != Some("image") {
        return Err(shape.into());
    }
    let (Some(data), Some(mime_type)) = (
        image.get("data").and_then(Value::as_str),
        image.get("mimeType").and_then(Value::as_str),
    ) else {
        return Err(shape.into());
    };
    if !IMAGE_TYPES.contains(&mime_type) {
        return Err(format!(
            "{mime_type} is not one of {}",
            IMAGE_TYPES.join(", ")
        ));
    }
    let too_large = || format!("larger than {} MB", MAX_IMAGE_BYTES / (1024 * 1024));
    // Too long to be small enough, without decoding it.
    if data.len() / 4 * 3 > MAX_IMAGE_BYTES + 3 {
        return Err(too_large());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "data is not base64".to_owned())?;
    if bytes.is_empty() {
        return Err("data is empty".into());
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(too_large());
    }
    Ok(json!({ "type": "image", "data": data, "mimeType": mime_type }))
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

    /// An engine whose plugin tool answers with the value it was given.
    struct Answering(Value);

    #[async_trait]
    impl RpcService for Answering {
        async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
            match method {
                methods::CALL_PLUGIN_TOOL => {
                    Ok(RpcReply::Value(json!({ "ok": true, "value": self.0 })))
                }
                other => Err(RpcError::UnknownMethod(other.into())),
            }
        }
    }

    /// What `tools/call` of a plugin tool answering `value` gives the client.
    async fn plugin_call(value: Value) -> Value {
        let tools = Tools::new(Arc::new(Zeron::with_client(
            memory_client(Arc::new(Answering(value))),
            Origin::default(),
        )));
        let reply = handle_request(
            &tools,
            json!(1),
            "tools/call",
            json!({ "name": "design__screenshot" }),
        )
        .await;
        assert_eq!(reply["result"]["isError"], false, "{reply}");
        reply["result"].clone()
    }

    /// A 1x1 PNG, base64.
    const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    #[tokio::test]
    async fn a_plugin_tools_image_content_reaches_the_agent_as_images() {
        let result = plugin_call(json!({
            "id": "screen-1",
            "imageContent": [
                { "type": "image", "data": PNG, "mimeType": "image/png" },
                { "type": "image", "data": PNG, "mimeType": "image/webp", "label": "extra fields go" },
            ],
        }))
        .await;
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 3, "{result}");
        assert_eq!(content[0]["type"], "text");
        let text: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(
            text,
            json!({ "id": "screen-1" }),
            "the images leave the text"
        );
        assert_eq!(
            content[1],
            json!({ "type": "image", "data": PNG, "mimeType": "image/png" })
        );
        assert_eq!(
            content[2],
            json!({ "type": "image", "data": PNG, "mimeType": "image/webp" })
        );
        assert!(
            result.get("structuredContent").is_none(),
            "clients would show it in place of the images"
        );
    }

    #[tokio::test]
    async fn a_bad_or_oversized_image_is_left_out_and_said_so() {
        use base64::Engine as _;
        let huge = base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_IMAGE_BYTES + 1]);
        let result = plugin_call(json!({
            "ok": true,
            "imageContent": [
                "data:image/png;base64,abc",
                { "type": "image", "data": "not base64!", "mimeType": "image/png" },
                { "type": "image", "data": PNG, "mimeType": "image/svg+xml" },
                { "type": "image", "data": huge, "mimeType": "image/png" },
                { "type": "image", "data": "A".repeat(2 * MAX_IMAGE_BYTES), "mimeType": "image/png" },
                { "type": "image", "data": PNG, "mimeType": "image/png" },
            ],
        }))
        .await;
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 3, "{result}");
        assert_eq!(content[0]["text"], pretty(&json!({ "ok": true })));
        assert_eq!(content[1]["type"], "image");
        assert!(
            result.get("structuredContent").is_none(),
            "clients would show it in place of the images"
        );
        assert_eq!(
            content[2]["text"],
            "imageContent[0] left out: not { type = \"image\", data = <base64>, mimeType }\n\
             imageContent[1] left out: data is not base64\n\
             imageContent[2] left out: image/svg+xml is not one of image/png, image/jpeg, image/webp, image/gif\n\
             imageContent[3] left out: larger than 5 MB\n\
             imageContent[4] left out: larger than 5 MB"
        );
    }

    #[tokio::test]
    async fn images_stop_before_the_reply_would_be_cut() {
        use base64::Engine as _;
        let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 3 * 1024 * 1024]);
        let image = json!({ "type": "image", "data": big, "mimeType": "image/png" });
        let result = plugin_call(json!({ "imageContent": [image, image, image] })).await;
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 3, "the text, one image, the note");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(
            content[2]["text"],
            "imageContent[1] left out: the reply would pass 8 MB\n\
             imageContent[2] left out: the reply would pass 8 MB"
        );
        let reply = ok_response(json!(1), result);
        assert_eq!(
            cap_reply(reply.clone()),
            reply,
            "sent whole, the image kept"
        );
    }

    #[tokio::test]
    async fn a_result_without_an_image_array_stays_text() {
        for value in [
            json!({ "imageContent": "screenshot.png" }),
            json!([{ "type": "image", "data": PNG, "mimeType": "image/png" }]),
            json!("done"),
        ] {
            let result = plugin_call(value.clone()).await;
            assert_eq!(
                result["content"],
                json!([{ "type": "text", "text": pretty(&value) }])
            );
            assert_eq!(result["structuredContent"], wrap_structured(value));
        }
    }

    #[test]
    fn side_chats_only_instructions_add_the_delegation_rule() {
        assert_eq!(instructions(false, false), INSTRUCTIONS);
        let side_chats = instructions(false, true);
        assert!(side_chats.starts_with(INSTRUCTIONS));
        assert!(side_chats.ends_with("with `create_chats` (or `create_chat`)."));
    }

    #[test]
    fn a_reply_too_big_to_send_is_cut_down_not_dropped() {
        let small = ok_response(
            json!(1),
            json!({ "content": [{ "type": "text", "text": "hi" }], "isError": false }),
        );
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
        assert!(
            cut["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .ends_with("`offset`.)")
        );
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

    #[test]
    fn only_an_agent_offered_wait_on_background_is_told_when_to_use_it() {
        assert!(!INSTRUCTIONS.contains("wait_on_background"));
        let waits = instructions(true, false);
        assert!(waits.starts_with(INSTRUCTIONS));
        assert!(waits.contains("call `wait_on_background`"));
        let both = instructions(true, true);
        assert!(both.contains("call `wait_on_background`"));
        assert!(both.ends_with("with `create_chats` (or `create_chat`)."));
    }
}
