//! Codex harness: spawns the installed `codex` CLI as `codex app-server` and
//! speaks JSON-RPC 2.0 over stdio — the same interface the Codex IDE extension
//! uses. Resurrected from the pre-ACP driver and modernized.
//!
//! VERSION PIN: the app-server API is EXPERIMENTAL (`capabilities.
//! experimentalApi`); this driver is validated against codex-cli 0.153.4 —
//! imageGeneration additionally follows the 0.154.0 schema (savedPath only).
//! Revalidate the method/notification surface when bumping past it.
//!
//! - `initialize` handshake (clientInfo + `capabilities.experimentalApi`) then
//!   the `initialized` notification; unknown notification methods tolerated.
//! - `thread/start` (or `thread/resume` with a fresh-start fallback) →
//!   `SessionStarted`; `turn/start` carries the prompt, model, effort,
//!   `sandboxPolicy`, and approval policy.
//! - Notifications map to [`AgentEvent`]s: agentMessage/reasoning deltas (both
//!   `delta`/`textDelta` spellings), item lifecycles → typed ToolCall/ToolResult,
//!   `thread/tokenUsage/updated` → Usage, turn/completed|failed|aborted → Done.
//! - Approvals + sandbox: yolo mode. The wire policy is always `"never"` and
//!   the sandbox is forced to `danger-full-access` — parity with the Claude
//!   adapter's auto-approve-everything (unattended runs). Stray
//!   `item/commandExecution/requestApproval` +
//!   `item/fileChange/requestApproval` still round-trip through
//!   [`RunControls::request_input`] as a synthesized yes/no question.
//! - Subagents are full child app-server threads. Parent spawn items establish
//!   their stable ownership; content arriving before the spawn is buffered.
//!   A registered child's notifications route through an EXPLICIT table
//!   ([`normalize::route_child_notification`]) — content, errors and child turns
//!   become tagged [`AgentEvent::Subagent`] events; unrelated child bookkeeping is
//!   consumed so it can never settle the parent turn, and unknown methods
//!   fall through to the parent path (fail open, never silent loss).
//! - Steering: `turn/steer { expectedTurnId }` into the live turn; a rejected
//!   steer (the turn-completed race) is queued and delivered as the next
//!   `turn/start` on the same thread. The session is persistent across turns
//!   while the steering mailbox lives.
//! - Interrupt: cancelling [`RunControls::interrupt`] sends `turn/interrupt`,
//!   escalating to SIGTERM → SIGKILL if the child is unresponsive; the stream
//!   always ends with `Done { status: Interrupted }`.

pub(crate) mod catalog;
mod normalize;
mod subagents;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ModelOption, ModelOptionChoice, ReasoningLevel,
    RunRequest, SlashCommand, SteeringMode, UserInputAnswer, UserInputQuestion,
};

use crate::jsonrpc::{Incoming, RpcClient};
use crate::process::{Child, Command, Stdio};
use crate::{Harness, HarnessError, RunControls, SteerMessage};
use catalog::{REASONING_LEVELS, sandbox_mode, sandbox_policy_value, static_models, to_effort};
use normalize::{
    ChildRoute, Phase, ReasoningStream, delta_text, item_id, item_type, notification_thread_id,
    route_child_notification, turn_error_message, turn_id, usage_event,
};

/// Locate the device's installed Codex CLI: `CODEX_EXECUTABLE`, then our own
/// PATH, then the login-shell PATH snapshot (the user's shell init shapes
/// PATH in ways a GUI/service launch never sees — see [`crate::shell_env`]),
/// then known install locations as a last resort. Resolved per call — cheap
/// after the snapshot is cached.
pub fn resolve_codex_executable() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CODEX_EXECUTABLE").filter(|p| !p.is_empty()) {
        return crate::executable::validate_native_override(&PathBuf::from(p)).ok();
    }
    let mut extra = Vec::new();
    if let Some(home) = crate::executable::home_dir() {
        extra.push(home.join(".local").join("bin").join("codex"));
        extra.push(home.join(".codex").join("bin").join("codex"));
        extra.push(home.join(".npm-global").join("bin").join("codex"));
    }
    extra.push(PathBuf::from("/opt/homebrew/bin/codex"));
    extra.push(PathBuf::from("/usr/local/bin/codex"));
    crate::executable::find_on_paths("codex", extra)
}

/// A ready-to-spawn `codex login` command for the engine's account flow.
///
/// Shares the harness's full resolution (`CODEX_EXECUTABLE`, PATH, login-shell
/// snapshot, install locations — including the Windows npm payload layout) and
/// its child-PATH composition, so "Add account" launches exactly the binary
/// the harness itself would run. `CODEX_HOME` isolates the login from the live
/// `~/.codex` session; the caller owns stdio wiring and cancellation.
pub fn login_command(codex_home: &std::path::Path) -> Result<Command, HarnessError> {
    let exe = CodexHarness::new().resolve_executable()?;
    let mut cmd = Command::new(&exe);
    crate::compose_child_path(&mut cmd, &exe);
    cmd.arg("login").env("CODEX_HOME", codex_home);
    Ok(cmd)
}

/// The Codex harness. Construct with [`CodexHarness::new`]; tests point it at a
/// fake app server with [`CodexHarness::with_executable`].
pub struct CodexHarness {
    executable: Option<PathBuf>,
    /// Grace between `turn/interrupt` and SIGTERM.
    interrupt_grace: Duration,
    /// Grace between SIGTERM and SIGKILL.
    kill_grace: Duration,
}

impl Default for CodexHarness {
    fn default() -> Self {
        Self {
            executable: None,
            interrupt_grace: Duration::from_secs(2),
            kill_grace: Duration::from_secs(3),
        }
    }
}

impl CodexHarness {
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
        if let Some(p) = std::env::var_os("CODEX_EXECUTABLE")
            && !p.is_empty()
        {
            return crate::executable::validate_native_override(&PathBuf::from(p));
        }
        resolve_codex_executable().ok_or_else(|| {
            HarnessError::NotInstalled(
                "codex (searched PATH, the login shell's PATH, ~/.local/bin, \
                 ~/.codex/bin, ~/.npm-global/bin, /opt/homebrew/bin, /usr/local/bin, \
                 and fnm/nvm/volta/pnpm/bun install dirs; Windows also checks USERPROFILE \
                 and explicit NVM_SYMLINK/VOLTA_HOME/PNPM_HOME; set CODEX_EXECUTABLE \
                 to override)"
                    .into(),
            )
        })
    }

    /// Short-lived discovery probe: a `codex app-server` handshake followed by
    /// `skills/list` — the only invocable-listing method the 0.146.x wire has
    /// (custom `~/.codex/prompts` are NOT exposed; the TUI-only built-ins
    /// aren't either). Preserve each skill's path and project context;
    /// skills are separate from the slash-command catalog.
    async fn discover_skills(&self, cwd: Option<&std::path::Path>) -> Result<Value, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut cmd = Command::new(&exe);
        cmd.arg("app-server");
        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        }
        crate::compose_child_path(&mut cmd, &exe);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(exe.display().to_string())
            } else {
                HarnessError::Io(e)
            }
        })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            shutdown_child(&mut child, self.kill_grace).await;
            return Err(HarnessError::Protocol("codex child has no stdio".into()));
        };
        // The receiver must stay alive for the client's reader loop; agent →
        // client traffic during the probe is ignored.
        let (client, _incoming) = RpcClient::new(stdin, stdout);
        let discovery = async {
            client
                .request(
                    "initialize",
                    json!({
                        "clientInfo": {
                            "name": "zeron-native",
                            "title": "Zeron",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "capabilities": { "experimentalApi": true },
                    }),
                )
                .await?;
            client.notify("initialized", None);
            let params = cwd
                .map(|cwd| json!({ "cwds": [cwd], "forceReload": true }))
                .unwrap_or_else(|| json!({}));
            let skills = client.request("skills/list", params).await?;
            Ok::<Value, HarnessError>(skills)
        };
        let result = tokio::time::timeout(Duration::from_secs(10), discovery).await;
        shutdown_child(&mut child, self.kill_grace).await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(HarnessError::Protocol("command discovery timed out".into())),
        }
    }

    /// Short-lived live catalog probe. `model/list` is paginated and already
    /// applies the signed-in account's rollout/visibility policy, so hidden or
    /// unavailable models (including staged Astra rollouts) never leak into a
    /// successful picker response.
    async fn discover_models(&self) -> Result<Vec<Model>, HarnessError> {
        let exe = self.resolve_executable()?;
        let mut cmd = Command::new(&exe);
        cmd.arg("app-server");
        crate::compose_child_path(&mut cmd, &exe);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(exe.display().to_string())
            } else {
                HarnessError::Io(e)
            }
        })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            shutdown_child(&mut child, self.kill_grace).await;
            return Err(HarnessError::Protocol("codex child has no stdio".into()));
        };
        let (client, _incoming) = RpcClient::new(stdin, stdout);
        let discovery = async {
            client
                .request(
                    "initialize",
                    json!({
                        "clientInfo": {
                            "name": "zeron-native",
                            "title": "Zeron",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "capabilities": { "experimentalApi": true },
                    }),
                )
                .await?;
            client.notify("initialized", None);

            let mut models = Vec::new();
            let mut model_ids = HashSet::new();
            let mut seen_cursors = HashSet::new();
            let mut cursor: Option<String> = None;
            let mut default_model_id: Option<String> = None;
            loop {
                let mut params = json!({ "limit": 20, "includeHidden": false });
                if let Some(cursor) = cursor.as_deref() {
                    params["cursor"] = Value::String(cursor.to_owned());
                }
                let page = client.request("model/list", params).await?;
                let (page_models, next_cursor) = parse_model_list_page(&page);
                for (model, is_default) in page_models {
                    if model_ids.insert(model.id.clone()) {
                        if is_default && default_model_id.is_none() {
                            default_model_id = Some(model.id.clone());
                        }
                        models.push(model);
                    }
                }
                let Some(next) = next_cursor.filter(|next| !next.is_empty()) else {
                    break;
                };
                if !seen_cursors.insert(next.clone()) {
                    break;
                }
                cursor = Some(next);
            }

            // Capability discovery is authoritative, including feature enablement.
            // A static model fallback must not promise experimental protocol support.
            let modes = client
                .request("collaborationMode/list", json!({}))
                .await
                .ok();
            let mut goals_enabled = false;
            let mut cursor: Option<String> = None;
            let mut seen = HashSet::new();
            loop {
                let mut params = json!({"limit": 100});
                if let Some(value) = &cursor {
                    params["cursor"] = Value::String(value.clone());
                }
                let Ok(features) = client.request("experimentalFeature/list", params).await else {
                    break;
                };
                goals_enabled |= features["data"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|f| f["name"] == "goals" && f["enabled"] == true)
                });
                let Some(next) = features["nextCursor"].as_str().filter(|s| !s.is_empty()) else {
                    break;
                };
                if !seen.insert(next.to_owned()) {
                    break;
                }
                cursor = Some(next.to_owned());
            }
            let mode = advertised_mode_option(modes.as_ref(), goals_enabled);
            for model in &mut models {
                model.options.extend(mode.clone());
            }

            if let Some(default_id) = default_model_id
                && let Some(index) = models.iter().position(|model| model.id == default_id)
                && index != 0
            {
                let default_model = models.remove(index);
                models.insert(0, default_model);
            }
            Ok::<Vec<Model>, HarnessError>(models)
        };
        let result = tokio::time::timeout(Duration::from_secs(10), discovery).await;
        shutdown_child(&mut child, self.kill_grace).await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(HarnessError::Protocol("model discovery timed out".into())),
        }
    }
}

