//! One omp session over one RPC process: bootstrap it, run its turns, and
//! carry Zeron's steers, stops and answers in. See PROTOCOL.md for the
//! contract each step relies on.
//!
//! - TURNS. Each prompt is a `prompt` command whose id is its ticket; omp
//!   owes exactly one `prompt_result` per accepted prompt (unless the ack
//!   says `agentInvoked: false`: a local slash command, done at once). A
//!   turn ends when its last ticket is reported, after a `get_state` barrier
//!   that refreshes the session id the `Done` carries. A run omp starts on
//!   its own (an async job's result waking it) is a wake turn: it ends at
//!   its terminal `agent_end`.
//! - STEERING. A later prompt is `prompt` with `streamingBehavior: "steer"`:
//!   one command that joins the running run at its next step, or starts a
//!   run when idle. It is confirmed (`Steered`) when its user message enters
//!   the conversation.
//! - STOP. `abort_and_restore_queue` withdraws queued input and aborts the
//!   run; the turn ends at the tickets' `aborted` results. The process (and
//!   its background jobs) lives on.
use super::{
    catalog,
    dialogs::Dialogs,
    host_tools::HostTools,
    normalize::{Normalizer, string},
    wire::{Process, response_data},
};
use crate::{HarnessError, SteerMessage, TurnControl};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::Duration,
};
use tokio::sync::mpsc;
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest};

/// Startup budget: extensions and MCP discovery can start cold.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
/// Bound on applying a model or thinking change mid-session.
const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound on the turn-end `get_state` barrier: omp answers at once, so
/// silence past this is a wedged process (the turn ends as it stands).
const STATE_TIMEOUT: Duration = Duration::from_secs(15);
/// After omp confirms an abort, how long its prompts' results may take.
/// omp reports them right after the run's terminal `agent_end`; this only
/// keeps a stop from leaving a turn Working if that ever fails.
const ABORT_SETTLE: Duration = Duration::from_secs(15);

enum Pending {
    Prompt,
    Abort,
    /// The turn-end barrier.
    Closing,
}

struct Turn {
    /// Prompts whose `prompt_result` is owed.
    tickets: HashSet<String>,
    /// The last reported outcome.
    outcome: Option<(DoneStatus, Option<String>)>,
    /// Started by omp itself: ends at its terminal `agent_end`.
    unprompted: bool,
    /// Every ticket is in; the barrier is out.
    closing: bool,
}

impl Turn {
    fn new(unprompted: bool) -> Self {
        Self {
            tickets: HashSet::new(),
            outcome: None,
            unprompted,
            closing: false,
        }
    }
}

pub(super) struct Runner {
    pub process: Process,
    pub tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    request: RunRequest,
    session: String,
    model: String,
    assistant: String,
    norm: Normalizer,
    turn: Option<Turn>,
    pending: HashMap<String, Pending>,
    /// Steers (their tickets) accepted, not yet seen entering the
    /// conversation, in the order omp will take them.
    deliveries: VecDeque<String>,
    /// The opening prompt's own user message is not a delivery.
    initial_message: bool,
    interrupting: bool,
    pub error: Option<String>,
    host_tools: HostTools,
    pub control: TurnControl,
    pub lost_context: Option<String>,
    abort_deadline: Option<tokio::time::Instant>,
    state_deadline: Option<tokio::time::Instant>,
    /// The command list last announced (omp re-sends it unchanged).
    commands: Option<Vec<zeron_proto::SlashCommand>>,
    /// A run is live: `agent_start` seen, its yielding `agent_end` not yet.
    running: bool,
}

impl Runner {
    pub fn new(
        process: Process,
        tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
        request: RunRequest,
    ) -> Self {
        Self {
            process,
            tx,
            request,
            session: String::new(),
            model: "default".into(),
            assistant: uuid::Uuid::new_v4().to_string(),
            norm: Normalizer::default(),
            // The opening prompt's turn: a failed startup ends it.
            turn: Some(Turn::new(false)),
            pending: HashMap::new(),
            deliveries: VecDeque::new(),
            initial_message: true,
            interrupting: false,
            error: None,
            host_tools: HostTools::default(),
            control: TurnControl::default(),
            lost_context: None,
            abort_deadline: None,
            state_deadline: None,
            commands: None,
            running: false,
        }
    }

    async fn emit(&self, event: AgentEvent) -> Result<(), HarnessError> {
        self.tx
            .send(Ok(event))
            .await
            .map_err(|_| HarnessError::Protocol("omp event consumer closed".into()))
    }

