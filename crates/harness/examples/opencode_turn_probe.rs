//! Live probe: one full turn through the native driver against a chosen
//! opencode binary — prints the event stream and exits 0 only on a
//! Completed turn with text. Arg 1 = executable (default: PATH resolution),
//! arg 2 = model (`provider/model`), arg 3 = prompt.
//!
//!     cargo run -p zeron-harness --example opencode_turn_probe -- \
//!         ~/.opencode/bin/opencode opencode/muse-spark-1.3-contributor-free \
//!         "Read /tmp/note.txt and reply with its secret word."

use futures::StreamExt;
use tokio::sync::mpsc;
use zeron_harness::{CancellationToken, Harness, OpencodeHarness, RunControls};
use zeron_proto::{AgentEvent, RunRequest, SandboxLevel};

#[tokio::main]
async fn main() {
    let exe = std::env::args().nth(1);
    let model = std::env::args().nth(2).filter(|m| m.contains('/'));
    let prompt = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "Reply with exactly: PONG".into());
    let workspace = tempfile::tempdir().expect("create isolated probe workspace");
    let cwd = workspace.path().to_str().expect("UTF-8 workspace path");
    let (_steer_tx, steering) = mpsc::channel(8);
    let request = RunRequest {
        prompt,
        harness: None,
        model,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: cwd.into(),
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
    let mut text = String::new();
    let mut tools = 0u32;
    let status = loop {
        let ev =
            match tokio::time::timeout(std::time::Duration::from_secs(120), stream.next()).await {
                Ok(Some(ev)) => ev,
                Ok(None) => break None,
                Err(_) => {
                    eprintln!("--- timed out");
                    std::process::exit(2);
                }
            };
        match ev {
            Ok(AgentEvent::Done { status, error, .. }) => {
                if let Some(error) = error {
                    eprintln!("ERR {error}");
                }
                break Some(status);
            }
            Ok(AgentEvent::TextDelta { text: delta }) => {
                text.push_str(&delta);
                eprintln!("TXT {delta}");
            }
            Ok(AgentEvent::ReasoningDelta { text }) => eprintln!("THK {}", text.trim_end()),
            Ok(AgentEvent::ToolCall { .. }) => {
                tools += 1;
                eprintln!("TOOL");
            }
            Ok(AgentEvent::Usage {
                input_tokens,
                output_tokens,
            }) => {
                eprintln!("USAGE {input_tokens}/{output_tokens}");
            }
            Ok(AgentEvent::Error { message }) => eprintln!("CHIP {message}"),
            Ok(other) => eprintln!("EV {other:?}"),
            Err(e) => eprintln!("ERR {e}"),
        }
    };
    eprintln!("--- done: {status:?} text={text:?} tools={tools}");
    match status {
        Some(zeron_proto::DoneStatus::Completed) if !text.trim().is_empty() => {}
        _ => std::process::exit(1),
    }
}
