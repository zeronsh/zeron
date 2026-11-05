//! Native Factory stream-jsonrpc driver. Compatibility and probe evidence: PROTOCOL.md.
mod catalog;
mod normalize;
mod subagent;

use crate::{
    Harness, HarnessError, RunControls, SteerMessage,
    approval::{self, Action},
    jsonrpc::{Incoming, RpcClient},
    process::{Command, Stdio, owned::Child},
};
use async_trait::async_trait;
use futures::{
    FutureExt, StreamExt,
    future::BoxFuture,
    stream::{BoxStream, FuturesUnordered},
};
use normalize::{Normalizer, string};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
    UserInputAnswer, UserInputQuestion,
};

const RPC_TIMEOUT: Duration = Duration::from_secs(20);
type Input =
    dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync;

pub struct DroidHarness {
    executable: Option<PathBuf>,
    models_cache: crate::catalog::Catalog,
    interrupt_grace: Duration,
    kill_grace: Duration,
}

impl Default for DroidHarness {
    fn default() -> Self {
        Self {
            executable: None,
            models_cache: Default::default(),
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(3),
        }
    }
}

impl DroidHarness {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
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
            .or_else(|| std::env::var_os("DROID_EXECUTABLE").map(PathBuf::from))
        {
            return crate::executable::validate_native_override(&path);
        }
        let home = crate::executable::home_or_current_dir();
        crate::executable::find_on_paths(
            "droid",
            vec![
                home.join(".local/bin/droid"),
                home.join(".npm-global/bin/droid"),
                PathBuf::from("/opt/homebrew/bin/droid"),
                PathBuf::from("/usr/local/bin/droid"),
            ],
        )
        .ok_or_else(|| {
            HarnessError::NotInstalled(
                "Factory Droid: install droid or set DROID_EXECUTABLE".into(),
            )
        })
    }
    fn spawn(
        &self,
        cwd: &Path,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<Process, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut cmd = Command::new(&exe);
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, &exe);
        cmd.env("FACTORY_DROID_AUTO_UPDATE_ENABLED", "false")
            .env("DROID_DISABLE_AUTO_UPDATE", "true");
        crate::apply_run_env(&mut cmd, env);
        cmd.args([
            "exec",
            "--input-format",
            "stream-jsonrpc",
            "--output-format",
            "stream-jsonrpc",
        ])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        let mut child = Child::new(cmd.spawn().map_err(|e| {
            crate::executable::spawn_error(e, Some(cwd), || crate::executable::binary_hint(&exe))
        })?);
        let tail = crate::StderrTail::default();
        let stderr = tail.clone();
        let mut lines = crate::lossy_lines::lossy_lines(child.stderr.take().expect("piped stderr"));
        let stderr_task = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                stderr.push(&line);
            }
            stderr.close();
        });
        let (client, incoming) = RpcClient::factory(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
        );
        Ok(Process {
            child,
            client,
            incoming,
            backlog: VecDeque::new(),
            tail,
            stderr_task,
            exit_deadline: None,
        })
    }
    async fn discover(&self) -> Result<Vec<Model>, HarnessError> {
        let mut process = self.spawn(
            &crate::executable::home_or_current_dir(),
            &Default::default(),
        )?;
        let (mut sender, receiver) = oneshot::channel();
        let grace = self.kill_grace;
        tokio::spawn(async move {
            let result = tokio::select! {
                value = query(&process.client, "droid.list_models", json!({})) => value.map(|v| catalog::models(&v)),
                _ = sender.closed() => Err(protocol("Droid model discovery cancelled")),
            };
            process.shutdown(grace).await;
            let _ = sender.send(result);
        });
        receiver
            .await
            .map_err(|_| protocol("Droid model discovery task failed"))?
    }
}

