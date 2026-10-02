//! The pure run-state reducer: [`reduce`] folds one [`WorkflowEvent`] into a
//! [`WorkflowRunsState`] and returns the delta it caused.
//!
//! * **Idempotent.** Events carry a per-run monotone sequence; one at or below
//!   the run's `last_event_sequence` changes nothing and returns `None`, so a
//!   journal can be replayed over a state that already saw part of it.
//! * **Derived, not stored.** An actor's status comes from its nodes; phase
//!   progress counts nodes; `phase_alongside` is learned from overlap.
//! * **Bounded.** At most [`WORKFLOW_MAX_RUNS`] runs; per run
//!   [`WORKFLOW_MAX_ACTORS`] actors, [`WORKFLOW_MAX_NODES`] nodes, 64 reports,
//!   32 artifacts and 32 questions; [`WORKFLOW_MAX_ENTRIES`] entries across
//!   all runs. Finished entries are evicted first (settled nodes, completed
//!   actors, oldest reports, then whole settled runs); what cannot be listed
//!   is counted in `nodes_unlisted` / `actors_unlisted` and flags `truncated`.
//!
//! The delta's `header` is only present when something other than the
//! sequence number changed, so a quiet event stream does not rewrite it.

use zeron_proto::*;

/// Fold `event` into `state`. `None`: nothing changed (a stale sequence, an
/// event for a run this state never saw, or an event that changes nothing).
pub fn reduce(state: &mut WorkflowRunsState, event: &WorkflowEvent) -> Option<WorkflowRunsDelta> {
    let mut delta = WorkflowRunsDelta::default();
    if matches!(event.kind, WorkflowEventKind::RunCreated { .. })
        && state.run(&event.run_id).is_none()
    {
        create_run(state, event, &mut delta);
    }
    let index = state
        .runs
        .iter()
        .position(|r| r.header.run_id == event.run_id)?;
    if event.seq <= state.runs[index].header.last_event_sequence {
        return finish(state, delta, false);
    }

    let mut t = Touch::new(&state.runs[index]);
    {
        let run = &mut state.runs[index];
        run.header.last_event_sequence = event.seq;
        apply(run, event, &mut t);
        recompute_actor_status(run, &mut t);
    }
    let evictions = enforce_caps(state, index, &mut t);
    let run = &state.runs[index];
    let mut change = WorkflowRunDelta::for_run(&event.run_id);
    if t.header_changed(&run.header) {
        change.header = Some(run.header.clone());
    }
    for key in &t.upserts {
        if let Some(entry) = entry_of(run, key) {
            change.upserts.push(entry);
        }
    }
    change.removed = t.removed;
    if !change.is_empty() {
        merge_run(&mut delta, change);
    }
    for (_, other) in evictions {
        merge_run(&mut delta, other);
    }
    finish(state, delta, true)
}

/// Bump the revision when the state moved; hand back the delta when the doc
/// has something to write (an event that only advanced the sequence has not).
fn finish(
    state: &mut WorkflowRunsState,
    mut delta: WorkflowRunsDelta,
    applied: bool,
) -> Option<WorkflowRunsDelta> {
    if applied || !delta.is_empty() {
        state.revision += 1;
    }
    if delta.is_empty() {
        return None;
    }
    delta.revision = state.revision;
    Some(delta)
}

fn merge_run(delta: &mut WorkflowRunsDelta, change: WorkflowRunDelta) {
    match delta.runs.iter_mut().find(|r| r.run_id == change.run_id) {
        Some(slot) => slot.merge(change),
        None => delta.runs.push(change),
    }
}

// ── touch tracking ────────────────────────────────────────────────────────

/// What one event touched in a run: the keys to upsert / remove, and the
/// header as it was (for change detection ignoring the sequence).
struct Touch {
    before: WorkflowRunHeader,
    upserts: Vec<String>,
    removed: Vec<String>,
}

impl Touch {
    fn new(run: &WorkflowRun) -> Self {
        Self {
            before: run.header.clone(),
            upserts: Vec::new(),
            removed: Vec::new(),
        }
    }

    fn up(&mut self, key: String) {
        self.removed.retain(|k| *k != key);
        if !self.upserts.contains(&key) {
            self.upserts.push(key);
        }
    }

    fn remove(&mut self, key: String) {
        self.upserts.retain(|k| *k != key);
        if !self.removed.contains(&key) {
            self.removed.push(key);
        }
    }

