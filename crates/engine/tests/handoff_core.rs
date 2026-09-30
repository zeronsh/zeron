//! Real-process handoff: does anything actually survive an `execve`?
//!
//! The unit tests replace the exec with a closure; this replaces the process.
//! The test binary re-executes itself as an "engine" (`engine_child`, driven by
//! environment variables, a no-op in a normal run) that holds an instance lock,
//! an IPC-style listener and a real shell in a terminal, then hands all of it
//! to a new image of the same binary with the library's own freeze / manifest /
//! exec / adopt code. The parent test process watches from outside.
//!
//! What only a real exec can show: the PID and the shell survive; the lock is
//! never released; a client that connects while no image is accepting is served
//! by the successor (the listener's backlog); the successor's children do not
//! inherit what was handed over; and a successor that fails to boot hands back
//! to the predecessor, which adopts the very same manifest.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use zeron_engine::handoff::{self, Manifest, exec_into};
use zeron_engine::{InstanceLock, Terminals};
use zeron_proto::TerminalEvent;

/// Set (with the others) by the parent to turn `engine_child` into an engine.
const ROLE: &str = "ZERON_HANDOFF_TEST_ROLE";
const DIR: &str = "ZERON_HANDOFF_TEST_DIR";
/// Make the FIRST successor fail before committing (exercises rollback).
const FAIL_FIRST_BOOT: &str = "ZERON_HANDOFF_TEST_FAIL_FIRST_BOOT";
/// Run the successor through the REAL `Engine::run_adopting`, with a data dir
/// that makes engine assembly fail after the terminals were adopted.
const REAL_ENGINE: &str = "ZERON_HANDOFF_TEST_REAL_ENGINE";

// ── the child: an "engine" that is either the predecessor or the successor ──

fn say(message: impl std::fmt::Display) {
    println!("T:{message}");
    let _ = std::io::stdout().flush();
}

/// All output the terminal has produced so far, once `done` accepts it.
async fn output_until(terminals: &Terminals, id: &str, done: impl Fn(&str) -> bool) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut rx = terminals.subscribe(id, None).expect("subscribe");
    let mut text = String::new();
    loop {
        if done(&text) {
            return text;
        }
        let event = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("expected output never arrived; got {text:?}"))
            .expect("terminal stream alive");
        if let TerminalEvent::Data { data, .. } = event {
            text.push_str(&String::from_utf8_lossy(&BASE64.decode(data).unwrap()));
        }
    }
}

