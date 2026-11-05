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
//!   the TS harness's swallowed-EPIPE behavior — but they close the client:
//!   every pending request fails at once instead of waiting for stdout EOF,
//!   which a descendant holding the pipe open can delay indefinitely.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

use crate::HarnessError;
use crate::process::{ChildStdin, ChildStdout};

/// A non-response line from the app server, in stdout order.
#[derive(Debug)]
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

type StdoutObserver = Box<dyn Fn(&str) + Send>;

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Result<Value, String>>>>>;

/// How a request fails when the transport lost its answer: the app server
/// exited first, or its stdin could not take the request (then with the
/// write error after it). [`transport_lost`] recognizes both.
const EXITED: &str = "app-server exited before responding";
const STDIN_CLOSED: &str = "app-server stdin closed";

/// The request never got an answer because the app server is gone, as
/// opposed to an answer that refused it.
pub(crate) fn transport_lost(error: &HarnessError) -> bool {
    let HarnessError::Protocol(message) = error else {
        return false;
    };
    message
        .split_once(": ")
        .is_some_and(|(_, reason)| reason.starts_with(EXITED) || reason.starts_with(STDIN_CLOSED))
}

/// Where realtime voice notifications go instead of the session channel.
struct VoiceRouter {
    sender: mpsc::Sender<Incoming>,
    overflow: Arc<AtomicBool>,
    abort_media: Box<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
pub(crate) struct RpcClient {
    next_id: Arc<AtomicI64>,
    pending: Pending,
    writer: mpsc::UnboundedSender<String>,
    closed: Arc<AtomicBool>,
    voice_router: Arc<Mutex<Option<VoiceRouter>>>,
    /// Ends the writer task (and with it the child's stdin) once the lines
    /// already queued are written, whichever clones are still alive.
    /// Only Windows' teardown needs it.
    #[cfg_attr(not(any(windows, test)), allow(dead_code))]
    close_stdin: Arc<tokio::sync::Notify>,
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
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<String>();
        let pending: Pending = Arc::default();
        let (incoming_tx, incoming_rx) = mpsc::channel(256);
        let closed = Arc::new(AtomicBool::new(false));
        let close_stdin = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(write_loop(
            stdin,
            writer_rx,
            Arc::clone(&pending),
            closed.clone(),
            close_stdin.clone(),
        ));
        let voice_router = Arc::default();
        tokio::spawn(read_loop(
            stdout,
            Arc::clone(&pending),
            incoming_tx,
            closed.clone(),
            observer,
            Arc::clone(&voice_router),
        ));
        (
            Self {
                next_id: Arc::new(AtomicI64::new(0)),
                pending,
                writer: writer_tx,
                closed,
                voice_router,
                close_stdin,
            },
            incoming_rx,
        )
    }

    /// Route `thread/realtime/*` notifications (and `account/updated`) to a
    /// voice channel so stdout never waits for playout. Overflow is terminal:
    /// the flag is set and `abort_media` runs.
    pub fn subscribe_voice(
        &self,
        abort_media: impl Fn() + Send + Sync + 'static,
    ) -> (mpsc::Receiver<Incoming>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel(32);
        let overflow = Arc::new(AtomicBool::new(false));
        *self.voice_router.lock().expect("voice router") = Some(VoiceRouter {
            sender: tx,
            overflow: overflow.clone(),
            abort_media: Box::new(abort_media),
        });
        (rx, overflow)
    }

