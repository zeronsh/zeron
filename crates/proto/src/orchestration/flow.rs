//! An orchestrator's flow: its steps (an entry, roles, gates, a finish) and
//! how one leads to the next (always, on a pass, or on a fail that may go
//! back a limited number of times a run). The shape is the "version 2 team"
//! JSON of Orchestrator Builder, the Zeron plugin by Master0fFate, so a team
//! made there runs here; a Gate step (the person's approval) is Zeron's own.
//!
//! The flow is the orchestrator's (`Orchestrator::workflow`); a run of it
//! (`Orchestrator::flow_run`) is a card per step on its board, each step's
//! card carrying where it stands in the flow (`WorkItem::flow`).

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Most steps a flow holds.
pub const MAX_NODES: usize = 32;
/// Most connections a flow holds.
pub const MAX_EDGES: usize = 1024;
/// The most a fail may go back a run.
pub const MAX_RETRIES: u32 = 10;
/// The entry's id.
pub const ENTRY: &str = "1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    #[serde(default = "two")]
    pub version: u32,
    pub name: String,
    /// (Snake case, as the version 2 team JSON has it.)
    #[serde(default, rename = "next_id", alias = "nextId")]
    pub next_id: u64,
    #[serde(default)]
    pub nodes: Vec<FlowNode>,
    #[serde(default)]
    pub edges: Vec<FlowEdge>,
}

fn two() -> u32 {
    2
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowNode {
    pub id: String,
    pub kind: NodeKind,
    pub name: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub instructions: String,
    /// Its thread's harness ("" = the crew's).
    #[serde(default)]
    pub harness: String,
    /// Its thread's model ("" = the crew's).
    #[serde(default)]
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    /// Where a run starts ("chief" in the plugin's shape).
    #[serde(rename = "chief")]
    Entry,
    Planner,
    Worker,
    Validator,
    Auditor,
    Qa,
    Researcher,
    /// The person approves before it goes on.
    Gate,
    Finish,
    #[serde(other)]
    Other,
}

impl NodeKind {
    /// A step a crew thread works (not the entry, a gate or the finish).
    pub fn is_role(self) -> bool {
        !matches!(self, NodeKind::Entry | NodeKind::Gate | NodeKind::Finish | NodeKind::Other)
    }

    /// A step that gets a card on the board.
    pub fn has_card(self) -> bool {
        self.is_role() || self == NodeKind::Gate
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub condition: Condition,
    #[serde(default, rename = "max_retries", alias = "maxRetries", skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Condition {
    #[default]
    Always,
    Pass,
    Fail,
}

/// How a step ended: its report's `result`, or a gate's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlowResult {
    Pass,
    Fail,
}

/// A run of an orchestrator's flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowRun {
    pub id: String,
    /// The flow as it was when the run started (later edits don't change it).
    pub flow: Workflow,
    /// What the run is for, in the person's words.
    pub task: String,
    pub state: RunState,
    /// Each step's card on the board, by step id.
    #[serde(default)]
    pub cards: BTreeMap<String, String>,
    /// How often each fail connection went back, by `from>to`.
    #[serde(default)]
    pub returns: BTreeMap<String, u32>,
    /// The thread the engine started for each role step, by step id: only
    /// its report ends the step.
    #[serde(default)]
    pub threads: BTreeMap<String, String>,
    /// The ask the run waits on, by step id: only its answer leads on.
    #[serde(default)]
    pub asks: BTreeMap<String, String>,
    /// The step now under way (none once it ended).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// Why it stopped, when it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    Running,
    /// Waits on the person (a gate, a return limit reached, a step with no
    /// way on).
    Blocked,
    Finished,
    Stopped,
    #[serde(other)]
    Other,
}

/// Where a card stands in its orchestrator's flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowStep {
    pub run: String,
    pub node: String,
    pub kind: NodeKind,
    /// The flow's name and the step's, for the card's chip.
    pub flow: String,
    pub step: String,
    /// The step's place in the run's order (1 = first).
    pub index: u32,
    /// How many steps the run has (its cards).
    pub of: u32,
}

