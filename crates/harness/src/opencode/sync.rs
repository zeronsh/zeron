//! OpenCode's sessions as the background service holds them, for the engine
//! to show as chats and keep current: every session any client made (the
//! TUI, the desktop app, Zeron), its history as Zeron's own agent events,
//! and a feed of which sessions changed.
//!
//! Read off the service only (2.x): a private server has no sessions but the
//! run's own. Shapes verified live against 2.3.0 — sessions list newest
//! first and page by cursor; messages list newest first too; a tool part
//! carries `name`, `state.{status, input, content, error}`.

use std::time::Duration;

use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use zeron_proto::{AgentEvent, UserInputAnswer, UserInputQuestion};

use super::{
    OUTPUT_CAP, OpencodeHarness, Server, cap_text, form_answer_body, map_questions, oc_tool_call,
    unwrap_data, v2_question_form_payload,
};
use crate::HarnessError;

/// How many sessions or messages to ask for per page.
const PAGE: usize = 200;

/// A bound on pages read for one list, against a cursor that never ends.
const MAX_PAGES: usize = 1000;

/// A session as the service lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeSession {
    pub id: String,
    pub title: Option<String>,
    /// Where it runs: the project it belongs to.
    pub directory: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub archived: bool,
    /// A turn is running in it, in whichever client started it.
    pub busy: bool,
    /// `provider/model`, as Zeron names a model.
    pub model: Option<String>,
    /// The Zeron chat a run of Zeron's made it for (its metadata).
    pub zeron_chat: Option<String>,
    /// The session it was forked from (`/fork`, in any client).
    pub forked_from: Option<String>,
    /// An undo staged in it: this message and everything after it are taken
    /// back until a redo, and gone at the next prompt.
    pub reverted_from: Option<String>,
    /// The files that undo put back, as OpenCode lists them under it.
    pub reverted_files: Vec<RevertedFile>,
}

/// A file an undo put back: its path, what the undone turn did to it
/// (`added`, `modified`, `deleted`), and the lines that turn added and
/// removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevertedFile {
    pub path: String,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
}

/// One message of a session's history.
#[derive(Clone, Debug)]
pub struct NativeMessage {
    /// OpenCode's own id (`msg_…`): stable, so a re-read finds the same one.
    pub id: String,
    pub created_at_ms: i64,
    /// When it finished being written (an assistant reply's last token),
    /// if it has.
    pub completed_at_ms: Option<i64>,
    pub kind: NativeMessageKind,
}

impl NativeMessage {
    /// What the session's context holds after this message, as the meter
    /// counts it: a reply's whole prompt and answer; a finished compaction's
    /// summary, all that is left of what came before. `None` for others.
    pub fn context_tokens(&self) -> Option<u64> {
        match &self.kind {
            NativeMessageKind::Assistant { context, .. } => *context,
            NativeMessageKind::Compaction {
                status: CompactionStatus::Completed,
                tokens_out,
                ..
            } => *tokens_out,
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum NativeMessageKind {
    User {
        text: String,
    },
    /// What the agent did, as the events a run of Zeron's would have sent.
    Assistant {
        events: Vec<AgentEvent>,
        /// Its tokens, prompt and reply (`None` until it has any).
        context: Option<u64>,
    },
    /// The session compacted: by `/compact`, or by itself as it filled.
    Compaction {
        status: CompactionStatus,
        /// What the summary read (the whole prompt, cache included) and
        /// wrote, once done — OpenCode's own "in · out".
        tokens_in: Option<u64>,
        tokens_out: Option<u64>,
        /// The summary, once written: empty while it runs (its words come
        /// on the feed meanwhile, [`NativeChange::CompactionText`]).
        summary: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionStatus {
    Running,
    Completed,
    Failed,
}

impl CompactionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CompactionStatus::Running => "running",
            CompactionStatus::Completed => "completed",
            CompactionStatus::Failed => "failed",
        }
    }
}

/// A question the agent is waiting on (the `question` tool's form), in
/// whichever client it was asked.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeQuestion {
    /// The form's id (`frm_…`): the request an answer names.
    pub form_id: String,
    /// The panel's questions. Each id is `<form id>:<field key>`, the same on
    /// every read, so a question read again stays the one on screen and its
    /// answer finds its field.
    pub questions: Vec<UserInputQuestion>,
}

/// What changed on the service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeChange {
    /// Something about this session changed: read it again.
    Session(String),
    /// The session is gone.
    Deleted(String),
    /// The feed (re)connected; changes while it was down are lost, so every
    /// session should be read again.
    Resync,
    /// Agents, commands, skills, models, providers or the configuration
    /// changed: catalogs read before are out of date.
    Catalog,
    /// The next words of the summary a compaction of this session is
    /// writing (the session's own copy has none until it is done).
    CompactionText { session: String, text: String },
}