    /// End the subscription identified by its `overflow` flag; a newer one is
    /// left in place.
    pub fn unsubscribe_voice(&self, overflow: &Arc<AtomicBool>) {
        let mut router = self.voice_router.lock().expect("voice router");
        if router
            .as_ref()
            .is_some_and(|r| Arc::ptr_eq(&r.overflow, overflow))
        {
            *router = None;
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Call before [`crate::shutdown_child`] / [`crate::shutdown_agent`]:
    /// Windows' teardown gives the child its grace to exit on stdin EOF, and
    /// the writer task keeps stdin open while any clone of this client is
    /// alive. Unix teardown signals the child instead; its order is unchanged.
    pub fn close_stdin_for_teardown(&self) {
        #[cfg(windows)]
        self.close_stdin();
    }

    /// Close the child's stdin once the lines already queued are written.
    #[cfg(any(windows, test))]
    fn close_stdin(&self) {
        self.close_stdin.notify_one();
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
        let method = method.to_owned();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().expect("pending lock");
            // Check under the same lock as EOF cleanup: a request racing the
            // reader exit must either be rejected here or cleared by it.
            if self.is_closed() {
                return Box::pin(async move {
                    Err(HarnessError::Protocol(format!("{method}: {EXITED}")))
                });
            }
            pending.insert(id, tx);
        }
        let line = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if self.writer.send(line.to_string()).is_err() {
            self.pending.lock().expect("pending lock").remove(&id);
            return Box::pin(async move {
                Err(HarnessError::Protocol(format!("{method}: {STDIN_CLOSED}")))
            });
        }
        Box::pin(async move {
            match rx.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(message)) => Err(HarnessError::Protocol(format!("{method}: {message}"))),
                // Sender dropped: the reader hit EOF and failed all pending.
                Err(_) => Err(HarnessError::Protocol(format!("{method}: {EXITED}"))),
            }
        })
    }

    /// Fire a notification (no id, no response).
    pub fn notify(&self, method: &str, params: Option<Value>) {
        let line = match params {
            Some(params) => json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            None => json!({ "jsonrpc": "2.0", "method": method }),
        };
        let _ = self.writer.send(line.to_string());
    }

    /// Answer a server→client request.
    pub fn respond(&self, id: &Value, result: Value) {
        let line = json!({ "jsonrpc": "2.0", "id": id, "result": result });
        let _ = self.writer.send(line.to_string());
    }

    /// Reject a server→client request (e.g. unknown method).
    pub fn respond_error(&self, id: &Value, code: i64, message: &str) {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        });
        let _ = self.writer.send(line.to_string());
    }
}

/// Owns the child's stdin; a write failure (EPIPE after the child died) is
/// tolerated and logged. Nothing written after it can be answered, so the
/// client closes and every pending request fails now: a caller must not wait
/// on stdout EOF, which a surviving descendant can hold off forever.
async fn write_loop(
    mut stdin: ChildStdin,
    mut rx: mpsc::UnboundedReceiver<String>,
    pending: Pending,
    closed: Arc<AtomicBool>,
    close: Arc<tokio::sync::Notify>,
) {
    loop {
        // Queued lines first: a close only lands once they are written.
        let line = tokio::select! {
            biased;
            line = rx.recv() => line,
            () = close.notified() => None,
        };
        let Some(line) = line else { return };
        let write = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        };
        if let Err(e) = write.await {
            tracing::debug!(target: "zeron_harness::rpc", "stdin write failed (tolerated): {e}");
            fail_pending(&pending, &closed, &format!("{STDIN_CLOSED} ({e})"));
            return;
        }
    }
}

