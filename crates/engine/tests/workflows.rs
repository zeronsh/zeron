//! Dynamic workflows against a real engine core with a [`FakeAsk`] standing
//! in for the child chats: scheduling, journal and resume, approval, delivery,
//! restart reconciliation, budgets, escalation and the goal interplay. The
//! child-chat mechanics themselves have their own tests (`ask_child.rs`,
//! `workflow_e2e.rs`).

mod support;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use support::*;
use zeron_engine::ask::{AskError, AskUsage, FakeAsk, FakeReply};
use zeron_engine::workflow::{
    Approval, Approver, Catalog, StartError, StartRequest, Tuning, WorkflowService,
};
use zeron_proto::{
    ChatConfig, HarnessId, MessageOrigin, NodeOutcome, SandboxLevel, UserInputQuestion,
    WorkflowEventKind, WorkflowEventMarker, WorkflowRun, WorkflowStatus, WorkflowStopReason,
};

const CHAT: &str = "main";

struct Rig {
    env: Env,
    ask: Arc<FakeAsk>,
    svc: WorkflowService,
    approver: Arc<ScriptedApprover>,
    project: std::path::PathBuf,
    /// The test runs on a paused clock: waiting must advance it (tokio does
    /// not auto-advance while the script's blocking thread is running).
    paused: bool,
    _dir: tempfile::TempDir,
}

struct ScriptedApprover {
    answer: Mutex<Approval>,
    asked: Mutex<Vec<UserInputQuestion>>,
}

#[async_trait]
impl Approver for ScriptedApprover {
    async fn approve(&self, _chat: &str, question: UserInputQuestion) -> Approval {
        self.asked.lock().unwrap().push(question);
        self.answer.lock().unwrap().clone()
    }
}

struct Catalogue(Vec<HarnessId>);

#[async_trait]
impl Catalog for Catalogue {
    async fn check(&self, harness: HarnessId, _model: Option<&str>) -> Result<(), String> {
        if self.0.contains(&harness) {
            Ok(())
        } else {
            Err(format!(
                "harness {harness:?} is not installed on this device"
            ))
        }
    }
}

fn quick_agent() -> Handler {
    Arc::new(|_, request, out, _controls| {
        text_turn(
            &out,
            &format!("read: {}", request.prompt.len()),
            Some((10, 5)),
        );
    })
}

fn rig() -> Rig {
    rig_with(quick_agent())
}

