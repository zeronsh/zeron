//! Codex harness: spawns the installed `codex` CLI as `codex app-server` and
//! speaks JSON-RPC 2.0 over stdio — the same interface the Codex IDE extension
//! uses. Resurrected from the pre-ACP driver and modernized.
//!
//! VERSION PIN: the app-server API is EXPERIMENTAL (`capabilities.
//! experimentalApi`); this driver is validated against codex-cli 0.153.4 —
//! imageGeneration additionally follows the 0.154.0 schema (savedPath only).
//! Revalidate the method/notification surface when bumping past it. A CLI
//! older than [`MIN_CODEX_VERSION`] is refused up front.
//!
//! - `initialize` handshake (clientInfo + `capabilities.experimentalApi`,
//!   opting out of the high-volume notifications nothing consumes) then the
//!   `initialized` notification; unknown notification methods tolerated.
//! - `thread/start` (or `thread/resume`, unarchiving an archived thread; only
//!   a conversation that is gone falls back to a fresh thread, with a visible
//!   notice) → `SessionStarted`; `turn/start` carries the prompt, model,
//!   effort, `sandboxPolicy`, approval policy and (plan mode) the
//!   `collaborationMode`.
//! - Notifications map to [`AgentEvent`]s: agentMessage/reasoning deltas (both
//!   `delta`/`textDelta` spellings), item lifecycles → typed ToolCall/ToolResult,
//!   `thread/tokenUsage/updated` → Usage, turn/completed|failed|aborted → Done.
//!   An `error` Codex will retry is progress, not an error; a final one is
//!   reported once, by the turn's Done.
//! - Approvals + sandbox follow the run's [`AccessMode`], on every
//!   `turn/start` (so a change applies from the next turn in place): full
//!   access is `"never"` + `danger-full-access`, as every run was before
//!   modes; auto-accept edits is `"untrusted"` + `workspace-write` (every
//!   command but codex's known-safe reads is asked, even inside the
//!   sandbox; file changes are accepted); supervised is `"untrusted"` +
//!   `read-only`. A title run stays `"never"`
//!   + `read-only`. `item/commandExecution/requestApproval`,
//!   `item/fileChange/requestApproval` and `item/permissions/requestApproval`
//!   the mode does not cover round-trip through
//!   [`RunControls::request_input`] as a yes/no question (a refusal
//!   declines); auto-accept edits accepts file changes outright. MCP
//!   elicitations (sent whatever the policy) are asked when they are a flat
//!   form and declined otherwise.
//! - Plan mode is Codex's `plan` collaboration mode. It sticks to the thread
//!   (across a resume too), so once a thread has planned every turn names
//!   its mode. The proposed plan arrives as its own `plan` item, rendered
//!   as reply text.
//! - Subagents are full child app-server threads. Parent spawn items establish
//!   their stable ownership; content arriving before the spawn is buffered.
//!   A registered child's notifications route through an EXPLICIT table
//!   ([`normalize::route_child_notification`]) — content, errors and child turns
//!   become tagged [`AgentEvent::Subagent`] events; unrelated child bookkeeping is
//!   consumed so it can never settle the parent turn, and unknown methods
//!   fall through to the parent path (fail open, never silent loss).
//! - Steering: `turn/steer { expectedTurnId, clientUserMessageId }` into the
//!   live turn, confirmed when its userMessage item (carrying that id) joins
//!   the turn; a rejected steer (the turn-completed race) is queued and
//!   delivered as the next `turn/start` on the same thread. The session is
//!   persistent across turns while the steering mailbox lives; a steer's
//!   changed model/effort/tier rides the next `turn/start`.
//! - Only the turn in flight settles: a stale or duplicate completion is
//!   ignored. Commands still running past their turn report as background
//!   work, so the host never reaps the app-server from under them.
//! - Turn stop ([`crate::TurnControl::stop_turn`]): `turn/interrupt` ends the
//!   turn `Interrupted`; the app-server and its threads (child agents
//!   included) stay up for the next `turn/start`.
//! - Interrupt: cancelling [`RunControls::interrupt`] tears the runtime down —
//!   `turn/interrupt`, escalating to SIGTERM → SIGKILL if the child is
//!   unresponsive; the stream always ends with `Done { status: Interrupted }`.

pub(crate) mod catalog;
mod elicitation;
mod normalize;
pub mod npm_repair;
pub mod realtime;
mod subagents;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, HarnessId, Model, ModelOption, ModelOptionChoice,
    ReasoningLevel, RunRequest, SandboxLevel, SlashCommand, SteeringMode, UserInputAnswer,
    UserInputQuestion,
};

use crate::jsonrpc::{Incoming, RpcClient, transport_lost};
use crate::process::{Child, Command, Stdio};
use crate::{Harness, HarnessError, RunControls};
use catalog::{REASONING_LEVELS, sandbox_mode, sandbox_policy_value, static_models, to_effort};
use normalize::{
    ChildRoute, Phase, ReasoningStream, delta_text, item_id, item_type, notification_thread_id,
    route_child_notification, settle_turn_error, turn_error_message, turn_id, usage_event,
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

/// `canonicalize` yields `\\?\`-prefixed verbatim paths on Windows, which
/// cmd.exe cannot launch batch shims through; keep the plain drive/UNC form.
fn plain_executable(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(rest) = path.to_str().and_then(|p| p.strip_prefix(r"\\?\")) {
            return match rest.strip_prefix(r"UNC\") {
                Some(share) => PathBuf::from(format!(r"\\{share}")),
                None => PathBuf::from(rest),
            };
        }
    }
    path
}

/// `thread/start` / `thread/resume` config keys that, set to false, take the
/// agent tools away (title runs, side-chats-only runs). The app-server merges
/// that `config` into its `-c` overrides (openai/codex rust-v0.153.4,
/// codex-rs/app-server/src/config_manager.rs `load_with_cli_overrides`), and
/// "Setting `agents.enabled=false` disables agent tools"
/// (docs/research/codex-subagents.md). An enabled `features.multi_agent_v2`
/// takes precedence over `agents.enabled` (codex-rs/core/config.schema.json).
const AGENT_TOOLS: [&str; 3] = [
    "features.multi_agent",
    "features.multi_agent_v2",
    "agents.enabled",
];

/// [`AGENT_TOOLS`] as config overrides that switch them off.
fn agent_tools_off() -> [(String, Value); 3] {
    AGENT_TOOLS.map(|key| (key.to_owned(), Value::Bool(false)))
}

/// Dotted `thread/start` config overrides that add an injected MCP server
/// to the user's `mcp_servers` table.
fn codex_mcp_overrides(mcp: &zeron_proto::McpServer) -> Vec<(String, Value)> {
    let key = |field: &str| format!("mcp_servers.{}.{field}", mcp.name);
    vec![
        (key("command"), mcp.command.clone().into()),
        (key("args"), json!(mcp.args)),
        (key("env"), json!(mcp.env)),
        // Our injected server's tools are Zeron's own: approve them in every
        // access mode, so Codex neither rejects a call under `never` nor
        // asks about one under the others.
        (key("default_tools_approval_mode"), "approve".into()),
    ]
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
    models_cache: crate::catalog::Catalog,
    executable: Option<PathBuf>,
    /// Grace between `turn/interrupt` and SIGTERM.
    interrupt_grace: Duration,
    /// Grace between SIGTERM and SIGKILL.
    kill_grace: Duration,
}

impl Default for CodexHarness {
    fn default() -> Self {
        Self {
            models_cache: crate::catalog::Catalog::default(),
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
            crate::executable::spawn_error(e, cwd, || crate::executable::binary_hint(&exe))
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
        client.close_stdin_for_teardown();
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
            crate::executable::spawn_error(e, None, || crate::executable::binary_hint(&exe))
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
                if legacy_model_page(&page) {
                    tracing::warn!(binary_path = %exe.display(), binary_version = ?crate::executable::binary_version(&exe), "Model discovery response lacks hidden flags; CLI may be outdated");
                }
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

            if let Some(default_id) = default_model_id
                && let Some(index) = models.iter().position(|model| model.id == default_id)
                && index != 0
            {
                let default_model = models.remove(index);
                models.insert(0, default_model);
            }
            if models.is_empty() {
                return Err(crate::CatalogFailure {
                    code: crate::CatalogFailureCode::Failed,
                    message: "Codex returned an empty model catalog".into(),
                }
                .into());
            }
            Ok::<Vec<Model>, HarnessError>(models)
        };
        let result = tokio::time::timeout(Duration::from_secs(10), discovery).await;
        client.close_stdin_for_teardown();
        shutdown_child(&mut child, self.kill_grace).await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(HarnessError::Protocol("model discovery timed out".into())),
        }
    }
}

/// The `serviceTier` a request runs with; "default" (Standard) is omitted.
fn service_tier(request: &RunRequest) -> Option<String> {
    request
        .model_options
        .get("serviceTier")
        .and_then(Value::as_str)
        .filter(|t| *t != "default")
        .map(str::to_owned)
}

/// What a request may do without asking: `auto_approve` still means full
/// access.
fn run_access(request: &RunRequest) -> AccessMode {
    if request.auto_approve {
        AccessMode::FullAccess
    } else {
        request.access
    }
}

