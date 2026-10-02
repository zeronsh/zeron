//! One running workflow: the [`Host`] a script talks to, and the scheduler
//! behind it.
//!
//! * **Journal first.** Every effect is keyed by call site and journaled; a
//!   resumed run answers from the previous run's journal instead of asking
//!   again (`replay`).
//! * **Per-actor FIFO.** An actor is one persistent child chat. Its asks queue
//!   in the order the script dispatched them and a single pump runs them one
//!   at a time, so they share context in script order.
//! * **Bounded.** A per-model gate (cap moved by the AIMD governor) then the
//!   run's global gate (`max_concurrency`) admit asks; commands have their own
//!   small gate. The script cannot exceed either.
//! * **Provider faults never reach the script.** Rate limits and transient
//!   errors are redriven with backoff; auth / quota / model errors stop the
//!   run (`stopped(provider)`, resumable). Only an ask's own failure (bad
//!   result, timeout, a question nobody can answer) is a value.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use zeron_proto::{
    ArtifactKind, ArtifactSummary, HarnessId, NodeKind, NodeOutcome, ReasoningLevel,
    WorkflowActorTag, WorkflowConcurrency, WorkflowEvent, WorkflowEventKind, WorkflowQuestion,
    WorkflowStopReason, WorkflowUsage,
};
use zeron_workflow::host::{
    ActorSpec, ArtifactContent, ArtifactRequest, AskReply, AskRequest, Completion, Host, ReadOp,
    RunReply, RunRequest, completed, completion,
};
use zeron_workflow::limits::MAX_ARTIFACT_BYTES;
use zeron_workflow::site::SiteKey;

use super::Shared;
use super::faults::{Fault, backoff, classify};
use super::governor::{Gate, Governor, Permit};
use super::prompts;
use super::store::{Journal, Record, Replay, RunMeta, confine};
use super::world::World;
use crate::ask::{AskError, AskProgress, AskSpec, EscalationHook, EscalationRaised, ProgressHook};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Default time one actor ask may take, unless the script sets `timeout_s`.
pub const DEFAULT_ASK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Consecutive redrives of one ask before a "transient" error is treated as
/// a deterministic provider failure (about half an hour at the 60 s cap).
pub const MAX_REDRIVES: u32 = 30;
/// Commands running at once per run.
const COMMAND_SLOTS: u32 = 4;
/// A run with no successful ask for this long (and work pending) is stalled.
pub const STALL_AFTER: Duration = Duration::from_secs(20 * 60);

/// Why the run was told to stop.
#[derive(Debug, Clone)]
pub struct StopInfo {
    pub reason: WorkflowStopReason,
    pub detail: String,
}

struct Job {
    req: AskRequest,
    digest: String,
    completer: zeron_workflow::host::Completer<AskReply>,
}

struct Resolved {
    harness: HarnessId,
    model: Option<String>,
    reasoning: Option<ReasoningLevel>,
}

pub(super) struct Actor {
    key: SiteKey,
    name: String,
    spec: ActorSpec,
    queue: Mutex<(VecDeque<Job>, bool)>,
    child: Mutex<Option<String>>,
    introduced: AtomicBool,
    resolved: Mutex<Option<Result<Arc<Resolved>, String>>>,
}

struct Lane {
    governor: Mutex<Governor>,
    gate: Arc<Gate>,
}

/// Defaults for asks that do not pick a harness or model.
#[derive(Debug, Clone)]
pub struct Defaults {
    pub harness: HarnessId,
    pub model: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
}

pub struct RunCore {
    shared: Arc<Shared>,
    me: Weak<RunCore>,
    pub run_id: String,
    pub chat_id: String,
    pub name: String,
    meta: Mutex<RunMeta>,
    journal: Journal,
    replay: Replay,
    emit_lock: Mutex<u64>,
    pub token: CancellationToken,
    flag: Arc<AtomicBool>,
    rt: tokio::runtime::Handle,
    world: World,
    defaults: Defaults,
    phase: Mutex<Option<String>>,
    actors: Mutex<HashMap<String, Arc<Actor>>>,
    lanes: Mutex<HashMap<String, Arc<Lane>>>,
    global: Arc<Gate>,
    commands: Arc<Gate>,
    waiting: AtomicU32,
    pumps: AtomicUsize,
    idle: tokio::sync::Notify,
    stop: Mutex<Option<StopInfo>>,
    usage: Mutex<WorkflowUsage>,
    asks_started: AtomicU32,
    reports: AtomicU32,
    artifacts: Mutex<HashMap<String, ArtifactSummary>>,
    started: Instant,
    base_elapsed_ms: u64,
    last_success: Mutex<Instant>,
    stalled: AtomicBool,
    launched: AtomicBool,
    questions: Mutex<HashMap<String, (String, SiteKey)>>,
    last_concurrency: Mutex<Option<WorkflowConcurrency>>,
}

