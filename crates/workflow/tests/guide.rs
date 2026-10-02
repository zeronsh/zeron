//! The guide's worked example must actually run: extract it from
//! `docs/workflow-guide.md` and execute it against a scripted host.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use serde_json::{Value, json};
use zeron_workflow::analyze;
use zeron_workflow::host::{AskReply, RunReply};
use zeron_workflow::testing::{FakeHost, Recorded};

const GUIDE: &str = include_str!("../../../docs/workflow-guide.md");

fn example() -> String {
    let start = GUIDE
        .find("```python example\n")
        .expect("the guide has a worked example")
        + "```python example\n".len();
    let end = GUIDE[start..].find("\n```").expect("the example is fenced") + start;
    GUIDE[start..end].to_owned()
}

fn finding(angle: &str, n: u32) -> Value {
    json!({
        "where": format!("src/{angle}.rs:{n}"),
        "what": format!("a {angle} problem #{n}"),
        "evidence": "read the function",
        "severity": if n == 1 { "high" } else { "low" },
    })
}

#[test]
fn the_guide_example_passes_analysis_with_the_graph_the_guide_describes() {
    let analysis = analyze("example.star", &example()).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(
        analysis.graph.phase_names(),
        ["review", "confirm", "fix", "summarize"]
    );
    assert_eq!(analysis.graph.commands.len(), 1);
    assert_eq!(analysis.graph.commands[0].command, "cargo");
    assert!(analysis.graph.phases[0].asks[0].site.fan_out);
    assert!(
        analysis.graph.phases[1].asks[0].site.fan_out,
        "confirm runs once per finding"
    );
    assert!(analysis.warnings.is_empty(), "{:?}", analysis.warnings);
}

