//! Claude runs across a live update: freeze a running `claude` stream-json
//! session, hand it over, and adopt it — against the fake CLI in
//! `tests/fixtures/fake_claude_handoff.py` (one real child process, real pipes).
//!
//! A "commit" here stands in for the engine's `execve`: the old run gives up
//! its child and pipes without closing them and a successor in this same
//! process adopts them. The exec path itself never commits; see
//! `a_run_task_dropped_while_frozen_leaves_the_child_running_and_adoptable`.

#![cfg(unix)]

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{
    CancellationToken, ClaudeHarness, FreezeRefusal, FreezeRequest, FrozenRun, Harness,
    HarnessError, HarnessHandoff, RunControls, SteerMessage,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, UserInputAnswer, UserInputQuestion,
};

type Events = BoxStream<'static, Result<AgentEvent, HarnessError>>;
type Answer = oneshot::Sender<Vec<UserInputAnswer>>;

const WAIT: Duration = Duration::from_secs(15);

fn fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_claude_handoff.py");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn harness() -> ClaudeHarness {
    ClaudeHarness::new()
        .with_executable(fixture())
        .with_graces(Duration::from_secs(2), Duration::from_secs(2))
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
    /// Every `request_input` call: the questions and the answer slot.
    questions: mpsc::UnboundedReceiver<(Vec<UserInputQuestion>, Answer)>,
    /// Every `rebind_input` call: the key and the answer slot.
    rebinds: mpsc::UnboundedReceiver<(String, Answer)>,
}

fn controls() -> (RunControls, Rig) {
    let (steer, steering) = mpsc::channel(32);
    let (freeze_tx, freeze) = mpsc::channel(1);
    let interrupt = CancellationToken::new();
    let (question_tx, questions) = mpsc::unbounded_channel();
    let (rebind_tx, rebinds) = mpsc::unbounded_channel();
    let controls = RunControls {
        execution_lease: None,
        request_input: Box::new(move |questions| {
            let (tx, rx) = oneshot::channel();
            let _ = question_tx.send((questions, tx));
            rx
        }),
        steering,
        interrupt: interrupt.clone(),
        freeze,
        rebind_input: Box::new(move |key| {
            let (tx, rx) = oneshot::channel();
            let _ = rebind_tx.send((key, tx));
            rx
        }),
    };
    (
        controls,
        Rig {
            steer,
            freeze: freeze_tx,
            interrupt,
            questions,
            rebinds,
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
    {
        unsafe { libc::close(fd) };
    }
}

fn dup(fd: RawFd) -> RawFd {
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    assert!(copy >= 3, "dup {fd}");
    copy
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

#[tokio::test]
async fn freeze_at_a_line_boundary_then_adopt_continues_the_same_conversation() {
    let (c, rig) = controls();
    let mut run = harness().run(request("first"), c).await.unwrap();
    let seen = until_text(&mut run, "echo: first").await;
    assert_eq!(session_starts(&seen), 1);
    until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;

    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    assert_eq!(frozen.handoff.harness, HarnessId::ClaudeCode);
    assert!(frozen.handoff.stderr_fd.is_some(), "stderr is a pipe");
    assert!(
        frozen
            .handoff
            .stderr_tail
            .iter()
            .any(|l| l == "fake-claude: started"),
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
        "the steer's replay is recognised: {seen:?}"
    );
    let end = until(&mut adopted, |e| matches!(e, AgentEvent::Done { .. })).await;
    assert!(matches!(
        end.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            session_id: Some(id),
            ..
        }) if id == "sess-handoff"
    ));
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn a_partial_stdout_line_at_freeze_time_is_not_lost() {
    let (c, rig) = controls();
    let mut run = harness().run(request("partial"), c).await.unwrap();
    until_text(&mut run, "part-start").await;
    // The first half of the next frame is on the pipe now; let the reader
    // take it off.
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
    until_text(&mut adopted, "partial-done").await;
    until(&mut adopted, |e| matches!(e, AgentEvent::Done { .. })).await;
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn parked_ask_user_question_survives_and_answers_with_the_original_ids() {
    let (c, mut rig) = controls();
    let mut run = harness().run(request("ask"), c).await.unwrap();
    until_text(&mut run, "asking").await;
    let (questions, _old_answer) = tokio::time::timeout(WAIT, rig.questions.recv())
        .await
        .unwrap()
        .expect("asked");
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].question, "Pick one");
    let frozen = freeze(&rig)
        .await
        .expect("a parked question is a safe point");
    let handoff = commit(frozen, &mut run).await;

    let (c2, mut rig2) = controls();
    let mut adopted = harness()
        .adopt(handoff.clone(), c2, request("ask"))
        .await
        .unwrap();
    close_originals(&handoff);
    let (key, answer) = tokio::time::timeout(WAIT, rig2.rebinds.recv())
        .await
        .unwrap()
        .expect("the parked question is rebound");
    assert_eq!(
        key, questions[0].id,
        "rebound under the original question id"
    );
    answer
        .send(vec![UserInputAnswer {
            question_id: questions[0].id.clone(),
            labels: vec!["B".into()],
        }])
        .unwrap();
    until_text(&mut adopted, "answered: B").await;
    until(&mut adopted, |e| matches!(e, AgentEvent::Done { .. })).await;
    assert!(
        rig2.questions.try_recv().is_err(),
        "no second prompt for the same question"
    );
    finish(rig2, &mut adopted, handoff.pid).await;
}

/// The user answered a moment before the freeze: the answer must reach the CLI
/// (written ahead of the pause), not be exported as a question still parked —
/// the engine no longer holds it, so the adopter could only answer with nothing.
#[tokio::test]
async fn an_answer_given_just_before_the_freeze_is_written_not_left_parked() {
    let (c, mut rig) = controls();
    let mut run = harness().run(request("ask"), c).await.unwrap();
    until_text(&mut run, "asking").await;
    let (questions, answer) = tokio::time::timeout(WAIT, rig.questions.recv())
        .await
        .unwrap()
        .expect("asked");
    // Answer and freeze become ready in the same breath (no await between).
    answer
        .send(vec![UserInputAnswer {
            question_id: questions[0].id.clone(),
            labels: vec!["B".into()],
        }])
        .unwrap();
    let frozen = freeze(&rig).await.expect("safe point");
    let handoff = commit(frozen, &mut run).await;

    let (c2, mut rig2) = controls();
    let mut adopted = harness()
        .adopt(handoff.clone(), c2, request("ask"))
        .await
        .unwrap();
    close_originals(&handoff);
    // The CLI already has the real answer; nothing is left to rebind.
    until_text(&mut adopted, "answered: B").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), rig2.rebinds.recv())
            .await
            .is_err(),
        "an answered question must not be rebound"
    );
    until(&mut adopted, |e| matches!(e, AgentEvent::Done { .. })).await;
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn freeze_is_busy_while_interrupting_or_holding_done() {
    let (c, rig) = controls();
    let mut run = harness().run(request("absorb-next"), c).await.unwrap();
    until(&mut run, |e| matches!(e, AgentEvent::Done { .. })).await;
    // This steer is answered by a bare result and never replayed: the turn
    // end is held back waiting for it.
    rig.steer.send(steer("swallowed")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("held done")
    );
    rig.interrupt.cancel();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("interrupting")
    );
    let tail = drain(&mut run).await;
    assert!(
        matches!(
            tail.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Interrupted,
                ..
            })
        ),
        "{tail:?}"
    );
}

