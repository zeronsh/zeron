//! A mock harness whose runs survive a live update: the reference
//! implementation of the handoff seam (`crate::handoff`) for engine tests.
//!
//! Every run drives a real child, `cat`, over real pipes: what the "agent"
//! does next is written to the child's stdin as a JSON line and only happens
//! when the child echoes it back on stdout. So at any moment work can sit in
//! the kernel pipe or in the reader's buffer, exactly like a real agent's
//! output, and a freeze exports real descriptors, a real pid and real
//! leftover bytes.
//!
//! What a turn does is picked by the first word of its prompt:
//!
//! - `ask` — asks one question (`q-<turn>`, options `yes`/`no`) through the
//!   engine's input bridge and answers `you said <label>` once answered;
//! - `hold` — says `working` and keeps the turn open until the next steer;
//! - `busy inline|held|setup` — like `hold`, but refuses to freeze;
//! - `slowstop` — like `hold`, but takes 1.5 s to honour an interrupt;
//! - `continue` — says `continued` and ends the turn;
//! - anything else — says `reply: <prompt>` and ends the turn.
//!
//! A steer is taken between turns and while a turn is open (never while a
//! question is parked): it emits `Steered` and starts a turn of its own.

use std::collections::VecDeque;
use std::os::fd::{AsRawFd, IntoRawFd};

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
    UserInputAnswer, UserInputQuestion,
};

use crate::handoff::{PausedWriter, WriteMsg, drain_steering, dup_inherited, run_writer};
use crate::line_reader::LineReader;
use crate::process::{ChildStdin, ChildStdout};
use crate::{
    ChildHandle, FreezeRefusal, FreezeRequest, FrozenRun, Harness, HarnessError, HarnessHandoff,
    RunControls, SteerMessage, SteerRecord,
};

const STATE_VERSION: u32 = 1;

/// See the module docs.
pub struct FreezableMock;

/// Why a mid-turn run refuses to freeze (the unsafe states real harnesses
/// have: an inline request in flight, a held turn end, a setup handshake).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Busy {
    Inline,
    Held,
    Setup,
}

impl Busy {
    fn parse(word: &str) -> Self {
        match word {
            "held" => Self::Held,
            "setup" => Self::Setup,
            _ => Self::Inline,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Inline => "an inline request is in flight",
            Self::Held => "a turn end is being held",
            Self::Setup => "the agent is still starting up",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Phase {
    /// Between turns.
    Parked,
    /// A turn is open, waiting for a steer.
    Working,
    /// A turn is open in a state that cannot freeze.
    Busy { busy: Busy },
    /// Waiting for the answer to `question_id`.
    Asking { question_id: String },
}

/// What the child echoes back, one JSON line each.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Wire {
    Say { text: String },
    Phase { phase: Phase },
    Ask { question_id: String },
    SlowStop,
    Steered { message_id: Option<String> },
    Done,
}

/// The exported protocol state: everything the echoed lines already did.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    session_id: String,
    turn: u32,
    phase: Phase,
    slow_stop: bool,
}

#[async_trait]
impl Harness for FreezableMock {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![Model {
            id: "mock-1".into(),
            label: "Mock 1".into(),
            description: None,
            reasoning_levels: vec![ReasoningLevel::Medium],
            options: vec![],
        }])
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let mut command = crate::process::Command::new("cat");
        command
            .stdin(crate::process::Stdio::piped())
            .stdout(crate::process::Stdio::piped())
            .stderr(crate::process::Stdio::null())
            .kill_on_drop(false);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let state = State {
            session_id: format!("mock-session-{}", uuid::Uuid::new_v4()),
            turn: 0,
            phase: Phase::Parked,
            slow_stop: false,
        };
        let (events, rx) = mpsc::unbounded_channel();
        let _ = events.send(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: request.model.clone().unwrap_or_else(|| "mock-1".into()),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: state.session_id.clone(),
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
        });
        let mut run = Run::new(
            state,
            ChildHandle::Owned(child),
            stdin,
            LineReader::new(stdout),
            events,
            controls,
            None,
        );
        run.begin_turn(&request.prompt);
        tokio::spawn(run.drive());
        Ok(stream(rx))
    }

    fn supports_adoption(&self) -> bool {
        true
    }

    async fn adopt(
        &self,
        handoff: HarnessHandoff,
        controls: RunControls,
        _request: RunRequest,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        if handoff.state_version != STATE_VERSION {
            return Err(HarnessError::Protocol(format!(
                "mock state version {} is not {STATE_VERSION}",
                handoff.state_version
            )));
        }
        let state: State = serde_json::from_value(handoff.state)
            .map_err(|e| HarnessError::Protocol(format!("mock state: {e}")))?;
        // Work on duplicates: the inherited originals stay untouched for a
        // rollback and are closed when the adoption commits.
        let stdin = ChildStdin::from_std(std::process::ChildStdin::from(dup_inherited(
            handoff.stdin_fd,
        )?))?;
        let stdout = ChildStdout::from_std(std::process::ChildStdout::from(dup_inherited(
            handoff.stdout_fd,
        )?))?;
        // Re-attach a parked question under the engine's request id: no
        // second prompt is shown, and the answer arrives here.
        let answer = match &state.phase {
            Phase::Asking { question_id } => Some((controls.rebind_input)(question_id.clone())),
            _ => None,
        };
        let (events, rx) = mpsc::unbounded_channel();
        let run = Run::new(
            state,
            ChildHandle::adopt(handoff.pid)?,
            stdin,
            LineReader::with_leftover(stdout, handoff.stdout_leftover),
            events,
            controls,
            answer,
        );
        tokio::spawn(run.drive());
        Ok(stream(rx))
    }
}

