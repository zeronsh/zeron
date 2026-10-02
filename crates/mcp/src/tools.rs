//! The tool catalog and its dispatch.
//!
//! Every tool is a thin composition of engine reads/writes from
//! [`Zeron`]; the only logic that lives here is argument resolution (chat
//! by prefix, project by path), sender attribution, and the "how do I
//! deliver a message to a chat in this state" choice the composer makes
//! for humans.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeron_doc::SessionCommandPayload;
use zeron_proto::{
    Chat, ChatConfig, Device, HarnessId, MAX_SPAWN_DEPTH, ReasoningLevel, RunRequest, SandboxLevel,
    Session, SessionStatus, Space, UserInputAnswer, WorktreeSpec, spawn_depth,
};

use crate::transcript::{RenderOptions, RenderedMessage, render_entries};
use crate::zeron::{
    HarnessInfo, TurnOutcome, Zeron, is_execution_host, is_online, resolve_chat_in,
    resolve_space_in, session_for, short,
};

/// Default and ceiling for the blocking waits.
const MAX_BATCH: usize = 32;
const DEFAULT_WAIT: Duration = Duration::from_secs(600);
const MAX_WAIT: Duration = Duration::from_secs(3600);
/// A session row older than this is not trusted to still be working
/// (the UI's staleness window): a crashed host must not read as busy forever.
const SESSION_STALE: chrono::Duration = chrono::Duration::seconds(45);
/// How long a completed wait keeps re-reading the transcript for the reply
/// when the session row outran the (separately synced) chat doc.
const TRANSCRIPT_GRACE: Duration = Duration::from_secs(20);
/// Fan-out cap: unarchived chats one chat may have spawned at once (side and
/// top-level together). Archiving finished workers frees slots.
pub const MAX_LIVE_SPAWNS: usize = 32;
/// Churn cap: chats one MCP server (one agent run) may create per minute,
/// so a create/archive loop cannot flood the registry.
pub const SPAWN_RATE: (usize, Duration) = (32, Duration::from_secs(60));

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub struct Tools {
    zeron: Arc<Zeron>,
    /// Serializes "count live spawns → write the row" so a concurrent
    /// `create_chats` batch cannot overshoot [`MAX_LIVE_SPAWNS`].
    create_gate: tokio::sync::Mutex<()>,
    /// Chats this server created (ids), for the live-spawn count while a
    /// fresh row has not folded into `WatchChats` yet.
    created: std::sync::Mutex<Vec<String>>,
    /// Recent creation instants: the per-server [`SPAWN_RATE`] window.
    recent_creates: std::sync::Mutex<std::collections::VecDeque<std::time::Instant>>,
}

fn chat_key_schema(extra: Value) -> Value {
    let mut properties = json!({
        "chat": {
            "type": "string",
            "description": "Chat id, unique id prefix, or exact title."
        }
    });
    if let (Some(base), Some(more)) = (properties.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            base.insert(k.clone(), v.clone());
        }
    }
    json!({ "type": "object", "properties": properties, "required": ["chat"] })
}

fn catalog() -> Vec<ToolDef> {
    let mut tools = vec![
        ToolDef {
            name: "whoami",
            description: "Which chat and device this server speaks for, plus the engine's workspace mode. Call this first when you need to know your own chat id.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "list_devices",
            description: "Devices in this workspace (the local engine's device is flagged). Chats and projects are hosted on a device. `executionHost` devices run agents; create_chat can target any one that is `online` (pass its id/name as device, or one of its `projects`).",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "list_projects",
            description: "Projects: a folder on a device. Each chat belongs to one project, which fixes its host device and working directory. Projects on other online devices are valid create_chat targets.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "list_harnesses",
            description: "Agent harnesses (claude-code, codex, cursor, …) and whether each is available on a device (default: this one).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string", "description": "Device id or name to ask (it must be online). Defaults to this device." }
                }
            }),
        },
        ToolDef {
            name: "list_models",
            description: "Models a harness offers on a device (default: this one). Model ids are harness-specific strings; pass one to create_chat.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "harness": { "type": "string", "description": "Harness id, e.g. claude-code or codex." },
                    "device": { "type": "string", "description": "Device id or name to ask (it must be online). Defaults to this device." }
                },
                "required": ["harness"]
            }),
        },
        ToolDef {
            name: "list_chats",
            description: "Chats with their project, host device, harness/model, live status, and last activity. Newest activity first.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Only chats in this project (id, path, or name)." },
                    "device": { "type": "string", "description": "Only chats hosted on this device (id or name)." },
                    "include_archived": { "type": "boolean", "default": false },
                    "parent": { "type": "string", "description": "Only side chats under this chat (id, prefix, or title)." },
                    "spawned_by": { "type": "string", "description": "Only chats (side or top-level) whose agent was this chat (id, prefix, or title) — e.g. your own id to list every chat you spawned." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 50 }
                }
            }),
        },
        ToolDef {
            name: "get_chat",
            description: "One chat's metadata, live status, and any question its agent is currently blocked on.",
            input_schema: chat_key_schema(json!({})),
        },
        ToolDef {
            name: "create_chat",
            description: "Create a chat on any online device (see list_devices/list_projects) with a harness and model; that device's engine runs it. kind 'side' (default when you are in a chat) makes a side chat under your chat: hidden from the sidebar, listed under its parent. kind 'chat' makes a real top-level chat that appears in the Sessions sidebar on every device like one the user created, marked as spawned by your chat. Either way the new chat records you as spawnedByChatId. Optionally send a first prompt and wait for the reply. Returns the new chat id. For parallel delegation use create_chats, or leave wait=false on every launch and wait only after all chats have been started.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["side", "chat"], "description": "side: a side chat under `parent` (default: your chat). chat: a top-level chat in the sidebar. Defaults to side when there is a parent, else chat." },
                    "project": { "type": "string", "description": "Project id, path, or name; fixes the host device and working directory. Required unless device is given." },
                    "device": { "type": "string", "description": "Host device (id or name). With project: only used to pick among same-named projects and must match. Without: a project-less chat on that device. Defaults to this device." },
                    "parent": { "type": "string", "description": "kind side only: the chat to hang it under (id, prefix, or title). Defaults to the chat you are speaking from." },
                    "harness": { "type": "string", "description": "Harness id (see list_harnesses). Defaults to claude-code when available." },
                    "model": { "type": "string", "description": "Model id from list_models. Omit for the harness default." },
                    "reasoning": { "type": "string", "description": "Reasoning level the model supports (e.g. low, medium, high, max)." },
                    "sandbox": { "type": "string", "enum": ["read-only", "workspace-write", "danger-full-access"], "default": "workspace-write" },
                    "title": { "type": "string", "description": "Sidebar title. Otherwise the engine titles it from the first exchange." },
                    "branch": { "type": "string", "description": "Branch label to record on the chat." },
                    "cwd": { "type": "string", "description": "Working directory override on the host (an existing worktree path). Defaults to the project folder, or ~ for a project-less chat." },
                    "worktree": { "type": "boolean", "default": false, "description": "Run in a fresh isolated git worktree the HOST creates off `branch` (default HEAD) on the first turn. Needs a git project and a prompt." },
                    "prompt": { "type": "string", "description": "First message to send right away." },
                    "wait": { "type": "boolean", "default": false, "description": "With prompt: block until the first turn finishes and return the reply." },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 3600, "default": 600 }
                }
            }),
        },
        ToolDef {
            name: "read_chat",
            description: "Read a chat's transcript as plain messages (newest window by default). Tool calls are summarized one per line.",
            input_schema: chat_key_schema(json!({
                "limit": { "type": "integer", "minimum": 1, "maximum": 500, "default": 40, "description": "How many messages, counted from the newest." },
                "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "Skip this many newest messages (paging backwards)." },
                "include_reasoning": { "type": "boolean", "default": false },
                "include_tools": { "type": "boolean", "default": true }
            })),
        },
        ToolDef {
            name: "send_message",
            description: "Send a message to a chat. Messages are attributed to your chat. mode 'auto' starts a turn when the chat is idle, steers a running turn through its live mailbox (at the next supported input boundary without interrupting the agent). Use mode 'queue' only to explicitly hold a message for later. With wait=true, blocks until the turn finishes and returns the assistant's reply. For parallel work use send_messages, or send to all chats with wait=false before waiting.",
            input_schema: chat_key_schema(json!({
                "text": { "type": "string" },
                "mode": { "type": "string", "enum": ["auto", "run", "steer", "queue"], "default": "auto" },
                "wait": { "type": "boolean", "default": false },
                "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 3600, "default": 600 }
            })),
        },
        ToolDef {
            name: "wait_for_turn",
            description: "Block until a chat is no longer working: returns completed, awaitingInput (answer with respond_to_input), errored, or timedOut, with the newest assistant message.",
            input_schema: chat_key_schema(json!({
                "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 3600, "default": 600 }
            })),
        },
        ToolDef {
            name: "interrupt_chat",
            description: "Stop the chat's running turn.",
            input_schema: chat_key_schema(json!({})),
        },
        ToolDef {
            name: "respond_to_input",
            description: "Answer a question the chat's agent is blocked on (see get_chat / wait_for_turn pendingInput). Labels are the option strings, or free text for open questions.",
            input_schema: chat_key_schema(json!({
                "request_id": { "type": "string", "description": "The pending request id. Defaults to the chat's current pending question." },
                "answers": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question_id": { "type": "string" },
                            "labels": { "type": "array", "items": { "type": "string" } }
                        },
                        "required": ["question_id", "labels"]
                    }
                }
            })),
        },
        ToolDef {
            name: "archive_chat",
            description: "Archive a chat (hide it from the active sidebar list); pass archived=false to restore. Use it to tidy up chats you created.",
            input_schema: chat_key_schema(json!({
                "archived": { "type": "boolean", "default": true }
            })),
        },
    ];
    for (name, single, description) in [
        (
            "create_chats",
            "create_chat",
            "Create multiple independent chats concurrently (each request picks its own kind, device, and project). Put each chat's prompt in its request to start all work together. Prefer this for parallel delegation, including harnesses that execute tool calls sequentially. Each request has create_chat arguments; wait defaults to false. Results preserve request order and include per-request errors; successful requests are not rolled back.",
        ),
        (
            "send_messages",
            "send_message",
            "Send messages to multiple independent chats concurrently. Prefer this to delegate parallel work to existing chats. Each request has send_message arguments; wait defaults to false. If wait=true, requests still run concurrently. Results preserve request order and include per-request errors; successful sends are not rolled back. Do not include dependent messages to the same chat.",
        ),
    ] {
        let item_schema = tools
            .iter()
            .find(|tool| tool.name == single)
            .unwrap()
            .input_schema
            .clone();
        tools.push(ToolDef {
            name, description,
            input_schema: json!({
                "type": "object",
                "properties": {"requests": {"type": "array", "minItems": 1, "maxItems": MAX_BATCH, "items": item_schema}},
                "required": ["requests"],
            }),
        });
    }
    tools
}

