//! Claude Code harness: spawns the installed `claude` CLI and speaks its
//! stream-json protocol directly — no adapter process in between. Resurrected
//! from the pre-ACP driver (see docs/research/harness.md) and modernized
//! against CLI 2.1.228.
//!
//! - stdout JSONL frames are normalized into [`AgentEvent`]s (init dedupe,
//!   subagent tagging, typed tool decoding, error-code mapping).
//! - PERMISSIONS ride the stdio control channel: `--permission-prompt-tool
//!   stdio` (undocumented — absent from `claude --help`, but it is the same
//!   transport the Claude Agent SDK's `query()` drives, and was re-validated
//!   live against 2.1.228: `can_use_tool` control requests arrive and
//!   allow/deny responses are honored). The alternative channel — an MCP
//!   permission tool — needs a server process and was rejected. Tool calls
//!   auto-allow (zeron sessions run unattended, parity with the ACP
//!   harness's preferred-allow behavior); `AskUserQuestion` round-trips
//!   through [`RunControls::request_input`].
//! - DONE is the CLI's own `result` frame, eagerly: background work (a
//!   spawned subagent) never holds the turn. The CLI natively runs a second
//!   wake turn when a background task finishes — a fresh `init` (same
//!   session id, deduped) plus another `result` — and both are forwarded;
//!   the engine's parked-session resume path turns them into the
//!   done→Working→done wake.
//! - SUBAGENT frames arrive on the same stdout tagged with a top-level
//!   `parent_tool_use_id`; they are wrapped in [`AgentEvent::Subagent`] and
//!   NEVER folded into the parent feed (a background subagent interleaves
//!   with the parent's own stream — folding them in split contiguous text
//!   around phantom tool calls).
//! - Steering: queued [`SteerMessage`]s are written to stdin as user lines at
//!   any time; the CLI folds them into the running turn at its own step
//!   boundary.
//! - Interrupt: cancelling [`RunControls::interrupt`] sends the protocol-level
//!   interrupt control request, then escalates to SIGTERM and SIGKILL.
//! - Live update (`crate::handoff`): a run freezes at a stdout line boundary
//!   (between turns, mid-turn, or with an `AskUserQuestion` parked) and a
//!   later image adopts the same child from its pipes, the reader's leftover
//!   bytes and `ClaudeLoopState`. Interrupting and a held turn end refuse
//!   (`Busy`); title runs never hand off.

pub mod catalog;
mod discovery;
mod normalize;
mod wire;

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SlashCommand,
    SteeringMode, UserInputAnswer, UserInputQuestion,
};

use crate::handoff::{StderrDrain, WriteMsg, run_writer};
use crate::line_reader::LineReader;
use crate::process::{ChildStdin, ChildStdout, Command, Stdio};
use crate::{
    ChildHandle, FreezeRefusal, FreezeRequest, Harness, HarnessError, HarnessHandoff, RunControls,
    Signal, SteerMessage, send_signal, shutdown_child,
};
use catalog::{apply_ultrathink, to_effort};
use normalize::Normalizer;
use wire::{ControlRequestFrame, Frame, allow_response, control_response_line};

/// Locate the device's installed Claude Code CLI: our own PATH, then the
/// login-shell PATH snapshot (the user's shell init shapes PATH in ways a
/// GUI/service launch never sees — see [`crate::shell_env`]), then known
/// install locations as a last resort. The `CLAUDE_CODE_EXECUTABLE` override
/// is applied by [`ClaudeHarness::resolve_executable`], so availability and
/// launches agree on one resolution order. Resolved per call — cheap after
/// the snapshot is cached.
fn resolve_claude_executable() -> Option<PathBuf> {
    let mut extra = Vec::new();
    if let Some(home) = crate::executable::home_dir() {
        extra.push(home.join(".claude").join("local").join("claude"));
        extra.push(home.join(".local").join("bin").join("claude"));
    }
    extra.push(PathBuf::from("/opt/homebrew/bin/claude"));
    extra.push(PathBuf::from("/usr/local/bin/claude"));
    crate::executable::find_on_paths("claude", extra)
}

/// The inline `--mcp-config` JSON for an injected server (the CLI accepts a
/// JSON string as well as a file path).
fn mcp_config_arg(mcp: &zeron_proto::McpServer) -> String {
    serde_json::json!({
        "mcpServers": {
            &mcp.name: {
                "command": mcp.command,
                "args": mcp.args,
                "env": mcp.env,
            }
        }
    })
    .to_string()
}

fn option_is_on(options: &serde_json::Map<String, Value>, key: &str) -> bool {
    match options.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "on" || s == "true",
        _ => false,
    }
}

/// The Claude Code harness. Construct with [`ClaudeHarness::new`]; tests point
/// it at a fake CLI with [`ClaudeHarness::with_executable`].
pub struct ClaudeHarness {
    executable: Option<PathBuf>,
    /// Grace between the interrupt control request and SIGTERM.
    interrupt_grace: Duration,
    /// Grace between SIGTERM and SIGKILL.
    kill_grace: Duration,
    initialize: discovery::InitializeCache,
    models_cache: crate::catalog::Catalog,
    workspace_commands: crate::skills::CommandDiscovery,
}

impl Default for ClaudeHarness {
    fn default() -> Self {
        Self {
            executable: None,
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(3),
            initialize: discovery::InitializeCache::default(),
            models_cache: crate::catalog::Catalog::default(),
            workspace_commands: crate::skills::CommandDiscovery::default(),
        }
    }
}

impl ClaudeHarness {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use a fixed CLI binary instead of PATH/known-location resolution.
    pub fn with_executable(mut self, path: impl Into<PathBuf>) -> Self {
        self.executable = Some(path.into());
        self
    }

    /// Tune the interrupt→SIGTERM→SIGKILL escalation timing.
    pub fn with_graces(mut self, interrupt_grace: Duration, kill_grace: Duration) -> Self {
        self.interrupt_grace = interrupt_grace;
        self.kill_grace = kill_grace;
        self
    }

    fn resolve_executable(&self) -> Result<PathBuf, HarnessError> {
        if let Some(p) = &self.executable {
            return crate::executable::validate_native_override(p);
        }
        if let Some(p) = std::env::var_os("CLAUDE_CODE_EXECUTABLE")
            && !p.is_empty()
        {
            return crate::executable::validate_native_override(&PathBuf::from(p));
        }
        resolve_claude_executable().ok_or_else(|| {
            HarnessError::NotInstalled(
                "claude (searched PATH, the login shell's PATH, ~/.claude/local, \
                 ~/.local/bin, /opt/homebrew/bin, /usr/local/bin, and \
                 fnm/nvm/volta/pnpm/bun install dirs; Windows also checks USERPROFILE \
                 and explicit NVM_SYMLINK/VOLTA_HOME/PNPM_HOME; set \
                 CLAUDE_CODE_EXECUTABLE to override)"
                    .into(),
            )
        })
    }

