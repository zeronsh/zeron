//! `zeron chat …`, `zeron harness list`, `zeron model list` and
//! `zeron guide` — the agent-facing CLI built on the same [`Tools`] the MCP
//! server exposes, so the two surfaces never drift.
//!
//! Human output is short, plain text, one fact per line — no color, no
//! emoji. `--json` prints exactly one JSON document on stdout (logs stay on
//! stderr; `main.rs` routes these subcommands to the stderr subscriber).
//! Errors print `error: <message>` (plus `hint: …` when useful) on stderr
//! with exit 1; settle outcomes map to the documented exit codes.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use serde_json::{Value, json};
use zeron_proto::orchestration::{CHAT_MENTION_PREFIX, ChildOutcome, ChildUpdateRef, child_update};
use zeron_proto::view::display_status;
use zeron_proto::{Chat, Session, SessionStatus};
use zeron_rpc::methods;

use crate::tools::{ChatError, CreateChatArgs, Tools};
use crate::zeron::{Origin, Zeron, session_for, short};

/// A settled (or given-up-on) chat after a `wait`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaitOutcome {
    Completed,
    NeedsInput,
    Errored,
    Interrupted,
    /// The last session row claims Working but is too old to trust — the
    /// host may be dead. Counted as settled so `wait` does not hang on a
    /// crashed peer.
    Unknown,
    TimedOut,
}

impl WaitOutcome {
    fn from_update(update: &zeron_proto::orchestration::ChildUpdate) -> Self {
        match update.outcome {
            ChildOutcome::Completed => Self::Completed,
            ChildOutcome::Errored => Self::Errored,
            ChildOutcome::Interrupted => Self::Interrupted,
            ChildOutcome::NeedsInput => Self::NeedsInput,
        }
    }

    /// serde/camelCase outcome names in `--json` output.
    fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NeedsInput => "needsInput",
            Self::Errored => "errored",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
            Self::TimedOut => "timedOut",
        }
    }

    /// Human wording for the same outcome.
    fn phrase(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NeedsInput => "needs input",
            Self::Errored => "errored",
            Self::Interrupted => "was interrupted",
            Self::Unknown => "status unknown (session stale — host may be offline)",
            Self::TimedOut => "timed out",
        }
    }

    /// Higher wins when several chats settle differently.
    fn severity(self) -> u8 {
        match self {
            Self::Completed => 0,
            Self::Unknown => 1,
            Self::NeedsInput => 2,
            Self::Interrupted => 3,
            Self::Errored => 4,
            Self::TimedOut => 5,
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            Self::Completed => 0,
            Self::Unknown => 1,
            Self::NeedsInput => 2,
            Self::Errored => 3,
            Self::Interrupted => 4,
            Self::TimedOut => 124,
        }
    }
}

/// One target of `wait`, resolved.
#[derive(Clone)]
struct Settled {
    chat: Chat,
    outcome: WaitOutcome,
    /// The dedupe key `child_update` produced — acked against the parent's
    /// notification ledger when this CLI runs inside the parent chat.
    turn_key: Option<String>,
}

/// A command failure carrying its process exit code.
#[derive(Debug)]
struct Failure {
    code: i32,
    message: String,
    hint: Option<String>,
}

impl Failure {
    fn error(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
            hint: None,
        }
    }
    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    fn code(mut self, code: i32) -> Self {
        self.code = code;
        self
    }
}

type CliResult<T> = Result<T, Failure>;

/// One command's result: the human text, the JSON document, the exit code.
#[derive(Debug)]
struct Report {
    human: String,
    json: Value,
    code: i32,
}

impl Report {
    fn new(human: impl Into<String>, json: Value) -> Self {
        Self {
            human: human.into(),
            json,
            code: 0,
        }
    }
    fn code(mut self, code: i32) -> Self {
        self.code = code;
        self
    }
}

fn emit(result: CliResult<Report>, json: bool) -> i32 {
    match result {
        Ok(report) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report.json).unwrap_or_default()
                );
            } else if !report.human.is_empty() {
                println!("{}", report.human);
            }
            report.code
        }
        Err(failure) => {
            eprintln!("error: {}", failure.message);
            if let Some(hint) = failure.hint {
                eprintln!("hint: {hint}");
            }
            failure.code
        }
    }
}

fn tools(ipc_port: u16) -> Tools {
    Tools::new(Arc::new(Zeron::new(
        format!("ws://127.0.0.1:{ipc_port}"),
        Origin::from_env(),
    )))
}

/// `<chat>` accepts a full id, a unique prefix, an exact title, `self`
/// (`$ZERON_CHAT_ID`), or an `@chat:<id>` mention pasted from a transcript.
async fn resolve(tools: &Tools, key: &str) -> CliResult<Chat> {
    let key = key
        .trim()
        .strip_prefix(CHAT_MENTION_PREFIX)
        .unwrap_or_else(|| key.trim());
    let key = if key == "self" {
        tools.zeron.origin().chat_id.clone().ok_or_else(|| {
            Failure::error("`self` needs ZERON_CHAT_ID — run inside a chat or pass a chat id")
        })?
    } else {
        key.to_owned()
    };
    tools
        .zeron
        .resolve_chat(&key)
        .await
        .map_err(|e| Failure::error(e.to_string()))
}

/// `zeron chat <something-else>`: the catch-all lands here with the
/// subcommand name in `args[0]`. Replay it against a bare table of the real
/// subcommands (names and aliases) so the printed error — including the
/// "a similar subcommand exists" tip — is clap's own wording rather than a
/// hand-copy that would drift from it.
fn unknown_subcommand(args: &[String]) -> i32 {
    let real = <ChatCommand as Subcommand>::augment_subcommands(clap::Command::new("chat"));
    let mut table = clap::Command::new("zeron chat").subcommand_required(true);
    for sub in real.get_subcommands() {
        table = table.subcommand(sub.clone());
    }
    let replay = std::iter::once("zeron chat").chain(args.iter().map(String::as_str));
    match table.try_get_matches_from(replay) {
        Err(error) => {
            eprint!("{}", error.render());
            eprintln!("hint: run `zeron chat --help` or `zeron guide chats`");
            // Usage errors exit 1 — 2 is reserved for "awaiting input".
            1
        }
        // A captured name is by definition not a real subcommand; this arm
        // exists only so a surprise parse success cannot report ok.
        Ok(_) => emit(
            Err::<Report, _>(Failure::error("unrecognized chat subcommand")),
            false,
        ),
    }
}

/// `90s`, `20m`, `1h`, or bare seconds.
fn parse_duration(raw: &str) -> Result<Duration, String> {
    let raw = raw.trim();
    let (digits, factor) = match raw.chars().last() {
        Some('s') => (&raw[..raw.len() - 1], 1u64),
        Some('m') => (&raw[..raw.len() - 1], 60),
        Some('h') => (&raw[..raw.len() - 1], 3600),
        _ => (raw, 1),
    };
    let n: u64 = digits.parse().map_err(|_| {
        format!("invalid duration {raw:?} (use e.g. 90s, 20m, 1h, or bare seconds)")
    })?;
    Ok(Duration::from_secs(n.saturating_mul(factor)))
}