impl OpencodeHarness {
    /// Every top-level session on the service, archived ones included
    /// (subagent sessions belong to their parent's transcript).
    pub async fn native_sessions(&self) -> Result<Vec<NativeSession>, HarnessError> {
        let server = self.require_service().await?;
        let mut sessions = Vec::new();
        for archived in [false, true] {
            let path = format!("/api/session?limit={PAGE}&archived={archived}");
            let listed = pages(&server, &path, None).await?;
            sessions.extend(listed.iter().filter_map(native_session));
        }
        sessions.sort_by_key(|session| session.created_at_ms);
        sessions.dedup_by(|a, b| a.id == b.id);
        Ok(sessions)
    }

    /// One session, `None` when it no longer exists or is a subagent's.
    pub async fn native_session(&self, id: &str) -> Result<Option<NativeSession>, HarnessError> {
        let server = self.require_service().await?;
        match server.get_json(&format!("/api/session/{id}"), None).await {
            Ok(info) => Ok(native_session(&unwrap_data(info))),
            Err(HarnessError::Protocol(message)) if message.contains("404") => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// A session's history, oldest first.
    pub async fn native_history(
        &self,
        session: &NativeSession,
    ) -> Result<Vec<NativeMessage>, HarnessError> {
        let server = self.require_service().await?;
        let path = format!("/api/session/{}/message?limit={PAGE}", session.id);
        let listed = pages(&server, &path, Some(&session.directory)).await?;
        let mut messages: Vec<NativeMessage> = listed.iter().filter_map(native_message).collect();
        messages.sort_by_key(|message| message.created_at_ms);
        messages.dedup_by(|a, b| a.id == b.id);
        Ok(messages)
    }

    /// The context window of `model` (`provider/model`), as OpenCode's
    /// provider catalog gives it.
    pub async fn native_context_window(&self, model: &str) -> Result<Option<u64>, HarnessError> {
        let Some((provider_id, model_id)) = model.split_once('/') else {
            return Ok(None);
        };
        let server = self.require_service().await?;
        let providers = server.provider_catalog(None).await?;
        Ok(providers
            .all
            .iter()
            .flatten()
            .filter(|provider| provider.id.as_deref() == Some(provider_id))
            .flat_map(|provider| provider.models.iter().flatten())
            .find(|(id, _)| id.as_str() == model_id)
            .and_then(|(_, model)| model.limit.context.filter(|n| *n > 0)))
    }

    /// The questions `session` is waiting on, oldest first.
    pub async fn native_questions(
        &self,
        session: &NativeSession,
    ) -> Result<Vec<NativeQuestion>, HarnessError> {
        let server = self.require_service().await?;
        let forms = server
            .get_json(
                &format!("/api/session/{}/form", session.id),
                Some(&session.directory),
            )
            .await?;
        Ok(unwrap_data(forms)
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(native_question)
            .collect())
    }

    /// Answer a question `session` is waiting on, from the panel's answers;
    /// no answers at all closes it unanswered, as the TUI's dismiss does.
    pub async fn answer_native_question(
        &self,
        session_id: &str,
        directory: &str,
        form_id: &str,
        questions: &[UserInputQuestion],
        answers: &[UserInputAnswer],
    ) -> Result<(), HarnessError> {
        let server = self.require_service().await?;
        if answers.is_empty() {
            let path = format!("/api/session/{session_id}/form/{form_id}");
            return server.delete(&path, Some(directory)).await;
        }
        let pairs: Vec<(String, UserInputQuestion)> = questions
            .iter()
            .filter_map(|question| {
                let key = question.id.strip_prefix(form_id)?.strip_prefix(':')?;
                Some((key.to_owned(), question.clone()))
            })
            .collect();
        if pairs.is_empty() {
            return Err(HarnessError::Protocol(
                "these questions aren't that form's".into(),
            ));
        }
        server
            .post_json(
                &format!("/api/session/{session_id}/form/{form_id}/reply"),
                Some(directory),
                &form_answer_body(&pairs, answers),
            )
            .await
            .map(|_| ())
    }

    /// Which sessions change, for as long as the receiver is kept. The feed
    /// redials itself after a drop and says so ([`NativeChange::Resync`]).
    pub async fn follow_native(&self) -> Result<mpsc::Receiver<NativeChange>, HarnessError> {
        let server = self.require_service().await?;
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut backoff = Duration::from_millis(500);
            loop {
                // (Let go of with its receiver, even while nothing changes.)
                let followed = tokio::select! {
                    _ = tx.closed() => return,
                    followed = follow_once(&server, &tx) => followed,
                };
                match followed {
                    Ok(()) => backoff = Duration::from_millis(500),
                    Err(error) => {
                        tracing::debug!(
                            target: "zeron_harness::opencode",
                            %error,
                            "opencode session feed dropped"
                        );
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                    }
                }
            }
        });
        Ok(rx)
    }
}