// ---- argument shapes ---------------------------------------------------------

#[derive(Deserialize)]
struct BatchArgs {
    requests: Vec<Value>,
}

#[derive(Deserialize)]
struct ChatArgs {
    chat: String,
}

#[derive(Deserialize)]
struct ListModelsArgs {
    harness: String,
    device: Option<String>,
}

#[derive(Deserialize, Default)]
struct DeviceArgs {
    device: Option<String>,
}

#[derive(Deserialize, Default)]
struct ListChatsArgs {
    project: Option<String>,
    device: Option<String>,
    #[serde(default)]
    include_archived: bool,
    parent: Option<String>,
    spawned_by: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, Default)]
struct CreateChatArgs {
    kind: Option<String>,
    project: Option<String>,
    device: Option<String>,
    parent: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    reasoning: Option<String>,
    sandbox: Option<String>,
    title: Option<String>,
    branch: Option<String>,
    cwd: Option<String>,
    #[serde(default)]
    worktree: bool,
    prompt: Option<String>,
    #[serde(default)]
    wait: bool,
    timeout_secs: Option<u64>,
}

/// Where a created chat lives: a side chat under a parent, or a top-level
/// chat in the sidebar. Both record their spawner as provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum ChatKind {
    Side,
    Chat,
}

fn kind_of(chat: &Chat) -> ChatKind {
    if chat.is_side_chat() {
        ChatKind::Side
    } else {
        ChatKind::Chat
    }
}

#[derive(Deserialize)]
struct ReadChatArgs {
    chat: String,
    limit: Option<usize>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    include_reasoning: bool,
    include_tools: Option<bool>,
}

#[derive(Deserialize)]
struct SendArgs {
    chat: String,
    text: String,
    mode: Option<String>,
    #[serde(default)]
    wait: bool,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
struct WaitArgs {
    chat: String,
    timeout_secs: Option<u64>,
}

#[derive(Deserialize)]
struct ArchiveArgs {
    chat: String,
    archived: Option<bool>,
}

#[derive(Deserialize)]
struct RespondArgs {
    chat: String,
    request_id: Option<String>,
    #[serde(default)]
    answers: Vec<AnswerArg>,
}

#[derive(Deserialize)]
struct AnswerArg {
    question_id: String,
    labels: Vec<String>,
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args).map_err(|e| format!("invalid arguments: {e}"))
}

fn wait_duration(secs: Option<u64>) -> Duration {
    secs.map(Duration::from_secs)
        .unwrap_or(DEFAULT_WAIT)
        .min(MAX_WAIT)
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

fn parse_enum<T: serde::de::DeserializeOwned>(what: &str, raw: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(raw.trim().to_owned()))
        .map_err(|_| format!("unknown {what}: {raw:?}"))
}

// ---- summaries ---------------------------------------------------------------

/// Live posture of one chat as the tools report it.
fn status_of(session: Option<&Session>) -> (String, Option<i64>) {
    let Some(session) = session else {
        return ("idle".into(), None);
    };
    let age = chrono::Utc::now() - session.updated_at;
    let stale = age > SESSION_STALE;
    let label = match session.status {
        SessionStatus::Working if stale => "working (stale — host may be offline)",
        SessionStatus::Working => "working",
        SessionStatus::AwaitingInput => "awaitingInput",
        SessionStatus::Errored => "errored",
        SessionStatus::Idle => "idle",
    };
    (label.into(), Some(age.num_seconds().max(0)))
}

fn summarize_chat(chat: &Chat, spaces: &[Space], sessions: &[Session]) -> Value {
    let space = chat
        .space_id
        .as_deref()
        .and_then(|id| spaces.iter().find(|s| s.id == id));
    let session = session_for(sessions, chat);
    let (status, status_age) = status_of(session.as_ref());
    json!({
        "id": chat.id,
        "title": chat.title,
        "project": space.map(|s| json!({
            "id": s.id,
            "name": s.display_name(),
            "path": s.path,
        })),
        "deviceId": chat.device_id,
        "cwd": chat.cwd.clone().or_else(|| space.map(|s| s.path.clone())),
        "branch": chat.branch,
        "harness": chat.config.as_ref().map(|c| c.harness),
        "model": chat.config.as_ref().and_then(|c| c.model.clone()),
        "reasoning": chat.config.as_ref().and_then(|c| c.reasoning),
        "archived": chat.archived,
        "kind": kind_of(chat),
        "parentChatId": chat.parent_chat_id,
        "spawnedByChatId": chat.spawned_by_chat_id,
        "status": status,
        "statusAgeSecs": status_age,
        "lastMessageAt": chat.last_message_at,
        "lastMessagePreview": chat.last_message_preview,
        "createdAt": chat.created_at,
    })
}

fn last_pending_input(messages: &[RenderedMessage]) -> Option<Value> {
    messages.iter().rev().find_map(|m| m.pending_input.clone())
}

// ---- dispatch ----------------------------------------------------------------

impl Tools {
    pub fn new(zeron: Arc<Zeron>) -> Self {
        Self {
            zeron,
            create_gate: tokio::sync::Mutex::new(()),
            created: std::sync::Mutex::new(Vec::new()),
            recent_creates: std::sync::Mutex::new(std::collections::VecDeque::new()),
        }
    }

    pub fn list(&self) -> Vec<ToolDef> {
        catalog()
    }

    pub fn has(&self, name: &str) -> bool {
        catalog().iter().any(|t| t.name == name)
    }

