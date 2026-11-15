//! Native Factory Droid stream-jsonrpc sessions. Settings are checked at a
//! correlated idle barrier before every prompt; only agent_turn_completed
//! settles a turn. The process and execution lease survive a foreground stop.
mod catalog;
mod dialogs;
mod normalize;
mod subagents;

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::Duration,
};

use async_trait::async_trait;
use futures::{
    StreamExt,
    future::BoxFuture,
    stream::{BoxStream, FuturesUnordered},
};
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};
use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
    UserInputAnswer, UserInputQuestion,
};

use crate::{
    Harness, HarnessError, RunControls, SteerMessage,
    jsonrpc::{Incoming, RpcClient},
    process::{Command, Stdio, owned::Child},
};
use normalize::string;

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(120);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_NOTIFICATION: &str = "droid.session_notification";

pub struct DroidHarness {
    executable: Option<PathBuf>,
    kill_grace: Duration,
    models_cache: crate::catalog::Catalog,
}

impl Default for DroidHarness {
    fn default() -> Self {
        Self {
            executable: None,
            kill_grace: Duration::from_secs(3),
            models_cache: Default::default(),
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
                "Factory Droid: install `droid` or set DROID_EXECUTABLE".into(),
            )
        })
    }

    fn context(&self) -> Result<crate::ModelContext, HarnessError> {
        crate::model_context::context(HarnessId::Droid, &self.resolve_executable()?, &[])
    }

    async fn discover_models(&self) -> Result<Vec<Model>, HarnessError> {
        let context = self.context()?;
        context.log();
        let mut host = Host::spawn(
            &context.binary_path,
            &std::env::current_dir()?,
            &BTreeMap::new(),
        )?;
        let result = host.call("droid.list_models", json!({}), RPC_TIMEOUT).await;
        host.shutdown(self.kill_grace).await;
        result.map(|result| catalog::models(&result))
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
    fn authoritative_prompt_end(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        launch_matches(live, next)
            && live.model == next.model
            && live.reasoning == next.reasoning
            && live.access == next.access
            && live.auto_approve == next.auto_approve
            && live.plan == next.plan
    }
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        launch_matches(live, next)
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        self.context().map(Some)
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(25),
                || Ok(self.context()?.key()),
                || self.discover_models(),
            )
            .await
    }
    fn fallback_models(&self) -> Vec<Model> {
        vec![Model {
            id: "default".into(),
            label: "Droid default".into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![],
            provider: None,
            default: false,
        }]
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.run_with_launch(request, controls, Launch::Coding)
            .await
    }

    async fn run_title(
        &self,
        mut request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        request.resume = None;
        request.worktree = None;
        request.attachments.clear();
        request.mcp = None;
        request.extra_mcp.clear();
        request.model_options.clear();
        request.auto_approve = false;
        request.access = AccessMode::Supervised;
        request.plan = false;
        let models = self
            .models_cache
            .cached_for(self.context()?.key())
            .unwrap_or_default();
        self.run_with_launch(request, controls, Launch::Title { models })
            .await
    }
}

