//! Orchestrators: long-lived manager chats that run crews of threads, the
//! board their work lives on, the asks that need the person, and the Brain
//! dump that routes whatever the person says to the right place.
//!
//! Design and contract: `docs/design/orchestrators.md`. Everything here is a
//! wire type (camelCase JSON). Enums carry an `Other` variant so a value from
//! a newer peer never costs a row.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// `Option<Option<T>>` that keeps `null` apart from absent: absent = `None`
/// (unchanged), `null` = `Some(None)` (cleared).
pub mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn deserialize<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
    where
        T: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Option::<T>::deserialize(d).map(Some)
    }

    pub fn serialize<T, S>(v: &Option<Option<T>>, s: S) -> Result<S::Ok, S::Error>
    where
        T: Serialize,
        S: Serializer,
    {
        match v {
            Some(inner) => inner.serialize(s),
            None => s.serialize_none(),
        }
    }
}

// ── Orchestrators ───────────────────────────────────────────────────────────

/// A long-lived manager agent. It *is* a chat (`chat_id`, same id): its
/// transcript, harness and model are the chat's. This row adds what makes it
/// an orchestrator. Synced (registry kind `orchestrators`), LWW per field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Orchestrator {
    /// Equal to the chat id.
    pub id: String,
    pub kind: OrchestratorKind,
    pub name: String,
    /// What it is for, in the person's words (the charter's first line).
    #[serde(default)]
    pub goal: String,
    /// The orchestrator's own rules (Capy's AGENTS.md): which models for
    /// what, what needs approval, how many threads at once… Only it reads
    /// them.
    #[serde(default)]
    pub rules: String,
    /// Rules every crew thread receives with each prompt it is sent by its
    /// orchestrator (Capy's CREW.md).
    #[serde(default)]
    pub crew_rules: String,
    /// The plan and working notes, kept by the orchestrator itself.
    #[serde(default)]
    pub notes: String,
    /// Hue 0–359 that marks this orchestrator everywhere (map, board, dump
    /// receipts, sidebar). Seeded from the id at creation.
    pub hue: u16,
    #[serde(default)]
    pub crew: CrewPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake: Option<WakeSchedule>,
    /// No new crew work starts and scheduled wakes are skipped.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrchestratorKind {
    /// A person-made orchestrator for one area or goal.
    Standard,
    /// The profile's one Brain dump: routes the person's dumps.
    BrainDump,
    #[serde(other)]
    Other,
}

/// Where and how crew threads run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewPolicy {
    /// Devices crew threads may run on; empty = the orchestrator's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
    /// Crew threads working at once; `None` = no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_running: Option<u32>,
    /// Default harness/model for new crew threads; `None` = the
    /// orchestrator's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// When an orchestrator wakes on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeSchedule {
    /// Wake every N minutes (≥ 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_minutes: Option<u32>,
    /// Wake at these local times, "HH:MM".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub at: Vec<String>,
    #[serde(default)]
    pub weekdays_only: bool,
    /// What it is told when it wakes.
    #[serde(default)]
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_woke_at: Option<DateTime<Utc>>,
}

/// A thread in an orchestrator's crew. Synced (registry kind `crew`), keyed
/// by the thread's chat id: a thread is in at most one crew.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrewMember {
    /// The thread's chat id.
    pub chat_id: String,
    pub orchestrator_id: String,
    /// The board item it works on, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// Short role, e.g. "auth PR", "flaky test hunt".
    #[serde(default)]
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_report: Option<ReportHead>,
    /// Released from the crew (work done or handed back). Kept for history.
    #[serde(default)]
    pub released: bool,
    pub joined_at: DateTime<Utc>,
}

// ── Reports (crew → orchestrator) ───────────────────────────────────────────

/// A crew thread's report to its orchestrator. Never a user turn: the engine
/// files it in the orchestrator's transcript as a `crewReports` message part
/// and wakes the orchestrator only when it should (see the design doc).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub id: String,
    pub orchestrator_id: String,
    pub from_chat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub state: ReportState,
    /// One line, ≤ 140 chars.
    pub summary: String,
    /// Markdown detail, optional.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<WorkLink>,
    /// Progress 0–1 of its item, when the thread knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    pub at: DateTime<Utc>,
}

/// The newest report of a crew thread, as its crew row keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHead {
    pub id: String,
    pub state: ReportState,
    pub summary: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReportState {
    /// Still going; recorded, never wakes the orchestrator by itself.
    Progress,
    /// Finished what it was given.
    Done,
    /// Can't go on without something.
    Blocked,
    Failed,
    /// A plain message to the orchestrator (a crew thread's `send_message`
    /// to its orchestrator becomes one of these).
    Message,
    #[serde(other)]
    Other,
}

