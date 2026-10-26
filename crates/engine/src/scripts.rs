use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

use futures::StreamExt;
use futures::stream::BoxStream;
use mlua::serde::SerializeOptions;
use mlua::{Function, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value, VmState};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot, watch};
use zeron_doc::SessionCommandPayload;
use zeron_proto::scripts::{
    CallScriptToolParams, NoticeLevel, ScriptCommand, ScriptInfo, ScriptNotice, ScriptTool,
    ScriptsFrame, ScriptsSnapshot, SetScriptEnabledParams,
};
use zeron_proto::{
    AgentEvent, Chat, DoneStatus, ReasoningLevel, RunRequest, SandboxLevel, Session, SessionStatus,
};

use crate::doc_host::DocHost;
use crate::fs_watch::{FsWatch, FsWatchHub};
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const MAX_SCRIPT_BYTES: usize = 256 * 1024;
const CALL_BUDGET: Duration = Duration::from_millis(250);
const TOOL_BUDGET: Duration = Duration::from_secs(1);
const BEFORE_RUN_TIMEOUT: Duration = Duration::from_secs(2);
const RESCAN_QUIET: Duration = Duration::from_millis(150);
const MAX_TIMER_SECONDS: f64 = 60.0 * 60.0 * 24.0;
const STATE_DIR: &str = ".state";
const SETTINGS_FILE: &str = "settings.json";

const EVENTS: &[&str] = &[
    "before_run",
    "run_start",
    "tool_call",
    "tool_result",
    "turn_done",
    "error",
    "input_requested",
    "unload",
];

#[derive(Clone)]
pub struct ScriptHost {
    shared: Arc<Shared>,
}

struct Shared {
    folder: PathBuf,
    tx: mpsc::Sender<Msg>,
    listening: AtomicBool,
    snapshot: watch::Sender<ScriptsSnapshot>,
    notices: broadcast::Sender<ScriptNotice>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

#[derive(Clone)]
pub struct ScriptLinks {
    chats: watch::Receiver<Vec<Chat>>,
    sessions: watch::Receiver<Vec<Session>>,
    doc_host: DocHost,
    runtime: Option<tokio::runtime::Handle>,
}

impl ScriptLinks {
    pub fn new(workspace: &WorkspaceHost, sessions: &SessionsEngine, doc_host: DocHost) -> Self {
        Self {
            chats: workspace.watch_chats(),
            sessions: sessions.watch_sessions(),
            doc_host,
            runtime: tokio::runtime::Handle::try_current().ok(),
        }
    }

    fn status_of(&self, chat_id: &str) -> SessionStatus {
        self.sessions
            .borrow()
            .iter()
            .find(|session| session.chat_id == chat_id)
            .map(|session| session.status)
            .unwrap_or(SessionStatus::Idle)
    }

    fn chats(&self) -> Vec<ChatSummary> {
        self.chats
            .borrow()
            .iter()
            .filter(|chat| !chat.archived)
            .map(|chat| ChatSummary {
                id: chat.id.clone(),
                title: chat.title.clone(),
                cwd: chat.cwd.clone(),
                status: self.status_of(&chat.id),
            })
            .collect()
    }

