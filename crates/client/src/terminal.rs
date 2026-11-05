//! Terminals on a chat's host: the engine's PTY sessions, carried by the hub
//! every viewer runs (`zeron_term_link::remote`, the desktop engine's too):
//!
//! - **Direct when it can be.** A terminal starts on a direct WebRTC link to
//!   its host when one is up, else on the relay while pairing runs beside it,
//!   then moves onto the direct link mid-stream — and back to the relay if
//!   the link dies — resuming after the last output it showed: no gap, no
//!   repeat. A network change or the app coming back to the foreground
//!   re-pairs at once ([`Client::network_path_changed`], [`Client::on_foreground`]).
//! - **No keystroke lost or doubled.** Input is numbered and held until the
//!   host acknowledges it; whichever path takes over replays what's pending
//!   and the host applies each number once. Writes never wait on the network.
//! - **The stream outlives the network.** When every path drops, it resumes
//!   from the last sequence seen (the host keeps a replay window and the
//!   shell outlives its viewers).

use std::sync::{Arc, Weak};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use zeron_proto::TerminalEvent;
use zeron_rpc::{RpcError, methods};
use zeron_term_link::remote::{Direct, Relay, TerminalPath, Write};

use crate::client::{Client, ClientInner};
use crate::error::{ClientError, Result};
use crate::live::b64;

/// An open terminal and where it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalInfo {
    pub id: String,
    pub device_id: String,
    pub cwd: String,
    /// Shell basename (`zsh`, `bash`, …).
    pub shell: String,
    /// The host answers the terminal's queries (device attributes, cursor
    /// position…) itself. A host from before that leaves them to the
    /// viewer, which then writes its emulator's answers back.
    pub host_replies: bool,
}

/// The host takes at most this much input per write (`MAX_INPUT_BYTES`).
const MAX_WRITE: usize = 64 * 1024;

/// What a link reports as it runs.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalUpdate {
    /// The output stream is (re)attached.
    Connected,
    /// The path carrying the terminal now.
    Path(TerminalPath),
    /// PTY output, raw bytes (not necessarily valid UTF-8).
    Output(Vec<u8>),
    /// The shell exited; the stream is over.
    Exited { code: i32, signal: Option<String> },
    /// Every path dropped; the next attempt is `retry_in_ms` away (or
    /// sooner: the network coming back, the app back in front).
    Reconnecting { retry_in_ms: u64 },
    /// The host no longer has this terminal (it exited long ago, or the
    /// host restarted).
    Gone,
}

/// Receives a link's updates on the client runtime.
pub trait TerminalSink: Send + Sync {
    fn update(&self, update: TerminalUpdate);
}

/// The relay path: host RPCs over the device relay (or the demo host).
struct ClientRelay(Weak<ClientInner>);

fn rpc_error(error: ClientError) -> RpcError {
    match error {
        ClientError::HostError(message) => RpcError::Failed(message),
        ClientError::Unsupported(message) => RpcError::UnknownMethod(message),
        other => RpcError::Transport(other.to_string()),
    }
}

#[async_trait::async_trait]
impl Relay for ClientRelay {
    async fn call(
        &self,
        target: &str,
        method: &str,
        params: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, RpcError> {
        let inner = self.0.upgrade().ok_or(RpcError::Closed)?;
        inner
            .host_rpc(target, method, params)
            .await
            .map_err(rpc_error)
    }

    async fn subscribe(
        &self,
        target: &str,
        method: &str,
        params: serde_json::Value,
    ) -> std::result::Result<BoxStream<'static, serde_json::Value>, RpcError> {
        let inner = self.0.upgrade().ok_or(RpcError::Closed)?;
        let stream = inner
            .host_subscribe(target, method, params)
            .await
            .map_err(rpc_error)?;
        Ok(futures::stream::unfold(stream, |mut stream| async move {
            stream.recv().await.map(|item| (item, stream))
        })
        .boxed())
    }
}

/// The paths for a terminal on `inner`'s client.
fn paths(inner: &Arc<ClientInner>) -> (Arc<dyn Relay>, Option<Arc<dyn Direct>>) {
    let relay: Arc<dyn Relay> = Arc::new(ClientRelay(Arc::downgrade(inner)));
    let direct = inner
        .live()
        .and_then(|live| live.direct())
        .map(|peers| Arc::new(peers.clone()) as Arc<dyn Direct>);
    (relay, direct)
}

