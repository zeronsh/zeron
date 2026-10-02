//! Dynamic workflows, engine side (`docs/workflows.md`).
//!
//! [`WorkflowService`] is what the rest of the engine (RPC, the command
//! plane, the goal controller) talks to: start a run (analysis, draft,
//! approval, launch), stop it, resume it from its journal, answer an actor's
//! escalation, read runs and artifacts, reconcile after a restart, and
//! deliver the result to the parent chat.
//!
//! The pieces underneath:
//!
//! * `core` — one running workflow: the `Host` its script talks to and the
//!   scheduler (per-actor FIFO, gates, governor, redrive, budgets);
//! * `store` — run directories: journal, script, meta, artifacts;
//! * `projection` — events folded into the parent chat's doc;
//! * `world` — shell gates and read-only project views, confined to the
//!   project root;
//! * `faults`, `governor` — provider-error classification, backoff, AIMD;
//! * `prompts` — actor instructions, approval text, completion message.

mod core;
pub mod demo;
mod faults;
mod governor;
mod projection;
mod prompts;
pub mod saved;
mod saved_ops;
pub mod store;
mod world;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use zeron_proto::{
    HarnessId, MessageOrigin, SavedRunRef, SavedScope, UserInputQuestion,
    WORKFLOW_APPROVAL_META_KIND, WorkflowApprovalMeta, WorkflowBudgets, WorkflowEvent,
    WorkflowEventKind, WorkflowEventMarker, WorkflowGraph, WorkflowRun, WorkflowRunHeader,
    WorkflowRunsState, WorkflowStatus, WorkflowStopReason,
};
use zeron_workflow::{Analysis, Diagnostic, Limits, RunError, analyze, run_script};

use self::core::{Defaults, RunCore, StopInfo};
pub use self::faults::{Fault, backoff, classify};
pub use self::governor::{Gate, Governor};
pub use self::projection::{BATCH as PROJECTION_BATCH, Projection, ProjectionStats};
pub use self::saved_ops::{
    SaveError, SaveOutcome, SaveRequest, SaveSource, SavedContext, parse_args_declaration,
};
use self::store::{ArtifactChunk, ArtifactIndex, RunMeta, RunOptions, StoreError, WorkflowStore};
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;
use crate::{DocHost, new_id};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Asks a run may start unless its budget says otherwise.
pub const DEFAULT_MAX_ASKS: u32 = 500;
/// Folder (inside the project) where approved scripts are drafted.
pub const DRAFT_DIR: &str = ".zeron/workflow-drafts";
/// Answer label that approves a workflow.
pub const APPROVE_LABEL: &str = "Run workflow";
/// Answer label that approves saving a workflow file.
pub const SAVE_LABEL: &str = "Save workflow";
pub const DENY_LABEL: &str = "Deny";

// ── seams ─────────────────────────────────────────────────────────────────

/// How a person (or a test) approves a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    Approved,
    Denied(String),
}

#[async_trait]
pub trait Approver: Send + Sync {
    async fn approve(&self, chat_id: &str, question: UserInputQuestion) -> Approval;
}

/// Checks that a harness (and model) a script names can run here.
#[async_trait]
pub trait Catalog: Send + Sync {
    async fn check(&self, harness: HarnessId, model: Option<&str>) -> Result<(), String>;
}

/// Production approver: the question rides the chat's live turn, exactly
/// like a harness permission question, and is answered with
/// `respond_to_input` from any client.
pub struct SessionsApprover {
    sessions: SessionsEngine,
    timeout: Duration,
}

#[async_trait]
impl Approver for SessionsApprover {
    async fn approve(&self, chat_id: &str, question: UserInputQuestion) -> Approval {
        // The first option is the "yes" (`Run workflow`, `Save workflow`).
        let approve_label = question.options.first().cloned().unwrap_or_default();
        let Some(rx) = self.sessions.request_input(chat_id, vec![question]) else {
            return Approval::Denied(
                "approval needs the chat's active turn: start the workflow from inside the chat"
                    .into(),
            );
        };
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(answers))
                if answers
                    .iter()
                    .any(|a| a.labels.iter().any(|l| l == &approve_label)) =>
            {
                Approval::Approved
            }
            // An interrupt (or the turn ending) answers parked questions with
            // nothing: that is a cancelled question, not a user's "no".
            Ok(Ok(answers)) if answers.is_empty() => Approval::Denied(
                "the approval was cancelled: the chat's turn was interrupted or ended before the question was answered"
                    .into(),
            ),
            Ok(Ok(_)) => Approval::Denied("the user denied it".into()),
            Ok(Err(_)) => Approval::Denied(
                "the approval question was lost (the turn ended before it was answered)".into(),
            ),
            Err(_) => Approval::Denied("nobody answered the approval question in time".into()),
        }
    }
}

/// Production catalog: harness installed and enabled here; a named model is
/// checked against the harness' catalog when that can be listed (an
/// unlistable catalog accepts, since refusing would be a guess).
pub struct EngineCatalog {
    sessions: SessionsEngine,
    doc_host: DocHost,
}

