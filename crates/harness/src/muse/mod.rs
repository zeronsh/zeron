//! Muse Code harness: spawns the installed `muse` CLI as an MSP
//! session host (`muse serve`) and speaks its JSON-RPC 2.0 wire directly.
//!
//! - The host is persistent per Zeron session: `session/start` (or
//!   `session/resume`) once, then one `turn/start` per prompt; steers ride
//!   `turn/steer` mid-turn and become the next `turn/start` between turns.
//! - Transcript items stream as `item/delta` / `item/completed`
//!   notifications and are normalized into [`AgentEvent`]s (see
//!   [`normalize`]). `turn/completed` is the turn's authoritative end.
//! - Approvals and questions arrive as server→client requests
//!   (`approval/request`, `userInput/request`): the presentation receipt is
//!   answered immediately, then the decision travels as `approval/decide` /
//!   `userInput/answer`. Unattended sessions (auto-approve, or Zeron's
//!   danger sandbox) start with `approvalMode: allowAll` so no approval
//!   opens at all.
//! - Interrupt sends `turn/interrupt`, then escalates to process teardown
//!   after a grace period. Command ids must be UUIDv7 — the host rejects
//!   any other version.

mod catalog;
mod normalize;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SlashCommand, SteeringMode, UserInputAnswer, UserInputQuestion,
};

use crate::jsonrpc::{Incoming, RpcClient};
use crate::process::{Command, Stdio, owned::Child};
use crate::{Harness, HarnessError, RunControls};
use normalize::Normalizer;

fn resolve_muse_executable() -> Option<PathBuf> {
    let mut extra = Vec::new();
    if let Some(home) = crate::executable::home_dir() {
        extra.push(home.join(".local").join("bin").join("muse"));
    }
    extra.push(PathBuf::from("/opt/homebrew/bin/muse"));
    extra.push(PathBuf::from("/usr/local/bin/muse"));
    crate::executable::find_on_paths("muse", extra)
}

fn command_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// The MSP approval mode a run starts with: Zeron's unattended modes never
/// want a mid-turn prompt blocking the durable command queue.
fn approval_mode(request: &RunRequest) -> &'static str {
    if request.auto_approve || request.sandbox == SandboxLevel::DangerFullAccess {
        "allowAll"
    } else {
        "onRequest"
    }
}

/// Turn input parts for one prompt: a leading `/skill` invocation the host
/// advertises becomes a native `skill` part; everything else is text, with
/// image attachments inlined as base64 parts.
fn input_parts(prompt: &str, attachments: &[String], skills: &[String]) -> Vec<Value> {
    let mut parts = Vec::new();
    let mut text = prompt.to_owned();
    if let Some((name, args)) = zeron_proto::invocation::leading_command(prompt)
        && skills.iter().any(|s| s == name)
    {
        parts.push(json!({"type": "skill", "selector": name, "arguments": args}));
        text = String::new();
    }
    if !text.is_empty() {
        parts.push(json!({"type": "text", "text": text}));
    }
    for path in attachments {
        if let Some(part) = image_part(path) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        parts.push(json!({"type": "text", "text": prompt}));
    }
    parts
}

fn image_part(path: &str) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    let media = match Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    use base64::Engine;
    Some(json!({
        "type": "image",
        "mediaType": media,
        "base64Data": base64::engine::general_purpose::STANDARD.encode(bytes),
    }))
}

/// The `config.mcpServers` payload for Zeron's injected MCP server.
fn mcp_config(request: &RunRequest) -> Option<Value> {
    let mcp = request.mcp.as_ref()?;
    Some(json!({
        "mcpServers": {
            &mcp.name: {
                "transport": "stdio",
                "command": mcp.command,
                "args": mcp.args,
                "env": mcp.env,
            }
        }
    }))
}

pub struct MuseHarness {
    executable: Option<PathBuf>,
    interrupt_grace: Duration,
    kill_grace: Duration,
    models_cache: crate::catalog::Catalog,
}

impl Default for MuseHarness {
    fn default() -> Self {
        Self {
            executable: None,
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(3),
            models_cache: crate::catalog::Catalog::default(),
        }
    }
}

impl MuseHarness {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    pub fn with_graces(mut self, interrupt_grace: Duration, kill_grace: Duration) -> Self {
        self.interrupt_grace = interrupt_grace;
        self.kill_grace = kill_grace;
        self
    }

    fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(p) = &self.executable {
            return crate::executable::validate_native_override(p);
        }
        for var in ["MUSE_EXECUTABLE", "MUSE_CODE_EXECUTABLE"] {
            if let Some(p) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                return crate::executable::validate_native_override(&PathBuf::from(p));
            }
        }
        resolve_muse_executable().ok_or_else(|| {
            HarnessError::NotInstalled(
                "muse (searched PATH, the login shell's PATH, ~/.local/bin, \
                 /opt/homebrew/bin, /usr/local/bin; set MUSE_EXECUTABLE to override)"
                    .into(),
            )
        })
    }

    /// Spawn `muse serve` with the sandbox posture the first request fixes
    /// for the host's lifetime (the wire cannot renegotiate it).
    fn spawn_host(&self, request: &RunRequest) -> Result<Host, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut cmd = Command::new(&exe);
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, &exe);
        cmd.arg("serve");
        if request.sandbox == SandboxLevel::DangerFullAccess {
            cmd.arg("--disable-sandbox");
        }
        cmd.current_dir(if request.cwd.is_empty() {
            Path::new(".")
        } else {
            Path::new(&request.cwd)
        })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        let mut child = Child::new(cmd.spawn()?);
        let tail = crate::StderrTail::default();
        let stderr = tail.clone();
        let mut lines = BufReader::new(child.stderr.take().expect("piped stderr")).lines();
        let stderr_task = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                stderr.push(&line);
            }
            stderr.close();
        });
        let (client, incoming) = RpcClient::new(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
        );
        Ok(Host {
            child,
            client,
            incoming,
            tail,
            stderr_task,
        })
    }

    async fn handshake(host: &Host) -> Result<(), HarnessError> {
        host.client
            .request(
                "initialize",
                json!({
                    "clientInfo": {"name": "zeron", "version": env!("CARGO_PKG_VERSION")},
                    // `sessionMcp` gates the `config.mcpServers` member of
                    // session/start; without the grant the host rejects the
                    // start outright (-32010 capabilityRequired).
                    "capabilities": {"requestedCapabilities": ["sessionMcp"]},
                }),
            )
            .await?;
        host.client.notify("initialized", None);
        Ok(())
    }

    /// One short-lived host for catalog/skills probes.
    async fn probe<T>(
        &self,
        cwd: &Path,
        call: impl AsyncFnOnce(&RpcClient, &str) -> Result<T, HarnessError>,
    ) -> Result<T, HarnessError> {
        let request = RunRequest {
            prompt: String::new(),
            harness: Some(HarnessId::Muse),
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: cwd.to_string_lossy().into_owned(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
            mcp: None,
        };
        let mut host = self.spawn_host(&request)?;
        let result = async {
            Self::handshake(&host).await?;
            let started = host
                .client
                .request(
                    "session/start",
                    json!({
                        "commandId": command_id(),
                        "workspaceRoot": cwd.to_string_lossy(),
                        "approvalMode": "onRequest",
                    }),
                )
                .await?;
            let session_id = started
                .pointer("/session/sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    HarnessError::Protocol("muse session/start gave no sessionId".into())
                })?
                .to_owned();
            call(&host.client, &session_id).await
        }
        .await;
        host.shutdown(self.kill_grace).await;
        result
    }
}

struct Host {
    child: Child,
    client: RpcClient,
    incoming: mpsc::Receiver<Incoming>,
    tail: crate::StderrTail,
    stderr_task: tokio::task::JoinHandle<()>,
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stderr_task.abort();
    }
}

impl Host {
    async fn shutdown(&mut self, grace: Duration) {
        self.stderr_task.abort();
        self.child.shutdown(grace).await;
    }
}