    fn build_command(&self, exe: &PathBuf, request: &RunRequest) -> Command {
        let mut cmd = Command::new(exe);
        crate::compose_child_path(&mut cmd, exe);
        cmd.args([
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            // Required by the CLI alongside `-p --output-format stream-json`.
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
            // Newer Claude models emit no readable thinking text unless a
            // summary is asked for (raw reasoning stays provider-private).
            "--thinking-display",
            "summarized",
            // Route permission prompts to the stdio control channel so
            // `can_use_tool` (and AskUserQuestion in particular) reaches us.
            // Undocumented flag; validated live against 2.1.228.
            "--permission-prompt-tool",
            "stdio",
        ]);
        // The 1M context window is selected via a model-id suffix
        // (`sonnet[1m]`), exactly how the CLI itself does it; fast mode and
        // always-on thinking are settings overrides.
        if let Some(model) = &request.model {
            let one_m = request
                .model_options
                .get("contextWindow")
                .and_then(Value::as_str)
                == Some("1m");
            cmd.arg("--model");
            cmd.arg(if one_m {
                format!("{model}[1m]")
            } else {
                model.clone()
            });
        }
        if let Some(effort) = to_effort(request.reasoning, request.model.as_deref()) {
            cmd.args(["--effort", effort]);
        }
        if request.auto_approve {
            cmd.args([
                "--permission-mode",
                "bypassPermissions",
                "--dangerously-skip-permissions",
            ]);
        } else {
            cmd.args(["--permission-mode", "default"]);
        }
        if let Some(resume) = &request.resume {
            cmd.arg(format!("--resume={resume}"));
        }
        let mut settings = serde_json::Map::new();
        if option_is_on(&request.model_options, "fastMode") {
            settings.insert("fastMode".into(), Value::Bool(true));
        }
        if option_is_on(&request.model_options, "thinking") {
            settings.insert("alwaysThinkingEnabled".into(), Value::Bool(true));
        }
        if request.reasoning == Some(ReasoningLevel::Ultracode) {
            settings.insert("ultracode".into(), Value::Bool(true));
        }
        if !settings.is_empty() {
            cmd.arg("--settings");
            cmd.arg(Value::Object(settings).to_string());
        }
        if !request.cwd.is_empty() {
            cmd.current_dir(&request.cwd);
        }
        // Not killed on drop: a run frozen for a live update is held through
        // an `execve` and its child must outlive the old image's handles.
        // Every ending shuts the child down explicitly.
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        cmd
    }

    /// Share the complete initialize response between model and command discovery.
    /// No user message is written; the short-lived child is retired after initialize.
    async fn initialize(&self) -> Result<Value, HarnessError> {
        self.initialize
            .get(
                || self.model_context().map(|c| c.unwrap().key()),
                || self.probe_initialize(None),
            )
            .await
    }

    async fn discover_commands(
        &self,
        cwd: Option<&std::path::Path>,
    ) -> Result<Vec<SlashCommand>, HarnessError> {
        let response = match cwd {
            Some(cwd) => self.probe_initialize(Some(cwd)).await?,
            None => self.initialize().await?,
        };
        Ok(parse_initialize_commands(&response))
    }

    async fn probe_initialize(&self, cwd: Option<&std::path::Path>) -> Result<Value, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut cmd = Command::new(&exe);
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        crate::compose_child_path(&mut cmd, &exe);
        cmd.args([
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            // Mandatory with --print + stream-json output; without it the
            // CLI exits immediately with a usage error.
            "--verbose",
        ]);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(crate::executable::binary_hint(&exe))
            } else {
                HarnessError::Io(e)
            }
        })?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            shutdown_child(&mut child, self.kill_grace).await;
            return Err(HarnessError::Protocol("claude child has no stdio".into()));
        };
        const PROBE_ID: &str = "zeron-command-probe";
        let discovery = async {
            let request = serde_json::json!({
                "type": "control_request",
                "request_id": PROBE_ID,
                "request": { "subtype": "initialize" },
            });
            stdin
                .write_all(format!("{request}\n").as_bytes())
                .await
                .map_err(HarnessError::Io)?;
            stdin.flush().await.map_err(HarnessError::Io)?;
            let mut lines = BufReader::new(stdout).lines();
            while let Some(line) = lines.next_line().await.map_err(HarnessError::Io)? {
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if frame.get("type").and_then(Value::as_str) != Some("control_response") {
                    continue;
                }
                let response = frame.get("response").cloned().unwrap_or(Value::Null);
                if response.get("request_id").and_then(Value::as_str) != Some(PROBE_ID) {
                    continue;
                }
                if response.get("subtype").and_then(Value::as_str) == Some("error") {
                    let msg = response
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("initialize control request failed");
                    return Err(HarnessError::Protocol(msg.into()));
                }
                return Ok(response);
            }
            Err(HarnessError::Protocol(
                "claude exited before answering the initialize control request".into(),
            ))
        };
        let result = tokio::time::timeout(Duration::from_secs(10), discovery).await;
        shutdown_child(&mut child, self.kill_grace).await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(HarnessError::Protocol("Claude initialize timed out".into())),
        }
    }
}

/// `commands` out of an `initialize` control_response payload
/// (`response.response.commands`: name / description / argumentHint).
fn parse_initialize_commands(response: &Value) -> Vec<SlashCommand> {
    response
        .get("response")
        .and_then(|r| r.get("commands"))
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|c| {
            let name = c.get("name").and_then(Value::as_str)?.trim();
            if name.is_empty() {
                return None;
            }
            Some(SlashCommand {
                name: name.to_owned(),
                description: c
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                input_hint: c
                    .get("argumentHint")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                    .map(str::to_owned),
            })
        })
        .collect()
}