impl RunCore {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        shared: Arc<Shared>,
        meta: RunMeta,
        journal: Journal,
        replay: Replay,
        world: World,
        defaults: Defaults,
        base_elapsed_ms: u64,
    ) -> Arc<Self> {
        let ceiling = meta.options.max_concurrency.max(1);
        Arc::new_cyclic(|me| Self {
            shared,
            me: me.clone(),
            run_id: meta.run_id.clone(),
            chat_id: meta.chat_id.clone(),
            name: meta.name.clone(),
            meta: Mutex::new(meta),
            journal,
            replay,
            emit_lock: Mutex::new(0),
            token: CancellationToken::new(),
            flag: Arc::new(AtomicBool::new(false)),
            rt: tokio::runtime::Handle::current(),
            world,
            defaults,
            phase: Mutex::new(None),
            actors: Mutex::default(),
            lanes: Mutex::default(),
            global: Gate::new(ceiling),
            commands: Gate::new(COMMAND_SLOTS),
            waiting: AtomicU32::new(0),
            pumps: AtomicUsize::new(0),
            idle: tokio::sync::Notify::new(),
            stop: Mutex::new(None),
            usage: Mutex::new(WorkflowUsage::default()),
            asks_started: AtomicU32::new(0),
            reports: AtomicU32::new(0),
            artifacts: Mutex::default(),
            started: Instant::now(),
            base_elapsed_ms,
            last_success: Mutex::new(Instant::now()),
            stalled: AtomicBool::new(false),
            launched: AtomicBool::new(false),
            questions: Mutex::default(),
            last_concurrency: Mutex::new(None),
        })
    }

    pub(super) fn mark_launched(&self) {
        self.launched.store(true, Ordering::SeqCst);
    }

    /// Approved and executing (not merely created).
    pub(super) fn is_launched(&self) -> bool {
        self.launched.load(Ordering::SeqCst)
    }

    pub(super) fn emit_usage_now(&self) {
        self.emit_usage();
    }

    fn me(&self) -> Arc<RunCore> {
        self.me.upgrade().expect("a run core outlives its calls")
    }

    pub(super) fn flag(&self) -> Arc<AtomicBool> {
        self.flag.clone()
    }

    pub(super) fn meta(&self) -> RunMeta {
        lock(&self.meta).clone()
    }

    pub(super) fn update_meta(&self, f: impl FnOnce(&mut RunMeta)) {
        let mut meta = lock(&self.meta);
        f(&mut meta);
        if let Err(err) = self.shared.store.write_meta(&meta) {
            tracing::warn!(run = %self.run_id, error = %err, "run meta write failed");
        }
    }

    pub(super) fn stop_info(&self) -> Option<StopInfo> {
        lock(&self.stop).clone()
    }

    pub(super) fn elapsed_ms(&self) -> u64 {
        self.base_elapsed_ms + self.started.elapsed().as_millis() as u64
    }

    pub(super) fn usage(&self) -> WorkflowUsage {
        let mut usage = lock(&self.usage).clone();
        usage.elapsed_ms = self.elapsed_ms();
        usage
    }

    pub(super) fn result_record(&self, value: &Value) {
        let _ = self.journal.append(&Record::Result {
            value: value.clone(),
        });
    }

    // ── events ─────────────────────────────────────────────────────────────

    /// Journal, project and publish one event. Serialized, so sequence,
    /// journal order and reducer order always agree.
    pub(super) fn emit(&self, kind: WorkflowEventKind) {
        let mut seq = lock(&self.emit_lock);
        *seq += 1;
        let event = WorkflowEvent {
            run_id: self.run_id.clone(),
            seq: *seq,
            at: crate::now_ms(),
            kind,
        };
        if let Err(err) = self.journal.append(&Record::Event(event.clone())) {
            tracing::warn!(run = %self.run_id, error = %err, "journal write failed");
        }
        self.shared.projection.apply(&self.chat_id, &event);
        let _ = self.shared.events_tx.send(event);
        lock(&self.meta).last_seq = *seq;
    }

    fn emit_usage(&self) {
        self.emit(WorkflowEventKind::UsageUpdated {
            usage: self.usage(),
        });
    }

    fn concurrency_changed(&self) {
        let throttled = lock(&self.lanes)
            .values()
            .any(|l| lock(&l.governor).is_throttled());
        let now = WorkflowConcurrency {
            cap: self.global.cap(),
            ceiling: self.meta().options.max_concurrency,
            in_flight: self.global.in_flight(),
            queued: self.waiting.load(Ordering::Relaxed),
            throttled,
        };
        let mut last = lock(&self.last_concurrency);
        if last.as_ref() != Some(&now) {
            *last = Some(now.clone());
            drop(last);
            self.emit(WorkflowEventKind::ConcurrencyChanged { concurrency: now });
        }
    }

    // ── stopping ───────────────────────────────────────────────────────────

    /// First reason wins; cancels the script and every pending ask.
    pub(super) fn request_stop(&self, reason: WorkflowStopReason, detail: impl Into<String>) {
        {
            let mut stop = lock(&self.stop);
            if stop.is_some() {
                return;
            }
            *stop = Some(StopInfo {
                reason,
                detail: detail.into(),
            });
        }
        self.flag.store(true, Ordering::SeqCst);
        self.token.cancel();
    }

    /// Wait (bounded) for every actor pump to wind down.
    pub(super) async fn wait_idle(&self, within: Duration) {
        let deadline = Instant::now() + within;
        while self.pumps.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            let _ = tokio::time::timeout(Duration::from_millis(50), self.idle.notified()).await;
        }
    }

    pub(super) fn has_work_pending(&self) -> bool {
        self.pumps.load(Ordering::SeqCst) > 0
    }

    // ── watchdogs ──────────────────────────────────────────────────────────

    /// Called periodically: raise the stall notice once, lift it on success.
    pub(super) fn check_stall(&self) {
        let quiet = lock(&self.last_success).elapsed();
        if quiet >= STALL_AFTER
            && self.has_work_pending()
            && !self.stalled.swap(true, Ordering::SeqCst)
        {
            self.emit(WorkflowEventKind::Stalled);
        }
    }

    fn note_success(&self) {
        *lock(&self.last_success) = Instant::now();
        if self.stalled.swap(false, Ordering::SeqCst) {
            self.emit(WorkflowEventKind::Unstalled);
        }
    }

    // ── actors and lanes ───────────────────────────────────────────────────

    fn lane_for(&self, resolved: &Resolved) -> Arc<Lane> {
        let key = format!(
            "{:?}/{}",
            resolved.harness,
            resolved.model.as_deref().unwrap_or("-")
        );
        let ceiling = self.meta().options.max_concurrency.max(1);
        lock(&self.lanes)
            .entry(key)
            .or_insert_with(|| {
                Arc::new(Lane {
                    governor: Mutex::new(Governor::new(ceiling)),
                    gate: Gate::new(ceiling),
                })
            })
            .clone()
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    async fn resolve(&self, actor: &Actor) -> Result<Arc<Resolved>, String> {
        if let Some(done) = lock(&actor.resolved).clone() {
            return done;
        }
        let out = self.resolve_uncached(actor).await.map(Arc::new);
        *lock(&actor.resolved) = Some(out.clone());
        out
    }

    async fn resolve_uncached(&self, actor: &Actor) -> Result<Resolved, String> {
        let spec = &actor.spec;
        let harness = match spec
            .harness
            .as_deref()
            .or(self.meta().options.harness.as_deref())
        {
            Some(name) => parse_harness(name)?,
            None => self.defaults.harness,
        };
        let model = spec
            .model
            .clone()
            .or_else(|| self.meta().options.model.clone())
            .or_else(|| {
                (harness == self.defaults.harness)
                    .then(|| self.defaults.model.clone())
                    .flatten()
            });
        let reasoning = match spec
            .reasoning
            .as_deref()
            .or(self.meta().options.reasoning.as_deref())
        {
            Some(r) => Some(parse_reasoning(r)?),
            None => (harness == self.defaults.harness)
                .then_some(self.defaults.reasoning)
                .flatten(),
        };
        let catalog = lock(&self.shared.catalog).clone();
        catalog.check(harness, model.as_deref()).await?;
        Ok(Resolved {
            harness,
            model,
            reasoning,
        })
    }

    // ── scheduling ─────────────────────────────────────────────────────────

    async fn pump(self: Arc<Self>, actor: Arc<Actor>) {
        self.pumps.fetch_add(1, Ordering::SeqCst);
        loop {
            let job = {
                let mut q = lock(&actor.queue);
                match q.0.pop_front() {
                    Some(job) => job,
                    None => {
                        q.1 = false; // pumping stops under the same lock that pushes check
                        break;
                    }
                }
            };
            self.process(&actor, job).await;
        }
        self.pumps.fetch_sub(1, Ordering::SeqCst);
        self.idle.notify_waiters();
    }

    /// Wait for a slot in the lane, then in the run; `None` when cancelled.
    async fn admit(&self, lane: &Lane) -> Option<(Permit, Permit)> {
        let lane_permit = tokio::select! {
            _ = self.token.cancelled() => return None,
            p = lane.gate.acquire() => p,
        };
        let global_permit = tokio::select! {
            _ = self.token.cancelled() => return None,
            p = self.global.acquire() => p,
        };
        Some((lane_permit, global_permit))
    }

    fn settle_ask(
        &self,
        job: Job,
        reply: AskReply,
        outcome: NodeOutcome,
        journal: bool,
        child: Option<String>,
        tokens: (u64, u64),
    ) {
        let (site_id, ordinal) = (job.req.key.site.clone(), job.req.key.ordinal);
        if journal {
            let _ = self.journal.append(&Record::AskDone {
                key: job.req.key.to_string(),
                digest: job.digest.clone(),
                ok: reply.ok,
                value: reply.value.clone(),
                error: reply.error.clone(),
                input_tokens: tokens.0,
                output_tokens: tokens.1,
                child_chat_id: child,
            });
        }
        {
            let mut usage = lock(&self.usage);
            usage.input_tokens += tokens.0;
            usage.output_tokens += tokens.1;
            if outcome != NodeOutcome::Cancelled {
                usage.nodes_used += 1;
            }
        }
        self.emit(WorkflowEventKind::NodeSettled {
            site_id,
            ordinal,
            outcome,
            cached: reply.cached,
            tokens: tokens.0 + tokens.1,
            error: reply.error.clone(),
            result_preview: reply.ok.then(|| preview(&reply.value)),
        });
        self.emit_usage();
        job.completer.complete(reply);
        if let Some(max) = self.meta().options.budgets.max_tokens
            && self.usage().total_tokens() >= max
        {
            self.request_stop(
                WorkflowStopReason::Budget,
                format!("the token budget of {max} was reached"),
            );
        }
    }

    /// The ask never produced a result (the run is stopping): record the
    /// node as cancelled and let the script's wait see an abandoned slot.
    fn settle_cancelled(&self, job: Job) {
        self.emit(WorkflowEventKind::NodeSettled {
            site_id: job.req.key.site.clone(),
            ordinal: job.req.key.ordinal,
            outcome: NodeOutcome::Cancelled,
            cached: false,
            tokens: 0,
            error: None,
            result_preview: None,
        });
        drop(job.completer);
    }

    async fn process(self: &Arc<Self>, actor: &Arc<Actor>, job: Job) {
        self.waiting.fetch_sub(1, Ordering::Relaxed);
        let resolved = match self.resolve(actor).await {
            Ok(r) => r,
            Err(message) => {
                self.settle_ask(
                    job,
                    AskReply::failed(message),
                    NodeOutcome::Failed,
                    true,
                    None,
                    (0, 0),
                );
                return;
            }
        };
        let lane = self.lane_for(&resolved);
        let Some(backend) = self.shared.doc_host.ask_backend() else {
            self.settle_ask(
                job,
                AskReply::failed("asks are not available in this engine"),
                NodeOutcome::Failed,
                true,
                None,
                (0, 0),
            );
            return;
        };
        let (site, ordinal) = (job.req.key.site.clone(), job.req.key.ordinal);
        let salt = u64::from_le_bytes(
            Sha256::digest(job.req.key.to_string().as_bytes())[..8]
                .try_into()
                .unwrap_or([0; 8]),
        );
        let mut attempt = 0u32;
        let mut dispatched = false;
        loop {
            if self.token.is_cancelled() {
                self.settle_cancelled(job);
                return;
            }
            let cooldown = lock(&lane.governor).cooldown_remaining_ms(self.now_ms());
            if cooldown > 0 {
                tokio::select! {
                    _ = self.token.cancelled() => {}
                    _ = tokio::time::sleep(Duration::from_millis(cooldown)) => {}
                }
                continue;
            }
            let Some(permits) = self.admit(&lane).await else {
                self.settle_cancelled(job);
                return;
            };
            self.concurrency_changed();
            if !dispatched {
                dispatched = true;
                self.emit(WorkflowEventKind::NodeDispatched {
                    site_id: site.clone(),
                    ordinal,
                });
            }
            let spec = self.build_spec(actor, &job, &resolved);
            let result = backend.ask(&self.chat_id, spec, self.token.clone()).await;
            drop(permits);
            self.concurrency_changed();
            match result {
                Ok(outcome) => {
                    lock(&actor.child).get_or_insert(outcome.child_chat_id.clone());
                    {
                        let mut g = lock(&lane.governor);
                        if g.on_success(self.now_ms()) {
                            lane.gate.set_cap(g.cap(self.now_ms()));
                        }
                    }
                    self.note_success();
                    let value = if job.req.schema.is_some() {
                        outcome.result.clone()
                    } else {
                        outcome.result.get("text").cloned().unwrap_or(Value::Null)
                    };
                    let mut reply = AskReply::ok(value);
                    reply.tokens = outcome.usage.total_tokens();
                    let tokens = (outcome.usage.input_tokens, outcome.usage.output_tokens);
                    self.settle_ask(
                        job,
                        reply,
                        NodeOutcome::Ok,
                        true,
                        Some(outcome.child_chat_id),
                        tokens,
                    );
                    return;
                }
                Err(failure) => {
                    if let Some(child) = &failure.child_chat_id {
                        lock(&actor.child).get_or_insert(child.clone());
                    }
                    let tokens = (failure.usage.input_tokens, failure.usage.output_tokens);
                    match &failure.error {
                        AskError::Cancelled => {
                            self.settle_cancelled(job);
                            return;
                        }
                        AskError::TurnFailed(message) => {
                            let fault = classify(message);
                            if fault.is_deterministic() {
                                self.request_stop(
                                    WorkflowStopReason::Provider,
                                    format!("{}: {}", fault.label(), one_line(message)),
                                );
                                self.settle_cancelled(job);
                                return;
                            }
                            if fault.is_retryable() {
                                attempt += 1;
                                if attempt > MAX_REDRIVES {
                                    self.request_stop(
                                        WorkflowStopReason::Provider,
                                        format!(
                                            "the provider kept failing ({}): {}",
                                            fault.label(),
                                            one_line(message)
                                        ),
                                    );
                                    self.settle_cancelled(job);
                                    return;
                                }
                                let retry_after = match &fault {
                                    Fault::RateLimited { retry_after } => {
                                        let mut g = lock(&lane.governor);
                                        let moved = g.on_rate_limit(
                                            self.now_ms(),
                                            retry_after.map(|d| d.as_millis() as u64),
                                        );
                                        if moved {
                                            lane.gate.set_cap(g.cap(self.now_ms()));
                                        }
                                        *retry_after
                                    }
                                    _ => None,
                                };
                                self.concurrency_changed();
                                // The failed turns still cost something.
                                {
                                    let mut usage = lock(&self.usage);
                                    usage.input_tokens += tokens.0;
                                    usage.output_tokens += tokens.1;
                                }
                                let pause = backoff(attempt - 1, salt, retry_after);
                                tokio::select! {
                                    _ = self.token.cancelled() => {}
                                    _ = tokio::time::sleep(pause) => {}
                                }
                                continue;
                            }
                        }
                        _ => {}
                    }
                    let message = failure.error.to_string();
                    self.settle_ask(
                        job,
                        AskReply::failed(message),
                        NodeOutcome::Failed,
                        true,
                        failure.child_chat_id.clone(),
                        tokens,
                    );
                    return;
                }
            }
        }
    }

    fn build_spec(self: &Arc<Self>, actor: &Arc<Actor>, job: &Job, resolved: &Resolved) -> AskSpec {
        let req = &job.req;
        let can_escalate = true;
        let first = !actor.introduced.load(Ordering::SeqCst);
        let system = first.then(|| prompts::actor_system(&actor.name, &self.name, can_escalate));
        let prompt = prompts::ask_prompt(
            system.as_deref(),
            first.then_some(actor.spec.persona.as_deref()).flatten(),
            &req.instructions,
            can_escalate,
        );
        let (schema, description) = match &req.schema {
            Some(schema) => (schema.clone(), "your final structured result".to_owned()),
            None => (
                json!({
                    "type": "object",
                    "properties": {"text": {"type": "string", "description": "Your complete final answer, as text."}},
                    "required": ["text"]
                }),
                "your final answer as text".to_owned(),
            ),
        };
        let mut spec = AskSpec::new(actor.name.clone(), prompt, schema);
        spec.title = Some(format!("{} · {}", self.name, actor.name));
        spec.result_description = description;
        spec.read_only = req.read_only;
        spec.persistent = true;
        spec.reuse_child = lock(&actor.child).clone();
        spec.timeout = req
            .timeout_s
            .map_or(DEFAULT_ASK_TIMEOUT, Duration::from_secs);
        spec.harness = Some(resolved.harness);
        spec.model = resolved.model.clone();
        spec.reasoning = resolved.reasoning;
        spec.workflow_actor = Some(WorkflowActorTag {
            run_id: self.run_id.clone(),
            site_id: actor.key.site.clone(),
            ordinal: actor.key.ordinal,
            name: actor.name.clone(),
        });
        let core = Arc::downgrade(self);
        let actor_ref = actor.clone();
        let node = req.key.clone();
        spec.progress = Some(ProgressHook(Arc::new({
            let core = core.clone();
            let actor = actor_ref.clone();
            let node = node.clone();
            move |p| {
                if let Some(core) = core.upgrade() {
                    core.on_progress(&actor, &node, p);
                }
            }
        })));
        spec.escalation = Some(EscalationHook(Arc::new(move |e| {
            if let Some(core) = core.upgrade() {
                core.on_escalation(&actor_ref, &node, e);
            }
        })));
        spec
    }

    fn on_progress(&self, actor: &Actor, node: &SiteKey, progress: AskProgress) {
        let (site_id, ordinal) = (node.site.clone(), node.ordinal);
        match progress {
            AskProgress::Child(child) => {
                actor.introduced.store(true, Ordering::SeqCst);
                let fresh = {
                    let mut slot = lock(&actor.child);
                    let fresh = slot.as_deref() != Some(child.as_str());
                    *slot = Some(child.clone());
                    fresh
                };
                if fresh {
                    self.emit(WorkflowEventKind::ActorChild {
                        site_id: actor.key.site.clone(),
                        ordinal: actor.key.ordinal,
                        child_chat_id: child,
                    });
                }
            }
            AskProgress::Executing | AskProgress::Resumed => {
                self.emit(WorkflowEventKind::NodeExecuting { site_id, ordinal })
            }
            AskProgress::Repairing { .. } => {
                self.emit(WorkflowEventKind::NodeRepairing { site_id, ordinal })
            }
            AskProgress::Nudged => self.emit(WorkflowEventKind::NodeNudged { site_id, ordinal }),
            AskProgress::Waiting => self.emit(WorkflowEventKind::NodeWaiting { site_id, ordinal }),
            AskProgress::Turn {
                turns,
                tool_calls,
                last_tool,
            } => self.emit(WorkflowEventKind::NodeProgress {
                site_id,
                ordinal,
                turn: turns,
                tool_calls,
                last_tool,
            }),
        }
    }

    fn on_escalation(&self, actor: &Actor, node: &SiteKey, e: EscalationRaised) {
        lock(&self.questions).insert(e.qid.clone(), (e.child_chat_id.clone(), node.clone()));
        let question = WorkflowQuestion {
            qid: e.qid.clone(),
            actor_site_id: actor.key.site.clone(),
            actor_ordinal: actor.key.ordinal,
            actor_name: actor.name.clone(),
            question: e.question.clone(),
            context: e.context.clone(),
            asked_at: crate::now_ms(),
        };
        self.emit(WorkflowEventKind::EscalationRaised { question });
        let text = prompts::escalation_message(
            &self.name,
            &self.run_id,
            &actor.name,
            &e.qid,
            &e.question,
            &e.context,
        );
        if let Err(err) = self.shared.doc_host.enqueue_machine_message(
            &self.chat_id,
            &format!("workflow-{}-q-{}", self.run_id, e.qid),
            &text,
            zeron_proto::MessageOrigin::Workflow {
                run_id: self.run_id.clone(),
                name: self.name.clone(),
                status: zeron_proto::WorkflowStatus::Running,
            },
        ) {
            tracing::warn!(run = %self.run_id, error = %err, "escalation notice not queued");
        }
    }

    /// Deliver the parent agent's answer; `false` when no such question waits.
    pub(super) async fn resolve_question(&self, qid: &str, answer: &str) -> bool {
        let Some((child, _)) = lock(&self.questions).get(qid).cloned() else {
            return false;
        };
        let Some(backend) = self.shared.doc_host.ask_backend() else {
            return false;
        };
        if backend
            .answer_escalation(&child, qid, answer.to_owned())
            .await
        {
            lock(&self.questions).remove(qid);
            self.emit(WorkflowEventKind::EscalationResolved {
                qid: qid.to_owned(),
            });
            true
        } else {
            false
        }
    }
}

