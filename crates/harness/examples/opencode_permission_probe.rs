//! Live probe: a permission-gated turn through the native driver against a
//! chosen opencode binary. The workspace carries an opencode config whose
//! `permissions` rules force `ask` for file edits, the prompt demands a file
//! edit, and the probe answers the surfaced question "Yes" through the
//! driver's real `request_input` bridge. Success = the question surfaced,
//! the turn completed, AND the file exists — a rejected permission reply
//! would hang or error the turn instead.
//!
//!     cargo run -p zeron-harness --example opencode_permission_probe -- \
//!         ~/.opencode/bin/opencode opencode/muse-spark-1.3-contributor-free

use futures::StreamExt;
use tokio::sync::mpsc;
use zeron_harness::{CancellationToken, Harness, OpencodeHarness, RunControls};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel, UserInputAnswer};

#[tokio::main]
async fn main() {
    let exe = std::env::args().nth(1);
    let model = std::env::args()
        .nth(2)
        .filter(|m| m.contains('/'))
        .unwrap_or_else(|| "opencode/muse-spark-1.3-contributor-free".into());
    let workspace = tempfile::tempdir().expect("isolated probe workspace");
    // The ask rules: every edit/write must be approved. Ordered rules,
    // first match wins (packages/schema/src/permission.ts: Effect allow|deny|ask).
    std::fs::write(
        workspace.path().join("opencode.json"),
        r#"{
            "permissions": [
                { "action": "edit", "resource": "**", "effect": "ask" },
                { "action": "write", "resource": "**", "effect": "ask" }
            ]
        }"#,
    )
    .expect("write workspace opencode.json");
    let cwd = workspace
        .path()
        .to_str()
        .expect("UTF-8 workspace path")
        .to_owned();

    let (_steer_tx, steering) = mpsc::channel(8);
    let request = RunRequest {
        prompt: "Create a file named note.txt in the current directory containing exactly the \
                 text: hello-from-probe. Then reply with exactly: DONE"
            .into(),
        harness: None,
        model: Some(model),
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd,
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
    };
    let mut harness = OpencodeHarness::new();
    if let Some(exe) = exe {
        harness = harness.with_executable(exe);
    }
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let mut stream = harness
        .run(
            request,
            RunControls {
                // Answer every question "Yes" — the driver routes permission
                // asks through here when auto_approve is off.
                request_input: {
                    let asked = std::sync::Arc::clone(&asked);
                    Box::new(move |questions: Vec<zeron_proto::UserInputQuestion>| {
                        let n = asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                        eprintln!(
                            "ASK #{n}: {} question(s): {:?}",
                            questions.len(),
                            questions.iter().map(|q| &q.question).collect::<Vec<_>>()
                        );
                        let (tx, rx) = tokio::sync::oneshot::channel();
                        let answers = questions
                            .iter()
                            .map(|q| UserInputAnswer {
                                question_id: q.id.clone(),
                                labels: vec!["Yes".into()],
                            })
                            .collect();
                        let _ = tx.send(answers);
                        rx
                    })
                },
                steering,
                interrupt: CancellationToken::new(),
            },
        )
        .await
        .expect("run starts");
    let mut text = String::new();
    let status = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(180), stream.next()).await {
            Ok(Some(Ok(AgentEvent::Done { status, error, .. }))) => {
                if let Some(error) = error {
                    eprintln!("ERR {error}");
                }
                break Some(status);
            }
            Ok(Some(Ok(AgentEvent::TextDelta { text: delta }))) => {
                text.push_str(&delta);
            }
            Ok(Some(Ok(AgentEvent::InputRequested { request_id, .. }))) => {
                eprintln!("INPUT-REQUESTED {request_id}");
            }
            Ok(Some(Ok(other))) => {
                eprintln!("EV {other:?}");
            }
            Ok(Some(Err(e))) => eprintln!("ERR {e}"),
            Ok(None) => break None,
            Err(_) => {
                eprintln!("--- timed out");
                std::process::exit(2);
            }
        }
    };
    let file = workspace.path().join("note.txt");
    let file_content = std::fs::read_to_string(&file).ok();
    let asked = asked.load(std::sync::atomic::Ordering::SeqCst);
    eprintln!(
        "--- done: {status:?} text={text:?} asks={asked} file={:?}",
        file_content
    );
    let ok = matches!(status, Some(DoneStatus::Completed))
        && asked > 0
        && file_content.as_deref() == Some("hello-from-probe");
    if !ok {
        std::process::exit(1);
    }
}
