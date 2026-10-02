//! Saved workflows against a real engine core: listing and shadowing, starting
//! with arguments (validated before anyone is asked), the approval that shows
//! them, saving through the approval question, atomic and path-safe writes,
//! rescans, resume of a saved run, and per-workflow run history.

mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use support::*;
use zeron_engine::ask::{AskError, FakeAsk, FakeReply};
use zeron_engine::workflow::{
    Approval, Approver, SaveError, SaveSource, SavedContext, SavedStart, StartError, StartRequest,
    WorkflowService,
};
use zeron_proto::{
    ChatConfig, HarnessId, SandboxLevel, SavedArg, SavedArgType, SavedScope, UserInputQuestion,
    WorkflowRun, WorkflowStatus, WorkflowStopReason,
};
use zeron_rpc::methods;

const CHAT: &str = "main";
const OTHER_CHAT: &str = "other";

struct Rig {
    env: Env,
    ask: Arc<FakeAsk>,
    svc: WorkflowService,
    approver: Arc<ScriptedApprover>,
    project: PathBuf,
    global: PathBuf,
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

fn quick_agent() -> Handler {
    Arc::new(|_, _request, out, _controls| text_turn(&out, "ok", Some((10, 5))))
}

fn chat_config() -> ChatConfig {
    ChatConfig {
        harness: HarnessId::Mock,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
    }
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project").canonicalize_or_create();
    let env = assemble(&dir.path().join("data"), Default::default(), quick_agent());
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
    for chat in [CHAT, OTHER_CHAT] {
        env.core
            .workspace
            .create_chat(
                chat,
                Some(&space),
                None,
                Some(chat_config()),
                Some(project.to_string_lossy().into_owned()),
            )
            .unwrap();
    }
    let ask = FakeAsk::new();
    env.core.doc_host.set_ask_backend(ask.clone());
    let svc = env.core.doc_host.workflows().expect("workflow service");
    let approver = Arc::new(ScriptedApprover {
        answer: Mutex::new(Approval::Approved),
        asked: Mutex::new(Vec::new()),
    });
    svc.set_approver(approver.clone());
    let global = dir.path().join("home").join("workflows");
    svc.set_global_workflows_dir(global.clone());
    Rig {
        env,
        ask,
        svc,
        approver,
        project,
        global,
        _dir: dir,
    }
}

trait CanonicalOrCreate {
    fn canonicalize_or_create(self) -> PathBuf;
}

impl CanonicalOrCreate for PathBuf {
    fn canonicalize_or_create(self) -> PathBuf {
        std::fs::create_dir_all(&self).unwrap();
        self.canonicalize().unwrap()
    }
}

impl Rig {
    fn questions(&self) -> Vec<UserInputQuestion> {
        self.approver.asked.lock().unwrap().clone()
    }

    fn answer(&self, a: Approval) {
        *self.approver.answer.lock().unwrap() = a;
    }

    fn project_file(&self, name: &str) -> PathBuf {
        self.project
            .join(".zeron/workflows")
            .join(format!("{name}.star"))
    }

