#![cfg(unix)]
//! An engine killed outright (SIGKILL: no teardown runs) leaves no agent
//! running for long: on Linux the kernel signals its agents at once
//! (`PR_SET_PDEATHSIG`), and the next engine on its data dir ends whatever
//! is still there (on macOS, everything) — the agent, and the shells it
//! started in process groups of their own. An unrelated orphan is never
//! touched, and an agent whose Claude login is mid-refresh is signalled only
//! once the refresh is saved.
//!
//! Its own test binary: the killed engine is this binary run again
//! (`ROLE`), with a private HOME and data dir. The agents are a fake
//! `claude` started through the real Claude harness; no real CLI or login
//! is involved.

use std::io::BufRead as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::claude::refresh_lock;
use zeron_harness::{CancellationToken, ClaudeHarness, Harness, RunControls};
use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

const ROLE: &str = "ZERON_AGENTS_DIE_ROLE";
const TEST: &str = "agents_a_killed_engine_left_running_are_ended_by_the_next";

/// `polite` exits on SIGTERM; `stubborn` ignores it (as does the shell it
/// starts in a process group of its own), so only SIGKILL ends them.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
dir="$(dirname "$0")"
case "$*" in
  *--input-format*) ;;
  *) echo "2.1.309 (Claude Code)"; exit 0 ;;
esac
if [ "$FAKE_MODE" = stubborn ]; then
  trap '' TERM
  perl -e 'setpgrp(0, 0); sleep 3600' &
  echo $! > "$dir/stubborn-shell.pid"
else
  trap 'exit 0' TERM
