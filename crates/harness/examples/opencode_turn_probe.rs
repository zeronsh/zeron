//! Live probe: one full turn through the native driver against a chosen
//! opencode binary — prints the event stream and exits 0 only on a
//! Completed turn with text. Arg 1 = executable (default or `-`: PATH
//! resolution, on OpenCode's background service where it has one),
//! arg 2 = model (`provider/model`), arg 3 = prompt.
//! Optional env: OPENCODE_PROBE_WORKSPACE uses an existing configured directory,
//! OPENCODE_PROBE_AGENT selects an agent, OPENCODE_PROBE_INTERRUPT_MS cancels
//! after the first text delta, OPENCODE_PROBE_MCP (a command line) gives the
//! run a stdio MCP server named `probe`, as the engine gives it Zeron's. Set
//! TMPDIR to control temporary workspace placement.
//!
//!     cargo run -p zeron-harness --example opencode_turn_probe -- \
//!         ~/.opencode/bin/opencode opencode/muse-spark-1.3-contributor-free \
//!         "Reply with exactly: PONG"

use futures::StreamExt;
use tokio::sync::mpsc;
use zeron_harness::{CancellationToken, Harness, OpencodeHarness, RunControls};
use zeron_proto::{AgentEvent, RunRequest, SandboxLevel};

#[tokio::main]
async fn main() {
    // (`-` leaves the binary to resolution, and so the background service.)
    let exe = std::env::args()
        .nth(1)
        .filter(|exe| !exe.is_empty() && exe != "-");
    let model = std::env::args().nth(2).filter(|m| m.contains('/'));
    let prompt = std::env::args()
        .nth(3)
        .unwrap_or_else(|| "Reply with exactly: PONG".into());
    let workspace = tempfile::tempdir().expect("create isolated probe workspace");
    let cwd = std::env::var("OPENCODE_PROBE_WORKSPACE").unwrap_or_else(|_| {
        workspace
            .path()
            .to_str()
            .expect("UTF-8 workspace path")
            .into()
    });
    let mut model_options = serde_json::Map::new();
    if let Ok(agent) = std::env::var("OPENCODE_PROBE_AGENT") {
        model_options.insert("agent".into(), agent.into());
    }
    let mut interrupt_after = std::env::var("OPENCODE_PROBE_INTERRUPT_MS")
        .ok()
        .map(|s| s.parse::<u64>().expect("interrupt milliseconds"));
    let expect_interrupt = interrupt_after.is_some();
    let answer_yes = std::env::var_os("OPENCODE_PROBE_ANSWER_YES").is_some();
    let interrupt = CancellationToken::new();
    let (_steer_tx, steering) = mpsc::channel(8);
    let mcp = std::env::var("OPENCODE_PROBE_MCP").ok().and_then(|line| {
        let mut words = line.split_whitespace().map(str::to_owned);
        Some(zeron_proto::McpServer {
            name: "probe".into(),
            command: words.next()?,
            args: words.collect(),
            env: Default::default(),
        })
    });
    let request = RunRequest {
        mcp,
        prompt,
        harness: None,
        model,
        reasoning: None,
        model_options,
        cwd,
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: !answer_yes,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    };
    let mut harness = OpencodeHarness::new();
    if let Some(exe) = exe {
        harness = harness.with_executable(exe);
    }
    let mut stream = harness
        .run(
            request,
            RunControls {
                realtime: None,
                execution_lease: None,
                request_input: Box::new(move |questions| {
                    assert!(
                        answer_yes,
                        "set OPENCODE_PROBE_ANSWER_YES to answer permission prompts"
                    );
                    let answers = questions
                        .into_iter()
                        .map(|question| {
                            eprintln!("PERMISSION Yes (once): {}", question.question);
                            zeron_proto::UserInputAnswer {
                                question_id: question.id,
                                labels: vec!["Yes".into()],
                            }
                        })
                        .collect();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    let _ = tx.send(answers);
                    rx
                }),
                steering,
                interrupt: interrupt.clone(),
                turn: Default::default(),
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
                if let Some(ms) = interrupt_after.take() {
                    let interrupt = interrupt.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                        interrupt.cancel();
                    });
                }
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
        Some(zeron_proto::DoneStatus::Completed)
            if !expect_interrupt && !text.trim().is_empty() => {}
        Some(zeron_proto::DoneStatus::Interrupted) if expect_interrupt => {}
        _ => std::process::exit(1),
    }
}
