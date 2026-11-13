//! The omp RPC process: one `omp --mode rpc-ui` child, its `ready` handshake,
//! protocol v2 and `rpc_chunk` reassembly. Line framing (and its tolerance of
//! stray output, lone surrogates and oversized lines) is Pi's transport: omp's
//! RPC descends from Pi's and frames the same way.
use super::dialogs::Dialogs;
use crate::{
    HarnessError,
    pi::rpc::{Client, Transport},
    process::{Command, Stdio, owned::Child},
};
use base64::Engine as _;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

/// omp's advertised ceiling on a reassembled v2 frame, when `ready` names none.
const DEFAULT_MAX_REASSEMBLED: u64 = 64 * 1024 * 1024;
/// Never reassemble more than this, whatever `ready` advertises.
const HARD_MAX_REASSEMBLED: u64 = 256 * 1024 * 1024;

pub(super) struct Process {
    pub child: Child,
    pub client: Client,
    pub tail: crate::StderrTail,
    pub dialogs: Dialogs,
    transport: Transport,
    stderr_task: tokio::task::JoinHandle<()>,
    exit_deadline: Option<tokio::time::Instant>,
    chunks: Reassembler,
}

impl Drop for Process {
    fn drop(&mut self) {
        self.stderr_task.abort();
    }
}

/// How a run's omp is launched.
pub(super) struct Launch<'a> {
    pub exe: &'a Path,
    pub cwd: &'a Path,
    /// `rpc-ui` for runs (a host that answers dialogs); `rpc` for discovery.
    pub mode: &'static str,
    pub args: &'a [String],
    pub env: &'a std::collections::BTreeMap<String, String>,
}

impl Process {
    pub fn spawn(launch: Launch<'_>) -> Result<Self, HarnessError> {
        let mut cmd = Command::new(launch.exe);
        // Process group plus the env scrubbing every agent launch gets.
        crate::process::owned::configure(&mut cmd);
        crate::compose_child_path(&mut cmd, launch.exe);
        crate::apply_run_env(&mut cmd, launch.env);
        cmd.args(["--mode", launch.mode])
            .args(launch.args)
            .current_dir(launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| {
            crate::executable::spawn_error(e, Some(launch.cwd), || {
                crate::executable::binary_hint(launch.exe)
            })
        })?;
        let mut child = Child::new(child);
        let tail = crate::StderrTail::default();
        let mut lines = crate::lossy_lines::lossy_lines(child.stderr.take().expect("piped stderr"));
        let stderr = tail.clone();
        // stderr is omp's log channel: kept as the crash message's tail only.
        let stderr_task = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                stderr.push(&line);
            }
            stderr.close();
        });
        let transport = Transport::named(
            child.stdin.take().expect("piped stdin"),
            child.stdout.take().expect("piped stdout"),
            "omp",
        );
        Ok(Self {
            child,
            client: transport.client.clone(),
            tail,
            dialogs: Dialogs::default(),
            transport,
            stderr_task,
            exit_deadline: None,
            chunks: Reassembler::new(DEFAULT_MAX_REASSEMBLED),
        })
    }

    /// Wait for `ready`, then move to protocol v2 (large frames as
    /// `rpc_chunk` sequences) when omp offers it. Frames before `ready`
    /// (there should be none) go to `backlog`.
    pub async fn handshake(&mut self, backlog: &mut Vec<Value>) -> Result<Value, HarnessError> {
        let ready = loop {
            let frame = self.next().await?;
            if frame["type"] == "ready" {
                break frame;
            }
            backlog.push(frame);
        };
        let ceiling = ready["maxReassembledFrameBytes"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_REASSEMBLED)
            .min(HARD_MAX_REASSEMBLED);
        self.chunks = Reassembler::new(ceiling);
        let v2 = ready["supportedProtocolVersions"]
            .as_array()
            .is_some_and(|versions| versions.iter().any(|v| v == 2));
        if v2 {
            self.query(
                json!({"type":"negotiate_protocol","protocolVersion":2}),
                backlog,
            )
            .await?;
        }
        Ok(ready)
    }

    /// The next logical frame: chunk sequences are reassembled, never
    /// dispatched piecewise.
    pub async fn next(&mut self) -> Result<Value, HarnessError> {
        loop {
            let frame = self.raw().await?;
            if frame["type"] == "rpc_chunk" {
                if let Some(frame) = self.chunks.push(&frame) {
                    return Ok(frame);
                }
                continue;
            }
            self.chunks.interrupted();
            return Ok(frame);
        }
    }

    async fn raw(&mut self) -> Result<Value, HarnessError> {
        loop {
            tokio::select! {
                // Drain buffered output before reporting process death. A
                // descendant may retain stdout, so EOF alone cannot govern
                // liveness.
                biased;
                _ = async {
                    match self.exit_deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => return Err(HarnessError::Protocol("omp exited".into())),
                status = self.child.wait(), if self.exit_deadline.is_none() => {
                    status?;
                    self.exit_deadline = Some(tokio::time::Instant::now() + Duration::from_millis(100));
                }
                frame = self.transport.incoming.recv() => {
                    return frame.unwrap_or_else(|| Err(HarnessError::Protocol("omp disconnected".into())));
                }
            }
        }
    }

    /// Send `command` and wait for its response, keeping every other frame
    /// in order in `backlog`. Dialogs are answered as they come: omp may
    /// hold the response until one is (an extension's startup prompt).
    pub async fn query(
        &mut self,
        command: Value,
        backlog: &mut Vec<Value>,
    ) -> Result<Value, HarnessError> {
        let id = self.client.request(command)?;
        loop {
            let frame = self.next().await?;
            if frame["type"] == "response" && frame["id"] == id.as_str() {
                return response_data(&frame);
            }
            if frame["type"] == "extension_ui_request" && Dialogs::interactive(&frame) {
                self.dialogs.request(self.client.clone(), &frame);
            } else {
                backlog.push(frame);
            }
        }
    }

    pub async fn shutdown(&mut self, grace: Duration) {
        self.dialogs.cancel(&self.client);
        self.transport.close_input_for_teardown();
        self.child.shutdown(grace).await;
    }
}