    fn header_changed(&self, now: &WorkflowRunHeader) -> bool {
        let mut a = self.before.clone();
        a.last_event_sequence = now.last_event_sequence;
        a != *now
    }
}

fn entry_of(run: &WorkflowRun, key: &str) -> Option<WorkflowEntry> {
    let found = run
        .actors
        .iter()
        .find(|e| e.key() == key)
        .cloned()
        .map(WorkflowEntry::Actor)
        .or_else(|| {
            run.nodes
                .iter()
                .find(|e| e.key() == key)
                .cloned()
                .map(WorkflowEntry::Node)
        })
        .or_else(|| {
            run.reports
                .iter()
                .find(|e| e.key() == key)
                .cloned()
                .map(WorkflowEntry::Report)
        })
        .or_else(|| {
            run.artifacts
                .iter()
                .find(|e| e.key() == key)
                .cloned()
                .map(WorkflowEntry::Artifact)
        })
        .or_else(|| {
            run.pending_questions
                .iter()
                .find(|e| e.key() == key)
                .cloned()
                .map(WorkflowEntry::Question)
        });
    found.or_else(|| {
        (key == GRAPH_KEY)
            .then(|| run.graph.clone().map(WorkflowEntry::Graph))
            .flatten()
    })
}

// ── creation and eviction of runs ─────────────────────────────────────────

fn create_run(state: &mut WorkflowRunsState, event: &WorkflowEvent, delta: &mut WorkflowRunsDelta) {
    let WorkflowEventKind::RunCreated { .. } = &event.kind else {
        return;
    };
    // Make room for the newcomer: oldest settled run first, else oldest.
    while state.runs.len() >= WORKFLOW_MAX_RUNS {
        let at = state
            .runs
            .iter()
            .position(|r| r.header.status.is_settled())
            .unwrap_or(0);
        let gone = state.runs.remove(at);
        delta.runs_removed.push(gone.header.run_id);
    }
    state.runs.push(WorkflowRun {
        header: WorkflowRunHeader {
            run_id: event.run_id.clone(),
            created_at: event.at,
            ..WorkflowRunHeader::default()
        },
        ..WorkflowRun::default()
    });
}

// ── applying one event ────────────────────────────────────────────────────

fn clip(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (format!("{}…", &text[..end]), true)
}

fn head(text: &str) -> String {
    if text.chars().count() <= WORKFLOW_HEAD_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(WORKFLOW_HEAD_CHARS).collect();
    format!("{cut}…")
}