fn stream(
    rx: mpsc::UnboundedReceiver<AgentEvent>,
) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|event| (Ok(event), rx))
    })
    .boxed()
}

fn question(question_id: &str) -> Vec<UserInputQuestion> {
    vec![UserInputQuestion {
        id: question_id.to_string(),
        header: "Question".into(),
        question: "Proceed?".into(),
        options: vec!["yes".into(), "no".into()],
        prefill: None,
        multiline: false,
        multi_select: false,
    }]
}

type RequestInput =
    Box<dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync>;

/// One run's loop, from a spawn or an adoption.
struct Run {
    state: State,
    /// `None` only after a commit handed the child over.
    child: Option<ChildHandle>,
    writer: mpsc::UnboundedSender<WriteMsg>,
    /// `None` only while a freeze holds the pipe.
    reader: Option<LineReader<ChildStdout>>,
    events: mpsc::UnboundedSender<AgentEvent>,
    request_input: RequestInput,
    steering: mpsc::Receiver<SteerMessage>,
    steering_open: bool,
    /// Steers taken out of the mailbox by a freeze that was then thawed:
    /// read before the mailbox, in order.
    held: VecDeque<SteerMessage>,
    interrupt: crate::CancellationToken,
    freeze: mpsc::Receiver<FreezeRequest>,
    answer: Option<oneshot::Receiver<Vec<UserInputAnswer>>>,
    _lease: Option<std::sync::Arc<tokio::sync::OwnedRwLockReadGuard<()>>>,
}

enum Frozen {
    Thawed,
    Committed,
}

impl Run {
    fn new(
        state: State,
        child: ChildHandle,
        stdin: ChildStdin,
        reader: LineReader<ChildStdout>,
        events: mpsc::UnboundedSender<AgentEvent>,
        controls: RunControls,
        answer: Option<oneshot::Receiver<Vec<UserInputAnswer>>>,
    ) -> Self {
        let RunControls {
            execution_lease,
            request_input,
            steering,
            interrupt,
            freeze,
            rebind_input: _,
        } = controls;
        Self {
            state,
            child: Some(child),
            writer: spawn_writer(stdin),
            reader: Some(reader),
            events,
            request_input,
            steering,
            steering_open: true,
            held: VecDeque::new(),
            interrupt,
            freeze,
            answer,
            _lease: execution_lease,
        }
    }

