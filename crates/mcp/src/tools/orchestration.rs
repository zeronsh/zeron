//! Orchestration tools: what an orchestrator, its crew threads and the Brain
//! dump use to run work (docs/design/orchestrators.md, "MCP tools"). Each is
//! one `Orchestrate` op or a read of `WatchOrchestration`; the engine scopes
//! every op to the calling chat again, so the filtering here only keeps each
//! agent's tool list to what it may use.
//!
//! The caller's role comes from the orchestration rows (an orchestrator's or
//! Brain dump's row has its chat's id; a crew row has the thread's), read
//! when the server starts and again when a tool it lacks is called.

use serde::Deserialize;
use serde_json::{Value, json};
use zeron_proto::orchestration::{
    AskKind, AskOption, OrchestrationState, OrchestratorKind, ReportState, RouteIntent, RouteTarget, WorkItemPatch,
    WorkLink, WorkState,
};
use zeron_rpc::methods;

use super::{ToolDef, Tools, parse, parse_enum, session_for, status_of};

pub(super) const REPORT: &str = "report";
pub(super) const ASK: &str = "ask";
pub(super) const BOARD: &str = "board";
pub(super) const ITEM_UPSERT: &str = "item_upsert";
pub(super) const CREW_ADD: &str = "crew_add";
pub(super) const CREW_RELEASE: &str = "crew_release";
pub(super) const NOTES_SET: &str = "notes_set";
pub(super) const WORLD: &str = "world";
pub(super) const ROUTE: &str = "route";
pub(super) const DUMP_DONE: &str = "dump_done";
pub(super) const ORCHESTRATOR_CREATE: &str = "orchestrator_create";

pub(super) const NAMES: [&str; 11] = [
    REPORT,
    ASK,
    BOARD,
    ITEM_UPSERT,
    CREW_ADD,
    CREW_RELEASE,
    NOTES_SET,
    WORLD,
    ROUTE,
    DUMP_DONE,
    ORCHESTRATOR_CREATE,
];

/// What the calling chat is to orchestration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Role {
    #[default]
    Plain,
    Orchestrator {
        id: String,
    },
    BrainDump {
        id: String,
    },
    Crew {
        orchestrator_id: String,
    },
}

impl Role {
    /// The role of `chat` in `state`: from its rows only.
    pub fn of(chat: Option<&str>, state: &OrchestrationState) -> Self {
        let Some(chat) = chat else { return Role::Plain };
        if let Some(o) = state.orchestrators.iter().find(|o| o.id == chat) {
            return match o.kind {
                OrchestratorKind::BrainDump => Role::BrainDump { id: o.id.clone() },
                OrchestratorKind::Standard => Role::Orchestrator { id: o.id.clone() },
                OrchestratorKind::Other => Role::Plain,
            };
        }
        match state.crew.iter().find(|m| m.chat_id == chat && !m.released) {
            Some(m) => Role::Crew {
                orchestrator_id: m.orchestrator_id.clone(),
            },
            None => Role::Plain,
        }
    }

    /// Whether this role is offered `tool` (one of [`NAMES`]).
    pub fn offers(&self, tool: &str) -> bool {
        match tool {
            ASK => true,
            REPORT => matches!(self, Role::Crew { .. }),
            BOARD | ITEM_UPSERT => matches!(self, Role::Orchestrator { .. } | Role::BrainDump { .. }),
            CREW_ADD | CREW_RELEASE | NOTES_SET => matches!(self, Role::Orchestrator { .. }),
            WORLD | ROUTE | DUMP_DONE | ORCHESTRATOR_CREATE => matches!(self, Role::BrainDump { .. }),
            _ => true,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Role::Plain => "plain",
            Role::Orchestrator { .. } => "orchestrator",
            Role::BrainDump { .. } => "brainDump",
            Role::Crew { .. } => "crew",
        }
    }
}

fn s(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn links_schema() -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["pr", "branch", "chat", "url"] },
                "label": { "type": "string" },
                "target": { "type": "string", "description": "URL; the chat id for chat, the branch name for branch." }
            },
            "required": ["kind", "label", "target"]
        }
    })
}

fn target_schema(description: &str) -> Value {
    json!({
        "type": "object",
        "description": description,
        "properties": {
            "kind": { "type": "string", "enum": ["orchestrator", "chat", "newOrchestrator", "newThread", "item", "note", "unsure"] },
            "id": { "type": "string", "description": "The orchestrator, chat or item id (not for note or unsure; for newOrchestrator/newThread, the id orchestrator_create or create_chat returned)." }
        },
        "required": ["kind"]
    })
}

const STATES: [&str; 7] = ["inbox", "planned", "active", "blocked", "review", "done", "dropped"];

