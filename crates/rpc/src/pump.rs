//! Bounded WebSocket pump for RPC sessions (client dials and engine listeners).
//!
//! The rpc crate's twin of the sync crate's bounded session pumps (a3c370b5
//! "recover from stalled sockets with bounded session pumps" + efa1a075
//! "keep slow transfers progressing", tests from 94cc6753), specialized to
//! ndjson string frames because zeron-sync's progress internals are private.
//! Reads and writes own independent await
//! points, so a stalled write can neither block inbound frames nor disable
//! the silence lease; every operation is bounded by a lease that only byte
//! progress renews, and large frames are fragmented with interleaved pings
//! so a slowly draining peer can still signal liveness. Expiry closes the
//! entire session: callers recover by reconnecting (pending RPCs fail as
//! `Closed`), never by resuming a partial send.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{Sink, SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::frame::{
    Frame,
    coding::{Data, OpCode},
};

const PING_INTERVAL: Duration = Duration::from_secs(15);
const SILENCE_LEASE: Duration = Duration::from_secs(45);

// Control frames can pass between fragments even before the peer has the
// complete application message. This exposes a slowly draining TCP buffer
// through returning pongs without changing the application protocol.
const FRAGMENT_BYTES: usize = 4096;

struct Activity {
    origin: tokio::time::Instant,
    read_ms: AtomicU64,
    write_ms: AtomicU64,
    completed_write_ms: AtomicU64,
    writing: AtomicBool,
    blocked_write: AtomicBool,
}

/// Shared byte-activity clock. It sends no extra frames and allocates once
/// per connection; updates are relaxed atomic stores, not wake broadcasts.
#[derive(Clone)]
pub struct Progress(Arc<Activity>);
impl Progress {
    fn new() -> Self {
        Self(Arc::new(Activity {
            origin: tokio::time::Instant::now(),
            read_ms: AtomicU64::new(0),
            write_ms: AtomicU64::new(0),
            completed_write_ms: AtomicU64::new(0),
            writing: AtomicBool::new(false),
            blocked_write: AtomicBool::new(false),
        }))
    }
    fn record(&self, write: bool) {
        let clock = if write {
            &self.0.write_ms
        } else {
            &self.0.read_ms
        };
        let ms = self.0.origin.elapsed().as_millis() as u64;
        clock.store(ms, Ordering::Relaxed);
    }
    fn latest(&self, write_only: bool) -> tokio::time::Instant {
        let write = self.0.write_ms.load(Ordering::Relaxed);
        let ms = if write_only {
            write
        } else {
            let received = self.0.read_ms.load(Ordering::Relaxed);
            let completed = self.0.completed_write_ms.load(Ordering::Relaxed);
            received
                .max(completed)
                .max(if self.0.writing.load(Ordering::Relaxed) {
                    write
                } else {
                    0
                })
        };
        self.0.origin + Duration::from_millis(ms)
    }
    pub(crate) fn writing(&self) -> impl Drop + '_ {
        self.0.blocked_write.store(false, Ordering::Relaxed);
        self.0.writing.store(true, Ordering::Relaxed);
        struct Guard<'a>(&'a Activity);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                // Give a slow completed send time to receive its reply.
                // Immediately buffered pings/echoes are not peer liveness.
                if self.0.blocked_write.load(Ordering::Relaxed) {
                    self.0
                        .completed_write_ms
                        .store(self.0.write_ms.load(Ordering::Relaxed), Ordering::Relaxed);
                }
                self.0.writing.store(false, Ordering::Relaxed);
            }
        }
        Guard(self.0.as_ref())
    }
    /// Run `future` under the progress lease: `lease` bounds byte inactivity
    /// (write bytes for `write_only`, read bytes plus in-flight/completed
    /// writes otherwise), not total duration, so a slow transfer that keeps
    /// moving bytes stays alive while a stalled one is torn down.
    pub(crate) async fn while_progressing<F>(
        &self,
        lease: Duration,
        write_only: bool,
        future: F,
    ) -> Result<F::Output, ()>
    where
        F: Future,
    {
        let started = tokio::time::Instant::now();
        tokio::pin!(future);
        loop {
            let deadline = self.latest(write_only).max(started) + lease;
            tokio::select! {
                // Poll I/O before its watchdog when both become ready.
                biased;
                result = std::future::poll_fn(|cx| {
                    let result = future.as_mut().poll(cx);
                    if write_only && result.is_pending() {
                        self.0.blocked_write.store(true, Ordering::Relaxed);
                    }
                    result
                }) => return Ok(result),
                _ = tokio::time::sleep_until(deadline) => {
                    if self.latest(write_only).max(started) + lease <= tokio::time::Instant::now() {
                        return Err(());
                    }
                }
            }
        }
    }
}