/// Every item of the list at `path`, page after page by its cursor.
async fn pages(server: &Server, path: &str, directory: Option<&str>) -> Result<Vec<Value>, HarnessError> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let path = match &cursor {
            Some(cursor) => format!("{path}&cursor={}", super::encode_directory(cursor)),
            None => path.to_owned(),
        };
        let mut page = server.get_json(&path, directory).await?;
        let listed = match page.get_mut("data").map(Value::take) {
            Some(Value::Array(listed)) => listed,
            _ => Vec::new(),
        };
        cursor = page
            .pointer("/cursor/next")
            .and_then(Value::as_str)
            .map(str::to_owned);
        // (The service caps a page below `limit` and still offers a next
        // cursor; only an empty page ends the list.)
        let done = listed.is_empty() || cursor.is_none();
        items.extend(listed);
        if done {
            break;
        }
    }
    Ok(items)
}

/// One connection of the session feed: until it ends or the receiver goes.
async fn follow_once(server: &Server, tx: &mpsc::Sender<NativeChange>) -> Result<(), String> {
    let response = server
        .request(reqwest::Method::GET, "/api/event")
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("GET /api/event: {}", response.status()));
    }
    if tx.send(NativeChange::Resync).await.is_err() {
        return Ok(());
    }
    let mut body = response.bytes_stream();
    let mut buffer = String::new();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find('\n') {
            let line: String = buffer.drain(..=end).collect();
            let Some(data) = line.trim_end().strip_prefix("data:") else {
                continue;
            };
            let Ok(frame) = serde_json::from_str::<Value>(data.trim_start()) else {
                continue;
            };
            if let Some(change) = change_of(&frame)
                && tx.send(change).await.is_err()
            {
                return Ok(());
            }
        }
    }
    Ok(())
}

