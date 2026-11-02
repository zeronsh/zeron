//! Control-only bridge. The child-stdin write boundary accepts no user envelope.
//! No tap/prompt forwarding path exists in this module.
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, mpsc};

use super::cli::Output;
use crate::probe::{McpServer, ProbeEvent, ProbeSender};

const INIT_ID: &str = "__zb_private_initialize";
const STATUS_ID: &str = "__zb_private_mcp_status";
#[derive(Default)]
struct Discovery {
    initialize: Option<Value>,
    waiting: Vec<String>,
    t_init: u64,
    ready: bool,
    unsupported: bool,
}
pub struct Real {
    child: Child,
    group: u32,
    stdin: Arc<Mutex<ChildStdin>>,
    discovery: Arc<Mutex<Discovery>>,
    tx: mpsc::Sender<(u64, Output)>,
    reader: tokio::task::JoinHandle<()>,
    poller: tokio::task::JoinHandle<()>,
}
impl Real {
    pub(crate) async fn start(args: &[String], tx: mpsc::Sender<(u64, Output)>) -> Result<Self> {
        control_only_args(args)?;
        let exe = std::env::var_os("ZB_REPLAY_REAL_CLAUDE")
            .context("mitm requires ZB_REPLAY_REAL_CLAUDE")?;
        ensure!(
            std::fs::canonicalize(&exe)? != std::fs::canonicalize(std::env::current_exe()?)?,
            "mitm recursion"
        );
        let t_spawn = crate::mono_now();
        let mut cmd = Command::new(exe);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        {
            let parent = std::process::id();
            unsafe {
                cmd.pre_exec(move || {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                    if libc::getppid() as u32 != parent {
                        libc::_exit(1);
                    }
                    Ok(())
                });
            }
        }
        let mut child = cmd.spawn().context("start mitm control-only child")?;
        let pid = child.id().context("real child has no pid")?;
        let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let discovery = Arc::new(Mutex::new(Discovery::default()));
        write_control(&stdin, &json!({"type":"control_request","request_id":INIT_ID,"request":{"subtype":"initialize","perTaskStopAffordance":true}})).await?;
        let state = discovery.clone();
        let sender = tx.clone();
        let reader = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let id = frame["response"]["request_id"].as_str().unwrap_or("");
                if frame["type"] == "control_response" && id == INIT_ID {
                    let mut state = state.lock().await;
                    state.t_init = crate::mono_now();
                    state.initialize = Some(frame.clone());
                    for request_id in std::mem::take(&mut state.waiting) {
                        let mut response = frame.clone();
                        response["response"]["request_id"] = request_id.into();
                        let _ = sender.send((0, Output::Frame(response, None))).await;
                    }
                } else if frame["type"] == "control_response" && id == STATUS_ID {
                    let mut state = state.lock().await;
                    if frame["response"]["subtype"] == "error" {
                        state.unsupported = true;
                        continue;
                    }
                    let payload = &frame["response"]["response"];
                    if let Some(servers) = payload
                        .get("mcpServers")
                        .or_else(|| payload.get("mcp_servers"))
                        .or_else(|| payload.get("servers"))
                        .and_then(Value::as_array)
                    {
                        let servers: Vec<McpServer> = servers
                            .iter()
                            .map(|s| McpServer {
                                name: s["name"].as_str().unwrap_or("").into(),
                                status: s["status"].as_str().unwrap_or("unknown").into(),
                            })
                            .collect();
                        if !state.ready
                            && servers
                                .iter()
                                .all(|s| matches!(s.status.as_str(), "connected" | "failed"))
                        {
                            state.ready = true;
                            ProbeSender::from_env().send(&ProbeEvent::McpReady {
                                t_spawn,
                                t_init: state.t_init,
                                t_ready: crate::mono_now(),
                                servers,
                                fallback: false,
                            });
                        }
                    } else {
                        state.unsupported = true;
                    }
                } else {
                    let _ = sender.send((0, Output::Frame(frame, None))).await;
                }
            }
        });
        let state = discovery.clone();
        let input = stdin.clone();
        let poller = tokio::spawn(async move {
            let mut previous = Vec::new();
            let mut stable_since = None;
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                let mut state = state.lock().await;
                if state.ready {
                    break;
                }
                if state.t_init == 0 {
                    continue;
                }
                // A missing/unsupported status RPC falls back to the complete
                // descendant pid set remaining stable for five seconds.
                if state.unsupported
                    || crate::mono_now().saturating_sub(state.t_init) > 5_000_000_000
                {
                    let tree = process_tree(pid);
                    if tree != previous {
                        stable_since = Some(crate::mono_now());
                        previous = tree;
                    }
                    let stable_since = *stable_since.get_or_insert_with(crate::mono_now);
                    if crate::mono_now().saturating_sub(stable_since) >= 5_000_000_000 {
                        state.ready = true;
                        ProbeSender::from_env().send(&ProbeEvent::McpReady {
                            t_spawn,
                            t_init: state.t_init,
                            t_ready: crate::mono_now(),
                            servers: Vec::new(),
                            fallback: true,
                        });
                        break;
                    }
                } else {
                    drop(state);
                    if write_control(&input, &json!({"type":"control_request","request_id":STATUS_ID,"request":{"subtype":"mcp_status"}})).await.is_err() { break; }
                }
            }
        });
        Ok(Self {
            child,
            group: pid,
            stdin,
            discovery,
            tx,
            reader,
            poller,
        })
    }
    pub(crate) async fn forward_control(&mut self, frame: &Value) -> Result<()> {
        ensure!(
            frame["type"] == "control_request",
            "mitm refuses non-control stdin"
        );
        let request_id = frame["request_id"]
            .as_str()
            .context("control request without id")?;
        ensure!(
            !request_id.starts_with("__zb_private_"),
            "reserved control request id"
        );
        if frame["request"]["subtype"] == "initialize" {
            let mut state = self.discovery.lock().await;
            if let Some(mut response) = state.initialize.clone() {
                response["response"]["request_id"] = request_id.into();
                self.tx.send((0, Output::Frame(response, None))).await?;
            } else {
                state.waiting.push(request_id.into());
            }
            return Ok(());
        }
        write_control(&self.stdin, frame).await
    }
    pub(crate) async fn stop(&mut self) {
        self.reader.abort();
        self.poller.abort();
        unsafe {
            libc::kill(-(self.group as i32), libc::SIGKILL);
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

// Engine runs are stream-json with prompts on stdin. Refuse alternate CLI
// invocation forms before a real process exists: `-p PROMPT` must not bypass
// the stdin boundary, nor may a system-prompt argument become a tap path.
fn control_only_args(args: &[String]) -> Result<()> {
    let mut at = 0;
    while at < args.len() {
        let arg = &args[at];
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(k, v)| (k, Some(v)));
        match flag {
            "--input-format"
            | "--output-format"
            | "--thinking-display"
            | "--permission-prompt-tool"
            | "--model"
            | "--effort"
            | "--permission-mode"
            | "--settings"
            | "--mcp-config"
            | "--tools"
            | "--setting-sources"
            | "--resume"
            | "--disallowedTools" => {
                if inline.is_none() {
                    at += 1;
                    ensure!(args.get(at).is_some(), "missing {flag} value");
                }
            }
            "--print"
            | "-p"
            | "--verbose"
            | "--include-partial-messages"
            | "--replay-user-messages"
            | "--dangerously-skip-permissions"
            | "--allow-dangerously-skip-permissions"
            | "--strict-mcp-config" => {
                ensure!(inline.is_none(), "mitm refuses prompt-bearing flag {arg}");
            }
            _ => anyhow::bail!("mitm refuses positional/prompt or unknown argument {flag}"),
        }
        at += 1;
    }
    Ok(())
}
impl Drop for Real {
    fn drop(&mut self) {
        self.reader.abort();
        self.poller.abort();
        unsafe {
            libc::kill(-(self.group as i32), libc::SIGKILL);
        }
    }
}

/// Whitelist both envelope and payload fields. A user/tool frame, or an
/// arbitrary `prompt` nested in a control request, cannot reach child stdin.
async fn write_control(stdin: &Mutex<ChildStdin>, frame: &Value) -> Result<()> {
    ensure!(
        frame["type"] == "control_request",
        "mitm refuses non-control stdin"
    );
    let subtype = frame["request"]["subtype"].as_str().unwrap_or("");
    let mut request = json!({"subtype":subtype});
    match subtype {
        "initialize" => {
            if let Some(value) = frame["request"]["perTaskStopAffordance"].as_bool() {
                request["perTaskStopAffordance"] = value.into();
            }
        }
        "interrupt" => {
            if let Some(value) = frame["request"]["cancel_queued"].as_bool() {
                request["cancel_queued"] = value.into();
            }
        }
        "mcp_status" => {}
        "set_model" => request["model"] = frame["request"]["model"].clone(),
        "set_permission_mode" => request["mode"] = frame["request"]["mode"].clone(),
        "stop_task" => request["task_id"] = frame["request"]["task_id"].clone(),
        "apply_flag_settings" => {
            let mut flags = serde_json::Map::new();
            for key in ["fastMode", "alwaysThinkingEnabled", "ultracode"] {
                let value = &frame["request"]["settings"][key];
                if value.is_boolean() || value.is_null() {
                    flags.insert(key.into(), value.clone());
                }
            }
            let effort = &frame["request"]["settings"]["effortLevel"];
            if effort.is_null()
                || matches!(
                    effort.as_str(),
                    Some("low" | "medium" | "high" | "xhigh" | "max")
                )
            {
                flags.insert("effortLevel".into(), effort.clone());
            }
            request["settings"] = Value::Object(flags);
        }
        _ => anyhow::bail!("mitm refuses unsupported control subtype {subtype}"),
    }
    let clean =
        json!({"type":"control_request","request_id":frame["request_id"],"request":request});
    let mut stdin = stdin.lock().await;
    stdin.write_all(format!("{clean}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn process_tree(root: u32) -> Vec<u32> {
    fn visit(pid: u32, out: &mut Vec<u32>) {
        if out.contains(&pid) {
            return;
        }
        out.push(pid);
        if let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) {
            for task in tasks.flatten() {
                if let Ok(children) = std::fs::read_to_string(task.path().join("children")) {
                    for child in children.split_whitespace().filter_map(|s| s.parse().ok()) {
                        visit(child, out);
                    }
                }
            }
        }
    }
    let mut tree = Vec::new();
    visit(root, &mut tree);
    tree.sort_unstable();
    tree
}
#[cfg(not(target_os = "linux"))]
fn process_tree(root: u32) -> Vec<u32> {
    vec![root]
}
