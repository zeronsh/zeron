//! Pi RPC is JSONL, not JSON-RPC 2.0. Responses and events share one ordered
//! channel: a get_state response is a barrier after preceding lifecycle events.
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

/// Retained bytes per JSONL frame. Bounds memory against a noisy peer.
const MAX_FRAME: usize = 32 * 1024 * 1024;
/// Longest JSON string kept verbatim. Larger strings are base64 images or
/// huge tool output: Zeron caps what it reads far below this (diffs 64 KiB,
/// output 16 KiB), so truncating them in place loses nothing it uses.
const MAX_STRING: usize = 1024 * 1024;
/// Largest `details` value kept. Tool `details` are renderer state Zeron never
/// reads; extension tools can store whole child transcripts there,
/// screenshots included, so one `tool_execution_end` or toolResult
/// `message_end` can carry hundreds of MB. Larger values become `null`.
const MAX_DETAILS: usize = 64 * 1024;
/// Frames Zeron never reads. These repeat every message of the run (images
/// included), so drop them before buffering. Tool updates pass through:
/// extensions stream progress in their details.
const IGNORED: [&str; 2] = ["agent_end", "turn_end"];
/// Pi writes `type` first: `{"type":"agent_end",...}`.
const TYPE_VALUE: &[u8] = b"{\"type\":\"";
/// Longest `data` string kept. Incoming image bytes arrive only under `data`
/// (tool results, session entries) and the driver never reads them, but one
/// result can carry several; a marker keeps the frame small regardless.
const MAX_DATA: usize = 4 * 1024;

