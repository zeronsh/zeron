//! Zeron's permission policy over ACP, against the fake agent in
//! `tests/fixtures/fake-policy-acp.py` (Devin's mode select and option set;
//! Hermes-style legacy modes when launched as `*legacy*`).

#![cfg(unix)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{AcpHarness, CancellationToken, Harness, RunControls};
use zeron_proto::{
    AgentEvent, AgentPolicy, DoneStatus, PermissionMode, PolicyCaps, RunRequest, SandboxLevel,
    UserInputAnswer, UserInputQuestion,
};

fn fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-policy-acp.py");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

/// The fake under a `*legacy*` name: only the superseded `modes` state.
fn legacy_fixture(dir: &Path) -> PathBuf {
    let link = dir.join("fake-legacy-acp.py");
    let _ = std::os::unix::fs::symlink(fixture(), &link);
    link
}

/// The run's project folder (must exist: the agent starts in it).
fn workspace() -> PathBuf {
    let dir = std::env::temp_dir().join("zeron-acp-policy-workspace");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A file in the project.
fn in_workspace(name: &str) -> String {
    workspace().join(name).display().to_string()
}

struct Outcome {
    /// The agent's mode when the turn ran.
    mode: String,
    /// What each permission request was answered with (option id, or
    /// `cancelled`).
    answers: Vec<String>,
    /// The approval questions the user was asked.
    asked: Vec<UserInputQuestion>,
}

/// One turn on `exe` under `mode`, whose tool calls ask permission in
/// order; `answers` are the user's answers to approval questions.
async fn run(exe: &Path, mode: PermissionMode, calls: Value, answers: &[&str]) -> Outcome {
    let harness = AcpHarness::devin().with_executable(exe);
    let request = RunRequest {
        policy: AgentPolicy::with_mode(mode),
        mcp: None,
        prompt: calls.to_string(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: workspace().display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    };
    let queue: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(
        answers.iter().map(|a| (*a).to_owned()).collect(),
    ));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let recorded = asked.clone();
    let (_steer_tx, steering) = mpsc::channel(8);
    let controls = RunControls {
        execution_lease: None,
        request_input: Box::new(move |questions: Vec<UserInputQuestion>| {
            let (tx, rx) = oneshot::channel();
            let label = queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected approval question");
            recorded.lock().unwrap().extend(questions.iter().cloned());
            let _ = tx.send(
                questions
                    .into_iter()
                    .map(|q| UserInputAnswer {
                        question_id: q.id,
                        labels: vec![label.clone()],
                    })
                    .collect(),
            );
            rx
        }),
        steering,
        interrupt: CancellationToken::new(),
    };
    let stream = harness.run(request, controls).await.expect("run starts");
    let events: Vec<AgentEvent> = tokio::time::timeout(
        Duration::from_secs(15),
        stream.map(|r| r.expect("stream event")).collect(),
    )
    .await
    .expect("run finished in time");
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            }
        )),
        "{events:?}"
    );
    let report: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{events:?}"));
    Outcome {
        mode: report["mode"].as_str().unwrap_or_default().to_owned(),
        answers: report["answers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap_or_default().to_owned())
            .collect(),
        asked: asked.lock().unwrap().clone(),
    }
}

fn exec(command: &str) -> Value {
    json!({"kind":"execute", "title":command, "rawInput":{"command":command}})
}

fn edit(path: &str) -> Value {
    json!({"kind":"edit", "title":"Edit", "locations":[{"path":path}]})
}

fn read(path: &str) -> Value {
    json!({"kind":"read", "title":"Read", "locations":[{"path":path}]})
}

#[tokio::test]
async fn bypass_keeps_the_agents_bypass_mode_and_auto_accepts() {
    let out = run(
        &fixture(),
        PermissionMode::Bypass,
        json!([exec("rm -rf /")]),
        &[],
    )
    .await;
    assert_eq!(out.mode, "bypass");
    // Unchanged: the first allow_always option, without asking.
    assert_eq!(out.answers, vec!["switch_bypass"]);
    assert!(out.asked.is_empty());
}