/// What a frame of the service's event feed means for the session list, if
/// anything. Streaming deltas are skipped: the steps and tools that close
/// them say the same, far less often.
fn change_of(frame: &Value) -> Option<NativeChange> {
    let kind = frame.get("type").and_then(Value::as_str)?;
    if matches!(
        kind,
        "agent.updated"
            | "command.updated"
            | "skill.updated"
            | "config.updated"
            | "model.updated"
            | "models-dev.refreshed"
            | "provider.updated"
            | "integration.updated"
            | "credential.updated"
            | "credential.switched"
            | "plugin.updated"
            | "mcp.status.changed"
    ) {
        return Some(NativeChange::Catalog);
    }
    let data = frame.get("data");
    let session = data
        .and_then(|data| data.get("sessionID"))
        .or_else(|| data.and_then(|data| data.pointer("/form/sessionID")))
        .or_else(|| frame.pointer("/durable/aggregateID"))
        .and_then(Value::as_str)
        .filter(|id| id.starts_with("ses"))?
        .to_owned();
    match kind {
        "session.deleted" => Some(NativeChange::Deleted(session)),
        "session.compaction.delta" => Some(NativeChange::CompactionText {
            session,
            text: data?.get("text").and_then(Value::as_str)?.to_owned(),
        }),
        "session.created"
        | "session.renamed"
        | "session.archived"
        | "session.unarchived"
        | "session.moved"
        | "session.forked"
        | "session.execution.started"
        | "session.execution.succeeded"
        | "session.execution.failed"
        | "session.execution.interrupted"
        | "session.idle"
        | "session.synthetic"
        | "session.step.ended"
        | "session.text.ended"
        | "session.tool.called"
        | "session.tool.success"
        | "session.tool.failed"
        | "session.revert.staged"
        | "session.revert.cleared"
        | "session.revert.committed"
        | "session.shell.started"
        | "session.shell.ended"
        | "session.compaction.started"
        | "session.compaction.ended"
        | "session.compaction.failed"
        | "session.inbox.enqueued"
        | "session.inbox.delivered"
        | "session.inbox.cancelled"
        | "session.step.started"
        | "session.step.failed"
        | "session.error"
        | "session.updated"
        | "form.created"
        | "form.replied"
        | "form.cancelled" => Some(NativeChange::Session(session)),
        _ => None,
    }
}

/// A listed session; subagent sessions (with a parent) are left out.
fn native_session(info: &Value) -> Option<NativeSession> {
    if info
        .get("parentID")
        .and_then(Value::as_str)
        .is_some_and(|parent| !parent.is_empty())
    {
        return None;
    }
    let id = info.get("id").and_then(Value::as_str)?.to_owned();
    let directory = info
        .pointer("/location/directory")
        .and_then(Value::as_str)?
        .to_owned();
    let time = |key: &str| {
        info.pointer(&format!("/time/{key}"))
            .and_then(Value::as_f64)
    };
    let forked_from = info
        .pointer("/fork/sessionID")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);
    let busy = match (time("busy"), time("idle")) {
        (Some(busy), Some(idle)) => busy > idle,
        (Some(_), None) => true,
        _ => false,
    };
    Some(NativeSession {
        id,
        title: info
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_owned),
        directory,
        created_at_ms: time("created").unwrap_or_default() as i64,
        updated_at_ms: time("updated").unwrap_or_default() as i64,
        archived: time("archived").is_some(),
        busy,
        model: match (
            info.pointer("/model/providerID").and_then(Value::as_str),
            info.pointer("/model/id").and_then(Value::as_str),
        ) {
            (Some(provider), Some(model)) => Some(format!("{provider}/{model}")),
            _ => None,
        },
        // (A fork copies its source's metadata: the tag is the source's.)
        zeron_chat: info
            .pointer("/metadata/zeron/chatId")
            .and_then(Value::as_str)
            .filter(|chat| !chat.is_empty() && forked_from.is_none())
            .map(str::to_owned),
        forked_from,
        reverted_from: info
            .pointer("/revert/messageID")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reverted_files: info
            .pointer("/revert/files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|file| {
                let count = |key: &str| file.get(key).and_then(Value::as_u64).unwrap_or(0);
                Some(RevertedFile {
                    path: file.get("file").and_then(Value::as_str)?.to_owned(),
                    status: file
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("modified")
                        .to_owned(),
                    additions: count("additions"),
                    deletions: count("deletions"),
                })
            })
            .collect(),
    })
}

