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
//!   permission tool — needs a server process and was rejected. The run's
//!   access mode decides what is asked: full access allows every tool call
//!   (as every run did before modes), auto-accept edits allows the edit
//!   tools and asks before the rest, supervised asks before each; a yes/no
//!   question through [`RunControls::request_input`]. `AskUserQuestion`
//!   round-trips through it too.
//! - ACCESS AND PLAN MODES map onto the CLI's `--permission-mode`
//!   ([`permission_mode`]) and change on the live process with
//!   `set_permission_mode`. Plan mode ends when the person approves
//!   `ExitPlanMode`; the run then continues in its access mode, and the next
//!   turn returns to plan mode while the chat is still in it.
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
//! - A `result` that is not a turn's end: every stdin user line carries a
//!   uuid, and the CLI reports each one's `command_lifecycle`. A result
//!   while one of ours is still written/queued is a boundary (a `now` steer
//!   aborting the turn, resume settling dead background tasks) — that
//!   message's turn ends with its own result. See [`Commands`].
//! - Steering: queued [`SteerMessage`]s are written to stdin as user lines at
//!   any time; the CLI folds them into the running turn at its own step
//!   boundary. A steer's changed model/effort/options apply to the live
//!   process first (`set_model`, `apply_flag_settings`) — never a restart.
//! - Turn stop ([`crate::TurnControl::stop_turn`]): the `interrupt` control
//!   request with `cancel_queued` ends the turn and the CLI keeps reading
//!   stdin. The run opens with an `initialize` declaring
//!   `perTaskStopAffordance`, which makes that interrupt spare background
//!   agents (the CLI otherwise stops them all); background shells survive
//!   either way. `background_tasks_changed` reports the live set to the host.
//! - Interrupt: cancelling [`RunControls::interrupt`] tears the runtime down —
//!   the interrupt control request, then SIGTERM and SIGKILL.

pub mod catalog;
mod discovery;
mod fork_tail;
mod normalize;
pub mod refresh_lock;
mod wire;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SlashCommand,
    SteeringMode, UserInputAnswer, UserInputQuestion,
};

use crate::approval::{self, Action};
use crate::process::owned::Child;
use crate::process::{ChildStdin, Command, Stdio};
use crate::{Harness, HarnessError, RunControls, Signal, send_signal};
use catalog::{apply_ultrathink, to_effort};
use fork_tail::ForkTail;
use normalize::{Normalizer, Transcript};
use wire::{
    ControlRequestFrame, Frame, allow_response, control_error_line, control_response_line,
    deny_response,
};

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
    let found = crate::executable::find_on_paths("claude", extra)?;
    Some(native_beside_npm_shim(&found).unwrap_or(found))
}

/// npm installs Claude Code on Windows as a `claude.cmd` shim, and current
/// packages carry the native CLI beside it. Launched directly, it skips a
/// `cmd.exe` → node hop that mangles arguments (our JSON `--settings` and
/// `--mcp-config` values) and outlives a killed parent.
fn native_beside_npm_shim(shim: &std::path::Path) -> Option<PathBuf> {
    let ext = shim.extension()?.to_str()?;
    if !(ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")) {
        return None;
    }
    let native = shim
        .parent()?
        .join("node_modules")
        .join("@anthropic-ai")
        .join("claude-code")
        .join("bin")
        .join("claude.exe");
    native.is_file().then_some(native)
}

/// Claude's config root for a run: the run's own `CLAUDE_CONFIG_DIR` or
/// `HOME` when its environment sets one, else ours.
fn config_root(env: &std::collections::BTreeMap<String, String>) -> PathBuf {
    if let Some(dir) = env.get("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(home) = env.get("HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(home).join(".claude");
    }
    crate::model_context::root(
        "CLAUDE_CONFIG_DIR",
        crate::executable::home_or_current_dir().join(".claude"),
    )
}

/// An administrator's `managed-mcp.json` (at the paths Claude Code reads).
/// While it exists the CLI refuses `--strict-mcp-config` and exits, and it
/// already keeps every other MCP server out on its own.
fn managed_mcp_config() -> bool {
    let path = if cfg!(target_os = "macos") {
        "/Library/Application Support/ClaudeCode/managed-mcp.json"
    } else if cfg!(windows) {
        r"C:\Program Files\ClaudeCode\managed-mcp.json"
    } else {
        "/etc/claude-code/managed-mcp.json"
    };
    std::path::Path::new(path).is_file()
}

/// The CLI's answer to `--resume` with a session it has no conversation
/// for (it exits right after).
fn no_conversation(text: &str) -> bool {
    text.contains("No conversation found")
}

/// Shown when a chat's Claude session could not be restored.
const SESSION_LOST: &str = "The previous Claude session could not be restored, so this chat \
     continues in a new one without its earlier context.";

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

/// The `--model` value: the 1M context window is selected via a model-id
/// suffix (`sonnet[1m]`), exactly how the CLI itself does it.
fn model_arg(request: &RunRequest) -> Option<String> {
    let model = request.model.as_ref()?;
    let one_m = request
        .model_options
        .get("contextWindow")
        .and_then(Value::as_str)
        == Some("1m");
    Some(if one_m {
        format!("{model}[1m]")
    } else {
        model.clone()
    })
}

/// Every flag-layer setting a request launches with, for
/// `apply_flag_settings` on a live process: `null` drops the override back
/// to the user's own settings, exactly as a launch without the flag
/// (verified live on 2.1.286 through `get_settings`).
fn flag_settings(request: &RunRequest) -> Value {
    let on = |set: bool| if set { Value::Bool(true) } else { Value::Null };
    serde_json::json!({
        "effortLevel": to_effort(request.reasoning, request.model.as_deref()),
        "fastMode": on(option_is_on(&request.model_options, "fastMode")),
        "alwaysThinkingEnabled": on(option_is_on(&request.model_options, "thinking")),
        "ultracode": on(request.reasoning == Some(ReasoningLevel::Ultracode)),
    })
}

fn option_is_on(options: &serde_json::Map<String, Value>, key: &str) -> bool {
    match options.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "on" || s == "true",
        _ => false,
    }
}

/// What a run may do without asking. `auto_approve` keeps meaning full
/// access, whatever the mode says.
fn access(request: &RunRequest) -> AccessMode {
    if request.auto_approve {
        AccessMode::FullAccess
    } else {
        request.access
    }
}

/// The CLI permission mode a run's access mode works in. Full access is
/// `bypassPermissions` only with `auto_approve`, as before modes: without it
/// the run stays in `default` and every tool call is allowed when asked,
/// so no request launches with a skip-permissions flag it did not before
/// (the CLI refuses those as root outside a sandbox).
fn access_permission_mode(request: &RunRequest) -> &'static str {
    if request.auto_approve {
        return "bypassPermissions";
    }
    match request.access {
        AccessMode::AutoAcceptEdits => "acceptEdits",
        AccessMode::Supervised | AccessMode::FullAccess => "default",
    }
}

/// The `--permission-mode` a run launches (or is switched) with: plan mode
/// over its access mode.
fn permission_mode(request: &RunRequest) -> &'static str {
    if request.plan {
        "plan"
    } else {
        access_permission_mode(request)
    }
}

/// How much a CLI permission mode holds back on its own, least first.
fn strictness(mode: &str) -> u8 {
    match mode {
        "bypassPermissions" => 0,
        "acceptEdits" => 1,
        "default" => 2,
        _ => 3, // plan
    }
}

