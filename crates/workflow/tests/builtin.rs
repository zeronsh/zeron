//! The bundled workflows run against a scripted host: they are also the
//! worked examples of the authoring guide's patterns (fresh eyes, independent
//! confirmation, deterministic gates), so they must stay correct.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};
use zeron_proto::saved_workflow::validate_args;
use zeron_workflow::host::{AskReply, ReadOp, RunReply};
use zeron_workflow::saved::{Builtin, builtins, parse};
use zeron_workflow::testing::{FakeHost, Recorded};
use zeron_workflow::{analyze, run_script};

fn builtin(name: &str) -> Builtin {
    *builtins()
        .iter()
        .find(|b| b.name == name)
        .unwrap_or_else(|| panic!("no built-in {name}"))
}

/// The arguments the engine would hand the script: declared defaults filled.
fn args_for(b: &Builtin, given: Value) -> Value {
    let meta = parse(b.name, Some(b.name), b.source).unwrap().meta;
    validate_args(&meta.args, &given).unwrap_or_else(|e| panic!("{e:?}"))
}

fn run_builtin(b: &Builtin, given: Value, host: &Arc<FakeHost>) -> Value {
    let analysis = analyze(b.name, b.source).unwrap_or_else(|d| panic!("{d:?}"));
    run_script(
        b.name,
        b.source,
        &args_for(b, given),
        &analysis,
        host.clone(),
        Arc::default(),
        &zeron_workflow::Limits::default(),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", b.name))
}

fn props(req: &zeron_workflow::host::AskRequest) -> Vec<String> {
    req.schema
        .as_ref()
        .and_then(|s| s["properties"].as_object().cloned())
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn there_are_three_and_each_has_a_distinct_name_and_description() {
    let names: Vec<_> = builtins().iter().map(|b| b.name).collect();
    assert_eq!(names, ["pr-review", "fix-until-green", "repo-audit"]);
    for b in builtins() {
        assert!(common::diagnostics(b.source).is_empty(), "{}", b.name);
    }
}

#[test]
fn defaults_alone_are_enough_to_run_each_of_them() {
    for b in builtins() {
        let meta = parse(b.name, Some(b.name), b.source).unwrap().meta;
        assert!(
            meta.args.iter().all(|a| !a.required),
            "{}: built-ins must run with no arguments",
            b.name
        );
        validate_args(&meta.args, &json!({})).unwrap();
    }
}

fn finding(n: u32, severity: &str) -> Value {
    json!({
        "where": format!("src/lib.rs:{n}"),
        "what": format!("problem {n}"),
        "evidence": "read it",
        "severity": severity,
    })
}

#[test]
fn pr_review_confirms_every_finding_with_a_fresh_agent_and_ranks_by_severity() {
    let b = builtin("pr-review");
    let host = FakeHost::new();
    host.on_read(|op| match op {
        ReadOp::GitChangedFiles { base } => {
            assert_eq!(base.as_deref(), Some("main"));
            Ok(json!(["src/a.rs", "src/b.rs"]))
        }
        other => panic!("unexpected read {other:?}"),
    });
    host.on_ask(|req| {
        let p = props(req);
        if p.contains(&"findings".to_owned()) {
            AskReply::ok(json!({"findings": [finding(1, "low"), finding(2, "high")]}))
        } else if p.contains(&"confirmed".to_owned()) {
            // The low one cannot be reproduced.
            AskReply::ok(
                json!({"confirmed": !req.instructions.contains("problem 1"), "evidence": "ran it"}),
            )
        } else {
            AskReply::ok(json!({"conclusion": "One real problem."}))
        }
    });
    let out = run_builtin(&b, json!({"max_findings": 4}), &host);
    assert_eq!(out["conclusion"], "One real problem.");
    let findings = out["findings"].as_array().unwrap();
    assert_eq!(
        findings.len(),
        4,
        "3 reviewers x 2 findings, capped at max_findings"
    );
    assert_eq!(findings[0]["severity"], "high", "most severe first");
    // Three high findings (one per reviewer) outrank the lows; the one low
    // that made the cut cannot be reproduced.
    assert_eq!(
        findings
            .iter()
            .filter(|f| f["status"] == "verified")
            .count(),
        3
    );
    assert_eq!(
        findings
            .iter()
            .filter(|f| f["status"] == "unconfirmed")
            .count(),
        1
    );
    for f in findings {
        for k in ["where", "what", "evidence", "status", "severity"] {
            assert!(f.get(k).is_some(), "{k}");
        }
    }
    let asks = host.asks();
    // 3 reviewers + 4 checkers + 1 judge; reviewers, checkers and the judge never write.
    assert_eq!(asks.len(), 3 + 4 + 1);
    assert!(asks.iter().all(|a| a.read_only));
    // Checkers are fresh actors, distinct from every reviewer.
    let actors: std::collections::HashSet<_> = asks.iter().map(|a| a.actor.to_string()).collect();
    assert_eq!(actors.len(), asks.len());
    assert!(asks[0].instructions.contains("src/a.rs") && asks[0].instructions.contains("`main`"));
    let phases: Vec<_> = host
        .recorded()
        .into_iter()
        .filter_map(|e| {
            if let Recorded::Phase(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(phases, ["review", "confirm", "summarize"]);
}

#[test]
fn pr_review_deep_adds_a_reviewer_and_honours_the_base_branch() {
    let b = builtin("pr-review");
    let host = FakeHost::new();
    host.on_read(|op| match op {
        ReadOp::GitChangedFiles { base } => {
            assert_eq!(base.as_deref(), Some("develop"));
            Ok(json!(["x.rs"]))
        }
        other => panic!("{other:?}"),
    });
    host.on_ask(|req| {
        if props(req).contains(&"findings".to_owned()) {
            AskReply::ok(json!({"findings": []}))
        } else {
            AskReply::ok(json!({"conclusion": "Clean."}))
        }
    });
    let out = run_builtin(&b, json!({"deep": true, "base": "develop"}), &host);
    assert_eq!(out["findings"], json!([]));
    let reviewers = host
        .recorded()
        .into_iter()
        .filter(|e| matches!(e, Recorded::Actor(_, s) if s.name.ends_with("reviewer")))
        .count();
    assert_eq!(reviewers, 4);
}

#[test]
fn pr_review_with_nothing_to_review_asks_no_one() {
    let b = builtin("pr-review");
    let host = FakeHost::new();
    host.on_read(|_| Ok(json!([])));
    let out = run_builtin(&b, json!({}), &host);
    assert!(
        out["conclusion"]
            .as_str()
            .unwrap()
            .starts_with("Nothing to review")
    );
    assert!(host.asks().is_empty());
}

#[test]
fn pr_review_survives_failing_agents() {
    let b = builtin("pr-review");
    let host = FakeHost::new();
    host.on_read(|_| Ok(json!(["a.rs"])));
    host.on_ask(|req| {
        let p = props(req);
        if p.contains(&"findings".to_owned()) {
            AskReply::ok(json!({"findings": [finding(1, "high")]}))
        } else {
            AskReply::failed("no result")
        }
    });
    let out = run_builtin(&b, json!({}), &host);
    assert!(
        out["conclusion"]
            .as_str()
            .unwrap()
            .starts_with("No conclusion")
    );
    assert!(
        out["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["status"] == "unconfirmed")
    );
}

#[test]
fn fix_until_green_loops_on_a_real_gate_and_stops_at_the_first_pass() {
    let b = builtin("fix-until-green");
    let host = FakeHost::new();
    let gates = Arc::new(AtomicUsize::new(0));
    let g = gates.clone();
    host.on_run(move |req| {
        assert_eq!(req.program, "cargo");
        assert_eq!(req.args, ["test", "--workspace"]);
        let n = g.fetch_add(1, Ordering::SeqCst);
        RunReply {
            exit_code: Some(if n < 2 { 101 } else { 0 }),
            stderr: "FAILED".into(),
            ..RunReply::default()
        }
    });
    host.on_ask(|req| {
        if props(req).contains(&"changed".to_owned()) {
            AskReply::ok(json!({"changed": ["src/lib.rs"], "notes": "fixed"}))
        } else {
            AskReply::ok(json!({"clean": true, "concerns": []}))
        }
    });
    let out = run_builtin(&b, json!({}), &host);
    assert_eq!(out["green"], true);
    assert_eq!(out["clean"], true);
    assert_eq!(gates.load(Ordering::SeqCst), 3, "red, red, green");
    let asks = host.asks();
    assert_eq!(asks.len(), 3, "two fix rounds and one audit");
    // One persistent fixer for every round; the auditor is a different, read-only actor.
    assert_eq!(asks[0].actor, asks[1].actor);
    assert_ne!(asks[1].actor, asks[2].actor);
    assert!(!asks[0].read_only && asks[2].read_only);
    assert!(
        out["conclusion"]
            .as_str()
            .unwrap()
            .contains("passes after 2 fix round")
    );
}

#[test]
fn fix_until_green_is_bounded_and_honest_when_it_never_passes() {
    let b = builtin("fix-until-green");
    let host = FakeHost::new();
    host.on_run(|_| RunReply {
        exit_code: Some(1),
        ..RunReply::default()
    });
    host.on_ask(|req| {
        if props(req).contains(&"changed".to_owned()) {
            AskReply::failed("could not produce a valid result")
        } else {
            AskReply::failed("no verdict")
        }
    });
    let out = run_builtin(
        &b,
        json!({"rounds": 2, "check": "clippy", "extra": ["-p", "x"]}),
        &host,
    );
    assert_eq!(out["green"], false);
    assert_eq!(out["clean"], Value::Null);
    let runs: Vec<_> = host
        .recorded()
        .into_iter()
        .filter_map(|e| {
            if let Recorded::Run(r) = e {
                Some(r)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(runs.len(), 3, "the first gate plus one per round");
    assert_eq!(runs[0].args, ["clippy", "-p", "x"]);
    assert!(out["verified"][0].as_str().unwrap().contains("failed"));
}

#[test]
fn fix_until_green_does_not_fix_what_is_already_green() {
    let b = builtin("fix-until-green");
    let host = FakeHost::new();
    host.on_ask(|_| AskReply::ok(json!({"clean": true, "concerns": []})));
    let out = run_builtin(&b, json!({}), &host);
    assert_eq!(out["green"], true);
    assert_eq!(host.asks().len(), 1, "only the audit");
}

#[test]
fn repo_audit_splits_files_among_auditors_and_confirms_each_finding() {
    let b = builtin("repo-audit");
    let host = FakeHost::new();
    host.on_read(|op| match op {
        ReadOp::Glob { pattern } => {
            assert_eq!(pattern, "crates/**");
            Ok(json!(
                (0..10).map(|i| format!("f{i}.rs")).collect::<Vec<_>>()
            ))
        }
        other => panic!("{other:?}"),
    });
    host.on_ask(|req| {
        let p = props(req);
        if p.contains(&"findings".to_owned()) {
            let first = req.instructions.lines().nth(1).unwrap_or("f?").to_owned();
            AskReply::ok(
                json!({"findings": [{"where": first, "what": "risky", "evidence": "code"}]}),
            )
        } else if p.contains(&"confirmed".to_owned()) {
            AskReply::ok(json!({"confirmed": true, "evidence": "yes"}))
        } else {
            AskReply::ok(json!({"conclusion": "Four risky spots."}))
        }
    });
    let out = run_builtin(&b, json!({"glob": "crates/**", "auditors": 4}), &host);
    assert_eq!(out["conclusion"], "Four risky spots.");
    assert_eq!(out["findings"].as_array().unwrap().len(), 4);
    let asks = host.asks();
    assert_eq!(asks.len(), 4 + 4 + 1);
    assert!(asks.iter().all(|a| a.read_only), "an audit never writes");
    // 10 files over 4 auditors: chunks of 3, 3, 3, 1 - every file read once.
    let covered: usize = asks[..4]
        .iter()
        .map(|a| {
            a.instructions
                .lines()
                .filter(|l| l.starts_with('f') && l.ends_with(".rs"))
                .count()
        })
        .sum();
    assert_eq!(covered, 10);
    assert!(
        host.recorded()
            .iter()
            .any(|e| matches!(e, Recorded::Artifact(_)))
    );
}

#[test]
fn repo_audit_with_no_matching_files_asks_no_one() {
    let b = builtin("repo-audit");
    let host = FakeHost::new();
    host.on_read(|_| Ok(json!([])));
    let out = run_builtin(&b, json!({}), &host);
    assert!(
        out["conclusion"]
            .as_str()
            .unwrap()
            .starts_with("No files match")
    );
    assert!(host.asks().is_empty());
}

#[test]
fn analysis_shows_what_the_user_will_approve() {
    let a = analyze("fix.star", builtin("fix-until-green").source).unwrap();
    assert_eq!(a.graph.phase_names(), ["fix", "review"]);
    assert_eq!(a.graph.commands.len(), 1);
    assert_eq!(a.graph.commands[0].command, "cargo");
    let a = analyze("audit.star", builtin("repo-audit").source).unwrap();
    assert_eq!(a.graph.phase_names(), ["audit", "confirm", "report"]);
    assert!(a.graph.commands.is_empty(), "the audit runs no commands");
    let a = analyze("pr.star", builtin("pr-review").source).unwrap();
    assert_eq!(a.graph.phase_names(), ["review", "confirm", "summarize"]);
    assert!(a.warnings.is_empty(), "{:?}", a.warnings);
}