    fn send(&self, chat: &str, text: String) -> Result<(), String> {
        let chat = self
            .chats
            .borrow()
            .iter()
            .find(|row| row.id == chat || row.title.as_deref() == Some(chat))
            .cloned()
            .ok_or_else(|| format!("no chat named {chat}"))?;
        let message_id = uuid::Uuid::new_v4().to_string();
        let payload = match self.status_of(&chat.id) {
            SessionStatus::Working => SessionCommandPayload::Steer {
                prompt: text,
                message_id: Some(message_id),
            },
            SessionStatus::AwaitingInput => {
                return Err(format!("chat {} is waiting for an answer", chat.id));
            }
            SessionStatus::Idle | SessionStatus::Errored => {
                let config = chat.config.as_ref();
                SessionCommandPayload::Run {
                    request: RunRequest {
                        prompt: text,
                        harness: config.map(|c| c.harness),
                        model: config.and_then(|c| c.model.clone()),
                        reasoning: config.and_then(|c| c.reasoning),
                        model_options: config.map(|c| c.model_options.clone()).unwrap_or_default(),
                        cwd: chat.cwd.clone().unwrap_or_else(|| "~".into()),
                        sandbox: config
                            .map(|c| c.sandbox)
                            .unwrap_or(SandboxLevel::WorkspaceWrite),
                        auto_approve: false,
                        resume: None,
                        attachments: Vec::new(),
                        worktree: None,
                        mcp: None,
                        env: Default::default(),
                    },
                    message_id,
                }
            }
        };
        let _guard = self.runtime.as_ref().map(|runtime| runtime.enter());
        self.doc_host
            .queue_command(&chat.id, payload)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChatSummary {
    id: String,
    title: Option<String>,
    cwd: Option<String>,
    status: SessionStatus,
}

enum Msg {
    Rescan,
    Event {
        chat_id: String,
        event: AgentEvent,
    },
    BeforeRun {
        chat_id: String,
        request: RunRequest,
        reply: oneshot::Sender<RunRequest>,
    },
    RunCommand {
        script: String,
        command: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    ListTools {
        reply: oneshot::Sender<Vec<ScriptTool>>,
    },
    SetEnabled {
        params: SetScriptEnabledParams,
        reply: oneshot::Sender<Result<(), String>>,
    },
    CallTool {
        params: CallScriptToolParams,
        reply: oneshot::Sender<Result<serde_json::Value, String>>,
    },
    Shutdown,
}

impl ScriptHost {
    pub fn start(folder: PathBuf, links: ScriptLinks) -> Self {
        let (tx, rx) = mpsc::channel();
        let (snapshot, _) = watch::channel(ScriptsSnapshot {
            folder: folder.to_string_lossy().into_owned(),
            scripts: Vec::new(),
        });
        let (notices, _) = broadcast::channel(64);
        let shared = Arc::new(Shared {
            folder: folder.clone(),
            tx: tx.clone(),
            listening: AtomicBool::new(false),
            snapshot,
            notices,
        });
        let weak = Arc::downgrade(&shared);
        let spawned = std::thread::Builder::new()
            .name("zeron-scripts".into())
            .spawn(move || Runner::new(folder, links, weak, tx).run(rx));
        if let Err(err) = spawned {
            tracing::error!(error = %err, "script host thread failed to start");
        }
        Self { shared }
    }

    pub fn folder(&self) -> &Path {
        &self.shared.folder
    }

    pub fn observe(&self, chat_id: &str, event: &AgentEvent) {
        if !self.shared.listening.load(Ordering::Relaxed) {
            return;
        }
        let interesting = matches!(
            event,
            AgentEvent::SessionStarted { .. }
                | AgentEvent::ToolCall { .. }
                | AgentEvent::ToolResult { .. }
                | AgentEvent::Error { .. }
                | AgentEvent::InputRequested { .. }
                | AgentEvent::Done { .. }
        );
        if interesting {
            let _ = self.shared.tx.send(Msg::Event {
                chat_id: chat_id.to_owned(),
                event: event.clone(),
            });
        }
    }

    pub async fn before_run(&self, chat_id: &str, request: RunRequest) -> RunRequest {
        if !self.shared.listening.load(Ordering::Relaxed) {
            return request;
        }
        let (reply, rx) = oneshot::channel();
        let sent = self.shared.tx.send(Msg::BeforeRun {
            chat_id: chat_id.to_owned(),
            request: request.clone(),
            reply,
        });
        if sent.is_err() {
            return request;
        }
        match tokio::time::timeout(BEFORE_RUN_TIMEOUT, rx).await {
            Ok(Ok(changed)) => changed,
            _ => request,
        }
    }

    pub fn reload(&self) {
        let _ = self.shared.tx.send(Msg::Rescan);
    }

    pub async fn set_enabled(&self, params: SetScriptEnabledParams) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.shared
            .tx
            .send(Msg::SetEnabled { params, reply })
            .map_err(|_| "scripts are not running".to_string())?;
        rx.await
            .unwrap_or_else(|_| Err("scripts are not running".into()))
    }

    pub async fn run_command(&self, script: &str, command: &str) -> Result<(), String> {
        let (reply, rx) = oneshot::channel();
        self.shared
            .tx
            .send(Msg::RunCommand {
                script: script.to_owned(),
                command: command.to_owned(),
                reply,
            })
            .map_err(|_| "scripts are not running".to_string())?;
        rx.await
            .unwrap_or_else(|_| Err("scripts are not running".into()))
    }

    pub async fn list_tools(&self) -> Vec<ScriptTool> {
        let (reply, rx) = oneshot::channel();
        if self.shared.tx.send(Msg::ListTools { reply }).is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    pub async fn call_tool(
        &self,
        params: CallScriptToolParams,
    ) -> Result<serde_json::Value, String> {
        let (reply, rx) = oneshot::channel();
        self.shared
            .tx
            .send(Msg::CallTool { params, reply })
            .map_err(|_| "scripts are not running".to_string())?;
        rx.await
            .unwrap_or_else(|_| Err("scripts are not running".into()))
    }

    pub fn frames(&self) -> BoxStream<'static, serde_json::Value> {
        let snapshots = futures::stream::unfold(
            (self.shared.snapshot.subscribe(), false),
            |(mut rx, emitted)| async move {
                if emitted {
                    rx.changed().await.ok()?;
                }
                let frame = ScriptsFrame::Scripts(rx.borrow_and_update().clone());
                Some((frame, (rx, true)))
            },
        );
        let notices =
            futures::stream::unfold(self.shared.notices.subscribe(), |mut rx| async move {
                loop {
                    match rx.recv().await {
                        Ok(notice) => return Some((ScriptsFrame::Notice(notice), rx)),
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            });
        futures::stream::select(snapshots, notices)
            .filter_map(|frame| async move { serde_json::to_value(&frame).ok() })
            .boxed()
    }
}

struct Timer {
    id: u64,
    at: Instant,
    every: Option<Duration>,
    run: Function,
}

struct Tool {
    def: ScriptTool,
    run: Function,
}

struct Command {
    id: String,
    label: String,
    run: Function,
}

#[derive(Default)]
struct Registry {
    handlers: HashMap<String, Vec<Function>>,
    commands: Vec<Command>,
    tools: Vec<Tool>,
    timers: Vec<Timer>,
    next_timer: u64,
    changed: bool,
}

struct Store {
    path: PathBuf,
    data: serde_json::Map<String, serde_json::Value>,
    dirty: bool,
}

impl Store {
    fn open(path: PathBuf) -> Self {
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            data,
            dirty: false,
        }
    }

    fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let write = self
            .path
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| serde_json::to_vec_pretty(&self.data).map_err(std::io::Error::other))
            .and_then(|bytes| std::fs::write(&self.path, bytes));
        if let Err(err) = write {
            tracing::warn!(path = %self.path.display(), error = %err, "script store write failed");
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }
}

struct Script {
    name: String,
    path: PathBuf,
    stamp: Stamp,
    lua: Lua,
    registry: Rc<RefCell<Registry>>,
    store: Rc<RefCell<Store>>,
    deadline: Rc<Cell<Option<Instant>>>,
    notices: broadcast::Sender<ScriptNotice>,
    error: Option<String>,
    last_failure: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    disabled: BTreeSet<String>,
}

impl Settings {
    fn path(folder: &Path) -> PathBuf {
        folder.join(STATE_DIR).join(SETTINGS_FILE)
    }

    fn open(folder: &Path) -> Self {
        std::fs::read(Self::path(folder))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, folder: &Path) {
        let path = Self::path(folder);
        let write = path
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| serde_json::to_vec_pretty(self).map_err(std::io::Error::other))
            .and_then(|bytes| std::fs::write(&path, bytes));
        if let Err(err) = write {
            tracing::warn!(path = %path.display(), error = %err, "script settings write failed");
        }
    }
}

struct Runner {
    folder: PathBuf,
    links: ScriptLinks,
    shared: std::sync::Weak<Shared>,
    notices: broadcast::Sender<ScriptNotice>,
    tx: mpsc::Sender<Msg>,
    files: BTreeMap<String, PathBuf>,
    scripts: BTreeMap<String, Script>,
    settings: Settings,
    run_starts: HashMap<String, (Instant, String)>,
    rescan_at: Option<Instant>,
    watch: Option<FsWatch>,
}

impl Runner {
    fn new(
        folder: PathBuf,
        links: ScriptLinks,
        shared: std::sync::Weak<Shared>,
        tx: mpsc::Sender<Msg>,
    ) -> Self {
        let notices = shared
            .upgrade()
            .map(|shared| shared.notices.clone())
            .unwrap_or_else(|| broadcast::channel(1).0);
        Self {
            settings: Settings::open(&folder),
            folder,
            links,
            shared,
            notices,
            tx,
            files: BTreeMap::new(),
            scripts: BTreeMap::new(),
            run_starts: HashMap::new(),
            rescan_at: None,
            watch: None,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        if let Err(err) = std::fs::create_dir_all(&self.folder) {
            tracing::warn!(folder = %self.folder.display(), error = %err, "scripts folder unavailable");
        }
        let watch_tx = self.tx.clone();
        self.watch = Some(FsWatchHub::global().watch(
            &self.folder,
            notify::RecursiveMode::NonRecursive,
            move |_| {
                let _ = watch_tx.send(Msg::Rescan);
            },
        ));
        self.rescan();
        loop {
            let wait = self
                .next_wake()
                .map(|at| at.saturating_duration_since(Instant::now()));
            let msg = match wait {
                Some(wait) => rx.recv_timeout(wait),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(msg) => self.handle(msg),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
        }
        for script in self.scripts.values_mut() {
            script.unload();
        }
    }

    fn next_wake(&self) -> Option<Instant> {
        self.scripts
            .values()
            .flat_map(|script| {
                script
                    .registry
                    .borrow()
                    .timers
                    .iter()
                    .map(|timer| timer.at)
                    .collect::<Vec<_>>()
            })
            .chain(self.rescan_at)
            .min()
    }

    fn tick(&mut self) {
        let now = Instant::now();
        if self.rescan_at.is_some_and(|at| at <= now) {
            self.rescan_at = None;
            self.rescan();
        }
        let mut publish = false;
        for script in self.scripts.values_mut() {
            script.fire_timers(now);
            script.store.borrow_mut().flush();
            let mut registry = script.registry.borrow_mut();
            if registry.changed {
                registry.changed = false;
                publish = true;
            }
        }
        if publish {
            self.publish();
        }
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Rescan => self.rescan_at = Some(Instant::now() + RESCAN_QUIET),
            Msg::Event { chat_id, event } => self.on_event(chat_id, event),
            Msg::BeforeRun {
                chat_id,
                request,
                reply,
            } => {
                let _ = reply.send(self.before_run(chat_id, request));
            }
            Msg::RunCommand {
                script,
                command,
                reply,
            } => {
                let result = match self.scripts.get_mut(&script) {
                    Some(script) => script.run_command(&command),
                    None => Err(format!("no script named {script}")),
                };
                let _ = reply.send(result);
            }
            Msg::ListTools { reply } => {
                let tools = self
                    .scripts
                    .values()
                    .flat_map(|script| {
                        script
                            .registry
                            .borrow()
                            .tools
                            .iter()
                            .map(|tool| tool.def.clone())
                            .collect::<Vec<_>>()
                    })
                    .collect();
                let _ = reply.send(tools);
            }
            Msg::SetEnabled { params, reply } => {
                let result = if self.files.contains_key(&params.script) {
                    if params.enabled {
                        self.settings.disabled.remove(&params.script);
                    } else {
                        self.settings.disabled.insert(params.script.clone());
                    }
                    self.settings.save(&self.folder);
                    self.rescan();
                    Ok(())
                } else {
                    Err(format!("no script named {}", params.script))
                };
                let _ = reply.send(result);
            }
            Msg::CallTool { params, reply } => {
                let owner = self.scripts.values_mut().find(|script| {
                    script
                        .registry
                        .borrow()
                        .tools
                        .iter()
                        .any(|tool| tool.def.name == params.name)
                });
                let result = match owner {
                    Some(script) => script.call_tool(&params),
                    None => Err(format!("unknown tool: {}", params.name)),
                };
                let _ = reply.send(result);
            }
            Msg::Shutdown => {}
        }
    }

    fn rescan(&mut self) {
        let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(&self.folder) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = script_name(&path) {
                    seen.insert(name, path);
                }
            }
        }
        let removed: Vec<String> = self
            .scripts
            .keys()
            .filter(|name| !seen.contains_key(*name) || self.settings.disabled.contains(*name))
            .cloned()
            .collect();
        for name in removed {
            if let Some(mut script) = self.scripts.remove(&name) {
                script.unload();
                tracing::info!(script = %name, "script unloaded");
            }
        }
        self.files = seen.clone();
        for (name, path) in seen {
            if self.settings.disabled.contains(&name) {
                continue;
            }
            let stamp = Stamp::of(&path);
            if stamp.is_some() && self.scripts.get(&name).map(|script| script.stamp) == stamp {
                continue;
            }
            if let Some(mut old) = self.scripts.remove(&name) {
                old.unload();
            }
            let Some(stamp) = stamp else {
                continue;
            };
            let script = Script::load(
                name.clone(),
                path,
                stamp,
                &self.folder,
                &self.links,
                self.notices.clone(),
            );
            match &script.error {
                None => tracing::info!(script = %name, "script loaded"),
                Some(error) => {
                    tracing::warn!(script = %name, error = %error, "script failed to load");
                    let _ = self.notices.send(ScriptNotice {
                        script: name.clone(),
                        text: error.clone(),
                        level: NoticeLevel::Error,
                    });
                }
            }
            self.scripts.insert(name, script);
        }
        self.publish();
    }