/// Claude's file-changing tools: auto-accept edits allows these unasked.
const EDIT_TOOLS: [&str; 4] = ["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// What a `can_use_tool` request has to ask the person before it is
/// allowed, in `access`; `None` allows it now. `ExitPlanMode` (the plan is
/// ready) asks whether to start implementing it.
fn approval_question(
    tool: &str,
    display_name: Option<&str>,
    description: Option<&str>,
    input: &Value,
    access: AccessMode,
) -> Option<UserInputQuestion> {
    if tool == "ExitPlanMode" {
        return (!access.is_full_access()).then(|| {
            approval::question(
                "Approve plan",
                "Claude wants to start implementing its plan. Allow it?",
            )
        });
    }
    let edit = EDIT_TOOLS.contains(&tool);
    let action = if edit { Action::Edit } else { Action::Other };
    if approval::allowed_without_asking(access, action) {
        return None;
    }
    let field = |key: &str| input.get(key).and_then(Value::as_str);
    Some(if tool == "Bash" {
        approval::command("Claude", field("command").unwrap_or_default())
    } else if edit {
        let paths: Vec<&str> = field("file_path")
            .or_else(|| field("notebook_path"))
            .into_iter()
            .collect();
        approval::edit("Claude", &paths)
    } else {
        let name = display_name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(tool);
        approval::tool("Claude", name, description)
    })
}

/// What a refused approval tells the agent.
const APPROVAL_DENIED: &str = "The user did not allow this.";

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

    /// The configuration arguments a run launches with — everything the
    /// process bakes in at spawn except the session to resume. Two requests
    /// with equal arguments (and cwd) are the same runtime.
    fn launch_args(request: &RunRequest) -> Vec<String> {
        let mut args: Vec<String> = [
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
        ]
        .map(str::to_owned)
        .into();
        if let Some(model) = model_arg(request) {
            args.push("--model".into());
            args.push(model);
        }
        if let Some(effort) = to_effort(request.reasoning, request.model.as_deref()) {
            args.extend(["--effort".into(), effort.into()]);
        }
        args.extend(["--permission-mode".into(), permission_mode(request).into()]);
        if request.auto_approve {
            // Full access. In plan mode the bypass is only made available —
            // the skip flag itself overrides `--permission-mode plan` — so
            // approving the plan can switch to it in place.
            args.push(if request.plan {
                "--allow-dangerously-skip-permissions".into()
            } else {
                "--dangerously-skip-permissions".into()
            });
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
            args.push("--settings".into());
            args.push(Value::Object(settings).to_string());
        }
        args
    }

    fn build_command(exe: &std::path::Path, request: &RunRequest, title_only: bool) -> Command {
        let mut cmd = Command::new(exe);
        // Its own process group, so teardown reaches the MCP servers and
        // tools it started; and no inherited Claude Code session markers.
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, exe);
        // Without it, a foreground subagent's (or forked skill's) text and
        // thinking never reach the stream, and its tab shows tool calls only.
        // The variable rather than `--forward-subagent-text`: a CLI older
        // than 2.1.211 ignores it, but exits on an unknown flag. A `claude`
        // the run starts in turn inherits it. Outside stream-json print
        // mode the CLI ignores it "so that nested invocations keep working
        // when it's set process-wide"
        // (https://code.claude.com/docs/en/env-vars). A nested stream-json
        // `claude` (an Agent SDK script, `claude -p --output-format
        // stream-json`) forwards its own subagents' text too, which only
        // adds tagged frames to its stream.
        cmd.env("CLAUDE_CODE_FORWARD_SUBAGENT_TEXT", "1");
        // Host-resolved environment (a Cloud device's `ANTHROPIC_API_KEY`
        // from the user's own key, `GH_TOKEN`, …). Never a Claude.ai
        // subscription token: those stay inside the CLI's own login.
        crate::apply_run_env(&mut cmd, &request.env);
        cmd.args(Self::launch_args(request));
        if let Some(resume) = &request.resume {
            cmd.arg(format!("--resume={resume}"));
        }
        if title_only {
            cmd.args([
                "--system-prompt",
                crate::TITLE_INSTRUCTIONS,
                "--tools",
                "",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--setting-sources",
                "",
            ]);
            if !managed_mcp_config() {
                cmd.arg("--strict-mcp-config");
            }
        } else if let Some(mcp) = &request.mcp {
            // Zeron's own server rides beside the user's configured servers
            // (no `--strict-mcp-config`): the CLI merges an inline JSON
            // config with settings-sourced ones.
            cmd.args(["--mcp-config", &mcp_config_arg(mcp)]);
            // Delegation goes through that server's side chats only. A bare
            // tool name "removes the matching tools from Claude's context"
            // (https://code.claude.com/docs/en/cli-reference), so the model
            // never calls them: a call refused at run time would stay
            // running (#893). The model calls the spawn tool `Agent`, the
            // init frame lists it as `Task`, and `Workflow` runs agents of
            // its own; with the rule, the init frame lists neither `Task`
            // nor `Workflow` (live-verified 2.1.310).
            if mcp.side_chats_only() {
                cmd.arg("--disallowedTools=Agent,Task,Workflow");
            }
        }
        if !request.cwd.is_empty() {
            cmd.current_dir(&request.cwd);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    /// Start one CLI process for a run, with its stdio wired up.
    fn launch(
        exe: &std::path::Path,
        request: &RunRequest,
        title_only: bool,
    ) -> Result<Runtime, HarnessError> {
        let mut child = Self::build_command(exe, request, title_only)
            .spawn()
            .map_err(|e| {
                let cwd = (!request.cwd.is_empty()).then(|| std::path::Path::new(&request.cwd));
                crate::executable::spawn_error(e, cwd, || crate::executable::binary_hint(exe))
            })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Child::new(child);
        let stdin =
            stdin.ok_or_else(|| HarnessError::Protocol("claude child has no stdin".into()))?;
        let stdout =
            stdout.ok_or_else(|| HarnessError::Protocol("claude child has no stdout".into()))?;
        let stderr_tail = crate::StderrTail::default();
        if let Some(stderr) = stderr {
            tokio::spawn(read_stderr(stderr, stderr_tail.clone()));
        }
        Ok(Runtime {
            child,
            stdout_lines: crate::lossy_lines::lossy_lines(stdout),
            stdin: Stdin::new(
                stdin,
                request.resume.is_some(),
                refresh_lock::config_dir_for(&request.env),
            ),
            stderr_tail,
        })
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
        crate::process::owned::configure(&mut cmd);
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
            // Discovery runs on every catalog refresh and workspace open:
            // the user's SessionStart hooks must not fire for it. MCP
            // servers stay — their prompts are commands too.
            "--settings",
            r#"{"disableAllHooks":true}"#,
        ]);
        // Nor may it attach to (or install into) a running IDE.
        cmd.env("CLAUDE_CODE_AUTO_CONNECT_IDE", "0")
            .env("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            crate::executable::spawn_error(e, cwd, || crate::executable::binary_hint(&exe))
        })?;
        let stdio = (child.stdin.take(), child.stdout.take());
        let stderr_tail = crate::StderrTail::default();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(read_stderr(stderr, stderr_tail.clone()));
        }
        let mut child = Child::new(child);
        let (Some(mut stdin), Some(stdout)) = stdio else {
            refresh_lock::settle(&refresh_lock::config_dir(), refresh_lock::SETTLE_MAX).await;
            child.shutdown(self.kill_grace).await;
            return Err(HarnessError::Protocol("claude child has no stdio".into()));
        };
        const PROBE_ID: &str = "zeron-command-probe";
        let discovery = async {
            let request = serde_json::json!({
                "type": "control_request",
                "request_id": PROBE_ID,
                "request": { "subtype": "initialize" },
            });
            // A CLI that fails on startup may exit before reading: the write
            // then fails (EPIPE), and that is the same exit before answering
            // — read on to its stdout's end and report its status and stderr.
            let sent = match stdin.write_all(format!("{request}\n").as_bytes()).await {
                Ok(()) => stdin.flush().await,
                Err(error) => Err(error),
            };
            if let Err(error) = sent {
                tracing::debug!(
                    target: "zeron_harness::claude",
                    %error,
                    "initialize request not delivered to the discovery probe"
                );
            }
            let mut lines = crate::lossy_lines::lossy_lines(stdout);
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
                return Ok(Some(response));
            }
            Ok(None)
        };
        let result = tokio::time::timeout(PROBE_TIMEOUT, discovery).await;
        let result = match result {
            Ok(Ok(Some(response))) => Ok(response),
            Ok(Ok(None)) => {
                // Exited before answering: why is in its status and stderr.
                let status = tokio::time::timeout(Duration::from_millis(200), child.wait())
                    .await
                    .ok()
                    .and_then(Result::ok);
                stderr_tail.wait_closed().await;
                Err(HarnessError::Protocol(format!(
                    "{} before answering the initialize control request",
                    crate::crash_message("claude", status, &stderr_tail)
                )))
            }
            Ok(Err(error)) => Err(error),
            // A CLI stuck on its own startup (an auth prompt, a broken
            // config) usually says so on stderr.
            Err(_) => Err(HarnessError::Protocol(probe_timeout_message(
                stderr_tail.snapshot(),
            ))),
        };
        // A probe starts right when an expired login gets refreshed, and
        // answers before that refresh is saved: retire it in the background
        // once no refresh is in flight, stdin still open (EOF would make the
        // CLI exit after waiting only 2s for the save — see `refresh_lock`).
        let kill_grace = self.kill_grace;
        let lock_dir = refresh_lock::config_dir();
        tokio::spawn(async move {
            refresh_lock::settle(&lock_dir, refresh_lock::SETTLE_MAX).await;
            drop(stdin);
            child.shutdown(kill_grace).await;
        });
        result
    }
}

