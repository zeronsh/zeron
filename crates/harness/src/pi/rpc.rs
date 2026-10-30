//! Pi RPC is JSONL, not JSON-RPC 2.0. Responses and events share one ordered
//! channel: a get_state response is a barrier after preceding lifecycle events.
//! Oh My Pi's protocol v2 splits an oversized object into `rpc_chunk` frames,
//! reassembled here; Pi never sends them.
use crate::HarnessError;
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::JoinHandle,
};

const MAX_FRAME: usize = 32 * 1024 * 1024;
/// OMP's advertised `maxReassembledFrameBytes`.
const MAX_REASSEMBLED: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Client {
    label: &'static str,
    writer: mpsc::UnboundedSender<Value>,
    next_id: Arc<AtomicU64>,
}
impl Client {
    pub fn send(&self, frame: Value) -> Result<(), HarnessError> {
        self.writer
            .send(frame)
            .map_err(|_| HarnessError::Protocol(format!("{} stdin closed", self.label)))
    }
    pub fn request(&self, mut frame: Value) -> Result<String, HarnessError> {
        let id = format!("zeron-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        frame["id"] = Value::String(id.clone());
        self.send(frame)?;
        Ok(id)
    }
}

pub(crate) struct Transport {
    pub client: Client,
    pub incoming: mpsc::Receiver<Result<Value, HarnessError>>,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
    /// Ends the writer (and Pi's stdin) once the frames queued are written.
    /// Only Windows' teardown needs it.
    #[cfg_attr(not(windows), allow(dead_code))]
    close_input: Arc<tokio::sync::Notify>,
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}
impl Transport {
    pub fn new(
        label: &'static str,
        input: impl AsyncWrite + Unpin + Send + 'static,
        output: impl AsyncRead + Unpin + Send + 'static,
    ) -> Self {
        Self::bounded(label, input, output, MAX_FRAME)
    }

