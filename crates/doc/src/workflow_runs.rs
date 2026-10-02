//! The chat doc's projection of its workflow runs (`meta.workflowRuns`).
//!
//! Host-only writer. Each run is a map of small JSON strings — `h` is the
//! header, every other key is one entry (`n:<site>#<ordinal>`, `a:…`, `r:…`,
//! `f:…`, `q:…`, `g`) — so a change to one node rewrites one short value
//! instead of the whole run. Readers rebuild a [`WorkflowRunsState`] from the
//! map; unreadable values are skipped (an older or newer build may have written
//! a shape this one does not know), never fatal.

use loro::{LoroMap, ToJson};
use zeron_proto::{
    WorkflowEntry, WorkflowRun, WorkflowRunHeader, WorkflowRunsDelta, WorkflowRunsState,
};

use crate::{DocError, SessionDoc};

const RUNS_KEY: &str = "workflowRuns";
const REV_KEY: &str = "workflowRev";
const HEADER_KEY: &str = "h";

impl SessionDoc {
    /// The revision the last [`Self::apply_workflow_delta`] wrote (cheap:
    /// lets a watcher skip reading the runs when nothing changed).
    pub fn workflow_revision(&self) -> u64 {
        match self.doc().get_map("meta").get(REV_KEY) {
            Some(loro::ValueOrContainer::Value(loro::LoroValue::I64(n))) => n.max(0) as u64,
            _ => 0,
        }
    }

    /// Every run of the chat, rebuilt from the doc.
    pub fn workflow_runs(&self) -> WorkflowRunsState {
        let meta = self.doc().get_map("meta");
        let Some(loro::ValueOrContainer::Container(loro::Container::Map(runs))) =
            meta.get(RUNS_KEY)
        else {
            return WorkflowRunsState {
                revision: self.workflow_revision(),
                runs: Vec::new(),
            };
        };
        let value = runs.get_deep_value().to_json_value();
        let mut out = WorkflowRunsState {
            revision: self.workflow_revision(),
            runs: Vec::new(),
        };
        let Some(by_run) = value.as_object() else {
            return out;
        };
        for entries in by_run.values() {
            let Some(entries) = entries.as_object() else {
                continue;
            };
            let Some(header) = entries
                .get(HEADER_KEY)
                .and_then(|v| v.as_str())
                .and_then(|s| serde_json::from_str::<WorkflowRunHeader>(s).ok())
            else {
                continue;
            };
            let mut run = WorkflowRun {
                header,
                ..WorkflowRun::default()
            };
            for (key, raw) in entries {
                if key == HEADER_KEY {
                    continue;
                }
                if let Some(entry) = raw
                    .as_str()
                    .and_then(|s| serde_json::from_str::<WorkflowEntry>(s).ok())
                {
                    run.upsert(entry);
                }
            }
            run.actors.sort_by_key(|a| a.order);
            run.nodes.sort_by_key(|n| n.order);
            run.reports.sort_by_key(|r| r.index);
            run.artifacts.sort_by(|a, b| a.id.cmp(&b.id));
            run.pending_questions.sort_by_key(|q| q.asked_at);
            out.runs.push(run);
        }
        out.runs.sort_by(|a, b| {
            (a.header.created_at, &a.header.run_id).cmp(&(b.header.created_at, &b.header.run_id))
        });
        out
    }

    /// Write a delta (one commit). Idempotent: every value is replaced whole.
    pub fn apply_workflow_delta(&self, delta: &WorkflowRunsDelta) -> Result<(), DocError> {
        if delta.is_empty() && delta.revision <= self.workflow_revision() {
            return Ok(());
        }
        let meta = self.doc().get_map("meta");
        let runs: LoroMap = meta.ensure_mergeable_map(RUNS_KEY)?;
        for id in &delta.runs_removed {
            if runs.get(id).is_some() {
                runs.delete(id)?;
            }
        }
        for change in &delta.runs {
            let run: LoroMap = runs.ensure_mergeable_map(&change.run_id)?;
            if let Some(header) = &change.header {
                run.insert(HEADER_KEY, serde_json::to_string(header)?)?;
            }
            for key in &change.removed {
                if run.get(key).is_some() {
                    run.delete(key)?;
                }
            }
            for entry in &change.upserts {
                run.insert(&entry.key(), serde_json::to_string(entry)?)?;
            }
        }
        if delta.revision > self.workflow_revision() {
            meta.insert(REV_KEY, delta.revision as i64)?;
        }
        self.doc().commit();
        Ok(())
    }

