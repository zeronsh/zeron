//! Dynamic workflows: the replicated state of a chat's workflow runs.
//!
//! A workflow is a Starlark script the agent writes; the engine runs it in
//! the background, journals it and drives many hidden child chats
//! (`docs/workflows.md`). This module holds only what clients need to *show*
//! a run — the state, its events and deltas, the static graph used by the
//! approval dialog — so desktop and mobile never link the interpreter.
//!
//! The state rides the PARENT chat's session doc (`meta.workflowRuns`,
//! host-only writer, one entry per actor / node / report / artifact so a
//! 500-node run writes small updates instead of one huge value). Everything is
//! additive and serde-defaulted, like the goal types.
//!
//! The pure reducer that folds [`WorkflowEvent`]s into this state lives in
//! `zeron-workflow` (it is the only crate besides the engine that needs it);
//! [`WorkflowRunsState::apply`] and [`WorkflowRunsState::diff`] live here
//! because every consumer of a delta stream needs them.

use serde::{Deserialize, Serialize};

/// Runs kept in the synced state (oldest finished evicted first).
pub const WORKFLOW_MAX_RUNS: usize = 8;
/// Actors listed per run.
pub const WORKFLOW_MAX_ACTORS: usize = 1024;
/// Nodes (asks and commands) listed per run.
pub const WORKFLOW_MAX_NODES: usize = 1024;
/// Entries (actors + nodes + reports + artifacts + questions) across all runs.
pub const WORKFLOW_MAX_ENTRIES: usize = 6144;
pub const WORKFLOW_MAX_REPORTS: usize = 64;
pub const WORKFLOW_MAX_ARTIFACTS: usize = 32;
pub const WORKFLOW_MAX_QUESTIONS: usize = 32;
/// Longest text preview carried in synced state (results, reports, errors).
pub const WORKFLOW_PREVIEW_BYTES: usize = 2048;
/// Longest instruction head shown on a node.
pub const WORKFLOW_HEAD_CHARS: usize = 240;

// ── status vocabulary ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowStatus {
    /// Created; waiting for the user's approval.
    #[default]
    Pending,
    Running,
    Completed,
    /// The script failed (a `fail()`, a runtime error, a limit).
    Errored,
    /// Stopped without finishing; `stop_reason` says why. Often resumable.
    Stopped,
}

