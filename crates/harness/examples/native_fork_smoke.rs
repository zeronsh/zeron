//! Opt-in live acceptance probe; uses an isolated project and consumes inference.
//! cargo run -p zeron-harness --example native_fork_smoke -- codex [model]
use futures::StreamExt;
use std::{sync::Arc, time::Duration};
use zeron_harness::{
    CancellationToken, ClaudeHarness, CodexHarness, Harness, NativeForkControls, OpencodeHarness,
    RunControls,
};
use zeron_proto::*;

async fn turn(
    h: &dyn Harness,
    cwd: &str,
    model: Option<String>,
    resume: Option<String>,
    prompt: &str,
    strict: bool,
) -> anyhow::Result<(String, Option<NativeForkPoint>, String)> {
    let (tx, steering) = tokio::sync::mpsc::channel(1);
    drop(tx);
    let interrupt = CancellationToken::new();
    let request: RunRequest = serde_json::from_value(serde_json::json!({
        "prompt":prompt, "cwd":cwd, "model":model, "resume":resume,
        "sandbox":"workspace-write", "autoApprove":true,
        "resumePolicy":if strict {"requireExisting"} else {"allowFresh"}
    }))?;
    let mut stream = h
        .run(
            request,
            RunControls {
                execution_lease: None,
                interrupt: interrupt.clone(),
                steering,
                request_input: Box::new(|_| {
                    let (_tx, rx) = tokio::sync::oneshot::channel();
                    rx
                }),
            },
        )
        .await?;
    let mut point = None;
    let mut text = String::new();
    let outcome = tokio::time::timeout(Duration::from_secs(180), async {
        while let Some(event) = stream.next().await {
            match event? {
                AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                AgentEvent::NativeForkReady { point: p, .. } => point = Some(p),
                AgentEvent::Done {
                    status,
                    session_id,
                    error,
                    ..
                } => {
                    anyhow::ensure!(
                        status == DoneStatus::Completed,
                        "Provider failed: {error:?}"
                    );
                    return Ok((
                        session_id.ok_or_else(|| anyhow::anyhow!("Missing session identity"))?,
                        point,
                        text,
                    ));
                }
                _ => {}
            }
        }
        anyhow::bail!("Stream closed without Done")
    })
    .await;
    interrupt.cancel();
    drop(stream);
    tokio::time::sleep(Duration::from_millis(700)).await;
    outcome.map_err(|_| anyhow::anyhow!("Turn timed out"))?
}
fn harness(provider: &str) -> Arc<dyn Harness> {
    match provider {
        "codex" => Arc::new(CodexHarness::new()),
        "claude" => Arc::new(ClaudeHarness::new()),
        "opencode" => Arc::new(OpencodeHarness::new()),
        _ => panic!("Choose codex, claude, or opencode"),
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = std::env::args().nth(1).expect("provider");
    let model = std::env::args().nth(2);
    anyhow::ensure!(
        provider != "opencode" || model.as_ref().is_some_and(|m| m.contains("free")),
        "OpenCode smoke tests require an explicit model containing free"
    );
    let project = tempfile::tempdir()?;
    let cwd = project.path().to_str().unwrap();
    // No repository tools are requested; these turns only exercise memory.
    let h = harness(&provider);
    let support = h.native_fork_support(project.path()).await;
    anyhow::ensure!(support.available, "Unavailable: {:?}", support.reason);
    let (parent, point, _) = turn(
        h.as_ref(),
        cwd,
        model.clone(),
        None,
        "Remember the fictional fruit label PINEAPPLE-734 for this memory test. This is public test data. Reply only ACK. Do not use tools.",
        false,
    )
    .await?;
    let point = point.ok_or_else(|| anyhow::anyhow!("No native point for completed turn"))?;
    let (_, _, _) = turn(
        h.as_ref(),
        cwd,
        model.clone(),
        Some(parent.clone()),
        "Also remember the fictional color label VIOLET-982. This is public test data. Reply only ACK. Do not use tools.",
        false,
    )
    .await?;
    if let Ok(path) = std::env::var("NATIVE_FORK_SMOKE_POINT") {
        std::fs::write(path, serde_json::to_vec_pretty(&point)?)?;
    }
    let child = h
        .fork_native(
            &point,
            NativeForkControls {
                execution_lease: None,
                interrupt: CancellationToken::new(),
                timeout: Duration::from_secs(90),
                source_idle: true,
            },
        )
        .await?;
    if let Ok(path) = std::env::var("NATIVE_FORK_SMOKE_POINT") {
        std::fs::write(
            format!("{path}.child.json"),
            serde_json::to_vec_pretty(&child)?,
        )?;
    }
    anyhow::ensure!(child.session_id != parent, "Fork reused parent identity");
    let question = "List the fictional labels mentioned earlier in THIS conversation, verbatim. These are public test data. Do not use tools. If only one label was mentioned, list only that one.";
    let (_, own_point, answer) = turn(
        h.as_ref(),
        cwd,
        model.clone(),
        Some(child.session_id.clone()),
        question,
        true,
    )
    .await?;
    anyhow::ensure!(
        answer.contains("PINEAPPLE-734") && !answer.contains("VIOLET-982"),
        "Child inherited the wrong context: {answer}"
    );
    anyhow::ensure!(
        own_point.is_some_and(|p| p.source_session_id == child.session_id),
        "Child did not emit its own native point"
    );
    drop(h);
    let resumed = harness(&provider);
    let (_, _, answer) = turn(
        resumed.as_ref(),
        cwd,
        model,
        Some(child.session_id),
        question,
        true,
    )
    .await?;
    anyhow::ensure!(
        answer.contains("PINEAPPLE-734") && !answer.contains("VIOLET-982"),
        "Restart lost native context: {answer}"
    );
    println!(
        "PASS {provider}: historical prefix, child point, strict resume, and fresh-driver restart"
    );
    Ok(())
}
