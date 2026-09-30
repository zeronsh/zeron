//! Cursor runs across a live update: freeze a running shim session, hand it
//! over, and adopt it — against `tests/fixtures/fake_cursor_handoff.py` (one real
//! child process, real pipes). A "commit" stands in for the engine's `execve`:
//! the old run gives up its child and pipes without closing them and a successor
//! in this same process adopts them.

#![cfg(unix)]

use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{
    CancellationToken, CursorHarness, FreezeRefusal, FreezeRequest, FrozenRun, Harness,
    HarnessError, HarnessHandoff, RunControls, SteerMessage,
};
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel};

type Events = BoxStream<'static, Result<AgentEvent, HarnessError>>;

const WAIT: Duration = Duration::from_secs(15);

fn fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_cursor_handoff.py");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn harness() -> CursorHarness {
    CursorHarness::new()
        .with_executable(fixture())
        .with_graces(Duration::from_secs(5), Duration::from_secs(2))
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: String::new(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

/// The engine's side of one run's controls.
struct Rig {
    steer: mpsc::Sender<SteerMessage>,
    freeze: mpsc::Sender<FreezeRequest>,
    interrupt: CancellationToken,
}

fn controls() -> (RunControls, Rig) {
    let (steer, steering) = mpsc::channel(32);
    let (freeze_tx, freeze) = mpsc::channel(1);
    let interrupt = CancellationToken::new();
    let controls = RunControls {
        execution_lease: None,
        request_input: Box::new(|_| oneshot::channel().1),
        steering,
        interrupt: interrupt.clone(),
        freeze,
        rebind_input: Box::new(|_| oneshot::channel().1),
    };
    (
        controls,
        Rig {
            steer,
            freeze: freeze_tx,
            interrupt,
        },
    )
}

fn steer(prompt: &str) -> SteerMessage {
    SteerMessage {
        prompt: prompt.into(),
        message_id: Some(format!("msg-{prompt}")),
    }
}

async fn next(stream: &mut Events, seen: &[AgentEvent]) -> Option<AgentEvent> {
    tokio::time::timeout(WAIT, stream.next())
        .await
        .unwrap_or_else(|_| panic!("the run went quiet; seen so far: {seen:?}"))
        .map(|event| event.expect("no stream error"))
}

/// Events up to and including the text delta `want`.
async fn until_text(stream: &mut Events, want: &str) -> Vec<AgentEvent> {
    until(
        stream,
        |e| matches!(e, AgentEvent::TextDelta { text } if text == want),
    )
    .await
}

async fn until(stream: &mut Events, hit: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    loop {
        let Some(event) = next(stream, &seen).await else {
            panic!("the stream ended first; seen: {seen:?}");
        };
        let done = hit(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

/// Every remaining event, until the stream ends.
async fn drain(stream: &mut Events) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    while let Some(event) = next(stream, &seen).await {
        seen.push(event);
    }
    seen
}

async fn freeze(rig: &Rig) -> Result<FrozenRun, FreezeRefusal> {
    let (reply, rx) = oneshot::channel();
    rig.freeze
        .send(FreezeRequest { reply })
        .await
        .expect("the run takes freeze requests");
    tokio::time::timeout(WAIT, rx)
        .await
        .expect("the run answers a freeze")
        .expect("the run replies")
}

/// Commit a freeze (a same-process successor) and check the old run let go.
async fn commit(frozen: FrozenRun, old: &mut Events) -> HarnessHandoff {
    let handoff = frozen.handoff.clone();
    frozen.commit.send(()).expect("the run awaits the verdict");
    let tail = drain(old).await;
    assert!(
        !tail.iter().any(|e| matches!(e, AgentEvent::Done { .. })),
        "a committed run ends its stream without a Done: {tail:?}"
    );
    assert!(alive(handoff.pid), "the child is handed over, not killed");
    handoff
}

fn alive(pid: i32) -> bool {
    // Reaped children are gone; a zombie still answers kill(0), so also ask
    // whether it has exited.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    let exited = rc == 0 && unsafe { info.si_pid() } == pid;
    let signalable = unsafe { libc::kill(pid, 0) } == 0;
    signalable && !exited
}

/// The engine closes the inherited originals once an adoption commits.
fn close_originals(handoff: &HarnessHandoff) {
    for fd in [handoff.stdin_fd, handoff.stdout_fd]
        .into_iter()
        .chain(handoff.stderr_fd)
        .chain(handoff.extra_fds.iter().copied())
    {
        unsafe { libc::close(fd) };
    }
}

fn session_starts(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        .count()
}

/// End an adopted run the ordinary way: its mailbox closes, the CLI sees
/// stdin EOF and exits, and the run reaps it.
async fn finish(rig: Rig, stream: &mut Events, pid: i32) -> Vec<AgentEvent> {
    drop(rig);
    let tail = drain(stream).await;
    assert!(!alive(pid), "the adopter reaped the child");
    tail
}

fn text_of(events: &[AgentEvent], prefix: &str) -> Option<String> {
    events.iter().find_map(|e| match e {
        AgentEvent::TextDelta { text } if text.starts_with(prefix) => Some(text.clone()),
        _ => None,
    })
}

#[tokio::test]
async fn cursor_adopt_continues_streaming_and_steering() {
    let (c, rig) = controls();
    let mut run = harness().run(request("first"), c).await.unwrap();
    let seen = until_text(&mut run, "echo: first").await;
    assert_eq!(session_starts(&seen), 1);
    until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;

    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    assert_eq!(frozen.handoff.harness, HarnessId::Cursor);
    assert!(frozen.handoff.stderr_fd.is_some(), "stderr is a pipe");
    assert!(
        frozen
            .handoff
            .stderr_tail
            .iter()
            .any(|l| l == "fake-cursor: started"),
        "{:?}",
        frozen.handoff.stderr_tail
    );
    let handoff = commit(frozen, &mut run).await;

    let (c2, rig2) = controls();
    let mut adopted = harness()
        .adopt(handoff.clone(), c2, request("first"))
        .await
        .expect("adopts");
    close_originals(&handoff);
    rig2.steer.send(steer("second")).await.unwrap();
    let seen = until_text(&mut adopted, "echo: second").await;
    assert_eq!(
        session_starts(&seen),
        0,
        "no second SessionStarted: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| matches!(e, AgentEvent::Steered { .. })),
        "the steer is recognised after adoption: {seen:?}"
    );
    let end = until(&mut adopted, |e| matches!(e, AgentEvent::Done { .. })).await;
    assert!(matches!(
        end.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            session_id: Some(id),
            ..
        }) if id == "agent-handoff"
    ));
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn cursor_shim_keeps_answering_with_the_same_parent_after_a_handoff() {
    // The real shim exits when its parent pid changes; exec keeps the engine's
    // pid, so it never notices. The fake exits with 9 in that case.
    let (c, rig) = controls();
    let mut run = harness().run(request("first"), c).await.unwrap();
    let seen = until_text(&mut run, "echo: first").await;
    let before = match text_of(&seen, "ppid=") {
        Some(text) => text,
        None => {
            let more = until(
                &mut run,
                |e| matches!(e, AgentEvent::TextDelta { text } if text.starts_with("ppid=")),
            )
            .await;
            text_of(&more, "ppid=").unwrap()
        }
    };
    assert_eq!(before, format!("ppid={}", std::process::id()));
    until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;
    let frozen = freeze(&rig).await.expect("safe point");
    let handoff = commit(frozen, &mut run).await;

    let (c2, rig2) = controls();
    let mut adopted = harness()
        .adopt(handoff.clone(), c2, request("first"))
        .await
        .unwrap();
    close_originals(&handoff);
    rig2.steer.send(steer("again")).await.unwrap();
    let seen = until(
        &mut adopted,
        |e| matches!(e, AgentEvent::TextDelta { text } if text.starts_with("ppid=")),
    )
    .await;
    assert_eq!(
        text_of(&seen, "ppid=").unwrap(),
        before,
        "the shim's parent is unchanged across the handoff"
    );
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn cursor_freeze_is_busy_while_interrupted() {
    let (c, rig) = controls();
    let mut run = harness().run(request("first"), c).await.unwrap();
    until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;
    rig.interrupt.cancel();
    // Let the loop take the interrupt (the fake takes seconds to wind down).
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("interrupting")
    );
    drain(&mut run).await;
}

#[tokio::test]
async fn a_thawed_cursor_run_resumes_with_its_drained_steers_first_and_in_order() {
    let (c, rig) = controls();
    let mut run = harness().run(request("first"), c).await.unwrap();
    until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;
    let prompts = |frozen: &FrozenRun| -> Vec<String> {
        frozen
            .handoff
            .undrained_steers
            .iter()
            .map(|s| s.prompt.clone())
            .collect()
    };
    // Steers that reach the mailbox while the run is frozen stay there...
    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    assert!(prompts(&frozen).is_empty());
    rig.steer.send(steer("one")).await.unwrap();
    rig.steer.send(steer("two")).await.unwrap();
    drop(frozen); // a failed exec: the run resumes where it was
    // ...and the next freeze drains them.
    let frozen = freeze(&rig).await.expect("still a safe point");
    assert_eq!(prompts(&frozen), ["one", "two"]);
    rig.steer.send(steer("three")).await.unwrap();
    drop(frozen);
    // Drained steers a thaw put back come first, then the mailbox.
    let frozen = freeze(&rig).await.expect("still a safe point");
    assert_eq!(prompts(&frozen), ["one", "two", "three"]);
    drop(frozen);
    let seen = until_text(&mut run, "echo: three").await;
    let order: Vec<&str> = seen
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } if text.starts_with("echo: ") => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["echo: one", "echo: two", "echo: three"], "{seen:?}");
    drop(rig);
    drain(&mut run).await;
}