impl DroidHarness {
    async fn run_with_launch(
        &self,
        request: RunRequest,
        controls: RunControls,
        launch: Launch,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let mut host = Host::spawn(
            &self.resolve_executable()?,
            Path::new(&request.cwd),
            &request.env,
        )?;
        let (tx, rx) = mpsc::channel(256);
        let grace = self.kill_grace;
        let RunControls {
            request_input,
            realtime: _,
            execution_lease,
            mut steering,
            interrupt,
            turn,
        } = controls;
        tokio::spawn(async move {
            let _lease = execution_lease;
            let consumer = tx.clone();
            let abandoned = async {
                tokio::select! {
                    _ = interrupt.cancelled() => {}
                    _ = consumer.closed() => {}
                }
            };
            tokio::pin!(abandoned);
            let opened = tokio::select! {
                opened = open(&mut host, &launch, &request, &tx) => Some(opened),
                _ = &mut abandoned => None,
            };
            match opened {
                Some(Ok((session_id, settings))) => {
                    let mut runner =
                        Runner::new(&mut host, launch, tx, request, turn, session_id, settings);
                    let result = tokio::select! {
                        result = runner.run(&mut steering) => result,
                        _ = &mut abandoned => {
                            runner.interrupt().await;
                            Ok(())
                        }
                    };
                    runner.close(result).await;
                }
                Some(Err(error)) => {
                    let _ = tx.send(Output::Event(failed(&error, None))).await;
                }
                None => {}
            }
            host.shutdown(grace).await;
        });
        Ok(
            futures::stream::unfold((rx, request_input), |(mut rx, input)| async move {
                loop {
                    match rx.recv().await? {
                        Output::Event(event) => return Some((Ok(event), (rx, input))),
                        Output::Input { questions, reply } => {
                            if !reply.is_closed() {
                                let _ = reply.send(input(questions));
                            }
                        }
                    }
                }
            })
            .boxed(),
        )
    }
}

fn launch_matches(live: &RunRequest, next: &RunRequest) -> bool {
    live.cwd == next.cwd
        && live.env == next.env
        && live.mcp == next.mcp
        && live.extra_mcp == next.extra_mcp
        && crate::same_model_options(&live.model_options, &next.model_options)
}

/// What a session is opened for, fixed for its life.
enum Launch {
    Coding,
    /// One tool-free turn that names a thread, its effort from `models`.
    Title {
        models: Vec<Model>,
    },
}

impl Launch {
    fn is_title(&self) -> bool {
        matches!(self, Self::Title { .. })
    }

    /// What a session is opened with, beside what names it.
    fn params(&self, request: &RunRequest) -> Value {
        match self {
            Self::Coding => {
                let mut params = json!({ "toolExecutionMode": "direct_only" });
                let servers: Vec<_> = request
                    .mcp
                    .iter()
                    .chain(&request.extra_mcp)
                    .map(|server| {
                        json!({
                            "name": server.name, "command": server.command,
                            "args": server.args, "env": server.env,
                        })
                    })
                    .collect();
                if !servers.is_empty() {
                    params["mcpServers"] = json!(servers);
                }
                params
            }
            Self::Title { .. } => json!({
                "systemPrompt": crate::TITLE_INSTRUCTIONS,
                "disableBuiltinSkills": true,
                "autoRejectPermissionRequests": true,
                "privacyLevel": "private",
                "restrictToolIds": ["__no-tools__"],
                "autonomyLevel": "off",
                "interactionMode": "auto",
                "blockOnStartupLoad": true,
                "blockOnMcpLoad": true,
            }),
        }
    }

    /// The parameters an opened session reports in its settings: one that
    /// reports another value was not opened as asked.
    fn reported(&self) -> &'static [&'static str] {
        match self {
            Self::Coding => &["toolExecutionMode"],
            Self::Title { .. } => &["systemPrompt", "autonomyLevel", "interactionMode"],
        }
    }
}

struct Host {
    child: Child,
    client: RpcClient,
    incoming: mpsc::Receiver<Incoming>,
    tail: crate::StderrTail,
    stderr: tokio::task::JoinHandle<()>,
}

