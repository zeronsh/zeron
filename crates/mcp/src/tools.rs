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
    Chat, ChatConfig, HarnessId, ReasoningLevel, RunRequest, SandboxLevel, Session, SessionStatus,
    Space, UserInputAnswer,
};

use crate::transcript::{RenderOptions, RenderedMessage, render_entries};
use crate::zeron::{HarnessInfo, TurnOutcome, Zeron, session_for, short};

/// Default and ceiling for the blocking waits.
const MAX_BATCH: usize = 32;
const DEFAULT_WAIT: Duration = Duration::from_secs(600);
const MAX_WAIT: Duration = Duration::from_secs(3600);
/// A session row older than this is not trusted to still be working
/// (the UI's staleness window): a crashed host must not read as busy forever.
const SESSION_STALE: chrono::Duration = chrono::Duration::seconds(45);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub struct Tools {
    zeron: Arc<Zeron>,
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
            description: "Devices in this workspace (the local engine's device is flagged). Chats and projects are hosted on a device.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "list_projects",
            description: "Projects: a folder on a device. Each chat belongs to one project, which fixes its host device and working directory.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "list_harnesses",
            description: "Agent harnesses (claude-code, codex, cursor, …) and whether each is available on this device.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: "list_models",
            description: "Models a harness offers on this device. Model ids are harness-specific strings; pass one to create_chat.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "harness": { "type": "string", "description": "Harness id, e.g. claude-code or codex." }
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
                    "parent": { "type": "string", "description": "Only chats created by this chat (id, prefix, or title) — e.g. your own id to list the chats you spawned." },
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
            description: "Create a chat in a project (or project-less on a device) with a harness and model. The new chat records your chat as its parent (parentChatId). Optionally send a first prompt and wait for the reply. Returns the new chat id. For parallel delegation use create_chats, or leave wait=false on every launch and wait only after all chats have been started.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Project id, path, or name. Required unless device is given." },
                    "device": { "type": "string", "description": "Host device (id or name) for a project-less chat; defaults to this device." },
                    "parent": { "type": "string", "description": "Parent chat to record (id, prefix, or title). Defaults to the chat you are speaking from." },
                    "harness": { "type": "string", "description": "Harness id (see list_harnesses). Defaults to claude-code when available." },
                    "model": { "type": "string", "description": "Model id from list_models. Omit for the harness default." },
                    "reasoning": { "type": "string", "description": "Reasoning level the model supports (e.g. low, medium, high, max)." },
                    "sandbox": { "type": "string", "enum": ["read-only", "workspace-write", "danger-full-access"], "default": "workspace-write" },
                    "title": { "type": "string", "description": "Sidebar title. Otherwise the engine titles it from the first exchange." },
                    "branch": { "type": "string", "description": "Branch label to record on the chat." },
                    "cwd": { "type": "string", "description": "Working directory override (an existing worktree path). Defaults to the project folder." },
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
            "Create multiple independent side chats concurrently. Put each chat's prompt in its request to start all work together. Prefer this for parallel delegation, including harnesses that execute tool calls sequentially. Each request has create_chat arguments; wait defaults to false. Results preserve request order and include per-request errors; successful requests are not rolled back.",
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
}

#[derive(Deserialize, Default)]
struct ListChatsArgs {
    project: Option<String>,
    device: Option<String>,
    #[serde(default)]
    include_archived: bool,
    parent: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize, Default)]