    fn write(&self, wire: Wire) {
        let line = serde_json::to_string(&wire).expect("wire lines serialize");
        let _ = self.writer.send(WriteMsg::Line(line));
    }

    fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    /// Decide what the turn does; it happens as the child echoes it back.
    fn begin_turn(&mut self, prompt: &str) {
        self.state.turn += 1;
        let mut words = prompt.split_whitespace();
        let first = words.next().unwrap_or("").to_ascii_lowercase();
        let say = |text: &str| Wire::Say { text: text.into() };
        let lines = match first.as_str() {
            "ask" => vec![
                say("I need an answer first."),
                Wire::Ask {
                    question_id: format!("q-{}", self.state.turn),
                },
            ],
            "hold" => vec![
                say("working"),
                Wire::Phase {
                    phase: Phase::Working,
                },
            ],
            "busy" => vec![
                say("busy"),
                Wire::Phase {
                    phase: Phase::Busy {
                        busy: Busy::parse(words.next().unwrap_or("")),
                    },
                },
            ],
            "slowstop" => vec![
                say("working"),
                Wire::SlowStop,
                Wire::Phase {
                    phase: Phase::Working,
                },
            ],
            "continue" => vec![say("continued"), Wire::Done],
            _ => vec![say(&format!("reply: {prompt}")), Wire::Done],
        };
        for line in lines {
            self.write(line);
        }
    }

