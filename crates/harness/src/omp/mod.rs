//! Native Oh My Pi (`omp --mode rpc-ui`) driver. OMP speaks a Pi-family JSONL
//! dialect, so the transport, normalizer, dialogs and MCP extension are Pi's.
//! The turn lifecycle differs; see PROTOCOL.md.
mod catalog;
pub(crate) mod sessions;

use crate::{
    Harness, HarnessError, RunControls,
    pi::{
        Launch, Process, load_images,
        normalize::{Normalizer, string},
        response_data, ui,
    },
};
use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::mpsc;
use zeron_proto::{AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode};

const LABEL: &str = "Oh My Pi";
/// First release with `prompt_result`, `negotiate_protocol` and
/// `abort_and_restore_queue`.
const MIN_VERSION: semver::Version = semver::Version::new(18, 11, 0);

pub struct OmpHarness {
    models_cache: crate::catalog::Catalog,
    workspace_commands: crate::skills::CommandDiscovery,
    session_store: Option<PathBuf>,
    agent_dir: Option<PathBuf>,
    executable: Option<PathBuf>,
    interrupt_grace: Duration,
    kill_grace: Duration,
}
impl Default for OmpHarness {
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
impl OmpHarness {
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
    /// OMP's agent directory (`PI_CODING_AGENT_DIR` for the child, like Pi).
    /// Defaults to the directory OMP itself resolves.
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
            .or_else(|| std::env::var_os("OMP_EXECUTABLE").map(PathBuf::from))
        {
            return crate::executable::validate_native_override(&path);
        }
        let home = crate::executable::home_or_current_dir();
        crate::executable::find_on_paths(
            "omp",
            vec![
                home.join(".local/bin/omp"),
                home.join(".bun/bin/omp"),
                home.join(".npm-global/bin/omp"),
                PathBuf::from("/opt/homebrew/bin/omp"),
                PathBuf::from("/usr/local/bin/omp"),
            ],
        )
        .ok_or_else(|| {
            HarnessError::NotInstalled("Oh My Pi CLI: install omp or set OMP_EXECUTABLE".into())
        })
    }
    fn agent_dir(&self) -> PathBuf {
        sessions::agent_dir(self.agent_dir.clone())
    }
    fn spawn(
        &self,
        cwd: &Path,
        args: &[String],
        mcp: Option<&zeron_proto::McpServer>,
    ) -> Result<Process, HarnessError> {
        let exe = self.resolve_executable()?;
        if self.executable.is_none()
            && let Some(version) = crate::executable::binary_version(&exe)
            && version < MIN_VERSION
        {
            return Err(HarnessError::Protocol(format!(
                "Oh My Pi {version} is unsupported; update omp to {MIN_VERSION} or newer"
            )));
        }
        // `rpc-ui` lets the agent's `ask` tool reach Zeron's input requests.
        // `--allow-home` stops OMP from moving a home-directory chat to a temp dir.
        let mut all = vec!["--mode".into(), "rpc-ui".into(), "--allow-home".into()];
        all.extend_from_slice(args);
        crate::pi::spawn(Launch {
            label: LABEL,
            exe: &exe,
            args: all,
            // Only an explicit directory is forced; otherwise OMP resolves its
            // own (OMP_PROFILE, env override, PI_CONFIG_DIR), as in a terminal.
            agent_dir: self.agent_dir.clone(),
            unset: sessions::PROFILE_VARS,
            cwd,
            mcp,
        })
    }
    async fn probe_commands(&self, cwd: &Path) -> Result<Value, HarnessError> {
        let mut process = self.spawn(cwd, &["--no-session".into()], None)?;
        let (mut tx, rx) = tokio::sync::oneshot::channel();
        let grace = self.kill_grace;
        tokio::spawn(async move {
            let result = tokio::select! {
                result=tokio::time::timeout(Duration::from_secs(60),async {
                    let mut backlog = vec![];
                    handshake(&mut process, &mut backlog).await?;
                    process.query(json!({"type":"get_available_commands"}),&mut backlog).await
                })=>result.unwrap_or_else(|_|Err(HarnessError::Protocol("Oh My Pi discovery timed out".into()))),
                _=tx.closed()=>Err(HarnessError::Protocol("Oh My Pi discovery cancelled".into())),
            };
            process.shutdown(grace).await;
            let _ = tx.send(result);
        });
        rx.await
            .map_err(|_| HarnessError::Protocol("Oh My Pi discovery task failed".into()))?
    }
}
/// Wait for OMP's `ready` frame and opt into protocol v2, whose `rpc_chunk`
/// framing carries frames over 1 MiB (a large `get_state`) losslessly.
async fn handshake(process: &mut Process, backlog: &mut Vec<Value>) -> Result<(), HarnessError> {
    let ready = loop {
        let frame = process.next().await?;
        if frame["type"] == "ready" {
            break frame;
        }
        if frame["type"] == "extension_ui_request" {
            process
                .dialogs
                .request(process.transport.client.clone(), &frame);
        } else {
            backlog.push(frame);
        }
    };
    if ready["supportedProtocolVersions"]
        .as_array()
        .is_some_and(|versions| versions.iter().any(|v| v == 2))
    {
        process
            .query(
                json!({"type":"negotiate_protocol","protocolVersion":2}),
                backlog,
            )
            .await?;
    }
    Ok(())
}
/// Which providers hold usable credentials decides which models OMP lists.
/// Auth lives in `agent.db`, which OMP rewrites on every token refresh and
/// usage update, so only provider, type and enabled state are read: tokens are
/// never selected, and a refresh does not invalidate the catalog.
fn credential_providers(agent: &Path) -> String {
    use rusqlite::{Connection, OpenFlags};
    let read = || -> rusqlite::Result<String> {
        let db = Connection::open_with_flags(
            agent.join("agent.db"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(Duration::from_millis(250))?;
        let mut query = db.prepare(
            "SELECT provider, credential_type, disabled_cause IS NULL \
             FROM auth_credentials ORDER BY 1, 2, 3",
        )?;
        let rows = query.query_map([], |row| {
            Ok(format!(
                "{}:{}:{}\n",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?
            ))
        })?;
        rows.collect()
    };
    // No database (or an unknown schema) contributes nothing; files and env still key.
    read().unwrap_or_default()
}
/// Whether `request` picks a thinking level (an effort, or thinking off).
fn thinking_selected(request: &RunRequest) -> bool {
    request.reasoning.is_some()
        || request
            .model_options
            .get(catalog::THINKING_OPTION)
            .is_some_and(|v| v == "off")
}
#[async_trait]
impl Harness for OmpHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Omp
    }
    fn display_name(&self) -> &str {
        LABEL
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
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    /// `set_model` and `set_thinking_level` change the live process. Clearing
    /// either back to the default cannot be expressed that way.
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        let model = |r: &RunRequest| r.model.as_deref().filter(|m| *m != "default").is_some();
        live.cwd == next.cwd
            && (model(next) || !model(live))
            && (thinking_selected(next) || !thinking_selected(live))
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        let root = self.agent_dir();
        crate::model_context::context_salted(
            HarnessId::Omp,
            &self.resolve_executable()?,
            &[
                root.join("config.yml"),
                root.join("models.yml"),
                root.join(".env"),
            ],
            credential_providers(&root).as_bytes(),
        )
        .map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(65),
                || Ok(self.model_context()?.expect("Oh My Pi model context").key()),
                || async {
                    catalog::models(&self.resolve_executable()?, self.agent_dir.as_deref()).await
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
                Ok(catalog::commands(&self.probe_commands(cwd).await?))
            })
            .await
    }
    async fn skills(
        &self,
        cwd: &Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        let mut skills = crate::skills::discover(HarnessId::Omp, cwd).await?;
        let commands = self.commands_for(cwd).await?;
        crate::skills::attach_advertised_commands(HarnessId::Omp, &mut skills, &commands);
        Ok(Some(skills))
    }
    fn fallback_models(&self) -> Vec<Model> {
        vec![Model {
            id: "default".into(),
            label: "Oh My Pi default".into(),
            description: Some("Model configured in Oh My Pi".into()),
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
                Err(error) => {
                    // OMP writes a session file only once a turn starts. A chat
                    // that never started one has no context to lose.
                    if !store.never_submitted(id) {
                        lost_context = Some(format!(
                            "Oh My Pi could not restore session {id}; starting a new session without the previous context: {error}"
                        ));
                    }
                    request.resume = None;
                    vec![]
                }
            },
            None => vec![],
        };
        let process = self.spawn(Path::new(&request.cwd), &args, request.mcp.as_ref())?;
        let (tx, rx) = mpsc::channel(256);
        let kill_grace = self.kill_grace;
        let interrupt_grace = self.interrupt_grace;
        tokio::spawn(async move {
            let mut runner = Runner {
                store,
                process,
                tx,
                request,
                norm: Normalizer::new(LABEL),
                active: true,
                interrupted: false,
                initial_pending: true,
                initial: None,
                unsettled: false,
                unacked: HashSet::new(),
                outstanding: HashSet::new(),
                deliveries: VecDeque::new(),
                queued: VecDeque::new(),
                barrier: None,
                session: String::new(),
                assistant: uuid::Uuid::new_v4().to_string(),
                lost_context,
            };
            // The lease lives through shutdown even if the consumer drops its stream.
            let RunControls {
                realtime: _,
                execution_lease: _lease,
                request_input,
                mut steering,
                interrupt,
                turn: _,
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
                    crate::crash_message(LABEL, status, &runner.process.tail)
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
struct Runner {
    store: sessions::Store,
    process: Process,
    tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    request: RunRequest,
    norm: Normalizer,
    active: bool,
    interrupted: bool,
    /// The run's first prompt has not reached the transcript yet.
    initial_pending: bool,
    initial: Option<String>,
    /// A prompt yielded while background work (async task, backgrounded
    /// bash) can still wake the agent; the turn ends at `session_settled`.
    unsettled: bool,
    /// Prompt ids not yet admitted. The next steer waits for admission so
    /// OMP receives inputs in Zeron's order.
    unacked: HashSet<String>,
    /// Prompt ids that have not completed (`agentInvoked:false` on the
    /// response, or their `prompt_result`).
    outstanding: HashSet<String>,
    /// Steers sent but not yet consumed, in submission order.
    deliveries: VecDeque<String>,
    queued: VecDeque<crate::SteerMessage>,
    /// The `get_state` request that refreshes session identity before Done.
    barrier: Option<String>,
    session: String,
    assistant: String,
    lost_context: Option<String>,
}
impl Runner {
    async fn emit(&self, event: AgentEvent) -> Result<(), HarnessError> {
        self.tx
            .send(Ok(event))
            .await
            .map_err(|_| HarnessError::Protocol("Oh My Pi event consumer closed".into()))
    }
    async fn started(&self, model: String) -> Result<(), HarnessError> {
        self.emit(AgentEvent::SessionStarted {
            harness: HarnessId::Omp,
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
        self.barrier = None;
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
    fn submit(&mut self, text: String, images: Value) -> Result<String, HarnessError> {
        // `followUp` is ignored when idle and queues after the current run otherwise.
        let id = self.process.transport.client.request(
            json!({"type":"prompt","message":text,"images":images,"streamingBehavior":if self.initial_pending {"followUp"} else {"steer"}}),
        )?;
        self.unacked.insert(id.clone());
        self.outstanding.insert(id.clone());
        Ok(id)
    }
    async fn bootstrap(&mut self, backlog: &mut Vec<Value>) -> Result<Value, HarnessError> {
        handshake(&mut self.process, backlog).await?;
        // Session-scoped in OMP (never written to config.yml): Zeron's pending
        // steers belong together at the next model step.
        self.process
            .query(json!({"type":"set_steering_mode","mode":"all"}), backlog)
            .await?;
        let request = self.request.clone();
        self.configure(&request, backlog).await?;
        let commands = self
            .process
            .query(json!({"type":"get_available_commands"}), backlog)
            .await?;
        self.emit(AgentEvent::AvailableCommands {
            commands: catalog::commands(&commands),
        })
        .await?;
        self.process
            .query(json!({"type":"get_state"}), backlog)
            .await
    }
    /// Select `request`'s model and thinking level on the live process.
    async fn configure(
        &mut self,
        request: &RunRequest,
        backlog: &mut Vec<Value>,
    ) -> Result<(), HarnessError> {
        if let Some(model) = request.model.as_deref().filter(|s| *s != "default") {
            let models = self
                .process
                .query(json!({"type":"get_available_models"}), backlog)
                .await?;
            let options = models["models"]
                .as_array()
                .ok_or_else(|| HarnessError::Protocol("Oh My Pi returned no models".into()))?;
            let found = options
                .iter()
                .find(|m| format!("{}/{}", string(m, "provider"), string(m, "id")) == model)
                .or_else(|| {
                    let mut matches = options.iter().filter(|m| string(m, "id") == model);
                    let first = matches.next()?;
                    matches.next().is_none().then_some(first)
                })
                .ok_or_else(|| {
                    HarnessError::Protocol(format!("Oh My Pi model is not available: {model}"))
                })?;
            self.process
                .query(
                    json!({"type":"set_model","provider":found["provider"],"modelId":found["id"]}),
                    backlog,
                )
                .await?;
        }
        if thinking_selected(request) {
            let supported = self
                .process
                .query(json!({"type":"get_available_thinking_levels"}), backlog)
                .await?;
            let levels = supported["levels"].as_array().cloned().unwrap_or_default();
            let desired = if request
                .model_options
                .get(catalog::THINKING_OPTION)
                .is_some_and(|v| v == "off")
            {
                "off".to_string()
            } else {
                serde_json::to_value(request.reasoning)
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
                    HarnessError::Protocol("Oh My Pi returned no supported thinking level".into())
                })?;
            self.process
                .query(
                    json!({"type":"set_thinking_level","level":effective}),
                    backlog,
                )
                .await?;
        }
        Ok(())
    }
    async fn run(
        &mut self,
        steering: &mut mpsc::Receiver<crate::SteerMessage>,
    ) -> Result<(), HarnessError> {
        let mut backlog = vec![];
        let state = tokio::time::timeout(Duration::from_secs(120), self.bootstrap(&mut backlog))
            .await
            .map_err(|_| HarnessError::Protocol("Oh My Pi startup timed out".into()))??;
        self.session = string(&state, "sessionId").into();
        if self.session.is_empty() {
            return Err(HarnessError::Protocol(
                "Oh My Pi get_state omitted sessionId".into(),
            ));
        }
        if self
            .request
            .resume
            .as_ref()
            .is_some_and(|id| id != &self.session)
        {
            return Err(HarnessError::Protocol(
                "Oh My Pi resumed a different session".into(),
            ));
        }
        if let Some(file) = state["sessionFile"].as_str() {
            self.store.remember(&self.session, Path::new(file))?;
        }
        self.norm.window = state["model"]["contextWindow"].as_u64();
        self.started(state["model"]["id"].as_str().unwrap_or("default").into())
            .await?;
        if let Some(message) = self.lost_context.take() {
            self.emit(AgentEvent::Error { message }).await?;
        }
        for frame in backlog {
            // Startup toasts ("extension loaded") are not part of the answer.
            if frame["type"] == "extension_ui_request" && frame["method"] == "notify" {
                continue;
            }
            self.frame(frame).await?;
        }
        let images = load_images(&self.request.attachments).await;
        self.initial = Some(self.submit(self.request.prompt.clone(), images)?);
        let mut open = true;
        loop {
            if self.unacked.is_empty()
                && let Some(steer) = self.queued.pop_front()
            {
                // A changed model or thinking level applies on the live
                // process before the message it was sent with.
                if let Some(next) = &steer.config {
                    let mut backlog = vec![];
                    self.configure(next, &mut backlog).await?;
                    self.request.model = next.model.clone();
                    self.request.reasoning = next.reasoning;
                    self.request.model_options = next.model_options.clone();
                    for frame in backlog {
                        self.frame(frame).await?;
                    }
                }
                if !self.active {
                    self.norm.reset();
                    self.active = true;
                }
                // Atomic in OMP: queued at a step boundary if busy, a new run if idle.
                let images = load_images(&steer.attachments).await;
                let id = self.submit(steer.prompt.clone(), images)?;
                self.deliveries.push_back(id);
                self.barrier = None;
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
    async fn confirm_delivery(&mut self, id: Option<&str>) -> Result<(), HarnessError> {
        let removed = match id {
            Some(id) => self
                .deliveries
                .iter()
                .position(|d| d == id)
                .and_then(|at| self.deliveries.remove(at)),
            None => self.deliveries.pop_front(),
        };
        if removed.is_some() {
            let old = std::mem::replace(&mut self.assistant, uuid::Uuid::new_v4().to_string());
            self.emit(AgentEvent::Steered {
                assistant_message_id: Some(old),
                next_assistant_message_id: Some(self.assistant.clone()),
            })
            .await?;
        }
        Ok(())
    }
    /// One prompt finished. Once none is left, refresh the session identity
    /// (a slash command can switch sessions) and end the turn.
    async fn completed(&mut self, id: &str) -> Result<(), HarnessError> {
        self.unacked.remove(id);
        if self.outstanding.remove(id) {
            if self.deliveries.iter().any(|d| d == id) {
                // Finished without reaching the transcript (a local command).
                self.confirm_delivery(Some(id)).await?;
            }
            if self.initial.as_deref() == Some(id) {
                self.initial_pending = false;
            }
        }
        self.settle_if_idle()
    }
    fn settle_if_idle(&mut self) -> Result<(), HarnessError> {
        if self.active
            && !self.unsettled
            && self.outstanding.is_empty()
            && self.queued.is_empty()
            && self.barrier.is_none()
        {
            let id = self
                .process
                .transport
                .client
                .request(json!({"type":"get_state"}))?;
            self.barrier = Some(id);
        }
        Ok(())
    }
    async fn state(&mut self, data: Value) -> Result<(), HarnessError> {
        let session = data["sessionId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| HarnessError::Protocol("Oh My Pi get_state omitted sessionId".into()))?;
        let changed = self.session != session;
        self.session = session.into();
        if let Some(file) = data["sessionFile"].as_str() {
            self.store.remember(&self.session, Path::new(file))?;
        }
        if changed {
            self.started(data["model"]["id"].as_str().unwrap_or("default").into())
                .await?;
        }
        self.norm.window = data["model"]["contextWindow"].as_u64().or(self.norm.window);
        if !self.interrupted {
            // Every input was answered; any still counted was consumed silently.
            while !self.deliveries.is_empty() {
                self.confirm_delivery(None).await?;
            }
        }
        self.finish().await
    }
    async fn interrupt(&mut self, grace: Duration) {
        self.interrupted = true;
        self.process.dialogs.cancel();
        self.queued.clear();
        self.deliveries.clear();
        if !self.active {
            return;
        }
        // Pi's clear_queue + abort. The withdrawn queue is discarded.
        let Ok(abort) = self
            .process
            .transport
            .client
            .request(json!({"type":"abort_and_restore_queue"}))
        else {
            return;
        };
        let _ = tokio::time::timeout(grace, async {
            while let Ok(frame) = self.process.next().await {
                if frame["id"] == abort && frame["type"] == "response" {
                    break;
                }
                if self.frame(frame).await.is_err() {
                    break;
                }
            }
        })
        .await;
    }
    async fn frame(&mut self, frame: Value) -> Result<(), HarnessError> {
        match string(&frame, "type") {
            "extension_ui_request" => {
                if !self.active {
                    // A background notification must not reopen a completed engine turn.
                    // No foreground run can own a dialog here, so cancel it explicitly.
                    ui::Dialogs::new(LABEL).request(self.process.transport.client.clone(), &frame);
                    tracing::debug!(method = %string(&frame, "method"), "Oh My Pi UI request outside a run");
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
            "available_commands_update" => {
                return self
                    .emit(AgentEvent::AvailableCommands {
                        commands: catalog::commands(&frame),
                    })
                    .await;
            }
            // Session-persistence failures (`source: "session-persistence"`).
            "notice" if frame["level"] == "error" => {
                if self.active {
                    return self
                        .emit(AgentEvent::Error {
                            message: format!("{LABEL}: {}", string(&frame, "message")),
                        })
                        .await;
                }
                tracing::warn!(message = %string(&frame, "message"), "Oh My Pi notice outside a run");
                return Ok(());
            }
            "command_output" if self.active => {
                return self
                    .emit(AgentEvent::TextDelta {
                        text: string(&frame, "text").into(),
                    })
                    .await;
            }
            "response" => {
                let id = string(&frame, "id").to_owned();
                if self.barrier.as_deref() == Some(id.as_str()) {
                    self.barrier = None;
                    if self.active && self.outstanding.is_empty() && self.queued.is_empty() {
                        return self.state(response_data(frame, LABEL)?).await;
                    }
                    return Ok(());
                }
                if !self.outstanding.contains(&id) {
                    return Ok(());
                }
                if frame["success"] != true {
                    // Failed before reaching the agent. A `prompt_result` with
                    // the same id may follow; the first completion wins.
                    self.norm.error = Some(format!(
                        "Oh My Pi prompt: {}",
                        frame["error"].as_str().unwrap_or("command failed")
                    ));
                    if self.unacked.contains(&id) {
                        return self.completed(&id).await;
                    }
                    return Ok(());
                }
                self.unacked.remove(&id);
                if frame["data"]["agentInvoked"] == false {
                    // A builtin slash command finished locally; no prompt_result follows.
                    return self.completed(&id).await;
                }
                return Ok(());
            }
            "prompt_result" => {
                let id = string(&frame, "id").to_owned();
                if !self.outstanding.contains(&id) {
                    return Ok(());
                }
                match string(&frame, "status") {
                    "aborted" if !self.interrupted => self.norm.aborted = true,
                    "error" => {
                        self.norm.error = Some(
                            frame["error"]["message"]
                                .as_str()
                                .unwrap_or("Oh My Pi provider failed")
                                .to_owned(),
                        )
                    }
                    _ => {}
                }
                if frame["sessionSettled"] == false {
                    self.unsettled = true;
                }
                return self.completed(&id).await;
            }
            "session_settled" => {
                // Ends a turn whose prompt yielded with background work
                // pending, or a run a background job started on its own.
                self.unsettled = false;
                return self.settle_if_idle();
            }
            "agent_start" => {
                self.store.mark_submitted(&self.session)?;
                if !self.active {
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
            }
            "message_start" if frame["message"]["role"] == "user" => {
                if self.initial_pending {
                    self.initial_pending = false;
                } else {
                    self.confirm_delivery(None).await?;
                }
            }
            _ => {}
        }
        for event in self.norm.map(&frame) {
            self.emit(event).await?;
        }
        Ok(())
    }
}
