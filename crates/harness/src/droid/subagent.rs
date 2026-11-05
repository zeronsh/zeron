//! Droid's parent RPC stream carries child lifecycle, not child messages.
//! Read the native child transcript at message granularity, as the Claude
//! fork and Grok drivers do. Private-format failures degrade to chip-only.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::time::Instant;
use zeron_proto::{AgentEvent, DoneStatus};

use super::normalize::{Normalizer, string};
use crate::jsonl_tail::TailReader;

pub(super) const POLL: Duration = Duration::from_millis(250);
const DRAIN_QUIET: Duration = Duration::from_secs(2);
const DRAIN_LIMIT: Duration = Duration::from_secs(10);

pub(super) struct Subagents {
    root: PathBuf,
    children: HashMap<String, Child>,
    ready: Vec<AgentEvent>,
}

struct Child {
    id: String,
    parent: String,
    tool: String,
    reader: Option<TailReader>,
    verified: Option<bool>,
    seen: HashSet<String>,
    norm: Normalizer,
    finish: Option<(DoneStatus, Instant)>,
    last_growth: Instant,
}

impl Subagents {
    pub fn new(env: &BTreeMap<String, String>, cwd: &str) -> Self {
        let home = env
            .get("FACTORY_HOME_OVERRIDE")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("FACTORY_HOME_OVERRIDE").map(PathBuf::from))
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(crate::executable::home_or_current_dir);
        let home = if home.is_absolute() {
            home
        } else {
            Path::new(cwd).join(home)
        };
        Self {
            root: home.join(".factory").join("sessions"),
            children: HashMap::new(),
            ready: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    pub fn is_draining(&self) -> bool {
        self.children.values().any(|child| child.finish.is_some())
    }

    pub fn start(&mut self, id: &str, parent: &str, tool: &str) {
        // The wire's id becomes a filename, never an arbitrary path.
        if uuid::Uuid::parse_str(id).is_err() || tool.is_empty() {
            return;
        }
        if self
            .children
            .get(id)
            .is_some_and(|child| child.tool == tool)
        {
            return;
        }
        if let Some(mut previous) = self.children.remove(id) {
            self.ready.extend(previous.read(&self.root));
            self.ready.push(
                previous.done(
                    previous
                        .finish
                        .map_or(DoneStatus::Interrupted, |(status, _)| status),
                ),
            );
        }
        self.children.insert(
            id.into(),
            Child {
                id: id.into(),
                parent: parent.into(),
                tool: tool.into(),
                reader: None,
                verified: None,
                seen: HashSet::new(),
                norm: Normalizer::default(),
                finish: None,
                last_growth: Instant::now(),
            },
        );
    }

    pub fn finish(&mut self, id: &str, status: &str) {
        if let Some(child) = self.children.get_mut(id)
            && child.finish.is_none()
        {
            let status = match status {
                "completed" => DoneStatus::Completed,
                "cancelled" | "canceled" | "killed" | "stopped" | "interrupted" => {
                    DoneStatus::Interrupted
                }
                _ => DoneStatus::Errored,
            };
            // Lifecycle can beat the last transcript append.
            let now = Instant::now();
            child.finish = Some((status, now));
            child.last_growth = now;
        }
    }

    pub fn permission(
        &mut self,
        params: &Value,
        item: &Value,
    ) -> Option<Result<Vec<AgentEvent>, &'static str>> {
        let ids = params["associatedSessionIds"].as_array()?;
        let child = ids
            .iter()
            .filter_map(Value::as_str)
            .find_map(|id| self.children.get(id).map(|_| id))?;
        let child = self.children.get_mut(child)?;
        Some(child.norm.permission(item).map(|events| {
            events
                .into_iter()
                .map(|event| tag(&child.tool, event))
                .collect()
        }))
    }

    pub fn poll(&mut self) -> Vec<AgentEvent> {
        let mut events = std::mem::take(&mut self.ready);
        self.children.retain(|_, child| {
            events.extend(child.read(&self.root));
            if let Some((status, at)) = child.finish
                && (Instant::now().duration_since(child.last_growth) >= DRAIN_QUIET
                    || Instant::now().duration_since(at) >= DRAIN_LIMIT)
            {
                events.push(child.done(status));
                false
            } else {
                true
            }
        });
        events
    }

    pub fn close(&mut self, status: DoneStatus) -> Vec<AgentEvent> {
        let mut events = self.poll();
        for (_, mut child) in self.children.drain() {
            events.extend(child.read(&self.root));
            events.push(child.done(child.finish.map_or(status, |(status, _)| status)));
        }
        events
    }
}

