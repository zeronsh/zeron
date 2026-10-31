//! MSP framing: newline-delimited JSON-RPC 2.0 on the host's stdio.
//!
//! Requests carry integer ids the client mints; every state-changing method
//! is a COMMAND whose params also carry a client-minted UUIDv7 `commandId`
//! (the server's idempotency handle — it never mints one itself, and a
//! fresh turn's id IS its `turn/start` command id). Queries (`initialize`,
//! `model/list`) carry no `commandId`. The host also sends requests of its
//! own (`approval/request`, `userInput/request`), answered on the same
//! stream.
use crate::HarnessError;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};

/// The host's own inbound frame limit (10 MiB).
const MAX_FRAME: usize = 10 * 1024 * 1024;

/// Methods that are queries, not commands: no `commandId`.
const QUERIES: &[&str] = &["initialize", "model/list", "session/read"];

#[derive(Clone)]
pub(super) struct Client {
    writer: mpsc::UnboundedSender<Value>,
    next_id: Arc<AtomicU64>,
}

impl Client {
    fn send(&self, frame: Value) -> Result<(), HarnessError> {
        self.writer
            .send(frame)
            .map_err(|_| HarnessError::Protocol("Muse stdin closed".into()))
    }

    /// Send `method`; commands get a fresh `commandId` unless `params`
    /// already carry one (a turn's id is chosen before it is sent). Returns
    /// the JSON-RPC id the response will answer.
    pub fn request(&self, method: &str, mut params: Value) -> Result<u64, HarnessError> {
        if !QUERIES.contains(&method) && params.get("commandId").is_none() {
            params["commandId"] = json!(command_id());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        Ok(id)
    }

    pub fn notify(&self, method: &str) -> Result<(), HarnessError> {
        self.send(json!({"jsonrpc":"2.0","method":method}))
    }

    /// Answer a host request.
    pub fn reply(&self, id: Value, result: Value) -> Result<(), HarnessError> {
        self.send(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }

    pub fn reply_error(
        &self,
        id: Value,
        code: i64,
        message: &str,
        kind: &str,
    ) -> Result<(), HarnessError> {
        self.send(json!({"jsonrpc":"2.0","id":id,"error":{
            "code":code,"message":message,"data":{"kind":kind,"retryable":false}
        }}))
    }
}

/// A UUIDv7: the 48-bit Unix-millisecond timestamp, version 7, then random
/// bits — the form MSP requires for every `commandId` and session id.
pub(super) fn command_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    let mut bytes = *uuid::Uuid::new_v4().as_bytes();
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    bytes[6] = 0x70 | (bytes[6] & 0x0f);
    bytes[8] = 0x80 | (bytes[8] & 0x3f);
    uuid::Uuid::from_bytes(bytes).to_string()
}

/// One decoded inbound frame.
#[derive(Debug)]
pub(super) enum Inbound {
    /// A response to one of our requests.
    Response {
        id: u64,
        result: Result<Value, Failure>,
    },
    /// The host asking us something (`approval/request`, …). What it asks
    /// also arrives as a notification, which is where the run acts on it.
    Request {
        id: Value,
        method: String,
    },
    Notification {
        method: String,
        params: Value,
    },
}

/// A server-authored MSP error: its `data.kind` is the machine-readable cause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Failure {
    pub kind: String,
    pub message: String,
}

pub(super) struct Transport {
    pub client: Client,
    pub incoming: mpsc::Receiver<Result<Inbound, HarnessError>>,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

impl Transport {
    pub fn new(
        input: impl AsyncWrite + Unpin + Send + 'static,
        output: impl AsyncRead + Unpin + Send + 'static,
    ) -> Self {
        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<Value>();
        let (tx, incoming) = mpsc::channel(512);
        let errors = tx.clone();
        let writer = tokio::spawn(async move {
            let mut input = input;
            while let Some(frame) = writer_rx.recv().await {
                let mut bytes = serde_json::to_vec(&frame).expect("JSON value");
                bytes.push(b'\n');
                let result = async {
                    input.write_all(&bytes).await?;
                    input.flush().await
                }
                .await;
                if let Err(e) = result {
                    let _ = errors.send(Err(e.into())).await;
                    break;
                }
            }
        });
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(output);
            let mut bytes = Vec::new();
            loop {
                // fill_buf bounds allocation before a noisy peer sends LF.
                let chunk = match reader.fill_buf().await {
                    Ok([]) => {
                        let _ = tx
                            .send(Err(HarnessError::Protocol("Muse stdout closed".into())))
                            .await;
                        break;
                    }
                    Ok(chunk) => chunk,
                    Err(e) => {
                        let _ = tx.send(Err(e.into())).await;
                        break;
                    }
                };
                let count = chunk
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(chunk.len(), |i| i + 1);
                if bytes.len() + count > MAX_FRAME {
                    let _ = tx
                        .send(Err(HarnessError::Protocol(
                            "Muse frame exceeds 10 MiB".into(),
                        )))
                        .await;
                    break;
                }
                bytes.extend_from_slice(&chunk[..count]);
                reader.consume(count);
                if bytes.last() != Some(&b'\n') {
                    continue;
                }
                if !bytes.iter().all(u8::is_ascii_whitespace) {
                    // A stray non-UTF-8 byte (raw tool output echoed into a
                    // frame) costs that character, never the session.
                    let frame = serde_json::from_str::<Value>(&String::from_utf8_lossy(&bytes))
                        .map_err(|e| HarnessError::Protocol(format!("Invalid Muse JSON: {e}")))
                        .and_then(decode);
                    let failed = frame.is_err();
                    if tx.send(frame).await.is_err() || failed {
                        break;
                    }
                }
                bytes.clear();
            }
        });
        Self {
            client: Client {
                writer: writer_tx,
                next_id: Arc::new(AtomicU64::new(1)),
            },
            incoming,
            reader,
            writer,
        }
    }
}

