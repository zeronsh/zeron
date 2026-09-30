//! ACP runs across a live update: freeze a running ACP agent session, hand it
//! over, and adopt it — against the fake agent in
//! `tests/fixtures/fake_acp_handoff.py` (one real child process, real pipes,
//! one session for its whole life). The agents share one loop, so the tests
//! use whichever spec exercises the path: Devin (no prompt-complete
//! extension, so a prompt ends only by its RPC response), Grok (subagent
//! tails, `prompt_complete`) and Antigravity (the system-message echo filter).
//!
//! A "commit" here stands in for the engine's `execve`: the old run gives up
//! its child and pipes without closing them and a successor in this same
//! process adopts them. The exec path itself never commits; see
//! `a_run_task_dropped_while_frozen_leaves_the_child_running_and_adoptable`.

#![cfg(target_os = "linux")]

use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{
    AcpHarness, CancellationToken, FreezeRefusal, FreezeRequest, FrozenRun, Harness, HarnessError,
    HarnessHandoff, RunControls, SteerMessage,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, UserInputAnswer, UserInputQuestion,
};

type Events = BoxStream<'static, Result<AgentEvent, HarnessError>>;
type Answer = oneshot::Sender<Vec<UserInputAnswer>>;

const WAIT: Duration = Duration::from_secs(15);
const SESSION: &str = "acp-handoff";

fn fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_acp_handoff.py");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn graces(harness: AcpHarness) -> AcpHarness {
    harness
        .with_executable(fixture())
        .with_graces(Duration::from_secs(1), Duration::from_secs(1))
}

/// No prompt-complete extension: a prompt ends only by its RPC response.
fn devin() -> AcpHarness {
    graces(AcpHarness::devin())
}

/// Subagent tails and the prompt-complete extension.
fn grok() -> AcpHarness {
    graces(AcpHarness::grok())
}

fn request(prompt: &str) -> RunRequest {
    request_in(prompt, "/tmp")
}

fn request_in(prompt: &str, cwd: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: cwd.into(),
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

fn is_text(event: &AgentEvent, want: &str) -> bool {
    matches!(event, AgentEvent::TextDelta { text } if text == want)
}

/// Events up to and including the text delta `want`.
async fn until_text(stream: &mut Events, want: &str) -> Vec<AgentEvent> {
    until(stream, |e| is_text(e, want)).await
}

/// Events up to and including a text delta starting with `prefix`.
async fn until_text_prefix(stream: &mut Events, prefix: &str) -> (String, Vec<AgentEvent>) {
    let seen = until(
        stream,
        |e| matches!(e, AgentEvent::TextDelta { text } if text.starts_with(prefix)),
    )
    .await;
    let Some(AgentEvent::TextDelta { text }) = seen.last() else {
        unreachable!()
    };
    (text[prefix.len()..].to_owned(), seen)
}

async fn until_done(stream: &mut Events) -> Vec<AgentEvent> {
    until(stream, |e| matches!(e, AgentEvent::Done { .. })).await
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
    assert!(
        !tail
            .iter()
            .any(|e| matches!(e, AgentEvent::Subagent { .. })),
        "a committed run's subagent tails end without settling: {tail:?}"
    );
    assert!(alive(handoff.pid), "the child is handed over, not killed");
    handoff
}

/// Running: neither gone nor exited (a zombie still answers kill(0)).
fn alive(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // The state follows the parenthesised command name.
        Ok(stat) => !matches!(
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().chars().next()),
            Some(Some('Z' | 'X'))
        ),
        Err(_) => false,
    }
}

async fn wait_dead(pid: i32) {
    let deadline = std::time::Instant::now() + WAIT;
    while alive(pid) {
        assert!(std::time::Instant::now() < deadline, "{pid} never died");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
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

/// The fake says protocol misuse (a second `initialize` or `session/new`, a
/// reused request id or prompt id, overlapping prompts) out loud.
fn assert_no_protocol_errors(events: &[AgentEvent]) {
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::TextDelta { text } if text.starts_with("PROTOCOL ERROR")
        )),
        "{events:?}"
    );
}