/// One attached terminal. Dropping it detaches (the shell keeps running on
/// the host); [`TerminalLink::close`] ends it — the close goes out even if
/// the link is dropped right after.
pub struct TerminalLink {
    info: TerminalInfo,
    inner: Weak<ClientInner>,
    relay: Arc<dyn Relay>,
    stream: CancellationToken,
}

/// Writes the terminal's own query answers back (hosts without host
/// replies). Doesn't keep the link alive.
#[derive(Clone)]
pub struct TerminalReplies {
    info: TerminalInfo,
    inner: Weak<ClientInner>,
    relay: Arc<dyn Relay>,
}

impl TerminalReplies {
    pub fn write(&self, bytes: Vec<u8>) {
        if let Some(inner) = self.inner.upgrade() {
            write(&inner, &self.relay, &self.info, bytes);
        }
    }
}

/// Queue input for the host, in typed order, never waiting on the network.
fn write(inner: &Arc<ClientInner>, relay: &Arc<dyn Relay>, info: &TerminalInfo, bytes: Vec<u8>) {
    // The hub spawns its relay writer on first use.
    let _runtime = crate::runtime::handle().enter();
    for chunk in bytes.chunks(MAX_WRITE).filter(|c| !c.is_empty()) {
        let _ = inner.remote_terminals.send(
            relay,
            &info.device_id,
            &info.id,
            Write::Input(chunk.to_vec()),
        );
    }
}

impl TerminalLink {
    pub fn info(&self) -> &TerminalInfo {
        &self.info
    }

    /// Bytes for the shell (typed keys, a paste). Queued in order.
    pub fn write(&self, bytes: Vec<u8>) {
        if let Some(inner) = self.inner.upgrade() {
            write(&inner, &self.relay, &self.info, bytes);
        }
    }

    /// The viewport's grid size changed.
    pub fn resize(&self, cols: u16, rows: u16) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let _runtime = crate::runtime::handle().enter();
        let _ = inner.remote_terminals.send(
            &self.relay,
            &self.info.device_id,
            &self.info.id,
            Write::Resize {
                cols: cols.max(2),
                rows: rows.max(1),
            },
        );
    }

    /// End the shell on the host. The stream reports the exit when it lands.
    pub fn close(&self) {
        let (relay, info, weak) = (self.relay.clone(), self.info.clone(), self.inner.clone());
        crate::runtime::shared().spawn(async move {
            let params = json!({ "terminalId": info.id });
            for attempt in 0..5u64 {
                if attempt > 0 {
                    tokio::time::sleep(Duration::from_millis(300 * attempt)).await;
                }
                match relay
                    .call(&info.device_id, methods::CLOSE_TERMINAL, params.clone())
                    .await
                {
                    Ok(_) | Err(RpcError::Failed(_) | RpcError::UnknownMethod(_)) => break,
                    Err(_) => {}
                }
            }
            if let Some(inner) = weak.upgrade() {
                inner.remote_terminals.ended(&info.device_id, &info.id);
            }
        });
    }

    /// For answering the terminal's queries when the host doesn't.
    pub fn replies(&self) -> TerminalReplies {
        TerminalReplies {
            info: self.info.clone(),
            inner: self.inner.clone(),
            relay: self.relay.clone(),
        }
    }
}

impl Drop for TerminalLink {
    /// Detach: the output stream stops (the shell keeps running on the host).
    fn drop(&mut self) {
        self.stream.cancel();
    }
}

impl Client {
    /// Open a shell for `chat_id` on its host, in the chat's working folder.
    /// Pairing with the host starts alongside, so the terminal's stream
    /// usually begins on the direct link.
    pub async fn open_terminal(&self, chat_id: &str, cols: u16, rows: u16) -> Result<TerminalInfo> {
        let device_id = self.chat_host(chat_id)?;
        if let Some(direct) = self.inner.live().and_then(|live| live.direct()) {
            direct.warm(&device_id);
        }
        let reply = self
            .inner
            .host_rpc(
                &device_id,
                methods::OPEN_TERMINAL,
                json!({ "chatId": chat_id, "cols": cols.max(2), "rows": rows.max(1) }),
            )
            .await?;
        let session: zeron_proto::TerminalSession = serde_json::from_value(reply)
            .map_err(|e| ClientError::HostError(format!("OpenTerminal: {e}")))?;
        Ok(TerminalInfo {
            id: session.id,
            device_id,
            cwd: session.cwd,
            shell: session.shell,
            host_replies: session.host_replies,
        })
    }

