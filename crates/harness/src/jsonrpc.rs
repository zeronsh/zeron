//! Minimal JSON-RPC 2.0 client over a child agent's stdio (newline-delimited
//! frames, id-multiplexed), ported from codex.ts's `startAppServer`. Shared by
//! the Codex app-server harness and the ACP harness — both protocols are
//! newline-framed JSON-RPC 2.0 over stdio.
//!
//! - Responses are matched to callers by numeric id (a shared pending map the
//!   reader task resolves directly, so requests can be awaited from anywhere —
//!   including inside the session loop — without starving notifications).
//! - Notifications and server→client requests (approvals) are pumped into an
//!   [`Incoming`] channel the session loop drains.
//! - Writes to a dead child's stdin (EPIPE) are tolerated and logged, matching
//!   the TS harness's swallowed-EPIPE behavior. Every frame is written as one
//!   buffer by [`run_writer`], so a line is never torn.
//!
//! # Live update (freeze / thaw / adopt)
//!
//! A run that hands its agent to the next engine image (see `crate::handoff`)
//! calls [`RpcClient::freeze`] at a safe point of its loop. The freeze
//! SUSPENDS the client; it does not end it:
//!
//! 1. it refuses ([`FreezeRefusal::Busy`]) while a request of ours awaits its
//!    response — that response would reach nobody after an exec. A protocol
//!    whose requests are long-lived (ACP's `session/prompt` spans a whole
//!    turn) uses [`RpcClient::freeze_carrying_requests`] instead: it exports
//!    them as [`RpcFrozen::in_flight`], and the successor re-registers the
//!    ones it still waits for with [`RpcClient::resume_request`], so their
//!    responses reach the adopted loop;
//! 2. the writer finishes the lines already queued and pauses, keeping stdin
//!    and its queue ([`WriteMsg::Pause`]);
//! 3. the reader stops at a line boundary and parks, keeping its
//!    [`LineReader`] (and so its buffered bytes);
//! 4. messages already read but not yet handled by the loop — still in the
//!    loop's [`Incoming`] channel, or held by a reader blocked on a full
//!    channel — are taken out, oldest first.
//!
//! [`RpcFrozen::leftover`] is those messages re-encoded as lines followed by
//! the reader's unconsumed bytes, so a successor built with
//! [`RpcClient::from_parts`] (same pipes, `leftover`, [`RpcFrozen::next_id`])
//! sees exactly the stream the old loop had not handled yet, in order, and
//! never reuses a request id (a late response cannot resolve the wrong call).
//!
//! A caller that drops its request future gives the id up: the entry leaves
//! the pending table at once, so an abandoned call never blocks a freeze and
//! its late response is discarded.
//!
//! Dropping the [`RpcFrozen`] THAWS: the taken messages are redelivered first,
//! in order, then reading resumes with the same buffer and the paused lines
//! are written; `next_id` and the pending table were never touched. Only a
//! same-process successor calls [`RpcFrozen::abandon`] (tests standing in for
//! the exec): both tasks let go of their pipes without closing them.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

use crate::handoff::{PausedWriter, PipeFd, WriteMsg, WriterFd, run_writer};
use crate::line_reader::LineReader;
use crate::process::{ChildStdin, ChildStdout};
use crate::{FreezeRefusal, HarnessError};

/// A non-response line from the app server, in stdout order.
#[derive(Debug, Clone)]
pub(crate) enum Incoming {
    Notification {
        method: String,
        params: Value,
    },
    /// Server→client request (approvals); must be answered via
    /// [`RpcClient::respond`] / [`RpcClient::respond_error`].
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// stdout EOF: the app server exited. All pending requests fail.
    Eof,
}

impl Incoming {
    /// The message as a wire line again (a freeze exports unhandled ones).
    fn to_line(&self) -> Option<String> {
        match self {
            Self::Notification { method, params } => {
                Some(json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string())
            }
            Self::Request { id, method, params } => Some(
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
                    .to_string(),
            ),
            Self::Eof => None,
        }
    }
}

pub(crate) type StdoutObserver = Box<dyn Fn(&str) + Send>;

/// Our requests awaiting a response, by id.
type Pending = Arc<Mutex<HashMap<i64, PendingCall>>>;

struct PendingCall {
    method: String,
    reply: oneshot::Sender<Result<Value, String>>,
}

/// A request of ours still awaiting its response when a client froze (see
/// [`RpcClient::freeze_carrying_requests`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct InFlight {
    pub id: i64,
    pub method: String,
}

/// Removes a request from the pending table when its caller goes away
/// without the response (a dropped future), so the id is not "in flight"
/// forever. A no-op once the reader has resolved it.
struct ForgetOnDrop {
    pending: Pending,
    id: i64,
}

impl Drop for ForgetOnDrop {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&self.id);
        }
    }
}

/// How long a freeze waits for the queued stdin lines to be written. A child
/// that stops reading its input would otherwise wedge the freeze; the freeze
/// answers `Busy` instead.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub(crate) struct RpcClient {
    /// The last request id issued (the next one is this + 1).
    next_id: Arc<AtomicI64>,
    pending: Pending,
    writer: mpsc::UnboundedSender<WriteMsg>,
    closed: Arc<AtomicBool>,
    /// Stops the reader task for a freeze.
    reader: mpsc::UnboundedSender<ReaderCtl>,
}

impl RpcClient {
    /// Spawn the writer + reader tasks over the child's stdio; returns the
    /// client and the incoming (notification/request) channel.
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> (Self, mpsc::Receiver<Incoming>) {
        Self::with_stdout_observer(stdin, stdout, None)
    }

    pub(crate) fn with_stdout_observer(
        stdin: ChildStdin,
        stdout: ChildStdout,
        observer: Option<StdoutObserver>,
    ) -> (Self, mpsc::Receiver<Incoming>) {
        let (client, incoming, _) = Self::start(stdin, stdout, Vec::new(), 1, observer, &[]);
        (client, incoming)
    }