/// The store lease rides the handoff as an extra descriptor and stays locked
/// through adoption; the conversation is free again only when the adopter ends.
#[tokio::test]
async fn the_conversation_store_stays_locked_through_a_handoff() {
    let root = tempfile::tempdir().unwrap();
    let leased = || harness().with_lease_root(root.path());
    let locked = |store: &Path| {
        let probe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.join(".zeron-owner.lock"))
            .unwrap();
        matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock))
    };
    let (c, rig) = controls();
    let mut run = leased().run(request("first"), c).await.unwrap();
    until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;
    let frozen = freeze(&rig).await.expect("safe point");
    assert_eq!(frozen.handoff.extra_fds.len(), 1, "the lease rides along");
    assert_eq!(frozen.handoff.state["hasLease"], true);
    let store = PathBuf::from(frozen.handoff.state["storeDir"].as_str().unwrap());
    assert!(locked(&store));
    let handoff = commit(frozen, &mut run).await;
    assert!(locked(&store), "still locked between the images");

    let (c2, rig2) = controls();
    let mut adopted = leased()
        .adopt(handoff.clone(), c2, request("first"))
        .await
        .unwrap();
    // The engine closes the inherited originals once the adoption commits: the
    // adopter's duplicate keeps the lock.
    close_originals(&handoff);
    assert!(locked(&store), "the adopter holds the lease");
    rig2.steer.send(steer("more")).await.unwrap();
    until_text(&mut adopted, "echo: more").await;
    finish(rig2, &mut adopted, handoff.pid).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!locked(&store), "released when the adopted run ends");
}