    /// Apply one echoed line.
    fn apply(&mut self, line: &str) {
        let Ok(wire) = serde_json::from_str::<Wire>(line) else {
            return;
        };
        match wire {
            Wire::Say { text } => self.emit(AgentEvent::TextDelta { text }),
            Wire::Phase { phase } => self.state.phase = phase,
            Wire::SlowStop => self.state.slow_stop = true,
            Wire::Ask { question_id } => {
                self.answer = Some((self.request_input)(question(&question_id)));
                self.state.phase = Phase::Asking { question_id };
            }
            Wire::Steered { message_id } => self.emit(AgentEvent::Steered {
                assistant_message_id: None,
                next_assistant_message_id: message_id.map(|id| format!("{id}-reply")),
            }),
            Wire::Done => {
                self.state.phase = Phase::Parked;
                self.state.slow_stop = false;
                self.emit(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some(self.state.session_id.clone()),
                });
            }
        }
    }

    fn takes_steers(&self) -> bool {
        self.steering_open && !matches!(self.state.phase, Phase::Asking { .. })
    }

    async fn next_steer(
        held: &mut VecDeque<SteerMessage>,
        steering: &mut mpsc::Receiver<SteerMessage>,
    ) -> Option<SteerMessage> {
        match held.pop_front() {
            Some(steer) => Some(steer),
            None => steering.recv().await,
        }
    }

    async fn drive(mut self) {
        loop {
            tokio::select! {
                biased;
                _ = self.interrupt.cancelled() => {
                    self.stop().await;
                    return;
                }
                Some(request) = self.freeze.recv() => {
                    if let Frozen::Committed = self.on_freeze(request).await {
                        return;
                    }
                }
                line = self.reader.as_mut().expect("reader").next_line() => match line {
                    Ok(Some(line)) => self.apply(&line),
                    Ok(None) | Err(_) => {
                        self.emit(AgentEvent::Done {
                            status: DoneStatus::Errored,
                            result: None,
                            error: Some("mock child exited".into()),
                            session_id: None,
                        });
                        return;
                    }
                },
                answer = async { self.answer.as_mut().expect("guarded").await },
                    if self.answer.is_some() =>
                {
                    self.answer = None;
                    let label = answer
                        .ok()
                        .and_then(|answers| answers.into_iter().next())
                        .and_then(|answer| answer.labels.into_iter().next())
                        .unwrap_or_else(|| "nothing".into());
                    self.write(Wire::Say { text: format!("you said {label}") });
                    self.write(Wire::Done);
                    // Until the echo lands the turn is still open.
                    self.state.phase = Phase::Working;
                }
                steer = Self::next_steer(&mut self.held, &mut self.steering), if self.takes_steers() => {
                    match steer {
                        Some(steer) => {
                            self.write(Wire::Steered { message_id: steer.message_id });
                            self.begin_turn(&steer.prompt);
                        }
                        None => self.steering_open = false,
                    }
                }
            }
        }
    }

    async fn stop(&mut self) {
        if self.state.slow_stop {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        let _ = self.writer.send(WriteMsg::Close);
        if let Some(child) = self.child.as_mut()
            && tokio::time::timeout(std::time::Duration::from_secs(2), child.wait())
                .await
                .is_err()
        {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        self.emit(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            result: None,
            error: None,
            session_id: None,
        });
    }

    /// Answer a freeze request at this (safe or unsafe) point.
    async fn on_freeze(&mut self, request: FreezeRequest) -> Frozen {
        if let Phase::Busy { busy } = self.state.phase {
            let _ = request.reply.send(Err(FreezeRefusal::Busy(busy.reason())));
            return Frozen::Thawed;
        }
        let Some(pid) = self.child.as_ref().and_then(ChildHandle::id) else {
            let _ = request
                .reply
                .send(Err(FreezeRefusal::Busy("the agent process has exited")));
            return Frozen::Thawed;
        };
        if !child_holdable(self.child.as_ref()) {
            let _ = request
                .reply
                .send(Err(FreezeRefusal::Busy("the agent process has exited")));
            return Frozen::Thawed;
        }
        // Queued stdin lines finish first; the writer then parks WITH its pipe
        // and queue, so a thaw needs nothing rebuilt.
        let (pause_tx, pause_rx) = oneshot::channel();
        let paused: Option<PausedWriter> = match self.writer.send(WriteMsg::Pause(pause_tx)) {
            Ok(()) => pause_rx.await.ok(),
            Err(_) => None,
        };
        let Some(paused) = paused else {
            self.release_child();
            let _ = request
                .reply
                .send(Err(FreezeRefusal::Busy("the agent's stdin is closed")));
            return Frozen::Thawed;
        };
        let (Some(stdin_fd), Some(reader)) = (paused.stdin_fd, self.reader.as_ref()) else {
            self.release_child();
            let _ = request.reply.send(Err(FreezeRefusal::Busy(
                "the agent's pipes are not exportable",
            )));
            return Frozen::Thawed;
        };
        // Steers a previous, thawed freeze put back come first, then the mailbox.
        let mut undrained: Vec<SteerRecord> = self.held.drain(..).map(SteerRecord::from).collect();
        undrained.extend(drain_steering(&mut self.steering));
        let handoff = HarnessHandoff {
            harness: HarnessId::Mock,
            state_version: STATE_VERSION,
            pid: pid as i32,
            stdin_fd,
            stdout_fd: reader.get_ref().as_raw_fd(),
            stderr_fd: None,
            stdout_leftover: reader.leftover().to_vec(),
            stderr_tail: Vec::new(),
            state: serde_json::to_value(&self.state).expect("state serializes"),
            extra_fds: Vec::new(),
            undrained_steers: undrained.clone(),
        };
        let (commit_tx, commit_rx) = oneshot::channel();
        let sent = request.reply.send(Ok(FrozenRun {
            handoff,
            commit: commit_tx,
        }));
        let committed = sent.is_ok() && commit_rx.await.is_ok();
        if committed {
            // A same-process successor owns the child and its pipes now (the
            // exec path never gets here): give them up without closing,
            // killing or reaping anything.
            paused.abandon().await;
            if let Some(child) = self.child.take() {
                let _ = child.release();
            }
            if let Some(reader) = self.reader.take()
                && let Ok(fd) = reader.into_parts().0.into_owned_fd()
            {
                let _ = fd.into_raw_fd();
            }
            return Frozen::Committed;
        }
        // Thawed: dropping the guard resumes the writer, the reader kept its
        // buffer, and the drained steers are read again, in order.
        drop(paused);
        self.release_child();
        self.held
            .extend(undrained.into_iter().map(SteerMessage::from));
        Frozen::Thawed
    }

    /// Let an adopted child be reaped again after a freeze that did not go ahead.
    fn release_child(&self) {
        if let Some(child) = &self.child {
            child.release_reaping();
        }
    }
}