/// How long the discovery probe waits: a cold start on Windows or behind
/// Bedrock can take well over 10s.
const PROBE_TIMEOUT: Duration = Duration::from_secs(25);

/// The discovery probe's timeout, with what the CLI printed (redacted).
fn probe_timeout_message(stderr: Option<String>) -> String {
    let timed_out = format!(
        "Claude initialize timed out after {}s",
        PROBE_TIMEOUT.as_secs()
    );
    match stderr {
        Some(tail) => format!("{timed_out}: {}", crate::redact_secrets(&tail)),
        None => timed_out,
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
    /// The `interrupt` control request ends the turn and the CLI keeps
    /// reading stdin; background agents and shells run on.
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    /// Same launch arguments, cwd and access. The CLI ignores the sandbox
    /// level, and option spellings that launch the same flags are the same
    /// runtime; supervised and full access without `auto_approve` launch
    /// alike but answer tool calls differently.
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && Self::launch_args(live) == Self::launch_args(next)
            && access(live) == access(next)
    }
    /// Model, context window, effort, fast mode, thinking, ultracode, the
    /// access mode and plan mode all change on the live process
    /// (`set_model`, `apply_flag_settings`, `set_permission_mode`). The
    /// directory and `auto_approve` need a new one — the CLI switches to
    /// `bypassPermissions` only if it launched able to — and so does a
    /// stricter permission mode: the prompt follows the switch on stdin, so
    /// a switch the CLI refused would run it with less asked than chosen.
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && live.auto_approve == next.auto_approve
            && strictness(permission_mode(next)) <= strictness(permission_mode(live))
    }
    fn access_modes(&self) -> &[AccessMode] {
        &AccessMode::ALL
    }
    /// The CLI's own plan mode; approving `ExitPlanMode` ends it.
    fn supports_plan(&self) -> bool {
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
        // Its tools are off and denied: the chat's modes do not apply.
        request.access = AccessMode::default();
        request.plan = false;
        self.run_with_mode(request, controls, true).await
    }
}

impl ClaudeHarness {
    async fn run_with_mode(
        &self,
        mut request: RunRequest,
        controls: RunControls,
        title_only: bool,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let exe = self.resolve_executable()?;
        // A session whose transcript holds no conversation cannot be
        // resumed: the CLI says so and exits, every time. Start fresh
        // instead — the chat's history is gone either way, and the user is
        // told. A transcript not found here is still resumed: the CLI may
        // keep it elsewhere, and says so when it does not.
        let mut notice = None;
        let normalizer = match request.resume.clone() {
            Some(session_id) => {
                let (normalizer, transcript) =
                    Normalizer::for_resume(&config_root(&request.env), &session_id).await;
                if transcript == Transcript::Empty {
                    tracing::info!(
                        target: "zeron_harness::claude",
                        %session_id,
                        "no conversation to resume; starting a new session"
                    );
                    request.resume = None;
                    notice = Some(SESSION_LOST.to_owned());
                }
                normalizer
            }
            None => Normalizer::new(),
        };
        let Runtime {
            child,
            stdout_lines,
            mut stdin,
            stderr_tail,
        } = Self::launch(&exe, &request, title_only)?;

        let lock_dir = refresh_lock::config_dir_for(&request.env);
        if !title_only {
            stdin.line(wire::initialize_request_line("zeron_initialize"));
        }

        // The initial prompt as the first stdin user line (streaming-input
        // mode). Ultrathink rides every user message — steers included.
        // Staged image attachments are inlined as base64 image content blocks
        // ahead of the text (verified against the real CLI); their path refs
        // also ride the prompt text, so a skipped/unreadable file degrades to
        // the old-app behavior (the agent opens the path with its Read tool).
        let images = load_image_blocks(&request.attachments).await;
        let first_command = uuid::Uuid::new_v4().to_string();
        let first = wire::prompt_line(
            &apply_ultrathink(request.reasoning, &request.prompt),
            &first_command,
            &images,
        );
        stdin.line(first);

        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        if let Some(message) = notice {
            let _ = event_tx.try_send(Ok(AgentEvent::Error { message }));
        }
        tokio::spawn(run_session(Session {
            normalizer,
            title_only,
            exe,
            launched: request.clone(),
            child,
            stdout_lines,
            first_command,
            stdin,
            event_tx,
            controls,
            config: request,
            interrupt_grace: self.interrupt_grace,
            kill_grace: self.kill_grace,
            lock_dir,
            stderr_tail,
        }));

        Ok(futures::stream::unfold(event_rx, |mut rx| async move {
            rx.recv().await.map(|ev| (ev, rx))
        })
        .boxed())
    }
}

/// Forward a child's stderr to the debug log and the crash-message tail,
/// and say when it closed: an exit's last lines land just after stdout's.
async fn read_stderr(stderr: impl tokio::io::AsyncRead + Unpin, tail: crate::StderrTail) {
    let mut lines = crate::lossy_lines::lossy_lines(stderr);
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "zeron_harness::claude", "stderr: {line}");
        tail.push(&line);
    }
    tail.close();
}

/// One launched CLI process and its stdio.
struct Runtime {
    child: Child,
    stdout_lines: crate::lossy_lines::LossyLines<crate::process::ChildStdout>,
    stdin: Stdin,
    stderr_tail: crate::StderrTail,
}

/// The run's stdin, through its writer task. Until the CLI reports its
/// session, a resuming run keeps a copy of everything it wrote: when the
/// session cannot be restored, the fresh process is told the same things.
struct Stdin {
    tx: mpsc::UnboundedSender<StdinMsg>,
    replay: Option<Vec<StdinMsg>>,
}

impl Stdin {
    fn new(stdin: ChildStdin, keep_for_replay: bool, lock_dir: PathBuf) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(stdin_writer(stdin, rx, lock_dir));
        Self {
            tx,
            replay: keep_for_replay.then(Vec::new),
        }
    }

    /// False once the writer is gone (the child died).
    fn send(&mut self, msg: StdinMsg) -> bool {
        if let Some(replay) = &mut self.replay {
            replay.push(msg.clone());
        }
        self.tx.send(msg).is_ok()
    }

    fn line(&mut self, line: String) -> bool {
        self.send(StdinMsg::Line(line))
    }

    /// Before the CLI's teardown: Windows' gives it its grace to exit on
    /// stdin EOF, which the writer holds off while any sender (a pending
    /// permission answer's) is alive. Unix teardown signals instead; its
    /// order is unchanged.
    fn close_for_teardown(&self) {
        #[cfg(windows)]
        let _ = self.tx.send(StdinMsg::Close);
    }
}