impl Child {
    fn read(&mut self, root: &Path) -> Vec<AgentEvent> {
        if self.reader.is_none() {
            // One level only: cwd encodings and child worktrees can differ.
            self.reader = std::fs::read_dir(root)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path().join(format!("{}.jsonl", self.id)))
                .find(|path| path.is_file())
                .map(TailReader::new);
        }
        let records = if let Some(reader) = self.reader.as_mut() {
            let position = reader.position();
            let records = reader.read_new();
            if reader.position() != position {
                self.last_growth = Instant::now();
            }
            records
        } else {
            Vec::new()
        };
        let mut events = Vec::new();
        for record in records {
            if self.verified.is_none() && record["type"] == "session_start" {
                self.verified = Some(
                    record["id"] == self.id
                        && record["callingSessionId"] == self.parent
                        && record["callingToolUseId"] == self.tool,
                );
            }
            if self.verified != Some(true) || record["type"] != "message" {
                continue;
            }
            let id = string(&record, "id");
            if id.is_empty() || !self.seen.insert(id.into()) {
                continue;
            }
            let message = &record["message"];
            for block in message["content"].as_array().into_iter().flatten() {
                match (string(message, "role"), string(block, "type")) {
                    ("assistant", "text") if !string(block, "text").is_empty() => {
                        events.push(AgentEvent::TextDelta {
                            text: format!("{}\n\n", string(block, "text")),
                        });
                    }
                    // Never render encrypted thinking, prompts, or context injections.
                    ("assistant", "thinking") if !string(block, "thinking").is_empty() => {
                        events.push(AgentEvent::ReasoningDelta {
                            text: format!("{}\n\n", string(block, "thinking")),
                        });
                    }
                    ("assistant", "tool_use")
                        if !string(block, "id").is_empty() && !string(block, "name").is_empty() =>
                    {
                        self.norm.upsert(block);
                        // Plan permission details remain authoritative, as on the wire.
                        if block["name"] != "ExitSpecMode" {
                            events.extend(self.norm.flush(string(block, "id")));
                        }
                    }
                    ("user", "tool_result") if !string(block, "tool_use_id").is_empty() => {
                        events.extend(self.norm.map(&json!({
                            "type":"tool_result", "toolUseId":block["tool_use_id"],
                            "content":block["content"], "isError":block["is_error"],
                        })));
                    }
                    _ => {}
                }
            }
        }
        events
            .into_iter()
            .map(|event| tag(&self.tool, event))
            .collect()
    }

    fn done(&self, status: DoneStatus) -> AgentEvent {
        tag(
            &self.tool,
            AgentEvent::Done {
                status,
                result: None,
                error: None,
                session_id: Some(self.id.clone()),
            },
        )
    }
}