fn advertised_mode_option(
    modes: Option<&Value>,
    goals_enabled: bool,
) -> Option<zeron_proto::ModelOption> {
    let has = |mode: &str| {
        modes
            .and_then(|m| m["data"].as_array())
            .is_some_and(|items| items.iter().any(|item| item["mode"] == mode))
    };
    if !has("default") {
        return None;
    }
    let mut option = zeron_proto::agent_mode_option(HarnessId::Codex)?;
    option.choices.retain(|choice| match choice.id.as_str() {
        "plan" => has("plan"),
        "goal" => goals_enabled,
        _ => true,
    });
    (option.choices.len() > 1).then_some(option)
}

fn reasoning_level(value: &str) -> Option<ReasoningLevel> {
    Some(match value {
        "minimal" => ReasoningLevel::Minimal,
        "low" => ReasoningLevel::Low,
        "medium" => ReasoningLevel::Medium,
        "high" => ReasoningLevel::High,
        "xhigh" => ReasoningLevel::XHigh,
        "max" => ReasoningLevel::Max,
        "ultra" => ReasoningLevel::Ultra,
        "ultracode" => ReasoningLevel::Ultracode,
        "ultrathink" => ReasoningLevel::Ultrathink,
        _ => return None,
    })
}

/// Codex accepts both names, but Zeron has historically persisted `fast`.
/// Normalize the app server's `priority` id so live and fallback catalogs do
/// not produce two different settings for the same tier.
fn normalized_service_tier(value: &str) -> &str {
    match value {
        "priority" => "fast",
        other => other,
    }
}

fn service_tier_label(value: &str) -> String {
    match value {
        "fast" | "priority" => "Fast".into(),
        "flex" => "Flex".into(),
        "ultrafast" => "Ultra Fast".into(),
        other => other.to_owned(),
    }
}

fn model_service_tier(item: &Value) -> Option<ModelOption> {
    let mut choices = vec![ModelOptionChoice {
        id: "default".into(),
        label: "Standard".into(),
    }];
    let mut seen = HashSet::from(["default".to_owned()]);
    for tier in item
        .get("serviceTiers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(wire_id) = tier.get("id").and_then(Value::as_str) else {
            continue;
        };
        let id = normalized_service_tier(wire_id).to_owned();
        if !seen.insert(id.clone()) {
            continue;
        }
        let label = tier
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| service_tier_label(wire_id));
        choices.push(ModelOptionChoice { id, label });
    }
    for tier in item
        .get("additionalSpeedTiers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(wire_id) = tier.as_str() else {
            continue;
        };
        let id = normalized_service_tier(wire_id).to_owned();
        if seen.insert(id.clone()) {
            choices.push(ModelOptionChoice {
                id,
                label: service_tier_label(wire_id),
            });
        }
    }
    if choices.len() == 1 {
        return None;
    }
    let default_choice = item
        .get("defaultServiceTier")
        .and_then(Value::as_str)
        .map(normalized_service_tier)
        .filter(|id| seen.contains(*id))
        .unwrap_or("default")
        .to_owned();
    Some(ModelOption {
        id: "serviceTier".into(),
        label: "Service Tier".into(),
        choices,
        default_choice,
    })
}

/// Parse one `model/list` page. Unknown future reasoning levels are ignored
/// independently instead of invalidating the complete catalog.
fn parse_model_list_page(result: &Value) -> (Vec<(Model, bool)>, Option<String>) {
    let mut models = Vec::new();
    for item in result
        .get("data")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if item.get("hidden").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(id) = item
            .get("model")
            .and_then(Value::as_str)
            .or_else(|| item.get("id").and_then(Value::as_str))
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let label = item
            .get("displayName")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .unwrap_or(id)
            .to_owned();
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|description| !description.is_empty())
            .map(str::to_owned);
        let reasoning_levels = item
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|effort| {
                effort
                    .get("reasoningEffort")
                    .and_then(Value::as_str)
                    .or_else(|| effort.as_str())
                    .and_then(reasoning_level)
            })
            .collect();
        let options = model_service_tier(item).into_iter().collect();
        models.push((
            Model {
                id: id.to_owned(),
                label,
                description,
                reasoning_levels,
                options,
            },
            item.get("isDefault").and_then(Value::as_bool) == Some(true),
        ));
    }
    let next_cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .map(str::to_owned);
    (models, next_cursor)
}

/// `skills/list` result → typed skills. Keep distinct paths for duplicate names.
/// Identical name/path pairs are deduplicated across cwd groups. The interface's
/// shortDescription is picker-sized; the model-facing description is a fallback.
fn parse_skills(result: &Value) -> Vec<zeron_proto::invocation::Skill> {
    let mut seen = HashSet::new();
    result
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|group| {
            group
                .get("skills")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|skill| {
            let name = skill.get("name")?.as_str()?;
            let path = skill.get("path")?.as_str()?;
            if !zeron_proto::invocation::valid_invocation_name(name)
                || !zeron_proto::invocation::valid_skill_path(path)
                || !seen.insert((name.to_owned(), path.to_owned()))
            {
                return None;
            }
            Some(zeron_proto::invocation::Skill {
                command: None,
                name: name.to_owned(),
                path: path.to_owned(),
                description: skill
                    .pointer("/interface/shortDescription")
                    .and_then(Value::as_str)
                    .or_else(|| skill.get("description").and_then(Value::as_str))
                    .unwrap_or_default()
                    .to_owned(),
                enabled: skill
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            })
        })
        .collect()
}

#[async_trait]
impl Harness for CodexHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Codex
    }
    fn display_name(&self) -> &str {
        // "Codex" (not "Codex CLI") — comet composer/defaults.ts
        // HARNESS_LABEL; must also match the registry's lazy descriptor so
        // the catalog entry doesn't change after the first resolve.
        "Codex"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    /// Native `turn/steer` injects into the active turn; a steer that misses
    /// the turn falls back to a follow-up `turn/start` on the same thread.
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        REASONING_LEVELS
    }
    fn installed(&self) -> bool {
        self.resolve_executable().is_ok()
    }
    /// Done is the CLI's own terminal frame, for wake turns too.
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn validate_request(&self, request: &RunRequest) -> Result<(), HarnessError> {
        let prompt = zeron_proto::invocation::harness_prompt(&request.prompt, self.id());
        validate_request_shape(request, &prompt)
    }

    /// The signed-in account's visible `model/list` is authoritative. A
    /// curated snapshot keeps the picker operational when the experimental
    /// discovery call is unavailable or temporarily fails; failed probes are
    /// intentionally not cached so reopening the picker retries rollout state.
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.resolve_executable()?;
        match self.discover_models().await {
            Ok(models) if !models.is_empty() => Ok(models),
            Ok(_) => Ok(static_models()),
            Err(error) => {
                tracing::debug!(
                    target: "zeron_harness::codex",
                    "model/list discovery failed; using fallback catalog: {error}"
                );
                Ok(static_models())
            }
        }
    }

    async fn skills(
        &self,
        cwd: &std::path::Path,
    ) -> Result<Option<Vec<zeron_proto::invocation::Skill>>, HarnessError> {
        self.discover_skills(Some(cwd))
            .await
            .map(|value| Some(parse_skills(&value)))
    }

    async fn commands(&self) -> Result<Vec<SlashCommand>, HarnessError> {
        Ok(vec![
            SlashCommand {
                name: "compact".into(),
                description: "Compact this conversation's context".into(),
                input_hint: None,
            },
            SlashCommand {
                name: "review".into(),
                description: "Review uncommitted changes, or supply review instructions".into(),
                input_hint: Some("optional instructions".into()),
            },
        ])
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
        request.model_options.clear();
        request.auto_approve = false;
        self.run_with_mode(request, controls, true).await
    }
}

