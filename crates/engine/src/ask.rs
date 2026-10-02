//! The child ask: run a short-lived hidden child chat and get a **typed
//! result** back from it.
//!
//! A caller (goal mode's verifier today, workflow agents next) hands the
//! engine a prompt and a JSON Schema. The engine creates a child chat under
//! the requesting chat, prompts it, and waits for the child to call the
//! run-scoped MCP tool `submit_result` with arguments matching the schema.
//!
//! Behaviour, in the order a run meets it:
//!
//! - **Repair.** A submission that violates the schema is answered, as the
//!   tool result, with the path-level violations; the model gets up to
//!   [`AskSpec::max_repairs`] (3) further tries inside the same turn.
//! - **Nudge.** A turn that ends without a submission gets
//!   [`AskSpec::max_nudges`] (1) follow-up prompt asking for it.
//! - **Typed failure.** After that the ask fails with an [`AskError`] — never
//!   a hang and never a guessed result.
//! - **Cancellation.** The caller's [`CancellationToken`] interrupts the child.
//! - **Cleanup.** However the ask ends, the child is archived (not deleted:
//!   its transcript is the evidence a verdict points at).
//!
//! Callers depend on the [`AskBackend`] trait, not on [`AskService`], so
//! schedulers can be tested against [`FakeAsk`].
//!
//! # Permissions of a headless child
//!
//! The child inherits the parent's `auto_approve` and may never run with a
//! *wider* sandbox than the parent chat's ([`capped_sandbox`]); a read-only ask
//! asks for [`SandboxLevel::ReadOnly`]. No harness here enforces that level for
//! chat runs today (Codex forces full access, Claude has no read-only mode), so
//! read-only intent is carried by the prompt, and the ask's MCP server exposes
//! only read tools. A child that parks on a question or an approval can never
//! be answered — nobody watches it — so the ask fails at once with
//! [`AskError::NeedsInput`] instead of waiting out its timeout.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use zeron_doc::{MessagePart, MessageRole};
use zeron_proto::{
    AgentEvent, AskSpecInfo, AskSubmitReply, ChatConfig, HarnessId, ReasoningLevel, RunRequest,
    SandboxLevel, SchemaViolation, SessionStatus,
};

use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;
use crate::{DocHost, new_id};

/// Default wait for the whole ask (all turns).
pub const DEFAULT_ASK_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// Repair rounds after a schema-violating submission.
pub const DEFAULT_MAX_REPAIRS: u32 = 3;
/// Follow-up prompts after a turn ended without a submission.
pub const DEFAULT_MAX_NUDGES: u32 = 1;
/// How long a child may keep running after its result was accepted before it
/// is interrupted (a model usually ends its turn right after the tool call).
const ACCEPTED_GRACE: Duration = Duration::from_secs(20);
/// Violations echoed back to the model per rejected submission.
const MAX_VIOLATIONS_SHOWN: usize = 12;

/// What to ask, of whom, and how.
#[derive(Debug, Clone)]
pub struct AskSpec {
    /// Short role name ("verifier", a workflow agent's name): prefixes the
    /// child chat's title and shows up in logs.
    pub label: String,
    /// Exact child chat title. Default: `label`.
    pub title: Option<String>,
    /// The task, written for a model with no other context. The primitive
    /// appends the "deliver through `submit_result`" protocol.
    pub prompt: String,
    /// JSON Schema (draft 2020-12 by default) the result must satisfy. Local
    /// `$ref`s only; remote and file references are refused.
    pub result_schema: Value,
    /// One line on what the result is, shown in the tool description.
    pub result_description: String,
    /// Harness for the child. Default: the requesting chat's.
    pub harness: Option<HarnessId>,
    /// Model for the child. Default: the requesting chat's when the harness is
    /// the same, else the harness default.
    pub model: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    /// The child must not modify anything (verifiers, analysis). See the
    /// module docs for what that does and does not enforce.
    pub read_only: bool,
    pub timeout: Duration,
    pub max_repairs: u32,
    pub max_nudges: u32,
}

impl AskSpec {
    pub fn new(label: impl Into<String>, prompt: impl Into<String>, result_schema: Value) -> Self {
        Self {
            label: label.into(),
            title: None,
            prompt: prompt.into(),
            result_schema,
            result_description: String::new(),
            harness: None,
            model: None,
            reasoning: None,
            read_only: false,
            timeout: DEFAULT_ASK_TIMEOUT,
            max_repairs: DEFAULT_MAX_REPAIRS,
            max_nudges: DEFAULT_MAX_NUDGES,
        }
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_harness(mut self, harness: HarnessId, model: Option<String>) -> Self {
        self.harness = Some(harness);
        self.model = model;
        self
    }

    fn chat_title(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.label.clone())
    }
}