#[derive(Clone)]
enum StdinMsg {
    Line(String),
    /// Close stdin (end of steering input): the CLI finishes the current turn
    /// and exits, which ends the run stream at stdout EOF.
    Close,
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

/// Owns the child's stdin; a write failure (EPIPE after the child died) is
/// tolerated and logged. EOF makes the CLI exit, so it waits out a login
/// refresh in flight first (see [`refresh_lock`]).
async fn stdin_writer(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<StdinMsg>,
    lock_dir: PathBuf,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            StdinMsg::Line(line) => {
                let write = async {
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.write_all(b"\n").await?;
                    stdin.flush().await
                };
                if let Err(e) = write.await {
                    tracing::debug!(target: "zeron_harness::claude", "stdin write failed (tolerated): {e}");
                    return;
                }
            }
            StdinMsg::Close => {
                refresh_lock::settle(&lock_dir, refresh_lock::SETTLE_MAX).await;
                let _ = stdin.shutdown().await;
                return;
            }
        }
    }
}

struct Session {
    normalizer: Normalizer,
    title_only: bool,
    /// What the process was launched from, for a fresh relaunch when its
    /// session cannot be restored.
    exe: PathBuf,
    launched: RunRequest,
    child: Child,
    stdout_lines: crate::lossy_lines::LossyLines<crate::process::ChildStdout>,
    /// The uuid the opening prompt was written with.
    first_command: String,
    stdin: Stdin,
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    controls: RunControls,
    /// The configuration the process runs with now: its launch request, then
    /// whatever a steer's `config` applied in place.
    config: RunRequest,
    interrupt_grace: Duration,
    kill_grace: Duration,
    /// Claude's credential dir: no signal lands mid-refresh (see
    /// [`refresh_lock`]).
    lock_dir: PathBuf,
    /// Rolling stderr tail for the crash message on an unexpected exit.
    stderr_tail: crate::StderrTail,
}