fn preview(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn one_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() > 300 {
        format!("{}…", line.chars().take(300).collect::<String>())
    } else {
        line.to_owned()
    }
}

fn parse_harness(name: &str) -> Result<HarnessId, String> {
    serde_json::from_value::<HarnessId>(Value::String(name.to_owned())).map_err(|_| {
        format!(
            "unknown harness {name:?}: use one of claude-code, codex, cursor, devin, grok, hermes, pi, opencode, antigravity (list_harnesses shows which are installed)"
        )
    })
}

fn parse_reasoning(name: &str) -> Result<ReasoningLevel, String> {
    let lower = name.to_lowercase();
    serde_json::from_value::<ReasoningLevel>(Value::String(lower.clone()))
        .or_else(|_| serde_json::from_value::<ReasoningLevel>(Value::String(lower.replace('-', ""))))
        .map_err(|_| format!("unknown reasoning level {name:?}: use minimal, low, medium, high, xhigh, max, ultra"))
}

fn digest_of(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize()
        .iter()
        .take(12)
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ── the Host the script sees ──────────────────────────────────────────────

impl Host for RunCore {
    fn enter_phase(&self, name: &str) {
        *lock(&self.phase) = Some(name.to_owned());
        self.emit(WorkflowEventKind::PhaseEntered {
            name: name.to_owned(),
        });
    }

    fn create_actor(&self, key: &SiteKey, spec: &ActorSpec) {
        let actor = Arc::new(Actor {
            key: key.clone(),
            name: spec.name.clone(),
            spec: spec.clone(),
            queue: Mutex::new((VecDeque::new(), false)),
            child: Mutex::new(None),
            introduced: AtomicBool::new(false),
            resolved: Mutex::new(None),
        });
        lock(&self.actors).insert(key.to_string(), actor);
        self.emit(WorkflowEventKind::ActorCreated {
            site_id: key.site.clone(),
            ordinal: key.ordinal,
            name: spec.name.clone(),
            harness: spec.harness.clone(),
            model: spec.model.clone(),
        });
    }

    fn ask(&self, req: AskRequest) -> Completion<AskReply> {
        let key = req.key.clone();
        let digest = digest_of(&[
            &req.instructions,
            &req.schema
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_default(),
            if req.read_only { "ro" } else { "rw" },
        ]);
        self.emit(WorkflowEventKind::NodeQueued {
            site_id: key.site.clone(),
            ordinal: key.ordinal,
            kind: NodeKind::Ask,
            actor_site_id: Some(req.actor.site.clone()),
            actor_ordinal: req.actor.ordinal,
            instructions_head: req
                .instructions
                .chars()
                .take(zeron_proto::WORKFLOW_HEAD_CHARS + 1)
                .collect(),
            phase_name: lock(&self.phase).clone(),
        });
        // A resumed run answers from the journal: no child, no cost.
        if let Some(Record::AskDone {
            digest: recorded,
            ok,
            value,
            error,
            input_tokens,
            output_tokens,
            child_chat_id,
            ..
        }) = self.replay.asks.get(&key.to_string())
            && *recorded == digest
        {
            let (completer, completion) = completion();
            let job = Job {
                req,
                digest,
                completer,
            };
            let mut reply = if *ok {
                AskReply::ok(value.clone())
            } else {
                AskReply::failed(error.clone().unwrap_or_default())
            };
            reply.cached = true;
            reply.tokens = input_tokens + output_tokens;
            lock(&self.usage).nodes_cached += 1;
            let _ = child_chat_id;
            let outcome = if *ok {
                NodeOutcome::Ok
            } else {
                NodeOutcome::Failed
            };
            self.settle_ask_cached(job, reply, outcome);
            return completion;
        }
        if let Some(max) = self.meta().options.budgets.max_asks
            && self.asks_started.fetch_add(1, Ordering::SeqCst) + 1 > max
        {
            self.request_stop(
                WorkflowStopReason::Budget,
                format!("the ask budget of {max} was reached"),
            );
            return completed(AskReply::failed("the run's ask budget was reached"));
        }
        let Some(actor) = lock(&self.actors).get(&req.actor.to_string()).cloned() else {
            return completed(AskReply::failed("internal error: ask on an unknown actor"));
        };
        let (completer, completion) = completion();
        self.waiting.fetch_add(1, Ordering::Relaxed);
        let spawn = {
            let mut q = lock(&actor.queue);
            q.0.push_back(Job {
                req,
                digest,
                completer,
            });
            !std::mem::replace(&mut q.1, true)
        };
        if spawn {
            let me = self.me();
            self.rt.spawn(me.pump(actor));
        }
        completion
    }

    fn run(&self, req: RunRequest) -> Completion<RunReply> {
        let head = std::iter::once(req.program.as_str())
            .chain(req.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ");
        let (site_id, ordinal) = (req.key.site.clone(), req.key.ordinal);
        self.emit(WorkflowEventKind::NodeQueued {
            site_id: site_id.clone(),
            ordinal,
            kind: NodeKind::Run,
            actor_site_id: None,
            actor_ordinal: 0,
            instructions_head: head
                .chars()
                .take(zeron_proto::WORKFLOW_HEAD_CHARS + 1)
                .collect(),
            phase_name: lock(&self.phase).clone(),
        });
        let digest = digest_of(&[
            &req.program,
            &req.args.join("\u{1f}"),
            req.cwd.as_deref().unwrap_or(""),
        ]);
        let (completer, completion) = completion();
        if let Some(Record::RunDone {
            digest: recorded,
            exit_code,
            stdout,
            stderr,
            timed_out,
            truncated,
            start_error,
            ..
        }) = self.replay.runs.get(&req.key.to_string())
            && *recorded == digest
        {
            let reply = RunReply {
                exit_code: *exit_code,
                stdout: stdout.clone(),
                stderr: stderr.clone(),
                timed_out: *timed_out,
                truncated: *truncated,
                start_error: start_error.clone(),
            };
            lock(&self.usage).nodes_cached += 1;
            self.settle_command(&req, &digest, &reply, true);
            completer.complete(reply);
            return completion;
        }
        let me = self.me();
        self.rt.spawn(async move {
            let permit = tokio::select! {
                _ = me.token.cancelled() => None,
                p = me.commands.acquire() => Some(p),
            };
            if permit.is_none() {
                me.emit(WorkflowEventKind::NodeSettled {
                    site_id: req.key.site.clone(),
                    ordinal: req.key.ordinal,
                    outcome: NodeOutcome::Cancelled,
                    cached: false,
                    tokens: 0,
                    error: None,
                    result_preview: None,
                });
                return; // dropping the completer abandons the script's wait
            }
            me.emit(WorkflowEventKind::NodeDispatched {
                site_id: req.key.site.clone(),
                ordinal: req.key.ordinal,
            });
            me.emit(WorkflowEventKind::NodeExecuting {
                site_id: req.key.site.clone(),
                ordinal: req.key.ordinal,
            });
            let world = me.world.clone();
            let flag = me.flag.clone();
            let request = req.clone();
            let reply = tokio::task::spawn_blocking(move || world.run(&request, &flag))
                .await
                .unwrap_or_else(|e| RunReply {
                    start_error: Some(format!("the command runner failed: {e}")),
                    ..RunReply::default()
                });
            drop(permit);
            if me.token.is_cancelled() && reply.exit_code.is_none() && !reply.timed_out {
                me.emit(WorkflowEventKind::NodeSettled {
                    site_id: req.key.site.clone(),
                    ordinal: req.key.ordinal,
                    outcome: NodeOutcome::Cancelled,
                    cached: false,
                    tokens: 0,
                    error: None,
                    result_preview: None,
                });
                return;
            }
            lock(&me.usage).nodes_used += 1;
            me.settle_command(&req, &digest, &reply, false);
            completer.complete(reply);
        });
        completion
    }

    fn read(&self, key: &SiteKey, op: &ReadOp) -> Result<Value, String> {
        let digest = digest_of(&[op.name(), &format!("{op:?}")]);
        if let Some(Record::ReadDone {
            digest: recorded,
            value,
            ..
        }) = self.replay.reads.get(&key.to_string())
            && *recorded == digest
        {
            let _ = self.journal.append(&Record::ReadDone {
                key: key.to_string(),
                digest,
                value: value.clone(),
            });
            return Ok(value.clone());
        }
        let value = self.world.read(op)?;
        let _ = self.journal.append(&Record::ReadDone {
            key: key.to_string(),
            digest,
            value: value.clone(),
        });
        Ok(value)
    }

    fn log(&self, line: &str) {
        let _ = self.journal.append(&Record::Log {
            line: line.to_owned(),
        });
    }

    fn report(&self, item: Value, artifact_id: Option<String>) -> Result<(), String> {
        let index = self.reports.fetch_add(1, Ordering::SeqCst);
        let text = match &item {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let _ = self.journal.append(&Record::Report {
            index,
            item: item.clone(),
            artifact_id: artifact_id.clone(),
        });
        self.emit(WorkflowEventKind::Report {
            index,
            text,
            truncated: false,
            artifact_id,
        });
        Ok(())
    }

    fn artifact(&self, req: ArtifactRequest) -> Result<(), String> {
        let (kind, content_type, extension, item_count, bytes): (_, _, String, _, _) =
            match &req.content {
                ArtifactContent::Markdown(text) => (
                    ArtifactKind::Markdown,
                    "text/markdown",
                    "md".to_owned(),
                    0,
                    text.clone().into_bytes(),
                ),
                ArtifactContent::Table { columns, rows } => (
                    ArtifactKind::Table,
                    "application/json",
                    "json".to_owned(),
                    rows.len() as u32,
                    serde_json::to_vec(&json!({"columns": columns, "rows": rows}))
                        .map_err(|e| e.to_string())?,
                ),
                ArtifactContent::Metrics(items) => (
                    ArtifactKind::Metrics,
                    "application/json",
                    "json".to_owned(),
                    items.len() as u32,
                    serde_json::to_vec(
                        &items
                            .iter()
                            .map(|m| json!({"label": m.label, "value": m.value, "unit": m.unit}))
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|e| e.to_string())?,
                ),
                ArtifactContent::File(path) => {
                    let full = confine(self.world.root(), path)?;
                    if !full.is_file() {
                        return Err(format!("{path:?} is not a file"));
                    }
                    let meta = std::fs::metadata(&full).map_err(|e| e.to_string())?;
                    if meta.len() as usize > MAX_ARTIFACT_BYTES {
                        return Err(format!(
                            "{path:?} is {} bytes; artifacts are limited to {MAX_ARTIFACT_BYTES}",
                            meta.len()
                        ));
                    }
                    let ext = full
                        .extension()
                        .and_then(|e| e.to_str())
                        .filter(|e| e.len() <= 8 && e.chars().all(|c| c.is_ascii_alphanumeric()))
                        .unwrap_or("bin")
                        .to_owned();
                    let bytes = std::fs::read(&full).map_err(|e| e.to_string())?;
                    let ct = if std::str::from_utf8(&bytes).is_ok() {
                        "text/plain"
                    } else {
                        "application/octet-stream"
                    };
                    (ArtifactKind::File, ct, ext, 0, bytes)
                }
            };
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(format!(
                "artifact content is {} bytes; the limit is {MAX_ARTIFACT_BYTES}",
                bytes.len()
            ));
        }
        self.shared
            .store
            .put_artifact(
                &self.run_id,
                &req.id,
                req.version,
                kind,
                &req.title,
                content_type,
                &extension,
                item_count,
                &bytes,
                crate::now_ms(),
            )
            .map_err(|e| e.to_string())?;
        let summary = ArtifactSummary {
            id: req.id.clone(),
            kind,
            title: req.title.clone(),
            version: req.version,
            content_type: content_type.to_owned(),
            bytes: bytes.len() as u64,
            item_count,
            primary: false,
        };
        lock(&self.artifacts).insert(req.id.clone(), summary.clone());
        self.emit(WorkflowEventKind::ArtifactPublished { summary });
        Ok(())
    }
}

impl RunCore {
    fn settle_ask_cached(&self, job: Job, reply: AskReply, outcome: NodeOutcome) {
        // Re-journal so the new run's journal is self-contained (a later
        // resume needs only it).
        let _ = self.journal.append(&Record::AskDone {
            key: job.req.key.to_string(),
            digest: job.digest.clone(),
            ok: reply.ok,
            value: reply.value.clone(),
            error: reply.error.clone(),
            input_tokens: reply.tokens,
            output_tokens: 0,
            child_chat_id: None,
        });
        self.emit(WorkflowEventKind::NodeSettled {
            site_id: job.req.key.site.clone(),
            ordinal: job.req.key.ordinal,
            outcome,
            cached: true,
            tokens: 0,
            error: reply.error.clone(),
            result_preview: reply.ok.then(|| preview(&reply.value)),
        });
        self.emit_usage();
        job.completer.complete(reply);
    }

    fn settle_command(&self, req: &RunRequest, digest: &str, reply: &RunReply, cached: bool) {
        let _ = self.journal.append(&Record::RunDone {
            key: req.key.to_string(),
            digest: digest.to_owned(),
            exit_code: reply.exit_code,
            stdout: reply.stdout.clone(),
            stderr: reply.stderr.clone(),
            timed_out: reply.timed_out,
            truncated: reply.truncated,
            start_error: reply.start_error.clone(),
        });
        let ok = reply.exit_code == Some(0) && !reply.timed_out && reply.start_error.is_none();
        let error = if ok {
            None
        } else if let Some(e) = &reply.start_error {
            Some(e.clone())
        } else if reply.timed_out {
            Some("timed out".to_owned())
        } else {
            Some(format!(
                "exit {}",
                reply.exit_code.map_or("?".into(), |c| c.to_string())
            ))
        };
        self.emit(WorkflowEventKind::NodeSettled {
            site_id: req.key.site.clone(),
            ordinal: req.key.ordinal,
            outcome: if ok {
                NodeOutcome::Ok
            } else {
                NodeOutcome::Failed
            },
            cached,
            tokens: 0,
            error,
            result_preview: Some(format!(
                "exit {}{}",
                reply.exit_code.map_or("none".into(), |c| c.to_string()),
                if reply.stdout.is_empty() {
                    String::new()
                } else {
                    format!("\n{}", reply.stdout.chars().take(600).collect::<String>())
                }
            )),
        });
        self.emit_usage();
    }

    pub(super) fn begin_budget_timer(&self) -> Option<tokio::task::JoinHandle<()>> {
        let secs = self.meta().options.budgets.max_runtime_seconds?;
        let me = self.me();
        Some(self.rt.spawn(async move {
            // Time already spent in earlier segments counts.
            let spent = Duration::from_millis(me.base_elapsed_ms);
            let left = Duration::from_secs(secs).saturating_sub(spent);
            tokio::time::sleep(left).await;
            me.request_stop(
                WorkflowStopReason::Budget,
                format!("the runtime budget of {secs}s was reached"),
            );
        }))
    }
}