#[test]
fn a_run_task_dropped_while_frozen_leaves_the_shim_running_and_adoptable() {
    // Stands in for the exec path: the engine never commits, the old image just
    // stops existing. Dropping the runtime drops the run task and its tokio
    // `Child`; the dups keep the pipes open as the exec'd image's inherited
    // descriptors would.
    let dup = |fd: RawFd| {
        // SAFETY: fcntl on a plain descriptor number.
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        assert!(copy >= 3, "dup {fd}");
        copy
    };
    let old = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (frozen, inherited) = old.block_on(async {
        let (c, rig) = controls();
        let mut run = harness().run(request("first"), c).await.unwrap();
        until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;
        let frozen = freeze(&rig).await.expect("safe point");
        let mut inherited = frozen.handoff.clone();
        inherited.stdin_fd = dup(inherited.stdin_fd);
        inherited.stdout_fd = dup(inherited.stdout_fd);
        inherited.stderr_fd = inherited.stderr_fd.map(dup);
        // The old image's engine side: gone with the runtime.
        std::mem::forget(rig);
        std::mem::forget(run);
        (frozen, inherited)
    });
    drop(old);
    drop(frozen);
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(inherited.pid), "a dropped run must not kill its shim");

    let new = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    new.block_on(async {
        let (c, rig) = controls();
        let mut adopted = harness()
            .adopt(inherited.clone(), c, request("first"))
            .await
            .unwrap();
        close_originals(&inherited);
        rig.steer.send(steer("after exec")).await.unwrap();
        until_text(&mut adopted, "echo: after exec").await;
        drop(rig);
        drain(&mut adopted).await;
    });
}

#[tokio::test]
async fn a_partial_stdout_line_at_freeze_time_is_not_lost() {
    let (c, rig) = controls();
    let mut run = harness().run(request("partial"), c).await.unwrap();
    until_text(&mut run, "part-start").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let frozen = freeze(&rig).await.expect("mid-turn at a line boundary");
    let leftover = frozen.handoff.stdout_leftover.clone();
    assert!(
        !leftover.is_empty(),
        "half a line was read before the freeze"
    );
    assert!(!leftover.contains(&b'\n'), "and only half a line");
    let handoff = commit(frozen, &mut run).await;

    let (c2, rig2) = controls();
    let mut adopted = harness()
        .adopt(handoff.clone(), c2, request("partial"))
        .await
        .unwrap();
    close_originals(&handoff);
    until_text(&mut adopted, "part-end").await;
    until(&mut adopted, |e| matches!(e, AgentEvent::Done { .. })).await;
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn an_unknown_state_version_is_refused_so_the_engine_falls_back() {
    let handoff = HarnessHandoff {
        harness: HarnessId::Cursor,
        state_version: 9999,
        pid: 4242,
        stdin_fd: 5,
        stdout_fd: 6,
        stderr_fd: None,
        extra_fds: Vec::new(),
        stdout_leftover: Vec::new(),
        stderr_tail: Vec::new(),
        state: serde_json::json!({}),
        undrained_steers: Vec::new(),
    };
    let (c, _rig) = controls();
    let Err(error) = harness().adopt(handoff, c, request("x")).await else {
        panic!("an unknown state version must be refused");
    };
    assert!(matches!(error, HarnessError::Protocol(_)), "{error}");
}