#[async_trait]
impl Harness for DroidHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Droid
    }
    fn display_name(&self) -> &str {
        "Factory Droid"
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
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
    fn access_modes(&self) -> &[AccessMode] {
        &AccessMode::ALL
    }
    fn supports_plan(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    fn authoritative_prompt_end(&self) -> bool {
        true
    }
    // Background-task wakes were observed, but Loop/cron coverage is unproven.
    fn deterministic_turn_end(&self) -> bool {
        false
    }
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && picked_model(live) == picked_model(next)
            && live.reasoning == next.reasoning
            && access(live) == access(next)
            && live.plan == next.plan
    }
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && (picked_model(next).is_some() || picked_model(live).is_none())
            && (next.reasoning.is_some() || live.reasoning.is_none())
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        crate::model_context::context(HarnessId::Droid, &self.resolve_executable()?, &[]).map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(30),
                || Ok(self.model_context()?.expect("Droid context").key()),
                || self.discover(),
            )
            .await
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn fallback_models(&self) -> Vec<Model> {
        vec![Model {
            id: "default".into(),
            label: "Droid default".into(),
            description: Some("Model configured in Droid".into()),
            reasoning_levels: vec![],
            options: vec![],
        }]
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let process = self.spawn(Path::new(&request.cwd), &request.env)?;
        let (tx, rx) = mpsc::channel(256);
        let kill_grace = self.kill_grace;
        let interrupt_grace = self.interrupt_grace;
        tokio::spawn(async move {
            let RunControls {
                realtime: _,
                execution_lease: _lease,
                request_input,
                mut steering,
                interrupt,
                turn,
            } = controls;
            let subagents = subagent::Subagents::new(&request.env, &request.cwd);
            let mut runner = Runner {
                process,
                request,
                tx,
                input: Arc::from(request_input),
                settings: Value::Null,
                models: Value::Null,
                session: String::new(),
                assistant: uuid::Uuid::new_v4().to_string(),
                norm: Normalizer::default(),
                active: true,
                queued: VecDeque::new(),
                deliveries: HashMap::new(),
                dialogs: HashMap::new(),
                answers: FuturesUnordered::new(),
                background: HashSet::new(),
                subagents,
                stop_deadline: None,
            };
            let consumer = runner.tx.clone();
            let result = tokio::select! {
                value = runner.run(&mut steering, &turn, interrupt_grace) => value,
                _ = interrupt.cancelled() => {
                    let _ = runner.process.client.request_now("droid.interrupt_session", json!({}));
                    runner.cancel_dialogs().await;
                    runner.finish("cancelled").await;
                    Ok(())
                },
                _ = consumer.closed() => Ok(()),
            };
            runner.cancel_dialogs().await;
            for event in runner.subagents.close(if result.is_err() {
                DoneStatus::Errored
            } else {
                DoneStatus::Interrupted
            }) {
                let _ = runner.emit(event).await;
            }
            if let Err(error) = result {
                let status =
                    tokio::time::timeout(Duration::from_millis(250), runner.process.child.wait())
                        .await
                        .ok()
                        .and_then(Result::ok);
                if status.is_some() {
                    runner.process.tail.wait_closed().await;
                }
                runner.norm.error = Some(if status.is_some() {
                    crate::crash_message("Droid", status, &runner.process.tail)
                } else {
                    error.to_string()
                });
                runner.finish("error").await;
            }
            runner.process.shutdown(kill_grace).await;
            turn.set_background(0);
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

struct Process {
    child: Child,
    client: RpcClient,
    incoming: mpsc::Receiver<Incoming>,
    backlog: VecDeque<Incoming>,
    tail: crate::StderrTail,
    stderr_task: tokio::task::JoinHandle<()>,
    exit_deadline: Option<tokio::time::Instant>,
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stderr_task.abort();
    }
}
impl Process {
    async fn receive(&mut self) -> Incoming {
        loop {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(self.exit_deadline.unwrap_or_else(tokio::time::Instant::now)), if self.exit_deadline.is_some() => return Incoming::Eof,
                _ = self.child.wait(), if self.exit_deadline.is_none() => self.exit_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(100)),
                event = self.incoming.recv() => return event.unwrap_or(Incoming::Eof),
            }
        }
    }
    async fn next(&mut self) -> Incoming {
        match self.backlog.pop_front() {
            Some(event) => event,
            None => self.receive().await,
        }
    }
    async fn shutdown(&mut self, grace: Duration) {
        self.client.close_stdin();
        self.child.shutdown(grace).await;
    }
}