/// What would stop a flow running, one line each (empty: it can run). The
/// checks of the Flow view (ds/orch_flow.luau's `check`), here so the engine
/// never runs a flow the view would have refused.
pub fn check(flow: &Workflow) -> Vec<String> {
    let mut out = Vec::new();
    if flow.nodes.is_empty() || flow.nodes.len() > MAX_NODES {
        out.push(format!("A flow has 1 to {MAX_NODES} steps"));
    }
    if flow.edges.len() > MAX_EDGES {
        out.push(format!("A flow has at most {MAX_EDGES} connections"));
    }
    let mut nodes: HashMap<&str, &FlowNode> = HashMap::new();
    for n in &flow.nodes {
        if nodes.insert(n.id.as_str(), n).is_some() {
            out.push(format!("Two steps share the id {}", n.id));
        }
        if n.kind == NodeKind::Other {
            out.push(format!("{}: an unknown kind of step", n.name));
        }
    }
    let entries: Vec<&FlowNode> = flow.nodes.iter().filter(|n| n.kind == NodeKind::Entry).collect();
    match entries.as_slice() {
        [e] if e.id == ENTRY => {}
        [] => out.push("Add an entry".into()),
        _ => out.push("One entry, with id 1".into()),
    }
    let mut outs: HashMap<&str, Vec<&FlowEdge>> = HashMap::new();
    let mut ins: HashMap<&str, usize> = HashMap::new();
    let mut seen = HashSet::new();
    for e in &flow.edges {
        let (Some(a), Some(b)) = (nodes.get(e.from.as_str()), nodes.get(e.to.as_str())) else {
            out.push("A connection points to a missing step".into());
            continue;
        };
        if !seen.insert((e.from.as_str(), e.to.as_str())) {
            out.push(format!("{}: a connection is there twice", a.name));
        }
        if e.from == e.to || a.kind == NodeKind::Finish || b.kind == NodeKind::Entry {
            out.push(format!("{}: a connection can't start at the finish or end at the entry", a.name));
        }
        if let Some(r) = e.max_retries
            && (e.condition != Condition::Fail || !(1..=MAX_RETRIES).contains(&r))
        {
            out.push(format!("{}: a return limit is 1 to {MAX_RETRIES}, on a fail", a.name));
        }
        if e.condition == Condition::Fail && b.kind == NodeKind::Finish {
            out.push(format!("{}: a fail can't lead to the finish", a.name));
        }
        outs.entry(e.from.as_str()).or_default().push(e);
        *ins.entry(e.to.as_str()).or_default() += 1;
    }
    // A fail that goes back is limited; a limit only on one that goes back.
    let reaches = |from: &str, to: &str, skip: &FlowEdge| -> bool {
        let mut stack = vec![from];
        let mut been = HashSet::new();
        while let Some(id) = stack.pop() {
            if id == to {
                return true;
            }
            if !been.insert(id) {
                continue;
            }
            for e in outs.get(id).into_iter().flatten() {
                if !std::ptr::eq(*e, skip) {
                    stack.push(e.to.as_str());
                }
            }
        }
        false
    };
    for e in &flow.edges {
        if e.condition != Condition::Fail || !nodes.contains_key(e.from.as_str()) {
            continue;
        }
        let back = reaches(&e.to, &e.from, e);
        let name = &nodes[e.from.as_str()].name;
        if back && e.max_retries.is_none() {
            out.push(format!("{name}: going back on a fail needs a limit"));
        } else if !back && e.max_retries.is_some() {
            out.push(format!("{name}: a limit is only for a fail that goes back"));
        }
    }
    // Every loop has a limited return on it.
    fn walk<'a>(
        id: &'a str,
        outs: &HashMap<&'a str, Vec<&'a FlowEdge>>,
        open: &mut HashSet<&'a str>,
        done: &mut HashSet<&'a str>,
    ) -> bool {
        if open.contains(id) {
            return true;
        }
        if !done.insert(id) {
            return false;
        }
        open.insert(id);
        let mut looped = false;
        for e in outs.get(id).into_iter().flatten() {
            let limited = e.condition == Condition::Fail && e.max_retries.is_some_and(|r| (1..=MAX_RETRIES).contains(&r));
            if !limited && walk(e.to.as_str(), outs, open, done) {
                looped = true;
            }
        }
        open.remove(id);
        looped
    }
    let (mut open, mut done) = (HashSet::new(), HashSet::new());
    if flow.nodes.iter().any(|n| walk(n.id.as_str(), &outs, &mut open, &mut done)) {
        out.push("A loop with no limit on it".into());
    }
    let mut reached = HashSet::new();
    let mut stack = vec![ENTRY];
    while let Some(id) = stack.pop() {
        if nodes.contains_key(id) && reached.insert(id) {
            stack.extend(outs.get(id).into_iter().flatten().map(|e| e.to.as_str()));
        }
    }
    let mut finish = false;
    for n in &flow.nodes {
        let list: &[&FlowEdge] = outs.get(n.id.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        if n.name.trim().is_empty() {
            out.push(format!("Step {}: add a name", n.id));
        }
        if n.kind != NodeKind::Entry && ins.get(n.id.as_str()).copied().unwrap_or(0) == 0 {
            out.push(format!("{}: connect something into it", n.name));
        }
        if n.kind != NodeKind::Finish {
            let pass = list.iter().filter(|e| e.condition == Condition::Pass).count();
            let fail = list.iter().filter(|e| e.condition == Condition::Fail).count();
            let ok = match list.len() {
                0 => {
                    out.push(format!("{}: connect it to what's next", n.name));
                    true
                }
                1 => list[0].condition == Condition::Always,
                2 => n.kind != NodeKind::Entry && pass == 1 && fail == 1,
                _ => false,
            };
            if !ok {
                out.push(format!("{}: one always, or one pass and one fail", n.name));
            }
        }
        if !reached.contains(n.id.as_str()) {
            out.push(format!("{}: not reached from the entry", n.name));
        }
        if n.kind.is_role() && n.instructions.trim().is_empty() {
            out.push(format!("{}: add instructions", n.name));
        }
        if !n.model.is_empty() && n.harness.is_empty() {
            out.push(format!("{}: choose an agent for this model", n.name));
        }
        if n.kind == NodeKind::Finish && reached.contains(n.id.as_str()) {
            finish = true;
        }
    }
    if !finish {
        out.push("Connect a finish".into());
    }
    out
}

