//! Task lifecycle owns the chip; verified child histories supply its live
//! transcript. TaskOutput snapshots are a fallback, never a second replay.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::Value;
use tokio::time::Instant;
use zeron_proto::{AgentEvent, DoneStatus};

use super::normalize::{Normalizer, string, tool_call};
use crate::jsonl_tail::TailReader;

pub(super) const POLL: Duration = Duration::from_millis(250);
const LOCATE_FOR: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(500);
const DRAIN_LIMIT: Duration = Duration::from_secs(5);

pub(super) struct Subagents {
    /// `<Droid's home>/.factory/sessions`.
    root: PathBuf,
    /// The session whose Task calls spawn these children.
    parent: String,
    live: HashMap<String, Child>,
    /// Children whose chips are frozen: availability and settlement repeat.
    settled: HashSet<String>,
}

struct Child {
    /// The Task call whose chip this child fills.
    tool: String,
    transcript: Transcript,
    /// Its settlement on the wire. The child stays live while its history
    /// drains: the last appends can come after it.
    ended: Option<Ended>,
}

#[derive(Clone, Copy)]
struct Ended {
    status: DoneStatus,
    at: Instant,
}

/// Where a child's transcript comes from.
enum Transcript {
    /// Its history file, looked for until the deadline.
    Awaited { until: Instant },
    /// Its history file. Nothing is shown before the file's header binds it
    /// to this parent and Task call.
    History {
        reader: TailReader,
        bound: bool,
        /// Message ids already shown: histories replay.
        shown: HashSet<String>,
        tools: Normalizer,
        grew: Instant,
    },
    /// TaskOutput progress, shown because no bound history had been read
    /// when it arrived. A history found later would repeat it.
    Wire(Wire),
    /// No history to read: an id that names no file, a file that never
    /// appeared, or one bound elsewhere.
    Absent,
}

impl Subagents {
    pub fn new(env: &BTreeMap<String, String>, cwd: &str, parent: &str) -> Self {
        let home = env
            .get("FACTORY_HOME_OVERRIDE")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                crate::model_context::root(
                    "FACTORY_HOME_OVERRIDE",
                    crate::executable::home_or_current_dir(),
                )
            });
        Self {
            // Droid resolves a relative home against its working directory.
            root: Path::new(cwd).join(home).join(".factory/sessions"),
            parent: parent.into(),
            live: HashMap::new(),
            settled: HashSet::new(),
        }
    }

    pub fn pending(&self) -> usize {
        self.live.len()
    }

    pub fn start(&mut self, id: &str, tool: &str, prompt: &str) -> Vec<AgentEvent> {
        if id.is_empty()
            || tool.is_empty()
            || self.live.contains_key(id)
            || self.settled.contains(id)
        {
            return vec![];
        }
        let transcript = if uuid::Uuid::parse_str(id).is_ok() {
            Transcript::Awaited {
                until: Instant::now() + LOCATE_FOR,
            }
        } else {
            Transcript::Absent
        };
        self.live.insert(
            id.into(),
            Child {
                tool: tool.into(),
                transcript,
                ended: None,
            },
        );
        vec![tag(
            tool,
            AgentEvent::UserMessage {
                text: prompt.into(),
            },
        )]
    }

    pub fn progress(&mut self, update: &Value) -> Vec<AgentEvent> {
        let id = string(update, "subagentSessionId");
        let Some(child) = self.live.get_mut(id).filter(|child| child.ended.is_none()) else {
            return vec![];
        };
        let mut events = child.read(&self.root, &self.parent, id);
        events.extend(child.wire(update));
        events
            .into_iter()
            .map(|event| tag(&child.tool, event))
            .collect()
    }

    pub fn settle(&mut self, id: &str, status: &str) -> Vec<AgentEvent> {
        if let Some(child) = self.live.get_mut(id).filter(|child| child.ended.is_none()) {
            child.ended = Some(Ended {
                status: match status {
                    "completed" => DoneStatus::Completed,
                    "cancelled" => DoneStatus::Interrupted,
                    _ => DoneStatus::Errored,
                },
                at: Instant::now(),
            });
        }
        self.poll()
    }

    pub fn poll(&mut self) -> Vec<AgentEvent> {
        let mut events = vec![];
        self.live.retain(|id, child| {
            events.extend(
                child
                    .read(&self.root, &self.parent, id)
                    .into_iter()
                    .map(|event| tag(&child.tool, event)),
            );
            let Some(ended) = child.ended.filter(|ended| child.drained(ended.at)) else {
                return true;
            };
            events.push(tag(&child.tool, done(id, ended.status)));
            self.settled.insert(id.clone());
            false
        });
        events
    }

    /// Freezes every chip: a child still running ends with `status`.
    pub fn close(&mut self, status: DoneStatus) -> Vec<AgentEvent> {
        let mut events = self.poll();
        for (id, child) in self.live.drain() {
            let status = child.ended.map_or(status, |ended| ended.status);
            events.push(tag(&child.tool, done(&id, status)));
            self.settled.insert(id);
        }
        events
    }
}

