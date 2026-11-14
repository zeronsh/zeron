//! OpenCode's TUI actions in Zeron's composer (2.x): the session actions its
//! TUI and desktop app offer that are not `/command` entries — the command
//! endpoint answers `CommandNotFoundError` for them — but each have a route
//! of their own on the service. And `!command`, which runs a shell command
//! in the session as the TUI's shell mode does.
//!
//! None writes to the transcript itself, and none starts a turn of the
//! agent's own: the session shows what it did, in Zeron as in OpenCode's
//! own clients, and the run settles the turn once it's done.
//! Semantics verified live against 2.3.0: a staged revert leaves the
//! messages listed (clients hide them) until the next prompt commits it;
//! clearing it brings them back; compaction is queued like a steer; a fork
//! is a new session; a shell run is a `shell` message of the session.

use std::time::Duration;

use serde_json::{Value, json};
use zeron_proto::{AgentEvent, SlashCommand, ToolCall};

use super::{OUTPUT_CAP, Server, cap_text, unwrap_data};
use crate::HarnessError;

/// How long a `!command` is waited on before the turn settles with what it
/// printed so far.
const SHELL_WAIT: Duration = Duration::from_secs(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TuiAction {
    Compact,
    Undo,
    Redo,
    Fork,
}

impl TuiAction {
    pub const ALL: [TuiAction; 4] = [
        TuiAction::Compact,
        TuiAction::Undo,
        TuiAction::Redo,
        TuiAction::Fork,
    ];

    pub fn name(self) -> &'static str {
        match self {
            TuiAction::Compact => "compact",
            TuiAction::Undo => "undo",
            TuiAction::Redo => "redo",
            TuiAction::Fork => "fork",
        }
    }

    fn description(self) -> &'static str {
        match self {
            TuiAction::Compact => "Compact the session",
            TuiAction::Undo => "Undo the last message",
            TuiAction::Redo => "Redo what undo took back",
            TuiAction::Fork => "Fork the session into a new chat",
        }
    }

    /// As the composer lists it.
    pub fn command(self) -> SlashCommand {
        SlashCommand {
            name: self.name().to_owned(),
            description: self.description().to_owned(),
            input_hint: None,
        }
    }

    /// The action `name` invokes, when the composer's entry for it is this
    /// one's — a command of the server's own by that name stays the server's.
    pub fn of(name: &str, commands: &[SlashCommand]) -> Option<Self> {
        let action = Self::ALL.into_iter().find(|action| action.name() == name)?;
        commands
            .iter()
            .any(|command| command.name == name && command.description == action.description())
            .then_some(action)
    }

    /// Do it. It answers with nothing: the transcript shows what it did
    /// as the session does in every client (the engine's session sync) —
    /// an undo's turn gone, a compaction's note, a fork's own chat. A
    /// compaction is waited on, so the chat shows it working until done.
    pub async fn run(
        self,
        server: &Server,
        session_id: &str,
        dir: Option<&str>,
    ) -> Result<Vec<AgentEvent>, HarnessError> {
        match self {
            TuiAction::Compact => {
                let started = now_ms();
                server
                    .post_json(
                        &format!("/api/session/{session_id}/compact"),
                        dir,
                        &json!({}),
                    )
                    .await?;
                let deadline = tokio::time::Instant::now() + COMPACT_WAIT;
                while tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    let messages = unwrap_data(
                        server
                            .get_json(&format!("/api/session/{session_id}/message?limit=20"), dir)
                            .await?,
                    );
                    if compaction_settled(&messages, started - 2000.0) {
                        break;
                    }
                }
            }
            TuiAction::Undo => {
                let messages = server
                    .get_json(&format!("/api/session/{session_id}/message?limit=200"), dir)
                    .await?;
                if let Some((id, _)) = last_prompt(&unwrap_data(messages)) {
                    server
                        .post_json(
                            &format!("/api/session/{session_id}/revert/stage"),
                            dir,
                            &json!({ "messageID": id }),
                        )
                        .await?;
                }
            }
            TuiAction::Redo => {
                let info = unwrap_data(
                    server
                        .get_json(&format!("/api/session/{session_id}"), dir)
                        .await?,
                );
                if info.get("revert").is_some_and(|revert| !revert.is_null()) {
                    let path = format!("/api/session/{session_id}/revert");
                    let request = server.request(reqwest::Method::DELETE, &path);
                    let response = server
                        .scoped(request, dir)
                        .await
                        .send()
                        .await
                        .map_err(|e| HarnessError::Protocol(format!("opencode DELETE {path}: {e}")))?;
                    if !response.status().is_success() {
                        return Err(HarnessError::Protocol(format!(
                            "opencode DELETE {path}: {}",
                            response.status()
                        )));
                    }
                }
            }
            TuiAction::Fork => {
                let forked = unwrap_data(
                    server
                        .post_json(&format!("/api/session/{session_id}/fork"), dir, &json!({}))
                        .await?,
                );
                if forked.get("id").and_then(Value::as_str).is_none() {
                    return Err(HarnessError::Protocol(
                        "opencode fork returned no session".into(),
                    ));
                }
            }
        }
        Ok(Vec::new())
    }
}