fn protocol(message: impl Into<String>) -> HarnessError {
    HarnessError::Protocol(message.into())
}
async fn query(client: &RpcClient, method: &str, params: Value) -> Result<Value, HarnessError> {
    tokio::time::timeout(RPC_TIMEOUT, client.request(method, params))
        .await
        .map_err(|_| protocol(format!("Droid {method} timed out")))?
}
fn access(request: &RunRequest) -> AccessMode {
    if request.auto_approve {
        AccessMode::FullAccess
    } else {
        request.access
    }
}
fn picked_model(request: &RunRequest) -> Option<&str> {
    request.model.as_deref().filter(|model| *model != "default")
}
fn autonomy(access: AccessMode) -> &'static str {
    match access {
        AccessMode::Supervised => "off",
        AccessMode::AutoAcceptEdits => "medium",
        AccessMode::FullAccess => "high",
    }
}
fn settings(request: &RunRequest) -> Value {
    let mut sent = json!({
        "autonomyLevel": autonomy(access(request)),
        "interactionMode": if request.plan { "spec" } else { "auto" },
        "toolExecutionMode": "direct_only",
    });
    if let Some(model) = picked_model(request) {
        sent["modelId"] = model.into();
        sent["specModeModelId"] = model.into();
        if let Some(effort) = request.reasoning {
            sent["reasoningEffort"] = serde_json::to_value(effort).unwrap();
            sent["specModeReasoningEffort"] = sent["reasoningEffort"].clone();
        }
    }
    sent
}
fn check_effective(sent: &Value, effective: &Value, models: &Value) -> Result<(), HarnessError> {
    let autonomy = &sent["autonomyLevel"];
    if !effective["availableAutonomyLevels"]
        .as_array()
        .is_some_and(|levels| levels.contains(autonomy))
    {
        return Err(protocol(format!(
            "Droid autonomy {autonomy} is not available; organization allows {}",
            effective["availableAutonomyLevels"]
        )));
    }
    for key in [
        "modelId",
        "reasoningEffort",
        "specModeModelId",
        "specModeReasoningEffort",
        "autonomyLevel",
        "interactionMode",
        "toolExecutionMode",
    ] {
        if let Some(wanted) = sent.get(key)
            && effective.get(key) != Some(wanted)
        {
            let efforts = models
                .as_array()
                .into_iter()
                .flatten()
                .find(|row| row["id"] == effective["modelId"])
                .map(|row| row["supportedReasoningEfforts"].clone())
                .unwrap_or(Value::Null);
            return Err(protocol(format!(
                "Droid {key}: requested {wanted}, effective {}; model supports reasoning efforts {efforts}",
                effective[key]
            )));
        }
    }
    Ok(())
}

struct DialogAnswer {
    id: String,
    result: Value,
    answers: Vec<UserInputAnswer>,
}
struct Runner {
    process: Process,
    request: RunRequest,
    tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    input: Arc<Input>,
    settings: Value,
    models: Value,
    session: String,
    assistant: String,
    norm: Normalizer,
    active: bool,
    queued: VecDeque<SteerMessage>,
    deliveries: HashMap<String, SteerMessage>,
    dialogs: HashMap<String, (Value, Value)>,
    answers: FuturesUnordered<BoxFuture<'static, DialogAnswer>>,
    background: HashSet<String>,
    subagents: subagent::Subagents,
    stop_deadline: Option<tokio::time::Instant>,
}

