mod common;

use common::*;
use serde_json::json;
use zeron_workflow::analyze;
use zeron_workflow::testing::FakeHost;

const REVIEW: &str = r#"
def confirm(f):
    a = agent("verifier")
    return a.ask("Re-check " + f, schema = schema.obj({"confirmed": schema.bool()})).result()

def main(args):
    phase("review")
    reviewers = [agent(name) for name in ["security", "perf"]]
    hs = [r.ask("Review for problems", read_only = True) for r in reviewers]
    findings = results(hs)
    phase("confirm")
    checks = pmap(confirm, findings)
    phase("gate")
    for i in range(3):
        g = run("cargo", ["test", "--workspace"])
        if g.ok:
            break
    return findings
"#;

#[test]
fn the_graph_lists_phases_actors_asks_and_commands() {
    let a = analyze("wf.star", REVIEW).unwrap();
    let g = &a.graph;
    assert_eq!(g.phase_names(), ["review", "confirm", "gate"]);
    assert_eq!(g.phases[0].asks.len(), 1);
    assert!(
        g.phases[0].asks[0].site.fan_out,
        "an ask in a comprehension fans out"
    );
    assert_eq!(g.phases[0].asks[0].actor.as_deref(), Some("r"));
    assert!(g.phases[0].asks[0].read_only);
    assert_eq!(
        g.phases[0].asks[0].instructions_head.as_deref(),
        Some("Review for problems")
    );
    // The confirm phase's ask lives in `confirm`, reached through pmap.
    assert_eq!(g.phases[1].asks.len(), 1);
    assert!(g.phases[1].asks[0].typed);
    assert!(
        g.phases[1].asks[0].site.fan_out,
        "a def passed to pmap runs per item"
    );
    assert_eq!(g.phases[2].commands.len(), 1);
    assert_eq!(g.phases[2].commands[0].command, "cargo");
    assert_eq!(
        g.phases[2].commands[0].args.as_deref(),
        Some(&["test".to_string(), "--workspace".to_string()][..])
    );
    assert!(
        g.phases[2].commands[0].site.fan_out,
        "a command in a for loop"
    );
    assert_eq!(g.commands.len(), 1);
    assert_eq!(g.actors.len(), 2);
    assert_eq!(g.actors[1].name, None, "a computed name is not a literal");
    assert_eq!(g.unphased_asks, 0);
    assert!(a.warnings.is_empty(), "{:?}", a.warnings);
}

#[test]
fn graph_site_ids_are_the_ids_the_runtime_journals() {
    let a = analyze("wf.star", REVIEW).unwrap();
    let host = FakeHost::new();
    host.on_ask(|req| {
        if req.instructions.starts_with("Re-check") {
            zeron_workflow::host::AskReply::ok(json!({"confirmed": true}))
        } else {
            zeron_workflow::host::AskReply::ok(json!("finding"))
        }
    });
    run(REVIEW, json!({}), &host).unwrap();
    let runtime_sites: Vec<String> = host
        .asks()
        .iter()
        .map(|a| a.key.site.rsplit('/').next().unwrap().to_owned())
        .collect();
    for ask in a.graph.phases.iter().flat_map(|p| &p.asks) {
        assert!(
            runtime_sites.contains(&ask.site.site_id),
            "{} not in {runtime_sites:?}",
            ask.site.site_id
        );
    }
    let run_site = &a.graph.commands[0].site.site_id;
    let ran = host
        .recorded()
        .into_iter()
        .any(|e| matches!(e, zeron_workflow::testing::Recorded::Run(r) if r.key.site == *run_site));
    assert!(ran);
}

#[test]
fn chained_calls_have_distinct_site_ids() {
    // A chained call has distinct sites for `.ask(...)` and `.result()`.
    let a = analyze(
        "wf.star",
        "def main(args):\n    phase(\"p\")\n    x = agent(\"a\").ask(\"q\").result()\n    return x.ok\n",
    )
    .unwrap();
    assert_eq!(a.graph.phases[0].asks[0].site.site_id, "3:9-3:28");
    assert_eq!(a.graph.actors[0].site.site_id, "3:9-3:19");
}

#[test]
fn syntax_errors_are_positioned_diagnostics() {
    let d = diagnostics("def main(args):\n    x = (1 +\n");
    assert_eq!(d.len(), 1);
    assert!(d[0].starts_with("wf.star:"), "{d:?}");
    let d = diagnostics("def main(args):\n    try:\n        pass\n");
    assert!(d[0].starts_with("wf.star:2:"), "{d:?}");
    let d = diagnostics("import os\n");
    assert!(d[0].starts_with("wf.star:1:"), "{d:?}");
}

#[test]
fn main_is_required_and_takes_one_parameter() {
    assert!(diagnostics("def other(args):\n    return 1\n")[0].contains("def main(args)"));
    assert!(diagnostics("def main():\n    return 1\n")[0].contains("exactly one parameter"));
    assert!(diagnostics("def main(args):\n    return 1\n").is_empty());
}

