//! Comparative tests against the pre-change pump from 94cc6753 (also the
//! document-pump behavior at main 67c960f4). Both run the same byte transport.
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
use tokio::io::{DuplexStream, ReadBuf};
use tokio::time::{Instant, Sleep};
use tokio_tungstenite::tungstenite::protocol::Role;

#[allow(clippy::collapsible_if)] // Keep the baseline control flow unchanged.
async fn original_pump<S, T>(
    ws: WebSocketStream<S>,
    mut out_rx: mpsc::Receiver<T>,
    in_tx: mpsc::Sender<T>,
    encode: fn(T) -> WsMessage,
    decode: fn(WsMessage) -> Option<T>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;
    let mut last_rx = tokio::time::Instant::now();
    loop {
        tokio::select! {
            frame = out_rx.recv() => match frame {
                Some(bytes) => {
                    if sink.send(encode(bytes)).await.is_err() {
                        break;
                    }
                }
                None => {
                    let _ = sink.send(WsMessage::Close(None)).await;
                    break;
                }
            },
            frame = stream.next() => match frame {
                Some(Ok(frame)) => {
                    last_rx = tokio::time::Instant::now();
                    if let Some(value) = decode(frame) {
                        if in_tx.send(value).await.is_err() {
                            break;
                        }
                    }
                }
                Some(Err(_)) | None => break,
            },
            _ = ping.tick() => {
                if sink.send(WsMessage::Text("ping".into())).await.is_err() {
                    break;
                }
            }
            _ = tokio::time::sleep_until(last_rx + SILENCE_LEASE) => {
                tracing::warn!("chat2 socket silent past lease; treating as dead");
                break;
            }
        }
    }
}

#[derive(Default)]
struct Counts {
    read: AtomicU64,
    written: AtomicU64,
}

struct MeteredIo {
    inner: DuplexStream,
    counts: Arc<Counts>,
    slow_read: bool,
    slow_write: bool,
    read_at: Pin<Box<Sleep>>,
    write_at: Pin<Box<Sleep>>,
}
impl MeteredIo {
    fn new(inner: DuplexStream, counts: Arc<Counts>, slow_read: bool, slow_write: bool) -> Self {
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

async fn transfer(original: bool, download: bool) -> bool {
    let (client, peer) = tokio::io::duplex(2048);
    let counts = Arc::new(Counts::default());
    let io = MeteredIo::new(client, counts.clone(), download, !download);
    let (io, progress) = ProgressIo::new(io);
    let client = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
    let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
    let (tx, out_rx) = mpsc::channel(1);
    let (in_tx, mut rx) = mpsc::channel(1);
    let decode = |frame| match frame {
        WsMessage::Binary(bytes) => Some(bytes),
        _ => None,
    };
    let task = tokio::spawn(async move {
        if original {
            original_pump(client, out_rx, in_tx, WsMessage::Binary, decode).await
        } else {
            pump(
                Connection {
                    socket: client,
                    progress,
                },
                out_rx,
                in_tx,
                WsMessage::Binary,
                decode,
            )
            .await
        }
    });
    let payload = vec![42; 128 * 1024];
    let expected = if download {
        payload.clone()
    } else {
        b"ack".to_vec()
    };
    let server = tokio::spawn(async move {
        if download {
            let _ = peer.send(WsMessage::Binary(payload)).await;
        }
        while let Some(Ok(frame)) = peer.next().await {
            let reply = match frame {
                WsMessage::Binary(bytes) => {
                    assert_eq!(bytes, vec![42; 128 * 1024]);
                    WsMessage::Binary(b"ack".to_vec())
                }
                WsMessage::Text(text) if text == "ping" => WsMessage::Text("pong".into()),
                _ => continue,
            };
            if peer.send(reply).await.is_err() {
                break;
            }
        }
    });
    let start = Instant::now();
    if !download {
        tx.send(vec![42; 128 * 1024]).await.unwrap();
    }
    let result = tokio::time::timeout(Duration::from_secs(150), rx.recv()).await;
    let delivered = matches!(result, Ok(Some(ref bytes)) if *bytes == expected);
    println!(
        "{} {} 128KiB at 2KiB/s: delivered={}, elapsed={:?}, sent={}, received={}",
        if original { "main" } else { "current" },
        if download { "download" } else { "upload" },
        delivered,
        start.elapsed(),
        counts.written.load(Ordering::Relaxed),
        counts.read.load(Ordering::Relaxed)
    );
    task.abort();
    server.abort();
    delivered
}

#[tokio::test(start_paused = true)]
async fn slow_upload_must_complete_without_reconnecting() {
    let _ = transfer(true, false).await;
    assert!(
        transfer(false, false).await,
        "current pump repeatedly abandons a healthy slow upload"
    );
}

#[tokio::test(start_paused = true)]
async fn slow_download_must_complete_without_reconnecting() {
    let _ = transfer(true, true).await;
    assert!(
        transfer(false, true).await,
        "current pump abandons a progressing download"
    );
}

#[tokio::test(start_paused = true)]
async fn buffered_upload_gets_pongs_before_the_message_finishes() {
    // The entire message fits in the local send buffer. Only the remote end
    // sees the 2 KiB/s bottleneck, just like a kernel/proxy buffering an uplink.
    let (client, peer) = tokio::io::duplex(256 * 1024);
    let (client, progress) = ProgressIo::new(client);
    let socket = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let peer = MeteredIo::new(peer, Arc::new(Counts::default()), true, false);
    let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
    let (tx, out_rx) = mpsc::channel(1);
    let (in_tx, mut rx) = mpsc::channel(1);
    let pump = tokio::spawn(super::pump(
        Connection { socket, progress },
        out_rx,
        in_tx,
        |v| v,
        |v| matches!(v, WsMessage::Text(_)).then_some(v),
    ));
    // UTF-8 characters cross fragment boundaries; reassembly must preserve them.
    let text = "€".repeat(44_000);
    tx.send(WsMessage::Text(text.clone())).await.unwrap();
    let server = tokio::spawn(async move {
        let mut pings = 0;
        while let Some(Ok(message)) = peer.next().await {
            match message {
                WsMessage::Ping(_) => {
                    pings += 1;
                    peer.flush().await.unwrap();
                }
                WsMessage::Text(value) if value != "ping" => {
                    assert_eq!(value, text);
                    assert!(pings > 0, "no liveness until message completed");
                    peer.send(WsMessage::Text("delivered".into()))
                        .await
                        .unwrap();
                }
                WsMessage::Text(_) => {
                    peer.send(WsMessage::Text("pong".into())).await.unwrap();
                }
                _ => {}
            }
        }
    });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(150), rx.recv())
            .await
            .unwrap(),
        Some(WsMessage::Text("delivered".into()))
    );
    assert!(!pump.is_finished());
    pump.abort();
    server.abort();
}