impl WorkflowStatus {
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Completed | Self::Errored | Self::Stopped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowStopReason {
    /// The user (or the parent agent on the user's behalf) stopped it.
    User,
    /// The engine restarted while it ran.
    Interrupted,
    /// A deterministic provider error (auth, quota, model unavailable).
    Provider,
    /// A run budget (asks, tokens, runtime) was reached.
    Budget,
    /// The user denied the approval.
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    /// `actor.ask(...)`: a prompt to a child chat.
    Ask,
    /// `run(cmd, ...)`: a shell gate.
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum NodePhase {
    #[default]
    Queued,
    /// Admitted by the scheduler, child about to be prompted.
    Dispatched,
    Executing,
    /// Parked on an escalation question.
    Waiting,
    /// The submission violated the schema; the model is repairing it.
    Repairing,
    /// The turn ended without a result; nudged once.
    Nudged,
    Settled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeOutcome {
    Ok,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum ActorStatus {
    /// No ask running (none yet, or all queued behind the actor's FIFO).
    #[default]
    Waiting,
    Running,
    Completed,
}

// ── entries ───────────────────────────────────────────────────────────────

/// One `agent(...)` call: a persistent child chat, created at its first ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowActor {
    /// Creation order within the run (lists are sorted by it; map-backed
    /// storage has no order of its own).
    #[serde(default)]
    pub order: u32,
    /// Call-site identity (see `zeron-workflow` `site`), plus how many times
    /// that site had run before (`agent()` inside a loop or `pmap`).
    pub site_id: String,
    pub ordinal: u32,
    pub name: String,
    /// The hidden child chat, once created (read-only openable by id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_chat_id: Option<String>,
    /// DERIVED from the actor's nodes by the reducer.
    #[serde(default)]
    pub status: ActorStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub asks: u32,
    #[serde(default)]
    pub failed_asks: u32,
}

impl WorkflowActor {
    pub fn key(&self) -> String {
        entry_key('a', &self.site_id, self.ordinal)
    }
}

/// One ask or one shell gate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowNode {
    /// Creation order within the run.
    #[serde(default)]
    pub order: u32,
    pub site_id: String,
    pub ordinal: u32,
    pub kind: NodeKind,
    #[serde(default)]
    pub phase: NodePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<NodeOutcome>,
    /// Replayed from the journal instead of run (a resume).
    #[serde(default)]
    pub cached: bool,
    /// The actor an ask belongs to (absent for commands).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_site_id: Option<String>,
    #[serde(default)]
    pub actor_ordinal: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_name: Option<String>,
    /// First [`WORKFLOW_HEAD_CHARS`] of the instructions (or the command).
    #[serde(default)]
    pub instructions_head: String,
    /// Turns the child has run for this ask.
    #[serde(default)]
    pub turn: u32,
    #[serde(default)]
    pub tool_calls: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<String>,
    #[serde(default)]
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    /// Why it failed (a `Result.error`), preview-sized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Preview of the result (≤ [`WORKFLOW_PREVIEW_BYTES`]); the full value is
    /// in the host's journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
}

impl WorkflowNode {
    pub fn key(&self) -> String {
        entry_key('n', &self.site_id, self.ordinal)
    }

    pub fn is_settled(&self) -> bool {
        self.phase == NodePhase::Settled
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowReport {
    /// 0-based position in the run's report stream.
    pub index: u32,
    /// Preview of the reported item (≤ [`WORKFLOW_PREVIEW_BYTES`]).
    pub text: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<String>,
    #[serde(default)]
    pub at: i64,
}

impl WorkflowReport {
    pub fn key(&self) -> String {
        format!("r:{}", self.index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    Markdown,
    Table,
    Metrics,
    File,
}

/// What clients list for an artifact; content is fetched on demand
/// (`WorkflowArtifactData` / `WorkflowArtifactRead`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactSummary {
    pub id: String,
    pub kind: ArtifactKind,
    pub title: String,
    /// Latest published version (1-based, ≤ 16 kept).
    pub version: u32,
    pub content_type: String,
    pub bytes: u64,
    /// Rows (table), items (metrics); 0 for markdown and files.
    #[serde(default)]
    pub item_count: u32,
    /// The artifact `report(..., artifact_id)` last pointed at, i.e. the one
    /// a card should lead with.
    #[serde(default)]
    pub primary: bool,
}

impl ArtifactSummary {
    pub fn key(&self) -> String {
        format!("f:{}", self.id)
    }
}

/// An actor's `escalate(...)` question, parked until the parent agent answers
/// it with `resolve_workflow_question`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowQuestion {
    pub qid: String,
    pub actor_site_id: String,
    pub actor_ordinal: u32,
    pub actor_name: String,
    pub question: String,
    #[serde(default)]
    pub context: String,
    pub asked_at: i64,
}

impl WorkflowQuestion {
    pub fn key(&self) -> String {
        format!("q:{}", self.qid)
    }
}

/// One keyed record of a run's synced state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "camelCase")]
pub enum WorkflowEntry {
    Actor(WorkflowActor),
    Node(WorkflowNode),
    Report(WorkflowReport),
    Artifact(ArtifactSummary),
    Question(WorkflowQuestion),
    Graph(WorkflowGraph),
}

impl WorkflowEntry {
    pub fn key(&self) -> String {
        match self {
            Self::Actor(e) => e.key(),
            Self::Node(e) => e.key(),
            Self::Report(e) => e.key(),
            Self::Artifact(e) => e.key(),
            Self::Question(e) => e.key(),
            Self::Graph(_) => GRAPH_KEY.to_owned(),
        }
    }
}

/// Entry key of a run's static graph.
pub const GRAPH_KEY: &str = "g";

pub fn entry_key(prefix: char, site_id: &str, ordinal: u32) -> String {
    format!("{prefix}:{site_id}#{ordinal}")
}

// ── header ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Asks and commands that ran (cached replays excluded).
    pub nodes_used: u32,
    /// Replayed from the journal on a resume.
    pub nodes_cached: u32,
    /// Wall time since launch, ms (accumulated across resumes).
    pub elapsed_ms: u64,
}

impl WorkflowUsage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowConcurrency {
    /// Asks the scheduler may have in flight right now.
    pub cap: u32,
    /// The configured ceiling (`max_concurrency`, default `clamp(cores-2,1,16)`).
    pub ceiling: u32,
    pub in_flight: u32,
    pub queued: u32,
    /// A governor lowered the cap after provider rate limits.
    #[serde(default)]
    pub throttled: bool,
}

/// How many nodes a phase has seen and finished — the card's "settled/observed".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhaseProgress {
    pub name: String,
    pub observed: u32,
    pub settled: u32,
}

/// The per-run record without its entries: replaced whole on every change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunHeader {
    pub run_id: String,
    pub name: String,
    /// The chat the run belongs to (its parent).
    pub chat_id: String,
    #[serde(default)]
    pub status: WorkflowStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<WorkflowStopReason>,
    /// A sentence for the user ("quota exceeded for model x").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_from: Option<String>,
    #[serde(default)]
    pub script_hash: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default)]
    pub usage: WorkflowUsage,
    /// The script's own failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `resume_workflow_run` can continue it.
    #[serde(default)]
    pub resumable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
    #[serde(default)]
    pub result_truncated: bool,
    #[serde(default)]
    pub concurrency: WorkflowConcurrency,
    #[serde(default)]
    pub phases: Vec<WorkflowPhaseProgress>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    /// Literal phases of the script in source order (the card's skeleton).
    #[serde(default)]
    pub phase_names: Vec<String>,
    /// Phases that had work in flight at the same time.
    #[serde(default)]
    pub phase_alongside: Vec<Vec<String>>,
    /// Entries (or whole runs) were dropped to stay within the caps.
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub actors_unlisted: u32,
    #[serde(default)]
    pub nodes_unlisted: u32,
    /// No ask succeeded for [`STALL_NOTICE_MS`] while work was pending.
    #[serde(default)]
    pub stalled: bool,
    /// Highest event folded into this state (idempotence guard).
    #[serde(default)]
    pub last_event_sequence: u64,
    /// Totals the lists above may have shed.
    #[serde(default)]
    pub reports_total: u32,
}