#[async_trait]
impl Harness for ClaudeHarness {
    fn id(&self) -> HarnessId {
        HarnessId::ClaudeCode
    }
    fn display_name(&self) -> &str {
        "Claude Code"
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
        ]
    }
    fn installed(&self) -> bool {
        // The launch resolver, not bare discovery: a valid CLAUDE_CODE_EXECUTABLE
        // (or a test `executable`) must report installed, and an invalid one
        // must not — availability and launches share one resolution.
        self.resolve_executable().is_ok()
    }
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    /// Done is the CLI's own terminal frame, for wake turns too.
    fn deterministic_turn_end(&self) -> bool {
        true
    }

    /// Credential and executable identity scopes both initialize and catalog caches.
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        crate::model_context::context(self.id(), &self.resolve_executable()?, &[]).map(Some)
    }
    fn fallback_models(&self) -> Vec<Model> {
        catalog::configured_models()
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.model_context()?.unwrap().log();
        self.models_cache
            .get_with_timeout(
                force,
                Duration::from_secs(35),
                || self.model_context().map(|c| c.unwrap().key()),
                || async {
                    let response = self.initialize().await?;
                    catalog::with_discovered_models(catalog::configured_models(), &response)
                },
            )
            .await
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.resolve_executable()?;
        match self.model_catalog(false).await {
            Ok(catalog) => Ok(catalog.models),
            Err(error) => {
                tracing::warn!(%error, source = "static", "Claude model discovery failed");
                Ok(self.fallback_models())
            }
        }
    }

    /// Slash commands from the CLI's `initialize` control-request handshake —
    /// the same channel the Claude Agent SDK's `query()` opens. The response
    /// carries every command with description + argument hint and involves no
    /// model turn (verified live, 2.1.228: the control_response is the first
    /// stdout line, well before any API traffic). Cached on success.
    async fn skills(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        let (skills, commands) = tokio::try_join!(
            crate::skills::discover(self.id(), cwd),
            self.workspace_commands
                .get(cwd, self.discover_commands(Some(cwd)))
        )?;
        // The native advertised catalog controls availability (including plugin
        // enablement and skillOverrides). Shared Agent Skills can use file delivery.
        Ok(Some(
            skills
                .into_iter()
                .filter_map(|mut skill| {
                    if crate::skills::is_shared_skill(&skill.path) {
                        // Shared files are not Claude command definitions. A
                        // same-named built-in must not replace their identity.
                        Some(skill)
                    } else if zeron_proto::invocation::valid_skill_command_name(&skill.name)
                        && commands.iter().any(|command| command.name == skill.name)
                    {
                        skill.command = Some(zeron_proto::invocation::SkillCommand {
                            name: skill.name.clone(),
                            harness: self.id(),
                        });
                        Some(skill)
                    } else {
                        None
                    }
                })
                .collect(),
        ))
    }

    async fn commands(&self) -> Result<Vec<SlashCommand>, HarnessError> {
        self.discover_commands(None).await
    }

    async fn commands_for(&self, cwd: &std::path::Path) -> Result<Vec<SlashCommand>, HarnessError> {
        self.workspace_commands
            .get(cwd, self.discover_commands(Some(cwd)))
            .await
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.run_with_mode(request, controls, false).await
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
        request.model_options.clear();
        request.auto_approve = false;
        self.run_with_mode(request, controls, true).await
    }

    fn supports_adoption(&self) -> bool {
        cfg!(unix)
    }

    /// Rebuild a run around a `claude` child a previous image froze: same
    /// pipes (duplicated), same conversation, no `SessionStarted` and no
    /// prompt written. A parked `AskUserQuestion` is re-attached under the
    /// engine's existing request.
    async fn adopt(
        &self,
        handoff: HarnessHandoff,
        controls: RunControls,
        request: RunRequest,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        #[cfg(unix)]
        {
            self.adopt_unix(handoff, controls, request)
        }
        #[cfg(not(unix))]
        {
            let _ = (handoff, controls, request);
            Err(HarnessError::Protocol(
                "Claude runs cannot be adopted on this platform".into(),
            ))
        }
    }
}

impl ClaudeHarness {
    #[cfg(unix)]
    fn adopt_unix(
        &self,
        handoff: HarnessHandoff,
        controls: RunControls,
        request: RunRequest,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        use crate::handoff::dup_inherited;
        if handoff.state_version != STATE_VERSION {
            return Err(HarnessError::Protocol(format!(
                "claude handoff state version {} is not {STATE_VERSION}",
                handoff.state_version
            )));
        }
        let state: ClaudeLoopState = serde_json::from_value(handoff.state)
            .map_err(|e| HarnessError::Protocol(format!("claude handoff state: {e}")))?;
        let stdin = ChildStdin::from_std(std::process::ChildStdin::from(dup_inherited(
            handoff.stdin_fd,
        )?))?;
        let stdout = ChildStdout::from_std(std::process::ChildStdout::from(dup_inherited(
            handoff.stdout_fd,
        )?))?;
        let stderr = match handoff.stderr_fd {
            Some(fd) => Some(tokio::process::ChildStderr::from_std(
                std::process::ChildStderr::from(dup_inherited(fd)?),
            )?),
            None => None,
        };
        let child = ChildHandle::adopt(handoff.pid)?;
        let stderr_tail = crate::StderrTail::seeded(handoff.stderr_tail);
        let stderr = stderr.map(|s| StderrDrain::spawn(s, stderr_tail.clone(), "claude"));
        // Parked questions answer under the engine's existing request: the
        // bridge finds it by any of its question ids. No second prompt.
        let answers = Answers::new();
        for parked in &state.parked_questions {
            let answer = match parked.questions.first() {
                Some(question) => (controls.rebind_input)(question.id.clone()),
                None => tokio::sync::oneshot::channel().1,
            };
            answers.push(await_answer(parked.control_request_id.clone(), answer));
        }
        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        tokio::spawn(run_session(Session {
            title_only: false,
            child,
            reader: LineReader::with_leftover(stdout, handoff.stdout_leftover),
            writer: spawn_writer(stdin),
            stderr,
            event_tx,
            controls,
            reasoning: request.reasoning,
            interrupt_grace: self.interrupt_grace,
            kill_grace: self.kill_grace,
            stderr_tail,
            state,
            answers,
        }));
        Ok(event_stream(event_rx))
    }
}