pub(super) fn catalog() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: REPORT,
            description: "Report to your orchestrator (you are on its crew). Use done, blocked or failed when it happens and progress at milestones; message for anything else it should know. Never send_message your orchestrator: a report is filed as a compact Crew update, and only done, blocked, failed and message wake it (progress is recorded quietly). The summary is one line (140 chars at most); detail is optional markdown.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "state": { "type": "string", "enum": ["progress", "done", "blocked", "failed", "message"] },
                    "summary": s("One line: what happened."),
                    "detail": s("Optional markdown: what changed, what you need, what's next."),
                    "item": s("Board item id, when not the one you were given."),
                    "links": links_schema(),
                    "progress": { "type": "number", "minimum": 0, "maximum": 1, "description": "How far your item is, 0 to 1." }
                },
                "required": ["state", "summary"]
            }),
        },
        ToolDef {
            name: ASK,
            description: "Ask the person something only they can decide: a question, a decision between options, an approval (merge, deploy, spend) or a blocker. It joins the one queue the person answers from, with a notification; the answer comes back to your chat as a message ([Answer from you] …). Ask only when it's a real decision; keep working on anything that doesn't depend on it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": s("Markdown, self-contained: the person reads it without your transcript."),
                    "kind": { "type": "string", "enum": ["question", "decision", "approval", "blocker"], "default": "question" },
                    "options": {
                        "type": "array",
                        "description": "Choices, best first. Strings, or {label, detail}. Omit for a free answer.",
                        "items": { "anyOf": [{ "type": "string" }, { "type": "object", "properties": { "label": { "type": "string" }, "detail": { "type": "string" } }, "required": ["label"] }] }
                    },
                    "urgent": { "type": "boolean", "default": false },
                    "item": s("The board item it is about.")
                },
                "required": ["question"]
            }),
        },
        ToolDef {
            name: BOARD,
            description: "Your board: work items (state, priority, progress, threads, links, outcome), your crew (each thread's status, what it's doing now, todo progress, last report) and the open asks. The Brain dump sees every board.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "open (default: everything not done or dropped), all, or one state: inbox, planned, active, blocked, review, done, dropped." }
                }
            }),
        },
        ToolDef {
            name: ITEM_UPSERT,
            description: "Create a board item (no id; title required) or change one (id plus the fields to change). States: inbox, planned (next), active (now), blocked, review, done, dropped. Keep states true, and put the answer or result in outcome when there is one: that is what the person reads.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": s("The item to change. Omit to create."),
                    "title": s("Short title."),
                    "detail": s("Markdown."),
                    "state": { "type": "string", "enum": STATES },
                    "priority": { "type": "string", "enum": ["urgent", "high", "normal", "low"] },
                    "progress": { "type": ["number", "null"], "minimum": 0, "maximum": 1, "description": "0 to 1; null derives it from the threads' todo lists." },
                    "outcome": { "type": ["string", "null"], "description": "The answer or result (markdown); null clears it." },
                    "threads": { "type": "array", "items": { "type": "string" }, "description": "Chat ids working on it." },
                    "links": links_schema(),
                    "order": { "type": "number", "description": "Sort key in its column, smaller first." },
                    "orchestrator": s("Brain dump only: the orchestrator that owns it (id); omit for a loose item.")
                }
            }),
        },
        ToolDef {
            name: CREW_ADD,
            description: "Put an existing thread into your crew (it reports to you from then on). New threads you create with create_chat join it already.",
            input_schema: json!({
                "type": "object",
                "properties": { "chat": s("Chat id, unique id prefix, or exact title."), "role": s("Short role, e.g. \"auth PR\"."), "item": s("The board item it works on.") },
                "required": ["chat"]
            }),
        },
        ToolDef {
            name: CREW_RELEASE,
            description: "Release a thread from your crew (its work is done or handed back). Kept for history.",
            input_schema: json!({ "type": "object", "properties": { "chat": s("Chat id, unique id prefix, or exact title.") }, "required": ["chat"] }),
        },
        ToolDef {
            name: NOTES_SET,
            description: "Replace your notes: your plan and working notes, kept across turns and shown to the person.",
            input_schema: json!({ "type": "object", "properties": { "text": s("Markdown.") }, "required": ["text"] }),
        },
        ToolDef {
            name: WORLD,
            description: "A compact snapshot to route a dump with: every orchestrator (id, name, goal, its crew threads with their items, and its open items), recent top-level chats (id, title, status, project, device) and the devices. A task, priority or cancel for a crew thread goes to its orchestrator.",
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        ToolDef {
            name: ROUTE,
            description: "Route one fragment of a dump and deliver it at once. excerpt is the fragment quoted verbatim from the dump; message is what the target is sent, rewritten so it can act without the rest of the dump. Targets: orchestrator or chat (existing, by id), newOrchestrator (create it first with orchestrator_create) or newThread (create it first with create_chat), item (a loose board item: create it with item_upsert), note (kept in your notes), unsure (with candidates, best first: the person picks).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dump": s("The dump id ([Dump <id>])."),
                    "excerpt": s("The fragment, verbatim from the dump."),
                    "target": target_schema("Where it goes."),
                    "intent": { "type": "string", "enum": ["task", "info", "question", "priority", "cancel", "note"] },
                    "message": s("What the target is sent."),
                    "candidates": { "type": "array", "items": target_schema("A likely place."), "description": "For unsure: the likely places, best first." }
                },
                "required": ["dump", "excerpt", "target", "intent"]
            }),
        },
        ToolDef {
            name: DUMP_DONE,
            description: "Finish a dump once every fragment is routed, with a one-line summary of what went where.",
            input_schema: json!({ "type": "object", "properties": { "dump": s("The dump id."), "summary": s("One line.") }, "required": ["dump", "summary"] }),
        },
        ToolDef {
            name: ORCHESTRATOR_CREATE,
            description: "Create an orchestrator for a new, lasting area of work (a one-off task is a thread: create_chat). Returns its id; with prompt, its first turn starts at once.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": s("Short name."),
                    "goal": s("What it is for, in the person's words."),
                    "rules": s("Its own rules (models, approvals, how many threads)."),
                    "project": s("Project id, path or name it works in."),
                    "device": s("Host device (id or name)."),
                    "prompt": s("Its first message.")
                },
                "required": ["name", "goal"]
            }),
        },
    ]
}

