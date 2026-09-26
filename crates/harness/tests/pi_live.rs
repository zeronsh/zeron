//! Opt-in real Pi lifecycle probe. Run with an isolated adapter wrapper and a
//! local mock provider; PI_LIVE_DIR contains the wrapper's adapter.pid file.
#![cfg(unix)]

use futures::StreamExt;
use std::{path::PathBuf, time::Duration};
use tokio::sync::{mpsc, oneshot};
use zeron_harness::{AcpHarness, CancellationToken, Harness, RunControls, SteerMessage};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

#[tokio::test]
#[ignore = "requires an isolated real Pi adapter and local mock provider"]
async fn real_pi_mock_lifecycle() {
    let cwd = PathBuf::from(std::env::var_os("PI_LIVE_DIR").expect("isolated Pi cwd"));
    let harness = AcpHarness::pi();
    let mut session = None;
    for scenario in ["boundary", "idle-kill", "resume", "interrupt", "mid-kill"] {
        let _ = std::fs::remove_file(cwd.join("tool.pid"));
        let (steer, steering) = mpsc::channel(8);
        let token = CancellationToken::new();
        let controls = RunControls {
            execution_lease: None,
            steering,
            interrupt: token.clone(),
            request_input: Box::new(|_| {
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(Vec::new());
                rx
            }),
        };
        let request = RunRequest {
            mcp: None,
            prompt: match scenario {
                "boundary" => "slow-model",
                "interrupt" | "mid-kill" => "slow-tool",
                _ => "hello",
            }
            .into(),
            harness: None,
            model: Some("mock/mock".into()),
            reasoning: None,
            model_options: Default::default(),
            cwd: cwd.display().to_string(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            attachments: Vec::new(),
            worktree: None,
            resume: if scenario == "resume" {
                session.clone()
            } else {
                None
            },
        };
        let mut stream = harness.run(request, controls).await.unwrap();
        let kill = || {
            let pid: i32 = std::fs::read_to_string(cwd.join("adapter.pid"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            // SAFETY: wrapper records the owned adapter for this test.
            assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        };
        let mut done_count = 0;
        let mut started = false;
        let mut tool_seen = false;
        let mut text = String::new();
        tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    AgentEvent::SessionStarted { session_id, .. } => {
                        if scenario == "resume" {
                            assert_eq!(Some(session_id), session);
                        }
                        if scenario == "boundary" && !started {
                            started = true;
                            steer
                                .send(SteerMessage {
                                    prompt: "second".into(),
                                    message_id: None,
                                })
                                .await
                                .unwrap();
                        }
                    }
                    AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
                    AgentEvent::ToolCall { .. } if !tool_seen => {
                        tool_seen = true;
                        tokio::time::timeout(Duration::from_secs(3), async {
                            while !cwd.join("tool.pid").exists() {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        })
                        .await
                        .expect("mock bash tool must actually start");
                        if scenario == "interrupt" {
                            token.cancel();
                        }
                        if scenario == "mid-kill" {
                            kill();
                        }
                    }
                    AgentEvent::Done {
                        status,
                        error,
                        session_id,
                        ..
                    } => {
                        done_count += 1;
                        let expected = match scenario {
                            "interrupt" => DoneStatus::Interrupted,
                            "mid-kill" => DoneStatus::Errored,
                            _ => DoneStatus::Completed,
                        };
                        assert_eq!(status, expected, "{scenario}: {error:?}");
                        if scenario == "mid-kill" {
                            assert!(error.as_deref().unwrap().contains("signal 9"), "{error:?}");
                        }
                        if scenario == "idle-kill" {
                            session = session_id;
                            kill();
                        }
                        if scenario == "boundary" && done_count == 2 || scenario == "resume" {
                            token.cancel();
                            break;
                        }
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("real Pi lifecycle must settle");
        assert_eq!(done_count, if scenario == "boundary" { 2 } else { 1 });
        if matches!(scenario, "boundary" | "idle-kill" | "resume") {
            assert!(text.contains("MOCK-DONE"));
        }
        if matches!(scenario, "interrupt" | "mid-kill") {
            assert!(tool_seen);
        }
        if tool_seen {
            let pid = std::fs::read_to_string(cwd.join("tool.pid")).unwrap();
            let stat =
                std::fs::read_to_string(format!("/proc/{}/stat", pid.trim())).unwrap_or_default();
            assert!(
                stat.is_empty() || stat.split_whitespace().nth(2) == Some("Z"),
                "tool survived {scenario}: {stat}"
            );
        }
        drop(stream);
        drop(steer);
        eprintln!("live {scenario}: {done_count} terminal(s), tool={tool_seen}");
    }
}