fn rig_with(handler: Handler) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let env = assemble(&dir.path().join("data"), Default::default(), handler);
    let space = "space-main".to_string();
    env.core
        .workspace
        .create_space(
            &space,
            &env.core.device_id,
            &project.to_string_lossy(),
            None,
            false,
        )
        .unwrap();
    env.core
        .workspace
        .create_chat(
            CHAT,
            Some(&space),
            None,
            Some(ChatConfig {
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            Some(project.to_string_lossy().into_owned()),
        )
        .unwrap();
    env.core.workspace.rename_chat(CHAT, "Parent").unwrap();
    let ask = FakeAsk::new();
    env.core.doc_host.set_ask_backend(ask.clone());
    let svc = env.core.doc_host.workflows().expect("workflow service");
    let approver = Arc::new(ScriptedApprover {
        answer: Mutex::new(Approval::Approved),
        asked: Mutex::new(Vec::new()),
    });
    svc.set_approver(approver.clone());
    svc.set_catalog(Arc::new(Catalogue(vec![
        HarnessId::Mock,
        HarnessId::ClaudeCode,
    ])));
    Rig {
        env,
        ask,
        svc,
        approver,
        project,
        paused: false,
        _dir: dir,
    }
}

fn text(s: &str) -> FakeReply {
    FakeReply::Result(json!({ "text": s }))
}

fn start(script: &str) -> StartRequest {
    StartRequest {
        script: Some(script.into()),
        name: Some("Demo".into()),
        args: json!({}),
        ..StartRequest::default()
    }
}

/// Poll with a REAL-time deadline: under a paused clock the virtual one runs
/// ahead of the blocking script thread.
async fn wait_real<F: FnMut() -> bool>(paused: bool, mut done: F, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        if paused {
            // Let the blocking thread run, then let virtual time pass.
            std::thread::sleep(Duration::from_millis(2));
            tokio::time::advance(Duration::from_secs(20)).await;
        } else {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::task::yield_now().await;
    }
}

async fn wait_settled(rig: &Rig, run_id: &str) -> WorkflowRun {
    wait_real(
        rig.paused,
        || {
            rig.svc
                .get(run_id)
                .map(|v| v.run.header.status.is_settled())
                .unwrap_or(false)
        },
        "the run to settle",
    )
    .await;
    rig.svc.get(run_id).unwrap().run
}

async fn wait_delivered(rig: &Rig, run_id: &str) {
    let store = rig.svc.store().clone();
    wait_real(
        rig.paused,
        || {
            store
                .read_meta(run_id)
                .map(|m| m.delivered)
                .unwrap_or(false)
        },
        "the completion message to be delivered",
    )
    .await;
}

fn workflow_messages(rig: &Rig) -> Vec<(String, MessageOrigin)> {
    rig.env
        .user_text(CHAT)
        .into_iter()
        .filter_map(|(t, o)| match o {
            Some(o @ MessageOrigin::Workflow { .. }) => Some((t, o)),
            _ => None,
        })
        .collect()
}

const REVIEW: &str = r#"
def confirm(f):
    return agent("verifier").ask("CONFIRM " + f, schema = schema.obj({"ok": schema.bool()})).result()

def main(args):
    phase("review")
    reviewers = [agent("r" + str(i)) for i in range(3)]
    hs = [r.ask("REVIEW " + str(i)) for i, r in enumerate(reviewers)]
    findings = results(hs)
    report({"found": len(findings)})
    phase("confirm")
    checks = pmap(confirm, findings)
    phase("gate")
    gate = run("sh", ["-c", "echo gate-ok"])
    artifact.markdown("summary", "Summary", "found " + str(len(findings)))
    return {"findings": findings, "confirmed": len([c for c in checks if c["ok"]]), "gate": gate.stdout.strip()}
"#;

fn review_handler(rig: &Rig) {
    rig.ask.on_call(|call| {
        let p = &call.spec.prompt;
        Some(if p.contains("CONFIRM") {
            FakeReply::Result(json!({"ok": true}))
        } else {
            text("a finding")
        })
    });
}

// ── the happy path ────────────────────────────────────────────────────────

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_runs_end_to_end_and_its_result_reaches_the_parent_once() {
    let rig = rig();
    review_handler(&rig);
    let out = rig.svc.start(CHAT, start(REVIEW)).await.expect("starts");
    assert_eq!(out.graph.phase_names(), ["review", "confirm", "gate"]);
    assert!(
        out.draft_path
            .as_deref()
            .unwrap()
            .starts_with(".zeron/workflow-drafts/demo-")
    );
    assert!(
        rig.project
            .join(out.draft_path.as_deref().unwrap())
            .exists()
    );

    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(
        run.header.status,
        WorkflowStatus::Completed,
        "{:?}",
        run.header
    );
    assert_eq!(run.actors.len(), 6, "3 reviewers + 3 verifiers");
    assert_eq!(run.nodes.len(), 7, "6 asks + 1 command");
    assert!(run.nodes.iter().all(|n| n.outcome == Some(NodeOutcome::Ok)));
    assert_eq!(run.reports.len(), 1);
    assert_eq!(run.artifacts.len(), 1);
    assert_eq!(run.artifacts[0].id, "summary");
    assert_eq!(
        run.header
            .phases
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["review", "confirm", "gate"]
    );
    assert!(
        run.header
            .result_preview
            .as_deref()
            .unwrap()
            .contains("gate-ok")
    );

    // The completion message: queued once, with the origin, delivered as a turn.
    wait_delivered(&rig, &out.run_id).await;
    wait_for(
        || workflow_messages(&rig).len() == 1,
        "the completion message in the transcript",
    )
    .await;
    let (body, origin) = workflow_messages(&rig).remove(0);
    assert!(body.starts_with("[Workflow completed] Demo"), "{body}");
    assert!(
        body.contains("gate-ok") && body.contains("summary (markdown)"),
        "{body}"
    );
    assert!(matches!(
        origin,
        MessageOrigin::Workflow {
            status: WorkflowStatus::Completed,
            ..
        }
    ));
    // Markers: started and completed.
    let markers: Vec<_> = rig
        .env
        .entries(CHAT)
        .into_iter()
        .filter_map(|e| match e.origin {
            Some(MessageOrigin::WorkflowEvent { marker, .. }) => Some(marker),
            _ => None,
        })
        .collect();
    assert_eq!(
        markers,
        [WorkflowEventMarker::Started, WorkflowEventMarker::Completed]
    );
    // The synced projection carries the same run (children hidden, one level down).
    rig.svc.flush();
    let doc = rig.env.core.doc_host.open(CHAT).unwrap();
    let synced = doc.doc().workflow_runs();
    assert_eq!(synced.run(&out.run_id).unwrap().nodes.len(), 7);
    assert_eq!(
        synced.run(&out.run_id).unwrap().header.status,
        WorkflowStatus::Completed
    );
    // Full results live in the journal only.
    let view = rig.svc.get(&out.run_id).unwrap();
    assert_eq!(
        view.result.unwrap()["findings"].as_array().unwrap().len(),
        3
    );
    assert_eq!(view.reports, vec![json!({"found": 3})]);

    // A restart (or a second reconcile) never delivers it again.
    rig.svc.reconcile();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(workflow_messages(&rig).len(), 1);
}

// ── approval ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn approval_shows_the_graph_and_a_denial_creates_a_settled_run_and_no_work() {
    let rig = rig();
    *rig.approver.answer.lock().unwrap() = Approval::Denied("not now".into());
    let err = rig.svc.start(CHAT, start(REVIEW)).await.unwrap_err();
    assert!(
        matches!(&err, StartError::Denied(m) if m == "not now"),
        "{err:?}"
    );
    assert!(rig.ask.calls().is_empty(), "nothing ran");
    let asked = rig.approver.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    let q = &asked[0];
    assert!(
        q.question
            .contains("Phases: review (1) → confirm (1) → gate (1)"),
        "{}",
        q.question
    );
    assert!(
        q.question.contains("$ sh -c echo gate-ok"),
        "{}",
        q.question
    );
    assert_eq!(q.options, ["Run workflow", "Deny"]);
    let meta = q.meta.as_ref().unwrap();
    assert_eq!(meta["kind"], "workflowApproval");
    assert_eq!(meta["graph"]["commands"][0]["command"], "sh");
    // The run is on record as denied, and a marker says so; no result message.
    let runs = rig.svc.list(Some(CHAT));
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, WorkflowStatus::Stopped);
    assert_eq!(runs[0].stop_reason, Some(WorkflowStopReason::Denied));
    assert!(!runs[0].resumable);
    assert!(workflow_messages(&rig).is_empty());
    assert!(
        rig.svc.resume(&runs[0].run_id, None, false).await.is_err(),
        "a denied run cannot be resumed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_script_with_problems_returns_diagnostics_and_creates_nothing() {
    let rig = rig();
    let err = rig
        .svc
        .start(
            CHAT,
            start("def main(args):\n    phase(\"empty\")\n    return 1\n"),
        )
        .await
        .unwrap_err();
    let StartError::Diagnostics(d) = err else {
        panic!("{err:?}")
    };
    assert!(
        d[0].to_string().starts_with("workflow.star:2:5"),
        "{}",
        d[0]
    );
    assert!(rig.svc.list(None).is_empty());
    assert!(
        rig.approver.asked.lock().unwrap().is_empty(),
        "no approval dialog for a broken script"
    );
    // `path` scripts are read inside the project only.
    std::fs::create_dir_all(rig.project.join("wf")).unwrap();
    std::fs::write(
        rig.project.join("wf/a.star"),
        "def main(args):\n    return 7\n",
    )
    .unwrap();
    let ok = rig
        .svc
        .start(
            CHAT,
            StartRequest {
                path: Some("wf/a.star".into()),
                ..StartRequest::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ok.name, "a");
    wait_settled(&rig, &ok.run_id).await;
    let escape = rig
        .svc
        .start(
            CHAT,
            StartRequest {
                path: Some("../secret.star".into()),
                ..StartRequest::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(escape, StartError::Invalid(_)));
    let both = rig
        .svc
        .start(
            CHAT,
            StartRequest {
                path: Some("wf/a.star".into()),
                script: Some("x".into()),
                ..StartRequest::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(both, StartError::Invalid(_)));
}

// ── scheduling ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_concurrency_cap_bounds_asks_in_flight() {
    let rig = rig();
    // Every ask takes 80 ms. With a cap of 3, the fourth start can only follow
    // the first finish, so no four starts fall within one ask's duration.
    let starts = Arc::new(Mutex::new(Vec::new()));
    let seen = starts.clone();
    rig.ask.on_call(move |_| {
        seen.lock().unwrap().push(std::time::Instant::now());
        Some(FakeReply::After(
            Duration::from_millis(80),
            Box::new(text("ok")),
        ))
    });
    let script = r#"
def main(args):
    phase("fan")
    hs = [agent("a" + str(i)).ask("work " + str(i)) for i in range(10)]
    return len(results(hs))
"#;
    let mut req = start(script);
    req.max_concurrency = Some(3);
    let out = rig.svc.start(CHAT, req).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Completed);
    assert_eq!(rig.ask.calls().len(), 10);
    let mut starts = starts.lock().unwrap().clone();
    starts.sort();
    for window in starts.windows(4) {
        let span = window[3] - window[0];
        assert!(
            span >= Duration::from_millis(70),
            "four asks started within {span:?}: the cap of 3 was exceeded"
        );
    }
    assert!(
        starts
            .windows(2)
            .any(|w| w[1] - w[0] < Duration::from_millis(40)),
        "asks did overlap"
    );
    assert_eq!(run.header.concurrency.ceiling, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_actor_is_fifo_and_keeps_one_child_across_asks() {
    let rig = rig();
    rig.ask.on_call(|_| {
        Some(FakeReply::After(
            Duration::from_millis(30),
            Box::new(text("ok")),
        ))
    });
    let script = r#"
def main(args):
    phase("p")
    a = agent("worker", persona = "Be terse.")
    hs = [a.ask("STEP " + str(i)) for i in range(4)]
    b = agent("other")
    hb = b.ask("OTHER")
    wait_all(hs)
    hb.result()
    return None
"#;
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    wait_settled(&rig, &out.run_id).await;
    let calls = rig.ask.calls();
    let worker: Vec<_> = calls.iter().filter(|c| c.spec.label == "worker").collect();
    assert_eq!(worker.len(), 4);
    let order: Vec<_> = worker
        .iter()
        .map(|c| {
            c.spec
                .prompt
                .split("STEP ")
                .nth(1)
                .unwrap()
                .chars()
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(order, ['0', '1', '2', '3'], "script order is run order");
    // The first ask creates the child; every later ask continues it.
    assert!(worker[0].spec.reuse_child.is_none());
    let reused: BTreeSet<_> = worker
        .iter()
        .skip(1)
        .map(|c| c.spec.reuse_child.clone())
        .collect();
    assert_eq!(reused.len(), 1, "one child for every later ask: {reused:?}");
    assert!(reused.iter().next().unwrap().is_some());
    // Standing instructions and the persona go with the first ask only.
    assert!(
        worker[0]
            .spec
            .prompt
            .contains("subagent inside a dynamic workflow")
    );
    assert!(worker[0].spec.prompt.contains("Be terse."));
    assert!(
        !worker[1]
            .spec
            .prompt
            .contains("subagent inside a dynamic workflow")
    );
    assert!(
        worker
            .iter()
            .all(|c| c.spec.persistent && c.spec.escalation.is_some())
    );
    assert_eq!(worker[0].spec.title.as_deref(), Some("Demo · worker"));
    // The other actor got its own child.
    let other = calls.iter().find(|c| c.spec.label == "other").unwrap();
    assert!(other.spec.reuse_child.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_ask_rejects_only_that_ask() {
    let rig = rig();
    rig.ask.on_call(|call| {
        Some(if call.spec.prompt.contains("BAD") {
            FakeReply::Fail(AskError::NoResult)
        } else {
            text("fine")
        })
    });
    let script = r#"
def main(args):
    phase("p")
    a = agent("w")
    good = a.ask("GOOD").result()
    bad = a.ask("BAD").result()
    again = a.ask("GOOD2").result()
    return [good.ok, bad.ok, bad.error, again.ok]
"#;
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Completed);
    let view = rig.svc.get(&out.run_id).unwrap();
    let result = view.result.unwrap();
    assert_eq!(result[0], true);
    assert_eq!(result[1], false);
    assert!(
        result[2]
            .as_str()
            .unwrap()
            .contains("without submitting a result")
    );
    assert_eq!(result[3], true);
    assert_eq!(
        run.nodes
            .iter()
            .filter(|n| n.outcome == Some(NodeOutcome::Failed))
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unavailable_harness_fails_the_ask_not_the_run() {
    let rig = rig();
    rig.ask.on_call(|_| Some(text("ok")));
    let script = r#"
def main(args):
    phase("p")
    ok = agent("a").ask("one").result()
    nope = agent("b", harness = "codex").ask("two").result()
    junk = agent("c", harness = "no-such-agent").ask("three").result()
    return [ok.ok, nope.ok, nope.error, junk.ok, junk.error]
"#;
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Completed);
    let r = rig.svc.get(&out.run_id).unwrap().result.unwrap();
    assert_eq!(r[0], true);
    assert_eq!(r[1], false);
    assert!(r[2].as_str().unwrap().contains("not installed"), "{r}");
    assert_eq!(r[3], false);
    assert!(r[4].as_str().unwrap().contains("unknown harness"), "{r}");
    assert_eq!(
        rig.ask.calls().len(),
        1,
        "only the valid ask reached a child"
    );
    // …and the same check at start for run-level defaults.
    let mut req = start("def main(args):\n    return 1\n");
    req.harness = Some("codex".into());
    assert!(matches!(
        rig.svc.start(CHAT, req).await.unwrap_err(),
        StartError::Invalid(_)
    ));
}

// ── provider faults ───────────────────────────────────────────────────────

#[tokio::test(start_paused = true)]
async fn transient_errors_are_redriven_with_backoff_and_never_reach_the_script() {
    let mut rig = rig();
    rig.paused = true;
    let n = Arc::new(AtomicUsize::new(0));
    let counter = n.clone();
    let times = Arc::new(Mutex::new(Vec::new()));
    let seen = times.clone();
    rig.ask.on_call(move |_| {
        seen.lock().unwrap().push(tokio::time::Instant::now());
        Some(if counter.fetch_add(1, Ordering::SeqCst) < 3 {
            FakeReply::Fail(AskError::TurnFailed("503 Service Unavailable".into()))
        } else {
            text("finally")
        })
    });
    let started = tokio::time::Instant::now();
    let script = "def main(args):\n    phase(\"p\")\n    r = agent(\"a\").ask(\"x\").result()\n    return [r.ok, r.value]\n";
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Completed);
    assert_eq!(
        rig.svc.get(&out.run_id).unwrap().result.unwrap(),
        json!([true, "finally"])
    );
    assert_eq!(n.load(Ordering::SeqCst), 4, "3 failures + 1 success");
    // Backoff 2s, 4s, 8s (±25 %) of virtual time between the redrives (the
    // test advances the clock in coarse steps, so gaps are at least that).
    let times = times.lock().unwrap().clone();
    let gaps: Vec<_> = times.windows(2).map(|w| w[1] - w[0]).collect();
    for (gap, min) in gaps.iter().zip([1.5, 3.0, 6.0]) {
        assert!(gap.as_secs_f64() >= min, "gaps {gaps:?}");
    }
    assert!(started.elapsed() >= Duration::from_secs(10));
    assert_eq!(run.nodes[0].outcome, Some(NodeOutcome::Ok));
}

#[tokio::test(start_paused = true)]
async fn rate_limits_throttle_the_model_and_show_it() {
    let mut rig = rig();
    rig.paused = true;
    let n = Arc::new(AtomicUsize::new(0));
    let c = n.clone();
    rig.ask.on_call(move |_| {
        Some(if c.fetch_add(1, Ordering::SeqCst) == 0 {
            FakeReply::Fail(AskError::TurnFailed(
                "429 Too Many Requests; retry-after: 7".into(),
            ))
        } else {
            text("ok")
        })
    });
    let mut events = rig.svc.subscribe();
    let script =
        "def main(args):\n    phase(\"p\")\n    return agent(\"a\").ask(\"x\").result().ok\n";
    let mut req = start(script);
    req.max_concurrency = Some(4);
    let started = tokio::time::Instant::now();
    let out = rig.svc.start(CHAT, req).await.unwrap();
    wait_settled(&rig, &out.run_id).await;
    assert!(
        started.elapsed() >= Duration::from_secs(7),
        "the Retry-After was honoured"
    );
    let mut throttled = false;
    while let Ok(e) = events.try_recv() {
        if let WorkflowEventKind::ConcurrencyChanged { concurrency } = e.kind {
            throttled |= concurrency.throttled;
        }
    }
    assert!(throttled, "the cap shows as throttled after a rate limit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deterministic_provider_error_stops_the_run_and_resume_continues_without_reasking() {
    let rig = rig();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    let broken = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let b = broken.clone();
    rig.ask.on_call(move |call| {
        c.fetch_add(1, Ordering::SeqCst);
        Some(
            if call.spec.prompt.contains("SECOND") && b.load(Ordering::SeqCst) {
                FakeReply::Fail(AskError::TurnFailed(
                    "401 Unauthorized: invalid API key".into(),
                ))
            } else {
                text(if call.spec.prompt.contains("FIRST") {
                    "one"
                } else {
                    "two"
                })
            },
        )
    });
    let script = r#"
def main(args):
    phase("p")
    a = agent("a")
    first = a.ask("FIRST").result()
    report("first done: " + str(first.value))
    second = a.ask("SECOND").result()
    return [first.value, second.value]
"#;
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Stopped);
    assert_eq!(run.header.stop_reason, Some(WorkflowStopReason::Provider));
    assert!(
        run.header
            .stop_detail
            .as_deref()
            .unwrap()
            .contains("authentication failed"),
        "{:?}",
        run.header.stop_detail
    );
    assert!(run.header.resumable);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    wait_delivered(&rig, &out.run_id).await;
    wait_for(|| workflow_messages(&rig).len() == 1, "the stop notice").await;
    assert!(workflow_messages(&rig)[0].0.contains("[Workflow stopped]"));

    // The user fixes the key; resume replays FIRST from the journal.
    broken.store(false, Ordering::SeqCst);
    let resumed = rig
        .svc
        .resume(&out.run_id, Some(json!({})), false)
        .await
        .unwrap();
    let run2 = wait_settled(&rig, &resumed.run_id).await;
    assert_eq!(
        run2.header.status,
        WorkflowStatus::Completed,
        "{:?}",
        run2.header
    );
    assert_eq!(
        run2.header.resumed_from.as_deref(),
        Some(out.run_id.as_str())
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3, "FIRST was not asked again");
    assert_eq!(run2.header.usage.nodes_cached, 1);
    assert!(run2.nodes.iter().any(|n| n.cached));
    assert_eq!(
        rig.svc.get(&resumed.run_id).unwrap().result.unwrap(),
        json!(["one", "two"])
    );
    // Different inputs are refused; the old run cannot be resumed twice at once
    // and a finished one not at all.
    assert!(
        rig.svc
            .resume(&out.run_id, Some(json!({"x": 1})), false)
            .await
            .is_err()
    );
    assert!(rig.svc.resume(&resumed.run_id, None, false).await.is_err());
}

// ── stopping, budgets, stalls ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_in_flight_asks_and_the_run_is_resumable() {
    let rig = rig();
    rig.ask.on_call(|_| Some(FakeReply::Hang));
    let script = "def main(args):\n    phase(\"p\")\n    hs = [agent(\"a\" + str(i)).ask(\"x\") for i in range(3)]\n    return len(wait_all(hs))\n";
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    wait_for(|| rig.ask.calls().len() == 3, "all three asks in flight").await;
    assert!(rig.svc.stop(&out.run_id, Some("changed my mind")).unwrap());
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Stopped);
    assert_eq!(run.header.stop_reason, Some(WorkflowStopReason::User));
    assert_eq!(run.header.stop_detail.as_deref(), Some("changed my mind"));
    assert!(run.header.resumable);
    assert_eq!(rig.ask.cancelled(), 3);
    assert!(
        run.nodes
            .iter()
            .all(|n| n.outcome == Some(NodeOutcome::Cancelled))
    );
    wait_delivered(&rig, &out.run_id).await; // the executor has fully wound down
    assert!(
        !rig.svc.stop(&out.run_id, None).unwrap(),
        "already finished"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn budgets_stop_a_run_with_a_reason() {
    let rig = rig();
    rig.ask.on_call(|_| {
        Some(FakeReply::ResultWithUsage(
            json!({"text": "x"}),
            AskUsage {
                input_tokens: 600,
                output_tokens: 400,
                elapsed_ms: 1,
                turns: 1,
            },
        ))
    });
    let looping = "def main(args):\n    phase(\"p\")\n    a = agent(\"a\")\n    for i in range(10):\n        a.ask(\"n\" + str(i)).result()\n    return 1\n";
    let mut req = start(looping);
    req.budgets.max_asks = Some(3);
    let out = rig.svc.start(CHAT, req).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Stopped);
    assert_eq!(run.header.stop_reason, Some(WorkflowStopReason::Budget));
    assert!(
        run.header
            .stop_detail
            .as_deref()
            .unwrap()
            .contains("ask budget of 3")
    );
    assert_eq!(rig.ask.calls().len(), 3);

    let mut req = start(looping);
    req.budgets.max_tokens = Some(2500);
    let out = rig.svc.start(CHAT, req).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.stop_reason, Some(WorkflowStopReason::Budget));
    assert!(
        run.header
            .stop_detail
            .as_deref()
            .unwrap()
            .contains("token budget")
    );
    assert!(run.header.usage.total_tokens() >= 2500);
}

#[tokio::test(start_paused = true)]
async fn a_long_silence_raises_the_stall_notice_and_a_success_lifts_it() {
    let mut rig = rig();
    rig.paused = true;
    rig.svc.set_tuning(Tuning {
        stall_check: Duration::from_secs(60),
        ..Tuning::default()
    });
    rig.ask.on_call(|_| {
        Some(FakeReply::After(
            Duration::from_secs(25 * 60),
            Box::new(text("late")),
        ))
    });
    let mut events = rig.svc.subscribe();
    let script =
        "def main(args):\n    phase(\"p\")\n    return agent(\"a\").ask(\"x\").result().ok\n";
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    wait_settled(&rig, &out.run_id).await;
    let mut kinds = Vec::new();
    while let Ok(e) = events.try_recv() {
        if matches!(
            e.kind,
            WorkflowEventKind::Stalled | WorkflowEventKind::Unstalled
        ) {
            kinds.push(matches!(e.kind, WorkflowEventKind::Stalled));
        }
    }
    assert_eq!(kinds, [true, false], "stalled once, then recovered");
}

// ── escalation ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_escalation_reaches_the_parent_and_the_answer_unblocks_only_that_ask() {
    let rig = rig();
    rig.ask.on_call(|call| {
        Some(if call.spec.prompt.contains("BLOCKED") {
            FakeReply::Escalate {
                question: "Which database?".into(),
                context: "two are configured".into(),
                then: Box::new(text("went with the answer")),
            }
        } else {
            text("independent")
        })
    });
    let script = r#"
def main(args):
    phase("p")
    stuck = agent("stuck").ask("BLOCKED task")
    free = agent("free").ask("free task")
    return [free.result().value, stuck.result().value]
"#;
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    wait_for(
        || {
            rig.svc
                .get(&out.run_id)
                .map(|v| v.run.pending_questions.len() == 1)
                .unwrap_or(false)
        },
        "the parked question",
    )
    .await;
    let view = rig.svc.get(&out.run_id).unwrap();
    let q = view.run.pending_questions[0].clone();
    assert_eq!(q.question, "Which database?");
    assert_eq!(q.actor_name, "stuck");
    // The other actor finished meanwhile: only the escalating ask is parked.
    wait_for(
        || {
            rig.svc.get(&out.run_id).unwrap().run.nodes.iter().any(|n| {
                n.outcome == Some(NodeOutcome::Ok) && n.instructions_head.starts_with("free")
            })
        },
        "the free ask to finish",
    )
    .await;
    // The parent agent is told, with the exact tool to answer with.
    wait_for(
        || {
            workflow_messages(&rig)
                .iter()
                .any(|(t, _)| t.contains("[Workflow question]"))
        },
        "the question in the parent chat",
    )
    .await;
    let note = workflow_messages(&rig)
        .into_iter()
        .find(|(t, _)| t.contains("[Workflow question]"))
        .unwrap()
        .0;
    assert!(
        note.contains(&format!(
            "resolve_workflow_question {{run_id: \"{}\", qid: \"{}\"",
            out.run_id, q.qid
        )),
        "{note}"
    );
    // Wrong qid / empty answers are refused; the real answer resumes it.
    assert!(
        rig.svc
            .resolve_question(&out.run_id, "nope", "x")
            .await
            .is_err()
    );
    assert!(
        rig.svc
            .resolve_question(&out.run_id, &q.qid, "  ")
            .await
            .is_err()
    );
    rig.svc
        .resolve_question(&out.run_id, &q.qid, "Postgres")
        .await
        .unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Completed);
    assert_eq!(rig.ask.escalation_answers(), ["Postgres"]);
    assert!(run.pending_questions.is_empty());
    assert_eq!(
        rig.svc.get(&out.run_id).unwrap().result.unwrap(),
        json!(["independent", "went with the answer"])
    );
}

// ── restart ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_left_running_by_a_dead_engine_becomes_interrupted_and_resumable() {
    let rig = rig();
    rig.ask.on_call(|_| Some(FakeReply::Hang));
    let script =
        "def main(args):\n    phase(\"p\")\n    return agent(\"a\").ask(\"x\").result().ok\n";
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    wait_for(|| rig.ask.calls().len() == 1, "the ask in flight").await;
    // Simulate a crash: a second service over the same data dir never saw the
    // run live (a restart), while the first is still "executing" it.
    let restarted = WorkflowService::new(
        rig.env.core.doc_host.clone(),
        rig.env.core.sessions.clone(),
        rig.env.core.workspace.clone(),
        &rig.svc.store().base(),
    );
    restarted.reconcile();
    let meta = restarted.store().read_meta(&out.run_id).unwrap();
    assert_eq!(meta.status, WorkflowStatus::Stopped);
    assert_eq!(meta.stop_reason, Some(WorkflowStopReason::Interrupted));
    let view = restarted.get(&out.run_id).unwrap();
    assert_eq!(view.run.header.status, WorkflowStatus::Stopped);
    assert!(view.run.header.resumable);
    wait_for(
        || workflow_messages(&rig).len() == 1,
        "the interruption notice",
    )
    .await;
    // Idempotent: another restart changes nothing and sends nothing more.
    restarted.reconcile();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(workflow_messages(&rig).len(), 1);
    // Clean up the still-running first executor.
    rig.svc.stop(&out.run_id, None).ok();
}

// ── journal, projection, limits ───────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_journal_rebuilds_the_same_state_the_chat_doc_holds() {
    let rig = rig();
    review_handler(&rig);
    #[cfg(unix)]
    let out = rig.svc.start(CHAT, start(REVIEW)).await.unwrap();
    #[cfg(not(unix))]
    let out = rig.svc.start(CHAT, start("def main(args):\n    phase(\"p\")\n    return agent(\"a\").ask(\"REVIEW\").result().ok\n")).await.unwrap();
    wait_settled(&rig, &out.run_id).await;
    rig.svc.flush();
    let replay = rig.svc.store().load_replay(&out.run_id).unwrap();
    let mut folded = zeron_proto::WorkflowRunsState::default();
    for e in &replay.events {
        zeron_workflow::reduce(&mut folded, e);
    }
    let from_journal = folded.run(&out.run_id).unwrap().clone();
    let doc = rig.env.core.doc_host.open(CHAT).unwrap();
    let from_doc = doc.doc().workflow_runs().run(&out.run_id).unwrap().clone();
    let norm = |mut r: WorkflowRun| {
        r.header.last_event_sequence = 0;
        r
    };
    assert_eq!(norm(from_doc), norm(from_journal));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_200_node_run_writes_a_bounded_amount_to_the_doc() {
    let rig = rig();
    rig.ask.on_call(|_| Some(text("ok")));
    let script = r#"
def main(args):
    phase("bulk")
    hs = [agent("a" + str(i % 20)).ask("task " + str(i)) for i in range(200)]
    return len(results(hs))
"#;
    let mut req = start(script);
    req.max_concurrency = Some(8);
    req.budgets.max_asks = Some(1000);
    let out = rig.svc.start(CHAT, req).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Completed);
    assert_eq!(run.nodes.len(), 200);
    rig.svc.flush();
    let stats = rig.svc.projection_stats();
    println!("200-node run: {stats:?}");
    assert!(stats.events >= 1000, "{stats:?}");
    assert!(stats.doc_writes < 120, "writes are batched: {stats:?}");
    assert!(stats.delta_bytes < 2_500_000, "{stats:?}");
    let doc = rig.env.core.doc_host.open(CHAT).unwrap();
    assert_eq!(
        doc.doc()
            .workflow_runs()
            .run(&out.run_id)
            .unwrap()
            .nodes
            .len(),
        200
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn world_reads_are_journaled_confined_and_replayed() {
    let rig = rig();
    std::fs::write(rig.project.join("a.txt"), "first").unwrap();
    let script = r#"
def main(args):
    phase("p")
    a = agent("a")
    before = files.read("a.txt")
    a.ask("STEP").result()
    return [before, files.read("../outside")]
"#;
    rig.ask.on_call(|_| Some(text("ok")));
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    // `../outside` is an error: the script fails with the confinement message.
    assert_eq!(run.header.status, WorkflowStatus::Errored);
    assert!(
        run.header
            .error
            .as_deref()
            .unwrap()
            .contains("inside the project"),
        "{:?}",
        run.header.error
    );
    let replay = rig.svc.store().load_replay(&out.run_id).unwrap();
    assert_eq!(replay.reads.len(), 1, "the successful read was journaled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artifacts_are_listed_read_in_pages_and_the_ids_cannot_escape() {
    let rig = rig();
    rig.ask.on_call(|_| Some(text("ok")));
    std::fs::write(rig.project.join("notes.md"), "file artifact body").unwrap();
    let script = r#"
def main(args):
    phase("p")
    agent("a").ask("x").result()
    artifact.markdown("report", "Report", "hello world")
    artifact.table("tbl", "Table", ["a", "b"], [[1, 2], [3, 4]])
    artifact.metrics("m", "Metrics", {"tests": 12})
    artifact.file("notes", "Notes", "notes.md")
    return None
"#;
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.artifacts.len(), 4);
    let by_id: BTreeSet<_> = run.artifacts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(by_id, BTreeSet::from(["m", "notes", "report", "tbl"]));
    let tbl = run.artifacts.iter().find(|a| a.id == "tbl").unwrap();
    assert_eq!(
        (tbl.item_count, tbl.content_type.as_str()),
        (2, "application/json")
    );
    let page = rig
        .svc
        .artifact_read(&out.run_id, "report", None, 6, 5)
        .unwrap();
    assert_eq!(String::from_utf8(page.bytes).unwrap(), "world");
    assert_eq!(page.total, 11);
    let file = rig
        .svc
        .artifact_read(&out.run_id, "notes", None, 0, 100)
        .unwrap();
    assert_eq!(String::from_utf8(file.bytes).unwrap(), "file artifact body");
    for bad in ["../meta", "..", "a/b", ""] {
        assert!(
            rig.svc
                .artifact_read(&out.run_id, bad, None, 0, 10)
                .is_err(),
            "{bad:?}"
        );
    }
    assert!(
        rig.svc
            .artifact_read("../../etc", "report", None, 0, 10)
            .is_err()
    );
    // A script cannot publish a file from outside the project.
    let evil = "def main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n    artifact.file(\"x\", \"X\", \"../../etc/passwd\")\n    return 1\n";
    let out = rig.svc.start(CHAT, start(evil)).await.unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.status, WorkflowStatus::Errored);
}

// ── goals ─────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_workflow_defers_goal_verification() {
    let rig = rig();
    let verifier_calls = Arc::new(AtomicUsize::new(0));
    let vc = verifier_calls.clone();
    rig.ask.on_call(move |call| {
        if call.spec.label == "Verifier" {
            vc.fetch_add(1, Ordering::SeqCst);
            return Some(FakeReply::Result(
                json!({"passed": true, "reason": "all done"}),
            ));
        }
        Some(FakeReply::After(
            Duration::from_millis(1500),
            Box::new(text("slow work")),
        ))
    });
    let script =
        "def main(args):\n    phase(\"p\")\n    return agent(\"slow\").ask(\"x\").result().value\n";
    let out = rig.svc.start(CHAT, start(script)).await.unwrap();
    assert!(rig.svc.has_running_run(CHAT));
    rig.env.set_goal(CHAT, "Finish the job");
    // The goal's first round runs and ends within milliseconds, but the
    // controller must not judge it while the workflow's agents still work.
    wait_for(
        || rig.env.complete_turns(CHAT) >= 1,
        "the goal's first round",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(
        verifier_calls.load(Ordering::SeqCst),
        0,
        "verification waits for the workflow"
    );
    assert!(rig.env.goal(CHAT).unwrap().status.is_running());
    // Once the workflow settles (and its result message has been delivered)
    // verification proceeds and the goal completes.
    wait_settled(&rig, &out.run_id).await;
    wait_for(
        || {
            rig.env
                .goal(CHAT)
                .is_some_and(|g| g.status == zeron_proto::GoalStatus::Complete)
        },
        "the goal to complete after the workflow",
    )
    .await;
    assert_eq!(
        verifier_calls.load(Ordering::SeqCst),
        1,
        "the round is judged exactly once"
    );
    assert!(!rig.svc.has_running_run(CHAT));
}

// ── the sidebar's feed ────────────────────────────────────────────────────

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_activity_feed_lists_every_chats_runs_as_briefs_and_only_pushes_changes() {
    let rig = rig();
    review_handler(&rig);
    let mut feed = rig.svc.watch_activity();
    assert!(
        feed.borrow().chats.is_empty(),
        "nothing ran yet: an empty feed"
    );
    let out = rig.svc.start(CHAT, start(REVIEW)).await.expect("starts");
    let run = wait_settled(&rig, &out.run_id).await;
    wait_delivered(&rig, &out.run_id).await;
    // the feed saw the run, settled, with the same header the doc holds
    wait_real(
        rig.paused,
        || {
            feed.borrow()
                .chats
                .get(CHAT)
                .and_then(|runs| runs.first())
                .is_some_and(|b| b.header.status == WorkflowStatus::Completed)
        },
        "the feed to show the settled run",
    )
    .await;
    let briefs = feed.borrow_and_update().chats.get(CHAT).cloned().unwrap();
    assert_eq!(briefs.len(), 1);
    assert_eq!(briefs[0].header.run_id, out.run_id);
    assert_eq!(briefs[0].header.phases, run.header.phases);
    assert_eq!(briefs[0].pending_questions, 0);
    // a second flush with nothing new does not wake a watcher
    rig.svc.flush();
    assert!(!feed.has_changed().unwrap_or(true), "no change, no push");
}
