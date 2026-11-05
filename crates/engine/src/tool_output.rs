//! `ReadToolOutput {chatId, toolId, offset}`: one command's whole output,
//! read from the run journal of the device that ran it.
//!
//! The chat's doc keeps only a command's last few lines
//! ([`zeron_doc::command_output_tail`]); the whole output never syncs
//! (docs/chat2-sync.md A1). A reader who expands a command asks its host for
//! the rest, and the host answers from its own journal:
//!
//! - only a command's (`ToolCall::Exec`) output, never another tool's (file
//!   reads, searches, fetches and MCP results stay on the host);
//! - made safe to leave the host exactly as the doc's tail is
//!   ([`zeron_doc::command_output_end`]): redacted before it is cut, and a
//!   command that prints secrets by nature answers only the withheld note;
//! - its last [`TOOL_OUTPUT_MAX_BYTES`], from a line's start, `trimmed` when
//!   earlier output was let go;
//! - in [`TOOL_OUTPUT_CHUNK_BYTES`] pieces, each ending on a character,
//!   asked for again from `nextOffset` until `done`.
//!
//! A subagent's doc (`{chat}--sub--{id}`) reads its root chat's journal,
//! where the subagent's tools are recorded. A chat whose journal is gone (an
//! old one, or one imported without it) answers [`ToolOutputError::NotAvailable`].

use zeron_proto::{TOOL_OUTPUT_CHUNK_BYTES, TOOL_OUTPUT_MAX_BYTES, ToolCall, ToolOutputChunk};

use crate::run_journal::{JournalError, RunJournal};