    async fn started(&self) -> Result<(), HarnessError> {
        self.emit(AgentEvent::SessionStarted {
            harness: HarnessId::Omp,
            model: self.model.clone(),
            tools: vec![],
            cwd: self.request.cwd.clone(),
            session_id: self.session.clone(),
            assistant_message_id: self.assistant.clone(),
        })
        .await
    }

    /// The run's last word for an unfinished turn (teardown, crash).
    pub async fn finish(&mut self) -> Result<(), HarnessError> {
        if self.turn.is_none() {
            return Ok(());
        }
        let status = if self.error.is_some() {
            DoneStatus::Errored
        } else {
            DoneStatus::Interrupted
        };
        let error = self.error.clone();
        self.end(status, error).await
    }

    /// Bootstrap the process, then run the opening prompt and every steer
    /// until steering closes and the last turn has ended.
    pub async fn run(
        &mut self,
        steering: &mut mpsc::Receiver<SteerMessage>,
    ) -> Result<(), HarnessError> {
        let mut backlog = Vec::new();
        let state = tokio::time::timeout(STARTUP_TIMEOUT, self.bootstrap(&mut backlog))
            .await
            .map_err(|_| HarnessError::Protocol("omp startup timed out".into()))??;
        self.session = string(&state, "sessionId").to_owned();
        if self.session.is_empty() {
            return Err(HarnessError::Protocol(
                "omp get_state omitted sessionId".into(),
            ));
        }
        if self
            .request
            .resume
            .as_ref()
            .is_some_and(|id| *id != self.session)
        {
            return Err(HarnessError::Protocol(
                "omp resumed a different session".into(),
            ));
        }
        self.observe_state(&state);
        self.started().await?;
        if let Some(message) = self.lost_context.take() {
            self.emit(AgentEvent::Error { message }).await?;
        }
        for frame in backlog {
            self.frame(frame).await?;
        }
        let prompt = self.request.prompt.clone();
        let attachments = self.request.attachments.clone();
        self.submit(&prompt, &attachments, false, false).await?;
        let mut open = true;
        loop {
            if !open && self.turn.is_none() {
                return Ok(());
            }
            let abort_deadline = self.abort_deadline;
            let state_deadline = self.state_deadline;
            tokio::select! {
                frame = self.process.next() => self.frame(frame?).await?,
                steer = steering.recv(), if open => match steer {
                    Some(steer) => self.steer(steer).await?,
                    None => open = false,
                },
                _ = self.control.stop_requested() => self.stop(),
                _ = sleep_until(abort_deadline), if abort_deadline.is_some() => {
                    tracing::warn!(target: "zeron_harness::omp", "omp confirmed the abort but reported no prompt result; ending the turn");
                    self.end(DoneStatus::Interrupted, None).await?;
                }
                _ = sleep_until(state_deadline), if state_deadline.is_some() => {
                    tracing::warn!(target: "zeron_harness::omp", "omp did not answer the turn-end get_state; ending the turn as it stands");
                    self.close().await?;
                }
            }
        }
    }

    async fn bootstrap(&mut self, backlog: &mut Vec<Value>) -> Result<Value, HarnessError> {
        self.process.handshake(backlog).await?;
        // Deltas only: the full projection re-sends the whole message on
        // every token. The rest are this session's own settings (omp does
        // not persist them): approvals and questions come to this host,
        // pending steers land together at the next step, and a steer never
        // interrupts a running tool.
        for (command, what) in [
            (
                json!({"type":"set_event_filter","events":null,"messageUpdates":"delta"}),
                "delta updates",
            ),
            (
                json!({"type":"set_ask_dialog","enabled":true}),
                "ask dialogs",
            ),
            (
                json!({"type":"set_steering_mode","mode":"all"}),
                "steering mode",
            ),
            (
                json!({"type":"set_interrupt_mode","mode":"wait"}),
                "interrupt mode",
            ),
        ] {
            if let Err(error) = self.process.query(command, backlog).await {
                tracing::warn!(target: "zeron_harness::omp", %error, "omp kept its default {what}");
            }
        }
        let (tools, definitions) = HostTools::start(
            self.request.mcp.as_ref(),
            &self.request.extra_mcp,
            &self.request.env,
        )
        .await;
        if !definitions.is_empty() {
            match self
                .process
                .query(
                    json!({"type":"set_host_tools","tools":definitions}),
                    backlog,
                )
                .await
            {
                Ok(_) => {
                    self.norm.host_tools = tools.names();
                    self.host_tools = tools;
                }
                Err(error) => {
                    tracing::warn!(target: "zeron_harness::omp", %error, "omp refused Zeron's tools")
                }
            }
        }
        let request = self.request.clone();
        self.configure(&request, backlog, true).await?;
        // Announced once the session is (see `run`).
        let commands = self
            .process
            .query(json!({"type":"get_available_commands"}), backlog)
            .await?;
        backlog.insert(
            0,
            json!({"type":"available_commands_update","commands":commands["commands"]}),
        );
        self.process
            .query(json!({"type":"get_state"}), backlog)
            .await
    }