impl Child {
    /// What the history gained since the last read.
    fn read(&mut self, root: &Path, parent: &str, id: &str) -> Vec<AgentEvent> {
        if let Transcript::Awaited { until } = self.transcript {
            // Like Claude's typed forks, cwd directory names are vendor-owned.
            let file = std::fs::read_dir(root)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path().join(format!("{id}.jsonl")))
                .find(|path| path.is_file());
            self.transcript = match file {
                Some(path) => Transcript::History {
                    reader: TailReader::new(path),
                    bound: false,
                    shown: HashSet::new(),
                    tools: Normalizer::default(),
                    grew: Instant::now(),
                },
                None if Instant::now() > until => Transcript::Absent,
                None => return vec![],
            };
        }
        let Transcript::History {
            reader,
            bound,
            shown,
            tools,
            grew,
        } = &mut self.transcript
        else {
            return vec![];
        };
        let before = reader.position();
        let records = reader.read_new();
        if reader.position() != before {
            *grew = Instant::now();
        }
        let mut events = vec![];
        for record in records {
            if !*bound {
                if record["type"] == "session_start" {
                    if record["id"] != id
                        || record["callingSessionId"] != parent
                        || record["callingToolUseId"] != self.tool
                    {
                        self.transcript = Transcript::Absent;
                        return vec![];
                    }
                    *bound = true;
                }
                continue;
            }
            let message_id = string(&record, "id");
            if record["type"] != "message"
                || message_id.is_empty()
                || !shown.insert(message_id.into())
            {
                continue;
            }
            let message = &record["message"];
            for block in message["content"].as_array().into_iter().flatten() {
                events.extend(match (string(message, "role"), string(block, "type")) {
                    ("assistant", "text") if !string(block, "text").is_empty() => {
                        Some(AgentEvent::TextDelta {
                            text: format!("{}\n\n", string(block, "text")),
                        })
                    }
                    ("assistant", "thinking") if !string(block, "thinking").is_empty() => {
                        Some(AgentEvent::ReasoningDelta {
                            text: string(block, "thinking").into(),
                        })
                    }
                    ("assistant", "tool_use") => tools.tool_call(block),
                    ("user", "tool_result") => tools.tool_result(
                        string(block, "tool_use_id"),
                        &block["content"],
                        block["is_error"].as_bool().unwrap_or(false),
                    ),
                    // User text includes private context injections, not just
                    // the Task prompt already shown at launch.
                    _ => None,
                });
            }
        }
        events
    }

    /// TaskOutput progress, shown unless a bound history already is.
    fn wire(&mut self, update: &Value) -> Vec<AgentEvent> {
        match &mut self.transcript {
            Transcript::History { bound: true, .. } => vec![],
            Transcript::Wire(wire) => wire.progress(update),
            _ => {
                let mut wire = Wire::default();
                let events = wire.progress(update);
                if !events.is_empty() {
                    self.transcript = Transcript::Wire(wire);
                }
                events
            }
        }
    }

    /// Whether a child that settled at `ended` has nothing more to show.
    /// Its history may still gain its last lines.
    fn drained(&self, ended: Instant) -> bool {
        let quiet_since = match &self.transcript {
            Transcript::Wire(_) | Transcript::Absent => return true,
            Transcript::Awaited { .. } => ended,
            Transcript::History { grew, .. } => ended.max(*grew),
        };
        quiet_since.elapsed() >= QUIET || ended.elapsed() >= DRAIN_LIMIT
    }
}