/// Hold back reaping of the child for the freeze window; `false` when it is gone.
fn child_holdable(child: Option<&ChildHandle>) -> bool {
    child.is_some_and(ChildHandle::hold_reaping)
}

fn spawn_writer(stdin: ChildStdin) -> mpsc::UnboundedSender<WriteMsg> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run_writer(stdin, rx, "mock"));
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn request(prompt: &str) -> RunRequest {
        RunRequest {
            mcp: None,
            prompt: prompt.into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: "/tmp".into(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            attachments: Vec::new(),
            resume: None,
            worktree: None,
        }
    }

    struct Rig {
        steer: mpsc::Sender<SteerMessage>,
        freeze: mpsc::Sender<FreezeRequest>,
        interrupt: crate::CancellationToken,
        questions: mpsc::UnboundedReceiver<oneshot::Sender<Vec<UserInputAnswer>>>,
        rebinds: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    fn new_controls() -> (RunControls, Rig) {
        let (steer, steering) = mpsc::channel(32);
        let (freeze_tx, freeze) = mpsc::channel(1);
        let interrupt = crate::CancellationToken::new();
        let (question_tx, questions) = mpsc::unbounded_channel();
        let rebinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = rebinds.clone();
        let rebind_questions = question_tx.clone();
        let controls = RunControls {
            execution_lease: None,
            request_input: Box::new(move |_| {
                let (tx, rx) = oneshot::channel();
                let _ = question_tx.send(tx);
                rx
            }),
            steering,
            interrupt: interrupt.clone(),
            freeze,
            rebind_input: Box::new(move |key| {
                seen.lock().unwrap().push(key);
                let (tx, rx) = oneshot::channel();
                let _ = rebind_questions.send(tx);
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

    async fn text_until(
        stream: &mut BoxStream<'static, Result<AgentEvent, HarnessError>>,
        want: &str,
    ) -> Vec<AgentEvent> {
        let mut seen = Vec::new();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), stream.next())
                .await
                .unwrap_or_else(|_| panic!("no {want:?} in {seen:?}"))
                .expect("stream open")
                .unwrap();
            let hit = matches!(&event, AgentEvent::TextDelta { text } if text == want);
            seen.push(event);
            if hit {
                return seen;
            }
        }
    }

    async fn freeze(rig: &Rig) -> Result<FrozenRun, FreezeRefusal> {
        let (reply, rx) = oneshot::channel();
        rig.freeze.send(FreezeRequest { reply }).await.unwrap();
        rx.await.unwrap()
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    #[tokio::test]
    async fn steers_put_back_by_a_thaw_are_exported_again_by_the_next_freeze_in_order() {
        let (controls, rig) = new_controls();
        let mut stream = FreezableMock.run(request("hold"), controls).await.unwrap();
        text_until(&mut stream, "working").await;
        let send = |prompt: &'static str| {
            let steer = rig.steer.clone();
            async move {
                steer
                    .send(SteerMessage {
                        prompt: prompt.into(),
                        message_id: Some(prompt.into()),
                    })
                    .await
                    .unwrap()
            }
        };
        send("one").await;
        send("two").await;
        let first = freeze(&rig).await.expect("safe point");
        assert_eq!(
            first
                .handoff
                .undrained_steers
                .iter()
                .map(|s| s.prompt.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        drop(first); // thaw: both go back, ahead of anything newer
        send("three").await;
        let second = freeze(&rig).await.expect("safe point again");
        assert_eq!(
            second
                .handoff
                .undrained_steers
                .iter()
                .map(|s| s.prompt.as_str())
                .collect::<Vec<_>>(),
            ["one", "two", "three"],
            "held steers first, then the mailbox, none lost or reordered"
        );
        drop(second);
        rig.interrupt.cancel();
    }

    #[tokio::test]
    async fn a_thawed_run_continues_with_its_undrained_steers() {
        let (controls, rig) = new_controls();
        let mut stream = FreezableMock.run(request("hold"), controls).await.unwrap();
        text_until(&mut stream, "working").await;
        let frozen = freeze(&rig).await.expect("a held turn is a safe point");
        assert!(alive(frozen.handoff.pid));
        // A steer that reaches the mailbox after the freeze stays there.
        rig.steer
            .send(SteerMessage {
                prompt: "late".into(),
                message_id: None,
            })
            .await
            .unwrap();
        drop(frozen); // thaw
        let events = text_until(&mut stream, "reply: late").await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::Steered { .. }))
        );
        rig.interrupt.cancel();
    }

    #[tokio::test]
    async fn busy_states_refuse_and_a_commit_hands_over_to_an_adopter() {
        let (controls, rig) = new_controls();
        let mut stream = FreezableMock
            .run(request("busy inline"), controls)
            .await
            .unwrap();
        text_until(&mut stream, "busy").await;
        assert_eq!(
            freeze(&rig).await.unwrap_err(),
            FreezeRefusal::Busy("an inline request is in flight")
        );
        rig.steer
            .send(SteerMessage {
                prompt: "ask".into(),
                message_id: Some("m1".into()),
            })
            .await
            .unwrap();
        text_until(&mut stream, "I need an answer first.").await;
        let mut rig = rig;
        let _parked = rig.questions.recv().await.expect("asked");
        // Queued behind the question: never read before the freeze.
        rig.steer
            .send(SteerMessage {
                prompt: "after".into(),
                message_id: Some("m2".into()),
            })
            .await
            .unwrap();
        let frozen = freeze(&rig)
            .await
            .expect("a parked question is a safe point");
        let handoff = frozen.handoff.clone();
        assert_eq!(handoff.undrained_steers.len(), 1);
        assert_eq!(handoff.undrained_steers[0].prompt, "after");
        frozen.commit.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(5), stream.next())
                .await
                .unwrap()
                .is_none(),
            "a committed run ends its stream"
        );
        assert!(alive(handoff.pid), "the child is handed over, not killed");

        let (controls, mut adopter) = new_controls();
        for steer in handoff.undrained_steers.clone() {
            adopter.steer.send(steer.into()).await.unwrap();
        }
        let mut adopted = FreezableMock
            .adopt(handoff.clone(), controls, request(""))
            .await
            .unwrap();
        let answer = adopter.questions.recv().await.expect("rebound");
        assert_eq!(*adopter.rebinds.lock().unwrap(), vec!["q-2".to_string()]);
        answer
            .send(vec![UserInputAnswer {
                question_id: "q-2".into(),
                labels: vec!["yes".into()],
            }])
            .unwrap();
        text_until(&mut adopted, "you said yes").await;
        text_until(&mut adopted, "reply: after").await;
        adopter.interrupt.cancel();
        let mut last = None;
        while let Some(event) = adopted.next().await {
            last = Some(event.unwrap());
        }
        assert!(matches!(
            last,
            Some(AgentEvent::Done {
                status: DoneStatus::Interrupted,
                ..
            })
        ));
        assert!(!alive(handoff.pid), "the adopter reaped the child");
        unsafe {
            libc::close(handoff.stdin_fd);
            libc::close(handoff.stdout_fd);
        }
    }
}