    fn ctx(&self) -> SavedContext {
        SavedContext {
            chat_id: Some(CHAT.into()),
            space_id: None,
        }
    }
}

fn text(s: &str) -> FakeReply {
    FakeReply::Result(json!({ "text": s }))
}

/// A saved workflow that echoes its arguments back as its result.
const ECHO: &str = r#"# zeron-workflow
# name: echo
# description: Returns what it was given
# when_to_use: In tests
# args:
#   base: {type: string, default: "main", description: "Branch"}
#   deep: {type: bool, default: false}
#   rounds: {type: int, required: true}
#   note: {type: json}

def main(args):
    phase("work")
    r = agent("worker").ask("work on " + args["base"]).result()
    return {"args": args, "answer": r.value}
"#;

fn put_project(rig: &Rig, name: &str, text: &str) {
    let dir = rig.project.join(".zeron/workflows");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.star")), text).unwrap();
}

fn put_global(rig: &Rig, name: &str, text: &str) {
    std::fs::create_dir_all(&rig.global).unwrap();
    std::fs::write(rig.global.join(format!("{name}.star")), text).unwrap();
}

fn saved(name: &str, args: Value) -> StartRequest {
    StartRequest {
        saved: Some(SavedStart {
            name: name.into(),
            scope: None,
            args,
        }),
        ..StartRequest::default()
    }
}

async fn wait_settled(rig: &Rig, run_id: &str) -> WorkflowRun {
    let deadline = std::time::Instant::now() + Duration::from_secs(40);
    loop {
        if let Ok(v) = rig.svc.get(run_id)
            && v.run.header.status.is_settled()
        {
            return v.run;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the run to settle"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ── listing ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_has_every_scope_shadowing_and_names_bad_files() {
    let rig = rig();
    put_global(
        &rig,
        "pr-review",
        "# zeron-workflow\n# description: my global take\n\ndef main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n",
    );
    put_project(&rig, "echo", ECHO);
    put_project(
        &rig,
        "broken",
        "# zeron-workflow\n# nope: 1\n# description: d\n",
    );

    let list = rig.svc.saved_list(&rig.ctx(), false).unwrap();
    let find = |name: &str, scope| {
        list.workflows
            .iter()
            .find(|w| w.name == name && w.scope == scope)
            .unwrap_or_else(|| panic!("{name} {scope:?}"))
    };
    assert_eq!(find("echo", SavedScope::Project).args.len(), 4);
    assert_eq!(
        find("echo", SavedScope::Project).when_to_use.as_deref(),
        Some("In tests")
    );
    assert_eq!(
        find("echo", SavedScope::Project).space_id.as_deref(),
        Some("space-main")
    );
    // Global shadows the built-in of the same name; both are listed and say so.
    assert_eq!(
        find("pr-review", SavedScope::Global).shadows,
        [SavedScope::Builtin]
    );
    assert_eq!(
        find("pr-review", SavedScope::Builtin).shadowed_by,
        Some(SavedScope::Global)
    );
    assert!(
        list.workflows
            .iter()
            .any(|w| w.name == "fix-until-green" && w.scope == SavedScope::Builtin)
    );
    assert_eq!(list.invalid.len(), 1);
    assert!(
        list.invalid[0].reason.contains("unknown key `nope`"),
        "{:?}",
        list.invalid
    );
    assert!(list.invalid[0].path.ends_with("broken.star"));

    // `all` covers every project of the device (here: the one).
    let all = rig.svc.saved_list(&SavedContext::default(), true).unwrap();
    assert!(
        all.workflows
            .iter()
            .any(|w| w.name == "echo" && w.scope == SavedScope::Project)
    );
    // Without a project there are no project workflows.
    let none = rig.svc.saved_list(&SavedContext::default(), false).unwrap();
    assert!(
        none.workflows
            .iter()
            .all(|w| w.scope != SavedScope::Project)
    );
    assert!(
        rig.svc
            .saved_list(
                &SavedContext {
                    chat_id: Some("nope".into()),
                    space_id: None
                },
                false
            )
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edits_on_disk_are_picked_up_without_a_restart() {
    let rig = rig();
    assert!(
        !rig.svc
            .saved_list(&rig.ctx(), false)
            .unwrap()
            .workflows
            .iter()
            .any(|w| w.name == "echo")
    );
    put_project(&rig, "echo", ECHO);
    assert!(
        rig.svc
            .saved_list(&rig.ctx(), false)
            .unwrap()
            .workflows
            .iter()
            .any(|w| w.name == "echo")
    );
    put_project(
        &rig,
        "echo",
        &ECHO.replace("Returns what it was given", "Edited in an editor"),
    );
    let detail = rig.svc.saved_get(&rig.ctx(), "echo", None).unwrap();
    assert_eq!(detail.summary.description, "Edited in an editor");
    assert!(detail.script.contains("def main(args)"));
    assert_eq!(detail.graph.unwrap().phase_names(), ["work"]);
    std::fs::remove_file(rig.project_file("echo")).unwrap();
    assert!(rig.svc.saved_get(&rig.ctx(), "echo", None).is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_reports_analysis_problems_instead_of_hiding_the_file() {
    let rig = rig();
    put_project(
        &rig,
        "oops",
        "# zeron-workflow\n# description: has a loop\n\ndef main(args):\n    while True:\n        pass\n",
    );
    let detail = rig.svc.saved_get(&rig.ctx(), "oops", None).unwrap();
    assert!(detail.graph.is_none());
    assert!(
        detail.diagnostics[0].starts_with("oops.star:5:"),
        "{:?}",
        detail.diagnostics
    );
}

// ── starting ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_saved_workflow_starts_with_defaults_filled_and_the_run_remembers_where_it_came_from() {
    let rig = rig();
    put_project(&rig, "echo", ECHO);
    rig.ask
        .on_call(|call| Some(text(&call.spec.prompt.chars().take(0).collect::<String>())));
    let out = rig
        .svc
        .start(CHAT, saved("echo", json!({"rounds": 3, "base": "dev"})))
        .await
        .expect("starts");
    assert_eq!(out.name, "echo");
    assert_eq!(out.graph.phase_names(), ["work"]);
    assert_eq!(
        out.draft_path.as_deref(),
        Some(".zeron/workflows/echo.star"),
        "the file is its own draft"
    );
    assert!(!rig.project.join(".zeron/workflow-drafts").exists());
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(
        run.header.status,
        WorkflowStatus::Completed,
        "{:?}",
        run.header
    );
    assert_eq!(run.header.saved_name.as_deref(), Some("echo"));
    assert_eq!(run.header.saved_scope, Some(SavedScope::Project));
    let view = rig.svc.get(&out.run_id).unwrap();
    // Defaults filled, absent optional left out, and the script saw exactly this.
    let expected = json!({"base": "dev", "deep": false, "rounds": 3});
    assert_eq!(view.result.as_ref().unwrap()["args"], expected);
    assert_eq!(view.args, expected);
    assert_eq!(view.saved.unwrap().name, "echo");
    // The whole file, frontmatter included, is the pinned script.
    let stored = rig.svc.store().read_script(&out.run_id).unwrap();
    assert_eq!(
        stored,
        std::fs::read_to_string(rig.project_file("echo")).unwrap()
    );
    // The approval named it and showed the arguments, structured and as text.
    let q = &rig.questions()[0];
    assert!(
        q.question.contains("Saved workflow: echo (project)"),
        "{}",
        q.question
    );
    assert!(
        q.question.contains("base = \"dev\"") && q.question.contains("rounds = 3"),
        "{}",
        q.question
    );
    let meta = q.meta.as_ref().unwrap();
    assert_eq!(meta["saved"], json!({"name": "echo", "scope": "project"}));
    assert_eq!(meta["args"], expected);
    assert_eq!(q.options, ["Run workflow", "Deny"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_arguments_are_rejected_with_every_problem_before_anyone_is_asked() {
    let rig = rig();
    put_project(&rig, "echo", ECHO);
    let err = rig
        .svc
        .start(
            CHAT,
            saved("echo", json!({"base": 5, "deep": "yes", "nope": 1})),
        )
        .await
        .unwrap_err();
    let StartError::Invalid(msg) = err else {
        panic!("{err:?}")
    };
    for needle in [
        "unknown argument 'nope' (declared: base, deep, rounds, note)",
        "argument 'base': expected a string",
        "argument 'deep': expected a bool",
        "missing required argument 'rounds'",
    ] {
        assert!(msg.contains(needle), "{needle} in {msg}");
    }
    assert!(
        rig.questions().is_empty(),
        "no approval for a call that cannot run"
    );
    assert!(rig.svc.list(Some(CHAT)).is_empty(), "no run was created");
    // A script with problems is also rejected before the question.
    put_project(
        &rig,
        "oops",
        "# zeron-workflow\n# description: d\n\ndef main(args):\n    while True:\n        pass\n",
    );
    let err = rig
        .svc
        .start(CHAT, saved("oops", json!({})))
        .await
        .unwrap_err();
    assert!(matches!(err, StartError::Diagnostics(_)), "{err:?}");
    assert!(rig.questions().is_empty());
    // Unknown names list what exists.
    let err = rig
        .svc
        .start(CHAT, saved("nope", json!({})))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("no saved workflow named \"nope\" (available:"),
        "{err}"
    );
    // Exactly one source.
    let mut both = saved("echo", json!({"rounds": 1}));
    both.script = Some("x".into());
    assert!(rig.svc.start(CHAT, both).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn denying_the_approval_stops_a_saved_run_like_any_other() {
    let rig = rig();
    put_project(&rig, "echo", ECHO);
    rig.answer(Approval::Denied("the user denied it".into()));
    let err = rig
        .svc
        .start(CHAT, saved("echo", json!({"rounds": 1})))
        .await
        .unwrap_err();
    assert!(matches!(err, StartError::Denied(_)));
    let runs = rig.svc.list(Some(CHAT));
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, WorkflowStatus::Stopped);
    assert_eq!(runs[0].stop_reason, Some(WorkflowStopReason::Denied));
    assert_eq!(
        runs[0].saved_name.as_deref(),
        Some("echo"),
        "even a denied run is attributed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_launching_a_saved_workflow_is_the_approval_but_an_agents_script_never_is() {
    let rig = rig();
    put_project(&rig, "echo", ECHO);
    rig.ask.on_call(|_| Some(text("ok")));
    let mut req = saved("echo", json!({"rounds": 1}));
    req.by_user = true;
    let out = rig.svc.start(CHAT, req).await.unwrap();
    assert!(
        rig.questions().is_empty(),
        "the launcher dialog was the approval"
    );
    wait_settled(&rig, &out.run_id).await;
    // `by_user` does nothing for a raw script.
    let mut raw = StartRequest {
        script: Some(
            "def main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n".into(),
        ),
        by_user: true,
        ..StartRequest::default()
    };
    raw.name = Some("raw".into());
    let out = rig.svc.start(CHAT, raw).await.unwrap();
    assert_eq!(
        rig.questions().len(),
        1,
        "an agent's script is still asked about"
    );
    wait_settled(&rig, &out.run_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scope_picks_the_file_and_project_wins_by_default() {
    let rig = rig();
    let body = |who: &str| {
        format!(
            "# zeron-workflow\n# description: {who}\n\ndef main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n    return \"{who}\"\n"
        )
    };
    put_global(&rig, "twin", &body("global"));
    put_project(&rig, "twin", &body("project"));
    rig.ask.on_call(|_| Some(text("ok")));
    let out = rig.svc.start(CHAT, saved("twin", json!({}))).await.unwrap();
    assert_eq!(
        wait_settled(&rig, &out.run_id).await.header.saved_scope,
        Some(SavedScope::Project)
    );
    assert_eq!(
        rig.svc.get(&out.run_id).unwrap().result.unwrap(),
        json!("project")
    );
    let mut req = saved("twin", json!({}));
    req.saved.as_mut().unwrap().scope = Some(SavedScope::Global);
    let out = rig.svc.start(CHAT, req).await.unwrap();
    wait_settled(&rig, &out.run_id).await;
    assert_eq!(
        rig.svc.get(&out.run_id).unwrap().result.unwrap(),
        json!("global")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_built_in_runs_with_no_arguments() {
    let rig = rig();
    rig.ask.on_call(|_| Some(text("ok")));
    // Nothing changed in the (empty) project: pr-review ends at once with an honest result.
    let out = rig
        .svc
        .start(CHAT, saved("pr-review", json!({})))
        .await
        .unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.saved_scope, Some(SavedScope::Builtin));
    assert!(out.draft_path.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn args_stay_frozen_inside_the_script() {
    let rig = rig();
    put_project(
        &rig,
        "mutates",
        "# zeron-workflow\n# description: tries to change its args\n# args:\n#   n: {type: int, default: 1}\n\ndef main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n    args[\"n\"] = 2\n    return args\n",
    );
    rig.ask.on_call(|_| Some(text("ok")));
    let out = rig
        .svc
        .start(CHAT, saved("mutates", json!({})))
        .await
        .unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(
        run.header.status,
        WorkflowStatus::Errored,
        "{:?}",
        run.header
    );
}

// ── resume keeps the pin ──────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resuming_a_saved_run_replays_the_pinned_file_not_the_edited_one() {
    let rig = rig();
    put_project(&rig, "echo", ECHO);
    let broken = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let b = broken.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = calls.clone();
    rig.ask.on_call(move |_| {
        c.fetch_add(1, Ordering::SeqCst);
        Some(if b.load(Ordering::SeqCst) {
            FakeReply::Fail(AskError::TurnFailed(
                "401 Unauthorized: invalid API key".into(),
            ))
        } else {
            text("ok")
        })
    });
    let out = rig
        .svc
        .start(CHAT, saved("echo", json!({"rounds": 2})))
        .await
        .unwrap();
    let run = wait_settled(&rig, &out.run_id).await;
    assert_eq!(run.header.stop_reason, Some(WorkflowStopReason::Provider));
    let pinned = rig.svc.store().read_script(&out.run_id).unwrap();

    // The author edits the file (and breaks nothing the run needs).
    put_project(
        &rig,
        "echo",
        &ECHO.replace("Returns what it was given", "Edited after the run"),
    );
    broken.store(false, Ordering::SeqCst);
    let resumed = rig.svc.resume(&out.run_id, None, true).await.unwrap();
    let run2 = wait_settled(&rig, &resumed.run_id).await;
    assert_eq!(
        run2.header.status,
        WorkflowStatus::Completed,
        "{:?}",
        run2.header
    );
    assert_eq!(
        rig.svc.store().read_script(&resumed.run_id).unwrap(),
        pinned,
        "the resume runs the pinned copy"
    );
    assert_eq!(
        run2.header.saved_name.as_deref(),
        Some("echo"),
        "a resumed run is still that workflow's"
    );
    assert_eq!(run2.header.saved_scope, Some(SavedScope::Project));
    // The args of the original run came along.
    assert_eq!(
        rig.svc.get(&resumed.run_id).unwrap().args,
        json!({"base": "main", "deep": false, "rounds": 2})
    );
    // Different args are refused (the pin covers inputs too).
    assert!(
        rig.svc
            .resume(&out.run_id, Some(json!({"rounds": 9})), true)
            .await
            .is_err()
    );
}

// ── saving ────────────────────────────────────────────────────────────────

fn save_req(name: &str, scope: SavedScope, script: &str) -> zeron_engine::workflow::SaveRequest {
    zeron_engine::workflow::SaveRequest {
        name: name.into(),
        description: "Does the thing".into(),
        when_to_use: Some("When asked".into()),
        args: Some(vec![SavedArg {
            name: "depth".into(),
            ty: SavedArgType::Int,
            required: false,
            default: Some(json!(2)),
            description: Some("How deep".into()),
        }]),
        scope,
        source: SaveSource::Script(script.into()),
        by_user: false,
        overwrite: false,
    }
}

const SCRIPT: &str = "def main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n    return args.get(\"depth\")\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saving_asks_the_user_with_the_stock_labels_then_writes_atomically() {
    let rig = rig();
    let out = rig
        .svc
        .saved_save(CHAT, save_req("mine", SavedScope::Project, SCRIPT))
        .await
        .unwrap();
    assert_eq!(out.path, rig.project_file("mine"));
    assert!(!out.overwrote);
    let q = &rig.questions()[0];
    assert_eq!(q.options, ["Save workflow", "Deny"]);
    assert!(
        q.question
            .contains("Save workflow \"mine\" to this project's workflows?"),
        "{}",
        q.question
    );
    assert!(
        q.question.contains("File: .zeron/workflows/mine.star"),
        "{}",
        q.question
    );
    assert!(
        q.question.contains("Arguments: depth (int, default 2)"),
        "{}",
        q.question
    );
    assert!(q.question.contains("When to use: When asked"));
    assert!(!q.question.contains("REPLACES"));
    let file = std::fs::read_to_string(rig.project_file("mine")).unwrap();
    assert!(file.starts_with("# zeron-workflow\n# name: mine\n# description: Does the thing\n# when_to_use: When asked\n# args:\n#   depth: {type: int, default: 2, description: \"How deep\"}\n\n"), "{file}");
    assert!(file.ends_with(SCRIPT));
    // It is immediately listed and runnable.
    let list = rig.svc.saved_list(&rig.ctx(), false).unwrap();
    let mine = list.workflows.iter().find(|w| w.name == "mine").unwrap();
    assert_eq!(mine.args[0].default, Some(json!(2)));
    assert_eq!(out.summary.name, "mine");
    // No temp files remain.
    let names: Vec<_> = std::fs::read_dir(rig.project.join(".zeron/workflows"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["mine.star"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_denied_save_writes_nothing() {
    let rig = rig();
    rig.answer(Approval::Denied("the user denied it".into()));
    let err = rig
        .svc
        .saved_save(CHAT, save_req("mine", SavedScope::Global, SCRIPT))
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("not saved:"), "{err}");
    assert!(!rig.global.exists() || std::fs::read_dir(&rig.global).unwrap().count() == 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overwriting_is_asked_about_and_refusal_keeps_the_old_file() {
    let rig = rig();
    rig.svc
        .saved_save(CHAT, save_req("mine", SavedScope::Global, SCRIPT))
        .await
        .unwrap();
    let path = rig.global.join("mine.star");
    let before = std::fs::read_to_string(&path).unwrap();
    let mut changed = save_req("mine", SavedScope::Global, SCRIPT);
    changed.description = "A different description".into();
    rig.answer(Approval::Denied("the user denied it".into()));
    assert!(rig.svc.saved_save(CHAT, changed.clone()).await.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    assert!(
        rig.questions()
            .last()
            .unwrap()
            .question
            .contains("This REPLACES the existing file")
    );
    rig.answer(Approval::Approved);
    let out = rig.svc.saved_save(CHAT, changed).await.unwrap();
    assert!(out.overwrote);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("A different description")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_question_says_what_a_save_shadows() {
    let rig = rig();
    put_global(
        &rig,
        "twin",
        "# zeron-workflow\n# description: g\n\ndef main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n",
    );
    rig.svc
        .saved_save(CHAT, save_req("twin", SavedScope::Project, SCRIPT))
        .await
        .unwrap();
    assert!(
        rig.questions()[0]
            .question
            .contains("It will hide the global workflow of the same name here."),
        "{}",
        rig.questions()[0].question
    );
    rig.svc
        .saved_save(CHAT, save_req("pr-review", SavedScope::Global, SCRIPT))
        .await
        .unwrap();
    assert!(
        rig.questions()[1]
            .question
            .contains("It will hide the built-in workflow"),
        "{}",
        rig.questions()[1].question
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_is_asked_and_nothing_written_for_a_save_that_cannot_work() {
    let rig = rig();
    for bad in [
        "../evil",
        "a/b",
        "..",
        "UPPER",
        "",
        "con",
        "x.star",
        "with space",
    ] {
        let err = rig
            .svc
            .saved_save(CHAT, save_req(bad, SavedScope::Project, SCRIPT))
            .await
            .unwrap_err();
        assert!(
            matches!(err, SaveError::Invalid(_) | SaveError::Saved(_)),
            "{bad:?}: {err}"
        );
    }
    assert!(rig.questions().is_empty());
    assert!(
        !rig.project.join(".zeron").exists(),
        "not even the folder was created"
    );
    assert!(!rig.project.parent().unwrap().join("evil.star").exists());
    // Broken script: diagnostics, no question.
    let err = rig
        .svc
        .saved_save(
            CHAT,
            save_req(
                "ok-name",
                SavedScope::Global,
                "def main(args):\n    while True:\n        pass\n",
            ),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains(".star:"), "{err}");
    // Bad frontmatter values.
    let mut m = save_req("ok-name", SavedScope::Global, SCRIPT);
    m.description = "two\nlines".into();
    assert!(rig.svc.saved_save(CHAT, m).await.is_err());
    let mut m = save_req("ok-name", SavedScope::Global, SCRIPT);
    m.args = Some(vec![SavedArg {
        name: "n".into(),
        ty: SavedArgType::Int,
        required: false,
        default: Some(json!("x")),
        description: None,
    }]);
    assert!(rig.svc.saved_save(CHAT, m).await.is_err());
    // Built-ins cannot be written.
    assert!(
        rig.svc
            .saved_save(CHAT, save_req("mine", SavedScope::Builtin, SCRIPT))
            .await
            .is_err()
    );
    assert!(rig.questions().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saving_from_a_run_uses_that_runs_script_and_only_your_own_chats() {
    let rig = rig();
    rig.ask.on_call(|_| Some(text("ok")));
    let adhoc = rig
        .svc
        .start(
            CHAT,
            StartRequest {
                script: Some(SCRIPT.into()),
                name: Some("adhoc".into()),
                ..StartRequest::default()
            },
        )
        .await
        .unwrap();
    wait_settled(&rig, &adhoc.run_id).await;
    let mut req = save_req("kept", SavedScope::Global, "ignored");
    req.source = SaveSource::FromRun(adhoc.run_id.clone());
    let out = rig.svc.saved_save(CHAT, req.clone()).await.unwrap();
    let file = std::fs::read_to_string(&out.path).unwrap();
    assert!(
        file.ends_with(SCRIPT),
        "the run's script, not the text given: {file}"
    );
    // Another chat cannot save this chat's run.
    let err = rig.svc.saved_save(OTHER_CHAT, req).await.unwrap_err();
    assert!(err.to_string().contains("belongs to another chat"), "{err}");
    // Unknown run.
    let mut req = save_req("kept2", SavedScope::Global, "x");
    req.source = SaveSource::FromRun("nope".into());
    assert!(rig.svc.saved_save(CHAT, req).await.is_err());
    // Re-saving a saved run keeps its declared args when none are given, and
    // never duplicates the frontmatter.
    put_project(&rig, "echo", ECHO);
    rig.ask.on_call(|_| Some(text("ok")));
    let run = rig
        .svc
        .start(CHAT, saved("echo", json!({"rounds": 1})))
        .await
        .unwrap();
    wait_settled(&rig, &run.run_id).await;
    let mut req = save_req("echo-copy", SavedScope::Global, "x");
    req.source = SaveSource::FromRun(run.run_id);
    req.args = None;
    let out = rig.svc.saved_save(CHAT, req).await.unwrap();
    let file = std::fs::read_to_string(&out.path).unwrap();
    assert_eq!(file.matches("# zeron-workflow").count(), 1, "{file}");
    assert!(
        file.contains("# name: echo-copy")
            && file.contains("#   rounds: {type: int, required: true}"),
        "{file}"
    );
    assert_eq!(out.summary.args.len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dialog_save_needs_no_question_but_still_refuses_to_replace_by_accident() {
    let rig = rig();
    let mut req = save_req("mine", SavedScope::Project, SCRIPT);
    req.by_user = true;
    rig.svc.saved_save(CHAT, req.clone()).await.unwrap();
    assert!(rig.questions().is_empty());
    let err = rig.svc.saved_save(CHAT, req.clone()).await.unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
    req.overwrite = true;
    assert!(rig.svc.saved_save(CHAT, req).await.unwrap().overwrote);
    assert!(rig.questions().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_one_file_and_refuses_built_ins() {
    let rig = rig();
    put_project(&rig, "echo", ECHO);
    put_global(&rig, "gone", "# zeron-workflow\n# description: d\n");
    rig.svc
        .saved_delete(&rig.ctx(), "echo", SavedScope::Project)
        .unwrap();
    assert!(!rig.project_file("echo").exists());
    assert!(
        rig.svc
            .saved_delete(&rig.ctx(), "echo", SavedScope::Project)
            .is_err()
    );
    rig.svc
        .saved_delete(&SavedContext::default(), "gone", SavedScope::Global)
        .unwrap();
    assert!(!rig.global.join("gone.star").exists());
    assert!(
        rig.svc
            .saved_delete(&rig.ctx(), "pr-review", SavedScope::Builtin)
            .is_err()
    );
    assert!(
        rig.svc
            .saved_delete(&rig.ctx(), "../x", SavedScope::Global)
            .is_err()
    );
    assert!(
        rig.svc
            .saved_delete(&SavedContext::default(), "x", SavedScope::Project)
            .is_err()
    );
}

// ── run history ───────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflows_runs_are_listed_newest_first_per_workflow_and_project() {
    let rig = rig();
    put_project(&rig, "echo", ECHO);
    put_global(
        &rig,
        "other",
        "# zeron-workflow\n# description: d\n\ndef main(args):\n    phase(\"p\")\n    agent(\"a\").ask(\"x\").result()\n",
    );
    rig.ask.on_call(|_| Some(text("ok")));
    let mut ids = Vec::new();
    for i in 1..=3 {
        let out = rig
            .svc
            .start(CHAT, saved("echo", json!({"rounds": i})))
            .await
            .unwrap();
        wait_settled(&rig, &out.run_id).await;
        ids.push(out.run_id);
        tokio::time::sleep(Duration::from_millis(3)).await;
    }
    let o = rig
        .svc
        .start(CHAT, saved("other", json!({})))
        .await
        .unwrap();
    wait_settled(&rig, &o.run_id).await;
    let project = rig.svc.saved_project(&rig.ctx()).unwrap();
    let runs = rig
        .svc
        .saved_runs("echo", SavedScope::Project, project.as_ref(), 10);
    assert_eq!(
        runs.iter().map(|h| h.run_id.clone()).collect::<Vec<_>>(),
        [ids[2].clone(), ids[1].clone(), ids[0].clone()]
    );
    assert!(runs.iter().all(|h| h.saved_name.as_deref() == Some("echo")));
    assert_eq!(
        rig.svc
            .saved_runs("echo", SavedScope::Project, project.as_ref(), 2)
            .len(),
        2
    );
    assert_eq!(
        rig.svc
            .saved_runs("other", SavedScope::Global, None, 10)
            .len(),
        1
    );
    assert!(
        rig.svc
            .saved_runs("echo", SavedScope::Global, None, 10)
            .is_empty(),
        "scope matters"
    );
    // A project workflow of the same name in another project has its own history.
    let elsewhere = saved_project_elsewhere(&rig);
    assert!(
        rig.svc
            .saved_runs("echo", SavedScope::Project, Some(&elsewhere), 10)
            .is_empty()
    );
}

fn saved_project_elsewhere(rig: &Rig) -> zeron_engine::workflow::saved::ProjectRef {
    let other = rig.project.parent().unwrap().join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    zeron_engine::workflow::saved::ProjectRef {
        root: other.canonicalize().unwrap(),
        space_id: None,
    }
}

// ── over RPC ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_rpc_surface_lists_gets_saves_starts_and_deletes() {
    let rig = rig();
    let client = Arc::new(zeron_rpc::memory_client(rig.env.core.rpc_service()));
    rig.ask.on_call(|_| Some(text("ok")));

    // Save by the dialog's own authority.
    let saved_out: Value = client
        .call_as(
            methods::WORKFLOW_SAVED_SAVE,
            json!({
                "chatId": CHAT, "name": "rpc-made", "description": "Made over RPC",
                "scope": "project", "script": SCRIPT, "byUser": true,
                "args": {"depth": {"type": "int", "default": 4}},
            }),
        )
        .await
        .unwrap();
    assert_eq!(saved_out["workflow"]["name"], "rpc-made");
    assert_eq!(saved_out["overwrote"], false);
    // Listing for a project and for everything.
    let list: Value = client
        .call_as(methods::WORKFLOW_SAVED_LIST, json!({"chatId": CHAT}))
        .await
        .unwrap();
    assert!(
        list["workflows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["name"] == "rpc-made" && w["scope"] == "project")
    );
    let all: Value = client
        .call_as(methods::WORKFLOW_SAVED_LIST, json!({"all": true}))
        .await
        .unwrap();
    assert!(
        all["workflows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["name"] == "rpc-made")
    );
    // Detail.
    let detail: Value = client
        .call_as(
            methods::WORKFLOW_SAVED_GET,
            json!({"name": "rpc-made", "chatId": CHAT}),
        )
        .await
        .unwrap();
    assert_eq!(detail["description"], "Made over RPC");
    assert!(
        detail["script"]
            .as_str()
            .unwrap()
            .contains("# zeron-workflow")
    );
    assert_eq!(detail["graph"]["phases"][0]["name"], "p");
    // Start with args (+ by_user).
    let started: Value = client
        .call_as(
            methods::WORKFLOW_START,
            json!({"chatId": CHAT, "saved": {"name": "rpc-made", "args": {"depth": 9}}, "byUser": true}),
        )
        .await
        .unwrap();
    let run_id = started["runId"].as_str().unwrap().to_owned();
    wait_settled(&rig, &run_id).await;
    let got: Value = client
        .call_as(methods::WORKFLOW_GET, json!({"runId": run_id}))
        .await
        .unwrap();
    assert_eq!(got["args"], json!({"depth": 9}));
    assert_eq!(
        got["saved"],
        json!({"name": "rpc-made", "scope": "project"})
    );
    assert_eq!(got["run"]["savedName"], "rpc-made");
    // History.
    let runs: Value = client
        .call_as(
            methods::WORKFLOW_SAVED_RUNS,
            json!({"name": "rpc-made", "scope": "project", "chatId": CHAT}),
        )
        .await
        .unwrap();
    assert_eq!(runs.as_array().unwrap().len(), 1);
    // Bad args come back as an error naming everything.
    let err = client
        .call_as::<Value>(
            methods::WORKFLOW_START,
            json!({"chatId": CHAT, "saved": {"name": "rpc-made", "args": {"depth": "x", "zzz": 1}}, "byUser": true}),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("unknown argument 'zzz'") && err.contains("argument 'depth'"),
        "{err}"
    );
    // Delete.
    let del: Value = client
        .call_as(
            methods::WORKFLOW_SAVED_DELETE,
            json!({"name": "rpc-made", "scope": "project", "chatId": CHAT}),
        )
        .await
        .unwrap();
    assert_eq!(del["deleted"], true);
    assert!(!rig.project_file("rpc-made").exists());
    // Malformed requests.
    assert!(
        client
            .call_as::<Value>(
                methods::WORKFLOW_SAVED_SAVE,
                json!({"chatId": CHAT, "name": "x", "description": "d", "scope": "global"})
            )
            .await
            .is_err(),
        "needs fromRun or script"
    );
    assert!(client.call_as::<Value>(methods::WORKFLOW_SAVED_SAVE, json!({"chatId": CHAT, "name": "x", "description": "d", "scope": "global", "script": SCRIPT, "args": 5, "byUser": true})).await.is_err());
}