/// A response's `data`, or its error as a protocol error naming the command.
pub(super) fn response_data(frame: &Value) -> Result<Value, HarnessError> {
    if frame["success"] == true {
        Ok(frame["data"].clone())
    } else {
        Err(HarnessError::Protocol(format!(
            "omp {}: {}",
            frame["command"].as_str().unwrap_or("command"),
            frame["error"].as_str().unwrap_or("command failed")
        )))
    }
}

/// Reassembles protocol v2's `rpc_chunk` sequences, strictly as omp's RPC
/// document specifies: one non-empty `chunkId`, indices from 0 without gaps,
/// `count`/`byteLength` unchanged throughout, no other frame in between, the
/// bytes within the advertised ceiling, equal to `byteLength`, strict UTF-8
/// and one JSON object. A sequence breaking any rule is dropped (logged) and
/// the stream goes on: one lost frame, not a lost run.
pub(super) struct Reassembler {
    ceiling: u64,
    open: Option<Sequence>,
}

struct Sequence {
    id: String,
    count: u64,
    length: u64,
    next: u64,
    bytes: Vec<u8>,
}

impl Reassembler {
    pub fn new(ceiling: u64) -> Self {
        Self {
            ceiling,
            open: None,
        }
    }

    /// An ordinary frame arrived: a sequence still open can never complete.
    pub fn interrupted(&mut self) {
        if let Some(sequence) = self.open.take() {
            tracing::warn!(
                target: "zeron_harness::omp",
                chunk = %sequence.id,
                "omp rpc_chunk sequence interrupted by another frame; dropped"
            );
        }
    }

    pub fn push(&mut self, frame: &Value) -> Option<Value> {
        match self.accept(frame) {
            Ok(done) => done,
            Err(reason) => {
                self.open = None;
                tracing::warn!(target: "zeron_harness::omp", "omp rpc_chunk dropped: {reason}");
                None
            }
        }
    }

