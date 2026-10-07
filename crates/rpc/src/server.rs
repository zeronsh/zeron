//! Server side: dispatch loop over string frames + the WebSocket acceptor.

use std::collections::HashMap;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::StatusCode;

use crate::{ClientFrame, RpcError, RpcReply, RpcService, ServerFrame};

struct Running {
    task: tokio::task::AbortHandle,
    credits: Option<Arc<Semaphore>>,
    window: usize,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serve one connection: read client frames from `inbound`, write server frames to `out`.
/// Returns when either channel closes; all request tasks are aborted on exit.
pub async fn serve_connection(
    service: Arc<dyn RpcService>,
    out: mpsc::Sender<String>,
    mut inbound: mpsc::Receiver<String>,
) {
    let mut running: HashMap<u64, Running> = HashMap::new();
    loop {
        let payload = tokio::select! {
            biased;
            _ = out.closed() => break,
            payload = inbound.recv() => match payload {
                Some(payload) => payload,
                None => break,
            },
        };
        // ndjson: a transport may batch several frames per message.
        for line in payload.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let frame: ClientFrame = match serde_json::from_str(line) {
                Ok(frame) => frame,
                Err(err) => {
                    tracing::warn!(error = %err, "rpc: dropping malformed client frame");
                    continue;
                }
            };
            running.retain(|_, task| !task.task.is_finished());
            if frame.cancel {
                running.remove(&frame.id);
                continue;
            }
            if frame.method.is_none() && frame.stream_credit > 0 {
                if let Some(request) = running.get(&frame.id)
                    && let Some(credits) = &request.credits
                {
                    let room = request.window.saturating_sub(credits.available_permits());
                    credits.add_permits(room.min(frame.stream_credit as usize));
                }
                continue;
            }
            let Some(method) = frame.method else {
                tracing::warn!(id = frame.id, "rpc: frame has neither method nor cancel");
                continue;
            };
            // Reusing an ID must not orphan the original request's lease/task.
            if let Some(previous) = running.remove(&frame.id) {
                drop(previous);
            }
            let window = frame
                .stream_window
                .map(|n| (n as usize).clamp(1, crate::client::STREAM_QUEUE_CAP));
            let credits = window.map(|n| Arc::new(Semaphore::new(n)));
            let task = tokio::spawn(handle_request(
                service.clone(),
                out.clone(),
                frame.id,
                method,
                frame.params,
                credits.clone(),
            ));
            running.insert(
                frame.id,
                Running {
                    task: task.abort_handle(),
                    credits,
                    window: window.unwrap_or(0),
                },
            );
        }
    }
    // Dropping the RAII leases also handles cancellation of this future.
}

async fn handle_request(
    service: Arc<dyn RpcService>,
    out: mpsc::Sender<String>,
    id: u64,
    method: String,
    params: serde_json::Value,
    credits: Option<Arc<Semaphore>>,
) {
    let send = |frame: ServerFrame| {
        let out = out.clone();
        async move {
            match serde_json::to_string(&frame) {
                Ok(json) => out.send(json).await.map_err(|_| RpcError::Closed),
                Err(err) => {
                    tracing::error!(error = %err, "rpc: failed to serialize server frame");
                    Err(RpcError::Closed)
                }
            }
        }
    };
    match service.handle(&method, params).await {
        Ok(RpcReply::Value(value)) => {
            let _ = send(ServerFrame {
                id,
                ok: Some(value),
                ..Default::default()
            })
            .await;
        }
        Ok(RpcReply::Stream(mut stream)) => {
            if let Some(credits) = &credits
                && send(ServerFrame {
                    id,
                    stream_window: Some(credits.available_permits() as u32),
                    ..Default::default()
                })
                .await
                .is_err()
            {
                return;
            }
            // Only the versioned checkout-PR stream uses an explicit readiness
            // frame. Sending it for legacy streams would make older clients remove
            // their pending stream as if it were a unary response.
            if method == crate::methods::WATCH_CHECKOUT_CHANGE_REQUEST
                && send(ServerFrame {
                    id,
                    ok: Some(serde_json::json!({ "stream": true })),
                    ..Default::default()
                })
                .await
                .is_err()
            {
                return;
            }
            loop {
                // Acquire before polling the producer: an unread subscription
                // retains at most its negotiated window and pauses only itself.
                if let Some(credits) = &credits {
                    match credits.acquire().await {
                        Ok(permit) => permit.forget(),
                        Err(_) => return,
                    }
                }
                let Some(item) = stream.next().await else {
                    break;
                };
                if send(ServerFrame {
                    id,
                    item: Some(item),
                    ..Default::default()
                })
                .await
                .is_err()
                {
                    return; // connection gone
                }
            }
            let _ = send(ServerFrame {
                id,
                done: true,
                ..Default::default()
            })
            .await;
        }
        Err(err) => {
            let _ = send(ServerFrame {
                id,
                err: Some(err.to_string()),
                ..Default::default()
            })
            .await;
        }
    }
}