/// No successful ask for this long (with work pending) raises a stall notice.
pub const STALL_NOTICE_MS: i64 = 20 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    #[serde(flatten)]
    pub header: WorkflowRunHeader,
    #[serde(default)]
    pub actors: Vec<WorkflowActor>,
    #[serde(default)]
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub reports: Vec<WorkflowReport>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactSummary>,
    #[serde(default)]
    pub pending_questions: Vec<WorkflowQuestion>,
    /// The static graph of the script, for the card skeleton. Its own entry
    /// (not part of the header) so it is written once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<WorkflowGraph>,
}

impl WorkflowRun {
    pub fn entry_count(&self) -> usize {
        self.actors.len()
            + self.nodes.len()
            + self.reports.len()
            + self.artifacts.len()
            + self.pending_questions.len()
            + usize::from(self.graph.is_some())
    }

    /// Every entry, keyed.
    pub fn entries(&self) -> Vec<WorkflowEntry> {
        let mut out = Vec::with_capacity(self.entry_count());
        out.extend(self.actors.iter().cloned().map(WorkflowEntry::Actor));
        out.extend(self.nodes.iter().cloned().map(WorkflowEntry::Node));
        out.extend(self.reports.iter().cloned().map(WorkflowEntry::Report));
        out.extend(self.artifacts.iter().cloned().map(WorkflowEntry::Artifact));
        out.extend(
            self.pending_questions
                .iter()
                .cloned()
                .map(WorkflowEntry::Question),
        );
        out.extend(self.graph.iter().cloned().map(WorkflowEntry::Graph));
        out
    }

    pub fn upsert(&mut self, entry: WorkflowEntry) {
        fn put<T>(list: &mut Vec<T>, item: T, same: impl Fn(&T) -> bool) {
            match list.iter_mut().find(|x| same(x)) {
                Some(slot) => *slot = item,
                None => list.push(item),
            }
        }
        match entry {
            WorkflowEntry::Actor(a) => {
                let key = a.key();
                put(&mut self.actors, a, |x| x.key() == key);
            }
            WorkflowEntry::Node(n) => {
                let key = n.key();
                put(&mut self.nodes, n, |x| x.key() == key);
            }
            WorkflowEntry::Report(r) => {
                let key = r.key();
                put(&mut self.reports, r, |x| x.key() == key);
            }
            WorkflowEntry::Artifact(f) => {
                let key = f.key();
                put(&mut self.artifacts, f, |x| x.key() == key);
            }
            WorkflowEntry::Question(q) => {
                let key = q.key();
                put(&mut self.pending_questions, q, |x| x.key() == key);
            }
            WorkflowEntry::Graph(g) => self.graph = Some(g),
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.actors.retain(|x| x.key() != key);
        self.nodes.retain(|x| x.key() != key);
        self.reports.retain(|x| x.key() != key);
        self.artifacts.retain(|x| x.key() != key);
        self.pending_questions.retain(|x| x.key() != key);
        if key == GRAPH_KEY {
            self.graph = None;
        }
    }
}

/// The synced state of every workflow run of one chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunsState {
    /// Bumped on every change; deltas carry the revision they produce.
    #[serde(default)]
    pub revision: u64,
    /// Oldest first, at most [`WORKFLOW_MAX_RUNS`].
    #[serde(default)]
    pub runs: Vec<WorkflowRun>,
}

impl WorkflowRunsState {
    pub fn run(&self, run_id: &str) -> Option<&WorkflowRun> {
        self.runs.iter().find(|r| r.header.run_id == run_id)
    }

    pub fn run_mut(&mut self, run_id: &str) -> Option<&mut WorkflowRun> {
        self.runs.iter_mut().find(|r| r.header.run_id == run_id)
    }

    pub fn entry_count(&self) -> usize {
        self.runs.iter().map(WorkflowRun::entry_count).sum()
    }

    /// Any run still pending or running.
    pub fn has_live_run(&self) -> bool {
        self.runs.iter().any(|r| !r.header.status.is_settled())
    }
}

// ── activity (sidebar) ────────────────────────────────────────────────────

/// One run as the sidebar sees it: the header (phase progress included) and
/// how many escalations wait for an answer. No entries — a chat that is not
/// open needs a few hundred bytes per run, not its node list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunBrief {
    #[serde(flatten)]
    pub header: WorkflowRunHeader,
    #[serde(default)]
    pub pending_questions: u32,
}

/// Runs of every chat the engine hosts (`WatchWorkflowActivity`): oldest
/// first per chat, only chats that have any. Pushed whole when a brief
/// changes (coalesced like the doc writes, ≤ 4 a second).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowActivity {
    #[serde(default)]
    pub chats: std::collections::BTreeMap<String, Vec<WorkflowRunBrief>>,
}