    fn publish(&self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let listening = self
            .scripts
            .values()
            .any(|script| script.error.is_none() && !script.registry.borrow().handlers.is_empty());
        shared.listening.store(listening, Ordering::Relaxed);
        let scripts = self
            .files
            .iter()
            .map(|(name, path)| {
                let mut info = ScriptInfo {
                    name: name.clone(),
                    path: path.to_string_lossy().into_owned(),
                    enabled: !self.settings.disabled.contains(name),
                    error: None,
                    commands: Vec::new(),
                    tools: Vec::new(),
                };
                if let Some(script) = self.scripts.get(name) {
                    let registry = script.registry.borrow();
                    info.error = script.error.clone();
                    info.commands = registry
                        .commands
                        .iter()
                        .map(|command| ScriptCommand {
                            id: command.id.clone(),
                            label: command.label.clone(),
                        })
                        .collect();
                    info.tools = registry
                        .tools
                        .iter()
                        .map(|tool| tool.def.name.clone())
                        .collect();
                }
                info
            })
            .collect();
        shared.snapshot.send_replace(ScriptsSnapshot {
            folder: self.folder.to_string_lossy().into_owned(),
            scripts,
        });
    }

    fn on_event(&mut self, chat_id: String, event: AgentEvent) {
        let (name, payload) = match event {
            AgentEvent::SessionStarted {
                harness,
                model,
                cwd,
                ..
            } => {
                let prompt = self
                    .run_starts
                    .entry(chat_id.clone())
                    .or_insert_with(|| (Instant::now(), String::new()))
                    .1
                    .clone();
                (
                    "run_start",
                    serde_json::json!({
                        "chat": chat_id,
                        "harness": harness,
                        "model": model,
                        "cwd": cwd,
                        "prompt": prompt,
                    }),
                )
            }
            AgentEvent::ToolCall { id, call } => {
                let mut payload = serde_json::to_value(call).unwrap_or_default();
                if let Some(fields) = payload.as_object_mut() {
                    fields.insert("chat".into(), chat_id.into());
                    fields.insert("id".into(), id.into());
                }
                ("tool_call", payload)
            }
            AgentEvent::ToolResult {
                id,
                is_error,
                output,
                ..
            } => (
                "tool_result",
                serde_json::json!({
                    "chat": chat_id,
                    "id": id,
                    "error": is_error,
                    "output": output,
                }),
            ),
            AgentEvent::Error { message } => (
                "error",
                serde_json::json!({ "chat": chat_id, "message": message }),
            ),
            AgentEvent::InputRequested {
                request_id,
                questions,
            } => (
                "input_requested",
                serde_json::json!({
                    "chat": chat_id,
                    "id": request_id,
                    "questions": questions,
                }),
            ),
            AgentEvent::Done { status, error, .. } => {
                let (started, prompt) = self
                    .run_starts
                    .remove(&chat_id)
                    .unwrap_or_else(|| (Instant::now(), String::new()));
                let status = match status {
                    DoneStatus::Completed => "completed",
                    DoneStatus::Interrupted => "interrupted",
                    DoneStatus::Errored => "errored",
                };
                (
                    "turn_done",
                    serde_json::json!({
                        "chat": chat_id,
                        "status": status,
                        "error": error,
                        "prompt": prompt,
                        "seconds": started.elapsed().as_secs_f64(),
                    }),
                )
            }
            _ => return,
        };
        for script in self.scripts.values_mut() {
            script.emit(name, &payload);
        }
    }

