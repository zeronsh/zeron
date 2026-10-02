use super::*;

struct Log {
    run: String,
    seq: u64,
    events: Vec<WorkflowEvent>,
}

impl Log {
    fn new(run: &str) -> Self {
        Self {
            run: run.into(),
            seq: 0,
            events: Vec::new(),
        }
    }

    fn push(&mut self, kind: WorkflowEventKind) -> &mut Self {
        self.seq += 1;
        self.events.push(WorkflowEvent {
            run_id: self.run.clone(),
            seq: self.seq,
            at: 1000 + self.seq as i64,
            kind,
        });
        self
    }

    fn created(&mut self) -> &mut Self {
        self.push(WorkflowEventKind::RunCreated {
            name: "demo".into(),
            chat_id: "chat".into(),
            script_hash: "h".into(),
            resumed_from: None,
            graph: Some(WorkflowGraph {
                phases: vec![
                    GraphPhase {
                        name: "review".into(),
                        ..Default::default()
                    },
                    GraphPhase {
                        name: "gate".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            concurrency_ceiling: 8,
            saved: None,
        })
    }

    fn launched(&mut self) -> &mut Self {
        self.push(WorkflowEventKind::RunLaunched {
            phase_names: Vec::new(),
        })
    }

    fn phase(&mut self, name: &str) -> &mut Self {
        self.push(WorkflowEventKind::PhaseEntered { name: name.into() })
    }

    fn actor(&mut self, site: &str, ordinal: u32) -> &mut Self {
        self.push(WorkflowEventKind::ActorCreated {
            site_id: site.into(),
            ordinal,
            name: format!("actor-{site}-{ordinal}"),
            harness: None,
            model: None,
        })
    }

    fn queued(&mut self, site: &str, ordinal: u32, actor: &str, actor_ordinal: u32) -> &mut Self {
        self.push(WorkflowEventKind::NodeQueued {
            site_id: site.into(),
            ordinal,
            kind: NodeKind::Ask,
            actor_site_id: Some(actor.into()),
            actor_ordinal,
            instructions_head: format!("do {site}#{ordinal}"),
            phase_name: None,
        })
    }

    fn step(&mut self, site: &str, ordinal: u32, to: &str) -> &mut Self {
        let (site_id, ordinal) = (site.to_owned(), ordinal);
        self.push(match to {
            "dispatched" => WorkflowEventKind::NodeDispatched { site_id, ordinal },
            "executing" => WorkflowEventKind::NodeExecuting { site_id, ordinal },
            "waiting" => WorkflowEventKind::NodeWaiting { site_id, ordinal },
            "repairing" => WorkflowEventKind::NodeRepairing { site_id, ordinal },
            "nudged" => WorkflowEventKind::NodeNudged { site_id, ordinal },
            other => panic!("{other}"),
        })
    }

    fn settled(&mut self, site: &str, ordinal: u32, outcome: NodeOutcome) -> &mut Self {
        self.push(WorkflowEventKind::NodeSettled {
            site_id: site.into(),
            ordinal,
            outcome,
            cached: false,
            tokens: 10,
            error: (outcome == NodeOutcome::Failed).then(|| "boom".to_owned()),
            result_preview: Some("{\"ok\":true}".into()),
        })
    }

    fn run_settled(&mut self, status: WorkflowStatus) -> &mut Self {
        self.push(WorkflowEventKind::RunSettled {
            status,
            stop_reason: None,
            stop_detail: None,
            error: None,
            result_preview: Some("done".into()),
            result_truncated: false,
            resumable: false,
        })
    }
}

fn fold(events: &[WorkflowEvent]) -> (WorkflowRunsState, Vec<WorkflowRunsDelta>) {
    let mut state = WorkflowRunsState::default();
    let mut deltas = Vec::new();
    for e in events {
        deltas.extend(reduce(&mut state, e));
    }
    (state, deltas)
}

/// The header's sequence is deliberately not part of a delta when nothing
/// else changed, so replayed state is compared without it.
fn normalized(mut state: WorkflowRunsState) -> WorkflowRunsState {
    state.revision = 0;
    for r in &mut state.runs {
        r.header.last_event_sequence = 0;
    }
    state
}

fn node<'a>(state: &'a WorkflowRunsState, run: &str, site: &str, ordinal: u32) -> &'a WorkflowNode {
    state
        .run(run)
        .unwrap()
        .nodes
        .iter()
        .find(|n| n.site_id == site && n.ordinal == ordinal)
        .unwrap()
}

#[test]
fn a_run_folds_into_the_card_state() {
    let mut log = Log::new("r1");
    log.created()
        .launched()
        .phase("review")
        .actor("a", 0)
        .queued("q", 0, "a", 0)
        .queued("q", 1, "a", 0)
        .step("q", 0, "dispatched")
        .step("q", 0, "executing");
    let (state, _) = fold(&log.events);
    let run = state.run("r1").unwrap();
    assert_eq!(run.header.status, WorkflowStatus::Running);
    assert_eq!(run.header.phase_names, ["review", "gate"]);
    assert_eq!(run.header.current_phase.as_deref(), Some("review"));
    assert_eq!(run.header.concurrency.ceiling, 8);
    assert_eq!(run.actors[0].status, ActorStatus::Running);
    assert_eq!(run.actors[0].asks, 2);
    assert_eq!(node(&state, "r1", "q", 1).phase, NodePhase::Queued);
    assert_eq!(node(&state, "r1", "q", 0).phase, NodePhase::Executing);
    assert_eq!(run.header.phases[0].observed, 2);
    assert_eq!(run.header.phases[0].settled, 0);
    assert!(run.graph.is_some());

    // Settle both: the actor is completed and the phase counts agree.
    log.settled("q", 0, NodeOutcome::Ok)
        .step("q", 1, "dispatched")
        .settled("q", 1, NodeOutcome::Failed)
        .run_settled(WorkflowStatus::Completed);
    let (state, _) = fold(&log.events);
    let run = state.run("r1").unwrap();
    assert_eq!(run.actors[0].status, ActorStatus::Completed);
    assert_eq!(run.actors[0].failed_asks, 1);
    assert_eq!(run.header.phases[0].settled, 2);
    assert_eq!(run.header.status, WorkflowStatus::Completed);
    assert_eq!(run.header.result_preview.as_deref(), Some("done"));
    assert_eq!(node(&state, "r1", "q", 1).error.as_deref(), Some("boom"));
}

#[test]
fn replaying_events_is_idempotent() {
    let mut log = Log::new("r1");
    log.created()
        .launched()
        .phase("review")
        .actor("a", 0)
        .queued("q", 0, "a", 0)
        .step("q", 0, "executing")
        .settled("q", 0, NodeOutcome::Ok);
    let (once, deltas) = fold(&log.events);
    // The same stream folded again over the final state changes nothing.
    let mut again = once.clone();
    for e in &log.events {
        assert_eq!(reduce(&mut again, e), None, "{e:?}");
    }
    assert_eq!(again, once);
    // …and so does delivering each event twice in a row.
    let mut doubled = WorkflowRunsState::default();
    for e in &log.events {
        reduce(&mut doubled, e);
        assert_eq!(reduce(&mut doubled, e), None);
    }
    assert_eq!(doubled.runs, once.runs);
    // Deltas rebuild the state on a client that only saw deltas.
    let mut client = WorkflowRunsState::default();
    for d in &deltas {
        client.apply(d);
        client.apply(d);
    }
    assert_eq!(normalized(client), normalized(once));
}

#[test]
fn events_for_unknown_runs_are_ignored() {
    let mut state = WorkflowRunsState::default();
    let e = WorkflowEvent {
        run_id: "ghost".into(),
        seq: 4,
        at: 1,
        kind: WorkflowEventKind::Stalled,
    };
    assert_eq!(reduce(&mut state, &e), None);
    assert!(state.runs.is_empty());
}

#[test]
fn a_node_belongs_to_the_phase_it_was_dispatched_in() {
    let mut log = Log::new("r");
    log.created().launched().phase("review").actor("a", 0);
    log.queued("q", 0, "a", 0);
    log.phase("gate");
    log.queued("g", 0, "a", 0);
    let (state, _) = fold(&log.events);
    assert_eq!(
        node(&state, "r", "q", 0).phase_name.as_deref(),
        Some("review")
    );
    assert_eq!(
        node(&state, "r", "g", 0).phase_name.as_deref(),
        Some("gate")
    );
    // Work from `review` is still open while `gate` queues: they overlap.
    assert_eq!(
        state.run("r").unwrap().header.phase_alongside,
        vec![vec!["gate".to_string(), "review".to_string()]]
    );
}

#[test]
fn nodes_beyond_the_cap_are_counted_not_listed_and_settled_ones_make_room() {
    let mut log = Log::new("r");
    log.created().launched().phase("p").actor("a", 0);
    for i in 0..(WORKFLOW_MAX_NODES as u32 + 20) {
        log.queued("q", i, "a", 0);
    }
    let (state, _) = fold(&log.events);
    let run = state.run("r").unwrap();
    assert_eq!(run.nodes.len(), WORKFLOW_MAX_NODES);
    assert_eq!(run.header.nodes_unlisted, 20);
    assert!(run.header.truncated);
    // Phase progress still counts the unlisted ones.
    assert_eq!(
        run.header.phases[0].observed,
        WORKFLOW_MAX_NODES as u32 + 20
    );

    // With settled nodes around, a newcomer evicts the oldest finished one.
    let mut log = Log::new("r");
    log.created().launched().phase("p").actor("a", 0);
    for i in 0..WORKFLOW_MAX_NODES as u32 {
        log.queued("q", i, "a", 0);
    }
    for i in 0..10 {
        log.settled("q", i, NodeOutcome::Ok);
    }
    for i in 0..3 {
        log.queued("n", i, "a", 0);
    }
    let (state, deltas) = fold(&log.events);
    let run = state.run("r").unwrap();
    assert_eq!(run.nodes.len(), WORKFLOW_MAX_NODES);
    assert_eq!(run.header.nodes_unlisted, 3);
    assert!(
        run.nodes
            .iter()
            .all(|n| !(n.site_id == "q" && n.ordinal < 3))
    );
    assert!(run.nodes.iter().any(|n| n.site_id == "n" && n.ordinal == 2));
    // The eviction travelled as a removal.
    let removed: Vec<_> = deltas
        .iter()
        .flat_map(|d| d.runs.iter().flat_map(|r| r.removed.iter()))
        .collect();
    assert!(removed.contains(&&"n:q#0".to_string()));
}

#[test]
fn the_total_entry_cap_evicts_finished_nodes_of_old_runs_first() {
    let mut events = Vec::new();
    for run in 0..7 {
        let mut log = Log::new(&format!("r{run}"));
        log.created().launched().phase("p").actor("a", 0);
        for i in 0..900u32 {
            log.queued("q", i, "a", 0).settled("q", i, NodeOutcome::Ok);
        }
        log.run_settled(WorkflowStatus::Completed);
        events.extend(log.events);
    }
    let (state, _) = fold(&events);
    assert!(
        state.entry_count() <= WORKFLOW_MAX_ENTRIES,
        "{}",
        state.entry_count()
    );
    // The oldest run paid first.
    let first = state.run("r0").unwrap();
    let last = state.run("r6").unwrap();
    assert!(first.nodes.len() < last.nodes.len());
    assert!(first.header.truncated);
}

#[test]
fn only_eight_runs_are_kept_and_finished_ones_go_first() {
    let mut events = Vec::new();
    for run in 0..8 {
        let mut log = Log::new(&format!("r{run}"));
        log.created().launched();
        if run != 0 {
            log.run_settled(WorkflowStatus::Completed);
        }
        events.extend(log.events);
    }
    let mut state = WorkflowRunsState::default();
    for e in &events {
        reduce(&mut state, e);
    }
    assert_eq!(state.runs.len(), 8);
    // r0 is still running, so the ninth run evicts r1 (the oldest FINISHED).
    let mut log = Log::new("r8");
    log.created();
    let delta = reduce(&mut state, &log.events[0]).unwrap();
    assert_eq!(delta.runs_removed, ["r1"]);
    assert_eq!(state.runs.len(), 8);
    assert!(state.run("r0").is_some());
    assert!(state.run("r1").is_none());
}

#[test]
fn reports_artifacts_and_questions_have_their_own_caps() {
    let mut log = Log::new("r");
    log.created().launched();
    for i in 0..70u32 {
        log.push(WorkflowEventKind::Report {
            index: i,
            text: format!("finding {i}"),
            truncated: false,
            artifact_id: None,
        });
    }
    for i in 0..40 {
        log.push(WorkflowEventKind::ArtifactPublished {
            summary: ArtifactSummary {
                id: format!("a{i}"),
                kind: ArtifactKind::Markdown,
                title: "t".into(),
                version: 1,
                content_type: "text/markdown".into(),
                bytes: 3,
                item_count: 0,
                primary: false,
            },
        });
    }
    let (state, _) = fold(&log.events);
    let run = state.run("r").unwrap();
    assert_eq!(run.reports.len(), WORKFLOW_MAX_REPORTS);
    assert_eq!(run.reports[0].index, 6, "the oldest reports were dropped");
    assert_eq!(run.header.reports_total, 70);
    assert_eq!(run.artifacts.len(), WORKFLOW_MAX_ARTIFACTS);
    assert!(run.header.truncated);
}

#[test]
fn escalations_park_until_resolved_or_the_run_settles() {
    let q = |id: &str| WorkflowQuestion {
        qid: id.into(),
        actor_site_id: "a".into(),
        actor_ordinal: 0,
        actor_name: "x".into(),
        question: "which?".into(),
        context: String::new(),
        asked_at: 1,
    };
    let mut log = Log::new("r");
    log.created().launched();
    log.push(WorkflowEventKind::EscalationRaised { question: q("1") });
    log.push(WorkflowEventKind::EscalationRaised { question: q("2") });
    log.push(WorkflowEventKind::EscalationResolved { qid: "1".into() });
    let (state, _) = fold(&log.events);
    assert_eq!(state.run("r").unwrap().pending_questions.len(), 1);
    log.run_settled(WorkflowStatus::Stopped);
    let (state, _) = fold(&log.events);
    assert!(state.run("r").unwrap().pending_questions.is_empty());
}

#[test]
fn settling_a_run_cancels_what_the_engine_did_not_settle() {
    let mut log = Log::new("r");
    log.created().launched().phase("p").actor("a", 0);
    log.queued("q", 0, "a", 0).step("q", 0, "executing");
    log.queued("q", 1, "a", 0);
    log.run_settled(WorkflowStatus::Stopped);
    let (state, _) = fold(&log.events);
    for ordinal in 0..2 {
        let n = node(&state, "r", "q", ordinal);
        assert_eq!(n.phase, NodePhase::Settled);
        assert_eq!(n.outcome, Some(NodeOutcome::Cancelled));
    }
    let run = state.run("r").unwrap();
    assert_eq!(run.actors[0].status, ActorStatus::Completed);
    assert_eq!(run.header.phases[0].settled, 2);
}

#[test]
fn quiet_events_do_not_rewrite_the_header() {
    let mut log = Log::new("r");
    log.created()
        .launched()
        .phase("p")
        .actor("a", 0)
        .queued("q", 0, "a", 0);
    let (mut state, _) = fold(&log.events);
    let progress = WorkflowEvent {
        run_id: "r".into(),
        seq: 100,
        at: 5,
        kind: WorkflowEventKind::NodeProgress {
            site_id: "q".into(),
            ordinal: 0,
            turn: 1,
            tool_calls: 3,
            last_tool: Some("grep".into()),
        },
    };
    let d = reduce(&mut state, &progress).unwrap();
    assert!(
        d.runs[0].header.is_none(),
        "a progress tick must not touch the header"
    );
    assert_eq!(d.runs[0].upserts.len(), 1);
    assert_eq!(node(&state, "r", "q", 0).tool_calls, 3);
}

#[test]
fn long_text_is_clipped_in_synced_state() {
    let mut log = Log::new("r");
    log.created()
        .launched()
        .phase("p")
        .actor("a", 0)
        .queued("q", 0, "a", 0);
    log.push(WorkflowEventKind::NodeSettled {
        site_id: "q".into(),
        ordinal: 0,
        outcome: NodeOutcome::Ok,
        cached: false,
        tokens: 0,
        error: None,
        result_preview: Some("é".repeat(5000)),
    });
    let (state, _) = fold(&log.events);
    let preview = node(&state, "r", "q", 0).result_preview.clone().unwrap();
    assert!(preview.len() <= WORKFLOW_PREVIEW_BYTES + 4);
    assert!(preview.ends_with('…'));
}

// ── property-style: random streams keep every invariant ───────────────────

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

fn random_log(seed: u64, runs: u64, steps: u64) -> Vec<WorkflowEvent> {
    let mut rng = Lcg(seed);
    let mut logs: Vec<Log> = (0..runs)
        .map(|r| {
            let mut l = Log::new(&format!("r{r}"));
            l.created().launched().phase("p");
            l
        })
        .collect();
    let mut next_node = vec![0u32; logs.len()];
    let mut live: Vec<Vec<u32>> = vec![Vec::new(); logs.len()];
    let mut actors = vec![0u32; logs.len()];
    for _ in 0..steps {
        let r = rng.next(runs) as usize;
        let log = &mut logs[r];
        match rng.next(9) {
            0 => {
                log.actor("a", actors[r]);
                actors[r] += 1;
            }
            1..=3 if actors[r] > 0 => {
                let n = next_node[r];
                next_node[r] += 1;
                log.queued("q", n, "a", rng.next(actors[r] as u64) as u32);
                live[r].push(n);
            }
            4 | 5 if !live[r].is_empty() => {
                let n = live[r][rng.next(live[r].len() as u64) as usize];
                log.step("q", n, "executing");
            }
            6 | 7 if !live[r].is_empty() => {
                let at = rng.next(live[r].len() as u64) as usize;
                let n = live[r].swap_remove(at);
                log.settled("q", n, NodeOutcome::Ok);
            }
            _ => {
                log.phase(if rng.next(2) == 0 { "p" } else { "gate" });
            }
        }
    }
    // Interleave the per-run logs.
    let mut cursors = vec![0usize; logs.len()];
    let mut out = Vec::new();
    loop {
        let candidates: Vec<usize> = (0..logs.len())
            .filter(|&i| cursors[i] < logs[i].events.len())
            .collect();
        if candidates.is_empty() {
            break;
        }
        let pick = candidates[rng.next(candidates.len() as u64) as usize];
        out.push(logs[pick].events[cursors[pick]].clone());
        cursors[pick] += 1;
    }
    out
}

#[test]
fn random_streams_keep_the_invariants() {
    for seed in 0..60u64 {
        let events = random_log(seed, 3, 700);
        let (state, deltas) = fold(&events);

        // Idempotent over a full replay.
        let mut again = state.clone();
        for e in &events {
            assert_eq!(reduce(&mut again, e), None, "seed {seed}");
        }
        assert_eq!(again, state, "seed {seed}");

        // A client that applied only the deltas converges.
        let mut client = WorkflowRunsState::default();
        for d in &deltas {
            client.apply(d);
        }
        assert_eq!(normalized(client), normalized(state.clone()), "seed {seed}");

        // Caps.
        assert!(state.runs.len() <= WORKFLOW_MAX_RUNS);
        assert!(state.entry_count() <= WORKFLOW_MAX_ENTRIES);
        for run in &state.runs {
            assert!(run.nodes.len() <= WORKFLOW_MAX_NODES);
            assert!(run.actors.len() <= WORKFLOW_MAX_ACTORS);
            // Derived actor status agrees with the nodes.
            for a in &run.actors {
                let mine: Vec<&WorkflowNode> = run
                    .nodes
                    .iter()
                    .filter(|n| {
                        n.actor_site_id.as_deref() == Some(&a.site_id)
                            && n.actor_ordinal == a.ordinal
                    })
                    .collect();
                let any_active = mine
                    .iter()
                    .any(|n| !matches!(n.phase, NodePhase::Settled | NodePhase::Queued));
                if any_active {
                    assert_eq!(a.status, ActorStatus::Running, "seed {seed}");
                }
                if !mine.is_empty() && mine.iter().all(|n| n.is_settled()) {
                    assert_eq!(a.status, ActorStatus::Completed, "seed {seed}");
                }
                assert_eq!(a.asks as usize, mine.len(), "seed {seed}");
            }
            // Settled never exceeds observed.
            for p in &run.header.phases {
                assert!(p.settled <= p.observed, "seed {seed} {p:?}");
            }
        }
    }
}

#[test]
fn a_two_hundred_node_run_writes_small_deltas() {
    // The sync cost model: how much a doc would be asked to write.
    let mut log = Log::new("r");
    log.created().launched().phase("p");
    for a in 0..20 {
        log.actor("a", a);
    }
    for i in 0..200u32 {
        log.queued("q", i, "a", i % 20)
            .step("q", i, "dispatched")
            .step("q", i, "executing")
            .settled("q", i, NodeOutcome::Ok);
    }
    log.run_settled(WorkflowStatus::Completed);
    let (_, deltas) = fold(&log.events);
    let total: usize = deltas
        .iter()
        .map(|d| serde_json::to_vec(d).unwrap().len())
        .sum();
    let largest = deltas
        .iter()
        .map(|d| serde_json::to_vec(d).unwrap().len())
        .max()
        .unwrap();
    // One event touches one node (+ its actor): never a whole-state write.
    assert!(largest < 6000, "largest delta {largest} bytes");
    let per_node = total / 200;
    assert!(per_node < 4000, "{per_node} bytes of delta per node");
    println!(
        "200-node run: {} deltas, {total} bytes, largest {largest}",
        deltas.len()
    );
}

#[test]
fn a_run_created_from_a_saved_workflow_carries_it_in_the_header_and_old_events_still_load() {
    let mut log = Log::new("r1");
    log.push(WorkflowEventKind::RunCreated {
        name: "pr-review".into(),
        chat_id: "chat".into(),
        script_hash: "h".into(),
        resumed_from: None,
        graph: None,
        concurrency_ceiling: 4,
        saved: Some(zeron_proto::SavedRunRef {
            name: "pr-review".into(),
            scope: zeron_proto::SavedScope::Global,
        }),
    });
    let (state, _) = fold(&log.events);
    let h = &state.runs[0].header;
    assert_eq!(h.saved_name.as_deref(), Some("pr-review"));
    assert_eq!(h.saved_scope, Some(zeron_proto::SavedScope::Global));

    // A journal written before saved workflows existed has no such field.
    let old =
        r#"{"type":"runCreated","name":"x","chatId":"c","scriptHash":"h","concurrencyCeiling":2}"#;
    let kind: WorkflowEventKind = serde_json::from_str(old).unwrap();
    let mut log = Log::new("r2");
    log.push(kind);
    let (state, _) = fold(&log.events);
    assert_eq!(state.runs[0].header.saved_name, None);
    assert_eq!(state.runs[0].header.saved_scope, None);
}