impl Host {
    fn spawn(exe: &Path, cwd: &Path, env: &BTreeMap<String, String>) -> Result<Self, HarnessError> {
        let mut command = Command::new(exe);
        crate::process::owned::configure(&mut command);
        crate::compose_child_path(&mut command, exe);
        crate::apply_run_env(&mut command, env);
        command
            .args([
                "exec",
                "--input-format",
                "stream-jsonrpc",
                "--output-format",
                "stream-jsonrpc",
            ])
            .env("FACTORY_DROID_AUTO_UPDATE_ENABLED", "false")
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = Child::new(command.spawn().map_err(|error| {
            crate::executable::spawn_error(error, Some(cwd), || crate::executable::binary_hint(exe))
        })?);
        let tail = crate::StderrTail::default();
        let capture = tail.clone();
        let mut lines = crate::lossy_lines::lossy_lines(child.stderr.take().expect("piped stderr"));
        let stderr = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                capture.push(&line);
            }
            capture.close();
        });
        let (client, incoming) = RpcClient::factory(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
        );
        Ok(Self {
            child,
            client,
            incoming,
            tail,
            stderr,
        })
    }

    async fn next(&mut self) -> Result<Incoming, HarnessError> {
        tokio::select! {
            biased;
            frame = self.incoming.recv() => match frame {
                None | Some(Incoming::Eof) => {
                    self.tail.wait_closed().await;
                    let detail = self
                        .tail
                        .snapshot()
                        .map(|tail| format!(": {tail}"))
                        .unwrap_or_default();
                    Err(HarnessError::Protocol(format!("Droid stdout closed{detail}")))
                }
                Some(frame) => Ok(frame),
            },
            status = self.child.wait() => Err(HarnessError::Protocol(
                crate::crash_message("Droid", status.ok(), &self.tail),
            )),
        }
    }

    /// A request made before a session's first turn: there is nobody to ask,
    /// so server requests are refused, and no turn for a notification.
    async fn call(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, HarnessError> {
        let mut response = self.client.request_now(method, params);
        tokio::time::timeout(timeout, async {
            loop {
                tokio::select! {
                    result = &mut response => return result,
                    frame = self.next() => {
                        if let Incoming::Request { id, method, .. } = frame? {
                            dialogs::refuse(&self.client, &id, &method);
                        }
                    }
                }
            }
        })
        .await
        .map_err(|_| HarnessError::Protocol(format!("{method} timed out")))?
    }

    async fn shutdown(&mut self, grace: Duration) {
        self.client.close_stdin();
        self.child.shutdown(grace).await;
        self.stderr.abort();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stderr.abort();
    }
}

/// Loads the request's saved session, or starts a fresh one, as `launch`
/// opens it: its id and the settings in effect.
async fn open(
    host: &mut Host,
    launch: &Launch,
    request: &RunRequest,
    tx: &mpsc::Sender<Output>,
) -> Result<(String, Value), HarnessError> {
    let params = launch.params(request);
    let opening = |mut session: Value| {
        for (key, value) in params.as_object().into_iter().flatten() {
            session[key] = value.clone();
        }
        session
    };
    let loaded = match &request.resume {
        Some(id) => {
            let load = opening(json!({ "sessionId": id }));
            match host.call("droid.load_session", load, START_TIMEOUT).await {
                // A loaded session answers without its id: it is the one asked for.
                Ok(session) => Some((id.clone(), session)),
                Err(HarnessError::Protocol(message))
                    if message.contains("Session not found (code -32004)") =>
                {
                    let text = "Droid started without the previous context because the saved \
                                session was not found.\n\n";
                    let _ = tx
                        .send(Output::Event(AgentEvent::TextDelta { text: text.into() }))
                        .await;
                    None
                }
                Err(error) => return Err(error),
            }
        }
        None => None,
    };
    let (id, session) = match loaded {
        Some(loaded) => loaded,
        None => {
            let start = opening(json!({ "machineId": "default", "cwd": request.cwd }));
            let session = host
                .call("droid.initialize_session", start, START_TIMEOUT)
                .await?;
            let id = string(&session, "sessionId").to_owned();
            if id.is_empty() {
                return Err(HarnessError::Protocol(
                    "Droid initialize_session returned no session id".into(),
                ));
            }
            (id, session)
        }
    };
    if session["cwd"]
        .as_str()
        .is_some_and(|cwd| Path::new(cwd) != Path::new(&request.cwd))
    {
        return Err(HarnessError::Protocol(
            "Droid restored a different working directory; refusing to prompt".into(),
        ));
    }
    let settings = &session["settings"];
    for key in launch.reported() {
        if settings[*key] != params[*key] {
            return Err(HarnessError::Protocol(format!(
                "Droid opened the session without the requested {key}; refusing to prompt"
            )));
        }
    }
    Ok((id, settings.clone()))
}