/// Couples a WebSocket with the activity clock of its underlying byte stream.
pub struct Connection<S> {
    pub socket: WebSocketStream<S>,
    pub progress: Progress,
}

/// Transparent stream wrapper: framing, encryption and payloads are unchanged.
pub struct ProgressIo<S> {
    inner: S,
    progress: Progress,
}
impl<S> ProgressIo<S> {
    pub fn new(inner: S) -> (Self, Progress) {
        let progress = Progress::new();
        (
            Self {
                inner,
                progress: progress.clone(),
            },
            progress,
        )
    }
}
impl<S: AsyncRead + Unpin> AsyncRead for ProgressIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if buf.filled().len() > before {
            this.progress.record(false);
        }
        result
    }
}
impl<S: AsyncWrite + Unpin> AsyncWrite for ProgressIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(cx, bytes);
        if matches!(result, Poll::Ready(Ok(n)) if n > 0) {
            this.progress.record(true);
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

async fn send_message<S>(sink: &mut S, message: WsMessage) -> Result<(), S::Error>
where
    S: Sink<WsMessage> + Unpin,
{
    let (bytes, kind) = match message {
        WsMessage::Binary(bytes) if bytes.len() > 4 * FRAGMENT_BYTES => (bytes, Data::Binary),
        WsMessage::Text(text) if text.len() > 4 * FRAGMENT_BYTES => (text.into_bytes(), Data::Text),
        other => return sink.send(other).await,
    };
    let count = bytes.len().div_ceil(FRAGMENT_BYTES);
    for (index, chunk) in bytes.chunks(FRAGMENT_BYTES).enumerate() {
        let opcode = if index == 0 { kind } else { Data::Continue };
        sink.send(WsMessage::Frame(Frame::message(
            chunk.to_vec(),
            OpCode::Data(opcode),
            index + 1 == count,
        )))
        .await?;
        sink.send(WsMessage::Ping(Vec::new())).await?;
    }
    Ok(())
}

/// Drive one RPC session: outbound text lines from `out_rx`, inbound text
/// lines to `in_tx`. Protocol-level pings keep the link liveness observable;
/// non-text frames never reach the ndjson layer.
pub(crate) async fn pump<S>(
    ws: Connection<S>,
    out_rx: mpsc::Receiver<String>,
    in_tx: mpsc::Sender<String>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pump_with_timing(ws, out_rx, in_tx, PING_INTERVAL, SILENCE_LEASE).await;
}

/// Drive a session's ordered frames with independent reads and writes.
///
/// `lease` bounds byte inactivity during reads/writes and each delivery to
/// the consumer. Expiry closes the entire session: callers must fail pending
/// RPCs as `Closed` and reconnect, never resume a partial send. Dropping
/// this future or the inbound receiver releases both socket halves.
pub async fn pump_with_timing<S>(
    ws: Connection<S>,
    mut out_rx: mpsc::Receiver<String>,
    in_tx: mpsc::Sender<String>,
    ping_interval: Duration,
    lease: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let progress = ws.progress;
    let (mut sink, mut stream) = ws.socket.split();
    // Each direction owns its await points. A full TCP send buffer must not
    // stop reads or silence detection; a full consumer inbox must not stop
    // the writer's deadline. Neither future is detached from this session.
    let writer = async {
        let mut ping = tokio::time::interval(ping_interval);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        loop {
            let frame = tokio::select! {
                frame = out_rx.recv() => match frame {
                    Some(text) => WsMessage::Text(text),
                    // Tear down immediately: a close handshake can itself
                    // block on the failed uplink we are trying to release.
                    None => return,
                },
                _ = ping.tick() => WsMessage::Ping(Vec::new()),
            };
            let _writing = progress.writing();
            // Never resume a canceled send on this socket: it may have
            // written only part of a frame. The caller replays unacknowledged
            // requests on a fresh connection; pending RPCs fail as Closed.
            if !matches!(
                progress
                    .while_progressing(lease, true, send_message(&mut sink, frame))
                    .await,
                Ok(Ok(()))
            ) {
                tracing::warn!("rpc socket write failed or stopped progressing; dropping session");
                return;
            }
        }
    };
    let reader = async {
        loop {
            let frame = match progress
                .while_progressing(lease, false, stream.next())
                .await
            {
                Ok(Some(Ok(WsMessage::Close(_)))) | Ok(None) => return,
                Ok(Some(Ok(frame))) => frame,
                _ => {
                    tracing::warn!("rpc socket silent past lease or failed; dropping session");
                    return;
                }
            };
            if let WsMessage::Text(text) = frame
                && !matches!(
                    progress
                        .while_progressing(lease, false, in_tx.send(text))
                        .await,
                    Ok(Ok(()))
                )
            {
                tracing::warn!("rpc socket consumer closed or stalled; dropping session");
                return;
            }
        }
    };
    tokio::select! {
        _ = writer => {},
        _ = reader => {},
        _ = in_tx.closed() => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        pin::Pin,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        task::{Context, Poll},
    };
    use tokio::io::DuplexStream;
    use tokio::task::JoinHandle;
    use tokio::time::{Instant, Sleep};
    use tokio_tungstenite::tungstenite::protocol::Role;

    struct Harness {
        server: WebSocketStream<DuplexStream>,
        tx: mpsc::Sender<String>,
        rx: mpsc::Receiver<String>,
        task: JoinHandle<()>,
    }

    async fn harness() -> Harness {
        // The byte pipe fills inside one frame, deterministically reproducing
        // an uplink that stops accepting bytes. Real WebSocket framing, no OS
        // buffer-size assumptions or unreachable public addresses.
        let (client, server) = tokio::io::duplex(64);
        let (io, progress) = ProgressIo::new(client);
        let socket = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
        let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let (tx, out) = mpsc::channel(2);
        let (incoming, rx) = mpsc::channel(1);
        let task = tokio::spawn(pump(Connection { socket, progress }, out, incoming));
        Harness {
            server,
            tx,
            rx,
            task,
        }
    }

    async fn stall(h: &Harness) {
        h.tx.send("7".repeat(4096)).await.unwrap();
        tokio::task::yield_now().await;
    }

    async fn finishes(mut task: JoinHandle<()>, within: Duration) {
        match tokio::time::timeout(within, &mut task).await {
            Ok(result) => result.expect("socket pump panicked"),
            Err(_) => {
                task.abort();
                panic!("socket pump did not terminate within {within:?}");
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_write_cannot_disable_silence_deadline() {
        let h = harness().await;
        stall(&h).await;
        finishes(h.task, SILENCE_LEASE + Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_write_does_not_block_inbound_frames() {
        let mut h = harness().await;
        stall(&h).await;
        h.server.send(WsMessage::Text("ack".into())).await.unwrap();
        let received = tokio::time::timeout(Duration::from_secs(1), h.rx.recv()).await;
        h.task.abort();
        assert_eq!(received.unwrap(), Some("ack".to_owned()));
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_consumer_cancels_a_stalled_write() {
        let h = harness().await;
        stall(&h).await;
        drop(h.rx);
        finishes(h.task, Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn full_inbound_queue_has_a_deadline() {
        let mut h = harness().await;
        for _ in 0..2 {
            h.server.send(WsMessage::Text("row".into())).await.unwrap();
            tokio::task::yield_now().await;
        }
        finishes(h.task, SILENCE_LEASE + Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn pongs_do_not_extend_a_stalled_write_forever() {
        let mut h = harness().await;
        stall(&h).await;
        let peer = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if h.server.send(WsMessage::Pong(Vec::new())).await.is_err() {
                    return;
                }
            }
        });
        finishes(h.task, SILENCE_LEASE + Duration::from_secs(1)).await;
        peer.abort();
    }

    /// A half-open link: reads drain, writes are accepted into the void, and
    /// nothing ever comes back. Tungstenite answers protocol pings during
    /// `read`, so a merely draining peer is alive — deadness needs this.
    struct Blackholed<S> {
        inner: S,
    }
    impl<S: AsyncRead + Unpin> AsyncRead for Blackholed<S> {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
        }
    }
    impl<S: AsyncWrite + Unpin> AsyncWrite for Blackholed<S> {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_keepalives_do_not_keep_a_dead_peer_alive() {
        let (client, server) = tokio::io::duplex(2048);
        let (io, progress) = ProgressIo::new(client);
        let socket = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
        let mut peer =
            WebSocketStream::from_raw_socket(Blackholed { inner: server }, Role::Server, None)
                .await;
        let (tx, out) = mpsc::channel(2);
        let (incoming, rx) = mpsc::channel(1);
        let task = tokio::spawn(pump(Connection { socket, progress }, out, incoming));
        let peer = tokio::spawn(async move { while peer.next().await.is_some() {} });
        let sender = tokio::spawn(async move {
            loop {
                // Models echoed app traffic as well as the pump's own ping.
                tokio::time::sleep(Duration::from_secs(5)).await;
                if tx.send("echo".to_owned()).await.is_err() {
                    return;
                }
            }
        });
        // `rx` stays alive: the teardown must come from the read lease, not a
        // dropped consumer.
        finishes(task, SILENCE_LEASE + Duration::from_secs(1)).await;
        sender.abort();
        peer.abort();
        drop(rx);
    }

    #[tokio::test(start_paused = true)]
    async fn slow_healthy_link_preserves_frame_order() {
        let mut h = harness().await;
        let peer = tokio::spawn(async move {
            while let Some(Ok(frame)) = h.server.next().await {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let reply = match frame {
                    WsMessage::Ping(_) => WsMessage::Pong(Vec::new()),
                    other => other,
                };
                if h.server.send(reply).await.is_err() {
                    break;
                }
            }
        });
        let started = Instant::now();
        for i in 0..30 {
            let frame = format!("row-{i}");
            h.tx.send(frame.clone()).await.unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(10), h.rx.recv())
                    .await
                    .unwrap(),
                Some(frame)
            );
        }
        assert!(started.elapsed() > SILENCE_LEASE);
        drop(h.tx);
        finishes(h.task, Duration::from_secs(1)).await;
        peer.abort();
    }

    // ── slow-transfer reliability (port of zeron efa1a075) ──────────────────

    #[derive(Default)]
    struct Counts {
        read: AtomicU64,
        written: AtomicU64,
    }

    /// Duplex stream throttled to 2 KiB/s per direction: partial reads and
    /// writes complete one chunk per second, deterministically reproducing a
    /// slow-but-progressing link.
    struct MeteredIo {
        inner: DuplexStream,
        counts: Arc<Counts>,
        slow_read: bool,
        slow_write: bool,
        read_at: Pin<Box<Sleep>>,
        write_at: Pin<Box<Sleep>>,
    }
    impl MeteredIo {
        fn new(
            inner: DuplexStream,
            counts: Arc<Counts>,
            slow_read: bool,
            slow_write: bool,
        ) -> Self {
            Self {
                inner,
                counts,
                slow_read,
                slow_write,
                read_at: Box::pin(tokio::time::sleep(Duration::from_secs(1))),
                write_at: Box::pin(tokio::time::sleep(Duration::from_secs(1))),
            }
        }
    }
    impl AsyncRead for MeteredIo {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.slow_read && self.read_at.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            let mut chunk = buf.take(if self.slow_read {
                2048
            } else {
                buf.remaining()
            });
            let result = Pin::new(&mut self.inner).poll_read(cx, &mut chunk);
            let n = chunk.filled().len();
            // ReadBuf::take initialized and filled this prefix of the original.
            unsafe {
                buf.assume_init(n);
            }
            buf.advance(n);
            if n > 0 {
                self.counts.read.fetch_add(n as u64, Ordering::Relaxed);
                self.read_at
                    .as_mut()
                    .reset(Instant::now() + Duration::from_secs(1));
            }
            result
        }
    }
    impl AsyncWrite for MeteredIo {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.slow_write && self.write_at.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            let len = if self.slow_write {
                bytes.len().min(2048)
            } else {
                bytes.len()
            };
            let result = Pin::new(&mut self.inner).poll_write(cx, &bytes[..len]);
            if let Poll::Ready(Ok(n)) = result {
                self.counts.written.fetch_add(n as u64, Ordering::Relaxed);
                if n > 0 {
                    self.write_at
                        .as_mut()
                        .reset(Instant::now() + Duration::from_secs(1));
                }
            }
            result
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    async fn slow_transfer(download: bool) -> bool {
        let (client, peer) = tokio::io::duplex(2048);
        let counts = Arc::new(Counts::default());
        let io = MeteredIo::new(client, counts.clone(), download, !download);
        let (io, progress) = ProgressIo::new(io);
        let socket = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
        let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        let (tx, out_rx) = mpsc::channel(1);
        let (in_tx, mut rx) = mpsc::channel(1);
        let task = tokio::spawn(pump(Connection { socket, progress }, out_rx, in_tx));
        let payload = "x".repeat(128 * 1024);
        let expected = if download {
            payload.clone()
        } else {
            "ack".to_owned()
        };
        let download_payload = payload.clone();
        let server = tokio::spawn(async move {
            if download {
                let _ = peer.send(WsMessage::Text(download_payload)).await;
            }
            while let Some(Ok(frame)) = peer.next().await {
                let reply = match frame {
                    WsMessage::Text(text) if text.len() == 128 * 1024 => {
                        WsMessage::Text("ack".into())
                    }
                    WsMessage::Ping(_) => WsMessage::Pong(Vec::new()),
                    _ => continue,
                };
                if peer.send(reply).await.is_err() {
                    break;
                }
            }
        });
        let start = Instant::now();
        if !download {
            tx.send(payload).await.unwrap();
        }
        let result = tokio::time::timeout(Duration::from_secs(150), rx.recv()).await;
        let delivered = matches!(result, Ok(Some(ref text)) if *text == expected);
        println!(
            "{} 128KiB at 2KiB/s: delivered={}, elapsed={:?}, sent={}, received={}",
            if download { "download" } else { "upload" },
            delivered,
            start.elapsed(),
            counts.written.load(Ordering::Relaxed),
            counts.read.load(Ordering::Relaxed)
        );
        assert!(
            !task.is_finished(),
            "a progressing transfer must complete without reconnecting"
        );
        task.abort();
        server.abort();
        delivered
    }

    #[tokio::test(start_paused = true)]
    async fn slow_upload_must_complete_without_reconnecting() {
        assert!(
            slow_transfer(false).await,
            "the pump repeatedly abandons a healthy slow upload"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn slow_download_must_complete_without_reconnecting() {
        assert!(
            slow_transfer(true).await,
            "the pump abandons a progressing download"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn buffered_upload_gets_pongs_before_the_message_finishes() {
        // The entire message fits in the local send buffer. Only the remote
        // end sees the 2 KiB/s bottleneck, just like a kernel/proxy buffering
        // an uplink.
        let (client, peer) = tokio::io::duplex(256 * 1024);
        let (io, progress) = ProgressIo::new(client);
        let socket = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
        let peer = MeteredIo::new(peer, Arc::new(Counts::default()), true, false);
        let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        let (tx, out_rx) = mpsc::channel(1);
        let (in_tx, mut rx) = mpsc::channel(1);
        let pump_task = tokio::spawn(pump(Connection { socket, progress }, out_rx, in_tx));
        // UTF-8 characters cross fragment boundaries; reassembly must preserve them.
        let text = "€".repeat(44_000);
        tx.send(text.clone()).await.unwrap();
        let server = tokio::spawn(async move {
            let mut pings = 0;
            while let Some(Ok(message)) = peer.next().await {
                match message {
                    WsMessage::Ping(_) => {
                        pings += 1;
                        peer.flush().await.unwrap();
                    }
                    WsMessage::Text(value) => {
                        assert_eq!(value, text);
                        assert!(pings > 0, "no liveness until message completed");
                        peer.send(WsMessage::Text("delivered".into()))
                            .await
                            .unwrap();
                    }
                    _ => {}
                }
            }
        });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(150), rx.recv())
                .await
                .unwrap(),
            Some("delivered".to_owned())
        );
        assert!(!pump_task.is_finished());
        pump_task.abort();
        server.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn healthy_idle_traffic_stays_bounded() {
        let (client, peer) = tokio::io::duplex(2048);
        let (io, progress) = ProgressIo::new(client);
        let socket = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
        let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
        let (tx, out_rx) = mpsc::channel::<String>(1);
        let (in_tx, _rx) = mpsc::channel(1);
        let task = tokio::spawn(pump(Connection { socket, progress }, out_rx, in_tx));
        let pings = Arc::new(AtomicU64::new(0));
        let observed = pings.clone();
        let server = tokio::spawn(async move {
            while let Some(Ok(frame)) = peer.next().await {
                if let WsMessage::Ping(_) = frame {
                    observed.fetch_add(1, Ordering::Relaxed);
                    if peer.send(WsMessage::Pong(Vec::new())).await.is_err() {
                        break;
                    }
                }
            }
        });
        tokio::time::sleep(Duration::from_secs(601)).await;
        assert!(!task.is_finished(), "healthy idle connection churned");
        // One 2-byte control frame per 15s in each direction and nothing else.
        assert_eq!(pings.load(Ordering::Relaxed), 40, "unexpected idle traffic");
        drop(tx);
        finishes(task, Duration::from_secs(1)).await;
        server.abort();
    }
}
