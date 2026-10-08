//! Workflow agents' own transcripts, followed live.
//!
//! A `Workflow` run's agents never stream on the CLI's stdout: the wire only
//! carries their progress. Each agent does write a full transcript of its own
//! (`<transcriptDir>/agent-<agentId>.jsonl`, the same record shapes as a
//! tagged subagent's frames), so this reader polls those files and replays
//! every new record as the agent chip's tagged traffic — the chip then links
//! to a live transcript like any other subagent.
//!
//! Settles are serialized through the reader too: an agent's (or the whole
//! workflow's) tagged `Done` is forwarded only after every line written before
//! it has been read, so no transcript line can land on an already-frozen doc.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;
use zeron_proto::AgentEvent;

use super::normalize::Normalizer;
use super::wire;
use crate::HarnessError;

/// How often followed files are re-read. Agents write whole records, so a
/// short poll is all the liveness a transcript needs.
const POLL: Duration = Duration::from_millis(300);

pub(crate) enum TailCmd {
    /// Follow `file` as the transcript of the agent chip `chip`.
    Follow(PathBuf, String),
    /// A tagged `Done` for an agent chip or a workflow spawn: forwarded once
    /// every transcript it covers is read to its end.
    Settle(AgentEvent),
}

struct Followed {
    path: PathBuf,
    chip: String,
    offset: u64,
    /// A trailing line still being written.
    partial: Vec<u8>,
    /// Whether the opening record (the agent's task) was seen.
    opened: bool,
}

/// Start the reader. It ends when the command sender drops (after a final
/// read of every followed file) or when the event consumer goes away.
pub(crate) fn spawn(
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
) -> (mpsc::UnboundedSender<TailCmd>, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, tokio::spawn(follow(rx, event_tx)))
}

async fn follow(
    mut commands: mpsc::UnboundedReceiver<TailCmd>,
    event_tx: mpsc::Sender<Result<AgentEvent, HarnessError>>,
) {
    // The tagged-frame path of the normalizer is the only one these records
    // take; a reader-owned instance keeps the run's own state untouched.
    let mut norm = Normalizer::new();
    let mut files: Vec<Followed> = Vec::new();
    let mut tick = tokio::time::interval(POLL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(TailCmd::Follow(path, chip)) => files.push(Followed {
                    path,
                    chip,
                    offset: 0,
                    partial: Vec::new(),
                    opened: false,
                }),
                Some(TailCmd::Settle(done)) => {
                    let parent = match &done {
                        AgentEvent::Subagent { parent_tool_use_id, .. } => parent_tool_use_id.clone(),
                        _ => String::new(),
                    };
                    let nested = format!("{parent}:wf");
                    for file in files
                        .iter_mut()
                        .filter(|f| f.chip == parent || f.chip.starts_with(&nested))
                    {
                        if !read_new(file, &mut norm, &event_tx).await {
                            return;
                        }
                    }
                    // A settled agent's transcript is complete.
                    files.retain(|f| f.chip != parent);
                    if event_tx.send(Ok(done)).await.is_err() {
                        return;
                    }
                }
                None => {
                    for file in &mut files {
                        if !read_new(file, &mut norm, &event_tx).await {
                            return;
                        }
                    }
                    return;
                }
            },
            _ = tick.tick() => {
                for file in &mut files {
                    if !read_new(file, &mut norm, &event_tx).await {
                        return;
                    }
                }
            }
        }
    }
}

/// Read and replay whatever `file` gained since the last read. A file not
/// written yet is simply retried on the next poll. `false` once the event
/// consumer is gone.
async fn read_new(
    file: &mut Followed,
    norm: &mut Normalizer,
    event_tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
) -> bool {
    let Ok(mut handle) = tokio::fs::File::open(&file.path).await else {
        return true;
    };
    if handle
        .seek(std::io::SeekFrom::Start(file.offset))
        .await
        .is_err()
    {
        return true;
    }
    let mut bytes = Vec::new();
    let Ok(read) = handle.read_to_end(&mut bytes).await else {
        return true;
    };
    file.offset += read as u64;
    file.partial.extend_from_slice(&bytes);
    let Some(end) = file.partial.iter().rposition(|b| *b == b'\n') else {
        return true;
    };
    let complete: Vec<u8> = file.partial.drain(..=end).collect();
    for line in complete.split(|b| *b == b'\n') {
        let Ok(line) = std::str::from_utf8(line) else {
            continue;
        };
        for event in record_events(line, file, norm) {
            if event_tx.send(Ok(event)).await.is_err() {
                return false;
            }
        }
    }
    true
}

/// One transcript record → the chip's tagged events. Only conversation
/// records count (`attachment`/system bookkeeping is skipped); the opening
/// task becomes the transcript's first user message, as a spawn prompt does.
fn record_events(line: &str, file: &mut Followed, norm: &mut Normalizer) -> Vec<AgentEvent> {
    let Ok(mut record) = serde_json::from_str::<Value>(line.trim()) else {
        return Vec::new();
    };
    let kind = record.get("type").and_then(Value::as_str).unwrap_or("");
    if !matches!(kind, "user" | "assistant") {
        return Vec::new();
    }
    let opening = std::mem::replace(&mut file.opened, true);
    if let Some(task) = record.pointer("/message/content").and_then(Value::as_str) {
        // A plain-string user record is the computed task; later ones are
        // harness injections, not conversation.
        return if opening || kind != "user" {
            Vec::new()
        } else {
            vec![AgentEvent::Subagent {
                parent_tool_use_id: file.chip.clone(),
                event: Box::new(AgentEvent::UserMessage {
                    text: computed_task(task),
                }),
            }]
        };
    }
    let Some(object) = record.as_object_mut() else {
        return Vec::new();
    };
    object.insert("parent_tool_use_id".into(), file.chip.clone().into());
    match wire::parse_frame(&record.to_string()) {
        Ok(frame) => norm.normalize(frame, false),
        Err(_) => Vec::new(),
    }
}