fn read_text_arg(inline: Option<String>, file: Option<String>, what: &str) -> CliResult<String> {
    let text = match (inline, file) {
        (Some(text), None) => text,
        (None, Some(path)) => {
            let read = if path == "-" {
                std::io::read_to_string(std::io::stdin())
            } else {
                std::fs::read_to_string(&path)
            };
            read.map_err(|e| Failure::error(format!("cannot read {what} from {path:?}: {e}")))?
        }
        (None, None) => {
            return Err(Failure::error(format!("missing {what}"))
                .hint(format!("pass --{what} <text> or --{what}-file <path|->")));
        }
        (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
    };
    if text.trim().is_empty() {
        return Err(Failure::error(format!("{what} is empty")));
    }
    Ok(text)
}

// ---- settle watching --------------------------------------------------------

/// Wait for every target to settle using `child_update(prev, next)` over the
/// WatchSessions stream — the same transition detection the parent notifier
/// runs, so the CLI agrees with delivered notifications.
///
/// `baseline` seeds `prev`: callers that just sent a message pass the
/// pre-send session so an unchanged `last_completed_turn` reads as
/// interrupted rather than completed, and an old settle does not count as
/// this send's outcome.
async fn wait_settle(
    zeron: &Zeron,
    targets: Vec<(Chat, Option<Session>)>,
    any: bool,
    timeout: Duration,
) -> anyhow::Result<Vec<Settled>> {
    let deadline = Instant::now() + timeout;
    let mut prev: HashMap<String, Option<Session>> = targets
        .iter()
        .map(|(chat, baseline)| (chat.id.clone(), baseline.clone()))
        .collect();
    let mut done: Vec<Settled> = Vec::new();
    'resubscribe: loop {
        if deadline.saturating_duration_since(Instant::now()).is_zero() {
            break;
        }
        let mut rx = zeron.subscribe(methods::WATCH_SESSIONS, json!({})).await?;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break 'resubscribe;
            }
            let item = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(item)) => item,
                Ok(None) => {
                    // Watch streams end at lifecycle boundaries; reattach
                    // rather than declaring a timeout early.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    continue 'resubscribe;
                }
                Err(_) => break 'resubscribe,
            };
            let sessions: Vec<Session> = match serde_json::from_value(item) {
                Ok(sessions) => sessions,
                Err(error) => {
                    tracing::warn!(%error, "WatchSessions: unexpected item");
                    continue;
                }
            };
            let now = chrono::Utc::now();
            let open: Vec<usize> = (0..targets.len())
                .filter(|i| !done.iter().any(|s| s.chat.id == targets[*i].0.id))
                .collect();
            for i in open {
                let chat = &targets[i].0;
                let Some(row) = session_for(&sessions, chat) else {
                    continue;
                };
                let before = prev.get(&chat.id).and_then(|s| s.as_ref());
                if let Some(update) = child_update(before, &row) {
                    done.push(Settled {
                        chat: chat.clone(),
                        outcome: WaitOutcome::from_update(&update),
                        turn_key: Some(update.key),
                    });
                } else if row.status == SessionStatus::Working
                    && now - row.updated_at
                        > chrono::Duration::milliseconds(zeron_proto::view::SESSION_STALE_MS)
                {
                    done.push(Settled {
                        chat: chat.clone(),
                        outcome: WaitOutcome::Unknown,
                        turn_key: None,
                    });
                } else {
                    prev.insert(chat.id.clone(), Some(row));
                }
            }
            if done.len() == targets.len() || (any && !done.is_empty()) {
                break 'resubscribe;
            }
        }
    }
    // Everyone left over hits the deadline state.
    for (chat, _) in &targets {
        if !done.iter().any(|s| s.chat.id == chat.id) {
            done.push(Settled {
                chat: chat.clone(),
                outcome: WaitOutcome::TimedOut,
                turn_key: None,
            });
        }
    }
    // Preserve the caller's target order.
    done.sort_by_key(|s| targets.iter().position(|(c, _)| c.id == s.chat.id));
    Ok(done)
}

/// Ack the observed child updates when this CLI is running inside the
/// parent (`ZERON_CHAT_ID` == the target's `parent_chat_id`). Acked keys are
/// never delivered to the parent again; failures are logged, never fatal.
async fn ack_observed(zeron: &Zeron, settled: &[Settled]) {
    let Some(origin) = zeron.origin().chat_id.as_deref() else {
        return;
    };
    let updates: Vec<ChildUpdateRef> = settled
        .iter()
        .filter(|s| s.chat.parent_chat_id.as_deref() == Some(origin))
        .filter_map(|s| {
            s.turn_key.as_ref().map(|key| ChildUpdateRef {
                child_chat_id: s.chat.id.clone(),
                turn_key: key.clone(),
            })
        })
        .collect();
    zeron.ack_child_updates(origin, updates).await;
}

/// A minimal chat row for ids we just minted — fields the wait/settle path
/// actually reads are id, device_id and parent_chat_id.
fn chat_shell() -> Chat {
    Chat {
        id: String::new(),
        device_id: String::new(),
        title: None,
        archived: false,
        cwd: None,
        branch: None,
        checkout_id: None,
        source_context: None,
        config: None,
        last_message_preview: None,
        last_message_at: None,
        created_at: chrono::Utc::now(),
        harness_session_id: None,
        harness_session_cwd: None,
        parent_chat_id: None,
        space_id: None,
        last_seen_at: None,
        room_gen: None,
        spawned_by_agent: false,
    }
}

fn title_of(chat: &Chat) -> String {
    chat.title
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| short(&chat.id).to_owned())
}

/// `chat.show`'s status word — the same derived display status the sidebar
/// uses (staleness-gated session row plus the unseen marker).
fn display_word(chat: &Chat, session: Option<&Session>) -> &'static str {
    match display_status(chat, session, chrono::Utc::now()) {
        zeron_proto::ChatIndicator::Working => "working",
        zeron_proto::ChatIndicator::AwaitingInput => "awaiting input",
        zeron_proto::ChatIndicator::Errored => "errored",
        zeron_proto::ChatIndicator::Completed => "completed",
        zeron_proto::ChatIndicator::Idle => "idle",
    }
}

// ---- clap --------------------------------------------------------------------