fn apply(run: &mut WorkflowRun, event: &WorkflowEvent, t: &mut Touch) {
    use WorkflowEventKind as K;
    match &event.kind {
        K::RunCreated {
            name,
            chat_id,
            script_hash,
            resumed_from,
            graph,
            concurrency_ceiling,
            saved,
        } => {
            let h = &mut run.header;
            h.name = name.clone();
            h.saved_name = saved.as_ref().map(|s| s.name.clone());
            h.saved_scope = saved.as_ref().map(|s| s.scope);
            h.chat_id = chat_id.clone();
            h.status = WorkflowStatus::Pending;
            h.script_hash = script_hash.clone();
            h.resumed_from = resumed_from.clone();
            h.concurrency.ceiling = *concurrency_ceiling;
            h.concurrency.cap = *concurrency_ceiling;
            if let Some(graph) = graph {
                h.phase_names = graph.phase_names();
                run.graph = Some(graph.clone());
                t.up(GRAPH_KEY.to_owned());
            }
        }
        K::RunLaunched { phase_names } => {
            run.header.status = WorkflowStatus::Running;
            run.header.started_at.get_or_insert(event.at);
            run.header.resumable = false;
            run.header.ended_at = None;
            run.header.stop_reason = None;
            run.header.stop_detail = None;
            if !phase_names.is_empty() {
                run.header.phase_names = phase_names.clone();
            }
        }
        K::PhaseEntered { name } => {
            run.header.current_phase = Some(name.clone());
            progress_of(&mut run.header, name);
        }
        K::ActorCreated {
            site_id,
            ordinal,
            name,
            harness,
            model,
        } => {
            let order = run.actors.iter().map(|a| a.order + 1).max().unwrap_or(0);
            let actor = WorkflowActor {
                order,
                site_id: site_id.clone(),
                ordinal: *ordinal,
                name: name.clone(),
                child_chat_id: None,
                status: ActorStatus::Waiting,
                phase_name: run.header.current_phase.clone(),
                harness: harness.clone(),
                model: model.clone(),
                asks: 0,
                failed_asks: 0,
            };
            let key = actor.key();
            if run.actors.iter().any(|a| a.key() == key) {
                return;
            }
            if run.actors.len() >= WORKFLOW_MAX_ACTORS {
                // Evict the oldest completed actor; else leave it unlisted.
                if let Some(at) = run
                    .actors
                    .iter()
                    .position(|a| a.status == ActorStatus::Completed)
                {
                    let gone = run.actors.remove(at);
                    run.header.actors_unlisted += 1;
                    run.header.truncated = true;
                    t.remove(gone.key());
                } else {
                    run.header.actors_unlisted += 1;
                    run.header.truncated = true;
                    return;
                }
            }
            run.actors.push(actor);
            t.up(key);
        }
        K::ActorChild {
            site_id,
            ordinal,
            child_chat_id,
        } => {
            let key = entry_key('a', site_id, *ordinal);
            if let Some(a) = run.actors.iter_mut().find(|a| a.key() == key) {
                a.child_chat_id = Some(child_chat_id.clone());
                t.up(key);
            }
        }
        K::NodeQueued {
            site_id,
            ordinal,
            kind,
            actor_site_id,
            actor_ordinal,
            instructions_head,
            phase_name,
        } => {
            let phase = phase_name
                .clone()
                .or_else(|| run.header.current_phase.clone());
            if let Some(p) = &phase {
                progress_of(&mut run.header, p).observed += 1;
                note_alongside(&mut run.header, p);
            }
            let order = run.nodes.iter().map(|n| n.order + 1).max().unwrap_or(0);
            let node = WorkflowNode {
                order,
                site_id: site_id.clone(),
                ordinal: *ordinal,
                kind: *kind,
                phase: NodePhase::Queued,
                outcome: None,
                cached: false,
                actor_site_id: actor_site_id.clone(),
                actor_ordinal: *actor_ordinal,
                phase_name: phase,
                instructions_head: head(instructions_head),
                turn: 0,
                tool_calls: 0,
                last_tool: None,
                tokens: 0,
                started_at: None,
                ended_at: None,
                error: None,
                result_preview: None,
            };
            let key = node.key();
            if run.nodes.iter().any(|n| n.key() == key) {
                return;
            }
            if run.nodes.len() >= WORKFLOW_MAX_NODES {
                // Evict the oldest settled node; else the newcomer goes unlisted.
                if let Some(at) = run.nodes.iter().position(WorkflowNode::is_settled) {
                    let gone = run.nodes.remove(at);
                    t.remove(gone.key());
                } else {
                    run.header.nodes_unlisted += 1;
                    run.header.truncated = true;
                    return;
                }
                run.header.nodes_unlisted += 1;
                run.header.truncated = true;
            }
            run.nodes.push(node);
            t.up(key);
        }
        K::NodeDispatched { site_id, ordinal } => {
            node_phase(run, t, site_id, *ordinal, NodePhase::Dispatched, event.at);
        }
        K::NodeExecuting { site_id, ordinal } => {
            node_phase(run, t, site_id, *ordinal, NodePhase::Executing, event.at);
        }
        K::NodeWaiting { site_id, ordinal } => {
            node_phase(run, t, site_id, *ordinal, NodePhase::Waiting, event.at);
        }
        K::NodeRepairing { site_id, ordinal } => {
            node_phase(run, t, site_id, *ordinal, NodePhase::Repairing, event.at);
        }
        K::NodeNudged { site_id, ordinal } => {
            node_phase(run, t, site_id, *ordinal, NodePhase::Nudged, event.at);
        }
        K::NodeProgress {
            site_id,
            ordinal,
            turn,
            tool_calls,
            last_tool,
        } => {
            let key = entry_key('n', site_id, *ordinal);
            if let Some(n) = run
                .nodes
                .iter_mut()
                .find(|n| n.key() == key && !n.is_settled())
            {
                n.turn = *turn;
                n.tool_calls = *tool_calls;
                n.last_tool = last_tool.clone();
                t.up(key);
            }
        }
        K::NodeSettled {
            site_id,
            ordinal,
            outcome,
            cached,
            tokens,
            error,
            result_preview,
        } => {
            let key = entry_key('n', site_id, *ordinal);
            let Some(n) = run.nodes.iter_mut().find(|n| n.key() == key) else {
                return;
            };
            if n.is_settled() {
                return;
            }
            n.phase = NodePhase::Settled;
            n.outcome = Some(*outcome);
            n.cached = *cached;
            n.tokens = *tokens;
            n.ended_at = Some(event.at);
            n.error = error.as_deref().map(|e| clip(e, WORKFLOW_PREVIEW_BYTES).0);
            n.result_preview = result_preview
                .as_deref()
                .map(|p| clip(p, WORKFLOW_PREVIEW_BYTES).0);
            let phase = n.phase_name.clone();
            if let Some(p) = phase {
                progress_of(&mut run.header, &p).settled += 1;
            }
            t.up(key);
        }
        K::Report {
            index,
            text,
            truncated,
            artifact_id,
        } => {
            run.header.reports_total = run.header.reports_total.max(index + 1);
            let (text, clipped) = clip(text, WORKFLOW_PREVIEW_BYTES);
            let report = WorkflowReport {
                index: *index,
                text,
                truncated: *truncated || clipped,
                artifact_id: artifact_id.clone(),
                at: event.at,
            };
            let key = report.key();
            if run.reports.iter().any(|r| r.key() == key) {
                return;
            }
            run.reports.push(report);
            t.up(key);
            while run.reports.len() > WORKFLOW_MAX_REPORTS {
                let gone = run.reports.remove(0);
                run.header.truncated = true;
                t.remove(gone.key());
            }
            if let Some(id) = artifact_id {
                for a in run.artifacts.iter_mut() {
                    let primary = a.id == *id;
                    if a.primary != primary {
                        a.primary = primary;
                        t.up(a.key());
                    }
                }
            }
        }
        K::ArtifactPublished { summary } => {
            let key = summary.key();
            let exists = run.artifacts.iter().any(|a| a.key() == key);
            if !exists && run.artifacts.len() >= WORKFLOW_MAX_ARTIFACTS {
                run.header.truncated = true;
                return;
            }
            let mut summary = summary.clone();
            if let Some(old) = run.artifacts.iter().find(|a| a.key() == key) {
                summary.primary = summary.primary || old.primary;
            }
            run.upsert(WorkflowEntry::Artifact(summary));
            t.up(key);
        }
        K::UsageUpdated { usage } => run.header.usage = usage.clone(),
        K::EscalationRaised { question } => {
            let key = question.key();
            if !run.pending_questions.iter().any(|q| q.key() == key) {
                if run.pending_questions.len() >= WORKFLOW_MAX_QUESTIONS {
                    run.header.truncated = true;
                    return;
                }
                run.pending_questions.push(question.clone());
            }
            t.up(key);
        }
        K::EscalationResolved { qid } => {
            let key = format!("q:{qid}");
            if run.pending_questions.iter().any(|q| q.key() == key) {
                run.pending_questions.retain(|q| q.key() != key);
                t.remove(key);
            }
        }
        K::ConcurrencyChanged { concurrency } => run.header.concurrency = concurrency.clone(),
        K::Stalled => run.header.stalled = true,
        K::Unstalled => run.header.stalled = false,
        K::RunSettled {
            status,
            stop_reason,
            stop_detail,
            error,
            result_preview,
            result_truncated,
            resumable,
        } => {
            run.header.status = *status;
            run.header.stop_reason = *stop_reason;
            run.header.stop_detail = stop_detail.clone();
            run.header.error = error.as_deref().map(|e| clip(e, WORKFLOW_PREVIEW_BYTES).0);
            run.header.result_preview = result_preview
                .as_deref()
                .map(|p| clip(p, WORKFLOW_PREVIEW_BYTES).0);
            run.header.result_truncated = *result_truncated;
            run.header.resumable = *resumable;
            run.header.ended_at = Some(event.at);
            run.header.stalled = false;
            // Whatever the engine did not settle individually is cancelled.
            let mut settled_phases: Vec<String> = Vec::new();
            for n in run.nodes.iter_mut().filter(|n| !n.is_settled()) {
                n.phase = NodePhase::Settled;
                n.outcome = Some(NodeOutcome::Cancelled);
                n.ended_at = Some(event.at);
                settled_phases.extend(n.phase_name.clone());
                t.up(n.key());
            }
            for p in settled_phases {
                progress_of(&mut run.header, &p).settled += 1;
            }
            for q in std::mem::take(&mut run.pending_questions) {
                t.remove(q.key());
            }
        }
    }
}