struct CreateChatArgs {
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
    prompt: Option<String>,
    #[serde(default)]
    wait: bool,
    timeout_secs: Option<u64>,
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
        "parentChatId": chat.parent_chat_id,
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
        Self { zeron }
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
            "list_harnesses" => self.list_harnesses().await,
            "list_models" => self.list_models(parse(args)?).await,
            "list_chats" => self.list_chats(parse(args)?).await,
            "get_chat" => self.get_chat(parse(args)?).await,
            "create_chat" => self.create_chat(parse(args)?).await,
            "create_chats" => self.batch(parse(args)?, true).await,
            "send_messages" => self.batch(parse(args)?, false).await,
            "read_chat" => self.read_chat(parse(args)?).await,
            "send_message" => self.send_message(parse(args)?).await,
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
                        Ok(args) => self.create_chat(args).await,
                        Err(error) => Err(error.into()),
                    }
                } else {
                    match serde_json::from_value(args) {
                        Ok(args) => self.send_message(args).await,
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
        let chat = match origin.chat_id.as_deref() {
            Some(id) => match self.zeron.resolve_chat(id).await {
                Ok(chat) => {
                    let (spaces, sessions) =
                        tokio::try_join!(self.zeron.spaces(), self.zeron.sessions())?;
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
            "note": if origin.chat_id.is_some() {
                "Messages you send are attributed to this chat; it cannot message itself."
            } else {
                "Not running inside a chat: messages are sent without attribution."
            },
        }))
    }

    async fn list_devices(&self) -> anyhow::Result<Value> {
        let (devices, local) =
            tokio::try_join!(self.zeron.devices(), self.zeron.local_device_id())?;
        Ok(json!({
            "devices": devices.iter().map(|d| json!({
                "id": d.id,
                "name": d.name,
                "platform": d.platform,
                "local": d.id == local,
                "lastSeenAt": d.last_seen_at,
                "version": d.version,
            })).collect::<Vec<_>>()
        }))
    }

    async fn list_projects(&self) -> anyhow::Result<Value> {
        let (spaces, devices) = tokio::try_join!(self.zeron.spaces(), self.zeron.devices())?;
        let device_name = |id: &str| devices.iter().find(|d| d.id == id).map(|d| d.name.clone());
        Ok(json!({
            "projects": spaces.iter().map(|s| json!({
                "id": s.id,
                "name": s.display_name(),
                "path": s.path,
                "deviceId": s.device_id,
                "deviceName": device_name(&s.device_id),
                "git": s.git_detected,
            })).collect::<Vec<_>>()
        }))
    }

    async fn list_harnesses(&self) -> anyhow::Result<Value> {
        let harnesses = self.zeron.harnesses().await?;
        Ok(json!({
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
        let models = self.zeron.models(harness).await?;
        Ok(json!({
            "harness": harness,
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
        if let Some(origin) = self.zeron.origin().chat_id.as_deref() {
            let chat = self.zeron.resolve_chat(origin).await?;
            anyhow::ensure!(
                chat.parent_chat_id.is_none(),
                "Side chats cannot create chats. Ask your parent chat to create another side chat."
            );
        }
        let harnesses = self.zeron.harnesses().await?;
        let harness = match args.harness.as_deref() {
            Some(raw) => {
                let id: HarnessId = parse_enum("harness", raw).map_err(anyhow::Error::msg)?;
                if let Some(info) = harnesses.iter().find(|h| h.id == id)
                    && !info.available()
                {
                    anyhow::bail!(
                        "harness {raw} is not available on this device (see list_harnesses)"
                    );
                }
                id
            }
            None => default_harness(&harnesses)?,
        };
        if let Some(model) = args.model.as_deref()
            && let Ok(models) = self.zeron.models(harness).await
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
        let config = ChatConfig {
            harness,
            model: args.model.clone(),
            reasoning,
            model_options: Default::default(),
            sandbox,
        };

        let (space, device_id) = match args.project.as_deref() {
            Some(project) => {
                let space = self.zeron.resolve_space(project).await?;
                let device_id = space.device_id.clone();
                (Some(space), device_id)
            }
            None => (
                None,
                self.zeron.resolve_device_id(args.device.as_deref()).await?,
            ),
        };

        // Parent: the explicit `parent` argument, else the chat this server
        // speaks for. Resolved so a prefix/title works and a typo fails loud.
        let parent_chat_id = match args
            .parent
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            Some(key) => Some(self.zeron.resolve_chat(key).await?.id),
            None => self.zeron.origin().chat_id.clone(),
        };

        if let Some(parent) = parent_chat_id.as_deref() {
            let chat = self.zeron.resolve_chat(parent).await?;
            anyhow::ensure!(
                chat.parent_chat_id.is_none(),
                "Cannot create a child of a side chat. Choose a top-level parent chat."
            );
        }
        let chat_id = uuid::Uuid::new_v4().to_string();
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
        if let Some(branch) = args
            .branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
        {
            mutate["branch"] = json!(branch);
        }
        if let Some(cwd) = args.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            mutate["cwd"] = json!(cwd);
        }
        self.zeron.mutate(mutate).await?;
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

        let mut result = json!({
            "chatId": chat_id,
            "deviceId": device_id,
            "project": space.as_ref().map(|s| json!({ "id": s.id, "name": s.display_name(), "path": s.path })),
            "harness": harness,
            "model": args.model,
            "reasoning": reasoning,
            "title": args.title,
            "parentChatId": parent_chat_id,
        });
        if let Some(prompt) = args.prompt.filter(|p| !p.trim().is_empty()) {
            // The row may not have folded into WatchChats yet; build the
            // chat locally from what we just wrote rather than re-reading.
            let chat = Chat {
                id: chat_id.clone(),
                device_id: device_id.clone(),
                title: args.title.clone(),
                archived: false,
                cwd: args.cwd.clone(),
                branch: args.branch.clone(),
                checkout_id: None,
                source_context: None,
                config: Some(config),
                last_message_preview: None,
                last_message_at: None,
                created_at: chrono::Utc::now(),
                harness_session_id: None,
                harness_session_cwd: None,
                parent_chat_id: parent_chat_id.clone(),
                space_id: space.as_ref().map(|s| s.id.clone()),
                last_seen_at: None,
                room_gen: None,
            };
            let sent = self
                .deliver(&chat, space.as_ref(), &harnesses, None, prompt, "run")
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
            .deliver(&chat, space, &harnesses, baseline.as_ref(), body, mode)
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

    /// Pick and perform the delivery the composer would.
    async fn deliver(
        &self,
        chat: &Chat,
        space: Option<&Space>,
        harnesses: &[HarnessInfo],
        session: Option<&Session>,
        text: String,
        mode: &str,
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
                    worktree: None,
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
        let entries = self.zeron.transcript(&chat.id).await.unwrap_or_default();
        let rendered = render_entries(&entries, RenderOptions::default());
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
        dispatch_barrier: Option<tokio::sync::Barrier>,
        beta_parent: Option<String>,
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
                methods::WATCH_DEVICES => stream(json!([{
                    "id": "dev-local", "name": "Laptop", "platform": "linux",
                    "lastSeenAt": null
                }])),
                methods::WATCH_SPACES => stream(json!([{
                    "id": "space-1", "deviceId": "dev-local", "path": "/repo/comet",
                    "gitDetected": true, "createdAt": "2026-09-01T00:00:00Z"
                }])),
                methods::WATCH_CHATS => stream(json!([
                    {
                        "id": "chat-alpha-1", "deviceId": "dev-local", "title": "Alpha",
                        "archived": false, "spaceId": "space-1",
                        "config": { "harness": "claude-code", "model": "opus", "reasoning": null, "sandbox": "workspace-write" },
                        "createdAt": "2026-09-01T00:00:00Z"
                    },
                    {
                        "id": "chat-beta-2", "deviceId": "dev-local", "title": "Beta",
                        "parentChatId": self.beta_parent,
                        "archived": false, "spaceId": "space-1",
                        "createdAt": "2026-09-02T00:00:00Z"
                    }
                ])),
                methods::WATCH_SESSIONS => stream(json!([])),
                methods::LIST_HARNESSES => RpcReply::Value(json!([
                    { "id": "claude-code", "name": "Claude Code", "supportsSteering": true,
                      "steeringMode": "step-boundary", "reasoningLevels": [], "installed": true, "enabled": true },
                    { "id": "codex", "name": "Codex", "supportsSteering": true,
                      "steeringMode": "turn-boundary", "reasoningLevels": [], "installed": false, "enabled": true }
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