impl ClaudeHarness {
    async fn run_with_mode(
        &self,
        request: RunRequest,
        controls: RunControls,
        title_only: bool,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut cmd = self.build_command(&exe, &request);
        if title_only {
            cmd.args([
                "--system-prompt",
                crate::TITLE_INSTRUCTIONS,
                "--tools",
                "",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--setting-sources",
                "",
            ]);
        } else if let Some(mcp) = &request.mcp {
            // Zeron's own server rides beside the user's configured servers
            // (no `--strict-mcp-config`): the CLI merges an inline JSON
            // config with settings-sourced ones.
            cmd.args(["--mcp-config", &mcp_config_arg(mcp)]);
        }
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(crate::executable::binary_hint(&exe))
            } else {
                HarnessError::Io(e)
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Protocol("claude child has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol("claude child has no stdout".into()))?;
        let stderr_tail = crate::StderrTail::default();
        let stderr = child
            .stderr
            .take()
            .map(|stderr| StderrDrain::spawn(stderr, stderr_tail.clone(), "claude"));
        let writer = spawn_writer(stdin);

        // The initial prompt as the first stdin user line (streaming-input
        // mode). Ultrathink rides every user message — steers included.
        // Staged image attachments are inlined as base64 image content blocks
        // ahead of the text (verified against the real CLI); their path refs
        // also ride the prompt text, so a skipped/unreadable file degrades to
        // the old-app behavior (the agent opens the path with its Read tool).
        let images = load_image_blocks(&request.attachments).await;
        let first = wire::user_message_line_with_images(
            &apply_ultrathink(request.reasoning, &request.prompt),
            &images,
        );
        let _ = writer.send(WriteMsg::Line(first));

        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        tokio::spawn(run_session(Session {
            title_only,
            child: ChildHandle::Owned(child),
            reader: LineReader::new(stdout),
            writer,
            stderr,
            event_tx,
            controls,
            reasoning: request.reasoning,
            interrupt_grace: self.interrupt_grace,
            kill_grace: self.kill_grace,
            stderr_tail,
            state: ClaudeLoopState::new(),
            answers: Answers::new(),
        }));
        Ok(event_stream(event_rx))
    }
}

fn event_stream(
    rx: mpsc::Receiver<Result<AgentEvent, HarnessError>>,
) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    futures::stream::unfold(
        rx,
        |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
    )
    .boxed()
}

/// The task that owns the child's stdin (see [`run_writer`]): whole lines
/// only, and it can pause for a freeze. [`WriteMsg::Close`] ends the input —
/// the CLI finishes the current turn and exits, which ends the run stream at
/// stdout EOF.
fn spawn_writer(stdin: ChildStdin) -> mpsc::UnboundedSender<WriteMsg> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run_writer(stdin, rx, "claude"));
    tx
}

/// Anthropic's API caps inline images at 5MB of raw bytes; larger files stay
/// path refs only.
const MAX_INLINE_IMAGE_BYTES: u64 = 5 * 1024 * 1024;

/// Media type for an inline image block — extension first, magic bytes as the
/// fallback (pasted screenshots may carry odd names). Only the API-supported
/// inline types map; anything else (svg/bmp/tiff/…) returns `None`.
fn image_media_type(path: &std::path::Path, bytes: &[u8]) -> Option<&'static str> {
    let by_ext = match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    };
    by_ext.or(match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        _ => None,
    })
}

/// Load `RunRequest::attachments` into inline image blocks, best-effort: an
/// unreadable, oversized, or unsupported file is skipped — its path ref still
/// rides the prompt text — never fatal to the run.
async fn load_image_blocks(paths: &[String]) -> Vec<wire::ImageBlock> {
    use base64::Engine as _;
    let mut blocks = Vec::new();
    for path in paths {
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(target: "zeron_harness::claude", %path, error = %err, "attachment unreadable; path ref only");
                continue;
            }
        };
        if bytes.len() as u64 > MAX_INLINE_IMAGE_BYTES {
            tracing::debug!(target: "zeron_harness::claude", %path, "attachment over inline cap; path ref only");
            continue;
        }
        let Some(media_type) = image_media_type(std::path::Path::new(path), &bytes) else {
            tracing::debug!(target: "zeron_harness::claude", %path, "attachment not an inline-supported image; path ref only");
            continue;
        };
        blocks.push(wire::ImageBlock {
            media_type: media_type.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        });
    }
    blocks
}

/// Schema version of [`ClaudeLoopState`] in a [`HarnessHandoff`]; an adopter
/// refuses any other, and the engine falls back to crash recovery.
#[cfg(unix)]
pub(crate) const STATE_VERSION: u32 = 1;

/// How long a freeze waits for the queued stdin lines to be written. A child
/// that stops reading its input while we stop reading its output would
/// otherwise wedge the freeze (and the run); the freeze answers `Busy`.
#[cfg(unix)]
const PAUSE_TIMEOUT: Duration = Duration::from_secs(2);

/// An `AskUserQuestion` waiting on the user, as a handoff carries it.
#[derive(Clone, Serialize, Deserialize)]
struct ParkedQuestion {
    /// The CLI's `can_use_tool` control-request id; the answer goes back
    /// under it.
    control_request_id: String,
    /// The tool input as the CLI sent it; the answers are merged into it.
    raw_input: Value,
    /// The questions as the engine knows them — their ids are random, minted
    /// once, and the engine rebinds by them.
    questions: Vec<UserInputQuestion>,
}

/// The run loop's protocol state: what a freeze exports and an adoption
/// resumes from. Everything else the loop keeps is either derived from the
/// pipes (exported beside this) or is a state a freeze refuses (interrupting,
/// a held turn end).
#[derive(Clone, Serialize, Deserialize)]
struct ClaudeLoopState {
    normalizer: Normalizer,
    /// Steer uuids written to stdin and not yet confirmed by their replay.
    pending_steers: VecDeque<String>,
    /// Top-level tool calls in flight: a steer must not abort them (see
    /// `wire::steer_message_line`).
    open_tools: BTreeSet<String>,
    steering_open: bool,
    any_done: bool,
    parked_questions: Vec<ParkedQuestion>,
}

impl ClaudeLoopState {
    fn new() -> Self {
        Self {
            normalizer: Normalizer::new(),
            pending_steers: VecDeque::new(),
            open_tools: BTreeSet::new(),
            steering_open: true,
            any_done: false,
            parked_questions: Vec::new(),
        }
    }
}

/// Answers for parked questions, keyed by control-request id. Kept in the
/// loop (not a detached task) so a freeze can export what is still parked,
/// and never dropped by a freeze: the engine resolves a question whose
/// receiver goes away.
type Answers = futures::stream::FuturesUnordered<
    futures::future::BoxFuture<'static, (String, Option<Vec<UserInputAnswer>>)>,
>;