    /// Select `request`'s model and thinking level on the live session. At
    /// startup a model omp can't select fails the run; mid-session the
    /// session keeps its model and says so.
    async fn configure(
        &mut self,
        request: &RunRequest,
        backlog: &mut Vec<Value>,
        startup: bool,
    ) -> Result<(), HarnessError> {
        if let Some(model) = selected_model(request) {
            let (provider, id) = match model.split_once('/') {
                Some((provider, id)) => (provider.to_owned(), id.to_owned()),
                None => {
                    // A bare id: the one model of that id omp offers.
                    let available = self
                        .process
                        .query(json!({"type":"get_available_models"}), backlog)
                        .await?;
                    let mut matches = available["models"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|m| string(m, "id") == model);
                    match (matches.next(), matches.next()) {
                        (Some(m), None) => (string(m, "provider").to_owned(), model.to_owned()),
                        _ => (String::new(), model.to_owned()),
                    }
                }
            };
            let result = self
                .process
                .query(
                    json!({"type":"set_model","provider":provider,"modelId":id}),
                    backlog,
                )
                .await;
            match result {
                Ok(_) => {}
                Err(error) if startup => {
                    return Err(HarnessError::Protocol(format!(
                        "omp cannot use model {model}: {error}"
                    )));
                }
                Err(error) => {
                    self.emit(AgentEvent::Error {
                        message: format!("omp kept its model: {error}"),
                    })
                    .await?;
                }
            }
        }
        if catalog::thinking_selected(request) {
            let supported = self
                .process
                .query(json!({"type":"get_available_thinking_levels"}), backlog)
                .await?;
            if let Some(level) = catalog::thinking_level(request, &supported) {
                self.process
                    .query(json!({"type":"set_thinking_level","level":level}), backlog)
                    .await?;
            }
        }
        Ok(())
    }

    fn observe_state(&mut self, state: &Value) {
        let model = &state["model"];
        if !string(model, "id").is_empty() {
            self.model = format!("{}/{}", string(model, "provider"), string(model, "id"));
        }
        self.norm.window = model["contextWindow"]
            .as_u64()
            .or_else(|| state["contextUsage"]["contextWindow"].as_u64())
            .or(self.norm.window);
        if state["hasPendingAsyncWork"] == true {
            self.publish_background(true);
        } else if state["isSettled"] == true {
            self.publish_background(false);
        }
    }

    /// omp's async jobs (background bash, tasks) keep the runtime: retiring
    /// it would kill them. When one finishes, omp wakes the agent itself.
    fn publish_background(&self, pending: bool) {
        self.control.set_background(usize::from(pending));
        self.control.set_wakes_on_background(true);
    }

    /// Send a prompt; it joins the turn as a ticket (a steer's with a
    /// delivery to confirm).
    async fn submit(
        &mut self,
        text: &str,
        attachments: &[String],
        steer: bool,
        delivery: bool,
    ) -> Result<(), HarnessError> {
        let mut command = json!({"type":"prompt","message":text});
        let images = crate::pi::load_images(attachments).await;
        if images.as_array().is_some_and(|i| !i.is_empty()) {
            command["images"] = images;
        }
        if steer {
            command["streamingBehavior"] = json!("steer");
        }
        let id = self.process.client.request(command)?;
        self.pending.insert(id.clone(), Pending::Prompt);
        if delivery {
            self.deliveries.push_back(id.clone());
        }
        if let Some(turn) = self.turn.as_mut() {
            turn.tickets.insert(id);
            turn.closing = false;
        }
        Ok(())
    }