impl Runner {
    fn emit(&self, event: AgentEvent) -> BoxFuture<'static, Result<(), HarnessError>> {
        let tx = self.tx.clone();
        async move {
            tx.send(Ok(event))
                .await
                .map_err(|_| protocol("Droid event stream closed"))
        }
        .boxed()
    }
    fn started(&self) -> BoxFuture<'static, Result<(), HarnessError>> {
        self.emit(AgentEvent::SessionStarted {
            harness: HarnessId::Droid,
            model: string(&self.settings, "modelId").into(),
            cwd: self.request.cwd.clone(),
            session_id: self.session.clone(),
            assistant_message_id: self.assistant.clone(),
            tools: vec![],
        })
    }
    async fn finish(&mut self, reason: &str) {
        if self.active {
            let event = self.norm.finish(reason, &self.session);
            let _ = self.emit(event).await;
            self.active = false;
        }
        self.stop_deadline = None;
    }
    async fn initialize(&mut self) -> Result<(), HarnessError> {
        let mcp: Vec<_> = self.request.mcp.iter().chain(&self.request.extra_mcp).collect();
        let mut sent = settings(&self.request);
        let mut loaded = false;
        let result = if let Some(id) = &self.request.resume {
            match query(
                &self.process.client,
                "droid.load_session",
                json!({"sessionId":id,"mcpServers":mcp}),
            )
            .await
            {
                Ok(result) => {
                    loaded = true;
                    result
                }
                Err(error) if error.to_string().contains("code -32004") => {
                    self.emit(AgentEvent::TextDelta { text: format!("Droid could not restore session {id}; starting a new session without the previous context: {error}\n") }).await?;
                    self.new_session(&sent, &mcp).await?
                }
                Err(error) => return Err(error),
            }
        } else {
            self.new_session(&sent, &mcp).await?
        };
        self.session = result["sessionId"]
            .as_str()
            .or_else(|| result["session"]["id"].as_str())
            .or(self.request.resume.as_deref().filter(|_| loaded))
            .unwrap_or("")
            .into();
        if self.session.is_empty() {
            return Err(protocol("Droid omitted sessionId"));
        }
        self.settings = result["settings"].clone();
        self.models = result["availableModels"].clone();
        // An omitted effort uses Droid's effective effort, including its spec model.
        if sent.get("reasoningEffort").is_none() {
            sent["specModeReasoningEffort"] = self.settings["reasoningEffort"].clone();
        }
        if sent.get("modelId").is_none() {
            sent["specModeModelId"] = self.settings["modelId"].clone();
        }
        if loaded
            || sent["specModeModelId"] != self.settings["specModeModelId"]
            || sent["specModeReasoningEffort"] != self.settings["specModeReasoningEffort"]
        {
            self.update(&sent).await?;
        }
        check_effective(&sent, &self.settings, &self.models)?;
        self.started().await
    }
    fn new_session(
        &self,
        sent: &Value,
        mcp: &[&zeron_proto::McpServer],
    ) -> BoxFuture<'static, Result<Value, HarnessError>> {
        let mut params = sent.clone();
        params["machineId"] = "default".into();
        params["cwd"] = self.request.cwd.clone().into();
        params["mcpServers"] = serde_json::to_value(mcp).unwrap();
        let client = self.process.client.clone();
        async move { query(&client, "droid.initialize_session", params).await }.boxed()
    }
    async fn update(&mut self, sent: &Value) -> Result<(), HarnessError> {
        let (id, response) = self
            .process
            .client
            .request_with_id("droid.update_session_settings", sent.clone());
        tokio::time::timeout(RPC_TIMEOUT, async {
            response.await?;
            loop {
                let event = self.process.receive().await;
                if let Incoming::Notification { method, params } = &event
                    && method == "droid.session_notification"
                    && params["notification"]["type"] == "settings_updated"
                    && params["notification"]["requestId"] == id
                {
                    if let Some(fields) = params["notification"]["settings"].as_object() {
                        for (key, value) in fields {
                            self.settings[key] = value.clone();
                        }
                    }
                    return Ok(());
                }
                if matches!(event, Incoming::Eof) {
                    return Err(protocol("Droid disconnected while applying settings"));
                }
                self.process.backlog.push_back(event);
            }
        })
        .await
        .map_err(|_| protocol("Droid settings update timed out"))?
    }
    async fn submit(&mut self, steer: Option<SteerMessage>) -> Result<(), HarnessError> {
        let was_active = std::mem::replace(&mut self.active, true);
        if let Some(config) = steer.as_ref().and_then(|s| s.config.as_ref()) {
            let mut sent = settings(config);
            sent["specModeModelId"] = sent
                .get("modelId")
                .cloned()
                .unwrap_or_else(|| self.settings["modelId"].clone());
            if sent.get("reasoningEffort").is_none()
                && sent["specModeModelId"] == self.settings["modelId"]
            {
                sent["specModeReasoningEffort"] = self.settings["reasoningEffort"].clone();
            }
            self.update(&sent).await?;
            if sent.get("reasoningEffort").is_none() {
                sent["specModeReasoningEffort"] = self.settings["reasoningEffort"].clone();
                if sent["specModeReasoningEffort"] != self.settings["specModeReasoningEffort"] {
                    self.update(&sent).await?;
                }
            }
            check_effective(&sent, &self.settings, &self.models)?;
            self.request = *config.clone();
        } else if !was_active {
            let mut sent = settings(&self.request);
            sent["specModeModelId"] = self.settings["modelId"].clone();
            sent["specModeReasoningEffort"] = self.settings["reasoningEffort"].clone();
            self.update(&sent).await?;
            check_effective(&sent, &self.settings, &self.models)?;
        }
        let (text, images) = steer
            .as_ref()
            .map(|s| (&s.prompt, &s.attachments))
            .unwrap_or((&self.request.prompt, &self.request.attachments));
        let mut params = json!({"text":text});
        if !images.is_empty() {
            params["imagePaths"] = json!(images);
        }
        let (id, response) = self
            .process
            .client
            .request_with_id("droid.add_user_message", params);
        if let Some(steer) = steer {
            self.deliveries.insert(id, steer);
        }
        self.active = true;
        tokio::time::timeout(RPC_TIMEOUT, response)
            .await
            .map_err(|_| protocol("Droid prompt acknowledgement timed out"))??;
        Ok(())
    }
    async fn run(
        &mut self,
        steering: &mut mpsc::Receiver<SteerMessage>,
        turn: &crate::TurnControl,
        interrupt_grace: Duration,
    ) -> Result<(), HarnessError> {
        self.initialize().await?;
        self.submit(None).await?;
        let mut open = true;
        let mut child_poll = tokio::time::Instant::now();
        loop {
            if self.stop_deadline.is_none()
                && self
                    .queued
                    .front()
                    .is_some_and(|s| !self.active || s.config.is_none())
            {
                let steer = self.queued.pop_front().unwrap();
                self.submit(Some(steer)).await?;
            }
            if !self.active
                && !open
                && self.queued.is_empty()
                && self.deliveries.is_empty()
                && self.background.is_empty()
                && !self.subagents.is_draining()
            {
                return Ok(());
            }
            tokio::select! {
                event = self.process.next() => match event {
                    Incoming::Notification { method, params } if method == "droid.session_notification" => {
                        if params["sessionId"].as_str().is_none_or(|id| id == self.session) {
                            self.notification(&params["notification"], turn).await?;
                        }
                    },
                    Incoming::Request { id, method, params } => self.request(id, &method, params).await?,
                    Incoming::Eof if !self.active => return Ok(()),
                    Incoming::Eof => return Err(protocol("Droid stdout closed before turn completion")),
                    _ => {}
                },
                steer = steering.recv(), if open => match steer { Some(steer) => self.queued.push_back(steer), None => open = false },
                answer = self.answers.next(), if !self.answers.is_empty() => {
                    if let Some(answer) = answer
                        && let Some((wire, _)) = self.dialogs.remove(&answer.id)
                    {
                        self.process.client.respond(&wire, answer.result);
                        self.emit(AgentEvent::InputResolved { request_id: answer.id, answers: Some(answer.answers) }).await?;
                    }
                },
                _ = tokio::time::sleep_until(child_poll), if !self.subagents.is_empty() => {
                    for event in self.subagents.poll() {
                        self.emit(event).await?;
                    }
                    child_poll = tokio::time::Instant::now() + subagent::POLL;
                },
                _ = turn.stop_requested(), if self.active && self.stop_deadline.is_none() => {
                    let _ = self.process.client.request_now("droid.interrupt_session", json!({}));
                    self.stop_deadline = Some(tokio::time::Instant::now() + interrupt_grace);
                    self.cancel_dialogs().await;
                    self.queued.clear();
                    self.deliveries.clear();
                },
                _ = tokio::time::sleep_until(self.stop_deadline.unwrap_or_else(tokio::time::Instant::now)), if self.stop_deadline.is_some() => {
                    self.finish("cancelled").await;
                    return Ok(());
                },
            }
        }
    }
    async fn notification(
        &mut self,
        frame: &Value,
        turn: &crate::TurnControl,
    ) -> Result<(), HarnessError> {
        match string(frame, "type") {
            "settings_updated" => { /* Applied only at the request-correlated update barrier. */ }
            "child_session_available" => {
                self.background
                    .insert(string(frame, "childSessionId").into());
                turn.set_background(self.background.len());
                let tool = string(frame, "toolUseId");
                for event in self.norm.flush(tool) {
                    self.emit(event).await?;
                }
                self.subagents
                    .start(string(frame, "childSessionId"), &self.session, tool);
            }
            "child_session_settled" => {
                self.background.remove(string(frame, "childSessionId"));
                turn.set_background(self.background.len());
                self.subagents
                    .finish(string(frame, "childSessionId"), string(frame, "status"));
            }
            "create_message" if frame["message"]["role"] == "user" => {
                if self.deliveries.remove(string(frame, "requestId")).is_some() {
                    let old =
                        std::mem::replace(&mut self.assistant, uuid::Uuid::new_v4().to_string());
                    self.emit(AgentEvent::Steered {
                        assistant_message_id: Some(old),
                        next_assistant_message_id: Some(self.assistant.clone()),
                    })
                    .await?;
                }
            }
            "queued_messages_discarded" => {
                if self.deliveries.remove(string(frame, "requestId")).is_some() {
                    self.emit(AgentEvent::Error {
                        message: "Droid discarded an undelivered steering message.".into(),
                    })
                    .await?;
                }
            }
            "assistant_text_delta" | "thinking_text_delta" | "tool_call" if !self.active => {
                self.active = true;
                self.assistant = uuid::Uuid::new_v4().to_string();
                self.started().await?;
            }
            _ => {}
        }
        for event in self.norm.map(frame) {
            self.emit(event).await?;
        }
        if frame["type"] == "agent_turn_completed" {
            self.cancel_dialogs().await;
            self.finish(if self.stop_deadline.is_some() {
                "cancelled"
            } else {
                string(frame, "reason")
            })
            .await;
        }
        Ok(())
    }
    async fn cancel_dialogs(&mut self) {
        self.answers.clear();
        for (id, (wire, result)) in self.dialogs.drain().collect::<Vec<_>>() {
            self.process.client.respond(&wire, result);
            let _ = self
                .emit(AgentEvent::InputResolved {
                    request_id: id,
                    answers: Some(vec![]),
                })
                .await;
        }
    }
    async fn request(
        &mut self,
        wire: Value,
        method: &str,
        params: Value,
    ) -> Result<(), HarnessError> {
        let (questions, cancelled, permission) = match method {
            "droid.request_permission" => {
                for item in params["toolUses"].as_array().into_iter().flatten() {
                    let events = match self
                        .subagents
                        .permission(&params, item)
                        .unwrap_or_else(|| self.norm.permission(item))
                    {
                        Ok(events) => events,
                        Err(message) => {
                            self.process
                                .client
                                .respond(&wire, json!({"selectedOption":"cancel"}));
                            self.emit(AgentEvent::Error {
                                message: message.into(),
                            })
                            .await?;
                            return Ok(());
                        }
                    };
                    for event in events {
                        self.emit(event).await?;
                    }
                }
                let Some(question) = permission_question(&params, access(&self.request)) else {
                    self.process.client.respond(&wire, json!({"selectedOption": if self.active { "proceed_once" } else { "cancel" }}));
                    return Ok(());
                };
                let option = if contains_plan(&params) {
                    let level = autonomy(access(&self.request));
                    let option = if level == "off" {
                        "proceed_once".into()
                    } else {
                        format!("proceed_auto_run_{level}")
                    };
                    if !params["options"].as_array().is_some_and(|options| {
                        options.iter().any(|offered| offered["value"] == option)
                    }) {
                        self.process
                            .client
                            .respond(&wire, json!({"selectedOption":"cancel"}));
                        self.emit(AgentEvent::Error {
                            message: format!(
                                "Droid plan not approved: {level} autonomy is not offered for implementation."
                            ).into(),
                        })
                        .await?;
                        return Ok(());
                    }
                    option
                } else {
                    "proceed_once".into()
                };
                (
                    vec![question],
                    json!({"selectedOption":"cancel"}),
                    Some(option),
                )
            }
            "droid.ask_user" => {
                let questions = params["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|q| UserInputQuestion {
                        id: q["index"].to_string(),
                        header: string(q, "topic").into(),
                        question: string(q, "question").into(),
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect(),
                        multi_select: q["multiSelect"] == true,
                        prefill: None,
                        multiline: false,
                    })
                    .collect();
                (questions, json!({"cancelled":true,"answers":[]}), None)
            }
            _ => {
                self.process
                    .client
                    .respond_error(&wire, -32601, "Unknown method");
                return Ok(());
            }
        };
        if !self.active || questions.is_empty() {
            self.process.client.respond(&wire, cancelled);
            return Ok(());
        }
        let id = wire.as_str().unwrap_or("").to_owned();
        self.emit(AgentEvent::InputRequested {
            request_id: id.clone(),
            questions: questions.clone(),
        })
        .await?;
        let receiver = (self.input)(questions.clone());
        self.dialogs.insert(id.clone(), (wire, cancelled.clone()));
        self.answers.push(async move {
            let answers = receiver.await.unwrap_or_default();
            let result = if let Some(option) = permission {
                json!({"selectedOption": if approval::approved(&questions[0], &answers) { option.as_str() } else { "cancel" }})
            } else {
                let encoded: Vec<_> = questions.iter().filter_map(|q| {
                    let answer = answers.iter().find(|a| a.question_id == q.id && !a.labels.is_empty())?;
                    Some(json!({"index":q.id.parse::<usize>().unwrap_or(1),"question":q.question,"answer":answer.labels.join(", ")}))
                }).collect();
                if encoded.len() != questions.len() { cancelled } else { json!({"answers":encoded}) }
            };
            DialogAnswer { id, result, answers }
        }.boxed());
        Ok(())
    }
}