fn await_answer(
    control_request_id: String,
    answer: tokio::sync::oneshot::Receiver<Vec<UserInputAnswer>>,
) -> futures::future::BoxFuture<'static, (String, Option<Vec<UserInputAnswer>>)> {
    Box::pin(async move { (control_request_id, answer.await.ok()) })
}

struct Session {
    title_only: bool,
    child: ChildHandle,
    reader: LineReader<ChildStdout>,
    writer: mpsc::UnboundedSender<WriteMsg>,
    stderr: Option<StderrDrain>,
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    controls: RunControls,
    reasoning: Option<ReasoningLevel>,
    interrupt_grace: Duration,
    kill_grace: Duration,
    /// Rolling stderr tail for the crash message on an unexpected exit.
    stderr_tail: crate::StderrTail,
    state: ClaudeLoopState,
    answers: Answers,
}

/// The next steer: those a thawed freeze put back first, then the mailbox.
async fn next_steer(
    held: &mut VecDeque<SteerMessage>,
    steering: &mut mpsc::Receiver<SteerMessage>,
) -> Option<SteerMessage> {
    match held.pop_front() {
        Some(steer) => Some(steer),
        None => steering.recv().await,
    }
}

/// The per-run event loop: one task multiplexing stdout frames, the steering
/// mailbox, parked answers, the interrupt token, live-update freezes and
/// consumer liveness.
async fn run_session(session: Session) {
    let Session {
        title_only,
        child,
        reader,
        writer,
        mut stderr,
        event_tx,
        controls,
        reasoning,
        interrupt_grace,
        kill_grace,
        stderr_tail,
        mut state,
        mut answers,
    } = session;
    let RunControls {
        execution_lease: _execution_lease,
        request_input,
        mut steering,
        interrupt,
        mut freeze,
        rebind_input: _rebind_input,
    } = controls;
    // `None` only after a committed freeze handed them to a successor.
    let mut child = Some(child);
    let mut reader = Some(reader);

    let mut interrupted = false;
    let mut interrupt_sent = false;
    // A turn end held back while steers wait for their replay. Rapid `now`
    // steers each interrupt the turn the previous one started, and the CLI
    // replays only the last (verified on 2.1.280; the earlier texts still
    // reach the model). If nothing follows the held result, the steers were
    // absorbed: release them and the turn end instead of spinning forever.
    const HELD_DONE_SETTLE: Duration = Duration::from_secs(5);
    let mut held_done: Option<(AgentEvent, tokio::time::Instant)> = None;
    let mut done_after_interrupt = false;
    let mut escalation: Option<tokio::task::JoinHandle<()>> = None;
    // Steers a freeze drained from the mailbox and a thaw put back.
    let mut held_steers: VecDeque<SteerMessage> = VecDeque::new();
    // With no sender `freeze.recv()` is `None` at once and forever.
    let mut freeze_open = true;

    'main: loop {
        tokio::select! {
            // Biased: a freeze request is answered ahead of new input (the
            // engine asks while nothing else should move), and the rare inputs
            // (steers, answers) go ahead of a possibly continuous stdout.
            biased;

            _ = interrupt.cancelled(), if !interrupt_sent => {
                interrupt_sent = true;
                interrupted = true;
                let _ = writer.send(WriteMsg::Line(wire::interrupt_request_line("int_1")));
                // Escalate if the CLI doesn't wind down within the grace
                // periods: SIGTERM (kills bash trees, runs SessionEnd hooks),
                // then SIGKILL. Aborted once the child is reaped.
                if let Some(pid) = child.as_ref().and_then(ChildHandle::signal_target) {
                    escalation = Some(tokio::spawn(async move {
                        tokio::time::sleep(interrupt_grace).await;
                        send_signal(&pid, Signal::Term);
                        tokio::time::sleep(kill_grace).await;
                        send_signal(&pid, Signal::Kill);
                    }));
                }
            },

            request = freeze.recv(), if freeze_open => match request {
                None => freeze_open = false,
                Some(request) => {
                    let refusal = if title_only {
                        Some(FreezeRefusal::Unsupported)
                    } else if interrupted {
                        Some(FreezeRefusal::Busy("interrupting"))
                    } else if held_done.is_some() {
                        Some(FreezeRefusal::Busy("held done"))
                    } else {
                        None
                    };
                    if let Some(refusal) = refusal {
                        let _ = request.reply.send(Err(refusal));
                        continue;
                    }
                    // An answer the user already gave but the loop has not written
                    // yet must go out BEFORE the pause: exported as "still
                    // parked" it would be lost across the exec (the engine no
                    // longer holds it) and the adopter would answer with nothing.
                    while !answers.is_empty()
                        && let Some(Some((request_id, answer))) =
                            futures::FutureExt::now_or_never(answers.next())
                    {
                        write_answer(&mut state, &writer, &request_id, answer);
                    }
                    let outcome = freeze_run(request, Freezing {
                        child: &mut child,
                        reader: &mut reader,
                        writer: &writer,
                        stderr: &mut stderr,
                        stderr_tail: &stderr_tail,
                        state: &state,
                        held_steers: &mut held_steers,
                        steering: &mut steering,
                    })
                    .await;
                    if let Frozen::Committed = outcome {
                        // The successor owns the child and its pipes; ending
                        // the stream (no Done) is the end the engine expects.
                        return;
                    }
                }
            },

            steer = next_steer(&mut held_steers, &mut steering), if state.steering_open && !interrupted => match steer {
                Some(msg) => {
                    let id = uuid::Uuid::new_v4().to_string();
                    let line = wire::steer_message_line(
                        &apply_ultrathink(reasoning, &msg.prompt),
                        &id,
                        state.open_tools.is_empty(),
                    );
                    state.pending_steers.push_back(id);
                    if writer.send(WriteMsg::Line(line)).is_err() { break 'main; }
                }
                None => {
                    // Mailbox closed: end the input so the run can finish
                    // after the current turn.
                    state.steering_open = false;
                    let _ = writer.send(WriteMsg::Close);
                }
            },

            Some((request_id, answer)) = answers.next(), if !answers.is_empty() => {
                write_answer(&mut state, &writer, &request_id, answer);
            },

            line = reader.as_mut().expect("read until committed").next_line() => match line {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    // The CLI is still producing: whatever it is doing is not
                    // the quiet end the held turn end waits for.
                    if let Some((_, deadline)) = held_done.as_mut() {
                        *deadline = tokio::time::Instant::now() + HELD_DONE_SETTLE;
                    }
                    let frame = match wire::parse_frame(line) {
                        Ok(frame) => frame,
                        Err(e) => {
                            tracing::debug!(target: "zeron_harness::claude", "unparseable frame (skipped): {e}");
                            continue;
                        }
                    };
                    if let Frame::ControlRequest(req) = frame {
                        if title_only {
                            let line = control_response_line(&req.request_id, serde_json::json!({
                                "behavior": "deny", "message": "Tools are disabled for title generation"
                            }));
                            let _ = writer.send(WriteMsg::Line(line));
                        } else {
                            handle_control_request(req, &request_input, &writer, &mut state, &mut answers);
                        }
                        continue;
                    }
                    // Only the CLI's replay confirms that a prompt joined its
                    // conversation. Writing stdin must not split ongoing text.
                    if let Frame::User(ref user) = frame {
                        // A replay confirms its steer and every earlier one:
                        // superseded steers are never replayed themselves.
                        if user.parent_tool_use_id.is_none()
                            && let Some(at) = user
                                .uuid
                                .as_ref()
                                .and_then(|id| state.pending_steers.iter().position(|p| p == id))
                        {
                            for _ in 0..=at {
                                state.pending_steers.pop_front();
                                let (prev, next) = state.normalizer.rotate_for_steer();
                                if event_tx.send(Ok(AgentEvent::Steered {
                                    assistant_message_id: Some(prev), next_assistant_message_id: Some(next),
                                })).await.is_err() { break 'main; }
                            }
                        }
                    }
                    for ev in state.normalizer.normalize(frame, interrupted) {
                        match &ev {
                            AgentEvent::ToolCall { id, .. } => {
                                state.open_tools.insert(id.clone());
                            }
                            AgentEvent::ToolResult { id, .. } => {
                                state.open_tools.remove(id);
                            }
                            AgentEvent::Done { .. } => state.open_tools.clear(),
                            _ => {}
                        }
                        let is_done = matches!(ev, AgentEvent::Done { .. });
                        // A `now` steer ends the turn it interrupts with a
                        // result frame; the steer continues the run, so that
                        // result is a steer boundary, not the end of the turn.
                        if is_done && !interrupted && !state.pending_steers.is_empty() {
                            held_done =
                                Some((ev, tokio::time::Instant::now() + HELD_DONE_SETTLE));
                            continue;
                        }
                        if is_done {
                            held_done = None;
                        }
                        if event_tx.send(Ok(ev)).await.is_err() {
                            break 'main; // consumer gone — reap below
                        }
                        if is_done {
                            state.any_done = true;
                            if interrupted {
                                done_after_interrupt = true;
                                break 'main;
                            }
                        }
                    }
                }
                Ok(None) => break 'main, // stdout EOF: the CLI exited
                Err(e) => {
                    let _ = event_tx.send(Err(HarnessError::Io(e))).await;
                    break 'main;
                }
            },

            _ = tokio::time::sleep_until(
                held_done.as_ref().map_or_else(tokio::time::Instant::now, |(_, d)| *d)
            ), if held_done.is_some() => {
                // The steers were absorbed into the turn that just ended.
                while state.pending_steers.pop_front().is_some() {
                    let (prev, next) = state.normalizer.rotate_for_steer();
                    if event_tx.send(Ok(AgentEvent::Steered {
                        assistant_message_id: Some(prev), next_assistant_message_id: Some(next),
                    })).await.is_err() { break 'main; }
                }
                let (done, _) = held_done.take().expect("guarded by if");
                if event_tx.send(Ok(done)).await.is_err() {
                    break 'main;
                }
                state.any_done = true;
            },

            _ = event_tx.closed() => break 'main,
        }
    }
    // (Only a commit takes the child, and a commit returns above.)
    let Some(mut child) = child else {
        return;
    };

    // A turn end still held when the CLI exited is the run's real end.
    if let Some((done, _)) = held_done.take()
        && !event_tx.is_closed()
        && event_tx.send(Ok(done)).await.is_ok()
    {
        state.any_done = true;
    }

    // Terminal bookkeeping: never end the stream without a Done unless the
    // consumer already hung up.
    if !event_tx.is_closed() {
        if interrupted && !done_after_interrupt {
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: state.normalizer.session_id.clone(),
                }))
                .await;
        } else if !interrupted && !state.any_done {
            // Stdout EOF usually just precedes the exit (and the last stderr
            // lines): give both a moment so the message says what happened.
            stderr_tail.wait_closed().await;
            let status = tokio::time::timeout(Duration::from_millis(500), child.wait())
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|outcome| outcome.status());
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(crate::crash_message("claude", status, &stderr_tail)),
                    session_id: state.normalizer.session_id.clone(),
                }))
                .await;
        }
    }

    child.shutdown(kill_grace).await;
    if let Some(handle) = escalation {
        handle.abort();
    }
}