/// Close the client and reject every awaiting request with `reason`. Done
/// under the pending lock so a request racing it is either rejected by
/// [`RpcClient::request_now`]'s closed check or drained here.
fn fail_pending(pending: &Pending, closed: &AtomicBool, reason: &str) {
    let mut pending = pending.lock().expect("pending lock");
    closed.store(true, Ordering::Release);
    for (_, sender) in pending.drain() {
        let _ = sender.send(Err(reason.to_owned()));
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

/// Parse stdout lines: responses resolve the pending map, everything else is
/// forwarded in order. Non-JSON noise is skipped; on EOF all pending requests
/// fail (their senders drop) and one final [`Incoming::Eof`] is delivered.
async fn read_loop(
    stdout: ChildStdout,
    pending: Pending,
    tx: mpsc::Sender<Incoming>,
    closed: Arc<AtomicBool>,
    observer: Option<StdoutObserver>,
    voice_router: Arc<Mutex<Option<VoiceRouter>>>,
) {
    let mut lines = crate::lossy_lines::lossy_lines(stdout);
    // A read error ends the loop like EOF: either way the child's stdout is
    // unusable, pending requests must fail, and the session loop must know.
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim();
        if let Some(url) =
            line.strip_prefix("Open the following link to authenticate the ACP server: ")
        {
            if let Some(observer) = &observer {
                observer(url);
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let Ok(mut msg) = serde_json::from_str::<Value>(line) else {
            tracing::debug!(target: "zeron_harness::rpc", "non-JSON stdout line (skipped)");
            continue;
        };
        if !msg.is_object() || msg.get("jsonrpc").is_some_and(|version| version != "2.0") {
            continue;
        }
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id");
        match (method, id) {
            // Response: resolve the awaiting request.
            (None, Some(id)) => {
                // A bare `{jsonrpc, id}` is a JS server answering a void
                // method (`JSON.stringify` drops an undefined `result`);
                // skipping it would strand the caller, so it resolves as a
                // null result. A frame carrying anything else but neither
                // `result` nor `error` is diagnostic noise, not a response.
                let bare = msg
                    .as_object()
                    .is_some_and(|frame| frame.keys().all(|k| k == "jsonrpc" || k == "id"));
                if msg.get("result").is_none() && msg.get("error").is_none() && !bare {
                    continue;
                }
                let Some(id) = response_id(id) else { continue };
                let Some(sender) = pending.lock().expect("pending lock").remove(&id) else {
                    continue;
                };
                let outcome = match msg.get("error") {
                    Some(err) => Err(response_error(err)),
                    None => Ok(msg
                        .get_mut("result")
                        .map(Value::take)
                        .unwrap_or(Value::Null)),
                };
                let _ = sender.send(outcome);
            }
            // Server→client request (approvals).
            (Some(method), Some(id)) => {
                let incoming = Incoming::Request {
                    id: id.clone(),
                    method: method.to_owned(),
                    params: msg
                        .get_mut("params")
                        .map(Value::take)
                        .unwrap_or(Value::Null),
                };
                if tx.send(incoming).await.is_err() {
                    return;
                }
            }
            // Notification.
            (Some(method), None) => {
                let realtime = method.starts_with("thread/realtime/");
                let account_updated = method == "account/updated";
                let incoming = Incoming::Notification {
                    method: method.to_owned(),
                    params: msg
                        .get_mut("params")
                        .map(Value::take)
                        .unwrap_or(Value::Null),
                };
                if let Some(router) = voice_router.lock().expect("voice router").as_ref() {
                    if account_updated {
                        // An account change ends any call; the session still sees it below.
                        (router.abort_media)();
                        if let Incoming::Notification { method, params } = &incoming {
                            let copy = Incoming::Notification {
                                method: method.clone(),
                                params: params.clone(),
                            };
                            if router.sender.try_send(copy).is_err() {
                                router.overflow.store(true, Ordering::Release);
                            }
                        }
                    } else if realtime {
                        if router.sender.try_send(incoming).is_err() {
                            router.overflow.store(true, Ordering::Release);
                            (router.abort_media)();
                        }
                        continue;
                    }
                }
                if tx.send(incoming).await.is_err() {
                    return;
                }
            }
            (None, None) => {}
        }
    }
    // EOF/read error: fail every awaiting request, then signal the loop.
    closed.store(true, Ordering::Release);
    pending.lock().expect("pending lock").clear();
    if let Some(router) = voice_router.lock().expect("voice router").as_ref() {
        (router.abort_media)();
    }
    let _ = tx.send(Incoming::Eof).await;
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
            voice_router: Arc::default(),
            close_stdin: Arc::default(),
        };
        client.notify("session/cancel", Some(json!({"sessionId": "parent"})));
        let frame: Value = serde_json::from_str(&receiver.try_recv().unwrap()).unwrap();
        assert_eq!(
            frame,
            json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": "parent"}})
        );
        assert!(frame.get("id").is_none());
        assert!(client.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn stale_voice_unsubscribe_keeps_the_newer_subscription() {
        let client = RpcClient {
            next_id: Arc::new(AtomicI64::new(0)),
            pending: Arc::default(),
            writer: mpsc::unbounded_channel().0,
            closed: Arc::new(AtomicBool::new(false)),
            voice_router: Arc::default(),
            close_stdin: Arc::default(),
        };
        let (_old_rx, old) = client.subscribe_voice(|| {});
        let (_new_rx, new) = client.subscribe_voice(|| {});
        client.unsubscribe_voice(&old);
        assert!(client.voice_router.lock().unwrap().is_some());
        client.unsubscribe_voice(&new);
        assert!(client.voice_router.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn requests_after_eof_fail_without_entering_pending_map() {
        let (writer, mut receiver) = mpsc::unbounded_channel();
        let client = RpcClient {
            next_id: Arc::new(AtomicI64::new(0)),
            pending: Arc::default(),
            writer,
            closed: Arc::new(AtomicBool::new(true)),
            voice_router: Arc::default(),
            close_stdin: Arc::default(),
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

    /// A child speaking `script` over piped stdio, plus its client.
    #[cfg(unix)]
    fn scripted_child(
        script: &str,
    ) -> (tokio::process::Child, RpcClient, mpsc::Receiver<Incoming>) {
        let mut child = tokio::process::Command::new("sh")
            .args(["-c", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (client, incoming) =
            RpcClient::new(child.stdin.take().unwrap(), child.stdout.take().unwrap());
        (child, client, incoming)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn void_response_resolves_its_caller_with_null() {
        let (_child, client, _incoming) =
            scripted_child(r#"read line; echo '{"jsonrpc":"2.0","id":1}'; sleep 5"#);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            client.request("session/set_mode", json!({})),
        )
        .await
        .expect("a void response must not strand its caller");
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_write_fails_pending_requests_while_stdout_stays_open() {
        // The child closes stdin but keeps stdout open, like a crashed adapter
        // whose descendant still holds the pipe.
        let (_child, client, _incoming) = scripted_child("exec 0<&-; sleep 5");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            client.request("session/prompt", json!({})),
        )
        .await
        .expect("a failed write must fail the request without waiting for EOF");
        let error = result.unwrap_err();
        assert!(error.to_string().contains("stdin closed"));
        assert!(transport_lost(&error), "{error}");
        assert!(client.is_closed());
        assert!(client.pending.lock().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_lost_answer_reads_as_transport_lost() {
        // EOF with the request outstanding.
        let (_child, client, _incoming) = scripted_child("read line");
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            client.request("thread/start", json!({})),
        )
        .await
        .expect("EOF fails the request")
        .unwrap_err();
        assert!(transport_lost(&error), "{error}");
        // And once closed, a request is turned away the same way.
        let error = client.request("turn/start", json!({})).await.unwrap_err();
        assert!(transport_lost(&error), "{error}");
        for refused in [
            HarnessError::Protocol("thread/start: bad request (code -32600)".into()),
            HarnessError::Protocol(EXITED.into()),
            HarnessError::Io(std::io::Error::other(EXITED)),
        ] {
            assert!(!transport_lost(&refused), "{refused}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_stdin_writes_what_was_queued_then_sends_eof() {
        // Echoes stdin back and exits on EOF, even with a clone still alive.
        let (mut child, client, mut incoming) = scripted_child("cat");
        let clone = client.clone();
        client.notify("one", None);
        client.notify("two", None);
        client.close_stdin();
        let status = tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
            .await
            .expect("stdin EOF ends the child")
            .unwrap();
        assert!(status.success());
        for method in ["one", "two"] {
            match incoming.recv().await {
                Some(Incoming::Notification { method: got, .. }) => assert_eq!(got, method),
                other => panic!("expected {method}, got {other:?}"),
            }
        }
        drop(clone);
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