impl ReportState {
    /// Whether a report in this state wakes an idle orchestrator.
    pub fn wakes(self) -> bool {
        !matches!(self, ReportState::Progress | ReportState::Other)
    }
}

// ── Board ───────────────────────────────────────────────────────────────────

/// A unit of work on the board. Synced (registry kind `workItems`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    pub id: String,
    /// Owning orchestrator; `None` = loose (Brain dump inbox).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_id: Option<String>,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub state: WorkState,
    pub priority: Priority,
    /// Explicit progress 0–1. When `None`, clients derive it from the
    /// threads' todo progress (`Session::todo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    /// Threads working on it (chat ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<WorkLink>,
    /// The answer or result, once there is one — what the person reads in
    /// Answers. Markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// The dump fragment it came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RouteRef>,
    /// Sort key inside its column (smaller first).
    #[serde(default)]
    pub order: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkState {
    /// Captured, not planned yet.
    Inbox,
    /// Planned, not started ("Next").
    Planned,
    /// Being worked on ("Now").
    Active,
    Blocked,
    /// Waiting on review/merge/checks.
    Review,
    Done,
    Dropped,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    Urgent,
    High,
    Normal,
    Low,
}

impl Default for Priority {
    fn default() -> Self {
        Priority::Normal
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkLink {
    pub kind: LinkKind,
    pub label: String,
    /// URL, or chat id for `Chat`, branch name for `Branch`.
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkKind {
    Pr,
    Branch,
    Chat,
    Url,
    #[serde(other)]
    Other,
}

// ── Asks (anything that needs the person) ───────────────────────────────────

/// A question, decision or blocker for the person. Synced (registry kind
/// `asks`). Raising one notifies; answering delivers the answer to
/// `from_chat` as a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    pub id: String,
    pub from_chat: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub kind: AskKind,
    /// Markdown.
    pub question: String,
    /// Choices; empty = free text only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<AskOption>,
    #[serde(default)]
    pub urgent: bool,
    pub state: AskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskKind {
    Question,
    Decision,
    /// Permission to do something (merge, deploy, spend).
    Approval,
    Blocker,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AskState {
    Open,
    Answered,
    Dismissed,
    #[serde(other)]
    Other,
}

// ── Brain dump ──────────────────────────────────────────────────────────────

/// One thing the person dumped. Synced (registry kind `dumps`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dump {
    pub id: String,
    pub text: String,
    pub state: DumpState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    /// The Brain dump's one-line account of what it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub device_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DumpState {
    /// The Brain dump is reading it.
    Routing,
    /// Every fragment has a place.
    Routed,
    /// Some fragment is `Unsure`: the person picks.
    NeedsYou,
    Failed,
    #[serde(other)]
    Other,
}

/// Where one fragment of a dump went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub id: String,
    /// The fragment, quoted verbatim from the dump text.
    pub excerpt: String,
    /// Char offsets of `excerpt` in the dump text, when found (clients
    /// underline it). `None` when the fragment paraphrases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<(u32, u32)>,
    pub target: RouteTarget,
    pub intent: RouteIntent,
    /// What was sent to the target, rewritten for it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    pub state: RouteState,
    /// For `Unsure`: the likely places, best first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<RouteTarget>,
    /// The target's reply, once it answered (questions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RouteTarget {
    /// An existing orchestrator.
    Orchestrator { id: String },
    /// An existing chat/thread (any, crew or not).
    Chat { id: String },
    /// A new orchestrator the Brain dump created for it. (Empty while not
    /// made yet: an unsure route's candidate, a reroute asking for one.)
    NewOrchestrator {
        #[serde(default)]
        id: String,
    },
    /// A new standalone thread the Brain dump started for it. (Empty while
    /// not made yet, as above.)
    NewThread {
        #[serde(default)]
        id: String,
    },
    /// A loose board item (no owner yet).
    Item { id: String },
    /// Kept in the Brain dump's notes; nothing to do yet.
    Note,
    /// Not sure; see `Route::candidates`.
    Unsure,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteIntent {
    /// Something to do.
    Task,
    /// Context for whoever works on it.
    Info,
    /// The person wants an answer.
    Question,
    /// Reprioritise/reorder.
    Priority,
    /// Stop/cancel something.
    Cancel,
    Note,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteState {
    /// Decided, not delivered yet.
    Pending,
    Delivered,
    /// The target answered (questions) or took it on.
    Answered,
    /// The person moved or undid it.
    Undone,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRef {
    pub dump_id: String,
    pub route_id: String,
}

// ── Live: what a thread is doing, how far it is ─────────────────────────────

/// What a chat's agent is doing right now, classified from its running tool
/// (see [`classify_command`]). Host-written on the `Session` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatActivity {
    pub kind: ActivityKind,
    /// Short label, e.g. "cargo test -p zeron-engine", "Edit sessions.rs".
    pub label: String,
    pub since: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActivityKind {
    Thinking,
    Reading,
    Editing,
    Running,
    Testing,
    Building,
    Installing,
    Merging,
    Pushing,
    Reviewing,
    Browsing,
    Delegating,
    #[serde(other)]
    Other,
}

/// A thread's todo progress, from its newest todo/plan tool call.
/// Host-written on the `Session` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TodoProgress {
    pub done: u32,
    pub total: u32,
    /// The item in progress, when one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
}

/// Classify a shell command line into an activity. Pure; shared by every
/// frontend and the engine.
pub fn classify_command(command: &str) -> ActivityKind {
    let c = command.trim().to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| c.contains(n));
    if has(&[
        "gh pr merge",
        "git merge",
        "git rebase",
        "git cherry-pick",
        "merge_pull",
    ]) {
        ActivityKind::Merging
    } else if has(&["git push", "gh pr create", "gh release"]) {
        ActivityKind::Pushing
    } else if has(&[
        "cargo test",
        "cargo nextest",
        "npm test",
        "npm run test",
        "pnpm test",
        "yarn test",
        "bun test",
        "pytest",
        "go test",
        "jest",
        "vitest",
        "zgx test",
        "xcodebuild test",
        "gradle test",
        "./gradlew test",
        "playwright test",
    ]) {
        ActivityKind::Testing
    } else if has(&[
        "cargo build",
        "cargo check",
        "cargo clippy",
        "npm run build",
        "pnpm build",
        "yarn build",
        "bun run build",
        "tsc",
        "make",
        "cmake",
        "xcodebuild",
        "gradle",
        "go build",
        "zgx check",
        "zgx lint",
    ]) {
        ActivityKind::Building
    } else if has(&[
        "npm install",
        "npm i ",
        "pnpm install",
        "yarn install",
        "bun install",
        "pip install",
        "uv pip",
        "cargo install",
        "cargo fetch",
        "brew install",
        "apt ",
        "pacman ",
    ]) {
        ActivityKind::Installing
    } else if has(&["gh pr view", "gh pr diff", "git diff", "git log", "git show"]) {
        ActivityKind::Reviewing
    } else {
        ActivityKind::Running
    }
}

// ── Live: machines and communications ───────────────────────────────────────

/// How busy a device is, sampled while someone watches (`WatchDeviceLoads`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLoad {
    pub device_id: String,
    /// 0–1 across all cores; `None` when the device didn't answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_used_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<u32>,
    pub online: bool,
    pub at: DateTime<Utc>,
}

/// One message moving between agents (or between the person and an agent),
/// for the live map and the comms feed. Ephemeral (`WatchComms`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommEvent {
    pub id: String,
    pub kind: CommKind,
    /// Chat id, or "you".
    pub from: String,
    /// Chat id, or "you".
    pub to: String,
    /// One line.
    pub summary: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CommKind {
    /// An orchestrator (or any agent) sent a thread a message / task.
    Message,
    /// A crew thread reported.
    Report,
    /// Something asked the person.
    Ask,
    /// The person answered an ask.
    Answer,
    /// The Brain dump routed a fragment.
    Route,
    /// A thread was spawned into a crew.
    Spawn,
    #[serde(other)]
    Other,
}

// ── Snapshot + ops (RPC) ────────────────────────────────────────────────────

/// Everything orchestration, as `WatchOrchestration` streams it: the full
/// state on subscribe and again (coalesced ≤ 4/s) on every change.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationState {
    pub orchestrators: Vec<Orchestrator>,
    pub crew: Vec<CrewMember>,
    pub items: Vec<WorkItem>,
    pub asks: Vec<Ask>,
    /// Newest first, at most 50.
    pub dumps: Vec<Dump>,
}

/// `Orchestrate` RPC params: one op. Ops marked (agent) are also allowed
/// for agent callers (the Zeron MCP), scoped to the caller's own chat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum OrchestrateOp {
    /// Create an orchestrator: a new chat + its row. Replies `{ id }`.
    /// With `prompt`, its first turn starts at once. (agent: Brain dump only)
    #[serde(rename_all = "camelCase")]
    CreateOrchestrator {
        name: String,
        #[serde(default)]
        goal: String,
        #[serde(default)]
        rules: String,
        #[serde(default)]
        crew_rules: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default)]
        crew: CrewPolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompt: Option<String>,
    },
    /// Make an existing chat an orchestrator (the person's "make this an
    /// orchestrator"). Replies `{ id }`.
    #[serde(rename_all = "camelCase")]
    PromoteChat { chat_id: String, name: String },
    /// Patch fields; absent = unchanged. (agent: own `notes` only)
    #[serde(rename_all = "camelCase")]
    UpdateOrchestrator {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rules: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crew_rules: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        crew: Option<CrewPolicy>,
        /// `Some(None)` clears it.
        #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
        wake: Option<Option<WakeSchedule>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        paused: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archived: Option<bool>,
    },
    /// Put an existing thread into a crew (moves it if it was in another).
    /// (agent: orchestrators, into their own crew)
    #[serde(rename_all = "camelCase")]
    AddToCrew {
        orchestrator_id: String,
        chat_id: String,
        #[serde(default)]
        role: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
    },
    /// (agent: orchestrators, own crew)
    #[serde(rename_all = "camelCase")]
    ReleaseFromCrew { chat_id: String },
    /// Create (no `id`) or patch an item. Replies `{ id }`. (agent)
    #[serde(rename_all = "camelCase")]
    UpsertItem { item: WorkItemPatch },
    /// (agent) — a crew thread files a report to its orchestrator.
    #[serde(rename_all = "camelCase")]
    Report {
        state: ReportState,
        summary: String,
        #[serde(default)]
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        links: Vec<WorkLink>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<f32>,
    },
    /// (agent) — raise an ask for the person. Replies `{ id }`.
    #[serde(rename_all = "camelCase")]
    Ask {
        kind: AskKind,
        question: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        options: Vec<AskOption>,
        #[serde(default)]
        urgent: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        item_id: Option<String>,
    },
    /// The person answers; the answer goes to the asker as a message.
    #[serde(rename_all = "camelCase")]
    AnswerAsk { id: String, answer: String },
    DismissAsk { id: String },
    /// The person dumps text. Replies `{ id }`; routing runs in the Brain
    /// dump's chat.
    Dump { text: String },
    /// (agent: Brain dump) record one route of a dump and deliver it.
    #[serde(rename_all = "camelCase")]
    Route {
        dump_id: String,
        excerpt: String,
        target: RouteTarget,
        intent: RouteIntent,
        #[serde(default)]
        message: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        candidates: Vec<RouteTarget>,
    },
    /// (agent: Brain dump) the dump is fully routed.
    #[serde(rename_all = "camelCase")]
    DumpDone { dump_id: String, summary: String },
    /// The person moves a route to another target (resolves `Unsure` too):
    /// the old delivery gets a short retraction, the new target the message.
    #[serde(rename_all = "camelCase")]
    Reroute {
        dump_id: String,
        route_id: String,
        target: RouteTarget,
    },
}