/// The per-run event loop: one task multiplexing stdout frames, the steering
/// mailbox, the interrupt token, and consumer liveness.
async fn run_session(session: Session) {
    let Session {
        normalizer: mut norm,
        title_only,
        exe,
        launched,
        mut child,
        mut stdout_lines,
        first_command,
        mut stdin,
        event_tx,
        controls,
        mut config,
        interrupt_grace,
        kill_grace,
        lock_dir,
        mut stderr_tail,
    } = session;
    let RunControls {
        realtime: _,
        execution_lease: _execution_lease,
        request_input,
        mut steering,
        interrupt,
        turn,
    } = controls;
    let request_input = Arc::new(request_input);

    let mut pending_steers = std::collections::VecDeque::new();
    let mut reconfigures = 0u64;
    let mut mode_switches = 0u64;
    // Top-level tool calls in flight: a steer must not abort them (see
    // `wire::steer_message_line`).
    let mut open_tools = std::collections::HashSet::new();
    let mut steering_open = true;
    let mut interrupted = false;
    let mut interrupt_sent = false;
    // A message of ours (or a background wake) began a turn that has not
    // reported its end: the CLI dying now leaves it to us to end it.
    let mut turn_open = true;
    // A resumed session the CLI turns out to have no conversation for is
    // relaunched fresh, once (see [`no_conversation`]).
    let mut restore_fallback = launched.resume.is_some();
    let mut relaunch = false;
    // Our configuration requests the CLI has not answered yet (`set_model`,
    // `apply_flag_settings`): what each asked for, and when we stop waiting.
    const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
    let mut pending_controls: std::collections::HashMap<
        String,
        (&'static str, Value, tokio::time::Instant),
    > = std::collections::HashMap::new();
    // A configuration request failed or went unanswered: the live process
    // may not run what `config` says, so the next change re-sends it all.
    let mut config_unsure = false;
    // What the CLI last refused, per request kind. Every message carrying
    // the same configuration asks again; its refusal was already shown, and
    // is shown again only once the ask changes or has succeeded.
    let mut refused: std::collections::HashMap<&'static str, Value> =
        std::collections::HashMap::new();
    // The permission mode the CLI last reported (`init`, `status`). It
    // leaves plan mode on its own when the plan is approved.
    let mut cli_mode: Option<String> = None;
    // A turn stop in flight (`TurnControl::stop_turn`): its request id, and
    // once the CLI has answered, how long a turn-ending `result` may still
    // take. The CLI answers a stop between turns without any result — the
    // deadline settles that shape so the host never waits on it.
    const STOP_SETTLE: Duration = Duration::from_secs(3);
    let mut stops_sent = 0u64;
    let mut stopping: Option<String> = None;
    let mut stop_settle_at: Option<tokio::time::Instant> = None;
    // A turn end held for steers when the stop came: that turn's end, sent
    // as stopped once the stop settles — never before. Released early, the
    // host started the replacement prompt while the stopped turn could still
    // report, and that late result settled the replacement.
    let mut stopped_end: Option<AgentEvent> = None;
    // When the stopped turn's one Done went out, and whether anything has
    // happened since — a message of ours written, or new output streamed. A
    // result in that window with neither can only be the stopped turn
    // reporting late, and is dropped. Anything else is reconciled like any
    // result (held for unconfirmed messages, settled by their fallback):
    // dropping on time alone lost a quick replacement's own end.
    let mut stop_done_at: Option<tokio::time::Instant> = None;
    let mut active_since_stop = false;
    let mut commands = Commands::default();
    commands.written(first_command);
    // A `result` that is a steer boundary rather than the turn's end (see
    // [`Commands`]). It is superseded — dropped — once the waiting message
    // starts its own turn, which ends with its own result; it is released
    // as the turn's end if that message is cancelled before it starts.
    let mut held_done: Option<AgentEvent> = None;
    // Last stdout activity, for the held result's settle fallback.
    let mut last_frame_at = tokio::time::Instant::now();
    let mut done_after_interrupt = false;
    let mut escalation: Option<tokio::task::JoinHandle<()>> = None;
    // The runtime's process tree when its teardown began (see
    // [`crate::shutdown_agent`]).
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut torn_down: Vec<crate::process::Tracked> = Vec::new();
    // The typed fork whose transcript is read off disk (see [`fork_tail`]).
    let claude_root = config_root(&launched.env);
    let mut tail: Option<ForkTail> = None;

    'main: loop {
        if std::mem::take(&mut relaunch) {
            // The CLI had no conversation for the session and exited
            // without starting one. Everything written to it goes to a fresh
            // process; its own init reports the new session id.
            let replay = stdin.replay.take().unwrap_or_default();
            if matches!(child.try_wait(), Ok(None)) {
                refresh_lock::settle(&lock_dir, refresh_lock::SETTLE_MAX).await;
            }
            stdin.close_for_teardown();
            child.shutdown(kill_grace).await;
            let mut fresh = launched.clone();
            fresh.resume = None;
            match ClaudeHarness::launch(&exe, &fresh, title_only) {
                Ok(runtime) => {
                    child = runtime.child;
                    stdout_lines = runtime.stdout_lines;
                    stdin = runtime.stdin;
                    stderr_tail = runtime.stderr_tail;
                }
                Err(error) => {
                    let _ = event_tx.send(Err(error)).await;
                    break 'main;
                }
            }
            for msg in replay {
                if !stdin.send(msg) {
                    break;
                }
            }
            if event_tx
                .send(Ok(AgentEvent::Error {
                    message: SESSION_LOST.to_owned(),
                }))
                .await
                .is_err()
            {
                break 'main;
            }
        }
        let control_deadline = pending_controls.values().map(|(_, _, at)| *at).min();
        match norm.fork_to_follow() {
            Some(fork) if tail.as_ref().is_none_or(|t| t.chip != fork.0) => {
                tail = Some(ForkTail::new(&claude_root, fork));
            }
            Some(_) => {}
            None => tail = None,
        }
        let tail_at = tail.as_ref().and_then(ForkTail::next_read);
        tokio::select! {
            line = stdout_lines.next_line() => match line {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    // The CLI is still producing: whatever it is doing is not
                    // the quiet end a held turn end's fallback waits for.
                    last_frame_at = tokio::time::Instant::now();
                    let frame = match wire::parse_frame(line) {
                        Ok(frame) => frame,
                        Err(e) => {
                            tracing::debug!(target: "zeron_harness::claude", "unparseable frame (skipped): {e}");
                            continue;
                        }
                    };
                    if restore_fallback
                        && !interrupted
                        && !norm.started()
                        && let Frame::Result(result) = &frame
                        && result
                            .errors
                            .iter()
                            .any(|e| e.as_str().is_some_and(no_conversation))
                    {
                        tracing::info!(
                            target: "zeron_harness::claude",
                            session = ?launched.resume,
                            "session could not be restored; relaunching fresh"
                        );
                        restore_fallback = false;
                        relaunch = true;
                        continue;
                    }
                    if let Frame::ControlResponse(response) = &frame {
                        let response = &response.response;
                        if stopping.as_deref() == Some(response.request_id.as_str()) {
                            stop_settle_at = Some(tokio::time::Instant::now() + STOP_SETTLE);
                        }
                        let error = response.error_message();
                        if let Some((what, asked, _)) = pending_controls.remove(&response.request_id) {
                            if let Some(error) = &error {
                                config_unsure = true;
                                if refused.get(what) != Some(&asked) {
                                    refused.insert(what, asked);
                                    if event_tx.send(Ok(AgentEvent::Error {
                                        message: format!("Claude could not change its {what}: {error}"),
                                    })).await.is_err() { break 'main; }
                                }
                            } else {
                                refused.remove(what);
                            }
                        } else if let Some(error) = error {
                            tracing::warn!(
                                target: "zeron_harness::claude",
                                request = %response.request_id,
                                "control request failed: {error}"
                            );
                        }
                        continue;
                    }
                    if let Frame::System(system) = &frame {
                        if let Some(mode) = &system.permission_mode {
                            cli_mode = Some(mode.clone());
                        }
                        match system.subtype.as_str() {
                            "background_tasks_changed" => {
                                turn.set_background(system.tasks.as_ref().map_or(0, Vec::len));
                            }
                            // The first, or a background wake's: either way a
                            // turn is running.
                            "init" => turn_open = true,
                            _ => {}
                        }
                    }
                    if let Frame::ControlRequest(req) = frame {
                        handle_control_request(req, title_only, &config, &request_input, &stdin.tx);
                        continue;
                    }
                    // Only the CLI confirms that a prompt joined its
                    // conversation — its lifecycle or its replay, whichever
                    // comes first. Writing stdin must not split ongoing text.
                    // A confirmation covers its steer and every earlier one:
                    // superseded steers are never replayed themselves. A
                    // message a turn took up (started, or replayed) also
                    // supersedes a result held for it: that result was only
                    // the boundary, and the message's turn ends with its own.
                    let (confirmed, taken_up) = match &frame {
                        Frame::CommandLifecycle(cmd) => {
                            let started = commands.lifecycle(&cmd.command_uuid, &cmd.state);
                            let confirmed = started || Commands::is_terminal(&cmd.state);
                            (confirmed.then(|| cmd.command_uuid.clone()), started)
                        }
                        Frame::User(user) if user.parent_tool_use_id.is_none() => {
                            let replayed = user.uuid.clone().filter(|id| pending_steers.contains(id));
                            let taken_up = replayed.is_some();
                            (replayed, taken_up)
                        }
                        _ => (None, false),
                    };
                    if taken_up {
                        held_done = None;
                        active_since_stop = true;
                    }
                    if let Some(at) = confirmed
                        .and_then(|id| pending_steers.iter().position(|p| *p == id))
                    {
                        for _ in 0..=at {
                            pending_steers.pop_front();
                            let (prev, next) = norm.rotate_for_steer();
                            if event_tx.send(Ok(AgentEvent::Steered {
                                assistant_message_id: Some(prev), next_assistant_message_id: Some(next),
                            })).await.is_err() { break 'main; }
                        }
                    }
                    // Every waiting message was cancelled before it started:
                    // the held result was the turn's real end after all.
                    if matches!(frame, Frame::CommandLifecycle(_))
                        && !commands.waiting()
                        && let Some(done) = held_done.take()
                    {
                        if event_tx.send(Ok(done)).await.is_err() {
                            break 'main;
                        }
                        turn_open = false;
                    }
                    // The stopped turn reporting late: a result with nothing
                    // since the stop's Done. Judged on the frame, before it
                    // expands — its own Usage is not new activity — and
                    // dropped whole, usage included.
                    if matches!(frame, Frame::Result(_))
                        && !interrupted
                        && stopping.is_none()
                        && !active_since_stop
                        && stop_done_at.is_some_and(|at| at.elapsed() < STOP_SETTLE)
                    {
                        tracing::debug!(
                            target: "zeron_harness::claude",
                            "late result of a stopped turn dropped"
                        );
                        continue;
                    }
                    let events = norm.normalize(frame, interrupted || stopping.is_some());
                    // The session is the CLI's now: nothing left to replay.
                    if norm.started() {
                        restore_fallback = false;
                        stdin.replay = None;
                    }
                    for ev in events {
                        match &ev {
                            AgentEvent::ToolCall { id, .. } => {
                                open_tools.insert(id.clone());
                            }
                            AgentEvent::ToolResult { id, .. } => {
                                open_tools.remove(id);
                            }
                            AgentEvent::Done { .. } => open_tools.clear(),
                            _ => {}
                        }
                        let is_done = matches!(ev, AgentEvent::Done { .. });
                        // A `now` steer ends the turn it interrupts with a
                        // result frame, and a message queued behind a turn
                        // that just ended starts another: either way that
                        // result is a steer boundary, not the end of the turn.
                        let boundary = if commands.reported {
                            commands.waiting()
                        } else {
                            !pending_steers.is_empty()
                        };
                        // New content is a new turn's: its result is its own.
                        if stop_done_at.is_some()
                            && match &ev {
                                AgentEvent::TextDelta { text }
                                | AgentEvent::ReasoningDelta { text } => !text.is_empty(),
                                AgentEvent::ToolCall { .. }
                                | AgentEvent::ToolResult { .. }
                                | AgentEvent::InputRequested { .. }
                                | AgentEvent::Subagent { .. } => true,
                                _ => false,
                            }
                        {
                            active_since_stop = true;
                        }
                        if is_done && !interrupted && stopping.is_none() && boundary {
                            held_done = Some(ev);
                            continue;
                        }
                        if is_done {
                            held_done = None;
                            // The stopped turn has ended; the runtime lives on.
                            if stopping.take().is_some() {
                                stopped_end = None;
                                stop_done_at = Some(tokio::time::Instant::now());
                                active_since_stop = false;
                            }
                            stop_settle_at = None;
                        }
                        if event_tx.send(Ok(ev)).await.is_err() {
                            break 'main; // consumer gone — reap below
                        }
                        if is_done {
                            turn_open = false;
                            if interrupted {
                                done_after_interrupt = true;
                                break 'main;
                            }
                        }
                    }
                }
                // stdout EOF: the CLI exited. An older CLI says it has no
                // conversation for the session on stderr alone.
                Ok(None) => {
                    if restore_fallback && !interrupted && !norm.started() {
                        stderr_tail.wait_closed().await;
                        if stderr_tail.snapshot().is_some_and(|tail| no_conversation(&tail)) {
                            restore_fallback = false;
                            relaunch = true;
                            continue;
                        }
                    }
                    break 'main;
                }
                Err(e) => {
                    let _ = event_tx.send(Err(HarnessError::Io(e))).await;
                    break 'main;
                }
            },

            // The typed fork's records, as it writes them.
            _ = tokio::time::sleep_until(tail_at.unwrap_or_else(tokio::time::Instant::now)),
                if tail_at.is_some() => {
                let records = tail.as_mut().map_or_else(Vec::new, ForkTail::read);
                for ev in records.into_iter().flat_map(|record| norm.fork_record(record)) {
                    if event_tx.send(Ok(ev)).await.is_err() {
                        break 'main;
                    }
                }
            },

            steer = steering.recv(), if steering_open && !interrupted => match steer {
                Some(msg) => {
                    // A message of ours now owns whatever result comes next.
                    active_since_stop = true;
                    // A changed configuration applies before the prompt, on
                    // the same stdin the CLI reads in order.
                    // A message starting a turn also puts back a plan mode
                    // the CLI left when its last plan was approved.
                    let mut switch_mode = !turn_open
                        && cli_mode.as_deref().is_some_and(|mode| mode != permission_mode(&config));
                    if let Some(next) = msg.config {
                        reconfigures += 1;
                        let resend = std::mem::take(&mut config_unsure);
                        let deadline = tokio::time::Instant::now() + CONTROL_TIMEOUT;
                        switch_mode |= resend || permission_mode(&next) != permission_mode(&config);
                        if resend || model_arg(&next) != model_arg(&config) {
                            let id = format!("zeron_model_{reconfigures}");
                            let model = model_arg(&next);
                            let line = wire::set_model_request_line(&id, model.as_deref());
                            pending_controls.insert(id, ("model", Value::from(model), deadline));
                            if !stdin.line(line) { break 'main; }
                        }
                        if resend || flag_settings(&next) != flag_settings(&config) {
                            let id = format!("zeron_settings_{reconfigures}");
                            let settings = flag_settings(&next);
                            let line = wire::apply_flag_settings_line(&id, settings.clone());
                            pending_controls.insert(id, ("settings", settings, deadline));
                            if !stdin.line(line) { break 'main; }
                        }
                        config = *next;
                    }
                    if switch_mode {
                        mode_switches += 1;
                        let id = format!("zeron_mode_{mode_switches}");
                        let mode = permission_mode(&config);
                        let deadline = tokio::time::Instant::now() + CONTROL_TIMEOUT;
                        let line = wire::set_permission_mode_line(&id, mode);
                        pending_controls.insert(id, ("permission mode", Value::from(mode), deadline));
                        if !stdin.line(line) { break 'main; }
                    }
                    let id = uuid::Uuid::new_v4().to_string();
                    let images = load_image_blocks(&msg.attachments).await;
                    let line = wire::steer_message_line_with_images(
                        &apply_ultrathink(config.reasoning, &msg.prompt),
                        &id,
                        open_tools.is_empty(),
                        &images,
                    );
                    pending_steers.push_back(id.clone());
                    commands.written(id);
                    turn_open = true;
                    if !stdin.line(line) { break 'main; }
                }
                None => {
                    // Mailbox closed: end the input so the run can finish
                    // after the current turn.
                    steering_open = false;
                    stdin.send(StdinMsg::Close);
                }
            },

            // End the in-flight turn, not the session: the CLI stays up, so
            // its background agents and shells keep running. Steers written
            // before the stop are cancelled with it (`cancel_queued`) — a
            // stopped turn must not continue into them.
            _ = turn.stop_requested(), if !interrupted => {
                stops_sent += 1;
                let id = format!("zeron_stop_{stops_sent}");
                pending_steers.clear();
                commands.cancel_waiting();
                stopping = Some(id.clone());
                stop_settle_at = None;
                stop_done_at = None;
                if !stdin.line(wire::stop_turn_request_line(&id)) {
                    break 'main;
                }
                // A turn end held for steer replays is that turn's real end:
                // its steers were just cancelled, so it ends stopped — once
                // the stop settles. A turn the CLI had already begun for
                // those steers reports first, and that report is the end.
                if let Some(mut done) = held_done.take() {
                    if let AgentEvent::Done { status, .. } = &mut done {
                        *status = DoneStatus::Interrupted;
                    }
                    stopped_end = Some(done);
                }
            },

            // The CLI answered the stop and no turn ended: it was between
            // turns. Settle the stop so the host sees the turn over.
            _ = tokio::time::sleep_until(stop_settle_at.unwrap_or_else(tokio::time::Instant::now)),
                if stop_settle_at.is_some() => {
                stop_settle_at = None;
                if stopping.take().is_some() {
                    let done = stopped_end.take().unwrap_or(AgentEvent::Done {
                        status: DoneStatus::Interrupted,
                        result: None,
                        error: None,
                        session_id: norm.session_id.clone(),
                    });
                    // No result came to settle a typed fork: it ends here,
                    // with the turn, or its chip runs on in the parked run.
                    for ev in norm.settle_typed_fork(DoneStatus::Interrupted) {
                        if event_tx.send(Ok(ev)).await.is_err() {
                            break 'main;
                        }
                    }
                    if event_tx.send(Ok(done)).await.is_err() {
                        break 'main;
                    }
                    // The turn's tools ended with it, as on a normalized
                    // Done: none may hold the next message back (`next`).
                    open_tools.clear();
                    turn_open = false;
                    stop_done_at = Some(tokio::time::Instant::now());
                    active_since_stop = false;
                }
            },

            _ = interrupt.cancelled(), if !interrupt_sent => {
                interrupt_sent = true;
                interrupted = true;
                // The runtime is going: so is everything it started.
                #[cfg(unix)]
                if let Some(pid) = child.id() {
                    torn_down = crate::process::descendants(pid).await;
                }
                stdin.line(wire::interrupt_request_line("int_1"));
                // Escalate if the CLI doesn't wind down within the grace
                // periods: SIGTERM (kills bash trees, runs SessionEnd hooks),
                // then SIGKILL — neither while a login refresh is being
                // saved. Aborted once the child is reaped.
                if let Some(pid) = crate::process::signal_target(&child) {
                    let lock_dir = lock_dir.clone();
                    escalation = Some(tokio::spawn(async move {
                        tokio::time::sleep(interrupt_grace).await;
                        refresh_lock::settle(&lock_dir, refresh_lock::SETTLE_MAX).await;
                        send_signal(&pid, Signal::Term);
                        tokio::time::sleep(kill_grace).await;
                        refresh_lock::settle(&lock_dir, refresh_lock::SETTLE_MAX).await;
                        send_signal(&pid, Signal::Kill);
                    }));
                }
            },

            _ = tokio::time::sleep_until(
                last_frame_at + commands.held_settle().unwrap_or_default()
            ), if held_done.is_some() && commands.held_settle().is_some() => {
                // The steers were absorbed into the turn that just ended.
                commands.cancel_waiting();
                while pending_steers.pop_front().is_some() {
                    let (prev, next) = norm.rotate_for_steer();
                    if event_tx.send(Ok(AgentEvent::Steered {
                        assistant_message_id: Some(prev), next_assistant_message_id: Some(next),
                    })).await.is_err() { break 'main; }
                }
                let done = held_done.take().expect("guarded by if");
                if event_tx.send(Ok(done)).await.is_err() {
                    break 'main;
                }
                turn_open = false;
            },

            // A configuration request the CLI never answered: assume the
            // worst, so the next change re-sends everything.
            _ = tokio::time::sleep_until(control_deadline.unwrap_or_else(tokio::time::Instant::now)),
                if control_deadline.is_some() => {
                let now = tokio::time::Instant::now();
                pending_controls.retain(|id, (_, _, at)| {
                    let waiting = *at > now;
                    if !waiting {
                        tracing::warn!(target: "zeron_harness::claude", request = %id, "control request unanswered");
                    }
                    waiting
                });
                config_unsure = true;
            },

            _ = event_tx.closed() => break 'main,
        }
    }

    // A turn end still held when the CLI exited is the run's real end.
    if let Some(done) = held_done.take()
        && !event_tx.is_closed()
        && event_tx.send(Ok(done)).await.is_ok()
    {
        turn_open = false;
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
                    session_id: norm.session_id.clone(),
                }))
                .await;
        } else if !interrupted && turn_open {
            // The CLI died mid-turn — the first or any later one. Its
            // tools and MCP servers are traced while it can still be; its
            // exit status and last stderr lines land just after stdout
            // closes, and say why.
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                for pid in crate::process::descendants(pid).await {
                    if !torn_down.contains(&pid) {
                        torn_down.push(pid);
                    }
                }
            }
            let status = tokio::time::timeout(Duration::from_millis(200), child.wait())
                .await
                .ok()
                .and_then(Result::ok);
            stderr_tail.wait_closed().await;
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(crate::crash_message("claude", status, &stderr_tail)),
                    session_id: norm.session_id.clone(),
                }))
                .await;
        }
    }

    // Its whole process group goes with it, and whatever was traced
    // before — on every exit path, not only an interrupt; never while a
    // login refresh is being saved.
    if matches!(child.try_wait(), Ok(None)) {
        refresh_lock::settle(&lock_dir, refresh_lock::SETTLE_MAX).await;
    }
    stdin.close_for_teardown();
    child.shutdown_with(torn_down, kill_grace).await;
    if let Some(handle) = escalation {
        handle.abort();
    }
}

