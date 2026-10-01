//! Native Pi JSONL RPC driver. See PROTOCOL.md for the legacy ACK barrier.
mod catalog;
mod mcp;
mod normalize;
mod rpc;
mod sessions;
mod ui;

use crate::{
    Harness, HarnessError, RunControls,
    process::{Command, Stdio, owned::Child},
};
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use normalize::{Normalizer, string};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    sync::mpsc,
};
use zeron_proto::{AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode};

pub struct PiHarness {
    models_cache: crate::catalog::Catalog,
    workspace_commands: crate::skills::CommandDiscovery,
    session_store: Option<PathBuf>,
    agent_dir: Option<PathBuf>,
    executable: Option<PathBuf>,
    interrupt_grace: Duration,
    kill_grace: Duration,
}
impl Default for PiHarness {
    fn default() -> Self {
        Self {
            executable: None,
            session_store: None,
            agent_dir: None,
            models_cache: Default::default(),
            workspace_commands: Default::default(),
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(3),
        }
    }
}
impl PiHarness {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }
    pub fn with_session_store(mut self, path: impl Into<PathBuf>) -> Self {
        self.session_store = Some(path.into());
        self
    }
    /// Pi's settings/credentials directory (`PI_CODING_AGENT_DIR`) for the
    /// child and for Zeron's own reads of it; defaults to the inherited one.
    pub fn with_agent_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.agent_dir = Some(path.into());
        self
    }
    pub fn with_graces(mut self, interrupt: Duration, kill: Duration) -> Self {
        self.interrupt_grace = interrupt;
        self.kill_grace = kill;
        self
    }
    pub fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(path) = self
            .executable
            .clone()
            .or_else(|| std::env::var_os("PI_EXECUTABLE").map(PathBuf::from))
        {
            return crate::executable::validate_native_override(&path);
        }
        let home = crate::executable::home_or_current_dir();
        crate::executable::find_on_paths(
            "pi",
            vec![
                home.join(".local/bin/pi"),
                home.join(".npm-global/bin/pi"),
                PathBuf::from("/opt/homebrew/bin/pi"),
                PathBuf::from("/usr/local/bin/pi"),
            ],
        )
        .ok_or_else(|| {
            HarnessError::NotInstalled(
                "Pi CLI: install Pi or set PI_EXECUTABLE (the pi-acp adapter is not used)".into(),
            )
        })
    }
    async fn probe(&self, cwd: &Path, models: bool) -> Result<Value, HarnessError> {
        let mut process = self.spawn(cwd, &["--no-session".into()], None, None)?;
        let (mut tx, rx) = tokio::sync::oneshot::channel();
        let grace = self.kill_grace;
        tokio::spawn(async move {
            let result = tokio::select! {
                result=tokio::time::timeout(Duration::from_secs(60),async {
                    if models {Ok(serde_json::to_value(catalog::models(&mut process).await?).unwrap())}
                    else {process.query(json!({"type":"get_commands"}),&mut vec![]).await}
                })=>result.unwrap_or_else(|_|Err(HarnessError::Protocol("Pi discovery timed out".into()))),
                _=tx.closed()=>Err(HarnessError::Protocol("Pi discovery cancelled".into())),
            };
            process.shutdown(grace).await;
            let _ = tx.send(result);
        });
        rx.await
            .map_err(|_| HarnessError::Protocol("Pi discovery task failed".into()))?
    }
    fn spawn(
        &self,
        cwd: &Path,
        args: &[String],
        mcp: Option<&zeron_proto::McpServer>,
        policy: Option<&zeron_proto::AgentPolicy>,
    ) -> Result<Process, HarnessError> {
        let exe = self.resolve_executable()?;
        if self.executable.is_none() {
            if let Some(version) = crate::executable::binary_version(&exe) {
                if version < semver::Version::new(0, 85, 1) {
                    return Err(HarnessError::Protocol(format!(
                        "Pi {version} is unsupported; update Pi to 0.85.1 or newer"
                    )));
                }
            }
        }
        let mut cmd = match policy {
            Some(policy) => {
                crate::sandboxing::policy_command(HarnessId::Pi, policy, mcp, &exe, cwd)?
            }
            None => Command::new(&exe),
        };
        // Process group plus the same env scrubbing the ACP launch applied.
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, &exe);
        if let Some(dir) = &self.agent_dir {
            cmd.env("PI_CODING_AGENT_DIR", dir);
        }
        cmd.args(["--mode", "rpc", "--no-themes"])
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let scratch = mcp
            .map(|config| self::mcp::configure(&mut cmd, config))
            .transpose()?;
        let mut child = Child::new(cmd.spawn()?);
        let tail = crate::StderrTail::default();
        let mut lines = BufReader::new(child.stderr.take().expect("piped stderr")).lines();
        let stderr = tail.clone();
        let stderr_task = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                stderr.push(&line);
            }
            stderr.close();
        });
        let transport = rpc::Transport::new(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
        );
        Ok(Process {
            child,
            _scratch: scratch,
            transport,
            tail,
            stderr_task,
            dialogs: Default::default(),
            exit_deadline: None,
        })
    }
}
struct Process {
    child: Child,
    _scratch: Option<crate::scratch::ScratchDir>,
    transport: rpc::Transport,
    tail: crate::StderrTail,
    stderr_task: tokio::task::JoinHandle<()>,
    dialogs: ui::Dialogs,
    exit_deadline: Option<tokio::time::Instant>,
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stderr_task.abort();
    }
}
impl Process {
    async fn next(&mut self) -> Result<Value, HarnessError> {
        loop {
            tokio::select! {
                // Drain buffered output before reporting process death. A descendant
                // may retain stdout, so EOF alone cannot govern liveness.
                biased;
                _ = async {
                    match self.exit_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => return Err(HarnessError::Protocol("Pi exited".into())),
                status = self.child.wait(), if self.exit_deadline.is_none() => {
                    status?;
                    self.exit_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(100));
                }
                frame = self.transport.incoming.recv() => return frame.unwrap_or_else(|| Err(HarnessError::Protocol("Pi disconnected".into()))),
            }
        }
    }
    async fn query(
        &mut self,
        command: Value,
        backlog: &mut Vec<Value>,
    ) -> Result<Value, HarnessError> {
        let id = self.transport.client.request(command)?;
        loop {
            let frame = self.next().await?;
            if frame["type"] == "response" && frame["id"] == id {
                return response_data(frame);
            }
            if frame["type"] == "extension_ui_request" {
                self.dialogs.request(self.transport.client.clone(), &frame);
            } else {
                backlog.push(frame);
            }
        }
    }
    async fn shutdown(&mut self, grace: Duration) {
        self.dialogs.cancel();
        self.child.shutdown(grace).await;
    }
}
fn response_data(frame: Value) -> Result<Value, HarnessError> {
    if frame["success"] == true {
        Ok(frame["data"].clone())
    } else {
        Err(HarnessError::Protocol(format!(
            "Pi {}: {}",
            string(&frame, "command"),
            frame["error"].as_str().unwrap_or("command failed")
        )))
    }
}
#[async_trait]
impl Harness for PiHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Pi
    }
    fn display_name(&self) -> &str {
        "Pi"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    /// Bypass only: Pi's RPC has no permission protocol — its tools run
    /// without asking anyone — so no policy can answer for it. The stricter
    /// modes wait for an OS sandbox that confines the process.
    fn policy_caps(&self) -> zeron_proto::PolicyCaps {
        zeron_proto::PolicyCaps {
            sandboxes: crate::sandboxing::os_sandboxes(),
            ..zeron_proto::PolicyCaps::bypass_only()
        }
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        let root = sessions::agent_dir(self.agent_dir.clone());
        crate::model_context::context(
            HarnessId::Pi,
            &self.resolve_executable()?,
            &[root.join("models.json"), root.join("settings.json")],
        )
        .map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(65),
                || Ok(self.model_context()?.expect("Pi model context").key()),
                || async {
                    let value = self.probe(&std::env::current_dir()?, true).await?;
                    serde_json::from_value(value).map_err(|e| HarnessError::Protocol(e.to_string()))
                },
            )
            .await
    }
    async fn commands(&self) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
        self.commands_for(&std::env::current_dir()?).await
    }
    async fn commands_for(
        &self,
        cwd: &Path,
    ) -> Result<Vec<zeron_proto::SlashCommand>, HarnessError> {
        self.workspace_commands
            .get(cwd, async {
                Ok(catalog::commands(&self.probe(cwd, false).await?))
            })
            .await
    }
    async fn skills(
        &self,
        cwd: &Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        let mut skills = crate::skills::discover(HarnessId::Pi, cwd).await?;
        let commands = self.commands_for(cwd).await?;
        crate::skills::attach_advertised_commands(HarnessId::Pi, &mut skills, &commands);
        Ok(Some(skills))
    }
    fn fallback_models(&self) -> Vec<Model> {
        vec![Model {
            id: "default".into(),
            label: "Pi default".into(),
            description: Some("Model configured in Pi".into()),
            reasoning_levels: vec![],
            options: vec![],
        }]
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let mut request = request;
        let store = sessions::Store::new(self.session_store.clone(), self.agent_dir.clone());
        let mut lost_context = None;
        let args = match &request.resume {
            Some(id) => match store.resume_args(id, Path::new(&request.cwd)) {
                Ok(args) => args,
                // Like every other harness, a session that can no longer be
                // reopened starts fresh with a visible notice. Failing instead
                // would strand the chat: the engine resumes the same id forever.
                Err(error) => {
                    lost_context = Some(format!(
                        "Pi could not restore session {id}; starting a new session without the previous context: {error}"
                    ));
                    vec![]
                }
            },
            None => vec![],
        };
        if lost_context.is_some() {
            request.resume = None;
        }
        let process = self.spawn(
            Path::new(&request.cwd),
            &args,
            request.mcp.as_ref(),
            Some(&request.policy),
        )?;
        let (tx, rx) = mpsc::channel(256);
        let kill_grace = self.kill_grace;
        let interrupt_grace = self.interrupt_grace;
        tokio::spawn(async move {
            let mut runner = Runner {
                store,
                interrupted: false,
                extension_commands: HashSet::new(),
                auto_compaction: true,
                initial_pending: true,
                deliveries: VecDeque::new(),
                native_queue: Vec::new(),
                queued: VecDeque::new(),
                process,
                tx,
                request,
                norm: Normalizer::default(),
                active: true,
                epoch: 0,
                pending: HashMap::new(),
                session: String::new(),
                assistant: uuid::Uuid::new_v4().to_string(),
                lost_context,
            };
            // The lease lives through shutdown even if the consumer drops its stream.
            let RunControls {
                execution_lease: _lease,
                request_input,
                mut steering,
                interrupt,
            } = controls;
            runner.process.dialogs.input = Some(std::sync::Arc::from(request_input));
            let consumer = runner.tx.clone();
            let result = tokio::select! {
                result=runner.run(&mut steering)=>result,
                _=interrupt.cancelled()=>{ runner.interrupt(interrupt_grace).await; Ok(()) },
                _=consumer.closed()=>Ok(()),
            };
            if let Err(error) = result {
                let status = runner.process.child.try_wait().ok().flatten();
                runner.norm.error = Some(if status.is_some() {
                    crate::crash_message("Pi", status, &runner.process.tail)
                } else {
                    error.to_string()
                });
            }
            runner.process.shutdown(kill_grace).await;
            let _ = runner.finish().await;
        });
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
                .boxed(),
        )
    }
}
#[derive(Clone)]
enum Pending {
    Prompt(u64),
    Barrier(u64),
    Command(u64),
    EmptyEntries(u64, String),
    EmptyState(u64, String, bool),
}
struct Delivery {
    epoch: u64,
    // Queue acceptance permits sending the next steer, but is not consumption.
    queued: bool,
}
struct Runner {
    extension_commands: HashSet<String>,
    auto_compaction: bool,
    interrupted: bool,
    initial_pending: bool,
    deliveries: VecDeque<Delivery>,
    native_queue: Vec<String>,
    queued: VecDeque<crate::SteerMessage>,
    store: sessions::Store,
    process: Process,
    tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    request: RunRequest,
    norm: Normalizer,
    active: bool,
    epoch: u64,
    pending: HashMap<String, Pending>,
    session: String,
    assistant: String,
    lost_context: Option<String>,
}
impl Runner {
    async fn emit(&self, event: AgentEvent) -> Result<(), HarnessError> {
        self.tx
            .send(Ok(event))
            .await
            .map_err(|_| HarnessError::Protocol("Pi event consumer closed".into()))
    }
    async fn started(&self, model: String) -> Result<(), HarnessError> {
        self.emit(AgentEvent::SessionStarted {
            harness: HarnessId::Pi,
            model,
            tools: vec![],
            cwd: self.request.cwd.clone(),
            session_id: self.session.clone(),
            assistant_message_id: self.assistant.clone(),
        })
        .await
    }
    async fn finish(&mut self) -> Result<(), HarnessError> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        self.process.dialogs.cancel();
        self.emit(AgentEvent::Done {
            status: if self.interrupted {
                zeron_proto::DoneStatus::Interrupted
            } else {
                self.norm.status()
            },
            result: None,
            error: self.norm.error.clone(),
            session_id: (!self.session.is_empty()).then(|| self.session.clone()),
        })
        .await
    }
    fn control_command(&self, text: &str) -> Option<Value> {
        let name = text
            .trim()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_start_matches('/');
        if self.extension_commands.contains(name) {
            None
        } else {
            catalog::builtin(text, self.auto_compaction)
        }
    }
    fn submit(&mut self, text: String, images: Value, steer: bool) -> Result<(), HarnessError> {
        self.store.mark_submitted(&self.session)?;
        let control = self.control_command(&text);
        let command=control.clone().unwrap_or_else(||json!({"type":"prompt","message":text,"images":images,"streamingBehavior":if steer {"steer"}else{"followUp"}}));
        let id = self.process.transport.client.request(command)?;
        self.pending.insert(
            id,
            if control.is_some() {
                Pending::Command(self.epoch)
            } else {
                Pending::Prompt(self.epoch)
            },
        );
        Ok(())
    }
    fn prompt(&mut self, text: String, images: Value) -> Result<(), HarnessError> {
        self.epoch += 1;
        self.active = true;
        self.norm.reset();
        self.submit(text, images, false)
    }
    async fn bootstrap(&mut self, backlog: &mut Vec<Value>) -> Result<Value, HarnessError> {
        // Pi defaults to one-at-a-time. Zeron's pending steers belong together
        // at the next model step. Pi persists this in its global settings, so
        // select it only while no mode is configured: an explicit choice (the
        // user's, or `/steering` in a chat) is never overwritten.
        if !self
            .store
            .steering_mode_configured(Path::new(&self.request.cwd))
        {
            self.process
                .query(json!({"type":"set_steering_mode","mode":"all"}), backlog)
                .await?;
        }
        if let Some(model) = self.request.model.as_deref().filter(|s| *s != "default") {
            let models = self
                .process
                .query(json!({"type":"get_available_models"}), backlog)
                .await?;
            let options = models["models"]
                .as_array()
                .ok_or_else(|| HarnessError::Protocol("Pi returned no models".into()))?;
            let found = options
                .iter()
                .find(|m| format!("{}/{}", string(m, "provider"), string(m, "id")) == model)
                .or_else(|| {
                    let mut matches = options.iter().filter(|m| string(m, "id") == model);
                    let first = matches.next()?;
                    matches.next().is_none().then_some(first)
                })
                .ok_or_else(|| {
                    HarnessError::Protocol(format!("Pi model is not available: {model}"))
                })?;
            self.process
                .query(
                    json!({"type":"set_model","provider":found["provider"],"modelId":found["id"]}),
                    backlog,
                )
                .await?;
        }
        if self.request.reasoning.is_some()
            || self
                .request
                .model_options
                .get("pi_thinking")
                .is_some_and(|v| v == "off")
        {
            let supported = self
                .process
                .query(json!({"type":"get_available_thinking_levels"}), backlog)
                .await?;
            let levels = supported["levels"].as_array().cloned().unwrap_or_default();
            let desired = if self
                .request
                .model_options
                .get("pi_thinking")
                .is_some_and(|v| v == "off")
            {
                "off".to_string()
            } else {
                serde_json::to_value(self.request.reasoning)
                    .unwrap()
                    .as_str()
                    .unwrap_or("medium")
                    .to_owned()
            };
            let order = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
            let rank = order
                .iter()
                .position(|s| *s == desired)
                .unwrap_or(order.len() - 1);
            let effective = order[..=rank]
                .iter()
                .rev()
                .find(|s| levels.iter().any(|v| v == **s))
                .ok_or_else(|| {
                    HarnessError::Protocol("Pi returned no supported thinking level".into())
                })?;
            self.process
                .query(
                    json!({"type":"set_thinking_level","level":effective}),
                    backlog,
                )
                .await?;
        }
        let commands = self
            .process
            .query(json!({"type":"get_commands"}), backlog)
            .await?;
        self.extension_commands = commands["commands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c["source"] == "extension")
            .map(|c| string(c, "name").to_owned())
            .collect();
        self.emit(AgentEvent::AvailableCommands {
            commands: catalog::commands(&commands),
        })
        .await?;
        self.process
            .query(json!({"type":"get_state"}), backlog)
            .await
    }
    async fn run(
        &mut self,
        steering: &mut mpsc::Receiver<crate::SteerMessage>,
    ) -> Result<(), HarnessError> {
        let mut backlog = vec![];
        // Same budget the ACP handshake gave Pi: extensions can start cold.
        let state = tokio::time::timeout(Duration::from_secs(120), self.bootstrap(&mut backlog))
            .await
            .map_err(|_| HarnessError::Protocol("Pi startup timed out".into()))??;
        self.session = string(&state, "sessionId").into();
        if self.session.is_empty() {
            return Err(HarnessError::Protocol(
                "Pi get_state omitted sessionId".into(),
            ));
        }
        if self
            .request
            .resume
            .as_ref()
            .is_some_and(|id| id != &self.session)
        {
            return Err(HarnessError::Protocol(
                "Pi resumed a different session".into(),
            ));
        }
        if let Some(file) = state["sessionFile"].as_str() {
            self.store.remember(&self.session, Path::new(file))?;
        }
        self.auto_compaction = state["autoCompactionEnabled"].as_bool().unwrap_or(true);
        self.norm.window = state["model"]["contextWindow"].as_u64();
        self.started(state["model"]["id"].as_str().unwrap_or("default").into())
            .await?;
        if let Some(message) = self.lost_context.take() {
            self.emit(AgentEvent::Error { message }).await?;
        }
        for frame in backlog {
            self.frame(frame).await?;
        }
        let images = load_images(&self.request.attachments).await;
        self.prompt(self.request.prompt.clone(), images)?;
        let mut open = true;
        loop {
            if self.deliveries.iter().all(|delivery| delivery.queued)
                && !self.initial_pending
                && !self.pending.values().any(|p| {
                    matches!(
                        p,
                        Pending::Prompt(_)
                            | Pending::Barrier(_)
                            | Pending::Command(_)
                            | Pending::EmptyEntries(..)
                            | Pending::EmptyState(..)
                    )
                })
            {
                if !self.queued.front().is_some_and(|s| {
                    (self.active && self.control_command(&s.prompt).is_some())
                        || (!self.deliveries.is_empty()
                            && s.prompt.strip_prefix('/').is_some_and(|command| {
                                self.extension_commands
                                    .contains(command.split_whitespace().next().unwrap_or(""))
                            }))
                }) {
                    if let Some(steer) = self.queued.pop_front() {
                        if !self.active {
                            self.norm.reset();
                        }
                        self.epoch += 1;
                        self.active = true;
                        // Atomic Pi operation: queue at a step boundary if busy, start if idle.
                        // A separate get_state + steer pair would strand an input on the idle race.
                        self.submit(steer.prompt.clone(), json!([]), true)?;
                        self.deliveries.push_back(Delivery {
                            epoch: self.epoch,
                            queued: false,
                        });
                    }
                }
            }
            if !self.active && !open && self.queued.is_empty() {
                return Ok(());
            }
            tokio::select! {
                incoming=self.process.next()=>self.frame(incoming?).await?,
                steer=steering.recv(),if open=>match steer {Some(steer)=>self.queued.push_back(steer),None=>open=false}
            }
        }
    }
    async fn confirm_delivery(&mut self) -> Result<(), HarnessError> {
        if self.deliveries.pop_front().is_some() {
            let old = std::mem::replace(&mut self.assistant, uuid::Uuid::new_v4().to_string());
            self.emit(AgentEvent::Steered {
                assistant_message_id: Some(old),
                next_assistant_message_id: Some(self.assistant.clone()),
            })
            .await?;
        }
        Ok(())
    }
    async fn interrupt(&mut self, grace: Duration) {
        self.interrupted = true;
        self.process.dialogs.cancel();
        self.queued.clear();
        self.deliveries.clear();
        if !self.active {
            return;
        }
        let _ = self
            .process
            .transport
            .client
            .request(json!({"type":"clear_queue"}));
        let Ok(abort) = self
            .process
            .transport
            .client
            .request(json!({"type":"abort"}))
        else {
            return;
        };
        let _ = tokio::time::timeout(grace, async {
            while let Ok(frame) = self.process.next().await {
                if frame["id"] == abort && frame["success"] == true {
                    break;
                }
                if self.frame(frame).await.is_err() {
                    break;
                }
            }
        })
        .await;
    }
    async fn state(&mut self, data: Value, empty_proof: Option<bool>) -> Result<(), HarnessError> {
        let session = data["sessionId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| HarnessError::Protocol("Pi get_state omitted sessionId".into()))?;
        let changed = self.session != session;
        self.session = session.into();
        let file = data["sessionFile"].as_str().map(Path::new);
        if let Some(file) = file {
            self.store.remember(&self.session, file)?;
        }
        if changed {
            self.started(data["model"]["id"].as_str().unwrap_or("default").into())
                .await?;
        }
        self.auto_compaction = data["autoCompactionEnabled"]
            .as_bool()
            .unwrap_or(self.auto_compaction);
        self.norm.window = data["model"]["contextWindow"].as_u64().or(self.norm.window);
        if data["isStreaming"] != false
            || data["isCompacting"] != false
            || self.pending.values().any(|p| {
                matches!(
                    p,
                    Pending::Prompt(_)
                        | Pending::Command(_)
                        | Pending::EmptyEntries(..)
                        | Pending::EmptyState(..)
                )
            })
        {
            return Ok(());
        }
        if file.is_some_and(|p| {
            p.metadata()
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        }) && data["messageCount"] == 0
            && !self.interrupted
        {
            if let Some(clean) = empty_proof {
                if clean {
                    self.store
                        .remember_empty(&data, Path::new(&self.request.cwd))?;
                }
            } else {
                let id = self
                    .process
                    .transport
                    .client
                    .request(json!({"type":"get_entries"}))?;
                self.pending
                    .insert(id, Pending::EmptyEntries(self.epoch, self.session.clone()));
                return Ok(());
            }
        }
        self.initial_pending = false;
        if !self.interrupted {
            // Only an unqueued input may have been handled without a model run.
            // Never turn an accepted-but-unconsumed queued steer into a receipt.
            if self.deliveries.iter().any(|delivery| delivery.queued) {
                return Err(HarnessError::Protocol(
                    "Pi settled with unconsumed steering messages".into(),
                ));
            }
            while !self.deliveries.is_empty() {
                self.confirm_delivery().await?;
            }
        }
        self.finish().await
    }
    async fn frame(&mut self, frame: Value) -> Result<(), HarnessError> {
        if frame["type"] == "queue_update" {
            let queue: Vec<String> = frame["steering"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|text| text.as_str().map(str::to_owned))
                .collect();
            // Preflight requests stay serialized through their ordered state
            // barrier. An appended queue entry therefore belongs to the one
            // submission in flight, including Pi's input/template expansions.
            if queue.len() > self.native_queue.len()
                && queue.starts_with(&self.native_queue)
                && let Some(delivery) = self.deliveries.back_mut()
                && self.pending.values().any(
                    |pending| matches!(pending, Pending::Prompt(epoch) if *epoch == delivery.epoch),
                )
            {
                delivery.queued = true;
            }
            self.native_queue = queue;
            return Ok(());
        }
        if frame["type"] == "extension_ui_request" {
            if !self.active {
                // A background notification must not reopen a completed engine turn.
                // No foreground run can own a dialog here, so cancel it explicitly.
                ui::Dialogs::default().request(self.process.transport.client.clone(), &frame);
                tracing::debug!(method = %string(&frame, "method"), "Pi UI request outside a run");
                return Ok(());
            }
            self.process
                .dialogs
                .request(self.process.transport.client.clone(), &frame);
            if frame["method"] == "notify" {
                self.emit(AgentEvent::TextDelta {
                    text: format!("\n{}\n", string(&frame, "message")),
                })
                .await?;
            }
            return Ok(());
        }
        if frame["type"] == "response" {
            let Some(pending) = self.pending.remove(string(&frame, "id")) else {
                return Ok(());
            };
            let data = response_data(frame)?;
            if let Pending::Command(epoch) = pending {
                if epoch == self.epoch {
                    self.initial_pending = false;
                    self.confirm_delivery().await?;
                    let text = if data.is_null() {
                        "Pi command completed.".into()
                    } else {
                        serde_json::to_string_pretty(&data).unwrap()
                    };
                    self.emit(AgentEvent::TextDelta { text }).await?;
                    let id = self
                        .process
                        .transport
                        .client
                        .request(json!({"type":"get_state"}))?;
                    self.pending.insert(id, Pending::Barrier(epoch));
                }
                return Ok(());
            }
            match pending {
                Pending::Prompt(epoch) if epoch == self.epoch && self.active => {
                    let id = self
                        .process
                        .transport
                        .client
                        .request(json!({"type":"get_state"}))?;
                    self.pending.insert(id, Pending::Barrier(epoch));
                }
                Pending::Barrier(epoch) if epoch == self.epoch && self.active => {
                    self.state(data, None).await?;
                }
                Pending::EmptyEntries(epoch, session) if epoch == self.epoch && self.active => {
                    let clean = data["entries"].as_array().is_some_and(|entries| {
                        entries.iter().all(|entry| {
                            matches!(
                                entry["type"].as_str(),
                                Some("model_change" | "thinking_level_change")
                            )
                        })
                    });
                    let id = self
                        .process
                        .transport
                        .client
                        .request(json!({"type":"get_state"}))?;
                    self.pending
                        .insert(id, Pending::EmptyState(epoch, session, clean));
                }
                Pending::EmptyState(epoch, session, clean)
                    if epoch == self.epoch && self.active =>
                {
                    let proof = (data["sessionId"] == session).then_some(clean);
                    self.state(data, proof).await?;
                }
                _ => {}
            }
            return Ok(());
        }
        if frame["type"] == "agent_start" {
            self.store.mark_submitted(&self.session)?;
        }
        if frame["type"] == "agent_start" && !self.active {
            self.epoch += 1;
            self.active = true;
            self.norm.reset();
            self.assistant = uuid::Uuid::new_v4().to_string();
            self.started(
                self.request
                    .model
                    .clone()
                    .unwrap_or_else(|| "default".into()),
            )
            .await?;
        }
        if frame["type"] == "message_start" && frame["message"]["role"] == "user" {
            if self.initial_pending {
                self.initial_pending = false;
            } else {
                self.confirm_delivery().await?;
            }
        }
        for event in self.norm.map(&frame) {
            self.emit(event).await?;
        }
        if frame["type"] == "agent_settled" {
            let id = self
                .process
                .transport
                .client
                .request(json!({"type":"get_state"}))?;
            self.pending.insert(id, Pending::Barrier(self.epoch));
        }
        Ok(())
    }
}
/// Inline attachments as native image blocks, best-effort like the Claude
/// driver: the path refs already ride the prompt text, so an unreadable,
/// oversized or non-inlinable file (SVG, HEIC, TIFF, ...) must not fail the turn.
async fn load_images(paths: &[String]) -> Value {
    use base64::Engine;
    let mut images = vec![];
    for path in paths {
        match tokio::fs::metadata(path).await {
            Ok(meta) if meta.len() <= 20 * 1024 * 1024 => {}
            Ok(_) => {
                tracing::debug!(%path, "Pi attachment over inline cap; path ref only");
                continue;
            }
            Err(error) => {
                tracing::warn!(%path, %error, "Pi attachment unreadable; path ref only");
                continue;
            }
        }
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%path, %error, "Pi attachment unreadable; path ref only");
                continue;
            }
        };
        let mime = if bytes.starts_with(b"\x89PNG") {
            "image/png"
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            "image/jpeg"
        } else if bytes.starts_with(b"GIF8") {
            "image/gif"
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
            "image/webp"
        } else {
            tracing::debug!(%path, "Pi attachment is not an inlinable image; path ref only");
            continue;
        };
        images.push(json!({"type":"image","data":base64::engine::general_purpose::STANDARD.encode(bytes),"mimeType":mime}));
    }
    Value::Array(images)
}

#[cfg(test)]
mod tests {
    use super::load_images;

    #[test]
    fn pi_offers_only_bypass() {
        use crate::Harness;
        let caps = super::PiHarness::new().policy_caps();
        assert_eq!(caps.modes, vec![zeron_proto::PermissionMode::Bypass]);
        assert_eq!(caps.sandboxes, crate::sandboxing::os_sandboxes());
    }

    #[tokio::test]
    async fn unsupported_or_missing_attachments_do_not_fail_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let svg = dir.path().join("b.svg");
        std::fs::write(&svg, b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        let missing = dir.path().join("gone.jpg");
        let images = load_images(&[
            svg.display().to_string(),
            missing.display().to_string(),
            png.display().to_string(),
        ])
        .await;
        let images = images.as_array().unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0]["mimeType"], "image/png");
    }
}
