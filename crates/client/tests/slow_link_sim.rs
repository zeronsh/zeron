//! Simulation (not a regression test; `--ignored`): how long the newest rows
//! of a running chat take to reach the phone over a slow relay, with frame
//! sizes measured on Villa's 0.2.101 engine (2026-10-01): opening tail ~56 KB,
//! full reset 1.7 MB / 8.4 MB, a running turn re-sent as an ~80 KB delta
//! every ~2 s.
//!
//! The link: one shared bottleneck (RATE bytes/s, LAT_MS one-way), every
//! tunnel channel buffered up to its SSH window, interleaved in 16 KB slices
//! (sshd round-robin), a small non-droppable send queue (kernel/DERP) in
//! front of the bottleneck. Time is compressed by SCALE (rates ×SCALE,
//! latencies and intervals ÷SCALE); reported times are real-link seconds.
//!
//! Run: cargo test -p zeron-client --test slow_link_sim -- --ignored --nocapture

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zeron_rpc::{RpcError, RpcReply, RpcService};

fn env(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn scale() -> f64 {
    env("SCALE", 10.0)
}
/// A real-link duration, compressed.
fn real(secs: f64) -> Duration {
    Duration::from_secs_f64(secs / scale())
}
/// Elapsed compressed time as real-link seconds.
fn secs(d: Duration) -> f64 {
    d.as_secs_f64() * scale()
}

// ── engine ─────────────────────────────────────────────────────────────────

/// JSON-ish text that deflates (level 1) about as well as real transcript
/// frames (≈3.1×).
fn filler(len: usize, seed: &mut u64) -> String {
    const WORDS: [&str; 48] = [
        "the",
        "file",
        "test",
        "error",
        "function",
        "return",
        "value",
        "let",
        "mut",
        "self",
        "crate",
        "client",
        "session",
        "transcript",
        "message",
        "update",
        "frame",
        "engine",
        "android",
        "kotlin",
        "rust",
        "build",
        "cargo",
        "path",
        "line",
        "string",
        "json",
        "type",
        "text",
        "tool",
        "output",
        "input",
        "status",
        "running",
        "done",
        "check",
        "phone",
        "link",
        "relay",
        "parts",
        "reasoning",
        "should",
        "because",
        "which",
        "when",
        "then",
        "with",
        "and",
    ];
    let mut s = String::with_capacity(len + 32);
    while s.len() < len {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        let r = *seed;
        if r % 29 == 0 {
            s.push_str(&format!("{:016x} ", r));
        } else {
            s.push_str(WORDS[(r % 48) as usize]);
            s.push(if r % 11 == 0 { '\n' } else { ' ' });
        }
    }
    s.truncate(len);
    s
}

fn frame(bytes: usize, pending: bool, seed: &mut u64) -> serde_json::Value {
    let mut v = serde_json::json!({ "reset": [], "pad": filler(bytes, seed) });
    if pending {
        v["historyPending"] = serde_json::Value::Bool(true);
    }
    v
}

struct Engine;

#[async_trait::async_trait]
impl RpcService for Engine {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        if method != "WatchDocMessages" {
            return Ok(RpcReply::Value(serde_json::json!({})));
        }
        let chat = params["chatId"].as_str().unwrap_or("").to_owned();
        let full_kb = if chat == "big" { 8430 } else { 1718 };
        let mut seed = 0x9e3779b97f4a7c15u64 ^ chat.len() as u64;
        let tail = frame(56 * 1024, true, &mut seed);
        let full = frame(full_kb * 1024, false, &mut seed);
        // A running turn: the whole live message (~80 KB) again every ~2 s.
        // An `interval` that skips missed ticks stands in for the engine's
        // watch channel, which coalesces while the connection is backed up.
        let mut tick = tokio::time::interval(real(2.0));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let deltas = futures::stream::unfold((tick, seed), |(mut tick, mut seed)| async move {
            tick.tick().await;
            let mut v = serde_json::json!({ "delta": true, "pad": filler(80 * 1024, &mut seed) });
            v["delta"] = serde_json::Value::Bool(true);
            Some((v, (tick, seed)))
        });
        Ok(RpcReply::Stream(
            futures::stream::iter([tail, full])
                .chain(deltas.skip(1))
                .boxed(),
        ))
    }
}