#[tokio::test]
async fn ask_mode_routes_permissions_to_the_user_and_honours_the_answer() {
    let calls = json!([
        read("/etc/hosts"),
        exec("make deploy"),
        exec("make deploy"),
        edit(&in_workspace("src/a.rs")),
        edit(&in_workspace("src/a.rs")),
    ]);
    let out = run(
        &fixture(),
        PermissionMode::Ask,
        calls,
        &["Allow once", "Deny", "Always allow"],
    )
    .await;
    // The agent's asking mode, not its bypass.
    assert_eq!(out.mode, "normal");
    // Allowed once: the agent's allow-once, never a mode switch or a
    // standing grant; denied: its reject-once; "always" is remembered by
    // Zeron, so the repeated edit doesn't ask.
    assert_eq!(
        out.answers,
        vec![
            "allow_once",
            "allow_once",
            "reject_once",
            "allow_once",
            "allow_once"
        ]
    );
    let questions: Vec<&str> = out.asked.iter().map(|q| q.question.as_str()).collect();
    assert_eq!(
        questions,
        vec![
            "Allow the agent to run `make deploy`?",
            "Allow the agent to run `make deploy`?",
            format!("Allow the agent to edit {}?", in_workspace("src/a.rs")).as_str(),
        ]
    );
    assert_eq!(
        out.asked[0].options,
        vec!["Allow once", "Always allow", "Deny"]
    );
}

#[tokio::test]
async fn auto_mode_runs_dev_commands_and_refuses_force_pushes_without_asking() {
    let calls = json!([
        exec("cargo test -p zeron-engine"),
        exec("git push --force origin main")
    ]);
    let out = run(&fixture(), PermissionMode::Auto, calls, &[]).await;
    assert_eq!(out.mode, "normal");
    assert_eq!(out.answers, vec!["allow_once", "reject_once"]);
    assert!(out.asked.is_empty());
}

#[tokio::test]
async fn plan_mode_selects_the_agents_plan_mode_and_refuses_edits() {
    let calls = json!([
        edit(&in_workspace("src/a.rs")),
        read(&in_workspace("src/a.rs")),
        exec("git log --oneline"),
        // Leaving plan mode is the user's call in Zeron, not the agent's.
        {"kind":"switch_mode", "title":"Ready to code?", "options":[
            {"optionId":"plan_bypass","name":"Yes, implement plan and bypass permissions","kind":"allow_once"},
            {"optionId":"plan_normal","name":"Yes, implement plan","kind":"allow_once"},
            {"optionId":"keep_planning","name":"No, plan needs changes","kind":"reject_once"},
        ]},
    ]);
    let out = run(&fixture(), PermissionMode::Plan, calls, &[]).await;
    assert_eq!(out.mode, "plan");
    assert_eq!(
        out.answers,
        vec!["reject_once", "allow_once", "allow_once", "keep_planning"]
    );
    assert!(out.asked.is_empty());
}

#[tokio::test]
async fn a_reject_option_is_never_picked_as_the_allow() {
    let only_rejects = json!({"kind":"execute", "title":"cargo test", "rawInput":{"command":"cargo test"},
    "options":[
        {"optionId":"no","name":"No","kind":"reject_once"},
        {"optionId":"never","name":"Never","kind":"reject_always"},
    ]});
    // Bypass used to fall back to the first option, a reject.
    let out = run(
        &fixture(),
        PermissionMode::Bypass,
        json!([only_rejects.clone()]),
        &[],
    )
    .await;
    assert_eq!(out.answers, vec!["cancelled"]);
    // Allowed by the policy, but the only allow would switch modes.
    let widening = json!({"kind":"execute", "title":"cargo test", "rawInput":{"command":"cargo test"},
    "options":[
        {"optionId":"switch_bypass","name":"Yes, switch to bypass mode","kind":"allow_once"},
        {"optionId":"no","name":"No","kind":"reject_once"},
    ]});
    let out = run(
        &fixture(),
        PermissionMode::Auto,
        json!([only_rejects, widening]),
        &[],
    )
    .await;
    assert_eq!(out.answers, vec!["cancelled", "cancelled"]);
}

#[tokio::test]
async fn legacy_modes_switch_through_set_mode_outside_bypass_only() {
    let dir = tempfile::tempdir().unwrap();
    let exe = legacy_fixture(dir.path());
    let out = run(&exe, PermissionMode::Ask, json!([read("/etc/hosts")]), &[]).await;
    assert_eq!(out.mode, "default");
    assert_eq!(out.answers, vec!["allow_once"]);
    let out = run(&exe, PermissionMode::Bypass, json!([]), &[]).await;
    assert_eq!(out.mode, "dont_ask", "Bypass leaves the agent's mode alone");
}

#[test]
fn caps_are_honest_per_agent() {
    assert_eq!(AcpHarness::grok().policy_caps(), PolicyCaps::bypass_only());
    assert_eq!(
        AcpHarness::hermes().policy_caps(),
        PolicyCaps::bypass_only()
    );
    let devin = AcpHarness::devin().policy_caps();
    assert_eq!(devin.modes, PermissionMode::ALL.to_vec());
    assert!(devin.native_plan);
    let antigravity = AcpHarness::antigravity().policy_caps();
    assert_eq!(antigravity.modes, PermissionMode::ALL.to_vec());
    assert!(!antigravity.native_plan);
}