/// A pending form as the panel's questions, when it is the question tool's.
fn native_question(form: &Value) -> Option<NativeQuestion> {
    let form_id = form.get("id").and_then(Value::as_str)?.to_owned();
    let payload = v2_question_form_payload(form).into_iter().next()?;
    let questions = map_questions(payload.get("properties")?)
        .into_iter()
        .map(|(key, mut question)| {
            question.id = format!("{form_id}:{key}");
            question
        })
        .collect::<Vec<_>>();
    (!questions.is_empty()).then_some(NativeQuestion { form_id, questions })
}

/// A user prompt or an assistant reply; bookkeeping (agent and model
/// switches, compaction, idle markers) has no transcript entry.
fn native_message(message: &Value) -> Option<NativeMessage> {
    let id = message.get("id").and_then(Value::as_str)?.to_owned();
    let created_at_ms = message
        .pointer("/time/created")
        .and_then(Value::as_f64)
        .unwrap_or_default() as i64;
    let kind = match message.get("type").and_then(Value::as_str)? {
        "user" => NativeMessageKind::User {
            text: message
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        // A `!command` run in the session, as the command chip it is.
        "shell" => NativeMessageKind::Assistant {
            events: super::actions::shell_events(
                Some(message),
                message
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            context: None,
        },
        "assistant" => NativeMessageKind::Assistant {
            events: message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .flat_map(part_events)
                .collect(),
            context: message
                .get("tokens")
                .map(|tokens| token_sum(tokens, &["input", "output", "cache/read", "cache/write"]))
                .filter(|tokens| *tokens > 0),
        },
        "compaction" => {
            let status = match message.get("status").and_then(Value::as_str) {
                Some("completed") => CompactionStatus::Completed,
                Some("failed" | "error") => CompactionStatus::Failed,
                _ => CompactionStatus::Running,
            };
            let tokens = message
                .get("tokens")
                .filter(|_| status == CompactionStatus::Completed);
            NativeMessageKind::Compaction {
                status,
                tokens_in: tokens.map(|t| token_sum(t, &["input", "cache/read", "cache/write"])),
                tokens_out: tokens.map(|t| token_sum(t, &["output", "reasoning"])),
                summary: message
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }
        }
        _ => return None,
    };
    Some(NativeMessage {
        id,
        created_at_ms,
        completed_at_ms: message
            .pointer("/time/completed")
            .and_then(Value::as_f64)
            .map(|ms| ms as i64),
        kind,
    })
}

/// The sum of a message's token counts at `paths` (`cache/read`, …).
fn token_sum(tokens: &Value, paths: &[&str]) -> u64 {
    paths
        .iter()
        .filter_map(|path| tokens.pointer(&format!("/{path}")).and_then(Value::as_u64))
        .fold(0, u64::saturating_add)
}

/// One part of an assistant message as the events that would have built it.
fn part_events(part: &Value) -> Vec<AgentEvent> {
    let text = || {
        part.get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    match part.get("type").and_then(Value::as_str) {
        Some("text") => text()
            .map(|text| vec![AgentEvent::TextDelta { text }])
            .unwrap_or_default(),
        Some("reasoning") => text()
            .map(|text| vec![AgentEvent::ReasoningDelta { text }])
            .unwrap_or_default(),
        Some("tool") => tool_events(part),
        _ => Vec::new(),
    }
}

/// A tool part: its call, and its result once it finished.
fn tool_events(part: &Value) -> Vec<AgentEvent> {
    let Some(id) = part.get("id").and_then(Value::as_str) else {
        return Vec::new();
    };
    let name = part.get("name").and_then(Value::as_str).unwrap_or_default();
    let state = part.get("state").cloned().unwrap_or(Value::Null);
    let input = state.get("input").cloned().unwrap_or(Value::Null);
    let mut events = vec![AgentEvent::ToolCall {
        id: id.to_owned(),
        call: oc_tool_call(name, &input),
    }];
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if matches!(status, "completed" | "error") {
        let output: String = state
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        let error = state.get("error").and_then(|error| {
            error.as_str().map(str::to_owned).or_else(|| {
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
        });
        let output = error.or((!output.is_empty()).then_some(output));
        events.push(AgentEvent::ToolResult {
            id: id.to_owned(),
            is_error: status == "error",
            output: output.map(|output| cap_text(&output, OUTPUT_CAP)),
            diff: None,
            preview_paths: Vec::new(),
            image_payloads: Vec::new(),
            exit_code: state
                .pointer("/metadata/exit")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok()),
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zeron_proto::ToolCall;

    #[test]
    fn sessions_read_their_place_and_state() {
        let listed = json!({
            "id": "ses_a", "title": "Fix the build", "location": {"directory": "/work"},
            "time": {"created": 10, "updated": 20, "busy": 30, "idle": 25}
        });
        let session = native_session(&listed).unwrap();
        assert_eq!(session.directory, "/work");
        assert_eq!(session.title.as_deref(), Some("Fix the build"));
        assert!(session.busy);
        assert!(!session.archived);
        let archived = json!({
            "id": "ses_b", "location": {"directory": "/work"},
            "time": {"created": 1, "updated": 2, "archived": 3, "busy": 1, "idle": 2}
        });
        let archived = native_session(&archived).unwrap();
        assert!(archived.archived);
        assert!(!archived.busy);
        assert_eq!(archived.title, None);
        // (A subagent's session is its parent's business.)
        let child = json!({"id": "ses_c", "parentID": "ses_a", "location": {"directory": "/w"}});
        assert!(native_session(&child).is_none());
        // (A fork's copied Zeron tag is its source's, not its own.)
        let fork = json!({
            "id": "ses_f", "location": {"directory": "/w"},
            "fork": {"sessionID": "ses_a"}, "metadata": {"zeron": {"chatId": "chat-a"}}
        });
        let fork = native_session(&fork).unwrap();
        assert_eq!(fork.forked_from.as_deref(), Some("ses_a"));
        assert_eq!(fork.zeron_chat, None);
    }

    #[test]
    fn an_assistant_message_reads_as_the_events_that_built_it() {
        let message = json!({
            "id": "msg_1", "type": "assistant", "time": {"created": 5},
            "content": [
                {"type": "reasoning", "text": "thinking"},
                {"type": "tool", "id": "prt_1", "name": "shell",
                 "state": {"status": "completed", "input": {"command": "ls"},
                           "content": [{"type": "text", "text": "a\nb"}],
                           "metadata": {"exit": 0}}},
                {"type": "tool", "id": "prt_2", "name": "todowrite",
                 "state": {"status": "running",
                           "input": {"todos": [{"content": "ship", "status": "in_progress"}]}}},
                {"type": "text", "text": "done"}
            ]
        });
        let message = native_message(&message).unwrap();
        let NativeMessageKind::Assistant { events, .. } = message.kind else {
            panic!("an assistant message");
        };
        assert!(matches!(&events[0], AgentEvent::ReasoningDelta { text } if text == "thinking"));
        assert!(matches!(
            &events[1],
            AgentEvent::ToolCall { id, call: ToolCall::Exec { command } } if id == "prt_1" && command == "ls"
        ));
        assert!(matches!(
            &events[2],
            AgentEvent::ToolResult { output: Some(output), exit_code: Some(0), is_error: false, .. }
                if output == "a\nb"
        ));
        assert!(
            matches!(&events[3], AgentEvent::ToolCall { call: ToolCall::Todo { items }, .. } if items.len() == 1)
        );
        assert!(matches!(&events[4], AgentEvent::TextDelta { text } if text == "done"));
    }

    #[test]
    fn bookkeeping_messages_have_no_entry() {
        for kind in ["agent-switched", "model-switched", "idle", "system"] {
            assert!(native_message(&json!({"id": "m", "type": kind})).is_none());
        }
        let user = native_message(&json!({"id": "m", "type": "user", "text": "hi"})).unwrap();
        assert!(matches!(user.kind, NativeMessageKind::User { text } if text == "hi"));
    }

    #[test]
    fn a_compaction_reads_as_opencode_counts_it_and_resets_the_context() {
        // (Shape captured live from 2.3.0.)
        let done = json!({
            "id": "msg_c", "type": "compaction", "status": "completed", "reason": "manual",
            "time": {"created": 10}, "summary": "## Goal\n- tides",
            "tokens": {"input": 4, "output": 2812, "reasoning": 0,
                       "cache": {"read": 646940, "write": 1427}}
        });
        let done = native_message(&done).unwrap();
        assert!(matches!(
            done.kind,
            NativeMessageKind::Compaction {
                status: CompactionStatus::Completed,
                tokens_in: Some(648_371),
                tokens_out: Some(2_812),
                ref summary,
            } if summary == "## Goal\n- tides"
        ));
        assert_eq!(done.context_tokens(), Some(2_812));
        let running = native_message(&json!({"id": "msg_d", "type": "compaction", "status": "running"})).unwrap();
        assert!(matches!(
            running.kind,
            NativeMessageKind::Compaction { status: CompactionStatus::Running, tokens_in: None, .. }
        ));
        assert_eq!(running.context_tokens(), None);
        let reply = native_message(&json!({
            "id": "msg_e", "type": "assistant", "content": [],
            "tokens": {"input": 10, "output": 5, "cache": {"read": 100, "write": 0}}
        }))
        .unwrap();
        assert_eq!(reply.context_tokens(), Some(115));
    }

    #[test]
    fn a_question_form_reads_with_ids_that_stay_put_and_answers_by_field() {
        let form = json!({
            "id": "frm_1", "sessionID": "ses_a", "title": "Questions",
            "metadata": {"kind": "question"},
            "fields": [
                {"key": "q0", "type": "string", "title": "Scope", "description": "Which part?",
                 "options": [{"value": "Engine", "label": "Engine"}, {"value": "UI", "label": "UI"}]},
                {"key": "q1", "type": "multiselect", "title": "Also", "description": "And?",
                 "options": [{"value": "Docs", "label": "Docs"}]}
            ]
        });
        let first = native_question(&form).unwrap();
        assert_eq!(first, native_question(&form).unwrap());
        assert_eq!(first.form_id, "frm_1");
        let ids: Vec<&str> = first.questions.iter().map(|q| q.id.as_str()).collect();
        assert_eq!(ids, ["frm_1:q0", "frm_1:q1"]);
        assert_eq!(first.questions[0].options, ["Engine", "UI"]);
        assert!(first.questions[1].multi_select);
        // (Another feature's form isn't the question panel's.)
        let other = json!({"id": "frm_2", "sessionID": "ses_a", "title": "x", "fields": []});
        assert!(native_question(&other).is_none());
    }

    #[test]
    fn the_feed_names_the_session_that_changed() {
        let idle = json!({"type": "session.idle", "data": {"sessionID": "ses_a"}});
        assert_eq!(
            change_of(&idle),
            Some(NativeChange::Session("ses_a".into()))
        );
        let deleted = json!({"type": "session.deleted", "durable": {"aggregateID": "ses_b"}});
        assert_eq!(
            change_of(&deleted),
            Some(NativeChange::Deleted("ses_b".into()))
        );
        let delta = json!({"type": "session.text.delta", "data": {"sessionID": "ses_a"}});
        assert_eq!(change_of(&delta), None);
        // (An agent added in the TUI: no session, the catalogs.)
        let agent = json!({"type": "agent.updated", "data": {}});
        assert_eq!(change_of(&agent), Some(NativeChange::Catalog));
        let form = json!({"type": "form.created", "data": {"form": {"sessionID": "ses_c"}}});
        assert_eq!(
            change_of(&form),
            Some(NativeChange::Session("ses_c".into()))
        );
    }
}
