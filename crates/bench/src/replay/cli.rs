//! CLI contract and the long-lived stream-json session/control loop.
use std::collections::VecDeque;
use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{ballast, mitm, script};
use crate::probe::{ProbeEvent, ProbeSender};

#[derive(Clone)]
pub(crate) struct Session {
    pub id: String,
    pub cwd: String,
    pub model: String,
    pub chat: String,
}
pub(crate) enum Output {
    Frame(Value, Option<(String, u64)>),
    End(Result<(String, Value, Option<Value>)>),
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    session_id: String,
    turns: u64,
    text_sha256: String,
    text_bytes: usize,
    transcript: String,
}
fn save_state(state: &mut State, path: Option<&PathBuf>, text: &str) -> Result<()> {
    state.turns += 1;
    state.transcript.push_str(text);
    state.text_bytes = state.transcript.len();
    state.text_sha256 = script::text_sha256(&state.transcript);
    if let Some(path) = path {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let staging = path.with_extension(format!("{}.new", std::process::id()));
        std::fs::write(&staging, serde_json::to_vec(state)?)?;
        std::fs::rename(staging, path)?;
    }
    Ok(())
}

pub fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    log_spawn(&args)?;
    if let Some(at) = args.iter().position(|s| s == "--ballast-child") {
        return ballast::child(args.get(at + 1).context("missing ballast MiB")?.parse()?);
    }
    if !args.iter().any(|s| s == "--print" || s == "-p") {
        println!(
            "{} (zb-replay)",
            std::env::var("ZB_REPLAY_VERSION").unwrap_or_else(|_| "2.1.314".into())
        );
        return Ok(());
    }
    ensure!(
        !args.iter().any(|s| s == "--tap"),
        "tap forwarding is intentionally unavailable; mitm never forwards prompts"
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(run(args))
}

pub(crate) fn fresh_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.iter().enumerate().find_map(|(i, arg)| {
        arg.strip_prefix(&format!("{key}="))
            .map(str::to_owned)
            .or_else(|| (arg == key).then(|| args.get(i + 1).cloned()).flatten())
    })
}
/// The engine hands the chat id to the CLI only inside the inline
/// `--mcp-config` (the `zeron` server's env), not in the CLI's own environment.
/// Returns its last eight characters: fixture chat ids share their first group,
/// and markers must tell chats apart.
fn chat_from_mcp_config(args: &[String]) -> Option<String> {
    let raw = arg_value(args, "--mcp-config")?;
    let config: Value = serde_json::from_str(&raw).ok()?;
    config["mcpServers"]
        .as_object()?
        .values()
        .find_map(|server| server["env"]["ZERON_CHAT_ID"].as_str())
        .map(|id| id.chars().skip(id.chars().count().saturating_sub(8)).collect())
}

fn log_spawn(args: &[String]) -> Result<()> {
    let probe = ProbeSender::from_env();
    probe.send(&ProbeEvent::Spawn {
        prog: "zb-replay".into(),
        argv: args.iter().take(8).cloned().collect(),
        ppid: unsafe { libc::getppid() } as u32,
        t: crate::mono_now(),
    });
    if let Some(dir) = std::env::var_os("ZB_REPLAY_LOG_DIR") {
        std::fs::create_dir_all(&dir)?;
        let allow = std::env::var("ZB_ENV_CAPTURE").unwrap_or_default();
        let allowed: Vec<&str> = allow
            .split([',', ' ', ':'])
            .filter(|s| !s.is_empty())
            .collect();
        let mut names: Vec<_> = std::env::vars_os()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        names.sort();
        let values: serde_json::Map<String, Value> = names
            .iter()
            .filter(|k| {
                matches!(k.as_str(), "MAKEFLAGS" | "CARGO_MAKEFLAGS")
                    || allowed.contains(&k.as_str())
            })
            .map(|k| {
                (
                    k.clone(),
                    std::env::var_os(k)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                        .into(),
                )
            })
            .collect();
        std::fs::write(
            PathBuf::from(dir).join(format!("argv-{}.json", std::process::id())),
            serde_json::to_vec(
                &json!({"argv":args,"cwd":std::env::current_dir()?,"env_names":names,"env":values}),
            )?,
        )?;
    }
    Ok(())
}