#[async_trait]
impl Catalog for EngineCatalog {
    async fn check(&self, harness: HarnessId, model: Option<&str>) -> Result<(), String> {
        // The test/demo harness is never offered in pickers; it always runs.
        if harness == HarnessId::Mock {
            return Ok(());
        }
        let descriptors = self.sessions.harness_descriptors();
        match descriptors.iter().find(|d| d.id == harness) {
            None => {
                return Err(format!(
                    "harness {harness:?} is not available on this device"
                ));
            }
            Some(d) if !d.installed => {
                return Err(format!(
                    "harness {} is not installed on this device",
                    d.name
                ));
            }
            Some(d) if d.enabled == Some(false) => {
                return Err(format!(
                    "harness {} is disabled in Settings → Providers",
                    d.name
                ));
            }
            Some(_) => {}
        }
        let Some(model) = model else { return Ok(()) };
        match self.doc_host.list_harness_models(harness).await {
            Some(models) if !models.iter().any(|m| m.id == model) => Err(format!(
                "model {model:?} is not offered by {harness:?} (list_models shows what is)"
            )),
            _ => Ok(()),
        }
    }
}

// ── requests and results ──────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct StartRequest {
    pub name: Option<String>,
    pub script: Option<String>,
    /// A script file inside the project (relative path).
    pub path: Option<String>,
    pub args: Value,
    pub max_concurrency: Option<u32>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub budgets: WorkflowBudgets,
    /// Start a saved workflow instead of `script` / `path`.
    pub saved: Option<SavedStart>,
    /// A person started it (a launcher click, `/workflow`): the click is the
    /// approval, since no agent turn exists to carry the question. Honoured
    /// only together with `saved` — an agent's script is never auto-approved.
    pub by_user: bool,
}

/// Which saved workflow to start and with what.
#[derive(Debug, Clone, Default)]
pub struct SavedStart {
    pub name: String,
    /// Look only in this scope; unset = project, then global, then built-in.
    pub scope: Option<SavedScope>,
    pub args: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The script does not pass analysis; no run was created.
    #[error("{}", zeron_workflow::diagnostic::render(.0))]
    Diagnostics(Vec<Diagnostic>),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Denied(String),
}

#[derive(Debug, Clone)]
pub struct StartOutcome {
    pub run_id: String,
    pub name: String,
    pub graph: WorkflowGraph,
    pub warnings: Vec<Diagnostic>,
    pub max_concurrency: u32,
    pub draft_path: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("no such workflow run: {0}")]
    NoRun(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// What `get_workflow_run` reports.
#[derive(Debug, Clone)]
pub struct RunView {
    pub run: WorkflowRun,
    /// The arguments the script received.
    pub args: Value,
    /// The saved workflow it came from.
    pub saved: Option<SavedRunRef>,
    /// The script's full return value, when it finished with one.
    pub result: Option<Value>,
    /// Every `report()` item in full.
    pub reports: Vec<Value>,
}

/// Knobs tests turn.
#[derive(Debug, Clone)]
pub struct Tuning {
    pub approval_timeout: Duration,
    pub stall_check: Duration,
    pub idle_wait: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            approval_timeout: Duration::from_secs(15 * 60),
            stall_check: Duration::from_secs(30),
            idle_wait: Duration::from_secs(15),
        }
    }
}

pub(crate) struct Shared {
    pub doc_host: DocHost,
    pub sessions: SessionsEngine,
    pub workspace: WorkspaceHost,
    pub store: WorkflowStore,
    pub projection: Projection,
    pub runs: Mutex<HashMap<String, Arc<RunCore>>>,
    pub approver: Mutex<Arc<dyn Approver>>,
    pub catalog: Mutex<Arc<dyn Catalog>>,
    pub events_tx: broadcast::Sender<WorkflowEvent>,
    pub tuning: Mutex<Tuning>,
    pub saved: Mutex<saved::SavedStore>,
}

#[derive(Clone)]
pub struct WorkflowService {
    shared: Arc<Shared>,
}

/// `clamp(cores - 2, 1, 16)`.
pub fn default_concurrency() -> u32 {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores.saturating_sub(2)).clamp(1, 16) as u32
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        "workflow".into()
    } else if cleaned.chars().count() > 80 {
        cleaned.chars().take(80).collect()
    } else {
        cleaned.to_owned()
    }
}

fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_owned();
    if s.is_empty() {
        "workflow".into()
    } else {
        s.chars().take(40).collect()
    }
}

fn defaults_for(workspace: &WorkspaceHost, chat_id: &str) -> Defaults {
    let config = workspace.chat_config(chat_id);
    Defaults {
        harness: config.as_ref().map_or(HarnessId::ClaudeCode, |c| c.harness),
        model: config.as_ref().and_then(|c| c.model.clone()),
        reasoning: config.as_ref().and_then(|c| c.reasoning),
    }
}