/// How a freeze ended for the run loop.
enum Frozen {
    /// The engine let the run go (a failed exec, a refusal): carry on.
    Thawed,
    #[cfg_attr(not(unix), allow(dead_code))]
    /// A same-process successor took the child and pipes: end the stream.
    Committed,
}

/// Answer a parked question: write the CLI's `control_response` with the
/// user's answers under the original request and question ids. A dropped
/// sender (the caller went away) degrades to empty answers so the agent is
/// unblocked rather than wedged.
fn write_answer(
    state: &mut ClaudeLoopState,
    writer: &mpsc::UnboundedSender<WriteMsg>,
    request_id: &str,
    answer: Option<Vec<UserInputAnswer>>,
) {
    let Some(at) = state
        .parked_questions
        .iter()
        .position(|parked| parked.control_request_id == request_id)
    else {
        return;
    };
    let parked = state.parked_questions.remove(at);
    let updated = updated_input_with_answers(
        &parked.raw_input,
        &parked.questions,
        &answer.unwrap_or_default(),
    );
    let line = control_response_line(request_id, allow_response(updated));
    let _ = writer.send(WriteMsg::Line(line));
}

/// What a freeze needs of the run loop.
#[cfg_attr(not(unix), allow(dead_code))]
struct Freezing<'a> {
    child: &'a mut Option<ChildHandle>,
    reader: &'a mut Option<LineReader<ChildStdout>>,
    writer: &'a mpsc::UnboundedSender<WriteMsg>,
    stderr: &'a mut Option<StderrDrain>,
    stderr_tail: &'a crate::StderrTail,
    state: &'a ClaudeLoopState,
    held_steers: &'a mut VecDeque<SteerMessage>,
    steering: &'a mut mpsc::Receiver<SteerMessage>,
}