/// The `Done` of a run that failed.
fn failed(error: &HarnessError, session_id: Option<String>) -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Errored,
        result: None,
        error: Some(normalize::sign_in_error(&error.to_string())),
        session_id,
    }
}

enum Output {
    Event(AgentEvent),
    // Run the input callback only on the consumer's next poll. The preceding
    // plan TextDelta has then passed through the engine's event fold.
    Input {
        questions: Vec<UserInputQuestion>,
        reply: oneshot::Sender<oneshot::Receiver<Vec<UserInputAnswer>>>,
    },
}

/// A prompted turn, from its first message to `agent_turn_completed`.
#[derive(Default)]
struct Turn {
    /// The id of its first message once Droid has consumed it: the id its
    /// completion carries.
    id: Option<String>,
    /// The last error Droid reported, repeated on `Done`.
    error: Option<String>,
    /// Whether assistant text has streamed, so a plan starts a paragraph.
    text: bool,
    /// When the turn must have completed, once it has been asked to stop.
    stop_by: Option<Instant>,
}

struct Runner<'a> {
    host: &'a mut Host,
    launch: Launch,
    tx: mpsc::Sender<Output>,
    request: RunRequest,
    control: crate::TurnControl,
    session_id: String,
    /// The settings Droid last reported in effect.
    settings: Value,
    /// The assistant message this turn's output belongs to.
    assistant: String,
    turn: Option<Turn>,
    /// Turns already completed: a repeated completion must not end the next.
    completed: HashSet<String>,
    normalizer: normalize::Normalizer,
    acknowledgements: FuturesUnordered<BoxFuture<'static, Result<Value, HarnessError>>>,
    dialogs: dialogs::Dialogs,
    /// Follow-up messages Droid has not consumed yet, by request id.
    deliveries: HashSet<String>,
    children: subagents::Subagents,
}

impl<'a> Runner<'a> {
    fn new(
        host: &'a mut Host,
        launch: Launch,
        tx: mpsc::Sender<Output>,
        request: RunRequest,
        control: crate::TurnControl,
        session_id: String,
        settings: Value,
    ) -> Self {
        Self {
            children: subagents::Subagents::new(&request.env, &request.cwd, &session_id),
            host,
            launch,
            tx,
            request,
            control,
            session_id,
            settings,
            assistant: uuid::Uuid::new_v4().to_string(),
            turn: None,
            completed: HashSet::new(),
            normalizer: Default::default(),
            acknowledgements: FuturesUnordered::new(),
            dialogs: Default::default(),
            deliveries: HashSet::new(),
        }
    }

    async fn emit(&mut self, event: AgentEvent) {
        let _ = self.tx.send(Output::Event(event)).await;
    }