impl WorkflowService {
    pub fn new(
        doc_host: DocHost,
        sessions: SessionsEngine,
        workspace: WorkspaceHost,
        store_root: &Path,
    ) -> Self {
        let (events_tx, _) = broadcast::channel(4096);
        let timeout = Tuning::default().approval_timeout;
        Self {
            shared: Arc::new(Shared {
                projection: Projection::new(doc_host.clone()),
                store: WorkflowStore::open(store_root),
                runs: Mutex::default(),
                approver: Mutex::new(Arc::new(SessionsApprover {
                    sessions: sessions.clone(),
                    timeout,
                })),
                catalog: Mutex::new(Arc::new(EngineCatalog {
                    sessions: sessions.clone(),
                    doc_host: doc_host.clone(),
                })),
                events_tx,
                tuning: Mutex::new(Tuning::default()),
                saved: Mutex::new(saved::SavedStore::new(saved::default_global_dir())),
                doc_host,
                sessions,
                workspace,
            }),
        }
    }

    // ── test and wiring seams ──────────────────────────────────────────────

    pub fn set_approver(&self, approver: Arc<dyn Approver>) {
        *lock(&self.shared.approver) = approver;
    }

    pub fn set_catalog(&self, catalog: Arc<dyn Catalog>) {
        *lock(&self.shared.catalog) = catalog;
    }

    pub fn set_tuning(&self, tuning: Tuning) {
        *lock(&self.shared.tuning) = tuning;
    }