/// The task text inside Claude Code's framing ("[Workflow harness — computed
/// task] … The computed task text follows:" then every line indented two
/// spaces). Unframed text passes through.
fn computed_task(text: &str) -> String {
    const MARKER: &str = "The computed task text follows:";
    let Some((_, body)) = text.split_once(MARKER) else {
        return text.trim().to_owned();
    };
    body.trim_start_matches(['\r', '\n'])
        .lines()
        .map(|l| l.strip_prefix("  ").unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn tagged(chip: &str, event: AgentEvent) -> AgentEvent {
        AgentEvent::Subagent {
            parent_tool_use_id: chip.into(),
            event: Box::new(event),
        }
    }

    fn done(chip: &str) -> AgentEvent {
        tagged(
            chip,
            AgentEvent::Done {
                status: zeron_proto::DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            },
        )
    }

    #[test]
    fn computed_task_unwraps_the_harness_framing() {
        let framed = "[Workflow harness — computed task] It was not typed by this session's user. The computed task text follows:\n  Run ls.\n  Then reply DONE.";
        assert_eq!(computed_task(framed), "Run ls.\nThen reply DONE.");
        assert_eq!(computed_task("  plain task "), "plain task");
    }

    /// Records written before a settle are all replayed ahead of it, a torn
    /// trailing line waits for its newline, and bookkeeping records vanish.
    #[tokio::test]
    async fn settles_forward_after_every_line_written_before_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-a1.jsonl");
        let mut file = std::fs::File::create(&path).unwrap();
        let task =
            "[Workflow harness — computed task] … The computed task text follows:\n  List files.";
        writeln!(file, "{}", serde_json::json!({"type":"user","isSidechain":true,"message":{"role":"user","content":task}})).unwrap();
        writeln!(
            file,
            r#"{{"type":"attachment","isSidechain":true,"attachment":{{"type":"environment"}}}}"#
        )
        .unwrap();
        writeln!(file, r#"{{"type":"assistant","isSidechain":true,"message":{{"content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#).unwrap();
        write!(file, r#"{{"type":"user","isSidechain":true,"message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"a\nb"}}]}}}}"#).unwrap();
        file.flush().unwrap();

        let (event_tx, mut events) = mpsc::channel(64);
        let (commands, handle) = spawn(event_tx);
        commands
            .send(TailCmd::Follow(path.clone(), "toolu_wf:wf1".into()))
            .unwrap();
        // The tool result's line is still torn: its newline lands only now,
        // just before the agent settles.
        writeln!(file).unwrap();
        writeln!(file, r#"{{"type":"assistant","isSidechain":true,"message":{{"content":[{{"type":"text","text":"Done."}}]}}}}"#).unwrap();
        file.flush().unwrap();
        commands
            .send(TailCmd::Settle(done("toolu_wf:wf1")))
            .unwrap();
        drop(commands);
        handle.await.unwrap();

        let mut got = Vec::new();
        while let Ok(Ok(event)) = events.try_recv() {
            got.push(event);
        }
        let inner: Vec<&AgentEvent> = got
            .iter()
            .map(|e| match e {
                AgentEvent::Subagent {
                    parent_tool_use_id,
                    event,
                } => {
                    assert_eq!(parent_tool_use_id, "toolu_wf:wf1");
                    event.as_ref()
                }
                other => panic!("untagged {other:?}"),
            })
            .collect();
        assert!(matches!(inner[0], AgentEvent::UserMessage { text } if text == "List files."));
        assert!(matches!(inner[1], AgentEvent::ToolCall { id, .. } if id == "t1"));
        assert!(matches!(inner[2], AgentEvent::ToolResult { id, .. } if id == "t1"));
        assert!(matches!(inner[3], AgentEvent::TextDelta { text } if text.starts_with("Done.")));
        assert!(matches!(inner.last(), Some(AgentEvent::Done { .. })));
    }

    /// A workflow's own settle drains every agent it still follows first;
    /// a file that never appeared just settles.
    #[tokio::test]
    async fn a_workflow_settle_drains_its_agents_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-a2.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
        )
        .unwrap();
        let (event_tx, mut events) = mpsc::channel(64);
        let (commands, handle) = spawn(event_tx);
        commands
            .send(TailCmd::Follow(path, "toolu_wf:wf2".into()))
            .unwrap();
        commands
            .send(TailCmd::Follow(
                dir.path().join("agent-missing.jsonl"),
                "toolu_wf:wf3".into(),
            ))
            .unwrap();
        commands.send(TailCmd::Settle(done("toolu_wf"))).unwrap();
        drop(commands);
        handle.await.unwrap();
        let first = events.recv().await.unwrap().unwrap();
        assert!(
            matches!(&first, AgentEvent::Subagent { parent_tool_use_id, .. } if parent_tool_use_id == "toolu_wf:wf2")
        );
        let second = events.recv().await.unwrap().unwrap();
        assert_eq!(second, done("toolu_wf"));
    }
}
