//! Frame → [`AgentEvent`] normalization (init dedupe, subagent tagging, tool
//! decoding, error-code mapping).

use serde_json::Value;
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, TodoItem, TodoStatus, ToolCall};

use super::wire::{ContentBlock, Frame, MessageFrame, ResultFrame};

/// Human-readable text for the CLI's assistant-level error codes. These arrive
/// as a terse `error` field on an `assistant` frame — usually with NO text
/// content and NOT as a `result` error — so a usage-limited or otherwise failed
/// turn looks like the agent simply never replied unless we surface it.
fn assistant_error_text(code: &str) -> String {
    match code {
        "authentication_failed" => "Authentication failed — sign in to Claude again.".into(),
        "oauth_org_not_allowed" => "This organization isn't allowed to use Claude here.".into(),
        "billing_error" => "Billing error — check your Claude plan or payment method.".into(),
        "rate_limit" => "Claude usage limit reached — try again after the limit resets.".into(),
        "overloaded" => "Claude is overloaded right now — try again shortly.".into(),
        "invalid_request" => "The request was rejected as invalid.".into(),
        "model_not_found" => "The selected model isn't available.".into(),
        "server_error" => "Claude had a server error — try again.".into(),
        "max_output_tokens" => "The reply hit the maximum output length.".into(),
        "unknown" => "Claude returned an unspecified error.".into(),
        other => format!("Claude error: {other}"),
    }
}

/// Which claude.ai usage window a `rate_limit_event` refers to.
fn rate_window_label(kind: &str) -> &'static str {
    match kind {
        "five_hour" => "5-hour",
        "seven_day" | "seven_day_overage_included" => "weekly",
        "seven_day_opus" => "weekly (Opus)",
        "seven_day_sonnet" => "weekly (Sonnet)",
        "overage" => "overage",
        _ => "usage",
    }
}

/// Fallback wording for a `result` error whose `errors` array is empty, so the
/// turn never ends with a blank (and therefore invisible) error.
fn result_error_text(subtype: &str) -> &'static str {
    match subtype {
        "error_max_turns" => "The run hit the maximum number of turns.",
        "error_max_budget_usd" => "The run hit its cost budget.",
        "error_max_structured_output_retries" => "The run exhausted its structured-output retries.",
        _ => "The run ended with an error.",
    }
}

/// The CLI seeds `result.errors` with internal `[ede_diagnostic]` breadcrumbs
/// for its error_during_execution telemetry ("turn aborted (…) stop_reason=…",
/// "result_type=… last_content_type=… stop_reason=…"). They're diagnostics
/// about the CLI's own turn accounting, not user-relevant errors — surfacing
/// them verbatim put raw `[ede_diagnostic] result_type=user …` boxes in the
/// transcript. They're debug-logged and dropped instead.
fn is_internal_diagnostic(message: &str) -> bool {
    message.contains("[ede_diagnostic]")
}

const CONTEXT_FULL: &str =
    "The conversation no longer fits the model's context window. Run /compact or start a new chat.";

/// Wording for the `terminal_reason`s that mean the turn failed. Others
/// (`completed`, the aborts, anything newer) are not failures by themselves.
fn terminal_reason_text(reason: &str) -> Option<&'static str> {
    Some(match reason {
        "api_error" => "Claude gave up after repeated API errors.",
        "prompt_too_long" => CONTEXT_FULL,
        // Also how a full context window ends; a rate limit says 429.
        "blocking_limit" => CONTEXT_FULL,
        "malformed_tool_use_exhausted" => "Claude gave up after repeated malformed tool calls.",
        "budget_exhausted" => "Claude stopped: the turn's token budget ran out.",
        "structured_output_retry_exhausted" => {
            "Claude could not produce the requested structured output."
        }
        "tool_deferred_unavailable" => {
            "Claude could not resume a deferred tool call: the tool is no longer available."
        }
        "turn_setup_failed" => "Claude could not start the turn.",
        "rapid_refill_breaker" => {
            "Claude stopped: the context refilled too quickly after compaction."
        }
        "image_error" => "Claude stopped: an image in the conversation could not be processed.",
        "model_error" => "Claude stopped: the model returned an error.",
        _ => return None,
    })
}

/// An abort the CLI files as an ordinary end: a stop, or a `now` steer.
fn aborted(f: &ResultFrame) -> bool {
    matches!(
        f.terminal_reason.as_deref(),
        Some("aborted_streaming" | "aborted_tools")
    )
}

/// A `success` result that is really a failed API call: an invalid key, a
/// rate limit, an overloaded API, a full context window. The CLI's own
/// `result` text says what happened ("Invalid API key · Fix external API
/// key"); a status we recognize gets plainer wording in front of it.
fn api_failure(f: &ResultFrame) -> Option<String> {
    let reason = f.terminal_reason.as_deref().unwrap_or("");
    let status = f.api_error_status;
    if !f.is_error && !matches!(status, Some(429 | 529)) && terminal_reason_text(reason).is_none() {
        return None;
    }
    // The CLI's text carries the specifics (a reset time, the API's own
    // message). Bounded: some are a raw API error body.
    let detail = f
        .result
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(|r| {
            let mut r = r.to_owned();
            if r.len() > 500 {
                let mut end = 500;
                while !r.is_char_boundary(end) {
                    end -= 1;
                }
                r.truncate(end);
                r.push('…');
            }
            r
        });
    let too_long = detail
        .as_deref()
        .is_some_and(|d| d.to_ascii_lowercase().contains("prompt is too long"));
    let headline = match status {
        Some(429) => Some("Claude rate limit reached — try again after it resets."),
        Some(529) => Some("Claude is overloaded right now — try again shortly."),
        _ if too_long => Some(CONTEXT_FULL),
        _ => terminal_reason_text(reason).filter(|_| reason != "api_error" || detail.is_none()),
    };
    Some(match (headline, detail) {
        (Some(headline), Some(detail)) if headline != CONTEXT_FULL => {
            format!("{headline} {detail}")
        }
        (Some(headline), _) => headline.to_owned(),
        (None, Some(detail)) => detail,
        (None, None) => "Claude's API request failed.".to_owned(),
    })
}

fn str_field(input: &Value, key: &str) -> String {
    input.get(key).and_then(Value::as_str).unwrap_or("").into()
}

fn opt_str_field(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// Decode a Claude `tool_use` block (name + input) into a typed [`ToolCall`].
pub(crate) fn decode_tool_use(name: &str, input: &Value) -> ToolCall {
    match name {
        "Bash" => ToolCall::Exec {
            command: str_field(input, "command"),
        },
        "Read" => ToolCall::ReadFile {
            path: str_field(input, "file_path"),
        },
        "Write" => ToolCall::WriteFile {
            path: str_field(input, "file_path"),
            content: opt_str_field(input, "content"),
        },
        "Edit" => ToolCall::EditFile {
            path: str_field(input, "file_path"),
            old_string: opt_str_field(input, "old_string"),
            new_string: opt_str_field(input, "new_string"),
        },
        "Grep" => ToolCall::Search {
            pattern: str_field(input, "pattern"),
            path: opt_str_field(input, "path"),
        },
        "Glob" => ToolCall::Glob {
            pattern: str_field(input, "pattern"),
        },
        "WebFetch" => ToolCall::WebFetch {
            url: str_field(input, "url"),
            prompt: opt_str_field(input, "prompt"),
        },
        "WebSearch" => ToolCall::WebSearch {
            query: str_field(input, "query"),
        },
        "TodoWrite" => ToolCall::Todo {
            items: input
                .get("todos")
                .and_then(Value::as_array)
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|t| {
                    TodoItem::new(
                        str_field(t, "content"),
                        TodoStatus::parse(t.get("status").and_then(Value::as_str).unwrap_or("")),
                    )
                })
                .collect(),
        },
        // The subagent spawn: name the chip — and the tab it opens — after
        // the TASK, not the bare tool ("Agent" alone says nothing in a tab
        // strip; the tool's `description` is where the work is named).
        "Agent" | "Task" => {
            let description = str_field(input, "description");
            ToolCall::Unknown {
                name: if description.is_empty() {
                    "Agent".into()
                } else {
                    format!("Agent: {description}")
                },
                input: (!input.is_null()).then(|| input.clone()),
            }
        }
        // MCP tools arrive as `mcp__<server>__<tool>`.
        _ => match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((server, tool)) => ToolCall::Mcp {
                server: server.into(),
                tool: tool.into(),
                input: (!input.is_null()).then(|| input.clone()),
            },
            None => ToolCall::Unknown {
                name: name.into(),
                input: (!input.is_null()).then(|| input.clone()),
            },
        },
    }
}

/// A forked skill's chip once it runs: a spawn named after the skill
/// (`Agent: /review`), like an Agent call is after its description.
fn forked_skill_call(input: Value) -> ToolCall {
    let name = match input
        .get("skill")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        Some(skill) => format!("Agent: /{skill}"),
        None => "Agent".into(),
    };
    ToolCall::Unknown {
        name,
        input: (!input.is_null()).then_some(input),
    }
}

fn new_message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Wrap an event as subagent-attributed traffic.
fn tag(parent: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: parent.to_owned(),
        event: Box::new(event),
    }
}

/// A subagent's assistant message, attributed. The 2.1.x wire streams NO
/// tagged partial deltas (live-verified): a subagent's text and thinking
/// arrive only as full blocks on its tagged assistant frames — emit them, in
/// block order with the tool calls, or subagent transcripts are
/// tool-chips-only.
fn subagent_message(parent: &str, f: &MessageFrame) -> Vec<AgentEvent> {
    let mut out: Vec<AgentEvent> = f
        .message
        .blocks()
        .filter_map(|b: ContentBlock| match b.kind.as_str() {
            "text" if !b.text.is_empty() => Some(tag(
                parent,
                AgentEvent::TextDelta {
                    text: format!("{}\n\n", b.text.trim_end()),
                },
            )),
            "thinking" if !b.thinking.trim().is_empty() => Some(tag(
                parent,
                AgentEvent::ReasoningDelta {
                    text: format!("{}\n\n", b.thinking.trim_end()),
                },
            )),
            "tool_use" => Some(tag(
                parent,
                AgentEvent::ToolCall {
                    id: b.id.clone(),
                    call: decode_tool_use(&b.name, &b.input),
                },
            )),
            _ => None,
        })
        .collect();
    if let Some(code) = &f.error {
        out.push(tag(
            parent,
            AgentEvent::Error {
                message: assistant_error_text(code),
            },
        ));
    }
    out
}

/// A subagent's tool results, attributed like its calls.
fn subagent_results(parent: &str, f: &MessageFrame) -> Vec<AgentEvent> {
    f.message
        .blocks()
        .filter(|b: &ContentBlock| b.kind == "tool_result")
        .map(|b| {
            tag(
                parent,
                AgentEvent::ToolResult {
                    id: b.tool_use_id.clone(),
                    is_error: b.is_error.unwrap_or(false),
                    output: None,
                    diff: None,
                },
            )
        })
        .collect()
}