#[tokio::test]
async fn a_thawed_run_resumes_with_its_drained_steers_first_and_in_order() {
    let (c, rig) = controls();
    let mut run = harness().run(request("hang"), c).await.unwrap();
    until_text(&mut run, "hanging").await;
    let prompts = |frozen: &FrozenRun| -> Vec<String> {
        frozen
            .handoff
            .undrained_steers
            .iter()
            .map(|s| s.prompt.clone())
            .collect()
    };
    // Steers that reach the mailbox while the run is frozen stay there...
    let frozen = freeze(&rig).await.expect("mid-turn at a line boundary");
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
    until_text(&mut run, "echo: one").await;
    until_text(&mut run, "echo: two").await;
    until_text(&mut run, "echo: three").await;
    drop(rig);
    drain(&mut run).await;
}

#[test]
fn a_run_task_dropped_while_frozen_leaves_the_child_running_and_adoptable() {
    // Stands in for the exec path: the engine never commits, the old image
    // just stops existing. Dropping the runtime drops the run task and its
    // tokio `Child`; the dups keep the pipes open as the exec'd image's
    // inherited descriptors would.
    let old = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (frozen, inherited) = old.block_on(async {
        let (c, rig) = controls();
        let mut run = harness().run(request("first"), c).await.unwrap();
        until_text(&mut run, "echo: first").await;
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
    assert!(
        alive(inherited.pid),
        "a dropped run must not kill its child"
    );

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
async fn adopted_child_that_died_in_the_gap_ends_the_run_with_its_real_exit_status() {
    let (c, rig) = controls();
    let mut run = harness().run(request("hang"), c).await.unwrap();
    until_text(&mut run, "hanging").await;
    let frozen = freeze(&rig).await.expect("mid-turn at a line boundary");
    let handoff = commit(frozen, &mut run).await;
    // The agent dies between the images.
    let line = b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"die 7\"}}\n";
    let written = unsafe { libc::write(handoff.stdin_fd, line.as_ptr().cast(), line.len()) };
    assert_eq!(written, line.len() as isize);
    let deadline = std::time::Instant::now() + WAIT;
    while alive(handoff.pid) {
        assert!(std::time::Instant::now() < deadline, "the agent never died");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (c2, rig2) = controls();
    let mut adopted = harness()
        .adopt(handoff.clone(), c2, request("hang"))
        .await
        .unwrap();
    close_originals(&handoff);
    let tail = drain(&mut adopted).await;
    let Some(AgentEvent::Done {
        status: DoneStatus::Errored,
        error: Some(error),
        ..
    }) = tail.last()
    else {
        panic!("the run ends with a crash: {tail:?}");
    };
    assert!(error.contains("exit code 7"), "{error}");
    assert!(
        error.contains("fake-claude: started"),
        "seeded from the exported tail: {error}"
    );
    assert!(error.contains("dying"), "read after adoption: {error}");
    drop(rig2);
}

#[tokio::test]
async fn an_unknown_state_version_is_refused_so_the_engine_falls_back() {
    let handoff = HarnessHandoff {
        harness: HarnessId::ClaudeCode,
        state_version: 9999,
        pid: 4242,
        stdin_fd: 100,
        stdout_fd: 101,
        stderr_fd: None,
        stdout_leftover: Vec::new(),
        stderr_tail: Vec::new(),
        state: serde_json::json!({}),
        extra_fds: Vec::new(),
        undrained_steers: Vec::new(),
    };
    let (c, _rig) = controls();
    let Err(err) = harness().adopt(handoff, c, request("x")).await else {
        panic!("adopted an unknown state version");
    };
    assert!(matches!(err, HarnessError::Protocol(_)), "{err}");
    assert!(harness().supports_adoption());
}