/// TaskOutput progress as a transcript. Its updates repeat while the output
/// is being read, its messages are cumulative snapshots, and its tool calls
/// carry no id.
#[derive(Default)]
struct Wire {
    last: Value,
    message: String,
    open: Vec<WireTool>,
    tools: usize,
}

struct WireTool {
    name: String,
    parameters: Value,
    id: String,
}

impl Wire {
    fn progress(&mut self, update: &Value) -> Vec<AgentEvent> {
        if self.last == *update {
            return vec![];
        }
        self.last = update.clone();
        match string(update, "type") {
            "message" => {
                let text = string(update, "text");
                let delta = text
                    .strip_prefix(&self.message)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("\n\n{text}"));
                self.message = text.into();
                if delta.is_empty() {
                    vec![]
                } else {
                    vec![AgentEvent::TextDelta { text: delta }]
                }
            }
            "tool_call" => {
                let name = string(update, "toolName");
                if name.is_empty() {
                    return vec![];
                }
                let id = format!("droid-progress-{}", self.tools);
                self.tools += 1;
                let parameters = update["parameters"].clone();
                let call = tool_call(name, &parameters);
                self.open.push(WireTool {
                    name: name.into(),
                    parameters,
                    id: id.clone(),
                });
                vec![AgentEvent::ToolCall { id, call }]
            }
            "tool_result" => {
                let Some(index) = self.open.iter().position(|tool| {
                    tool.name == string(update, "toolName")
                        && tool.parameters == update["parameters"]
                }) else {
                    return vec![];
                };
                let output = update["fullOutput"]
                    .as_str()
                    .or_else(|| update["details"].as_str())
                    .unwrap_or_else(|| string(update, "text"));
                vec![AgentEvent::ToolResult {
                    id: self.open.remove(index).id,
                    is_error: matches!(string(update, "status"), "error" | "failed"),
                    output: zeron_proto::command_output_cap(output),
                    diff: None,
                    exit_code: update["exitCode"].as_i64().and_then(|n| n.try_into().ok()),
                    preview_paths: Vec::new(),
                    image_payloads: Vec::new(),
                }]
            }
            "error" => vec![AgentEvent::Error {
                message: string(update, "error").into(),
            }],
            _ => vec![],
        }
    }
}

fn tag(tool: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: tool.into(),
        event: Box::new(event),
    }
}