/// An id the CLI minted, safe in a path.
fn path_safe(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// CLI-synthesized text that rides a user frame but is NOT conversation:
/// `<system-reminder>` context injections and the interruption marker the CLI
/// stamps into the transcript when a turn (or a subagent) is stopped.
///
/// On a TAGGED frame the distinction is load-bearing, not cosmetic. A tagged
/// user message means "the parent steered its subagent", which announces more
/// work and is therefore the one event allowed to resurrect a settled spawn
/// chip. The CLI emits `[Request interrupted by user]` on the child feed
/// immediately AFTER the subagent's `done{interrupted}` — read as a steer it
/// un-settled a chip that nothing would ever settle again, and the spinner ran
/// forever (2026-08-21: "orchestrator killed them but spinner doesn't stop").
/// It announces the opposite of more work.
///
/// Prefix-matched: the CLI ships at least two spellings of the marker
/// (`…by user]` and `…by user for tool use]`).
fn is_synthetic_user_text(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<system-reminder>") || text.starts_with("[Request interrupted")
}

/// What `--resume <id>` would find in Claude's native history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transcript {
    /// A transcript with conversation in it.
    Conversation,
    /// A transcript read through to its end without a single user or
    /// assistant line. The CLI refuses to resume it ("No conversation
    /// found") and exits.
    Empty,
    /// The history could not be looked at, or holds no file for the session
    /// — the CLI may keep it under another config root (a wrapper setting
    /// its own `CLAUDE_CONFIG_DIR`): leave it to the CLI, whose "No
    /// conversation found" relaunches the run fresh when it truly is gone.
    Unknown,
}

/// A `/skill` typed as the prompt that runs in a fork (`context: fork`). No
/// tool call announces it: its `task_started` has no tool id, and the CLI
/// holds its transcript back, then replays it tagged `forked-command-<name>`
/// after its `task_notification`, "together and in order after the forked
/// skill finishes" (https://code.claude.com/docs/en/headless.md#follow-subagent-messages;
/// live-verified 2.1.310).
struct TypedFork {
    /// Its chip, `forked-command-<name>-<task>`: the CLI reuses its own id
    /// on every run of the command.
    chip: String,
    /// Its task id, which its notification carries.
    task: String,
    /// The prompt its tab opened with, until the replay echoes it.
    echo: Option<String>,
    /// What its notification said, kept for the turn's end.
    status: Option<DoneStatus>,
    /// Its session, whose history holds its transcript while it runs.
    session: String,
    /// Uuids of the records shown from that transcript.
    seen: std::collections::HashSet<String>,
    /// `None` until its replay begins, then whether the replay repeated a
    /// record `seen`.
    replayed: Option<bool>,
}

/// Per-run normalization state.
///
/// `saw_init` dedupes `system:init` — the CLI re-emits it every time the model
/// is re-invoked WITHIN one session (a background-subagent wake turn, a
/// scheduled wakeup), not just at start (live-verified against 2.1.228: a
/// background subagent finishing produces a second init with the SAME session
/// id, then a second `result`). Downstream, `SessionStarted` is the fold's run
/// boundary (it resets accumulated parts), so one run ⇒ one `SessionStarted`;
/// the wake turn's own frames flow through and the engine's parked-session
/// resume turns them into the done→Working→done wake.
pub(crate) struct Normalizer {
    saw_init: bool,
    last_model: Option<String>,
    /// Background-agent ids (`task_started.task_id`) → the spawning Agent
    /// tool_use id. `SendMessage` steers address the AGENT id; this map
    /// re-keys them onto the spawn chip's feed (the wire never echoes the
    /// steer on the child feed — live-verified 2.1.228).
    agent_tasks: std::collections::HashMap<String, String>,
    /// Later tool calls that wake the same task → its original spawn.
    agent_tool_spawns: std::collections::HashMap<String, String>,
    /// Parent-feed `Skill` calls, tool_use id → input, until their task
    /// starts (a `context: fork` skill runs as a subagent under its Skill
    /// call's id, and its `task_started` re-decodes the chip as a spawn) or
    /// their result shows they ran inline.
    skill_calls: std::collections::HashMap<String, Value>,
    /// tool_use ids of Agent/Task spawn calls, recorded from their own
    /// assistant frames (plus `task_started`'s agent-task pairing). Gates
    /// `task_notification`: background SHELL tasks settle through the same
    /// subtype carrying their Bash call's id, and tagging that Done as
    /// subagent traffic stamped a spawn ref onto an ordinary Run chip —
    /// which then opened as an empty, never-created subagent doc (user
    /// report 2026-08-20).
    agent_spawn_tools: std::collections::HashSet<String>,
    /// The typed fork of the turn under way (a turn runs one at most). Its
    /// replay feeds its chip only until the turn ends: a `forked-command-*`
    /// id after that stays the CLI's own, which no chip carries.
    typed_fork: Option<TypedFork>,
    /// Typed forks' transcripts are read off disk (see `fork_tail`) until a
    /// replay repeats none of the records read.
    fork_transcripts: bool,
    /// Rotates at each assistant-frame close and at each steer; SessionStarted
    /// carries the first value so folds can attribute deltas from the start.
    assistant_message_id: String,
    /// Last session id seen (init or result) — used for synthetic Dones.
    pub session_id: Option<String>,
    /// This turn already showed its failure as an `Error` part (an
    /// assistant-level error code, a blocking rate limit): the result's own
    /// wording of the same failure is not repeated.
    turn_error_shown: bool,
    /// This turn already said it is retrying the API.
    retry_noticed: bool,
}

impl Normalizer {
    pub fn new() -> Self {
        Self {
            saw_init: false,
            last_model: None,
            agent_tasks: std::collections::HashMap::new(),
            agent_tool_spawns: std::collections::HashMap::new(),
            skill_calls: std::collections::HashMap::new(),
            agent_spawn_tools: std::collections::HashSet::new(),
            typed_fork: None,
            fork_transcripts: true,
            assistant_message_id: new_message_id(),
            session_id: None,
            turn_error_shown: false,
            retry_noticed: false,
        }
    }

    /// The CLI reported its session (`system:init`).
    pub fn started(&self) -> bool {
        self.saw_init
    }

    /// Restore only spawn identity from Claude's native history. The CLI
    /// does not replay task_started when --resume starts a new process.
    /// Session ids are global; search project directories rather than
    /// duplicating the CLI's cwd encoding (including long-path hashing).
    /// Long sessions' histories run to hundreds of MB: the scan runs off the
    /// runtime and only parses lines that can name a spawn or its agent.
    /// The same scan says whether there is a conversation to resume at all.
    pub async fn for_resume(config_root: &std::path::Path, session_id: &str) -> (Self, Transcript) {
        let config_root = config_root.to_owned();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || Self::restore_spawns(&config_root, &session_id))
            .await
            .unwrap_or_else(|_| (Self::new(), Transcript::Unknown))
    }

    fn restore_spawns(config_root: &std::path::Path, session_id: &str) -> (Self, Transcript) {
        use std::io::BufRead as _;
        let mut norm = Self::new();
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return (norm, Transcript::Unknown);
        }
        let Ok(projects) = std::fs::read_dir(config_root.join("projects")) else {
            return (norm, Transcript::Unknown);
        };
        let Some(file) = projects.flatten().find_map(|project| {
            std::fs::File::open(project.path().join(format!("{session_id}.jsonl"))).ok()
        }) else {
            return (norm, Transcript::Unknown);
        };
        let mut transcript = Transcript::Empty;
        // Only a transcript read whole, every line understood, is Empty: a
        // read error or a line that might have been conversation leaves the
        // call to the CLI.
        let mut doubtful = false;
        let mut reader = std::io::BufReader::with_capacity(1 << 16, file);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Err(_) => {
                    doubtful = true;
                    break;
                }
                Ok(_) => {}
            }
            // Torn lines and older/newer unknown shapes are harmless.
            let Ok(text) = std::str::from_utf8(&line) else {
                doubtful = true;
                continue;
            };
            // Most of a transcript is bookkeeping (queue operations, request
            // records, attachments); one user or assistant record is enough.
            if transcript == Transcript::Empty
                && (text.contains("\"type\":\"user\"") || text.contains("\"type\":\"assistant\""))
            {
                match serde_json::from_str::<Value>(text) {
                    Ok(record) => {
                        if matches!(
                            record.get("type").and_then(Value::as_str),
                            Some("user" | "assistant")
                        ) {
                            transcript = Transcript::Conversation;
                        }
                    }
                    Err(_) => doubtful = true,
                }
            }
            if !(text.contains("\"Agent\"")
                || text.contains("\"Task\"")
                || text.contains("agentId"))
            {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            // A subagent's own nested spawns are not chips on this chat.
            if record.get("isSidechain").and_then(Value::as_bool) == Some(true)
                || !record.get("parent_tool_use_id").is_none_or(Value::is_null)
            {
                continue;
            }
            let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
                continue;
            };
            match record.get("type").and_then(Value::as_str) {
                Some("assistant") => {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) == Some("tool_use")
                            && matches!(
                                block.get("name").and_then(Value::as_str),
                                Some("Agent" | "Task")
                            )
                            && let Some(id) = block
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                        {
                            norm.agent_spawn_tools.insert(id.to_owned());
                        }
                    }
                }
                Some("user") => {
                    if let Some(agent) = record
                        .pointer("/toolUseResult/agentId")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        && let Some(spawn) = blocks
                            .iter()
                            .filter(|b| {
                                b.get("type").and_then(Value::as_str) == Some("tool_result")
                            })
                            .filter_map(|b| b.get("tool_use_id").and_then(Value::as_str))
                            .find(|id| norm.agent_spawn_tools.contains(*id))
                    {
                        norm.agent_tasks
                            .entry(agent.to_owned())
                            .or_insert_with(|| spawn.to_owned());
                    }
                }
                _ => {}
            }
        }
        if doubtful && transcript == Transcript::Empty {
            transcript = Transcript::Unknown;
        }
        (norm, transcript)
    }

    /// Rotate the assistant message id for a steer boundary; returns
    /// (previous, next) for the `Steered` event.
    pub fn rotate_for_steer(&mut self) -> (String, String) {
        let prev = std::mem::replace(&mut self.assistant_message_id, new_message_id());
        (prev, self.assistant_message_id.clone())
    }

    /// The transcript a tagged frame feeds: the typed fork's chip, for any
    /// `forked-command-*` id while it runs (the name after the prefix "can
    /// differ from the one you typed"); the spawn a later call woke; else the
    /// id itself.
    fn route<'a>(&'a self, parent: &'a str) -> &'a str {
        match &self.typed_fork {
            Some(fork) if parent.starts_with("forked-command-") => &fork.chip,
            _ => self
                .agent_tool_spawns
                .get(parent)
                .map_or(parent, String::as_str),
        }
    }

    /// The turn is ending: its typed fork settles with the status its
    /// notification brought, else the turn's, once (a late notification
    /// finds no spawn). No tool result ever answers the chip, so it is
    /// resolved here too.
    pub(crate) fn settle_typed_fork(&mut self, turn: DoneStatus) -> Vec<AgentEvent> {
        let Some(fork) = self.typed_fork.take() else {
            return Vec::new();
        };
        self.agent_tasks.remove(&fork.task);
        self.agent_spawn_tools.remove(&fork.chip);
        // Nothing promises the replay reuses the file's uuids: when it
        // repeated none, every record showed twice, so this run's later
        // forks wait for their replay.
        if !fork.seen.is_empty() && fork.replayed == Some(false) {
            tracing::warn!(
                target: "zeron_harness::claude",
                chip = %fork.chip,
                "the typed fork's replay repeated no record read off disk; no longer reading forks' transcripts"
            );
            self.fork_transcripts = false;
        }
        let status = fork.status.unwrap_or(turn);
        vec![
            tag(
                &fork.chip,
                AgentEvent::Done {
                    status,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ),
            AgentEvent::ToolResult {
                id: fork.chip,
                is_error: status == DoneStatus::Errored,
                output: None,
                diff: None,
            },
        ]
    }

    /// The typed fork whose transcript the driver reads off disk, as
    /// `(chip, session, task)`, until its replay begins.
    pub(crate) fn fork_to_follow(&self) -> Option<(&str, &str, &str)> {
        self.typed_fork
            .as_ref()
            .filter(|fork| {
                self.fork_transcripts
                    && fork.replayed.is_none()
                    && path_safe(&fork.session)
                    && path_safe(&fork.task)
            })
            .map(|fork| {
                (
                    fork.chip.as_str(),
                    fork.session.as_str(),
                    fork.task.as_str(),
                )
            })
    }

    /// One record of the typed fork's transcript on disk: its messages and
    /// tool results, as its replay shows them. Its prompt (the tab opened
    /// with it) and the bookkeeping around them (attachments, API request
    /// records) are not conversation. Once the replay has begun, it brings
    /// the rest.
    pub(crate) fn fork_record(&mut self, record: Value) -> Vec<AgentEvent> {
        let Some(fork) = self
            .typed_fork
            .as_mut()
            .filter(|fork| fork.replayed.is_none())
        else {
            return Vec::new();
        };
        let assistant = match record.get("type").and_then(Value::as_str) {
            Some("assistant") => true,
            Some("user") => false,
            _ => return Vec::new(),
        };
        let Ok(f) = serde_json::from_value::<MessageFrame>(record) else {
            return Vec::new();
        };
        let events = if assistant {
            subagent_message(&fork.chip, &f)
        } else {
            subagent_results(&fork.chip, &f)
        };
        if !events.is_empty()
            && let Some(uuid) = f.uuid
        {
            fork.seen.insert(uuid);
        }
        events
    }

    /// A frame of the typed fork's replay, which brings the rest of its
    /// transcript. A record already read off disk shows nothing: the replay
    /// repeats each one under its uuid (live-verified 2.1.310).
    fn fork_repeat(&mut self, parent: &str, uuid: Option<&str>) -> bool {
        let Some(fork) = self.typed_fork.as_mut().filter(|fork| fork.chip == parent) else {
            return false;
        };
        let repeat = uuid.is_some_and(|uuid| fork.seen.contains(uuid));
        fork.replayed = Some(fork.replayed == Some(true) || repeat);
        repeat
    }

    /// Normalize one stdout frame into 0+ unified events. `interrupted` folds
    /// a post-interrupt `result` into `Done { status: Interrupted }`.
    pub fn normalize(&mut self, frame: Frame, interrupted: bool) -> Vec<AgentEvent> {
        match frame {
            Frame::System(f) => {
                // A background subagent's completion arrives as an UNTAGGED
                // `task_notification` carrying the spawning tool's id — the
                // wire's only terminal signal for it (live-verified 2.1.228:
                // no tagged frame follows; the subagent's stream just stops).
                // Surface it as the subagent's tagged Done so the chip flips
                // done/failed and the transcript freezes.
                if f.subtype == "task_notification" {
                    // A resumed task's notification may carry SendMessage's
                    // id, or omit the tool id entirely. Its task id is stable.
                    let parent = f
                        .task_id
                        .as_deref()
                        .and_then(|task| self.agent_tasks.get(task).map(String::as_str))
                        .or_else(|| f.tool_use_id.as_deref().filter(|t| !t.is_empty()));
                    let Some(parent) = parent else {
                        return Vec::new();
                    };
                    // Only a KNOWN spawn settles as a subagent. Background
                    // SHELL tasks (`Bash` with `run_in_background`) settle
                    // through this same subtype carrying the Bash call's id —
                    // tagging that Done would bind a subagent ref onto an
                    // ordinary Run chip. Spawn ids are learned from live
                    // calls/task starts or restored from the native history.
                    if !self.agent_spawn_tools.contains(parent) {
                        return Vec::new();
                    }
                    let status = match f.status.as_deref().unwrap_or("") {
                        "completed" | "complete" | "succeeded" | "success" => DoneStatus::Completed,
                        "failed" | "errored" | "error" => DoneStatus::Errored,
                        "killed" | "cancelled" | "canceled" | "stopped" | "interrupted" => {
                            DoneStatus::Interrupted
                        }
                        // Non-terminal notification shapes: nothing to close.
                        _ => return Vec::new(),
                    };
                    // A typed fork's transcript is still to come: settled
                    // now, its tab would drop the replay as stragglers.
                    if let Some(fork) = self.typed_fork.as_mut().filter(|fork| fork.chip == parent)
                    {
                        fork.status = Some(status);
                        return Vec::new();
                    }
                    return vec![tag(
                        parent,
                        AgentEvent::Done {
                            status,
                            result: None,
                            error: None,
                            session_id: None,
                        },
                    )];
                }
                // An AGENT task starting (subagent_type present — subagent-
                // owned shell tasks carry the same subtype without it):
                // record agentId → spawn id for SendMessage steer re-keying.
                if f.subtype == "task_started"
                    && f.subagent_type.is_some()
                    && let (Some(task), Some(tool)) = (
                        f.task_id.as_deref().filter(|t| !t.is_empty()),
                        f.tool_use_id.as_deref().filter(|t| !t.is_empty()),
                    )
                {
                    let spawn = self
                        .agent_tasks
                        .entry(task.to_owned())
                        .or_insert_with(|| tool.to_owned());
                    self.agent_spawn_tools.insert(spawn.clone());
                    self.agent_tool_spawns
                        .insert(tool.to_owned(), spawn.clone());
                    // A `context: fork` skill runs as a subagent under its
                    // Skill call's id, and this start precedes all of its
                    // traffic (live-verified 2.1.310). Re-decoded as a spawn,
                    // the chip gives that traffic a card and a tab to feed.
                    return self
                        .skill_calls
                        .remove(tool)
                        .map(|input| AgentEvent::ToolCall {
                            id: tool.to_owned(),
                            call: forked_skill_call(input),
                        })
                        .into_iter()
                        .collect();
                }
                // A `/skill` typed as the prompt starts its fork with no tool
                // id, named after the command (see [`TypedFork`]). Its chip,
                // opened with the skill's prompt, gives the fork a row and a
                // tab while the CLI holds its transcript back.
                if f.subtype == "task_started"
                    && f.subagent_type.is_some()
                    && f.tool_use_id.as_deref().is_none_or(str::is_empty)
                    && let Some(task) = f.task_id.as_deref().filter(|t| !t.is_empty())
                    && let Some(name) = f
                        .description
                        .as_deref()
                        .and_then(|d| d.split_whitespace().next())
                        .and_then(|command| command.strip_prefix('/'))
                        .filter(|name| !name.is_empty())
                {
                    // A turn runs one at most. Should another start before
                    // the first settled, the first ends here, not run on as
                    // a chip nothing settles.
                    let mut out = self.settle_typed_fork(DoneStatus::Interrupted);
                    let chip = format!("forked-command-{name}-{task}");
                    self.agent_tasks.insert(task.to_owned(), chip.clone());
                    self.agent_spawn_tools.insert(chip.clone());
                    let prompt = f
                        .prompt
                        .as_ref()
                        .and_then(Value::as_str)
                        .filter(|p| !p.trim().is_empty());
                    out.push(AgentEvent::ToolCall {
                        id: chip.clone(),
                        call: forked_skill_call(serde_json::json!({ "skill": name })),
                    });
                    out.extend(prompt.map(|text| {
                        tag(
                            &chip,
                            AgentEvent::UserMessage {
                                text: text.to_owned(),
                            },
                        )
                    }));
                    self.typed_fork = Some(TypedFork {
                        chip,
                        task: task.to_owned(),
                        echo: prompt.map(str::to_owned),
                        status: None,
                        session: f.session_id.clone(),
                        seen: std::collections::HashSet::new(),
                        replayed: None,
                    });
                    return out;
                }
                // The CLI retrying a failed API call waits between attempts
                // with nothing on the wire: keep the turn visibly alive, and
                // once the retries add up, say why it is slow — once.
                if f.subtype == "api_retry" {
                    let attempt = f.attempt.unwrap_or(1);
                    tracing::info!(
                        target: "zeron_harness::claude",
                        attempt,
                        max = ?f.max_retries,
                        status = ?f.error_status,
                        delay_ms = ?f.retry_delay_ms,
                        "Claude API retry"
                    );
                    let mut out = vec![AgentEvent::ReasoningDelta {
                        text: String::new(),
                    }];
                    if attempt >= 3 && !self.retry_noticed {
                        self.retry_noticed = true;
                        let why = match (f.error_status, f.error.as_ref().and_then(Value::as_str)) {
                            (Some(429), _) | (_, Some("rate_limit")) => {
                                "is rate limited".to_owned()
                            }
                            (Some(529), _) | (_, Some("overloaded")) => "is overloaded".to_owned(),
                            (Some(status), _) => format!("returned an error ({status})"),
                            (None, _) => "could not be reached".to_owned(),
                        };
                        let of = f
                            .max_retries
                            .map(|m| format!(" of {m}"))
                            .unwrap_or_default();
                        out.push(AgentEvent::Error {
                            message: format!(
                                "The Claude API {why} — still retrying (attempt {attempt}{of})."
                            ),
                        });
                    }
                    return out;
                }
                // Compaction runs a whole model call with nothing streamed.
                if f.subtype == "status" && f.status.as_deref() == Some("compacting") {
                    return vec![AgentEvent::ReasoningDelta {
                        text: String::new(),
                    }];
                }
                // The context shrank: report the new occupancy now rather
                // than at the next assistant frame.
                if f.subtype == "compact_boundary" {
                    return f
                        .compact_metadata
                        .as_ref()
                        .and_then(|m| m.get("post_tokens"))
                        .and_then(Value::as_u64)
                        .map(|tokens| AgentEvent::ContextUsage {
                            tokens: Some(tokens),
                            window: None,
                        })
                        .into_iter()
                        .collect();
                }
                if f.subtype != "init" || self.saw_init {
                    return Vec::new();
                }
                self.saw_init = true;
                self.session_id = Some(f.session_id.clone());
                vec![AgentEvent::SessionStarted {
                    harness: HarnessId::ClaudeCode,
                    model: f.model,
                    tools: f.tools,
                    cwd: f.cwd,
                    session_id: f.session_id,
                    assistant_message_id: self.assistant_message_id.clone(),
                }]
            }

            // Frames with `parent_tool_use_id` set belong to a SUBAGENT's
            // nested transcript: a background subagent runs concurrently with
            // the parent's own stream, so folding them into the parent feed
            // would split a contiguous text block around phantom tool calls
            // (and a subagent's message boundaries must never rotate the
            // parent's assistant message id). They are wrapped in
            // `AgentEvent::Subagent` instead — the engine routes them to the
            // subagent's own doc.
            Frame::StreamEvent(f) => {
                if f.event.kind != "content_block_delta" {
                    return Vec::new();
                }
                if let Some(parent) = &f.parent_tool_use_id {
                    let parent = self.route(parent);
                    return match f.event.delta.kind.as_str() {
                        "text_delta" => vec![tag(
                            parent,
                            AgentEvent::TextDelta {
                                text: f.event.delta.text,
                            },
                        )],
                        "thinking_delta" if !f.event.delta.thinking.is_empty() => vec![tag(
                            parent,
                            AgentEvent::ReasoningDelta {
                                text: f.event.delta.thinking,
                            },
                        )],
                        // Subagent liveness heartbeats have no consumer; the
                        // parent turn is already done (eager-done policy).
                        _ => Vec::new(),
                    };
                }
                match f.event.delta.kind.as_str() {
                    "text_delta" => vec![AgentEvent::TextDelta {
                        text: f.event.delta.text,
                    }],
                    "thinking_delta" => vec![AgentEvent::ReasoningDelta {
                        text: f.event.delta.thinking,
                    }],
                    // A big tool input (a 90-line Write) streams as a long run
                    // of input_json_delta frames with nothing else — minutes of
                    // apparent silence that reads as a stalled run. Surface
                    // them as empty reasoning deltas: the engine treats those
                    // as pure liveness heartbeats (never journaled/rendered).
                    "input_json_delta" => vec![AgentEvent::ReasoningDelta {
                        text: String::new(),
                    }],
                    _ => Vec::new(),
                }
            }

            Frame::Assistant(f) => {
                if let Some(parent) = &f.parent_tool_use_id {
                    let parent = &self.route(parent).to_owned();
                    if self.fork_repeat(parent, f.uuid.as_deref()) {
                        return Vec::new();
                    }
                    return subagent_message(parent, &f);
                }
                // Record spawn tool ids up front: `task_notification` keys on
                // them; a task may finish before a `task_started` arrives. A
                // Skill call waits for its task start: only a forked skill
                // gets one.
                for b in f.message.blocks() {
                    if b.kind != "tool_use" {
                        continue;
                    }
                    match b.name.as_str() {
                        "Agent" | "Task" => {
                            self.agent_spawn_tools.insert(b.id.clone());
                        }
                        "Skill" => {
                            self.skill_calls.insert(b.id.clone(), b.input.clone());
                        }
                        _ => {}
                    }
                }
                let mut out: Vec<AgentEvent> = f
                    .message
                    .blocks()
                    .filter(|b: &ContentBlock| b.kind == "tool_use")
                    .flat_map(|b| {
                        let call = AgentEvent::ToolCall {
                            id: b.id.clone(),
                            call: decode_tool_use(&b.name, &b.input),
                        };
                        // A spawn's `prompt` is the subagent's opening user
                        // message — the wire never echoes it on the child
                        // feed (child user frames carry tool results and
                        // steers only), so seed it here and the subagent
                        // transcript starts the way every chat does.
                        let opening = matches!(b.name.as_str(), "Agent" | "Task")
                            .then(|| b.input.get("prompt"))
                            .flatten()
                            .and_then(Value::as_str)
                            .filter(|p| !p.trim().is_empty())
                            .map(|prompt| {
                                tag(
                                    &b.id,
                                    AgentEvent::UserMessage {
                                        text: prompt.to_owned(),
                                    },
                                )
                            });
                        // A SendMessage steer never echoes on the child feed
                        // (live-verified) — surface it from the parent's own
                        // call, re-keyed onto the spawn it addresses.
                        let steer = (b.name == "SendMessage")
                            .then(|| {
                                let to = ["to", "recipient"]
                                    .iter()
                                    .find_map(|k| b.input.get(*k))
                                    .and_then(Value::as_str)?;
                                let spawn = self.agent_tasks.get(to)?;
                                let text = ["message", "content"]
                                    .iter()
                                    .find_map(|k| b.input.get(*k))
                                    .and_then(Value::as_str)
                                    .filter(|m| !m.trim().is_empty())?;
                                Some(tag(
                                    spawn,
                                    AgentEvent::UserMessage {
                                        text: text.to_owned(),
                                    },
                                ))
                            })
                            .flatten();
                        std::iter::once(call).chain(opening).chain(steer)
                    })
                    .collect();
                self.last_model = f.message.model.clone().or(self.last_model.take());
                if let Some(usage) = &f.message.usage {
                    let fields = [
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                    ];
                    if fields
                        .iter()
                        .any(|key| usage.get(*key).and_then(Value::as_u64).is_some())
                    {
                        let tokens = fields
                            .iter()
                            .filter_map(|key| usage.get(*key).and_then(Value::as_u64))
                            .fold(0u64, u64::saturating_add);
                        out.push(AgentEvent::ContextUsage {
                            tokens: Some(tokens),
                            window: None,
                        });
                    }
                }
                // A failed turn (usage limit, billing, auth, overloaded, …)
                // carries a terse `error` code here — often with empty content
                // and no `result` error — so surface it visibly.
                if let Some(code) = &f.error {
                    self.turn_error_shown = true;
                    out.push(AgentEvent::Error {
                        message: assistant_error_text(code),
                    });
                }
                // The enclosing assistant frame closes the streamed message
                // item; rotate so post-boundary deltas get a fresh id.
                let (prev, _next) = self.rotate_for_steer();
                out.push(AgentEvent::AssistantMessageCompleted {
                    assistant_message_id: prev,
                });
                out
            }

            Frame::User(f) => {
                if let Some(parent) = &f.parent_tool_use_id {
                    let parent = &self.route(parent).to_owned();
                    if self.fork_repeat(parent, f.uuid.as_deref()) {
                        return Vec::new();
                    }
                    // A subagent's tool results echo on the main channel too;
                    // they belong to its transcript, attributed like its calls.
                    let mut out = subagent_results(parent, &f);
                    // A tagged user frame's TEXT blocks are the parent
                    // steering its subagent (SendMessage-style follow-ups —
                    // tool results ride their own blocks, filtered above).
                    // Synthetic harness injections are not conversation, and
                    // must not read as a steer — see [`is_synthetic_user_text`].
                    // Nor is a typed fork's replay of the prompt its tab
                    // opened with, dropped once.
                    let mut echo = self
                        .typed_fork
                        .as_mut()
                        .filter(|fork| fork.chip == *parent)
                        .map(|fork| &mut fork.echo);
                    out.extend(
                        f.message
                            .blocks()
                            .filter(|b: &ContentBlock| {
                                b.kind == "text"
                                    && !b.text.trim().is_empty()
                                    && !is_synthetic_user_text(&b.text)
                                    && echo
                                        .as_mut()
                                        .and_then(|echo| {
                                            echo.take_if(|p| p.trim() == b.text.trim())
                                        })
                                        .is_none()
                            })
                            .map(|b| tag(parent, AgentEvent::UserMessage { text: b.text })),
                    );
                    return out;
                }
                f.message
                    .blocks()
                    .filter(|b: &ContentBlock| b.kind == "tool_result")
                    .map(|b| {
                        // A skill whose result came back without a task
                        // start ran inline: it never forks now.
                        self.skill_calls.remove(&b.tool_use_id);
                        AgentEvent::ToolResult {
                            id: b.tool_use_id.clone(),
                            is_error: b.is_error.unwrap_or(false),
                            output: None,
                            diff: None,
                        }
                    })
                    .collect()
            }

            // A claude.ai plan window was hit. A hard `rejected` blocks the
            // turn — make it visible; allowed/allowed_warning stay quiet, and
            // so does a reject the account's overage carries past.
            Frame::RateLimit(f) => {
                if !f.rate_limit_info.blocks() {
                    return Vec::new();
                }
                let window =
                    rate_window_label(f.rate_limit_info.rate_limit_type.as_deref().unwrap_or(""));
                self.turn_error_shown = true;
                vec![AgentEvent::Error {
                    message: format!(
                        "Claude {window} limit reached — the turn was blocked. Try again after it resets."
                    ),
                }]
            }

            Frame::Result(f) => {
                let error_shown = std::mem::take(&mut self.turn_error_shown);
                self.retry_noticed = false;
                // A result line that would not parse (see `parse_frame`):
                // its turn still ends, with what little the line told us.
                if f.unreadable {
                    tracing::warn!(target: "zeron_harness::claude", "unreadable result frame");
                    let failed = f.is_error && !interrupted;
                    let status = if interrupted {
                        DoneStatus::Interrupted
                    } else if failed {
                        DoneStatus::Errored
                    } else {
                        DoneStatus::Completed
                    };
                    let mut out = self.settle_typed_fork(status);
                    out.push(AgentEvent::Done {
                        status,
                        result: None,
                        error: (failed && !error_shown)
                            .then(|| "Claude's result for this turn could not be read.".to_owned()),
                        session_id: self.session_id.clone(),
                    });
                    return out;
                }
                let model_usage = self
                    .last_model
                    .as_ref()
                    .and_then(|model| {
                        f.model_usage.get(model).or_else(|| {
                            f.model_usage.values().find(|entry| {
                                entry.get("canonicalModel").and_then(Value::as_str)
                                    == Some(model.as_str())
                            })
                        })
                    })
                    .or_else(|| {
                        (f.model_usage.len() == 1)
                            .then(|| f.model_usage.values().next())
                            .flatten()
                    });
                let window = model_usage
                    .and_then(|entry| entry.get("contextWindow"))
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0);
                if let Some(id) = &f.session_id {
                    self.session_id = Some(id.clone());
                }
                let usage = AgentEvent::Usage {
                    input_tokens: f.usage.input_tokens,
                    output_tokens: f.usage.output_tokens,
                };
                let failure = (f.subtype == "success" && !interrupted && !aborted(&f))
                    .then(|| api_failure(&f))
                    .flatten();
                let done = if let Some(error) = failure {
                    AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: (!error_shown).then_some(error),
                        session_id: f.session_id,
                    }
                } else if f.subtype == "success" {
                    AgentEvent::Done {
                        status: if interrupted || aborted(&f) {
                            DoneStatus::Interrupted
                        } else {
                            DoneStatus::Completed
                        },
                        result: f.result,
                        error: None,
                        session_id: f.session_id,
                    }
                } else {
                    // Split the CLI's internal `[ede_diagnostic]` breadcrumbs
                    // off the real errors: diagnostics are debug-logged, never
                    // surfaced as transcript error parts.
                    let (diagnostics, errors): (Vec<String>, Vec<String>) = f
                        .errors
                        .iter()
                        .map(|e| match e {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .partition(|m| is_internal_diagnostic(m));
                    for diagnostic in &diagnostics {
                        tracing::debug!(
                            target: "zeron_harness::claude",
                            "internal CLI diagnostic (not surfaced): {diagnostic}"
                        );
                    }
                    let error = if !errors.is_empty() {
                        // Real user-relevant errors — surface verbatim.
                        Some(errors.join("; "))
                    } else {
                        match f.subtype.as_str() {
                            // Known run-failure subtypes stay visible with
                            // their mapped human wording (never blank — a
                            // blank error folds to no part and the failed
                            // turn reads as a silent non-reply).
                            "error_max_turns"
                            | "error_max_budget_usd"
                            | "error_max_structured_output_retries" => {
                                Some(result_error_text(&f.subtype).to_owned())
                            }
                            // Diagnostic-only ends (the CLI's turn-accounting
                            // telemetry, typically `error_during_execution`
                            // after an abort): nothing user-relevant to show.
                            _ if !diagnostics.is_empty() => None,
                            // The failure already has its part; a generic
                            // line under it adds nothing.
                            _ if error_shown => None,
                            _ => Some(result_error_text(&f.subtype).to_owned()),
                        }
                    };
                    let interrupted = interrupted || aborted(&f);
                    AgentEvent::Done {
                        status: if interrupted {
                            DoneStatus::Interrupted
                        } else {
                            DoneStatus::Errored
                        },
                        result: None,
                        error,
                        session_id: f.session_id,
                    }
                };
                let mut out = Vec::new();
                if let Some(window) = window {
                    out.push(AgentEvent::ContextUsage {
                        tokens: None,
                        window: Some(window),
                    });
                }
                out.push(usage);
                if let AgentEvent::Done { status, .. } = &done {
                    out.extend(self.settle_typed_fork(*status));
                }
                out.push(done);
                out
            }

            // Control frames are handled by the run loop, not normalized.
            Frame::ControlRequest(_)
            | Frame::ControlResponse(_)
            | Frame::CommandLifecycle(_)
            | Frame::Other => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_typed_tools() {
        assert_eq!(
            decode_tool_use("Bash", &json!({"command": "ls -la"})),
            ToolCall::Exec {
                command: "ls -la".into()
            }
        );
        assert_eq!(
            decode_tool_use(
                "Edit",
                &json!({"file_path": "/a", "old_string": "x", "new_string": "y"})
            ),
            ToolCall::EditFile {
                path: "/a".into(),
                old_string: Some("x".into()),
                new_string: Some("y".into())
            }
        );
        assert_eq!(
            decode_tool_use(
                "TodoWrite",
                &json!({"todos": [{"content": "t", "status": "completed"}]})
            ),
            ToolCall::Todo {
                items: vec![TodoItem::new("t", TodoStatus::Completed)]
            }
        );
        // Claude's in-progress state survives; unknown/missing reads as pending.
        assert_eq!(
            decode_tool_use(
                "TodoWrite",
                &json!({"todos": [
                    {"content": "a", "status": "completed"},
                    {"content": "b", "status": "in_progress", "activeForm": "Doing b"},
                    {"content": "c", "status": "pending"},
                    {"content": "d"},
                ]})
            ),
            ToolCall::Todo {
                items: vec![
                    TodoItem::new("a", TodoStatus::Completed),
                    TodoItem::new("b", TodoStatus::InProgress),
                    TodoItem::new("c", TodoStatus::Pending),
                    TodoItem::new("d", TodoStatus::Pending),
                ]
            }
        );
        assert_eq!(
            decode_tool_use("mcp__linear__search", &json!({"q": "bug"})),
            ToolCall::Mcp {
                server: "linear".into(),
                tool: "search".into(),
                input: Some(json!({"q": "bug"}))
            }
        );
        assert!(matches!(
            decode_tool_use("Mystery", &json!({})),
            ToolCall::Unknown { .. }
        ));
    }

    fn normalize_one(raw: &str) -> Vec<AgentEvent> {
        let frame = crate::claude::wire::parse_frame(raw).expect("frame parses");
        Normalizer::new().normalize(frame, false)
    }

    #[test]
    fn a_rejected_window_carried_by_overage_is_not_an_error() {
        let blocked = normalize_one(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","overageStatus":"rejected"}}"#,
        );
        assert!(matches!(blocked.as_slice(), [AgentEvent::Error { .. }]));
        for overage in [
            r#""overageStatus":"allowed""#,
            r#""overageStatus":"allowed_warning""#,
            r#""isUsingOverage":true"#,
        ] {
            let raw = format!(
                r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"five_hour",{overage}}}}}"#
            );
            assert!(normalize_one(&raw).is_empty(), "{overage}");
        }
    }

    fn result_done(raw: &str) -> AgentEvent {
        let events = normalize_one(raw);
        assert_eq!(events.len(), 2, "usage + done");
        events.into_iter().nth(1).expect("done event")
    }

    #[test]
    fn stream_deltas_map_to_text_reasoning_and_heartbeats() {
        // Real thinking text streams as a reasoning delta.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
        );
        assert_eq!(ev, vec![AgentEvent::ReasoningDelta { text: "hmm".into() }]);
        // Redacted thinking (estimated_tokens only) yields the empty
        // heartbeat shape the engine filters.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"","estimated_tokens":50}}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ReasoningDelta {
                text: String::new()
            }]
        );
        // A tool input being generated (input_json_delta) is a liveness
        // heartbeat, not silence — minutes of a big Write must not read as
        // a stalled run.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"file_"}}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ReasoningDelta {
                text: String::new()
            }]
        );
        // Signature deltas stay dropped.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"abc"}}}"#,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn subagent_stream_deltas_arrive_tagged() {
        let ev = normalize_one(
            r#"{"type":"stream_event","parent_tool_use_id":"toolu_sub","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"sub text"}}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::TextDelta {
                    text: "sub text".into()
                }),
            }]
        );
        // Subagent input_json heartbeats are dropped — the parent turn is
        // already done under the eager-done policy, nothing to keep alive.
        let ev = normalize_one(
            r#"{"type":"stream_event","parent_tool_use_id":"toolu_sub","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}}"#,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn subagent_tool_calls_and_results_arrive_tagged_without_boundary_rotation() {
        let mut norm = Normalizer::new();
        let before = norm.assistant_message_id.clone();
        let frame = crate::claude::wire::parse_frame(
            r#"{"type":"assistant","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
        )
        .expect("parses");
        let ev = norm.normalize(frame, false);
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::ToolCall {
                    id: "t1".into(),
                    call: ToolCall::Exec {
                        command: "ls".into()
                    },
                }),
            }]
        );
        // A subagent's assistant frame must NOT rotate the parent's message
        // id (it would split the parent's contiguous text mid-stream).
        assert_eq!(norm.assistant_message_id, before);

        let frame = crate::claude::wire::parse_frame(
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#,
        )
        .expect("parses");
        let ev = norm.normalize(frame, false);
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::ToolResult {
                    id: "t1".into(),
                    is_error: false,
                    output: None,
                    diff: None,
                }),
            }]
        );
    }

    #[test]
    fn spawn_prompt_seeds_the_subagent_opening_user_message() {
        // The wire never echoes a Task's prompt on the child feed, so the
        // spawn itself seeds the subagent's opening user entry.
        let ev = normalize_one(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_sub","name":"Task","input":{"description":"probe","prompt":"scan the fold path"}}]}}"#,
        );
        assert!(matches!(
            &ev[..],
            [
                AgentEvent::ToolCall { id, .. },
                AgentEvent::Subagent { parent_tool_use_id, event },
                AgentEvent::AssistantMessageCompleted { .. },
            ] if id == "toolu_sub"
                && parent_tool_use_id == "toolu_sub"
                && matches!(event.as_ref(), AgentEvent::UserMessage { text } if text == "scan the fold path")
        ));
        // No prompt → no synthetic opening; ordinary tools never spawn one.
        for frame in [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Task","input":{"description":"probe"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"ls","prompt":"red herring"}}]}}"#,
        ] {
            let ev = normalize_one(frame);
            assert!(
                !ev.iter().any(|e| matches!(e, AgentEvent::Subagent { .. })),
                "{frame}: {ev:?}"
            );
        }
    }

    /// Killing a subagent puts `done{interrupted}` on the child feed and then
    /// an interruption MARKER as a tagged user frame. Read as a steer, that
    /// marker resurrects the spawn chip the `done` just settled — and nothing
    /// ever settles it again, so the chip spins forever. It is CLI
    /// bookkeeping, filtered like a `<system-reminder>`; a real steer on the
    /// same frame shape still gets through.
    #[test]
    fn the_interruption_marker_is_not_a_steer() {
        for marker in [
            "[Request interrupted by user]",
            "[Request interrupted by user for tool use]",
        ] {
            let frame = format!(
                r#"{{"type":"user","parent_tool_use_id":"toolu_spawn","message":{{"content":[{{"type":"text","text":"{marker}"}}]}}}}"#
            );
            assert!(
                !normalize_one(&frame)
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Subagent { .. })),
                "{marker} leaked as a steer"
            );
        }
        // A genuine steer on the very same frame shape still arrives.
        let real = r#"{"type":"user","parent_tool_use_id":"toolu_spawn","message":{"content":[{"type":"text","text":"Keep going."}]}}"#;
        assert!(
            normalize_one(real).iter().any(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if parent_tool_use_id == "toolu_spawn"
                        && matches!(event.as_ref(), AgentEvent::UserMessage { text } if text == "Keep going.")
            )),
            "a real steer must still reach the subagent"
        );
    }

    #[test]
    fn send_message_steers_rekey_onto_the_spawn_feed() {
        // Live 2.1.228: the steer NEVER echoes on the child feed; the only
        // wire evidence is the parent's SendMessage call addressed to the
        // agent id that task_started paired with the spawn tool id.
        let mut norm = Normalizer::new();
        let started = crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_started","task_id":"a20b2336","tool_use_id":"toolu_spawn","subagent_type":"general-purpose","prompt":"p","description":"d"}"#,
        )
        .expect("parses");
        assert!(norm.normalize(started, false).is_empty());
        let send = crate::claude::wire::parse_frame(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send","name":"SendMessage","input":{"to":"a20b2336","message":"Also read the rebuild.","summary":"s"}}]}}"#,
        )
        .expect("parses");
        let ev = norm.normalize(send, false);
        assert!(
            ev.iter().any(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if parent_tool_use_id == "toolu_spawn"
                        && matches!(event.as_ref(), AgentEvent::UserMessage { text } if text == "Also read the rebuild.")
            )),
            "{ev:?}"
        );
        // Unknown recipient (no task_started seen) or a subagent-owned shell
        // task's task_started: no steer synthesized.
        let mut norm = Normalizer::new();
        let shell_task = crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_started","task_id":"bg1","tool_use_id":"toolu_bash","task_type":"local_bash"}"#,
        )
        .expect("parses");
        assert!(norm.normalize(shell_task, false).is_empty());
        let send = crate::claude::wire::parse_frame(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"SendMessage","input":{"to":"bg1","message":"x"}}]}}"#,
        )
        .expect("parses");
        assert!(
            !norm
                .normalize(send, false)
                .iter()
                .any(|e| matches!(e, AgentEvent::Subagent { .. }))
        );
    }

    #[test]
    fn tagged_user_text_becomes_a_subagent_steer() {
        // A tagged user frame's TEXT block is the parent steering its
        // subagent — forwarded as a tagged UserMessage so the subagent doc
        // grows a user entry.
        let ev = normalize_one(
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"text","text":"Also check the rebuild path."}]}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::UserMessage {
                    text: "Also check the rebuild path.".into(),
                }),
            }]
        );
        // Synthetic harness injections are not conversation, and blank text
        // is noise; an UNTAGGED user text frame is not a steer at all (the
        // parent chat's user messages come from doc commands).
        for frame in [
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"text","text":"<system-reminder>tick</system-reminder>"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"text","text":"   "}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"typed into the parent"}]}}"#,
        ] {
            assert_eq!(normalize_one(frame), Vec::new(), "frame: {frame}");
        }
        // Mixed frames keep both: the tool result AND the steer text.
        let ev = normalize_one(
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false},{"type":"text","text":"Keep going."}]}}"#,
        );
        assert!(matches!(
            &ev[..],
            [
                AgentEvent::Subagent { event: first, .. },
                AgentEvent::Subagent { event: second, .. },
            ] if matches!(first.as_ref(), AgentEvent::ToolResult { .. })
                && matches!(second.as_ref(), AgentEvent::UserMessage { text } if text == "Keep going.")
        ));
    }

    #[test]
    fn task_notification_settles_the_subagent_with_a_tagged_done() {
        // The wire's ONLY terminal signal for a background subagent
        // (live-verified 2.1.228): an untagged system frame carrying the
        // spawning tool's id. Shape from the captured fixture. The spawn's
        // own tool_use frame always precedes it — that's what marks the id
        // as an AGENT task (shell tasks share the subtype).
        let spawn = |norm: &mut Normalizer| {
            let frame = crate::claude::wire::parse_frame(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_agent","name":"Agent","input":{"description":"probe"}}]}}"#,
            )
            .expect("parses");
            norm.normalize(frame, false);
        };
        let notify = |norm: &mut Normalizer, raw: &str| {
            let frame = crate::claude::wire::parse_frame(raw).expect("parses");
            norm.normalize(frame, false)
        };
        let mut norm = Normalizer::new();
        spawn(&mut norm);
        let ev = notify(
            &mut norm,
            r#"{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_agent","status":"completed","summary":"DONE."}"#,
        );
        assert!(matches!(
            &ev[..],
            [AgentEvent::Subagent { parent_tool_use_id, event }]
                if parent_tool_use_id == "toolu_agent"
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
        let mut norm = Normalizer::new();
        spawn(&mut norm);
        let ev = notify(
            &mut norm,
            r#"{"type":"system","subtype":"task_notification","tool_use_id":"toolu_agent","status":"failed"}"#,
        );
        assert!(matches!(
            &ev[..],
            [AgentEvent::Subagent { event, .. }]
                if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Errored, .. })
        ));
        // Non-terminal or id-less notifications close nothing.
        let mut norm = Normalizer::new();
        spawn(&mut norm);
        assert!(notify(
            &mut norm,
            r#"{"type":"system","subtype":"task_notification","tool_use_id":"toolu_agent","status":"running"}"#,
        )
        .is_empty());
        assert!(
            normalize_one(
                r#"{"type":"system","subtype":"task_notification","status":"completed"}"#,
            )
            .is_empty()
        );
    }

    #[test]
    fn resumed_task_notifications_settle_the_original_spawn() {
        for notification in [
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_send","status":"completed"}"#,
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","status":"stopped"}"#,
        ] {
            let mut norm = Normalizer::new();
            for raw in [
                r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_spawn","subagent_type":"general-purpose"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send","name":"SendMessage","input":{"to":"a1","message":"Keep going."}}]}}"#,
                r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_send","subagent_type":"general-purpose"}"#,
            ] {
                norm.normalize(crate::claude::wire::parse_frame(raw).unwrap(), false);
            }
            let events = norm.normalize(
                crate::claude::wire::parse_frame(notification).unwrap(),
                false,
            );
            let expected = if notification.contains("completed") {
                DoneStatus::Completed
            } else {
                DoneStatus::Interrupted
            };
            assert_eq!(
                events,
                vec![tag(
                    "toolu_spawn",
                    AgentEvent::Done {
                        status: expected,
                        result: None,
                        error: None,
                        session_id: None,
                    }
                )]
            );
            // The next steer must still use the original transcript too.
            let events = norm.normalize(crate::claude::wire::parse_frame(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send2","name":"SendMessage","input":{"to":"a1","message":"One more check."}}]}}"#,
            ).unwrap(), false);
            assert!(
                events.iter().any(|e| matches!(e,
                    AgentEvent::Subagent { parent_tool_use_id, event }
                        if parent_tool_use_id == "toolu_spawn"
                            && matches!(event.as_ref(), AgentEvent::UserMessage { .. })
                )),
                "{events:?}"
            );
        }
    }

    #[test]
    fn resumed_child_frames_use_the_original_transcript() {
        let mut norm = Normalizer::new();
        for raw in [
            r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_spawn","subagent_type":"general-purpose"}"#,
            r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_send","subagent_type":"general-purpose"}"#,
        ] {
            norm.normalize(crate::claude::wire::parse_frame(raw).unwrap(), false);
        }
        for raw in [
            r#"{"type":"stream_event","parent_tool_use_id":"toolu_send","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"working"}}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"toolu_send","message":{"content":[{"type":"text","text":"finished"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"toolu_send","message":{"content":[{"type":"tool_result","tool_use_id":"child-tool"}]}}"#,
        ] {
            let events = norm.normalize(crate::claude::wire::parse_frame(raw).unwrap(), false);
            assert!(!events.is_empty());
            assert!(events.iter().all(|e| matches!(e,
                AgentEvent::Subagent { parent_tool_use_id, .. } if parent_tool_use_id == "toolu_spawn"
            )), "{events:?}");
        }
    }

    #[tokio::test]
    async fn resumed_session_restores_task_ids_from_the_native_transcript() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("projects/encoded-project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("session-1.jsonl"), concat!(
            "not json\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_spawn","name":"Agent","input":{"description":"scout"}}]}}"#,
            "\n",
            r#"{"type":"user","toolUseResult":{"agentId":"a1","status":"async_launched"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_spawn"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send","name":"SendMessage","input":{"to":"a1","message":"continue"}}]}}"#,
            "\n",
            r#"{"type":"user","toolUseResult":{"agentId":"a1"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_send"}]}}"#,
            "\n",
            // A subagent's own nested spawn is not a chip on this chat.
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"toolu_nested","name":"Agent","input":{"description":"deeper"}}]}}"#,
            "\n",
            r#"{"type":"user","isSidechain":true,"toolUseResult":{"agentId":"a2"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_nested"}]}}"#,
            "\n{torn tail"
        )).unwrap();
        let (mut norm, transcript) = Normalizer::for_resume(root.path(), "session-1").await;
        assert_eq!(transcript, Transcript::Conversation);
        assert!(!norm.agent_spawn_tools.contains("toolu_nested"));
        assert!(!norm.agent_tasks.contains_key("a2"));
        // The revived process has no original spawn frame on its wire.
        norm.normalize(crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_send","subagent_type":"general-purpose"}"#,
        ).unwrap(), false);
        let events = norm.normalize(crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_send","status":"completed"}"#,
        ).unwrap(), false);
        assert_eq!(
            events,
            vec![tag(
                "toolu_spawn",
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                }
            )]
        );
        assert!(!norm.saw_init, "history must not consume the live init");
        // Another config root may hold it: only the CLI can tell.
        let (missing, transcript) = Normalizer::for_resume(root.path(), "missing").await;
        assert!(missing.agent_tasks.is_empty());
        assert_eq!(transcript, Transcript::Unknown);
    }

    #[tokio::test]
    async fn a_transcript_without_conversation_cannot_be_resumed() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("projects/encoded-project");
        std::fs::create_dir_all(&project).unwrap();
        // What a run that died before its first reply leaves behind.
        std::fs::write(
            project.join("bare.jsonl"),
            concat!(
                r#"{"type":"queue-operation","operation":"enqueue","content":"{\"type\":\"user\"}"}"#,
                "\n",
                r#"{"type":"last-prompt","lastPrompt":"hi"}"#,
                "\n",
            ),
        )
        .unwrap();
        assert_eq!(
            Normalizer::for_resume(root.path(), "bare").await.1,
            Transcript::Empty
        );
        // A line that might have been conversation, cut short or not text:
        // the CLI decides.
        for (name, body) in [
            (
                "torn",
                &b"{\"type\":\"last-prompt\"}\n{\"type\":\"user\",\"mess"[..],
            ),
            ("garbled", &b"{\"type\":\"last-prompt\"}\n\xff\xfe\n"[..]),
        ] {
            std::fs::write(project.join(format!("{name}.jsonl")), body).unwrap();
            assert_eq!(
                Normalizer::for_resume(root.path(), name).await.1,
                Transcript::Unknown,
                "{name}"
            );
        }
        // No history at all: nothing can be said either way.
        let nowhere = tempfile::tempdir().unwrap();
        assert_eq!(
            Normalizer::for_resume(nowhere.path(), "bare").await.1,
            Transcript::Unknown
        );
    }

    #[test]
    fn a_forked_skill_becomes_a_spawn_when_its_task_starts() {
        // Shapes from a live 2.1.310 capture. A `context: fork` skill runs as
        // a subagent under its Skill call's id, and its `task_started` comes
        // before any of that traffic. An inline skill starts no task and
        // stays an ordinary tool.
        let feed = |norm: &mut Normalizer, raw: &str| {
            norm.normalize(
                crate::claude::wire::parse_frame(raw).expect("parses"),
                false,
            )
        };
        let skill_call = |id: &str, skill: &str| {
            format!(
                r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Skill","input":{{"skill":"{skill}"}}}}]}}}}"#
            )
        };
        let task_started = |task: &str, tool: &str| {
            format!(
                r#"{{"type":"system","subtype":"task_started","task_id":"{task}","tool_use_id":"{tool}","subagent_type":"general-purpose","task_type":"local_agent","is_backgrounded":false}}"#
            )
        };
        let skill_text = r#"{"type":"user","parent_tool_use_id":"toolu_skill","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /p/.claude/skills/fanout"}]}}"#;
        let mut norm = Normalizer::new();
        let events = feed(&mut norm, &skill_call("toolu_skill", "fanout"));
        assert!(
            matches!(&events[0], AgentEvent::ToolCall { id, call } if id == "toolu_skill" && !call.is_subagent_spawn()),
            "{events:?}"
        );
        // Its task start turns the chip into a spawn, ahead of its traffic.
        assert_eq!(
            feed(&mut norm, &task_started("a1", "toolu_skill")),
            [AgentEvent::ToolCall {
                id: "toolu_skill".into(),
                call: ToolCall::Unknown {
                    name: "Agent: /fanout".into(),
                    input: Some(json!({"skill": "fanout"})),
                },
            }]
        );
        let events = feed(&mut norm, skill_text);
        assert!(
            matches!(&events[..], [AgentEvent::Subagent { parent_tool_use_id, event }]
                if parent_tool_use_id == "toolu_skill"
                    && matches!(event.as_ref(), AgentEvent::UserMessage { .. })),
            "its traffic passes as is: {events:?}"
        );
        let events = feed(
            &mut norm,
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_skill","status":"completed"}"#,
        );
        assert!(
            matches!(&events[..], [AgentEvent::Subagent { parent_tool_use_id, event }]
                if parent_tool_use_id == "toolu_skill"
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })),
            "it settles like any spawn: {events:?}"
        );
        // An agent the skill spawns starts a task and sends traffic too, but
        // its call is on the skill's own feed: nothing here is re-decoded.
        assert!(feed(&mut norm, &task_started("a2", "toolu_nested")).is_empty());
        assert!(matches!(
            &feed(
                &mut norm,
                r#"{"type":"assistant","parent_tool_use_id":"toolu_nested","message":{"content":[{"type":"text","text":"alpha"}]}}"#,
            )[..],
            [AgentEvent::Subagent { parent_tool_use_id, .. }] if parent_tool_use_id == "toolu_nested"
        ));
        // An inline skill's result comes back untagged, with no task start:
        // it stays a tool, and nothing waits on it any more.
        feed(&mut norm, &skill_call("toolu_inline", "lint"));
        feed(
            &mut norm,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_inline","content":"Launching skill: lint"}]}}"#,
        );
        assert!(norm.skill_calls.is_empty(), "{:?}", norm.skill_calls);
        // So a late task start for it re-decodes nothing.
        assert!(feed(&mut norm, &task_started("a3", "toolu_inline")).is_empty());
    }

    // `/fanout` typed as the prompt, in the shapes of a live 2.1.310 capture
    // (`live-2.1.310-typed-forked-command.jsonl`): a task start with no tool
    // id, its notification, then its held transcript, replayed under
    // `forked-command-fanout`.
    const TYPED_STARTED: &str = r#"{"type":"system","subtype":"task_started","task_id":"t1","description":"/fanout","subagent_type":"general-purpose","task_type":"local_agent","skip_transcript":true,"ambient":true,"prompt":"Fan out two agents.","is_backgrounded":false,"spawn_depth":1}"#;
    const TYPED_NOTIFIED: &str = r#"{"type":"system","subtype":"task_notification","task_id":"t1","status":"completed","summary":"/fanout","skip_transcript":true,"ambient":true}"#;
    const TYPED_ECHO: &str = r#"{"type":"user","parent_tool_use_id":"forked-command-fanout","message":{"role":"user","content":[{"type":"text","text":"Fan out two agents."}]}}"#;
    const TYPED_CALL: &str = r#"{"type":"assistant","parent_tool_use_id":"forked-command-fanout","message":{"content":[{"type":"tool_use","id":"toolu_alpha","name":"Agent","input":{"description":"alpha","prompt":"Reply alpha."}}]}}"#;
    const TYPED_RESULT: &str = r#"{"type":"user","parent_tool_use_id":"forked-command-fanout","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_alpha"}]}}"#;
    const TURN_END: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"alpha bravo","session_id":"s1"}"#;
    const TYPED_CHIP: &str = "forked-command-fanout-t1";

    fn feed_one(norm: &mut Normalizer, raw: &str) -> Vec<AgentEvent> {
        norm.normalize(
            crate::claude::wire::parse_frame(raw).expect("parses"),
            false,
        )
    }

    /// How many of `events` feed `parent`'s transcript.
    fn routed_to(events: &[AgentEvent], parent: &str) -> usize {
        events
            .iter()
            .filter(|e| {
                matches!(e, AgentEvent::Subagent { parent_tool_use_id, .. } if parent_tool_use_id == parent)
            })
            .count()
    }

    fn typed_settled(status: DoneStatus) -> [AgentEvent; 2] {
        [
            tag(
                TYPED_CHIP,
                AgentEvent::Done {
                    status,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ),
            AgentEvent::ToolResult {
                id: TYPED_CHIP.into(),
                is_error: status == DoneStatus::Errored,
                output: None,
                diff: None,
            },
        ]
    }

    #[test]
    fn a_typed_fork_is_a_spawn_chip_from_its_task_start() {
        let mut norm = Normalizer::new();
        let started = feed_one(&mut norm, TYPED_STARTED);
        assert_eq!(
            started,
            [
                AgentEvent::ToolCall {
                    id: TYPED_CHIP.into(),
                    call: ToolCall::Unknown {
                        name: "Agent: /fanout".into(),
                        input: Some(json!({"skill": "fanout"})),
                    },
                },
                tag(
                    TYPED_CHIP,
                    AgentEvent::UserMessage {
                        text: "Fan out two agents.".into()
                    }
                ),
            ]
        );
        assert!(
            matches!(&started[0], AgentEvent::ToolCall { call, .. } if call.is_subagent_spawn())
        );
        // A description of several words is named by its first. (Typed with
        // arguments, 2.1.310 still describes the bare command: see
        // `a_typed_forks_replayed_prompt_is_dropped_once`.)
        let mut norm = Normalizer::new();
        let started = feed_one(
            &mut norm,
            &TYPED_STARTED.replace(r#""/fanout""#, r#""/code-review low""#),
        );
        assert!(
            matches!(&started[0], AgentEvent::ToolCall { id, call: ToolCall::Unknown { name, .. } }
                if id == "forked-command-code-review-t1" && name == "Agent: /code-review"),
            "{started:?}"
        );
        // An agent task without a tool id that names no command is not one.
        let mut norm = Normalizer::new();
        assert!(
            feed_one(
                &mut norm,
                &TYPED_STARTED.replace(r#""/fanout""#, r#""Explore the repo""#)
            )
            .is_empty()
        );
        assert!(norm.typed_fork.is_none());
    }

    #[test]
    fn a_typed_forks_replay_feeds_its_chip_under_any_forked_command_id() {
        let mut norm = Normalizer::new();
        feed_one(&mut norm, TYPED_STARTED);
        // Notified before its replay: settled here, its tab would drop the
        // replay as stragglers.
        assert!(feed_one(&mut norm, TYPED_NOTIFIED).is_empty());
        for raw in [
            TYPED_CALL.to_owned(),
            // The name after the prefix "can differ from the one you typed".
            TYPED_CALL.replace("forked-command-fanout", "forked-command-plugin-fanout"),
            TYPED_RESULT.to_owned(),
            r#"{"type":"stream_event","parent_tool_use_id":"forked-command-fanout","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"alpha"}}}"#.to_owned(),
        ] {
            let events = feed_one(&mut norm, &raw);
            assert!(
                !events.is_empty() && routed_to(&events, TYPED_CHIP) == events.len(),
                "{raw}: {events:?}"
            );
        }
        // An agent it starts keeps its own transcript.
        let nested = feed_one(
            &mut norm,
            &TYPED_CALL.replace("forked-command-fanout", "toolu_alpha"),
        );
        assert_eq!(routed_to(&nested, "toolu_alpha"), 1, "{nested:?}");
    }

    #[test]
    fn a_typed_forks_replayed_prompt_is_dropped_once() {
        let mut norm = Normalizer::new();
        feed_one(&mut norm, TYPED_STARTED);
        // A tool result first leaves the echo to come.
        assert_eq!(routed_to(&feed_one(&mut norm, TYPED_RESULT), TYPED_CHIP), 1);
        assert!(
            feed_one(&mut norm, TYPED_ECHO).is_empty(),
            "the replay echoes the prompt the tab opened with"
        );
        // The same words again are the fork's own conversation.
        assert_eq!(
            feed_one(&mut norm, TYPED_ECHO),
            [tag(
                TYPED_CHIP,
                AgentEvent::UserMessage {
                    text: "Fan out two agents.".into()
                }
            )]
        );
        // `/fanout two words` (live 2.1.310): the start describes the bare
        // command, and its prompt and the echo both end with the arguments.
        let with_args = |raw: &str| {
            raw.replace(
                "Fan out two agents.",
                r"Fan out two agents.\n\n\nARGUMENTS: two words",
            )
        };
        let mut norm = Normalizer::new();
        let started = feed_one(&mut norm, &with_args(TYPED_STARTED));
        assert_eq!(
            started.get(1),
            Some(&tag(
                TYPED_CHIP,
                AgentEvent::UserMessage {
                    text: "Fan out two agents.\n\n\nARGUMENTS: two words".into()
                }
            )),
            "{started:?}"
        );
        assert!(feed_one(&mut norm, &with_args(TYPED_ECHO)).is_empty());
    }

    #[test]
    fn a_typed_fork_settles_once_just_before_its_turns_done() {
        let mut norm = Normalizer::new();
        feed_one(&mut norm, TYPED_STARTED);
        // Notified failed in a turn that completes: its own status wins.
        assert!(feed_one(&mut norm, &TYPED_NOTIFIED.replace("completed", "failed")).is_empty());
        feed_one(&mut norm, TYPED_CALL);
        let end = feed_one(&mut norm, TURN_END);
        let n = end.len();
        assert!(n >= 3, "{end:?}");
        assert_eq!(
            end[n - 3..n - 1],
            typed_settled(DoneStatus::Errored),
            "{end:?}"
        );
        assert!(
            matches!(
                end[n - 1],
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    ..
                }
            ),
            "{end:?}"
        );
        assert_eq!(routed_to(&end, TYPED_CHIP), 1, "{end:?}");
        // A late notification, and the next turn's end, settle nothing again.
        assert!(feed_one(&mut norm, TYPED_NOTIFIED).is_empty());
        assert!(!feed_one(&mut norm, TURN_END).iter().any(|e| matches!(
            e,
            AgentEvent::Subagent { .. } | AgentEvent::ToolResult { .. }
        )));
    }

    #[test]
    fn each_typed_run_of_a_command_is_its_own_chip() {
        let mut norm = Normalizer::new();
        for raw in [TYPED_STARTED, TYPED_NOTIFIED, TYPED_CALL, TURN_END] {
            feed_one(&mut norm, raw);
        }
        // The CLI replays every run of the command under the same id.
        let again = feed_one(
            &mut norm,
            &TYPED_STARTED.replace(r#""task_id":"t1""#, r#""task_id":"t2""#),
        );
        assert!(
            matches!(&again[0], AgentEvent::ToolCall { id, .. } if id == "forked-command-fanout-t2"),
            "{again:?}"
        );
        let call = feed_one(&mut norm, TYPED_CALL);
        assert_eq!(routed_to(&call, "forked-command-fanout-t2"), 1, "{call:?}");
    }

    #[test]
    fn a_second_typed_fork_settles_the_first() {
        let mut norm = Normalizer::new();
        feed_one(&mut norm, TYPED_STARTED);
        // No result between the two starts: the first must not run on.
        let second = feed_one(
            &mut norm,
            &TYPED_STARTED.replace(r#""task_id":"t1""#, r#""task_id":"t2""#),
        );
        assert_eq!(
            second[..2],
            typed_settled(DoneStatus::Interrupted),
            "{second:?}"
        );
        assert!(
            matches!(&second[2], AgentEvent::ToolCall { id, .. } if id == "forked-command-fanout-t2"),
            "{second:?}"
        );
        // The first's late notification finds no spawn.
        assert!(feed_one(&mut norm, TYPED_NOTIFIED).is_empty());
    }

    #[test]
    fn nothing_reaches_a_settled_typed_forks_chip() {
        let mut norm = Normalizer::new();
        for raw in [TYPED_STARTED, TYPED_NOTIFIED, TYPED_CALL, TURN_END] {
            feed_one(&mut norm, raw);
        }
        // Late replay frames keep the CLI's own id, which no chip carries:
        // they stay in the run journal. The echo routed to the chip would
        // read as a steer, and reopen it.
        for raw in [TYPED_ECHO, TYPED_CALL] {
            let late = feed_one(&mut norm, raw);
            assert!(
                !late.is_empty() && routed_to(&late, "forked-command-fanout") == late.len(),
                "{raw}: {late:?}"
            );
        }
    }

    #[test]
    fn an_interrupted_turn_settles_its_typed_fork_too() {
        // Stopped before any notification: settled as the turn ended.
        let mut norm = Normalizer::new();
        feed_one(&mut norm, TYPED_STARTED);
        let end = norm.normalize(crate::claude::wire::parse_frame(TURN_END).unwrap(), true);
        let n = end.len();
        assert_eq!(
            end[n - 3..n - 1],
            typed_settled(DoneStatus::Interrupted),
            "{end:?}"
        );
        assert!(matches!(
            end[n - 1],
            AgentEvent::Done {
                status: DoneStatus::Interrupted,
                ..
            }
        ));
        // A result line too torn to read still ends the turn, and its fork.
        let mut norm = Normalizer::new();
        feed_one(&mut norm, TYPED_STARTED);
        let end = feed_one(
            &mut norm,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["#,
        );
        assert_eq!(end[..2], typed_settled(DoneStatus::Errored), "{end:?}");
        assert!(matches!(
            end[2..],
            [AgentEvent::Done {
                status: DoneStatus::Errored,
                ..
            }]
        ));
    }

    // `/f-slow-steps` typed as the prompt (three 20 s steps, live 2.1.310):
    // what stdout brought, and the fork's transcript as the CLI wrote it to
    // disk meanwhile.
    const SLOW: &str =
        include_str!("../../tests/fixtures/claude/live-2.1.310-typed-fork-slow-steps.jsonl");
    const SLOW_ON_DISK: &str = include_str!(
        "../../tests/fixtures/claude/live-2.1.310-typed-fork-slow-steps-transcript.jsonl"
    );
    const SLOW_CHIP: &str = "forked-command-f-slow-steps-ae74593bb0c051579";
    /// The slow fork's tab, past the prompt it opened with.
    const SLOW_TAB: [&str; 9] = [
        "thinking This is the f-slow-s",
        "call qByz",
        "result qByz",
        "thinking Step 1 returned \"ste",
        "call Y4gV",
        "result Y4gV",
        "call E8dg",
        "result E8dg",
        "text steps done",
    ];

    /// The slow fork started, and its transcript's records on disk.
    fn slow_fork() -> (Normalizer, Vec<Value>) {
        let mut norm = Normalizer::new();
        feed_one(&mut norm, SLOW.lines().next().unwrap());
        let records = SLOW_ON_DISK
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        (norm, records)
    }

    fn slow_replay() -> impl Iterator<Item = &'static str> {
        SLOW.lines()
            .filter(|line| line.contains(r#""parent_tool_use_id":"forked-command-"#))
    }

    /// What `events` add to the slow fork's tab.
    fn slow_tab(events: &[AgentEvent]) -> Vec<String> {
        events
            .iter()
            .map(|e| match e {
                AgentEvent::Subagent {
                    parent_tool_use_id,
                    event,
                } if parent_tool_use_id == SLOW_CHIP => match event.as_ref() {
                    AgentEvent::ReasoningDelta { text } => format!("thinking {:.20}", text),
                    AgentEvent::TextDelta { text } => format!("text {}", text.trim()),
                    AgentEvent::ToolCall { id, .. } => format!("call {}", &id[id.len() - 4..]),
                    AgentEvent::ToolResult { id, .. } => {
                        format!("result {}", &id[id.len() - 4..])
                    }
                    other => format!("{other:?}"),
                },
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_typed_forks_transcript_on_disk_shows_its_messages_and_results_only() {
        let (mut norm, records) = slow_fork();
        assert_eq!(
            norm.fork_to_follow(),
            Some((
                SLOW_CHIP,
                "133a2e35-c338-4601-8108-99dcb7d09af2",
                "ae74593bb0c051579"
            ))
        );
        // Its prompt (the tab opened with it), a date attachment and an API
        // request record carrying message content show nothing.
        let shown: Vec<AgentEvent> = records
            .into_iter()
            .flat_map(|r| norm.fork_record(r))
            .collect();
        assert_eq!(slow_tab(&shown), SLOW_TAB);
        // Nor would user text: nothing steers a typed fork.
        let text = json!({"type": "user", "uuid": "u9", "message": {"role": "user",
            "content": [{"type": "text", "text": "Keep going."}]}});
        assert!(norm.fork_record(text).is_empty());
    }

    #[test]
    fn a_record_shows_once_whether_the_disk_or_the_replay_brings_it_first() {
        // Read off disk first: the replay repeats it under its uuid.
        let (mut norm, records) = slow_fork();
        let mut shown: Vec<AgentEvent> = records
            .into_iter()
            .flat_map(|r| norm.fork_record(r))
            .collect();
        for frame in slow_replay() {
            shown.extend(feed_one(&mut norm, frame));
        }
        assert_eq!(slow_tab(&shown), SLOW_TAB);
        // The fork ends between two reads: what the replay brought first is
        // not read again.
        let (mut norm, records) = slow_fork();
        let (before, after) = records.split_at(5);
        let mut shown: Vec<AgentEvent> = before
            .iter()
            .flat_map(|r| norm.fork_record(r.clone()))
            .collect();
        for frame in slow_replay() {
            shown.extend(feed_one(&mut norm, frame));
        }
        shown.extend(after.iter().flat_map(|r| norm.fork_record(r.clone())));
        assert_eq!(slow_tab(&shown), SLOW_TAB);
    }

    #[test]
    fn a_replay_that_repeats_nothing_read_off_disk_ends_the_reading() {
        let next_fork = SLOW
            .lines()
            .next()
            .unwrap()
            .replace("ae74593bb0c051579", "a2");
        let run = |uuids: &str| {
            let (mut norm, records) = slow_fork();
            records.into_iter().for_each(|r| {
                norm.fork_record(r);
            });
            let replayed: Vec<AgentEvent> = slow_replay()
                .flat_map(|f| feed_one(&mut norm, &f.replacen(r#""uuid":""#, uuids, 1)))
                .collect();
            feed_one(&mut norm, TURN_END);
            feed_one(&mut norm, &next_fork);
            (slow_tab(&replayed), norm.fork_to_follow().is_some())
        };
        // Repeated under the same uuids: nothing shows twice, and the next
        // fork is read too.
        assert_eq!(run(r#""uuid":""#), (Vec::<String>::new(), true));
        // Under other uuids it all showed twice: the next fork fills from its
        // replay alone.
        assert_eq!(
            run(r#""uuid":"x"#),
            (SLOW_TAB.map(String::from).to_vec(), false)
        );
    }

    #[test]
    fn shell_task_notification_never_settles_a_subagent() {
        // `Bash` with `run_in_background` settles through the SAME
        // `task_notification` subtype, carrying the Bash call's own id.
        // Tagging that Done bound a subagent ref onto an ordinary Run chip,
        // which then rendered as a spawn chip opening an empty, never-created
        // subagent doc (user report 2026-08-20).
        let mut norm = Normalizer::new();
        for raw in [
            // The shell task's start is already unmapped (no subagent_type)…
            r#"{"type":"system","subtype":"task_started","task_id":"bg1","tool_use_id":"toolu_bash","task_type":"local_bash"}"#,
            // …and the Bash call itself must not mark the id as a spawn.
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_bash","name":"Bash","input":{"command":"git clone …","run_in_background":true}}]}}"#,
        ] {
            let frame = crate::claude::wire::parse_frame(raw).expect("parses");
            norm.normalize(frame, false);
        }
        let done = crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_notification","task_id":"bg1","tool_use_id":"toolu_bash","status":"completed"}"#,
        )
        .expect("parses");
        assert!(norm.normalize(done, false).is_empty());
    }

    #[test]
    fn subagent_assistant_blocks_emit_tagged_reasoning_text_and_calls() {
        // No tagged partial deltas exist on the 2.1.x wire: a subagent's text
        // and thinking arrive only as full blocks on its tagged assistant
        // frames. Each ends on a blank line, so blocks that fold together
        // stay apart; a signature-only thinking block carries nothing.
        let ev = normalize_one(
            r#"{"type":"assistant","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"thinking","thinking":"","signature":"sig"},{"type":"thinking","thinking":"Plan it. "},{"type":"thinking","thinking":"Then list."},{"type":"text","text":"working on it"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
        );
        let tagged = |event: AgentEvent| AgentEvent::Subagent {
            parent_tool_use_id: "toolu_sub".into(),
            event: Box::new(event),
        };
        assert_eq!(
            ev,
            [
                tagged(AgentEvent::ReasoningDelta {
                    text: "Plan it.\n\n".into()
                }),
                tagged(AgentEvent::ReasoningDelta {
                    text: "Then list.\n\n".into()
                }),
                tagged(AgentEvent::TextDelta {
                    text: "working on it\n\n".into()
                }),
                tagged(AgentEvent::ToolCall {
                    id: "t1".into(),
                    call: ToolCall::Exec {
                        command: "ls".into()
                    },
                }),
            ]
        );
    }

    #[test]
    fn wake_turn_init_is_deduped_but_second_result_still_emits_done() {
        // Live-verified 2.1.228 shape: background subagent → eager result,
        // then a second init (same session id) + second result on completion.
        let mut norm = Normalizer::new();
        let init = r#"{"type":"system","subtype":"init","model":"m","cwd":"/x","session_id":"s1"}"#;
        let frame = crate::claude::wire::parse_frame(init).unwrap();
        assert_eq!(norm.normalize(frame, false).len(), 1, "first init");
        let frame = crate::claude::wire::parse_frame(init).unwrap();
        assert!(
            norm.normalize(frame, false).is_empty(),
            "wake init deduped — SessionStarted is the fold's run boundary"
        );
        let result = r#"{"type":"result","subtype":"success","session_id":"s1"}"#;
        let frame = crate::claude::wire::parse_frame(result).unwrap();
        let events = norm.normalize(frame, false);
        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    ..
                })
            ),
            "wake turn settles with its own Done"
        );
    }

    #[test]
    fn ede_diagnostics_never_surface_as_errors() {
        // The CLI's internal turn-accounting breadcrumbs must not become
        // transcript error parts (they showed up as raw red boxes).
        let done = result_done(
            r#"{"type":"result","subtype":"error_during_execution","errors":["[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=null"]}"#,
        );
        match done {
            AgentEvent::Done { status, error, .. } => {
                assert_eq!(status, DoneStatus::Errored);
                assert_eq!(error, None, "diagnostic-only failure surfaces no text");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn real_errors_survive_diagnostic_filtering() {
        let done = result_done(
            r#"{"type":"result","subtype":"error_during_execution","errors":["[ede_diagnostic] turn aborted (x) stop_reason=null","Something real broke"]}"#,
        );
        match done {
            AgentEvent::Done { error, .. } => {
                assert_eq!(error.as_deref(), Some("Something real broke"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    fn done_of(events: &[AgentEvent]) -> (DoneStatus, Option<String>) {
        match events.last() {
            Some(AgentEvent::Done { status, error, .. }) => (*status, error.clone()),
            other => panic!("no done: {other:?}"),
        }
    }

    #[test]
    fn api_failures_filed_as_success_end_errored() {
        // Live 2.1.309 with an invalid key: the assistant frame carries the
        // error code, and the result says `success` with `is_error`.
        let mut norm = Normalizer::new();
        let assistant = norm.normalize(
            crate::claude::wire::parse_frame(
                r#"{"type":"assistant","message":{"model":"<synthetic>","content":[{"type":"text","text":"Invalid API key · Fix external API key"}]},"error":"authentication_failed","api_error_status":401}"#,
            )
            .unwrap(),
            false,
        );
        assert_eq!(
            assistant
                .iter()
                .filter(|e| matches!(e, AgentEvent::Error { .. }))
                .count(),
            1
        );
        let invalid_key = r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key · Fix external API key","terminal_reason":"api_error","api_error_status":401,"session_id":"s"}"#;
        let done = norm.normalize(
            crate::claude::wire::parse_frame(invalid_key).unwrap(),
            false,
        );
        // Errored, and the failure is not shown a second time.
        assert_eq!(done_of(&done), (DoneStatus::Errored, None));
        // Without the assistant's code, the result's own text is the error.
        assert_eq!(
            done_of(&normalize_one(invalid_key)),
            (
                DoneStatus::Errored,
                Some("Invalid API key · Fix external API key".into())
            )
        );

        let (status, error) = done_of(&normalize_one(
            r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: 429 rate_limit_error (resets 3pm)","api_error_status":429}"#,
        ));
        assert_eq!(status, DoneStatus::Errored);
        let error = error.unwrap();
        assert!(error.starts_with("Claude rate limit reached"), "{error}");
        assert!(error.contains("resets 3pm"), "{error}");

        let (_, error) = done_of(&normalize_one(
            r#"{"type":"result","subtype":"success","is_error":true,"result":"API Error: 529","api_error_status":529}"#,
        ));
        assert!(error.unwrap().contains("overloaded"));

        for full in [
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Prompt is too long","terminal_reason":"prompt_too_long"}"#,
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Prompt is too long"}"#,
        ] {
            let (status, error) = done_of(&normalize_one(full));
            assert_eq!(status, DoneStatus::Errored);
            assert!(error.unwrap().contains("/compact"), "{full}");
        }
    }

    #[test]
    fn terminal_reasons_decide_how_a_success_ended() {
        for (raw, expected) in [
            (
                r#"{"type":"result","subtype":"success","is_error":false,"result":"hi","terminal_reason":"completed"}"#,
                DoneStatus::Completed,
            ),
            (
                r#"{"type":"result","subtype":"success","is_error":false,"terminal_reason":"aborted_streaming"}"#,
                DoneStatus::Interrupted,
            ),
            (
                r#"{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"aborted_tools","errors":[]}"#,
                DoneStatus::Interrupted,
            ),
            (
                r#"{"type":"result","subtype":"success","is_error":false,"terminal_reason":"budget_exhausted"}"#,
                DoneStatus::Errored,
            ),
        ] {
            assert_eq!(done_of(&normalize_one(raw)).0, expected, "{raw}");
        }
    }

    #[test]
    fn an_unreadable_result_ends_the_turn() {
        let ok =
            normalize_one(r#"{"type":"result","subtype":"success","usage":{"input_tokens":"x"}}"#);
        assert_eq!(done_of(&ok), (DoneStatus::Completed, None));
        let failed = normalize_one(
            r#"{"type":"result","subtype":"success","is_error":true,"usage":{"input_tokens":"x"}}"#,
        );
        assert_eq!(done_of(&failed).0, DoneStatus::Errored);
        assert!(done_of(&failed).1.is_some());
    }

    #[test]
    fn api_retries_keep_the_turn_alive_and_are_noticed_once() {
        let mut norm = Normalizer::new();
        let retry = |norm: &mut Normalizer, attempt: u32| {
            norm.normalize(
                crate::claude::wire::parse_frame(&format!(
                    r#"{{"type":"system","subtype":"api_retry","attempt":{attempt},"max_retries":10,"retry_delay_ms":4000,"error_status":529,"error":"overloaded"}}"#
                ))
                .unwrap(),
                false,
            )
        };
        let heartbeat = AgentEvent::ReasoningDelta {
            text: String::new(),
        };
        assert_eq!(retry(&mut norm, 1), vec![heartbeat.clone()]);
        assert_eq!(retry(&mut norm, 2), vec![heartbeat.clone()]);
        let third = retry(&mut norm, 3);
        assert!(matches!(
            &third[..],
            [AgentEvent::ReasoningDelta { .. }, AgentEvent::Error { message }]
                if message.contains("overloaded") && message.contains("3 of 10")
        ));
        assert_eq!(retry(&mut norm, 4), vec![heartbeat.clone()]);
        // The next turn may notice its own retries.
        norm.normalize(
            crate::claude::wire::parse_frame(r#"{"type":"result","subtype":"success"}"#).unwrap(),
            false,
        );
        assert_eq!(retry(&mut norm, 3).len(), 2);
    }

    #[test]
    fn compaction_is_liveness_then_new_context_usage() {
        assert_eq!(
            normalize_one(r#"{"type":"system","subtype":"status","status":"compacting"}"#),
            vec![AgentEvent::ReasoningDelta {
                text: String::new()
            }]
        );
        assert_eq!(
            normalize_one(
                r#"{"type":"system","subtype":"compact_boundary","compact_metadata":{"trigger":"auto","pre_tokens":180000,"post_tokens":9000}}"#
            ),
            vec![AgentEvent::ContextUsage {
                tokens: Some(9000),
                window: None
            }]
        );
        assert!(normalize_one(r#"{"type":"system","subtype":"compact_boundary"}"#).is_empty());
        assert!(
            normalize_one(
                r#"{"type":"system","subtype":"informational","content":"x","level":"info"}"#
            )
            .is_empty()
        );
    }

    #[test]
    fn known_failure_subtypes_keep_mapped_wording() {
        // A known run-failure subtype stays visible with human wording even
        // when its errors array is all diagnostics (or empty).
        let done = result_done(
            r#"{"type":"result","subtype":"error_max_turns","errors":["[ede_diagnostic] turn aborted (max) stop_reason=null"]}"#,
        );
        match done {
            AgentEvent::Done { error, .. } => {
                assert_eq!(
                    error.as_deref(),
                    Some("The run hit the maximum number of turns.")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let done = result_done(r#"{"type":"result","subtype":"error_max_turns","errors":[]}"#);
        match done {
            AgentEvent::Done { error, .. } => {
                assert_eq!(
                    error.as_deref(),
                    Some("The run hit the maximum number of turns.")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn context_counts_cached_prompt_and_matches_primary_model() {
        let mut normalizer = Normalizer::new();
        let events = normalizer.normalize(super::super::wire::parse_frame(r#"{"type":"assistant","message":{"model":"primary","content":[],"usage":{"input_tokens":200,"cache_read_input_tokens":40000,"cache_creation_input_tokens":1800,"output_tokens":100}}}"#).unwrap(), false);
        assert!(events.contains(&AgentEvent::ContextUsage {
            tokens: Some(42000),
            window: None
        }));
        let events = normalizer.normalize(super::super::wire::parse_frame(r#"{"type":"assistant","parent_tool_use_id":"child","message":{"model":"child","content":[],"usage":{"input_tokens":999999}}}"#).unwrap(), false);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ContextUsage { .. }))
        );
        let events = normalizer.normalize(super::super::wire::parse_frame(r#"{"type":"result","subtype":"success","usage":{"input_tokens":999999},"modelUsage":{"primary":{"contextWindow":200000},"child":{"contextWindow":1000000}}}"#).unwrap(), false);
        assert!(events.contains(&AgentEvent::ContextUsage {
            tokens: None,
            window: Some(200000)
        }));
        assert!(!events.iter().any(|e| matches!(
            e,
            AgentEvent::ContextUsage {
                tokens: Some(_),
                ..
            }
        )));
    }
}