    async fn steer(&mut self, steer: SteerMessage) -> Result<(), HarnessError> {
        // A changed model or thinking level applies before the message it
        // came with (omp runs commands in order).
        if let Some(next) = steer.config {
            let mut backlog = vec![];
            tokio::time::timeout(
                CONFIGURE_TIMEOUT,
                self.configure(&next, &mut backlog, false),
            )
            .await
            .map_err(|_| {
                HarnessError::Protocol(format!(
                    "omp did not apply the model change within {}s",
                    CONFIGURE_TIMEOUT.as_secs()
                ))
            })??;
            self.request.model = next.model.clone();
            self.request.reasoning = next.reasoning;
            self.request.model_options = next.model_options.clone();
            for frame in backlog {
                self.frame(frame).await?;
            }
        }
        if self.turn.is_none() {
            // Idle: this prompt opens the next turn.
            self.begin(false);
        }
        self.submit(&steer.prompt, &steer.attachments, true, true)
            .await
    }

    fn begin(&mut self, unprompted: bool) {
        self.turn = Some(Turn::new(unprompted));
        self.interrupting = false;
        self.abort_deadline = None;
        self.norm.reset();
    }

    /// End the turn in place: withdraw queued input, abort the run. The
    /// tickets' `aborted` results end it for real.
    fn stop(&mut self) {
        if self.turn.is_none() || self.interrupting {
            return;
        }
        self.interrupting = true;
        self.process.dialogs.cancel(&self.process.client);
        if let Ok(id) = self
            .process
            .client
            .request(json!({"type":"abort_and_restore_queue"}))
        {
            self.pending.insert(id, Pending::Abort);
        }
    }

    /// Runtime teardown: stop the turn and give omp `grace` to report it.
    pub async fn interrupt(&mut self, grace: Duration) {
        self.stop();
        // Nothing sent and no run live (startup, a closing barrier): there
        // is no result to wait for.
        if !self.running && self.turn.as_ref().is_some_and(|t| t.tickets.is_empty()) {
            return;
        }
        let _ = tokio::time::timeout(grace, async {
            while self.turn.is_some() {
                let Ok(frame) = self.process.next().await else {
                    break;
                };
                if self.frame(frame).await.is_err() {
                    break;
                }
            }
        })
        .await;
    }

    /// The oldest accepted steer entered the conversation.
    async fn confirm_delivery(&mut self) -> Result<(), HarnessError> {
        if self.deliveries.pop_front().is_some() {
            self.steered().await?;
        }
        Ok(())
    }

    async fn steered(&mut self) -> Result<(), HarnessError> {
        let old = std::mem::replace(&mut self.assistant, uuid::Uuid::new_v4().to_string());
        self.emit(AgentEvent::Steered {
            assistant_message_id: Some(old),
            next_assistant_message_id: Some(self.assistant.clone()),
        })
        .await
    }

