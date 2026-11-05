//! Direct terminal links: one terminal's I/O over a peer-to-peer stream (the
//! preview peers' WebRTC DataChannel), so a remote terminal's keystrokes and
//! output never detour through the device-room relay.
//!
//! The stream is opened as service `terminal/{id}` on the host device and
//! speaks length-prefixed binary frames, `kind u8 ‖ len u32 BE ‖ payload`:
//!
//! - viewer → host: `ATTACH {afterSeq u64, client utf8}` (first frame,
//!   exactly once), `INPUT {seq u64, bytes}`, `RESIZE {cols u16, rows u16}`,
//!   `PING {nonce u64}`;
//! - host → viewer: `DATA {seq u64, bytes}`, `EXIT {seq u64, code i32}`,
//!   `ERROR utf8`, `PONG {nonce u64}`, `ACK {inputSeq u64}`.
//!
//! The host's side is an ordinary terminal subscription ([`HostTerminals`]),
//! so replay and `afterSeq` resume behave exactly as over the relay. Input is
//! numbered per viewer and acknowledged once applied: whatever a dying link
//! swallowed is replayed over the next path, and the host applies it exactly
//! once. PTY bytes travel raw; base64 only reappears at the viewer's
//! local RPC boundary.
//!
//! Each side reads frames on a task of its own: a frame read is several
//! awaits, and abandoning one midway (another `select!` branch winning) would
//! desynchronize the stream.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use zeron_proto::TerminalEvent;

/// A device's own terminals, as the link's host side serves them.
pub trait HostTerminals: Clone + Send + Sync + 'static {
    /// Events after `after_seq` (the replay window), then the live tail;
    /// the channel ends after `Exit`.
    fn subscribe(
        &self,
        terminal_id: &str,
        after_seq: u64,
    ) -> Result<mpsc::UnboundedReceiver<TerminalEvent>, String>;
    /// Apply input numbered `seq` by `client`, once (a replay of a seq
    /// already applied is dropped). `Ok(false)` = a duplicate.
    fn write_sequenced(
        &self,
        terminal_id: &str,
        client: &str,
        seq: u64,
        bytes: &[u8],
    ) -> Result<bool, String>;
    fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), String>;
}

/// Service id prefix the host serves terminal links under.
pub const SERVICE_PREFIX: &str = "terminal/";

const ATTACH: u8 = 1;
const INPUT: u8 = 2;
const RESIZE: u8 = 3;
const PING: u8 = 4;
const DATA: u8 = 16;
const EXIT: u8 = 17;
const ERROR: u8 = 18;
const PONG: u8 = 19;
const ACK: u8 = 20;

/// Largest frame either side accepts. Output batches are bounded by the
/// engine's pacing, input by `MAX_INPUT_BYTES`; this only stops runaways.
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// How often the viewer proves the path is alive.
const PING_INTERVAL: Duration = Duration::from_secs(1);
/// Silence after which the viewer declares the path dead. A DataChannel
/// whose network vanished otherwise looks open until ICE consent expires
/// (~30s), and typing into it would go nowhere.
const SILENCE_LIMIT: Duration = Duration::from_millis(3500);
/// The host drops a viewer it hasn't heard from (pings stopped), or whose
/// stream hasn't taken a frame, for this long — rather than buffering the
/// terminal's output for a peer that isn't reading.
const HOST_SILENCE_LIMIT: Duration = Duration::from_secs(8);
/// The host waits this long for `ATTACH` before dropping the stream.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

pub fn service_id(terminal_id: &str) -> String {
    format!("{SERVICE_PREFIX}{terminal_id}")
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    kind: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    writer.write_all(&frame).await
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<(u8, Vec<u8>)> {
    let kind = reader.read_u8().await?;
    let len = reader.read_u32().await? as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::other("oversized terminal frame"));
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload).await?;
    Ok((kind, payload))
}

/// Read whole frames on a task of its own; the channel ends with the stream.
fn spawn_reader<R>(mut reader: R) -> (mpsc::Receiver<(u8, Vec<u8>)>, tokio::task::JoinHandle<()>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::channel(64);
    let task = tokio::spawn(async move {
        while let Ok(frame) = read_frame(&mut reader).await {
            if tx.send(frame).await.is_err() {
                break;
            }
        }
    });
    (rx, task)
}

fn u64_at(payload: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        payload.get(at..at + 8)?.try_into().ok()?,
    ))
}