#[derive(Deserialize)]
struct ReportArgs {
    state: String,
    summary: String,
    #[serde(default)]
    detail: String,
    item: Option<String>,
    #[serde(default)]
    links: Vec<WorkLink>,
    progress: Option<f64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OptionArg {
    Label(String),
    Full {
        label: String,
        #[serde(default)]
        detail: String,
    },
}

#[derive(Deserialize)]
struct AskArgs {
    question: String,
    kind: Option<String>,
    #[serde(default)]
    options: Vec<OptionArg>,
    #[serde(default)]
    urgent: bool,
    item: Option<String>,
}

#[derive(Deserialize, Default)]
struct BoardArgs {
    filter: Option<String>,
}

#[derive(Deserialize)]
struct ItemArgs {
    id: Option<String>,
    title: Option<String>,
    detail: Option<String>,
    state: Option<String>,
    priority: Option<String>,
    #[serde(default, with = "zeron_proto::orchestration::double_option")]
    progress: Option<Option<f64>>,
    #[serde(default, with = "zeron_proto::orchestration::double_option")]
    outcome: Option<Option<String>>,
    threads: Option<Vec<String>>,
    links: Option<Vec<WorkLink>>,
    order: Option<f64>,
    orchestrator: Option<String>,
}

#[derive(Deserialize)]
struct CrewAddArgs {
    chat: String,
    #[serde(default)]
    role: String,
    item: Option<String>,
}

#[derive(Deserialize)]
struct ChatOnly {
    chat: String,
}

#[derive(Deserialize)]
struct NotesArgs {
    text: String,
}

#[derive(Deserialize)]
struct RouteArgs {
    dump: String,
    excerpt: String,
    target: RouteTarget,
    intent: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    candidates: Vec<RouteTarget>,
}

#[derive(Deserialize)]
struct DumpDoneArgs {
    dump: String,
    summary: String,
}

#[derive(Deserialize)]
struct CreateArgs {
    name: String,
    goal: String,
    #[serde(default)]
    rules: String,
    project: Option<String>,
    device: Option<String>,
    prompt: Option<String>,
}

fn nonempty(what: &str, value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{what} is empty"));
    }
    Ok(value.to_owned())
}

fn fraction(what: &str, value: f64) -> Result<f32, String> {
    if !(0.0..=1.0).contains(&value) {
        return Err(format!("{what} must be between 0 and 1"));
    }
    Ok(value as f32)
}

/// A tool's arguments as the `Orchestrate` op they become, or what's wrong
/// with them. Pure (no lookups), so it's what the tests check.
pub(super) fn op_for(tool: &str, args: Value, role: &Role) -> Result<Value, String> {
    let own = |role: &Role| match role {
        Role::Orchestrator { id } | Role::BrainDump { id } => Some(id.clone()),
        _ => None,
    };
    match tool {
        REPORT => {
            let a: ReportArgs = parse(args)?;
            let state: ReportState = parse_enum("report state", &a.state)?;
            if state == ReportState::Other {
                return Err(format!("unknown report state: {:?}", a.state));
            }
            let summary = nonempty("summary", &a.summary)?;
            if summary.contains('\n') {
                return Err("summary is one line: put the rest in detail".into());
            }
            let progress = a.progress.map(|p| fraction("progress", p)).transpose()?;
            Ok(json!({
                "op": "report", "state": state, "summary": summary, "detail": a.detail,
                "itemId": a.item, "links": a.links, "progress": progress,
            }))
        }
        ASK => {
            let a: AskArgs = parse(args)?;
            let kind: AskKind = match a.kind.as_deref() {
                Some(raw) => parse_enum("ask kind", raw)?,
                None => AskKind::Question,
            };
            if kind == AskKind::Other {
                return Err(format!("unknown ask kind: {:?}", a.kind.unwrap_or_default()));
            }
            let options: Vec<AskOption> = a
                .options
                .into_iter()
                .map(|o| match o {
                    OptionArg::Label(label) => AskOption { label, detail: String::new() },
                    OptionArg::Full { label, detail } => AskOption { label, detail },
                })
                .filter(|o| !o.label.trim().is_empty())
                .collect();
            Ok(json!({
                "op": "ask", "kind": kind, "question": nonempty("question", &a.question)?,
                "options": options, "urgent": a.urgent, "itemId": a.item,
            }))
        }
        ITEM_UPSERT => {
            let a: ItemArgs = parse(args)?;
            let state = match a.state.as_deref() {
                Some(raw) => {
                    let state: WorkState = parse_enum("item state", raw)?;
                    if state == WorkState::Other {
                        return Err(format!("unknown item state: {raw:?}"));
                    }
                    Some(state)
                }
                None => None,
            };
            let priority = a.priority.as_deref().map(|raw| parse_enum("priority", raw)).transpose()?;
            let progress = match a.progress {
                Some(Some(p)) => Some(Some(fraction("progress", p)?)),
                Some(None) => Some(None),
                None => None,
            };
            let id = a.id.map(|id| id.trim().to_owned()).filter(|id| !id.is_empty());
            if id.is_none() && a.title.as_deref().is_none_or(|t| t.trim().is_empty()) {
                return Err("a new item needs a title".into());
            }
            let orchestrator_id = match (&a.orchestrator, role) {
                (Some(o), Role::BrainDump { .. }) => Some(Some(o.clone())),
                (Some(_), _) => return Err("only the Brain dump sets an item's orchestrator".into()),
                (None, Role::Orchestrator { id: mine }) if id.is_none() => Some(Some(mine.clone())),
                (None, _) => None,
            };
            let item = WorkItemPatch {
                id,
                orchestrator_id,
                title: a.title,
                detail: a.detail,
                state,
                priority,
                progress,
                threads: a.threads,
                links: a.links,
                outcome: a.outcome,
                source: None,
                order: a.order.filter(|o| o.is_finite()),
            };
            Ok(json!({ "op": "upsertItem", "item": item }))
        }
        NOTES_SET => {
            let a: NotesArgs = parse(args)?;
            let id = own(role).ok_or("only an orchestrator keeps notes")?;
            Ok(json!({ "op": "updateOrchestrator", "id": id, "notes": a.text }))
        }
        ROUTE => {
            let a: RouteArgs = parse(args)?;
            let intent: RouteIntent = parse_enum("intent", &a.intent)?;
            if intent == RouteIntent::Other || a.target == RouteTarget::Other {
                return Err("unknown intent or target kind".into());
            }
            let needs_id = !matches!(a.target, RouteTarget::Note | RouteTarget::Unsure);
            let has_id = match &a.target {
                RouteTarget::Orchestrator { id }
                | RouteTarget::Chat { id }
                | RouteTarget::NewOrchestrator { id }
                | RouteTarget::NewThread { id }
                | RouteTarget::Item { id } => !id.trim().is_empty(),
                _ => false,
            };
            if needs_id && !has_id {
                return Err("this target needs an id".into());
            }
            if a.target == RouteTarget::Unsure && a.candidates.is_empty() {
                return Err("unsure needs candidates".into());
            }
            if needs_id && matches!(a.target, RouteTarget::Orchestrator { .. } | RouteTarget::Chat { .. } | RouteTarget::NewOrchestrator { .. } | RouteTarget::NewThread { .. }) && a.message.trim().is_empty() {
                return Err("message is empty: say what the target should do".into());
            }
            Ok(json!({
                "op": "route", "dumpId": nonempty("dump", &a.dump)?, "excerpt": nonempty("excerpt", &a.excerpt)?,
                "target": a.target, "intent": intent, "message": a.message, "candidates": a.candidates,
            }))
        }
        DUMP_DONE => {
            let a: DumpDoneArgs = parse(args)?;
            Ok(json!({ "op": "dumpDone", "dumpId": nonempty("dump", &a.dump)?, "summary": nonempty("summary", &a.summary)? }))
        }
        other => Err(format!("{other} has no plain op")),
    }
}