/// Where a step goes on `result` (`None`: it has only an always way on, or
/// it wasn't given a result): its connection. `None` when there's no way.
pub fn next<'a>(flow: &'a Workflow, from: &str, result: Option<FlowResult>) -> Option<&'a FlowEdge> {
    let outs: Vec<&FlowEdge> = flow.edges.iter().filter(|e| e.from == from).collect();
    if let [only] = outs.as_slice()
        && only.condition == Condition::Always
    {
        return Some(only);
    }
    let want = match result? {
        FlowResult::Pass => Condition::Pass,
        FlowResult::Fail => Condition::Fail,
    };
    outs.into_iter().find(|e| e.condition == want)
}

/// Whether a step needs a pass or a fail to go on (it has a pass and a fail
/// way on).
pub fn conditional(flow: &Workflow, node: &str) -> bool {
    flow.edges.iter().any(|e| e.from == node && e.condition != Condition::Always)
}

/// The steps that get a card, in the run's order (breadth first from the
/// entry, the way on before a fail's way back).
pub fn order(flow: &Workflow) -> Vec<&FlowNode> {
    let rank = |c: Condition| match c {
        Condition::Always => 0,
        Condition::Pass => 1,
        Condition::Fail => 2,
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = std::collections::VecDeque::from([ENTRY.to_owned()]);
    while let Some(id) = queue.pop_front() {
        let Some(n) = flow.nodes.iter().find(|n| n.id == id) else { continue };
        if !seen.insert(id.clone()) {
            continue;
        }
        if n.kind.has_card() {
            out.push(n);
        }
        let mut next: Vec<&FlowEdge> = flow.edges.iter().filter(|e| e.from == id).collect();
        next.sort_by_key(|e| rank(e.condition));
        queue.extend(next.into_iter().map(|e| e.to.clone()));
    }
    out
}

/// The steps that lead into `node` (its card waits on theirs).
pub fn before<'a>(flow: &'a Workflow, node: &str) -> Vec<&'a str> {
    flow.edges
        .iter()
        .filter(|e| e.to == node && e.condition != Condition::Fail)
        .map(|e| e.from.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, kind: NodeKind, name: &str) -> FlowNode {
        FlowNode {
            id: id.into(),
            kind,
            name: name.into(),
            x: 0.0,
            y: 0.0,
            instructions: if kind.is_role() { format!("Do the {name} part") } else { String::new() },
            harness: String::new(),
            model: String::new(),
        }
    }

    fn edge(from: &str, to: &str, condition: Condition, max_retries: Option<u32>) -> FlowEdge {
        FlowEdge { from: from.into(), to: to.into(), condition, max_retries }
    }

    /// Plan → Build → Review: QA back to the Validator on a fail (3 at most).
    pub(crate) fn plan_build_review() -> Workflow {
        Workflow {
            version: 2,
            name: "Plan → Build → Review".into(),
            next_id: 7,
            nodes: vec![
                node("1", NodeKind::Entry, "Entry"),
                node("2", NodeKind::Planner, "Planner"),
                node("3", NodeKind::Worker, "Worker"),
                node("4", NodeKind::Validator, "Validator"),
                node("6", NodeKind::Qa, "QA"),
                node("5", NodeKind::Finish, "Finish"),
            ],
            edges: vec![
                edge("1", "2", Condition::Always, None),
                edge("2", "3", Condition::Always, None),
                edge("3", "4", Condition::Always, None),
                edge("4", "6", Condition::Always, None),
                edge("6", "4", Condition::Fail, Some(3)),
                edge("6", "5", Condition::Pass, None),
            ],
        }
    }

    #[test]
    fn the_plugins_json_reads_as_a_flow() {
        let json = r#"{"version":2,"name":"T","next_id":7,"nodes":[
            {"id":"1","kind":"chief","name":"Chief","x":300,"y":48,"instructions":"","harness":"","model":""},
            {"id":"2","kind":"qa","name":"QA","x":1,"y":2,"instructions":"Try it","provider":"openai","icon":"<svg/>"},
            {"id":"3","kind":"finish","name":"Finish","x":1,"y":2}],
            "edges":[{"from":"1","to":"2","condition":"always"},{"from":"2","to":"3","condition":"pass"},
            {"from":"2","to":"2","condition":"fail","max_retries":2}]}"#;
        let flow: Workflow = serde_json::from_str(json).unwrap();
        assert_eq!(flow.next_id, 7);
        // (Written back in the same shape.)
        let back = serde_json::to_value(&flow).unwrap();
        assert_eq!(back["next_id"], 7);
        assert_eq!(back["edges"][2]["max_retries"], 2);
        assert_eq!(flow.nodes[0].kind, NodeKind::Entry);
        assert_eq!(flow.edges[2].max_retries, Some(2));
        assert_eq!(serde_json::to_value(NodeKind::Entry).unwrap(), "chief");
    }

    #[test]
    fn plan_build_review_runs() {
        assert!(check(&plan_build_review()).is_empty(), "{:?}", check(&plan_build_review()));
    }

    #[test]
    fn a_loop_back_needs_a_limit() {
        let mut f = plan_build_review();
        f.edges[4].max_retries = None;
        let issues = check(&f);
        assert!(issues.iter().any(|i| i.contains("needs a limit")), "{issues:?}");
        assert!(issues.iter().any(|i| i.contains("loop with no limit")), "{issues:?}");
    }

    #[test]
    fn the_way_on_follows_the_result() {
        let f = plan_build_review();
        assert_eq!(next(&f, "3", None).unwrap().to, "4");
        assert_eq!(next(&f, "6", Some(FlowResult::Pass)).unwrap().to, "5");
        assert_eq!(next(&f, "6", Some(FlowResult::Fail)).unwrap().to, "4");
        assert!(next(&f, "6", None).is_none());
        assert!(conditional(&f, "6") && !conditional(&f, "3"));
    }

    #[test]
    fn steps_in_order_and_what_each_waits_on() {
        let f = plan_build_review();
        let names: Vec<&str> = order(&f).iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["Planner", "Worker", "Validator", "QA"]);
        assert_eq!(before(&f, "4"), ["3"]);
    }
}