#[async_trait]
impl Harness for MuseHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Muse
    }
    fn display_name(&self) -> &str {
        "Muse"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::XHigh,
            ReasoningLevel::Max,
            ReasoningLevel::Ultra,
        ]
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    /// `turn/completed` is the host's own durable terminal record.
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && live.model == next.model
            && live.auto_approve == next.auto_approve
            && live.sandbox == next.sandbox
    }
    /// Model changes ride `session/setModel` on the live host.
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && live.auto_approve == next.auto_approve
            && live.sandbox == next.sandbox
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        crate::model_context::context(self.id(), &self.resolve_executable()?, &[]).map(Some)
    }
    fn fallback_models(&self) -> Vec<Model> {
        catalog::fallback_models()
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(35),
                || self.model_context().map(|c| c.unwrap().key()),
                || async {
                    let cwd = std::env::current_dir()?;
                    // `model/list` needs no session; the probe host still
                    // requires the handshake before any method.
                    let mut host = self.spawn_host(&RunRequest {
                        prompt: String::new(),
                        harness: Some(HarnessId::Muse),
                        model: None,
                        reasoning: None,
                        model_options: serde_json::Map::new(),
                        cwd: cwd.to_string_lossy().into_owned(),
                        sandbox: SandboxLevel::WorkspaceWrite,
                        auto_approve: false,
                        resume: None,
                        attachments: Vec::new(),
                        worktree: None,
                        mcp: None,
                    })?;
                    let result = async {
                        Self::handshake(&host).await?;
                        host.client.request("model/list", json!({})).await
                    }
                    .await;
                    host.shutdown(self.kill_grace).await;
                    Ok(catalog::parse_models(&result?))
                },
            )
            .await
    }

    async fn commands(&self) -> Result<Vec<SlashCommand>, HarnessError> {
        self.commands_for(&std::env::current_dir()?).await
    }

    /// Muse's slash surface is its skill catalog (`skill/list`).
    async fn commands_for(&self, cwd: &Path) -> Result<Vec<SlashCommand>, HarnessError> {
        let skills = self.probe_skills(cwd).await?;
        Ok(skills
            .into_iter()
            .map(|(selector, display, description, hint)| SlashCommand {
                name: selector,
                description: if description.is_empty() {
                    display
                } else {
                    description
                },
                input_hint: hint,
            })
            .collect())
    }

    async fn skills(
        &self,
        cwd: &Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        let skills = self.probe_skills(cwd).await?;
        Ok(Some(
            skills
                .into_iter()
                .map(
                    |(selector, _display, description, _hint)| zeron_proto::invocation::Skill {
                        name: selector.clone(),
                        path: format!("harness-skill:muse:{selector}"),
                        description,
                        enabled: true,
                        command: Some(zeron_proto::invocation::SkillCommand {
                            name: selector,
                            harness: HarnessId::Muse,
                        }),
                    },
                )
                .collect(),
        ))
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let mut host = self.spawn_host(&request)?;
        let (tx, rx) = mpsc::channel(256);
        let kill_grace = self.kill_grace;
        tokio::spawn(async move {
            let RunControls {
                realtime: _,
                execution_lease: _lease,
                request_input,
                steering,
                interrupt,
                turn: _,
            } = controls;
            let mut runner = Runner {
                host: &mut host,
                tx: tx.clone(),
                request_input: Arc::from(request_input),
                norm: Normalizer::default(),
                session_id: String::new(),
                turn_id: None,
                done_pending: true,
                skill_names: Vec::new(),
                assistant: uuid::Uuid::new_v4().to_string(),
                done_sent: false,
            };
            let consumer = tx.clone();
            let result = tokio::select! {
                result = runner.run(request, steering) => result,
                _ = interrupt.cancelled() => {
                    runner.interrupt().await;
                    Ok(())
                }
                _ = consumer.closed() => Ok(()),
            };
            if let Err(error) = result {
                let status = runner.host.child.try_wait().ok().flatten();
                let message = if status.is_some() {
                    crate::crash_message("Muse", status, &runner.host.tail)
                } else {
                    error.to_string()
                };
                if runner.done_pending {
                    let _ = tx
                        .send(Ok(AgentEvent::Done {
                            status: DoneStatus::Errored,
                            result: None,
                            error: Some(message),
                            session_id: (!runner.session_id.is_empty())
                                .then(|| runner.session_id.clone()),
                        }))
                        .await;
                }
            }
            host.shutdown(kill_grace).await;
        });
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
                .boxed(),
        )
    }
}