impl CodexHarness {
    async fn run_with_mode(
        &self,
        mut request: RunRequest,
        controls: RunControls,
        title_only: bool,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        validate_request_shape(&request, &request.prompt)?;
        let exe = self.resolve_executable()?;
        // Yolo mode: danger-full-access + approvalPolicy "never" (set below) —
        // codex's --dangerously-bypass-approvals-and-sandbox equivalent.
        // Parity with the Claude adapter, which auto-approves every
        // can_use_tool and so effectively grants full access. This also
        // sidesteps codex ≤0.144.x's workspace-write bug where a linked
        // worktree on a slash-named branch derives a malformed mount that
        // kills every command.
        request.sandbox = if title_only {
            zeron_proto::SandboxLevel::ReadOnly
        } else {
            zeron_proto::SandboxLevel::DangerFullAccess
        };
        let mut cmd = Command::new(&exe);
        cmd.arg("app-server");
        crate::compose_child_path(&mut cmd, &exe);
        if !request.cwd.is_empty() {
            cmd.current_dir(&request.cwd);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                HarnessError::NotInstalled(exe.display().to_string())
            } else {
                HarnessError::Io(e)
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| HarnessError::Protocol("codex child has no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| HarnessError::Protocol("codex child has no stdout".into()))?;
        let stderr_tail = crate::StderrTail::default();
        if let Some(stderr) = child.stderr.take() {
            let tail = stderr_tail.clone();
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "zeron_harness::codex", "stderr: {line}");
                    tail.push(&line);
                }
            });
        }

        let (client, incoming) = RpcClient::new(stdin, stdout);
        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        tokio::spawn(run_session(Session {
            title_only,
            child,
            client,
            incoming,
            event_tx,
            controls,
            request,
            interrupt_grace: self.interrupt_grace,
            kill_grace: self.kill_grace,
            stderr_tail,
        }));

        Ok(futures::stream::unfold(event_rx, |mut rx| async move {
            rx.recv().await.map(|ev| (ev, rx))
        })
        .boxed())
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

struct Session {
    title_only: bool,
    child: Child,
    client: RpcClient,
    incoming: mpsc::Receiver<Incoming>,
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
    controls: RunControls,
    request: RunRequest,
    interrupt_grace: Duration,
    kill_grace: Duration,
    /// Rolling stderr tail for the crash message on an unexpected exit.
    stderr_tail: crate::StderrTail,
}

/// Turn-routing state (port of codex.ts's activeTurnId/completedTurnIds): the
/// `turn/start` response and the turn lifecycle notifications are separate
/// app-server messages that may arrive in either order — never revive a turn
/// that `turn/completed` already declared finished.
#[derive(Default)]
struct TurnRouter {
    active: Option<String>,
    completed: VecDeque<String>,
}

impl TurnRouter {
    fn is_completed(&self, id: &str) -> bool {
        self.completed.iter().any(|c| c == id)
    }

    fn note_started(&mut self, id: String) {
        if id.is_empty() || self.is_completed(&id) {
            return;
        }
        // A replacement `turn/started` is authoritative evidence that a stale
        // active turn is over, even if its completion notification was lost.
        if let Some(prev) = self.active.take()
            && prev != id
        {
            self.remember_completed(prev);
        }
        self.active = Some(id);
    }

    fn note_completed(&mut self, id: &str) {
        if id.is_empty() {
            return;
        }
        self.remember_completed(id.to_owned());
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
    }

    /// Adopt a turn id from a `turn/start` RESPONSE (the notification is
    /// allowed to beat it).
    fn adopt_started(&mut self, id: String) {
        self.active = (!id.is_empty() && !self.is_completed(&id)).then_some(id);
    }

    fn remember_completed(&mut self, id: String) {
        self.completed.push_back(id);
        // Bounded so a months-long persistent session can't grow it forever.
        while self.completed.len() > 32 {
            self.completed.pop_front();
        }
    }
}

fn new_message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Rotate the assistant message id; returns (previous, next).
fn rotate(id: &mut String) -> (String, String) {
    let prev = std::mem::replace(id, new_message_id());
    (prev, id.clone())
}

async fn send(tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>, ev: AgentEvent) -> bool {
    tx.send(Ok(ev)).await.is_ok()
}

/// Preserve the selected path in the app-server's native skill input. Text
/// stays first for command routing; repeated selections do not load a skill twice.
fn prompt_input(text: &str) -> Value {
    use zeron_proto::invocation::{Invocation, invocation_links, invocation_prompt};
    let mut input = vec![json!({"type": "text", "text": invocation_prompt(text)})];
    let mut seen = std::collections::HashSet::new();
    for (_, invocation) in invocation_links(text) {
        if let Invocation::Skill { name, path, .. } = invocation {
            if !zeron_proto::invocation::native_skill_identity(&path)
                && seen.insert((name.clone(), path.clone()))
            {
                input.push(json!({"type": "skill", "name": name, "path": path}));
            }
        }
    }
    Value::Array(input)
}

fn validate_agent_mode(request: &RunRequest) -> Result<(), HarnessError> {
    let Some(value) = request.model_options.get(zeron_proto::AGENT_MODE_OPTION) else {
        return Ok(());
    };
    let Some(mode) = value.as_str() else {
        return Err(HarnessError::Protocol("Codex mode must be a string".into()));
    };
    if matches!(mode, "default" | "plan" | "goal") {
        Ok(())
    } else {
        Err(HarnessError::Protocol(format!(
            "Unsupported Codex mode: {mode}"
        )))
    }
}

fn validate_request_shape(request: &RunRequest, prompt: &str) -> Result<(), HarnessError> {
    validate_agent_mode(request)?;
    let native = command_request(prompt, "")?;
    if native
        .as_ref()
        .is_some_and(|(method, _)| *method == "thread/compact/start")
        && request.resume.is_none()
    {
        return Err(HarnessError::Protocol(
            "/compact needs an existing Codex conversation".into(),
        ));
    }
    if native.is_some() && !request.attachments.is_empty() {
        return Err(HarnessError::Protocol(
            "Codex commands cannot include attachments; send them in a separate prompt".into(),
        ));
    }
    Ok(())
}

/// Map supported leading commands to native app-server operations.
fn command_request(
    text: &str,
    thread_id: &str,
) -> Result<Option<(&'static str, Value)>, HarnessError> {
    let decoded = zeron_proto::invocation::invocation_prompt(text);
    let Some((name, args)) = zeron_proto::invocation::leading_command(&decoded) else {
        return Ok(None);
    };
    if matches!(name, "compact" | "review")
        && zeron_proto::invocation::invocation_links(text)
            .iter()
            .any(|(_, invocation)| {
                matches!(
                    invocation,
                    zeron_proto::invocation::Invocation::Skill { .. }
                )
            })
    {
        return Err(HarnessError::Protocol(
            "Codex commands cannot include skill selections; send them in a separate prompt".into(),
        ));
    }
    match name {
        "compact" if args.is_empty() => Ok(Some((
            "thread/compact/start",
            json!({"threadId": thread_id}),
        ))),
        "compact" => Err(HarnessError::Protocol(
            "/compact takes no arguments; send other text separately".into(),
        )),
        "review" => Ok(Some((
            "review/start",
            json!({
                "threadId": thread_id, "delivery": "inline",
                "target": if args.is_empty() { json!({"type":"uncommittedChanges"}) }
                    else { json!({"type":"custom", "instructions":args}) },
            }),
        ))),
        // Known client commands need explicit UI mappings. Do not silently
        // send those to the model; unknown slash tokens and paths stay literal.
        "model" | "permissions" | "approvals" | "new" | "clear" | "resume" | "fork" | "status"
        | "diff" | "mention" | "mcp" | "skills" | "plan" | "fast" | "logout" | "quit" | "exit"
        | "init" | "rename" | "feedback" | "ps" | "stop" | "clean" | "archive" | "delete" => {
            Err(HarnessError::Protocol(format!(
                "/{name} is not mapped in Zeron's Codex integration. Available commands: /compact and /review."
            )))
        }
        _ => Ok(None),
    }
}