/// `<marker>-<digits>` in `text` → the number (the echoed command line holds
/// `$$`, never digits, so this only matches the shell's own output).
fn number_after(text: &str, marker: &str) -> Option<u32> {
    text.match_indices(marker).find_map(|(at, _)| {
        let digits: String = text[at + marker.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits.parse().ok()
    })
}

#[test]
fn engine_child() {
    if std::env::var_os(ROLE).is_none() {
        return; // an ordinary test run: nothing to do
    }
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(child_main());
}

async fn child_main() {
    let dir = PathBuf::from(std::env::var(DIR).expect("data dir"));
    match handoff::read_adoption_from_env() {
        Ok(Some(adoption)) => successor(&dir, adoption).await,
        Ok(None) => predecessor(&dir).await,
        Err(error) => {
            say(format!("BADMANIFEST {error}"));
            std::process::exit(3);
        }
    }
}

async fn predecessor(dir: &Path) {
    if std::env::var_os(REAL_ENGINE).is_some() {
        // Assembly opens the project-actions store AFTER adopting terminals;
        // a directory where its file should be makes that open fail with
        // EISDIR — a realistic "the new build cannot open this store" failure.
        std::fs::create_dir_all(dir.join("profiles/local/project-actions.json")).unwrap();
    }
    let lock = InstanceLock::acquire(dir).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let terminals = Terminals::new();
    let session = terminals
        .open_with_shell("/tmp", 80, 24, Some("/bin/sh"))
        .unwrap();
    terminals
        .write_bytes(&session.id, b"echo shellpid-$$\n")
        .unwrap();
    let text = output_until(&terminals, &session.id, |text| {
        number_after(text, "shellpid-").is_some()
    })
    .await;
    let shell = number_after(&text, "shellpid-").expect("shell pid");
    say(format!(
        "OLD pid={} port={port} shell={shell}",
        std::process::id()
    ));

    // Wait for the parent's go-ahead.
    let mut go = String::new();
    std::io::stdin().read_line(&mut go).unwrap();

    // ── the handoff, with the library's own pieces ──
    let frozen = terminals.freeze().await.expect("freeze");
    handoff::set_inheritable(listener.as_raw_fd(), true).unwrap();
    handoff::set_inheritable(lock.raw_fd(), true).unwrap();
    frozen.make_inheritable().unwrap();
    let manifest = Manifest {
        version: handoff::MANIFEST_VERSION,
        from_exe: std::env::current_exe().unwrap(),
        from_version: "predecessor".into(),
        listener_fd: listener.as_raw_fd(),
        lock_fd: lock.raw_fd(),
        terminals: frozen.handoffs().to_vec(),
        runs: Vec::new(),
    };
    let manifest_fd = manifest.write_anon().unwrap();
    say("EXEC");
    let error = exec_into(
        &std::env::current_exe().unwrap(),
        manifest_fd.as_raw_fd(),
        1,
    );
    say(format!("EXECFAILED {error}"));
    std::process::exit(4);
}

/// The successor as production runs it: `Engine::run_adopting`. Its assembly
/// fails (see [`REAL_ENGINE`]), so it rolls back — until the loop guard stops it.
async fn successor_real_engine(dir: &Path, adoption: handoff::Adoption) {
    say(format!(
        "BOOT pid={} attempt={}",
        std::process::id(),
        adoption.attempt
    ));
    // Hold here until the parent has looked at the state this boot inherited
    // (every failed boot before it must have left the shell and the lock
    // alone); the parent's check is then deterministic, not a race with a
    // child that fails in milliseconds.
    let mut ack = String::new();
    std::io::stdin().read_line(&mut ack).unwrap();
    let config = zeron_engine::EngineConfig {
        data_dir: dir.to_path_buf(),
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        ipc_port: 0,
        default_harness: zeron_engine::HarnessId::Mock,
        org_id: None,
        workos_client_id: Some("client_test".into()),
    };
    let result = zeron_engine::Engine::new(config)
        .run_adopting(adoption)
        .await;
    say(format!(
        "GAVEUP {}",
        result.err().map(|e| e.to_string()).unwrap_or_default()
    ));
    std::process::exit(6);
}

async fn successor(dir: &Path, adoption: handoff::Adoption) {
    if std::env::var_os(REAL_ENGINE).is_some() {
        return successor_real_engine(dir, adoption).await;
    }
    let attempt = adoption.attempt;
    say(format!(
        "BOOT pid={} attempt={attempt} rolled_back_from={}",
        std::process::id(),
        adoption.rolled_back_from.as_deref().unwrap_or("-")
    ));
    adoption.secure_originals();
    let lock = InstanceLock::adopt(adoption.manifest.lock_fd, dir).unwrap();
    let terminals = Terminals::adopt(adoption.manifest.terminals.clone()).unwrap();
    let listener = handoff::listener_from_inherited(adoption.manifest.listener_fd).unwrap();

    if std::env::var_os(FAIL_FIRST_BOOT).is_some() && attempt == 1 {
        say("FAILING");
        // Everything above was done on duplicates; the originals are still
        // there for the predecessor.
        handoff::roll_back(adoption, &anyhow::anyhow!("simulated boot failure"));
        say("ROLLBACKFAILED");
        std::process::exit(5);
    }

    // What `EngineCore::commit_adoption` does in production.
    lock.arm();
    terminals.arm();
    let terminal = adoption.manifest.terminals[0].id.clone();
    adoption.commit();
    say(format!("NEW pid={} attempt={attempt}", std::process::id()));

    // The terminal carries on in the same shell.
    terminals
        .write_bytes(&terminal, b"echo post-$((1+1)); echo shellpid-$$\n")
        .unwrap();
    // (The earlier `shellpid-<n>` line is in the replay window too: take the
    // LAST one, which is this shell's answer after the handoff.)
    let text = output_until(&terminals, &terminal, |text| {
        text.contains("post-2\r\n") && text.matches("shellpid-").count() >= 3
    })
    .await;
    let last = text.rfind("shellpid-").unwrap();
    say(format!(
        "SHELL post_ok={} pid={}",
        text.contains("post-2\r\n"),
        number_after(&text[last..], "shellpid-").unwrap_or(0)
    ));

    // Children of the successor must not inherit what was handed over.
    let listing = Command::new("ls")
        .arg(if cfg!(target_os = "macos") {
            "/dev/fd"
        } else {
            "/proc/self/fd"
        })
        .output()
        .unwrap();
    let leaked: Vec<u32> = String::from_utf8_lossy(&listing.stdout)
        .split_whitespace()
        .filter_map(|entry| entry.parse().ok())
        .filter(|fd| *fd > 3) // 0-2 are ours; 3 is the directory `ls` is reading
        .collect();
    say(format!("LEAKED {leaked:?}"));

    // The client that connected while no image was accepting is served here.
    // Anything on this machine may probe a fresh local listener (the Zeron
    // desktop app's preview discovery does exactly that), so identify the
    // parent's client by what it sends and ignore the rest.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (mut connection, peer) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .expect("the parent's client was never accepted")
            .unwrap();
        let mut hello = String::new();
        let read = tokio::time::timeout(
            Duration::from_secs(2),
            tokio::io::AsyncBufReadExt::read_line(
                &mut tokio::io::BufReader::new(&mut connection),
                &mut hello,
            ),
        )
        .await;
        if matches!(read, Ok(Ok(n)) if n > 0) && hello.trim() == "hello-from-parent" {
            tokio::io::AsyncWriteExt::write_all(&mut connection, b"welcome-from-successor\n")
                .await
                .unwrap();
            say("SERVED");
            break;
        }
        say(format!("IGNORED foreign connection from {peer}"));
    }

    // Live until the parent is done.
    let mut rest = String::new();
    let _ = std::io::stdin().read_to_string(&mut rest);
    terminals.shutdown();
}