    fn accept(&mut self, frame: &Value) -> Result<Option<Value>, &'static str> {
        let id = frame["chunkId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("no chunkId")?;
        let index = frame["index"].as_u64().ok_or("no index")?;
        let count = frame["count"]
            .as_u64()
            .filter(|c| *c > 0)
            .ok_or("no count")?;
        let length = frame["byteLength"].as_u64().ok_or("no byteLength")?;
        if length > self.ceiling {
            return Err("over the reassembly ceiling");
        }
        let data = base64::engine::general_purpose::STANDARD
            .decode(frame["data"].as_str().ok_or("no data")?)
            .map_err(|_| "data is not base64")?;
        if index == 0 {
            if self.open.is_some() {
                // A new sequence began before the last completed.
                self.interrupted();
            }
            self.open = Some(Sequence {
                id: id.to_owned(),
                count,
                length,
                next: 0,
                bytes: Vec::new(),
            });
        }
        let sequence = self
            .open
            .as_mut()
            .ok_or("a sequence must start at index 0")?;
        if sequence.id != id
            || sequence.count != count
            || sequence.length != length
            || sequence.next != index
        {
            return Err("chunk out of sequence");
        }
        if sequence.bytes.len() as u64 + data.len() as u64 > length {
            return Err("more bytes than byteLength");
        }
        sequence.bytes.extend_from_slice(&data);
        sequence.next += 1;
        if sequence.next < count {
            return Ok(None);
        }
        let sequence = self.open.take().expect("open sequence");
        if sequence.bytes.len() as u64 != length {
            return Err("fewer bytes than byteLength");
        }
        let text = String::from_utf8(sequence.bytes).map_err(|_| "not UTF-8")?;
        match serde_json::from_str::<Value>(&text) {
            Ok(value) if value.is_object() => Ok(Some(value)),
            _ => Err("not one JSON object"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks(id: &str, frame: &Value, pieces: usize) -> Vec<Value> {
        let bytes = serde_json::to_vec(frame).unwrap();
        let size = bytes.len().div_ceil(pieces);
        bytes
            .chunks(size)
            .enumerate()
            .map(|(index, piece)| {
                json!({
                    "type": "rpc_chunk",
                    "chunkId": id,
                    "index": index,
                    "count": bytes.len().div_ceil(size),
                    "byteLength": bytes.len(),
                    "data": base64::engine::general_purpose::STANDARD.encode(piece),
                })
            })
            .collect()
    }

    #[test]
    fn a_chunked_frame_reassembles_once_complete() {
        let frame = json!({"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"é".repeat(5000)}]}});
        let mut r = Reassembler::new(1 << 20);
        let parts = chunks("rpc-1", &frame, 4);
        assert_eq!(parts.len(), 4);
        for part in &parts[..3] {
            assert_eq!(r.push(part), None);
        }
        assert_eq!(r.push(&parts[3]), Some(frame));
    }

    #[test]
    fn broken_sequences_are_dropped_and_the_next_one_still_reassembles() {
        let frame = json!({"type":"tool_execution_end","toolCallId":"t","result":{"content":[{"type":"text","text":"x".repeat(900)}]}});
        let mut r = Reassembler::new(1 << 20);
        let parts = chunks("a", &frame, 3);
        // A gap.
        assert_eq!(r.push(&parts[0]), None);
        assert_eq!(r.push(&parts[2]), None);
        // Starting mid-sequence.
        assert_eq!(r.push(&parts[1]), None);
        // Interrupted by an ordinary frame.
        assert_eq!(r.push(&parts[0]), None);
        r.interrupted();
        assert_eq!(r.push(&parts[1]), None);
        // A changed byteLength.
        let mut lying = parts.clone();
        lying[1]["byteLength"] = json!(1);
        assert_eq!(r.push(&lying[0]), None);
        assert_eq!(r.push(&lying[1]), None);
        // Over the ceiling.
        let mut small = Reassembler::new(10);
        assert_eq!(small.push(&parts[0]), None);
        // Not UTF-8, and not an object.
        let bad = |bytes: &[u8]| {
            json!({"type":"rpc_chunk","chunkId":"b","index":0,"count":1,"byteLength":bytes.len(),
                   "data":base64::engine::general_purpose::STANDARD.encode(bytes)})
        };
        assert_eq!(r.push(&bad(&[0xff, 0xfe])), None);
        assert_eq!(r.push(&bad(b"[1,2]")), None);
        // A clean sequence afterwards.
        for part in &parts[..2] {
            assert_eq!(r.push(part), None);
        }
        assert_eq!(r.push(&parts[2]), Some(frame));
    }

    #[tokio::test]
    async fn the_process_reads_chunks_as_one_frame_between_ordinary_ones() {
        use tokio::io::AsyncWriteExt;
        let (host, peer) = tokio::io::duplex(1 << 16);
        let (read, write) = tokio::io::split(host);
        let (_peer_read, mut peer_write) = tokio::io::split(peer);
        let mut transport = Transport::named(write, read, "omp");
        let mut chunks_state = Reassembler::new(1 << 20);
        let big = json!({"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"y".repeat(3000)}});
        let mut lines = String::new();
        lines.push_str("{\"type\":\"agent_start\"}\n");
        for part in chunks("c", &big, 3) {
            lines.push_str(&format!("{part}\n"));
        }
        lines.push_str("{\"type\":\"agent_end\"}\n");
        peer_write.write_all(lines.as_bytes()).await.unwrap();
        let mut out = vec![];
        while out.len() < 3 {
            let frame = transport.incoming.recv().await.unwrap().unwrap();
            if frame["type"] == "rpc_chunk" {
                if let Some(done) = chunks_state.push(&frame) {
                    out.push(done);
                }
            } else {
                chunks_state.interrupted();
                out.push(frame);
            }
        }
        assert_eq!(out[0]["type"], "agent_start");
        assert_eq!(out[1], big);
        assert_eq!(out[2]["type"], "agent_end");
    }
}