impl WorkflowRun {
    pub fn brief(&self) -> WorkflowRunBrief {
        WorkflowRunBrief {
            header: self.header.clone(),
            pending_questions: self.pending_questions.len() as u32,
        }
    }
}

impl WorkflowRunsState {
    /// Briefs of every run, oldest first.
    pub fn briefs(&self) -> Vec<WorkflowRunBrief> {
        self.runs.iter().map(WorkflowRun::brief).collect()
    }
}

// ── deltas ────────────────────────────────────────────────────────────────

/// What changed in one run: its header (replaced whole, when present),
/// upserted entries and removed entry keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunDelta {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<WorkflowRunHeader>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upserts: Vec<WorkflowEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

impl WorkflowRunDelta {
    pub fn for_run(run_id: impl Into<String>) -> Self {
        Self {
            run_id: run_id.into(),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.header.is_none() && self.upserts.is_empty() && self.removed.is_empty()
    }

    /// Fold a later delta of the same run into this one (the coalescer that
    /// batches doc writes): last writer wins per key, a removal cancels an
    /// earlier upsert of its key and the reverse.
    pub fn merge(&mut self, later: WorkflowRunDelta) {
        debug_assert_eq!(self.run_id, later.run_id);
        if later.header.is_some() {
            self.header = later.header;
        }
        for entry in later.upserts {
            let key = entry.key();
            self.removed.retain(|k| *k != key);
            match self.upserts.iter_mut().find(|e| e.key() == key) {
                Some(slot) => *slot = entry,
                None => self.upserts.push(entry),
            }
        }
        for key in later.removed {
            self.upserts.retain(|e| e.key() != key);
            if !self.removed.contains(&key) {
                self.removed.push(key);
            }
        }
    }
}

/// A batch of changes across runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunsDelta {
    /// The state revision after applying this delta.
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<WorkflowRunDelta>,
    /// Whole runs evicted by the run cap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs_removed: Vec<String>,
}

impl WorkflowRunsDelta {
    pub fn is_empty(&self) -> bool {
        self.runs.iter().all(WorkflowRunDelta::is_empty) && self.runs_removed.is_empty()
    }

    pub fn merge(&mut self, later: WorkflowRunsDelta) {
        self.revision = self.revision.max(later.revision);
        for run in later.runs {
            match self.runs.iter_mut().find(|r| r.run_id == run.run_id) {
                Some(slot) => slot.merge(run),
                None => self.runs.push(run),
            }
        }
        for id in later.runs_removed {
            self.runs.retain(|r| r.run_id != id);
            if !self.runs_removed.contains(&id) {
                self.runs_removed.push(id);
            }
        }
    }
}

impl WorkflowRunsState {
    /// Apply a delta. Idempotent: applying one twice leaves the same state,
    /// and a delta older than the state's revision is ignored.
    pub fn apply(&mut self, delta: &WorkflowRunsDelta) {
        if delta.revision != 0 && delta.revision <= self.revision {
            return;
        }
        for id in &delta.runs_removed {
            self.runs.retain(|r| r.header.run_id != *id);
        }
        for change in &delta.runs {
            if self.run(&change.run_id).is_none() {
                let Some(header) = &change.header else {
                    continue; // a change to a run this view never saw
                };
                self.runs.push(WorkflowRun {
                    header: header.clone(),
                    ..WorkflowRun::default()
                });
            }
            let Some(run) = self.run_mut(&change.run_id) else {
                continue;
            };
            if let Some(header) = &change.header {
                run.header = header.clone();
            }
            for key in &change.removed {
                run.remove(key);
            }
            for entry in &change.upserts {
                run.upsert(entry.clone());
            }
        }
        self.revision = self.revision.max(delta.revision);
    }

    /// What turns `self` into `next`, `None` when they are equal. Lets a
    /// host serve per-subscriber deltas without keeping a change log.
    pub fn diff(&self, next: &WorkflowRunsState) -> Option<WorkflowRunsDelta> {
        let mut out = WorkflowRunsDelta {
            revision: next.revision,
            ..WorkflowRunsDelta::default()
        };
        for old in &self.runs {
            if next.run(&old.header.run_id).is_none() {
                out.runs_removed.push(old.header.run_id.clone());
            }
        }
        for run in &next.runs {
            let Some(old) = self.run(&run.header.run_id) else {
                out.runs.push(WorkflowRunDelta {
                    run_id: run.header.run_id.clone(),
                    header: Some(run.header.clone()),
                    upserts: run.entries(),
                    removed: Vec::new(),
                });
                continue;
            };
            let mut change = WorkflowRunDelta::for_run(run.header.run_id.clone());
            if old.header != run.header {
                change.header = Some(run.header.clone());
            }
            let before: std::collections::HashMap<String, WorkflowEntry> =
                old.entries().into_iter().map(|e| (e.key(), e)).collect();
            let now = run.entries();
            for entry in &now {
                if before.get(&entry.key()) != Some(entry) {
                    change.upserts.push(entry.clone());
                }
            }
            let live: std::collections::HashSet<String> = now.iter().map(|e| e.key()).collect();
            change.removed = before.into_keys().filter(|k| !live.contains(k)).collect();
            change.removed.sort();
            if !change.is_empty() {
                out.runs.push(change);
            }
        }
        (!out.is_empty()).then_some(out)
    }
}