/// How long a compaction is waited on before the turn settles anyway (the
/// transcript still shows it finish).
const COMPACT_WAIT: Duration = Duration::from_secs(15 * 60);

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or_default()
}

/// Whether the compaction started since `since` is done (or failed).
fn compaction_settled(messages: &Value, since: f64) -> bool {
    messages.as_array().into_iter().flatten().any(|message| {
        message.get("type").and_then(Value::as_str) == Some("compaction")
            && message
                .pointer("/time/created")
                .and_then(Value::as_f64)
                .is_some_and(|created| created >= since)
            && message
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| !matches!(status, "running" | "pending"))
    })
}

/// The newest prompt in a session's message list (newest first or not).
fn last_prompt(messages: &Value) -> Option<(String, String)> {
    messages
        .as_array()?
        .iter()
        .filter(|message| message.get("type").and_then(Value::as_str) == Some("user"))
        .max_by_key(|message| {
            message
                .pointer("/time/created")
                .and_then(Value::as_f64)
                .unwrap_or_default() as i64
        })
        .and_then(|message| {
            Some((
                message.get("id")?.as_str()?.to_owned(),
                message
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            ))
        })
}

/// The shell command a `!command` prompt runs, as the TUI's shell mode.
pub(super) fn shell_command(prompt: &str) -> Option<&str> {
    prompt
        .trim_start()
        .strip_prefix('!')
        .map(str::trim)
        .filter(|command| !command.is_empty())
}