impl MuseHarness {
    /// `(selector, displayName, description, argumentHint)` rows.
    async fn probe_skills(
        &self,
        cwd: &Path,
    ) -> Result<Vec<(String, String, String, Option<String>)>, HarnessError> {
        self.probe(cwd, async |client: &RpcClient, session_id: &str| {
            let result = client
                .request("skill/list", json!({"sessionId": session_id}))
                .await?;
            Ok(result
                .get("skills")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| {
                            let selector = row.get("selector").and_then(Value::as_str)?.to_owned();
                            Some((
                                selector,
                                row.get("displayName")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                row.get("description")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                row.get("argumentHint")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default())
        })
        .await
    }
}

struct Runner<'a> {
    host: &'a mut Host,
    tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    request_input: Arc<
        dyn Fn(Vec<UserInputQuestion>) -> tokio::sync::oneshot::Receiver<Vec<UserInputAnswer>>
            + Send
            + Sync,
    >,
    norm: Normalizer,
    session_id: String,
    turn_id: Option<String>,
    /// A turn (including the first, still in setup) owes the engine a `Done`.
    done_pending: bool,
    skill_names: Vec<String>,
    assistant: String,
    done_sent: bool,
}

impl Runner<'_> {
    async fn emit(&self, event: AgentEvent) -> Result<(), HarnessError> {
        self.tx
            .send(Ok(event))
            .await
            .map_err(|_| HarnessError::Protocol("Muse event consumer closed".into()))
    }

    async fn run(
        &mut self,
        request: RunRequest,
        mut steering: mpsc::Receiver<crate::SteerMessage>,
    ) -> Result<(), HarnessError> {
        MuseHarness::handshake(self.host).await?;
        let mut lost_context: Option<String> = None;
        let session = if let Some(resume) = &request.resume {
            match self
                .host
                .client
                .request(
                    "session/resume",
                    json!({
                        "commandId": command_id(),
                        "sessionId": resume,
                        "excludeItems": true,
                        "config": mcp_config(&request),
                    }),
                )
                .await
            {
                Ok(value) => Some(value),
                Err(error) => {
                    lost_context = Some(format!(
                        "Muse could not restore session {resume}; starting a new session without the previous context: {error}"
                    ));
                    None
                }
            }
        } else {
            None
        };
        let mut mcp_dropped = false;
        let session = match session {
            Some(value) => value,
            None => {
                let mut params = json!({
                    "commandId": command_id(),
                    "workspaceRoot": request.cwd,
                    "approvalMode": approval_mode(&request),
                });
                if let Some(model) = request.model.as_deref().filter(|m| *m != "default") {
                    params["modelId"] = model.into();
                }
                let has_mcp = mcp_config(&request).is_some();
                if let Some(config) = mcp_config(&request) {
                    params["config"] = config;
                }
                match self
                    .host
                    .client
                    .request("session/start", params.clone())
                    .await
                {
                    Ok(value) => value,
                    // A host that still refuses the MCP config (older build,
                    // capability not granted) must not kill the chat: start
                    // without Zeron's tools and say so in the transcript.
                    Err(error) if has_mcp => {
                        tracing::warn!(%error, "muse session/start refused the MCP config; retrying without it");
                        params.as_object_mut().map(|o| o.remove("config"));
                        params["commandId"] = command_id().into();
                        mcp_dropped = true;
                        self.host.client.request("session/start", params).await?
                    }
                    Err(error) => return Err(error),
                }
            }
        };
        self.session_id = session
            .pointer("/session/sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| HarnessError::Protocol("muse session gave no sessionId".into()))?
            .to_owned();
        if let Ok(result) = self
            .host
            .client
            .request("skill/list", json!({"sessionId": self.session_id}))
            .await
        {
            self.skill_names = result
                .get("skills")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|r| {
                            r.get("selector").and_then(Value::as_str).map(str::to_owned)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let commands: Vec<SlashCommand> = result
                .get("skills")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .filter_map(|r| {
                            Some(SlashCommand {
                                name: r.get("selector").and_then(Value::as_str)?.to_owned(),
                                description: r
                                    .get("description")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                                input_hint: r
                                    .get("argumentHint")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if !commands.is_empty() {
                let _ = self.emit(AgentEvent::AvailableCommands { commands }).await;
            }
        }
        self.emit(AgentEvent::SessionStarted {
            harness: HarnessId::Muse,
            model: request.model.clone().unwrap_or_else(|| "default".into()),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: self.session_id.clone(),
            assistant_message_id: self.assistant.clone(),
        })
        .await?;
        if let Some(message) = lost_context {
            self.emit(AgentEvent::TextDelta {
                text: format!("_{message}_\n\n"),
            })
            .await?;
        }
        if mcp_dropped {
            self.emit(AgentEvent::TextDelta {
                text: "_The Muse host refused Zeron's MCP server config; this session runs without Zeron tools._\n\n".into(),
            })
            .await?;
        }
        self.start_turn(&request.prompt, &request.attachments, request.reasoning)
            .await?;

        loop {
            tokio::select! {
                incoming = self.host.incoming.recv() => {
                    match incoming {
                        Some(Incoming::Notification { method, params }) => {
                            let is_terminal = method == "turn/completed";
                            for event in self.norm.map(&method, &params) {
                                self.emit(event).await?;
                            }
                            if is_terminal {
                                self.turn_id = None;
                                self.done_pending = false;
                                self.done_sent = true;
                                self.emit(AgentEvent::Done {
                                    status: self.norm.status(),
                                    result: None,
                                    error: self.norm.error.clone(),
                                    session_id: Some(self.session_id.clone()),
                                }).await?;
                                self.done_sent = false;
                            }
                        }
                        Some(Incoming::Request { id, method, params }) => {
                            self.host.client.respond(&id, json!({}));
                            self.answer_server_request(method, params);
                        }
                        Some(Incoming::Eof) | None => {
                            return Err(HarnessError::Protocol("Muse host exited".into()));
                        }
                    }
                }
                steer = steering.recv() => {
                    let Some(steer) = steer else {
                        // The engine closed the mailbox: the session is over.
                        return Ok(());
                    };
                    self.steer(steer).await?;
                }
            }
        }
    }

    async fn start_turn(
        &mut self,
        prompt: &str,
        attachments: &[String],
        reasoning: Option<ReasoningLevel>,
    ) -> Result<(), HarnessError> {
        self.norm.reset_turn();
        self.assistant = uuid::Uuid::new_v4().to_string();
        let mut params = json!({
            "commandId": command_id(),
            "sessionId": self.session_id,
            "input": input_parts(prompt, attachments, &self.skill_names),
        });
        if let Some(level) = reasoning {
            params["reasoningEffort"] = catalog::effort_arg(level).into();
        }
        let result = self.host.client.request("turn/start", params).await?;
        self.turn_id = result
            .get("turnId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.done_pending = true;
        Ok(())
    }

    async fn steer(&mut self, steer: crate::SteerMessage) -> Result<(), HarnessError> {
        if let Some(model) = steer
            .config
            .as_ref()
            .and_then(|config| config.model.as_deref().filter(|m| *m != "default"))
        {
            let _ = self
                .host
                .client
                .request(
                    "session/setModel",
                    json!({
                        "commandId": command_id(),
                        "sessionId": self.session_id,
                        "model": {"modelId": model},
                    }),
                )
                .await;
        }
        let reasoning = steer.config.as_ref().and_then(|c| c.reasoning);
        match &self.turn_id {
            Some(turn_id) => {
                let mut params = json!({
                    "commandId": command_id(),
                    "sessionId": self.session_id,
                    "expectedTurnId": turn_id,
                    "input": input_parts(&steer.prompt, &steer.attachments, &self.skill_names),
                });
                if let Some(level) = reasoning {
                    params["reasoningEffort"] = catalog::effort_arg(level).into();
                }
                match self.host.client.request("turn/steer", params).await {
                    Ok(_) => {
                        self.emit(AgentEvent::Steered {
                            assistant_message_id: Some(self.assistant.clone()),
                            next_assistant_message_id: None,
                        })
                        .await
                    }
                    // The turn ended between the engine's steer and ours:
                    // the prompt becomes the next turn instead of vanishing.
                    Err(_) => {
                        self.turn_id = None;
                        self.start_turn(&steer.prompt, &steer.attachments, reasoning)
                            .await
                    }
                }
            }
            None => {
                self.start_turn(&steer.prompt, &steer.attachments, reasoning)
                    .await
            }
        }
    }

    async fn interrupt(&mut self) {
        if let Some(turn_id) = &self.turn_id {
            let _ = self
                .host
                .client
                .request(
                    "turn/interrupt",
                    json!({
                        "commandId": command_id(),
                        "sessionId": self.session_id,
                        "turnId": turn_id,
                    }),
                )
                .await;
        }
        self.done_sent = true;
        self.done_pending = false;
        let _ = self
            .emit(AgentEvent::Done {
                status: DoneStatus::Interrupted,
                result: None,
                error: None,
                session_id: (!self.session_id.is_empty()).then(|| self.session_id.clone()),
            })
            .await;
    }

    /// Answer a server→client request off the main loop (the receipt has
    /// already been sent): approvals become a yes/no question unless the
    /// session is unattended, questions round-trip through `request_input`.
    fn answer_server_request(&self, method: String, params: Value) {
        let client = self.host.client.clone();
        let request_input = self.request_input.clone();
        let session_id = self.session_id.clone();
        tokio::spawn(async move {
            match method.as_str() {
                "approval/request" => {
                    let approved = decide_approval(&request_input, &params).await;
                    if let (Some(choice), Some(approval_id)) = (
                        pick_choice(&params, approved),
                        params.get("approvalId").and_then(Value::as_str),
                    ) {
                        let _ = client
                            .request(
                                "approval/decide",
                                json!({
                                    "commandId": command_id(),
                                    "sessionId": session_id,
                                    "approvalId": approval_id,
                                    "requirementId": params.get("currentRequirementId").cloned().unwrap_or(Value::Null),
                                    "choiceId": choice,
                                }),
                            )
                            .await;
                    }
                }
                "userInput/request" => {
                    let questions: Vec<UserInputQuestion> = params
                        .get("questions")
                        .and_then(Value::as_array)
                        .map(|list| list.iter().map(msp_question).collect())
                        .unwrap_or_default();
                    if questions.is_empty() {
                        return;
                    }
                    let answers = (request_input)(questions.clone()).await.unwrap_or_default();
                    let wire: Vec<Value> = questions
                        .iter()
                        .map(|q| {
                            let answer = answers.iter().find(|a| a.question_id == q.id);
                            let labels: Vec<String> =
                                answer.map(|a| a.labels.clone()).unwrap_or_default();
                            if q.multi_select {
                                json!({"questionId": q.id, "selectedLabels": labels})
                            } else if let Some(label) = labels.first() {
                                if q.options.iter().any(|o| o == label) {
                                    json!({"questionId": q.id, "selectedLabel": label})
                                } else {
                                    json!({"questionId": q.id, "freeText": label})
                                }
                            } else {
                                json!({"questionId": q.id, "freeText": ""})
                            }
                        })
                        .collect();
                    if let Some(id) = params.get("userInputId").and_then(Value::as_str) {
                        let _ = client
                            .request(
                                "userInput/answer",
                                json!({
                                    "commandId": command_id(),
                                    "sessionId": session_id,
                                    "userInputId": id,
                                    "answers": wire,
                                }),
                            )
                            .await;
                    }
                }
                _ => {}
            }
        });
    }
}

fn msp_question(raw: &Value) -> UserInputQuestion {
    let options = raw
        .get("options")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|o| o.get("label").and_then(Value::as_str).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    UserInputQuestion {
        id: raw
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        header: raw
            .get("header")
            .and_then(Value::as_str)
            .unwrap_or("Question")
            .to_owned(),
        question: raw
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        options,
        multi_select: raw.pointer("/selection/mode").and_then(Value::as_str) == Some("multiple"),
        prefill: None,
        multiline: false,
    }
}

/// Unattended approvals never reach here (`allowAll` opens none), so a
/// request that does arrive is the user's to answer.
async fn decide_approval(
    request_input: &Arc<
        dyn Fn(Vec<UserInputQuestion>) -> tokio::sync::oneshot::Receiver<Vec<UserInputAnswer>>
            + Send
            + Sync,
    >,
    params: &Value,
) -> bool {
    let tool = params
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or("a tool");
    let question = UserInputQuestion {
        id: params
            .get("approvalId")
            .and_then(Value::as_str)
            .unwrap_or("approval")
            .to_owned(),
        header: "Muse approval".into(),
        question: format!("Allow {tool} to run?"),
        options: vec!["Yes".into(), "No".into()],
        multi_select: false,
        prefill: None,
        multiline: false,
    };
    let id = question.id.clone();
    match (request_input)(vec![question]).await {
        Ok(answers) => answers
            .iter()
            .find(|a| a.question_id == id)
            .is_some_and(|a| a.labels.iter().any(|l| l == "Yes")),
        Err(_) => false,
    }
}

fn pick_choice(params: &Value, approved: bool) -> Option<String> {
    let choices = params.get("availableChoices").and_then(Value::as_array)?;
    let wanted = if approved {
        ["approvedForSession", "approved", "approvedPolicyAmendment"]
    } else {
        ["denied", "deniedPolicyAmendment", "abort"]
    };
    for decision in wanted {
        if let Some(choice) = choices
            .iter()
            .find(|c| c.get("decision").and_then(Value::as_str) == Some(decision))
        {
            return choice
                .get("choiceId")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }
    // Fall back to any choice on the same side of the fence.
    choices
        .iter()
        .find(|c| {
            let d = c.get("decision").and_then(Value::as_str).unwrap_or("");
            approved == d.starts_with("approved")
        })
        .and_then(|c| c.get("choiceId").and_then(Value::as_str).map(str::to_owned))
}
