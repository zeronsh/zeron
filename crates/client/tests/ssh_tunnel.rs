//! The direct link through a real SSH tunnel (an in-process russh server
//! offering what Windows OpenSSH offers by default, "none,zlib@openssh.com",
//! forwarding direct-tcpip channels to an in-process engine):
//!
//! - the phone asks for zlib and it is used after auth;
//! - each on-screen transcript streams on its own channel, which closes as
//!   soon as the chat leaves the screen;
//! - a transcript whose channel falls behind (the engine producing more than
//!   the link carries) moves to a fresh channel and shows its newest rows;
//! - a local-profile desktop's pins are read from its settings file over an
//!   `exec` channel and show as pinned on the phone.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use russh::server::{Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use zeron_client::direct::{SshAuth, SshTarget, generate_ed25519, probe};
use zeron_rpc::{RpcError, RpcReply, RpcService};

struct Engine;

#[async_trait::async_trait]
impl RpcService for Engine {
    async fn handle(&self, method: &str, _: serde_json::Value) -> Result<RpcReply, RpcError> {
        assert_eq!(method, "EngineInfo");
        let mut text = String::new();
        let words = [
            "reasoning",
            "tool",
            "output",
            "the",
            "file",
            "crates/client/src",
            "error",
            "let",
        ];
        let mut i = 0u64;
        while text.len() < 1024 * 1024 {
            i = i
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            text.push_str(words[(i >> 60) as usize % words.len()]);
            text.push_str(&format!(" {:x} ", i >> 48));
        }
        Ok(RpcReply::Value(
            serde_json::json!({ "deviceId": "pc", "pad": text }),
        ))
    }
}

#[derive(Clone)]
struct Sshd {
    engine_port: u16,
    /// direct-tcpip channels opened / finished.
    opened: Arc<AtomicU64>,
    closed: Arc<AtomicU64>,
    /// What `exec` prints (the desktop's `ui-settings.json`); `None`: the
    /// command fails. Every command run is recorded.
    settings: Arc<std::sync::Mutex<Option<String>>>,
    execs: Arc<std::sync::Mutex<Vec<String>>>,
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
        let closed = self.closed.clone();
        tokio::spawn(async move {
            let mut engine = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let mut stream = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut engine).await;
            let _ = engine.shutdown().await;
            closed.fetch_add(1, Ordering::SeqCst);
        });
        Ok(())
    }

    async fn channel_close(&mut self, _: ChannelId, _: &mut Session) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.execs
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(data).into_owned());
        session.channel_success(channel)?;
        let settings = self.settings.lock().unwrap().clone();
        match settings {
            Some(text) => {
                session.data(channel, text.into_bytes())?;
                session.exit_status_request(channel, 0)?;
            }
            None => session.exit_status_request(channel, 1)?,
        }
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

/// Count bytes server → phone on the wire, carrying at most `rate` bytes/s
/// that way when set.
async fn counting_proxy(to: u16, down: Arc<AtomicU64>, rate: Option<u64>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (client, _) = listener.accept().await.unwrap();
            let server = TcpStream::connect(("127.0.0.1", to)).await.unwrap();
            let (mut cr, mut cw) = client.into_split();
            let (mut sr, mut sw) = server.into_split();
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut cr, &mut sw).await;
            });
            let down = down.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; if rate.is_some() { 8 * 1024 } else { 64 * 1024 }];
                loop {
                    use tokio::io::AsyncReadExt;
                    let n = match sr.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    down.fetch_add(n as u64, Ordering::Relaxed);
                    if cw.write_all(&buf[..n]).await.is_err() {
                        return;
                    }
                    if let Some(rate) = rate {
                        tokio::time::sleep(std::time::Duration::from_secs_f64(
                            n as f64 / rate as f64,
                        ))
                        .await;
                    }
                }
            });
        }
    });
    port
}

struct Machine {
    target: SshTarget,
    down: Arc<AtomicU64>,
    opened: Arc<AtomicU64>,
    closed: Arc<AtomicU64>,
    settings: Arc<std::sync::Mutex<Option<String>>>,
    execs: Arc<std::sync::Mutex<Vec<String>>>,
}