/// The key a value belongs to, when the driver treats it specially.
#[derive(Clone, Copy, PartialEq)]
enum Key {
    Other,
    Details,
    Data,
}
#[derive(Debug, PartialEq)]
enum Line {
    Frame(Vec<u8>),
    /// An ignored frame, dropped as soon as its type was read.
    Skipped(&'static str),
    /// Over MAX_FRAME even after string truncation; carries the frame type.
    Oversized(Option<String>),
}

/// Splits stdout into LF-terminated frames while streaming, truncating long
/// JSON strings and dropping large `details` values in place so the frame
/// stays valid JSON.
struct LineScanner {
    line: Vec<u8>,
    max_line: usize,
    max_string: usize,
    in_string: bool,
    escaped: bool,
    /// Offset in `line` of the current string body (after the opening quote).
    string_start: usize,
    /// Body bytes of the current string seen so far, kept or not.
    string_len: usize,
    /// Body bytes kept for the current string: MAX_STRING, or MAX_DATA for a
    /// `data` value.
    string_limit: usize,
    /// Open objects and arrays.
    depth: usize,
    /// The last string closed was a key candidate.
    last_key: Key,
    /// The key of the value being read (set at its `:`).
    value_key: Key,
    /// A `details` value being kept: its offset in `line` and its depth.
    capture: Option<(usize, usize)>,
    /// The rest of an oversized `details` value is dropped; holds its depth.
    skip: Option<usize>,
    /// Set once the rest of the current line is dropped; returned at its LF.
    discard: Option<Line>,
}
impl Default for LineScanner {
    fn default() -> Self {
        Self::new(MAX_FRAME, MAX_STRING)
    }
}
impl LineScanner {
    fn new(max_line: usize, max_string: usize) -> Self {
        Self {
            line: Vec::new(),
            max_line,
            max_string,
            in_string: false,
            escaped: false,
            string_start: 0,
            string_len: 0,
            string_limit: max_string,
            depth: 0,
            last_key: Key::Other,
            value_key: Key::Other,
            capture: None,
            skip: None,
            discard: None,
        }
    }
    /// Body limit for the string that just opened.
    fn open_string(&mut self) {
        self.in_string = true;
        self.escaped = false;
        self.string_len = 0;
        self.string_start = self.line.len();
        self.string_limit = if self.value_key == Key::Data {
            MAX_DATA
        } else {
            self.max_string
        };
        self.value_key = Key::Other;
    }
    /// Consumes bytes up to and including the first LF. Returns how many
    /// bytes were consumed and the finished line, if any.
    fn feed(&mut self, chunk: &[u8]) -> (usize, Option<Line>) {
        for (i, &b) in chunk.iter().enumerate() {
            // JSON text never holds a raw LF, so LF always ends the frame.
            if b == b'\n' {
                let line = self
                    .discard
                    .take()
                    .unwrap_or_else(|| Line::Frame(std::mem::take(&mut self.line)));
                self.line = Vec::new();
                self.in_string = false;
                self.escaped = false;
                self.depth = 0;
                self.last_key = Key::Other;
                self.value_key = Key::Other;
                self.capture = None;
                self.skip = None;
                return (i + 1, Some(line));
            }
            if self.discard.is_some() {
                continue;
            }
            let dropping = self.skip.is_some();
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if b == b'\\' {
                    self.escaped = true;
                } else if b == b'"' {
                    self.in_string = false;
                    if dropping {
                        continue;
                    }
                    if self.string_len > self.string_limit {
                        self.truncate_string();
                    }
                    self.push(b);
                    if self.discard.is_some() {
                        continue;
                    }
                    if self.string_start == TYPE_VALUE.len() && self.line.starts_with(TYPE_VALUE) {
                        self.skip_ignored();
                    } else {
                        let end = self.line.len().saturating_sub(1);
                        self.last_key = if self.capture.is_some() {
                            Key::Other
                        } else {
                            match self.line.get(self.string_start..end) {
                                Some(b"details") => Key::Details,
                                Some(b"data") => Key::Data,
                                _ => Key::Other,
                            }
                        };
                    }
                    continue;
                }
                self.string_len += 1;
                if !dropping && self.string_len <= self.string_limit {
                    self.push(b);
                }
                continue;
            }
            if let Some(depth) = self.skip {
                match b {
                    b'"' => {
                        self.in_string = true;
                        self.escaped = false;
                    }
                    b'{' | b'[' => self.depth += 1,
                    b'}' | b']' if self.depth > depth => self.depth -= 1,
                    // The dropped value ended; this byte belongs to its parent.
                    b',' | b'}' | b']' if self.depth == depth => self.skip = None,
                    _ => {}
                }
                if self.skip.is_some() {
                    continue;
                }
            }
            if self
                .capture
                .is_some_and(|(_, depth)| depth == self.depth && matches!(b, b',' | b'}' | b']'))
            {
                self.capture = None;
            }
            match b {
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => self.depth = self.depth.saturating_sub(1),
                _ => {}
            }
            self.push(b);
            if b == b':' && self.discard.is_none() {
                self.value_key = std::mem::replace(&mut self.last_key, Key::Other);
                if self.value_key == Key::Details {
                    self.capture = Some((self.line.len(), self.depth));
                }
            } else if !b.is_ascii_whitespace() {
                self.last_key = Key::Other;
            }
            if b == b'"' && self.skip.is_none() {
                self.open_string();
            } else if b == b'"' {
                self.in_string = true;
                self.escaped = false;
            }
        }
        (chunk.len(), None)
    }
    fn push(&mut self, b: u8) {
        if self.line.len() >= self.max_line {
            self.discard = Some(Line::Oversized(frame_type(&self.line)));
            self.line = Vec::new();
            return;
        }
        self.line.push(b);
        if let Some((start, depth)) = self.capture
            && self.line.len() - start > MAX_DETAILS
        {
            self.line.truncate(start);
            self.line.extend_from_slice(b"null");
            self.capture = None;
            self.skip = Some(depth);
        }
    }
    /// Called when the leading `type` value closes: drops an ignored frame.
    fn skip_ignored(&mut self) {
        let kind = &self.line[TYPE_VALUE.len()..self.line.len() - 1];
        if let Some(kind) = IGNORED.iter().find(|k| k.as_bytes() == kind) {
            self.discard = Some(Line::Skipped(kind));
            self.line = Vec::new();
        }
    }
    /// Cuts the kept prefix back to a point that is still a valid JSON string
    /// body (not inside an escape, a UTF-8 sequence or a surrogate pair), then
    /// appends a marker.
    fn truncate_string(&mut self) {
        let body = &self.line[self.string_start..];
        let cut = (0..=body.len().min(16))
            .map(|back| body.len() - back)
            .find(|&cut| valid_string_body(&body[..cut]))
            .unwrap_or(0);
        let omitted = self.string_len - cut;
        self.line.truncate(self.string_start + cut);
        self.line
            .extend_from_slice(format!("... [{omitted} bytes omitted by Zeron]").as_bytes());
    }
}
fn valid_string_body(body: &[u8]) -> bool {
    let mut quoted = Vec::with_capacity(body.len() + 2);
    quoted.push(b'"');
    quoted.extend_from_slice(body);
    quoted.push(b'"');
    serde_json::from_slice::<String>(&quoted).is_ok()
}
/// Pi serializes `type` first, so the leading `"type":"…"` names the frame.
fn frame_type(line: &[u8]) -> Option<String> {
    const KEY: &[u8] = b"\"type\":\"";
    let head = &line[..line.len().min(4096)];
    let start = head.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    let len = head[start..].iter().position(|b| *b == b'"')?;
    String::from_utf8(head[start..start + len].to_vec()).ok()
}