// ── the slow link ──────────────────────────────────────────────────────────

const SLICE: usize = 16 * 1024;

struct Conn {
    /// sshd's buffer for this channel (≤ the SSH window); dropped on close.
    buf: Mutex<VecDeque<u8>>,
    closed: AtomicBool,
    deliver: tokio::sync::mpsc::UnboundedSender<(Instant, Vec<u8>)>,
}

struct Link {
    conns: Mutex<Vec<Arc<Conn>>>,
    window: usize,
    compress: bool,
    lat: Duration,
}

impl Link {
    fn close_all(&self) {
        for c in lock(&self.conns).iter() {
            c.closed.store(true, Ordering::Release);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}

/// The bottleneck: a token bucket draining a small shared send queue that is
/// refilled round-robin from every open channel's buffer.
fn spawn_scheduler(link: Arc<Link>) {
    let rate = env("RATE", 20.0 * 1024.0) * scale();
    let wire_cap = env("WIRE_KB", 64.0) as usize * 1024;
    tokio::spawn(async move {
        let mut wire: VecDeque<(Arc<Conn>, Vec<u8>)> = VecDeque::new();
        let mut wire_bytes = 0usize;
        let mut rr = 0usize;
        let mut tokens = 0f64;
        let mut last = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(2)).await;
            let now = Instant::now();
            tokens = (tokens + rate * (now - last).as_secs_f64()).min(SLICE as f64);
            last = now;
            // Refill the send queue (sshd packs channels when it's short).
            loop {
                if wire_bytes >= wire_cap {
                    break;
                }
                let conns = lock(&link.conns).clone();
                let live: Vec<_> = conns
                    .iter()
                    .filter(|c| !c.closed.load(Ordering::Acquire) && !lock(&c.buf).is_empty())
                    .cloned()
                    .collect();
                if live.is_empty() {
                    break;
                }
                let c = live[rr % live.len()].clone();
                rr += 1;
                let chunk: Vec<u8> = {
                    let mut b = lock(&c.buf);
                    let n = b.len().min(SLICE);
                    b.drain(..n).collect()
                };
                wire_bytes += chunk.len();
                wire.push_back((c, chunk));
            }
            // Transmit what the bucket allows.
            while let Some((c, chunk)) = wire.front_mut() {
                if tokens < 1.0 {
                    break;
                }
                let n = (tokens as usize).min(chunk.len());
                let part: Vec<u8> = chunk.drain(..n).collect();
                tokens -= n as f64;
                wire_bytes -= n;
                let _ = c.deliver.send((now + link.lat, part));
                if chunk.is_empty() {
                    wire.pop_front();
                }
            }
        }
    });
}