    fn before_run(&mut self, chat_id: String, mut request: RunRequest) -> RunRequest {
        let mut run = serde_json::json!({
            "chat": chat_id,
            "prompt": request.prompt,
            "model": request.model,
            "reasoning": request.reasoning,
            "cwd": request.cwd,
            "harness": request.harness,
            "sandbox": request.sandbox,
        });
        for script in self.scripts.values_mut() {
            if let Some(changed) = script.mutate("before_run", &run) {
                run = changed;
            }
        }
        if let Some(prompt) = run.get("prompt").and_then(|v| v.as_str()) {
            request.prompt = prompt.to_owned();
        }
        match run.get("model") {
            Some(serde_json::Value::String(model)) => request.model = Some(model.clone()),
            Some(serde_json::Value::Null) | None => request.model = None,
            _ => {}
        }
        match run.get("reasoning") {
            Some(serde_json::Value::Null) | None => request.reasoning = None,
            Some(value) => match serde_json::from_value::<ReasoningLevel>(value.clone()) {
                Ok(level) => request.reasoning = Some(level),
                Err(_) => tracing::warn!(value = %value, "script set an unknown reasoning level"),
            },
        }
        self.run_starts
            .insert(chat_id, (Instant::now(), request.prompt.clone()));
        request
    }
}

fn script_name(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    if !matches!(extension, "lua" | "luau") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.is_empty() || stem.starts_with('.') || stem.starts_with('_') {
        return None;
    }
    Some(stem.to_owned())
}

fn command_id(label: &str) -> String {
    let mut id = String::with_capacity(label.len());
    for ch in label.chars() {
        if ch.is_alphanumeric() {
            id.extend(ch.to_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.trim_end_matches('-').to_owned()
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn describe(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { cause, .. } => describe(cause),
        mlua::Error::RuntimeError(message) | mlua::Error::SyntaxError { message, .. } => {
            message.clone()
        }
        mlua::Error::MemoryError(_) => {
            format!(
                "the script used more than {} MB",
                MEMORY_LIMIT / (1024 * 1024)
            )
        }
        mlua::Error::WithContext { context, cause } => {
            format!("{context}: {}", describe(cause))
        }
        other => other.to_string(),
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Nil => "nil".into(),
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.to_string_lossy(),
        other => format!("<{}>", other.type_name()),
    }
}

fn to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    lua.to_value_with(
        value,
        SerializeOptions::new()
            .serialize_none_to_null(false)
            .serialize_unit_to_null(false),
    )
}

fn tool_schema(lua: &Lua, input: Option<Table>) -> mlua::Result<serde_json::Value> {
    let empty = || serde_json::json!({ "type": "object", "properties": {} });
    let Some(input) = input else {
        return Ok(empty());
    };
    let mut raw: serde_json::Value = lua.from_value(Value::Table(input))?;
    if raw.get("type").and_then(|t| t.as_str()) == Some("object") {
        return Ok(raw);
    }
    let Some(fields) = raw.as_object_mut() else {
        return Ok(empty());
    };
    let mut required = Vec::new();
    let mut properties = serde_json::Map::new();
    for (name, schema) in std::mem::take(fields) {
        let mut schema = match schema {
            serde_json::Value::String(kind) => serde_json::json!({ "type": kind }),
            other => other,
        };
        let optional = schema
            .as_object_mut()
            .and_then(|s| s.remove("optional"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !optional {
            required.push(name.clone());
        }
        properties.insert(name, schema);
    }
    Ok(serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
    }))
}

impl Script {
    fn load(
        name: String,
        path: PathBuf,
        stamp: Stamp,
        folder: &Path,
        links: &ScriptLinks,
        notices: broadcast::Sender<ScriptNotice>,
    ) -> Self {
        let store = Store::open(folder.join(STATE_DIR).join(format!("{name}.json")));
        let lua = Lua::new_with(
            StdLib::TABLE
                | StdLib::STRING
                | StdLib::MATH
                | StdLib::UTF8
                | StdLib::BIT
                | StdLib::OS
                | StdLib::COROUTINE,
            LuaOptions::default(),
        )
        .unwrap_or_else(|_| Lua::new());
        let mut script = Self {
            name,
            path,
            stamp,
            lua,
            registry: Rc::default(),
            store: Rc::new(RefCell::new(store)),
            deadline: Rc::default(),
            notices,
            error: None,
            last_failure: None,
        };
        if let Err(err) = script.install(links) {
            script.error = Some(describe(&err));
        }
        script
    }

    fn install(&mut self, links: &ScriptLinks) -> mlua::Result<()> {
        let source = std::fs::read_to_string(&self.path).map_err(mlua::Error::external)?;
        if source.len() > MAX_SCRIPT_BYTES {
            return Err(mlua::Error::runtime(format!(
                "the script is over {} KB",
                MAX_SCRIPT_BYTES / 1024
            )));
        }
        let lua = &self.lua;
        lua.set_memory_limit(MEMORY_LIMIT)?;
        let watched = self.deadline.clone();
        let ticks = Cell::new(0u32);
        lua.set_interrupt(move |_| {
            let tick = ticks.get().wrapping_add(1);
            ticks.set(tick);
            if tick.is_multiple_of(256)
                && let Some(deadline) = watched.get()
                && Instant::now() > deadline
            {
                return Err(mlua::Error::runtime(
                    "the script ran past its time budget (an endless loop, or too much work in one handler)",
                ));
            }
            Ok(VmState::Continue)
        });

        let globals = lua.globals();
        let script_name = self.name.clone();
        globals.set(
            "print",
            lua.create_function(move |_, args: mlua::Variadic<Value>| {
                let line: Vec<String> = args.iter().map(value_text).collect();
                tracing::info!(target: "zeron_scripts", script = %script_name, "{}", line.join("\t"));
                Ok(())
            })?,
        )?;

        let zeron = lua.create_table()?;
        zeron.set("version", env!("CARGO_PKG_VERSION"))?;
        let script = lua.create_table()?;
        script.set("name", self.name.as_str())?;
        script.set("path", self.path.to_string_lossy().as_ref())?;
        zeron.set("script", script)?;

        let registry = self.registry.clone();
        zeron.set(
            "on",
            lua.create_function(move |_, (event, handler): (String, Function)| {
                if !EVENTS.contains(&event.as_str()) {
                    return Err(mlua::Error::runtime(format!(
                        "unknown event {event:?}; one of {}",
                        EVENTS.join(", ")
                    )));
                }
                let mut registry = registry.borrow_mut();
                registry.handlers.entry(event).or_default().push(handler);
                registry.changed = true;
                Ok(())
            })?,
        )?;

        let registry = self.registry.clone();
        zeron.set(
            "command",
            lua.create_function(move |_, (label, run): (String, Function)| {
                let label = label.trim().to_owned();
                if label.is_empty() {
                    return Err(mlua::Error::runtime("a command needs a label"));
                }
                let mut registry = registry.borrow_mut();
                let base = command_id(&label);
                let mut id = base.clone();
                let mut n = 1;
                while registry.commands.iter().any(|command| command.id == id) {
                    n += 1;
                    id = format!("{base}-{n}");
                }
                registry.commands.push(Command { id, label, run });
                registry.changed = true;
                Ok(())
            })?,
        )?;

        let registry = self.registry.clone();
        zeron.set(
            "tool",
            lua.create_function(move |lua, spec: Table| {
                let name: String = spec.get("name")?;
                if !valid_tool_name(&name) {
                    return Err(mlua::Error::runtime(format!(
                        "tool name {name:?} must be lowercase letters, digits and underscores"
                    )));
                }
                let description: String = spec.get("description")?;
                let run: Function = spec.get("run")?;
                let input_schema = tool_schema(lua, spec.get("input")?)?;
                let mut registry = registry.borrow_mut();
                registry.tools.retain(|tool| tool.def.name != name);
                registry.tools.push(Tool {
                    def: ScriptTool {
                        name,
                        description,
                        input_schema,
                    },
                    run,
                });
                registry.changed = true;
                Ok(())
            })?,
        )?;

        for (key, repeat) in [("after", false), ("every", true)] {
            let registry = self.registry.clone();
            zeron.set(
                key,
                lua.create_function(move |lua, (seconds, run): (f64, Function)| {
                    if !(seconds >= 0.0 && seconds <= MAX_TIMER_SECONDS) {
                        return Err(mlua::Error::runtime(format!(
                            "{key} wants seconds between 0 and {MAX_TIMER_SECONDS}"
                        )));
                    }
                    if repeat && seconds < 0.05 {
                        return Err(mlua::Error::runtime("every needs at least 0.05 seconds"));
                    }
                    let period = Duration::from_secs_f64(seconds);
                    let id = {
                        let mut registry = registry.borrow_mut();
                        registry.next_timer += 1;
                        let id = registry.next_timer;
                        registry.timers.push(Timer {
                            id,
                            at: Instant::now() + period,
                            every: repeat.then_some(period),
                            run,
                        });
                        id
                    };
                    let registry = registry.clone();
                    lua.create_function(move |_, ()| {
                        registry.borrow_mut().timers.retain(|timer| timer.id != id);
                        Ok(())
                    })
                })?,
            )?;
        }

        let notices = self.notices.clone();
        let script_name = self.name.clone();
        zeron.set(
            "notify",
            lua.create_function(move |_, (text, level): (String, Option<String>)| {
                let level = match level.as_deref() {
                    None => NoticeLevel::Info,
                    Some(level) => NoticeLevel::parse(level).ok_or_else(|| {
                        mlua::Error::runtime(format!(
                            "unknown notice level {level:?}; one of info, success, error"
                        ))
                    })?,
                };
                let _ = notices.send(ScriptNotice {
                    script: script_name.clone(),
                    text,
                    level,
                });
                Ok(())
            })?,
        )?;

        let store_table = lua.create_table()?;
        let store = self.store.clone();
        store_table.set(
            "get",
            lua.create_function(move |lua, (key, default): (String, Value)| {
                match store.borrow().data.get(&key) {
                    Some(value) => to_lua(lua, value),
                    None => Ok(default),
                }
            })?,
        )?;
        let store = self.store.clone();
        store_table.set(
            "set",
            lua.create_function(move |lua, (key, value): (String, Value)| {
                let mut store = store.borrow_mut();
                if value.is_nil() {
                    store.data.remove(&key);
                } else {
                    let json: serde_json::Value = lua.from_value(value)?;
                    store.data.insert(key, json);
                }
                store.dirty = true;
                Ok(())
            })?,
        )?;
        zeron.set("store", store_table)?;

        let chat_links = links.clone();
        zeron.set(
            "chats",
            lua.create_function(move |lua, ()| lua.to_value(&chat_links.chats()))?,
        )?;
        let send_links = links.clone();
        zeron.set(
            "send",
            lua.create_function(move |_, (chat, text): (String, String)| {
                send_links.send(&chat, text).map_err(mlua::Error::runtime)
            })?,
        )?;

        globals.set("zeron", zeron)?;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| i64::from(d.subsec_nanos()))
            .unwrap_or(1);
        let randomseed: Function = globals.get::<Table>("math")?.get("randomseed")?;
        randomseed.call::<()>(seed)?;
        lua.sandbox(true)?;

        let chunk_name = format!("={}", self.name);
        self.budgeted(CALL_BUDGET, || {
            lua.load(&source).set_name(chunk_name).exec()
        })
    }

    fn budgeted<R>(
        &self,
        budget: Duration,
        f: impl FnOnce() -> mlua::Result<R>,
    ) -> mlua::Result<R> {
        self.deadline.set(Some(Instant::now() + budget));
        let result = f();
        self.deadline.set(None);
        result
    }

    fn handlers(&self, event: &str) -> Vec<Function> {
        if self.error.is_some() {
            return Vec::new();
        }
        self.registry
            .borrow()
            .handlers
            .get(event)
            .cloned()
            .unwrap_or_default()
    }

    fn fail(&mut self, what: &str, error: &mlua::Error) {
        let text = format!("{what} failed: {}", describe(error));
        tracing::warn!(script = %self.name, "{text}");
        if self.last_failure.as_deref() == Some(text.as_str()) {
            return;
        }
        self.last_failure = Some(text.clone());
        let _ = self.notices.send(ScriptNotice {
            script: self.name.clone(),
            text,
            level: NoticeLevel::Error,
        });
    }

    fn emit(&mut self, event: &str, payload: &serde_json::Value) {
        for handler in self.handlers(event) {
            let call = self.budgeted(CALL_BUDGET, || {
                handler.call::<()>(to_lua(&self.lua, payload)?)
            });
            if let Err(err) = call {
                self.fail(event, &err);
            }
        }
    }

    fn mutate(&mut self, event: &str, payload: &serde_json::Value) -> Option<serde_json::Value> {
        let handlers = self.handlers(event);
        if handlers.is_empty() {
            return None;
        }
        let Ok(Value::Table(table)) = to_lua(&self.lua, payload) else {
            return None;
        };
        for handler in handlers {
            let call = self.budgeted(CALL_BUDGET, || handler.call::<()>(table.clone()));
            if let Err(err) = call {
                self.fail(event, &err);
            }
        }
        self.lua.from_value(Value::Table(table)).ok()
    }

    fn run_command(&mut self, id: &str) -> Result<(), String> {
        let run = self
            .registry
            .borrow()
            .commands
            .iter()
            .find(|command| command.id == id)
            .map(|command| command.run.clone())
            .ok_or_else(|| format!("{} has no command {id}", self.name))?;
        self.budgeted(CALL_BUDGET, || run.call::<()>(()))
            .map_err(|err| {
                self.fail(id, &err);
                describe(&err)
            })
    }

    fn call_tool(&mut self, params: &CallScriptToolParams) -> Result<serde_json::Value, String> {
        let run = self
            .registry
            .borrow()
            .tools
            .iter()
            .find(|tool| tool.def.name == params.name)
            .map(|tool| tool.run.clone())
            .ok_or_else(|| format!("unknown tool: {}", params.name))?;
        let result = self.budgeted(TOOL_BUDGET, || {
            let args = match to_lua(&self.lua, &params.arguments)? {
                Value::Table(table) => table,
                _ => self.lua.create_table()?,
            };
            if let Some(chat) = &params.chat_id {
                args.set("chat", chat.as_str())?;
            }
            Ok(match run.call::<Value>(args)? {
                Value::Nil => serde_json::Value::String("done".into()),
                other => self.lua.from_value(other)?,
            })
        });
        result.map_err(|err| {
            self.fail(&params.name, &err);
            describe(&err)
        })
    }

    fn fire_timers(&mut self, now: Instant) {
        if self.error.is_some() {
            return;
        }
        let due: Vec<(u64, Function)> = {
            let mut registry = self.registry.borrow_mut();
            let mut due = Vec::new();
            registry.timers.retain_mut(|timer| {
                if timer.at > now {
                    return true;
                }
                due.push((timer.id, timer.run.clone()));
                match timer.every {
                    Some(period) => {
                        timer.at = now + period;
                        true
                    }
                    None => false,
                }
            });
            due
        };
        for (id, run) in due {
            if let Err(err) = self.budgeted(CALL_BUDGET, || run.call::<()>(())) {
                self.fail("timer", &err);
                self.registry
                    .borrow_mut()
                    .timers
                    .retain(|timer| timer.id != id);
            }
        }
    }

    fn unload(&mut self) {
        for handler in self.handlers("unload") {
            if let Err(err) = self.budgeted(CALL_BUDGET, || handler.call::<()>(())) {
                self.fail("unload", &err);
            }
        }
        self.store.borrow_mut().flush();
        self.registry.borrow_mut().handlers.clear();
    }
}