/// Stamped on a workflow actor's child chat (`meta.workflowActor`) so clients
/// can tell it from a verifier or a person's chat and open it read-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowActorTag {
    pub run_id: String,
    pub site_id: String,
    #[serde(default)]
    pub ordinal: u32,
    #[serde(default)]
    pub name: String,
}

/// How a transcript watch carries workflow state: the whole state on the
/// opening (and reset) frame, deltas after.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowsUpdate {
    Full(WorkflowRunsState),
    Delta(WorkflowRunsDelta),
}

// ── events (the journal's vocabulary) ─────────────────────────────────────

/// One thing that happened in a run. Events are the journal's spine and the
/// reducer's input; `seq` is monotone per run, so replay and duplicate
/// delivery are harmless.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEvent {
    pub run_id: String,
    pub seq: u64,
    /// Epoch millis.
    pub at: i64,
    #[serde(flatten)]
    pub kind: WorkflowEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorkflowEventKind {
    /// The run exists (pending approval, or being resumed).
    RunCreated {
        name: String,
        chat_id: String,
        script_hash: String,
        #[serde(default)]
        resumed_from: Option<String>,
        #[serde(default)]
        graph: Option<WorkflowGraph>,
        #[serde(default)]
        concurrency_ceiling: u32,
    },
    /// Approved and executing.
    RunLaunched {
        #[serde(default)]
        phase_names: Vec<String>,
    },
    PhaseEntered {
        name: String,
    },
    ActorCreated {
        site_id: String,
        ordinal: u32,
        name: String,
        #[serde(default)]
        harness: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
    /// The actor's child chat was created.
    ActorChild {
        site_id: String,
        ordinal: u32,
        child_chat_id: String,
    },
    NodeQueued {
        site_id: String,
        ordinal: u32,
        kind: NodeKind,
        #[serde(default)]
        actor_site_id: Option<String>,
        #[serde(default)]
        actor_ordinal: u32,
        #[serde(default)]
        instructions_head: String,
        /// The phase the script was in when it dispatched the node.
        #[serde(default)]
        phase_name: Option<String>,
    },
    NodeDispatched {
        site_id: String,
        ordinal: u32,
    },
    NodeExecuting {
        site_id: String,
        ordinal: u32,
    },
    NodeWaiting {
        site_id: String,
        ordinal: u32,
    },
    NodeRepairing {
        site_id: String,
        ordinal: u32,
    },
    NodeNudged {
        site_id: String,
        ordinal: u32,
    },
    NodeProgress {
        site_id: String,
        ordinal: u32,
        turn: u32,
        tool_calls: u32,
        #[serde(default)]
        last_tool: Option<String>,
    },
    NodeSettled {
        site_id: String,
        ordinal: u32,
        outcome: NodeOutcome,
        #[serde(default)]
        cached: bool,
        #[serde(default)]
        tokens: u64,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        result_preview: Option<String>,
    },
    Report {
        index: u32,
        text: String,
        #[serde(default)]
        truncated: bool,
        #[serde(default)]
        artifact_id: Option<String>,
    },
    ArtifactPublished {
        summary: ArtifactSummary,
    },
    UsageUpdated {
        usage: WorkflowUsage,
    },
    EscalationRaised {
        question: WorkflowQuestion,
    },
    EscalationResolved {
        qid: String,
    },
    ConcurrencyChanged {
        concurrency: WorkflowConcurrency,
    },
    /// No successful ask for [`STALL_NOTICE_MS`].
    Stalled,
    /// Work resumed after a stall notice.
    Unstalled,
    RunSettled {
        status: WorkflowStatus,
        #[serde(default)]
        stop_reason: Option<WorkflowStopReason>,
        #[serde(default)]
        stop_detail: Option<String>,
        #[serde(default)]
        error: Option<String>,
        #[serde(default)]
        result_preview: Option<String>,
        #[serde(default)]
        result_truncated: bool,
        #[serde(default)]
        resumable: bool,
    },
}

// ── static graph ──────────────────────────────────────────────────────────

/// A call site in the script. `site_id` is the journal's identity for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphSite {
    pub site_id: String,
    pub line: u32,
    pub col: u32,
    /// The call runs inside a loop, comprehension or `pmap`: many times.
    #[serde(default)]
    pub fan_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphActor {
    #[serde(flatten)]
    pub site: GraphSite,
    /// The literal name, when it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphAsk {
    #[serde(flatten)]
    pub site: GraphSite,
    /// The variable the ask was made on (`reviewer` in `reviewer.ask(...)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// First characters of the instructions when they are a literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions_head: Option<String>,
    #[serde(default)]
    pub typed: bool,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphCommand {
    #[serde(flatten)]
    pub site: GraphSite,
    /// The literal program.
    pub command: String,
    /// Literal arguments, when the argument list is a literal list of strings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GraphPhase {
    pub name: String,
    pub line: u32,
    pub col: u32,
    #[serde(default)]
    pub asks: Vec<GraphAsk>,
    #[serde(default)]
    pub commands: Vec<GraphCommand>,
}

/// What the approval dialog and the card skeleton show about a script,
/// computed by static analysis before anything runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGraph {
    pub phases: Vec<GraphPhase>,
    pub actors: Vec<GraphActor>,
    /// Every literal command the script can run (also listed per phase).
    pub commands: Vec<GraphCommand>,
    /// Asks and commands that sit outside every phase (a diagnostic, but a
    /// graph is still produced for display).
    #[serde(default)]
    pub unphased_asks: u32,
}

impl WorkflowGraph {
    pub fn phase_names(&self) -> Vec<String> {
        self.phases.iter().map(|p| p.name.clone()).collect()
    }
}

// ── commands (command plane) ──────────────────────────────────────────────

/// A workflow mutation, executed by the chat's host. Approval answers
/// travel the existing `respond_input` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WorkflowCommand {
    Stop {
        run_id: String,
        #[serde(default)]
        reason: Option<String>,
    },
    Resume {
        run_id: String,
    },
    /// Answer an actor's escalation (what `resolve_workflow_question` does).
    Answer {
        run_id: String,
        qid: String,
        answer: String,
    },
}