/// An engine behind an SSH server, and how the phone reaches it.
async fn machine(
    engine: Arc<dyn RpcService>,
    offer: &'static [russh::compression::Name],
) -> Machine {
    machine_at(engine, offer, None).await
}

/// [`machine`] behind a link carrying at most `rate` bytes/s to the phone.
async fn machine_at(
    engine: Arc<dyn RpcService>,
    offer: &'static [russh::compression::Name],
    rate: Option<u64>,
) -> Machine {
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
    let config = Arc::new(russh::server::Config {
        keys: vec![host_key],
        auth_rejection_time: std::time::Duration::from_millis(1),
        preferred: russh::Preferred {
            compression: std::borrow::Cow::Borrowed(offer),
            ..russh::Preferred::DEFAULT
        },
        ..Default::default()
    });
    let sshd = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let sshd_port = sshd.local_addr().unwrap().port();
    let opened = Arc::new(AtomicU64::new(0));
    let closed = Arc::new(AtomicU64::new(0));
    let settings = Arc::new(std::sync::Mutex::new(None));
    let execs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut server = Sshd {
        engine_port,
        opened: opened.clone(),
        closed: closed.clone(),
        settings: settings.clone(),
        execs: execs.clone(),
    };
    tokio::spawn(async move {
        let _ = server.run_on_socket(config, &sshd).await;
    });
    let down = Arc::new(AtomicU64::new(0));
    let port = counting_proxy(sshd_port, down.clone(), rate).await;

    let key = generate_ed25519("test").unwrap();
    let target = SshTarget {
        host: "127.0.0.1".into(),
        port,
        user: "villa".into(),
        auth: SshAuth::Key {
            private_key: key.private_openssh,
            passphrase: None,
        },
        engine_port,
        host_key_fingerprint: Some(fingerprint),
        endpoints: Vec::new(),
    };
    Machine {
        target,
        down,
        opened,
        closed,
        settings,
        execs,
    }
}