fn progress_of<'a>(header: &'a mut WorkflowRunHeader, name: &str) -> &'a mut WorkflowPhaseProgress {
    if let Some(at) = header.phases.iter().position(|p| p.name == name) {
        return &mut header.phases[at];
    }
    header.phases.push(WorkflowPhaseProgress {
        name: name.to_owned(),
        observed: 0,
        settled: 0,
    });
    header.phases.last_mut().expect("just pushed")
}

/// Phases with unfinished work alongside `phase` form a group.
fn note_alongside(header: &mut WorkflowRunHeader, phase: &str) {
    let busy: Vec<String> = header
        .phases
        .iter()
        .filter(|p| p.name != phase && p.observed > p.settled)
        .map(|p| p.name.clone())
        .collect();
    if busy.is_empty() {
        return;
    }
    let mut group: std::collections::BTreeSet<String> = busy.into_iter().collect();
    group.insert(phase.to_owned());
    // Merge with every existing group it touches.
    let mut merged = group;
    header.phase_alongside.retain(|g| {
        if g.iter().any(|p| merged.contains(p)) {
            merged.extend(g.iter().cloned());
            false
        } else {
            true
        }
    });
    header.phase_alongside.push(merged.into_iter().collect());
    header.phase_alongside.sort();
}

fn node_phase(
    run: &mut WorkflowRun,
    t: &mut Touch,
    site_id: &str,
    ordinal: u32,
    to: NodePhase,
    at: i64,
) {
    let key = entry_key('n', site_id, ordinal);
    let Some(n) = run
        .nodes
        .iter_mut()
        .find(|n| n.key() == key && !n.is_settled())
    else {
        return;
    };
    n.phase = to;
    if to == NodePhase::Dispatched || (to == NodePhase::Executing && n.started_at.is_none()) {
        n.started_at.get_or_insert(at);
    }
    let phase = n.phase_name.clone();
    t.up(key);
    if let Some(p) = phase {
        note_alongside(&mut run.header, &p);
    }
}

