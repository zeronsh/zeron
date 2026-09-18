//! Bounded WebSocket pump for document sync and device relay sessions.

use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
#[cfg(test)]
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::frame::{
    Frame,
    coding::{Data, OpCode},
};

mod progress;
pub use progress::{Connection, Progress, ProgressIo};

const PING_INTERVAL: Duration = Duration::from_secs(15);
const SILENCE_LEASE: Duration = Duration::from_secs(45);

// Control frames can pass between fragments even before the peer has the
// complete application message. This exposes a slowly draining TCP buffer
// through returning pongs without changing the application protocol.
const FRAGMENT_BYTES: usize = 4096;

async fn send_message<S>(sink: &mut S, message: WsMessage) -> Result<(), S::Error>
where
    S: futures::Sink<WsMessage> + Unpin,
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

pub(crate) async fn pump<S, T>(
    ws: Connection<S>,
    out_rx: mpsc::Receiver<T>,
    in_tx: mpsc::Sender<T>,
    encode: fn(T) -> WsMessage,
    decode: fn(WsMessage) -> Option<T>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pump_with_timing(
        ws,
        out_rx,
        in_tx,
        encode,
        decode,
        PING_INTERVAL,
        SILENCE_LEASE,
    )
    .await;
}

/// Drive a session's ordered frames with independent reads and writes.
///
/// `lease` bounds byte inactivity during reads/writes and each delivery to the
/// consumer. Expiry closes the entire session: callers must recover using
/// their protocol's cursor or fail pending RPCs, never resume a partial send.
/// Dropping this future or the inbound receiver releases both socket halves.
pub async fn pump_with_timing<S, T>(
    ws: Connection<S>,
    mut out_rx: mpsc::Receiver<T>,
    in_tx: mpsc::Sender<T>,
    encode: fn(T) -> WsMessage,
    decode: fn(WsMessage) -> Option<T>,
    ping_interval: Duration,
    lease: Duration,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let progress = ws.progress;
    let (mut sink, mut stream) = ws.socket.split();
    // Each direction owns its await points. A full TCP send buffer must not
    // stop reads or silence detection; a full actor inbox must not stop the
    // writer's deadline. Neither future is detached from this session.
    let writer = async {
        let mut ping = tokio::time::interval(ping_interval);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        loop {
            let frame = tokio::select! {
                frame = out_rx.recv() => match frame {
                    Some(value) => encode(value),
                    // Tear down immediately: a close handshake can itself
                    // block on the failed uplink we are trying to release.
                    None => return,
                },
                _ = ping.tick() => WsMessage::Text("ping".into()),
            };
            // Byte progress during a blocked send extends the read grace:
            // the peer cannot answer a frame it has not finished receiving.
            let _writing = progress.writing();
            // Never resume a canceled send on this socket: it may have
            // written only part of a frame. The actor replays unacked data
            // on a fresh connection using its existing deduplication IDs.
            if !matches!(
                progress
                    .while_progressing(lease, true, send_message(&mut sink, frame))
                    .await,
                Ok(Ok(()))
            ) {
                tracing::warn!("socket write failed or stopped progressing; reconnecting");
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
                    tracing::warn!("socket read failed or silent past lease; reconnecting");
                    return;
                }
            };
            if let Some(value) = decode(frame) {
                // Bound backpressure without dropping/reordering a live
                // stream's rows. Disconnect makes the cursor replay them.
                if !matches!(
                    tokio::time::timeout(lease, in_tx.send(value)).await,
                    Ok(Ok(()))
                ) {
                    tracing::warn!("socket consumer closed or stalled; reconnecting");
                    return;
                }
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
    use tokio::io::DuplexStream;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::tungstenite::protocol::Role;

    struct Harness {
        server: WebSocketStream<DuplexStream>,
        tx: mpsc::Sender<WsMessage>,
        rx: mpsc::Receiver<WsMessage>,
        task: JoinHandle<()>,
    }

    async fn harness() -> Harness {
        // The byte pipe fills inside one frame, deterministically reproducing
        // an uplink that stops accepting bytes. Real WebSocket framing, no OS
        // buffer-size assumptions or unreachable public addresses.
        let (client, server) = tokio::io::duplex(64);
        let (client, progress) = ProgressIo::new(client);
        let client = Connection {
            socket: WebSocketStream::from_raw_socket(client, Role::Client, None).await,
            progress,
        };
        let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let (tx, out) = mpsc::channel(2);
        let (incoming, rx) = mpsc::channel(1);
        let task = tokio::spawn(pump(
            client,
            out,
            incoming,
            |v| v,
            |v| match v {
                WsMessage::Text(ref text) if text == "pong" => None,
                WsMessage::Text(_) | WsMessage::Binary(_) => Some(v),
                _ => None,
            },
        ));
        Harness {
            server,
            tx,
            rx,
            task,
        }
    }

    async fn stall(h: &Harness) {
        h.tx.send(WsMessage::Binary(vec![7; 4096])).await.unwrap();
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
    async fn unanswered_keepalives_do_not_keep_a_dead_peer_alive() {
        let mut h = harness().await;
        let peer = tokio::spawn(async move { while h.server.next().await.is_some() {} });
        let sender = tokio::spawn(async move {
            loop {
                // Models relay echo traffic as well as the pump's text ping.
                tokio::time::sleep(Duration::from_secs(5)).await;
                if h.tx
                    .send(WsMessage::Binary(b"echo".to_vec()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        });
        finishes(h.task, SILENCE_LEASE + Duration::from_secs(1)).await;
        sender.abort();
        peer.abort();
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
        assert_eq!(received.unwrap(), Some(WsMessage::Text("ack".into())));
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
                if h.server.send(WsMessage::Text("pong".into())).await.is_err() {
                    return;
                }
            }
        });
        finishes(h.task, SILENCE_LEASE + Duration::from_secs(1)).await;
        peer.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn slow_healthy_link_preserves_text_and_binary_order() {
        let mut h = harness().await;
        let peer = tokio::spawn(async move {
            while let Some(Ok(frame)) = h.server.next().await {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let reply = match frame {
                    WsMessage::Text(ref text) if text == "ping" => WsMessage::Text("pong".into()),
                    other => other,
                };
                if h.server.send(reply).await.is_err() {
                    break;
                }
            }
        });
        let started = tokio::time::Instant::now();
        for i in 0..30 {
            let frame = if i % 2 == 0 {
                WsMessage::Text(format!("row-{i}"))
            } else {
                WsMessage::Binary(vec![i; 8])
            };
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
}

#[cfg(test)]
mod reliability_tests;
