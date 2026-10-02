mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::*;
use serde_json::{Value, json};
use zeron_workflow::host::{AskReply, ReadOp, RunReply};
use zeron_workflow::testing::{FakeHost, Recorded};
use zeron_workflow::{Limits, RunError};

fn err_text(r: Result<Value, RunError>) -> String {
    r.unwrap_err().to_string()
}

#[test]
fn a_minimal_workflow_asks_and_returns_a_value() {
    let host = FakeHost::new();
    let out = run(
        r#"
def main(args):
    phase("go")
    a = agent("reviewer")
    r = a.ask("look at " + args["what"]).result()
    return {"ok": r.ok, "value": r.value, "cached": r.cached}
"#,
        json!({"what": "x"}),
        &host,
    )
    .unwrap();
    assert_eq!(
        out,
        json!({"ok": true, "value": {"echo": "look at x"}, "cached": false})
    );
    let rec = host.recorded();
    assert!(matches!(&rec[0], Recorded::Phase(p) if p == "go"));
    assert!(matches!(&rec[1], Recorded::Actor(_, spec) if spec.name == "reviewer"));
    assert!(matches!(&rec[2], Recorded::Ask(a) if a.instructions == "look at x"));
}

#[test]
fn asks_dispatch_before_any_join_so_a_fan_out_runs_concurrently() {
    let host = FakeHost::new();
    host.delay_asks(Duration::from_millis(200));
    let started = Instant::now();
    let out = run(
        r#"
def main(args):
    phase("fan")
    hs = [agent("a" + str(i)).ask("q" + str(i)) for i in range(5)]
    return [h.result().value["echo"] for h in hs]
"#,
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(out, json!(["q0", "q1", "q2", "q3", "q4"]));
    assert!(
        started.elapsed() < Duration::from_millis(700),
        "5 asks of 200ms must overlap, took {:?}",
        started.elapsed()
    );
    assert_eq!(host.max_in_flight(), 5);
}

#[test]
fn failures_are_values_the_script_can_branch_on() {
    let host = FakeHost::new();
    host.on_ask(|req| {
        if req.instructions == "boom" {
            AskReply::failed("the child could not produce a valid result")
        } else {
            AskReply::ok(json!(1))
        }
    });
    let out = run(
        r#"
def main(args):
    phase("p")
    a = agent("w")
    good = a.ask("fine").result()
    bad = a.ask("boom").result()
    return [good.ok, bad.ok, bad.error, bad.value]
"#,
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(
        out,
        json!([
            true,
            false,
            "the child could not produce a valid result",
            null
        ])
    );
}

#[test]
fn results_and_wait_all_helpers() {
    let host = FakeHost::new();
    host.on_ask(|req| {
        if req.instructions.contains("bad") {
            AskReply::failed("x")
        } else {
            AskReply::ok(json!(req.instructions.len()))
        }
    });
    let out = run(
        r#"
def main(args):
    phase("p")
    a = agent("w")
    hs = [a.ask("ok1"), a.ask("bad"), a.ask("ok333")]
    all = wait_all(hs)
    return [[r.ok for r in all], results(hs)]
"#,
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(out, json!([[true, false, true], [3, 5]]));
}

#[test]
fn schema_and_options_reach_the_host() {
    let host = FakeHost::new();
    run(
        r#"
def main(args):
    phase("p")
    a = agent("w", persona = "Be terse.", harness = "codex", model = "gpt", reasoning = "high")
    a.ask("x", schema = schema.obj({"v": schema.str("a verdict")}), read_only = True, timeout_s = 90).result()
    return None
"#,
        json!({}),
        &host,
    )
    .unwrap();
    let rec = host.recorded();
    let Recorded::Actor(_, spec) = &rec[1] else {
        panic!("{rec:?}")
    };
    assert_eq!(spec.persona.as_deref(), Some("Be terse."));
    assert_eq!(spec.harness.as_deref(), Some("codex"));
    assert_eq!(spec.model.as_deref(), Some("gpt"));
    assert_eq!(spec.reasoning.as_deref(), Some("high"));
    let ask = &host.asks()[0];
    assert!(ask.read_only);
    assert_eq!(ask.timeout_s, Some(90));
    assert_eq!(
        ask.schema.as_ref().unwrap(),
        &json!({"type": "object", "properties": {"v": {"type": "string", "description": "a verdict"}}, "required": ["v"]})
    );
}

#[test]
fn schema_helpers_build_json_schema() {
    let host = FakeHost::new();
    let out = run(
        r#"
def main(args):
    return {
        "s": schema.str("d", min_len = 1, max_len = 9),
        "i": schema.int(min = 0, max = 10),
        "n": schema.num(min = 0.5),
        "b": schema.bool(),
        "e": schema.enum(["pass", "fail"]),
        "l": schema.list(schema.int(), min_items = 1),
        "o": schema.obj({"a": schema.int(), "b": schema.opt(schema.str())}),
        "o2": schema.obj({"a": schema.int(), "b": schema.str()}, required = ["a"]),
        "any": schema.any("whatever"),
    }
"#,
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(
        out["s"],
        json!({"type": "string", "minLength": 1, "maxLength": 9, "description": "d"})
    );
    assert_eq!(
        out["i"],
        json!({"type": "integer", "minimum": 0, "maximum": 10})
    );
    assert_eq!(out["n"], json!({"type": "number", "minimum": 0.5}));
    assert_eq!(
        out["e"],
        json!({"type": "string", "enum": ["pass", "fail"]})
    );
    assert_eq!(
        out["l"],
        json!({"type": "array", "items": {"type": "integer"}, "minItems": 1})
    );
    assert_eq!(out["o"]["required"], json!(["a"]));
    assert!(out["o"]["properties"]["b"].get("x-optional").is_none());
    assert_eq!(out["o2"]["required"], json!(["a"]));
    assert_eq!(out["any"], json!({"description": "whatever"}));
}

#[test]
fn run_returns_the_exit_code_as_a_value() {
    let host = FakeHost::new();
    host.on_run(|req| RunReply {
        exit_code: Some(if req.args == ["--bad"] { 2 } else { 0 }),
        stdout: "out".into(),
        stderr: "err".into(),
        ..RunReply::default()
    });
    let out = run(
        r#"
def main(args):
    phase("gate")
    good = run("cargo", ["test"], timeout_s = 30, cwd = "crates/x")
    bad = run("cargo", ["--bad"])
    return [good.ok, good.exit_code, good.stdout, bad.ok, bad.exit_code, bad.stderr, bad.timed_out]
"#,
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(out, json!([true, 0, "out", false, 2, "err", false]));
    let Some(Recorded::Run(req)) = host
        .recorded()
        .into_iter()
        .find(|e| matches!(e, Recorded::Run(_)))
    else {
        panic!()
    };
    assert_eq!(req.program, "cargo");
    assert_eq!(req.timeout_s, 30);
    assert_eq!(req.cwd.as_deref(), Some("crates/x"));
}

#[test]
fn run_refuses_aliasing_absolute_cwd_and_bad_timeouts() {
    let host = FakeHost::new();
    // An alias is invisible to the analysis, so the runtime refuses it.
    let t = err_text(run(
        "def main(args):\n    r = run\n    return r(\"sh\", [\"-c\", \"id\"]).ok\n",
        json!({}),
        &host,
    ));
    assert!(t.contains("string literal"), "{t}");
    let t = err_text(run(
        "def main(args):\n    return run(\"ls\", cwd = \"/etc\").ok\n",
        json!({}),
        &host,
    ));
    assert!(t.contains("inside the project"), "{t}");
    let t = err_text(run(
        "def main(args):\n    return run(\"ls\", timeout_s = 0).ok\n",
        json!({}),
        &host,
    ));
    assert!(t.contains("timeout_s"), "{t}");
    assert!(
        host.recorded()
            .iter()
            .all(|e| !matches!(e, Recorded::Run(_)))
    );
}

#[test]
fn ordinals_count_executions_of_a_call_site() {
    let host = FakeHost::new();
    run(
        r#"
def main(args):
    phase("loop")
    a = agent("w")
    hs = []
    for i in range(3):
        hs.append(a.ask("round " + str(i)))
    wait_all(hs)
    return None
"#,
        json!({}),
        &host,
    )
    .unwrap();
    let asks = host.asks();
    assert_eq!(asks.len(), 3);
    assert_eq!(asks[0].key.site, asks[1].key.site);
    assert_eq!(
        asks.iter().map(|a| a.key.ordinal).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    // All three share the actor created once.
    assert!(asks.iter().all(|a| a.actor == asks[0].actor));
}

#[test]
fn a_rerun_of_the_same_script_produces_the_same_keys() {
    let script = r#"
def review(i):
    return agent("r" + str(i)).ask("check " + str(i)).result().value["echo"]

def main(args):
    phase("a")
    x = pmap(review, [1, 2, 3, 4])
    phase("b")
    h = agent("z").ask("final")
    return [x, h.result().ok]
"#;
    let keys = || {
        let host = FakeHost::new();
        run(script, json!({}), &host).unwrap();
        let mut keys: Vec<String> = host.asks().iter().map(|a| a.key.to_string()).collect();
        keys.sort();
        keys
    };
    assert_eq!(keys(), keys());
}

#[test]
fn pmap_runs_a_top_level_def_per_item_concurrently_in_order() {
    let host = FakeHost::new();
    host.delay_asks(Duration::from_millis(150));
    let started = Instant::now();
    let out = run(
        r#"
def check(item):
    r = agent("c" + item).ask("check " + item).result()
    return r.value["echo"]

def main(args):
    phase("confirm")
    return pmap(check, args["items"])
"#,
        json!({"items": ["a", "b", "c", "d"]}),
        &host,
    )
    .unwrap();
    assert_eq!(out, json!(["check a", "check b", "check c", "check d"]));
    assert!(
        started.elapsed() < Duration::from_millis(550),
        "{:?}",
        started.elapsed()
    );
    assert!(host.max_in_flight() >= 2);
    // Keys are scoped by item index, not by thread timing.
    let mut sites: Vec<String> = host.asks().iter().map(|a| a.key.site.clone()).collect();
    sites.sort();
    assert!(
        sites[0].contains("[0]/") && sites[3].contains("[3]/"),
        "{sites:?}"
    );
}

#[test]
fn parallel_runs_zero_argument_defs() {
    let host = FakeHost::new();
    let out = run(
        r#"
def one():
    return 1
def two():
    return agent("t").ask("x").result().ok

def main(args):
    phase("p")
    return parallel([one, two])
"#,
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(out, json!([1, true]));
}

#[test]
fn pmap_rejects_lambdas_and_reports_item_errors() {
    let host = FakeHost::new();
    let d = diagnostics("def main(args):\n    return pmap(lambda x: x, [1])\n");
    assert!(d[0].contains("lambda"), "{d:?}");
    let t = err_text(run(
        r#"
def boom(x):
    return fail("bad item " + str(x))

def main(args):
    return pmap(boom, [1, 2])
"#,
        json!({}),
        &host,
    ));
    assert!(
        t.contains("item 0 failed") && t.contains("bad item 1"),
        "{t}"
    );
    // A worker may not return an actor or handle: only JSON crosses back.
    let t = err_text(run(
        "def mk(x):\n    return agent(\"a\")\ndef main(args):\n    return pmap(mk, [1])\n",
        json!({}),
        &host,
    ));
    assert!(t.contains("JSON-able"), "{t}");
    // phase() belongs to main.
    let t = err_text(run(
        "def inner(x):\n    phase(\"nope\")\n    return agent(\"a\").ask(\"q\").result().ok\ndef main(args):\n    return pmap(inner, [1])\n",
        json!({}),
        &host,
    ));
    assert!(t.contains("phase() cannot be called from a pmap"), "{t}");
}

#[test]
fn scripts_are_hermetic() {
    for name in ["time", "random", "open", "getenv", "eval", "import_"] {
        let d = diagnostics(&format!("def main(args):\n    return {name}\n"));
        assert!(
            !d.is_empty() && d[0].starts_with("wf.star:2:12"),
            "{name}: {d:?}"
        );
    }
    let d = diagnostics("def main(args):\n    while True:\n        pass\n");
    assert!(
        d[0].contains("wf.star:2:5") && d[0].contains("for _ in range"),
        "{d:?}"
    );
}

#[test]
fn infinite_loops_hit_the_step_limit() {
    let host = FakeHost::new();
    let limits = Limits {
        ticks: 100_000,
        ..Limits::default()
    };
    let r = run_with(
        "def main(args):\n    n = 0\n    for i in range(10000000):\n        n += 1\n    return n\n",
        json!({}),
        &host,
        Arc::default(),
        &limits,
    );
    assert!(matches!(r, Err(RunError::Limit(_))), "{r:?}");
}

#[test]
fn memory_is_capped() {
    let host = FakeHost::new();
    let limits = Limits {
        heap_bytes: 8 << 20,
        ..Limits::default()
    };
    let r = run_with(
        "def main(args):\n    x = []\n    for i in range(5000000):\n        x.append('x' * 1000 + str(i))\n    return len(x)\n",
        json!({}),
        &host,
        Arc::default(),
        &limits,
    );
    assert!(matches!(r, Err(RunError::Limit(_))), "{r:?}");
}

#[test]
fn compute_time_is_capped_but_waiting_on_agents_is_not() {
    let host = FakeHost::new();
    let limits = Limits {
        compute: Duration::from_millis(80),
        ticks: u64::MAX / 2,
        ..Limits::default()
    };
    let r = run_with(
        "def main(args):\n    n = 0\n    for i in range(1000000000):\n        n += 1\n    return n\n",
        json!({}),
        &host,
        Arc::default(),
        &limits,
    );
    assert!(
        matches!(&r, Err(RunError::Limit(m)) if m.contains("interpreter time")),
        "{r:?}"
    );

    // 400 ms of waiting on an ask is well over the 80 ms compute budget.
    host.delay_asks(Duration::from_millis(400));
    let r = run_with(
        "def main(args):\n    phase(\"p\")\n    return agent(\"a\").ask(\"x\").result().ok\n",
        json!({}),
        &host,
        Arc::default(),
        &limits,
    );
    assert_eq!(r.unwrap(), json!(true));
}

#[test]
fn cancel_stops_a_compute_loop() {
    let host = FakeHost::new();
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        c.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    let limits = Limits {
        ticks: u64::MAX / 2,
        compute: Duration::from_secs(600),
        ..Limits::default()
    };
    let r = run_with(
        "def main(args):\n    for i in range(2000000000):\n        pass\n",
        json!({}),
        &host,
        cancel,
        &limits,
    );
    assert_eq!(r, Err(RunError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn cancel_while_blocked_on_an_ask_returns_and_drops_it() {
    let host = FakeHost::new();
    host.delay_asks(Duration::from_secs(30));
    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        c.store(true, Ordering::SeqCst);
    });
    let started = Instant::now();
    let r = run_with(
        "def main(args):\n    phase(\"p\")\n    return agent(\"a\").ask(\"x\").result().ok\n",
        json!({}),
        &host,
        cancel,
        &Limits::default(),
    );
    assert_eq!(r, Err(RunError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn world_reads_reach_the_host_and_errors_name_the_call() {
    let host = FakeHost::new();
    host.on_read(|op| match op {
        ReadOp::Glob { pattern } => Ok(json!([format!("{pattern}/a.rs")])),
        ReadOp::Read { path } if path == "missing" => Ok(Value::Null),
        ReadOp::Read { .. } => Ok(json!("text")),
        ReadOp::GitDiff { .. } => Err("the diff is 900000 bytes; the limit is 262144".into()),
        _ => Ok(json!(["x"])),
    });
    let out = run(
        r#"
def main(args):
    return [
        files.glob("src/**"),
        files.read("a.txt"),
        files.read("missing"),
        files.grep("TODO", glob = "*.rs"),
        git.changed_files("main"),
        git.status(),
        git.log(5),
    ]
"#,
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(out[0], json!(["src/**/a.rs"]));
    assert_eq!(out[1], json!("text"));
    assert_eq!(out[2], Value::Null);
    let reads: Vec<_> = host
        .recorded()
        .into_iter()
        .filter_map(|e| match e {
            Recorded::Read(_, op) => Some(op),
            _ => None,
        })
        .collect();
    assert_eq!(reads.len(), 7);
    assert_eq!(
        reads[4],
        ReadOp::GitChangedFiles {
            base: Some("main".into())
        }
    );
    assert_eq!(
        reads[6],
        ReadOp::GitLog {
            limit: 5,
            path: None
        }
    );

    let t = err_text(run(
        "def main(args):\n    return git.diff()\n",
        json!({}),
        &host,
    ));
    assert!(t.contains("git.diff") && t.contains("900000"), "{t}");
    for bad in [
        "files.read(\"/etc/passwd\")",
        "files.read(\"../x\")",
        "files.glob(\"../**\")",
        "git.changed_files(\"--output=x\")",
        "git.log(0)",
    ] {
        let t = err_text(run(
            &format!("def main(args):\n    return {bad}\n"),
            json!({}),
            &host,
        ));
        assert!(!t.is_empty(), "{bad}");
    }
}

#[test]
fn reports_logs_and_artifacts_are_capped_and_journaled() {
    let host = FakeHost::new();
    run(
        r#"
def main(args):
    log("starting")
    report({"finding": "x"}, artifact_id = "summary")
    report("plain text")
    artifact.markdown("summary", "Summary", "hi")
    artifact.markdown("summary", "Summary", "hi again")
    artifact.table("t", "Table", ["a", "b"], [[1, 2], {"a": 3, "b": 4}])
    artifact.metrics("m", "Metrics", {"tests": 12, "failed": 0})
    artifact.metrics("m2", "Metrics", [{"label": "cov", "value": 81.5, "unit": "%"}])
    artifact.file("f", "A file", "docs/x.md")
    return None
"#,
        json!({}),
        &host,
    )
    .unwrap();
    let rec = host.recorded();
    assert!(rec.contains(&Recorded::Log("starting".into())));
    assert!(rec.contains(&Recorded::Report(
        json!({"finding": "x"}),
        Some("summary".into())
    )));
    let versions: Vec<u32> = rec
        .iter()
        .filter_map(|e| match e {
            Recorded::Artifact(a) if a.id == "summary" => Some(a.version),
            _ => None,
        })
        .collect();
    assert_eq!(versions, [1, 2]);

    // Too many ids.
    let mut src = String::from("def main(args):\n    log(\"x\")\n");
    for i in 0..33 {
        src.push_str(&format!("    artifact.markdown(\"id{i}\", \"t\", \"x\")\n"));
    }
    let t = err_text(run(&src, json!({}), &FakeHost::new()));
    assert!(t.contains("at most 32 distinct artifact ids"), "{t}");
    // Too many versions of one id.
    let mut src = String::from("def main(args):\n    log(\"x\")\n");
    src.push_str("    for i in range(17):\n        pass\n");
    let t = err_text(run(
        "def main(args):\n    log(\"x\")\n    for i in range(17):\n        artifact.markdown(\"a\", \"t\", \"x\")\n    return None\n",
        json!({}),
        &FakeHost::new(),
    ));
    assert!(t.contains("16 versions"), "{t}");
    // Oversized report.
    let t = err_text(run(
        "def main(args):\n    log(\"x\")\n    report(\"x\" * 20000)\n",
        json!({}),
        &FakeHost::new(),
    ));
    assert!(t.contains("16384"), "{t}");
    // Artifact ids must be literal.
    let d = diagnostics("def main(args):\n    i = \"a\"\n    artifact.markdown(i, \"t\", \"x\")\n");
    assert!(d[0].contains("string literal"), "{d:?}");
}

#[test]
fn args_are_frozen() {
    let host = FakeHost::new();
    let t = err_text(run(
        "def main(args):\n    args[\"x\"] = 1\n    return args\n",
        json!({"y": 2}),
        &host,
    ));
    assert!(t.contains("Immutable"), "{t}");
}

#[test]
fn a_script_failure_carries_a_position() {
    let host = FakeHost::new();
    let e = run(
        "def main(args):\n    fail(\"nope: \" + args[\"why\"])\n",
        json!({"why": "because"}),
        &host,
    )
    .unwrap_err();
    let RunError::Script(d) = e else {
        panic!("{e:?}")
    };
    assert_eq!((d.line, d.col), (2, 5));
    assert!(d.message.contains("nope: because"));
}

#[test]
fn main_must_return_json() {
    let host = FakeHost::new();
    let t = err_text(run(
        "def main(args):\n    return agent(\"a\")\n",
        json!({}),
        &host,
    ));
    assert!(t.contains("JSON-able"), "{t}");
}

#[test]
fn f_strings_take_bare_names_only() {
    let host = FakeHost::new();
    let out = run(
        "def main(args):\n    n = 3\n    return f\"{n} items\"\n",
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(out, json!("3 items"));
    let d = diagnostics("def main(args):\n    a = struct(b = 1)\n    return f\"{a.b}\"\n");
    assert!(
        !d.is_empty(),
        "attribute access in an f-string must not parse"
    );
}

#[test]
fn json_helpers_and_sorted_are_available() {
    let host = FakeHost::new();
    let out = run(
        "def main(args):\n    return [json.encode({\"a\": 1}), json.decode(\"[1, 2]\"), sorted([\"b\", \"a\"])]\n",
        json!({}),
        &host,
    )
    .unwrap();
    assert_eq!(out, json!(["{\"a\":1}", [1, 2], ["a", "b"]]));
}