async fn proxy(link: Arc<Link>, engine: std::net::SocketAddr) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_scheduler(link.clone());
    tokio::spawn(async move {
        loop {
            let (client, _) = listener.accept().await.unwrap();
            let server = TcpStream::connect(engine).await.unwrap();
            client.set_nodelay(true).ok();
            server.set_nodelay(true).ok();
            let (mut c_rd, mut c_wr) = client.into_split();
            let (mut s_rd, mut s_wr) = server.into_split();
            let (deliver, mut deliveries) = tokio::sync::mpsc::unbounded_channel();
            let conn = Arc::new(Conn {
                buf: Mutex::new(VecDeque::new()),
                closed: AtomicBool::new(false),
                deliver,
            });
            lock(&link.conns).push(conn.clone());
            // Up: client → engine after the one-way latency (not throttled).
            let lat = link.lat;
            let up = conn.clone();
            tokio::spawn(async move {
                let mut b = vec![0u8; 64 * 1024];
                loop {
                    match c_rd.read(&mut b).await {
                        Ok(0) | Err(_) => {
                            // The phone closed the channel; sshd learns of it a
                            // one-way trip later and drops what it buffered.
                            tokio::time::sleep(lat).await;
                            up.closed.store(true, Ordering::Release);
                            lock(&up.buf).clear();
                            return;
                        }
                        Ok(n) => {
                            let bytes = b[..n].to_vec();
                            tokio::time::sleep(lat).await;
                            if s_wr.write_all(&bytes).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
            // Down, engine side: read only while the channel's window has room.
            let down = conn.clone();
            let window = link.window;
            let compress = link.compress;
            tokio::spawn(async move {
                let mut z = flate2::Compress::new(flate2::Compression::fast(), true);
                let mut b = vec![0u8; 32 * 1024];
                loop {
                    if down.closed.load(Ordering::Acquire) {
                        return; // drops the engine socket
                    }
                    if lock(&down.buf).len() >= window {
                        tokio::time::sleep(Duration::from_millis(2)).await;
                        continue;
                    }
                    let n = match tokio::time::timeout(Duration::from_millis(50), s_rd.read(&mut b))
                        .await
                    {
                        Err(_) => continue,
                        Ok(Ok(0)) | Ok(Err(_)) => return,
                        Ok(Ok(n)) => n,
                    };
                    let out = if compress {
                        let mut out = Vec::with_capacity(n + 64);
                        z.compress_vec(&b[..n], &mut out, flate2::FlushCompress::Sync)
                            .unwrap();
                        while out.len() == out.capacity() {
                            out.reserve(4096);
                            z.compress_vec(&[], &mut out, flate2::FlushCompress::Sync)
                                .unwrap();
                        }
                        out
                    } else {
                        b[..n].to_vec()
                    };
                    lock(&down.buf).extend(out);
                }
            });
            // Down, phone side: arrives after the latency; dropped if closed.
            let closed = conn.clone();
            tokio::spawn(async move {
                let mut z = flate2::Decompress::new(true);
                while let Some((at, bytes)) = deliveries.recv().await {
                    tokio::time::sleep_until(at.into()).await;
                    if closed.closed.load(Ordering::Acquire) {
                        continue;
                    }
                    let plain = if compress {
                        let mut out = Vec::with_capacity(bytes.len() * 8 + 1024);
                        let mut input = &bytes[..];
                        loop {
                            let before_in = z.total_in();
                            z.decompress_vec(input, &mut out, flate2::FlushDecompress::Sync)
                                .unwrap();
                            let used = (z.total_in() - before_in) as usize;
                            input = &input[used..];
                            if input.is_empty() && out.len() < out.capacity() {
                                break;
                            }
                            out.reserve(out.capacity().max(4096));
                        }
                        out
                    } else {
                        bytes
                    };
                    if c_wr.write_all(&plain).await.is_err() {
                        closed.closed.store(true, Ordering::Release);
                    }
                }
            });
        }
    });
    addr
}

// ── scenarios ──────────────────────────────────────────────────────────────

#[derive(Default, Debug, Clone)]
struct Seen {
    tail: Option<f64>,
    full: Option<f64>,
    deltas: usize,
}

async fn watch(
    rpc: &zeron_rpc::RpcClient,
    chat: &'static str,
) -> (zeron_rpc::RpcSubscription, Arc<Mutex<Seen>>) {
    let sub = rpc
        .subscribe_scoped(
            "WatchDocMessages",
            serde_json::json!({ "chatId": chat, "openingTail": true }),
        )
        .await
        .unwrap();
    (sub, Arc::new(Mutex::new(Seen::default())))
}

/// Record when each kind of frame lands (real-link seconds since `began`).
fn record(
    mut sub: zeron_rpc::RpcSubscription,
    seen: Arc<Mutex<Seen>>,
    began: Instant,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(v) = sub.recv().await {
            let t = secs(began.elapsed());
            let mut s = lock(&seen);
            if v.get("historyPending").is_some() {
                s.tail.get_or_insert(t);
            } else if v.get("reset").is_some() {
                s.full.get_or_insert(t);
            } else {
                s.deltas += 1;
            }
        }
    })
}

async fn wait_tail(seen: &Arc<Mutex<Seen>>, budget: f64) -> Option<f64> {
    let began = Instant::now();
    loop {
        if let Some(t) = lock(seen).tail {
            return Some(t);
        }
        if secs(began.elapsed()) > budget {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[derive(Clone, Copy, Debug)]
struct Setup {
    /// One WebSocket per open transcript, closed on leaving (else one shared).
    per_chat: bool,
    /// SSH zlib (both ends).
    compress: bool,
    /// Leaving a chat cancels its stream this long after (the 5 s beat).
    cancel_after: f64,
    /// Per-chat: a spare channel is already open (else opening one costs a
    /// channel-open round trip before the WebSocket handshake).
    spare: bool,
}

async fn scenario(name: &str, setup: Setup, first: &'static str, second: &'static str) {
    let engine_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let engine_addr = engine_listener.local_addr().unwrap();
    tokio::spawn(zeron_rpc::serve_ws_listener(
        engine_listener,
        Arc::new(Engine),
    ));
    let link = Arc::new(Link {
        conns: Mutex::new(Vec::new()),
        window: env("WINDOW_KB", 2048.0) as usize * 1024,
        compress: setup.compress,
        lat: real(env("LAT_MS", 800.0) / 1000.0),
    });
    let addr = proxy(link.clone(), engine_addr).await;
    let url = format!("ws://{addr}/");
    let shared = zeron_rpc::connect_ws(&url).await.unwrap();
    let dwell = env("DWELL", 3.0);

    // Open the first chat on an idle link.
    let t0 = Instant::now();
    let rpc_a = if setup.per_chat {
        Some(open_channel(&url, setup.spare, link.lat).await)
    } else {
        None
    };
    let (sub_a, seen_a) = watch(rpc_a.as_ref().unwrap_or(&shared), first).await;
    let rec_a = record(sub_a, seen_a.clone(), t0);
    let a_tail = wait_tail(&seen_a, 600.0).await;
    // Look at it a moment, then go back and open the second.
    tokio::time::sleep(real(dwell)).await;
    let t1 = Instant::now();
    let rpc_b = if setup.per_chat {
        Some(open_channel(&url, setup.spare, link.lat).await)
    } else {
        None
    };
    let (sub_b, seen_b) = watch(rpc_b.as_ref().unwrap_or(&shared), second).await;
    let rec_b = record(sub_b, seen_b.clone(), t1);
    // The first chat's stream stops a beat later (its own channel closes).
    let cancel_after = setup.cancel_after;
    let per_chat = setup.per_chat;
    let link_for_close = link.clone();
    let closer = tokio::spawn(async move {
        tokio::time::sleep(real(cancel_after)).await;
        rec_a.abort(); // drops the subscription: {cancel} goes out
        if per_chat {
            // The fix closes the SSH channel explicitly (dropping the
            // RpcClient alone leaves the socket open until the WebSocket
            // message in progress finishes): sshd hears it a one-way trip
            // later and discards what it buffered for that channel.
            let a = lock(&link_for_close.conns)[1].clone();
            tokio::time::sleep(link_for_close.lat).await;
            a.closed.store(true, Ordering::Release);
            lock(&a.buf).clear();
        }
        drop(rpc_a);
    });
    let b_tail = wait_tail(&seen_b, 900.0).await;
    // How far the second chat gets in the next minute.
    tokio::time::sleep(real(60.0)).await;
    let b = lock(&seen_b).clone();
    println!(
        "{name:<44} first({first}) tail {:>6}  | second({second}) tail {:>6}  full {:>7}  deltas in +60s: {}",
        fmt(a_tail),
        fmt(b_tail),
        fmt(b.full),
        b.deltas,
    );
    let _ = closer.await;
    rec_b.abort();
    drop(rpc_b);
    drop(shared);
    link.close_all();
}

async fn open_channel(url: &str, spare: bool, lat: Duration) -> zeron_rpc::RpcClient {
    if !spare {
        // SSH_MSG_CHANNEL_OPEN → CONFIRMATION.
        tokio::time::sleep(lat * 2).await;
    }
    zeron_rpc::connect_ws(url).await.unwrap()
}

fn fmt(t: Option<f64>) -> String {
    t.map_or("  >max".into(), |t| format!("{t:5.1}s"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn slow_link_newest_rows() {
    println!(
        "link: {} KB/s, one-way {} ms, window {} KB, time ÷{}",
        env("RATE", 20480.0) / 1024.0,
        env("LAT_MS", 800.0),
        env("WINDOW_KB", 2048.0),
        scale()
    );
    let now = Setup {
        per_chat: false,
        compress: false,
        cancel_after: 2.5,
        spare: false,
    };
    let only = std::env::var("ONLY").unwrap_or_default();
    let per = Setup {
        per_chat: true,
        cancel_after: 0.0,
        ..now
    };
    let all: [(&str, Setup); 6] = [
        ("round5-12 (shared transcript WS, no zlib)", now),
        (
            "zlib only",
            Setup {
                compress: true,
                ..now
            },
        ),
        ("channel per chat (fresh), closed on leave", per),
        (
            "channel per chat (spare), closed on leave",
            Setup { spare: true, ..per },
        ),
        (
            "channel per chat (fresh) + zlib",
            Setup {
                compress: true,
                ..per
            },
        ),
        (
            "channel per chat (spare) + zlib",
            Setup {
                compress: true,
                spare: true,
                ..per
            },
        ),
    ];
    for (name, setup) in all {
        if !only.is_empty() && !name.contains(&only) {
            continue;
        }
        scenario(name, setup, "small", "big").await;
        scenario(name, setup, "big", "small").await;
    }
}

#[test]
#[ignore]
fn filler_deflates_like_a_transcript() {
    use std::io::Write;
    let mut seed = 7u64;
    let text = serde_json::to_string(&frame(1024 * 1024, false, &mut seed)).unwrap();
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(text.as_bytes()).unwrap();
    let out = z.finish().unwrap();
    println!("filler ratio {:.2}", text.len() as f64 / out.len() as f64);
}

// ── the real client: older rows over a slow relay ──────────────────────────
//
// The phone's own Direct client (SSH tunnel, a channel per open transcript,
// the backlog guard) against a 0.2.101-shaped engine behind the slow link
// above (one TCP connection now: the SSH session, zlib by russh). The
// transcript is Villa's measured shape: a long history, then a live row of
// ~1290 parts (~484 KB) that a busy turn re-sends whole every ~3.2 s; the
// opening tail is its last 128 parts. Protocol timers inside the client
// (probe, beat) are not compressed; the backlog check is (÷SCALE).
//
// Run: cargo test -p zeron-client --test slow_link_sim history -- --ignored --nocapture

mod history_sim {
    use super::*;
    use russh::server::{Msg, Server as _, Session};
    use russh::{Channel, ChannelId};
    use std::sync::atomic::AtomicU64;
    use zeron_doc::{
        MessagePart, MessageRole, MessageStatus, SessionMessageEntry, TranscriptFrame,
    };

    fn fixture(name: &str) -> serde_json::Value {
        let text = match name {
            "EngineInfo" => include_str!("../src/direct/fixtures/EngineInfo.json"),
            "WatchDevices" => include_str!("../src/direct/fixtures/WatchDevices.json"),
            "WatchSpaces" => include_str!("../src/direct/fixtures/WatchSpaces.json"),
            "WatchChats" => include_str!("../src/direct/fixtures/WatchChats.json"),
            "WatchSessions" => include_str!("../src/direct/fixtures/WatchSessions.json"),
            "ListHarnesses" => include_str!("../src/direct/fixtures/ListHarnesses.json"),
            "ListModels" => include_str!("../src/direct/fixtures/ListModels.codex.json"),
            other => panic!("no fixture {other}"),
        };
        serde_json::from_str(text).unwrap()
    }

    fn text_entry(id: &str, role: MessageRole, len: usize, seed: &mut u64) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: format!("{id}-t"),
                text: filler(len, seed),
            }],
            created_at: 1_700_000_000_000,
            device_id: "pc".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    /// The engine's `read_opening_tail(128)`: the newest 128 parts, the
    /// first row cut to its last parts.
    fn opening_tail(entries: &[SessionMessageEntry]) -> Vec<SessionMessageEntry> {
        let mut out = Vec::new();
        let mut left = 128usize;
        for e in entries.iter().rev() {
            if left == 0 {
                break;
            }
            let mut e = e.clone();
            if e.parts.len() > left {
                e.parts = e.parts[e.parts.len() - left..].to_vec();
            }
            left -= e.parts.len();
            out.push(e);
        }
        out.reverse();
        out
    }

    pub(super) struct Shape {
        /// Raw size of the complete reset (KB).
        pub full_kb: usize,
        /// A turn runs: the live row grows a part and is re-sent whole.
        pub busy: bool,
    }

    pub(super) struct Villa {
        pub history: Vec<SessionMessageEntry>,
        pub live: Mutex<SessionMessageEntry>,
        pub busy: bool,
        pub watches: AtomicU64,
    }

    impl Villa {
        pub fn new(shape: &Shape) -> Arc<Self> {
            let mut seed = 0x51u64;
            // ~1290 parts, ~484 KB.
            let mut live = text_entry("live", MessageRole::Assistant, 0, &mut seed);
            live.status = Some(MessageStatus::Streaming);
            live.parts = (0..1290)
                .map(|i| MessagePart::Text {
                    id: format!("p{i}"),
                    text: filler(330, &mut seed),
                })
                .collect();
            let history_bytes = (shape.full_kb * 1024).saturating_sub(484 * 1024);
            let rows = (history_bytes / (12 * 1024)).max(4);
            let history = (0..rows)
                .map(|i| {
                    let role = if i % 2 == 0 {
                        MessageRole::User
                    } else {
                        MessageRole::Assistant
                    };
                    text_entry(&format!("h{i:04}"), role, 12 * 1024 - 200, &mut seed)
                })
                .collect();
            Arc::new(Self {
                history,
                live: Mutex::new(live),
                busy: shape.busy,
                watches: AtomicU64::new(0),
            })
        }

        pub fn rows(&self) -> usize {
            self.history.len() + 1
        }

        fn entries(&self) -> Vec<SessionMessageEntry> {
            let mut all = self.history.clone();
            all.push(lock(&self.live).clone());
            all
        }
    }

    pub(super) struct VillaEngine(pub Arc<Villa>);

    #[async_trait::async_trait]
    impl RpcService for VillaEngine {
        async fn handle(
            &self,
            method: &str,
            _params: serde_json::Value,
        ) -> Result<RpcReply, RpcError> {
            match method {
                "EngineInfo" | "ListHarnesses" | "ListModels" => {
                    Ok(RpcReply::Value(fixture(method)))
                }
                "WatchDevices" | "WatchSpaces" | "WatchChats" | "WatchSessions" => {
                    Ok(RpcReply::Stream(
                        futures::stream::iter([fixture(method)])
                            .chain(futures::stream::pending())
                            .boxed(),
                    ))
                }
                "WatchDocMessages" => {
                    self.0.watches.fetch_add(1, Ordering::SeqCst);
                    let entries = self.0.entries();
                    let mut tail = serde_json::json!({
                        "reset": opening_tail(&entries)
                    });
                    tail["historyPending"] = serde_json::Value::Bool(true);
                    let full = serde_json::to_value(TranscriptFrame::reset(&entries)).unwrap();
                    let villa = self.0.clone();
                    let mut tick = tokio::time::interval(real(3.2));
                    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    // Like the engine: the next frame is made when the last
                    // one is handed on (its queue holds up to 256).
                    let deltas = futures::stream::unfold(
                        (tick, villa, 0u64),
                        |(mut tick, villa, mut n)| async move {
                            tick.tick().await;
                            if !villa.busy {
                                futures::future::pending::<()>().await;
                            }
                            n += 1;
                            let (after, entry) = {
                                let mut live = lock(&villa.live);
                                let mut seed = n;
                                let id = format!("q{}", live.parts.len());
                                live.parts.push(MessagePart::Text {
                                    id,
                                    text: filler(330, &mut seed),
                                });
                                (villa.history.last().map(|e| e.id.clone()), live.clone())
                            };
                            let frame = serde_json::json!({
                                "upsert": [{ "after": after, "entry": entry }],
                                "append": [], "remove": [], "count": villa.rows()
                            });
                            Some((frame, (tick, villa, n)))
                        },
                    );
                    Ok(RpcReply::Stream(
                        futures::stream::iter([tail, full])
                            .chain(deltas.skip(1))
                            .boxed(),
                    ))
                }
                _ => Ok(RpcReply::Value(serde_json::json!({}))),
            }
        }
    }

    #[derive(Clone)]
    struct Sshd {
        engine_port: u16,
        opened: Arc<AtomicU64>,
    }

    impl russh::server::Server for Sshd {
        type Handler = Self;
        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl russh::server::Handler for Sshd {
        type Error = russh::Error;

        async fn auth_publickey(
            &mut self,
            _: &str,
            _: &russh::keys::ssh_key::PublicKey,
        ) -> Result<russh::server::Auth, Self::Error> {
            Ok(russh::server::Auth::Accept)
        }

        async fn channel_open_direct_tcpip(
            &mut self,
            channel: Channel<Msg>,
            _host: &str,
            _port: u32,
            _oa: &str,
            _op: u32,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            let port = self.engine_port;
            self.opened.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut engine = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut engine).await;
                let _ = engine.shutdown().await;
            });
            Ok(())
        }

        async fn channel_close(
            &mut self,
            _: ChannelId,
            _: &mut Session,
        ) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    pub(super) struct Machine {
        pub target: zeron_client::direct::SshTarget,
        pub opened: Arc<AtomicU64>,
        pub link: Arc<Link>,
    }

    /// The engine behind an SSH server offering zlib, behind the slow link.
    pub(super) async fn machine(engine: Arc<dyn RpcService>) -> Machine {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let engine_port = listener.local_addr().unwrap().port();
        tokio::spawn(zeron_rpc::serve_ws_listener(listener, engine));
        let host_key = russh::keys::PrivateKey::from(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[9u8; 32]),
        );
        let fingerprint = host_key
            .public_key()
            .fingerprint(russh::keys::HashAlg::Sha256)
            .to_string();
        static OFFER: [russh::compression::Name; 2] =
            [russh::compression::NONE, russh::compression::ZLIB_LEGACY];
        let config = Arc::new(russh::server::Config {
            keys: vec![host_key],
            auth_rejection_time: Duration::from_millis(1),
            preferred: russh::Preferred {
                compression: std::borrow::Cow::Borrowed(&OFFER),
                ..russh::Preferred::DEFAULT
            },
            ..Default::default()
        });
        let sshd = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sshd_addr = sshd.local_addr().unwrap();
        let opened = Arc::new(AtomicU64::new(0));
        let mut server = Sshd {
            engine_port,
            opened: opened.clone(),
        };
        tokio::spawn(async move {
            let _ = server.run_on_socket(config, &sshd).await;
        });
        let link = Arc::new(Link {
            conns: Mutex::new(Vec::new()),
            // The SSH session's socket buffers on the computer's side.
            window: env("WINDOW_KB", 256.0) as usize * 1024,
            compress: false,
            lat: real(env("LAT_MS", 800.0) / 1000.0),
        });
        let addr = proxy(link.clone(), sshd_addr).await;
        let key = zeron_client::direct::generate_ed25519("sim").unwrap();
        let target = zeron_client::direct::SshTarget {
            host: "127.0.0.1".into(),
            port: addr.port(),
            user: "villa".into(),
            auth: zeron_client::direct::SshAuth::Key {
                private_key: key.private_openssh,
                passphrase: None,
            },
            engine_port,
            host_key_fingerprint: Some(fingerprint),
            endpoints: Vec::new(),
        };
        Machine {
            target,
            opened,
            link,
        }
    }

    pub(super) fn client(m: &Machine, dir: &std::path::Path) -> zeron_client::Client {
        let mut config = zeron_client::ClientConfig::new("https://edge.invalid", dir);
        config.device_id = "android-sim".into();
        config.platform = "android".into();
        zeron_client::Client::new(
            config,
            zeron_client::Credentials::Direct(m.target.clone()),
            Arc::new(zeron_client::events::NullListener),
        )
        .unwrap()
    }
}

/// One visit pattern: how long until the opening tail and the complete
/// history show, and how many transcript channels/subscriptions it took.
async fn history_scenario(name: &str, shape: history_sim::Shape, away: Option<(f64, f64)>) {
    use history_sim::*;
    let villa = Villa::new(&shape);
    let m = machine(Arc::new(VillaEngine(villa.clone()))).await;
    let dir = tempfile::tempdir().unwrap();
    let client = client(&m, dir.path());
    let began = Instant::now();
    loop {
        if client
            .direct_status()
            .is_some_and(|s| s.phase == zeron_client::direct::DirectPhase::Live)
            && !client.workspace().sessions.is_empty()
        {
            break;
        }
        assert!(secs(began.elapsed()) < 600.0, "never linked");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let linked = secs(began.elapsed());
    let chat = client.workspace().sessions.keys().next().unwrap().clone();
    let h = client.open_session(&chat).unwrap();
    let opened_before = m.opened.load(Ordering::SeqCst);
    let t0 = Instant::now();
    h.set_view_attached(true);
    let budget = env("BUDGET", 420.0);
    let (mut tail, mut full) = (None, None);
    let mut pending_seen = false;
    let mut left = false;
    let mut back = false;
    while secs(t0.elapsed()) < budget {
        let t = secs(t0.elapsed());
        if let Some((leave_at, gone_for)) = away {
            if !left && t >= leave_at {
                h.set_view_attached(false);
                left = true;
            } else if left && !back && t >= leave_at + gone_for {
                h.set_view_attached(true);
                back = true;
            }
        }
        let snap = h.snapshot();
        let rows = snap.transcript_messages().len();
        if rows > 0 {
            tail.get_or_insert(t);
        }
        pending_seen |= history_pending(&snap);
        if rows >= villa.rows() {
            full = Some(t);
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let channels = m.opened.load(Ordering::SeqCst) - opened_before;
    println!(
        "{name:<46} link {linked:5.1}s | tail {:>7} | all {} rows {:>7} | indicator {} | transcript channels {channels}, subscriptions {}",
        fmt(tail),
        villa.rows(),
        fmt(full),
        if pending_seen { "yes" } else { "no " },
        villa.watches.load(Ordering::SeqCst),
    );
    client.shutdown();
    m.link.close_all();
}

/// Whether the snapshot says older rows are still on their way (the
/// "正在加载更早的消息…" (Loading earlier messages…) indicator).
fn history_pending(snap: &zeron_client::SessionSnapshot) -> bool {
    snap.history_pending
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn slow_link_history() {
    use history_sim::Shape;
    let check = |s: f64| Duration::from_secs_f64(s / scale());
    zeron_client::direct::set_transcript_lag_check(check(10.0), check(15.0));
    println!(
        "link: {} KB/s, one-way {} ms, time ÷{}; budget {} s",
        env("RATE", 20480.0) / 1024.0,
        env("LAT_MS", 800.0),
        scale(),
        env("BUDGET", 420.0)
    );
    let only = std::env::var("ONLY").unwrap_or_default();
    // Name, transcript, and when to leave for the home list and for how
    // long (simulated seconds), if at all.
    type Scenario = (&'static str, Shape, Option<(f64, f64)>);
    let all: Vec<Scenario> = vec![
        (
            "idle 1.7 MB, stay",
            Shape {
                full_kb: 1718,
                busy: false,
            },
            None,
        ),
        (
            "busy 2.2 MB, stay",
            Shape {
                full_kb: 2205,
                busy: true,
            },
            None,
        ),
        (
            "busy 8.4 MB, stay",
            Shape {
                full_kb: 8430,
                busy: true,
            },
            None,
        ),
        (
            "busy 2.2 MB, list for 10 s at 30 s",
            Shape {
                full_kb: 2205,
                busy: true,
            },
            Some((30.0, 10.0)),
        ),
        (
            "busy 8.4 MB, list for 10 s at 60 s",
            Shape {
                full_kb: 8430,
                busy: true,
            },
            Some((60.0, 10.0)),
        ),
        // A whale (the desktop measured a 16.4 MB opening reset): more than
        // a WebSocket message may be by default (16 MiB per frame).
        (
            "idle 17 MB whale, stay",
            Shape {
                full_kb: 17400,
                busy: false,
            },
            None,
        ),
    ];
    for (name, shape, away) in all {
        if !only.is_empty() && !name.contains(&only) {
            continue;
        }
        history_scenario(name, shape, away).await;
    }
}
