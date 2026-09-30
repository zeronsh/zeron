//! End-to-end live handoff with REAL processes: a real `zeron headless` engine,
//! a real shell, a scripted agent (the fake Claude CLI the harness tests use),
//! and a real handoff to a second copy of the binary. The engine keeps its PID,
//! so the agent and the shell stay its children — this asserts exactly that,
//! plus that the transcript continues, a parked question still answers, and no
//! client connection is ever refused.
//!
//! Unix only; drives the engine through the same MCP tool layer an agent uses.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde_json::{Value, json};
use zeron_mcp::{Origin, Tools, Zeron};

const WAIT: Duration = Duration::from_secs(60);

/// `E2E_SHOW=1` narrates the run (used by `scripts/live-update-demo.sh`).
fn show(line: impl AsRef<str>) {
    if std::env::var_os("E2E_SHOW").is_some() {
        println!("» {}", line.as_ref());
    }
}

fn fake_claude() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/harness/tests/fixtures/fake_claude_handoff.py");
    let path = path.canonicalize().expect("the fake claude fixture exists");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The pids of `parent`'s children, with their command lines (Linux `/proc`).
fn children_of(parent: u32) -> Vec<(u32, String)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        // "pid (comm) S ppid ..." — comm may contain spaces, so split after ')'.
        let Some(rest) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
            continue;
        };
        let ppid: u32 = rest
            .split_whitespace()
            .nth(1)
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        if ppid == parent {
            let cmdline = std::fs::read(format!("/proc/{pid}/cmdline"))
                .map(|raw| String::from_utf8_lossy(&raw).replace('\0', " "))
                .unwrap_or_default();
            found.push((pid, cmdline));
        }
    }
    found
}

fn alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat
            .rsplit_once(')')
            .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z')),
        Err(_) => false,
    }
}

fn exe_of(pid: u32) -> PathBuf {
    std::fs::read_link(format!("/proc/{pid}/exe")).unwrap_or_default()
}

struct Env {
    dir: tempfile::TempDir,
    port: u16,
    engine: Child,
    tools: Tools,
    zeron: Arc<Zeron>,
}

impl Env {
    async fn start() -> Self {
        Self::start_with(true).await
    }

    /// `allow_any_target`: lift the rule that a handoff only goes to this
    /// install's own binary (the tests hand off to copies in a temp dir).
    async fn start_with(allow_any_target: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let engine = Self::spawn_engine(
            dir.path(),
            port,
            Path::new(env!("CARGO_BIN_EXE_zeron")),
            allow_any_target,
        );
        let zeron = Arc::new(Zeron::new(
            format!("ws://127.0.0.1:{port}"),
            Origin::default(),
        ));
        let mut env = Self {
            dir,
            port,
            engine,
            tools: Tools::new(zeron.clone()),
            zeron,
        };
        env.wait_ready().await;
        env
    }