// ── the parent: watches from outside ──

struct Child_ {
    child: Child,
    lines: mpsc::Receiver<String>,
}

fn spawn_engine(dir: &Path, fail_first_boot: bool) -> Child_ {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "engine_child", "--nocapture", "--test-threads=1"])
        .env(ROLE, "1")
        .env(DIR, dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if fail_first_boot {
        command.env(FAIL_FIRST_BOOT, "1");
    }
    let mut child = command.spawn().unwrap();
    let stdout: ChildStdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            // libtest prints "test engine_child ... " on the same line as the
            // first message, so the marker is not always at the start.
            if let Some(at) = line.find("T:") {
                eprintln!("[engine child] {}", &line[at + 2..]);
                let _ = tx.send(line[at + 2..].to_string());
            }
        }
    });
    Child_ { child, lines }
}

impl Child_ {
    /// The next protocol line starting with `prefix`.
    fn expect(&self, prefix: &str) -> String {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("timed out waiting for {prefix:?}"));
            if line.starts_with(prefix) {
                return line;
            }
            assert!(
                !line.starts_with("EXECFAILED")
                    && !line.starts_with("ROLLBACKFAILED")
                    && !line.starts_with("BADMANIFEST"),
                "engine child failed: {line}"
            );
        }
    }

    fn go(&mut self) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "go").unwrap();
        stdin.flush().unwrap();
    }

    fn finish(mut self) {
        drop(self.child.stdin.take()); // EOF: the successor shuts down
        let status = self.child.wait().unwrap();
        assert!(status.success(), "engine child exited with {status}");
    }
}

fn field(line: &str, key: &str) -> u32 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no {key} in {line:?}"))
        .parse()
        .unwrap()
}