    /// Replace one run wholesale (startup reconciliation).
    pub fn replace_workflow_run(&self, run: &WorkflowRun) -> Result<(), DocError> {
        let meta = self.doc().get_map("meta");
        let runs: LoroMap = meta.ensure_mergeable_map(RUNS_KEY)?;
        // A mergeable child keeps its content when its key is deleted, so a
        // wholesale replace clears the entries instead.
        let map: LoroMap = runs.ensure_mergeable_map(&run.header.run_id)?;
        map.clear()?;
        map.insert(HEADER_KEY, serde_json::to_string(&run.header)?)?;
        for entry in run.entries() {
            map.insert(&entry.key(), serde_json::to_string(&entry)?)?;
        }
        meta.insert(REV_KEY, (self.workflow_revision() + 1) as i64)?;
        self.doc().commit();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::*;

    fn node(site: &str, order: u32, phase: NodePhase) -> WorkflowNode {
        WorkflowNode {
            order,
            site_id: site.into(),
            ordinal: 0,
            kind: NodeKind::Ask,
            phase,
            outcome: None,
            cached: false,
            actor_site_id: None,
            actor_ordinal: 0,
            phase_name: None,
            instructions_head: String::new(),
            turn: 0,
            tool_calls: 0,
            last_tool: None,
            tokens: 0,
            started_at: None,
            ended_at: None,
            error: None,
            result_preview: None,
        }
    }

    fn header(id: &str) -> WorkflowRunHeader {
        WorkflowRunHeader {
            run_id: id.into(),
            name: "demo".into(),
            chat_id: "c".into(),
            created_at: 5,
            ..Default::default()
        }
    }

    #[test]
    fn deltas_round_trip_through_the_doc_and_replicate() {
        let host = SessionDoc::init("chat").unwrap();
        assert!(host.workflow_runs().runs.is_empty());
        let mut delta = WorkflowRunDelta::for_run("r1");
        delta.header = Some(header("r1"));
        delta
            .upserts
            .push(WorkflowEntry::Node(node("b", 1, NodePhase::Queued)));
        delta
            .upserts
            .push(WorkflowEntry::Node(node("a", 0, NodePhase::Executing)));
        let d = WorkflowRunsDelta {
            revision: 3,
            runs: vec![delta],
            runs_removed: vec![],
        };
        host.apply_workflow_delta(&d).unwrap();
        let state = host.workflow_runs();
        assert_eq!(state.revision, 3);
        let run = state.run("r1").unwrap();
        assert_eq!(
            run.nodes
                .iter()
                .map(|n| n.site_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );

        // One node changes: only its value is rewritten.
        let mut change = WorkflowRunDelta::for_run("r1");
        change
            .upserts
            .push(WorkflowEntry::Node(node("a", 0, NodePhase::Settled)));
        change.removed.push("n:b#0".into());
        host.apply_workflow_delta(&WorkflowRunsDelta {
            revision: 4,
            runs: vec![change],
            runs_removed: vec![],
        })
        .unwrap();
        let state = host.workflow_runs();
        assert_eq!(state.run("r1").unwrap().nodes.len(), 1);
        assert_eq!(state.run("r1").unwrap().nodes[0].phase, NodePhase::Settled);

        let replica = SessionDoc::from_doc(loro::LoroDoc::new());
        replica
            .doc()
            .import(&host.export_snapshot().unwrap())
            .unwrap();
        assert_eq!(replica.workflow_runs(), host.workflow_runs());

        // Replaying the same delta is harmless.
        let before = host.workflow_runs();
        host.apply_workflow_delta(&WorkflowRunsDelta {
            revision: 4,
            runs: vec![],
            runs_removed: vec![],
        })
        .unwrap();
        assert_eq!(host.workflow_runs(), before);

        host.apply_workflow_delta(&WorkflowRunsDelta {
            revision: 5,
            runs: vec![],
            runs_removed: vec!["r1".into()],
        })
        .unwrap();
        assert!(host.workflow_runs().runs.is_empty());
    }

    #[test]
    fn unreadable_values_are_skipped_and_replace_rewrites_a_run() {
        let doc = SessionDoc::init("chat").unwrap();
        let mut run = WorkflowRun {
            header: header("r1"),
            ..Default::default()
        };
        run.nodes.push(node("a", 0, NodePhase::Queued));
        doc.replace_workflow_run(&run).unwrap();
        // A value from a future build that this one cannot decode.
        let runs = doc
            .doc()
            .get_map("meta")
            .ensure_mergeable_map(RUNS_KEY)
            .unwrap();
        let r: LoroMap = runs.ensure_mergeable_map("r1").unwrap();
        r.insert("n:future#0", "{\"kind\":\"hologram\"}").unwrap();
        doc.doc().commit();
        assert_eq!(doc.workflow_runs().run("r1").unwrap().nodes.len(), 1);
        run.nodes[0].phase = NodePhase::Settled;
        doc.replace_workflow_run(&run).unwrap();
        let again = doc.workflow_runs();
        assert_eq!(again.run("r1").unwrap().nodes[0].phase, NodePhase::Settled);
        assert!(again.revision >= 2);
    }

    #[test]
    fn a_rebuilt_thin_doc_keeps_the_runs() {
        let doc = SessionDoc::init("chat").unwrap();
        let mut run = WorkflowRun {
            header: header("r1"),
            ..Default::default()
        };
        run.nodes.push(node("a", 0, NodePhase::Queued));
        doc.replace_workflow_run(&run).unwrap();
        let rebuilt = crate::rebuild_thin_doc(&doc).unwrap().doc;
        assert_eq!(rebuilt.workflow_runs(), doc.workflow_runs());
    }

    #[test]
    fn workflow_commands_round_trip_through_the_ledger() {
        use crate::{
            SessionCommandEntry, SessionCommandKind, SessionCommandPayload, SessionCommandStatus,
        };
        let doc = SessionDoc::init("chat").unwrap();
        let entry = SessionCommandEntry {
            id: "c1".into(),
            payload: SessionCommandPayload::Workflow {
                command: WorkflowCommand::Stop {
                    run_id: "r1".into(),
                    reason: Some("enough".into()),
                },
            },
            issued_by: "dev".into(),
            issued_at: 1,
            based_on: None,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        };
        doc.queue_command(&entry).unwrap();
        let read = doc.read_commands().unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].kind(), SessionCommandKind::Workflow);
        assert_eq!(read[0].payload, entry.payload);
    }
}