/// Accept WebSocket connections forever, serving each with `service`.
pub async fn serve_ws_listener(listener: TcpListener, service: Arc<dyn RpcService>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                tracing::debug!(%peer, "rpc: connection accepted");
                tokio::spawn(serve_ws_socket(stream, service.clone()));
            }
            Err(err) => {
                tracing::warn!(error = %err, "rpc: accept failed");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}

async fn serve_ws_socket(stream: TcpStream, service: Arc<dyn RpcService>) {
    // Native viewports dial this socket with a bare `connect_async` and send
    // no `Origin` header. A browser always attaches `Origin` to a WebSocket
    // handshake and cannot forge or suppress it from script, and WebSockets
    // are exempt from the Same-Origin Policy — so only rejecting any handshake
    // that carries `Origin` keeps a page the user happens to visit from
    // reaching this local socket. Keep this check.
    //
    // The large `Err` (ErrorResponse) is the shape tungstenite's Callback
    // trait requires; it can't be boxed away here.
    #[allow(clippy::result_large_err)]
    let reject_cross_origin = |req: &HandshakeRequest, resp: HandshakeResponse| {
        if let Some(origin) = req.headers().get("origin") {
            tracing::warn!(
                origin = %String::from_utf8_lossy(origin.as_bytes()),
                "rpc: rejecting handshake carrying an Origin header (cross-origin browser dial)"
            );
            let mut err = ErrorResponse::new(Some("origin not allowed on local IPC".to_string()));
            *err.status_mut() = StatusCode::FORBIDDEN;
            return Err(err);
        }
        Ok(resp)
    };
    let ws = match tokio_tungstenite::accept_hdr_async(stream, reject_cross_origin).await {
        Ok(ws) => ws,
        Err(err) => {
            tracing::warn!(error = %err, "rpc: websocket handshake failed");
            return;
        }
    };
    let (out_tx, out_rx) = mpsc::channel::<String>(crate::FRAME_QUEUE_CAP);
    let (in_tx, in_rx) = mpsc::channel::<String>(crate::FRAME_QUEUE_CAP);
    let pump = tokio::spawn(pump_socket(ws, out_rx, in_tx));
    serve_connection(service, out_tx, in_rx).await;
    pump.abort();
}

/// Keep both socket directions independently pollable. A blocked socket write
/// must not prevent receiving the credits/cancellation that release producers.
pub(crate) async fn pump_socket<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    mut outbound: mpsc::Receiver<String>,
    inbound: mpsc::Sender<String>,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut source) = ws.split();
    let reader = async {
        while let Some(message) = source.next().await {
            match message {
                Ok(WsMessage::Text(text)) => {
                    if inbound.send(text).await.is_err() {
                        break;
                    }
                }
                Ok(WsMessage::Close(_)) | Err(_) => break,
                Ok(_) => {}
            }
        }
    };
    let writer = async {
        while let Some(text) = outbound.recv().await {
            if sink.send(WsMessage::Text(text)).await.is_err() {
                return;
            }
        }
        let _ = sink.send(WsMessage::Close(None)).await;
    };
    tokio::select! {
        _ = inbound.closed() => {},
        _ = reader => {},
        _ = writer => {},
    }
}

#[cfg(test)]
mod pump_tests {
    use super::*;
    #[tokio::test]
    async fn stalled_socket_write_does_not_block_incoming_control_frames() {
        let (a, b) = tokio::io::duplex(64);
        let server = tokio_tungstenite::WebSocketStream::from_raw_socket(
            a,
            tokio_tungstenite::tungstenite::protocol::Role::Server,
            None,
        )
        .await;
        let mut peer = tokio_tungstenite::WebSocketStream::from_raw_socket(
            b,
            tokio_tungstenite::tungstenite::protocol::Role::Client,
            None,
        )
        .await;
        let (out, outbound) = mpsc::channel(1);
        let (inbound, mut received) = mpsc::channel(1);
        let pump = tokio::spawn(pump_socket(server, outbound, inbound));
        out.send("x".repeat(16 * 1024)).await.unwrap();
        peer.send(WsMessage::Text("credit".into())).await.unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), received.recv())
                .await
                .unwrap()
                .as_deref(),
            Some("credit")
        );
        drop(received);
        tokio::time::timeout(std::time::Duration::from_secs(1), pump)
            .await
            .unwrap()
            .unwrap();
    }
}