/// The process state letters of `pid` (`ps -o stat=`): `Z…` is a zombie.
fn state_of(pid: u32) -> String {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// The parent pid of `pid`, from `ps` (portable across Linux and macOS).
fn parent_of(pid: u32) -> u32 {
    let output = Command::new("ps")
        .args(["-o", "ppid=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap()
}

fn run_scenario(fail_first_boot: bool) {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = spawn_engine(dir.path(), fail_first_boot);
    let old = engine.expect("OLD ");
    let (pid, port, shell) = (
        field(&old, "pid"),
        field(&old, "port"),
        field(&old, "shell"),
    );
    assert!(
        InstanceLock::holder(dir.path()).is_some(),
        "the engine holds the data-dir lock"
    );

    engine.go();
    // A client that shows up while the engine is being replaced must be
    // queued, never refused — whichever image happens to own the socket.
    let mut refused = 0;
    let mut client = loop {
        match std::net::TcpStream::connect(("127.0.0.1", port as u16)) {
            Ok(stream) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                refused += 1;
                assert!(
                    refused < 3,
                    "the listener refused a client during the handoff"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("connect: {error}"),
        }
    };
    assert_eq!(refused, 0, "no connection is ever refused");
    client
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    client.write_all(b"hello-from-parent\n").unwrap();

    engine.expect("EXEC");
    let booted = engine.expect("BOOT ");
    assert_eq!(field(&booted, "pid"), pid, "exec keeps the PID");
    assert_eq!(field(&booted, "attempt"), 1);
    assert!(
        booted.contains("rolled_back_from=-"),
        "a first adoption is not a rollback: {booted}"
    );
    if fail_first_boot {
        engine.expect("FAILING");
        assert!(
            InstanceLock::holder(dir.path()).is_some(),
            "the lock is held through a failed adoption"
        );
        let again = engine.expect("BOOT ");
        assert_eq!(field(&again, "pid"), pid, "the rollback keeps the PID too");
        assert_eq!(
            field(&again, "attempt"),
            2,
            "the predecessor adopted on attempt 2"
        );
        assert!(
            again.contains(&format!(
                "rolled_back_from={}",
                zeron_update::current_version()
            )),
            "the predecessor is told which version failed: {again}"
        );
    }
    let new = engine.expect("NEW ");
    assert_eq!(field(&new, "pid"), pid);
    assert!(
        InstanceLock::holder(dir.path()).is_some(),
        "the lock was never released"
    );

    let shell_line = engine.expect("SHELL ");
    assert_eq!(
        field(&shell_line, "pid"),
        shell,
        "the same shell process kept running"
    );
    assert!(shell_line.contains("post_ok=true"));
    assert_eq!(
        parent_of(shell),
        pid,
        "the shell is still the engine's child"
    );
    assert_eq!(
        engine.expect("LEAKED"),
        "LEAKED []",
        "the successor's children inherit nothing that was handed over"
    );

    let mut greeting = String::new();
    BufReader::new(&mut client)
        .read_line(&mut greeting)
        .unwrap();
    assert_eq!(
        greeting.trim(),
        "welcome-from-successor",
        "the client queued during the gap was served by the successor"
    );
    engine.expect("SERVED");
    engine.finish();
    assert!(
        InstanceLock::acquire(dir.path()).is_ok(),
        "the lock is released when the engine exits"
    );
}

/// The production rollback path, end to end: a successor whose engine assembly
/// FAILS after it adopted the terminals must leave the shells running, hand
/// back, and — after the loop guard's three tries — give up without ever having
/// hung a shell up in between.
#[test]
fn a_failed_engine_boot_never_hangs_up_the_adopted_shells() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "engine_child", "--nocapture", "--test-threads=1"])
        .env(ROLE, "1")
        .env(DIR, dir.path())
        .env(REAL_ENGINE, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(at) = line.find("T:") {
                eprintln!("[engine child] {}", &line[at + 2..]);
                let _ = tx.send(line[at + 2..].to_string());
            }
        }
    });
    let mut engine = Child_ { child, lines };
    let old = engine.expect("OLD ");
    let (pid, shell) = (field(&old, "pid"), field(&old, "shell"));
    engine.go();
    engine.expect("EXEC");
    for attempt in 1..=3 {
        let booted = engine.expect("BOOT ");
        assert_eq!(field(&booted, "pid"), pid, "every image keeps the PID");
        assert_eq!(field(&booted, "attempt"), attempt);
        // The point: however many boots have already failed, the shell is
        // still running and still the engine's child.
        assert!(
            unsafe { libc::kill(shell as libc::pid_t, 0) } == 0,
            "attempt {attempt}: the shell was hung up by an earlier failed boot"
        );
        assert_eq!(parent_of(shell), pid);
        // A hung-up shell that nobody has reaped yet is a zombie: it still
        // answers `kill(pid, 0)` and still has a parent, so look at its state.
        assert!(
            !state_of(shell).starts_with('Z'),
            "attempt {attempt}: the shell is a zombie — an earlier failed boot hung it up"
        );
        assert!(
            InstanceLock::holder(dir.path()).is_some(),
            "attempt {attempt}: the lock was released by an earlier failed boot"
        );
        engine.go(); // let this boot proceed (and fail)
    }
    let gave_up = engine.expect("GAVEUP");
    // It must be the failure this test arranges — assembly failing AFTER the
    // terminals were adopted — or it no longer covers the unarmed path.
    assert!(
        gave_up.contains("Is a directory"),
        "the boot failed for a different reason than the test arranges: {gave_up}"
    );
    let status = engine.child.wait().unwrap();
    assert_eq!(
        status.code(),
        Some(6),
        "the engine stops after the third failure"
    );
}

#[test]
fn a_handoff_keeps_the_pid_the_shell_the_lock_and_the_queued_client() {
    run_scenario(false);
}

#[test]
fn a_successor_that_cannot_boot_hands_back_and_the_predecessor_adopts_the_same_manifest() {
    run_scenario(true);
}

#[test]
fn a_garbled_handoff_environment_is_refused_not_guessed_at() {
    let dir = tempfile::tempdir().unwrap();
    for fd in ["", "abc", "0", "-1", "12345"] {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "engine_child", "--nocapture", "--test-threads=1"])
            .env(ROLE, "1")
            .env(DIR, dir.path())
            .env(handoff::HANDOFF_FD_ENV, fd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("T:BADMANIFEST"), "fd {fd:?}: {stdout}");
        assert_eq!(output.status.code(), Some(3), "fd {fd:?}");
    }
}