    /// Resume a client a previous image froze (see the module docs): `stdin`
    /// and `stdout` are the same pipes (duplicates of the inherited
    /// descriptors), `leftover` is [`RpcFrozen::leftover`] — replayed before
    /// anything is read from `stdout` — and `next_id` is
    /// [`RpcFrozen::next_id`], the id the next request uses.
    pub fn from_parts<W, R>(
        stdin: W,
        stdout: R,
        leftover: Vec<u8>,
        next_id: i64,
    ) -> (Self, mpsc::Receiver<Incoming>)
    where
        W: AsyncWrite + WriterFd + Unpin + Send + 'static,
        R: AsyncRead + PipeFd + Unpin + Send + 'static,
    {
        Self::from_parts_with_observer(stdin, stdout, leftover, next_id, None)
    }

    /// [`Self::from_parts`] with a stdout observer (see
    /// [`Self::with_stdout_observer`]).
    pub(crate) fn from_parts_with_observer<W, R>(
        stdin: W,
        stdout: R,
        leftover: Vec<u8>,
        next_id: i64,
        observer: Option<StdoutObserver>,
    ) -> (Self, mpsc::Receiver<Incoming>)
    where
        W: AsyncWrite + WriterFd + Unpin + Send + 'static,
        R: AsyncRead + PipeFd + Unpin + Send + 'static,
    {
        let (client, incoming, _) = Self::start(stdin, stdout, leftover, next_id, observer, &[]);
        (client, incoming)
    }

    /// [`Self::from_parts_with_observer`] for a client that carries requests a
    /// previous image froze in flight ([`RpcFrozen::in_flight`]): they are
    /// entered in the pending table BEFORE the reader starts, so a response
    /// already sitting in `leftover` (or in the pipe) cannot be read and
    /// discarded as unknown before its waiter exists. The returned futures
    /// correspond to `carried`, in order; each resolves with its response
    /// whenever it arrives (nothing is written), and is an error for an id this
    /// client could issue itself or a duplicate.
    pub(crate) fn from_parts_carrying<W, R>(
        stdin: W,
        stdout: R,
        leftover: Vec<u8>,
        next_id: i64,
        observer: Option<StdoutObserver>,
        carried: &[InFlight],
    ) -> (
        Self,
        mpsc::Receiver<Incoming>,
        Vec<futures::future::BoxFuture<'static, Result<Value, HarnessError>>>,
    )
    where
        W: AsyncWrite + WriterFd + Unpin + Send + 'static,
        R: AsyncRead + PipeFd + Unpin + Send + 'static,
    {
        let (client, incoming, receivers) =
            Self::start(stdin, stdout, leftover, next_id, observer, carried);
        let futures = carried
            .iter()
            .zip(receivers)
            .map(|(call, rx)| match rx {
                Some(rx) => client.await_response(call.id, call.method.clone(), rx),
                None => {
                    let (method, id) = (call.method.clone(), call.id);
                    Box::pin(async move {
                        Err(HarnessError::Protocol(format!(
                            "{method}: request id {id} cannot be resumed here"
                        )))
                    }) as futures::future::BoxFuture<'static, _>
                }
            })
            .collect();
        (client, incoming, futures)
    }