    async fn run(
        &mut self,
        steering: &mut mpsc::Receiver<SteerMessage>,
    ) -> Result<(), HarnessError> {
        self.configure().await?;
        if self.launch.is_title() {
            self.verify_no_tools().await?;
        }
        self.emit(AgentEvent::SessionStarted {
            harness: HarnessId::Droid,
            model: string(&self.settings, "modelId").into(),
            tools: vec![],
            cwd: self.request.cwd.clone(),
            session_id: self.session_id.clone(),
            assistant_message_id: self.assistant.clone(),
        })
        .await;
        self.turn = Some(Turn::default());
        self.send_message(
            self.request.prompt.clone(),
            self.request.attachments.clone(),
        );
        let mut queued = VecDeque::new();
        // A title is its one turn: nothing steers it or follows it.
        let mut mailbox_open = !self.launch.is_title();
        let mut child_poll = tokio::time::interval(subagents::POLL);
        child_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.turn.is_none() {
                if let Some(message) = queued.pop_front() {
                    self.prompt(message).await?;
                } else if !mailbox_open && self.children.pending() == 0 {
                    return Ok(());
                }
            }
            let stop_by = self.turn.as_ref().and_then(|turn| turn.stop_by);
            tokio::select! {
                biased;
                _ = child_poll.tick(), if self.children.pending() > 0 => {
                    let events = self.children.poll();
                    self.child_events(events).await;
                }
                frame = self.host.next() => match frame? {
                    Incoming::Notification { method, params } if method == SESSION_NOTIFICATION => {
                        self.notification(params).await;
                    }
                    Incoming::Request { id, method, params } => {
                        self.server_request(id, &method, &params).await;
                    }
                    _ => {}
                },
                _ = self.control.stop_requested() => self.stop(),
                ack = self.acknowledgements.next(), if !self.acknowledgements.is_empty() => {
                    if let Some(ack) = ack {
                        ack?;
                    }
                }
                answer = self.dialogs.next(), if self.dialogs.has_pending() => {
                    if let Some((key, result)) = answer {
                        self.dialogs.answer(&self.host.client, &key, result);
                    }
                }
                message = steering.recv(), if mailbox_open => match message {
                    Some(message) if self.turn.is_none() => self.prompt(message).await?,
                    // A stopped turn takes nothing more, new settings wait for
                    // the idle barrier, and nothing overtakes a waiting message.
                    Some(message)
                        if stop_by.is_some() || message.config.is_some() || !queued.is_empty() =>
                    {
                        queued.push_back(message);
                    }
                    Some(message) => self.follow_up(message.prompt, message.attachments),
                    None => mailbox_open = false,
                },
                _ = overdue(stop_by) => {
                    return Err(HarnessError::Protocol(format!(
                        "Droid did not complete its interrupted turn within {} seconds",
                        STOP_TIMEOUT.as_secs()
                    )));
                }
            }
        }
    }

    /// Holds a request's response: a refusal, or none in time, ends the run.
    fn track(&self, response: BoxFuture<'static, Result<Value, HarnessError>>) {
        self.acknowledgements.push(Box::pin(async move {
            tokio::time::timeout(RPC_TIMEOUT, response)
                .await
                .map_err(|_| HarnessError::Protocol("Droid request timed out".into()))?
        }));
    }

    /// Sends a user message: the id of its request.
    fn send_message(&mut self, text: String, images: Vec<String>) -> String {
        let mut params = json!({ "text": text });
        if !images.is_empty() {
            params["imagePaths"] = json!(images);
        }
        let (id, response) = self
            .host
            .client
            .request_with_id("droid.add_user_message", params);
        self.track(response);
        id
    }

    /// Sends a message whose consumption starts the next assistant message.
    fn follow_up(&mut self, text: String, images: Vec<String>) {
        let id = self.send_message(text, images);
        self.deliveries.insert(id);
    }

    /// Starts a turn with a message that arrived while none was running,
    /// under the configuration it brings.
    async fn prompt(&mut self, message: SteerMessage) -> Result<(), HarnessError> {
        if let Some(config) = message.config {
            self.request = *config;
        }
        self.configure().await?;
        self.turn = Some(Turn::default());
        self.follow_up(message.prompt, message.attachments);
        Ok(())
    }

    fn stop(&mut self) {
        let Some(turn) = self.turn.as_mut().filter(|turn| turn.stop_by.is_none()) else {
            return;
        };
        turn.stop_by = Some(Instant::now() + STOP_TIMEOUT);
        self.dialogs.cancel(&self.host.client);
        self.track(
            self.host
                .client
                .request_now("droid.interrupt_session", json!({})),
        );
    }

    /// Asks for the request's settings and refuses to go on under any others.
    async fn configure(&mut self) -> Result<(), HarnessError> {
        let wanted = settings(&self.launch, &self.request, &self.settings)?;
        let (id, ack) = self
            .host
            .client
            .request_with_id("droid.update_session_settings", wanted.clone());
        self.track(ack);
        self.settings = tokio::time::timeout(RPC_TIMEOUT, self.settings_updated(&id))
            .await
            .map_err(|_| {
                HarnessError::Protocol("droid.update_session_settings timed out".into())
            })??;
        for (key, value) in wanted.as_object().into_iter().flatten() {
            if self.settings.get(key) != Some(value) {
                return Err(HarnessError::Protocol(format!(
                    "Droid changed requested {key}; refusing to send a prompt under different settings"
                )));
            }
        }
        Ok(())
    }

    /// A title's launch asks for no tools; only the catalog says it held.
    async fn verify_no_tools(&mut self) -> Result<(), HarnessError> {
        let catalog = self
            .host
            .call("droid.list_tools", json!({}), START_TIMEOUT)
            .await?;
        let tools = catalog["tools"]
            .as_array()
            .ok_or_else(|| HarnessError::Protocol("Droid returned no title tool catalog".into()))?;
        if tools
            .iter()
            .any(|tool| tool["currentlyAllowed"].as_bool() != Some(false))
        {
            return Err(HarnessError::Protocol(
                "Droid title tools remain enabled or their availability is unknown".into(),
            ));
        }
        Ok(())
    }

    /// The settings in effect after update `request`. Its `{}` response is
    /// only an ACK, and of the `settings_updated` notifications that follow,
    /// some of them partial, only the one carrying its id says.
    async fn settings_updated(&mut self, request: &str) -> Result<Value, HarnessError> {
        loop {
            tokio::select! {
                ack = self.acknowledgements.next(), if !self.acknowledgements.is_empty() => {
                    if let Some(ack) = ack {
                        ack?;
                    }
                }
                frame = self.host.next() => match frame? {
                    Incoming::Notification { method, params } if method == SESSION_NOTIFICATION => {
                        let notification = &params["notification"];
                        if notification["type"] == "settings_updated"
                            && notification["requestId"] == request
                        {
                            return Ok(notification["settings"].clone());
                        }
                        self.notification(params).await;
                    }
                    Incoming::Request { id, method, .. } => {
                        dialogs::refuse(&self.host.client, &id, &method);
                    }
                    _ => {}
                },
            }
        }
    }

    async fn notification(&mut self, params: Value) {
        if params["sessionId"]
            .as_str()
            .is_some_and(|id| id != self.session_id)
        {
            return;
        }
        let notification = &params["notification"];
        match string(notification, "type") {
            "child_session_available" => {
                let id = string(notification, "childSessionId");
                let tool = string(notification, "toolUseId");
                if id != self.session_id && self.normalizer.is_spawn(tool) {
                    let prompt = self
                        .normalizer
                        .spawn_prompt(tool)
                        .unwrap_or_else(|| string(notification, "description"));
                    let events = self.children.start(id, tool, prompt);
                    self.child_events(events).await;
                }
            }
            "tool_progress_update" => {
                let events = self.children.progress(&notification["update"]);
                self.child_events(events).await;
            }
            "child_session_settled" => {
                let events = self.children.settle(
                    string(notification, "childSessionId"),
                    string(notification, "status"),
                );
                self.child_events(events).await;
            }
            "create_message" => self.consumed(notification).await,
            "queued_messages_discarded" => {
                let lost = normalize::discarded(notification)
                    .into_iter()
                    .filter(|id| self.deliveries.remove(*id))
                    .count();
                if lost > 0 {
                    self.emit(AgentEvent::Error {
                        message: "Droid discarded queued messages; they were not delivered. \
                                  Send them again."
                            .into(),
                    })
                    .await;
                }
            }
            "agent_turn_completed" => self.turn_completed(notification).await,
            _ => {
                let Some(turn) = &mut self.turn else { return };
                let Some(event) = self.normalizer.event(notification) else {
                    return;
                };
                match &event {
                    AgentEvent::Error { message } => turn.error = Some(message.clone()),
                    AgentEvent::TextDelta { text } => turn.text |= !text.is_empty(),
                    _ => {}
                }
                self.emit(event).await;
            }
        }
    }

    /// `create_message` with a request id is Droid's receipt for a message
    /// it consumed. The turn's first names the turn; a follow-up's starts the
    /// next assistant message.
    async fn consumed(&mut self, notification: &Value) {
        let message = &notification["message"];
        let Some(request) = notification["requestId"]
            .as_str()
            .filter(|_| message["role"] == "user")
        else {
            return;
        };
        if let Some(turn) = self.turn.as_mut().filter(|turn| turn.id.is_none()) {
            turn.id = message["id"].as_str().map(str::to_owned);
        }
        if self.deliveries.remove(request) {
            let previous = std::mem::replace(&mut self.assistant, uuid::Uuid::new_v4().to_string());
            self.emit(AgentEvent::Steered {
                assistant_message_id: Some(previous),
                next_assistant_message_id: Some(self.assistant.clone()),
            })
            .await;
        }
    }

    async fn turn_completed(&mut self, notification: &Value) {
        let Some(turn) = &self.turn else { return };
        let id = notification["turnId"].as_str();
        if id.is_some_and(|id| {
            self.completed.contains(id) || turn.id.as_deref().is_some_and(|own| own != id)
        }) {
            return;
        }
        self.completed.extend(id.map(str::to_owned));
        let mut error = turn.error.clone();
        if notification["reason"] == "model_authentication_failed" {
            let message = normalize::authentication_error(
                error
                    .as_deref()
                    .unwrap_or("Droid model authentication failed"),
            );
            self.emit(AgentEvent::Error {
                message: message.clone(),
            })
            .await;
            error = Some(message);
        }
        self.finish(normalize::terminal(notification), error).await;
    }

    async fn child_events(&mut self, events: Vec<AgentEvent>) {
        self.control.set_background(self.children.pending());
        for event in events {
            self.emit(event).await;
        }
    }

    async fn finish(&mut self, status: DoneStatus, error: Option<String>) {
        self.turn = None;
        self.dialogs.cancel(&self.host.client);
        self.emit(AgentEvent::Done {
            status,
            result: None,
            error,
            session_id: Some(self.session_id.clone()),
        })
        .await;
    }

    /// The run is being torn down: a turn in flight ends interrupted.
    async fn interrupt(&mut self) {
        drop(
            self.host
                .client
                .request_now("droid.interrupt_session", json!({})),
        );
        if let Some(turn) = self.turn.take() {
            self.finish(DoneStatus::Interrupted, turn.error).await;
        }
    }

    /// Ends the run: what `run` left open is answered and settled, and a
    /// failure is reported.
    async fn close(&mut self, result: Result<(), HarnessError>) {
        self.dialogs.cancel(&self.host.client);
        let status = if result.is_err() {
            DoneStatus::Errored
        } else {
            DoneStatus::Interrupted
        };
        let events = self.children.close(status);
        self.child_events(events).await;
        if let Err(error) = result {
            self.turn = None;
            let done = failed(&error, Some(self.session_id.clone()));
            self.emit(done).await;
        }
    }

    async fn server_request(&mut self, id: Value, method: &str, params: &Value) {
        // Nobody is asked by a title, outside a turn, or once it is stopping.
        let asking = !self.launch.is_title()
            && self
                .turn
                .as_ref()
                .is_some_and(|turn| turn.stop_by.is_none());
        match method {
            "droid.request_permission" if asking => self.permission(id, params).await,
            "droid.ask_user" if asking => {
                let questions = dialogs::questions(params);
                if questions.is_empty() {
                    self.host.client.respond(&id, dialogs::cancel_questions());
                } else {
                    let native = questions.clone();
                    self.dialogs
                        .ask(
                            &self.tx,
                            id,
                            questions,
                            move |answers| dialogs::question_response(&native, &answers),
                            dialogs::cancel_questions(),
                        )
                        .await;
                }
            }
            _ => dialogs::refuse(&self.host.client, &id, method),
        }
    }

    /// A plan leaving Spec mode is shown as the turn's text and approved by
    /// the person at every access level; any other tool use is asked about
    /// only where the access mode does not cover it.
    async fn permission(&mut self, id: Value, params: &Value) {
        let access = dialogs::access(&self.request);
        let (question, option) = match dialogs::spec(params) {
            Some(spec) => {
                let Some(plan) = dialogs::plan(spec) else {
                    let message = "Droid supplied an empty or oversized plan; approval cancelled";
                    return self.refuse_permission(&id, message.into()).await;
                };
                self.plan(plan).await;
                let question = crate::approval::question(
                    "Approve plan",
                    "Droid wants to start implementing this plan. Allow it?",
                );
                (Some(question), dialogs::proceed(access))
            }
            None => (dialogs::permission_question(params, access), "proceed_once"),
        };
        if !dialogs::offered(params, option) {
            let message =
                format!("Droid did not offer {option}; approval cancelled without changing access");
            return self.refuse_permission(&id, message).await;
        }
        let proceed = json!({ "selectedOption": option });
        let Some(question) = question else {
            return self.host.client.respond(&id, proceed);
        };
        let asked = question.clone();
        self.dialogs
            .ask(
                &self.tx,
                id,
                vec![question],
                move |answers| {
                    if crate::approval::approved(&asked, &answers) {
                        proceed
                    } else {
                        dialogs::cancel_permission()
                    }
                },
                dialogs::cancel_permission(),
            )
            .await;
    }

    async fn refuse_permission(&mut self, id: &Value, message: String) {
        self.host.client.respond(id, dialogs::cancel_permission());
        self.emit(AgentEvent::Error { message }).await;
    }

    async fn plan(&mut self, plan: String) {
        let Some(turn) = &mut self.turn else { return };
        let text = if turn.text {
            format!("\n\n{plan}")
        } else {
            plan
        };
        turn.text = true;
        self.emit(AgentEvent::TextDelta { text }).await;
    }
}

