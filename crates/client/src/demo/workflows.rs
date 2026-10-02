//! The workflows demo chat (`DemoFixture::Workflows`): a chat whose agent keeps
//! a checklist, works toward a goal, and runs two workflows — one live, one
//! finished with artifacts — written into its doc the way a host would
//! (`meta.goal`, `meta.workflowRuns`, marker entries). The simulated host
//! also executes goal and workflow commands, so the phone's controls can be
//! tried offline. [`RunSpec`] builds runs of any size for tests.

use zeron_doc::{DocError, MessageRole, SessionDoc, SessionMessageEntry};
use zeron_proto::{
    ActorStatus, ArtifactKind, ArtifactSummary, Goal, GoalCommand, GoalLimits, GoalReasonKind, GoalStatus, GoalVerdict, GraphPhase, MessageOrigin, NodeKind, NodeOutcome,
    NodePhase, TodoItem, TodoStatus, ToolCall, VerdictOutcome, WorkflowActor, WorkflowCommand,
    WorkflowConcurrency, WorkflowEventMarker, WorkflowGraph, WorkflowNode,
    WorkflowPhaseProgress, WorkflowQuestion, WorkflowReport, WorkflowRun, WorkflowRunHeader,
    WorkflowRunsState, WorkflowStatus, WorkflowStopReason, WorkflowUsage,
    workflow_marker_text,
};

use super::transcripts::{PHONE, entry, text, tool};

pub(crate) const CHAT: &str = "chat-workflows";
/// The live run's id and the finished one's.
pub const LIVE_RUN: &str = "run-live";
pub const DONE_RUN: &str = "run-done";

const MIN: i64 = 60_000;

/// One phase of a generated run.
#[derive(Debug, Clone)]
pub struct PhaseSpec {
    pub name: String,
    /// Agents that work in the phase.
    pub agents: usize,
    /// Of them: finished well, then finished badly. The rest are working
    /// (in the run's current phase) or not started (later phases).
    pub done: usize,
    pub failed: usize,
    /// Runs alongside the previous phase.
    pub parallel: bool,
}

impl PhaseSpec {
    pub fn new(name: &str, agents: usize, done: usize, failed: usize) -> Self {
        Self {
            name: name.into(),
            agents,
            done,
            failed,
            parallel: false,
        }
    }
}

/// A run to generate: phases of agents with their progress, plus the extras a
/// card can show.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub id: String,
    pub name: String,
    pub status: WorkflowStatus,
    pub stop_reason: Option<WorkflowStopReason>,
    pub phases: Vec<PhaseSpec>,
    /// An agent waiting on a question for the person.
    pub question: bool,
    /// A markdown report, a table and a metrics artifact.
    pub artifacts: bool,
    pub result: Option<String>,
    pub reports: usize,
    pub at: i64,
}

impl RunSpec {
    pub fn new(id: &str, name: &str, status: WorkflowStatus, phases: Vec<PhaseSpec>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status,
            stop_reason: None,
            phases,
            question: false,
            artifacts: false,
            result: None,
            reports: 0,
            at: 1_700_000_000_000,
        }
    }
}

