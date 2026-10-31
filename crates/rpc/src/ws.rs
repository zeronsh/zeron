//! The socket <-> string-channel pump both ends of the local RPC WebSocket run.

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};

/// Largest single frame we write. tungstenite refuses any *frame* over 16 MiB
/// by default, and one watch reply carries a whole transcript — a long chat's
/// is past that, and the peer dropping the socket on it then repeats on every
/// reconnect. Fragmenting keeps every frame small, so even a peer built
/// before this still reassembles the message (up to its 64 MiB message cap).
const FRAGMENT_BYTES: usize = 1 << 20;

/// Largest message (and, from older peers that don't fragment, frame) we
/// accept. Local IPC behind the bearer token: the cap only stops runaways.
const MAX_MESSAGE_BYTES: usize = 512 << 20;

pub(crate) fn config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE_BYTES),
        max_frame_size: Some(MAX_MESSAGE_BYTES),
        ..WebSocketConfig::default()
    }
}

/// Shuttle frames between `ws` and the string channels until either side
/// closes. Returns why the socket went away when it wasn't a clean close.
pub(crate) async fn pump<S>(
    ws: WebSocketStream<S>,
    mut out: mpsc::Receiver<String>,
    inbound: mpsc::Sender<String>,
) -> Option<String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();
    loop {
        tokio::select! {
            frame = out.recv() => match frame {
                Some(text) => {
                    if let Err(error) = send_text(&mut sink, text).await {
                        return Some(format!("sending to the socket failed: {error}"));
                    }
                }
                None => {
                    let _ = sink.send(WsMessage::Close(None)).await;
                    return None;
                }
            },
            message = stream.next() => match message {
                Some(Ok(WsMessage::Text(text))) => {
                    if inbound.send(text).await.is_err() {
                        return None;
                    }
                }
                Some(Ok(WsMessage::Close(_))) | None => return None,
                Some(Err(error)) => return Some(format!("reading the socket failed: {error}")),
                Some(Ok(_)) => {} // ping/pong/binary — ignored
            },
        }
    }
}

async fn send_text<S>(
    sink: &mut futures::stream::SplitSink<WebSocketStream<S>, WsMessage>,
    text: String,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if text.len() <= FRAGMENT_BYTES {
        return sink.send(WsMessage::Text(text)).await;
    }
    // A text message may split mid-codepoint: the receiver validates UTF-8
    // over the reassembled message, not per fragment.
    let bytes = text.into_bytes();
    let mut chunks = bytes.chunks(FRAGMENT_BYTES).peekable();
    let mut opcode = OpCode::Data(Data::Text);
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        sink.feed(WsMessage::Frame(Frame::message(
            chunk.to_vec(),
            opcode,
            last,
        )))
        .await?;
        opcode = OpCode::Data(Data::Continue);
    }
    sink.flush().await
}