#[derive(Clone)]
pub(super) struct Client {
    writer: mpsc::UnboundedSender<Value>,
    next_id: Arc<AtomicU64>,
}
impl Client {
    pub fn send(&self, frame: Value) -> Result<(), HarnessError> {
        self.writer
            .send(frame)
            .map_err(|_| HarnessError::Protocol("Pi stdin closed".into()))
    }
    pub fn request(&self, mut frame: Value) -> Result<String, HarnessError> {
        let id = format!("zeron-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        frame["id"] = Value::String(id.clone());
        self.send(frame)?;
        Ok(id)
    }
}

pub(super) struct Transport {
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
        input: impl AsyncWrite + Unpin + Send + 'static,
        output: impl AsyncRead + Unpin + Send + 'static,
    ) -> Self {
        Self::bounded(input, output, MAX_FRAME)
    }

    fn bounded(
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
            let mut scanner = LineScanner::new(max_frame, MAX_STRING);
            loop {
                // fill_buf bounds allocation before a hostile/noisy peer sends LF.
                let chunk = match reader.fill_buf().await {
                    Ok([]) => {
                        let _ = tx
                            .send(Err(HarnessError::Protocol("Pi stdout closed".into())))
                            .await;
                        break;
                    }
                    Ok(chunk) => chunk,
                    Err(e) => {
                        let _ = tx.send(Err(e.into())).await;
                        break;
                    }
                };
                let (count, line) = scanner.feed(chunk);
                reader.consume(count);
                match line {
                    None | Some(Line::Skipped(_)) => continue,
                    Some(Line::Frame(bytes)) if bytes.iter().all(u8::is_ascii_whitespace) => {
                        continue;
                    }
                    Some(Line::Frame(bytes)) => match parse_frame(&bytes) {
                        Some(frame) => {
                            if tx.send(Ok(frame)).await.is_err() {
                                break;
                            }
                        }
                        // Stray output (a dependency's console.log) is not a
                        // reason to end the run.
                        None => tracing::warn!(
                            target: "zeron_harness::pi",
                            "unparseable Pi RPC line skipped"
                        ),
                    },
                    // One oversized frame (a huge tool result) costs that
                    // frame, not the whole run: drop it and pick up at the
                    // next line.
                    Some(Line::Oversized(kind)) => tracing::warn!(
                        target: "zeron_harness::pi",
                        "Pi RPC {} frame over {} MiB dropped",
                        kind.as_deref().unwrap_or("unknown"),
                        max_frame / (1024 * 1024)
                    ),
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[tokio::test]
    async fn fragmented_utf8_and_responses_preserve_wire_order() {
        let (host, peer) = tokio::io::duplex(8192);
        let (read, write) = tokio::io::split(host);
        let (peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new(write, read);
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
        let mut transport = Transport::bounded(write, read, 64);
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
    fn scan(scanner: &mut LineScanner, input: &[u8], chunk: usize) -> Vec<Line> {
        let mut lines = vec![];
        for piece in input.chunks(chunk) {
            let mut piece = piece;
            while !piece.is_empty() {
                let (count, line) = scanner.feed(piece);
                lines.extend(line);
                piece = &piece[count..];
            }
        }
        lines
    }
    fn frame(line: &Line) -> Value {
        match line {
            Line::Frame(bytes) => serde_json::from_slice(bytes).unwrap(),
            other => panic!("expected a frame, got {other:?}"),
        }
    }
    #[test]
    fn long_strings_truncate_to_valid_json_at_every_cut() {
        // Escapes, multi-byte UTF-8 and a surrogate pair around every cut point.
        let text = "a\\\"b\nñ€\u{1F600}\t\u{1}".repeat(3);
        let input = format!("{}\n", json!({"type":"turn_end","text":text,"n":1}));
        let escaped = serde_json::to_string(&text).unwrap().len() - 2;
        // From 8: keys and "turn_end" stay whole; only `text` is long.
        for max in 8..=escaped + 1 {
            let mut scanner = LineScanner::new(MAX_FRAME, max);
            let lines = scan(&mut scanner, input.as_bytes(), 7);
            assert_eq!(lines.len(), 1);
            let value = frame(&lines[0]);
            assert_eq!(value["n"], 1, "structure survives at max={max}");
            let kept = value["text"].as_str().unwrap();
            if max >= escaped {
                assert_eq!(kept, text);
            } else {
                let (prefix, marker) = kept.split_once("... [").unwrap();
                assert!(text.starts_with(prefix), "max={max}");
                assert!(marker.ends_with("bytes omitted by Zeron]"));
            }
        }
    }
    #[test]
    fn ignored_frames_are_dropped_at_any_size_and_oversized_others_fail() {
        let mut scanner = LineScanner::new(256, 64);
        // Many short strings: truncation alone cannot keep this under the cap.
        let parts: Vec<_> = (0..100).map(|i| format!("part {i}")).collect();
        // Written as Pi does, `type` first (json! would sort the keys).
        let frame_of =
            |kind: &str, body: &Value| format!("{{\"type\":\"{kind}\",\"messages\":{body}}}\n");
        let input = [
            frame_of("agent_end", &json!(parts)),
            frame_of("turn_end", &json!([])),
            // A nested ignored type is not the frame type.
            format!(
                "{}\n",
                json!({"message":{"type":"agent_end"},"type":"message_end"})
            ),
            frame_of("agent_endless", &json!([])),
            frame_of("message_end", &json!(parts)),
        ]
        .concat();
        let lines = scan(&mut scanner, input.as_bytes(), 13);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], Line::Skipped("agent_end"));
        assert_eq!(lines[1], Line::Skipped("turn_end"));
        assert_eq!(frame(&lines[2])["type"], "message_end");
        assert_eq!(frame(&lines[3])["type"], "agent_endless");
        assert_eq!(lines[4], Line::Oversized(Some("message_end".into())));
    }
    #[test]
    fn large_data_strings_keep_a_marker_and_small_ones_stay_whole() {
        let image = "A".repeat(2 * 1024 * 1024);
        let frames = [
            // Nested tool-result images, as Pi sends them.
            format!(
                "{}\n",
                json!({"message":{"role":"toolResult","content":[
                    {"type":"image","data":image,"mimeType":"image/png"},
                    {"type":"text","text":"after"},
                ]}})
            ),
            // `data` holding structure, a short string, and lookalike keys.
            format!(
                "{}\n",
                json!({"type":"response","data":{"sessionId":"s","entries":[{"type":"message","data":image}]},"metadata":image,"note":"data"})
            ),
        ]
        .concat();
        for chunk in [1, 13, 1 << 16] {
            let mut scanner = LineScanner::default();
            let lines = scan(&mut scanner, frames.as_bytes(), chunk);
            assert_eq!(lines.len(), 2);
            let tool = frame(&lines[0]);
            let content = tool["message"]["content"].as_array().unwrap();
            let kept = content[0]["data"].as_str().unwrap();
            assert!(kept.len() <= MAX_DATA + 64, "chunk {chunk}");
            assert!(kept.starts_with("AA"));
            assert!(kept.ends_with("bytes omitted by Zeron]"));
            assert_eq!(content[1]["text"], "after");
            let response = frame(&lines[1]);
            assert_eq!(response["data"]["sessionId"], "s");
            assert!(
                response["data"]["entries"][0]["data"]
                    .as_str()
                    .unwrap()
                    .len()
                    <= MAX_DATA + 64
            );
            // A `data` string that is short, and keys that only look like `data`.
            assert!(response["metadata"].as_str().unwrap().len() > MAX_STRING);
            assert_eq!(response["note"], "data");
        }
    }
    #[test]
    fn large_details_become_null_and_the_rest_of_the_frame_survives() {
        // Strings full of JSON syntax and escapes inside the dropped value.
        let noise = "}]\",{[\\".repeat(20_000);
        let big = json!({"results":[{"messages":[{"text":noise,"nested":{"details":[1,{}]}},{"a":1}],"b":2},3],"n":[[]]});
        let frames = [
            // Like Pi: `type` first, `details` between other keys.
            format!(
                "{{\"type\":\"tool_execution_end\",\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"done\"}}],\"details\" : {big}}},\"isError\":false}}\n"
            ),
            // Last key, and a scalar.
            format!("{}\n", json!({"message":{"details":"x".repeat(MAX_DETAILS * 2)}})),
            // Small values and `details` as a value, not a key, are kept.
            format!("{}\n", json!({"details":{"a":[1,2]},"b":"details","c":["details",{"d":1}]})),
        ]
        .concat();
        for chunk in [1, 7, 4096] {
            let mut scanner = LineScanner::default();
            let lines = scan(&mut scanner, frames.as_bytes(), chunk);
            assert_eq!(lines.len(), 3);
            let end = frame(&lines[0]);
            assert_eq!(end["result"]["details"], Value::Null, "chunk {chunk}");
            assert_eq!(end["result"]["content"][0]["text"], "done");
            assert_eq!(end["isError"], false);
            assert_eq!(end["type"], "tool_execution_end");
            assert_eq!(frame(&lines[1]), json!({"message":{"details":null}}));
            assert_eq!(
                frame(&lines[2]),
                json!({"details":{"a":[1,2]},"b":"details","c":["details",{"d":1}]})
            );
        }
    }
    #[test]
    fn details_pruning_matches_a_reference_on_random_frames() {
        // xorshift: deterministic, no dependency.
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        fn random(next: &mut impl FnMut(u64) -> u64, depth: u32) -> Value {
            let text = |next: &mut dyn FnMut(u64) -> u64| {
                let unit = ["a", "}", "]", ",", "\"", "\\", "ñ", "details", ":"][next(9) as usize];
                // Often short, often around and over MAX_DETAILS.
                let len = if next(3) == 0 {
                    next(MAX_DETAILS as u64)
                } else {
                    next(12)
                };
                Value::String(unit.repeat(len as usize))
            };
            match if depth == 0 { next(3) } else { next(5) } {
                0 => text(next),
                1 => json!(next(1000)),
                2 => [Value::Null, json!(true), json!([]), json!({})][next(4) as usize].clone(),
                3 => Value::Array((0..next(4)).map(|_| random(next, depth - 1)).collect()),
                _ => Value::Object(
                    (0..next(4))
                        .map(|i| {
                            let key = if next(3) == 0 {
                                "details".into()
                            } else {
                                format!("k{i}")
                            };
                            (key, random(next, depth - 1))
                        })
                        .collect(),
                ),
            }
        }
        // What the scanner should produce: oversized `details` values become null.
        fn reference(value: &Value) -> Value {
            match value {
                Value::Object(map) => Value::Object(
                    map.iter()
                        .map(|(k, v)| {
                            let large = serde_json::to_vec(v).unwrap().len() > MAX_DETAILS;
                            let v = if k == "details" && large {
                                Value::Null
                            } else if k == "details" {
                                v.clone()
                            } else {
                                reference(v)
                            };
                            (k.clone(), v)
                        })
                        .collect(),
                ),
                Value::Array(items) => Value::Array(items.iter().map(reference).collect()),
                other => other.clone(),
            }
        }
        let mut pruned = 0;
        for case in 0..300 {
            // A `details` value sized around the cap, among random siblings.
            let size = MAX_DETAILS - 64 + next(128) as usize;
            let edge = json!({"k":random(&mut next, 2),"details":["}".repeat(size), random(&mut next, 3)]});
            let value = json!({"type":"message_end","message":random(&mut next, 5),"edge":edge});
            let expected = reference(&value);
            pruned += usize::from(expected != value);
            let chunk = 1 + next(5000) as usize;
            let mut scanner = LineScanner::default();
            let lines = scan(&mut scanner, format!("{value}\n").as_bytes(), chunk);
            assert_eq!(lines.len(), 1, "case {case}");
            assert_eq!(frame(&lines[0]), expected, "case {case}, chunk {chunk}");
        }
        assert!(pruned > 10, "only {pruned} cases exercised pruning");
    }
    #[tokio::test]
    async fn frames_of_images_over_32_mib_do_not_break_the_stream() {
        let (host, peer) = tokio::io::duplex(1 << 16);
        let (read, write) = tokio::io::split(host);
        let (_peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::new(write, read);
        let image = "A".repeat(4 * 1024 * 1024);
        let images = json!([{"role":"toolResult","content":(0..10)
            .map(|_| json!({"type":"image","data":image,"mimeType":"image/png"}))
            .collect::<Vec<_>>()}]);
        let writer = tokio::spawn(async move {
            // A subagent result: 40 MiB of child transcript in `details`.
            let transcript = json!({"results":[{"messages":images}]});
            let frames = format!(
                "{{\"type\":\"agent_end\",\"messages\":{images}}}\n{{\"type\":\"message_end\",\"message\":{}}}\n{{\"type\":\"tool_execution_end\",\"toolCallId\":\"t\",\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"ok\"}}],\"details\":{transcript}}},\"isError\":false}}\n{}\n",
                images[0],
                json!({"type":"agent_settled"})
            );
            peer_write.write_all(frames.as_bytes()).await.unwrap();
            peer_write
        });
        // agent_end is dropped unread; message_end keeps its shape, not its images.
        let end = transport.incoming.recv().await.unwrap().unwrap();
        assert_eq!(end["type"], "message_end");
        let content = end["message"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 10);
        // The driver never reads image bytes, so only a marker is kept.
        let data = content[0]["data"].as_str().unwrap();
        assert!(data.len() <= MAX_DATA + 64, "{} bytes", data.len());
        assert!(data.ends_with("bytes omitted by Zeron]"));
        assert_eq!(content[0]["mimeType"], "image/png");
        let tool = transport.incoming.recv().await.unwrap().unwrap();
        assert_eq!(tool["type"], "tool_execution_end");
        assert_eq!(tool["result"]["content"][0]["text"], "ok");
        assert_eq!(tool["result"]["details"], Value::Null);
        let settled = transport.incoming.recv().await.unwrap().unwrap();
        assert_eq!(settled["type"], "agent_settled");
        drop(writer.await.unwrap());
    }
}