async fn start_turn(client: &RpcClient, mut params: Value) -> Result<String, HarnessError> {
    let text = params
        .pointer("/input/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let thread_id = params["threadId"].as_str().unwrap_or_default();
    let native = command_request(text, thread_id)?;
    let goal_mode = params
        .as_object_mut()
        .and_then(|p| p.remove("zeronGoalMode"))
        .and_then(|value| value.as_bool())
        .filter(|_| native.is_none());
    if goal_mode == Some(false) {
        // Older servers lack goal APIs; ordinary build/plan turns still work.
        let thread_id = params["threadId"].clone();
        if let Ok(current) = client
            .request("thread/goal/get", json!({"threadId": thread_id}))
            .await
        {
            if current["goal"]["status"].as_str() == Some("active") {
                client
                    .request(
                        "thread/goal/set",
                        json!({"threadId": thread_id, "status": "paused"}),
                    )
                    .await?;
            }
        }
    }
    if goal_mode == Some(true) {
        let thread_id = params["threadId"].clone();
        let current = client
            .request("thread/goal/get", json!({"threadId": thread_id}))
            .await?;
        let status = current["goal"]["status"].as_str();
        if status.is_none() || status == Some("complete") {
            let objective = params
                .pointer("/input/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            client
                .request(
                    "thread/goal/set",
                    json!({"threadId": thread_id, "objective": objective, "status": "active"}),
                )
                .await?;
        } else if status != Some("active") {
            client
                .request(
                    "thread/goal/set",
                    json!({"threadId": thread_id, "status": "active"}),
                )
                .await?;
        }
    }
    if native.is_some()
        && params["input"]
            .as_array()
            .is_some_and(|input| input.len() > 1)
    {
        return Err(HarnessError::Protocol(
            "Codex commands cannot include skill selections; send them in a separate prompt".into(),
        ));
    }
    let (method, params) = native.unwrap_or(("turn/start", params));
    let started = client.request(method, params).await?;
    Ok(started["turn"]["id"].as_str().unwrap_or("").to_owned())
}

/// The per-run event loop: one task multiplexing app-server messages, the
/// steering mailbox, the interrupt token, and consumer liveness.
async fn run_session(session: Session) {
    let Session {
        title_only,
        mut child,
        client,
        mut incoming,
        event_tx,
        controls,
        request,
        interrupt_grace,
        kill_grace,
        stderr_tail,
    } = session;
    let RunControls {
        request_input,
        mut steering,
        interrupt,
    } = controls;
    let request_input = Arc::new(request_input);

    // ---- wire params ------------------------------------------------------
    // Parity with the Claude adapter, which auto-approves every `can_use_tool`
    // regardless of `auto_approve` (zeron sessions run unattended; combined
    // with the danger-full-access override above this is codex's yolo mode):
    // never surface wire approvals. "on-request" turned
    // every command into a yes/no question (user report: "asking me for
    // approval at every step"). The approval-as-input plumbing below stays for
    // stray requests and a future explicit permission-mode setting.
    let approval_policy = "never";
    let effort = to_effort(request.reasoning);
    // Service tier rides thread-start and every turn (mirrors the Codex IDE
    // client). "default" means Standard — omit it entirely.
    let service_tier = request
        .model_options
        .get("serviceTier")
        .and_then(Value::as_str)
        .filter(|t| *t != "default")
        .map(str::to_owned);

    let start_params = {
        let mut p = serde_json::Map::new();
        if title_only {
            p.insert("baseInstructions".into(), crate::TITLE_INSTRUCTIONS.into());
            p.insert(
                "developerInstructions".into(),
                crate::TITLE_INSTRUCTIONS.into(),
            );
            p.insert("ephemeral".into(), true.into());
            p.insert(
                "config".into(),
                json!({
                    "project_doc_max_bytes": 0,
                    "web_search": "disabled",
                    "features.shell_tool": false,
                    "features.apply_patch_freeform": false,
                    "features.multi_agent": false,
                    "features.apps": false,
                    "features.multi_agent_v2": false,
                    "agents.enabled": false,
                    "features.browser_use": false,
                    "features.computer_use": false,
                    "features.js_repl": false,
                    "features.image_generation": false,
                    "features.memories": false
                }),
            );
        }
        p.insert("cwd".into(), Value::String(request.cwd.clone()));
        p.insert("approvalPolicy".into(), approval_policy.into());
        p.insert("sandbox".into(), sandbox_mode(request.sandbox).into());
        if let Some(model) = &request.model {
            p.insert("model".into(), Value::String(model.clone()));
        }
        if let Some(tier) = &service_tier {
            p.insert("serviceTier".into(), Value::String(tier.clone()));
        }
        p
    };

    // ---- handshake + thread + first turn (interruptible) ------------------
    let setup = async {
        client
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "zeron-native",
                        "title": "Zeron",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": { "experimentalApi": true },
                }),
            )
            .await?;
        client.notify("initialized", None);

        let mut start_params = start_params.clone();
        if title_only {
            // Disable each configured MCP server explicitly: an empty table
            // would merge with user configuration and leave servers enabled.
            let config = client
                .request("config/read", json!({"includeLayers": false}))
                .await?;
            if let Some(servers) = config["config"]["mcp_servers"].as_object() {
                let overrides = start_params
                    .get_mut("config")
                    .and_then(Value::as_object_mut)
                    .unwrap();
                for name in servers.keys() {
                    overrides.insert(format!("mcp_servers.{name}.enabled"), false.into());
                }
            }
        }
        let thread = if let Some(resume) = &request.resume {
            let mut p = start_params.clone();
            p.insert("threadId".into(), Value::String(resume.clone()));
            match client.request("thread/resume", Value::Object(p)).await {
                Ok(thread) => thread,
                // A missing/foreign rollout falls back to a fresh thread.
                Err(e) => {
                    if command_request(&request.prompt, resume)?.is_some() {
                        return Err(e);
                    }
                    tracing::debug!(
                        target: "zeron_harness::codex",
                        "thread/resume failed (starting fresh): {e}"
                    );
                    client
                        .request("thread/start", Value::Object(start_params.clone()))
                        .await?
                }
            }
        } else {
            client
                .request("thread/start", Value::Object(start_params.clone()))
                .await?
        };
        let thread_id = thread["thread"]["id"].as_str().unwrap_or("").to_owned();
        let mut children = subagents::Subagents::new(thread_id.clone());
        children.restore(&thread["thread"]);
        Ok::<_, HarnessError>((thread_id, children))
    };
    let (thread_id, mut children) = tokio::select! {
        res = setup => match res {
            Ok(thread_id) => thread_id,
            Err(e) => {
                let _ = event_tx
                    .send(Ok(AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some(e.to_string()),
                        session_id: None,
                    }))
                    .await;
                shutdown_child(&mut child, kill_grace).await;
                return;
            }
        },
        _ = interrupt.cancelled() => {
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: None,
                }))
                .await;
            shutdown_child(&mut child, kill_grace).await;
            return;
        }
    };

    let turn_params = |text: &str| -> Value {
        let mut p = serde_json::Map::new();
        p.insert("threadId".into(), Value::String(thread_id.clone()));
        p.insert("input".into(), prompt_input(text));
        p.insert("approvalPolicy".into(), approval_policy.into());
        p.insert(
            "sandboxPolicy".into(),
            sandbox_policy_value(request.sandbox),
        );
        // Reasoning summaries stream (`item/reasoning/summaryTextDelta`) only
        // when asked for — without this codex "thinks" in silence for minutes:
        // nothing renders and the UI's 45s staleness gate flips Working off
        // (user report: "not streaming, doesn't say it's working").
        p.insert("summary".into(), "auto".into());
        {
            let mode = request
                .model_options
                .get(zeron_proto::AGENT_MODE_OPTION)
                .and_then(Value::as_str)
                .unwrap_or("default");
            if request
                .model_options
                .contains_key(zeron_proto::AGENT_MODE_OPTION)
            {
                p.insert("collaborationMode".into(), json!({
                    "mode": if mode == "plan" { "plan" } else { "default" },
                    "settings": { "model": request.model.as_deref(), "reasoning_effort": effort, "developer_instructions": null },
                }));
            }
            if request
                .model_options
                .contains_key(zeron_proto::AGENT_MODE_OPTION)
            {
                p.insert("zeronGoalMode".into(), (mode == "goal").into());
            }
        }
        if let Some(model) = &request.model {
            p.insert("model".into(), Value::String(model.clone()));
        }
        if let Some(effort) = effort {
            p.insert("effort".into(), effort.into());
        }
        if let Some(tier) = &service_tier {
            p.insert("serviceTier".into(), Value::String(tier.clone()));
        }
        Value::Object(p)
    };

    let mut assistant_message_id = new_message_id();
    if !send(
        &event_tx,
        AgentEvent::SessionStarted {
            harness: HarnessId::Codex,
            model: request.model.clone().unwrap_or_default(),
            tools: Vec::new(),
            cwd: request.cwd.clone(),
            session_id: thread_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
        },
    )
    .await
    {
        shutdown_child(&mut child, kill_grace).await;
        return;
    }

    let mut router = TurnRouter::default();
    match start_turn(&client, turn_params(&request.prompt)).await {
        Ok(id) => router.adopt_started(id),
        Err(e) => {
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(e.to_string()),
                    session_id: Some(thread_id.clone()),
                }))
                .await;
            shutdown_child(&mut child, kill_grace).await;
            return;
        }
    }

    // ---- main loop --------------------------------------------------------
    // Deltas seen per agent-message item, so a model that never streams
    // (item/completed only) still emits its text exactly once.
    let mut streamed_text: HashSet<String> = HashSet::new();
    let mut plan_text: HashMap<String, String> = HashMap::new();
    let (answer_tx, mut answer_rx) = mpsc::unbounded_channel::<SteerMessage>();
    let mut reasoning_streams: HashMap<String, ReasoningStream> = HashMap::new();
    // Token usage is held until the turn ends, emitted just before Done.
    let mut pending_usage: Option<AgentEvent> = None;
    // Steers whose `turn/steer` lost the turn-completed race; delivered as the
    // next `turn/start` when the expected turn's end notification arrives.
    let mut queued_steers: VecDeque<String> = VecDeque::new();
    // Native requests can expire before the user answers. Keep their bridge
    // waiters cancellable by `serverRequest/resolved` so dropping the receiver
    // becomes observable to the engine's pending-input cleanup.
    let mut pending_server_requests = HashMap::<String, tokio::task::AbortHandle>::new();
    // Assistant-message questions are not JSON-RPC requests and have no
    // provider cancellation notification, so their bridge waits are owned by
    // the run and must end with it.
    let mut assistant_question_waiters = Vec::<tokio::task::AbortHandle>::new();
    let mut steering_open = true;
    let mut interrupted = false;
    let mut interrupt_sent = false;
    // A Done has been emitted for the turn currently/last in flight.
    let mut done_current = false;
    let mut current_native = command_request(&request.prompt, &thread_id)
        .ok()
        .flatten()
        .is_some();
    let mut done_after_interrupt = false;
    let mut escalation: Option<tokio::task::JoinHandle<()>> = None;

    'main: loop {
        tokio::select! {
            inc = incoming.recv() => match inc {
                Some(Incoming::Notification { method, params }) => {
                // Foreign-thread traffic FIRST: a child thread's turn/thread
                // bookkeeping must never reach the parent turn router below
                // (a child's turn/completed would settle the PARENT turn).
                if let Some(nthread) = notification_thread_id(&method, &params)
                    && !nthread.is_empty()
                    && nthread != thread_id
                {
                    match route_child_notification(&method) {
                        ChildRoute::Parent => {
                            // Unknown/parent-owned: fall through so a codex
                            // update degrades to "the parent sees it",
                            // never silent loss.
                        }
                        ChildRoute::Consumed => continue,
                        ChildRoute::Subagent => {
                            for event in children.notification(&nthread, &method, &params) {
                                if !send(&event_tx, event).await {
                                    break 'main;
                                }
                            }
                            continue;
                        }
                    }
                }
                match method.as_str() {
                    "serverRequest/resolved" => {
                        if let Some(key) = request_id_key(params.get("requestId"))
                            && let Some(waiter) = pending_server_requests.remove(&key)
                        {
                            waiter.abort();
                        }
                    }
                    "turn/started" => {
                        let id = turn_id(&params);
                        if !router.is_completed(&id) { done_current = false; }
                        router.note_started(id);
                    }

                    "item/agentMessage/delta" => {
                        streamed_text.insert(item_id(&params));
                        if let Some(text) = delta_text(&params)
                            && !send(&event_tx, AgentEvent::TextDelta { text }).await
                        {
                            break 'main;
                        }
                    }

                    "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta"
                    | "item/reasoning/summaryPartAdded" => {
                        for event in reasoning_streams.entry(thread_id.clone()).or_default()
                            .map(&method, &params)
                        {
                            if !send(&event_tx, event).await {
                                break 'main;
                            }
                        }
                    }

                    "item/plan/delta" => {
                        let id = item_id(&params);
                        let text = plan_text.entry(id.clone()).or_default();
                        if let Some(delta) = delta_text(&params) { text.push_str(&delta); }
                        if !send(&event_tx, AgentEvent::ToolCall { id, call: zeron_proto::ToolCall::Plan { text: text.clone() } }).await { break 'main; }
                    }
                    "item/started" | "item/completed" => {
                        let phase = if method == "item/started" {
                            Phase::Started
                        } else {
                            Phase::Completed
                        };
                        let item = params.get("item").unwrap_or(&Value::Null);
                        if phase == Phase::Completed {
                            let output = match item_type(item) {
                                "exitedReviewMode" => item.get("review").and_then(Value::as_str),
                                _ => None,
                            };
                            if let Some(text) = output
                                && !send(&event_tx, AgentEvent::TextDelta { text: text.into() }).await
                            { break 'main; }
                        }
                        if matches!(item_type(item), "agentMessage" | "agent_message") {
                            if phase == Phase::Completed {
                                if let Some(questions) = item.get("questions").and_then(Value::as_array).filter(|questions| !questions.is_empty()) {
                                    let questions: Vec<UserInputQuestion> = questions.iter().map(|question| UserInputQuestion {
                                        id: new_message_id(), header: "Question".into(),
                                        question: question["title"].as_str().unwrap_or_default().into(),
                                        options: question["options"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
                                        multi_select: false, option_descriptions: Vec::new(), allow_custom: true, non_blocking: true,
                                    }).collect();
                                    let ask = Arc::clone(&request_input);
                                    let answers = answer_tx.clone();
                                    let waiter = tokio::spawn(async move {
                                        let response = (ask)(questions.clone()).await.unwrap_or_default();
                                        let lines: Vec<String> = questions.iter().filter_map(|question| response.iter()
                                            .find(|answer| answer.question_id == question.id && !answer.labels.is_empty())
                                            .map(|answer| format!("{}: {}", question.question, answer.labels.join(", ")))).collect();
                                        if !lines.is_empty() {
                                            let _ = answers.send(SteerMessage { prompt: format!("Answers to your questions:\n{}", lines.join("\n")), message_id: None });
                                        }
                                    });
                                    assistant_question_waiters.push(waiter.abort_handle());
                                }
                                // Fallback for non-streamed messages only.
                                let id = item.get("id").and_then(Value::as_str).unwrap_or("");
                                let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                                if !streamed_text.contains(id)
                                    && !text.is_empty()
                                    && !send(&event_tx, AgentEvent::TextDelta { text: text.into() }).await
                                {
                                    break 'main;
                                }
                                // Codex emits several assistant messages per
                                // turn (commentary, final answer); their
                                // deltas carry no separator, so consecutive
                                // messages rendered concatenated
                                // ("…waiting.Beta's 90-second…" — live
                                // finding). Close each message as a
                                // paragraph.
                                if !send(
                                    &event_tx,
                                    AgentEvent::TextDelta {
                                        text: "\n\n".into(),
                                    },
                                )
                                .await
                                {
                                    break 'main;
                                }
                                // Deltas are token chunks, not steering
                                // boundaries: the completed item is the
                                // provider-authoritative end of the text part.
                                let (prev, _next) = rotate(&mut assistant_message_id);
                                if !send(
                                    &event_tx,
                                    AgentEvent::AssistantMessageCompleted {
                                        assistant_message_id: prev,
                                    },
                                )
                                .await
                                {
                                    break 'main;
                                }
                            }
                        } else {
                            for ev in children.parent_item(phase, item) {
                                if !send(&event_tx, ev).await {
                                    break 'main;
                                }
                            }
                        }
                    }

                    "turn/plan/updated" | "thread/goal/updated" | "thread/goal/cleared" => {
                        for event in normalize::activity_events(&method, &params) {
                            if !send(&event_tx, event).await { break 'main; }
                        }
                    }

                    "thread/tokenUsage/updated" => {
                        if let Some(usage) = normalize::context_usage_event(&params)
                            && !send(&event_tx, usage).await { break 'main; }
                        if let Some(usage) = usage_event(&params) {
                            pending_usage = Some(usage);
                        }
                    }

                    "turn/completed" => {
                        let id = turn_id(&params);
                        router.note_completed(&id);
                        // Item ids never span turns; without this the set grew
                        // one entry per message for a persistent session's life.
                        streamed_text.clear();
                        plan_text.clear();
                        if let Some(usage) = pending_usage.take()
                            && !send(&event_tx, usage).await
                        {
                            break 'main;
                        }
                        let error = turn_error_message(&params).or_else(|| {
                            (params
                                .pointer("/turn/status")
                                .and_then(Value::as_str)
                                == Some("failed"))
                            .then(|| "Codex turn failed".to_owned())
                        });
                        let status = if interrupted {
                            DoneStatus::Interrupted
                        } else if error.is_some() {
                            DoneStatus::Errored
                        } else {
                            DoneStatus::Completed
                        };
                        done_current = true;
                        if !send(
                            &event_tx,
                            AgentEvent::Done {
                                status,
                                result: None,
                                error,
                                session_id: Some(thread_id.clone()),
                            },
                        )
                        .await
                        {
                            break 'main;
                        }
                        if interrupted {
                            done_after_interrupt = true;
                            break 'main;
                        }
                        // Persistent session: a steer that lost the race with
                        // this turn's end becomes the next turn now; otherwise
                        // stay alive for the mailbox — the caller owns teardown.
                        current_native = false;
                        if let Some(text) = queued_steers.pop_front() {
                            current_native = command_request(&text, &thread_id).ok().flatten().is_some();
                            if !steer_as_new_turn(
                                &client,
                                turn_params(&text),
                                &mut router,
                                &event_tx,
                                &mut assistant_message_id,
                                &mut done_current,
                            )
                            .await
                            {
                                break 'main;
                            }
                        } else if !steering_open {
                            break 'main;
                        }
                    }

                    "turn/failed" => {
                        router.note_completed(&turn_id(&params));
                        if let Some(usage) = pending_usage.take()
                            && !send(&event_tx, usage).await
                        {
                            break 'main;
                        }
                        done_current = true;
                        if interrupted {
                            done_after_interrupt = true;
                        }
                        let _ = send(
                            &event_tx,
                            AgentEvent::Done {
                                status: if interrupted {
                                    DoneStatus::Interrupted
                                } else {
                                    DoneStatus::Errored
                                },
                                result: None,
                                error: Some(
                                    turn_error_message(&params)
                                        .unwrap_or_else(|| "Codex turn failed".into()),
                                ),
                                session_id: Some(thread_id.clone()),
                            },
                        )
                        .await;
                        break 'main;
                    }

                    "turn/aborted" => {
                        router.note_completed(&turn_id(&params));
                        done_current = true;
                        if interrupted {
                            done_after_interrupt = true;
                        }
                        let _ = send(
                            &event_tx,
                            AgentEvent::Done {
                                status: DoneStatus::Interrupted,
                                result: None,
                                error: None,
                                session_id: Some(thread_id.clone()),
                            },
                        )
                        .await;
                        break 'main;
                    }

                    "error" => {
                        // 0.146.x nests it (`params.error.message`); older
                        // builds were flat (`params.message`) — accept both.
                        let message = params
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .or_else(|| params.get("message").and_then(Value::as_str))
                            .unwrap_or("Codex error")
                            .to_owned();
                        if !send(&event_tx, AgentEvent::Error { message }).await {
                            break 'main;
                        }
                    }

                    // thread/status, mcpServer startup, account noise, … —
                    // unknown notification methods are tolerated by design.
                    _ => {}
                }
                }

                Some(Incoming::Request { id, method, params }) => {
                    if let Some((key, waiter)) = handle_server_request(
                        &client,
                        id,
                        &method,
                        &params,
                        request.auto_approve,
                        &request_input,
                    ) {
                        if let Some(previous) = pending_server_requests.insert(key, waiter) {
                            previous.abort();
                        }
                    }
                }

                // stdout EOF or reader gone: the app server exited.
                Some(Incoming::Eof) | None => break 'main,
            },

            steer = async {
                tokio::select! { steer = steering.recv() => steer, answer = answer_rx.recv() => answer }
            }, if steering_open && !interrupted => match steer {
                Some(msg) => {
                    let text = msg.prompt;
                    // Native operations run at a turn boundary, never as text
                    // injected into an already running model turn. Later messages
                    // must stay behind queued commands: Steered acknowledgments
                    // retire the engine's accepted-message ledger in FIFO order.
                    if !done_current && (!queued_steers.is_empty() || current_native || !matches!(command_request(&text, &thread_id), Ok(None))) {
                        queued_steers.push_back(text);
                        continue 'main;
                    }
                    if let Some(expected) = router.active.clone() {
                        let steer_params = json!({
                            "threadId": thread_id,
                            "expectedTurnId": expected,
                            "input": prompt_input(&text),
                        });
                        match client.request("turn/steer", steer_params).await {
                            Ok(_) => {
                                let (prev, next) = rotate(&mut assistant_message_id);
                                if !send(
                                    &event_tx,
                                    AgentEvent::Steered {
                                        assistant_message_id: Some(prev),
                                        next_assistant_message_id: Some(next),
                                    },
                                )
                                .await
                                {
                                    break 'main;
                                }
                            }
                            // A failed `turn/steer` does NOT mean the text is
                            // bad: most commonly the active turn finished
                            // between the UI send and this request. Queue it
                            // for redelivery as the next `turn/start` when the
                            // expected turn's end arrives (also the safe
                            // fallback for older Codex without steering).
                            Err(e) => {
                                tracing::debug!(
                                    target: "zeron_harness::codex",
                                    "turn/steer rejected (queued as next turn): {e}"
                                );
                                if router.active.as_deref() == Some(expected.as_str())
                                    && !router.is_completed(&expected)
                                {
                                    queued_steers.push_back(text);
                                } else {
                                    current_native = command_request(&text, &thread_id).ok().flatten().is_some();
                                    if !steer_as_new_turn(
                                        &client, turn_params(&text), &mut router, &event_tx,
                                        &mut assistant_message_id, &mut done_current,
                                    ).await { break 'main; }
                                }
                            }
                        }
                    } else {
                        current_native = command_request(&text, &thread_id).ok().flatten().is_some();
                        if !steer_as_new_turn(
                            &client, turn_params(&text), &mut router, &event_tx,
                            &mut assistant_message_id, &mut done_current,
                        ).await { break 'main; }
                    }
                }
                None => {
                    // Mailbox closed (the caller's graceful idle-reap): finish
                    // once nothing is in flight — mirrors codex.ts's steer loop
                    // `finish()` on a null take.
                    steering_open = false;
                    if done_current && router.active.is_none() && queued_steers.is_empty() {
                        break 'main;
                    }
                }
            },

            _ = interrupt.cancelled(), if !interrupt_sent => {
                interrupt_sent = true;
                interrupted = true;
                if let Some(turn) = router.active.clone() {
                    let client = client.clone();
                    let thread = thread_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = client
                            .request("turn/interrupt", json!({ "threadId": thread, "turnId": turn }))
                            .await
                        {
                            tracing::debug!(
                                target: "zeron_harness::codex",
                                "turn/interrupt failed (escalation will reap): {e}"
                            );
                        }
                    });
                    // Escalate if the app server doesn't wind down (turn/aborted)
                    // within the grace periods: SIGTERM, then SIGKILL.
                    if let Some(pid) = crate::process::signal_target(&child) {
                        escalation = Some(tokio::spawn(async move {
                            tokio::time::sleep(interrupt_grace).await;
                            send_signal(&pid, Signal::Term);
                            tokio::time::sleep(kill_grace).await;
                            send_signal(&pid, Signal::Kill);
                        }));
                    }
                } else {
                    // Idle between turns: nothing to interrupt — the terminal
                    // bookkeeping below still guarantees Done { Interrupted }.
                    break 'main;
                }
            },

            _ = event_tx.closed() => break 'main,
        }
    }

    // Detached request waiters otherwise outlive a crashed/interrupted app
    // server and keep the engine response channels falsely open.
    for waiter in pending_server_requests.into_values() {
        waiter.abort();
    }
    for waiter in assistant_question_waiters {
        waiter.abort();
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
                    session_id: Some(thread_id.clone()),
                }))
                .await;
        } else if !interrupted && !done_current {
            // A child KILLED mid-turn (OS memory pressure, `killall codex`)
            // must not read as a silent success — codex.ts's signal-death
            // handling, reduced to the turn-in-flight case.
            let status = child.try_wait().ok().flatten();
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(crate::crash_message(
                        "codex app-server",
                        status,
                        &stderr_tail,
                    )),
                    session_id: Some(thread_id.clone()),
                }))
                .await;
        }
    }

    shutdown_child(&mut child, kill_grace).await;
    if let Some(handle) = escalation {
        handle.abort();
    }
}