async fn idle_traffic(original: bool) -> (u64, u64) {
    let (client, peer) = tokio::io::duplex(2048);
    let counts = Arc::new(Counts::default());
    let (io, progress) = ProgressIo::new(MeteredIo::new(client, counts.clone(), false, false));
    let socket = WebSocketStream::from_raw_socket(io, Role::Client, None).await;
    let mut peer = WebSocketStream::from_raw_socket(peer, Role::Server, None).await;
    let (_tx, out_rx) = mpsc::channel::<Vec<u8>>(1);
    let (in_tx, _rx) = mpsc::channel(1);
    let task = tokio::spawn(async move {
        if original {
            original_pump(socket, out_rx, in_tx, WsMessage::Binary, |_| None).await
        } else {
            pump(
                Connection { socket, progress },
                out_rx,
                in_tx,
                WsMessage::Binary,
                |_| None,
            )
            .await
        }
    });
    let server = tokio::spawn(async move {
        while let Some(Ok(frame)) = peer.next().await {
            if matches!(frame, WsMessage::Text(ref t) if t == "ping")
                && peer.send(WsMessage::Text("pong".into())).await.is_err()
            {
                break;
            }
        }
    });
    tokio::time::sleep(Duration::from_secs(601)).await;
    assert!(!task.is_finished(), "healthy idle connection churned");
    let totals = (
        counts.written.load(Ordering::Relaxed),
        counts.read.load(Ordering::Relaxed),
    );
    println!(
        "{} idle 601s: connections=1, sent={}, received={}",
        if original { "main" } else { "current" },
        totals.0,
        totals.1
    );
    task.abort();
    server.abort();
    totals
}

#[tokio::test(start_paused = true)]
async fn healthy_idle_traffic_does_not_increase() {
    let baseline = idle_traffic(true).await;
    let current = idle_traffic(false).await;
    assert_eq!(current, baseline);
    assert_eq!(current, (400, 240), "40 masked pings and 40 unmasked pongs");
}