    #[allow(clippy::type_complexity)]
    fn start<W, R>(
        stdin: W,
        stdout: R,
        leftover: Vec<u8>,
        next_id: i64,
        observer: Option<StdoutObserver>,
        carried: &[InFlight],
    ) -> (
        Self,
        mpsc::Receiver<Incoming>,
        Vec<Option<oneshot::Receiver<Result<Value, String>>>>,
    )
    where
        W: AsyncWrite + WriterFd + Unpin + Send + 'static,
        R: AsyncRead + PipeFd + Unpin + Send + 'static,
    {
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<WriteMsg>();
        tokio::spawn(run_writer(stdin, writer_rx, "rpc"));
        let pending: Pending = Arc::default();
        // Carried requests wait BEFORE the reader can see their responses.
        let receivers: Vec<_> = {
            let mut table = pending.lock().expect("pending lock");
            carried
                .iter()
                .map(|call| {
                    if call.id >= next_id || table.contains_key(&call.id) {
                        return None;
                    }
                    let (tx, rx) = oneshot::channel();
                    table.insert(
                        call.id,
                        PendingCall {
                            method: call.method.clone(),
                            reply: tx,
                        },
                    );
                    Some(rx)
                })
                .collect()
        };
        let (incoming_tx, incoming_rx) = mpsc::channel(256);
        let closed = Arc::new(AtomicBool::new(false));
        let (reader_tx, reader_rx) = mpsc::unbounded_channel();
        tokio::spawn(read_loop(
            LineReader::with_leftover(stdout, leftover),
            Arc::clone(&pending),
            incoming_tx,
            closed.clone(),
            observer,
            reader_rx,
        ));
        (
            Self {
                next_id: Arc::new(AtomicI64::new(next_id.saturating_sub(1))),
                pending,
                writer: writer_tx,
                closed,
                reader: reader_tx,
            },
            incoming_rx,
            receivers,
        )
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The id the next request will carry.
    pub fn next_request_id(&self) -> i64 {
        self.next_id.load(Ordering::Relaxed) + 1
    }

    fn has_pending(&self) -> bool {
        !self.pending.lock().expect("pending lock").is_empty()
    }

    /// Suspend the client at a safe point for a live update (see the module
    /// docs). `incoming` is the receiver this client was created with; the
    /// messages still waiting in it are taken out (and put back by a thaw).
    ///
    /// Refuses with [`FreezeRefusal::Busy`] while one of our requests awaits
    /// its response, when the agent's output has closed, or when the agent
    /// does not take its queued input within a bound. A refusal leaves the
    /// client exactly as it was. The caller must not issue requests while the
    /// returned [`RpcFrozen`] lives: they would be answered to nobody after
    /// an exec.
    pub async fn freeze(
        &self,
        incoming: &mut mpsc::Receiver<Incoming>,
    ) -> Result<RpcFrozen, FreezeRefusal> {
        self.freeze_with(incoming, false).await
    }

    /// [`Self::freeze`] for a protocol whose requests outlive a safe point
    /// (ACP's `session/prompt` is in flight for a whole turn): instead of
    /// refusing while requests of ours await their responses, it lists them
    /// in [`RpcFrozen::in_flight`] (taken once the reader has stopped, so no
    /// response can slip between the list and the pipe). A successor
    /// re-registers the ones it still waits for with
    /// [`Self::resume_request`]; their responses, still in the pipe or not
    /// yet written, then reach it.
    ///
    /// A request missing from the list was already answered: its caller
    /// holds the response (the run loop must handle it before a hand-over).
    /// A thaw leaves every request with its caller, untouched.
    pub async fn freeze_carrying_requests(
        &self,
        incoming: &mut mpsc::Receiver<Incoming>,
    ) -> Result<RpcFrozen, FreezeRefusal> {
        self.freeze_with(incoming, true).await
    }

    async fn freeze_with(
        &self,
        incoming: &mut mpsc::Receiver<Incoming>,
        carry_requests: bool,
    ) -> Result<RpcFrozen, FreezeRefusal> {
        const IN_FLIGHT: FreezeRefusal = FreezeRefusal::Busy("a JSON-RPC request is in flight");
        const OUTPUT_CLOSED: FreezeRefusal = FreezeRefusal::Busy("the agent's output has closed");
        if !carry_requests && self.has_pending() {
            return Err(IN_FLIGHT);
        }
        if self.is_closed() {
            return Err(OUTPUT_CLOSED);
        }
        // Pause the writer first, while the reader still drains the agent's
        // output: an agent blocked on a full stdout may not read its stdin.
        let (pause_tx, pause_rx) = oneshot::channel();
        if self.writer.send(WriteMsg::Pause(pause_tx)).is_err() {
            return Err(FreezeRefusal::Busy("the agent's input has closed"));
        }
        let paused = match tokio::time::timeout(PAUSE_TIMEOUT, pause_rx).await {
            Ok(Ok(paused)) => paused,
            _ => return Err(FreezeRefusal::Busy("the agent is not taking its input")),
        };
        // Then stop the reader at a line boundary. (A refusal from here on
        // drops `paused`, which resumes the writer.)
        let (stop_tx, stop_rx) = oneshot::channel();
        if self.reader.send(ReaderCtl::Stop(stop_tx)).is_err() {
            return Err(OUTPUT_CLOSED);
        }
        let Ok(parked) = stop_rx.await else {
            return Err(OUTPUT_CLOSED);
        };
        // From here on dropping `reader` thaws the reader with `redeliver`.
        let mut reader = ReaderGuard {
            resume: Some(parked.resume),
            redeliver: VecDeque::new(),
        };
        // Unhandled messages: the loop's channel first (older), then what
        // the reader still held.
        while let Ok(message) = incoming.try_recv() {
            reader.redeliver.push_back(message);
        }
        reader.redeliver.extend(parked.backlog);
        let in_flight = if carry_requests {
            let mut calls: Vec<InFlight> = self
                .pending
                .lock()
                .expect("pending lock")
                .iter()
                .map(|(id, call)| InFlight {
                    id: *id,
                    method: call.method.clone(),
                })
                .collect();
            calls.sort_by_key(|call| call.id);
            calls
        } else if self.has_pending() {
            return Err(IN_FLIGHT);
        } else {
            Vec::new()
        };
        let mut leftover = Vec::new();
        for message in &reader.redeliver {
            // An EOF is never followed by a parked reader; refuse if it were.
            let Some(line) = message.to_line() else {
                return Err(OUTPUT_CLOSED);
            };
            leftover.extend_from_slice(line.as_bytes());
            leftover.push(b'\n');
        }
        leftover.extend_from_slice(&parked.leftover);
        #[cfg(unix)]
        if let Some(fd) = parked.stdout_fd {
            crate::handoff::grow_pipe(fd);
        }
        Ok(RpcFrozen {
            stdin_fd: paused.stdin_fd,
            stdout_fd: parked.stdout_fd,
            leftover,
            next_id: self.next_request_id(),
            in_flight,
            writer: Some(paused),
            reader,
            closed: self.closed.clone(),
        })
    }

    /// Send a request and await its response (resolved by the reader task).
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, HarnessError> {
        self.request_now(method, params).await
    }

    /// [`Self::request`], but the line is queued for writing before this
    /// returns rather than on first poll — so a notification sent afterwards
    /// (a steer's `session/cancel`) can never overtake it on the wire.
    pub fn request_now(
        &self,
        method: &str,
        params: Value,
    ) -> futures::future::BoxFuture<'static, Result<Value, HarnessError>> {
        self.request_with_id(method, params).1
    }

    /// [`Self::request_now`], also returning the request's id (a run that may
    /// hand this request to a successor names it by id; see
    /// [`Self::freeze_carrying_requests`]).
    pub fn request_with_id(
        &self,
        method: &str,
        params: Value,
    ) -> (
        i64,
        futures::future::BoxFuture<'static, Result<Value, HarnessError>>,
    ) {
        let method = method.to_owned();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let Some(rx) = self.register(id, &method) else {
            return (
                id,
                Box::pin(async move {
                    Err(HarnessError::Protocol(format!(
                        "{method}: app-server exited before responding"
                    )))
                }),
            );
        };
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if self.writer.send(WriteMsg::Line(line.to_string())).is_err() {
            self.pending.lock().expect("pending lock").remove(&id);
            return (
                id,
                Box::pin(async move {
                    Err(HarnessError::Protocol(format!(
                        "{method}: app-server stdin closed"
                    )))
                }),
            );
        }
        (id, self.await_response(id, method, rx))
    }