/// Run `command` in the session, and wait for it to exit (or [`SHELL_WAIT`]
/// to run out); the session's shell message shows it and what it printed.
pub(super) async fn run_shell(
    server: &Server,
    session_id: &str,
    dir: Option<&str>,
    command: &str,
) -> Result<Vec<AgentEvent>, HarnessError> {
    let started = now_ms();
    server
        .post_json(
            &format!("/api/session/{session_id}/shell"),
            dir,
            &json!({ "command": command }),
        )
        .await?;
    let deadline = tokio::time::Instant::now() + SHELL_WAIT;
    let mut seen: Option<Value> = None;
    loop {
        let messages = unwrap_data(
            server
                .get_json(&format!("/api/session/{session_id}/message?limit=20"), dir)
                .await?,
        );
        seen = shell_message(&messages, command, started - 2000.0).or(seen);
        let exited = seen
            .as_ref()
            .and_then(|message| message.get("status").and_then(Value::as_str))
            .is_some_and(|status| status != "running");
        if exited || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    // (The session's shell message is its chip, in every client.)
    let _ = seen;
    Ok(Vec::new())
}

/// This run's `shell` message: the newest for `command` since `since`.
fn shell_message(messages: &Value, command: &str, since: f64) -> Option<Value> {
    messages
        .as_array()?
        .iter()
        .filter(|message| {
            message.get("type").and_then(Value::as_str) == Some("shell")
                && message.get("command").and_then(Value::as_str) == Some(command)
                && message
                    .pointer("/time/created")
                    .and_then(Value::as_f64)
                    .is_some_and(|created| created >= since)
        })
        .max_by_key(|message| {
            message
                .pointer("/time/created")
                .and_then(Value::as_f64)
                .unwrap_or_default() as i64
        })
        .cloned()
}

/// A shell message as the command chip a run's would be.
pub(super) fn shell_events(message: Option<&Value>, command: &str) -> Vec<AgentEvent> {
    let id = message
        .and_then(|message| message.get("id").and_then(Value::as_str))
        .unwrap_or("shell")
        .to_owned();
    let mut events = vec![AgentEvent::ToolCall {
        id: id.clone(),
        call: ToolCall::Exec {
            command: command.to_owned(),
        },
    }];
    let Some(message) = message else {
        return events;
    };
    let exit = message.get("exit").and_then(Value::as_i64);
    let output = message
        .pointer("/output/output")
        .and_then(Value::as_str)
        .filter(|output| !output.is_empty())
        .map(|output| cap_text(output, OUTPUT_CAP));
    if message.get("status").and_then(Value::as_str) != Some("running") {
        events.push(AgentEvent::ToolResult {
            id,
            is_error: exit.is_some_and(|code| code != 0),
            output,
            diff: None,
            preview_paths: Vec::new(),
            image_payloads: Vec::new(),
            exit_code: exit.and_then(|code| i32::try_from(code).ok()),
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_action_is_ours_only_where_the_composer_lists_ours() {
        let ours = vec![TuiAction::Undo.command()];
        assert_eq!(TuiAction::of("undo", &ours), Some(TuiAction::Undo));
        // (A command of the server's own by that name stays the server's.)
        let theirs = vec![SlashCommand {
            name: "undo".into(),
            description: "my own undo".into(),
            input_hint: None,
        }];
        assert_eq!(TuiAction::of("undo", &theirs), None);
        assert_eq!(TuiAction::of("review", &ours), None);
    }

    #[test]
    fn a_compaction_is_settled_once_its_message_is_done() {
        let running = json!([{"type": "compaction", "status": "running", "time": {"created": 10}}]);
        assert!(!compaction_settled(&running, 5.0));
        let done = json!([{"type": "compaction", "status": "completed", "time": {"created": 10}}]);
        assert!(compaction_settled(&done, 5.0));
        // (An earlier compaction is not this one.)
        assert!(!compaction_settled(&done, 50.0));
    }

    #[test]
    fn a_bang_prompt_is_a_shell_command() {
        assert_eq!(shell_command("!ls -la"), Some("ls -la"));
        assert_eq!(shell_command("  ! git status "), Some("git status"));
        assert_eq!(shell_command("!"), None);
        assert_eq!(shell_command("hello !"), None);
    }

    #[test]
    fn undo_finds_the_newest_prompt() {
        let messages = json!([
            {"id": "m3", "type": "assistant", "time": {"created": 3}},
            {"id": "m2", "type": "user", "text": "second", "time": {"created": 2}},
            {"id": "m1", "type": "user", "text": "first", "time": {"created": 1}}
        ]);
        assert_eq!(last_prompt(&messages), Some(("m2".into(), "second".into())));
        assert_eq!(last_prompt(&json!([])), None);
    }

    #[test]
    fn a_shell_message_reads_as_a_command_chip() {
        // (Shape captured live from 2.3.0.)
        let message = json!({
            "id": "msg_1", "type": "shell", "command": "echo hi", "status": "exited", "exit": 0,
            "time": {"created": 10, "completed": 11},
            "output": {"output": "hi\n", "cursor": 3, "size": 3, "truncated": false}
        });
        let found = shell_message(&json!([message]), "echo hi", 5.0).unwrap();
        let events = shell_events(Some(&found), "echo hi");
        assert!(
            matches!(&events[0], AgentEvent::ToolCall { call: ToolCall::Exec { command }, .. } if command == "echo hi")
        );
        assert!(matches!(
            &events[1],
            AgentEvent::ToolResult { output: Some(output), exit_code: Some(0), is_error: false, .. } if output == "hi\n"
        ));
        // (An older run of the same command is not this one.)
        assert!(shell_message(&json!([found]), "echo hi", 50.0).is_none());
    }
}