    /// Attach to an open terminal: replays everything after `after_seq`
    /// (0 = the host's whole replay window), then follows live output.
    pub fn attach_terminal(
        &self,
        info: TerminalInfo,
        after_seq: u64,
        sink: Arc<dyn TerminalSink>,
    ) -> TerminalLink {
        let stream = CancellationToken::new();
        let (relay, direct) = paths(&self.inner);
        if let Some(direct) = self.inner.live().and_then(|live| live.direct()) {
            direct.warm(&info.device_id);
        }
        crate::runtime::shared().spawn(follow(
            Arc::downgrade(&self.inner),
            relay.clone(),
            direct,
            info.clone(),
            after_seq,
            sink,
            stream.clone(),
        ));
        TerminalLink {
            info,
            inner: Arc::downgrade(&self.inner),
            relay,
            stream,
        }
    }

    /// The device hosting `chat_id`.
    pub(crate) fn chat_host(&self, chat_id: &str) -> Result<String> {
        self.inner
            .workspace
            .chat(chat_id)
            .map(|c| c.device_id)
            .ok_or_else(|| ClientError::NotFound(chat_id.to_owned()))
    }
}

/// Follow the terminal over the hub, resubscribing from the last sequence
/// whenever every path drops (0.4s → 5s, cut short by the network coming
/// back or the app returning to the foreground).
async fn follow(
    weak: Weak<ClientInner>,
    relay: Arc<dyn Relay>,
    direct: Option<Arc<dyn Direct>>,
    info: TerminalInfo,
    mut last_seq: u64,
    sink: Arc<dyn TerminalSink>,
    cancel: CancellationToken,
) {
    let mut backoff = Duration::from_millis(400);
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let hub = inner.remote_terminals.clone();
        let wake = inner.terminals_wake.clone();
        drop(inner);
        let subscribe = hub.subscribe_tracked(
            relay.clone(),
            direct.clone(),
            &info.device_id,
            &info.id,
            last_seq,
            info.host_replies,
        );
        let subscribed = tokio::select! {
            _ = cancel.cancelled() => return,
            subscribed = subscribe => subscribed,
        };
        match subscribed {
            Ok((mut events, mut path)) => {
                sink.update(TerminalUpdate::Connected);
                sink.update(TerminalUpdate::Path(*path.borrow_and_update()));
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        changed = path.changed() => {
                            if changed.is_ok() {
                                sink.update(TerminalUpdate::Path(*path.borrow_and_update()));
                            }
                        }
                        item = events.next() => {
                            let Some(item) = item else { break };
                            backoff = Duration::from_millis(400);
                            match serde_json::from_value::<TerminalEvent>(item) {
                                Ok(TerminalEvent::Data { seq, data }) => {
                                    if seq <= last_seq {
                                        continue;
                                    }
                                    last_seq = seq;
                                    if let Some(bytes) = b64::decode(&data) {
                                        sink.update(TerminalUpdate::Output(bytes));
                                    }
                                }
                                Ok(TerminalEvent::Exit { exit_code, signal, .. }) => {
                                    sink.update(TerminalUpdate::Exited {
                                        code: exit_code,
                                        signal,
                                    });
                                    return;
                                }
                                Err(_) => {}
                            }
                        }
                    }
                }
            }
            Err(RpcError::Failed(message)) if message.contains("Terminal not found") => {
                sink.update(TerminalUpdate::Gone);
                return;
            }
            Err(RpcError::UnknownMethod(_)) => {
                sink.update(TerminalUpdate::Gone);
                return;
            }
            Err(_) => {}
        }
        sink.update(TerminalUpdate::Reconnecting {
            retry_in_ms: backoff.as_millis() as u64,
        });
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(backoff) => backoff = (backoff * 2).min(Duration::from_secs(5)),
            _ = wake.notified() => backoff = Duration::from_millis(400),
        }
    }
}