fn completed(events: &[AgentEvent]) -> bool {
    matches!(
        events.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            session_id: Some(id),
            ..
        }) if id == SESSION
    )
}

/// End an adopted run the ordinary way: its mailbox closes between turns,
/// and the run stops and reaps the agent.
async fn finish(rig: Rig, stream: &mut Events, pid: i32) -> Vec<AgentEvent> {
    drop(rig);
    let tail = drain(stream).await;
    assert!(!alive(pid), "the adopter reaped the child");
    tail
}

async fn adopt_with(harness: &AcpHarness, handoff: &HarnessHandoff, prompt: &str) -> (Events, Rig) {
    let (c, rig) = controls();
    let adopted = harness
        .adopt(handoff.clone(), c, request(prompt))
        .await
        .expect("adopts");
    close_originals(handoff);
    (adopted, rig)
}

fn in_flight_methods(handoff: &HarnessHandoff) -> Vec<String> {
    handoff.state["inFlight"]
        .as_array()
        .expect("the in-flight requests are exported")
        .iter()
        .map(|call| call["method"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn acp_adopt_continues_a_session_and_keeps_the_same_session_id() {
    let (c, rig) = controls();
    let mut run = devin().run(request("hang"), c).await.unwrap();
    let seen = until_text(&mut run, "hanging").await;
    assert_eq!(session_starts(&seen), 1);
    let frozen = freeze(&rig)
        .await
        .expect("a prompt in flight is a safe point at a line boundary");
    assert_eq!(frozen.handoff.harness, HarnessId::Devin);
    assert!(frozen.handoff.stderr_fd.is_some(), "stderr is a pipe");
    assert!(
        frozen
            .handoff
            .stderr_tail
            .iter()
            .any(|l| l == "fake-acp: started"),
        "{:?}",
        frozen.handoff.stderr_tail
    );
    assert_eq!(in_flight_methods(&frozen.handoff), ["session/prompt"]);
    let handoff = commit(frozen, &mut run).await;

    // The steer preempts the carried-over prompt: the agent answers it under
    // its ORIGINAL request id with `cancelled`, and only that response lets
    // the steer continue the session as the next prompt.
    let (mut adopted, rig2) = adopt_with(&devin(), &handoff, "hang").await;
    rig2.steer.send(steer("end")).await.unwrap();
    let seen = until_done(&mut adopted).await;
    assert!(
        seen.iter().any(|e| is_text(e, "echo: end")),
        "steered into the carried-over session: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| matches!(e, AgentEvent::Steered { .. })),
        "{seen:?}"
    );
    assert_eq!(session_starts(&seen), 0, "no second SessionStarted");
    assert_no_protocol_errors(&seen);
    assert!(completed(&seen), "{seen:?}");
    let tail = finish(rig2, &mut adopted, handoff.pid).await;
    assert_no_protocol_errors(&tail);
}

#[tokio::test]
async fn acp_freeze_mid_prompt_exports_the_pending_prompt_and_the_adopter_receives_its_response() {
    let (c, rig) = controls();
    let mut run = devin().run(request("slow"), c).await.unwrap();
    until_text(&mut run, "working").await;
    // Mid-prompt: the agent is about to answer it while nobody reads.
    let frozen = freeze(&rig)
        .await
        .expect("a prompt in flight is a safe point");
    assert_eq!(in_flight_methods(&frozen.handoff), ["session/prompt"]);
    let handoff = commit(frozen, &mut run).await;
    tokio::time::sleep(Duration::from_millis(1300)).await;

    // The response landed in the pipe between the images; the adopter must
    // deliver it to its loop by the original id to end the turn.
    let (mut adopted, rig2) = adopt_with(&devin(), &handoff, "slow").await;
    let seen = until_done(&mut adopted).await;
    assert!(seen.iter().any(|e| is_text(e, "finished")), "{seen:?}");
    assert!(completed(&seen), "{seen:?}");
    assert_no_protocol_errors(&seen);
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn a_steering_call_in_flight_is_carried_and_its_answer_reaches_the_adopter() {
    // The same fake, advertising the `_session/steering` extension.
    let dir = tempfile::tempdir().unwrap();
    let steering = dir.path().join("fake_acp_handoff_steering.py");
    std::os::unix::fs::symlink(fixture(), &steering).unwrap();
    let harness = AcpHarness::hermes()
        .with_executable(&steering)
        .with_graces(Duration::from_secs(1), Duration::from_secs(1));
    let (c, rig) = controls();
    let mut run = harness.run(request("hang"), c).await.unwrap();
    until_text(&mut run, "hanging").await;
    rig.steer.send(steer("mid-turn")).await.unwrap();
    // The agent answers the steering call 1 s later, while nobody reads.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let frozen = freeze(&rig).await.expect("calls in flight are carried");
    assert_eq!(
        in_flight_methods(&frozen.handoff),
        ["session/prompt", "_session/steering"]
    );
    let handoff = commit(frozen, &mut run).await;

    let (mut adopted, rig2) = adopt_with(&harness, &handoff, "hang").await;
    let seen = until_done(&mut adopted).await;
    // (Where the agent's own reply text falls around the Steered boundary
    // is decided by the loop's ordinary select, not by the hand-over.)
    assert!(
        seen.iter().any(|e| matches!(e, AgentEvent::Steered { .. })),
        "the carried call's answer confirmed the injection: {seen:?}"
    );
    assert!(
        seen.iter().any(|e| is_text(e, "steer: mid-turn")),
        "{seen:?}"
    );
    assert!(completed(&seen), "{seen:?}");
    assert_no_protocol_errors(&seen);
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn acp_freeze_is_busy_during_setup_a_busy_session_cancel_and_interrupting() {
    let slow = tempfile::Builder::new()
        .prefix("slow-start")
        .tempdir()
        .unwrap();
    let (c, rig) = controls();
    let mut run = grok()
        .run(request_in("first", slow.path().to_str().unwrap()), c)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("the ACP session is starting")
    );
    until_done(&mut run).await;

    // A steer right after a turn cancels the agent's possibly self-continued
    // turn and waits out a flush before prompting: not a safe point.
    rig.steer.send(steer("second")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("a busy session is being cancelled")
    );
    until_text(&mut run, "echo: second").await;
    until_done(&mut run).await;
    drop(freeze(&rig).await.expect("safe again between turns"));

    rig.steer.send(steer("stubborn")).await.unwrap();
    until_text(&mut run, "hanging").await;
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
async fn acp_process_group_is_not_killed_when_the_old_task_ends_after_commit() {
    let (c, rig) = controls();
    let mut run = devin().run(request("grandchild"), c).await.unwrap();
    let (grandchild, _) = until_text_prefix(&mut run, "grandchild: ").await;
    let grandchild: i32 = grandchild.parse().unwrap();
    until_done(&mut run).await;
    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    let handoff = commit(frozen, &mut run).await;
    // The old task has ended (its stream is over): its group-kill `Drop`
    // must have been disarmed, or the agent's descendants die with it.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(alive(handoff.pid), "the agent survived the old task");
    assert!(alive(grandchild), "its process group survived the old task");

    // The adopter still owns the GROUP: ending the run takes the
    // descendants with the agent, as for a spawned run.
    let (mut adopted, rig2) = adopt_with(&devin(), &handoff, "grandchild").await;
    finish(rig2, &mut adopted, handoff.pid).await;
    wait_dead(grandchild).await;
}

#[tokio::test]
async fn acp_scratch_dir_survives_the_old_image_and_is_removed_by_the_new_one() {
    let (c, rig) = controls();
    let harness = devin().with_adapter_scratch();
    let mut run = harness.run(request("tmp"), c).await.unwrap();
    let (scratch, _) = until_text_prefix(&mut run, "tmp: ").await;
    let scratch = PathBuf::from(scratch);
    assert!(scratch.is_dir(), "{scratch:?}");
    until_done(&mut run).await;
    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    let handoff = commit(frozen, &mut run).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(scratch.is_dir(), "the old image left the scratch dir alone");

    let (mut adopted, rig2) = adopt_with(&harness, &handoff, "tmp").await;
    rig2.steer.send(steer("more")).await.unwrap();
    until_text(&mut adopted, "echo: more").await;
    assert!(scratch.is_dir(), "still in use by the adopted run");
    finish(rig2, &mut adopted, handoff.pid).await;
    assert!(!scratch.exists(), "the adopter removed it at run end");
}

#[tokio::test]
async fn an_adopted_scratch_dir_must_be_one_of_ours() {
    let (c, rig) = controls();
    let mut run = devin().run(request("first"), c).await.unwrap();
    until_done(&mut run).await;
    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    let handoff = commit(frozen, &mut run).await;
    // A manifest naming a directory we never created must not get it
    // deleted at run end.
    let victim = tempfile::tempdir().unwrap();
    let mut forged = handoff.clone();
    forged.state["scratch"] = serde_json::json!(victim.path());
    let (c, _rig) = controls();
    let Err(err) = devin().adopt(forged, c, request("first")).await else {
        panic!("adopted a foreign scratch dir");
    };
    assert!(matches!(err, HarnessError::Protocol(_)), "{err}");
    assert!(victim.path().is_dir());
    let (mut adopted, rig2) = adopt_with(&devin(), &handoff, "first").await;
    finish(rig2, &mut adopted, handoff.pid).await;
}

/// `<root>/<encoded cwd>/child-1/chat_history.jsonl`, grok's layout.
fn child_history(root: &Path) -> PathBuf {
    let dir = root.join("%2Ftmp").join("child-1");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("chat_history.jsonl")
}

fn append(path: &Path, text: &str) {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    file.write_all(text.as_bytes()).unwrap();
}

fn assistant_line(text: &str) -> String {
    format!("{{\"type\":\"assistant\",\"content\":\"{text}\"}}\n")
}

fn subagent_texts(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            } if parent_tool_use_id == "sp1" => match &**event {
                AgentEvent::TextDelta { text } => Some(text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn grok_subagent_tail_does_not_re_emit_the_child_transcript_after_adopt() {
    let root = tempfile::tempdir().unwrap();
    let history = child_history(root.path());
    append(&history, &assistant_line("child one"));
    let harness = grok().with_sessions_root(root.path());
    let (c, rig) = controls();
    let mut run = harness.run(request("spawn"), c).await.unwrap();
    // The tail's first entry may come before or after the agent's text.
    let mut seen = Vec::new();
    while !(seen.iter().any(|e| is_text(e, "spawned"))
        && subagent_texts(&seen) == ["child one\n\n"])
    {
        let event = next(&mut run, &seen).await.expect("the run goes on");
        seen.push(event);
    }
    assert_no_protocol_errors(&seen);
    // Half an entry is on disk when the run freezes: the tail's carry.
    let second = assistant_line("child two");
    let (head, rest) = second.split_at(20);
    append(&history, head);
    tokio::time::sleep(Duration::from_millis(600)).await;
    let frozen = freeze(&rig)
        .await
        .expect("a prompt in flight is a safe point");
    let handoff = commit(frozen, &mut run).await;
    append(&history, rest);
    append(&history, &assistant_line("child three"));

    let (mut adopted, rig2) = adopt_with(&harness, &handoff, "spawn").await;
    rig2.steer.send(steer("finish-sub")).await.unwrap();
    // The tail settles its chip after a short drain, independently of the
    // parent's turn.
    let chip_done = |e: &AgentEvent| matches!(e, AgentEvent::Subagent { event, .. } if matches!(**event, AgentEvent::Done { .. }));
    let mut seen = Vec::new();
    while !(seen.iter().any(|e| is_text(e, "echo: finish-sub")) && seen.iter().any(chip_done)) {
        let event = next(&mut adopted, &seen).await.expect("the run goes on");
        seen.push(event);
    }
    assert_eq!(
        subagent_texts(&seen),
        ["child two\n\n", "child three\n\n"],
        "only what the old image had not emitted, once: {seen:?}"
    );
    assert_no_protocol_errors(&seen);
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn stale_grok_prompt_complete_is_still_discarded_after_adopt() {
    let (c, rig) = controls();
    let mut run = grok().run(request("first"), c).await.unwrap();
    let seen = until_done(&mut run).await;
    assert!(
        seen.iter().any(|e| is_text(e, "promptId: zeron-p1")),
        "{seen:?}"
    );
    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    let handoff = commit(frozen, &mut run).await;

    // The agent replays the settled prompt's completion before answering
    // the next one: it must not settle the adopted run's new turn.
    let (mut adopted, rig2) = adopt_with(&grok(), &handoff, "first").await;
    rig2.steer.send(steer("replay")).await.unwrap();
    let seen = until_done(&mut adopted).await;
    let echoed = seen
        .iter()
        .position(|e| is_text(e, "echo: replay"))
        .unwrap_or_else(|| panic!("the stale completion ended the turn early: {seen:?}"));
    assert!(echoed < seen.len() - 1);
    assert!(
        seen.iter().any(|e| is_text(e, "promptId: zeron-p2")),
        "prompt ids continue: {seen:?}"
    );
    assert!(completed(&seen), "{seen:?}");
    assert_no_protocol_errors(&seen);
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn a_parked_permission_question_is_answered_with_the_original_jsonrpc_id() {
    let (c, mut rig) = controls();
    let mut run = devin().run(request("ask"), c).await.unwrap();
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

    let (mut adopted, mut rig2) = adopt_with(&devin(), &handoff, "ask").await;
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
    // The fake names any other response id.
    let seen = until_done(&mut adopted).await;
    assert!(
        seen.iter().any(|e| is_text(e, "answered: opt-b")),
        "{seen:?}"
    );
    assert!(
        rig2.questions.try_recv().is_err(),
        "no second prompt for the same question"
    );
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn an_answer_given_just_before_a_freeze_is_written_not_exported_as_parked() {
    // The answer and the next freeze request are both ready when the loop
    // looks: the user answered while the run was frozen, the exec failed,
    // and the engine asks again before the thawed loop has run.
    let (c, mut rig) = controls();
    let mut run = devin().run(request("ask"), c).await.unwrap();
    until_text(&mut run, "asking").await;
    let (questions, answer) = tokio::time::timeout(WAIT, rig.questions.recv())
        .await
        .unwrap()
        .expect("asked");
    let first = freeze(&rig)
        .await
        .expect("a parked question is a safe point");
    // The engine resolved the question: it no longer holds its resolver.
    answer
        .send(vec![UserInputAnswer {
            question_id: questions[0].id.clone(),
            labels: vec!["B".into()],
        }])
        .unwrap();
    let (reply, again) = oneshot::channel();
    rig.freeze.send(FreezeRequest { reply }).await.unwrap();
    drop(first); // thaw
    let frozen = tokio::time::timeout(WAIT, again)
        .await
        .expect("answered")
        .expect("replied")
        .expect("a safe point");
    let handoff = commit(frozen, &mut run).await;

    let (mut adopted, mut rig2) = adopt_with(&devin(), &handoff, "ask").await;
    let seen = until_done(&mut adopted).await;
    assert!(
        seen.iter().any(|e| is_text(e, "answered: opt-b")),
        "the answer reached the agent: {seen:?}"
    );
    assert!(
        rig2.rebinds.try_recv().is_err(),
        "nothing was left parked to rebind"
    );
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn antigravity_echo_filter_state_survives_so_a_split_wakeup_block_never_leaks() {
    let harness = graces(AcpHarness::antigravity());
    let (c, rig) = controls();
    let mut run = harness.run(request("echo-head"), c).await.unwrap();
    until_text(&mut run, "before ").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let frozen = freeze(&rig)
        .await
        .expect("a prompt in flight is a safe point");
    let handoff = commit(frozen, &mut run).await;
    // The rest of the block arrives in the same turn, after the hand-over.
    let line = format!(
        "{}\n",
        serde_json::json!({"jsonrpc": "2.0", "method": "test/say", "params": {"text": "echo-tail", "end": true}})
    );
    let written = unsafe { libc::write(handoff.stdin_fd, line.as_ptr().cast(), line.len()) };
    assert_eq!(written, line.len() as isize);

    let (mut adopted, rig2) = adopt_with(&harness, &handoff, "echo-head").await;
    let seen = until_done(&mut adopted).await;
    let text: String = seen
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        text, " after",
        "the held-back block head was carried: {seen:?}"
    );
    finish(rig2, &mut adopted, handoff.pid).await;
}

#[tokio::test]
async fn a_thawed_run_resumes_with_its_drained_steers_first_and_in_order() {
    // Hermes steers at the turn boundary: every steer is queued and the
    // queue becomes the next prompt, in order.
    let (c, rig) = controls();
    let mut run = graces(AcpHarness::hermes())
        .run(request("slow"), c)
        .await
        .unwrap();
    until_text(&mut run, "working").await;
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
    // ...and the next freeze (answered ahead of new input) drains them.
    let frozen = freeze(&rig).await.expect("still a safe point");
    assert_eq!(prompts(&frozen), ["one", "two"]);
    // Arrives while the drained ones are held: it must follow them.
    rig.steer.send(steer("three")).await.unwrap();
    drop(frozen);
    let seen = until_text(&mut run, "echo: one\n\ntwo\n\nthree").await;
    assert_no_protocol_errors(&seen);
    rig.interrupt.cancel();
    drain(&mut run).await;
}

#[test]
fn a_run_task_dropped_while_frozen_leaves_the_child_running_and_adoptable() {
    // Stands in for the exec path: the engine never commits, the old image
    // just stops existing. Dropping the runtime drops the run task and its
    // child handle; the dups keep the pipes open as the exec'd image's
    // inherited descriptors would.
    let old = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (frozen, inherited) = old.block_on(async {
        let (c, rig) = controls();
        let mut run = devin().run(request("first"), c).await.unwrap();
        until_text(&mut run, "echo: first").await;
        until_done(&mut run).await;
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
        "a dropped frozen run must not kill its child or its group"
    );

    let new = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    new.block_on(async {
        let (mut adopted, rig) = adopt_with(&devin(), &inherited, "first").await;
        rig.steer.send(steer("after exec")).await.unwrap();
        let seen = until_done(&mut adopted).await;
        assert!(
            seen.iter().any(|e| is_text(e, "echo: after exec")),
            "{seen:?}"
        );
        assert_no_protocol_errors(&seen);
        finish(rig, &mut adopted, inherited.pid).await;
    });
}

#[tokio::test]
async fn adopted_child_that_died_in_the_gap_ends_the_run_with_its_real_exit_status() {
    let (c, rig) = controls();
    let mut run = devin().run(request("hang"), c).await.unwrap();
    until_text(&mut run, "hanging").await;
    let frozen = freeze(&rig).await.expect("mid-turn at a line boundary");
    let handoff = commit(frozen, &mut run).await;
    // The agent dies between the images.
    let line = b"{\"jsonrpc\":\"2.0\",\"method\":\"test/die\",\"params\":{\"code\":7}}\n";
    let written = unsafe { libc::write(handoff.stdin_fd, line.as_ptr().cast(), line.len()) };
    assert_eq!(written, line.len() as isize);
    wait_dead(handoff.pid).await;

    let (mut adopted, rig2) = adopt_with(&devin(), &handoff, "hang").await;
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
        error.contains("fake-acp: started"),
        "seeded from the exported tail: {error}"
    );
    assert!(error.contains("dying"), "read after adoption: {error}");
    drop(rig2);
}

#[tokio::test]
async fn an_unknown_state_version_is_refused_so_the_engine_falls_back() {
    let handoff = HarnessHandoff {
        harness: HarnessId::Devin,
        state_version: 9999,
        pid: 4242,
        stdin_fd: 100,
        stdout_fd: 101,
        stderr_fd: None,
        extra_fds: Vec::new(),
        stdout_leftover: Vec::new(),
        stderr_tail: Vec::new(),
        state: serde_json::json!({}),
        undrained_steers: Vec::new(),
    };
    let (c, _rig) = controls();
    let Err(err) = devin().adopt(handoff, c, request("x")).await else {
        panic!("adopted an unknown state version");
    };
    assert!(matches!(err, HarnessError::Protocol(_)), "{err}");
    for harness in [
        AcpHarness::devin(),
        AcpHarness::grok(),
        AcpHarness::hermes(),
        AcpHarness::antigravity(),
    ] {
        assert!(harness.supports_adoption(), "{:?}", harness.id());
    }
    assert!(!zeron_harness::PiHarness::new().supports_adoption());
}
