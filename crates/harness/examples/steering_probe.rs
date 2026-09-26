//! Opt-in live steering regression. Uses real model quota in a disposable cwd.
//! cargo run -p zeron-harness --example steering_probe -- <harness> [model|--models]
//! Checks a running shell survives a rapid burst and every follow-up is acted on.
use futures::StreamExt;
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{
    AcpHarness, CancellationToken, ClaudeHarness, CodexHarness, CursorHarness, Harness,
    OpencodeHarness, RunControls, SteerMessage,
};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let name = std::env::args().nth(1).expect("harness name");
    let model = std::env::args().nth(2);
    let require_mid_turn = std::env::args().any(|arg| arg == "--require-mid-turn");
    let harness: Arc<dyn Harness> = match name.as_str() {
        "claude" => Arc::new(ClaudeHarness::new()),
        "codex" => Arc::new(CodexHarness::new()),
        "cursor" => Arc::new(CursorHarness::new()),
        "opencode" => Arc::new(OpencodeHarness::new()),
        "grok" => Arc::new(AcpHarness::grok()),
        "devin" => Arc::new(AcpHarness::devin()),
        "hermes" => Arc::new(AcpHarness::hermes()),
        "pi" => Arc::new(AcpHarness::pi()),
        "antigravity" => Arc::new(AcpHarness::antigravity()),
        _ => anyhow::bail!("unknown harness"),
    };
    if model.as_deref() == Some("--models") {
        let models = tokio::time::timeout(Duration::from_secs(45), harness.models()).await??;
        for model in models {
            println!("{} {}", model.id, model.label);
        }
        return Ok(());
    }
    let workspace = tempfile::tempdir()?;
    let token = uuid::Uuid::new_v4().to_string();
    let prompt = format!(
        "This is a small steering reliability test. Remember secret {token}. Run exactly this shell command in the current directory: `printf started > started; sleep 8; printf survived > survivor`. Let it finish, then reply INITIAL-DONE. Later messages are additive: do not cancel this command or any earlier request, and execute every follow-up exactly once."
    );
    let (steer, steering) = mpsc::channel(64);
    let interrupt = CancellationToken::new();
    let request = RunRequest {
        mcp: None,
        prompt,
        harness: None,
        model,
        reasoning: None,
        model_options: Default::default(),
        cwd: workspace.path().to_string_lossy().into_owned(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: vec![],
        worktree: None,
        resume: None,
    };
    let controls = RunControls {
        steering,
        interrupt: interrupt.clone(),
        request_input: Box::new(|_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
    };
    let mut stream =
        tokio::time::timeout(Duration::from_secs(90), harness.run(request, controls)).await??;
    let mut injected = false;
    let mut done = 0;
    let mut boundaries = 0;
    let mut ticker = tokio::time::interval(Duration::from_millis(25));
    let result = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            tokio::select! {
                _ = ticker.tick(), if !injected => {
                    if workspace.path().join("started").exists() {
                        for index in 0..3 {
                            steer.send(SteerMessage { prompt: format!("Additive follow-up {index}: keep all earlier work running. Write ONLY the secret from the first user message into file followup-{index}. Do not read it from another file. Use a shell command to append {index} as one line to receipts. Reply FOLLOWUP-{index}-DONE."), message_id: Some(format!("probe-{index}")) }).await?;
                        }
                        injected = true;
                        println!("{name}: injected 3 steers while shell was running");
                    }
                }
                event = stream.next() => {
                    let event = event.ok_or_else(|| anyhow::anyhow!("stream ended before all messages completed"))??;
                    match event {
                        AgentEvent::Steered { .. } => boundaries += 1,
                        AgentEvent::Done { status, error, .. } => {
                            anyhow::ensure!(status == DoneStatus::Completed, "turn failed: {status:?} {error:?}");
                            done += 1;
                            if require_mid_turn && done == 1 {
                                anyhow::ensure!(injected && boundaries == 3 && (0..3).all(|i| workspace.path().join(format!("followup-{i}")).exists()), "steers were queued instead of handled by the active turn ({boundaries} confirmations)");
                            }
                            if injected && (0..3).all(|i| workspace.path().join(format!("followup-{i}")).exists()) { break; }
                        }
                        AgentEvent::Error { message } => eprintln!("{name}: {message}"),
                        _ => {}
                    }
                }
            }
        }
        anyhow::ensure!(std::fs::read_to_string(workspace.path().join("survivor"))? == "survived", "original child did not survive");
        for i in 0..3 { anyhow::ensure!(std::fs::read_to_string(workspace.path().join(format!("followup-{i}")))?.trim() == token, "follow-up {i} lost original context"); }
        let receipts = std::fs::read_to_string(workspace.path().join("receipts"))?;
        let mut receipts: Vec<_> = receipts.lines().collect(); receipts.sort();
        anyhow::ensure!(receipts == ["0", "1", "2"], "follow-ups missing or duplicated: {receipts:?}");
        Ok::<_, anyhow::Error>(())
    }).await;
    interrupt.cancel();
    drop(steer);
    drop(stream);
    result??;
    println!(
        "PASS {name}: child survived; all 3 messages retained context and ran once; {done} completions, {boundaries} steer boundaries"
    );
    Ok(())
}