    /// `Ok` is the tool's structured result; `Err` is a message the model
    /// should read (surfaced as `isError`, never as a protocol error).
    pub async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let result = match name {
            "whoami" => self.whoami().await,
            "list_devices" => self.list_devices().await,
            "list_projects" => self.list_projects().await,
            "list_harnesses" => self.list_harnesses(parse(args)?).await,
            "list_models" => self.list_models(parse(args)?).await,
            "list_chats" => self.list_chats(parse(args)?).await,
            "get_chat" => self.get_chat(parse(args)?).await,
            // The create/send futures are large (resolution, host catalog
            // reads, delivery and an optional wait inline): box them so a
            // tool call never overflows a worker thread's stack.
            "create_chat" => Box::pin(self.create_chat(parse(args)?)).await,
            "create_chats" => Box::pin(self.batch(parse(args)?, true)).await,
            "send_messages" => Box::pin(self.batch(parse(args)?, false)).await,
            "read_chat" => self.read_chat(parse(args)?).await,
            "send_message" => Box::pin(self.send_message(parse(args)?)).await,
            "wait_for_turn" => self.wait_for_turn(parse(args)?).await,
            "interrupt_chat" => self.interrupt_chat(parse(args)?).await,
            "respond_to_input" => self.respond_to_input(parse(args)?).await,
            "archive_chat" => self.archive_chat(parse(args)?).await,
            other => return Err(format!("unknown tool: {other}")),
        };
        result.map_err(|e| e.to_string())
    }

    /// Poll every request together, including its optional wait. A waiting
    /// first chat must not prevent subsequent chats from receiving their work.
    async fn batch(&self, args: BatchArgs, create: bool) -> anyhow::Result<Value> {
        anyhow::ensure!(
            (1..=MAX_BATCH).contains(&args.requests.len()),
            "requests must contain between 1 and {MAX_BATCH} items"
        );
        let results = futures::future::join_all(args.requests.into_iter().enumerate().map(
            |(index, args)| async move {
                let result = if create {
                    match serde_json::from_value(args) {
                        Ok(args) => Box::pin(self.create_chat(args)).await,
                        Err(error) => Err(error.into()),
                    }
                } else {
                    match serde_json::from_value(args) {
                        Ok(args) => Box::pin(self.send_message(args)).await,
                        Err(error) => Err(error.into()),
                    }
                };
                match result {
                    Ok(result) => json!({"index": index, "isError": false, "result": result}),
                    Err(error) => {
                        json!({"index": index, "isError": true, "error": error.to_string()})
                    }
                }
            },
        ))
        .await;
        Ok(json!({"results": results}))
    }

    async fn whoami(&self) -> anyhow::Result<Value> {
        let origin = self.zeron.origin().clone();
        let local_device = self.zeron.local_device_id().await?;
        let engine = self.zeron.engine_info().await.unwrap_or(Value::Null);
        let mut spawning = json!({
            "maxSpawnDepth": MAX_SPAWN_DEPTH,
            "maxLiveSpawns": MAX_LIVE_SPAWNS,
            "maxCreatesPerMinute": SPAWN_RATE.0,
        });
        let chat = match origin.chat_id.as_deref() {
            Some(id) => match self.zeron.chats().await.and_then(|c| {
                resolve_chat_in(&c, id).map(|chat| (chat, c))
            }) {
                Ok((chat, chats)) => {
                    let (spaces, sessions) =
                        tokio::try_join!(self.zeron.spaces(), self.zeron.sessions())?;
                    let depth = spawn_depth(&chat.id, spawner_lookup(&chats));
                    spawning["spawnDepth"] = json!(depth);
                    spawning["liveSpawns"] = json!(live_spawns(&chats, &chat.id));
                    spawning["canCreateChats"] = json!(
                        !chat.is_side_chat() && depth.is_some_and(|d| d < MAX_SPAWN_DEPTH)
                    );
                    summarize_chat(&chat, &spaces, &sessions)
                }
                Err(_) => json!({ "id": id }),
            },
            None => Value::Null,
        };
        Ok(json!({
            "chat": chat,
            "originDeviceId": origin.device_id,
            "localDeviceId": local_device,
            "workspaceScope": engine.get("workspaceScope").cloned().unwrap_or(Value::Null),
            "spawning": spawning,
            "note": if origin.chat_id.is_some() {
                "Messages you send are attributed to this chat; it cannot message itself."
            } else {
                "Not running inside a chat: messages are sent without attribution."
            },
        }))
    }

    async fn list_devices(&self) -> anyhow::Result<Value> {
        let (devices, spaces, local) = tokio::try_join!(
            self.zeron.devices(),
            self.zeron.spaces(),
            self.zeron.local_device_id()
        )?;
        let now = chrono::Utc::now();
        Ok(json!({
            "devices": devices.iter().map(|d| json!({
                "id": d.id,
                "name": d.name,
                "platform": d.platform,
                "local": d.id == local,
                "online": is_online(d, &local, now),
                "executionHost": is_execution_host(d),
                "lastSeenAt": d.last_seen_at,
                "version": d.version,
                "projects": spaces.iter().filter(|s| s.device_id == d.id).map(|s| json!({
                    "id": s.id,
                    "name": s.display_name(),
                    "path": s.path,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>()
        }))
    }

    async fn list_projects(&self) -> anyhow::Result<Value> {
        let (spaces, devices, local) = tokio::try_join!(
            self.zeron.spaces(),
            self.zeron.devices(),
            self.zeron.local_device_id()
        )?;
        let now = chrono::Utc::now();
        let device = |id: &str| devices.iter().find(|d| d.id == id);
        Ok(json!({
            "projects": spaces.iter().map(|s| json!({
                "id": s.id,
                "name": s.display_name(),
                "path": s.path,
                "deviceId": s.device_id,
                "deviceName": device(&s.device_id).map(|d| d.name.clone()),
                "deviceOnline": s.device_id == local
                    || device(&s.device_id).is_some_and(|d| is_online(d, &local, now)),
                "git": s.git_detected,
            })).collect::<Vec<_>>()
        }))
    }

    /// `Some(id)` when `key` names another device (so reads route there),
    /// `None` for this engine's own. A remote target must be online.
    async fn remote_target(&self, key: Option<&str>) -> anyhow::Result<Option<String>> {
        if key.map(str::trim).is_none_or(str::is_empty) {
            return Ok(None);
        }
        let (devices, local) =
            tokio::try_join!(self.zeron.devices(), self.zeron.local_device_id())?;
        let id = self.zeron.resolve_device_id(key).await?;
        if id == local {
            return Ok(None);
        }
        let device = devices.iter().find(|d| d.id == id);
        if let Some(device) = device {
            check_host(device, &devices, &local)?;
        }
        Ok(Some(id))
    }

    async fn list_harnesses(&self, args: DeviceArgs) -> anyhow::Result<Value> {
        let target = self.remote_target(args.device.as_deref()).await?;
        let harnesses = self.zeron.harnesses_on(target.as_deref()).await?;
        Ok(json!({
            "deviceId": target,
            "harnesses": harnesses.iter().map(|h| json!({
                "id": h.id,
                "name": h.name,
                "available": h.available(),
                "installed": h.installed,
                "steersMidTurn": h.steers_mid_turn(),
                "reasoningLevels": h.reasoning_levels,
            })).collect::<Vec<_>>()
        }))
    }

    async fn list_models(&self, args: ListModelsArgs) -> anyhow::Result<Value> {
        let harness: HarnessId =
            parse_enum("harness", &args.harness).map_err(anyhow::Error::msg)?;
        let target = self.remote_target(args.device.as_deref()).await?;
        let models = self.zeron.models_on(harness, target.as_deref()).await?;
        Ok(json!({
            "harness": harness,
            "deviceId": target,
            "models": models.iter().map(|m| json!({
                "id": m.id,
                "label": m.label,
                "description": m.description,
                "reasoningLevels": m.reasoning_levels,
            })).collect::<Vec<_>>()
        }))
    }

    async fn list_chats(&self, args: ListChatsArgs) -> anyhow::Result<Value> {
        let (mut chats, spaces, sessions) = tokio::try_join!(
            self.zeron.chats(),
            self.zeron.spaces(),
            self.zeron.sessions()
        )?;
        if let Some(project) = args.project.as_deref() {
            let space = self.zeron.resolve_space(project).await?;
            chats.retain(|c| c.space_id.as_deref() == Some(space.id.as_str()));
        }
        if args.device.is_some() {
            let device = self.zeron.resolve_device_id(args.device.as_deref()).await?;
            chats.retain(|c| c.device_id == device);
        }
        if !args.include_archived {
            chats.retain(|c| !c.archived);
        }
        if let Some(parent) = args.parent.as_deref() {
            let parent = self.zeron.resolve_chat(parent).await?;
            chats.retain(|c| c.parent_chat_id.as_deref() == Some(parent.id.as_str()));
        }
        if let Some(spawner) = args.spawned_by.as_deref() {
            let spawner = self.zeron.resolve_chat(spawner).await?;
            chats.retain(|c| c.spawned_by_chat_id.as_deref() == Some(spawner.id.as_str()));
        }
        chats.sort_by(|a, b| {
            let a_at = a.last_message_at.unwrap_or(a.created_at);
            let b_at = b.last_message_at.unwrap_or(b.created_at);
            b_at.cmp(&a_at)
        });
        let total = chats.len();
        let limit = args.limit.unwrap_or(50).clamp(1, 500);
        Ok(json!({
            "total": total,
            "chats": chats.iter().take(limit)
                .map(|c| summarize_chat(c, &spaces, &sessions))
                .collect::<Vec<_>>()
        }))
    }

    async fn get_chat(&self, args: ChatArgs) -> anyhow::Result<Value> {
        let chat = self.zeron.resolve_chat(&args.chat).await?;
        let (spaces, sessions, entries) = tokio::try_join!(
            self.zeron.spaces(),
            self.zeron.sessions(),
            self.zeron.transcript(&chat.id)
        )?;
        let rendered = render_entries(&entries, RenderOptions::default());
        let mut summary = summarize_chat(&chat, &spaces, &sessions);
        summary["messageCount"] = json!(rendered.len());
        summary["pendingInput"] = last_pending_input(&rendered).unwrap_or(Value::Null);
        summary["lastMessage"] = rendered.last().map(|m| json!(m)).unwrap_or(Value::Null);
        Ok(summary)
    }

    async fn create_chat(&self, args: CreateChatArgs) -> anyhow::Result<Value> {
        let (chats, spaces, devices, local) = tokio::try_join!(
            self.zeron.chats(),
            self.zeron.spaces(),
            self.zeron.devices(),
            self.zeron.local_device_id()
        )?;

        // Provenance: the chat this server speaks for. It must be allowed to
        // spawn at all — side chats never, spawned chats only while shallow.
        let spawner = match self.zeron.origin().chat_id.as_deref() {
            Some(origin) => Some(resolve_chat_in(&chats, origin)?),
            None => None,
        };
        if let Some(spawner) = &spawner {
            anyhow::ensure!(
                !spawner.is_side_chat(),
                "Side chats cannot create chats. Ask your parent chat to create another side chat."
            );
            check_spawn_depth(spawner, &chats)?;
        }

        // Placement: `kind` decides whether the chat hangs off a parent.
        let explicit_parent = args
            .parent
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty());
        let kind = match args.kind.as_deref().map(str::trim).filter(|k| !k.is_empty()) {
            Some(raw) => Some(match raw.to_ascii_lowercase().as_str() {
                "side" | "side-chat" | "side_chat" => ChatKind::Side,
                "chat" | "top-level" | "top_level" | "toplevel" => ChatKind::Chat,
                _ => anyhow::bail!("unknown kind {raw:?}; expected \"side\" or \"chat\""),
            }),
            None => None,
        };
        let parent = match (kind, explicit_parent) {
            (Some(ChatKind::Chat), Some(_)) => anyhow::bail!(
                "parent only applies to kind \"side\"; a kind \"chat\" chat is top-level (its spawner is recorded as spawnedByChatId)"
            ),
            (Some(ChatKind::Chat), None) => None,
            (_, Some(key)) => Some(resolve_chat_in(&chats, key)?),
            (_, None) => spawner.clone(),
        };
        let kind = match (kind, &parent) {
            (Some(ChatKind::Side), None) => anyhow::bail!(
                "kind \"side\" needs a parent chat: run inside a Zeron chat or pass parent"
            ),
            (Some(kind), _) => kind,
            (None, Some(_)) => ChatKind::Side,
            (None, None) => ChatKind::Chat,
        };
        if let Some(parent) = &parent {
            anyhow::ensure!(
                !parent.is_side_chat(),
                "Cannot create a child of a side chat. Choose a top-level parent chat."
            );
        }

        // Host: a project (fixes device + folder) or a device, on any online
        // execution host in the workspace. That host's engine runs the chat.
        let (space, device_id) = resolve_target(
            args.project.as_deref(),
            args.device.as_deref(),
            &spaces,
            &devices,
            &local,
        )?;
        if let Some(device) = devices.iter().find(|d| d.id == device_id) {
            check_host(device, &devices, &local)?;
        } else if device_id != local {
            anyhow::bail!(
                "device {device_id} is not in this workspace; {}",
                host_listing(&devices, &local)
            );
        }
        let remote = (device_id != local).then_some(device_id.as_str());

        let harnesses = self.zeron.harnesses_on(remote).await.map_err(|e| {
            anyhow::anyhow!("could not reach device {device_id} to list its harnesses: {e}")
        })?;
        let harness = match args.harness.as_deref() {
            Some(raw) => {
                let id: HarnessId = parse_enum("harness", raw).map_err(anyhow::Error::msg)?;
                // The mock test rig is never offered in pickers (always
                // "disabled"), but an explicit request for it is honoured when
                // the host has it, as the dev rig's `ZERON_HARNESS=mock` does.
                if let Some(info) = harnesses.iter().find(|h| h.id == id)
                    && !info.available()
                    && !(id == HarnessId::Mock && info.installed)
                {
                    anyhow::bail!(
                        "harness {raw} is not available on device {device_id}; available there: {}",
                        available_harnesses(&harnesses)
                    );
                }
                id
            }
            None => default_harness(&harnesses)?,
        };
        if let Some(model) = args.model.as_deref()
            && let Ok(models) = self.zeron.models_on(harness, remote).await
            && !models.is_empty()
            && !models.iter().any(|m| m.id == model)
        {
            anyhow::bail!(
                "model {model:?} is not offered by {harness:?}; available: {}",
                models
                    .iter()
                    .map(|m| m.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let reasoning: Option<ReasoningLevel> = match args.reasoning.as_deref() {
            Some(raw) => Some(parse_enum("reasoning level", raw).map_err(anyhow::Error::msg)?),
            None => None,
        };
        let sandbox: SandboxLevel = match args.sandbox.as_deref() {
            Some(raw) => parse_enum("sandbox", raw).map_err(anyhow::Error::msg)?,
            None => SandboxLevel::WorkspaceWrite,
        };
        let branch = args
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_owned);
        let cwd = args
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_owned);
        let prompt = args.prompt.filter(|p| !p.trim().is_empty());
        let worktree = if args.worktree {
            let space = space
                .as_ref()
                .filter(|s| s.git_detected)
                .ok_or_else(|| anyhow::anyhow!("worktree needs a git project (see list_projects)"))?;
            anyhow::ensure!(
                prompt.is_some(),
                "worktree needs a prompt: the host creates the worktree when the first turn starts"
            );
            anyhow::ensure!(cwd.is_none(), "pass either worktree or cwd, not both");
            Some(WorktreeSpec {
                repo_path: space.path.clone(),
                base: branch.clone().unwrap_or_else(|| "HEAD".into()),
                space_id: Some(space.id.clone()),
            })
        } else {
            None
        };
        let config = ChatConfig {
            harness,
            model: args.model.clone(),
            reasoning,
            model_options: Default::default(),
            sandbox,
        };

        let chat_id = uuid::Uuid::new_v4().to_string();
        let spawned_by = spawner.as_ref().map(|c| c.id.clone());
        let parent_chat_id = parent.as_ref().map(|c| c.id.clone());
        let mut mutate = json!({
            "op": "createChat",
            "chatId": chat_id,
            "deviceId": device_id,
            "config": config,
        });
        if let Some(space) = &space {
            mutate["spaceId"] = json!(space.id);
        }
        if let Some(parent) = &parent_chat_id {
            mutate["parentChatId"] = json!(parent);
        }
        if let Some(spawner) = &spawned_by {
            mutate["spawnedByChatId"] = json!(spawner);
        }
        if let Some(branch) = &branch {
            mutate["branch"] = json!(branch);
        }
        if let Some(cwd) = &cwd {
            mutate["cwd"] = json!(cwd);
        }
        {
            // Count and write under one gate so a batch cannot overshoot.
            let _gate = self.create_gate.lock().await;
            if let Some(spawner) = &spawned_by {
                let live = {
                    let created = self.created.lock().unwrap();
                    live_spawns(&chats, spawner)
                        + created
                            .iter()
                            .filter(|id| !chats.iter().any(|c| &c.id == *id))
                            .count()
                };
                anyhow::ensure!(
                    live < MAX_LIVE_SPAWNS,
                    "chat {} already has {live} live spawned chats (limit {MAX_LIVE_SPAWNS}); archive finished ones with archive_chat first",
                    short(spawner)
                );
            }
            self.take_rate_slot()?;
            self.zeron.mutate(mutate).await?;
            self.created.lock().unwrap().push(chat_id.clone());
        }
        if let Some(title) = args
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            self.zeron
                .mutate(json!({ "op": "renameChat", "chatId": chat_id, "title": title }))
                .await?;
        }

        let device_name = devices
            .iter()
            .find(|d| d.id == device_id)
            .map(|d| d.name.clone());
        let mut result = json!({
            "chatId": chat_id,
            "kind": kind,
            "deviceId": device_id,
            "deviceName": device_name,
            "project": space.as_ref().map(|s| json!({ "id": s.id, "name": s.display_name(), "path": s.path })),
            "harness": harness,
            "model": args.model,
            "reasoning": reasoning,
            "title": args.title,
            "parentChatId": parent_chat_id,
            "spawnedByChatId": spawned_by,
        });
        if let Some(prompt) = prompt {
            // The row may not have folded into WatchChats yet; build the
            // chat locally from what we just wrote rather than re-reading.
            let chat = Chat {
                id: chat_id.clone(),
                device_id: device_id.clone(),
                title: args.title.clone(),
                archived: false,
                cwd: cwd.clone(),
                branch: branch.clone(),
                checkout_id: None,
                source_context: None,
                config: Some(config),
                last_message_preview: None,
                last_message_at: None,
                created_at: chrono::Utc::now(),
                harness_session_id: None,
                harness_session_cwd: None,
                parent_chat_id: parent_chat_id.clone(),
                spawned_by_chat_id: spawned_by.clone(),
                space_id: space.as_ref().map(|s| s.id.clone()),
                last_seen_at: None,
                room_gen: None,
            };
            let sent = self
                .deliver(&chat, space.as_ref(), &harnesses, None, prompt, "run", worktree)
                .await?;
            result["sent"] = sent;
            if args.wait {
                result["turn"] = self
                    .await_turn(
                        &chat,
                        None,
                        true,
                        wait_duration(args.timeout_secs),
                        now_millis(),
                    )
                    .await?;
            }
        }
        Ok(result)
    }

    /// One slot of the per-server creation budget ([`SPAWN_RATE`]).
    fn take_rate_slot(&self) -> anyhow::Result<()> {
        let (max, window) = SPAWN_RATE;
        let now = std::time::Instant::now();
        let mut recent = self.recent_creates.lock().unwrap();
        while recent
            .front()
            .is_some_and(|at| now.duration_since(*at) >= window)
        {
            recent.pop_front();
        }
        if recent.len() >= max {
            let wait = recent
                .front()
                .map(|at| window.saturating_sub(now.duration_since(*at)))
                .unwrap_or(window);
            anyhow::bail!(
                "created {max} chats in the last {}s; retry in {}s",
                window.as_secs(),
                wait.as_secs().max(1)
            );
        }
        recent.push_back(now);
        Ok(())
    }

    async fn read_chat(&self, args: ReadChatArgs) -> anyhow::Result<Value> {
        let chat = self.zeron.resolve_chat(&args.chat).await?;
        let entries = self.zeron.transcript(&chat.id).await?;
        let rendered = render_entries(
            &entries,
            RenderOptions {
                include_reasoning: args.include_reasoning,
                include_tools: args.include_tools.unwrap_or(true),
            },
        );
        let total = rendered.len();
        let limit = args.limit.unwrap_or(40).clamp(1, 500);
        let end = total.saturating_sub(args.offset);
        let start = end.saturating_sub(limit);
        let window = &rendered[start..end];
        Ok(json!({
            "chatId": chat.id,
            "title": chat.title,
            "total": total,
            "returned": window.len(),
            "olderRemaining": start,
            "newerSkipped": total - end,
            "pendingInput": last_pending_input(&rendered),
            "messages": window,
        }))
    }

    async fn send_message(&self, args: SendArgs) -> anyhow::Result<Value> {
        let text = args.text.trim();
        if text.is_empty() {
            anyhow::bail!("text is empty");
        }
        let chat = self.zeron.resolve_chat(&args.chat).await?;
        if self.zeron.origin().chat_id.as_deref() == Some(chat.id.as_str()) {
            anyhow::bail!(
                "refusing to send a message to your own chat ({})",
                short(&chat.id)
            );
        }
        let (spaces, sessions, harnesses) = tokio::try_join!(
            self.zeron.spaces(),
            self.zeron.sessions(),
            self.zeron.harnesses()
        )?;
        let space = chat
            .space_id
            .as_deref()
            .and_then(|id| spaces.iter().find(|s| s.id == id));
        let baseline = session_for(&sessions, &chat);
        let mode = args.mode.as_deref().unwrap_or("auto");
        let sent_at = now_millis();
        let body = self.attribute(&chat, text).await;
        let mut result = json!({
            "chatId": chat.id,
            "title": chat.title,
        });
        result["sent"] = self
            .deliver(&chat, space, &harnesses, baseline.as_ref(), body, mode, None)
            .await?;
        if args.wait {
            result["turn"] = self
                .await_turn(
                    &chat,
                    baseline.as_ref(),
                    true,
                    wait_duration(args.timeout_secs),
                    sent_at,
                )
                .await?;
        }
        Ok(result)
    }

    async fn wait_for_turn(&self, args: WaitArgs) -> anyhow::Result<Value> {
        let chat = self.zeron.resolve_chat(&args.chat).await?;
        let turn = self
            .await_turn(&chat, None, false, wait_duration(args.timeout_secs), 0)
            .await?;
        Ok(json!({ "chatId": chat.id, "title": chat.title, "turn": turn }))
    }

    async fn archive_chat(&self, args: ArchiveArgs) -> anyhow::Result<Value> {
        let chat = self.zeron.resolve_chat(&args.chat).await?;
        let archived = args.archived.unwrap_or(true);
        self.zeron
            .mutate(json!({ "op": "setChatArchived", "chatId": chat.id, "archived": archived }))
            .await?;
        Ok(json!({ "chatId": chat.id, "title": chat.title, "archived": archived }))
    }

    async fn interrupt_chat(&self, args: ChatArgs) -> anyhow::Result<Value> {
        let chat = self.zeron.resolve_chat(&args.chat).await?;
        let command_id = self
            .zeron
            .queue_command(&chat.id, &SessionCommandPayload::Interrupt {})
            .await?;
        Ok(json!({ "chatId": chat.id, "commandId": command_id }))
    }

    async fn respond_to_input(&self, args: RespondArgs) -> anyhow::Result<Value> {
        let chat = self.zeron.resolve_chat(&args.chat).await?;
        let request_id = match args.request_id {
            Some(id) => id,
            None => {
                let entries = self.zeron.transcript(&chat.id).await?;
                let rendered = render_entries(&entries, RenderOptions::default());
                last_pending_input(&rendered)
                    .and_then(|p| {
                        p.get("requestId")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("chat {} has no pending question", short(&chat.id))
                    })?
            }
        };
        if args.answers.is_empty() {
            anyhow::bail!("answers is empty");
        }
        let answers = args
            .answers
            .into_iter()
            .map(|a| UserInputAnswer {
                question_id: a.question_id,
                labels: a.labels,
            })
            .collect();
        let command_id = self
            .zeron
            .queue_command(
                &chat.id,
                &SessionCommandPayload::RespondInput {
                    request_id: request_id.clone(),
                    answers,
                },
            )
            .await?;
        Ok(json!({ "chatId": chat.id, "requestId": request_id, "commandId": command_id }))
    }

    // ---- shared pieces -------------------------------------------------------

    /// Prefix the sender's identity when this server speaks for a chat, so
    /// the receiving agent (and the human reading that transcript) can tell
    /// an agent-to-agent message from a typed one.
    async fn attribute(&self, target: &Chat, text: &str) -> String {
        let Some(origin_id) = self.zeron.origin().chat_id.as_deref() else {
            return text.to_owned();
        };
        if origin_id == target.id {
            return text.to_owned();
        }
        let title = match self.zeron.resolve_chat(origin_id).await {
            Ok(chat) => chat.title,
            Err(_) => None,
        };
        let label = match title {
            Some(title) if !title.trim().is_empty() => {
                format!("{} ({})", title.trim(), short(origin_id))
            }
            _ => short(origin_id).to_owned(),
        };
        format!(
            "[Message from Zeron chat {label}. Reply to it with the Zeron `send_message` tool, chat {}.]\n\n{text}",
            short(origin_id)
        )
    }

    /// Pick and perform the delivery the composer would. `worktree` only
    /// rides a first run (create_chat `worktree: true`).
    #[allow(clippy::too_many_arguments)]
    async fn deliver(
        &self,
        chat: &Chat,
        space: Option<&Space>,
        harnesses: &[HarnessInfo],
        session: Option<&Session>,
        text: String,
        mode: &str,
        worktree: Option<WorktreeSpec>,
    ) -> anyhow::Result<Value> {
        let harness = chat
            .config
            .as_ref()
            .map(|c| c.harness)
            .map_or_else(|| default_harness(harnesses), Ok)?;
        let (status, _) = status_of(session);
        // A quiet tool call can outlive the UI's stale-status window. Route
        // through steering and let the host decide whether a live run exists.
        let live = session.map(|s| s.status).unwrap_or(SessionStatus::Idle);
        let chosen = match mode {
            "auto" => match live {
                SessionStatus::Idle | SessionStatus::Errored => "run",
                SessionStatus::Working => "steer",
                SessionStatus::AwaitingInput => anyhow::bail!(
                    "chat {} is waiting for an answer; use respond_to_input (or mode 'queue' to hold this message for after the turn)",
                    short(&chat.id)
                ),
            },
            "run" | "steer" | "queue" => mode,
            other => anyhow::bail!("unknown mode {other:?}; expected auto, run, steer, or queue"),
        };
        let id = match chosen {
            "run" => {
                let config = chat.config.clone();
                let cwd = chat
                    .cwd
                    .clone()
                    .or_else(|| space.map(|s| s.path.clone()))
                    .unwrap_or_else(|| "~".into());
                let request = RunRequest {
                    mcp: None,
                    prompt: text,
                    harness: Some(harness),
                    model: config.as_ref().and_then(|c| c.model.clone()),
                    reasoning: config.as_ref().and_then(|c| c.reasoning),
                    model_options: config
                        .as_ref()
                        .map(|c| c.model_options.clone())
                        .unwrap_or_default(),
                    cwd,
                    sandbox: config
                        .as_ref()
                        .map(|c| c.sandbox)
                        .unwrap_or(SandboxLevel::WorkspaceWrite),
                    auto_approve: false,
                    resume: None,
                    attachments: Vec::new(),
                    worktree,
                };
                self.zeron
                    .queue_command(
                        &chat.id,
                        &SessionCommandPayload::Run {
                            request,
                            message_id: uuid::Uuid::new_v4().to_string(),
                        },
                    )
                    .await?
            }
            "steer" => {
                self.zeron
                    .queue_command(
                        &chat.id,
                        &SessionCommandPayload::Steer {
                            prompt: text,
                            message_id: Some(uuid::Uuid::new_v4().to_string()),
                        },
                    )
                    .await?
            }
            _ => self.zeron.queue_message(&chat.id, &text).await?,
        };
        Ok(json!({
            "delivery": chosen,
            "id": id,
            "chatStatusAtSend": status,
        }))
    }

    /// Wait, then report the outcome with the assistant messages that
    /// landed since `since_millis`.
    async fn await_turn(
        &self,
        chat: &Chat,
        baseline: Option<&Session>,
        expect_turn: bool,
        timeout: Duration,
        since_millis: i64,
    ) -> anyhow::Result<Value> {
        let (outcome, session) = self
            .zeron
            .wait_for_turn(chat, baseline, expect_turn, timeout)
            .await?;
        // The session row and the transcript sync separately: for a chat
        // hosted on another device the "idle" row can land here before the
        // reply does. After a completed send, give the replica a moment to
        // catch up instead of reporting an empty reply.
        let grace = std::time::Instant::now() + TRANSCRIPT_GRACE;
        let rendered = loop {
            let entries = self.zeron.transcript(&chat.id).await.unwrap_or_default();
            let rendered = render_entries(&entries, RenderOptions::default());
            let settled = rendered
                .iter()
                .rev()
                .find(|m| m.role == zeron_doc::MessageRole::Assistant)
                .is_some_and(|m| {
                    m.created_at >= since_millis.saturating_sub(2_000)
                        && m.status != Some(zeron_doc::MessageStatus::Streaming)
                });
            if outcome != TurnOutcome::Completed
                || since_millis == 0
                || settled
                || std::time::Instant::now() >= grace
            {
                break rendered;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        let replies: Vec<&RenderedMessage> = rendered
            .iter()
            .filter(|m| m.role == zeron_doc::MessageRole::Assistant)
            .filter(|m| m.created_at >= since_millis.saturating_sub(2_000))
            .collect();
        let replies: Vec<&RenderedMessage> = if replies.is_empty() {
            rendered
                .iter()
                .rev()
                .find(|m| m.role == zeron_doc::MessageRole::Assistant)
                .into_iter()
                .collect()
        } else {
            replies
        };
        let (status, _) = status_of(session.as_ref());
        Ok(json!({
            "outcome": outcome,
            "status": status,
            "timedOut": outcome == TurnOutcome::TimedOut,
            "pendingInput": last_pending_input(&rendered),
            "replies": replies,
        }))
    }
}

/// claude-code when it is offered here, else the first available harness.
fn default_harness(harnesses: &[HarnessInfo]) -> anyhow::Result<HarnessId> {
    if harnesses.is_empty() {
        return Ok(HarnessId::ClaudeCode);
    }
    harnesses
        .iter()
        .find(|h| h.id == HarnessId::ClaudeCode && h.available())
        .or_else(|| {
            harnesses
                .iter()
                .find(|h| h.available() && h.id != HarnessId::Mock)
        })
        .map(|h| h.id)
        .ok_or_else(|| {
            anyhow::anyhow!("no harness is available on this device (see list_harnesses)")
        })
}

fn available_harnesses(harnesses: &[HarnessInfo]) -> String {
    let ids: Vec<String> = harnesses
        .iter()
        .filter(|h| h.available())
        .map(|h| {
            serde_json::to_value(h.id)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| h.name.clone())
        })
        .collect();
    if ids.is_empty() {
        "none".into()
    } else {
        ids.join(", ")
    }
}

/// `id → its spawner` over a registry snapshot, for [`spawn_depth`].
fn spawner_lookup(chats: &[Chat]) -> impl Fn(&str) -> Option<String> + '_ {
    |id| {
        chats
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.spawned_by_chat_id.clone())
    }
}

/// Unarchived chats `spawner` created (side and top-level).
fn live_spawns(chats: &[Chat], spawner: &str) -> usize {
    chats
        .iter()
        .filter(|c| !c.archived && c.spawned_by_chat_id.as_deref() == Some(spawner))
        .count()
}

/// Refuse when `spawner` already sits at [`MAX_SPAWN_DEPTH`] (or its
/// provenance loops), so spawned top-level chats cannot recurse unbounded.
fn check_spawn_depth(spawner: &Chat, chats: &[Chat]) -> anyhow::Result<()> {
    match spawn_depth(&spawner.id, spawner_lookup(chats)) {
        Some(depth) if depth < MAX_SPAWN_DEPTH => Ok(()),
        Some(depth) => anyhow::bail!(
            "This chat is {depth} agent spawns deep (limit {MAX_SPAWN_DEPTH}); it can run and message chats but not create more. Ask the chat that spawned you to create it."
        ),
        None => anyhow::bail!("This chat's spawn provenance loops; it cannot create chats."),
    }
}

/// "Online execution hosts: …" for error messages.
fn host_listing(devices: &[Device], local: &str) -> String {
    let now = chrono::Utc::now();
    let hosts: Vec<String> = devices
        .iter()
        .filter(|d| is_execution_host(d) && is_online(d, local, now))
        .map(|d| {
            let tag = if d.id == local { ", this device" } else { "" };
            format!("{} ({}{tag})", d.name, d.id)
        })
        .collect();
    if hosts.is_empty() {
        "no execution host is online".into()
    } else {
        format!("online execution hosts: {}", hosts.join(", "))
    }
}

/// The chosen host must run an engine and be reachable right now: commands
/// to an offline host would sit in the doc until it returns, which an agent
/// waiting on a reply cannot tell from a hang.
fn check_host(device: &Device, devices: &[Device], local: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        is_execution_host(device),
        "device {} ({}) is a {} viewer without an agent engine, so it cannot run chats; {}",
        device.name,
        device.id,
        device.platform,
        host_listing(devices, local)
    );
    let now = chrono::Utc::now();
    if !is_online(device, local, now) {
        let seen = device.last_seen_at.map_or_else(
            || "never seen".to_owned(),
            |at| format!("last seen {}s ago", (now - at).num_seconds().max(0)),
        );
        anyhow::bail!(
            "device {} ({}) is offline ({seen}); {}",
            device.name,
            device.id,
            host_listing(devices, local)
        );
    }
    Ok(())
}

/// Project and/or device → (project, host device id). With both, the project
/// is looked up among that device's projects and must live there.
fn resolve_target(
    project: Option<&str>,
    device: Option<&str>,
    spaces: &[Space],
    devices: &[Device],
    local: &str,
) -> anyhow::Result<(Option<Space>, String)> {
    let project = project.map(str::trim).filter(|p| !p.is_empty());
    let device = device.map(str::trim).filter(|d| !d.is_empty());
    let device_id = match device {
        Some(key) => Some(resolve_device_in(devices, key)?),
        None => None,
    };
    match (project, device_id) {
        (Some(project), Some(device_id)) => {
            let on_device: Vec<Space> = spaces
                .iter()
                .filter(|s| s.device_id == device_id)
                .cloned()
                .collect();
            match resolve_space_in(&on_device, project) {
                Ok(space) => Ok((Some(space), device_id)),
                Err(_) => {
                    let listed: Vec<String> = on_device
                        .iter()
                        .map(|s| format!("{} ({})", s.path, s.id))
                        .collect();
                    anyhow::bail!(
                        "no project matches {project:?} on device {device_id}; its projects: {}",
                        if listed.is_empty() {
                            "none (omit project for a project-less chat)".into()
                        } else {
                            listed.join(", ")
                        }
                    )
                }
            }
        }
        (Some(project), None) => {
            // The same repo is often a project on several devices: an
            // ambiguous name means this device's copy when it has exactly one.
            let space = resolve_space_in(spaces, project).or_else(|err| {
                let local_spaces: Vec<Space> = spaces
                    .iter()
                    .filter(|s| s.device_id == local)
                    .cloned()
                    .collect();
                resolve_space_in(&local_spaces, project)
                    .map_err(|_| anyhow::anyhow!("{err} (or pass device to pick the host)"))
            })?;
            let device_id = space.device_id.clone();
            Ok((Some(space), device_id))
        }
        (None, Some(device_id)) => Ok((None, device_id)),
        (None, None) => Ok((None, local.to_owned())),
    }
}

/// Device id or exact (case-insensitive) name over a snapshot.
fn resolve_device_in(devices: &[Device], key: &str) -> anyhow::Result<String> {
    if let Some(device) = devices.iter().find(|d| d.id == key) {
        return Ok(device.id.clone());
    }
    let by_name: Vec<&Device> = devices
        .iter()
        .filter(|d| d.name.trim().eq_ignore_ascii_case(key))
        .collect();
    match by_name.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => anyhow::bail!(
            "no device matches {key:?}; known: {}",
            devices
                .iter()
                .map(|d| format!("{} ({})", d.name, d.id))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        many => anyhow::bail!(
            "{} devices are named {key:?}; use an id: {}",
            many.len(),
            many.iter()
                .map(|d| d.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zeron::Origin;
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::sync::Mutex;
    use zeron_rpc::{RpcError, RpcReply, RpcService, memory_client, methods};

    /// A fixed little workspace: one device, one project, one chat with a
    /// two-message transcript. Writes are recorded for assertions.
    #[derive(Default)]
    struct World {
        writes: Mutex<Vec<(String, Value)>>,
        /// Catalog reads (ListHarnesses/ListModels) with their params, to
        /// assert device routing.
        reads: Mutex<Vec<(String, Value)>>,
        dispatch_barrier: Option<tokio::sync::Barrier>,
        beta_parent: Option<String>,
        /// Extra chat rows appended to the two fixed ones.
        extra_chats: Vec<Value>,
        /// Simulate a remote host: every created chat's session row reads as
        /// a finished turn at once, while its reply reaches the transcript
        /// only from the third read on (the doc syncing behind the row).
        lagging_remote_reply: bool,
        transcript_reads: std::sync::atomic::AtomicUsize,
    }

    fn stream(item: Value) -> RpcReply {
        RpcReply::Stream(futures::stream::iter(vec![item]).boxed())
    }

    #[async_trait]
    impl RpcService for World {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            Ok(match method {
                methods::LOCAL_DEVICE => RpcReply::Value(json!({ "deviceId": "dev-local" })),
                methods::ENGINE_INFO => RpcReply::Value(json!({
                    "deviceId": "dev-local", "workspaceScope": "local"
                })),
                methods::WATCH_DEVICES => {
                    let now = chrono::Utc::now();
                    stream(json!([
                        { "id": "dev-local", "name": "Laptop", "platform": "linux",
                          "lastSeenAt": null },
                        { "id": "dev-gpu", "name": "GPU box", "platform": "linux",
                          "lastSeenAt": now - chrono::Duration::seconds(10),
                          "capabilities": ["message-queue-v1"] },
                        { "id": "dev-pixel", "name": "Pixel", "platform": "android",
                          "lastSeenAt": now, "capabilities": ["message-queue-v1"] },
                        { "id": "dev-old", "name": "Old desktop", "platform": "macos",
                          "lastSeenAt": now - chrono::Duration::days(2) },
                        { "id": "dev-phone", "name": "iPhone", "platform": "ios",
                          "lastSeenAt": now },
                    ]))
                }
                methods::WATCH_SPACES => stream(json!([
                    {
                        "id": "space-1", "deviceId": "dev-local", "path": "/repo/comet",
                        "gitDetected": true, "createdAt": "2026-09-01T00:00:00Z"
                    },
                    {
                        "id": "space-gpu", "deviceId": "dev-gpu", "path": "/srv/train",
                        "gitDetected": true, "createdAt": "2026-09-01T00:00:00Z"
                    },
                    {
                        "id": "space-gpu-comet", "deviceId": "dev-gpu", "path": "/home/gpu/comet",
                        "gitDetected": false, "createdAt": "2026-09-01T00:00:00Z"
                    }
                ])),
                methods::WATCH_CHATS => {
                    let mut chats = vec![
                        json!({
                            "id": "chat-alpha-1", "deviceId": "dev-local", "title": "Alpha",
                            "archived": false, "spaceId": "space-1",
                            "config": { "harness": "claude-code", "model": "opus", "reasoning": null, "sandbox": "workspace-write" },
                            "createdAt": "2026-09-01T00:00:00Z"
                        }),
                        json!({
                            "id": "chat-beta-2", "deviceId": "dev-local", "title": "Beta",
                            "parentChatId": self.beta_parent,
                            "archived": false, "spaceId": "space-1",
                            "createdAt": "2026-09-02T00:00:00Z"
                        }),
                    ];
                    chats.extend(self.extra_chats.iter().cloned());
                    stream(Value::Array(chats))
                }
                methods::WATCH_SESSIONS if self.lagging_remote_reply => {
                    let rows: Vec<Value> = self
                        .writes
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(_, p)| p["op"] == "createChat")
                        .map(|(_, p)| json!({
                            "chatId": p["chatId"], "deviceId": p["deviceId"], "status": "idle",
                            "startedAt": null, "updatedAt": chrono::Utc::now(),
                            "lastCompletedTurn": "turn-1"
                        }))
                        .collect();
                    stream(Value::Array(rows))
                }
                methods::WATCH_SESSIONS => stream(json!([])),
                methods::WATCH_DOC_MESSAGES
                    if self.lagging_remote_reply
                        && self
                            .transcript_reads
                            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                            >= 2 =>
                {
                    stream(json!({ "reset": [
                        { "id": "u1", "role": "user", "createdAt": 1, "deviceId": "dev-gpu",
                          "parts": [{ "kind": "text", "id": "t", "text": "go" }] },
                        { "id": "a9", "role": "assistant", "createdAt": now_millis(),
                          "deviceId": "dev-gpu", "status": "complete",
                          "parts": [{ "kind": "text", "id": "t", "text": "late reply" }] }
                    ]}))
                }
                methods::LIST_HARNESSES | methods::LIST_MODELS
                    if params.get("targetDeviceId").is_some() =>
                {
                    self.reads
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params.clone()));
                    if method == methods::LIST_MODELS {
                        RpcReply::Value(json!([{ "id": "gpu-model", "label": "GPU model" }]))
                    } else {
                        // The GPU box runs Codex only.
                        RpcReply::Value(json!([
                            { "id": "claude-code", "name": "Claude Code", "installed": false },
                            { "id": "codex", "name": "Codex", "installed": true, "enabled": true }
                        ]))
                    }
                }
                methods::LIST_HARNESSES => RpcReply::Value(json!([
                    { "id": "claude-code", "name": "Claude Code", "supportsSteering": true,
                      "steeringMode": "step-boundary", "reasoningLevels": [], "installed": true, "enabled": true },
                    { "id": "codex", "name": "Codex", "supportsSteering": true,
                      "steeringMode": "turn-boundary", "reasoningLevels": [], "installed": false, "enabled": true },
                    { "id": "mock", "name": "Mock", "installed": true, "enabled": false }
                ])),
                methods::LIST_MODELS => RpcReply::Value(json!([
                    { "id": "opus", "label": "Opus" }, { "id": "sonnet", "label": "Sonnet" }
                ])),
                methods::WATCH_DOC_MESSAGES => stream(json!({ "reset": [
                    { "id": "u1", "role": "user", "createdAt": 1, "deviceId": "dev-local",
                      "parts": [{ "kind": "text", "id": "t", "text": "hi" }] },
                    { "id": "a1", "role": "assistant", "createdAt": 2, "deviceId": "dev-local",
                      "status": "complete",
                      "parts": [{ "kind": "text", "id": "t", "text": "hello back" }] }
                ]})),
                methods::MUTATE | methods::QUEUE_COMMAND | methods::QUEUE_MESSAGE => {
                    self.writes
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params));
                    if method == methods::QUEUE_COMMAND
                        && let Some(barrier) = &self.dispatch_barrier
                    {
                        // No dispatch can finish until both chats have work.
                        // A sequential batch deadlocks here, before any waits.
                        barrier.wait().await;
                    }
                    RpcReply::Value(json!({ "commandId": "cmd-1", "id": "q-1" }))
                }
                other => return Err(RpcError::UnknownMethod(other.into())),
            })
        }
    }

    fn tools(world: Arc<World>, origin: Origin) -> Tools {
        let client = memory_client(world);
        Tools::new(Arc::new(Zeron::with_client(client, origin)))
    }

    #[tokio::test]
    async fn catalog_is_well_formed() {
        let defs = catalog();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), defs.len(), "tool names must be unique");
        for def in &defs {
            assert_eq!(def.input_schema["type"], "object", "{}", def.name);
            assert!(!def.description.is_empty());
        }
    }

    #[tokio::test]
    async fn list_and_read_chats() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let listed = tools.call("list_chats", json!({})).await.unwrap();
        assert_eq!(listed["total"], 2);
        // Newest activity first: Beta was created later.
        assert_eq!(listed["chats"][0]["title"], "Beta");
        assert_eq!(listed["chats"][1]["project"]["name"], "comet");
        assert_eq!(listed["chats"][1]["status"], "idle");

        let read = tools
            .call("read_chat", json!({ "chat": "alpha" }))
            .await
            .unwrap();
        assert_eq!(read["total"], 2);
        assert_eq!(read["messages"][1]["text"], "hello back");

        let read = tools
            .call("read_chat", json!({ "chat": "chat-alpha", "limit": 1 }))
            .await
            .unwrap();
        assert_eq!(read["returned"], 1);
        assert_eq!(read["olderRemaining"], 1);
        assert_eq!(read["messages"][0]["id"], "a1");
    }

    #[tokio::test]
    async fn send_attributes_and_refuses_self() {
        let world = Arc::new(World::default());
        let tools = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-beta-2".into()),
                device_id: Some("dev-local".into()),
            },
        );
        let err = tools
            .call("send_message", json!({ "chat": "beta", "text": "loop" }))
            .await
            .unwrap_err();
        assert!(err.contains("own chat"), "{err}");

        let sent = tools
            .call(
                "send_message",
                json!({ "chat": "alpha", "text": "please review" }),
            )
            .await
            .unwrap();
        assert_eq!(sent["sent"]["delivery"], "run");
        let writes = world.writes.lock().unwrap();
        let (method, params) = writes.last().expect("a queued command");
        assert_eq!(method, methods::QUEUE_COMMAND);
        assert_eq!(params["chatId"], "chat-alpha-1");
        assert_eq!(params["command"]["kind"], "run");
        let prompt = params["command"]["request"]["prompt"].as_str().unwrap();
        assert!(
            prompt.starts_with("[Message from Zeron chat Beta (chat-bet)"),
            "{prompt}"
        );
        assert!(prompt.ends_with("please review"));
        assert_eq!(params["command"]["request"]["cwd"], "/repo/comet");
        assert_eq!(params["command"]["request"]["harness"], "claude-code");
        assert_eq!(params["command"]["request"]["model"], "opus");
    }

    #[tokio::test]
    async fn auto_steers_busy_chats_even_at_turn_boundaries_or_after_long_quiet_tools() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let chats = tools.zeron.chats().await.unwrap();
        let session = Session {
            chat_id: chats[0].id.clone(),
            device_id: "dev-local".into(),
            status: SessionStatus::Working,
            started_at: None,
            updated_at: chrono::Utc::now() - chrono::Duration::minutes(10),
            last_completed_turn: None,
        };
        // No mid-turn capability is required for a live mailbox delivery.
        let sent = tools
            .deliver(
                &chats[0],
                None,
                &[],
                Some(&session),
                "follow up".into(),
                "auto",
                None,
            )
            .await
            .unwrap();
        assert_eq!(sent["delivery"], "steer");
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, methods::QUEUE_COMMAND);
        assert_eq!(writes[0].1["command"]["kind"], "steer");
    }

    #[tokio::test]
    async fn create_chat_writes_the_row_and_validates_model() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let err = tools
            .call(
                "create_chat",
                json!({ "project": "comet", "model": "nope" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("not offered"), "{err}");

        let created = tools
            .call(
                "create_chat",
                json!({ "project": "/repo/comet", "model": "sonnet", "title": "Review", "prompt": "go" }),
            )
            .await
            .unwrap();
        let chat_id = created["chatId"].as_str().unwrap().to_owned();
        let writes = world.writes.lock().unwrap();
        assert_eq!(writes.len(), 3, "createChat, renameChat, run");
        assert_eq!(writes[0].1["op"], "createChat");
        assert_eq!(writes[0].1["chatId"], chat_id);
        assert_eq!(writes[0].1["spaceId"], "space-1");
        assert!(
            writes[0].1.get("parentChatId").is_none(),
            "no origin, no parent"
        );
        assert_eq!(writes[0].1["config"]["harness"], "claude-code");
        assert_eq!(writes[0].1["config"]["model"], "sonnet");
        assert_eq!(writes[1].1["op"], "renameChat");
        assert_eq!(writes[2].0, methods::QUEUE_COMMAND);
        assert_eq!(writes[2].1["command"]["request"]["prompt"], "go");
    }

    #[tokio::test]
    async fn create_chat_records_the_origin_as_parent() {
        let world = Arc::new(World::default());
        let tools = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-beta-2".into()),
                device_id: None,
            },
        );
        let created = tools
            .call("create_chat", json!({ "project": "/repo/comet" }))
            .await
            .unwrap();
        assert_eq!(created["parentChatId"], "chat-beta-2");
        assert_eq!(
            world.writes.lock().unwrap()[0].1["parentChatId"],
            "chat-beta-2"
        );

        // An explicit parent (by title) overrides the origin.
        let created = tools
            .call(
                "create_chat",
                json!({ "project": "/repo/comet", "parent": "Alpha" }),
            )
            .await
            .unwrap();
        assert_eq!(created["parentChatId"], "chat-alpha-1");
        let err = tools
            .call(
                "create_chat",
                json!({ "project": "/repo/comet", "parent": "nope" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("no chat matches"), "{err}");
    }

    #[tokio::test]
    async fn wait_on_an_idle_chat_returns_immediately() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let waited = tools
            .call(
                "wait_for_turn",
                json!({ "chat": "alpha", "timeout_secs": 5 }),
            )
            .await
            .unwrap();
        assert_eq!(waited["turn"]["outcome"], "completed");
        assert_eq!(waited["turn"]["replies"][0]["text"], "hello back");
    }

    #[tokio::test]
    async fn send_with_wait_keeps_waiting_until_a_turn_lands() {
        // The stub never grows a session row, so a send-then-wait must run
        // to its deadline rather than declaring the (unstarted) run done.
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let started = std::time::Instant::now();
        let sent = tools
            .call(
                "send_message",
                json!({ "chat": "alpha", "text": "hi", "wait": true, "timeout_secs": 1 }),
            )
            .await
            .unwrap();
        assert_eq!(sent["turn"]["outcome"], "timedOut");
        assert!(started.elapsed() >= Duration::from_millis(900));
    }

    #[tokio::test]
    async fn batches_dispatch_all_chats_before_waiting_and_keep_partial_results() {
        for (name, requests) in [
            (
                "create_chats",
                json!([
                    {"prompt": "first", "wait": true, "timeout_secs": 1},
                    {"harness": "not-a-harness"},
                    {"prompt": "second", "wait": true, "timeout_secs": 1},
                ]),
            ),
            (
                "send_messages",
                json!([
                    {"chat": "alpha", "text": "first", "wait": true, "timeout_secs": 1},
                    {"chat": "missing", "text": "invalid"},
                    {"chat": "beta", "text": "second", "wait": true, "timeout_secs": 1},
                ]),
            ),
        ] {
            let world = Arc::new(World {
                dispatch_barrier: Some(tokio::sync::Barrier::new(2)),
                ..Default::default()
            });
            let tools = tools(world.clone(), Origin::default());
            let reply = tokio::time::timeout(
                Duration::from_secs(3),
                crate::jsonrpc::handle_request(
                    &tools,
                    json!(42),
                    "tools/call",
                    json!({"name": name, "arguments": {"requests": requests}}),
                ),
            )
            .await
            .expect("both dispatches must proceed while other requests are waiting");
            assert_eq!(reply["result"]["isError"], false, "{reply}");
            let results = &reply["result"]["structuredContent"]["results"];
            for i in [0, 2] {
                assert_eq!(results[i]["index"], i);
                assert_eq!(results[i]["isError"], false, "{reply}");
                assert_eq!(results[i]["result"]["turn"]["outcome"], "timedOut");
            }
            assert_eq!(results[1]["index"], 1);
            assert_eq!(results[1]["isError"], true);
            let writes = world.writes.lock().unwrap();
            let dispatched: Vec<_> = writes
                .iter()
                .filter(|(method, _)| method == methods::QUEUE_COMMAND)
                .collect();
            assert_eq!(dispatched.len(), 2);
            assert_ne!(dispatched[0].1["chatId"], dispatched[1].1["chatId"]);
        }
    }

    #[tokio::test]
    async fn invalid_batch_sizes_have_no_side_effects() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        for name in ["create_chats", "send_messages"] {
            for requests in [vec![], vec![json!({"prompt": "hello"}); MAX_BATCH + 1]] {
                assert!(
                    tools
                        .call(name, json!({"requests": requests}))
                        .await
                        .is_err()
                );
            }
        }
        assert!(world.writes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn side_chats_cannot_create_chats_or_be_parents() {
        let world = Arc::new(World {
            beta_parent: Some("chat-alpha-1".into()),
            ..Default::default()
        });
        let side = tools(
            world.clone(),
            Origin {
                chat_id: Some("chat-beta-2".into()),
                device_id: None,
            },
        );
        for args in [json!({}), json!({"parent":"Alpha"})] {
            assert!(
                side.call("create_chat", args)
                    .await
                    .unwrap_err()
                    .contains("Side chats cannot")
            );
        }
        let batch = side
            .call("create_chats", json!({"requests":[{}, {"parent":"Alpha"}]}))
            .await
            .unwrap();
        assert!(
            batch["results"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["isError"] == true)
        );
        let root = tools(world.clone(), Origin::default());
        assert!(
            root.call("create_chat", json!({"parent":"Beta"}))
                .await
                .unwrap_err()
                .contains("child of a side chat")
        );
        assert!(world.writes.lock().unwrap().is_empty());
    }

    fn origin(chat: &str) -> Origin {
        Origin {
            chat_id: Some(chat.into()),
            device_id: Some("dev-local".into()),
        }
    }

    fn creates(world: &World) -> Vec<Value> {
        world
            .writes
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p)| m == methods::MUTATE && p["op"] == "createChat")
            .map(|(_, p)| p.clone())
            .collect()
    }

    #[tokio::test]
    async fn kinds_record_placement_and_provenance() {
        let world = Arc::new(World::default());
        let alpha = tools(world.clone(), origin("chat-alpha-1"));

        // Default inside a chat: a side chat under it, spawned by it.
        let side = alpha
            .call("create_chat", json!({ "project": "comet" }))
            .await
            .unwrap();
        assert_eq!(side["kind"], "side");
        assert_eq!(side["parentChatId"], "chat-alpha-1");
        assert_eq!(side["spawnedByChatId"], "chat-alpha-1");

        // kind chat: top-level (no parent) with provenance only.
        let top = alpha
            .call("create_chat", json!({ "project": "comet", "kind": "chat" }))
            .await
            .unwrap();
        assert_eq!(top["kind"], "chat");
        assert!(top["parentChatId"].is_null());
        assert_eq!(top["spawnedByChatId"], "chat-alpha-1");

        let rows = creates(&world);
        assert_eq!(rows[0]["parentChatId"], "chat-alpha-1");
        assert_eq!(rows[0]["spawnedByChatId"], "chat-alpha-1");
        assert!(rows[1].get("parentChatId").is_none(), "{}", rows[1]);
        assert_eq!(rows[1]["spawnedByChatId"], "chat-alpha-1");

        // A top-level chat has no placement parent to override.
        let err = alpha
            .call(
                "create_chat",
                json!({ "kind": "chat", "parent": "Alpha", "project": "comet" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("parent only applies"), "{err}");
        let err = alpha
            .call("create_chat", json!({ "kind": "sidebar" }))
            .await
            .unwrap_err();
        assert!(err.contains("unknown kind"), "{err}");

        // Outside a chat: nothing to hang a side chat under.
        let terminal = tools(world.clone(), Origin::default());
        let err = terminal
            .call("create_chat", json!({ "kind": "side" }))
            .await
            .unwrap_err();
        assert!(err.contains("needs a parent"), "{err}");
        let plain = terminal.call("create_chat", json!({})).await.unwrap();
        assert_eq!(plain["kind"], "chat");
        assert!(plain["spawnedByChatId"].is_null());
        assert_eq!(creates(&world).len(), 3);
    }

    #[tokio::test]
    async fn targets_any_online_execution_host() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), origin("chat-alpha-1"));

        // A project on the GPU box: that device hosts it, and harness and
        // model checks ask the GPU box, not this engine.
        let err = tools
            .call(
                "create_chat",
                json!({ "project": "/srv/train", "kind": "chat", "harness": "claude-code" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("not available on device dev-gpu"), "{err}");
        assert!(err.contains("codex"), "{err}");
        let created = tools
            .call(
                "create_chat",
                json!({ "project": "train", "kind": "chat", "harness": "codex",
                        "model": "gpu-model", "prompt": "train it" }),
            )
            .await
            .unwrap();
        assert_eq!(created["deviceId"], "dev-gpu");
        assert_eq!(created["deviceName"], "GPU box");
        assert_eq!(created["project"]["id"], "space-gpu");
        {
            let reads = world.reads.lock().unwrap();
            assert!(reads.iter().all(|(_, p)| p["targetDeviceId"] == "dev-gpu"));
            assert!(reads.iter().any(|(m, _)| m == methods::LIST_MODELS));
        }
        let run = world
            .writes
            .lock()
            .unwrap()
            .iter()
            .find(|(m, _)| m == methods::QUEUE_COMMAND)
            .cloned()
            .unwrap();
        assert_eq!(run.1["command"]["request"]["cwd"], "/srv/train");
        assert_eq!(run.1["command"]["request"]["harness"], "codex");

        // Project-less on a device by name; a side chat can live there too.
        let side = tools
            .call("create_chat", json!({ "device": "gpu box", "harness": "codex" }))
            .await
            .unwrap();
        assert_eq!(side["deviceId"], "dev-gpu");
        assert_eq!(side["kind"], "side");
        assert!(side["project"].is_null());

        // An on-device Android engine is a host like any other.
        let phone = tools
            .call("create_chat", json!({ "device": "Pixel", "harness": "codex" }))
            .await
            .unwrap();
        assert_eq!(phone["deviceId"], "dev-pixel");

        // device + project: the project is looked up on that device.
        let scoped = tools
            .call(
                "create_chat",
                json!({ "device": "dev-gpu", "project": "comet", "harness": "codex" }),
            )
            .await
            .unwrap();
        assert_eq!(scoped["project"]["id"], "space-gpu-comet");
        let err = tools
            .call(
                "create_chat",
                json!({ "device": "dev-gpu", "project": "/repo/comet" }),
            )
            .await
            .unwrap_err();
        assert!(err.contains("on device dev-gpu"), "{err}");
        assert!(err.contains("/srv/train (space-gpu)"), "{err}");

        // Offline hosts, phone clients and unknown devices are refused with
        // the valid choices listed.
        for (device, want) in [
            ("Old desktop", "is offline"),
            ("iPhone", "ios viewer without an agent engine"),
            ("nope", "no device matches"),
        ] {
            let err = tools
                .call("create_chat", json!({ "device": device }))
                .await
                .unwrap_err();
            assert!(err.contains(want), "{device}: {err}");
            if want != "no device matches" {
                assert!(err.contains("GPU box (dev-gpu)"), "{err}");
                assert!(err.contains("Laptop (dev-local, this device)"), "{err}");
                let hosts = err.split("online execution hosts:").nth(1).unwrap();
                assert!(!hosts.contains("iPhone"), "{err}");
                assert!(!hosts.contains("Old desktop"), "{err}");
            }
        }
        let err = tools
            .call("create_chat", json!({ "project": "comet" , "device": "Old desktop"}))
            .await
            .unwrap_err();
        assert!(err.contains("no project matches"), "{err}");
        // Without a device, a name shared across devices means this device's.
        let local = tools
            .call("create_chat", json!({ "project": "comet" }))
            .await
            .unwrap();
        assert_eq!(local["deviceId"], "dev-local");
        // No harness named: the host's own default (the GPU box has Codex).
        let default = tools
            .call("create_chat", json!({ "project": "train" }))
            .await
            .unwrap();
        assert_eq!(default["harness"], "codex");
        assert_eq!(creates(&world).len(), 6);
    }

    #[tokio::test]
    async fn a_remote_reply_that_syncs_after_the_session_row_is_still_returned() {
        let world = Arc::new(World {
            lagging_remote_reply: true,
            ..Default::default()
        });
        let tools = tools(world.clone(), origin("chat-alpha-1"));
        let created = tools
            .call(
                "create_chat",
                json!({ "kind": "chat", "device": "GPU box", "harness": "codex",
                        "prompt": "go", "wait": true, "timeout_secs": 10 }),
            )
            .await
            .unwrap();
        assert_eq!(created["turn"]["outcome"], "completed");
        assert_eq!(created["turn"]["replies"][0]["text"], "late reply", "{created}");
        assert!(world.transcript_reads.load(std::sync::atomic::Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn the_mock_rig_is_explicit_only() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let created = tools
            .call("create_chat", json!({ "harness": "mock" }))
            .await
            .unwrap();
        assert_eq!(created["harness"], "mock");
        let err = tools
            .call("create_chat", json!({ "harness": "codex" }))
            .await
            .unwrap_err();
        assert!(err.contains("available there: claude-code"), "{err}");
        let default = tools.call("create_chat", json!({})).await.unwrap();
        assert_eq!(default["harness"], "claude-code");
    }

    #[tokio::test]
    async fn device_listings_expose_hosts_and_their_projects() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), Origin::default());
        let listed = tools.call("list_devices", json!({})).await.unwrap();
        let by_id = |id: &str| {
            listed["devices"]
                .as_array()
                .unwrap()
                .iter()
                .find(|d| d["id"] == id)
                .cloned()
                .unwrap()
        };
        assert_eq!(by_id("dev-local")["online"], true);
        assert_eq!(by_id("dev-local")["local"], true);
        assert_eq!(by_id("dev-gpu")["online"], true);
        assert_eq!(by_id("dev-gpu")["executionHost"], true);
        assert_eq!(by_id("dev-gpu")["projects"][0]["path"], "/srv/train");
        assert_eq!(by_id("dev-old")["online"], false);
        assert_eq!(by_id("dev-phone")["executionHost"], false);
        assert_eq!(by_id("dev-pixel")["executionHost"], true);

        let projects = tools.call("list_projects", json!({})).await.unwrap();
        assert_eq!(projects["projects"][1]["deviceName"], "GPU box");
        assert_eq!(projects["projects"][1]["deviceOnline"], true);

        let remote = tools
            .call("list_harnesses", json!({ "device": "GPU box" }))
            .await
            .unwrap();
        assert_eq!(remote["deviceId"], "dev-gpu");
        assert_eq!(remote["harnesses"][1]["available"], true);
        let local = tools.call("list_harnesses", Value::Null).await.unwrap();
        assert!(local["deviceId"].is_null());
        let models = tools
            .call("list_models", json!({ "harness": "codex", "device": "dev-gpu" }))
            .await
            .unwrap();
        assert_eq!(models["models"][0]["id"], "gpu-model");
        let err = tools
            .call("list_harnesses", json!({ "device": "Old desktop" }))
            .await
            .unwrap_err();
        assert!(err.contains("offline"), "{err}");
    }

    fn spawned(id: &str, by: &str, parent: Option<&str>, archived: bool) -> Value {
        json!({
            "id": id, "deviceId": "dev-local", "title": id, "archived": archived,
            "spaceId": "space-1", "spawnedByChatId": by, "parentChatId": parent,
            "createdAt": "2026-09-03T00:00:00Z"
        })
    }

    #[tokio::test]
    async fn spawned_top_level_chats_spawn_until_the_depth_limit() {
        // alpha (user) → w1 → w2 → w3: depth 1, 2, 3.
        let world = Arc::new(World {
            extra_chats: vec![
                spawned("w1", "chat-alpha-1", None, false),
                spawned("w2", "w1", None, false),
                spawned("w3", "w2", None, false),
                spawned("w2-side", "w2", Some("w2"), false),
            ],
            ..Default::default()
        });
        for (from, ok) in [("w1", true), ("w2", true), ("w3", false)] {
            let result = tools(world.clone(), origin(from))
                .call("create_chat", json!({ "kind": "chat" }))
                .await;
            match (ok, result) {
                (true, Ok(created)) => assert_eq!(created["spawnedByChatId"], from),
                (false, Err(err)) => {
                    assert!(err.contains("3 agent spawns deep"), "{err}");
                    assert!(err.contains("limit 3"), "{err}");
                }
                (ok, other) => panic!("{from}: expected ok={ok}, got {other:?}"),
            }
        }
        // A spawned side chat still cannot create anything.
        let err = tools(world.clone(), origin("w2-side"))
            .call("create_chat", json!({ "kind": "chat" }))
            .await
            .unwrap_err();
        assert!(err.contains("Side chats cannot"), "{err}");

        let who = tools(world.clone(), origin("w3"))
            .call("whoami", json!({}))
            .await
            .unwrap();
        assert_eq!(who["spawning"]["spawnDepth"], 3);
        assert_eq!(who["spawning"]["canCreateChats"], false);
        assert_eq!(who["chat"]["kind"], "chat");
        assert_eq!(who["chat"]["spawnedByChatId"], "w2");

        // spawned_by lists both kinds; parent only the side chats.
        let listed = tools(world.clone(), Origin::default())
            .call("list_chats", json!({ "spawned_by": "w2" }))
            .await
            .unwrap();
        let mut ids: Vec<&str> = listed["chats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, ["w2-side", "w3"]);
        let listed = tools(world.clone(), Origin::default())
            .call("list_chats", json!({ "parent": "w2" }))
            .await
            .unwrap();
        assert_eq!(listed["total"], 1);
        assert_eq!(listed["chats"][0]["kind"], "side");
    }

    #[tokio::test]
    async fn fan_out_is_capped_by_live_spawns_and_rate() {
        // 31 live spawns (plus archived ones, which do not count): one more
        // fits, the next is refused until something is archived.
        let mut extra: Vec<Value> = (0..MAX_LIVE_SPAWNS - 1)
            .map(|i| spawned(&format!("live-{i}"), "chat-alpha-1", None, false))
            .collect();
        extra.push(spawned("done", "chat-alpha-1", None, true));
        let world = Arc::new(World {
            extra_chats: extra,
            ..Default::default()
        });
        let alpha = tools(world.clone(), origin("chat-alpha-1"));
        let batch = alpha
            .call(
                "create_chats",
                json!({ "requests": [{ "kind": "chat" }, { "kind": "chat" }, { "kind": "chat" }] }),
            )
            .await
            .unwrap();
        let errors: Vec<&Value> = batch["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["isError"] == true)
            .collect();
        assert_eq!(errors.len(), 2, "{batch}");
        assert!(
            errors[0]["error"]
                .as_str()
                .unwrap()
                .contains("archive finished ones")
        );
        assert_eq!(creates(&world).len(), 1);

        // The per-server rate limit applies even without a spawner.
        let world = Arc::new(World::default());
        let terminal = tools(world.clone(), Origin::default());
        for _ in 0..SPAWN_RATE.0 {
            terminal.call("create_chat", json!({})).await.unwrap();
        }
        let err = terminal.call("create_chat", json!({})).await.unwrap_err();
        assert!(err.contains("retry in"), "{err}");
    }

    #[tokio::test]
    async fn worktree_runs_the_first_turn_in_a_host_created_checkout() {
        let world = Arc::new(World::default());
        let tools = tools(world.clone(), origin("chat-alpha-1"));
        for (args, want) in [
            (json!({ "project": "comet", "worktree": true }), "needs a prompt"),
            (
                json!({ "device": "dev-gpu", "project": "comet", "worktree": true, "prompt": "x" }),
                "git project",
            ),
            (
                json!({ "project": "comet", "worktree": true, "prompt": "x", "cwd": "/w" }),
                "not both",
            ),
        ] {
            let err = tools.call("create_chat", args).await.unwrap_err();
            assert!(err.contains(want), "{err}");
        }
        tools
            .call(
                "create_chat",
                json!({ "project": "comet", "kind": "chat", "worktree": true,
                        "branch": "main", "prompt": "fix it" }),
            )
            .await
            .unwrap();
        let writes = world.writes.lock().unwrap();
        let (_, run) = writes
            .iter()
            .find(|(m, _)| m == methods::QUEUE_COMMAND)
            .unwrap();
        let spec = &run["command"]["request"]["worktree"];
        assert_eq!(spec["repoPath"], "/repo/comet");
        assert_eq!(spec["base"], "main");
        assert_eq!(spec["spaceId"], "space-1");
    }

    #[tokio::test]
    async fn initialize_and_list_over_jsonrpc() {
        let world = Arc::new(World::default());
        let tools = tools(world, Origin::default());
        let init = crate::jsonrpc::handle_request(
            &tools,
            json!(1),
            "initialize",
            json!({ "protocolVersion": "2025-03-26" }),
        )
        .await;
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "zeron");
        let list =
            crate::jsonrpc::handle_request(&tools, json!(2), "tools/list", Value::Null).await;
        assert!(list["result"]["tools"].as_array().unwrap().len() >= 10);
        let bad = crate::jsonrpc::handle_request(
            &tools,
            json!(3),
            "tools/call",
            json!({ "name": "nope" }),
        )
        .await;
        assert_eq!(bad["error"]["code"], -32602);
        let whoami = crate::jsonrpc::handle_request(
            &tools,
            json!(4),
            "tools/call",
            json!({ "name": "whoami" }),
        )
        .await;
        assert_eq!(whoami["result"]["isError"], false);
        assert_eq!(
            whoami["result"]["structuredContent"]["localDeviceId"],
            "dev-local"
        );
    }
}