/// How far the CLI has taken each stdin user message we wrote, by the uuid
/// it was written with. A `result` frame ends the TURN only when no message
/// is still waiting to be taken up. A waiting one makes the result a steer
/// boundary — a `now` steer aborting the turn it interrupts, or a message
/// queued behind a turn that just finished — and that message's own turn
/// ends with its own result. Deciding this from the CLI's own lifecycle
/// rather than from quiet time: a held result released by a quiet timer
/// marked the steered turn done mid-way through any long tool call.
#[derive(Default)]
struct Commands {
    /// Uuid → how far it got. Ended commands are dropped.
    open: std::collections::HashMap<String, CommandState>,
    /// The CLI reports `command_lifecycle`. Older CLIs never do: their only
    /// confirmation is the replay, and the settle fallback covers the rest.
    reported: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CommandState {
    /// On stdin; the CLI has not acknowledged it yet.
    Written,
    /// The CLI read it and will start or cancel it.
    Queued,
    /// A turn took it up.
    Started,
}

impl Commands {
    /// Without lifecycle reports, how long a held result waits in silence
    /// for the replay that supersedes it.
    const UNREPORTED_SETTLE: Duration = Duration::from_secs(5);
    /// A lifecycle-reporting CLI acknowledges a message as soon as it reads
    /// it. A hold resting only on unacknowledged messages is a safety net
    /// against a message the CLI never reports, not a timing guess.
    const UNACKNOWLEDGED_SETTLE: Duration = Duration::from_secs(30);