#[test]
fn every_phase_must_do_work_and_claims_the_rest_of_its_block() {
    let d = diagnostics("def main(args):\n    phase(\"empty\")\n    x = 1\n    return x\n");
    assert_eq!(d.len(), 1);
    assert!(
        d[0].starts_with("wf.star:2:5") && d[0].contains("\"empty\""),
        "{d:?}"
    );
    // An ask after the phase, in a nested block, counts.
    assert!(diagnostics("def main(args):\n    phase(\"p\")\n    for i in range(2):\n        agent(\"a\").ask(\"q\")\n    return 1\n").is_empty());
    // An ask BEFORE the phase does not.
    let d = diagnostics(
        "def main(args):\n    agent(\"a\").ask(\"q\")\n    phase(\"p\")\n    return 1\n",
    );
    assert!(d[0].contains("\"p\""), "{d:?}");
    // …and a call to a helper def that asks counts.
    assert!(diagnostics("def h():\n    return agent(\"a\").ask(\"q\").result()\ndef main(args):\n    phase(\"p\")\n    return h()\n").is_empty());
    // A second phase ends the first's claim.
    let d = diagnostics(
        "def main(args):\n    phase(\"a\")\n    phase(\"b\")\n    agent(\"x\").ask(\"q\")\n    return 1\n",
    );
    assert!(d[0].contains("\"a\""), "{d:?}");
}

#[test]
fn phases_need_literal_names_and_statement_position() {
    assert!(
        diagnostics("def main(args):\n    n = \"x\"\n    phase(n)\n    agent(\"a\").ask(\"q\")\n")
            [0]
        .contains("string literal")
    );
    assert!(
        diagnostics("def main(args):\n    x = phase(\"p\")\n    agent(\"a\").ask(\"q\")\n")[0]
            .contains("statement of its own")
    );
}

#[test]
fn run_needs_a_literal_program_and_interpreters_need_literal_args() {
    let d = diagnostics("def main(args):\n    c = \"cargo\"\n    run(c, [\"test\"])\n");
    assert!(
        d[0].starts_with("wf.star:3:5") && d[0].contains("string literal"),
        "{d:?}"
    );
    // A shell with computed arguments is arbitrary code: refused statically.
    let d = diagnostics("def main(args):\n    run(\"bash\", [\"-c\", args[\"cmd\"]])\n");
    assert!(d[0].contains("arbitrary code"), "{d:?}");
    assert!(
        diagnostics("def main(args):\n    run(\"/bin/sh\", args[\"x\"])\n")[0]
            .contains("arbitrary code")
    );
    // The same shell with a fully literal command is visible to the approver.
    assert!(diagnostics("def main(args):\n    run(\"bash\", [\"-c\", \"echo hi\"])\n").is_empty());
    // A non-interpreter may take computed arguments.
    assert!(diagnostics("def main(args):\n    run(\"cargo\", args[\"extra\"])\n").is_empty());
}

#[test]
fn recursion_is_rejected_before_it_runs() {
    let d = diagnostics(
        "def a(n):\n    return b(n)\ndef b(n):\n    return a(n)\ndef main(args):\n    return a(1)\n",
    );
    assert!(
        d.iter()
            .any(|m| m.contains("recursion") && m.contains("a -> b -> a")),
        "{d:?}"
    );
    let d = diagnostics("def f(n):\n    return f(n - 1)\ndef main(args):\n    return f(3)\n");
    assert!(d[0].contains("recursion"), "{d:?}");
}

#[test]
fn host_calls_are_refused_at_module_level() {
    let d = diagnostics("A = agent(\"x\")\ndef main(args):\n    return 1\n");
    assert!(
        d[0].starts_with("wf.star:1:5") && d[0].contains("module level"),
        "{d:?}"
    );
    let d = diagnostics("phase(\"x\")\ndef main(args):\n    return 1\n");
    assert!(d[0].contains("top-level"), "{d:?}");
    // Constants are fine.
    assert!(
        diagnostics("ANGLES = [\"a\", \"b\"]\ndef main(args):\n    return ANGLES\n").is_empty()
    );
}

#[test]
fn pmap_needs_a_top_level_def() {
    let d = diagnostics("def main(args):\n    f = len\n    return pmap(f, [1])\n");
    assert!(d[0].contains("top-level def"), "{d:?}");
    let d = diagnostics("def main(args):\n    return parallel(args[\"x\"])\n");
    assert!(d[0].contains("list literal"), "{d:?}");
}

#[test]
fn oversized_scripts_are_refused() {
    let big = format!(
        "def main(args):\n    return 1\n# {}\n",
        "x".repeat(300 * 1024)
    );
    let d = diagnostics(&big);
    assert!(d[0].contains("262144"), "{d:?}");
}

#[test]
fn asks_outside_phases_are_a_warning_not_an_error() {
    let a = analyze("wf.star", "def main(args):\n    agent(\"a\").ask(\"q\")\n    phase(\"p\")\n    agent(\"b\").ask(\"r\")\n").unwrap();
    assert_eq!(a.graph.unphased_asks, 1);
    assert_eq!(a.warnings.len(), 1);
}

#[test]
fn a_dynamic_phase_loop_still_has_a_graph() {
    // Same-named phases merge.
    let a = analyze(
        "wf.star",
        "def main(args):\n    for i in range(2):\n        phase(\"round\")\n        agent(\"a\").ask(\"q\")\n    return 1\n",
    )
    .unwrap();
    assert_eq!(a.graph.phases.len(), 1);
    assert!(a.graph.phases[0].asks[0].site.fan_out);
}