#[derive(Debug, thiserror::Error)]
pub enum ToolOutputError {
    #[error("Only a command's output can be shown in full.")]
    NotACommand,
    #[error("This command has no output yet.")]
    NotFinished,
    #[error("This command's whole output isn't kept on the device that ran it.")]
    NotAvailable,
    #[error("That offset isn't inside the output.")]
    BadOffset,
    #[error(transparent)]
    Journal(#[from] JournalError),
}

/// The chat whose journal records `doc_id`'s tools: a subagent's doc (and a
/// nested one's) is named after its root chat.
pub fn journal_chat(doc_id: &str) -> &str {
    doc_id
        .split_once("--sub--")
        .map_or(doc_id, |(chat, _)| chat)
}

/// One chunk of the whole output of tool `tool_id` in `doc_id`, from byte
/// `offset` of the redacted text.
pub fn read(
    journal: &RunJournal,
    doc_id: &str,
    tool_id: &str,
    offset: u64,
) -> Result<ToolOutputChunk, ToolOutputError> {
    let exchange = journal
        .tool_exchange(journal_chat(doc_id), tool_id)?
        .ok_or(ToolOutputError::NotAvailable)?;
    let ToolCall::Exec { command } = &exchange.call else {
        return Err(ToolOutputError::NotACommand);
    };
    let result = exchange.result.ok_or(ToolOutputError::NotFinished)?;
    let whole = result.output.as_deref().and_then(|output| {
        zeron_doc::command_output_end(command, output, usize::MAX, TOOL_OUTPUT_MAX_BYTES)
    });
    let (text, lines, trimmed) = match whole {
        Some(end) => (end.text, end.lines, end.truncated),
        // It printed nothing: an empty, finished answer.
        None => (String::new(), 0, false),
    };
    let start = usize::try_from(offset).map_err(|_| ToolOutputError::BadOffset)?;
    if start > text.len() || !text.is_char_boundary(start) {
        return Err(ToolOutputError::BadOffset);
    }
    let mut end = (start + TOOL_OUTPUT_CHUNK_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok(ToolOutputChunk {
        text: text[start..end].to_owned(),
        next_offset: end as u64,
        done: end == text.len(),
        bytes: text.len() as u64,
        lines,
        trimmed,
        exit_code: result.exit_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{AgentEvent, DoneStatus};

    fn call(id: &str, call: ToolCall) -> AgentEvent {
        AgentEvent::ToolCall {
            id: id.into(),
            call,
        }
    }

    fn exec(id: &str, command: &str) -> AgentEvent {
        call(
            id,
            ToolCall::Exec {
                command: command.into(),
            },
        )
    }

    fn result(id: &str, output: &str, exit_code: Option<i32>) -> AgentEvent {
        AgentEvent::ToolResult {
            id: id.into(),
            is_error: exit_code.is_some_and(|c| c != 0),
            output: Some(output.into()),
            diff: None,
            exit_code,
        }
    }

    fn journal(events: &[AgentEvent]) -> (tempfile::TempDir, RunJournal) {
        let dir = tempfile::tempdir().unwrap();
        let journal = RunJournal::open(dir.path()).unwrap();
        for event in events {
            journal.append("chat-1", event).unwrap();
        }
        (dir, journal)
    }

    /// Every chunk from offset 0 until `done`, joined.
    fn read_all(journal: &RunJournal, doc: &str, tool: &str) -> (String, ToolOutputChunk, usize) {
        let mut text = String::new();
        let mut offset = 0;
        let mut chunks = 0;
        loop {
            let chunk = read(journal, doc, tool, offset).unwrap();
            chunks += 1;
            text.push_str(&chunk.text);
            offset = chunk.next_offset;
            if chunk.done {
                return (text, chunk, chunks);
            }
            assert!(chunks < 1000, "no end");
        }
    }

    #[test]
    fn a_commands_whole_output_round_trips_from_the_journal() {
        let output: String = (1..=300)
            .map(|i| format!("test case_{i} ... ok\n"))
            .collect();
        let (_dir, journal) = journal(&[
            exec("t1", "cargo test"),
            AgentEvent::TextDelta {
                text: "t1 is mentioned here".into(),
            },
            result("t1", &output, Some(0)),
            AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            },
        ]);
        let (text, last, chunks) = read_all(&journal, "chat-1", "t1");
        assert_eq!(chunks, 1);
        assert_eq!(text, output.trim_end());
        assert_eq!(last.lines, 300);
        assert_eq!(last.bytes, text.len() as u64);
        assert_eq!(last.exit_code, Some(0));
        assert!(!last.trimmed);
        // The doc kept ten of those lines and said how many there are.
        let tail = zeron_doc::command_output_tail("cargo test", &output).unwrap();
        assert_eq!(tail.text.lines().count(), zeron_doc::COMMAND_TAIL_MAX_LINES);
        assert_eq!(tail.lines, last.lines);
    }

    #[test]
    fn the_newest_call_and_result_win_and_subagent_tools_are_found() {
        let (_dir, journal) = journal(&[
            exec("t1", "make"),
            result("t1", "first", Some(2)),
            // A harness refreshed the same call; the last record is its own.
            exec("t1", "make all"),
            result("t1", "second", Some(0)),
            AgentEvent::Subagent {
                parent_tool_use_id: "spawn".into(),
                event: Box::new(exec("s1", "ls")),
            },
            AgentEvent::Subagent {
                parent_tool_use_id: "spawn".into(),
                event: Box::new(result("s1", "a.rs\nb.rs\n", Some(0))),
            },
        ]);
        let chunk = read(&journal, "chat-1", "t1", 0).unwrap();
        assert_eq!((chunk.text.as_str(), chunk.exit_code), ("second", Some(0)));
        // A subagent's doc reads its root chat's journal.
        let chunk = read(&journal, "chat-1--sub--spawn", "s1", 0).unwrap();
        assert_eq!(chunk.text, "a.rs\nb.rs");
        assert_eq!(journal_chat("chat-1--sub--spawn--sub--x"), "chat-1");
    }

    #[test]
    fn the_output_is_redacted_before_it_leaves_the_host() {
        let output = "cloning https://bob:hunter2@example.com/repo.git\n\
                      GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789\n\
                      curl -H 'Authorization: Bearer sk-ant-api03-abcdefghijklmnop'\n\
                      -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n\
                      done\n";
        let (_dir, journal) = journal(&[exec("t1", "./deploy.sh"), result("t1", output, Some(0))]);
        let (text, _, _) = read_all(&journal, "chat-1", "t1");
        for secret in ["hunter2", "ghp_abcdef", "sk-ant-api03", "b3BlbnNzaC1rZXkt"] {
            assert!(!text.contains(secret), "{secret} survived: {text}");
        }
        assert!(text.contains("[private key redacted]"), "{text}");
        assert!(text.ends_with("done"));
        // Exactly what the doc's tail would keep of it.
        assert_eq!(
            text,
            zeron_proto::redact::redact_command_output(output).trim_end()
        );
    }

    #[test]
    fn a_command_that_prints_secrets_answers_only_the_withheld_note() {
        let (_dir, journal) = journal(&[
            exec("t1", "/usr/bin/zsh -lc 'cat .env.local'"),
            result(
                "t1",
                "STRIPE_KEY=sk_live_abc\nDB=postgres://u:p@h/db\n",
                Some(0),
            ),
        ]);
        let chunk = read(&journal, "chat-1", "t1", 0).unwrap();
        assert_eq!(chunk.text, zeron_proto::redact::SECRET_OUTPUT_WITHHELD);
        assert!(chunk.done && !chunk.trimmed);
        assert_eq!(chunk.exit_code, Some(0));
    }

    #[test]
    fn another_tools_output_is_refused() {
        let (_dir, journal) = journal(&[
            call(
                "r1",
                ToolCall::ReadFile {
                    path: "/w/.env".into(),
                },
            ),
            result("r1", "SECRET=1", None),
        ]);
        assert!(matches!(
            read(&journal, "chat-1", "r1", 0),
            Err(ToolOutputError::NotACommand)
        ));
    }

    #[test]
    fn a_big_output_comes_in_chunks_ending_on_characters_and_keeps_its_last_megabyte() {
        // Multi-byte text, longer than the cap: its end, from a line's start.
        let line = format!("{}\n", "é ".repeat(60));
        let output = line.repeat(TOOL_OUTPUT_MAX_BYTES / line.len() + 500);
        let (_dir, journal) = journal(&[exec("t1", "yes é"), result("t1", &output, Some(0))]);
        let first = read(&journal, "chat-1", "t1", 0).unwrap();
        assert!(!first.done && first.text.len() <= TOOL_OUTPUT_CHUNK_BYTES);
        let (text, last, chunks) = read_all(&journal, "chat-1", "t1");
        assert!(last.trimmed);
        assert!(text.len() <= TOOL_OUTPUT_MAX_BYTES);
        assert_eq!(last.bytes, text.len() as u64);
        assert!(text.starts_with("é "), "starts on a whole line");
        assert!(output.trim_end().ends_with(&text));
        assert!(chunks >= text.len().div_ceil(TOOL_OUTPUT_CHUNK_BYTES));
        // An offset inside a character, or past the end, is refused.
        assert!(matches!(
            read(&journal, "chat-1", "t1", 1),
            Err(ToolOutputError::BadOffset)
        ));
        assert!(matches!(
            read(&journal, "chat-1", "t1", last.bytes + 1),
            Err(ToolOutputError::BadOffset)
        ));
    }

    #[test]
    fn a_missing_journal_or_call_or_result_says_so() {
        let (_dir, journal) = journal(&[exec("t1", "sleep 100")]);
        // An old chat whose journal is gone, or a tool it never recorded.
        assert!(matches!(
            read(&journal, "old-chat", "t1", 0),
            Err(ToolOutputError::NotAvailable)
        ));
        assert!(matches!(
            read(&journal, "chat-1", "t9", 0),
            Err(ToolOutputError::NotAvailable)
        ));
        assert!(matches!(
            read(&journal, "chat-1", "t1", 0),
            Err(ToolOutputError::NotFinished)
        ));
        assert_eq!(
            ToolOutputError::NotAvailable.to_string(),
            "This command's whole output isn't kept on the device that ran it."
        );
    }

    #[test]
    fn a_command_that_printed_nothing_answers_empty_and_done() {
        let (_dir, journal) = journal(&[exec("t1", "true"), result("t1", "  \n", Some(0))]);
        let chunk = read(&journal, "chat-1", "t1", 0).unwrap();
        assert!(chunk.text.is_empty() && chunk.done);
        assert_eq!((chunk.bytes, chunk.lines), (0, 0));
    }
}