/// Build the run a [`RunSpec`] describes.
pub fn build_run(spec: &RunSpec) -> WorkflowRun {
    let live = !spec.status.is_settled();
    let mut actors = Vec::new();
    let mut nodes = Vec::new();
    let mut progress = Vec::new();
    // The first phase with unfinished work is where control flow is.
    let current = spec
        .phases
        .iter()
        .position(|p| p.done + p.failed < p.agents);
    for (pi, phase) in spec.phases.iter().enumerate() {
        let mut settled = 0;
        for i in 0..phase.agents {
            let order = actors.len() as u32;
            let site = format!("s{pi}-{i}");
            let name = format!("{}-{}", phase.name, i + 1);
            let state = if i < phase.done {
                (NodePhase::Settled, Some(NodeOutcome::Ok))
            } else if i < phase.done + phase.failed {
                (NodePhase::Settled, Some(NodeOutcome::Failed))
            } else if current == Some(pi) && live {
                (NodePhase::Executing, None)
            } else if current.is_some_and(|c| pi > c) || !live && spec.status != WorkflowStatus::Completed {
                (NodePhase::Queued, None)
            } else {
                (NodePhase::Settled, Some(NodeOutcome::Ok))
            };
            if state.0 == NodePhase::Settled {
                settled += 1;
            }
            let started = state.0 != NodePhase::Queued;
            actors.push(WorkflowActor {
                order,
                site_id: site.clone(),
                ordinal: 0,
                name,
                child_chat_id: started.then(|| format!("{}-child-{order}", spec.id)),
                status: match state.0 {
                    NodePhase::Settled => ActorStatus::Completed,
                    NodePhase::Queued => ActorStatus::Waiting,
                    _ => ActorStatus::Running,
                },
                phase_name: Some(phase.name.clone()),
                harness: Some("claude".into()),
                model: Some("sonnet".into()),
                asks: u32::from(started),
                failed_asks: u32::from(state.1 == Some(NodeOutcome::Failed)),
            });
            nodes.push(WorkflowNode {
                order,
                site_id: site.clone(),
                ordinal: 0,
                kind: NodeKind::Ask,
                phase: state.0,
                outcome: state.1,
                cached: false,
                actor_site_id: Some(site),
                actor_ordinal: 0,
                phase_name: Some(phase.name.clone()),
                instructions_head: format!("Review the {} area for problems", phase.name),
                turn: if state.0 == NodePhase::Executing { 2 } else { 0 },
                tool_calls: if state.0 == NodePhase::Executing { 5 } else { 0 },
                last_tool: (state.0 == NodePhase::Executing).then(|| "Grep".to_owned()),
                tokens: if started { 4_200 } else { 0 },
                started_at: started.then_some(spec.at),
                ended_at: (state.0 == NodePhase::Settled).then_some(spec.at + MIN),
                error: (state.1 == Some(NodeOutcome::Failed)).then(|| "The agent could not read the file.".to_owned()),
                result_preview: (state.1 == Some(NodeOutcome::Ok)).then(|| "No problems found.".to_owned()),
            });
        }
        progress.push(WorkflowPhaseProgress {
            name: phase.name.clone(),
            observed: phase.agents as u32,
            settled,
        });
    }
    let used = nodes.iter().filter(|n| n.phase != NodePhase::Queued).count() as u32;
    let working = nodes.iter().filter(|n| n.phase == NodePhase::Executing).count() as u32;
    let mut artifacts = Vec::new();
    if spec.artifacts {
        let make = |id: &str, kind, title: &str, ct: &str, bytes, items, primary| ArtifactSummary {
            id: id.into(),
            kind,
            title: title.into(),
            version: 1,
            content_type: ct.into(),
            bytes,
            item_count: items,
            primary,
        };
        artifacts.push(make("summary", ArtifactKind::Markdown, "Security review summary", "text/markdown", 1_840, 0, true));
        artifacts.push(make("findings", ArtifactKind::Table, "Findings by severity", "application/json", 612, 4, false));
        artifacts.push(make("coverage", ArtifactKind::Metrics, "Coverage", "application/json", 240, 3, false));
    }
    let mut pending_questions = Vec::new();
    if spec.question
        && let Some(actor) = actors.iter().find(|a| a.status == ActorStatus::Running)
    {
        pending_questions.push(WorkflowQuestion {
            qid: "q1".into(),
            actor_site_id: actor.site_id.clone(),
            actor_ordinal: 0,
            actor_name: actor.name.clone(),
            question: "Should I treat the vendored `third_party/` directory as in scope?".into(),
            context: "It has 3 findings, all in code we do not edit.".into(),
            asked_at: spec.at + 2 * MIN,
        });
    }
    let header = WorkflowRunHeader {
        run_id: spec.id.clone(),
        name: spec.name.clone(),
        chat_id: CHAT.into(),
        status: spec.status,
        stop_reason: spec.stop_reason,
        stop_detail: (spec.stop_reason == Some(WorkflowStopReason::Provider)).then(|| "The provider rejected the key (401).".to_owned()),
        script_hash: "4f9c2a7be01d".into(),
        created_at: spec.at,
        started_at: Some(spec.at),
        ended_at: spec.status.is_settled().then_some(spec.at + 6 * MIN),
        usage: WorkflowUsage {
            input_tokens: 42_000,
            output_tokens: 6_300,
            nodes_used: used,
            nodes_cached: 0,
            elapsed_ms: 192_000,
        },
        resumable: spec.status == WorkflowStatus::Stopped && spec.stop_reason != Some(WorkflowStopReason::Denied),
        result_preview: spec.result.clone(),
        concurrency: WorkflowConcurrency {
            cap: 8,
            ceiling: 8,
            in_flight: working,
            queued: nodes.iter().filter(|n| n.phase == NodePhase::Queued).count() as u32,
            throttled: false,
        },
        phases: progress,
        current_phase: current.map(|i| spec.phases[i].name.clone()),
        phase_names: spec.phases.iter().map(|p| p.name.clone()).collect(),
        phase_alongside: spec
            .phases
            .windows(2)
            .filter(|w| w[1].parallel)
            .map(|w| vec![w[0].name.clone(), w[1].name.clone()])
            .collect(),
        reports_total: spec.reports as u32,
        ..Default::default()
    };
    WorkflowRun {
        header,
        actors,
        nodes,
        reports: (0..spec.reports)
            .map(|i| WorkflowReport {
                index: i as u32,
                text: format!("Checked area {}: nothing needs changing.", i + 1),
                truncated: false,
                artifact_id: None,
                at: spec.at + (i as i64 + 1) * MIN,
            })
            .collect(),
        artifacts,
        pending_questions,
        graph: Some(WorkflowGraph {
            phases: spec
                .phases
                .iter()
                .map(|p| GraphPhase {
                    name: p.name.clone(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }),
    }
}

/// The goal of the demo chat, mid-way: round 3, one verdict in.
pub fn demo_goal(now: i64) -> Goal {
    let mut goal = Goal::new(
        "goal-demo",
        "Make the security review pass: every finding fixed or explained, and the test suite green.",
        &GoalLimits::default(),
        now - 25 * MIN,
    )
    .expect("a valid objective");
    goal.iteration = 3;
    goal.tokens_used = 48_200;
    goal.verifier_tokens_used = 3_100;
    goal.time_used_seconds = 17 * 60;
    goal.verdicts = vec![
        GoalVerdict {
            iteration: 1,
            outcome: VerdictOutcome::NotSatisfied,
            reason: "Two findings are still open.".into(),
            next_action: Some("Fix the path traversal in the upload handler".into()),
            at: now - 14 * MIN,
            verifier_chat_id: None,
            verifier_tokens: Some(1_400),
            verifier_ms: Some(31_000),
        },
        GoalVerdict {
            iteration: 2,
            outcome: VerdictOutcome::NotSatisfied,
            reason: "The fix has no regression test.".into(),
            next_action: Some("Add a regression test for the upload path".into()),
            at: now - 5 * MIN,
            verifier_chat_id: None,
            verifier_tokens: Some(1_700),
            verifier_ms: Some(28_000),
        },
    ];
    goal.pending = None;
    goal
}

/// The two runs of the demo chat.
pub fn demo_runs(now: i64) -> WorkflowRunsState {
    let mut done = RunSpec::new(
        DONE_RUN,
        "dependency-audit",
        WorkflowStatus::Completed,
        vec![
            PhaseSpec::new("collect", 2, 2, 0),
            PhaseSpec::new("compare", 4, 4, 0),
            PhaseSpec::new("report", 1, 1, 0),
        ],
    );
    done.artifacts = true;
    done.reports = 4;
    done.at = now - 3 * 60 * MIN;
    done.result = Some("Audited 212 dependencies. 3 need an upgrade, none are exploitable here. The summary lists each with the version to move to.".into());
    let mut live = RunSpec::new(
        LIVE_RUN,
        "security-review",
        WorkflowStatus::Running,
        vec![
            PhaseSpec::new("scan", 3, 3, 0),
            PhaseSpec::new("review", 8, 4, 1),
            PhaseSpec::new("verify", 3, 0, 0),
            PhaseSpec::new("fix", 2, 0, 0),
            PhaseSpec::new("report", 1, 0, 0),
        ],
    );
    live.question = true;
    live.artifacts = true;
    live.reports = 2;
    live.at = now - 9 * MIN;
    WorkflowRunsState {
        revision: 2,
        runs: vec![build_run(&done), build_run(&live)],
    }
}

fn marker(id: &str, at: i64, origin: MessageOrigin, text_body: String) -> SessionMessageEntry {
    let mut e = entry(id, MessageRole::System, "host", at, vec![text("t0", &text_body)]);
    e.origin = Some(origin);
    e
}

/// The transcript of the demo chat.
pub fn transcript(host: &str, now: i64) -> Vec<SessionMessageEntry> {
    let t = |ago: i64| now - ago * MIN;
    let todo = vec![
        TodoItem::new("Map the attack surface", TodoStatus::Completed),
        TodoItem::new("Run the dependency audit workflow", TodoStatus::Completed),
        TodoItem::new("Fix the path traversal in the upload handler", TodoStatus::InProgress),
        TodoItem::new("Add a regression test for the upload path", TodoStatus::Pending),
        TodoItem::new("Re-run the security review", TodoStatus::Pending),
    ];
    let mut round2 = entry(
        "m-round2",
        MessageRole::User,
        "host",
        t(14),
        vec![text("t0", "Continue toward the goal. Next: fix the path traversal in the upload handler.")],
    );
    round2.origin = Some(MessageOrigin::Goal {
        goal_id: "goal-demo".into(),
        round: 2,
        title: "Fix the path traversal in the upload handler".into(),
    });
    let mut result = entry(
        "m-result",
        MessageRole::User,
        "host",
        t(170),
        vec![text(
            "t0",
            "[Workflow completed] dependency-audit (run run-done)\ncompleted · 7 agents · 7 asks · 48.2k tokens\n<workflow_result>\nAudited 212 dependencies.\n</workflow_result>",
        )],
    );
    result.origin = Some(MessageOrigin::Workflow {
        run_id: DONE_RUN.into(),
        name: "dependency-audit".into(),
        status: WorkflowStatus::Completed,
    });
    vec![
        entry(
            "m1",
            MessageRole::User,
            PHONE,
            t(200),
            vec![text("t0", "Audit our dependencies, then review the whole repo for security problems.")],
        ),
        marker(
            "wf-run-done-start",
            t(180),
            MessageOrigin::WorkflowEvent {
                run_id: DONE_RUN.into(),
                marker: WorkflowEventMarker::Started,
                name: "dependency-audit".into(),
                detail: String::new(),
            },
            workflow_marker_text(WorkflowEventMarker::Started, "dependency-audit", ""),
        ),
        marker(
            "wf-run-done-end",
            t(174),
            MessageOrigin::WorkflowEvent {
                run_id: DONE_RUN.into(),
                marker: WorkflowEventMarker::Completed,
                name: "dependency-audit".into(),
                detail: String::new(),
            },
            workflow_marker_text(WorkflowEventMarker::Completed, "dependency-audit", ""),
        ),
        result,
        entry(
            "m2",
            MessageRole::Assistant,
            host,
            t(60),
            vec![
                tool("td1", ToolCall::Todo { items: todo }, false, None),
                text("t1", "The audit is clean. I set a goal so I keep going until the review passes."),
            ],
        ),
        marker(
            "goal-set",
            t(25),
            MessageOrigin::GoalEvent {
                goal_id: "goal-demo".into(),
                event: zeron_proto::GoalEventKind::Set,
                round: 0,
                title: "Make the security review pass".into(),
                detail: String::new(),
                verifier_chat_id: None,
            },
            "Goal set: Make the security review pass".into(),
        ),
        marker(
            "goal-verdict-1",
            t(14),
            MessageOrigin::GoalEvent {
                goal_id: "goal-demo".into(),
                event: zeron_proto::GoalEventKind::NotSatisfied,
                round: 1,
                title: "Fix the path traversal in the upload handler".into(),
                detail: "Two findings are still open.".into(),
                verifier_chat_id: None,
            },
            "Verifier: round 1 not satisfied".into(),
        ),
        round2,
        marker(
            "wf-run-live-start",
            t(9),
            MessageOrigin::WorkflowEvent {
                run_id: LIVE_RUN.into(),
                marker: WorkflowEventMarker::Started,
                name: "security-review".into(),
                detail: String::new(),
            },
            workflow_marker_text(WorkflowEventMarker::Started, "security-review", ""),
        ),
        entry(
            "m3",
            MessageRole::Assistant,
            host,
            t(8),
            vec![text("t0", "Started the review workflow. Eight reviewers are looking at the code in parallel; I will fix what they find.")],
        ),
    ]
}

/// Write the demo chat's goal and runs the way a host would.
pub(crate) fn seed(doc: &SessionDoc, now: i64) -> Result<(), DocError> {
    doc.set_goal(&demo_goal(now))?;
    let state = demo_runs(now);
    let delta = WorkflowRunsState::default()
        .diff(&state)
        .expect("a populated state differs from an empty one");
    doc.apply_workflow_delta(&delta)
}

// MARK: - The simulated host

/// The lifecycle marker the controller leaves in the transcript.
fn goal_marker(doc: &SessionDoc, event: zeron_proto::GoalEventKind, goal: &Goal, now: i64) -> Result<(), DocError> {
    let mut e = marker(
        &format!("goal-{}-{now}", goal.id),
        now,
        MessageOrigin::GoalEvent {
            goal_id: goal.id.clone(),
            event,
            round: goal.iteration,
            title: goal.summary_title.clone(),
            detail: String::new(),
            verifier_chat_id: None,
        },
        MessageOrigin::goal_event_text(event, goal.iteration, &goal.summary_title, ""),
    );
    e.device_id = "host".into();
    doc.push_message(&e)
}

/// Execute a goal command like the controller would, minus the loop.
pub(crate) fn goal_command(doc: &SessionDoc, command: GoalCommand, now: i64) -> Result<(), DocError> {
    let current = doc.goal();
    match command {
        GoalCommand::Set { objective, limits, replace } => {
            if current.as_ref().is_some_and(|g| g.status.is_running() || g.status == GoalStatus::Paused) && !replace {
                return Ok(());
            }
            if let Ok(goal) = Goal::new(crate::new_id(), &objective, &limits, now) {
                doc.set_goal(&goal)?;
                goal_marker(doc, zeron_proto::GoalEventKind::Set, &goal, now)?;
            }
        }
        GoalCommand::Pause => {
            if let Some(mut goal) = current.filter(|g| g.status.is_running()) {
                goal.stop(GoalStatus::Paused, GoalReasonKind::User, "Paused by you");
                doc.set_goal(&goal)?;
                goal_marker(doc, zeron_proto::GoalEventKind::Paused, &goal, now)?;
            }
        }
        GoalCommand::Resume => {
            if let Some(mut goal) = current.filter(|g| g.status.is_stopped() && g.status != GoalStatus::Complete) {
                goal.status = GoalStatus::Active;
                goal.reason = None;
                doc.set_goal(&goal)?;
                goal_marker(doc, zeron_proto::GoalEventKind::Resumed, &goal, now)?;
            }
        }
        GoalCommand::Clear => {
            if let Some(goal) = current {
                doc.clear_goal()?;
                goal_marker(doc, zeron_proto::GoalEventKind::Cleared, &goal, now)?;
            }
        }
    }
    Ok(())
}

/// Execute a workflow command: stop settles the live run, resume starts it
/// again, an answer clears its question.
pub(crate) fn workflow_command(doc: &SessionDoc, command: WorkflowCommand) -> Result<(), DocError> {
    let before = doc.workflow_runs();
    let mut after = before.clone();
    match command {
        WorkflowCommand::Stop { run_id, .. } => {
            if let Some(run) = after.run_mut(&run_id).filter(|r| r.header.status == WorkflowStatus::Running) {
                run.header.status = WorkflowStatus::Stopped;
                run.header.stop_reason = Some(WorkflowStopReason::User);
                run.header.resumable = true;
                run.pending_questions.clear();
                for n in run.nodes.iter_mut().filter(|n| n.phase != NodePhase::Settled) {
                    n.phase = NodePhase::Settled;
                    n.outcome = Some(NodeOutcome::Cancelled);
                }
                for a in &mut run.actors {
                    if a.status == ActorStatus::Running {
                        a.status = ActorStatus::Completed;
                    }
                }
            }
        }
        WorkflowCommand::Resume { run_id } => {
            if let Some(run) = after.run_mut(&run_id).filter(|r| r.header.status == WorkflowStatus::Stopped && r.header.resumable) {
                run.header.status = WorkflowStatus::Running;
                run.header.stop_reason = None;
                run.header.stop_detail = None;
                run.header.resumable = false;
            }
        }
        WorkflowCommand::Answer { run_id, qid, .. } => {
            if let Some(run) = after.run_mut(&run_id) {
                run.pending_questions.retain(|q| q.qid != qid);
            }
        }
    }
    after.revision = before.revision + 1;
    if let Some(delta) = before.diff(&after) {
        doc.apply_workflow_delta(&delta)?;
    }
    Ok(())
}

/// `WorkflowArtifactRead` for the demo's artifacts.
pub(crate) fn artifact_reply(artifact_id: &str) -> serde_json::Value {
    let (content_type, title, body) = match artifact_id {
        "findings" => (
            "application/json",
            "Findings by severity",
            r#"{"columns":["Severity","Area","Finding","Status"],"rows":[["high","uploads","Path traversal in the filename","fixing"],["medium","auth","Token lifetime is 30 days","open"],["low","logs","User agent logged unescaped","fixed"],["low","deps","Two packages a minor behind","fixed"]]}"#,
        ),
        "coverage" => (
            "application/json",
            "Coverage",
            r#"[{"label":"Files reviewed","value":412},{"label":"Findings","value":4},{"label":"Coverage","value":87.5,"unit":"%"}]"#,
        ),
        _ => (
            "text/markdown",
            "Security review summary",
            "## Summary\n\nFour findings across **412 files**.\n\n1. **High** - path traversal in the upload filename (being fixed)\n2. **Medium** - token lifetime is 30 days\n3. **Low** - unescaped user agent in logs\n4. **Low** - two packages behind\n\n> Nothing here is exploitable without an account.",
        ),
    };
    serde_json::json!({
        "version": { "version": 1, "contentType": content_type, "title": title },
        "offset": 0,
        "total": body.len(),
        "encoding": "utf8",
        "data": body,
    })
}