fn tag(tool: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: tool.into(),
        event: Box::new(event),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    const CHILD: &str = "11111111-1111-4111-8111-111111111111";

    fn tracker(root: &Path) -> Subagents {
        Subagents::new(
            &BTreeMap::from([("FACTORY_HOME_OVERRIDE".into(), root.display().to_string())]),
            "/unused",
        )
    }

    fn history(root: &Path, parent: &str) -> std::fs::File {
        let project = root.join(".factory/sessions/another-worktree");
        std::fs::create_dir_all(&project).unwrap();
        let mut file = std::fs::File::create(project.join(format!("{CHILD}.jsonl"))).unwrap();
        writeln!(file, "{}", json!({"type":"session_start","id":CHILD,"callingSessionId":parent,"callingToolUseId":"spawn"})).unwrap();
        file
    }

    #[tokio::test(start_paused = true)]
    async fn verifies_identity_deduplicates_messages_and_settles_once() {
        let root = tempfile::tempdir().unwrap();
        let mut tracker = tracker(root.path());
        assert_eq!(tracker.root, root.path().join(".factory/sessions"));
        tracker.start(CHILD, "parent", "spawn");
        assert!(tracker.poll().is_empty());
        let mut file = history(root.path(), "parent");
        let record = json!({"type":"message","id":"m","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}});
        writeln!(file, "{record}\n{record}").unwrap();
        assert!(
            matches!(tracker.poll().as_slice(), [AgentEvent::Subagent { event, .. }] if matches!(event.as_ref(), AgentEvent::TextDelta { text } if text == "hello\n\n"))
        );
        assert!(tracker.poll().is_empty());
        tracker.finish("unknown-child", "failed");
        tokio::time::advance(DRAIN_QUIET).await;
        assert!(tracker.poll().is_empty());
        assert!(!tracker.is_empty());
        tracker.finish(CHILD, "failed");
        tokio::time::advance(DRAIN_QUIET).await;
        assert!(
            matches!(tracker.poll().as_slice(), [AgentEvent::Subagent { event, .. }] if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Errored, .. }))
        );
        assert!(tracker.is_empty());
        assert!(tracker.poll().is_empty());
        tracker.start(CHILD, "parent", "old-spawn");
        tracker.start(CHILD, "parent", "new-spawn");
        assert!(
            matches!(tracker.poll().as_slice(), [AgentEvent::Subagent { parent_tool_use_id, event }] if parent_tool_use_id == "old-spawn" && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Interrupted, .. }))
        );
        tracker.finish(CHILD, "cancelled");
        tokio::time::advance(DRAIN_QUIET).await;
        assert!(
            matches!(tracker.poll().as_slice(), [AgentEvent::Subagent { parent_tool_use_id, event }] if parent_tool_use_id == "new-spawn" && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Interrupted, .. }))
        );
        assert!(tracker.is_empty());
    }

    #[test]
    fn foreign_histories_and_path_ids_never_render() {
        let root = tempfile::tempdir().unwrap();
        let mut tracker = tracker(root.path());
        tracker.start("../foreign", "parent", "spawn");
        assert!(tracker.is_empty());
        tracker.start(CHILD, "parent", "spawn");
        assert!(
            !tracker.is_draining(),
            "orphan histories must not hold the exit gate"
        );
        let mut file = history(root.path(), "foreign");
        writeln!(file, "{}", json!({"type":"message","id":"m","message":{"role":"assistant","content":[{"type":"text","text":"FOREIGN"}]}})).unwrap();
        assert!(tracker.poll().is_empty());
        assert!(
            matches!(tracker.close(DoneStatus::Interrupted).as_slice(), [AgentEvent::Subagent { event, .. }] if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Interrupted, .. }))
        );
        assert!(tracker.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn drain_waits_for_partial_growth_and_has_a_hard_limit() {
        let root = tempfile::tempdir().unwrap();
        let mut tracker = tracker(root.path());
        let mut file = history(root.path(), "parent");
        tracker.start(CHILD, "parent", "spawn");
        assert!(tracker.poll().is_empty());
        tracker.finish(CHILD, "completed");
        assert!(tracker.is_draining());
        tokio::time::advance(Duration::from_millis(1500)).await;
        write!(file, "{{\"type\":\"message\",\"id\":\"late\",").unwrap();
        assert!(
            tracker.poll().is_empty(),
            "partial bytes extend the quiet period"
        );
        tokio::time::advance(Duration::from_millis(1500)).await;
        writeln!(file, "\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"late\"}}]}}}}").unwrap();
        assert!(
            matches!(tracker.poll().as_slice(), [AgentEvent::Subagent { event, .. }] if matches!(event.as_ref(), AgentEvent::TextDelta { text } if text == "late\n\n"))
        );
        tokio::time::advance(DRAIN_QUIET).await;
        assert!(
            matches!(tracker.poll().as_slice(), [AgentEvent::Subagent { event, .. }] if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. }))
        );
        assert!(tracker.is_empty());

        tracker.start(CHILD, "parent", "spawn");
        tracker.poll();
        tracker.finish(CHILD, "completed");
        let start = Instant::now();
        while Instant::now().duration_since(start) < DRAIN_LIMIT {
            tokio::time::advance(Duration::from_secs(1)).await;
            write!(file, " ").unwrap();
            let events = tracker.poll();
            if Instant::now().duration_since(start) < DRAIN_LIMIT {
                assert!(events.is_empty(), "new bytes keep the drain open");
                assert!(!tracker.is_empty());
            } else {
                assert!(
                    matches!(events.as_slice(), [AgentEvent::Subagent { event, .. }] if matches!(event.as_ref(), AgentEvent::Done { .. }))
                );
            }
        }
        assert!(
            tracker.is_empty(),
            "continuous growth cannot hold the runtime forever"
        );
    }

    #[test]
    fn relayed_permissions_belong_to_the_child_and_keep_authoritative_plans() {
        let root = tempfile::tempdir().unwrap();
        let mut tracker = tracker(root.path());
        tracker.start(CHILD, "parent", "spawn");
        let params = json!({"associatedSessionIds":["parent",CHILD]});
        let item = json!({"toolUse":{"id":"plan","name":"ExitSpecMode","input":{"plan":"preliminary"}},"confirmationType":"exit_spec_mode","details":{"plan":"authoritative","title":"Child plan"}});
        let events = tracker.permission(&params, &item).unwrap().unwrap();
        assert!(
            matches!(events.as_slice(), [AgentEvent::Subagent { parent_tool_use_id, event }] if parent_tool_use_id == "spawn" && matches!(event.as_ref(), AgentEvent::ToolCall { call, .. } if serde_json::to_string(call).unwrap().contains("authoritative")))
        );
        assert!(
            tracker
                .permission(&params, &item)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(tracker.permission(&json!({}), &item).is_none());
    }
}