    /// Every event of every run, as it is journaled.
    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.shared.events_tx.subscribe()
    }

    /// Every chat's runs as briefs: the sidebar's feed.
    pub fn watch_activity(&self) -> tokio::sync::watch::Receiver<zeron_proto::WorkflowActivity> {
        self.shared.projection.watch_activity()
    }

    pub fn projection_stats(&self) -> ProjectionStats {
        self.shared.projection.stats()
    }

    pub fn flush(&self) {
        self.shared.projection.flush_all();
    }

    pub fn store(&self) -> &WorkflowStore {
        &self.shared.store
    }

    /// The chat has a workflow running (the goal controller waits for it).
    pub fn has_running_run(&self, chat_id: &str) -> bool {
        lock(&self.shared.runs)
            .values()
            .any(|c| c.chat_id == chat_id && c.is_launched())
    }

    // ── starting ───────────────────────────────────────────────────────────

    pub async fn start(
        &self,
        chat_id: &str,
        req: StartRequest,
    ) -> Result<StartOutcome, StartError> {
        let chat = self
            .shared
            .workspace
            .chat(chat_id)
            .map_err(|e| StartError::Invalid(e.to_string()))?
            .ok_or_else(|| StartError::Invalid(format!("no such chat: {chat_id}")))?;
        let root = chat
            .cwd
            .clone()
            .map(PathBuf::from)
            .and_then(|p| p.canonicalize().ok())
            .filter(|p| p.is_dir())
            .ok_or_else(|| {
                StartError::Invalid(
                    "the chat's project folder is not available on this device".into(),
                )
            })?;
        let mut saved_ref: Option<SavedRunRef> = None;
        let mut saved_args: Option<Value> = None;
        let mut saved_path: Option<String> = None;
        let (script, label) = match (&req.script, &req.path, &req.saved) {
            (Some(s), None, None) => (s.clone(), "workflow.star".to_owned()),
            (None, Some(path), None) => {
                let full = store::confine(&root, path).map_err(StartError::Invalid)?;
                let text = std::fs::read_to_string(&full)
                    .map_err(|e| StartError::Invalid(format!("could not read {path:?}: {e}")))?;
                (text, path.clone())
            }
            (None, None, Some(sv)) => {
                // Resolve, then validate the arguments BEFORE anything is
                // shown to a person: a call that cannot run is not worth an
                // approval, and the caller fixes it from the error.
                let store = lock(&self.shared.saved).clone();
                let project = saved::ProjectRef {
                    root: root.clone(),
                    space_id: None,
                };
                let loaded = store
                    .resolve(&sv.name, sv.scope, Some(&project))
                    .map_err(|e| StartError::Invalid(e.to_string()))?;
                let args = zeron_proto::validate_args(&loaded.summary.args, &sv.args)
                    .map_err(|errors| StartError::Invalid(errors.join("\n")))?;
                saved_ref = Some(SavedRunRef {
                    name: loaded.summary.name.clone(),
                    scope: loaded.summary.scope,
                });
                saved_args = Some(args);
                saved_path = loaded.summary.path.as_deref().map(|p| {
                    Path::new(p)
                        .strip_prefix(&root)
                        .map_or_else(|_| p.to_owned(), |r| r.to_string_lossy().into_owned())
                });
                (loaded.text, format!("{}.star", sv.name))
            }
            _ => {
                return Err(StartError::Invalid(
                    "pass exactly one of `script`, `path` and `saved`".into(),
                ));
            }
        };
        let analysis = analyze(&label, &script).map_err(StartError::Diagnostics)?;
        let args = match saved_args {
            Some(args) => args,
            None => match req.args.clone() {
                Value::Null => json!({}),
                v @ Value::Object(_) => v,
                _ => return Err(StartError::Invalid("`args` must be an object".into())),
            },
        };
        let name = sanitize_name(
            req.name
                .as_deref()
                .or(saved_ref.as_ref().map(|s| s.name.as_str()))
                .unwrap_or(match &req.path {
                    Some(p) => Path::new(p)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("workflow"),
                    None => "workflow",
                }),
        );
        let defaults = defaults_for(&self.shared.workspace, chat_id);
        // Validate what the caller picked up front, with a clear error.
        if let Some(h) = &req.harness {
            let id: HarnessId = serde_json::from_value(Value::String(h.clone()))
                .map_err(|_| StartError::Invalid(format!("unknown harness {h:?}")))?;
            let catalog = lock(&self.shared.catalog).clone();
            catalog
                .check(id, req.model.as_deref())
                .await
                .map_err(StartError::Invalid)?;
        }
        let max_concurrency = req
            .max_concurrency
            .unwrap_or_else(default_concurrency)
            .clamp(1, 32);
        let mut budgets = req.budgets.clone();
        budgets.max_asks.get_or_insert(DEFAULT_MAX_ASKS);
        let script_hash = sha_hex(script.as_bytes());
        let args_hash = sha_hex(serde_json::to_string(&args).unwrap_or_default().as_bytes());
        // A saved workflow already is a file the person can open; drafting a
        // copy would only add a second one to keep in sync.
        let draft_path = if saved_ref.is_some() {
            saved_path
        } else {
            self.write_draft(&root, &name, &script_hash, &script)
        };
        let meta = RunMeta {
            run_id: new_id(),
            chat_id: chat_id.to_owned(),
            name: name.clone(),
            script_hash,
            args_hash,
            args,
            options: RunOptions {
                max_concurrency,
                harness: req.harness.clone(),
                model: req.model.clone(),
                reasoning: req.reasoning.clone(),
                budgets,
            },
            project_root: root.to_string_lossy().into_owned(),
            draft_path: draft_path.clone(),
            created_at: crate::now_ms(),
            saved: saved_ref.clone(),
            ..RunMeta::default()
        };
        let outcome = StartOutcome {
            run_id: meta.run_id.clone(),
            name,
            graph: analysis.graph.clone(),
            warnings: analysis.warnings.clone(),
            max_concurrency,
            draft_path,
        };
        self.begin(
            meta,
            script,
            analysis,
            defaults,
            store::Replay::default(),
            0,
            req.by_user && saved_ref.is_some(),
        )
        .await?;
        Ok(outcome)
    }

    /// Best effort: the script, where the user can read it, inside the
    /// project (`.zeron/workflow-drafts/`; add it to .gitignore). The run's
    /// own copy under the data dir is authoritative.
    fn write_draft(&self, root: &Path, name: &str, hash: &str, script: &str) -> Option<String> {
        let rel = format!("{DRAFT_DIR}/{}-{}.star", slug(name), &hash[..8]);
        let full = root.join(&rel);
        std::fs::create_dir_all(full.parent()?).ok()?;
        std::fs::write(&full, script).ok()?;
        Some(rel)
    }

    #[allow(clippy::too_many_arguments)]
    async fn begin(
        &self,
        meta: RunMeta,
        script: String,
        analysis: Analysis,
        defaults: Defaults,
        replay: store::Replay,
        base_elapsed_ms: u64,
        user_initiated: bool,
    ) -> Result<(), StartError> {
        let shared = self.shared.clone();
        shared
            .store
            .create_run(&meta, &script)
            .map_err(|e| StartError::Invalid(e.to_string()))?;
        let journal = shared
            .store
            .open_journal(&meta.run_id)
            .map_err(|e| StartError::Invalid(e.to_string()))?;
        let world = world::World::new(PathBuf::from(&meta.project_root));
        let core = RunCore::new(
            shared.clone(),
            meta.clone(),
            journal,
            replay,
            world,
            defaults.clone(),
            base_elapsed_ms,
        );
        lock(&shared.runs).insert(meta.run_id.clone(), core.clone());
        core.emit(WorkflowEventKind::RunCreated {
            name: meta.name.clone(),
            chat_id: meta.chat_id.clone(),
            script_hash: meta.script_hash.clone(),
            resumed_from: meta.resumed_from.clone(),
            graph: Some(analysis.graph.clone()),
            concurrency_ceiling: meta.options.max_concurrency,
            saved: meta.saved.clone(),
        });
        let auto = shared
            .sessions
            .last_request(&meta.chat_id)
            .is_some_and(|r| r.auto_approve);
        if !auto && !user_initiated {
            let question = approval_question(&meta, &analysis.graph, &script, &defaults);
            let approver = lock(&shared.approver).clone();
            if let Approval::Denied(reason) = approver.approve(&meta.chat_id, question).await {
                self.settle_unlaunched(&core, &reason);
                return Err(StartError::Denied(reason));
            }
        }
        core.mark_launched();
        let this = self.clone();
        let args = meta.args.clone();
        tokio::spawn(async move { this.execute(core, script, analysis, args).await });
        Ok(())
    }

    /// A run that never launched (denied, approval lost): settle it without
    /// a result message — the start call already told the caller.
    fn settle_unlaunched(&self, core: &Arc<RunCore>, detail: &str) {
        core.emit(WorkflowEventKind::RunSettled {
            status: WorkflowStatus::Stopped,
            stop_reason: Some(WorkflowStopReason::Denied),
            stop_detail: Some(detail.to_owned()),
            error: None,
            result_preview: None,
            result_truncated: false,
            resumable: false,
        });
        core.update_meta(|m| {
            m.status = WorkflowStatus::Stopped;
            m.stop_reason = Some(WorkflowStopReason::Denied);
            m.delivered = true;
        });
        lock(&self.shared.runs).remove(&core.run_id);
        self.shared.projection.flush(&core.chat_id);
        let _ = self.shared.doc_host.push_system_marker(
            &core.chat_id,
            &format!("workflow-{}-end", core.run_id),
            &zeron_proto::workflow_marker_text(WorkflowEventMarker::Denied, &core.name, detail),
            MessageOrigin::WorkflowEvent {
                run_id: core.run_id.clone(),
                marker: WorkflowEventMarker::Denied,
                name: core.name.clone(),
                detail: detail.to_owned(),
            },
        );
    }

    // ── executing ──────────────────────────────────────────────────────────

    async fn execute(&self, core: Arc<RunCore>, script: String, analysis: Analysis, args: Value) {
        let meta = core.meta();
        core.emit(WorkflowEventKind::RunLaunched {
            phase_names: analysis.graph.phase_names(),
        });
        core.update_meta(|m| m.status = WorkflowStatus::Running);
        let marker = if meta.resumed_from.is_some() {
            WorkflowEventMarker::Resumed
        } else {
            WorkflowEventMarker::Started
        };
        let _ = self.shared.doc_host.push_system_marker(
            &core.chat_id,
            &format!("workflow-{}-start", core.run_id),
            &zeron_proto::workflow_marker_text(marker, &core.name, ""),
            MessageOrigin::WorkflowEvent {
                run_id: core.run_id.clone(),
                marker,
                name: core.name.clone(),
                detail: String::new(),
            },
        );
        let budget_timer = core.begin_budget_timer();
        let stall_every = lock(&self.shared.tuning).stall_check;
        let watchdog = {
            let core = core.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(stall_every);
                tick.tick().await;
                loop {
                    tick.tick().await;
                    core.check_stall();
                }
            })
        };
        let host: Arc<dyn zeron_workflow::host::Host> = core.clone();
        let flag = core.flag();
        let outcome = tokio::task::spawn_blocking(move || {
            run_script(
                "workflow.star",
                &script,
                &args,
                &analysis,
                host,
                flag,
                &Limits::default(),
            )
        })
        .await;
        watchdog.abort();
        if let Some(t) = budget_timer {
            t.abort();
        }
        // Leftover asks (handles the script never joined) are dropped.
        core.token.cancel();
        let idle_wait = lock(&self.shared.tuning).idle_wait;
        core.wait_idle(idle_wait).await;
        self.finish(&core, outcome).await;
    }

    async fn finish(
        &self,
        core: &Arc<RunCore>,
        outcome: Result<Result<Value, RunError>, tokio::task::JoinError>,
    ) {
        let stop = core.stop_info();
        let interrupted = || StopInfo {
            reason: WorkflowStopReason::Interrupted,
            detail: "the workflow was cancelled".into(),
        };
        let (status, stop_reason, stop_detail, error, result) = match outcome {
            Ok(Ok(value)) => (WorkflowStatus::Completed, None, None, None, Some(value)),
            Ok(Err(RunError::Cancelled)) | Ok(Err(RunError::Limit(_))) if stop.is_some() => {
                let s = stop.clone().unwrap_or_else(interrupted);
                (
                    WorkflowStatus::Stopped,
                    Some(s.reason),
                    Some(s.detail),
                    None,
                    None,
                )
            }
            Ok(Err(RunError::Cancelled)) => {
                let s = interrupted();
                (
                    WorkflowStatus::Stopped,
                    Some(s.reason),
                    Some(s.detail),
                    None,
                    None,
                )
            }
            Ok(Err(e)) => (
                WorkflowStatus::Errored,
                None,
                None,
                Some(e.to_string()),
                None,
            ),
            Err(e) => (
                WorkflowStatus::Errored,
                None,
                None,
                Some(format!("the interpreter failed: {e}")),
                None,
            ),
        };
        let resumable = status == WorkflowStatus::Stopped
            && !matches!(stop_reason, Some(WorkflowStopReason::Denied));
        let preview_chars = zeron_proto::WORKFLOW_PREVIEW_BYTES / 2;
        let (result_preview, result_truncated) = match &result {
            Some(v) => {
                let text = match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (
                    Some(text.chars().take(preview_chars).collect::<String>()),
                    text.chars().count() > preview_chars,
                )
            }
            None => (None, false),
        };
        if let Some(v) = &result {
            core.result_record(v);
        }
        core.emit_usage_now();
        core.emit(WorkflowEventKind::RunSettled {
            status,
            stop_reason,
            stop_detail,
            error,
            result_preview,
            result_truncated,
            resumable,
        });
        core.update_meta(|m| {
            m.status = status;
            m.stop_reason = stop_reason;
        });
        self.shared.projection.flush(&core.chat_id);
        // Queue the result BEFORE the run stops counting as running: a goal
        // controller watching this chat must never see a gap (no workflow, no
        // queued message) in which it could verify.
        if let Some(run) = self.shared.projection.run(&core.chat_id, &core.run_id) {
            self.deliver(&core.run_id, &core.chat_id, &run, result.as_ref());
        }
        lock(&self.shared.runs).remove(&core.run_id);
        self.shared.doc_host.goal_nudge(&core.chat_id).await;
    }

    /// Queue the completion message and the end marker, once.
    fn deliver(&self, run_id: &str, chat_id: &str, run: &WorkflowRun, result: Option<&Value>) {
        let Ok(mut meta) = self.shared.store.read_meta(run_id) else {
            return;
        };
        if meta.delivered {
            return;
        }
        let h = &run.header;
        let marker = match (h.status, h.stop_reason) {
            (WorkflowStatus::Completed, _) => WorkflowEventMarker::Completed,
            (WorkflowStatus::Errored, _) => WorkflowEventMarker::Errored,
            (_, Some(WorkflowStopReason::Denied)) => WorkflowEventMarker::Denied,
            _ => WorkflowEventMarker::Stopped,
        };
        let mut detail = prompts::summary_line(run);
        if let Some(why) = h.stop_detail.as_deref().or(h.error.as_deref()) {
            detail.push_str(" — ");
            detail.push_str(why);
        }
        let _ = self.shared.doc_host.push_system_marker(
            chat_id,
            &format!("workflow-{run_id}-end"),
            &zeron_proto::workflow_marker_text(marker, &h.name, &detail),
            MessageOrigin::WorkflowEvent {
                run_id: run_id.to_owned(),
                marker,
                name: h.name.clone(),
                detail,
            },
        );
        let text = prompts::completion_message(run, result);
        match self.shared.doc_host.enqueue_machine_message(
            chat_id,
            &format!("workflow-{run_id}-done"),
            &text,
            MessageOrigin::Workflow {
                run_id: run_id.to_owned(),
                name: h.name.clone(),
                status: h.status,
            },
        ) {
            Ok(_) => {
                meta.delivered = true;
                let _ = self.shared.store.write_meta(&meta);
            }
            Err(err) => tracing::warn!(run = %run_id, error = %err, "workflow result not queued"),
        }
    }

    // ── controlling ────────────────────────────────────────────────────────

    /// Stop a running workflow. `Ok(true)` when it was running.
    pub fn stop(&self, run_id: &str, reason: Option<&str>) -> Result<bool, WorkflowError> {
        if let Some(core) = lock(&self.shared.runs).get(run_id).cloned() {
            core.request_stop(
                WorkflowStopReason::User,
                reason.unwrap_or("stopped by the user").to_owned(),
            );
            return Ok(true);
        }
        let meta = self
            .shared
            .store
            .read_meta(run_id)
            .map_err(|_| WorkflowError::NoRun(run_id.to_owned()))?;
        if matches!(
            meta.status,
            WorkflowStatus::Pending | WorkflowStatus::Running
        ) {
            // An orphan: nothing is executing it.
            self.reconcile_run(&meta);
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn resolve_question(
        &self,
        run_id: &str,
        qid: &str,
        answer: &str,
    ) -> Result<(), WorkflowError> {
        let core = lock(&self.shared.runs)
            .get(run_id)
            .cloned()
            .ok_or_else(|| WorkflowError::NoRun(run_id.to_owned()))?;
        if answer.trim().is_empty() {
            return Err(WorkflowError::Invalid("the answer is empty".into()));
        }
        if core.resolve_question(qid, answer).await {
            Ok(())
        } else {
            Err(WorkflowError::Invalid(format!(
                "no question {qid} is waiting in this run"
            )))
        }
    }

    /// Continue a stopped run from its journal: answers already given are
    /// replayed, not asked again. Returns the new run.
    ///
    /// `by_user`: a person asked for it (the command plane) — their click is
    /// the approval; an agent's resume asks like a start does.
    pub async fn resume(
        &self,
        run_id: &str,
        args: Option<Value>,
        by_user: bool,
    ) -> Result<StartOutcome, StartError> {
        let store = &self.shared.store;
        let old = store
            .read_meta(run_id)
            .map_err(|_| StartError::Invalid(format!("no such workflow run: {run_id}")))?;
        let resumable = old.status == WorkflowStatus::Stopped
            && !matches!(old.stop_reason, Some(WorkflowStopReason::Denied));
        if !resumable {
            return Err(StartError::Invalid(format!(
                "run {run_id} is {:?} and cannot be resumed (only stopped runs can)",
                old.status
            )));
        }
        if lock(&self.shared.runs)
            .values()
            .any(|c| c.meta().resumed_from.as_deref() == Some(run_id))
        {
            return Err(StartError::Invalid(format!(
                "run {run_id} is already being resumed"
            )));
        }
        let script = store
            .read_script(run_id)
            .map_err(|e| StartError::Invalid(e.to_string()))?;
        if sha_hex(script.as_bytes()) != old.script_hash {
            return Err(StartError::Invalid(
                "the stored script no longer matches its hash; refusing to resume".into(),
            ));
        }
        if let Some(args) = &args {
            let hash = sha_hex(serde_json::to_string(args).unwrap_or_default().as_bytes());
            if hash != old.args_hash {
                return Err(StartError::Invalid(
                    "the arguments differ from the original run's; a resume replays the same inputs (start a new run instead)".into(),
                ));
            }
        }
        let analysis = analyze("workflow.star", &script).map_err(StartError::Diagnostics)?;
        let replay = store
            .load_replay(run_id)
            .map_err(|e| StartError::Invalid(e.to_string()))?;
        let elapsed = self
            .shared
            .projection
            .run(&old.chat_id, run_id)
            .map(|r| r.header.usage.elapsed_ms)
            .unwrap_or(0);
        let defaults = defaults_for(&self.shared.workspace, &old.chat_id);
        let meta = RunMeta {
            run_id: new_id(),
            resumed_from: Some(run_id.to_owned()),
            created_at: crate::now_ms(),
            status: WorkflowStatus::Pending,
            stop_reason: None,
            delivered: false,
            last_seq: 0,
            ..old
        };
        let outcome = StartOutcome {
            run_id: meta.run_id.clone(),
            name: meta.name.clone(),
            graph: analysis.graph.clone(),
            warnings: analysis.warnings.clone(),
            max_concurrency: meta.options.max_concurrency,
            draft_path: meta.draft_path.clone(),
        };
        self.begin(meta, script, analysis, defaults, replay, elapsed, by_user)
            .await?;
        Ok(outcome)
    }

    // ── reading ────────────────────────────────────────────────────────────

    pub fn list(&self, chat_id: Option<&str>) -> Vec<WorkflowRunHeader> {
        let mut out = Vec::new();
        for meta in self.shared.store.list_metas() {
            if chat_id.is_some_and(|c| c != meta.chat_id) {
                continue;
            }
            let mut header = self
                .shared
                .projection
                .run(&meta.chat_id, &meta.run_id)
                .map(|r| r.header)
                .unwrap_or_else(|| WorkflowRunHeader {
                    run_id: meta.run_id.clone(),
                    name: meta.name.clone(),
                    chat_id: meta.chat_id.clone(),
                    status: meta.status,
                    stop_reason: meta.stop_reason,
                    resumed_from: meta.resumed_from.clone(),
                    saved_name: meta.saved.as_ref().map(|s| s.name.clone()),
                    saved_scope: meta.saved.as_ref().map(|s| s.scope),
                    script_hash: meta.script_hash.clone(),
                    created_at: meta.created_at,
                    ..WorkflowRunHeader::default()
                });
            // The store is written when a run settles; the doc projection is
            // batched and can lag behind an abrupt exit. A settled run is settled.
            if meta.status.is_settled() && !header.status.is_settled() {
                header.status = meta.status;
                header.stop_reason = meta.stop_reason;
            }
            out.push(header);
        }
        out
    }

    pub fn get(&self, run_id: &str) -> Result<RunView, WorkflowError> {
        let meta = self
            .shared
            .store
            .read_meta(run_id)
            .map_err(|_| WorkflowError::NoRun(run_id.to_owned()))?;
        let replay = self.shared.store.load_replay(run_id)?;
        let run = self
            .shared
            .projection
            .run(&meta.chat_id, run_id)
            .or_else(|| rebuild(&replay))
            .ok_or_else(|| WorkflowError::NoRun(run_id.to_owned()))?;
        Ok(RunView {
            run,
            args: meta.args.clone(),
            saved: meta.saved.clone(),
            result: replay.result,
            reports: replay
                .reports
                .into_iter()
                .map(|(_, item, _)| item)
                .collect(),
        })
    }

    pub fn artifact_index(&self, run_id: &str, id: &str) -> Result<ArtifactIndex, WorkflowError> {
        Ok(self.shared.store.artifact_index(run_id, id)?)
    }

    pub fn artifact_read(
        &self,
        run_id: &str,
        id: &str,
        version: Option<u32>,
        offset: u64,
        limit: u64,
    ) -> Result<ArtifactChunk, WorkflowError> {
        Ok(self
            .shared
            .store
            .read_artifact(run_id, id, version, offset, limit)?)
    }

    // ── restart ────────────────────────────────────────────────────────────

    /// Runs the journal says were running with nobody executing them (the
    /// engine restarted) become `stopped(interrupted)`, resumable. Runs that
    /// settled but never delivered their result deliver it now.
    pub fn reconcile(&self) {
        for meta in self.shared.store.list_metas() {
            if lock(&self.shared.runs).contains_key(&meta.run_id) {
                continue;
            }
            match meta.status {
                WorkflowStatus::Pending | WorkflowStatus::Running => self.reconcile_run(&meta),
                _ if !meta.delivered && meta.stop_reason != Some(WorkflowStopReason::Denied) => {
                    if let Ok(view) = self.get(&meta.run_id) {
                        self.deliver(&meta.run_id, &meta.chat_id, &view.run, view.result.as_ref());
                    }
                }
                _ => {}
            }
        }
    }

    fn reconcile_run(&self, meta: &RunMeta) {
        let Ok(replay) = self.shared.store.load_replay(&meta.run_id) else {
            return;
        };
        let was_running = meta.status == WorkflowStatus::Running;
        let mut state = WorkflowRunsState::default();
        for e in &replay.events {
            zeron_workflow::reduce(&mut state, e);
        }
        let reason = if was_running {
            WorkflowStopReason::Interrupted
        } else {
            WorkflowStopReason::Denied
        };
        let event = WorkflowEvent {
            run_id: meta.run_id.clone(),
            seq: replay.last_seq + 1,
            at: crate::now_ms(),
            kind: WorkflowEventKind::RunSettled {
                status: WorkflowStatus::Stopped,
                stop_reason: Some(reason),
                stop_detail: Some(
                    if was_running {
                        "the engine restarted while the workflow was running"
                    } else {
                        "the engine restarted before the workflow was approved"
                    }
                    .into(),
                ),
                error: None,
                result_preview: None,
                result_truncated: false,
                resumable: was_running,
            },
        };
        if let Ok(journal) = self.shared.store.open_journal(&meta.run_id) {
            let _ = journal.append(&store::Record::Event(event.clone()));
        }
        zeron_workflow::reduce(&mut state, &event);
        let mut updated = meta.clone();
        updated.status = WorkflowStatus::Stopped;
        updated.stop_reason = Some(reason);
        updated.last_seq = event.seq;
        if !was_running {
            updated.delivered = true;
        }
        let _ = self.shared.store.write_meta(&updated);
        let Some(run) = state.runs.into_iter().next() else {
            return;
        };
        self.shared.projection.replace_run(&meta.chat_id, &run);
        let _ = self.shared.events_tx.send(event);
        if was_running {
            self.deliver(&meta.run_id, &meta.chat_id, &run, None);
        }
    }
}

/// A run's state rebuilt by folding its journaled events.
fn rebuild(replay: &store::Replay) -> Option<WorkflowRun> {
    let mut state = WorkflowRunsState::default();
    for e in &replay.events {
        zeron_workflow::reduce(&mut state, e);
    }
    state.runs.into_iter().next()
}

fn saved_script_body(script: &str) -> &str {
    zeron_workflow::saved::strip_frontmatter(script)
}

fn approval_question(
    meta: &RunMeta,
    graph: &WorkflowGraph,
    script: &str,
    defaults: &Defaults,
) -> UserInputQuestion {
    let harness = meta.options.harness.clone().or_else(|| {
        serde_json::to_value(defaults.harness)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
    });
    let model = meta
        .options
        .model
        .clone()
        .or_else(|| defaults.model.clone());
    // A saved file opens with its frontmatter, which the approval already
    // shows as the name, the arguments and the limits.
    let shown = if meta.saved.is_some() {
        saved_script_body(script)
    } else {
        script
    };
    let text = prompts::approval_text(&prompts::ApprovalFacts {
        name: &meta.name,
        graph,
        max_concurrency: meta.options.max_concurrency,
        harness: harness.as_deref(),
        model: model.as_deref(),
        budgets: &meta.options.budgets,
        script_hash: &meta.script_hash,
        draft_path: meta.draft_path.as_deref(),
        script: shown,
        saved: meta.saved.as_ref(),
        args: &meta.args,
    });
    let approval = WorkflowApprovalMeta {
        run_id: meta.run_id.clone(),
        name: meta.name.clone(),
        script_hash: meta.script_hash.clone(),
        draft_path: meta.draft_path.clone(),
        graph: graph.clone(),
        max_concurrency: meta.options.max_concurrency,
        budgets: meta.options.budgets.clone(),
        harness,
        model,
        excerpt: prompts::excerpt(shown, 40),
        saved: meta.saved.clone(),
        args: match &meta.args {
            Value::Object(o) if !o.is_empty() => meta.args.clone(),
            _ => Value::Null,
        },
    };
    let mut meta_json = serde_json::to_value(&approval).unwrap_or(Value::Null);
    if let Some(o) = meta_json.as_object_mut() {
        o.insert("kind".into(), json!(WORKFLOW_APPROVAL_META_KIND));
    }
    UserInputQuestion {
        id: "workflow-approval".into(),
        header: if meta.resumed_from.is_some() {
            "Resume workflow".into()
        } else {
            "Run workflow".into()
        },
        question: text,
        options: vec![APPROVE_LABEL.into(), DENY_LABEL.into()],
        multi_select: false,
        prefill: None,
        multiline: false,
        meta: Some(meta_json),
    }
}