/// Actor status is derived from the actor's nodes.
fn recompute_actor_status(run: &mut WorkflowRun, t: &mut Touch) {
    for a in run.actors.iter_mut() {
        let mut asks = 0u32;
        let mut failed = 0u32;
        let mut active = false;
        let mut unsettled = false;
        for n in run.nodes.iter().filter(|n| {
            n.actor_site_id.as_deref() == Some(a.site_id.as_str()) && n.actor_ordinal == a.ordinal
        }) {
            asks += 1;
            if n.outcome == Some(NodeOutcome::Failed) {
                failed += 1;
            }
            match n.phase {
                NodePhase::Settled => {}
                NodePhase::Queued => unsettled = true,
                _ => {
                    active = true;
                    unsettled = true;
                }
            }
        }
        let status = if active {
            ActorStatus::Running
        } else if asks > 0 && !unsettled {
            ActorStatus::Completed
        } else {
            ActorStatus::Waiting
        };
        if a.status != status || a.asks != asks || a.failed_asks != failed {
            a.status = status;
            a.asks = asks;
            a.failed_asks = failed;
            t.up(a.key());
        }
    }
}

// ── caps across runs ──────────────────────────────────────────────────────

/// Keep the whole state within [`WORKFLOW_MAX_ENTRIES`]: evict finished
/// entries — settled nodes of settled runs first, then of any run but this
/// one's live ones — and record what went. Returns the deltas for the *other*
/// runs touched.
fn enforce_caps(
    state: &mut WorkflowRunsState,
    current: usize,
    t: &mut Touch,
) -> Vec<(String, WorkflowRunDelta)> {
    let mut out: Vec<(String, WorkflowRunDelta)> = Vec::new();
    while state.entry_count() > WORKFLOW_MAX_ENTRIES {
        // Candidate order: settled runs oldest-first, then the others.
        let mut order: Vec<usize> = (0..state.runs.len()).collect();
        order.sort_by_key(|&i| (!state.runs[i].header.status.is_settled(), i == current, i));
        let mut evicted = false;
        for i in order {
            let run = &mut state.runs[i];
            let Some(at) = run.nodes.iter().position(WorkflowNode::is_settled) else {
                continue;
            };
            let gone = run.nodes.remove(at);
            run.header.nodes_unlisted += 1;
            run.header.truncated = true;
            if i == current {
                t.remove(gone.key());
            } else {
                let id = run.header.run_id.clone();
                let mut change = WorkflowRunDelta::for_run(&id);
                change.header = Some(run.header.clone());
                change.removed.push(gone.key());
                match out.iter_mut().find(|(rid, _)| *rid == id) {
                    Some((_, slot)) => slot.merge(change),
                    None => out.push((id, change)),
                }
            }
            evicted = true;
            break;
        }
        if !evicted {
            // Nothing finished to drop: stop (live work is never evicted).
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests;