fn decode(frame: Value) -> Result<Inbound, HarnessError> {
    if frame["jsonrpc"] != "2.0" {
        return Err(HarnessError::Protocol(
            "Muse sent a frame that is not JSON-RPC 2.0".into(),
        ));
    }
    let params = frame.get("params").cloned().unwrap_or(Value::Null);
    if let Some(method) = frame["method"].as_str() {
        return Ok(match frame.get("id") {
            Some(id) if !id.is_null() => Inbound::Request {
                id: id.clone(),
                method: method.into(),
            },
            _ => Inbound::Notification {
                method: method.into(),
                params,
            },
        });
    }
    let id = frame["id"]
        .as_u64()
        .ok_or_else(|| HarnessError::Protocol("Muse response has no usable id".into()))?;
    let result = match (frame.get("result"), frame.get("error")) {
        (Some(result), None) => Ok(result.clone()),
        (None, Some(error)) => Err(Failure {
            kind: error["data"]["kind"].as_str().unwrap_or("unknown").into(),
            message: error["message"].as_str().unwrap_or("request failed").into(),
        }),
        _ => {
            return Err(HarnessError::Protocol(
                "Muse response must carry exactly one of result or error".into(),
            ));
        }
    };
    Ok(Inbound::Response { id, result })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_are_uuid_v7() {
        let id = uuid::Uuid::parse_str(&command_id()).unwrap();
        assert_eq!(id.get_version_num(), 7);
        assert_eq!(id.get_variant(), uuid::Variant::RFC4122);
        assert_ne!(command_id(), command_id());
    }

    #[tokio::test]
    async fn commands_carry_a_command_id_and_queries_do_not() {
        let (host, peer) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(host);
        let (peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new(write, read);
        let query = transport.client.request("model/list", json!({})).unwrap();
        let command = transport
            .client
            .request("turn/start", json!({"commandId":"turn-1"}))
            .unwrap();
        transport
            .client
            .request("turn/interrupt", json!({}))
            .unwrap();
        let mut lines = BufReader::new(peer_read).lines();
        let first: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(first["id"], query);
        assert!(first["params"].get("commandId").is_none());
        let second: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(second["id"], command);
        assert_eq!(second["params"]["commandId"], "turn-1");
        let third: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert!(third["params"]["commandId"].is_string());

        for frame in [
            json!({"jsonrpc":"2.0","id":query,"result":{"models":[]}}),
            json!({"jsonrpc":"2.0","id":command,"error":{"code":-32020,"message":"gone","data":{"kind":"sessionNotFound"}}}),
            json!({"jsonrpc":"2.0","id":7,"method":"approval/request","params":{"approvalId":"a"}}),
            json!({"jsonrpc":"2.0","method":"item/delta","params":{"delta":"ñ"}}),
        ] {
            peer_write
                .write_all(format!("{frame}\r\n").as_bytes())
                .await
                .unwrap();
        }
        assert!(matches!(
            transport.incoming.recv().await.unwrap().unwrap(),
            Inbound::Response { id, result: Ok(_) } if id == query
        ));
        assert!(matches!(
            transport.incoming.recv().await.unwrap().unwrap(),
            Inbound::Response { result: Err(Failure { kind, .. }), .. } if kind == "sessionNotFound"
        ));
        assert!(matches!(
            transport.incoming.recv().await.unwrap().unwrap(),
            Inbound::Request { method, .. } if method == "approval/request"
        ));
        assert!(matches!(
            transport.incoming.recv().await.unwrap().unwrap(),
            Inbound::Notification { method, .. } if method == "item/delta"
        ));
    }
}