/// Fields of a work item to set; `id: None` creates.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `Some(None)` = make it loose.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub orchestrator_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<WorkState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub progress: Option<Option<f32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<WorkLink>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]
    pub outcome: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RouteRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
}

/// Hue for a new orchestrator, spread by the golden angle from its id so
/// neighbours differ.
pub fn seed_hue(id: &str) -> u16 {
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    ((h as f64 * 137.508) % 360.0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_commands() {
        assert_eq!(classify_command("cargo test -p zeron-engine --lib"), ActivityKind::Testing);
        assert_eq!(classify_command("gh pr merge 1383 --squash"), ActivityKind::Merging);
        assert_eq!(classify_command("git push -u origin HEAD"), ActivityKind::Pushing);
        assert_eq!(classify_command("cargo build --release"), ActivityKind::Building);
        assert_eq!(classify_command("pnpm install"), ActivityKind::Installing);
        assert_eq!(classify_command("ls -la"), ActivityKind::Running);
    }

    #[test]
    fn unknown_enum_values_decode_as_other() {
        let s: WorkState = serde_json::from_str("\"someday\"").unwrap();
        assert_eq!(s, WorkState::Other);
        let t: RouteTarget = serde_json::from_str(r#"{"kind":"planet","id":"x"}"#).unwrap();
        assert_eq!(t, RouteTarget::Other);
        // A candidate not made yet carries no id.
        let t: RouteTarget = serde_json::from_str(r#"{"kind":"newThread"}"#).unwrap();
        assert_eq!(t, RouteTarget::NewThread { id: String::new() });
    }

    #[test]
    fn double_option_tells_null_from_absent() {
        let p: WorkItemPatch = serde_json::from_str(r#"{"outcome":null}"#).unwrap();
        assert_eq!(p.outcome, Some(None));
        let p: WorkItemPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(p.outcome, None);
    }

    #[test]
    fn ops_round_trip() {
        let op = OrchestrateOp::Dump { text: "fix the auth thing".into() };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"op":"dump","text":"fix the auth thing"}"#);
        assert_eq!(serde_json::from_str::<OrchestrateOp>(&json).unwrap(), op);
    }
}