    async fn frame(&mut self, frame: Value) -> Result<(), HarnessError> {
        match string(&frame, "type") {
            "response" => return self.response(&frame).await,
            "prompt_result" => return self.prompt_result(&frame).await,
            "agent_start" => {
                self.running = true;
                if self.turn.as_ref().is_some_and(|t| t.closing) {
                    // The prompt's turn is over; this run is omp's own.
                    self.close().await?;
                }
                if self.turn.is_none() {
                    // A wake: omp resumed on its own (background work ended).
                    self.control.note_wake();
                    self.begin(true);
                    self.assistant = uuid::Uuid::new_v4().to_string();
                    self.started().await?;
                }
            }
            "message_start" if frame["message"]["role"] == "user" => {
                // Only what the person sent: extensions and agents attribute
                // the messages they inject otherwise.
                let attribution = frame["message"]["attribution"].as_str().unwrap_or("user");
                if attribution == "user" && self.turn.is_some() {
                    if self.initial_message {
                        self.initial_message = false;
                    } else {
                        self.confirm_delivery().await?;
                    }
                }
            }
            "extension_ui_request" => {
                if !Dialogs::interactive(&frame) {
                    if self.turn.is_some()
                        && frame["method"] == "notify"
                        && matches!(frame["notifyType"].as_str(), Some("warning" | "error"))
                    {
                        self.emit(AgentEvent::Error {
                            message: format!("omp: {}", string(&frame, "message")),
                        })
                        .await?;
                    }
                    return Ok(());
                }
                if self.turn.is_some() {
                    self.process
                        .dialogs
                        .request(self.process.client.clone(), &frame);
                } else {
                    // Outside a turn nobody waits on an answer: decline it
                    // rather than reopen a finished turn.
                    Dialogs::default().request(self.process.client.clone(), &frame);
                }
                return Ok(());
            }
            "host_tool_call" => {
                self.host_tools.call(self.process.client.clone(), &frame);
                return Ok(());
            }
            "host_tool_cancel" => {
                self.host_tools.cancel(&frame);
                return Ok(());
            }
            "available_commands_update" => {
                let commands = catalog::commands(&frame);
                if self.commands.as_ref() == Some(&commands) {
                    return Ok(());
                }
                self.commands = Some(commands.clone());
                return self.emit(AgentEvent::AvailableCommands { commands }).await;
            }
            "command_output" => {
                // A local command answers while omp dispatches it, before
                // its ack: a steer carrying it is delivered now, so the
                // output lands in its own reply.
                let dispatching = self
                    .deliveries
                    .iter()
                    .position(|ticket| matches!(self.pending.get(ticket), Some(Pending::Prompt)));
                if let Some(at) = dispatching {
                    self.deliveries.remove(at);
                    self.steered().await?;
                }
                if self.turn.is_some() {
                    let text = string(&frame, "text");
                    if !text.is_empty() {
                        self.emit(AgentEvent::TextDelta {
                            text: format!("{text}\n"),
                        })
                        .await?;
                    }
                }
                return Ok(());
            }
            "session_settled" => {
                self.publish_background(false);
                return Ok(());
            }
            "rpc_frame_error" => {
                tracing::warn!(target: "zeron_harness::omp", error = %string(&frame, "error"), "omp dropped a frame too large for the transport");
                return Ok(());
            }
            _ => {}
        }
        if self.turn.is_some() {
            for event in self.norm.map(&frame) {
                self.emit(event).await?;
            }
        }
        // A wake turn ends where its run yields (older omp: terminal end).
        let yielded = frame["type"] == "agent_end"
            && frame["yielded"]
                .as_bool()
                .unwrap_or(frame["isTerminal"] != false);
        if yielded {
            self.running = false;
        }
        if yielded
            && self
                .turn
                .as_ref()
                .is_some_and(|t| t.unprompted && t.tickets.is_empty() && !t.closing)
        {
            let outcome = self.norm.status();
            if let Some(turn) = self.turn.as_mut() {
                turn.outcome = Some(outcome);
            }
            self.closing()?;
        }
        Ok(())
    }

    async fn response(&mut self, frame: &Value) -> Result<(), HarnessError> {
        let Some(id) = frame["id"].as_str().map(str::to_owned) else {
            // A command omp could not attribute (unparseable, unknown).
            tracing::warn!(target: "zeron_harness::omp", response = %frame, "uncorrelated omp response");
            return Ok(());
        };
        let Some(pending) = self.pending.remove(&id) else {
            return Ok(());
        };
        match pending {
            Pending::Prompt => match response_data(frame) {
                Ok(data) if data["agentInvoked"] == false => {
                    // Handled locally (a slash command): nothing more comes.
                    self.ticket_done(&id, (DoneStatus::Completed, None), true)
                        .await?;
                }
                Ok(_) => {}
                Err(error) => {
                    let message = format!("omp could not take the message: {error}");
                    let last = self
                        .turn
                        .as_ref()
                        .is_some_and(|t| t.tickets.len() == 1 && t.tickets.contains(&id));
                    if !last {
                        self.emit(AgentEvent::Error {
                            message: message.clone(),
                        })
                        .await?;
                    }
                    self.ticket_done(&id, (DoneStatus::Errored, Some(message)), false)
                        .await?;
                }
            },
            Pending::Abort => {
                if let Err(error) = response_data(frame) {
                    tracing::warn!(target: "zeron_harness::omp", %error, "omp refused the abort");
                }
                // The withdrawn steers will never enter the conversation.
                self.deliveries.clear();
                if self.turn.as_ref().is_some_and(|t| !t.closing) {
                    self.abort_deadline = Some(tokio::time::Instant::now() + ABORT_SETTLE);
                }
            }
            Pending::Closing => {
                self.state_deadline = None;
                match response_data(frame) {
                    Ok(state) => self.refresh(&state).await?,
                    Err(error) => {
                        tracing::warn!(target: "zeron_harness::omp", %error, "omp turn-end get_state failed")
                    }
                }
                if self.turn.as_ref().is_some_and(|t| t.closing) {
                    self.close().await?;
                }
            }
        }
        Ok(())
    }