    fn bounded(
        label: &'static str,
        input: impl AsyncWrite + Unpin + Send + 'static,
        output: impl AsyncRead + Unpin + Send + 'static,
        max_frame: usize,
    ) -> Self {
        let (writer_tx, mut writer_rx) = mpsc::unbounded_channel::<Value>();
        let (tx, incoming) = mpsc::channel(256);
        let errors = tx.clone();
        let close_input = Arc::new(tokio::sync::Notify::new());
        let close = close_input.clone();
        let writer = tokio::spawn(async move {
            let mut input = input;
            loop {
                let frame = tokio::select! {
                    biased;
                    frame = writer_rx.recv() => frame,
                    () = close.notified() => None,
                };
                let Some(frame) = frame else { break };
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
            // Inside an oversized frame: discard through its newline.
            let mut skipping = false;
            let mut chunks = Chunks::default();
            loop {
                // fill_buf bounds allocation before a hostile/noisy peer sends LF.
                let chunk = match reader.fill_buf().await {
                    Ok([]) => {
                        let _ = tx
                            .send(Err(HarnessError::Protocol(format!(
                                "{label} stdout closed"
                            ))))
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
                let complete = chunk[count - 1] == b'\n';
                if skipping {
                    reader.consume(count);
                    skipping = !complete;
                    continue;
                }
                // One oversized frame (a huge tool result) costs that frame,
                // not the whole run: drop it and pick up at the next line.
                if bytes.len() + count > max_frame {
                    tracing::warn!(
                        target: "zeron_harness::pi",
                        "{label} RPC frame over {} MiB dropped",
                        max_frame / (1024 * 1024)
                    );
                    bytes.clear();
                    reader.consume(count);
                    skipping = !complete;
                    continue;
                }
                bytes.extend_from_slice(&chunk[..count]);
                reader.consume(count);
                if !complete {
                    continue;
                }
                if !bytes.iter().all(u8::is_ascii_whitespace) {
                    match parse_frame(&bytes).map(|frame| chunks.accept(frame)) {
                        Some(Ok(None)) => {}
                        Some(Ok(Some(frame))) => {
                            if tx.send(Ok(frame)).await.is_err() {
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            let _ = tx.send(Err(HarnessError::Protocol(e))).await;
                            break;
                        }
                        // Stray output (a dependency's console.log) is not a
                        // reason to end the run.
                        None => tracing::warn!(
                            target: "zeron_harness::pi",
                            "unparseable {label} RPC line skipped"
                        ),
                    }
                }
                bytes.clear();
            }
        });
        Self {
            client: Client {
                label,
                writer: writer_tx,
                next_id: Arc::new(AtomicU64::new(1)),
            },
            incoming,
            reader,
            writer,
            close_input,
        }
    }

    /// Before Pi's teardown: Windows' gives it its grace to exit on stdin
    /// EOF, which the writer holds off while any [`Client`] clone (an open
    /// dialog's) is alive. Unix teardown signals instead; its order is
    /// unchanged.
    pub fn close_input_for_teardown(&self) {
        #[cfg(windows)]
        self.close_input.notify_one();
    }
}

/// A frame, tolerating the one thing JS emits that serde_json refuses: a
/// lone UTF-16 surrogate escape (a string sliced mid-pair), which becomes
/// U+FFFD instead of costing the frame.
fn parse_frame(bytes: &[u8]) -> Option<Value> {
    serde_json::from_slice(bytes).ok().or_else(|| {
        let repaired = replace_lone_surrogates(bytes)?;
        serde_json::from_slice(&repaired).ok()
    })
}

/// `bytes` with each unpaired `\uD800`–`\uDFFF` escape replaced by `\uFFFD`;
/// `None` when there is none to replace.
fn replace_lone_surrogates(bytes: &[u8]) -> Option<Vec<u8>> {
    fn unit(bytes: &[u8], at: usize) -> Option<u16> {
        if bytes.get(at..at + 2)? != b"\\u" {
            return None;
        }
        let hex = bytes.get(at + 2..at + 6)?;
        if !hex.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        u16::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut changed = false;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'\\' {
            out.push(bytes[at]);
            at += 1;
            continue;
        }
        match unit(bytes, at) {
            Some(0xD800..=0xDBFF) if matches!(unit(bytes, at + 6), Some(0xDC00..=0xDFFF)) => {
                out.extend_from_slice(&bytes[at..at + 12]);
                at += 12;
            }
            Some(0xD800..=0xDFFF) => {
                out.extend_from_slice(b"\\ufffd");
                changed = true;
                at += 6;
            }
            Some(_) => {
                out.extend_from_slice(&bytes[at..at + 6]);
                at += 6;
            }
            // Any other escape: keep it whole, so `\\u` stays literal text.
            None => {
                let end = (at + 2).min(bytes.len());
                out.extend_from_slice(&bytes[at..end]);
                at = end;
            }
        }
    }
    changed.then_some(out)
}

/// One in-progress `rpc_chunk` sequence. Chunks of one object arrive
/// uninterrupted and in order; anything else is a protocol failure.
#[derive(Default)]
struct Chunks {
    id: String,
    count: u64,
    length: usize,
    data: Vec<u8>,
    next: u64,
}
impl Chunks {
    /// A complete frame, `None` while a chunked object is incomplete.
    fn accept(&mut self, frame: Value) -> Result<Option<Value>, String> {
        use base64::Engine;
        if frame["type"] != "rpc_chunk" {
            if self.next > 0 {
                return Err("RPC chunk sequence interrupted".into());
            }
            return Ok(Some(frame));
        }
        let id = frame["chunkId"].as_str().unwrap_or_default();
        let (Some(index), Some(count), Some(length)) = (
            frame["index"].as_u64(),
            frame["count"].as_u64(),
            frame["byteLength"].as_u64(),
        ) else {
            return Err("malformed RPC chunk".into());
        };
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        if index == 0 {
            if self.next > 0 {
                return Err("RPC chunk sequence interrupted".into());
            }
            if id.is_empty() || count == 0 || length > MAX_REASSEMBLED {
                return Err("RPC chunk sequence exceeds the reassembly limit".into());
            }
            *self = Self {
                id: id.into(),
                count,
                length,
                data: Vec::new(),
                next: 0,
            };
        } else if index != self.next
            || id != self.id
            || count != self.count
            || length != self.length
        {
            return Err("RPC chunk sequence out of order".into());
        }
        let part = base64::engine::general_purpose::STANDARD
            .decode(frame["data"].as_str().unwrap_or_default())
            .map_err(|e| format!("invalid RPC chunk data: {e}"))?;
        if self.data.len() + part.len() > self.length {
            return Err("RPC chunk data exceeds its byteLength".into());
        }
        self.data.extend_from_slice(&part);
        self.next += 1;
        if self.next < self.count {
            return Ok(None);
        }
        let data = std::mem::take(&mut self.data);
        *self = Self::default();
        if data.len() != length {
            return Err("RPC chunk data does not match its byteLength".into());
        }
        let text =
            std::str::from_utf8(&data).map_err(|e| format!("RPC chunk is not UTF-8: {e}"))?;
        serde_json::from_str(text)
            .map(Some)
            .map_err(|e| format!("invalid chunked RPC JSON: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn fragmented_utf8_and_responses_preserve_wire_order() {
        let (host, peer) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(host);
        let (peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new("Pi", write, read);
        let id = transport
            .client
            .request(json!({"type":"get_state"}))
            .unwrap();
        let mut lines = BufReader::new(peer_read).lines();
        let request: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(request["id"], id);
        assert!(request.get("jsonrpc").is_none());
        let frames = [
            json!({"type":"message_update", "text":"ñ\u{2028}ok"}),
            json!({"type":"agent_settled"}),
            json!({"type":"response","id":id,"success":true}),
        ];
        for frame in &frames {
            for byte in format!("{frame}\r\n").bytes() {
                peer_write.write_all(&[byte]).await.unwrap();
            }
        }
        for expected in frames {
            assert_eq!(transport.incoming.recv().await.unwrap().unwrap(), expected);
        }
        peer_write.shutdown().await.unwrap();
        drop(peer_write);
        assert!(transport.incoming.recv().await.unwrap().is_err());
    }
    #[tokio::test]
    async fn noise_lone_surrogates_and_oversized_frames_do_not_end_the_stream() {
        let (host, mut peer) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(host);
        let mut transport = Transport::bounded("Pi", write, read, 64);
        peer.write_all(b"not json\n").await.unwrap();
        peer.write_all(br#"{"type":"message_update","text":"cut \ud83d here"}"#)
            .await
            .unwrap();
        peer.write_all(b"\n").await.unwrap();
        // Over the 64-byte bound, arriving in pieces before its newline.
        peer.write_all(format!(r#"{{"type":"huge","text":"{}"#, "x".repeat(100)).as_bytes())
            .await
            .unwrap();
        peer.write_all(br#""}"#).await.unwrap();
        peer.write_all(b"\n{\"type\":\"agent_settled\"}\n")
            .await
            .unwrap();
        let first = transport.incoming.recv().await.unwrap().unwrap();
        assert_eq!(
            first,
            json!({"type":"message_update","text":"cut \u{fffd} here"})
        );
        let next = transport.incoming.recv().await.unwrap().unwrap();
        assert_eq!(next, json!({"type":"agent_settled"}));
    }

    #[test]
    fn surrogate_repair_keeps_pairs_and_literal_backslashes() {
        let pair = br#"{"t":"\ud83d\ude00"}"#;
        assert!(replace_lone_surrogates(pair).is_none());
        let literal = br#"{"t":"\\ud800"}"#;
        assert!(replace_lone_surrogates(literal).is_none());
        let trailing = br#"{"t":"a\udc00b\ud800"}"#;
        let repaired = replace_lone_surrogates(trailing).unwrap();
        assert_eq!(repaired, br#"{"t":"a\ufffdb\ufffd"}"#);
        assert_eq!(parse_frame(trailing).unwrap()["t"], "a\u{fffd}b\u{fffd}");
    }
    fn chunks(value: &Value, id: &str, size: usize) -> Vec<Value> {
        use base64::Engine;
        let bytes = value.to_string().into_bytes();
        let parts: Vec<_> = bytes.chunks(size).collect();
        parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                json!({"type":"rpc_chunk","chunkId":id,"index":index,"count":parts.len(),
                    "byteLength":bytes.len(),"data":base64::engine::general_purpose::STANDARD.encode(part)})
            })
            .collect()
    }
    #[tokio::test]
    async fn chunked_frames_reassemble_in_wire_order() {
        let (host, mut peer) = tokio::io::duplex(1 << 20);
        let (read, write) = tokio::io::split(host);
        let mut transport = Transport::new("Oh My Pi", write, read);
        let big = json!({"type":"response","id":"s","success":true,"data":{"systemPrompt":"ñ".repeat(40_000)}});
        let mut wire = String::new();
        for frame in chunks(&big, "rpc-1", 7_001) {
            wire.push_str(&format!("{frame}\n"));
        }
        wire.push_str("{\"type\":\"agent_end\"}\n");
        peer.write_all(wire.as_bytes()).await.unwrap();
        assert_eq!(transport.incoming.recv().await.unwrap().unwrap(), big);
        assert_eq!(
            transport.incoming.recv().await.unwrap().unwrap(),
            json!({"type":"agent_end"})
        );
    }
    #[test]
    fn invalid_chunk_sequences_are_protocol_failures() {
        let value = json!({"type":"response","data":"x".repeat(100)});
        let parts = chunks(&value, "rpc-1", 30);
        let other = chunks(&value, "rpc-2", 30);
        for (name, frames) in [
            (
                "interleaved",
                vec![parts[0].clone(), json!({"type":"agent_end"})],
            ),
            ("skipped", vec![parts[0].clone(), parts[2].clone()]),
            ("foreign id", vec![parts[0].clone(), other[1].clone()]),
            ("restarted", vec![parts[0].clone(), other[0].clone()]),
        ] {
            let mut state = Chunks::default();
            let results: Vec<_> = frames.into_iter().map(|f| state.accept(f)).collect();
            assert!(results.last().unwrap().is_err(), "{name}: {results:?}");
        }
        let mut lying = parts[0].clone();
        lying["byteLength"] = json!(MAX_REASSEMBLED + 1);
        assert!(Chunks::default().accept(lying).is_err());
        let mut state = Chunks::default();
        let mut short = parts.clone();
        for part in &mut short {
            part["byteLength"] = json!(10);
        }
        assert!(
            short
                .into_iter()
                .map(|f| state.accept(f))
                .any(|r| r.is_err())
        );
    }
}