fi
echo $$ > "$dir/$FAKE_MODE.pid"
while :; do sleep 1; done
"#;

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    let stat = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_default();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_pid(path: &Path) -> Option<i32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// SIGKILLs whatever this test started that is still running, whatever
/// way the test ends: the pids it holds, and the agents' (from their pid
/// files in the work dir).
struct Cleanup(Vec<i32>, PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let agents = ["polite.pid", "stubborn.pid", "stubborn-shell.pid"]
            .iter()
            .filter_map(|file| read_pid(&self.1.join(file)));
        for pid in self.0.iter().copied().chain(agents) {
            if alive(pid) {
                // SAFETY: a process this test started, seconds ago.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
    }
}

fn request(mode: &str, work: &Path) -> RunRequest {
    RunRequest {
        prompt: "hello".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: work.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
        mcp: None,
        env: [
            ("FAKE_MODE".to_owned(), mode.to_owned()),
            (
                "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_owned(),
                work.join(format!("config-{mode}")).display().to_string(),
            ),
        ]
        .into(),
        extra_mcp: Vec::new(),
    }
}

fn controls() -> RunControls {
    let (_steer, steering) = tokio::sync::mpsc::channel(1);
    RunControls {
        realtime: None,
        execution_lease: None,
        steering,
        interrupt: CancellationToken::new(),
        request_input: Box::new(|_| tokio::sync::oneshot::channel().1),
        turn: Default::default(),
    }
}

/// The engine that gets killed: it opens the data dir, starts both agents
/// through the Claude harness, reports, and waits to be killed.
fn engine(data: PathBuf, work: PathBuf) -> ! {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let _core = EngineCore::assemble(
            &data,
            Arc::new(HarnessRegistry::new()),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        let script = work.join("claude");
        // Each run held open for good (the fake never answers its
        // handshake, so a run may not return before it is killed).
        for mode in ["polite", "stubborn"] {
            let (script, work) = (script.clone(), work.clone());
            tokio::spawn(async move {
                let _run = ClaudeHarness::new()
                    .with_executable(&script)
                    .run(request(mode, &work), controls())
                    .await;
                std::future::pending::<()>().await;
            });
        }
        for file in ["polite.pid", "stubborn.pid", "stubborn-shell.pid"] {
            while read_pid(&work.join(file)).is_none() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        println!("engine ready");
        std::future::pending::<()>().await;
    });
    unreachable!()
}

#[test]
fn agents_a_killed_engine_left_running_are_ended_by_the_next() {
    if std::env::var_os(ROLE).is_some() {
        engine(
            std::env::var_os("ZERON_DATA_DIR").unwrap().into(),
            std::env::var_os("ZERON_AGENTS_DIE_WORK").unwrap().into(),
        );
    }
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let data = root.path().join("data");
    let work = root.path().join("work");
    for dir in [&home, &data, &work] {
        std::fs::create_dir_all(dir).unwrap();
    }
    for mode in ["polite", "stubborn"] {
        std::fs::create_dir_all(work.join(format!("config-{mode}"))).unwrap();
    }
    // Private HOME and data dir for both engines: nothing here may reach
    // the user's own (~/.zeron, ~/.claude).
    // SAFETY: the only test in this binary, before any thread starts.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("ZERON_DATA_DIR", &data);
    }
    let script = work.join("claude");
    std::fs::write(&script, FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    // An orphan this engine never started, in a group of its own, like the
    // agents' shells: it must outlive the sweep.
    let unrelated = std::process::Command::new("/bin/sh")
        .args([
            "-c",
            "perl -e 'setpgrp(0, 0); sleep 300' >/dev/null 2>&1 & echo $!",
        ])
        .output()
        .unwrap();
    let unrelated: i32 = String::from_utf8_lossy(&unrelated.stdout)
        .trim()
        .parse()
        .unwrap();
    let mut cleanup = Cleanup(vec![unrelated], work.clone());

    let mut first = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(ROLE, "engine")
        .env("ZERON_AGENTS_DIE_WORK", &work)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    cleanup.0.push(first.id() as i32);
    let out = std::io::BufReader::new(first.stdout.take().unwrap());
    let (ready_tx, ready) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            if line.ends_with("engine ready") {
                let _ = ready_tx.send(());
            }
        }
    });
    ready
        .recv_timeout(Duration::from_secs(60))
        .expect("the first engine never started its agents");
    let polite = read_pid(&work.join("polite.pid")).unwrap();
    let stubborn = read_pid(&work.join("stubborn.pid")).unwrap();
    let shell = read_pid(&work.join("stubborn-shell.pid")).unwrap();
    let ledgers = data.join(zeron_harness::process::registry::LEDGER_DIR);
    let ledger_count = || std::fs::read_dir(&ledgers).map_or(0, |dir| dir.count());
    assert_eq!(ledger_count(), 1, "the first engine recorded its agents");

    // Killed outright: no teardown of any kind runs.
    first.kill().unwrap();
    first.wait().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    // Linux: the kernel's death signal already ended the agent that heeds SIGTERM.
    #[cfg(target_os = "linux")]
    wait_until(
        "the death signal to end the polite agent",
        Duration::from_secs(5),
        || !alive(polite),
    );
    // macOS: no kernel help; everything is still running (what the bench found).
    #[cfg(target_os = "macos")]
    assert!(alive(polite), "polite agent ended without the sweep");
    assert!(
        alive(stubborn) && alive(shell),
        "stubborn agent ended without the sweep"
    );

    // The stubborn agent's Claude is mid-refresh when the next engine starts.
    let [held, _] = refresh_lock::lock_paths(&work.join("config-stubborn"));
    std::fs::create_dir(&held).unwrap();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let next = EngineCore::assemble(
        &data,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    wait_until(
        "the polite agent to be ended",
        Duration::from_secs(10),
        || !alive(polite),
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        alive(stubborn) && alive(shell),
        "an agent was signalled while its login refresh was in flight"
    );

    std::fs::remove_dir(&held).unwrap();
    wait_until(
        "the stubborn agent and its shell to be ended",
        Duration::from_secs(20),
        || !alive(stubborn) && !alive(shell),
    );
    assert!(
        alive(unrelated),
        "the sweep ended a process it never started"
    );
    // The dead engine's ledger is done with.
    wait_until(
        "the swept ledger to be removed",
        Duration::from_secs(5),
        || ledger_count() == 0,
    );
    drop(next);
}