fn encode_event(event: &TerminalEvent) -> (u8, Vec<u8>) {
    match event {
        TerminalEvent::Data { seq, data } => {
            let bytes = BASE64.decode(data).unwrap_or_default();
            let mut payload = Vec::with_capacity(8 + bytes.len());
            payload.extend_from_slice(&seq.to_be_bytes());
            payload.extend_from_slice(&bytes);
            (DATA, payload)
        }
        TerminalEvent::Exit { seq, exit_code, .. } => {
            let mut payload = seq.to_be_bytes().to_vec();
            payload.extend_from_slice(&exit_code.to_be_bytes());
            (EXIT, payload)
        }
    }
}

fn decode_event(kind: u8, payload: &[u8]) -> Option<TerminalEvent> {
    let seq = u64_at(payload, 0)?;
    match kind {
        DATA => Some(TerminalEvent::Data {
            seq,
            data: BASE64.encode(&payload[8..]),
        }),
        EXIT => Some(TerminalEvent::Exit {
            seq,
            exit_code: i32::from_be_bytes(payload.get(8..12)?.try_into().ok()?),
            signal: None,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

/// A write that can't complete (the viewer stopped taking frames) ends the
/// link instead of letting the terminal's output pile up behind it.
async fn send_bounded<W: AsyncWrite + Unpin>(writer: &mut W, kind: u8, payload: &[u8]) -> bool {
    matches!(
        tokio::time::timeout(HOST_SILENCE_LIMIT, write_frame(writer, kind, payload)).await,
        Ok(Ok(()))
    )
}

/// Serve one viewer's link to `terminal_id` on `io` until either side ends.
pub async fn serve<T, S>(terminals: T, terminal_id: String, io: S)
where
    T: HostTerminals,
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(io);
    let attach = match tokio::time::timeout(ATTACH_TIMEOUT, read_frame(&mut reader)).await {
        Ok(Ok((ATTACH, payload))) => u64_at(&payload, 0).map(|after| {
            (
                after,
                String::from_utf8_lossy(payload.get(8..).unwrap_or_default()).into_owned(),
            )
        }),
        _ => None,
    };
    let Some((after_seq, client)) = attach.filter(|(_, client)| !client.is_empty()) else {
        let _ = write_frame(&mut writer, ERROR, b"expected attach").await;
        return;
    };
    let mut events = match terminals.subscribe(&terminal_id, after_seq) {
        Ok(events) => events,
        Err(err) => {
            let _ = write_frame(&mut writer, ERROR, err.as_bytes()).await;
            return;
        }
    };
    let (mut frames, reader_task) = spawn_reader(reader);
    let mut heard = tokio::time::Instant::now();
    let mut check = tokio::time::interval(PING_INTERVAL);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                let exit = matches!(event, TerminalEvent::Exit { .. });
                let (kind, payload) = encode_event(&event);
                if !send_bounded(&mut writer, kind, &payload).await || exit {
                    break;
                }
            }
            frame = frames.recv() => {
                let Some((kind, payload)) = frame else { break };
                heard = tokio::time::Instant::now();
                match kind {
                    INPUT if payload.len() >= 8 => {
                        let seq = u64_at(&payload, 0).unwrap_or(0);
                        if let Err(err) =
                            terminals.write_sequenced(&terminal_id, &client, seq, &payload[8..])
                        {
                            tracing::debug!(error = %err, "terminal link input failed");
                        }
                        if !send_bounded(&mut writer, ACK, &seq.to_be_bytes()).await {
                            break;
                        }
                    }
                    RESIZE if payload.len() == 4 => {
                        if let Err(err) = terminals.resize(
                            &terminal_id,
                            u16::from_be_bytes([payload[0], payload[1]]),
                            u16::from_be_bytes([payload[2], payload[3]]),
                        ) {
                            tracing::debug!(error = %err, "terminal link resize failed");
                        }
                    }
                    PING if payload.len() == 8 => {
                        if !send_bounded(&mut writer, PONG, &payload).await {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            _ = check.tick() => {
                if heard.elapsed() > HOST_SILENCE_LIMIT {
                    tracing::debug!("terminal link viewer went silent");
                    break;
                }
            }
        }
    }
    reader_task.abort();
    let _ = writer.shutdown().await;
}

// ---------------------------------------------------------------------------
// Viewer
// ---------------------------------------------------------------------------

/// What the viewer sends over an attached link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// Input numbered by the viewer (see the module docs).
    Input {
        seq: u64,
        bytes: Vec<u8>,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
}

/// The viewer's end of an attached link. Dropping every sender detaches.
pub struct Link {
    pub outgoing: mpsc::UnboundedSender<Outgoing>,
    /// Host events in order. Ends after `Exit`, or when the path dies
    /// (closed, or silent past [`SILENCE_LIMIT`]) — then the caller resumes
    /// from its last seq over whatever path is available.
    pub events: mpsc::UnboundedReceiver<TerminalEvent>,
    /// Highest input seq the host has applied, as it acknowledges them.
    pub acks: mpsc::UnboundedReceiver<u64>,
}

/// Attach to the host's terminal over `io` as viewer `client`, resuming
/// after `after_seq`. Fails when the host refuses (unknown terminal, not
/// allowed).
pub async fn attach<S>(io: S, after_seq: u64, client: &str) -> anyhow::Result<Link>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(io);
    let mut attach = after_seq.to_be_bytes().to_vec();
    attach.extend_from_slice(client.as_bytes());
    write_frame(&mut writer, ATTACH, &attach).await?;
    // The first frame tells us whether the host took the attach: a refusal
    // is an ERROR, a live terminal at least answers our first ping.
    write_frame(&mut writer, PING, &0u64.to_be_bytes()).await?;
    let first = tokio::time::timeout(ATTACH_TIMEOUT, read_frame(&mut reader))
        .await
        .map_err(|_| anyhow::anyhow!("terminal link timed out"))??;
    if first.0 == ERROR {
        anyhow::bail!("{}", String::from_utf8_lossy(&first.1));
    }

    let (outgoing, mut to_host) = mpsc::unbounded_channel::<Outgoing>();
    let (to_viewer, events) = mpsc::unbounded_channel();
    let (ack_tx, acks) = mpsc::unbounded_channel();
    let first_event = decode_event(first.0, &first.1);
    let (mut frames, reader_task) = spawn_reader(reader);
    tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ping.tick().await;
        let mut heard = tokio::time::Instant::now();
        let mut nonce = 0u64;
        let done = first_event.is_some_and(|event| {
            let exit = matches!(event, TerminalEvent::Exit { .. });
            to_viewer.send(event).is_err() || exit
        });
        if !done {
            loop {
                tokio::select! {
                    frame = frames.recv() => {
                        let Some((kind, payload)) = frame else { break };
                        heard = tokio::time::Instant::now();
                        match kind {
                            PONG => {}
                            ACK => {
                                if let Some(seq) = u64_at(&payload, 0) {
                                    let _ = ack_tx.send(seq);
                                }
                            }
                            ERROR => {
                                tracing::debug!(error = %String::from_utf8_lossy(&payload), "terminal link ended by host");
                                break;
                            }
                            _ => {
                                let Some(event) = decode_event(kind, &payload) else { break };
                                let exit = matches!(event, TerminalEvent::Exit { .. });
                                if to_viewer.send(event).is_err() || exit {
                                    break;
                                }
                            }
                        }
                    }
                    out = to_host.recv() => {
                        let sent = match out {
                            Some(Outgoing::Input { seq, bytes }) => {
                                let mut payload = Vec::with_capacity(8 + bytes.len());
                                payload.extend_from_slice(&seq.to_be_bytes());
                                payload.extend_from_slice(&bytes);
                                write_frame(&mut writer, INPUT, &payload).await
                            }
                            Some(Outgoing::Resize { cols, rows }) => {
                                let mut payload = cols.to_be_bytes().to_vec();
                                payload.extend_from_slice(&rows.to_be_bytes());
                                write_frame(&mut writer, RESIZE, &payload).await
                            }
                            None => break,
                        };
                        if sent.is_err() {
                            break;
                        }
                    }
                    _ = ping.tick() => {
                        if heard.elapsed() > SILENCE_LIMIT {
                            tracing::debug!("terminal link went silent");
                            break;
                        }
                        nonce += 1;
                        if write_frame(&mut writer, PING, &nonce.to_be_bytes()).await.is_err() {
                            break;
                        }
                    }
                    _ = to_viewer.closed() => break,
                }
            }
        }
        reader_task.abort();
        let _ = writer.shutdown().await;
    });
    Ok(Link {
        outgoing,
        events,
        acks,
    })
}