/// The built-in instructions for `role` (docs/design/orchestrators.md,
/// "Built-in prompts"), from its rows.
pub fn role_prompt(role: &Role, chat: Option<&str>, state: &OrchestrationState) -> Option<String> {
    use zeron_proto::orchestration::role_prompts;
    match role {
        Role::Plain => None,
        Role::Orchestrator { id } => {
            let o = state.orchestrators.iter().find(|o| &o.id == id)?;
            Some(role_prompts::orchestrator(&o.name, &o.goal, &o.rules))
        }
        Role::BrainDump { .. } => Some(role_prompts::BRAIN_DUMP.to_owned()),
        Role::Crew { orchestrator_id } => {
            let o = state.orchestrators.iter().find(|o| &o.id == orchestrator_id)?;
            let item = state
                .crew
                .iter()
                .find(|m| Some(m.chat_id.as_str()) == chat && &m.orchestrator_id == orchestrator_id)
                .and_then(|m| m.item_id.as_deref())
                .and_then(|id| state.items.iter().find(|i| i.id == id))
                .map(|i| i.title.as_str());
            Some(role_prompts::crew(&o.name, item, &o.crew_rules))
        }
    }
}

/// Why a new crew thread can't start now, or `None`: its orchestrator is
/// paused, or `running` of its threads already work and that's its limit.
pub(super) fn crew_refusal(orchestrator: &zeron_proto::orchestration::Orchestrator, running: usize, starts: bool) -> Option<String> {
    if orchestrator.paused {
        return Some(format!("{} is paused: no new crew work starts until the person resumes it.", orchestrator.name));
    }
    match orchestrator.crew.max_running {
        Some(max) if starts && running >= max as usize => Some(format!(
            "{}'s crew is full: {running} of {max} threads are working. Wait for one to report done (or release one with crew_release), then start the next.",
            orchestrator.name
        )),
        _ => None,
    }
}

/// Crew threads of `orchestrator` that are working (or waiting on the
/// person) now.
pub(super) fn crew_running(
    state: &OrchestrationState,
    orchestrator: &str,
    chats: &[zeron_proto::Chat],
    sessions: &[zeron_proto::Session],
) -> usize {
    state
        .crew
        .iter()
        .filter(|m| m.orchestrator_id == orchestrator && !m.released)
        .filter(|m| {
            chats.iter().find(|c| c.id == m.chat_id).is_some_and(|chat| {
                session_for(sessions, chat).is_some_and(|s| {
                    matches!(s.status, zeron_proto::SessionStatus::Working | zeron_proto::SessionStatus::AwaitingInput)
                })
            })
        })
        .count()
}