/// Resolves once a turn asked to stop has outlived `stop_by`: never for one
/// that was not asked.
async fn overdue(stop_by: Option<Instant>) {
    match stop_by {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn model<'a>(request: &'a RunRequest, effective: &'a Value) -> Result<&'a str, HarnessError> {
    let model = request
        .model
        .as_deref()
        .filter(|model| *model != "default")
        .unwrap_or_else(|| string(effective, "modelId"));
    if model.is_empty() {
        return Err(HarnessError::Protocol(
            "Droid returned no default model".into(),
        ));
    }
    Ok(model)
}

fn settings(
    launch: &Launch,
    request: &RunRequest,
    effective: &Value,
) -> Result<Value, HarnessError> {
    let model = model(request, effective)?;
    let current = &effective["reasoningEffort"];
    let effort = match launch {
        _ if model == "auto" => json!("none"),
        Launch::Coding => request
            .reasoning
            .map_or_else(|| current.clone(), |level| json!(level)),
        Launch::Title { models } => {
            catalog::title_effort(model, request.reasoning, current, models)
        }
    };
    Ok(json!({
        "modelId": model, "specModeModelId": model,
        "reasoningEffort": effort, "specModeReasoningEffort": effort,
        "autonomyLevel": dialogs::autonomy(dialogs::access(request)),
        "interactionMode": if request.plan { "spec" } else { "auto" },
    }))
}