#[derive(Args)]
pub struct JsonFlag {
    /// Print one JSON document on stdout instead of human output.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct SpawnArgs {
    /// First message to send the new chat.
    #[arg(long, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    /// Read the first message from a file, or `-` for stdin.
    #[arg(long, value_name = "PATH", conflicts_with = "prompt")]
    prompt_file: Option<String>,
    /// Harness id (see `zeron harness list`). Defaults to the parent's.
    #[arg(long)]
    harness: Option<String>,
    /// Model id the harness offers (see `zeron model list`). Defaults to the
    /// parent's on the same harness, else the harness default.
    #[arg(long)]
    model: Option<String>,
    /// Reasoning level the model supports. Defaults to the parent's on the
    /// same harness.
    #[arg(long)]
    reasoning: Option<String>,
    /// Sidebar title; otherwise the engine titles it from the first exchange.
    #[arg(long)]
    title: Option<String>,
    /// Project (id, path, or name). Defaults to the parent's project.
    #[arg(long)]
    project: Option<String>,
    /// Host device for a project-less chat (id or name).
    #[arg(long)]
    device: Option<String>,
    /// Force a fresh git worktree of the project repo (the default for git projects).
    #[arg(long, conflicts_with_all = ["same_checkout", "cwd"])]
    worktree: bool,
    /// Base ref for the new worktree branch (default: the parent's branch, else HEAD).
    #[arg(long)]
    base: Option<String>,
    /// Share the parent's checkout instead of a fresh worktree.
    #[arg(long, conflicts_with_all = ["worktree", "cwd"])]
    same_checkout: bool,
    /// Explicit working directory — no worktree.
    #[arg(long, conflicts_with_all = ["worktree", "same_checkout"])]
    cwd: Option<String>,
    /// Sandbox level; may only be lowered from the parent's.
    #[arg(long, value_parser = ["read-only", "workspace-write", "danger-full-access"])]
    sandbox: Option<String>,
    /// Parent chat to nest under (default: this chat).
    #[arg(long, conflicts_with = "no_parent")]
    parent: Option<String>,
    /// Spawn top-level even when run inside a chat.
    #[arg(long)]
    no_parent: bool,
    /// Wait until the first turn settles; the exit code is the outcome.
    #[arg(long)]
    wait: bool,
    /// Deadline for --wait: 90s, 20m, 1h, or bare seconds.
    #[arg(long, default_value = "20m", value_parser = parse_duration)]
    timeout: Duration,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct TellArgs {
    /// Chat to message: id, id prefix, exact title, or `self`.
    chat: String,
    /// Message text.
    #[arg(conflicts_with = "message_file")]
    text: Option<String>,
    /// Read the message from a file, or `-` for stdin.
    #[arg(long, value_name = "PATH", conflicts_with = "text")]
    message_file: Option<String>,
    /// auto (default): run when idle, steer a live turn; queue holds for turn end.
    #[arg(long, value_parser = ["auto", "steer", "queue"], default_value = "auto")]
    mode: String,
    /// Wait until the turn settles; the exit code is the outcome.
    #[arg(long)]
    wait: bool,
    /// Deadline for --wait: 90s, 20m, 1h, or bare seconds.
    #[arg(long, default_value = "20m", value_parser = parse_duration)]
    timeout: Duration,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct WaitArgs {
    /// Chats to wait for: id, id prefix, exact title, or `self`.
    #[arg(required = true)]
    chats: Vec<String>,
    /// Return as soon as any one chat settles.
    #[arg(long)]
    any: bool,
    /// Deadline: 90s, 20m, 1h, or bare seconds.
    #[arg(long, default_value = "20m", value_parser = parse_duration)]
    timeout: Duration,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct ChatArg {
    /// Chat: id, id prefix, exact title, or `self`.
    chat: String,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct LogArgs {
    /// Chat: id, id prefix, exact title, or `self`.
    chat: String,
    /// How many messages, counted from the newest.
    #[arg(long)]
    limit: Option<usize>,
    /// Include the tool-call ledger.
    #[arg(long)]
    tools: bool,
    /// Include model thinking.
    #[arg(long)]
    reasoning: bool,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct ListArgs {
    /// Only children of this chat (default when bare: this chat / self).
    #[arg(long, num_args = 0..=1, default_missing_value = "self")]
    children: Option<String>,
    /// Only chats in this project (id, path, or name).
    #[arg(long)]
    project: Option<String>,
    /// Include archived chats.
    #[arg(long)]
    archived: bool,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct AnswerArgs {
    /// Chat awaiting input: id, id prefix, exact title, or `self`.
    chat: String,
    /// Option labels (or free text) to answer with — one per pending question.
    #[arg(required = true)]
    answers: Vec<String>,
    /// The pending request id; defaults to the chat's current question.
    #[arg(long)]
    request: Option<String>,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct ArchiveArgs {
    /// Chat: id, id prefix, exact title, or `self`.
    chat: String,
    /// Restore the chat instead of archiving it.
    #[arg(long)]
    unarchive: bool,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Args)]
pub struct ForkArgs {
    /// Chat to fork: id, id prefix, exact title, or `self`.
    chat: String,
    /// First message to send the fork.
    #[arg(long, conflicts_with = "prompt_file")]
    prompt: Option<String>,
    /// Read the first message from a file, or `-` for stdin.
    #[arg(long, value_name = "PATH", conflicts_with = "prompt")]
    prompt_file: Option<String>,
    #[command(flatten)]
    json: JsonFlag,
}

#[derive(Subcommand)]
pub enum ChatCommand {
    /// Spawn a child chat with a prompt (defaults inherit the parent).
    Spawn(SpawnArgs),
    /// Send a message to a chat (attributed to this chat when run inside one).
    Tell(TellArgs),
    /// Block until chats settle; exit code is the outcome.
    Wait(WaitArgs),
    /// Print the last assistant reply of the latest settled turn.
    #[command(alias = "read")]
    Output(ChatArg),
    /// Print a chat's summary: status, harness/model, cwd/branch, children.
    Show(ChatArg),
    /// Print the transcript (newest window).
    #[command(alias = "transcript", alias = "history")]
    Log(LogArgs),
    /// List chats.
    List(ListArgs),
    /// Stop the chat's running turn.
    Interrupt(ChatArg),
    /// Answer a question the chat is blocked on.
    Answer(AnswerArgs),
    /// Archive a chat (--unarchive restores).
    Archive(ArchiveArgs),
    /// Copy a chat's settled history into a new chat.
    Fork(ForkArgs),
    /// Anything unrecognized lands here (name first) so it can be reported
    /// with a suggestion instead of a bare parse error.
    #[command(external_subcommand)]
    Unknown(Vec<String>),
}

#[derive(Subcommand)]
pub enum HarnessCommand {
    /// Agent harnesses and whether each is available on this device.
    List {
        #[command(flatten)]
        json: JsonFlag,
    },
}

#[derive(Subcommand)]
pub enum ModelCommand {
    /// Models a harness offers on this device.
    List {
        /// Harness id (see `zeron harness list`).
        harness: String,
        #[command(flatten)]
        json: JsonFlag,
    },
}

// ---- entry points --------------------------------------------------------------

/// `zeron chat <command>` — dispatch, print, return the process exit code.
pub async fn run_chat(command: ChatCommand, ipc_port: u16) -> i32 {
    let tools = tools(ipc_port);
    match command {
        ChatCommand::Spawn(a) => {
            let json = a.json.json;
            emit(spawn(&tools, a).await, json)
        }
        ChatCommand::Tell(a) => {
            let json = a.json.json;
            emit(tell(&tools, a).await, json)
        }
        ChatCommand::Wait(a) => {
            let json = a.json.json;
            emit(wait(&tools, a).await, json)
        }
        ChatCommand::Output(a) => {
            let json = a.json.json;
            emit(output(&tools, a).await, json)
        }
        ChatCommand::Show(a) => {
            let json = a.json.json;
            emit(show(&tools, a).await, json)
        }
        ChatCommand::Log(a) => {
            let json = a.json.json;
            emit(log(&tools, a).await, json)
        }
        ChatCommand::List(a) => {
            let json = a.json.json;
            emit(list(&tools, a).await, json)
        }
        ChatCommand::Interrupt(a) => {
            let json = a.json.json;
            emit(interrupt(&tools, a).await, json)
        }
        ChatCommand::Answer(a) => {
            let json = a.json.json;
            emit(answer(&tools, a).await, json)
        }
        ChatCommand::Archive(a) => {
            let json = a.json.json;
            emit(archive(&tools, a).await, json)
        }
        ChatCommand::Fork(a) => {
            let json = a.json.json;
            emit(fork(&tools, a).await, json)
        }
        ChatCommand::Unknown(args) => unknown_subcommand(&args),
    }
}

/// `zeron harness list`.
pub async fn run_harness(command: HarnessCommand, ipc_port: u16) -> i32 {
    let tools = tools(ipc_port);
    match command {
        HarnessCommand::List { json } => {
            let json = json.json;
            emit(harness_list(&tools).await, json)
        }
    }
}

/// `zeron model list <harness>`.
pub async fn run_model(command: ModelCommand, ipc_port: u16) -> i32 {
    let tools = tools(ipc_port);
    match command {
        ModelCommand::List { harness, json } => {
            let json = json.json;
            emit(model_list(&tools, &harness).await, json)
        }
    }
}

/// `zeron guide [chapter]` — pure content, no engine needed.
pub fn run_guide(chapter: Option<String>, json: bool) -> i32 {
    let result = match chapter.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        None => Ok(Report::new(
            zeron_guide::overview(),
            json!({
                "chapters": zeron_guide::chapters()
                    .iter()
                    .map(|c| json!({ "name": c.name, "summary": c.summary }))
                    .collect::<Vec<_>>()
            }),
        )),
        Some(name) => match zeron_guide::chapter(name) {
            Some(chapter) => Ok(Report::new(
                chapter.body.trim_end(),
                json!({ "name": chapter.name, "summary": chapter.summary, "body": chapter.body }),
            )),
            None => Err(Failure::error(format!("no chapter {name:?}")).hint(format!(
                "chapters: {}",
                zeron_guide::chapters()
                    .iter()
                    .map(|c| c.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        },
    };
    emit(result, json)
}

// ---- commands -----------------------------------------------------------------

async fn spawn(tools: &Tools, args: SpawnArgs) -> CliResult<Report> {
    let prompt = read_text_arg(args.prompt, args.prompt_file, "prompt")?;
    let result = tools
        .create_chat(CreateChatArgs {
            project: args.project,
            device: args.device,
            parent: args.parent,
            harness: args.harness,
            model: args.model,
            reasoning: args.reasoning,
            sandbox: args.sandbox,
            title: args.title,
            branch: None,
            cwd: args.cwd,
            worktree: args.worktree,
            base: args.base,
            same_checkout: args.same_checkout,
            no_parent: args.no_parent,
            prompt: Some(prompt),
            wait: false,
            timeout_secs: None,
        })
        .await
        .map_err(|e| match e {
            ChatError::Limit(m) => Failure::error(m)
                .hint("wait for a child to finish or archive one")
                .code(5),
            ChatError::Failed(e) => Failure::error(e.to_string()),
        })?;
    let chat_id = result["chatId"].as_str().unwrap_or_default().to_owned();
    let title = result["title"]
        .as_str()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("chat")
        .to_owned();
    // Resolve the harness display name for the human line.
    let harness_id = result["harness"].as_str().unwrap_or_default().to_owned();
    let harness_name = tools
        .call("list_harnesses", json!({}))
        .await
        .ok()
        .and_then(|v| {
            v["harnesses"].as_array()?.iter().find_map(|h| {
                if h["id"].as_str() == Some(harness_id.as_str()) {
                    h["name"].as_str().map(str::to_owned)
                } else {
                    None
                }
            })
        })
        .unwrap_or(harness_id);
    let model = result["model"].as_str().unwrap_or("default");
    let mut human = format!("Spawned {title} on {harness_name} · {model}\n");
    let mut json_out = result.clone();
    let mut code = 0;
    if args.wait {
        // The row may not have folded into WatchChats yet; build the chat
        // locally from the create reply rather than re-reading.
        let chat = Chat {
            id: chat_id.clone(),
            device_id: result["deviceId"].as_str().unwrap_or_default().to_owned(),
            parent_chat_id: result["parentChatId"].as_str().map(str::to_owned),
            ..chat_shell()
        };
        let settled = wait_settle(
            tools.zeron.as_ref(),
            vec![(chat, None)],
            false,
            args.timeout,
        )
        .await
        .map_err(|e| Failure::error(e.to_string()))?;
        let settled = settled.into_iter().next().expect("one target settles");
        let reply = tools
            .last_reply(&settled.chat)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        human.push_str(&format!(
            "{}: {}\n",
            title_of(&settled.chat),
            settled.outcome.phrase()
        ));
        for line in reply.lines().take(20) {
            human.push_str(line);
            human.push('\n');
        }
        json_out["turn"] = json!({
            "outcome": settled.outcome.label(),
            "turnKey": settled.turn_key,
            "reply": reply,
        });
        ack_observed(tools.zeron.as_ref(), std::slice::from_ref(&settled)).await;
        code = settled.outcome.exit_code();
    }
    // The UI parses the id off the last line — keep it last.
    human.push_str(&format!("{CHAT_MENTION_PREFIX}{chat_id}"));
    Ok(Report::new(human, json_out).code(code))
}

async fn tell(tools: &Tools, args: TellArgs) -> CliResult<Report> {
    let text = read_text_arg(args.text, args.message_file, "text")?;
    let chat = resolve(tools, &args.chat).await?;
    // `auto` cannot deliver into a chat that is blocked on a question — say
    // so with the actionable exit code rather than a generic send failure.
    if args.mode == "auto" {
        let sessions = tools
            .zeron
            .sessions()
            .await
            .map_err(|e| Failure::error(e.to_string()))?;
        if session_for(&sessions, &chat).is_some_and(|s| s.status == SessionStatus::AwaitingInput) {
            return Err(Failure::error(format!(
                "chat {} is waiting for an answer",
                short(&chat.id)
            ))
            .hint(format!(
                "use `zeron chat answer {} <answer>`",
                short(&chat.id)
            ))
            .code(2));
        }
    }
    let (chat, baseline, sent) = tools
        .send_text(&chat.id, &text, &args.mode)
        .await
        .map_err(|e| Failure::error(e.to_string()))?;
    let delivery = sent["delivery"].as_str().unwrap_or("run").to_owned();
    let mut json_out = json!({
        "chatId": chat.id,
        "title": chat.title,
        "delivery": delivery,
        "id": sent["id"],
    });
    let mut human = format!("Messaged {} ({})", title_of(&chat), short(&chat.id));
    let mut code = 0;
    if args.wait {
        let settled = wait_settle(
            tools.zeron.as_ref(),
            vec![(chat.clone(), baseline)],
            false,
            args.timeout,
        )
        .await
        .map_err(|e| Failure::error(e.to_string()))?
        .into_iter()
        .next()
        .expect("one target settles");
        let reply = tools
            .last_reply(&settled.chat)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        human.push_str(&format!(
            "\n{}: {}",
            title_of(&settled.chat),
            settled.outcome.phrase()
        ));
        if !reply.is_empty() {
            human.push('\n');
            for line in reply.lines().take(20) {
                human.push_str(line);
                human.push('\n');
            }
            human.pop();
        }
        json_out["turn"] = json!({
            "outcome": settled.outcome.label(),
            "turnKey": settled.turn_key,
            "reply": reply,
        });
        ack_observed(tools.zeron.as_ref(), std::slice::from_ref(&settled)).await;
        code = settled.outcome.exit_code();
    }
    Ok(Report::new(human, json_out).code(code))
}

async fn wait(tools: &Tools, args: WaitArgs) -> CliResult<Report> {
    let mut targets = Vec::with_capacity(args.chats.len());
    for key in &args.chats {
        targets.push((resolve(tools, key).await?, None));
    }
    let settled = wait_settle(tools.zeron.as_ref(), targets, args.any, args.timeout)
        .await
        .map_err(|e| Failure::error(e.to_string()))?;
    let mut human = String::new();
    let mut rows = Vec::with_capacity(settled.len());
    let mut worst = WaitOutcome::Completed;
    for s in &settled {
        if s.outcome.severity() > worst.severity() {
            worst = s.outcome;
        }
        let reply = tools
            .last_reply(&s.chat)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        human.push_str(&format!("{}: {}\n", title_of(&s.chat), s.outcome.phrase()));
        for line in reply.lines().take(20) {
            human.push_str(line);
            human.push('\n');
        }
        rows.push(json!({
            "chatId": s.chat.id,
            "title": s.chat.title,
            "outcome": s.outcome.label(),
            "turnKey": s.turn_key,
            "reply": reply,
        }));
    }
    while human.ends_with('\n') {
        human.pop();
    }
    ack_observed(tools.zeron.as_ref(), &settled).await;
    // --any: the exit code is the settled outcome, not pending stragglers'.
    let code = if args.any {
        settled
            .iter()
            .filter(|s| s.outcome != WaitOutcome::TimedOut)
            .map(|s| s.outcome)
            .max_by_key(|o| o.severity())
            .unwrap_or(WaitOutcome::TimedOut)
            .exit_code()
    } else {
        worst.exit_code()
    };
    Ok(Report::new(human, json!({ "chats": rows })).code(code))
}

async fn output(tools: &Tools, args: ChatArg) -> CliResult<Report> {
    let chat = resolve(tools, &args.chat).await?;
    let reply = tools
        .last_reply(&chat)
        .await
        .map_err(|e| Failure::error(e.to_string()))?
        .ok_or_else(|| {
            Failure::error(format!("chat {} has no settled reply yet", short(&chat.id)))
                .hint("wait for a turn to finish, or read `zeron chat log` for context")
        })?;
    // Reading a child from inside its parent acks the settle the parent was
    // told about — the observed key comes from the current session row.
    if let Ok(sessions) = tools.zeron.sessions().await
        && let Some(session) = session_for(&sessions, &chat)
        && let Some(update) = child_update(None, &session)
    {
        let s = Settled {
            chat: chat.clone(),
            outcome: WaitOutcome::from_update(&update),
            turn_key: Some(update.key),
        };
        ack_observed(tools.zeron.as_ref(), &[s]).await;
    }
    Ok(Report::new(
        reply.clone(),
        json!({ "chatId": chat.id, "title": chat.title, "reply": reply }),
    ))
}

async fn show(tools: &Tools, args: ChatArg) -> CliResult<Report> {
    let chat = resolve(tools, &args.chat).await?;
    let (spaces, sessions, entries) = tokio::try_join!(
        tools.zeron.spaces(),
        tools.zeron.sessions(),
        tools.zeron.transcript(&chat.id),
    )
    .map_err(|e| Failure::error(e.to_string()))?;
    let space = chat
        .space_id
        .as_deref()
        .and_then(|id| spaces.iter().find(|s| s.id == id));
    let session = session_for(&sessions, &chat);
    let children: Vec<Chat> = tools
        .zeron
        .chats()
        .await
        .map_err(|e| Failure::error(e.to_string()))?
        .into_iter()
        .filter(|c| c.parent_chat_id.as_deref() == Some(chat.id.as_str()))
        .collect();
    let rendered = crate::transcript::render_entries(&entries, Default::default());
    let pending = rendered.iter().rev().find_map(|m| m.pending_input.clone());
    let pending_labels: Vec<String> = pending
        .as_ref()
        .and_then(|p| p["questions"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|q| {
            let header = q["header"].as_str().unwrap_or("question");
            Some(format!(
                "{header}: {}",
                q["options"]
                    .as_array()?
                    .iter()
                    .filter_map(|o| o.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            ))
        })
        .collect();
    let child_rows: Vec<Value> = children
        .iter()
        .map(|c| {
            let session = session_for(&sessions, c);
            let status = display_word(c, session.as_ref());
            json!({ "id": c.id, "title": c.title, "status": status })
        })
        .collect();
    let status = display_word(&chat, session.as_ref());
    fn kebab(v: &impl serde::Serialize) -> String {
        serde_json::to_value(v)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "default".into())
    }
    let config_line = match &chat.config {
        Some(c) => format!(
            "{} · {} · {}",
            kebab(&c.harness),
            c.model.as_deref().unwrap_or("default"),
            c.reasoning
                .as_ref()
                .map(kebab)
                .unwrap_or_else(|| "default".into()),
        ),
        None => "default".into(),
    };
    let mut human = format!(
        "title:    {}\nchat:     {}\nstatus:   {}\nconfig:   {}\n",
        title_of(&chat),
        chat.id,
        status,
        config_line,
    );
    if let Some(space) = space {
        human.push_str(&format!(
            "project:  {} ({})\n",
            space.display_name(),
            space.path
        ));
    }
    human.push_str(&format!("device:   {}\n", chat.device_id));
    if let Some(cwd) = &chat.cwd {
        human.push_str(&format!("cwd:      {cwd}\n"));
    }
    if let Some(branch) = &chat.branch {
        human.push_str(&format!("branch:   {branch}\n"));
    }
    if let Some(parent) = &chat.parent_chat_id {
        human.push_str(&format!("parent:   {CHAT_MENTION_PREFIX}{parent}\n"));
    }
    human.push_str(&format!(
        "spawned:  {}\n",
        if chat.spawned_by_agent {
            "by an agent"
        } else {
            "by a user"
        }
    ));
    if !pending_labels.is_empty() {
        human.push_str("pending input:\n");
        for label in &pending_labels {
            human.push_str(&format!("  - {label}\n"));
        }
    }
    if !child_rows.is_empty() {
        human.push_str("children:\n");
        for row in &child_rows {
            human.push_str(&format!(
                "  {} {} {}\n",
                short(row["id"].as_str().unwrap_or_default()),
                row["status"].as_str().unwrap_or("idle"),
                row["title"].as_str().unwrap_or("untitled"),
            ));
        }
    }
    let human = human.trim_end().to_owned();
    Ok(Report::new(
        human,
        json!({
            "chatId": chat.id,
            "title": chat.title,
            "status": status,
            "harness": chat.config.as_ref().map(|c| c.harness),
            "model": chat.config.as_ref().and_then(|c| c.model.clone()),
            "reasoning": chat.config.as_ref().and_then(|c| c.reasoning),
            "sandbox": chat.config.as_ref().map(|c| c.sandbox),
            "project": space.map(|s| json!({ "id": s.id, "name": s.display_name(), "path": s.path })),
            "deviceId": chat.device_id,
            "cwd": chat.cwd,
            "branch": chat.branch,
            "archived": chat.archived,
            "parentChatId": chat.parent_chat_id,
            "spawnedByAgent": chat.spawned_by_agent,
            "pendingInput": pending,
            "children": child_rows,
        }),
    ))
}

async fn log(tools: &Tools, args: LogArgs) -> CliResult<Report> {
    let chat = resolve(tools, &args.chat).await?;
    let result = tools
        .call(
            "read_chat",
            json!({
                "chat": chat.id,
                "limit": args.limit.unwrap_or(40),
                "include_reasoning": args.reasoning,
                "include_tools": args.tools,
            }),
        )
        .await
        .map_err(Failure::error)?;
    let mut human = String::new();
    for m in result["messages"].as_array().into_iter().flatten() {
        let role = m["role"].as_str().unwrap_or("?");
        let iso = m["createdAtIso"].as_str().unwrap_or_default();
        human.push_str(&format!("== {role} {iso}\n"));
        if let Some(text) = m["text"].as_str().filter(|t| !t.is_empty()) {
            human.push_str(text);
            human.push('\n');
        }
        for line in m["tools"].as_array().into_iter().flatten() {
            human.push_str(&format!("  {}\n", line.as_str().unwrap_or_default()));
        }
        if let Some(reasoning) = m["reasoning"].as_str() {
            for line in reasoning.lines() {
                human.push_str(&format!("  {line}\n"));
            }
        }
        for error in m["errors"].as_array().into_iter().flatten() {
            human.push_str(&format!(
                "  error: {}\n",
                error.as_str().unwrap_or_default()
            ));
        }
    }
    if human.is_empty() {
        human.push_str("(no messages)");
    }
    Ok(Report::new(human.trim_end().to_owned(), result))
}

async fn list(tools: &Tools, args: ListArgs) -> CliResult<Report> {
    let parent = match args.children.as_deref() {
        Some(key) => Some(resolve(tools, key).await?.id),
        None => None,
    };
    let mut call_args = json!({ "include_archived": args.archived });
    if let Some(parent) = &parent {
        call_args["parent"] = json!(parent);
    }
    if let Some(project) = &args.project {
        call_args["project"] = json!(project);
    }
    let result = tools
        .call("list_chats", call_args)
        .await
        .map_err(Failure::error)?;
    let mut human = String::new();
    for c in result["chats"].as_array().into_iter().flatten() {
        let config = format!(
            "{} · {}",
            c["harness"].as_str().unwrap_or("?"),
            c["model"].as_str().unwrap_or("default")
        );
        human.push_str(&format!(
            "{}  {:<15} {}  ({})\n",
            short(c["id"].as_str().unwrap_or_default()),
            c["status"].as_str().unwrap_or("idle"),
            c["title"].as_str().unwrap_or("untitled"),
            config,
        ));
    }
    if human.is_empty() {
        human.push_str("(no chats)");
    }
    Ok(Report::new(human.trim_end().to_owned(), result))
}

async fn interrupt(tools: &Tools, args: ChatArg) -> CliResult<Report> {
    let chat = resolve(tools, &args.chat).await?;
    let result = tools
        .call("interrupt_chat", json!({ "chat": chat.id }))
        .await
        .map_err(Failure::error)?;
    Ok(Report::new(
        format!("Interrupted {}", title_of(&chat)),
        result,
    ))
}

async fn answer(tools: &Tools, args: AnswerArgs) -> CliResult<Report> {
    let chat = resolve(tools, &args.chat).await?;
    // `<answer>...` maps onto the pending question's labels. A single
    // question takes every answer (multi-select); several questions pair
    // positionally.
    let request_id = match args.request {
        Some(id) => id,
        None => {
            let entries = tools
                .zeron
                .transcript(&chat.id)
                .await
                .map_err(|e| Failure::error(e.to_string()))?;
            let rendered = crate::transcript::render_entries(&entries, Default::default());
            rendered
                .iter()
                .rev()
                .find_map(|m| m.pending_input.clone())
                .and_then(|p| p["requestId"].as_str().map(str::to_owned))
                .ok_or_else(|| {
                    Failure::error(format!("chat {} has no pending question", short(&chat.id)))
                })?
        }
    };
    // Question count decides how the positional answers split.
    let question_ids: Vec<String> = {
        let entries = tools
            .zeron
            .transcript(&chat.id)
            .await
            .map_err(|e| Failure::error(e.to_string()))?;
        let rendered = crate::transcript::render_entries(&entries, Default::default());
        rendered
            .iter()
            .rev()
            .find_map(|m| m.pending_input.clone())
            .filter(|p| p["requestId"].as_str() == Some(request_id.as_str()))
            .and_then(|p| p["questions"].as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|q| q["id"].as_str().map(str::to_owned))
            .collect()
    };
    let answers: Vec<Value> = match question_ids.len() {
        0 => {
            // No transcript visibility of the request (remote doc not synced
            // yet): send the labels against the request id directly.
            vec![json!({ "question_id": request_id, "labels": args.answers })]
        }
        1 => vec![json!({ "question_id": question_ids[0], "labels": args.answers })],
        n => {
            if args.answers.len() != n {
                return Err(Failure::error(format!(
                    "{n} questions are pending but {} answers were given",
                    args.answers.len()
                ))
                .hint("give one answer per pending question, in order"));
            }
            question_ids
                .into_iter()
                .zip(args.answers)
                .map(|(q, a)| json!({ "question_id": q, "labels": [a] }))
                .collect()
        }
    };
    let result = tools
        .call(
            "respond_to_input",
            json!({ "chat": chat.id, "request_id": request_id, "answers": answers }),
        )
        .await
        .map_err(Failure::error)?;
    Ok(Report::new(format!("Answered {}", title_of(&chat)), result))
}

async fn archive(tools: &Tools, args: ArchiveArgs) -> CliResult<Report> {
    let chat = resolve(tools, &args.chat).await?;
    let archived = !args.unarchive;
    let result = tools
        .call(
            "archive_chat",
            json!({ "chat": chat.id, "archived": archived }),
        )
        .await
        .map_err(Failure::error)?;
    Ok(Report::new(
        format!(
            "{} {}",
            if archived { "Archived" } else { "Unarchived" },
            title_of(&chat)
        ),
        result,
    ))
}

async fn fork(tools: &Tools, args: ForkArgs) -> CliResult<Report> {
    let chat = resolve(tools, &args.chat).await?;
    let prompt = match (args.prompt, args.prompt_file) {
        (None, None) => None,
        (a, b) => Some(read_text_arg(a, b, "prompt")?),
    };
    let result = tools
        .fork_chat(crate::tools::ForkArgs {
            chat: chat.id.clone(),
            prompt,
            wait: false,
            timeout_secs: None,
        })
        .await
        .map_err(|e| Failure::error(e.to_string()))?;
    let fork_id = result["chatId"].as_str().unwrap_or_default();
    Ok(Report::new(
        format!(
            "Forked {} → {CHAT_MENTION_PREFIX}{fork_id}",
            title_of(&chat)
        ),
        result,
    ))
}

async fn harness_list(tools: &Tools) -> CliResult<Report> {
    let result = tools
        .call("list_harnesses", json!({}))
        .await
        .map_err(Failure::error)?;
    let mut human = String::new();
    for h in result["harnesses"].as_array().into_iter().flatten() {
        human.push_str(&format!(
            "{:<14} {}  {}\n",
            h["id"].as_str().unwrap_or("?"),
            h["name"].as_str().unwrap_or("?"),
            if h["available"].as_bool() == Some(true) {
                "available"
            } else {
                "unavailable"
            },
        ));
    }
    Ok(Report::new(human.trim_end().to_owned(), result))
}

async fn model_list(tools: &Tools, harness: &str) -> CliResult<Report> {
    let result = tools
        .call("list_models", json!({ "harness": harness }))
        .await
        .map_err(Failure::error)?;
    let mut human = String::new();
    for m in result["models"].as_array().into_iter().flatten() {
        let levels = m["reasoningLevels"]
            .as_array()
            .map(|l| {
                l.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        human.push_str(&format!(
            "{:<20} {}{}\n",
            m["id"].as_str().unwrap_or("?"),
            m["label"].as_str().unwrap_or("?"),
            if levels.is_empty() {
                String::new()
            } else {
                format!("  reasoning: {levels}")
            },
        ));
    }
    Ok(Report::new(human.trim_end().to_owned(), result))
}

#[cfg(test)]
mod tests {
    //! `zeron chat` against the in-memory engine stub — same transport path
    //! as the real socket (`memory_client` speaks the same envelopes).
    use super::*;
    use async_trait::async_trait;
    use futures::StreamExt;
    use std::sync::Mutex;
    use zeron_rpc::{RpcError, RpcReply, RpcService, memory_client};

    /// A configurable workspace: chats, session-frame script (each
    /// WATCH_SESSIONS subscribe replays the frames then stays open), spaces,
    /// a fixed transcript. Writes and RPC calls are recorded for asserts.
    #[derive(Default)]
    struct Stub {
        writes: Mutex<Vec<(String, Value)>>,
        chats: Vec<Value>,
        session_frames: Vec<Value>,
        spaces: Vec<Value>,
        harnesses: Vec<Value>,
        models: Vec<Value>,
        doc: Vec<Value>,
    }

    fn chat_json(id: &str, title: Option<&str>, parent: Option<&str>, spawned: bool) -> Value {
        json!({
            "id": id, "deviceId": "dev-local", "title": title,
            "parentChatId": parent, "spawnedByAgent": spawned,
            "archived": false, "spaceId": "space-1",
            "config": { "harness": "claude-code", "model": "opus",
                        "reasoning": null, "sandbox": "workspace-write" },
            "createdAt": "2026-09-01T00:00:00Z"
        })
    }

    fn session_json(chat: &str, status: &str, turn: Option<&str>, updated_ms: i64) -> Value {
        let at = |ms: i64| {
            chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
                .unwrap()
                .to_rfc3339()
        };
        json!({
            "chatId": chat, "deviceId": "dev-local", "status": status,
            "lastCompletedTurn": turn, "startedAt": null,
            "updatedAt": at(updated_ms)
        })
    }

    fn now_ms() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    #[async_trait]
    impl RpcService for Stub {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            Ok(match method {
                methods::LOCAL_DEVICE => RpcReply::Value(json!({ "deviceId": "dev-local" })),
                methods::ENGINE_INFO => {
                    RpcReply::Value(json!({ "deviceId": "dev-local", "workspaceScope": "local" }))
                }
                methods::WATCH_DEVICES => RpcReply::Stream(
                    futures::stream::iter(vec![
                        json!([{ "id": "dev-local", "name": "Laptop", "platform": "linux" }]),
                    ])
                    .boxed(),
                ),
                methods::WATCH_SPACES => {
                    RpcReply::Stream(futures::stream::iter(vec![json!(self.spaces)]).boxed())
                }
                methods::WATCH_CHATS => {
                    RpcReply::Stream(futures::stream::iter(vec![json!(self.chats)]).boxed())
                }
                // Each attach replays the scripted frames, then stays open —
                // a settled chat is detected inside one subscription.
                methods::WATCH_SESSIONS => {
                    let frames = if self.session_frames.is_empty() {
                        vec![json!([])]
                    } else {
                        self.session_frames.clone()
                    };
                    RpcReply::Stream(
                        futures::stream::iter(frames)
                            .chain(futures::stream::pending())
                            .boxed(),
                    )
                }
                methods::LIST_HARNESSES => RpcReply::Value(json!(self.harnesses)),
                methods::LIST_MODELS => RpcReply::Value(json!(self.models)),
                methods::WATCH_DOC_MESSAGES => RpcReply::Stream(
                    futures::stream::iter(vec![json!({ "reset": self.doc })]).boxed(),
                ),
                methods::MUTATE | methods::QUEUE_COMMAND | methods::QUEUE_MESSAGE => {
                    self.writes
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params));
                    RpcReply::Value(json!({ "commandId": "cmd-1", "id": "q-1" }))
                }
                methods::ACK_CHILD_UPDATES => {
                    self.writes
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params));
                    RpcReply::Value(json!({}))
                }
                methods::FORK_SIDE_CHAT => {
                    self.writes
                        .lock()
                        .unwrap()
                        .push((method.to_owned(), params.clone()));
                    let source = self
                        .chats
                        .iter()
                        .find(|c| c["id"] == params["sourceChatId"])
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let mut fork = source;
                    fork["id"] = params["chatId"].clone();
                    fork["parentChatId"] = params["parentChatId"].clone();
                    RpcReply::Value(fork)
                }
                other => return Err(RpcError::UnknownMethod(other.into())),
            })
        }
    }

    fn base_stub() -> Stub {
        Stub {
            chats: vec![chat_json("chat-parent-1", Some("Parent"), None, false)],
            harnesses: vec![json!({
                "id": "claude-code", "name": "Claude Code",
                "supportsSteering": true, "steeringMode": "step-boundary",
                "reasoningLevels": [], "installed": true, "enabled": true
            })],
            models: vec![json!({ "id": "opus", "label": "Opus" })],
            spaces: vec![json!({
                "id": "space-1", "deviceId": "dev-local", "path": "/repo/comet",
                "gitDetected": true, "createdAt": "2026-09-01T00:00:00Z"
            })],
            doc: vec![
                json!({ "id": "u1", "role": "user", "createdAt": 1, "deviceId": "dev-local",
                        "parts": [{ "kind": "text", "id": "t", "text": "hi" }] }),
                json!({ "id": "a1", "role": "assistant", "createdAt": 2, "deviceId": "dev-local",
                        "status": "complete",
                        "parts": [{ "kind": "text", "id": "t", "text": "the answer" }] }),
            ],
            ..Default::default()
        }
    }

    fn tools_for(stub: Stub, origin: Option<&str>) -> (Tools, Arc<Stub>) {
        let stub = Arc::new(stub);
        let client = memory_client(stub.clone());
        let tools = Tools::new(Arc::new(Zeron::with_client(
            client,
            Origin {
                chat_id: origin.map(str::to_owned),
                device_id: Some("dev-local".into()),
            },
        )));
        (tools, stub)
    }

    fn fixture(origin: Option<&str>) -> (Tools, Arc<Stub>) {
        tools_for(base_stub(), origin)
    }

    fn spawn_args() -> SpawnArgs {
        SpawnArgs {
            prompt: Some("go".into()),
            prompt_file: None,
            harness: None,
            model: None,
            reasoning: None,
            title: Some("Child".into()),
            project: None,
            device: None,
            worktree: false,
            base: None,
            same_checkout: false,
            cwd: None,
            sandbox: None,
            parent: None,
            no_parent: false,
            wait: false,
            timeout: Duration::from_secs(1),
            json: JsonFlag { json: false },
        }
    }

    #[tokio::test]
    async fn spawn_inherits_parent_and_defaults_to_a_worktree() {
        let (tools, stub) = fixture(Some("chat-parent-1"));
        let report = spawn(&tools, spawn_args()).await.unwrap();
        let last = report.human.lines().last().unwrap_or_default();
        let id = report.json["chatId"].as_str().unwrap().to_owned();
        assert_eq!(last, format!("@chat:{id}"), "{}", report.human);
        assert_eq!(report.json["harness"], "claude-code");
        assert_eq!(report.json["model"], "opus");
        assert_eq!(report.json["sandbox"], "workspace-write");
        assert_eq!(report.json["parentChatId"], "chat-parent-1");
        assert_eq!(report.json["spawnedByAgent"], true);
        let writes = stub.writes.lock().unwrap();
        assert_eq!(writes[0].1["op"], "createChat");
        assert_eq!(writes[0].1["spawnedByAgent"], true);
        assert_eq!(writes[0].1["parentChatId"], "chat-parent-1");
        // Git project → the Run carries a host-side worktree spec off the
        // repo path.
        let run = &writes[2].1["command"]["request"];
        assert_eq!(writes[2].1["command"]["kind"], "run");
        assert_eq!(run["worktree"]["repoPath"], "/repo/comet");
        assert_eq!(run["worktree"]["base"], "HEAD");
        assert_eq!(run["worktree"]["spaceId"], "space-1");
    }

    #[tokio::test]
    async fn spawn_same_checkout_and_cwd_and_no_parent() {
        let (tools, stub) = fixture(Some("chat-parent-1"));
        let mut args = spawn_args();
        args.same_checkout = true;
        let report = spawn(&tools, args).await.unwrap();
        assert!(report.json["worktree"].is_null());
        {
            let run = stub.writes.lock().unwrap()[2].1["command"]["request"].clone();
            assert!(run["worktree"].is_null());
            // Parent has no cwd of its own → falls back to the project root.
            assert_eq!(run["cwd"], "/repo/comet");
        }

        let mut args = spawn_args();
        args.cwd = Some("/tmp/scratch".into());
        let report = spawn(&tools, args).await.unwrap();
        assert_eq!(report.json["cwd"], "/tmp/scratch");

        let mut args = spawn_args();
        args.no_parent = true;
        let report = spawn(&tools, args).await.unwrap();
        assert!(report.json["parentChatId"].is_null());
    }

    #[tokio::test]
    async fn spawn_refuses_past_depth_four() {
        // Chain: top → a → b → c → d. Spawning under d nests 5 deep.
        let mut stub = base_stub();
        let chain = [
            ("c-a", Some("chat-parent-1")),
            ("c-b", Some("c-a")),
            ("c-c", Some("c-b")),
            ("c-d", Some("c-c")),
        ];
        for (id, parent) in chain {
            stub.chats.push(chat_json(id, None, parent, true));
        }
        let (tools, _) = tools_for(stub, Some("chat-parent-1"));
        let mut args = spawn_args();
        args.parent = Some("c-d".into());
        let err = spawn(&tools, args).await.unwrap_err();
        assert_eq!(err.code, 1);
        assert!(err.message.contains("depth"), "{}", err.message);
        // But under c-c (depth 3 → child 4) it is allowed.
        let mut args = spawn_args();
        args.parent = Some("c-c".into());
        spawn(&tools, args).await.unwrap();
    }

    #[tokio::test]
    async fn spawn_refuses_over_eight_running_children() {
        let mut stub = base_stub();
        for i in 0..8 {
            let id = format!("c-kid-{i}");
            stub.chats
                .push(chat_json(&id, None, Some("chat-parent-1"), true));
        }
        stub.session_frames = vec![json!(
            stub.chats
                .iter()
                .filter(|c| c["spawnedByAgent"] == true)
                .map(|c| session_json(c["id"].as_str().unwrap(), "working", None, now_ms()))
                .collect::<Vec<_>>()
        )];
        let (tools, _) = tools_for(stub, Some("chat-parent-1"));
        let err = spawn(&tools, spawn_args()).await.unwrap_err();
        assert_eq!(err.code, 5, "{}", err.message);
        assert_eq!(
            err.hint.as_deref(),
            Some("wait for a child to finish or archive one")
        );
        // A sibling not spawned by an agent does not count against the cap.
        let mut stub = base_stub();
        for i in 0..8 {
            let id = format!("c-kid-{i}");
            stub.chats
                .push(chat_json(&id, None, Some("chat-parent-1"), false));
        }
        stub.session_frames = vec![json!(
            stub.chats
                .iter()
                .filter(|c| c["id"].as_str().unwrap_or_default().starts_with("c-kid"))
                .map(|c| session_json(c["id"].as_str().unwrap(), "working", None, now_ms()))
                .collect::<Vec<_>>()
        )];
        let (tools, _) = tools_for(stub, Some("chat-parent-1"));
        spawn(&tools, spawn_args()).await.unwrap();
    }

    fn wait_args(chats: &[&str], any: bool, timeout: Duration) -> WaitArgs {
        WaitArgs {
            chats: chats.iter().map(|c| c.to_string()).collect(),
            any,
            timeout,
            json: JsonFlag { json: false },
        }
    }

    #[tokio::test]
    async fn wait_reports_each_outcome_with_its_exit_code() {
        // Already-settled rows answer immediately off their first snapshot.
        for (status, turn, outcome, code) in [
            ("idle", Some("t1"), "completed", 0),
            ("errored", Some("t1"), "errored", 3),
            ("awaitingInput", Some("t1"), "needsInput", 2),
        ] {
            let mut stub = base_stub();
            stub.chats
                .push(chat_json("c-1", Some("Kid"), Some("chat-parent-1"), true));
            stub.session_frames = vec![json!([session_json("c-1", status, turn, 1)])];
            let (tools, _) = tools_for(stub, None);
            let report = wait(&tools, wait_args(&["c-1"], false, Duration::from_secs(5)))
                .await
                .unwrap();
            assert_eq!(report.code, code, "{status}");
            assert_eq!(report.json["chats"][0]["outcome"], outcome);
            assert_eq!(report.json["chats"][0]["reply"], "the answer");
            assert!(report.human.contains("Kid: "));
        }

        // Working → Idle with an UNCHANGED marker is an interrupt.
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", Some("Kid"), Some("chat-parent-1"), true));
        stub.session_frames = vec![
            json!([session_json("c-1", "working", Some("t1"), now_ms())]),
            json!([session_json("c-1", "idle", Some("t1"), now_ms() + 5)]),
        ];
        let (tools, _) = tools_for(stub, None);
        let report = wait(&tools, wait_args(&["c-1"], false, Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(report.code, 4);
        assert_eq!(report.json["chats"][0]["outcome"], "interrupted");
        assert!(
            report.json["chats"][0]["turnKey"]
                .as_str()
                .unwrap()
                .starts_with("stopped:")
        );

        // A stale Working row counts as settled-unknown, not a hang.
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json(
            "c-1",
            "working",
            None,
            now_ms() - 120_000
        )])];
        let (tools, _) = tools_for(stub, None);
        let report = wait(&tools, wait_args(&["c-1"], false, Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(report.code, 1);
        assert_eq!(report.json["chats"][0]["outcome"], "unknown");

        // Nothing settling inside the deadline is a timeout.
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json("c-1", "working", None, now_ms())])];
        let (tools, _) = tools_for(stub, None);
        let report = wait(
            &tools,
            wait_args(&["c-1"], false, Duration::from_millis(300)),
        )
        .await
        .unwrap();
        assert_eq!(report.code, 124);
        assert_eq!(report.json["chats"][0]["outcome"], "timedOut");
    }

    #[tokio::test]
    async fn wait_any_returns_on_the_first_settle() {
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.chats
            .push(chat_json("c-2", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([
            session_json("c-1", "working", None, now_ms()),
            session_json("c-2", "idle", Some("t9"), 3),
        ])];
        let (tools, _) = tools_for(stub, None);
        let report = wait(
            &tools,
            wait_args(&["c-1", "c-2"], true, Duration::from_secs(5)),
        )
        .await
        .unwrap();
        assert_eq!(report.code, 0);
        let outcomes: Vec<&str> = report.json["chats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["outcome"].as_str().unwrap())
            .collect();
        assert_eq!(outcomes, ["timedOut", "completed"]);
    }

    #[tokio::test]
    async fn wait_acks_only_when_the_caller_is_the_parent() {
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json("c-1", "idle", Some("t1"), 1)])];

        // Run inside the parent: the settle is acked.
        let (tools, stub) = tools_for(stub, Some("chat-parent-1"));
        wait(&tools, wait_args(&["c-1"], false, Duration::from_secs(5)))
            .await
            .unwrap();
        {
            let writes = stub.writes.lock().unwrap();
            let acks: Vec<_> = writes
                .iter()
                .filter(|(m, _)| m == methods::ACK_CHILD_UPDATES)
                .collect();
            assert_eq!(acks.len(), 1);
            assert_eq!(acks[0].1["parentChatId"], "chat-parent-1");
            assert_eq!(acks[0].1["updates"][0]["childChatId"], "c-1");
            assert_eq!(acks[0].1["updates"][0]["turnKey"], "done:t1");
        }

        // From an unrelated chat: no ack.
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json("c-1", "idle", Some("t1"), 1)])];
        let (tools, stub) = tools_for(stub, Some("chat-other"));
        wait(&tools, wait_args(&["c-1"], false, Duration::from_secs(5)))
            .await
            .unwrap();
        assert!(
            !stub
                .writes
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == methods::ACK_CHILD_UPDATES)
        );
    }