impl Tools {
    /// A new crew thread of `orchestrator` as its crew policy says: where it
    /// may run (the first allowed device when none is named), its default
    /// harness and model, its item (on this board only), and no start past
    /// `maxRunning` (a clear refusal instead).
    pub(super) async fn apply_crew_policy(&self, orchestrator: &str, args: &mut super::CreateChatArgs) -> anyhow::Result<()> {
        let Ok(state) = self.orchestration_state().await else {
            return Ok(()); // an engine without orchestration has no policy
        };
        let Some(row) = state.orchestrators.iter().find(|o| o.id == orchestrator) else {
            return Ok(());
        };
        let policy = &row.crew;
        let starts = args.prompt.as_deref().is_some_and(|p| !p.trim().is_empty());
        let (chats, sessions) = tokio::try_join!(self.zeron.chats(), self.zeron.sessions())?;
        if let Some(refusal) = crew_refusal(row, crew_running(&state, orchestrator, &chats, &sessions), starts) {
            anyhow::bail!(refusal);
        }
        if let Some(item) = args.item.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
            anyhow::ensure!(
                state.items.iter().any(|i| i.id == item && i.orchestrator_id.as_deref() == Some(orchestrator)),
                "No item {item} on {}'s board (see board)",
                row.name
            );
        }
        if !policy.devices.is_empty() && !args.cloud {
            if args.device.is_none() && args.project.is_none() {
                args.device = Some(policy.devices[0].clone());
            } else {
                let (_, device) = self.zeron.resolve_target(args.project.as_deref(), args.device.as_deref()).await?;
                anyhow::ensure!(
                    policy.devices.contains(&device),
                    "{}'s crew runs on {} only (its crew policy)",
                    row.name,
                    policy.devices.join(", ")
                );
            }
        }
        if args.harness.is_none() {
            args.harness = policy.harness.clone();
            if args.model.is_none() {
                args.model = policy.model.clone();
            }
        } else if args.model.is_none() && args.harness == policy.harness {
            args.model = policy.model.clone();
        }
        Ok(())
    }

    /// Read the caller's role from the orchestration rows (plain when the
    /// engine predates them or the server speaks for no chat).
    pub async fn resolve_role(&self) -> Role {
        let chat = self.zeron.origin().chat_id.clone();
        let (role, prompt) = match chat {
            None => (Role::Plain, None),
            Some(_) => match self.orchestration_state().await {
                Ok(state) => {
                    let role = Role::of(chat.as_deref(), &state);
                    let prompt = role_prompt(&role, chat.as_deref(), &state);
                    (role, prompt)
                }
                Err(_) => (Role::Plain, None),
            },
        };
        *self.role.write().unwrap_or_else(|e| e.into_inner()) = Some(role.clone());
        *self.role_prompt.write().unwrap_or_else(|e| e.into_inner()) = prompt;
        role
    }

    /// Zeron's built-in instructions for this chat's role, when it has one.
    pub fn role_instructions(&self) -> Option<String> {
        self.role_prompt.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// See [`crew_wait_note`].
    pub(super) fn crew_wait_note(&self, chat: &zeron_proto::Chat) -> Option<Value> {
        crew_wait_note(&self.role(), chat)
    }

    /// The role as last read.
    pub(super) fn role(&self) -> Role {
        self.role.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
    }

    async fn orchestration_state(&self) -> anyhow::Result<OrchestrationState> {
        let value = self.zeron.snapshot(methods::WATCH_ORCHESTRATION, json!({})).await?;
        Ok(serde_json::from_value(value)?)
    }

    async fn orchestrate(&self, op: Value) -> Result<Value, String> {
        self.zeron.call(methods::ORCHESTRATE, op).await.map_err(|e| format!("{e:#}"))
    }

    pub(super) async fn orchestration_tool(&self, tool: &str, args: Value) -> Result<Value, String> {
        let mut role = self.role();
        if !role.offers(tool) {
            // Made an orchestrator (or put in a crew) since this server started.
            role = self.resolve_role().await;
            if !role.offers(tool) {
                return Err(format!("{tool} isn't for this chat ({} chat)", role.label()));
            }
        }
        match tool {
            REPORT | ASK | ITEM_UPSERT | NOTES_SET | ROUTE | DUMP_DONE => {
                let op = op_for(tool, args, &role)?;
                self.orchestrate(op).await
            }
            CREW_ADD => {
                let a: CrewAddArgs = parse(args)?;
                let Role::Orchestrator { id } = &role else { unreachable!() };
                let chat = self.zeron.resolve_chat(&a.chat).await.map_err(|e| format!("{e:#}"))?;
                self.orchestrate(json!({
                    "op": "addToCrew", "orchestratorId": id, "chatId": chat.id, "role": a.role, "itemId": a.item,
                }))
                .await
            }
            CREW_RELEASE => {
                let a: ChatOnly = parse(args)?;
                let chat = self.zeron.resolve_chat(&a.chat).await.map_err(|e| format!("{e:#}"))?;
                self.orchestrate(json!({ "op": "releaseFromCrew", "chatId": chat.id })).await
            }
            BOARD => {
                let a: BoardArgs = parse(args)?;
                self.board(&role, a.filter.as_deref()).await.map_err(|e| format!("{e:#}"))
            }
            WORLD => self.world().await.map_err(|e| format!("{e:#}")),
            ORCHESTRATOR_CREATE => {
                let a: CreateArgs = parse(args)?;
                let (space, device) = match (a.project.as_deref(), a.device.as_deref()) {
                    (None, None) => (None, None),
                    (project, device) => {
                        let (space, device) =
                            self.zeron.resolve_target(project, device).await.map_err(|e| format!("{e:#}"))?;
                        (space.map(|s| s.id), Some(device))
                    }
                };
                self.orchestrate(json!({
                    "op": "createOrchestrator", "name": nonempty("name", &a.name)?, "goal": a.goal.trim(),
                    "rules": a.rules, "spaceId": space, "deviceId": device,
                    "prompt": a.prompt.filter(|p| !p.trim().is_empty()),
                }))
                .await
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }

    async fn board(&self, role: &Role, filter: Option<&str>) -> anyhow::Result<Value> {
        let state = self.orchestration_state().await?;
        let (chats, sessions) = tokio::try_join!(self.zeron.chats(), self.zeron.sessions())?;
        let mine = match role {
            Role::Orchestrator { id } => Some(id.as_str()),
            _ => None,
        };
        let filter = filter.map(str::trim).filter(|f| !f.is_empty()).unwrap_or("open");
        let want_state: Option<WorkState> = match filter {
            "open" | "all" => None,
            raw => Some(parse_enum::<WorkState>("filter", raw).map_err(anyhow::Error::msg)?),
        };
        let items: Vec<Value> = state
            .items
            .iter()
            .filter(|i| mine.is_none_or(|m| i.orchestrator_id.as_deref() == Some(m)))
            .filter(|i| match (filter, want_state) {
                ("open", _) => !matches!(i.state, WorkState::Done | WorkState::Dropped),
                (_, Some(s)) => i.state == s,
                _ => true,
            })
            .map(|i| {
                json!({
                    "id": i.id, "orchestratorId": i.orchestrator_id, "title": i.title, "state": i.state,
                    "priority": i.priority, "progress": i.progress, "threads": i.threads, "links": i.links,
                    "outcome": i.outcome, "detail": i.detail,
                })
            })
            .collect();
        let crew: Vec<Value> = state
            .crew
            .iter()
            .filter(|m| !m.released && mine.is_none_or(|o| m.orchestrator_id == o))
            .map(|m| {
                let chat = chats.iter().find(|c| c.id == m.chat_id);
                let session = chat.and_then(|c| session_for(&sessions, c));
                json!({
                    "chat": m.chat_id, "orchestratorId": m.orchestrator_id,
                    "title": chat.and_then(|c| c.title.clone()), "role": m.role, "item": m.item_id,
                    "status": status_of(session.as_ref()).0,
                    "activity": session.as_ref().and_then(|s| s.activity.clone()),
                    "todo": session.as_ref().and_then(|s| s.todo.clone()),
                    "lastReport": m.last_report, "deviceId": chat.map(|c| c.device_id.clone()),
                })
            })
            .collect();
        let asks: Vec<Value> = state
            .asks
            .iter()
            .filter(|a| a.state == zeron_proto::orchestration::AskState::Open)
            .filter(|a| mine.is_none_or(|o| a.orchestrator_id.as_deref() == Some(o)))
            .map(|a| json!({ "id": a.id, "from": a.from_chat, "kind": a.kind, "question": a.question, "urgent": a.urgent, "item": a.item_id }))
            .collect();
        Ok(json!({ "items": items, "crew": crew, "asks": asks }))
    }

    async fn world(&self) -> anyhow::Result<Value> {
        let state = self.orchestration_state().await?;
        let (chats, sessions, spaces, devices) =
            tokio::try_join!(self.zeron.chats(), self.zeron.sessions(), self.zeron.spaces(), self.zeron.devices())?;
        let in_orchestration = |id: &str| state.orchestrators.iter().any(|o| o.id == id);
        let orchestrators: Vec<Value> = state
            .orchestrators
            .iter()
            .filter(|o| o.kind == OrchestratorKind::Standard && !o.archived)
            .map(|o| {
                let crew: Vec<Value> = state
                    .crew
                    .iter()
                    .filter(|m| m.orchestrator_id == o.id && !m.released)
                    .map(|m| {
                        let chat = chats.iter().find(|c| c.id == m.chat_id);
                        let session = chat.and_then(|c| session_for(&sessions, c));
                        let item = m.item_id.as_deref().and_then(|id| state.items.iter().find(|i| i.id == id));
                        json!({
                            "chat": m.chat_id, "title": chat.and_then(|c| c.title.clone()), "role": m.role,
                            "status": status_of(session.as_ref()).0,
                            "item": item.map(|i| json!({ "id": i.id, "title": i.title, "state": i.state })),
                        })
                    })
                    .collect();
                let items: Vec<Value> = state
                    .items
                    .iter()
                    .filter(|i| i.orchestrator_id.as_deref() == Some(o.id.as_str()))
                    .filter(|i| !matches!(i.state, WorkState::Done | WorkState::Dropped))
                    .map(|i| json!({ "id": i.id, "title": i.title, "state": i.state }))
                    .collect();
                json!({ "id": o.id, "name": o.name, "goal": o.goal, "paused": o.paused, "crew": crew, "items": items })
            })
            .collect();
        let mut top: Vec<_> = chats
            .iter()
            .filter(|c| c.parent_chat_id.is_none() && !c.archived && !in_orchestration(&c.id))
            .filter(|c| !zeron_proto::voice::is_orchestrator_chat(&c.id))
            .collect();
        top.sort_by(|a, b| b.last_message_at.cmp(&a.last_message_at));
        let recent: Vec<Value> = top
            .into_iter()
            .take(30)
            .map(|c| {
                let session = session_for(&sessions, c);
                let project = c.space_id.as_deref().and_then(|id| spaces.iter().find(|s| s.id == id));
                json!({
                    "id": c.id, "title": c.title, "status": status_of(session.as_ref()).0,
                    "project": project.map(|p| p.display_name()), "deviceId": c.device_id,
                })
            })
            .collect();
        let devices: Vec<Value> = devices
            .iter()
            .map(|d| json!({ "id": d.id, "name": d.name, "lastSeenAt": d.last_seen_at }))
            .collect();
        let loose: Vec<Value> = state
            .items
            .iter()
            .filter(|i| i.orchestrator_id.is_none() && !matches!(i.state, WorkState::Done | WorkState::Dropped))
            .map(|i| json!({ "id": i.id, "title": i.title, "state": i.state }))
            .collect();
        Ok(json!({
            "orchestrators": orchestrators,
            "chats": recent,
            "looseItems": loose,
            "devices": devices,
            "note": "Crew threads are listed under their orchestrator. A task, priority or cancel for one goes to that orchestrator (name the thread in message); info and questions may go to the thread.",
        }))
    }
}

/// An orchestrator doesn't sit waiting on its own crew: their reports wake it,
/// all at once, and a turn spent waiting is one in which it can't answer the
/// person. Waiting on a crew thread returns this at once instead of the
/// thread's turn.
pub(super) fn crew_wait_note(role: &Role, chat: &zeron_proto::Chat) -> Option<Value> {
    let Role::Orchestrator { id } = role else { return None };
    let crew = chat.parent_chat_id.as_deref() == Some(id.as_str())
        && chat.child_kind == Some(zeron_proto::ChildKind::Crew);
    crew.then(|| {
        json!({
            "outcome": "notWaited",
            "note": "Crew threads report back on their own. End your turn now: their reports wake you together, as Crew updates.",
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use zeron_proto::orchestration::{CrewMember, CrewPolicy, Orchestrator};

    fn orchestrator(id: &str, kind: OrchestratorKind) -> Orchestrator {
        Orchestrator {
            id: id.into(),
            kind,
            name: id.into(),
            goal: String::new(),
            rules: String::new(),
            crew_rules: String::new(),
            notes: String::new(),
            hue: 0,
            crew: CrewPolicy::default(),
            wake: None,
            paused: false,
            archived: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn member(chat: &str, orch: &str, released: bool) -> CrewMember {
        CrewMember {
            chat_id: chat.into(),
            orchestrator_id: orch.into(),
            item_id: None,
            role: String::new(),
            last_report: None,
            released,
            joined_at: Utc::now(),
        }
    }

    #[test]
    fn an_orchestrator_never_waits_on_its_crew() {
        let chat = |parent: Option<&str>, kind: Option<zeron_proto::ChildKind>| zeron_proto::Chat {
            parent_chat_id: parent.map(str::to_owned),
            child_kind: kind,
            ..serde_json::from_value(json!({
                "id": "c1", "deviceId": "d", "title": null, "archived": false, "cwd": null,
                "branch": null, "checkoutId": null, "config": null, "lastMessagePreview": null,
                "lastMessageAt": null, "createdAt": "2026-11-12T00:00:00Z"
            }))
            .unwrap()
        };
        let orch = Role::Orchestrator { id: "o1".into() };
        let crew = chat(Some("o1"), Some(zeron_proto::ChildKind::Crew));
        assert_eq!(crew_wait_note(&orch, &crew).unwrap()["outcome"], "notWaited");
        // Another orchestrator's crew, a plain subagent, or a plain caller: waits as ever.
        assert!(crew_wait_note(&orch, &chat(Some("o2"), Some(zeron_proto::ChildKind::Crew))).is_none());
        assert!(crew_wait_note(&orch, &chat(Some("o1"), Some(zeron_proto::ChildKind::Subagent))).is_none());
        assert!(crew_wait_note(&Role::Plain, &crew).is_none());
    }

    #[test]
    fn each_role_gets_its_built_in_prompt() {
        let mut o = orchestrator("o1", OrchestratorKind::Standard);
        o.name = "Ops".into();
        o.goal = "Ship it".into();
        o.crew_rules = "Tests first".into();
        let mut m = member("c1", "o1", false);
        m.item_id = Some("i1".into());
        let item = zeron_proto::orchestration::WorkItem {
            id: "i1".into(),
            orchestrator_id: Some("o1".into()),
            title: "Auth PR".into(),
            detail: String::new(),
            state: WorkState::Active,
            priority: Default::default(),
            progress: None,
            threads: vec![],
            links: vec![],
            outcome: None,
            source: None,
            order: 0.0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            done_at: None,
        };
        let state = OrchestrationState {
            orchestrators: vec![o, orchestrator("b1", OrchestratorKind::BrainDump)],
            crew: vec![m],
            items: vec![item],
            ..Default::default()
        };
        let prompt = |chat: &str| role_prompt(&Role::of(Some(chat), &state), Some(chat), &state);
        assert!(prompt("o1").unwrap().starts_with("You are an orchestrator in Zeron: Ops. Goal: Ship it."));
        let crew = prompt("c1").unwrap();
        assert!(crew.starts_with("You are on Ops's crew, working on Auth PR."), "{crew}");
        assert!(crew.contains("Crew rules: Tests first."));
        assert!(prompt("b1").unwrap().starts_with("You route the person's brain dumps."));
        assert_eq!(prompt("someone"), None);
    }

    #[test]
    fn a_full_or_paused_crew_refuses_new_work_clearly() {
        let mut o = orchestrator("o1", OrchestratorKind::Standard);
        assert_eq!(crew_refusal(&o, 9, true), None, "no limit");
        o.crew.max_running = Some(2);
        assert_eq!(crew_refusal(&o, 1, true), None);
        let full = crew_refusal(&o, 2, true).unwrap();
        assert!(full.contains("2 of 2 threads are working"), "{full}");
        assert_eq!(crew_refusal(&o, 2, false), None, "a thread that doesn't start yet is fine");
        o.paused = true;
        assert!(crew_refusal(&o, 0, false).unwrap().contains("paused"));
    }

    #[test]
    fn roles_come_from_rows_not_ids() {
        let state = OrchestrationState {
            orchestrators: vec![orchestrator("o1", OrchestratorKind::Standard), orchestrator("b1", OrchestratorKind::BrainDump)],
            crew: vec![member("c1", "o1", false), member("c2", "o1", true)],
            ..Default::default()
        };
        assert_eq!(Role::of(Some("o1"), &state), Role::Orchestrator { id: "o1".into() });
        assert_eq!(Role::of(Some("b1"), &state), Role::BrainDump { id: "b1".into() });
        assert_eq!(Role::of(Some("c1"), &state), Role::Crew { orchestrator_id: "o1".into() });
        assert_eq!(Role::of(Some("c2"), &state), Role::Plain, "released");
        // A chat whose id looks like a Brain dump's is plain without a row.
        assert_eq!(Role::of(Some("brain-dump-abc"), &state), Role::Plain);
        assert_eq!(Role::of(None, &state), Role::Plain);
    }

    #[test]
    fn each_role_sees_its_tools() {
        let offered = |role: &Role| NAMES.iter().copied().filter(|t| role.offers(t)).collect::<Vec<_>>();
        assert_eq!(offered(&Role::Plain), vec![ASK]);
        assert_eq!(offered(&Role::Crew { orchestrator_id: "o".into() }), vec![REPORT, ASK]);
        assert_eq!(
            offered(&Role::Orchestrator { id: "o".into() }),
            vec![ASK, BOARD, ITEM_UPSERT, CREW_ADD, CREW_RELEASE, NOTES_SET]
        );
        assert_eq!(
            offered(&Role::BrainDump { id: "b".into() }),
            vec![ASK, BOARD, ITEM_UPSERT, WORLD, ROUTE, DUMP_DONE, ORCHESTRATOR_CREATE]
        );
    }

    #[test]
    fn arguments_are_checked_before_anything_is_sent() {
        let crew = Role::Crew { orchestrator_id: "o".into() };
        let ok = op_for(REPORT, json!({ "state": "done", "summary": " merged ", "progress": 1.0 }), &crew).unwrap();
        assert_eq!(ok["op"], "report");
        assert_eq!(ok["summary"], "merged");
        assert!(op_for(REPORT, json!({ "state": "celebrating", "summary": "x" }), &crew).is_err());
        assert!(op_for(REPORT, json!({ "state": "done", "summary": "  " }), &crew).is_err());
        assert!(op_for(REPORT, json!({ "state": "done", "summary": "a\nb" }), &crew).is_err());
        assert!(op_for(REPORT, json!({ "state": "done", "summary": "x", "progress": 3 }), &crew).is_err());
        assert!(op_for(REPORT, json!({ "summary": "x" }), &crew).is_err(), "state is required");

        let ask = op_for(ASK, json!({ "question": "Ship?", "options": ["yes", { "label": "no", "detail": "wait" }] }), &crew).unwrap();
        assert_eq!(ask["kind"], "question");
        assert_eq!(ask["options"][1]["detail"], "wait");
        assert!(op_for(ASK, json!({ "question": "Ship?", "kind": "vibe" }), &crew).is_err());

        let orch = Role::Orchestrator { id: "o".into() };
        let created = op_for(ITEM_UPSERT, json!({ "title": "Auth", "state": "active", "outcome": null }), &orch).unwrap();
        assert_eq!(created["item"]["orchestratorId"], "o", "an orchestrator's new item is its own");
        assert_eq!(created["item"]["outcome"], Value::Null);
        assert!(op_for(ITEM_UPSERT, json!({ "state": "active" }), &orch).is_err(), "new needs a title");
        assert!(op_for(ITEM_UPSERT, json!({ "id": "i", "state": "someday" }), &orch).is_err());
        assert!(op_for(ITEM_UPSERT, json!({ "title": "x", "orchestrator": "other" }), &orch).is_err());
        let patch = op_for(ITEM_UPSERT, json!({ "id": "i", "progress": null }), &orch).unwrap();
        assert_eq!(patch["item"]["progress"], Value::Null);
        assert!(patch["item"].get("orchestratorId").is_none(), "a patch keeps its owner");

        let notes = op_for(NOTES_SET, json!({ "text": "plan" }), &orch).unwrap();
        assert_eq!(notes, json!({ "op": "updateOrchestrator", "id": "o", "notes": "plan" }));

        let brain = Role::BrainDump { id: "b".into() };
        let route = op_for(
            ROUTE,
            json!({ "dump": "d", "excerpt": "fix auth", "target": { "kind": "orchestrator", "id": "o" }, "intent": "task", "message": "Fix auth" }),
            &brain,
        )
        .unwrap();
        assert_eq!(route["target"], json!({ "kind": "orchestrator", "id": "o" }));
        assert!(op_for(ROUTE, json!({ "dump": "d", "excerpt": "x", "target": { "kind": "chat" }, "intent": "task", "message": "m" }), &brain).is_err());
        assert!(op_for(ROUTE, json!({ "dump": "d", "excerpt": "x", "target": { "kind": "unsure" }, "intent": "task" }), &brain).is_err());
        assert!(op_for(ROUTE, json!({ "dump": "d", "excerpt": "x", "target": { "kind": "chat", "id": "c" }, "intent": "task" }), &brain).is_err());
        assert!(op_for(ROUTE, json!({ "dump": "d", "excerpt": "x", "target": { "kind": "note" }, "intent": "gossip" }), &brain).is_err());
        assert!(op_for(DUMP_DONE, json!({ "dump": "d", "summary": "" }), &brain).is_err());
    }
}