    /// Wait again for a request a previous image sent and froze in flight
    /// ([`RpcFrozen::in_flight`]): its response, whenever it arrives, resolves
    /// the returned future. Nothing is written. Refused for an id this client
    /// could still issue itself or already waits for.
    pub fn resume_request(
        &self,
        call: &InFlight,
    ) -> futures::future::BoxFuture<'static, Result<Value, HarnessError>> {
        let method = call.method.clone();
        let id = call.id;
        if id >= self.next_request_id() || self.is_pending(id) {
            return Box::pin(async move {
                Err(HarnessError::Protocol(format!(
                    "{method}: request id {id} cannot be resumed here"
                )))
            });
        }
        let Some(rx) = self.register(call.id, &method) else {
            return Box::pin(async move {
                Err(HarnessError::Protocol(format!(
                    "{method}: app-server exited before responding"
                )))
            });
        };
        self.await_response(call.id, method, rx)
    }

    fn is_pending(&self, id: i64) -> bool {
        self.pending.lock().expect("pending lock").contains_key(&id)
    }

    /// Enter `id` in the pending table; `None` once the output has closed.
    fn register(&self, id: i64, method: &str) -> Option<oneshot::Receiver<Result<Value, String>>> {
        let (tx, rx) = oneshot::channel();
        let mut pending = self.pending.lock().expect("pending lock");
        // Check under the same lock as EOF cleanup: a request racing the
        // reader exit must either be rejected here or cleared by it.
        if self.is_closed() {
            return None;
        }
        pending.insert(
            id,
            PendingCall {
                method: method.to_owned(),
                reply: tx,
            },
        );
        Some(rx)
    }

    fn await_response(
        &self,
        id: i64,
        method: String,
        rx: oneshot::Receiver<Result<Value, String>>,
    ) -> futures::future::BoxFuture<'static, Result<Value, HarnessError>> {
        let forget = ForgetOnDrop {
            pending: Arc::clone(&self.pending),
            id,
        };
        Box::pin(async move {
            let _forget = forget;
            match rx.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(message)) => Err(HarnessError::Protocol(format!("{method}: {message}"))),
                // Sender dropped: the reader hit EOF and failed all pending.
                Err(_) => Err(HarnessError::Protocol(format!(
                    "{method}: app-server exited before responding"
                ))),
            }
        })
    }

    /// Fire a notification (no id, no response).
    pub fn notify(&self, method: &str, params: Option<Value>) {
        let line = match params {
            Some(params) => json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            None => json!({ "jsonrpc": "2.0", "method": method }),
        };
        let _ = self.writer.send(WriteMsg::Line(line.to_string()));
    }

    /// Answer a server→client request.
    pub fn respond(&self, id: &Value, result: Value) {
        let line = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let _ = self.writer.send(WriteMsg::Line(line.to_string()));
    }

    /// Reject a server→client request (e.g. unknown method).
    pub fn respond_error(&self, id: &Value, code: i64, message: &str) {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        });
        let _ = self.writer.send(WriteMsg::Line(line.to_string()));
    }
}

/// The id of a response, tolerantly. zeron always sends numeric ids, but
/// JSON-RPC lets a server echo them re-encoded — a string `"5"` or float
/// `5.0` still names request 5. Dropping such a response would strand its
/// caller forever (the session would spin Working with no per-turn timeout).
fn response_id(id: &Value) -> Option<i64> {
    if let Some(n) = id.as_i64() {
        return Some(n);
    }
    if let Some(s) = id.as_str() {
        return s.parse().ok();
    }
    id.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)
}

/// Preserve the agent's rejection reason as well as the generic RPC label.
fn response_error(error: &Value) -> String {
    let Some(message) = error.get("message").and_then(Value::as_str) else {
        return error.to_string();
    };
    let mut rendered = message.to_owned();
    if let Some(code) = error.get("code").and_then(Value::as_i64) {
        rendered.push_str(&format!(" (code {code})"));
    }
    if let Some(data) = error.get("data").filter(|v| !v.is_null()) {
        let detail = data
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| data.to_string());
        if !detail.is_empty() {
            rendered.push_str(": ");
            rendered.push_str(&detail);
        }
    }
    rendered
}

/// A client suspended by [`RpcClient::freeze`]: the writer is paused, the
/// reader parked at a line boundary, and the loop's unhandled messages taken
/// out. Dropping it THAWS the client (see the module docs).
pub(crate) struct RpcFrozen {
    /// The stdin descriptor, for the manifest (`None` where there is none).
    pub stdin_fd: Option<i32>,
    /// The stdout descriptor, for the manifest (`None` where there is none).
    pub stdout_fd: Option<i32>,
    /// The unhandled messages as lines, oldest first, then every byte read
    /// off stdout but not yet parsed. Feed it to [`RpcClient::from_parts`].
    pub leftover: Vec<u8>,
    /// The id the next request must carry; ids already used may still be
    /// answered late and must never be reused.
    pub next_id: i64,
    /// Requests of ours still awaiting their responses, oldest first (only
    /// from [`RpcClient::freeze_carrying_requests`]; otherwise empty).
    pub in_flight: Vec<InFlight>,
    writer: Option<PausedWriter>,
    reader: ReaderGuard,
    closed: Arc<AtomicBool>,
}

impl RpcFrozen {
    /// A same-process successor took over the pipes: end both tasks WITHOUT
    /// closing the descriptors, and fail any later request on this client.
    /// The exec path never calls this. Returns once both have let go.
    pub async fn abandon(mut self) {
        self.closed.store(true, Ordering::Release);
        if let Some(writer) = self.writer.take() {
            writer.abandon().await;
        }
        self.reader.abandon().await;
    }
}