    fn written(&mut self, id: String) {
        self.open.insert(id, CommandState::Written);
    }

    fn is_terminal(state: &str) -> bool {
        matches!(state, "completed" | "cancelled" | "canceled" | "failed")
    }

    /// Apply one `command_lifecycle` frame. True when a turn took the
    /// command up (`started`).
    fn lifecycle(&mut self, id: &str, state: &str) -> bool {
        self.reported = true;
        if Self::is_terminal(state) {
            self.open.remove(id);
            return false;
        }
        let next = match state {
            "started" => CommandState::Started,
            "queued" => CommandState::Queued,
            _ => return false,
        };
        if let Some(current) = self.open.get_mut(id)
            && *current != CommandState::Started
        {
            *current = next;
        }
        next == CommandState::Started
    }

    /// A message is written or queued but no turn has taken it up.
    fn waiting(&self) -> bool {
        self.open.values().any(|s| *s != CommandState::Started)
    }

    /// A stop's `cancel_queued` drops every message not yet taken up.
    fn cancel_waiting(&mut self) {
        self.open.retain(|_, s| *s == CommandState::Started);
    }

    /// The quiet time after which a held result is released as the turn's
    /// end; `None` = only the CLI's own lifecycle settles it.
    fn held_settle(&self) -> Option<Duration> {
        if !self.reported {
            Some(Self::UNREPORTED_SETTLE)
        } else if self.open.values().any(|s| *s == CommandState::Queued) {
            // A queued message always starts or is cancelled.
            None
        } else {
            Some(Self::UNACKNOWLEDGED_SETTLE)
        }
    }
}

type RequestInputFn = Box<
    dyn Fn(Vec<UserInputQuestion>) -> tokio::sync::oneshot::Receiver<Vec<UserInputAnswer>>
        + Send
        + Sync,
>;

/// Serve one `can_use_tool` control request (the CLI blocks until SOME
/// response arrives, so every request must be answered). What `config`'s
/// access mode covers is allowed at once; anything else asks the person
/// first (see [`approval_question`]), in a subtask so the frame loop keeps
/// flowing, and only a yes allows it. `AskUserQuestion` is intercepted —
/// surface the questions through the engine's input bridge (which owns the
/// `InputRequested`/`InputResolved` lifecycle), wait for the user's answers,
/// and hand them back keyed by question text, as the tool expects.
///
/// Any other request is answered with an error: the CLI waits for a reply to
/// every request it sends, and one left unanswered would stall the run.
fn handle_control_request(
    req: ControlRequestFrame,
    title_only: bool,
    config: &RunRequest,
    request_input: &Arc<RequestInputFn>,
    stdin_tx: &mpsc::UnboundedSender<StdinMsg>,
) {
    if req.request.subtype != "can_use_tool" {
        tracing::debug!(
            target: "zeron_harness::claude",
            "unhandled control_request subtype: {}", req.request.subtype
        );
        let line = control_error_line(
            &req.request_id,
            &format!("unsupported control request: {}", req.request.subtype),
        );
        let _ = stdin_tx.send(StdinMsg::Line(line));
        return;
    }
    if title_only {
        let line = control_response_line(
            &req.request_id,
            serde_json::json!({
                "behavior": "deny", "message": "Tools are disabled for title generation"
            }),
        );
        let _ = stdin_tx.send(StdinMsg::Line(line));
        return;
    }
    if req.request.tool_name != "AskUserQuestion" {
        let body = req.request;
        // Approving the plan leaves plan mode for the run's access mode,
        // not the CLI's `default`.
        let allow = {
            let after_plan = access_permission_mode(config);
            let exit_plan = body.tool_name == "ExitPlanMode";
            move |input: Value| {
                if exit_plan {
                    wire::exit_plan_response(input, after_plan)
                } else {
                    allow_response(input)
                }
            }
        };
        let question = approval_question(
            &body.tool_name,
            body.display_name.as_deref(),
            body.description.as_deref(),
            &body.input,
            access(config),
        );
        let Some(question) = question else {
            let line = control_response_line(&req.request_id, allow(body.input));
            let _ = stdin_tx.send(StdinMsg::Line(line));
            return;
        };
        let answers = (request_input)(vec![question.clone()]);
        let stdin_tx = stdin_tx.clone();
        tokio::spawn(async move {
            // A dropped sender or no answer is a refusal, like a no.
            let answers = answers.await.unwrap_or_default();
            let response = if approval::approved(&question, &answers) {
                allow(body.input)
            } else {
                deny_response(APPROVAL_DENIED)
            };
            let line = control_response_line(&req.request_id, response);
            let _ = stdin_tx.send(StdinMsg::Line(line));
        });
        return;
    }
    let request_input = Arc::clone(request_input);
    let stdin_tx = stdin_tx.clone();
    tokio::spawn(async move {
        let request_id = req.request_id;
        let input = req.request.input;
        let questions = parse_questions(&input);
        // The engine's input bridge is the SOLE emitter of
        // `InputRequested`/`InputResolved`: it mints the request id, parks the
        // resolver for `respond_input`, and surfaces both events. Emitting our
        // own copy here (keyed by Claude's control-request id) folded a SECOND
        // input part into the doc whose id no resolver knew — the QuestionPanel
        // answered that unanswerable twin and the run never resumed.
        //
        // No answers — the person closed the questions, or a dropped sender
        // (caller went away) — is a decline: deny the tool so the agent reads
        // that it went unanswered, rather than an empty pick, and is
        // unblocked rather than wedged.
        let answers = (request_input)(questions.clone()).await.unwrap_or_default();
        let response = if answers.is_empty() {
            serde_json::json!({ "behavior": "deny", "message": QUESTIONS_DISMISSED })
        } else {
            allow_response(updated_input_with_answers(&input, &questions, &answers))
        };
        let line = control_response_line(&request_id, response);
        let _ = stdin_tx.send(StdinMsg::Line(line));
    });
}

/// What a declined `AskUserQuestion` tells the agent.
const QUESTIONS_DISMISSED: &str = "The user dismissed the questions without answering.";

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
    fn probe_timeout_says_what_the_cli_printed_without_its_secrets() {
        let message = probe_timeout_message(Some(
            "Invalid API key sk-ant-secret-value\nPlease run /login".into(),
        ));
        assert!(
            message.starts_with("Claude initialize timed out after"),
            "{message}"
        );
        assert!(message.contains("Please run /login"), "{message}");
        assert!(!message.contains("secret-value"), "{message}");
        assert_eq!(
            probe_timeout_message(None),
            format!(
                "Claude initialize timed out after {}s",
                PROBE_TIMEOUT.as_secs()
            )
        );
    }

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

