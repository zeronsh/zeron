//! Opt-in live test. Uses real Cursor quota in a disposable workspace.
//! ZERON_CURSOR_STATE_DIR=$(mktemp -d) cargo run -p zeron-harness --example cursor_stability_probe -- sessions 20
//! cargo run -p zeron-harness --example cursor_stability_probe -- models 1000
use futures::StreamExt;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{CancellationToken, CursorHarness, Harness, RunControls};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

async fn turn(
    harness: &CursorHarness,
    cwd: &str,
    session: Option<String>,
    prompt: String,
    fault: &str,
) -> (String, String) {
    let (tx, steering) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        steering,
        interrupt: token.clone(),
        request_input: Box::new(|_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
    };
    let request = RunRequest {
        prompt,
        harness: None,
        model: Some("composer-2.5".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.into(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: vec![],
        worktree: None,
        resume: session.clone(),
    };
    let mut stream = harness
        .run(request, controls)
        .await
        .expect("start real Cursor run");
    let mut id = session.unwrap_or_default();
    let mut text = String::new();
    let mut injected = false;
    let effect_file = PathBuf::from(cwd).join("side-effects.txt");
    let prior_effects = std::fs::read_to_string(&effect_file)
        .unwrap_or_default()
        .lines()
        .count();
    let began = Instant::now();
    tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(event) = stream.next().await {
            match event.expect("event") {
                AgentEvent::SessionStarted { session_id, .. } => id = session_id,
                AgentEvent::TextDelta { text: chunk } => text.push_str(&chunk),
                AgentEvent::ToolCall { .. } if !fault.is_empty() && !injected => {
                    // Kill/drop only AFTER the requested side effect happened.
                    // A later resume must not replay this already-executed command.
                    tokio::time::timeout(Duration::from_secs(10), async {
                        while std::fs::read_to_string(&effect_file)
                            .unwrap_or_default()
                            .lines()
                            .count()
                            <= prior_effects
                        {
                            tokio::time::sleep(Duration::from_millis(25)).await;
                        }
                    })
                    .await
                    .expect("side effect before injected fault");
                    injected = true;
                    match fault {
                        "interrupt" => token.cancel(),
                        "drop" => break,
                        "kill" => {
                            let root = PathBuf::from(
                                std::env::var_os("ZERON_CURSOR_STATE_DIR")
                                    .expect("isolated state root required"),
                            );
                            let dir =
                                std::fs::read_to_string(root.join("by-agent").join(&id)).unwrap();
                            let owner: serde_json::Value = serde_json::from_slice(
                                &std::fs::read(PathBuf::from(dir.trim()).join(".zeron-owner.json"))
                                    .unwrap(),
                            )
                            .unwrap();
                            assert!(
                                tokio::process::Command::new("kill")
                                    .arg("-KILL")
                                    .arg(owner["pid"].as_u64().unwrap().to_string())
                                    .status()
                                    .await
                                    .unwrap()
                                    .success()
                            );
                        }
                        _ => panic!("unknown fault"),
                    }
                }
                AgentEvent::Done { status, error, .. } => {
                    if fault.is_empty() {
                        assert_eq!(status, DoneStatus::Completed, "{error:?}");
                    } else if fault == "interrupt" {
                        assert_eq!(status, DoneStatus::Interrupted);
                    } else if fault == "kill" {
                        assert_eq!(status, DoneStatus::Errored);
                    }
                    println!(
                        "turn fault={fault:?} status={status:?} ms={}",
                        began.elapsed().as_millis()
                    );
                    break;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("bounded real turn");
    assert!(!id.is_empty());
    assert!(fault.is_empty() || injected, "fault was not exercised");
    drop(stream);
    drop(tx);
    (id, text)
}

async fn parked(harness: &CursorHarness, count: usize) {
    let workspace = tempfile::tempdir().unwrap();
    let nonce = format!("PARKED-STABILITY-{}", uuid::Uuid::new_v4());
    let (tx, steering) = mpsc::channel(8);
    let controls = RunControls {
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
    };
    let request = RunRequest {
        prompt: format!(
            "Remember this exact token: {nonce}. Reply only that token. Do not use tools or files."
        ),
        harness: None,
        model: Some("composer-2.5".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: workspace.path().to_str().unwrap().into(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: vec![],
        worktree: None,
        resume: None,
    };
    let mut stream = harness.run(request, controls).await.unwrap();
    let mut id = String::new();
    let mut text = String::new();
    let mut completed = 0;
    tokio::time::timeout(Duration::from_secs(300),async {
        while let Some(event)=stream.next().await {
            match event.unwrap() {
                AgentEvent::SessionStarted{session_id,..}=>id=session_id,
                AgentEvent::TextDelta{text:chunk}=>text.push_str(&chunk),
                AgentEvent::Done{status,error,..}=>{
                    assert_eq!(status,DoneStatus::Completed,"{error:?}");
                    assert!(text.contains(&nonce),"lost parked context: {text}");
                    text.clear(); completed+=1;
                    println!("parked_turn={completed} checkpoint_recall=true");
                    if completed==count {break;}
                    tx.send(zeron_harness::SteerMessage{prompt:"Repeat the exact PARKED-STABILITY token from earlier. Reply only the token. Do not use tools or files.".into(),message_id:None}).await.unwrap();
                }
                _=>{}
            }
        }
    }).await.unwrap();
    assert_eq!(completed, count);
    drop(stream);
    drop(tx);
    let (_, text) = turn(
        harness,
        workspace.path().to_str().unwrap(),
        Some(id),
        "Repeat the exact PARKED-STABILITY token from earlier. Reply only the token.".into(),
        "",
    )
    .await;
    assert!(text.contains(&nonce));
    println!(
        "{}",
        serde_json::json!({"parkedTurns":completed,"resumedAfterClose":true,"failures":0})
    );
}

// Queue faster than the SDK can complete turns, including channel backpressure.
async fn burst(harness: &CursorHarness, count: usize, cancel: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let nonce = format!("BURST-{}", uuid::Uuid::new_v4());
    // Establish a completed checkpoint before testing cancellation of a later
    // turn. An interrupted first turn may not have a provider checkpoint yet.
    let seed = if cancel {
        Some(
            turn(
                harness,
                workspace.path().to_str().unwrap(),
                None,
                format!("Remember token {nonce}. Reply only {nonce}. Do not use tools."),
                "",
            )
            .await
            .0,
        )
    } else {
        None
    };
    let (tx, steering) = mpsc::channel(8);
    let token = CancellationToken::new();
    let controls = RunControls {
        steering,
        interrupt: token.clone(),
        request_input: Box::new(|_| {
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(vec![]);
            rx
        }),
    };
    let request = RunRequest {
        prompt: if cancel {
            format!(
                "Remember token {nonce}. First run shell command `sleep 30`, then reply only {nonce}."
            )
        } else {
            format!("Remember token {nonce}. Reply only {nonce} and INITIAL. Do not use tools.")
        },
        harness: None,
        model: Some("composer-2.5".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: workspace.path().to_str().unwrap().into(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: vec![],
        worktree: None,
        resume: seed,
    };
    let mut stream = harness.run(request, controls).await.unwrap();
    let producer = tokio::spawn(async move {
        for n in 0..count {
            let prompt = if cancel {
                "Append one line to forbidden-replay.txt using a shell command.".into()
            } else {
                format!(
                    "Reply only the BURST token from the initial prompt and marker ITEM-{n:04}. Do not use tools."
                )
            };
            if tx
                .send(zeron_harness::SteerMessage {
                    prompt,
                    message_id: None,
                })
                .await
                .is_err()
            {
                return n;
            }
        }
        count
    });
    let mut id = String::new();
    let mut text = String::new();
    let mut completed = 0;
    let mut steered = 0;
    let mut cancelled = 0;
    tokio::time::timeout(Duration::from_secs(600), async {
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                AgentEvent::SessionStarted { session_id, .. } => id = session_id,
                AgentEvent::TextDelta { text: chunk } => text.push_str(&chunk),
                AgentEvent::Steered { .. } => steered += 1,
                AgentEvent::ToolCall { .. } if cancel => {
                    // Let the producer fill the harness queue before cancelling.
                    while !producer.is_finished() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    token.cancel();
                }
                AgentEvent::Done { status, error, .. } => {
                    if cancel {
                        assert_eq!(status, DoneStatus::Interrupted, "{error:?}");
                        cancelled += 1;
                    } else {
                        assert_eq!(status, DoneStatus::Completed, "{error:?}");
                        assert!(text.contains(&nonce), "lost context: {text}");
                        let marker = if completed == 0 {
                            "INITIAL".into()
                        } else {
                            format!("ITEM-{:04}", completed - 1)
                        };
                        assert!(
                            text.contains(&marker),
                            "out of order/missing turn: expected {marker}, got {text}"
                        );
                        completed += 1;
                        text.clear();
                        println!("burst_completed={completed} ordered=true context=true");
                    }
                }
                _ => {}
            }
        }
    })
    .await
    .expect("burst must settle and close");
    assert_eq!(producer.await.unwrap(), count, "every steer was enqueued");
    if cancel {
        assert_eq!(cancelled, 1);
        assert_eq!(steered, 0);
    } else {
        assert_eq!(completed, count + 1);
        assert_eq!(steered, count);
    }
    assert!(!id.is_empty());
    let (resumed, text) = turn(
        harness,
        workspace.path().to_str().unwrap(),
        Some(id.clone()),
        "Repeat the exact BURST token from the first prompt. Do not use tools.".into(),
        "",
    )
    .await;
    assert_eq!(resumed, id);
    assert!(text.contains(&nonce), "resume lost context: {text}");
    assert!(
        !workspace.path().join("forbidden-replay.txt").exists(),
        "cancelled queued prompt executed"
    );
    println!(
        "{}",
        serde_json::json!({"queued":count,"completed":completed,"steered":steered,"interrupted":cancelled,"resumed":true,"failures":0})
    );
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mode = args.get(1).map(String::as_str).unwrap_or("models");
    let count: usize = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(1000);
    let harness = CursorHarness::new();
    if mode == "resolve" {
        let (exe, args) = harness.resolve_shim().await.unwrap();
        println!("{}", serde_json::json!({"exe":exe,"args":args}));
    } else if mode == "models" || mode == "outage" {
        let start = Instant::now();
        let baseline = harness.models().await.expect("live models");
        assert!(
            baseline.len() > 2,
            "a two-row fallback is not a live stress baseline"
        );
        let outage = if mode == "outage" {
            let flag = PathBuf::from(
                std::env::var_os("ZERON_CURSOR_STRESS_OUTAGE_FLAG")
                    .expect("set a disposable fault-injection flag path"),
            );
            std::fs::write(&flag, "rate-limit").unwrap();
            println!("warm_catalog={} waiting_for_refresh=true", baseline.len());
            tokio::time::sleep(Duration::from_secs(61)).await;
            Some(flag)
        } else {
            None
        };
        let results = futures::future::join_all((0..count).map(|_| harness.models())).await;
        for result in results {
            assert_eq!(result.unwrap(), baseline);
        }
        if let Some(flag) = outage {
            std::fs::remove_file(flag).unwrap();
            println!("outage_requests={count} retained_catalog=true waiting_for_recovery=true");
            tokio::time::sleep(Duration::from_secs(61)).await;
            assert_eq!(harness.models().await.unwrap(), baseline);
            println!("recovery_live_refresh=true");
        }
        println!(
            "{}",
            serde_json::json!({"requests": count+1+usize::from(mode == "outage"), "models":baseline.len(), "elapsedMs":start.elapsed().as_millis(), "failures":0})
        );
    } else if mode == "burst" || mode == "cancel-burst" {
        assert!(std::env::var_os("ZERON_CURSOR_STATE_DIR").is_some());
        burst(&harness, count, mode == "cancel-burst").await;
    } else if mode == "parked" {
        assert!(std::env::var_os("ZERON_CURSOR_STATE_DIR").is_some());
        parked(&harness, count).await;
    } else if mode == "sessions" {
        assert!(
            std::env::var_os("ZERON_CURSOR_STATE_DIR").is_some(),
            "set an isolated ZERON_CURSOR_STATE_DIR"
        );
        let workspace = tempfile::tempdir().unwrap();
        let cwd = workspace.path().to_str().unwrap();
        let nonce = format!("CURSOR-STABILITY-{}", uuid::Uuid::new_v4());
        let (mut id, _) = turn(&harness, cwd, None, format!("Remember this exact token for this conversation: {nonce}. Reply only OK. Do not use tools or write files."), "").await;
        let mut expected_effects = 0;
        for round in 0..count {
            let fault = match round % 6 {
                1 => "kill",
                3 => "interrupt",
                5 => "drop",
                _ => "",
            };
            if !fault.is_empty() {
                expected_effects += 1;
                (id, _) = turn(
                    &harness,
                    cwd,
                    Some(id),
                    "Use the shell tool to run exactly: printf 'once\\n' >> side-effects.txt; sleep 30\nThen reply DONE. Do not do anything else."
                        .into(),
                    fault,
                )
                .await;
            }
            let (next, text) = turn(&harness, cwd, Some(id.clone()), "What exact CURSOR-STABILITY token did I ask you to remember? Reply only that token. Do not use tools or files.".into(), "").await;
            assert_eq!(id, next, "recovery must retain the original session");
            assert!(
                text.contains(&nonce),
                "conversation checkpoint lost at round {round}: {text}"
            );
            id = next;
            let effects = std::fs::read_to_string(workspace.path().join("side-effects.txt"))
                .unwrap_or_default()
                .lines()
                .count();
            assert_eq!(
                effects, expected_effects,
                "interrupted tool side effect must not replay"
            );
            println!(
                "checkpoint_recall round={} fault={fault:?} passed=true",
                round + 1
            );
        }
        println!(
            "{}",
            serde_json::json!({"rounds":count,"checkpointRecallFailures":0,"sessionId":id})
        );
    } else {
        panic!("use models, sessions, or resolve");
    }
}
