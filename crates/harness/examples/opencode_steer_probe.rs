//! Live probe: a mid-turn steer through the native driver against a chosen
//! opencode binary. Starts a deliberately long turn, pushes a steer into the
//! steering mailbox once text is streaming, and timestamps every event so
//! the capture shows whether the steer entered the RUNNING turn (Steered
//! between text deltas, no idle gap) or fell back to the turn boundary.
//! Success = a Steered event, the turn completing, and the steer's effect
//! visible in the final text.
//!
//!     cargo run -p zeron-harness --example opencode_steer_probe -- \
//!         ~/.opencode/bin/opencode opencode/muse-spark-1.3-contributor-free

use futures::StreamExt;
use tokio::sync::mpsc;
use zeron_harness::{CancellationToken, Harness, OpencodeHarness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

#[tokio::main]
async fn main() {
    let exe = std::env::args().nth(1);
    let model = std::env::args()
        .nth(2)
        .filter(|m| m.contains('/'))
        .unwrap_or_else(|| "opencode/muse-spark-1.3-contributor-free".into());
    let workspace = tempfile::tempdir().expect("isolated probe workspace");
    let cwd = workspace
        .path()
        .to_str()
        .expect("UTF-8 workspace path")
        .to_owned();
    let started = std::time::Instant::now();
    let stamp = move || format!("[{:>6}ms]", started.elapsed().as_millis());

    let (steer_tx, steering) = mpsc::channel(4);
    let prompt = "Count slowly from 1 to 25, one number per line, thinking \
                  briefly between numbers. After 25, write FINAL-LINE.";
    let request = RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: Some(model),
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd,
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
    };
    let mut harness = OpencodeHarness::new();
    if let Some(exe) = exe {
        harness = harness.with_executable(exe);
    }
    let mut stream = harness
        .run(
            request,
            RunControls {
                request_input: Box::new(|_| panic!("probe must not ask for input")),
                steering,
                interrupt: CancellationToken::new(),
            },
        )
        .await
        .expect("run starts");

    // Steer 2.5s in: the counting turn is mid-stream by then. Whether the
    // steer entered the RUNNING turn is read off the timestamps (TXT lines
    // both sides of STEERED, no Done between).
    let steer_sender = steer_tx.clone();
    let steer_text = "Stop counting. Reply with exactly: STEERED-WINS";
    let text_to_send = steer_text.to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        let _ = steer_sender
            .send(SteerMessage {
                prompt: text_to_send,
                message_id: Some("probe-steer-1".into()),
            })
            .await;
    });
    drop(steer_tx);

    let mut text = String::new();
    let mut steered_at: Option<u128> = None;
    let status = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(180), stream.next()).await {
            Ok(Some(Ok(AgentEvent::Done { status, error, .. }))) => {
                if let Some(error) = error {
                    eprintln!("{} ERR {error}", stamp());
                }
                break Some(status);
            }
            Ok(Some(Ok(AgentEvent::TextDelta { text: delta }))) => {
                text.push_str(&delta);
                eprintln!("{} TXT {delta}", stamp());
            }
            Ok(Some(Ok(AgentEvent::Steered { .. }))) => {
                steered_at = Some(started.elapsed().as_millis());
                eprintln!("{} STEERED", stamp());
            }
            Ok(Some(Ok(AgentEvent::ToolCall { call, .. }))) => {
                eprintln!("{} TOOL {call:?}", stamp());
            }
            Ok(Some(Ok(other))) => eprintln!("{} EV {other:?}", stamp()),
            Ok(Some(Err(e))) => eprintln!("{} ERR {e}", stamp()),
            Ok(None) => break None,
            Err(_) => {
                eprintln!("{} --- timed out", stamp());
                std::process::exit(2);
            }
        }
    };
    eprintln!(
        "--- done: {status:?} steered_at={steered_at:?} text_tail={:?}",
        text.chars().rev().take(80).collect::<String>()
    );
    let ok = matches!(status, Some(DoneStatus::Completed))
        && steered_at.is_some()
        && text.contains("STEERED-WINS");
    if !ok {
        std::process::exit(1);
    }
}