    async fn prompt_result(&mut self, frame: &Value) -> Result<(), HarnessError> {
        let Some(id) = frame["id"].as_str().map(str::to_owned) else {
            return Ok(());
        };
        let outcome = match string(frame, "status") {
            "aborted" => (DoneStatus::Interrupted, None),
            "error" => (
                DoneStatus::Errored,
                Some(
                    frame["error"]["message"]
                        .as_str()
                        .filter(|m| !m.is_empty())
                        .unwrap_or("omp provider request failed")
                        .to_owned(),
                ),
            ),
            _ => (DoneStatus::Completed, None),
        };
        if frame["sessionSettled"] == false {
            self.publish_background(true);
        }
        self.ticket_done(&id, outcome, false).await
    }

    /// A ticket is in: its outcome becomes the turn's, and the turn closes
    /// when it was the last. `local`: answered without a model turn, so a
    /// steer it carried is delivered now.
    async fn ticket_done(
        &mut self,
        id: &str,
        outcome: (DoneStatus, Option<String>),
        local: bool,
    ) -> Result<(), HarnessError> {
        let Some(turn) = self.turn.as_mut() else {
            return Ok(());
        };
        if !turn.tickets.remove(id) {
            return Ok(());
        }
        let completed = outcome.0 == DoneStatus::Completed;
        turn.outcome = Some(outcome);
        let last = turn.tickets.is_empty();
        if let Some(at) = self.deliveries.iter().position(|d| d == id) {
            self.deliveries.remove(at);
            // A steer answered locally was delivered; one refused or
            // aborted before it entered the conversation never will be.
            if local {
                self.steered().await?;
            } else if completed {
                // Done without its user message seen: delivered all the same.
                self.steered().await?;
            }
        }
        if local {
            self.initial_message = false;
        }
        if last && self.running {
            // A run is still live (one omp started meanwhile): the turn
            // ends where it yields.
            if let Some(turn) = self.turn.as_mut() {
                turn.unprompted = true;
            }
        } else if last {
            self.closing()?;
        }
        Ok(())
    }

    /// Every ticket is in: ask for the session's state before the `Done`
    /// (a slash command may have switched sessions).
    fn closing(&mut self) -> Result<(), HarnessError> {
        let Some(turn) = self.turn.as_mut() else {
            return Ok(());
        };
        if turn.closing {
            return Ok(());
        }
        turn.closing = true;
        self.abort_deadline = None;
        let id = self.process.client.request(json!({"type":"get_state"}))?;
        self.pending.insert(id, Pending::Closing);
        self.state_deadline = Some(tokio::time::Instant::now() + STATE_TIMEOUT);
        Ok(())
    }

    /// The barrier answered (or never will): end the turn as it stands.
    async fn close(&mut self) -> Result<(), HarnessError> {
        self.state_deadline = None;
        let Some(turn) = self.turn.as_mut() else {
            return Ok(());
        };
        if !turn.tickets.is_empty() {
            // A steer joined meanwhile: the turn goes on.
            turn.closing = false;
            return Ok(());
        }
        let (mut status, error) = turn.outcome.take().unwrap_or((DoneStatus::Completed, None));
        if self.interrupting && status != DoneStatus::Errored {
            status = DoneStatus::Interrupted;
        }
        // Steers that joined a completed turn were part of it.
        while status == DoneStatus::Completed && !self.deliveries.is_empty() {
            self.confirm_delivery().await?;
        }
        self.end(status, error).await
    }

    /// A turn-end state: the session may have changed under a slash command.
    async fn refresh(&mut self, state: &Value) -> Result<(), HarnessError> {
        self.observe_state(state);
        let session = string(state, "sessionId");
        if !session.is_empty() && session != self.session {
            self.session = session.to_owned();
            self.started().await?;
        }
        Ok(())
    }

    async fn end(&mut self, status: DoneStatus, error: Option<String>) -> Result<(), HarnessError> {
        self.turn = None;
        self.interrupting = false;
        self.abort_deadline = None;
        self.state_deadline = None;
        self.deliveries.clear();
        self.initial_message = false;
        self.process.dialogs.cancel(&self.process.client);
        self.emit(AgentEvent::Done {
            status,
            result: None,
            error,
            session_id: (!self.session.is_empty()).then(|| self.session.clone()),
        })
        .await?;
        self.norm.reset();
        Ok(())
    }
}

/// The model `request` names, unless it leaves the choice to omp.
fn selected_model(request: &RunRequest) -> Option<&str> {
    request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty() && *m != "default")
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