async fn run(offer: &'static [russh::compression::Name]) -> (u64, usize) {
    let m = machine(Arc::new(Engine), offer).await;
    let result = probe(&m.target).await.expect("probe over the tunnel");
    assert_eq!(result.engine_device_id, "pc");
    (m.down.load(Ordering::Relaxed), 1024 * 1024)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tunnel_is_compressed_when_the_server_offers_zlib() {
    // What Windows OpenSSH offers with its default `Compression delayed`.
    let (zlib, payload) = run(&[russh::compression::NONE, russh::compression::ZLIB_LEGACY]).await;
    // A server with compression off.
    let (plain, _) = run(&[russh::compression::NONE]).await;
    println!(
        "server → phone for a ~{} KB reply: zlib {} KB, none {} KB",
        payload / 1024,
        zlib / 1024,
        plain / 1024
    );
    assert!(
        plain as usize > payload,
        "uncompressed carries the whole reply"
    );
    assert!(
        zlib * 2 < plain,
        "zlib@openssh.com is negotiated and used after auth"
    );
}

// ── transcripts on their own channels ─────────────────────────────────────

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

fn entry(id: &str) -> serde_json::Value {
    serde_json::to_value(zeron_doc::SessionMessageEntry {
        id: id.into(),
        role: zeron_doc::MessageRole::Assistant,
        parts: vec![zeron_doc::MessagePart::Text {
            id: format!("{id}-t"),
            text: format!("text of {id}"),
        }],
        created_at: 1,
        device_id: "pc".into(),
        status: None,
        continuation_of: None,
        duration_ms: None,
    })
    .unwrap()
}

/// A 0.2.101-shaped engine: registry snapshots, and `WatchDocMessages`
/// with `openingTail` (tail, then the complete reset, then silence).
/// Records every transcript watch and when its stream ends.
struct Workspace(Arc<std::sync::Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl RpcService for Workspace {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        use futures::StreamExt;
        match method {
            "EngineInfo" | "ListHarnesses" | "ListModels" => Ok(RpcReply::Value(fixture(method))),
            "WatchDevices" | "WatchSpaces" | "WatchChats" | "WatchSessions" => {
                Ok(RpcReply::Stream(
                    futures::stream::iter([fixture(method)])
                        .chain(futures::stream::pending())
                        .boxed(),
                ))
            }
            // What 0.2.101 answers on a local (signed-out) profile.
            "WatchSidebarPreferences" => Ok(RpcReply::Stream(
                futures::stream::iter([serde_json::json!({
                    "revision": 0, "synced": false, "initialized": false,
                    "pinnedSessionIds": []
                })])
                .chain(futures::stream::pending())
                .boxed(),
            )),
            "WatchDocMessages" => {
                let chat = params["chatId"].as_str().unwrap_or_default().to_owned();
                self.0.lock().unwrap().push(format!("watch {chat}"));
                struct Unwatch(Arc<std::sync::Mutex<Vec<String>>>, String);
                impl Drop for Unwatch {
                    fn drop(&mut self) {
                        self.0.lock().unwrap().push(format!("unwatch {}", self.1));
                    }
                }
                let guard = Unwatch(self.0.clone(), chat);
                let frames = [
                    serde_json::json!({ "reset": [entry("m3")], "historyPending": true }),
                    serde_json::json!({ "reset": [entry("m1"), entry("m2"), entry("m3")] }),
                ];
                Ok(RpcReply::Stream(
                    futures::stream::iter(frames)
                        .chain(futures::stream::pending())
                        .map(move |v| {
                            let _ = &guard;
                            v
                        })
                        .boxed(),
                ))
            }
            _ => Ok(RpcReply::Value(serde_json::json!({}))),
        }
    }
}

/// The transcript tests run one at a time (the busy one loads the machine).
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn eventually(what: &str, secs: u64, ok: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while !ok() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn each_open_transcript_has_its_own_channel_closed_on_leaving() {
    let _serial = SERIAL.lock().await;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let m = machine(
        Arc::new(Workspace(seen.clone())),
        &[russh::compression::NONE, russh::compression::ZLIB_LEGACY],
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = zeron_client::ClientConfig::new("https://edge.invalid", dir.path());
    config.device_id = "android-test".into();
    config.platform = "android".into();
    let client = zeron_client::Client::new(
        config,
        zeron_client::Credentials::Direct(m.target.clone()),
        Arc::new(zeron_client::events::NullListener),
    )
    .unwrap();
    eventually("live", 20, || {
        client
            .direct_status()
            .is_some_and(|s| s.phase == zeron_client::direct::DirectPhase::Live)
    })
    .await;
    // Feed, requests, and the spare for the first transcript.
    eventually("three channels at connect", 10, || {
        m.opened.load(Ordering::SeqCst) >= 3
    })
    .await;
    let chats: Vec<String> = client
        .workspace()
        .sessions
        .keys()
        .take(2)
        .cloned()
        .collect();
    let ids = |h: &zeron_client::SessionHandle| -> Vec<String> {
        h.snapshot()
            .transcript_messages()
            .iter()
            .map(|m| m.id.clone())
            .collect()
    };

    let a = client.open_session(&chats[0]).unwrap();
    a.set_view_attached(true);
    eventually("chat A's rows", 10, || ids(&a) == ["m1", "m2", "m3"]).await;
    // A took the spare; a new spare is opened for the next chat.
    eventually("spare refilled", 10, || {
        m.opened.load(Ordering::SeqCst) >= 4
    })
    .await;
    let closed_before = m.closed.load(Ordering::SeqCst);

    // Leaving A closes its channel at once (not at the next 5 s beat).
    let left = std::time::Instant::now();
    a.set_view_attached(false);
    eventually("A's channel closed", 3, || {
        m.closed.load(Ordering::SeqCst) > closed_before
    })
    .await;
    eventually("A's stream ended", 3, || {
        seen.lock()
            .unwrap()
            .iter()
            .any(|e| e == &format!("unwatch {}", chats[0]))
    })
    .await;
    println!(
        "A's channel closed {} ms after leaving",
        left.elapsed().as_millis()
    );

    let b = client.open_session(&chats[1]).unwrap();
    b.set_view_attached(true);
    eventually("chat B's rows", 10, || ids(&b) == ["m1", "m2", "m3"]).await;
    assert_eq!(
        ids(&a),
        ["m1", "m2", "m3"],
        "A keeps what it showed after its channel closed"
    );
    println!(
        "channels opened {}, closed {}; engine saw {:?}",
        m.opened.load(Ordering::SeqCst),
        m.closed.load(Ordering::SeqCst),
        seen.lock().unwrap()
    );
    client.shutdown();
}

// ── a transcript that falls behind ─────────────────────────────────────────

/// A chat running a busy agentic turn: its live row is re-sent whole every
/// 10 ms (~100 KB, like the ~484 KB rows Villa's engine re-sends on every
/// tool update), its last part named for when it was written (ms since
/// `start`). Every subscription opens with the tail and the complete
/// transcript as they are now.
struct Busy {
    start: std::time::Instant,
    pad: Arc<String>,
    /// Set when the test is done: the streams end.
    stopped: std::sync::atomic::AtomicBool,
}

impl Busy {
    fn live(&self) -> serde_json::Value {
        // A big part that stays, and a small last one that changes (as a
        // running tool call's does).
        let v = self.start.elapsed().as_millis() as u64;
        let mut row = entry("live");
        row["parts"] = serde_json::json!([
            { "kind": "text", "id": "live-pad", "text": self.pad.as_str() },
            { "kind": "text", "id": format!("v{v}"), "text": "tool output" },
        ]);
        row
    }
}

struct BusyEngine(Arc<Busy>);

#[async_trait::async_trait]
impl RpcService for BusyEngine {
    async fn handle(&self, method: &str, params: serde_json::Value) -> Result<RpcReply, RpcError> {
        use futures::StreamExt;
        match method {
            "EngineInfo" | "ListHarnesses" | "ListModels" => Ok(RpcReply::Value(fixture(method))),
            "WatchDevices" | "WatchSpaces" | "WatchChats" | "WatchSessions" => {
                Ok(RpcReply::Stream(
                    futures::stream::iter([fixture(method)])
                        .chain(futures::stream::pending())
                        .boxed(),
                ))
            }
            "WatchDocMessages" => {
                let _ = params;
                let opening = [
                    serde_json::json!({ "reset": [self.0.live()], "historyPending": true }),
                    serde_json::json!({ "reset": [entry("m1"), self.0.live()] }),
                ];
                let busy = self.0.clone();
                // Like the engine's own stream: the next copy is made when
                // the last one is handed to the connection's queue.
                let deltas = futures::stream::unfold(busy, |busy| async move {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    if busy.stopped.load(Ordering::Relaxed) {
                        return None;
                    }
                    let frame = serde_json::json!({
                        "upsert": [{ "after": "m1", "entry": busy.live() }],
                        "append": [], "remove": [], "count": 2
                    });
                    Some((frame, busy))
                });
                Ok(RpcReply::Stream(
                    futures::stream::iter(opening).chain(deltas).boxed(),
                ))
            }
            _ => Ok(RpcReply::Value(serde_json::json!({}))),
        }
    }
}

/// Regression (round5-13 "最近的都没有" — none of the newest rows): when a
/// running chat produces more than the link carries, the engine queues up
/// to 256 frames per connection and the phone shows the chat as it was
/// further and further back. The transcript's channel is checked for
/// backlog and swapped for a fresh one (an empty engine queue, the newest
/// rows first), so what's shown stays within seconds of the engine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transcript_that_falls_behind_catches_up_on_a_fresh_channel() {
    use std::time::{Duration, Instant};
    let _serial = SERIAL.lock().await;
    zeron_client::direct::set_transcript_lag_check(
        Duration::from_millis(300),
        Duration::from_millis(1000),
    );
    let mut pad = String::new();
    let mut i = 7u64;
    while pad.len() < 100 * 1024 {
        i = i
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        pad.push_str(&format!("{:016x}", i));
    }
    let start = Instant::now();
    let busy = Arc::new(Busy {
        start,
        pad: Arc::new(pad),
        stopped: std::sync::atomic::AtomicBool::new(false),
    });
    // ~10 MB/s of transcript (about 5 MB/s deflated) over a 1 MB/s link.
    let m = machine_at(
        Arc::new(BusyEngine(busy.clone())),
        &[russh::compression::NONE, russh::compression::ZLIB_LEGACY],
        Some(1_000_000),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = zeron_client::ClientConfig::new("https://edge.invalid", dir.path());
    config.device_id = "android-test".into();
    config.platform = "android".into();
    let client = zeron_client::Client::new(
        config,
        zeron_client::Credentials::Direct(m.target.clone()),
        Arc::new(zeron_client::events::NullListener),
    )
    .unwrap();
    eventually("live", 20, || {
        client
            .direct_status()
            .is_some_and(|s| s.phase == zeron_client::direct::DirectPhase::Live)
    })
    .await;
    eventually("chats", 10, || !client.workspace().sessions.is_empty()).await;
    let chat = client.workspace().sessions.keys().next().unwrap().clone();
    let h = client.open_session(&chat).unwrap();
    h.set_view_attached(true);
    // When the copy of the live row on screen was written (ms since start).
    let shown = |h: &zeron_client::SessionHandle| -> Option<u64> {
        let rows = h.snapshot().transcript_messages();
        let row = rows.iter().find(|r| r.id == "live")?;
        row.parts.last()?.id().strip_prefix('v')?.parse().ok()
    };
    eventually("the live row", 20, || shown(&h).is_some()).await;
    let opened_before = m.opened.load(Ordering::SeqCst);
    // Let a backlog build (without the check it only grows: ~4 s behind
    // per 5 s), then watch how far behind the screen is.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let mut worst = 0u64;
    let until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < until {
        let now = start.elapsed().as_millis() as u64;
        if let Some(v) = shown(&h) {
            worst = worst.max(now.saturating_sub(v));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let fresh = m.opened.load(Ordering::SeqCst) - opened_before;
    println!(
        "worst lag {worst} ms over the last 8 s; {fresh} channels opened meanwhile; {} KB down",
        m.down.load(Ordering::Relaxed) / 1024
    );
    client.shutdown();
    busy.stopped.store(true, Ordering::Relaxed);
    zeron_client::direct::set_transcript_lag_check(
        Duration::from_secs(10),
        Duration::from_secs(15),
    );
    assert!(fresh > 0, "the backlogged channel was replaced");
    assert!(
        worst < 6_000,
        "the screen stays within seconds of the engine (worst {worst} ms)"
    );
}

// ── the desktop's pins ─────────────────────────────────────────────────────

/// Regression (desktop pins never reached the phone): a desktop on a local
/// profile keeps its pins only in its own `ui-settings.json`; its engine
/// holds none. The phone reads that file (read-only, over an `exec`
/// channel) and shows those chats as pinned.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_desktops_pins_are_read_from_its_settings_file() {
    let _serial = SERIAL.lock().await;
    let chats = fixture("WatchChats");
    let pinned: String = chats
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["archived"] != true && c["parentChatId"].is_null() && c["spaceId"].is_null())
        .map(|c| c["id"].as_str().unwrap().to_owned())
        .unwrap();
    let m = machine(
        Arc::new(Workspace(Arc::new(std::sync::Mutex::new(Vec::new())))),
        &[russh::compression::NONE, russh::compression::ZLIB_LEGACY],
    )
    .await;
    // Shaped like Villa's (2026-10-02), BOM included.
    *m.settings.lock().unwrap() = Some(format!(
        "\u{feff}{{\"sidebarPinnedSessionIdsByProfile\":{{\"local\":[\"{pinned}\",\"gone-chat\"]}},\"sidebarSectionsByProfile\":{{\"local\":[]}}}}"
    ));
    let dir = tempfile::tempdir().unwrap();
    let mut config = zeron_client::ClientConfig::new("https://edge.invalid", dir.path());
    config.device_id = "android-test".into();
    config.platform = "android".into();
    let client = zeron_client::Client::new(
        config,
        zeron_client::Credentials::Direct(m.target.clone()),
        Arc::new(zeron_client::events::NullListener),
    )
    .unwrap();
    eventually("the desktop's pin", 20, || {
        client
            .workspace()
            .session(&pinned)
            .is_some_and(|r| r.pinned)
    })
    .await;
    let ws = client.workspace();
    assert_eq!(
        ws.front
            .pinned
            .iter()
            .map(|r| r.id.clone())
            .collect::<Vec<_>>(),
        [pinned.clone()]
    );
    // The device fixture is a Windows machine.
    let execs = m.execs.lock().unwrap().clone();
    assert_eq!(
        execs.first().map(String::as_str),
        Some(r#"cmd /c type "%LOCALAPPDATA%\Zeron\ui-settings.json""#),
        "{execs:?}"
    );
    client.shutdown();
}