/// The approval policy and sandbox a run's thread and turns get. Full access
/// is codex's `--dangerously-bypass-approvals-and-sandbox`, as every run was
/// before modes; auto-accept edits writes the workspace and, like
/// supervised, asks before anything but codex's known-safe reads (under
/// `"on-request"` a sandboxed `rm -rf` or `git push` would run unasked).
/// A title run never asks and never writes, whatever its chat's mode.
fn permissions(access: AccessMode, title_only: bool) -> (&'static str, SandboxLevel) {
    match access {
        _ if title_only => ("never", SandboxLevel::ReadOnly),
        AccessMode::FullAccess => ("never", SandboxLevel::DangerFullAccess),
        AccessMode::AutoAcceptEdits => ("untrusted", SandboxLevel::WorkspaceWrite),
        AccessMode::Supervised => ("untrusted", SandboxLevel::ReadOnly),
    }
}

/// `turn/start`'s `collaborationMode` (`plan` or `default`). Its settings
/// take precedence over the turn's model and effort, so they repeat them;
/// a null `developer_instructions` keeps the mode's built-in ones (0.164.0
/// schema).
fn collaboration_mode(plan: bool, model: &str, effort: Option<&str>) -> Value {
    json!({
        "mode": if plan { "plan" } else { "default" },
        "settings": {
            "model": model,
            "reasoning_effort": effort,
            "developer_instructions": null,
        },
    })
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

fn legacy_model_page(page: &Value) -> bool {
    page.get("data")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            !items.is_empty() && items.iter().all(|item| item.get("hidden").is_none())
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
        let description = match item
            .get("upgrade")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            Some(upgrade) => Some(format!(
                "{}(upgrade: {upgrade})",
                description.map(|d| format!("{d} ")).unwrap_or_default()
            )),
            None => description,
        };
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
    fn executable_path(&self) -> Option<PathBuf> {
        self.resolve_executable().ok()
    }
    /// Done is the CLI's own terminal frame, for wake turns too.
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    /// `turn/interrupt` ends the turn; the app-server and its thread — child
    /// agent threads included — stay up for the next `turn/start`.
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    /// Supervised asks before what codex does not know to be safe, and
    /// auto-accept edits before stepping outside the workspace sandbox.
    fn access_modes(&self) -> &[AccessMode] {
        &AccessMode::ALL
    }
    /// Codex's own `plan` collaboration mode.
    fn supports_plan(&self) -> bool {
        true
    }
    /// What the app-server launch and its turns read: the sandbox follows
    /// the access mode, and `serviceTier` is the only model option used.
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && live.model == next.model
            && to_effort(live.reasoning) == to_effort(next.reasoning)
            && service_tier(live) == service_tier(next)
            && run_access(live) == run_access(next)
            && live.plan == next.plan
    }
    /// `turn/start` carries model, effort, service tier, approval policy,
    /// sandbox and collaboration mode, and each sticks for later turns — so
    /// a change applies from the next turn without a new app-server.
    /// Clearing a model, effort or tier back to the default cannot be
    /// expressed that way (an omitted parameter keeps the previous value).
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && (next.model.is_some() || live.model.is_none())
            && (to_effort(next.reasoning).is_some() || to_effort(live.reasoning).is_none())
            && (service_tier(next).is_some() || service_tier(live).is_none())
    }

    /// The signed-in account's visible `model/list` is authoritative. A
    /// curated snapshot keeps the picker operational when the experimental
    /// discovery call is unavailable and no last-good catalog exists. Explicit
    /// picker refreshes bypass cooldowns while overlapping callers coalesce.
    fn model_context(&self) -> Result<Option<crate::ModelContext>, HarnessError> {
        crate::model_context::context(self.id(), &self.resolve_executable()?, &[]).map(Some)
    }
    fn fallback_models(&self) -> Vec<Model> {
        static_models()
    }
    async fn model_catalog(&self, force: bool) -> Result<crate::ModelCatalog, HarnessError> {
        self.model_context()?.unwrap().log();
        self.models_cache
            .get_with(
                force,
                || self.model_context().map(|c| c.unwrap().key()),
                || self.discover_models(),
            )
            .await
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        self.resolve_executable()?;
        match self.model_catalog(false).await {
            Ok(catalog) => Ok(catalog.models),
            Err(error) if !crate::CatalogFailure::classify(&error).allows_stale() => Err(error),
            Err(error) => {
                tracing::warn!(%error, source = "static", "Model discovery failed");
                Ok(self.fallback_models())
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
        self.run_with_mode(request, controls, false, false).await
    }

    async fn start_idle(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.run_with_mode(request, controls, false, true).await
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
        // Its policy never asks; anything that still would is the person's.
        request.auto_approve = false;
        request.access = AccessMode::Supervised;
        request.plan = false;
        self.run_with_mode(request, controls, true, false).await
    }
}

impl CodexHarness {
    async fn run_with_mode(
        &self,
        mut request: RunRequest,
        controls: RunControls,
        title_only: bool,
        idle: bool,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let native = command_request(&request.prompt, "")?;
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
        // Pin the physical release for this process and its voice helper: an
        // installer may move the current symlink while this runtime stays warm.
        let exe = plain_executable(
            self.resolve_executable()?
                .canonicalize()
                .map_err(HarnessError::Io)?,
        );
        // Cached per binary; a fixed test executable is never version-probed.
        if self.executable.is_none() {
            let probe = exe.clone();
            let version =
                tokio::task::spawn_blocking(move || crate::executable::binary_version(&probe))
                    .await
                    .ok()
                    .flatten();
            check_codex_version(version.as_ref())?;
        }
        // The sandbox is the access mode's (see [`permissions`]), never the
        // request's own level.
        request.sandbox = permissions(run_access(&request), title_only).1;
        let AppServer {
            child,
            client,
            incoming,
            stderr_tail,
        } = spawn_app_server(&exe, &request)?;
        let (event_tx, event_rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(256);
        tokio::spawn(run_session(Session {
            title_only,
            idle,
            executable: exe,
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

/// One `codex app-server` process and its wire.
struct AppServer {
    child: Child,
    client: RpcClient,
    incoming: mpsc::Receiver<Incoming>,
    stderr_tail: crate::StderrTail,
}

fn spawn_app_server(exe: &Path, request: &RunRequest) -> Result<AppServer, HarnessError> {
    let mut cmd = Command::new(exe);
    cmd.arg("app-server");
    crate::compose_child_path(&mut cmd, exe);
    // Host-resolved credentials (a Cloud device's managed CODEX_HOME,
    // GH_TOKEN, …). Absent on laptops: the CLI's own login is used.
    crate::apply_run_env(&mut cmd, &request.env);
    if !request.cwd.is_empty() {
        cmd.current_dir(&request.cwd);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| {
        let cwd = (!request.cwd.is_empty()).then(|| Path::new(&request.cwd));
        crate::executable::spawn_error(e, cwd, || crate::executable::binary_hint(exe))
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
            let mut lines = crate::lossy_lines::lossy_lines(stderr);
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "zeron_harness::codex", "stderr: {line}");
                tail.push(&line);
            }
            // The tail is complete: a crash report need not wait it out.
            tail.close();
        });
    }
    let (client, incoming) = RpcClient::new(stdin, stdout);
    Ok(AppServer {
        child,
        client,
        incoming,
        stderr_tail,
    })
}

/// Waits before relaunching an app-server that died before answering
/// `initialize`: a just-started machine can lose the first launch (a CLI
/// still finishing its install, a cold disk), and nothing has run yet.
const STARTUP_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(3),
    Duration::from_secs(8),
];

/// High-volume notifications nothing here consumes: the turn's running diff
/// and every command's / patch's output stream. Opting out at `initialize`
/// spares the wire (and the reader) from parsing them only to drop them.
const OPTED_OUT_NOTIFICATIONS: &[&str] = &[
    "turn/diff/updated",
    "item/commandExecution/outputDelta",
    "item/fileChange/outputDelta",
];

/// The oldest Codex CLI whose app-server speaks the wire this driver maps:
/// nested `error` notifications, `turn/steer`, child-thread routing (the
/// live captures under `tests/fixtures/codex` start at 0.146.1). An older
/// CLI fails confusingly mid-run instead of saying it needs an update.
const MIN_CODEX_VERSION: semver::Version = semver::Version::new(0, 146, 0);

/// `Err` when `version` is known and older than [`MIN_CODEX_VERSION`]; an
/// unknown version (a wrapper, a dev build) is given the benefit of the doubt.
/// A pre-release counts as its release: `0.146.0-alpha.2` already speaks the
/// wire, though semver orders it before `0.146.0`.
fn check_codex_version(version: Option<&semver::Version>) -> Result<(), HarnessError> {
    let release = version.map(|v| semver::Version::new(v.major, v.minor, v.patch));
    match (version, release) {
        (Some(version), Some(release)) if release < MIN_CODEX_VERSION => {
            Err(HarnessError::Protocol(format!(
                "Codex CLI {version} is too old; update it to {MIN_CODEX_VERSION} or newer \
                 (npm install -g @openai/codex@latest)"
            )))
        }
        _ => Ok(()),
    }
}

fn initialize_params(opt_out: bool) -> Value {
    let mut capabilities = json!({ "experimentalApi": true });
    if opt_out {
        capabilities["optOutNotificationMethods"] = json!(OPTED_OUT_NOTIFICATIONS);
    }
    json!({
        "clientInfo": {
            "name": "zeron-native",
            "title": "Zeron",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "capabilities": capabilities,
    })
}

/// A setup request's failure as display text. When the app-server died
/// under it, "exited before responding" says nothing: report its exit
/// status and last stderr lines instead.
async fn setup_failure(
    error: HarnessError,
    client: &RpcClient,
    child: &mut Child,
    stderr_tail: &crate::StderrTail,
) -> String {
    // Only a lost answer: a refusal that arrived before the exit says more.
    if !client.is_closed() || !transport_lost(&error) {
        return error.to_string();
    }
    crash_report(child, stderr_tail).await
}

/// The exited app-server's status and last stderr lines, once its stderr
/// has drained and it has been reaped (each briefly waited for).
async fn crash_report(child: &mut Child, stderr_tail: &crate::StderrTail) -> String {
    stderr_tail.wait_closed().await;
    let status = match tokio::time::timeout(Duration::from_millis(500), child.wait()).await {
        Ok(Ok(status)) => Some(status),
        _ => child.try_wait().ok().flatten(),
    };
    crate::crash_message("codex app-server", status, stderr_tail)
}

/// Why a `thread/resume` failure means the conversation is gone for good,
/// in words for the restore notice: no rollout on this machine, one Codex
/// can't read (empty, not UTF-8, unparseable — live, 0.164.0: "failed to
/// read thread: thread-store internal error: …"), a thread id Codex never
/// issued. A fresh thread is the only way forward then. `None` for anything
/// else — a dead process, a config error — which would fail a fresh start
/// too, or lose history needlessly. A bare "not found" is no such sign: an
/// I/O error ("entity not found"), a missing environment variable or helper
/// binary say it too.
fn resume_unrecoverable(error: &HarnessError) -> Option<&'static str> {
    let HarnessError::Protocol(message) = error else {
        return None;
    };
    if transport_lost(error) {
        return None;
    }
    let message = message.to_ascii_lowercase();
    let says = |needles: &[&str]| needles.iter().any(|needle| message.contains(needle));
    if says(&[
        "no rollout",
        "thread not found",
        "session not found",
        "conversation not found",
    ]) {
        Some("it isn't saved on this device")
    } else if says(&["failed to read thread", "rollout at"]) {
        // Only a history that is empty or unreadable as data is gone. One
        // the OS would not hand over (permissions, a lock, a busy disk) is
        // still there for the next message.
        if says(&[
            "os error",
            "permission denied",
            "locked",
            "busy",
            "too many open files",
            "timed out",
        ]) {
            None
        } else if message.contains("is empty") {
            Some("its saved history is empty")
        } else if says(&[
            "valid utf-8",
            "failed to parse",
            "parse error",
            "expected value",
            "eof while parsing",
            "invalid json",
        ]) {
            Some("its saved history couldn't be read")
        } else {
            None
        }
    } else if says(&["invalid session id", "invalid thread id"]) {
        Some("Codex doesn't recognize its id")
    } else {
        None
    }
}

/// `thread/resume`, unarchiving the thread first when Codex reports it
/// archived (the CLI's own archive command, or another client's).
async fn resume_thread(client: &RpcClient, params: Value) -> Result<Value, HarnessError> {
    match client.request("thread/resume", params.clone()).await {
        Err(HarnessError::Protocol(message)) if message.contains("is archived") => {
            tracing::info!(
                target: "zeron_harness::codex",
                "thread/resume: thread archived; unarchiving it"
            );
            client
                .request(
                    "thread/unarchive",
                    json!({ "threadId": params["threadId"] }),
                )
                .await?;
            client.request("thread/resume", params).await
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

struct Session {
    executable: PathBuf,
    title_only: bool,
    idle: bool,
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
    /// The app-server announced `active` (`turn/started`). Until then it may
    /// not have registered the turn: `turn/interrupt` answers "no active
    /// turn to interrupt" (live, 0.159.3, an interrupt right after the
    /// `turn/start` response) and the turn would run to its end.
    announced: bool,
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
        self.announced = true;
    }

    fn note_completed(&mut self, id: &str) {
        if id.is_empty() {
            return;
        }
        self.remember_completed(id.to_owned());
        if self.active.as_deref() == Some(id) {
            self.active = None;
            self.announced = false;
        }
    }

    /// Adopt a turn id from a `turn/start` RESPONSE (the notification is
    /// allowed to beat it). A turn already running keeps its id: a follow-up
    /// `turn/start` folds into it and answers with that same id anyway, and
    /// an announced turn is authoritative.
    fn adopt_started(&mut self, id: String) {
        if self.active.is_some() || id.is_empty() || self.is_completed(&id) {
            return;
        }
        self.active = Some(id);
        self.announced = false;
    }

    /// Whether a `turn/completed` (or failed/aborted) for `id` ends the turn
    /// in flight. A completion for a turn already settled, or for another
    /// turn while ours runs, is stale: settling on it marked the running
    /// turn done mid-way.
    fn settles(&self, id: &str) -> bool {
        id.is_empty() || (!self.is_completed(id) && self.active.as_deref().is_none_or(|a| a == id))
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

async fn start_turn(client: &RpcClient, params: Value) -> Result<String, HarnessError> {
    let text = params
        .pointer("/input/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let thread_id = params["threadId"].as_str().unwrap_or_default();
    let native = command_request(text, thread_id)?;
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

/// What `thread/start` / `thread/resume` reports the thread runs with: the
/// model and effort a collaboration mode falls back to when the request
/// names none, and whether a resumed thread is still in plan mode.
struct ThreadMode {
    model: String,
    effort: Option<String>,
    plan: bool,
}

/// The per-run event loop: one task multiplexing app-server messages, the
/// steering mailbox, the interrupt token, and consumer liveness.
async fn run_session(session: Session) {
    let Session {
        executable,
        title_only,
        idle,
        mut child,
        mut client,
        mut incoming,
        event_tx,
        controls,
        request,
        interrupt_grace,
        kill_grace,
        mut stderr_tail,
    } = session;
    let RunControls {
        realtime,
        execution_lease: _execution_lease,
        request_input,
        mut steering,
        interrupt,
        turn,
    } = controls;
    let request_input = Arc::new(request_input);

    // ---- wire params ------------------------------------------------------
    // The access mode's approval policy and sandbox (see [`permissions`]).
    // A mode that asks routes its approvals to the person explicitly: a
    // configured `approvals_reviewer` could otherwise hand them to an AI
    // reviewer instead.
    let (approval_policy, sandbox) = permissions(run_access(&request), title_only);
    let asks_person = |live: &RunRequest| !title_only && !run_access(live).is_full_access();
    let start_tier = service_tier(&request);

    let start_params = {
        let mut p = serde_json::Map::new();
        if title_only {
            p.insert("baseInstructions".into(), crate::TITLE_INSTRUCTIONS.into());
            p.insert(
                "developerInstructions".into(),
                crate::TITLE_INSTRUCTIONS.into(),
            );
            p.insert("ephemeral".into(), true.into());
            let mut config = json!({
                "project_doc_max_bytes": 0,
                "web_search": "disabled",
                "features.shell_tool": false,
                "features.apply_patch_freeform": false,
                "features.apps": false,
                "features.browser_use": false,
                "features.computer_use": false,
                "features.js_repl": false,
                "features.image_generation": false,
                "features.memories": false
            });
            config
                .as_object_mut()
                .expect("title config overrides are an object")
                .extend(agent_tools_off());
            p.insert("config".into(), config);
        }
        p.insert("cwd".into(), Value::String(request.cwd.clone()));
        if let Some(mcp) = request.mcp.as_ref().filter(|_| !title_only) {
            // Zeron's own MCP server as dotted config overrides on top of the
            // user's `mcp_servers` table (the same layer the title run uses
            // to switch servers off).
            let overrides = p
                .entry("config")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("thread/start config overrides are an object");
            overrides.extend(codex_mcp_overrides(mcp));
            // Delegation goes through that server's side chats only. The
            // engine doesn't ask this of Codex yet: see
            // `crate::switches_native_subagents`.
            if mcp.side_chats_only() {
                overrides.extend(agent_tools_off());
            }
        }
        p.insert("approvalPolicy".into(), approval_policy.into());
        p.insert("sandbox".into(), sandbox_mode(sandbox).into());
        if asks_person(&request) {
            p.insert("approvalsReviewer".into(), "user".into());
        }
        if let Some(model) = &request.model {
            p.insert("model".into(), Value::String(model.clone()));
        }
        if let Some(tier) = &start_tier {
            p.insert("serviceTier".into(), Value::String(tier.clone()));
        }
        p
    };

    // ---- handshake (interruptible; relaunches a process that died first) ---
    let mut attempt = 0;
    let mut opt_out = true;
    loop {
        let handshake = client.request("initialize", initialize_params(opt_out));
        let failed = tokio::select! {
            res = handshake => match res {
                Ok(_) => break,
                Err(e) => e,
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
                client.close_stdin_for_teardown();
                shutdown_child(&mut child, kill_grace).await;
                return;
            }
        };
        // Only a process that is gone gets another launch: an answer that
        // refuses (bad config, unsupported version) would refuse again.
        let exited = client.is_closed();
        // An app-server that predates notification opt-outs may reject the
        // capability outright; the opt-out is an optimization, so ask again
        // without it.
        if !exited && std::mem::take(&mut opt_out) {
            tracing::debug!(
                target: "zeron_harness::codex",
                "initialize refused ({failed}); retrying without notification opt-outs"
            );
            continue;
        }
        if exited && attempt < STARTUP_RETRY_DELAYS.len() {
            let status = child.try_wait().ok().flatten();
            tracing::warn!(
                target: "zeron_harness::codex",
                attempt = attempt + 1,
                error = %crate::crash_message("codex app-server", status, &stderr_tail),
                "app-server died before initialize; relaunching"
            );
            client.close_stdin_for_teardown();
            shutdown_child(&mut child, kill_grace).await;
            tokio::select! {
                _ = tokio::time::sleep(STARTUP_RETRY_DELAYS[attempt]) => {}
                _ = interrupt.cancelled() => {
                    let _ = event_tx
                        .send(Ok(AgentEvent::Done {
                            status: DoneStatus::Interrupted,
                            result: None,
                            error: None,
                            session_id: None,
                        }))
                        .await;
                    return;
                }
            }
            attempt += 1;
            match spawn_app_server(&executable, &request) {
                Ok(next) => {
                    child = next.child;
                    client = next.client;
                    incoming = next.incoming;
                    stderr_tail = next.stderr_tail;
                    continue;
                }
                Err(e) => {
                    let _ = event_tx
                        .send(Ok(AgentEvent::Done {
                            status: DoneStatus::Errored,
                            result: None,
                            error: Some(e.to_string()),
                            session_id: None,
                        }))
                        .await;
                    return;
                }
            }
        }
        // Out of launches: say why, with what the process printed.
        let error = if exited {
            stderr_tail.wait_closed().await;
            format!(
                "initialize: {}",
                crate::crash_message(
                    "codex app-server",
                    child.try_wait().ok().flatten(),
                    &stderr_tail
                )
            )
        } else {
            failed.to_string()
        };
        let _ = event_tx
            .send(Ok(AgentEvent::Done {
                status: DoneStatus::Errored,
                result: None,
                error: Some(error),
                session_id: None,
            }))
            .await;
        client.close_stdin_for_teardown();
        shutdown_child(&mut child, kill_grace).await;
        return;
    }
    client.notify("initialized", None);

    // ---- thread + first turn (interruptible) ------------------------------
    let setup = async {
        if realtime.is_some() {
            // Codex's first account/read announces the initial auth snapshot
            // via account/updated. Load it before attaching the voice router,
            // which must keep aborting media on subsequent identity updates.
            // A failed warmup must not block text; voice probes still verify auth.
            let _ = client
                .request("account/read", json!({"refreshToken": false}))
                .await;
        }

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
        let mut restore_notice = None;
        let thread = if let Some(resume) = &request.resume {
            let mut p = start_params.clone();
            p.insert("threadId".into(), Value::String(resume.clone()));
            match resume_thread(&client, Value::Object(p))
                .await
                .map_err(|e| (resume_unrecoverable(&e), e))
            {
                Ok(thread) => thread,
                // A missing/unreadable/foreign rollout falls back to a fresh
                // thread — visibly: the reply will not know the earlier
                // conversation. The log keeps Codex's own words.
                Err((Some(reason), e)) => {
                    if command_request(&request.prompt, resume)?.is_some() {
                        return Err(e);
                    }
                    tracing::warn!(
                        target: "zeron_harness::codex",
                        "thread/resume failed (starting fresh): {e}"
                    );
                    restore_notice = Some(format!(
                        "Couldn't restore the earlier Codex conversation ({reason}), so this \
                         reply starts a new one without its context."
                    ));
                    client
                        .request("thread/start", Value::Object(start_params.clone()))
                        .await?
                }
                // A dead process or a refusal a fresh thread would hit too:
                // fail the run rather than silently dropping its history.
                Err((None, e)) => return Err(e),
            }
        } else {
            client
                .request("thread/start", Value::Object(start_params.clone()))
                .await?
        };
        let thread_id = thread["thread"]["id"].as_str().unwrap_or("").to_owned();
        let mut children = subagents::Subagents::new(thread_id.clone());
        children.restore(&thread["thread"]);
        let voice_context = realtime::ThreadContext {
            cwd: thread["cwd"].as_str().unwrap_or(&request.cwd).to_owned(),
            model_provider: thread["modelProvider"].as_str().map(str::to_owned),
        };
        let thread_mode = ThreadMode {
            model: thread["model"].as_str().unwrap_or_default().to_owned(),
            effort: thread["reasoningEffort"].as_str().map(str::to_owned),
            plan: thread.pointer("/collaborationMode/mode") == Some(&json!("plan")),
        };
        Ok::<_, HarnessError>((thread_id, children, voice_context, restore_notice, thread_mode))
    };
    let (thread_id, mut children, voice_context, restore_notice, thread_mode) = tokio::select! {
        res = setup => match res {
            Ok(thread_id) => thread_id,
            Err(e) => {
                let error = setup_failure(e, &client, &mut child, &stderr_tail).await;
                let _ = event_tx
                    .send(Ok(AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some(error),
                        session_id: None,
                    }))
                    .await;
                client.close_stdin_for_teardown();
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
            client.close_stdin_for_teardown();
            shutdown_child(&mut child, kill_grace).await;
            return;
        }
    };

    // Model, effort, service tier, access and plan ride every `turn/start`
    // (each sticks for later turns): a steer's in-place `config` takes
    // effect from the next turn the runtime starts.
    let mut live = request.clone();
    // Plan mode sticks to the thread, across a resume too (live, 0.164.0).
    // Once this runtime has asked for it, or the resumed thread is in it,
    // every turn names its mode so that leaving plan is explicit; a thread
    // that never planned is sent none, as before modes.
    let mut names_mode = thread_mode.plan;
    let mut turn_params = |live: &RunRequest, text: &str| -> Value {
        let mut p = serde_json::Map::new();
        p.insert("threadId".into(), Value::String(thread_id.clone()));
        p.insert("input".into(), prompt_input(text));
        let (approval_policy, sandbox) = permissions(run_access(live), title_only);
        p.insert("approvalPolicy".into(), approval_policy.into());
        p.insert("sandboxPolicy".into(), sandbox_policy_value(sandbox));
        if asks_person(live) {
            p.insert("approvalsReviewer".into(), "user".into());
        }
        // Reasoning summaries stream (`item/reasoning/summaryTextDelta`) only
        // when asked for — without this codex "thinks" in silence for minutes:
        // nothing renders and the UI's 45s staleness gate flips Working off
        // (user report: "not streaming, doesn't say it's working").
        p.insert("summary".into(), "auto".into());
        if let Some(model) = &live.model {
            p.insert("model".into(), Value::String(model.clone()));
        }
        if let Some(effort) = to_effort(live.reasoning) {
            p.insert("effort".into(), effort.into());
        }
        if let Some(tier) = service_tier(live) {
            p.insert("serviceTier".into(), Value::String(tier));
        }
        names_mode |= live.plan;
        if names_mode {
            let model = live.model.as_deref().unwrap_or(&thread_mode.model);
            let effort = to_effort(live.reasoning).or(thread_mode.effort.as_deref());
            if model.is_empty() {
                tracing::warn!(
                    target: "zeron_harness::codex",
                    "no model to name the collaboration mode with; leaving it unchanged"
                );
            } else {
                p.insert(
                    "collaborationMode".into(),
                    collaboration_mode(live.plan, model, effort),
                );
            }
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
        client.close_stdin_for_teardown();
        shutdown_child(&mut child, kill_grace).await;
        return;
    }
    if let Some(message) = restore_notice
        && !send(&event_tx, AgentEvent::Error { message }).await
    {
        client.close_stdin_for_teardown();
        shutdown_child(&mut child, kill_grace).await;
        return;
    }

    let mut router = TurnRouter::default();
    let _voice_bridge = realtime.map(|controls| {
        realtime::attach(
            client.clone(),
            thread_id.clone(),
            executable,
            voice_context,
            controls,
        )
    });
    if !idle {
        match start_turn(&client, turn_params(&live, &request.prompt)).await {
            Ok(id) => router.adopt_started(id),
            Err(e) => {
                let error = setup_failure(e, &client, &mut child, &stderr_tail).await;
                let _ = event_tx
                    .send(Ok(AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some(error),
                        session_id: Some(thread_id.clone()),
                    }))
                    .await;
                client.close_stdin_for_teardown();
                shutdown_child(&mut child, kill_grace).await;
                return;
            }
        }
    }

    // ---- main loop --------------------------------------------------------
    // Deltas seen per agent-message item, so a model that never streams
    // (item/completed only) still emits its text exactly once.
    let mut streamed_text: HashSet<String> = HashSet::new();
    let mut reasoning_streams: HashMap<String, ReasoningStream> = HashMap::new();
    // Token usage is held until the turn ends, emitted just before Done.
    let mut pending_usage: Option<AgentEvent> = None;
    // The turn's last final `error` notification, reported by its end (see
    // [`settle_turn_error`]): never as an error part AND the Done's error.
    let mut held_error: Option<String> = None;
    // The account's rate-limit windows (`account/rateLimits/updated`), for
    // a limit error's reset time.
    let mut rate_limits: Option<Value> = None;
    // Server→client requests answered from their own task (approvals and
    // questions waiting on a person), bounded.
    let request_slots = Arc::new(tokio::sync::Semaphore::new(MAX_PENDING_SERVER_REQUESTS));
    // An interrupted turn's background-terminal query in flight: the turn
    // and the process ids Codex still runs (`None`: it could not say).
    let mut prune_query: Option<tokio::sync::oneshot::Receiver<(String, Option<HashSet<String>>)>> =
        None;
    // Steers whose `turn/steer` lost the turn-completed race; delivered as the
    // next `turn/start` when the expected turn's end notification arrives.
    // Each carries the configuration it was sent with, adopted when its own
    // `turn/start` goes out.
    let mut queued_steers: VecDeque<(String, Option<Box<RunRequest>>)> = VecDeque::new();
    // Steers the app-server accepted into the running turn, by the
    // `clientUserMessageId` they were sent with. Each is confirmed
    // (`Steered`) when its userMessage item joins the turn — not when
    // `turn/steer` returns, which split the reply still streaming ahead of
    // it into the steer's segment.
    let mut accepted_steers: VecDeque<String> = VecDeque::new();
    // A turn stop waiting for the app-server to announce the turn.
    let mut stop_when_announced = false;
    // Commands the parent thread started and the app-server has not reported
    // finished, with their process id and the turn that started them. One
    // still open after its turn is background work (a dev server, a
    // watcher): its `item/completed` arrives when the process exits, turns
    // later (live, 0.159.3). The host's idle reaper must not retire the
    // app-server — and the process with it — while it runs.
    let mut open_commands: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    // An interrupted turn's foreground command is killed WITHOUT an
    // `item/completed`: once its process has had a moment to go, drop the
    // turn's commands whose process is gone (background ones live on).
    let mut prune_at: Option<(tokio::time::Instant, String)> = None;
    let mut steering_open = true;
    let mut interrupted = false;
    let mut interrupt_sent = false;
    // A turn stop (`TurnControl::stop_turn`) in flight: the turn's end reads
    // as Interrupted and the app-server stays up for the next turn.
    let mut stopping = false;
    // A Done has been emitted for the turn currently/last in flight.
    let mut done_current = idle;
    let mut current_native = command_request(&request.prompt, &thread_id)
        .ok()
        .flatten()
        .is_some();
    let mut done_after_interrupt = false;
    let mut escalation: Option<tokio::task::JoinHandle<()>> = None;
    // The runtime's process tree when its teardown began (see
    // [`crate::shutdown_agent`]).
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut torn_down: Vec<crate::process::Tracked> = Vec::new();

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
                    "turn/started" => {
                        let id = turn_id(&params);
                        // Native voice handoffs start a turn without going through turn/start.
                        // Publish the boundary before its text/tool deltas reach the parked engine.
                        if done_current && !id.is_empty() && !router.is_completed(&id) {
                            done_current = false;
                            let (prev, next) = rotate(&mut assistant_message_id);
                            if !send(&event_tx, AgentEvent::Steered {
                                assistant_message_id: Some(prev),
                                next_assistant_message_id: Some(next),
                            }).await {
                                break 'main;
                            }
                        }
                        router.note_started(id);
                        if std::mem::take(&mut stop_when_announced)
                            && let Some(active) = router.active.clone()
                        {
                            send_turn_interrupt(&client, &thread_id, active);
                        }
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

                    "item/started" | "item/completed" => {
                        let phase = if method == "item/started" {
                            Phase::Started
                        } else {
                            Phase::Completed
                        };
                        let item = params.get("item").unwrap_or(&Value::Null);
                        if matches!(item_type(item), "commandExecution" | "command_execution") {
                            let id = item.get("id").and_then(Value::as_str).unwrap_or_default();
                            let changed = if phase == Phase::Started {
                                let pid = item.get("processId").and_then(Value::as_str).map(str::to_owned);
                                open_commands.insert(id.to_owned(), (pid, router.active.clone())).is_none()
                            } else {
                                open_commands.remove(id).is_some()
                            };
                            if changed {
                                turn.set_background(open_commands.len());
                            }
                        }
                        if matches!(item_type(item), "userMessage" | "user_message")
                            && let Some(at) = item
                                .get("clientId")
                                .and_then(Value::as_str)
                                .and_then(|id| accepted_steers.iter().position(|s| s == id))
                        {
                            for _ in 0..=at {
                                accepted_steers.pop_front();
                                let (prev, next) = rotate(&mut assistant_message_id);
                                if !send(&event_tx, AgentEvent::Steered {
                                    assistant_message_id: Some(prev),
                                    next_assistant_message_id: Some(next),
                                }).await {
                                    break 'main;
                                }
                            }
                        }
                        if phase == Phase::Completed {
                            let output = match item_type(item) {
                                "exitedReviewMode" => {
                                    item.get("review").and_then(Value::as_str).map(str::to_owned)
                                }
                                "contextCompaction" => Some("Context compacted.".to_owned()),
                                // Plan mode's proposed plan: Codex lifts it
                                // out of the agent message into its own item
                                // (the completed one is authoritative, its
                                // `item/plan/delta`s may not add up to it).
                                "plan" => item.get("text").and_then(Value::as_str)
                                    .filter(|text| !text.trim().is_empty())
                                    .map(|text| format!("{}\n\n", text.trim_end())),
                                _ => None,
                            };
                            if let Some(text) = output
                                && !send(&event_tx, AgentEvent::TextDelta { text }).await
                            { break 'main; }
                        }
                        if matches!(item_type(item), "agentMessage" | "agent_message") {
                            if phase == Phase::Completed {
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

                    "thread/tokenUsage/updated" => {
                        if let Some(usage) = normalize::context_usage_event(&params)
                            && !send(&event_tx, usage).await { break 'main; }
                        if let Some(usage) = usage_event(&params) {
                            pending_usage = Some(usage);
                        }
                    }

                    // Codex's `update_plan` tool: the whole checklist, replaced
                    // on every call, with an in-progress step.
                    "turn/plan/updated" => {
                        for ev in normalize::plan_update_events(&params) {
                            if !send(&event_tx, ev).await { break 'main; }
                        }
                    }

                    "turn/completed" => {
                        let id = turn_id(&params);
                        if !router.settles(&id) {
                            tracing::debug!(
                                target: "zeron_harness::codex",
                                turn = %id,
                                "stale turn/completed ignored (not the turn in flight)"
                            );
                            router.note_completed(&id);
                            continue 'main;
                        }
                        router.note_completed(&id);
                        stop_when_announced = false;
                        // Accepted steers whose item never surfaced still
                        // joined this turn: confirm them before its end.
                        while accepted_steers.pop_front().is_some() {
                            let (prev, next) = rotate(&mut assistant_message_id);
                            if !send(&event_tx, AgentEvent::Steered {
                                assistant_message_id: Some(prev),
                                next_assistant_message_id: Some(next),
                            }).await {
                                break 'main;
                            }
                        }
                        // Item ids never span turns; without this the set grew
                        // one entry per message for a persistent session's life.
                        streamed_text.clear();
                        if let Some(usage) = pending_usage.take()
                            && !send(&event_tx, usage).await
                        {
                            break 'main;
                        }
                        let (error, notice) =
                            settle_turn_error(&params, held_error.take(), rate_limits.as_ref());
                        let status = if interrupted || stopping {
                            DoneStatus::Interrupted
                        } else if error.is_some() {
                            DoneStatus::Errored
                        } else if params.pointer("/turn/status").and_then(Value::as_str)
                            == Some("interrupted")
                        {
                            // Interrupted by someone else (another client of the
                            // thread, Codex itself): still not a finished answer.
                            DoneStatus::Interrupted
                        } else {
                            DoneStatus::Completed
                        };
                        if status == DoneStatus::Interrupted {
                            prune_at = Some((tokio::time::Instant::now() + COMMAND_EXIT_GRACE, id.clone()));
                        }
                        if let Some(message) = notice.filter(|_| status == DoneStatus::Completed)
                            && !send(&event_tx, AgentEvent::Error { message }).await
                        {
                            break 'main;
                        }
                        stopping = false;
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
                        if let Some((text, config)) = queued_steers.pop_front() {
                            if let Some(next) = config {
                                live = *next;
                            }
                            current_native = command_request(&text, &thread_id).ok().flatten().is_some();
                            if !steer_as_new_turn(
                                &client,
                                turn_params(&live, &text),
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
                        let id = turn_id(&params);
                        if !router.settles(&id) {
                            router.note_completed(&id);
                            continue 'main;
                        }
                        router.note_completed(&id);
                        stop_when_announced = false;
                        accepted_steers.clear();
                        let held = held_error.take();
                        if let Some(usage) = pending_usage.take()
                            && !send(&event_tx, usage).await
                        {
                            break 'main;
                        }
                        done_current = true;
                        if std::mem::take(&mut stopping) && !interrupted {
                            // The stopped turn's end: the thread lives on.
                            if !send(&event_tx, AgentEvent::Done {
                                status: DoneStatus::Interrupted,
                                result: None,
                                error: None,
                                session_id: Some(thread_id.clone()),
                            }).await {
                                break 'main;
                            }
                            continue 'main;
                        }
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
                                    turn_error_message(&params, rate_limits.as_ref())
                                        .or(held)
                                        .unwrap_or_else(|| "Codex turn failed".into()),
                                ),
                                session_id: Some(thread_id.clone()),
                            },
                        )
                        .await;
                        break 'main;
                    }

                    "turn/aborted" => {
                        let id = turn_id(&params);
                        if !router.settles(&id) {
                            router.note_completed(&id);
                            continue 'main;
                        }
                        router.note_completed(&id);
                        stop_when_announced = false;
                        accepted_steers.clear();
                        held_error = None;
                        done_current = true;
                        prune_at = Some((tokio::time::Instant::now() + COMMAND_EXIT_GRACE, id.clone()));
                        if std::mem::take(&mut stopping) && !interrupted {
                            if !send(&event_tx, AgentEvent::Done {
                                status: DoneStatus::Interrupted,
                                result: None,
                                error: None,
                                session_id: Some(thread_id.clone()),
                            }).await {
                                break 'main;
                            }
                            continue 'main;
                        }
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
                        let notice = normalize::error_notice(&params, rate_limits.as_ref());
                        if notice.will_retry {
                            // "Reconnecting… 2/5": Codex is on it. Not an
                            // outcome, so not a permanent error in the
                            // transcript (nor a sign-in prompt).
                            tracing::info!(
                                target: "zeron_harness::codex",
                                "codex retrying: {}", notice.message
                            );
                        } else if !done_current {
                            // The turn's end reports it (turn/completed
                            // usually repeats it as `turn.error`).
                            held_error = Some(notice.message);
                        } else if !send(&event_tx, AgentEvent::Error { message: notice.message }).await {
                            break 'main;
                        }
                    }

                    "account/rateLimits/updated" => {
                        normalize::merge_rate_limits(&mut rate_limits, &params);
                    }

                    // thread/status, mcpServer startup, account noise, … —
                    // unknown notification methods are tolerated by design.
                    _ => {}
                }
                }

                Some(Incoming::Request { id, method, params }) => {
                    handle_server_request(
                        &client,
                        id,
                        &method,
                        &params,
                        run_access(&live),
                        &request_input,
                        &request_slots,
                    );
                }

                // stdout EOF or reader gone: the app server exited.
                Some(Incoming::Eof) | None => break 'main,
            },

            steer = steering.recv(), if steering_open && !interrupted => match steer {
                Some(msg) => {
                    let text = msg.prompt;
                    // Native operations run at a turn boundary, never as text
                    // injected into an already running model turn. So does a
                    // changed configuration: `turn/steer` carries no model,
                    // effort or tier, and only a `turn/start` applies them.
                    // Later messages must stay behind queued ones: Steered
                    // acknowledgments retire the engine's accepted-message
                    // ledger in FIFO order.
                    if !done_current
                        && (!queued_steers.is_empty()
                            || current_native
                            || msg.config.is_some()
                            || !matches!(command_request(&text, &thread_id), Ok(None)))
                    {
                        queued_steers.push_back((text, msg.config));
                        continue 'main;
                    }
                    if let Some(next) = msg.config {
                        live = *next;
                    }
                    if let Some(expected) = router.active.clone() {
                        let client_id = new_message_id();
                        let steer_params = json!({
                            "threadId": thread_id,
                            "expectedTurnId": expected,
                            "clientUserMessageId": client_id,
                            "input": prompt_input(&text),
                        });
                        match client.request("turn/steer", steer_params).await {
                            Ok(_) => accepted_steers.push_back(client_id),
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
                                    queued_steers.push_back((text, None));
                                } else {
                                    current_native = command_request(&text, &thread_id).ok().flatten().is_some();
                                    if !steer_as_new_turn(
                                        &client, turn_params(&live, &text), &mut router, &event_tx,
                                        &mut assistant_message_id, &mut done_current,
                                    ).await { break 'main; }
                                }
                            }
                        }
                    } else {
                        current_native = command_request(&text, &thread_id).ok().flatten().is_some();
                        if !steer_as_new_turn(
                            &client, turn_params(&live, &text), &mut router, &event_tx,
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

            // End the in-flight turn, not the app-server: its threads (child
            // agents included) keep running. Steers still waiting for this
            // turn's end go with it — a stopped turn never continues.
            _ = turn.stop_requested(), if !interrupted => {
                queued_steers.clear();
                match router.active.clone() {
                    Some(active) if !done_current => {
                        stopping = true;
                        if router.announced {
                            send_turn_interrupt(&client, &thread_id, active);
                        } else {
                            stop_when_announced = true;
                        }
                    }
                    // Between turns: nothing to interrupt. Settle the stop so
                    // the host sees the turn over.
                    _ => {
                        done_current = true;
                        if !send(&event_tx, AgentEvent::Done {
                            status: DoneStatus::Interrupted,
                            result: None,
                            error: None,
                            session_id: Some(thread_id.clone()),
                        }).await {
                            break 'main;
                        }
                    }
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

            _ = tokio::time::sleep_until(
                prune_at.as_ref().map_or_else(tokio::time::Instant::now, |(at, _)| *at)
            ), if prune_at.is_some() => {
                let (_, stopped) = prune_at.take().expect("guarded by if");
                // Ask from a task: the answer may take a moment, and the turn
                // stream keeps flowing meanwhile.
                let (tx, rx) = tokio::sync::oneshot::channel();
                let client = client.clone();
                let thread = thread_id.clone();
                tokio::spawn(async move {
                    let _ = tx.send((stopped, background_terminals(&client, &thread).await));
                });
                prune_query = Some(rx);
            },

            answer = async { prune_query.as_mut().expect("guarded by if").await }, if prune_query.is_some() => {
                prune_query = None;
                if let Ok((stopped, live)) = answer {
                    open_commands.retain(|item, (pid, started_in)| {
                        started_in.as_deref() != Some(stopped.as_str())
                            || match &live {
                                Some(live) => {
                                    live.contains(item.as_str())
                                        || pid.as_deref().is_some_and(|pid| live.contains(pid))
                                }
                                None => pid.as_deref().is_some_and(process_alive),
                            }
                    });
                    turn.set_background(open_commands.len());
                }
            },

            _ = event_tx.closed() => break 'main,
        }
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
            // handling, reduced to the turn-in-flight case. A final error
            // the turn already reported (and its end never came to carry)
            // leads: it is usually why the turn went nowhere.
            let crash = crash_report(&mut child, &stderr_tail).await;
            let error = match held_error.take() {
                Some(held) => format!("{held}\n\n{crash}"),
                None => crash,
            };
            let _ = event_tx
                .send(Ok(AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: Some(error),
                    session_id: Some(thread_id.clone()),
                }))
                .await;
        }
    }

    client.close_stdin_for_teardown();
    crate::shutdown_agent(&mut child, torn_down, kill_grace).await;
    if let Some(handle) = escalation {
        handle.abort();
    }
}

/// How long an interrupted turn's commands get to exit before a process
/// still running counts as background work.
const COMMAND_EXIT_GRACE: Duration = Duration::from_secs(3);

/// Server→client requests answered from their own task at once. A flood
/// beyond this (a misbehaving MCP server) is refused instead of growing
/// without bound.
const MAX_PENDING_SERVER_REQUESTS: usize = 32;

/// The `processId`s and item ids of the commands Codex still runs for
/// `thread` (`thread/backgroundTerminals/list`, paged). The authority on a
/// command item's `processId`, which names Codex's own exec session rather
/// than promising an OS pid. `None` when the app-server can't answer (an
/// older build without the method).
async fn background_terminals(client: &RpcClient, thread: &str) -> Option<HashSet<String>> {
    let mut live = HashSet::new();
    let mut cursor: Option<String> = None;
    for _ in 0..100 {
        let mut params = json!({ "threadId": thread });
        if let Some(cursor) = &cursor {
            params["cursor"] = cursor.clone().into();
        }
        let page = match tokio::time::timeout(
            Duration::from_secs(5),
            client.request("thread/backgroundTerminals/list", params),
        )
        .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(e)) => {
                tracing::debug!(target: "zeron_harness::codex", "background terminals unavailable: {e}");
                return None;
            }
            Err(_) => return None,
        };
        for terminal in page["data"].as_array().into_iter().flatten() {
            for key in ["processId", "itemId"] {
                if let Some(id) = terminal[key].as_str().filter(|id| !id.is_empty()) {
                    live.insert(id.to_owned());
                }
            }
        }
        match page["nextCursor"].as_str().filter(|next| !next.is_empty()) {
            Some(next) if cursor.as_deref() != Some(next) => cursor = Some(next.to_owned()),
            _ => return Some(live),
        }
    }
    Some(live)
}

/// Fallback for an app-server that can't list its terminals: whether the
/// command's reported process id names a running OS process.
fn process_alive(pid: &str) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only probes for existence; nothing is delivered.
        pid.parse::<i32>()
            .is_ok_and(|pid| pid > 0 && unsafe { libc::kill(pid, 0) } == 0)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let Ok(pid) = pid.parse::<u32>() else {
            return false;
        };
        if pid == 0 {
            return false;
        }
        // SAFETY: a query-only handle, closed before returning; a vanished
        // pid yields a null handle.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let alive = GetExitCodeProcess(handle, &mut code) != 0 && code == STILL_ACTIVE as u32;
            CloseHandle(handle);
            alive
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Ask the app-server to end `turn` (fire-and-forget: its end arrives as the
/// turn's own completion notification).
fn send_turn_interrupt(client: &RpcClient, thread_id: &str, turn: String) {
    let client = client.clone();
    let thread = thread_id.to_owned();
    tokio::spawn(async move {
        if let Err(e) = client
            .request(
                "turn/interrupt",
                json!({ "threadId": thread, "turnId": turn }),
            )
            .await
        {
            tracing::warn!(target: "zeron_harness::codex", "turn/interrupt failed: {e}");
        }
    });
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

/// What an approval request asks to do, for
/// [`crate::approval::allowed_without_asking`].
fn approval_action(method: &str) -> crate::approval::Action {
    if method == "item/fileChange/requestApproval" {
        crate::approval::Action::Edit
    } else {
        crate::approval::Action::Other
    }
}

/// Serve one server→client request. An approval the run's `access` covers
/// is accepted outright (full access: belt to the wire-level `"never"`;
/// auto-accept edits: file changes); any other round-trips through
/// `request_input` as a yes/no question (in a subtask so the message loop
/// keeps flowing), a refusal declining. MCP elicitations are asked or
/// declined. Anything else is rejected as unsupported so the server never
/// wedges awaiting a reply. At most [`MAX_PENDING_SERVER_REQUESTS`] wait on
/// a person at once; the rest are refused as overloaded.
fn handle_server_request(
    client: &RpcClient,
    id: Value,
    method: &str,
    params: &Value,
    access: AccessMode,
    request_input: &Arc<RequestInputFn>,
    slots: &Arc<tokio::sync::Semaphore>,
) {
    let slot = || {
        let permit = Arc::clone(slots).try_acquire_owned().ok();
        if permit.is_none() {
            tracing::warn!(
                target: "zeron_harness::codex",
                "too many pending server requests; refusing {method}"
            );
            client.respond_error(&id, -32000, "client overloaded: too many pending requests");
        }
        permit
    };
    // A tool's user-input request (EXPERIMENTAL, codex 0.146.x) is a CONTENT
    // question, never auto-approvable — route it to the input bridge and
    // answer keyed by question id, `{ answers: { <id>: { answers: [..] } } }`.
    if method == "item/tool/requestUserInput" {
        let questions = user_input_questions(params);
        if questions.is_empty() {
            client.respond(&id, json!({ "answers": {} }));
            return;
        }
        let Some(permit) = slot() else { return };
        let client = client.clone();
        let request_input = Arc::clone(request_input);
        tokio::spawn(async move {
            let _permit = permit;
            let asked: Vec<UserInputQuestion> = questions.iter().map(|(_, q)| q.clone()).collect();
            let answers = (request_input)(asked).await.unwrap_or_default();
            let mut by_id = serde_json::Map::new();
            for (wire_id, q) in &questions {
                let labels: Vec<Value> = answers
                    .iter()
                    .find(|a| a.question_id == q.id)
                    .map(|a| a.labels.iter().cloned().map(Value::String).collect())
                    .unwrap_or_default();
                by_id.insert(wire_id.clone(), json!({ "answers": labels }));
            }
            client.respond(&id, json!({ "answers": by_id }));
        });
        return;
    }
    // MCP servers elicit regardless of the approval policy. A form the input
    // bridge can ask is asked; anything else is declined — a reply the
    // server handles, where an error failed its tool call.
    if method == "mcpServer/elicitation/request" {
        let Some(form) = elicitation::form(params) else {
            let mode = params["mode"].as_str().unwrap_or_default();
            tracing::info!(
                target: "zeron_harness::codex",
                mode, "declining an MCP elicitation Zeron can't ask"
            );
            client.respond(&id, elicitation::decline());
            return;
        };
        let Some(permit) = slot() else { return };
        let client = client.clone();
        let request_input = Arc::clone(request_input);
        tokio::spawn(async move {
            let _permit = permit;
            let answers = (request_input)(form.questions.clone())
                .await
                .unwrap_or_default();
            client.respond(&id, form.response(&answers));
        });
        return;
    }
    // Each approval's own accept / decline reply.
    let reply: fn(&Value, bool) -> Value = match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            |_, accept| json!({ "decision": if accept { "accept" } else { "decline" } })
        }
        // Granting means granting what was asked, for this turn; an empty
        // grant is the refusal.
        "item/permissions/requestApproval" => |params, accept| {
            json!({
                "permissions": if accept { params["permissions"].clone() } else { json!({}) },
                "scope": "turn",
            })
        },
        _ => {
            tracing::debug!(
                target: "zeron_harness::codex",
                "unhandled server request: {method}"
            );
            client.respond_error(&id, -32601, &format!("unsupported method: {method}"));
            return;
        }
    };
    if crate::approval::allowed_without_asking(access, approval_action(method)) {
        client.respond(&id, reply(params, true));
        return;
    }

    let Some(permit) = slot() else { return };
    let question = approval_question(method, params);
    let params = params.clone();
    let client = client.clone();
    let request_input = Arc::clone(request_input);
    tokio::spawn(async move {
        let _permit = permit;
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
        let accept = crate::approval::approved(&question, &answers);
        client.respond(&id, reply(&params, accept));
    });
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
        .enumerate()
        .map(|(ix, q)| {
            let field = |keys: [&str; 3]| {
                keys.iter()
                    .find_map(|k| q.get(*k).and_then(Value::as_str))
                    .unwrap_or("")
                    .to_owned()
            };
            let wire_id = {
                let id = field(["id", "questionId", "question_id"]);
                if id.is_empty() { format!("q{ix}") } else { id }
            };
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
                prefill: None,
                multiline: false,
                multi_select: ["multiSelect", "multi_select"]
                    .iter()
                    .find_map(|k| q.get(*k).and_then(Value::as_bool))
                    .unwrap_or(false),
            };
            (wire_id, question)
        })
        .collect()
}

/// Synthesize the yes/no question an approval request surfaces to the user
/// (the shared [`crate::approval`] wording; permissions spell out what
/// they grant).
fn approval_question(method: &str, params: &Value) -> UserInputQuestion {
    if method.contains("commandExecution") {
        let command = match params.get("command") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        crate::approval::command("Codex", &command)
    } else if method.contains("permissions") {
        let permissions = &params["permissions"];
        let mut asks = Vec::new();
        if permissions.pointer("/network/enabled") == Some(&Value::Bool(true)) {
            asks.push("network access".to_owned());
        }
        let fs = &permissions["fileSystem"];
        for (key, access) in [("write", "write"), ("read", "read")] {
            let paths: Vec<&str> = fs[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if !paths.is_empty() {
                asks.push(format!("{access} access to {}", paths.join(", ")));
            }
        }
        for entry in fs["entries"].as_array().into_iter().flatten() {
            if let (Some(access), Some(path)) = (
                entry["access"].as_str().filter(|a| *a != "deny"),
                entry.pointer("/path/path").and_then(Value::as_str),
            ) {
                asks.push(format!("{access} access to {path}"));
            }
        }
        let what = if asks.is_empty() {
            "additional permissions".to_owned()
        } else {
            asks.join("; ")
        };
        let why = params["reason"]
            .as_str()
            .filter(|r| !r.is_empty())
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        crate::approval::question(
            "Approve permissions",
            format!("Codex wants {what}{why}. Allow it?"),
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
        crate::approval::edit("Codex", &paths)
    }
}

use crate::{Signal, send_signal, shutdown_child};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn current_schema_and_legacy_visibility_are_compatible() {
        let page = json!({"data":[{"model":"current", "hidden":false, "isDefault":true,
            "description":"Current model", "upgrade":"next", "upgradeInfo":{"retirementAt":"2026-12-01"},
            "availabilityNux":{"message":"Available"}, "serviceTiers":["default","fast"],
            "defaultServiceTier":"default", "inputModalities":["text","image"]}], "nextCursor":"next-page"});
        let (models, next) = parse_model_list_page(&page);
        assert_eq!(
            models[0].0.description.as_deref(),
            Some("Current model (upgrade: next)")
        );
        assert!(models[0].1);
        assert_eq!(next.as_deref(), Some("next-page"));
        assert!(!legacy_model_page(&page));
        assert!(legacy_model_page(&json!({"data":[{"model":"old"}]})));
        assert!(!legacy_model_page(&json!({"data":[]})));
    }

    fn run_request(access: AccessMode, plan: bool) -> RunRequest {
        RunRequest {
            mcp: None,
            prompt: String::new(),
            harness: None,
            model: Some("gpt-5.6-sol".into()),
            reasoning: None,
            model_options: serde_json::Map::new(),
            cwd: "/repo".into(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            access,
            plan,
            attachments: Vec::new(),
            worktree: None,
            resume: None,
            env: Default::default(),
        }
    }

    #[test]
    fn each_access_mode_has_its_own_policy_and_sandbox() {
        let wire = |access, title_only| {
            let (policy, sandbox) = permissions(access, title_only);
            (policy, sandbox_mode(sandbox), sandbox_policy_value(sandbox))
        };
        assert_eq!(
            wire(AccessMode::FullAccess, false),
            ("never", "danger-full-access", json!({"type": "dangerFullAccess"}))
        );
        assert_eq!(
            wire(AccessMode::AutoAcceptEdits, false),
            (
                "untrusted",
                "workspace-write",
                json!({"type": "workspaceWrite", "networkAccess": true})
            )
        );
        assert_eq!(
            wire(AccessMode::Supervised, false),
            ("untrusted", "read-only", json!({"type": "readOnly"}))
        );
        for access in AccessMode::ALL {
            assert_eq!(
                wire(access, true),
                ("never", "read-only", json!({"type": "readOnly"}))
            );
        }
        // `auto_approve` still means full access, whatever the mode says.
        let mut request = run_request(AccessMode::Supervised, false);
        assert_eq!(run_access(&request), AccessMode::Supervised);
        request.auto_approve = true;
        assert_eq!(run_access(&request), AccessMode::FullAccess);
    }

    #[test]
    fn auto_accept_edits_accepts_file_changes_and_asks_the_rest() {
        use crate::approval::allowed_without_asking;
        let methods = [
            "item/fileChange/requestApproval",
            "item/commandExecution/requestApproval",
            "item/permissions/requestApproval",
        ];
        let allowed = |access| {
            methods.map(|method| allowed_without_asking(access, approval_action(method)))
        };
        assert_eq!(allowed(AccessMode::FullAccess), [true, true, true]);
        assert_eq!(allowed(AccessMode::AutoAcceptEdits), [true, false, false]);
        assert_eq!(allowed(AccessMode::Supervised), [false, false, false]);
    }

    #[test]
    fn access_and_plan_changes_apply_in_place() {
        let harness = CodexHarness::new();
        let live = run_request(AccessMode::FullAccess, false);
        for next in [
            run_request(AccessMode::Supervised, false),
            run_request(AccessMode::AutoAcceptEdits, false),
            run_request(AccessMode::FullAccess, true),
        ] {
            assert!(!harness.same_runtime(&live, &next), "{next:?}");
            assert!(harness.reconfigures_in_place(&live, &next), "{next:?}");
        }
        // A request that auto-approves is the same full-access runtime.
        let mut auto = run_request(AccessMode::Supervised, false);
        auto.auto_approve = true;
        assert!(harness.same_runtime(&live, &auto));
        let mut moved = run_request(AccessMode::Supervised, false);
        moved.cwd = "/elsewhere".into();
        assert!(!harness.reconfigures_in_place(&live, &moved));
        assert_eq!(harness.access_modes(), &AccessMode::ALL);
        assert!(harness.supports_plan());
    }

    #[test]
    fn collaboration_modes_repeat_the_turn_model_and_effort() {
        assert_eq!(
            collaboration_mode(true, "gpt-5.6-sol", Some("high")),
            json!({"mode": "plan", "settings": {
                "model": "gpt-5.6-sol", "reasoning_effort": "high", "developer_instructions": null,
            }})
        );
        assert_eq!(
            collaboration_mode(false, "gpt-5.6-sol", None),
            json!({"mode": "default", "settings": {
                "model": "gpt-5.6-sol", "reasoning_effort": null, "developer_instructions": null,
            }})
        );
    }

    #[test]
    fn approval_questions_are_yes_no() {
        let q = approval_question(
            "item/commandExecution/requestApproval",
            &json!({"itemId": "c1", "command": "rm -rf /tmp/x"}),
        );
        assert_eq!(q.header, "Approve command");
        assert_eq!(q.question, "Codex wants to run `rm -rf /tmp/x`. Allow it?");
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

        let q = approval_question(
            "item/permissions/requestApproval",
            &json!({"reason": "install deps", "permissions": {
                "network": {"enabled": true},
                "fileSystem": {"write": ["/repo/.cache"], "entries": [
                    {"access": "read", "path": {"type": "path", "path": "/etc/hosts"}},
                    {"access": "deny", "path": {"type": "path", "path": "/secret"}}
                ]}
            }}),
        );
        assert_eq!(q.header, "Approve permissions");
        assert_eq!(
            q.question,
            "Codex wants network access; write access to /repo/.cache; read access to /etc/hosts (install deps). Allow it?"
        );
    }

    #[test]
    fn only_a_known_old_cli_is_turned_away() {
        assert!(check_codex_version(None).is_ok());
        assert!(check_codex_version(Some(&semver::Version::new(0, 164, 0))).is_ok());
        assert!(check_codex_version(Some(&MIN_CODEX_VERSION)).is_ok());
        // A pre-release of the minimum is that release, not an older one.
        let alpha = semver::Version::parse("0.146.0-alpha.2").unwrap();
        assert!(check_codex_version(Some(&alpha)).is_ok());
        let old_alpha = semver::Version::parse("0.145.0-alpha.9").unwrap();
        assert!(check_codex_version(Some(&old_alpha)).is_err());
        let err = check_codex_version(Some(&semver::Version::new(0, 120, 3)))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Codex CLI 0.120.3 is too old") && err.contains("0.146.0"),
            "{err}"
        );
    }

    #[test]
    fn only_a_missing_conversation_starts_fresh() {
        let protocol = |m: &str| HarnessError::Protocol(m.into());
        // Live 0.164.0 wordings, as the RPC client renders them.
        for (gone, reason) in [
            (
                "thread/resume: no rollout found for thread id 019a0000-0000-7000-8000-000000000000 (code -32600)",
                "it isn't saved on this device",
            ),
            (
                "thread/resume: thread not found: abc",
                "it isn't saved on this device",
            ),
            (
                "thread/resume: failed to read thread: thread-store internal error: failed to read session metadata /h/sessions/2026/10/23/rollout-x.jsonl: rollout at /h/sessions/2026/10/23/rollout-x.jsonl is empty (code -32603)",
                "its saved history is empty",
            ),
            (
                "thread/resume: failed to read thread: thread-store internal error: failed to read session metadata /h/sessions/2026/10/23/rollout-x.jsonl: stream did not contain valid UTF-8 (code -32603)",
                "its saved history couldn't be read",
            ),
            (
                "thread/resume: invalid session id: invalid character: expected an optional prefix of `urn:uuid:` (code -32600)",
                "Codex doesn't recognize its id",
            ),
        ] {
            assert_eq!(
                resume_unrecoverable(&protocol(gone)),
                Some(reason),
                "{gone}"
            );
        }
        for keep in [
            "thread/resume: app-server exited before responding",
            "thread/resume: app-server stdin closed",
            "thread/resume: failed to load configuration",
            "thread/resume: session x is archived. Run `codex unarchive x` to unarchive it first.",
            // "not found" alone is about something other than the thread.
            "thread/resume: failed to open /repo/AGENTS.md: entity not found (code -32603)",
            "thread/resume: environment variable not found: MY_PROVIDER_KEY (code -32603)",
            "thread/resume: model provider `corp` not found (code -32600)",
            // A rollout the OS would not hand over still exists.
            "thread/resume: failed to read thread: thread-store internal error: failed to read session metadata /h/sessions/2026/10/23/rollout-x.jsonl: Permission denied (os error 13) (code -32603)",
            "thread/resume: failed to read thread: thread-store internal error: rollout at /h/sessions/2026/10/23/rollout-x.jsonl is locked (code -32603)",
            "thread/resume: failed to read thread: thread-store internal error: Too many open files (os error 24) (code -32603)",
            "thread/resume: failed to read thread: thread-store internal error: something new (code -32603)",
        ] {
            assert_eq!(resume_unrecoverable(&protocol(keep)), None, "{keep}");
        }
        assert!(transport_lost(&protocol(
            "thread/start: app-server exited before responding"
        )));
        // A failed write names its cause after the wire's own words.
        assert!(transport_lost(&protocol(
            "thread/resume: app-server stdin closed (Broken pipe (os error 32))"
        )));
        assert_eq!(
            resume_unrecoverable(&protocol(
                "thread/resume: app-server stdin closed (Broken pipe (os error 32))"
            )),
            None
        );
        assert!(!transport_lost(&protocol("thread/start: bad request")));
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
mod mcp_injection_tests {
    use super::*;

    #[test]
    fn codex_mcp_overrides_use_the_dotted_mcp_servers_keys() {
        let mcp = zeron_proto::McpServer {
            name: "zeron".into(),
            command: "/opt/zeron/zeron".into(),
            args: vec!["mcp".into()],
            env: [("ZERON_CHAT_ID".to_owned(), "chat-1".to_owned())]
                .into_iter()
                .collect(),
        };
        let overrides: serde_json::Map<String, Value> =
            codex_mcp_overrides(&mcp).into_iter().collect();
        assert_eq!(overrides["mcp_servers.zeron.command"], "/opt/zeron/zeron");
        assert_eq!(overrides["mcp_servers.zeron.args"], json!(["mcp"]));
        assert_eq!(
            overrides["mcp_servers.zeron.env"],
            json!({ "ZERON_CHAT_ID": "chat-1" })
        );
        assert_eq!(
            overrides["mcp_servers.zeron.default_tools_approval_mode"],
            "approve"
        );
    }
}

#[cfg(test)]
mod skill_discovery_tests {
    use super::*;
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