impl std::fmt::Debug for RpcFrozen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcFrozen")
            .field("stdin_fd", &self.stdin_fd)
            .field("stdout_fd", &self.stdout_fd)
            .field("leftover_bytes", &self.leftover.len())
            .field("next_id", &self.next_id)
            .field("in_flight", &self.in_flight)
            .finish()
    }
}

/// A freeze's hold on the parked reader. Dropping it resumes the reader,
/// which first redelivers `redeliver` (in order) to the loop.
struct ReaderGuard {
    resume: Option<oneshot::Sender<ReaderResume>>,
    redeliver: VecDeque<Incoming>,
}

impl ReaderGuard {
    async fn abandon(&mut self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if let Some(resume) = self.resume.take()
            && resume.send(ReaderResume::Abandon(ack_tx)).is_ok()
        {
            let _ = ack_rx.await;
        }
    }
}

impl Drop for ReaderGuard {
    fn drop(&mut self) {
        if let Some(resume) = self.resume.take() {
            let _ = resume.send(ReaderResume::Thaw(std::mem::take(&mut self.redeliver)));
        }
    }
}

enum ReaderCtl {
    /// Stop at a line boundary and park; reply with what the reader holds.
    Stop(oneshot::Sender<ParkedReader>),
}

/// What a parked reader hands a freeze.
struct ParkedReader {
    stdout_fd: Option<i32>,
    /// Bytes read but not yet parsed into lines.
    leftover: Vec<u8>,
    /// Parsed messages not yet delivered (a copy: the reader keeps its own
    /// until told what to redeliver, so an abandoned freeze loses nothing).
    backlog: VecDeque<Incoming>,
    resume: oneshot::Sender<ReaderResume>,
}

enum ReaderResume {
    /// Carry on, first delivering these (the loop's unhandled messages,
    /// then the reader's own backlog) in order.
    Thaw(VecDeque<Incoming>),
    /// A same-process successor owns stdout: let go without closing it.
    Abandon(oneshot::Sender<()>),
}

/// Parse stdout lines: responses resolve the pending map, everything else is
/// forwarded in order. Non-JSON noise is skipped; on EOF all pending requests
/// fail (their senders drop) and one final [`Incoming::Eof`] is delivered.
///
/// Stoppable for a freeze at any await point without losing a byte or a
/// message: reading is cancel-safe ([`LineReader`]) and a parsed message waits
/// in `backlog` until the channel has room for it.
async fn read_loop<R>(
    mut lines: LineReader<R>,
    pending: Pending,
    tx: mpsc::Sender<Incoming>,
    closed: Arc<AtomicBool>,
    observer: Option<StdoutObserver>,
    mut ctl: mpsc::UnboundedReceiver<ReaderCtl>,
) where
    R: AsyncRead + PipeFd + Unpin,
{
    let mut backlog: VecDeque<Incoming> = VecDeque::new();
    let mut ctl_open = true;
    let mut at_eof = false;
    loop {
        if at_eof && backlog.is_empty() {
            return;
        }
        tokio::select! {
            biased;
            request = ctl.recv(), if ctl_open => match request {
                None => ctl_open = false,
                // After EOF there is nothing left to hand over: dropping the
                // reply refuses the freeze.
                Some(ReaderCtl::Stop(_)) if at_eof => {}
                Some(ReaderCtl::Stop(reply)) => {
                    let (resume_tx, resume_rx) = oneshot::channel();
                    let parked = ParkedReader {
                        stdout_fd: lines.get_ref().pipe_fd(),
                        leftover: lines.leftover().to_vec(),
                        backlog: backlog.clone(),
                        resume: resume_tx,
                    };
                    if reply.send(parked).is_err() {
                        continue; // the freeze gave up before the answer
                    }
                    match resume_rx.await {
                        Ok(ReaderResume::Thaw(redeliver)) => backlog = redeliver,
                        Ok(ReaderResume::Abandon(ack)) => {
                            lines.into_parts().0.leak_pipe();
                            let _ = ack.send(());
                            return;
                        }
                        // The freeze went away before taking anything out of
                        // the channel: carry on with our own backlog.
                        Err(_) => {}
                    }
                }
            },
            permit = tx.reserve(), if !backlog.is_empty() => match permit {
                Ok(permit) => permit.send(backlog.pop_front().expect("guarded")),
                Err(_) => return, // the loop is gone
            },
            line = lines.next_line(), if backlog.is_empty() && !at_eof => match line {
                Ok(Some(line)) => {
                    if let Some(message) = parse_line(&line, &pending, observer.as_ref()) {
                        backlog.push_back(message);
                    }
                }
                // A read error ends the stream like EOF: either way the
                // child's stdout is unusable, pending requests must fail, and
                // the session loop must know.
                Ok(None) | Err(_) => {
                    closed.store(true, Ordering::Release);
                    pending.lock().expect("pending lock").clear();
                    backlog.push_back(Incoming::Eof);
                    at_eof = true;
                }
            },
        }
    }
}