/// Copied from the discovery parser's nested response contract and catalog.rs.
pub fn control_success(request_id: &str, initialize: bool) -> Value {
    let payload = if initialize {
        json!({"commands":[],"models":[{"value":"haiku","displayName":"Replay Haiku","description":"Deterministic replay"}],
            "account":null,"output_style":"default","available_output_styles":["default"]})
    } else {
        json!({})
    };
    json!({"type":"control_response","response":{"subtype":"success","request_id":request_id,"response":payload}})
}
pub(crate) fn emit(
    frame: &Value,
    marker: Option<(String, u64)>,
    probe: &ProbeSender,
) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, frame)?;
    out.write_all(b"\n")?;
    out.flush()?;
    if let Some((chat, seq)) = marker {
        probe.send(&ProbeEvent::Mark {
            chat,
            seq,
            t: crate::mono_now(),
        });
    }
    Ok(())
}
fn lifecycle(user: &Value, state: &str, probe: &ProbeSender) -> Result<()> {
    if let Some(uuid) = user["uuid"].as_str() {
        emit(
            &json!({"type":"command_lifecycle","command_uuid":uuid,"state":state}),
            None,
            probe,
        )?;
    }
    Ok(())
}
fn result(session: &Session, error: Option<&str>, usage: Value) -> Value {
    json!({"type":"result","subtype":if error.is_some() {"error_during_execution"} else {"success"},
        "is_error":error.is_some(),"errors":error.into_iter().collect::<Vec<_>>(),"result":"",
        "session_id":session.id,"usage":usage,"modelUsage":{},"num_turns":1,
        "duration_ms":0,"duration_api_ms":0,"total_cost_usd":0.0,
        "terminal_reason":if error.is_some() {"aborted_streaming"} else {"completed"}})
}
fn prompt(user: &Value) -> String {
    let content = &user["message"]["content"];
    content.as_str().map(str::to_owned).unwrap_or_else(|| {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "text")
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}
fn env_number(key: &str) -> Result<f64> {
    let value: f64 = std::env::var(key).unwrap_or_else(|_| "0".into()).parse()?;
    ensure!(
        value.is_finite() && value >= 0.0,
        "{key} must be finite and nonnegative"
    );
    Ok(value)
}

async fn run(args: Vec<String>) -> Result<()> {
    let mode = std::env::var("ZB_REPLAY_MODE").unwrap_or_else(|_| "replay".into());
    ensure!(
        matches!(mode.as_str(), "replay" | "mitm" | "ballast"),
        "unknown replay mode {mode}"
    );
    let resume = arg_value(&args, "--resume");
    let id = resume.clone().unwrap_or(fresh_id()?);
    ensure!(
        !id.is_empty()
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "unsafe session id"
    );
    let state_dir = std::env::var_os("ZB_REPLAY_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR").map(|d| PathBuf::from(d).join("zb-replay-state"))
        });
    let state_path = state_dir.map(|d| d.join(format!("{id}.json")));
    let probe = ProbeSender::from_env();
    let session = Session {
        id,
        cwd: std::env::current_dir()?.to_string_lossy().into_owned(),
        model: arg_value(&args, "--model").unwrap_or_else(|| "haiku".into()),
        chat: std::env::var("ZB_REPLAY_CHAT")
            .or_else(|_| std::env::var("ZERON_CHAT_ID"))
            .ok()
            .or_else(|| chat_from_mcp_config(&args))
            .unwrap_or_else(|| "00000000".into())
            .chars()
            .take(8)
            .collect(),
    };
    let mut state = State {
        session_id: session.id.clone(),
        ..State::default()
    };
    let mut state_size = 0;
    if resume.is_some() {
        let loaded = state_path.as_ref().and_then(|p| std::fs::read(p).ok());
        let Some(bytes) = loaded else {
            emit(
                &result(
                    &session,
                    Some(&format!(
                        "No conversation found with session ID: {}",
                        session.id
                    )),
                    json!({}),
                ),
                None,
                &probe,
            )?;
            bail!("No conversation found with session ID: {}", session.id);
        };
        state_size = bytes.len();
        state = serde_json::from_slice(&bytes).context("invalid replay state")?;
        ensure!(
            state.session_id == session.id
                && state.text_bytes == state.transcript.len()
                && state.text_sha256 == script::text_sha256(&state.transcript),
            "replay state hash/identity mismatch"
        );
    }
    let startup = env_number("ZB_REPLAY_STARTUP_MS")?
        + if resume.is_some() {
            env_number("ZB_REPLAY_RESUME_MS_PER_MB")? * state_size as f64 / 1_000_000.0
        } else {
            0.0
        };
    ensure!(
        startup.is_finite() && startup / 1000.0 < u64::MAX as f64,
        "startup delay overflow"
    );
    let (tx, mut rx) = mpsc::channel(32);
    let mut real = if mode == "mitm" {
        Some(mitm::Real::start(&args, tx.clone()).await?)
    } else {
        None
    };
    // Allocation runs off the control thread; a cold 300 MiB touch cannot delay initialize.
    let mut ballast_task = tokio::spawn(async {
        let mib: usize = std::env::var("ZB_REPLAY_BALLAST_MIB")
            .unwrap_or_else(|_| "0".into())
            .parse()?;
        let _memory = tokio::task::spawn_blocking(move || ballast::Ballast::new(mib)).await??;
        let _children = ballast::Children::from_env()?;
        std::future::pending::<()>().await;
        Ok::<(), anyhow::Error>(())
    });
    // Tokio's stdin uses an uncancellable runtime blocking task: shutting down
    // on SIGTERM with an open pipe would wait for that read forever. A detached
    // OS reader thread instead lets process exit finish teardown immediately.
    let (input, mut lines) = mpsc::channel(32);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if input.blocking_send(line).is_err() {
                break;
            }
        }
    });
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut pending = VecDeque::<Value>::new();
    let mut active: Option<(Value, tokio::task::JoinHandle<()>)> = None;
    let mut active_text = String::new();
    let mut generation = 1u64;
    let mut initialized = false;
    let mut eof = false;
    loop {
        if active.is_none()
            && let Some(user) = pending.pop_front()
        {
            active_text.clear();
            lifecycle(&user, "started", &probe)?;
            if args.iter().any(|a| a == "--replay-user-messages") {
                emit(&user, None, &probe)?;
            }
            let first = !initialized;
            let turn_session = session.clone();
            let turn_tx = tx.clone();
            let text = prompt(&user);
            // Ballast is replay with resident memory, so hibernation probes
            // still play their short script and emit resume markers.
            let synthetic = mode == "mitm";
            let handle = tokio::spawn(async move {
                if first && startup > 0.0 {
                    tokio::time::sleep(Duration::from_secs_f64(startup / 1000.0)).await;
                }
                if first {
                    let init = json!({"type":"system","subtype":"init","session_id":turn_session.id,
                        "cwd":turn_session.cwd,"model":turn_session.model,"tools":["Bash","Read","Grep","Edit","Write"],
                        "mcp_servers":[],"permissionMode":"bypassPermissions"});
                    let _ = turn_tx.send((generation, Output::Frame(init, None))).await;
                }
                let outcome = turn(&text, synthetic, &turn_session, &turn_tx, generation).await;
                let _ = turn_tx.send((generation, Output::End(outcome))).await;
            });
            active = Some((user, handle));
        }
        if eof && active.is_none() && pending.is_empty() {
            break;
        }
        tokio::select! {
            _ = terminate.recv() => { break; }
            _ = interrupt.recv() => { break; }
            allocated = &mut ballast_task => {
                allocated??;
                bail!("ballast owner exited unexpectedly");
            }
            line = lines.recv(), if !eof => {
                let Some(line) = line else { eof = true; continue; };
                let line = line?;
                if line.trim().is_empty() { continue; }
                let frame: Value = serde_json::from_str(&line).context("invalid stdin JSON")?;
                match frame["type"].as_str() {
                    Some("control_request") => {
                        let subtype = frame["request"]["subtype"].as_str().unwrap_or("");
                        let id = frame["request_id"].as_str().unwrap_or("");
                        if let Some(real) = &mut real { real.forward_control(&frame).await?; }
                        else { emit(&control_success(id, subtype == "initialize"), None, &probe)?; }
                        if subtype == "interrupt" {
                            if let Some((user, handle)) = active.take() {
                                handle.abort(); generation += 1;
                                save_state(&mut state, state_path.as_ref(), &active_text)?;
                                lifecycle(&user, "cancelled", &probe)?;
                                emit(&result(&session, Some("Interrupted"), json!({})), None, &probe)?;
                            }
                            if frame["request"]["cancel_queued"] == true {
                                while let Some(user) = pending.pop_front() { lifecycle(&user, "cancelled", &probe)?; }
                            }
                        }
                    }
                    Some("user") => {
                        lifecycle(&frame, "queued", &probe)?;
                        if frame["priority"] == "now" && let Some((user, handle)) = active.take() {
                            handle.abort(); generation += 1;
                            save_state(&mut state, state_path.as_ref(), &active_text)?;
                            lifecycle(&user, "cancelled", &probe)?;
                            emit(&result(&session, Some("Interrupted by next command"), json!({})), None, &probe)?;
                        }
                        pending.push_back(frame);
                    }
                    // Never route engine control responses (tool inputs can contain prompts),
                    // unknown envelopes, or non-JSON bytes into the real child.
                    _ => {}
                }
            }
            Some((source, output)) = rx.recv() => {
                if source != 0 && source != generation { continue; }
                match output {
                    Output::Frame(frame, mark) => {
                        if source != 0 && frame["event"]["delta"]["type"] == "text_delta" && frame["parent_tool_use_id"].is_null() {
                            if let Some(text) = frame["event"]["delta"]["text"].as_str() { active_text.push_str(text); }
                        }
                        let init = frame["type"] == "system" && frame["subtype"] == "init";
                        emit(&frame, mark, &probe)?;
                        if init {
                            initialized = true;
                            probe.send(&ProbeEvent::Init { session: session.id.clone(), resumed: resume.is_some(), t:crate::mono_now() });
                        }
                    }
                    Output::End(outcome) => {
                        let Some((user, _)) = active.take() else { continue; };
                        match outcome {
                            Ok((text, usage, raw_result)) => {
                                save_state(&mut state, state_path.as_ref(), &text)?;
                                lifecycle(&user, "completed", &probe)?;
                                match raw_result {
                                    Some(frame) if frame.is_null() => {},
                                    Some(frame) => emit(&frame, None, &probe)?,
                                    None => emit(&result(&session, None, usage), None, &probe)?,
                                }
                            }
                            Err(error) => {
                                save_state(&mut state, state_path.as_ref(), &active_text)?;
                                lifecycle(&user, "failed", &probe)?;
                                emit(&result(&session, Some(&format!("{error:#}")), json!({})), None, &probe)?;
                            }
                        }
                        generation += 1;
                    }
                }
            }
        }
    }
    if let Some((_, handle)) = active {
        handle.abort();
        let _ = handle.await;
    }
    ballast_task.abort();
    let _ = ballast_task.await;
    if let Some(real) = &mut real {
        real.stop().await;
    }
    Ok(())
}
async fn turn(
    text: &str,
    synthetic: bool,
    session: &Session,
    tx: &mpsc::Sender<(u64, Output)>,
    generation: u64,
) -> Result<(String, Value, Option<Value>)> {
    if !synthetic {
        let play = match script::Play::parse(text)? {
            Some(play) => Some(play),
            None if !matches!(text, "zb:hold" | "zb:echo") && !text.starts_with("zb:echo ") => {
                std::env::var("ZB_REPLAY_DEFAULT_SCRIPT")
                    .ok()
                    .map(|path| script::Play {
                        path,
                        speed: 1.0,
                        markers: 0,
                        loops: 1,
                    })
            }
            None => None,
        };
        if let Some(play) = play {
            return script::play(&play, session, tx, generation).await;
        }
    }
    let text = if text == "zb:hold" || synthetic {
        "held"
    } else {
        text.strip_prefix("zb:echo ").unwrap_or(text)
    }
    .to_owned();
    for f in [
        json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}}),
        json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}),
        script::assistant(session, vec![json!({"type":"text","text":text})]),
    ] {
        tx.send((generation, Output::Frame(f, None))).await?;
    }
    Ok((text, json!({"input_tokens":0,"output_tokens":0}), None))
}

#[cfg(test)]
mod chat_tests {
    use super::*;

    #[test]
    fn chat_id_comes_from_the_inline_mcp_config() {
        let args = vec![
            "--print".to_string(),
            "--mcp-config".to_string(),
            r#"{"mcpServers":{"zeron":{"command":"z","env":{"ZERON_CHAT_ID":"b4000002-0000-4000-8000-0000000000b4"}}}}"#
                .to_string(),
        ];
        assert_eq!(chat_from_mcp_config(&args).as_deref(), Some("000000b4"));
        assert_eq!(chat_from_mcp_config(&["--print".to_string()]), None);
        let empty = vec!["--mcp-config".to_string(), r#"{"mcpServers":{}}"#.to_string()];
        assert_eq!(chat_from_mcp_config(&empty), None);
    }
}