/// Stop at this line boundary and offer the run to the engine; suspended
/// until its verdict. Refusals the loop can see itself (interrupting, a held
/// turn end) are answered before this is called.
#[cfg(unix)]
async fn freeze_run(request: FreezeRequest, run: Freezing<'_>) -> Frozen {
    use crate::handoff::{PipeFd, drain_steering};
    use crate::{FrozenRun, SteerRecord};
    use std::os::fd::AsRawFd;
    let busy = |reply: tokio::sync::oneshot::Sender<_>, reason| {
        let _ = reply.send(Err(FreezeRefusal::Busy(reason)));
        Frozen::Thawed
    };
    // The engine rebinds a parked question by one of its ids.
    if run
        .state
        .parked_questions
        .iter()
        .any(|parked| parked.questions.is_empty())
    {
        return busy(request.reply, "a question without questions is pending");
    }
    let (Some(child), Some(reader)) = (run.child.as_ref(), run.reader.as_ref()) else {
        return busy(request.reply, "the agent process has exited");
    };
    let Some(pid) = child.id() else {
        return busy(request.reply, "the agent process has exited");
    };
    // An exit inside the freeze window must stay reapable by the next image.
    if !child.hold_reaping() {
        return busy(request.reply, "the agent process has exited");
    }
    // Queued stdin lines finish first; the writer then parks WITH its pipe
    // and queue, so a thaw needs nothing rebuilt.
    let (pause_tx, pause_rx) = tokio::sync::oneshot::channel();
    let paused = match run.writer.send(WriteMsg::Pause(pause_tx)) {
        Ok(()) => tokio::time::timeout(PAUSE_TIMEOUT, pause_rx)
            .await
            .ok()
            .and_then(Result::ok),
        Err(_) => None,
    };
    let Some(paused) = paused else {
        child.release_reaping();
        return busy(request.reply, "the agent is not taking its input");
    };
    let Some(stdin_fd) = paused.stdin_fd else {
        child.release_reaping();
        return busy(request.reply, "the agent's pipes are not exportable");
    };
    let stdout_fd = reader.get_ref().as_raw_fd();
    crate::handoff::grow_pipe(stdout_fd);
    // Steers a previous, thawed freeze put back come first, then the mailbox.
    let mut undrained: Vec<SteerRecord> =
        run.held_steers.drain(..).map(SteerRecord::from).collect();
    undrained.extend(drain_steering(run.steering));
    let handoff = HarnessHandoff {
        harness: HarnessId::ClaudeCode,
        state_version: STATE_VERSION,
        pid: pid as i32,
        stdin_fd,
        stdout_fd,
        stderr_fd: run.stderr.as_ref().and_then(StderrDrain::fd),
        stdout_leftover: reader.leftover().to_vec(),
        stderr_tail: run.stderr_tail.lines(),
        state: serde_json::to_value(run.state).expect("claude loop state serializes"),
        extra_fds: Vec::new(),
        undrained_steers: undrained.clone(),
    };
    let (commit_tx, commit_rx) = tokio::sync::oneshot::channel();
    let sent = request.reply.send(Ok(FrozenRun {
        handoff,
        commit: commit_tx,
    }));
    if sent.is_ok() && commit_rx.await.is_ok() {
        // A same-process successor owns the child and its pipes now (the
        // exec path never gets here): give them up without closing, killing
        // or reaping anything.
        paused.abandon().await;
        if let Some(stderr) = run.stderr.take() {
            stderr.abandon().await;
        }
        if let Some(child) = run.child.take() {
            let _ = child.release();
        }
        if let Some(reader) = run.reader.take() {
            reader.into_parts().0.leak_pipe();
        }
        return Frozen::Committed;
    }
    // Thawed: dropping the guard resumes the writer, the reader kept its
    // buffer, and the drained steers are read again, in order.
    drop(paused);
    child.release_reaping();
    run.held_steers
        .extend(undrained.into_iter().map(SteerMessage::from));
    Frozen::Thawed
}

#[cfg(not(unix))]
async fn freeze_run(request: FreezeRequest, _run: Freezing<'_>) -> Frozen {
    let _ = request.reply.send(Err(FreezeRefusal::Unsupported));
    Frozen::Thawed
}

type RequestInputFn = Box<
    dyn Fn(Vec<UserInputQuestion>) -> tokio::sync::oneshot::Receiver<Vec<UserInputAnswer>>
        + Send
        + Sync,
>;

/// Serve one `can_use_tool` control request. Every tool is auto-approved
/// (unattended parity — the CLI still blocks until SOME response arrives, so
/// every request must be answered); `AskUserQuestion` is intercepted —
/// surface the questions through the engine's input bridge (which owns the
/// `InputRequested`/`InputResolved` lifecycle) and park them until the
/// answers arrive (the run loop keeps flowing meanwhile, and hands them back
/// keyed by question text, as the tool expects).
fn handle_control_request(
    req: ControlRequestFrame,
    request_input: &RequestInputFn,
    writer: &mpsc::UnboundedSender<WriteMsg>,
    state: &mut ClaudeLoopState,
    answers: &mut Answers,
) {
    if req.request.subtype != "can_use_tool" {
        tracing::debug!(
            target: "zeron_harness::claude",
            "unhandled control_request subtype: {}", req.request.subtype
        );
        return;
    }
    if req.request.tool_name != "AskUserQuestion" {
        let line = control_response_line(&req.request_id, allow_response(req.request.input));
        let _ = writer.send(WriteMsg::Line(line));
        return;
    }
    let questions = parse_questions(&req.request.input);
    // The engine's input bridge is the SOLE emitter of
    // `InputRequested`/`InputResolved`: it mints the request id, parks the
    // resolver for `respond_input`, and surfaces both events. Emitting our
    // own copy here (keyed by Claude's control-request id) folded a SECOND
    // input part into the doc whose id no resolver knew — the QuestionPanel
    // answered that unanswerable twin and the run never resumed.
    let answer = (request_input)(questions.clone());
    answers.push(await_answer(req.request_id.clone(), answer));
    state.parked_questions.push(ParkedQuestion {
        control_request_id: req.request_id,
        raw_input: req.request.input,
        questions,
    });
}