    #[test]
    fn runs_ask_the_cli_to_forward_subagent_text() {
        let request = RunRequest {
            mcp: None,
            prompt: "hi".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: String::new(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            access: Default::default(),
            plan: false,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
            env: Default::default(),
        };
        let mut cmd = ClaudeHarness::build_command(std::path::Path::new("claude"), &request, false);
        let value = cmd
            .as_std_mut()
            .get_envs()
            .find(|(key, _)| *key == "CLAUDE_CODE_FORWARD_SUBAGENT_TEXT")
            .and_then(|(_, value)| value.map(|v| v.to_os_string()));
        assert_eq!(value.as_deref(), Some(std::ffi::OsStr::new("1")));
    }
}

#[cfg(test)]
mod access_tests {
    use super::*;
    use serde_json::json;

    fn request(access: AccessMode, auto_approve: bool, plan: bool) -> RunRequest {
        RunRequest {
            mcp: None,
            prompt: "hi".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: "/work".into(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve,
            access,
            plan,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
            env: Default::default(),
        }
    }

    /// The permission flags a request launches with.
    fn permission_args(request: &RunRequest) -> Vec<String> {
        let args = ClaudeHarness::launch_args(request);
        let mut out = Vec::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--permission-mode" {
                out.push(arg);
                out.extend(args.next());
            } else if arg.contains("skip-permissions") {
                out.push(arg);
            }
        }
        out
    }

    #[test]
    fn modes_launch_with_their_permission_mode() {
        use AccessMode::*;
        let cases: [(AccessMode, bool, bool, &[&str]); 8] = [
            // Full access launches exactly as before modes.
            (
                FullAccess,
                true,
                false,
                &[
                    "--permission-mode",
                    "bypassPermissions",
                    "--dangerously-skip-permissions",
                ],
            ),
            (FullAccess, false, false, &["--permission-mode", "default"]),
            // `auto_approve` still means full access.
            (
                Supervised,
                true,
                false,
                &[
                    "--permission-mode",
                    "bypassPermissions",
                    "--dangerously-skip-permissions",
                ],
            ),
            (
                AutoAcceptEdits,
                false,
                false,
                &["--permission-mode", "acceptEdits"],
            ),
            (Supervised, false, false, &["--permission-mode", "default"]),
            // Plan mode wins; full access keeps the bypass for afterwards
            // without the flag that would override plan mode.
            (
                FullAccess,
                true,
                true,
                &[
                    "--permission-mode",
                    "plan",
                    "--allow-dangerously-skip-permissions",
                ],
            ),
            (AutoAcceptEdits, false, true, &["--permission-mode", "plan"]),
            (Supervised, false, true, &["--permission-mode", "plan"]),
        ];
        for (access, auto_approve, plan, expected) in cases {
            assert_eq!(
                permission_args(&request(access, auto_approve, plan)),
                expected,
                "{access:?} auto_approve={auto_approve} plan={plan}"
            );
        }
        assert_eq!(
            access_permission_mode(&request(AutoAcceptEdits, false, true)),
            "acceptEdits"
        );
    }

    #[test]
    fn mode_changes_reconfigure_the_live_process() {
        use AccessMode::*;
        let harness = ClaudeHarness::new();
        let full = request(FullAccess, false, false);
        let supervised = request(Supervised, false, false);
        // Same flags, different answers: not the same runtime as is.
        assert!(!harness.same_runtime(&full, &supervised));
        assert!(harness.reconfigures_in_place(&full, &supervised));
        let planning = request(Supervised, false, true);
        assert!(!harness.same_runtime(&supervised, &planning));
        assert!(harness.reconfigures_in_place(&planning, &supervised));
        // A stricter CLI mode relaunches: a refused switch must not leave
        // the next prompt running with less asked.
        assert!(!harness.reconfigures_in_place(&supervised, &planning));
        let auto_edits = request(AutoAcceptEdits, false, false);
        assert!(harness.reconfigures_in_place(&supervised, &auto_edits));
        assert!(!harness.reconfigures_in_place(&auto_edits, &supervised));
        let auto_planning = request(FullAccess, true, true);
        let auto = request(FullAccess, true, false);
        assert!(harness.reconfigures_in_place(&auto_planning, &auto));
        // Bypass is only reachable on a process launched for it.
        assert!(!harness.reconfigures_in_place(&full, &auto));
        // Under `auto_approve` the mode changes nothing.
        assert!(harness.same_runtime(&auto, &request(Supervised, true, false)));
        assert_eq!(harness.access_modes(), &AccessMode::ALL);
        assert!(harness.supports_plan());
    }

    #[test]
    fn tool_calls_ask_what_the_mode_does_not_cover() {
        use AccessMode::*;
        let bash = json!({"command": "cargo test", "description": "Run tests"});
        let edit = json!({"file_path": "/work/src/lib.rs", "old_string": "a"});
        let notebook = json!({"notebook_path": "/work/a.ipynb"});
        let ask = |tool: &str, input: &Value, access: AccessMode| {
            approval_question(tool, None, None, input, access).map(|q| q.question)
        };
        for tool in ["Bash", "Edit", "WebFetch", "ExitPlanMode", "mcp__x__y"] {
            assert_eq!(ask(tool, &bash, FullAccess), None, "{tool}");
        }
        for tool in EDIT_TOOLS {
            assert_eq!(ask(tool, &edit, AutoAcceptEdits), None, "{tool}");
        }
        assert_eq!(
            ask("Bash", &bash, AutoAcceptEdits).as_deref(),
            Some("Claude wants to run `cargo test`. Allow it?")
        );
        assert_eq!(
            ask("Write", &edit, Supervised).as_deref(),
            Some("Claude wants to modify /work/src/lib.rs. Allow it?")
        );
        assert_eq!(
            ask("NotebookEdit", &notebook, Supervised).as_deref(),
            Some("Claude wants to modify /work/a.ipynb. Allow it?")
        );
        let mcp = approval_question(
            "mcp__linear__search",
            Some("search (linear)"),
            Some("Find the bug"),
            &json!({}),
            Supervised,
        )
        .unwrap();
        assert_eq!(mcp.header, "Approve tool");
        assert_eq!(
            mcp.question,
            "Claude wants to use search (linear) (Find the bug). Allow it?"
        );
        assert_eq!(
            ask("ExitPlanMode", &json!({"plan": "p"}), AutoAcceptEdits).as_deref(),
            Some("Claude wants to start implementing its plan. Allow it?")
        );
        let plan = approval_question("ExitPlanMode", None, None, &json!({}), Supervised).unwrap();
        assert_eq!(plan.options, vec![approval::ALLOW, approval::DENY]);
    }
}

#[cfg(test)]
mod resolution_tests {
    use super::*;

    #[test]
    fn an_npm_shim_resolves_to_the_native_cli_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("claude.cmd");
        std::fs::write(&shim, b"@echo off").unwrap();
        // An older package without the native binary: keep the shim.
        assert_eq!(native_beside_npm_shim(&shim), None);
        let bin = dir
            .path()
            .join("node_modules/@anthropic-ai/claude-code/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("claude.exe"), b"MZ").unwrap();
        assert_eq!(native_beside_npm_shim(&shim), Some(bin.join("claude.exe")));
        // Only shims are redirected.
        assert_eq!(native_beside_npm_shim(&dir.path().join("claude.exe")), None);
        assert_eq!(native_beside_npm_shim(&dir.path().join("claude")), None);
    }

    #[test]
    fn a_runs_own_config_dir_wins() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("HOME".to_owned(), "/home/cloud".to_owned());
        assert_eq!(config_root(&env), PathBuf::from("/home/cloud/.claude"));
        env.insert("CLAUDE_CONFIG_DIR".to_owned(), "/cfg".to_owned());
        assert_eq!(config_root(&env), PathBuf::from("/cfg"));
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
