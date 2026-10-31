use futures::StreamExt;
use zeron_harness::{AcpHarness, CancellationToken, Harness, RunControls};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let harness = AcpHarness::droid();
    for attempt in 1..=2 {
        let start = std::time::Instant::now();
        let models = harness.models().await?;
        println!(
            "Discovery {attempt}: {} models in {:?}",
            models.len(),
            start.elapsed()
        );
        if attempt == 1 {
            for model in &models {
                println!(
                    "  {} - {} {:?}",
                    model.id, model.label, model.reasoning_levels
                );
            }
        }
    }
    let Some(model) = std::env::args().nth(1) else {
        return Ok(());
    };
    let (_steering, steering) = tokio::sync::mpsc::channel(8);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(|_| {
            let (tx, rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(Vec::new());
            rx
        }),
        steering,
        interrupt: CancellationToken::new(),
        turn: Default::default(),
    };
    let request = RunRequest {
        mcp: None,
        prompt: "Reply with exactly: Droid ACP verified. Do not use tools.".into(),
        harness: None,
        model: Some(model.clone()),
        reasoning: None,
        model_options: Default::default(),
        cwd: std::env::current_dir()?.to_string_lossy().into_owned(),
        sandbox: SandboxLevel::ReadOnly,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
    };
    println!("Running real Droid ACP with {model}");
    let mut stream = harness.run(request, controls).await?;
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::TextDelta { text } => print!("{text}"),
                AgentEvent::Done { status, error, .. } => {
                    println!("\nDone: {status:?}");
                    anyhow::ensure!(status == DoneStatus::Completed, "{error:?}");
                    return Ok(());
                }
                _ => {}
            }
        }
        anyhow::bail!("stream closed without Done")
    })
    .await?
}