/// What the child cost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AskUsage {
    /// Summed across the child's turns, when the harness reports usage.
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Wall time from creating the child to its cleanup.
    pub elapsed_ms: u64,
    /// Turns the child ran (1 + nudges).
    pub turns: u32,
}

impl AskUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// A successful ask.
#[derive(Debug, Clone)]
pub struct AskOutcome {
    /// The schema-valid submission.
    pub result: Value,
    pub child_chat_id: String,
    pub usage: AskUsage,
    /// Rejected submissions the model repaired.
    pub repairs: u32,
    /// The turn ended once without a submission and was nudged.
    pub nudged: bool,
}

impl AskOutcome {
    /// Decode the result into `T` (the schema is the contract; this only
    /// guards a caller whose struct drifted from its schema).
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T, AskError> {
        serde_json::from_value(self.result.clone())
            .map_err(|e| AskError::InvalidResult(vec![violation("", format!("decode: {e}"))]))
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AskError {
    /// The child could not be created or started (bad schema, harness not
    /// installed, no parent, no IPC port, nesting).
    #[error("ask setup failed: {0}")]
    Setup(String),
    /// The caller cancelled it.
    #[error("ask cancelled")]
    Cancelled,
    #[error("the child did not answer within {0:?}")]
    Timeout(Duration),
    /// The child parked on a question or an approval nobody can give.
    #[error("the child asked for input it cannot get: {0}")]
    NeedsInput(String),
    /// The child's turn errored or was interrupted.
    #[error("the child's turn failed: {0}")]
    TurnFailed(String),
    /// It ended its turn — twice — without calling `submit_result`.
    #[error("the child ended its turn without submitting a result")]
    NoResult,
    /// Every repair round was spent on schema-violating submissions.
    #[error("the child could not produce a schema-valid result: {}", join_violations(.0))]
    InvalidResult(Vec<SchemaViolation>),
}

fn join_violations(violations: &[SchemaViolation]) -> String {
    violations
        .iter()
        .take(3)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// A failed ask: the error plus what was spent before it, so a budget keeps
/// counting a verifier that crashed.
#[derive(Debug, Clone)]
pub struct AskFailure {
    pub error: AskError,
    pub child_chat_id: Option<String>,
    pub usage: AskUsage,
}

impl AskFailure {
    pub fn new(error: AskError) -> Self {
        Self {
            error,
            child_chat_id: None,
            usage: AskUsage::default(),
        }
    }
}

impl std::fmt::Display for AskFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for AskFailure {}

/// The seam callers program against.
#[async_trait]
pub trait AskBackend: Send + Sync {
    /// Run one ask for `parent_chat_id`. Cancelling `cancel` interrupts the
    /// child and resolves with [`AskError::Cancelled`]; the child is archived
    /// on every path.
    async fn ask(
        &self,
        parent_chat_id: &str,
        spec: AskSpec,
        cancel: CancellationToken,
    ) -> Result<AskOutcome, AskFailure>;
}

/// [`AskBackend::ask`] with the result decoded into `T`.
pub async fn ask_typed<T: DeserializeOwned>(
    backend: &dyn AskBackend,
    parent_chat_id: &str,
    spec: AskSpec,
    cancel: CancellationToken,
) -> Result<(T, AskOutcome), AskFailure> {
    let outcome = backend.ask(parent_chat_id, spec, cancel).await?;
    match outcome.decode::<T>() {
        Ok(value) => Ok((value, outcome)),
        Err(error) => Err(AskFailure {
            error,
            child_chat_id: Some(outcome.child_chat_id.clone()),
            usage: outcome.usage,
        }),
    }
}

/// The less privileged of the parent's sandbox and the one an ask wants:
/// a child never escalates past its requester.
pub fn capped_sandbox(parent: SandboxLevel, wanted: SandboxLevel) -> SandboxLevel {
    fn rank(level: SandboxLevel) -> u8 {
        match level {
            SandboxLevel::ReadOnly => 0,
            SandboxLevel::WorkspaceWrite => 1,
            SandboxLevel::DangerFullAccess => 2,
        }
    }
    if rank(wanted) < rank(parent) {
        wanted
    } else {
        parent
    }
}

fn violation(path: &str, message: impl Into<String>) -> SchemaViolation {
    SchemaViolation {
        path: path.to_owned(),
        message: message.into(),
    }
}

// ── JSON Schema ────────────────────────────────────────────────────────────

/// Refuses `$ref`s that leave the document: a model-influenced schema must
/// never make the engine read a file or fetch a URL.
struct NoExternalRefs;

impl boon::UrlLoader for NoExternalRefs {
    fn load(&self, url: &str) -> Result<Value, Box<dyn std::error::Error>> {
        Err(format!("external schema reference not allowed: {url}").into())
    }
}

const SCHEMA_URL: &str = "zeron-ask:result";

fn compile(schema: &Value) -> Result<(boon::Schemas, boon::SchemaIndex), String> {
    let mut schemas = boon::Schemas::new();
    let mut compiler = boon::Compiler::new();
    compiler.use_loader(Box::new(NoExternalRefs));
    compiler.set_default_draft(boon::Draft::V2020_12);
    compiler
        .add_resource(SCHEMA_URL, schema.clone())
        .map_err(|e| e.to_string())?;
    let index = compiler
        .compile(SCHEMA_URL, &mut schemas)
        .map_err(|e| format!("{e:#}"))?;
    Ok((schemas, index))
}

/// Check that `schema` compiles (and references nothing external).
pub fn check_schema(schema: &Value) -> Result<(), String> {
    if !schema.is_object() && !schema.is_boolean() {
        return Err("a JSON Schema must be an object".into());
    }
    compile(schema).map(drop)
}

/// Validate `value` against `schema`. `Err` carries one entry per violated
/// location, in document order, each with a JSON-Pointer path.
pub fn validate_result(schema: &Value, value: &Value) -> Result<(), Vec<SchemaViolation>> {
    let (schemas, index) = compile(schema).map_err(|e| vec![violation("", e)])?;
    let Err(error) = schemas.validate(value, index) else {
        return Ok(());
    };
    let basic = serde_json::to_value(error.basic_output()).unwrap_or(Value::Null);
    let mut out: Vec<SchemaViolation> = Vec::new();
    for unit in basic
        .get("errors")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let path = unit
            .get("instanceLocation")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message = unit
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("does not match the schema");
        // The renderer prefixes locations with `#` when a document root is
        // meant; the model reads plain pointers.
        let path = path.trim_start_matches('#');
        if !out.iter().any(|v| v.path == path && v.message == message) {
            out.push(violation(path, message));
        }
    }
    if out.is_empty() {
        out.push(violation("", error.to_string()));
    }
    Err(out)
}

// ── the engine-backed implementation ───────────────────────────────────────

/// What a child-ask chat's `meta.askChild` holds: presence alone marks it.
const ASK_CHILD_MARKER: &str = "ask";

/// Byte comparison that doesn't stop at the first difference, so response
/// timing reveals nothing about how much of a guessed token was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = u8::from(a.len() != b.len());
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

struct AskState {
    ask_id: String,
    /// The submission secret handed only to the child's MCP server.
    token: String,
    schema: Value,
    description: String,
    max_repairs: u32,
    repairs_used: u32,
    accepted: Option<Value>,
    exhausted: Vec<SchemaViolation>,
    notify: Arc<Notify>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The engine's ask runner. Cheap to clone.
#[derive(Clone)]
pub struct AskService {
    sessions: SessionsEngine,
    workspace: WorkspaceHost,
    doc_host: DocHost,
    /// child chat id → live ask.
    live: Arc<Mutex<HashMap<String, Arc<Mutex<AskState>>>>>,
}

impl AskService {
    pub fn new(sessions: SessionsEngine, workspace: WorkspaceHost, doc_host: DocHost) -> Self {
        Self {
            sessions,
            workspace,
            doc_host,
            live: Arc::default(),
        }
    }

    /// The `submit_result` tool's advertisement for the ask running in `chat_id`.
    pub fn spec_for(&self, chat_id: &str, ask_id: &str) -> Option<AskSpecInfo> {
        let state = lock(&self.live).get(chat_id).cloned()?;
        let state = lock(&state);
        (state.ask_id == ask_id).then(|| AskSpecInfo {
            ask_id: state.ask_id.clone(),
            result_schema: state.schema.clone(),
            result_description: state.description.clone(),
        })
    }

    /// The `submit_result` tool call: validate, and either accept (the ask's
    /// waiter wakes) or answer with the violations to repair.
    pub fn submit(
        &self,
        chat_id: &str,
        ask_id: &str,
        token: &str,
        result: Value,
    ) -> AskSubmitReply {
        let Some(state) = lock(&self.live).get(chat_id).cloned() else {
            return rejected("No result is being collected from this chat any more.", 0);
        };
        let mut state = lock(&state);
        if state.ask_id != ask_id {
            return rejected("This result was meant for a different request.", 0);
        }
        // Only the child's own MCP server holds the token: a process that
        // merely learned the ids (the worker's shell, another device) can't
        // answer for the verifier.
        if !constant_time_eq(state.token.as_bytes(), token.as_bytes()) {
            tracing::warn!(chat = %chat_id, "ask submission without the ask's token refused");
            return rejected(
                "This result was not submitted by the request's own chat.",
                0,
            );
        }
        if state.accepted.is_some() {
            return AskSubmitReply {
                accepted: true,
                message: "Already received. Stop now; do not submit again.".into(),
                violations: Vec::new(),
                repairs_left: 0,
            };
        }
        if !state.exhausted.is_empty() {
            return rejected(
                "Too many invalid submissions; this request has been closed.",
                0,
            );
        }
        match validate_result(&state.schema, &result) {
            Ok(()) => {
                state.accepted = Some(result);
                state.notify.notify_one();
                AskSubmitReply {
                    accepted: true,
                    message: "Result received. Finish now with a one-line confirmation; do not do further work.".into(),
                    violations: Vec::new(),
                    repairs_left: state.max_repairs.saturating_sub(state.repairs_used),
                }
            }
            Err(violations) => {
                state.repairs_used += 1;
                let repairs_left = state.max_repairs.saturating_sub(state.repairs_used - 1);
                if repairs_left == 0 {
                    state.exhausted = violations.clone();
                    state.notify.notify_one();
                    return AskSubmitReply {
                        accepted: false,
                        message: "Rejected, and no repair attempts are left: the request is closed. Stop.".into(),
                        violations,
                        repairs_left: 0,
                    };
                }
                let shown: Vec<String> = violations
                    .iter()
                    .take(MAX_VIOLATIONS_SHOWN)
                    .map(|v| format!("- {v}"))
                    .collect();
                let more = violations.len().saturating_sub(MAX_VIOLATIONS_SHOWN);
                let tail = if more > 0 {
                    format!("\n(+{more} more)")
                } else {
                    String::new()
                };
                AskSubmitReply {
                    accepted: false,
                    message: format!(
                        "Rejected: the result does not match the schema.\n{}{tail}\nFix exactly these and call submit_result again ({repairs_left} attempt{} left).",
                        shown.join("\n"),
                        if repairs_left == 1 { "" } else { "s" },
                    ),
                    violations,
                    repairs_left,
                }
            }
        }
    }

    async fn run(
        &self,
        parent_chat_id: &str,
        spec: &AskSpec,
        cancel: &CancellationToken,
        started: Instant,
        child_slot: &mut Option<String>,
    ) -> Result<(Value, u32, bool, u32), AskError> {
        check_schema(&spec.result_schema)
            .map_err(|e| AskError::Setup(format!("bad schema: {e}")))?;
        let parent = self
            .workspace
            .chat(parent_chat_id)
            .map_err(|e| AskError::Setup(e.to_string()))?
            .ok_or_else(|| AskError::Setup(format!("no such chat: {parent_chat_id}")))?;
        let parent_config = parent.config.clone();
        // One level of nesting: a child of a child hangs off the root chat, so
        // the sidebar's "chats created by this chat" list stays flat.
        let attach_to = parent
            .parent_chat_id
            .clone()
            .unwrap_or_else(|| parent.id.clone());
        let harness = spec
            .harness
            .or(parent_config.as_ref().map(|c| c.harness))
            .unwrap_or_else(|| self.doc_host.harness_for(parent_chat_id));
        let same_harness = parent_config.as_ref().map(|c| c.harness) == Some(harness);
        let (model, reasoning, model_options) = match (&parent_config, same_harness) {
            (Some(config), true) => (
                spec.model.clone().or_else(|| config.model.clone()),
                spec.reasoning.or(config.reasoning),
                config.model_options.clone(),
            ),
            _ => (spec.model.clone(), spec.reasoning, Default::default()),
        };
        let parent_sandbox = parent_config
            .as_ref()
            .map_or(SandboxLevel::WorkspaceWrite, |c| c.sandbox);
        let sandbox = if spec.read_only {
            capped_sandbox(parent_sandbox, SandboxLevel::ReadOnly)
        } else {
            parent_sandbox
        };
        let auto_approve = self
            .sessions
            .last_request(parent_chat_id)
            .is_some_and(|r| r.auto_approve);
        let cwd = parent.cwd.clone().unwrap_or_else(|| "~".into());

        let child_id = new_id();
        let ask_id = new_id();
        // Two v4 UUIDs: 244 random bits, held in memory and the child's env.
        let token = format!("{}{}", new_id(), new_id()).replace('-', "");
        let mcp = self
            .sessions
            .ask_mcp_server(&child_id, &ask_id, &token)
            .ok_or_else(|| {
                AskError::Setup(
                    "the engine serves no IPC port, so submit_result is unreachable".into(),
                )
            })?;
        let config = ChatConfig {
            harness,
            model: model.clone(),
            reasoning,
            model_options: model_options.clone(),
            sandbox,
        };
        self.workspace
            .create_chat_with_parent(
                &child_id,
                parent.space_id.as_deref(),
                Some(&parent.device_id),
                Some(config),
                Some(cwd.clone()),
                Some(attach_to),
            )
            .map_err(|e| AskError::Setup(e.to_string()))?;
        // From here on the child exists and must be cleaned up.
        *child_slot = Some(child_id.clone());
        // Titling it also keeps the auto-titler off it.
        let _ = self.workspace.rename_chat(&child_id, &spec.chat_title());
        let handle = self
            .doc_host
            .open(&child_id)
            .map_err(|e| AskError::Setup(e.to_string()))?;
        // A marker, not the id: the synced doc must not carry what a
        // submission is checked against.
        let _ = handle.doc().set_ask_child(ASK_CHILD_MARKER);

        let state = Arc::new(Mutex::new(AskState {
            ask_id: ask_id.clone(),
            token,
            schema: spec.result_schema.clone(),
            description: spec.result_description.clone(),
            max_repairs: spec.max_repairs,
            repairs_used: 0,
            accepted: None,
            exhausted: Vec::new(),
            notify: Arc::new(Notify::new()),
        }));
        let notify = lock(&state).notify.clone();
        lock(&self.live).insert(child_id.clone(), state.clone());

        let request = RunRequest {
            prompt: String::new(),
            harness: Some(harness),
            model,
            reasoning,
            model_options,
            cwd,
            sandbox,
            auto_approve,
            resume: None,
            attachments: Vec::new(),
            worktree: None,
            mcp: Some(mcp),
        };
        let deadline = tokio::time::Instant::from_std(started + spec.timeout);
        let mut statuses = self.sessions.watch_sessions();
        let mut nudged = false;
        let mut nudges_used = 0u32;
        let mut turns = 0u32;
        let mut prompt = compose_prompt(spec);
        let mut accepted_deadline: Option<tokio::time::Instant> = None;
        'turns: loop {
            let completed_before = self
                .sessions
                .session_status(&child_id)
                .and_then(|s| s.last_completed_turn);
            let mut turn_request = request.clone();
            turn_request.prompt = prompt.clone();
            self.sessions
                .dispatch(&child_id, harness, turn_request, Some(new_id()))
                .await
                .map_err(|e| AskError::Setup(format!("could not start the child: {e}")))?;
            turns += 1;
            loop {
                // Copy out under the lock: a guard held across the branches
                // below would deadlock the re-locks inside them.
                let (accepted, repairs, exhausted) = {
                    let state = lock(&state);
                    (
                        state.accepted.clone(),
                        state.repairs_used,
                        state.exhausted.clone(),
                    )
                };
                if let Some(result) = accepted.clone() {
                    // Let the child's closing remark land; never wait on it
                    // beyond the grace period.
                    if !self.sessions.turn_in_flight(&child_id) {
                        return Ok((result, repairs, nudged, turns));
                    }
                    let until = *accepted_deadline
                        .get_or_insert_with(|| tokio::time::Instant::now() + ACCEPTED_GRACE);
                    if tokio::time::Instant::now() >= until {
                        return Ok((result, repairs, nudged, turns));
                    }
                }
                if !exhausted.is_empty() {
                    return Err(AskError::InvalidResult(exhausted));
                }
                let session = self.sessions.session_status(&child_id);
                match session.as_ref().map(|s| s.status) {
                    Some(SessionStatus::AwaitingInput) => {
                        return Err(AskError::NeedsInput(self.pending_question(&child_id)));
                    }
                    Some(SessionStatus::Working) => {}
                    // Idle or Errored: this turn is over.
                    other => {
                        if let Some(result) = accepted {
                            return Ok((result, repairs, nudged, turns));
                        }
                        if other == Some(SessionStatus::Errored) {
                            return Err(AskError::TurnFailed(self.last_error(&child_id)));
                        }
                        let completed = session.and_then(|s| s.last_completed_turn);
                        if completed == completed_before {
                            return Err(AskError::TurnFailed(
                                "the child's turn was interrupted".into(),
                            ));
                        }
                        if nudges_used >= spec.max_nudges {
                            return Err(AskError::NoResult);
                        }
                        nudges_used += 1;
                        nudged = true;
                        prompt = NUDGE_PROMPT.to_owned();
                        continue 'turns;
                    }
                }
                let wake = accepted_deadline.unwrap_or(deadline).min(deadline);
                tokio::select! {
                    _ = cancel.cancelled() => return Err(AskError::Cancelled),
                    _ = tokio::time::sleep_until(wake) => {
                        if tokio::time::Instant::now() >= deadline {
                            return Err(AskError::Timeout(spec.timeout));
                        }
                    }
                    _ = statuses.changed() => {}
                    _ = notify.notified() => {}
                }
            }
        }
    }

    fn last_error(&self, chat_id: &str) -> String {
        let text = self.doc_host.open(chat_id).ok().and_then(|handle| {
            handle
                .doc()
                .read_entries()
                .ok()?
                .iter()
                .rev()
                .filter(|e| e.role == MessageRole::Assistant)
                .find_map(|e| {
                    e.parts.iter().rev().find_map(|p| match p {
                        MessagePart::Error { message, .. } => Some(message.clone()),
                        _ => None,
                    })
                })
        });
        text.unwrap_or_else(|| "the harness reported an error".into())
    }

    fn pending_question(&self, chat_id: &str) -> String {
        // The journal has it the instant it is raised; the transcript only
        // after its next coalesced commit.
        let text = self
            .sessions
            .subscribe(chat_id, 0)
            .ok()
            .and_then(|(replay, _rx)| {
                replay.iter().rev().find_map(|e| match &e.event {
                    AgentEvent::InputRequested { questions, .. } => {
                        questions.first().map(|q| q.question.clone())
                    }
                    _ => None,
                })
            });
        text.unwrap_or_else(|| "a question or approval".into())
    }

    /// Tokens the child's turns reported, from the run journal (lag-free,
    /// unlike a live subscription).
    fn journal_usage(&self, chat_id: &str) -> (u64, u64) {
        let Ok((replay, _rx)) = self.sessions.subscribe(chat_id, 0) else {
            return (0, 0);
        };
        replay.iter().fold((0, 0), |(i, o), e| match &e.event {
            AgentEvent::Usage {
                input_tokens,
                output_tokens,
            } => (i + input_tokens, o + output_tokens),
            _ => (i, o),
        })
    }
}

#[async_trait]
impl AskBackend for AskService {
    async fn ask(
        &self,
        parent_chat_id: &str,
        spec: AskSpec,
        cancel: CancellationToken,
    ) -> Result<AskOutcome, AskFailure> {
        let started = Instant::now();
        let mut child: Option<String> = None;
        let result = self
            .run(parent_chat_id, &spec, &cancel, started, &mut child)
            .await;
        // Cleanup runs on every path: stop the child if it is still going,
        // forget its ask, archive it (the transcript is the evidence).
        let mut usage = AskUsage::default();
        if let Some(child_id) = &child {
            if self.sessions.turn_in_flight(child_id) {
                let _ = self.sessions.interrupt(child_id).await;
            }
            lock(&self.live).remove(child_id);
            let (input_tokens, output_tokens) = self.journal_usage(child_id);
            usage.input_tokens = input_tokens;
            usage.output_tokens = output_tokens;
            if let Err(err) = self.workspace.set_chat_archived(child_id, true) {
                tracing::warn!(chat = %child_id, error = %err, "ask child archive failed");
            }
        }
        usage.elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok((result, repairs, nudged, turns)) => {
                usage.turns = turns;
                Ok(AskOutcome {
                    result,
                    child_chat_id: child.unwrap_or_default(),
                    usage,
                    repairs,
                    nudged,
                })
            }
            Err(error) => Err(AskFailure {
                error,
                child_chat_id: child,
                usage,
            }),
        }
    }
}

fn rejected(message: &str, repairs_left: u32) -> AskSubmitReply {
    AskSubmitReply {
        accepted: false,
        message: message.into(),
        violations: Vec::new(),
        repairs_left,
    }
}

const NUDGE_PROMPT: &str = "Your turn ended without calling the `submit_result` tool, so nothing \
was received. Call `submit_result` now with your final result (its arguments must match the \
tool's schema). Do only what is needed to produce the result.";

fn compose_prompt(spec: &AskSpec) -> String {
    let guard = if spec.read_only {
        "\nYou are read-only: do not create, modify, delete, move, or format any file, and do \
not change git state or run anything that changes the environment. Reading files and running \
read-only commands (tests included, if they leave no changes behind) is fine."
    } else {
        ""
    };
    format!(
        "{}\n\n---\nHow to answer: call the `submit_result` tool of the `zeron` MCP server \
exactly once with your result; its arguments are the result and must match the tool's schema. \
Only that tool call is read — an answer written as prose is lost. If the tool rejects \
your submission, it lists what to fix: fix it and call it again. There is no person in this \
conversation: never ask a question and never wait for approval; decide, then submit.{guard}",
        spec.prompt
    )
}

// ── a scriptable backend for tests ─────────────────────────────────────────

/// One scripted reply of a [`FakeAsk`].
#[derive(Debug, Clone)]
pub enum FakeReply {
    /// Succeed with this result (checked against the spec's schema).
    Result(Value),
    /// [`FakeReply::Result`] reporting this cost, for budget accounting.
    ResultWithUsage(Value, AskUsage),
    /// Fail with this error.
    Fail(AskError),
    /// Wait this long (or until cancelled), then play the inner reply.
    After(Duration, Box<FakeReply>),
    /// Never answer; resolves only when cancelled or the spec's timeout hits.
    Hang,
}

/// A recorded [`FakeAsk::ask`] call.
#[derive(Debug, Clone)]
pub struct FakeCall {
    pub parent_chat_id: String,
    pub spec: AskSpec,
}

type FakeHandler = dyn Fn(&FakeCall) -> Option<FakeReply> + Send + Sync;

/// An [`AskBackend`] with scripted answers: no chats, no harness, no engine.
/// Replies are consumed in order; a [`FakeAsk::on_call`] handler (consulted
/// first) can answer from the call instead. An empty script fails the ask
/// with [`AskError::Setup`], loudly, rather than inventing a result.
#[derive(Default)]
pub struct FakeAsk {
    script: Mutex<VecDeque<FakeReply>>,
    handler: Mutex<Option<Arc<FakeHandler>>>,
    calls: Mutex<Vec<FakeCall>>,
    next_child: Mutex<u32>,
    cancelled: std::sync::atomic::AtomicUsize,
}

impl FakeAsk {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn push(&self, reply: FakeReply) -> &Self {
        lock(&self.script).push_back(reply);
        self
    }

