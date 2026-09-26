//! Real-model conversational steering check (no tools or additive jobs).
//! cargo run -p zeron-harness --example cursor_steering_probe -- gemini-3-flash
use futures::StreamExt;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, CursorHarness, Harness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model = std::env::args()
        .nth(1)
        .expect("select an inexpensive model");
    let workspace = tempfile::tempdir()?;
    let (steer, steering) = mpsc::channel(64);
    let interrupt = CancellationToken::new();
    let request = RunRequest {
        mcp: None,
        prompt: "This is a conversational steering test. Start writing 200 numbered lines explaining how rain forms, with a full sentence on each line. Do not use tools. If I send a bare digit while you write, that REPLACES the prior request: stop explaining and reply only LATEST:<digit>, using the latest digit received. Do not respond separately to superseded digits.".into(),
        harness: None, model: Some(model), reasoning: None,
        model_options: Default::default(), cwd: workspace.path().to_string_lossy().into_owned(),
        sandbox: SandboxLevel::DangerFullAccess, auto_approve: true,
        attachments: vec![], worktree: None, resume: None,
    };
    let mut stream = CursorHarness::new()
        .run(
            request,
            RunControls {
                steering,
                interrupt: interrupt.clone(),
                request_input: Box::new(|_| {
                    let (tx, rx) = oneshot::channel();
                    let _ = tx.send(vec![]);
                    rx
                }),
            },
        )
        .await?;
    let mut injected = false;
    let mut text = String::new();
    let mut acknowledgments = 0;
    let mut turns = 0;
    let result = tokio::time::timeout(Duration::from_secs(150), async {
        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::TextDelta { text: delta } => {
                    print!("{delta}");
                    text.push_str(&delta);
                    if !injected {
                        injected = true;
                        let sender = steer.clone();
                        tokio::spawn(async move {
                            for i in 1..=6 {
                                if sender
                                    .send(SteerMessage {
                                        prompt: i.to_string(),
                                        message_id: Some(format!("digit-{i}")),
                                    })
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(75)).await;
                            }
                        });
                    }
                }
                AgentEvent::Steered { .. } => acknowledgments += 1,
                AgentEvent::Done { status, error, .. } => {
                    anyhow::ensure!(status == DoneStatus::Completed, "{status:?}: {error:?}");
                    turns += 1;
                    if acknowledgments == 6 {
                        break;
                    }
                }
                AgentEvent::Error { message } => eprintln!("{message}"),
                _ => {}
            }
        }
        anyhow::ensure!(
            acknowledgments == 6 && turns == 1,
            "{acknowledgments} acknowledgments, {turns} turns"
        );
        anyhow::ensure!(
            text.contains("LATEST:6"),
            "latest instruction was not followed"
        );
        for i in 1..6 {
            anyhow::ensure!(
                !text.contains(&format!("LATEST:{i}")),
                "responded to superseded digit {i}"
            );
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    interrupt.cancel();
    drop(steer);
    drop(stream);
    result??;
    println!(
        "\nPASS: six rapid corrections produced only the latest answer within the active turn"
    );
    Ok(())
}