    fn spawn_engine(dir: &Path, port: u16, exe: &Path, allow_any_target: bool) -> Child {
        let mut command = Command::new(exe);
        if allow_any_target {
            command.env("ZERON_HANDOFF_ANY_EXE", "1");
        }
        command
            .arg("headless")
            .env("HOME", dir)
            .env("ZERON_DATA_DIR", dir.join("data"))
            .env("ZERON_IPC_PORT", port.to_string())
            .env("CLAUDE_CODE_EXECUTABLE", fake_claude())
            .env("ZERON_AUTO_UPDATE", "0")
            .env(
                "RUST_LOG",
                std::env::var("E2E_LOG").unwrap_or_else(|_| "warn".into()),
            )
            .stdin(Stdio::null())
            .stdout(if std::env::var_os("E2E_LOG").is_some() {
                Stdio::inherit()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn zeron headless")
    }

    async fn wait_ready(&mut self) {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(info) = self.zeron.engine_info().await
                && info.get("capabilities").is_some()
            {
                return;
            }
            assert!(Instant::now() < deadline, "the engine never became ready");
            assert!(
                self.engine.try_wait().unwrap().is_none(),
                "the engine exited"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn tool(&self, name: &str, args: Value) -> Value {
        self.tools
            .call(name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e}"))
    }

    async fn transcript(&self, chat: &str) -> String {
        self.tool("read_chat", json!({ "chat": chat, "limit": 200 }))
            .await
            .to_string()
    }

    async fn wait_transcript_contains(&self, chat: &str, needle: &str) {
        let deadline = Instant::now() + WAIT;
        loop {
            let text = self.transcript(chat).await;
            if text.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{needle:?} never appeared in: {text}"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    fn agent_pid(&self) -> u32 {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some((pid, _)) = children_of(self.engine.id())
                .into_iter()
                .find(|(_, cmd)| cmd.contains("fake_claude_handoff"))
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "no agent child of the engine");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn shell_pid(&self) -> u32 {
        let children = children_of(self.engine.id());
        children
            .iter()
            .find(|(_, cmd)| {
                let first = cmd.split_whitespace().next().unwrap_or("");
                ["sh", "bash", "zsh", "fish", "dash"]
                    .iter()
                    .any(|shell| first.ends_with(shell))
            })
            .map(|(pid, _)| *pid)
            .unwrap_or_else(|| panic!("a shell child of the engine among {children:?}"))
    }

    async fn open_terminal(&self, chat: &str) -> String {
        let session = self
            .zeron
            .call(
                "OpenTerminal",
                json!({ "chatId": chat, "cols": 80, "rows": 24 }),
            )
            .await
            .expect("OpenTerminal");
        session["id"].as_str().expect("terminal id").to_string()
    }

    async fn terminal_write(&self, terminal: &str, text: &str) {
        self.zeron
            .call(
                "WriteTerminal",
                json!({ "terminalId": terminal, "data": BASE64.encode(text) }),
            )
            .await
            .expect("WriteTerminal");
    }

    /// Output of `terminal` (replayed window + live) until it contains `needle`.
    async fn wait_terminal_output(&self, terminal: &str, needle: &str) {
        let mut stream = self
            .zeron
            .subscribe("SubscribeTerminal", json!({ "terminalId": terminal }))
            .await
            .expect("SubscribeTerminal");
        let deadline = tokio::time::Instant::now() + WAIT;
        let mut seen = Vec::new();
        loop {
            let item = tokio::time::timeout_at(deadline, stream.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "{needle:?} never appeared in terminal output: {}",
                        String::from_utf8_lossy(&seen)
                    )
                })
                .expect("terminal stream alive");
            if item["type"] == "data"
                && let Some(bytes) = item["data"].as_str().and_then(|d| BASE64.decode(d).ok())
            {
                seen.extend(bytes);
            }
            if String::from_utf8_lossy(&seen).contains(needle) {
                return;
            }
        }
    }

    async fn handoff_to(&self, exe: &Path) {
        self.zeron
            .call("HandoffEngine", json!({ "exe": exe }))
            .await
            .expect("HandoffEngine");
    }

    async fn wait_handoff_done(&self, exe: &Path) {
        let deadline = Instant::now() + WAIT;
        loop {
            if exe_of(self.engine.id()) == exe.canonicalize().unwrap() {
                // The new image serves again.
                if self.zeron.engine_info().await.is_ok() {
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "the engine never moved onto {}",
                exe.display()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn stop(&mut self) {
        let _ = self.engine.kill();
        let _ = self.engine.wait();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.stop();
        // The fake agent and shells die with their parent's pipes closing; make
        // sure no test child outlives the test.
        for (pid, cmd) in children_of(std::process::id()) {
            if cmd.contains("fake_claude_handoff") {
                unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            }
        }
        let _ = &self.dir;
        let _ = self.port;
    }
}

/// A second copy of the binary: a different path the engine can exec into.
fn copy_of_binary(dir: &Path) -> PathBuf {
    let target = dir.join("zeron-next");
    std::fs::copy(env!("CARGO_BIN_EXE_zeron"), &target).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    target
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handoff_keeps_agent_and_shell_alive_and_the_transcript_continuous() {
    let mut env = Env::start().await;
    let created = env
        .tool(
            "create_chat",
            json!({ "harness": "claude-code", "prompt": "ask" }),
        )
        .await;
    let chat = created["chatId"]
        .as_str()
        .or_else(|| created["id"].as_str())
        .unwrap_or_else(|| panic!("no chat id in {created}"))
        .to_string();
    env.wait_transcript_contains(&chat, "asking").await;

    let engine_pid = env.engine.id();
    let agent_pid = env.agent_pid();
    assert!(alive(agent_pid));
    let before_exe = exe_of(engine_pid);
    show(format!(
        "engine pid {engine_pid} running {}",
        before_exe.display()
    ));
    show(format!("agent pid  {agent_pid} (a child of the engine)"));

    // A real shell with a background job that must outlive the handoff.
    let terminal = env.open_terminal(&chat).await;
    env.terminal_write(&terminal, "echo ready-$((1+1)); sleep 300 &\n")
        .await;
    env.wait_terminal_output(&terminal, "ready-2").await;
    let shell_pid = env.shell_pid();
    show(format!("shell pid  {shell_pid} (a child of the engine)"));
    show("agent is parked on a question; handing the engine to a new binary...");

    // The turn is parked on a question (the fake asked and waits).
    let deadline = Instant::now() + WAIT;
    let (request_id, question_id) = loop {
        let read = env
            .tool("read_chat", json!({ "chat": chat, "limit": 50 }))
            .await;
        if let Some(id) = read["pendingInput"]["requestId"].as_str() {
            let question = read["pendingInput"]["questions"][0]["id"]
                .as_str()
                .unwrap_or_else(|| panic!("no question id in {read}"));
            break (id.to_string(), question.to_string());
        }
        assert!(Instant::now() < deadline, "no pending question in {read}");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    // A client hammering the port across the handoff: nothing may be refused.
    let refused = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pinger = {
        let (refused, stop, port) = (refused.clone(), stop.clone(), env.port);
        tokio::spawn(async move {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                if tokio::net::TcpStream::connect(("127.0.0.1", port))
                    .await
                    .is_err()
                {
                    refused.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };

    let next = copy_of_binary(env.dir.path());
    env.handoff_to(&next).await;
    env.wait_handoff_done(&next).await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = pinger.await;

    show(format!(
        "engine pid {} now running {}",
        env.engine.id(),
        exe_of(engine_pid).display()
    ));
    show(format!(
        "agent pid  {} / shell pid {} — unchanged, still alive",
        env.agent_pid(),
        env.shell_pid()
    ));
    show(format!(
        "connections refused during the handoff: {}",
        refused.load(std::sync::atomic::Ordering::SeqCst)
    ));
    assert_eq!(env.engine.id(), engine_pid, "same pid");
    assert_ne!(exe_of(engine_pid), before_exe, "a different image now");
    assert_eq!(env.agent_pid(), agent_pid, "same agent process");
    assert!(alive(agent_pid));
    assert_eq!(env.shell_pid(), shell_pid, "same shell");
    assert!(alive(shell_pid));
    assert_eq!(
        refused.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no connection was refused during the handoff"
    );

    // The parked question still answers, with the original ids.
    env.tool(
        "respond_to_input",
        json!({ "chat": chat, "request_id": request_id,
                "answers": [{ "question_id": question_id, "labels": ["B"] }] }),
    )
    .await;
    env.wait_transcript_contains(&chat, "answered: B").await;
    show("the parked question was answered after the handoff (the agent replied: answered: B)");
    env.tool("send_message", json!({ "chat": chat, "text": "second" }))
        .await;
    env.wait_transcript_contains(&chat, "echo: second").await;

    let transcript = env.transcript(&chat).await;
    assert_eq!(
        transcript.matches("answered: B").count(),
        1,
        "no duplicated entries: {transcript}"
    );
    assert!(!transcript.contains("aborted"), "{transcript}");
    env.terminal_write(&terminal, "echo still-$((1+2))\n").await;
    env.wait_terminal_output(&terminal, "still-3").await;
    env.stop();
}

async fn wait_handoff_outcome(env: &Env) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = env
            .zeron
            .call("HandoffStatus", json!({}))
            .await
            .expect("HandoffStatus");
        if status["state"] != "idle" && status["state"] != "running" {
            return status;
        }
        assert!(Instant::now() < deadline, "no handoff outcome: {status}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn create_chat(env: &Env, prompt: &str) -> String {
    let created = env
        .tool(
            "create_chat",
            json!({ "harness": "claude-code", "prompt": prompt }),
        )
        .await;
    created["chatId"]
        .as_str()
        .or_else(|| created["id"].as_str())
        .unwrap_or_else(|| panic!("no chat id in {created}"))
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_preflight_leaves_the_old_engine_untouched() {
    let mut env = Env::start().await;
    let chat = create_chat(&env, "first").await;
    env.wait_transcript_contains(&chat, "echo: first").await;
    let engine_pid = env.engine.id();
    let agent_pid = env.agent_pid();
    let before_exe = exe_of(engine_pid);

    // A "new build" that cannot even answer the preflight.
    let bogus = env.dir.path().join("zeron-garbage");
    std::fs::write(&bogus, "#!/bin/sh\necho garbage\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bogus, std::fs::Permissions::from_mode(0o755)).unwrap();
    env.handoff_to(&bogus).await;
    let status = wait_handoff_outcome(&env).await;
    assert_eq!(status["state"], "failed", "{status}");
    assert!(
        status["message"]
            .as_str()
            .unwrap_or("")
            .contains("preflight")
            || status["message"].as_str().unwrap_or("").contains("handoff"),
        "{status}"
    );

    // Nothing was touched: same image, same agent, still serving and working.
    assert_eq!(env.engine.id(), engine_pid);
    assert_eq!(exe_of(engine_pid), before_exe);
    assert_eq!(env.agent_pid(), agent_pid);
    env.tool("send_message", json!({ "chat": chat, "text": "second" }))
        .await;
    env.wait_transcript_contains(&chat, "echo: second").await;
    env.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_handoff_disturbs_nothing_and_goes_through_once_it_clears() {
    let mut env = Env::start().await;
    let chat = create_chat(&env, "hang").await;
    env.wait_transcript_contains(&chat, "hanging").await;
    let engine_pid = env.engine.id();
    let agent_pid = env.agent_pid();
    let before_exe = exe_of(engine_pid);
    // The fake ignores the interrupt request, so the run stays in its
    // interrupting state until the escalation kills it: not a point at which a
    // run may be carried across.
    env.tool("interrupt_chat", json!({ "chat": chat })).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let next = copy_of_binary(env.dir.path());
    env.handoff_to(&next).await;
    let status = wait_handoff_outcome(&env).await;
    assert_eq!(status["busy"], true, "deferred, not failed: {status}");
    assert_eq!(exe_of(engine_pid), before_exe, "nothing was touched");
    assert_eq!(env.engine.id(), engine_pid);

    // Once the interrupted run has settled the same request goes through.
    let deadline = Instant::now() + WAIT;
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        env.handoff_to(&next).await;
        let deadline_status = Instant::now() + Duration::from_secs(5);
        let mut moved = false;
        loop {
            if exe_of(engine_pid) == next.canonicalize().unwrap() {
                moved = true;
                break;
            }
            let status = env.zeron.call("HandoffStatus", json!({})).await;
            if let Ok(status) = status
                && status["busy"] == true
            {
                break;
            }
            if Instant::now() > deadline_status {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if moved {
            break;
        }
        assert!(Instant::now() < deadline, "the handoff never went through");
    }
    env.wait_handoff_done(&next).await;
    assert_eq!(env.engine.id(), engine_pid);
    assert!(
        !alive(agent_pid),
        "the interrupted agent was stopped, not carried"
    );
    // The engine still works on its new image.
    let second = create_chat(&env, "first").await;
    env.wait_transcript_contains(&second, "echo: first").await;
    env.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_cannot_make_the_engine_exec_an_arbitrary_binary() {
    // The engine's own rule (no override): only this install's binary.
    let mut env = Env::start_with(false).await;
    let chat = create_chat(&env, "first").await;
    env.wait_transcript_contains(&chat, "echo: first").await;
    let engine_pid = env.engine.id();
    let before_exe = exe_of(engine_pid);
    let stranger = copy_of_binary(env.dir.path());
    env.handoff_to(&stranger).await;
    let status = wait_handoff_outcome(&env).await;
    assert_eq!(status["state"], "failed", "{status}");
    assert!(
        status["message"]
            .as_str()
            .unwrap_or("")
            .contains("not this install's binary"),
        "{status}"
    );
    assert_eq!(exe_of(engine_pid), before_exe, "still the same image");
    env.tool("send_message", json!({ "chat": chat, "text": "second" }))
        .await;
    env.wait_transcript_contains(&chat, "echo: second").await;
    env.stop();
}