    #[tokio::test]
    async fn output_prints_the_last_reply_and_acks() {
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json("c-1", "idle", Some("t1"), 1)])];
        let (tools, stub) = tools_for(stub, Some("chat-parent-1"));
        let report = output(
            &tools,
            ChatArg {
                chat: "c-1".into(),
                json: JsonFlag { json: false },
            },
        )
        .await
        .unwrap();
        assert_eq!(report.human, "the answer");
        assert_eq!(report.code, 0);
        assert!(
            stub.writes
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == methods::ACK_CHILD_UPDATES)
        );

        // A chat with no settled reply fails with a hint.
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.doc = vec![];
        let (tools, _) = tools_for(stub, None);
        let err = output(
            &tools,
            ChatArg {
                chat: "c-1".into(),
                json: JsonFlag { json: false },
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, 1);
        assert!(err.hint.is_some());
    }

    #[tokio::test]
    async fn tell_attributes_and_maps_outcome_codes() {
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", Some("Kid"), Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json("c-1", "idle", Some("t1"), 1)])];
        let (tools, stub) = tools_for(stub, Some("chat-parent-1"));
        let report = tell(
            &tools,
            TellArgs {
                chat: "c-1".into(),
                text: Some("ping".into()),
                message_file: None,
                mode: "auto".into(),
                wait: false,
                timeout: Duration::from_secs(1),
                json: JsonFlag { json: false },
            },
        )
        .await
        .unwrap();
        assert_eq!(report.code, 0);
        let writes = stub.writes.lock().unwrap();
        let prompt = writes[0].1["command"]["request"]["prompt"]
            .as_str()
            .unwrap();
        assert!(
            prompt.starts_with(
                "[Message from Zeron chat Parent (@chat:chat-parent-1). Reply with `zeron chat tell chat-par <message>`.]"
            ),
            "{prompt}"
        );
    }

    #[tokio::test]
    async fn tell_to_a_blocked_chat_exits_2() {
        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json(
            "c-1",
            "awaitingInput",
            None,
            now_ms()
        )])];
        let (tools, _) = tools_for(stub, Some("chat-parent-1"));
        let err = tell(
            &tools,
            TellArgs {
                chat: "c-1".into(),
                text: Some("ping".into()),
                message_file: None,
                mode: "auto".into(),
                wait: false,
                timeout: Duration::from_secs(1),
                json: JsonFlag { json: false },
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, 2);
        assert!(err.hint.as_deref().unwrap().contains("answer"));
    }

    #[tokio::test]
    async fn resolve_accepts_self_prefix_title_and_mention() {
        let (tools, _) = fixture(Some("chat-parent-1"));
        assert_eq!(resolve(&tools, "self").await.unwrap().id, "chat-parent-1");
        assert_eq!(
            resolve(&tools, "chat-pa").await.unwrap().id,
            "chat-parent-1"
        );
        assert_eq!(resolve(&tools, "Parent").await.unwrap().id, "chat-parent-1");
        assert_eq!(
            resolve(&tools, "@chat:chat-parent-1").await.unwrap().id,
            "chat-parent-1"
        );
        // Without an origin, `self` is a clear error.
        let (tools, _) = fixture(None);
        let err = resolve(&tools, "self").await.unwrap_err();
        assert!(err.message.contains("ZERON_CHAT_ID"));
    }

    #[tokio::test]
    async fn json_shapes_stay_stable() {
        let (tools, _) = fixture(Some("chat-parent-1"));
        let report = spawn(&tools, spawn_args()).await.unwrap();
        let keys: Vec<&str> = report
            .json
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        // serde_json maps are ordered: the contract is the key set.
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            [
                "chatId",
                "cwd",
                "deviceId",
                "harness",
                "model",
                "parentChatId",
                "project",
                "reasoning",
                "sandbox",
                "sent",
                "spawnedByAgent",
                "title",
                "worktree"
            ]
        );

        let mut stub = base_stub();
        stub.chats
            .push(chat_json("c-1", None, Some("chat-parent-1"), true));
        stub.session_frames = vec![json!([session_json("c-1", "idle", Some("t1"), 1)])];
        let (tools, _) = tools_for(stub, None);
        let report = wait(&tools, wait_args(&["c-1"], false, Duration::from_secs(5)))
            .await
            .unwrap();
        let keys: Vec<&str> = report.json["chats"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect::<Vec<_>>();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, ["chatId", "outcome", "reply", "title", "turnKey"]);
    }

    #[test]
    fn forgiving_aliases_parse_and_unknown_names_are_caught() {
        let parse = |argv: &[&str]| {
            let matches =
                <ChatCommand as Subcommand>::augment_subcommands(clap::Command::new("chat"))
                    .try_get_matches_from(argv.iter().copied())
                    .unwrap();
            <ChatCommand as clap::FromArgMatches>::from_arg_matches(&matches).unwrap()
        };
        assert!(matches!(
            parse(&["chat", "read", "c-1"]),
            ChatCommand::Output(_)
        ));
        assert!(matches!(
            parse(&["chat", "output", "c-1"]),
            ChatCommand::Output(_)
        ));
        assert!(matches!(
            parse(&["chat", "transcript", "c-1"]),
            ChatCommand::Log(_)
        ));
        assert!(matches!(
            parse(&["chat", "history", "c-1"]),
            ChatCommand::Log(_)
        ));
        // Anything else is caught so `run_chat` can print the suggestion.
        match parse(&["chat", "reed", "c-1"]) {
            ChatCommand::Unknown(args) => assert_eq!(args[0], "reed"),
            _ => panic!("expected the catch-all"),
        }
        assert_eq!(unknown_subcommand(&["reed".into()]), 1);
    }
}