fn done(id: &str, status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: Some(id.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write as _;

    const CHILD: &str = "11111111-1111-4111-8111-111111111111";

    fn feed(root: &Path) -> Subagents {
        Subagents::new(
            &BTreeMap::from([("FACTORY_HOME_OVERRIDE".into(), root.display().to_string())]),
            "/unused",
            "parent",
        )
    }

    fn history(root: &Path, parent: &str, tool: &str) -> std::fs::File {
        let project = root.join(".factory/sessions/worktree");
        std::fs::create_dir_all(&project).unwrap();
        let mut file = std::fs::File::create(project.join(format!("{CHILD}.jsonl"))).unwrap();
        writeln!(file, "{}", json!({"type":"session_start","id":CHILD,"callingSessionId":parent,"callingToolUseId":tool})).unwrap();
        file
    }

    #[test]
    fn droid_child_headers_gate_content_and_path_ids_never_become_paths() {
        for (parent, tool) in [("other-parent", "task"), ("parent", "other-task")] {
            let root = tempfile::tempdir().unwrap();
            let mut feed = feed(root.path());
            let mut file = history(root.path(), parent, tool);
            writeln!(file, "{}", json!({"type":"message","id":"private","message":{"role":"assistant","content":[{"type":"text","text":"FOREIGN"}]}})).unwrap();
            feed.start(CHILD, "task", "prompt");
            assert!(feed.poll().is_empty());
            assert!(feed.start(CHILD, "other-task", "other prompt").is_empty());
            assert!(feed.settle("unknown", "failed").is_empty());
            assert_eq!(feed.pending(), 1);
            assert!(matches!(feed.close(DoneStatus::Interrupted).as_slice(),
                [AgentEvent::Subagent { parent_tool_use_id, event }] if parent_tool_use_id == "task"
                && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Interrupted, .. })));
            assert!(feed.close(DoneStatus::Errored).is_empty());
            assert!(feed.start(CHILD, "task", "prompt").is_empty());
        }
        let root = tempfile::tempdir().unwrap();
        let mut feed = feed(root.path());
        feed.start("../escape", "task", "prompt");
        assert!(feed.poll().is_empty());
        assert!(matches!(
            feed.live["../escape"].transcript,
            Transcript::Absent
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn droid_child_drains_partial_growth_but_not_forever() {
        let root = tempfile::tempdir().unwrap();
        let mut feed = feed(root.path());
        let mut file = history(root.path(), "parent", "task");
        feed.start(CHILD, "task", "prompt");
        feed.poll();
        assert!(feed.settle(CHILD, "completed").is_empty());
        let started = Instant::now();
        while started.elapsed() < DRAIN_LIMIT {
            tokio::time::advance(QUIET).await;
            write!(file, " ").unwrap();
            let events = feed.poll();
            if started.elapsed() < DRAIN_LIMIT {
                assert!(events.is_empty());
                assert_eq!(feed.pending(), 1);
            } else {
                assert!(
                    matches!(events.as_slice(), [AgentEvent::Subagent { event, .. }]
                    if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. }))
                );
            }
        }
        assert_eq!(feed.pending(), 0);
        assert!(feed.poll().is_empty());
    }

    #[test]
    fn droid_wire_fallback_correlates_tools_and_ignores_transcript_replays() {
        let root = tempfile::tempdir().unwrap();
        let mut feed = feed(root.path());
        feed.start(CHILD, "task", "prompt");
        let update = |kind, name, path| json!({"type":kind,"subagentSessionId":CHILD,"toolName":name,"parameters":{"file_path":path}});
        let read = update("tool_call", "Read", "a.txt");
        assert!(
            matches!(feed.progress(&read).as_slice(), [AgentEvent::Subagent { event, .. }]
            if matches!(event.as_ref(), AgentEvent::ToolCall { id, .. } if id == "droid-progress-0"))
        );
        assert!(feed.progress(&read).is_empty());
        feed.progress(&update("tool_call", "Read", "b.txt"));
        let mut result = update("tool_result", "Read", "b.txt");
        result["fullOutput"] = json!("whole output");
        result["details"] = json!("summary");
        result["status"] = json!("failed");
        assert!(
            matches!(feed.progress(&result).as_slice(), [AgentEvent::Subagent { event, .. }]
            if matches!(event.as_ref(), AgentEvent::ToolResult { id, output:Some(output), is_error:true, .. }
                if id == "droid-progress-1" && output == "whole output"))
        );
        let mut file = history(root.path(), "parent", "task");
        writeln!(file, "{}", json!({"type":"message","id":"m","message":{"role":"assistant","content":[{"type":"text","text":"replay"}]}})).unwrap();
        assert!(feed.poll().is_empty());
        for text in ["Hello", "Hello world", "Hello world"] {
            feed.progress(&json!({"type":"message","subagentSessionId":CHILD,"text":text}));
        }
        let ended = feed.settle(CHILD, "cancelled");
        assert!(
            matches!(ended.as_slice(), [AgentEvent::Subagent { event, .. }]
            if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Interrupted, .. }))
        );
        assert!(feed.progress(&read).is_empty());
    }

    #[test]
    fn droid_status_only_progress_does_not_disable_the_full_transcript() {
        let root = tempfile::tempdir().unwrap();
        let mut feed = feed(root.path());
        feed.start(CHILD, "task", "prompt");
        assert!(
            feed.progress(
                &json!({"type":"status","subagentSessionId":CHILD,"text":"Reading task output"})
            )
            .is_empty()
        );
        let mut file = history(root.path(), "parent", "task");
        writeln!(file, "{}", json!({"type":"message","id":"m","message":{"role":"assistant","content":[{"type":"text","text":"answer"}]}})).unwrap();
        assert_eq!(
            feed.poll(),
            vec![tag(
                "task",
                AgentEvent::TextDelta {
                    text: "answer\n\n".into()
                }
            )]
        );
        assert!(
            feed.progress(&json!({"type":"message","subagentSessionId":CHILD,"text":"answer"}))
                .is_empty()
        );
    }
}