fn contains_plan(params: &Value) -> bool {
    params["toolUses"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|item| item["confirmationType"] == "exit_spec_mode")
}
fn permission_question(params: &Value, access: AccessMode) -> Option<UserInputQuestion> {
    let items: Vec<_> = params["toolUses"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    if items.is_empty() {
        return Some(approval::tool("Droid", "an unspecified tool", None));
    }
    // Leaving spec mode is a plan decision, not a tool permission.
    // Full access (including legacy auto_approve) must still ask.
    if contains_plan(params) {
        return Some(approval::question(
            "Approve plan",
            "Review the plan above. Allow Droid to start implementing it?",
        ));
    }
    if items.iter().all(|item| {
        let action = match string(item, "confirmationType") {
            "edit" | "create" | "apply_patch" => Action::Edit,
            _ => Action::Other,
        };
        approval::allowed_without_asking(access, action)
    }) {
        return None;
    }
    if items.len() == 1 {
        let item = items[0];
        let details = &item["details"];
        return Some(match string(item, "confirmationType") {
            "exec" => approval::command("Droid", string(details, "fullCommand")),
            "edit" | "create" | "apply_patch" => approval::edit(
                "Droid",
                &details["filePath"].as_str().into_iter().collect::<Vec<_>>(),
            ),
            _ => approval::tool("Droid", string(&item["toolUse"], "name"), None),
        });
    }
    let names = items
        .iter()
        .map(|item| {
            let details = &item["details"];
            details["fullCommand"]
                .as_str()
                .or_else(|| details["filePath"].as_str())
                .unwrap_or_else(|| string(&item["toolUse"], "name"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    Some(approval::tool("Droid", "the requested tools", Some(&names)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> RunRequest {
        serde_json::from_value(json!({"prompt":"hello","model":null,"reasoning":null,"cwd":"/tmp","sandbox":"workspace-write","resume":null})).unwrap()
    }
    #[test]
    fn settings_are_explicit_and_runtime_capabilities_match() {
        let harness = DroidHarness::new();
        let mut req = request();
        for (access, level) in [
            (AccessMode::Supervised, "off"),
            (AccessMode::AutoAcceptEdits, "medium"),
            (AccessMode::FullAccess, "high"),
        ] {
            req.access = access;
            for plan in [false, true] {
                req.plan = plan;
                let sent = settings(&req);
                assert_eq!(sent["autonomyLevel"], level);
                assert_eq!(sent["interactionMode"], if plan { "spec" } else { "auto" });
            }
        }
        req.access = AccessMode::Supervised;
        req.auto_approve = true;
        req.plan = false;
        assert_eq!(settings(&req)["autonomyLevel"], "high");
        req.plan = true;
        assert_eq!(settings(&req)["interactionMode"], "spec");
        assert!(settings(&req).get("modelId").is_none());
        req.model = Some("model-fast".into());
        req.reasoning = Some(ReasoningLevel::High);
        let sent = settings(&req);
        assert_eq!(sent["specModeModelId"], sent["modelId"]);
        assert_eq!(sent["specModeReasoningEffort"], sent["reasoningEffort"]);
        let mut next = req.clone();
        next.plan = false;
        assert!(!harness.same_runtime(&req, &next));
        assert!(harness.reconfigures_in_place(&req, &next));
        next.model = None;
        assert!(!harness.reconfigures_in_place(&req, &next));
        assert_eq!(harness.access_modes(), AccessMode::ALL);
        assert!(
            harness.supports_plan()
                && harness.stops_turn_in_place()
                && harness.authoritative_prompt_end()
        );
        assert!(!harness.deterministic_turn_end());
    }
}