/// Deliver a steer as a fresh `turn/start` on the same thread (the fallback
/// leg of the steer race, and the between-turns delivery path). Returns false
/// when the loop should end (turn/start failed or the consumer hung up).
async fn steer_as_new_turn(
    client: &RpcClient,
    params: Value,
    router: &mut TurnRouter,
    event_tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
    assistant_message_id: &mut String,
    done_current: &mut bool,
) -> bool {
    match start_turn(client, params).await {
        Ok(id) => {
            router.adopt_started(id);
            *done_current = false;
            let (prev, next) = rotate(assistant_message_id);
            send(
                event_tx,
                AgentEvent::Steered {
                    assistant_message_id: Some(prev),
                    next_assistant_message_id: Some(next),
                },
            )
            .await
        }
        Err(e) => {
            let _ = send(
                event_tx,
                AgentEvent::Error {
                    message: format!("Steering failed: {e}"),
                },
            )
            .await;
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Approvals (approval-as-input parity with zeron's UX)
// ---------------------------------------------------------------------------

type RequestInputFn = Box<
    dyn Fn(Vec<UserInputQuestion>) -> tokio::sync::oneshot::Receiver<Vec<UserInputAnswer>>
        + Send
        + Sync,
>;

/// Serve one server→client request. Approval requests round-trip through
/// `request_input` as a synthesized yes/no question (in a subtask so the
/// message loop keeps flowing); with `auto_approve` they're accepted outright
/// (belt to the wire-level `approvalPolicy: "never"`). Anything else is
/// rejected as unsupported so the server never wedges awaiting a reply.
fn handle_server_request(
    client: &RpcClient,
    id: Value,
    method: &str,
    params: &Value,
    auto_approve: bool,
    request_input: &Arc<RequestInputFn>,
) -> Option<(String, tokio::task::AbortHandle)> {
    // A tool's user-input request (EXPERIMENTAL, codex 0.146.x) is a CONTENT
    // question, never auto-approvable — route it to the input bridge and
    // answer keyed by question id, `{ answers: { <id>: { answers: [..] } } }`.
    if method == "item/tool/requestUserInput" {
        if params["questions"].as_array().is_some_and(|questions| {
            questions
                .iter()
                .any(|question| question["isSecret"] == true)
        }) {
            client.respond_error(
                &id,
                -32602,
                "Secret input is not supported by this client; use an external credential flow",
            );
            return None;
        }
        let questions = user_input_questions(params);
        if questions.is_empty() {
            client.respond(&id, json!({ "answers": {} }));
            return None;
        }
        if questions
            .iter()
            .any(|(_, question)| !question.allow_custom && question.options.is_empty())
        {
            // An impossible choice-only request is a native cancellation, not
            // a free-text prompt whose answer cannot be represented faithfully.
            client.respond(&id, json!({ "answers": {} }));
            return None;
        }
        if !valid_user_input_request(&questions) {
            client.respond_error(&id, -32602, "Invalid or ambiguous question payload");
            return None;
        }
        let client = client.clone();
        let request_input = Arc::clone(request_input);
        let key = request_id_key(Some(&id));
        let waiter = tokio::spawn(async move {
            let asked: Vec<UserInputQuestion> = questions.iter().map(|(_, q)| q.clone()).collect();
            let answers = match (request_input)(asked).await {
                Ok(answers) => answers,
                Err(_) => {
                    client.respond_error(
                        &id,
                        -32603,
                        "The question response channel closed before an answer was delivered",
                    );
                    return;
                }
            };
            match user_input_response(&questions, &answers) {
                Ok(response) => client.respond(&id, response),
                Err(message) => client.respond_error(&id, -32602, message),
            }
        });
        return key.map(|key| (key, waiter.abort_handle()));
    }
    let is_approval = matches!(
        method,
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
    );
    if !is_approval {
        tracing::debug!(
            target: "zeron_harness::codex",
            "unhandled server request: {method}"
        );
        client.respond_error(&id, -32601, &format!("unsupported method: {method}"));
        return None;
    }
    if auto_approve {
        client.respond(&id, json!({ "decision": "accept" }));
        return None;
    }

    let question = approval_question(method, params);
    let client = client.clone();
    let request_input = Arc::clone(request_input);
    let key = request_id_key(Some(&id));
    let waiter = tokio::spawn(async move {
        // The engine's input bridge owns the `InputRequested`/`InputResolved`
        // lifecycle (it mints the request id the resolver is parked under);
        // emitting our own copy here doubled the doc's input part with an id
        // `respond_input` could never match.
        //
        // A dropped sender (caller went away) degrades to a decline so the
        // agent is unblocked — never silently allowed.
        let answers = (request_input)(vec![question.clone()])
            .await
            .unwrap_or_default();
        let accept = answers.iter().any(|a| {
            a.question_id == question.id && a.labels.iter().any(|l| l.eq_ignore_ascii_case("yes"))
        });
        client.respond(
            &id,
            json!({ "decision": if accept { "accept" } else { "decline" } }),
        );
    });
    key.map(|key| (key, waiter.abort_handle()))
}

/// JSON-RPC request ids may be strings or numbers. Their JSON representation
/// is stable across the request and its `serverRequest/resolved` notification.
fn request_id_key(id: Option<&Value>) -> Option<String> {
    serde_json::to_string(id?).ok()
}

fn valid_user_input_request(questions: &[(String, UserInputQuestion)]) -> bool {
    let mut wire_ids = HashSet::new();
    questions
        .iter()
        .all(|(wire_id, _)| !wire_id.trim().is_empty() && wire_ids.insert(wire_id.clone()))
        && zeron_proto::valid_input_questions(
            &questions
                .iter()
                .map(|(_, question)| question.clone())
                .collect::<Vec<_>>(),
        )
}

/// Keep UI question identity separate from native wire identity. Never turn an
/// unknown/missing answer into a successful empty answer for the provider.
fn user_input_response(
    questions: &[(String, UserInputQuestion)],
    answers: &[zeron_proto::UserInputAnswer],
) -> Result<Value, &'static str> {
    let asked: Vec<_> = questions
        .iter()
        .map(|(_, question)| question.clone())
        .collect();
    if !zeron_proto::valid_input_answers(&asked, answers) {
        return Err("Invalid answer IDs, choices, or cardinality");
    }
    if answers.is_empty() {
        return Ok(json!({"answers": {}}));
    }
    let by_id: serde_json::Map<String, Value> = questions
        .iter()
        .map(|(wire_id, question)| {
            let labels = answers
                .iter()
                .find(|answer| answer.question_id == question.id)
                .map(|answer| answer.labels.clone())
                .unwrap_or_default();
            (wire_id.clone(), json!({"answers": labels}))
        })
        .collect();
    Ok(json!({"answers":by_id}))
}

/// Parse `item/tool/requestUserInput` questions into (wire id, question)
/// pairs, tolerant of field spellings; answers key by the WIRE id.
fn user_input_questions(params: &Value) -> Vec<(String, UserInputQuestion)> {
    params
        .get("questions")
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .map(|q| {
            let field = |keys: [&str; 3]| {
                keys.iter()
                    .find_map(|k| q.get(*k).and_then(Value::as_str))
                    .unwrap_or("")
                    .to_owned()
            };
            // Preserve a missing or blank provider id so validation rejects
            // the malformed native request instead of inventing a key that
            // Codex never sent and cannot match in the response.
            let wire_id = field(["id", "questionId", "question_id"]);
            let question = UserInputQuestion {
                id: new_message_id(),
                header: {
                    let h = field(["header", "title", "label"]);
                    if h.is_empty() {
                        "Codex question".into()
                    } else {
                        h
                    }
                },
                question: field(["question", "prompt", "text"]),
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
                option_descriptions: q
                    .get("options")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|option| {
                        option
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .collect(),
                allow_custom: q
                    .get("options")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty)
                    || q.get("isOther").and_then(Value::as_bool).unwrap_or(true),
                non_blocking: params.get("isBlocking").and_then(Value::as_bool) == Some(false),
                multi_select: ["multiSelect", "multi_select"]
                    .iter()
                    .find_map(|k| q.get(*k).and_then(Value::as_bool))
                    .unwrap_or(false),
            };
            (wire_id, question)
        })
        .collect()
}

/// Synthesize the yes/no question an approval request surfaces to the user.
fn approval_question(method: &str, params: &Value) -> UserInputQuestion {
    let (header, question) = if method.contains("commandExecution") {
        let command = match params.get("command") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        (
            "Approve command".to_owned(),
            if command.is_empty() {
                "Codex wants to run a command. Allow it?".to_owned()
            } else {
                format!("Codex wants to run `{command}`. Allow it?")
            },
        )
    } else {
        let paths: Vec<&str> = params
            .get("changes")
            .and_then(Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|c| c.get("path").and_then(Value::as_str))
            .collect();
        (
            "Approve file change".to_owned(),
            if paths.is_empty() {
                "Codex wants to modify files. Allow it?".to_owned()
            } else {
                format!("Codex wants to modify {}. Allow it?", paths.join(", "))
            },
        )
    };
    UserInputQuestion {
        id: new_message_id(),
        header,
        question,
        options: vec!["Yes".into(), "No".into()],
        option_descriptions: Vec::new(),
        allow_custom: false,
        non_blocking: false,
        multi_select: false,
    }
}

use crate::{Signal, send_signal, shutdown_child};

#[cfg(test)]
mod tests {
    #[test]
    fn question_answers_preserve_native_ids_and_never_silently_drop_text() {
        let questions = user_input_questions(
            &json!({"questions":[{"id":"native-id","question":"What next?","isOther":true,"options":[{"label":"Plan"}]}]}),
        );
        let answer = zeron_proto::UserInputAnswer {
            question_id: questions[0].1.id.clone(),
            labels: vec!["Build a feature with café 日本語".into()],
        };
        assert_eq!(
            user_input_response(&questions, &[answer]).unwrap(),
            json!({"answers":{"native-id":{"answers":["Build a feature with café 日本語"]}}})
        );
        assert!(
            user_input_response(
                &questions,
                &[zeron_proto::UserInputAnswer {
                    question_id: "wrong-id".into(),
                    labels: vec!["Plan".into()]
                }]
            )
            .is_err()
        );
        assert_eq!(
            user_input_response(&questions, &[]).unwrap(),
            json!({"answers":{}})
        );
    }

    #[test]
    fn question_payload_rejects_ambiguous_native_keys_and_labels() {
        let duplicate_ids = user_input_questions(&json!({"questions":[
            {"id":"same","question":"First","isOther":true},
            {"id":"same","question":"Second","isOther":true}
        ]}));
        assert!(!valid_user_input_request(&duplicate_ids));

        let duplicate_labels = user_input_questions(&json!({"questions":[{
            "id":"q","question":"Choose","isOther":false,
            "options":[{"label":"Same"},{"label":"Same"}]
        }]}));
        assert!(!valid_user_input_request(&duplicate_labels));

        for malformed in [
            json!({"questions":[{"id":"  ","question":"Blank","isOther":true}]}),
            json!({"questions":[{"question":"Missing","isOther":true}]}),
        ] {
            let questions = user_input_questions(&malformed);
            assert!(!valid_user_input_request(&questions));
            assert_eq!(
                questions[0].0,
                malformed["questions"][0]["id"].as_str().unwrap_or("")
            );
        }
    }

    #[test]
    fn explicit_unknown_mode_is_rejected() {
        let mut request = RunRequest {
            prompt: "test".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: String::new(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
        };
        request
            .model_options
            .insert(zeron_proto::AGENT_MODE_OPTION.into(), "invented".into());
        assert!(validate_agent_mode(&request).is_err());
    }

    #[test]
    fn question_capability_contract() {
        let mapped = user_input_questions(
            &serde_json::json!({"questions":[{"id":"q","question":"Choose","isOther":false,"options":[{"label":"One","description":"First option"}]}]}),
        );
        assert!(!mapped[0].1.allow_custom);
        assert_eq!(mapped[0].1.option_descriptions, ["First option"]);
        assert!(
            !approval_question(
                "item/commandExecution/requestApproval",
                &serde_json::json!({})
            )
            .allow_custom
        );
    }

    use super::*;
    use serde_json::json;

    #[test]
    fn modes_require_live_protocol_and_enabled_goal_feature() {
        assert!(advertised_mode_option(None, true).is_none());
        let modes = json!({"data":[{"mode":"default"},{"mode":"plan"}]});
        let option = advertised_mode_option(Some(&modes), false).unwrap();
        assert_eq!(
            option
                .choices
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            vec!["default", "plan"]
        );
        assert_eq!(
            advertised_mode_option(Some(&modes), true)
                .unwrap()
                .choices
                .len(),
            3
        );
        assert!(
            advertised_mode_option(Some(&json!({"data":[{"mode":"default"}]})), false).is_none()
        );
        assert!(catalog::static_models().iter().all(|m| {
            m.options
                .iter()
                .all(|o| o.id != zeron_proto::AGENT_MODE_OPTION)
        }));
    }

    #[test]
    fn approval_questions_are_yes_no() {
        let q = approval_question(
            "item/commandExecution/requestApproval",
            &json!({"itemId": "c1", "command": "rm -rf /tmp/x"}),
        );
        assert_eq!(q.header, "Approve command");
        assert!(q.question.contains("rm -rf /tmp/x"));
        assert_eq!(q.options, vec!["Yes".to_string(), "No".to_string()]);
        assert!(!q.multi_select);

        let q = approval_question(
            "item/fileChange/requestApproval",
            &json!({"changes": [{"path": "/a.rs"}, {"path": "/b.rs"}]}),
        );
        assert_eq!(q.header, "Approve file change");
        assert!(q.question.contains("/a.rs, /b.rs"));

        // Command as argv array joins with spaces.
        let q = approval_question(
            "item/commandExecution/requestApproval",
            &json!({"command": ["git", "push", "--force"]}),
        );
        assert!(q.question.contains("git push --force"));
    }

    #[test]
    fn model_page_skips_hidden_and_unknown_efforts() {
        let page = json!({
            "data": [
                {
                    "id": "hidden",
                    "model": "hidden",
                    "displayName": "Hidden",
                    "hidden": true,
                    "supportedReasoningEfforts": [{ "reasoningEffort": "high" }]
                },
                {
                    "id": "gpt-6-astra",
                    "model": "gpt-6-astra",
                    "displayName": "GPT-6-Astra",
                    "description": "  Most capable  ",
                    "hidden": false,
                    "supportedReasoningEfforts": [
                        { "reasoningEffort": "high" },
                        { "reasoningEffort": "future" }
                    ],
                    "serviceTiers": [{ "id": "priority", "name": "Fast" }],
                    "additionalSpeedTiers": ["fast"],
                    "defaultServiceTier": null,
                    "isDefault": true
                }
            ],
            "nextCursor": "next"
        });
        let (models, cursor) = parse_model_list_page(&page);
        assert_eq!(cursor.as_deref(), Some("next"));
        assert_eq!(models.len(), 1);
        let (astra, is_default) = &models[0];
        assert_eq!(astra.id, "gpt-6-astra");
        assert_eq!(astra.description.as_deref(), Some("Most capable"));
        assert_eq!(astra.reasoning_levels, vec![ReasoningLevel::High]);
        assert!(*is_default);
        assert_eq!(astra.options[0].choices.len(), 2);
        assert_eq!(astra.options[0].choices[1].id, "fast");
    }

    #[test]
    fn turn_router_never_revives_completed_turns() {
        let mut r = TurnRouter::default();
        r.note_completed("t-1");
        // The turn/start response arriving after turn/completed must not
        // resurrect the turn.
        r.adopt_started("t-1".into());
        assert_eq!(r.active, None);
        // Nor may a late turn/started notification.
        r.note_started("t-1".into());
        assert_eq!(r.active, None);

        r.note_started("t-2".into());
        assert_eq!(r.active.as_deref(), Some("t-2"));
        // A replacement started turn retires the stale one.
        r.note_started("t-3".into());
        assert_eq!(r.active.as_deref(), Some("t-3"));
        assert!(r.is_completed("t-2"));
    }
}

#[cfg(test)]
mod skill_discovery_tests {
    use super::*;

    #[test]
    fn request_preflight_decodes_selected_commands_before_attachment_check() {
        let command = zeron_proto::invocation::Invocation::Command {
            name: "review".into(),
        };
        let request = RunRequest {
            prompt: format!("  {} check errors", command.link()),
            harness: Some(HarnessId::Codex),
            model: None,
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: String::new(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            resume: None,
            attachments: vec!["/tmp/image.png".into()],
            worktree: None,
        };
        assert_eq!(
            CodexHarness::default()
                .validate_request(&request)
                .unwrap_err()
                .to_string(),
            "harness protocol error: Codex commands cannot include attachments; send them in a separate prompt"
        );
    }

    #[test]
    fn selected_skills_use_native_identity_for_initial_and_steered_inputs() {
        use zeron_proto::invocation::Invocation;
        let a = Invocation::Skill {
            command: None,
            name: "review".into(),
            path: "/repo/a b/SKILL.md".into(),
        };
        let b = Invocation::Skill {
            command: None,
            name: "review".into(),
            path: "/repo/other/SKILL.md".into(),
        };
        let raw = format!("Use {} then {} and {}", a.link(), b.link(), a.link());
        let input = prompt_input(&raw);
        assert_eq!(input.as_array().unwrap().len(), 3);
        assert_eq!(
            input[1],
            json!({"type":"skill","name":"review","path":"/repo/a b/SKILL.md"})
        );
        assert_eq!(input[2]["path"], "/repo/other/SKILL.md");
        assert!(!input[0]["text"].as_str().unwrap().contains("zeron-invoke:"));
        for raw in [
            "$review".into(),
            format!("`{}`", a.link()),
            format!("\\{}", a.link()),
            format!("![skill example {}](example.png)", a.link()),
            format!("    {}", a.link()),
        ] {
            let input = prompt_input(&raw);
            assert_eq!(input.as_array().unwrap().len(), 1);
            assert_eq!(input[0]["text"], raw);
        }
        let command = Invocation::Command {
            name: "review".into(),
        };
        assert_eq!(
            command_request(&format!("  {} check", command.link()), "t")
                .unwrap()
                .unwrap()
                .0,
            "review/start"
        );
        assert!(command_request(&format!("/review {}", a.link()), "t").is_err());
    }

    #[test]
    fn backtick_labels_keep_native_skill_identity_with_repeated_selections() {
        use zeron_proto::invocation::{Invocation, harness_prompt};
        let skill = Invocation::Skill {
            command: None,
            name: "review`ui".into(),
            path: "/repo/é skill/SKILL.md".into(),
        };
        let file = zeron_proto::file_mentions::local_file_link("src/a`b.rs", false);
        let raw = format!("{} on {file} and {}", skill.link(), skill.link());
        let input = prompt_input(&harness_prompt(&raw, HarnessId::Codex));
        assert_eq!(input.as_array().unwrap().len(), 2);
        assert_eq!(
            input[1],
            json!({"type":"skill", "name":"review`ui", "path":"/repo/é skill/SKILL.md"})
        );
        let text = input[0]["text"].as_str().unwrap();
        assert!(!text.contains("zeron-invoke:"));
        assert!(!text.contains("zeron-file:"));
        assert_eq!(text.matches("/repo/%C3%A9%20skill/SKILL.md").count(), 2);
    }

    #[test]
    fn commands_map_arguments_and_leave_inline_mentions_literal() {
        assert!(
            command_request("please /review this", "t")
                .unwrap()
                .is_none()
        );
        for code in [
            "    /review",
            "\t/review",
            "\n    /compact",
            "\u{a0}/review",
            "`/review`",
            "```\n/review\n```",
        ] {
            assert!(command_request(code, "t").unwrap().is_none());
        }
        assert!(command_request("/tmp/file.rs", "t").unwrap().is_none());
        assert!(command_request("/tmp", "t").unwrap().is_none());
        assert!(command_request("/compact extra", "t").is_err());
        assert!(command_request("/model", "t").is_err());
        let (method, params) = command_request("/review check errors", "t")
            .unwrap()
            .unwrap();
        assert_eq!(method, "review/start");
        assert_eq!(
            params["target"],
            json!({"type":"custom","instructions":"check errors"})
        );
        assert_eq!(params["delivery"], "inline");
    }

    #[test]
    fn catalog_rejects_invalid_identities_without_changing_valid_names() {
        use zeron_proto::invocation::{Invocation, invocation_links};
        let mut entries = vec![];
        for name in [
            "",
            "two words",
            " padded",
            "padded ",
            "line\nbreak",
            "tab\tname",
            "nul\0name",
            "non\u{a0}breaking",
        ] {
            entries.push(json!({"name":name,"path":"/repo/SKILL.md"}));
        }
        for path in ["", "/repo/line\nbreak/SKILL.md", "/repo/\0/SKILL.md"] {
            entries.push(json!({"name":"invalid-path","path":path}));
        }
        for (name, path) in [
            (r"review[ui]\draft`", "/repo/é skill/SKILL.md"),
            ("审查-é", "harness-skill:custom:审查-é"),
        ] {
            entries.push(json!({"name":name,"path":path}));
        }
        let skills = parse_skills(&json!({"data":[{"skills":entries}]}));
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, r"review[ui]\draft`");
        assert_eq!(skills[1].path, "harness-skill:custom:审查-é");
        for skill in skills {
            let invocation = Invocation::Skill {
                name: skill.name,
                path: skill.path,
                command: skill.command,
            };
            assert_eq!(invocation_links(&invocation.link())[0].1, invocation);
        }
    }

    #[test]
    fn preserves_paths_enabled_state_and_duplicate_names() {
        let value = json!({"data": [{"skills": [
            {"name":"review", "path":"/a/SKILL.md", "description":"A", "enabled":true},
            {"name":"review", "path":"/b/SKILL.md", "description":"B", "enabled":false},
            {"name":"review", "path":"/a/SKILL.md", "description":"duplicate"}
        ]}]});
        let skills = parse_skills(&value);
        assert_eq!(skills.len(), 2);
        assert_ne!(skills[0].path, skills[1].path);
        assert!(!skills[1].enabled);
    }
}