    pub fn push_result(&self, result: Value) -> &Self {
        self.push(FakeReply::Result(result))
    }

    pub fn on_call(
        &self,
        handler: impl Fn(&FakeCall) -> Option<FakeReply> + Send + Sync + 'static,
    ) {
        *lock(&self.handler) = Some(Arc::new(handler));
    }

    pub fn calls(&self) -> Vec<FakeCall> {
        lock(&self.calls).clone()
    }

    /// Asks that ended because their token was cancelled.
    pub fn cancelled(&self) -> usize {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn child_id(&self) -> String {
        let mut n = lock(&self.next_child);
        *n += 1;
        format!("fake-child-{n}")
    }
}

#[async_trait]
impl AskBackend for FakeAsk {
    async fn ask(
        &self,
        parent_chat_id: &str,
        spec: AskSpec,
        cancel: CancellationToken,
    ) -> Result<AskOutcome, AskFailure> {
        let call = FakeCall {
            parent_chat_id: parent_chat_id.to_owned(),
            spec: spec.clone(),
        };
        lock(&self.calls).push(call.clone());
        let child = self.child_id();
        let failure = |error: AskError| {
            if error == AskError::Cancelled {
                self.cancelled
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            AskFailure {
                error,
                child_chat_id: Some(child.clone()),
                usage: AskUsage::default(),
            }
        };
        let handler = lock(&self.handler).clone();
        let mut reply = handler
            .and_then(|h| h(&call))
            .or_else(|| lock(&self.script).pop_front())
            .ok_or_else(|| failure(AskError::Setup("FakeAsk has no scripted reply".into())))?;
        loop {
            match reply {
                FakeReply::After(delay, inner) => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(failure(AskError::Cancelled)),
                        _ = tokio::time::sleep(delay) => {}
                    }
                    reply = *inner;
                }
                FakeReply::Hang => {
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(failure(AskError::Cancelled)),
                        _ = tokio::time::sleep(spec.timeout) => {
                            return Err(failure(AskError::Timeout(spec.timeout)));
                        }
                    }
                }
                FakeReply::Fail(error) => return Err(failure(error)),
                FakeReply::Result(result) => {
                    reply = FakeReply::ResultWithUsage(
                        result,
                        AskUsage {
                            turns: 1,
                            ..AskUsage::default()
                        },
                    );
                }
                FakeReply::ResultWithUsage(result, usage) => {
                    if let Err(violations) = validate_result(&spec.result_schema, &result) {
                        return Err(failure(AskError::InvalidResult(violations)));
                    }
                    return Ok(AskOutcome {
                        result,
                        child_chat_id: child,
                        usage,
                        repairs: 0,
                        nudged: false,
                    });
                }
            }
        }
    }
}