/// One stdout line: a response resolves its caller (`None`), anything the
/// loop must see is returned.
fn parse_line(
    line: &str,
    pending: &Pending,
    observer: Option<&StdoutObserver>,
) -> Option<Incoming> {
    let line = line.trim();
    if let Some(url) = line.strip_prefix("Open the following link to authenticate the ACP server: ")
    {
        if let Some(observer) = observer {
            observer(url);
        }
        return None;
    }
    if line.is_empty() {
        return None;
    }
    let Ok(mut msg) = serde_json::from_str::<Value>(line) else {
        tracing::debug!(target: "zeron_harness::rpc", "non-JSON stdout line (skipped)");
        return None;
    };
    if !msg.is_object() || msg.get("jsonrpc").is_some_and(|version| version != "2.0") {
        return None;
    }
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id");
    let params = |msg: &mut Value| {
        msg.get_mut("params")
            .map(Value::take)
            .unwrap_or(Value::Null)
    };
    match (method, id) {
        // Response: resolve the awaiting request.
        (None, Some(id)) => {
            if msg.get("result").is_none() && msg.get("error").is_none() {
                return None;
            }
            let sender = pending
                .lock()
                .expect("pending lock")
                .remove(&response_id(id)?)?
                .reply;
            let outcome = match msg.get("error") {
                Some(err) => Err(response_error(err)),
                None => Ok(msg
                    .get_mut("result")
                    .map(Value::take)
                    .unwrap_or(Value::Null)),
            };
            let _ = sender.send(outcome);
            None
        }
        // Server→client request (approvals).
        (Some(method), Some(id)) => Some(Incoming::Request {
            id: id.clone(),
            method: method.to_owned(),
            params: params(&mut msg),
        }),
        // Notification.
        (Some(method), None) => Some(Incoming::Notification {
            method: method.to_owned(),
            params: params(&mut msg),
        }),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_notification_wire_has_no_id() {
        let (writer, mut receiver) = mpsc::unbounded_channel();
        let client = RpcClient {
            next_id: Arc::new(AtomicI64::new(0)),
            pending: Arc::default(),
            writer,
            closed: Arc::new(AtomicBool::new(false)),
            reader: mpsc::unbounded_channel().0,
        };
        client.notify("session/cancel", Some(json!({"sessionId": "parent"})));
        let Ok(WriteMsg::Line(line)) = receiver.try_recv() else {
            panic!("one line queued");
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            frame,
            json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "parent"}})
        );
        assert!(frame.get("id").is_none());
        assert!(client.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn requests_after_eof_fail_without_entering_pending_map() {
        let (writer, mut receiver) = mpsc::unbounded_channel();
        let client = RpcClient {
            next_id: Arc::new(AtomicI64::new(0)),
            pending: Arc::default(),
            writer,
            closed: Arc::new(AtomicBool::new(true)),
            reader: mpsc::unbounded_channel().0,
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            client.request("session/prompt", json!({})),
        )
        .await
        .expect("a request after EOF cannot wait for another EOF");
        assert!(result.unwrap_err().to_string().contains("exited"));
        assert!(client.pending.lock().unwrap().is_empty());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn rpc_error_preserves_string_and_structured_details() {
        for data in [
            json!("A prompt is already running"),
            json!({"reason": "A prompt is already running"}),
            json!(["busy", 7]),
        ] {
            let rendered = response_error(
                &json!({"code": -32600, "message": "Invalid request", "data": data}),
            );
            assert!(rendered.starts_with("Invalid request (code -32600): "));
            assert!(rendered.ends_with(data.as_str().unwrap_or(&data.to_string())));
        }
    }

    // ---- live-update freeze ------------------------------------------------

    use crate::FreezeRefusal;
    use crate::handoff::PipeFd;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    impl PipeFd for DuplexStream {
        fn pipe_fd(&self) -> Option<i32> {
            None
        }
        fn leak_pipe(self) {}
    }

    /// The agent's end of a client's stdio.
    struct Server {
        /// What the client wrote (its stdin).
        lines: tokio::io::Lines<BufReader<DuplexStream>>,
        /// What the client reads (its stdout).
        out: DuplexStream,
    }

    impl Server {
        async fn expect_line(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
                .await
                .expect("the client writes a line")
                .unwrap()
                .expect("stdin open");
            serde_json::from_str(&line).unwrap()
        }

        async fn nothing_written(&mut self) {
            assert!(
                tokio::time::timeout(Duration::from_millis(150), self.lines.next_line())
                    .await
                    .is_err(),
                "the writer is paused"
            );
        }

        async fn write(&mut self, bytes: &[u8]) {
            self.out.write_all(bytes).await.unwrap();
            self.out.flush().await.unwrap();
        }

        async fn send(&mut self, frame: Value) {
            self.write(format!("{frame}\n").as_bytes()).await;
        }

        async fn notifications(&mut self, range: std::ops::Range<usize>) {
            let mut all = String::new();
            for n in range {
                all.push_str(&format!(
                    "{}\n",
                    json!({"jsonrpc": "2.0", "method": format!("n{n}"), "params": {"n": n}})
                ));
            }
            self.write(all.as_bytes()).await;
        }
    }

    fn pair_from(leftover: Vec<u8>, next_id: i64) -> (RpcClient, mpsc::Receiver<Incoming>, Server) {
        let (client_in, server_in) = tokio::io::duplex(1 << 20);
        let (server_out, client_out) = tokio::io::duplex(1 << 20);
        let (client, incoming) = RpcClient::from_parts(client_in, client_out, leftover, next_id);
        let server = Server {
            lines: BufReader::new(server_in).lines(),
            out: server_out,
        };
        (client, incoming, server)
    }

    /// A carried request's response can already be buffered when the adopter's
    /// reader starts (the old image read it off the pipe but had not handled it
    /// yet). The request must be registered BEFORE the reader runs, or the
    /// response is read and discarded as unknown and the waiter hangs forever.
    /// (Tokio's scheduling usually hides the ordering bug, so this pins the
    /// contract — carried ids are waiting before the reader exists — more than
    /// it reproduces a failure.)
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_carried_request_whose_response_is_already_buffered_still_resolves() {
        for round in 0..50 {
            let leftover = format!(
                "{}\n",
                json!({"jsonrpc": "2.0", "id": 4, "result": {"round": round}})
            )
            .into_bytes();
            let (client_in, _server_in) = tokio::io::duplex(1 << 16);
            let (_server_out, client_out) = tokio::io::duplex(1 << 16);
            let carried = [
                InFlight {
                    id: 4,
                    method: "session/prompt".into(),
                },
                InFlight {
                    id: 9,
                    method: "too/new".into(),
                },
            ];
            let (_client, _incoming, mut calls) =
                RpcClient::from_parts_carrying(client_in, client_out, leftover, 6, None, &carried);
            let too_new = calls.pop().unwrap();
            let prompt = calls.pop().unwrap();
            let result = tokio::time::timeout(Duration::from_secs(5), prompt)
                .await
                .expect("the buffered response reaches its waiter")
                .expect("a result");
            assert_eq!(result["round"], round);
            // An id this client could still issue itself cannot be carried.
            assert!(too_new.await.is_err());
        }
    }

    fn rpc_pair() -> (RpcClient, mpsc::Receiver<Incoming>, Server) {
        pair_from(Vec::new(), 1)
    }

    async fn recv(incoming: &mut mpsc::Receiver<Incoming>) -> Incoming {
        tokio::time::timeout(Duration::from_secs(5), incoming.recv())
            .await
            .expect("a message arrives")
            .expect("channel open")
    }

    fn method(message: &Incoming) -> &str {
        match message {
            Incoming::Notification { method, .. } | Incoming::Request { method, .. } => method,
            Incoming::Eof => "<eof>",
        }
    }

    async fn answer(server: &mut Server, result: Value) -> Value {
        let request = server.expect_line().await;
        server
            .send(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
            .await;
        request
    }

    #[tokio::test]
    async fn freeze_refuses_while_a_request_is_in_flight() {
        let (client, mut incoming, mut server) = rpc_pair();
        let call = tokio::spawn({
            let client = client.clone();
            async move { client.request("m", json!({})).await }
        });
        let request = server.expect_line().await;
        assert!(matches!(
            client.freeze(&mut incoming).await,
            Err(FreezeRefusal::Busy(_))
        ));
        // Refusing changed nothing: the response still resolves the caller.
        server
            .send(json!({"jsonrpc": "2.0", "id": request["id"], "result": {"ok": true}}))
            .await;
        assert_eq!(call.await.unwrap().unwrap(), json!({"ok": true}));
        let frozen = client
            .freeze(&mut incoming)
            .await
            .expect("nothing in flight");
        drop(frozen);
    }

    #[tokio::test]
    async fn next_id_continues_after_adoption_so_late_responses_cannot_collide() {
        let (client, mut incoming, mut server) = rpc_pair();
        for _ in 0..3 {
            let call = tokio::spawn({
                let client = client.clone();
                async move { client.request("m", json!({})).await }
            });
            answer(&mut server, json!({})).await;
            call.await.unwrap().unwrap();
        }
        assert_eq!(client.next_request_id(), 4);
        let frozen = client.freeze(&mut incoming).await.unwrap();
        assert_eq!(frozen.next_id, 4);

        let (adopted, _incoming2, mut server2) = pair_from(frozen.leftover.clone(), frozen.next_id);
        assert_eq!(adopted.next_request_id(), 4);
        let call = tokio::spawn({
            let adopted = adopted.clone();
            async move { adopted.request("after", json!({})).await }
        });
        let request = server2.expect_line().await;
        assert_eq!(request["id"], 4, "ids continue: {request}");
        // A late answer to the predecessor's last request resolves nothing.
        server2
            .send(json!({"jsonrpc": "2.0", "id": 3, "result": "stale"}))
            .await;
        server2
            .send(json!({"jsonrpc": "2.0", "id": 4, "result": "fresh"}))
            .await;
        assert_eq!(call.await.unwrap().unwrap(), json!("fresh"));
    }

    #[tokio::test]
    async fn freeze_exports_unprocessed_messages_ahead_of_the_leftover_in_order() {
        let (client, mut incoming, mut server) = rpc_pair();
        // More than the incoming channel holds: the reader is blocked on a
        // full channel when the freeze arrives, and must still stop.
        server.notifications(0..300).await;
        server
            .send(json!({"jsonrpc": "2.0", "id": "srv-1", "method": "ask", "params": {"q": 1}}))
            .await;
        server.write(br#"{"jsonrpc": "2.0", "method": "la"#).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        // The loop handled the first two before freezing.
        assert_eq!(method(&recv(&mut incoming).await), "n0");
        assert_eq!(method(&recv(&mut incoming).await), "n1");

        let frozen = client.freeze(&mut incoming).await.expect("safe point");
        assert!(
            frozen
                .leftover
                .ends_with(br#"{"jsonrpc": "2.0", "method": "la"#),
            "the partial line comes last"
        );

        // The successor replays every unprocessed message, in order, then the
        // partial line completes from the pipe.
        let (_adopted, mut incoming2, mut server2) =
            pair_from(frozen.leftover.clone(), frozen.next_id);
        server2.write(b"te\"}\n").await;
        for n in 2..300 {
            let message = recv(&mut incoming2).await;
            let Incoming::Notification { method, params } = message else {
                panic!("n{n}: {message:?}");
            };
            assert_eq!(
                (method.as_str(), &params),
                (format!("n{n}").as_str(), &json!({"n": n}))
            );
        }
        let Incoming::Request { id, method, params } = recv(&mut incoming2).await else {
            panic!("the server request survives");
        };
        assert_eq!(
            (id, method.as_str(), params),
            (json!("srv-1"), "ask", json!({"q": 1}))
        );
        assert_eq!(method_of(recv(&mut incoming2).await), "late");
    }

    fn method_of(message: Incoming) -> String {
        method(&message).to_owned()
    }

    #[tokio::test]
    async fn a_thaw_redelivers_unprocessed_messages_in_order_and_resumes_both_directions() {
        let (client, mut incoming, mut server) = rpc_pair();
        server.notifications(0..300).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(method(&recv(&mut incoming).await), "n0");
        let frozen = client.freeze(&mut incoming).await.expect("safe point");
        // Written while frozen: held by the paused writer, not lost.
        client.notify("during", None);
        server.nothing_written().await;
        drop(frozen); // thaw
        assert_eq!(server.expect_line().await["method"], "during");
        server.notifications(300..301).await;
        for n in 1..301 {
            assert_eq!(method_of(recv(&mut incoming).await), format!("n{n}"));
        }
        // The pending table and id counter were untouched.
        let call = tokio::spawn({
            let client = client.clone();
            async move { client.request("m", json!({})).await }
        });
        let request = answer(&mut server, json!("ok")).await;
        assert_eq!(request["id"], 1);
        assert_eq!(call.await.unwrap().unwrap(), json!("ok"));
        // And it can freeze again.
        drop(client.freeze(&mut incoming).await.expect("again"));
    }

    #[tokio::test]
    async fn freeze_refuses_once_the_agent_output_has_closed() {
        let (client, mut incoming, server) = rpc_pair();
        drop(server);
        assert!(matches!(recv(&mut incoming).await, Incoming::Eof));
        assert!(matches!(
            client.freeze(&mut incoming).await,
            Err(FreezeRefusal::Busy(_))
        ));
    }

    #[tokio::test]
    async fn a_dropped_request_future_leaves_nothing_pending() {
        // A caller that gives up on a request (ACP abandons a steering call
        // after a bound) must not leave its id behind: `freeze` would refuse
        // until a response that may never come.
        let (client, mut incoming, mut server) = rpc_pair();
        let call = client.request_now("m", json!({}));
        server.expect_line().await;
        drop(call);
        assert!(client.pending.lock().unwrap().is_empty());
        drop(
            client
                .freeze(&mut incoming)
                .await
                .expect("nothing in flight"),
        );
        // Never polled at all: same.
        drop(client.request_now("n", json!({})));
        assert!(client.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_carrying_freeze_exports_in_flight_requests_and_a_successor_resolves_them_by_id() {
        let (client, mut incoming, mut server) = rpc_pair();
        let (first, _first_call) = client.request_with_id("session/prompt", json!({}));
        let (second, _second_call) = client.request_with_id("_session/steering", json!({}));
        assert_eq!(server.expect_line().await["id"], first);
        assert_eq!(server.expect_line().await["id"], second);
        // The plain freeze keeps refusing (Codex relies on it)...
        assert!(matches!(
            client.freeze(&mut incoming).await,
            Err(FreezeRefusal::Busy(_))
        ));
        // ...the carrying one exports them, oldest first.
        let frozen = client
            .freeze_carrying_requests(&mut incoming)
            .await
            .expect("requests in flight are carried");
        assert_eq!(
            frozen.in_flight,
            [
                InFlight {
                    id: first,
                    method: "session/prompt".into()
                },
                InFlight {
                    id: second,
                    method: "_session/steering".into()
                },
            ]
        );
        assert_eq!(frozen.next_id, 3);

        // The agent answers while nobody reads; the successor picks the
        // response up from the pipe and routes it to the re-registered call.
        let (adopted, _incoming2, mut server2) = pair_from(frozen.leftover.clone(), frozen.next_id);
        let resumed = adopted.resume_request(&frozen.in_flight[0]);
        server2
            .send(json!({"jsonrpc": "2.0", "id": first, "result": {"stopReason": "end_turn"}}))
            .await;
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), resumed)
                .await
                .expect("the carried request resolves")
                .unwrap(),
            json!({"stopReason": "end_turn"})
        );
        // A request the successor did not resume is answered to nobody.
        server2
            .send(json!({"jsonrpc": "2.0", "id": second, "result": {}}))
            .await;
        let call = tokio::spawn({
            let adopted = adopted.clone();
            async move { adopted.request("after", json!({})).await }
        });
        let request = answer(&mut server2, json!("fresh")).await;
        assert_eq!(request["id"], 3, "ids continue past the carried ones");
        assert_eq!(call.await.unwrap().unwrap(), json!("fresh"));
    }

    #[tokio::test]
    async fn a_thawed_carrying_freeze_leaves_in_flight_requests_with_their_callers() {
        let (client, mut incoming, mut server) = rpc_pair();
        let call = tokio::spawn({
            let client = client.clone();
            async move { client.request("session/prompt", json!({})).await }
        });
        let request = server.expect_line().await;
        let frozen = client
            .freeze_carrying_requests(&mut incoming)
            .await
            .expect("carried");
        assert_eq!(frozen.in_flight.len(), 1);
        server
            .send(json!({"jsonrpc": "2.0", "id": request["id"], "result": "late"}))
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!call.is_finished(), "the parked reader does not parse");
        drop(frozen); // thaw
        assert_eq!(call.await.unwrap().unwrap(), json!("late"));
    }

    #[tokio::test]
    async fn resuming_refuses_ids_this_client_could_issue_or_already_waits_for() {
        let (client, _incoming, _server) = pair_from(Vec::new(), 5);
        let (id, _call) = client.request_with_id("m", json!({}));
        assert_eq!(id, 5);
        for bad in [
            InFlight {
                id: 5,
                method: "m".into(),
            },
            InFlight {
                id: 6,
                method: "m".into(),
            },
        ] {
            let result =
                tokio::time::timeout(Duration::from_millis(100), client.resume_request(&bad))
                    .await
                    .expect("refused at once");
            assert!(result.is_err(), "{bad:?}");
        }
        let _ok = client.resume_request(&InFlight {
            id: 4,
            method: "m".into(),
        });
        assert_eq!(client.pending.lock().unwrap().len(), 2);
    }

    #[test]
    fn rpc_error_handles_missing_null_and_unstructured_fields() {
        for error in [
            json!({"message": "busy"}),
            json!({"message": "busy", "data": null}),
            json!({"message": "busy", "data": ""}),
        ] {
            assert_eq!(response_error(&error), "busy");
        }
        for error in [
            json!({"code": -32600, "data": {"reason": "busy"}}),
            json!("unstructured error"),
        ] {
            assert_eq!(response_error(&error), error.to_string());
        }
    }
}