/// Parse Claude's `AskUserQuestion` tool input into [`UserInputQuestion`]s
/// (tolerant of `header`/`title`, `question`/`prompt`, string or object
/// options — option descriptions are dropped, the wire type carries labels).
fn parse_questions(input: &Value) -> Vec<UserInputQuestion> {
    let raw = input.get("questions").and_then(Value::as_array);
    raw.map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|q| {
            let field =
                |keys: [&str; 2]| keys.iter().find_map(|k| q.get(*k).and_then(Value::as_str));
            UserInputQuestion {
                id: uuid::Uuid::new_v4().to_string(),
                header: field(["header", "title"]).unwrap_or("Question").into(),
                question: field(["question", "prompt"]).unwrap_or("").into(),
                prefill: None,
                multiline: false,
                multi_select: ["multiSelect", "multi_select"]
                    .iter()
                    .find_map(|k| q.get(*k).and_then(Value::as_bool))
                    .unwrap_or(false),
                options: q
                    .get("options")
                    .and_then(Value::as_array)
                    .map(|a| a.as_slice())
                    .unwrap_or_default()
                    .iter()
                    .map(|op| match op {
                        Value::String(s) => s.clone(),
                        other => other
                            .get("label")
                            .or_else(|| other.get("value"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Merge the user's answers back into the tool input, keyed by question text
/// (single-select ⇒ a string, multi-select ⇒ an array), as the tool expects.
fn updated_input_with_answers(
    input: &Value,
    questions: &[UserInputQuestion],
    answers: &[UserInputAnswer],
) -> Value {
    let mut updated = match input {
        Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    let mut by_question = serde_json::Map::new();
    for q in questions {
        let labels: Vec<String> = answers
            .iter()
            .find(|a| a.question_id == q.id)
            .map(|a| a.labels.clone())
            .unwrap_or_default();
        let value = if q.multi_select {
            Value::Array(labels.into_iter().map(Value::String).collect())
        } else {
            Value::String(labels.into_iter().next().unwrap_or_default())
        };
        by_question.insert(q.question.clone(), value);
    }
    updated.insert("answers".into(), Value::Object(by_question));
    Value::Object(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_questions_tolerantly() {
        let input = json!({
            "questions": [
                {
                    "header": "Choice",
                    "question": "Pick one",
                    "options": ["A", {"label": "B", "description": "second"}],
                    "multiSelect": false
                },
                { "title": "Alt", "prompt": "Pick many", "multi_select": true }
            ]
        });
        let qs = parse_questions(&input);
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].header, "Choice");
        assert_eq!(qs[0].options, vec!["A".to_string(), "B".to_string()]);
        assert!(!qs[0].multi_select);
        assert_eq!(qs[1].header, "Alt");
        assert_eq!(qs[1].question, "Pick many");
        assert!(qs[1].multi_select);
    }

    #[test]
    fn answers_key_by_question_text() {
        let input =
            json!({"questions": [{"header": "H", "question": "Pick one", "options": ["A", "B"]}]});
        let qs = parse_questions(&input);
        let answers = vec![UserInputAnswer {
            question_id: qs[0].id.clone(),
            labels: vec!["B".into()],
        }];
        let updated = updated_input_with_answers(&input, &qs, &answers);
        assert_eq!(updated["answers"]["Pick one"], json!("B"));
        // Original input is preserved alongside the answers.
        assert!(updated["questions"].is_array());
    }

    fn frame(raw: &str) -> Frame {
        wire::parse_frame(raw).unwrap()
    }

    #[test]
    fn loop_state_round_trips_and_resumes_the_same_conversation() {
        let mut state = ClaudeLoopState::new();
        let started = state.normalizer.normalize(
            frame(r#"{"type":"system","subtype":"init","model":"m","session_id":"s-1"}"#),
            false,
        );
        let AgentEvent::SessionStarted {
            assistant_message_id,
            ..
        } = &started[0]
        else {
            panic!("{started:?}");
        };
        let assistant_message_id = assistant_message_id.clone();
        state.normalizer.normalize(
            frame(
                r#"{"type":"system","subtype":"task_started","task_id":"agent-1","tool_use_id":"toolu_spawn","subagent_type":"general"}"#,
            ),
            false,
        );
        state.pending_steers.push_back("steer-1".into());
        state.open_tools.insert("toolu_open".into());
        let input = json!({"questions": [{"question": "Pick one", "options": ["A", "B"]}]});
        state.parked_questions.push(ParkedQuestion {
            control_request_id: "cr-1".into(),
            questions: parse_questions(&input),
            raw_input: input,
        });

        let text = serde_json::to_value(&state).unwrap();
        let mut back: ClaudeLoopState = serde_json::from_value(text.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), text, "lossless");
        // The same conversation: no second SessionStarted, the same message
        // id, and a background agent's pairing still settles its spawn.
        let again = back.normalizer.normalize(
            frame(r#"{"type":"system","subtype":"init","model":"m","session_id":"s-1"}"#),
            false,
        );
        assert!(again.is_empty(), "{again:?}");
        assert_eq!(back.normalizer.session_id.as_deref(), Some("s-1"));
        assert_eq!(back.normalizer.rotate_for_steer().0, assistant_message_id);
        let settled = back.normalizer.normalize(
            frame(
                r#"{"type":"system","subtype":"task_notification","tool_use_id":"toolu_spawn","status":"completed"}"#,
            ),
            false,
        );
        assert!(
            matches!(&settled[..], [AgentEvent::Subagent { parent_tool_use_id, .. }] if parent_tool_use_id == "toolu_spawn"),
            "{settled:?}"
        );
        assert_eq!(back.parked_questions[0].questions[0].question, "Pick one");
        assert_eq!(
            back.parked_questions[0].questions[0].id, state.parked_questions[0].questions[0].id,
            "question ids survive: the engine rebinds by them"
        );
    }
}

#[cfg(test)]
mod mcp_injection_tests {
    use super::*;

    #[test]
    fn mcp_config_arg_spells_the_server_the_way_the_cli_reads_it() {
        let mcp = zeron_proto::McpServer {
            name: "zeron".into(),
            command: "/opt/zeron/zeron".into(),
            args: vec!["mcp".into()],
            env: [("ZERON_CHAT_ID".to_owned(), "chat-1".to_owned())]
                .into_iter()
                .collect(),
        };
        let parsed: Value = serde_json::from_str(&mcp_config_arg(&mcp)).unwrap();
        assert_eq!(parsed["mcpServers"]["zeron"]["command"], "/opt/zeron/zeron");
        assert_eq!(
            parsed["mcpServers"]["zeron"]["args"],
            serde_json::json!(["mcp"])
        );
        assert_eq!(
            parsed["mcpServers"]["zeron"]["env"]["ZERON_CHAT_ID"],
            "chat-1"
        );
    }
}