/// The scripted verifier behind `ZERON_MOCK_GOAL=1`: two not-satisfied
/// verdicts (a few seconds apart, like a real check) and then a pass.
pub fn demo_verifier() -> Arc<FakeAsk> {
    let fake = FakeAsk::new();
    let round = Arc::new(Mutex::new(0u32));
    fake.on_call(move |_| {
        let mut n = lock(&round);
        *n += 1;
        let (passed, reason, next) = match *n {
            1 => (
                false,
                "Ran the build: it still fails in the manifest parser, and no regression test covers it.",
                "Fix the manifest parser and add a regression test",
            ),
            2 => (
                false,
                "The parser is fixed and tested, but the full suite and the changelog were not run or updated.",
                "Run the full test suite and update the changelog",
            ),
            _ => (
                true,
                "cargo test passes (214 tests); the changelog entry is present; the build is green.",
                "",
            ),
        };
        Some(FakeReply::After(
            Duration::from_secs(3),
            Box::new(FakeReply::ResultWithUsage(
                serde_json::json!({"passed": passed, "reason": reason, "nextAction": next}),
                AskUsage {
                    input_tokens: 4_200,
                    output_tokens: 600,
                    elapsed_ms: 3_000,
                    turns: 1,
                },
            )),
        ))
    });
    fake
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "passed": {"type": "boolean"},
                "reason": {"type": "string", "minLength": 1},
                "items": {"type": "array", "items": {"type": "object", "required": ["name"]}}
            },
            "required": ["passed", "reason"],
            "additionalProperties": false
        })
    }

    #[test]
    fn valid_results_pass() {
        assert!(validate_result(&schema(), &json!({"passed": true, "reason": "ok"})).is_ok());
    }

    #[test]
    fn violations_carry_json_pointer_paths() {
        let value = json!({"passed": "yes", "reason": "", "items": [{"name": 1}, {}], "extra": 1});
        let violations = validate_result(&schema(), &value).unwrap_err();
        let paths: Vec<&str> = violations.iter().map(|v| v.path.as_str()).collect();
        assert!(paths.contains(&"/passed"), "{violations:?}");
        assert!(paths.contains(&"/reason"), "{violations:?}");
        assert!(paths.contains(&"/items/1"), "{violations:?}");
        // The message names the problem, not the schema machinery.
        assert!(
            violations
                .iter()
                .any(|v| v.path == "/passed" && v.message.contains("boolean"))
        );
    }

    #[test]
    fn a_wrong_root_type_reports_the_root() {
        let violations = validate_result(&schema(), &json!([1])).unwrap_err();
        assert_eq!(violations[0].path, "");
    }

    #[test]
    fn broken_schemas_are_refused_up_front() {
        assert!(check_schema(&json!({"type": "nonsense"})).is_err());
        assert!(check_schema(&json!("string")).is_err());
        assert!(check_schema(&schema()).is_ok());
    }

    #[test]
    fn external_references_are_refused() {
        let file = json!({"$ref": "file:///etc/hostname"});
        assert!(check_schema(&file).is_err());
        let http = json!({"$ref": "https://example.com/schema.json"});
        assert!(check_schema(&http).is_err());
        // Local references still work.
        let local = json!({"$defs": {"n": {"type": "integer"}}, "$ref": "#/$defs/n"});
        assert!(check_schema(&local).is_ok());
        assert!(validate_result(&local, &json!(3)).is_ok());
        assert!(validate_result(&local, &json!("x")).is_err());
    }

    #[test]
    fn sandbox_never_widens() {
        use SandboxLevel::*;
        assert_eq!(capped_sandbox(WorkspaceWrite, ReadOnly), ReadOnly);
        assert_eq!(capped_sandbox(ReadOnly, DangerFullAccess), ReadOnly);
        assert_eq!(
            capped_sandbox(DangerFullAccess, WorkspaceWrite),
            WorkspaceWrite
        );
        assert_eq!(
            capped_sandbox(WorkspaceWrite, WorkspaceWrite),
            WorkspaceWrite
        );
    }

    #[test]
    fn prompt_appends_the_protocol_and_the_read_only_guard() {
        let plain = compose_prompt(&AskSpec::new("a", "Do the thing.", json!({})));
        assert!(plain.starts_with("Do the thing."));
        assert!(plain.contains("submit_result"));
        assert!(!plain.contains("read-only"));
        let guarded = compose_prompt(&AskSpec::new("a", "Look.", json!({})).read_only());
        assert!(guarded.contains("You are read-only"));
    }

    #[tokio::test]
    async fn fake_ask_plays_its_script_and_records_calls() {
        let fake = FakeAsk::new();
        fake.push_result(json!({"passed": true, "reason": "ok"}))
            .push(FakeReply::Fail(AskError::NoResult));
        let spec = || AskSpec::new("v", "p", schema());
        let ok = fake
            .ask("chat", spec(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(ok.result["passed"], true);
        let err = fake
            .ask("chat", spec(), CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(err.error, AskError::NoResult);
        let empty = fake
            .ask("chat", spec(), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(empty.error, AskError::Setup(_)));
        assert_eq!(fake.calls().len(), 3);
        assert_eq!(fake.calls()[0].parent_chat_id, "chat");
    }

    #[tokio::test]
    async fn fake_ask_rejects_results_that_break_the_schema_and_honours_cancel() {
        let fake = FakeAsk::new();
        fake.push_result(json!({"passed": "nope"}));
        let err = fake
            .ask(
                "c",
                AskSpec::new("v", "p", schema()),
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(matches!(err.error, AskError::InvalidResult(_)));

        fake.push(FakeReply::Hang);
        let token = CancellationToken::new();
        let waiting = {
            let token = token.clone();
            let fake = fake.clone();
            tokio::spawn(
                async move { fake.ask("c", AskSpec::new("v", "p", schema()), token).await },
            )
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
        let err = waiting.await.unwrap().unwrap_err();
        assert_eq!(err.error, AskError::Cancelled);
    }

    #[tokio::test]
    async fn typed_decoding() {
        #[derive(serde::Deserialize)]
        struct Verdict {
            passed: bool,
            reason: String,
        }
        let fake = FakeAsk::new();
        fake.push_result(json!({"passed": false, "reason": "not yet"}));
        let (verdict, outcome): (Verdict, _) = ask_typed(
            fake.as_ref(),
            "c",
            AskSpec::new("v", "p", schema()),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(!verdict.passed);
        assert_eq!(verdict.reason, "not yet");
        assert_eq!(outcome.child_chat_id, "fake-child-1");
    }
}
