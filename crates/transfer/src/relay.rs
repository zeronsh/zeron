//! The relay fallback: an ordered byte pipe built from ordinary RPCs over
//! the DeviceRoom link, so the transfer protocol runs unchanged when no
//! direct WebRTC path pairs in time.
//!
//! - Receiver → sender bytes ride one stream (`FileTransferPipe`), whose
//!   first item `{ready: true}` confirms the pipe exists.
//! - Sender → receiver bytes ride unary `FileTransferPipeWrite` calls of at
//!   most [`CHUNK`] raw bytes (base64 — the attachment uploads' relay-safe
//!   size), [`IN_FLIGHT`] outstanding. The server runs each call on its own
//!   task, so writes carry a sequence number and are applied in order; a
//!   write's reply waits for its bytes to be accepted, which is the pipe's
//!   backpressure.
//!
//! Dropping either end closes the pipe: the stream's cancellation removes
//! the server side, whose transfer lane then reads end-of-stream.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, WriteHalf};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use zeron_proto::FileTransferTransport;
use zeron_rpc::{RpcClient, methods};

use crate::{BoxIo, Tunnel};

/// Raw bytes per pipe message (≈64 KB of base64).
pub const CHUNK: usize = 48 * 1024;
/// Unacknowledged writes per pipe.
const IN_FLIGHT: usize = 12;
const BUFFER: usize = 512 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(90);

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Sender side: lanes over one relay link to the receiving engine.
pub struct RelayTunnel {
    client: Arc<RpcClient>,
    from_device: String,
    stop: CancellationToken,
}

impl RelayTunnel {
    pub fn new(client: Arc<RpcClient>, from_device: impl Into<String>) -> Self {
        Self {
            client,
            from_device: from_device.into(),
            stop: CancellationToken::new(),
        }
    }
}

impl Drop for RelayTunnel {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

#[async_trait::async_trait]
impl Tunnel for RelayTunnel {
    fn transport(&self) -> FileTransferTransport {
        FileTransferTransport::Relay
    }

    fn max_lanes(&self) -> usize {
        2
    }

    async fn open_lane(&self) -> anyhow::Result<BoxIo> {
        let pipe_id = uuid::Uuid::new_v4().to_string();
        let mut stream = self
            .client
            .subscribe_scoped(
                methods::FILE_TRANSFER_PIPE,
                serde_json::json!({ "pipeId": pipe_id, "fromDeviceId": self.from_device }),
            )
            .await?;
        let first = tokio::time::timeout(READY_TIMEOUT, stream.recv())
            .await
            .map_err(|_| anyhow::anyhow!("the other device did not open a relay pipe"))?
            .ok_or_else(|| anyhow::anyhow!("the other device refused the relay pipe"))?;
        anyhow::ensure!(
            first.get("ready").and_then(|v| v.as_bool()) == Some(true),
            "unexpected relay pipe reply"
        );
        let (app, pipe) = tokio::io::duplex(BUFFER);
        let (mut read, mut write) = tokio::io::split(pipe);
        let stop = self.stop.child_token();
        let client = self.client.clone();
        tokio::spawn(async move {
            let upstream = async {
                let mut seq: u64 = 0;
                let mut calls = tokio::task::JoinSet::new();
                let mut buffer = vec![0u8; CHUNK];
                loop {
                    let length = read.read(&mut buffer).await?;
                    while calls.len() >= IN_FLIGHT {
                        calls.join_next().await.expect("non-empty")??;
                    }
                    let mut params = serde_json::json!({ "pipeId": pipe_id, "seq": seq });
                    if length == 0 {
                        params["eof"] = true.into();
                    } else {
                        params["data"] = b64().encode(&buffer[..length]).into();
                    }
                    seq += 1;
                    let client = client.clone();
                    calls.spawn(async move {
                        tokio::time::timeout(
                            WRITE_TIMEOUT,
                            client.call(methods::FILE_TRANSFER_PIPE_WRITE, params),
                        )
                        .await
                        .map_err(|_| anyhow::anyhow!("relay pipe write timed out"))??;
                        anyhow::Ok(())
                    });
                    if length == 0 {
                        break;
                    }
                }
                while let Some(result) = calls.join_next().await {
                    result??;
                }
                anyhow::Ok(())
            };
            let downstream = async {
                while let Some(item) = stream.recv().await {
                    if let Some(data) = item.get("data").and_then(|v| v.as_str()) {
                        write.write_all(&b64().decode(data)?).await?;
                    } else if item.get("eof").and_then(|v| v.as_bool()) == Some(true) {
                        write.shutdown().await?;
                        return anyhow::Ok(());
                    }
                }
                anyhow::bail!("relay pipe closed")
            };
            let result = tokio::select! {
                _ = stop.cancelled() => Ok(()),
                result = async { tokio::try_join!(upstream, downstream) } => result.map(|_| ()),
            };
            if let Err(error) = result {
                tracing::debug!(%error, "relay pipe ended");
            }
            // Dropping `stream` cancels the server side.
        });
        Ok(Box::new(app))
    }