#[test]
fn the_guide_example_runs_end_to_end_against_a_fake_host() {
    let host = FakeHost::new();
    host.on_ask(|req| {
        let props = req
            .schema
            .as_ref()
            .and_then(|s| s["properties"].as_object().cloned())
            .unwrap_or_default();
        if props.contains_key("findings") {
            let angle = ["security", "correctness", "performance"]
                .into_iter()
                .find(|a| req.instructions.contains(a))
                .unwrap();
            AskReply::ok(json!({"findings": [finding(angle, 1), finding(angle, 2)]}))
        } else if props.contains_key("confirmed") {
            // Performance findings cannot be reproduced.
            let confirmed = !req.instructions.contains("performance");
            AskReply::ok(json!({"confirmed": confirmed, "evidence": "ran it"}))
        } else if props.contains_key("changed") {
            AskReply::ok(json!({"changed": ["src/lib.rs"], "notes": "fixed"}))
        } else {
            AskReply::ok(
                json!({"conclusion": "Four problems are real; two could not be reproduced."}),
            )
        }
    });
    let gates = Arc::new(AtomicUsize::new(0));
    let g = gates.clone();
    host.on_run(move |req| {
        assert_eq!(req.program, "cargo");
        assert_eq!(req.args, ["test", "--workspace"]);
        assert_eq!(req.timeout_s, 900);
        // Red on the first round, green after one fix.
        let n = g.fetch_add(1, Ordering::SeqCst);
        RunReply {
            exit_code: Some(if n == 0 { 101 } else { 0 }),
            stderr: "test result: FAILED. 1 failed".into(),
            ..RunReply::default()
        }
    });
    let out = run(&example(), json!({"strong_model": "opus"}), &host).unwrap();

    assert_eq!(
        out["conclusion"],
        "Four problems are real; two could not be reproduced."
    );
    let findings = out["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 6);
    let statuses: Vec<&str> = findings
        .iter()
        .map(|f| f["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses.iter().filter(|s| **s == "verified").count(), 4);
    assert_eq!(statuses.iter().filter(|s| **s == "unconfirmed").count(), 2);
    for f in findings {
        for key in ["where", "what", "evidence", "status", "severity"] {
            assert!(f.get(key).is_some(), "{key}");
        }
    }
    assert_eq!(out["verified"], json!(["cargo test --workspace passed"]));
    assert_eq!(out["not_covered"].as_array().unwrap().len(), 1);

    // Two gate runs (red, fix, green), exactly one fixer ask, one judge ask.
    assert_eq!(gates.load(Ordering::SeqCst), 2);
    let asks = host.asks();
    assert_eq!(
        asks.len(),
        3 + 6 + 1 + 1,
        "3 reviewers, 6 checkers, 1 fixer, 1 judge"
    );
    // The confirmers are fresh actors, one per finding; reviewers are read-only.
    let actors: std::collections::HashSet<_> = asks.iter().map(|a| a.actor.to_string()).collect();
    assert_eq!(actors.len(), 3 + 6 + 1 + 1);
    assert!(asks.iter().take(3).all(|a| a.read_only));
    // The strong model reaches the judge actor only.
    let judge = host
        .recorded()
        .into_iter()
        .find_map(|e| match e {
            Recorded::Actor(_, s) if s.name == "judge" => Some(s),
            _ => None,
        })
        .unwrap();
    assert_eq!(judge.model.as_deref(), Some("opus"));
    // Reports and the table artifact were produced as the guide says.
    let reports: Vec<_> = host
        .recorded()
        .into_iter()
        .filter_map(|e| match e {
            Recorded::Report(v, _) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(
        reports,
        [
            json!({"reviewed": 6}),
            json!({"confirmed": 4, "unconfirmed": 2})
        ]
    );
    let phases: Vec<_> = host
        .recorded()
        .into_iter()
        .filter_map(|e| match e {
            Recorded::Phase(p) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(phases, ["review", "confirm", "fix", "summarize"]);
}

#[test]
fn the_example_survives_failing_agents_and_a_gate_that_never_passes() {
    let host = FakeHost::new();
    host.on_ask(|req| {
        let props = req
            .schema
            .as_ref()
            .and_then(|s| s["properties"].as_object().cloned())
            .unwrap_or_default();
        if props.contains_key("findings") {
            AskReply::ok(json!({"findings": [finding("security", 1)]}))
        } else if props.contains_key("confirmed") {
            AskReply::ok(json!({"confirmed": true, "evidence": "yes"}))
        } else if props.contains_key("changed") {
            AskReply::failed("the fixer could not produce a valid result")
        } else {
            AskReply::failed("no conclusion")
        }
    });
    host.on_run(|_| RunReply {
        exit_code: Some(1),
        ..RunReply::default()
    });
    let out = run(&example(), json!({}), &host).unwrap();
    assert!(
        out["conclusion"]
            .as_str()
            .unwrap()
            .starts_with("No conclusion was produced")
    );
    assert_eq!(
        out["verified"],
        json!(["cargo test --workspace still failing"])
    );
    // Three bounded gate rounds, two fixer attempts between them.
    let gates = host
        .recorded()
        .into_iter()
        .filter(|e| matches!(e, Recorded::Run(_)))
        .count();
    assert_eq!(gates, 3);
}

// ── saved workflows (the guide's frontmatter example) ─────────────────────

fn saved_example() -> String {
    let start = GUIDE
        .find("```python saved-example\n")
        .expect("the guide has a saved-workflow example")
        + "```python saved-example\n".len();
    let end = GUIDE[start..].find("\n```").expect("the example is fenced") + start;
    format!("{}\n", &GUIDE[start..end])
}

#[test]
fn the_saved_workflow_example_parses_validates_and_runs() {
    use zeron_proto::saved_workflow::{SavedArgType, validate_args};
    use zeron_workflow::host::ReadOp;
    use zeron_workflow::saved::parse;

    let text = saved_example();
    let file = parse("doc-drift.star", Some("doc-drift"), &text)
        .unwrap_or_else(|d| panic!("{}", zeron_workflow::diagnostic::render(&d)));
    let args = &file.meta.args;
    assert_eq!(
        args.iter()
            .map(|a| (a.name.as_str(), a.ty))
            .collect::<Vec<_>>(),
        [
            ("folders", SavedArgType::Json),
            ("max_files", SavedArgType::Int),
            ("strict", SavedArgType::Bool),
            ("branch", SavedArgType::String),
        ]
    );
    assert!(file.meta.when_to_use.is_some());
    let analysis = analyze("doc-drift.star", &text).unwrap_or_else(|d| panic!("{d:?}"));
    assert_eq!(analysis.graph.phase_names(), ["check"]);

    // Defaults alone run it; `branch` is absent (hence `args.get`).
    let given = validate_args(args, &json!({})).unwrap();
    assert_eq!(
        given,
        json!({"folders": ["docs"], "max_files": 40, "strict": false})
    );
    let host = FakeHost::new();
    host.on_read(|op| match op {
        ReadOp::Glob { pattern } if pattern == "docs/**/*.md" => {
            Ok(json!(["docs/a.md", "docs/b.md"]))
        }
        other => panic!("unexpected read {other:?}"),
    });
    host.on_ask(|req| {
        assert!(req.read_only);
        assert!(!req.instructions.contains("since"), "{}", req.instructions);
        AskReply::ok(json!({"findings": [{"file": "docs/a.md", "claim": "c", "reality": "r"}]}))
    });
    let out = run(&text, given, &host).unwrap();
    assert_eq!(out["conclusion"], "1 stale claim(s) found");

    // With arguments: two folders, strict, a branch.
    let given = validate_args(
        args,
        &json!({"folders": ["docs", "guides"], "strict": true, "branch": "main", "max_files": 1}),
    )
    .unwrap();
    let host = FakeHost::new();
    host.on_read(|op| match op {
        ReadOp::Glob { pattern } => Ok(json!([format!("{pattern}-1"), format!("{pattern}-2")])),
        other => panic!("unexpected read {other:?}"),
    });
    host.on_ask(|req| {
        assert!(
            req.instructions.contains("WRONG or unclear"),
            "{}",
            req.instructions
        );
        assert!(req.instructions.contains("since `main`"));
        assert_eq!(
            req.instructions.lines().count(),
            2,
            "max_files = 1 leaves one path"
        );
        AskReply::ok(json!({"findings": []}))
    });
    let out = run(&text, given, &host).unwrap();
    assert_eq!(out["findings"], json!([]));
    assert_eq!(host.asks().len(), 2, "one checker per folder");

    // Bad arguments never reach the script.
    let err = validate_args(args, &json!({"max_files": "many", "stict": true})).unwrap_err();
    assert_eq!(err.len(), 2, "{err:?}");
}