// ── transcript origins ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowEventMarker {
    Started,
    Completed,
    Errored,
    Stopped,
    Denied,
    Resumed,
}

/// Plain-words text of a lifecycle marker: what a client that predates
/// [`crate::MessageOrigin::WorkflowEvent`] shows.
pub fn workflow_marker_text(marker: WorkflowEventMarker, name: &str, detail: &str) -> String {
    let head = match marker {
        WorkflowEventMarker::Started => format!("Workflow started: {name}"),
        WorkflowEventMarker::Completed => format!("Workflow completed: {name}"),
        WorkflowEventMarker::Errored => format!("Workflow failed: {name}"),
        WorkflowEventMarker::Stopped => format!("Workflow stopped: {name}"),
        WorkflowEventMarker::Denied => format!("Workflow denied: {name}"),
        WorkflowEventMarker::Resumed => format!("Workflow resumed: {name}"),
    };
    if detail.trim().is_empty() {
        head
    } else {
        format!("{head} — {}", detail.trim())
    }
}

/// Marker for `UserInputQuestion::meta` of an approval request, so a UI can
/// render the graph instead of the plain question text.
pub const WORKFLOW_APPROVAL_META_KIND: &str = "workflowApproval";

/// The structured payload of a workflow approval question (`meta`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowApprovalMeta {
    pub run_id: String,
    pub name: String,
    pub script_hash: String,
    /// Where the script was drafted (project-relative when inside it).
    #[serde(default)]
    pub draft_path: Option<String>,
    pub graph: WorkflowGraph,
    pub max_concurrency: u32,
    #[serde(default)]
    pub budgets: WorkflowBudgets,
    /// Resolved default harness / model for asks that do not pick one.
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// First lines of the script.
    #[serde(default)]
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowBudgets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_asks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runtime_seconds: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(site: &str, ordinal: u32, phase: NodePhase) -> WorkflowNode {
        WorkflowNode {
            order: 0,
            site_id: site.into(),
            ordinal,
            kind: NodeKind::Ask,
            phase,
            outcome: None,
            cached: false,
            actor_site_id: Some("a".into()),
            actor_ordinal: 0,
            phase_name: Some("review".into()),
            instructions_head: "look".into(),
            turn: 0,
            tool_calls: 0,
            last_tool: None,
            tokens: 0,
            started_at: None,
            ended_at: None,
            error: None,
            result_preview: None,
        }
    }

    fn run(id: &str) -> WorkflowRun {
        WorkflowRun {
            header: WorkflowRunHeader {
                run_id: id.into(),
                name: "demo".into(),
                chat_id: "c".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn state_round_trips_and_ignores_unknown_fields() {
        let mut r = run("r1");
        r.nodes.push(node("3:5-3:20", 0, NodePhase::Executing));
        let state = WorkflowRunsState {
            revision: 4,
            runs: vec![r],
        };
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["runs"][0]["runId"], "r1");
        assert_eq!(json["runs"][0]["nodes"][0]["siteId"], "3:5-3:20");
        let mut with_future = json.clone();
        with_future["runs"][0]["somethingNew"] = serde_json::json!(1);
        with_future["runs"][0]["nodes"][0]["alsoNew"] = serde_json::json!("x");
        let back: WorkflowRunsState = serde_json::from_value(with_future).unwrap();
        assert_eq!(back, state);
    }

    #[test]
    fn an_empty_object_decodes_to_the_default_state() {
        let state: WorkflowRunsState = serde_json::from_str("{}").unwrap();
        assert_eq!(state, WorkflowRunsState::default());
        let run: WorkflowRun =
            serde_json::from_str(r#"{"runId":"x","name":"n","chatId":"c"}"#).unwrap();
        assert_eq!(run.header.status, WorkflowStatus::Pending);
        assert!(run.nodes.is_empty());
    }

    #[test]
    fn events_round_trip_with_flattened_kind() {
        let ev = WorkflowEvent {
            run_id: "r".into(),
            seq: 7,
            at: 99,
            kind: WorkflowEventKind::NodeSettled {
                site_id: "s".into(),
                ordinal: 2,
                outcome: NodeOutcome::Failed,
                cached: false,
                tokens: 5,
                error: Some("boom".into()),
                result_preview: None,
            },
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "nodeSettled");
        assert_eq!(json["runId"], "r");
        assert_eq!(json["outcome"], "failed");
        assert_eq!(serde_json::from_value::<WorkflowEvent>(json).unwrap(), ev);
    }

    #[test]
    fn apply_is_idempotent_and_diff_inverts_it() {
        let mut a = WorkflowRunsState::default();
        let mut r = run("r1");
        r.nodes.push(node("s1", 0, NodePhase::Queued));
        r.nodes.push(node("s2", 0, NodePhase::Queued));
        let mut b = WorkflowRunsState {
            revision: 1,
            runs: vec![r],
        };
        let d1 = a.diff(&b).unwrap();
        a.apply(&d1);
        assert_eq!(a, b);
        a.apply(&d1); // idempotent
        assert_eq!(a, b);

        b.revision = 2;
        let r = b.run_mut("r1").unwrap();
        r.nodes[0].phase = NodePhase::Settled;
        r.nodes.remove(1);
        r.header.usage.nodes_used = 1;
        let d2 = a.diff(&b).unwrap();
        assert_eq!(d2.runs[0].upserts.len(), 1);
        assert_eq!(d2.runs[0].removed, vec!["n:s2#0".to_string()]);
        a.apply(&d2);
        assert_eq!(a, b);
        assert!(a.diff(&b).is_none());

        b.revision = 3;
        b.runs.clear();
        let d3 = a.diff(&b).unwrap();
        assert_eq!(d3.runs_removed, vec!["r1".to_string()]);
        a.apply(&d3);
        assert_eq!(a, b);
    }

    #[test]
    fn stale_deltas_are_ignored() {
        let mut state = WorkflowRunsState {
            revision: 5,
            runs: vec![run("r1")],
        };
        let stale = WorkflowRunsDelta {
            revision: 3,
            runs_removed: vec!["r1".into()],
            ..Default::default()
        };
        state.apply(&stale);
        assert_eq!(state.runs.len(), 1);
    }

    #[test]
    fn merging_deltas_keeps_the_last_write_per_key() {
        let mut first = WorkflowRunDelta::for_run("r");
        first
            .upserts
            .push(WorkflowEntry::Node(node("a", 0, NodePhase::Queued)));
        first
            .upserts
            .push(WorkflowEntry::Node(node("b", 0, NodePhase::Queued)));
        let mut second = WorkflowRunDelta::for_run("r");
        second
            .upserts
            .push(WorkflowEntry::Node(node("a", 0, NodePhase::Settled)));
        second.removed.push("n:b#0".into());
        first.merge(second);
        assert_eq!(first.upserts.len(), 1);
        match &first.upserts[0] {
            WorkflowEntry::Node(n) => assert_eq!(n.phase, NodePhase::Settled),
            other => panic!("{other:?}"),
        }
        assert_eq!(first.removed, vec!["n:b#0".to_string()]);
        // …and an upsert after a removal revives the key.
        let mut third = WorkflowRunDelta::for_run("r");
        third
            .upserts
            .push(WorkflowEntry::Node(node("b", 0, NodePhase::Queued)));
        first.merge(third);
        assert!(first.removed.is_empty());
        assert_eq!(first.upserts.len(), 2);
    }

    #[test]
    fn briefs_carry_the_header_and_the_question_count_and_round_trip() {
        let mut r = run("r1");
        r.header.status = WorkflowStatus::Running;
        r.pending_questions.push(WorkflowQuestion {
            qid: "q".into(),
            actor_site_id: "s".into(),
            actor_ordinal: 0,
            actor_name: "a".into(),
            question: "?".into(),
            context: String::new(),
            asked_at: 1,
        });
        let state = WorkflowRunsState {
            revision: 3,
            runs: vec![r],
        };
        let briefs = state.briefs();
        assert_eq!(briefs[0].pending_questions, 1);
        assert_eq!(briefs[0].header.run_id, "r1");
        let activity = WorkflowActivity {
            chats: [("c".to_string(), briefs)].into(),
        };
        let json = serde_json::to_value(&activity).unwrap();
        assert_eq!(
            json["chats"]["c"][0]["runId"], "r1",
            "the header is flattened"
        );
        assert_eq!(json["chats"]["c"][0]["pendingQuestions"], 1);
        assert_eq!(
            serde_json::from_value::<WorkflowActivity>(json).unwrap(),
            activity
        );
        // an old or empty frame decodes to nothing rather than failing
        let empty: WorkflowActivity = serde_json::from_str("{}").unwrap();
        assert!(empty.chats.is_empty());
    }

    #[test]
    fn marker_text_reads_in_words() {
        assert_eq!(
            workflow_marker_text(WorkflowEventMarker::Completed, "Review", ""),
            "Workflow completed: Review"
        );
        assert_eq!(
            workflow_marker_text(WorkflowEventMarker::Stopped, "Review", "quota exceeded"),
            "Workflow stopped: Review — quota exceeded"
        );
    }

    #[test]
    fn entries_round_trip_even_though_nodes_have_their_own_kind_field() {
        let entries = vec![
            WorkflowEntry::Node(node("3:5-3:20", 2, NodePhase::Executing)),
            WorkflowEntry::Graph(WorkflowGraph::default()),
            WorkflowEntry::Report(WorkflowReport {
                index: 1,
                text: "x".into(),
                truncated: false,
                artifact_id: None,
                at: 3,
            }),
        ];
        for entry in entries {
            let json = serde_json::to_string(&entry).unwrap();
            let back: WorkflowEntry = serde_json::from_str(&json).unwrap();
            assert_eq!(back, entry, "{json}");
        }
        let json =
            serde_json::to_value(WorkflowEntry::Node(node("s", 0, NodePhase::Queued))).unwrap();
        assert_eq!(json["entry"], "node");
        assert_eq!(json["kind"], "ask", "the node's own kind is untouched");
    }

    #[test]
    fn origins_commands_and_approval_meta_round_trip_and_old_readers_fall_back() {
        let origin = crate::MessageOrigin::Workflow {
            run_id: "r".into(),
            name: "Demo".into(),
            status: WorkflowStatus::Completed,
        };
        let json = serde_json::to_value(&origin).unwrap();
        assert_eq!(json["kind"], "workflow");
        assert_eq!(
            serde_json::from_value::<crate::MessageOrigin>(json).unwrap(),
            origin
        );
        let marker = crate::MessageOrigin::WorkflowEvent {
            run_id: "r".into(),
            marker: WorkflowEventMarker::Stopped,
            name: "Demo".into(),
            detail: "quota".into(),
        };
        assert_eq!(
            serde_json::from_value::<crate::MessageOrigin>(serde_json::to_value(&marker).unwrap())
                .unwrap(),
            marker
        );
        // A newer kind this build does not know is `Unknown`, never an error.
        let future: crate::MessageOrigin =
            serde_json::from_str(r#"{"kind":"workflowCard","runId":"r"}"#).unwrap();
        assert_eq!(future, crate::MessageOrigin::Unknown);

        let command = WorkflowCommand::Answer {
            run_id: "r".into(),
            qid: "q".into(),
            answer: "yes".into(),
        };
        let json = serde_json::to_value(&command).unwrap();
        assert_eq!(json["action"], "answer");
        assert_eq!(
            serde_json::from_value::<WorkflowCommand>(json).unwrap(),
            command
        );

        let meta = WorkflowApprovalMeta {
            run_id: "r".into(),
            name: "Demo".into(),
            script_hash: "h".into(),
            draft_path: None,
            graph: WorkflowGraph::default(),
            max_concurrency: 4,
            budgets: WorkflowBudgets::default(),
            harness: None,
            model: None,
            excerpt: String::new(),
        };
        let back: WorkflowApprovalMeta =
            serde_json::from_value(serde_json::to_value(&meta).unwrap()).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn the_input_question_meta_is_additive() {
        // A question written before `meta` existed still decodes…
        let old: crate::UserInputQuestion =
            serde_json::from_str(r#"{"id":"q","header":"h","question":"?","options":["a"]}"#)
                .unwrap();
        assert!(old.meta.is_none());
        // …and one without meta serializes exactly as before (no key at all).
        assert!(serde_json::to_value(&old).unwrap().get("meta").is_none());
        let with = crate::UserInputQuestion {
            meta: Some(serde_json::json!({"kind": WORKFLOW_APPROVAL_META_KIND})),
            ..old
        };
        let back: crate::UserInputQuestion =
            serde_json::from_value(serde_json::to_value(&with).unwrap()).unwrap();
        assert_eq!(back.meta.unwrap()["kind"], "workflowApproval");
    }

    #[test]
    fn graphs_survive_the_wire_with_flattened_sites() {
        let graph = WorkflowGraph {
            phases: vec![GraphPhase {
                name: "review".into(),
                line: 5,
                col: 5,
                asks: vec![GraphAsk {
                    site: GraphSite {
                        site_id: "6:9-6:30".into(),
                        line: 6,
                        col: 9,
                        fan_out: true,
                    },
                    actor: Some("r".into()),
                    instructions_head: Some("Review".into()),
                    typed: true,
                    read_only: true,
                }],
                commands: vec![],
            }],
            actors: vec![GraphActor {
                site: GraphSite {
                    site_id: "4:9-4:20".into(),
                    line: 4,
                    col: 9,
                    fan_out: false,
                },
                name: Some("reviewer".into()),
                harness: None,
                model: None,
            }],
            commands: vec![],
            unphased_asks: 0,
        };
        let json = serde_json::to_value(&graph).unwrap();
        assert_eq!(json["phases"][0]["asks"][0]["siteId"], "6:9-6:30");
        assert_eq!(json["phases"][0]["asks"][0]["fanOut"], true);
        assert_eq!(
            serde_json::from_value::<WorkflowGraph>(json).unwrap(),
            graph
        );
    }
}