    async fn close(&self) {
        self.stop.cancel();
    }
}

struct ServerPipe {
    writer: tokio::sync::Mutex<Option<WriteHalf<DuplexStream>>>,
    next: watch::Sender<u64>,
}

/// Receiver side: pipes opened by peers, keyed by their minted id.
#[derive(Clone, Default)]
pub struct RelayPipes(Arc<Mutex<HashMap<String, Arc<ServerPipe>>>>);

struct Registration {
    pipes: RelayPipes,
    id: String,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.pipes.0.lock().unwrap().remove(&self.id);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteParams {
    pipe_id: String,
    seq: u64,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    eof: bool,
}

impl RelayPipes {
    /// Register `pipe_id` and return (the stream to reply with, the local
    /// end for the transfer service). The stream's drop unregisters it.
    pub fn open(
        &self,
        pipe_id: &str,
    ) -> anyhow::Result<(BoxStream<'static, serde_json::Value>, DuplexStream)> {
        anyhow::ensure!(
            !pipe_id.is_empty() && pipe_id.len() <= 64,
            "invalid relay pipe id"
        );
        let (local, remote) = tokio::io::duplex(BUFFER);
        let (mut read, write) = tokio::io::split(remote);
        {
            let mut pipes = self.0.lock().unwrap();
            anyhow::ensure!(!pipes.contains_key(pipe_id), "relay pipe already open");
            anyhow::ensure!(pipes.len() < 64, "too many relay pipes");
            pipes.insert(
                pipe_id.to_owned(),
                Arc::new(ServerPipe {
                    writer: tokio::sync::Mutex::new(Some(write)),
                    next: watch::channel(0).0,
                }),
            );
        }
        let registration = Registration {
            pipes: self.clone(),
            id: pipe_id.to_owned(),
        };
        let ready = futures::stream::once(async { serde_json::json!({ "ready": true }) });
        // Read the receiver's bytes in relay-sized chunks until it closes.
        let (tx, rx) = tokio::sync::mpsc::channel::<serde_json::Value>(4);
        let reader = tokio::spawn(async move {
            let mut buffer = vec![0u8; CHUNK];
            loop {
                match read.read(&mut buffer).await {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(serde_json::json!({ "eof": true })).await;
                        return;
                    }
                    Ok(n) => {
                        let item = serde_json::json!({ "data": b64().encode(&buffer[..n]) });
                        if tx.send(item).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });
        let items = futures::stream::unfold(
            (rx, AbortOnDrop(reader), registration),
            |(mut rx, reader, registration)| async move {
                let item = rx.recv().await?;
                Some((item, (rx, reader, registration)))
            },
        );
        Ok((ready.chain(items).boxed(), local))
    }

    /// Apply one sender write in `seq` order.
    pub async fn write(&self, params: serde_json::Value) -> anyhow::Result<()> {
        let params: WriteParams = serde_json::from_value(params)?;
        let pipe = self
            .0
            .lock()
            .unwrap()
            .get(&params.pipe_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown relay pipe"))?;
        let mut next = pipe.next.subscribe();
        tokio::time::timeout(WRITE_TIMEOUT, next.wait_for(|n| *n == params.seq))
            .await
            .map_err(|_| anyhow::anyhow!("relay pipe write out of order"))??;
        {
            let mut writer = pipe.writer.lock().await;
            let Some(stream) = writer.as_mut() else {
                anyhow::bail!("relay pipe closed");
            };
            if let Some(data) = params.data.as_deref() {
                let bytes = b64().decode(data)?;
                anyhow::ensure!(bytes.len() <= CHUNK, "oversized relay pipe write");
                stream.write_all(&bytes).await?;
            }
            if params.eof {
                let _ = stream.shutdown().await;
                *writer = None;
            }
        }
        pipe.next.send_replace(params.seq + 1);
        Ok(())
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
